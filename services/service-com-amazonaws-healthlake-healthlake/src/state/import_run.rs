// SPDX-License-Identifier: CC0-1.0

//! The RUN of a FHIR import job: reading the FHIR data the request's
//! `InputDataConfig` names and DELIVERING each resource it holds into the data
//! store the job imports into.
//!
//! `StartFHIRImportJob` is documented as importing "bulk FHIR data into an ACTIVE
//! data store" from "the InputDataConfig object", whose one member is an
//! `S3Uri` — "the user-specified Amazon S3 location of the FHIR data to be
//! imported into HealthLake". So an import is not a label on a job record: the
//! resources it reads are resources the data store HOLDS afterwards, served by
//! its FHIR REST data plane ([`crate::service::fhir_rest`]) exactly as it serves
//! one a caller created through the endpoint.
//!
//! # Where the data comes from
//!
//! This emulator holds an Amazon S3 of its own, and it is reached the way any
//! emulated service reaches another: the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN
//! PROCESS — no socket, no outbound request, no host resolution, and no address
//! a caller supplies is ever dialled. An object a caller staged with `PutObject`
//! is therefore the object an import job reads.
//!
//! HOW MUCH IS READ IS BOUNDED, four ways, each checked before the bytes are
//! taken into this process where the listing declares them: how many objects one
//! job reads ([`MAX_IMPORT_OBJECTS`]), how large one object may be
//! ([`MAX_IMPORT_OBJECT_BYTES`]), how many bytes one job reads in total
//! ([`MAX_IMPORT_TOTAL_BYTES`]) and how many resources one job imports
//! ([`MAX_IMPORT_RESOURCES`]) — so no product of caller-chosen factors is the
//! bound.
//!
//! # What the input holds
//!
//! The bulk form HealthLake documents for import is newline-delimited FHIR JSON:
//! one resource per line. A file that is instead ONE JSON document — a single
//! resource, or an array of them — is read as that document, because that is
//! what the bytes are; nothing here keys off a file name or an extension. The
//! JSON itself is read by `serde_json`, never by hand.
//!
//! A file holding no FHIR resource at all is a scanned NON-FHIR file, and a line
//! that is not a FHIR resource — unreadable JSON, a JSON value that is not an
//! object, an object carrying no `resourceType`, or one whose `id` is not a FHIR
//! logical id — is a resource with a customer error. Both are COUNTS on the
//! job's `JobProgressReport` rather than a refusal of the request: the job ran,
//! and what it found is what it reports.
//!
//! # What the job then reports
//!
//! The counts come from the run itself, and the terminal status follows them: a
//! run that imported every resource it scanned is `COMPLETED`, one that could
//! not import some of them is `COMPLETED_WITH_ERRORS`, and one that imported
//! nothing — an input nothing could be read from, or an emulator retention
//! ceiling reached — is `FAILED` with the `Message` saying which. Nothing is
//! reported as imported that the data store does not hold.
//!
//! WHERE the job wrote belongs to the same report: an import's results go into a
//! folder of the job's OWN under the output location the request supplied, so
//! the location the job reports is derived here too, by
//! [`import_job_output_location`].

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use ::serde_json::Value;

use crate::ops::types::{InputDataConfig, JobProgressReport, OutputDataConfig, S3Configuration};
use crate::service::fhir_rest::{is_logical_id, is_resource_type_name, stored_content};

use super::{
    recorded_instant, ComAmazonawsHealthlakeHealthlakeState, FhirResource, FhirResourceVersion,
    FhirRetentionCeiling, JobStatus, MAX_FHIR_RESOURCES_PER_DATASTORE, MAX_FHIR_RESOURCE_BYTES,
    MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE, MAX_FHIR_RESOURCE_BYTES_TOTAL,
};

/// How long after the call that starts it a FHIR import job's pending status
/// transition falls due.
///
/// AWS documents no duration for an import run, so this span is CHOSEN, for the
/// reason [`super::EXPORT_JOB_RUN`] is and with the same value: it is non-zero,
/// so a caller that describes the job immediately after starting it still
/// observes the `SUBMITTED` status it was answered with, and short enough that
/// nothing waits long for the settled one. It is never slept on — the due
/// instant is stored on the record and materialised by the next call that reads
/// it.
pub const IMPORT_JOB_RUN: Duration = Duration::from_millis(500);

