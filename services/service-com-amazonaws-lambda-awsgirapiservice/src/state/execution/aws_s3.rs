// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 half of the SDK seam a RUNNING handler calls another service through:
//! one call on a bucket or on its objects, built the way S3's own protocol binds it.
//!
//! The calls on ONE object live here; the calls on a bucket's own configuration and on its
//! objects as a SET live beside them in [`bucket`], because those are the operations a
//! handler ADMINISTERS a bucket with rather than stores through.
//!
//! The other services a handler reaches bind their requests with an AWS JSON protocol,
//! where the request body IS the operation's input document, so one code path serves every
//! operation of every one of them ([`super::aws_api`]). S3 does not: its model is `restXml`,
//! so each operation binds its input to a METHOD, a PATH, a QUERY and HEADERS of its own,
//! and answers either an XML document or the object's own bytes. That binding is this
//! module, and it is not written out here: the request is built, signed, dispatched and
//! parsed by the S3 CLIENT this crate holds (`aws_sdk_s3`), over the same in-process
//! callback transport every other call of this seam uses, so the wire details are the
//! client's and the answer is the emulated S3 service's own.
//!
//! What a guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator: the client is given the callback's own
//!   transport and a path-style endpoint, so no host name is resolved and no socket is
//!   opened.
//! * The call is signed as the identity the invocation was given — the session of the
//!   function's execution role ([`super::execution_role_session`]) — so S3 judges it
//!   against the policies in force on that role and the bucket's own policy, exactly as it
//!   judges any other signed request.
//! * Only the operations in [`BOUND_OPERATIONS`] are bound, and only the input members each
//!   entry names. An operation or a member this seam does not bind is reported as a call
//!   that was NOT MADE, naming it, rather than sent with a value dropped — a dropped member
//!   would make a call that did something other than what the handler asked for.
//! * The answer's own status, headers and document are what S3 answered: a success is that
//!   operation's output members, and a refusal is S3's own error code, message and status,
//!   which the SDK the guest calls raises as `ClientError` ([`super::aws_sdk_guest`]).
//!
//! An object's BYTES travel over the seam as the base64 text an AWS JSON protocol carries a
//! blob as, in both directions — `Body` in, `Body` out — and the guest SDK hands the answer's
//! bytes over as the stream its own client answers with ([`super::aws_api::Answer::streams`]).

use ::std::collections::HashMap;
use ::std::sync::{Arc, Mutex, PoisonError};

use ::aws_sdk_s3::error::ProvideErrorMetadata;
use ::aws_sdk_s3::Client;
use ::serde_json::{Map, Value};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::aws_api::{Answer, CallerIdentity, Refusal, MAX_RESPONSE_BYTES};
use super::sdk_call::insert_flag;

/// The bucket-wide half of this seam: a bucket's own configuration subresources, and the
/// operations that act on its objects as a set.
mod bucket;

/// The provider name the signing identity carries: it names this runtime rather than any
/// AWS credential source, and never travels on the wire.
const PROVIDER_NAME: &str = "aws-lambda-execution-role";

/// The member an object's own bytes travel in, in both directions (`GetObject$Body`,
/// `PutObject$Body`).
const BODY_MEMBER: &str = "Body";

/// The operations of Amazon S3 this seam binds, in the order a refusal lists them.
///
/// One entry per operation [`call`] builds a request for; a handler naming any other
/// operation of the service is told the call was not made and why, because a request this
/// module cannot bind is one it would otherwise be guessing at.
pub const BOUND_OPERATIONS: &[&str] = &[
    "GetObject",
    "HeadObject",
    "PutObject",
    "CopyObject",
    "DeleteObject",
    "ListObjectsV2",
    // The bucket-wide half ([`bucket`]): what a handler administering a bucket calls —
    // reading and replacing the bucket's own notification and tagging subresources, listing
    // the versions it holds, and removing a set of them in one call.
    "GetBucketNotificationConfiguration",
    "PutBucketNotificationConfiguration",
    "GetBucketTagging",
    "PutBucketTagging",
    "ListObjectVersions",
    "DeleteObjects",
];

