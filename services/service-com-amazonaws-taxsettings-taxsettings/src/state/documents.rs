// SPDX-License-Identifier: CC0-1.0

//! The CONTENT of the tax documents submitted with a registration: where this
//! service keeps each document, and the delivery of one into the bucket a
//! download names.
//!
//! `GetTaxRegistrationDocument` "downloads your tax documents to the Amazon S3
//! bucket that you specify in your request"
//! (`smithy_query(query="doc",
//! shape="com.amazonaws.taxsettings#GetTaxRegistrationDocument")`), so the
//! operation DELIVERS an object rather than only reporting where one would be —
//! and a delivery has something to deliver only if the write that accepted the
//! document kept its content. Both halves live here:
//!
//! * a write holds this service's own copy of every document submitted in
//!   `VerificationDetails$taxRegistrationDocuments` ([`hold_documents`]), at the
//!   very location a read of that document reports
//!   ([`super::document_download_url`]), and drops the copies of a revision whose
//!   access tokens it retires ([`release_documents`]);
//! * a download writes that copy into the location the request named
//!   ([`deliver_document`]).
//!
//! The documents are OBJECTS, and the object protocol of this emulator is served
//! by Amazon S3, so they are held in THAT service's store rather than in a second
//! copy of this service's own: a caller that reads the reported location and the
//! delivery below then see the one store, and that store's own ceilings are what
//! bound the bytes a sequence of writes leaves resident. The bucket an account's
//! documents are held in is this service's own
//! ([`DOCUMENT_BUCKET_PROVISIONER`]) — it stays out of the account's own
//! `ListBuckets`, and a bucket of that name anybody else holds is never written
//! into.
//!
//! LOCK ORDER: every function here takes the object store's lock, holds no other
//! while it does and takes owned values rather than a guard — so each must be
//! called with NO guard of this service's own store held, exactly like
//! [`super::organization`]'s read and [`super::support_case`]'s write. Keeping the
//! cross-service access one-directional and guard-free is what makes a lock cycle
//! between the two stores unreachable.

use ::service_com_amazonaws_s3_amazons3::state as objects;

use super::{document_bucket, HeldTaxDocument};
use crate::ops::types::TaxRegistrationEntry;

/// What the bucket holding one account's tax documents is recorded as
/// provisioned by in the one object store (`Bucket::provisioned_by`), which keeps
/// it out of the account's own `ListBuckets`: the documents there are the
/// SERVICE's copies of what was submitted with the account's tax registration,
/// not objects the caller created.
///
/// It is also what tells this service's bucket from a bucket of the same name
/// anybody else holds, so a write of a document never lands in one that is not
/// this service's.
pub const DOCUMENT_BUCKET_PROVISIONER: &str = "taxsettings";

/// The object key ONE held document occupies in the account's document bucket:
/// the coordinates its access token carries — the registration revision the
/// document was submitted with, and the document's position in that submission.
///
/// The one statement of where a document is kept, so the location a read reports
/// ([`super::document_download_url`]), the object a write holds and the copy a
/// download delivers cannot drift apart.
pub fn document_object_key(revision: u64, index: usize) -> String {
    format!("{revision}/{index}")
}

/// What a stored object this service writes carries as its content type.
///
/// A submitted `TaxRegistrationDocument` names no media type — it carries a name
/// and bytes (`smithy_query(query="shape", shape="TaxRegistrationDocFile")`) — so
/// the object is stored without one, exactly as an object written by a
/// `PutObject` that named none is.
const DOCUMENT_CONTENT_TYPE: Option<String> = None;

/// One submitted document whose content this service holds: its position in the
/// submission, which is the coordinate its access token and its object key carry,
/// and the bytes themselves.
#[derive(Debug, Clone)]
pub struct SubmittedDocument {
    /// The document's position in `VerificationDetails$taxRegistrationDocuments`,
    /// from zero.
    pub index: usize,
    /// The document's content.
    pub content: Vec<u8>,
}

