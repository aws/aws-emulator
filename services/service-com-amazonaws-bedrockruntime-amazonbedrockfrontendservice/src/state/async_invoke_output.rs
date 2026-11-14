// SPDX-License-Identifier: CC0-1.0

//! What a finished asynchronous invocation WRITES, and the settle that delivers it.
//!
//! # The rule this module implements
//!
//! `StartAsyncInvoke` accepts an invocation with an output location
//! (`outputDataConfig.s3OutputDataConfig.s3Uri`), and the whole point of the asynchronous
//! API is that a client polls the invocation to a terminal status and then READS what it
//! produced out of its own bucket. So an invocation reaching the modelled `Completed`
//! status is not the status field alone: its output has to be there to read, under the
//! invocation's own segment of the submitted prefix, by the time any read reports it
//! finished. An invocation whose output could NOT be written carried out none of the work
//! it was accepted for, so it ends in the modelled `Failed` status carrying the reason
//! ([`super::ASYNC_INVOKE_FAILURE_MESSAGE`]) rather than reporting a success no read of the
//! object store would confirm.
//!
//! # What it writes and what it will not claim
//!
//! One object per finished invocation: the MANIFEST of the job, at
//! `<prefix>/<invocationId>/manifest.json`, which is what indexes an asynchronous
//! invocation's output under the prefix every invocation submitted against that location
//! shares. Everything it states is a fact of the record — which invocation it is, which
//! model was invoked, when the invocation was submitted and when it ended, and where its
//! own segment is.
//!
//! No language model runs in this emulator, so NO media bytes are written beside it: a
//! video file nothing rendered would be output synthesised to look computed. The status,
//! the timestamps, the object and its contents are all real; the frames a model would have
//! generated are the one thing absent, and nothing here pretends otherwise.
//!
//! # Where the delivery happens
//!
//! Through the Amazon S3 emulator's own store on the shared engine — the SAME store
//! `PutObject` writes and `GetObject` / `ListObjectsV2` read — so one object has one home
//! and a client reads the invocation's output through Amazon S3's own operations. The
//! guard of this service's store is NEVER held while the object store's is taken (see
//! [`settle_due_invocations`]), exactly as the metric publication beside it
//! ([`super::invocation_metrics`]) holds neither.

use ::service_com_amazonaws_s3_amazons3 as s3;

use crate::ops::types::AsyncInvokeOutputDataConfig;

use super::AsyncInvokeRecord;

/// The object a finished invocation writes under its own segment of the submitted prefix:
/// the manifest that indexes what the invocation produced.
pub const MANIFEST_OBJECT: &str = "manifest.json";

/// The version the manifest states for its own layout, so a reader can tell one layout
/// from the next.
const MANIFEST_VERSION: &str = "1.0";

/// What delivering one due invocation's output needs, taken off the record under this
/// service's guard so nothing is held while the object store is written.
#[derive(Debug, Clone)]
pub struct DueInvocation {
    /// The invocation's own ARN: what the settle applies the terminal transition to.
    pub invocation_arn: String,
    /// The invocation id inside that ARN, which is the segment its output lives under.
    pub invocation_id: String,
    /// The model the invocation was submitted against.
    pub model_id: String,
    /// `outputDataConfig` as submitted: where the output goes.
    pub output_data_config: AsyncInvokeOutputDataConfig,
    /// Epoch milliseconds of the submitting call.
    pub submit_time: i64,
    /// Epoch milliseconds the invocation's terminal transition was due at, which is the
    /// instant the record carries as its end time.
    pub settles_at: i64,
}

impl DueInvocation {
    /// What one record needs delivering, or `None` for a record whose ARN names no
    /// invocation id — which is not a record this service minted, so it has no segment to
    /// write under.
    pub fn of(record: &AsyncInvokeRecord) -> Option<Self> {
        let invocation_id = ::service_common::arn::resource_id_of_type(
            &record.invocation_arn,
            super::ASYNC_INVOKE_ARN_RESOURCE_TYPE,
        )?;
        Some(Self {
            invocation_arn: record.invocation_arn.clone(),
            invocation_id: invocation_id.to_string(),
            model_id: record.model_id.clone(),
            output_data_config: record.output_data_config.clone(),
            submit_time: record.submit_time,
            settles_at: record.settles_at,
        })
    }
}

/// The bucket and the prefix an `s3OutputDataConfig.s3Uri` addresses, with any trailing
/// key separator dropped.
///
/// An output location is a PREFIX rather than one object, so a URI naming only a bucket
/// addresses its root (an empty prefix) rather than nothing, and the same prefix written
/// with and without a trailing separator addresses the same place. `None` for a value that
/// addresses no bucket at all.
pub fn output_location(config: &AsyncInvokeOutputDataConfig) -> Option<(&str, &str)> {
    let AsyncInvokeOutputDataConfig::s3OutputDataConfig(s3_config) = config;
    let rest = s3_config.s3_uri.trim().strip_prefix("s3://")?;
    let (bucket, tail) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then_some((bucket, tail.trim_end_matches('/')))
}

