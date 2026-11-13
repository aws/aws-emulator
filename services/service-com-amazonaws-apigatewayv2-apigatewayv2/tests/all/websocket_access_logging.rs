// SPDX-License-Identifier: CC0-1.0

//! What a deployed WEBSOCKET api stage LOGS about the calls a connection makes: the
//! access-log record its own `AccessLogSettings` ask for.
//!
//! The rule under test, one level above the recorded case:
//!
//! > A WEBSOCKET stage carrying `AccessLogSettings` — a `DestinationArn` naming a log group
//! > and a `$context` format — writes ONE record per ROUTE EXECUTION into THAT group,
//! > rendered by THAT format: the connection request, each message a client sends and the
//! > close are each one record, a call the gateway REFUSED is a call it answered and so is
//! > logged too, and a record's `$context.requestId` is the id the event that call's
//! > integration was handed reported, which is what lets a client join a run to the record
//! > of it. A variable the call has no value for renders as the placeholder a field with no
//! > value carries. A stage carrying no `AccessLogSettings` delivers nothing.
//!
//! Only a cross-service suite can pin this: the api, the stage and its logging configuration
//! are this service's, a route's answer belongs to a function AWS Lambda holds and runs, and
//! the group the records land in is CloudWatch Logs' — all three on ONE engine, as
//! `core/router/src/lib.rs` mounts them. Every assertion reads the outcome back through
//! CloudWatch Logs' own reader (`FilterLogEvents`), so a record that did not reach that store
//! cannot pass. The session itself is driven over a REAL connection to the socket the api
//! announces.
//!
//! The rows differ along what the rule generalises over: the CALL (the connection request, a
//! message, the close), the OUTCOME (a route that served it, a message the api routes
//! nowhere, a connection request the backend refused) and the FORMAT (a JSON one and a
//! Common-Log-Format one).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_in_region};
use crate::http_api_access_logging::{delivered_to, emulator, log_group_arn};
use crate::http_api_backend_dispatch::given_function;
use crate::websocket_transport_data_plane::{exchange, handshake_at, ClientSocket};

/// The Region every request below is signed for and every ARN names — the one the Lambda
/// harness creates its functions in, so the api and the function it names live in the same
/// Region.
const REGION: &str = "us-east-1";

/// The stage every case deploys.
const STAGE: &str = "prod";

/// The destination each case points its stage at.
const GROUP: &str = "/apigwv2/websocket-access";

/// The format most cases render their records by: a JSON one naming the members only a
/// WebSocket call has beside the ones every API Gateway call has.
const JSON_FORMAT: &str = concat!(
    r#"{"requestId":"$context.requestId","eventType":"$context.eventType","#,
    r#""routeKey":"$context.routeKey","status":"$context.status","#,
    r#""connectionId":"$context.connectionId","stage":"$context.stage","#,
    r#""messageDirection":"$context.messageDirection"}"#
);

/// The other format a case renders by: the Common-Log-Format shape, which names a member no
/// WebSocket call carries (`$context.identity.user`) so the placeholder is asserted too.
const CLF_FORMAT: &str = concat!(
    r#"$context.identity.sourceIp - $context.identity.user [$context.requestTime] ""#,
    r#"$context.eventType $context.routeKey" $context.status $context.requestId"#
);

/// How long a case waits for the records of a session, and how often it looks: a record is
/// delivered after the call it is about was answered, so it is read by polling the group.
const RECORD_WAIT: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// A handler that ADMITS the connection and echoes a message: the `2xx` a connection request
/// completes on, and a body for a two-way route to send back.
///
/// The body carries the id the gateway told the handler this call is
/// (`requestContext.requestId`), which is what makes the record's `$context.requestId`
/// assertable against the id the INTEGRATION observed rather than against itself.
fn admitting_handler() -> String {
    "import json\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   observed = event[\"requestContext\"][\"requestId\"]\n\
     \x20   return {\"statusCode\": 200,\n\
     \x20           \"body\": json.dumps({\"ok\": True, \"requestId\": observed})}\n"
        .to_string()
}

