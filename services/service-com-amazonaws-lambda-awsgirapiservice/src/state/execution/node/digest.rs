// SPDX-License-Identifier: CC0-1.0

//! The three COMPUTATIONS the `crypto` core module is served by: a digest, a keyed digest,
//! the key a digest derives and cryptographic random bytes.
//!
//! None of them is written here. A digest is the arithmetic the RustCrypto implementations
//! this crate links already state, the keyed digest is `hmac` over one of them, the derived
//! key is `hkdf` over the same, and the random bytes are the platform's own cryptographic
//! source. They are host functions rather than guest source because that is where those
//! implementations are — not because the guest is given any new reach: an algorithm name and
//! bytes go in, bytes come out, and nothing here opens a file, a socket or an environment.

use ::hmac::Mac;
use ::sha2::Digest;

/// The algorithm names a digest may be asked for, each the name a Node program spells the
/// algorithm with.
///
/// A name that is not one of these is answered with nothing, so the guest's `crypto` reports
/// the algorithm as unsupported rather than being handed the digest of another one.
pub(super) const DIGEST_ALGORITHMS: &[&str] = &["md5", "sha1", "sha256", "sha384", "sha512"];

/// The digest of `data` under `algorithm`, hex in and hex out.
#[must_use]
pub(super) fn digest(algorithm: &str, data: &str) -> Option<String> {
    if !DIGEST_ALGORITHMS.contains(&algorithm) {
        return None;
    }
    let bytes = ::hex::decode(data).ok()?;
    let digested = match algorithm {
        "md5" => ::hex::encode(::md5::Md5::digest(&bytes)),
        "sha1" => ::hex::encode(::sha1::Sha1::digest(&bytes)),
        "sha256" => ::hex::encode(::sha2::Sha256::digest(&bytes)),
        "sha384" => ::hex::encode(::sha2::Sha384::digest(&bytes)),
        "sha512" => ::hex::encode(::sha2::Sha512::digest(&bytes)),
        _ => return None,
    };
    Some(digested)
}

/// The keyed digest of `data` under `algorithm` with `key`, hex in and hex out.
#[must_use]
pub(super) fn hmac(algorithm: &str, key: &str, data: &str) -> Option<String> {
    if !DIGEST_ALGORITHMS.contains(&algorithm) {
        return None;
    }
    let key = ::hex::decode(key).ok()?;
    let bytes = ::hex::decode(data).ok()?;
    // `new_from_slice` accepts a key of any length, which is what HMAC is defined for; one
    // arm per hash, because the keyed construction is instantiated over the hash's type.
    macro_rules! keyed {
        ($hash:ty) => {{
            let mut keyed = ::hmac::Hmac::<$hash>::new_from_slice(&key).ok()?;
            keyed.update(&bytes);
            ::hex::encode(keyed.finalize().into_bytes())
        }};
    }
    let keyed = match algorithm {
        "md5" => keyed!(::md5::Md5),
        "sha1" => keyed!(::sha1::Sha1),
        "sha256" => keyed!(::sha2::Sha256),
        "sha384" => keyed!(::sha2::Sha384),
        "sha512" => keyed!(::sha2::Sha512),
        _ => return None,
    };
    Some(keyed)
}

/// How many bytes one derivation may ask for.
///
/// The construction's OWN ceiling is 255 times the digest's length (RFC 5869 §2.3), which the
/// expansion below refuses past exactly, per digest; this is that ceiling for the longest
/// digest here — the bound the host checks BEFORE a length becomes an allocation, as
/// [`MAX_RANDOM_BYTES`] is for a draw, because the bytes are materialised in host memory
/// outside the engine's own allocation ceiling.
pub(super) const MAX_DERIVED_BYTES: usize = 255 * 64;

/// The `length` bytes HKDF derives from `ikm` under `algorithm`, salted with `salt` and bound
/// to `info` — hex in, hex out.
///
/// The extract-and-expand construction itself is not written here: it is the one its own crate
/// states, over the same digests as the keyed digest above, so a key derived here is the key
/// any other implementation of the construction derives from the same four inputs.
#[must_use]
pub(super) fn hkdf(
    algorithm: &str,
    ikm: &str,
    salt: &str,
    info: &str,
    length: usize,
) -> Option<String> {
    if !DIGEST_ALGORITHMS.contains(&algorithm) || length > MAX_DERIVED_BYTES {
        return None;
    }
    let ikm = ::hex::decode(ikm).ok()?;
    let salt = ::hex::decode(salt).ok()?;
    let info = ::hex::decode(info).ok()?;
    // One arm per hash, because the construction is instantiated over the hash's type.
    macro_rules! derived {
        ($hash:ty) => {{
            let mut bytes = vec![0_u8; length];
            ::hkdf::Hkdf::<$hash>::new(Some(&salt), &ikm)
                .expand(&info, &mut bytes)
                .ok()?;
            ::hex::encode(bytes)
        }};
    }
    let derived = match algorithm {
        "md5" => derived!(::md5::Md5),
        "sha1" => derived!(::sha1::Sha1),
        "sha256" => derived!(::sha2::Sha256),
        "sha384" => derived!(::sha2::Sha384),
        "sha512" => derived!(::sha2::Sha512),
        _ => return None,
    };
    Some(derived)
}

