// SPDX-License-Identifier: CC0-1.0

//! The TRUSTED IDENTITY PROPAGATION context a federated mint sets in the session it vends,
//! and the condition keys that session answers for.
//!
//! "The trusted identity propagation context that Amazon Web Services STS set in a session
//! from the identity provider's token. A member is absent when the session has none of its
//! keys" (`TrustedIdentityPropagationContext`, this service's model): the context is not a
//! value a request carries, it is what the TOKEN a federated assume exchanged established
//! about the end user the session acts for. So it is built here, once, out of the evidence
//! the verification already produced — the token's own claims and the identity provider the
//! account holds for its issuer — and stored on the session, which is what makes the
//! `aws:OriginIdentity/…` keys of a later request answerable
//! ([`condition_context`]).
//!
//! Which key each member carries is the model's own definition:
//!
//! * `Subject` is "the end user's identifier, taken from the token claim that the context
//!   provider maps as the subject", which is the subject claim of the token presented — RFC
//!   7519 §4.1.2's `sub` for a web identity, and the `NameID` of the assertion's subject for
//!   a SAML 2.0 authentication response;
//! * `Issuer` is, for `AssumeRoleWithWebIdentity`, "the `iss` claim of the token without the
//!   `https://` prefix" — the form the account registers an identity provider under
//!   ([`super::web_identity_provider_name`]) — and for `AssumeRoleWithSAML` "a hash of the
//!   key that signed the SAML assertion, not the `Issuer` element of the assertion"
//!   ([`super::saml::SamlAssertion::signing_key_digest`]); and
//! * `FederatedProviderArn` is "the ARN of the IAM identity provider that the request
//!   federated through", read from the account's own provider records so it names a provider
//!   IAM really holds and is absent for a federation that went through none.
//!
//! Two keys are deliberately NOT set. `SubjectType` ("whether the end user is a person or a
//! non-human caller") and the `aws:TransactionContext/IdentityProvider/*` keys ("additional
//! claims about the request that the identity provider asserted", each "keyed by the part of
//! the key name after `aws:TransactionContext/IdentityProvider/`") are values a context
//! provider MAPS out of the token when it is configured on the identity provider, and no
//! operation of this account's identity service configures such a mapping — so a session has
//! none of those keys, and the members stating them are absent rather than filled with a
//! value nothing asserted.

use crate::ops::types::{OriginIdentity, TrustedIdentityPropagationContext};

/// The condition key carrying the end user's identifier:
/// `OriginIdentity$Subject`'s own key, as its documentation names it.
pub const ORIGIN_IDENTITY_SUBJECT_KEY: &str = "aws:OriginIdentity/Subject";

/// The condition key carrying the type of the end user (`OriginIdentity$SubjectType`).
pub const ORIGIN_IDENTITY_SUBJECT_TYPE_KEY: &str = "aws:OriginIdentity/SubjectType";

/// The condition key carrying the issuer the session's origin identity came from
/// (`OriginIdentity$Issuer`).
pub const ORIGIN_IDENTITY_ISSUER_KEY: &str = "aws:OriginIdentity/Issuer";

/// The condition key carrying the IAM identity provider the request federated through
/// (`OriginIdentity$FederatedProviderArn`).
pub const ORIGIN_IDENTITY_FEDERATED_PROVIDER_ARN_KEY: &str =
    "aws:OriginIdentity/FederatedProviderArn";

/// The context a session carries for `origin`, and `None` when the origin identity holds no
/// key at all — "a member is absent when the session has none of its keys", which for the
/// context itself means the member is not reported.
#[must_use]
pub fn context(origin: OriginIdentity) -> Option<TrustedIdentityPropagationContext> {
    if origin.subject.is_none()
        && origin.subject_type.is_none()
        && origin.issuer.is_none()
        && origin.federated_provider_arn.is_none()
    {
        return None;
    }
    Some(TrustedIdentityPropagationContext {
        origin_identity: Some(origin),
        // See the module documentation: the `aws:TransactionContext/IdentityProvider/*` keys
        // are the ones a context provider maps, and this account configures no mapping, so
        // the session has none of them.
        transaction_context: None,
    })
}

