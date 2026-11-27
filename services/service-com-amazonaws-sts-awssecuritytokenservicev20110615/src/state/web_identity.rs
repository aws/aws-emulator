// SPDX-License-Identifier: CC0-1.0

//! Which web identity a presented token stands for, and the class a token that stands for
//! none is refused as.
//!
//! `AssumeRoleWithWebIdentity` federates the identity a token names, and the whole question
//! it has to answer first is whether the token is one the provider the account registered
//! really issued. That answer is a VERIFICATION, in the order RFC 7515 §5.2 and OpenID
//! Connect put it: the value is a compact JSON Web Token, the clock has not run out on it,
//! something SIGNED it at all, its issuer is a provider the account can resolve, and the
//! signature checks out against the key set that issuer publishes.
//!
//! Two kinds of issuer are resolvable without leaving this process, and they are the only
//! kinds whose tokens can be federated:
//!
//! * a token this emulator ITSELF issued ([`::service_common::web_identity_tokens`], the
//!   registry every service that mints one records it in — the Amazon Cognito Identity
//!   `GetOpenIdToken` / `GetOpenIdTokenForDeveloperIdentity` issuance, and the projected
//!   service account token an Amazon EKS cluster's Kubernetes API server issues), which is
//!   evidence of the issuance in place of a signature; and
//! * a token signed by an issuer this emulator HOSTS — an Amazon Cognito user pool, which
//!   publishes its key set in this same process ([`published_key_set`]) at the location the
//!   token's own `iss` claim leads a verifier to — whose signature is therefore checked
//!   here exactly as any third party holding that key set would check it.
//!
//! Every OTHER issuer publishes its key set at a URL outside the emulator, and the emulator
//! makes no outbound connection to fetch one, so for such a token there is no verification
//! key to be had — the one class AWS answers with the key-retrieval message.
//!
//! Each class is refused with the fixed text AWS words for THAT class
//! ([`TokenDefect::message`]), so a caller can tell a token nothing signed (never
//! retryable, a security event) from a token whose signature did not check out from a
//! provider whose keys could not be read (operational, retryable).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::service_common::jwt;
use ::service_common::web_identity_tokens::IssuedWebIdentity;

/// The claim naming "the principal that issued the JWT" (RFC 7519 §4.1.1), which an OpenID
/// Connect ID token carries as the issuer's URL. It is what decides which provider's keys
/// and which condition keys a token is read against.
const ISSUER_CLAIM: &str = "iss";

/// The claim naming who the token was issued FOR (RFC 7519 §4.1.3) — the client id an
/// OpenID Connect provider issued it to, which is the `Audience` the exchange reports and
/// the `<provider>:aud` condition key a trust policy decides on.
const AUDIENCE_CLAIM: &str = "aud";

/// The claim naming the identity the token speaks for (RFC 7519 §4.1.2), reported as
/// `SubjectFromWebIdentityToken`.
const SUBJECT_CLAIM: &str = "sub";

/// The claim a token carries its expiry in: "the expiration time on or after which the JWT
/// MUST NOT be accepted for processing", a NumericDate (RFC 7519 §4.1.4), which an OpenID
/// Connect ID token is required to carry.
const EXPIRY_CLAIM: &str = "exp";

/// The claim naming what authenticated the identity the token speaks for (RFC 8176's
/// authentication methods references), published to a trust policy as one
/// `<provider>:amr` value per method.
const AUTHENTICATION_METHODS_CLAIM: &str = "amr";

/// The claim an identity provider carries the SESSION TAGS of its token in: an OpenID
/// Connect provider that tags the identity its token names writes them under the
/// `https://aws.amazon.com/tags` claim, whose `principal_tags` member holds one tag key per
/// member with the value(s) it carries (AWS IAM User Guide, *Passing session tags using
/// `AssumeRoleWithWebIdentity`*). A token this emulator ITSELF issued carries them as the
/// issuance recorded them instead ([`IssuedWebIdentity::principal_tags`]), which is the
/// evidence it holds in place of a signature.
const SESSION_TAGS_CLAIM: &str = "https://aws.amazon.com/tags";

/// The member of [`SESSION_TAGS_CLAIM`] holding the tags themselves, keyed by tag key.
const PRINCIPAL_TAGS_MEMBER: &str = "principal_tags";