/// Make ONE Amazon S3 call: `operation`, with `body` as the operation's input document (the
/// keyword arguments the handler passed its client method).
pub async fn call(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    region: &str,
    operation: &str,
    body: &str,
) -> Result<Answer, Refusal> {
    let input = Input::read(body)?;
    let observed = Observed::default();
    let client = client(
        callback,
        identity,
        region,
        observed.clone(),
        input.text("Bucket")?.as_deref(),
    );
    match operation {
        "GetObject" => get_object(&client, &input, &observed).await,
        "HeadObject" => head_object(&client, &input, &observed).await,
        "PutObject" => put_object(&client, &input, &observed).await,
        "CopyObject" => copy_object(&client, &input, &observed).await,
        "DeleteObject" => delete_object(&client, &input, &observed).await,
        "ListObjectsV2" => list_objects_v2(&client, &input, &observed).await,
        "GetBucketNotificationConfiguration" => {
            bucket::get_bucket_notification_configuration(&client, &input, &observed).await
        }
        "PutBucketNotificationConfiguration" => {
            bucket::put_bucket_notification_configuration(&client, &input, &observed).await
        }
        "GetBucketTagging" => bucket::get_bucket_tagging(&client, &input, &observed).await,
        "PutBucketTagging" => bucket::put_bucket_tagging(&client, &input, &observed).await,
        "ListObjectVersions" => bucket::list_object_versions(&client, &input, &observed).await,
        "DeleteObjects" => bucket::delete_objects(&client, &input, &observed).await,
        other => Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies binds no {other} call on Amazon S3; the \
             operations it binds are {}",
            BOUND_OPERATIONS.join(", ")
        ))),
    }
}

/// An Amazon S3 client over the in-process callback into this same emulator, signing as the
/// identity this invocation was given.
///
/// Path-style addressing is what the emulated S3 service routes on (its operations are
/// bound under a `{Bucket}` path label) — except for an ACCESS POINT, which is not addressed
/// that way at all: AWS's endpoint rules resolve an access point ARN in the `Bucket` field to
/// the access point's own endpoint host (`<name>-<account-id>.s3-accesspoint.<region>…`) and
/// refuse it path-style, so a call whose bucket is one is left to the client's own
/// addressing and arrives at that host, which the emulated service resolves. `bucket` is
/// what the handler put in that field, for exactly that decision.
///
/// The transport is in-process, so everything a
/// network client needs and this one cannot have — retries, timeouts, identity caching and
/// stalled-stream protection, each of which needs a sleep implementation the callback
/// transport is not given — is switched off.
fn client(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    region: &str,
    observed: Observed,
    bucket: Option<&str>,
) -> Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(!bucket.is_some_and(addresses_an_access_point))
        .http_client(callback.http_client())
        .interceptor(observed)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        );
    let config = match identity {
        Some(identity) => config.credentials_provider(::aws_sdk_s3::config::Credentials::new(
            identity.access_key_id.clone(),
            identity.secret_access_key.clone(),
            Some(identity.session_token.clone()),
            None,
            PROVIDER_NAME,
        )),
        None => config.credentials_provider(callback.aws_credentials_provider()),
    };
    Client::from_conf(config.build())
}

/// The resource type an S3 ACCESS POINT's ARN names
/// (`arn:<partition>:s3:<region>:<account-id>:accesspoint/<name>`).
const ACCESS_POINT_RESOURCE_TYPE: &str = "accesspoint";

/// Whether what the handler put in the `Bucket` field is an ACCESS POINT's ARN — "you can
/// use the access point ARN in place of the bucket name" — read through the workspace's one
/// ARN parse rather than by matching on the text.
fn addresses_an_access_point(bucket: &str) -> bool {
    ::service_common::arn::parse(bucket)
        .is_some_and(|arn| arn.resource_type() == ACCESS_POINT_RESOURCE_TYPE)
}

/// The URL a presigned object request carries its own signature in.
///
/// The URL is built AND signed by the S3 client this module holds, with the same identity
/// and the same endpoint a call made here uses, so what comes back addresses this emulator's
/// own object endpoint and carries a SigV4 query signature over exactly the request it
/// names, good for `expires_in`. Nothing is sent: presigning is a signature over a request
/// that has not been made.
///
/// Only the two operations a presigned URL is handed out for are signed — reading an object
/// and writing one — and any other is reported as a URL that was not signed, naming it.
pub async fn presigned(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    region: &str,
    operation: &str,
    body: &str,
    expires_in: ::std::time::Duration,
) -> Result<String, Refusal> {
    let input = Input::read(body)?;
    let client = client(
        callback,
        identity,
        region,
        Observed::default(),
        input.text("Bucket")?.as_deref(),
    );
    let config = ::aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)
        .map_err(|error| Refusal::NotMade(error.to_string()))?;
    let signed = match operation {
        "GetObject" => {
            input.only(&["Bucket", "Key", "VersionId", "Range"])?;
            client
                .get_object()
                .bucket(input.required_text("Bucket")?)
                .key(input.required_text("Key")?)
                .set_version_id(input.text("VersionId")?)
                .set_range(input.text("Range")?)
                .presigned(config)
                .await
                .map(|presigned| presigned.uri().to_string())
                .map_err(|error| error.to_string())
        }
        "PutObject" => {
            input.only(&["Bucket", "Key", "ContentType"])?;
            client
                .put_object()
                .bucket(input.required_text("Bucket")?)
                .key(input.required_text("Key")?)
                .set_content_type(input.text("ContentType")?)
                .presigned(config)
                .await
                .map(|presigned| presigned.uri().to_string())
                .map_err(|error| error.to_string())
        }
        other => {
            return Err(Refusal::NotMade(format!(
                "the AWS SDK this runtime supplies presigns a GetObject or a PutObject URL \
                 on Amazon S3; {other} is not one it presigns"
            )))
        }
    };
    signed.map_err(Refusal::NotMade)
}

