// SPDX-License-Identifier: CC0-1.0

//! A `SecureString` parameter's value at rest: the KMS ciphertext it is stored as, and
//! which reads may have that ciphertext opened for their caller.
//!
//! A `SecureString`'s value is sealed under the KMS key the put named
//! ([`super::DEFAULT_SECURE_STRING_KEY_ID`] when it named none), and Parameter Store does
//! not hold that key: it asks KMS to seal the value on the way in and to open it on the
//! way out, as the CALLER and through its own service path. The ciphertext is bound to
//! the one parameter it belongs to by the encryption context [`PARAMETER_ARN_CONTEXT_KEY`]
//! naming that parameter's ARN.
//!
//! What this store therefore holds for such a version is the CIPHERTEXT, base64-encoded
//! ([`seal`]) — the plaintext is not written anywhere. Three things follow, and they are
//! the whole of this module:
//!
//! * a read that asks for no decryption reports the stored ciphertext verbatim, so a
//!   caller entitled to list or copy parameters without being entitled to their contents
//!   receives bytes only KMS can open;
//! * a read that asks for decryption has the value opened under that key ([`open`]), so a
//!   caller the key does not permit `kms:Decrypt` is refused rather than answered with
//!   the value;
//! * the decision is made under that encryption context, so a key policy statement scoped
//!   to one parameter's entry admits the read of THAT parameter and leaves the reads of
//!   every other parameter sealed under the same key to the policy's other statements —
//!   which is how one key serves many parameters while each parameter's value stays
//!   readable only by the principals its own statement names.
//!
//! Both the sealing and the authorization decision are the KMS service's OWN
//! ([`kms_store::seal`] / [`kms_store::unseal`], [`kms_store::authorization`]), made
//! against the key material, key policy, caller identity policies and grants that KMS's
//! handlers wrote, so this service and KMS cannot disagree about a key, the ciphertext a
//! read reports is one `kms:Decrypt` opens under the same encryption context, and the
//! refusal a caller sees is the sentence KMS itself words for it.
//!
//! A read that asks for no decryption opens nothing, and neither does a type that is not
//! encrypted at all: "This flag is ignored for String and StringList parameter types"
//! (`smithy_query(query="doc",shape="GetParameterRequest",member="WithDecryption")`).
//!
//! A key's own KEY STATE decides every one of these calls before its policy is read: KMS
//! runs no cryptographic operation under a key that is not usable right now, so a value
//! can neither be sealed nor opened under a key that is disabled or awaiting deletion or
//! key material ([`key_state_refusal`]). Parameter Store words none of that itself — it
//! RELAYS what its KMS call failed with ([`relayed`]), KMS's own sentence followed by the
//! metadata block an AWS error carries — and reports it as the `InvalidKeyId` both the
//! read and the write model for a key that cannot serve the request.
//!
//! An Amazon Web Services managed key — the one [`super::DEFAULT_SECURE_STRING_KEY_ID`]
//! names — carries a policy AWS manages for the service that owns it, so a caller needs
//! no grant of its own on it; its value is still sealed and opened under it like any
//! other.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

/// The encryption-context entry a `SecureString`'s ciphertext is bound to: the parameter's
/// own Amazon Resource Name, under this entry name.
pub const PARAMETER_ARN_CONTEXT_KEY: &str = "PARAMETER_ARN";

/// The KMS operation a read that reports a decrypted `SecureString` performs on the key.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The service name KMS's own error metadata carries, which is the service a relayed KMS
/// refusal names.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status a KMS client error is answered with, which the relayed metadata block
/// states.
const KMS_REFUSAL_STATUS: u16 = 400;

/// The error code KMS refuses a cryptographic operation on a DISABLED key under.
const DISABLED_EXCEPTION: &str = "DisabledException";

/// The error code KMS refuses a cryptographic operation under for every other key state
/// it cannot run in — a key awaiting deletion or awaiting imported key material.
const INVALID_STATE_EXCEPTION: &str = "KMSInvalidStateException";

