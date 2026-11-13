// SPDX-License-Identifier: CC0-1.0

//! What a DICOM import job's COMPLETION does outside this service, and what its outcome
//! is decided from.
//!
//! `StartDICOMImportJob` "stores processing results in the file specified by the
//! `outputS3Uri` parameter", and `GetDICOMImportJob` tells a caller that "an import job
//! can return a `jobStatus` as `COMPLETED` even if validation issues are discovered during
//! the import process. If a `jobStatus` returns as `COMPLETED`, we still recommend you
//! review the output manifests written to S3, as they provide details on the success or
//! failure of individual P10 object imports"
//! (`smithy_query(query="operation",operation="StartDICOMImportJob,GetDICOMImportJob")`).
//! Two things follow, and this module owns both:
//!
//! * the job's own outcome is about whether it could IMPORT ITS INPUT, not about any one
//!   object: an input the object store says is not there, or that it says holds nothing,
//!   is a job with nothing to import, which `FAILED` and the `message` the model
//!   documents as "The error message thrown if an import job fails" report
//!   ([`outcome_of`]). An object that is not a readable DICOM P10 file does NOT fail the
//!   job — it is one of the individual failures the manifests carry;
//! * a job that reaches a terminal status has WRITTEN those manifests beneath
//!   `outputS3Uri`, under the one folder per job AWS HealthImaging names
//!   `<datastoreId>-DicomImport-<jobId>` ([`output_location`]). They are the only record
//!   of what each object contributed, so they are rendered from exactly what the import
//!   read ([`job_output_objects`]) and from the image sets it materialised.
//!
//! The write is a hop into ANOTHER service, so it cannot be made under this service's
//! state lock and cannot be made by the clock. The job therefore CARRIES the output it
//! owes ([`JobOutput`]) and its terminal status moves with that delivery, exactly as the
//! sibling asynchronous services deliver theirs: the manifests exist before any read
//! reports the job terminal, and a job whose results could not be written reaches
//! `FAILED` rather than reporting a success whose manifest is not there.
//!
//! Two things drive the delivery, through the one claim below, so it is made EXACTLY ONCE:
//!
//! * the start call arms a timer for the instant the completion falls due
//!   ([`deliver_when_due`]), because a client that submits a job and then watches the
//!   output prefix never calls this service again;
//! * every job read makes the delivery the clock has ALREADY made due
//!   ([`deliver_due_outputs`]), which is the same materialisation a read already makes for
//!   the status — owed at that instant whether or not anybody polled — and is what still
//!   delivers when the timer did not run, or when the read that claimed a delivery was
//!   dropped before it reported back ([`DELIVERY_RECLAIM_AFTER`]).
//!
//! Nothing here reaches off this process: the hop is the in-process callback into this
//! same emulator ([`::service_common::s3_client`]), addressed by the REGION the start call
//! was signed for and by nothing else. The bucket and prefix a request carries select
//! objects of this emulator's own object store; they are never an address.

use ::std::collections::BTreeSet;
use ::std::sync::Arc;
use ::std::time::{Duration, Instant, SystemTime};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{
    service_state, ComAmazonawsMedicalimagingAhigatewayserviceState, ImportPrefixRead,
    ImportedImageSet, JobStatus, IMPORT_JOB_COMPLETION_DELAY, NO_STUDY_OR_SERIES,
};

/// The object every finished import job writes beneath its own output folder: the summary
/// of the whole job.
pub const JOB_OUTPUT_MANIFEST_KEY: &str = "job-output-manifest.json";

/// The object naming every input file that landed in an image set, one JSON record per
/// line. Written only by a job that imported at least one file.
pub const JOB_OUTPUT_SUCCESS_KEY: &str = "SUCCESS/success.ndjson";

/// The object naming every input file that landed in none, with the reason, one JSON
/// record per line. Written only by a job that failed at least one file.
pub const JOB_OUTPUT_FAILURE_KEY: &str = "FAILURE/failure.ndjson";

/// The `message` the job summary of a job that imported its input carries.
pub const IMPORT_JOB_COMPLETED_SUMMARY: &str = "IMPORT_JOB_COMPLETED";

/// The media type of the job summary.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type of the two line-per-record manifests.
const NDJSON_MEDIA_TYPE: &str = "application/x-ndjson";

