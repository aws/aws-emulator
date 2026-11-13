// SPDX-License-Identifier: CC0-1.0

//! Registering a `lambda` scalable target, whose resource is ONE qualified Lambda function.
//!
//! RULE: a scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
//! that resource in the service that owns it — `lambda:function:ProvisionedConcurrency` scales
//! the provisioned concurrency of a function, which is allocated to a published version or an
//! alias ("the unique identifier is the function name with a function version or alias name
//! suffix that is not `$LATEST`"), so a qualifier Lambda holds no version and no alias for
//! names no concurrency to scale and the registration is refused HTTP 400
//! `ValidationException` with nothing registered. Which refusal is which rule: a qualifier that
//! can carry no provisioned concurrency at all is refused for that qualifier, an id that is not
//! of the documented form is the triple that describes no one resource, and a well-formed id
//! whose qualifier the owning service does not hold is refused for the qualified ARN that was
//! looked for.
//!
//! The rows differ along what the rule generalises over: an alias that exists, a published
//! version that exists, a version number and an alias name that do not, a function that does
//! not exist at all, the unpublished version, and an unqualified id.
//!
//! This suite builds its OWN emulator rather than using the shared one
//! ([`super::emulator::Emulator`]), because the resource lives in a service the other suites do
//! not mount: AWS Lambda, where the function, its version and its alias are created through
//! that service's own API.

use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{error_code, error_message, ACCOUNT, REGION};

/// The dimension every row scales.
const DIMENSION: &str = "lambda:function:ProvisionedConcurrency";

/// Where Lambda binds the function operations this suite calls.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The function every row's resource id names, except the row whose function is absent.
const FUNCTION: &str = "ingest";

/// The alias created on that function's published version.
const ALIAS: &str = "live";

/// Application Auto Scaling, AWS Lambda — which owns the function — and the two services a
/// registration also touches, over ONE engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests; the `Authorization` header is still what routes each
    // request and names its region, as a signed client's would.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// Call one Application Auto Scaling operation.
async fn application_auto_scaling(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AnyScaleFrontendService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/application-autoscaling\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    (status, serde_json::from_slice(&payload).unwrap_or(Value::Null))
}

/// Call one Lambda operation, which that service binds to a REST route.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/lambda\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    (status, serde_json::from_slice(&payload).unwrap_or(Value::Null))
}

/// Create the function, publish version 1 of it, and point one alias at that version — the
/// resource a provisioned-concurrency target can be registered on.
async fn given_function(router: &MultiServiceRouter) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": {"ZipFile": base64::engine::general_purpose::STANDARD.encode(&package)},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created:?}"
    );
    let (status, alias) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/aliases"),
        Some(json!({"Name": ALIAS, "FunctionVersion": "1"})),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateAlias: expected 201, got {status} {alias:?}"
    );
}

/// The qualified ARN a refusal reports as the one it looked for.
fn qualified_arn(function: &str, qualifier: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{qualifier}")
}

/// The sentence a (namespace, resource id, dimension) triple that does not describe one
/// resource is refused with: fixed text, since it is the combination that is unsupported.
const UNSUPPORTED_TRIPLE: &str =
    "Unsupported service namespace, resource type or scalable dimension";

/// The sentence a resource id whose QUALIFIER can carry no provisioned concurrency is refused
/// with.
const UNSUPPORTED_QUALIFIER: &str = "$LATEST is not a supported qualifier for Auto Scaling";

/// One registration against a `lambda` resource id.
struct Row {
    case: &'static str,
    resource_id: &'static str,
    /// The sentence the registration is refused with, or `None` for one that is accepted.
    refusal: Option<String>,
}

/// The rows, over the span of qualifier forms.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an alias the function holds",
            resource_id: "function:ingest:live",
            refusal: None,
        },
        Row {
            case: "a published version the function holds",
            resource_id: "function:ingest:1",
            refusal: None,
        },
        Row {
            case: "an alias the function does not hold",
            resource_id: "function:ingest:never-created",
            refusal: Some(format!(
                "Validation failed for resource: function:ingest:never-created, scalable \
                 dimension: {DIMENSION}. Reason: Cannot find alias arn: {}",
                qualified_arn("ingest", "never-created")
            )),
        },
        Row {
            case: "a version number the function has not published",
            resource_id: "function:ingest:9",
            refusal: Some(format!(
                "Validation failed for resource: function:ingest:9, scalable dimension: \
                 {DIMENSION}. Reason: Cannot find version arn: {}",
                qualified_arn("ingest", "9")
            )),
        },
        Row {
            case: "an alias of a function that does not exist at all",
            resource_id: "function:absent-fn:live",
            refusal: Some(format!(
                "Validation failed for resource: function:absent-fn:live, scalable dimension: \
                 {DIMENSION}. Reason: Cannot find alias arn: {}",
                qualified_arn("absent-fn", "live")
            )),
        },
        Row {
            case: "the unpublished version, which can carry no provisioned concurrency",
            resource_id: "function:ingest:$LATEST",
            refusal: Some(UNSUPPORTED_QUALIFIER.to_string()),
        },
        Row {
            case: "an unqualified function, which is not the documented id form",
            resource_id: "function:ingest",
            refusal: Some(UNSUPPORTED_TRIPLE.to_string()),
        },
    ]
}

/// The scalable targets this service reports for one resource id.
async fn targets(router: &MultiServiceRouter, resource_id: &str) -> Vec<Value> {
    let (status, body) = application_auto_scaling(
        router,
        "DescribeScalableTargets",
        json!({
            "ServiceNamespace": "lambda",
            "ResourceIds": [resource_id],
            "ScalableDimension": DIMENSION,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeScalableTargets to answer 200, got {status} {body:?}"
    );
    body["ScalableTargets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn provisioned_concurrency_is_only_registered_on_a_qualifier_lambda_holds() {
    for row in rows() {
        let case = row.case;
        let router = router();
        given_function(&router).await;

        let (status, body) = application_auto_scaling(
            &router,
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "lambda",
                "ResourceId": row.resource_id,
                "ScalableDimension": DIMENSION,
                "MinCapacity": 0,
                "MaxCapacity": 2,
            }),
        )
        .await;

        let registered = targets(&router, row.resource_id).await;
        match &row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the registration to be accepted, got {status} \
                     {body:?}"
                );
                assert_eq!(
                    registered.len(),
                    1,
                    "case {case:?}: expected the registered target to be reported, got \
                     {registered:?}"
                );
                assert_eq!(
                    (
                        registered[0]["MinCapacity"].clone(),
                        registered[0]["MaxCapacity"].clone()
                    ),
                    (json!(0), json!(2)),
                    "case {case:?}: expected the registered window, got {:?}",
                    registered[0]
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body:?}"
                );
                let message = error_message(&body);
                assert_eq!(
                    &message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                assert!(
                    registered.is_empty(),
                    "case {case:?}: expected a refused registration to register nothing, got \
                     {registered:?}"
                );
            }
        }
    }
}
