// SPDX-License-Identifier: CC0-1.0

//! The envelope a secret version's value is stored under, and the KMS calls that
//! seal and open it.
//!
//! A stored value is not held as it arrived. "Secrets Manager uses the Amazon Web
//! Services managed key `aws/secretsmanager`" unless the secret names a key of its
//! own, in which case the value is "encrypted using a customer managed key"
//! (`CreateSecretRequest$KmsKeyId`, this service's Smithy model) — and a value
//! encrypted under a key is a value the service cannot produce, or replace, without
//! that key. So for a secret naming a customer managed key:
//!
//! * a WRITE asks that key for a fresh data key (`kms:GenerateDataKey`), seals the
//!   value under it with AES-256-GCM, and stores the wrapped data key beside the
//!   ciphertext — the plaintext is never stored;
//! * a READ asks that key to unwrap the data key the version was sealed with
//!   (`kms:Decrypt`) and opens the ciphertext with it.
//!
//! That is why the key's own state decides both paths: a key KMS will not perform a
//! cryptographic operation with cannot produce a data key and cannot unwrap one, so
//! the call that needed it fails, relaying the KMS refusal it met. Nothing here
//! inspects the key's state itself: KMS owns that judgement, and this module asks it.
//!
//! WHICH failure the operation answers follows from WHY the key could not be used,
//! which KMS states as the error code of its refusal ([`KmsRefusal::kind`]): an
//! identifier that resolves to no key is a fault of the value the request carried, a
//! key that exists but will not perform the operation is a key the account has to
//! prepare, and authority the caller does not hold is an access decision. Each
//! operation answers those kinds with the error classes it models, so this module
//! states the kind and never the class.
//!
//! A key's PERMISSIONS decide both paths for the same reason. Each call is one this
//! service makes on the CALLER's behalf and through its own service path, so it is
//! authorized as that caller: "you must also have the kms:GenerateDataKey and
//! kms:Decrypt permissions" on the key for a write and a read of a secret under a
//! customer managed key. The decision is KMS's OWN ([`kms_store::authorization`]),
//! taken against the key policy, caller identity policies and grants KMS's and IAM's
//! handlers wrote and under the encryption context the value is bound to, with
//! `kms:ViaService` naming this service's endpoint — so a key policy that grants a
//! role only `kms:Decrypt` lets that role read the secret and refuses its write. Such
//! a refusal is NOT relayed, unlike every other KMS refusal here: the operation is
//! answered `AccessDeniedException` carrying this service's own sentence for it
//! ([`KMS_ACCESS_DENIED_MESSAGE`]), which states nothing about the principal, the
//! action or the key — the access the operation needed was simply not allowed.
//!
//! A secret left on the service-managed key names no key a caller can act on — it
//! cannot be disabled, scheduled for deletion or deleted — so there is no refusal
//! for this module to meet and the value is held as sent.
//!
//! KMS is reached the way the rest of this emulator reaches another service: the AWS
//! SDK driven over the emulator's own in-process transport, through the operation
//! callback ([`smithy_server_runtime::OperationContext::callback`]), so no socket is
//! opened and this crate names no other service's crate. An emulator assembled
//! WITHOUT KMS is the one case a value under a customer managed key is held as sent:
//! the transport itself refuses the call, nothing is said about the key, and a
//! service that refused every write of a value would be unusable in such an
//! emulator. Every other failure leaves the write refused and nothing stored, and a
//! READ of a value that IS sealed has no way to open it and is refused.
//!
//! The AEAD and the nonce come from `ring` (`ring::aead::AES_256_GCM`,
//! `ring::rand::SystemRandom`); the data key itself is KMS's, never this service's.

use std::collections::HashMap;
use std::fmt;

use ::aws_sdk_kms::error::{ProvideErrorMetadata, SdkError};
use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ::ring::digest::{digest, SHA256};
use ::ring::rand::{SecureRandom, SystemRandom};

/// How the `Message` of `DecryptionFailure` introduces the KMS refusal a read met.
///
/// This is the clause the service answers WITH THE RELAY on the wire: the recorded
/// refusal of a read of a secret whose customer managed key is disabled is
/// "Secrets Manager can't decrypt the secret value: &lt;the KMS refusal&gt;". The
/// model's own sentence for the error class ([`DECRYPTION_FAILURE_MESSAGE`]) is this
/// service's DESCRIPTION of `DecryptionFailure` rather than that wire message, so it
/// is what the refusal states when there is no KMS answer for the clause to
/// introduce.
const DECRYPTION_RELAY_PREFIX: &str = "Secrets Manager can't decrypt the secret value:";

