// SPDX-License-Identifier: CC0-1.0

//! The ASC X9.143 (TR-31) key block an exported key is wrapped in.
//!
//! One concern: turning a key's stored material, plus the header fields a caller asked
//! for, into the ASCII key block `ExportKey` answers with — encrypted and
//! authenticated under a wrapping key this store also holds. The key material itself
//! never leaves [`super`] in the clear: this module reads it, wraps it, and hands back
//! the block.
//!
//! The block is the one the standard defines, built with real primitives
//! ([`super::block_cipher`]):
//!
//! * the key block encryption key and the key block MAC key are DERIVED from the
//!   wrapping key (the key block protection key) by the CMAC-based derivation the
//!   standard's key derivation binding method uses — version `B` for a TDES protection
//!   key, version `D` for an AES one;
//! * the MAC is a CMAC over the header, the optional blocks and the plaintext payload;
//! * that MAC is the initialisation vector of the payload's CBC encryption, so the
//!   authentication is bound to the ciphertext.
//!
//! What is written here is the block's own layout, which no crate on the registry
//! implements; every cipher, MAC and chaining step is a crate's.

use std::collections::BTreeMap;

use crate::ops::types::KeyModesOfUse;

use super::block_cipher::{
    aes_cbc_encrypt, aes_cmac, tdes_cbc_encrypt, tdes_cmac, AES_BLOCK, TDES_BLOCK,
};

/// The version id of a key block whose protection key is a TDES key: the TDES key
/// derivation binding method.
const VERSION_TDES: char = 'B';

/// The version id of a key block whose protection key is an AES key: the AES key
/// derivation binding method.
const VERSION_AES: char = 'D';

/// How many characters the key block header occupies: version id, length, key usage,
/// algorithm, mode of use, key version, exportability, optional block count and the two
/// reserved characters.
const HEADER_LEN: usize = 16;

/// The largest key block the header's 4-character length field can describe.
pub(super) const MAX_BLOCK_LEN: usize = 9_999;

/// The most optional blocks the header's 2-character count field can describe.
pub(super) const MAX_OPTIONAL_BLOCKS: usize = 99;

/// The optional block id the standard reserves for the padding block that aligns the
/// header region, so a caller cannot supply one of its own.
const PADDING_BLOCK_ID: &str = "PB";

/// The shortest optional block: its two-character id and its two-character length.
const OPTIONAL_BLOCK_OVERHEAD: usize = 4;

/// The key version number a key block carries when the key has no version of its own,
/// from the standard's "key version number is not used" value.
pub(super) const KEY_VERSION_UNUSED: &str = "00";

/// The derivation data's key usage indicator for the key block ENCRYPTION key.
const DERIVE_USAGE_ENCRYPTION: [u8; 2] = [0x00, 0x00];

/// The derivation data's key usage indicator for the key block MAC key.
const DERIVE_USAGE_MAC: [u8; 2] = [0x00, 0x01];

/// The separator the derivation data carries between the key usage indicator and the
/// algorithm.
const DERIVE_SEPARATOR: u8 = 0x00;

/// One key block to build: what is wrapped, what the header says about it, and the key
/// it is wrapped under.
pub(super) struct KeyBlock<'a> {
    /// The key material the block carries.
    pub payload: &'a [u8],
    /// The `KeyAlgorithm` of that material, which the header's algorithm comes from.
    pub payload_algorithm: &'a str,
    /// The two-character key usage the header carries.
    pub usage_code: &'a str,
    /// The modes of use the header's mode of use comes from.
    pub modes_of_use: &'a KeyModesOfUse,
    /// The header's exportability character.
    pub exportability: char,
    /// The header's two-character key version number.
    pub key_version: &'a str,
    /// The optional blocks the header carries, in one stable order.
    pub optional_blocks: &'a BTreeMap<String, String>,
    /// The key block protection key: the wrapping key's own stored material.
    pub protection_key: &'a [u8],
    /// That key's `KeyAlgorithm`, which decides the binding method.
    pub protection_key_algorithm: &'a str,
    /// Bytes the payload's trailing padding is taken from, so two exports of one key
    /// are not the same block.
    pub padding: &'a [u8],
}

