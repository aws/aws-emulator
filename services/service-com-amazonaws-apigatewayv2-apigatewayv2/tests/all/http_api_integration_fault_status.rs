// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` HTTP data plane's FAULT PROPAGATION: which status a client is answered
//! with when the route a deployed HTTP api stage resolved cannot produce a backend response.
//!
//! The rule these cases assert, one level above the recorded cases (one `$default` stage
//! created with `AutoDeploy` on over a Lambda `AWS_PROXY` integration whose
//! `TimeoutInMillis` is 50):
//!
//! > Every status an `execute-api` invoke is answered with is computed from the api's own
//! > stored records and names the fault class it met, never the emulator's scaffold channel
//! > and never a success no backend produced: a stage with no deployment released to it
//! > serves nothing and is `403` `{"message":"Forbidden"}`; a path naming no stage of the api,
//! > and a method and path no route key of the api matches, name nothing the endpoint has and
//! > are `404` `{"message":"Not Found"}`; and a matched route whose integration produced no
//! > response is `500` `{"message":"Internal Server Error"}`, the status an HTTP api maps an
//! > integration fault to, whatever that integration's `TimeoutInMillis`,
//! > `PayloadFormatVersion` or target is.
//!
//! The `500` is the HTTP api's own mapping, and the two statuses it is NOT are pinned here
//! because a client branches on them: it is not the `502` a REST api answers for a malformed
//! Lambda proxy response (`Bad Gateway` belongs to the other data plane), and it is not the
//! `501` `NotImplemented` the scaffold channel answers, which no `execute-api` endpoint ever
//! produces.
//!
//! What these cases cannot reach, stated so a passing run is not read as more than it is: the
//! `503` an HTTP api answers when an integration RAN and overran its `TimeoutInMillis`. That
//! class needs a backend that actually ran, and an `AWS_PROXY` integration's backend is a
//! Lambda deployment package this emulator has no in-process runtime for (lambda `Invoke`
//! answers `501` for `RequestResponse`), so every `AWS_PROXY` route here meets the
//! "produced no response" class above instead. The runtime is owned by the open issues
//! `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`
//! and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`,
//! and the dispatch half by
//! `20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`.
//!
//! The rows span what the rule generalises over: the integration's `TimeoutInMillis` (absent,
//! each published bound of the window the shape declares, and a value inside it), its
//! `PayloadFormatVersion` (both values the model admits), the form of the route key (a
//! literal path, a path variable, the greedy remainder, the `$default` catch-all), which
//! stage serves the request (the one that serves the api's root and a named one), how the
//! request addresses the api (its own `execute-api` host and the emulator's own endpoint with
//! the api id in the path), and which fault class the request meets.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The region the control-plane requests are signed for, and so the region the api's own
/// `ApiEndpoint` host names.
const REGION: &str = "eu-west-1";

/// The stage that serves an api's root, reached with no stage segment in the path at all.
const ROOT_STAGE: &str = "$default";

/// The status and sentence an HTTP api answers for a matched route whose integration
/// produced no response.
const INTEGRATION_FAULT: u16 = 500;
const INTEGRATION_FAULT_MESSAGE: &str = "Internal Server Error";

/// The status and sentence for an endpoint that serves nothing: an api or stage this
/// endpoint does not serve, and a stage with no deployment released to it.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status and sentence for a method and path no route key of the api matches.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status the REST data plane answers for a malformed Lambda proxy response. An HTTP
/// api answers `500` for every integration fault, so this must never be seen here.
const BAD_GATEWAY: u16 = 502;

/// The status the scaffold channel answers for an operation that is not implemented
/// (`crate::scaffolded`), paired with [`SCAFFOLD_ERROR_CODE`]: a data plane is no modelled
/// operation, so neither is ever a legitimate answer on an invoke URL.
const SCAFFOLD_STATUS: u16 = 501;
const SCAFFOLD_ERROR_CODE: &str = "NotImplemented";

/// The error code the front door answers when NO service claims a request.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The lower bound of `IntegerWithLengthBetween50And30000`, the shape
/// `CreateIntegration$TimeoutInMillis` targets — and the window the recorded cases sit at
/// the bottom of.
const MIN_TIMEOUT_MILLIS: u32 = 50;

