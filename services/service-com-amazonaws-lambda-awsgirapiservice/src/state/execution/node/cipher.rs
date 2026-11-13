// SPDX-License-Identifier: CC0-1.0

//! The SYMMETRIC CIPHERS the `crypto` core module's `createCipheriv` and `createDecipheriv`
//! are served by: the AES transforms a Node program names by their OpenSSL name.
//!
//! None of the arithmetic is written here. Each mode is the transform a maintained
//! RustCrypto implementation already states — `aes-gcm` for the authenticated mode, `cbc`
//! and `ctr` for the two unauthenticated ones over the `aes` block cipher — and this module
//! is only the name table and the hex seam over them, exactly as [`super::digest`] is for the
//! digests. An algorithm name, a key, an IV, the additional data and the bytes go in; the
//! bytes and, for an authenticated mode, the tag come out. Nothing here opens a file, a
//! socket or an environment.
//!
//! A cipher this host does not implement is REFUSED by name ([`Outcome::UNKNOWN`]), so the
//! guest's `crypto` reports it the way Node reports a cipher its own build does not carry,
//! rather than answering with the transform of another algorithm. The same holds for a key
//! or an IV that is not the length the named cipher takes, and for a message whose tag or
//! padding does not verify: a failure to open is reported as one and never as plaintext.

use ::aes::cipher::block_padding::Pkcs7;
use ::aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, StreamCipher};
use ::aes_gcm::aead::{Aead, Payload};
use ::aes_gcm::{AesGcm, KeyInit};

/// The nonce length the AES-GCM ciphers here take, which is the 96-bit nonce the mode's own
/// specification names as the one every implementation of it supports.
const GCM_NONCE_BYTES: usize = 12;

/// The tag length those ciphers produce and verify: AES-GCM's full 128-bit tag.
const GCM_TAG_BYTES: usize = 16;

/// The block length of AES, which is the IV length its block modes take.
const AES_BLOCK_BYTES: usize = 16;

/// AES-GCM over a 192-bit key, the one key size the `aes-gcm` crate names no alias for.
type Aes192Gcm = AesGcm<::aes::Aes192, ::aes_gcm::aead::consts::U12, ::aes_gcm::aead::consts::U16>;

/// What one call answers with, as the first word of the answer.
///
/// The guest turns each of these into the error Node reports for that failure, so a refusal
/// says WHICH of them it is rather than being one opaque failure.
pub(super) struct Outcome;

impl Outcome {
    /// The transform was computed: the bytes follow, and then the tag of an authenticated
    /// mode (empty for the others).
    pub(super) const OK: &'static str = "ok";
    /// This host computes no cipher of that name.
    pub(super) const UNKNOWN: &'static str = "unknown";
    /// The key is not the length that cipher takes.
    pub(super) const KEY_LENGTH: &'static str = "keylen";
    /// The IV is not the length that cipher's mode takes.
    pub(super) const IV_LENGTH: &'static str = "iv";
    /// The message did not open: an authenticated mode's tag did not verify, or a padded
    /// mode's padding was not the padding it writes.
    pub(super) const UNOPENED: &'static str = "unopened";
    /// The message, or its additional data, is longer than one call transforms
    /// ([`MAX_CIPHER_BYTES`]).
    pub(super) const TOO_LONG: &'static str = "toolong";
    /// A part of the call was not the hex this seam carries bytes as, which is a fault of the
    /// seam itself rather than of the cipher the call named.
    pub(super) const MALFORMED: &'static str = "malformed";
}

/// The ciphers this host computes, by the name a Node program names them under.
///
/// One per transform implemented below; a name that is not here is refused rather than
/// answered, which is what makes the list the whole of what `crypto.getCiphers()` reports.
pub(super) const CIPHERS: &[&str] = &[
    "aes-128-gcm",
    "aes-192-gcm",
    "aes-256-gcm",
    "aes-128-cbc",
    "aes-192-cbc",
    "aes-256-cbc",
    "aes-128-ctr",
    "aes-192-ctr",
    "aes-256-ctr",
];

