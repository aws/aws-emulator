// SPDX-License-Identifier: CC0-1.0

//! The tokens a pool issues: how one is minted, and the key its JWTs are signed
//! with.

use super::*;

/// A fresh refresh token: the opaque credential a completed sign-in hands its
/// caller, and the value a later refresh presents back.
///
/// "Your user's refresh token" (`AuthenticationResultType.RefreshToken` in this
/// service's model) is a bearer credential and nothing else: unlike the access
/// and id tokens beside it, it states no claim its holder can read, and
/// everything this service knows about it lives in the [`TokenRecord`] keyed by
/// it. It is therefore minted by the one component that generates secrets — the
/// emulator's credential store — and then removed from that store again: the
/// caller gets the secret, the emulator keeps no credential it would accept as a
/// request signature, and nothing accumulates per sign-in.
///
/// That secret is then SEALED under `key` and handed over in the JWE compact
/// serialization Amazon Cognito vends a refresh token in
/// ([`RefreshTokenKey::seal`]): the form is the published JOSE one, five
/// `.`-separated base64url segments inside the modelled `TokenModelType` pattern
/// `^[A-Za-z0-9-_=.]+$`, and the credential inside it stays unreadable to its
/// holder.
///
/// `None` when the store minted no session-token secret, when the minted
/// credential could not be removed from the store again — a credential the
/// emulator would still accept as a request signature is not the token a sign-in
/// promises, so it is refused rather than handed out, and the failed removal is
/// logged — or when the credential could not be sealed. Either way the sign-in
/// refuses rather than reporting a credential nothing issued.
pub fn mint_refresh_token(
    store: &::emulator_server::CredentialStore,
    key: &RefreshTokenKey,
) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secret = minted.secret(::emulator_server::aws_credentials::SESSION_TOKEN);
    // The refresh token authenticates to this service's own token exchange, not
    // to the emulator's request signing; see above.
    if let Err(error) = store.delete(&minted.id) {
        ::tracing::warn!(
            access_key_id = %minted.id,
            error = ?error,
            "a credential minted for a refresh token could not be deleted"
        );
        return None;
    }
    key.seal(secret?.as_bytes())
}

/// `len` random hex characters, drawn from v4 UUIDs (122 bits of entropy each)
/// with their hyphens removed, so the result satisfies `^[\w+]+$`.
pub(super) fn random_token(len: usize) -> String {
    let mut token = String::with_capacity(len + 32);
    while token.len() < len {
        token.extend(new_uuid().chars().filter(|c| *c != '-'));
    }
    token.truncate(len);
    token
}

/// `len` random characters of [`ALPHANUMERIC_ALPHABET`], drawn from the same
/// randomness [`random_token`] draws on: each byte of a v4 UUID, read as the hex
/// pair it is rendered as, picks one character of the alphabet. A byte past the
/// last whole multiple of the alphabet is DROPPED rather than folded, so no
/// character is drawn more often than another.
pub(super) fn random_alphanumeric_token(len: usize) -> String {
    let alphabet = ALPHANUMERIC_ALPHABET.len();
    // The byte values that leave the draw uniform: the ones below the last whole
    // multiple of the alphabet's size.
    let drawable = 256 - 256 % alphabet;
    let mut token = String::with_capacity(len);
    while token.len() < len {
        let hex = random_token(len.saturating_sub(token.len()).saturating_mul(4));
        for pair in hex.as_bytes().chunks(2) {
            let drawn = ::std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .map(usize::from)
                .filter(|drawn| *drawn < drawable);
            if let Some(drawn) = drawn {
                token.push(char::from(ALPHANUMERIC_ALPHABET[drawn % alphabet]));
            }
        }
    }
    token.truncate(len);
    token
}

/// The 62 characters a mixed-case alphanumeric token is drawn from.
pub(super) const ALPHANUMERIC_ALPHABET: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// What one sign-in's tokens are built from, read from the stored pool, app
/// client and user under the state lock before any token is minted.
pub(super) struct TokenInputs {
    /// The pool's token issuer (its regional user-pool endpoint).
    pub(super) issuer: String,
    /// The stored user's immutable id, the `sub` claim.
    pub(super) sub: String,
    /// The username as STORED, which a case-insensitive pool may case
    /// differently from the request.
    pub(super) stored_username: String,
    /// The stored user's own attributes, as the claims an IDENTITY token carries
    /// them ([`attribute_claims`]).
    pub(super) attribute_claims: ::serde_json::Map<String, ::serde_json::Value>,
    /// The names of the pool's groups the stored user belongs to, in stored order.
    pub(super) groups: Vec<String>,
    /// What those groups' roles and precedences put in the identity token.
    pub(super) role_claims: GroupRoleClaims,
    /// The pool's own token-signing keys: each token of this sign-in is signed
    /// with the key of its own kind and names it by `kid`.
    pub(super) signing_keys: PoolSigningKeys,
    pub(super) access_ttl: Duration,
    pub(super) id_ttl: Duration,
    pub(super) refresh_ttl: Duration,
}

