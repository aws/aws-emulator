// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE sentence this service refuses a parameter value in,
//! whatever established that the value is not one it admits.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, this service's front door, the modelled-constraint walk and the handlers —
//! with the emulator's own IAM mounted beside imagebuilder on one engine, the way the
//! assembled emulator runs them. So what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rule under test: a value this service cannot accept for a parameter is refused HTTP
//! 400 `InvalidParameterValueException` stating the parameter as the API addresses it and the
//! requirement its value had to satisfy, with the value itself withheld — whether a modelled
//! constraint, a lookup of the resource the value names, a parse of the document it carries,
//! or the opaqueness of a token this service issues is what established the value is not
//! admitted. The sentence is ONE sentence — `The value supplied for parameter '<member>' is
//! not valid. <requirement>` — for every class the recordings show it for, which is a
//! modelled constraint, a name resolved against another service's resource, a parse and a
//! page token this service did not issue. The
//! refusal names the request it answers in the body's `requestId`, and nothing of the store
//! changes.
//!
//! `tests/all/parameter_value_refusals.rs` spans the modelled-constraint classes; the rows
//! here span what the rule generalises over BESIDE them: what established the value is not
//! admitted (a model constraint, an IAM lookup, a JSON parse, an opaque page token), which
//! operation carried it (a create, an update, a policy write, two listings), and which
//! member it rode in.
//!
//! The emulator and the calls this suite drives are the harness
//! `tests/all/invalid_parameter_sentence_template.rs` reads the same refusals through, which
//! is why they are `pub(crate)` here: one copy of the wire path, two rules asserted over it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The error this service refuses a parameter value with
/// (`smithy_query(query="shape",shape="InvalidParameterValueException")`: a client error,
/// `@httpError` 400).
pub(crate) const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The instance profile this account holds, so a configuration can be created at all.
pub(crate) const HELD_PROFILE: &str = "EC2InstanceProfileForImageBuilder";

/// A trust policy admitting Amazon EC2, so the staged role is one an instance profile may
/// carry. Free of `&`, `=`, `+` and `%`, so it travels in an `awsQuery` form body as
/// written.
pub(crate) const EC2_TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// The configuration an update row replaces.
const UPDATED_CONFIGURATION: &str = "wording-probe-config";

/// A component document with one build phase, so a `CreateComponent` row is refused for the
/// parameter it is about rather than for its document.
pub(crate) const BUILD_DOCUMENT: &str = "name: Probe\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: run\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hi\n";

/// One row of the table: what established the value is not admitted, the request, the
/// parameter the refusal names and the requirement it states for it.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    /// The whole sentence the refusal must state.
    message: String,
}

/// One emulator: imagebuilder and IAM on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Status and parsed body of one imagebuilder call, with the error shape the header names
/// folded into the body as `$.__type` when the body carries none.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value, String) {
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
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(shape_name)
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed, error_type)
}

/// The shape name a discriminator value names, without its namespace or locator.
pub(crate) fn shape_name(raw: &str) -> String {
    raw.rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One `awsQuery` IAM action: `POST /` with the parameters in the form body. Every value
/// here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> u16 {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://iam.{REGION}.amazonaws.com/"))
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    router.dispatch(request).await.status().as_u16()
}

/// Create the one instance profile the accepted calls below resolve against, the way AWS
/// builds one.
pub(crate) async fn stage_iam(router: &MultiServiceRouter) {
    for (action, parameters) in [
        (
            "CreateRole",
            vec![
                ("RoleName", HELD_PROFILE),
                ("AssumeRolePolicyDocument", EC2_TRUST_POLICY),
            ],
        ),
        (
            "CreateInstanceProfile",
            vec![("InstanceProfileName", HELD_PROFILE), ("Path", "/")],
        ),
        (
            "AddRoleToInstanceProfile",
            vec![
                ("InstanceProfileName", HELD_PROFILE),
                ("RoleName", HELD_PROFILE),
            ],
        ),
    ] {
        let status = iam(router, action, &parameters).await;
        assert_eq!(
            status, 200,
            "setup: IAM {action} must answer 200, got {status}"
        );
    }
}

/// The ARN a configuration named `name` carries in [`REGION`].
pub(crate) fn configuration_arn(name: &str) -> String {
    format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/{name}")
}

