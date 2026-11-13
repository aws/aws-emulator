// SPDX-License-Identifier: CC0-1.0

//! One session `CreateSession` vended, and what it is scoped to.
//!
//! A session is two things at once, and they travel in two different places on a Zonal
//! endpoint request:
//!
//! * the KEY PAIR the request is SIGNED with — an access key id and a secret access key the
//!   emulator's credential store minted, which is what makes the signature verify at all;
//!   and
//! * the session TOKEN, which "you also apply … to the `x-amz-s3session-token` request
//!   header for authorization" (`CreateSession`'s own documentation) — not the
//!   `x-amz-security-token` header an STS temporary credential travels in. It is not
//!   signed with and it is not a SigV4 credential: S3 authorizes it, against the bucket the
//!   session was created for, because "temporary security credentials are scoped to the
//!   bucket and expire after 5 minutes".
//!
//! So the store keeps, per session, the expiry both halves share and the scope the token is
//! judged against.

use super::*;

/// What a presented session token authorizes, as
/// [`ComAmazonawsS3Amazons3State::session_token_verdict`] answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTokenVerdict {
    /// The token names a live session created for the bucket the request addressed.
    Authorized,
    /// No session this service vended carries this token.
    Unknown,
    /// A session carries it, and the clock has passed its expiry: "after the expiration
    /// time, any calls that you make with those credentials will fail".
    Expired,
    /// A live session carries it, and it was created for ANOTHER bucket — "temporary
    /// security credentials are scoped to the bucket".
    OtherBucket,
}

/// One session this service vended and has not released.
#[derive(Debug, Clone)]
pub struct VendedSession {
    /// The instant the session's credentials stop being accepted: "temporary security
    /// credentials are scoped to the bucket and expire after 5 minutes".
    pub expires_at: SystemTime,
    /// What the session is scoped to, for a session `CreateSession` vended; `None` for a
    /// record made about the COUNT of sessions alone
    /// ([`ComAmazonawsS3Amazons3State::record_vended_session`]).
    pub scope: Option<SessionScope>,
}

/// What one session is scoped to: the directory bucket it was created for and the token a
/// Zonal endpoint request presents it with.
#[derive(Debug, Clone)]
pub struct SessionScope {
    /// The directory bucket `CreateSession` was called on. A Zonal endpoint request
    /// presenting this session's token against any OTHER bucket is presenting a session
    /// that does not cover it.
    pub bucket: String,
    /// The session token the caller was handed, as it is presented in
    /// `x-amz-s3session-token`.
    pub token: String,
    /// The MODE the session was created in, which decides WHICH Zonal endpoint operations
    /// the caller may execute under it — see [`SessionMode`].
    pub mode: SessionMode,
    /// The server-side encryption algorithm the session was ESTABLISHED with: "Amazon S3
    /// will use the encryption settings values from the `CreateSession` request to protect
    /// new objects in the directory bucket", and "in the Zonal endpoint API calls … the
    /// encryption request headers must match the encryption settings that are specified in
    /// the `CreateSession` request" (`PutObjectRequest$ServerSideEncryption`, of a directory
    /// bucket) — so a write naming another algorithm is refused against this one.
    ///
    /// `None` for a session established under no algorithm at all, which names nothing for a
    /// write to differ from.
    pub encryption_algorithm: Option<String>,
}

/// The MODE one session was created in: "specifies the mode of the session that will be
/// created, either `ReadWrite` or `ReadOnly`. … A `ReadWrite` session is capable of executing
/// all the Zonal endpoint API operations on a directory bucket. A `ReadOnly` session is
/// constrained to execute [a published set of] Zonal endpoint API operations"
/// (`CreateSessionRequest$SessionMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionMode {
    /// "Capable of executing all the Zonal endpoint API operations on a directory bucket" —
    /// and the DEFAULT, because "if no session mode is specified, the default behavior
    /// attempts to create a session with the maximum allowable privilege. It will first
    /// attempt to create a `ReadWrite` session" (same member).
    #[default]
    ReadWrite,
    /// "Constrained to execute the following Zonal endpoint API operations" — the published
    /// set, which [`crate::service::express_session::READ_ONLY_SESSION_OPERATIONS`] carries.
    ReadOnly,
}

/// The `SessionMode` value naming [`SessionMode::ReadOnly`], as the model's `SessionMode`
/// enum publishes it (`smithy_query(query="shape",shape="SessionMode")` reports
/// `ReadOnly | ReadWrite`).
pub const READ_ONLY_SESSION_MODE: &str = "ReadOnly";

impl SessionMode {
    /// The mode a `CreateSession` request naming `requested` is created in: the one it named,
    /// and [`SessionMode::ReadWrite`] for a request naming none — "the default behavior
    /// attempts to create a session with the maximum allowable privilege".
    pub fn requested(requested: Option<&str>) -> Self {
        match requested {
            Some(READ_ONLY_SESSION_MODE) => Self::ReadOnly,
            _ => Self::ReadWrite,
        }
    }
}

/// The header a Zonal endpoint request carries its session token in: "you also apply the
/// session token to the `x-amz-s3session-token` request header for authorization"
/// (`CreateSession`'s own documentation).
pub const SESSION_TOKEN_HEADER: &str = "x-amz-s3session-token";

/// WHICH session a request presents, in the two places a session travels — the one key a
/// stored session is looked up by ([`ComAmazonawsS3Amazons3State::presented_session_scope`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentedSession {
    /// The session token the request carries in [`SESSION_TOKEN_HEADER`].
    Token(String),
    /// The access key id the request is SIGNED with, for a request presenting no token: the
    /// scope and the expiry belong to the credentials as much as to the token.
    Credential(String),
}

/// The session `headers` presents, or `None` for a request presenting neither a session token
/// nor a credential this emulator can read a key id out of.
///
/// The reader a HANDLER uses, over the `http::HeaderMap` its `OperationContext` carries; the
/// routing seam reads the same two places off the runtime request
/// ([`crate::service::express_session::presented_token`]).
pub fn presented_session(headers: &::http::HeaderMap) -> Option<PresentedSession> {
    let token = headers
        .get(SESSION_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty());
    if let Some(token) = token {
        return Some(PresentedSession::Token(token.to_string()));
    }
    Some(PresentedSession::Credential(
        ::emulator_server::sigv4_credential_scope(headers)?.access_key_id,
    ))
}
