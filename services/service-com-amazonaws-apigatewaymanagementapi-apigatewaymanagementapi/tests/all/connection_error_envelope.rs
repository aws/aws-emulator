// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE every modelled failure of the `@connections` surface is answered in —
//! status, `content-type` and `x-amzn-errortype`, the three things a restJson1 client
//! reads before it looks at a body at all.
//!
//! The rule under test: **a `@connections` request is answered by this service in its
//! own modelled restJson1 envelope — the HTTP status is the one declared for the failure
//! (the `@httpError` of the error shape for a modelled one, so always a terminal 4xx and
//! never a retryable 5xx), `content-type` is `application/json`, and `x-amzn-errortype`
//! names that failure's shape** — whichever operation it names, whichever endpoint path
//! it is addressed through and whatever the connection id looks like.
//!
//! WHY it is pinned here. This service signs as `execute-api`, a signing name it SHARES
//! with the Amazon Connect Participant Service, and the router resolves a request's
//! routing key from the SigV4 credential scope: with both services claiming on the
//! signing name alone the router found two claimants and answered every `@connections`
//! call `500 AmbiguousService` in an `application/x-amz-json-1.1` envelope — a status no
//! operation of this service declares, a code no AWS model declares, and a media type
//! this service does not speak. A `410` tells a sender to stop writing to a dead
//! WebSocket connection; a retryable `500` makes it retry forever and never reap the
//! connection, and the wrong media type keeps the SDK from modelling the failure at all.
//! `crate::service::can_handle` now claims on the signing name AND this service's own
//! modelled path space (`/@connections/…`), so the request reaches a handler — and these
//! rows pin what the handler's answer looks like on the wire.
//!
//! Each row of each table differs along what the rule generalises over: the operation
//! (all three), the endpoint form the caller addresses (the modelled path, a deployed
//! stage's invoke path, a custom domain's two-segment base path, on three different
//! hosts), the connection-id form, the failure class (`GoneException` for a well-formed id
//! naming no open connection, `BadRequestException` for an id that could never have been
//! minted) and the payload size.
//!
//! The ONE answer of this surface that is NOT in this envelope is the refusal of an
//! over-limit `PostToConnection` payload, which the endpoint in front of the modelled
//! operations answers itself; `the_oversized_payload_refusal` pins that, and
//! `crate::service::OversizedPayload` says why.
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use engine::InMemoryEngine;
use http::HeaderMap;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call_with_headers, minted, register, HOST};

/// `GoneException` is a client error with `@httpError(410)`
/// (`smithy_query(query="shape",shape="GoneException")`), and it is the common error all
/// three operations declare for a connection id the registry does not hold.
const GONE_STATUS: u16 = 410;
const GONE_TYPE: &str = "GoneException";

/// The refusal of a `ConnectionId` that could never have been minted. The model declares no
/// client-input error for these operations, so the status and code are the ones API
/// Gateway words such a refusal in (`crate::state::invalid_connection_id`), and the
/// envelope is the service's own restJson1 one, exactly as a modelled failure's is.
const INVALID_ID_STATUS: u16 = 400;
const INVALID_ID_TYPE: &str = "BadRequestException";

/// The status the `@connections` endpoint refuses an over-limit `Data` payload with — the
/// same 413 the model declares for `PayloadTooLargeException`
/// (`smithy_query(query="shape",shape="PayloadTooLargeException")`), answered by the
/// endpoint in front of the modelled operations and so NOT in the modelled envelope.
const TOO_LARGE_STATUS: u16 = 413;

/// The media type restJson1 serializes a body in, success or failure.
const JSON: &str = "application/json";

/// The `Data` maximum this service's model declares (`@length(min: 0, max: 131072)` on
/// the `Data` blob). Read from the crate constant so the two cannot disagree.
const MAX_DATA_BYTES: usize = mgmt::ops::post_to_connection::MAX_DATA_BYTES;

/// The three operations the model binds under `/@connections/{ConnectionId}`, by the
/// HTTP method each one is reached with (`smithy_query(query="digest")`: `GET`,
/// `POST`, `DELETE`).
const METHODS: [&str; 3] = ["GET", "POST", "DELETE"];

/// One endpoint form a caller addresses these operations through: the host, and the
/// prefix that endpoint's URL puts in front of the modelled binding.
///
/// AWS documents the endpoint as
/// `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`, and a
/// custom domain's base-path mapping adds its own segments, so the prefix spans none, one
/// and two segments.
const ENDPOINTS: [(&str, &str, &str); 3] = [
    ("the modelled path itself", HOST, ""),
    (
        "a deployed stage's invoke path",
        "a1b2c3d4e5.execute-api.us-east-1.amazonaws.com",
        "/prod",
    ),
    (
        "a custom domain's two-segment base path",
        "sockets.example.com",
        "/v1/socket",
    ),
];

