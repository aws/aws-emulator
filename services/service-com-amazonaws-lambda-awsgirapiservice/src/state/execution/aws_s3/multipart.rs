// SPDX-License-Identifier: CC0-1.0

//! The MULTIPART half of the Amazon S3 seam a running handler calls S3 through: one object
//! stored as several parts, and the upload those parts belong to.
//!
//! An object a handler sends in pieces is a multipart upload: it is STARTED, each piece is
//! sent as a numbered part (or copied from another object), and the upload is either COMPLETED
//! into the object the parts concatenate to or ABORTED, which is what the managed transfer
//! library a managed runtime ships does with anything over its own threshold
//! ([`super::super::aws_sdk_guest`]). Every one of them is bound exactly as the single-request
//! calls beside it are ([`super`]): the request is built, signed and parsed by the S3 client
//! this crate holds, over the in-process callback, so the answer is the emulated S3 service's
//! own — that operation's output members for a success, and S3's own code, message and status
//! for a refusal, including the refusal of a part below the size S3 documents.
//!
//! The input of each is read out of the document the handler's client method was given, with
//! the nested container a completion carries (`MultipartUpload`, whose `Parts` name the parts
//! and the tags they were stored under) walked member by member as the model lays it out. A
//! member this binding does not carry stops the call rather than travelling dropped, which is
//! the rule the whole seam holds to.

use ::serde_json::{Map, Value};

use ::aws_sdk_s3::types::{
    CompletedMultipartUpload, CompletedPart, ObjectCannedAcl, ServerSideEncryption, StorageClass,
};
use ::aws_sdk_s3::Client;

use super::super::aws_api::{Answer, Refusal};
use super::{insert_number, insert_text, insert_timestamp, refused, Input, Observed, BODY_MEMBER};

/// `CreateMultipartUpload`: the upload the parts of one object are sent under.
///
/// Its answer carries the `UploadId` every later call of the upload is addressed by, which is
/// what a transfer holds on to until it completes or aborts.
pub(super) async fn create_multipart_upload(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "ContentType",
        "ContentEncoding",
        "ContentDisposition",
        "ContentLanguage",
        "CacheControl",
        "Metadata",
        "StorageClass",
        "ServerSideEncryption",
        "SSEKMSKeyId",
        "Tagging",
        "ACL",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .create_multipart_upload()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .set_content_type(input.text("ContentType")?)
        .set_content_encoding(input.text("ContentEncoding")?)
        .set_content_disposition(input.text("ContentDisposition")?)
        .set_content_language(input.text("ContentLanguage")?)
        .set_cache_control(input.text("CacheControl")?)
        .set_metadata(input.string_map("Metadata")?)
        .set_storage_class(
            input
                .text("StorageClass")?
                .map(|value| StorageClass::from(value.as_str())),
        )
        .set_server_side_encryption(
            input
                .text("ServerSideEncryption")?
                .map(|value| ServerSideEncryption::from(value.as_str())),
        )
        .set_ssekms_key_id(input.text("SSEKMSKeyId")?)
        .set_tagging(input.text("Tagging")?)
        .set_acl(
            input
                .text("ACL")?
                .map(|value| ObjectCannedAcl::from(value.as_str())),
        )
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let started = match sent {
        Ok(started) => started,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Bucket", started.bucket());
    insert_text(&mut document, "Key", started.key());
    insert_text(&mut document, "UploadId", started.upload_id());
    super::insert_encryption(
        &mut document,
        started
            .server_side_encryption()
            .map(ServerSideEncryption::as_str),
        started.ssekms_key_id(),
        started.bucket_key_enabled(),
    );
    observed.answer(document, Vec::new())
}

/// `UploadPart`: one numbered piece of the object being assembled, carrying its bytes.
///
/// Its answer carries the `ETag` the part was stored under, which the completion below names
/// the part by.
pub(super) async fn upload_part(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "UploadId",
        "PartNumber",
        BODY_MEMBER,
        "ExpectedBucketOwner",
    ])?;
    let payload = input.blob(BODY_MEMBER)?.unwrap_or_default();
    let sent = client
        .upload_part()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .upload_id(input.required_text("UploadId")?)
        .set_part_number(input.number("PartNumber")?)
        .body(::aws_sdk_s3::primitives::ByteStream::from(payload))
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let stored = match sent {
        Ok(stored) => stored,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "ETag", stored.e_tag());
    super::insert_encryption(
        &mut document,
        stored
            .server_side_encryption()
            .map(ServerSideEncryption::as_str),
        stored.ssekms_key_id(),
        stored.bucket_key_enabled(),
    );
    observed.answer(document, Vec::new())
}

