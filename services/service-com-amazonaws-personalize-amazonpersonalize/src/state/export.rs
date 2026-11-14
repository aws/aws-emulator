// SPDX-License-Identifier: CC0-1.0

//! What a dataset EXPORT job does: the records it writes out of a dataset, the CSV it
//! renders them as, where in object storage it puts them, and the record
//! `DescribeDatasetExportJob` reports.
//!
//! # What is real here
//!
//! Amazon Personalize "exports data from your dataset to an Amazon S3 bucket"
//! (`CreateDatasetExportJob` doc), and which half of the dataset it exports is the
//! request's `ingestionMode`: `BULK` for "data that you imported using a dataset import
//! job", `PUT` for "data that you imported incrementally (using the console, PutEvents,
//! PutUsers and PutItems operations)", or `ALL` for both. This module does the same
//! thing against the emulator's own Amazon S3, in process through the framework's
//! callback — the same transport an import job reads through
//! ([`crate::state::import::s3_client`]) — so what a client later reads out of its
//! bucket is exactly the records this emulator holds, and a dataset holding none of the
//! requested kind exports nothing at all rather than an empty success.
//!
//! The lifecycle is the one the model documents for every create in this service
//! ("CREATE PENDING > CREATE IN_PROGRESS > ACTIVE -or- CREATE FAILED"), so the status a
//! describe reports is a function of the stored due instant and what the export actually
//! did ([`crate::state::terminal_status`]).

use std::collections::BTreeSet;
use std::time::SystemTime;

use ::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state as ingestion;

use crate::ops::types::{DatasetExportJob, DatasetExportJobOutput, S3DataConfig, Tag};
use crate::state::import::{self, Row, S3Location};
use crate::state::{ingested, terminal_status, timestamp_of, DatasetRecord};

// ---------------------------------------------------------------------------
// Ingestion modes
// ---------------------------------------------------------------------------

/// "Only `BULK` data that you imported using a dataset import job"
/// (`CreateDatasetExportJob$ingestionMode`).
pub const BULK_INGESTION_MODE: &str = "BULK";
/// "Only `PUT` data that you imported incrementally (using the console, PutEvents,
/// PutUsers and PutItems operations)" (same source).
pub const PUT_INGESTION_MODE: &str = "PUT";
/// "`ALL` for both types" (same source).
pub const ALL_INGESTION_MODE: &str = "ALL";

/// "The default value is `PUT`" (same source).
pub const DEFAULT_INGESTION_MODE: &str = PUT_INGESTION_MODE;

/// The values `IngestionMode` admits, in the order the model lists them.
pub const INGESTION_MODES: &[&str] = &[BULK_INGESTION_MODE, PUT_INGESTION_MODE, ALL_INGESTION_MODE];

/// Whether `mode` exports the rows a dataset import job read into the dataset.
pub fn exports_bulk(mode: &str) -> bool {
    mode == BULK_INGESTION_MODE || mode == ALL_INGESTION_MODE
}

/// Whether `mode` exports the records the ingestion data plane put into the dataset.
pub fn exports_put(mode: &str) -> bool {
    mode == PUT_INGESTION_MODE || mode == ALL_INGESTION_MODE
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// The most rows one export job writes.
///
/// The EMULATOR's own bound on the work one request does: a dataset's two halves are each
/// bounded ([`crate::state::import::MAX_DATASET_ROWS`] and the ingestion store's
/// per-collection and per-tracker retention), but a group's trackers are many, so without
/// a ceiling one request could render an unbounded CSV in memory. It is checked while the
/// rows are collected, so at most one row past it is ever held, and an export past it
/// FAILS naming the bound rather than writing a silently truncated file.
pub const MAX_EXPORT_ROWS: usize = 25_000;

/// The same bound for dataset export jobs as every other resource of this service keeps
/// (see [`crate::state::MAX_SCHEMAS`]).
pub const MAX_DATASET_EXPORT_JOBS: usize = 1_000;

// ---------------------------------------------------------------------------
// ARNs, records and output shapes
// ---------------------------------------------------------------------------

/// A dataset export job's ARN:
/// `arn:aws:personalize:<region>:<account>:dataset-export-job/<jobName>`.
///
/// Minted from the job's own name, exactly as every other resource of this service is, so
/// a duplicate create collides on an ARN that is already present.
pub fn dataset_export_job_arn(region: &str, job_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "personalize",
        region,
        &format!("dataset-export-job/{job_name}"),
    )
}

