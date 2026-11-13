// SPDX-License-Identifier: CC0-1.0

//! A multipart upload IN PROGRESS, and the integrity checks and minted ids every write of
//! this service shares.

use super::*;

/// One in-progress multipart upload this emulator holds, as the
/// `CreateMultipartUpload` that initiated it recorded it.
///
/// A multipart upload is a RESOURCE of its own, distinct from the object it will become:
/// "This action initiates a multipart upload and returns an upload ID … You also include
/// this upload ID in the final request to either complete or abort the multipart upload
/// request" (`CreateMultipartUpload`'s own documentation). So everything the initiating
/// request settles about the FINISHED object is kept here until a later
/// `CompleteMultipartUpload` applies it — the same members [`Object`] keeps, because that
/// is what the completed object is made of — and the upload is addressed by its id in the
/// meantime.
///
/// The parts uploaded into it so far are here too ([`Self::parts`]): a part belongs to the
/// upload rather than to the object, because nothing of it is addressable as an object
/// until a completion assembles it.
#[derive(Debug, Clone)]
pub struct MultipartUpload {
    /// The object key the upload was initiated for — "Object key for which the multipart
    /// upload is to be initiated" (`CreateMultipartUploadRequest$Key`). An upload is
    /// addressed by its id AND its key, so a request naming the id under another key
    /// names no upload.
    pub key: String,
    /// When the upload was initiated: the instant
    /// [`ComAmazonawsS3Amazons3State::create_multipart_upload`] recorded it, which is what
    /// `MultipartUpload$Initiated` reports. Never reassigned, so the value a reader sees
    /// does not move while the upload is in progress.
    pub initiated_at: SystemTime,
    /// A `StorageClass` value; [`DEFAULT_STORAGE_CLASS`] when the request named none —
    /// what `ListParts` reports as the "class of storage used to store the uploaded
    /// object" (`ListPartsOutput$StorageClass`).
    pub storage_class: String,
    pub content_type: Option<String>,
    pub cache_control: Option<String>,
    pub content_disposition: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub expires: Option<String>,
    pub website_redirect_location: Option<String>,
    /// The user-defined metadata the initiating request carried in its `x-amz-meta-`
    /// headers: "A map of metadata to store with the object in S3"
    /// (`CreateMultipartUploadRequest$Metadata`) — the object the upload completes into,
    /// so it is held with the upload until then.
    pub metadata: BTreeMap<String, String>,
    /// The tag set the initiating request's `x-amz-tagging` header named, judged against
    /// an OBJECT's published tag-set limits ([`OBJECT_TAG_SET_LIMITS`]) exactly as
    /// `PutObject`'s own header is. Empty when the request carried none.
    pub tags: Vec<ResourceTag>,
    /// The server-side encryption configuration the upload was initiated under, which
    /// every reader of the upload reports back — see [`ObjectEncryption`].
    pub encryption: ObjectEncryption,
    /// The `ChecksumAlgorithm` the initiating request asked S3 to create the object's
    /// checksum with, or `None` when it named none. The CONFIGURATION, not a checksum:
    /// nothing here computes one, so no checksum VALUE is recorded or reported.
    pub checksum_algorithm: Option<String>,
    /// The `ChecksumType` the initiating request asked for, kept for the same reason as
    /// [`Self::checksum_algorithm`].
    pub checksum_type: Option<String>,
    /// The parts `UploadPart` has recorded against this upload, by part number.
    ///
    /// Keyed by part number because that is what addresses a part — "A part number uniquely
    /// identifies a part and also defines its position within the object being created"
    /// (`UploadPart`'s own documentation) — so a second upload of one number replaces what
    /// that number held rather than adding a part beside it. Sorted, because every reader
    /// of an upload reports its parts in ascending part-number order
    /// (`ListPartsRequest$PartNumberMarker`: "Only parts with higher part numbers will be
    /// listed").
    ///
    /// A part is recorded only through [`ComAmazonawsS3Amazons3State::upload_part`], the
    /// only place that sees the byte budgets a retained part counts against; an initiation
    /// records an upload with no part at all.
    pub parts: BTreeMap<i32, UploadedPart>,
    /// The Object Lock the INITIATING request asked the completed object to be stored with
    /// — see [`RequestedObjectLock`].
    ///
    /// "When you initiate a multipart upload, you specify the options you want on the
    /// finished object" (`CreateMultipartUpload`'s own documentation), and the Object Lock
    /// mode, retain-until date and legal hold are three of those options, so they are held
    /// with the upload until the completion stores them on the version it writes.
    pub object_lock: RequestedObjectLock,
    /// WHO initiated the upload: the principal the initiating request signed for, as every
    /// reader of the upload reports it — see [`UploadInitiator`].
    pub initiator: UploadInitiator,
}

impl MultipartUpload {
    /// How large this upload's user-defined metadata is, in UTF-8 bytes summed over every
    /// key and value — what [`MAX_OBJECT_METADATA_BYTES`] bounds, measured the way
    /// [`Object::metadata_bytes`] measures a stored object's.
    pub fn metadata_bytes(&self) -> usize {
        self.metadata
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum()
    }

    /// The part `part_number` names, or `None` when this upload holds no such part — which
    /// is what makes a read of an upload report only the parts something really uploaded.
    pub fn part(&self, part_number: i32) -> Option<&UploadedPart> {
        self.parts.get(&part_number)
    }

    /// How many bytes the parts of this upload retain — what the per-bucket and
    /// service-wide byte budgets count for an upload, measured the way
    /// [`Object::retained_bytes`] measures a stored object's: the parts, plus the
    /// caller-supplied strings held beside them.
    pub fn retained_bytes(&self) -> usize {
        self.parts
            .values()
            .map(UploadedPart::retained_bytes)
            .sum::<usize>()
            + self
                .object_lock
                .retention
                .as_ref()
                .map_or(0, ObjectRetention::retained_bytes)
    }
}

/// One part `UploadPart` recorded against an in-progress multipart upload.
///
/// A part is the data "you provide … as a part of an object in your request"
/// (`UploadPart`'s own documentation) plus what every reader of the upload reports about
/// it: the members `Part` declares (`smithy_query(query="shape",shape="Part")`) — its
/// number, the instant it was uploaded, its entity tag and its size — so `ListParts`
/// answers from the recorded part rather than from anything re-derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedPart {
    /// "Part number identifying the part. This is a positive integer between 1 and 10,000"
    /// (`Part$PartNumber`), inside [`MIN_PART_NUMBER`]..=[`MAX_PART_NUMBER`]. Kept on the
    /// part as well as in the map's key so a reader that holds a part holds its number.
    pub part_number: i32,
    /// "Entity tag returned when the part was uploaded" (`Part$ETag`) — the quoted MD5 of
    /// the part's own bytes, as [`object_etag`] renders it, so a later completion can be
    /// judged against the tag the upload really answered.
    pub etag: String,
    /// "Date and time at which the part was uploaded" (`Part$LastModified`): the instant
    /// the store recorded it. Reassigned only by an upload that overwrites the part, so a
    /// reader sees the moment the bytes it is reporting arrived.
    pub uploaded_at: SystemTime,
    /// The part's data, which a completion assembles into the object's body. "Size in
    /// bytes of the uploaded part data" (`Part$Size`) is its length, so nothing records a
    /// size beside it that could disagree with the bytes.
    pub body: Vec<u8>,
    /// The digest of THIS part's own bytes, under the algorithm the part write settled, or
    /// `None` when neither the part write nor the initiation named one.
    ///
    /// A part-level checksum is per-part: "the Base64 encoded, … checksum of the part"
    /// (`Part$ChecksumSHA256` and its nine siblings), never the object's composition over
    /// the parts — so its [`ObjectChecksum::checksum_type`] is
    /// [`CHECKSUM_TYPE_FULL_OBJECT`]: the value is the digest of ALL of the part's bytes.
    /// Recorded by the write that received those bytes, so `UploadPart`'s answer and every
    /// later `ListParts` report the same value.
    pub checksum: Option<ObjectChecksum>,
}

impl UploadedPart {
    /// "Size in bytes of the uploaded part data" (`Part$Size`) — the length of the bytes
    /// recorded, as the member's `long` type carries it.
    pub fn size(&self) -> i64 {
        i64::try_from(self.body.len()).unwrap_or(i64::MAX)
    }

    /// How many bytes this part retains: its data AND the strings kept beside it — the
    /// entity tag and the recorded checksum — the way [`Object::retained_bytes`] counts a
    /// stored object's, so no part of an upload escapes the budgets.
    pub fn retained_bytes(&self) -> usize {
        self.body.len()
            + self.etag.len()
            + self
                .checksum
                .as_ref()
                .map_or(0, ObjectChecksum::retained_bytes)
    }
}

/// The id an initiated multipart upload is addressed by.
///
/// S3's upload ids are OPAQUE: `MultipartUploadId` is a plain string in the model
/// (`smithy_query(query="shape",shape="MultipartUploadId")` reports no pattern and no
/// length), and the only thing documented about one is that it identifies the upload every
/// later part, completion and abort request names ("This upload ID is used to associate
/// all of the parts in the specific multipart upload"). So this is DERIVED rather than
/// drawn from a generator, exactly as [`mint_version_id`] is: the SHA-256 of the bucket,
/// the key and the store's monotonic mint sequence, URL-safe Base64 encoded so the id
/// survives the round trip through the `uploadId` query parameter a client sends it back
/// in. Two uploads — of one key or anywhere in the store — therefore never collide, and
/// the id stays what it was once recorded, because nothing re-derives it.
pub fn mint_upload_id(bucket: &str, key: &str, sequence: u64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(bucket.as_bytes());
    // A separator, so ("a", "bc") and ("ab", "c") cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(key.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    ::aws_smithy_types::base64::encode(digest.finalize().as_slice())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "")
}

/// The entity tag S3 reports for an object written by a single `PutObject`.
///
/// The rule is `PutObjectOutput$ETag`'s own documentation: "General purpose buckets - To
/// ensure that data is not corrupted traversing the network, for objects where the ETag
/// is the MD5 digest of the object, you can calculate the MD5 while putting an object to
/// Amazon S3 and compare the returned ETag to the calculated MD5 value." A one-part,
/// unencrypted PUT into a general purpose bucket — the only kind this emulator records —
/// is exactly that case, so the tag is the MD5 of the stored bytes, lowercase hex, inside
/// the double quotes S3 wraps it in.
///
/// One PART of a multipart upload carries the same tag over its own bytes — "Entity tag
/// returned when the part was uploaded" (`Part$ETag`) — so `UploadPart` renders a part's
/// tag through here too, rather than deriving one of its own.
pub fn object_etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

