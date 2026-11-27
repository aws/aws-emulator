// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListImagePackages`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a read of an image build version's packages reports the inventory
//! that build version holds, in one page bounded by `maxResults` and continued by a
//! `nextToken` that belongs to that listing alone; a build version the calling Region
//! holds no inventory for reports an empty list rather than a refusal.
//!
//! The rows span what the rule generalises over: how many packages an inventory holds
//! (none, one, a few, more than one page can carry), the order they were recorded in
//! against the order they are reported in, the page size the request asks for, which
//! Region asks, and which listing a token was issued for.
//!
//! The inventory is recorded through the store's own write path
//! (`ComAmazonawsImagebuilderImagebuilderState::insert_image`), which is where the image
//! build path files what a build observed: Systems Manager Inventory is taken on the
//! instance a build ran on, so nothing a request carries can derive it. That write path's
//! own bounds — how many build versions the store holds and how many packages one record
//! keeps — are exercised by the last two cases.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    image_build_version_arn, ComAmazonawsImagebuilderImagebuilderState, ImageBuildVersionRecord,
    MAX_IMAGES, MAX_IMAGE_PACKAGES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The largest page size a request may ask for: the modelled `maxResults` range's own
/// maximum (`smithy_query(query="operation",operation="ListImagePackages")` reports
/// `maxResults` range 1..100, and the member documents "The largest page size that you can
/// request is 100 items").
const MAX_PACKAGES_PER_PAGE: usize = 100;

/// The most packages one page carries when the request asks for no page size, from the same
/// member's documentation: "If you don't specify a value, Image Builder returns up to 25
/// items."
const DEFAULT_PACKAGES_PER_PAGE: usize = 25;

/// The smallest page size a request may ask for: that same range's minimum.
const MIN_PACKAGES_PER_PAGE: usize = 1;

/// One row of the inventory table: the case, the image version and build number the
/// inventory was recorded for, and the packages it holds, in the order they were recorded.
type InventoryRow<'a> = (&'a str, &'a str, u64, Vec<(&'a str, &'a str)>);

/// One emulator with an empty store, mounted exactly as the router mounts it, beside the
/// shared engine a build path would file its records in.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handler reads the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `ListImagePackages` with `body`, in `region`.
async fn list_packages_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, region, "POST", "/ListImagePackages", Some(body)).await
}

/// `ListImagePackages` for `arn` with an optional page size and token, in `region`.
async fn list_packages_page(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("imageBuildVersionArn".to_string(), json!(arn));
    if let Some(max_results) = max_results {
        body.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        body.insert("nextToken".to_string(), json!(token));
    }
    list_packages_in(router, region, &Value::Object(body)).await
}

/// File one image build version's inventory in the store, the way the image build path
/// does, and answer the build version ARN a caller reads it back by.
fn recorded_image(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    name: &str,
    version: &str,
    build_version: u64,
    packages: &[(&str, &str)],
) -> String {
    let arn = image_build_version_arn(region, name, version, build_version);
    let record = ImageBuildVersionRecord {
        arn: arn.clone(),
        region: region.to_string(),
        packages: packages
            .iter()
            .map(|(package, package_version)| {
                ((*package).to_string(), (*package_version).to_string())
            })
            .collect::<BTreeMap<String, String>>(),
        ..ImageBuildVersionRecord::default()
    };
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(record);
    assert!(
        filed,
        "case recording {arn}: expected the store to file the build version"
    );
    arn
}

/// One member of an answer, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member in {body}"))
}

/// The packages one page carries, in the order it carried them.
fn page_packages(case: &str, body: &Value) -> Vec<(String, String)> {
    member(case, body, "imagePackageList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list in {body}"))
        .iter()
        .map(|package| {
            let name = package
                .get("packageName")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a packageName in {package}"))
                .to_string();
            let version = package
                .get("packageVersion")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a packageVersion in {package}"))
                .to_string();
            (name, version)
        })
        .collect()
}

/// The `nextToken` one page carries, if any.
fn page_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// The inventory as it is REPORTED: package name order, which is the order the store holds
/// it in.
fn reported_order(packages: &[(&str, &str)]) -> Vec<(String, String)> {
    let ordered: BTreeMap<String, String> = packages
        .iter()
        .map(|(package, version)| ((*package).to_string(), (*version).to_string()))
        .collect();
    ordered.into_iter().collect()
}

