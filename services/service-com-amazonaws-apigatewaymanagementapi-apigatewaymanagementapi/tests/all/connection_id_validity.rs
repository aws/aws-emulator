// SPDX-License-Identifier: CC0-1.0

//! How the three `@connections` operations read the `ConnectionId` a request sent, driven
//! over HTTP through the same `MultiServiceRouter` the production emulator serves with.
//!
//! The rule these cases assert, one level above the recorded case (one well-formed token
//! nothing had minted, and four malformed ones):
//!
//! > A connection id is not an arbitrary string: API Gateway MINTS it when a client's
//! > handshake completes — standard base64 of `CONNECTION_ID_BYTES` bytes, the
//! > 16-character single-padded token — and the three `@connections` operations address a
//! > connection BY it. So the answer is decided by the id's SHAPE first: a value that could
//! > never have been minted is refused, from the value alone and before the registry is
//! > consulted, with HTTP 400 `BadRequestException` and the message `Invalid connectionId: `
//! > plus the `ConnectionId` the request sent; a value of that shape naming no open
//! > connection is the modelled `GoneException` (HTTP 410, memberless, so it carries no
//! > message at all) — whether it was never minted or minted and since closed.
//!
//! WHY the two are not interchangeable: a caller that pushes to a connection prunes its
//! own record of that connection on the 410, which is the right thing to do for a socket
//! that has closed and the wrong thing to do for an id it mis-spelled — answering 410 there
//! makes it delete a record it should keep, and a 410 carries no message (the modelled
//! shape has no members), so nothing would say what was wrong with the request.
//!
//! The rows span what the rule generalises over: the FORM of the `ConnectionId` (the padded
//! base64 shape AWS mints, an all-`A` one of that same shape, and — malformed — an unpadded
//! token, a dashed one, a single character, a very long one, text that is not base64 at all,
//! base64 of the wrong number of bytes), the operation (all three), and the connection id's
//! history (never minted, open, closed).
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{
    call, call_with_headers, error_code, error_message, ids_api_gateway_does_not_mint, minted,
    rendered, register, HOST, MINTED_IDS,
};

/// The status and code API Gateway refuses a `ConnectionId` that could never have been
/// minted with, and the fixed text of its message.
const INVALID_STATUS: u16 = 400;
const INVALID_TYPE: &str = "BadRequestException";
const INVALID_MESSAGE: &str = "Invalid connectionId: ";

/// The modelled refusal for a well-formed id that names no open connection
/// (`@httpError(410)`).
const GONE_STATUS: u16 = 410;
const GONE_TYPE: &str = "GoneException";

/// `ConnectionId` forms that ARE of the shape API Gateway mints: standard base64 of
/// `mgmt::state::CONNECTION_ID_BYTES` bytes.
fn well_formed_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the padded base64 shape AWS mints", minted("validity-1")),
        ("another token of that same shape", minted("validity-2")),
        (
            "an all-A id of that same shape",
            "AAAAAAAAAAAAAAA=".to_string(),
        ),
    ]
}

/// `ConnectionId` forms that could never have been minted, and so are refused from the
/// value alone: the form, the token as the request SENDS it (percent-encoded where a path
/// segment has to be), and the value the service reads back out of the path.
///
/// The rows span the ways a value fails the shape: the alphabet, the padding, the decoded
/// length (too few bytes, too many), length extremes, and a token whose characters a path
/// segment carries encoded.
const MALFORMED_IDS: [(&str, &str, &str); 8] = [
    ("a single character", "x", "x"),
    (
        "text that is not base64, percent-encoded as a path segment carries it",
        "not%20base64%21%21",
        "not base64!!",
    ),
    ("an unpadded token", "Fabc1cWIPHcCJ7A", "Fabc1cWIPHcCJ7A"),
    ("a dashed id", "cid-with-dashes", "cid-with-dashes"),
    (
        "a long underscore-separated id",
        "ZZZ_0000000000000000000000000000",
        "ZZZ_0000000000000000000000000000",
    ),
    (
        "an id of base64url characters",
        "a-b_c9DEF0123456",
        "a-b_c9DEF0123456",
    ),
    ("base64 of too few bytes", "jR4tQ%3D%3D", "jR4tQ=="),
    (
        "base64 of too many bytes",
        "0123456789abcdef",
        "0123456789abcdef",
    ),
];

/// A 200-character token, the longest form the recorded cases send. Built rather than
/// spelled out, so the length is readable.
fn very_long_id() -> String {
    "A".repeat(200)
}

/// The three operations, by the method each is reached with, and the payload each sends.
fn operations() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("GET", Vec::new()),
        ("POST", b"payload".to_vec()),
        ("DELETE", Vec::new()),
    ]
}

/// A registration instant in the past, so an instant the service writes later is strictly
/// greater.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// The two shared catalogs are the forms they claim to be: every id
/// `crate::support::MINTED_IDS` holds IS the shape the service mints, and no value
/// `crate::support::ids_api_gateway_does_not_mint` holds is — so the tables the rules are
/// asserted over, here and in the sibling suites, cannot drift into each other.
#[tokio::test]
async fn the_catalogs_are_the_forms_they_claim_to_be() {
    for (case, connection_id) in MINTED_IDS {
        assert!(
            mgmt::state::is_well_formed_connection_id(connection_id),
            "case {case:?}: {connection_id:?} must be the shape API Gateway mints"
        );
    }
    for (case, connection_id) in ids_api_gateway_does_not_mint() {
        assert!(
            !mgmt::state::is_well_formed_connection_id(&connection_id),
            "case {case:?}: {connection_id:?} must NOT be the shape API Gateway mints"
        );
    }
}

