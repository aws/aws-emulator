// SPDX-License-Identifier: CC0-1.0

//! The `@connections` operations served through the owning API's INVOKE path.
//!
//! The rule under test: a request for one of this service's operations is addressed
//! through the owning API's invoke endpoint, whose path carries that endpoint's own
//! prefix — the deployed stage, or the base path a custom domain maps — in front of the
//! modelled `/@connections/{ConnectionId}` binding, so the operation is resolved from
//! that binding with the prefix dropped and answers exactly what the unprefixed
//! modelled path answers.
//!
//! That is the URL AWS documents for these operations
//! (`https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`, and
//! "the stage name is typically appended to the API endpoint" per the API Gateway V2
//! `Api$ApiEndpoint` doc), and the one every SDK client built from a stage endpoint
//! sends: the client is configured with the stage URL and appends the modelled URI to
//! it. Before the `crate::service` seam that strips it, such a path matched no modelled
//! URI pattern, so the request named no operation and was answered with the protocol's
//! routing failure instead of reaching a handler.
//!
//! The rows differ along what the rule generalises over: the number and spelling of the
//! prefix segments (none, one stage, a stage using each character class a stage name
//! allows, a two-segment custom-domain base path), the endpoint host, and the
//! connection-id form. Every row drives all three operations plus each failure the
//! service answers from a request, so the prefix is shown to change nothing about which
//! operation runs or what it reports.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::Value;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{
    as_wire_timestamp, call_at, call_with_headers, error_code, ids_api_gateway_does_not_mint,
    minted, register, HOST,
};

/// The `Data` maximum this service's model declares (`@length(min: 0, max: 131072)` on
/// the `Data` blob). Read from the crate constant so the two cannot disagree.
const MAX_DATA_BYTES: usize = mgmt::ops::post_to_connection::MAX_DATA_BYTES;

/// One row: an endpoint the caller addresses, and the connection it addresses there.
struct Row {
    case: &'static str,
    /// The endpoint host. Only the signing name in the credential scope decides which
    /// service claims the request, so the host varies freely across rows.
    host: &'static str,
    /// The invoke endpoint's path prefix, exactly as it appears in front of the
    /// modelled binding. Empty for the modelled path itself — the baseline row.
    prefix: &'static str,
    /// What the row's connection is called; its `ConnectionId` is the token
    /// [`crate::support::minted`] makes of it, since a connection exists only under an id
    /// of the shape the service mints.
    connection: &'static str,
    source_ip: &'static str,
    user_agent: &'static str,
    /// Seconds past the epoch the connection is registered at. In the past, so an
    /// instant the service writes later is strictly greater.
    connected_at_secs: u64,
    /// The payload a successful `PostToConnection` delivers on this row.
    payload: &'static [u8],
}

/// The table. Stage names may use letters, digits, `_` and `-`, so one row spells a
/// stage out of each of those classes; `/v1/socket` is the two-segment form a custom
/// domain's base-path mapping produces; the empty prefix is the modelled path.
///
/// Every connection's id is the token [`crate::support::minted`] makes of its name, since a
/// value of another shape is refused before any connection is looked up, whatever the
/// prefix.
const ROWS: &[Row] = &[
    Row {
        case: "the modelled path with no invoke prefix at all",
        host: HOST,
        prefix: "",
        connection: "baseline",
        source_ip: "192.0.2.1",
        user_agent: "baseline/1",
        connected_at_secs: 1_600_000_000,
        payload: b"hello",
    },
    Row {
        case: "a deployed stage in front of the modelled path",
        host: "a1b2c3d4e5.execute-api.us-east-1.amazonaws.com",
        prefix: "/prod",
        connection: "deployed",
        source_ip: "198.51.100.17",
        user_agent: "sdk-client/2.1",
        connected_at_secs: 1_700_000_000,
        payload: b"{\"action\":\"echo\"}",
    },
    Row {
        case: "a different stage, and an empty payload",
        host: "zzz999aaa0.execute-api.eu-west-1.amazonaws.com",
        prefix: "/dev",
        connection: "other-stg",
        source_ip: "2001:db8::2:1",
        user_agent: "curl/8.4.0",
        connected_at_secs: 1,
        payload: b"",
    },
    Row {
        case: "a stage spelling every character class a stage name allows",
        host: "mix3dstage.execute-api.ap-southeast-2.amazonaws.com",
        prefix: "/Stage_name-1",
        connection: "mixed-stg",
        source_ip: "203.0.113.9",
        user_agent: "agent (with spaces)",
        connected_at_secs: 0,
        payload: b"\x00\x01\x02\xff",
    },
    Row {
        case: "a two-segment base path, as a custom domain mapping serves it",
        host: "sockets.example.com",
        prefix: "/v1/socket",
        connection: "mapped",
        source_ip: "192.0.2.250",
        user_agent: "mapped-domain/0.1",
        connected_at_secs: 1_234_567_890,
        payload: &[b'p'; 4096],
    },
];