/// The TEXT CODINGS the `buffer` core module converts bytes with, computed by the crates
/// this emulator already links rather than written out in guest source.
///
/// `hex` and `base64` (in both its standard and its URL alphabet) are coding, not language:
/// a maintained implementation already states each of them, and the digests above are
/// handed their input and report their answer in the first of the two, so the one seam
/// serves both.
#[must_use]
pub(super) fn encode(coding: &str, bytes: Vec<u8>) -> Option<String> {
    let encoded = match coding {
        "hex" => ::hex::encode(bytes),
        "base64" => ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, bytes),
        "base64url" => ::base64::Engine::encode(
            &::base64::engine::general_purpose::URL_SAFE_NO_PAD,
            bytes,
        ),
        _ => return None,
    };
    Some(encoded)
}

/// The bytes `text` encodes under `coding`.
///
/// A text that is not that coding decodes to what it holds up to the first character that is
/// not — which is how a Node buffer decodes a partly-valid string — so a decode answers with
/// bytes rather than with a failure.
#[must_use]
pub(super) fn decode(coding: &str, text: &str) -> Option<Vec<u8>> {
    let decoded = match coding {
        "hex" => {
            // Whole bytes only, stopping at the first character that is not a hex digit.
            let digits: String = text
                .chars()
                .take_while(|character| character.is_ascii_hexdigit())
                .collect();
            let whole = digits.len() - (digits.len() % 2);
            ::hex::decode(&digits[..whole]).ok()?
        }
        "base64" | "base64url" => {
            // Both alphabets and an unpadded tail, which is what a Node buffer accepts for
            // either of them.
            let cleaned: String = text
                .chars()
                .filter(|character| character.is_ascii_alphanumeric() || *character == '+' || *character == '/' || *character == '-' || *character == '_')
                .map(|character| match character {
                    '-' => '+',
                    '_' => '/',
                    other => other,
                })
                .collect();
            ::base64::Engine::decode(
                &::base64::engine::general_purpose::STANDARD_NO_PAD,
                cleaned.trim_end_matches('='),
            )
            .ok()?
        }
        _ => return None,
    };
    Some(decoded)
}

/// How many random bytes one call may draw.
///
/// A HOST bound on the work one `randomBytes` call makes this process do. Node's own limit
/// on the size of a single `randomBytes` request is far larger; this is well above what a
/// key, a nonce or an identifier needs.
pub(super) const MAX_RANDOM_BYTES: usize = 64 * 1024;

