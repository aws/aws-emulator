// SPDX-License-Identifier: CC0-1.0

//! The cryptography behind this service's data-protection operations.
//!
//! Every cryptographic PRIMITIVE here comes from a maintained crate: `aes` and `des`
//! for the block ciphers, `cmac` for CMAC, `hmac` with `sha2` for the HMAC family,
//! `hex` for the coding. What is written out is the handful of MODES and MAC chains
//! those primitives are run in, each the published definition:
//!
//! * the block cipher modes of NIST SP 800-38A — ECB, CBC, OFB, and CFB with a
//!   SELECTABLE segment size, because the modelled `EncryptionMode` catalog names CFB
//!   at four of them (1, 8, 64 and 128 bits) and no maintained crate parameterises
//!   CFB that way. The single-segment mode crates would cover four of this catalog's
//!   eight values and still leave CFB1 and CFB64 to be written out, at the cost of a
//!   per-mode dispatch over each of this service's five block cipher key algorithms;
//!   one definition of each mode over the one keyed-cipher wrapper covers the whole
//!   catalog instead.
//! * the CBC-MAC chain of ISO 9797-1 (algorithm 1, and the retail MAC of algorithm
//!   3), which is the MAC the modelled `MacAlgorithm` names and is not what `cmac`
//!   computes.
//! * the CVV/CVC computation and its decimalisation, which is a payment-industry
//!   algorithm no general crate implements.
//!
//! None of these is a stand-in: each mode is pinned against the worked examples NIST
//! publishes for it (see the tests at the foot of this file), a ciphertext is the
//! cipher's output and nothing else — so a decrypt under the same key, mode and IV
//! returns the plaintext bit for bit — and a MAC or a card verification value
//! recomputed from the same inputs is the same value, which is exactly what the
//! `Verify*` operations compare against.

use std::fmt;

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};
use cmac::{Cmac, Mac};
use des::{Des, TdesEde2, TdesEde3};
use hmac::Hmac;
use sha2::{Sha224, Sha256, Sha384, Sha512};

/// The modelled `KeyAlgorithm` values this service can run a symmetric computation
/// under, each with the number of key bytes it takes.
///
/// The TDES and AES entries are the algorithm's own key length; the HMAC entries are
/// the block length of the hash the algorithm selects, the length the control plane
/// gives an HMAC key's material. The names are matched exactly
/// (`smithy_query(query="shape",shape="SymmetricKeyAlgorithm")` on the control
/// plane's model), so an algorithm the model gains later has no working material and
/// the request is refused rather than run under a plausible-looking length.
fn material_len(key_algorithm: &str) -> Option<usize> {
    let len = match key_algorithm {
        "TDES_2KEY" => 16,
        "TDES_3KEY" => 24,
        "AES_128" => 16,
        "AES_192" => 24,
        "AES_256" => 32,
        "HMAC_SHA224" | "HMAC_SHA256" => 64,
        "HMAC_SHA384" | "HMAC_SHA512" => 128,
        _ => return None,
    };
    Some(len)
}

/// Separates the facts [`working_material`] derives from, so two different
/// concatenations of them cannot derive the same bytes.
const FIELD_SEPARATOR: &[u8] = b"\0";

/// Domain separation for [`working_material`], so the derivation cannot collide with
/// any other use of the same primitive.
const WORKING_MATERIAL_LABEL: &[u8] = b"payment-cryptography data plane working key";

/// The working material of the key `key_arn` names, or `None` when its algorithm is
/// not one a symmetric computation runs under.
///
/// This material protects nothing — it is emulator fixture material, derived rather
/// than drawn, so that what this service answers is computed and reproducible — but it
/// is a key's own and no other's, and it never leaves this process.
///
/// A key is minted and kept by the CONTROL plane, which keeps its secret private —
/// only the key's check value, itself computed from that secret, leaves that record.
/// So the bytes this service computes under are derived, once per key, from the facts
/// that key publishes: its check value, its ARN and its algorithm, expanded to the
/// algorithm's key length with HMAC-SHA-256. Two keys therefore never share working
/// material, and the same key always yields the same bytes — which is what makes a
/// ciphertext invertible and a MAC reproducible across calls. The caller stores the
/// result (see [`super::ComAmazonawsPaymentcryptographydataPaymentcryptographydataplaneState`]),
/// so a key's bytes are derived once and read from the store on every later call.
pub fn working_material(
    key_algorithm: &str,
    key_arn: &str,
    key_check_value: &str,
) -> Option<Vec<u8>> {
    let len = material_len(key_algorithm)?;
    let mut material = Vec::with_capacity(len + 32);
    let mut counter: u8 = 1;
    while material.len() < len {
        let mut expand = <Hmac<Sha256> as Mac>::new_from_slice(key_check_value.as_bytes()).ok()?;
        expand.update(WORKING_MATERIAL_LABEL);
        expand.update(FIELD_SEPARATOR);
        expand.update(key_arn.as_bytes());
        expand.update(FIELD_SEPARATOR);
        expand.update(key_algorithm.as_bytes());
        expand.update(&[counter]);
        material.extend_from_slice(&expand.finalize().into_bytes());
        counter = counter.checked_add(1)?;
    }
    material.truncate(len);
    Some(material)
}