/// `etag` WITHOUT the double quotes S3 stores and reports it in.
///
/// An entity tag is held and reported quoted (see [`object_etag`]) — but an event
/// notification reports the digest alone: the `etag` of an EventBridge detail and the
/// `eTag` of a `Records` document both carry it without the surrounding quotes. One place,
/// so both documents report the same value and no caller has to know the difference
/// between the stored form and the published one.
pub fn unquoted_etag(etag: &str) -> &str {
    etag.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(etag)
}

/// The entity tag S3 reports for an object a multipart upload was completed into, over the
/// parts it was assembled from, in the order they were assembled.
///
/// A multipart object's tag is NOT the digest of its bytes: "if an object is uploaded as a
/// multipart upload, then the ETag of the object is not an MD5 digest of the entire object.
/// Amazon S3 calculates the MD5 digest of each individual part as it is uploaded. The MD5
/// digests are used to determine the ETag for the final object. Amazon S3 concatenates the
/// bytes for the MD5 digests together and then calculates the MD5 digest of these
/// concatenated values" (Amazon S3 User Guide, *Checking object integrity*). S3 then
/// "appends a dash with the total number of parts to the ETag", which is what makes a
/// multipart tag recognisable as one.
///
/// So the digest is over the raw digest BYTES of each part, not over their hex renderings,
/// and the count is the number of parts the completion assembled — one part included, which
/// still carries the dashed form.
pub fn multipart_object_etag(parts: &[&[u8]]) -> String {
    let mut digest = Md5::new();
    for part in parts {
        digest.update(Md5::digest(part));
    }
    format!("\"{}-{}\"", hex::encode(digest.finalize()), parts.len())
}

/// The entity tag's own VALUE: the digest [`object_etag`] rendered, without the double
/// quotes around it.
///
/// Those quotes are HTTP's quoted-string syntax for the `ETag` response header — "An
/// entity-tag consists of an opaque quoted string" (RFC 9110 §8.8.3) — so they belong to
/// the renderings that reproduce that header verbatim (the header itself, and the legacy
/// listing elements that echo it), not to the tag. A field that carries the tag as its own
/// typed value in a response document, and a precondition comparison against a tag a
/// client may send back with or without the quotes, work on this value instead.
pub fn etag_value(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// What a conditional WRITE's members say about the key it addresses, when they do not all
/// hold — see [`unmet_write_precondition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnmetWriteCondition {
    /// `If-Match` names an entity tag at a key holding no live object at all: nothing was
    /// ever written there, or the version the key currently resolves to is a delete marker.
    /// There is no tag to compare, so what the request named is not there.
    NoSuchKey,
    /// The condition carried by the request header this names did not hold against the
    /// object the key holds.
    Condition(&'static str),
}

/// The conditional WRITE member a request carries that does NOT hold at a key whose current
/// object carries `held_etag` (`None` for a key holding nothing), named by its header, or
/// `None` when every one of them holds.
///
/// The two documented conditions, each quoted from the member that carries it:
///
/// * "Uploads the object only if the ETag (entity tag) value provided during the WRITE
///   operation matches the ETag of the object in S3. If the ETag values do not match, the
///   operation returns a 412 Precondition Failed error" (`PutObjectRequest$IfMatch`) — a tag
///   is the tag OF an object, so a key holding no live object has none to compare: the
///   address the condition is about is not there, which is
///   [`UnmetWriteCondition::NoSuchKey`] ([`NO_SUCH_KEY_CODE`], 404) rather than a
///   precondition that failed. A key hidden by a delete marker holds no live object either,
///   so it is answered the same way — and a caller using `If-Match` for optimistic
///   concurrency can tell "someone else changed it" (retry with a fresh tag) from "it is
///   gone" (create it) instead of retrying forever.
/// * "Uploads the object only if the object key name does not already exist in the bucket
///   specified. Otherwise, Amazon S3 returns a 412 Precondition Failed error"
///   (`PutObjectRequest$IfNoneMatch`), whose value "expects the '*' (asterisk) character":
///   what it asks is that the key hold nothing.
///
/// The tags are compared by VALUE ([`etag_value`]): a client may send a tag back with or
/// without the double quotes HTTP's quoted-string syntax wraps it in, and both name the same
/// object.
///
/// ONE place for the rule, so every conditional write in this service — the one-shot write
/// and the completion of a multipart upload — refuses exactly the same requests and names
/// the same header in the refusal's `<Condition>`.
pub fn unmet_write_precondition(
    held_etag: Option<&str>,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
) -> Option<UnmetWriteCondition> {
    if if_none_match.is_some() && held_etag.is_some() {
        return Some(UnmetWriteCondition::Condition(IF_NONE_MATCH_CONDITION));
    }
    let if_match = if_match?;
    let Some(held) = held_etag else {
        return Some(UnmetWriteCondition::NoSuchKey);
    };
    (etag_value(held) != etag_value(if_match))
        .then_some(UnmetWriteCondition::Condition(IF_MATCH_CONDITION))
}

/// The `If-Match` value that matches whatever entity tag the addressed object carries:
/// "`If-Match` does accept a string value of an '*' (asterisk) character to denote a match
/// of any ETag" (`DeleteObjectRequest$IfMatch`).
pub const ANY_ENTITY_TAG: &str = "*";

/// The conditional DELETE member a request carries that does NOT hold against the object
/// version it addresses, named by its header, or `None` when every one of them holds —
/// `held` being that version, or `None` when the request addresses nothing the store holds.
///
/// The three conditions, each quoted from the member that carries it:
///
/// * "Deletes the object if the ETag (entity tag) value provided during the delete operation
///   matches the ETag of the object in S3. If the ETag values do not match, the operation
///   returns a `412 Precondition Failed` error" (`DeleteObjectRequest$IfMatch`), with
///   [`ANY_ENTITY_TAG`] matching any tag — a tag is the tag OF an object, so a request that
///   addresses no live object has none to compare: the address the condition is about is not
///   there, which is [`UnmetWriteCondition::NoSuchKey`] ([`NO_SUCH_KEY_CODE`], 404) rather
///   than a precondition that failed, exactly as it is for a conditional WRITE
///   ([`unmet_write_precondition`]). A key whose current version is a delete marker holds no
///   live object either, so it is answered the same way;
/// * "the object is deleted only if its modification times matches the provided `Timestamp`.
///   If the `Timestamp` values do not match, the operation returns a `412 Precondition
///   Failed` error. If the `Timestamp` matches or if the object doesn't exist, the operation
///   returns a `204 Success (No Content)` response"
///   (`$IfMatchLastModifiedTime`), compared at the one-second resolution an HTTP date
///   carries (RFC 9110 §5.6.7), which is the format that member is bound in;
/// * "the object is deleted only if its size matches the provided size in bytes"
///   (`$IfMatchSize`), with the same "or if the object doesn't exist" success.
///
/// "You can use the `If-Match`, `x-amz-if-match-last-modified-time` and
/// `x-amz-if-match-size` conditional headers in conjunction with each-other or individually"
/// (`$IfMatchSize`): every member the request carries has to hold, so one failing condition
/// refuses the delete and is the one the refusal's `<Condition>` names.
///
/// The `If-Match` value is judged by the one rule every entity-tag precondition in this
/// module is judged by ([`entity_tag_matches`]): by VALUE, with the wildcard, the weak marker
/// and the comma-separated list RFC 7232 §3.1 admits.
pub fn unmet_delete_precondition(
    held: Option<&Object>,
    if_match: Option<&str>,
    if_match_last_modified_time: Option<SystemTime>,
    if_match_size: Option<i64>,
) -> Option<UnmetWriteCondition> {
    if let Some(expected) = if_match {
        let Some(object) = held else {
            return Some(UnmetWriteCondition::NoSuchKey);
        };
        if !entity_tag_matches(expected, &object.etag) {
            return Some(UnmetWriteCondition::Condition(IF_MATCH_CONDITION));
        }
    }
    let time_fails = if_match_last_modified_time.is_some_and(|expected| {
        held.is_some_and(|object| {
            whole_seconds(object.last_modified) != whole_seconds(expected)
        })
    });
    if time_fails {
        return Some(UnmetWriteCondition::Condition(
            IF_MATCH_LAST_MODIFIED_TIME_CONDITION,
        ));
    }
    let size_fails = if_match_size.is_some_and(|expected| {
        held.is_some_and(|object| {
            i64::try_from(object.body.len()).unwrap_or(i64::MAX) != expected
        })
    });
    size_fails.then_some(UnmetWriteCondition::Condition(IF_MATCH_SIZE_CONDITION))
}

/// The conditional READ member a request carries that does NOT hold against an object whose
/// entity tag is `etag` and which was last modified at `last_modified`, named by its header,
/// or `None` when every one of them holds.
///
/// The two conditions a read is REFUSED for, as `GetObjectRequest` documents them: "Return
/// the object only if its entity tag (ETag) is the same as the one specified in this header;
/// otherwise, return a 412 Precondition Failed error" (`$IfMatch`), and "Return the object
/// only if it has not been modified since the specified time; otherwise, return a 412
/// Precondition Failed error" (`$IfUnmodifiedSince`). The same member documents the
/// precedence when both are present: "If-Match condition evaluates to true, and;
/// If-Unmodified-Since condition evaluates to false; then, S3 returns 200 OK and the data
/// requested" — so an `If-Match` that holds decides the read on its own.
///
/// `If-Unmodified-Since` is compared at the one-second resolution an HTTP date has (RFC 9110
/// §5.6.7), so an object stored within the same second as the date it is judged against
/// counts as unmodified since it.
///
/// ONE place for the rule, so a GET and a HEAD of the same object cannot disagree about
/// whether its preconditions hold or about which header a refusal names.
pub fn unmet_read_precondition(
    etag: &str,
    last_modified: SystemTime,
    if_match: Option<&str>,
    if_unmodified_since: Option<SystemTime>,
) -> Option<&'static str> {
    if let Some(expected) = if_match {
        return (etag_value(etag) != etag_value(expected)).then_some(IF_MATCH_CONDITION);
    }
    if_unmodified_since
        .is_some_and(|since| whole_seconds(last_modified) > whole_seconds(since))
        .then_some(IF_UNMODIFIED_SINCE_CONDITION)
}

