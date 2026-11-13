// SPDX-License-Identifier: CC0-1.0

//! The WEBSOCKET api's own SOCKET, driven over a real connection: the RFC 6455 opening
//! handshake it completes (or refuses), the frames it dispatches, and the replies a client
//! reads back.
//!
//! The rule these cases assert, one level above the recorded cases (one api, one MOCK
//! integration, three integration responses, one stage):
//!
//! > A WEBSOCKET api announces, as its `ApiEndpoint`, the address of the socket it is served
//! > on, and that socket answers an RFC 6455 opening handshake FROM THE API'S OWN RECORDS:
//! > a stage the endpoint does not serve (no stage of that name, or no deployment released
//! > to it) is refused `403`; a stage whose `$connect` route's integration cannot reply
//! > successfully is refused `500`; any other deployed stage is answered `101 Switching
//! > Protocols`. Over the established socket each message is dispatched to the route the
//! > api's `RouteSelectionExpression` selects (or `$default`), that route's integration
//! > reply is selected among its `IntegrationResponses` by the status the call produced, and
//! > a TWO-WAY route's selected response renders the payload the client reads back. The
//! > connection the handshake minted is the connection the `@connections` management API
//! > addresses: a `PostToConnection` to it is DELIVERED over that socket, and once the
//! > client closes, each of the three `@connections` operations answers `GoneException`.
//!
//! The rows span what the rule generalises over: the stage's state (deployed, not deployed,
//! absent), the `$connect` route's integration (absent, a MOCK that selects a `2xx`, a MOCK
//! that selects nothing, one fronting a backend), the selecting status (`200`, `404`, a
//! status no pattern matches, none at all), the integration-response key forms (`/2\d\d/`,
//! `/4\d{2}/`, `$default`), whether the serving route is two-way, and the message's own
//! shape (a selected action, an unselected action, no action member, text that is not JSON).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use crate::api_lifecycle::{as_str, call, emulator};

/// How long a case waits for one frame before failing: generous for a loaded test machine,
/// and bounded so a case that will never be answered fails instead of hanging the suite.
const FRAME_DEADLINE: Duration = Duration::from_secs(10);

/// The status a completed handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// The statuses a refused handshake carries: the `403` an `execute-api` endpoint answers for
/// a stage it does not serve, and the `500` a gateway answers for a connection request its
/// api's configuration cannot be served from.
const FORBIDDEN: u16 = 403;
const INTERNAL_SERVER_ERROR: u16 = 500;

/// A WebSocket api whose messages select a route by their `action` member.
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// One api, created with the socket its `ApiEndpoint` announces.
pub(crate) struct Api {
    pub(crate) api_id: String,
    /// The `host:port` of the api's own socket, read off the announced `ApiEndpoint` rather
    /// than spelled out here, so a case dials the address the service reported.
    pub(crate) authority: String,
}

/// Create a WEBSOCKET api and read the socket address it announces.
pub(crate) async fn create_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> Api {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(websocket_api(name))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {created}"
    );
    let announced = as_str(&created, "apiEndpoint");
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a WEBSOCKET api must announce a wss:// socket, got {announced:?}")
        })
        .to_string();
    Api {
        api_id: as_str(&created, "apiId"),
        authority,
    }
}

/// Create one integration of the api and answer its `IntegrationId`.
pub(crate) async fn create_integration(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    body: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {created}"
    );
    as_str(&created, "integrationId")
}

/// Create one integration response of an integration.
pub(crate) async fn create_integration_response(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    integration_id: &str,
    body: Value,
) {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegrationResponse must answer 201, got {created}"
    );
}

/// Create one route over `integration_id`, and a route response for it when `two_way` is
/// set — "to define a two-way route, create a route response".
pub(crate) async fn create_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    integration_id: &str,
    two_way: bool,
) {
    let (status, created) = call(
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
        "case {case:?}: CreateRoute must answer 201, got {created}"
    );
    if two_way {
        let route_id = as_str(&created, "routeId");
        let (status, response) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
            Some(json!({ "routeResponseKey": "$default" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRouteResponse must answer 201, got {response}"
        );
    }
}

/// Create a stage, and release a deployment to it when `deploy` is set.
pub(crate) async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage: &str,
    deploy: bool,
) {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {created}"
    );
    if deploy {
        let (status, deployment) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateDeployment must answer 201, got {deployment}"
        );
    }
}