/// The longest input one call may transform.
///
/// A HOST bound on the work one `update`/`final` pair makes this process do: the bytes are
/// held in host memory, outside the engine's own allocation ceiling, so the length is bounded
/// on this side as well. It is far above the payload of a key, a token or an encrypted
/// message.
pub(super) const MAX_CIPHER_BYTES: usize = 8 * 1024 * 1024;

/// The transform `algorithm` names, applied to `data`.
///
/// Every byte string travels as hex, as the digests beside this do. `aad` is the additional
/// authenticated data of an authenticated mode and `tag` the tag a decryption verifies
/// against; both are empty where the mode or the direction has none. The answer's first word
/// is an [`Outcome`]; on [`Outcome::OK`] the bytes follow, and then the tag an encryption
/// produced.
#[must_use]
pub(super) fn cipher(
    algorithm: &str,
    key: &str,
    iv: &str,
    aad: &str,
    data: &str,
    tag: &str,
    decrypting: bool,
) -> Vec<String> {
    let Some((bits, mode)) = named(algorithm) else {
        return refused(Outcome::UNKNOWN);
    };
    let (Ok(key), Ok(iv), Ok(aad), Ok(data), Ok(tag)) = (
        ::hex::decode(key),
        ::hex::decode(iv),
        ::hex::decode(aad),
        ::hex::decode(data),
        ::hex::decode(tag),
    ) else {
        return refused(Outcome::MALFORMED);
    };
    if key.len() * 8 != bits {
        return refused(Outcome::KEY_LENGTH);
    }
    if iv.len() != mode.iv_bytes() {
        return refused(Outcome::IV_LENGTH);
    }
    if data.len() > MAX_CIPHER_BYTES || aad.len() > MAX_CIPHER_BYTES {
        return refused(Outcome::TOO_LONG);
    }
    match mode {
        Mode::Gcm => gcm(bits, &key, &iv, &aad, &data, &tag, decrypting),
        Mode::Cbc => cbc(bits, &key, &iv, &data, decrypting),
        Mode::Ctr => ctr(bits, &key, &iv, &data),
    }
}

/// The answer of a call that computed nothing, carrying why.
fn refused(outcome: &'static str) -> Vec<String> {
    vec![outcome.to_string()]
}

/// The answer of a call that computed `bytes`, with the `tag` an encryption produced.
fn computed(bytes: &[u8], tag: &[u8]) -> Vec<String> {
    vec![
        Outcome::OK.to_string(),
        ::hex::encode(bytes),
        ::hex::encode(tag),
    ]
}

/// The mode of operation a cipher name's last word states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Galois/Counter Mode: authenticated, so it carries additional data and a tag.
    Gcm,
    /// Cipher Block Chaining, with the PKCS#7 padding a block mode's own writer adds.
    Cbc,
    /// Counter mode: the block cipher as a stream cipher.
    Ctr,
}

impl Mode {
    /// The IV length this mode takes.
    fn iv_bytes(self) -> usize {
        match self {
            Self::Gcm => GCM_NONCE_BYTES,
            Self::Cbc | Self::Ctr => AES_BLOCK_BYTES,
        }
    }
}

/// The key length in bits and the mode the cipher `algorithm` names, or `None` for a name
/// this host computes no cipher for.
fn named(algorithm: &str) -> Option<(usize, Mode)> {
    let lowered = algorithm.to_ascii_lowercase();
    if !CIPHERS.contains(&lowered.as_str()) {
        return None;
    }
    let mut words = lowered.split('-');
    let family = words.next()?;
    let bits = words.next()?.parse::<usize>().ok()?;
    let mode = match words.next()? {
        "gcm" => Mode::Gcm,
        "cbc" => Mode::Cbc,
        "ctr" => Mode::Ctr,
        _ => return None,
    };
    (family == "aes" && words.next().is_none()).then_some((bits, mode))
}

