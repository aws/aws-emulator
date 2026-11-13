// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3-amazons3 service.
//!
//! The bucket map is the only place a bucket or an object exists: CreateBucket /
//! HeadBucket answer from a bucket's own record, and the object-storage loop
//! (PutObject / GetObject / ListObjectsV2 / DeleteObject / DeleteBucket) computes
//! every response from the objects stored inside it.
//!
//! A bucket whose versioning status is `Enabled` holds more than one version of a key:
//! the CURRENT one, which every reader that names no `versionId` answers with, and the
//! noncurrent ones a later write or a delete displaced, each addressable by the version id
//! the store minted for it ([`mint_version_id`]). A delete that names no version adds a
//! version of its own — a [`DeleteMarker`], which "becomes the current version of the
//! object" and takes nothing away — so a key's versions are one chain of
//! [`StoredVersion`]s, newest first. Every stored version counts against the same ceilings
//! a current object does, and every one of them has to be deleted before the bucket can
//! be ([`Bucket::holds_no_versions`]).
//!
//! A bucket also holds the multipart uploads it has IN PROGRESS ([`MultipartUpload`]),
//! each addressable by the upload id the store minted for it ([`mint_upload_id`]) until a
//! completion or an abort takes it away. An in-progress upload is not an object: it is
//! where everything the initiating `CreateMultipartUpload` settled about the object it
//! will become is kept in the meantime, together with the parts uploaded into it so far
//! ([`UploadedPart`]), each addressable by its own part number.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use md5::{Digest, Md5};

// The ACCESS POINTS that front this store's buckets — which bucket each one reaches and the
// policy it is judged by — live in `state/access_points.rs`.
pub mod access_points;
// The bucket a request ADDRESSES, which is what a bucket-scoped refusal that did not name
// a bucket itself is named by — see `state/addressed_bucket.rs`.
pub(crate) mod addressed_bucket;
pub mod aws_published;
// What a bucket's stored notification configuration DOES when an object changes — which
// configured destinations are notified, what each notification carries, and the
// in-process delivery itself — lives in `state/notifications.rs`.
pub mod notifications;
// The request-correlation pair one response carries, which is also what a notification
// names the call it came of by — see `state/request_ids.rs`.
pub mod request_ids;

// What a bucket's stored replication configuration DOES when an object version is written
// or a delete marker is laid — which rules cover a key, where each one copies it, and what
// the source version then reports — lives in `state/replication.rs`.
pub mod replication;

// Whether the KMS key an SSE-KMS object was stored under lets THIS caller have its
// plaintext — the `kms:Decrypt` every read that serves such an object's bytes performs on
// the caller's behalf — lives in `state/sse_kms.rs`.
pub mod sse_kms;

// Whether the IAM role a metadata configuration names for its ANNOTATION table lets S3
// Metadata read annotations through it — the question every call that provisions that table
// asks before it does — lives in `state/metadata_annotation_role.rs`.
pub mod metadata_annotation_role;

/// The Region a general purpose bucket is created in when the request names no
/// `CreateBucketConfiguration.LocationConstraint`.
///
/// From that member's own documentation: "If you don't specify a Region, the bucket is
/// created in the US East (N. Virginia) Region (us-east-1) by default."
pub const DEFAULT_BUCKET_REGION: &str = "us-east-1";

/// The Region the legacy `LocationConstraint` value `EU` names.
///
/// Same member documentation: "Configurations using the value `EU` will create a bucket
/// in `eu-west-1`."
pub const EU_LOCATION_CONSTRAINT_REGION: &str = "eu-west-1";

/// The legacy `LocationConstraint` value that names [`EU_LOCATION_CONSTRAINT_REGION`]
/// rather than a Region code (it is one of `BucketLocationConstraint`'s enum values).
const EU_LOCATION_CONSTRAINT: &str = "EU";

/// How many general purpose buckets one account may hold, and so how many entries this
/// service's bucket map can ever carry.
///
/// AWS publishes this as a service quota — 10,000 general purpose buckets per account by
/// default — and the model corroborates the same ceiling: `ListBucketsRequest$MaxBuckets`
/// is bounded `range {min 1, max 10000}`
/// (`smithy_query(query="operation",operation="ListBuckets")`), i.e. one page may name
/// every bucket an account is allowed to hold. A create past it is refused rather than
/// recorded, so the buckets of the account this emulator keeps are bounded whatever a
/// caller does. It counts the ACCOUNT's buckets: a bucket another emulated service
/// provisioned as the object store of one of its own resources
/// ([`Bucket::provisioned_by`]) is bounded by that service's own limit on the resource it
/// belongs to.
pub const MAX_BUCKETS_PER_ACCOUNT: usize = 10_000;

/// What [`ComAmazonawsS3Amazons3State::create_bucket`] did, so the caller answers each
/// outcome without having to re-derive it — and cannot reach the stored-bucket cases
/// without having gone through the quota check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketCreation {
    /// The bucket was recorded.
    Created,
    /// A bucket of that name already exists; NOTHING was written, so the existing bucket
    /// keeps the Region it was created in. What to answer differs by that Region, so this
    /// reports the outcome rather than deciding it.
    NameTaken,
    /// The account already holds [`MAX_BUCKETS_PER_ACCOUNT`] buckets; nothing was written.
    QuotaReached,
}

/// The `StorageClass` an object gets when the `PutObject` request names none.
///
/// From `PutObjectRequest$StorageClass`'s own documentation: "By default, Amazon S3 uses
/// the STANDARD Storage Class to store newly created objects."
pub const DEFAULT_STORAGE_CLASS: &str = "STANDARD";

/// The `ObjectAttributes` value naming an object's entity tag, as the
/// `x-amz-object-attributes` header spells it.
///
/// The five root-level fields `GetObjectAttributes` reports are each named by one value of
/// that enum (`smithy_query(query="shape",shape="ObjectAttributes")` publishes `[ETag,
/// Checksum, ObjectParts, StorageClass, ObjectSize]`); these constants are what the handler
/// matches the header's values against, so the spelling lives in one place.
pub const OBJECT_ATTRIBUTE_ETAG: &str = "ETag";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's size in bytes.
pub const OBJECT_ATTRIBUTE_OBJECT_SIZE: &str = "ObjectSize";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's storage class.
pub const OBJECT_ATTRIBUTE_STORAGE_CLASS: &str = "StorageClass";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the object's checksum container.
pub const OBJECT_ATTRIBUTE_CHECKSUM: &str = "Checksum";
/// See [`OBJECT_ATTRIBUTE_ETAG`] — the value naming the collection of parts a multipart
/// upload assembled the object from.
pub const OBJECT_ATTRIBUTE_OBJECT_PARTS: &str = "ObjectParts";

/// The header `GetObjectAttributes` binds the fields it is to report from
/// (`smithy_query(query="operation",operation="GetObjectAttributes")` reports
/// `ObjectAttributes: {httpHeader: "x-amz-object-attributes", required: true}`), which is also
/// how S3's refusal of a request that states no field names it.
pub const OBJECT_ATTRIBUTES_HEADER: &str = "x-amz-object-attributes";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a `GetObjectAttributes` that
/// carries no `x-amz-object-attributes` at all.
///
/// "Specifies the fields at the root level that you want returned in the response"
/// (`GetObjectAttributesRequest$ObjectAttributes`, a REQUIRED `@httpHeader` member): a request
/// that carries no such header states no field, so there is no attribute name for a refusal to
/// report as one the published set does not list. Fixed text, naming the header and that it
/// carried nothing.
pub const OBJECT_ATTRIBUTES_UNSTATED_MESSAGE: &str =
    "The x-amz-object-attributes header specifying the attributes to be retrieved is either \
     missing or empty";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an `x-amz-object-attributes`
/// that names a field outside the published `ObjectAttributes` set.
///
/// Fixed text: the document names the rejected attribute in its `<ArgumentValue>` rather than
/// in the sentence.
pub const INVALID_OBJECT_ATTRIBUTE_MESSAGE: &str = "Invalid attribute name specified.";

/// The `StorageClass` values whose objects are ARCHIVED: not readable, not copyable and
/// not usable until a temporary copy of them has been restored.
///
/// "Before using an object as a source object for the copy operation, you must restore a
/// copy of it if … the storage class of the source object is `GLACIER` or `DEEP_ARCHIVE`"
/// (`smithy_query(query="doc",shape="CopyObjectRequest",member="StorageClass")`), and the
/// same two are the storage classes `InvalidObjectState` names — "the S3 Glacier Flexible
/// Retrieval storage class, the S3 Glacier Deep Archive storage class, the S3
/// Intelligent-Tiering Archive Access tier, or the S3 Intelligent-Tiering Deep Archive
/// Access tier" (`smithy_query(query="shape",shape="InvalidObjectState")`), whose other two
/// entries are TIERS of `INTELLIGENT_TIERING` rather than storage classes and are not
/// recorded here. Both are values of the `StorageClass` enum
/// (`smithy_query(query="shape",shape="StorageClass")`).
///
/// ONE list, because the operations that judge an object by it — `CopyObject`'s source and
/// `RestoreObject`'s target — must not disagree about which objects are archived.
pub const ARCHIVED_STORAGE_CLASSES: [&str; 2] = ["GLACIER", "DEEP_ARCHIVE"];

/// The longest object key this emulator records, in UTF-8 bytes.
///
/// The published S3 limit: "The name for a key is a sequence of Unicode characters whose
/// UTF-8 encoding is at most 1,024 bytes long" (Amazon S3 User Guide, "Creating object key
/// names"). The model constrains only the other end — `ObjectKey` is `length {min 1}`
/// (`smithy_query(query="shape",shape="ObjectKey")`) — so nothing refuses a longer key
/// before this check, and a key is allocated per stored object.
pub const MAX_OBJECT_KEY_BYTES: usize = 1_024;

/// How many objects one bucket in this emulator may hold.
///
/// NOT a fidelity claim: S3 publishes no such limit ("you can store any number of objects
/// in a bucket"). It is the emulator's own ceiling, because every object it records lives
/// in this process's memory — set generously so no realistic workflow meets it, and
/// enforced so a caller looping on `PutObject` with fresh keys cannot grow the store
/// without bound. A write past it is refused rather than recorded, the way a create past
/// [`MAX_BUCKETS_PER_ACCOUNT`] is.
pub const MAX_OBJECTS_PER_BUCKET: usize = 1_000;

/// How many bytes of object payload one bucket in this emulator may hold in total.
///
/// NOT a fidelity claim, in either direction: AWS bounds what one bucket holds not at all,
/// and the only size constraints it enforces are on a SINGLE payload — 5 GiB per
/// `PutObject`, and 5 MiB to 5 GiB per part of a multipart upload. This is the emulator's
/// own budget, because an object's bytes are kept verbatim in this process's memory, and it
/// is therefore set ABOVE the traffic AWS accepts rather than below it: an SDK or the CLI
/// uploading a file of tens of MiB in 8 MiB parts, several such files into one bucket, and
/// the parts of an upload in flight beside the objects already stored. A write past it is
/// refused rather than recorded — a refusal that is the emulator's own and says so, never a
/// quota the API announces.
pub const MAX_OBJECT_BYTES_PER_BUCKET: usize = 64 * 1024 * 1024;

/// How many objects this emulator will hold ACROSS every bucket.
///
/// The per-bucket ceilings alone would leave total memory bounded only by their product
/// with [`MAX_BUCKETS_PER_ACCOUNT`], so the service-wide ones below are what actually
/// bound this process: whatever a caller does — one bucket or ten thousand — the object
/// store holds at most this many objects and [`MAX_OBJECT_BYTES_TOTAL`] bytes of payload.
pub const MAX_OBJECTS_TOTAL: usize = 10_000;

/// How many bytes of object payload this emulator will hold across every bucket.
///
/// Same reason as [`MAX_OBJECTS_TOTAL`], and the same order as the service-wide retained
/// budgets of this workspace's peer services (appconfig's
/// `MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`, codeguruprofiler's `MAX_RETAINED_PROFILE_BYTES`).
/// A write that would take the whole store past it is refused rather than recorded.
///
/// Sized above [`MAX_OBJECT_BYTES_PER_BUCKET`] for the same reason that one is sized above
/// the traffic AWS accepts: several buckets each holding the multipart uploads of an
/// ordinary session is traffic S3 serves, so it is traffic this store records.
pub const MAX_OBJECT_BYTES_TOTAL: usize = 128 * 1024 * 1024;

/// How many sessions `CreateSession` keeps vended at once.
///
/// Each session is a credential this process accepts for signing until it expires, so the
/// count is what bounds that memory whatever a caller does — a vend past this is refused
/// rather than recorded, and an expired one is released
/// ([`ComAmazonawsS3Amazons3State::release_expired_sessions`]) before the bound is read, so
/// a caller that keeps a reasonable number of live sessions never meets it. Not a fidelity
/// claim: AWS publishes no limit on concurrent sessions, and the same order as the peer
/// credential budget in this workspace (s3control's `DATA_ACCESS_CREDENTIAL_QUOTA`).
pub const MAX_VENDED_SESSIONS: usize = 256;

/// The version id an object written while the bucket was NOT versioning-enabled is
/// addressed by.
///
/// "Objects stored in your bucket before you set the versioning state have a version ID
/// of null", and `PutBucketVersioning`'s own documentation says the same of a suspended
/// bucket: "Suspended—Disables versioning for the objects in the bucket. All objects
/// added to the bucket receive the version ID null." Such a version is stored with no
/// minted id at all ([`Object::version_id`] is `None`), so this is the string a request
/// names it by rather than a value this store keeps.
pub const NULL_VERSION_ID: &str = "null";

/// The `BucketVersioningStatus` value that makes a bucket version its objects.
///
/// "Enabled—Enables versioning for the objects in the bucket. All objects added to the
/// bucket receive a unique version ID" (`PutBucketVersioning`'s documentation).
pub const VERSIONING_ENABLED_STATUS: &str = "Enabled";

/// The `BucketVersioningStatus` value of a bucket whose versioning was turned off again.
///
/// "Suspended—Disables versioning for the objects in the bucket. All objects added to the
/// bucket receive the version ID null" (`PutBucketVersioning`'s documentation). A
/// suspended bucket KEEPS every version it already holds — suspension is a state of the
/// bucket, not of its versions — which is what makes it different from a bucket that was
/// never versioned at all.
pub const VERSIONING_SUSPENDED_STATUS: &str = "Suspended";

/// Who pays for downloads from a bucket whose request payment configuration has never
/// been set.
///
/// "By default, the bucket owner pays for downloads from the bucket"
/// (`PutBucketRequestPayment`'s own documentation), and `BucketOwner` is one of the two
/// values `Payer` takes (`PAYER_VALUES`) — so a never-configured bucket reports this
/// rather than reporting no payer at all.
pub const DEFAULT_PAYER: &str = "BucketOwner";

/// The most user-defined metadata one stored object may carry, in UTF-8 bytes summed
/// over every `x-amz-meta-` key and value.
///
/// The published S3 limit: "Within the PUT request header, the user-defined metadata is
/// limited to 2 KB in size" (Amazon S3 User Guide, "Working with object metadata"). The
/// model constrains `Metadata` not at all, so nothing refuses more before this check, and
/// every byte of it is retained with the object.
pub const MAX_OBJECT_METADATA_BYTES: usize = 2 * 1024;

/// What [`ComAmazonawsS3Amazons3State::put_object`] did, so the caller answers each
/// outcome without re-deriving it — and cannot record an object without having gone
/// through the key, count and byte-budget checks, per bucket AND service-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectWrite {
    /// The object was recorded at a key the bucket did not hold.
    Created,
    /// The object replaced the one the bucket held at that key.
    Replaced,
    /// The key is longer than [`MAX_OBJECT_KEY_BYTES`]; nothing was written.
    KeyTooLong { bytes: usize },
    /// The request's user-defined metadata is larger than
    /// [`MAX_OBJECT_METADATA_BYTES`]; nothing was written.
    MetadataTooLarge { bytes: usize },
    /// The bucket already holds [`MAX_OBJECTS_PER_BUCKET`] objects at other keys; nothing
    /// was written.
    ObjectCountReached,
    /// Recording the object would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// This emulator already holds [`MAX_OBJECTS_TOTAL`] objects across every bucket;
    /// nothing was written.
    TotalObjectCountReached,
    /// Recording the object would take the whole store past
    /// [`MAX_OBJECT_BYTES_TOTAL`] retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What a `DeleteObject` that named NO `versionId` did
/// ([`ComAmazonawsS3Amazons3State::remove_object`]), so the caller answers each outcome
/// without re-deriving it from the bucket's versioning state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectDeletion {
    /// The bucket holds no versioning state, and the `null` version at that key was
    /// removed; everything it retained is released.
    Removed,
    /// The bucket holds no versioning state and held nothing at that key, so nothing was
    /// written — "if there isn't a null version, Amazon S3 does not remove anything"
    /// (`DeleteObject`'s documentation).
    NothingThere,
    /// The bucket carries a versioning state, so a DELETE MARKER is now the key's current
    /// version, under `version_id` (`None` for the `null` marker of a suspended bucket).
    /// Every version behind it is still stored. `displaced_object` is whether the key had
    /// a current object for the marker to hide, which is what `DeleteObject`'s
    /// `x-amz-delete-marker` reports.
    MarkerLaid {
        version_id: Option<String>,
        displaced_object: bool,
    },
    /// The bucket is at [`MAX_OBJECTS_PER_BUCKET`] stored versions, so the delete marker
    /// this delete would lay was refused rather than recorded; nothing was written.
    VersionCountReached,
    /// This emulator is at [`MAX_OBJECTS_TOTAL`] stored versions across every bucket, so
    /// the delete marker was refused rather than recorded; nothing was written.
    TotalVersionCountReached,
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_tags`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record a tag set without having gone
/// through the byte budgets a stored object's tags count against, per bucket AND
/// service-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectTagWrite {
    /// The object's tag set is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no object at that key; nothing was written.
    NoSuchKey,
    /// Keeping the tag set would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the tag set would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_retention`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record a retention without
/// having gone through the byte budgets a stored object's members count against, per
/// bucket AND service-wide. The shape [`ObjectTagWrite`] has, for the same reason: a
/// retention is one more string kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRetentionWrite {
    /// The addressed object version's retention is now the one that was handed in (or
    /// none, when the call removed it).
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the retention would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the retention would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_encryption`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record an encryption
/// configuration without having gone through the byte budgets a stored object's members
/// count against, per bucket AND service-wide. The shape [`ObjectRetentionWrite`] has, for
/// the same reason: an encryption configuration is a handful of caller-supplied strings
/// kept beside an object ([`ObjectEncryption::retained_bytes`] counts them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectEncryptionWrite {
    /// The addressed object version's encryption configuration is now the one that was
    /// handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the configuration would take the bucket past
    /// [`MAX_OBJECT_BYTES_PER_BUCKET`] retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the configuration would take the whole store past
    /// [`MAX_OBJECT_BYTES_TOTAL`] retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_legal_hold`] did, so the caller
/// answers each outcome without re-deriving it.
///
/// Shorter than [`ObjectRetentionWrite`] by the two byte-budget outcomes, and for a
/// reason: a legal hold is one `ObjectLockLegalHoldStatus` value — whether the hold is on
/// or off — so what the store keeps for it is a flag rather than a caller-supplied
/// string, and it cannot take the bucket or the whole store past a byte budget the way a
/// retention's mode can ([`Object::retained_bytes`] counts no bytes for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectLegalHoldWrite {
    /// The addressed object version's legal hold status is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
}

/// What [`ComAmazonawsS3Amazons3State::replace_object_acl`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record an ACL without having gone
/// through the byte budgets a stored object's members count against, per bucket AND
/// service-wide. The shape [`ObjectRetentionWrite`] has, for the same reason: an ACL is a
/// list of caller-supplied strings kept beside an object ([`acl_grants_bytes`] measures
/// it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectAclWrite {
    /// The addressed object version's ACL is now the one that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the ACL would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`] retained
    /// bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the ACL would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`] retained
    /// bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::restore_object_version`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record a restore without having gone
/// through the byte budgets a stored object's members count against, per bucket AND
/// service-wide. The shape [`ObjectRetentionWrite`] has, for the same reason: a restore is
/// one more record kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRestoreWrite {
    /// The addressed object version now carries the restore that was handed in.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// Keeping the restore would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the restore would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::put_object_annotation`] did, so the caller answers
/// each outcome without re-deriving it — and cannot record an annotation without having
/// gone through the per-object annotation ceiling and the byte budgets a stored object's
/// members count against, per bucket AND service-wide. The shape [`ObjectRestoreWrite`]
/// has, for the same reason: an annotation is one more payload kept beside an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationWrite {
    /// The addressed object version now carries the annotation that was handed in, under
    /// the name it was handed in with.
    Applied,
    /// The bucket exists but holds no such object version; nothing was written.
    NoSuchObject,
    /// The object already carries [`MAX_ANNOTATIONS_PER_OBJECT`] annotations and this call
    /// names a new one; nothing was written.
    LimitReached,
    /// Keeping the annotation would take the bucket past [`MAX_OBJECT_BYTES_PER_BUCKET`]
    /// retained bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Keeping the annotation would take the whole store past [`MAX_OBJECT_BYTES_TOTAL`]
    /// retained bytes; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::remove_object_annotation`] did, so the caller
/// answers each outcome without re-deriving it — the removal counterpart of
/// [`AnnotationWrite`], and without its ceilings: taking an annotation away only releases
/// bytes, so no budget can refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationRemoval {
    /// The addressed object version carried that name and no longer does; the payload and
    /// everything it retained are gone.
    Removed,
    /// The bucket exists and holds the addressed object version, but that version carries
    /// no annotation under the name; nothing was removed.
    NoSuchAnnotation,
    /// The bucket exists but holds no such object version; nothing was removed.
    NoSuchObject,
}

/// How many annotations one object may carry.
///
/// From `PutObjectAnnotation`'s own documentation: "Each object can have up to 1,000
/// annotations." A write that would take an object past it is refused rather than
/// recorded.
pub const MAX_ANNOTATIONS_PER_OBJECT: usize = 1_000;

/// The longest an annotation name may be, in BYTES.
///
/// "Length Constraints: Minimum length of 1. Maximum length of 512 bytes"
/// (`PutObjectAnnotationRequest$AnnotationName`), which is the bound the
/// `AnnotationNameTooLong` error is answered past (the sentence that error carries states
/// the rule rather than this number — see `crate::scaffolded::DOCUMENTED_ERROR_MESSAGES`).
/// `AnnotationName` carries no `length` trait in the model, so nothing refuses an
/// over-long name before a handler does.
pub const MAX_ANNOTATION_NAME_BYTES: usize = 512;

/// The smallest an annotation payload may be, in bytes, and the largest: "Must be between
/// 1 byte and 1 MiB in size" (`PutObjectAnnotationRequest$AnnotationPayload`).
pub const MIN_ANNOTATION_PAYLOAD_BYTES: usize = 1;
/// See [`MIN_ANNOTATION_PAYLOAD_BYTES`].
pub const MAX_ANNOTATION_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The name prefixes S3 reserves for itself, which no caller's annotation name may start
/// with: "Annotation names are UTF-8 encoded and cannot start with `aws` or `s3`
/// (case-insensitive)" (`DeleteObjectAnnotationRequest$AnnotationName`).
pub const RESERVED_ANNOTATION_NAME_PREFIXES: [&str; 2] = ["aws", "s3"];

/// Why an annotation name is not one this service accepts — see
/// [`annotation_name_rejection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationNameRejection {
    /// The name is longer than [`MAX_ANNOTATION_NAME_BYTES`] bytes.
    TooLong { bytes: usize },
    /// The name carries no character at all, under the documented minimum length of 1.
    Empty,
    /// The name begins with a character no annotation name may begin with — see
    /// [`names_begin_with_a_letter_or_a_digit`].
    LeadingCharacter { character: char },
    /// The name carries a character no annotation name may carry at all — see
    /// [`annotation_names_carry`].
    Character { character: char },
    /// The name starts with one of the [`RESERVED_ANNOTATION_NAME_PREFIXES`].
    ReservedPrefix { prefix: &'static str },
}

