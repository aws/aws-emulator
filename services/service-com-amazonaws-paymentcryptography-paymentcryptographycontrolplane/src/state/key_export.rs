// SPDX-License-Identifier: CC0-1.0

//! What `ExportKey` asks of this store: which key is under export, which key wraps it,
//! and what the wrapped answer carries.
//!
//! One concern: the preconditions an export has to meet and the wrapped key it answers
//! with. A key's stored material never leaves this module in the clear — it goes into
//! an X9.143 key block ([`super::key_block`]) or an AS 2805 key cryptogram, both
//! encrypted under a wrapping key this same store holds, so what a caller receives is
//! readable only by someone who already holds that key.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    ExportAs2805KeyCryptogram, ExportDukptInitialKey, ExportTr31KeyBlock, KeyModesOfUse,
};

use super::block_cipher::{tdes_encrypt_blocks, AES_BLOCK, TDES_BLOCK};
use super::key_block::{self, KeyBlock, KEY_VERSION_UNUSED};
use super::{
    default_kcv_algorithm, dukpt, is_symmetric_algorithm, key_check_value,
    ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore,
    StoredKey, KEY_STATE_DELETE_COMPLETE,
};

/// The `WrappedKeyMaterialFormat` of a key exported in a TR-31 key block, from
/// `smithy_query(query="shape",shape="WrappedKeyMaterialFormat")`.
pub const WRAPPED_KEY_FORMAT_TR31: &str = "TR31_KEY_BLOCK";

/// The `WrappedKeyMaterialFormat` of a key exported as a cryptogram, from the same
/// shape.
pub const WRAPPED_KEY_FORMAT_CRYPTOGRAM: &str = "KEY_CRYPTOGRAM";

/// The `KeyClass` of one half of a key pair that is a public key, from
/// `smithy_query(query="shape",shape="KeyClass")`.
pub const KEY_CLASS_PUBLIC_KEY: &str = "PUBLIC_KEY";

/// The `KeyUsage` of a key that verifies digital signatures — what a certificate
/// authority public key imported into this service is for, from
/// `smithy_query(query="shape",shape="KeyUsage")`.
pub const KEY_USAGE_DIGITAL_SIGNATURE: &str = "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE";

/// The `KeyUsage` of a base derivation key, the key an initial DUKPT key is derived
/// from, from the same shape.
pub const KEY_USAGE_BASE_DERIVATION_KEY: &str = "TR31_B0_BASE_DERIVATION_KEY";

/// The `KeyUsage` values of a key that may wrap another key for export: the key
/// encryption key and the key block protection key, from the same shape.
pub const WRAPPING_KEY_USAGES: &[&str] = &[
    "TR31_K0_KEY_ENCRYPTION_KEY",
    "TR31_K1_KEY_BLOCK_PROTECTION_KEY",
];

/// The `KeyAlgorithm` values a key may be wrapped under for export.
///
/// "PCI requires specific minimum key strength of wrapping keys used to protect the
/// keys being exchanged electronically … wrapping keys used for transport must be at
/// least as strong as the key being protected" (the operation's own documentation), and
/// the ciphers a key block or a key cryptogram is encrypted with are the two symmetric
/// block ciphers — the modelled `KeyAlgorithm` values of those, from
/// `smithy_query(query="shape",shape="KeyAlgorithm")`. An HMAC key keys a MAC and
/// encrypts nothing, and an asymmetric key is half a pair.
pub const WRAPPING_KEY_ALGORITHMS: &[&str] =
    &["AES_128", "AES_192", "AES_256", "TDES_2KEY", "TDES_3KEY"];

/// The X9.143 key usage an initial DUKPT key's key block carries: the initial DUKPT
/// key, which is what an IPEK export wraps rather than the base derivation key it was
/// derived from.
const TR31_USAGE_INITIAL_DUKPT_KEY: &str = "B1";

/// How many bytes of padding one key block payload can need: one cipher block less one
/// byte, so a whole block is always enough whichever binding method is in use.
const MAX_PAYLOAD_PADDING: usize = AES_BLOCK;