/// A refusal KMS answered, as the caller of Parameter Store sees it: this service words
/// none of these refusals itself, it RELAYS what its KMS call failed with, so the message
/// is KMS's own sentence followed by the metadata block an AWS error carries — the service
/// that refused, the status its class is answered with, the error code it was stated
/// under, and the id of the request it answered.
///
/// The request id is the only per-call field: KMS mints one per call, so it is drawn from
/// this emulator's one request-id mint rather than taken from anything the caller sent.
fn relayed(message: &str, code: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_REFUSAL_STATUS}; \
         Error Code: {code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// Why `key` cannot run the cryptographic operation this call needs of it right now, as
/// the refusal to relay; `None` when the key's state lets it run.
///
/// A KMS key serves a cryptographic operation only in a key state that permits one
/// ([`kms_store::Cmk::is_usable`]), and which refusal says so is the key state itself: a
/// key that is merely disabled is KMS's own `DisabledException`, and every other state
/// outside a usable one is its `KMSInvalidStateException`. Both carry the sentence KMS
/// words for the state ([`kms_store::invalid_state_message`]), so a key suspended as a
/// kill switch refuses every value sealed under it in KMS's own words rather than in a
/// second copy of them here.
fn key_state_refusal(key: &kms_store::Cmk) -> Option<String> {
    if key.is_usable() {
        return None;
    }
    let code = if key.reported_key_state() == kms_store::KEY_STATE_DISABLED {
        DISABLED_EXCEPTION
    } else {
        INVALID_STATE_EXCEPTION
    };
    Some(relayed(&kms_store::invalid_state_message(key), code))
}

/// One stored value a read was asked to report decrypted: the ciphertext as the store
/// holds it, the parameter it is bound to, and the key it is sealed under.
#[derive(Debug, Clone)]
pub struct SealedValue {
    /// The parameter's ARN, which is the value of [`PARAMETER_ARN_CONTEXT_KEY`].
    pub parameter_arn: String,
    /// The `KeyId` the version was stored under, in any form KMS accepts.
    pub key_id: String,
    /// The version's stored value: the base64 ciphertext [`seal`] produced.
    pub stored: String,
}

/// The value reporting `version` of `record` has to open, or `None` when it has none: a
/// read that did not ask for decryption, and a version of a type that is not encrypted,
/// each report the stored value without opening anything.
pub fn value_to_open(
    with_decryption: Option<bool>,
    record: &super::ParameterRecord,
    version: &super::ParameterVersionRecord,
) -> Option<SealedValue> {
    if !with_decryption.unwrap_or(false) || version.type_ != super::TYPE_SECURE_STRING {
        return None;
    }
    Some(SealedValue {
        parameter_arn: record.arn(),
        key_id: version
            .key_id
            .clone()
            .unwrap_or_else(|| super::DEFAULT_SECURE_STRING_KEY_ID.to_string()),
        stored: version.value.clone(),
    })
}

/// Why a put cannot store a `SecureString`, named by the modelled error that reports it.
#[derive(Debug)]
pub enum SealRefusal {
    /// The `KeyId` names no key this endpoint holds, so there is no key to encrypt the
    /// value under: `InvalidKeyId`. Carries KMS's own sentence for the identifier.
    UnknownKey(String),
    /// The key is there and its key state runs no cryptographic operation, so nothing can
    /// seal the value under it: `InvalidKeyId` too, the one refusal both the read and the
    /// write model for a key that cannot serve the request. Carries the refusal KMS
    /// answered, relayed ([`key_state_refusal`]).
    UnusableKey(String),
    /// The key is there and the value still could not be sealed under it, which nothing
    /// about the request explains: a server fault.
    Unavailable(String),
}

/// Seal `plaintext` under the key `key_id` names, bound to `parameter_arn`, and report it
/// in the form the store holds and a read reports: the ciphertext, base64-encoded.
///
/// Reaches no state of THIS service, so a handler calls it with its own store guard
/// released.
///
/// `region` is the Region the request was signed for, which is where the key is resolved:
/// a parameter and the key it is sealed under live at the same endpoint.
pub fn seal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key_id: &str,
    parameter_arn: &str,
    plaintext: &str,
) -> Result<String, SealRefusal> {
    let Some(kms) = kms_store::service_state(engine) else {
        return Err(SealRefusal::Unavailable(
            "no KMS service is mounted beside Systems Manager, so a SecureString cannot be \
             encrypted"
                .to_string(),
        ));
    };
    let mut store = kms.lock();
    // A predefined Amazon Web Services alias "has no key ID" until the first call that
    // asks for one, which is what a put naming no `KeyId` — and so falling back to the
    // Amazon Web Services managed key of this service — does.
    if !matches!(store.resolve(region, key_id), kms_store::KeyRef::Found(_)) {
        if let Some(alias_name) = kms_store::alias_named_by(region, key_id) {
            store.associate_predefined_aws_alias(region, &alias_name);
        }
    }
    let kms_store::KeyRef::Found(key) = store.resolve(region, key_id) else {
        return Err(SealRefusal::UnknownKey(kms_store::key_not_found_message(
            region, key_id,
        )));
    };
    // The key's own key state decides the call before its material is reached: a key KMS
    // would run no `GenerateDataKey`/`Encrypt` under seals nothing here either, so the put
    // is refused rather than storing a value the key cannot open again.
    if let Some(refusal) = key_state_refusal(key) {
        return Err(SealRefusal::UnusableKey(refusal));
    }
    let blob = kms_store::seal(key, plaintext.as_bytes(), Some(&context(parameter_arn))).ok_or_else(
        || {
            SealRefusal::Unavailable(format!(
                "the value could not be encrypted under {}",
                key.arn
            ))
        },
    )?;
    Ok(::aws_smithy_types::base64::encode(&blob))
}

