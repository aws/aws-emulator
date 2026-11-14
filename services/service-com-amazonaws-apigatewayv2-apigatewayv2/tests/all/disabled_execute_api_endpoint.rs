// SPDX-License-Identifier: CC0-1.0

//! `Api$DisableExecuteApiEndpoint`: what the api's DEFAULT `execute-api` endpoint answers
//! while that member is set, and that it serves again as soon as it is cleared.
//!
//! The rule these cases assert, one level above the recorded case (one HTTP api with a
//! manually created stage plus its WEBSOCKET twin):
//!
//! > "To require that clients use a custom domain name to invoke your API, disable the
//! > default endpoint" (`Api$DisableExecuteApiEndpoint`) — so while an api has that member
//! > set, its default `execute-api` endpoint serves NOTHING of the api, and a request to it
//! > is answered exactly as that endpoint answers a request for an api it serves none of,
//! > whatever stage, deployment, route or integration the address would otherwise have
//! > resolved: `404` `{"message":"Not Found"}` for a request to an HTTP api, and `403`
//! > `{"message":"Forbidden"}` for an RFC 6455 opening handshake made at a WEBSOCKET api's,
//! > which is REFUSED rather than upgraded. Clearing the member makes the endpoint serve
//! > again. The member is a property of the endpoint rather than of the configuration a
//! > stage's deployment froze, so it takes effect on every stage of the api at once and needs
//! > no redeployment.
//!
//! The rows span what the rule generalises over: the api's `ProtocolType` (`HTTP` and
//! `WEBSOCKET`), how the stage was released (a stage pinned by its own `CreateDeployment`,
//! and an `AutoDeploy` stage), the stage name (a plain one, one at an unusual form, and the
//! `$default` stage that serves the api's root with no stage segment at all), the addressing
//! (the api's own `execute-api` host, the emulator's own endpoint with the api id in the
//! path, and the stage-only path), where the member's value came from (`CreateApi` or
//! `UpdateApi`), and both directions of the transition.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, REGION};
use crate::websocket_handshake_data_plane::{message, send, Answer, CLIENT_KEY};
use crate::websocket_transport_data_plane::handshake_at;

/// The status and sentence an `execute-api` endpoint answers a WEBSOCKET handshake it serves
/// no api or stage for — which is what an api that turned its own default endpoint off is on
/// that endpoint.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status and sentence an HTTP api's `execute-api` endpoint answers a request it serves
/// no api for — which is what an api that turned its own default endpoint off is on that
/// endpoint, so a client cannot tell the two apart.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status a completed opening handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// What an HTTP api's served route answers here: its `HTTP_PROXY` integration names "an
/// endpoint over the network" (`Integration$IntegrationUri`) this emulator opens no
/// connection to, so the caller is sent there with the redirect that preserves its method
/// and body (`http_api_jwt_data_plane`).
const TEMPORARY_REDIRECT: u16 = 307;

/// The upstream that integration names, so a served answer can be told from any refusal by
/// the address it carries.
const UPSTREAM: &str = "https://upstream.example.com/orders";

/// What a WEBSOCKET api's handshake is answered with at the SERVICE endpoint while the
/// endpoint serves: this emulator's front door carries no WebSocket transport, so it refuses
/// an otherwise admitted handshake there and names the socket the `ApiEndpoint` announces
/// (`websocket_handshake_data_plane`). The api's own socket is driven in the second case
/// below, where a served handshake really is a `101`.
const NOT_IMPLEMENTED: u16 = 501;

/// The error code the front door answers when NO service claims a request: asserted absent,
/// because an api that turned its default endpoint off is still the api that endpoint
/// addresses and must be REFUSED by this service rather than left unclaimed.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// One api, created with its route, its stage and (unless the stage auto-deploys) a
/// deployment released to that stage.
struct Served {
    api_id: String,
    /// The host of the api's own default endpoint, `{api-id}.execute-api.{region}
    /// .amazonaws.com`.
    endpoint_host: String,
    stage: String,
}

/// The `CreateApi` body of one row's api: a WEBSOCKET api needs a
/// `RouteSelectionExpression` ("required for WebSocket APIs",
/// `CreateApi$RouteSelectionExpression`), and both carry the member under test.
fn api_body(protocol_type: &str, disabled: bool) -> Value {
    let mut body = json!({
        "name": "endpoint-gate",
        "protocolType": protocol_type,
        "disableExecuteApiEndpoint": disabled
    });
    if protocol_type == "WEBSOCKET" {
        body["routeSelectionExpression"] = json!("$request.body.action");
    }
    body
}

