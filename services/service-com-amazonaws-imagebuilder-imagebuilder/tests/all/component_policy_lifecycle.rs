// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the state `PutComponentPolicy` writes: a component's resource
//! policy is state the component the request names OWNS.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. `PutComponentPolicy` carries its members in a
//! JSON body; `GetComponentPolicy` and `DeleteComponent` carry their ARN in a query
//! parameter, as their modelled `@httpQuery` puts it.
//!
//! The rule under test: a resource policy is state of the one resource its ARN names, so a
//! write is reachable only through the Region that holds that resource, it is discarded
//! with the resource itself (a component built again under the name carries the unset
//! document, never the deleted build's policy), a document outside the member's modelled
//! length is refused with the stored document left as it was, and every call answers a
//! fresh request id beside the ARN of the record it touched.
//!
//! `tests/all/component_policy.rs` spans the documents a write stores and the rendering a
//! read reports for them; the rows here span the OTHER axes of the same rule: which Region
//! the endpoint serves, whether the resource still exists, and where the document sits
//! against the published length bounds.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Regions a case signs for. A policy written through one of them must be reachable
/// through that one alone.
const REGIONS: [&str; 3] = ["us-west-2", "eu-west-1", "ap-northeast-1"];

/// The document a policy read reports for a resource no policy has been applied to.
const UNSET_POLICY: &str = "{}";

/// The modelled `ResourcePolicyDocument` length bounds
/// (`smithy_query(query="operation",operation="PutComponentPolicy")` reports the `policy`
/// member as `len 1..30000`).
const MIN_POLICY_LENGTH: usize = 1;
const MAX_POLICY_LENGTH: usize = 30_000;

/// The modelled `NonEmptyString` bounds a `requestId` stays inside (`len 1..1024`).
const MIN_REQUEST_ID_LENGTH: usize = 1;
const MAX_REQUEST_ID_LENGTH: usize = 1024;

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

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
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

/// `GetComponentPolicy` for `arn`, through the `region` endpoint.
async fn get_policy(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetComponentPolicy?componentArn={arn}"),
        None,
    )
    .await
}

/// `PutComponentPolicy` applying `document` to `arn`, through the `region` endpoint.
async fn put_policy(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    document: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/PutComponentPolicy",
        Some(&json!({ "componentArn": arn, "policy": document })),
    )
    .await
}

/// `DeleteComponent` for the build version `arn` names, through the `region` endpoint.
async fn delete_component(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/DeleteComponent?componentBuildVersionArn={arn}"),
        None,
    )
    .await
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

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// Assert `status`, the error code and the message of a refusal.
fn assert_refused(
    case: &str,
    status: u16,
    body: &Value,
    expect_status: u16,
    expect_code: &str,
    expect_message: &str,
) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
    let message = text(case, body, "message");
    assert_eq!(
        message, expect_message,
        "case {case:?}: expected the message {expect_message:?}, got {message:?}"
    );
}

/// The message a call naming a resource this Region holds nothing under answers with.
fn not_found_message(arn: &str) -> String {
    format!("The following required resource 'Component' cannot be found: '{arn}'.")
}

/// A build component document whose single `build` phase echoes `greeting`, so two builds
/// of one component name can differ in content the way a new build version must.
fn build_document(greeting: &str) -> String {
    format!(
        "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: \
         Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - \
         echo {greeting}\n"
    )
}

