// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the whole modelled surface of `ApiGatewayManagementApi`:
//! `PostToConnection`, `GetConnection` and `DeleteConnection`, driven over HTTP through
//! the same `MultiServiceRouter` the production emulator serves with.
//!
//! The rule under test, stated once: **every response is computed from the connection
//! registry in this service's engine state** — a connection exists only from the moment
//! the registration seam registered it until `DeleteConnection` removes it, each
//! operation reads or writes that one record, a well-formed `ConnectionId` the registry no
//! longer holds is `GoneException` (HTTP 410) for all three operations, and one that could
//! never have been minted is refused as an invalid `ConnectionId` (HTTP 400
//! `BadRequestException`).
//!
//! Each test asserts that rule over a TABLE of inputs rather than over one recorded
//! call: the rows differ along connection-id form, identity values, registration
//! instant, payload count, payload size and payload content.
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{
    as_wire_timestamp, call, call_with_headers, error_code, ids_api_gateway_does_not_mint, minted,
    rendered, register, HOST,
};

/// The `Data` maximum this service's model declares (`@length(min: 0, max: 131072)` on
/// the `Data` blob). Read from the crate constant so the two cannot disagree.
const MAX_DATA_BYTES: usize = mgmt::ops::post_to_connection::MAX_DATA_BYTES;

/// One row of the registry table.
#[derive(Debug, Clone)]
struct Case {
    /// Names the row in every assertion message.
    name: &'static str,
    connection_id: String,
    source_ip: &'static str,
    user_agent: &'static str,
    /// Seconds past the epoch the connection is registered at. Deliberately in the
    /// past, so an instant the service writes later is strictly greater.
    connected_at_secs: u64,
    /// The payloads `PostToConnection` sends, in order.
    payloads: Vec<Vec<u8>>,
}

impl Case {
    fn connected_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.connected_at_secs)
    }
}

/// The table every lifecycle test runs. Rows differ along identity values (IPv4 / IPv6 /
/// proxied), registration instant, payload count, payload size and payload content (text,
/// raw binary, empty); every `ConnectionId` is of the shape the service mints, since that
/// is the only shape a connection can be registered under.
fn registry_table() -> Vec<Case> {
    vec![
        Case {
            name: "no payload sent yet",
            connection_id: minted("no-payload"),
            source_ip: "192.0.2.10",
            user_agent: "AWS-SDK-Test/1.0",
            connected_at_secs: 1_000_000,
            payloads: vec![],
        },
        Case {
            name: "one short text payload",
            connection_id: minted("one-text"),
            source_ip: "198.51.100.7",
            user_agent: "Mozilla/5.0 (compatible)",
            connected_at_secs: 1_700_000_000,
            payloads: vec![b"hello".to_vec()],
        },
        Case {
            name: "three payloads keep send order",
            connection_id: minted("three-ones"),
            source_ip: "2001:db8::1",
            user_agent: "wscat/5.2.0",
            connected_at_secs: 1_600_000_000,
            payloads: vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()],
        },
        Case {
            name: "empty and raw-binary payloads",
            connection_id: minted("raw-bytes"),
            source_ip: "203.0.113.255",
            user_agent: "curl/8.5.0",
            connected_at_secs: 1,
            payloads: vec![Vec::new(), (0u8..=255).collect(), vec![0xff; 1024]],
        },
    ]
}

fn register_table(engine: &Arc<InMemoryEngine>, table: &[Case]) {
    for case in table {
        register(
            engine,
            &case.connection_id,
            case.source_ip,
            case.user_agent,
            case.connected_at(),
        );
    }
}