/// The integration a row's route fronts: `MOCK` "is not permitted for APIs with a protocol
/// type of HTTP" (`Integration$IntegrationType`), so an HTTP api gets the `HTTP_PROXY`
/// upstream its protocol allows and a WEBSOCKET api the `MOCK` its own cases use.
fn integration_body(protocol_type: &str) -> Value {
    if protocol_type == "HTTP" {
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": UPSTREAM,
            "integrationMethod": "ANY",
            "payloadFormatVersion": "1.0"
        })
    } else {
        json!({ "integrationType": "MOCK" })
    }
}

/// The route key a row's route carries: the HTTP api's own path, and the WEBSOCKET api's
/// catch-all. No `$connect` route, so the connection request itself has nothing to decide —
/// what these cases are about is the ENDPOINT, not the connection's own backend.
fn route_key(protocol_type: &str) -> &'static str {
    if protocol_type == "HTTP" {
        "ANY /orders"
    } else {
        "$default"
    }
}

/// Create an api of `protocol_type`, a route over an integration its protocol allows, and
/// `stage` — released by its own `CreateDeployment` unless the stage auto-deploys.
async fn serve_api(
    emulator: &MultiServiceRouter,
    case: &str,
    protocol_type: &str,
    stage: &str,
    auto_deploy: bool,
    disabled: bool,
) -> Served {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(api_body(protocol_type, disabled)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {created}"
    );
    let api_id = as_str(&created, "apiId");
    assert_eq!(
        created.get("disableExecuteApiEndpoint").and_then(Value::as_bool),
        Some(disabled),
        "case {case:?}: CreateApi must report the member it was given, got {created}"
    );

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(integration_body(protocol_type)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let (status, route) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": route_key(protocol_type),
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {route}"
    );

    let (status, created_stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage, "autoDeploy": auto_deploy })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {created_stage}"
    );
    if !auto_deploy {
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
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetStage must answer 200, got {fetched}"
    );
    assert!(
        !as_str(&fetched, "deploymentId").is_empty(),
        "case {case:?}: the stage must carry a released deployment, got {fetched}"
    );

    Served {
        endpoint_host: format!("{api_id}.execute-api.{REGION}.amazonaws.com"),
        api_id,
        stage: stage.to_string(),
    }
}

/// Set `DisableExecuteApiEndpoint` on the api and read the member back, so each step of a
/// case starts from a value the control plane reports.
async fn set_disabled(emulator: &MultiServiceRouter, case: &str, api_id: &str, disabled: bool) {
    let (status, updated) = call(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}"),
        Some(json!({ "disableExecuteApiEndpoint": disabled })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: UpdateApi must answer 201, got {updated}"
    );
    assert_eq!(
        updated.get("disableExecuteApiEndpoint").and_then(Value::as_bool),
        Some(disabled),
        "case {case:?}: UpdateApi must report the member it was given, got {updated}"
    );
}

/// One row of the endpoint table: the api it stands up, and the address a client reaches it
/// at.
struct Row {
    case: &'static str,
    protocol_type: &'static str,
    stage: &'static str,
    /// Whether the stage auto-deploys, instead of being pinned by its own deployment.
    auto_deploy: bool,
    /// Whether the api turns its default endpoint off at CREATE time; the case drives the
    /// member through both states from there either way.
    disabled_at_create: bool,
    /// The host the client addresses.
    host: fn(&Served) -> String,
    /// The path it addresses within that host.
    path: fn(&Served) -> String,
}

/// The emulator's own endpoint, which is where a client that resolves no AWS host points.
const EMULATOR_ENDPOINT: &str = "127.0.0.1:9297";

