// SPDX-License-Identifier: CC0-1.0

//! The wire codes, statuses and sentences this service refuses a request with, and the
//! seam that reaches the shared store.
//!
//! Several of the operations that answer these — `PutBucketPolicy`, `GetBucketPolicy`,
//! `PutBucketTagging` and `GetBucketTagging` among them — declare NO errors in the model,
//! yet S3 answers each of these on them, so they are returned through
//! [`crate::scaffolded::Scaffolded::unmodeled`] rather than as a modelled shape.

use super::*;

/// "The specified bucket does not exist."
pub const NO_SUCH_BUCKET_CODE: &str = "NoSuchBucket";
/// See [`NO_SUCH_BUCKET_CODE`].
pub const NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist";
/// [`NO_SUCH_BUCKET_MESSAGE`] as the DIRECTORY bucket surface words it: the same condition,
/// terminated with a full stop.
///
/// The Zonal and `s3express-control` endpoints answer their own error documents, and the
/// sentence they carry for this code ends in a full stop where the general purpose endpoint's
/// does not — as recorded against `CreateSession`, whose only modelled error it is, for a
/// directory-bucket name nothing holds. Read through [`no_such_bucket_message`], so the two
/// surfaces cannot be worded differently in two places.
pub const DIRECTORY_BUCKET_NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist.";

/// The sentence a bucket named `bucket` that this emulator does not hold is refused with: the
/// one the SURFACE that name belongs to words.
///
/// WHICH surface a name belongs to is read off the name itself, which is also what every SDK
/// resolves the endpoint from: a name shaped `<base>--<zone-id>--x-s3` names a directory
/// bucket ([`directory_bucket_zone`]) and no other name can be served by a Zonal or
/// `s3express-control` endpoint at all. The ONE place that choice is made, so every operation
/// that answers this code about a name nothing holds words it the same way.
pub fn no_such_bucket_message(bucket: &str) -> &'static str {
    match directory_bucket_zone(bucket).is_some() {
        true => DIRECTORY_BUCKET_NO_SUCH_BUCKET_MESSAGE,
        false => NO_SUCH_BUCKET_MESSAGE,
    }
}
/// "The specified key does not exist." — what a read of an object no bucket holds
/// answers with.
pub const NO_SUCH_KEY_CODE: &str = "NoSuchKey";
/// See [`NO_SUCH_KEY_CODE`].
pub const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";
/// The HTTP status [`NO_SUCH_KEY_CODE`] is rendered with: 404, from Amazon S3's "Error
/// responses" list of error codes ("404 Not Found").
pub const NO_SUCH_KEY_STATUS: u16 = 404;
/// The code S3 answers a read or a write that names a `versionId` the bucket holds no
/// version for: "Indicates that the version ID specified in the request does not match an
/// existing version", with HTTP 404. Not a shape in this model, so it is answered through
/// the unmodelled channel.
pub const NO_SUCH_VERSION_CODE: &str = "NoSuchVersion";
/// See [`NO_SUCH_VERSION_CODE`].
pub const NO_SUCH_VERSION_MESSAGE: &str =
    "The specified version does not exist.";

/// How many characters a version id this store MINTS carries ([`mint_version_id`]): the
/// width of the opaque token S3 issues for a version, which is also the SHORTEST a version
/// id may be ([`version_id_is_well_formed`]).
pub const VERSION_ID_CHARS: usize = 32;

/// The most characters a version id may carry: `ObjectVersionId` is a plain string in the
/// model (`smithy_query(query="shape",shape="ObjectVersionId")` reports no length), and S3
/// documents a version id of "no more than 1,024 bytes" (Amazon S3 User Guide, *Using
/// versioning in S3 buckets*), so a longer token cannot be one.
pub const MAX_VERSION_ID_CHARS: usize = 1024;

/// The characters a version id is drawn from, beside the ASCII alphanumerics.
///
/// The token is opaque and travels in a `versionId` query parameter, so S3 draws it from
/// the alphanumerics and the unreserved characters that survive that round trip.
pub(super) const VERSION_ID_EXTRA_CHARS: [char; 3] = ['-', '_', '.'];

/// The character a version id's alphabet writes the value ZERO as: the first digit of the
/// URL-safe Base64 alphabet S3's own published ids are written in.
///
/// A token made of nothing but it carries the zero value — see
/// [`version_id_is_well_formed`].
pub(super) const VERSION_ID_ZERO_DIGIT: char = 'A';

/// Whether `version_id` is SHAPED like a version id S3 issues.
///
/// A `versionId` is one of exactly two things: the literal [`NULL_VERSION_ID`], which names
/// the version a write made while the bucket had no versioning state, or a token of the
/// published shape — between [`VERSION_ID_CHARS`] and [`MAX_VERSION_ID_CHARS`] characters,
/// each an ASCII alphanumeric or one of [`VERSION_ID_EXTRA_CHARS`]. The width is a RANGE
/// rather than one number because the token is opaque: this store mints ids of its own
/// width, and an id another S3 issued is just as much a version id.
///
/// An opaque token's text IS the identity it names, and one identity is not nameable: the
/// ZERO value, which a token writes as nothing but [`VERSION_ID_ZERO_DIGIT`]. No issuer
/// mints it — an issued id carries the moment and the sequence the version was recorded at
/// — so a caller that sends it has invented a token rather than named a version, which is
/// what S3 reports by refusing the ARGUMENT instead of the version.
///
/// Anything outside that is not a version id of any object in any bucket either, so both are
/// a malformed ARGUMENT rather than an absent version: S3 answers the former
/// [`INVALID_ARGUMENT_CODE`] with HTTP 400 and reserves [`NO_SUCH_VERSION_CODE`] with HTTP
/// 404 for a well-formed id that names no stored version — which is why an id of the
/// published shape that this store never minted is a 404 and not a 400. Checked by every
/// operation that takes one `versionId` as an argument, so one id is well-formed or
/// malformed everywhere; the batch delete reports per identifier what it could not do with
/// that pair instead (`ops/delete_objects.rs`).
pub fn version_id_is_well_formed(version_id: &str) -> bool {
    if version_id == NULL_VERSION_ID {
        return true;
    }
    let characters = version_id.chars().count();
    (VERSION_ID_CHARS..=MAX_VERSION_ID_CHARS).contains(&characters)
        && version_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || VERSION_ID_EXTRA_CHARS.contains(&character)
        })
        && version_id
            .chars()
            .any(|character| character != VERSION_ID_ZERO_DIGIT)
}

/// The request field a malformed `versionId` is reported against — the `versionId` query
/// parameter every operation that takes one binds it from
/// (`smithy_query(query="operation",operation="GetObject")` reports
/// `VersionId: {httpQuery: "versionId"}`), which is how S3's `InvalidArgument` document names
/// the argument it rejected.
pub const VERSION_ID_ARGUMENT: &str = "versionId";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a `versionId` that is not
/// shaped like a version id ([`version_id_is_well_formed`]).
pub const INVALID_VERSION_ID_MESSAGE: &str = "Invalid version id specified";

/// The request field a refused `x-amz-copy-source` is reported against — the header every
/// operation that takes a copy source binds it from
/// (`smithy_query(query="operation",operation="CopyObject,UploadPartCopy")` reports
/// `CopySource: {httpHeader: "x-amz-copy-source"}`), which is how S3's `InvalidArgument`
/// document names the argument it rejected.
pub const COPY_SOURCE_ARGUMENT: &str = "x-amz-copy-source";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an `x-amz-copy-source` value
/// that is not a `<bucket>/<key>` reference at all ([`CopySourceFault::Uri`]).
///
/// Fixed text: it states that the REFERENCE is not one, and names neither the value nor the
/// rule it broke — the field and the value travel in the `<ArgumentName>` /
/// `<ArgumentValue>` pair of the same document instead
/// ([`crate::scaffolded::Scaffolded::invalid_copy_source`]).
pub const INVALID_COPY_SOURCE_URI_MESSAGE: &str = "Invalid copy source URI.";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an `x-amz-copy-source` whose
/// reference names a bucket and no object key after it
/// ([`CopySourceFault::ObjectKey`]) — the other half of the same reading, and fixed text for
/// the same reason.
pub const INVALID_COPY_SOURCE_KEY_MESSAGE: &str = "Invalid copy source object key";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a copy whose source names a
/// version id that is not one: the request cannot be carried out as given, and the sentence
/// states nothing but that.
///
/// Fixed text, with nothing of the request in it. It is not the malformed-argument refusal a
/// `versionId` PARAMETER earns ([`INVALID_VERSION_ID_MESSAGE`]): the version here qualifies a
/// copy SOURCE reference rather than addressing the resource the request is about, so what
/// S3 reports is that the request itself is not one it can serve.
pub const COPY_SOURCE_INVALID_VERSION_MESSAGE: &str = "Invalid Request";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a copy whose source names a
/// DELETE MARKER by version id.
///
/// Fixed text, naming the condition itself: a delete marker holds no bytes to copy, so the
/// request cannot be served — and the sentence says which part of it is at fault, where the
/// generic "method not allowed" S3 answers a READ of a marker with would tell the caller
/// nothing about the version id it supplied.
pub const COPY_SOURCE_IS_DELETE_MARKER_MESSAGE: &str =
    "The source of a copy request may not specifically refer to a delete marker by version id.";

/// The request field a refused page bound is reported against — the query parameter
/// `ListBuckets` binds it from (`smithy_query(query="operation",operation="ListBuckets")`
/// reports `MaxBuckets: {httpQuery: "max-buckets"}`).
pub const MAX_BUCKETS_ARGUMENT: &str = "max-buckets";

/// The request field a refused continuation token is reported against — the query parameter
/// `ListObjectsV2` binds it from
/// (`smithy_query(query="operation",operation="ListObjectsV2")` reports
/// `ContinuationToken: {httpQuery: "continuation-token"}`), which is how S3's
/// `InvalidArgument` document names the argument it rejected.
pub const CONTINUATION_TOKEN_ARGUMENT: &str = "continuation-token";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a `continuation-token` that is
/// not one a listing of this service issued.
///
/// Fixed text, and the document echoes no value beside it: the token is "obfuscated and is
/// not a real key" (`ListObjectsV2Output$NextContinuationToken`), so what the caller has to
/// fix is named by the `<ArgumentName>` alone.
pub const INVALID_CONTINUATION_TOKEN_MESSAGE: &str = "The continuation token provided is incorrect";

/// The bounds a page of buckets may be asked for within: `MaxBuckets`' own published range
/// (`range {min 1, max 10000}`, the same source).
pub const MAX_BUCKETS_BOUNDS: ::std::ops::RangeInclusive<i32> = 1..=10_000;

/// Why a `max-buckets` value cannot bound a page of buckets.
///
/// The two faults S3 states different sentences for: a value that is no integer at all, and
/// an integer outside the published bounds. See [`MaxBucketsFault::message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxBucketsFault {
    /// The value is not an integer (`1.5`, `many`), so it names no number of buckets.
    NotAnInteger,
    /// The value is an integer outside [`MAX_BUCKETS_BOUNDS`].
    OutsideBounds,
}

impl MaxBucketsFault {
    /// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for this fault.
    ///
    /// Each is fixed text: the one for a value that is no integer says so, and the one for an
    /// integer outside the range states the BOUNDS rather than the value — the value is the
    /// envelope's `<ArgumentValue>`, beside the `<ArgumentName>` naming
    /// [`MAX_BUCKETS_ARGUMENT`].
    pub fn message(&self) -> String {
        match self {
            Self::NotAnInteger => {
                format!("Provided {MAX_BUCKETS_ARGUMENT} not an integer or within integer range")
            }
            Self::OutsideBounds => format!(
                "Argument {MAX_BUCKETS_ARGUMENT} must be an integer between {} and {}",
                MAX_BUCKETS_BOUNDS.start(),
                MAX_BUCKETS_BOUNDS.end()
            ),
        }
    }
}

/// How `value` fails to bound a page of buckets, or `None` for a value that bounds one.
///
/// A parameter stated with NO value states no bound at all — `?max-buckets=` names no number
/// — so it is no fault either: such a request is served the page an absent parameter asks
/// for. (The boundary withholds it from the codec for the same reason, since the codec reads
/// the empty text as an integer it cannot parse.)
///
/// One reading, so the boundary that WITHHOLDS such a value from the codec and the operation
/// that refuses it cannot disagree about which values are servable.
pub fn max_buckets_fault(value: &str) -> Option<MaxBucketsFault> {
    if value.is_empty() {
        return None;
    }
    match value.parse::<i32>() {
        Err(_) => Some(MaxBucketsFault::NotAnInteger),
        Ok(bound) if !MAX_BUCKETS_BOUNDS.contains(&bound) => Some(MaxBucketsFault::OutsideBounds),
        Ok(_) => None,
    }
}

/// Whether the codec must not be handed the `max-buckets` value `value`: one that bounds no
/// page ([`max_buckets_fault`]), and one stated with no value at all, which the codec would
/// read as an integer it cannot parse where S3 serves the request.
pub fn withholds_max_buckets(value: &str) -> bool {
    value.is_empty() || max_buckets_fault(value).is_some()
}

/// The request field a refused Object Ownership setting is reported against — the header
/// `CreateBucket` binds it from (`smithy_query(query="operation",operation="CreateBucket")`
/// reports `ObjectOwnership: {httpHeader: "x-amz-object-ownership"}`).
pub const OBJECT_OWNERSHIP_ARGUMENT: &str = "x-amz-object-ownership";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an Object Ownership setting
/// outside the published set: the header's own name and then the value as the request sent
/// it, which is the one variable part of it.
pub fn invalid_object_ownership_message(value: &str) -> String {
    format!("Invalid {OBJECT_OWNERSHIP_ARGUMENT} header: {value}")
}