/// Whether an annotation name may carry `character` ANYWHERE in it.
///
/// An annotation name is spelled out of letters, digits and the separators a namespaced name
/// is written with ([`ANNOTATION_NAME_SEPARATORS`]): "annotation names are UTF-8 encoded"
/// (`DeleteObjectAnnotationRequest$AnnotationName`), so the letter-or-digit test is made over
/// that whole alphabet (`char::is_alphanumeric`) rather than over ASCII, and a name of any
/// script is one this service accepts. Recorded behaviour: a name carrying a symbol — an `!`
/// inside one — is refused rather than stored, so the characters a name is made of are a rule
/// of the namespace and not only of its first position. The model states no `pattern` of its
/// own (`smithy_query(query="shape",shape="AnnotationName")`), which is why the alphabet is
/// stated here, once, for every operation that takes a name or a prefix of one.
pub fn annotation_names_carry(character: char) -> bool {
    character.is_alphanumeric() || ANNOTATION_NAME_SEPARATORS.contains(&character)
}

/// The separators an annotation name is spelled with: the ones a namespaced name of this
/// service's own corpus carries (`team.review.notes`, `my-aws-notes`, `report/2026`,
/// `com.example.pipeline.stage`). Anything else — a symbol, a space, a punctuation mark — is
/// not a character a name of this namespace is written with
/// ([`annotation_names_carry`]).
///
/// The path separator is spelled INSIDE a name only: a name may not OPEN with it
/// ([`ANNOTATION_NAME_OPENING_SEPARATORS`]), which is the recorded `InvalidPrefix` refusal of
/// a listing filtered by a prefix of `/`.
pub const ANNOTATION_NAME_SEPARATORS: [char; 4] = ['-', '_', '.', '/'];

/// Whether an annotation name may begin with `character`.
///
/// An annotation name opens with a LETTER, a DIGIT or one of the separators a namespaced
/// name is spelled with ([`ANNOTATION_NAME_OPENING_SEPARATORS`]). Recorded behaviour: S3
/// refuses an annotation selector that no name of the namespace could begin with — a listing
/// filtered by a prefix of `/` is answered `InvalidPrefix` rather than with an empty page —
/// so a name opening with a path separator, a space or a symbol is not a name this namespace
/// holds. The model states no character rule of its own (`AnnotationName` carries no
/// `pattern` trait), so what is refused here is only what the recording evidences: a leading
/// character outside letters, digits and those separators. The test is made over UTF-8 rather
/// than ASCII because "annotation names are UTF-8 encoded"
/// (`DeleteObjectAnnotationRequest$AnnotationName`): `char::is_alphanumeric` is the
/// letter-or-digit test over that alphabet, so a name opening with a letter of any script is
/// one this service accepts.
///
/// It says nothing about the REST of the name.
pub fn names_begin_with_a_letter_or_a_digit(character: char) -> bool {
    character.is_alphanumeric() || ANNOTATION_NAME_OPENING_SEPARATORS.contains(&character)
}

/// The separators an annotation name may OPEN with beside a letter or a digit: the ones a
/// namespaced name of this service's own corpus is spelled with (`team.review.notes`,
/// `my-aws-notes`, `com.example.pipeline.stage`), which a name may therefore also lead with.
///
/// A path separator is deliberately NOT one of them: that is the leading character the
/// recorded `InvalidPrefix` refusal is about.
pub const ANNOTATION_NAME_OPENING_SEPARATORS: [char; 3] = ['-', '_', '.'];

/// Why `name` is not an annotation name this service accepts, or `None` when it is one.
///
/// ONE place, so a name is valid or invalid wherever it arrives: the two bounds its own
/// member documents (1 to [`MAX_ANNOTATION_NAME_BYTES`] bytes), the prefixes S3 keeps for
/// itself ([`RESERVED_ANNOTATION_NAME_PREFIXES`]) and the characters a name is spelled out of
/// ([`annotation_names_carry`], and [`names_begin_with_a_letter_or_a_digit`] for the first of
/// them). The length is measured in BYTES because that is the unit both the member and the
/// `AnnotationNameTooLong` error state it in, which differs from the character count for any
/// name outside ASCII.
///
/// The ORDER is what the refusals say: each rejection carries its own sentence, and a name
/// under a reserved prefix is refused for the PREFIX however it is spelled after it — the
/// recorded refusal of an `aws:`-prefixed name names the prefix rule rather than the colon in
/// it — so the reserved prefixes are judged before the characters.
pub fn annotation_name_rejection(name: &str) -> Option<AnnotationNameRejection> {
    if name.len() > MAX_ANNOTATION_NAME_BYTES {
        return Some(AnnotationNameRejection::TooLong { bytes: name.len() });
    }
    let Some(first) = name.chars().next() else {
        return Some(AnnotationNameRejection::Empty);
    };
    let reserved = RESERVED_ANNOTATION_NAME_PREFIXES.into_iter().find(|reserved| {
        name.as_bytes()
            .get(..reserved.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(reserved.as_bytes()))
    });
    if let Some(prefix) = reserved {
        return Some(AnnotationNameRejection::ReservedPrefix { prefix });
    }
    if !names_begin_with_a_letter_or_a_digit(first) {
        return Some(AnnotationNameRejection::LeadingCharacter { character: first });
    }
    name.chars()
        .find(|character| !annotation_names_carry(*character))
        .map(|character| AnnotationNameRejection::Character { character })
}

/// How many in-progress multipart uploads one bucket in this emulator keeps.
///
/// NOT a fidelity claim: S3 publishes no limit on concurrent multipart uploads of a
/// bucket, and the model bounds nothing either (`MultipartUploadId` is a plain string,
/// `smithy_query(query="shape",shape="MultipartUploadId")`). It is the emulator's own
/// ceiling, for the same reason as [`MAX_OBJECTS_PER_BUCKET`]: every upload this store
/// records lives in this process's memory, so a caller looping on
/// `CreateMultipartUpload` must not grow it without bound. An initiation past it is
/// refused rather than recorded.
pub const MAX_MULTIPART_UPLOADS_PER_BUCKET: usize = 1_000;

/// How many in-progress multipart uploads this emulator keeps ACROSS every bucket.
///
/// Same reason as [`MAX_OBJECTS_TOTAL`]: the per-bucket ceiling alone would leave the
/// total bounded only by its product with [`MAX_BUCKETS_PER_ACCOUNT`], so this is what
/// bounds the uploads this process holds whatever a caller does.
pub const MAX_MULTIPART_UPLOADS_TOTAL: usize = 10_000;

/// How many RETIRED upload addresses one bucket keeps
/// ([`Bucket::retired_multipart_uploads`]): the `(Key, UploadId)` pairs it held an upload
/// under that a completion or an abort has taken away, which is what makes a repeated abort
/// a no-op instead of a refusal.
///
/// NOT a fidelity claim — S3 publishes no such limit — but the same emulator guard as
/// [`MAX_MULTIPART_UPLOADS_PER_BUCKET`]: a caller looping on initiate-and-abort would
/// otherwise grow this record for the life of the process. As many as the bucket may have in
/// progress at once, so every upload a caller runs through the loop at full width stays
/// distinguishable afterwards.
pub const MAX_RETIRED_MULTIPART_UPLOADS_PER_BUCKET: usize = MAX_MULTIPART_UPLOADS_PER_BUCKET;

/// What [`ComAmazonawsS3Amazons3State::create_multipart_upload`] did, so the caller
/// answers each outcome without re-deriving it — and cannot record an upload without
/// having gone through the key, metadata and count checks, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipartUploadCreation {
    /// The upload was recorded, and is addressed by this id from now on.
    Initiated { upload_id: String },
    /// The key is longer than [`MAX_OBJECT_KEY_BYTES`]; nothing was written.
    KeyTooLong { bytes: usize },
    /// The request's user-defined metadata is larger than
    /// [`MAX_OBJECT_METADATA_BYTES`]; nothing was written.
    MetadataTooLarge { bytes: usize },
    /// The bucket already holds [`MAX_MULTIPART_UPLOADS_PER_BUCKET`] in-progress
    /// uploads; nothing was written.
    UploadCountReached,
    /// This emulator already holds [`MAX_MULTIPART_UPLOADS_TOTAL`] in-progress uploads
    /// across every bucket; nothing was written.
    TotalUploadCountReached,
}

/// The lowest part number a part of a multipart upload may carry.
///
/// From `UploadPartRequest$PartNumber`'s own documentation: "Part number of part being
/// uploaded. This is a positive integer between 1 and 10,000." The model constrains the
/// member not at all (`PartNumber` is a plain integer,
/// `smithy_query(query="shape",shape="PartNumber")`), so nothing refuses a number outside
/// the published range before a handler does.
pub const MIN_PART_NUMBER: i32 = 1;

/// The highest part number a part of a multipart upload may carry — the other end of the
/// range [`MIN_PART_NUMBER`] opens, and so also the most parts one upload can hold, since
/// "a part number uniquely identifies a part" (`UploadPart`'s own documentation).
pub const MAX_PART_NUMBER: i32 = 10_000;

/// The fewest bytes a part of a multipart upload may carry, EXCEPT the last one.
///
/// The published multipart upload limit: "Part size: 5 MiB to 5 GiB. There is no minimum
/// size limit on the last part of your multipart upload" (Amazon S3 User Guide, *Amazon S3
/// multipart upload limits*). Nothing refuses a smaller part when it is UPLOADED — a part
/// is only known to be the last one once a completion names the parts to assemble — so this
/// is what a completion judges every part but the final one against.
pub const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// What [`ComAmazonawsS3Amazons3State::upload_part`] did, so the caller answers each
/// outcome without re-deriving it — and cannot record a part without having gone through
/// the byte budgets, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartUpload {
    /// The part was recorded against the upload. `replaced` is whether the upload already
    /// held a part of that number: "If you upload a new part using the same part number
    /// that was used with a previous part, the previously uploaded part is overwritten"
    /// (`UploadPart`'s own documentation).
    Stored { replaced: bool },
    /// The bucket has no such upload in progress under that id and key; nothing was
    /// written.
    NoSuchUpload,
    /// Recording the part would leave the bucket holding more than
    /// [`MAX_OBJECT_BYTES_PER_BUCKET`] bytes; nothing was written.
    ByteBudgetReached { would_hold: usize },
    /// Recording the part would leave this emulator holding more than
    /// [`MAX_OBJECT_BYTES_TOTAL`] bytes across every bucket; nothing was written.
    TotalByteBudgetReached { would_hold: usize },
}

/// What [`ComAmazonawsS3Amazons3State::abort_multipart_upload`] did, so the caller answers
/// each outcome without re-deriving it — and cannot discard an upload without having
/// addressed it by both its id and its key, the way every other reach for an upload does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadAbort {
    /// The upload was discarded with every part it held: "After a multipart upload is
    /// aborted, no additional parts can be uploaded using that upload ID. The storage
    /// consumed by any previously uploaded parts will be freed"
    /// (`AbortMultipartUpload`'s own documentation).
    Discarded,
    /// The address HELD an upload that is already gone — a completion or an earlier abort
    /// took it away ([`Bucket::retired_multipart_uploads`]) — so there was nothing left to
    /// discard and the state the abort asks for already holds.
    AlreadyDiscarded,
    /// The bucket never had an upload in progress under that id and key, so the id names no
    /// upload of this bucket at all and nothing was removed.
    NoSuchUpload,
}

/// What [`ComAmazonawsS3Amazons3State::complete_multipart_upload`] did, so the caller
/// answers each outcome without re-deriving it — and cannot turn an upload into an object
/// without having gone through the object-write ceilings, per bucket AND service-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadCompletion {
    /// The parts became the object at the upload's key, and the upload is no longer in
    /// progress: "after you successfully complete … a multipart upload, you can no longer
    /// use the upload ID" (`ListParts`'s own documentation). The write is what the object
    /// write did — [`ObjectWrite::Created`] or [`ObjectWrite::Replaced`].
    Completed(ObjectWrite),
    /// The bucket has no such upload in progress under that id and key — never initiated,
    /// or already completed or aborted — so nothing was written.
    NoSuchUpload,
    /// The object write the completion performs was refused (an over-long key, or one of
    /// the ceilings), so nothing was written and the upload is left in progress exactly as
    /// it was, with every part it held.
    Refused(ObjectWrite),
}

/// How many tags one bucket's tag set may carry.
///
/// The Amazon S3 User Guide's bucket tagging restrictions ("Using cost allocation S3
/// bucket tags", the page `PutBucketTagging`'s own `InvalidTag` documentation points at)
/// publish three limits on a bucket tag set: at most 50 tags per bucket, a key of at most
/// 128 Unicode characters and a value of at most 256. A tag set past any of them is
/// refused rather than stored, which is what bounds what one bucket entry can hold.
pub const MAX_TAGS_PER_BUCKET: usize = 50;

/// How many tags one OBJECT's tag set may carry.
///
/// `PutObjectTagging`'s own documentation publishes this one: "Note that Amazon S3 limits
/// the maximum number of tags to 10 tags per object." The character bounds below are the
/// same for both tag sets (the "Tag Restrictions" page that operation points at), so the
/// count is the only published limit that differs between a bucket's tag set and an
/// object's.
pub const MAX_TAGS_PER_OBJECT: usize = 10;

/// The longest tag key a tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAGS_PER_BUCKET`], and the same bound for an object's tag set. The
/// model constrains the other end of the key — `Tag$Key` targets `ObjectKey`,
/// `length {min 1}` (`smithy_query(query="shape",shape="Tag")`) — so an empty key is
/// already refused at the boundary, before any handler runs.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value a tag set may carry, in Unicode characters.
///
/// Same source as [`MAX_TAG_KEY_CHARS`]. `Tag$Value` targets the unconstrained `Value`
/// string, so nothing refuses an over-long value before this check.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The first key more than one of `tags` names, or `None` when each names its own.
///
/// A tag predicate matches ONE key to one value, so two predicates on the same key can never
/// both hold. ONE reading for every member that takes a list of tag predicates — a lifecycle
/// rule's filter and an S3 Intelligent-Tiering filter's conjunction among them — so two
/// configuration families cannot drift apart on which tag sets they admit; each family keeps
/// its own refusal shape.
pub fn first_repeated_tag_key(tags: &[crate::ops::types::Tag]) -> Option<&str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    tags.iter()
        .find(|tag| !seen.insert(tag.key.as_str()))
        .map(|tag| tag.key.as_str())
}

/// The punctuation a tag key or value may carry, beside Unicode letters, digits and
/// whitespace.
///
/// The published tag restrictions the two tagging operations point at ("Tag restrictions",
/// Amazon S3 User Guide, and the same list in the AWS *Tagging Resources* guide): "the
/// allowed characters are letters, whitespace, and numbers representable in UTF-8, plus
/// the following special characters: + - = . _ : / @". A code point outside that set — an
/// emoji, a currency sign, a bracket — is refused rather than stored, which is what
/// [`tag_text_is_allowed`] decides.
pub const TAG_PUNCTUATION: [char; 8] = ['+', '-', '=', '.', '_', ':', '/', '@'];

/// Whether every character of `text` is one a tag key or value may carry.
///
/// ONE character class for both halves of a tag and for both tag sets S3 holds, from the
/// published restrictions quoted on [`TAG_PUNCTUATION`]: Unicode letters, Unicode digits,
/// whitespace, and that punctuation. "Letters" is the Unicode property, not ASCII, so a
/// tag written in any script is storable; a SYMBOL (an emoji is one) is not a letter and
/// is refused. Stated once, by the function that finds the offending character.
pub fn tag_text_is_allowed(text: &str) -> bool {
    first_disallowed_tag_char(text).is_none()
}

/// The first character of `text` that a tag key or value may NOT carry, or `None` when
/// every one of them is allowed.
///
/// The one place the class is spelled; [`tag_text_is_allowed`] is this answering "none".
pub fn first_disallowed_tag_char(text: &str) -> Option<char> {
    text.chars().find(|character| {
        !(character.is_alphabetic()
            || character.is_numeric()
            || character.is_whitespace()
            || TAG_PUNCTUATION.contains(character))
    })
}

/// One tag on a resource this emulator holds — a bucket's tag set or an object's: the
/// key/value pair `Tag` carries on the wire.
///
/// ONE type for both, because a tag is the same thing in both places (`Tagging$TagSet` is
/// the payload of `PutBucketTagging` and of `PutObjectTagging` alike). Held separately
/// from the modelled `Tag` shape so this store owns no wire type, the way [`Bucket`]
/// holds the Region as a plain `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceTag {
    /// The tag's key — unique within one resource's tag set.
    pub key: String,
    /// The tag's value.
    pub value: String,
}

/// How many bytes a tag set retains: every key and value, in UTF-8 bytes.
///
/// What the object byte budgets count a tag set as — see [`Object::retained_bytes`].
pub fn tag_set_bytes(tags: &[ResourceTag]) -> usize {
    tags.iter()
        .map(|tag| tag.key.len() + tag.value.len())
        .sum()
}

/// The published limits one resource's tag set is judged against.
///
/// The character bounds are the same for every tag set, so what a scope carries is the tag
/// COUNT its resource publishes plus the noun a refusal names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagSetLimits {
    /// The most tags this resource's tag set may carry.
    pub max_tags: usize,
    /// What the resource is called in a refusal ("bucket", "object").
    pub resource: &'static str,
    /// The `<Message>` this resource's tagging path states for a key in the namespace AWS
    /// keeps for its own tags — see [`RESERVED_TAG_KEY_PREFIX`].
    ///
    /// Part of the limits rather than of the cause because the two tagging paths word that
    /// one refusal differently and neither sentence is interchangeable with the other: a
    /// BUCKET tag set is refused as a write of a system tag
    /// ([`SYSTEM_TAGS_NOT_BY_REQUESTER_MESSAGE`]), an OBJECT tag set as a key carrying the
    /// reserved prefix ([`RESERVED_TAG_KEY_PREFIX_MESSAGE`]).
    pub reserved_key_prefix_message: &'static str,
}

/// The limits a BUCKET's tag set is judged against ([`MAX_TAGS_PER_BUCKET`]).
pub const BUCKET_TAG_SET_LIMITS: TagSetLimits = TagSetLimits {
    max_tags: MAX_TAGS_PER_BUCKET,
    resource: "bucket",
    reserved_key_prefix_message: SYSTEM_TAGS_NOT_BY_REQUESTER_MESSAGE,
};

/// The limits an OBJECT's tag set is judged against ([`MAX_TAGS_PER_OBJECT`]).
pub const OBJECT_TAG_SET_LIMITS: TagSetLimits = TagSetLimits {
    max_tags: MAX_TAGS_PER_OBJECT,
    resource: "object",
    reserved_key_prefix_message: RESERVED_TAG_KEY_PREFIX_MESSAGE,
};

/// Which published restriction a tag set breaks.
///
/// Each variant is one of the published restrictions on a tag set, or the key-uniqueness
/// the set itself has. AWS answers every one of them with `InvalidTag`, which this
/// service's model does not declare as a shape
/// (`smithy_query(query="operation",operation="PutBucketTagging")` and the same query for
/// `PutObjectTagging` both report `errors: []`), so the caller renders it through the
/// framework's refusal channel — never as a success and never as a 501.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagSetRejectionCause {
    /// The tag set carries more tags than its resource's [`TagSetLimits::max_tags`].
    TooManyTags { count: usize },
    /// A tag carries an EMPTY key. `Tag$Key` targets `ObjectKey`, `length {min 1}`
    /// (`smithy_query(query="shape",shape="Tag")`), so the modelled `Tagging` container
    /// can never carry one — but a tag set that arrives as the URL-encoded
    /// `x-amz-tagging` header is not a modelled `Tag` list, so this is where such a pair
    /// is caught (see [`tag_set_from_header`]).
    EmptyKey,
    /// A key is longer than [`MAX_TAG_KEY_CHARS`] Unicode characters.
    KeyTooLong { chars: usize },
    /// A value is longer than [`MAX_TAG_VALUE_CHARS`] Unicode characters.
    ValueTooLong { key: String, chars: usize },
    /// Two tags in one set name the same key, so the set is not a key/value mapping.
    DuplicateKey { key: String },
    /// A key begins with the prefix AWS keeps for the tags it generates itself
    /// ([`RESERVED_TAG_KEY_PREFIX`]), which a caller may therefore not write.
    ReservedKeyPrefix { key: String },
    /// A key or a value carries a character outside the published class every tag set
    /// shares ([`tag_text_is_allowed`]).
    DisallowedCharacter {
        /// Which half of the tag carries it ([`TAG_KEY_FIELD`], [`TAG_VALUE_FIELD`]).
        field: &'static str,
        /// The character the published class does not allow.
        character: char,
    },
}

/// Why a tag set cannot be stored on a resource: the restriction it breaks, the limits it
/// was judged against — so the refusal names the resource and the bound — and the TAG the
/// refusal is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSetRejection {
    /// The limits the set was judged against.
    pub limits: TagSetLimits,
    /// The restriction it breaks.
    pub cause: TagSetRejectionCause,
    /// The tag the restriction refused, or `None` for a restriction on the SET rather than
    /// on one of its tags.
    ///
    /// What the error document names the refusal by ([`TagSetRejection::tag_key`] and
    /// [`TagSetRejection::tag_value`] read it), because every sentence S3 states for a tag
    /// is fixed text that names no tag: the envelope is the only place a refused set says
    /// WHICH of its tags the caller must change.
    pub tag: Option<ResourceTag>,
}

impl std::fmt::Display for TagSetRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let TagSetLimits {
            max_tags,
            resource,
            reserved_key_prefix_message,
        } = self.limits;
        match &self.cause {
            // The resource's noun capitalised, and the published maximum the set was
            // judged against: both come from the limits, never from the request — the
            // count the set carried is not in the sentence.
            TagSetRejectionCause::TooManyTags { .. } => {
                let mut characters = resource.chars();
                let noun: String = match characters.next() {
                    Some(first) => first.to_uppercase().chain(characters).collect(),
                    None => String::new(),
                };
                write!(f, "{noun} tags cannot be greater than {max_tags}")
            }
            TagSetRejectionCause::EmptyKey | TagSetRejectionCause::KeyTooLong { .. } => {
                write!(f, "{INVALID_TAG_KEY_MESSAGE}")
            }
            TagSetRejectionCause::ValueTooLong { .. } => {
                write!(f, "{INVALID_TAG_VALUE_MESSAGE}")
            }
            // Which half of the tag carried the character is what decides which of the two
            // sentences S3 states; the character itself is not in either of them.
            TagSetRejectionCause::DisallowedCharacter { field, .. } => {
                let message = if *field == TAG_KEY_FIELD {
                    INVALID_TAG_KEY_MESSAGE
                } else {
                    INVALID_TAG_VALUE_MESSAGE
                };
                write!(f, "{message}")
            }
            TagSetRejectionCause::DuplicateKey { .. } => {
                write!(f, "Cannot provide multiple Tags with the same key")
            }
            // The sentence this resource's tagging path states for the reservation, which
            // is fixed text: the key that carried the prefix is a variable part of the
            // ENVELOPE ([`TagSetRejection::tag_key`]) rather than of the message.
            TagSetRejectionCause::ReservedKeyPrefix { .. } => {
                write!(f, "{reserved_key_prefix_message}")
            }
        }
    }
}

/// Which half of a tag [`TagSetRejectionCause::DisallowedCharacter`] names, for the KEY.
pub const TAG_KEY_FIELD: &str = "Tag.Key";

/// Which half of a tag [`TagSetRejectionCause::DisallowedCharacter`] names, for the VALUE.
pub const TAG_VALUE_FIELD: &str = "Tag.Value";

/// The `<Message>` S3 sends for a tag whose KEY breaks one of the published restrictions —
/// its length or the character class — and for a tag that carries no key at all.
///
/// Fixed text: S3 states which HALF of the tag was not valid and nothing else, interpolating
/// neither the key, the bound it broke nor the character class.
const INVALID_TAG_KEY_MESSAGE: &str = "The TagKey you have provided is invalid";

/// The `<Message>` S3 sends for a tag whose VALUE breaks one of those restrictions — the
/// value half of [`INVALID_TAG_KEY_MESSAGE`], worded the same way.
const INVALID_TAG_VALUE_MESSAGE: &str = "The TagValue you have provided is invalid";

