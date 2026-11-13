// SPDX-License-Identifier: CC0-1.0

//! The initial DUKPT key an IPEK export derives from a base derivation key.
//!
//! One concern: `ExportKey`'s `ExportDukptInitialKey` attribute asks for an Initial Pin
//! Encryption Key instead of the key under export itself — "IPEK is generated from BDK
//! (Base Derivation Key) and `ExportDukptInitialKey` attribute KSN
//! (`KeySerialNumber`)", and "the generated IPEK does not persist within Amazon Web
//! Services Payment Cryptography and has to be re-generated each time during export"
//! (the operation's own documentation). So this module derives it, nothing stores it,
//! and the same base derivation key and key serial number always derive the same one.
//!
//! Both DUKPT families are derived the way their standard defines, through the ciphers
//! in [`super::block_cipher`]:
//!
//! * a TDES base derivation key derives a double-length initial key as the two halves
//!   the key serial number's initial id encrypts to — once under the base derivation
//!   key, once under the variant of it the standard names;
//! * an AES base derivation key derives an initial key of its own length from
//!   derivation data naming the version, the counter, the key usage, the algorithm, the
//!   key length and the initial key id.

use super::block_cipher::{aes_encrypt_block, tdes_encrypt_blocks, AES_BLOCK, TDES_BLOCK};

/// How many bytes the key serial number of a TDES DUKPT key carries.
const TDES_KSN_LEN: usize = 10;

/// How many bytes the key serial number of an AES DUKPT key carries: the initial key id
/// and the transaction counter.
const AES_KSN_LEN: usize = 12;

/// How many bytes of a key serial number name the initial key, in both families.
const INITIAL_KEY_ID_LEN: usize = 8;

/// The bits of a TDES key serial number's eighth byte that belong to the initial key
/// id; the rest are the transaction counter, which the derivation clears.
const TDES_INITIAL_ID_MASK: u8 = 0xE0;

/// The variant of a TDES base derivation key the right half of the initial key is
/// encrypted under, from the standard's own constant.
const TDES_VARIANT_MASK: [u8; 16] = [
    0xC0, 0xC0, 0xC0, 0xC0, 0x00, 0x00, 0x00, 0x00, 0xC0, 0xC0, 0xC0, 0xC0, 0x00, 0x00, 0x00, 0x00,
];

/// The `KeyAlgorithm` of the initial key a TDES base derivation key derives: the
/// double-length TDES key the standard defines the derivation for.
const TDES_INITIAL_KEY_ALGORITHM: &str = "TDES_2KEY";

/// The version byte of AES DUKPT derivation data.
const AES_DERIVE_VERSION: u8 = 0x01;

/// The key usage indicator of AES DUKPT derivation data for the initial key: key
/// derivation, initial key.
const AES_DERIVE_USAGE: [u8; 2] = [0x80, 0x01];

/// One derived initial key: its material and the `KeyAlgorithm` that material is a key
/// of, which is what the exported key block's header and check value are built from.
pub(super) struct InitialKey {
    pub material: Vec<u8>,
    pub key_algorithm: &'static str,
}

/// The initial DUKPT key `key_serial_number` names under the base derivation key
/// `bdk`, or why the pair derives none.
///
/// Every refusal names the rule it breaks: the key serial number's length belongs to
/// one DUKPT family, and the base derivation key's algorithm has to be of that same
/// family.
pub(super) fn initial_key(
    bdk_algorithm: &str,
    bdk: &[u8],
    key_serial_number: &str,
) -> Result<InitialKey, String> {
    let ksn = hex::decode(key_serial_number).map_err(|_| {
        format!(
            "ExportAttributes.ExportDukptInitialKey.KeySerialNumber {key_serial_number:?} is not \
             hexadecimal"
        )
    })?;
    match bdk_algorithm {
        "TDES_2KEY" => tdes_initial_key(bdk, &ksn),
        "AES_128" | "AES_192" | "AES_256" => aes_initial_key(bdk_algorithm, bdk, &ksn),
        other => Err(format!(
            "an initial DUKPT key is derived from a {TDES_INITIAL_KEY_ALGORITHM} or an AES base \
             derivation key, and the key under export is a {other} key"
        )),
    }
}