/// Build `block`, or say why it cannot be built.
///
/// Every refusal names the member that cannot be rendered and what the standard admits
/// there, so a caller can fix the request from the sentence alone.
pub(super) fn build(block: &KeyBlock<'_>) -> Result<String, String> {
    let (version, block_size, mac_len) = binding_method(block.protection_key_algorithm)?;
    let algorithm = algorithm_code(block.payload_algorithm).ok_or_else(|| {
        format!(
            "a TR-31 key block carries no algorithm character for a {} key",
            block.payload_algorithm
        )
    })?;
    let mode = mode_of_use(block.modes_of_use).ok_or_else(|| {
        "KeyModesOfUse names no mode of use a TR-31 key block header can carry".to_string()
    })?;
    if block.usage_code.len() != 2 {
        return Err(format!(
            "a TR-31 key block header carries a two-character key usage, not {:?}",
            block.usage_code
        ));
    }
    if block.key_version.len() != 2 {
        return Err(format!(
            "KeyBlockHeaders.KeyVersion carries two characters, not {:?}",
            block.key_version
        ));
    }

    let (optional, optional_count) = optional_block_region(block.optional_blocks, block_size)?;
    let plaintext = payload(block.payload, block.padding, block_size)?;

    // The header's own length field covers the whole block, so it is computed BEFORE
    // the header is rendered: the header is part of what the MAC authenticates, and a
    // length filled in afterwards would not be the text that was authenticated.
    let total = HEADER_LEN
        .checked_add(optional.len())
        .and_then(|len| len.checked_add(plaintext.len().checked_mul(2)?))
        .and_then(|len| len.checked_add(mac_len.checked_mul(2)?))
        .ok_or_else(|| "the key under export does not fit a TR-31 key block".to_string())?;
    if total > MAX_BLOCK_LEN {
        return Err(format!(
            "the key block this request describes is {total} characters, more than the \
             {MAX_BLOCK_LEN} a TR-31 key block holds; name fewer or shorter optional blocks"
        ));
    }

    let header = format!(
        "{version}{total:04}{usage}{algorithm}{mode}{key_version}{exportability}\
         {optional_count:02}00",
        usage = block.usage_code,
        key_version = block.key_version,
        exportability = block.exportability,
    );

    let mut authenticated = Vec::with_capacity(header.len() + optional.len() + plaintext.len());
    authenticated.extend_from_slice(header.as_bytes());
    authenticated.extend_from_slice(optional.as_bytes());
    authenticated.extend_from_slice(&plaintext);

    let failed = || "the key under export could not be wrapped under the wrapping key".to_string();
    let encryption_key =
        derive(block.protection_key, DERIVE_USAGE_ENCRYPTION, version).ok_or_else(failed)?;
    let mac_key = derive(block.protection_key, DERIVE_USAGE_MAC, version).ok_or_else(failed)?;
    let mac = if version == VERSION_TDES {
        tdes_cmac(&mac_key, &authenticated)
    } else {
        aes_cmac(&mac_key, &authenticated)
    }
    .filter(|mac| mac.len() == mac_len)
    .ok_or_else(failed)?;
    let ciphertext = if version == VERSION_TDES {
        tdes_cbc_encrypt(&encryption_key, &mac, &plaintext)
    } else {
        aes_cbc_encrypt(&encryption_key, &mac, &plaintext)
    }
    .ok_or_else(failed)?;

    Ok(format!(
        "{header}{optional}{}{}",
        hex::encode_upper(&ciphertext),
        hex::encode_upper(&mac)
    ))
}

/// The binding method a key block protected by `key_algorithm` uses: its version id,
/// the block size of its cipher, and the length of its MAC.
///
/// The modelled `KeyAlgorithm` values are matched exactly, the rule every table in this
/// module follows, so an algorithm the model gains later is refused rather than wrapped
/// under a method that was not defined for it.
fn binding_method(key_algorithm: &str) -> Result<(char, usize, usize), String> {
    match key_algorithm {
        "TDES_2KEY" | "TDES_3KEY" => Ok((VERSION_TDES, TDES_BLOCK, TDES_BLOCK)),
        "AES_128" | "AES_192" | "AES_256" => Ok((VERSION_AES, AES_BLOCK, AES_BLOCK)),
        other => Err(format!(
            "a TR-31 key block is protected by a TDES or an AES key encryption key, and the \
             wrapping key named is a {other} key"
        )),
    }
}

