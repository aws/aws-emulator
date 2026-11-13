// SPDX-License-Identifier: CC0-1.0

//! The S3 Express session token a Zonal endpoint request presents, and what it authorizes.
//!
//! A client that has taken a session on a directory bucket signs its Zonal endpoint calls
//! with the session's key pair and "appl[ies] the session token to the
//! `x-amz-s3session-token` request header for authorization" (`CreateSession`'s own
//! documentation). That token is not a SigV4 credential and is not what the signature is
//! computed from: it says WHICH session the caller is acting under, and "temporary security
//! credentials are scoped to the bucket and expire after 5 minutes" — so what a presented
//! token authorizes is the bucket its own session was created for, for as long as that
//! session lives.
//!
//! This module is where that one rule lives, applied to EVERY request that presents such a
//! token rather than per operation: the token is read off the request, looked up among the
//! sessions `CreateSession` vended ([`crate::state::ComAmazonawsS3Amazons3State`]), and the
//! request is refused unless the session it names is live and covers the bucket the request
//! addresses. A request that presents NO session token presents no session, so this rule has
//! nothing to say about it — such a request is authorized by what the bucket itself grants,
//! the way every other request of this service is.
//!
//! The refusal is served as an OPERATION
//! ([`super::EmulatedService::override_operation`] routes to it) because that is the one
//! seam a service can answer a request from before the operation it addressed runs; the
//! framework has already verified the request's SIGNATURE by then, so a refusal here is
//! only ever about the session, never about the signing.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state::SessionTokenVerdict;

/// The header a Zonal endpoint request carries its session token in: "you also apply the
/// session token to the `x-amz-s3session-token` request header for authorization"
/// (`CreateSession`'s own documentation).
///
/// The name itself lives beside the sessions in [`crate::state`], so the routing seam here
/// and a handler reading its own request's session name the one header.
pub const SESSION_TOKEN_HEADER: &str = crate::state::SESSION_TOKEN_HEADER;

/// The key this service serves the refusal of an unauthorized session under, one per
/// condition ([`SessionRefusal`]).
///
/// Each carries the `@` sigil the sibling door key carries
/// ([`super::invalid_bucket_name::REFUSE_INVALID_BUCKET_NAME`]), which is a character no
/// modelled operation's name holds, so the "no routing key the model's protocol can resolve
/// reaches these" claim is structural: nothing but
/// [`super::EmulatedService::override_operation`] ever looks one up. One key per condition
/// because the operation behind it answers one fixed document, and the seam that decides
/// WHICH condition refused the request can only hand back a borrow of an operation the
/// service already holds.
pub const SESSION_DENIED_KEYS: [(&str, SessionRefusal); 4] = [
    (REFUSE_INVALID_TOKEN, INVALID_TOKEN),
    (REFUSE_EXPIRED_TOKEN, EXPIRED_TOKEN),
    (REFUSE_OTHER_BUCKET, OTHER_BUCKET),
    (REFUSE_READ_ONLY_SESSION, READ_ONLY_SESSION),
];

/// See [`SESSION_DENIED_KEYS`].
const REFUSE_INVALID_TOKEN: &str = "@refuse-session-invalid-token";
/// See [`SESSION_DENIED_KEYS`].
const REFUSE_EXPIRED_TOKEN: &str = "@refuse-session-expired-token";
/// See [`SESSION_DENIED_KEYS`].
const REFUSE_OTHER_BUCKET: &str = "@refuse-session-other-bucket";
/// The key the refusal of a WRITE presented with a `ReadOnly` session is served under — see
/// [`SESSION_DENIED_KEYS`] and [`READ_ONLY_SESSION_OPERATIONS`].
pub const REFUSE_READ_ONLY_SESSION: &str = "@refuse-session-read-only";