/// The sentence this service states for `parameter`, whatever established the value is not
/// admitted — a modelled constraint, a lookup against the service that owns the resource the
/// value names, a parse, or a token this service did not issue
/// (`service_common::validation_message::parameter_value_mismatch`).
fn constraint_message(parameter: &str, requirement: &str) -> String {
    format!("The value supplied for parameter '{parameter}' is not valid. {requirement}")
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// A name no instance profile was ever created under: well-formed for the member's
/// `@pattern ^[\w+=,.@-]+$`, so the model admits it and the IAM lookup is what places it.
pub(crate) const ABSENT_PROFILE: &str = "ImageBuilderProfileThatWasNeverCreated";

/// The requirement clause stated for an instance profile this account holds none of.
pub(crate) fn absent_profile_requirement() -> String {
    "The provided instance profile does not exist. Please specify a different instance \
     profile and try again."
        .to_string()
}

/// The rows: one per thing that establishes a value is not one this service admits.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // A constraint of the model: the version pattern of `VersionNumber`
            // (`smithy_query(query="shape",shape="VersionNumber")`).
            case: "a value a modelled pattern does not admit",
            method: "PUT",
            uri: "/CreateComponent",
            body: Some(json!({
                "name": "wording-probe-component",
                "semanticVersion": "1.0",
                "platform": "Linux",
                "clientToken": "wtok-0001",
                "data": BUILD_DOCUMENT,
            })),
            message: constraint_message(
                "semanticVersion",
                r"semanticVersion must match pattern ^[0-9]+\.[0-9]+\.[0-9]+$",
            ),
        },
        Row {
            // An IAM lookup: the model admits the name, and this account holds no profile
            // under it.
            case: "an instance profile this account holds none of, on a create",
            method: "PUT",
            uri: "/CreateInfrastructureConfiguration",
            body: Some(json!({
                "name": "absent-profile-wording-config",
                "instanceProfileName": ABSENT_PROFILE,
                "clientToken": "wtok-0002",
            })),
            message: constraint_message("instanceProfileName", &absent_profile_requirement()),
        },
        Row {
            // The SAME refusal from a second operation: one sentence, not one per handler.
            case: "an instance profile this account holds none of, on an update",
            method: "PUT",
            uri: "/UpdateInfrastructureConfiguration",
            body: Some(json!({
                "infrastructureConfigurationArn": configuration_arn(UPDATED_CONFIGURATION),
                "instanceProfileName": ABSENT_PROFILE,
                "clientToken": "wtok-0003",
            })),
            message: constraint_message("instanceProfileName", &absent_profile_requirement()),
        },
        Row {
            // A parse: a resource policy is a JSON document, and this one is not.
            case: "a policy document that is not JSON",
            method: "PUT",
            uri: "/PutComponentPolicy",
            body: Some(json!({
                "componentArn": format!(
                    "arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/wording-probe/1.0.0/1"
                ),
                "policy": "not-a-json-document",
            })),
            message: constraint_message("policy", "Policy is malformed and cannot be parsed"),
        },
        Row {
            // An opaque token: the only value the member admits is one a previous page of
            // that same listing handed back.
            case: "a page token this service did not issue, on a listing of components",
            method: "POST",
            uri: "/ListComponents",
            body: Some(json!({ "nextToken": "not-a-token-this-service-issued" })),
            message: constraint_message(
                "nextToken",
                "The provided token does not meet the required format.",
            ),
        },
        Row {
            case: "a page token this service did not issue, on a listing of configurations",
            method: "POST",
            uri: "/ListInfrastructureConfigurations",
            body: Some(json!({ "nextToken": "not-a-token-this-service-issued" })),
            message: constraint_message(
                "nextToken",
                "The provided token does not meet the required format.",
            ),
        },
    ]
}

#[tokio::test]
async fn every_parameter_value_this_service_refuses_is_stated_in_one_sentence() {
    let router = emulator();
    stage_iam(&router).await;

    // The configuration the update row replaces: created with the profile this account DOES
    // hold, so the update's refusal is about the value that request carried.
    let (status, created, _) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": UPDATED_CONFIGURATION,
            "instanceProfileName": HELD_PROFILE,
            "clientToken": "wtok-setup",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the create must succeed, got {status} with {created}"
    );

    for row in rows() {
        let (status, refused, error_type) =
            call(&router, row.method, row.uri, row.body.as_ref()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {refused}",
            row.case
        );
        let code = refused
            .get("__type")
            .and_then(Value::as_str)
            .map(shape_name)
            .unwrap_or_default();
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {:?}: expected $.__type {INVALID_PARAMETER_VALUE:?}, got {code:?}",
            row.case
        );
        assert_eq!(
            error_type, INVALID_PARAMETER_VALUE,
            "case {:?}: expected x-amzn-errortype {INVALID_PARAMETER_VALUE:?}, got \
             {error_type:?}",
            row.case
        );
        let expected = row.message.as_str();
        let message = refused
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {:?}: expected the message {expected:?}, got {message:?}",
            row.case
        );
        let request_id = refused
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            is_request_id(request_id),
            "case {:?}: expected the refusal to name the request it answers, got \
             {request_id:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_refused_parameter_value_leaves_the_store_as_it_was() {
    let router = emulator();
    stage_iam(&router).await;

    let arn = configuration_arn(UPDATED_CONFIGURATION);
    let (status, created, _) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": UPDATED_CONFIGURATION,
            "instanceProfileName": HELD_PROFILE,
            "clientToken": "stok-setup",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: the create must succeed, got {status} with {created}"
    );

    // A create refused for its instance profile stores NOTHING: the ARN it would have
    // minted names nothing.
    let absent = "absent-profile-store-config";
    let (status, refused, _) = call(
        &router,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&json!({
            "name": absent,
            "instanceProfileName": ABSENT_PROFILE,
            "clientToken": "stok-0001",
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a create refused for its profile\": expected HTTP 400, got {status} with \
         {refused}"
    );
    let (status, missing) = read(&router, &configuration_arn(absent)).await;
    assert_eq!(
        status, 404,
        "case \"a create refused for its profile\": it must store nothing, got {status} with \
         {missing}"
    );

    // An update refused for its instance profile replaces NOTHING: the stored configuration
    // still reports the profile it was created with.
    let (status, refused, _) = call(
        &router,
        "PUT",
        "/UpdateInfrastructureConfiguration",
        Some(&json!({
            "infrastructureConfigurationArn": arn,
            "instanceProfileName": ABSENT_PROFILE,
            "clientToken": "stok-0002",
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an update refused for its profile\": expected HTTP 400, got {status} with \
         {refused}"
    );
    let (status, read_back) = read(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case \"an update refused for its profile\": the configuration must read back, got \
         {status} with {read_back}"
    );
    let reported = read_back["infrastructureConfiguration"]["instanceProfileName"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        reported, HELD_PROFILE,
        "case \"an update refused for its profile\": expected the stored profile \
         {HELD_PROFILE:?}, got {reported:?}"
    );
}

/// `GetInfrastructureConfiguration` for `arn`.
async fn read(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let (status, body, _) = call(
        router,
        "GET",
        &format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
        None,
    )
    .await;
    (status, body)
}