/// The optional block region of the header, and how many blocks it carries.
///
/// Each block is its two-character id, its length as two hexadecimal characters
/// (counting the id and the length field themselves), then its value. A padding block
/// closes the region when the header and the blocks together are not a multiple of the
/// cipher's block size, which the standard requires them to be.
fn optional_block_region(
    blocks: &BTreeMap<String, String>,
    block_size: usize,
) -> Result<(String, usize), String> {
    if blocks.len() > MAX_OPTIONAL_BLOCKS {
        return Err(format!(
            "KeyBlockHeaders.OptionalBlocks names {} blocks, more than the \
             {MAX_OPTIONAL_BLOCKS} a TR-31 key block header holds",
            blocks.len()
        ));
    }
    let mut region = String::new();
    let mut count = 0usize;
    for (id, value) in blocks {
        if id.len() != 2 {
            return Err(format!(
                "KeyBlockHeaders.OptionalBlocks names the block id {id:?}, and an optional \
                 block id is two characters"
            ));
        }
        if id == PADDING_BLOCK_ID {
            return Err(format!(
                "KeyBlockHeaders.OptionalBlocks names the block id {PADDING_BLOCK_ID}, which a \
                 TR-31 key block reserves for its padding block"
            ));
        }
        let len = value
            .len()
            .checked_add(OPTIONAL_BLOCK_OVERHEAD)
            .filter(|len| *len <= 0xFF)
            .ok_or_else(|| {
                format!(
                    "the optional block {id} carries {} characters, more than one TR-31 optional \
                     block holds",
                    value.len()
                )
            })?;
        region.push_str(id);
        region.push_str(&format!("{len:02X}"));
        region.push_str(value);
        count += 1;
    }

    let aligned = HEADER_LEN
        .checked_add(region.len())
        .ok_or_else(|| "the optional blocks do not fit a TR-31 key block".to_string())?;
    let mut filler = (block_size - aligned % block_size) % block_size;
    if filler > 0 {
        // A padding block cannot be shorter than its own id and length field, so a
        // deficit smaller than that takes one more whole cipher block.
        if filler < OPTIONAL_BLOCK_OVERHEAD {
            filler += block_size;
        }
        region.push_str(PADDING_BLOCK_ID);
        region.push_str(&format!("{filler:02X}"));
        for _ in 0..(filler - OPTIONAL_BLOCK_OVERHEAD) {
            region.push('0');
        }
        count += 1;
    }
    if count > MAX_OPTIONAL_BLOCKS {
        return Err(format!(
            "KeyBlockHeaders.OptionalBlocks needs a padding block, which would take the header \
             past the {MAX_OPTIONAL_BLOCKS} optional blocks a TR-31 key block holds"
        ));
    }
    Ok((region, count))
}

/// The plaintext payload of a key block: the key's length in bits, the key itself, then
/// padding up to a whole number of cipher blocks.
fn payload(key: &[u8], padding: &[u8], block_size: usize) -> Result<Vec<u8>, String> {
    let bits = key
        .len()
        .checked_mul(8)
        .and_then(|bits| u16::try_from(bits).ok())
        .ok_or_else(|| {
            "the key under export is longer than a TR-31 key block payload holds".to_string()
        })?;
    let mut plaintext = Vec::with_capacity(block_size);
    plaintext.extend_from_slice(&bits.to_be_bytes());
    plaintext.extend_from_slice(key);
    let filler = (block_size - plaintext.len() % block_size) % block_size;
    let filler = padding.get(..filler).ok_or_else(|| {
        "the key under export could not be padded to a whole TR-31 key block".to_string()
    })?;
    plaintext.extend_from_slice(filler);
    Ok(plaintext)
}

