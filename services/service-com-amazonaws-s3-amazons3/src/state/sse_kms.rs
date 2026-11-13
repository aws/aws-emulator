// SPDX-License-Identifier: CC0-1.0

//! Whether the KMS key an object was stored under lets THIS caller have its plaintext.
//!
//! An object stored with server-side encryption using a customer-managed KMS key
//! ([`super::KMS_SSE_ALGORITHMS`], the two `ServerSideEncryption` values a KMS key id
//! belongs to) is readable only THROUGH that key — its plaintext comes of opening the
//! object's data key — so every read that serves such an object's BYTES performs
//! `kms:Decrypt` on that key, as the caller and through S3's own service path. "If the
//! object is encrypted with SSE-KMS, you must also have the
//! kms:GenerateDataKey and kms:Decrypt permissions in IAM identity-based policies and KMS
//! key policies for the KMS key" (`HeadObject`'s own documentation of the permissions an
//! SSE-KMS object's read needs). A caller no authority in force reaches that action for is
//! refused — S3 surfaces KMS's denial rather than the body — and the refusal is S3's own
//! `AccessDenied` (HTTP 403) carrying the sentence KMS worded for it.
//!
//! WHO the caller is comes from the credential the request presents, which for a session
//! another service vended (a role session an identity pool hands out, one `sts:AssumeRole`
//! minted) is the principal of that session rather than the account: the decision is made
//! against the role the session is of, so a key policy that grants `kms:Decrypt` to one
//! role admits that role's sessions and leaves every other session to the policy's other
//! statements.
//!
//! One class of read presents no credential of a caller's at all: a read ANOTHER AWS SERVICE
//! performs as its own service principal for one of its resources — CloudFront fetching an
//! origin object for a distribution whose origin access control signed the fetch. AWS decides
//! such a read for that service principal, with `aws:SourceArn` naming the resource it acts
//! for, so a key whose policy grants `cloudfront.amazonaws.com` for one distribution opens an
//! object's bytes for that distribution's fetches and for nothing else. Which calls are in
//! that class is recorded by the calling service, under the access key id of the credential
//! it created for the one call and signed it with, for only as long as that call is in flight
//! ([`::service_common::service_principal_calls`]) — so the record is resolved here through
//! this service's one reading of who a request speaks for
//! ([`crate::service::request_authorization::service_principal_call`]), from the access key id
//! the request PRESENTS, which the emulator's verifier resolves and checks the signature
//! against when request authentication is on, and from the account this service serves; a read
//! presenting any other credential is decided as above.
//!
//! Both halves are the KMS service's OWN ([`kms_store::authorization`]), made against the
//! key policy, caller identity policies and grants that KMS's and IAM's handlers wrote, so
//! this service and KMS cannot disagree about a key and the refusal a caller sees is the
//! sentence KMS itself words for it. This module reaches no state of THIS service, so a
//! handler asks it while still holding its own store guard — the object whose bytes are
//! about to be served is the one the guard is held for, and the stores this reads (KMS's,
//! and through it STS's and IAM's, beside the shared directory of in-flight
//! service-principal calls) belong to services that do not read this one's, so no call here
//! can come back round to the guard a caller holds.
//!
//! The other half of this module is about a WRITE rather than a read: an object S3 is asked
//! to encrypt under KMS is encrypted under a data key KMS mints, so the write performs
//! `kms:GenerateDataKey` on that key and a caller the key's authorities refuse that action to
//! is refused the write, with nothing stored ([`data_key_refusal`] — the same decision as the
//! read's, asked of the write's action); and the key itself is resolved against KMS before
//! anything is stored ([`write_key`]), which settles the ARN the object records and refuses
//! the write when KMS holds no key for the reference, or will not mint a data key under the
//! one it holds, for want of a key rather than of permission.
//!
//! Permission is not the only thing KMS answers about a key. A key's own KEY STATE decides
//! whether it runs cryptographic operations AT ALL, so a read that would serve an SSE-KMS
//! object's bytes while its key is in a state that runs none has no plaintext to serve: S3
//! relays KMS's own failure rather than the body ([`decrypt_key_state_relay`], the read's
//! half of the relay [`write_key`] makes for a write, under the same two exceptions).
//!
//! What is NOT decided here: a read that serves no bytes. "A HEAD request has the same
//! options as a GET operation on an object" (`HeadObject`'s own documentation) except that
//! it answers no body at all, and the metadata it reports is S3's own record of the object
//! rather than anything the key has to open — which is also the recorded behaviour of the
//! real service, where a HEAD of an object keeps reporting its size and its key id while
//! the key itself can no longer open it.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

