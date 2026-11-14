// SPDX-License-Identifier: CC0-1.0

//! The account's STREAMS: the record one holds, the files it streams, and what every
//! reader of one reports.
//!
//! A stream is the set of S3 files AWS IoT serves to a device over MQTT, together with
//! the role it reads them under — "Creates a stream for delivering one or more large
//! files in chunks over MQTT" (`CreateStream`). The id is the stream's identity: the
//! model binds it as the `@httpLabel` of `/streams/{streamId}` on every single-stream
//! operation (`smithy_query(query="operation",operation="CreateStream")`), which is
//! why the store is a `BTreeMap` keyed by it — the map IS the store, and `ListStreams`
//! is `@paginated`, so a page boundary is only meaningful over a stable order.
//!
//! One record carries everything a reader hands back, so `DescribeStream` after
//! `CreateStream` reports the created stream member for member.
//!
//! A stream is DELETED on its own terms: an OTA update that names one holds nothing
//! back, because the reference lives on the OTA update's record and not on the
//! stream's, so the delete is not a reference count and the OTA update keeps the
//! `streamId` it was created with.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{StreamFile, StreamInfo};

/// How many streams this emulator keeps for the account.
///
/// AWS publishes a service quota on an account's streams; the exact figure is not in
/// the model, so this is the emulator's own ceiling on a store a caller can grow — the
/// same reason [`super::MAX_POLICIES`] exists — and `CreateStream` past it refuses
/// with the modelled `LimitExceededException` rather than growing without bound.
pub const MAX_STREAMS: usize = 100;

/// How many tags one stream may carry: the same documented IoT tagging limit
/// [`super::MAX_POLICY_TAGS`] records, for the same reason.
pub const MAX_STREAM_TAGS: usize = super::MAX_POLICY_TAGS;

/// The `streamVersion` a freshly created stream carries.
///
/// `CreateStream` answers a `streamVersion` for a stream that has had exactly one
/// write (its create), and `UpdateStream` moves it on from there — the same rule
/// [`super::FIRST_POLICY_VERSION_ID`] states for a policy's first version.
pub const FIRST_STREAM_VERSION: i32 = 1;

/// The ARN of a stream: `arn:aws:iot:<region>:<account>:stream/<streamId>`.
///
/// `stream/` is the resource type AWS IoT's stream ARNs use — it is what
/// `CreateStream$streamArn` / `StreamInfo$streamArn` report and what an
/// `iot:DescribeStream` policy statement names a stream by; the namespace and account
/// segments are the ones [`super::thing_arn`] documents.
pub fn stream_arn(region: &str, stream_id: &str) -> String {
    ::emulator_server::emulator_arn("iot", region, &format!("stream/{stream_id}"))
}

/// The message every operation that addresses ONE stream refuses an id the account
/// holds no stream under with.
///
/// The fixed text is the family's — a stream is either one this account holds or no
/// stream at all — and the variable parts are the id the request named and the
/// account the request was signed for ([`::engine::emulator_account_id`]).
pub fn missing_stream_message(stream_id: &str) -> String {
    format!(
        "Requested stream {stream_id} does not exist in given account {}",
        ::engine::emulator_account_id()
    )
}

/// One stream in the store, as this service persists it.
///
/// The files are kept as the MODELLED `StreamFile`s the request carried, so a reader
/// hands back what the writer sent rather than a reassembled copy that could drift
/// from it.
#[derive(Debug, Clone)]
pub struct RegisteredStream {
    /// The id the stream was created under; also its key in the store.
    pub stream_id: String,
    /// `arn:aws:iot:<region>:<account>:stream/<streamId>`, minted on create.
    pub stream_arn: String,
    /// `CreateStream$description`, when the request carried one.
    pub description: Option<String>,
    /// `CreateStream$files` — "The files to stream", as the writer supplied them.
    pub files: Vec<StreamFile>,
    /// `CreateStream$roleArn` — "An IAM role that allows the IoT service principal to
    /// access your S3 files".
    pub role_arn: String,
    /// Which version of the stream this record is: [`FIRST_STREAM_VERSION`] until an
    /// update moves it.
    pub stream_version: i32,
    /// `Tag$Key` -> `Tag$Value` for the tags the create request carried.
    pub tags: BTreeMap<String, Option<String>>,
    /// When the stream was created — `StreamInfo$createdAt`.
    pub created_at: SystemTime,
    /// When the stream last changed — `StreamInfo$lastUpdatedAt`. Equal to
    /// [`RegisteredStream::created_at`] until a later write moves it.
    pub last_updated_at: SystemTime,
}

impl RegisteredStream {
    /// A newly created stream: the minted identity plus the requested files, role and
    /// tags, at [`FIRST_STREAM_VERSION`].
    pub fn new(
        stream_id: String,
        stream_arn: String,
        description: Option<String>,
        files: Vec<StreamFile>,
        role_arn: String,
        tags: BTreeMap<String, Option<String>>,
        created_at: SystemTime,
    ) -> Self {
        Self {
            stream_id,
            stream_arn,
            description,
            files,
            role_arn,
            stream_version: FIRST_STREAM_VERSION,
            tags,
            created_at,
            last_updated_at: created_at,
        }
    }

    /// The record as `DescribeStream` reports it — every member of the stored record,
    /// so the read and the create cannot drift.
    pub fn to_stream_info(&self) -> StreamInfo {
        StreamInfo {
            stream_id: Some(self.stream_id.clone()),
            stream_arn: Some(self.stream_arn.clone()),
            stream_version: Some(self.stream_version),
            description: self.description.clone(),
            files: Some(self.files.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            role_arn: Some(self.role_arn.clone()),
        }
    }
}
