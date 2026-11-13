// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resource-policy slice of this service, over EVERY kind of
//! resource one of its `Put*Policy` / `Get*Policy` pairs addresses.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. Each `Put*Policy` carries its members in a JSON
//! body; each `Get*Policy` carries the resource's ARN in a query parameter, as its modelled
//! `@httpQuery` puts it.
//!
//! The rule under test: a resource policy is ATTACHED to the resource whose ARN the write
//! names, whatever kind of resource that is, so the matching read reports the document the
//! last successful write applied — and the unset-policy document `{}` where no write has
//! applied one. A write naming an ARN the calling Region holds no such resource under
//! reports the resource missing (HTTP 404 `ResourceNotFoundException`, naming the kind the
//! ARN identifies and the ARN the request carried), and a document that is not parseable
//! JSON is refused as an invalid value of the `policy` member, leaving whatever policy the
//! resource already carried in place.
//!
//! The rows span what the rule generalises over: WHICH kind of resource the policy is
//! attached to (a component build version, an image recipe, an image build version), how
//! many writes the resource has seen, and the way a document fails to be JSON.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The document a policy read reports for a resource no policy has been applied to.
const UNSET_POLICY: &str = "{}";

/// A build component document: one `build` phase, which is all a component these cases
/// attach a policy to needs.
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

/// Assert the answer is a refusal carrying `status`, `code` and `message`.
fn assert_refused(case: &str, status: u16, body: &Value, expected: u16, code: &str, message: &str) {
    assert_eq!(
        status, expected,
        "case {case:?}: expected HTTP {expected}, got {status} with {body}"
    );
    let actual_code = error_code(case, body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected {code:?}, got {actual_code:?} with {body}"
    );
    let actual_message = text(case, body, "message");
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// `CreateComponent` for one build version of `name`, answering its build version ARN.
async fn create_component(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "platform": "Linux",
        "clientToken": format!("component-{name}"),
        "data": BUILD_DOCUMENT,
    });
    let case = format!("create the component {name}");
    let (status, answer) = call_in(router, REGION, "PUT", "/CreateComponent", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// `CreateImageRecipe` for `name`, including `component_arn`, answering the recipe's ARN.
async fn create_recipe(router: &MultiServiceRouter, name: &str, component_arn: &str) -> String {
    let body = json!({
        "name": name,
        "semanticVersion": "1.0.0",
        "parentImage": "ami-0123456789abcdef0",
        "clientToken": format!("recipe-{name}"),
        "components": [{ "componentArn": component_arn }],
    });
    let case = format!("create the image recipe {name}");
    let (status, answer) = call_in(router, REGION, "PUT", "/CreateImageRecipe", Some(&body)).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "imageRecipeArn")
}

/// `CreateInfrastructureConfiguration` for `name`, answering its ARN.
async fn create_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "clientToken": format!("configuration-{name}"),
        "instanceProfileName": "EC2InstanceProfileForImageBuilder",
    });
    let case = format!("create the infrastructure configuration {name}");
    let (status, answer) = call_in(
        router,
        REGION,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&body),
    )
    .await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "infrastructureConfigurationArn")
}

/// `CreateImage` naming `recipe` and `configuration`, answering the build version's ARN.
async fn create_image(router: &MultiServiceRouter, recipe: &str, configuration: &str) -> String {
    let body = json!({
        "imageRecipeArn": recipe,
        "infrastructureConfigurationArn": configuration,
        "clientToken": "image-policy-case",
    });
    let (status, answer) = call_in(router, REGION, "PUT", "/CreateImage", Some(&body)).await;
    let answer = assert_ok("create the image build version", status, answer);
    text("create the image build version", &answer, "imageBuildVersionArn")
}

