// SPDX-License-Identifier: CC0-1.0

//! The JWT this service vends against the account's OUTBOUND web identity issuer: what it
//! claims, how long it lasts, and how it is signed.
//!
//! `GetWebIdentityToken` "returns a signed JSON Web Token (JWT) that represents the calling
//! Amazon Web Services identity", which "is signed by Amazon Web Services STS and can be
//! publicly verified using the verification keys published at the issuer's JWKS endpoint"
//! (this service's model). Two facts follow, and both are why this module exists rather
//! than the handler holding the rules:
//!
//! * the token is signed with a key of the ACCOUNT's issuer, which IAM holds and publishes
//!   at that issuer's own key set endpoint
//!   (`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::IssuerSigningKeys`,
//!   over the workspace's one JOSE signing seam `::service_common::jws_signing`) — so a
//!   token this service vends verifies against the very key set a relying party fetches,
//!   and neither a key nor the compact serialization is laid out here;
//! * what it claims is a function of the CALLER and of the request, so one mint cannot
//!   report an identity the credential does not speak for.
//!
//! Nothing here is written to the store: the token carries its own claims and its own
//! signature, so what a verifier decides about it comes from the token and the published
//! key set rather than from a record this service would have to keep.

use ::std::time::SystemTime;

use crate::ops::types::Tag;

/// How long a web identity token lasts when the request names no duration: "If not
/// specified, the default duration is 300 seconds (5 minutes)"
/// (`GetWebIdentityTokenRequest$DurationSeconds`, this service's model).
pub const WEB_IDENTITY_TOKEN_DEFAULT_DURATION_SECONDS: i32 = 300;

/// The most bytes the claim set of one web identity token may occupy.
///
/// It is the one serialized-token budget this service declares
/// ([`super::SESSION_TOKEN_SIZE_LIMIT`], the model's own `minimumSessionTokenSizeType`
/// range maximum): a token STS issues is measured against one ceiling whichever operation
/// issued it, so "what fits in a token this service vends" is a single number rather than
/// one per operation. A request whose tags push the claim set past it is
/// `JWTPayloadSizeExceededException` — "reduce the number of request tags included in the
/// GetWebIdentityToken API call to reduce the token payload size" — which is the member the
/// caller controls.
pub const MAX_WEB_IDENTITY_TOKEN_PAYLOAD_BYTES: usize = super::SESSION_TOKEN_SIZE_LIMIT;

/// The `iss` claim: the issuer the account was given.
const ISSUER_CLAIM: &str = "iss";

/// The `sub` claim: who the token represents.
const SUBJECT_CLAIM: &str = "sub";

/// The `aud` claim: "the intended recipient of the web identity token. This value populates
/// the aud claim in the JWT" (`GetWebIdentityTokenRequest$Audience`).
const AUDIENCE_CLAIM: &str = "aud";

/// The `iat` claim (RFC 7519 §4.1.6): when the token was issued.
const ISSUED_AT_CLAIM: &str = "iat";

/// The `nbf` claim (RFC 7519 §4.1.5): the first instant the token may be accepted, which
/// for a token issued now is now.
const NOT_BEFORE_CLAIM: &str = "nbf";

/// The `exp` claim (RFC 7519 §4.1.4): "the expiration is determined by adding the
/// DurationSeconds value to the time the token was issued"
/// (`GetWebIdentityTokenResponse$Expiration`).
const EXPIRY_CLAIM: &str = "exp";

/// The claim set one web identity token carries, for the caller `subject` and the request's
/// own audiences, tags and window.
///
/// The request's tags come FIRST and the registered claims after them, so a tag whose key
/// is the name of a registered claim cannot restate the token's issuer, subject or window:
/// "these tags are added as custom claims to the JWT"
/// (`GetWebIdentityTokenRequest$Tags`), which is a claim beside the standard ones and never
/// in place of one.
///
/// `issued_at` and `expires_at` are seconds since the Unix epoch, the only form RFC 7519
/// §2 defines a `NumericDate` in.
#[must_use]
pub fn web_identity_token_claims(
    issuer: &str,
    subject: &str,
    audiences: &[String],
    tags: &[Tag],
    issued_at: i64,
    expires_at: i64,
) -> ::serde_json::Value {
    let mut claims = ::serde_json::Map::new();
    for tag in tags {
        claims.insert(
            tag.key.clone(),
            ::serde_json::Value::String(tag.value.clone()),
        );
    }
    for (name, value) in [
        (ISSUER_CLAIM, ::serde_json::Value::String(issuer.to_string())),
        (
            SUBJECT_CLAIM,
            ::serde_json::Value::String(subject.to_string()),
        ),
        (
            AUDIENCE_CLAIM,
            ::serde_json::Value::Array(
                audiences
                    .iter()
                    .map(|audience| ::serde_json::Value::String(audience.clone()))
                    .collect(),
            ),
        ),
        (ISSUED_AT_CLAIM, ::serde_json::Value::from(issued_at)),
        (NOT_BEFORE_CLAIM, ::serde_json::Value::from(issued_at)),
        (EXPIRY_CLAIM, ::serde_json::Value::from(expires_at)),
    ] {
        claims.insert(name.to_string(), value);
    }
    ::serde_json::Value::Object(claims)
}

/// Whether `claims` is past the budget one web identity token's payload may occupy
/// ([`MAX_WEB_IDENTITY_TOKEN_PAYLOAD_BYTES`]), measured on the octets the token would
/// carry.
#[must_use]
pub fn web_identity_token_payload_too_large(claims: &::serde_json::Value) -> bool {
    claims.to_string().len() > MAX_WEB_IDENTITY_TOKEN_PAYLOAD_BYTES
}

/// How AWS words `JWTPayloadSizeExceededException`, as the shape itself states the
/// condition: the payload is over the maximum, and the member to reduce is the request's
/// tags. It carries nothing from the request, because what the caller has to change is the
/// member and not one value of it.
#[must_use]
pub fn jwt_payload_size_exceeded_message() -> String {
    "The requested token payload size exceeds the maximum allowed size. Reduce the number \
     of request tags included in the GetWebIdentityToken API call to reduce the token \
     payload size."
        .to_string()
}

/// How AWS words `SessionDurationEscalationException`, as that shape states the condition.
#[must_use]
pub fn session_duration_escalation_message() -> String {
    "The requested token duration would extend the session beyond its original expiration \
     time. You cannot use this operation to extend the lifetime of a session beyond what \
     was granted when the session was originally created."
        .to_string()
}

/// How AWS words `OutboundWebIdentityFederationDisabledException`, as that shape states the
/// condition: the feature is off for this account, and it has to be turned on first.
#[must_use]
pub fn outbound_web_identity_federation_disabled_message() -> String {
    "The outbound web identity federation feature is not enabled for this account. To use \
     this feature, you must first enable it through the Amazon Web Services Management \
     Console or API."
        .to_string()
}

/// Whether a token lasting until `expires_at` would outlast the session the request was
/// signed with.
///
/// "The requested token duration would extend the session beyond its original expiration
/// time. You cannot use this operation to extend the lifetime of a session beyond what was
/// granted when the session was originally created"
/// (`SessionDurationEscalationException`): a request signed with a temporary credential may
/// mint a token that ends no later than that credential does, and a request signed with a
/// long-term key (`None`) has no such ceiling.
#[must_use]
pub fn outlasts_the_calling_session(
    session_expires_at: Option<SystemTime>,
    expires_at: SystemTime,
) -> bool {
    session_expires_at.is_some_and(|ends| expires_at > ends)
}