const ROWS: &[Row] = &[
    Row {
        case: "an HTTP api's manually created stage, on the api's own execute-api host",
        protocol_type: "HTTP",
        stage: "prod",
        auto_deploy: false,
        disabled_at_create: false,
        host: |served| served.endpoint_host.clone(),
        path: |served| format!("/{}/orders", served.stage),
    },
    Row {
        case: "an HTTP api's $default AutoDeploy stage, reached with no stage segment",
        protocol_type: "HTTP",
        stage: "$default",
        auto_deploy: true,
        disabled_at_create: false,
        host: |served| served.endpoint_host.clone(),
        path: |_| "/orders".to_string(),
    },
    Row {
        case: "an HTTP api addressed on the emulator's own endpoint, the api id in the path",
        protocol_type: "HTTP",
        stage: "beta",
        auto_deploy: false,
        disabled_at_create: true,
        host: |_| EMULATOR_ENDPOINT.to_string(),
        path: |served| {
            format!(
                "/_aws/execute-api/{}/{}/orders",
                served.api_id, served.stage
            )
        },
    },
    Row {
        case: "an HTTP api addressed by its stage path alone, at a stage name of an unusual \
               form",
        protocol_type: "HTTP",
        stage: "Stage_1-x",
        auto_deploy: false,
        disabled_at_create: false,
        host: |_| EMULATOR_ENDPOINT.to_string(),
        path: |served| format!("/{}/orders", served.stage),
    },
    Row {
        case: "a WEBSOCKET api's manually created stage, on the api's own execute-api host",
        protocol_type: "WEBSOCKET",
        stage: "sockets",
        auto_deploy: false,
        disabled_at_create: false,
        host: |served| served.endpoint_host.clone(),
        path: |served| format!("/{}", served.stage),
    },
    Row {
        case: "a WEBSOCKET api addressed on the emulator's own endpoint, the api id in the \
               path",
        protocol_type: "WEBSOCKET",
        stage: "ws-local",
        auto_deploy: true,
        disabled_at_create: true,
        host: |_| EMULATOR_ENDPOINT.to_string(),
        path: |served| format!("/_aws/execute-api/{}/{}", served.api_id, served.stage),
    },
];

/// Send what a client of this row's protocol sends: an ordinary request to a route of an
/// HTTP api, and an RFC 6455 opening handshake to a WEBSOCKET api's stage.
async fn request(emulator: &MultiServiceRouter, row: &Row, host: &str, path: &str) -> Answer {
    if row.protocol_type == "HTTP" {
        return send(emulator, host, "GET", path, &[]).await;
    }
    send(
        emulator,
        host,
        "GET",
        path,
        &[
            ("connection", "Upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-key", CLIENT_KEY),
            ("sec-websocket-version", "13"),
        ],
    )
    .await
}

/// Assert the endpoint SERVED this request: the route's own answer for an HTTP api, and the
/// one answer the front door owes an admitted handshake for a WEBSOCKET api — neither of
/// which is the refusal a disabled endpoint gives.
fn assert_served(case: &str, protocol_type: &str, answer: &Answer) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the api's own endpoint must be served by this service, got {}",
        answer.summary()
    );
    if protocol_type == "HTTP" {
        assert_eq!(
            answer.status, TEMPORARY_REDIRECT,
            "case {case:?}: expected the route's upstream redirect {TEMPORARY_REDIRECT}, got {}",
            answer.summary()
        );
        assert_eq!(
            answer.location.as_deref(),
            Some(UPSTREAM),
            "case {case:?}: expected the upstream {UPSTREAM:?} in Location, got {:?}",
            answer.location
        );
        return;
    }
    assert_eq!(
        answer.status, NOT_IMPLEMENTED,
        "case {case:?}: expected {NOT_IMPLEMENTED} for a handshake this endpoint cannot \
         upgrade, got {}",
        answer.summary()
    );
}

/// The refusal a disabled default endpoint owes a request of this protocol: the answer that
/// endpoint gives for an api it serves none of, which for an HTTP api is `404`
/// `{"message":"Not Found"}` and for a WEBSOCKET handshake `403` `{"message":"Forbidden"}`.
fn refusal_of(protocol_type: &str) -> (u16, &'static str) {
    if protocol_type == "HTTP" {
        (NOT_FOUND, NOT_FOUND_MESSAGE)
    } else {
        (FORBIDDEN, FORBIDDEN_MESSAGE)
    }
}

/// Assert the endpoint REFUSED this request as an endpoint that serves nothing, with the
/// answer its protocol's plane gives for an api it does not serve — from this service and not
/// from the front door.
fn assert_refused(case: &str, protocol_type: &str, answer: &Answer) {
    let (status, sentence) = refusal_of(protocol_type);
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: a disabled endpoint must be refused by this service, not left \
         unclaimed, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected {status} on a disabled default endpoint, got {}",
        answer.summary()
    );
    assert_eq!(
        message(answer),
        sentence,
        "case {case:?}: expected the {sentence:?} body, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.accept, None,
        "case {case:?}: a refused handshake carries no Sec-WebSocket-Accept, got {:?}",
        answer.accept
    );
}