/// How AWS words the refusal of a value that is not a readable identity token, including one
/// that carries no expiry at all: an ID token is a JSON Web Token, and an OAuth 2.0 ACCESS
/// token presented in its place is the mistake the sentence names.
const UNREADABLE_TOKEN_MESSAGE: &str =
    "The ID Token provided is not a valid JWT. (You may see this error if you sent an \
     Access Token)";

/// How AWS words the refusal of a token NOTHING signed — RFC 7519 §6's unsecured JWT, whose
/// header names the `none` algorithm or whose signature segment is empty. Fixed text: the
/// class is the token's own form, so the sentence names nothing of the request.
const UNSIGNED_TOKEN_MESSAGE: &str = "We don't accept unsigned JWTs";

/// How AWS words the refusal of a signed token whose signature does not check out against
/// the key set its issuer publishes. Fixed text: a verifier that rejected a signature says
/// no more than that it did, so the sentence names neither the key nor the claim that
/// failed.
const UNVALIDATED_TOKEN_MESSAGE: &str =
    "The web identity token provided could not be validated. See the \
     AssumeRoleWithWebIdentity documentation for requirements.";

/// How AWS words the refusal of a token whose issuer's verification key could not be
/// RETRIEVED: the key lives at the issuer's own key-set endpoint, and the verifier could not
/// read it, so nothing was decided about the token itself.
const UNVERIFIABLE_TOKEN_MESSAGE: &str =
    "Couldn't retrieve verification key from your identity provider, please reference \
     AssumeRoleWithWebIdentity documentation for requirements";

/// How AWS words the refusal of a token the caller's OWN partition minted: a JWT vended
/// against the account's outbound web identity issuer, presented back to this partition's
/// token exchange.
///
/// That federation is OUTBOUND only — the token is for a party outside the partition to
/// verify — so inside the partition the exchange does not serve it, and the sentence says so
/// and names the operation it is answering. Fixed text: the class is the token's issuer, so
/// nothing of the request or of the account appears in it.
const STS_ISSUED_TOKEN_MESSAGE: &str =
    "AssumeRoleWithWebIdentity with STS-issued tokens is not yet supported.";

/// Why a presented value is not a web identity this service can federate.
///
/// One variant per class AWS tells apart, in the order a verifier decides them. The caller
/// maps each to the modelled error its class carries — every one of these is
/// `InvalidIdentityToken` except [`TokenDefect::Expired`], which is `ExpiredToken` — and
/// states it with [`TokenDefect::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenDefect {
    /// Not a compact JSON Web Token at all, or one carrying no readable `iss` or `exp`
    /// claim, which an ID token is required to carry.
    Unreadable,
    /// The clock has passed the token's own `exp`. Carries both instants as the NumericDate
    /// seconds the claim is written in — the clock's and the token's — so the caller can see
    /// by how much it missed.
    Expired { at: i64, expiry: i64 },
    /// Nothing signed the token: RFC 7519 §6's unsecured form.
    Unsigned,
    /// The `iss` names a web identity issuer of the caller's OWN partition — the account's
    /// outbound federation issuer — whose tokens this exchange does not serve.
    StsIssued,
    /// The `iss` names a provider the ACCOUNT has not registered, so there is no provider
    /// whose keys or condition keys the token could be read against. Carries the issuer the
    /// token claims.
    UnregisteredIssuer(String),
    /// The issuer's published key set was read, and the signature over the token's own bytes
    /// does not check out against it.
    NotValidated,
    /// The verification key could not be retrieved, so whether the issuer signed the token
    /// is UNDECIDED — the issuer publishes its key set outside this emulator, or the key it
    /// published is not one a signature can be checked under here.
    KeyUnretrievable,
}

impl TokenDefect {
    /// The sentence AWS words for this class. Fixed text per class, except the expiry, whose
    /// two instants are the clock's and the token's own.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Unreadable => UNREADABLE_TOKEN_MESSAGE.to_string(),
            Self::Expired { at, expiry } => format!(
                "Token expired: current date/time {at} must be before the expiration \
                 date/time {expiry}"
            ),
            Self::Unsigned => UNSIGNED_TOKEN_MESSAGE.to_string(),
            Self::StsIssued => STS_ISSUED_TOKEN_MESSAGE.to_string(),
            Self::UnregisteredIssuer(issuer) => {
                format!("No OpenIDConnect provider found in your account for {issuer}")
            }
            Self::NotValidated => UNVALIDATED_TOKEN_MESSAGE.to_string(),
            Self::KeyUnretrievable => UNVERIFIABLE_TOKEN_MESSAGE.to_string(),
        }
    }
}