/// Why a key could not be exported.
#[derive(Debug, Clone)]
pub enum ExportRejection {
    /// The identifier names no key this store holds, so there is nothing to export or
    /// to wrap with. Carries the identifier AS THE REQUEST GAVE IT, which is what the
    /// modelled `ResourceNotFoundException.ResourceId` reports.
    NotFound(String),
    /// The request is inadmissible; the message says why, naming the member.
    Invalid(String),
}

/// One exported key, as `ExportKey`'s `WrappedKey` reports it.
#[derive(Debug, Clone)]
pub struct WrappedKeyMaterial {
    /// The `KeyArn` of the key the material is wrapped under, read from the record the
    /// wrapping key identifier resolved to.
    pub wrapping_key_arn: String,
    /// Which of the modelled formats the material is in.
    pub wrapped_key_material_format: &'static str,
    /// The wrapped key material itself.
    pub key_material: String,
    /// The check value of the key that was wrapped, computed from the very bytes the
    /// material carries.
    pub key_check_value: String,
    /// The algorithm that check value was computed by.
    pub key_check_value_algorithm: String,
}

/// What one `ExportKey` call asks for, beside the mechanism's own parameters.
pub struct KeyExport<'a> {
    /// `ExportKeyIdentifier`: the key under export.
    pub export_key_identifier: &'a str,
    /// `ExportAttributes.ExportDukptInitialKey`, when the caller asked for an initial
    /// DUKPT key derived from the key under export instead of that key itself.
    pub dukpt_initial_key: Option<&'a ExportDukptInitialKey>,
    /// `ExportAttributes.KeyCheckValueAlgorithm`: the algorithm the answered check
    /// value is computed by, or the exported key's own default when the request names
    /// none.
    pub key_check_value_algorithm: Option<&'a str>,
}

/// The key material an export wraps: the key under export's own, or the initial DUKPT
/// key derived from it.
struct ExportPayload {
    material: Vec<u8>,
    key_algorithm: String,
    /// The two-character X9.143 key usage this material's key block header carries.
    usage_code: String,
    modes_of_use: KeyModesOfUse,
}