/// The request field a refused canned ACL is reported against — the header every operation
/// that takes one binds it from (`smithy_query(query="operation",
/// operation="CopyObject,PutObject,CreateBucket,CreateMultipartUpload,PutBucketAcl,PutObjectAcl")`
/// reports `ACL: {httpHeader: "x-amz-acl"}`).
pub const CANNED_ACL_ARGUMENT: &str = "x-amz-acl";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a canned ACL outside the
/// published set: NONE at all. The envelope's `<ArgumentName>` / `<ArgumentValue>` pair is
/// the whole of what it says, naming [`CANNED_ACL_ARGUMENT`] and echoing the value as the
/// request sent it.
pub const INVALID_CANNED_ACL_MESSAGE: &str = "";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a request that names an ACL in
/// BOTH header forms at once: a canned ACL in `x-amz-acl` and explicit grants in the
/// `x-amz-grant-*` headers.
///
/// Fixed text naming the two forms, with nothing of the request in it: "you can specify a
/// canned ACL … or specify access permissions explicitly" (`PutObjectRequest$ACL`) is an
/// either/or, and a request that does both describes two different ACLs for one resource.
pub const BOTH_ACL_FORMS_MESSAGE: &str =
    "Specifying both Canned ACLs and Header Grants is not allowed";

/// The code S3 answers a write whose user-defined metadata is larger than one request may
/// carry ([`MAX_OBJECT_METADATA_BYTES`]), with HTTP 400 (Amazon S3 "Error responses", list
/// of error codes). Not a shape in this model, so it goes out through the unmodelled
/// channel.
pub const METADATA_TOO_LARGE_CODE: &str = "MetadataTooLarge";

/// See [`METADATA_TOO_LARGE_CODE`] — the fixed text S3 publishes for that code. The two
/// numbers a caller trims its metadata by are the `<Size>` / `<MaxSizeAllowed>` pair of the
/// same document, not part of this sentence
/// ([`crate::scaffolded::Scaffolded::metadata_too_large`]).
pub const METADATA_TOO_LARGE_MESSAGE: &str =
    "Your metadata headers exceed the maximum allowed metadata size";

/// The HTTP status [`METADATA_TOO_LARGE_CODE`] is rendered with: 400, from the same list.
pub const METADATA_TOO_LARGE_STATUS: u16 = 400;

/// The code S3 answers a `DeleteBucket` of a bucket that still holds something, with HTTP
/// 409 Conflict: "All objects (including all object versions and delete markers) in the
/// bucket must be deleted before the bucket itself can be deleted" (`DeleteBucket`'s own
/// documentation). Not a shape in this model, so it goes out through the unmodelled
/// channel.
pub const BUCKET_NOT_EMPTY_CODE: &str = "BucketNotEmpty";
/// See [`BUCKET_NOT_EMPTY_CODE`] — the wording for a bucket that has no versioning state,
/// whose only contents are the current object at each key.
pub const BUCKET_NOT_EMPTY_MESSAGE: &str = "The bucket you tried to delete is not empty";
/// See [`BUCKET_NOT_EMPTY_CODE`] — the wording for a bucket that HAS a versioning state,
/// where what is left to delete is the stored versions rather than a current object. The
/// code and the 409 are the requirement; this sentence is the recorded example of how S3
/// says "versions" to a versioned bucket's caller.
pub const BUCKET_NOT_EMPTY_VERSIONED_MESSAGE: &str =
    "The bucket you tried to delete is not empty. You must delete all versions in the bucket.";
/// The code S3 answers a write whose declared digest is not the digest of the bytes it
/// carried — a `Content-MD5` or an `x-amz-checksum-*` value that is well formed but does
/// not describe the payload ([`IntegrityRejection`] decides which sentence it carries).
pub const BAD_DIGEST_CODE: &str = "BadDigest";
/// The HTTP status [`BAD_DIGEST_CODE`] is rendered with: 400, from Amazon S3's list of
/// error codes ("400 Bad Request").
pub const BAD_DIGEST_STATUS: u16 = 400;
/// The code S3 answers a write whose body is larger than one request may carry
/// ([`MAX_OBJECT_BYTES`]).
pub const ENTITY_TOO_LARGE_CODE: &str = "EntityTooLarge";
/// The code S3 answers a conditional write whose precondition does not hold, with HTTP
/// 412 (`PutObjectRequest$IfMatch`: "If the ETag values do not match, the operation
/// returns a 412 Precondition Failed error").
pub const PRECONDITION_FAILED_CODE: &str = "PreconditionFailed";
/// See [`PRECONDITION_FAILED_CODE`] — the text Amazon S3 "Error responses" publishes for
/// that code in its list of error codes.
pub const PRECONDITION_FAILED_MESSAGE: &str =
    "At least one of the pre-conditions you specified did not hold";
/// The HTTP status [`PRECONDITION_FAILED_CODE`] is rendered with: 412, from the same
/// list ("412 Precondition Failed").
pub const PRECONDITION_FAILED_STATUS: u16 = 412;

/// The `<Condition>` S3's `PreconditionFailed` document names the conditional header that
/// did not hold by, for each conditional member this service judges.
///
/// The message that code carries is FIXED ([`PRECONDITION_FAILED_MESSAGE`]) and states that
/// "at least one" condition failed, so the member is what says WHICH one — and its value is
/// the request HEADER the failing member is bound to, as the model binds it
/// (`smithy_query(query="operation",operation="GetObject,PutObject,DeleteObject,CopyObject")`
/// reports `IfMatch: {httpHeader: "If-Match"}`, `IfNoneMatch: {httpHeader: "If-None-Match"}`,
/// `IfUnmodifiedSince: {httpHeader: "If-Unmodified-Since"}`,
/// `IfMatchLastModifiedTime: {httpHeader: "x-amz-if-match-last-modified-time"}`,
/// `IfMatchSize: {httpHeader: "x-amz-if-match-size"}` and the four `CopySourceIf*` members),
/// which is the name the client wrote on the request it has to change.
///
/// The copy-source conditions carry S3's `x-amz-copy-source-` prefix in front of the
/// standard HTTP condition's own canonical spelling, which is how S3's document words them;
/// a header S3 defines itself ([`IF_MATCH_LAST_MODIFIED_TIME_CONDITION`] and
/// [`IF_MATCH_SIZE_CONDITION`], and the annotation operations'
/// [`OBJECT_IF_MATCH_CONDITION`]) has no canonical HTTP spelling and is named as the model
/// publishes it.
pub const IF_MATCH_CONDITION: &str = "If-Match";
/// See [`IF_MATCH_CONDITION`].
pub const IF_NONE_MATCH_CONDITION: &str = "If-None-Match";
/// See [`IF_MATCH_CONDITION`].
pub const IF_UNMODIFIED_SINCE_CONDITION: &str = "If-Unmodified-Since";
/// See [`IF_MATCH_CONDITION`].
pub const IF_MODIFIED_SINCE_CONDITION: &str = "If-Modified-Since";
/// See [`IF_MATCH_CONDITION`].
pub const IF_MATCH_LAST_MODIFIED_TIME_CONDITION: &str = "x-amz-if-match-last-modified-time";
/// See [`IF_MATCH_CONDITION`].
pub const IF_MATCH_SIZE_CONDITION: &str = "x-amz-if-match-size";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_MATCH_CONDITION: &str = "x-amz-copy-source-If-Match";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_NONE_MATCH_CONDITION: &str = "x-amz-copy-source-If-None-Match";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_MODIFIED_SINCE_CONDITION: &str = "x-amz-copy-source-If-Modified-Since";
/// See [`IF_MATCH_CONDITION`].
pub const COPY_SOURCE_IF_UNMODIFIED_SINCE_CONDITION: &str =
    "x-amz-copy-source-If-Unmodified-Since";
/// See [`IF_MATCH_CONDITION`] — the header the annotation operations bind their own
/// conditional member from (`PutObjectAnnotationRequest$ObjectIfMatch`).
pub const OBJECT_IF_MATCH_CONDITION: &str = "x-amz-object-if-match";
/// See [`IF_MATCH_CONDITION`] — the four headers `RenameObject` binds its SOURCE conditions
/// from (`smithy_query(query="operation",operation="RenameObject")` reports `SourceIfMatch:
/// {httpHeader: "x-amz-rename-source-if-match"}` and the three beside it), which is the name
/// the client wrote on the request it has to change.
pub const RENAME_SOURCE_IF_MATCH_CONDITION: &str = "x-amz-rename-source-if-match";
/// See [`RENAME_SOURCE_IF_MATCH_CONDITION`].
pub const RENAME_SOURCE_IF_NONE_MATCH_CONDITION: &str = "x-amz-rename-source-if-none-match";
/// See [`RENAME_SOURCE_IF_MATCH_CONDITION`].
pub const RENAME_SOURCE_IF_MODIFIED_SINCE_CONDITION: &str =
    "x-amz-rename-source-if-modified-since";
/// See [`RENAME_SOURCE_IF_MATCH_CONDITION`].
pub const RENAME_SOURCE_IF_UNMODIFIED_SINCE_CONDITION: &str =
    "x-amz-rename-source-if-unmodified-since";
/// The code S3 answers a read of a bucket that has no ownership controls associated with
/// it, with HTTP 404 — the case `GetBucketOwnershipControls`'s own documentation describes
/// ("A bucket doesn't have OwnershipControls settings in the following cases: … You've
/// manually deleted the bucket ownership control value using the
/// DeleteBucketOwnershipControls API operation").
pub const OWNERSHIP_CONTROLS_NOT_FOUND_CODE: &str = "OwnershipControlsNotFoundError";
/// See [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`].
pub const OWNERSHIP_CONTROLS_NOT_FOUND_MESSAGE: &str =
    "The bucket ownership controls were not found";
/// The code S3 answers a read of a bucket that has no `PublicAccessBlockConfiguration`
/// associated with it, with HTTP 404 — the subresource-not-there case, answered the way
/// [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`] answers it for ownership controls, because
/// `GetPublicAccessBlock` reports the configuration as its WHOLE @httpPayload and so has
/// no way to report "there is none" in a 200.
pub const NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE: &str =
    "NoSuchPublicAccessBlockConfiguration";
/// See [`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`].
pub const NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_MESSAGE: &str =
    "The public access block configuration was not found";
/// The code S3 answers a read of a bucket that has no website configuration associated
/// with it, with HTTP 404: "NoSuchWebsiteConfiguration - The specified bucket does not have
/// a website configuration" (Amazon S3 "Error responses", list of error codes). The
/// subresource-not-there case, answered the way [`OWNERSHIP_CONTROLS_NOT_FOUND_CODE`]
/// answers it for ownership controls, because `GetBucketWebsite` reports the configuration
/// as its WHOLE output document and so has no way to report "there is none" in a 200.
pub const NO_SUCH_WEBSITE_CONFIGURATION_CODE: &str = "NoSuchWebsiteConfiguration";
/// See [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`] — the text that same list publishes for it.
pub const NO_SUCH_WEBSITE_CONFIGURATION_MESSAGE: &str =
    "The specified bucket does not have a website configuration";

/// The code S3 answers a request whose XML payload does not validate against the schema
/// the API publishes for it: "MalformedXML - The XML provided does not match the schema"
/// (`PutBucketTagging`'s own list of special errors), with HTTP 400.
pub const MALFORMED_XML_CODE: &str = "MalformedXML";

/// The HTTP status [`MALFORMED_XML_CODE`] is rendered with: 400, from that same list.
pub const MALFORMED_XML_STATUS: u16 = 400;

/// The `<Message>` S3 sends with [`MALFORMED_XML_CODE`]: "The XML you provided was not well
/// formed or did not validate against our published schema" — the FIXED text that code's
/// entry in Amazon S3's "Error responses" list publishes, which is what a schema refusal
/// carries however the document failed to validate.
pub const MALFORMED_XML_MESSAGE: &str =
    "The XML you provided was not well-formed or did not validate against our published \
     schema";

/// The first character of `document` that the XML 1.0 character set does not allow, or
/// `None` when every one of them is a `Char`.
///
/// XML 1.0 defines the characters a document may contain as
/// `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]` (W3C XML 1.0,
/// §2.2 "Characters"), so a C0 control other than tab, line feed and carriage return — and
/// the two non-characters `#xFFFE`/`#xFFFF` — cannot appear in one at all. A request
/// document carrying one is not well-formed XML whatever it otherwise says, which is
/// [`MALFORMED_XML_CODE`]. Surrogates are unrepresentable in a Rust `str`, so there is
/// nothing to check for them.
pub fn first_forbidden_xml_char(document: &str) -> Option<char> {
    document.chars().find(|character| {
        matches!(character, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}')
            || matches!(character, '\u{fffe}' | '\u{ffff}')
    })
}

/// The `Content-Type` an object is stored with when the write that created it named none.
///
/// S3 never infers a type from the key: an object put with no `Content-Type` is stored and
/// reported as `binary/octet-stream` (the default `Content-Type` the Amazon S3 User Guide
/// documents for an upload that specifies none), so a later `GetObject` or `HeadObject`
/// reports exactly that rather than a type the caller never sent.
pub const DEFAULT_CONTENT_TYPE: &str = "binary/octet-stream";

/// The code S3 answers a `CreateBucket` whose location constraint is not a location it can
/// create a bucket in at all: "InvalidLocationConstraint - The specified
/// location-constraint is not valid", HTTP 400 (Amazon S3 "Error responses", list of error
/// codes).
///
/// Distinct from [`ILLEGAL_LOCATION_CONSTRAINT_CODE`], which is about a constraint naming a
/// REAL Region the endpoint the request reached cannot place a bucket in: this one is about
/// a value that names no Region the constraint publishes at all
/// (`CreateBucketConfiguration$LocationConstraint`'s own value set — which is where
/// `us-east-1` is absent, that Region being the one an ABSENT constraint means).
pub const INVALID_LOCATION_CONSTRAINT_CODE: &str = "InvalidLocationConstraint";

