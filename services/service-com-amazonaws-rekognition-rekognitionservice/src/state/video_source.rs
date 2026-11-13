// SPDX-License-Identifier: CC0-1.0

//! WHERE the video a stored-video analysis runs on lives, and why one cannot be reached.
//!
//! A `Video` carries exactly one member — "the Amazon S3 bucket name and file name for the
//! video" (`smithy_query(query="shape",shape="Video")`) — so, unlike an `Image`, a video is
//! never submitted inline: it is always an object of the object store. This emulator holds
//! an Amazon S3 of its own, reached the way any emulated service reaches another, through
//! the operation callback, so a video a caller staged with `PutObject` is a video this
//! service can start a job on, and an address nothing was staged at is one it refuses.
//!
//! What is read is the object's METADATA and not a byte of the media: a start operation
//! "starts asynchronous detection" and reports a job identifier, so at the instant it
//! answers, the only thing it has established about the video is that the store serves the
//! object named. That is also what AWS's own refusal says it could not do ("Unable to get
//! object metadata from S3", [`super::image_source::UNREADABLE_OBJECT_MESSAGE`]), and it is
//! why nothing here reports a codec, a duration, a frame rate or a frame count: no decoder
//! runs in this process, so no property of the media itself is measured, and the published
//! ceiling on media size and duration is a property of media this emulator's in-memory
//! object store cannot hold.
//!
//! [`UnreachableVideo`] is the one reason type the video paths refuse with: each variant has
//! exactly one modelled answer, which the handler picks.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::ops::types::Video;

/// Why the video a request named cannot be analysed. Each variant has exactly one modelled
/// answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnreachableVideo {
    /// The request identifies no video at all: its `Video` carries no `S3Object`, which the
    /// model allows (the member is optional) so the request reaches the handler.
    /// Answered with `InvalidParameterException`.
    NoObject(String),
    /// The request names an object this emulator's Amazon S3 does not serve, or names one
    /// incompletely. Answered with `InvalidS3ObjectException`.
    Unreadable(String),
}

impl UnreachableVideo {
    /// The reason, as the modelled error's `Message`.
    pub fn message(&self) -> &str {
        match self {
            Self::NoObject(message) | Self::Unreadable(message) => message,
        }
    }

    /// The refusal for a request that identifies no video object.
    pub fn no_object() -> Self {
        Self::NoObject(super::modeled_error::INVALID_PARAMETERS_MESSAGE.to_string())
    }

    /// The refusal for a video object whose metadata did not arrive. Worded exactly as the
    /// image path words the same class, because it is the same failure of the same store.
    pub fn unreadable() -> Self {
        Self::Unreadable(super::image_source::UNREADABLE_OBJECT_MESSAGE.to_string())
    }
}

/// The object a video analysis job runs on: the address the store serves it at, as the
/// request named it.
///
/// It holds no property of the MEDIA, because none is measured — see this module's docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoLocation {
    /// `S3Object.Bucket` — the bucket the object store serves the video from.
    pub bucket: String,
    /// `S3Object.Name` — the object's key.
    pub name: String,
    /// `S3Object.Version` — the object version the request pinned, when it pinned one.
    pub version: Option<String>,
}

/// The video object `video` names, once the object store has confirmed it serves it.
///
/// This is the one check a start operation makes against the video, and it is a read of the
/// object's metadata: the bytes of the media are never taken into this process.
pub async fn locate_video(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    video: &Video,
) -> Result<VideoLocation, UnreachableVideo> {
    let Some(object) = video.s3_object.as_ref() else {
        return Err(UnreachableVideo::no_object());
    };
    // An S3 object is identified by its bucket and its key: without both there is no object
    // to read, and nothing was read, so this is the same refusal as an address the store
    // does not serve.
    let (Some(bucket), Some(name)) = (
        object.bucket.as_deref().filter(|value| !value.is_empty()),
        object.name.as_deref().filter(|value| !value.is_empty()),
    ) else {
        return Err(UnreachableVideo::unreadable());
    };
    let region = super::request_region(ctx.parts());
    let head = super::image_source::s3_client(ctx, &region)
        .head_object()
        .bucket(bucket)
        .key(name)
        .set_version_id(object.version.clone())
        .send()
        .await;
    if let Err(error) = head {
        if !super::image_source::store_denies_object(error.code()) {
            // Not the store reporting an object it does not serve but this emulator failing
            // to ask its own object store — worth a warning, because a deployment with no
            // Amazon S3 mounted to ask is one where no video is ever reachable. The caller's
            // own values are rendered in their escaped `Debug` form, so a bucket or key
            // carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?name,
                code = ?error.code(),
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a video's object metadata could not be read from the emulator's Amazon S3"
            );
        }
        return Err(UnreachableVideo::unreadable());
    }
    Ok(VideoLocation {
        bucket: bucket.to_string(),
        name: name.to_string(),
        version: object.version.clone(),
    })
}