#[tokio::test]
async fn a_disabled_default_endpoint_serves_nothing_and_serves_again_when_it_is_cleared() {
    for row in ROWS {
        let emulator = emulator();
        let served = serve_api(
            &emulator,
            row.case,
            row.protocol_type,
            row.stage,
            row.auto_deploy,
            row.disabled_at_create,
        )
        .await;
        let host = (row.host)(&served);
        let path = (row.path)(&served);

        // The member is driven through both states and back, starting from the value the
        // api was CREATED with: what the endpoint answers follows the member at every step,
        // in both directions, with no redeployment of the stage in between.
        for (step, disabled) in [
            row.disabled_at_create,
            !row.disabled_at_create,
            row.disabled_at_create,
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!(
                "{} [step {step}, DisableExecuteApiEndpoint {disabled}]",
                row.case
            );
            if step > 0 {
                set_disabled(&emulator, &case, &served.api_id, disabled).await;
            }
            let answer = request(&emulator, row, &host, &path).await;
            if disabled {
                assert_refused(&case, row.protocol_type, &answer);
            } else {
                assert_served(&case, row.protocol_type, &answer);
            }
        }
    }
}

#[tokio::test]
async fn a_disabled_default_endpoint_refuses_the_handshake_on_the_apis_own_socket() {
    /// One row of the socket table: the stage the handshake names, and where the member's
    /// first value came from.
    struct SocketRow {
        case: &'static str,
        stage: &'static str,
        auto_deploy: bool,
        disabled_at_create: bool,
    }

    const SOCKET_ROWS: &[SocketRow] = &[
        SocketRow {
            case: "a manually created stage, the member set by UpdateApi",
            stage: "prod",
            auto_deploy: false,
            disabled_at_create: false,
        },
        SocketRow {
            case: "an AutoDeploy stage at a stage name of an unusual form, the member set \
                   by CreateApi",
            stage: "Stage_1-x",
            auto_deploy: true,
            disabled_at_create: true,
        },
    ];

    for row in SOCKET_ROWS {
        let emulator = emulator();
        let served = serve_api(
            &emulator,
            row.case,
            "WEBSOCKET",
            row.stage,
            row.auto_deploy,
            row.disabled_at_create,
        )
        .await;
        // The address the api ANNOUNCES for its socket, read back off the control plane so
        // the case dials what a client would be handed.
        let (status, api) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{}", served.api_id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetApi must answer 200, got {api}",
            row.case
        );
        let announced = as_str(&api, "apiEndpoint");
        let authority = announced
            .strip_prefix("wss://")
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a WEBSOCKET api must announce a wss:// socket, got \
                     {announced:?}",
                    row.case
                )
            })
            .to_string();

        for (step, disabled) in [
            row.disabled_at_create,
            !row.disabled_at_create,
            row.disabled_at_create,
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!(
                "{} [step {step}, DisableExecuteApiEndpoint {disabled}]",
                row.case
            );
            if step > 0 {
                set_disabled(&emulator, &case, &served.api_id, disabled).await;
            }
            let answered = handshake_at(&case, &authority, &format!("/{}", served.stage)).await;
            match (disabled, answered) {
                (true, Err(status)) => assert_eq!(
                    status, FORBIDDEN,
                    "case {case:?}: expected the handshake refused {FORBIDDEN} on a disabled \
                     default endpoint, got {status}"
                ),
                (true, Ok(_socket)) => panic!(
                    "case {case:?}: expected the handshake refused {FORBIDDEN}, got \
                     {SWITCHING_PROTOCOLS}"
                ),
                (false, Ok(_socket)) => {}
                (false, Err(status)) => panic!(
                    "case {case:?}: expected the handshake completed with \
                     {SWITCHING_PROTOCOLS}, got {status}"
                ),
            }
        }
    }
}

/// An api id this emulator holds no api for, so its `execute-api` endpoint serves nothing —
/// the answer a disabled default endpoint has to be indistinguishable from. Shaped like an
/// api id (`Id` is an unconstrained string in this model) and never minted by a case here,
/// which creates its apis through `CreateApi` and reads their ids back.
const UNHELD_API_ID: &str = "zz9999zz99";

/// One row of the twin table: how a client addresses an api, and the stage it names.
struct TwinRow {
    case: &'static str,
    stage: &'static str,
    auto_deploy: bool,
    /// The host the client addresses, built from the api's own endpoint host.
    host: fn(&Served) -> String,
    /// The path that reaches the api's route at that host.
    path: fn(&Served) -> String,
    /// A path within the same api that no route of it matches, so the refusal can be shown
    /// to be answered before a route is looked for at all.
    unmatched_path: fn(&Served) -> String,
}