/// A handler that REFUSES the connection request, which is what a `$connect` backend does to
/// turn a client away: the status it answers is the status the handshake is refused with.
fn refusing_handler() -> String {
    "import json\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   return {\"statusCode\": 403, \"body\": json.dumps({\"message\": \"Nope\"})}\n"
        .to_string()
}

/// The three services this rule spans, over ONE engine: this service, the one that holds and
/// runs the functions its routes front, and the one the records land in. The harness is the
/// HTTP plane's own access-log suite's ([`crate::http_api_access_logging`]), which spans the
/// same three — one copy, so the wire a record is read back over cannot drift between the
/// two planes.
async fn delivered(router: &MultiServiceRouter) -> Option<Vec<String>> {
    delivered_to(router, GROUP).await
}

/// Look again until the group holds at least `expected` records, or the wait above runs out:
/// a record is delivered after its call was answered, so the client that drove the call
/// waits for the CONDITION rather than for a fixed time.
async fn records_once_at_least(
    router: &MultiServiceRouter,
    case: &str,
    expected: usize,
) -> Vec<String> {
    let deadline = Instant::now() + RECORD_WAIT;
    let mut records = delivered(router).await.unwrap_or_default();
    while records.len() < expected && Instant::now() < deadline {
        tokio::time::sleep(LOOK_INTERVAL).await;
        records = delivered(router).await.unwrap_or_default();
    }
    assert!(
        records.len() >= expected,
        "case {case:?}: expected at least {expected} record(s) in {GROUP:?}, got {records:?}"
    );
    records
}

/// The ARN of the destination log group, as a stage's `DestinationArn` names one.
fn group_arn() -> String {
    log_group_arn(GROUP)
}

/// One deployed api: the `host:port` of the socket its `ApiEndpoint` announces, which is
/// what a case dials.
struct Api {
    authority: String,
}

/// One control-plane request that must answer `201`.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    uri: &str,
    body: Value,
) -> Value {
    let (status, answered) = call_in_region(router, REGION, "POST", uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: {what} must answer 201, got {status} {answered}"
    );
    answered
}

/// A deployed WEBSOCKET api whose `$connect` route fronts `connect_function`, whose `ping`
/// route fronts `message_function` two-way, and whose stage logs by `format` (or by nothing
/// at all when `format` is `None`).
async fn given_api(
    router: &MultiServiceRouter,
    case: &str,
    connect_function: &str,
    message_function: &str,
    format: Option<&str>,
) -> Api {
    let api = created(
        router,
        case,
        "CreateApi",
        "/v2/apis",
        json!({
            "name": "access-logged-socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let api_id = as_str(&api, "apiId");
    let announced = as_str(&api, "apiEndpoint");
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a WEBSOCKET api must announce a wss:// socket, got {announced:?}")
        })
        .to_string();

    for (route_key, function, two_way) in [
        ("$connect", connect_function, false),
        ("$disconnect", message_function, false),
        ("ping", message_function, true),
    ] {
        let integration = created(
            router,
            case,
            "CreateIntegration",
            &format!("/v2/apis/{api_id}/integrations"),
            json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": function,
                "integrationMethod": "POST"
            }),
        )
        .await;
        let integration_id = as_str(&integration, "integrationId");
        let route = created(
            router,
            case,
            "CreateRoute",
            &format!("/v2/apis/{api_id}/routes"),
            json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}")
            }),
        )
        .await;
        if two_way {
            let route_id = as_str(&route, "routeId");
            created(
                router,
                case,
                "CreateRouteResponse",
                &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
                json!({ "routeResponseKey": "$default" }),
            )
            .await;
        }
    }
    let mut stage = json!({ "stageName": STAGE });
    if let Some(format) = format {
        stage["accessLogSettings"] = json!({
            "destinationArn": group_arn(),
            "format": format,
        });
    }
    created(
        router,
        case,
        "CreateStage",
        &format!("/v2/apis/{api_id}/stages"),
        stage,
    )
    .await;
    created(
        router,
        case,
        "CreateDeployment",
        &format!("/v2/apis/{api_id}/deployments"),
        json!({ "stageName": STAGE }),
    )
    .await;
    Api {
        authority,
    }
}