/// Connection-id forms of the shape API Gateway MINTS.
///
/// `GetConnectionRequest.ConnectionId` targets `__string` with `@httpLabel` and
/// `@required` and NO `@pattern` or `@length`
/// (`smithy_query(query="shape",shape="GetConnectionRequest")`), so what decides the answer
/// is the shape API Gateway MINTS an id in (`crate::state::is_well_formed_connection_id`).
/// These rows are of that shape — the token this service hands out — since every row here
/// addresses a connection that exists or existed.
fn id_forms() -> Vec<(&'static str, String)> {
    vec![
        ("a token carrying this case's name", minted("envelope-1")),
        ("another token of that shape", minted("envelope-2")),
        (
            "an all-A token of that shape",
            "AAAAAAAAAAAAAAA=".to_string(),
        ),
    ]
}

/// Connection-id forms that could never have been minted, which is what the 400 refusal is
/// about: a single character, text that is not base64, an unpadded token and base64 of the
/// wrong number of bytes.
const MALFORMED_ID_FORMS: [(&str, &str); 4] = [
    ("a single character", "x"),
    ("a dashed id", "cid-with-dashes"),
    ("an unpadded token", "Fabc1cWIPHcCJ7A"),
    ("base64 of too many bytes", "0123456789abcdef"),
];

/// A registration instant in the past, so an instant the service writes later is
/// strictly greater.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// One header's value as text, or `"<absent>"`.
fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
        .unwrap_or_else(|| "<absent>".to_string())
}

/// Assert one response IS the modelled envelope for `expected_type`: the modelled
/// status, the restJson1 media type, the error shape in `x-amzn-errortype`, and a JSON
/// object body an SDK can decode.
fn assert_modelled_envelope(
    case: &str,
    expected_status: u16,
    expected_type: &str,
    answer: &(u16, HeaderMap, Bytes),
) {
    let (status, headers, body) = answer;
    let rendered = String::from_utf8_lossy(body);
    assert_eq!(
        *status, expected_status,
        "case {case:?}: expected the modelled HTTP {expected_status}, got {status} \
         {rendered:?}"
    );
    // Stated separately from the equality above: a 5xx is RETRYABLE, so answering one
    // here is not merely the wrong number — it makes a client retry forever instead of
    // reaping the connection.
    assert!(
        *status < 500,
        "case {case:?}: a modelled failure must be a terminal 4xx, not a retryable \
         server fault, got {status} {rendered:?}"
    );
    assert_eq!(
        header(headers, "content-type"),
        JSON,
        "case {case:?}: expected the restJson1 media type {JSON:?}, got {:?}",
        header(headers, "content-type")
    );
    assert_eq!(
        header(headers, "x-amzn-errortype"),
        expected_type,
        "case {case:?}: expected x-amzn-errortype to name the modelled shape \
         {expected_type:?}, got {:?}",
        header(headers, "x-amzn-errortype")
    );
    let value: serde_json::Value = serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("case {case:?}: the error body must be JSON ({e}): {rendered:?}"));
    assert!(
        value.is_object(),
        "case {case:?}: the error body must be a JSON object, got {value}"
    );
}

/// An id that could never have been minted names no connection anywhere, so it is refused
/// as an invalid `ConnectionId` — HTTP 400 `BadRequestException` — in the SAME envelope the
/// modelled failures travel in, for every operation, every endpoint form and every id
/// form.
#[tokio::test]
async fn a_malformed_connection_id_is_refused_in_the_same_envelope() {
    for (endpoint, host, prefix) in ENDPOINTS {
        for (id_form, connection_id) in MALFORMED_ID_FORMS {
            for method in METHODS {
                // One OTHER connection is registered, so each failure below is about the
                // id that was asked for and not about an empty registry.
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let open = minted("open");
                register(&engine, &open, "192.0.2.1", "other/1", connected_at());

                let path = format!("{prefix}/@connections/{connection_id}");
                let payload = if method == "POST" {
                    b"push".to_vec()
                } else {
                    Vec::new()
                };
                let answer = call_with_headers(&engine, host, method, &path, payload).await;
                assert_modelled_envelope(
                    &format!("{method} {path} on {host} ({endpoint}, {id_form})"),
                    INVALID_ID_STATUS,
                    INVALID_ID_TYPE,
                    &answer,
                );
                assert!(
                    mgmt::state::connection(&*engine, connection_id).is_none(),
                    "case {method} {path}: a failed call must not register the value it \
                     refused"
                );
            }
        }
    }
}