/// An inventory of `count` packages, named so that their recorded order is not their
/// reported order.
fn generated_inventory(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            (
                format!("pkg-{:02}", count - index),
                format!("{}.{}.0", index, count),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_read_reports_the_inventory_the_build_version_holds_one_page_at_a_time() {
    let (router, engine) = emulator();

    // An inventory recorded out of name order, with package names and versions in the
    // forms a built image carries them (the modelled `ImagePackage` carries `packageName`
    // and `packageVersion` as free text, so the forms below are the ones an AMI's
    // inventory holds rather than a modelled catalog).
    let few: Vec<(&str, &str)> = vec![
        ("zlib", "1.2.11-29.amzn2.0.1"),
        ("amazon-ssm-agent", "3.2.1377.0-1.amzn2"),
        ("openssl11-libs", "1:1.1.1g-12.amzn2.0.16"),
    ];
    // One package more than a single page can carry, so the listing spans two pages even
    // at the largest page size.
    let one_page_over = generated_inventory(MAX_PACKAGES_PER_PAGE + 1);
    let one_page_over: Vec<(&str, &str)> = one_page_over
        .iter()
        .map(|(package, version)| (package.as_str(), version.as_str()))
        .collect();

    // Each row is one build version: how many packages it holds and in which order they
    // were recorded.
    let rows: Vec<InventoryRow> = vec![
        ("an inventory of no packages", "1.0.0", 1, Vec::new()),
        (
            "an inventory of one package",
            "1.0.0",
            2,
            vec![("curl", "7.88.1-1.amzn2.0.1")],
        ),
        ("an inventory of a few packages", "2.3.4", 1, few.clone()),
        (
            "an inventory one page cannot carry",
            "10.0.0",
            7,
            one_page_over.clone(),
        ),
    ];

    for (case, version, build_version, packages) in &rows {
        let name = format!("image-{}-{build_version}", version.replace('.', "-"));
        let arn = recorded_image(
            &engine,
            REGION,
            &name,
            version,
            *build_version,
            packages,
        );
        let expected = reported_order(packages);

        // With no page size the answer carries the documented default, and a token only
        // while a package it did not carry remains.
        let (status, listed) = list_packages_page(&router, REGION, &arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let first_page = page_packages(case, &listed);
        let head: Vec<(String, String)> = expected
            .iter()
            .take(DEFAULT_PACKAGES_PER_PAGE)
            .cloned()
            .collect();
        assert_eq!(
            first_page, head,
            "case {case:?}: expected {head:?}, got {first_page:?}"
        );
        assert_eq!(
            page_token(&listed).is_some(),
            expected.len() > DEFAULT_PACKAGES_PER_PAGE,
            "case {case:?}: expected a nextToken only with packages left over, got {listed}"
        );
        let request_id = member(case, &listed, "requestId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !request_id.is_empty(),
            "case {case:?}: expected a requestId naming the answer, got {listed}"
        );

        // A read changes nothing a later read sees: the same call answers the same page.
        let (_, again) = list_packages_page(&router, REGION, &arn, None, None).await;
        let repeated = page_packages(case, &again);
        assert_eq!(
            repeated, first_page,
            "case {case:?}: expected a read to leave the inventory alone, got {repeated:?} \
             after {first_page:?}"
        );

        // Every page size the model admits, walked to the end: the pages together are the
        // whole inventory, in order, and none is longer than the size asked for.
        for page_size in [
            MIN_PACKAGES_PER_PAGE,
            2,
            7,
            DEFAULT_PACKAGES_PER_PAGE,
            MAX_PACKAGES_PER_PAGE,
        ] {
            let mut walked: Vec<(String, String)> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let (status, page) =
                    list_packages_page(&router, REGION, &arn, Some(page_size), token.as_deref())
                        .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} at {page_size} per page: expected HTTP 200, got {status} \
                     with {page}"
                );
                let entries = page_packages(case, &page);
                assert!(
                    entries.len() <= page_size,
                    "case {case:?} at {page_size} per page: expected at most {page_size} \
                     entries, got {entries:?}"
                );
                walked.extend(entries);
                pages += 1;
                token = page_token(&page);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= expected.len(),
                    "case {case:?} at {page_size} per page: expected the walk to end, got \
                     {pages} pages for {} packages",
                    expected.len()
                );
            }
            assert_eq!(
                walked, expected,
                "case {case:?} at {page_size} per page: expected {expected:?}, got {walked:?}"
            );
            let want_pages = expected.len().div_ceil(page_size).max(1);
            assert_eq!(
                pages, want_pages,
                "case {case:?} at {page_size} per page: expected {want_pages} page(s), got \
                 {pages}"
            );
        }
    }

    // Two build versions hold two inventories: each read reports its own.
    let other = recorded_image(
        &engine,
        REGION,
        "image-2-3-4-1",
        "2.3.4",
        2,
        &[("tzdata", "2024a-1.amzn2.0.1")],
    );
    let (status, listed) = list_packages_page(&router, REGION, &other, None, None).await;
    assert_eq!(
        status, 200,
        "case a second build version: expected HTTP 200, got {status} with {listed}"
    );
    let entries = page_packages("a second build version", &listed);
    assert_eq!(
        entries,
        vec![("tzdata".to_string(), "2024a-1.amzn2.0.1".to_string())],
        "case a second build version: expected its own inventory, got {entries:?}"
    );
}

