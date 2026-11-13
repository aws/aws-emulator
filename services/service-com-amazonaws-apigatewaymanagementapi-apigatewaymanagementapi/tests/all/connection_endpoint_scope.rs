// SPDX-License-Identifier: CC0-1.0

//! WHICH connections a `@connections` request addresses: the ones established at the
//! endpoint it reached.
//!
//! The rule these cases assert, one level above the recorded case (one live connection of
//! one api, addressed at another api's endpoint):
//!
//! > A connection belongs to the api whose data-plane ENDPOINT minted it, and AWS publishes
//! > these three operations on that same endpoint
//! > (`https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`), so a
//! > request that reached one api's endpoint addresses only the connections established
//! > there: another api's LIVE connection id is the modelled `GoneException` (HTTP 410) on
//! > it, for every one of the three operations, and it is served at its own endpoint
//! > unchanged. An address that is no api's endpoint addresses the registry as a whole.
//!
//! WHY it matters: a backend holding ids from two apis would otherwise push one api's
//! message onto another api's socket, and a developer reading `GetConnection` at the wrong
//! endpoint would be told a connection exists where it cannot be reached.
//!
//! The rows span what the rule generalises over: which endpoint the request reached (each
//! of two apis' own, and one that is no api's), which connection it names (each api's own
//! live one), and the operation (all three).
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call_with_headers, error_code, minted, HOST};

/// The two addresses that ARE api endpoints in these cases, as a core-owned listener
/// reports one (`host:port`), and one address that is no api's endpoint at all.
const FIRST_ENDPOINT: &str = "127.0.0.1:34567";
const SECOND_ENDPOINT: &str = "127.0.0.1:45678";

/// The stage path an endpoint's caller carries in front of the modelled binding, as the
/// documented `@connections` URL does.
const STAGE_PREFIX: &str = "/prod";

/// The three operations, by the method each is reached with, and the payload each sends.
fn operations() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("GET", Vec::new()),
        ("POST", b"push".to_vec()),
        ("DELETE", Vec::new()),
    ]
}

/// A registration instant in the past, so an instant the service writes later is strictly
/// greater.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// An engine holding one live connection at each of the two endpoints, registered the way
/// the data plane that owns the socket registers one: through the registry handle, with the
/// endpoint its api is reached at.
fn engine_with_two_endpoints() -> (Arc<InMemoryEngine>, String, String) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let registry = mgmt::state::registry(&*engine).expect("the engine carries the registry");
    let first = minted("first-conn");
    let second = minted("secondconn");
    for (endpoint, connection_id) in [(FIRST_ENDPOINT, &first), (SECOND_ENDPOINT, &second)] {
        mgmt::state::serve_connections_at(&registry, endpoint);
        assert!(
            mgmt::state::open_connection(
                &registry,
                connection_id.clone(),
                mgmt::state::Identity {
                    source_ip: "127.0.0.1".to_string(),
                    user_agent: "scope/1".to_string(),
                },
                connected_at(),
                Some(endpoint.to_string()),
                None,
            ),
            "the connection {connection_id} must be registered at {endpoint}"
        );
    }
    (engine, first, second)
}

#[tokio::test]
async fn a_connection_is_addressed_only_at_the_endpoint_it_was_established_at() {
    // (the endpoint the request reaches, the connection it names, whether that endpoint
    // serves it)
    let rows: [(&str, bool, bool); 6] = [
        (FIRST_ENDPOINT, true, true),
        (FIRST_ENDPOINT, false, false),
        (SECOND_ENDPOINT, false, true),
        (SECOND_ENDPOINT, true, false),
        // An address that is no api's endpoint addresses the registry as a whole, which is
        // how the emulator's own service endpoint reaches either connection.
        (HOST, true, true),
        (HOST, false, true),
    ];

    for (endpoint, first_connection, served) in rows {
        for (method, payload) in operations() {
            let (engine, first, second) = engine_with_two_endpoints();
            let connection_id = if first_connection { &first } else { &second };
            let case = format!(
                "{method} on {} at {endpoint}",
                if first_connection {
                    "the first api's connection"
                } else {
                    "the second api's connection"
                }
            );

            let path = format!("{STAGE_PREFIX}/@connections/{connection_id}");
            let (status, headers, body) =
                call_with_headers(&engine, endpoint, method, &path, payload).await;
            if served {
                let expected = if method == "DELETE" { 204 } else { 200 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: the endpoint serves this connection, so expected HTTP \
                     {expected}, got {status} {body:?}"
                );
            } else {
                assert_eq!(
                    status, 410,
                    "case {case:?}: another endpoint's connection must answer HTTP 410, \
                     got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&headers),
                    "GoneException",
                    "case {case:?}: expected GoneException, got {}",
                    error_code(&headers)
                );
            }

            // Whatever the verdict, the OTHER endpoint's record is untouched: a refusal
            // deletes nothing and delivers nothing.
            let other = if first_connection { &second } else { &first };
            let stored = mgmt::state::connection(&*engine, other)
                .unwrap_or_else(|| panic!("case {case:?}: the other connection must still exist"));
            assert_eq!(
                stored.delivered_count, 0,
                "case {case:?}: nothing must be delivered to the other connection, got {} \
                 deliveries",
                stored.delivered_count
            );
        }
    }
}