/// Create one build version of the component `name` carries in `region`, with `greeting`
/// as its document's content, and answer its build version ARN.
async fn create_component(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    greeting: &str,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert(
        "clientToken".to_string(),
        json!(format!("token-{region}-{name}-{greeting}")),
    );
    body.insert("data".to_string(), json!(build_document(greeting)));
    let case = format!("create the component {name} in {region}");
    let (status, answer) =
        call_in(router, region, "PUT", "/CreateComponent", Some(&Value::Object(body))).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// The document a read of `arn` through the `region` endpoint reports, asserted to be a
/// successful read.
async fn reported_policy(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    arn: &str,
) -> String {
    let (status, body) = get_policy(router, region, arn).await;
    let body = assert_ok(case, status, body);
    text(case, &body, "policy")
}

/// A document of exactly `length` characters: an object whose one value is padded out to
/// it. Already in the rendered form (no whitespace between its tokens), so a read reports
/// it back unchanged.
fn padded_document(length: usize) -> String {
    let empty = json!({ "Sid": "" }).to_string();
    let padding = length
        .checked_sub(empty.len())
        .expect("a length that leaves room for the object around the padding");
    let document = json!({ "Sid": "a".repeat(padding) }).to_string();
    assert_eq!(
        document.len(),
        length,
        "expected a document of {length} characters, got {}",
        document.len()
    );
    document
}

#[tokio::test]
async fn a_policy_is_reachable_only_through_the_region_that_holds_the_component() {
    let router = emulator();

    // One component of the same name in each Region, each given its own document.
    let mut held: Vec<(&str, String, String)> = Vec::new();
    for (index, region) in REGIONS.iter().enumerate() {
        let arn = create_component(&router, region, "regional-policy-component", "hello").await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("{index}{index}{index}11111111") },
                "Action": "imagebuilder:GetComponent",
                "Resource": "*",
            }],
        })
        .to_string();

        let case = format!("a write through the {region} endpoint");
        let (status, body) = put_policy(&router, region, &arn, &document).await;
        let body = assert_ok(&case, status, body);
        let named = text(&case, &body, "componentArn");
        assert_eq!(
            named, arn,
            "case {case:?}: expected the write to name the component {arn:?}, got {named:?}"
        );
        held.push((region, arn, document));
    }

    for (region, arn, document) in &held {
        let case = format!("a read through the {region} endpoint, which holds {arn}");
        let reported = reported_policy(&router, region, &case, arn).await;
        assert_eq!(
            reported, *document,
            "case {case:?}: expected the document written there, got {reported:?}"
        );

        for other in REGIONS.iter().filter(|candidate| *candidate != region) {
            let case = format!("{arn} reached through the {other} endpoint");
            let (status, body) = get_policy(&router, other, arn).await;
            assert_refused(
                &case,
                status,
                &body,
                404,
                "ResourceNotFoundException",
                &not_found_message(arn),
            );

            let intruder = json!({ "Version": "2012-10-17", "Sid": "from-another-region" })
                .to_string();
            let (status, body) = put_policy(&router, other, arn, &intruder).await;
            assert_refused(
                &case,
                status,
                &body,
                404,
                "ResourceNotFoundException",
                &not_found_message(arn),
            );
        }

        // Every refusal above left the document the owning Region reports as it was.
        let case = format!("{arn} after the refusals of the other Regions' endpoints");
        let reported = reported_policy(&router, region, &case, arn).await;
        assert_eq!(
            reported, *document,
            "case {case:?}: expected the stored document to survive, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_component_takes_its_policy_with_it() {
    let router = emulator();
    let region = REGIONS[0];

    let kept = create_component(&router, region, "kept-component", "hello").await;
    let doomed = create_component(&router, region, "doomed-component", "hello").await;
    let kept_document = json!({ "Version": "2012-10-17", "Sid": "kept" }).to_string();
    let doomed_document = json!({ "Version": "2012-10-17", "Sid": "doomed" }).to_string();

    for (case, arn, document) in [
        ("the component that survives", &kept, &kept_document),
        ("the component that is deleted", &doomed, &doomed_document),
    ] {
        let (status, body) = put_policy(&router, region, arn, document).await;
        assert_ok(case, status, body);
        let reported = reported_policy(&router, region, case, arn).await;
        assert_eq!(
            reported, *document,
            "case {case:?}: expected the document just written, got {reported:?}"
        );
    }

    let case = "delete the component a policy was applied to";
    let (status, body) = delete_component(&router, region, &doomed).await;
    let body = assert_ok(case, status, body);
    let deleted = text(case, &body, "componentBuildVersionArn");
    assert_eq!(
        deleted, doomed,
        "case {case:?}: expected the delete to name {doomed:?}, got {deleted:?}"
    );

    // Neither a read nor a write reaches the policy of a resource that is gone.
    let case = "the deleted component";
    let (status, body) = get_policy(&router, region, &doomed).await;
    assert_refused(
        case,
        status,
        &body,
        404,
        "ResourceNotFoundException",
        &not_found_message(&doomed),
    );
    let (status, body) = put_policy(&router, region, &doomed, &doomed_document).await;
    assert_refused(
        case,
        status,
        &body,
        404,
        "ResourceNotFoundException",
        &not_found_message(&doomed),
    );

    let case = "the component that survives the other's delete";
    let reported = reported_policy(&router, region, case, &kept).await;
    assert_eq!(
        reported, kept_document,
        "case {case:?}: expected its own document, got {reported:?}"
    );

    // The next build of the deleted name counts up from the greatest build still stored,
    // so with none stored it is built at the ARN the deleted build carried — and it is a
    // new resource, which carries no policy at all.
    let case = "a component built again under the deleted name";
    let rebuilt = create_component(&router, region, "doomed-component", "goodbye").await;
    assert_eq!(
        rebuilt, doomed,
        "case {case:?}: expected the rebuilt component at {doomed:?}, got {rebuilt:?}"
    );
    let reported = reported_policy(&router, region, case, &rebuilt).await;
    assert_eq!(
        reported, UNSET_POLICY,
        "case {case:?}: expected the unset-policy document, got {reported:?}"
    );
}

#[tokio::test]
async fn every_policy_call_answers_a_fresh_modelled_request_id() {
    let router = emulator();
    let region = REGIONS[0];
    let arn = create_component(&router, region, "request-id-component", "hello").await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for index in 0..4 {
        let document = json!({ "Version": "2012-10-17", "Sid": format!("write-{index}") })
            .to_string();
        let case = format!("write {index}");

        let (status, body) = put_policy(&router, region, &arn, &document).await;
        let body = assert_ok(&case, status, body);
        for name in ["write", "read"] {
            let body = if name == "write" {
                body.clone()
            } else {
                let (status, read) = get_policy(&router, region, &arn).await;
                let read = assert_ok(&case, status, read);
                let reported = text(&case, &read, "policy");
                assert_eq!(
                    reported, document,
                    "case {case:?}: expected the document just written, got {reported:?}"
                );
                read
            };
            let id = text(&case, &body, "requestId");
            assert!(
                (MIN_REQUEST_ID_LENGTH..=MAX_REQUEST_ID_LENGTH).contains(&id.len()),
                "case {case:?}: expected the {name} request id inside the modelled length, \
                 got {id:?}"
            );
            assert!(
                seen.insert(id.clone()),
                "case {case:?}: expected a fresh {name} request id, got the already-seen {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_document_outside_the_modelled_length_is_refused_and_leaves_the_stored_one_alone() {
    let router = emulator();
    let region = REGIONS[0];
    let arn = create_component(&router, region, "length-bound-component", "hello").await;

    // Both published bounds are accepted: the member's only other requirement is that the
    // document parse as JSON, which a one-character JSON number satisfies.
    let smallest = "7".to_string();
    let largest = padded_document(MAX_POLICY_LENGTH);
    assert_eq!(
        smallest.len(),
        MIN_POLICY_LENGTH,
        "expected a document of the modelled minimum length, got {smallest:?}"
    );
    for (case, document) in [
        ("a document of the modelled minimum length", &smallest),
        ("a document of exactly the modelled maximum length", &largest),
    ] {
        let (status, body) = put_policy(&router, region, &arn, document).await;
        assert_ok(case, status, body);
        let reported = reported_policy(&router, region, case, &arn).await;
        assert_eq!(
            reported, *document,
            "case {case:?}: expected the document just written, got {reported:?}"
        );
    }

    // One character past either bound is refused by the modelled-constraint walk, and the
    // document stored by the last accepted write — the maximum-length one — survives.
    for (case, document) in [
        (
            "an empty document, one character short of the modelled minimum",
            String::new(),
        ),
        (
            "a document one character past the modelled maximum",
            padded_document(MAX_POLICY_LENGTH + 1),
        ),
    ] {
        let (status, body) = put_policy(&router, region, &arn, &document).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(case, &body);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected the modelled-constraint refusal, got {code:?} with {body}"
        );
        let reported = reported_policy(&router, region, case, &arn).await;
        assert_eq!(
            reported, largest,
            "case {case:?}: expected the stored document to survive the refusal, got \
             {reported:?}"
        );
    }
}