/// One keyed symmetric block cipher, as the key's algorithm names it.
///
/// `Des` is not a `KeyAlgorithm` of its own: it is the single-DES cipher the retail
/// MAC and the card verification value run each half of a double-length Triple DES
/// key under, which is why it is keyed from eight bytes rather than from a key's
/// whole material.
enum BlockCipher {
    Des(Des),
    Tdes2(TdesEde2),
    Tdes3(TdesEde3),
    Aes128(Aes128),
    Aes192(Aes192),
    Aes256(Aes256),
}

impl fmt::Debug for BlockCipher {
    /// Names the cipher and never its key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Des(_) => "Des",
            Self::Tdes2(_) => "TdesEde2",
            Self::Tdes3(_) => "TdesEde3",
            Self::Aes128(_) => "Aes128",
            Self::Aes192(_) => "Aes192",
            Self::Aes256(_) => "Aes256",
        };
        write!(f, "BlockCipher({name})")
    }
}

impl BlockCipher {
    /// The cipher `key_algorithm` names, keyed by `material`, or `None` when that
    /// algorithm names no block cipher (the HMAC algorithms name a hash instead).
    fn new(key_algorithm: &str, material: &[u8]) -> Option<Self> {
        let cipher = match key_algorithm {
            "TDES_2KEY" => Self::Tdes2(<TdesEde2 as KeyInit>::new_from_slice(material).ok()?),
            "TDES_3KEY" => Self::Tdes3(<TdesEde3 as KeyInit>::new_from_slice(material).ok()?),
            "AES_128" => Self::Aes128(<Aes128 as KeyInit>::new_from_slice(material).ok()?),
            "AES_192" => Self::Aes192(<Aes192 as KeyInit>::new_from_slice(material).ok()?),
            "AES_256" => Self::Aes256(<Aes256 as KeyInit>::new_from_slice(material).ok()?),
            _ => return None,
        };
        Some(cipher)
    }

    /// The cipher's block length in bytes: eight for Triple DES, sixteen for AES.
    fn block_len(&self) -> usize {
        match self {
            Self::Des(_) | Self::Tdes2(_) | Self::Tdes3(_) => 8,
            Self::Aes128(_) | Self::Aes192(_) | Self::Aes256(_) => 16,
        }
    }