/// `count` cryptographic random bytes, as hex, or `None` when more than
/// [`MAX_RANDOM_BYTES`] were asked for or the platform's source could not be read.
#[must_use]
pub(super) fn random_hex(count: usize) -> Option<String> {
    if count > MAX_RANDOM_BYTES {
        return None;
    }
    let mut bytes = vec![0_u8; count];
    ::getrandom::fill(&mut bytes).ok()?;
    Some(::hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per algorithm this module computes, each against the digest of the empty
    /// input that algorithm's own specification fixes — the one value every implementation
    /// of it agrees on.
    #[test]
    fn every_named_algorithm_computes_its_own_digest() {
        let cases: &[(&str, &str)] = &[
            ("md5", "d41d8cd98f00b204e9800998ecf8427e"),
            ("sha1", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
            (
                "sha256",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "sha384",
                "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
            ),
            (
                "sha512",
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
            ),
        ];
        for (algorithm, expected) in cases {
            let actual = digest(algorithm, "");
            assert_eq!(
                actual.as_deref(),
                Some(*expected),
                "case {algorithm:?}: expected the digest of the empty input, got {actual:?}"
            );
        }
        for absent in ["sha3-256", "", "SHA256", "crc32"] {
            assert!(
                digest(absent, "").is_none(),
                "case {absent:?}: this runtime computes no such digest, so none must be \
                 answered"
            );
        }
    }

    /// The keyed digest of RFC 4231's first test case, one row per algorithm it fixes, so
    /// the key handling is pinned and not only the hash.
    #[test]
    fn a_keyed_digest_is_the_keyed_digest_of_its_algorithm() {
        // RFC 4231 test case 1: a 20-byte key of 0x0b over the text "Hi There".
        let key = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";
        let data = ::hex::encode(b"Hi There");
        let cases: &[(&str, &str)] = &[
            (
                "sha256",
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                "sha512",
                "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
            ),
        ];
        for (algorithm, expected) in cases {
            let actual = hmac(algorithm, key, &data);
            assert_eq!(
                actual.as_deref(),
                Some(*expected),
                "case {algorithm:?}: expected the published keyed digest, got {actual:?}"
            );
        }
    }

    /// Every coding converts bytes to text and back: one row per coding, each over a byte
    /// string with the padding and the alphabet edge the coding has.
    #[test]
    fn a_coding_round_trips_the_bytes_it_was_given() {
        // One, two and three bytes, so each base64 padding case is covered, plus the two
        // bytes whose base64 digits differ between the standard and the URL alphabet.
        let cases: &[&[u8]] = &[b"", b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0xbf]];
        for coding in ["hex", "base64", "base64url"] {
            for bytes in cases {
                let text = encode(coding, bytes.to_vec())
                    .unwrap_or_else(|| panic!("case {coding}: the coding must be known"));
                let back = decode(coding, &text);
                assert_eq!(
                    back.as_deref(),
                    Some(*bytes),
                    "case {coding} over {bytes:?}: expected the bytes back from {text:?}, got \
                     {back:?}"
                );
            }
        }
        for absent in ["base32", "", "HEX"] {
            assert!(
                encode(absent, Vec::new()).is_none() && decode(absent, "").is_none(),
                "case {absent:?}: this runtime converts no such coding"
            );
        }
    }

    /// RULE: a derived key is the extract-and-expand construction's own answer over the four
    /// inputs, which is what makes a key derived here the key any other implementation
    /// derives.
    ///
    /// The rows are RFC 5869's published test vectors: case 1 (SHA-256, with a salt and an
    /// info), case 3 (SHA-256, with neither) — plus the length the construction generalises
    /// over, asserted by the first vector's own output length.
    #[test]
    fn a_derived_key_is_its_constructions_own() {
        // Each row: the case, the digest, the input key material, the salt, the info, the
        // length, and the key RFC 5869 publishes for it.
        let cases: &[(&str, &str, &str, &str, &str, usize, &str)] = &[
            (
                "RFC 5869 case 1",
                "sha256",
                "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
                "000102030405060708090a0b0c",
                "f0f1f2f3f4f5f6f7f8f9",
                42,
                "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
            ),
            (
                "RFC 5869 case 3, with neither a salt nor an info",
                "sha256",
                "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
                "",
                "",
                42,
                "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8",
            ),
        ];
        for (case, algorithm, ikm, salt, info, length, expected) in cases {
            let derived = hkdf(algorithm, ikm, salt, info, *length);
            assert_eq!(
                derived.as_deref(),
                Some(*expected),
                "case {case:?}: expected the published derived key, got {derived:?}"
            );
        }
        // A digest this module does not compute derives nothing, and neither does a length
        // past the host's ceiling.
        for (case, algorithm, length) in [
            ("a digest this module does not compute", "sha3-256", 32_usize),
            ("a length past the host's ceiling", "sha256", MAX_DERIVED_BYTES + 1),
        ] {
            assert!(
                hkdf(algorithm, "0b", "", "", length).is_none(),
                "case {case:?}: expected no key to be derived"
            );
        }
    }

    /// Random bytes are the count asked for, and two draws of a non-trivial count differ —
    /// the property a source of randomness has that a constant does not.
    #[test]
    fn random_bytes_are_as_many_as_were_asked_for() {
        for count in [0_usize, 1, 16, 1024] {
            let drawn = random_hex(count).expect("the platform's random source");
            assert_eq!(
                drawn.len(),
                count * 2,
                "case {count} bytes: expected {} hex digits, got {}",
                count * 2,
                drawn.len()
            );
        }
        let first = random_hex(32).expect("the platform's random source");
        let second = random_hex(32).expect("the platform's random source");
        assert_ne!(
            first, second,
            "two 32-byte draws of a random source must differ, got {first} twice"
        );
        assert!(
            random_hex(MAX_RANDOM_BYTES + 1).is_none(),
            "a draw past the per-call ceiling is refused, not answered"
        );
    }
}