/// One record, read back as the JSON [`JSON_FORMAT`] renders.
fn parsed(case: &str, record: &str) -> Value {
    serde_json::from_str(record).unwrap_or_else(|error| {
        panic!("case {case:?}: the record {record:?} is not the JSON the format is: {error}")
    })
}

/// A session that connects, sends one routed message and one the api routes nowhere, and
/// closes leaves one record per route execution — each naming the call it is about.
#[tokio::test]
async fn every_call_of_a_session_leaves_one_record_rendered_by_the_stages_format() {
    let case = "a session over an access-logged stage";
    let (router, engine) = emulator();
    let admitting =
        given_function(&router, &engine, case, "socket-access-admit", &admitting_handler()).await;
    let api = given_api(&router, case, &admitting, &admitting, Some(JSON_FORMAT)).await;

    let mut socket: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must be completed, got {status}")
        });
    // The connection request is the first route execution, and its record carries the
    // handshake's own status.
    let records = records_once_at_least(&router, case, 1).await;
    let connect = parsed(case, &records[0]);
    let connection_id = connect
        .get("connectionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !connection_id.is_empty(),
        "case {case:?}: the record must name the connection, got {records:?}"
    );
    assert_eq!(
        connect,
        json!({
            "requestId": as_text(&connect, "requestId"),
            "eventType": "CONNECT",
            "routeKey": "$connect",
            "status": "101",
            "connectionId": connection_id,
            "stage": STAGE,
            "messageDirection": "IN",
        }),
        "case {case:?}: expected the connection request's own record, got {records:?}"
    );

    // A message a route serves: the record names that route and the status its integration
    // answered, and the id the record names is the id the INTEGRATION was told the call is —
    // the handler answers it, so the two can be compared rather than the record with itself.
    let replied = exchange(case, &mut socket, r#"{"action":"ping"}"#).await;
    let reply = replied
        .unwrap_or_else(|| panic!("case {case:?}: the two-way route must answer the frame"));
    let observed = parsed(case, &reply);
    let observed_id = as_text(&observed, "requestId");
    assert!(
        !observed_id.is_empty(),
        "case {case:?}: the handler must report the id it was given, got {reply:?}"
    );
    let records = records_once_at_least(&router, case, 2).await;
    let message = parsed(case, &records[1]);
    assert_eq!(
        message,
        json!({
            "requestId": observed_id,
            "eventType": "MESSAGE",
            "routeKey": "ping",
            "status": "200",
            "connectionId": connection_id,
            "stage": STAGE,
            "messageDirection": "IN",
        }),
        "case {case:?}: expected the message's own record, naming the call the integration \
         observed ({observed_id:?}), got {records:?}"
    );

    // A message the api routes nowhere is a call the gateway refused, and a refused call is
    // one the stage logged.
    let replied = exchange(case, &mut socket, r#"{"action":"nowhere"}"#).await;
    assert!(
        replied.is_some_and(|reply| reply.contains("Forbidden")),
        "case {case:?}: an unrouted action must be answered the gateway's own frame"
    );
    let records = records_once_at_least(&router, case, 3).await;
    let unrouted = parsed(case, &records[2]);
    assert_eq!(
        unrouted,
        json!({
            "requestId": as_text(&unrouted, "requestId"),
            "eventType": "MESSAGE",
            // No route served it, so the format's reference renders as the placeholder.
            "routeKey": "-",
            "status": "403",
            "connectionId": connection_id,
            "stage": STAGE,
            "messageDirection": "IN",
        }),
        "case {case:?}: expected the unrouted message's own record, got {records:?}"
    );

    // The close is the session's last route execution.
    socket
        .close(None)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the close must be sent, got {error}"));
    let records = records_once_at_least(&router, case, 4).await;
    let disconnect = parsed(case, &records[3]);
    assert_eq!(
        disconnect,
        json!({
            "requestId": as_text(&disconnect, "requestId"),
            "eventType": "DISCONNECT",
            "routeKey": "$disconnect",
            "status": "200",
            "connectionId": connection_id,
            "stage": STAGE,
            "messageDirection": "IN",
        }),
        "case {case:?}: expected the close's own record, got {records:?}"
    );

    // Every call of the connection is one call: the four records name four different calls
    // of the same connection.
    let ids: Vec<String> = records
        .iter()
        .map(|record| as_text(&parsed(case, record), "requestId"))
        .collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case {case:?}: each call is named by its own requestId, got {ids:?}"
    );
}

