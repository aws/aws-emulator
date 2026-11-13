// SPDX-License-Identifier: CC0-1.0

//! Wire-shape probes for `ApiGatewayManagementApi` on restJson1: the exact bytes,
//! status codes and JSON keys a real AWS SDK client parses.
//!
//! The lifecycle suite proves each response is computed from the connection registry.
//! This one pins the ENVELOPE that registry is reported in, where the model — not the
//! stored value — decides the form:
//!
//! * `ConnectedAt` / `LastActiveAt` target `__timestampIso8601`, which the model marks
//!   `@timestampFormat("date-time")`, so they render as RFC 3339 date-time STRINGS, not
//!   as the restJson1 default epoch-seconds number.
//! * the `@jsonName`s the model declares (`connectedAt`, `identity`, `lastActiveAt`,
//!   `sourceIp`, `userAgent`) are the keys on the wire.
//! * a modelled error renders with that shape's `@httpError` status, its shape named in
//!   `x-amzn-errortype` and a body carrying the shape's own modelled members — so a
//!   memberless one carries none at all.
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call, call_with_headers, error_code, minted, register, HOST};

/// Registration instants and the RFC 3339 text a `@timestampFormat("date-time")` member
/// renders them as. Written out by hand (the format is the model's, not this crate's)
/// so a fallback to restJson1's default epoch-seconds encoding fails here instead of
/// silently shipping a number where a string belongs. Rows span the epoch itself, the
/// second after it, and three widely separated later instants.
const TIMESTAMP_TABLE: [(u64, &str); 5] = [
    (0, "1970-01-01T00:00:00Z"),
    (1, "1970-01-01T00:00:01Z"),
    (1_000_000, "1970-01-12T13:46:40Z"),
    (1_600_000_000, "2020-09-13T12:26:40Z"),
    (1_700_000_000, "2023-11-14T22:13:20Z"),
];

fn register_at(engine: &Arc<InMemoryEngine>, connection_id: &str, connected_at_secs: u64) {
    register(
        engine,
        connection_id,
        "192.0.2.80",
        "probe/1",
        SystemTime::UNIX_EPOCH + Duration::from_secs(connected_at_secs),
    );
}

#[tokio::test]
async fn get_connection_renders_iso8601_date_times_and_the_modeled_json_names() {
    for (secs, expected) in TIMESTAMP_TABLE {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = minted("probe-ts");
        register_at(&engine, &connection_id, secs);

        let (status, body) = call(&engine, "GET", &connection_id, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {secs}: GetConnection must answer 200, got {status}"
        );
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("case {secs}: body must be JSON ({e}): {body:?}"));

        // Nothing was delivered, so both instants are the registration instant and both
        // must render in the modelled format.
        for member in ["connectedAt", "lastActiveAt"] {
            let rendered = value.get(member);
            assert_eq!(
                rendered.and_then(serde_json::Value::as_str),
                Some(expected),
                "case {secs}: {member} must be the ISO 8601 date-time {expected:?}, got {rendered:?}"
            );
        }

        assert_eq!(
            value.pointer("/identity/sourceIp").and_then(serde_json::Value::as_str),
            Some("192.0.2.80"),
            "case {secs}: identity.sourceIp must use the modelled @jsonName keys, got {value}"
        );
        assert_eq!(
            value.pointer("/identity/userAgent").and_then(serde_json::Value::as_str),
            Some("probe/1"),
            "case {secs}: identity.userAgent must use the modelled @jsonName keys, got {value}"
        );
    }
}