/// See [`INVALID_LOCATION_CONSTRAINT_CODE`] — the text that list publishes for it.
///
/// Fixed text: the value that was rejected is named by the envelope's `<LocationConstraint>`
/// member rather than interpolated into the sentence.
pub const INVALID_LOCATION_CONSTRAINT_MESSAGE: &str =
    "The specified location-constraint is not valid";

/// The code S3 answers a `CreateBucket` whose location constraint cannot be served by the
/// endpoint the request was sent to: "IllegalLocationConstraintException - The specified
/// location-constraint is not valid", HTTP 400 (Amazon S3 "Error responses", list of error
/// codes).
pub const ILLEGAL_LOCATION_CONSTRAINT_CODE: &str = "IllegalLocationConstraintException";

/// What stands in the message below for a create that names NO location constraint, which
/// is the one thing that sentence varies in.
pub const UNSPECIFIED_LOCATION_CONSTRAINT: &str = "unspecified";

/// See [`ILLEGAL_LOCATION_CONSTRAINT_CODE`] — the wording S3 uses for a create whose
/// location constraint the endpoint it was sent to cannot serve.
///
/// Fixed text with ONE variable part: the constraint the request named, spelled as the
/// request spelled it, and [`UNSPECIFIED_LOCATION_CONSTRAINT`] for a create that named
/// none — the two readings of one sentence, so neither can drift from the other.
pub fn illegal_location_constraint_message(location_constraint: Option<&str>) -> String {
    let named = location_constraint.unwrap_or(UNSPECIFIED_LOCATION_CONSTRAINT);
    format!(
        "The {named} location constraint is incompatible for the region specific endpoint \
         this request was sent to."
    )
}

/// The host of S3's GLOBAL endpoint, which is not an endpoint for any one Region: "if you
/// send your `CreateBucket` request to the s3.amazonaws.com global endpoint, the request
/// goes to the us-east-1 Region" (`CreateBucket`'s own documentation).
pub(super) const GLOBAL_ENDPOINT_HOST: &str = "s3.amazonaws.com";

/// Whether `parts` was addressed to the GLOBAL endpoint rather than to an endpoint for one
/// Region.
///
/// Read off the host the request carries, because that is the only thing that tells the two
/// apart: a request to the global endpoint is signed for [`DEFAULT_BUCKET_REGION`] ("the
/// signature calculations in Signature Version 4 must use us-east-1 as the Region, even if
/// the location constraint in the request specifies another Region", `CreateBucket`'s own
/// documentation), exactly as a request to that Region's own endpoint is. The
/// virtual-hosted-style form of the same endpoint carries the bucket as a further label in
/// front of it, so a host that ENDS in the global one is addressed to it too.
pub fn addressed_global_endpoint(parts: &::http::request::Parts) -> bool {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())
        .unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host == GLOBAL_ENDPOINT_HOST
        || host
            .strip_suffix(GLOBAL_ENDPOINT_HOST)
            .is_some_and(|label| label.ends_with('.'))
}

/// The Region the request was signed for: the third field of its SigV4 credential scope,
/// which is the endpoint a client addressed this emulator as.
///
/// The parsing lives in `emulator_server::sigv4_credential_scope`, the one place this
/// whole emulator reads a credential scope, so no service re-splits the header. A request
/// carrying no parseable scope is read as the Region the unspecified location constraint
/// means ([`DEFAULT_BUCKET_REGION`]), which is the endpoint a client that named none
/// reached.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_BUCKET_REGION.to_string())
}

/// The ABAC status a general purpose bucket carries until a `PutBucketAbac` sets one.
///
/// `BucketAbacStatus` names the two states the property can be in — `Enabled` and
/// `Disabled` (`smithy_query(query="shape",shape="BucketAbacStatus")`) — and a bucket
/// comes into being in neither of them by request: `CreateBucket` carries no ABAC member
/// (`smithy_query(query="operation",operation="CreateBucket")`), and ABAC takes effect
/// only once it is enabled ("If ABAC is enabled on your bucket, you can use tags on the
/// bucket for access control", `GetBucketAbac`'s own documentation). So the state a
/// never-configured bucket is in is the other one.
pub const DEFAULT_BUCKET_ABAC_STATUS: &str = "Disabled";

/// How many ownership control rules this emulator records for one bucket.
///
/// NOT a fidelity claim: `OwnershipControlsRules` carries no `@length` in the model
/// (`smithy_query(query="shape",shape="OwnershipControlsRules")`), so nothing bounds the
/// list a request may send, and a bucket's rules are kept in this process's memory — the
/// same reason [`MAX_BUCKET_POLICY_BYTES`] bounds a policy. Each rule names one value of
/// the three-value `ObjectOwnership` enum, so this ceiling is far above any list that says
/// something new; a put past it is refused rather than recorded.
pub const MAX_OWNERSHIP_CONTROLS_RULES: usize = 10;

/// The `SSEAlgorithm` a general purpose bucket's default encryption uses until a
/// `PutBucketEncryption` names another one.
///
/// "By default, all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`'s own
/// documentation); SSE-S3 is the `ServerSideEncryption` value `AES256`
/// (`smithy_query(query="shape",shape="ServerSideEncryption")`).
pub const DEFAULT_SSE_ALGORITHM: &str = "AES256";

/// Whether a bucket's default encryption uses an S3 Bucket Key until a configuration
/// enables one: "General purpose buckets - By default, S3 Bucket Key is not enabled"
/// (`ServerSideEncryptionRule$BucketKeyEnabled`).
pub const DEFAULT_BUCKET_KEY_ENABLED: bool = false;

/// The `EncryptionType` values a general purpose bucket blocks until a
/// `PutBucketEncryption` names another set.
///
/// `BlockedEncryptionTypes` is "a bucket-level setting for Amazon S3 general purpose
/// buckets used to prevent the upload of new objects encrypted with the specified
/// server-side encryption type", and "currently, this parameter only supports blocking or
/// unblocking server-side encryption with customer-provided keys (SSE-C)"
/// (`ServerSideEncryptionRule$BlockedEncryptionTypes`). A general purpose bucket blocks
/// SSE-C uploads by default, which is why this is what a never-configured bucket reports.
pub const DEFAULT_BLOCKED_ENCRYPTION_TYPES: [&str; 1] = [SSE_C_ENCRYPTION_TYPE];

/// The `EncryptionType` value that names server-side encryption with customer-provided
/// keys — one of the two values the enum publishes
/// (`smithy_query(query="shape",shape="EncryptionType")` reports `[NONE, SSE-C]`), which
/// `tests/all/bucket_encryption.rs` re-derives from that value list.
pub const SSE_C_ENCRYPTION_TYPE: &str = "SSE-C";

/// The `ServerSideEncryption` values a `KMSMasterKeyID` may accompany in a default
/// encryption rule.
///
/// `ApplyServerSideEncryptionByDefault$KMSMasterKeyID`'s own documentation: "General
/// purpose buckets - This parameter is allowed if and only if `SSEAlgorithm` is set to
/// `aws:kms` or `aws:kms:dsse`." Both are values of the `ServerSideEncryption` enum, so a
/// rule naming a key beside any other algorithm is one S3 refuses rather than records.
pub const KMS_SSE_ALGORITHMS: [&str; 2] = ["aws:kms", "aws:kms:dsse"];

/// The code a read of an object version's RETENTION answers with when no retention is
/// associated with that version, with HTTP 404.
///
/// The subresource-not-there case, answered the way [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`]
/// answers it for a bucket's website configuration: `GetObjectRetention` reports the
/// `Retention` container as its WHOLE @httpPayload
/// (`smithy_query(query="operation",operation="GetObjectRetention")`), so a 200 has no way
/// to report "there is none". The code is named for the subresource the request addressed,
/// as every row of that family is.
pub const NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE: &str = "NoSuchObjectLockConfiguration";
/// See [`NO_SUCH_OBJECT_LOCK_CONFIGURATION_CODE`] — the sentence S3 sends beside it,
/// verbatim, article and all: "The specified object does not have a ObjectLock
/// configuration". Fixed text, naming neither the bucket, the key nor the version.
pub const NO_SUCH_OBJECT_LOCK_CONFIGURATION_MESSAGE: &str =
    "The specified object does not have a ObjectLock configuration";

/// The `<Message>` S3 sends beside the 403 `AccessDenied` of a call an Object Lock
/// retention bars: "Access Denied because object protected by object lock."
///
/// A fixed sentence for that one condition — it names neither the object, the mode nor the
/// date, so nothing in it is derived from the request — which is why it lives here beside
/// the other code/message pairs rather than being built in the handler.
pub const OBJECT_LOCK_ACCESS_DENIED_MESSAGE: &str =
    "Access Denied because object protected by object lock.";

/// The `<Message>` S3 sends beside the 400 [`INVALID_REQUEST_CODE`] of an object-level
/// Object Lock call — a legal hold or a retention, read or written — addressed to a bucket
/// that has no Object Lock configuration: "Bucket is missing Object Lock Configuration".
///
/// A fixed sentence for that one condition, naming neither the bucket nor the key, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in each of the handlers that answer it.
pub const OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE: &str =
    "Bucket is missing Object Lock Configuration";

/// The `<Message>` S3 sends beside the 400 [`INVALID_REQUEST_CODE`] of an object WRITE that
/// asks for an Object Lock — a mode, a retain-until date or a legal hold among its headers —
/// into a bucket that has no Object Lock configuration: "Bucket is missing
/// ObjectLockConfiguration", which names the configuration by the SHAPE the bucket would
/// carry it in (`ObjectLockConfiguration`) rather than in prose.
///
/// The write path and the object-level lock SUBRESOURCE calls word the same cause
/// differently, and each is answered in its own words: the subresource calls'
/// ([`OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE`]) spells the configuration out, and a write
/// that carries the lock among its own headers names the shape. Fixed text either way — it
/// names neither the bucket nor the key — which is why both live here beside the other
/// code/message pairs rather than being built in each handler that answers one.
pub const OBJECT_LOCK_CONFIGURATION_MISSING_ON_WRITE_MESSAGE: &str =
    "Bucket is missing ObjectLockConfiguration";

/// The code a read of a BUCKET's Object Lock configuration answers with when the bucket
/// carries none, with HTTP 404.
///
/// The subresource-not-there case for this subresource, named the way the rest of that
/// family is ([`NO_SUCH_PUBLIC_ACCESS_BLOCK_CONFIGURATION_CODE`],
/// `ReplicationConfigurationNotFoundError`): `GetObjectLockConfiguration` reports the
/// `ObjectLockConfiguration` container as its WHOLE @httpPayload
/// (`smithy_query(query="operation",operation="GetObjectLockConfiguration")`), so a 200 has
/// no way to report "there is none".
pub const OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE: &str = "ObjectLockConfigurationNotFoundError";
/// See [`OBJECT_LOCK_CONFIGURATION_NOT_FOUND_CODE`] — the fixed sentence S3 sends beside it,
/// which names neither the bucket nor anything else of the request.
pub const OBJECT_LOCK_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "Object Lock configuration does not exist for this bucket";

/// The code S3 answers a bucket-level call the bucket's own state cannot serve:
/// `InvalidBucketState`, HTTP 409 — the status S3's "Error responses" list carries for it
/// ("the request is not valid with the current state of the bucket").
///
/// The model declares no shape for it, so it travels through the unmodelled channel with
/// the message of the condition that was refused.
pub const INVALID_BUCKET_STATE_CODE: &str = "InvalidBucketState";
/// The HTTP status [`INVALID_BUCKET_STATE_CODE`] is rendered with: 409.
pub const INVALID_BUCKET_STATE_STATUS: u16 = 409;

/// The `<Message>` S3 sends beside the 409 [`INVALID_BUCKET_STATE_CODE`] of a
/// `PutObjectLockConfiguration` addressed to a bucket whose versioning status is not
/// `Enabled`: "Object Lock works only in versioned buckets" (Amazon S3 User Guide, *Using S3
/// Object Lock*), so the configuration cannot be applied to such a bucket at all.
///
/// A fixed sentence for that one condition, naming nothing of the request — which is why it
/// lives here beside the other code/message pairs rather than being built in the handler.
pub const OBJECT_LOCK_VERSIONING_REQUIRED_MESSAGE: &str =
    "Versioning must be 'Enabled' on the bucket to apply a Object Lock configuration";

/// The code S3 answers a call that cannot be served because of the state the object is in:
/// `InvalidObjectState`, HTTP 403 — the status the shape itself carries
/// (`smithy_query(query="shape",shape="InvalidObjectState")`).
///
/// The shape is in this model, but only `GetObject` lists it, and the `<Message>` depends
/// on WHICH state barred the call: a read of an archived object is told the object is
/// archived ([`crate::scaffolded::DOCUMENTED_ERROR_MESSAGES`] carries that one), while a
/// restore of an object that is not archived is told the storage class does not allow one
/// ([`RESTORE_NOT_ALLOWED_MESSAGE`]). So a handler that must word its own message answers
/// this code through the unmodelled channel, where the message travels with it.
pub const INVALID_OBJECT_STATE_CODE: &str = "InvalidObjectState";

