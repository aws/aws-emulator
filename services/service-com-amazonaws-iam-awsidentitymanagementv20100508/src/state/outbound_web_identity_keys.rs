// SPDX-License-Identifier: CC0-1.0

//! The verification keys the account's outbound web identity ISSUER signs with, and the
//! discovery document it publishes beside them.
//!
//! The issuer an account is given when it enables the feature "hosts the OpenID Connect
//! (OIDC) discovery endpoints at `/.well-known/openid-configuration` and
//! `/.well-known/jwks.json`"
//! (`EnableOutboundWebIdentityFederationResponse$IssuerIdentifier`), and a JWT vended
//! against that issuer "can be publicly verified using the verification keys published at
//! the issuer's JWKS endpoint" (`GetWebIdentityToken`, the STS model). The key material is
//! therefore the ISSUER's and not one token's: it is minted with the issuer
//! ([`super::ComAmazonawsIamAwsidentitymanagementv20100508State::enable_outbound_web_identity_federation`]),
//! kept across a disable and a re-enable — the disable "does not affect tokens that were
//! issued before the feature was disabled", so a verifier of such a token must still find
//! the key that signed it — and published unchanged by the key set endpoint.
//!
//! What a key IS, how it is generated, how it is published as a JWK and how a token is
//! signed with it is the workspace's one JOSE signing seam
//! ([`::service_common::jws_signing`]), beside the reading that verifies such a token; the
//! two well-known paths and the key set's locator are
//! [`::service_common::openid_discovery`]'s. What is decided HERE is only what belongs to
//! this account: that it holds one key per algorithm STS's `GetWebIdentityToken` admits,
//! and what its discovery document says — which is not the same document a user pool
//! publishes, because an issuer announces the endpoints it itself serves and this one
//! serves no authorization, token or userinfo endpoint.

#[allow(unused_imports)]
use super::*;

/// The JWS algorithms a token vended against this issuer may be signed under: "Valid values
/// are RS256 (RSA with SHA-256) and ES384 (ECDSA using P-384 curve with SHA-384)"
/// (`GetWebIdentityTokenRequest$SigningAlgorithm`, the STS model) — which is every algorithm
/// the shared signing seam can generate a key for, listed in its one order so the published
/// set does not depend on how this file is written.
pub const JWT_SIGNING_ALGORITHMS: &[&str] = ::service_common::jws_signing::SIGNABLE_ALGORITHMS;

/// Every key the account's issuer publishes: one per algorithm a token may be signed under.
///
/// Minted with the issuer and never regenerated, so a relying party that fetched the key
/// set keeps finding the key that signed the tokens it holds. There is no empty value: an
/// issuer exists only once it holds a key for every algorithm its tokens may be signed
/// under, so nothing can publish `{"keys": []}` or a subset of the set
/// `GetWebIdentityToken` documents.
#[derive(Debug, Clone)]
pub struct IssuerSigningKeys {
    keys: Vec<::service_common::jws_signing::SigningKey>,
}

impl IssuerSigningKeys {
    /// One freshly generated key pair per algorithm in [`JWT_SIGNING_ALGORITHMS`], or
    /// `None` when the signing seam could not produce one for any of them — in which case
    /// the caller mints no issuer at all, rather than one that cannot sign an algorithm its
    /// model documents.
    ///
    /// Two key generations, which is CPU-bound work measured in milliseconds, so the CALLER
    /// runs this on the blocking pool with NO state guard held — `spawn_blocking(..).await`
    /// is a real await point, so the keys of one enable cannot starve the worker that serves
    /// health checks and every other request, and a shutdown or a timeout can land while
    /// they are generated. It is run once per account, by the enable that mints the issuer.
    #[must_use]
    pub fn generate() -> Option<Self> {
        let keys = JWT_SIGNING_ALGORITHMS
            .iter()
            .map(|algorithm| ::service_common::jws_signing::SigningKey::generate(algorithm))
            .collect::<Option<Vec<_>>>()?;
        Some(Self { keys })
    }

    /// The key `algorithm` is signed under, or `None` when the issuer publishes none for it
    /// — an algorithm outside [`JWT_SIGNING_ALGORITHMS`], or one whose generation failed.
    #[must_use]
    pub fn for_algorithm(
        &self,
        algorithm: &str,
    ) -> Option<&::service_common::jws_signing::SigningKey> {
        self.keys
            .iter()
            .find(|key| key.algorithm() == algorithm)
    }

    /// The JSON Web Key Set this issuer publishes: the PUBLIC half of every key it signs
    /// with, in the one order [`Self::generate`] built them in.
    #[must_use]
    pub fn to_key_set(&self) -> ::serde_json::Value {
        ::service_common::jws_signing::key_set(&self.keys)
    }
}

/// The OpenID Connect discovery document `issuer` publishes: who the issuer is, where its
/// key set is, and which algorithms a token of its own is signed under.
///
/// Every value is a function of the issuer and of the algorithms this account publishes keys
/// for, so the document a relying party reads and the token it then verifies cannot
/// disagree. It names no authorization, token, revocation or userinfo endpoint, because an
/// STS web identity issuer serves none of them: what it exists for is to let a relying party
/// find the key set.
#[must_use]
pub fn openid_configuration(issuer: &str) -> ::serde_json::Value {
    ::serde_json::json!({
        "issuer": issuer,
        "jwks_uri": ::service_common::openid_discovery::jwks_uri(issuer),
        "id_token_signing_alg_values_supported": JWT_SIGNING_ALGORITHMS,
        "response_types_supported": ["id_token"],
        "subject_types_supported": ["public"],
        "claims_supported": ["aud", "exp", "iat", "iss", "nbf", "sub"],
    })
}