/// The claims an IDENTITY token carries of the sign-in itself, which no stored
/// attribute of the same name stands in for.
///
/// The group and role claims are among them: a user's GROUPS decide
/// `cognito:groups`, `cognito:roles` and `cognito:preferred_role`, so a stored
/// attribute of one of those names supplies none of them.
const MINTED_ID_CLAIMS: [&str; 13] = [
    "sub",
    "iss",
    "aud",
    "cognito:username",
    "token_use",
    "auth_time",
    "iat",
    "exp",
    "jti",
    "origin_jti",
    "cognito:groups",
    "cognito:roles",
    "cognito:preferred_role",
];

/// The attributes whose claim is a BOOLEAN, although every attribute VALUE is
/// carried as text: the two flags this API verifies are `Boolean` schema
/// attributes ([`super::FLAG_CONFIRMED`] is the string `"true"`), and the claim
/// of such an attribute is the boolean it states.
const BOOLEAN_ID_CLAIMS: [&str; 2] = ["email_verified", "phone_number_verified"];

/// The claims the stored `attributes` of a user put in an IDENTITY token.
///
/// An identity token "contains claims about the identity of the authenticated
/// user, such as `name`, `family_name`, and `phone_number`"
/// (`AuthenticationResultType.IdToken` in this service's model), so the user's
/// own attributes ARE those claims: each travels under the name it is stored
/// under — which for a custom attribute is already its `custom:` name — and the
/// two verification flags travel as the boolean each states rather than as the
/// text an attribute value is carried in. The claims the sign-in mints itself
/// ([`MINTED_ID_CLAIMS`]) are the sign-in's own and are never taken from an
/// attribute.
pub(super) fn attribute_claims(
    attributes: &BTreeMap<String, String>,
) -> ::serde_json::Map<String, ::serde_json::Value> {
    attributes
        .iter()
        .filter(|(name, _value)| !MINTED_ID_CLAIMS.contains(&name.as_str()))
        .map(|(name, value)| {
            let claim = if BOOLEAN_ID_CLAIMS.contains(&name.as_str()) {
                ::serde_json::Value::Bool(value == super::FLAG_CONFIRMED)
            } else {
                ::serde_json::Value::String(value.clone())
            };
            (name.clone(), claim)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Token signing
//
// Amazon Cognito signs every token a user pool issues against a key pair
// belonging to THAT pool, and publishes the verifying half as a JSON Web Key
// Set: a verifier must "decode the token ... and verify the signature" against
// the key the token's `kid` names in the pool's `jwks.json` (Amazon Cognito
// Developer Guide, "Verifying a JSON web token"). Every consumer of such a
// token — Verified Permissions, an API Gateway JWT authorizer, any JOSE
// library — reads `alg` and `kid` from the header, finds the matching JWK and
// checks the signature over `base64url(header) '.' base64url(claims)`, so a
// token whose third segment is EMPTY is unusable: it is not even a
// syntactically valid signed JWT.
//
// A pool signs with RS256 (RSASSA-PKCS1-v1_5 over SHA-256, RFC 7518 §3.3),
// which is the one algorithm the pool's own OpenID discovery document
// advertises (`id_token_signing_alg_values_supported`), and it holds a SEPARATE
// key pair per token type: the access token and the identity token name
// different `kid`s of the same published key set, so a verifier can pin one
// token type's key without accepting the other's.
//
// What a signing key IS — the key pair, the `kid` that names it, the public JWK
// the key set publishes and the compact serialization a signed token travels in
// — is the workspace's one JOSE signing seam
// (`::service_common::jws_signing`), beside the reading that verifies such a
// token, so nothing of it is laid out here. What this pool decides is only that
// it holds one key per token type.
// ---------------------------------------------------------------------------

/// The `alg` of every token this service signs, and of the JWK it publishes for
/// the key that signed it: RSASSA-PKCS1-v1_5 using SHA-256 (RFC 7518 §3.1/§3.3),
/// named by the seam that signs under it.
pub(super) const TOKEN_SIGNING_ALGORITHM: &str = ::service_common::jws_signing::RS256;

/// Which of a pool's two token-signing keys a token is signed with.
///
/// A pool keeps one key pair per token type, so the two kinds of token name
/// different keys of the one published key set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningKeyUse {
    /// The key the pool's ACCESS tokens are signed with, including the
    /// machine-to-machine access token an app client's own credentials earn.
    AccessToken,
    /// The key the pool's IDENTITY tokens are signed with.
    IdToken,
}

/// One user pool's token-signing keys: one key pair per token type, both
/// published in the pool's key set.
///
/// The keys belong to the pool, so a token one pool issued carries one pool's
/// `kid` and verifies against only that pool's JWKS.
#[derive(Debug, Clone)]
pub struct PoolSigningKeys {
    /// The key pair the pool's access tokens are signed with.
    access_token: ::service_common::jws_signing::SigningKey,
    /// The key pair the pool's identity tokens are signed with.
    id_token: ::service_common::jws_signing::SigningKey,
}

impl PoolSigningKeys {
    /// A pool's freshly generated keys: one key pair per token type, so the two
    /// token types of one sign-in are signed by DIFFERENT keys.
    ///
    /// Two key generations of the signing seam's own modulus size, which is
    /// CPU-bound work: the caller runs this on the blocking pool rather than on
    /// the worker serving the request. INFALLIBLE — the only failure the seam
    /// reports is the cryptography library failing to produce or encode a key,
    /// which a pool cannot be created without.
    #[must_use]
    pub fn generate() -> Self {
        let generate = || {
            ::service_common::jws_signing::SigningKey::generate(TOKEN_SIGNING_ALGORITHM)
                .expect("the library yields an RSA key pair")
        };
        Self {
            access_token: generate(),
            id_token: generate(),
        }
    }

    /// The key the token `key_use` names is signed with.
    #[must_use]
    pub fn for_use(
        &self,
        key_use: SigningKeyUse,
    ) -> &::service_common::jws_signing::SigningKey {
        match key_use {
            SigningKeyUse::AccessToken => &self.access_token,
            SigningKeyUse::IdToken => &self.id_token,
        }
    }

    /// The PUBLIC half of every key this pool signs with, as the entries of its
    /// published key set, in a stable order.
    #[must_use]
    pub fn to_jwks_entries(&self) -> Vec<serde_json::Value> {
        vec![self.access_token.to_jwk(), self.id_token.to_jwk()]
    }

    /// Those entries as the key set document itself, through the shared rendering of it
    /// ([`::service_common::jws_signing::key_set`]), so the envelope is not laid out here.
    #[must_use]
    pub fn to_key_set(&self) -> serde_json::Value {
        ::service_common::jws_signing::key_set([&self.access_token, &self.id_token])
    }
}

/// `claims` as a JWS in compact serialization (RFC 7515 §3.1) signed with the
/// pool key `key`, through the one assembly of such a token this workspace holds
/// ([`::service_common::jws_signing::SigningKey::signed_compact_jws`]): the
/// header names the algorithm and the key (`kid`) a verifier checks the
/// signature with.
///
/// `None` when the signature could not be computed, so a token is either
/// signed or not issued.
pub(super) fn signed_jws(
    claims: &serde_json::Value,
    key: &::service_common::jws_signing::SigningKey,
) -> Option<String> {
    key.signed_compact_jws(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a user's own attributes are the claims of its identity token, each under the
    /// name it is stored under, with the two verification flags as the boolean each states
    /// and the sign-in's own claims never taken from an attribute.
    ///
    /// The rows span what the rule generalises over: a standard text attribute, each
    /// verification flag in each of its two states, a custom attribute, the immutable id the
    /// sign-in mints itself, and an attribute whose value is empty.
    #[test]
    fn a_users_attributes_are_its_identity_tokens_claims() {
        let attributes: BTreeMap<String, String> = [
            ("sub", "11111111-2222-3333-4444-555555555555"),
            ("email", "reader@example.test"),
            ("email_verified", FLAG_CONFIRMED),
            ("phone_number", "+15550100"),
            ("phone_number_verified", FLAG_UNCONFIRMED),
            ("given_name", "Ada"),
            ("custom:tier", "gold"),
            ("nickname", ""),
            ("cognito:username", "not-the-stored-username"),
            ("cognito:groups", "not-a-group-of-this-user"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let claims = attribute_claims(&attributes);
        // (case, the claim, what it must be)
        let rows: &[(&str, &str, Option<::serde_json::Value>)] = &[
            (
                "a standard text attribute",
                "email",
                Some(::serde_json::json!("reader@example.test")),
            ),
            (
                "a confirmed verification flag",
                "email_verified",
                Some(::serde_json::json!(true)),
            ),
            (
                "an unconfirmed verification flag",
                "phone_number_verified",
                Some(::serde_json::json!(false)),
            ),
            (
                "another standard attribute",
                "given_name",
                Some(::serde_json::json!("Ada")),
            ),
            (
                "a custom attribute, under its own name",
                "custom:tier",
                Some(::serde_json::json!("gold")),
            ),
            (
                "an attribute whose value is empty",
                "nickname",
                Some(::serde_json::json!("")),
            ),
            // The claims the sign-in mints are the sign-in's own: an attribute of that name
            // states nothing about them.
            ("the immutable id", "sub", None),
            ("the username claim", "cognito:username", None),
            (
                "the group claim, which the user's groups decide",
                "cognito:groups",
                None,
            ),
        ];
        for (case, claim, expected) in rows {
            let held = claims.get(*claim).cloned();
            assert_eq!(
                held, *expected,
                "case {case:?}: expected the claim {claim:?} to be {expected:?}, got {held:?}"
            );
        }
    }
}
