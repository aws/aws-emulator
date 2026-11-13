// SPDX-License-Identifier: CC0-1.0

//! THE REPORTS of a bulk thing provisioning task: the objects a run's own records are written
//! into, and the link each one is reported under.
//!
//! `ListThingRegistrationTaskReports` reports `resourceLinks`, "Links to the task resources",
//! for one `reportType` — `RESULTS` or `ERRORS`
//! ([`crate::ops::types::REPORT_TYPE_VALUES`]) — as an `S3FileUrlList`: every link is the URL
//! of an Amazon S3 OBJECT, which is what a report of a task is. So the run this service
//! performs writes its records into real objects of the emulator's own Amazon S3
//! ([`write_report_objects`]), the task records where they went, and a reader of them hands
//! back a presigned URL per object — a caller that follows one, or that reads the bucket and
//! key out of it with `GetObject`, reads the very bytes this service wrote.
//!
//! What a report holds is the task's own run ([`super::thing_registration::RegistrationOutcome`])
//! and nothing else:
//!
//! * the `RESULTS` report holds the lines of the input file that provisioned a thing, and the
//!   thing each one provisioned;
//! * the `ERRORS` report holds the lines that provisioned none, with the reason that line was
//!   not provisioned, and — for a run that could not be performed at all — the reason no line
//!   of the file was read.
//!
//! A row is one line of the input file, written as newline delimited JSON, which is the form
//! the input file itself is written in: AWS publishes no layout for a report object, so what
//! one carries is the run's own records and nothing more. A big run is reported over SEVERAL
//! objects ([`REPORT_OBJECT_ROWS`] rows each), which is why `resourceLinks` is a list and why
//! the listing of it is paginated.
//!
//! The objects are written by the call that PERFORMS the run, as AWS IoT writes a task's
//! report while the task runs; a reader of them only reads, so two reads of one unchanged task
//! answer the same report.

use std::collections::BTreeMap;
use std::time::Duration;

use ::serde_json::json;

use super::thing_registration::{RegistrationOutcome, ThingRegistrationTask};

/// `ReportType`'s value for the report of what a run PROVISIONED
/// ([`crate::ops::types::REPORT_TYPE_VALUES`]).
pub const REPORT_TYPE_RESULTS: &str = "RESULTS";

/// `ReportType`'s value for the report of what a run could NOT provision.
pub const REPORT_TYPE_ERRORS: &str = "ERRORS";

/// Most rows one object of a report holds.
///
/// AWS IoT reports a task over as many objects as its run needs, which is what makes
/// `resourceLinks` a list; this is the emulator's own row count per object, so a run of
/// [`super::thing_registration::MAX_INPUT_FILE_LINES`] lines is reported over a handful of
/// them rather than one.
pub const REPORT_OBJECT_ROWS: usize = 100;

/// How long a link this service hands out stays valid.
///
/// AWS documents no lifetime for one, so this is the emulator's own: long enough for a caller
/// to follow a link it was just handed, and no lifetime of its own beyond that.
pub const REPORT_URL_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// The media type a report object is stored under: newline delimited JSON, which is what one
/// holds.
const REPORT_MEDIA_TYPE: &str = "application/x-ndjson";

/// The bucket of the emulator's own Amazon S3 that this service keeps its report objects in,
/// for `region`.
///
/// AWS IoT keeps them in a bucket of its OWN — the role a task names grants the service
/// `s3:GetObject` on the input file and nothing else — so the reports go into this service's
/// bucket rather than into the bucket the input file was staged in. Per-region, because a task
/// started in one Region is not a task of another.
#[must_use]
pub fn report_bucket(region: &str) -> String {
    format!("aws-iot-bulk-provisioning-reports-{region}")
}

/// The key object `index` of `report_type`'s report of the task `task_id` is held under.
///
/// Every segment comes from the task's own identity, so the same object is addressed by every
/// read of one unchanged run.
#[must_use]
pub fn report_object_key(task_id: &str, report_type: &str, index: usize) -> String {
    format!("{task_id}/{report_type}/{index}.json")
}

/// Where a task's report objects were written: the bucket they are in, and the key of each
/// object of each report, in the objects' own order.
///
/// A key is here only for an object the write PUT, so a link is reported only for a report
/// object the emulator's Amazon S3 actually holds.
#[derive(Debug, Clone, Default)]
pub struct ReportObjects {
    /// The bucket the objects are in ([`report_bucket`]), empty when none were written.
    pub bucket: String,
    /// The keys of each report's objects, keyed by `ReportType` value.
    pub keys: BTreeMap<String, Vec<String>>,
}

impl ReportObjects {
    /// The keys of `report_type`'s objects, in order.
    #[must_use]
    pub fn keys_of(&self, report_type: &str) -> &[String] {
        self.keys
            .get(report_type)
            .map_or(&[] as &[String], Vec::as_slice)
    }
}

/// The objects each of a run's two reports is written into: the report type, and the bytes of
/// each of its objects in order.
///
/// Computed from the run alone, so this is what a task's report holds before any of it has
/// been written anywhere.
#[must_use]
pub fn report_objects_of(outcome: &RegistrationOutcome) -> Vec<(&'static str, Vec<String>)> {
    [REPORT_TYPE_RESULTS, REPORT_TYPE_ERRORS]
        .into_iter()
        .map(|report_type| (report_type, objects_of(outcome, report_type)))
        .collect()
}

/// The bytes of each object of `report_type`'s report of `outcome`.
fn objects_of(outcome: &RegistrationOutcome, report_type: &str) -> Vec<String> {
    report_rows(outcome, report_type)
        .chunks(REPORT_OBJECT_ROWS)
        .map(|rows| {
            // Newline DELIMITED: every row is ended by its own newline, as the input file's
            // own lines are.
            let mut object = rows.join("\n");
            object.push('\n');
            object
        })
        .collect()
}