/// Whether a READ of an object whose entity tag is `etag` and which was last modified at
/// `last_modified` is answered NOT MODIFIED under the two conditions a read states for a
/// cached copy, or `false` when the read is to be served.
///
/// The two conditions as `GetObjectRequest` documents them: "Return the object only if its
/// entity tag (ETag) is different from the one specified in this header; otherwise, return a
/// 304 Not Modified error" (`$IfNoneMatch`), and "Return the object only if it has been
/// modified since the specified time; otherwise, return a 304 Not Modified error"
/// (`$IfModifiedSince`). The same member documents the precedence when both are present:
/// "If-None-Match condition evaluates to false, and; If-Modified-Since condition evaluates
/// to true; then, S3 returns 304 Not Modified" — so an `If-None-Match` decides the read on
/// its own, exactly as an `If-Match` decides [`unmet_read_precondition`] on its own.
///
/// `If-None-Match` is judged the way every other entity-tag condition in this module is
/// ([`entity_tag_matches`]: by value, with the wildcard, the weak marker and the
/// comma-separated list), and `If-Modified-Since` at the one-second resolution an HTTP date
/// has (RFC 9110 §5.6.7), so an object stored within the same second as the date it is
/// judged against counts as not modified since it.
///
/// ONE place for the rule, so a GET and a HEAD of the same object cannot disagree about
/// whether the caller's copy is current.
pub fn read_is_not_modified(
    etag: &str,
    last_modified: SystemTime,
    if_none_match: Option<&str>,
    if_modified_since: Option<SystemTime>,
) -> bool {
    if let Some(expected) = if_none_match {
        return entity_tag_matches(expected, etag);
    }
    if_modified_since
        .is_some_and(|since| whole_seconds(last_modified) <= whole_seconds(since))
}

/// `instant` truncated to whole seconds since the epoch, the resolution an HTTP date carries
/// — `None` for an instant before the epoch, which no object this store holds was written
/// at.
pub(super) fn whole_seconds(instant: SystemTime) -> Option<u64> {
    instant
        .duration_since(::std::time::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// The source one `x-amz-copy-source` value names: the bucket, the key and the stored
/// version it addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySource {
    /// The source bucket, URL-decoded.
    pub bucket: String,
    /// The source object's key, URL-decoded.
    pub key: String,
    /// The stored version the value's `?versionId=` named, or `None` when it named none —
    /// in which case the copy is of the key's current version.
    pub version_id: Option<String>,
}

/// The `?versionId=` subresource split off a RAW `x-amz-copy-source` value: the path half
/// and the version id half, or `None` for a value whose query names anything else.
///
/// The split is made on the value AS SENT, before any percent-decoding: the value "must be
/// URL-encoded", so a `?` a KEY carries arrives as `%3F` and the only unescaped `?` left is
/// the one that introduces the subresource — which is what makes `versionId` the only
/// subresource such a value can carry. A value carrying no `?` at all names no version.
fn copy_source_halves(value: &str) -> Option<(&str, Option<&str>)> {
    match value.split_once('?') {
        None => Some((value, None)),
        Some((path, query)) => match query.strip_prefix("versionId=") {
            Some(version_id) if !version_id.is_empty() => Some((path, Some(version_id))),
            _ => None,
        },
    }
}

/// Why an `x-amz-copy-source` value names no source object — which decides the sentence S3
/// refuses it with.
///
/// The value is read as a REFERENCE first and as an object key second, so the two faults are
/// the two halves of that reading: a value that is not a `<bucket>/<key>` reference at all,
/// and one that names a bucket and then no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopySourceFault {
    /// The value is not a reference: its escapes do not decode, its first segment is no
    /// bucket name, or it carries a subresource that is not `versionId`.
    Uri,
    /// The value names a bucket and no object key after it.
    ObjectKey,
}

impl CopySourceFault {
    /// The `<Message>` S3 refuses this fault with — fixed text per fault, naming which half
    /// of the reference is at fault and nothing of the value itself.
    pub fn message(&self) -> &'static str {
        match self {
            Self::Uri => INVALID_COPY_SOURCE_URI_MESSAGE,
            Self::ObjectKey => INVALID_COPY_SOURCE_KEY_MESSAGE,
        }
    }
}

/// Read an `x-amz-copy-source` value into the bucket, key and version it names, or report
/// why it names no source object at all.
///
/// "Specify the name of the source bucket and the key of the source object, separated by a
/// slash (/). For example, to copy the object `reports/january.pdf` from the general
/// purpose bucket `awsexamplebucket`, use `awsexamplebucket/reports/january.pdf`. The
/// value must be URL-encoded." A stored version is addressed by appending
/// `?versionId=<version-id>` to that value
/// (`smithy_query(query="doc",shape="CopyObjectRequest",member="CopySource")`).
///
/// So the value is DECODED first and read afterwards: the whole path half is decoded in one
/// pass as the URL-encoded text the member says it is ([`url_decoded`] states what that
/// means character for character) and the bucket is then what stands before the FIRST slash
/// of the decoded text, with the key — slashes of its own included — after it. A client that
/// encodes the separator too (`bucket%2Fdir%2Ffile`, which every SDK produces for a key
/// carrying a slash or a space) therefore names exactly the object a client that left it
/// literal does, instead of naming no bucket-and-key pair. The member's own pattern
/// (`^\/?.+\/.+$`) allows one leading slash, which is stripped before the split.
///
/// The bucket half has to be SHAPED like a bucket name
/// ([`crate::service::bucket_name_shaped`]): a value whose first segment cannot be a bucket
/// name — the scheme of a URI, say — addresses no object in any account, so it is a value
/// this member cannot carry rather than a bucket that is missing
/// ([`CopySourceFault::Uri`]). A value whose bucket half IS one and which names nothing
/// after it carries no object key instead ([`CopySourceFault::ObjectKey`]).
///
/// ONE place for the rule, so every operation that takes a copy source — the whole-object
/// copy and the part copy, which document the member identically — addresses exactly the
/// same objects and refuses exactly the same values.
pub fn parse_copy_source(value: &str) -> Result<CopySource, CopySourceFault> {
    let Some((path, version_id)) = copy_source_halves(value) else {
        return Err(CopySourceFault::Uri);
    };
    let Some(path) = url_decoded(path) else {
        return Err(CopySourceFault::Uri);
    };
    let path = path.strip_prefix('/').unwrap_or(&path);
    let (bucket, key) = match path.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        // No separator at all: the whole value is the bucket half, so what is missing is
        // the key — for a value that could be a bucket name. Anything else is no reference.
        None => (path, ""),
    };
    if !crate::service::bucket_name_shaped(bucket) {
        return Err(CopySourceFault::Uri);
    }
    if key.is_empty() {
        return Err(CopySourceFault::ObjectKey);
    }
    let version_id = match version_id {
        None => None,
        Some(version_id) => match url_decoded(version_id) {
            Some(version_id) => Some(version_id),
            None => return Err(CopySourceFault::Uri),
        },
    };
    Ok(CopySource {
        bucket: bucket.to_string(),
        key: key.to_string(),
        version_id,
    })
}

/// The fault `value` carries as an `x-amz-copy-source`, or `None` for a value that names a
/// source object.
///
/// The same reading as [`parse_copy_source`], for the one caller that has the value and not
/// the request: the boundary refusal of a value the modelled pattern rejects, which is
/// worded from the same two faults as a value that reaches a handler
/// (`crate::service::published_refusal`).
pub fn copy_source_fault(value: &str) -> Option<CopySourceFault> {
    parse_copy_source(value).err()
}

/// One `x-amz-copy-source` value in the form the modelled member's own pattern is applied
/// to: the path half decoded, with the characters that decoding gives a meaning of their own
/// put back as escapes, and the `?versionId=` half left exactly as it arrived.
///
/// The member's pattern (`^\/?.+\/.+$`) is enforced against the value AS SENT, and a value
/// whose separator is encoded (`bucket%2Fdir%2Ffile`) carries no literal slash to satisfy it
/// — so the request would be refused for its syntax instead of being resolved against the
/// source it names. Decoding the path half first is what S3 does
/// ([`parse_copy_source`] states the rule), and this is where the request's own header is
/// put into that form, so the pattern is applied to the DECODED reference.
///
/// `%`, `?` and `+` are re-escaped because the decoded text is read by [`parse_copy_source`]
/// again: left literal, a `%` would be taken for the start of an escape, a `?` for the
/// subresource delimiter and a `+` for a space, so a key carrying any of them would come out
/// as a different key. Everything else is left decoded, which makes this form stable — a
/// value already in it is returned unchanged.
///
/// `None` when the escapes do not decode to UTF-8, which no bucket name or object key this
/// store holds is: such a value is left exactly as it arrived for the handler to refuse.
pub fn canonical_copy_source(value: &str) -> Option<String> {
    let (path, version_id) = copy_source_halves(value)?;
    let decoded = url_decoded(path)?;
    let decoded =
        ::percent_encoding::utf8_percent_encode(&decoded, &COPY_SOURCE_RESERVED).to_string();
    Some(match version_id {
        None => decoded,
        Some(version_id) => format!("{decoded}?versionId={version_id}"),
    })
}

/// The three characters a DECODED copy source keeps as escapes — see
/// [`canonical_copy_source`] for why each has to: they are the three that
/// [`parse_copy_source`] reads as something other than themselves.
const COPY_SOURCE_RESERVED: ::percent_encoding::AsciiSet = ::percent_encoding::AsciiSet::EMPTY
    .add(b'%')
    .add(b'?')
    .add(b'+');

/// One URL-ENCODED element of a copy source, decoded — or `None` when its escapes do not
/// decode to UTF-8, which no bucket name or object key this store holds is.
///
/// The member's value "must be URL-encoded"
/// (`smithy_query(query="doc",shape="CopyObjectRequest",member="CopySource")`), so it is read
/// as one URL-encoded string: every `%XX` escape stands for the byte it spells, and a `+`
/// stands for a SPACE — the two substitutions URL encoding makes. A key that really carries a
/// plus therefore has to arrive as `%2B`, exactly as a key carrying a slash of its own has to
/// arrive as `%2F` for the separator to stay the separator; a value spelling the plus
/// literally names the key with a space in that position, and resolves (or fails to resolve)
/// against the object of THAT name. The `?versionId=` half is read by the same rule, because
/// it is part of the same URL-encoded value.
///
/// The same reading a QUERY parameter gets (`smithy_server_runtime`'s own query decode) and
/// the opposite of the one a PATH LABEL gets, where a plus is a plus
/// (`service_common::percent_decode_path_label`, which documents the distinction). Neither
/// stands in for this one: both are lossy on an escape that is not UTF-8, where a copy source
/// has to report that it names no object this store could hold rather than carry a
/// replacement character into a key.
pub(super) fn url_decoded(value: &str) -> Option<String> {
    let spaced: ::std::borrow::Cow<'_, str> = match value.contains('+') {
        true => ::std::borrow::Cow::Owned(value.replace('+', " ")),
        false => ::std::borrow::Cow::Borrowed(value),
    };
    ::percent_encoding::percent_decode_str(&spaced)
        .decode_utf8()
        .ok()
        .map(|decoded| decoded.into_owned())
}

/// The prefix of the second documented `x-amz-copy-source` form — the ARN of an object "as
/// accessed through the access point", or through S3 on Outposts, which this emulator holds
/// none of.
pub const ACCESS_POINT_ARN_PREFIX: &str = "arn:";