/// What a presented token stands for: the identity it names, and the CLAIM SET it carried.
///
/// The claims are kept beside the identity because a trust policy decides on them: an
/// identity provider publishes one condition key per claim of the token it issued
/// ([`condition_context`]), so a decision about a verified token needs the claims that token
/// actually carried rather than the three the exchange reports back.
///
/// A token THIS emulator issued carries no claim set here: what it stands for is the
/// issuance the registry recorded, and the keys published for it are the facts that
/// issuance holds.
#[derive(Debug, Clone)]
pub struct WebIdentity {
    /// The identity the exchange reports: the issuer, the audience, the subject, the
    /// authentication methods, the principal tags the token carries and the token's own
    /// expiry.
    pub identity: IssuedWebIdentity,
    /// The claims the token carried, for a token whose issuer signed it; empty for an
    /// issuance this emulator made itself.
    pub claims: ::serde_json::Map<String, ::serde_json::Value>,
}

/// Every spelling of a web identity's ISSUER that a role's trust policy may name in its
/// `Principal` element.
///
/// A trust policy admits a web identity by naming its issuer as a FEDERATED principal, and
/// AWS accepts two spellings of one issuer: the service principal or host name an identity
/// provider is known by (`cognito-identity.amazonaws.com`, `accounts.google.com`) and the
/// ARN of the IAM OpenID Connect provider the account registered for it
/// (`arn:aws:iam::<account>:oidc-provider/<host>`). The `iss` claim itself carries the
/// issuer as a URL, so the scheme is dropped to get the name the other two are built from —
/// the form a policy and a condition key both spell it in.
#[must_use]
pub fn principals(issuer: &str) -> Vec<String> {
    let provider = super::web_identity_provider_name(issuer);
    vec![
        provider.to_string(),
        issuer.to_string(),
        format!(
            "arn:aws:iam::{}:oidc-provider/{provider}",
            ::engine::emulator_account_id()
        ),
    ]
}

/// The condition keys an identity provider publishes about ONE of its tokens, for a trust
/// policy's `Condition` to be decided against.
///
/// A provider publishes the CLAIMS of the token it issued, each key named
/// `<provider>:<claim>` after the provider the token came from — so `<provider>:aud` is "the
/// intended audience (also known as client ID) of the web identity token", `<provider>:sub`
/// is the identity it names, and every other claim the issuer put in the token is published
/// under its own name, which is what lets a trust policy be written over any one of them. A
/// claim carrying a LIST of values publishes one pair per value (`amr`, the authentication
/// methods references, and any other multi-valued claim), the form a policy decides under
/// one of IAM's set qualifiers. Prefixing every key with the provider is what keeps a clause
/// written for one provider from ever deciding another's token.
///
/// A claim whose value is an object is published under no key: there is no scalar for a
/// clause to compare against, so a clause over it stays undecided and withholds the grant.
#[must_use]
pub fn condition_context(web_identity: &WebIdentity) -> Vec<(String, String)> {
    let provider = super::web_identity_provider_name(&web_identity.identity.issuer);
    let mut context: Vec<(String, String)> = Vec::new();
    for (claim, value) in &web_identity.claims {
        for text in scalar_values(value) {
            context.push((format!("{provider}:{claim}"), text));
        }
    }
    // The three facts the ISSUANCE holds, for a token this emulator issued itself (which
    // carries no claim set here) and for a claim a verified token did not carry.
    let carried = |claim: &str| {
        web_identity
            .claims
            .get(claim)
            .is_some_and(|value| !scalar_values(value).is_empty())
    };
    if !carried(AUDIENCE_CLAIM) {
        context.push((
            format!("{provider}:{AUDIENCE_CLAIM}"),
            web_identity.identity.audience.clone(),
        ));
    }
    if !carried(SUBJECT_CLAIM) {
        context.push((
            format!("{provider}:{SUBJECT_CLAIM}"),
            web_identity.identity.subject.clone(),
        ));
    }
    if !carried(AUTHENTICATION_METHODS_CLAIM) {
        context.extend(
            web_identity
                .identity
                .authentication_methods
                .iter()
                .map(|method| (format!("{provider}:{AUTHENTICATION_METHODS_CLAIM}"), method.clone())),
        );
    }
    context
}