/// The content of each document submitted with `entry`, in submission order.
///
/// A `TaxRegistrationDocument` carries its content one of two ways
/// (`smithy_query(query="shape", shape="TaxRegistrationDocument")`): an uploaded
/// `file`, whose `fileContent` IS the document, or an `s3Location` naming the
/// object the document is already stored as in the caller's own bucket, whose
/// bytes are read from the one object store. A document naming both is taken from
/// the file, the same precedence [`super::document_metadatas`] names it by.
///
/// A document this emulator can find no content for is left out rather than held
/// as something it is not: an `s3Location` naming an object no bucket of the
/// caller's holds is a reference nothing here can resolve, so there are no bytes
/// to copy and a later download has none to deliver. Only the caller's own
/// buckets are read ([`caller_bucket`]).
///
/// Called BEFORE this service's own store is locked — it takes the object store's
/// lock itself (see the module's LOCK ORDER).
pub fn submitted_documents<Eng: Send + Sync + 'static>(
    engine: &Eng,
    entry: &TaxRegistrationEntry,
) -> Vec<SubmittedDocument> {
    let Some(documents) = entry
        .verification_details
        .as_ref()
        .and_then(|details| details.tax_registration_documents.as_ref())
    else {
        return Vec::new();
    };
    // The object store is reached only for a document that REFERENCES an object,
    // so a submission of uploaded files alone takes no lock of it at all.
    let references = documents
        .iter()
        .any(|document| document.file.is_none() && document.s3_location.is_some());
    let state = references.then(|| objects::service_state(engine)).flatten();
    let store = state.as_ref().map(|state| state.lock());
    documents
        .iter()
        .enumerate()
        .filter_map(|(index, document)| {
            let content = match (document.file.as_ref(), document.s3_location.as_ref()) {
                (Some(file), _) => Some(file.file_content.clone()),
                (None, Some(location)) => {
                    // The location is the CALLER's: only a bucket of the caller's
                    // own is a document source ([`caller_bucket`]), so a request
                    // cannot name a bucket another service stood up — this
                    // service's own document buckets among them — and have its
                    // content copied into the registration being written.
                    let held = store
                        .as_ref()
                        .and_then(|store| caller_bucket(store, &location.bucket))
                        .and_then(|bucket| bucket.object(&location.key))
                        .map(|object| object.body.clone());
                    if held.is_none() {
                        ::tracing::warn!(
                            bucket = %location.bucket,
                            key = %location.key,
                            "no bucket of the caller's own holds an object at the location a \
                             submitted tax registration document names, so this service holds \
                             no content for that document"
                        );
                    }
                    held
                }
                (None, None) => None,
            }?;
            Some(SubmittedDocument { index, content })
        })
        .collect()
}

/// What [`hold_documents`] did with one write's documents.
///
/// It is what lets a writer that stores ONE entry for MANY accounts
/// (`BatchPutTaxRegistration` names up to 550) stop copying: the content of a
/// submission is the same bytes for every account it is written to, so once the
/// object store will take no more of them, copying them again per remaining
/// account would do the work of the whole fan-out only to be refused each time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentsHeld {
    /// Every document the write carried is held — including a write that carried
    /// none, which has nothing to hold.
    Held,
    /// The copies were not taken and no further copy of this request's would be
    /// either: the engine carries no object store, the account's document bucket
    /// is not one this service holds, or that store is at a ceiling of its own.
    /// Each condition is logged where it is answered.
    TakesNoMore,
}

