// SPDX-License-Identifier: CC0-1.0

//! The SIGNING half of a compact JWS, and the key an issuer publishes to have it verified:
//! the counterpart of [`crate::jwt`]'s reading, held here once so that what one emulated
//! issuer signs and what that module verifies cannot diverge.
//!
//! Several emulated services ARE an OIDC issuer — a Cognito user pool mints the identity
//! and access tokens of a sign-in, an account's outbound web identity federation mints the
//! JWT STS vends — and each of them needs exactly the same four things: a key pair of the
//! algorithm it signs under, the `kid` that names it, the PUBLIC half as the JWK its key set
//! publishes, and the compact serialization of RFC 7515 §3.1 over a claim set. None of that
//! is service-specific, so none of it is written per service: an issuer here decides only
//! WHICH algorithms it signs under and WHAT its tokens claim.
//!
//! Only PUBLIC parameters leave this module. [`SigningKey::to_jwk`] carries the parameters
//! the key's family requires and nothing private, the `Debug` rendering names a key by its
//! `kid` and its algorithm alone, and the private half is reachable only through
//! [`SigningKey::sign`], which answers a signature and never the key.
//!
//! An in-process COMPUTATION over data: it opens no listener, spawns nothing and reads
//! nothing off the host.

/// RSASSA-PKCS1-v1_5 using SHA-256 (RFC 7518 §3.3).
pub const RS256: &str = "RS256";

/// ECDSA using P-384 and SHA-384 (RFC 7518 §3.4).
pub const ES384: &str = "ES384";

/// The algorithms a key can be GENERATED for here, lexicographically — the order a key set
/// and a discovery document list them in, so a published set does not depend on how this
/// file is written. A service signs under whichever of them its own API documents.
pub const SIGNABLE_ALGORITHMS: &[&str] = &[ES384, RS256];

/// The `kty` of an RSA key, as the published JWK names it (RFC 7518 §6.3).
const RSA_KEY_TYPE: &str = "RSA";

/// The `kty` of an elliptic-curve key (RFC 7518 §6.2).
const EC_KEY_TYPE: &str = "EC";

/// The `crv` of the curve [`ES384`] is defined over.
const EC_CURVE: &str = "P-384";

/// How many bytes each published coordinate of a P-384 point carries: the curve's full
/// width, which RFC 7518 §6.2.1.2 requires both `x` and `y` to be padded to.
const EC_COORDINATE_BYTES: usize = 48;

/// The uncompressed-point prefix a public EC key is encoded with (SEC 1 §2.3.3); the
/// published JWK carries the two coordinates of that point rather than the point.
const EC_UNCOMPRESSED_POINT_PREFIX: u8 = 0x04;

/// The `use` every published entry carries: these keys verify signatures (RFC 7517 §4.2).
const SIGNATURE_KEY_USE: &str = "sig";

/// The `typ` of the JOSE header of a JWT (RFC 7519 §5.1).
const TOKEN_HEADER_TYPE: &str = "JWT";

/// The modulus size an RSA key pair is generated at: 2048 bits, the width
/// RSASSA-PKCS1-v1_5 with SHA-256 is used at and the smallest a JOSE verifier accepts.
const RSA_KEY_SIZE: ::aws_lc_rs::rsa::KeySize = ::aws_lc_rs::rsa::KeySize::Rsa2048;

/// The private half of one signing key, behind an [`::std::sync::Arc`] because neither key
/// pair type is clonable and the record an issuer holds its keys in is.
#[derive(Clone)]
enum PrivateKey {
    /// The RSA key pair [`RS256`] signs with.
    Rsa(::std::sync::Arc<::aws_lc_rs::signature::RsaKeyPair>),
    /// The P-384 key pair [`ES384`] signs with.
    Ecdsa(::std::sync::Arc<::aws_lc_rs::signature::EcdsaKeyPair>),
}