/// Whether a copy source names an object by ARN rather than by `bucket/key`.
///
/// The one leading slash the member's pattern allows is stripped the way
/// [`parse_copy_source`] strips it, so a value is classified the same here and there.
pub fn copy_source_is_arn(value: &str) -> bool {
    value
        .strip_prefix('/')
        .unwrap_or(value)
        .starts_with(ACCESS_POINT_ARN_PREFIX)
}

/// The four copy-source conditional members one request carried, as
/// [`unmet_copy_source_precondition`] judges them.
///
/// Every operation that takes a copy source declares the same four
/// (`x-amz-copy-source-if-match`, `-if-none-match`, `-if-modified-since` and
/// `-if-unmodified-since`), so the values travel as one struct and the rule lives in one
/// place.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopySourceConditions<'a> {
    pub if_match: Option<&'a str>,
    pub if_none_match: Option<&'a str>,
    /// The instant `x-amz-copy-source-if-modified-since` named, in whole seconds since the
    /// epoch — the unit the HTTP date it arrives as carries.
    pub if_modified_since: Option<i64>,
    /// The same for `x-amz-copy-source-if-unmodified-since`.
    pub if_unmodified_since: Option<i64>,
}

/// The copy-source precondition a request carries that does NOT hold against `source`,
/// named by its header, or `None` when every one of them holds.
///
/// The four members are evaluated in the order RFC 7232 gives them — an entity-tag
/// precondition first, and the date precondition beside it only when that tag member is
/// absent — which is exactly what the two combinations the members document describe:
/// "if both the x-amz-copy-source-if-match and x-amz-copy-source-if-unmodified-since
/// headers are present … x-amz-copy-source-if-match condition evaluates to true
/// [and] x-amz-copy-source-if-unmodified-since condition evaluates to false" answers 200
/// and copies, while "x-amz-copy-source-if-none-match condition evaluates to false [and]
/// x-amz-copy-source-if-modified-since condition evaluates to true" answers 412.
pub fn unmet_copy_source_precondition(
    conditions: &CopySourceConditions<'_>,
    source: &Object,
) -> Option<&'static str> {
    // The stored time is compared at whole-second granularity, because that is the
    // granularity of the HTTP-date these members carry and of the `Last-Modified` the
    // client read the value from.
    let modified_at = i64::try_from(whole_seconds(source.last_modified).unwrap_or_default())
        .unwrap_or(i64::MAX);
    if let Some(header) = conditions.if_match {
        if !entity_tag_matches(header, &source.etag) {
            return Some(COPY_SOURCE_IF_MATCH_CONDITION);
        }
    } else if let Some(since) = conditions.if_unmodified_since {
        // "Copies the object if it hasn't been modified since the specified time."
        if modified_at > since {
            return Some(COPY_SOURCE_IF_UNMODIFIED_SINCE_CONDITION);
        }
    }
    if let Some(header) = conditions.if_none_match {
        if entity_tag_matches(header, &source.etag) {
            return Some(COPY_SOURCE_IF_NONE_MATCH_CONDITION);
        }
    } else if let Some(since) = conditions.if_modified_since {
        // "Copies the object if it has been modified since the specified time."
        if modified_at <= since {
            return Some(COPY_SOURCE_IF_MODIFIED_SINCE_CONDITION);
        }
    }
    None
}

/// Whether an entity-tag precondition value names `etag`.
///
/// An `ETag` is reported inside quotes, and a client may send the value back with or
/// without them, as a comma-separated list, or as the [`ANY_ENTITY_TAG`] wildcard that names
/// any existing entity (RFC 7232 §2.3, §3.1) — so each candidate is compared on the tag's
/// own value ([`etag_value`]), with the weak marker stripped too.
pub fn entity_tag_matches(header: &str, etag: &str) -> bool {
    let stored = etag_value(etag);
    header.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == ANY_ENTITY_TAG || etag_value(candidate.trim_start_matches("W/")) == stored
    })
}

/// The same digest as [`object_etag`], Base64 encoded the way the `Content-MD5` header
/// carries it: "The Base64 encoded 128-bit MD5 digest of the data"
/// (`PutObjectRequest$ContentMD5`), which is what makes that header checkable.
pub fn content_md5(body: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(Md5::digest(body))
}

/// The `ChecksumType` of a checksum computed over the object's own bytes, whole.
///
/// One of the two values `ChecksumType` carries
/// (`smithy_query(query="shape",shape="ChecksumType")` publishes `[COMPOSITE,
/// FULL_OBJECT]`), and the one a single write's checksum has: there are no parts for it to
/// be composed of.
pub const CHECKSUM_TYPE_FULL_OBJECT: &str = "FULL_OBJECT";

/// The `ChecksumType` of a checksum computed over the CHECKSUMS of an object's parts —
/// "the checksum type … determines how part-level checksums are combined to create an
/// object-level checksum for multipart objects"
/// (`CompleteMultipartUploadRequest$ChecksumType`) — and the one a completed multipart
/// upload's checksum has unless the algorithm admits no such composition.
pub const CHECKSUM_TYPE_COMPOSITE: &str = "COMPOSITE";

/// The `ChecksumAlgorithm` values whose checksum is only ever over the whole object.
///
/// "The CRC64NVME checksum is always a full object checksum"
/// (`CompleteMultipartUploadRequest$ChecksumCRC64NVME`), so a multipart object checksummed
/// with it carries [`CHECKSUM_TYPE_FULL_OBJECT`] rather than a composition of its parts'
/// checksums.
pub const FULL_OBJECT_CHECKSUM_ALGORITHMS: [&str; 1] = ["CRC64NVME"];

/// The `ChecksumAlgorithm` an object S3 stores carries when the request that stored it
/// named none of its own.
///
/// S3 protects every object it stores with a checksum, so an object is stored with one
/// whether or not a client asked for it: the value is reported by the write that stored the
/// object and by every later read of it. The algorithm of that default is the one the model
/// states is "always a full object checksum" (`ChecksumCRC64NVME` on every output that
/// carries it) and the only `Checksum*` member whose documentation does NOT carry the
/// "only present if the checksum was uploaded with the object" caveat its siblings do
/// (`GetObjectOutput$ChecksumCRC32` and the rest) — so it is the one a client never
/// uploaded that is nonetheless there to read.
pub const DEFAULT_CHECKSUM_ALGORITHM: &str = "CRC64NVME";

/// The `ChecksumAlgorithm` values a multipart object may carry a
/// [`CHECKSUM_TYPE_FULL_OBJECT`] checksum under.
///
/// A multipart upload's object-level checksum is settled from its PARTS, so a full-object
/// value is only available for an algorithm whose part digests combine into the digest of
/// the whole object — the CRC-based ones. The Amazon S3 User Guide (*Checking object
/// integrity*) publishes full object checksums for exactly those (`CRC32`, `CRC32C` and
/// `CRC64NVME`) and composite checksums for the others, which is why a multipart upload
/// asking for `FULL_OBJECT` under any other algorithm names a pair S3 does not settle (see
/// [`settle_multipart_checksum_type`]).
pub const FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS: [&str; 3] = ["CRC32", "CRC32C", "CRC64NVME"];

/// The `ChecksumType` a multipart upload under `algorithm` is settled with when its
/// initiating request asked for `requested` (`None` when it asked for no type at all), or
/// `None` when the two name a combination S3 does not settle.
///
/// The rule, for every `ChecksumAlgorithm` the model publishes:
///
/// * an algorithm whose checksum "is always a full object checksum"
///   ([`FULL_OBJECT_CHECKSUM_ALGORITHMS`]) settles [`CHECKSUM_TYPE_FULL_OBJECT`], and
///   admits no other type;
/// * [`CHECKSUM_TYPE_FULL_OBJECT`] is settled for an algorithm whose parts' checksums
///   combine into the whole object's ([`FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS`]) when the
///   request asks for it, and for no other algorithm;
/// * every other algorithm settles [`CHECKSUM_TYPE_COMPOSITE`] — the composition over the
///   parts' checksums that "determines how part-level checksums are combined to create an
///   object-level checksum for multipart objects"
///   (`CreateMultipartUploadRequest$ChecksumType`).
///
/// ONE place for the rule, so the type an initiation reports and records, the type a later
/// completion composes the value under ([`multipart_checksum`]), and the type a reader of
/// the upload reports cannot disagree.
pub fn settle_multipart_checksum_type(
    algorithm: &str,
    requested: Option<&str>,
) -> Option<&'static str> {
    let always_full_object = FULL_OBJECT_CHECKSUM_ALGORITHMS.contains(&algorithm);
    let may_be_full_object = FULL_OBJECT_CAPABLE_CHECKSUM_ALGORITHMS.contains(&algorithm);
    match requested {
        Some(CHECKSUM_TYPE_FULL_OBJECT) => may_be_full_object.then_some(CHECKSUM_TYPE_FULL_OBJECT),
        Some(CHECKSUM_TYPE_COMPOSITE) => (!always_full_object).then_some(CHECKSUM_TYPE_COMPOSITE),
        // The request named no type at all: the one its algorithm settles.
        None if always_full_object => Some(CHECKSUM_TYPE_FULL_OBJECT),
        None => Some(CHECKSUM_TYPE_COMPOSITE),
        // A value outside the two the enum publishes is no type this settles, so it is
        // refused like any other pair it does not settle rather than folded into "the
        // request named none", which would settle a type the request never asked for.
        Some(_) => None,
    }
}

/// The message S3 refuses a request naming a `ChecksumType` its `ChecksumAlgorithm` cannot
/// produce with.
///
/// The fixed text is "The ", " checksum type cannot be used with the " and " checksum
/// algorithm."; the two variable parts come from the request — the type as the enum spells it
/// (upper case, [`CHECKSUM_TYPE_FULL_OBJECT`] / [`CHECKSUM_TYPE_COMPOSITE`]) and the algorithm
/// rendered LOWER case, the spelling the `x-amz-checksum-*` member names carry it in. The
/// sentence therefore says which of the two headers has to change, in whichever direction the
/// pair is impossible ([`settle_multipart_checksum_type`] decides that it is).
pub fn incompatible_checksum_pair_message(checksum_type: &str, algorithm: &str) -> String {
    format!(
        "The {checksum_type} checksum type cannot be used with the {} checksum algorithm.",
        algorithm.to_ascii_lowercase()
    )
}

