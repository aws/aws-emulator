// SPDX-License-Identifier: CC0-1.0

//! Digital signatures: what a `SigningAlgorithmSpec` signs, and the signature a key
//! pair's private half produces or its public half verifies.
//!
//! One concern of [`super`]'s store, in its own file: the helpers here are reached as
//! `crate::state::…` exactly as they were before, because the parent re-exports them.

use super::*;

/// The `MessageType` of a message that is signed as it was given, hashed by the
/// signing algorithm itself: "use RAW for unhashed messages"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`), and the
/// value KMS assumes when a request names none.
pub const MESSAGE_TYPE_RAW: &str = "RAW";

/// The `MessageType` of a message that is ALREADY the hash the signing algorithm
/// would have computed: "when the value is DIGEST, KMS skips the hashing step in the
/// signing algorithm" (the same documentation). See [`MESSAGE_TYPE_RAW`].
pub const MESSAGE_TYPE_DIGEST: &str = "DIGEST";

/// The `MessageType` of the "64-byte representative μ used in ML-DSA signing as
/// defined in NIST FIPS 204 Section 6.2" (the same documentation), which is therefore
/// a message type the ML-DSA signing algorithm alone takes. See [`MESSAGE_TYPE_RAW`].
pub const MESSAGE_TYPE_EXTERNAL_MU: &str = "EXTERNAL_MU";

/// The `SigningAlgorithmSpec` of pure Ed25519 (`smithy_query(shape="SigningAlgorithmSpec")`),
/// which hashes the message inside the signature scheme itself (RFC 8032) and so
/// "requires KMS MessageType:RAW"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
pub const SIGNING_ALGORITHM_ED25519_SHA_512: &str = "ED25519_SHA_512";

/// The `SigningAlgorithmSpec` of prehashed Ed25519, which "requires KMS
/// MessageType:DIGEST" (the same documentation). See
/// [`SIGNING_ALGORITHM_ED25519_SHA_512`].
pub const SIGNING_ALGORITHM_ED25519_PH_SHA_512: &str = "ED25519_PH_SHA_512";

/// The `SigningAlgorithmSpec` of ML-DSA, the one algorithm whose message may be an
/// external μ. See [`SIGNING_ALGORITHM_ED25519_SHA_512`].
pub const SIGNING_ALGORITHM_ML_DSA_SHAKE_256: &str = "ML_DSA_SHAKE_256";

/// The hash a `SigningAlgorithmSpec` hashes the message with, which is stated in the
/// algorithm's own name: "signing algorithms that end in SHA_256 use the SHA_256
/// hashing algorithm", and the same for SHA_384 and SHA_512
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`). It is also
/// the length a message of [`MESSAGE_TYPE_DIGEST`] must have, because such a message
/// is that hash already computed.
///
/// `None` for an algorithm the same documentation gives another hash — the SHAKE_256
/// of ML-DSA and the SM3 of SM2DSA.
pub fn signing_hash(signing_algorithm: &str) -> Option<&'static ::aws_lc_rs::digest::Algorithm> {
    use ::aws_lc_rs::digest::{SHA256, SHA384, SHA512};

    [
        ("SHA_256", &SHA256),
        ("SHA_384", &SHA384),
        ("SHA_512", &SHA512),
    ]
    .into_iter()
    .find_map(|(suffix, hash)| signing_algorithm.ends_with(suffix).then_some(hash))
}

/// Whether `signing_algorithm` signs a message of `message_type` at all.
///
/// An external μ is "used in ML-DSA signing", so ML-DSA is the only algorithm that
/// takes one; each Edwards25519 algorithm takes exactly one message type
/// ("ED25519_SHA_512 signing algorithm requires KMS MessageType:RAW",
/// "ED25519_PH_SHA_512 signing algorithm requires KMS MessageType:DIGEST"); and every
/// other algorithm signs either the message or its digest, since it "begins with a
/// hash function" that `DIGEST` skips (all quoted from
/// `smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
pub fn signs_message_type(signing_algorithm: &str, message_type: &str) -> bool {
    if let Some(required) = required_message_type(signing_algorithm) {
        return message_type == required;
    }
    if signing_algorithm == SIGNING_ALGORITHM_ML_DSA_SHAKE_256 {
        return [
            MESSAGE_TYPE_RAW,
            MESSAGE_TYPE_DIGEST,
            MESSAGE_TYPE_EXTERNAL_MU,
        ]
        .contains(&message_type);
    }
    [MESSAGE_TYPE_RAW, MESSAGE_TYPE_DIGEST].contains(&message_type)
}