/// The prefix AWS keeps for the tags it generates itself, and which a caller's tag key may
/// therefore not begin with: "the `aws:` prefix is reserved for Amazon Web Services use",
/// and "you can't edit, delete, or add tags with the `aws:` prefix" (AWS *Tagging your
/// resources*, *Tag restrictions* — the same list both S3 tagging operations point at).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The `<Message>` S3's OBJECT tagging path sends for a tag key beginning with
/// [`RESERVED_TAG_KEY_PREFIX`].
///
/// Fixed text: S3 states the reservation and nothing else; the offending key is a member of
/// the error document rather than part of this sentence.
const RESERVED_TAG_KEY_PREFIX_MESSAGE: &str = "Your TagKey cannot be prefixed with aws:";

/// The `<Message>` S3's BUCKET tagging path sends for the same reservation — the other
/// wording of that one refusal, stated about the TAGS rather than about the key's spelling:
/// a key in the reserved namespace names a tag the service generates, and a requester may
/// not write one.
///
/// Also fixed text, with no variable part: it names neither the key, the prefix nor the
/// bucket.
const SYSTEM_TAGS_NOT_BY_REQUESTER_MESSAGE: &str =
    "System tags cannot be added/updated by requester";

impl TagSetRejection {
    /// The wire error code S3 answers this rejection with.
    ///
    /// Two codes, one per class of restriction, both HTTP 400: a tag set carrying more
    /// tags than its resource may hold is refused `BadRequest` — the set is too big,
    /// whatever is in it — while a restriction one INDIVIDUAL tag breaks (its key, its
    /// value, or the set naming that key twice) is refused `InvalidTag`, "the tag provided
    /// was not a valid tag. This error can occur if the tag did not pass input validation"
    /// (`PutBucketTagging`'s own list of special errors). Neither code is a shape in this
    /// model, so a caller renders it through the unmodelled channel.
    pub fn code(&self) -> &'static str {
        match self.cause {
            TagSetRejectionCause::TooManyTags { .. } => BAD_REQUEST_CODE,
            TagSetRejectionCause::EmptyKey
            | TagSetRejectionCause::KeyTooLong { .. }
            | TagSetRejectionCause::ValueTooLong { .. }
            | TagSetRejectionCause::DuplicateKey { .. }
            | TagSetRejectionCause::ReservedKeyPrefix { .. }
            | TagSetRejectionCause::DisallowedCharacter { .. } => INVALID_TAG_CODE,
        }
    }

    /// The tag KEY this rejection's error document names, or `None` for a rejection whose
    /// document names none.
    ///
    /// S3's envelope for a refusal about ONE tag is
    /// `<Error><Code/><Message/><TagKey/><TagValue/><RequestId/><HostId/></Error>`: the
    /// code and the message are the fixed text of the class — each of them states which
    /// RESTRICTION the tag broke and names no tag — so the members between them are the
    /// variable part, copied from the request, and the only place the refusal says WHICH
    /// tag of the set it is about. The caller renders them through the naming channel, so
    /// no handler decides them for itself.
    ///
    /// A rejection about the SET rather than about one of its tags — its size — names no
    /// tag, and neither does one about a tag that carries no key at all: there is nothing
    /// for the document to name it by.
    pub fn tag_key(&self) -> Option<&str> {
        self.tag
            .as_ref()
            .map(|tag| tag.key.as_str())
            .filter(|key| !key.is_empty())
    }

    /// The tag VALUE that same document names, for a rejection about the value rather than
    /// about the key — the other half of the pair the request carried, written after
    /// [`TagSetRejection::tag_key`].
    ///
    /// Only a restriction the VALUE breaks names it: a key the published bounds refuse, a
    /// key in the reserved namespace and a key the set names twice are all about the key,
    /// and the value the request happened to pair with it is no part of why they were
    /// refused.
    pub fn tag_value(&self) -> Option<&str> {
        let refused_value = match &self.cause {
            TagSetRejectionCause::ValueTooLong { .. } => true,
            TagSetRejectionCause::DisallowedCharacter { field, .. } => *field == TAG_VALUE_FIELD,
            TagSetRejectionCause::TooManyTags { .. }
            | TagSetRejectionCause::EmptyKey
            | TagSetRejectionCause::KeyTooLong { .. }
            | TagSetRejectionCause::DuplicateKey { .. }
            | TagSetRejectionCause::ReservedKeyPrefix { .. } => false,
        };
        refused_value
            .then(|| self.tag.as_ref().map(|tag| tag.value.as_str()))
            .flatten()
    }
}

/// The code S3 answers a tag set whose SIZE is past what the resource may hold:
/// "BadRequest", HTTP 400 (Amazon S3 "Error responses", list of error codes).
pub const BAD_REQUEST_CODE: &str = "BadRequest";

/// The code S3 answers a tag that does not pass input validation: "InvalidTag - The tag
/// provided was not a valid tag. This error can occur if the tag did not pass input
/// validation" (`PutBucketTagging`'s own list of special errors), HTTP 400.
pub const INVALID_TAG_CODE: &str = "InvalidTag";

/// The HTTP status both tag-set codes are rendered with: 400, from that same list.
pub const TAG_REJECTION_STATUS: u16 = 400;

/// Whether `tags` may be stored as the tag set of a resource with these `limits`,
/// reporting the first restriction it breaks.
///
/// ONE rule for every tag set S3 holds: the count against the resource's own published
/// maximum, then each key and value against the character bounds every tag set shares, then
/// each key against the prefix AWS keeps for the tags it generates itself
/// ([`RESERVED_TAG_KEY_PREFIX`]), and the key-uniqueness the set itself has. The count is
/// checked before the per-tag rules so an enormous set is refused for what it is rather than
/// for whichever of its tags happens to come first, and a key's own bounds are checked
/// before the reservation so the key the refusal NAMES is one within those bounds.
///
/// The reservation is the one rule of the set that depends on WHO stated the key: a tag in
/// that namespace is AWS's own, so a requester may not ADD or UPDATE one, while the AWS
/// services that own such a tag set it from inside AWS through no public API
/// ([`TagKeyOrigin`]). A requester's set that carries such a key with the value the resource
/// ALREADY holds for it neither adds nor updates it — which is why `in_force`, the tag set
/// this one replaces, is part of the rule: a read-modify-write of a resource AWS has tagged
/// itself states those tags back unchanged, and refusing it would leave such a resource's
/// own tags unwritable. Every other restriction here is the set's own and holds whichever
/// path it arrived on.
pub fn validate_tag_set(
    tags: &[ResourceTag],
    limits: TagSetLimits,
    origin: TagKeyOrigin,
    in_force: &[ResourceTag],
) -> Result<(), TagSetRejection> {
    // A restriction on the SET names no tag; one a single tag breaks carries that tag, so
    // the error document can name it.
    let refuse_set = |cause| {
        Err(TagSetRejection {
            limits,
            cause,
            tag: None,
        })
    };
    let refuse_tag = |cause, tag: &ResourceTag| {
        Err(TagSetRejection {
            limits,
            cause,
            tag: Some(tag.clone()),
        })
    };
    if tags.len() > limits.max_tags {
        return refuse_set(TagSetRejectionCause::TooManyTags { count: tags.len() });
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for tag in tags {
        if tag.key.is_empty() {
            return refuse_tag(TagSetRejectionCause::EmptyKey, tag);
        }
        let key_chars = tag.key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
            return refuse_tag(TagSetRejectionCause::KeyTooLong { chars: key_chars }, tag);
        }
        if let Some(character) = first_disallowed_tag_char(&tag.key) {
            return refuse_tag(
                TagSetRejectionCause::DisallowedCharacter {
                    field: TAG_KEY_FIELD,
                    character,
                },
                tag,
            );
        }
        // Judged after the key's own bounds, so the key this refusal names in the error
        // document is one those bounds admit — and only for a key a REQUESTER states, since
        // the namespace is AWS's to write in, and only where stating it would ADD or UPDATE
        // the tag: a key already in force with this very value is neither.
        if origin == TagKeyOrigin::Caller
            && tag.key.starts_with(RESERVED_TAG_KEY_PREFIX)
            && !carries_tag_unchanged(in_force, tag)
        {
            return refuse_tag(
                TagSetRejectionCause::ReservedKeyPrefix {
                    key: tag.key.clone(),
                },
                tag,
            );
        }
        let value_chars = tag.value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return refuse_tag(
                TagSetRejectionCause::ValueTooLong {
                    key: tag.key.clone(),
                    chars: value_chars,
                },
                tag,
            );
        }
        if let Some(character) = first_disallowed_tag_char(&tag.value) {
            return refuse_tag(
                TagSetRejectionCause::DisallowedCharacter {
                    field: TAG_VALUE_FIELD,
                    character,
                },
                tag,
            );
        }
        if !seen.insert(tag.key.as_str()) {
            return refuse_tag(
                TagSetRejectionCause::DuplicateKey {
                    key: tag.key.clone(),
                },
                tag,
            );
        }
    }
    Ok(())
}

/// Whether `tag` is already in force with exactly this value, so a set that carries it
/// leaves it as it is.
///
/// What the reservation on [`RESERVED_TAG_KEY_PREFIX`] forbids a requester is adding a tag
/// in that namespace and updating one; a value identical to the one in force does neither.
///
/// It decides what a set the request CARRIES may hold, and nothing about a key the set
/// omits: whether replacing a tag set with one that leaves out a key in that namespace is
/// the deletion the reservation also forbids is a separate rule of this surface, which this
/// emulator has no recorded answer for and which no caller here states.
fn carries_tag_unchanged(in_force: &[ResourceTag], tag: &ResourceTag) -> bool {
    in_force
        .iter()
        .any(|held| held.key == tag.key && held.value == tag.value)
}

/// The tag set a write's `x-amz-tagging` header names, or which published restriction it
/// breaks — the ONE seam a handler uses, so no handler can store a header-delivered tag
/// set the limits would have refused.
///
/// "The tag-set for the object. The tag-set must be encoded as URL Query parameters. (For
/// example, "Key1=Value1")" — `PutObjectRequest$Tagging`, and the same sentence on
/// `CopyObjectRequest$Tagging` and `CreateMultipartUploadRequest$Tagging`. So the header
/// is an `application/x-www-form-urlencoded` string, read with the runtime's own parser
/// for that grammar (`aws_smithy_schema_server::query::parse_form_urlencoded`, which the
/// query-protocol services already read their bodies with): each `&`-separated pair is
/// one tag, percent escapes and `+` are decoded, and a pair with no `=` is a key with an
/// empty value. An EMPTY header carries no pairs and so is an empty tag set — the same
/// state as no tag set at all.
///
/// The PAIR COUNT is judged BEFORE the set is materialised, by counting the `&`-separated
/// pairs the same grammar defines — which allocates nothing — so a header naming more
/// pairs than the resource may hold is refused for exactly that without this process ever
/// holding a tag set it would never store. What is then materialised is at most
/// `limits.max_tags` tags, each judged by [`validate_tag_set`] like any other tag set.
pub fn tag_set_from_header(
    header: &str,
    limits: TagSetLimits,
    origin: TagKeyOrigin,
    in_force: &[ResourceTag],
) -> Result<Vec<ResourceTag>, TagSetRejection> {
    let pairs = header.split('&').filter(|pair| !pair.is_empty()).count();
    if pairs > limits.max_tags {
        return Err(TagSetRejection {
            limits,
            cause: TagSetRejectionCause::TooManyTags { count: pairs },
            tag: None,
        });
    }
    let tags: Vec<ResourceTag> =
        ::aws_smithy_schema_server::query::parse_form_urlencoded(header.as_bytes())
            .into_iter()
            .map(|(key, value)| ResourceTag { key, value })
            .collect();
    validate_tag_set(&tags, limits, origin, in_force)?;
    Ok(tags)
}

// ---------------------------------------------------------------------------
// Access control lists.
//
// ONE type for every ACL S3 holds, the way `ResourceTag` is one tag for every tag set:
// an ACL is "a list of grants" (`AccessControlPolicy$Grants`) in a bucket's record and
// in an object's alike, so the grant, the grantee and the rules they are judged against
// live here once rather than in each handler.
// ---------------------------------------------------------------------------

/// Every value the `Permission` enum publishes: the access an ACL grant may give, and so
/// the only values a grant may name. Re-exported from the model-generated value list,
/// like [`EVENT_VALUES`].
pub use crate::ops::types::PERMISSION_VALUES;

/// Every value the `Type` enum publishes: how an ACL grantee may be named. Re-exported
/// from the model-generated value list, like [`PERMISSION_VALUES`].
pub use crate::ops::types::TYPE_VALUES;

/// Every value the `ObjectCannedACL` enum publishes: the canned ACLs an OBJECT may be
/// given, which [`object_canned_acl_grants`] expands. Re-exported from the
/// model-generated value list, like [`PERMISSION_VALUES`].
pub use crate::ops::types::OBJECT_CANNED_A_C_L_VALUES;

/// The `Grantee$Type` of a grantee named by its canonical user id — `TYPE_VALUES[1]`.
pub const GRANTEE_CANONICAL_USER: &str = "CanonicalUser";

/// The `Grantee$Type` of a grantee named by a predefined group's URI — `TYPE_VALUES[2]`.
pub const GRANTEE_GROUP: &str = "Group";

/// The `Grantee$Type` of a grantee named by the email address of an account —
/// `TYPE_VALUES[0]`.
pub const GRANTEE_BY_EMAIL: &str = "AmazonCustomerByEmail";

/// `Permission` `FULL_CONTROL`: "the read, write, read ACP, and write ACP permissions"
/// (`PutBucketAclRequest$GrantFullControl`).
pub const PERMISSION_FULL_CONTROL: &str = "FULL_CONTROL";
/// `Permission` `READ`: "Allows grantee to list the objects in the bucket"
/// (`PutBucketAclRequest$GrantRead`).
pub const PERMISSION_READ: &str = "READ";
/// `Permission` `READ_ACP`: "Allows grantee to read the bucket ACL"
/// (`PutBucketAclRequest$GrantReadACP`).
pub const PERMISSION_READ_ACP: &str = "READ_ACP";
/// `Permission` `WRITE`: "Allows grantee to create new objects in the bucket"
/// (`PutBucketAclRequest$GrantWrite`).
pub const PERMISSION_WRITE: &str = "WRITE";
/// `Permission` `WRITE_ACP`: "Allows grantee to write the ACL for the applicable bucket"
/// (`PutBucketAclRequest$GrantWriteACP`).
pub const PERMISSION_WRITE_ACP: &str = "WRITE_ACP";

/// The URI of the predefined group *All Users* — "anyone in the world", which is what
/// makes a grant to it a PUBLIC grant.
///
/// The three URIs below are the predefined groups the Amazon S3 User Guide publishes
/// ("Amazon S3 predefined groups"), and they are the only URIs an ACL grantee of type
/// `Group` may name.
pub const ALL_USERS_GROUP_URI: &str = "http://acs.amazonaws.com/groups/global/AllUsers";

/// The URI of the predefined group *Authenticated Users* — every AWS account.
pub const AUTHENTICATED_USERS_GROUP_URI: &str =
    "http://acs.amazonaws.com/groups/global/AuthenticatedUsers";

/// The URI of the predefined group *Log Delivery* — the group S3 server access logging
/// and other AWS log producers write through, so a grant to it is what lets logs land in
/// the bucket.
pub const LOG_DELIVERY_GROUP_URI: &str = "http://acs.amazonaws.com/groups/s3/LogDelivery";

/// Every predefined group URI an ACL grantee of type [`GRANTEE_GROUP`] may name. See
/// [`ALL_USERS_GROUP_URI`] for the source.
pub const PREDEFINED_GROUP_URIS: [&str; 3] = [
    ALL_USERS_GROUP_URI,
    AUTHENTICATED_USERS_GROUP_URI,
    LOG_DELIVERY_GROUP_URI,
];

/// The Regions in which S3 has discontinued Email Grantee ACLs.
///
/// From `PutBucketAcl`'s own documentation: "As of October 1, 2025, Amazon S3 has
/// discontinued support for Email Grantee Access Control Lists (ACLs). If you attempt to
/// use an Email Grantee ACL in a request after October 1, 2025, the request will receive
/// an HTTP 405 (Method Not Allowed) error. This change affects the following Amazon Web
/// Services Regions: US East (N. Virginia), US West (N. California), US West (Oregon),
/// Asia Pacific (Singapore), Asia Pacific (Sydney), Asia Pacific (Tokyo), Europe
/// (Ireland), and South America (São Paulo)" — those eight Regions by their codes.
pub const EMAIL_GRANTEE_DISCONTINUED_REGIONS: [&str; 8] = [
    "us-east-1",
    "us-west-1",
    "us-west-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-northeast-1",
    "eu-west-1",
    "sa-east-1",
];

/// How many grants one ACL may carry.
///
/// Published, unlike [`MAX_OWNERSHIP_CONTROLS_RULES`]: "An ACL can have up to 100 grants"
/// (Amazon S3 User Guide, *Access control list (ACL) overview*). Nothing in the model
/// bounds `Grants` (`smithy_query(query="shape",shape="Grants")` carries no `@length`), so
/// this is also what bounds the grants one bucket can park in this process's memory; an
/// ACL past it is refused rather than recorded.
pub const MAX_ACL_GRANTS: usize = 100;

/// Who an ACL grant gives its permission to: the `Grantee` of one `Grant`.
///
/// Which member carries the grantee depends on [`Self::grantee_type`], exactly as the
/// wire shape does (`smithy_query(query="shape",shape="Grantee")`): a `CanonicalUser` is
/// named by `ID`, a `Group` by `URI` and an `AmazonCustomerByEmail` by `EmailAddress`.
/// Held separately from the modelled `Grantee` shape so this store owns no wire type, the
/// way [`ResourceTag`] is held separately from `Tag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclGrantee {
    /// A `Type` value ([`TYPE_VALUES`]): [`GRANTEE_CANONICAL_USER`], [`GRANTEE_GROUP`]
    /// or [`GRANTEE_BY_EMAIL`].
    pub grantee_type: String,
    /// `Grantee$ID`, the canonical user id of the grantee — present exactly when the
    /// type is [`GRANTEE_CANONICAL_USER`].
    pub id: Option<String>,
    /// `Grantee$URI`, the predefined group's URI — present exactly when the type is
    /// [`GRANTEE_GROUP`], and then one of [`PREDEFINED_GROUP_URIS`].
    pub uri: Option<String>,
    /// `Grantee$EmailAddress` — present exactly when the type is [`GRANTEE_BY_EMAIL`].
    pub email_address: Option<String>,
}

impl AclGrantee {
    /// The grantee that is the account whose canonical user id is `id`.
    pub fn canonical_user(id: impl Into<String>) -> Self {
        Self {
            grantee_type: GRANTEE_CANONICAL_USER.to_string(),
            id: Some(id.into()),
            uri: None,
            email_address: None,
        }
    }

    /// The grantee that is the predefined group named by `uri`.
    pub fn group(uri: impl Into<String>) -> Self {
        Self {
            grantee_type: GRANTEE_GROUP.to_string(),
            id: None,
            uri: Some(uri.into()),
            email_address: None,
        }
    }
}

/// One grant of an ACL: who, and what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclGrant {
    /// Who the permission is given to.
    pub grantee: AclGrantee,
    /// A `Permission` value ([`PERMISSION_VALUES`]).
    pub permission: String,
}

impl AclGrant {
    /// The grant giving `permission` to the account whose canonical user id is `id`.
    pub fn to_canonical_user(id: impl Into<String>, permission: &str) -> Self {
        Self {
            grantee: AclGrantee::canonical_user(id),
            permission: permission.to_string(),
        }
    }

    /// The grant giving `permission` to the predefined group named by `uri`.
    pub fn to_group(uri: &str, permission: &str) -> Self {
        Self {
            grantee: AclGrantee::group(uri),
            permission: permission.to_string(),
        }
    }
}

/// The ACL a bucket carries until a `PutBucketAcl` sets another one: the owner's
/// `FULL_CONTROL` and nothing else.
///
/// The canned ACL `private` — "Owner gets FULL_CONTROL. No one else has access rights
/// (default)" (Amazon S3 User Guide, *Canned ACL*) — which is the ACL `CreateBucket`
/// applies when its request names none (`CreateBucketRequest$ACL` defaults to nothing,
/// and S3 documents `private` as the default canned ACL). Derived on read rather than
/// written at create time, so a reader reports it without changing anything.
pub fn default_acl_grants() -> Vec<AclGrant> {
    vec![AclGrant::to_canonical_user(
        canonical_user_id(),
        PERMISSION_FULL_CONTROL,
    )]
}

/// The grants a `BucketCannedACL` value stands for, or `None` for a value outside
/// `BUCKET_CANNED_A_C_L_VALUES`.
///
/// "Amazon S3 supports a set of predefined ACLs, known as canned ACLs. Each canned ACL has
/// a predefined set of grantees and permissions" (`PutBucketAcl`'s own documentation); the
/// four expansions are the bucket rows of the published *Canned ACL* table:
///
/// * `private` — owner `FULL_CONTROL`, nobody else;
/// * `public-read` — owner `FULL_CONTROL`, `AllUsers` `READ`;
/// * `public-read-write` — owner `FULL_CONTROL`, `AllUsers` `READ` and `WRITE`;
/// * `authenticated-read` — owner `FULL_CONTROL`, `AuthenticatedUsers` `READ`;
/// * `log-delivery-write` — owner `FULL_CONTROL`, `LogDelivery` `WRITE` and `READ_ACP`.
pub fn canned_acl_grants(canned: &str) -> Option<Vec<AclGrant>> {
    let owner = AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_FULL_CONTROL);
    let grants = match canned {
        "private" => vec![owner],
        "public-read" => vec![
            owner,
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_READ),
        ],
        "public-read-write" => vec![
            owner,
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_READ),
            AclGrant::to_group(ALL_USERS_GROUP_URI, PERMISSION_WRITE),
        ],
        "authenticated-read" => vec![
            owner,
            AclGrant::to_group(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
        ],
        // The fifth bucket row of that same table: "log-delivery-write — the LogDelivery
        // group gets WRITE and READ_ACP permissions on the bucket", which is the ACL S3
        // server access logging requires on a target bucket.
        "log-delivery-write" => vec![
            owner,
            AclGrant::to_group(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE),
            AclGrant::to_group(LOG_DELIVERY_GROUP_URI, PERMISSION_READ_ACP),
        ],
        _ => return None,
    };
    Some(grants)
}

/// The grants an `ObjectCannedACL` value stands for, or `None` for a value outside
/// [`OBJECT_CANNED_A_C_L_VALUES`].
///
/// The OBJECT rows of the published *Canned ACL* table (Amazon S3 User Guide, *Canned
/// ACL*), as [`canned_acl_grants`] carries the bucket ones:
///
/// * `private`, `public-read`, `public-read-write` and `authenticated-read` stand for the
///   same grants on an object as on a bucket, so they come from that one table rather than
///   being restated here (`log-delivery-write` is a BUCKET-only value, which is why a
///   value the object enum does not publish is refused);
/// * `bucket-owner-read` — "Object owner gets FULL_CONTROL. Bucket owner gets READ";
/// * `bucket-owner-full-control` — "Both the object owner and the bucket owner get
///   FULL_CONTROL";
/// * `aws-exec-read` — "Owner gets FULL_CONTROL", the one grant of that row this emulator
///   can state: the READ it also gives goes to an Amazon EC2 grantee whose canonical user
///   id AWS publishes nowhere, and an id is not invented for it.
///
/// This emulator serves ONE account ([`canonical_user_id`]), so an object's owner and its
/// bucket's owner are the same grantee: `bucket-owner-full-control` names that one
/// grantee's `FULL_CONTROL` twice, and an ACL granting the same grantee the same
/// permission twice grants nothing a second time, so it stands for the single grant.
pub fn object_canned_acl_grants(canned: &str) -> Option<Vec<AclGrant>> {
    let owner = AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_FULL_CONTROL);
    let grants = match canned {
        "bucket-owner-read" => vec![
            owner,
            AclGrant::to_canonical_user(canonical_user_id(), PERMISSION_READ),
        ],
        "bucket-owner-full-control" | "aws-exec-read" => vec![owner],
        shared if OBJECT_CANNED_A_C_L_VALUES.contains(&shared) => canned_acl_grants(shared)?,
        _ => return None,
    };
    Some(grants)
}