/// The KMS operation a read that serves an SSE-KMS object's bytes performs on its key.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The KMS operation a WRITE that stores an SSE-KMS object performs on its key: the data
/// key the object's bytes are encrypted under is one KMS mints under that key.
///
/// "You must also have the kms:GenerateDataKey and kms:Decrypt permissions in IAM
/// identity-based policies and KMS key policies for the KMS key" (`HeadObject`'s own
/// documentation of what an SSE-KMS object needs), the first of the two being what the write
/// side of that pair performs.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// This service's endpoint prefix, the first label of its regional endpoint
/// (`smithy_query(query="service_info")` reports `endpointPrefix: "s3"`) — which is what
/// [`via_service`] names.
const ENDPOINT_PREFIX: &str = "s3";

/// The encryption-context entry S3 binds an SSE-KMS object's data key to: the Amazon
/// Resource Name of what is being encrypted, under this entry name.
///
/// Amazon S3 User Guide, *Using server-side encryption with KMS keys*, "Encryption
/// context": S3 "uses the object or bucket Amazon Resource Name (ARN) as the encryption
/// context pair", `aws:s3:arn` naming it — the bucket's ARN for an object using an S3
/// Bucket Key, and the object's own otherwise.
const ARN_CONTEXT_KEY: &str = "aws:s3:arn";

/// One stored object a read is about to serve the bytes of: where it lives, and the
/// encryption configuration the write that stored it settled on.
#[derive(Debug, Clone, Copy)]
pub struct EncryptedObject<'a> {
    /// The bucket holding the object — for a copy, the SOURCE bucket, since the key that
    /// has to open the bytes is the source object's.
    pub bucket: &'a str,
    /// The object's key.
    pub key: &'a str,
    /// What [`super::Object::encryption`] records for it.
    pub encryption: &'a super::ObjectEncryption,
}

/// Why KMS runs NO cryptographic operation on the key `object` is stored under, whoever
/// asks — the key's own key state refuses them all — or `None` when that state runs them.
///
/// The `kms:Decrypt` behind serving an SSE-KMS object's bytes is a cryptographic operation
/// on that object's key, and a key whose `KeyState` is outside the states that run them
/// fails such a call inside KMS before any question of permission: a DISABLED key raises
/// `DisabledException` and a key in any other unusable state `KMSInvalidStateException`,
/// which is the split KMS's own `Decrypt` makes ([`unusable`] words both and
/// [`kms_store::Cmk::is_usable`] decides which states run them). So the read has no
/// plaintext to serve and S3 relays KMS's failure, under the exception's name qualified by
/// the service that raised it and with KMS's own sentence, exactly as a WRITE relays the
/// same failure of its own envelope call ([`write_key`]) — one key state, one relay, for
/// both halves of the envelope. The refusal lifts the moment the key's state runs
/// cryptographic operations again, because nothing about the object changed.
///
/// Asked only where BYTES are served: a read answering metadata alone has nothing for the
/// key to open, so it asks KMS nothing (the module header carries that half).
///
/// Nothing is relayed for an object that is not SSE-KMS, for an engine carrying no KMS
/// service to ask, or for a reference this endpoint resolves no key for — the absence of a
/// key is the object-write path's subject, as it is for [`decrypt_refusal`].
///
/// `region` is the Region the request was signed for, which is where the object's key is
/// resolved: an object and the key it is encrypted under live at the same endpoint.
///
/// Reading a key state changes nothing about the key: this resolves the reference the object
/// already records and materialises no alias, so a read decided here leaves every later
/// caller seeing exactly what it would have seen.
pub fn decrypt_key_state_relay<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    object: EncryptedObject<'_>,
) -> Option<Relay> {
    if !object.encryption.uses_kms() {
        return None;
    }
    let key_id = object.encryption.kms_key_id.as_deref()?;
    let kms = kms_store::service_state(engine)?;
    let store = kms.lock();
    match store.resolve(region, key_id) {
        kms_store::KeyRef::Found(key) if !key.is_usable() => Some(unusable(key)),
        kms_store::KeyRef::Found(_)
        | kms_store::KeyRef::NotFound
        | kms_store::KeyRef::MalformedArn
        | kms_store::KeyRef::UnauthorizedAccount => None,
    }
}