/// The socket a client gets from a completed handshake.
pub(crate) type ClientSocket = tokio_tungstenite::WebSocketStream<TcpStream>;

/// The loopback host the core binds a resource listener on by default, and so the only
/// address these cases ever dial: a case connects to the emulator's own listener on this
/// machine and nowhere else.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The port of the api's own announced socket, having checked the address is
/// [`LOOPBACK_HOST`] — so the dial below names that literal and reaches nothing else.
fn loopback_port(case: &str, authority: &str) -> u16 {
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a host:port, got {authority:?}"));
    assert_eq!(
        host, LOOPBACK_HOST,
        "case {case:?}: a case dials only the emulator's own loopback listener, got \
         {authority:?}"
    );
    port.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"))
}

/// Dial the api's own announced socket on the loopback address above.
pub(crate) async fn dial_loopback(case: &str, authority: &str) -> std::io::Result<TcpStream> {
    let port = loopback_port(case, authority);
    TcpStream::connect(("127.0.0.1", port)).await
}

/// Open an RFC 6455 handshake against `authority`'s `/{stage}` path, as a client library
/// does: the status a refusal carries, or the established socket.
async fn handshake(
    case: &str,
    authority: &str,
    stage: &str,
) -> Result<ClientSocket, u16> {
    handshake_at(case, authority, &format!("/{stage}")).await
}

/// [`handshake`] against any `path` of the announced socket — a stage, and whatever query
/// string the client carries, which is what an identity source and a `$connect` event read.
pub(crate) async fn handshake_at(
    case: &str,
    authority: &str,
    path: &str,
) -> Result<ClientSocket, u16> {
    let stream = dial_loopback(case, authority).await.unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the announced socket {authority:?} must accept a connection, \
             got {error}"
        )
    });
    let url = format!("ws://{authority}{path}");
    match tokio_tungstenite::client_async(&url, stream).await {
        Ok((socket, response)) => {
            assert_eq!(
                response.status().as_u16(),
                SWITCHING_PROTOCOLS,
                "case {case:?}: a completed handshake must answer 101"
            );
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("case {case:?}: the handshake failed before any answer: {error}"),
    }
}

/// Send one message and read the frame the api answers with, or `None` when it answers
/// nothing within [`FRAME_DEADLINE`].
pub(crate) async fn exchange(
    case: &str,
    socket: &mut ClientSocket,
    message: &str,
) -> Option<String> {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the message must be sent, got {error}"));
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(text.as_str().to_string()),
        Ok(Some(Ok(Message::Binary(bytes)))) => Some(String::from_utf8_lossy(&bytes).to_string()),
        Ok(Some(Ok(other))) => panic!("case {case:?}: unexpected frame {other:?}"),
        Ok(Some(Err(error))) => panic!("case {case:?}: the socket failed: {error}"),
        Ok(None) => panic!("case {case:?}: the socket closed instead of answering"),
        Err(_elapsed) => None,
    }
}