/// One resource kind a policy pair addresses: the operations, the member its ARN rides in,
/// the resource-type token a missing-resource report names it by, and an ARN of that kind
/// that names nothing.
struct Kind {
    /// What a failing assertion calls this kind.
    case: &'static str,
    /// The `Put*Policy` operation's name, which is also its request target.
    put: &'static str,
    /// The `Get*Policy` operation's name.
    get: &'static str,
    /// The member both operations name the resource by.
    member: &'static str,
    /// The resource-type token the not-found report names.
    resource_type: &'static str,
    /// The resource portion of an ARN of this kind that names nothing stored.
    absent_resource: &'static str,
}

/// The kinds a policy is attached to in these cases: the three whose resources this store
/// holds and whose policy pair is implemented.
const KINDS: [Kind; 3] = [
    Kind {
        case: "a component build version",
        put: "PutComponentPolicy",
        get: "GetComponentPolicy",
        member: "componentArn",
        resource_type: "Component",
        absent_resource: "component/absent/1.0.0/1",
    },
    Kind {
        case: "an image recipe",
        put: "PutImageRecipePolicy",
        get: "GetImageRecipePolicy",
        member: "imageRecipeArn",
        resource_type: "ImageRecipe",
        absent_resource: "image-recipe/absent/1.0.0",
    },
    Kind {
        case: "an image build version",
        put: "PutImagePolicy",
        get: "GetImagePolicy",
        member: "imageArn",
        resource_type: "Image",
        absent_resource: "image/absent/1.0.0/1",
    },
];

/// `<Kind::put>` applying `document` to `arn`.
async fn put_policy(
    router: &MultiServiceRouter,
    kind: &Kind,
    arn: &str,
    document: &str,
) -> (u16, Value) {
    let mut body = serde_json::Map::new();
    body.insert(kind.member.to_string(), json!(arn));
    body.insert("policy".to_string(), json!(document));
    call_in(
        router,
        REGION,
        "PUT",
        &format!("/{}", kind.put),
        Some(&Value::Object(body)),
    )
    .await
}

/// `<Kind::get>` for `arn`.
async fn get_policy(router: &MultiServiceRouter, kind: &Kind, arn: &str) -> (u16, Value) {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/{}?{}={arn}", kind.get, kind.member),
        None,
    )
    .await
}

/// The policy document a read reports for `arn`.
async fn reported_policy(router: &MultiServiceRouter, case: &str, kind: &Kind, arn: &str) -> String {
    let (status, answer) = get_policy(router, kind, arn).await;
    let answer = assert_ok(case, status, answer);
    text(case, &answer, "policy")
}

/// The ARNs of one resource of each kind, in the order [`KINDS`] lists them.
async fn stored_arns(router: &MultiServiceRouter) -> Vec<String> {
    let component = create_component(router, "policy-component").await;
    let recipe = create_recipe(router, "policy-recipe", &component).await;
    let configuration = create_configuration(router, "policy-infra").await;
    let image = create_image(router, &recipe, &configuration).await;
    vec![component, recipe, image]
}

