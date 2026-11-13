// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListVersionsByFunction`: the entries it reports for one
//! function, the order it reports them in, the per-version snapshot each entry carries,
//! the paging the model documents and the refusals it answers.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals. Each
//! listed entry is checked against what `GetFunctionConfiguration` reports for the same
//! qualifier, so nothing here trusts the listing alone.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, as the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`) spells it.
const LATEST: &str = "$LATEST";

/// "Lambda returns up to 50 versions per call" (`ListVersionsByFunction`), and
/// "ListVersionsByFunction returns a maximum of 50 items in each response, even if you
/// set the number higher" (its `MaxItems` member).
const MAX_VERSIONS_PER_PAGE: usize = 50;

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }
}

/// Dispatch one JSON request, the shape every operation here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// `ListVersionsByFunction` on a function reference, with the given query string.
async fn list_versions(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/versions{query}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `UpdateFunctionCode` on a function reference, with the given request body.
async fn update_function_code(
    router: &MultiServiceRouter,
    reference: &str,
    body: &Value,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/code");
    json_call(router, "PUT", &uri, body).await
}

/// `PublishVersion` on a function reference, with the given request body.
async fn publish_version(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetFunctionConfiguration` on a function name, qualified by a version label.
async fn get_function_configuration(
    router: &MultiServiceRouter,
    name: &str,
    qualifier: &str,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/configuration?Qualifier={qualifier}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// must report, so the test never reads the value it is checking out of the response.
fn expected_code_sha256(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// Deployment package bytes of a given length. The content is irrelevant to the API —
/// only its length and its digest are reported — but it differs per length so two
/// packages never share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN a function of `name` must carry.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN of one version of a function, as a LISTING entry names it: every entry of
/// `ListVersionsByFunction` reports a version, so its ARN carries that version's label —
/// the unpublished version included, which a read addressed to the FUNCTION reports without
/// (`function_arn`).
fn version_arn(name: &str, label: &str) -> String {
    format!("{}:{label}", function_arn(name))
}

/// Create a plain .zip function of `name` from `code`, asserting it was stored.
async fn create_zip_function(router: &MultiServiceRouter, name: &str, code: &[u8]) {
    let answer = json_call(
        router,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(code) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name}: expected 201 from CreateFunction, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every stored function's activation and update instants into the past, so the
/// next read materialises the transitions the clock owes. This is what waiting would do,
/// without waiting.
fn settle_pending_transitions(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// Replace `name`'s deployment package with `code` and publish the result, answering the
/// version label the publish reported.
///
/// Changing the code is what makes the publish a new version: "Lambda doesn't publish a
/// version if the function's configuration and code haven't changed since the last
/// version."
async fn publish_new_code(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    code: &[u8],
) -> String {
    settle_pending_transitions(engine);
    let updated =
        update_function_code(router, name, &json!({ "ZipFile": zip_file(code) })).await;
    assert_eq!(
        updated.status,
        200,
        "case {case}: expected 200 from UpdateFunctionCode, got {} {}",
        updated.status,
        updated.text()
    );
    settle_pending_transitions(engine);
    let published = publish_version(router, name, &Value::Null).await;
    assert_eq!(
        published.status,
        201,
        "case {case}: expected 201 from PublishVersion, got {} {}",
        published.status,
        published.text()
    );
    published.json()["Version"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Version label, got {:?}", published.json()))
        .to_string()
}

/// The `Versions` array of a successful listing.
fn versions_of(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case}: expected 200 from ListVersionsByFunction, got {} {}",
        answer.status,
        answer.text()
    );
    let body = answer.json();
    body["Versions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Versions array, got {body:?}"))
        .clone()
}

/// The `Version` label of each entry of a listing page.
fn labels_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["Version"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The version labels a function with `published` published versions must report, in the
/// order they must be reported in: the unpublished version first, then the published
/// ones in ascending numeric order.
fn expected_labels(published: usize) -> Vec<String> {
    let mut labels = vec![LATEST.to_string()];
    labels.extend((1..=published).map(|version| version.to_string()));
    labels
}

/// `ListVersionsByFunction` reports the unpublished version and every published version
/// of the named function, each from its own stored snapshot, in version order.
///
/// The rows differ along the one thing the rule generalises over: how many versions the
/// function has. The twelve-version row is what distinguishes the numeric order the rule
/// states from a lexicographic one (`10` sorts before `9` as text), and the zero-version
/// row is the function nobody has published from, which still has its unpublished
/// version.
#[tokio::test]
async fn list_versions_by_function_reports_every_version_from_its_own_snapshot() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for published_count in [0_usize, 1, 3, 12] {
        let name = format!("versions-{published_count}");
        let case = name.clone();

        // One distinct package per version, so an entry reporting another version's
        // snapshot is caught by its digest and its size.
        let mut packages = vec![package_bytes(16 + published_count)];
        create_zip_function(&router, &name, &packages[0]).await;
        for version in 1..=published_count {
            let code = package_bytes(128 * version + published_count);
            let label = publish_new_code(&router, &engine, &case, &name, &code).await;
            assert_eq!(
                label,
                version.to_string(),
                "case {case}: expected PublishVersion to hand out version {version}, got {label}"
            );
            packages.push(code);
        }
        // The unpublished version carries the newest code, which is the last package
        // pushed above.
        let latest_package = packages.last().cloned().unwrap_or_default();

        let answer = list_versions(&router, &name, "").await;
        let entries = versions_of(&case, &answer);
        assert_eq!(
            labels_of(&entries),
            expected_labels(published_count),
            "case {case}: expected the unpublished version then every published version in \
             ascending order, got {:?}",
            labels_of(&entries)
        );
        assert!(
            answer.json()["NextMarker"].is_null(),
            "case {case}: a listing that reported every version is the last page, so it \
             carries no NextMarker, got {:?}",
            answer.json()["NextMarker"]
        );

        for entry in &entries {
            let label = entry["Version"].as_str().unwrap_or_default().to_string();
            // Each entry's snapshot: the unpublished version holds the newest package,
            // and a published version holds the package the function had when it was
            // published.
            let package = if label == LATEST {
                latest_package.clone()
            } else {
                let number: usize = label.parse().unwrap_or_default();
                packages.get(number).cloned().unwrap_or_default()
            };
            assert_eq!(
                entry["FunctionName"], json!(name),
                "case {case} version {label}: expected the function's name, got {:?}",
                entry["FunctionName"]
            );
            assert_eq!(
                entry["FunctionArn"],
                json!(version_arn(&name, &label)),
                "case {case} version {label}: expected the version's own ARN, got {:?}",
                entry["FunctionArn"]
            );
            assert_eq!(
                entry["CodeSha256"],
                json!(expected_code_sha256(&package)),
                "case {case} version {label}: expected the digest of that version's \
                 package, got {:?}",
                entry["CodeSha256"]
            );
            assert_eq!(
                entry["CodeSize"],
                json!(package.len()),
                "case {case} version {label}: expected the size of that version's package, \
                 got {:?}",
                entry["CodeSize"]
            );

            // The listing reports the version-specific configuration, so every member of
            // it is the one `GetFunctionConfiguration` reports for the same qualifier.
            let fetched = get_function_configuration(&router, &name, &label).await;
            assert_eq!(
                fetched.status,
                200,
                "case {case} version {label}: expected 200 from GetFunctionConfiguration, \
                 got {} {}",
                fetched.status,
                fetched.text()
            );
            let configuration = fetched.json();
            for member in [
                "FunctionName",
                "Version",
                "Runtime",
                "Role",
                "Handler",
                "CodeSize",
                "CodeSha256",
                "MemorySize",
                "Timeout",
                "PackageType",
                "RevisionId",
                "Description",
                "LastModified",
            ] {
                assert_eq!(
                    entry[member], configuration[member],
                    "case {case} version {label}: the listed {member} must be what \
                     GetFunctionConfiguration reports ({:?}), got {:?}",
                    configuration[member], entry[member]
                );
            }
            // `FunctionArn` is the one member that differs for the UNPUBLISHED version: a
            // listing entry names a version, so it carries the version's label, while the
            // configuration read addressed to the function reports the function's own ARN
            // (asserted against `version_arn` above, and the same value for every
            // published version).
            if label != LATEST {
                assert_eq!(
                    entry["FunctionArn"], configuration["FunctionArn"],
                    "case {case} version {label}: the listed FunctionArn must be what \
                     GetFunctionConfiguration reports ({:?}), got {:?}",
                    configuration["FunctionArn"], entry["FunctionArn"]
                );
            }
        }
    }
}

/// A paged walk reports every version of the function exactly once, in version order,
/// whatever the page size — and no page is larger than the maximum the model documents.
///
/// The rows differ along the page size, including one that is a divisor of the entry
/// count, one that is not, one that is the whole count and one far past it.
#[tokio::test]
async fn list_versions_by_function_pages_over_every_version() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let name = "paged";
    let published_count = 11_usize;
    create_zip_function(&router, name, &package_bytes(32)).await;
    for version in 1..=published_count {
        publish_new_code(
            &router,
            &engine,
            "paged",
            name,
            &package_bytes(64 * version),
        )
        .await;
    }
    let expected = expected_labels(published_count);

    for page_size in [1_usize, 2, 5, 12, 1_000] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging did not terminate after {pages} pages"
            );
            let query = match &marker {
                Some(marker) => format!("?MaxItems={page_size}&Marker={marker}"),
                None => format!("?MaxItems={page_size}"),
            };
            let answer = list_versions(&router, name, &query).await;
            let page = versions_of(&format!("page size {page_size}"), &answer);
            let allowed = page_size.min(MAX_VERSIONS_PER_PAGE);
            assert!(
                page.len() <= allowed,
                "case page size {page_size}: expected at most {allowed} entries, got {}",
                page.len()
            );
            for label in labels_of(&page) {
                assert!(
                    !seen.contains(&label),
                    "case page size {page_size}: version {label} was reported twice"
                );
                seen.push(label);
            }
            match answer.json()["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every version once in version order, got \
             {seen:?}"
        );
    }

    // A marker continues AFTER the version it names, whichever version that is.
    for start_after in ["$LATEST", "1", "7", "11"] {
        let answer = list_versions(&router, name, &format!("?Marker={start_after}")).await;
        let labels = labels_of(&versions_of(
            &format!("marker {start_after}"),
            &answer,
        ));
        let tail: Vec<String> = expected
            .iter()
            .skip_while(|label| label.as_str() != start_after)
            .skip(1)
            .cloned()
            .collect();
        assert_eq!(
            labels, tail,
            "case marker {start_after}: expected the versions after it, got {labels:?}"
        );
    }
}

/// "Lambda returns up to 50 versions per call": a function with more versions than that
/// answers a full page and a marker to continue from, even when `MaxItems` asks for more.
#[tokio::test]
async fn list_versions_by_function_reports_at_most_fifty_versions_per_call() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One past the documented page size, so the unpublished version and the published
    // ones together need three pages at the maximum size.
    let published_count = MAX_VERSIONS_PER_PAGE + 1;
    let name = "many-versions";
    create_zip_function(&router, name, &package_bytes(8)).await;
    for version in 1..=published_count {
        publish_new_code(&router, &engine, "many", name, &package_bytes(8 + version)).await;
    }
    let expected = expected_labels(published_count);

    for (case, query) in [
        ("defaulted", String::new()),
        ("at the maximum", format!("?MaxItems={MAX_VERSIONS_PER_PAGE}")),
        ("past the maximum", "?MaxItems=10000".to_string()),
    ] {
        let answer = list_versions(&router, name, &query).await;
        let page = versions_of(case, &answer);
        assert_eq!(
            page.len(),
            MAX_VERSIONS_PER_PAGE,
            "case {case}: expected a full page of {MAX_VERSIONS_PER_PAGE} versions, got {}",
            page.len()
        );
        assert_eq!(
            labels_of(&page),
            expected[..MAX_VERSIONS_PER_PAGE],
            "case {case}: expected the first {MAX_VERSIONS_PER_PAGE} versions in order, got {:?}",
            labels_of(&page)
        );
        let next = answer.json()["NextMarker"].as_str().map(str::to_string);
        assert_eq!(
            next.as_deref(),
            expected.get(MAX_VERSIONS_PER_PAGE - 1).map(String::as_str),
            "case {case}: expected a marker naming the last version this page reported, got \
             {next:?}"
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    /// The case name.
    name: &'static str,
    /// The function reference in the URI.
    reference: &'static str,
    /// The query string of the request.
    query: &'static str,
    /// The status the call must answer.
    status: u16,
    /// The wire error code the call must answer.
    error_code: &'static str,
}

/// A request naming no function, or a page the operation cannot address, is refused with
/// the modelled error rather than answered with an empty page.
#[tokio::test]
async fn list_versions_by_function_refuses_a_request_it_cannot_serve() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "refusals", &package_bytes(48)).await;
    settle_pending_transitions(&engine);

    let cases = [
        // "The resource specified in the request does not exist."
        RefusalCase {
            name: "unknown-function",
            reference: "absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "unknown-function-by-arn",
            reference: "arn:aws:lambda:us-east-1:000000000000:function:absent",
            query: "",
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // A qualifier in the name names a version, and this operation lists the versions
        // OF A FUNCTION, so the identifier must carry none at all — refused on the
        // REQUEST, before any lookup, whether or not the function has that version.
        RefusalCase {
            name: "a version number in the name",
            reference: "refusals:4",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "an alias in the name",
            reference: "refusals:live",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a version number in the name of a function that does not exist",
            reference: "absent:4",
            query: "",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // The marker is the version label a previous page ended at, so a value that is
        // no version label names no page.
        RefusalCase {
            name: "marker-is-not-a-version",
            reference: "refusals",
            query: "?Marker=not-a-version",
            status: 400,
            error_code: "InvalidParameterValueException",
        },
    ];

    for case in cases {
        let answer = list_versions(&router, case.reference, case.query).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.error_code),
            "case {}: expected the {} error code, got {}",
            case.name,
            case.error_code,
            answer.error_code()
        );
    }

    // `MaxItems` is modelled `@range(min: 1, max: 10000)`: a page size outside it is
    // refused rather than paged over, and each published bound is accepted.
    for (case, max_items) in [("zero", "0"), ("negative", "-1"), ("past the maximum", "10001")] {
        let answer = list_versions(&router, "refusals", &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {case}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }
    for (case, max_items) in [("at the minimum", "1"), ("at the maximum", "10000")] {
        let answer = list_versions(&router, "refusals", &format!("?MaxItems={max_items}")).await;
        let page = versions_of(case, &answer);
        assert_eq!(
            labels_of(&page),
            vec![LATEST.to_string()],
            "case MaxItems {case}: expected the unpublished version, got {:?}",
            labels_of(&page)
        );
    }
}