    /// Encrypt one block in place. `block` must be [`Self::block_len`] bytes long.
    fn encrypt_block(&self, block: &mut [u8]) {
        match self {
            Self::Des(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
            Self::Tdes2(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
            Self::Tdes3(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes128(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes192(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes256(c) => c.encrypt_block(GenericArray::from_mut_slice(block)),
        }
    }

    /// Decrypt one block in place. `block` must be [`Self::block_len`] bytes long.
    fn decrypt_block(&self, block: &mut [u8]) {
        match self {
            Self::Des(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
            Self::Tdes2(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
            Self::Tdes3(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes128(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes192(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
            Self::Aes256(c) => c.decrypt_block(GenericArray::from_mut_slice(block)),
        }
    }
}

/// A block cipher mode of operation, as the modelled `EncryptionMode` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Electronic codebook.
    Ecb,
    /// Cipher block chaining.
    Cbc,
    /// Cipher feedback over segments of this many BITS.
    Cfb(usize),
    /// Output feedback.
    Ofb,
}

/// The mode `name` names for a cipher of `block_len` bytes, or the reason it names
/// none for that cipher.
///
/// The names are the modelled `EncryptionMode` values
/// (`smithy_query(query="shape",shape="EncryptionMode")`: CBC, CFB, CFB1, CFB8,
/// CFB64, CFB128, ECB, OFB). A CFB segment is a number of bits of the cipher's
/// output, so a segment wider than the cipher's own block names no mode at all —
/// `CFB128` with a 64-bit Triple DES block, say.
fn mode_for(name: &str, block_len: usize) -> Result<Mode, String> {
    let block_bits = block_len * 8;
    let mode = match name {
        "ECB" => Mode::Ecb,
        "CBC" => Mode::Cbc,
        "OFB" => Mode::Ofb,
        "CFB" => Mode::Cfb(block_bits),
        "CFB1" => Mode::Cfb(1),
        "CFB8" => Mode::Cfb(8),
        "CFB64" => Mode::Cfb(64),
        "CFB128" => Mode::Cfb(128),
        other => return Err(format!("Mode {other} is not a valid block cipher mode.")),
    };
    if let Mode::Cfb(segment_bits) = mode {
        if segment_bits > block_bits {
            return Err(format!(
                "Mode {name} is not valid for a cipher with a {block_bits}-bit block."
            ));
        }
    }
    Ok(mode)
}

/// What a symmetric encrypt or decrypt was asked to do, with everything it is
/// computed from.
pub struct SymmetricRequest<'a> {
    /// The key's modelled `KeyAlgorithm`.
    pub key_algorithm: &'a str,
    /// The key's working material.
    pub material: &'a [u8],
    /// The modelled `EncryptionMode` the request names.
    pub mode: &'a str,
    /// The initialization vector, already decoded; `None` defaults it to zero, as
    /// `SymmetricEncryptionAttributes$InitializationVector` says the service does.
    pub initialization_vector: Option<&'a [u8]>,
    /// The data to transform.
    pub data: &'a [u8],
}

impl fmt::Debug for SymmetricRequest<'_> {
    /// Names the key algorithm and the mode, and never the key material or the data
    /// (both of which the model marks sensitive).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SymmetricRequest")
            .field("key_algorithm", &self.key_algorithm)
            .field("mode", &self.mode)
            .field("material", &format_args!("<redacted>"))
            .field("data", &format_args!("<{} bytes>", self.data.len()))
            .finish()
    }
}

impl SymmetricRequest<'_> {
    /// The cipher, mode and IV this request runs under, or the reason it runs under
    /// none.
    fn resolve(&self) -> Result<(BlockCipher, Mode, Vec<u8>), String> {
        let Some(cipher) = BlockCipher::new(self.key_algorithm, self.material) else {
            return Err(format!(
                "KeyAlgorithm {} is not valid for symmetric encryption.",
                self.key_algorithm
            ));
        };
        let mode = mode_for(self.mode, cipher.block_len())?;
        let block_len = cipher.block_len();
        // "When you use TDES or TDES DUKPT, the plaintext data length must be a
        // multiple of 8 bytes. For AES or AES DUKPT, the plaintext data length must
        // be a multiple of 16 bytes" (`EncryptData`'s own documentation), which is
        // the cipher's block length either way.
        if self.data.is_empty() || !self.data.len().is_multiple_of(block_len) {
            return Err(format!(
                "The data length must be a multiple of {block_len} bytes for KeyAlgorithm {}.",
                self.key_algorithm
            ));
        }
        let iv = match self.initialization_vector {
            None => vec![0u8; block_len],
            Some(iv) if iv.len() == block_len => iv.to_vec(),
            Some(iv) => {
                return Err(format!(
                    "InitializationVector must be {block_len} bytes for KeyAlgorithm {}, not {}.",
                    self.key_algorithm,
                    iv.len()
                ));
            }
        };
        Ok((cipher, mode, iv))
    }
}

/// Encrypt `request`'s data, or the reason it cannot be encrypted as asked.
pub fn symmetric_encrypt(request: &SymmetricRequest<'_>) -> Result<Vec<u8>, String> {
    let (cipher, mode, iv) = request.resolve()?;
    let data = request.data;
    Ok(match mode {
        Mode::Ecb => each_block(&cipher, data, |cipher, block| cipher.encrypt_block(block)),
        Mode::Cbc => cbc_encrypt(&cipher, &iv, data),
        Mode::Cfb(segment_bits) => cfb(&cipher, &iv, data, segment_bits, Direction::Encrypt),
        Mode::Ofb => ofb(&cipher, &iv, data),
    })
}

/// Decrypt `request`'s data, or the reason it cannot be decrypted as asked.
///
/// A block cipher in these modes carries no integrity check, so this answers the
/// plaintext the named key, mode and IV produce and nothing else: a request naming a
/// different key or a different mode from the one the data was encrypted under gets
/// that combination's own plaintext, never the one the original caller encrypted.
pub fn symmetric_decrypt(request: &SymmetricRequest<'_>) -> Result<Vec<u8>, String> {
    let (cipher, mode, iv) = request.resolve()?;
    let data = request.data;
    Ok(match mode {
        Mode::Ecb => each_block(&cipher, data, |cipher, block| cipher.decrypt_block(block)),
        Mode::Cbc => cbc_decrypt(&cipher, &iv, data),
        Mode::Cfb(segment_bits) => cfb(&cipher, &iv, data, segment_bits, Direction::Decrypt),
        Mode::Ofb => ofb(&cipher, &iv, data),
    })
}

/// Which way a CFB segment feeds back: the ciphertext is the feedback either way, so
/// encryption feeds back what it produced and decryption feeds back what it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Encrypt,
    Decrypt,
}

/// ECB: every block transformed on its own.
fn each_block(
    cipher: &BlockCipher,
    data: &[u8],
    transform: impl Fn(&BlockCipher, &mut [u8]),
) -> Vec<u8> {
    let mut out = data.to_vec();
    for block in out.chunks_mut(cipher.block_len()) {
        transform(cipher, block);
    }
    out
}

/// CBC encryption: each plaintext block is XORed with the previous ciphertext block
/// (the IV for the first) before the cipher runs.
fn cbc_encrypt(cipher: &BlockCipher, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut chain = iv.to_vec();
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(cipher.block_len()) {
        let mut buffer: Vec<u8> = block
            .iter()
            .zip(chain.iter())
            .map(|(byte, previous)| byte ^ previous)
            .collect();
        cipher.encrypt_block(&mut buffer);
        chain.copy_from_slice(&buffer);
        out.extend_from_slice(&buffer);
    }
    out
}

/// CBC decryption: the inverse of [`cbc_encrypt`].
fn cbc_decrypt(cipher: &BlockCipher, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut chain = iv.to_vec();
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(cipher.block_len()) {
        let mut buffer = block.to_vec();
        cipher.decrypt_block(&mut buffer);
        for (byte, previous) in buffer.iter_mut().zip(chain.iter()) {
            *byte ^= previous;
        }
        chain.copy_from_slice(block);
        out.extend_from_slice(&buffer);
    }
    out
}

/// OFB: the cipher is run on its own output to make a keystream, which the data is
/// XORed with. The same computation encrypts and decrypts.
fn ofb(cipher: &BlockCipher, iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut register = iv.to_vec();
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(cipher.block_len()) {
        cipher.encrypt_block(&mut register);
        out.extend(
            block
                .iter()
                .zip(register.iter())
                .map(|(byte, key)| byte ^ key),
        );
    }
    out
}

/// CFB over segments of `segment_bits` bits: the cipher is run on a feedback
/// register, the leading `segment_bits` bits of its output are XORed with the next
/// segment of data, and the CIPHERTEXT segment is shifted into the register.
fn cfb(
    cipher: &BlockCipher,
    iv: &[u8],
    data: &[u8],
    segment_bits: usize,
    direction: Direction,
) -> Vec<u8> {
    let mut register = to_bits(iv);
    let input = to_bits(data);
    let mut out = Vec::with_capacity(input.len());
    for segment in input.chunks(segment_bits) {
        let mut block = from_bits(&register);
        cipher.encrypt_block(&mut block);
        let keystream = to_bits(&block);
        let transformed: Vec<bool> = segment
            .iter()
            .zip(keystream.iter())
            .map(|(bit, key)| bit ^ key)
            .collect();
        let feedback = match direction {
            Direction::Encrypt => transformed.clone(),
            Direction::Decrypt => segment.to_vec(),
        };
        register.drain(..feedback.len());
        register.extend(feedback);
        out.extend(transformed);
    }
    from_bits(&out)
}

/// `bytes` as bits, most significant bit of each byte first.
fn to_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1 == 1);
        }
    }
    bits
}