/// Register every row of the table on one engine, send its payloads, and read each
/// connection back through `GetConnection`: every member of the response must be the
/// stored record's, and one connection's traffic must not touch another's.
#[tokio::test]
async fn registered_connections_round_trip_through_get_connection() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let table = registry_table();
    register_table(&engine, &table);

    for case in &table {
        for (index, payload) in case.payloads.iter().enumerate() {
            let (status, body) =
                call(&engine, "POST", &case.connection_id, payload.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: PostToConnection #{index} ({} bytes) must answer 200, got {status} {}",
                case.name,
                payload.len(),
                rendered(&body)
            );
            assert!(
                body.is_empty(),
                "case {:?}: PostToConnection models no output members, got body {:?}",
                case.name,
                body
            );
        }
    }

    for case in &table {
        let (status, body) = call(&engine, "GET", &case.connection_id, Vec::new()).await;
        assert_eq!(
            status,
            200,
            "case {:?}: GetConnection must answer 200, got {status} {}",
            case.name,
            rendered(&body)
        );

        let stored = mgmt::state::connection(&*engine, &case.connection_id)
            .unwrap_or_else(|| panic!("case {:?}: the record must still be registered", case.name));

        // The persisted record is what the operations wrote: ConnectedAt is still the
        // instant the seam registered, the payloads are all there in send order, and
        // LastActiveAt moved only if a payload was delivered.
        assert_eq!(
            stored.connected_at,
            case.connected_at(),
            "case {:?}: ConnectedAt must stay the registration instant, got {:?}",
            case.name,
            stored.connected_at
        );
        assert_eq!(
            stored.messages, case.payloads,
            "case {:?}: the delivered payloads must be recorded in send order, got {} messages",
            case.name,
            stored.messages.len()
        );
        assert_eq!(
            stored.delivered_count,
            case.payloads.len() as u64,
            "case {:?}: every delivery must be counted, got {}",
            case.name,
            stored.delivered_count
        );
        assert_eq!(
            stored.delivered_bytes,
            case.payloads.iter().map(|p| p.len() as u64).sum::<u64>(),
            "case {:?}: delivered bytes must total the payloads sent, got {}",
            case.name,
            stored.delivered_bytes
        );
        if case.payloads.is_empty() {
            assert_eq!(
                stored.last_active_at,
                case.connected_at(),
                "case {:?}: with nothing delivered LastActiveAt must still be ConnectedAt, got {:?}",
                case.name,
                stored.last_active_at
            );
        } else {
            assert!(
                stored.last_active_at > case.connected_at(),
                "case {:?}: a delivered payload must advance LastActiveAt past ConnectedAt, got {:?}",
                case.name,
                stored.last_active_at
            );
        }

        // And the wire response is that record, member for member.
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!("case {:?}: GetConnection body must be JSON ({e}): {body:?}", case.name)
        });
        assert_eq!(
            value.get("connectedAt").and_then(serde_json::Value::as_str),
            Some(as_wire_timestamp(stored.connected_at).as_str()),
            "case {:?}: connectedAt must render the stored instant, got {value}",
            case.name
        );
        assert_eq!(
            value.get("lastActiveAt").and_then(serde_json::Value::as_str),
            Some(as_wire_timestamp(stored.last_active_at).as_str()),
            "case {:?}: lastActiveAt must render the stored instant, got {value}",
            case.name
        );
        assert_eq!(
            value
                .pointer("/identity/sourceIp")
                .and_then(serde_json::Value::as_str),
            Some(case.source_ip),
            "case {:?}: identity.sourceIp must be the recorded one, got {value}",
            case.name
        );
        assert_eq!(
            value
                .pointer("/identity/userAgent")
                .and_then(serde_json::Value::as_str),
            Some(case.user_agent),
            "case {:?}: identity.userAgent must be the recorded one, got {value}",
            case.name
        );
    }
}

/// `GetConnection` reports; it must not change what a later caller sees. Two reads in a
/// row answer identically, and only a `PostToConnection` between them moves
/// `LastActiveAt`.
#[tokio::test]
async fn get_connection_reports_without_advancing_last_active_at() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let connection_id = minted("report-only");
    let connected_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    register(
        &engine,
        &connection_id,
        "192.0.2.44",
        "reader/1",
        connected_at,
    );

    let (_, first) = call(&engine, "GET", &connection_id, Vec::new()).await;
    let (_, second) = call(&engine, "GET", &connection_id, Vec::new()).await;
    assert_eq!(
        first, second,
        "case {:?}: two reads with no activity between them must answer identically, got {first:?} then {second:?}",
        "repeated read"
    );

    let (status, _) = call(&engine, "POST", &connection_id, b"ping".to_vec()).await;
    assert_eq!(status, 200, "PostToConnection must answer 200, got {status}");

    let (_, third) = call(&engine, "GET", &connection_id, Vec::new()).await;
    assert_ne!(
        second, third,
        "case {:?}: delivering data must change the reported LastActiveAt, got {third:?}",
        "read after activity"
    );
    let stored = mgmt::state::connection(&*engine, &connection_id).expect("still registered");
    assert_eq!(
        stored.connected_at, connected_at,
        "ConnectedAt must never move, got {:?}",
        stored.connected_at
    );
    assert!(
        stored.last_active_at > connected_at,
        "LastActiveAt must advance past ConnectedAt, got {:?}",
        stored.last_active_at
    );
}