/// `GetObject`: the bytes and the metadata of the version the request addresses.
async fn get_object(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Key", "VersionId", "Range", "IfMatch", "IfNoneMatch"])?;
    let sent = client
        .get_object()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .set_version_id(input.text("VersionId")?)
        .set_range(input.text("Range")?)
        .set_if_match(input.text("IfMatch")?)
        .set_if_none_match(input.text("IfNoneMatch")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "ContentType", read.content_type());
    insert_text(&mut document, "ContentRange", read.content_range());
    insert_text(&mut document, "ContentEncoding", read.content_encoding());
    insert_text(&mut document, "ContentDisposition", read.content_disposition());
    insert_text(&mut document, "CacheControl", read.cache_control());
    insert_text(&mut document, "AcceptRanges", read.accept_ranges());
    insert_text(&mut document, "ETag", read.e_tag());
    insert_text(&mut document, "VersionId", read.version_id());
    insert_text(&mut document, "Expiration", read.expiration());
    insert_number(&mut document, "ContentLength", read.content_length());
    insert_timestamp(&mut document, "LastModified", read.last_modified());
    insert_encryption(
        &mut document,
        read.server_side_encryption()
            .map(::aws_sdk_s3::types::ServerSideEncryption::as_str),
        read.ssekms_key_id(),
        read.bucket_key_enabled(),
    );
    insert_text(
        &mut document,
        "StorageClass",
        read.storage_class()
            .map(::aws_sdk_s3::types::StorageClass::as_str),
    );
    insert_metadata(&mut document, read.metadata());
    // What the answer says it carries, before any of it is held: an object larger than one
    // call may read back is refused on the length S3 reported rather than after the bytes
    // have been materialised.
    if read.content_length().is_some_and(|length| {
        length > i64::try_from(MAX_RESPONSE_BYTES).unwrap_or(i64::MAX)
    }) {
        return Err(Refusal::ResponseTooLarge);
    }
    // The object's bytes, read whole out of the in-process answer and bounded by what one
    // call may read back.
    let bytes = read
        .body
        .collect()
        .await
        .map_err(|error| Refusal::NotMade(format!("the object's bytes could not be read: {error}")))?
        .into_bytes();
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Refusal::ResponseTooLarge);
    }
    document.insert(BODY_MEMBER.to_string(), Value::String(base64_text(&bytes)));
    observed.answer(document, vec![BODY_MEMBER.to_string()])
}

/// `HeadObject`: the same metadata a read reports, without the bytes.
async fn head_object(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Key", "VersionId"])?;
    let sent = client
        .head_object()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .set_version_id(input.text("VersionId")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        // A `HEAD` answer carries NO BODY on the wire, so an SDK has no code or sentence to
        // read out of one and reports the STATUS as the code with that status's own reason
        // as the message. That is what a handler catching a missing object matches on, so
        // it is what this refusal carries.
        Err(error) => return refused_without_a_document(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "ContentType", read.content_type());
    insert_text(&mut document, "ETag", read.e_tag());
    insert_text(&mut document, "VersionId", read.version_id());
    insert_text(&mut document, "AcceptRanges", read.accept_ranges());
    insert_number(&mut document, "ContentLength", read.content_length());
    insert_timestamp(&mut document, "LastModified", read.last_modified());
    insert_encryption(
        &mut document,
        read.server_side_encryption()
            .map(::aws_sdk_s3::types::ServerSideEncryption::as_str),
        read.ssekms_key_id(),
        read.bucket_key_enabled(),
    );
    insert_text(
        &mut document,
        "StorageClass",
        read.storage_class()
            .map(::aws_sdk_s3::types::StorageClass::as_str),
    );
    insert_metadata(&mut document, read.metadata());
    observed.answer(document, Vec::new())
}

/// `PutObject`: the write, carrying the handler's own bytes as the object's payload.
async fn put_object(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        BODY_MEMBER,
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
    ])?;
    let payload = input.blob(BODY_MEMBER)?.unwrap_or_default();
    let sent = client
        .put_object()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .body(::aws_sdk_s3::primitives::ByteStream::from(payload))
        .set_content_type(input.text("ContentType")?)
        .set_content_encoding(input.text("ContentEncoding")?)
        .set_content_disposition(input.text("ContentDisposition")?)
        .set_content_language(input.text("ContentLanguage")?)
        .set_cache_control(input.text("CacheControl")?)
        .set_metadata(input.string_map("Metadata")?)
        .set_storage_class(
            input
                .text("StorageClass")?
                .map(|value| ::aws_sdk_s3::types::StorageClass::from(value.as_str())),
        )
        .set_server_side_encryption(input.text("ServerSideEncryption")?.map(|value| {
            ::aws_sdk_s3::types::ServerSideEncryption::from(value.as_str())
        }))
        .set_ssekms_key_id(input.text("SSEKMSKeyId")?)
        .set_tagging(input.text("Tagging")?)
        .set_acl(
            input
                .text("ACL")?
                .map(|value| ::aws_sdk_s3::types::ObjectCannedAcl::from(value.as_str())),
        )
        .send()
        .await;
    let written = match sent {
        Ok(written) => written,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "ETag", written.e_tag());
    insert_text(&mut document, "VersionId", written.version_id());
    insert_text(&mut document, "Expiration", written.expiration());
    insert_encryption(
        &mut document,
        written
            .server_side_encryption()
            .map(::aws_sdk_s3::types::ServerSideEncryption::as_str),
        written.ssekms_key_id(),
        written.bucket_key_enabled(),
    );
    observed.answer(document, Vec::new())
}

