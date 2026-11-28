// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the page size this service's paginated listings share.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is what
//! a signed AWS SDK request observes.
//!
//! The rule under test: a page carries at most as many entries as the request's `maxResults`,
//! every size the modelled range admits served as asked, and at most the page size the
//! member documents as its default when the request names none; a size outside that range is
//! refused as the parameter value it is.
//!
//! The rows span what the rule generalises over: the page size asked for (each published
//! bound, the documented default, a size either side of it, and sizes between the default
//! and the maximum), whether the listing is longer than the page, and which listing is read —
//! one of a resource's own inventory (`ListImagePackages`) and one of the Region's resources
//! (`ListImages`), the two shapes of paginated listing this service serves.
//!
//! The entries are filed through the store's own write path
//! (`ComAmazonawsImagebuilderImagebuilderState::insert_image`), which is where the image
//! build path files what a build produced and observed, so nothing a request to these
//! listings carries can derive them.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    image_build_version_arn, image_version_arn, ComAmazonawsImagebuilderImagebuilderState,
    ImageBuildVersionRecord,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// The smallest page size a request may ask for: the `maxResults` range's minimum (the
/// shape every list operation's `maxResults` targets, `RestrictedInteger`, which this
/// crate declares with the maximum the service enforces —
/// `smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`).
const MIN_PAGE_SIZE: usize = 1;

/// The largest page size a request may ask for: that same range's maximum, the page size
/// Image Builder refuses anything above.
const MAX_PAGE_SIZE: usize = 25;

/// The page size a request that names none gets, from the same member's documentation: "If
/// you don't specify a value, Image Builder returns up to 25 items."
const DEFAULT_PAGE_SIZE: usize = 25;

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The page sizes a case asks for: each published bound, the documented default and one
/// below it, and a size between — every one of them a size the declared range admits. The
/// default IS the maximum here, so there is no size above it to ask for.
const SERVED_PAGE_SIZES: &[usize] = &[
    MIN_PAGE_SIZE,
    2,
    DEFAULT_PAGE_SIZE - 1,
    DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE - 1,
    MAX_PAGE_SIZE,
];

/// One emulator with an empty store, mounted exactly as the router mounts it, beside the
/// shared engine the image build path would file its records in.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and
/// the handlers read the Region from.
fn request(method: &str, uri: &str, body: &Value) -> http::Request<bytes::Bytes> {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a well-formed test request")
}