/// How the `Message` of `EncryptionFailure` introduces the KMS refusal a write met,
/// which is the mirror of [`DECRYPTION_RELAY_PREFIX`] — the same clause for the
/// operation the key could not perform.
const ENCRYPTION_RELAY_PREFIX: &str = "Secrets Manager can't encrypt the secret value:";

/// The `Message` of `DecryptionFailure` when there is no KMS answer to relay:
/// "Secrets Manager can't decrypt the protected secret text using the provided KMS
/// key." (this service's Smithy model).
pub const DECRYPTION_FAILURE_MESSAGE: &str =
    "Secrets Manager can't decrypt the protected secret text using the provided KMS key.";

/// The `Message` of `EncryptionFailure` when there is no KMS answer to relay:
/// "Secrets Manager can't encrypt the protected secret text using the provided KMS
/// key. Check that the KMS key is available, enabled, and not in an invalid state."
/// (this service's Smithy model).
pub const ENCRYPTION_FAILURE_MESSAGE: &str =
    "Secrets Manager can't encrypt the protected secret text using the provided KMS \
     key. Check that the KMS key is available, enabled, and not in an invalid state.";

/// The `Message` of the `AccessDeniedException` an operation is refused with when the
/// KMS call this service makes on the caller's behalf is refused for want of authority
/// on the secret's key: "Access to KMS is not allowed".
///
/// This sentence is Secrets Manager's OWN, and it is the whole message: it names no
/// principal, no action and no key, and it carries nothing of the answer KMS gave. The
/// cryptographic call is one this service makes, so what KMS told THIS SERVICE about the
/// authority behind it is not told to the caller; all the caller is told is that the
/// access to KMS the operation needed was not allowed. It is the same sentence whichever
/// cryptographic action the key withheld, so a read and a write of a secret under a key
/// the caller cannot use are refused alike.
pub const KMS_ACCESS_DENIED_MESSAGE: &str = "Access to KMS is not allowed";

/// The service name the relayed KMS refusal names itself by, which is KMS's own in
/// the wrapped failure Secrets Manager relays.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status a relayed KMS refusal reports for a request KMS rejected, and the one
/// for a fault on its side: KMS's JSON protocol answers a client-side refusal of a
/// cryptographic operation with 400 and a fault of its own with 500, whichever
/// exception shape carries it, so the status the relay states is the one for the
/// refusal's CLASS rather than the status line of one answer.
const KMS_CLIENT_STATUS: u16 = 400;
const KMS_FAULT_STATUS: u16 = 500;

/// The KMS operation a write of a secret value performs on the secret's key: the
/// data key the value is sealed under is one that key generates.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// The KMS operation a read of a sealed secret value performs on the key that sealed
/// it: the wrapped data key is one only that key unwraps.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The error code KMS answers a cryptographic operation no authority in force reaches
/// for, which is the code the relayed clause of such a refusal states.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The error code KMS answers a request whose key identifier resolves to no key with —
/// "The request was rejected because the specified entity or resource could not be
/// found" (`NotFoundException`, the KMS model), which every cryptographic operation of
/// KMS models. It is the one refusal that is about the identifier the request carried
/// rather than about a key, which is why it is the code [`KmsRefusal::kind`] separates
/// first.
const KEY_NOT_FOUND_CODE: &str = "NotFoundException";

/// How the `Message` of `InvalidParameterException` introduces the KMS refusal of a key
/// a request NAMED but that is not one: "The operation failed because of an invalid KMS
/// key: &lt;the KMS refusal&gt;".
///
/// It is this service's own clause, the counterpart of [`ENCRYPTION_RELAY_PREFIX`] for a
/// key that could not be resolved at all: what the caller has to change is the value it
/// sent, not the key.
const INVALID_KMS_KEY_PREFIX: &str = "The operation failed because of an invalid KMS key:";

/// The `Message` of `InvalidParameterException` when there is no KMS answer to relay:
/// "The parameter name or value is invalid." (this service's Smithy model).
pub const INVALID_PARAMETER_MESSAGE: &str = "The parameter name or value is invalid.";

/// The entry of the encryption context naming the secret the value belongs to.
const CONTEXT_SECRET_ARN: &str = "SecretARN";

/// The entry of the encryption context naming the version the value belongs to.
const CONTEXT_SECRET_VERSION_ID: &str = "SecretVersionId";