/// The context a WEB IDENTITY exchange sets in the session it mints: the token's subject, the
/// issuer it names with its scheme dropped, and the ARN of the IAM OpenID Connect provider
/// the account holds for that issuer.
///
/// `engine` is read for that provider record alone, so the ARN names a provider
/// `iam:CreateOpenIDConnectProvider` really wrote; a token of an issuer the account resolves
/// without a provider record of its own federates through no IAM identity provider, and the
/// member is absent for it.
#[must_use]
pub fn web_identity_context<Eng: Send + Sync + 'static>(
    engine: &Eng,
    web_identity: &super::web_identity::WebIdentity,
) -> Option<TrustedIdentityPropagationContext> {
    let issuer = super::web_identity_provider_name(&web_identity.identity.issuer);
    context(OriginIdentity {
        subject: stated(&web_identity.identity.subject),
        subject_type: None,
        issuer: stated(issuer),
        federated_provider_arn: open_id_connect_provider_arn(engine, &web_identity.identity.issuer),
    })
}

/// The context a SAML 2.0 exchange sets in the session it mints: the subject the assertion
/// names, the digest of the key that signed that assertion, and the ARN of the IAM SAML
/// provider the response was verified against — the provider the request federated through,
/// which the account holds under that ARN ([`super::saml::registered_provider`]).
#[must_use]
pub fn saml_context(
    provider: &super::saml::SamlProvider,
    assertion: &super::saml::SamlAssertion,
) -> Option<TrustedIdentityPropagationContext> {
    context(OriginIdentity {
        subject: stated(&assertion.subject),
        subject_type: None,
        issuer: stated(&assertion.signing_key_digest),
        federated_provider_arn: stated(&provider.arn),
    })
}

/// The ARN the account holds its OpenID Connect provider for `issuer` under, read from IAM's
/// own records in the same engine; `None` when it holds none.
#[must_use]
fn open_id_connect_provider_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    issuer: &str,
) -> Option<String> {
    let named = super::web_identity_provider_name(issuer);
    if named.is_empty() {
        return None;
    }
    let iam = super::iam_state(engine)?;
    let iam = iam.lock();
    iam.open_id_connect_providers
        .iter()
        .find(|(_, record)| super::web_identity_provider_name(&record.url) == named)
        .map(|(arn, _)| arn.clone())
}

/// The condition keys a session carrying `context` answers for, as `(key, value)` pairs.
///
/// Each member of the context IS a condition key of the session — `OriginIdentity$Subject`
/// is "the value of the `aws:OriginIdentity/Subject` key", and so on for the other three — so
/// a later request the session's credential signs is decided against the very values the
/// response reported. A key the context does not carry is left out, so a clause over it
/// stays undecided and withholds the grant rather than reading as an empty value.
#[must_use]
pub fn condition_context(
    carried: Option<&TrustedIdentityPropagationContext>,
) -> Vec<(String, String)> {
    let Some(origin) = carried.and_then(|carried| carried.origin_identity.as_ref()) else {
        return Vec::new();
    };
    [
        (ORIGIN_IDENTITY_SUBJECT_KEY, origin.subject.as_deref()),
        (
            ORIGIN_IDENTITY_SUBJECT_TYPE_KEY,
            origin.subject_type.as_deref(),
        ),
        (ORIGIN_IDENTITY_ISSUER_KEY, origin.issuer.as_deref()),
        (
            ORIGIN_IDENTITY_FEDERATED_PROVIDER_ARN_KEY,
            origin.federated_provider_arn.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key.to_string(), value.to_string())))
    .collect()
}

/// The keys [`condition_context`] publishes for a session, as the policy evaluator needs
/// them: the keys whose absence from the context means the session does not carry the value
/// rather than this emulator not knowing the key.
#[must_use]
pub fn published_condition_keys(
    carried: Option<&TrustedIdentityPropagationContext>,
) -> Vec<&'static str> {
    const KEYS: [&str; 4] = [
        ORIGIN_IDENTITY_SUBJECT_KEY,
        ORIGIN_IDENTITY_SUBJECT_TYPE_KEY,
        ORIGIN_IDENTITY_ISSUER_KEY,
        ORIGIN_IDENTITY_FEDERATED_PROVIDER_ARN_KEY,
    ];
    let published = condition_context(carried);
    KEYS.into_iter()
        .filter(|key| published.iter().any(|(named, _)| named == key))
        .collect()
}

/// `value` as a member of the context, and `None` for a value the evidence did not state:
/// an empty string is no identifier, no issuer and no ARN, so the key is absent rather than
/// carrying nothing.
fn stated(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}
