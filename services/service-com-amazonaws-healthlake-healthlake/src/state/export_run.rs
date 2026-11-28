// SPDX-License-Identifier: CC0-1.0

//! The RUN of a FHIR export job: WRITING the FHIR resources the data store holds
//! into the Amazon S3 location the request's `OutputDataConfig` names.
//!
//! `StartFHIRExportJob` is documented as starting "a FHIR export job", whose
//! required `OutputDataConfig` carries the `S3Configuration` the exported data
//! goes to and whose required `DataAccessRoleArn` is the role the service assumes
//! to write there. So an export is not a label on a job record: once the job has
//! settled, the destination location HOLDS the data store's resources, and a
//! client that lists that prefix reads them back.
//!
//! # Where the data goes
//!
//! This emulator holds an Amazon S3 of its own, and it is reached the way any
//! emulated service reaches another: the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes IN
//! PROCESS — no socket, no outbound request, no host resolution, and no address
//! a caller supplies is ever dialled. The client is the one the import run reads
//! its input with (`super::import_run::s3_client`), so a read and a write
//! cannot reach two different object stores, and the object this run writes is
//! the object a caller's own `GetObject` answers.
//!
//! # What is written
//!
//! A folder of the job's own — `<DatastoreId>-FHIR_EXPORT-<JobId>/` — inside the
//! location the request supplied, the same way an import job's results go into a
//! folder of its own ([`super::import_job_output_location`]): two exports under
//! one supplied location therefore write two distinct sets of objects, each
//! naming the job that wrote it. In it, one newline-delimited JSON file per
//! resource TYPE the data store holds, each line one stored resource exactly as
//! the data store's own FHIR endpoint answers it — so nothing here re-derives a
//! server-owned value or invents content. The folder itself is written as the
//! empty-bodied key ending in the delimiter: a completed export leaves its output
//! location holding the folder it created, which is what a client listing that
//! location reads back, including for a data store that held no resource to write
//! into it.
//!
//! The bytes written are the bytes the data store already holds, which its own
//! retention ceilings bound ([`super::MAX_FHIR_RESOURCES_PER_DATASTORE`],
//! [`super::MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]), and the objects written are
//! one per resource type held — so an export reads and writes no more than the
//! store it exports from.
//!
//! # What the job then reports
//!
//! The terminal status follows the run: a job whose objects were all written
//! reaches `COMPLETED`, and one whose destination could not be written ends in
//! the modelled `FAILED` status with the `Message` saying so.
//!
//! The run happens before the job record exists, so the gates `StartFHIRExportJob`
//! re-checks once it takes the store's guard back can still refuse the START after
//! the output was written — a data store deleted, or a job ceiling reached, while
//! this run was writing. Nothing is deleted on that path: the objects were written
//! while the data store was exportable, and no job record claims them.

use std::collections::BTreeMap;

use crate::ops::types::OutputDataConfig;

use super::{s3_client, split_s3_uri, Datastore, JobStatus};

/// The segment naming the folder one export job writes into, between the data
/// store's identifier and the job's own.
///
/// The counterpart of the import run's own output segment for the export
/// direction, and named for the same reason: the folder states which job wrote
/// the objects in it, so two jobs under one supplied location do not write over
/// each other.
pub const EXPORT_JOB_OUTPUT_SEGMENT_INFIX: &str = "-FHIR_EXPORT-";

/// The media type of an exported object: newline-delimited FHIR JSON, the bulk
/// form HealthLake documents for FHIR data, which is also what an import job
/// reads back.
pub const EXPORT_CONTENT_TYPE: &str = "application/fhir+ndjson";

