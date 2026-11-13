// SPDX-License-Identifier: CC0-1.0

//! The `@connections` ERROR LAYER on the wire: what the body of a failure carries, and the
//! refusal an endpoint that serves no deployed stage earns — driven over HTTP through the
//! same `MultiServiceRouter` the production emulator serves with.
//!
//! Two rules, each asserted over a table.
//!
//! > **The body of a restJson1 failure carries the error shape's own modelled members and
//! > nothing else**, with the shape named in `x-amzn-errortype`: `GoneException` and
//! > `ForbiddenException` declare NO members
//! > (`smithy_query(query="shape", shape="GoneException,ForbiddenException")`), so a failure
//! > of one carries no member at all, and no body carries the AWS-JSON-family `__type`
//! > discriminator, which a client generated from this model cannot read.
//!
//! > **A `@connections` request is served only on an endpoint that resolves to a DEPLOYED
//! > stage**: these operations are reached through the owning api's own invoke endpoint
//! > (`…/{stage}/@connections/{id}`, per `Api$ApiEndpoint`), so an addressing whose stage no
//! > api has deployed is refused with the modelled `ForbiddenException` (HTTP 403) and the
//! > sentence API Gateway words that refusal in, BEFORE the `ConnectionId` is judged and
//! > before the connection store is consulted.
//!
//! WHO decides the second one: the apis and stages are API Gateway V2's records, which this
//! service cannot read, so that service installs its own reading into the shared cell this
//! service asks (`mgmt::state::connections_endpoint_authority`). These cases install a
//! reading of their own — the cell, the ORDER and the envelope are what this suite owns —
//! while the resolution against real apis, stages and deployments is driven end to end in
//! `core/router/tests/all/apigatewayv2_websocket_deploy_then_connections.rs`.
//!
//! The rows differ along what each rule generalises over: the operation (all three), the
//! failure class (the memberless 410 and 403, the 400 that carries a sentence, the modelled
//! 413), the connection-id form, and the invoke-path prefix (none, a deployed stage, a stage
//! nothing deployed, a two-segment base path).

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;
use service_common::execute_api_connections_endpoint::EndpointAuthority;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{
    call_with_headers, error_code, error_message, ids_api_gateway_does_not_mint, rendered, register,
    HOST, MINTED_IDS,
};

/// The `Data` maximum the model declares, read from the crate constant.
const MAX_DATA_BYTES: usize = mgmt::ops::post_to_connection::MAX_DATA_BYTES;

/// The prefix the installed reading below treats as a deployed stage, and two it does not.
const DEPLOYED_PREFIX: &str = "/dev";
const UNDEPLOYED_PREFIXES: [(&str, &str); 3] = [
    ("a stage name nothing deployed", "/test"),
    ("a stage whose spelling is nearly the deployed one", "/Dev"),
    ("a two-segment base path nothing maps", "/v1/socket"),
];

/// The three operations, by the method each is reached with, and the payload each sends.
const OPERATIONS: [(&str, &[u8]); 3] = [("GET", b""), ("POST", b"push"), ("DELETE", b"")];

/// A registration instant in the past.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// A reading standing in for the owner of apis and stages: exactly one stage is deployed.
///
/// It reads the PREFIX alone, which is what this suite varies; the real reading resolves the
/// api and the deployment released to the stage out of that service's own records.
fn only_the_deployed_stage(
    _engine: &dyn Any,
    _host: Option<&str>,
    prefix: &str,
) -> EndpointAuthority {
    if prefix == DEPLOYED_PREFIX {
        EndpointAuthority::Deployed
    } else {
        EndpointAuthority::Undeployed
    }
}

/// An engine whose shared cell carries [`only_the_deployed_stage`] — what an emulator with
/// the owner of apis and stages mounted beside this service looks like.
fn engine_with_one_deployed_stage() -> Arc<InMemoryEngine> {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    mgmt::state::connections_endpoint_authority(&*engine)
        .expect("the shared cell lives in this engine")
        .lock()
        .install(only_the_deployed_stage);
    engine
}

/// The request path for `connection_id` under `prefix`.
fn path(prefix: &str, connection_id: &str) -> String {
    format!("{prefix}/@connections/{connection_id}")
}

/// A failure's body as a JSON document, naming the case when it is not one.
fn body_document(case: &str, body: &bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("case {case:?}: the error body must be JSON ({e}): {body:?}"))
}