/// The WEB IDENTITY FEDERATION a record of a call made with the session this token mints
/// names: the provider that issued the token, and the identity that token established.
///
/// The provider is the identifier that provider is KNOWN BY
/// ([`super::web_identity_provider_name`]), the same spelling the condition keys are named
/// after, and the attributes are the facts CloudTrail reports a federated session's identity
/// by — the audience the token was issued for, the identity it names and the authentication
/// methods it was issued under — each under the `<provider>:<claim>` key the exchange
/// published it as ([`condition_context`]), so what an auditor reads off a record is what the
/// trust-policy decision was taken over. The authentication methods are a LIST claim, so they
/// are one attribute holding their JSON array, there being one string per key; a claim the
/// token carried no value for contributes no attribute at all.
#[must_use]
pub fn recorded_federation(
    web_identity: &WebIdentity,
) -> ::service_common::management_events::RecordedWebIdentityFederation {
    let provider = super::web_identity_provider_name(&web_identity.identity.issuer);
    let mut attributes: Vec<(String, String)> = Vec::new();
    for (claim, value) in [
        (AUDIENCE_CLAIM, web_identity.identity.audience.clone()),
        (SUBJECT_CLAIM, web_identity.identity.subject.clone()),
    ] {
        if !value.is_empty() {
            attributes.push((format!("{provider}:{claim}"), value));
        }
    }
    if !web_identity.identity.authentication_methods.is_empty() {
        attributes.push((
            format!("{provider}:{AUTHENTICATION_METHODS_CLAIM}"),
            json_array(&web_identity.identity.authentication_methods),
        ));
    }
    ::service_common::management_events::RecordedWebIdentityFederation {
        federated_provider: provider.to_string(),
        attributes,
    }
}

/// `values` as the text of a JSON array of strings, which is how a multi-valued claim is
/// carried by a member that holds one string per key.
fn json_array(values: &[String]) -> String {
    ::serde_json::Value::Array(
        values
            .iter()
            .map(|value| ::serde_json::Value::String(value.clone()))
            .collect(),
    )
    .to_string()
}

/// The keys [`condition_context`] publishes, as the policy evaluator needs them: the keys
/// whose absence from the context means the token carried no such value rather than this
/// emulator not knowing the key.
///
/// A key the provider publishes no value for is not among them — an issuer that puts no
/// `amr` claim in its tokens publishes no such key either, so a clause written over it stays
/// undecided and withholds the grant rather than reading as "the token carried none".
#[must_use]
pub fn published_condition_keys(web_identity: &WebIdentity) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (key, _) in condition_context(web_identity) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// The values a claim publishes as a condition key, as the text a policy clause compares
/// against: one value for a scalar claim (a string, a number — a NumericDate expiry is one —
/// or a boolean), one per entry for a claim carrying a list of them, and none at all for an
/// object, which has no scalar to compare.
fn scalar_values(value: &::serde_json::Value) -> Vec<String> {
    match value {
        ::serde_json::Value::String(text) => vec![text.clone()],
        ::serde_json::Value::Number(number) => vec![number.to_string()],
        ::serde_json::Value::Bool(flag) => vec![flag.to_string()],
        ::serde_json::Value::Array(values) => {
            values.iter().flat_map(scalar_values).collect()
        }
        _ => Vec::new(),
    }
}