/// One key an issuer signs with: the private half a token is signed with, and the public
/// half its key set publishes under the `kid` a verifier finds it by.
#[derive(Clone)]
pub struct SigningKey {
    /// The JWS algorithm this key signs under, which is also the `alg` of its published
    /// entry and of the header of every token it signed.
    algorithm: &'static str,
    /// The `kid` the token header carries and the published entry is found by: this key's
    /// RFC 7638 JWK thumbprint ([`crate::jwt::jwk_thumbprint`]), so it is derived from the
    /// key itself rather than minted beside it.
    kid: String,
    /// The PUBLIC key parameters RFC 7638 §3.2 requires of this key's family, which the
    /// `kid` above is the thumbprint OF — so the entry and its name cannot describe
    /// different keys.
    public: ::serde_json::Value,
    private: PrivateKey,
}

/// Renders the key's PUBLIC identity only: private key material is never logged, traced or
/// put in a response, so it is not in this rendering either.
impl ::std::fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("SigningKey")
            .field("alg", &self.algorithm)
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    /// A freshly generated key pair for `algorithm`, or `None` for an algorithm outside
    /// [`SIGNABLE_ALGORITHMS`] and for a cryptography library that could not produce or
    /// encode a key — in which case the caller publishes no key for it and signs nothing,
    /// rather than signing with something that is not a key.
    ///
    /// Generating a key pair is CPU-bound work measured in milliseconds, so a caller mints
    /// an issuer's keys when the issuer is created and never per request.
    #[must_use]
    pub fn generate(algorithm: &str) -> Option<Self> {
        match algorithm {
            RS256 => Self::generate_rsa(),
            ES384 => Self::generate_ecdsa(),
            _ => None,
        }
    }

    /// The RSA half of [`Self::generate`].
    fn generate_rsa() -> Option<Self> {
        use ::aws_lc_rs::encoding::AsDer as _;

        let generated = ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(RSA_KEY_SIZE).ok()?;
        let pkcs8 = generated.as_der().ok()?.as_ref().to_vec();
        let (modulus, exponent) = rsa_public_components(&pkcs8)?;
        let private = ::aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&pkcs8).ok()?;
        // RFC 7638 §3.2 / RFC 7518 §6.3.1: `e`, `kty` and `n`, and nothing else.
        let public = ::serde_json::json!({
            "e": crate::jwt::encode_base64url(&exponent),
            "kty": RSA_KEY_TYPE,
            "n": crate::jwt::encode_base64url(&modulus),
        });
        Some(Self {
            algorithm: RS256,
            kid: crate::jwt::jwk_thumbprint(&public)?,
            public,
            private: PrivateKey::Rsa(::std::sync::Arc::new(private)),
        })
    }

    /// The elliptic-curve half of [`Self::generate`].
    fn generate_ecdsa() -> Option<Self> {
        use ::aws_lc_rs::signature::KeyPair as _;

        let pair = ::aws_lc_rs::signature::EcdsaKeyPair::generate(
            &::aws_lc_rs::signature::ECDSA_P384_SHA384_FIXED_SIGNING,
        )
        .ok()?;
        // SEC 1 §2.3.3: the public key is `0x04 || X || Y`, each coordinate the curve's
        // full width, and RFC 7518 §6.2.1 publishes those two coordinates rather than the
        // point.
        let point = pair.public_key().as_ref().to_vec();
        let (prefix, coordinates) = point.split_first()?;
        if *prefix != EC_UNCOMPRESSED_POINT_PREFIX || coordinates.len() != 2 * EC_COORDINATE_BYTES
        {
            return None;
        }
        let (x, y) = coordinates.split_at(EC_COORDINATE_BYTES);
        // RFC 7638 §3.2: `crv`, `kty`, `x` and `y`, and nothing else.
        let public = ::serde_json::json!({
            "crv": EC_CURVE,
            "kty": EC_KEY_TYPE,
            "x": crate::jwt::encode_base64url(x),
            "y": crate::jwt::encode_base64url(y),
        });
        Some(Self {
            algorithm: ES384,
            kid: crate::jwt::jwk_thumbprint(&public)?,
            public,
            private: PrivateKey::Ecdsa(::std::sync::Arc::new(pair)),
        })
    }

    /// The JWS algorithm this key signs under.
    #[must_use]
    pub fn algorithm(&self) -> &'static str {
        self.algorithm
    }

    /// The `kid` naming this key in the issuer's key set, which the header of a token
    /// signed with it carries so a verifier knows which entry to check it against.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// This key's PUBLIC half as the entry a key set publishes: the parameters its family
    /// requires, with the `alg`, the `kid` and the `use` a verifier reads beside them
    /// (RFC 7517 §4). No private parameter is in it.
    #[must_use]
    pub fn to_jwk(&self) -> ::serde_json::Value {
        let mut jwk = self.public.clone();
        if let Some(members) = jwk.as_object_mut() {
            members.insert(
                "alg".to_string(),
                ::serde_json::Value::String(self.algorithm.to_string()),
            );
            members.insert(
                "kid".to_string(),
                ::serde_json::Value::String(self.kid.clone()),
            );
            members.insert(
                "use".to_string(),
                ::serde_json::Value::String(SIGNATURE_KEY_USE.to_string()),
            );
        }
        jwk
    }

    /// The signature over `signing_input` under this key's own algorithm, in the form
    /// RFC 7518 puts in a JWS's third segment: the modulus's width for RSASSA-PKCS1-v1_5,
    /// and the fixed `R || S` of §3.4 for ECDSA — never the ASN.1 encoding, which no JOSE
    /// verifier reads.
    ///
    /// `None` when the library could not sign, in which case the caller issues no token
    /// rather than one carrying bytes that are not a signature.
    #[must_use]
    pub fn sign(&self, signing_input: &[u8]) -> Option<Vec<u8>> {
        let random = ::aws_lc_rs::rand::SystemRandom::new();
        match &self.private {
            PrivateKey::Rsa(key) => {
                // A PKCS #1 signature is exactly as long as the key's modulus.
                let mut signature = vec![0_u8; key.public_modulus_len()];
                key.sign(
                    &::aws_lc_rs::signature::RSA_PKCS1_SHA256,
                    &random,
                    signing_input,
                    &mut signature,
                )
                .ok()?;
                Some(signature)
            }
            PrivateKey::Ecdsa(key) => {
                Some(key.sign(&random, signing_input).ok()?.as_ref().to_vec())
            }
        }
    }

    /// `claims` as a compact JWS (RFC 7515 §3.1) signed with this key:
    /// `base64url(header) '.' base64url(claims) '.' base64url(signature)`, where the header
    /// names the algorithm and the `kid` a verifier finds the key by in the issuer's
    /// published set.
    ///
    /// The ONE assembly of such a token in this workspace, so what an emulated issuer signs
    /// is exactly what [`crate::jwt::verify`] re-splits and checks.
    ///
    /// `None` when the signature could not be computed, so a token is either SIGNED or not
    /// issued — a third segment that is not a signature is a token no issuer vended.
    #[must_use]
    pub fn signed_compact_jws(&self, claims: &::serde_json::Value) -> Option<String> {
        let header = ::serde_json::json!({
            "alg": self.algorithm,
            "kid": self.kid,
            "typ": TOKEN_HEADER_TYPE,
        });
        // RFC 7515 §5.1: the signature is computed over the ENCODED header and payload,
        // joined by '.', so those exact bytes are what the token carries.
        let signing_input = format!(
            "{}.{}",
            crate::jwt::encode_base64url(header.to_string().as_bytes()),
            crate::jwt::encode_base64url(claims.to_string().as_bytes())
        );
        let signature = self.sign(signing_input.as_bytes())?;
        Some(format!(
            "{signing_input}.{}",
            crate::jwt::encode_base64url(&signature)
        ))
    }
}

