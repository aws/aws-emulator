// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `ListComponentBuildVersions` enumerates.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a build-version listing enumerates exactly the component build
//! versions this service's state holds under the component version the request names — one
//! entry per stored build version, each carrying that build version's own ARN and the
//! attributes the create that stored it wrote — and a paged walk of that listing visits
//! each of those entries exactly once, in one order, whatever page size the request asks
//! for.
//!
//! The rows below therefore differ along what that rule generalises over: how many build
//! versions one component version holds (one, two, and more than nine, so the build node
//! is two digits), whether other versions and other names are stored beside them, whether
//! the request names a version at all, and the page size it asks for.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A build component document: one `build` phase, so the derived type is `BUILD`.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// The greatest number of entries one page carries when the request names no
/// `maxResults`, from the modelled `maxResults` range (`RestrictedInteger`, range 1..25).
const MAX_PAGE: usize = 25;

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in [`REGION`] — the signal the router routes on and
/// the handlers read the Region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{REGION}/imagebuilder/\
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
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// `CreateComponent` for one build version of `name` at `version`, told apart from the
/// others by `client_token` and reported by `description`.
async fn create_component(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
    client_token: &str,
    description: &str,
) {
    let body = json!({
        "name": name,
        "semanticVersion": version,
        "platform": "Linux",
        "clientToken": client_token,
        "data": BUILD_DOCUMENT,
        "description": description,
    });
    let (status, created) = call(router, "PUT", "/CreateComponent", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case creating {name:?} {version:?} ({client_token:?}): expected HTTP 200, got \
         {status} with {created}"
    );
}

/// `GetComponent` for one build version ARN.
async fn get_component(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/GetComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
}

/// `ListComponentBuildVersions` with `body`.
async fn list_build_versions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/ListComponentBuildVersions",
        Some(body),
    )
    .await
}

/// The version ARN of `normalized` at `version`: the ARN a request names, with no build
/// number suffix.
fn version_arn(normalized: &str, version: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/{normalized}/{version}")
}

/// The ARN the `build`th build version of `normalized` at `version` carries.
fn build_version_arn(normalized: &str, version: &str, build: usize) -> String {
    format!("{}/{build}", version_arn(normalized, version))
}

/// The entries of one build-version listing page.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body.get("componentSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a componentSummaryList in {body}"))
        .clone()
}

/// The ARNs one build-version listing page reports, in the order it reported them.
fn summary_arns(case: &str, body: &Value) -> Vec<String> {
    summaries(case, body)
        .iter()
        .map(|summary| {
            summary
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn in {summary}"))
                .to_string()
        })
        .collect()
}

/// The `nextToken` a page hands back, when it is truncated.
fn page_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One member of a listing entry or of a component, or a panic naming the case.
fn member(case: &str, entry: &Value, name: &str) -> Value {
    entry
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {entry}"))
}

/// `values`, sorted, so a listing is compared as the set of what it enumerated.
fn sorted(values: &[String]) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted
}

/// Walk one build-version listing to its end under `page_size`, returning every ARN it
/// reported in the order it reported them.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    query: &Value,
    page_size: usize,
    expected_total: usize,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = query.as_object().cloned().unwrap_or_default();
        body.insert("maxResults".to_string(), json!(page_size));
        if let Some(carried) = &token {
            body.insert("nextToken".to_string(), json!(carried));
        }
        let (status, page) = list_build_versions(router, &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {page}"
        );
        let arns = summary_arns(case, &page);
        assert!(
            arns.len() <= page_size,
            "case {case:?}: expected at most {page_size} entries on a page, got {arns:?}"
        );
        walked.extend(arns);
        token = page_token(&page);
        // A token is handed back only while an entry this walk has not carried remains.
        let expects_token = walked.len() < expected_total;
        assert_eq!(
            token.is_some(),
            expects_token,
            "case {case:?}: expected nextToken present={expects_token} after {} of \
             {expected_total} entries, got {page}",
            walked.len()
        );
        if token.is_none() {
            return walked;
        }
    }
}