/// How many bytes an ACL retains: every grant's permission, its grantee's `Type` and
/// whichever member names that grantee, in UTF-8 bytes.
///
/// What the object byte budgets count an ACL kept beside an object as — see
/// [`Object::retained_bytes`], which counts a tag set the same way ([`tag_set_bytes`]).
pub fn acl_grants_bytes(grants: &[AclGrant]) -> usize {
    grants
        .iter()
        .map(|grant| {
            grant.permission.len()
                + grant.grantee.grantee_type.len()
                + [
                    &grant.grantee.id,
                    &grant.grantee.uri,
                    &grant.grantee.email_address,
                ]
                .iter()
                .filter_map(|member| member.as_ref())
                .map(String::len)
                .sum::<usize>()
        })
        .sum()
}

/// Why an ACL cannot be stored on a bucket or an object.
///
/// Each variant is one rule the API publishes for an ACL; the caller maps it to the wire
/// code and status S3 answers it with, the way a [`TagSetRejection`] is rendered by its
/// caller — this store owns no wire concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclRejection {
    /// The ACL carries more than [`MAX_ACL_GRANTS`] grants.
    TooManyGrants { count: usize },
    /// A grant names a `Permission` outside [`PERMISSION_VALUES`].
    UnknownPermission { permission: String },
    /// A grantee names a `Type` outside [`TYPE_VALUES`].
    UnknownGranteeType { grantee_type: String },
    /// A grantee's `Type` and its members disagree: the member that type is named by is
    /// missing or empty.
    GranteeMemberMissing {
        grantee_type: String,
        member: &'static str,
    },
    /// A `Group` grantee names a URI that is not one of [`PREDEFINED_GROUP_URIS`].
    UnknownGroupUri { uri: String },
    /// An `AmazonCustomerByEmail` grantee in a Region where S3 has discontinued Email
    /// Grantee ACLs ([`EMAIL_GRANTEE_DISCONTINUED_REGIONS`]).
    EmailGranteeDiscontinued { region: String },
    /// An `AmazonCustomerByEmail` grantee anywhere else: this emulator serves ONE account
    /// and holds no email address for it, so no email address it is given can match an
    /// account on record.
    EmailGranteeUnresolvable { email_address: String },
}

impl std::fmt::Display for AclRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyGrants { count } => write!(
                f,
                "the access control list carries {count} grants, more than the \
                 {MAX_ACL_GRANTS} grants one ACL may hold"
            ),
            Self::UnknownPermission { permission } => write!(
                f,
                "Grant.Permission must be one of {PERMISSION_VALUES:?}, got {permission:?}"
            ),
            Self::UnknownGranteeType { grantee_type } => write!(
                f,
                "Grant.Grantee.Type must be one of {TYPE_VALUES:?}, got {grantee_type:?}"
            ),
            Self::GranteeMemberMissing {
                grantee_type,
                member,
            } => write!(
                f,
                "a Grant.Grantee of type {grantee_type:?} is named by its {member}, which \
                 this grant does not carry"
            ),
            Self::UnknownGroupUri { uri } => write!(
                f,
                "Grant.Grantee.URI must be one of the predefined group URIs \
                 {PREDEFINED_GROUP_URIS:?}, got {uri:?}"
            ),
            Self::EmailGranteeDiscontinued { region } => write!(
                f,
                "Email Grantee ACLs are no longer supported in {region}; name the grantee \
                 by its canonical user ID instead"
            ),
            Self::EmailGranteeUnresolvable { email_address } => write!(
                f,
                "the email address {email_address:?} does not match any account on record"
            ),
        }
    }
}

impl AclRejection {
    /// The wire error code S3 answers this broken rule with, and the HTTP status its
    /// error-responses list carries for that code ([`Self::status`]).
    ///
    /// ONE mapping for every ACL S3 holds, so the code a client branches on is the same
    /// whether the ACL was named in a request body, in the `x-amz-grant-*` headers or as a
    /// canned ACL, and whether the resource is a bucket or an object.
    pub fn code(&self) -> &'static str {
        match self {
            // The grantee's own identity is the parameter that is not valid, which is what
            // `InvalidArgument` reports ("Invalid Argument", HTTP 400).
            Self::UnknownGroupUri { .. } => INVALID_ARGUMENT_CODE,
            // "As of October 1, 2025, Amazon S3 has discontinued support for Email Grantee
            // Access Control Lists … the request will receive an HTTP 405 (Method Not
            // Allowed) error", in the Regions that change affects.
            Self::EmailGranteeDiscontinued { .. } => METHOD_NOT_ALLOWED_CODE,
            // Everywhere else an email grantee is still taken by the API and then resolved
            // against the accounts on record, of which this emulator has exactly one and
            // no email address for it.
            Self::EmailGranteeUnresolvable { .. } => UNRESOLVABLE_GRANT_BY_EMAIL_CODE,
            // The rest are ACLs that do not validate against the schema the API publishes
            // for one: too many grants, a value outside a published set, or a grantee whose
            // `Type` and members disagree.
            Self::TooManyGrants { .. }
            | Self::UnknownPermission { .. }
            | Self::UnknownGranteeType { .. }
            | Self::GranteeMemberMissing { .. } => MALFORMED_ACL_CODE,
        }
    }

    /// The HTTP status [`Self::code`] is rendered with: 400 for every one of them except
    /// the discontinued-email refusal, whose documented status is 405.
    pub fn status(&self) -> u16 {
        match self {
            Self::EmailGranteeDiscontinued { .. } => METHOD_NOT_ALLOWED_STATUS,
            _ => ACL_REFUSAL_STATUS,
        }
    }
}

/// The HTTP status an ACL refusal is rendered with, except the discontinued-email one
/// ([`METHOD_NOT_ALLOWED_STATUS`]): 400, the status S3's error-responses list carries for
/// `MalformedACLError`, `MissingRequestBodyError`, `AccessControlListNotSupported` and
/// `UnresolvableGrantByEmailAddress` alike.
pub const ACL_REFUSAL_STATUS: u16 = 400;

/// Whether `grants` may be stored as the ACL of a resource in `region`, reporting the
/// first published rule it breaks.
///
/// ONE rule for every ACL S3 holds. The count is judged before the per-grant rules so an
/// enormous list is refused for what it is rather than for whichever grant happens to come
/// first, and each grant is then judged on its `Permission` and on whether its grantee's
/// `Type` and members agree — the agreement the wire shape leaves open, because `Grantee`
/// makes only `Type` required and every id-carrying member optional.
///
/// `region` decides only how an email grantee is refused: see
/// [`EMAIL_GRANTEE_DISCONTINUED_REGIONS`].
pub fn validate_acl_grants(grants: &[AclGrant], region: &str) -> Result<(), AclRejection> {
    if grants.len() > MAX_ACL_GRANTS {
        return Err(AclRejection::TooManyGrants {
            count: grants.len(),
        });
    }
    for grant in grants {
        if !PERMISSION_VALUES.contains(&grant.permission.as_str()) {
            return Err(AclRejection::UnknownPermission {
                permission: grant.permission.clone(),
            });
        }
        let grantee = &grant.grantee;
        let named = |member: Option<&String>| -> Option<String> {
            member
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        match grantee.grantee_type.as_str() {
            GRANTEE_CANONICAL_USER => {
                // The id is recorded as given: S3 resolves a canonical user id against
                // the account namespace, which an emulator serving one account cannot do,
                // so it refuses only an id that is not there at all.
                if named(grantee.id.as_ref()).is_none() {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "ID",
                    });
                }
            }
            GRANTEE_GROUP => {
                let Some(uri) = named(grantee.uri.as_ref()) else {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "URI",
                    });
                };
                if !PREDEFINED_GROUP_URIS.contains(&uri.as_str()) {
                    return Err(AclRejection::UnknownGroupUri { uri });
                }
            }
            GRANTEE_BY_EMAIL => {
                let Some(email_address) = named(grantee.email_address.as_ref()) else {
                    return Err(AclRejection::GranteeMemberMissing {
                        grantee_type: grantee.grantee_type.clone(),
                        member: "EmailAddress",
                    });
                };
                if EMAIL_GRANTEE_DISCONTINUED_REGIONS.contains(&region) {
                    return Err(AclRejection::EmailGranteeDiscontinued {
                        region: region.to_string(),
                    });
                }
                return Err(AclRejection::EmailGranteeUnresolvable { email_address });
            }
            unknown => {
                return Err(AclRejection::UnknownGranteeType {
                    grantee_type: unknown.to_string(),
                })
            }
        }
    }
    Ok(())
}

/// The grantee one `type=value` pair of an `x-amz-grant-*` header names, or `None` when
/// the pair is not one of the published forms.
///
/// "You specify each grantee as a type=value pair, where the type is one of the following:
/// id – if the value specified is the canonical user ID of an Amazon Web Services account[;]
/// uri – if you are granting permissions to a predefined group[;] emailAddress – if the
/// value specified is the email address of an [account]" (`PutBucketAcl`'s own
/// documentation). The value may be quoted — the documented examples spell it
/// `emailAddress="xyz@amazon.com"` — so one pair of surrounding double quotes is stripped,
/// and the type is matched without regard to case, the way an HTTP header's token is read.
pub fn acl_grantee_from_pair(pair: &str) -> Option<AclGrantee> {
    let (grantee_type, value) = pair.split_once('=')?;
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .trim();
    if value.is_empty() {
        return None;
    }
    match grantee_type.trim().to_ascii_lowercase().as_str() {
        "id" => Some(AclGrantee::canonical_user(value)),
        "uri" => Some(AclGrantee::group(value)),
        "emailaddress" => Some(AclGrantee {
            grantee_type: GRANTEE_BY_EMAIL.to_string(),
            id: None,
            uri: None,
            email_address: Some(value.to_string()),
        }),
        _ => None,
    }
}

/// The grants one `x-amz-grant-*` header names, or the pair that is not a grantee.
///
/// The header carries a comma-separated list of the pairs [`acl_grantee_from_pair`] reads
/// ("x-amz-grant-read: emailAddress="xyz@amazon.com", emailAddress="abc@amazon.com"" —
/// `PutBucketAcl`'s documented example), and every grantee in it is granted the one
/// `permission` that header stands for.
pub fn acl_grants_from_header(header: &str, permission: &str) -> Result<Vec<AclGrant>, String> {
    let mut grants: Vec<AclGrant> = Vec::new();
    for pair in header.split(',').map(str::trim).filter(|pair| !pair.is_empty()) {
        let Some(grantee) = acl_grantee_from_pair(pair) else {
            return Err(pair.to_string());
        };
        grants.push(AclGrant {
            grantee,
            permission: permission.to_string(),
        });
    }
    Ok(grants)
}

/// The code S3 answers an ACL request whose ACL does not validate against the schema the
/// API publishes for it: "MalformedACLError - The XML that you provided was not well
/// formed or did not validate against our published schema", HTTP 400 (Amazon S3 "Error
/// responses", list of error codes).
pub const MALFORMED_ACL_CODE: &str = "MalformedACLError";

/// The code S3 answers a request that carries no body where one is required: "This happens
/// when the user sends an empty XML document as a request. The error message is, 'Request
/// body is empty'", HTTP 400 (same list) — what an ACL request that names no ACL at all
/// gets, since the ACL travels either in the body or in the request headers.
pub const MISSING_REQUEST_BODY_CODE: &str = "MissingRequestBodyError";
/// See [`MISSING_REQUEST_BODY_CODE`].
pub const MISSING_REQUEST_BODY_MESSAGE: &str = "Request body is empty";

/// The code S3 answers an attempt to set an ACL on a bucket whose Object Ownership setting
/// is `BucketOwnerEnforced`: "AccessControlListNotSupported - The bucket does not allow
/// ACLs", HTTP 400 (same list).
///
/// The case `PutBucketAcl`'s own documentation describes: "If your bucket uses the bucket
/// owner enforced setting for S3 Object Ownership, ACLs are disabled and no longer affect
/// permissions. … Requests to set ACLs or update ACLs fail and return the
/// AccessControlListNotSupported error code. Requests to read ACLs are still supported."
pub const ACL_NOT_SUPPORTED_CODE: &str = "AccessControlListNotSupported";
/// See [`ACL_NOT_SUPPORTED_CODE`].
pub const ACL_NOT_SUPPORTED_MESSAGE: &str = "The bucket does not allow ACLs";

/// The code S3 answers an ACL naming a grantee by an email address that resolves to no
/// account: "UnresolvableGrantByEmailAddress - The email address that you provided does
/// not match any account on record", HTTP 400 (same list).
pub const UNRESOLVABLE_GRANT_BY_EMAIL_CODE: &str = "UnresolvableGrantByEmailAddress";

/// The code S3 answers a request the resource does not allow at all: "MethodNotAllowed -
/// The specified method is not allowed against this resource", HTTP 405 (same list) — what
/// an Email Grantee ACL gets in the Regions where S3 has discontinued it
/// ([`EMAIL_GRANTEE_DISCONTINUED_REGIONS`]), and what a read that names a version which is
/// a DELETE MARKER gets ("If the specified version is a delete marker, the response
/// returns a 405 Method Not Allowed error" — `GetObject`'s own documentation, whose
/// options `HeadObject` shares).
pub const METHOD_NOT_ALLOWED_CODE: &str = "MethodNotAllowed";
/// See [`METHOD_NOT_ALLOWED_CODE`] — the message of that same list entry.
pub const METHOD_NOT_ALLOWED_MESSAGE: &str =
    "The specified method is not allowed against this resource.";
/// The HTTP status [`METHOD_NOT_ALLOWED_CODE`] is rendered with: 405, from the same list
/// and from `PutBucketAcl`'s own documentation ("the request will receive an HTTP 405
/// (Method Not Allowed) error").
pub const METHOD_NOT_ALLOWED_STATUS: u16 = 405;

/// The `<ResourceType>` S3's `MethodNotAllowed` document names a request for an object's
/// BitTorrent file by: the `?torrent` subresource the method was refused against.
///
/// The code's message is fixed ("the specified method is not allowed against this
/// resource"), so the `<Method>` / `<ResourceType>` pair is the only part of the document
/// that says which method and which resource type it was \u2014 see
/// [`crate::scaffolded::Scaffolded::method_not_allowed`].
pub const TORRENT_RESOURCE_TYPE: &str = "TORRENT";

/// The `<ResourceType>` of the same document for a call addressed at a version that is a
/// DELETE MARKER: "if the specified version is a delete marker, the response returns a 405
/// Method Not Allowed error" (`GetObject`'s own documentation, whose options `HeadObject`
/// shares), so the resource the method was refused against is the marker.
pub const DELETE_MARKER_RESOURCE_TYPE: &str = "DeleteMarker";

/// The methods the resource type `resource_type` DOES allow, as the `Allow` header's list,
/// or `None` for a resource type nothing is published about.
///
/// A delete marker allows exactly one: "to permanently delete a delete marker, you must
/// include its version ID in a DeleteObject request" (Amazon S3 User Guide, *Managing delete
/// markers*) — a marker holds no bytes, so every other object-level method is refused
/// against it, and removing it is what brings the object back. The `?torrent` subresource
/// publishes no such set, so a refusal against it advertises none rather than inventing one.
pub fn methods_allowed_on(resource_type: &str) -> Option<&'static str> {
    match resource_type {
        DELETE_MARKER_RESOURCE_TYPE => Some("DELETE"),
        _ => None,
    }
}

/// The `ObjectOwnership` value that DISABLES ACLs on a bucket: "ACLs are disabled and no
/// longer affect permissions" (`PutBucketAcl`'s documentation), so a request to set one is
/// refused with [`ACL_NOT_SUPPORTED_CODE`] while a request to read one still answers.
pub const BUCKET_OWNER_ENFORCED: &str = "BucketOwnerEnforced";

/// The `ObjectOwnership` value a bucket is left in once its ownership controls are
/// REMOVED: ACLs affect permissions again, and "the uploading account will own the object
/// if the object is uploaded with the bucket-owner-full-control canned ACL"
/// (`smithy_query(query="shape",shape="ObjectOwnership")`). A bucket with no
/// `OwnershipControls` is one whose ACLs are enabled — the state
/// `GetBucketOwnershipControls` describes as "the bucket was created before the
/// BucketOwnerEnforced setting was introduced" or "you've manually deleted the bucket
/// ownership control value".
pub const OBJECT_WRITER: &str = "ObjectWriter";

/// The `ObjectOwnership` a general purpose bucket is CREATED with when the request names
/// none: "By default, `ObjectOwnership` is set to `BucketOwnerEnforced` and ACLs are
/// disabled" (`smithy_query(query="shape",shape="ObjectOwnership")`), so a bucket nothing
/// has configured refuses an ACL write with [`ACL_NOT_SUPPORTED_CODE`].
pub const DEFAULT_OBJECT_OWNERSHIP: &str = BUCKET_OWNER_ENFORCED;

/// The code S3 answers a request that would leave a bucket with BOTH an ACL that grants
/// more than the owner's `FULL_CONTROL` and the `BucketOwnerEnforced` Object Ownership
/// setting that disables ACLs: "InvalidBucketAclWithObjectOwnership - Bucket cannot have
/// ACLs set with ObjectOwnership's BucketOwnerEnforced setting", HTTP 400 (Amazon S3
/// "Error responses", list of error codes).
pub const INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_CODE: &str =
    "InvalidBucketAclWithObjectOwnership";
/// See [`INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_CODE`] — the text of that same entry.
pub const INVALID_BUCKET_ACL_WITH_OBJECT_OWNERSHIP_MESSAGE: &str =
    "Bucket cannot have ACLs set with ObjectOwnership's BucketOwnerEnforced setting";

/// Whether one ACL grant gives its permission to a PREDEFINED GROUP that stands for people
/// outside the bucket owner's account — `AllUsers` (anyone) or `AuthenticatedUsers` (any
/// AWS account).
///
/// "The Meaning of Public" (Amazon S3 User Guide, Block Public Access): an ACL is public
/// when it grants access to `AllUsers` or `AuthenticatedUsers`. A grant to the
/// `LogDelivery` group is to an S3 service principal, not to the public, and a grant to a
/// canonical user or an email address names one account.
pub fn acl_grant_is_public(grant: &AclGrant) -> bool {
    grant
        .grantee
        .uri
        .as_deref()
        .is_some_and(|uri| uri == ALL_USERS_GROUP_URI || uri == AUTHENTICATED_USERS_GROUP_URI)
}

/// Whether `grants` gives any access to a predefined group that stands for people outside
/// the bucket owner's account — see [`acl_grant_is_public`], the per-grant rule this is the
/// any-of over.
pub fn acl_grants_public_access(grants: &[AclGrant]) -> bool {
    grants.iter().any(acl_grant_is_public)
}

/// Whether `grants` gives `permission` to the caller that presents NO credential.
///
/// `AllUsers` is the predefined group that "includes anyone in the world" while
/// `AuthenticatedUsers` "includes all AWS accounts" (Amazon S3 User Guide, *Access control
/// list (ACL) overview*, Amazon S3 predefined groups), so a request speaking for nobody is
/// reached by the first and NOT by the second. `FULL_CONTROL` "allows grantee the READ,
/// WRITE, READ_ACP, and WRITE_ACP permissions" (same overview), so it answers for any one
/// permission asked about.
///
/// The grants passed in are what an ACL READ reports ([`Bucket::object_acl_grants`]), which
/// is where `IgnorePublicAcls` has already dropped every public grant S3 ignores: a grant
/// S3 ignores gives nobody anything, so it cannot admit this caller either.
pub fn acl_grants_to_all_users(grants: &[AclGrant], permission: &str) -> bool {
    grants.iter().any(|grant| {
        grant.grantee.uri.as_deref() == Some(ALL_USERS_GROUP_URI)
            && (grant.permission == permission || grant.permission == PERMISSION_FULL_CONTROL)
    })
}

/// The ARN of the S3 bucket `bucket`: `arn:aws:s3:::<bucket>`.
///
/// An S3 bucket's ARN leaves the Region and the account-id fields EMPTY, "because a bucket
/// name is globally unique" (see [`ArnField`]), so it is spelled here rather than through
/// `engine::emulator_arn`, which always carries the account. One place, so every refusal
/// and every policy decision that names a bucket names it the same way.
pub fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The ARN of the object `key` in `bucket`: `arn:aws:s3:::<bucket>/<key>` — the bucket's
/// ARN ([`bucket_arn`]) with the key as the rest of the resource part, which is how S3
/// names an object in a policy and in a refusal.
pub fn object_arn(bucket: &str, key: &str) -> String {
    format!("{}/{key}", bucket_arn(bucket))
}

/// The principal a request speaks for, as `sts:GetCallerIdentity` reports it: the account's
/// own identity, `arn:aws:iam::<account>:user/emulator`, which is what a refusal names.
///
/// This emulator serves one account, and a request presenting its credential speaks for
/// that account; the ARN is built through `engine::emulator_arn` so it carries the one
/// account id this emulator presents.
pub fn caller_principal_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The message S3 words an `AccessDenied` the Block Public Access `BlockPublicAcls` setting
/// caused: who was refused, which IAM `action`, on which `resource`, and the setting that
/// prevented it.
///
/// Every value in it comes from the request: the `principal` is the one the request signed
/// for ([`crate::service::request_authorization::request_caller`] resolves it, and
/// [`caller_principal_arn`] is what the account's own credential speaks for), the action is
/// the operation the request called and the resource is the bucket or object it addressed.
/// One place, so the bucket half and the object half of the same guardrail word it
/// identically.
pub fn block_public_acls_denial(principal: &str, action: &str, resource: &str) -> String {
    block_public_access_denial(
        principal,
        action,
        resource,
        "public ACLs are prevented by the BlockPublicAcls",
    )
}

/// The message S3 words an `AccessDenied` the Block Public Access `BlockPublicPolicy`
/// setting caused — the policy half of the guardrail [`block_public_acls_denial`] states
/// for an ACL, in the same sentence with the setting that prevented the call.
///
/// Every value in it comes from the request, exactly as in the ACL half.
pub fn block_public_policy_denial(principal: &str, action: &str, resource: &str) -> String {
    block_public_access_denial(
        principal,
        action,
        resource,
        "public policies are prevented by the BlockPublicPolicy",
    )
}

/// The one sentence a Block Public Access refusal is worded in: who was refused, which IAM
/// `action`, on which `resource`, and the `prevented_by` clause naming what the setting
/// prevents and which setting it is.
///
/// The sentence shape itself is the crate's one authorization refusal
/// ([`authorization_denial`]), so a guardrail refusal and a policy refusal read alike and
/// only the reason clause differs — which is the only thing that differs about them.
fn block_public_access_denial(
    principal: &str,
    action: &str,
    resource: &str,
    prevented_by: &str,
) -> String {
    authorization_denial(
        principal,
        action,
        resource,
        &format!("because {prevented_by} setting in S3 Block Public Access."),
    )
}

/// The versioning state a bucket carries once `PutBucketVersioning` has set one.
///
/// A bucket that has never had the subresource set carries NO value of this at all —
/// "If the versioning state has never been set on a bucket, it has no versioning state;
/// a GetBucketVersioning request does not return a versioning state value"
/// (`PutBucketVersioning`'s own documentation) — which is why [`Bucket::versioning`] is
/// an `Option` of it rather than a status defaulting to anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketVersioning {
    /// A `BucketVersioningStatus` value: `Enabled` or `Suspended`
    /// (`BUCKET_VERSIONING_STATUS_VALUES`), exactly as the configuration carried it.
    pub status: String,
    /// An `MFADeleteStatus` value, or `None` while the bucket has never been configured
    /// with MFA delete: "This element is only returned if the bucket has been configured
    /// with MFA delete. If the bucket has never been so configured, this element is not
    /// returned" (`GetBucketVersioningOutput$MFADelete`).
    pub mfa_delete: Option<String>,
}