#[tokio::test]
async fn a_build_version_this_region_holds_no_inventory_for_reports_no_packages() {
    let (router, engine) = emulator();

    // One build version really is recorded, in one Region, so the empty answers below are
    // about the build version asked for and not about an empty store.
    let recorded = recorded_image(
        &engine,
        REGION,
        "recorded-image",
        "1.0.0",
        1,
        &[("bash", "4.2.46-35.amzn2")],
    );

    // Build version ARNs nothing recorded, differing in name, version and build number —
    // plus the recorded one read through another Region's endpoint, which reaches only its
    // own Region's resources.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a build version of a name nothing built",
            image_build_version_arn(REGION, "absent-image", "1.0.0", 1),
            REGION,
        ),
        (
            "another version of a name that was built",
            image_build_version_arn(REGION, "recorded-image", "2.0.0", 1),
            REGION,
        ),
        (
            "another build number of a version that was built",
            image_build_version_arn(REGION, "recorded-image", "1.0.0", 4),
            REGION,
        ),
        (
            "a build version of another Region",
            image_build_version_arn(OTHER_REGION, "recorded-image", "1.0.0", 1),
            OTHER_REGION,
        ),
        (
            "a recorded build version read from another Region's endpoint",
            recorded.clone(),
            OTHER_REGION,
        ),
    ];

    for (case, arn, region) in &rows {
        let (status, listed) = list_packages_page(&router, region, arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let entries = page_packages(case, &listed);
        assert!(
            entries.is_empty(),
            "case {case:?}: expected no packages, got {entries:?}"
        );
        assert_eq!(
            page_token(&listed),
            None,
            "case {case:?}: expected no nextToken, got {listed}"
        );
    }

    // And the recorded build version still reports its inventory, so none of the reads
    // above removed anything.
    let (status, listed) = list_packages_page(&router, REGION, &recorded, None, None).await;
    assert_eq!(
        status, 200,
        "case the recorded build version: expected HTTP 200, got {status} with {listed}"
    );
    let entries = page_packages("the recorded build version", &listed);
    assert_eq!(
        entries,
        vec![("bash".to_string(), "4.2.46-35.amzn2".to_string())],
        "case the recorded build version: expected its inventory, got {entries:?}"
    );
}