/// Which member of the request a version's value arrived in, and therefore which
/// member of the response reports it: "The decrypted secret value, if the secret
/// value was originally provided as binary data … this field is omitted. The secret
/// value appears in SecretString instead."
/// (`GetSecretValueResponse$SecretBinary`, this service's Smithy model.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// `SecretString`.
    Text,
    /// `SecretBinary`.
    Binary,
}

/// A secret value in the clear: the bytes, and the member they travel in.
///
/// Produced from a request on the way in and from an opened envelope on the way
/// out; never stored, which is what [`StoredValue`] is for.
#[derive(Clone)]
pub struct SecretValue {
    kind: ValueKind,
    bytes: Vec<u8>,
}

/// The value is `@sensitive`, so it is withheld from the formatter: a derived
/// `Debug` would carry the plaintext into every log and trace that formats a
/// request or a stored version.
impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretValue")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl SecretValue {
    /// The value a request carried, or `None` for one that carried neither member —
    /// "You must include SecretBinary or SecretString, but not both", which the
    /// operations refuse before reaching here.
    pub fn from_request(
        secret_string: Option<&String>,
        secret_binary: Option<&Vec<u8>>,
    ) -> Option<Self> {
        match (secret_string, secret_binary) {
            (Some(text), _) => Some(Self {
                kind: ValueKind::Text,
                bytes: text.as_bytes().to_vec(),
            }),
            (None, Some(blob)) => Some(Self {
                kind: ValueKind::Binary,
                bytes: blob.clone(),
            }),
            (None, None) => None,
        }
    }

    /// The value for the response, in the one member it travels in.
    ///
    /// A text value was handed to this service as a `String`, so its bytes are
    /// UTF-8; the AEAD's authentication tag is what guarantees the bytes read back
    /// are the bytes sealed.
    pub fn into_members(self) -> (Option<String>, Option<Vec<u8>>) {
        match self.kind {
            ValueKind::Text => (
                Some(String::from_utf8_lossy(&self.bytes).into_owned()),
                None,
            ),
            ValueKind::Binary => (None, Some(self.bytes)),
        }
    }
}

/// A secret version's value as the store holds it.
#[derive(Clone)]
pub struct StoredValue {
    kind: ValueKind,
    content: Content,
    /// The digest of the value, which is what an `@idempotencyToken` replay is
    /// judged on: a sealed value cannot be compared as bytes, because each seal
    /// draws its own data key and nonce.
    fingerprint: Vec<u8>,
}

/// The value is withheld from the formatter for the reason [`SecretValue`]'s own
/// `Debug` withholds it; what a log may say about a stored value is its member and
/// whether a customer managed key sealed it.
impl fmt::Debug for StoredValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredValue")
            .field("kind", &self.kind)
            .field("sealed", &self.is_sealed())
            .finish_non_exhaustive()
    }
}

/// How the bytes of a stored value are held.
#[derive(Clone)]
enum Content {
    /// Held as sent: the secret is on the Amazon Web Services managed key, which no
    /// caller can act on, so no KMS refusal stands between a read and the value.
    AsSent(Vec<u8>),
    /// Sealed under a data key the secret's customer managed key wrapped.
    Sealed(Sealed),
}

/// A value sealed under a KMS data key: the wrapped key, the nonce and the
/// ciphertext with its authentication tag.
#[derive(Clone)]
struct Sealed {
    /// The key that wrapped the data key, as the write that sealed the value named
    /// it. A version is opened under the key it was SEALED with, so this — not
    /// whatever key the secret names now — is the key a read of this version is
    /// authorized against.
    key_id: String,
    /// The `CiphertextBlob` KMS wrapped the data key as. Only the key that wrapped
    /// it can unwrap it, and the blob names that key, so a version is opened under
    /// the key it was sealed with whatever key the secret names now — which is what
    /// keeps a version the secret's key change did not re-encrypt readable (see
    /// [`crate::ops::update_secret`]).
    wrapped_data_key: Vec<u8>,
    nonce: [u8; NONCE_LEN],
    body: Vec<u8>,
}

impl StoredValue {
    /// The value held as it arrived, for a secret on the service-managed key.
    pub fn as_sent(value: &SecretValue) -> Self {
        Self {
            kind: value.kind,
            fingerprint: fingerprint(value.kind, &value.bytes),
            content: Content::AsSent(value.bytes.clone()),
        }
    }

    /// Whether a customer managed key sealed this value.
    pub fn is_sealed(&self) -> bool {
        matches!(self.content, Content::Sealed(_))
    }