/// The RAW digest bytes the `ChecksumAlgorithm` value `algorithm` computes over `data`, or
/// `None` for a value this emulator has no implementation of.
///
/// Each algorithm is computed by a crate that implements it rather than by anything written
/// here: the five S3 has long published (`CRC32`, `CRC32C`, `CRC64NVME`, `SHA1`, `SHA256`)
/// through `aws-smithy-checksums`, the AWS SDK's own implementation of exactly these
/// checksums, and the others through the hash crate for each (`sha2` for `SHA512`, `md-5`
/// for `MD5`, `xxhash-rust` for the three `XXHASH` values). The bytes are the digest's own,
/// big-endian for the integer-valued ones, because that is what a Base64 checksum value and
/// a composition over part checksums are both built from.
pub(super) fn checksum_digest(algorithm: &str, data: &[u8]) -> Option<Vec<u8>> {
    match algorithm {
        "CRC32" | "CRC32C" | "CRC64NVME" | "SHA1" | "SHA256" => {
            // The SDK crate names these in lowercase, which is the spelling its
            // `FromStr` accepts (and the spelling the header names carry).
            let parsed: ::aws_smithy_checksums::ChecksumAlgorithm =
                algorithm.to_ascii_lowercase().parse().ok()?;
            let mut checksum = parsed.into_impl();
            checksum.update(data);
            Some(checksum.finalize().to_vec())
        }
        "SHA512" => {
            use ::sha2::Digest as _;
            Some(::sha2::Sha512::digest(data).to_vec())
        }
        "MD5" => Some(Md5::digest(data).to_vec()),
        "XXHASH64" => Some(::xxhash_rust::xxh64::xxh64(data, 0).to_be_bytes().to_vec()),
        "XXHASH3" => Some(::xxhash_rust::xxh3::xxh3_64(data).to_be_bytes().to_vec()),
        "XXHASH128" => Some(::xxhash_rust::xxh3::xxh3_128(data).to_be_bytes().to_vec()),
        _ => None,
    }
}

/// The checksum of `data` under `algorithm`, as a response reports it: "the Base64 encoded
/// … checksum of the object" (every `Checksum*` member's own documentation), over the whole
/// of the object's bytes.
///
/// `None` for an algorithm [`checksum_digest`] has no implementation of, in which case
/// nothing is recorded and nothing is reported — never a value under another algorithm.
pub fn full_object_checksum(algorithm: &str, data: &[u8]) -> Option<String> {
    Some(::aws_smithy_types::base64::encode(checksum_digest(
        algorithm, data,
    )?))
}

/// The checksum ONE whole-object write records with the object it stores: the digest of the
/// bytes it stored under the algorithm the request named, or under
/// [`DEFAULT_CHECKSUM_ALGORITHM`] when it named none.
///
/// The rule is the model's own: a `Checksum*` member of a write's answer "is present if the
/// object was uploaded with the `CRC64NVME` checksum algorithm, or if it was uploaded without
/// a checksum (and Amazon S3 added the default checksum, `CRC64NVME`, to the uploaded object)"
/// (`PutObjectOutput$ChecksumCRC64NVME`) — so EVERY stored object carries a checksum, and the
/// one an unchecksummed write gets is the default algorithm's. The type is
/// [`CHECKSUM_TYPE_FULL_OBJECT`]: a single write stores the whole object, so there are no
/// parts for a composition to be over ("for `PutObject` uploads, the checksum type is always
/// `FULL_OBJECT`", `PutObjectRequest$ChecksumType`).
///
/// ONE place for it, so every whole-object write of this service records the same checksum
/// over the same bytes, and the value a later read reports describes those bytes. `None` only
/// for an algorithm [`checksum_digest`] has no implementation of, in which case the object
/// carries no checksum rather than one computed under another algorithm.
pub fn stored_object_checksum(algorithm: Option<&str>, body: &[u8]) -> Option<ObjectChecksum> {
    let algorithm = algorithm.unwrap_or(DEFAULT_CHECKSUM_ALGORITHM);
    Some(ObjectChecksum {
        algorithm: algorithm.to_string(),
        value: full_object_checksum(algorithm, body)?,
        checksum_type: CHECKSUM_TYPE_FULL_OBJECT.to_string(),
    })
}

/// The COMPOSITE checksum of an object assembled from `parts`, under `algorithm`.
///
/// The rule is the one the Amazon S3 User Guide states for a multipart object's
/// object-level checksum (*Checking object integrity*): S3 checksums each part, then
/// "concatenates the … checksums … and then calculates the checksum of these concatenated
/// values", reporting it "with the total number of parts" after a dash — the same
/// composition an entity tag of a multipart object is built with ([`multipart_object_etag`]),
/// over the request's checksum algorithm instead of MD5. So a COMPOSITE value is the
/// algorithm's digest of the parts' RAW digests, Base64 encoded, followed by `-<parts>`.
pub fn composite_checksum(algorithm: &str, parts: &[&[u8]]) -> Option<String> {
    let mut digests: Vec<u8> = Vec::new();
    for part in parts {
        digests.extend_from_slice(&checksum_digest(algorithm, part)?);
    }
    let combined = checksum_digest(algorithm, &digests)?;
    Some(format!(
        "{}-{}",
        ::aws_smithy_types::base64::encode(combined),
        parts.len()
    ))
}

/// The checksum one completed multipart upload's object carries: its value and its
/// `ChecksumType`, under the algorithm the upload was initiated with — or, for an upload
/// that named no algorithm at all, under [`DEFAULT_CHECKSUM_ALGORITHM`], because the object
/// a completion stores carries a checksum whether or not the upload configured one. `None`
/// only for an algorithm this emulator cannot compute, in which case the object carries no
/// checksum rather than one it did not compute.
///
/// The type is the one [`settle_multipart_checksum_type`] settles for the upload's
/// algorithm and the type it asked for — the same rule the initiation reported and recorded
/// — so an upload whose pair S3 does not settle carries no checksum rather than one under a
/// type it never agreed to. The value follows from the type — a composition over the parts,
/// or the digest of the whole assembled object — so the two can never describe different
/// computations.
pub fn multipart_checksum(
    algorithm: Option<&str>,
    requested_type: Option<&str>,
    parts: &[&[u8]],
) -> Option<ObjectChecksum> {
    let algorithm = algorithm.unwrap_or(DEFAULT_CHECKSUM_ALGORITHM);
    let checksum_type = settle_multipart_checksum_type(algorithm, requested_type)?;
    let value = if checksum_type == CHECKSUM_TYPE_COMPOSITE {
        composite_checksum(algorithm, parts)?
    } else {
        full_object_checksum(algorithm, &parts.concat())?
    };
    Some(ObjectChecksum {
        algorithm: algorithm.to_string(),
        value,
        checksum_type: checksum_type.to_string(),
    })
}

/// The checksum one stored object was written with: which algorithm computed it, the value
/// a reader reports, and whether it was computed over the object's bytes or over its parts'
/// checksums.
///
/// Recorded by the write that stored the object, from the bytes it stored, so every later
/// read reports the same value — "this checksum is only present if the checksum was uploaded
/// with the object" (`Checksum*`'s own documentation), which is why an object written with no
/// checksum algorithm carries none of this and reports no checksum at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectChecksum {
    /// The `ChecksumAlgorithm` value the write named, which decides WHICH `Checksum*`
    /// member carries [`Self::value`].
    pub algorithm: String,
    /// The Base64 checksum, with the `-<parts>` suffix a composition carries.
    pub value: String,
    /// [`CHECKSUM_TYPE_FULL_OBJECT`] or [`CHECKSUM_TYPE_COMPOSITE`].
    pub checksum_type: String,
}

impl ObjectChecksum {
    /// How many bytes this record retains — the strings kept beside the object, which the
    /// per-bucket and service-wide budgets count the way they count every other string an
    /// object holds ([`ObjectEncryption::retained_bytes`] counts its own the same way).
    pub fn retained_bytes(&self) -> usize {
        self.algorithm.len() + self.value.len() + self.checksum_type.len()
    }
}

/// One part a completed multipart upload assembled an object from, as a read of the
/// object's attributes reports it.
///
/// The members `ObjectPart` declares for a general purpose bucket
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ObjectPart")`): "the part number
/// identifying the part", "the size of the uploaded part in bytes", and the part's own
/// checksum under the algorithm the upload was initiated with — nothing else of the part
/// survives the completion, because its bytes become the object's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPartRecord {
    /// "The part number identifying the part. This value is a positive integer between 1
    /// and 10,000" (`ObjectPart$PartNumber`) — the number the completion assembled it under.
    pub part_number: i32,
    /// "The size of the uploaded part in bytes" (`ObjectPart$Size`).
    pub size: i64,
    /// The Base64 checksum of this part's own bytes, under the object's checksum algorithm,
    /// or `None` when the upload named no algorithm — in which case a read of the object's
    /// attributes reports no `Part` element at all for a general purpose bucket ("if an
    /// additional checksum … isn't applied to the object specified in the request, the
    /// response doesn't return the Part element", `GetObjectAttributesParts$Parts`).
    pub checksum: Option<String>,
}

impl ObjectPartRecord {
    /// How many bytes this record retains: the checksum string kept with it, which the
    /// budgets count like every other string beside an object.
    pub fn retained_bytes(&self) -> usize {
        self.checksum.as_ref().map_or(0, String::len)
    }
}

/// One span of an object's bytes: the first and last byte INCLUSIVE, the way HTTP's
/// `Content-Range` and `Range` both state a span.
///
/// Non-empty by construction ([`ByteSpan::new`]), because an empty span is not a span HTTP
/// can state: `Content-Range` names a first and a last byte, so a response can only report
/// one that holds at least one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSpan {
    /// The first byte of the span, counted from zero.
    pub(super) first: usize,
    /// The LAST byte of the span, inclusive.
    pub(super) last: usize,
}

impl ByteSpan {
    /// The span from `first` to `last` inclusive, or `None` when that is no span at all
    /// (`last` before `first`).
    pub fn new(first: usize, last: usize) -> Option<Self> {
        (first <= last).then_some(Self { first, last })
    }

    /// The first byte of the span.
    pub fn first(self) -> usize {
        self.first
    }

    /// The last byte of the span, inclusive.
    pub fn last(self) -> usize {
        self.last
    }

    /// How many bytes the span holds — what `Content-Length` reports for a response carrying
    /// it, since "Content-Length is the length of the body actually being returned".
    pub fn len(self) -> usize {
        self.last - self.first + 1
    }

    /// Never empty: see [`ByteSpan`].
    pub fn is_empty(self) -> bool {
        false
    }

    /// Those bytes of `body`, clamped to what it holds so a span can never read past it.
    pub fn slice(self, body: &[u8]) -> Vec<u8> {
        let first = self.first.min(body.len());
        let end = (self.last + 1).min(body.len());
        body[first..end].to_vec()
    }

    /// The `Content-Range` header value for this span of a representation of `total` bytes:
    /// `bytes <first>-<last>/<total>`, the one form RFC 9110 (HTTP Semantics, "Content-Range")
    /// defines for a `bytes` range.
    pub fn content_range(self, total: usize) -> String {
        format!("bytes {}-{}/{}", self.first, self.last, total)
    }
}