/// RULE: a build-version listing reports one entry per stored BUILD version of the
/// component version its ARN names, each carrying that build version's own ARN and the
/// attributes the create that stored it wrote — so what a create stored is what this
/// listing enumerates, however many build versions one version holds.
#[tokio::test]
async fn a_build_version_listing_enumerates_exactly_what_the_creates_stored() {
    // The rows differ along how many build versions one component version holds: one, two,
    // and more than nine, where the build node the ARN carries is two digits.
    for held in [1_usize, 2, 12] {
        let case = format!("a version holding {held} build version(s)");
        let router = emulator();
        for build in 1..=held {
            create_component(
                &router,
                "alpha-component",
                "1.0.0",
                &format!("token-alpha-100-build-{build}"),
                &format!("build number {build}"),
            )
            .await;
        }
        // A version of the same name, and another name, so the listing is shown to report
        // the build versions of the version it names and no others.
        create_component(
            &router,
            "alpha-component",
            "1.1.0",
            "token-alpha-110",
            "the minor line",
        )
        .await;
        create_component(
            &router,
            "beta-component",
            "1.0.0",
            "token-beta-100",
            "another name",
        )
        .await;

        let (status, listed) = list_build_versions(
            &router,
            &json!({"componentVersionArn": version_arn("alpha-component", "1.0.0")}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {listed}"
        );
        let want: Vec<String> = (1..=held)
            .map(|build| build_version_arn("alpha-component", "1.0.0", build))
            .collect();
        let got = summary_arns(&case, &listed);
        assert_eq!(
            sorted(&got),
            sorted(&want),
            "case {case:?}: expected the build versions {want:?}, got {got:?}"
        );

        // Each entry carries what the create that stored it wrote, and the build version it
        // stands for is the one a read of its ARN answers: entry and read agree member for
        // member on everything both report.
        for (build, arn) in want.iter().enumerate().map(|(index, arn)| (index + 1, arn)) {
            let case = format!("{case}, the entry for build {build}");
            let summary = summaries(&case, &listed)
                .into_iter()
                .find(|summary| summary.get("arn").and_then(Value::as_str) == Some(arn.as_str()))
                .unwrap_or_else(|| panic!("case {case:?}: expected an entry for {arn} in {listed}"));
            assert_eq!(
                member(&case, &summary, "description"),
                json!(format!("build number {build}")),
                "case {case:?}: expected the description its create wrote, got {summary}"
            );

            let (status, read) = get_component(&router, arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 reading {arn} back, got {status} with {read}"
            );
            let component = member(&case, &read, "component");
            for name in [
                "arn",
                "name",
                "version",
                "description",
                "platform",
                "type",
                "owner",
                "state",
                "dateCreated",
                "supportedOsVersions",
            ] {
                let listed_member = summary.get(name).cloned();
                let read_member = component.get(name).cloned();
                if listed_member.is_none() && read_member.is_none() {
                    continue;
                }
                assert_eq!(
                    listed_member, read_member,
                    "case {case:?}: expected the entry's {name} to be the one a read of \
                     {arn} reports, got {listed_member:?} and {read_member:?}"
                );
            }
        }

        // The listing naming no version at all asks about every component this account
        // owns in the calling Region, which is these build versions and the other two.
        let case = format!("{case}, no ARN at all");
        let (status, every) = list_build_versions(&router, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {every}"
        );
        let mut want_every = want.clone();
        want_every.push(build_version_arn("alpha-component", "1.1.0", 1));
        want_every.push(build_version_arn("beta-component", "1.0.0", 1));
        assert!(
            want_every.len() <= MAX_PAGE,
            "case {case:?}: this row stores {} build versions, which one unpaged listing \
             cannot carry",
            want_every.len()
        );
        let got_every = summary_arns(&case, &every);
        assert_eq!(
            sorted(&got_every),
            sorted(&want_every),
            "case {case:?}: expected {want_every:?}, got {got_every:?}"
        );
    }
}

/// RULE: a paged walk of a build-version listing visits each entry the listing selected
/// exactly once and in one order, whatever page size the request asks for.
#[tokio::test]
async fn a_paged_walk_visits_every_build_version_once_whatever_the_page_size() {
    let held = 12_usize;
    let router = emulator();
    for build in 1..=held {
        create_component(
            &router,
            "alpha-component",
            "1.0.0",
            &format!("token-alpha-100-build-{build}"),
            &format!("build number {build}"),
        )
        .await;
    }
    create_component(
        &router,
        "alpha-component",
        "1.1.0",
        "token-alpha-110",
        "the minor line",
    )
    .await;
    create_component(
        &router,
        "beta-component",
        "1.0.0",
        "token-beta-100",
        "another name",
    )
    .await;

    let under_version: Vec<String> = (1..=held)
        .map(|build| build_version_arn("alpha-component", "1.0.0", build))
        .collect();
    let mut every: Vec<String> = under_version.clone();
    every.push(build_version_arn("alpha-component", "1.1.0", 1));
    every.push(build_version_arn("beta-component", "1.0.0", 1));

    // (case, the query the walk pages through, what that query selects)
    let queries: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the build versions of one version",
            json!({"componentVersionArn": version_arn("alpha-component", "1.0.0")}),
            under_version,
        ),
        ("every build version this account owns", json!({}), every),
    ];

    for (case, query, selected) in queries {
        // The page sizes differ from one entry per page up to more than the listing holds,
        // including sizes that divide the listing and sizes that do not.
        let mut orders: Vec<(usize, Vec<String>)> = Vec::new();
        for page_size in [1_usize, 2, 3, 5, 11, selected.len(), MAX_PAGE] {
            let case = format!("{case}, {page_size} entries per page");
            let walked = walk(&router, &case, &query, page_size, selected.len()).await;
            assert_eq!(
                sorted(&walked),
                sorted(&selected),
                "case {case:?}: expected the walk to visit {selected:?}, got {walked:?}"
            );
            orders.push((page_size, walked));
        }

        // And the order a walk reports them in does not depend on the page size: the
        // listing has one order, so a token's position in it is the same however the
        // caller pages through it.
        let (first_size, first_order) = orders
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected at least one walk"));
        for (page_size, order) in &orders {
            assert_eq!(
                order, &first_order,
                "case {case:?}: expected the order of the walk at {page_size} entries per \
                 page to be the order at {first_size} entries per page ({first_order:?}), \
                 got {order:?}"
            );
        }
    }
}