/// The HTTP status [`INVALID_OBJECT_STATE_CODE`] is rendered with: 403, from the shape's
/// own `@httpError`.
pub const INVALID_OBJECT_STATE_STATUS: u16 = 403;

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when a restore is asked
/// for an object whose storage class has nothing to restore: "Restore is not allowed for
/// the object's current storage class."
///
/// A fixed sentence for that one condition — it names neither the object nor its class, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in the handler.
pub const RESTORE_NOT_ALLOWED_MESSAGE: &str =
    "Restore is not allowed for the object's current storage class";

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when a read is barred by
/// the storage class the object's bytes are in: "The operation is not valid for the object's
/// storage class."
///
/// A fixed sentence for that one condition — it names neither the object nor its class, so
/// nothing in it is derived from the request — which is why it lives here beside the other
/// code/message pairs rather than being built in the handler.
pub const INVALID_OBJECT_STATE_READ_MESSAGE: &str =
    "The operation is not valid for the object's storage class";

/// The `<Message>` S3 sends beside [`INVALID_OBJECT_STATE_CODE`] when the call is barred by
/// the storage class the SOURCE object's bytes are in: "Operation is not valid for the
/// source object's storage class."
///
/// The same condition [`INVALID_OBJECT_STATE_READ_MESSAGE`] words for the object being
/// read, worded for the object being copied FROM — a fixed sentence naming neither the
/// object nor its class, which is why it lives here beside the other code/message pairs.
pub const INVALID_OBJECT_STATE_COPY_SOURCE_MESSAGE: &str =
    "Operation is not valid for the source object's storage class";

/// The code `RestoreObject` answers when a retrieval is already running on the version it
/// names: `RestoreAlreadyInProgress`, with HTTP 409 Conflict — the special error that
/// operation publishes for the cause "Object restore is already in progress". Not a shape
/// in this model, so it goes out through the unmodelled channel.
pub const RESTORE_ALREADY_IN_PROGRESS_CODE: &str = "RestoreAlreadyInProgress";

/// The HTTP status [`RESTORE_ALREADY_IN_PROGRESS_CODE`] is rendered with: 409, from the
/// same list of special errors.
pub const RESTORE_ALREADY_IN_PROGRESS_STATUS: u16 = 409;

/// See [`RESTORE_ALREADY_IN_PROGRESS_CODE`] — the cause that list publishes for it.
pub const RESTORE_ALREADY_IN_PROGRESS_MESSAGE: &str = "Object restore is already in progress";

/// The code S3 answers a request whose value is not valid for the parameter carrying it:
/// "InvalidArgument - Invalid Argument", HTTP 400 (Amazon S3 "Error responses", list of
/// error codes).
pub const INVALID_ARGUMENT_CODE: &str = "InvalidArgument";
/// The HTTP status [`INVALID_ARGUMENT_CODE`] is rendered with: 400, from the same list.
pub const INVALID_ARGUMENT_STATUS: u16 = 400;

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a part number outside the
/// published range a part of a multipart upload is addressed by
/// ([`MIN_PART_NUMBER`]..=[`MAX_PART_NUMBER`]).
///
/// FIXED text stating that range, with nothing of the request in it: the number the request
/// carried is what the envelope's `<PartNumber>` member says
/// ([`crate::scaffolded::ErrorDetail`]), not part of the sentence. Read through here by every
/// operation that addresses a part by number, so one rejected part number cannot be worded
/// two ways.
pub fn invalid_part_number_message() -> String {
    format!("Part number must be an integer between {MIN_PART_NUMBER} and {MAX_PART_NUMBER}, inclusive")
}

/// The code S3 answers an annotation NAME that breaks the naming rules with — a name that
/// carries no character, one carrying a character no annotation name may carry, and one
/// under a prefix S3 keeps for itself ([`annotation_name_rejection`]) — with HTTP 400.
///
/// Recorded behaviour: this is the code on the wire for that whole class. The model's own
/// shape for it is spelled `InvalidAnnotationName`
/// (`smithy_query(query="operation",operation="PutObjectAnnotation")`), which is the shape
/// id and not the code the service sends, so the refusal goes out through the unmodelled
/// channel — the same way [`NO_SUCH_VERSION_CODE`] does for a code this model carries no
/// shape for at all.
pub const INVALID_ANNOTATION_CODE: &str = "InvalidAnnotation";

/// The HTTP status [`INVALID_ANNOTATION_CODE`] is rendered with: 400, the status the model
/// binds to the error shape of that class.
pub const INVALID_ANNOTATION_STATUS: u16 = 400;

/// See [`INVALID_ANNOTATION_CODE`] — the sentence the model states for this condition
/// (`smithy_query(query="shape",shape="InvalidAnnotationName")`: "The annotation name you
/// provided is invalid"), which is what a name that breaks no rule this service words a
/// sentence of its own for is refused with.
pub const INVALID_ANNOTATION_MESSAGE: &str = "The annotation name you provided is invalid.";

/// See [`INVALID_ANNOTATION_CODE`] — the sentence that code carries for a name spelled with
/// a character no annotation name is spelled with ([`annotation_names_carry`]), which is the
/// break of the rules it reports for every name whose characters are not the namespace's.
///
/// Recorded behaviour: fixed text naming the rule, with nothing of the request in it — the
/// offending character is not spliced into it.
pub const INVALID_ANNOTATION_CHARACTERS_MESSAGE: &str =
    "Your annotation name contains invalid characters.";

/// See [`INVALID_ANNOTATION_CODE`] — the sentence that code carries for a name under one of
/// the prefixes S3 keeps for itself: "Annotation names are UTF-8 encoded and cannot start
/// with `aws` or `s3` (case-insensitive)" (`DeleteObjectAnnotationRequest$AnnotationName`),
/// which is the rule this sentence states. Recorded behaviour: fixed text, naming both
/// reserved prefixes however the refused name was spelled after one.
pub const RESERVED_ANNOTATION_PREFIX_MESSAGE: &str =
    "Your annotation name cannot start with 'aws' or 's3'.";

/// The code S3 answers an annotation name longer than [`MAX_ANNOTATION_NAME_BYTES`] bytes
/// with, with HTTP 400 — answered identically by every operation that takes an annotation
/// name, since the name is judged before the annotation it addresses is resolved.
pub const ANNOTATION_NAME_TOO_LONG_CODE: &str = "AnnotationNameTooLong";

/// The HTTP status [`ANNOTATION_NAME_TOO_LONG_CODE`] is rendered with: 400, the status the
/// model binds to that error shape.
pub const ANNOTATION_NAME_TOO_LONG_STATUS: u16 = 400;

/// The zone the rendering above names, and renders the instant in: the one S3's own error
/// documents are written in, which its `<ArgumentValue>` dates show to be US Pacific.
///
/// A zone rather than a fixed offset, so the abbreviation is the one the INSTANT falls in —
/// `PDT` through the daylight-saving span of its year and `PST` outside it — and a date
/// rendered here is the same text S3 renders for that instant. It is the SERVICE's zone and
/// nothing else: every timestamp this emulator stores, reports and compares is UTC, and
/// this is only how one error document writes a date down.
const SERVICE_TIME_ZONE: ::chrono_tz::Tz = ::chrono_tz::America::Los_Angeles;

/// A TIMESTAMP the service rejected, as the `<ArgumentValue>` of an
/// [`INVALID_ARGUMENT_CODE`] envelope renders it.
///
/// A refused argument's value travels in the error document
/// ([`crate::scaffolded::Scaffolded::invalid_argument`]), and a timestamp is rendered there
/// the way the SERVICE renders a date rather than the way the wire carries one or the way
/// the caller happened to spell it: `EEE MMM dd HH:mm:ss zzz yyyy` — the weekday and month
/// abbreviated, the day of the month and the time of day zero-padded, the zone named, and
/// the year last. Nothing of the request's own text survives: two requests naming the same
/// instant in different spellings are refused with the same value, and the sub-second part
/// of a parsed instant — which that rendering does not carry — is not in it.
///
/// One place, so every refused timestamp of this service reads the same wherever it
/// arrived: the `Retention` document of a retention write, or the
/// [`OBJECT_LOCK_RETAIN_UNTIL_DATE_HEADER`] of an object write.
pub fn refused_timestamp_text(date: &::aws_smithy_types::DateTime) -> String {
    match ::chrono::DateTime::from_timestamp(date.secs(), 0) {
        Some(instant) => instant
            .with_timezone(&SERVICE_TIME_ZONE)
            .format("%a %b %d %H:%M:%S %Z %Y")
            .to_string(),
        // An instant outside the range a calendar date can be written in has no such
        // rendering; the format the wire carried it in is then the whole of what is known
        // about it, which is better than refusing the request with no value at all.
        None => date
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default(),
    }
}

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for a
/// `NotificationConfiguration` naming a destination it could not validate.
///
/// A FIXED sentence: nothing about the request is interpolated into it. WHICH destinations
/// failed, and why each did, travel in the numbered `<ArgumentName{n}>` /
/// `<ArgumentValue{n}>` series beside it
/// ([`crate::scaffolded::Scaffolded::invalid_arguments`]).
pub const UNABLE_TO_VALIDATE_DESTINATIONS_MESSAGE: &str =
    "Unable to validate the following destination configurations";

/// The `<Message>` S3 sends with [`INVALID_ARGUMENT_CODE`] for an `Events` entry that is
/// not one of the published `Event` values.
///
/// Also FIXED text — the event that is not published is not interpolated into it — and the
/// sentence both paths that can refuse such a value answer with: the boundary refusal of
/// the modelled enum (`crate::service`) and
/// [`NotificationConfigurationRejection::UnsupportedEvent`], which guards the store.
pub const UNSUPPORTED_EVENT_MESSAGE: &str = "The event is not supported for notifications";

/// The code S3 answers a request whose content is not one it can carry out as given:
/// "InvalidRequest", HTTP 400 (Amazon S3 "Error responses", list of error codes).
///
/// What a cors configuration breaking one of the limits its API publishes is refused with
/// — see [`CorsConfigurationRejection::code`] — where [`INVALID_ARGUMENT_CODE`] is the
/// code for a value that is wrong for the PARAMETER carrying it.
pub const INVALID_REQUEST_CODE: &str = "InvalidRequest";

/// The code S3 answers a request whose HEADERS ask for functionality the service does not
/// implement for the resource addressed: "NotImplemented — A header you provided implies
/// functionality that is not implemented", HTTP 501 (Amazon S3 "Error responses", list of
/// error codes).
///
/// What a member S3 documents for DIRECTORY buckets alone is answered with when the request
/// addresses a general purpose bucket: the service implements that functionality for one
/// kind of bucket and not for the other, so the header, not the value in it, is what cannot
/// be served. The model declares no shape for the code, so it travels through the unmodelled
/// channel.
pub const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";
/// The HTTP status [`NOT_IMPLEMENTED_CODE`] is rendered with: 501, from the same list.
pub const NOT_IMPLEMENTED_STATUS: u16 = 501;
/// See [`NOT_IMPLEMENTED_CODE`] — the text that same list publishes for it, which names the
/// header class rather than the member, so nothing of the request is interpolated into it.
pub const NOT_IMPLEMENTED_HEADER_MESSAGE: &str =
    "A header you provided implies functionality that is not implemented";

/// The code S3 answers a request that asks a bucket for a PARAMETER its bucket type does not
/// serve at all: `NotSupported`, HTTP 501.
///
/// Distinct from [`NOT_IMPLEMENTED_CODE`], which says the service has not implemented what
/// the request implies: this one says the request asked for something this KIND of bucket
/// does not have — a directory bucket's listing has no "start after this key", as recorded
/// against `ListObjectsV2` on one. The model declares no shape for the code, so it travels
/// through the unmodelled channel.
pub const NOT_SUPPORTED_CODE: &str = "NotSupported";
/// The HTTP status [`NOT_SUPPORTED_CODE`] is rendered with: 501, as recorded.
pub const NOT_SUPPORTED_STATUS: u16 = 501;

/// "The CORS configuration does not exist": the code `GetBucketCors` answers for a bucket
/// no cors configuration has been set on, with HTTP 404 (Amazon S3 "Error responses").
///
/// The subresource-not-there case, answered the way [`NO_SUCH_WEBSITE_CONFIGURATION_CODE`]
/// answers it for a website configuration, because `GetBucketCors` reports the rules as its
/// WHOLE output document and so has no way to report "there are none" in a 200.
pub const NO_SUCH_CORS_CONFIGURATION_CODE: &str = "NoSuchCORSConfiguration";
/// See [`NO_SUCH_CORS_CONFIGURATION_CODE`] — the text that same list publishes for it.
pub const NO_SUCH_CORS_CONFIGURATION_MESSAGE: &str = "The CORS configuration does not exist";

/// The code `GetBucketLifecycleConfiguration` answers for a bucket no lifecycle
/// configuration has been set on: "Error code: NoSuchLifecycleConfiguration Description:
/// The lifecycle configuration does not exist. HTTP Status Code: 404 Not Found" — the
/// special error that operation's own documentation publishes for it.
///
/// The subresource-not-there case, answered the way [`NO_SUCH_CORS_CONFIGURATION_CODE`]
/// answers it for a cors configuration, because the read reports the rules as its WHOLE
/// output document and so has no way to report "there are none" in a 200.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_CODE: &str = "NoSuchLifecycleConfiguration";
/// See [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] — the text that same documentation
/// publishes for it.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_MESSAGE: &str = "The lifecycle configuration does not exist";
/// The HTTP status [`NO_SUCH_LIFECYCLE_CONFIGURATION_CODE`] is rendered with: 404, from the
/// same documentation.
pub const NO_SUCH_LIFECYCLE_CONFIGURATION_STATUS: u16 = 404;

/// The code S3 answers a request it could not carry out at all: "InternalError - We
/// encountered an internal error. Please try again" (same list) — what a handler answers
/// when the shared store is unreachable, since every other code would be a false statement
/// about the bucket.
pub const INTERNAL_ERROR_CODE: &str = "InternalError";