/// The one `MessageType` a signing algorithm requires, for an algorithm that takes
/// exactly one: "ED25519_SHA_512 signing algorithm requires KMS MessageType:RAW" and
/// "ED25519_PH_SHA_512 signing algorithm requires KMS MessageType:DIGEST"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`). `None` for
/// an algorithm that signs either the message or its digest, which is every other one.
///
/// It is the table [`signs_message_type`] judges a request against, so the predicate and
/// the one message type an algorithm takes cannot disagree.
pub fn required_message_type(signing_algorithm: &str) -> Option<&'static str> {
    match signing_algorithm {
        SIGNING_ALGORITHM_ED25519_SHA_512 => Some(MESSAGE_TYPE_RAW),
        SIGNING_ALGORITHM_ED25519_PH_SHA_512 => Some(MESSAGE_TYPE_DIGEST),
        _ => None,
    }
}

/// The message a request is refused with when the `MessageType` it names is not one
/// `signing_algorithm` takes ([`signs_message_type`] is that judgement): fixed text
/// around the message type and the algorithm, both as the request named them.
///
/// ONE function for every operation that signs or verifies a message, so the refusal of
/// one cannot be worded differently from the refusal of another.
pub fn incompatible_message_type_message(message_type: &str, signing_algorithm: &str) -> String {
    format!("Message type {message_type} is incompatible with algorithm {signing_algorithm}.")
}

/// Why no signature could be produced for a request the key and the algorithm are
/// otherwise right for.
#[derive(Debug)]
pub enum SignatureRefusal {
    /// No library reachable from here signs with this key pair under this algorithm,
    /// so there are no signature bytes to answer with. The KMS key is still real —
    /// the caller is told the operation is not written yet rather than handed bytes
    /// that are not a signature of its message.
    Unsupported,
    /// The library refused to produce a signature it should have produced, which is a
    /// server fault rather than anything about the request.
    Failed,
}

/// The signature of `message` under the PRIVATE half of `key`'s key pair, with
/// `signing_algorithm`.
///
/// `message_type` says what `message` is: [`MESSAGE_TYPE_RAW`] is the message, which
/// is hashed with the algorithm's own hash ([`signing_hash`]) before it is signed, and
/// [`MESSAGE_TYPE_DIGEST`] is that hash already computed, which is signed as it
/// stands — "when the value is DIGEST, KMS skips the hashing step in the signing
/// algorithm" (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
///
/// The bytes are the encoding the algorithm is defined to produce, which is what the
/// `Signature` member carries: "the encoding of this value is defined by PKCS #1 in
/// RFC 8017" for the RSA algorithms and "a DER-encoded object as defined by ANSI
/// X9.62-2005 and RFC 3279 Section 2.2.3" for the ECDSA ones
/// (`smithy_query(query="doc",shape="SignResponse",member="Signature")`). The private
/// key is only read here, never copied out: it is the library that signs, and this
/// function composes nothing cryptographic of its own.
pub fn sign_message(
    key: &Cmk,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Result<Vec<u8>, SignatureRefusal> {
    use ::aws_lc_rs::signature::{EcdsaKeyPair, Ed25519KeyPair, RsaKeyPair};

    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(SignatureRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let encoding =
                rsa_signing_encoding(signing_algorithm).ok_or(SignatureRefusal::Unsupported)?;
            let digest = signed_digest(signing_algorithm, message_type, message)?;
            let pair =
                RsaKeyPair::from_pkcs8(private_key).map_err(|_| SignatureRefusal::Failed)?;
            // A PKCS #1 signature is exactly as long as the key's modulus.
            let mut signature = vec![0u8; pair.public_modulus_len()];
            pair.sign_digest(encoding, &digest, &mut signature)
                .map_err(|_| SignatureRefusal::Failed)?;
            Ok(signature)
        }
        KeyPairKind::Ecdsa(curve) => {
            let digest = signed_digest(signing_algorithm, message_type, message)?;
            let pair = EcdsaKeyPair::from_pkcs8(curve, private_key)
                .map_err(|_| SignatureRefusal::Failed)?;
            Ok(pair
                .sign_digest(&digest)
                .map_err(|_| SignatureRefusal::Failed)?
                .as_ref()
                .to_vec())
        }
        // Pure Ed25519 hashes the message within the signature scheme itself (RFC
        // 8032), so what it signs is the message and not a digest of it.
        KeyPairKind::Ed25519 if signing_algorithm == SIGNING_ALGORITHM_ED25519_SHA_512 => {
            let pair =
                Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| SignatureRefusal::Failed)?;
            Ok(pair.sign(message).as_ref().to_vec())
        }
        // Prehashed Ed25519 signs the SHA-512 digest of the message — the digest the
        // caller computed and submitted — under the prehash flag of RFC 8032 Section
        // 5.1, which is why its signature over a message is not the pure Ed25519
        // signature over that same message.
        KeyPairKind::Ed25519 if signing_algorithm == SIGNING_ALGORITHM_ED25519_PH_SHA_512 => {
            let digest = signed_digest(signing_algorithm, message_type, message)?;
            super::ed25519ph::sign_prehash(private_key, digest.as_ref())
                .ok_or(SignatureRefusal::Failed)
        }
        // The SM2DSA of GM/T 0003-2012 is the one signature scheme no library reachable
        // from here produces a signature with.
        KeyPairKind::Ed25519 | KeyPairKind::Sm2 => Err(SignatureRefusal::Unsupported),
    }
}