/// What a `Range` header asks of a representation of a given size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeRequest {
    /// The header asks for exactly this span.
    Span(ByteSpan),
    /// The header is well formed but asks for bytes the representation does not have, which
    /// is the one range condition HTTP answers with a refusal ([`INVALID_RANGE_CODE`]).
    Unsatisfiable,
    /// The header asks for nothing this emulator can answer as a single range — a form the
    /// `bytes` unit does not define, or several ranges at once, which "Amazon S3 doesn't
    /// support retrieving multiple ranges of data per `GET` request"
    /// (`GetObjectRequest$Range`). RFC 9110 lets a server "ignore a Range header field",
    /// which is what S3 does: the whole representation is served.
    Ignored,
}

/// What the `Range` header `header` asks of a representation of `total` bytes.
///
/// The `bytes` unit's three forms, from RFC 9110's "Range" grammar: `bytes=<first>-<last>`,
/// `bytes=<first>-` (to the end) and `bytes=-<suffix>` (the last `<suffix>` bytes). A
/// `<last>` past the end is clamped to it, because the span is "the remaining bytes of the
/// representation" in that case, and a suffix longer than the representation selects the
/// whole of it.
///
/// One place for the rule, so every reader that serves a ranged request ([`Object`]'s own
/// readers) answers the same span, the same refusal and the same ignored header.
pub fn requested_range(header: &str, total: usize) -> RangeRequest {
    let Some(set) = header.trim().strip_prefix("bytes=") else {
        // Another unit entirely: "An origin server MUST ignore a Range header field that
        // contains a range unit it does not understand" (RFC 9110).
        return RangeRequest::Ignored;
    };
    let mut spans = set.split(',');
    let (Some(single), None) = (spans.next(), spans.next()) else {
        return RangeRequest::Ignored;
    };
    let Some((first, last)) = single.trim().split_once('-') else {
        return RangeRequest::Ignored;
    };
    let (first, last) = (first.trim(), last.trim());
    match (first.parse::<usize>(), last.parse::<usize>()) {
        // `bytes=-<suffix>`: the last `suffix` bytes. A zero-length suffix selects nothing,
        // which is unsatisfiable, and so is any suffix of an empty representation.
        (Err(_), Ok(suffix)) if first.is_empty() => {
            if suffix == 0 || total == 0 {
                return RangeRequest::Unsatisfiable;
            }
            let first = total.saturating_sub(suffix);
            ByteSpan::new(first, total - 1).map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        // `bytes=<first>-` : from `first` to the end of the representation.
        (Ok(first), Err(_)) if last.is_empty() => {
            if first >= total {
                return RangeRequest::Unsatisfiable;
            }
            ByteSpan::new(first, total - 1).map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        // `bytes=<first>-<last>` : the span, with `last` clamped to the final byte. A
        // `<last>` BEFORE `<first>` makes the spec "syntactically invalid" (RFC 9110's
        // `byte-range-spec` grammar), which is a header to ignore rather than a range to
        // refuse: the refusal is for a valid range the representation cannot satisfy.
        (Ok(first), Ok(last)) => {
            if last < first {
                return RangeRequest::Ignored;
            }
            if first >= total {
                return RangeRequest::Unsatisfiable;
            }
            ByteSpan::new(first, last.min(total - 1))
                .map_or(RangeRequest::Unsatisfiable, RangeRequest::Span)
        }
        _ => RangeRequest::Ignored,
    }
}

/// The code S3 answers a read whose `Range` asks for bytes the object does not have:
/// `InvalidRange`, HTTP 416 — not a shape in this model, so it travels through the
/// unmodelled channel.
pub const INVALID_RANGE_CODE: &str = "InvalidRange";
/// See [`INVALID_RANGE_CODE`] — the status that code's entry in Amazon S3's "Error
/// responses" list publishes: "416 Requested Range Not Satisfiable".
pub const INVALID_RANGE_STATUS: u16 = 416;
/// The `<Message>` that same entry publishes — FIXED text, naming neither the range asked
/// for nor the object's size.
pub const INVALID_RANGE_MESSAGE: &str = "The requested range is not satisfiable";

/// The code S3 answers a read whose `partNumber` names no part of the object:
/// `InvalidPartNumber`, HTTP 416 — also unmodelled in this model, from the same published
/// list.
pub const INVALID_PART_NUMBER_CODE: &str = "InvalidPartNumber";
/// See [`INVALID_PART_NUMBER_CODE`].
pub const INVALID_PART_NUMBER_STATUS: u16 = 416;
/// The `<Message>` that code's entry publishes — FIXED text, which is why it names no part
/// number.
pub const INVALID_PART_NUMBER_MESSAGE: &str = "The requested partnumber is not satisfiable";

/// The value `accept-ranges` carries for an object S3 can serve byte ranges of: "Indicates
/// that a range of bytes was specified in the request" (`GetObjectOutput$AcceptRanges`).
///
/// The one range unit HTTP defines over a representation's bytes, and the unit
/// [`requested_range`] reads a `Range` header in — so the header a read ADVERTISES and the
/// unit a read is SERVED in cannot drift apart, and the two readers that advertise it
/// (`GetObject` and `HeadObject`) answer the same value.
pub const ACCEPT_RANGES_BYTES: &str = "bytes";


/// The `Checksum*` members a reader reports for a stored checksum: the one its algorithm
/// names, and the `ChecksumType` beside it. Every other member is `None`, because an object
/// carries the checksum of ONE algorithm — the one its write named.
///
/// One place for the algorithm-to-member mapping, so the four readers that report a stored
/// checksum (the write that computed it, `GetObject`, `HeadObject` and the completion of a
/// multipart upload) cannot map it differently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedChecksums {
    pub crc32: Option<String>,
    pub crc32c: Option<String>,
    pub crc64nvme: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    pub md5: Option<String>,
    pub xxhash64: Option<String>,
    pub xxhash3: Option<String>,
    pub xxhash128: Option<String>,
    pub checksum_type: Option<String>,
}

/// `value` without the `-<parts>` suffix a COMPOSITE checksum carries.
///
/// A composition is reported "with the total number of parts" after a dash (Amazon S3 User
/// Guide, *Checking object integrity*) wherever the VALUE itself is reported — the
/// `x-amz-checksum-*` header of a write or a read, and the element a completion answers —
/// which is what [`composite_checksum`] renders. A read of the object's ATTRIBUTES is the
/// one place that reports the digest ALONE: `GetObjectAttributes` states how many parts the
/// object holds in `ObjectParts$TotalPartsCount` beside the checksum container, so the count
/// is not repeated inside the value there.
///
/// Only a trailing dash followed by digits is dropped: a Base64 checksum value is written in
/// the standard alphabet, which has no `-`, so no part of a digest can be mistaken for the
/// suffix.
pub fn checksum_without_part_count(value: &str) -> &str {
    match value.rsplit_once('-') {
        Some((digest, parts))
            if !parts.is_empty() && parts.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            digest
        }
        _ => value,
    }
}

/// The members a reader reports for `checksum`, or nothing at all for an object that
/// carries none — see [`ReportedChecksums`].
pub fn reported_checksums(checksum: Option<&ObjectChecksum>) -> ReportedChecksums {
    let Some(checksum) = checksum else {
        return ReportedChecksums::default();
    };
    let value = Some(checksum.value.clone());
    let mut reported = ReportedChecksums {
        checksum_type: Some(checksum.checksum_type.clone()),
        ..ReportedChecksums::default()
    };
    match checksum.algorithm.as_str() {
        "CRC32" => reported.crc32 = value,
        "CRC32C" => reported.crc32c = value,
        "CRC64NVME" => reported.crc64nvme = value,
        "SHA1" => reported.sha1 = value,
        "SHA256" => reported.sha256 = value,
        "SHA512" => reported.sha512 = value,
        "MD5" => reported.md5 = value,
        "XXHASH64" => reported.xxhash64 = value,
        "XXHASH3" => reported.xxhash3 = value,
        "XXHASH128" => reported.xxhash128 = value,
        // An algorithm no member of the model names cannot be reported under one; nothing
        // records such a checksum ([`checksum_digest`] computes none for it), so this is
        // unreachable rather than a silent drop.
        _ => return ReportedChecksums::default(),
    }
    reported
}

/// The `x-amz-checksum-*` request header one `ChecksumAlgorithm` value's digest arrives
/// on.
///
/// Derived rather than tabulated: every `Checksum*` member of this model binds
/// `x-amz-checksum-` followed by its algorithm in lowercase
/// (`smithy_query(query="operation",operation="PutObject")` reports `ChecksumSHA256 ->
/// x-amz-checksum-sha256`, `ChecksumCRC64NVME -> x-amz-checksum-crc64nvme`, and so on for
/// all ten), so a refusal names the header the value really arrived on and the two cannot
/// drift apart.
pub fn checksum_header_name(algorithm: &str) -> String {
    format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase())
}

/// The values of the ten `Checksum*` members one write declared — what
/// [`verify_declared_integrity`] judges.
///
/// One struct rather than a table per operation, because the algorithm each member stands
/// for is a property of the MODEL, not of the operation: every write that accepts these
/// headers (`PutObject`, `UploadPart`, `PutObjectAnnotation`) hands over the members it
/// deserialized and the mapping from member to algorithm lives here, in one place, so a
/// `ChecksumAlgorithm` value added to the model is added once.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeclaredChecksums<'a> {
    pub crc32: Option<&'a str>,
    pub crc32c: Option<&'a str>,
    pub crc64nvme: Option<&'a str>,
    pub sha1: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub sha512: Option<&'a str>,
    pub md5: Option<&'a str>,
    pub xxhash64: Option<&'a str>,
    pub xxhash3: Option<&'a str>,
    pub xxhash128: Option<&'a str>,
}

impl<'a> DeclaredChecksums<'a> {
    /// Each `ChecksumAlgorithm` value paired with the value its member carried, in the
    /// model's own member order (`smithy_query(query="operation",operation="PutObject")`:
    /// `ChecksumCRC32`, `ChecksumCRC32C`, `ChecksumCRC64NVME`, `ChecksumSHA1`,
    /// `ChecksumSHA256`, `ChecksumSHA512`, `ChecksumMD5`, `ChecksumXXHASH64`,
    /// `ChecksumXXHASH3`, `ChecksumXXHASH128`) — which is the order a request naming more
    /// than one of them is judged in.
    pub fn pairs(&self) -> [(&'static str, Option<&'a str>); 10] {
        [
            ("CRC32", self.crc32),
            ("CRC32C", self.crc32c),
            ("CRC64NVME", self.crc64nvme),
            ("SHA1", self.sha1),
            ("SHA256", self.sha256),
            ("SHA512", self.sha512),
            ("MD5", self.md5),
            ("XXHASH64", self.xxhash64),
            ("XXHASH3", self.xxhash3),
            ("XXHASH128", self.xxhash128),
        ]
    }