/// Why KMS refuses to open `object` for the principal this request speaks for, or `None`
/// when the read may be served. Served: an object that is not SSE-KMS at all; one under a
/// key AWS MANAGES, whose policy AWS manages for the service that owns it, so a caller
/// needs no grant of its own on it — which is also what an SSE-KMS write naming no key id
/// is stored under, the Amazon Web Services managed key for Amazon S3; and one whose key
/// does permit the caller the decrypt.
///
/// Refused: a caller no authority on the key reaches `kms:Decrypt` for, and an identifier
/// whose account segment is not an account id, which KMS refuses for want of permission
/// BEFORE the resource it names is looked up — so no key is resolved for it and nothing can
/// open the object's bytes for this caller.
///
/// A well-formed identifier this endpoint holds no key for is left to be served: WHICH key
/// a write may name at all, and what a read answers when that key is gone, is the subject of
/// the object-write path rather than of this decision.
///
/// `region` is the Region the request was signed for, which is where the object's key is
/// resolved: an object and the key it is encrypted under live at the same endpoint.
///
/// An engine carrying no KMS service holds no key to ask and no policy to read, so nothing
/// is refused on evidence that is not there.
pub fn decrypt_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
) -> Option<String> {
    key_use_refusal(engine, ctx, region, object, DECRYPT_OPERATION)
}

/// Why KMS refuses to mint the data key `object` would be encrypted under for the principal
/// this request speaks for, or `None` when the write may be stored.
///
/// Every write that stores an object under a customer-managed KMS key is encrypted under a
/// data key KMS mints for it, so the write performs `kms:GenerateDataKey` on that key — as
/// the caller and through this service's own path — before anything is recorded. A caller no
/// authority in force reaches that action for is refused, and nothing is written: the
/// refusal is the same `AccessDenied` (HTTP 403) a read's refusal is, carrying the sentence
/// KMS worded for it.
///
/// `object` is the object as the write WOULD store it: its bucket, its key and the
/// encryption configuration the write settled on — the write's own members, or the bucket's
/// default encryption configuration for a write that named none — so the encryption context
/// the data key is bound to is the one the stored object would be read back under
/// ([`EncryptedObject`] and [`encryption_context`] are shared with the read side for exactly
/// that reason, which is what makes an S3 Bucket Key observable: a key policy scoped to the
/// bucket's ARN admits a bucket-key write and withholds a write of the same bytes with the
/// Bucket Key off, whose data key is bound to the object's own ARN).
///
/// What is NOT refused here is what the read side does not refuse either: an object that is
/// not SSE-KMS, a key AWS manages (whose policy AWS manages for the service that owns it),
/// and a reference this endpoint resolves no key for — the absence of a key, and a key whose
/// state mints none, are [`write_key`]'s relay rather than a want of permission.
pub fn data_key_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
) -> Option<String> {
    key_use_refusal(engine, ctx, region, object, GENERATE_DATA_KEY_OPERATION)
}

