// SPDX-License-Identifier: CC0-1.0

//! What a finished data-automation job WRITES, and the settle that delivers it.
//!
//! # The rule this module implements
//!
//! An asynchronous data-automation job is submitted with an output location
//! (`outputConfiguration.s3Uri`), and the whole point of the asynchronous API is that a
//! client polls the job to a terminal status and then READS that job's output out of its own
//! bucket. So a job reaching `Success` is not the status field alone: the objects it extracted
//! have to be there to read, under the job's own segment of the submitted prefix, by the time
//! any read reports it finished.
//!
//! Two objects per successful job, both derived from what reading the asset established
//! ([`super::asset_extraction`]):
//!
//! * the per-asset STANDARD OUTPUT document
//!   ([`standard_output_key`]), carrying that asset's `metadata` — which asset and
//!   subdocument it is, its modality, the bucket and key it was read from, its file type and
//!   its page range — and, under the member its modality is named by, the `representation` of
//!   what was extracted from it;
//! * the JOB METADATA document ([`job_metadata_key`]) at the top of that segment, which
//!   indexes the standard output of every asset and is the location
//!   `GetDataAutomationStatus` reports ([`super::Invocation::output_metadata_s3_uri`]) — so
//!   the two cannot name different places: both are laid out here. It is written for every
//!   job that finishes its work, because a location a status response reports and a client
//!   cannot read is the symptom this module exists to remove.
//!
//! The delivery is made THROUGH the same in-process callback the asset was read with
//! ([`super::s3_client`]), so this crate names no other service's crate and nothing leaves
//! this process.
//!
//! # What it writes and what it will not claim
//!
//! Everything in both documents comes from the record and from the asset's own bytes. A
//! modality this process extracts nothing from (an image) still gets its metadata — which
//! asset it was and what format it is are facts — and NO representation, because text nothing
//! read is not written. An asset nothing recognised, or one past the read ceiling, gets an
//! index that names no standard output rather than a document stating what was never read.
//!
//! # What the delivery does NOT decide
//!
//! The job's terminal STATUS rests on the asset resolution alone
//! ([`super::Invocation::advance_to`]): a failure must be grounded in what the object store
//! answered about the ASSET. An output location the store refuses (a bucket it does not hold)
//! is therefore logged and leaves the outcome where the asset put it — the same best-effort
//! footing as the lifecycle event a notified submission asks for
//! ([`super::job_events::publish_pending`]).

use std::time::SystemTime;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::asset_extraction::AssetReading;
use super::{InputAsset, JOB_METADATA_OBJECT};

/// The key segment a job's standard output sits under, inside one asset's segment.
const STANDARD_OUTPUT_SEGMENT: &str = "standard_output";

/// The standard-output document itself, at the end of that segment.
const STANDARD_OUTPUT_OBJECT: &str = "result.json";

/// The index of the asset a job's output is written for.
///
/// `inputConfiguration.s3Uri` names ONE asset, so a job has exactly one — asset `0` — and the
/// layout still carries the index because the document it indexes is a list.
const ASSET_INDEX: usize = 0;

/// The index of the logical subdocument a job's standard output covers.
///
/// One asset processed whole is one logical subdocument, `0`: splitting a document into
/// several is what a project's document override asks for, which nothing here does.
const SUBDOCUMENT_INDEX: usize = 0;

/// The checksum algorithm the objects a job writes are stored under.
///
/// Named EXPLICITLY rather than left to the SDK's or the store's default, because a read of a
/// job's output object reports the algorithm the write named, and the recorded output of a
/// data-automation job carries a CRC32 checksum over the whole object.
const OUTPUT_CHECKSUM: ::aws_sdk_s3::types::ChecksumAlgorithm =
    ::aws_sdk_s3::types::ChecksumAlgorithm::Crc32;

/// The request header the SDK gives a blob payload by default, which the write below drops.
///
/// A data-automation job sets no media type on the documents it writes, so each one carries
/// the object store's own default for an upload that names none; a store that recorded this
/// header would report a TRANSPORT decision as the media type the job wrote its output under.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The bucket and the prefix an `outputConfiguration.s3Uri` addresses, with any trailing key
/// separator dropped.
///
/// The modelled `S3Uri` pattern admits a URI naming a bucket with nothing under it, and an
/// output location is a PREFIX rather than one object, so a URI naming only a bucket addresses
/// its root (an empty prefix) rather than nothing. `None` for a value that addresses no
/// bucket at all.
#[must_use]
pub fn output_location(s3_uri: &str) -> Option<(&str, &str)> {
    // The one `s3://` split of this crate, with the trailing separator of a PREFIX dropped:
    // a URI naming only a bucket has an empty tail, which is its root.
    super::s3_bucket_and_tail(s3_uri).map(|(bucket, prefix)| (bucket, prefix.trim_end_matches('/')))
}