/// The `Message` an export job ends with when its destination could not be
/// written.
///
/// Fixed text, like every other sentence this service states a job's outcome
/// with: it quotes neither the bucket nor the key from the request, and stays
/// inside the modelled `Message` alphabet
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`, len 1..2048).
pub const EXPORT_NOT_WRITTEN_MESSAGE: &str =
    "The export job could not write the data store FHIR resources to the destination Amazon S3 \
     bucket.";

/// What one export run did: the `Message` it failed with when it could not write
/// its output, and nothing when it wrote everything the data store held.
#[derive(Debug, Default, Clone)]
pub struct ExportRun {
    /// Why the run could not write, `None` for a run that wrote everything it
    /// had.
    pub failure: Option<String>,
}

impl ExportRun {
    /// The transition the clock owes a job this run started: the terminal status
    /// the run warrants, and the `Message` that status comes with.
    ///
    /// The message travels WITH the status it explains, so a job still
    /// `SUBMITTED` cannot report why it ended.
    pub fn owed(self) -> (JobStatus, Option<String>) {
        match self.failure {
            Some(message) => (JobStatus::Failed, Some(message)),
            None => (JobStatus::Completed, None),
        }
    }
}

/// The objects one export of `datastore` writes, by their path INSIDE the job's
/// own folder: one newline-delimited JSON file per resource type held, holding
/// every resource of that type.
///
/// Read off the data store's own records, in their held order, so the exported
/// bytes are the bytes a FHIR `read` of each resource answers.
pub fn exported_files(datastore: &Datastore) -> BTreeMap<String, String> {
    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ((resource_type, _), resource) in &datastore.fhir_resources {
        files
            .entry(resource_type.clone())
            .or_default()
            .push(resource.content.to_string());
    }
    files
        .into_iter()
        .map(|(resource_type, lines)| {
            (
                format!("{resource_type}/{resource_type}-0.ndjson"),
                lines.join("\n"),
            )
        })
        .collect()
}

/// Write `files` into the location `output` names, under the folder of the job
/// `job_id` names, and report what that run did.
///
/// Answers an [`ExportRun`] rather than an error channel: writing the output is
/// the JOB's own work, so what it could not do is the job's own terminal status
/// and `Message`, never a failure of the `StartFHIRExportJob` request that
/// submitted it.
pub async fn write_export(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    output: &OutputDataConfig,
    datastore_id: &str,
    job_id: &str,
    files: BTreeMap<String, String>,
) -> ExportRun {
    let OutputDataConfig::S3Configuration(configuration) = output;
    let Some((bucket, prefix)) = split_s3_uri(&configuration.s3_uri) else {
        // The member targets `S3Uri`, whose pattern the request boundary already
        // enforced, so this is unreachable; answering the job's own failure
        // keeps the run total.
        return ExportRun {
            failure: Some(EXPORT_NOT_WRITTEN_MESSAGE.to_string()),
        };
    };
    let client = s3_client(ctx, region);
    // The job's OWN FOLDER is written first, as the empty-bodied key ending in the
    // delimiter an object store represents a folder with — the same marker an
    // import job reads past. The export creates that folder under the location it
    // was given, so the location holds the job's output folder even for a data
    // store that held no resource to write into it.
    for (path, body) in ::std::iter::once((String::new(), String::new())).chain(files) {
        let key = output_key(&prefix, datastore_id, job_id, &path);
        // The write names no encryption member, so the object is stored under the
        // destination BUCKET's own encryption configuration — which is what
        // `S3Configuration$KmsKeyId` ("the KMS key ID used to access the S3
        // bucket") is about: it qualifies the destination the job was pointed at,
        // and the job reports it back on its own record.
        let mut put = client
            .put_object()
            .bucket(&bucket)
            .key(&key)
            .body(::aws_sdk_s3::primitives::ByteStream::from(
                body.into_bytes(),
            ));
        if !path.is_empty() {
            put = put.content_type(EXPORT_CONTENT_TYPE);
        }
        if let Err(error) = put.send().await {
            // Either the object store refused the write at this address, or
            // nothing answered it at all — and the second is worth a warning,
            // because a deployment with no Amazon S3 mounted to ask is one where
            // no export ever writes anything. The caller's own values are
            // rendered in their escaped `Debug` form, so a bucket or key carrying
            // a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                code = ?::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                "a HealthLake export job could not write its output in the emulator's Amazon S3"
            );
            return ExportRun {
                failure: Some(EXPORT_NOT_WRITTEN_MESSAGE.to_string()),
            };
        }
    }
    ExportRun { failure: None }
}

/// The object key one exported file is written at: the key prefix the supplied
/// location names, then the job's own folder, then the file's path inside it.
///
/// The supplied prefix is joined to the folder by exactly ONE `/` whether or not
/// it already ends in one, and a location naming the bucket root writes the
/// folder at the top of the bucket.
fn output_key(prefix: &str, datastore_id: &str, job_id: &str, path: &str) -> String {
    let folder = format!("{datastore_id}{EXPORT_JOB_OUTPUT_SEGMENT_INFIX}{job_id}");
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        format!("{folder}/{path}")
    } else {
        format!("{prefix}/{folder}/{path}")
    }
}