/// `CopyObject`: one stored object written again under another address.
async fn copy_object(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Key",
        "CopySource",
        "MetadataDirective",
        "Metadata",
        "ContentType",
        "StorageClass",
        "ServerSideEncryption",
        "TaggingDirective",
        "Tagging",
    ])?;
    let sent = client
        .copy_object()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .copy_source(input.required_text("CopySource")?)
        .set_metadata_directive(input.text("MetadataDirective")?.map(|value| {
            ::aws_sdk_s3::types::MetadataDirective::from(value.as_str())
        }))
        .set_metadata(input.string_map("Metadata")?)
        .set_content_type(input.text("ContentType")?)
        .set_storage_class(
            input
                .text("StorageClass")?
                .map(|value| ::aws_sdk_s3::types::StorageClass::from(value.as_str())),
        )
        .set_server_side_encryption(input.text("ServerSideEncryption")?.map(|value| {
            ::aws_sdk_s3::types::ServerSideEncryption::from(value.as_str())
        }))
        .set_tagging_directive(input.text("TaggingDirective")?.map(|value| {
            ::aws_sdk_s3::types::TaggingDirective::from(value.as_str())
        }))
        .set_tagging(input.text("Tagging")?)
        .send()
        .await;
    let copied = match sent {
        Ok(copied) => copied,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    if let Some(result) = copied.copy_object_result() {
        let mut nested = Map::new();
        insert_text(&mut nested, "ETag", result.e_tag());
        insert_timestamp(&mut nested, "LastModified", result.last_modified());
        document.insert("CopyObjectResult".to_string(), Value::Object(nested));
    }
    insert_text(&mut document, "VersionId", copied.version_id());
    insert_text(
        &mut document,
        "CopySourceVersionId",
        copied.copy_source_version_id(),
    );
    insert_encryption(
        &mut document,
        copied
            .server_side_encryption()
            .map(::aws_sdk_s3::types::ServerSideEncryption::as_str),
        copied.ssekms_key_id(),
        copied.bucket_key_enabled(),
    );
    observed.answer(document, Vec::new())
}