/// How many job-output deliveries this service may have in flight at once.
///
/// CHOSEN, and a bound on WORK rather than on jobs: every submitted job arms a timer, so
/// without it a burst of submissions would leave an unbounded number of writes
/// outstanding. A submission that finds no slot free arms nothing and leaves the delivery
/// to the first read that finds it due, which is the same delivery through the same claim.
pub const MAX_DELIVERIES_IN_FLIGHT: usize = 32;

/// The most deliveries ONE read makes before it answers.
///
/// CHOSEN. A read materialises what the clock owes, and a listing can find many jobs due
/// at once; this bounds how many one response pays for. What is left over is delivered by
/// the next read or by its own timer — nothing is dropped, because the claim leaves an
/// undelivered job exactly as it found it.
const MAX_DELIVERIES_PER_READ: usize = 4;

/// How long into one read a further delivery may still be STARTED.
///
/// CHOSEN, and the bound that matters for a read's latency: with it, the wait one response
/// can add is this budget plus the one delivery it was inside the budget to start.
const READ_DELIVERY_BUDGET: Duration = Duration::from_secs(2);

/// How long a CLAIMED delivery may be outstanding before another caller may claim it
/// again.
///
/// CHOSEN, and a liveness bound rather than a timeout on the work: a claim is made under
/// the state lock and reported back by the claimant, so a claimant that never reports back
/// — the read it was made on was dropped — would otherwise leave the job claimed and never
/// terminal. It is measured on the MONOTONIC clock the claim is stamped with, so a reclaim
/// cannot race a claimant that is still working, not even across a wall-clock jump.
const DELIVERY_RECLAIM_AFTER: Duration = Duration::from_secs(30);

/// Added to the wait a timer makes, so the completion is due — not a microsecond short of
/// due — when the claim runs.
const DUE_SLACK: Duration = Duration::from_millis(10);

/// Where in the delivery of one job's output this service is.
///
/// A job that owes no delivery holds no [`JobOutput`] at all, which is what a delivery
/// that has been MADE leaves behind: the state below is only ever about one still owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// Nobody is making it; the first caller to find it due may claim it.
    Owed,
    /// One caller claimed it at this instant and has not reported back yet.
    InFlight(Instant),
}

/// The job output one import job still owes the prefix it named: where it goes, and the
/// objects it consists of.
///
/// Rendered when the job's input was read ([`job_output_objects`]), so what is written is
/// what that read established and not what a later state of the store would say.
#[derive(Debug, Clone)]
pub struct JobOutput {
    pub state: DeliveryState,
    /// The Region the object store is addressed in — the one the start call was signed
    /// for.
    pub region: String,
    pub bucket: String,
    /// The job's own output folder, ending in `/`.
    pub key_prefix: String,
    /// Each object's key WITHIN [`JobOutput::key_prefix`], its media type and its body.
    pub objects: Vec<(&'static str, &'static str, String)>,
}

/// The bucket and the key prefix one job's output objects go under:
/// `<outputS3Uri>/<datastoreId>-DicomImport-<jobId>/`, the folder AWS HealthImaging names
/// one import job's output after.
///
/// `None` when `output_s3_uri` is not an S3 URI at all — which the member's `@pattern`
/// has already ruled out by the time a handler reads it.
pub fn output_location(
    output_s3_uri: &str,
    datastore_id: &str,
    job_id: &str,
) -> Option<(String, String)> {
    let (bucket, prefix) = super::parse_s3_uri(output_s3_uri)?;
    let folder = format!("{datastore_id}-DicomImport-{job_id}/");
    let key_prefix = match prefix.as_str() {
        "" => folder,
        prefix if prefix.ends_with('/') => format!("{prefix}{folder}"),
        prefix => format!("{prefix}/{folder}"),
    };
    Some((bucket, key_prefix))
}

/// The terminal outcome one import has, given what the object store answered about its
/// input prefix: `None` for a job that is to COMPLETE, and the `message` it is to carry
/// for one that is to FAIL.
///
/// An import that has nothing to import did not run: a job whose input the store says is
/// not there, or says holds nothing, FAILS. A prefix no object store answered about
/// establishes nothing either way, so such a job completes exactly as it would have
/// before anything was asked.
pub fn outcome_of(read: &ImportPrefixRead, input_s3_uri: &str) -> Option<String> {
    read.holds_nothing()
        .then(|| empty_input_message(input_s3_uri))
}