/// The inverse of [`to_bits`]; a trailing partial byte is padded with zero bits,
/// which the callers here never produce because every length is a whole number of
/// bytes.
fn from_bits(bits: &[bool]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(bits.len().div_ceil(8));
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (index, bit) in chunk.iter().enumerate() {
            if *bit {
                byte |= 1 << (7 - index);
            }
        }
        bytes.push(byte);
    }
    bytes
}

/// `mac`, the full MAC as hex, cut to the length the request asked for.
///
/// `MacLength` is a number of HEX CHARACTERS of the MAC: the modelled `MacOutputType`
/// admits an ODD number of them (`length 4..128, pattern ^[0-9a-fA-F]+$`), which a MAC
/// cut at a byte boundary can never be, and `MacLength`'s own 4..32 range sits inside
/// that length bound. A request naming no `MacLength` gets the algorithm's whole MAC.
/// A length longer than the MAC the algorithm computes names no MAC at all, so it is
/// refused with the reason.
pub fn truncated_mac(mac: &str, mac_length: Option<i32>) -> Result<String, String> {
    let Some(mac_length) = mac_length else {
        return Ok(mac.to_string());
    };
    let length = usize::try_from(mac_length).unwrap_or_default();
    if length == 0 || length > mac.len() {
        return Err(format!(
            "MacLength {mac_length} is longer than the {} hexadecimal characters this MAC carries.",
            mac.len()
        ));
    }
    Ok(mac[..length].to_string())
}

/// Why a MAC could not be computed.
#[derive(Debug, Clone)]
pub enum MacRefusal {
    /// The request is inadmissible; the message says why.
    Invalid(String),
    /// The algorithm is one this emulator does not compute yet; the value names it.
    NotImplemented(String),
    /// The key's working material cannot key a computation its algorithm admits — an
    /// invariant of this emulator's own derivation rather than anything the caller
    /// asked for, so it is a server fault and not a refusal of the request.
    Unusable(String),
}

/// How many bytes of key material a computation that uses two single-DES halves
/// takes: the retail MAC and the card verification value each need a double-length
/// Triple DES key.
const RETAIL_MAC_KEY_BYTES: usize = 16;

/// The MAC of `message` under `material`, by the modelled `MacAlgorithm` `algorithm`.
///
/// Each branch is the published definition of that algorithm, run with real
/// primitives over the key's own working material:
///
/// * `ISO9797_ALGORITHM1` — the CBC-MAC of the Triple DES key over the message padded
///   by ISO 9797-1 padding method 1 (zero bytes to a whole block), with a zero IV.
/// * `ISO9797_ALGORITHM3` — the retail MAC: the same CBC-MAC under the key's first
///   single-DES half, then that result decrypted under the second half and encrypted
///   under the first again.
/// * `CMAC` — the CMAC of the key's own block cipher (AES or Triple DES).
/// * `HMAC` — the HMAC of the hash the key's algorithm selects.
/// * `HMAC_SHA224` / `HMAC_SHA256` / `HMAC_SHA384` / `HMAC_SHA512` — the HMAC of the
///   named hash, which the key's algorithm has to be the key for.
pub fn generate_mac(
    algorithm: &str,
    key_algorithm: &str,
    material: &[u8],
    message: &[u8],
) -> Result<Vec<u8>, MacRefusal> {
    match algorithm {
        "ISO9797_ALGORITHM1" => iso9797_algorithm1(key_algorithm, material, message),
        "ISO9797_ALGORITHM3" => iso9797_algorithm3(key_algorithm, material, message),
        "CMAC" => cmac(key_algorithm, material, message),
        "HMAC" => hmac("HMAC", key_algorithm, material, message),
        "HMAC_SHA224" | "HMAC_SHA256" | "HMAC_SHA384" | "HMAC_SHA512" => {
            hmac(algorithm, key_algorithm, material, message)
        }
        // The one modelled algorithm with no computation here yet: its definition is
        // not one this emulator implements, so it says so rather than answering a MAC
        // nothing computed.
        other => Err(MacRefusal::NotImplemented(other.to_string())),
    }
}