    /// Whether this holds exactly `value` — the test an `@idempotencyToken` replay
    /// turns on: "If you call this operation with a ClientRequestToken that matches
    /// an existing version's VersionId, and you specify the same secret data, the
    /// operation succeeds but does nothing." (`PutSecretValue`, this service's
    /// Smithy model.)
    pub fn holds(&self, value: &SecretValue) -> bool {
        self.kind == value.kind && self.fingerprint == fingerprint(value.kind, &value.bytes)
    }

    /// The NAMES of the fields this value states, read as the key/value JSON document a
    /// secret value is written as ("a JSON text that you want to encrypt",
    /// `CreateSecretRequest$SecretString`, this service's Smithy model) — and nothing of
    /// what any of them holds.
    ///
    /// This is what a service that requires a FIELD of a secret has to know: one that
    /// documents which fields the secret it is pointed at must store asks which fields are
    /// there, so only the names are answered and the value itself never leaves this store.
    ///
    /// A value that is not a JSON object states no fields, which is an answer: an empty
    /// list. `None` means the question cannot be answered here at all — the value is
    /// sealed under a customer managed key, and opening one takes the KMS call a store
    /// read is not ([`open`]) — so a caller reports nothing about the fields rather than
    /// treating a value it cannot read as one that states none.
    pub fn stated_field_names(&self) -> Option<Vec<String>> {
        let Content::AsSent(bytes) = &self.content else {
            return None;
        };
        let Ok(::serde_json::Value::Object(fields)) = ::serde_json::from_slice(bytes) else {
            return Some(Vec::new());
        };
        Some(fields.keys().cloned().collect())
    }
}

/// The encryption context the KMS calls for one secret version carry: the secret and
/// the version the value belongs to, which is what binds a sealed value to the place
/// it is stored — a data key wrapped for one version cannot be unwrapped for
/// another.
#[derive(Debug, Clone)]
pub struct ValueContext {
    secret_arn: String,
    version_id: String,
}

impl ValueContext {
    /// The context for the version `version_id` of the secret `secret_arn`.
    pub fn new(secret_arn: &str, version_id: &str) -> Self {
        Self {
            secret_arn: secret_arn.to_string(),
            version_id: version_id.to_string(),
        }
    }

    /// The `EncryptionContext` of the KMS call.
    fn kms(&self) -> HashMap<String, String> {
        HashMap::from([
            (CONTEXT_SECRET_ARN.to_string(), self.secret_arn.clone()),
            (
                CONTEXT_SECRET_VERSION_ID.to_string(),
                self.version_id.clone(),
            ),
        ])
    }

    /// The same two entries as the additional authenticated data of the local AEAD,
    /// so the ciphertext is bound to its version as well as the data key is.
    fn aad(&self) -> Vec<u8> {
        format!(
            "{CONTEXT_SECRET_ARN}={}&{CONTEXT_SECRET_VERSION_ID}={}",
            self.secret_arn, self.version_id
        )
        .into_bytes()
    }
}

/// Why the key a call needed could not be used, which is what decides WHICH class an
/// operation naming that key is refused under: KMS answers each kind of key failure
/// under its own error code, and the code says whether the caller named the wrong key,
/// has to prepare the key it named, or lacks the authority to use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// No authority in force reaches the cryptographic operation on the key for this
    /// caller ([`ACCESS_DENIED_CODE`]).
    Denied,
    /// The identifier resolves to NO key at the endpoint it was asked of
    /// ([`KEY_NOT_FOUND_CODE`]), so what is wrong is the key the request named rather
    /// than anything about a key.
    NoSuchKey,
    /// A key that EXISTS refused the cryptographic operation — its state, its usage or
    /// its material — so the key has to be brought into a state that performs it.
    Unusable,
    /// KMS stated nothing: the call reached no answer, so nothing is known about the
    /// key.
    Unstated,
}

/// A KMS call the envelope needed did not produce a result, so the value could not
/// be sealed or opened.
#[derive(Debug, Clone)]
pub struct KmsRefusal {
    /// The KMS refusal to relay, when KMS answered with one; `None` when KMS did not
    /// answer at all, which is nothing to report about the key.
    relayed: Option<String>,
    /// The error code KMS stated the refusal under, which is what [`Self::kind`] reads
    /// the kind of key failure off; `None` when KMS did not answer, or answered without
    /// one.
    code: Option<String>,
}

impl KmsRefusal {
    /// A failure KMS stated nothing about.
    fn unrelayed() -> Self {
        Self {
            relayed: None,
            code: None,
        }
    }