/// AES-GCM, over the key size `bits` names.
fn gcm(
    bits: usize,
    key: &[u8],
    nonce: &[u8],
    aad: &[u8],
    data: &[u8],
    tag: &[u8],
    decrypting: bool,
) -> Vec<String> {
    // One arm per key size, because the mode is instantiated over the block cipher's type.
    macro_rules! sealed {
        ($cipher:ty) => {{
            let Ok(held) = <$cipher>::new_from_slice(key) else {
                return refused(Outcome::KEY_LENGTH);
            };
            let nonce = ::aes_gcm::Nonce::from_slice(nonce);
            if decrypting {
                if tag.len() != GCM_TAG_BYTES {
                    return refused(Outcome::UNOPENED);
                }
                // The crate takes the message with its tag appended, which is how the mode
                // itself carries one.
                let mut message = data.to_vec();
                message.extend_from_slice(tag);
                match held.decrypt(nonce, Payload { msg: &message, aad }) {
                    Ok(opened) => computed(&opened, &[]),
                    Err(_) => refused(Outcome::UNOPENED),
                }
            } else {
                match held.encrypt(nonce, Payload { msg: data, aad }) {
                    // The tag is the last bytes of what the crate answers; the message is
                    // what precedes it, which is what Node hands back separately.
                    Ok(sealed) if sealed.len() >= GCM_TAG_BYTES => {
                        let cut = sealed.len() - GCM_TAG_BYTES;
                        computed(&sealed[..cut], &sealed[cut..])
                    }
                    _ => refused(Outcome::UNOPENED),
                }
            }
        }};
    }
    match bits {
        128 => sealed!(::aes_gcm::Aes128Gcm),
        192 => sealed!(Aes192Gcm),
        256 => sealed!(::aes_gcm::Aes256Gcm),
        _ => refused(Outcome::KEY_LENGTH),
    }
}

/// AES-CBC with PKCS#7 padding, over the key size `bits` names.
fn cbc(bits: usize, key: &[u8], iv: &[u8], data: &[u8], decrypting: bool) -> Vec<String> {
    macro_rules! chained {
        ($block:ty) => {{
            if decrypting {
                let Ok(held) = ::cbc::Decryptor::<$block>::new_from_slices(key, iv) else {
                    return refused(Outcome::KEY_LENGTH);
                };
                match held.decrypt_padded_vec_mut::<Pkcs7>(data) {
                    Ok(opened) => computed(&opened, &[]),
                    Err(_) => refused(Outcome::UNOPENED),
                }
            } else {
                let Ok(held) = ::cbc::Encryptor::<$block>::new_from_slices(key, iv) else {
                    return refused(Outcome::KEY_LENGTH);
                };
                computed(&held.encrypt_padded_vec_mut::<Pkcs7>(data), &[])
            }
        }};
    }
    match bits {
        128 => chained!(::aes::Aes128),
        192 => chained!(::aes::Aes192),
        256 => chained!(::aes::Aes256),
        _ => refused(Outcome::KEY_LENGTH),
    }
}