/// The addressings an api id travels in, which is what this table generalises over: the host
/// `Api$ApiEndpoint` documents, and the emulator's own rewriting of it with the id in the
/// path. The stage-only addressing is left out on purpose: it carries no api id, so there is
/// no address of an api nothing holds to compare against.
const TWIN_ROWS: &[TwinRow] = &[
    TwinRow {
        case: "the api's own execute-api host, a manually created stage",
        stage: "prod",
        auto_deploy: false,
        host: |served| served.endpoint_host.clone(),
        path: |served| format!("/{}/orders", served.stage),
        unmatched_path: |served| format!("/{}/no-such-route", served.stage),
    },
    TwinRow {
        case: "the api's own execute-api host, the $default AutoDeploy stage reached with no \
               stage segment",
        stage: "$default",
        auto_deploy: true,
        host: |served| served.endpoint_host.clone(),
        path: |_| "/orders".to_string(),
        unmatched_path: |_| "/no-such-route".to_string(),
    },
    TwinRow {
        case: "the emulator's own endpoint, the api id behind the endpoint marker",
        stage: "beta",
        auto_deploy: false,
        host: |_| EMULATOR_ENDPOINT.to_string(),
        path: |served| {
            format!(
                "/_aws/execute-api/{}/{}/orders",
                served.api_id, served.stage
            )
        },
        unmatched_path: |served| {
            format!(
                "/_aws/execute-api/{}/{}/no-such-route",
                served.api_id, served.stage
            )
        },
    },
];

/// The address of an api id this emulator holds no api for, built by the SAME row functions
/// that address a created api, so the two are compared at one addressing.
fn unheld(stage: &str) -> Served {
    Served {
        endpoint_host: format!("{UNHELD_API_ID}.execute-api.{REGION}.amazonaws.com"),
        api_id: UNHELD_API_ID.to_string(),
        stage: stage.to_string(),
    }
}

#[tokio::test]
async fn a_disabled_default_endpoint_answers_as_an_endpoint_that_serves_no_api() {
    for row in TWIN_ROWS {
        let emulator = emulator();
        // Two apis differing ONLY in the member, which is the shape a stack deploying a
        // pair of them produces: the enabled one serves its route, the disabled one serves
        // nothing on the endpoint it was announced at.
        let disabled = serve_api(
            &emulator,
            &format!("{} [disabled twin]", row.case),
            "HTTP",
            row.stage,
            row.auto_deploy,
            true,
        )
        .await;
        let enabled = serve_api(
            &emulator,
            &format!("{} [enabled twin]", row.case),
            "HTTP",
            row.stage,
            row.auto_deploy,
            false,
        )
        .await;

        let case = format!("{} [enabled twin]", row.case);
        let served = send(
            &emulator,
            &(row.host)(&enabled),
            "GET",
            &(row.path)(&enabled),
            &[],
        )
        .await;
        assert_served(&case, "HTTP", &served);

        let case = format!("{} [disabled twin]", row.case);
        let refused = send(
            &emulator,
            &(row.host)(&disabled),
            "GET",
            &(row.path)(&disabled),
            &[],
        )
        .await;
        assert_refused(&case, "HTTP", &refused);

        // The whole point of the member: a client probing the default endpoint cannot tell
        // the disabled api from an api id the endpoint space serves nothing for, so the two
        // answers are compared status and body alike.
        let nothing = unheld(row.stage);
        let case = format!("{} [an api id nothing serves]", row.case);
        let unserved = send(
            &emulator,
            &(row.host)(&nothing),
            "GET",
            &(row.path)(&nothing),
            &[],
        )
        .await;
        assert_refused(&case, "HTTP", &unserved);
        assert_eq!(
            (refused.status, refused.text.as_str()),
            (unserved.status, unserved.text.as_str()),
            "case {case:?}: a disabled default endpoint must answer exactly as an endpoint \
             that serves no api, got {} against {}",
            refused.summary(),
            unserved.summary()
        );

        // The refusal precedes the route: a path no route of the disabled api matches is
        // answered the same way, so nothing of the api's configuration is observable through
        // the endpoint it turned off.
        let case = format!("{} [disabled twin, a path no route matches]", row.case);
        let unmatched = send(
            &emulator,
            &(row.host)(&disabled),
            "GET",
            &(row.unmatched_path)(&disabled),
            &[],
        )
        .await;
        assert_refused(&case, "HTTP", &unmatched);
    }
}
