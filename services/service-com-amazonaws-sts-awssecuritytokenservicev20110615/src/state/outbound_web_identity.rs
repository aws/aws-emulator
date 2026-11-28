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
//!
//! The refusal texts below are what the SERVICE answers each error class with. A shape's
//! `@documentation` is API-reference prose written for a reader of the reference, not the
//! `message` the wire carries, so none of them is sourced from it: each is the fixed text of
//! its class, and a part of a message that varies at all comes from the request or from the
//! store rather than from a description of the shape.

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

/// The claim the request's `Scope` is carried in: "Amazon Web Services STS copies the value
/// exactly as you send it into the top-level `scope` claim of the token. If you don't specify
/// `Scope`, the token doesn't include a `scope` claim" (`GetWebIdentityTokenRequest$Scope`).
const SCOPE_CLAIM: &str = "scope";

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
/// §2 defines a `NumericDate` in. `stated_mode` is the subject mode the token states beside
/// its subject, for a mode that states one ([`TokenSubject::stated_mode`]), and `scope` the
/// OAuth 2.0 scope values the request asked to carry ([`requested_scope`]).
#[must_use]
pub fn web_identity_token_claims(
    issuer: &str,
    subject: &TokenSubject,
    audiences: &[String],
    tags: &[Tag],
    issued_at: i64,
    expires_at: i64,
    scope: Option<&str>,
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
            ::serde_json::Value::String(subject.subject.clone()),
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
    // The mode the subject was taken under, for the one mode that states it.
    if let Some(mode) = subject.stated_mode {
        claims.insert(
            SUBJECT_MODE_CLAIM.to_string(),
            ::serde_json::Value::String(mode.to_string()),
        );
    }
    // The scope values the request asked to carry, copied exactly as it sent them — and no
    // claim at all for a request that named none.
    if let Some(scope) = scope {
        claims.insert(
            SCOPE_CLAIM.to_string(),
            ::serde_json::Value::String(scope.to_string()),
        );
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

/// How AWS words `JWTPayloadSizeExceededException` on the wire: the one fixed phrase it
/// answers an oversized claim set with — not the reference prose the shape is DOCUMENTED
/// with, and not a rendering of the budget or of the size measured, neither of which it
/// reports.
#[must_use]
pub fn jwt_payload_size_exceeded_message() -> String {
    "JWT payload exceeds maximum allowed size".to_string()
}

/// How AWS words `SessionDurationEscalationException` on the wire: the one fixed sentence
/// stating the rule the request broke — the token's expiry has to precede the session's.
/// Like the two refusals beside it, it is the SERVICE's own text rather than the shape's
/// documentation, and it interpolates neither of the two instants it compares.
#[must_use]
pub fn session_duration_escalation_message() -> String {
    "Requested token expiry time must be before the original session's expiry time.".to_string()
}

/// How AWS words `OutboundWebIdentityFederationDisabledException`: the one fixed sentence it
/// answers every request refused for this reason with, naming the feature that is off.
///
/// It is the SERVICE's own wording and not the prose the shape is DOCUMENTED with — the two
/// are different texts, and a client matching, logging or surfacing a refusal reads the
/// first. The message carries nothing from the request or from state, because the only thing
/// to report is that the account has the feature off.
#[must_use]
pub fn outbound_web_identity_federation_disabled_message() -> String {
    "OutboundWebIdentityFederation is disabled.".to_string()
}

/// Whether `algorithm` is one a web identity token may be signed under: "valid values are
/// RS256 (RSA with SHA-256) and ES384 (ECDSA using P-384 curve with SHA-384)"
/// (`GetWebIdentityTokenRequest$SigningAlgorithm`, this service's model) — which is the set
/// the account's issuer publishes a key for
/// (`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::JWT_SIGNING_ALGORITHMS`),
/// so this operation admits exactly the algorithms a relying party can verify a token under.
///
/// The model constrains the member's LENGTH and not its value set, so the set is a rule this
/// service enforces over the REQUEST itself: it is a property of the request and of nothing
/// else, which is why it is decided from the value rather than by looking for a key.
#[must_use]
pub fn supported_signing_algorithm(algorithm: &str) -> bool {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::JWT_SIGNING_ALGORITHMS
        .contains(&algorithm)
}

/// How AWS refuses a `SigningAlgorithm` outside that set: one fixed sentence for the error
/// class, pointing the caller at the operation's own documentation for the algorithms it
/// admits. It names no request value, so every such refusal reads the same.
#[must_use]
pub fn unsupported_signing_algorithm_message() -> String {
    "Requested SigningAlgorithm is not supported. See the GetWebIdentityToken documentation \
     for supported algorithms"
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

// ---------------------------------------------------------------------------
// Which identity the token speaks for
// ---------------------------------------------------------------------------

/// The subject mode a request that names none asks for: "if you don't specify a value, Amazon
/// Web Services STS uses `AWSIdentity`" (`GetWebIdentityTokenRequest$SubjectMode`, this
/// service's model).
pub const DEFAULT_SUBJECT_MODE: &str = "AWSIdentity";

/// The mode that puts the END USER the calling session acts for in the token's subject,
/// rather than the calling principal itself (same member).
pub const ORIGIN_IDENTITY_SUBJECT_MODE: &str = "OriginIdentity";

/// The claim STS adds beside the subject when the token's subject is the origin identity:
/// "Amazon Web Services STS also adds a `subject_mode` claim to the token" (same member).
const SUBJECT_MODE_CLAIM: &str = "subject_mode";

/// What the `sub` claim of one token carries, and whether the token states the mode it was
/// minted under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSubject {
    /// The value of the `sub` claim.
    pub subject: String,
    /// The mode to state in the [`SUBJECT_MODE_CLAIM`] claim, for a mode that is stated.
    pub stated_mode: Option<&'static str>,
}

/// Why a request's `SubjectMode` is one this operation will not mint a token under.
///
/// A value outside the modelled set is NOT among them: `SubjectMode` targets an enum
/// (`webIdentityTokenSubjectModeType`, values `AWSIdentity` and `OriginIdentity`), so the
/// boundary walk refuses one before this is asked — with the constraint clause AWS words for
/// an enum member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectModeRefusal {
    /// The mode asks for the origin identity, and the calling session carries no origin
    /// identity to put in the token.
    NoOriginIdentity,
    /// The origin identity's subject is not a value STS puts in a token's subject.
    SubjectNotAllowed,
}

impl SubjectModeRefusal {
    /// How this service words the refusal of each class.
    ///
    /// Each is the fixed text of its class and carries nothing of the request: the subject of
    /// a calling session is the end user's own identifier, which a refusal does not echo.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoOriginIdentity => "The calling session's trusted identity propagation \
                                       context must carry the aws:OriginIdentity/Subject key \
                                       and either the aws:OriginIdentity/Issuer key or the \
                                       aws:OriginIdentity/FederatedProviderArn key to request \
                                       the OriginIdentity subject mode."
                .to_string(),
            Self::SubjectNotAllowed => "The calling session's aws:OriginIdentity/Subject key \
                                        must not begin with arn:, must not begin or end with \
                                        a space, and must not carry a control, format or \
                                        separator character other than a space."
                .to_string(),
        }
    }
}