/// `DeleteObject`: the removal, or the delete marker the removal stacked.
async fn delete_object(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Key", "VersionId"])?;
    let sent = client
        .delete_object()
        .bucket(input.required_text("Bucket")?)
        .key(input.required_text("Key")?)
        .set_version_id(input.text("VersionId")?)
        .send()
        .await;
    let deleted = match sent {
        Ok(deleted) => deleted,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    if let Some(marker) = deleted.delete_marker() {
        document.insert("DeleteMarker".to_string(), Value::Bool(marker));
    }
    insert_text(&mut document, "VersionId", deleted.version_id());
    observed.answer(document, Vec::new())
}

/// `ListObjectsV2`: one page of the bucket's current objects.
async fn list_objects_v2(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Prefix",
        "Delimiter",
        "MaxKeys",
        "ContinuationToken",
        "StartAfter",
    ])?;
    let sent = client
        .list_objects_v2()
        .bucket(input.required_text("Bucket")?)
        .set_prefix(input.text("Prefix")?)
        .set_delimiter(input.text("Delimiter")?)
        .set_max_keys(input.number("MaxKeys")?)
        .set_continuation_token(input.text("ContinuationToken")?)
        .set_start_after(input.text("StartAfter")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Name", listed.name());
    insert_text(&mut document, "Prefix", listed.prefix());
    insert_text(&mut document, "Delimiter", listed.delimiter());
    insert_text(
        &mut document,
        "ContinuationToken",
        listed.continuation_token(),
    );
    insert_text(
        &mut document,
        "NextContinuationToken",
        listed.next_continuation_token(),
    );
    insert_text(&mut document, "StartAfter", listed.start_after());
    insert_number(&mut document, "MaxKeys", listed.max_keys().map(i64::from));
    insert_number(&mut document, "KeyCount", listed.key_count().map(i64::from));
    if let Some(truncated) = listed.is_truncated() {
        document.insert("IsTruncated".to_string(), Value::Bool(truncated));
    }
    let contents: Vec<Value> = listed
        .contents()
        .iter()
        .map(|object| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", object.key());
            insert_text(&mut entry, "ETag", object.e_tag());
            insert_number(&mut entry, "Size", object.size());
            insert_timestamp(&mut entry, "LastModified", object.last_modified());
            insert_text(
                &mut entry,
                "StorageClass",
                object
                    .storage_class()
                    .map(::aws_sdk_s3::types::ObjectStorageClass::as_str),
            );
            Value::Object(entry)
        })
        .collect();
    if !contents.is_empty() {
        document.insert("Contents".to_string(), Value::Array(contents));
    }
    let prefixes: Vec<Value> = listed
        .common_prefixes()
        .iter()
        .map(|common| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Prefix", common.prefix());
            Value::Object(entry)
        })
        .collect();
    if !prefixes.is_empty() {
        document.insert("CommonPrefixes".to_string(), Value::Array(prefixes));
    }
    observed.answer(document, Vec::new())
}

/// The input document one call carries: the keyword arguments the handler passed, read as
/// the operation's own input members.
struct Input {
    members: Map<String, Value>,
}

impl Input {
    /// The document `body` carries, or a call not made when it is not an object at all.
    fn read(body: &str) -> Result<Self, Refusal> {
        let parsed: Value = ::serde_json::from_str(body)
            .map_err(|error| Refusal::NotMade(format!("the call's parameters are not a document: {error}")))?;
        match parsed {
            Value::Object(members) => Ok(Self { members }),
            _ => Err(Refusal::NotMade(
                "the call's parameters must be the operation's own input members".to_string(),
            )),
        }
    }