/// The upper bound of that same shape.
const MAX_TIMEOUT_MILLIS: u32 = 30_000;

/// A Lambda proxy integration: "AWS_PROXY: for integrating the route or method request with
/// the Lambda function-invoking action" (`IntegrationType`), with the `PayloadFormatVersion`
/// and `TimeoutInMillis` the row asks for.
fn lambda_proxy(
    function: &str,
    payload_format_version: &str,
    timeout_in_millis: Option<u32>,
) -> Value {
    let mut integration = json!({
        "integrationType": "AWS_PROXY",
        "integrationUri": format!("arn:aws:lambda:{REGION}:000000000000:function:{function}"),
        "payloadFormatVersion": payload_format_version
    });
    if let Some(timeout) = timeout_in_millis {
        integration["timeoutInMillis"] = json!(timeout);
    }
    integration
}

/// What a client needs to address a deployed api: its id and the host its own `ApiEndpoint`
/// names.
struct Deployment {
    api_id: String,
    endpoint_host: String,
}

impl Deployment {
    /// The path a request carries to reach `route_path` of `stage`: the stage that serves the
    /// api's root is reached with no stage segment at all, a named one with its name ahead of
    /// the route path ("The stage name is typically appended to this URI to form a complete
    /// path to a deployed API stage", `Api$ApiEndpoint`).
    fn path(&self, stage: &str, route_path: &str) -> String {
        if stage == ROOT_STAGE {
            route_path.to_string()
        } else {
            format!("/{stage}{route_path}")
        }
    }
}

/// Create an api, one integration, the routes that front it and a stage over them; answer
/// what a client needs to address it.
///
/// `stage` is the whole `CreateStage` body, so a row can deploy the stage (`AutoDeploy`) or
/// leave it with nothing released to it.
async fn deploy(
    emulator: &MultiServiceRouter,
    case: &str,
    name: &str,
    integration: Value,
    route_keys: &[&str],
    stage: Value,
) -> Deployment {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {created}"
    );
    let integration_id = as_str(&created, "integrationId");

    for route_key in route_keys {
        let (status, route) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRoute for {route_key:?} must answer 201, got {status} {route}"
        );
    }

    let (status, created_stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(stage.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {created_stage}"
    );
    let released = created_stage.get("deploymentId").and_then(Value::as_str);
    let asked_for_deployment = stage.get("autoDeploy") == Some(&json!(true));
    assert_eq!(
        released.is_some(),
        asked_for_deployment,
        "case {case:?}: a stage reports a deployment exactly when one was released to it, \
         got {created_stage}"
    );

    Deployment {
        api_id,
        endpoint_host,
    }
}

/// Assert one invoke was answered with the status and `{"message": …}` body the fault class
/// maps to — and with none of the answers that would mean the data plane did not decide it.
fn answered(case: &str, answer: &Answer, status: u16, body: &str) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the invoke URL must be served by this service, got {}",
        answer.summary()
    );
    assert!(
        answer.status != SCAFFOLD_STATUS && answer.error_type.as_deref() != Some(SCAFFOLD_ERROR_CODE),
        "case {case:?}: an invoke is never answered from the scaffold channel, got {}",
        answer.summary()
    );
    assert_ne!(
        answer.status, BAD_GATEWAY,
        "case {case:?}: an HTTP api maps an integration fault to {INTEGRATION_FAULT}, not to \
         the REST data plane's {BAD_GATEWAY}, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected {status}, got {}",
        answer.summary()
    );
    assert_eq!(
        message(answer),
        body,
        "case {case:?}: expected the {body:?} body, got {}",
        answer.summary()
    );
}

/// One row of the integration-fault table: how the Lambda-backed integration behind the
/// route is configured, which stage serves it, and the request that reaches it.
struct FaultRow {
    case: &'static str,
    payload_format_version: &'static str,
    timeout_in_millis: Option<u32>,
    /// Every route key the api carries; the first one is the one the request must match.
    route_keys: &'static [&'static str],
    stage_name: &'static str,
    method: &'static str,
    route_path: &'static str,
}