/// Hold this service's own copies of the documents a write stored as `revision`
/// of `account_id`'s registration, where a read of one reports it.
///
/// The copies go into the account's document bucket
/// ([`super::document_bucket`]), which this service stands up on the first write
/// that carries a document for the account and which no other holder of that name
/// is adopted for. One bucket per account, named from the account alone, so a
/// sequence of writes records no bucket per request; past the object store's own
/// bucket quota it records none at all, and the bytes those buckets hold are
/// bounded by that store's own ceilings — which is also why a retired revision's
/// copies are dropped rather than left ([`release_documents`]).
///
/// The copying this does is bounded by those same ceilings: it stops at the FIRST
/// document the store would not take and answers [`DocumentsHeld::TakesNoMore`],
/// so neither the documents behind it nor the accounts behind this one are copied
/// for a store that is full ([`DocumentsHeld`]).
///
/// A copy the store would not take is a document the service holds no content for
/// rather than a failure of the write that submitted it — which is also what an
/// engine with no object store on it leaves behind — so the registration is
/// stored either way and a download of such a document delivers nothing.
///
/// Called with NO guard of this service's own store held (see the module's LOCK
/// ORDER).
pub fn hold_documents<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
    revision: u64,
    documents: &[SubmittedDocument],
) -> DocumentsHeld {
    if documents.is_empty() {
        return DocumentsHeld::Held;
    }
    let bucket = document_bucket(account_id);
    let Some(state) = objects::service_state(engine) else {
        ::tracing::warn!(
            %bucket,
            "no object store is reachable from this engine, so a tax registration's documents \
             are held nowhere"
        );
        return DocumentsHeld::TakesNoMore;
    };
    let mut store = state.lock();
    if !service_owned_bucket(&mut store, &bucket) {
        ::tracing::warn!(
            %bucket,
            "the object store holds no tax-document bucket of this service's for this account, \
             so its documents are held nowhere"
        );
        return DocumentsHeld::TakesNoMore;
    }
    for document in documents {
        let key = document_object_key(revision, document.index);
        let stored = objects::Object::stored(document.content.clone(), DOCUMENT_CONTENT_TYPE);
        match store.put_object(&bucket, &key, stored) {
            Some(objects::ObjectWrite::Created | objects::ObjectWrite::Replaced) => {}
            // A ceiling of the object store's own (its keys, its counts or its
            // byte budgets), which the next copy would meet as well: stop here
            // rather than copying the rest of this request's documents into a
            // store that is refusing them.
            outcome => {
                ::tracing::warn!(
                    %bucket,
                    %key,
                    ?outcome,
                    "the object store did not take this service's copy of a submitted tax \
                     registration document, so no further copy of this write was attempted"
                );
                return DocumentsHeld::TakesNoMore;
            }
        }
    }
    DocumentsHeld::Held
}

/// Drop the copies of the `documents` documents of `revision` of `account_id`'s
/// registration — the write-side counterpart of [`hold_documents`].
///
/// A document access token names one revision of one account's registration, so
/// rewriting or deleting that registration retires every token it had AND the
/// location each of those tokens named: the copies of a retired revision are gone
/// from the object store, exactly as its tokens are gone from this service's
/// state. Only a bucket this service provisioned is touched, for the reason
/// [`DOCUMENT_BUCKET_PROVISIONER`] states.
///
/// Called with NO guard of this service's own store held (see the module's LOCK
/// ORDER).
pub fn release_documents<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
    revision: u64,
    documents: usize,
) {
    if documents == 0 {
        return;
    }
    let bucket = document_bucket(account_id);
    let Some(state) = objects::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    if store
        .bucket(&bucket)
        .and_then(objects::Bucket::provisioned_by)
        != Some(DOCUMENT_BUCKET_PROVISIONER)
    {
        return;
    }
    for index in 0..documents {
        store.remove_object(&bucket, &document_object_key(revision, index));
    }
}