/// See [`INTERNAL_ERROR_CODE`] — the fixed text S3 sends with it, from that same list,
/// nothing of which comes from the request.
pub const INTERNAL_ERROR_MESSAGE: &str = "We encountered an internal error. Please try again.";

/// The HTTP status [`INTERNAL_ERROR_CODE`] is rendered with: 500, from the same list.
pub const INTERNAL_ERROR_STATUS: u16 = 500;

/// How many default-encryption rules this emulator records for one bucket.
///
/// NOT a fidelity claim, for the same reason as [`MAX_OWNERSHIP_CONTROLS_RULES`]:
/// `ServerSideEncryptionRules` carries no `@length` in the model
/// (`smithy_query(query="shape",shape="ServerSideEncryptionRules")`), so nothing bounds
/// the list a request may send, and a bucket's rules live in this process's memory. A put
/// past this ceiling is refused rather than recorded.
pub const MAX_SERVER_SIDE_ENCRYPTION_RULES: usize = 10;

/// How many replication rules one configuration may carry.
///
/// AWS publishes the bound on the operation itself: "A replication configuration must
/// include at least one rule, and can contain a maximum of 1,000"
/// (`PutBucketReplication`'s own documentation). The lower end is the same sentence: a
/// configuration naming no rule is not one S3 records.
pub const MAX_REPLICATION_RULES: usize = 1_000;

/// How many destinations ONE write replicates to, and how many bytes of replicas it copies.
///
/// This emulator's own ceilings, not AWS quotas: a stored configuration may name up to
/// [`MAX_REPLICATION_RULES`] destinations and an object may be as large as
/// [`MAX_OBJECT_BYTES_PER_BUCKET`], and the work ONE write would do is the PRODUCT of the
/// two — both of them a caller's to choose. So the fan-out one call performs is bounded
/// here, in the place that performs it ([`ComAmazonawsS3Amazons3State::replicate_write`]),
/// rather than by each factor separately: at most this many copies, carrying at most
/// [`MAX_REPLICATION_BYTES_PER_WRITE`] bytes between them. Past either, the copies are not
/// made and the source version reports [`REPLICATION_STATUS_FAILED`], which is what
/// "replication did not happen" says — nothing claims a copy that was not written.
pub const MAX_REPLICATION_DESTINATIONS_PER_WRITE: usize = 10;

/// See [`MAX_REPLICATION_DESTINATIONS_PER_WRITE`] — the bytes half of the same bound, so a
/// write of a large object fans out to fewer destinations rather than copying the product of
/// the two ceilings.
pub const MAX_REPLICATION_BYTES_PER_WRITE: usize = 4 * MAX_OBJECT_BYTES_PER_BUCKET;

/// How many S3 Intelligent-Tiering configurations one bucket may carry.
///
/// AWS publishes the bound on the operation that records them: "You can have up to 1,000
/// S3 Intelligent-Tiering configurations per bucket"
/// (`PutBucketIntelligentTieringConfiguration`'s own documentation), which the same
/// documentation's `TooManyConfigurations` error names again as "the 1,000-configuration
/// limit". A put that would add a configuration past it is refused rather than recorded,
/// so what one bucket's map holds is bounded whatever a caller does.
pub const MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// What [`Bucket::put_intelligent_tiering_configuration`] did, so the handler answers each
/// outcome without re-deriving it — and cannot reach the stored case without having gone
/// through the configuration-count check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntelligentTieringWrite {
    /// The configuration is now the one the bucket carries under that id.
    Stored,
    /// The bucket already carries [`MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET`]
    /// configurations and this put named a new id; nothing was written.
    ConfigurationCountReached,
}

/// The code S3 answers a put that would take a bucket past the configuration limit with:
/// "TooManyConfigurations … You are attempting to create a new configuration but have
/// already reached the 1,000-configuration limit"
/// (`PutBucketIntelligentTieringConfiguration`'s and `PutBucketMetricsConfiguration`'s own
/// special errors, which publish the same code and text), under "HTTP 400 Bad
/// Request Error".
pub const TOO_MANY_CONFIGURATIONS_CODE: &str = "TooManyConfigurations";

/// See [`TOO_MANY_CONFIGURATIONS_CODE`] — the description AWS documents for that code,
/// nothing of which comes from the request. The sentence is the one that code's own
/// "Special errors" entry publishes, word for word, for every family that shares the code.
pub const TOO_MANY_CONFIGURATIONS_MESSAGE: &str =
    "You are attempting to create a new configuration but have already reached the \
     1,000-configuration limit.";

/// The HTTP status [`TOO_MANY_CONFIGURATIONS_CODE`] is rendered with: 400, from that same
/// special-errors list.
pub const TOO_MANY_CONFIGURATIONS_STATUS: u16 = 400;

/// The code S3 answers a `Tiering` whose `Days` is not a number of days that tier accepts
/// with: `InvalidAccessTier`, under "HTTP 400 Bad Request".
///
/// The range a tiering may name is a property of the ACCESS TIER it names — "the minimum
/// number of days specified for Archive Access tier must be at least 90 days and Deep
/// Archive Access tier must be at least 180 days. The maximum can be up to 2 years (730
/// days)" (`Tiering$Days`) — so a pair outside it is reported against the tier rather than
/// as a bare bad argument.
pub const INVALID_ACCESS_TIER_CODE: &str = "InvalidAccessTier";

/// The HTTP status [`INVALID_ACCESS_TIER_CODE`] is rendered with: 400, the status S3 gives
/// a configuration it cannot apply.
pub const INVALID_ACCESS_TIER_STATUS: u16 = 400;

/// The code S3 answers a put whose configuration id is not one it can record with:
/// `InvalidConfigId`, under "HTTP 400 Bad Request".
///
/// The id is what identifies the configuration on the bucket, so a refusal about the id
/// itself — one longer than [`MAX_CONFIGURATION_ID_CHARS`] — is reported against the id
/// rather than as a bare bad argument.
pub const INVALID_CONFIG_ID_CODE: &str = "InvalidConfigId";

/// See [`INVALID_CONFIG_ID_CODE`] — the fixed text S3 sends for that condition, which
/// states the bound rather than repeating the id the request named, so nothing of the
/// request reaches it.
///
/// Its wording is NOT literal at either end, and the enforced bound is the inclusive range
/// `1..=`[`MAX_CONFIGURATION_ID_CHARS`]: an id of exactly that many characters is recorded,
/// and the shortest id S3 records is one character. Do not narrow the check to match the
/// prose.
pub const INVALID_CONFIG_ID_MESSAGE: &str =
    "Config Id should not be more than 1 char and less than 64 chars in length";

/// The HTTP status [`INVALID_CONFIG_ID_CODE`] is rendered with: 400, the status S3 gives a
/// configuration it cannot apply.
pub const INVALID_CONFIG_ID_STATUS: u16 = 400;

/// The `<Message>` S3 sends beside [`INVALID_CONFIG_ID_CODE`] for an id spelled with a
/// character a configuration id may not carry: the sentence QUOTES the pattern the id has to
/// match ([`CONFIGURATION_ID_PATTERN`]), and nothing in it comes from the request.
///
/// A second sentence for the same code, beside [`INVALID_CONFIG_ID_MESSAGE`]: the code names
/// the id as the field at fault and the sentence says WHICH rule about it the value broke —
/// the character class here, the length bound there.
pub const INVALID_CONFIG_ID_PATTERN_MESSAGE: &str =
    r"Config Id does not match the pattern ^[a-zA-Z0-9\-_.]+$";

/// The pattern a bucket configuration id has to match, as the refusal that quotes it spells
/// it ([`INVALID_CONFIG_ID_PATTERN_MESSAGE`]): one or more of a letter, a number, a dash, an
/// underscore or a period.
///
/// It is the character class the id members themselves publish — "can only contain letters,
/// numbers, periods, dashes, and underscores" (`MetricsConfiguration$Id`) — written the way
/// the wire refusal states it, and [`configuration_id_fault`] is the one reading of it.
pub const CONFIGURATION_ID_PATTERN: &str = r"^[a-zA-Z0-9\-_.]+$";

/// The code S3 answers a call on a bucket configuration that states NO id it could identify
/// one by: `InvalidConfigurationId`, under HTTP 400.
///
/// Distinct from [`INVALID_CONFIG_ID_CODE`], which reports an id that WAS stated and broke a
/// rule about its spelling: with no id at all there is no configuration to report a rule
/// against, so S3 names the id itself as the thing that is not one.
pub const INVALID_CONFIGURATION_ID_CODE: &str = "InvalidConfigurationId";

/// See [`INVALID_CONFIGURATION_ID_CODE`] — the fixed text S3 sends for that condition,
/// nothing of which comes from the request.
pub const INVALID_CONFIGURATION_ID_MESSAGE: &str = "The specified configuration id is invalid.";

/// The HTTP status [`INVALID_CONFIGURATION_ID_CODE`] is rendered with: 400, the status S3
/// gives a request naming an argument it cannot act on.
pub const INVALID_CONFIGURATION_ID_STATUS: u16 = 400;

/// The code S3 answers a bucket-configuration document whose EXPORT DESTINATION is not a
/// bucket it could write to with: `InvalidDestination`, under HTTP 400.
///
/// Distinct from [`MALFORMED_XML_CODE`]: the document validates against the published schema
/// — the member is a string and it is there — and what S3 cannot act on is the VALUE of the
/// destination, so the refusal names the destination.
pub const INVALID_DESTINATION_CODE: &str = "InvalidDestination";

/// The HTTP status [`INVALID_DESTINATION_CODE`] is rendered with: 400.
pub const INVALID_DESTINATION_STATUS: u16 = 400;

/// The code S3 answers a bucket-configuration write whose DOCUMENT `Id` is not the id the
/// request addresses with, where the operation publishes a code naming that disagreement:
/// `IdMismatch`, under HTTP 400.
pub const ID_MISMATCH_CODE: &str = "IdMismatch";

/// See [`ID_MISMATCH_CODE`] — the fixed text S3 sends for that condition, which states the
/// rule rather than echoing either id back.
pub const ID_MISMATCH_MESSAGE: &str = "Document ID does not match the specified configuration ID.";

/// The HTTP status [`ID_MISMATCH_CODE`] is rendered with: 400.
pub const ID_MISMATCH_STATUS: u16 = 400;

/// Why a value is not the ARN of a BUCKET data may be exported to — which is what decides
/// the sentence a write naming it is refused with.
///
/// ONE type for every member that takes a bucket ARN as an export destination, because the
/// rule is one rule: a bucket ARN names neither a Region nor an account id, "because a
/// bucket name is globally unique" ([`is_s3_bucket_arn`] carries the grammar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketArnFault {
    /// The value is not a bucket ARN at all — a bare bucket name, say, or an ARN of another
    /// service.
    NotAnArn,
    /// The value is an S3 ARN whose Region field carries a value, which a bucket ARN never
    /// does.
    RegionQualified,
}

impl BucketArnFault {
    /// The `<Message>` a write naming such a destination is refused with: the fixed sentence
    /// S3 publishes for the rule the value broke, with nothing of the request in it.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAnArn => "Invalid bucket ARN.",
            Self::RegionQualified => "Bucket ARN should not specify a region.",
        }
    }
}

/// Why `value` is not the ARN of a bucket data may be exported to, or `None` when it is one.
///
/// The one reading of the destination rule, shared by every member that takes an export
/// destination bucket, so two configuration families cannot drift apart on which values they
/// record. The REGION-qualified case is told apart from the rest because S3 states a rule of
/// its own for it: a bucket ARN carries no Region.
pub fn bucket_arn_fault(value: &str) -> Option<BucketArnFault> {
    if is_s3_bucket_arn(value) {
        return None;
    }
    match ::service_common::arn::parse(value) {
        // An S3 ARN naming a resource and a Region: the Region is the one thing wrong with
        // it, whether or not it also names an account, which is the rule S3 states its own
        // sentence for.
        Some(parsed)
            if parsed.vendor == "s3"
                && !parsed.region.is_empty()
                && !parsed.resource.is_empty() =>
        {
            Some(BucketArnFault::RegionQualified)
        }
        _ => Some(BucketArnFault::NotAnArn),
    }
}

/// How many decimal digits an AWS account id carries: twelve, the form every account id in
/// this emulator takes ([`::engine::emulator_account_id`] mints one).
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an AWS account id: exactly [`ACCOUNT_ID_DIGITS`] decimal digits.
///
/// One reading for every member that takes an account id, so a configuration family cannot
/// admit a value another refuses.
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|digit| digit.is_ascii_digit())
}

/// Why a value is not an id a bucket configuration may be identified by — which is what
/// decides the refusal a call naming it answers with.
///
/// ONE type for every id-addressed bucket configuration family (analytics, metrics, S3
/// Inventory and S3 Intelligent-Tiering), because the id rule is one rule: the families
/// cannot drift apart on which values they record, nor on how a value outside the rule is
/// reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigurationIdFault {
    /// The call states no id at all, or an empty one: there is no value to judge against the
    /// id rule, so the id itself is what the refusal names
    /// ([`INVALID_CONFIGURATION_ID_CODE`]).
    Unstated,
    /// The id carries a character a configuration id may not
    /// ([`CONFIGURATION_ID_PATTERN`]).
    OutsidePattern,
    /// The id is longer than [`MAX_CONFIGURATION_ID_CHARS`] characters.
    TooLong,
}