/// How this service states a job that found nothing to import.
///
/// `DICOMImportJobProperties$message` is documented only as "The error message thrown if
/// an import job fails", so the sentence is this service's own, with the one variable part
/// — the input prefix the request named — taken from the request. It carries only
/// characters the member's `^[\w -:`]+$` pattern admits, which an `S3Uri` does too.
pub fn empty_input_message(input_s3_uri: &str) -> String {
    format!(
        "The import job failed because no DICOM P10 objects were found in the input prefix \
         {input_s3_uri}"
    )
}

/// How this service states a job whose results could not be written where it was asked to
/// write them.
///
/// Same rule as [`empty_input_message`]: the import's results ARE the manifests under the
/// output prefix, so a job that could not write them did not finish, and the caller is
/// told which prefix refused them.
pub fn unwritable_output_message(output_s3_uri: &str) -> String {
    format!(
        "The import job failed because its results could not be written to the output prefix \
         {output_s3_uri}"
    )
}

/// The objects one job's output consists of: the job summary, and the per-file manifests
/// of what each scanned object contributed.
///
/// Rendered from the read itself, so every count is of what was really scanned, imported
/// and refused, and every image set named is one this import brought into existence.
pub fn job_output_objects(
    datastore_id: &str,
    job_id: &str,
    read: &ImportPrefixRead,
    imported: &[ImportedImageSet],
    failure: Option<&str>,
) -> Vec<(&'static str, &'static str, String)> {
    let mut imported_keys: BTreeSet<&str> = BTreeSet::new();
    let mut success = String::new();
    for image_set in imported {
        for key in &image_set.source_keys {
            imported_keys.insert(key.as_str());
            success.push_str(
                &::serde_json::json!({
                    "inputFile": key,
                    "imageSetId": image_set.image_set_id,
                })
                .to_string(),
            );
            success.push('\n');
        }
    }
    let mut failures = String::new();
    for object in &read.objects {
        if imported_keys.contains(object.key.as_str()) {
            continue;
        }
        failures.push_str(
            &::serde_json::json!({
                "inputFile": object.key,
                // An object that WAS read as a DICOM P10 file and still landed in no image
                // set is one the grouping could not place.
                "message": object.skipped.unwrap_or(NO_STUDY_OR_SERIES),
            })
            .to_string(),
        );
        failures.push('\n');
    }
    let scanned = read.objects.len();
    let summary = ::serde_json::json!({
        "jobSummary": {
            "jobId": job_id,
            "datastoreId": datastore_id,
            "code": if failure.is_some() { "FAILED" } else { "SUCCESS" },
            "message": failure.unwrap_or(IMPORT_JOB_COMPLETED_SUMMARY),
            "numberOfScannedFiles": scanned,
            "numberOfImportedFiles": imported_keys.len(),
            "numberOfFilesWithCustomerError": scanned.saturating_sub(imported_keys.len()),
            // Nothing in this emulator fails an object for a reason of its own: every
            // refusal above is about what the object itself carried.
            "numberOfFilesWithServerError": 0,
            "numberOfGeneratedImageSets": imported.len(),
            "imageSetsSummary": imported
                .iter()
                .map(|image_set| ::serde_json::json!({
                    "imageSetId": image_set.image_set_id,
                    "numberOfMatchedSOPInstances": image_set.source_keys.len(),
                }))
                .collect::<Vec<_>>(),
        },
    });
    let mut objects = vec![(
        JOB_OUTPUT_MANIFEST_KEY,
        JSON_MEDIA_TYPE,
        summary.to_string(),
    )];
    // Each per-file manifest is written by the job that has such files and not by one that
    // has none: an empty manifest states nothing the summary does not.
    if !success.is_empty() {
        objects.push((JOB_OUTPUT_SUCCESS_KEY, NDJSON_MEDIA_TYPE, success));
    }
    if !failures.is_empty() {
        objects.push((JOB_OUTPUT_FAILURE_KEY, NDJSON_MEDIA_TYPE, failures));
    }
    objects
}