#[tokio::test]
async fn a_policy_is_attached_to_the_resource_its_arn_names() {
    let router = emulator();
    let arns = stored_arns(&router).await;

    // (case, the document a write applies, what a read then reports for it)
    let writes: [(&str, &str, &str); 3] = [
        // A one-statement share policy, as a caller writes one.
        (
            "a share policy",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"111111111111"},"Action":["imagebuilder:GetComponent"],"Resource":"*"}]}"#,
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"111111111111"},"Action":["imagebuilder:GetComponent"],"Resource":"*"}]}"#,
        ),
        // A second write REPLACES the first, rather than merging into it.
        (
            "a replacement policy",
            r#"{"Version":"2012-10-17","Statement":[]}"#,
            r#"{"Version":"2012-10-17","Statement":[]}"#,
        ),
        // The unset-policy document written explicitly is still a document.
        ("the empty document", UNSET_POLICY, UNSET_POLICY),
    ];

    for (kind, arn) in KINDS.iter().zip(&arns) {
        // Before any write, the read reports the unset-policy document rather than failing.
        let case = format!("{}, before any write", kind.case);
        let actual = reported_policy(&router, &case, kind, arn).await;
        assert_eq!(
            actual, UNSET_POLICY,
            "case {case:?}: expected {UNSET_POLICY:?}, got {actual:?}"
        );

        for (write_case, document, reported) in writes {
            let case = format!("{}, {write_case}", kind.case);
            let (status, answer) = put_policy(&router, kind, arn, document).await;
            let answer = assert_ok(&case, status, answer);
            // The write names the resource it wrote to, read back off the stored record.
            let named = text(&case, &answer, kind.member);
            assert_eq!(
                named, *arn,
                "case {case:?}: expected the write to name {arn:?}, got {named:?}"
            );
            let actual = reported_policy(&router, &case, kind, arn).await;
            assert_eq!(
                actual, reported,
                "case {case:?}: expected {reported:?}, got {actual:?}"
            );
        }

        // A policy is attached to ONE resource: every other resource still reports its
        // own policy, which is the unset-policy document — the one this row's last write
        // left on its own resource, so the two cannot be told apart by accident.
        for (other_kind, other_arn) in KINDS.iter().zip(&arns) {
            if other_arn == arn {
                continue;
            }
            let case = format!("{}, after writing to {}", other_kind.case, kind.case);
            let actual = reported_policy(&router, &case, other_kind, other_arn).await;
            assert_eq!(
                actual, UNSET_POLICY,
                "case {case:?}: expected a resource of another kind to keep its own policy, \
                 got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_write_naming_no_stored_resource_reports_it_missing() {
    let router = emulator();
    let arns = stored_arns(&router).await;

    for (kind, arn) in KINDS.iter().zip(&arns) {
        // The write that establishes what the refusals below must leave alone.
        let stored = r#"{"Version":"2012-10-17","Statement":[]}"#;
        let case = format!("{}, the policy to keep", kind.case);
        let (status, answer) = put_policy(&router, kind, arn, stored).await;
        assert_ok(&case, status, answer);

        // An ARN of this kind that names nothing: both the write and the read report the
        // resource missing, naming the kind the ARN identifies and the ARN verbatim.
        let absent = format!(
            "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:{}",
            kind.absent_resource
        );
        let expected = format!(
            "The following required resource '{}' cannot be found: '{absent}'.",
            kind.resource_type
        );
        let case = format!("{}, an ARN nothing was stored under", kind.case);
        let (status, answer) = put_policy(&router, kind, &absent, stored).await;
        assert_refused(
            &case,
            status,
            &answer,
            404,
            "ResourceNotFoundException",
            &expected,
        );
        let (status, answer) = get_policy(&router, kind, &absent).await;
        assert_refused(
            &format!("{case}, read"),
            status,
            &answer,
            404,
            "ResourceNotFoundException",
            &expected,
        );

        // A document that is not JSON is refused as the value of the `policy` member,
        // whichever kind the ARN names and whether or not it names a stored resource —
        // and the stored policy is left as it was.
        let malformed = "The value supplied for parameter 'policy' is not valid. Policy is \
                         malformed and cannot be parsed";
        for (document_case, document) in [
            ("an unterminated object", "{this is not valid json"),
            ("a bare word", "not json at all"),
            ("an unterminated array", "[1, 2"),
            ("a lone closing brace", "}"),
        ] {
            let case = format!("{}, {document_case}", kind.case);
            let (status, answer) = put_policy(&router, kind, arn, document).await;
            assert_refused(
                &case,
                status,
                &answer,
                400,
                "InvalidParameterValueException",
                malformed,
            );
            let actual = reported_policy(&router, &case, kind, arn).await;
            assert_eq!(
                actual, stored,
                "case {case:?}: expected the stored policy to survive the refusal, got \
                 {actual:?}"
            );
        }
    }
}