/// The Block Public Access settings a bucket carries.
///
/// Four INDEPENDENT switches, not an `Option` each: every member of
/// `PublicAccessBlockConfiguration` is a `Setting` (a boolean) documented as "Specifies
/// whether Amazon S3 should block …  Setting this element to TRUE causes …"
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`), so a switch the
/// configuration does not turn on is off — there is no third state for a member to be in.
///
/// [`Bucket::public_access_block`] is an `Option` of it because a bucket can carry NO
/// configuration at all — the state `DeletePublicAccessBlock` leaves it in, and the only
/// state in which a read of the subresource is not-found. A bucket this store records comes
/// into being with [`Self::all_enabled`] rather than with nothing, because S3 creates every
/// new bucket with all four settings on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BucketPublicAccessBlock {
    /// Whether S3 blocks public ACLs for the bucket and its objects.
    pub block_public_acls: bool,
    /// Whether S3 ignores public ACLs on the bucket and its objects.
    pub ignore_public_acls: bool,
    /// Whether S3 blocks public bucket policies for the bucket.
    pub block_public_policy: bool,
    /// Whether S3 restricts access to the bucket when it has a public policy.
    pub restrict_public_buckets: bool,
}

impl BucketPublicAccessBlock {
    /// Every one of the four switches ON — the configuration a bucket comes into being
    /// carrying.
    ///
    /// "Amazon S3 automatically enables S3 Block Public Access … for new buckets" (Amazon
    /// S3 User Guide, Blocking public access to your Amazon S3 storage): a bucket created
    /// today starts out fully blocked, so a read of the subresource on a bucket nothing has
    /// configured reports four `true`s rather than "there is none".
    pub const fn all_enabled() -> Self {
        Self {
            block_public_acls: true,
            ignore_public_acls: true,
            block_public_policy: true,
            restrict_public_buckets: true,
        }
    }
}

/// Every value the `Event` enum publishes: the bucket events a notification configuration
/// may ask to be notified about, and so the only values one may name.
///
/// Re-exported from the model-generated value list for the shape rather than restated, so
/// there is ONE source of truth and a regenerated model cannot leave this behind.
pub use crate::ops::types::EVENT_VALUES;

/// Every value the `FilterRuleName` enum publishes: a key-name filter rule filters on the
/// prefix or on the suffix of an object key and on nothing else. Re-exported from the
/// model-generated value list, like [`EVENT_VALUES`].
pub use crate::ops::types::FILTER_RULE_NAME_VALUES;

/// Each `FilterRuleName` wire value beside the CANONICAL spelling S3 keeps the rule under.
///
/// The enum is the REQUEST vocabulary — `FilterRuleName`'s values are `prefix` and `suffix`
/// (`smithy_query(query="shape",shape="FilterRuleName")`), which is what a caller sends —
/// while the rule S3 stores, and reports to every reader of the bucket's notification
/// document, is named with the capitalised spelling. The two differ only in the case of the
/// first character, so the pair is kept here once and [`canonical_filter_rule_name`] is the
/// one way either spelling is resolved.
const FILTER_RULE_NAME_SPELLINGS: &[(&str, &str)] = &[
    (FILTER_RULE_PREFIX, "Prefix"),
    (FILTER_RULE_SUFFIX, "Suffix"),
];

/// The `FilterRuleName` value that filters on the START of an object key, and the one that
/// filters on its END — the request vocabulary's two values, named so a rule of one kind
/// can be asked for by kind.
pub const FILTER_RULE_PREFIX: &str = "prefix";
/// See [`FILTER_RULE_PREFIX`].
pub const FILTER_RULE_SUFFIX: &str = "suffix";

/// The canonical spelling of one key-name filter rule's `Name`, or `None` for a value that
/// is no `FilterRuleName` at all.
///
/// Both spellings resolve — the wire value a request carries and the canonical one a stored
/// rule holds — so a configuration read out of the store is as valid as the one that was
/// put, and the store never holds two spellings of one rule kind.
pub fn canonical_filter_rule_name(name: &str) -> Option<&'static str> {
    FILTER_RULE_NAME_SPELLINGS
        .iter()
        .find(|(wire, _)| wire.eq_ignore_ascii_case(name))
        .map(|(_, canonical)| *canonical)
}

/// The WIRE value of a filter rule kind, as the sentence refusing a second rule of that
/// kind spells it, or `None` for a value that is no `FilterRuleName`.
fn filter_rule_kind(name: &str) -> Option<&'static str> {
    FILTER_RULE_NAME_SPELLINGS
        .iter()
        .find(|(wire, _)| wire.eq_ignore_ascii_case(name))
        .map(|(wire, _)| *wire)
}

/// How many destination configurations one bucket's notification configuration may carry.
///
/// `PutBucketNotificationConfiguration`'s own documentation points at the published quota
/// for this ("For more information about the number of event notification configurations
/// that you can create per bucket, see Amazon S3 service quotas"), which is 100 event
/// notification configurations per bucket; nothing in the model bounds the three lists
/// (`TopicConfigurationList` and its siblings carry no `@length`), so this is also what
/// bounds the records one bucket can park in this process's memory. A put past it is
/// refused rather than recorded, the way a create past [`MAX_BUCKETS_PER_ACCOUNT`] is.
pub const MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET: usize = 100;

/// Which destination a notification configuration publishes to, which is what decides the
/// member its ARN arrives on and the service that ARN has to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDestination {
    /// An Amazon SNS topic (`TopicConfiguration`).
    Topic,
    /// An Amazon SQS queue (`QueueConfiguration`).
    Queue,
    /// A Lambda function (`LambdaFunctionConfiguration`).
    LambdaFunction,
}

impl NotificationDestination {
    /// The member the destination ARN arrives on, as the wire spells it — the `xmlName`
    /// each required ARN member carries (`Topic`, `Queue` and `CloudFunction`
    /// respectively, from `smithy_query(query="shape",shape="TopicConfiguration")` and its
    /// siblings). Used to name the offending member in a refusal.
    pub fn arn_member(self) -> &'static str {
        match self {
            Self::Topic => "Topic",
            Self::Queue => "Queue",
            Self::LambdaFunction => "CloudFunction",
        }
    }

    /// The service field the destination's ARN names: an SNS topic ARN for a
    /// `TopicConfiguration`, an SQS queue ARN for a `QueueConfiguration` and a Lambda
    /// function ARN for a `LambdaFunctionConfiguration` — the three destinations
    /// `PutBucketNotificationConfiguration`'s documentation describes.
    pub fn arn_service(self) -> &'static str {
        match self {
            Self::Topic => "sns",
            Self::Queue => "sqs",
            Self::LambdaFunction => "lambda",
        }
    }

    /// What the destination IS, as the per-destination detail of a validation refusal names
    /// it — the noun S3's own sentence for that destination kind uses (see
    /// [`destination_permissions_detail`]).
    pub fn noun(self) -> &'static str {
        match self {
            Self::Topic => "topic",
            Self::Queue => "queue",
            Self::LambdaFunction => "Lambda function",
        }
    }
}

/// The per-destination detail S3 carries for a destination that EXISTS and whose resource
/// policy does not let Amazon S3 publish this bucket's notifications to it.
///
/// The `<ArgumentValue{n}>` beside the fixed
/// [`UNABLE_TO_VALIDATE_DESTINATIONS_MESSAGE`] sentence: the refusal's message is the same
/// for every destination that failed, so the detail is where the reason for ONE of them is
/// stated. Only the destination's kind varies in it ([`NotificationDestination::noun`]);
/// nothing of the request is interpolated.
pub fn destination_permissions_detail(destination: NotificationDestination) -> String {
    format!(
        "Permissions on the destination {} do not allow S3 to publish notifications from \
         this bucket",
        destination.noun()
    )
}

/// The same detail for a destination the service that owns it does not hold at all, which
/// is a different reason and so reads differently.
pub fn destination_absent_detail(destination: NotificationDestination) -> String {
    format!("The destination {} does not exist", destination.noun())
}

/// One object key name filtering rule of a configuration's `Filter`.
///
/// Both members are optional in the model (`smithy_query(query="shape",shape="FilterRule")`),
/// so each is kept exactly as the request carried it rather than defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationFilterRule {
    /// A `FilterRuleName` value in its CANONICAL spelling
    /// ([`canonical_filter_rule_name`]) — the name S3 keeps the rule under and reports to
    /// every reader — or `None` when the rule named none.
    pub name: Option<String>,
    /// "The value that the filter searches for in object key names" (`FilterRule$Value`),
    /// or `None` when the rule named none.
    pub value: Option<String>,
}

impl NotificationFilterRule {
    /// Which KIND of rule this is, as the `FilterRuleName` wire vocabulary spells it, or
    /// `None` for a rule naming no published name at all. Whichever spelling the rule is
    /// held in answers the same kind, so a filter cannot carry one kind twice under two
    /// spellings.
    pub fn kind(&self) -> Option<&'static str> {
        filter_rule_kind(self.name.as_deref()?)
    }

    /// The value this rule filters by, empty when it named none — which filters on
    /// nothing, the way a filter with no rule at all does.
    pub fn filtered_value(&self) -> &str {
        self.value.as_deref().unwrap_or_default()
    }
}

/// One destination configuration a bucket's notification configuration carries: a topic,
/// a queue or a Lambda function, with the events it is notified about.
///
/// The three lists have identical members (an optional `Id`, a required destination ARN, a
/// required `Events` list and an optional `Filter`), so one record serves all three and
/// the destination kind travels beside it as a [`NotificationDestination`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotificationTarget {
    /// The configuration's id. `None` only before the store has seen it: "An optional
    /// unique identifier for configurations in a notification configuration. If you don't
    /// provide one, Amazon S3 will assign an ID" (`NotificationId`), so
    /// [`ComAmazonawsS3Amazons3State::put_notification_configuration`] mints one for every
    /// record that arrives without, and every STORED record carries `Some`.
    pub id: Option<String>,
    /// The destination's ARN, verbatim.
    pub arn: String,
    /// The bucket events this destination is notified about, in the order the request
    /// carried them. Each is one of [`EVENT_VALUES`].
    pub events: Vec<String>,
    /// The key-name filter rules of the configuration's `Filter`, or `None` when it
    /// carries no `Filter` (and so matches every key).
    pub filter_rules: Option<Vec<NotificationFilterRule>>,
}

impl NotificationTarget {
    /// The value this configuration filters keys by for one rule KIND (`prefix` /
    /// `suffix`), empty when it carries no rule of that kind — which restricts nothing, so
    /// it is the value a configuration with no `Filter` filters by too.
    fn filtered_by(&self, kind: &str) -> &str {
        self.filter_rules
            .iter()
            .flatten()
            .find(|rule| rule.kind() == Some(kind))
            .map_or("", NotificationFilterRule::filtered_value)
    }
}

/// The whole notification configuration a bucket carries.
///
/// A bucket is created with the EMPTY value of this: "By default, your bucket has no event
/// notifications configured. That is, the notification configuration will be an empty
/// NotificationConfiguration" (`PutBucketNotificationConfiguration`'s documentation),
/// which is why this is a plain field rather than an `Option` — "no notifications" and "an
/// empty NotificationConfiguration" are the same state, and a put carrying an empty
/// container is how notifications are turned back off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BucketNotificationConfiguration {
    /// The SNS topic configurations, in the order the request carried them.
    pub topic_configurations: Vec<NotificationTarget>,
    /// The SQS queue configurations, in the order the request carried them.
    pub queue_configurations: Vec<NotificationTarget>,
    /// The Lambda function configurations, in the order the request carried them.
    pub lambda_function_configurations: Vec<NotificationTarget>,
    /// Whether the configuration carries an `EventBridgeConfiguration`, which "enables
    /// delivery of events to Amazon EventBridge"
    /// (`NotificationConfiguration$EventBridgeConfiguration`). The shape has no members,
    /// so its PRESENCE is the whole state and a `bool` records it exactly.
    pub event_bridge_enabled: bool,
}

impl BucketNotificationConfiguration {
    /// Every destination configuration, with its destination kind, in list order:
    /// topics, then queues, then Lambda functions.
    pub fn targets(&self) -> impl Iterator<Item = (NotificationDestination, &NotificationTarget)> {
        self.topic_configurations
            .iter()
            .map(|target| (NotificationDestination::Topic, target))
            .chain(
                self.queue_configurations
                    .iter()
                    .map(|target| (NotificationDestination::Queue, target)),
            )
            .chain(
                self.lambda_function_configurations
                    .iter()
                    .map(|target| (NotificationDestination::LambdaFunction, target)),
            )
    }

    /// The same records, for writing — what lets the store mint an id into each record
    /// that arrived without one.
    fn targets_mut(&mut self) -> impl Iterator<Item = &mut NotificationTarget> {
        self.topic_configurations
            .iter_mut()
            .chain(self.queue_configurations.iter_mut())
            .chain(self.lambda_function_configurations.iter_mut())
    }

    /// How many destination configurations the container carries, across all three lists —
    /// what [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`] bounds.
    pub fn configuration_count(&self) -> usize {
        self.topic_configurations.len()
            + self.queue_configurations.len()
            + self.lambda_function_configurations.len()
    }

    /// Whether the bucket has no event notifications configured at all — the state a
    /// bucket is created in, and the one an empty `NotificationConfiguration` puts it back
    /// into.
    pub fn is_empty(&self) -> bool {
        self.configuration_count() == 0 && !self.event_bridge_enabled
    }
}

/// Why a `NotificationConfiguration` cannot be applied to a bucket.
///
/// Every one of these is a request S3 refuses with `InvalidArgument` (HTTP 400) rather
/// than recording, so a refused put leaves the bucket's current configuration exactly as
/// it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationConfigurationRejection {
    /// An event outside [`EVENT_VALUES`]. The sentence S3 sends for this is FIXED text,
    /// so neither member is interpolated into it; they are what the refusal is logged with.
    UnsupportedEvent {
        /// The member the configuration arrived on.
        member: &'static str,
        /// The value that is not a published event.
        event: String,
    },
    /// A configuration naming no event at all: `Events` is a REQUIRED member of each
    /// configuration shape, so a configuration with an empty list asks for nothing to be
    /// published and cannot be applied.
    NoEvents {
        /// The member the configuration arrived on.
        member: &'static str,
    },
    /// A filter rule whose `Name` is not one of [`FILTER_RULE_NAME_VALUES`].
    InvalidFilterRuleName {
        /// The rule's `Name`, or the empty string when it named none.
        name: String,
    },
    /// A second rule of one KIND inside a single `Filter`: "overlapping prefixes and
    /// suffixes are not supported" (`FilterRule$Name`), so one filter carries at most one
    /// prefix rule and at most one suffix rule.
    DuplicateFilterRuleKind {
        /// The kind named twice, as the request vocabulary spells it (`prefix`/`suffix`).
        kind: &'static str,
    },
    /// Two configurations that would both match the same key for the same event VALUE: the
    /// two name one event between them, and their prefixes overlap AND their suffixes
    /// overlap, which leaves the document unable to say which rule an object matches.
    AmbiguousConfiguration,
    /// Two configurations that SHARE AN EVENT TYPE without naming the same value: one names
    /// the `s3:<Type>:*` form, which "stands for every event of that type", and the other an
    /// event of that type, so one object event would be published for both. The conflict is
    /// in the EVENT, which is the field the refusal names.
    OverlappingEventTypes {
        /// The two `Event` values the pair overlaps in, as the request spelled them: the one
        /// each configuration names that the other is notified of too, in the order the
        /// document carried the two configurations and separated by `, ` — the value the
        /// refusal's `<ArgumentValue>` carries
        /// ([`NotificationConfigurationRejection::argument`]).
        events: String,
    },
    /// Two configurations carrying the same `Id`: "An optional UNIQUE identifier for
    /// configurations in a notification configuration" (`NotificationId`).
    DuplicateConfigurationId {
        /// The id that is used twice.
        id: String,
    },
    /// A destination ARN that is not an ARN of the service that destination lives in.
    InvalidDestinationArn {
        /// The member the ARN arrived on (`Topic`, `Queue` or `CloudFunction`).
        member: &'static str,
        /// The service field such an ARN has to name.
        service: &'static str,
        /// The value that is not such an ARN.
        arn: String,
    },
    /// More configurations than [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`].
    TooManyConfigurations {
        /// How many the request carried.
        count: usize,
    },
}

impl std::fmt::Display for NotificationConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEvent { .. } => f.write_str(UNSUPPORTED_EVENT_MESSAGE),
            Self::NoEvents { member } => write!(
                f,
                "A {member} configuration must specify at least one event"
            ),
            Self::InvalidFilterRuleName { name } => write!(
                f,
                "filter rule name must be either prefix or suffix, got {name}"
            ),
            Self::DuplicateFilterRuleKind { kind } => {
                write!(f, "Cannot specify more than one {kind} rule in a filter.")
            }
            Self::AmbiguousConfiguration => f.write_str(
                "Configuration is ambiguously defined. Cannot have overlapping suffixes in \
                 two rules if the prefixes are overlapping for the same event type.",
            ),
            Self::OverlappingEventTypes { .. } => f.write_str(
                "Configurations overlap. Configurations on the same bucket cannot share a \
                 common event type.",
            ),
            Self::DuplicateConfigurationId { id } => {
                write!(f, "Configuration ID {id} is not unique")
            }
            Self::InvalidDestinationArn {
                member,
                service,
                arn,
            } => write!(
                f,
                "The {member} ARN {arn} is not a valid {service} ARN"
            ),
            Self::TooManyConfigurations { count } => write!(
                f,
                "A notification configuration may carry at most \
                 {MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET} configurations, got {count}"
            ),
        }
    }
}

impl NotificationConfigurationRejection {
    /// The request field this refusal is ABOUT, as S3 names it in the `<ArgumentName>` of
    /// its `InvalidArgument` document, beside the value that field carried — or `None` for a
    /// refusal whose document names no argument.
    ///
    /// S3's `InvalidArgument` document carries an `<ArgumentName>` / `<ArgumentValue>` pair
    /// beside its code and message (Amazon S3 API Reference, *Error responses*), and for a
    /// document whose configurations share an event type the field it names is the `Event`,
    /// carrying the two values the pair overlaps in: that class's sentence is the same for
    /// every document in it, so the pair is the whole of what says which events to change.
    ///
    /// Every other rejection answers `None` — the ambiguous-filters class included: the
    /// message is what states the rule each of them broke, and naming a field S3 is not known
    /// to name in those documents would invent one.
    #[must_use]
    pub fn argument(&self) -> Option<(&'static str, Option<&str>)> {
        match self {
            Self::OverlappingEventTypes { events } => {
                Some((EVENT_ARGUMENT, Some(events.as_str())))
            }
            _ => None,
        }
    }
}

/// The request field a document whose configurations share an event type is reported about:
/// the `Event` they overlap in, as the API publishes that member.
const EVENT_ARGUMENT: &str = "Event";

impl std::error::Error for NotificationConfigurationRejection {}

/// What one field of an ARN carries for the service that owns the resource.
///
/// The Region and the account-id fields are the two that differ between services: a
/// regional, account-owned resource names both, a GLOBAL service's ARN leaves the Region
/// out (`arn:aws:iam::<account>:role/<name>`) and an Amazon S3 bucket's ARN leaves both out
/// (`arn:aws:s3:::<bucket>`), because a bucket name is globally unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnField {
    /// The field carries a value.
    Present,
    /// The field is EMPTY — the `::` an ARN of such a service spells.
    Empty,
}

impl ArnField {
    /// Whether one ARN field as written satisfies this expectation.
    fn holds(self, field: &str) -> bool {
        match self {
            Self::Present => !field.is_empty(),
            Self::Empty => field.is_empty(),
        }
    }
}

/// Whether `arn` is an ARN of `service` whose Region and account-id fields are as that
/// service writes them, and whose resource part starts with `resource_prefix` and carries
/// something past it.
///
/// The value is read through the ONE shared ARN parse (`service_common::arn`), so this
/// crate does not split an ARN its own way. `resource_prefix` is the leading segment the
/// resource KIND is named by where one exists (`role/` for an IAM role) and empty for a
/// service whose resource part is the bare name (an S3 bucket, an SQS queue).
///
/// This is the one place an ARN MEMBER is judged in this crate: every member that takes an
/// ARN — a notification destination, a replication role, a replication destination bucket —
/// is judged through it rather than through a rule of its own.
pub fn is_arn_of(
    arn: &str,
    service: &str,
    region: ArnField,
    account: ArnField,
    resource_prefix: &str,
) -> bool {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return false;
    };
    let resource = parsed.resource;
    parsed.vendor == service
        && region.holds(parsed.region)
        && account.holds(parsed.account)
        && resource.len() > resource_prefix.len()
        && resource.starts_with(resource_prefix)
}

/// Whether `arn` is an ARN of `service` for a regional, account-owned resource named by its
/// bare resource part — the form every notification destination takes.
fn is_arn_of_service(arn: &str, service: &str) -> bool {
    is_arn_of(arn, service, ArnField::Present, ArnField::Present, "")
}

/// Whether `value` is the ARN of an S3 bucket: `arn:<partition>:s3:::<name>`.
///
/// A bucket ARN carries neither a Region nor an account id, because a bucket name is
/// globally unique, and its resource part is the bare bucket name. Read through
/// [`is_arn_of`], the crate's one ARN grammar.
///
/// Shared, because MORE than one member takes a bucket ARN: a replication rule's
/// `Destination$Bucket` and the analytics export's `AnalyticsS3BucketDestination$Bucket`
/// are both "the Amazon Resource Name (ARN) of the bucket", so neither may judge one its
/// own way.
pub fn is_s3_bucket_arn(value: &str) -> bool {
    is_arn_of(value, "s3", ArnField::Empty, ArnField::Empty, "")
}

/// Whether `value` is the ARN of an S3 ACCESS POINT:
/// `arn:<partition>:s3:<region>:<account-id>:accesspoint/<name>`.
///
/// An access point is a REGIONAL, account-owned resource — unlike the bucket it fronts,
/// whose ARN carries neither ([`is_s3_bucket_arn`]) — and its resource part names the kind
/// before the name, so a bucket ARN is not one of these however well formed it is. The
/// account is read as PRESENT rather than as this account's: an access point in another
/// account is still an access point, and a filter may name one.
///
/// Shared, because more than one member takes an access point ARN: `MetricsFilter` and
/// `MetricsAndOperator` both declare "the access point ARN used when evaluating a metrics
/// filter", so neither may judge one its own way. Read through [`is_arn_of`], the crate's
/// one ARN grammar.
pub fn is_s3_access_point_arn(value: &str) -> bool {
    is_arn_of(
        value,
        "s3",
        ArnField::Present,
        ArnField::Present,
        ACCESS_POINT_RESOURCE_PREFIX,
    )
}

/// What an access point ARN's resource part begins with, before the access point's own
/// name: the resource kind, as AWS spells it in the ARN of an access point
/// (`arn:aws:s3:<region>:<account-id>:accesspoint/<name>`).
const ACCESS_POINT_RESOURCE_PREFIX: &str = "accesspoint/";