/// Refuse a key whose own algorithm cannot compute the requested MAC algorithm, as AWS
/// words that refusal: the requested MAC algorithm, the key's own `KeyAlgorithm`, and the
/// kind of key the caller has to use instead.
///
/// The fixed text is `Invalid key with KeyAlgorithm <requested> used with MacAlgorithm
/// <key's KeyAlgorithm>. Please use valid <requested> Key.` — the two variable parts are
/// the `MacAlgorithm` the REQUEST named and the `KeyAlgorithm` the resolved key reports,
/// and nothing of the key's identity (its ARN is a per-run value AWS's sentence does not
/// contain) appears in it.
fn wrong_key(algorithm: &str, key_algorithm: &str) -> MacRefusal {
    MacRefusal::Invalid(format!(
        "Invalid key with KeyAlgorithm {algorithm} used with MacAlgorithm {key_algorithm}. \
         Please use valid {algorithm} Key."
    ))
}

/// The `KeyAlgorithm` values the modelled `MacAlgorithm` `algorithm` is computed under,
/// or `None` when the algorithm names no computation here.
///
/// One table for every MAC algorithm, so the compatibility rule is stated once: the
/// handler consults it BEFORE the key's modes of use are judged ([`mac_key_check`]), and
/// each computation below consults it again rather than restating its own key set.
fn mac_key_algorithms(algorithm: &str) -> Option<&'static [&'static str]> {
    Some(match algorithm {
        // The ISO 9797 CBC-MACs are Triple DES chains.
        "ISO9797_ALGORITHM1" | "ISO9797_ALGORITHM3" => &["TDES_2KEY", "TDES_3KEY"],
        // CMAC is defined for the algorithms that name a block cipher.
        "CMAC" => &["AES_128", "AES_192", "AES_256", "TDES_2KEY", "TDES_3KEY"],
        // `HMAC` names whichever hash the KEY selects, so any HMAC key computes it.
        "HMAC" => &[
            "HMAC_SHA224",
            "HMAC_SHA256",
            "HMAC_SHA384",
            "HMAC_SHA512",
        ],
        // An explicit `HMAC_SHA*` names a hash itself, and has to name the one the key
        // selects.
        "HMAC_SHA224" => &["HMAC_SHA224"],
        "HMAC_SHA256" => &["HMAC_SHA256"],
        "HMAC_SHA384" => &["HMAC_SHA384"],
        "HMAC_SHA512" => &["HMAC_SHA512"],
        _ => return None,
    })
}

/// Whether the key `key_algorithm` names may compute the MAC `algorithm` names.
///
/// The ALGORITHM-COMPATIBILITY half of a MAC request, separated from the computation so
/// an operation can judge it where AWS judges it: before the key's `KeyModesOfUse` are
/// consulted, so a data-encryption key handed to a MAC operation is told which key it
/// needs rather than which mode of use to widen. `Ok` for an algorithm with no
/// computation here — that request is answered where the computation is reached, as the
/// algorithm it does not implement rather than as a key that cannot perform it.
pub fn mac_key_check(algorithm: &str, key_algorithm: &str) -> Result<(), MacRefusal> {
    match mac_key_algorithms(algorithm) {
        Some(admitted) if !admitted.contains(&key_algorithm) => {
            Err(wrong_key(algorithm, key_algorithm))
        }
        _ => Ok(()),
    }
}

/// The key material of a key the algorithm ADMITS turned out not to key the computation.
///
/// Not the compatibility refusal above: the algorithm pair was already admitted by
/// [`mac_key_check`], so what failed is the length or the form of the working material
/// this service derived for the key ([`working_material`] always derives the algorithm's
/// own key length, so this is unreachable), and that is this emulator's invariant rather
/// than the caller's request.
fn unusable_material(algorithm: &str, key_algorithm: &str) -> MacRefusal {
    MacRefusal::Unusable(format!(
        "The working material held for this {key_algorithm} key does not key the \
         {algorithm} computation."
    ))
}

/// ISO 9797-1 MAC algorithm 1: the Triple DES CBC-MAC of the padded message.
fn iso9797_algorithm1(
    key_algorithm: &str,
    material: &[u8],
    message: &[u8],
) -> Result<Vec<u8>, MacRefusal> {
    mac_key_check("ISO9797_ALGORITHM1", key_algorithm)?;
    let Some(cipher) = BlockCipher::new(key_algorithm, material) else {
        return Err(unusable_material("ISO9797_ALGORITHM1", key_algorithm));
    };
    Ok(cbc_mac(&cipher, &padded(message, cipher.block_len())))
}

