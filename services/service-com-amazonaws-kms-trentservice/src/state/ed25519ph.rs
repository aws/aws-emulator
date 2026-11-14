// SPDX-License-Identifier: CC0-1.0

//! The PREHASHED Edwards25519 signature scheme — Ed25519ph, RFC 8032 Section 5.1 with
//! the prehash flag set and an empty context, which is the HashEdDSA of FIPS 186-5
//! Section 7.8 — produced and checked by `ed25519-dalek`, the library in this workspace
//! that implements it.
//!
//! One concern of [`super`]'s store, in its own file: the pure Edwards25519 scheme and
//! every other signature scheme stay with the key material in [`super::signatures`],
//! which is the only caller of what is here.
//!
//! A caller signing under this scheme hands KMS the SHA-512 PREHASH of its message
//! rather than the message, so the scheme has to be driven from a digest that is
//! already computed: the prehash is presented to the library as the message digest
//! ([`Prehash`]) while the scheme's own hashing — the `dom2` prefix, the nonce and the
//! challenge of RFC 8032 — stays SHA-512, exactly as it is when the library hashes the
//! message itself. Nothing cryptographic is composed here.

use ::ed25519_dalek::hazmat::{raw_sign_prehashed, raw_verify_prehashed, ExpandedSecretKey};
use ::ed25519_dalek::pkcs8::DecodePrivateKey;
use ::ed25519_dalek::{Signature, SigningKey};
use ::sha2::digest::consts::U64;
use ::sha2::digest::{FixedOutput, HashMarker, Output, OutputSizeUser, Update};
use ::sha2::Sha512;

/// How many bytes the prehash of this scheme is: SHA-512 produces 64.
pub(super) const PREHASH_LEN: usize = 64;

/// The context string of the scheme KMS implements: Ed25519ph takes an optional
/// context, and the HashEdDSA of FIPS 186-5 Section 7.8 — which is the scheme the KMS
/// prehashed Edwards25519 signing algorithm names — takes none, so every signature here
/// is produced and checked with no context at all.
const CONTEXT: Option<&[u8]> = None;

/// A SHA-512 prehash the CALLER computed, presented as the message digest the signature
/// scheme signs or checks.
///
/// The scheme takes the message digest as a digest still to be finalised, because it
/// normally hashes the message itself; under this scheme KMS is handed the prehash
/// instead ("when the value is DIGEST, KMS skips the hashing step in the signing
/// algorithm"), so finalising this one yields those bytes unchanged. It is used for that
/// one step only — the scheme's own hashing is [`Sha512`] — and so it hashes nothing and
/// absorbs no input.
#[derive(Clone)]
struct Prehash([u8; PREHASH_LEN]);

/// A digest still to be fed anything is 64 zero bytes here, which is what a hash
/// function's initial state amounts to for a value that absorbs no input. Nothing in
/// this module constructs one: every prehash comes from a caller's own bytes
/// ([`Prehash::new`]).
impl Default for Prehash {
    fn default() -> Self {
        Self([0u8; PREHASH_LEN])
    }
}

impl Prehash {
    /// The prehash `digest` carries, or `None` when it is not as many bytes as SHA-512
    /// produces and so is no prehash of this scheme.
    fn new(digest: &[u8]) -> Option<Self> {
        Some(Self(digest.try_into().ok()?))
    }
}

/// Finalising yields 64 bytes, the length of a SHA-512 digest.
impl OutputSizeUser for Prehash {
    type OutputSize = U64;
}

/// Absorbs nothing: the digest is already computed, and the scheme never feeds this one
/// any input.
impl Update for Prehash {
    fn update(&mut self, _data: &[u8]) {}
}

/// Yields the prehash the caller computed.
impl FixedOutput for Prehash {
    fn finalize_into(self, out: &mut Output<Self>) {
        // Byte by byte: the output is as long as the prehash by construction
        // ([`OutputSizeUser`]), and copying this way needs that of neither side.
        for (slot, byte) in out.iter_mut().zip(self.0) {
            *slot = byte;
        }
    }
}

/// Stands in for a hash function, which is what lets the scheme take it as its message
/// digest.
impl HashMarker for Prehash {}

/// The Ed25519ph signature of `prehash` under the key pair whose private half is
/// `private_key_pkcs8` (a DER-encoded PKCS#8 `PrivateKeyInfo`): 64 bytes, the `R` and
/// `S` encoding of RFC 8032 Section 5.1.6.
///
/// `None` means the bytes are not an Edwards25519 private key, `prehash` is not a
/// SHA-512 digest, or the library refused to sign — none of which is anything about the
/// scheme being implemented.
pub(super) fn sign_prehash(private_key_pkcs8: &[u8], prehash: &[u8]) -> Option<Vec<u8>> {
    let signing_key = SigningKey::from_pkcs8_der(private_key_pkcs8).ok()?;
    let expanded = ExpandedSecretKey::from(&signing_key.to_bytes());
    let signature = raw_sign_prehashed::<Sha512, Prehash>(
        &expanded,
        Prehash::new(prehash)?,
        &signing_key.verifying_key(),
        CONTEXT,
    )
    .ok()?;
    Some(signature.to_bytes().to_vec())
}