impl Row {
    fn connected_at(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.connected_at_secs)
    }

    /// This row's `ConnectionId`: a token of the shape the service mints, carrying the
    /// row's own name.
    fn connection_id(&self) -> String {
        minted(self.connection)
    }

    /// The request path this row's caller sends for `connection_id`.
    fn path(&self, connection_id: &str) -> String {
        format!("{}/@connections/{connection_id}", self.prefix)
    }
}

/// Parse a response body as JSON, naming the row when it is not.
fn json(case: &str, body: &Bytes) -> Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("case {case:?}: expected a JSON body ({e}), got {body:?}"))
}

/// A fresh engine with this row's connection registered through the one seam that makes
/// a connection exist.
fn engine_with(row: &Row) -> Arc<InMemoryEngine> {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    register(
        &engine,
        &row.connection_id(),
        row.source_ip,
        row.user_agent,
        row.connected_at(),
    );
    engine
}

#[tokio::test]
async fn get_connection_reports_the_registry_through_the_invoke_path() {
    for row in ROWS {
        let engine = engine_with(row);
        let connection_id = row.connection_id();
        let (status, body) = call_at(
            &engine,
            row.host,
            "GET",
            &row.path(&connection_id),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetConnection at {:?} must answer 200, got {status} {body:?}",
            row.case,
            row.path(&connection_id)
        );

        let value = json(row.case, &body);
        let expected_connected_at = as_wire_timestamp(row.connected_at());
        for member in ["connectedAt", "lastActiveAt"] {
            let actual = value.get(member).and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(expected_connected_at.as_str()),
                "case {:?}: {member} must be the registration instant \
                 {expected_connected_at:?}, got {actual:?}",
                row.case
            );
        }
        let source_ip = value.pointer("/identity/sourceIp").and_then(Value::as_str);
        assert_eq!(
            source_ip,
            Some(row.source_ip),
            "case {:?}: expected identity.sourceIp {:?}, got {source_ip:?}",
            row.case,
            row.source_ip
        );
        let user_agent = value.pointer("/identity/userAgent").and_then(Value::as_str);
        assert_eq!(
            user_agent,
            Some(row.user_agent),
            "case {:?}: expected identity.userAgent {:?}, got {user_agent:?}",
            row.case,
            row.user_agent
        );
    }
}