/// ISO 9797-1 MAC algorithm 3, the retail MAC: a single-DES CBC-MAC under the key's
/// left half, finished by a decrypt under its right half and an encrypt under the
/// left half again.
fn iso9797_algorithm3(
    key_algorithm: &str,
    material: &[u8],
    message: &[u8],
) -> Result<Vec<u8>, MacRefusal> {
    mac_key_check("ISO9797_ALGORITHM3", key_algorithm)?;
    if material.len() < RETAIL_MAC_KEY_BYTES {
        return Err(unusable_material("ISO9797_ALGORITHM3", key_algorithm));
    }
    let (left, right) = (&material[..8], &material[8..16]);
    let Some(first) = single_des(left) else {
        return Err(unusable_material("ISO9797_ALGORITHM3", key_algorithm));
    };
    let Some(second) = single_des(right) else {
        return Err(unusable_material("ISO9797_ALGORITHM3", key_algorithm));
    };
    let mut mac = cbc_mac(&first, &padded(message, first.block_len()));
    second.decrypt_block(&mut mac);
    first.encrypt_block(&mut mac);
    Ok(mac)
}

/// The CMAC of the key's own block cipher. Defined for the AES and Triple DES
/// algorithms, which are the ones that name a block cipher.
fn cmac(key_algorithm: &str, material: &[u8], message: &[u8]) -> Result<Vec<u8>, MacRefusal> {
    mac_key_check("CMAC", key_algorithm)?;
    let mac = match key_algorithm {
        "AES_128" => <Cmac<Aes128> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "AES_192" => <Cmac<Aes192> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "AES_256" => <Cmac<Aes256> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "TDES_2KEY" => <Cmac<TdesEde2> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "TDES_3KEY" => <Cmac<TdesEde3> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        _ => None,
    };
    mac.ok_or_else(|| unusable_material("CMAC", key_algorithm))
}

/// The HMAC of the hash the key's algorithm selects.
///
/// `HMAC` names whichever hash the KEY selects — "the hash selected at key creation",
/// as the control plane's `KeyCheckValueAlgorithm` documentation puts it — while an
/// explicit `HMAC_SHA*` names a hash itself, and has to name the one the key selects.
fn hmac(
    algorithm: &str,
    key_algorithm: &str,
    material: &[u8],
    message: &[u8],
) -> Result<Vec<u8>, MacRefusal> {
    mac_key_check(algorithm, key_algorithm)?;
    let mac = match key_algorithm {
        "HMAC_SHA224" => <Hmac<Sha224> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "HMAC_SHA256" => <Hmac<Sha256> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "HMAC_SHA384" => <Hmac<Sha384> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        "HMAC_SHA512" => <Hmac<Sha512> as Mac>::new_from_slice(material)
            .ok()
            .map(|mac| mac.chain_update(message).finalize().into_bytes().to_vec()),
        _ => None,
    };
    mac.ok_or_else(|| unusable_material(algorithm, key_algorithm))
}

/// The single-DES cipher keyed by `key`, one half of a double-length Triple DES key.
fn single_des(key: &[u8]) -> Option<BlockCipher> {
    Some(BlockCipher::Des(
        <Des as KeyInit>::new_from_slice(key).ok()?,
    ))
}

/// ISO 9797-1 padding method 1: zero bytes appended until the message fills whole
/// blocks (and one whole block for an empty message).
fn padded(message: &[u8], block_len: usize) -> Vec<u8> {
    let mut padded = message.to_vec();
    let remainder = padded.len() % block_len;
    if padded.is_empty() || remainder != 0 {
        padded.resize(padded.len() + (block_len - remainder), 0);
    }
    padded
}

/// The CBC-MAC of `data` (whole blocks) with a zero IV: the last ciphertext block.
fn cbc_mac(cipher: &BlockCipher, data: &[u8]) -> Vec<u8> {
    let mut state = vec![0u8; cipher.block_len()];
    for block in data.chunks(cipher.block_len()) {
        for (byte, message) in state.iter_mut().zip(block.iter()) {
            *byte ^= message;
        }
        cipher.encrypt_block(&mut state);
    }
    state
}

/// The service code a CVV2 (and an American Express CSC) is computed under.
///
/// The CVV2 variants of `CardGenerationAttributes` carry a card expiry date and no
/// service code — `CardVerificationValue2` models that one member alone — because the
/// CVV2 computation takes the service code `000`.
pub const CVV2_SERVICE_CODE: &str = "000";

/// How many hex characters the card verification value's data block carries: two
/// eight-byte blocks.
const CARD_DATA_BLOCK_DIGITS: usize = 32;