/// RULE: a listing is a report, so it leaves the state it read untouched — what a second
/// identical listing reports is what the first one did.
#[tokio::test]
async fn listing_build_versions_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    for build in 1..=3 {
        create_component(
            &router,
            "alpha-component",
            "1.0.0",
            &format!("token-alpha-100-build-{build}"),
            &format!("build number {build}"),
        )
        .await;
    }

    let query = json!({"componentVersionArn": version_arn("alpha-component", "1.0.0")});
    let case = "the first listing";
    let (status, first) = list_build_versions(&router, &query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {first}"
    );

    // Pages walked between the two listings, so the walk is shown to write nothing either.
    let listed = summary_arns(case, &first);
    let walked = walk(&router, "a walk between the two listings", &query, 1, listed.len()).await;
    assert_eq!(
        sorted(&walked),
        sorted(&listed),
        "case a walk between the two listings: expected {listed:?}, got {walked:?}"
    );

    let case = "the second listing";
    let (status, second) = list_build_versions(&router, &query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {second}"
    );
    assert_eq!(
        summaries(case, &second),
        summaries("the first listing", &first),
        "case {case:?}: expected the entries the first listing reported, got {second}"
    );
    // `requestId` identifies the request rather than the state, so it is the one member a
    // second identical listing need not repeat.
    for body in [&first, &second] {
        assert!(
            body.get("requestId").and_then(Value::as_str).is_some(),
            "case {case:?}: expected a requestId in {body}"
        );
    }
}