/// Why `value` is not an ARN, as AWS states it where a refusal quotes the reason.
///
/// The ARN grammar is the published one [`is_arn_of`] reads —
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` — and the diagnostics are
/// the ones AWS's own ARN parser publishes for each way a value breaks it, in the order it
/// checks them: the `arn:` prefix, the six sections, then the three sections that may not
/// be empty. The last arm is for a value that IS an ARN and still does not name the
/// resource the member requires.
///
/// Nothing here is derived from the recorded request: the reason comes from `value`'s own
/// shape, and `value` itself is not in any of the sentences.
pub fn malformed_arn_reason(value: &str) -> &'static str {
    const PREFIX: &str = "arn:";
    if !value.starts_with(PREFIX) {
        return "Malformed ARN - doesn't start with 'arn:'";
    }
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    if fields.len() < 6 {
        return "Malformed ARN - doesn't contain enough sections";
    }
    if fields[1].is_empty() {
        return "Malformed ARN - no AWS partition";
    }
    if fields[2].is_empty() {
        return "Malformed ARN - no AWS service";
    }
    if fields[5].is_empty() {
        return "Malformed ARN - no AWS resource";
    }
    "Invalid ARN - the ARN does not name the resource this member requires"
}

/// Whether `configuration` is one S3 applies to a bucket, and what is wrong with it when
/// it is not.
///
/// Checked BEFORE anything is written, because "the PUT notification is an atomic
/// operation … If the message fails, the entire PUT action will fail, and Amazon S3 will
/// not add the configuration to your bucket" (`PutBucketNotificationConfiguration`'s own
/// documentation).
///
/// Three of the rules here are ALSO modelled constraints, and the boundary validator
/// refuses those before any handler runs (`Event` and `FilterRuleName` are enums and
/// `Events` is required, so a request breaking one never reaches the store). They are
/// still checked here because this function, not the wire, is what guards the STORE: a
/// configuration no reader could report back is never recorded, whoever writes it.
///
/// The last two rules are about the document as a whole rather than one value in it, and
/// no model can state either: a `Filter` carries at most one rule of each kind, and no two
/// configurations may both match the same key for the same event
/// ([`configurations_are_ambiguous`]).
pub fn validate_notification_configuration(
    configuration: &BucketNotificationConfiguration,
) -> Result<(), NotificationConfigurationRejection> {
    let count = configuration.configuration_count();
    if count > MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET {
        return Err(NotificationConfigurationRejection::TooManyConfigurations { count });
    }

    let mut seen_ids: BTreeSet<&str> = BTreeSet::new();
    for (destination, target) in configuration.targets() {
        let member = destination.arn_member();

        if !is_arn_of_service(&target.arn, destination.arn_service()) {
            return Err(NotificationConfigurationRejection::InvalidDestinationArn {
                member,
                service: destination.arn_service(),
                arn: target.arn.clone(),
            });
        }

        if target.events.is_empty() {
            return Err(NotificationConfigurationRejection::NoEvents { member });
        }
        for event in &target.events {
            if !EVENT_VALUES.contains(&event.as_str()) {
                return Err(NotificationConfigurationRejection::UnsupportedEvent {
                    member,
                    event: event.clone(),
                });
            }
        }

        for rule in target.filter_rules.iter().flatten() {
            let name = rule.name.as_deref().unwrap_or_default();
            if canonical_filter_rule_name(name).is_none() {
                return Err(NotificationConfigurationRejection::InvalidFilterRuleName {
                    name: name.to_string(),
                });
            }
        }

        // "Overlapping prefixes and suffixes are not supported" (`FilterRule$Name`): one
        // `Filter` carries at most ONE rule of each kind, because two of a kind leave the
        // filter with two answers for one key.
        let mut seen_kinds: BTreeSet<&str> = BTreeSet::new();
        for rule in target.filter_rules.iter().flatten() {
            if let Some(kind) = rule.kind() {
                if !seen_kinds.insert(kind) {
                    return Err(NotificationConfigurationRejection::DuplicateFilterRuleKind {
                        kind,
                    });
                }
            }
        }

        if let Some(id) = target.id.as_deref() {
            if !seen_ids.insert(id) {
                return Err(
                    NotificationConfigurationRejection::DuplicateConfigurationId {
                        id: id.to_string(),
                    },
                );
            }
        }
    }

    // And the document as a WHOLE: no two of its configurations may both be matched by one
    // object event. WHICH field the conflict is in decides what S3 answers:
    //
    // * two configurations that name the SAME event value are told apart by their key
    //   filters alone — "overlapping prefixes and suffixes are not supported"
    //   ([`filters_overlap`] is that half of the rule) — so a pair whose filters overlap too
    //   leaves the document ambiguous;
    // * two that do NOT name the same value and still share an event — one names the
    //   `s3:<Type>:*` form and the other an event of that type — overlap in the EVENT, and
    //   that is what S3 reports.
    //
    // The check is over every pair across all three lists, since which destination a
    // configuration publishes to does not make two rules distinguish an object.
    //
    // Each configuration's events are read ONCE, into the two bit sets [`named_mask`] and
    // [`event_mask`] encode them as, so the pair walk below costs two ANDs per pair: the walk
    // is bounded by the configuration count alone (at most
    // [`MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET`], checked above, so at most 4950
    // pairs) and reading the `Events` lists is ONE pass over them, whatever a request
    // carries in them. Comparing the lists pair by pair instead would multiply the pair
    // count by the two lists' lengths, which no published limit bounds.
    let targets: Vec<(&NotificationTarget, u64, u64)> = configuration
        .targets()
        .map(|(_, target)| (target, named_mask(target), event_mask(target)))
        .collect();
    for (position, (one, one_named, one_events)) in targets.iter().enumerate() {
        for (other, other_named, other_events) in &targets[position + 1..] {
            if one_events & other_events == 0 || !filters_overlap(one, other) {
                continue;
            }
            if one_named & other_named != 0 {
                return Err(NotificationConfigurationRejection::AmbiguousConfiguration);
            }
            return Err(NotificationConfigurationRejection::OverlappingEventTypes {
                events: overlapping_events((one, *one_events), (other, *other_events)),
            });
        }
    }

    Ok(())
}

/// The two `Event` values two configurations overlap in, as the refusal's `<ArgumentValue>`
/// carries them: the one each configuration names that the other is notified of too, in the
/// order the document carried the two configurations, separated by `, `.
///
/// Each value is the one the REQUEST spelled, read through the same per-value set delivery is
/// matched by ([`value_mask`]), so both really would be reached by one object event: for a
/// type's wildcard that is the wildcard as written, and for a literal value the value itself.
fn overlapping_events(
    one: (&NotificationTarget, u64),
    other: (&NotificationTarget, u64),
) -> String {
    let (one_target, one_events) = one;
    let (other_target, other_events) = other;
    format!(
        "{}, {}",
        overlapping_event(one_target, other_events),
        overlapping_event(other_target, one_events)
    )
}

/// The first `Event` `target` names that is also one of `events`.
///
/// The caller has already found that the two configurations' expanded sets intersect, so one
/// such value exists; a slip that left none falls back to the empty string rather than
/// panicking, because nothing a request carries may take the serving process down.
fn overlapping_event(target: &NotificationTarget, events: u64) -> String {
    target
        .events
        .iter()
        .find(|event| value_mask(event) & events != 0)
        .cloned()
        .unwrap_or_default()
}

/// The events one configuration NAMES, as the same bit set [`event_mask`] uses — the values
/// as written, with no wildcard expanded.
///
/// What two configurations sharing a bit of THIS set means is that both name one event value,
/// which is the pair whose conflict is in their key filters rather than in their events.
fn named_mask(target: &NotificationTarget) -> u64 {
    target
        .events
        .iter()
        .filter_map(|event| {
            EVENT_VALUES
                .iter()
                .position(|published| *published == event.as_str())
        })
        .fold(0, |mask, index| mask | (1 << index))
}

/// The events one configuration is NOTIFIED OF, as one bit per published `Event` value —
/// the set two configurations are asked to share an element of.
///
/// The `Event` vocabulary is CLOSED and small ([`EVENT_VALUES`], which
/// [`EVENT_VALUES_FIT_ONE_MASK`] holds to the width of this mask), and every event of the
/// container has already been checked to be one of its values by the time this is built, so
/// the mask loses nothing: a value outside the set has been refused, and a value repeated in
/// one list is the same bit.
///
/// The set is the one DELIVERY reads, not the literal text of the `Events` list: a value the
/// configuration names covers the event itself, and the `s3:<Type>:*` form "stands for every
/// event of that type" ([`notifications::event_matches`], the one reading of that rule in
/// this crate). So a configuration naming the wildcard of a type and another naming one
/// event of that type both match that event, and the pair is ambiguous for the same reason
/// two configurations naming the same value are: an object event would be published to both
/// and S3 could not say which rule it matched.
fn event_mask(target: &NotificationTarget) -> u64 {
    target
        .events
        .iter()
        .fold(0, |mask, event| mask | value_mask(event))
}

/// The published events ONE `Event` value is notified of, as that same bit set: the value
/// itself, and for the `s3:<Type>:*` form every published event of that type.
fn value_mask(event: &str) -> u64 {
    EVENT_VALUES
        .iter()
        .enumerate()
        .filter(|(_, published)| notifications::event_matches(event, published))
        .fold(0, |mask, (index, _)| mask | (1 << index))
}

/// Whether the published `Event` catalog still fits in the one mask [`event_mask`] encodes
/// a configuration's events as. Asserted by this crate's own test beside the rule it serves.
pub const EVENT_VALUES_FIT_ONE_MASK: usize = u64::BITS as usize;

/// Whether two destination configurations of one document would both match the same object
/// KEY: their prefixes overlap and their suffixes overlap, so some key passes both filters.
///
/// A rule's prefix and suffix each restrict the keys it matches, and two restrictions of
/// the same kind overlap when one is a prefix (respectively a suffix) of the other — a
/// configuration carrying no rule of that kind restricts nothing, which overlaps every
/// value. Two configurations whose prefixes are disjoint, or whose suffixes are, can
/// therefore never both match one key and are both kept, however their events compare.
fn filters_overlap(one: &NotificationTarget, other: &NotificationTarget) -> bool {
    prefixes_overlap(
        one.filtered_by(FILTER_RULE_PREFIX),
        other.filtered_by(FILTER_RULE_PREFIX),
    ) && suffixes_overlap(
        one.filtered_by(FILTER_RULE_SUFFIX),
        other.filtered_by(FILTER_RULE_SUFFIX),
    )
}

/// Whether two key-name PREFIXES restrict to key sets that intersect: one is a prefix of
/// the other, which includes either of them being empty.
fn prefixes_overlap(one: &str, other: &str) -> bool {
    one.starts_with(other) || other.starts_with(one)
}

/// Whether two key-name SUFFIXES restrict to key sets that intersect: one is a suffix of
/// the other, which includes either of them being empty.
fn suffixes_overlap(one: &str, other: &str) -> bool {
    one.ends_with(other) || other.ends_with(one)
}

/// One rule of a bucket's default server-side-encryption configuration: one
/// `ServerSideEncryptionRule` of the `ServerSideEncryptionConfiguration` a
/// `PutBucketEncryption` carried.
///
/// Every member is optional exactly where the model makes it so
/// (`smithy_query(query="shape",shape="ServerSideEncryptionRule")`): a rule may name a
/// default algorithm, a Bucket Key setting, a set of blocked encryption types, or any
/// combination of them. `sse_algorithm` and `kms_master_key_id` are the two members of
/// the rule's `ApplyServerSideEncryptionByDefault` container, flattened here because
/// `SSEAlgorithm` is REQUIRED inside it — so the container is present in the stored rule
/// exactly when the algorithm is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketEncryptionRule {
    /// `ApplyServerSideEncryptionByDefault$SSEAlgorithm`: a `ServerSideEncryption` value
    /// (`SERVER_SIDE_ENCRYPTION_VALUES`), or `None` when the rule names no default
    /// encryption at all.
    pub sse_algorithm: Option<String>,
    /// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`, verbatim as the request
    /// carried it — "You can specify the key ID, key alias, or the Amazon Resource Name
    /// (ARN) of the KMS key" (that member's own documentation), and S3 "doesn't validate
    /// the KMS key ID provided in PutBucketEncryption requests" (`PutBucketEncryption`'s
    /// documentation), so whichever form arrives is what comes back.
    pub kms_master_key_id: Option<String>,
    /// `ServerSideEncryptionRule$BucketKeyEnabled`, or `None` when the rule names no
    /// Bucket Key setting.
    pub bucket_key_enabled: Option<bool>,
    /// `BlockedEncryptionTypes$EncryptionType`: the `EncryptionType` values the rule
    /// blocks, in the order the request carried them, or `None` when the rule carries no
    /// `BlockedEncryptionTypes` container.
    pub blocked_encryption_types: Option<Vec<String>>,
}

/// The default encryption rule a general purpose bucket carries while no
/// `PutBucketEncryption` has configured one.
///
/// "By default, all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`'s own
/// documentation), and "General purpose buckets - By default, S3 Bucket Key is not
/// enabled" (`ServerSideEncryptionRule$BucketKeyEnabled`) — so the rule a never-configured
/// bucket reports is [`DEFAULT_SSE_ALGORITHM`] with the Bucket Key off and
/// [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`] blocked.
///
/// COMPUTED rather than stored, so a read of a bucket that was never configured reports
/// this without writing anything: [`Bucket::encryption`] stays `None` and a later reader
/// sees exactly what this one saw.
pub fn default_encryption_rule() -> BucketEncryptionRule {
    BucketEncryptionRule {
        sse_algorithm: Some(DEFAULT_SSE_ALGORITHM.to_string()),
        kms_master_key_id: None,
        bucket_key_enabled: Some(DEFAULT_BUCKET_KEY_ENABLED),
        blocked_encryption_types: Some(default_blocked_encryption_types()),
    }
}

/// The `EncryptionType` values a bucket blocks while no `PutBucketEncryption` has named a
/// set of its own — [`DEFAULT_BLOCKED_ENCRYPTION_TYPES`], as a stored rule holds them.
pub fn default_blocked_encryption_types() -> Vec<String> {
    DEFAULT_BLOCKED_ENCRYPTION_TYPES
        .iter()
        .map(|blocked| (*blocked).to_string())
        .collect()
}

/// Every value the `Protocol` enum publishes: the two schemes a website redirect may be
/// made over. Re-exported from the model-generated value list, like [`EVENT_VALUES`], so
/// there is ONE source of truth for the set.
pub use crate::ops::types::PROTOCOL_VALUES;

/// How many `CORSRule`s one bucket's cors configuration may carry.
///
/// The published limit, from `CORSConfiguration$CORSRules`'s own documentation: "A set of
/// origins and methods (cross-origin access that you want to allow). You can add up to 100
/// rules to the configuration." The model bounds the list at neither end
/// (`smithy_query(query="shape",shape="CORSRules")`), so this check is what enforces it —
/// and it also bounds what one bucket's record parks in this process's memory. A put past
/// it is refused rather than recorded.
pub const MAX_CORS_RULES: usize = 100;

/// The longest `CORSRule$ID` a stored rule may carry, in characters.
///
/// That member's own documentation: "Unique identifier for the rule. The value cannot be
/// longer than 255 characters." `ID` is a plain string with no `@length` in the model
/// (`smithy_query(query="shape",shape="ID")`), so nothing refuses a longer id before this
/// check does.
pub const MAX_CORS_RULE_ID_CHARS: usize = 255;

/// The HTTP methods a `CORSRule$AllowedMethods` entry may name.
///
/// That member's own documentation: "An HTTP method that you allow the origin to execute.
/// Valid values are `GET`, `PUT`, `HEAD`, `POST`, and `DELETE`." `AllowedMethod` is a plain
/// string in the model (`smithy_query(query="shape",shape="AllowedMethods")`), not an enum,
/// so this catalog is what refuses a method S3 does not support — including one that names
/// a supported method in another case, since the published values are upper case.
pub const CORS_ALLOWED_METHODS: [&str; 5] = ["GET", "PUT", "HEAD", "POST", "DELETE"];

/// The most bytes a bucket's cors configuration document may carry.
///
/// `PutBucketCors`'s own documentation: "The cors subresource is an XML document in which
/// you configure rules that identify origins and the HTTP methods that can be executed on
/// your bucket. The document is limited to 64 KB in size." Measured over the stored rules by
/// [`cors_configuration_bytes`], which is also what keeps one bucket's record from parking
/// an arbitrarily large configuration in this process's memory.
pub const MAX_CORS_CONFIGURATION_BYTES: usize = 64 * 1024;

/// One `CORSRule` of a bucket's cors configuration: "Specifies a cross-origin access rule
/// for an Amazon S3 bucket."
///
/// Every member is optional exactly where the model makes it so
/// (`smithy_query(query="shape",shape="CORSRule")`): `AllowedMethods` and `AllowedOrigins`
/// are REQUIRED, the other three are not. The two optional lists are plain `Vec`s rather
/// than `Option`s for the reason [`BucketWebsite::routing_rules`] is: both are
/// `xmlFlattened`, so the XML boundary parses a document naming no `AllowedHeader` to the
/// same absent list as one naming none at all, and there is nothing for a `None` to mean
/// that an empty list does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketCorsRule {
    /// `CORSRule$ID`, or `None` when the rule names none.
    pub id: Option<String>,
    /// `CORSRule$AllowedHeaders`: "Headers that are specified in the
    /// `Access-Control-Request-Headers` header", in the order the document named them.
    pub allowed_headers: Vec<String>,
    /// `CORSRule$AllowedMethods`, a REQUIRED member: each one of
    /// [`CORS_ALLOWED_METHODS`], in the order the document named them.
    pub allowed_methods: Vec<String>,
    /// `CORSRule$AllowedOrigins`, a REQUIRED member: "One or more origins you want
    /// customers to be able to access the bucket from", in the order the document named
    /// them.
    pub allowed_origins: Vec<String>,
    /// `CORSRule$ExposeHeaders`: "One or more headers in the response that you want
    /// customers to be able to access from their applications", in document order.
    pub expose_headers: Vec<String>,
    /// `CORSRule$MaxAgeSeconds`: "The time in seconds that your browser is to cache the
    /// preflight response for the specified resource", or `None` when the rule names none.
    pub max_age_seconds: Option<i32>,
}

/// The size of the smallest `CORSConfiguration` document that carries `rules`, in bytes.
///
/// A LOWER bound on the document the request carried — the markup of each element plus the
/// text inside it, with no attributes, namespace or whitespace — so a configuration this
/// reports more than [`MAX_CORS_CONFIGURATION_BYTES`] for could not have arrived in a
/// document within the published 64 KB. Element names are the `xmlName`s the model puts on
/// the members (`smithy_query(query="shape",shape="CORSRule")`), which is also how
/// `GetBucketCors` spells them on the way out.
pub fn cors_configuration_bytes(rules: &[BucketCorsRule]) -> usize {
    /// The bytes `<name>text</name>` takes.
    fn element(name: &str, text: &str) -> usize {
        name.len() * 2 + "<></>".len() + text.len()
    }
    /// The bytes every entry of a flattened list named `name` takes.
    fn elements(name: &str, texts: &[String]) -> usize {
        texts.iter().map(|text| element(name, text)).sum()
    }

    let mut bytes = element("CORSConfiguration", "");
    for rule in rules {
        bytes += element("CORSRule", "");
        if let Some(id) = &rule.id {
            bytes += element("ID", id);
        }
        bytes += elements("AllowedHeader", &rule.allowed_headers);
        bytes += elements("AllowedMethod", &rule.allowed_methods);
        bytes += elements("AllowedOrigin", &rule.allowed_origins);
        bytes += elements("ExposeHeader", &rule.expose_headers);
        if let Some(max_age_seconds) = rule.max_age_seconds {
            bytes += element("MaxAgeSeconds", &max_age_seconds.to_string());
        }
    }
    bytes
}

/// Why a cors configuration is not one S3 would apply to a bucket.
///
/// Each variant names the member that is wrong and the published constraint it breaks, so
/// the refusal's message states the rule the caller has to satisfy rather than one request's
/// wording. [`Self::code`] is the wire error code each one goes out with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsConfigurationRejection {
    /// A `CORSConfiguration` naming no `CORSRule` at all: `CORSConfiguration$CORSRules` is
    /// a REQUIRED member, so a document naming none does not validate against the schema
    /// S3 publishes.
    NoRules,
    /// More rules than [`MAX_CORS_RULES`].
    TooManyRules {
        /// How many the document carried.
        count: usize,
    },
    /// A rule naming no `AllowedMethod`: `CORSRule$AllowedMethods` is a REQUIRED member.
    NoAllowedMethods {
        /// Which rule carries it, counted from 1 as the document orders them.
        rule: usize,
    },
    /// A rule naming no `AllowedOrigin`: `CORSRule$AllowedOrigins` is a REQUIRED member.
    NoAllowedOrigins {
        /// Which rule carries it, counted from 1.
        rule: usize,
    },
    /// An `AllowedMethod` outside [`CORS_ALLOWED_METHODS`].
    UnsupportedMethod {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// The value that is not a published method.
        method: String,
    },
    /// An `ID` longer than [`MAX_CORS_RULE_ID_CHARS`] characters.
    IdTooLong {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// How many characters it carried.
        length: usize,
    },
    /// A `MaxAgeSeconds` below zero, which is no cache time: the member is "the time in
    /// seconds that your browser is to cache the preflight response".
    NegativeMaxAgeSeconds {
        /// Which rule carries it, counted from 1.
        rule: usize,
        /// The value that is not a time in seconds.
        seconds: i32,
    },
    /// A configuration whose document could not fit in
    /// [`MAX_CORS_CONFIGURATION_BYTES`]. See [`cors_configuration_bytes`].
    DocumentTooLarge {
        /// The size of the smallest document carrying these rules.
        bytes: usize,
    },
}

impl CorsConfigurationRejection {
    /// The wire error code S3 answers this refusal with.
    ///
    /// Two codes from S3's published "Error responses" list, both HTTP 400: a document that
    /// does not satisfy the schema the API publishes is [`MALFORMED_XML_CODE`] ("The XML
    /// provided does not match the schema"), and a document that satisfies the schema but
    /// carries a value or a count the API does not accept — an HTTP method outside the
    /// published set above all — is [`INVALID_REQUEST_CODE`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRules | Self::NoAllowedMethods { .. } | Self::NoAllowedOrigins { .. } => {
                MALFORMED_XML_CODE
            }
            Self::TooManyRules { .. }
            | Self::UnsupportedMethod { .. }
            | Self::IdTooLong { .. }
            | Self::NegativeMaxAgeSeconds { .. }
            | Self::DocumentTooLarge { .. } => INVALID_REQUEST_CODE,
        }
    }
}

impl std::fmt::Display for CorsConfigurationRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRules => write!(
                f,
                "CORSConfiguration.CORSRules names no CORSRule; a cors configuration \
                 carries at least one rule"
            ),
            Self::TooManyRules { count } => write!(
                f,
                "CORSConfiguration.CORSRules carries {count} rules, more than the \
                 {MAX_CORS_RULES} rules one cors configuration may hold"
            ),
            Self::NoAllowedMethods { rule } => write!(
                f,
                "CORSRule {rule}: AllowedMethods names no AllowedMethod; a cors rule \
                 allows at least one method"
            ),
            Self::NoAllowedOrigins { rule } => write!(
                f,
                "CORSRule {rule}: AllowedOrigins names no AllowedOrigin; a cors rule \
                 allows at least one origin"
            ),
            Self::UnsupportedMethod { rule, method } => write!(
                f,
                "CORSRule {rule}: AllowedMethod must be one of {}, got {method:?}",
                CORS_ALLOWED_METHODS.join(", ")
            ),
            Self::IdTooLong { rule, length } => write!(
                f,
                "CORSRule {rule}: ID is {length} characters long, more than the \
                 {MAX_CORS_RULE_ID_CHARS} characters a rule identifier may carry"
            ),
            Self::NegativeMaxAgeSeconds { rule, seconds } => write!(
                f,
                "CORSRule {rule}: MaxAgeSeconds is a time in seconds to cache the \
                 preflight response and cannot be negative, got {seconds}"
            ),
            Self::DocumentTooLarge { bytes } => write!(
                f,
                "the cors configuration takes {bytes} bytes, more than the \
                 {MAX_CORS_CONFIGURATION_BYTES} bytes a cors document may carry"
            ),
        }
    }
}

impl std::error::Error for CorsConfigurationRejection {}

/// Whether `rules` may be stored as a bucket's cors configuration, reporting the first
/// published rule they break.
///
/// Checked BEFORE anything is written, so a refused put leaves the bucket's current cors
/// configuration exactly as it was — the order every other bucket subresource put keeps.
/// The counts are judged before the per-rule values so an oversized document is refused for
/// what it is rather than for whichever rule happens to come first.
///
/// Two of the rules here (both required lists being non-empty) are ALSO what the modelled
/// `@required` traits express, and the boundary validator refuses a payload missing either
/// member before any handler runs. They are still checked here because this function, not
/// the wire, is what guards the STORE: a configuration no reader could report back is never
/// recorded, whoever writes it.
pub fn validate_cors_configuration(
    rules: &[BucketCorsRule],
) -> Result<(), CorsConfigurationRejection> {
    if rules.is_empty() {
        return Err(CorsConfigurationRejection::NoRules);
    }
    if rules.len() > MAX_CORS_RULES {
        return Err(CorsConfigurationRejection::TooManyRules { count: rules.len() });
    }
    let bytes = cors_configuration_bytes(rules);
    if bytes > MAX_CORS_CONFIGURATION_BYTES {
        return Err(CorsConfigurationRejection::DocumentTooLarge { bytes });
    }

    for (index, rule) in rules.iter().enumerate() {
        // Counted the way the document orders the rules, so the message names the rule a
        // caller can find.
        let position = index + 1;
        if rule.allowed_methods.is_empty() {
            return Err(CorsConfigurationRejection::NoAllowedMethods { rule: position });
        }
        if rule.allowed_origins.is_empty() {
            return Err(CorsConfigurationRejection::NoAllowedOrigins { rule: position });
        }
        for method in &rule.allowed_methods {
            if !CORS_ALLOWED_METHODS.contains(&method.as_str()) {
                return Err(CorsConfigurationRejection::UnsupportedMethod {
                    rule: position,
                    method: method.clone(),
                });
            }
        }
        if let Some(id) = rule.id.as_deref() {
            let length = id.chars().count();
            if length > MAX_CORS_RULE_ID_CHARS {
                return Err(CorsConfigurationRejection::IdTooLong {
                    rule: position,
                    length,
                });
            }
        }
        if let Some(seconds) = rule.max_age_seconds {
            if seconds < 0 {
                return Err(CorsConfigurationRejection::NegativeMaxAgeSeconds {
                    rule: position,
                    seconds,
                });
            }
        }
    }

    Ok(())
}