/// The card verification value of `pan`, `expiry_date` and `service_code` under
/// `material`, `length` digits long, or the reason it could not be computed.
///
/// The published CVV/CVC computation: the account number, the expiry date and the
/// service code are concatenated, padded right with zeros to two eight-byte blocks,
/// the first block is encrypted under the key's left half, XORed with the second
/// block and encrypted under the whole double-length key; the result's hex digits are
/// then decimalised — the decimal digits in order first, then the remaining
/// characters each reduced by ten — and the leading `length` of them are the value.
pub fn card_validation_data(
    key_algorithm: &str,
    material: &[u8],
    pan: &str,
    expiry_date: &str,
    service_code: &str,
    length: usize,
) -> Result<String, String> {
    if !matches!(key_algorithm, "TDES_2KEY" | "TDES_3KEY")
        || material.len() < RETAIL_MAC_KEY_BYTES
    {
        return Err(format!(
            "KeyAlgorithm {key_algorithm} is not valid for card validation data."
        ));
    }
    let mut digits = format!("{pan}{expiry_date}{service_code}");
    if digits.len() > CARD_DATA_BLOCK_DIGITS || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "The account number, expiry date and service code form {} digits, which does not fit \
             the {CARD_DATA_BLOCK_DIGITS}-digit card data block.",
            digits.len()
        ));
    }
    while digits.len() < CARD_DATA_BLOCK_DIGITS {
        digits.push('0');
    }
    let block = hex::decode(&digits).map_err(|_| {
        "The account number, expiry date and service code are not decimal digits.".to_string()
    })?;
    let (left, right) = (&material[..8], &material[8..16]);
    let (Some(first), Some(second)) = (single_des(left), single_des(right)) else {
        return Err(format!(
            "KeyAlgorithm {key_algorithm} is not valid for card validation data."
        ));
    };
    let mut state = block[..8].to_vec();
    first.encrypt_block(&mut state);
    for (byte, tail) in state.iter_mut().zip(block[8..].iter()) {
        *byte ^= tail;
    }
    first.encrypt_block(&mut state);
    second.decrypt_block(&mut state);
    first.encrypt_block(&mut state);
    let value = decimalised(&hex::encode_upper(state), length);
    if value.len() < length {
        return Err("The card validation data could not be computed.".to_string());
    }
    Ok(value)
}