/// One invocation's own segment of the submitted prefix: the prefix, then the invocation
/// id.
///
/// The prefix a submission names is shared by every invocation submitted against it, so
/// one invocation's output lives under a segment of its OWN id. A prefix that addresses
/// the bucket root contributes no segment of its own.
fn invocation_prefix(prefix: &str, invocation_id: &str) -> String {
    if prefix.is_empty() {
        invocation_id.to_string()
    } else {
        format!("{prefix}/{invocation_id}")
    }
}

/// The key of the manifest of the invocation `invocation_id` names, under `prefix`.
pub fn manifest_key(prefix: &str, invocation_id: &str) -> String {
    format!(
        "{}/{MANIFEST_OBJECT}",
        invocation_prefix(prefix, invocation_id)
    )
}

/// The manifest document: which invocation this is, what it was invoked against, when it
/// ran, and where its own segment of the output location is.
///
/// No recording covers this document's own body, so what it states is exactly what it is
/// the index of — every member comes off the stored record.
fn manifest_body(invocation: &DueInvocation, bucket: &str, prefix: &str) -> ::serde_json::Value {
    ::serde_json::json!({
        "version": MANIFEST_VERSION,
        "jobId": invocation.invocation_id,
        "modelId": invocation.model_id,
        "submitTime": super::datetime_of_millis(invocation.submit_time)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default(),
        "endTime": super::datetime_of_millis(invocation.settles_at)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default(),
        "s3Uri": format!(
            "s3://{bucket}/{}",
            invocation_prefix(prefix, &invocation.invocation_id)
        ),
    })
}

/// Materialise every async-invoke transition the clock has already made due at
/// `now_millis`, having first DELIVERED each invocation's output to the location its
/// submission named.
///
/// The ONE settle path every reporting operation takes, so an invocation's output is
/// written and its outcome settled the same way whichever read got there first, and the
/// object is in the caller's bucket before any read reports the invocation finished. The
/// transition was owed at this instant whether or not anybody called, which is what makes
/// materialising it part of answering a read rather than a read with a side effect of its
/// own.
///
/// Each invocation is delivered and settled with NO guard of this service's store held, so
/// this service's store and the object store are never locked at once; an invocation a
/// concurrent read already settled is left exactly as that read left it
/// ([`super::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::settle_async_invoke`]).
pub fn settle_due_invocations<Eng: Send + Sync + 'static>(engine: &Eng, now_millis: i64) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let due = state.lock().due_async_invokes(now_millis);
    for invocation in due {
        let failure = deliver(engine, &invocation);
        state
            .lock()
            .settle_async_invoke(&invocation.invocation_arn, failure);
    }
}

/// Write one finished invocation's output into the location its submission named, and
/// report what stopped it, or `None` once the output is in the caller's bucket.
///
/// The failure is reported in the service's own words
/// ([`super::ASYNC_INVOKE_FAILURE_MESSAGE`]) and what exactly refused the write is logged
/// where a developer can see it, with the caller's own values rendered in their escaped
/// `Debug` form so a bucket name carrying a newline cannot forge a log line.
fn deliver<Eng: Send + Sync + 'static>(engine: &Eng, invocation: &DueInvocation) -> Option<String> {
    let failed = || Some(super::ASYNC_INVOKE_FAILURE_MESSAGE.to_string());
    let Some((bucket, prefix)) = output_location(&invocation.output_data_config) else {
        ::tracing::warn!(
            invocation = ?invocation.invocation_id,
            "an async invocation's output location addresses no bucket, so its output was not \
             written"
        );
        return failed();
    };
    let Some(store) = s3::state::service_state(engine) else {
        ::tracing::warn!(
            invocation = ?invocation.invocation_id,
            "no Amazon S3 is mounted beside this service, so an async invocation's output \
             could not be written"
        );
        return failed();
    };
    let key = manifest_key(prefix, &invocation.invocation_id);
    let body = match ::serde_json::to_vec(&manifest_body(invocation, bucket, prefix)) {
        Ok(body) => body,
        Err(error) => {
            ::tracing::warn!(%error, "an async invocation's manifest could not be rendered");
            return failed();
        }
    };
    let object = s3::state::Object::stored(body, Some(super::JSON_MIME.to_string()));
    let written = {
        let mut objects = store.lock();
        objects.put_object(bucket, &key, object)
    };
    match written {
        Some(s3::state::ObjectWrite::Created | s3::state::ObjectWrite::Replaced) => None,
        refused => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                outcome = ?refused,
                "an async invocation's output could not be written to this emulator's Amazon S3"
            );
            failed()
        }
    }
}