#[tokio::test]
async fn a_token_belongs_to_the_listing_that_issued_it() {
    let (router, engine) = emulator();

    let packages: Vec<(&str, &str)> = vec![
        ("acl", "2.2.51-14.amzn2"),
        ("binutils", "2.29.1-31.amzn2"),
        ("coreutils", "8.22-24.amzn2"),
    ];
    let first = recorded_image(&engine, REGION, "token-image-one", "1.0.0", 1, &packages);
    let second = recorded_image(
        &engine,
        REGION,
        "token-image-two",
        "1.0.0",
        1,
        &[("dhclient", "12:4.2.5-79.amzn2.0.2")],
    );

    let (status, page) = list_packages_page(&router, REGION, &first, Some(1), None).await;
    assert_eq!(
        status, 200,
        "case the first page: expected HTTP 200, got {status} with {page}"
    );
    let issued = page_token(&page)
        .unwrap_or_else(|| panic!("case the first page: expected a nextToken in {page}"));

    // A token this operation did not issue for THIS listing is refused, however it is
    // wrong: fabricated, edited, spent on another image, or spent in another Region.
    let edited = format!("{issued}00");
    let refusals: Vec<(&str, &str, &str, Option<usize>)> = vec![
        ("a fabricated token", "not-a-token", REGION, Some(1)),
        ("an edited token", edited.as_str(), REGION, Some(1)),
        (
            "a token issued for another image",
            issued.as_str(),
            REGION,
            Some(1),
        ),
        (
            "a token issued in another Region",
            issued.as_str(),
            OTHER_REGION,
            Some(1),
        ),
    ];
    for (case, token, region, page_size) in &refusals {
        let arn = if *case == "a token issued for another image" {
            second.as_str()
        } else {
            first.as_str()
        };
        let (status, refused) =
            list_packages_page(&router, region, arn, *page_size, Some(token)).await;
        assert_refused(
            case,
            status,
            &refused,
            400,
            "InvalidParameterValueException",
        );
    }

    // The token it did issue continues that listing, whatever page size the next request
    // asks for: the page size says how much of the listing to answer with, not which
    // listing it is.
    for page_size in [MIN_PACKAGES_PER_PAGE, MAX_PACKAGES_PER_PAGE] {
        let (status, page) =
            list_packages_page(&router, REGION, &first, Some(page_size), Some(&issued)).await;
        assert_eq!(
            status, 200,
            "case continuing at {page_size} per page: expected HTTP 200, got {status} with \
             {page}"
        );
        let entries = page_packages("continuing a listing", &page);
        let expected: Vec<(String, String)> = reported_order(&packages)
            .into_iter()
            .skip(1)
            .take(page_size)
            .collect();
        assert_eq!(
            entries, expected,
            "case continuing at {page_size} per page: expected {expected:?}, got {entries:?}"
        );
    }

    // Each published `maxResults` bound is served, and one past either bound is refused by
    // the modelled range before the handler runs — as the parameter value it is, which is
    // the class this service refuses every value its model does not admit with.
    for page_size in [MIN_PACKAGES_PER_PAGE, MAX_PACKAGES_PER_PAGE] {
        let (status, page) = list_packages_page(&router, REGION, &first, Some(page_size), None).await;
        assert_eq!(
            status, 200,
            "case the page size {page_size}: expected HTTP 200, got {status} with {page}"
        );
    }
    for page_size in [
        MIN_PACKAGES_PER_PAGE - 1,
        MAX_PACKAGES_PER_PAGE + 1,
    ] {
        let (status, refused) =
            list_packages_page(&router, REGION, &first, Some(page_size), None).await;
        assert_refused(
            &format!("the page size {page_size}"),
            status,
            &refused,
            400,
            "InvalidParameterValueException",
        );
    }

    // An ARN that is not a full build version ARN is refused as the parameter value it is:
    // the input "must be a full build version ARN", and a value a modelled ARN pattern does
    // not admit is this service's `InvalidParameterValueException`.
    let not_build_versions: Vec<(&str, String)> = vec![
        (
            "an image version ARN carrying no build number",
            format!("arn:aws:imagebuilder:{REGION}:000000000000:image/token-image-one/1.0.0"),
        ),
        (
            "an image recipe ARN",
            format!(
                "arn:aws:imagebuilder:{REGION}:000000000000:image-recipe/token-image-one/1.0.0"
            ),
        ),
        ("text that is no ARN at all", "token-image-one".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:{REGION}:000000000000:image/ami-00000000000000000"),
        ),
    ];
    for (case, arn) in &not_build_versions {
        let (status, refused) = list_packages_page(&router, REGION, arn, None, None).await;
        assert_refused(case, status, &refused, 400, "InvalidParameterValueException");
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("imageBuildVersionArn"),
            "case {case:?}: expected the refused parameter named, got {refused}"
        );
    }
}

