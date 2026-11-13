// SPDX-License-Identifier: CC0-1.0

//! The block cipher primitives this service's key material is driven through.
//!
//! One concern: every place a key's stored bytes are handed to a cipher goes through
//! this module, so there is exactly one TDES key bundle expansion, one CMAC and one
//! CBC chaining in the crate — the key check value, the X9.143 key block and the
//! initial key derivation all call in here rather than carrying a copy of their own.
//!
//! Nothing here is written by hand: the ciphers come from `aes` and `des`, the CMAC
//! from `cmac` and the chaining from `cbc`. What this module adds is the dispatch from
//! a key's LENGTH to the right cipher, which the crates cannot do for a key whose
//! length is only known at run time.

use aes::cipher::block_padding::NoPadding;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use aes::{Aes128, Aes192, Aes256};
use cmac::{Cmac, Mac};
use des::TdesEde3;

/// How many bytes one TDES block carries.
pub(super) const TDES_BLOCK: usize = 8;

/// How many bytes one AES block carries.
pub(super) const AES_BLOCK: usize = 16;

/// The 24-byte EDE3 bundle a TDES key of 16 or 24 bytes drives.
///
/// A two-key TDES key is the standard K1|K2|K1 bundle, so both lengths drive the same
/// cipher. `None` for a length that is neither, so a key of the wrong size is refused
/// rather than silently truncated.
pub(super) fn tdes_bundle(key: &[u8]) -> Option<[u8; 24]> {
    let mut bundle = [0u8; 24];
    match key.len() {
        16 => {
            bundle.get_mut(..16)?.copy_from_slice(key);
            bundle.get_mut(16..)?.copy_from_slice(key.get(..8)?);
        }
        24 => bundle.copy_from_slice(key),
        _ => return None,
    }
    Some(bundle)
}

/// Every 8-byte block of `data`, encrypted under the TDES key `key` on its own: the ECB
/// use the key check value and the AS 2805 key cryptogram are both defined in terms of.
///
/// `None` when `key` is not a TDES key length or `data` is not a whole number of
/// blocks.
pub(super) fn tdes_encrypt_blocks(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || !data.len().is_multiple_of(TDES_BLOCK) {
        return None;
    }
    let cipher = <TdesEde3 as KeyInit>::new_from_slice(&tdes_bundle(key)?).ok()?;
    let mut encrypted = Vec::with_capacity(data.len());
    for plain in data.chunks(TDES_BLOCK) {
        let mut block = GenericArray::clone_from_slice(plain);
        cipher.encrypt_block(&mut block);
        encrypted.extend_from_slice(&block);
    }
    Some(encrypted)
}

/// One 16-byte block encrypted under the AES key `key`, whichever of the three AES key
/// lengths it carries.
pub(super) fn aes_encrypt_block(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() != AES_BLOCK {
        return None;
    }
    let mut block = GenericArray::clone_from_slice(data);
    match key.len() {
        16 => <Aes128 as KeyInit>::new_from_slice(key).ok()?.encrypt_block(&mut block),
        24 => <Aes192 as KeyInit>::new_from_slice(key).ok()?.encrypt_block(&mut block),
        32 => <Aes256 as KeyInit>::new_from_slice(key).ok()?.encrypt_block(&mut block),
        _ => return None,
    }
    Some(block.to_vec())
}

/// A CMAC over `data` under the TDES key `key`.
pub(super) fn tdes_cmac(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let mut mac = <Cmac<TdesEde3> as Mac>::new_from_slice(&tdes_bundle(key)?).ok()?;
    mac.update(data);
    Some(mac.finalize().into_bytes().to_vec())
}

/// A CMAC over `data` under the AES key `key`, whichever of the three AES key lengths
/// it carries.
pub(super) fn aes_cmac(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let mac = match key.len() {
        16 => {
            let mut mac = <Cmac<Aes128> as Mac>::new_from_slice(key).ok()?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        24 => {
            let mut mac = <Cmac<Aes192> as Mac>::new_from_slice(key).ok()?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        32 => {
            let mut mac = <Cmac<Aes256> as Mac>::new_from_slice(key).ok()?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        _ => return None,
    };
    Some(mac)
}

/// CBC encryption of `data` under the TDES key `key` from the initialisation vector
/// `iv`. `data` must already be a whole number of blocks: the mode applies no padding
/// of its own, because the payload it is used for carries the standard's own.
pub(super) fn tdes_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let bundle = tdes_bundle(key)?;
    let mut buffer = data.to_vec();
    let len = buffer.len();
    let encrypted = cbc::Encryptor::<TdesEde3>::new_from_slices(&bundle, iv)
        .ok()?
        .encrypt_padded_mut::<NoPadding>(&mut buffer, len)
        .ok()?;
    Some(encrypted.to_vec())
}

/// CBC encryption under an AES key, under the same rule as [`tdes_cbc_encrypt`].
pub(super) fn aes_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    let mut buffer = data.to_vec();
    let len = buffer.len();
    let encrypted = match key.len() {
        16 => cbc::Encryptor::<Aes128>::new_from_slices(key, iv)
            .ok()?
            .encrypt_padded_mut::<NoPadding>(&mut buffer, len)
            .ok()?,
        24 => cbc::Encryptor::<Aes192>::new_from_slices(key, iv)
            .ok()?
            .encrypt_padded_mut::<NoPadding>(&mut buffer, len)
            .ok()?,
        32 => cbc::Encryptor::<Aes256>::new_from_slices(key, iv)
            .ok()?
            .encrypt_padded_mut::<NoPadding>(&mut buffer, len)
            .ok()?,
        _ => return None,
    };
    Some(encrypted.to_vec())
}