/// The fixture's own sequence: a connection is registered, `DeleteConnection` removes it,
/// and every later call on that id — `GetConnection`, `PostToConnection` and a second
/// `DeleteConnection` — answers the modelled `GoneException` in the modelled envelope.
#[tokio::test]
async fn a_deleted_connection_answers_gone_in_the_modelled_envelope() {
    for (endpoint, host, prefix) in ENDPOINTS {
        for (id_form, connection_id) in id_forms() {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            register(&engine, &connection_id, "198.51.100.7", "sender/1", connected_at());

            let path = format!("{prefix}/@connections/{connection_id}");
            let case = format!("{path} on {host} ({endpoint}, {id_form})");

            // The delete itself is the modelled 204, which is what makes the connection
            // gone; it is not an error envelope.
            let (status, _, body) =
                call_with_headers(&engine, host, "DELETE", &path, Vec::new()).await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeleteConnection must answer the modelled 204, got \
                 {status} {:?}",
                String::from_utf8_lossy(&body)
            );
            assert!(
                mgmt::state::connection(&*engine, &connection_id).is_none(),
                "case {case:?}: the deleted record must be gone from the registry"
            );

            for method in METHODS {
                let payload = if method == "POST" {
                    b"push after the delete".to_vec()
                } else {
                    Vec::new()
                };
                let answer = call_with_headers(&engine, host, method, &path, payload).await;
                assert_modelled_envelope(
                    &format!("{method} after the delete, {case}"),
                    GONE_STATUS,
                    GONE_TYPE,
                    &answer,
                );
            }
        }
    }
}

/// The ONE answer of this surface that is not in the modelled envelope: the refusal of an
/// over-limit `PostToConnection` payload, which the `@connections` ENDPOINT answers in
/// front of the modelled operations — the status alone, with no body and nothing naming an
/// error shape, so a restJson1 client resolves no modelled error from it and reports the
/// status itself. Sizes span the first byte past the published maximum and well past it.
#[tokio::test]
async fn the_oversized_payload_refusal_is_the_status_alone() {
    for (endpoint, host, prefix) in ENDPOINTS {
        for size in [MAX_DATA_BYTES + 1, MAX_DATA_BYTES + 1024, MAX_DATA_BYTES * 2] {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let connection_id = minted("oversize");
            register(&engine, &connection_id, "203.0.113.9", "sender/2", connected_at());

            let path = format!("{prefix}/@connections/{connection_id}");
            let case = format!("POST {size} bytes to {path} on {host} ({endpoint})");
            let (status, headers, body) =
                call_with_headers(&engine, host, "POST", &path, vec![b'x'; size]).await;
            assert_eq!(
                status, TOO_LARGE_STATUS,
                "case {case:?}: expected HTTP {TOO_LARGE_STATUS}, got {status} {:?}",
                String::from_utf8_lossy(&body)
            );
            assert_eq!(
                header(&headers, "x-amzn-errortype"),
                "<absent>",
                "case {case:?}: the refusal names no error shape, got {:?}",
                header(&headers, "x-amzn-errortype")
            );
            assert!(
                body.is_empty(),
                "case {case:?}: the refusal carries no body, got {:?}",
                String::from_utf8_lossy(&body)
            );
            let stored = mgmt::state::connection(&*engine, &connection_id)
                .expect("the connection must still be registered");
            assert_eq!(
                stored.delivered_count, 0,
                "case {case:?}: a refused payload must not be delivered, got \
                 {} deliveries",
                stored.delivered_count
            );
        }
    }
}

/// A SUCCESS is served in the same media type, from the same registry — the envelope
/// rule is about every answer, not only the failures, and it is what shows the request
/// reached this service rather than the router's refusal.
#[tokio::test]
async fn a_registered_connection_is_reported_in_the_restjson_media_type() {
    for (endpoint, host, prefix) in ENDPOINTS {
        for (id_form, connection_id) in id_forms() {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            register(&engine, &connection_id, "192.0.2.44", "reader/1", connected_at());

            let path = format!("{prefix}/@connections/{connection_id}");
            let case = format!("GET {path} on {host} ({endpoint}, {id_form})");
            let (status, headers, body) =
                call_with_headers(&engine, host, "GET", &path, Vec::new()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetConnection on a registered connection must answer \
                 200, got {status} {:?}",
                String::from_utf8_lossy(&body)
            );
            assert_eq!(
                header(&headers, "content-type"),
                JSON,
                "case {case:?}: expected the restJson1 media type {JSON:?}, got {:?}",
                header(&headers, "content-type")
            );
            assert_eq!(
                header(&headers, "x-amzn-errortype"),
                "<absent>",
                "case {case:?}: a success names no error shape, got {:?}",
                header(&headers, "x-amzn-errortype")
            );
        }
    }
}