impl ConfigurationIdFault {
    /// The S3 error code a call naming such an id is refused with.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_CODE,
            Self::OutsidePattern | Self::TooLong => INVALID_CONFIG_ID_CODE,
        }
    }

    /// The HTTP status that refusal is rendered with — 400 for every one of them: the id is
    /// an argument of the request, and a request S3 cannot act on is a bad request.
    pub fn status(self) -> u16 {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_STATUS,
            Self::OutsidePattern | Self::TooLong => INVALID_CONFIG_ID_STATUS,
        }
    }

    /// The `<Message>` that refusal carries: the fixed sentence S3 publishes for the rule the
    /// value broke, with nothing of the request in it.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unstated => INVALID_CONFIGURATION_ID_MESSAGE,
            Self::OutsidePattern => INVALID_CONFIG_ID_PATTERN_MESSAGE,
            Self::TooLong => INVALID_CONFIG_ID_MESSAGE,
        }
    }
}

/// Why `value` is not an id a bucket configuration may be identified by, or `None` when it
/// is one.
///
/// The one reading of the id rule: non-empty, spelled in [`CONFIGURATION_ID_PATTERN`]'s
/// character class, and at most [`MAX_CONFIGURATION_ID_CHARS`] characters. The model
/// constrains none of the id shapes (`smithy_query(query="shape",shape="AnalyticsId")`
/// reports a plain string), so nothing refuses a value outside it before a handler does, and
/// an id is what one bucket's configuration maps key on.
pub fn configuration_id_fault(value: &str) -> Option<ConfigurationIdFault> {
    if value.is_empty() {
        return Some(ConfigurationIdFault::Unstated);
    }
    if !value.chars().all(is_configuration_id_character) {
        return Some(ConfigurationIdFault::OutsidePattern);
    }
    if value.chars().count() > MAX_CONFIGURATION_ID_CHARS {
        return Some(ConfigurationIdFault::TooLong);
    }
    None
}

/// Whether `character` is one [`CONFIGURATION_ID_PATTERN`]'s class holds: a letter, a
/// number, a period, a dash or an underscore.
pub(super) fn is_configuration_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
}

/// The code S3 answers a listing whose `continuation-token` is not one it issued:
/// `MalformedContinuationToken`, under HTTP 400 — the token is an argument of the request
/// and a token the service cannot resume from names no page of the listing.
pub const MALFORMED_CONTINUATION_TOKEN_CODE: &str = "MalformedContinuationToken";

/// See [`MALFORMED_CONTINUATION_TOKEN_CODE`] — the fixed text S3 sends for that condition,
/// as the service words it on the wire (its grammar is S3's own), with nothing of the
/// request in it: the token is "an opaque value that Amazon S3 understands"
/// (`ListBucketAnalyticsConfigurationsRequest$ContinuationToken`), so a refusal does not
/// echo it back.
pub const MALFORMED_CONTINUATION_TOKEN_MESSAGE: &str = "The continuation-token you provided invalid.";

/// The HTTP status [`MALFORMED_CONTINUATION_TOKEN_CODE`] is rendered with: 400.
pub const MALFORMED_CONTINUATION_TOKEN_STATUS: u16 = 400;

/// The most characters a bucket configuration id may carry.
///
/// 64, the limit AWS publishes on the id members themselves: "The ID has a 64 character
/// limit" (`MetricsConfiguration$Id`), and the same 64 for an S3 Intelligent-Tiering
/// configuration id. Neither `MetricsId` nor `IntelligentTieringId` is constrained in the
/// model — `smithy_query(query="shape",shape="IntelligentTieringId")` reports a bare string
/// — so nothing refuses a longer id before a handler does, and ONE constant serves both
/// families so the two cannot drift. A put naming a longer id is refused rather than
/// recorded, so what one bucket's maps key on is bounded whatever a caller does.
///
/// The LENGTH is shared, and so is the reading of WHICH values identify a configuration
/// ([`configuration_id_fault`]); the CODE a refusal carries is each family's own, since AWS
/// answers an id outside the rule per family — [`INVALID_CONFIG_ID_CODE`] for an analytics
/// or S3 Intelligent-Tiering id, which those operations report against the id itself, and
/// [`MALFORMED_XML_CODE`] for a metrics configuration's `Id`, whose rule the metrics
/// operation reports as the document failing to validate against the published schema
/// (`PutBucketMetricsConfiguration` publishes no error of its own for a document it cannot
/// take).
pub const MAX_CONFIGURATION_ID_CHARS: usize = 64;

/// The code S3 answers a `GetBucketReplication` for a bucket that has no replication
/// configuration with: `ReplicationConfigurationNotFoundError` (Amazon S3 "Error
/// responses", list of error codes), with HTTP 404.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_CODE: &str = "ReplicationConfigurationNotFoundError";
/// The message S3 answers [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] with: "The
/// replication configuration was not found" — fixed text, nothing of which comes from the
/// request, phrased the way the service words it on the wire rather than the way the
/// error-code list describes the condition ("there is no replication configuration for this
/// bucket"), which is a description of the case and not the `<Message>` a client reads.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_MESSAGE: &str =
    "The replication configuration was not found";
/// The HTTP status [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] is rendered with: 404,
/// from the same list.
pub const REPLICATION_CONFIGURATION_NOT_FOUND_STATUS: u16 = 404;

/// The code S3 answers a `PutBucketLogging` whose `LoggingEnabled$TargetBucket` is not a
/// bucket log files can be delivered to with: "InvalidTargetBucketForLogging - The target
/// bucket for logging does not exist, is not owned by you, or does not have the appropriate
/// grants for the log-delivery group." (Amazon S3 "Error responses", list of error codes),
/// with HTTP 400.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_CODE: &str = "InvalidTargetBucketForLogging";
/// See [`INVALID_TARGET_BUCKET_FOR_LOGGING_CODE`] — the fixed text S3 publishes for that
/// code, nothing of which comes from the request.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_MESSAGE: &str =
    "The target bucket for logging does not exist, is not owned by you, or does not have \
     the appropriate grants for the log-delivery group.";
/// The HTTP status [`INVALID_TARGET_BUCKET_FOR_LOGGING_CODE`] is rendered with: 400, from
/// the same list.
pub const INVALID_TARGET_BUCKET_FOR_LOGGING_STATUS: u16 = 400;

/// The code S3 answers a `PutBucketLogging` whose target bucket is in another Region than
/// the bucket being logged with: "CrossLocationLoggingProhibited - Cross-location logging
/// not allowed. Buckets in one geographic location cannot log information to a bucket in
/// another location." (Amazon S3 "Error responses", list of error codes), with HTTP 403 —
/// the refusal behind "All logs are saved to buckets in the same Amazon Web Services Region
/// as the source bucket" (`PutBucketLogging`'s own documentation).
pub const CROSS_LOCATION_LOGGING_PROHIBITED_CODE: &str = "CrossLocationLoggingProhibited";
/// See [`CROSS_LOCATION_LOGGING_PROHIBITED_CODE`] — the fixed text S3 publishes for that
/// code.
pub const CROSS_LOCATION_LOGGING_PROHIBITED_MESSAGE: &str =
    "Cross-location logging not allowed. Buckets in one geographic location cannot log \
     information to a bucket in another location.";
/// The HTTP status [`CROSS_LOCATION_LOGGING_PROHIBITED_CODE`] is rendered with: 403, from
/// the same list.
pub const CROSS_LOCATION_LOGGING_PROHIBITED_STATUS: u16 = 403;

/// How many metrics configurations one bucket may hold, and so how many entries one
/// bucket's metrics record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 metrics
/// configurations per bucket" (`PutBucketMetricsConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_METRICS_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters a metrics configuration id may carry.
///
/// From the member's own documentation: "The ID has a 64 character limit and can only
/// contain letters, numbers, periods, dashes, and underscores" (`MetricsConfiguration$Id`).
/// The model constrains `MetricsId` no further — `smithy_query(query="shape",
/// shape="MetricsId")` reports a plain string with no `length` or `pattern` — so nothing
/// refuses a longer or oddly spelled id before [`configuration_id_fault`] does. The length
/// half is [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares;
/// the character set is this family's own.
pub const MAX_METRICS_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// Whether `value` is long enough and short enough to identify a bucket configuration:
/// between one and [`MAX_CONFIGURATION_ID_CHARS`] characters.
///
/// The LENGTH half every bucket configuration id shares, so the families cannot drift apart
/// on it: [`configuration_id_fault`] composes it with the character set the id members
/// document, and [`is_valid_inventory_id`] is it alone. An EMPTY id identifies no
/// configuration, so it is never one.
pub fn is_valid_configuration_id(value: &str) -> bool {
    let characters = value.chars().count();
    characters > 0 && characters <= MAX_CONFIGURATION_ID_CHARS
}

/// The code S3 answers a read that names a bucket subresource configuration the bucket
/// carries none of under that id: "NoSuchConfiguration - The specified configuration does
/// not exist." (Amazon S3 "Error responses", list of error codes), with HTTP 404 — the
/// subresource-not-there case for a configuration a put identifies by an id, answered the
/// way [`REPLICATION_CONFIGURATION_NOT_FOUND_CODE`] answers it for the one a bucket carries
/// at most one of.
pub const NO_SUCH_CONFIGURATION_CODE: &str = "NoSuchConfiguration";
/// See [`NO_SUCH_CONFIGURATION_CODE`] — the fixed text S3 publishes for that code, nothing
/// of which comes from the request.
pub const NO_SUCH_CONFIGURATION_MESSAGE: &str = "The specified configuration does not exist.";
/// The HTTP status [`NO_SUCH_CONFIGURATION_CODE`] is rendered with: 404, from the same
/// list.
pub const NO_SUCH_CONFIGURATION_STATUS: u16 = 404;

/// The code S3 answers a `CreateBucketMetadataConfiguration` for a bucket that already
/// carries one: the create cannot be carried out because the resource it would create is
/// already there, and the code names that condition.
///
/// AWS's text for it is fixed, with nothing from the request in it:
/// [`METADATA_CONFIGURATION_ALREADY_EXISTS_MESSAGE`].
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_CODE: &str = "MetadataConfigurationAlreadyExists";
/// See [`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`] — the text S3 sends for that code.
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_MESSAGE: &str =
    "The metadata configuration already exists";
/// The HTTP status [`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`] is rendered with: 409
/// Conflict, the status S3 renders every "the resource is already there" code with
/// (`BucketAlreadyExists` and `BucketAlreadyOwnedByYou` carry the same one).
pub const METADATA_CONFIGURATION_ALREADY_EXISTS_STATUS: u16 = 409;

/// The code S3 answers a call that addresses the S3 Metadata configuration of a general
/// purpose bucket which carries none — a bucket that never had one, or whose configuration
/// a `DeleteBucketMetadataConfiguration` has removed.
///
/// The metadata-configuration family names its own not-there condition rather than sharing
/// the id-addressed [`NO_SUCH_CONFIGURATION_CODE`] of the other bucket subresources, the
/// same way it names its own already-there condition
/// ([`METADATA_CONFIGURATION_ALREADY_EXISTS_CODE`]): a configuration a bucket carries at
/// most one of, addressed by the bucket alone.
pub const METADATA_CONFIGURATION_NOT_FOUND_CODE: &str = "MetadataConfigurationNotFound";
/// See [`METADATA_CONFIGURATION_NOT_FOUND_CODE`] — the text S3 sends for that code, fixed,
/// with nothing from the request in it.
pub const METADATA_CONFIGURATION_NOT_FOUND_MESSAGE: &str = "The metadata configuration was not found";
/// The HTTP status [`METADATA_CONFIGURATION_NOT_FOUND_CODE`] is rendered with: 404, the
/// status S3 renders every "the subresource is not there" code with (the sibling
/// [`NO_SUCH_CONFIGURATION_STATUS`] is the same one).
pub const METADATA_CONFIGURATION_NOT_FOUND_STATUS: u16 = 404;

/// What [`Bucket::put_metrics_configuration`], [`Bucket::put_analytics_configuration`] or
/// [`Bucket::put_inventory_configuration`] did, so the caller answers each outcome without
/// re-deriving it — and cannot reach the stored cases without having gone through the
/// per-bucket ceiling.
///
/// ONE type for every one of those families, because a put of any of them is the same
/// transaction: a full replacement of the configuration of the id it names, bounded by how
/// many of them one bucket may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketConfigurationWrite {
    /// The bucket carried no configuration under that id; this one was recorded.
    Stored,
    /// A configuration of that id was already there and this one replaced it IN FULL.
    Replaced,
    /// The bucket already holds as many configurations of that family as it may
    /// ([`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`],
    /// [`MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET`],
    /// [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`]) and this put would add one more;
    /// NOTHING was written.
    QuotaReached,
}

/// Record `configuration` under `id` in one of a bucket's per-id configuration maps,
/// refusing a NEW id once the map holds `ceiling` of them.
///
/// The one implementation behind [`Bucket::put_metrics_configuration`],
/// [`Bucket::put_analytics_configuration`] and [`Bucket::put_inventory_configuration`]: each
/// stores the modelled container whole under the id the request identifies it by, so
/// replacing an id already present overwrites that entry alone and counts against no
/// ceiling.
pub(super) fn put_keyed_configuration<T>(
    configurations: &mut BTreeMap<String, T>,
    id: String,
    configuration: T,
    ceiling: usize,
) -> BucketConfigurationWrite {
    // The ceiling is read BEFORE the entry is taken, because taking it borrows the map.
    let at_the_ceiling = configurations.len() >= ceiling;
    match configurations.entry(id) {
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            entry.insert(configuration);
            BucketConfigurationWrite::Replaced
        }
        std::collections::btree_map::Entry::Vacant(_) if at_the_ceiling => {
            BucketConfigurationWrite::QuotaReached
        }
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(configuration);
            BucketConfigurationWrite::Stored
        }
    }
}