/// The identity `presented` stands for at `now`, or the class it is refused as.
///
/// The order is the one a verifier applies, and each step is decided from what the step
/// before it established:
///
/// 1. a token THIS emulator issued resolves to the issuance it recorded — the evidence it
///    holds in place of a signature, because it publishes no verification key of its own,
///    and it is federated only while the ACCOUNT can still resolve its issuer as an
///    identity provider (step 6's rule, which is a fact about the account rather than
///    about the token, so it holds for an issuance of this emulator's own as much as for a
///    signed token);
/// 2. a value that is not a compact JSON Web Token, or that carries no readable `exp` or
///    `iss`, is [`TokenDefect::Unreadable`];
/// 3. a token whose `exp` has passed is [`TokenDefect::Expired`] — an expired token is
///    expired WHATEVER its signature, so this is decided before the signature is;
/// 4. a token nothing signed is [`TokenDefect::Unsigned`], which is decided from the token
///    alone and so needs no provider;
/// 5. a token the caller's OWN partition minted — one vended against the account's outbound
///    web identity issuer — is [`TokenDefect::StsIssued`], which this exchange does not
///    serve, decided before the account's providers are consulted because no provider for
///    that issuer can be registered;
/// 6. a token whose issuer the account registered no provider for is
///    [`TokenDefect::UnregisteredIssuer`]; and
/// 7. a token of a registered issuer is verified against the key set that issuer publishes
///    in this process, accepted when the signature checks out and otherwise refused as the
///    signature having failed ([`TokenDefect::NotValidated`]) or as the key not being
///    readable at all ([`TokenDefect::KeyUnretrievable`]), which are different answers to
///    the caller.
pub fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    presented: &str,
    now: SystemTime,
) -> Result<WebIdentity, TokenDefect> {
    if let Some(issued) = super::issued_web_identity_tokens(engine)
        .and_then(|registry| registry.lock().issued(presented, now).cloned())
    {
        // WHICH providers the account can federate is the account's own record, not the
        // token's: an issuance this emulator made is exchanged only while the issuer it
        // names is one the account resolves — the provider it registered for that issuer,
        // or Amazon Cognito Identity's own, which every account resolves. So a token
        // minted by an issuer this emulator hosts (an Amazon EKS cluster's Kubernetes API
        // server) is federated once `iam:CreateOpenIDConnectProvider` registered that
        // cluster's issuer, and named as unregistered until it was.
        if !registered_issuer(engine, &issued.issuer) {
            return Err(TokenDefect::UnregisteredIssuer(issued.issuer));
        }
        return Ok(WebIdentity {
            identity: issued,
            claims: ::serde_json::Map::new(),
        });
    }

    let Ok(token) = jwt::read_unsecured(presented) else {
        return Err(TokenDefect::Unreadable);
    };
    let Some(expiry) = token.claims.get(EXPIRY_CLAIM).and_then(numeric_date) else {
        return Err(TokenDefect::Unreadable);
    };
    let at = unix_seconds(now);
    if at >= expiry {
        return Err(TokenDefect::Expired { at, expiry });
    }
    let issuer = token.claim_str(ISSUER_CLAIM).unwrap_or_default().trim();
    if issuer.is_empty() {
        // An OpenID Connect ID token is required to name the principal that issued it
        // (RFC 7519 §4.1.1), and a token naming none cannot be read against any provider's
        // keys or condition keys at all.
        return Err(TokenDefect::Unreadable);
    }
    // The unsecured form is the shared reader's own judgement, so this module holds no
    // second copy of it: `read` refuses exactly the token nothing signed.
    if matches!(jwt::read(presented), Err(jwt::FormDefect::Unsecured)) {
        return Err(TokenDefect::Unsigned);
    }
    // A token the caller's OWN partition minted: web identity federation hosted inside the
    // partition is outbound only, so a JWT vended against the account's own issuer is not
    // exchanged back here — and the caller is told that class rather than that the issuer is
    // unregistered, which it cannot act on, because no provider for that issuer can be
    // registered in the partition at all.
    if partition_issued(engine, issuer) {
        return Err(TokenDefect::StsIssued);
    }
    if !registered_issuer(engine, issuer) {
        return Err(TokenDefect::UnregisteredIssuer(issuer.to_string()));
    }
    let Some(key_set) = published_key_set(engine, issuer) else {
        // The issuer publishes its key set at its own endpoint, outside this emulator, and
        // nothing here can fetch it: the signature is unchecked, so the token must not be
        // federated — and the caller is told the key could not be retrieved rather than that
        // its token was bad.
        return Err(TokenDefect::KeyUnretrievable);
    };
    match jwt::verify(presented, &key_set) {
        Ok(verified) => Ok(identity_of(&verified, expiry)),
        // Both say the same thing to a caller: no key this issuer publishes signed this
        // token.
        Err(jwt::SignatureDefect::Invalid | jwt::SignatureDefect::UnknownKey) => {
            Err(TokenDefect::NotValidated)
        }
        // The check could not be MADE under the algorithm the token names or the key the
        // issuer published for it, so nothing was decided about the token: that is the
        // key-retrieval class, not a signature that failed.
        Err(
            jwt::SignatureDefect::UnsupportedAlgorithm(_) | jwt::SignatureDefect::UnsupportedKey,
        ) => Err(TokenDefect::KeyUnretrievable),
        // The form rules, already decided above; stated as the class they are.
        Err(jwt::SignatureDefect::Form(jwt::FormDefect::Unsecured)) => Err(TokenDefect::Unsigned),
        Err(jwt::SignatureDefect::Form(_)) => Err(TokenDefect::Unreadable),
    }
}