/// The memberless modelled failures carry NO body member — the empty JSON document exactly —
/// and name their shape in `x-amzn-errortype` alone.
#[tokio::test]
async fn a_memberless_modelled_failure_carries_the_empty_json_document() {
    for (id_form, connection_id) in MINTED_IDS {
        for (method, payload) in OPERATIONS {
            // Registered and then deleted, so the id is well formed and nothing holds it:
            // the modelled `GoneException`, which declares no members.
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            register(&engine, connection_id, "192.0.2.7", "sender/1", connected_at());
            let (status, _, _) =
                call_with_headers(&engine, HOST, "DELETE", &path("", connection_id), Vec::new())
                    .await;
            assert_eq!(
                status, 204,
                "case {id_form:?}: the delete that closes the connection must answer 204"
            );

            let case = format!("{method} on a closed connection ({id_form})");
            let (status, headers, body) = call_with_headers(
                &engine,
                HOST,
                method,
                &path("", connection_id),
                payload.to_vec(),
            )
            .await;
            assert_eq!(
                status, 410,
                "case {case:?}: expected the modelled HTTP 410, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                "GoneException",
                "case {case:?}: expected the shape in x-amzn-errortype, got {}",
                error_code(&headers)
            );
            assert_eq!(
                body_document(&case, &body),
                serde_json::json!({}),
                "case {case:?}: a memberless error shape has nothing to serialize, so the \
                 body is the empty JSON document, got {}",
                rendered(&body)
            );
        }
    }
}

/// One row of the failure table below: a case name, the request it makes, and what its
/// body must carry.
struct FailureRow {
    case: &'static str,
    method: &'static str,
    connection_id: String,
    /// The invoke-path prefix the row addresses: an undeployed stage for the 403 row, the
    /// deployed one for every other.
    prefix: &'static str,
    expected_status: u16,
    /// Text the body's `message` must carry, or `None` for a memberless shape, whose body
    /// carries no member at all.
    expected_text: Option<&'static str>,
}

/// The same rule over the OTHER failures this service answers from a request: each body
/// carries the members its shape declares — a sentence where there is one — and never the
/// shape discriminator.
#[tokio::test]
async fn no_failure_body_carries_the_shape_discriminator() {
    let engine = engine_with_one_deployed_stage();
    let live = MINTED_IDS[0].1;
    register(&engine, live, "192.0.2.8", "sender/2", connected_at());

    // One row per failure class this service answers IN ITS OWN ENVELOPE, with the member
    // its shape declares. The over-limit payload is deliberately not a row: the endpoint in
    // front of the modelled operations refuses that one with the status alone, which
    // `connection_error_envelope` pins.
    let malformed = ids_api_gateway_does_not_mint();
    let (_, malformed_id) = &malformed[0];
    let rows = [
        FailureRow {
            case: "the memberless 410",
            method: "GET",
            connection_id: MINTED_IDS[1].1.to_string(),
            prefix: DEPLOYED_PREFIX,
            expected_status: 410,
            expected_text: None,
        },
        FailureRow {
            case: "the 403 of an undeployed stage",
            method: "GET",
            connection_id: MINTED_IDS[1].1.to_string(),
            prefix: UNDEPLOYED_PREFIXES[0].1,
            expected_status: 403,
            expected_text: Some("Forbidden"),
        },
        FailureRow {
            case: "the 400 of a malformed id",
            method: "GET",
            connection_id: malformed_id.clone(),
            prefix: DEPLOYED_PREFIX,
            expected_status: 400,
            expected_text: Some("Invalid connectionId: "),
        },
    ];

    for row in rows {
        let FailureRow {
            case,
            method,
            connection_id,
            prefix,
            expected_status,
            expected_text,
        } = row;
        let (status, headers, body) = call_with_headers(
            &engine,
            HOST,
            method,
            &path(prefix, &connection_id),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {}",
            rendered(&body)
        );
        let document = body_document(case, &body);
        assert!(
            document.get("__type").is_none(),
            "case {case:?}: a restJson1 body carries no shape discriminator, got {document}"
        );
        match expected_text {
            None => assert_eq!(
                document,
                serde_json::json!({}),
                "case {case:?}: a memberless error shape carries no member, got {document}"
            ),
            Some(text) => assert!(
                error_message(&body).contains(text),
                "case {case:?}: expected a message carrying {text:?}, got {}",
                error_message(&body)
            ),
        }
        assert!(
            !error_code(&headers).starts_with("<no "),
            "case {case:?}: a failure names its shape in x-amzn-errortype, got {}",
            error_code(&headers)
        );
    }
}