fn fault_rows() -> &'static [FaultRow] {
    &[
        FaultRow {
            case: "the recorded case: a 50 ms timeout at the bottom of the window, payload \
                   format 2.0, under the stage that serves the api's root",
            payload_format_version: "2.0",
            timeout_in_millis: Some(MIN_TIMEOUT_MILLIS),
            route_keys: &["GET /x"],
            stage_name: ROOT_STAGE,
            method: "GET",
            route_path: "/x",
        },
        FaultRow {
            case: "the top of the timeout window",
            payload_format_version: "2.0",
            timeout_in_millis: Some(MAX_TIMEOUT_MILLIS),
            route_keys: &["POST /orders"],
            stage_name: ROOT_STAGE,
            method: "POST",
            route_path: "/orders",
        },
        FaultRow {
            case: "a timeout inside the window, on a route key binding a path variable",
            payload_format_version: "2.0",
            timeout_in_millis: Some(1_234),
            route_keys: &["DELETE /items/{id}"],
            stage_name: ROOT_STAGE,
            method: "DELETE",
            route_path: "/items/42",
        },
        FaultRow {
            case: "no timeout named at all, payload format 1.0, on the greedy remainder \
                   under a named stage",
            payload_format_version: "1.0",
            timeout_in_millis: None,
            route_keys: &["ANY /{proxy+}"],
            stage_name: "prod",
            method: "GET",
            route_path: "/deep/er/still",
        },
        FaultRow {
            case: "the $default route catch-all under a named stage",
            payload_format_version: "1.0",
            timeout_in_millis: Some(MIN_TIMEOUT_MILLIS),
            route_keys: &["$default"],
            stage_name: "live",
            method: "PUT",
            route_path: "/anything",
        },
    ]
}

/// Every configuration of a Lambda-backed route meets the SAME fault class — its integration
/// produced no response — so every one of them is answered `500`
/// `{"message":"Internal Server Error"}`, on the api's own endpoint host and on the
/// emulator's own endpoint with the api id in the path.
#[tokio::test]
async fn a_route_whose_integration_produces_no_response_is_answered_five_hundred() {
    for row in fault_rows() {
        let case = row.case;
        let emulator = emulator();
        let deployment = deploy(
            &emulator,
            case,
            "fault-status-api",
            lambda_proxy(
                "fault-status-backend",
                row.payload_format_version,
                row.timeout_in_millis,
            ),
            row.route_keys,
            json!({ "stageName": row.stage_name, "autoDeploy": true }),
        )
        .await;

        let stage_path = deployment.path(row.stage_name, row.route_path);
        // The api's own `ApiEndpoint` host, the addressing `Api$ApiEndpoint` documents.
        let answer = send(
            &emulator,
            &deployment.endpoint_host,
            row.method,
            &stage_path,
            &[],
        )
        .await;
        answered(case, &answer, INTEGRATION_FAULT, INTEGRATION_FAULT_MESSAGE);

        // The same request at the emulator's own endpoint, with the api id carried in the
        // path: the same records decide it, so the same fault class is answered.
        let rewritten = format!("/_aws/execute-api/{}{stage_path}", deployment.api_id);
        let answer = send(&emulator, "127.0.0.1:9297", row.method, &rewritten, &[]).await;
        answered(case, &answer, INTEGRATION_FAULT, INTEGRATION_FAULT_MESSAGE);
    }
}

/// One row of the other fault classes: what the api was given, what the request asks for,
/// and the status and sentence the class maps to.
struct ClassRow {
    case: &'static str,
    /// The `CreateStage` body: a stage with `AutoDeploy` on has a deployment released to it,
    /// one without has nothing to serve.
    stage: fn() -> Value,
    route_keys: &'static [&'static str],
    method: &'static str,
    /// The WHOLE path the request carries within the api — spelled out rather than built
    /// from the stage name, because two of these rows are about a path that names no stage
    /// of the api at all.
    request_path: &'static str,
    status: u16,
    message: &'static str,
}