/// What `CreateDatasetExportJob` persists, and what `DescribeDatasetExportJob`
/// reconstructs a [`DatasetExportJob`] from.
#[derive(Debug, Clone)]
pub struct DatasetExportJobRecord {
    pub job_name: String,
    pub dataset_export_job_arn: String,
    pub dataset_arn: String,
    pub ingestion_mode: String,
    pub role_arn: String,
    /// The `jobOutput.s3DataDestination.path` the caller asked for, exactly as supplied.
    pub destination_path: String,
    /// Its `kmsKeyArn`, when the request named one.
    pub kms_key_arn: Option<String>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The instant the clock owes this job its terminal status.
    pub active_at: SystemTime,
    /// Why the export could not be carried out, for a job that ends `CREATE FAILED`.
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
}

/// The `DatasetExportJob` `DescribeDatasetExportJob` returns for a stored record.
pub fn dataset_export_job_of(record: &DatasetExportJobRecord) -> DatasetExportJob {
    let status = terminal_status(record.active_at, record.failure_reason.as_deref());
    DatasetExportJob {
        job_name: Some(record.job_name.clone()),
        dataset_export_job_arn: Some(record.dataset_export_job_arn.clone()),
        dataset_arn: Some(record.dataset_arn.clone()),
        ingestion_mode: Some(record.ingestion_mode.clone()),
        role_arn: Some(record.role_arn.clone()),
        status: Some(status.to_string()),
        job_output: Some(DatasetExportJobOutput {
            s3_data_destination: S3DataConfig {
                path: record.destination_path.clone(),
                kms_key_arn: record.kms_key_arn.clone(),
            },
        }),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        // "If a dataset export job fails, provides the reason why" — reported only once
        // the clock has made the failure due, so a job still in progress carries none.
        failure_reason: record
            .failure_reason
            .clone()
            .filter(|_| status == crate::state::STATUS_CREATE_FAILED),
    }
}

// ---------------------------------------------------------------------------
// Refusal and failure wording
// ---------------------------------------------------------------------------

/// What a create answers when the dataset holds none of the data the requested
/// `ingestionMode` names: the fixed sentence Amazon Personalize words it with, whose one
/// variable part is the mode the request asked for.
pub fn no_data_found_message(ingestion_mode: &str) -> String {
    format!("No data found in datasets for ingestionMode: {ingestion_mode}")
}

/// What a create answers for a `jobOutput` path naming no storage this emulator writes
/// to. `S3DataConfig$path`'s modelled pattern admits `http` and `https` too, and neither
/// names a bucket an export can be written into.
///
/// EMULATOR-AUTHORED wording: neither the model nor a recording carries Amazon
/// Personalize's own sentence for a `jobOutput` path outside Amazon S3, so this states the
/// fault in the class the model does declare for it (`InvalidInputException`, "provide a
/// valid value for the field or parameter") and names the member at fault. Replace it with
/// the service's text once a recording shows it.
pub fn invalid_destination_message(path: &str) -> String {
    format!(
        "The job output path {path} names no Amazon S3 bucket; Amazon Personalize exports \
         data to Amazon S3."
    )
}

/// Why an export of more rows than this emulator writes in one job failed.
pub fn too_many_rows_message(dataset_arn: &str) -> String {
    format!(
        "Dataset {dataset_arn} holds more than the {MAX_EXPORT_ROWS} records this emulator \
         exports in one job."
    )
}

// ---------------------------------------------------------------------------
// The export itself
// ---------------------------------------------------------------------------

/// The field names `dataset`'s own schema declares, in the order it declares them — the
/// COLUMNS of the exported file.
///
/// A dataset "is associated with a dataset type and has a set of required field and
/// keywords" through the schema `CreateDataset` named, and that schema is where each
/// column's spelling comes from: a caller reading an export back addresses a column by the
/// name it wrote in the schema, not by the canonical name this crate matches a field with
/// (case-insensitively) internally.
///
/// Empty for a dataset whose schema this store no longer holds; the records' own column
/// names stand then.
pub fn schema_fields(
    store: &crate::state::ComAmazonawsPersonalizeAmazonpersonalizeState,
    dataset: &DatasetRecord,
) -> Vec<String> {
    store
        .schemas
        .get(&dataset.schema_arn)
        .map(|schema| crate::state::declared_schema_fields(&schema.schema))
        .unwrap_or_default()
}

/// The name a column is exported under: the schema's own spelling of the field it names,
/// matched the way every field name in this crate is matched (case-insensitively), or the
/// column's own name for one the schema does not declare.
fn exported_column(schema_fields: &[String], column: &str) -> String {
    schema_fields
        .iter()
        .find(|field| field.eq_ignore_ascii_case(column))
        .cloned()
        .unwrap_or_else(|| column.to_string())
}