/// The handshake's own answer, over the api's announced socket, for every state the stage
/// and the connection request's route can be in.
#[tokio::test]
async fn the_announced_socket_answers_the_handshake_from_the_apis_records() {
    /// One row: what the api holds, and the status the handshake must be answered with.
    struct Row {
        case: &'static str,
        /// The `$connect` route's integration, when the api has one.
        connect_integration: Option<Value>,
        /// The integration responses that integration holds.
        connect_responses: Vec<Value>,
        deploy: bool,
        /// The stage the handshake names, which is not always the stage that was created.
        handshake_stage: &'static str,
        expected: Result<(), u16>,
    }

    let stage = "prod";
    let rows = vec![
        Row {
            case: "a deployed stage with no $connect route completes the handshake",
            connect_integration: None,
            connect_responses: Vec::new(),
            deploy: true,
            handshake_stage: stage,
            expected: Ok(()),
        },
        Row {
            // The recorded case: a MOCK integration on $connect with no integration
            // response to select, so the connection request has no reply.
            case: "a $connect route bound to a MOCK integration with no integration \
                   response fails the handshake",
            connect_integration: Some(json!({ "integrationType": "MOCK" })),
            connect_responses: Vec::new(),
            deploy: true,
            handshake_stage: stage,
            expected: Err(INTERNAL_SERVER_ERROR),
        },
        Row {
            case: "a $connect route whose MOCK integration selects a 2xx response still \
                   fails the handshake: a MOCK gives no connection decision",
            connect_integration: Some(json!({
                "integrationType": "MOCK",
                "requestTemplates": { "$default": "{\"statusCode\": 200}" }
            })),
            connect_responses: vec![json!({ "integrationResponseKey": "/2\\d\\d/" })],
            deploy: true,
            handshake_stage: stage,
            expected: Err(INTERNAL_SERVER_ERROR),
        },
        Row {
            case: "a $connect route whose MOCK integration selects a 4xx response fails \
                   the handshake",
            connect_integration: Some(json!({
                "integrationType": "MOCK",
                "requestTemplates": { "$default": "{\"statusCode\": 404}" }
            })),
            connect_responses: vec![json!({ "integrationResponseKey": "/4\\d{2}/" })],
            deploy: true,
            handshake_stage: stage,
            expected: Err(INTERNAL_SERVER_ERROR),
        },
        Row {
            // A $connect route whose integration fronts a FUNCTION this emulator does not
            // hold: the backend is the one that would admit or refuse the client, and an
            // invocation that reaches no function gives the connection request no decision,
            // so the handshake is refused rather than admitted on a decision nothing made.
            // (A $connect route whose function DOES answer is driven in
            // `websocket_lambda_routes.rs`, where AWS Lambda is mounted beside this
            // service.)
            case: "a $connect route fronting a function this emulator cannot reach fails \
                   the handshake",
            connect_integration: Some(json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:connect",
                "integrationMethod": "POST"
            })),
            connect_responses: Vec::new(),
            deploy: true,
            handshake_stage: stage,
            expected: Err(INTERNAL_SERVER_ERROR),
        },
        Row {
            case: "a stage with no deployment released to it serves nothing",
            connect_integration: None,
            connect_responses: Vec::new(),
            deploy: false,
            handshake_stage: stage,
            expected: Err(FORBIDDEN),
        },
        Row {
            case: "a stage name the api does not hold serves nothing",
            connect_integration: None,
            connect_responses: Vec::new(),
            deploy: true,
            handshake_stage: "no-such-stage",
            expected: Err(FORBIDDEN),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = emulator();
        let api = create_api(&emulator, case, "handshake-api").await;

        // Every api carries a $default route over a MOCK integration, so the only thing
        // that differs between the rows is the connection request's own route.
        let default_integration = create_integration(
            &emulator,
            case,
            &api.api_id,
            json!({ "integrationType": "MOCK" }),
        )
        .await;
        create_route(
            &emulator,
            case,
            &api.api_id,
            "$default",
            &default_integration,
            false,
        )
        .await;

        if let Some(integration) = row.connect_integration {
            let connect_integration =
                create_integration(&emulator, case, &api.api_id, integration).await;
            for response in row.connect_responses {
                create_integration_response(
                    &emulator,
                    case,
                    &api.api_id,
                    &connect_integration,
                    response,
                )
                .await;
            }
            create_route(
                &emulator,
                case,
                &api.api_id,
                "$connect",
                &connect_integration,
                false,
            )
            .await;
        }

        create_stage(&emulator, case, &api.api_id, stage, row.deploy).await;

        let answered = handshake(case, &api.authority, row.handshake_stage).await;
        match (&row.expected, &answered) {
            (Ok(()), Ok(_socket)) => {}
            (Err(expected), Err(status)) => assert_eq!(
                status, expected,
                "case {case:?}: expected the handshake refused {expected}, got {status}"
            ),
            (Ok(()), Err(status)) => panic!(
                "case {case:?}: expected the handshake completed with {SWITCHING_PROTOCOLS}, \
                 got {status}"
            ),
            (Err(expected), Ok(_socket)) => panic!(
                "case {case:?}: expected the handshake refused {expected}, got \
                 {SWITCHING_PROTOCOLS}"
            ),
        }
    }
}