/// How many analytics configurations one bucket may hold, and so how many entries one
/// bucket's analytics record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 analytics
/// configurations per bucket" (`PutBucketAnalyticsConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_ANALYTICS_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters an analytics configuration id may carry.
///
/// [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares: the model
/// constrains `AnalyticsId` no further — `smithy_query(query="shape",shape="AnalyticsId")`
/// reports a plain string with no `length` or `pattern` — so nothing refuses a longer id
/// before a handler does, and an id is what one bucket's analytics map keys on.
pub const MAX_ANALYTICS_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// How many S3 Inventory configurations one bucket may hold, and so how many entries one
/// bucket's inventory record can ever carry.
///
/// AWS publishes the bound on the operation itself: "You can have up to 1,000 inventory
/// configurations per bucket" (`PutBucketInventoryConfiguration`'s own documentation), which
/// also names the refusal past it ([`TOO_MANY_CONFIGURATIONS_CODE`]). A put past it is
/// refused rather than recorded, so a caller looping on fresh ids cannot grow one bucket's
/// record without end.
pub const MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET: usize = 1_000;

/// The most characters an S3 Inventory configuration id may carry.
///
/// [`MAX_CONFIGURATION_ID_CHARS`], the bound every bucket configuration id shares: the model
/// constrains `InventoryId` no further — `smithy_query(query="shape",shape="InventoryId")`
/// reports a plain string with no `length` or `pattern` — so nothing refuses a longer id
/// before a handler does, and an id is what one bucket's inventory map keys on.
pub const MAX_INVENTORY_ID_CHARS: usize = MAX_CONFIGURATION_ID_CHARS;

/// Whether `value` is an id an S3 Inventory configuration may be identified by: a
/// configuration id by length ([`is_valid_configuration_id`], i.e. between one and
/// [`MAX_INVENTORY_ID_CHARS`] characters), which is all `InventoryId` constrains.
pub fn is_valid_inventory_id(value: &str) -> bool {
    is_valid_configuration_id(value)
}

/// The HTTP status [`INVALID_REQUEST_CODE`] is rendered with: 400, from Amazon S3's "Error
/// responses" list of error codes. It is the status the replication prerequisite is refused
/// under: a replication configuration applies only to a bucket whose versioning is enabled.
pub const INVALID_REQUEST_STATUS: u16 = 400;

/// The `<Message>` S3 sends beside the 400 [`INVALID_REQUEST_CODE`] of a copy whose source
/// and destination name the same object and which would change nothing about it: "This copy
/// request is illegal because it is trying to copy an object to itself without changing the
/// object's metadata, storage class, website redirect location or encryption attributes."
///
/// The sentence S3 publishes for that one condition, and it states the condition itself: the
/// attributes a self-copy has to change to be served. Nothing of the request is interpolated
/// into it — it names neither the bucket nor the key — which is why it lives here beside the
/// other code/message pairs rather than being worded in the handler, and why the generic
/// sentence the `InvalidRequest` SHAPE carries ([`INVALID_REQUEST_MESSAGE`]) is not what this
/// condition answers with.
pub const SELF_COPY_WITHOUT_CHANGE_MESSAGE: &str =
    "This copy request is illegal because it is trying to copy an object to itself without \
     changing the object's metadata, storage class, website redirect location or encryption \
     attributes.";

/// "The bucket policy does not exist."
pub const NO_SUCH_BUCKET_POLICY_CODE: &str = "NoSuchBucketPolicy";
/// See [`NO_SUCH_BUCKET_POLICY_CODE`].
pub const NO_SUCH_BUCKET_POLICY_MESSAGE: &str = "The bucket policy does not exist";
/// The code `GetBucketTagging` answers for a bucket no tag set has been stored on, with
/// HTTP 404 — the subresource-not-there case, answered the way
/// [`NO_SUCH_BUCKET_POLICY_CODE`] answers it for a bucket policy, because `TagSet` is a
/// required output member and so a 200 has no way to report "there is none".
pub const NO_SUCH_TAG_SET_CODE: &str = "NoSuchTagSet";
/// See [`NO_SUCH_TAG_SET_CODE`] — the fixed text S3 publishes for that code in its "Error
/// responses" list of error codes, which is what the service sends for this condition:
/// nothing in it comes from the request.
pub const NO_SUCH_TAG_SET_MESSAGE: &str = "The TagSet does not exist";
/// The code S3 answers a `PutBucketVersioning` whose `VersioningConfiguration` is not
/// one it can apply: "Indicates that the versioning configuration specified in the
/// request is invalid", HTTP 400 (Amazon S3 "Error responses", list of error codes).
pub const ILLEGAL_VERSIONING_CONFIGURATION_CODE: &str = "IllegalVersioningConfigurationException";

/// "The policy must be a valid JSON policy document."
pub const MALFORMED_POLICY_CODE: &str = "MalformedPolicy";
/// See [`MALFORMED_POLICY_CODE`].
pub const MALFORMED_POLICY_MESSAGE: &str =
    "Policy must be a JSON policy document with a non-empty Statement list, each statement \
     carrying an Effect of Allow or Deny and an Action";

/// The most bytes a bucket policy may carry, and so the most one bucket's policy record
/// can hold.
///
/// `PutBucketPolicy$Policy` is the `@httpPayload`, a plain string with no `@length` in
/// the model, so without a ceiling one request could park an arbitrarily large document
/// in the store — once per bucket, up to [`MAX_BUCKETS_PER_ACCOUNT`]. AWS bounds it with
/// the bucket-policy limit it publishes, 20 KB, and refuses a longer document.
pub const MAX_BUCKET_POLICY_BYTES: usize = 20 * 1024;

/// The statements `policy` carries, or `None` when the document is not a JSON object with
/// a `Statement` of the shape a policy writes it in: a list of statements, or a single
/// statement object.
///
/// One place locates a document's statements, so the grammar check
/// ([`is_valid_policy_document`]) and the public-access check
/// ([`policy_grants_public_access`]) cannot drift on what they are reading.
pub(super) fn policy_statements(parsed: &::serde_json::Value) -> Option<Vec<&::serde_json::Value>> {
    match parsed.as_object()?.get("Statement")? {
        ::serde_json::Value::Array(list) => Some(list.iter().collect()),
        single @ ::serde_json::Value::Object(_) => Some(vec![single]),
        _ => None,
    }
}

/// The principals a policy document GRANTS TO: every value of every statement's `Principal`
/// or `NotPrincipal` element, in the order the document names them and without repeats — the
/// same identity named twice is one identity to resolve.
///
/// The element is written as a bare string, a list of strings, or a map keyed by principal
/// kind (`AWS`, `Service`, `Federated`, `CanonicalUser`) whose values are each one string or
/// a list of them ([`principal_names_wildcard`] reads the same three forms), and the values
/// of every kind are reported: which of them names an IAM identity at all is the resolution's
/// question, not this reading's.
///
/// Empty for a text this cannot read as a policy: what makes a document unusable is stated by
/// the grammar ([`is_valid_policy_document`]) and not a second time here.
pub fn policy_principals(policy: &str) -> Vec<String> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return Vec::new();
    };
    let Some(statements) = policy_statements(&parsed) else {
        return Vec::new();
    };
    let mut named: Vec<String> = Vec::new();
    for value in statements
        .iter()
        .filter_map(|statement| statement.as_object())
        .flat_map(|statement| {
            ["Principal", "NotPrincipal"]
                .into_iter()
                .filter_map(|element| statement.get(element))
        })
        .flat_map(principal_element_values)
    {
        if !named.contains(&value) {
            named.push(value);
        }
    }
    named
}

/// The string values one `Principal`/`NotPrincipal` element names, in each of the three forms
/// a policy writes it in — see [`policy_principals`].
fn principal_element_values(element: &::serde_json::Value) -> Vec<String> {
    match element {
        ::serde_json::Value::String(value) => vec![value.clone()],
        ::serde_json::Value::Array(list) => {
            list.iter().flat_map(principal_element_values).collect()
        }
        ::serde_json::Value::Object(kinds) => {
            kinds.values().flat_map(principal_element_values).collect()
        }
        _ => Vec::new(),
    }
}

/// Whether `policy` is a policy document S3 accepts on `PutBucketPolicy`.
///
/// The grammar an IAM resource policy has to satisfy: a JSON object carrying a
/// `Statement` that is a non-empty list of objects (or a single statement object), each
/// with an `Effect` of exactly `Allow` or `Deny` and an `Action`. A `Policy` that is not
/// even JSON fails here first, which is what `MalformedPolicy` reports.
pub fn is_valid_policy_document(policy: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(statements) = policy_statements(&parsed) else {
        return false;
    };
    !statements.is_empty()
        && statements.iter().all(|statement| {
            let Some(statement) = statement.as_object() else {
                return false;
            };
            let effect = statement.get("Effect").and_then(::serde_json::Value::as_str);
            matches!(effect, Some("Allow") | Some("Deny")) && statement.contains_key("Action")
        })
}

/// The condition keys that, when a statement's `Condition` names one, keep that statement
/// from granting access to EVERYONE: each of them limits the grant to a set of fixed
/// values (an account, an organization, a VPC, an access point, a caller, a key), so the
/// statement reaches those callers rather than the public.
///
/// The catalog is the one Amazon S3 publishes under "The Meaning of 'Public'" (Amazon S3
/// User Guide, Blocking public access): a policy that grants access to a wildcard
/// principal is public "unless it contains a `Condition` that restricts access to a set of
/// fixed values" for one of these keys. Lower-case because IAM condition keys are
/// case-insensitive, so lookups fold the policy's spelling to this one.
pub(super) const PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS: [&str; 13] = [
    "aws:principalarn",
    "aws:principalorgid",
    "aws:principalorgpaths",
    "aws:sourceaccount",
    "aws:sourcearn",
    "aws:sourceip",
    "aws:sourceowner",
    "aws:sourcevpc",
    "aws:sourcevpce",
    "aws:userid",
    "s3:dataaccesspointaccount",
    "s3:dataaccesspointarn",
    "s3:x-amz-server-side-encryption-aws-kms-key-id",
];

/// The one entry of [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`] whose values are address
/// ranges rather than identifiers, so what "every value" means for it is
/// [`WHOLE_INTERNET_CIDRS`] rather than the bare wildcard.
pub(super) const SOURCE_IP_CONDITION_KEY: &str = "aws:sourceip";

/// The `aws:SourceIp` values that name no fixed set of addresses: the whole IPv4 and IPv6
/// space.
///
/// From the same guide: an `aws:SourceIp` condition keeps a statement non-public only when
/// the addresses it names are not the entire internet, so a statement conditioned on one
/// of these still grants access to everyone.
pub(super) const WHOLE_INTERNET_CIDRS: [&str; 2] = ["0.0.0.0/0", "::/0"];

/// IAM's two wildcard characters, which a condition operand carrying either of them matches
/// a CLASS of values with rather than naming fixed ones: `*` stands for any run of characters
/// and `?` for exactly one (the pair
/// `service_common::iam_authorization::wildcard_match` compares a pattern by). A statement
/// whose only restriction is such an operand still reaches everyone the key could name, so it
/// restricts nothing for the purpose of "The Meaning of 'Public'".
pub(super) const WILDCARD_CHARACTERS: [char; 2] = ['*', '?'];

/// Whether `policy` grants access to the PUBLIC — the question
/// `GetBucketPolicyStatus`'s `PolicyStatus$IsPublic` answers about the bucket's policy.
///
/// The rule, from "The Meaning of 'Public'": a policy is public when ANY of its statements
/// both allows and reaches everyone — an `Effect` of `Allow` whose principal is the
/// wildcard (`"*"`, or `"*"` among the values of a principal map such as `{"AWS": "*"}`),
/// or an `Allow` that names a `NotPrincipal` and so grants to every principal it does not
/// list — and is not held back by a `Condition` restricting it to FIXED values of one of
/// [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`]. A statement that grants nobody anything
/// (no principal at all), one whose `Effect` is `Deny`, and a document that is not a policy
/// are all non-public, as is a bucket carrying no policy — which is why the caller passes
/// the policy it HAS, and reports a bucket without one as not public.
pub fn policy_grants_public_access(policy: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(statements) = policy_statements(&parsed) else {
        return false;
    };
    statements.iter().any(|statement| {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        let allows = statement
            .get("Effect")
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|effect| effect == "Allow");
        allows && grants_every_principal(statement) && !condition_restricts_principals(statement)
    })
}

/// Whether `statement` grants to every principal: a wildcard `Principal`, or a
/// `NotPrincipal` (which names the principals the statement does NOT reach, so an `Allow`
/// carrying one reaches all the others).
pub(super) fn grants_every_principal(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    if statement.contains_key("NotPrincipal") {
        return true;
    }
    let Some(principal) = statement.get("Principal") else {
        // A statement with no principal grants nothing to anybody.
        return false;
    };
    principal_names_wildcard(principal)
}

/// Whether a `Principal` value names the wildcard, in any of the forms a policy writes it:
/// the bare string, a list carrying it, or a principal map (`{"AWS": "*"}`,
/// `{"AWS": ["*", …]}`) one of whose values is it.
pub(super) fn principal_names_wildcard(principal: &::serde_json::Value) -> bool {
    match principal {
        ::serde_json::Value::String(value) => value == "*",
        ::serde_json::Value::Array(list) => list.iter().any(principal_names_wildcard),
        ::serde_json::Value::Object(map) => map.values().any(principal_names_wildcard),
        _ => false,
    }
}