/// The most FHIR import jobs this emulator holds at once, across every data
/// store.
///
/// A ceiling, not a claim about any AWS quota: it exists so a
/// `StartFHIRImportJob` loop cannot grow the emulator's heap without bound, and
/// it also bounds the per-request
/// [`ComAmazonawsHealthlakeHealthlakeState::settle_import_jobs`] walk and the
/// idempotency walk
/// [`ComAmazonawsHealthlakeHealthlakeState::import_job_for_client_token`] makes.
/// `StartFHIRImportJob` refuses past it with the `ThrottlingException` it models
/// — the quota-shaped refusal every other minting operation of this service
/// answers a full store with.
pub const MAX_IMPORT_JOBS: usize = 200;

/// The most Amazon S3 objects one import job reads.
pub const MAX_IMPORT_OBJECTS: usize = 100;

/// The largest single input object one import job reads.
///
/// A count ceiling alone does not bound the bytes taken into this process, so the
/// SIZE of one object is bounded too. It is the same ceiling the FHIR data plane
/// holds one stored resource to ([`MAX_FHIR_RESOURCE_BYTES`]) times the
/// resources a bulk file may reasonably carry, so a file of many small resources
/// is admitted while no single read is unbounded.
pub const MAX_IMPORT_OBJECT_BYTES: usize = MAX_FHIR_RESOURCE_BYTES * 8;

/// The most input bytes one import job reads across every object.
pub const MAX_IMPORT_TOTAL_BYTES: usize = 32 << 20;

/// The most FHIR resources one import job reads out of its input.
///
/// One data store holds at most [`MAX_FHIR_RESOURCES_PER_DATASTORE`] resources,
/// so a job that read more could not deliver them anyway; bounding the READ
/// keeps the parsed resources a job holds in memory bounded as well.
pub const MAX_IMPORT_RESOURCES: usize = MAX_FHIR_RESOURCES_PER_DATASTORE;

/// How many resources are parsed between yields.
///
/// Parsing is CPU work with no await of its own, so the read hands the runtime a
/// yield point this often: a cancelled request or a shutdown has somewhere to
/// land even while one object of many resources is being read.
const RESOURCES_PER_YIELD: usize = 256;

/// How many logical ids an imported resource that carries none offers before the
/// delivery gives up on it.
const MAX_ID_MINT_ATTEMPTS: usize = 8;

/// The fixed infix of the folder one FHIR import job writes its results into,
/// between the data store it imported into and the job itself.
///
/// The per-job output folder HealthLake writes under is named
/// `<DatastoreId>-FHIR_IMPORT-<JobId>`, so this is the one literal text in a
/// reported output location: everything around it comes from the request and
/// from the job's own record.
const IMPORT_JOB_OUTPUT_SEGMENT_INFIX: &str = "-FHIR_IMPORT-";

/// The output location one FHIR import job REPORTS, derived from the location
/// the request that started it supplied.
///
/// An import does not write its results into the supplied location itself: it
/// writes them into a folder of its OWN under that location, named for the data
/// store it imported into and for the job
/// (`<DatastoreId>-FHIR_IMPORT-<JobId>/`), and that job-scoped location is what
/// `ImportJobProperties$JobOutputDataConfig` states — so two jobs writing under
/// one supplied location report two distinct locations, each naming the job that
/// wrote it. The supplied location is joined to the segment by exactly ONE `/`
/// whether or not it already ends in one, the trailing `/` marks the result as a
/// folder, and `KmsKeyId` is reported exactly as supplied.
pub fn import_job_output_location(
    supplied: OutputDataConfig,
    datastore_id: &str,
    job_id: &str,
) -> OutputDataConfig {
    let OutputDataConfig::S3Configuration(configuration) = supplied;
    OutputDataConfig::S3Configuration(S3Configuration {
        s3_uri: format!(
            "{}/{datastore_id}{IMPORT_JOB_OUTPUT_SEGMENT_INFIX}{job_id}/",
            configuration.s3_uri.trim_end_matches('/')
        ),
        kms_key_id: configuration.kms_key_id,
    })
}