/// Each failure this service answers in its own envelope renders in the restJson1 error
/// envelope with its own status: one row per such failure. The two refusals of a
/// `ConnectionId` differ — an id that could never have been minted is the 400
/// `BadRequestException`, a well-formed id naming no open connection is the modelled 410
/// `GoneException` — so both are pinned here.
#[tokio::test]
async fn modeled_errors_render_with_their_own_status_and_type() {
    /// Which connection id a row addresses.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Target {
        /// An id that could never have been minted.
        Malformed,
        /// A well-formed id no connection was ever opened for.
        NeverMinted,
        /// An id whose connection the row deletes first.
        Deleted,
    }

    // (case, method, request body length, target, expected status, expected error shape)
    let table: [(&str, &str, usize, Target, u16, &str); 6] = [
        (
            "get on an id that could never have been minted",
            "GET",
            0,
            Target::Malformed,
            400,
            "BadRequestException",
        ),
        (
            "post to an id that could never have been minted",
            "POST",
            4,
            Target::Malformed,
            400,
            "BadRequestException",
        ),
        (
            "delete an id that could never have been minted",
            "DELETE",
            0,
            Target::Malformed,
            400,
            "BadRequestException",
        ),
        (
            "get on a well-formed id nothing minted",
            "GET",
            0,
            Target::NeverMinted,
            410,
            "GoneException",
        ),
        (
            "post to a closed connection",
            "POST",
            4,
            Target::Deleted,
            410,
            "GoneException",
        ),
        (
            "delete a closed connection",
            "DELETE",
            0,
            Target::Deleted,
            410,
            "GoneException",
        ),
    ];

    for (case, method, body_len, target, expected_status, expected_type) in table {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        // One open connection, so every failure below is about the id that was asked for
        // and not about an empty registry.
        register_at(&engine, &minted("probe-err"), 1_700_000_000);

        let connection_id = match target {
            Target::Malformed => "not-registered".to_string(),
            Target::NeverMinted => minted("probe-never"),
            Target::Deleted => {
                let gone = minted("probe-gone");
                register_at(&engine, &gone, 1_700_000_000);
                let (status, _) = call(&engine, "DELETE", &gone, Vec::new()).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: the delete that closes the connection must answer 204, \
                     got {status}"
                );
                gone
            }
        };
        let (status, headers, body) = call_with_headers(
            &engine,
            HOST,
            method,
            &format!("/@connections/{connection_id}"),
            vec![b'x'; body_len],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&headers),
            expected_type,
            "case {case:?}: expected the error shape {expected_type:?} in x-amzn-errortype, \
             got {}",
            error_code(&headers)
        );
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("case {case:?}: error body must be JSON ({e}): {body:?}"));
        assert!(
            value.get("__type").is_none(),
            "case {case:?}: a restJson1 error body names no shape, got {value}"
        );
    }
}

/// An over-limit `PostToConnection` payload is refused by the `@connections` endpoint in
/// front of the modelled operations, so it is the one answer of this surface that carries
/// NO error envelope at all: the status alone, with no body to decode. Sizes span the first
/// byte past the published maximum and well past it.
#[tokio::test]
async fn an_oversized_payload_is_refused_with_the_status_alone() {
    let max = mgmt::ops::post_to_connection::MAX_DATA_BYTES;
    for size in [max + 1, max + 1024, max * 3] {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = minted("probe-big");
        register_at(&engine, &connection_id, 1_700_000_000);

        let (status, body) = call(&engine, "POST", &connection_id, vec![b'x'; size]).await;
        assert_eq!(status, 413, "case {size}: expected HTTP 413, got {status}");
        assert!(
            body.is_empty(),
            "case {size}: the refusal carries no body, got {body:?}"
        );
        let stored = mgmt::state::connection(&*engine, &connection_id)
            .expect("the connection must still be registered");
        assert_eq!(
            stored.delivered_count, 0,
            "case {size}: a refused payload must not be delivered, got {} deliveries",
            stored.delivered_count
        );
    }
}

/// `DeleteConnection` models no output members and a 204 success status, so the
/// response carries no body at all — not an empty JSON object.
#[tokio::test]
async fn delete_connection_answers_204_with_an_empty_body() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let connection_id = minted("probe-del");
    register_at(&engine, &connection_id, 1_600_000_000);

    let (status, body) = call(&engine, "DELETE", &connection_id, Vec::new()).await;
    assert_eq!(status, 204, "expected HTTP 204, got {status}");
    assert!(body.is_empty(), "a 204 carries no body, got {body:?}");
}

/// `PostToConnection` models no output members and a 200 success status.
#[tokio::test]
async fn post_to_connection_answers_200_with_an_empty_body() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let connection_id = minted("probe-post");
    register_at(&engine, &connection_id, 1_600_000_000);

    let (status, body) = call(&engine, "POST", &connection_id, b"payload".to_vec()).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}");
    assert!(body.is_empty(), "the success response carries no body, got {body:?}");
}