    /// Which kind of key failure this refusal is, read off the code KMS stated it
    /// under.
    pub fn kind(&self) -> RefusalKind {
        match self.code.as_deref() {
            Some(ACCESS_DENIED_CODE) => RefusalKind::Denied,
            Some(KEY_NOT_FOUND_CODE) => RefusalKind::NoSuchKey,
            _ if self.relayed.is_some() => RefusalKind::Unusable,
            _ => RefusalKind::Unstated,
        }
    }

    /// The `DecryptionFailure` message for this refusal.
    pub fn decryption_failure_message(&self) -> String {
        self.worded(DECRYPTION_RELAY_PREFIX, DECRYPTION_FAILURE_MESSAGE)
    }

    /// The `EncryptionFailure` message for this refusal.
    pub fn encryption_failure_message(&self) -> String {
        self.worded(ENCRYPTION_RELAY_PREFIX, ENCRYPTION_FAILURE_MESSAGE)
    }

    /// The `InvalidParameterException` message for a refusal of the key a request
    /// NAMED: this service's clause for a `KmsKeyId` that is not a usable key
    /// ([`INVALID_KMS_KEY_PREFIX`]), then the KMS refusal that says what is wrong with
    /// it, so the caller sees which identifier to correct.
    pub fn invalid_key_message(&self) -> String {
        self.worded(INVALID_KMS_KEY_PREFIX, INVALID_PARAMETER_MESSAGE)
    }

    /// The sentence to answer an AUTHORIZATION refusal of the key with, or `None` when
    /// the call failed for a reason that is not one.
    ///
    /// A caller whose authority does not reach the cryptographic operation on the secret's
    /// key is NOT AUTHORIZED, which is a refusal of the ACCESS the operation needed rather
    /// than a failure of the service to encrypt or decrypt: the operation is answered with
    /// the common `AccessDeniedException` carrying this service's own sentence for that
    /// class ([`KMS_ACCESS_DENIED_MESSAGE`]), the same one whichever action the key
    /// withheld and whichever operation needed it.
    pub fn denied_message(&self) -> Option<&'static str> {
        matches!(self.kind(), RefusalKind::Denied).then_some(KMS_ACCESS_DENIED_MESSAGE)
    }

    /// Whether KMS STATED this refusal, so what it says about the key is the key's own
    /// answer rather than this emulator failing to ask.
    pub fn stated_by_kms(&self) -> bool {
        self.relayed.is_some()
    }

    /// An AUTHORIZATION refusal of the key, which this service answers as the common
    /// `AccessDeniedException` rather than as a failure to encrypt or decrypt
    /// ([`Self::denied_message`]).
    ///
    /// `message` is the sentence KMS worded for the action it withheld, kept as the
    /// refusal this call MET — relayed as every other KMS refusal this service wraps is,
    /// so a path that reports what the key could not do (a replica's status, say) reports
    /// it — while the answer to the caller stays this service's own sentence for the
    /// class.
    fn denied(message: &str) -> Self {
        Self {
            code: Some(ACCESS_DENIED_CODE.to_string()),
            relayed: Some(format!(
                "{message} {}",
                relay_clause(ACCESS_DENIED_CODE, KMS_CLIENT_STATUS, None)
            )),
        }
    }

    /// What Secrets Manager could not do, and then the KMS refusal it wraps — so
    /// the caller learns which key state or permission refused the call, and
    /// therefore whether it can change. With no KMS answer to introduce there is
    /// nothing to relay, and the modelled sentence for the failure stands alone.
    fn worded(&self, prefix: &str, unrelayed: &str) -> String {
        match &self.relayed {
            Some(relayed) => format!("{prefix} {relayed}"),
            None => unrelayed.to_string(),
        }
    }
}