impl KeyStore {
    /// Export the key `export` names in a TR-31 key block wrapped under the key
    /// `request` names.
    ///
    /// "In TR-31, you must use an initial key such as KEK to encrypt or wrap the key
    /// under export" (the operation's own documentation), so the wrapping key is a key
    /// encryption key this store holds and the block is built from both records'
    /// material.
    pub fn export_tr31_key_block(
        &mut self,
        export: &KeyExport<'_>,
        request: &ExportTr31KeyBlock,
        now: SystemTime,
    ) -> Result<WrappedKeyMaterial, ExportRejection> {
        // Drawn before the records are read, so the two borrows below are both
        // immutable: the payload's padding is the only thing this call takes from the
        // store's generator, and no key record is touched.
        let padding = self.fresh_padding(MAX_PAYLOAD_PADDING);
        let key = self.exportable_key(export.export_key_identifier, now)?;
        let wrapping = self.wrapping_key(&request.wrapping_key_identifier, now)?;
        let payload = export_payload(key, export.dukpt_initial_key)?;
        if !is_symmetric_algorithm(&payload.key_algorithm) {
            return Err(ExportRejection::Invalid(format!(
                "a TR-31 key block carries a symmetric key, and the key under export is a {} key",
                payload.key_algorithm
            )));
        }

        let headers = request.key_block_headers.as_ref();
        let modes_of_use = headers
            .and_then(|headers| headers.key_modes_of_use.as_ref())
            .unwrap_or(&payload.modes_of_use);
        let exportability =
            exportability_code(headers.and_then(|headers| headers.key_exportability.as_deref()))?;
        let key_version = headers
            .and_then(|headers| headers.key_version.as_deref())
            .unwrap_or(KEY_VERSION_UNUSED);
        let optional_blocks: BTreeMap<String, String> = headers
            .and_then(|headers| headers.optional_blocks.as_ref())
            .map(|blocks| {
                blocks
                    .iter()
                    .map(|(id, value)| (id.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let block = key_block::build(&KeyBlock {
            payload: &payload.material,
            payload_algorithm: &payload.key_algorithm,
            usage_code: &payload.usage_code,
            modes_of_use,
            exportability,
            key_version,
            optional_blocks: &optional_blocks,
            protection_key: wrapping.material.bytes(),
            protection_key_algorithm: &wrapping.key_attributes.key_algorithm,
            padding: &padding,
        })
        .map_err(ExportRejection::Invalid)?;

        let (key_check_value, key_check_value_algorithm) =
            check_value(&payload, export.key_check_value_algorithm)?;
        Ok(WrappedKeyMaterial {
            wrapping_key_arn: wrapping.key_arn.clone(),
            wrapped_key_material_format: WRAPPED_KEY_FORMAT_TR31,
            key_material: block,
            key_check_value,
            key_check_value_algorithm,
        })
    }

    /// Export the key `export` names as an AS 2805 key cryptogram under the key
    /// `request` names.
    ///
    /// The cryptogram is the key under export encrypted under a VARIANT of the wrapping
    /// key: the key variant the standard forms by the constant the modelled
    /// `As2805KeyVariant` value ends with, applied to the leading byte of each
    /// component of the key encryption key.
    pub fn export_as2805_key_cryptogram(
        &mut self,
        export: &KeyExport<'_>,
        request: &ExportAs2805KeyCryptogram,
        now: SystemTime,
    ) -> Result<WrappedKeyMaterial, ExportRejection> {
        let key = self.exportable_key(export.export_key_identifier, now)?;
        let wrapping = self.wrapping_key(&request.wrapping_key_identifier, now)?;
        let payload = export_payload(key, export.dukpt_initial_key)?;
        if !is_symmetric_algorithm(&payload.key_algorithm) {
            return Err(ExportRejection::Invalid(format!(
                "an AS 2805 key cryptogram carries a symmetric key, and the key under export is \
                 a {} key",
                payload.key_algorithm
            )));
        }
        let variant = as2805_variant(&request.as2805_key_variant).ok_or_else(|| {
            ExportRejection::Invalid(format!(
                "As2805KeyVariant {} names no key variant this service forms",
                request.as2805_key_variant
            ))
        })?;
        let wrapping_algorithm = &wrapping.key_attributes.key_algorithm;
        if !matches!(wrapping_algorithm.as_str(), "TDES_2KEY" | "TDES_3KEY") {
            return Err(ExportRejection::Invalid(format!(
                "an AS 2805 key cryptogram is encrypted under a TDES key encryption key, and the \
                 wrapping key named is a {wrapping_algorithm} key"
            )));
        }

        let mut varianted = wrapping.material.bytes().to_vec();
        for component in varianted.chunks_mut(TDES_BLOCK) {
            if let Some(leading) = component.first_mut() {
                *leading ^= variant;
            }
        }
        let cryptogram =
            tdes_encrypt_blocks(&varianted, &payload.material).ok_or_else(|| {
                ExportRejection::Invalid(format!(
                    "a {} key is not a whole number of TDES blocks, so it has no AS 2805 key \
                     cryptogram",
                    payload.key_algorithm
                ))
            })?;

        let (key_check_value, key_check_value_algorithm) =
            check_value(&payload, export.key_check_value_algorithm)?;
        Ok(WrappedKeyMaterial {
            wrapping_key_arn: wrapping.key_arn.clone(),
            wrapped_key_material_format: WRAPPED_KEY_FORMAT_CRYPTOGRAM,
            key_material: hex::encode_upper(&cryptogram),
            key_check_value,
            key_check_value_algorithm,
        })
    }

    /// The precondition every export under an ASYMMETRIC key exchange mechanism — TR-34,
    /// RSA wrap and unwrap, and ECDH — has to meet: the key under export is one this
    /// store holds and may leave the service, and `certificate_authority_identifier`
    /// names the imported certificate authority public key the wrapping certificate is
    /// signed by.
    ///
    /// "The receiving HSM … public key certificate … signed by the root certificate
    /// (`CertificateAuthorityPublicKeyIdentifier`) imported into Amazon Web Services
    /// Payment Cryptography" (the operation's own documentation): the identifier names a
    /// PUBLIC KEY imported for signature verification, so an identifier naming no key, or
    /// naming a key of any other class or usage, is refused before anything is wrapped.
    pub fn check_asymmetric_export(
        &self,
        export: &KeyExport<'_>,
        certificate_authority_identifier: &str,
        now: SystemTime,
    ) -> Result<(), ExportRejection> {
        self.exportable_key(export.export_key_identifier, now)?;
        let authority = self
            .live_key(certificate_authority_identifier, now)
            .ok_or_else(|| {
                ExportRejection::NotFound(certificate_authority_identifier.to_string())
            })?;
        let attributes = &authority.key_attributes;
        if attributes.key_class != KEY_CLASS_PUBLIC_KEY
            || attributes.key_usage != KEY_USAGE_DIGITAL_SIGNATURE
        {
            return Err(ExportRejection::Invalid(format!(
                "CertificateAuthorityPublicKeyIdentifier names the key {}, which is a {} for {}, \
                 and a certificate authority public key is a {KEY_CLASS_PUBLIC_KEY} for \
                 {KEY_USAGE_DIGITAL_SIGNATURE}",
                authority.key_arn, attributes.key_class, attributes.key_usage
            )));
        }
        Ok(())
    }

    /// The key `identifier` names, when this store still holds it at `now`: a key the
    /// clock has finished deleting is gone, so it resolves to nothing.
    ///
    /// The lookup every operation that USES a key goes through, rather than only the
    /// export this module serves: a key past its deletion window answers the modelled
    /// `ResourceNotFoundException` wherever it is named.
    pub fn live_key(&self, identifier: &str, now: SystemTime) -> Option<&StoredKey> {
        self.key(identifier)
            .filter(|key| key.effective_state(now) != KEY_STATE_DELETE_COMPLETE)
    }

    /// The key `identifier` names, refusing one this store does not hold and one the
    /// caller may not take out of the service.
    ///
    /// `Exportable` "specifies whether the key is exportable from the service" and is
    /// immutable after the key is created, so a key created without it can never be
    /// exported by any mechanism.
    fn exportable_key(
        &self,
        identifier: &str,
        now: SystemTime,
    ) -> Result<&StoredKey, ExportRejection> {
        let key = self
            .live_key(identifier, now)
            .ok_or_else(|| ExportRejection::NotFound(identifier.to_string()))?;
        if !key.exportable {
            return Err(ExportRejection::Invalid(format!(
                "Export is not enabled for the key {}.",
                key.key_arn
            )));
        }
        Ok(key)
    }

    /// The wrapping key `identifier` names, refusing one that cannot wrap a key for
    /// export.
    ///
    /// A key encryption key is what wraps an exported key, and it has to be able to:
    /// its `KeyUsage` says it is one ([`WRAPPING_KEY_USAGES`]), its `KeyAlgorithm` is a
    /// symmetric cipher a key is encrypted under, its `KeyModesOfUse` permit wrapping,
    /// and it is still enabled — `UsageStopTimestamp` is "the date and time after which
    /// Amazon Web Services Payment Cryptography will stop using the key material for
    /// cryptographic operations", and wrapping a key is one.
    fn wrapping_key(
        &self,
        identifier: &str,
        now: SystemTime,
    ) -> Result<&StoredKey, ExportRejection> {
        let key = self
            .live_key(identifier, now)
            .ok_or_else(|| ExportRejection::NotFound(identifier.to_string()))?;
        let attributes = &key.key_attributes;
        if !WRAPPING_KEY_USAGES.contains(&attributes.key_usage.as_str()) {
            return Err(ExportRejection::Invalid(format!(
                "the wrapping key {} is a {} key, and a key is wrapped for export under a key \
                 whose KeyUsage is one of {}",
                key.key_arn,
                attributes.key_usage,
                WRAPPING_KEY_USAGES.join(", ")
            )));
        }
        if !WRAPPING_KEY_ALGORITHMS.contains(&attributes.key_algorithm.as_str()) {
            return Err(ExportRejection::Invalid(format!(
                "the wrapping key {} is a {} key, and a key is wrapped for export under one of \
                 {}",
                key.key_arn,
                attributes.key_algorithm,
                WRAPPING_KEY_ALGORITHMS.join(", ")
            )));
        }
        let modes = &attributes.key_modes_of_use;
        if !(modes.wrap || modes.no_restrictions) {
            return Err(ExportRejection::Invalid(format!(
                "the wrapping key {} has no Wrap in its KeyModesOfUse, so it cannot wrap a key \
                 for export",
                key.key_arn
            )));
        }
        if !key.enabled {
            return Err(ExportRejection::Invalid(format!(
                "the wrapping key {} is not enabled, so its key material is not used for \
                 cryptographic operations",
                key.key_arn
            )));
        }
        Ok(key)
    }

    /// `len` bytes of fresh padding from this store's generator: what masks the length
    /// of the key inside a key block payload, and the one thing an export draws.
    fn fresh_padding(&mut self, len: usize) -> Vec<u8> {
        self.fresh_material(len).0
    }
}

/// The material an export wraps: the initial DUKPT key the request asked for, or the
/// key under export's own material.
fn export_payload(
    key: &StoredKey,
    dukpt_initial_key: Option<&ExportDukptInitialKey>,
) -> Result<ExportPayload, ExportRejection> {
    let attributes = &key.key_attributes;
    let Some(request) = dukpt_initial_key else {
        let usage_code = tr31_key_usage(&attributes.key_usage).ok_or_else(|| {
            ExportRejection::Invalid(format!(
                "a key block carries no key usage for a {} key",
                attributes.key_usage
            ))
        })?;
        return Ok(ExportPayload {
            material: key.material.bytes().to_vec(),
            key_algorithm: attributes.key_algorithm.clone(),
            usage_code: usage_code.to_string(),
            modes_of_use: attributes.key_modes_of_use.clone(),
        });
    };

    // "IPEK is generated from BDK (Base Derivation Key) and ExportDukptInitialKey
    // attribute KSN" (the operation's own documentation), so the key under export has
    // to be a base derivation key for there to be an initial key to derive.
    if attributes.key_usage != KEY_USAGE_BASE_DERIVATION_KEY {
        return Err(ExportRejection::Invalid(format!(
            "ExportAttributes.ExportDukptInitialKey asks for an initial DUKPT key, which is \
             derived from a {KEY_USAGE_BASE_DERIVATION_KEY}, and the key under export is a {} key",
            attributes.key_usage
        )));
    }
    let initial = dukpt::initial_key(
        &attributes.key_algorithm,
        key.material.bytes(),
        &request.key_serial_number,
    )
    .map_err(ExportRejection::Invalid)?;
    Ok(ExportPayload {
        material: initial.material,
        key_algorithm: initial.key_algorithm.to_string(),
        usage_code: TR31_USAGE_INITIAL_DUKPT_KEY.to_string(),
        modes_of_use: attributes.key_modes_of_use.clone(),
    })
}

/// The check value of the material an export wraps, and the algorithm it was computed
/// by: the one the request named, or the default for a key of that algorithm.
///
/// Computed from the very bytes the wrapped material carries, so the receiver can check
/// what it unwrapped against it.
fn check_value(
    payload: &ExportPayload,
    requested: Option<&str>,
) -> Result<(String, String), ExportRejection> {
    let algorithm = match requested {
        Some(named) => named.to_string(),
        None => default_kcv_algorithm(&payload.key_algorithm)
            .ok_or_else(|| {
                ExportRejection::Invalid(format!(
                    "a {} key has no default KeyCheckValueAlgorithm in this service",
                    payload.key_algorithm
                ))
            })?
            .to_string(),
    };
    let value = key_check_value(&algorithm, &payload.key_algorithm, &payload.material)
        .ok_or_else(|| {
            ExportRejection::Invalid(format!(
                "ExportAttributes.KeyCheckValueAlgorithm {algorithm} has no definition for a {} \
                 key; use {} instead",
                payload.key_algorithm,
                default_kcv_algorithm(&payload.key_algorithm).unwrap_or("SHA_1")
            ))
        })?;
    Ok((value, algorithm))
}

/// The X9.143 key usage of a key whose modelled `KeyUsage` is `key_usage`.
///
/// The modelled values ARE those usages — "as defined in section A.5.2 of the TR-31
/// spec" (`KeyUsage`'s own documentation) — and each row below is the two characters the
/// corresponding value names. The values are matched exactly, never by the shape of the
/// name, so a usage the model gains later answers `None` and the export is refused
/// instead of being rendered under a usage guessed from its spelling. Source:
/// `smithy_query(query="shape",shape="KeyUsage")`.
fn tr31_key_usage(key_usage: &str) -> Option<&'static str> {
    let code = match key_usage {
        "TR31_B0_BASE_DERIVATION_KEY" => "B0",
        "TR31_C0_CARD_VERIFICATION_KEY" => "C0",
        "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY" => "D0",
        "TR31_D1_ASYMMETRIC_KEY_FOR_DATA_ENCRYPTION" => "D1",
        "TR31_E0_EMV_MKEY_APP_CRYPTOGRAMS" => "E0",
        "TR31_E1_EMV_MKEY_CONFIDENTIALITY" => "E1",
        "TR31_E2_EMV_MKEY_INTEGRITY" => "E2",
        "TR31_E4_EMV_MKEY_DYNAMIC_NUMBERS" => "E4",
        "TR31_E5_EMV_MKEY_CARD_PERSONALIZATION" => "E5",
        "TR31_E6_EMV_MKEY_OTHER" => "E6",
        "TR31_K0_KEY_ENCRYPTION_KEY" => "K0",
        "TR31_K1_KEY_BLOCK_PROTECTION_KEY" => "K1",
        "TR31_K2_TR34_ASYMMETRIC_KEY" => "K2",
        "TR31_K3_ASYMMETRIC_KEY_FOR_KEY_AGREEMENT" => "K3",
        "TR31_M0_ISO_16609_MAC_KEY" => "M0",
        "TR31_M1_ISO_9797_1_MAC_KEY" => "M1",
        "TR31_M3_ISO_9797_3_MAC_KEY" => "M3",
        "TR31_M6_ISO_9797_5_CMAC_KEY" => "M6",
        "TR31_M7_HMAC_KEY" => "M7",
        "TR31_P0_PIN_ENCRYPTION_KEY" => "P0",
        "TR31_P1_PIN_GENERATION_KEY" => "P1",
        "TR31_S0_ASYMMETRIC_KEY_FOR_DIGITAL_SIGNATURE" => "S0",
        "TR31_V1_IBM3624_PIN_VERIFICATION_KEY" => "V1",
        "TR31_V2_VISA_PIN_VERIFICATION_KEY" => "V2",
        _ => return None,
    };
    Some(code)
}

/// The key block exportability character `KeyBlockHeaders.KeyExportability` asks for.
///
/// `KeyExportability` "can be used to further restrict exportability of the key after
/// export" (the operation's own documentation); the three modelled values
/// (`smithy_query(query="shape",shape="KeyExportability")`) are the standard's three
/// characters. A request that names none exports a key this service has already
/// required to be exportable, so the block says so.
fn exportability_code(requested: Option<&str>) -> Result<char, ExportRejection> {
    let code = match requested {
        None | Some("EXPORTABLE") => 'E',
        Some("NON_EXPORTABLE") => 'N',
        Some("SENSITIVE") => 'S',
        Some(other) => {
            return Err(ExportRejection::Invalid(format!(
                "KeyBlockHeaders.KeyExportability {other} is not an exportability a key block \
                 header carries"
            )))
        }
    };
    Ok(code)
}

/// The key variant constant a modelled `As2805KeyVariant` names.
///
/// Each modelled value ends in the two hexadecimal digits of the variant constant the
/// standard forms that key's variant with
/// (`smithy_query(query="shape",shape="As2805KeyVariant")`), and the rows below are
/// those constants. Matched exactly rather than parsed out of the name, so a variant the
/// model gains later is refused instead of being formed from a guess.
fn as2805_variant(variant: &str) -> Option<u8> {
    let constant = match variant {
        "TERMINAL_MAJOR_KEY_VARIANT_00" => 0x00,
        "PIN_ENCRYPTION_KEY_VARIANT_28" => 0x28,
        "MESSAGE_AUTHENTICATION_KEY_VARIANT_24" => 0x24,
        "DATA_ENCRYPTION_KEY_VARIANT_22" => 0x22,
        _ => return None,
    };
    Some(constant)
}