/// An endpoint whose stage no api has deployed is refused 403 `ForbiddenException` with the
/// sentence `Forbidden`, for every operation and every undeployed addressing — while the
/// deployed stage answers exactly what the modelled binding decides.
#[tokio::test]
async fn an_endpoint_that_deploys_no_stage_is_forbidden() {
    for (prefix_case, prefix) in UNDEPLOYED_PREFIXES {
        for (method, payload) in OPERATIONS {
            let engine = engine_with_one_deployed_stage();
            // A LIVE connection, so the refusal below is about the endpoint and not about
            // the connection: the stage is resolved before the store is consulted.
            let live = MINTED_IDS[0].1;
            register(&engine, live, "192.0.2.9", "sender/3", connected_at());

            let case = format!("{method} through {prefix_case} ({prefix})");
            let (status, headers, body) = call_with_headers(
                &engine,
                HOST,
                method,
                &path(prefix, live),
                payload.to_vec(),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected the modelled HTTP 403, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                "ForbiddenException",
                "case {case:?}: expected ForbiddenException in x-amzn-errortype, got {}",
                error_code(&headers)
            );
            assert_eq!(
                error_message(&body),
                "Forbidden",
                "case {case:?}: expected the sentence API Gateway refuses an endpoint with, \
                 got {}",
                rendered(&body)
            );

            // Nothing about the connection changed, which is what "before the store is
            // consulted" means on the wire.
            let stored = mgmt::state::connection(&*engine, live)
                .unwrap_or_else(|| panic!("case {case:?}: the connection must still exist"));
            assert_eq!(
                stored.delivered_count, 0,
                "case {case:?}: a refused call must deliver nothing, got {} deliveries",
                stored.delivered_count
            );
            assert_eq!(
                stored.last_active_at, stored.connected_at,
                "case {case:?}: a refused call must not move LastActiveAt, got {:?}",
                stored.last_active_at
            );

            // And the DEPLOYED stage serves the very same call.
            let (status, _, body) = call_with_headers(
                &engine,
                HOST,
                method,
                &path(DEPLOYED_PREFIX, live),
                payload.to_vec(),
            )
            .await;
            let expected = if method == "DELETE" { 204 } else { 200 };
            assert_eq!(
                status, expected,
                "case {case:?}: the deployed stage must answer {expected}, got {status} {}",
                rendered(&body)
            );
        }
    }
}

/// The endpoint is judged FIRST: an undeployed stage answers 403 even for a `ConnectionId`
/// of a form the service would otherwise refuse 400, and even for an over-limit payload the
/// payload gate would otherwise refuse 413.
#[tokio::test]
async fn the_endpoint_is_judged_before_the_request_it_carries() {
    let malformed = ids_api_gateway_does_not_mint();
    for (prefix_case, prefix) in UNDEPLOYED_PREFIXES {
        let engine = engine_with_one_deployed_stage();
        let live = MINTED_IDS[0].1;
        register(&engine, live, "192.0.2.10", "sender/4", connected_at());

        // A malformed id on an undeployed stage.
        for (id_form, connection_id) in &malformed {
            let case = format!("GET {id_form} through {prefix_case} ({prefix})");
            let (status, headers, body) = call_with_headers(
                &engine,
                HOST,
                "GET",
                &path(prefix, connection_id),
                Vec::new(),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: the endpoint is judged before the ConnectionId, so expected \
                 HTTP 403, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                "ForbiddenException",
                "case {case:?}: expected ForbiddenException, got {}",
                error_code(&headers)
            );
        }

        // An over-limit payload on an undeployed stage.
        let case = format!("POST an over-limit payload through {prefix_case} ({prefix})");
        let (status, headers, body) = call_with_headers(
            &engine,
            HOST,
            "POST",
            &path(prefix, live),
            vec![b'x'; MAX_DATA_BYTES + 1],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: the endpoint is judged before the payload, so expected HTTP 403, \
             got {status} {}",
            rendered(&body)
        );
        assert_eq!(
            error_code(&headers),
            "ForbiddenException",
            "case {case:?}: expected ForbiddenException, got {}",
            error_code(&headers)
        );
    }
}

/// With no reading installed — an emulator the owner of apis and stages is not mounted in —
/// nothing judges the addressing, so every prefix is served as the modelled binding decides.
#[tokio::test]
async fn an_addressing_nothing_judges_is_served() {
    for (prefix_case, prefix) in UNDEPLOYED_PREFIXES {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let live = MINTED_IDS[0].1;
        register(&engine, live, "192.0.2.11", "sender/5", connected_at());

        let case = format!("GET through {prefix_case} ({prefix}) with nothing judging it");
        let (status, _, body) =
            call_with_headers(&engine, HOST, "GET", &path(prefix, live), Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registry's own answer, got {status} {}",
            rendered(&body)
        );
    }
}

/// A `ConnectionId` the client percent-encoded — which is how a `+`, `/` or `=` travels in a
/// path segment — is read as the id it decodes to, so its form is judged on the DECODED
/// value.
#[tokio::test]
async fn a_percent_encoded_connection_id_is_judged_on_its_decoded_value() {
    // The standard base64 alphabet's two non-alphanumeric characters and the padding, which
    // a client percent-encodes in a path segment.
    let decoded = "ab+/efghij1234Q=";
    let encoded = "ab%2B%2Fefghij1234Q%3D";
    assert!(
        mgmt::state::is_well_formed_connection_id(decoded),
        "the case's id {decoded:?} must be the form API Gateway mints"
    );

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    register(&engine, decoded, "192.0.2.12", "sender/6", connected_at());

    let (status, _, body) =
        call_with_headers(&engine, HOST, "GET", &path("", encoded), Vec::new()).await;
    assert_eq!(
        status, 200,
        "case \"a percent-encoded id\": expected the registry's own answer for \
         {decoded:?}, got {status} {}",
        rendered(&body)
    );
}