/// The `Data` size table. Every size up to and including the published maximum is
/// delivered; the first size past it, and bigger ones, are refused by the `@connections`
/// endpoint with the status alone (HTTP 413) and deliver nothing.
///
/// Bounds come from the model's `@length(min: 0, max: 131072)` on the `Data` blob
/// shape: both published bounds are tested, plus one byte past the maximum.
#[tokio::test]
async fn the_data_size_table_pins_the_modeled_length_bound() {
    let accepted: [(&str, usize); 5] = [
        ("the published minimum", 0),
        ("one byte", 1),
        ("one kibibyte", 1024),
        ("one byte under the maximum", MAX_DATA_BYTES - 1),
        ("the published maximum", MAX_DATA_BYTES),
    ];
    let refused: [(&str, usize); 3] = [
        ("one byte past the maximum", MAX_DATA_BYTES + 1),
        ("a kibibyte past the maximum", MAX_DATA_BYTES + 1024),
        ("twice the maximum", MAX_DATA_BYTES * 2),
    ];

    for (name, size) in accepted {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = minted("sized");
        register(
            &engine,
            &connection_id,
            "192.0.2.1",
            "sizer/1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_234_567),
        );

        let payload = vec![b'x'; size];
        let (status, body) = call(&engine, "POST", &connection_id, payload.clone()).await;
        assert_eq!(
            status,
            200,
            "case {name:?} ({size} bytes): must be delivered with 200, got {status} {}",
            rendered(&body)
        );
        let stored = mgmt::state::connection(&*engine, &connection_id).expect("still registered");
        assert_eq!(
            stored.messages,
            vec![payload],
            "case {name:?} ({size} bytes): the payload must be recorded verbatim, got {} messages of {:?} bytes",
            stored.messages.len(),
            stored.messages.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }

    for (name, size) in refused {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = minted("sized");
        register(
            &engine,
            &connection_id,
            "192.0.2.1",
            "sizer/1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_234_567),
        );

        let (status, body) = call(&engine, "POST", &connection_id, vec![b'x'; size]).await;
        assert_eq!(
            status,
            413,
            "case {name:?} ({size} bytes): must be refused with 413, got {status} {}",
            rendered(&body)
        );
        // The refusal is the `@connections` endpoint's own, in front of the modelled
        // operations, so it carries no error envelope to decode; `connection_error_envelope`
        // pins that answer.
        assert!(
            body.is_empty(),
            "case {name:?} ({size} bytes): the refusal carries no body, got {body:?}"
        );
        let stored = mgmt::state::connection(&*engine, &connection_id).expect("still registered");
        assert!(
            stored.messages.is_empty(),
            "case {name:?} ({size} bytes): a refused payload must not be delivered, got {} messages",
            stored.messages.len()
        );
        assert_eq!(
            stored.delivered_count, 0,
            "case {name:?} ({size} bytes): a refused payload must not be counted, got {}",
            stored.delivered_count
        );
    }
}

/// The retained-payload window is BOUNDED: delivering more than
/// `state::MAX_RETAINED_MESSAGES` payloads keeps the most recent ones and drops the
/// oldest, while the counters stay exact. Rows cross the bound by one and by a lot.
#[tokio::test]
async fn delivered_payloads_are_retained_within_the_declared_bound() {
    let max_messages = mgmt::state::MAX_RETAINED_MESSAGES;
    let max_bytes = mgmt::state::MAX_RETAINED_BYTES;

    for count in [1, max_messages, max_messages + 1, max_messages * 4] {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = minted("bounded");
        register(
            &engine,
            &connection_id,
            "192.0.2.9",
            "flooder/1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );

        for index in 0..count {
            let (status, body) =
                call(&engine, "POST", &connection_id, format!("m{index}").into_bytes()).await;
            assert_eq!(
                status,
                200,
                "case {count}: delivery #{index} must answer 200, got {status} {}",
                rendered(&body)
            );
        }

        let stored = mgmt::state::connection(&*engine, &connection_id).expect("still registered");
        assert_eq!(
            stored.messages.len(),
            count.min(max_messages),
            "case {count}: the retained window must hold min(count, {max_messages}), got {}",
            stored.messages.len()
        );
        assert!(
            stored.messages.iter().map(Vec::len).sum::<usize>() <= max_bytes,
            "case {count}: retained bytes must stay within {max_bytes}, got {}",
            stored.messages.iter().map(Vec::len).sum::<usize>()
        );
        // The window keeps the MOST RECENT payloads, in send order.
        let expected: Vec<Vec<u8>> = (count.saturating_sub(max_messages)..count)
            .map(|index| format!("m{index}").into_bytes())
            .collect();
        assert_eq!(
            stored.messages, expected,
            "case {count}: the window must keep the most recent payloads in order, got {:?}",
            stored
                .messages
                .iter()
                .map(|m| String::from_utf8_lossy(m).to_string())
                .collect::<Vec<_>>()
        );
        // Eviction never loses the totals.
        assert_eq!(
            stored.delivered_count, count as u64,
            "case {count}: every delivery must stay counted, got {}",
            stored.delivered_count
        );
        assert_eq!(
            stored.delivered_bytes,
            (0..count)
                .map(|index| format!("m{index}").len() as u64)
                .sum::<u64>(),
            "case {count}: delivered bytes must total every payload, got {}",
            stored.delivered_bytes
        );
    }
}

/// `DeleteConnection` removes the record (204, no body), and that removal is what makes
/// the connection gone: every operation on the id afterwards answers `GoneException`.
#[tokio::test]
async fn delete_connection_removes_the_record_and_every_later_call_is_gone() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let table = registry_table();
    register_table(&engine, &table);

    for case in &table {
        let (status, body) = call(&engine, "DELETE", &case.connection_id, Vec::new()).await;
        assert_eq!(
            status,
            204,
            "case {:?}: DeleteConnection must answer 204, got {status} {}",
            case.name,
            rendered(&body)
        );
        assert!(
            body.is_empty(),
            "case {:?}: a 204 carries no body, got {body:?}",
            case.name
        );
        assert!(
            mgmt::state::connection(&*engine, &case.connection_id).is_none(),
            "case {:?}: the record must be gone from the registry",
            case.name
        );

        // GET, POST and a second DELETE all read the same missing entry.
        for (method, payload) in [
            ("GET", Vec::new()),
            ("POST", b"hi".to_vec()),
            ("DELETE", Vec::new()),
        ] {
            let (status, headers, body) = call_with_headers(
                &engine,
                HOST,
                method,
                &format!("/@connections/{}", case.connection_id),
                payload,
            )
            .await;
            assert_eq!(
                status,
                410,
                "case {:?}: {method} after delete must answer 410, got {status} {}",
                case.name,
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                "GoneException",
                "case {:?}: {method} after delete must name GoneException, got {}",
                case.name,
                error_code(&headers)
            );
        }
    }
}

/// A `ConnectionId` that could never have been minted names no connection anywhere, so all
/// three operations refuse it as an invalid `ConnectionId` (HTTP 400 `BadRequestException`
/// naming the id) — never a 200, never a 501, never the `GoneException` that would claim
/// the id once named a live connection, and never an auto-registration that would make
/// the next `GetConnection` succeed. A WELL-FORMED id nothing minted is that
/// `GoneException`, which is the other half of the rule.
#[tokio::test]
async fn an_unknown_connection_id_is_refused_by_its_shape_for_every_operation() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());

    // One registered connection, so the failures below are about the id and not about
    // an empty registry.
    let open = minted("registered");
    register(
        &engine,
        &open,
        "192.0.2.2",
        "present/1",
        SystemTime::UNIX_EPOCH + Duration::from_secs(42),
    );

    // (the id, the status and code it must be refused with). The malformed rows span the
    // alphabet, the padding, the decoded length and length extremes — the shared catalog of
    // values API Gateway does not mint, plus the well-formed tokens that name no open
    // connection.
    let mut unknown: Vec<(String, u16, &str)> = ids_api_gateway_does_not_mint()
        .into_iter()
        .map(|(_case, id)| (id, 400u16, "BadRequestException"))
        .collect();
    unknown.extend([
        (minted("never-minted"), 410, "GoneException"),
        ("AAAAAAAAAAAAAAA=".to_string(), 410, "GoneException"),
        (minted("Registered"), 410, "GoneException"),
    ]);

    for (id, expected_status, expected_code) in unknown {
        for (method, payload) in [
            ("GET", Vec::new()),
            ("POST", b"data".to_vec()),
            ("DELETE", Vec::new()),
        ] {
            let (status, headers, body) =
                call_with_headers(&engine, HOST, method, &format!("/@connections/{id}"), payload)
                    .await;
            assert_eq!(
                status,
                expected_status,
                "case {id:?}: {method} on an id naming no open connection must answer \
                 {expected_status}, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                expected_code,
                "case {id:?}: {method} must name {expected_code}, got {}",
                error_code(&headers)
            );
        }
        assert!(
            mgmt::state::connection(&*engine, &id).is_none(),
            "case {id:?}: referencing an unknown id must not register it"
        );
    }

    // The registered connection is untouched by any of that.
    let (status, _) = call(&engine, "GET", &open, Vec::new()).await;
    assert_eq!(
        status, 200,
        "the registered connection must still answer 200, got {status}"
    );
}