/// The digest `signing_algorithm` signs for a message of `message_type`: the hash of
/// the message, or the hash the caller already computed, taken as the digest it says
/// it is.
fn signed_digest(
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Result<::aws_lc_rs::digest::Digest, SignatureRefusal> {
    use ::aws_lc_rs::digest::{digest, Digest};

    let hash = signing_hash(signing_algorithm).ok_or(SignatureRefusal::Unsupported)?;
    if message_type == MESSAGE_TYPE_DIGEST {
        // The caller's bytes ARE the digest, so the only thing to check is that they
        // are as many bytes as that hash produces — which the caller checked before
        // reaching here, and the library checks again.
        return Digest::import_less_safe(message, hash).map_err(|_| SignatureRefusal::Failed);
    }
    Ok(digest(hash, message))
}

/// The RSA signature scheme a `SigningAlgorithmSpec` names: the RSASSA-PKCS1-v1_5 or
/// the RSASSA-PSS scheme of RFC 8017, with the hash the algorithm's name carries.
/// `None` for an algorithm that is not one an RSA key pair signs with
/// ([`RSA_SIGNING_ALGORITHMS`] is that set).
fn rsa_signing_encoding(
    signing_algorithm: &str,
) -> Option<&'static dyn ::aws_lc_rs::signature::RsaEncoding> {
    use ::aws_lc_rs::signature::{
        RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RSA_PSS_SHA256, RSA_PSS_SHA384,
        RSA_PSS_SHA512,
    };

    match signing_algorithm {
        "RSASSA_PKCS1_V1_5_SHA_256" => Some(&RSA_PKCS1_SHA256),
        "RSASSA_PKCS1_V1_5_SHA_384" => Some(&RSA_PKCS1_SHA384),
        "RSASSA_PKCS1_V1_5_SHA_512" => Some(&RSA_PKCS1_SHA512),
        "RSASSA_PSS_SHA_256" => Some(&RSA_PSS_SHA256),
        "RSASSA_PSS_SHA_384" => Some(&RSA_PSS_SHA384),
        "RSASSA_PSS_SHA_512" => Some(&RSA_PSS_SHA512),
        _ => None,
    }
}