/// The lifecycle expiration in force on one stored object: when the rule that applies to
/// it expires it, and which rule that is.
///
/// Derived from the bucket's configuration and the object, never stored: an object moves
/// into and out of a rule as its tag set, its size and the configuration change, so the
/// answer is computed per read ([`Bucket::object_expiration`]) rather than stamped on the
/// object when it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectExpiration {
    /// When the object expires.
    pub expires_at: SystemTime,
    /// The `ID` of the rule that expires it.
    pub rule_id: String,
}

impl ObjectExpiration {
    /// This expiration as the `Expiration` response member carries it: "It includes the
    /// expiry-date and rule-id key-value pairs providing object expiration information. The
    /// value of the rule-id is URL-encoded" (`GetObjectOutput$Expiration`), the expiry date
    /// as an HTTP date.
    ///
    /// `None` only when the instant cannot be rendered as an HTTP date, which a stored
    /// object's never is.
    pub fn header_value(&self) -> Option<String> {
        let expiry_date = ::aws_smithy_types::DateTime::from(self.expires_at)
            .fmt(::aws_smithy_types::date_time::Format::HttpDate)
            .ok()?;
        Some(format!(
            "expiry-date=\"{expiry_date}\", rule-id=\"{}\"",
            url_encoded_key(&self.rule_id)
        ))
    }
}

/// When a lifecycle rule aborts one in-progress multipart upload, and which rule does it.
///
/// Derived from the bucket's configuration and the upload, the way [`ObjectExpiration`] is
/// derived for an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartUploadAbort {
    /// When the incomplete upload is aborted.
    pub aborts_at: SystemTime,
    /// The `ID` of the rule that aborts it — what `x-amz-abort-rule-id` reports.
    pub rule_id: String,
}

/// The `ExpirationStatus` value that puts a rule in effect: "If 'Enabled', the rule is
/// currently being applied. If 'Disabled', the rule is not currently being applied"
/// (`LifecycleRule$Status`).
pub const LIFECYCLE_RULE_ENABLED: &str = "Enabled";

/// The instant `days` days after `from`, rounded up to the next midnight UTC.
///
/// How S3 dates a lifecycle action: Amazon S3 adds the number of days to the time the
/// object was created (or the upload was initiated) and rounds the result up to midnight
/// UTC of the following day, which is why an action's date is always a midnight and never
/// the time of day the write happened to land on.
fn midnight_after_days(from: SystemTime, days: i32) -> Option<SystemTime> {
    let from = i64::try_from(
        from.duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
    .ok()?;
    let due = from.checked_add(i64::from(days).checked_mul(SECONDS_PER_DAY)?)?;
    let whole_days = due.div_euclid(SECONDS_PER_DAY);
    // Already a midnight: there is nothing to round up to.
    let midnight = if due.rem_euclid(SECONDS_PER_DAY) == 0 {
        whole_days
    } else {
        whole_days.checked_add(1)?
    }
    .checked_mul(SECONDS_PER_DAY)?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(midnight).ok()?))
}

/// The instant `date` names, or `None` when it is outside the range a [`SystemTime`] on
/// this platform can hold.
///
/// The ONE place a modelled timestamp becomes an instant the store keeps, whichever member
/// carried it: the midnight UTC a lifecycle `Date` names, and the end of the period an
/// object version's retention runs to ([`ObjectRetention::retain_until`]), so the value
/// `GetObjectRetention` reports back is the one the write recorded.
/// `::aws_smithy_types::DateTime::from(SystemTime)` is the other direction, and both are
/// the crate's own conversions — nothing here re-derives epoch arithmetic, which is also
/// what lets a date BEFORE the epoch through rather than refusing it.
pub fn instant_of(date: &::aws_smithy_types::DateTime) -> Option<SystemTime> {
    SystemTime::try_from(*date).ok()
}

/// Whether `tags` carries `tag`'s key with `tag`'s value — "this tag must exist in the
/// object's tag set in order for the rule to apply" (`LifecycleRuleFilter$Tag`).
fn carries_tag(tags: &[ResourceTag], tag: &crate::ops::types::Tag) -> bool {
    tags.iter()
        .any(|carried| carried.key == tag.key && carried.value == tag.value)
}

/// The key prefix `rule` applies to: the legacy `Prefix`, the filter's own `Prefix`, or the
/// one inside its `And` — and the empty prefix, which every key starts with, for a rule
/// that names none.
fn lifecycle_rule_prefix(rule: &crate::ops::types::LifecycleRule) -> &str {
    if let Some(prefix) = &rule.prefix {
        return prefix.as_str();
    }
    match &rule.filter {
        None => "",
        Some(filter) => filter
            .prefix
            .as_deref()
            .or_else(|| filter.and.as_ref().and_then(|and| and.prefix.as_deref()))
            .unwrap_or(""),
    }
}

/// Whether `rule` applies to an object at `key` of `size` bytes carrying `tags`.
///
/// The predicates are `LifecycleRuleFilter`'s own: a key prefix, a tag the object's tag set
/// must carry, a minimum object size and a maximum one, with `And` requiring all of the
/// ones it names at once. A filter that names none "applies to all objects in the bucket"
/// (that shape's own documentation), as does a rule whose only predicate is the legacy
/// `Prefix`.
fn lifecycle_rule_applies(
    rule: &crate::ops::types::LifecycleRule,
    key: &str,
    size: i64,
    tags: &[ResourceTag],
) -> bool {
    if !key.starts_with(lifecycle_rule_prefix(rule)) {
        return false;
    }
    let Some(filter) = &rule.filter else {
        // A legacy `Prefix` rule: the prefix above is the whole of what it selects on.
        return true;
    };
    if let Some(tag) = &filter.tag {
        if !carries_tag(tags, tag) {
            return false;
        }
    }
    if !object_size_matches(size, filter.object_size_greater_than, filter.object_size_less_than) {
        return false;
    }
    if let Some(and) = &filter.and {
        if and.tags.iter().flatten().any(|tag| !carries_tag(tags, tag)) {
            return false;
        }
        if !object_size_matches(size, and.object_size_greater_than, and.object_size_less_than) {
            return false;
        }
    }
    true
}

/// Whether an object of `size` bytes is inside the bounds a filter names: strictly above
/// the "minimum object size to which the rule applies" and strictly below the maximum
/// (`LifecycleRuleFilter$ObjectSizeGreaterThan` / `$ObjectSizeLessThan`).
fn object_size_matches(size: i64, greater_than: Option<i64>, less_than: Option<i64>) -> bool {
    greater_than.is_none_or(|minimum| size > minimum)
        && less_than.is_none_or(|maximum| size < maximum)
}

/// The `TableStatus` a metadata table carries while it is still being created:
/// "`CREATING` - The journal table is in the process of being created in the specified
/// table bucket" (`smithy_query(query="doc",shape="JournalTableConfigurationResult",
/// member="TableStatus")`).
pub const METADATA_TABLE_STATUS_CREATING: &str = "CREATING";

/// The `TableStatus` a metadata table carries once it is in service: "`ACTIVE` - The
/// journal table has been created successfully, and records are being delivered to the
/// table" (same member's documentation).
pub const METADATA_TABLE_STATUS_ACTIVE: &str = "ACTIVE";

/// The `TableStatus` a metadata table carries when it could not be created: "`FAILED` -
/// Amazon S3 is unable to create the journal table, or Amazon S3 is unable to deliver
/// records" (same member's documentation). A table in this status carries the `ErrorDetails`
/// saying why — see [`MetadataTableFailure`].
pub const METADATA_TABLE_STATUS_FAILED: &str = "FAILED";

/// The `ErrorDetails$ErrorCode` provisioning reports when the journal table it would create
/// is already in the managed table bucket's namespace: "`JournalTableAlreadyExists` - A
/// journal table already exists in the Amazon Web Services managed table bucket's namespace"
/// (`smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`, the V2 list).
pub const METADATA_JOURNAL_TABLE_EXISTS_CODE: &str = "JournalTableAlreadyExists";

/// The `ErrorDetails$ErrorMessage` that code carries — the FIXED sentence S3 sends with it,
/// naming neither the table nor the bucket.
pub const METADATA_JOURNAL_TABLE_EXISTS_MESSAGE: &str =
    "A journal table already exists in the AWS managed table bucket's namespace. Delete the \
     journal table, and then try again. To create a new S3 Metadata table, you must delete \
     the S3 Metadata configuration for this bucket, and then create a new S3 Metadata \
     configuration.";

/// The same, for the INVENTORY table: "`InventoryTableAlreadyExists` - An inventory table
/// already exists in the Amazon Web Services managed table bucket's namespace" (same list).
pub const METADATA_INVENTORY_TABLE_EXISTS_CODE: &str = "InventoryTableAlreadyExists";

/// See [`METADATA_INVENTORY_TABLE_EXISTS_CODE`].
pub const METADATA_INVENTORY_TABLE_EXISTS_MESSAGE: &str =
    "An inventory table already exists in the AWS managed table bucket's namespace. Delete \
     the inventory table, and then try again. To create a new S3 Metadata table, you must \
     delete the S3 Metadata configuration for this bucket, and then create a new S3 \
     Metadata configuration.";

/// The code an inventory table carries when the journal table it depends on could not be
/// created: "`JournalTableNotAvailable` - The journal table that the inventory table relies
/// on has a `FAILED` status. An inventory table requires a journal table with an `ACTIVE`
/// status" (same published list).
pub const METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE: &str = "JournalTableNotAvailable";

/// See [`METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE`].
pub const METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE: &str =
    "The journal table that the inventory table relies on has a FAILED status. An inventory \
     table requires a journal table with an ACTIVE status. To create a new journal or \
     inventory table, you must delete the metadata configuration for this bucket, along with \
     any journal or inventory tables, and then create a new metadata configuration.";

/// How long after the call that asked for it a metadata table reaches
/// [`METADATA_TABLE_STATUS_ACTIVE`] in this emulator.
///
/// A CHOSEN span, not an AWS-documented duration: what IS faithful is that the table is
/// NOT in service when the create returns — the status member publishes a transient
/// `CREATING` value for exactly that window — so a caller reading the configuration
/// straight after the create still observes `CREATING`, as it would against AWS. The
/// instant it falls due is stored ([`MetadataTable::active_at`]) and nothing sleeps for
/// it, the way [`RESTORE_COMPLETION_DELAY`] is handled.
pub const METADATA_TABLE_ACTIVATION_DELAY: std::time::Duration =
    std::time::Duration::from_millis(500);

/// `DestinationResult$TableBucketType` of a bucket's metadata configuration: "V2 metadata
/// configurations are stored in Amazon Web Services managed table buckets"
/// (`smithy_query(query="doc",shape="DestinationResult",member="TableBucketType")`), and
/// `aws` is the value that "indicates an Amazon Web Services managed table bucket". One of
/// `S3TablesBucketType`'s own two values.
pub const METADATA_TABLE_BUCKET_TYPE: &str = "aws";

/// The name of the Amazon Web Services managed table bucket this emulator reports a
/// metadata configuration's tables as stored in.
///
/// ONE managed bucket per Region, shared by every configuration, which is what the `aws`
/// bucket type means: the caller names no table bucket on a V2 create and AWS does not
/// publish the managed bucket's name, so this is the name this emulator reports. It
/// satisfies the form every S3 Tables operation accepts for a table bucket
/// (`[a-z0-9_-]{3,63}`, from the `TableBucketARN` pattern those requests publish).
///
/// REPORTED-ONLY: no table bucket is created in the `s3tables` emulator behind this ARN,
/// so a caller cannot follow it into that service and find one. The bucket is that
/// service's resource and this service owns no part of its state, so the ARN names where
/// the metadata tables WOULD live — which is all `GetBucketMetadataConfiguration` reports
/// about them.
pub const METADATA_TABLE_BUCKET_NAME: &str = "aws-s3";

/// What the namespace holding ONE general purpose bucket's metadata tables begins with —
/// "the namespace in the table bucket where the metadata tables for a metadata
/// configuration are stored" (`smithy_query(query="doc",shape="DestinationResult",
/// member="TableNamespace")`).
///
/// Every general purpose bucket's metadata tables live in a namespace OF THEIR OWN inside
/// the managed table bucket, named for the bucket they describe
/// ([`metadata_table_namespace`]) — which is what lets the tables inside it be named for
/// nothing but which of the three they are ([`METADATA_JOURNAL_TABLE_NAME`]).
pub const METADATA_TABLE_NAMESPACE_PREFIX: &str = "b_";

/// The service namespace of a metadata table's (and its table bucket's) ARN, so one reads
/// `arn:aws:s3tables:<region>:<account>:bucket/<bucket>` and a table's
/// `…:bucket/<bucket>/table/<table>` — the two forms every S3 Tables request publishes a
/// pattern for.
const METADATA_TABLE_ARN_SERVICE: &str = "s3tables";

/// The name of the JOURNAL table of a metadata configuration
/// (`JournalTableConfigurationResult$TableName`).
///
/// The tables of a V2 configuration are S3-MANAGED: the caller names none of them, and each
/// general purpose bucket's three tables are alone in a namespace of their own
/// ([`metadata_table_namespace`]), so a table's name says which of the three it is and
/// nothing else — the coordinates a client builds a table reference from are that namespace
/// plus this name.
pub const METADATA_JOURNAL_TABLE_NAME: &str = "journal";
/// See [`METADATA_JOURNAL_TABLE_NAME`] — the INVENTORY table's name
/// (`InventoryTableConfigurationResult$TableName`).
pub const METADATA_INVENTORY_TABLE_NAME: &str = "inventory";
/// See [`METADATA_JOURNAL_TABLE_NAME`] — the ANNOTATION table's name
/// (`AnnotationTableConfigurationResult$TableName`).
pub const METADATA_ANNOTATION_TABLE_NAME: &str = "annotation";

/// The `ConfigurationState` value that asks for a table to be created, for both the
/// inventory table (`InventoryConfigurationState`) and the annotation table
/// (`AnnotationConfigurationState`) — each enum publishes `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape",shape="InventoryConfigurationState,AnnotationConfigurationState")`).
pub const METADATA_CONFIGURATION_STATE_ENABLED: &str = "ENABLED";

/// The other value those two enums publish: the state of a table a configuration does NOT
/// ask for, which is what a reader reports for a table the configuration never named — the
/// `ConfigurationState` of every optional table result is required, so the container is
/// reported whatever state the table is in.
pub const METADATA_CONFIGURATION_STATE_DISABLED: &str = "DISABLED";

/// The `TableSseAlgorithm` value that encrypts a metadata table with a Key Management
/// Service key, and so the one value that needs a key ARN beside it: the other value that
/// enum publishes, `AES256`, is S3 managed encryption and takes none
/// (`smithy_query(query="shape",shape="TableSseAlgorithm")`).
pub const METADATA_TABLE_SSE_KMS_ALGORITHM: &str = "aws:kms";

/// The fewest days journal table records may be retained for: "Journal table records must
/// be retained for a minimum of 7 days. To set this value, specify any whole number from
/// `7` to `2147483647`" (`smithy_query(query="doc",shape="RecordExpiration",
/// member="Days")`).
pub const MIN_METADATA_RECORD_EXPIRATION_DAYS: i32 = 7;

/// The most days journal table records may be retained for — the upper end of the same
/// published range.
pub const MAX_METADATA_RECORD_EXPIRATION_DAYS: i32 = 2_147_483_647;

/// The `<Message>` S3 sends beside [`INVALID_REQUEST_CODE`] for a journal table record
/// expiration that does not state a usable retention period.
///
/// FIXED text stating the rule, with nothing of the request in it: it names the member to
/// set, the published range it has to be in and what to do next, which is the whole of what a
/// caller has to act on ([`metadata_record_expiration_refusal`] decides when it goes out).
/// The two numbers are [`MIN_METADATA_RECORD_EXPIRATION_DAYS`] and
/// [`MAX_METADATA_RECORD_EXPIRATION_DAYS`] as that sentence spells them.
pub const INVALID_RECORD_EXPIRATION_MESSAGE: &str =
    "When RecordExpiration is set to ENABLED, the Days value must be specified. This value \
     must be a whole number from 7 to 2147483647. Specify a valid Days value, and then try \
     again.";

/// The longest IAM role ARN this emulator records for an annotation table.
///
/// An emulator resource guard, NOT an AWS quota: `Role` is "The ARN of the IAM role used
/// to manage the annotation table" (`AnnotationTableConfiguration$Role`) and the model
/// bounds its length at neither end, so this is what keeps the one caller-supplied string
/// a metadata configuration retains from growing without limit. 2048 is the longest ARN
/// AWS accepts anywhere in an IAM request.
pub const MAX_METADATA_TABLE_ROLE_CHARS: usize = 2048;

/// The most metadata tables this emulator records in the managed table bucket's namespace
/// ([`ComAmazonawsS3Amazons3State::metadata_namespace_holds`]).
///
/// An emulator resource guard, NOT an AWS quota. A table that reached service OUTLIVES the
/// configuration that created it, and nothing in this service's API deletes a table from an
/// S3 Tables namespace — only the name of a table that never came into service is released
/// with the configuration that asked for it
/// ([`ComAmazonawsS3Amazons3State::release_unprovisioned_metadata_tables`]) — so without a
/// ceiling a caller creating and deleting buckets could grow it for the life of the process.
/// Three
/// tables (journal, inventory, annotation) for as many general purpose buckets as this
/// account may hold ([`MAX_BUCKETS_PER_ACCOUNT`]) is the most any sequence of creates can
/// legitimately need; a create that would carry the namespace past it is refused with
/// nothing recorded.
pub const MAX_METADATA_NAMESPACE_TABLES: usize = 3 * MAX_BUCKETS_PER_ACCOUNT;

/// The `InvalidArgument` message the encryption configuration carried in the request member
/// `member` earns, or `None` when there is nothing wrong with it.
///
/// "If server-side encryption with Key Management Service (KMS) keys (SSE-KMS) is specified,
/// you must also specify the KMS key Amazon Resource Name (ARN)"
/// (`MetadataTableEncryptionConfiguration$KmsKeyArn`): ONE rule for every table that carries
/// that container, so the create that names all three tables and each update that names one
/// cannot drift apart on it. The other algorithm `TableSseAlgorithm` publishes, `AES256`, is
/// S3 managed encryption and takes no key.
pub fn metadata_table_encryption_refusal(
    member: &str,
    encryption: Option<&crate::ops::types::MetadataTableEncryptionConfiguration>,
) -> Option<String> {
    let encryption = encryption?;
    (encryption.sse_algorithm == METADATA_TABLE_SSE_KMS_ALGORITHM
        && encryption.kms_key_arn.is_none())
    .then(|| {
        format!(
            "{member}.EncryptionConfiguration names SseAlgorithm {:?} without a \
             KmsKeyArn; a KMS key ARN is required with that algorithm",
            encryption.sse_algorithm
        )
    })
}

/// The `InvalidRequest` message the journal table record expiration settings `expiration`
/// earn, or `None` when there is nothing wrong with them.
///
/// "If you enable journal table record expiration, you can set the number of days to retain
/// your journal table records. Journal table records must be retained for a minimum of 7
/// days. To set this value, specify any whole number from `7` to `2147483647`"
/// (`RecordExpiration$Days`): the model bounds that member at neither end, so a day count
/// outside the published range reaches a handler. Recorded behaviour: S3 answers that class
/// [`INVALID_REQUEST_CODE`] carrying the fixed sentence
/// [`INVALID_RECORD_EXPIRATION_MESSAGE`], which states the rule and nothing of the request.
/// A retention period is what the member bounds, so settings that name NO day count are
/// nothing for this rule to judge — the one place that decides it, for every request that
/// carries a `RecordExpiration`: the create that settles a configuration's journal table and
/// the update that changes its record expiration, so the two cannot drift apart on it.
pub fn metadata_record_expiration_refusal(
    expiration: &crate::ops::types::RecordExpiration,
) -> Option<&'static str> {
    let days = expiration.days?;
    (!(MIN_METADATA_RECORD_EXPIRATION_DAYS..=MAX_METADATA_RECORD_EXPIRATION_DAYS).contains(&days))
        .then_some(INVALID_RECORD_EXPIRATION_MESSAGE)
}

/// The `ErrorDetails` an inventory table being provisioned NOW carries, or `None` when
/// nothing stops it from being created: `name_taken` says the managed table bucket's
/// namespace already holds the name it would be created under, and `journal_failed` that the
/// journal table it reads from could not be created.
///
/// ONE rule for every request that provisions an inventory table — the create that names a
/// whole configuration and the update that enables the table on one — so the two cannot
/// drift apart on it. Its OWN name is judged first: that is what stops a second table being
/// created under a name a table already holds ([`METADATA_INVENTORY_TABLE_EXISTS_CODE`]),
/// and only a table whose name is free can then report the journal table it relies on —
/// "the journal table that the inventory table relies on has a `FAILED` status. An inventory
/// table requires a journal table with an `ACTIVE` status"
/// ([`METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE`], from
/// `smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`'s V2 list).
pub fn metadata_inventory_table_failure(
    name_taken: bool,
    journal_failed: bool,
) -> Option<MetadataTableFailure> {
    if name_taken {
        Some(MetadataTableFailure {
            code: METADATA_INVENTORY_TABLE_EXISTS_CODE,
            message: METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
        })
    } else if journal_failed {
        Some(MetadataTableFailure {
            code: METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
            message: METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
        })
    } else {
        None
    }
}

/// What taking the S3 Metadata configuration of a general purpose bucket away did — the
/// outcomes the caller of
/// [`ComAmazonawsS3Amazons3State::delete_metadata_configuration`] answers differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataConfigurationDeletion {
    /// The bucket carried a configuration and carries none now.
    Deleted,
    /// This store holds no such general purpose bucket.
    NoSuchBucket,
    /// The bucket is there and carried NO metadata configuration, so there was none to
    /// delete: the condition this family names for itself
    /// ([`METADATA_CONFIGURATION_NOT_FOUND_CODE`]).
    NoConfiguration,
}

/// What enabling or disabling the inventory table of a bucket's metadata configuration did —
/// the outcomes the caller of
/// [`ComAmazonawsS3Amazons3State::update_metadata_inventory_table`] answers differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataInventoryTableUpdate {
    /// Recorded: the state the request named is the configuration's and the table behind it
    /// followed. A table provisioned under a name the namespace already holds is recorded
    /// [`METADATA_TABLE_STATUS_FAILED`], which is a provisioning outcome a READER reports
    /// rather than a refusal of this call.
    Recorded,
    /// This store holds no such general purpose bucket.
    NoSuchBucket,
    /// The bucket carries no metadata configuration, so there is no inventory table of one to
    /// enable or disable and nothing is written.
    NoConfiguration,
    /// The enable would provision a table under a name the managed table bucket's namespace
    /// has no room left for ([`MAX_METADATA_NAMESPACE_TABLES`]) — this emulator's own
    /// ceiling, with nothing written.
    NamespaceFull,
}

/// One table of a bucket's S3 Metadata configuration: its name, and when it falls due to
/// be in service.
///
/// Nothing here is table CONTENT: the Iceberg table itself is an S3 Tables resource this
/// service owns no part of. What is recorded is what every reader of the configuration
/// reports about the table — the name S3 created it under and the status it is in
/// ([`Self::status_at`]).
#[derive(Debug, Clone)]
pub struct MetadataTable {
    /// Which of the configuration's three tables this is: one of
    /// [`METADATA_JOURNAL_TABLE_NAME`], [`METADATA_INVENTORY_TABLE_NAME`] and
    /// [`METADATA_ANNOTATION_TABLE_NAME`].
    pub name: String,
    /// The namespace of the managed table bucket the table was created in — the general
    /// purpose bucket's own ([`metadata_table_namespace`]).
    pub namespace: String,
    /// When the table falls due to be in service — the instant it stops being reported as
    /// `CREATING`, [`METADATA_TABLE_ACTIVATION_DELAY`] after the call that asked for it.
    pub active_at: SystemTime,
    /// Why the table could not be created, or `None` for one whose creation is under way.
    ///
    /// A table that cannot be created never reaches `ACTIVE`: it is reported `FAILED` with
    /// the `ErrorDetails` this record carries ([`MetadataTableFailure`]), from the create
    /// onwards.
    pub failure: Option<MetadataTableFailure>,
}