/// Download `document` into the place `key` names in `bucket`: the delivery
/// `GetTaxRegistrationDocument` is documented to perform.
///
/// The object written is the copy this service holds ([`hold_documents`]), so what
/// the destination receives is the content that was submitted rather than anything
/// assembled here, and a caller reading the destination reads the same bytes a
/// read of the reported download location serves.
///
/// Delivering the same document to the same place twice writes the same object, so
/// a caller that asks twice is left with exactly what one call left: the request
/// names where the document goes, and nothing about the destination's state is
/// consumed or counted.
///
/// The delivery is confined to the caller's own buckets ([`caller_bucket`]): a
/// destination the store holds no bucket of the caller's for takes nothing — there
/// is nowhere for this service to write — and neither does a document this service
/// holds no content for; both are logged. Nothing of this service's own state is
/// touched either way, so a later read of the document is answered exactly what
/// this one was.
///
/// Called with NO guard of this service's own store held (see the module's LOCK
/// ORDER).
pub fn deliver_document<Eng: Send + Sync + 'static>(
    engine: &Eng,
    document: &HeldTaxDocument,
    bucket: &str,
    key: &str,
) {
    let held_bucket = document_bucket(&document.account_id);
    let held_key = document_object_key(document.revision, document.index);
    let Some(state) = objects::service_state(engine) else {
        ::tracing::warn!(
            %bucket,
            %key,
            "no object store is reachable from this engine, so no tax document was downloaded"
        );
        return;
    };
    let mut store = state.lock();
    // The destination is the CALLER's: only a bucket of the caller's own is
    // downloaded into ([`caller_bucket`]), so a request cannot name a bucket
    // another service stood up — this service's own document buckets among them,
    // whose names and keys are derived rather than secret — and have one
    // account's held document replaced by another's.
    if caller_bucket(&store, bucket).is_none() {
        ::tracing::warn!(
            %bucket,
            %key,
            "the destination the request named is no bucket of the caller's own, so nothing was \
             downloaded into it"
        );
        return;
    }
    let Some(content) = store
        .bucket(&held_bucket)
        .and_then(|held| held.object(&held_key))
        .map(|object| object.body.clone())
    else {
        ::tracing::warn!(
            bucket = %held_bucket,
            key = %held_key,
            "this service holds no content for the tax document a download asked for, so \
             nothing was downloaded"
        );
        return;
    };
    let stored = objects::Object::stored(content, DOCUMENT_CONTENT_TYPE);
    match store.put_object(bucket, key, stored) {
        Some(objects::ObjectWrite::Created | objects::ObjectWrite::Replaced) => {}
        // Any answer but a write is a condition of the object store's own (a key
        // or a budget of that destination); the bucket being one this emulator
        // does not hold was answered above.
        outcome => ::tracing::warn!(
            %bucket,
            %key,
            ?outcome,
            "the destination the request named did not take the tax document"
        ),
    }
}

/// The bucket named `bucket` when it is one of the CALLER's own, or `None` when
/// the store holds no such bucket or holds it for a service rather than for the
/// caller.
///
/// The one screen every CALLER-SUPPLIED location of this module passes through —
/// the source an `s3Location` reads from and the destination a download writes to
/// — so a request can name only buckets its own account created. A bucket another
/// service stood up (`Bucket::provisioned_by`) is not one of them: this service's
/// own document buckets are exactly such buckets, and their names and keys are
/// derived from the account and the revision rather than secret, so without this
/// screen a request could name one account's held documents as its source or as
/// its destination and read or replace them. The write side screens the same fact
/// from the other direction ([`service_owned_bucket`]).
fn caller_bucket<'store>(
    store: &'store objects::ComAmazonawsS3Amazons3State,
    bucket: &str,
) -> Option<&'store objects::Bucket> {
    store
        .bucket(bucket)
        .filter(|held| held.provisioned_by().is_none())
}

/// Whether the object store holds this service's own document bucket under
/// `bucket`, standing it up when it does not.
///
/// IDEMPOTENT: the name is derived from the account, so a call either records that
/// account's bucket or finds it already recorded. A bucket of that name the store
/// holds for anybody else is NOT adopted — a document written into it would be
/// written into a bucket that is not this service's — so the answer is `false` and
/// the documents are held nowhere.
fn service_owned_bucket(
    store: &mut objects::ComAmazonawsS3Amazons3State,
    bucket: &str,
) -> bool {
    match store.provision_bucket(bucket, objects::DEFAULT_BUCKET_REGION, DOCUMENT_BUCKET_PROVISIONER)
    {
        objects::BucketCreation::Created => true,
        // The name is in use, which for this derived name is ordinarily the
        // bucket an earlier write recorded — but a name taken by ANY bucket
        // answers this, so the holder of it is what decides.
        objects::BucketCreation::NameTaken => {
            store
                .bucket(bucket)
                .and_then(objects::Bucket::provisioned_by)
                == Some(DOCUMENT_BUCKET_PROVISIONER)
        }
        objects::BucketCreation::QuotaReached => false,
    }
}