/// The leading `length` decimal digits of `hex`, by the published decimalisation: the
/// characters `0`-`9` in the order they appear, then the remaining characters each
/// reduced by ten (`A` -> 0 … `F` -> 5).
fn decimalised(hex: &str, length: usize) -> String {
    let mut digits: String = hex.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < length {
        let letters = hex
            .chars()
            .filter_map(|c| c.to_digit(16).filter(|value| *value >= 10))
            .map(|value| char::from_digit(value - 10, 10).unwrap_or('0'));
        digits.extend(letters);
    }
    digits.truncate(length);
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AES-128 key, IV and first plaintext block of the worked examples in NIST
    /// SP 800-38A appendix F, which every mode below is pinned against.
    const NIST_KEY: &str = "2b7e151628aed2a6abf7158809cf4f3c";
    const NIST_IV: &str = "000102030405060708090a0b0c0d0e0f";
    const NIST_PLAINTEXT: &str = "6bc1bee22e409f96e93d7e117393172a";

    /// One mode, the ciphertext NIST SP 800-38A appendix F publishes for it over the
    /// first plaintext block, and the mode name the model uses.
    ///
    /// ECB F.1.1, CBC F.2.1, CFB8 F.3.7, CFB128 F.3.13, OFB F.4.1 — each the AES-128
    /// example, whose first block is what a single-block message exercises.
    fn nist_vectors() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ECB", "3AD77BB40D7A3660A89ECAF32466EF97"),
            ("CBC", "7649ABAC8119B246CEE98E9B12E9197D"),
            ("CFB8", "3B79424C9C0DD436BACE9E0ED4586A4F"),
            ("CFB128", "3B3FD92EB72DAD20333449F8E83CFB4A"),
            ("OFB", "3B3FD92EB72DAD20333449F8E83CFB4A"),
        ]
    }

    #[test]
    fn the_modes_match_the_published_vectors() {
        let material = hex::decode(NIST_KEY).expect("the key is hex");
        let iv = hex::decode(NIST_IV).expect("the IV is hex");
        let data = hex::decode(NIST_PLAINTEXT).expect("the plaintext is hex");
        for (mode, expected) in nist_vectors() {
            // ECB takes no IV, and the published ECB example uses none.
            let initialization_vector = (mode != "ECB").then_some(iv.as_slice());
            let request = SymmetricRequest {
                key_algorithm: "AES_128",
                material: &material,
                mode,
                initialization_vector,
                data: &data,
            };
            let cipher_text = hex::encode_upper(
                symmetric_encrypt(&request).unwrap_or_else(|e| panic!("mode {mode}: {e}")),
            );
            assert_eq!(
                cipher_text, expected,
                "mode {mode}: expected the published ciphertext {expected}, got {cipher_text}"
            );
            let round_tripped = hex::encode_upper(
                symmetric_decrypt(&SymmetricRequest {
                    data: &hex::decode(&cipher_text).expect("the ciphertext is hex"),
                    ..request
                })
                .unwrap_or_else(|e| panic!("mode {mode}: {e}")),
            );
            assert_eq!(
                round_tripped,
                NIST_PLAINTEXT.to_uppercase(),
                "mode {mode}: decrypt must return the plaintext, got {round_tripped}"
            );
        }
    }

    #[test]
    fn every_modelled_mode_round_trips_under_every_symmetric_cipher() {
        let modes = ["ECB", "CBC", "CFB", "CFB1", "CFB8", "CFB64", "CFB128", "OFB"];
        for key_algorithm in ["TDES_2KEY", "TDES_3KEY", "AES_128", "AES_192", "AES_256"] {
            let material = working_material(key_algorithm, "arn:key", "A1B2C3")
                .unwrap_or_else(|| panic!("{key_algorithm} has working material"));
            let cipher = BlockCipher::new(key_algorithm, &material).expect("a block cipher");
            let data = b"0123456789abcdef".repeat(3)[..cipher.block_len() * 3].to_vec();
            for mode in modes {
                let request = SymmetricRequest {
                    key_algorithm,
                    material: &material,
                    mode,
                    initialization_vector: None,
                    data: &data,
                };
                let case = format!("{key_algorithm}/{mode}");
                // A CFB segment wider than the cipher's block names no mode for it.
                let Ok(cipher_text) = symmetric_encrypt(&request) else {
                    assert!(
                        mode == "CFB128" && cipher.block_len() == 8,
                        "case {case}: only a segment wider than the block is refused"
                    );
                    continue;
                };
                assert_ne!(
                    cipher_text, data,
                    "case {case}: the ciphertext must not be the plaintext"
                );
                let plain = symmetric_decrypt(&SymmetricRequest {
                    data: &cipher_text,
                    ..request
                })
                .unwrap_or_else(|e| panic!("case {case}: {e}"));
                assert_eq!(plain, data, "case {case}: decrypt must return the plaintext");
            }
        }
    }

    #[test]
    fn working_material_is_stable_and_key_bound() {
        let first = working_material("AES_128", "arn:one", "A1B2C3").expect("material");
        let again = working_material("AES_128", "arn:one", "A1B2C3").expect("material");
        let other_key = working_material("AES_128", "arn:two", "A1B2C3").expect("material");
        let other_kcv = working_material("AES_128", "arn:one", "D4E5F6").expect("material");
        assert_eq!(first, again, "the same key must derive the same material");
        assert_ne!(
            first, other_key,
            "a different key ARN must derive different material"
        );
        assert_ne!(
            first, other_kcv,
            "a different check value must derive different material"
        );
        assert_eq!(
            working_material("AES_256", "arn:one", "A1B2C3")
                .expect("material")
                .len(),
            32,
            "AES_256 takes 32 bytes of material"
        );
        assert!(
            working_material("RSA_2048", "arn:one", "A1B2C3").is_none(),
            "an asymmetric algorithm has no symmetric working material"
        );
    }

    #[test]
    fn the_retail_mac_is_the_cbc_mac_finished_under_the_second_half() {
        let material = working_material("TDES_2KEY", "arn:mac", "A1B2C3").expect("material");
        // On a message of ONE block the two ISO 9797 algorithms coincide: algorithm 1
        // under a double-length Triple DES key is E(D(E(m))) over that block, which is
        // exactly algorithm 3's single-DES chain finished under the second half. On a
        // longer message they part, because algorithm 1 chains under Triple DES and
        // algorithm 3 chains under single DES.
        let one_block = b"12345678";
        let two_blocks = b"1234567887654321";
        let short_1 = iso9797_algorithm1("TDES_2KEY", &material, one_block).expect("algorithm 1");
        let short_3 = iso9797_algorithm3("TDES_2KEY", &material, one_block).expect("algorithm 3");
        let long_1 = iso9797_algorithm1("TDES_2KEY", &material, two_blocks).expect("algorithm 1");
        let long_3 = iso9797_algorithm3("TDES_2KEY", &material, two_blocks).expect("algorithm 3");
        assert_eq!(short_1.len(), 8, "a Triple DES CBC-MAC is one 8-byte block");
        assert_eq!(short_3.len(), 8, "a retail MAC is one 8-byte block");
        assert_eq!(
            short_1, short_3,
            "on one block the two algorithms are the same computation, got {short_1:?} and \
             {short_3:?}"
        );
        assert_ne!(
            long_1, long_3,
            "on two blocks the two algorithms chain differently, got {long_1:?} twice"
        );
        assert_ne!(
            short_1, long_1,
            "the MAC must depend on the whole message, got {short_1:?} twice"
        );
    }

    #[test]
    fn a_mac_is_refused_for_a_key_that_cannot_compute_it() {
        let aes = working_material("AES_128", "arn:mac", "A1B2C3").expect("material");
        assert!(
            matches!(
                generate_mac("ISO9797_ALGORITHM1", "AES_128", &aes, b"00"),
                Err(MacRefusal::Invalid(_))
            ),
            "an AES key computes no Triple DES CBC-MAC"
        );
        assert!(
            matches!(
                generate_mac("HMAC_SHA256", "AES_128", &aes, b"00"),
                Err(MacRefusal::Invalid(_))
            ),
            "an AES key computes no HMAC"
        );
        assert!(
            matches!(
                generate_mac("AS2805_4_1", "AES_128", &aes, b"00"),
                Err(MacRefusal::NotImplemented(_))
            ),
            "the one algorithm with no computation here says so"
        );
    }

    #[test]
    fn decimalisation_takes_the_decimal_digits_then_the_reduced_letters() {
        // The published decimalisation, on a block with too few decimal digits to
        // fill the value: the digits first, then A..F reduced by ten.
        assert_eq!(decimalised("ABCDEF0123456789", 3), "012");
        assert_eq!(decimalised("ABCDEF", 5), "01234");
        assert_eq!(decimalised("1A2B3C4D5E6F7081", 5), "12345");
    }
}