/// The Zonal endpoint API operations a `ReadOnly` session executes: "a `ReadOnly` session is
/// constrained to execute the following Zonal endpoint API operations: `GetObject`,
/// `HeadObject`, `ListObjectsV2`, `GetObjectAttributes`, `ListParts`, and
/// `ListMultipartUploads`" (`CreateSessionRequest$SessionMode`).
///
/// `CreateSession` itself is in the set because it is not executed UNDER a session at all:
/// "you use IAM credentials to make the CreateSession API request on the bucket" (that
/// operation's own documentation), so taking a new session is never the thing a session's own
/// mode constrains.
pub const READ_ONLY_SESSION_OPERATIONS: [&str; 7] = [
    "GetObject",
    "HeadObject",
    "ListObjectsV2",
    "GetObjectAttributes",
    "ListParts",
    "ListMultipartUploads",
    "CreateSession",
];

/// Whether a `ReadOnly` session executes `operation` — `false` for every operation outside
/// the published set, and for a request whose address names no operation of this model at all,
/// because such a session is "CONSTRAINED to execute the following" and nothing else.
pub fn read_only_session_serves(operation: Option<&str>) -> bool {
    operation.is_some_and(|operation| READ_ONLY_SESSION_OPERATIONS.contains(&operation))
}

/// The session token `req` presents, when it presents one at all.
pub fn presented_token(req: &RtRequest) -> Option<&str> {
    req.headers()
        .get(SESSION_TOKEN_HEADER)
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// How this service refuses a session that authorizes nothing on the bucket the request
/// addressed: the wire code, the status and the sentence S3 publishes for it.
///
/// Three conditions, three published codes, because a client branches on them differently —
/// each code, status and sentence taken from this service's own catalog
/// (`crate::state`), which is where every wire refusal of this service is worded:
///
/// * a token no session of this emulator carries — [`crate::state::INVALID_TOKEN_CODE`]
///   (400): nothing can be said about a session that was never vended;
/// * a session whose expiry the CLOCK has passed — [`crate::state::EXPIRED_TOKEN_CODE`]
///   (400), which is what tells a caller to take a new session rather than to fix its
///   request; and
/// * a live session created for ANOTHER bucket — [`crate::state::ACCESS_DENIED_CODE`] (403):
///   the session is a credential this caller holds, and it grants nothing on the bucket this
///   request addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRefusal {
    /// The `<Code>` a client branches on.
    pub code: &'static str,
    /// The HTTP status it is answered with.
    pub status: u16,
    /// The `<Message>`, which states the condition and no value from the request.
    pub message: &'static str,
}

/// See [`SessionRefusal`].
const INVALID_TOKEN: SessionRefusal = SessionRefusal {
    code: crate::state::INVALID_TOKEN_CODE,
    status: crate::state::INVALID_TOKEN_STATUS,
    message: crate::state::INVALID_TOKEN_MESSAGE,
};
/// See [`SessionRefusal`].
const EXPIRED_TOKEN: SessionRefusal = SessionRefusal {
    code: crate::state::EXPIRED_TOKEN_CODE,
    status: crate::state::EXPIRED_TOKEN_STATUS,
    message: crate::state::EXPIRED_TOKEN_MESSAGE,
};
/// See [`SessionRefusal`].
const OTHER_BUCKET: SessionRefusal = SessionRefusal {
    code: crate::state::ACCESS_DENIED_CODE,
    status: crate::state::ACCESS_DENIED_STATUS,
    message: crate::state::ACCESS_DENIED_MESSAGE,
};
/// A live session that covers the bucket and was created `ReadOnly`, presented with an
/// operation such a session does not execute: the session is a credential this caller holds
/// and it grants nothing beyond the published set, which is the same refusal — code, status
/// and sentence — that a session granting nothing on the addressed bucket earns.
const READ_ONLY_SESSION: SessionRefusal = SessionRefusal {
    code: crate::state::ACCESS_DENIED_CODE,
    status: crate::state::ACCESS_DENIED_STATUS,
    message: crate::state::ACCESS_DENIED_MESSAGE,
};

/// The refusal `verdict` earns, or `None` for the one verdict that is not a refusal.
pub fn refusal_for(verdict: SessionTokenVerdict) -> Option<SessionRefusal> {
    match verdict {
        SessionTokenVerdict::Authorized => None,
        SessionTokenVerdict::Unknown => Some(INVALID_TOKEN),
        SessionTokenVerdict::Expired => Some(EXPIRED_TOKEN),
        SessionTokenVerdict::OtherBucket => Some(OTHER_BUCKET),
    }
}