/// Whether `signature` is the Ed25519ph signature of `prehash` under the PUBLIC half of
/// the key pair whose private half is `private_key_pkcs8` — the half `GetPublicKey`
/// publishes, derived here from the private half by the same library.
///
/// `false` for a signature of another prehash, under another key pair, with another
/// scheme, or that is no 64-byte Edwards25519 signature at all. `None` means the bytes
/// are not an Edwards25519 private key or `prehash` is not a SHA-512 digest.
pub(super) fn prehash_signature_matches(
    private_key_pkcs8: &[u8],
    prehash: &[u8],
    signature: &[u8],
) -> Option<bool> {
    let signing_key = SigningKey::from_pkcs8_der(private_key_pkcs8).ok()?;
    let prehash = Prehash::new(prehash)?;
    let Ok(signature) = Signature::from_slice(signature) else {
        // Bytes that are not an encoded signature of this scheme are not a signature of
        // this message, which is what the caller is told.
        return Some(false);
    };
    Some(
        raw_verify_prehashed::<Sha512, Prehash>(
            &signing_key.verifying_key(),
            prehash,
            CONTEXT,
            &signature,
        )
        .is_ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Ed25519ph test vector of RFC 8032 Section 7.3, the standard's own, as that
    /// section's own hex: the secret key, the message, and the signature the scheme
    /// produces over it. The vector is what pins this module to Ed25519ph rather than to
    /// some other use of an Edwards25519 key pair — a signature over the message itself,
    /// or over the prehash as if it were a message, is a different 64 bytes.
    const RFC_8032_SECRET_KEY: &str =
        "833fe62409237b9d62ec77587520911e9a759cec1d19755b7da901b96dca3d42";
    /// The vector's message: the three bytes "abc".
    const RFC_8032_MESSAGE: &[u8] = b"abc";
    /// The vector's signature over that message.
    const RFC_8032_SIGNATURE: &str = "98a70222f0b8121aa9d30f813d683f809e462b469c7ff87639499\
                                      bb94e6dae4131f85042463c2a355a2003d062adf5aaa10b8c61e6\
                                      36062aaad11c2a26083406";

    /// The bytes one of the vector's hex strings states.
    fn vector_bytes(hex: &str) -> Vec<u8> {
        ::hex::decode(hex).expect("the vector is hex")
    }

    /// The PKCS#8 `PrivateKeyInfo` of the vector's secret key, as the store holds an
    /// Edwards25519 private key: encoded by `aws-lc-rs`, the library that encodes every
    /// private key this service keeps.
    fn vector_private_key_pkcs8() -> Vec<u8> {
        ::aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&vector_bytes(
            RFC_8032_SECRET_KEY,
        ))
        .expect("the vector's secret key is an Edwards25519 seed")
        .to_pkcs8v1()
        .expect("an Edwards25519 private key encodes as PKCS#8")
        .as_ref()
        .to_vec()
    }

    /// The SHA-512 prehash of the vector's message, which is what a caller signing under
    /// this scheme submits.
    fn vector_prehash() -> Vec<u8> {
        ::aws_lc_rs::digest::digest(&::aws_lc_rs::digest::SHA512, RFC_8032_MESSAGE)
            .as_ref()
            .to_vec()
    }

    /// The scheme is Ed25519ph: it reproduces the standard's own signature, byte for
    /// byte, and accepts it.
    #[test]
    fn reproduces_the_rfc_8032_prehashed_vector() {
        let private_key = vector_private_key_pkcs8();
        let prehash = vector_prehash();
        let expected = vector_bytes(RFC_8032_SIGNATURE);
        let signature =
            sign_prehash(&private_key, &prehash).expect("the vector's key pair signs the prehash");
        assert_eq!(
            signature, expected,
            "the Ed25519ph signature of RFC 8032 Section 7.3, got {signature:?}"
        );
        assert_eq!(
            prehash_signature_matches(&private_key, &prehash, &expected),
            Some(true),
            "the standard's own signature verifies"
        );
    }

    /// A signature of anything else does not verify, and neither do bytes that are no
    /// signature.
    #[test]
    fn refuses_a_signature_of_another_prehash() {
        let private_key = vector_private_key_pkcs8();
        let prehash = vector_prehash();
        let signature = vector_bytes(RFC_8032_SIGNATURE);
        let mut tampered = prehash.clone();
        tampered[0] ^= 1;
        assert_eq!(
            prehash_signature_matches(&private_key, &tampered, &signature),
            Some(false),
            "a one-bit-different prehash is another message"
        );
        assert_eq!(
            prehash_signature_matches(&private_key, &prehash, &[0u8; 8]),
            Some(false),
            "eight bytes are no Edwards25519 signature"
        );
        assert_eq!(
            prehash_signature_matches(&private_key, &prehash[..32], &signature),
            None,
            "32 bytes are no SHA-512 prehash"
        );
    }
}