/// The double-length initial key a TDES base derivation key derives.
///
/// The key serial number's initial key id — its leftmost bytes, with the transaction
/// counter cleared — is encrypted twice: under the base derivation key for the left
/// half of the initial key, and under the variant of that key for the right half.
fn tdes_initial_key(bdk: &[u8], ksn: &[u8]) -> Result<InitialKey, String> {
    if ksn.len() != TDES_KSN_LEN {
        return Err(format!(
            "a TDES DUKPT key serial number carries {TDES_KSN_LEN} bytes, and the one named \
             carries {}",
            ksn.len()
        ));
    }
    let failed =
        || "the initial DUKPT key could not be derived from the key under export".to_string();
    let mut initial_id = ksn
        .get(..INITIAL_KEY_ID_LEN)
        .ok_or_else(failed)?
        .to_vec();
    if let Some(last) = initial_id.last_mut() {
        *last &= TDES_INITIAL_ID_MASK;
    }

    let mut variant = bdk.to_vec();
    for (byte, mask) in variant.iter_mut().zip(TDES_VARIANT_MASK.iter()) {
        *byte ^= mask;
    }

    let left = tdes_encrypt_blocks(bdk, &initial_id).ok_or_else(failed)?;
    let right = tdes_encrypt_blocks(&variant, &initial_id).ok_or_else(failed)?;
    let mut material = Vec::with_capacity(TDES_BLOCK * 2);
    material.extend_from_slice(&left);
    material.extend_from_slice(&right);
    Ok(InitialKey {
        material,
        key_algorithm: TDES_INITIAL_KEY_ALGORITHM,
    })
}

/// The initial key an AES base derivation key derives: as many blocks of the
/// standard's derivation data as the key length needs, each encrypted under the base
/// derivation key, truncated to that length.
fn aes_initial_key(
    bdk_algorithm: &str,
    bdk: &[u8],
    ksn: &[u8],
) -> Result<InitialKey, String> {
    if ksn.len() != AES_KSN_LEN {
        return Err(format!(
            "an AES DUKPT key serial number carries {AES_KSN_LEN} bytes, and the one named \
             carries {}",
            ksn.len()
        ));
    }
    let failed =
        || "the initial DUKPT key could not be derived from the key under export".to_string();
    let initial_id = ksn.get(..INITIAL_KEY_ID_LEN).ok_or_else(failed)?;
    let (algorithm, key_algorithm) = match bdk_algorithm {
        "AES_128" => ([0x00, 0x02], "AES_128"),
        "AES_192" => ([0x00, 0x03], "AES_192"),
        "AES_256" => ([0x00, 0x04], "AES_256"),
        _ => return Err(failed()),
    };
    let bits = u16::try_from(bdk.len().checked_mul(8).ok_or_else(failed)?)
        .map_err(|_| failed())?;

    let mut material: Vec<u8> = Vec::with_capacity(bdk.len());
    let mut counter: u8 = 1;
    while material.len() < bdk.len() {
        let mut data = vec![AES_DERIVE_VERSION, counter];
        data.extend_from_slice(&AES_DERIVE_USAGE);
        data.extend_from_slice(&algorithm);
        data.extend_from_slice(&bits.to_be_bytes());
        data.extend_from_slice(initial_id);
        if data.len() != AES_BLOCK {
            return Err(failed());
        }
        material.extend_from_slice(&aes_encrypt_block(bdk, &data).ok_or_else(failed)?);
        counter = counter.checked_add(1).ok_or_else(failed)?;
    }
    material.truncate(bdk.len());
    Ok(InitialKey {
        material,
        key_algorithm,
    })
}