fn class_rows() -> &'static [ClassRow] {
    &[
        ClassRow {
            case: "a stage with no deployment released to it serves nothing",
            stage: || json!({ "stageName": ROOT_STAGE }),
            route_keys: &["GET /x"],
            method: "GET",
            request_path: "/x",
            status: FORBIDDEN,
            message: FORBIDDEN_MESSAGE,
        },
        ClassRow {
            case: "a path naming no stage of the api, where the api has no root stage",
            stage: || json!({ "stageName": "prod", "autoDeploy": true }),
            route_keys: &["GET /x"],
            method: "GET",
            request_path: "/x",
            status: NOT_FOUND,
            message: NOT_FOUND_MESSAGE,
        },
        ClassRow {
            case: "a path no route key of the api matches",
            stage: || json!({ "stageName": ROOT_STAGE, "autoDeploy": true }),
            route_keys: &["GET /x"],
            method: "GET",
            request_path: "/nowhere",
            status: NOT_FOUND,
            message: NOT_FOUND_MESSAGE,
        },
        ClassRow {
            case: "a method no route key of the api names for that path",
            stage: || json!({ "stageName": ROOT_STAGE, "autoDeploy": true }),
            route_keys: &["GET /x"],
            method: "POST",
            request_path: "/x",
            status: NOT_FOUND,
            message: NOT_FOUND_MESSAGE,
        },
        ClassRow {
            case: "a deeper path than the only route key names",
            stage: || json!({ "stageName": ROOT_STAGE, "autoDeploy": true }),
            route_keys: &["GET /x"],
            method: "GET",
            request_path: "/x/deeper",
            status: NOT_FOUND,
            message: NOT_FOUND_MESSAGE,
        },
        ClassRow {
            case: "an unmatched path under a NAMED stage, which the request names ahead of it",
            stage: || json!({ "stageName": "prod", "autoDeploy": true }),
            route_keys: &["GET /x"],
            method: "GET",
            request_path: "/prod/nowhere",
            status: NOT_FOUND,
            message: NOT_FOUND_MESSAGE,
        },
    ]
}

/// The classes that are decided BEFORE the integration: an endpoint that serves nothing is
/// `403`, and a request no route key matches is `404` — each computed from the api's own
/// records, and neither of them the integration's `500`.
#[tokio::test]
async fn an_endpoint_that_serves_nothing_and_an_unmatched_request_name_their_own_class() {
    for row in class_rows() {
        let case = row.case;
        let emulator = emulator();
        let deployment = deploy(
            &emulator,
            case,
            "fault-class-api",
            lambda_proxy("fault-class-backend", "2.0", Some(MIN_TIMEOUT_MILLIS)),
            row.route_keys,
            (row.stage)(),
        )
        .await;
        let answer = send(
            &emulator,
            &deployment.endpoint_host,
            row.method,
            row.request_path,
            &[],
        )
        .await;
        answered(case, &answer, row.status, row.message);
    }
}

/// An invoke is a READ of the api's records: repeating it answers the same, and the stage it
/// addressed reports exactly what it reported before — no deployment moved, no counter bumped.
#[tokio::test]
async fn invoking_a_stage_changes_nothing_a_later_caller_sees() {
    let case = "an invoke repeated against one deployed stage";
    let emulator = emulator();
    let deployment = deploy(
        &emulator,
        case,
        "fault-idempotence-api",
        lambda_proxy("fault-idempotence-backend", "2.0", Some(MIN_TIMEOUT_MILLIS)),
        &["GET /x"],
        json!({ "stageName": ROOT_STAGE, "autoDeploy": true }),
    )
    .await;

    let (status, before) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{}/stages/{ROOT_STAGE}", deployment.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage must answer 200, got {status} {before}"
    );

    for attempt in 1..=3 {
        let answer = send(&emulator, &deployment.endpoint_host, "GET", "/x", &[]).await;
        answered(
            case,
            &answer,
            INTEGRATION_FAULT,
            INTEGRATION_FAULT_MESSAGE,
        );
        let (status, after) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{}/stages/{ROOT_STAGE}", deployment.api_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage must answer 200 after invoke {attempt}, got {status} {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: invoke {attempt} changed the stage GetStage reports"
        );
    }
}