/// The value `member` carries in a record, as the record's own text.
fn as_text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A connection request the `$connect` backend REFUSED is logged — with the status the
/// handshake was refused with — and it is the only record such a session leaves.
#[tokio::test]
async fn a_refused_connection_request_is_the_only_record_of_that_session() {
    let case = "a connection request the backend refused";
    let (router, engine) = emulator();
    let refusing =
        given_function(&router, &engine, case, "socket-access-refuse", &refusing_handler()).await;
    let api = given_api(&router, case, &refusing, &refusing, Some(JSON_FORMAT)).await;

    let refused = handshake_at(case, &api.authority, &format!("/{STAGE}")).await;
    assert!(
        matches!(refused, Err(403)),
        "case {case:?}: the backend's own status must refuse the handshake, got {:?}",
        refused.map(|_| "a completed handshake")
    );
    let records = records_once_at_least(&router, case, 1).await;
    let record = parsed(case, &records[0]);
    assert_eq!(
        record,
        json!({
            "requestId": as_text(&record, "requestId"),
            "eventType": "CONNECT",
            "routeKey": "$connect",
            "status": "403",
            "connectionId": as_text(&record, "connectionId"),
            "stage": STAGE,
            "messageDirection": "IN",
        }),
        "case {case:?}: expected the refused connection request's own record, got {records:?}"
    );
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: a refused handshake carries no message and no close, got {records:?}"
    );
}

/// The same session logged by a stage whose format is a Common-Log-Format line: the same
/// facts in that format's own field order, with the placeholder for a member no call carries.
#[tokio::test]
async fn a_common_log_format_stage_renders_the_same_calls_in_that_format() {
    let case = "a clf-formatted stage";
    let (router, engine) = emulator();
    let admitting =
        given_function(&router, &engine, case, "socket-access-clf", &admitting_handler()).await;
    let api = given_api(&router, case, &admitting, &admitting, Some(CLF_FORMAT)).await;

    let mut socket: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must be completed, got {status}")
        });
    exchange(case, &mut socket, r#"{"action":"ping"}"#).await;
    let records = records_once_at_least(&router, case, 2).await;
    for (what, expected) in [
        ("the connection request", "\"CONNECT $connect\" 101 "),
        ("the message", "\"MESSAGE ping\" 200 "),
    ] {
        assert!(
            records.iter().any(|record| {
                record.starts_with("127.0.0.1 - - [") && record.contains(expected)
            }),
            "case {case:?}: expected a Common Log Format line for {what} carrying \
             {expected:?}, got {records:?}"
        );
    }
}

/// A WEBSOCKET stage carrying no `AccessLogSettings` serves its session and delivers nothing.
#[tokio::test]
async fn a_stage_with_no_access_log_settings_delivers_nothing() {
    let case = "no access log settings";
    let (router, engine) = emulator();
    let admitting =
        given_function(&router, &engine, case, "socket-access-none", &admitting_handler()).await;
    let api = given_api(&router, case, &admitting, &admitting, None).await;

    let mut socket: ClientSocket = handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| {
            panic!("case {case:?}: the handshake must be completed, got {status}")
        });
    let replied = exchange(case, &mut socket, r#"{"action":"ping"}"#).await;
    assert!(
        replied.is_some(),
        "case {case:?}: the stage must keep serving its session"
    );
    assert_eq!(
        delivered(&router).await,
        None,
        "case {case:?}: expected no destination group {GROUP:?} to exist at all"
    );
}