/// Arm the delivery of one job's output for the instant its completion falls due.
///
/// Returns at once: the delivery runs in a task of its own, because the start call has to
/// answer while the job is still `SUBMITTED`. It exists for the client that reads the
/// output prefix rather than the job.
///
/// It materialises exactly what the clock has made due, through the same claim a read
/// uses, so what it writes is what a read of the same instant would have written, and a
/// delivery another caller already claimed is left alone.
pub fn deliver_when_due<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    datastore_id: String,
    job_id: String,
    due_at: SystemTime,
) {
    let Some(state) = service_state(&*engine) else {
        return;
    };
    if !state.lock().begin_delivery() {
        ::tracing::warn!(
            job = %job_id,
            "{MAX_DELIVERIES_IN_FLIGHT} import job output deliveries are already in flight; \
             this job's output is written by the next read that finds its completion due"
        );
        return;
    }
    ::tokio::spawn(async move {
        let wait = due_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .saturating_add(DUE_SLACK)
            // A due instant a clock jump put far in the future cannot make this task wait
            // longer than the span the completion was promised in.
            .min(IMPORT_JOB_COMPLETION_DELAY.saturating_add(DUE_SLACK));
        ::tokio::time::sleep(wait).await;
        // The guard is released before anything is delivered, so this service's store is
        // never held across a call into another service's.
        let claimed = service_state(&*engine).and_then(|state| {
            let mut guard = state.lock();
            claim_one(&mut guard, &datastore_id, &job_id, SystemTime::now())
        });
        if let Some(claim) = claimed {
            deliver(&*engine, &callback, &datastore_id, &job_id, claim).await;
        }
        // The slot is held for the whole wait AND the delivery, and given back however
        // either ends.
        if let Some(state) = service_state(&*engine) {
            state.lock().finish_delivery();
        }
    });
}

/// Make the job-output deliveries the CLOCK has already made due, at most
/// [`MAX_DELIVERIES_PER_READ`] of them and starting no further one past
/// [`READ_DELIVERY_BUDGET`].
///
/// Every job read calls this before it reports: the delivery was owed at this instant
/// whether or not anybody polled, so making it changes nothing a caller could not have
/// observed by waiting — and it is what makes the manifests present before the read that
/// reports the job terminal. Each delivery is claimed one at a time, so a read that has
/// spent its budget leaves the rest exactly as it found them, for their own timers or for
/// the next read.
///
/// `asked_about` is the job the read is ABOUT, when it is about one: its delivery is made
/// first, so a read of one job reports that job's own completion rather than spending its
/// budget on another's and answering non-terminal.
pub async fn deliver_due_outputs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    now: SystemTime,
    asked_about: Option<&str>,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The jobs this read may deliver, named under the lock and bounded before anything is
    // written: what is left over is for the next read or for the jobs' own timers.
    let due: Vec<(String, String)> = {
        let guard = state.lock();
        let mut due: Vec<(String, String)> = guard
            .datastores
            .values()
            .flat_map(|datastore| datastore.import_jobs.values())
            .filter(|job| delivery_is_due(job, now))
            .map(|job| (job.datastore_id.clone(), job.job_id.clone()))
            .collect();
        // The read's own job first, then the rest in the store's order.
        due.sort_by_key(|(_, job_id)| asked_about != Some(job_id.as_str()));
        due.truncate(MAX_DELIVERIES_PER_READ);
        due
    };
    let started = Instant::now();
    for (datastore_id, job_id) in &due {
        if started.elapsed() >= READ_DELIVERY_BUDGET {
            break;
        }
        // The guard is released before anything is delivered, so this service's store is
        // never held across a call into another service's. A job that cannot be claimed
        // here — another caller got to it first — costs this read nothing: the next one in
        // the list is tried instead.
        let claimed = {
            let mut guard = state.lock();
            claim_one(&mut guard, datastore_id, job_id, now)
        };
        let Some(claim) = claimed else {
            continue;
        };
        deliver(engine, callback, datastore_id, job_id, claim).await;
    }
}

/// Whether `job` still owes its output AND the clock has made its completion due.
///
/// A delivery another caller claimed is not due — until that claim is stale
/// ([`DELIVERY_RECLAIM_AFTER`]), which is how a claimant that never reported back leaves
/// the job deliverable instead of wedged.
fn delivery_is_due(job: &super::ImportJob, now: SystemTime) -> bool {
    now >= job.completed_due_at
        && job.output_owed.as_ref().is_some_and(|owed| match owed.state {
            DeliveryState::Owed => true,
            DeliveryState::InFlight(claimed_at) => claimed_at.elapsed() >= DELIVERY_RECLAIM_AFTER,
        })
}

