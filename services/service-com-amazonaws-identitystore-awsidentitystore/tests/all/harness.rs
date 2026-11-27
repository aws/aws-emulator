// SPDX-License-Identifier: CC0-1.0

//! The front door this crate's suites drive, in ONE place.
//!
//! Every case goes through the same door a client uses: `POST /` with
//! `X-Amz-Target: AWSIdentityStore.<Operation>` and an `awsJson1_1` body
//! (OPERATIONS.md, "Wire protocol"), dispatched by the production
//! [`MultiServiceRouter`]. Nothing here reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from OPERATIONS.md, "Wire protocol".
const TARGET_PREFIX: &str = "AWSIdentityStore";

/// The identity store this emulator ADVERTISES: the store IAM Identity Center
/// provisioned for the instance `sso-admin` seeds, which is what `ListInstances` points a
/// client at (`identitystore::state::SEED_IDENTITY_STORE_ID`). Most suites address this
/// one. This API creates no identity store, so WHICH stores exist is Identity Center's
/// answer: in a router with no Identity Center request behind it — as here — the account's
/// enablement is undecided and every well-formed `IdentityStoreId` of this account names
/// the store its caller has and is served (see
/// [`crate::caller_named_identity_stores`]).
pub const STORE: &str = identitystore::state::SEED_IDENTITY_STORE_ID;

/// An `IdentityStoreId` the model admits that names NO identity store of this account, so
/// a call addressed to it is refused rather than served, whatever Identity Center has
/// decided.
///
/// It is an ARN, because the ARN form is what carries the OWNING ACCOUNT: this one names
/// a store of ANOTHER account (`210987654321`, which is not
/// `engine::emulator_account_id()`), which is not a store this emulator holds however its
/// id ends. A bare `d-…` id is refused only once Identity Center has answered which
/// stores the account has, which these suites' router never asks.
pub const MISSING_STORE: &str =
    "arn:aws:identitystore::210987654321:identitystore/d-9999999999";

/// The access key id a call is signed with when a case does not care which caller
/// makes it.
pub const DEFAULT_AKID: &str = "AKIDIDENTITYSTORE";

/// The HTTP status this service answers `ResourceNotFoundException` with.
///
/// Named once here because it is a property of the ERROR CLASS and not of any one
/// operation: every suite that expects a resource this service does not hold expects
/// this status beside the not-found error code
/// (`identitystore::service::error_status`).
pub const NOT_FOUND: u16 = 400;

/// A router with just this service mounted on a fresh engine.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the request still carries a SigV4
    // credential scope, which is what routes it to this service and names its caller.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call made by the caller whose access key id is `akid`: the HTTP
/// status, the wire error code and the parsed body.
pub async fn call_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("host", "identitystore.us-east-1.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1\
                 /identitystore/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    let code = if header_code.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    (status, code, parsed)
}

/// [`call_as`] for the default caller.
pub async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    call_as(router, DEFAULT_AKID, operation, body).await
}

/// A call by `akid` that must answer 200, returning its body.
pub async fn ok_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, code, parsed) = call_as(router, akid, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {code} with {parsed}"
    );
    parsed
}

/// A call that must answer 200, returning its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    ok_as(router, DEFAULT_AKID, operation, body, case).await
}

/// A call by `akid` that must be refused with `status` and an error code naming `error`.
pub async fn refused_as(
    router: &MultiServiceRouter,
    akid: &str,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    let (actual_status, code, parsed) = call_as(router, akid, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected {operation} to answer {status} {error}, \
         got {actual_status} {code} with {parsed}"
    );
    assert!(
        code.ends_with(error),
        "case {case:?}: expected {operation} to answer {error}, got {code:?} with {parsed}"
    );
    parsed
}

/// A call that must be refused with `status` and an error code naming `error`.
pub async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    refused_as(router, DEFAULT_AKID, operation, body, status, error, case).await
}

/// A required string member of a response body.
pub fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The upper bound of the modelled `ResourceRevision` length: len 1..64, pattern
/// `^[0-9]+$` (`smithy_query(query="shape",shape="ResourceRevision")`). A value of that
/// many digits is well-formed, so a request carrying one reaches the handler instead of
/// being refused at the boundary.
pub const REVISION_MAX_LEN: usize = 64;

/// How one row of a `Revision`-precondition table spells the value it sends, given the
/// resource's current revision.
///
/// `Revision` is the caller's expected current revision, and the modelled
/// `ResourceRevision` is a DECIMAL STRING, so the value names the NUMBER it spells:
/// every spelling of the current revision — padded or not — is that revision, and no
/// other number is. One table, shared by the user and the group suites and by the
/// updates and the deletes, so the four operations that take the member are held to one
/// rule.
#[derive(Debug, Clone, Copy)]
pub enum Precondition {
    /// No `Revision` member at all: the call applies regardless of the revision.
    Absent,
    /// The current revision, spelled as the response reported it.
    Current,
    /// The current revision with one leading zero.
    CurrentWithALeadingZero,
    /// The current revision zero-padded to the longest value the length admits.
    CurrentPaddedToMaxLength,
    /// The revision the resource carried before its last update.
    Stale,
    /// One past the current revision: a revision it has not reached.
    OnePast,
    /// `0`, which no stored resource carries — a create is revision 1.
    Zero,
    /// `0` with leading zeros: the same number, still no resource's revision.
    ZeroPadded,
    /// The longest value the length admits, far past any revision a store reaches.
    LongestAllowed,
}

impl Precondition {
    /// The member this row sends when the resource is at revision `current`, or `None`
    /// for the row that sends none.
    pub fn member(self, current: u64) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Current => Some(current.to_string()),
            Self::CurrentWithALeadingZero => Some(format!("0{current}")),
            Self::CurrentPaddedToMaxLength => {
                Some(format!("{current:0>width$}", width = REVISION_MAX_LEN))
            }
            Self::Stale => Some(current.saturating_sub(1).to_string()),
            Self::OnePast => Some((current + 1).to_string()),
            Self::Zero => Some("0".to_string()),
            Self::ZeroPadded => Some("0".repeat(8)),
            Self::LongestAllowed => Some("9".repeat(REVISION_MAX_LEN)),
        }
    }

    /// Whether the value this row sends names the resource's current revision, and so
    /// lets the call through.
    pub fn holds(self) -> bool {
        matches!(
            self,
            Self::Absent
                | Self::Current
                | Self::CurrentWithALeadingZero
                | Self::CurrentPaddedToMaxLength
        )
    }
}

/// The rows a `Revision`-precondition table walks: four values that name the current
/// revision (one of them by naming no revision at all) and five that do not, spanning
/// the spellings the modelled pattern admits and both published length bounds.
///
/// A row is sent against a resource that has been updated once, so `Stale` has a
/// revision to name.
pub const PRECONDITION_ROWS: [(&str, Precondition); 9] = [
    ("no revision at all", Precondition::Absent),
    ("the current revision", Precondition::Current),
    (
        "the current revision with a leading zero",
        Precondition::CurrentWithALeadingZero,
    ),
    (
        "the current revision padded to the longest value allowed",
        Precondition::CurrentPaddedToMaxLength,
    ),
    (
        "the revision before the last update",
        Precondition::Stale,
    ),
    ("one revision past the current", Precondition::OnePast),
    ("revision zero", Precondition::Zero),
    ("revision zero, zero-padded", Precondition::ZeroPadded),
    (
        "the longest revision allowed",
        Precondition::LongestAllowed,
    ),
];

/// The `Message` of an error body, whichever casing the protocol rendered it in.
pub fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}