    /// Refuse to send a call carrying a member this binding does not carry, naming it: a
    /// member silently dropped would make a request other than the one asked for.
    fn only(&self, bound: &[&str]) -> Result<(), Refusal> {
        let mut unbound: Vec<&str> = self
            .members
            .keys()
            .map(String::as_str)
            .filter(|name| !bound.contains(name))
            .collect();
        unbound.sort_unstable();
        if unbound.is_empty() {
            return Ok(());
        }
        Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies does not carry the {} parameter(s) of this \
             Amazon S3 call; the ones it carries are {}",
            unbound.join(", "),
            bound.join(", ")
        )))
    }

    /// A string member, or `None` where the document carries none.
    fn text(&self, name: &str) -> Result<Option<String>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(other) => Err(Refusal::NotMade(format!(
                "{name} must be a string, got {other}"
            ))),
        }
    }

    /// A string member the operation requires.
    fn required_text(&self, name: &str) -> Result<String, Refusal> {
        self.text(name)?.ok_or_else(|| {
            Refusal::NotMade(format!("this Amazon S3 call needs a {name} parameter"))
        })
    }

    /// An integer member, as the model's own `@range` keeps it.
    fn number(&self, name: &str) -> Result<Option<i32>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .map(Some)
                .ok_or_else(|| {
                    Refusal::NotMade(format!("{name} must be a whole number, got {number}"))
                }),
            Some(other) => Err(Refusal::NotMade(format!(
                "{name} must be a whole number, got {other}"
            ))),
        }
    }

    /// A boolean member, as the model's own `boolean` shape carries it.
    fn boolean(&self, name: &str) -> Result<Option<bool>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(other) => Err(Refusal::NotMade(format!(
                "{name} must be true or false, got {other}"
            ))),
        }
    }

    /// A member whose shape is a STRUCTURE, read as the document its own members make up, or
    /// `None` where the document carries none.
    fn structure(&self, name: &str) -> Result<Option<Self>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(members)) => Ok(Some(Self {
                members: members.clone(),
            })),
            Some(other) => Err(Refusal::NotMade(format!(
                "{name} must be a document of its own members, got {other}"
            ))),
        }
    }

    /// The same, for a member the operation requires.
    fn required_structure(&self, name: &str) -> Result<Self, Refusal> {
        self.structure(name)?.ok_or_else(|| {
            Refusal::NotMade(format!("this Amazon S3 call needs a {name} parameter"))
        })
    }

    /// A member whose shape is a LIST OF STRUCTURES, read as one document per element; an
    /// empty list where the document carries the member as none at all, which is what a list
    /// member a caller left out holds.
    fn structures(&self, name: &str) -> Result<Vec<Self>, Refusal> {
        let elements = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(elements)) => elements,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "{name} must be a list, got {other}"
                )))
            }
        };
        elements
            .iter()
            .map(|element| match element {
                Value::Object(members) => Ok(Self {
                    members: members.clone(),
                }),
                other => Err(Refusal::NotMade(format!(
                    "every element of {name} must be a document of its own members, got {other}"
                ))),
            })
            .collect()
    }

    /// The same, for a list-of-structures member the operation REQUIRES: a document that
    /// carries none of it stops the call rather than sending the required member emptied.
    fn required_structures(&self, name: &str) -> Result<Vec<Self>, Refusal> {
        if !self
            .members
            .get(name)
            .is_some_and(|value| !value.is_null())
        {
            return Err(Refusal::NotMade(format!(
                "this Amazon S3 call needs a {name} parameter"
            )));
        }
        self.structures(name)
    }

    /// A member whose shape is a LIST OF STRINGS, read as the values it holds; an empty list
    /// where the document carries the member as none at all.
    fn text_list(&self, name: &str) -> Result<Vec<String>, Refusal> {
        let elements = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(elements)) => elements,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "{name} must be a list, got {other}"
                )))
            }
        };
        elements
            .iter()
            .map(|element| match element {
                Value::String(text) => Ok(text.clone()),
                other => Err(Refusal::NotMade(format!(
                    "every element of {name} must be a string, got {other}"
                ))),
            })
            .collect()
    }

    /// The same, for a list member the operation REQUIRES: a document that carries none of
    /// it stops the call rather than sending the required member emptied.
    fn required_text_list(&self, name: &str) -> Result<Vec<String>, Refusal> {
        if !self
            .members
            .get(name)
            .is_some_and(|value| !value.is_null())
        {
            return Err(Refusal::NotMade(format!(
                "this Amazon S3 call needs a {name} parameter"
            )));
        }
        self.text_list(name)
    }

    /// A map of strings to strings (`Metadata`), or `None` where the document carries none.
    fn string_map(&self, name: &str) -> Result<Option<HashMap<String, String>>, Refusal> {
        let entries = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(entries)) => entries,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "{name} must be a map of strings, got {other}"
                )))
            }
        };
        let mut held = HashMap::with_capacity(entries.len());
        for (key, value) in entries {
            let Value::String(text) = value else {
                return Err(Refusal::NotMade(format!(
                    "{name}[{key}] must be a string, got {value}"
                )));
            };
            held.insert(key.clone(), text.clone());
        }
        Ok(Some(held))
    }

    /// A member carrying BYTES, which travel as the base64 text an AWS JSON protocol
    /// carries a blob as — which is how the SDK the guest calls hands over the bytes a
    /// handler passed ([`super::aws_sdk_guest`]).
    fn blob(&self, name: &str) -> Result<Option<Vec<u8>>, Refusal> {
        let Some(text) = self.text(name)? else {
            return Ok(None);
        };
        ::base64::Engine::decode(&::base64::engine::general_purpose::STANDARD, text)
            .map(Some)
            .map_err(|error| {
                Refusal::NotMade(format!("{name} did not arrive as the bytes it carries: {error}"))
            })
    }
}

/// What the ANSWER itself carried, recorded as it arrived rather than assumed: the status
/// S3 answered and the id it gave the request.
///
/// An SDK client reports an operation's output members and not the HTTP answer they were
/// read out of, so the one place that answer passes through is read here and nowhere else.
#[derive(Clone, Default)]
struct Observed(Arc<Mutex<Option<Carried>>>);

/// The facts of one HTTP answer this seam reports to the guest.
#[derive(Clone)]
struct Carried {
    /// The status S3 answered.
    status: u16,
    /// The id it gave the request it answered.
    request_id: Option<String>,
}

/// The header S3 carries the id of the request it answers in.
const S3_REQUEST_ID_HEADER: &str = "x-amz-request-id";

/// The name this interceptor is registered under on the client's own configuration.
const OBSERVER_NAME: &str = "lambda-runtime-s3-answer";

impl ::std::fmt::Debug for Observed {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("Observed").finish_non_exhaustive()
    }
}

impl ::aws_smithy_runtime_api::client::interceptors::Intercept for Observed {
    fn name(&self) -> &'static str {
        OBSERVER_NAME
    }

    fn read_before_deserialization(
        &self,
        context: &::aws_smithy_runtime_api::client::interceptors::context::BeforeDeserializationInterceptorContextRef<'_>,
        _components: &::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _config: &mut ::aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), ::aws_smithy_runtime_api::box_error::BoxError> {
        let response = context.response();
        let carried = Carried {
            status: response.status().as_u16(),
            request_id: response
                .headers()
                .get(S3_REQUEST_ID_HEADER)
                .map(str::to_string),
        };
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(carried);
        Ok(())
    }
}