/// Claim the output delivery of one job, or answer `None` when it owes none, when its
/// completion is not due yet, or when another caller holds a claim on it that is not
/// stale.
///
/// The claim is the write that makes the delivery exactly once: it moves the job's
/// delivery to [`DeliveryState::InFlight`], stamped with this instant, under the state
/// lock — so only one caller is ever handed the work, and until it reports back the job
/// keeps the non-terminal status it was in.
fn claim_one(
    store: &mut ComAmazonawsMedicalimagingAhigatewayserviceState,
    datastore_id: &str,
    job_id: &str,
    now: SystemTime,
) -> Option<JobOutput> {
    let job = store
        .datastores
        .get_mut(datastore_id)?
        .import_jobs
        .get_mut(job_id)?;
    if !delivery_is_due(job, now) {
        return None;
    }
    let owed = job.output_owed.as_mut()?;
    owed.state = DeliveryState::InFlight(Instant::now());
    Some(owed.clone())
}

/// Make one claimed delivery: write the job's output objects, then apply the terminal
/// status it settles.
async fn deliver<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    datastore_id: &str,
    job_id: &str,
    claim: JobOutput,
) {
    let client = ::service_common::s3_client::s3_client(callback, &claim.region);
    let mut written = true;
    for (key, media_type, body) in &claim.objects {
        let put = client
            .put_object()
            .bucket(&claim.bucket)
            .key(format!("{}{key}", claim.key_prefix))
            .content_type(*media_type)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                body.clone().into_bytes(),
            ))
            .send()
            .await;
        if let Err(error) = put {
            ::tracing::warn!(
                bucket = ?claim.bucket,
                key = ?claim.key_prefix,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a DICOM import job's output could not be written to its output prefix"
            );
            written = false;
            break;
        }
    }
    settle(engine, datastore_id, job_id, written);
}

/// Apply the terminal status this delivery settles, and mark the delivery made so nothing
/// is written twice.
///
/// A delivery that wrote the manifests settles the outcome the job's input established; one
/// that could not write them leaves the job `FAILED`, because the results of an import are
/// the manifests and a job that did not write them did not finish. An input failure the
/// read already established is what the job reports either way: it is the FIRST thing that
/// went wrong with the job.
fn settle<Eng: Send + Sync + 'static>(
    engine: &Eng,
    datastore_id: &str,
    job_id: &str,
    written: bool,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let mut guard = state.lock();
    let Some(job) = guard
        .datastores
        .get_mut(datastore_id)
        .and_then(|datastore| datastore.import_jobs.get_mut(job_id))
    else {
        return;
    };
    if !written && job.failure.is_none() {
        job.failure = Some(unwritable_output_message(&job.output_s3_uri));
    }
    job.output_owed = None;
    job.settle_terminal();
}

impl ComAmazonawsMedicalimagingAhigatewayserviceState {
    /// Take one of the [`MAX_DELIVERIES_IN_FLIGHT`] slots, or answer `false` when they are
    /// all taken.
    fn begin_delivery(&mut self) -> bool {
        if self.deliveries_in_flight >= MAX_DELIVERIES_IN_FLIGHT {
            return false;
        }
        self.deliveries_in_flight += 1;
        true
    }

    /// Give one slot back, however the delivery that held it ended.
    fn finish_delivery(&mut self) {
        self.deliveries_in_flight = self.deliveries_in_flight.saturating_sub(1);
    }
}

impl super::ImportJob {
    /// Apply the terminal status and `endedAt` this job's outcome settles.
    ///
    /// `FAILED` with the `message` the outcome carries, or `COMPLETED` with none —
    /// `jobStatus` is the status of the job's EXECUTION, so a job that imported its input
    /// completed whatever any one object contributed.
    pub(super) fn settle_terminal(&mut self) {
        self.job_status = if self.failure.is_some() {
            JobStatus::Failed
        } else {
            JobStatus::Completed
        };
        // The due instant, not `now`: `endedAt` must not change between reads, and the
        // delivery that settles the job is made at the instant the completion fell due.
        self.ended_at = Some(self.completed_due_at);
    }
}