/// The [`SESSION_DENIED_KEYS`] key the refusal `verdict` earns is served under, or `None`
/// for the one verdict that is not a refusal.
pub fn denial_key(verdict: SessionTokenVerdict) -> Option<&'static str> {
    match verdict {
        SessionTokenVerdict::Authorized => None,
        SessionTokenVerdict::Unknown => Some(REFUSE_INVALID_TOKEN),
        SessionTokenVerdict::Expired => Some(REFUSE_EXPIRED_TOKEN),
        SessionTokenVerdict::OtherBucket => Some(REFUSE_OTHER_BUCKET),
    }
}

/// The operation this service serves a request whose session token authorizes nothing with.
///
/// It holds the refusal rather than resolving it: WHICH condition refused the request is
/// read from the store by the routing seam, which is the seam that has the request.
pub struct SessionDenied {
    /// What the routing seam decided about the token this request presented.
    refusal: SessionRefusal,
}

impl SessionDenied {
    /// The operation that answers `refusal`.
    pub fn new(refusal: SessionRefusal) -> Self {
        Self { refusal }
    }

    /// The error document this refusal is answered with, rendered through the SAME protocol
    /// every other answer of this service is rendered through, so it carries this service's
    /// envelope and its correlation headers.
    fn answer(&self, svc: &dyn Service) -> RtResponse {
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().protocol());
        let document = crate::scaffolded::UnmodeledMessage {
            message: self.refusal.message.to_string(),
        };
        protocol
            .serialize_error(
                &document,
                &crate::scaffolded::UNMODELED_SCHEMA,
                self.refusal.code,
                self.refusal.status,
            )
            .unwrap_or_else(|error| {
                ::tracing::warn!(
                    code = self.refusal.code,
                    %error,
                    "this refusal of a session token could not be rendered in this \
                     service's envelope"
                );
                let status = StatusCode::try_from(self.refusal.status)
                    .unwrap_or_else(|_| StatusCode::try_from(400).expect("400 is a status"));
                RtResponse::new(status, SdkBody::empty())
            })
    }
}

impl ErasedOperation for SessionDenied {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        _req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.answer(svc) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the verdict table: the case, the verdict, and the code and status it earns
    /// (`None` for the verdict that earns no refusal).
    type VerdictRow = (&'static str, SessionTokenVerdict, Option<(&'static str, u16)>);

    /// Each verdict earns the published code, status and sentence for its condition, and the
    /// authorized one earns no refusal at all.
    #[test]
    fn every_verdict_earns_its_published_refusal() {
        let rows: [VerdictRow; 4] = [
            ("a session that covers the bucket", SessionTokenVerdict::Authorized, None),
            (
                "a token no session carries",
                SessionTokenVerdict::Unknown,
                Some((crate::state::INVALID_TOKEN_CODE, crate::state::INVALID_TOKEN_STATUS)),
            ),
            (
                "a token whose session has expired",
                SessionTokenVerdict::Expired,
                Some((crate::state::EXPIRED_TOKEN_CODE, crate::state::EXPIRED_TOKEN_STATUS)),
            ),
            (
                "a session created for another bucket",
                SessionTokenVerdict::OtherBucket,
                Some((crate::state::ACCESS_DENIED_CODE, crate::state::ACCESS_DENIED_STATUS)),
            ),
        ];
        for (case, verdict, expected) in rows {
            let actual = refusal_for(verdict).map(|refusal| (refusal.code, refusal.status));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert_eq!(
                denial_key(verdict).is_some(),
                expected.is_some(),
                "case {case:?}: a refused verdict must name the key it is served under, got \
                 {:?}",
                denial_key(verdict)
            );
            if let Some(refusal) = refusal_for(verdict) {
                assert!(
                    !refusal.message.is_empty(),
                    "case {case:?}: a refusal must state its condition, got {:?}",
                    refusal.message
                );
            }
        }
    }
}