/// Why a metadata table could not be created: the `ErrorDetails` a reader reports beside its
/// `FAILED` status.
///
/// The members are `ErrorDetails`' own
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ErrorDetails")`): the error code and
/// the message, each a published pair from that shape's own documentation — so the code and
/// the sentence a caller reads are AWS's rather than anything built here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataTableFailure {
    /// An `ErrorDetails$ErrorCode` value, e.g. [`METADATA_JOURNAL_TABLE_EXISTS_CODE`].
    pub code: &'static str,
    /// The `ErrorDetails$ErrorMessage` that code publishes.
    pub message: &'static str,
}

impl MetadataTable {
    /// The table `name` of the namespace `namespace`, as the call that asked for it at
    /// `created_at` records it: being created now, in service a span later.
    pub fn creating(namespace: &str, name: &str, created_at: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            namespace: namespace.to_string(),
            active_at: created_at + METADATA_TABLE_ACTIVATION_DELAY,
            failure: None,
        }
    }

    /// The same table that could not be created, with `failure` saying why: reported
    /// `FAILED` from the create onwards rather than ever falling due.
    pub fn failed(
        namespace: &str,
        name: &str,
        created_at: SystemTime,
        failure: MetadataTableFailure,
    ) -> Self {
        Self {
            failure: Some(failure),
            ..Self::creating(namespace, name, created_at)
        }
    }

    /// The table's name QUALIFIED by the namespace it lives in
    /// ([`metadata_qualified_table_name`]) — what identifies it inside the managed table
    /// bucket, since the three table names repeat in every bucket's namespace.
    pub fn qualified_name(&self) -> String {
        metadata_qualified_table_name(&self.namespace, &self.name)
    }

    /// The `TableStatus` the table is in at `instant`: `FAILED` for a table that could not
    /// be created, else being created until it falls due and in service from then on.
    pub fn status_at(&self, instant: SystemTime) -> &'static str {
        if self.failure.is_some() {
            METADATA_TABLE_STATUS_FAILED
        } else if instant < self.active_at {
            METADATA_TABLE_STATUS_CREATING
        } else {
            METADATA_TABLE_STATUS_ACTIVE
        }
    }

    /// Whether this table's creation was still under way at `instant` — the one state in
    /// which the configuration carrying it leaves NOTHING standing under its name.
    ///
    /// A table in service holds the name it was created under until that table is deleted
    /// through the S3 Tables API that owns it, and a table that FAILED never held its name at
    /// all — whatever table is really there does — so a table still being created is the only
    /// one whose name is free again once the configuration it belongs to is taken away. See
    /// [`ComAmazonawsS3Amazons3State::release_unprovisioned_metadata_tables`], which is what
    /// reads it.
    pub fn creating_at(&self, instant: SystemTime) -> bool {
        self.status_at(instant) == METADATA_TABLE_STATUS_CREATING
    }
}

/// One of the OPTIONAL tables of a metadata configuration — the inventory table or the
/// annotation table — as the configuration names it.
///
/// The state is what the request carried, and the TABLE is there exactly when that state
/// is [`METADATA_CONFIGURATION_STATE_ENABLED`]: a disabled table is a configuration with
/// no table behind it, which is why `InventoryTableConfigurationResult` requires its
/// `ConfigurationState` and leaves its `TableStatus`, `TableName` and `TableArn` optional
/// (`smithy_query(query="shape",shape="InventoryTableConfigurationResult")`).
///
/// ONE type for both, because both are the same record: a state, the table it has while
/// that state is enabled, and — for the annotation table alone — the IAM role that
/// manages it (`AnnotationTableConfiguration$Role`; the inventory table has no such
/// member, so its `role` is always `None`).
#[derive(Debug, Clone)]
pub struct MetadataTableConfiguration {
    /// The `ConfigurationState` the request named for this table.
    pub configuration_state: String,
    /// The table itself, or `None` while the state above is not enabled.
    pub table: Option<MetadataTable>,
    /// "The ARN of the IAM role used to manage the annotation table"
    /// (`AnnotationTableConfiguration$Role`), for the annotation table and nothing else.
    pub role: Option<String>,
}

/// Record `configuration_state` as the state of the optional table configuration `slot`,
/// with `table_name` the name a table this creates is created under, `role` the IAM role the
/// call names for the table (`None` when it names none), `failure` why a table this creates
/// could not be created (`None` when nothing stops it), and `now` the instant of the call
/// asking for it.
///
/// The shape of enabling or disabling one of a metadata configuration's optional tables: the
/// state the request names becomes the table's state, and the table behind it follows —
/// enabling a table the configuration has none of creates one, being created from `now`
/// ([`METADATA_TABLE_ACTIVATION_DELAY`] later in service) or reported `FAILED` with `failure`
/// when provisioning it could not succeed, and disabling one drops it, which is what leaves
/// the reported result carrying a state with no table name, status or ARN. Enabling a table
/// that is already there leaves that table exactly as it is, so a repeated enable neither
/// restarts its creation nor renames it — and `failure` is therefore about the table this
/// call would CREATE, never about one already there.
///
/// `role` is "the new IAM role ARN to apply" (`AnnotationTableConfigurationUpdates$Role`) —
/// for the annotation table, the only optional table managed by one: a call that names one
/// replaces the role in effect, and a call that names none leaves whatever role the
/// configuration carried in place (the inventory table has no such member, so its caller
/// names none and its `role` stays `None`).
fn set_optional_metadata_table_state(
    slot: &mut Option<MetadataTableConfiguration>,
    configuration_state: String,
    role: Option<String>,
    namespace: &str,
    table_name: &str,
    failure: Option<MetadataTableFailure>,
    now: SystemTime,
) {
    let enabled = configuration_state == METADATA_CONFIGURATION_STATE_ENABLED;
    let carried = slot.take();
    let carried_table = carried.as_ref().and_then(|table| table.table.clone());
    *slot = Some(MetadataTableConfiguration {
        configuration_state,
        table: enabled.then(|| {
            carried_table.unwrap_or_else(|| match failure {
                Some(failure) => MetadataTable::failed(namespace, table_name, now, failure),
                None => MetadataTable::creating(namespace, table_name, now),
            })
        }),
        role: role.or_else(|| carried.and_then(|table| table.role)),
    });
}

/// The S3 Metadata configuration a general purpose bucket carries, as
/// `CreateBucketMetadataConfiguration` recorded it.
///
/// A bucket carries at most one: the create "creates an S3 Metadata V2 metadata
/// configuration for a general purpose bucket" (that operation's own documentation) and
/// every other operation on it names only the bucket, so this lives behind an `Option` on
/// [`Bucket`] and a second create while one is there is refused rather than recorded.
///
/// The JOURNAL table is not optional, because `MetadataConfiguration$JournalTableConfiguration`
/// is a required member: a configuration always has one, and the inventory and annotation
/// tables are there only when the create named them.
#[derive(Debug, Clone)]
pub struct BucketMetadataConfiguration {
    /// The journal table, which every configuration has.
    pub journal: MetadataTable,
    /// "The journal table record expiration settings for the journal table"
    /// (`JournalTableConfiguration$RecordExpiration`), stored as the modelled container
    /// the request carried — the way [`Bucket::replication`] keeps its own — so a reader
    /// reports the settings the bucket actually carries rather than a re-derived copy.
    pub journal_record_expiration: crate::ops::types::RecordExpiration,
    /// The inventory table configuration, or `None` when the create named none.
    pub inventory: Option<MetadataTableConfiguration>,
    /// The annotation table configuration, or `None` when the create named none.
    pub annotation: Option<MetadataTableConfiguration>,
}

impl BucketMetadataConfiguration {
    /// The namespace of the managed table bucket this configuration's tables live in — the
    /// general purpose bucket's own ([`metadata_table_namespace`]), which
    /// `DestinationResult$TableNamespace` reports.
    ///
    /// Read off the JOURNAL table, which every configuration has, so what is reported is the
    /// namespace the tables were really created in rather than a second copy of the rule.
    pub fn table_namespace(&self) -> &str {
        &self.journal.namespace
    }

    /// Every table this configuration has: the journal table, which it always has, and the
    /// optional inventory and annotation tables while their state has one behind it.
    ///
    /// The read seam for anything that acts on a configuration's tables as a SET — what
    /// taking the configuration away releases from the managed table bucket's namespace — so
    /// no caller re-derives which of the three containers carries a table of its own.
    pub fn tables(&self) -> impl Iterator<Item = &MetadataTable> + '_ {
        [
            Some(&self.journal),
            self.inventory
                .as_ref()
                .and_then(|inventory| inventory.table.as_ref()),
            self.annotation
                .as_ref()
                .and_then(|annotation| annotation.table.as_ref()),
        ]
        .into_iter()
        .flatten()
    }
}

/// The namespace of the managed table bucket that holds the metadata tables of the general
/// purpose bucket `bucket`: [`METADATA_TABLE_NAMESPACE_PREFIX`] and the bucket's own name.
///
/// Derived from the bucket's own name rather than minted, so the namespace a reader reports
/// for one bucket is the same on every read, and one bucket's tables are never in another's
/// namespace.
pub fn metadata_table_namespace(bucket: &str) -> String {
    format!("{METADATA_TABLE_NAMESPACE_PREFIX}{bucket}")
}

/// The name `table` of the namespace `namespace`, as one value — what identifies one
/// metadata table inside the managed table bucket.
///
/// The three table names ([`METADATA_JOURNAL_TABLE_NAME`] and the other two) are the same in
/// every bucket's namespace, so a name alone does not tell two buckets' journal tables
/// apart: what does is the namespace it is in. This is the one place that joins the two, so
/// the record of which tables the managed table bucket holds
/// ([`ComAmazonawsS3Amazons3State::metadata_namespace_holds`]) and the id a table's ARN
/// carries cannot drift apart.
///
/// Written in the character set an S3 Tables name is written in — every S3 Tables request
/// publishes `^[0-9a-z_]*$` for a `NamespaceName` and a `TableName`, and
/// `[a-zA-Z0-9-_]{1,255}` for the table portion of a table ARN — so each character of the
/// general purpose bucket's name outside that set becomes an underscore. Two bucket names
/// that differ only in the characters this folds (a dot where another has a hyphen) therefore
/// identify the same table, which is what the `*AlreadyExists` provisioning failures report.
pub fn metadata_qualified_table_name(namespace: &str, table: &str) -> String {
    format!("{namespace}_{table}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The ARN of the managed table bucket a metadata configuration's tables live in, in
/// `region` — see [`METADATA_TABLE_BUCKET_NAME`] for what it does and does not claim.
pub fn metadata_table_bucket_arn(region: &str) -> String {
    ::engine::emulator_arn(
        METADATA_TABLE_ARN_SERVICE,
        region,
        &format!("bucket/{METADATA_TABLE_BUCKET_NAME}"),
    )
}

/// The ARN of the metadata table `table` of the namespace `namespace` in `region`: the table
/// bucket's own ARN above with the table's id appended, which is the form every S3 Tables
/// request publishes for a table ARN.
///
/// The id is the table's name qualified by its namespace
/// ([`metadata_qualified_table_name`]), because a table ARN identifies ONE table of the
/// table bucket: the last segment of an S3 Tables table ARN is the table's own id rather
/// than its name, and the bare name repeats in every bucket's namespace.
pub fn metadata_table_arn(region: &str, namespace: &str, table: &str) -> String {
    ::engine::emulator_arn(
        METADATA_TABLE_ARN_SERVICE,
        region,
        &format!(
            "bucket/{METADATA_TABLE_BUCKET_NAME}/table/{}",
            metadata_qualified_table_name(namespace, table)
        ),
    )
}

/// Where a DIRECTORY bucket is placed, and how its data is kept there.
///
/// Both members come from the `CreateBucketConfiguration` the create carried: "the location
/// type is Availability Zone or Local Zone" and, for a directory bucket, "the name of the
/// location is the Zone ID of the Availability Zone (AZ) or Local Zone (LZ) where the
/// bucket will be created" (`LocationInfo`), with "the number of Zone … that's used for
/// redundancy for the bucket" beside it (`BucketInfo$DataRedundancy`). Recorded as the
/// create settled them, so every later answer about the bucket reports the placement it was
/// created with rather than one re-derived from its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryBucketPlacement {
    /// The `LocationType` value the create named: one of [`LOCATION_TYPE_VALUES`].
    pub location_type: String,
    /// The Zone ID the create named, which is also the zone segment of the bucket's own
    /// name ([`directory_bucket_zone`]).
    pub zone_id: String,
    /// The `DataRedundancy` value the create named, or the one its location type implies
    /// ([`implied_data_redundancy`]).
    pub data_redundancy: String,
}

/// The `BucketType` value that asks `CreateBucket` for a directory bucket — the single
/// value that enum publishes (`smithy_query(query="shape",shape="BucketType")`).
pub const DIRECTORY_BUCKET_TYPE: &str = "Directory";

/// The `LocationType` value for an Availability Zone, the first of that enum's two values.
pub const LOCATION_TYPE_AVAILABILITY_ZONE: &str = "AvailabilityZone";

/// The `LocationType` value for a Local Zone, the other one.
pub const LOCATION_TYPE_LOCAL_ZONE: &str = "LocalZone";

/// The `DataRedundancy` value a bucket in an Availability Zone is stored with.
pub const DATA_REDUNDANCY_SINGLE_AVAILABILITY_ZONE: &str = "SingleAvailabilityZone";

/// The `DataRedundancy` value a bucket in a Local Zone is stored with.
pub const DATA_REDUNDANCY_SINGLE_LOCAL_ZONE: &str = "SingleLocalZone";

/// The redundancy a directory bucket in `location_type` is kept with when the create named
/// none: one zone, the zone it was placed in — the `DataRedundancy` value that names that
/// zone's kind ("the number of Zone (Availability Zone or Local Zone) that's used for
/// redundancy for the bucket", `BucketInfo$DataRedundancy`, whose two values are exactly
/// the two location types).
pub fn implied_data_redundancy(location_type: &str) -> &'static str {
    if location_type == LOCATION_TYPE_LOCAL_ZONE {
        DATA_REDUNDANCY_SINGLE_LOCAL_ZONE
    } else {
        DATA_REDUNDANCY_SINGLE_AVAILABILITY_ZONE
    }
}

/// The suffix every directory bucket's name ends with.
///
/// "Directory bucket names must … end with the suffix `--x-s3`" and carry the zone before
/// it: the name is `<base>--<zone-id>--x-s3` (Amazon S3 User Guide, *Directory bucket
/// naming rules*), which is why the zone a request's bucket lives in can be read off its
/// name at all ([`directory_bucket_zone`]).
pub const DIRECTORY_BUCKET_SUFFIX: &str = "--x-s3";

/// The separator the base name, the zone id and the suffix of a directory bucket's name are
/// joined with — two hyphens, which is what makes a zone id (itself hyphenated, `usw2-az1`)
/// readable out of the name.
const DIRECTORY_BUCKET_SEPARATOR: &str = "--";

/// The Zone ID in the directory-bucket name `name`, or `None` for a name that is not a
/// directory bucket's.
///
/// The rule is the published naming rule above, read in one place so the create that places
/// a bucket in a zone and anything that later asks which zone a name claims agree.
pub fn directory_bucket_zone(name: &str) -> Option<&str> {
    let base = name.strip_suffix(DIRECTORY_BUCKET_SUFFIX)?;
    let (stem, zone_id) = base.rsplit_once(DIRECTORY_BUCKET_SEPARATOR)?;
    (!stem.is_empty() && !zone_id.is_empty()).then_some(zone_id)
}

/// The ARN of the directory bucket `bucket` in `region`: `CreateBucket` reports one for a
/// directory bucket and for no other kind ("this parameter is only supported for S3
/// directory buckets", `CreateBucketOutput$BucketArn`).
///
/// A directory bucket's ARN carries the Region and the account — unlike a general purpose
/// bucket's ([`bucket_arn`]), whose name is globally unique — and is scoped to the
/// `s3express` namespace, which is what the member's own pattern admits beside `s3`
/// (`^arn:[^:]+:(s3|s3express):`).
pub fn directory_bucket_arn(region: &str, bucket: &str) -> String {
    ::engine::emulator_arn(
        DIRECTORY_BUCKET_ARN_SERVICE,
        region,
        &format!("bucket/{bucket}"),
    )
}

/// The ARN namespace a directory bucket is named in — the S3 Express One Zone service,
/// whose own API operations are the `s3express:` actions a directory bucket's policy grants.
const DIRECTORY_BUCKET_ARN_SERVICE: &str = "s3express";

/// The storage class the objects of a directory bucket are stored in.
///
/// "Directory buckets … only support the S3 Express One Zone storage class"
/// (`PutObjectRequest$StorageClass`), so a write into one stores its object in that class
/// rather than in the [`DEFAULT_STORAGE_CLASS`] a general purpose bucket's objects default
/// to — and it is the class an appending write requires
/// (`PutObjectRequest$WriteOffsetBytes`).
pub const EXPRESS_ONEZONE_STORAGE_CLASS: &str = "EXPRESS_ONEZONE";

/// The code S3 answers a bucket name that does not follow the published naming rules with:
/// "InvalidBucketName — The specified bucket is not valid" (Amazon S3 *Error responses*,
/// list of error codes), HTTP 400.
pub const INVALID_BUCKET_NAME_CODE: &str = "InvalidBucketName";

/// See [`INVALID_BUCKET_NAME_CODE`] — the text that list publishes for it.
pub const INVALID_BUCKET_NAME_MESSAGE: &str = "The specified bucket is not valid.";

/// The `<Message>` the modelled `InvalidRequest` shape states for itself
/// (`smithy_query(query="shape",shape="InvalidRequest")`), for the places that answer that
/// code through the unmodelled channel because the operation does not declare the shape.
pub const INVALID_REQUEST_MESSAGE: &str =
    "A parameter or header in your request isn't valid. For details, see the description of \
     this API operation.";

/// One stored VERSION of a key: the object a write recorded, or the delete marker a
/// delete that named no version laid over it.
///
/// A separate type rather than a flag on [`Object`], so a reader that wants an object's
/// bytes cannot be handed a marker by mistake: a marker holds no bytes, no metadata and no
/// entity tag, and `ListObjectVersions` reports it in its own `DeleteMarker` container.
#[derive(Debug, Clone)]
pub enum StoredVersion {
    /// A version that holds bytes — what a read answers with. BOXED: an [`Object`] is
    /// two orders of magnitude larger than a marker, and a chain of them would otherwise
    /// carry that size for every delete marker too.
    Object(Box<Object>),
    /// A delete marker: "If bucket versioning is enabled, the operation inserts a delete
    /// marker, which becomes the current version of the object" (`DeleteObject`'s
    /// documentation). While it is a key's current version every read that names no
    /// `versionId` behaves as if the object were deleted, and the versions behind it stay
    /// addressable by their own ids.
    DeleteMarker(DeleteMarker),
}

impl StoredVersion {
    /// The id this version is addressed by, or `None` for the `null` version — one stored
    /// while the bucket's versioning status was not `Enabled`, which a request names by
    /// [`NULL_VERSION_ID`].
    pub fn version_id(&self) -> Option<&str> {
        match self {
            Self::Object(object) => object.version_id.as_deref(),
            Self::DeleteMarker(marker) => marker.version_id.as_deref(),
        }
    }

    /// Whether `version_id` — as a request writes it — names this version.
    pub fn named_by(&self, version_id: &str) -> bool {
        match self.version_id() {
            Some(id) => id == version_id,
            None => version_id == NULL_VERSION_ID,
        }
    }

    /// The object this version holds, or `None` when it is a delete marker.
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object.as_ref()),
            Self::DeleteMarker(_) => None,
        }
    }

    /// The object this version holds, for writing one of its subresources, or `None` when
    /// it is a delete marker.
    fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Self::Object(object) => Some(object.as_mut()),
            Self::DeleteMarker(_) => None,
        }
    }

    /// The delete marker this version is, or `None` when it holds an object.
    pub fn as_delete_marker(&self) -> Option<&DeleteMarker> {
        match self {
            Self::Object(_) => None,
            Self::DeleteMarker(marker) => Some(marker),
        }
    }

    /// When this version came into being: the write that stored the object, or the delete
    /// that laid the marker.
    pub fn last_modified(&self) -> SystemTime {
        match self {
            Self::Object(object) => object.last_modified,
            Self::DeleteMarker(marker) => marker.last_modified,
        }
    }

    /// How many bytes this version retains, which the per-bucket and service-wide budgets
    /// count. A marker retains only the id it is addressed by.
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Object(object) => object.retained_bytes(),
            Self::DeleteMarker(marker) => marker.version_id.as_ref().map_or(0, String::len),
        }
    }
}

/// One delete marker: "a placeholder (marker) for a versioned object that was named in a
/// simple DELETE request… it has a key name and version ID like any other object"
/// (Amazon S3 User Guide, "Working with delete markers"), and nothing else — it holds no
/// bytes, so `ListObjectVersions`' `DeleteMarkerEntry` reports neither a size nor an
/// entity tag for it.
#[derive(Debug, Clone)]
pub struct DeleteMarker {
    /// The version id the marker is addressed by, or `None` for the `null` marker a
    /// versioning-SUSPENDED bucket's delete lays ("All objects added to the bucket receive
    /// the version ID null").
    ///
    /// Minted by [`ComAmazonawsS3Amazons3State::remove_object`], the one place that sees
    /// both the bucket's versioning state and the store's version sequence, so no handler
    /// constructs one.
    pub version_id: Option<String>,
    /// When the delete that laid the marker was made, which is what a listing reports as
    /// its `LastModified`.
    pub last_modified: SystemTime,
}

// The resources this state holds and the refusals it answers with, each in its own file.
// Every name stays part of this module's API: a handler keeps writing
// `crate::state::Bucket` wherever the record itself lives.

// The ACCOUNT-level Block Public Access configuration in force on this store's buckets,
// which the service that owns the API over it writes here.
pub mod account_public_access_block;
// One general purpose bucket and everything it carries.
mod bucket;
// The continuation token every listing of a bucket's id-addressed configurations pages
// with, and the reading of a token a caller hands back.
mod configuration_tokens;
// The grammar the tag set a write delivers in the `x-amz-tagging` header has to be in.
mod inline_tagging;
// What a bucket's stored lifecycle configuration is, and which ones S3 admits.
mod lifecycle;
// One stored object version.
mod object;
// The server-side encryption headers a READ of an object may not carry.
mod read_encryption_headers;
// The wire codes, statuses and sentences a refusal of this service carries.
mod refusals;
// The renames this store has performed, remembered by the client token that asked for them.
mod renames;
// One session `CreateSession` vended, and what it is scoped to.
mod sessions;
// The bucket map itself, and the operations the store performs on it.
mod store;
// The customer-provided key an SSE-C write supplies, and what a read of such an object must
// present.
pub mod sse_customer;
// Who named the tag keys one request carries, which is what decides whether a key in the
// namespace AWS keeps for its own tags may be stored.
mod tag_key_origin;
// A multipart upload in progress, with the integrity checks and minted ids it shares.
mod uploads;
// A bucket's website configuration, and the schema its document has to validate against.
mod website;

pub use account_public_access_block::*;
pub use bucket::*;
pub use configuration_tokens::*;
pub use inline_tagging::*;
pub use lifecycle::*;
pub use object::*;
pub use read_encryption_headers::*;
pub use refusals::*;
pub use renames::*;
pub use sessions::*;
pub use sse_customer::{
    customer_key_encryption, read_refusal, requested_customer_key, CustomerKey,
    CustomerKeyRejection, SSE_CUSTOMER_ALGORITHM,
};
pub use store::*;
pub use tag_key_origin::*;
pub use uploads::*;
pub use website::*;

#[cfg(test)]
mod tests;
