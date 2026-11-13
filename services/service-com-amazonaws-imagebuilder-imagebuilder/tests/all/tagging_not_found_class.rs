// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH not-found class the resource-tagging route answers.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The resource ARN rides in the URI path, as the
//! modelled `@httpLabel` puts it, percent-encoded the way a non-greedy label is.
//!
//! The rule under test: the TAG LISTING is answered by the resource-tagging layer, so an
//! ARN that resolves to no resource of the calling Region is reported as that layer's own
//! not-found class — the wire code `NotFoundException`, HTTP 404, and the fixed sentence
//! "Resource was not found", which names neither the kind of resource nor the ARN the
//! request carried — rather than as this service's modelled `ResourceNotFoundException`,
//! whose sentence names both. A resource that exists answers with its tags, and one that
//! exists and was never tagged answers with an empty tag map: neither is a not-found
//! report.
//!
//! The rows span what the rule generalises over: which kind of resource the ARN names
//! (every kind the modelled `ImageBuilderArn` admits, whether or not this store holds that
//! kind at all), why the ARN resolves to nothing (never created, another Region, a version
//! or build nothing was stored under), and whether the resource that does exist carries
//! tags.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the case that checks a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The wire code the resource-tagging layer's not-found refusal carries.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence that refusal carries: fixed text, naming nothing of the request.
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// A build component document: one `build` phase.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    \
                              steps:\n      - name: Greet\n        action: ExecuteBash\n        \
                              inputs:\n          commands:\n            - echo hello\n";

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

/// A request signed for `imagebuilder` in `region`.
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

/// Status and parsed body of one call.
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
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a refusal carries, from `__type`.
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

/// `ListTagsForResource` for `arn`, in `region`.
async fn list_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        None,
    )
    .await
}

/// The tags the listing reports for `arn`.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = list_tags(router, REGION, arn).await;
    let listed = assert_ok(case, status, listed);
    listed
        .get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags object in {listed}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected text in tags.{key}"))
                    .to_string(),
            )
        })
        .collect()
}

/// `CreateComponent` for one build version of `name` in `region`, carrying `tags`.
async fn create_component(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<Value>,
) -> String {
    let mut body = serde_json::Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert(
        "clientToken".to_string(),
        json!(format!("component-{region}-{name}")),
    );
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tags);
    }
    let case = format!("create the component {name} in {region}");
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateComponent",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

#[tokio::test]
async fn a_tag_listing_reports_an_unresolved_arn_as_the_tagging_layers_own_class() {
    let router = emulator();

    // One component that DOES exist, in each Region, so a row that names a stored resource
    // from the wrong endpoint differs from its neighbours only in the Region.
    let tagged = create_component(
        &router,
        REGION,
        "tagged-component",
        Some(json!({ "Owner": "platform" })),
    )
    .await;
    let elsewhere = create_component(&router, OTHER_REGION, "regional-component", None).await;

    // Each row is an ARN that resolves to no resource of [`REGION`], and why. The kinds are
    // the ones the modelled `ImageBuilderArn` pattern admits
    // (`smithy_query(query="shape",shape="ImageBuilderArn")`), including the kinds this
    // store holds none of at all.
    let rows: Vec<(String, String)> = vec![
        (
            "a component nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/absent/1.0.0/1"),
        ),
        (
            "a build number nothing was stored under".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/tagged-component/1.0.0/2"),
        ),
        (
            "a version nothing was stored under".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/tagged-component/2.0.0/1"),
        ),
        (
            "an image recipe nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/absent/1.0.0"),
        ),
        (
            "a container recipe, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:container-recipe/absent/1.0.0"),
        ),
        (
            "an infrastructure configuration nothing created".to_string(),
            format!(
                "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent"
            ),
        ),
        (
            "a distribution configuration nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/absent"),
        ),
        (
            "an image pipeline nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/absent"),
        ),
        (
            "an image build version nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image/absent/1.0.0/1"),
        ),
        (
            "a lifecycle policy, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:lifecycle-policy/absent"),
        ),
        (
            "a build workflow nothing created".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/absent/1.0.0/1"),
        ),
        (
            "a stored component reached from another Region's endpoint".to_string(),
            elsewhere.clone(),
        ),
    ];

    for (case, arn) in &rows {
        let (status, body) = list_tags(&router, REGION, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        let code = error_code(case, &body);
        assert_eq!(
            code, TAGGING_NOT_FOUND_CODE,
            "case {case:?}: expected the tagging layer's own class, got {code:?} with {body}"
        );
        let message = text(case, &body, "message");
        assert_eq!(
            message, TAGGING_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the fixed sentence {TAGGING_NOT_FOUND_MESSAGE:?}, got \
             {message:?}"
        );
        // The sentence is fixed text: it carries neither the ARN nor the kind of resource.
        assert!(
            !message.contains(arn.as_str()),
            "case {case:?}: expected the sentence to withhold the ARN, got {message:?}"
        );
    }

    // The same call on a resource that DOES exist answers with its tags, and on one that
    // exists and was never tagged with an empty map: neither is a not-found report.
    let actual = reported_tags(&router, "the component that exists", &tagged).await;
    let expected: BTreeMap<String, String> =
        [("Owner".to_string(), "platform".to_string())].into_iter().collect();
    assert_eq!(
        actual, expected,
        "case \"the component that exists\": expected {expected:?}, got {actual:?}"
    );
    let untagged = create_component(&router, REGION, "untagged-component", None).await;
    let actual = reported_tags(&router, "a component that was never tagged", &untagged).await;
    assert!(
        actual.is_empty(),
        "case \"a component that was never tagged\": expected an empty tag map, got {actual:?}"
    );
}