#[tokio::test]
async fn post_to_connection_delivers_through_the_invoke_path_and_advances_last_active_at() {
    for row in ROWS {
        let engine = engine_with(row);
        let connection_id = row.connection_id();
        let (status, body) = call_at(
            &engine,
            row.host,
            "POST",
            &row.path(&connection_id),
            row.payload.to_vec(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: PostToConnection at {:?} must answer 200, got {status} {body:?}",
            row.case,
            row.path(&connection_id)
        );
        assert!(
            body.is_empty(),
            "case {:?}: the success response carries no body, got {body:?}",
            row.case
        );

        // Delivered to the ONE record the registry holds for this id, read back through
        // the emulator-internal read seam.
        let stored = mgmt::state::connection(&*engine, &connection_id)
            .unwrap_or_else(|| panic!("case {:?}: the connection must still exist", row.case));
        assert_eq!(
            stored.messages.as_slice(),
            &[row.payload.to_vec()],
            "case {:?}: expected exactly the delivered payload, got {:?}",
            row.case,
            stored.messages
        );
        assert_eq!(
            stored.delivered_count, 1,
            "case {:?}: expected one delivery counted, got {}",
            row.case, stored.delivered_count
        );
        assert!(
            stored.last_active_at > stored.connected_at,
            "case {:?}: delivery must move LastActiveAt past ConnectedAt, got {:?} and {:?}",
            row.case,
            stored.last_active_at,
            stored.connected_at
        );

        // And the report of it comes back through the same prefixed path.
        let (status, body) = call_at(
            &engine,
            row.host,
            "GET",
            &row.path(&connection_id),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetConnection after the post must answer 200, got {status} {body:?}",
            row.case
        );
        let value = json(row.case, &body);
        let reported = value.get("lastActiveAt").and_then(Value::as_str);
        let expected = as_wire_timestamp(stored.last_active_at);
        assert_eq!(
            reported,
            Some(expected.as_str()),
            "case {:?}: expected the stored LastActiveAt {expected:?}, got {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn delete_connection_through_the_invoke_path_removes_the_connection() {
    for row in ROWS {
        let engine = engine_with(row);
        let connection_id = row.connection_id();
        let (status, body) = call_at(
            &engine,
            row.host,
            "DELETE",
            &row.path(&connection_id),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {:?}: DeleteConnection at {:?} must answer 204, got {status} {body:?}",
            row.case,
            row.path(&connection_id)
        );
        assert!(
            body.is_empty(),
            "case {:?}: a 204 carries no body, got {body:?}",
            row.case
        );
        assert!(
            mgmt::state::connection(&*engine, &connection_id).is_none(),
            "case {:?}: the deleted connection must be gone from the registry",
            row.case
        );

        // Every operation now answers the modelled Gone for that id, prefix and all.
        for (method, payload) in [
            ("GET", Vec::new()),
            ("POST", b"after the delete".to_vec()),
            ("DELETE", Vec::new()),
        ] {
            let (status, headers, body) = call_with_headers(
                &engine,
                row.host,
                method,
                &row.path(&connection_id),
                payload,
            )
            .await;
            assert_eq!(
                status, 410,
                "case {:?}: {method} after the delete must answer 410, got {status} {body:?}",
                row.case
            );
            assert_eq!(
                error_code(&headers),
                "GoneException",
                "case {:?}: {method} after the delete must answer GoneException, got {}",
                row.case,
                error_code(&headers)
            );
        }
    }
}

#[tokio::test]
async fn the_modelled_failures_answer_the_same_under_an_invoke_prefix() {
    for row in ROWS {
        let engine = engine_with(row);
        let connection_id = row.connection_id();

        // An id that could never have been minted is refused as an invalid
        // `ConnectionId`, whatever the prefix. The value comes from the shared catalog of
        // forms API Gateway does not mint.
        let (another_form, malformed_id) = ids_api_gateway_does_not_mint()
            .into_iter()
            .next()
            .expect("the catalog of values API Gateway does not mint is not empty");
        let (status, headers, body) = call_with_headers(
            &engine,
            row.host,
            "GET",
            &row.path(&malformed_id),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: {another_form} ({malformed_id}) must answer 400, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&headers),
            "BadRequestException",
            "case {:?}: {another_form} must answer BadRequestException, got {}",
            row.case,
            error_code(&headers)
        );

        // A well-formed id naming no open connection is the modelled Gone, whatever the
        // prefix.
        let (status, headers, body) = call_with_headers(
            &engine,
            row.host,
            "GET",
            &row.path(&minted("not-minted")),
            Vec::new(),
        )
        .await;
        assert_eq!(
            status, 410,
            "case {:?}: a well-formed unknown id must answer 410, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&headers),
            "GoneException",
            "case {:?}: a well-formed unknown id must answer GoneException, got {}",
            row.case,
            error_code(&headers)
        );

        // A payload one byte past the modelled maximum is the endpoint's own 413, judged
        // before the connection is resolved — so the prefix cannot change it either.
        let (status, _headers, body) = call_with_headers(
            &engine,
            row.host,
            "POST",
            &row.path(&connection_id),
            vec![b'x'; MAX_DATA_BYTES + 1],
        )
        .await;
        assert_eq!(
            status, 413,
            "case {:?}: a payload past the maximum must answer 413, got {status} {body:?}",
            row.case
        );
        // The endpoint's own refusal carries no error envelope at all, which
        // `connection_error_envelope` pins; what matters here is that the prefix does not
        // change it.
        let stored = mgmt::state::connection(&*engine, &connection_id)
            .unwrap_or_else(|| panic!("case {:?}: the connection must still exist", row.case));
        assert_eq!(
            stored.delivered_count, 0,
            "case {:?}: a refused payload must not be delivered, got {} deliveries",
            row.case, stored.delivered_count
        );
    }
}

/// The seam resolves the MODELLED binding out of an invoke path; it does not invent an
/// operation for a path that does not carry that binding. Each row here is a near miss
/// on one of its parts, with the refusal it must get: a path this service's modelled
/// path space does not claim at all is the router's `UnknownService` (404), while
/// `/{stage}/@connections/` — claimed, since it carries the segment, but naming no
/// modelled operation once the prefix is dropped (the `ConnectionId` label cannot be
/// empty) — is the restJson1 routing failure this service renders.
#[tokio::test]
async fn a_path_that_does_not_carry_the_modelled_binding_is_not_served() {
    /// What the router answers a request no service claims: no claimant, so no
    /// service's own envelope.
    const UNKNOWN_SERVICE: u16 = 404;
    /// What this service answers a request it claims that names no modelled operation
    /// (the protocol-correct default for a `rest` service).
    const NO_SUCH_OPERATION: u16 = 501;

    let near_misses: [(&str, &str, u16); 6] = [
        (
            "the collection without an id",
            "/prod/@connections",
            UNKNOWN_SERVICE,
        ),
        (
            "an empty connection id",
            "/prod/@connections/",
            NO_SUCH_OPERATION,
        ),
        (
            "the segment misspelled",
            "/prod/connections/abc123",
            UNKNOWN_SERVICE,
        ),
        (
            "the segment singular",
            "/prod/@connection/abc123",
            UNKNOWN_SERVICE,
        ),
        (
            "the at-sign dropped",
            "/prod/connections/abc123/",
            UNKNOWN_SERVICE,
        ),
        (
            "another api's path space",
            "/prod/v2/apis/abc123",
            UNKNOWN_SERVICE,
        ),
    ];

    for (case, path, expected) in near_misses {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        register(
            &engine,
            &minted("near-miss"),
            "192.0.2.1",
            "near-miss/1",
            SystemTime::UNIX_EPOCH,
        );

        for method in ["GET", "POST", "DELETE"] {
            let (status, body) = call_at(&engine, HOST, method, path, Vec::new()).await;
            assert_eq!(
                status, expected,
                "case {case:?}: {method} {path} names no modelled operation, so it must be \
                 refused with {expected}, got {status} {body:?}"
            );
        }
    }
}