/// The identity a VERIFIED token names: the three claims the exchange reports back
/// (`Provider`, `Audience`, `SubjectFromWebIdentityToken`), the authentication methods it
/// carries, and its own expiry — beside the whole claim set, which is what a trust policy's
/// conditions are decided against.
///
/// A claim the token does not carry reads as absent rather than as a refusal: the issuer
/// signed exactly these claims, and what a trust policy makes of a missing `aud` or `sub` is
/// that policy's decision — a clause written over the condition key it would publish stays
/// undecided and withholds the grant.
fn identity_of(token: &jwt::CompactJws, expiry: i64) -> WebIdentity {
    WebIdentity {
        identity: IssuedWebIdentity {
            issuer: token.claim_str(ISSUER_CLAIM).unwrap_or_default().to_string(),
            audience: audience_of(token),
            subject: token.claim_str(SUBJECT_CLAIM).unwrap_or_default().to_string(),
            authentication_methods: token
                .claims
                .get(AUTHENTICATION_METHODS_CLAIM)
                .map(authentication_methods)
                .unwrap_or_default(),
            principal_tags: claimed_principal_tags(&token.claims),
            // An instant past what the clock type can represent is not an expiry any
            // comparison can be made against, so it reads as the epoch rather than
            // overflowing: the token was admitted by the comparison above, which was made on
            // the claim's own seconds.
            expires_at: UNIX_EPOCH
                .checked_add(Duration::from_secs(u64::try_from(expiry).unwrap_or(0)))
                .unwrap_or(UNIX_EPOCH),
        },
        claims: token.claims.clone(),
    }
}

/// The PRINCIPAL TAGS a verified token carries: the pairs its [`SESSION_TAGS_CLAIM`] states,
/// in tag-key order, each with the first value its member carries (a tag is one value, and
/// the claim writes it as a list of them).
///
/// Bounded by what one session may hold ([`super::MAX_SESSION_TAGS`], "You can pass up to 50
/// session tags", `AssumeRole$Tags`): the tags of the session this token mints are kept in
/// that session's record, so a claim naming more than a session can hold contributes only
/// the tags, in key order, that fit.
///
/// A token carrying no such claim, or carrying it as anything but the documented structure,
/// states no tag at all: there is no pair for a session to be tagged with.
fn claimed_principal_tags(
    claims: &::serde_json::Map<String, ::serde_json::Value>,
) -> Vec<(String, String)> {
    let Some(::serde_json::Value::Object(tags)) = claims.get(SESSION_TAGS_CLAIM) else {
        return Vec::new();
    };
    let Some(::serde_json::Value::Object(principal_tags)) = tags.get(PRINCIPAL_TAGS_MEMBER)
    else {
        return Vec::new();
    };
    principal_tags
        .iter()
        .filter_map(|(tag_key, value)| {
            Some((tag_key.clone(), scalar_values(value).into_iter().next()?))
        })
        // The member order of the claim's own document is the ISSUER's; the tags of a session
        // are held in tag-key order, so they are ordered by key here — which is also what
        // makes the ceiling below cut at the same tags for one claim however that document
        // happened to be written.
        .collect::<::std::collections::BTreeMap<String, String>>()
        .into_iter()
        .take(super::MAX_SESSION_TAGS)
        .collect()
}