    /// How many of the ten members carried a value — what decides whether the write
    /// declared ONE checksum type or several, which S3 refuses
    /// ([`IntegrityRejection::MultipleChecksumTypes`]).
    pub fn declared_count(&self) -> usize {
        self.pairs()
            .into_iter()
            .filter(|(_, declared)| declared.is_some())
            .count()
    }

    /// The `ChecksumAlgorithm` value of the FIRST member that carried one, in that same
    /// order, or `None` when the write declared no individual checksum — which is what makes
    /// an individual checksum name the algorithm a write's own checksum is computed under
    /// ("if you provide an individual checksum, Amazon S3 ignores any provided
    /// ChecksumAlgorithm parameter", `PutObjectRequest$ChecksumAlgorithm`).
    pub fn named_algorithm(&self) -> Option<&'static str> {
        self.pairs()
            .into_iter()
            .find_map(|(algorithm, declared)| declared.map(|_| algorithm))
    }
}

/// Why a write's declared integrity value cannot be accepted.
///
/// Two classes, because a client branches on them differently: a value that cannot
/// POSSIBLY be a digest of the named width is a malformed request (fix the client), while a
/// well-formed digest that is not the digest of the bytes received says the bytes are not
/// the bytes the client hashed (recompute and retry the same payload). Neither code is a
/// shape in this model, so a handler renders one through the unmodelled channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityRejection {
    /// An `x-amz-checksum-*` value that does not Base64-decode, or decodes to a width the
    /// named algorithm's digest does not have.
    MalformedChecksum {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// A well-formed digest that is not the one the algorithm computes over the payload.
    ChecksumMismatch {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// A `Content-MD5` that does not Base64-decode to a 128-bit digest.
    MalformedContentMd5,
    /// More than one `x-amz-checksum-*` header on one write: each names a checksum TYPE for
    /// the same bytes, and S3 computes and stores exactly one per object, so a request
    /// carrying several asks for something no stored object can report.
    MultipleChecksumTypes,
    /// A well-formed `Content-MD5` that is not the MD5 of the payload.
    ContentMd5Mismatch,
}

impl IntegrityRejection {
    /// The wire error code S3 answers this rejection with.
    ///
    /// A malformed `x-amz-checksum-*` header is `InvalidRequest` — "a parameter or header
    /// in your request isn't valid" — and a malformed `Content-MD5` is `InvalidDigest`, the
    /// code S3's *Error responses* list publishes for that header specifically. Either
    /// value failing the COMPARISON is `BadDigest`, the code that list publishes for a
    /// digest that does not describe the bytes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedChecksum { .. } | Self::MultipleChecksumTypes => INVALID_REQUEST_CODE,
            Self::MalformedContentMd5 => INVALID_DIGEST_CODE,
            Self::ChecksumMismatch { .. } | Self::ContentMd5Mismatch => BAD_DIGEST_CODE,
        }
    }

    /// The HTTP status every one of them is rendered with: 400, from that same list.
    pub fn status(&self) -> u16 {
        BAD_DIGEST_STATUS
    }
}

impl std::fmt::Display for IntegrityRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The header the value arrived on is the only variable part: S3 names the
            // header it could not read and nothing about the value or the width expected.
            Self::MalformedChecksum { algorithm } => write!(
                f,
                "Value for {} header is invalid.",
                checksum_header_name(algorithm)
            ),
            // The algorithm whose digest did not match, as the `ChecksumAlgorithm` enum
            // spells it, which is how S3 names it in this sentence.
            Self::ChecksumMismatch { algorithm } => write!(
                f,
                "The {algorithm} you specified did not match the calculated checksum."
            ),
            Self::MalformedContentMd5 => write!(f, "{INVALID_DIGEST_MESSAGE}"),
            Self::ContentMd5Mismatch => write!(f, "{CONTENT_MD5_MISMATCH_MESSAGE}"),
            Self::MultipleChecksumTypes => write!(f, "{MULTIPLE_CHECKSUM_TYPES_MESSAGE}"),
        }
    }
}

/// The code S3 answers a `Content-MD5` that is not a Base64 128-bit digest at all:
/// "InvalidDigest — The Content-MD5 or checksum value that you specified is not valid"
/// (Amazon S3 *Error responses*, list of error codes), HTTP 400.
pub const INVALID_DIGEST_CODE: &str = "InvalidDigest";

/// See [`INVALID_DIGEST_CODE`] — the text that list publishes for it.
pub const INVALID_DIGEST_MESSAGE: &str =
    "The Content-MD5 or checksum value that you specified is not valid.";

/// The `<Message>` S3 sends with [`INVALID_REQUEST_CODE`] for a write that declared more
/// than one `x-amz-checksum-*` header: "Expecting a single x-amz-checksum- header. Multiple
/// checksum Types are not allowed."
///
/// Fixed text that names the header PREFIX rather than any of the members the request
/// carried, so nothing of the request is interpolated into it.
pub const MULTIPLE_CHECKSUM_TYPES_MESSAGE: &str =
    "Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed.";

/// The `<Message>` S3 sends with [`BAD_DIGEST_CODE`] for a `Content-MD5` that IS a digest
/// but not this payload's: fixed text, with no variable part — it names neither the digest
/// declared nor the one computed.
pub const CONTENT_MD5_MISMATCH_MESSAGE: &str =
    "The Content-MD5 you specified did not match what we received.";

/// Whether the integrity values one write declared describe the bytes it carried.
///
/// ONE rule for every write that accepts them, so the code and the sentence a client
/// branches on are the same whichever operation and whichever header carried the value:
/// each value's SHAPE is judged first — a digest of the wrong width cannot be the digest of
/// anything, so it is refused before any comparison — and only then is it compared against
/// the digest of the payload actually received. `Content-MD5` is judged first because it is
/// the integrity check HTTP itself defines over the body.
///
/// A declared algorithm this emulator cannot compute ([`checksum_digest`] implements every
/// value the model's `ChecksumAlgorithm` publishes) is left unjudged rather than refused: no
/// comparison is possible, so neither answer would be a true statement about the value.
pub fn verify_declared_integrity(
    content_md5: Option<&str>,
    checksums: &DeclaredChecksums<'_>,
    body: &[u8],
) -> Result<(), IntegrityRejection> {
    // One write declares ONE checksum type: "you can only specify one checksum algorithm per
    // request" (`PutObjectRequest$ChecksumAlgorithm` names the single algorithm a write's
    // checksum is computed under), and a stored object carries one checksum
    // ([`Object::checksum`]), so several `x-amz-checksum-*` headers at once ask for something
    // no object can report. Judged before any value is read, because it is a property of the
    // request alone.
    if checksums.declared_count() > 1 {
        return Err(IntegrityRejection::MultipleChecksumTypes);
    }
    if let Some(declared) = content_md5 {
        // "The Base64 encoded 128-bit MD5 digest of the data" (`PutObjectRequest$ContentMD5`)
        // — so a value that is not Base64 at all, or is Base64 of anything other than 16
        // bytes, is not a digest the header may carry.
        let decoded = ::aws_smithy_types::base64::decode(declared.trim())
            .map_err(|_| IntegrityRejection::MalformedContentMd5)?;
        if decoded.len() != MD5_DIGEST_BYTES {
            return Err(IntegrityRejection::MalformedContentMd5);
        }
        if decoded != Md5::digest(body).as_slice() {
            return Err(IntegrityRejection::ContentMd5Mismatch);
        }
    }
    for (algorithm, declared) in checksums.pairs() {
        let Some(declared) = declared else { continue };
        let Some(expected) = checksum_digest(algorithm, body) else {
            continue;
        };
        let decoded = ::aws_smithy_types::base64::decode(declared.trim()).map_err(|_| {
            IntegrityRejection::MalformedChecksum {
                algorithm: algorithm.to_string(),
            }
        })?;
        if decoded.len() != expected.len() {
            return Err(IntegrityRejection::MalformedChecksum {
                algorithm: algorithm.to_string(),
            });
        }
        if decoded != expected {
            return Err(IntegrityRejection::ChecksumMismatch {
                algorithm: algorithm.to_string(),
            });
        }
    }
    Ok(())
}

/// How many bytes an MD5 digest is wide: "the Base64 encoded 128-bit MD5 digest of the
/// data" (`PutObjectRequest$ContentMD5`), which is 16 bytes.
pub(super) const MD5_DIGEST_BYTES: usize = 128 / 8;

/// Why the `Checksum*` members a write declared for a whole MULTIPART object cannot be
/// accepted, or `None` when they describe `computed` — the checksum the assembled object
/// really carries ([`multipart_checksum`]).
///
/// The same rule [`verify_declared_integrity`] applies to the bytes of a one-shot write,
/// over the value a COMPOSITION produces instead: a completion "can be used as a data
/// integrity check to verify that the data received is the same data that was originally
/// sent" (`CompleteMultipartUploadRequest$ChecksumCRC64NVME` and its siblings), and the
/// digest the object carries is the one the initiation's algorithm and type settled — so a
/// declared value that is not that one, and a value declared under any OTHER algorithm, is
/// a digest the object does not have. Either is [`BAD_DIGEST_CODE`], the code that class
/// publishes for a digest that does not describe what the service holds.
///
/// Declaring several at once is refused before any comparison, exactly as it is on a
/// one-shot write: one object carries one checksum.
pub fn verify_declared_object_checksum(
    declared: &DeclaredChecksums<'_>,
    computed: Option<&ObjectChecksum>,
) -> Result<(), ObjectChecksumRejection> {
    if declared.declared_count() > 1 {
        return Err(ObjectChecksumRejection::MultipleChecksumTypes);
    }
    for (algorithm, value) in declared.pairs() {
        let Some(value) = value else { continue };
        let describes = computed.is_some_and(|checksum| {
            checksum.algorithm == algorithm && checksum.value == value.trim()
        });
        if !describes {
            return Err(ObjectChecksumRejection::Mismatch {
                algorithm: algorithm.to_string(),
            });
        }
    }
    Ok(())
}

/// Why the object checksum a COMPLETION declared cannot be accepted — see
/// [`verify_declared_object_checksum`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectChecksumRejection {
    /// A declared value that is not the digest the assembled object carries.
    Mismatch {
        /// The `ChecksumAlgorithm` value whose member carried it.
        algorithm: String,
    },
    /// More than one `x-amz-checksum-*` header on one completion, which asks for something
    /// no stored object can report — the condition
    /// [`IntegrityRejection::MultipleChecksumTypes`] states for a one-shot write, worded
    /// from the same place.
    MultipleChecksumTypes,
}

