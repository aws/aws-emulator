// SPDX-License-Identifier: CC0-1.0

//! Where the BYTES of a submitted `Image` come from, and why one cannot be read.
//!
//! An `Image` carries its content "either as base64-encoded image bytes or as a reference
//! to an image in an Amazon S3 bucket" (`DetectFaces`' own documentation), so half of this
//! service's image surface is an object of the object store — and this emulator holds an
//! Amazon S3 of its own, reached the way any emulated service reaches another: through the
//! operation callback ([`smithy_server_runtime::OperationContext::callback`]), which routes
//! IN PROCESS, with no socket, no outbound request and no host resolution. An image a
//! caller staged with `PutObject` is therefore the image Rekognition analyses, and once its
//! bytes are here they are decoded exactly like inline bytes — one image answers the same
//! whichever way it was submitted.
//!
//! HOW MUCH IS READ IS BOUNDED, and bounded twice: [`MAX_S3_IMAGE_BYTES`] is checked
//! against the length Amazon S3's own answer DECLARES before a byte of the body is taken
//! into this process, and against the bytes that did arrive, so an answer that declared no
//! length cannot get past it either.
//!
//! [`UnreadableImage`] is the one reason type the image paths refuse with: each variant has
//! exactly one modelled answer, which the handler picks.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::ops::types::Image;

/// The largest image this service reads out of an S3 object.
///
/// AWS publishes 15 MB for an image stored in an Amazon S3 bucket (the inline form is
/// capped at 5 MB by the model's own `ImageBlob` length constraint, which is refused before
/// a handler runs). The size is checked against what Amazon S3 DECLARES the object holds
/// before any of its bytes are taken into this process.
pub const MAX_S3_IMAGE_BYTES: usize = 15 * 1024 * 1024;

/// The message `InvalidS3ObjectException` is stated with: it names what the service could
/// not do and what the caller should check, and carries nothing from the request — the
/// caller already knows the location it sent.
pub const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The message an `Image` carrying neither form is refused with, as AWS words it for that
/// error class: fixed text that interpolates nothing — neither a request value nor a member
/// name — so it is the same for every image operation. The model marks both members
/// optional, so such a request reaches the handler and is refused there.
pub const MISSING_IMAGE_CONTENT_MESSAGE: &str =
    "Requested image should either contain bytes or s3 object.";

/// The message `InvalidImageFormatException` is stated with — the error AWS answers for
/// bytes that are not an image in a format it reads.
pub const INVALID_IMAGE_FORMAT_MESSAGE: &str = "Request has invalid image format";

/// The message `ImageTooLargeException` is stated with, as AWS words it for the error
/// class.
pub const IMAGE_TOO_LARGE_MESSAGE: &str = "Image size is too large";

/// The Amazon S3 error codes in which the store ANSWERS that it does not serve the object a
/// request named: the four in which it reports that an address holds nothing, and the two
/// in which it reports that the read is not permitted. Every other answer is this emulator
/// failing to ask its own object store, which establishes nothing about the object — a
/// synchronous analysis has no result to give either way, so both refuse alike, but only
/// the second is worth a warning.
pub(super) const UNREADABLE_OBJECT_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "NotFound",
    "AccessDenied",
    "AllAccessDisabled",
];

/// Why a submitted image cannot be analysed. Each variant has exactly one modelled answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnreadableImage {
    /// The bytes are not an image in a format this service decodes, or they are in one and
    /// the decoder cannot read them. Answered with `InvalidImageFormatException`.
    InvalidFormat(String),
    /// The image is over a published or emulator-side limit.
    /// Answered with `ImageTooLargeException`.
    TooLarge(String),
    /// The image is an S3 object whose bytes this emulator cannot obtain.
    /// Answered with `InvalidS3ObjectException`.
    S3Unreadable(String),
    /// The request does not identify an image at all.
    /// Answered with `InvalidParameterException`.
    InvalidParameter(String),
    /// The analysis did not complete — a panic or a cancellation on the blocking pool. This
    /// says nothing about the image, so it is answered as the server fault it is, with
    /// `InternalServerError`.
    NotAnalysed(String),
}