/// What the token's `sub` claim carries for `mode`, or the class the request is refused as.
///
/// * `AWSIdentity` — the default — puts "the ARN of the calling Amazon Web Services
///   principal" in the claim, which is the identity the presented credential speaks for.
/// * `OriginIdentity` puts "the value of the `aws:OriginIdentity/Subject` key in the trusted
///   identity propagation context of the calling session", and the token also states the mode
///   it was minted under. To use it "the trusted identity propagation context of the calling
///   session must contain the `aws:OriginIdentity/Subject` key and either the
///   `aws:OriginIdentity/Issuer` key or the `aws:OriginIdentity/FederatedProviderArn` key. If
///   it doesn't, or if the subject is blank, Amazon Web Services STS rejects the request with
///   a validation error", and the subject itself must be one STS will state
///   ([`origin_subject_is_allowed`]).
///
/// (`GetWebIdentityTokenRequest$SubjectMode`, this service's model.)
pub fn token_subject(
    mode: Option<&str>,
    caller_arn: &str,
    propagated: Option<&crate::ops::types::TrustedIdentityPropagationContext>,
) -> Result<TokenSubject, SubjectModeRefusal> {
    // The modelled value set is the boundary walk's to enforce (see [`SubjectModeRefusal`]),
    // so what is left here is the default and the two modes themselves.
    if mode.unwrap_or(DEFAULT_SUBJECT_MODE) != ORIGIN_IDENTITY_SUBJECT_MODE {
        return Ok(TokenSubject {
            subject: caller_arn.to_string(),
            stated_mode: None,
        });
    }
    let origin = propagated
        .and_then(|carried| carried.origin_identity.as_ref())
        .ok_or(SubjectModeRefusal::NoOriginIdentity)?;
    let subject = origin.subject.as_deref().unwrap_or_default();
    let names_its_provider =
        origin.issuer.is_some() || origin.federated_provider_arn.is_some();
    // "If it doesn't, or if the subject is blank, Amazon Web Services STS rejects the request
    // with a validation error": a subject of nothing but space is blank too.
    if subject.trim().is_empty() || !names_its_provider {
        return Err(SubjectModeRefusal::NoOriginIdentity);
    }
    if !origin_subject_is_allowed(subject) {
        return Err(SubjectModeRefusal::SubjectNotAllowed);
    }
    Ok(TokenSubject {
        subject: subject.to_string(),
        stated_mode: Some(ORIGIN_IDENTITY_SUBJECT_MODE),
    })
}