/// The audience a token claims. RFC 7519 §4.1.3 writes `aud` as a string, or as an array of
/// strings "in the general case where the claim value is an array"; a token issued to one
/// client carries that one value either way, so the first entry of an array is the audience
/// it names.
fn audience_of(token: &jwt::CompactJws) -> String {
    match token.claims.get(AUDIENCE_CLAIM) {
        Some(::serde_json::Value::String(audience)) => audience.clone(),
        Some(::serde_json::Value::Array(audiences)) => audiences
            .iter()
            .find_map(::serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// The authentication methods a claim carries: RFC 8176 writes `amr` as an array of method
/// names, and a token writing a single name reads as that one method.
fn authentication_methods(claim: &::serde_json::Value) -> Vec<String> {
    match claim {
        ::serde_json::Value::String(method) => vec![method.clone()],
        ::serde_json::Value::Array(methods) => methods
            .iter()
            .filter_map(::serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether `issuer` is the web identity issuer the account's OUTBOUND federation vends
/// against: the issuer of the caller's own partition, which this emulator hosts and
/// `sts:GetWebIdentityToken` signs tokens for.
///
/// Asked of IAM's own record in the same engine
/// (`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State::holds_web_identity_issuer`),
/// which is the one reading of "this is the partition's own issuer" — the same one IAM's
/// provider registration refuses such a URL by — whatever the feature's flag stands at: a
/// token vended while it was on stays a token of that issuer after it has been turned off.
fn partition_issued<Eng: Send + Sync + 'static>(engine: &Eng, issuer: &str) -> bool {
    super::iam_state(engine).is_some_and(|iam| iam.lock().holds_web_identity_issuer(issuer))
}

/// Whether the ACCOUNT can resolve `issuer` as an identity provider at all.
///
/// Two ways it can: the account registered an IAM OpenID Connect provider for that issuer —
/// `iam:CreateOpenIDConnectProvider` keys one by the ARN its URL derives
/// ([`super::web_identity_provider_name`] drops the scheme the same way) — or the issuer is
/// Amazon Cognito Identity's own, which every account's identity pools are issued by and
/// which is registered nowhere. Read from IAM's own provider records in the same engine, so
/// registering a provider is what makes its tokens resolvable.
pub fn registered_issuer<Eng: Send + Sync + 'static>(engine: &Eng, issuer: &str) -> bool {
    let provider = super::web_identity_provider_name(issuer);
    if provider.is_empty() {
        return false;
    }
    if provider == super::COGNITO_IDENTITY_PROVIDER_NAME {
        return true;
    }
    super::iam_state(engine).is_some_and(|iam| {
        iam.lock()
            .open_id_connect_providers
            .values()
            .any(|record| super::web_identity_provider_name(&record.url) == provider)
    })
}

/// The key set `issuer` publishes, read from the service that OWNS that issuer in this same
/// shared engine.
///
/// A verifier checks a token against the key its header names in the key set the token's
/// ISSUER publishes. The issuers this emulator serves are its own Amazon Cognito user pools:
/// each publishes its key set at `<issuer>/.well-known/jwks.json` and holds it in the
/// user-pool service's state in this engine — the one store that endpoint serves it from — so
/// the set is read from there rather than fetched over a network this emulator never leaves.
/// Only PUBLIC key parameters are read: no private key material crosses this seam.
///
/// `None` when nothing in this process publishes a key set for `issuer`, which is every
/// issuer outside the emulator.
///
/// LOCK ORDER: this takes the user-pool service's state lock and nothing else, and the
/// session registry's lock is not held when a token is resolved. Nothing in the user-pool
/// service reads this service's state, so that order is the only one taken.
#[must_use]
pub fn published_key_set<Eng: Send + Sync + 'static>(
    engine: &Eng,
    issuer: &str,
) -> Option<::serde_json::Value> {
    let pools = ::service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
        ::state::service_state(engine)?;
    // A user pool's issuer URL ends in the pool's own id, which is how the pool that
    // publishes a key set is found; the whole URL still has to agree, because a pool answers
    // only for an issuer it publishes — one naming an endpoint this emulator serves that
    // pool at, which is the pool's own reading.
    let user_pool_id = issuer.trim_end_matches('/').rsplit('/').next()?.to_string();
    let pools = pools.lock();
    let pool = pools.pool(&user_pool_id)?;
    pool.publishes_issuer(issuer).then(|| pool.jwks())
}

/// `at` as whole seconds since the Unix epoch, the unit a token's expiry claim is written in
/// (RFC 7519 §2's NumericDate).
fn unix_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A claim written as RFC 7519 §2's NumericDate, in whole seconds.
///
/// A NumericDate is "a JSON numeric value representing the number of seconds from
/// 1970-01-01T00:00:00Z UTC until the specified UTC date/time, ignoring leap seconds" where
/// "non-integer values can be used" (§2) — an issuer that writes a fractional instant has
/// written a valid one, so it is read and truncated to the whole seconds the refusal states.
/// A claim that is not a JSON number at all is no NumericDate and reads as absent.
fn numeric_date(value: &::serde_json::Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return Some(seconds);
    }
    let seconds = value.as_f64()?;
    if !seconds.is_finite() {
        return None;
    }
    // A float-to-integer cast saturates at the integer's bounds rather than wrapping, so an
    // instant beyond them reads as the nearest representable one.
    Some(seconds.trunc() as i64)
}

#[cfg(test)]
mod tests {
    use super::{claimed_principal_tags, SESSION_TAGS_CLAIM};

    /// The claim set a token carrying `tags` under the session-tags claim has.
    fn claims(tags: ::serde_json::Value) -> ::serde_json::Map<String, ::serde_json::Value> {
        let mut claims = ::serde_json::Map::new();
        claims.insert("sub".to_string(), ::serde_json::json!("user-1"));
        claims.insert(SESSION_TAGS_CLAIM.to_string(), tags);
        claims
    }

    /// One row of the table below: the case, the token's claim set, and the tags it carries.
    type ClaimRow = (
        &'static str,
        ::serde_json::Map<String, ::serde_json::Value>,
        Vec<(&'static str, &'static str)>,
    );

    /// RULE: the principal tags a verified token carries are the `principal_tags` member of
    /// its `https://aws.amazon.com/tags` claim, one value per tag key, in tag-key order; a
    /// token carrying that claim in any other shape carries no tag at all.
    ///
    /// The rows differ along what the rule generalises over: how many tags the claim states,
    /// how the claim writes a value (the documented list, a bare scalar, a non-scalar), and
    /// the shapes that state no tag (claim absent, claim not an object, member missing).
    #[test]
    fn a_tokens_session_tags_claim_states_its_principal_tags() {
        // (case, the token's claim set, the tags it carries)
        let rows: Vec<ClaimRow> = vec![
            (
                "one tag, written the documented way",
                claims(::serde_json::json!({"principal_tags": {"department": ["engineering"]}})),
                vec![("department", "engineering")],
            ),
            (
                "several tags, reported in tag-key order",
                claims(::serde_json::json!({
                    "principal_tags": {"tenant": ["acme"], "department": ["engineering"]},
                    "transitive_tag_keys": ["tenant"],
                })),
                vec![("department", "engineering"), ("tenant", "acme")],
            ),
            (
                "a value written as a bare string",
                claims(::serde_json::json!({"principal_tags": {"tenant": "acme"}})),
                vec![("tenant", "acme")],
            ),
            (
                "a value list naming several values: the tag takes the first",
                claims(::serde_json::json!({"principal_tags": {"tenant": ["acme", "globex"]}})),
                vec![("tenant", "acme")],
            ),
            (
                "a value that is no scalar at all",
                claims(::serde_json::json!({"principal_tags": {"tenant": {"name": "acme"}}})),
                Vec::new(),
            ),
            (
                "the claim carrying no principal tags member",
                claims(::serde_json::json!({"transitive_tag_keys": ["tenant"]})),
                Vec::new(),
            ),
            (
                "the member written as something other than a map",
                claims(::serde_json::json!({"principal_tags": ["tenant"]})),
                Vec::new(),
            ),
            (
                "the claim written as something other than an object",
                claims(::serde_json::json!("tenant=acme")),
                Vec::new(),
            ),
            ("no such claim at all", ::serde_json::Map::new(), Vec::new()),
        ];

        for (case, claims, expected) in rows {
            let actual = claimed_principal_tags(&claims);
            let expected: Vec<(String, String)> = expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a claim naming more tags than one session may hold contributes only the tags, in
    /// key order, that fit — the ceiling is the one the model states for a session's tags.
    #[test]
    fn a_claim_past_the_session_tag_ceiling_contributes_only_what_fits() {
        let ceiling = super::super::MAX_SESSION_TAGS;
        // The rows walk the bound: under it, exactly at it, and one and ten past it.
        for named in [1, ceiling - 1, ceiling, ceiling + 1, ceiling + 10] {
            let mut principal_tags = ::serde_json::Map::new();
            for index in 0..named {
                // Keyed so the order is the tag-key order the reader reports.
                principal_tags.insert(format!("tag{index:04}"), ::serde_json::json!(["value"]));
            }
            let carried = claimed_principal_tags(&claims(::serde_json::json!({
                "principal_tags": ::serde_json::Value::Object(principal_tags),
            })));
            let expected = named.min(ceiling);
            assert_eq!(
                carried.len(),
                expected,
                "case {named} tag(s) named: expected {expected} carried, got {}",
                carried.len()
            );
            for (position, (key, value)) in carried.iter().enumerate() {
                assert_eq!(
                    (key.as_str(), value.as_str()),
                    (format!("tag{position:04}").as_str(), "value"),
                    "case {named} tag(s) named: expected the tags in key order, got {carried:?}"
                );
            }
        }
    }
}