/// The rows `ingestion_mode` exports out of `dataset`: the bulk rows an import job read
/// into it, the records the ingestion data plane put into it, or both — each column named
/// as [`schema_fields`] spells it, so one field is one column however the half it came
/// from wrote it.
///
/// `Err` is the reason the export could not be carried out, ready for the job's failure
/// state. A pure read of both stores.
pub fn export_rows(
    store: &crate::state::ComAmazonawsPersonalizeAmazonpersonalizeState,
    ingested_store: Option<&ingestion::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState>,
    dataset: &DatasetRecord,
    ingestion_mode: &str,
) -> Result<Vec<Row>, String> {
    // One more than the ceiling is collected, and only ever one more: the length is what
    // says the store holds more rows than one job writes.
    let budget = MAX_EXPORT_ROWS.saturating_add(1);
    let mut rows: Vec<Row> = Vec::new();
    if exports_bulk(ingestion_mode) {
        rows.extend(dataset.imported.iter().take(budget).cloned());
    }
    if exports_put(ingestion_mode) && rows.len() < budget {
        rows.extend(ingested::put_rows(ingested_store, dataset, budget - rows.len()));
    }
    if rows.len() > MAX_EXPORT_ROWS {
        return Err(too_many_rows_message(&dataset.dataset_arn));
    }

    let schema_fields = schema_fields(store, dataset);
    Ok(rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(column, value)| (exported_column(&schema_fields, &column), value))
                .collect()
        })
        .collect())
}

/// The columns of the exported file, in the order its header row lists them.
///
/// Every column any exported row carries is emitted and nothing else: the dataset's schema
/// fields come first, in the order the schema declares them, and a column no field names
/// follows in a stable alphabetical order — so one dataset's export reads the same
/// whichever order its rows were stored in. A schema field no record carries a value for
/// is not a column of this export.
fn export_columns(schema_fields: &[String], rows: &[Row]) -> Vec<String> {
    let present: BTreeSet<&str> = rows
        .iter()
        .flat_map(|row| row.keys())
        .map(String::as_str)
        .collect();
    let mut columns: Vec<String> = present.into_iter().map(str::to_string).collect();
    // A stable sort by that rank keeps the alphabetical order the set already has within
    // each rank, so the ordering is total.
    columns.sort_by_key(|column| {
        schema_fields
            .iter()
            .position(|field| field == column)
            .unwrap_or(schema_fields.len())
    });
    columns
}

/// `rows` as the comma-delimited CSV Amazon Personalize exports, with a header row naming
/// the columns — the same format a dataset import job reads
/// ([`crate::state::import::read_rows`]), so an export can be imported back.
///
/// Rendered by the `csv` crate, so a value holding a comma, a quote or a newline is
/// quoted as RFC 4180 requires rather than corrupting the file.
pub fn render_csv(schema_fields: &[String], rows: &[Row]) -> Result<String, String> {
    let columns = export_columns(schema_fields, rows);
    let mut writer = ::csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    writer
        .write_record(&columns)
        .map_err(|error| format!("The export's header row could not be written: {error}"))?;
    for row in rows {
        writer
            .write_record(
                columns
                    .iter()
                    .map(|column| row.get(column).map(String::as_str).unwrap_or_default()),
            )
            .map_err(|error| format!("A record of the export could not be written: {error}"))?;
    }
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("The export could not be rendered: {error}"))?;
    String::from_utf8(bytes)
        .map_err(|_| "The records being exported are not UTF-8 text.".to_string())
}

/// The key the export object is written under, below the `path` the request named.
///
/// The path is a FOLDER — "The path to the Amazon S3 bucket where the job's output is
/// stored. For example: `s3://bucket-name/folder-name/`"
/// (`DatasetExportJob$jobOutput` doc) — so the job's own name is what names the file
/// inside it, and two jobs writing to one folder do not overwrite each other.
pub fn export_object_key(location: &S3Location, job_name: &str) -> String {
    let prefix = location.key.trim_start_matches('/');
    if prefix.is_empty() {
        format!("{job_name}.csv")
    } else if prefix.ends_with('/') {
        format!("{prefix}{job_name}.csv")
    } else {
        format!("{prefix}/{job_name}.csv")
    }
}

/// The content type the exported object is stored with: it is CSV text.
const EXPORT_CONTENT_TYPE: &str = "text/csv";

/// Write `body` to `location`'s bucket under `key`, and report the `s3://` path it
/// landed at.
///
/// `Err` is the reason the export could not be written, for the job's failure state: a
/// write that did not happen is never reported as an export that did.
pub async fn write_export(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
    key: &str,
    body: String,
) -> Result<String, String> {
    let client = import::s3_client(ctx, region);
    client
        .put_object()
        .bucket(&location.bucket)
        .key(key)
        .content_type(EXPORT_CONTENT_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            body.into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "Could not write the export to s3://{}/{key}: {}",
                location.bucket,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error),
            )
        })?;
    Ok(format!("s3://{}/{key}", location.bucket))
}
