// SPDX-License-Identifier: CC0-1.0

//! Shared harness for this crate's suites: one copy of the router, the request
//! builder and the response readers.
//!
//! Declared in `tests/all.rs` like any other suite (the layout rule requires every
//! file under `tests/all/` to be declared), so the sibling suites reach it as
//! `crate::support`. It holds no tests of its own — only the pieces
//! `connection_lifecycle` and `probe` would otherwise each own a copy of, which would
//! then drift.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use engine::InMemoryEngine;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

/// The endpoint host an `execute-api` caller addresses. Its first label is also the
/// SigV4 signing name this service claims, so a request routes to it either way.
pub const HOST: &str = "execute-api.us-east-1.amazonaws.com";

/// A router serving only this service over `engine`. Built per call on purpose: every
/// router shares the one engine, so anything a later call observes came from engine
/// state and not from a handle held across calls.
fn router(engine: &Arc<InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(mgmt::EmulatedService::new(Arc::clone(engine)));
    router
}

/// A signed-looking request addressing `path` on `host`, presenting `access_key_id` as its
/// SigV4 credential.
fn request(
    access_key_id: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> http::Request<Bytes> {
    http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/execute-api/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(body))
        .expect("the test request is well formed")
}

/// The access key id a request presents unless a case names another: one this emulator holds
/// no record for, which is the position the account's own published credential is in.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// Dispatch one request through the production router and return its status and body.
pub async fn call(
    engine: &Arc<InMemoryEngine>,
    method: &str,
    connection_id: &str,
    body: Vec<u8>,
) -> (u16, Bytes) {
    call_at(engine, HOST, method, &format!("/@connections/{connection_id}"), body).await
}

/// Dispatch one request at an arbitrary endpoint host and request path — what a caller
/// addressing the owning API's invoke URL sends, where the path carries that URL's own
/// prefix (its stage, or a custom domain's base-path mapping) in front of the modelled
/// `/@connections/{ConnectionId}` binding.
pub async fn call_at(
    engine: &Arc<InMemoryEngine>,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, Bytes) {
    let (status, _headers, body) = call_with_headers(engine, host, method, path, body).await;
    (status, body)
}

/// As [`call_at`], and also the RESPONSE HEADERS.
///
/// A restJson1 client reads two of them off a failure before it looks at the body: the
/// `content-type` it decodes the envelope with, and the `x-amzn-errortype` it resolves
/// the error SHAPE from. They are part of the answer, so a suite that pins the envelope
/// needs them — see `connection_error_envelope`.
pub async fn call_with_headers(
    engine: &Arc<InMemoryEngine>,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, http::HeaderMap, Bytes) {
    call_signed_by(engine, DEFAULT_ACCESS_KEY_ID, host, method, path, body).await
}