/// One job's own segment of the submitted prefix: the prefix, then the job's `invocationId`.
///
/// The prefix a submission names is shared by every job submitted under it, so one job's
/// output lives under a segment of its OWN id. A prefix that addresses the bucket root
/// contributes no segment of its own.
fn job_prefix(prefix: &str, invocation_id: &str) -> String {
    if prefix.is_empty() {
        invocation_id.to_string()
    } else {
        format!("{prefix}/{invocation_id}")
    }
}

/// The key of the job-metadata document of the job `invocation_id` names, under `prefix`.
#[must_use]
pub fn job_metadata_key(prefix: &str, invocation_id: &str) -> String {
    format!(
        "{}/{JOB_METADATA_OBJECT}",
        job_prefix(prefix, invocation_id)
    )
}

/// The key of the standard-output document of one asset segment of that job.
#[must_use]
pub fn standard_output_key(
    prefix: &str,
    invocation_id: &str,
    asset_index: usize,
    subdocument_index: usize,
) -> String {
    format!(
        "{}/{asset_index}/{STANDARD_OUTPUT_SEGMENT}/{subdocument_index}/{STANDARD_OUTPUT_OBJECT}",
        job_prefix(prefix, invocation_id)
    )
}

/// The standard-output document of one asset: its `metadata`, and the `representation` of what
/// was extracted from it under the member its modality is named by.
///
/// `asset_id` and `logical_subdocument_id` are the ids of the segment this document covers, as
/// the recorded document carries them — text, not numbers, unlike the page indices beside
/// them. The page members are stated only when the extraction counted the asset's pages, and
/// the modality's content member only when it extracted something: a representation of text
/// nothing read would be an invented claim about the asset.
fn standard_output_body(
    input_bucket: &str,
    input_key: &str,
    reading: &AssetReading,
) -> ::serde_json::Value {
    let mut metadata = ::serde_json::json!({
        "asset_id": ASSET_INDEX.to_string(),
        "logical_subdocument_id": SUBDOCUMENT_INDEX.to_string(),
        "semantic_modality": reading.modality,
        "s3_bucket": input_bucket,
        "s3_key": input_key,
        "file_type": reading.file_type,
    });
    if let Some(pages) = reading.page_count {
        metadata["number_of_pages"] = ::serde_json::json!(pages);
        metadata["start_page_index"] = ::serde_json::json!(0);
        metadata["end_page_index"] = ::serde_json::json!(pages.saturating_sub(1));
    }
    let mut body = ::serde_json::json!({ "metadata": metadata });
    if let Some(text) = &reading.text {
        body[reading.content_member()] = ::serde_json::json!({
            "representation": { "text": text },
            // The member the recorded document carries for an extracted asset, with no count
            // in it: this emulator states no statistic it did not compute.
            "statistics": {},
        });
    }
    body
}

/// The job-metadata document: where this job's output lives, and which standard-output
/// document belongs to which asset segment.
///
/// NO RECORDING covers this document's own body — only its LOCATION, which the status response
/// reports — so what it states is exactly what it is an index of: the job, the location its
/// output is under, the modality it processed when the asset established one, and the
/// standard-output object of each asset segment that HAS one. A job whose asset nothing could
/// be read out of indexes no segment, which is the honest index of no output rather than a
/// location nothing answers for.
fn job_metadata_body(
    bucket: &str,
    prefix: &str,
    invocation_id: &str,
    reading: Option<&AssetReading>,
    standard_output_key: &str,
) -> ::serde_json::Value {
    let mut body = ::serde_json::json!({
        "job_id": invocation_id,
        "s3_bucket": bucket,
        "s3_prefix": job_prefix(prefix, invocation_id),
        "output_metadata": [],
    });
    if let Some(reading) = reading {
        body["semantic_modality"] = ::serde_json::json!(reading.modality);
        body["output_metadata"] = ::serde_json::json!([{
            "asset_id": ASSET_INDEX.to_string(),
            "segment_metadata": [{
                "standard_output_path": format!("s3://{bucket}/{standard_output_key}"),
            }],
        }]);
    }
    body
}