#[tokio::test]
async fn a_malformed_connection_id_is_refused_as_an_invalid_connection_id() {
    let mut forms: Vec<(&str, String, String)> = MALFORMED_IDS
        .iter()
        .map(|(form, sent, read)| (*form, (*sent).to_string(), (*read).to_string()))
        .collect();
    forms.push(("a 200-character token", very_long_id(), very_long_id()));

    for (id_form, sent, read) in forms {
        for (method, payload) in operations() {
            // One OTHER connection is open, so each refusal below is about the id that
            // was asked for and not about an empty registry.
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let open = minted("open");
            register(&engine, &open, "192.0.2.1", "other/1", connected_at());

            let case = format!("{method} on {id_form} ({sent})");
            let (status, headers, body) =
                call_with_headers(&engine, HOST, method, &format!("/@connections/{sent}"), payload)
                    .await;
            assert_eq!(
                status,
                INVALID_STATUS,
                "case {case:?}: expected HTTP {INVALID_STATUS}, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                INVALID_TYPE,
                "case {case:?}: expected {INVALID_TYPE}, got {}",
                error_code(&headers)
            );
            assert_eq!(
                error_message(&body),
                format!("{INVALID_MESSAGE}{read}"),
                "case {case:?}: expected the message to name the id the request sent, got {}",
                rendered(&body)
            );

            // The refusal changed nothing: no connection was registered for the id, and
            // the open one is untouched.
            assert!(
                mgmt::state::connection(&*engine, &read).is_none(),
                "case {case:?}: a refused call must not register the id it refused"
            );
            let other = mgmt::state::connection(&*engine, &open)
                .unwrap_or_else(|| panic!("case {case:?}: the open connection must still exist"));
            assert_eq!(
                other.delivered_count, 0,
                "case {case:?}: a refused call must deliver nothing, got {} deliveries",
                other.delivered_count
            );
        }
    }
}

/// A WELL-FORMED id the registry does not hold reaches the store and is the modelled
/// `GoneException` — whether it was never registered here at all, or was registered and then
/// deleted. Nothing about this emulator's own mint log enters the decision.
#[tokio::test]
async fn a_well_formed_id_that_names_no_open_connection_is_gone() {
    for (id_form, connection_id) in well_formed_ids() {
        for (method, payload) in operations() {
            // One OTHER connection is open, so each refusal below is about the id that
            // was asked for and not about an empty registry.
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let open = minted("open");
            register(&engine, &open, "192.0.2.1", "other/1", connected_at());

            let case = format!("{method} on {id_form} ({connection_id})");
            let (status, headers, body) =
                call_with_headers(&engine, HOST, method, &path(&connection_id), payload).await;
            assert_eq!(
                status,
                GONE_STATUS,
                "case {case:?}: a well-formed id naming no open connection must answer \
                 HTTP {GONE_STATUS}, got {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                GONE_TYPE,
                "case {case:?}: expected {GONE_TYPE}, got {}",
                error_code(&headers)
            );
            assert_eq!(
                error_message(&body),
                "<no message>",
                "case {case:?}: GoneException is memberless, so it carries no message, \
                 got {}",
                rendered(&body)
            );
            assert!(
                mgmt::state::connection(&*engine, &connection_id).is_none(),
                "case {case:?}: a refused call must not register the id it refused"
            );
        }
    }
}

#[tokio::test]
async fn a_minted_id_is_served_while_its_connection_is_open_and_gone_once_it_closes() {
    for (id_form, connection_id) in well_formed_ids() {
        // Open: the id names a live connection, so each operation serves it.
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        register(
            &engine,
            &connection_id,
            "198.51.100.7",
            "sender/1",
            connected_at(),
        );

        let case = format!("{id_form} ({connection_id})");
        let (status, body) = call(&engine, "GET", &connection_id, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConnection on the open connection must answer 200, got \
             {status} {}",
            rendered(&body)
        );
        let (status, body) = call(&engine, "POST", &connection_id, b"payload".to_vec()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PostToConnection on the open connection must answer 200, got \
             {status} {}",
            rendered(&body)
        );
        let (status, body) = call(&engine, "DELETE", &connection_id, Vec::new()).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteConnection must answer the modelled 204, got {status} {}",
            rendered(&body)
        );

        // Closed: the id is still of the shape the service mints, so every later call is
        // the modelled Gone and not the invalid-id refusal.
        for (method, payload) in operations() {
            let (status, headers, body) =
                call_with_headers(&engine, HOST, method, &path(&connection_id), payload).await;
            assert_eq!(
                status,
                GONE_STATUS,
                "case {case:?}: {method} after the delete must answer {GONE_STATUS}, got \
                 {status} {}",
                rendered(&body)
            );
            assert_eq!(
                error_code(&headers),
                GONE_TYPE,
                "case {case:?}: {method} after the delete must answer {GONE_TYPE}, got {}",
                error_code(&headers)
            );
        }
    }
}

/// The modelled request path for `connection_id`.
fn path(connection_id: &str) -> String {
    format!("/@connections/{connection_id}")
}
