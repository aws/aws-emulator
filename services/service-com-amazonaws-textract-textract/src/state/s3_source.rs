// SPDX-License-Identifier: CC0-1.0

//! Where the BYTES of a submitted document come from: the request's own inline `Bytes`,
//! or the object Amazon S3 holds at the location it named.
//!
//! Textract takes a document "as base64-encoded bytes or an Amazon S3 object"
//! (`DetectDocumentText`'s `Document` member), and an asynchronous `Start*` takes nothing
//! else: a `DocumentLocation` is an S3 object only. So half of this service's input
//! surface is an object of the object store, and this emulator holds an Amazon S3 of its
//! own — reached the way any emulated service reaches another, through the operation
//! callback ([`smithy_server_runtime::OperationContext::callback`]), which routes IN
//! PROCESS: no socket, no outbound request, no host resolution. An object a caller staged
//! with `PutObject` is therefore the object Textract reads, and once its bytes are here
//! they are decoded and recognised exactly like inline bytes
//! ([`super::read_content`]) — one document answers the same whichever way it was
//! submitted.
//!
//! ## What grounds a refusal
//!
//! `InvalidS3ObjectException` says Textract "is unable to access the S3 object that's
//! specified in the request", so it is answered only for an object the store really did
//! not serve, and the two ways that happens are kept apart:
//!
//! * Amazon S3 ANSWERED that there is no such bucket, key or version, or that the read is
//!   not permitted ([`UNREADABLE_OBJECT_CODES`]) — a fact about the request, so an
//!   asynchronous submit is refused with it and no job is created
//!   ([`DocumentBytes::Established`]);
//! * nothing answered the read at all — this emulator was assembled with no object store
//!   to ask — so nothing about the object is established: a synchronous read answers the
//!   modelled error (it has no result to give), while an asynchronous job is created and
//!   ends in the modelled `FAILED` state carrying the reason
//!   ([`DocumentBytes::Unestablished`]).
//!
//! HOW MUCH IS READ IS BOUNDED, and bounded twice: the published ceiling for a document
//! ([`super::MAX_S3_DOCUMENT_BYTES`]) is checked against the length Amazon S3's own answer
//! DECLARES before a byte of the body is taken into this process, and the bytes are
//! checked against it again by [`super::read_content`], so an answer that declared no
//! length cannot get past it either.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use super::{DocumentRef, S3Ref, Unreadable, MAX_S3_DOCUMENT_BYTES};

/// What this service established about the bytes of a submitted document.
#[derive(Debug)]
pub enum DocumentBytes {
    /// The document's bytes.
    Read(Vec<u8>),
    /// The bytes cannot be obtained, and Amazon S3's own answer (or the size it declared)
    /// ESTABLISHED why: the reason is a fact about the request.
    Established(Unreadable),
    /// The bytes cannot be obtained and nothing established why: no object store answered
    /// the read.
    Unestablished(Unreadable),
}

/// The message Amazon Textract states `InvalidS3ObjectException` with: it names what the
/// service could not do and what the caller should check, and carries nothing from the
/// request — the caller already knows the location it sent, and the object's address is
/// not echoed back.
pub const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the object
/// a request named: the four in which it reports that an address holds nothing, and the
/// two in which it reports that the read is not permitted. Every other answer is this
/// emulator failing to ask its own object store, which establishes nothing about the
/// object.
const UNREADABLE_OBJECT_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "NotFound",
    "AccessDenied",
    "AllAccessDisabled",
];

/// The bytes of the document `reference` names.
///
/// `inline` are the `Document.Bytes` the request carried, when it carried them; a
/// reference naming an S3 object is read from the object store instead.
pub async fn document_bytes(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    reference: &DocumentRef,
    inline: Option<Vec<u8>>,
) -> DocumentBytes {
    let Some(object) = reference.s3.as_ref() else {
        return DocumentBytes::Read(inline.unwrap_or_default());
    };
    let region = super::request_region(ctx.parts());
    object_bytes(&s3_client(ctx, &region), object).await
}

/// Read one object's bytes through `client`.
async fn object_bytes(client: &::aws_sdk_s3::Client, object: &S3Ref) -> DocumentBytes {
    let read = client
        .get_object()
        .bucket(&object.bucket)
        .key(&object.name)
        .set_version_id(object.version.clone())
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            let code = error.code().unwrap_or_default();
            if UNREADABLE_OBJECT_CODES.contains(&code) {
                // The store itself says the object is not there, or not readable by this
                // caller: the refusal is grounded in its answer.
                return DocumentBytes::Established(unreadable(object));
            }
            // Any other answer is this emulator failing to ask its own object store
            // rather than the store reporting an unreadable object — and it is worth a
            // warning, because a deployment with no Amazon S3 mounted to ask is one where
            // no S3-sourced document is ever read. The caller's own values are rendered
            // in their escaped `Debug` form, so a bucket or key carrying a newline cannot
            // forge a log line.
            ::tracing::warn!(
                bucket = ?object.bucket,
                key = ?object.name,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a document could not be read from the emulator's Amazon S3"
            );
            return DocumentBytes::Unestablished(unreadable(object));
        }
    };
    // The published ceiling, against what the store SAYS the object holds, before any of
    // it is read into this process.
    if let Some(declared) = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
    {
        if declared > MAX_S3_DOCUMENT_BYTES {
            return DocumentBytes::Established(Unreadable::TooLarge(format!(
                "the document is {declared} bytes; the maximum size of a document read from an \
                 Amazon S3 object is {MAX_S3_DOCUMENT_BYTES} bytes"
            )));
        }
    }
    match read.body.collect().await {
        Ok(body) => DocumentBytes::Read(body.into_bytes().to_vec()),
        // The store answered, but the body did not arrive: nothing about the document was
        // established.
        Err(error) => {
            ::tracing::warn!(
                bucket = ?object.bucket,
                key = ?object.name,
                error = %error,
                "a document's bytes did not arrive from the emulator's Amazon S3"
            );
            DocumentBytes::Unestablished(unreadable(object))
        }
    }
}

/// The reason an object this service could not read is refused with, logged against the
/// address it was read from.
fn unreadable(object: &S3Ref) -> Unreadable {
    ::tracing::debug!(
        bucket = ?object.bucket,
        key = ?object.name,
        "Amazon Textract could not obtain the document object"
    );
    Unreadable::S3Unreadable(UNREADABLE_OBJECT_MESSAGE.to_string())
}

/// The Amazon S3 client this service reads a document object with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is
/// an ADDRESS the in-process router reads rather than a host any resolver looks up: this
/// client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a
/// caller names. The one caller-supplied value that reaches its authority is the signing
/// region, and it is narrowed before it gets here — [`super::request_region`] admits only a
/// value shaped like a Region identifier and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
/// `pub(super)`: the document insights job writes its result back through the SAME
/// in-process client ([`super::insights`]), so a read and a write of this service's object
/// store cannot be addressed differently.
pub(super) fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