/// Whether an origin identity's subject is one STS puts in a token's `sub` claim.
///
/// "Amazon Web Services STS denies the request if the subject begins with the text `arn:`, in
/// any combination of uppercase and lowercase letters. Amazon Web Services STS also denies the
/// request if the subject begins or ends with a space, or contains a control, format, or
/// separator character other than a space" (`GetWebIdentityTokenRequest$SubjectMode`).
///
/// The character classes are Unicode general categories, so they are matched with the
/// maintained regular-expression engine this crate reads its other Unicode classes with
/// rather than by listing code points here.
#[must_use]
pub fn origin_subject_is_allowed(subject: &str) -> bool {
    /// An ARN prefix is the one text a subject may not begin with, case-insensitively.
    const ARN_PREFIX: &str = "arn:";

    // `get` rather than an index: the subject is the token's own claim, so its fourth BYTE
    // may sit inside a character, and slicing there would abort the request instead of
    // answering it.
    if subject
        .get(..ARN_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(ARN_PREFIX))
    {
        return false;
    }
    if subject.starts_with(' ') || subject.ends_with(' ') {
        return false;
    }
    !forbidden_subject_characters().is_match(subject)
}

/// The characters an origin identity's subject may not carry: Unicode's control (`Cc`),
/// format (`Cf`) and separator (`Zl`, `Zp`, `Zs`) categories, with the space itself allowed.
fn forbidden_subject_characters() -> &'static ::regex::Regex {
    static FORBIDDEN: ::std::sync::OnceLock<::regex::Regex> = ::std::sync::OnceLock::new();
    FORBIDDEN.get_or_init(|| {
        ::regex::Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]|[\p{Zs}&&[^ ]]")
            .expect("the forbidden-character class is a valid regular expression")
    })
}

// ---------------------------------------------------------------------------
// The scope values the token carries
// ---------------------------------------------------------------------------

/// Why a request's `Scope` is one this operation will not mint a token under.
///
/// The VALUES themselves are the model's to constrain — `scopeType` is
/// `@length 1..256` over `^[\x21\x23-\x2B\x2D-\x5B\x5D-\x7E]+( …)*$`, which already refuses a
/// leading or trailing space, a double quotation mark, a comma and a backslash — so what is
/// left here are the three rules that are about the REQUEST as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeRefusal {
    /// The token's subject is not the origin identity.
    NotOriginIdentity,
    /// The request named more than one audience, or none.
    NotOneAudience,
    /// A scope value is repeated.
    RepeatedValue,
}

impl ScopeRefusal {
    /// How this service words the refusal of each class: the fixed text of its class, naming
    /// the rule the request broke and no value of the request.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotOriginIdentity => {
                "Scope can be specified only when SubjectMode is OriginIdentity.".to_string()
            }
            Self::NotOneAudience => {
                "Scope can be specified only when the request names exactly one audience."
                    .to_string()
            }
            Self::RepeatedValue => "Scope must not repeat a scope value.".to_string(),
        }
    }
}

/// The scope values the vended token carries, or the class the request is refused as.
///
/// "One or more OAuth 2.0 scope values to include in the JSON Web Token (JWT). Amazon Web
/// Services STS copies the value exactly as you send it into the top-level `scope` claim of
/// the token. If you don't specify `Scope`, the token doesn't include a `scope` claim…
/// Separate multiple values with a single space… You can't repeat a scope value. Scope values
/// are case sensitive… You can specify `Scope` only when `SubjectMode` is `OriginIdentity` and
/// you request exactly one audience" (`GetWebIdentityTokenRequest$Scope`, this service's
/// model). So the value is carried VERBATIM — nothing is reordered, folded or de-duplicated —
/// and the three conditions it may be sent under are decided before anything is signed.
pub fn requested_scope<'a>(
    scope: Option<&'a str>,
    subject: &TokenSubject,
    audiences: &[String],
) -> Result<Option<&'a str>, ScopeRefusal> {
    let Some(scope) = scope else {
        return Ok(None);
    };
    if subject.stated_mode != Some(ORIGIN_IDENTITY_SUBJECT_MODE) {
        return Err(ScopeRefusal::NotOriginIdentity);
    }
    if audiences.len() != 1 {
        return Err(ScopeRefusal::NotOneAudience);
    }
    // Case SENSITIVE, as the member states: `read` and `READ` are two values, and only an
    // exact repeat is one.
    let values: Vec<&str> = scope.split(' ').collect();
    for (position, value) in values.iter().enumerate() {
        if values[..position].contains(value) {
            return Err(ScopeRefusal::RepeatedValue);
        }
    }
    Ok(Some(scope))
}