#[tokio::test]
async fn the_store_holds_at_most_one_record_per_build_version_up_to_its_bound() {
    let (router, engine) = emulator();

    // Fill the store to its bound, then ask it for one more build version.
    for index in 0..MAX_IMAGES {
        let arn = image_build_version_arn(REGION, &format!("bounded-{index}"), "1.0.0", 1);
        let filed = engine
            .state::<ComAmazonawsImagebuilderImagebuilderState>()
            .lock()
            .insert_image(ImageBuildVersionRecord {
                arn,
                region: REGION.to_string(),
                packages: [("bash".to_string(), "4.2.46-35.amzn2".to_string())]
                    .into_iter()
                    .collect(),
                ..ImageBuildVersionRecord::default()
            });
        assert!(
            filed,
            "case build version {index}: expected the store to file it below its bound"
        );
    }

    let over_bound = image_build_version_arn(REGION, "over-the-bound", "1.0.0", 1);
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(ImageBuildVersionRecord {
            arn: over_bound.clone(),
            region: REGION.to_string(),
            packages: [("curl".to_string(), "7.88.1-1.amzn2.0.1".to_string())]
                .into_iter()
                .collect(),
            ..ImageBuildVersionRecord::default()
        });
    assert!(
        !filed,
        "case one build version past the bound: expected the store to refuse it"
    );
    let (status, listed) = list_packages_page(&router, REGION, &over_bound, None, None).await;
    assert_eq!(
        status, 200,
        "case one build version past the bound: expected HTTP 200, got {status} with {listed}"
    );
    let entries = page_packages("one build version past the bound", &listed);
    assert!(
        entries.is_empty(),
        "case one build version past the bound: expected nothing stored for it, got {entries:?}"
    );

    // A build version the store already holds is filed whatever the count, because that
    // stores no additional record: re-recording it replaces its inventory.
    let held = image_build_version_arn(REGION, "bounded-0", "1.0.0", 1);
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(ImageBuildVersionRecord {
            arn: held.clone(),
            region: REGION.to_string(),
            packages: [("zlib".to_string(), "1.2.11-29.amzn2.0.1".to_string())]
                .into_iter()
                .collect(),
            ..ImageBuildVersionRecord::default()
        });
    assert!(
        filed,
        "case a build version the store already holds: expected it filed at the bound"
    );
    let (status, listed) = list_packages_page(&router, REGION, &held, None, None).await;
    assert_eq!(
        status, 200,
        "case a build version the store already holds: expected HTTP 200, got {status} with \
         {listed}"
    );
    let entries = page_packages("a build version the store already holds", &listed);
    assert_eq!(
        entries,
        vec![("zlib".to_string(), "1.2.11-29.amzn2.0.1".to_string())],
        "case a build version the store already holds: expected the inventory just recorded, \
         got {entries:?}"
    );
}

#[tokio::test]
async fn one_build_version_keeps_at_most_the_bounded_inventory() {
    let (router, engine) = emulator();

    // An inventory past the bound one record keeps: the record keeps the bound's worth, in
    // the package-name order it holds them in.
    let submitted: Vec<(String, String)> = (0..MAX_IMAGE_PACKAGES + 3)
        .map(|index| (format!("pkg-{index:06}"), format!("1.{index}.0")))
        .collect();
    let arn = image_build_version_arn(REGION, "over-stuffed-image", "1.0.0", 1);
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(ImageBuildVersionRecord {
            arn: arn.clone(),
            region: REGION.to_string(),
            packages: submitted.iter().cloned().collect(),
            ..ImageBuildVersionRecord::default()
        });
    assert!(filed, "case an over-stuffed inventory: expected it filed");

    let kept = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .image(&arn, REGION)
        .map(|record| record.packages.len())
        .unwrap_or_default();
    assert_eq!(
        kept, MAX_IMAGE_PACKAGES,
        "case an over-stuffed inventory: expected {MAX_IMAGE_PACKAGES} packages kept, got {kept}"
    );

    // And the read reports that inventory from its start, a page at a time.
    let (status, listed) = list_packages_page(&router, REGION, &arn, None, None).await;
    assert_eq!(
        status, 200,
        "case an over-stuffed inventory: expected HTTP 200, got {status} with {listed}"
    );
    let entries = page_packages("an over-stuffed inventory", &listed);
    let head: Vec<(String, String)> = submitted
        .iter()
        .take(DEFAULT_PACKAGES_PER_PAGE)
        .cloned()
        .collect();
    assert_eq!(
        entries, head,
        "case an over-stuffed inventory: expected {head:?}, got {entries:?}"
    );
    assert!(
        page_token(&listed).is_some(),
        "case an over-stuffed inventory: expected a nextToken, got {listed}"
    );
}