/// Write the output of one finished job into the location its submission named.
///
/// The index is written for EVERY job that finishes its work, because the status response
/// reports its location and a reported location a client cannot read is the very symptom this
/// module exists to remove; the standard output is written for an asset something was read
/// out of, and the index then names it. Both are written with the job's own extraction, the
/// standard output FIRST, so the index never names an object that is not there yet. A write
/// the object store refuses is logged and nothing else: the job's status is settled on its
/// asset, not on this delivery (see the module documentation).
async fn deliver(
    callback: &EmbeddedEmulator,
    region: &str,
    pending: &PendingSettle,
    reading: Option<&AssetReading>,
) {
    let Some((bucket, prefix)) = output_location(&pending.output_s3_uri) else {
        ::tracing::warn!(
            "a data automation job's output location addresses no bucket, so its output was \
             not written"
        );
        return;
    };
    // The asset's own address, for the metadata that says what was processed. A submission
    // whose input URI addresses no object never reaches a successful settle.
    let Some((input_bucket, input_key)) = super::s3_object_address(&pending.input_s3_uri) else {
        return;
    };
    let standard_output = standard_output_key(
        prefix,
        &pending.invocation_id,
        ASSET_INDEX,
        SUBDOCUMENT_INDEX,
    );
    let client = super::s3_client(callback, region);
    let mut documents: Vec<(String, ::serde_json::Value)> = Vec::new();
    if let Some(reading) = reading {
        documents.push((
            standard_output.clone(),
            standard_output_body(input_bucket, input_key, reading),
        ));
    }
    documents.push((
        job_metadata_key(prefix, &pending.invocation_id),
        job_metadata_body(
            bucket,
            prefix,
            &pending.invocation_id,
            reading,
            &standard_output,
        ),
    ));
    for (key, body) in documents {
        let Ok(bytes) = ::serde_json::to_vec(&body) else {
            continue;
        };
        if let Err(error) = client
            .put_object()
            .bucket(bucket)
            .key(&key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(bytes))
            .checksum_algorithm(OUTPUT_CHECKSUM)
            .customize()
            .mutate_request(|request| {
                request.headers_mut().remove(CONTENT_TYPE_HEADER);
            })
            .send()
            .await
        {
            // The caller's own values are rendered with their escaped `Debug` form, so a key
            // carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a data automation job's output could not be written to this emulator's \
                 Amazon S3"
            );
        }
    }
}

/// What the settle of one job needs, taken off the record under the lock so nothing is held
/// across a call into another service.
struct PendingSettle {
    /// The job's own id — the segment its output lives under.
    invocation_id: String,
    /// `inputConfiguration.s3Uri` as submitted: the asset to read.
    input_s3_uri: String,
    /// `outputConfiguration.s3Uri` as submitted: the prefix the output goes under.
    output_s3_uri: String,
}

/// Materialise every lifecycle transition the clock owes the job `invocation_arn` names,
/// having first DELIVERED what that job extracted when the transition it owes is the terminal
/// one.
///
/// The ONE path both callers take — the status read that finds a transition due, and the
/// settle a notified submission starts on its job's behalf — so a job's output is written and
/// its outcome settled the same way whichever got there first, and the objects are there
/// before any read can report the job finished.
///
/// A transition the clock owes but has not yet made terminal (`Created` -> `InProgress`) needs
/// no answer about the asset and writes nothing, so it is materialised without costing a read
/// of the object store; a job that already reached a terminal status needs nothing at all.
pub async fn settle_due_transitions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    region: &str,
    invocation_arn: &str,
    now: SystemTime,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    // The guard is released before the asset is read, so this service's store and another
    // service's are never held at once.
    let pending = {
        let store = state.lock();
        store
            .invocation(invocation_arn)
            .filter(|invocation| invocation.terminal_transition_due(now))
            .map(|invocation| PendingSettle {
                invocation_id: invocation.invocation_id.clone(),
                input_s3_uri: invocation.input_s3_uri.clone(),
                output_s3_uri: invocation.output_s3_uri.clone(),
            })
    };
    let Some(pending) = pending else {
        // No terminal transition is owed, so nothing about the asset could settle anything and
        // nothing is asked. The clock may still owe the non-terminal advance, which this
        // materialises exactly as a read did before any output was written.
        state
            .lock()
            .settle_invocation(invocation_arn, now, InputAsset::Available);
        return;
    };
    let resolved = super::resolve_input_asset(callback, region, &pending.input_s3_uri).await;
    // Only a job that is about to finish its work has output to write.
    if resolved.verdict == InputAsset::Available {
        if resolved.reading.is_none() {
            ::tracing::warn!(
                "nothing could be read out of a data automation job's input asset, so its \
                 output indexes no standard output"
            );
        }
        deliver(callback, region, &pending, resolved.reading.as_ref()).await;
    }
    state
        .lock()
        .settle_invocation(invocation_arn, now, resolved.verdict);
}