/// Seal `value` for the version `context` names, under the key `kms_key_id` names.
///
/// A secret on the service-managed key (`None`, the empty string, or the managed
/// key's own alias) has no customer managed key to call, so its value is held as
/// sent.
pub async fn seal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: Option<&str>,
    context: &ValueContext,
    value: &SecretValue,
) -> Result<StoredValue, KmsRefusal> {
    let Some(key_id) = customer_managed_key(kms_key_id) else {
        return Ok(StoredValue::as_sent(value));
    };
    // The data key is one the caller's own authority on the key has to reach, so the
    // key is asked about THIS caller before anything is sealed under it.
    if let Some(refusal) = use_refusal(
        engine,
        ctx,
        region,
        key_id,
        GENERATE_DATA_KEY_OPERATION,
        context,
    ) {
        return Err(refusal);
    }

    let generated = match kms_client(ctx, region)
        .generate_data_key()
        .key_id(key_id)
        .key_spec(::aws_sdk_kms::types::DataKeySpec::Aes256)
        .set_encryption_context(Some(context.kms()))
        .send()
        .await
    {
        Ok(generated) => generated,
        // An emulator that holds no KMS cannot seal anything, and refusing every
        // write of a value under a customer managed key would leave this service
        // unusable without one — see this module's header. Every other failure
        // means the value was not sealed, so it is not stored: a refusal KMS
        // stated, and a fault that reached no answer, are both the write failing.
        Err(error) => {
            return match call_failure(&error) {
                CallFailure::NoKms => Ok(StoredValue::as_sent(value)),
                CallFailure::Failed(refusal) => Err(refusal),
            };
        }
    };

    let (Some(data_key), Some(wrapped_data_key)) =
        (generated.plaintext(), generated.ciphertext_blob())
    else {
        return Err(KmsRefusal::unrelayed());
    };
    let Some((nonce, body)) = seal_under(data_key.as_ref(), &context.aad(), &value.bytes) else {
        return Err(KmsRefusal::unrelayed());
    };
    Ok(StoredValue {
        kind: value.kind,
        fingerprint: fingerprint(value.kind, &value.bytes),
        content: Content::Sealed(Sealed {
            key_id: key_id.to_string(),
            wrapped_data_key: wrapped_data_key.as_ref().to_vec(),
            nonce,
            body,
        }),
    })
}

/// Open `stored`, the value of the version `context` names.
///
/// A value held as sent is answered as it is; a sealed one is answered only once KMS
/// unwraps the data key it was sealed under. The unwrap names no key: the wrapped
/// data key carries the identity of the key that wrapped it, which is what keeps a
/// version readable after the secret is pointed at another key.
pub async fn open<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    context: &ValueContext,
    stored: &StoredValue,
) -> Result<SecretValue, KmsRefusal> {
    let sealed = match &stored.content {
        Content::AsSent(bytes) => {
            return Ok(SecretValue {
                kind: stored.kind,
                bytes: bytes.clone(),
            });
        }
        Content::Sealed(sealed) => sealed,
    };
    // Unwrapping the data key is the caller's own use of the key that sealed it, so
    // that key is asked about THIS caller before the ciphertext is opened.
    if let Some(refusal) =
        use_refusal(engine, ctx, region, &sealed.key_id, DECRYPT_OPERATION, context)
    {
        return Err(refusal);
    }

    let opened = match kms_client(ctx, region)
        .decrypt()
        .ciphertext_blob(::aws_smithy_types::Blob::new(
            sealed.wrapped_data_key.clone(),
        ))
        .set_encryption_context(Some(context.kms()))
        .send()
        .await
    {
        Ok(opened) => opened,
        // A sealed value cannot be opened without KMS, whichever way the call
        // failed, so every failure refuses this read.
        Err(error) => {
            return Err(match call_failure(&error) {
                CallFailure::NoKms => KmsRefusal::unrelayed(),
                CallFailure::Failed(refusal) => refusal,
            });
        }
    };

    let Some(data_key) = opened.plaintext() else {
        return Err(KmsRefusal::unrelayed());
    };
    let Some(bytes) = open_under(data_key.as_ref(), &context.aad(), sealed) else {
        return Err(KmsRefusal::unrelayed());
    };
    Ok(SecretValue {
        kind: stored.kind,
        bytes,
    })
}

/// What KMS says about the key `kms_key_id` that stops it being used at all, as the
/// `EncryptionFailure` message a write under that key would be refused with;
/// `None` when the key can be used.
///
/// A secret is encrypted under the key it names, so a key KMS will not even describe
/// cannot hold one — which is what a replica whose Region names such a key reports
/// as the reason its creation failed, and it reports it as what the service could
/// not do with that key: the same message, relayed KMS answer and all, that a
/// write refused under it carries. The judgement is KMS's, as everywhere else in
/// this module: the key's own state and the permissions on it are KMS's to state,
/// and this asks it (`kms:DescribeKey`).
///
/// `None`, for the same two cases the sealing path treats as nothing said about the
/// key: the Amazon Web Services managed key, which names no key a caller can act on,
/// and an emulator assembled without KMS, where the request reached no KMS at all.
pub async fn key_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_id: Option<&str>,
) -> Option<String> {
    let key_id = customer_managed_key(kms_key_id)?;
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_id)
        .send()
        .await
    {
        Ok(_) => None,
        Err(error) => match call_failure(&error) {
            CallFailure::NoKms => None,
            // A fault KMS stated nothing about says nothing about the key, so
            // there is nothing to report: the modelled sentence for the failure
            // class stands for a refused write, not for a judgement of the key.
            CallFailure::Failed(refusal) => refusal
                .stated_by_kms()
                .then(|| refusal.encryption_failure_message()),
        },
    }
}