/// The JSON Web Key Set (RFC 7517 §5) an issuer publishes for `keys`: the PUBLIC half of
/// each, in the order given.
///
/// The one rendering of that document, so every issuer this emulator serves publishes its
/// keys in the shape RFC 7517 fixes and none writes the envelope again.
#[must_use]
pub fn key_set<'k>(keys: impl IntoIterator<Item = &'k SigningKey>) -> ::serde_json::Value {
    ::serde_json::json!({
        "keys": keys.into_iter().map(SigningKey::to_jwk).collect::<Vec<_>>(),
    })
}

/// The public modulus and exponent of the RSA key `private_key_pkcs8` holds, unsigned
/// big-endian as RFC 7518 §6.3.1 publishes them, or `None` when the encoding is not an RSA
/// PKCS#8 key.
///
/// RFC 5958 §2: the `privateKey` octet string of an RSA PKCS#8 key IS the PKCS#1
/// `RSAPrivateKey`, which carries the public modulus and exponent beside the private ones —
/// so the published parameters are read out of the key the issuer holds rather than kept a
/// second time beside it.
fn rsa_public_components(private_key_pkcs8: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let pkcs8 = ::pkcs8::PrivateKeyInfo::try_from(private_key_pkcs8).ok()?;
    let key = <::pkcs1::RsaPrivateKey as ::pkcs1::der::Decode>::from_der(pkcs8.private_key).ok()?;
    Some((
        key.modulus.as_bytes().to_vec(),
        key.public_exponent.as_bytes().to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a key generated here signs a token that VERIFIES against the entry it
    /// publishes — for every algorithm this module can generate — and the entry carries no
    /// private parameter.
    #[test]
    fn a_generated_key_signs_a_token_its_published_entry_verifies() {
        for algorithm in SIGNABLE_ALGORITHMS {
            let key = SigningKey::generate(algorithm)
                .unwrap_or_else(|| panic!("case {algorithm}: no key was generated"));
            assert_eq!(
                key.algorithm(),
                *algorithm,
                "case {algorithm}: expected the key to sign under it, got {:?}",
                key.algorithm()
            );
            let claims = ::serde_json::json!({ "iss": "https://issuer.example", "sub": "probe" });
            let token = key
                .signed_compact_jws(&claims)
                .unwrap_or_else(|| panic!("case {algorithm}: nothing was signed"));
            let set = key_set([&key]);
            let verified = crate::jwt::verify(&token, &set).unwrap_or_else(|defect| {
                panic!("case {algorithm}: the token does not verify: {defect}")
            });
            assert_eq!(
                verified.claim_str("sub"),
                Some("probe"),
                "case {algorithm}: expected the claims as signed, got {:?}",
                verified.claims
            );
            for private in ["d", "p", "q", "dp", "dq", "qi"] {
                assert!(
                    key.to_jwk().get(private).is_none(),
                    "case {algorithm}: a published entry must carry no {private}: {}",
                    key.to_jwk()
                );
            }
        }
    }

    /// RULE: a token is verified against the key its `kid` names, so one issuer's key does
    /// not verify another's token, and an algorithm this module cannot generate yields no
    /// key at all.
    #[test]
    fn a_token_verifies_only_against_the_key_that_signed_it() {
        let signer = SigningKey::generate(RS256).expect("an RSA key");
        let other = SigningKey::generate(RS256).expect("a second RSA key");
        assert_ne!(
            signer.kid(),
            other.kid(),
            "case two generated keys: expected different key ids, got {:?}",
            signer.kid()
        );
        let token = signer
            .signed_compact_jws(&::serde_json::json!({ "sub": "probe" }))
            .expect("a signed token");
        assert!(
            crate::jwt::verify(&token, &key_set([&other])).is_err(),
            "case another issuer's key set: the token must not verify against it"
        );
        for algorithm in ["", "RS512", "none", "rs256"] {
            assert!(
                SigningKey::generate(algorithm).is_none(),
                "case {algorithm:?}: no key may be generated for an algorithm outside the set"
            );
        }
    }
}