/// The key block encryption or MAC key derived from the protection key `kbpk`.
///
/// The derivation is the standard's: a CMAC under the protection key over derivation
/// data naming the counter, which of the two keys is being derived, the algorithm and
/// the key length — run once per cipher block of the key being derived, so a key longer
/// than one block is the concatenation of consecutive counters.
fn derive(kbpk: &[u8], usage: [u8; 2], version: char) -> Option<Vec<u8>> {
    let (algorithm, cipher_block) = derivation_algorithm(kbpk.len(), version)?;
    let bits = u16::try_from(kbpk.len().checked_mul(8)?).ok()?;
    let mut derived: Vec<u8> = Vec::with_capacity(kbpk.len());
    let mut counter: u8 = 1;
    while derived.len() < kbpk.len() {
        let mut data = vec![counter, usage[0], usage[1], DERIVE_SEPARATOR];
        data.extend_from_slice(&algorithm);
        data.extend_from_slice(&bits.to_be_bytes());
        // One cipher block of derivation data, zero filled.
        data.resize(cipher_block, 0);
        let block = if version == VERSION_TDES {
            tdes_cmac(kbpk, &data)?
        } else {
            aes_cmac(kbpk, &data)?
        };
        derived.extend_from_slice(&block);
        counter = counter.checked_add(1)?;
    }
    derived.truncate(kbpk.len());
    Some(derived)
}

/// The derivation data's algorithm indicator for a protection key of `key_len` bytes
/// under `version`, and the cipher block that version's derivation data fills.
fn derivation_algorithm(key_len: usize, version: char) -> Option<([u8; 2], usize)> {
    let algorithm = match (version, key_len) {
        (VERSION_TDES, 16) => [0x00, 0x00],
        (VERSION_TDES, 24) => [0x00, 0x01],
        (VERSION_AES, 16) => [0x00, 0x02],
        (VERSION_AES, 24) => [0x00, 0x03],
        (VERSION_AES, 32) => [0x00, 0x04],
        _ => return None,
    };
    let cipher_block = if version == VERSION_TDES {
        TDES_BLOCK
    } else {
        AES_BLOCK
    };
    Some((algorithm, cipher_block))
}

/// The header's algorithm character for a key of `key_algorithm`.
///
/// The characters are the standard's: `T` for Triple DES, `A` for AES, `H` for HMAC,
/// `R` for RSA and `E` for an elliptic curve key. The modelled `KeyAlgorithm` values
/// (`smithy_query(query="shape",shape="KeyAlgorithm")`) are matched exactly rather than
/// by a name prefix, the rule the rest of this module's tables follow: an algorithm the
/// model gains later answers `None` and the export is refused instead of being rendered
/// under the wrong character.
fn algorithm_code(key_algorithm: &str) -> Option<char> {
    let code = match key_algorithm {
        "TDES_2KEY" | "TDES_3KEY" => 'T',
        "AES_128" | "AES_192" | "AES_256" => 'A',
        "HMAC_SHA224" | "HMAC_SHA256" | "HMAC_SHA384" | "HMAC_SHA512" => 'H',
        "RSA_2048" | "RSA_3072" | "RSA_4096" => 'R',
        "ECC_NIST_P256" | "ECC_NIST_P384" | "ECC_NIST_P521" => 'E',
        _ => return None,
    };
    Some(code)
}

/// The header's mode of use character for `modes`.
///
/// The standard gives one character per permitted use, and a key's nine modelled modes
/// of use can name a combination of them, so the rows are tried in the order below and
/// the first that holds is the mode the header carries: no restrictions, then the
/// signing combinations, then the enciphering ones, then generation, then derivation.
/// `None` for a key that permits nothing at all, which no mode of use describes.
fn mode_of_use(modes: &KeyModesOfUse) -> Option<char> {
    let enciphers = modes.encrypt || modes.wrap;
    let deciphers = modes.decrypt || modes.unwrap;
    let mode = if modes.no_restrictions {
        'N'
    } else if modes.sign && deciphers {
        'T'
    } else if modes.generate && modes.verify {
        'C'
    } else if modes.sign {
        'S'
    } else if modes.verify && !enciphers && !deciphers {
        'V'
    } else if enciphers && deciphers {
        'B'
    } else if enciphers {
        'E'
    } else if deciphers {
        'D'
    } else if modes.generate {
        'G'
    } else if modes.derive_key {
        'X'
    } else {
        return None;
    };
    Some(mode)
}