/// The bytes one megabyte of the `JobProgressReport` size members stands for.
///
/// `TotalSizeOfScannedFilesInMB` is "the size (in MB) of files scanned from the
/// Amazon S3 input bucket", and a megabyte is 10^6 bytes, so the reported size is
/// the scanned bytes over this.
const BYTES_PER_MB: f64 = 1_000_000.0;

/// The `Message` an import job ends with when nothing could be read from the
/// input location at all: the object store answered that it holds nothing there,
/// or did not answer the read.
///
/// Fixed text that quotes NOTHING from the request — the caller already knows the
/// location it sent — and within the modelled `Message` alphabet
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`, len 1..2048), so the text a job reports
/// is one the model would admit. Every message below is written to that alphabet
/// for the same reason.
const INPUT_UNREADABLE_MESSAGE: &str =
    "The import job could not read the FHIR data from the input Amazon S3 location.";

/// The `Message` an import job ends with when the input location holds no object
/// to read.
const INPUT_EMPTY_MESSAGE: &str = "No FHIR data was found at the input Amazon S3 location.";

/// One FHIR resource an import read out of its input, before it is delivered.
#[derive(Debug, Clone)]
pub struct ImportedResource {
    /// The `resourceType` the resource carries, checked against the FHIR
    /// resource-type form by the reader.
    resource_type: String,
    /// The FHIR logical id the resource carries, when it carries a well-formed
    /// one. `None` leaves the id to the data store's own mint, as a FHIR
    /// `create` does.
    id: Option<String>,
    /// The resource JSON as the input held it.
    content: Value,
    /// Which scanned file this resource came from, so the progress report can
    /// state how many FILES were imported without a second pass over the input.
    file: usize,
}

/// What one import job's run read out of its input: the resources to deliver, and
/// the counts the `JobProgressReport` states.
///
/// Every count is of something the run actually did, so a report cannot claim a
/// file or a resource nothing read.
#[derive(Debug, Clone, Default)]
pub struct ImportRun {
    /// The resources read, in the order the input held them.
    resources: Vec<ImportedResource>,
    /// How many input objects were read.
    scanned_files: usize,
    /// How many bytes those objects held together.
    scanned_bytes: usize,
    /// How many objects could not be read, or did not arrive as text.
    files_read_with_error: usize,
    /// How many objects held no FHIR resource at all.
    non_fhir_files: usize,
    /// How many bytes those objects held together.
    non_fhir_bytes: usize,
    /// How many resources were scanned, including the ones that could not be
    /// read as a FHIR resource.
    scanned_resources: usize,
    /// How many of those could not be read as a FHIR resource.
    resources_with_error: usize,
    /// Why the run could not be carried out at all, when it could not: the
    /// `Message` the job ends `FAILED` with.
    failure: Option<String>,
}

/// What delivering an import run established: the terminal status the clock owes
/// the job, the `Message` that status comes with, and the counts the run
/// produced.
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    /// The status the job reaches when its run span is over.
    pub status: JobStatus,
    /// Why it ends that way, when there is anything to say.
    pub message: Option<String>,
    /// The counts the run produced, reported by `DescribeFHIRImportJob` and
    /// `ListFHIRImportJobs` from the job's own record.
    pub report: JobProgressReport,
}

/// Read the FHIR data the import job's `input` names out of this emulator's own
/// Amazon S3.
///
/// Answers an [`ImportRun`] rather than an error channel: reading the input is
/// the JOB's own work, so what it could not do is the job's own terminal status
/// and `Message`, never a failure of the `StartFHIRImportJob` request that
/// submitted it.
pub async fn read_import_input(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    input: &InputDataConfig,
) -> ImportRun {
    let InputDataConfig::S3Uri(uri) = input;
    let Some((bucket, prefix)) = split_s3_uri(uri) else {
        // The member targets `S3Uri`, whose pattern the request boundary already
        // enforced, so this is unreachable; answering the job's own failure
        // keeps the run total.
        return ImportRun {
            failure: Some(INPUT_UNREADABLE_MESSAGE.to_string()),
            ..ImportRun::default()
        };
    };
    let client = s3_client(ctx, region);
    let objects = match input_objects(&client, &bucket, &prefix).await {
        Ok(objects) => objects,
        Err(failure) => {
            return ImportRun {
                failure: Some(failure),
                ..ImportRun::default()
            }
        }
    };
    if objects.is_empty() {
        return ImportRun {
            failure: Some(INPUT_EMPTY_MESSAGE.to_string()),
            ..ImportRun::default()
        };
    }

    let mut run = ImportRun::default();
    let mut remaining_bytes = MAX_IMPORT_TOTAL_BYTES;
    for (file, (key, _listed)) in objects.iter().enumerate() {
        let read = client
            .get_object()
            .bucket(&bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                ::tracing::debug!(
                    bucket = ?bucket,
                    key = ?key,
                    code = ?::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                    "a HealthLake import job could not read an input object"
                );
            });
        let Ok(object) = read else {
            run.files_read_with_error += 1;
            continue;
        };
        let Ok(body) = object.body.collect().await else {
            run.files_read_with_error += 1;
            continue;
        };
        let bytes = body.into_bytes();
        // The listing's declared sizes decided the bounds above; these are the
        // backstop for an object whose fetched bytes disagree with its listing.
        if bytes.len() > MAX_IMPORT_OBJECT_BYTES || bytes.len() > remaining_bytes {
            run.failure = Some(input_too_large_message());
            break;
        }
        remaining_bytes -= bytes.len();
        run.scanned_files += 1;
        run.scanned_bytes = run.scanned_bytes.saturating_add(bytes.len());
        let Ok(text) = ::std::str::from_utf8(&bytes) else {
            // Nothing in a file that is not text can be a FHIR JSON resource.
            run.files_read_with_error += 1;
            run.non_fhir_files += 1;
            run.non_fhir_bytes = run.non_fhir_bytes.saturating_add(bytes.len());
            continue;
        };
        let held_before = run.resources.len();
        if let Err(failure) = read_resources(&mut run, file, text).await {
            run.failure = Some(failure);
            break;
        }
        if run.resources.len() == held_before {
            run.non_fhir_files += 1;
            run.non_fhir_bytes = run.non_fhir_bytes.saturating_add(bytes.len());
        }
    }
    run
}

impl ComAmazonawsHealthlakeHealthlakeState {
    /// Deliver `run`'s resources into the data store `datastore_id`, and report
    /// what that established.
    ///
    /// Each resource is stored exactly as the FHIR data plane stores one a caller
    /// wrote: through the ONE builder of a stored resource's bytes
    /// ([`stored_content`]), under the logical id the imported resource carries
    /// or else one the data store's own mint assigns, charged against the SAME
    /// three retention ceilings a create and an update are charged against
    /// ([`ComAmazonawsHealthlakeHealthlakeState::admits_fhir_resource`]). So a
    /// read of an imported resource and a read of a created one are the same
    /// interaction over the same kind of record.
    ///
    /// An id the data store already holds a resource under is a further VERSION
    /// of that resource, as an `update` of it would be: the version it held moves
    /// into the resource's history, where a read as of an earlier instant is
    /// still answered from.
    pub fn deliver_import_run(
        &mut self,
        datastore_id: &str,
        run: ImportRun,
        now: SystemTime,
    ) -> ImportOutcome {
        let stamp = recorded_instant(now);
        let Ok(last_updated) = stamp.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
            return ImportOutcome {
                status: JobStatus::Failed,
                message: Some(INPUT_UNREADABLE_MESSAGE.to_string()),
                report: progress_report(&run, 0, 0, run.resources_with_error),
            };
        };
        let mut imported = 0usize;
        let mut refused = run.resources_with_error;
        let mut imported_files: BTreeSet<usize> = BTreeSet::new();
        let mut ceiling: Option<FhirRetentionCeiling> = None;

        for resource in &run.resources {
            let id = match resource.id.clone() {
                Some(carried) => carried,
                None => {
                    // FHIR requires the SERVER to assign a logical id to a
                    // resource that carries none, and the mint is checked
                    // against the data store before it is used, so an import can
                    // never overwrite a resource held under another id.
                    match self.mint_unused_fhir_id(datastore_id, &resource.resource_type, now) {
                        Some(minted) => minted,
                        None => {
                            refused += 1;
                            continue;
                        }
                    }
                }
            };
            let key = (resource.resource_type.clone(), id.clone());
            // What the data store holds at this id decides the version this
            // write lands on, how many of the versions it would retain must be
            // dropped to keep the retained-version ceiling, and what those
            // dropped versions give back.
            let held = self
                .datastores
                .get(datastore_id)
                .and_then(|datastore| datastore.fhir_resources.get(&key))
                .map(|held| {
                    let drops = held.forced_drops_on_supersede();
                    (held.version.saturating_add(1), drops, held.release_dropping(drops))
                });
            let (version, drops, released) = held.unwrap_or((1, 0, 0));
            let content = stored_content(resource.content.clone(), &id, version, &last_updated);
            let serialized = content.to_string();
            let bytes = serialized.len();
            // The same per-resource ceiling the FHIR data plane holds a submitted
            // resource to: a resource larger than this emulator stores is not
            // stored by either writer.
            if bytes > MAX_FHIR_RESOURCE_BYTES {
                refused += 1;
                continue;
            }
            if let Err(reached) =
                self.admits_fhir_resource(datastore_id, bytes, released, held.is_none())
            {
                // A retention ceiling is about this emulator rather than about
                // one resource, so the delivery STOPS here: the resources left
                // are reported as scanned and not imported, which is what the
                // data store holds.
                ceiling = Some(reached);
                break;
            }
            let next = FhirResourceVersion {
                version,
                last_updated: stamp,
                content,
                bytes,
            };
            let Some(datastore) = self.datastores.get_mut(datastore_id) else {
                // The caller resolved the data store under this same lock.
                break;
            };
            let released = match datastore.fhir_resources.get_mut(&key) {
                Some(held) => held.supersede_dropping(next, drops),
                None => {
                    datastore.fhir_resources.insert(
                        key,
                        FhirResource {
                            resource_type: resource.resource_type.clone(),
                            id,
                            version: next.version,
                            last_updated: next.last_updated,
                            content: next.content,
                            bytes: next.bytes,
                            superseded: Vec::new(),
                        },
                    );
                    0
                }
            };
            datastore.fhir_resource_bytes = datastore
                .fhir_resource_bytes
                .saturating_sub(released)
                .saturating_add(bytes);
            imported += 1;
            imported_files.insert(resource.file);
        }

        let report = progress_report(&run, imported, imported_files.len(), refused);
        let (status, message) = outcome_status(&run, imported, refused, ceiling);
        ImportOutcome {
            status,
            message,
            report,
        }
    }

    /// A logical id for an imported resource of `resource_type` that carries
    /// none, from the data store's OWN mint — the one a FHIR create assigns from
    /// — checked against the resources the data store holds so an import cannot
    /// overwrite one of them.
    ///
    /// `None` when the capped walk found no unused candidate. Each call advances
    /// the mint counter, so every candidate differs and one attempt is enough in
    /// practice; the cap is what BOUNDS the walk, because this runs while the
    /// store lock is held — the same reason
    /// [`crate::service::fhir_rest`] bounds its own mint walk.
    fn mint_unused_fhir_id(
        &mut self,
        datastore_id: &str,
        resource_type: &str,
        now: SystemTime,
    ) -> Option<String> {
        for _ in 0..MAX_ID_MINT_ATTEMPTS {
            let candidate = self.next_fhir_resource_id(now);
            let taken = self
                .datastores
                .get(datastore_id)
                .is_some_and(|datastore| {
                    datastore.fhir_resource(resource_type, &candidate).is_some()
                });
            if !taken {
                return Some(candidate);
            }
        }
        None
    }
}

/// The terminal status the clock owes a delivered run, and the `Message` that
/// status comes with.
///
/// The status FOLLOWS the counts: everything scanned imported is `COMPLETED`,
/// some of it imported is `COMPLETED_WITH_ERRORS`, and none of it imported is
/// `FAILED` — so a job cannot report a success for an input it read nothing from
/// or for resources the data store does not hold.
fn outcome_status(
    run: &ImportRun,
    imported: usize,
    refused: usize,
    ceiling: Option<FhirRetentionCeiling>,
) -> (JobStatus, Option<String>) {
    if let Some(reached) = ceiling {
        return (
            JobStatus::Failed,
            Some(ceiling_message(reached, imported)),
        );
    }
    if let Some(failure) = &run.failure {
        // An input the run could not read through, with nothing imported from
        // what it did read: the reason it gives is the whole story.
        if imported == 0 {
            return (JobStatus::Failed, Some(failure.clone()));
        }
        return (
            JobStatus::CompletedWithErrors,
            Some(failure.clone()),
        );
    }
    if refused == 0 && run.files_read_with_error == 0 {
        return (JobStatus::Completed, None);
    }
    let partial = partial_message(run, imported, refused);
    if imported == 0 {
        return (JobStatus::Failed, Some(partial));
    }
    (JobStatus::CompletedWithErrors, Some(partial))
}

/// `JobProgressReport$Throughput`, "the transaction rate the import job is
/// processed at", for a run that scanned `scanned_bytes`.
///
/// The rate is the data the run took in over the time the run OCCUPIES — the
/// span between the job's `SubmitTime` and the `EndTime` the clock owes it,
/// [`IMPORT_JOB_RUN`] — stated in the megabytes the report's own size members
/// are stated in ([`BYTES_PER_MB`]) per second. It is stated to one decimal
/// place, so an import of well under a megabyte is reported as the `0.0` such a
/// rate rounds to rather than as a long fraction, and every run reports a rate:
/// a run that scanned nothing is `0.0`, not an absent member.
fn throughput(scanned_bytes: usize) -> f64 {
    let megabytes_per_second =
        (scanned_bytes as f64 / BYTES_PER_MB) / IMPORT_JOB_RUN.as_secs_f64();
    (megabytes_per_second * THROUGHPUT_DECIMALS).round() / THROUGHPUT_DECIMALS
}

/// The scale that states a reported `Throughput` to one decimal place.
const THROUGHPUT_DECIMALS: f64 = 10.0;

/// The `JobProgressReport` one run produced.
///
/// Every member is a count, a size or a rate the run itself measured, and a
/// member it measured is REPORTED even when what it measured is zero — the one
/// report AWS serializes carries its `Throughput` alongside the counts. The
/// members left absent are the ones this run has nothing to measure for:
/// `TotalFilesConverted` / `TotalResourcesGenerated` are documented as populated
/// only for an import that runs a transformation phase, which this one does not,
/// and so are the non-FHIR counts a FHIR import does not convert.
fn progress_report(
    run: &ImportRun,
    imported: usize,
    imported_files: usize,
    refused: usize,
) -> JobProgressReport {
    JobProgressReport {
        total_number_of_scanned_files: Some(count(run.scanned_files)),
        total_size_of_scanned_files_in_mb: Some(run.scanned_bytes as f64 / BYTES_PER_MB),
        total_number_of_imported_files: Some(count(imported_files)),
        total_number_of_resources_scanned: Some(count(run.scanned_resources)),
        total_number_of_resources_imported: Some(count(imported)),
        total_number_of_resources_with_customer_error: Some(count(refused)),
        total_number_of_files_read_with_customer_error: Some(count(run.files_read_with_error)),
        total_number_of_scanned_non_fhir_files: Some(count(run.non_fhir_files)),
        total_size_of_scanned_non_fhir_files_in_mb: Some(run.non_fhir_bytes as f64 / BYTES_PER_MB),
        throughput: Some(throughput(run.scanned_bytes)),
        ..JobProgressReport::default()
    }
}

/// A count as the modelled `GenericLong` members carry one, saturating rather
/// than wrapping on a count past what the member can hold.
fn count(counted: usize) -> i64 {
    i64::try_from(counted).unwrap_or(i64::MAX)
}

/// Read every FHIR resource one input file holds into `run`.
///
/// The bulk form is newline-delimited JSON, one resource per line; a file that is
/// instead ONE JSON document — a resource, or an array of them — is read as that
/// document. `Err` is the reason the whole run stops: the resource ceiling.
async fn read_resources(run: &mut ImportRun, file: usize, text: &str) -> Result<(), String> {
    match ::serde_json::from_str::<Value>(text) {
        // One JSON document holding one resource.
        Ok(Value::Object(object)) => take_resource(run, file, Value::Object(object)),
        // One JSON document holding a list of resources.
        Ok(Value::Array(values)) => {
            for (index, value) in values.into_iter().enumerate() {
                if index.is_multiple_of(RESOURCES_PER_YIELD) {
                    ::tokio::task::yield_now().await;
                }
                take_resource(run, file, value)?;
            }
            Ok(())
        }
        // Anything else is read line by line: newline-delimited JSON is not one
        // JSON document, so a whole-document parse of it fails.
        _ => {
            for (index, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                if index.is_multiple_of(RESOURCES_PER_YIELD) {
                    ::tokio::task::yield_now().await;
                }
                match ::serde_json::from_str::<Value>(line) {
                    Ok(value) => take_resource(run, file, value)?,
                    Err(_) => {
                        run.scanned_resources += 1;
                        run.resources_with_error += 1;
                    }
                }
            }
            Ok(())
        }
    }
}

/// Take one scanned JSON value as a FHIR resource of `file`, or count it as a
/// resource that could not be read as one.
///
/// A FHIR resource is a JSON object carrying a `resourceType`, and the id it
/// carries — when it carries one — has to be a FHIR logical id, because that id
/// is what the resource is stored and read back under.
fn take_resource(run: &mut ImportRun, file: usize, value: Value) -> Result<(), String> {
    run.scanned_resources += 1;
    if run.resources.len() >= MAX_IMPORT_RESOURCES {
        return Err(too_many_resources_message());
    }
    if !value.is_object() {
        run.resources_with_error += 1;
        return Ok(());
    }
    let resource_type = value
        .get("resourceType")
        .and_then(Value::as_str)
        .filter(|resource_type| is_resource_type_name(resource_type))
        .map(str::to_string);
    let Some(resource_type) = resource_type else {
        run.resources_with_error += 1;
        return Ok(());
    };
    let carried = value.get("id").and_then(Value::as_str);
    if carried.is_some_and(|id| !is_logical_id(id)) {
        run.resources_with_error += 1;
        return Ok(());
    }
    let id = carried.map(str::to_string);
    run.resources.push(ImportedResource {
        resource_type,
        id,
        content: value,
        file,
    });
    Ok(())
}

/// The input objects under `prefix` with their listed sizes, in listing order, or
/// the `Message` the job fails with.
///
/// The SIZE is carried so a job can refuse an object, or a whole input, that is
/// past its byte bounds BEFORE fetching and buffering any of it.
async fn input_objects(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<(String, usize)>, String> {
    let mut objects: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.clone())
            .send()
            .await;
        let page = match page {
            Ok(page) => page,
            Err(error) => {
                // Either the object store answered that it holds nothing at this
                // address, or nothing answered the read at all — and the second
                // is worth a warning, because a deployment with no Amazon S3
                // mounted to ask is one where no import ever reads anything. The
                // caller's own values are rendered in their escaped `Debug`
                // form, so a bucket or key carrying a newline cannot forge a log
                // line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    prefix = ?prefix,
                    code = ?::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                    "a HealthLake import job could not list its input in the emulator's Amazon S3"
                );
                return Err(INPUT_UNREADABLE_MESSAGE.to_string());
            }
        };
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the delimiter is a folder marker rather than a
            // file of FHIR data.
            if key.ends_with('/') {
                continue;
            }
            let size = usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(0);
            if size > MAX_IMPORT_OBJECT_BYTES {
                return Err(input_too_large_message());
            }
            objects.push((key.to_string(), size));
            if objects.len() > MAX_IMPORT_OBJECTS {
                return Err(too_many_files_message());
            }
        }
        // The listed sizes bound the whole input before a byte of it is fetched.
        let listed: usize = objects
            .iter()
            .fold(0usize, |total, (_, size)| total.saturating_add(*size));
        if listed > MAX_IMPORT_TOTAL_BYTES {
            return Err(input_too_large_message());
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    Ok(objects)
}

/// An `s3://bucket/key` URI split into its bucket and its key prefix, or `None`
/// for a string the modelled `S3Uri` pattern
/// (`^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`) would not have admitted.
///
/// An import location may name one object or a prefix holding many, and a key is
/// a prefix of itself, so both are read with the one listing the prefix gives.
/// `pub(crate)` because every Amazon S3 location this service is pointed at — a
/// sample data file among them ([`super::sample_data`]) — is split the one way.
pub(crate) fn split_s3_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("s3://")?;
    let (bucket, key) = match rest.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// The Amazon S3 client an import job reads its input with, addressed to