impl ObjectChecksumRejection {
    /// The wire error code S3 answers this rejection with: the codes
    /// [`IntegrityRejection`] answers the same two conditions with on a one-shot write.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Mismatch { .. } => BAD_DIGEST_CODE,
            Self::MultipleChecksumTypes => INVALID_REQUEST_CODE,
        }
    }

    /// The HTTP status both are rendered with: 400, from that same published list.
    pub fn status(&self) -> u16 {
        BAD_DIGEST_STATUS
    }
}

impl std::fmt::Display for ObjectChecksumRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The algorithm as the HEADER the value arrived on spells it — lowercase —
            // which is how a COMPLETION names it; a one-shot write's own mismatch names the
            // `ChecksumAlgorithm` enum value instead
            // ([`IntegrityRejection::ChecksumMismatch`]), and the two sentences are
            // otherwise the same words.
            Self::Mismatch { algorithm } => write!(
                f,
                "The {} you specified did not match the calculated checksum.",
                algorithm.to_ascii_lowercase()
            ),
            Self::MultipleChecksumTypes => write!(f, "{MULTIPLE_CHECKSUM_TYPES_MESSAGE}"),
        }
    }
}

/// The one `EncodingType` the model declares: `url`
/// (`smithy_query(query="shape",shape="EncodingType")` reports exactly one value), which
/// is also the one the AWS CLI sends on every listing.
pub const ENCODING_TYPE_URL: &str = "url";

/// The query parameter every listing binds its `EncodingType` from: `encoding-type`
/// (`smithy_query(query="operation",operation="ListObjects,ListObjectsV2,
/// ListObjectVersions,ListMultipartUploads")` reports `EncodingType:
/// {httpQuery: "encoding-type"}` on each of them) — the field name S3's refusal of a value
/// it cannot encode with reports, because that is how the API publishes it.
pub const ENCODING_TYPE_PARAMETER: &str = "encoding-type";

/// The sentence S3 refuses an `encoding-type` outside [`ENCODING_TYPE_URL`] with, stated
/// once because every listing that binds the parameter answers the same one — and because a
/// request the BOUNDARY refuses for it is worded from here too
/// (`crate::service::published_refusal`), so the two cannot disagree.
pub const INVALID_ENCODING_METHOD_MESSAGE: &str = "Invalid Encoding Method specified in Request";

/// The characters percent-encoding LEAVES ALONE: RFC 3986's unreserved set,
/// `A-Z a-z 0-9 - _ . ~`.
///
/// Everything else in a key name is encoded as `%XX` per UTF-8 code value, which is what
/// `encoding-type=url` asks for: "non-ASCII characters that are used in an object's key
/// name will be percent-encoded according to UTF-8 code values. For example, the object
/// `test_file(3).png` will appear as `test_file%283%29.png`" (the `EncodingType` enum's
/// own documentation) — so even an ASCII character outside that set (`(`, `)`, and the
/// `/` of a key prefix) is encoded, which is why the set is subtractive rather than a
/// hand-written list of characters to escape.
pub(super) const KEY_UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The characters a LISTING leaves literal in a key-name element it writes under
/// `encoding-type=url`: [`KEY_UNRESERVED`]'s set plus the `/` that separates the elements of
/// a key prefix, which S3 reports as itself so a prefix reads as the prefix it is. The space
/// is not here — it is written `+` — see [`listing_encoded_key`].
pub(super) const LISTING_KEY_LITERAL: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// `value` as a URI PATH carries it: percent-encoded per UTF-8 code value, leaving RFC
/// 3986's unreserved set alone.
///
/// What a URI reads a key out of — the path of the `Location` a completed multipart upload
/// reports, and the `rule-id` of an `x-amz-expiration` header ("the value of the rule-id is
/// URL-encoded", `GetObjectOutput$Expiration`). A LISTING writes its key-name elements in the
/// other form S3 uses ([`listing_encoded_key`]), which is not this one.
pub fn url_encoded_key(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, KEY_UNRESERVED).to_string()
}

/// `value` as a listing reports it when the request asked for `encoding-type=url`.
///
/// The elements this applies to are the ones `ListObjectsV2Output$EncodingType` names —
/// "returns encoded key name values in the following response elements: Delimiter,
/// Prefix, Key, and StartAfter" — plus the same key-name elements of the other listings
/// (`ListObjectVersions`' markers and its `CommonPrefixes`). A listing that asked for no
/// encoding type reports every one of them verbatim.
///
/// The form is the one S3 answers with, which is a FORM encoding rather than a path one:
/// a character outside the unreserved set becomes `%XX` per UTF-8 code value — "the object
/// `test_file(3).png` will appear as `test_file%283%29.png`" (the `EncodingType` enum's own
/// documentation) — a SPACE becomes `+`, and the `/` that separates the elements of a key
/// prefix is left as it is. The two exceptions are what makes the element decodable back to
/// the stored key by the plus-aware unquoting every SDK applies to it (botocore's
/// `decode_list_object_v2` passes each key-name element through `unquote_plus`), and they
/// are what a recorded listing of keys carrying a space and a `/` shows.
pub fn listing_encoded_key(value: &str) -> String {
    value
        .split(' ')
        .map(|run| {
            ::percent_encoding::utf8_percent_encode(run, LISTING_KEY_LITERAL).to_string()
        })
        .collect::<Vec<String>>()
        .join("+")
}

/// The canonical user id of the one account this emulator serves — what an S3 response
/// reports as the `Owner$ID` of the account's buckets and objects.
///
/// S3 names an owner by its CANONICAL USER ID, not by its account id: "a 64-digit
/// hexadecimal encoded, 256-bit value" (AWS General Reference, *AWS account identifiers*),
/// so [`::engine::emulator_account_id`] itself is the wrong shape for the member. This
/// derives one from that account id by SHA-256, which makes it deterministic (every read
/// reports the same owner, whatever the clock or the request does), of the documented
/// shape, and a disclosure of nothing: it is a hash of the canonical placeholder account
/// `000000000000`.
///
/// Service-wide rather than per-operation, because every reader that reports an owner —
/// `ListBuckets`, and the ACL and object-listing operations when they are implemented —
/// owes the SAME id for this account.
pub fn canonical_user_id() -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(::engine::emulator_account_id().as_bytes());
    // 32 digest bytes render as the 64 lowercase hex digits a canonical user id carries.
    ::hex::encode(digest.finalize())
}

/// WHO initiated one multipart upload — the principal every reader of the upload reports
/// as its `Initiator`.
///
/// "Container element that identifies who initiated the multipart upload" (`Initiator`),
/// whose `ID` is read off the PRINCIPAL: "if the principal is an Amazon Web Services
/// account, it provides the Canonical User ID. If the principal is an IAM User, it provides
/// a user ARN value" (`Initiator$ID`), with the principal's friendly name beside it
/// (`Initiator$DisplayName`). So this is recorded by the initiation, from the credential
/// that signed it, rather than derived by a reader: the two readers of an upload
/// (`ListMultipartUploads` and `ListParts`) report the principal that really started it,
/// and the answer does not move when another principal reads the upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInitiator {
    /// What `Initiator$ID` carries for this principal: its user ARN when it is an IAM
    /// identity of the account, and the account's canonical user id when the principal is
    /// the account itself.
    pub id: String,
    /// The principal's friendly name, as `Initiator$DisplayName` carries it — `None` for a
    /// principal that has none (the account itself), where S3 has no name to report.
    pub display_name: Option<String>,
}

impl UploadInitiator {
    /// The initiator an upload initiated by the principal `principal_arn` speaks for,
    /// which is the ARN `sts:GetCallerIdentity` reports for the credential that signed the
    /// initiation ([`crate::service::request_authorization::request_caller`] resolves it).
    ///
    /// An IAM identity is reported BY that ARN, which is what `Initiator$ID` says; the
    /// account itself — its root, or a credential that names no identity at all — is
    /// reported by the account's canonical user id ([`canonical_user_id`]), the same id
    /// every `Owner` of this account is reported by.
    pub fn of_principal(principal_arn: &str) -> Self {
        match principal_display_name(principal_arn) {
            Some(display_name) => Self {
                id: principal_arn.to_string(),
                display_name: Some(display_name),
            },
            None => Self {
                id: canonical_user_id(),
                display_name: None,
            },
        }
    }
}

/// The friendly name of the principal `principal_arn` names, or `None` when it names no
/// identity that has one.
///
/// An IAM identity's ARN ends in its resource field, `<entity type>/<path…>/<name>` — a
/// user's `user/<path>/<name>`, an assumed-role session's
/// `assumed-role/<role name>/<session name>` — and the LAST segment of that path is the
/// name the identity is known by, which is what a response reports as its display name.
/// Everything else has no such name: the account's own root (`…:root`, a resource field with
/// no `/` at all), a principal that is not an ARN, and the empty ARN of a credential that
/// speaks for nobody.
///
/// The ARN itself is read through the one parse this tree reads an ARN with
/// ([`::service_common::arn::resource`]), so this does not split the value its own way.
fn principal_display_name(principal_arn: &str) -> Option<String> {
    let resource = ::service_common::arn::resource(principal_arn)?;
    let name = resource
        .split('/')
        .next_back()
        .filter(|name| !name.is_empty())?;
    (name != resource).then(|| name.to_string())
}

/// The version id a write into a versioning-enabled bucket is stored under.
///
/// S3's version ids are OPAQUE: `ObjectVersionId` is a plain string in the model
/// (`smithy_query(query="shape",shape="ObjectVersionId")` reports no pattern and no
/// length), and the only thing documented about them is that each is unique ("All objects
/// added to the bucket receive a unique version ID" — `PutBucketVersioning`). So this is
/// DERIVED rather than drawn from a generator: the SHA-256 of the bucket, the key and the
/// store's monotonic mint sequence, rendered as the [`VERSION_ID_CHARS`] characters an S3
/// version id is shaped like. Two versions of a key — and two versions anywhere in the
/// store — therefore never collide, and the value stays what it was once stored, because
/// nothing re-derives it.
///
/// Hex, so every character is an ASCII alphanumeric: the id survives a round trip through
/// the `versionId` query parameter a client sends it back in, where `+` would arrive
/// decoded as a space and `/` would end the path segment, and it carries only characters
/// [`version_id_is_well_formed`] accepts.
pub fn mint_version_id(bucket: &str, key: &str, sequence: u64) -> String {
    use ::sha2::Digest;
    let mut digest = ::sha2::Sha256::new();
    digest.update(bucket.as_bytes());
    // A separator, so ("a", "bc") and ("ab", "c") cannot hash the same bytes.
    digest.update([0u8]);
    digest.update(key.as_bytes());
    digest.update([0u8]);
    digest.update(sequence.to_be_bytes());
    // Each byte renders as two hex digits, so half of `VERSION_ID_CHARS` digest bytes
    // fill the id exactly.
    ::hex::encode(&digest.finalize()[..VERSION_ID_CHARS / 2])
}