/// Status and parsed body of one call, with the error shape the header names folded into
/// `__type` so one accessor reads either place it travels.
async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let response = router.dispatch(request("POST", uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One listing call: the operation's URI, the members that say WHICH listing, and the page
/// size and token this case reads it at.
async fn list_page(
    router: &MultiServiceRouter,
    uri: &str,
    selector: &Map<String, Value>,
    page_size: Option<usize>,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = selector.clone();
    if let Some(page_size) = page_size {
        body.insert("maxResults".to_string(), json!(page_size));
    }
    if let Some(token) = token {
        body.insert("nextToken".to_string(), json!(token));
    }
    call(router, uri, &Value::Object(body)).await
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

/// The entries one page carries, as the text of `member` on each of them, in the order the
/// page carried them.
fn page_entries(case: &str, body: &Value, list_member: &str, member: &str) -> Vec<String> {
    body.get(list_member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {list_member} member in {body}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list in {body}"))
        .iter()
        .map(|entry| {
            entry
                .get(member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a {member} in {entry}"))
                .to_string()
        })
        .collect()
}

/// Assert two listings agree, naming the case and the FIRST place they differ rather than
/// dumping both: a page of a hundred entries says nothing when it is printed whole.
fn assert_same_entries(case: &str, expected: &[String], actual: &[String]) {
    if let Some((index, (want, got))) = expected
        .iter()
        .zip(actual.iter())
        .enumerate()
        .find(|(_, (want, got))| want != got)
    {
        panic!(
            "case {case:?}: expected entry {index} to be {want:?}, got {got:?} \
             ({} entries expected, {} answered)",
            expected.len(),
            actual.len()
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "case {case:?}: expected {} entries, got {} of them, the last being {:?}",
        expected.len(),
        actual.len(),
        actual.last()
    );
}

/// One listing under test: the emulator it is read from, where it is read, which members say
/// which listing it is, which output member carries its page, which member of an entry
/// identifies it, and the entries it reports in the order it reports them.
///
/// Each listing is staged in an emulator of its OWN, so what one listing holds is never an
/// entry of the other.
struct Listing {
    router: MultiServiceRouter,
    operation: &'static str,
    uri: &'static str,
    selector: Map<String, Value>,
    list_member: &'static str,
    entry_member: &'static str,
    expected: Vec<String>,
}

/// A build version holding `count` packages, read back by `ListImagePackages`: the inventory
/// of ONE resource, reported in package-name order.
fn package_listing(count: usize) -> Listing {
    let (router, engine) = emulator();
    let arn = image_build_version_arn(REGION, "paged-inventory", "1.0.0", 1);
    let packages: BTreeMap<String, String> = (0..count)
        .map(|index| (format!("pkg-{index:04}"), format!("1.{index}.0")))
        .collect();
    let expected: Vec<String> = packages.keys().cloned().collect();
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(ImageBuildVersionRecord {
            arn: arn.clone(),
            region: REGION.to_string(),
            name: "paged-inventory".to_string(),
            version: "1.0.0".to_string(),
            build_version: 1,
            packages,
            ..ImageBuildVersionRecord::default()
        });
    assert!(
        filed,
        "case staging the inventory: expected the store to file the build version"
    );
    let mut selector = Map::new();
    selector.insert("imageBuildVersionArn".to_string(), json!(arn));
    Listing {
        router,
        operation: "ListImagePackages",
        uri: "/ListImagePackages",
        selector,
        list_member: "imagePackageList",
        entry_member: "packageName",
        expected,
    }
}

/// `count` image build versions, read back by `ListImages`: the listing of a REGION's
/// resources, reported in the order of the ARN each entry carries.
fn image_listing(count: usize) -> Listing {
    let (router, engine) = emulator();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..count {
        let name = format!("paged-image-{index:04}");
        let filed = engine
            .state::<ComAmazonawsImagebuilderImagebuilderState>()
            .lock()
            .insert_image(ImageBuildVersionRecord {
                arn: image_build_version_arn(REGION, &name, "1.0.0", 1),
                region: REGION.to_string(),
                name: name.clone(),
                version: "1.0.0".to_string(),
                build_version: 1,
                platform: Some("Linux".to_string()),
                image_type: Some("AMI".to_string()),
                ..ImageBuildVersionRecord::default()
            });
        assert!(
            filed,
            "case staging image {index}: expected the store to file the build version"
        );
        expected.push(image_version_arn(REGION, &name, "1.0.0"));
    }
    expected.sort();
    Listing {
        router,
        operation: "ListImages",
        uri: "/ListImages",
        selector: Map::new(),
        list_member: "imageVersionList",
        entry_member: "arn",
        expected,
    }
}

/// RULE: a page carries at most the request's `maxResults` entries — every size the modelled
/// range admits served as asked, the whole listing walked in one order whatever that size is
/// — and at most the documented default when the request names no page size; a size outside
/// the modelled range is refused as that parameter's value.
#[tokio::test]
async fn every_modelled_page_size_is_served_as_asked() {
    // Each listing holds more entries than the largest page can carry, so a page is bounded
    // by the size asked for rather than by the listing running out.
    let listings = vec![
        package_listing(MAX_PAGE_SIZE + 23),
        image_listing(MAX_PAGE_SIZE + 11),
    ];

    for listing in &listings {
        let router = &listing.router;
        let total = listing.expected.len();

        // With no page size the answer carries the documented default, and a `nextToken`
        // while an entry it did not carry remains.
        let case = format!("{} with no page size", listing.operation);
        let (status, page) = list_page(router, listing.uri, &listing.selector, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page}"
        );
        let entries = page_entries(&case, &page, listing.list_member, listing.entry_member);
        let head: Vec<String> = listing
            .expected
            .iter()
            .take(DEFAULT_PAGE_SIZE)
            .cloned()
            .collect();
        assert_same_entries(&case, &head, &entries);
        assert!(
            page_token(&page).is_some(),
            "case {case:?}: expected a nextToken while entries remain, got {page}"
        );

        for page_size in SERVED_PAGE_SIZES {
            let case = format!("{} at {page_size} entries per page", listing.operation);
            let (status, page) = list_page(
                router,
                listing.uri,
                &listing.selector,
                Some(*page_size),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page}"
            );
            let entries = page_entries(&case, &page, listing.list_member, listing.entry_member);
            let head: Vec<String> = listing.expected.iter().take(*page_size).cloned().collect();
            assert_same_entries(&case, &head, &entries);
            assert_eq!(
                page_token(&page).is_some(),
                *page_size < total,
                "case {case:?}: expected a nextToken only while entries remain, got {page}"
            );
        }

        // Walked to the end, the pages together are the whole listing in one order, whatever
        // page size the walk asks for.
        for page_size in [DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE] {
            let case = format!("{} walked at {page_size} per page", listing.operation);
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let (status, page) = list_page(
                    router,
                    listing.uri,
                    &listing.selector,
                    Some(page_size),
                    token.as_deref(),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {page}"
                );
                let entries = page_entries(&case, &page, listing.list_member, listing.entry_member);
                assert!(
                    entries.len() <= page_size,
                    "case {case:?}: expected at most {page_size} entries, got {} of them",
                    entries.len()
                );
                walked.extend(entries);
                pages += 1;
                token = page_token(&page);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= total,
                    "case {case:?}: expected the walk to end, read {pages} pages for {total} \
                     entries"
                );
            }
            assert_same_entries(&case, &listing.expected, &walked);
            assert_eq!(
                pages,
                total.div_ceil(page_size),
                "case {case:?}: expected {} page(s), got {pages}",
                total.div_ceil(page_size)
            );
        }

        // One step past either published bound is refused by the modelled range before the
        // handler runs — as the parameter value it is, naming the member it arrived in.
        for (case, page_size) in [
            (
                format!("{} one below the modelled minimum", listing.operation),
                MIN_PAGE_SIZE - 1,
            ),
            (
                format!("{} one above the modelled maximum", listing.operation),
                MAX_PAGE_SIZE + 1,
            ),
        ] {
            let (status, refused) = list_page(
                router,
                listing.uri,
                &listing.selector,
                Some(page_size),
                None,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {refused}"
            );
            let code = error_code(&case, &refused);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code:?} in {refused}"
            );
            let message = refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains("maxResults"),
                "case {case:?}: expected the refused parameter named, got {refused}"
            );
        }
    }
}