/// The rows of `report_type`'s report of `outcome`, one per line of the input file the report
/// is about.
fn report_rows(outcome: &RegistrationOutcome, report_type: &str) -> Vec<String> {
    match report_type {
        REPORT_TYPE_RESULTS => outcome
            .lines
            .iter()
            .filter_map(|line| {
                let thing_name = line.provisioned.as_ref().ok()?;
                Some(json!({ "lineNumber": line.line_number, "thingName": thing_name }).to_string())
            })
            .collect(),
        REPORT_TYPE_ERRORS => {
            let mut rows: Vec<String> = outcome
                .lines
                .iter()
                .filter_map(|line| {
                    let reason = line.provisioned.as_ref().err()?;
                    Some(
                        json!({ "lineNumber": line.line_number, "errorMessage": reason })
                            .to_string(),
                    )
                })
                .collect();
            // A run that could not be performed at all read no line, so it has no per-line
            // row: what it reports is the one reason the file was never read.
            if let Some(message) = &outcome.failure_message {
                rows.push(json!({ "errorMessage": message }).to_string());
            }
            rows
        }
        // `reportType` is a modelled enum, refused outside its value set before any handler
        // runs, so no other value reaches a report.
        _ => Vec::new(),
    }
}

/// Write `objects` into this service's own bucket of the emulator's Amazon S3, and answer
/// where they went.
///
/// Nothing here opens a socket or leaves the host: the client rides the operation callback,
/// the emulator's in-process channel into its own services — the same channel the task's input
/// file is read over.
///
/// A report whose object the store did not take is reported by no link: only what Amazon S3
/// ANSWERED is recorded, so a link this service hands out always names an object that store
/// holds.
pub async fn write_report_objects(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    task_id: &str,
    objects: Vec<(&'static str, Vec<String>)>,
) -> ReportObjects {
    let bucket = report_bucket(region);
    let client = super::thing_registration::s3_client(ctx, region);
    // The service's own bucket, created on first use: it is this emulator's, not a caller's,
    // so nothing but this write creates it. A bucket this account already holds answers an
    // error that says exactly that, which is not a reason to report no object.
    let mut create = client.create_bucket().bucket(&bucket);
    // A bucket is created in the Region the request is sent to, which every Region but
    // `us-east-1` states as a location constraint; `us-east-1` is the one Region a
    // `CreateBucket` names no constraint for ("Buckets in Region us-east-1 have a
    // LocationConstraint of null", the Amazon S3 model).
    if region != US_EAST_1 {
        create = create.create_bucket_configuration(
            ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(region))
                .build(),
        );
    }
    let created = create.send().await;
    if let Err(error) = &created {
        ::tracing::debug!(
            bucket = ?bucket,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "the bulk thing provisioning report bucket was not created by this task"
        );
    }
    let mut written = ReportObjects {
        bucket: bucket.clone(),
        keys: BTreeMap::new(),
    };
    for (report_type, report) in objects {
        for (index, object) in report.into_iter().enumerate() {
            let key = report_object_key(task_id, report_type, index);
            let put = client
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .content_type(REPORT_MEDIA_TYPE)
                .body(::aws_sdk_s3::primitives::ByteStream::from(
                    object.into_bytes(),
                ))
                .send()
                .await;
            match put {
                Ok(_) => written
                    .keys
                    .entry(report_type.to_string())
                    .or_default()
                    .push(key),
                Err(error) => ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a bulk thing provisioning report object was not stored in the emulator's \
                     Amazon S3"
                ),
            }
        }
    }
    written
}

/// The link to each object of `report_type`'s report of `task` — `resourceLinks`, "Links to
/// the task resources".
///
/// One link per object the report holds, in the objects' own order, so the Nth link is the Nth
/// object of that report.
#[must_use]
pub fn report_links(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    task: &ThingRegistrationTask,
    report_type: &str,
) -> Vec<String> {
    task.reports
        .keys_of(report_type)
        .iter()
        .map(|key| report_link(ctx, &task.reports.bucket, key))
        .collect()
}

/// One link: the object's own Amazon S3 address, presigned against the base URL this emulator
/// is reached on.
///
/// Signed through the framework's URL signer, which is what makes the link both a presigned
/// one and one this emulator answers: it signs for the emulator's current base URL, so the
/// host it names is the host the caller reached this emulator on rather than a host only AWS
/// serves, and the object's bucket and key travel in the PATH, the way a path-style Amazon S3
/// URL carries them.
fn report_link(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    bucket: &str,
    key: &str,
) -> String {
    let path = format!("/{bucket}/{key}");
    let request = ::smithy_server_runtime::SignedUrlRequest::new(
        ::smithy_server_runtime::http::Method::GET,
        S3_SIGNING_NAME,
        &path,
    );
    match ctx.url_signer().sign(request, REPORT_URL_EXPIRY) {
        Ok(signed) => signed.as_str().to_string(),
        // Nothing about a stored bucket, a stored key or this emulator's own endpoint makes a
        // request unsignable, so this is unreachable in practice; the object is still held at
        // that address, so the address is what is reported rather than no link at all. The
        // endpoint is `scheme://host[:port]` with no path of its own, and a trailing separator
        // is dropped so the object's path is the only one the link carries.
        Err(_) => format!("{}{path}", ctx.endpoint().as_str().trim_end_matches('/')),
    }
}

/// The signing name of the service that SERVES a report object: a link is an Amazon S3 object
/// URL, so it is presigned for Amazon S3 and the emulator's own Amazon S3 answers it.
const S3_SIGNING_NAME: &str = "s3";

/// The one Region whose buckets are created with no location constraint at all.
const US_EAST_1: &str = "us-east-1";