/// Over an established socket, each message is dispatched to the route the api's selection
/// expression selects and answered by what that route's selected integration response
/// renders.
#[tokio::test]
async fn a_message_is_answered_by_the_integration_response_its_status_selects() {
    /// One row: the message sent, and the reply the client must read back (`None` for a
    /// message the api answers nothing to).
    struct Row {
        case: &'static str,
        message: &'static str,
        expected: Option<&'static str>,
    }

    let case = "three integration responses selected by the synthesized status";
    let emulator = emulator();
    let api = create_api(&emulator, case, "dispatch-api").await;

    // The $default route's MOCK integration synthesizes `200`, and holds three
    // integration responses whose keys span the forms a key takes.
    let defaulted = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    for response in [
        json!({
            "integrationResponseKey": "/2\\d\\d/",
            "responseTemplates": { "$default": "{\"picked\":\"2xx\"}" }
        }),
        json!({
            "integrationResponseKey": "/4\\d{2}/",
            "responseTemplates": { "$default": "{\"picked\":\"4xx\"}" }
        }),
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": "{\"picked\":\"default\"}" }
        }),
    ] {
        create_integration_response(&emulator, case, &api.api_id, &defaulted, response).await;
    }
    create_route(&emulator, case, &api.api_id, "$default", &defaulted, true).await;

    // A second, NAMED route whose MOCK integration synthesizes `404`, so the selecting
    // status — and with it the selected response — differs per route.
    let notfound = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 404}" }
        }),
    )
    .await;
    for response in [
        json!({
            "integrationResponseKey": "/2\\d\\d/",
            "responseTemplates": { "$default": "{\"picked\":\"2xx\"}" }
        }),
        json!({
            "integrationResponseKey": "/4\\d{2}/",
            "responseTemplates": { "$default": "{\"picked\":\"4xx\"}" }
        }),
    ] {
        create_integration_response(&emulator, case, &api.api_id, &notfound, response).await;
    }
    create_route(&emulator, case, &api.api_id, "missing", &notfound, true).await;

    // A ONE-WAY route: it holds no route response, so nothing is sent back for it however
    // its integration replied.
    let one_way = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    create_integration_response(
        &emulator,
        case,
        &api.api_id,
        &one_way,
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": "{\"picked\":\"one-way\"}" }
        }),
    )
    .await;
    create_route(&emulator, case, &api.api_id, "oneway", &one_way, false).await;

    create_stage(&emulator, case, &api.api_id, "prod", true).await;
    let mut socket = handshake(case, &api.authority, "prod")
        .await
        .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));

    let rows = vec![
        Row {
            case: "the selected route's 2xx response renders the reply",
            message: r#"{"action":"$default"}"#,
            expected: Some(r#"{"picked":"2xx"}"#),
        },
        Row {
            case: "a route whose integration synthesizes 404 selects the 4xx response",
            message: r#"{"action":"missing"}"#,
            expected: Some(r#"{"picked":"4xx"}"#),
        },
        Row {
            case: "an action no route key matches is served by $default",
            message: r#"{"action":"not-a-route"}"#,
            expected: Some(r#"{"picked":"2xx"}"#),
        },
        Row {
            case: "a message carrying no action member is served by $default",
            message: r#"{"hello":"world"}"#,
            expected: Some(r#"{"picked":"2xx"}"#),
        },
        Row {
            case: "text that is not JSON selects nothing and is served by $default",
            message: "not json at all",
            expected: Some(r#"{"picked":"2xx"}"#),
        },
        Row {
            case: "a one-way route sends nothing back",
            message: r#"{"action":"oneway"}"#,
            expected: None,
        },
    ];

    for row in rows {
        let answered = exchange(row.case, &mut socket, row.message).await;
        assert_eq!(
            answered.as_deref(),
            row.expected,
            "case {:?}: expected the reply {:?}, got {answered:?}",
            row.case,
            row.expected
        );
    }
}

/// A message the api routes nowhere — no route key is selected and the api has no `$default`
/// route — is answered with the gateway's own `Forbidden` frame, naming the connection and
/// the call.
#[tokio::test]
async fn a_message_no_route_serves_is_answered_with_the_gateways_forbidden_frame() {
    let case = "an api with one named route and no $default route";
    let emulator = emulator();
    let api = create_api(&emulator, case, "unrouted-api").await;
    let integration = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    create_integration_response(
        &emulator,
        case,
        &api.api_id,
        &integration,
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": "{\"picked\":\"named\"}" }
        }),
    )
    .await;
    create_route(&emulator, case, &api.api_id, "named", &integration, true).await;
    create_stage(&emulator, case, &api.api_id, "prod", true).await;

    let mut socket = handshake(case, &api.authority, "prod")
        .await
        .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));

    // The rows span what the rule generalises over: an action the api holds a route for,
    // and one it does not.
    for (label, message, expected) in [
        ("a selected route still serves", r#"{"action":"named"}"#, None),
        (
            "an unselected action with no $default route",
            r#"{"action":"nowhere"}"#,
            Some("Forbidden"),
        ),
    ] {
        let answered = exchange(label, &mut socket, message).await;
        match expected {
            None => assert_eq!(
                answered.as_deref(),
                Some(r#"{"picked":"named"}"#),
                "case {label:?}: expected the route's reply, got {answered:?}"
            ),
            Some(reason) => {
                let frame: Value = serde_json::from_str(
                    answered
                        .as_deref()
                        .unwrap_or_else(|| panic!("case {label:?}: expected a frame, got nothing")),
                )
                .unwrap_or_else(|error| {
                    panic!("case {label:?}: expected a JSON frame, got {answered:?} ({error})")
                });
                assert_eq!(
                    frame["message"].as_str(),
                    Some(reason),
                    "case {label:?}: expected the gateway's {reason:?} frame, got {frame}"
                );
                assert!(
                    frame["connectionId"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {label:?}: the frame must name the connection, got {frame}"
                );
                assert!(
                    frame["requestId"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {label:?}: the frame must name the call, got {frame}"
                );
            }
        }
    }
}

/// Deleting the api closes its socket: the address it announced accepts no connection, which
/// is what makes a deleted api stop answering.
#[tokio::test]
async fn deleting_the_api_closes_the_socket_it_announced() {
    let case = "the api is deleted while its socket is announced";
    let emulator = emulator();
    let api = create_api(&emulator, case, "closing-api").await;
    let integration = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    create_route(&emulator, case, &api.api_id, "$default", &integration, false).await;
    create_stage(&emulator, case, &api.api_id, "prod", true).await;

    // While the api exists its socket completes the handshake.
    let socket = handshake(case, &api.authority, "prod").await;
    assert!(
        socket.is_ok(),
        "case {case:?}: the announced socket must complete the handshake while the api exists"
    );
    drop(socket);

    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{}", api.api_id),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: DeleteApi must answer 204, got {status} {deleted}"
    );

    // The port is closed, so a client dialling the announced address is refused. A port the
    // operating system has not yet released may still accept the connection, in which case
    // the handshake itself must not be completed. The address is the loopback one the api
    // announced and this case already dialled above.
    match dial_loopback(case, &api.authority).await {
        Err(_refused) => {}
        Ok(stream) => {
            let url = format!("ws://{}/prod", api.authority);
            let answered = tokio_tungstenite::client_async(&url, stream).await;
            assert!(
                answered.is_err(),
                "case {case:?}: a deleted api's socket must not complete a handshake"
            );
        }
    }
}

/// The host the `@connections` management API is addressed at, and the signing name its
/// requests carry: AWS publishes that API on the api's own invoke endpoint, whose service is
/// `execute-api`.
const CONNECTIONS_HOST: &str = "execute-api.eu-west-1.amazonaws.com";

/// A router serving the api's control plane AND the `@connections` management API over ONE
/// engine, which is how a connection the api's socket carries is the connection that API
/// addresses.
fn emulator_with_connections() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One `@connections` call, signed for the data plane's own service: its status and body.
async fn connections_call(
    emulator: &MultiServiceRouter,
    method: &str,
    connection_id: &str,
    payload: Option<&str>,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{CONNECTIONS_HOST}/@connections/{connection_id}"))
        .header("host", CONNECTIONS_HOST)
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/eu-west-1/execute-api/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(Bytes::from(payload.unwrap_or_default().to_string()))
        .expect("the test request is well formed");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A connection the api's socket carries is the connection the `@connections` management API
/// addresses: a post to it is delivered over that socket, and once the client closes, each of
/// the three operations answers `GoneException`.
#[tokio::test]
async fn a_carried_connection_is_posted_to_and_then_gone_after_the_client_closes() {
    let case = "a live connection addressed through @connections";
    let emulator = emulator_with_connections();
    let api = create_api(&emulator, case, "connections-api").await;

    // The $default route is two-way and renders the CONNECTION's own id, which is how a
    // client learns the id AWS gives its handler in `$context.connectionId`.
    let integration = create_integration(
        &emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    create_integration_response(
        &emulator,
        case,
        &api.api_id,
        &integration,
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": "$context.connectionId" }
        }),
    )
    .await;
    create_route(&emulator, case, &api.api_id, "$default", &integration, true).await;
    create_stage(&emulator, case, &api.api_id, "prod", true).await;

    let mut socket = handshake(case, &api.authority, "prod")
        .await
        .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));
    let connection_id = exchange(case, &mut socket, r#"{"action":"whoami"}"#)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the route must reply with the connection id"));
    assert!(
        !connection_id.is_empty(),
        "case {case:?}: the connection must have an id, got {connection_id:?}"
    );

    // GetConnection reports the live connection.
    let (status, reported) = connections_call(&emulator, "GET", &connection_id, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnection must report the live connection, got {status} {reported}"
    );

    // A post is DELIVERED over the socket: the client reads exactly the payload sent.
    let payload = r#"{"hello":"socket"}"#;
    let (status, posted) =
        connections_call(&emulator, "POST", &connection_id, Some(payload)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PostToConnection must answer 200, got {status} {posted}"
    );
    let delivered = match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text.as_str().to_string(),
        Ok(Some(Ok(Message::Binary(bytes)))) => String::from_utf8_lossy(&bytes).to_string(),
        other => panic!("case {case:?}: the posted payload must arrive, got {other:?}"),
    };
    assert_eq!(
        delivered, payload,
        "case {case:?}: expected the posted payload {payload:?}, got {delivered:?}"
    );

    // The client closes. From then on the connection no longer exists, so each of the three
    // operations answers GoneException (HTTP 410). The close travels over the socket, so the
    // answer is polled rather than assumed immediate.
    socket
        .close(None)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the client must close, got {error}"));
    drop(socket);

    let deadline = tokio::time::Instant::now() + FRAME_DEADLINE;
    loop {
        let (status, _body) = connections_call(&emulator, "GET", &connection_id, None).await;
        if status == GONE {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected {GONE} once the client closed, got {status}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // The rows span the three operations the management API models over one connection.
    for (method, payload) in [
        ("GET", None),
        ("POST", Some(r#"{"after":"close"}"#)),
        ("DELETE", None),
    ] {
        let (status, body) = connections_call(&emulator, method, &connection_id, payload).await;
        assert_eq!(
            status, GONE,
            "case {method:?}: expected {GONE} for a connection that has closed, got {status} \
             {body}"
        );
    }
}

/// The status `GoneException` carries: "The connection with the provided id no longer
/// exists".
const GONE: u16 = 410;