/// The customer managed key a secret's `KmsKeyId` names, or `None` for a secret left
/// on the Amazon Web Services managed key: "If you don't specify this value, then
/// Secrets Manager uses the Amazon Web Services managed key `aws/secretsmanager`"
/// (`CreateSecretRequest$KmsKeyId`, this service's Smithy model), which an empty
/// string names too (`UpdateSecretRequest$KmsKeyId`).
pub fn customer_managed_key(kms_key_id: Option<&str>) -> Option<&str> {
    kms_key_id.filter(|key| !key.is_empty() && !super::names_default_kms_key(key))
}

/// Seal `plaintext` under the data key `data_key`, binding `aad` into the
/// ciphertext, and answer the nonce drawn for it with the ciphertext and its tag.
///
/// `None` only when the AEAD itself refuses the inputs — a data key that is not
/// AES-256 material, or a plaintext larger than AES-GCM can carry, which the
/// modelled 65536-byte bound on a secret value already excludes.
fn seal_under(data_key: &[u8], aad: &[u8], plaintext: &[u8]) -> Option<([u8; NONCE_LEN], Vec<u8>)> {
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, data_key).ok()?);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).ok()?;
    // `seal_in_place_append_tag` leaves `body` holding the ciphertext followed by
    // the authentication tag.
    let mut body = plaintext.to_vec();
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut body,
        )
        .ok()?;
    Some((nonce, body))
}

/// Open `sealed` with the data key `data_key` under `aad`.
///
/// `None` when the tag does not verify: the ciphertext was sealed under another data
/// key or for another version, or has been altered.
fn open_under(data_key: &[u8], aad: &[u8], sealed: &Sealed) -> Option<Vec<u8>> {
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, data_key).ok()?);
    let mut body = sealed.body.clone();
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(sealed.nonce),
            Aad::from(aad),
            &mut body,
        )
        .ok()?;
    Some(plaintext.to_vec())
}

/// The digest an idempotent replay compares a value by: the member it travels in,
/// then its bytes, so the same bytes in `SecretString` and in `SecretBinary` are not
/// one value.
fn fingerprint(kind: ValueKind, bytes: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(bytes.len() + 1);
    input.push(match kind {
        ValueKind::Text => b'S',
        ValueKind::Binary => b'B',
    });
    input.extend_from_slice(bytes);
    digest(&SHA256, &input).as_ref().to_vec()
}

/// The error types that are the in-process TRANSPORT's own refusal rather than an
/// answer from KMS: this emulator holds no KMS at the endpoint that was called, or
/// more than one of its services claims it. Either way the request reached no KMS,
/// so it says nothing about the key — the same reading every service that calls
/// another in process makes of these two codes.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// The response headers a request identifier is read from, so the relay reports the
/// identifier of the answer it wraps whenever that answer carries one.
const REQUEST_ID_HEADERS: [&str; 2] = ["x-amzn-requestid", "x-amz-request-id"];

/// How the relayed clause states a field the wrapped answer did not carry.
const ABSENT_FIELD: &str = "null";

/// Why a KMS call did not produce a result.
enum CallFailure {
    /// The request reached no KMS at all: this emulator was assembled without one,
    /// so the TRANSPORT refused it ([`TRANSPORT_REFUSALS`]) and nothing was said
    /// about the key.
    NoKms,
    /// The call did not do its work: a refusal KMS stated, which is relayed, or a
    /// fault that reached no answer, which has nothing to relay.
    Failed(KmsRefusal),
}