/// Whether `statement`'s `Condition` holds its grant back from the public: it restricts
/// one of [`PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS`] to FIXED values, which is what the
/// guide requires — a key whose operand names every value it could take (the bare
/// wildcard, or the whole address space for `aws:SourceIp`) leaves the grant public, and a
/// list restricts only while every value in it is fixed.
pub(super) fn condition_restricts_principals(
    statement: &::serde_json::Map<String, ::serde_json::Value>,
) -> bool {
    let Some(condition) = statement.get("Condition").and_then(::serde_json::Value::as_object) else {
        return false;
    };
    condition.values().any(|operands| {
        operands
            .as_object()
            .into_iter()
            .flatten()
            .any(|(key, values)| {
                let key = key.to_ascii_lowercase();
                PUBLIC_ACCESS_RESTRICTING_CONDITION_KEYS.contains(&key.as_str())
                    && names_fixed_values(&key, values)
            })
    })
}

/// Whether a condition key's operand names fixed values: a single fixed value, or a
/// non-empty list of them.
pub(super) fn names_fixed_values(key: &str, values: &::serde_json::Value) -> bool {
    match values {
        ::serde_json::Value::String(value) => is_fixed_value(key, value.trim()),
        ::serde_json::Value::Array(list) => {
            !list.is_empty() && list.iter().all(|value| names_fixed_values(key, value))
        }
        _ => false,
    }
}

/// Whether one operand value of `key` is a FIXED value rather than one every value the key
/// could take falls under.
///
/// Three kinds of operand name no fixed value: the empty one; one whose wildcards leave every
/// value of the key's own value space matching ([`names_every_value`] — `vpc-*` admits every
/// VPC there is, while `o-exampleorgid/r-examplerootid/*` admits the paths under one
/// organization root and so names a set); and, for `aws:SourceIp`, the whole address space
/// ([`WHOLE_INTERNET_CIDRS`]).
pub(super) fn is_fixed_value(key: &str, value: &str) -> bool {
    if value.is_empty() || names_every_value(key, value) {
        return false;
    }
    key != SOURCE_IP_CONDITION_KEY || !WHOLE_INTERNET_CIDRS.contains(&value)
}

/// Whether `value`, read as the pattern a wildcard operator compares with, matches EVERY
/// value `key` could name — which is what makes a condition over it restrict nothing.
///
/// A value carrying no wildcard names itself and nothing else, so it always restricts. One
/// that carries a wildcard restricts only while what it fixes BEFORE the first wildcard goes
/// beyond the prefix every value of that key begins with ([`VALUE_SPACE_PREFIXES`]): `vpc-*`
/// fixes exactly `vpc-`, which every VPC id starts with, so it admits every VPC and names no
/// set, while `o-exampleorgid/r-examplerootid/*` fixes an organization and a root inside it
/// and so names the paths under that root. The bare `*` fixes nothing at all and therefore
/// matches everything, whatever the key.
fn names_every_value(key: &str, value: &str) -> bool {
    let Some(fixed) = value.split(WILDCARD_CHARACTERS).next().filter(|fixed| {
        // `split` yields the whole value when it carries no wildcard at all.
        fixed.len() < value.len()
    }) else {
        return false;
    };
    let prefix = VALUE_SPACE_PREFIXES
        .iter()
        .find(|(known, _)| *known == key)
        .map_or("", |(_, prefix)| *prefix);
    fixed.len() <= prefix.len() && prefix.starts_with(fixed)
}

/// The prefix every value of a condition key begins with, for the keys whose values are
/// identifiers of ONE published form — so a pattern that fixes no more than this names no
/// value of that key in particular ([`names_every_value`]).
///
/// Each prefix is the published form of that key's own values: a VPC id is `vpc-<id>` and a
/// VPC endpoint id `vpce-<id>` (Amazon VPC, resource id formats); an organization id is "`o-`
/// followed by 10 to 32 lowercase letters or digits" and an organization PATH begins with
/// that id (AWS Organizations, `Organization$Id` and `aws:PrincipalOrgPaths`); an ARN begins
/// with `arn:` (AWS General Reference, *Amazon Resource Names*). A key whose values carry no
/// such prefix — an account id, a unique id, an IP range — is not listed, and for those only
/// a pattern that fixes nothing at all matches every value.
const VALUE_SPACE_PREFIXES: [(&str, &str); 7] = [
    ("aws:sourcevpc", "vpc-"),
    ("aws:sourcevpce", "vpce-"),
    ("aws:principalorgid", "o-"),
    ("aws:principalorgpaths", "o-"),
    ("aws:principalarn", "arn:"),
    ("aws:sourcearn", "arn:"),
    ("s3:dataaccesspointarn", "arn:"),
];

/// What a bucket policy says about `action` on `resource` for a caller that presents NO
/// credential.
///
/// Decided by the tree's ONE evaluation of IAM policy documents
/// (`service_common::iam_authorization`), which is the same rule every other emulated
/// service that enforces a resource policy answers with — an explicit `Deny` about the
/// request wins, otherwise an `Allow` about it grants, and a document that says nothing
/// about it states nothing. The caller is passed with NO principal spellings at all, which
/// is exactly what the anonymous caller is: only a statement whose principal element reaches
/// EVERY principal is about its request, and one naming an account or a role is not.
pub fn policy_decision_for_anonymous(
    policy: &str,
    action: &str,
    resource: &str,
) -> ::service_common::iam_authorization::Decision {
    ::service_common::iam_authorization::resource_decision(
        policy,
        &::service_common::iam_authorization::Request::new(action, resource, &[]),
    )
}

/// The IAM action an object READ is authorized under: "To use `GetObject`, you must have
/// the `READ` access to the object (or version)" (`GetObject`'s own documentation), which
/// is the same permission a read of that object's metadata needs — "A `HEAD` request has
/// the same options as a `GET` operation on an object" (`HeadObject`'s own documentation).
pub const GET_OBJECT_ACTION: &str = "s3:GetObject";

/// The IAM action that LISTING a bucket's contents is authorized under, and the one that
/// decides what a read of an address holding nothing is TOLD: "if the object that you
/// request doesn't exist, the error that Amazon S3 returns depends on whether you also have
/// the `s3:ListBucket` permission. If you have the `s3:ListBucket` permission on the bucket,
/// Amazon S3 returns an HTTP status code 404 Not Found error. If you don't have the
/// `s3:ListBucket` permission, Amazon S3 returns an HTTP status code 403 Access Denied
/// error" (`GetObject`'s own documentation).
pub const LIST_BUCKET_ACTION: &str = "s3:ListBucket";

/// The condition key saying whether the request arrived over a secure transport: "whether
/// the request was sent using SSL" (AWS General Reference, *Global condition context keys*).
///
/// This service ANSWERS it rather than leaving it open, because it knows: the emulator
/// serves every endpoint over TLS, so every request it judges arrived that way. A bucket
/// whose policy carries the guardrail clause AWS publishes for every bucket — a `Deny` of
/// everything that did NOT arrive over a secure transport — therefore refuses nothing here,
/// where a key with no value would leave that clause undecided and an undecided refusal
/// still refuses.
pub(super) const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The value that key carries, as IAM's boolean literals spell it.
pub(super) const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition keys a request judged here carries a value for, and so DECLARES it models
/// — the keys whose absence from a policy's clause is decided rather than unknown. Every
/// other key keeps the fail-closed reading ([`Bucket::caller_is_denied`]).
pub(super) const REQUEST_CONDITION_KEYS: &[&str] = &[SECURE_TRANSPORT_KEY];

/// The `Payer` value under which the REQUESTER, rather than the bucket owner, pays for a
/// download — one of the two values `Payer` takes (`PAYER_VALUES`), the other being
/// [`DEFAULT_PAYER`].
pub const PAYER_REQUESTER: &str = "Requester";

/// The code S3 answers a request its authorization refuses with, and the status and message
/// that code carries (Amazon S3 "Error responses", list of error codes: "AccessDenied —
/// Access Denied", HTTP 403).
pub const ACCESS_DENIED_CODE: &str = "AccessDenied";
/// See [`ACCESS_DENIED_CODE`].
pub const ACCESS_DENIED_STATUS: u16 = 403;
/// See [`ACCESS_DENIED_CODE`] — the text that same published list carries for it.
pub const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The code S3 answers a request presenting a security token that names nothing, with the
/// status and the message that code carries (Amazon S3 "Error responses", list of error
/// codes: "InvalidToken — The provided token is malformed or otherwise invalid", HTTP 400).
///
/// What an S3 EXPRESS session token no session of this emulator carries earns: nothing can be
/// said about a session that was never vended — see `crate::service::express_session`.
pub const INVALID_TOKEN_CODE: &str = "InvalidToken";
/// See [`INVALID_TOKEN_CODE`].
pub const INVALID_TOKEN_STATUS: u16 = 400;
/// See [`INVALID_TOKEN_CODE`] — the text that same published list carries for it.
pub const INVALID_TOKEN_MESSAGE: &str = "The provided token is malformed or otherwise invalid.";

/// The code S3 answers a request presenting a security token whose lifetime has passed, with
/// the status and the message that code carries (same published list: "ExpiredToken — The
/// provided token has expired", HTTP 400).
///
/// What an S3 EXPRESS session the CLOCK has expired earns — "after the expiration time, any
/// calls that you make with those credentials will fail" (`CreateSession`'s own
/// documentation) — which is what tells a caller to take a new session rather than to fix its
/// request.
pub const EXPIRED_TOKEN_CODE: &str = "ExpiredToken";
/// See [`EXPIRED_TOKEN_CODE`].
pub const EXPIRED_TOKEN_STATUS: u16 = 400;
/// See [`EXPIRED_TOKEN_CODE`] — the text that same published list carries for it.
pub const EXPIRED_TOKEN_MESSAGE: &str = "The provided token has expired.";

/// The code S3 answers a request whose ACCESS KEY ID resolves to no principal at all, with
/// the status and the message that code carries (Amazon S3 "Error responses", list of error
/// codes: "InvalidAccessKeyId — The AWS access key ID that you provided does not exist in our
/// records", HTTP 403).
///
/// What a session credential whose principal is GONE earns — a role session whose role the
/// account no longer holds (`sts_store::Caller::principal_is_withdrawn`) — and it is answered
/// at credential resolution, before any policy is read: the two codes mean opposite things to
/// a client, since this one is terminal and tells a credential provider to stop using the
/// credential, while [`ACCESS_DENIED_CODE`] says the credential is still believed valid and
/// only the grant is missing.
pub const INVALID_ACCESS_KEY_ID_CODE: &str = "InvalidAccessKeyId";
/// See [`INVALID_ACCESS_KEY_ID_CODE`].
pub const INVALID_ACCESS_KEY_ID_STATUS: u16 = 403;
/// See [`INVALID_ACCESS_KEY_ID_CODE`] — the sentence that same published list carries for it,
/// which is fixed text in full: the rejected key id is not echoed back.
pub const INVALID_ACCESS_KEY_ID_MESSAGE: &str =
    "The AWS Access Key Id you provided does not exist in our records.";

/// The message S3 refuses a request carrying no credential against a Requester Pays bucket
/// with: "Anonymous users cannot invoke requests against Requester Pays buckets" — fixed
/// text with no variable parts, because there is no principal to name.
///
/// A Requester Pays bucket charges the REQUESTER, and "requests to a Requester Pays bucket
/// must be authenticated: you cannot make anonymous requests"
/// (`PutBucketRequestPayment`'s own documentation of the feature), since an anonymous
/// request names no account to charge. The acknowledgement header does not rescue it for
/// the same reason.
pub const REQUESTER_PAYS_ANONYMOUS_MESSAGE: &str =
    "Anonymous users cannot invoke requests against Requester Pays buckets. Please \
     authenticate.";

/// The query parameters a request's CREDENTIAL can travel in when it carries no
/// `Authorization` header: the SigV4 presigned fields ("`X-Amz-Credential`", "`X-Amz-
/// Signature`") and the legacy presigned form's access key id (AWS General Reference,
/// *Authenticating requests: using query parameters*).
///
/// `pub(crate)`: the refusal wording asks the same question of the request it is answering
/// for ([`crate::service::sigv4`]), so both read one catalog.
pub(crate) const CREDENTIAL_QUERY_PARAMETERS: [&str; 3] =
    ["X-Amz-Credential", "X-Amz-Signature", "AWSAccessKeyId"];

/// Whether `parts` presents NO credential at all — the ANONYMOUS caller, which is what
/// decides whether an operation judges the request against what the bucket grants to
/// everyone.
///
/// "If you grant READ access to the anonymous user, the `GetObject` operation returns the
/// object without using an authorization header" (`GetObject`'s own documentation): a
/// request carrying a credential speaks for the account that credential names, whatever the
/// signature then verifies as, and one carrying none speaks for nobody. A credential travels
/// in the `Authorization` header of a signed request or in the query string of a presigned
/// one ([`CREDENTIAL_QUERY_PARAMETERS`]).
pub fn request_is_anonymous(parts: &::http::request::Parts) -> bool {
    let signed = parts
        .headers
        .get(::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty());
    if signed {
        return false;
    }
    let query = parts.uri.query().unwrap_or_default();
    !CREDENTIAL_QUERY_PARAMETERS
        .iter()
        .any(|parameter| crate::service::query_carries(query, parameter))
}

/// The Region a `CreateBucket` request places its bucket in, from the
/// `CreateBucketConfiguration.LocationConstraint` it carries (or does not).
///
/// The rule is the member's own documentation: an absent constraint means
/// [`DEFAULT_BUCKET_REGION`], the legacy value `EU` means
/// [`EU_LOCATION_CONSTRAINT_REGION`], and every other value of
/// `BucketLocationConstraint` IS the Region code.
pub fn bucket_region(location_constraint: Option<&str>) -> String {
    match location_constraint {
        None => DEFAULT_BUCKET_REGION.to_string(),
        Some(EU_LOCATION_CONSTRAINT) => EU_LOCATION_CONSTRAINT_REGION.to_string(),
        Some(region) => region.to_string(),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3Amazons3State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3Amazons3State>)
}