/// As [`call_with_headers`], presenting `access_key_id` as the request's SigV4 credential.
///
/// WHICH credential a call presents is part of the answer: every `@connections` call is
/// authorized against the principal that credential speaks for, so a suite about that
/// decision signs with the key of the session or user it seeded.
pub async fn call_signed_by(
    engine: &Arc<InMemoryEngine>,
    access_key_id: &str,
    host: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> (u16, http::HeaderMap, Bytes) {
    let response = router(engine)
        .dispatch(request(access_key_id, host, method, path, body))
        .await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (status, headers, response.into_body())
}

/// The error SHAPE a restJson1 failure names, read where a restJson1 client reads it: the
/// `x-amzn-errortype` response header.
///
/// NOT the body. A restJson1 error body carries the error shape's own modelled members and
/// nothing else — no shape discriminator — so the shape travels in this header alone (the
/// service's `rest_json_error_body` rule), and a memberless shape's body is `{}`.
pub fn error_code(headers: &http::HeaderMap) -> String {
    headers
        .get("x-amzn-errortype")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
        .unwrap_or_else(|| "<no x-amzn-errortype>".to_string())
}

/// The `message` member an error body carries, or `"<no message>"`.
pub fn error_message(body: &Bytes) -> String {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A response body rendered for an assertion message.
pub fn rendered(body: &Bytes) -> String {
    String::from_utf8_lossy(body).to_string()
}

/// Connection-id forms of the shape API Gateway MINTS: the standard-alphabet base64,
/// canonically padded, of an 11-byte token — the 16-character single-padded form its own
/// ids have (`mgmt::state::CONNECTION_ID_BYTES`). Named once here so every suite drives the
/// same catalog; `connections_error_layer` asserts each one IS well formed.
///
/// None of these carries `+` or `/`, so a suite can write one into a request path as it
/// stands; the percent-encoded form is driven by `connections_error_layer`.
pub const MINTED_IDS: [(&str, &str); 6] = [
    ("the padded base64 form AWS mints", "Lx9ZKcvPmjQCJhA="),
    ("an all-A token", "AAAAAAAAAAAAAAA="),
    ("an all-z token", "zzzzzzzzzzzzzz8="),
    ("a token of digits and letters", "0123456789abcQ4="),
    ("an upper-case token", "ZZZZZZZZZZZZZZw="),
    ("the form the recorded calls use", "Fabc1cWIPHcCJ7A="),
];

/// How long an "overlong" malformed id is: far past the 16 characters a minted id has, so no
/// decoding of it can produce an 11-byte token.
const OVERLONG_ID_CHARS: usize = 200;

/// `ConnectionId` values that are NOT the form API Gateway mints, each with the way it
/// differs: too short, outside the base64 alphabet, unpadded, a base64url spelling, a token
/// of the wrong length, and an overlong value.
pub fn ids_api_gateway_does_not_mint() -> Vec<(String, String)> {
    [
        ("a single character", "x".to_string()),
        ("characters outside the base64 alphabet", "cid-with-dashes".to_string()),
        ("unpadded base64", "0123456789abcdef".to_string()),
        ("a base64url spelling", "a-b_c9DEF0123A4=".to_string()),
        ("base64 of a shorter token", "AAAAAAAAAAA=".to_string()),
        ("base64 of a longer token", "AAAAAAAAAAAAAAAAAAAA".to_string()),
        ("an overlong id", "A".repeat(OVERLONG_ID_CHARS)),
        ("the empty-looking padding alone", "====".to_string()),
    ]
    .into_iter()
    .map(|(case, id)| (case.to_string(), id))
    .collect()
}

/// A stored instant rendered the way the modelled `__timestampIso8601`
/// (`@timestampFormat("date-time")`) members are serialized on the wire.
pub fn as_wire_timestamp(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("a stored instant renders as an ISO 8601 date-time")
}

/// Register a connection through the emulator-internal seam — the only way a
/// connection comes to exist. Panics if the seam declines, since every caller here
/// registers a fresh id.
pub fn register(
    engine: &Arc<InMemoryEngine>,
    connection_id: &str,
    source_ip: &str,
    user_agent: &str,
    connected_at: SystemTime,
) {
    assert!(
        mgmt::state::register_connection(
            &**engine,
            connection_id,
            mgmt::state::Identity {
                source_ip: source_ip.to_string(),
                user_agent: user_agent.to_string(),
            },
            connected_at,
        ),
        "the registration seam must register {connection_id}"
    );
}

/// A `ConnectionId` of the shape API Gateway mints — standard base64 of
/// `mgmt::state::CONNECTION_ID_BYTES` bytes, which is the 16-character single-padded token
/// its endpoints hand out — carrying `label`, so each case addresses a connection of its own
/// and nothing here spells a token out by hand.
///
/// `label` is folded into exactly those bytes: a shorter one is padded, a longer one
/// truncated, so every answer is a well-formed id whatever the case called itself.
pub fn minted(label: &str) -> String {
    use base64::Engine as _;

    let mut bytes = label.as_bytes().to_vec();
    bytes.resize(mgmt::state::CONNECTION_ID_BYTES, b'-');
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