/// `UploadPartCopy`: one part taken from ANOTHER object rather than from the handler's bytes,
/// which is how a large object is copied.
pub(super) async fn upload_part_copy(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "UploadId",
        "PartNumber",
        "CopySource",
        "CopySourceRange",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .upload_part_copy()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .upload_id(input.required_text("UploadId")?)
        .set_part_number(input.number("PartNumber")?)
        .copy_source(input.required_text("CopySource")?)
        .set_copy_source_range(input.text("CopySourceRange")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let copied = match sent {
        Ok(copied) => copied,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    if let Some(result) = copied.copy_part_result() {
        let mut nested = Map::new();
        insert_text(&mut nested, "ETag", result.e_tag());
        insert_timestamp(&mut nested, "LastModified", result.last_modified());
        document.insert("CopyPartResult".to_string(), Value::Object(nested));
    }
    insert_text(
        &mut document,
        "CopySourceVersionId",
        copied.copy_source_version_id(),
    );
    observed.answer(document, Vec::new())
}

/// `CompleteMultipartUpload`: the object the parts concatenate to.
///
/// The parts are named by the `MultipartUpload` container the handler built — each part's
/// number and the tag it was stored under — because that list is what decides WHICH parts the
/// object is made of and in what order.
pub(super) async fn complete_multipart_upload(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "UploadId",
        "MultipartUpload",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .complete_multipart_upload()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .upload_id(input.required_text("UploadId")?)
        .set_multipart_upload(read_completed_upload(input)?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let completed = match sent {
        Ok(completed) => completed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Location", completed.location());
    insert_text(&mut document, "Bucket", completed.bucket());
    insert_text(&mut document, "Key", completed.key());
    insert_text(&mut document, "ETag", completed.e_tag());
    insert_text(&mut document, "VersionId", completed.version_id());
    insert_text(&mut document, "Expiration", completed.expiration());
    super::insert_encryption(
        &mut document,
        completed
            .server_side_encryption()
            .map(ServerSideEncryption::as_str),
        completed.ssekms_key_id(),
        completed.bucket_key_enabled(),
    );
    observed.answer(document, Vec::new())
}

/// `AbortMultipartUpload`: the upload given up on, and the parts it held released.
pub(super) async fn abort_multipart_upload(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Key", "UploadId", "ExpectedBucketOwner"])?;
    let sent = client
        .abort_multipart_upload()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .upload_id(input.required_text("UploadId")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// The `MultipartUpload` container a completion carries, read member by member: the parts the
/// object is assembled from, each by its number and the tag it was stored under.
fn read_completed_upload(input: &Input) -> Result<Option<CompletedMultipartUpload>, Refusal> {
    let Some(container) = input.structure("MultipartUpload")? else {
        return Ok(None);
    };
    container.only(&["Parts"])?;
    let mut parts = Vec::new();
    for part in container.structures("Parts")? {
        part.only(&["PartNumber", "ETag"])?;
        parts.push(
            CompletedPart::builder()
                .set_part_number(part.number("PartNumber")?)
                .set_e_tag(part.text("ETag")?)
                .build(),
        );
    }
    Ok(Some(
        CompletedMultipartUpload::builder()
            .set_parts(if parts.is_empty() { None } else { Some(parts) })
            .build(),
    ))
}

/// `ListParts`: the parts one upload has received so far, which is how a handler that
/// resumes or audits an upload learns what it already holds.
///
/// "Lists the parts that have been uploaded for a specific multipart upload" (`ListParts`):
/// each part by its number, the tag it was stored under, its size and the instant it was
/// uploaded.
pub(super) async fn list_parts(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "UploadId",
        "MaxParts",
        "PartNumberMarker",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .list_parts()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .upload_id(input.required_text("UploadId")?)
        .set_max_parts(input.number("MaxParts")?)
        .set_part_number_marker(input.text("PartNumberMarker")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Bucket", listed.bucket());
    insert_text(&mut document, "Key", listed.key());
    insert_text(&mut document, "UploadId", listed.upload_id());
    insert_text(
        &mut document,
        "PartNumberMarker",
        listed.part_number_marker(),
    );
    insert_text(
        &mut document,
        "NextPartNumberMarker",
        listed.next_part_number_marker(),
    );
    insert_number(&mut document, "MaxParts", listed.max_parts().map(i64::from));
    if let Some(truncated) = listed.is_truncated() {
        document.insert("IsTruncated".to_string(), Value::Bool(truncated));
    }
    insert_text(
        &mut document,
        "StorageClass",
        listed.storage_class().map(StorageClass::as_str),
    );
    let parts: Vec<Value> = listed
        .parts()
        .iter()
        .map(|part| {
            let mut held = Map::new();
            insert_number(&mut held, "PartNumber", part.part_number().map(i64::from));
            insert_text(&mut held, "ETag", part.e_tag());
            insert_number(&mut held, "Size", part.size());
            insert_timestamp(&mut held, "LastModified", part.last_modified());
            Value::Object(held)
        })
        .collect();
    if !parts.is_empty() {
        document.insert("Parts".to_string(), Value::Array(parts));
    }
    observed.answer(document, Vec::new())
}

/// `ListMultipartUploads`: the uploads of a bucket that are still in progress, which is what a
/// handler cleaning up after an interrupted transfer reads.
pub(super) async fn list_multipart_uploads(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Prefix",
        "Delimiter",
        "MaxUploads",
        "KeyMarker",
        "UploadIdMarker",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .list_multipart_uploads()
        .bucket(input.required_text("Bucket")?)
        .set_prefix(input.text("Prefix")?)
        .set_delimiter(input.text("Delimiter")?)
        .set_max_uploads(input.number("MaxUploads")?)
        .set_key_marker(input.text("KeyMarker")?)
        .set_upload_id_marker(input.text("UploadIdMarker")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Bucket", listed.bucket());
    insert_text(&mut document, "Prefix", listed.prefix());
    insert_text(&mut document, "Delimiter", listed.delimiter());
    insert_text(&mut document, "KeyMarker", listed.key_marker());
    insert_text(&mut document, "UploadIdMarker", listed.upload_id_marker());
    insert_text(
        &mut document,
        "NextKeyMarker",
        listed.next_key_marker(),
    );
    insert_text(
        &mut document,
        "NextUploadIdMarker",
        listed.next_upload_id_marker(),
    );
    insert_number(
        &mut document,
        "MaxUploads",
        listed.max_uploads().map(i64::from),
    );
    if let Some(truncated) = listed.is_truncated() {
        document.insert("IsTruncated".to_string(), Value::Bool(truncated));
    }
    let uploads: Vec<Value> = listed
        .uploads()
        .iter()
        .map(|upload| {
            let mut held = Map::new();
            insert_text(&mut held, "UploadId", upload.upload_id());
            insert_text(&mut held, "Key", upload.key());
            insert_timestamp(&mut held, "Initiated", upload.initiated());
            insert_text(
                &mut held,
                "StorageClass",
                upload.storage_class().map(StorageClass::as_str),
            );
            Value::Object(held)
        })
        .collect();
    if !uploads.is_empty() {
        document.insert("Uploads".to_string(), Value::Array(uploads));
    }
    observed.answer(document, Vec::new())
}