/// AES-CTR, over the key size `bits` names. The transform is its own inverse, so the
/// direction decides nothing.
fn ctr(bits: usize, key: &[u8], iv: &[u8], data: &[u8]) -> Vec<String> {
    macro_rules! counted {
        ($block:ty) => {{
            let Ok(mut held) = ::ctr::Ctr128BE::<$block>::new_from_slices(key, iv) else {
                return refused(Outcome::KEY_LENGTH);
            };
            let mut bytes = data.to_vec();
            held.apply_keystream(&mut bytes);
            computed(&bytes, &[])
        }};
    }
    match bits {
        128 => counted!(::aes::Aes128),
        192 => counted!(::aes::Aes192),
        256 => counted!(::aes::Aes256),
        _ => refused(Outcome::KEY_LENGTH),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of `answer`, for an answer that computed some.
    fn opened(answer: &[String]) -> Option<(String, String)> {
        match answer {
            [outcome, bytes, tag] if outcome == Outcome::OK => {
                Some((bytes.clone(), tag.clone()))
            }
            _ => None,
        }
    }

    /// A key of `bits` bits and an IV of `bytes` bytes, as hex: the material a row below
    /// drives one cipher with.
    fn material(bits: usize, bytes: usize) -> (String, String) {
        (
            ::hex::encode(vec![0x2b_u8; bits / 8]),
            ::hex::encode(vec![0x7e_u8; bytes]),
        )
    }

    /// RULE: every cipher this host names computes its own transform, and what one of them
    /// sealed it opens again — one row per name in the table, so each key size of each mode
    /// is driven, not only the one an issue cited.
    #[test]
    fn every_named_cipher_round_trips_its_own_bytes() {
        let plaintext = ::hex::encode(b"the code a custom sender delivers");
        let aad = ::hex::encode(b"the context the message is bound to");
        for name in CIPHERS {
            let (bits, mode) = named(name).expect("the table's own name is a cipher");
            let (key, iv) = material(bits, mode.iv_bytes());
            let sealed = cipher(name, &key, &iv, &aad, &plaintext, "", false);
            let (ciphertext, tag) = opened(&sealed)
                .unwrap_or_else(|| panic!("case {name:?}: expected bytes, got {sealed:?}"));
            assert_ne!(
                ciphertext, plaintext,
                "case {name:?}: expected the bytes to be enciphered, got the plaintext back"
            );
            assert_eq!(
                tag.is_empty(),
                mode != Mode::Gcm,
                "case {name:?}: expected a tag from an authenticated mode and none from any \
                 other, got {tag:?}"
            );
            let answer = cipher(name, &key, &iv, &aad, &ciphertext, &tag, true);
            let (back, _) = opened(&answer)
                .unwrap_or_else(|| panic!("case {name:?}: expected plaintext, got {answer:?}"));
            assert_eq!(
                back, plaintext,
                "case {name:?}: expected the bytes back, got {back:?}"
            );
        }
    }

    /// RULE: a message that does not open is reported as not having opened, never as
    /// plaintext — over a TABLE of the ways one can fail to open.
    #[test]
    fn a_message_that_does_not_open_is_refused() {
        let plaintext = ::hex::encode(b"a six digit code");
        let (key, iv) = material(256, GCM_NONCE_BYTES);
        let sealed = cipher("aes-256-gcm", &key, &iv, "", &plaintext, "", false);
        let (ciphertext, tag) = opened(&sealed).expect("the bytes were sealed");
        // Each row: the case, and the call that must not answer with plaintext.
        let rows: Vec<(&str, Vec<String>)> = vec![
            (
                "a tag of another message",
                cipher(
                    "aes-256-gcm",
                    &key,
                    &iv,
                    "",
                    &ciphertext,
                    &::hex::encode(vec![0_u8; GCM_TAG_BYTES]),
                    true,
                ),
            ),
            (
                "additional data the message was not bound to",
                cipher(
                    "aes-256-gcm",
                    &key,
                    &iv,
                    &::hex::encode(b"another context"),
                    &ciphertext,
                    &tag,
                    true,
                ),
            ),
            (
                "another key",
                cipher(
                    "aes-256-gcm",
                    &::hex::encode(vec![0x11_u8; 32]),
                    &iv,
                    "",
                    &ciphertext,
                    &tag,
                    true,
                ),
            ),
            (
                "another nonce",
                cipher(
                    "aes-256-gcm",
                    &key,
                    &::hex::encode(vec![0x11_u8; GCM_NONCE_BYTES]),
                    "",
                    &ciphertext,
                    &tag,
                    true,
                ),
            ),
            (
                "no tag at all",
                cipher("aes-256-gcm", &key, &iv, "", &ciphertext, "", true),
            ),
            (
                "padding no writer of it wrote",
                cipher(
                    "aes-256-cbc",
                    &key,
                    &::hex::encode(vec![0x7e_u8; AES_BLOCK_BYTES]),
                    "",
                    &::hex::encode(vec![0x33_u8; AES_BLOCK_BYTES]),
                    "",
                    true,
                ),
            ),
        ];
        for (case, answer) in rows {
            assert_eq!(
                answer,
                vec![Outcome::UNOPENED.to_string()],
                "case {case:?}: expected the message to be refused, got {answer:?}"
            );
        }
    }

    /// RULE: a cipher this host does not compute, a key that is not its length and an IV
    /// that is not its mode's length are each refused by what they are — nothing is answered
    /// with another algorithm's transform.
    #[test]
    fn a_cipher_a_name_does_not_state_is_refused_by_name() {
        let plaintext = ::hex::encode(b"a six digit code");
        // Each row: the case, the call, and the outcome it must report.
        let rows: Vec<(&str, Vec<String>, &str)> = vec![
            (
                "a cipher no name here states",
                cipher("aes-256-ocb", &::hex::encode(vec![0_u8; 32]), &::hex::encode(vec![0_u8; 12]), "", &plaintext, "", false),
                Outcome::UNKNOWN,
            ),
            (
                "a block cipher this host does not carry",
                cipher("des-ede3-cbc", &::hex::encode(vec![0_u8; 24]), &::hex::encode(vec![0_u8; 8]), "", &plaintext, "", false),
                Outcome::UNKNOWN,
            ),
            (
                "the empty name",
                cipher("", "", "", "", &plaintext, "", false),
                Outcome::UNKNOWN,
            ),
            (
                "a key one byte short of the named size",
                cipher("aes-256-gcm", &::hex::encode(vec![0_u8; 31]), &::hex::encode(vec![0_u8; 12]), "", &plaintext, "", false),
                Outcome::KEY_LENGTH,
            ),
            (
                "a key of another named size",
                cipher("aes-128-gcm", &::hex::encode(vec![0_u8; 32]), &::hex::encode(vec![0_u8; 12]), "", &plaintext, "", false),
                Outcome::KEY_LENGTH,
            ),
            (
                "an IV that is not the mode's nonce",
                cipher("aes-256-gcm", &::hex::encode(vec![0_u8; 32]), &::hex::encode(vec![0_u8; 16]), "", &plaintext, "", false),
                Outcome::IV_LENGTH,
            ),
            (
                "an IV that is not a block",
                cipher("aes-256-cbc", &::hex::encode(vec![0_u8; 32]), &::hex::encode(vec![0_u8; 12]), "", &plaintext, "", false),
                Outcome::IV_LENGTH,
            ),
        ];
        for (case, answer, expected) in rows {
            assert_eq!(
                answer,
                vec![expected.to_string()],
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }
    }

    /// A message past what one call transforms is refused as the length it is, and a part
    /// that is not the hex this seam carries bytes as is refused as the seam's own fault —
    /// neither is reported as the cipher being unknown.
    #[test]
    fn a_message_past_the_bound_and_a_malformed_part_are_refused_as_what_they_are() {
        let (key, iv) = material(256, GCM_NONCE_BYTES);
        let too_long = ::hex::encode(vec![0x41_u8; MAX_CIPHER_BYTES + 1]);
        // Each row: the case, the call, and the outcome it must report.
        let rows: Vec<(&str, Vec<String>, &str)> = vec![
            (
                "a message longer than one call transforms",
                cipher("aes-256-gcm", &key, &iv, "", &too_long, "", false),
                Outcome::TOO_LONG,
            ),
            (
                "additional data longer than one call carries",
                cipher("aes-256-gcm", &key, &iv, &too_long, "00", "", false),
                Outcome::TOO_LONG,
            ),
            (
                "a part that is not hex",
                cipher("aes-256-gcm", &key, &iv, "", "not hex at all", "", false),
                Outcome::MALFORMED,
            ),
        ];
        for (case, answer, expected) in rows {
            assert_eq!(
                answer,
                vec![expected.to_string()],
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }
    }

/// The published test vector of the mode this trigger's message is sealed with, so the
    /// transform is pinned to AES-GCM's own arithmetic and not only to its own inverse.
    ///
    /// NIST's GCM validation set, test case 16 of the AES-256 cases (McGrew & Viega, "The
    /// Galois/Counter Mode of Operation"): a 256-bit key, a 96-bit IV, 60 bytes of plaintext
    /// and 20 bytes of additional data.
    #[test]
    fn the_authenticated_mode_computes_its_published_vector() {
        let key = "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308";
        let iv = "cafebabefacedbaddecaf888";
        let plaintext = "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
                         1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39";
        let aad = "feedfacedeadbeeffeedfacedeadbeefabaddad2";
        let expected = "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa\
                        8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662";
        let expected_tag = "76fc6ece0f4e1768cddf8853bb2d551b";
        let answer = cipher("aes-256-gcm", key, iv, aad, plaintext, "", false);
        let (sealed, tag) = opened(&answer).expect("the vector seals");
        assert_eq!(
            (sealed.as_str(), tag.as_str()),
            (expected, expected_tag),
            "expected the published ciphertext and tag, got {sealed:?} {tag:?}"
        );
    }
}