impl Observed {
    /// What the answer carried, or a call not made where no answer reached this seam at all.
    fn carried(&self) -> Result<Carried, Refusal> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                Refusal::NotMade("the call reached no Amazon S3 answer at all".to_string())
            })
    }

    /// One served answer: the output members `document` holds, under the status and request
    /// id S3 itself answered.
    fn answer(&self, document: Map<String, Value>, streams: Vec<String>) -> Result<Answer, Refusal> {
        let carried = self.carried()?;
        Ok(Answer {
            status: carried.status,
            body: Value::Object(document).to_string(),
            error_type: None,
            // Amazon S3 speaks `restXml` rather than a query-compatible protocol, so no
            // answer of its carries a legacy query error code.
            query_error_code: None,
            request_id: carried.request_id,
            streams,
            // Amazon S3's own protocol carries an object's bytes as the body itself rather
            // than as a member of a JSON document, so the bytes of this answer are the ones
            // `streams` names and no value of its document is base64.
            blobs: Vec::new(),
        })
    }
}

/// S3's own REFUSAL, as the answer the guest reads back: the code it named, the message it
/// worded and the status it answered, so the SDK the guest calls raises the modelled error
/// class for that code with that message.
///
/// A call that reached no answer — nothing to report a status or a code from — is reported
/// as a call that was not made, carrying why.
fn refused<E: ProvideErrorMetadata>(
    error: &::aws_sdk_s3::error::SdkError<E, ::aws_smithy_runtime_api::http::Response>,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    let Some(code) = error.code() else {
        return Err(Refusal::NotMade(format!(
            "the Amazon S3 call could not be made: {error}"
        )));
    };
    let carried = observed.carried()?;
    let mut document = Map::new();
    // `__type` and `message` are where the SDK the guest calls reads a refusal's code and
    // sentence from, so the answer carries S3's own in both.
    document.insert("__type".to_string(), Value::String(code.to_string()));
    document.insert(
        "message".to_string(),
        Value::String(error.message().unwrap_or_default().to_string()),
    );
    Ok(Answer {
        status: carried.status,
        body: Value::Object(document).to_string(),
        error_type: Some(code.to_string()),
        query_error_code: None,
        request_id: carried.request_id,
        streams: Vec::new(),
        blobs: Vec::new(),
    })
}

/// The same refusal for an operation whose answer carries NO DOCUMENT: a `HEAD`, where the
/// code an SDK reports is the status itself and the message is that status's reason phrase,
/// because there is no body to read either out of.
fn refused_without_a_document<E: ProvideErrorMetadata>(
    error: &::aws_sdk_s3::error::SdkError<E, ::aws_smithy_runtime_api::http::Response>,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    let Ok(carried) = observed.carried() else {
        return Err(Refusal::NotMade(format!(
            "the Amazon S3 call could not be made: {error}"
        )));
    };
    let code = carried.status.to_string();
    let mut document = Map::new();
    document.insert("__type".to_string(), Value::String(code.clone()));
    document.insert(
        "message".to_string(),
        Value::String(
            ::http::StatusCode::from_u16(carried.status)
                .ok()
                .and_then(|status| status.canonical_reason())
                .unwrap_or_default()
                .to_string(),
        ),
    );
    Ok(Answer {
        status: carried.status,
        body: Value::Object(document).to_string(),
        error_type: Some(code),
        query_error_code: None,
        request_id: carried.request_id,
        streams: Vec::new(),
        blobs: Vec::new(),
    })
}

/// The base64 an answer's blob member travels as.
fn base64_text(bytes: &[u8]) -> String {
    ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, bytes)
}

/// Put a string member on the answer's document, where the answer carried one.
fn insert_text(document: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::String(value.to_string()));
    }
}

/// Put a numeric member on it, the same way.
fn insert_number(document: &mut Map<String, Value>, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Number(value.into()));
    }
}

/// Put a timestamp member on it, as the epoch seconds an AWS JSON protocol carries a
/// timestamp as — which is the form this seam hands every other service's timestamps over
/// in.
fn insert_timestamp(
    document: &mut Map<String, Value>,
    name: &str,
    value: Option<&::aws_smithy_types::DateTime>,
) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Number(value.secs().into()));
    }
}