/// Which of the two [`CallFailure`]s an SDK error is.
///
/// A refusal KMS stated is relayed as it was answered: the message KMS stated, the
/// service and error code it stated it under, the status its class is answered with,
/// and the identifier of that answer — a field the answer did not carry is stated as
/// absent rather than invented.
fn call_failure<E: ProvideErrorMetadata + ::std::error::Error + 'static>(
    error: &SdkError<E, HttpResponse>,
) -> CallFailure {
    let SdkError::ServiceError(context) = error else {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "KMS did not answer the envelope call for this secret version"
        );
        return CallFailure::Failed(KmsRefusal::unrelayed());
    };
    let refused = context.err();
    let code = refused.code();
    if code.is_some_and(|code| TRANSPORT_REFUSALS.contains(&code)) {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "no KMS answered the envelope call for this secret version"
        );
        return CallFailure::NoKms;
    }
    let answered = context.raw();
    let status = if answered.status().as_u16() >= KMS_FAULT_STATUS {
        KMS_FAULT_STATUS
    } else {
        KMS_CLIENT_STATUS
    };
    let request_id = REQUEST_ID_HEADERS
        .into_iter()
        .find_map(|header| answered.headers().get(header))
        .map(str::to_string)
        .unwrap_or_default();
    let request_id = (!request_id.is_empty()).then_some(request_id);
    let clause = relay_clause(code.unwrap_or(ABSENT_FIELD), status, request_id.as_deref());
    let wrapped = match refused.message() {
        Some(message) => format!("{message} {clause}"),
        None => clause,
    };
    CallFailure::Failed(KmsRefusal {
        relayed: Some(wrapped),
        // The code KMS stated the refusal under is what the kind of key failure is read
        // off ([`KmsRefusal::kind`]), so it is kept beside the relay rather than
        // collapsed into the relay's text.
        code: code.map(str::to_string),
    })
}

/// The clause a relayed KMS refusal ends in: which service refused, the status its
/// class is answered with, the code it was stated under and the identifier of the
/// answer it belongs to.
///
/// A request identifier belongs to the ANSWER, and the refusal is relayed with the one
/// that answer carried. An answer that carries none — and a refusal taken from the key's
/// own policy, which was not a call of its own — is still an answer to one request, so
/// the relay states an identifier of that form for it rather than leaving the field out
/// of a clause that states it.
fn relay_clause(code: &str, status: u16, request_id: Option<&str>) -> String {
    let minted = request_id.map_or_else(|| ::uuid::Uuid::new_v4().to_string(), str::to_string);
    format!(
        "(Service: {KMS_SERVICE_NAME}; Status Code: {status}; Error Code: {code}; \
         Request ID: {minted}; Proxy: {ABSENT_FIELD})"
    )
}

/// Why the key `key_id` names does not let THIS caller perform `operation` on it under
/// the encryption context `context`, as the refusal to relay; `None` when it does.
///
/// The judgement is KMS's own ([`kms_store::authorization::authorize_use`]), made
/// against the key policy, the caller's identity-based policies and the grants on the
/// key, with `kms:ViaService` naming this service's path to it — so a key confined to
/// this service's use of it admits these calls and refuses a caller's direct ones, and a
/// policy that grants one cryptographic action and not another decides each of a secret's
/// reads and writes on its own.
///
/// `None`, so nothing is refused on evidence that is not there, for: an engine carrying
/// no KMS service, where there is no key to ask and no policy to read; an identifier this
/// endpoint holds no key for, which the call itself then answers for; and a key AWS
/// MANAGES, whose policy AWS manages for the service that owns it, so a caller needs no
/// grant of its own on it.
fn use_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
    operation: &str,
    context: &ValueContext,
) -> Option<KmsRefusal> {
    let kms = kms_store::service_state(engine)?;
    // The caller is resolved before the KMS store is locked, because resolving it reads
    // the STS and IAM stores.
    let caller = kms_store::authorization::caller(engine, ctx);
    let store = kms.lock();
    let key = match store.resolve(region, key_id) {
        kms_store::KeyRef::Found(key) => key,
        // An identifier whose account segment is not an account id is refused for want of
        // permission before any key is looked up, with KMS's own sentence for it.
        kms_store::KeyRef::UnauthorizedAccount => {
            return Some(KmsRefusal::denied(
                &kms_store::unauthorized_account_message(operation),
            ))
        }
        kms_store::KeyRef::NotFound | kms_store::KeyRef::MalformedArn => return None,
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    kms_store::authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        kms_store::authorization::RequestedUse::through_service(operation, &via_service(region)),
        Some(&context.kms()),
        None,
    )
    .err()
    .map(|denial| KmsRefusal::denied(&denial))
}

/// The value of the `kms:ViaService` condition key for a cryptographic operation this
/// service performs on a caller's behalf: this service's regional endpoint name, which is
/// what a key policy confining a key to Secrets Manager's use of it names.
fn via_service(region: &str) -> String {
    format!(
        "{}.{region}.amazonaws.com",
        super::authorization::ACTION_PREFIX
    )
}

/// A KMS client over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the key is used where the secret
/// that names it lives. The transport is in-process, so there is nothing to retry,
/// time out or watch for a stalled stream — and each of those would need a sleep
/// implementation this client is not given. The same goes for caching the identity:
/// the credentials the callback hands over are used directly.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