impl UnreadableImage {
    /// The reason, as the modelled error's `Message`.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidFormat(message)
            | Self::TooLarge(message)
            | Self::S3Unreadable(message)
            | Self::InvalidParameter(message)
            | Self::NotAnalysed(message) => message,
        }
    }

    /// The refusal for bytes this service cannot decode as an image.
    pub fn invalid_format() -> Self {
        Self::InvalidFormat(INVALID_IMAGE_FORMAT_MESSAGE.to_string())
    }

    /// The refusal for an S3 object whose bytes did not arrive.
    pub fn s3_unreadable() -> Self {
        Self::S3Unreadable(UNREADABLE_OBJECT_MESSAGE.to_string())
    }
}

/// The bytes of the image a request submitted: its own inline `Bytes`, or the object this
/// emulator's Amazon S3 holds at the location it named.
pub async fn image_bytes(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    image: &Image,
) -> Result<Vec<u8>, UnreadableImage> {
    if let Some(bytes) = image.bytes.as_ref() {
        if !bytes.is_empty() {
            return Ok(bytes.clone());
        }
        // `Bytes` present and empty IS inline content, and none of it is an image — a
        // different thing from a request that identified no content at all. The model's own
        // `ImageBlob` length constraint refuses it at the boundary, so this is the answer
        // for a request that somehow gets past that; an S3 object named beside it is still
        // read.
        if image.s3_object.is_none() {
            return Err(UnreadableImage::invalid_format());
        }
    }
    let Some(object) = image.s3_object.as_ref() else {
        return Err(UnreadableImage::InvalidParameter(
            MISSING_IMAGE_CONTENT_MESSAGE.to_string(),
        ));
    };
    // An S3 object is identified by its bucket and its key: without both there is no object
    // to read, and nothing was read, so this is the same refusal as an address the store
    // does not serve.
    let (Some(bucket), Some(name)) = (
        object.bucket.as_deref().filter(|value| !value.is_empty()),
        object.name.as_deref().filter(|value| !value.is_empty()),
    ) else {
        return Err(UnreadableImage::s3_unreadable());
    };
    let region = super::request_region(ctx.parts());
    let read = s3_client(ctx, &region)
        .get_object()
        .bucket(bucket)
        .key(name)
        .set_version_id(object.version.clone())
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            if !store_denies_object(error.code()) {
                // Not the store reporting an unreadable object but this emulator failing to
                // ask its own object store — worth a warning, because a deployment with no
                // Amazon S3 mounted to ask is one where no S3-sourced image is ever read.
                // The caller's own values are rendered in their escaped `Debug` form, so a
                // bucket or key carrying a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?name,
                    code = ?error.code(),
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an image could not be read from the emulator's Amazon S3"
                );
            }
            return Err(UnreadableImage::s3_unreadable());
        }
    };
    // The published ceiling, against what the store SAYS the object holds, before any of it
    // is read into this process.
    if let Some(declared) = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
    {
        if declared > MAX_S3_IMAGE_BYTES {
            return Err(too_large());
        }
    }
    match read.body.collect().await {
        Ok(body) => {
            let bytes = body.into_bytes();
            if bytes.len() > MAX_S3_IMAGE_BYTES {
                return Err(too_large());
            }
            Ok(bytes.to_vec())
        }
        // The store answered, but the body did not arrive: nothing of the image is here.
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?name,
                error = %error,
                "an image's bytes did not arrive from the emulator's Amazon S3"
            );
            Err(UnreadableImage::s3_unreadable())
        }
    }
}

/// Whether `code` is the object store's own answer that it does not serve the object a
/// request named — one of [`UNREADABLE_OBJECT_CODES`] — rather than this emulator failing
/// to ask its object store at all. Both refuse the request alike; only the second is worth
/// a warning, and both the image and the video path classify it the same way.
pub(super) fn store_denies_object(code: Option<&str>) -> bool {
    UNREADABLE_OBJECT_CODES.contains(&code.unwrap_or_default())
}

/// The refusal for an object larger than AWS serves an image analysis for.
///
/// [`MAX_S3_IMAGE_BYTES`] is AWS's own published ceiling for an S3-sourced image, so the
/// refusal is AWS's own wording for the error class and carries nothing from the request:
/// the caller already knows how large the object it named is.
fn too_large() -> UnreadableImage {
    UnreadableImage::TooLarge(IMAGE_TOO_LARGE_MESSAGE.to_string())
}

/// The Amazon S3 client this service reads a submitted object with, addressed to `region`.
///
/// One client for every object this service reads out of the store, whether that is an
/// `Image`'s bytes here or the metadata of an `Image`/`Video`'s object elsewhere in
/// [`super`]: they all reach the SAME emulated Amazon S3 the same way.
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