/// Put how an object is PROTECTED on the answer's document: the algorithm in force on it,
/// the key that algorithm used, and whether a bucket-scoped key was used with it.
///
/// The key's identity travels with the algorithm wherever one of them does: a handler that
/// learns only the algorithm cannot tell which customer master key protects the bytes it
/// was handed, so it can neither audit that key nor write under the same one — and the
/// service answered the key itself, so it is reported rather than dropped. A member the
/// answer did not carry is left off, so an object under no key reports none.
fn insert_encryption(
    document: &mut Map<String, Value>,
    algorithm: Option<&str>,
    key_id: Option<&str>,
    bucket_key_enabled: Option<bool>,
) {
    insert_text(document, "ServerSideEncryption", algorithm);
    insert_text(document, "SSEKMSKeyId", key_id);
    insert_flag(document, "BucketKeyEnabled", bucket_key_enabled);
}

/// Put the object's user-defined metadata on it, where the answer carried any.
fn insert_metadata(document: &mut Map<String, Value>, metadata: Option<&HashMap<String, String>>) {
    let Some(metadata) = metadata else {
        return;
    };
    let entries: Map<String, Value> = metadata
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    document.insert("Metadata".to_string(), Value::Object(entries));
}

#[cfg(test)]
mod tests {
    use super::{Input, BOUND_OPERATIONS, BODY_MEMBER};

    /// RULE: a call is sent only with the members its operation's binding carries, so a
    /// parameter the binding does not carry is reported as a call that was not made rather
    /// than dropped. The rows differ along the FORM of the document a handler passed: the
    /// bound members alone, one unbound member, several, and the empty document.
    #[test]
    fn a_parameter_the_binding_does_not_carry_stops_the_call() {
        let bound = ["Bucket", "Key", "VersionId"];
        let rows: [(&str, &str, Option<&str>); 5] = [
            ("the bound members alone", r#"{"Bucket":"b","Key":"k"}"#, None),
            ("every bound member", r#"{"Bucket":"b","Key":"k","VersionId":"v"}"#, None),
            ("the empty document", "{}", None),
            (
                "one unbound member",
                r#"{"Bucket":"b","Key":"k","PartNumber":2}"#,
                Some("PartNumber"),
            ),
            (
                "two unbound members",
                r#"{"Bucket":"b","Range":"bytes=0-1","Zebra":"z"}"#,
                Some("Range, Zebra"),
            ),
        ];
        for (case, body, expected) in rows {
            let input = Input::read(body).expect("the row's document is an object");
            let answered = input.only(&bound).err().map(|refusal| format!("{refusal:?}"));
            match expected {
                None => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the call to be sent, got {answered:?}"
                ),
                Some(named) => {
                    let reported = answered.unwrap_or_default();
                    assert!(
                        reported.contains(named),
                        "case {case:?}: expected the refusal to name {named:?}, got \
                         {reported:?}"
                    );
                }
            }
        }
    }

    /// The members a call carries are read as the types the model gives them, and bytes
    /// travel as the base64 a blob member is carried as. The rows differ along the type the
    /// document holds against the type the member has.
    #[test]
    fn a_member_is_read_as_the_type_its_model_gives_it() {
        let input = Input::read(
            r#"{"Bucket":"b","MaxKeys":7,"Metadata":{"owner":"team"},"Body":"aGVsbG8=",
                "Bad":[1]}"#,
        )
        .expect("the document is an object");
        assert_eq!(
            input.text("Bucket").expect("a string member"),
            Some("b".to_string()),
            "a string member reads as its text"
        );
        assert_eq!(
            input.number("MaxKeys").expect("a numeric member"),
            Some(7),
            "a numeric member reads as its number"
        );
        assert_eq!(
            input
                .string_map("Metadata")
                .expect("a map member")
                .and_then(|held| held.get("owner").cloned()),
            Some("team".to_string()),
            "a map member reads as its entries"
        );
        assert_eq!(
            input.blob(BODY_MEMBER).expect("a blob member"),
            Some(b"hello".to_vec()),
            "a blob member reads as the bytes its base64 carries"
        );
        for (case, answered) in [
            ("a list where a string belongs", input.text("Bad").is_err()),
            ("a list where a number belongs", input.number("Bad").is_err()),
            ("a list where a map belongs", input.string_map("Bad").is_err()),
            ("text that is not base64", Input::read(r#"{"Body":"not base64!"}"#)
                .expect("the document is an object")
                .blob(BODY_MEMBER)
                .is_err()),
        ] {
            assert!(
                answered,
                "case {case:?}: expected the call not to be made, got a call that was"
            );
        }
        assert!(
            input.required_text("Key").is_err(),
            "a required member the document does not carry stops the call"
        );
    }

    /// Every operation this seam binds is named once, so the refusal that lists them lists
    /// each bound operation exactly once.
    #[test]
    fn the_bound_operations_are_named_once_each() {
        let mut sorted = BOUND_OPERATIONS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            BOUND_OPERATIONS.len(),
            "case {BOUND_OPERATIONS:?}: expected each bound operation once, got {sorted:?}"
        );
    }
}