/// `region`.
///
/// `pub(crate)` because the FHIR EXPORT job writes its output through the SAME
/// seam ([`super::export_run`]): one client configuration for every Amazon S3
/// call this service makes, so a read and a write cannot reach two different
/// object stores.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url`
/// below is an ADDRESS the in-process router reads rather than a host any
/// resolver looks up: this client reaches the emulator's own Amazon S3 and
/// nothing else, whatever bucket or key a caller names. The one caller-supplied
/// value that reaches its authority is the signing region, and it is narrowed
/// before it gets here — [`super::request_region`] admits only a value shaped
/// like a Region identifier and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of
/// which would need a sleep implementation this client is not given. The same
/// goes for caching the identity: the credentials the callback hands over are
/// used directly, and they only sign this operation's in-process calls.
pub(crate) fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // request addresses its bucket in the path.
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

/// The `Message` a job ends with when its input is past the bytes this emulator
/// reads in one job. The bound is the SERVICE's own, so the sentence states it
/// and quotes nothing from the request.
fn input_too_large_message() -> String {
    format!(
        "The input is larger than the {MAX_IMPORT_TOTAL_BYTES} bytes this emulator reads in one \
         import job."
    )
}

/// The `Message` a job ends with when its input holds more files than this
/// emulator reads in one job.
fn too_many_files_message() -> String {
    format!(
        "The input holds more than the {MAX_IMPORT_OBJECTS} files this emulator reads in one \
         import job."
    )
}