/// Why the authorities in force on `object`'s KMS key refuse `operation` to the principal
/// this request speaks for, or `None` when the call may be served.
///
/// ONE decision for both sides of the envelope — the `kms:GenerateDataKey` a write performs
/// and the `kms:Decrypt` a read performs — so a key's policy, its grants and the caller's
/// identity policies cannot be read one way for a write and another for a read, and the two
/// are judged under the same encryption context.
fn key_use_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: EncryptedObject<'_>,
    operation: &str,
) -> Option<String> {
    if !object.encryption.uses_kms() {
        return None;
    }
    let key_id = object.encryption.kms_key_id.as_deref()?;
    let kms = kms_store::service_state(engine)?;
    // The caller is resolved before the KMS store is locked, because resolving it reads
    // the STS and IAM stores.
    let caller = caller(engine, ctx);
    let store = kms.lock();
    let key = match store.resolve(region, key_id) {
        kms_store::KeyRef::Found(key) => key,
        // An identifier whose account segment is not an account id is refused for want of
        // permission before any key is looked up, with KMS's own sentence for it, so the
        // object's bytes cannot be opened under it at all.
        kms_store::KeyRef::UnauthorizedAccount => {
            return Some(kms_store::unauthorized_account_message(operation))
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
        Some(&encryption_context(&object)),
        None,
    )
    .err()
}

/// The wire error code S3 answers a WRITE whose SSE-KMS key reference KMS holds no key for,
/// and the status it carries.
///
/// Such a write is encrypted under a data key S3 asks KMS for, so a reference that resolves
/// to no key fails inside KMS rather than in S3: S3 relays KMS's own failure, under the KMS
/// exception's name qualified by the service that raised it, and refuses the write. Recorded
/// behaviour — the s3 model binds no error shape for the relay, so neither the code nor the
/// status can come from it; the message is KMS's own ([`write_key`]).
pub const KMS_NOT_FOUND_CODE: &str = "KMS.NotFoundException";

/// The HTTP status [`KMS_NOT_FOUND_CODE`] is answered with.
pub const KMS_NOT_FOUND_STATUS: u16 = 400;

/// The wire error code S3 answers a WRITE whose SSE-KMS key is in a state KMS will not mint
/// a data key under, and the status it carries.
///
/// The envelope call behind the write fails inside KMS — a key whose key state refuses
/// cryptographic operations mints no data key — so S3 relays that failure under the KMS
/// exception's name qualified by the service that raised it, exactly as it relays an absent
/// key ([`KMS_NOT_FOUND_CODE`]). WHICH exception KMS raised is the key state's own: a key that
/// is merely DISABLED is [`KMS_DISABLED_CODE`], and this code is the one for every other
/// unusable state ([`unusable`]). Recorded behaviour: the s3 model binds no error shape for
/// the relay, so neither the code nor the status can come from it; the message is KMS's own
/// ([`kms_store::invalid_state_message`]).
pub const KMS_INVALID_STATE_CODE: &str = "KMS.KMSInvalidStateException";

/// The HTTP status [`KMS_INVALID_STATE_CODE`] is answered with — the status of this relay
/// family, the one [`KMS_NOT_FOUND_STATUS`] carries.
pub const KMS_INVALID_STATE_STATUS: u16 = KMS_NOT_FOUND_STATUS;

/// The wire error code S3 answers a WRITE whose SSE-KMS key is DISABLED.
///
/// A disabled key is the one unusable key state KMS words its own exception for: every
/// cryptographic operation on such a key raises `DisabledException`, "the request was
/// rejected because the specified KMS key is not enabled"
/// (`smithy_query(query="shape",shape="DisabledException",model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")`),
/// which KMS's data-key operations declare beside `KMSInvalidStateException` — so the envelope
/// call behind the write fails with that exception and S3 relays it under its name, qualified
/// by the service that raised it, rather than under the one for the other unusable states. The
/// distinction is what a client branches on: a disabled key is re-enabled and the write
/// retried, while a key pending deletion is in a lifecycle state no retry recovers.
pub const KMS_DISABLED_CODE: &str = "KMS.DisabledException";

/// The HTTP status [`KMS_DISABLED_CODE`] is answered with — the status of this relay family,
/// the one [`KMS_NOT_FOUND_STATUS`] carries.
pub const KMS_DISABLED_STATUS: u16 = KMS_NOT_FOUND_STATUS;

/// The Amazon Web Services managed key for Amazon S3, named by the predefined Amazon Web
/// Services alias AWS publishes for it.
///
/// "You can optionally tell Amazon S3 to encrypt data at rest by using server-side encryption
/// with other key options" (`PutObjectRequest$ServerSideEncryption`), and SSE-KMS names no key
/// of its own when neither the request nor the bucket's default rule carries one —
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID` is optional, and so is
/// `PutObjectRequest$SSEKMSKeyId` — so what encrypts such a write is this account's Amazon Web
/// Services managed key for the service, whose alias has the `aws/<service-name>` form KMS
/// documents ([`kms_store::is_predefined_aws_alias`]) with this service's own endpoint name.
pub const AWS_MANAGED_KEY_ALIAS: &str = "alias/aws/s3";

/// A refusal KMS raised that S3 RELAYS to the caller of the write: the code, the status and
/// the sentence KMS worded, kept apart so the handler answers them through the unmodelled
/// channel unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relay {
    /// The wire error code — [`KMS_NOT_FOUND_CODE`], [`KMS_DISABLED_CODE`] or
    /// [`KMS_INVALID_STATE_CODE`].
    pub code: &'static str,
    /// The HTTP status that code is answered with.
    pub status: u16,
    /// KMS's own sentence for the failure.
    pub message: String,
}

/// What KMS answers about the key one write's server-side encryption would be performed
/// under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteKey {
    /// Nothing was asked of KMS: the write is not SSE-KMS, the engine carries no KMS service
    /// to ask, or the reference is one KMS declines BEFORE resolving anything (an ARN naming
    /// another account, or an identifier that is no key reference at all) — a want of
    /// permission rather than an absence, which is the subject of a different relay. The
    /// write is stored under the reference it named, unresolved.
    Unasked,
    /// The ARN of the key the object is encrypted under, which is what the object records and
    /// every read and write response reports.
    Resolved(String),
    /// KMS refuses to mint a data key, so there is nothing to encrypt the object with and the
    /// write is refused rather than recorded.
    Refused(Relay),
}

/// The key one write's SSE-KMS encryption is performed under, resolved against KMS.
///
/// Every write that asks S3 for SSE-KMS is encrypted under a data key KMS mints, so the
/// encryption's key reference is resolved against KMS before anything is stored, and three
/// things come of that resolution:
///
/// * a reference KMS holds no key for cannot produce a data key, so the write is refused with
///   KMS's own sentence for the form the request supplied
///   ([`kms_store::key_not_found_message`]) rather than recorded under a key that does not
///   exist;
/// * a key whose key state refuses cryptographic operations — one awaiting deletion, or
///   disabled — mints no data key either, so the write is refused under the exception that
///   state raises inside KMS, with the sentence KMS words for it ([`unusable`]);
/// * a reference KMS DOES resolve names ONE key whatever form the reference took, so what the
///   object records is that key's ARN: "you can specify the key ID, key alias, or the Amazon
///   Resource Name (ARN) of the KMS key" (`PutObjectRequest$SSEKMSKeyId`) are three spellings
///   of one key, and the value every reader of the object reports is the key itself.
///
/// An encryption naming NO key is encrypted under the Amazon Web Services managed key for
/// this service ([`AWS_MANAGED_KEY_ALIAS`]), whose alias is a PREDEFINED Amazon Web Services
/// one: such an alias "exists in every account and Region while carrying no target key", and
/// "if you specify a predefined Amazon Web Services alias … KMS associates the alias with an
/// Amazon Web Services managed key"
/// ([`kms_store::ComAmazonawsKmsTrentserviceState::associate_predefined_aws_alias`]) — so
/// asking for it is what gives it a key, whose `KeyManager` is `AWS`, and the ARN resolved is
/// that key's. The same resolution every other service of this tree makes of such a
/// reference.
///
/// `region` is the Region the request was signed for, which is where the key is resolved: a
/// KMS key is a regional resource, and the object and the key it is encrypted under live at
/// the same endpoint.
///
/// Only a WRITE asks this: resolving a predefined alias materialises the managed key behind
/// it, and a read that did that would change what a later caller sees. A reader reports the
/// ARN the write recorded.
///
/// LOCKING: this takes the KMS store's lock and no other, and a caller holds NONE of this
/// service's own while it asks — the encryption it asks about is read out of this service's
/// store and that guard released first, so no request can hold the two in opposite orders.
pub fn write_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    encryption: &super::ObjectEncryption,
) -> WriteKey {
    if !encryption.uses_kms() {
        return WriteKey::Unasked;
    }
    let Some(kms) = kms_store::service_state(engine) else {
        return WriteKey::Unasked;
    };
    let reference = encryption
        .kms_key_id
        .as_deref()
        .unwrap_or(AWS_MANAGED_KEY_ALIAS);
    let mut store = kms.lock();
    if kms_store::is_predefined_aws_alias(reference) {
        return match store.associate_predefined_aws_alias(region, reference) {
            Some(key) => WriteKey::Resolved(key.arn.clone()),
            None => WriteKey::Refused(not_found(region, reference)),
        };
    }
    match store.resolve(region, reference) {
        kms_store::KeyRef::Found(key) if !key.is_usable() => WriteKey::Refused(unusable(key)),
        kms_store::KeyRef::Found(key) => WriteKey::Resolved(key.arn.clone()),
        kms_store::KeyRef::NotFound => WriteKey::Refused(not_found(region, reference)),
        kms_store::KeyRef::MalformedArn | kms_store::KeyRef::UnauthorizedAccount => {
            WriteKey::Unasked
        }
    }
}

/// Whether KMS, at `region`'s endpoint, resolves `reference` to a key THIS account owns.
///
/// The question an operation restricted to the bucket owner's OWN keys asks before it records
/// a key on a resource: a KMS key ARN names one key in one Region of one account, and KMS
/// resolves such a reference to a key only when both match the endpoint it was asked of
/// ([`kms_store::ComAmazonawsKmsTrentserviceState::resolve`]), so a reference another
/// account's key is named by, one naming a Region this endpoint does not serve, and one no
/// key exists for all alike resolve nothing here.
///
/// Read-only, unlike [`write_key`]: nothing is materialised for the reference, so asking does
/// not change what a later caller sees. The reference is taken as given rather than defaulted,
/// because a member that REQUIRES a key ARN names one.
///
/// An engine carrying no KMS store cannot answer the question at all, which is reported as
/// nothing contradicting the reference — the same silence [`WriteKey::Unasked`] carries.
///
/// LOCKING: this takes the KMS store's lock and no other, under [`write_key`]'s discipline —
/// a caller holds NONE of this service's own while it asks.
pub fn resolves_an_owned_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    reference: &str,
) -> bool {
    let Some(kms) = kms_store::service_state(engine) else {
        return true;
    };
    let store = kms.lock();
    matches!(store.resolve(region, reference), kms_store::KeyRef::Found(_))
}

/// The relay for a key whose key state mints no data key, worded by KMS for that state.
///
/// The exception is the key state's own, because it is KMS that raises it: a DISABLED key
/// refuses a cryptographic operation with `DisabledException` ([`KMS_DISABLED_CODE`]), and a
/// key in any other state outside `Enabled` — one awaiting deletion, one awaiting imported
/// material — with `KMSInvalidStateException` ([`KMS_INVALID_STATE_CODE`]), which is the same
/// split KMS's own data-key handler makes. The sentence is KMS's for the state the key is
/// actually in, whichever of the two it is.
fn unusable(key: &kms_store::Cmk) -> Relay {
    let disabled = key.key_state == kms_store::KEY_STATE_DISABLED;
    Relay {
        code: if disabled {
            KMS_DISABLED_CODE
        } else {
            KMS_INVALID_STATE_CODE
        },
        status: if disabled {
            KMS_DISABLED_STATUS
        } else {
            KMS_INVALID_STATE_STATUS
        },
        message: kms_store::invalid_state_message(key),
    }
}

/// The relay for a reference this endpoint holds no key for, worded by KMS for the form the
/// reference took.
fn not_found(region: &str, reference: &str) -> Relay {
    Relay {
        code: KMS_NOT_FOUND_CODE,
        status: KMS_NOT_FOUND_STATUS,
        message: kms_store::key_not_found_message(region, reference),
    }
}

/// Whose read this is, as the key policy decides it.
///
/// A read ANOTHER AWS SERVICE is performing as its own service principal for one of its
/// resources is that principal, acting for that resource: the calling service signed the call
/// with a credential it created for it alone and recorded both under that credential's access
/// key id, so the record is resolved from the access key id the request PRESENTS — through
/// this service's ONE reading of who a request speaks for
/// ([`crate::service::request_authorization::service_principal_call`], which carries both the
/// credential and the account the record is kept to). Every other read is the principal of the
/// credential it presents, which is KMS's own resolution
/// ([`kms_store::authorization::caller`]).
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> kms_store::authorization::Caller {
    if let Some(call) = crate::service::request_authorization::service_principal_call(engine, ctx) {
        ::tracing::debug!(
            principal = %call.principal,
            source_arn = %call.source_arn,
            "this read is one another service is making as its own service principal, so \
             the object's key is asked about that principal"
        );
        return kms_store::authorization::Caller::service_principal(
            &call.principal,
            &call.source_arn,
        );
    }
    kms_store::authorization::caller(engine, ctx)
}

/// The encryption context the data key of `object` is bound to, which is what a key policy
/// statement scoped to one bucket or one object is decided against.
///
/// [`ARN_CONTEXT_KEY`] names the resource being encrypted, and the pairs the write supplied
/// in `x-amz-server-side-encryption-context` come beside it: that header carries "the
/// Amazon Web Services KMS Encryption Context as an additional encryption context to use
/// for object encryption" (`PutObjectRequest$SSEKMSEncryptionContext`), so what a read
/// decrypts under is the one entry S3 always binds plus whatever the write added.
fn encryption_context(object: &EncryptedObject<'_>) -> HashMap<String, String> {
    let encrypted = match object.encryption.bucket_key_enabled {
        // "Setting this header to true causes Amazon S3 to use an S3 Bucket Key"
        // (`PutObjectRequest$BucketKeyEnabled`), and a bucket-level key is bound to the
        // bucket rather than to each object under it.
        Some(true) => super::bucket_arn(object.bucket),
        _ => super::object_arn(object.bucket, object.key),
    };
    let mut context = HashMap::from([(ARN_CONTEXT_KEY.to_string(), encrypted)]);
    context.extend(supplied_context(
        object.encryption.kms_encryption_context.as_deref(),
    ));
    context
}

/// The pairs an object's stored `SSEKMSEncryptionContext` names: "a Base64-encoded string
/// holding a UTF-8 encoded JSON of additional encryption context key-value pairs"
/// (`PutObjectRequest$SSEKMSEncryptionContext`), decoded.
///
/// A value that is not that — the member is stored verbatim as the write sent it — adds no
/// pair, so a read is decided under the entry S3 itself binds and nothing invented.
fn supplied_context(header: Option<&str>) -> Vec<(String, String)> {
    let Some(decoded) = header.and_then(|value| ::aws_smithy_types::base64::decode(value).ok())
    else {
        return Vec::new();
    };
    let Ok(::serde_json::Value::Object(pairs)) =
        ::serde_json::from_slice::<::serde_json::Value>(&decoded)
    else {
        return Vec::new();
    };
    pairs
        .into_iter()
        .filter_map(|(name, value)| Some((name, value.as_str()?.to_string())))
        .collect()
}

/// The value of the `kms:ViaService` condition key for a cryptographic operation this
/// service performs on a caller's behalf: this service's regional endpoint name, which is
/// what a key policy confining a key to S3's use of it names.
fn via_service(region: &str) -> String {
    format!("{ENDPOINT_PREFIX}.{region}.amazonaws.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of [`the_encryption_context_names_what_was_encrypted`]: the case, whether an
    /// S3 Bucket Key is in force, the context the write supplied, the ARN the entry must
    /// name, and how many entries the context must hold.
    type ContextRow = (&'static str, Option<bool>, Option<String>, &'static str, usize);

    /// The context an object's record is decrypted under: the rows differ along whether an
    /// S3 Bucket Key binds it to the bucket, and along what the write supplied beside it
    /// (nothing, one pair, several, and a value that is not an encoded JSON object).
    #[test]
    fn the_encryption_context_names_what_was_encrypted() {
        let pairs = ::aws_smithy_types::base64::encode(br#"{"tenant":"payments","zone":"a"}"#);
        let one = ::aws_smithy_types::base64::encode(br#"{"tenant":"payments"}"#);
        let rows: [ContextRow; 5] = [
            (
                "an object key, no supplied context",
                None,
                None,
                "arn:aws:s3:::ledger/reports/held.csv",
                1,
            ),
            (
                "an S3 Bucket Key binds the bucket",
                Some(true),
                None,
                "arn:aws:s3:::ledger",
                1,
            ),
            (
                "an S3 Bucket Key switched off binds the object",
                Some(false),
                None,
                "arn:aws:s3:::ledger/reports/held.csv",
                1,
            ),
            (
                "one supplied pair beside the object entry",
                None,
                Some(one),
                "arn:aws:s3:::ledger/reports/held.csv",
                2,
            ),
            (
                "several supplied pairs",
                None,
                Some(pairs),
                "arn:aws:s3:::ledger/reports/held.csv",
                3,
            ),
        ];
        for (case, bucket_key_enabled, supplied, arn, entries) in rows {
            let encryption = super::super::ObjectEncryption {
                sse_algorithm: Some(super::super::KMS_SSE_ALGORITHMS[0].to_string()),
                kms_key_id: Some("alias/ledger".to_string()),
                kms_encryption_context: supplied,
                bucket_key_enabled,
                ..super::super::ObjectEncryption::default()
            };
            let context = encryption_context(&EncryptedObject {
                bucket: "ledger",
                key: "reports/held.csv",
                encryption: &encryption,
            });
            assert_eq!(
                context.get(ARN_CONTEXT_KEY).map(String::as_str),
                Some(arn),
                "case {case:?}: expected {arn:?}, got {context:?}"
            );
            assert_eq!(
                context.len(),
                entries,
                "case {case:?}: expected {entries} entries, got {context:?}"
            );
        }
    }

    /// A stored context that is not an encoded JSON object of strings adds no pair rather
    /// than inventing one: the rows differ along how the value fails to be one.
    #[test]
    fn a_stored_context_that_is_not_encoded_pairs_adds_nothing() {
        let rows: [(&str, Option<String>); 5] = [
            ("no header at all", None),
            ("not base64", Some("not base64 at all".to_string())),
            (
                "base64 of something that is not JSON",
                Some(::aws_smithy_types::base64::encode(b"tenant=payments")),
            ),
            (
                "base64 of a JSON array",
                Some(::aws_smithy_types::base64::encode(br#"["tenant"]"#)),
            ),
            (
                "base64 of a JSON object whose value is not a string",
                Some(::aws_smithy_types::base64::encode(br#"{"tenant":7}"#)),
            ),
        ];
        for (case, header) in rows {
            let supplied = supplied_context(header.as_deref());
            assert!(
                supplied.is_empty(),
                "case {case:?}: expected no pair, got {supplied:?}"
            );
        }
    }
}