/// Whether `signature` is a signature of `message` under `key`'s key pair with
/// `signing_algorithm`.
///
/// "The signature is verified by using the public key in the same asymmetric KMS key"
/// (`smithy_query(query="doc",shape="Verify")`): the check is performed by the library
/// that implements the algorithm, against the public half DERIVED from the private half
/// this store holds ([`public_key_spki`]) — the same bytes `GetPublicKey` publishes — so
/// a signature verifies here exactly when it verifies outside KMS, and a signature of
/// another message, under another key or with another algorithm does not verify.
///
/// `message_type` says what `message` is, as it does for [`sign_message`]: a message of
/// [`MESSAGE_TYPE_RAW`] is hashed by the scheme itself, and one of
/// [`MESSAGE_TYPE_DIGEST`] is the hash already computed, so the signature is checked
/// over that digest with the hashing step skipped — "a message and its hash digest are
/// considered to be the same message"
/// (`smithy_query(query="doc",shape="VerifyRequest",member="Message")`).
///
/// The errors mean what they mean for [`sign_message`]: `Unsupported` when no library
/// reachable from here verifies this key pair under this algorithm, and `Failed` when a
/// library refused a step it should have performed.
pub fn signature_matches(
    key: &Cmk,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
    signature: &[u8],
) -> Result<bool, SignatureRefusal> {
    let Some(private_key) = key.material.private_key_pkcs8() else {
        return Err(SignatureRefusal::Unsupported);
    };

    // Prehashed Ed25519 is checked over the digest the request carries, by the library
    // that implements that scheme — the same one the signature was produced with.
    if signing_algorithm == SIGNING_ALGORITHM_ED25519_PH_SHA_512
        && matches!(key_pair_kind(&key.key_spec), Some(KeyPairKind::Ed25519))
    {
        let digest = signed_digest(signing_algorithm, message_type, message)?;
        return super::ed25519ph::prehash_signature_matches(
            private_key,
            digest.as_ref(),
            signature,
        )
        .ok_or(SignatureRefusal::Failed);
    }

    let Some(algorithm) = verification_algorithm(&key.key_spec, signing_algorithm) else {
        return Err(SignatureRefusal::Unsupported);
    };
    let public_key =
        public_key_spki(&key.key_spec, private_key).ok_or(SignatureRefusal::Failed)?;
    let parsed = ::aws_lc_rs::signature::ParsedPublicKey::new(algorithm, &public_key)
        .map_err(|_| SignatureRefusal::Failed)?;
    if message_type == MESSAGE_TYPE_DIGEST {
        let digest = signed_digest(signing_algorithm, message_type, message)?;
        return Ok(parsed.verify_digest_sig(&digest, signature).is_ok());
    }
    Ok(parsed.verify_sig(message, signature).is_ok())
}

/// How a signature produced with `signing_algorithm` under a key pair of `key_spec` is
/// VERIFIED: the same scheme, hash and curve, named from the verifying side.
///
/// The RSA schemes are verified for any modulus the specs allow, and each curve spec
/// names its own curve, so the parameters a signature is checked with are the key's own
/// rather than the ones a request asked for. `None` for a pairing no library reachable
/// from here verifies THIS way — SM2DSA, which [`sign_message`] likewise produces no
/// signature with; ML-DSA, whose key pair this service does not hold; and the prehashed
/// Ed25519 scheme, which [`signature_matches`] checks through the library that
/// implements it before it asks here.
fn verification_algorithm(
    key_spec: &str,
    signing_algorithm: &str,
) -> Option<&'static dyn ::aws_lc_rs::signature::VerificationAlgorithm> {
    use ::aws_lc_rs::signature::{
        ECDSA_P256K1_SHA256_ASN1, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
        ECDSA_P521_SHA512_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
        RSA_PKCS1_2048_8192_SHA512, RSA_PSS_2048_8192_SHA256, RSA_PSS_2048_8192_SHA384,
        RSA_PSS_2048_8192_SHA512,
    };

    match key_pair_kind(key_spec)? {
        KeyPairKind::Rsa(_) => match signing_algorithm {
            "RSASSA_PKCS1_V1_5_SHA_256" => Some(&RSA_PKCS1_2048_8192_SHA256),
            "RSASSA_PKCS1_V1_5_SHA_384" => Some(&RSA_PKCS1_2048_8192_SHA384),
            "RSASSA_PKCS1_V1_5_SHA_512" => Some(&RSA_PKCS1_2048_8192_SHA512),
            "RSASSA_PSS_SHA_256" => Some(&RSA_PSS_2048_8192_SHA256),
            "RSASSA_PSS_SHA_384" => Some(&RSA_PSS_2048_8192_SHA384),
            "RSASSA_PSS_SHA_512" => Some(&RSA_PSS_2048_8192_SHA512),
            _ => None,
        },
        KeyPairKind::Ecdsa(_) => match (key_spec, signing_algorithm) {
            ("ECC_NIST_P256", "ECDSA_SHA_256") => Some(&ECDSA_P256_SHA256_ASN1),
            ("ECC_NIST_P384", "ECDSA_SHA_384") => Some(&ECDSA_P384_SHA384_ASN1),
            ("ECC_NIST_P521", "ECDSA_SHA_512") => Some(&ECDSA_P521_SHA512_ASN1),
            ("ECC_SECG_P256K1", "ECDSA_SHA_256") => Some(&ECDSA_P256K1_SHA256_ASN1),
            _ => None,
        },
        KeyPairKind::Ed25519 if signing_algorithm == SIGNING_ALGORITHM_ED25519_SHA_512 => {
            Some(&ED25519)
        }
        KeyPairKind::Ed25519 | KeyPairKind::Sm2 => None,
    }
}