/// The `Message` a job ends with when its input holds more FHIR resources than
/// this emulator imports in one job.
fn too_many_resources_message() -> String {
    format!(
        "The input holds more than the {MAX_IMPORT_RESOURCES} FHIR resources this emulator \
         imports in one job."
    )
}

/// The `Message` a job ends with when a retention ceiling stopped the delivery:
/// which ceiling, and how many resources the data store did take.
fn ceiling_message(reached: FhirRetentionCeiling, imported: usize) -> String {
    match reached {
        FhirRetentionCeiling::ResourceCount => format!(
            "This data store already holds the maximum number of FHIR resources this emulator \
             retains: {MAX_FHIR_RESOURCES_PER_DATASTORE}. Resources imported: {imported}."
        ),
        FhirRetentionCeiling::DatastoreBytes => format!(
            "This data store already retains as many FHIR resource bytes as this emulator holds \
             for one data store: {MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE}. Resources imported: \
             {imported}."
        ),
        FhirRetentionCeiling::StoreBytes => format!(
            "This emulator already retains as many FHIR resource bytes as it holds across every \
             data store: {MAX_FHIR_RESOURCE_BYTES_TOTAL}. Resources imported: {imported}."
        ),
    }
}

/// The `Message` a job ends with when some of what it scanned could not be
/// imported: the counts the run measured, and nothing from the request.
fn partial_message(run: &ImportRun, imported: usize, refused: usize) -> String {
    format!(
        "Imported {imported} of {} scanned FHIR resources. Resources with customer error: \
         {refused}. Files that could not be read: {}.",
        run.scanned_resources, run.files_read_with_error
    )
}