/// Why a read cannot report a value decrypted, named by the modelled error that reports
/// it.
#[derive(Debug)]
pub enum OpenRefusal {
    /// The parameter's key does not permit this caller the decrypt:
    /// `AccessDeniedException`. Carries the sentence KMS words for the refusal.
    Denied(String),
    /// The value cannot be opened under the key the version names at all — the identifier
    /// resolves to no key at this endpoint any more, the key's key state runs no
    /// cryptographic operation ([`key_state_refusal`]), or the stored ciphertext is not one
    /// that key sealed for this parameter: `InvalidKeyId`.
    Key(String),
}

/// Open every value in `values`, under the key each one is sealed under and as this
/// request's caller, and report the plaintexts by the position each value sits at in the
/// answer the read is building.
///
/// Every value a read reports has to be opened before the answer is served, so the first
/// refusal is the whole read's.
///
/// `region` is the Region the request was signed for, which is where the keys the
/// parameters name are resolved: a parameter and the key it is sealed under live at the
/// same endpoint.
///
/// Reaches no state of THIS service, so a handler calls it with its own store guard
/// released — the stores this consults (KMS's, and through it STS's and IAM's) are never
/// read while this service's guard is held.
pub fn open<Eng>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    values: &[(usize, SealedValue)],
) -> Result<Vec<(usize, String)>, OpenRefusal>
where
    Eng: Send + Sync + 'static,
{
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let Some(kms) = kms_store::service_state(engine) else {
        return Err(OpenRefusal::Key(
            "no KMS service is mounted beside Systems Manager, so a SecureString cannot be \
             decrypted"
                .to_string(),
        ));
    };
    // The caller is resolved before the KMS store is locked, because resolving it reads
    // the STS and IAM stores.
    let caller = kms_store::authorization::caller(engine, ctx);
    let store = kms.lock();
    let mut opened = Vec::with_capacity(values.len());
    for (at, value) in values {
        let key = match store.resolve(region, &value.key_id) {
            kms_store::KeyRef::Found(key) => key,
            // An identifier whose account segment is not an account id is refused for
            // want of permission before any key is looked up, so the value cannot be
            // opened under it at all.
            kms_store::KeyRef::UnauthorizedAccount => {
                return Err(OpenRefusal::Denied(
                    kms_store::unauthorized_account_message(DECRYPT_OPERATION),
                ))
            }
            // The key the value was sealed under is no longer there, so nothing can open
            // the ciphertext this read would have to report.
            kms_store::KeyRef::NotFound | kms_store::KeyRef::MalformedArn => {
                return Err(OpenRefusal::Key(kms_store::key_not_found_message(
                    region,
                    &value.key_id,
                )))
            }
        };
        // The key's own key state decides the read before its policy is read and before
        // its material is reached, exactly as a direct `kms:Decrypt` is decided: a key KMS
        // runs no cryptographic operation under opens nothing here either, so a key
        // suspended as a kill switch refuses every value sealed under it instead of the
        // plaintext being served.
        if let Some(refusal) = key_state_refusal(key) {
            return Err(OpenRefusal::Key(refusal));
        }
        let context = context(&value.parameter_arn);
        // An Amazon Web Services managed key's policy is AWS's own, managed for the
        // service that owns the key, so a caller needs no grant of its own on it.
        if key.key_manager == kms_store::KEY_MANAGER_CUSTOMER {
            kms_store::authorization::authorize_use(
                &caller,
                &store,
                region,
                key,
                kms_store::authorization::RequestedUse::through_service(
                    DECRYPT_OPERATION,
                    &via_service(region),
                ),
                Some(&context),
                None,
            )
            .map_err(OpenRefusal::Denied)?;
        }
        // The ciphertext opens only under the key that sealed it and the encryption
        // context it was bound to, so a value whose key has been rotated past its
        // material, or whose stored bytes are not that key's envelope, has no plaintext
        // to report rather than one being invented.
        let decoded = ::aws_smithy_types::base64::decode(&value.stored).ok();
        let plaintext = decoded
            .as_deref()
            .and_then(kms_store::parse_envelope)
            .and_then(|envelope| kms_store::unseal(key, &envelope, Some(&context)))
            .and_then(|opened| String::from_utf8(opened.plaintext).ok())
            .ok_or_else(|| {
                OpenRefusal::Key(format!(
                    "the value of {} is not a ciphertext {} opens",
                    value.parameter_arn, key.arn
                ))
            })?;
        opened.push((*at, plaintext));
    }
    Ok(opened)
}

/// The encryption context a value of the parameter `parameter_arn` names is bound to.
fn context(parameter_arn: &str) -> HashMap<String, String> {
    HashMap::from([(
        PARAMETER_ARN_CONTEXT_KEY.to_string(),
        parameter_arn.to_string(),
    )])
}

/// The value of the `kms:ViaService` condition key for a cryptographic operation this
/// service performs on a caller's behalf: this service's regional endpoint name, which is
/// what a key policy confining a key to Parameter Store's use of it names.
fn via_service(region: &str) -> String {
    format!("{}.{region}.amazonaws.com", super::ARN_SERVICE)
}
