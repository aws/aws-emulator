// SPDX-License-Identifier: CC0-1.0

//! What instant a job's `EndTime` reports, across every job record this service
//! holds.
//!
//! The rule under test: a timestamp a record reports for a transition the clock
//! owed it is the instant that transition FELL DUE — the instant it happened
//! whether or not anybody called — and never the instant of the call that
//! materialised it. So `ExportJobProperties$EndTime` ("the time the export job
//! completed"), `ImportJobProperties$EndTime` and
//! `TransformationJobProperties$EndTime` each report the due instant of the step
//! that reached the terminal status, whatever the polling delay before some
//! caller first read the record, and two jobs' `EndTime`s therefore order the
//! way the jobs really ended rather than the way they were read.
//!
//! The table below spans that class along the axes the rule generalises over:
//! the job KIND (all three records, so the describe of each reports the same
//! way), the terminal STATUS the clock owes (every terminal value the two
//! modelled status enums declare between them), the SPAN the transition was
//! armed for, and the DELAY between the instant it fell due and the first read —
//! the delay being exactly what a reported read-clock timestamp would drift
//! with. [`a_started_jobs_end_time_is_its_submit_time_plus_its_run_span`] then
//! drives the same rule through the operation that MINTS a record
//! (`StartFHIRExportJob`), where the run span is the service's own
//! `EXPORT_JOB_RUN`, and reads two jobs back in the REVERSE of the order they
//! ended.
//!
//! A started import, export or transformation job is armed with one fixed span,
//! so the rows are
//! seeded straight into the SAME store the handlers read, through the shared
//! engine the harness exposes — exactly how `tests/all/fhir_export_job_describe.rs`
//! and `tests/all/data_transformation_job.rs` seed their rows — which is what
//! lets one row differ from the next in the span and the status its pending
//! transition carries. Every describe itself goes over the real awsJson1_0 wire
//! through `MultiServiceRouter::dispatch`, so the reported timestamp is the
//! serialized one.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::ops::types::{
    DataTransformationS3Configuration, InputDataConfig, OutputDataConfig, S3Configuration,
    TransformationInputDataConfig, TransformationOutputDataConfig,
};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ExportJob, ImportJob, JobStatus, TransformationJob,
    TransformationJobStatus, EXPORT_JOB_RUN,
};

use crate::fhir_datastore_lifecycle::{HealthLake, SETTLING_DEADLINE};
use crate::fhir_export_job_start::{output_data_config, start_request};

/// The epoch second a seeded record's `SubmitTime` is measured from. Fixed, so
/// nothing a row asserts about `EndTime` can be satisfied by the submit instant
/// being near the present.
const BASE_SECS: i64 = 1_700_000_000;

/// How far a reported instant may sit from the instant it must report.
///
/// The conversion under test reads the monotonic and the wall clock in one
/// place, microseconds apart, so the only drift is between those two reads —
/// never the scheduling delay before a caller looked, which every row below
/// makes larger than this.
const TOLERANCE: Duration = Duration::from_millis(150);

/// The `DataAccessRoleArn` a seeded job carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeJobs";

/// Which record a row seeds, and the terminal status the clock owes it.
///
/// A FHIR export and a FHIR import job share the modelled `JobStatus` enum; a
/// data transformation job has its own `TransformationJobStatus`, which declares
/// no `CANCEL_*` value — so the owed status travels WITH the kind rather than as
/// a string either of them might not admit.
#[derive(Clone, Copy)]
enum Owed {
    Export(JobStatus),
    Import(JobStatus),
    Transformation(TransformationJobStatus),
}

impl Owed {
    /// The wire value the status this row is owed serializes as, which is what
    /// the describe must report once it has fallen due.
    fn wire(self) -> &'static str {
        match self {
            Self::Export(status) | Self::Import(status) => status.as_wire(),
            Self::Transformation(status) => status.as_wire(),
        }
    }
}

/// One row: a job record with a terminal transition the clock owes it, and when
/// the row is first read.
struct Row {
    case: &'static str,
    /// The `JobId` the row is seeded under. Satisfies both modelled job-id
    /// patterns (`JobId`, len 1..32, and `DataTransformationJobId`,
    /// `^[a-zA-Z0-9-]+$`).
    id: &'static str,
    owed: Owed,
    /// How long after the row is seeded its transition falls due.
    span: Duration,
    /// How long after it fell due the row is first read — the delay a timestamp
    /// stamped from the reader's clock would be wrong by.
    read_after_due: Duration,
}

/// The rows the table spans. Between them they cover every terminal value the
/// two modelled status enums declare (`JobStatus`: `COMPLETED`,
/// `COMPLETED_WITH_ERRORS`, `FAILED`, `CANCEL_COMPLETED`, `CANCEL_FAILED`;
/// `TransformationJobStatus`: the first three), all three job records, spans
/// from a fifth of a second to near a second, and reads from a third of a
/// second to two seconds after the transition fell due.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "export job, COMPLETED, short span, read long after",
            id: "export-completed",
            owed: Owed::Export(JobStatus::Completed),
            span: Duration::from_millis(200),
            read_after_due: Duration::from_millis(1_800),
        },
        Row {
            case: "export job, FAILED, long span, read soon after",
            id: "export-failed",
            owed: Owed::Export(JobStatus::Failed),
            span: Duration::from_millis(900),
            read_after_due: Duration::from_millis(350),
        },
        Row {
            case: "export job, CANCEL_COMPLETED, middling span",
            id: "export-cancelled",
            owed: Owed::Export(JobStatus::CancelCompleted),
            span: Duration::from_millis(500),
            read_after_due: Duration::from_millis(900),
        },
        Row {
            case: "import job, COMPLETED_WITH_ERRORS, short span, read long after",
            id: "import-partial",
            owed: Owed::Import(JobStatus::CompletedWithErrors),
            span: Duration::from_millis(200),
            read_after_due: Duration::from_millis(2_000),
        },
        Row {
            case: "import job, CANCEL_FAILED, long span",
            id: "import-cancel-failed",
            owed: Owed::Import(JobStatus::CancelFailed),
            span: Duration::from_millis(900),
            read_after_due: Duration::from_millis(600),
        },
        Row {
            case: "transformation job, COMPLETED, short span, read long after",
            id: "transformation-completed",
            owed: Owed::Transformation(TransformationJobStatus::Completed),
            span: Duration::from_millis(250),
            read_after_due: Duration::from_millis(1_500),
        },
        Row {
            case: "transformation job, COMPLETED_WITH_ERRORS, middling span, read soon after",
            id: "transformation-partial",
            owed: Owed::Transformation(TransformationJobStatus::CompletedWithErrors),
            span: Duration::from_millis(400),
            read_after_due: Duration::from_millis(400),
        },
        Row {
            case: "transformation job, FAILED, long span",
            id: "transformation-failed",
            owed: Owed::Transformation(TransformationJobStatus::Failed),
            span: Duration::from_millis(900),
            read_after_due: Duration::from_millis(1_000),
        },
    ]
}

/// `at` as the epoch seconds a reported timestamp carries.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A required timestamp member of a reported job, as epoch seconds.
fn timestamp(case: &str, properties: &Value, member: &str) -> f64 {
    properties
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: must report a {member}, got {properties}"))
}

/// Seed every row as a job record of `datastore_id`, armed to fall due its own
/// span after this call, and report the instant they were armed at on both
/// clocks.
fn seed(service: &HealthLake, datastore_id: &str, rows: &[Row]) -> (Instant, SystemTime) {
    let armed_at = Instant::now();
    let armed_wall = SystemTime::now();
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    for row in rows {
        let due = armed_at + row.span;
        match row.owed {
            Owed::Export(next) => {
                store.export_jobs.insert(
                    row.id.to_string(),
                    ExportJob {
                        id: row.id.to_string(),
                        datastore_id: datastore_id.to_string(),
                        name: None,
                        status: JobStatus::InProgress,
                        pending: Some((due, next, None)),
                        submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                        ended_at: None,
                        output_data_config: OutputDataConfig::S3Configuration(S3Configuration {
                            s3_uri: "s3://healthlake-export-bucket/outgoing".to_string(),
                            kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                        }),
                        data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                        message: None,
                        client_token: None,
                        request_fingerprint: String::new(),
                    },
                );
            }
            Owed::Import(next) => {
                store.import_jobs.insert(
                    row.id.to_string(),
                    ImportJob {
                        id: row.id.to_string(),
                        datastore_id: datastore_id.to_string(),
                        name: None,
                        status: JobStatus::InProgress,
                        pending: Some((due, next, None)),
                        submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                        ended_at: None,
                        input_data_config: InputDataConfig::S3Uri(
                            "s3://healthlake-import-input/incoming".to_string(),
                        ),
                        output_data_config: None,
                        data_access_role_arn: Some(DATA_ACCESS_ROLE_ARN.to_string()),
                        message: None,
                        progress_report: None,
                        validation_level: None,
                        // A record this suite seeds was not started through the
                        // API, so it carries no `ClientToken` and there is no
                        // request to fingerprint.
                        client_token: None,
                        request_fingerprint: String::new(),
                    },
                );
            }
            Owed::Transformation(next) => {
                store.transformation_jobs.insert(
                    row.id.to_string(),
                    TransformationJob {
                        id: row.id.to_string(),
                        name: None,
                        status: TransformationJobStatus::InProgress,
                        pending: Some((due, next, None)),
                        submitted_at: ::aws_smithy_types::DateTime::from_secs(BASE_SECS),
                        ended_at: None,
                        input_data_config: TransformationInputDataConfig {
                            s3_uri: "s3://healthlake-transform-input/ccda".to_string(),
                            source_format: Some("CCDA".to_string()),
                        },
                        output_data_config: TransformationOutputDataConfig {
                            s3_configuration: DataTransformationS3Configuration {
                                s3_uri: "s3://healthlake-transform-output/fhir".to_string(),
                                kms_key_id: "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
                            },
                        },
                        data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                        profile_id: None,
                        profile_name: None,
                        profile_version: None,
                        drift_detection_enabled: None,
                        provenance_enabled: None,
                        message: None,
                        progress_report: None,
                        client_token: None,
                    },
                );
            }
        }
    }
    (armed_at, armed_wall)
}

/// What the describe of `row`'s kind reports for it.
async fn describe(service: &HealthLake, datastore_id: &str, row: &Row) -> Value {
    match row.owed {
        Owed::Export(_) => service
            .ok(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": datastore_id, "JobId": row.id }),
            )
            .await["ExportJobProperties"]
            .clone(),
        Owed::Import(_) => service
            .ok(
                "DescribeFHIRImportJob",
                &json!({ "DatastoreId": datastore_id, "JobId": row.id }),
            )
            .await["ImportJobProperties"]
            .clone(),
        Owed::Transformation(_) => service
            .ok("DescribeDataTransformationJob", &json!({ "JobId": row.id }))
            .await["TransformationJobProperties"]
            .clone(),
    }
}

/// Look at `DescribeFHIRExportJob` until the export job has ENDED — a job that
/// reached a terminal status is the one that reports an `EndTime` — and answer that
/// record; fail once [`SETTLING_DEADLINE`] has passed without it.
///
/// A transition this service owes is materialised by whichever call reads the record
/// next, so waiting for one means LOOKING again until the condition holds, never
/// sleeping for the span — which would make the outcome depend on how fast the host
/// runs.
async fn poll_until_export_ended(
    service: &HealthLake,
    datastore_id: &str,
    job_id: &str,
) -> Value {
    let deadline = Instant::now() + SETTLING_DEADLINE;
    loop {
        let properties = service
            .ok(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
            )
            .await["ExportJobProperties"]
            .clone();
        if properties.get("EndTime").is_some() {
            return properties;
        }
        assert!(
            Instant::now() < deadline,
            "the export job {job_id} did not end within {SETTLING_DEADLINE:?}, \
             last read {properties}"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn end_time_is_the_instant_the_owed_transition_fell_due_not_the_instant_of_the_read() {
    let service = HealthLake::new();
    let (datastore_id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "job clock store" }))
        .await;

    let mut rows = rows();
    let (armed_at, armed_wall) = seed(&service, &datastore_id, &rows);
    // Read in the order the rows come due and are then waited out, so a record
    // settled by an EARLIER row's describe (every walk settles every record of
    // its kind that is already due) is still asserted against its own due
    // instant.
    rows.sort_by_key(|row| row.span + row.read_after_due);

    for row in &rows {
        let case = row.case;
        let target = armed_at + row.span + row.read_after_due;
        let now = Instant::now();
        if target > now {
            tokio::time::sleep(target - now).await;
        }

        let read_wall = SystemTime::now();
        let properties = describe(&service, &datastore_id, row).await;
        assert_eq!(
            properties["JobStatus"],
            json!(row.owed.wire()),
            "case {case:?}: the owed terminal status must be reported once due, got {properties}"
        );

        // The instant the transition fell due: the span it was armed for after
        // the instant it was armed, on the wall clock a timestamp is reported
        // on.
        let expected = epoch_secs(armed_wall + row.span);
        let reported = timestamp(case, &properties, "EndTime");
        assert!(
            (reported - expected).abs() <= TOLERANCE.as_secs_f64(),
            "case {case:?}: EndTime must report the instant the job ended ({expected}), got {reported}"
        );
        // ... which is NOT the instant of this read, however long after the
        // transition fell due it happened.
        let behind_the_read = epoch_secs(read_wall) - reported;
        assert!(
            behind_the_read >= row.read_after_due.as_secs_f64() - TOLERANCE.as_secs_f64(),
            "case {case:?}: EndTime must sit about {:?} before a read that late, got {behind_the_read}s before it",
            row.read_after_due
        );

        // Written once: a later read of the same record reports the same
        // instant rather than its own clock.
        let again = describe(&service, &datastore_id, row).await;
        assert_eq!(
            again["EndTime"], properties["EndTime"],
            "case {case:?}: two reads of one ended job must report the same EndTime, got {again} after {properties}"
        );
    }
}

#[tokio::test]
async fn a_started_jobs_end_time_is_its_submit_time_plus_its_run_span() {
    // A started export WRITES its output, so the object store it writes through is
    // mounted and its destination bucket created: the rule under test is the
    // instant the job ended, which is reported for a job that did its work.
    let service = HealthLake::with_object_store();
    let created_bucket = service
        .s3()
        .create_bucket()
        .bucket("healthlake-export-bucket")
        .send()
        .await;
    assert!(
        created_bucket.is_ok(),
        "Amazon S3 must create the destination bucket, got {created_bucket:?}"
    );
    let (datastore_id, _) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "exporting store" }))
        .await;
    // The data store is exported from once it is usable, so the wait is for THAT
    // condition rather than for a span: the harness looks again until it holds.
    service.poll_until_status(&datastore_id, "ACTIVE").await;

    // Two jobs started a known gap apart, so the one started first is also the
    // one that ends first: both are armed for the same span. The gap is the FIRST
    // job's own run — the second start waits for that job to end rather than for a
    // fixed time — so it is at least the run span whatever the host's speed.
    let mut started: Vec<(&str, String)> = Vec::new();
    for case in ["first started", "second started"] {
        if let Some((_, earlier)) = started.last() {
            poll_until_export_ended(&service, &datastore_id, earlier).await;
        }
        let response = service
            .ok(
                "StartFHIRExportJob",
                &start_request(
                    &datastore_id,
                    Some(case),
                    &output_data_config("s3://healthlake-export-bucket/clock", "export-key_1"),
                ),
            )
            .await;
        let job_id = response["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a start must answer a JobId, got {response}"))
            .to_string();
        started.push((case, job_id));
    }

    // Both jobs have to have ENDED before their `EndTime`s are read, which is a
    // condition each record reports for itself.
    for (_, job_id) in &started {
        poll_until_export_ended(&service, &datastore_id, job_id).await;
    }

    // Read in the REVERSE of the order they ended: whoever looks first cannot
    // decide what either job's EndTime is.
    let mut reported: Vec<(&str, f64, f64)> = Vec::new();
    for (case, job_id) in started.iter().rev() {
        let properties = service
            .ok(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": &datastore_id, "JobId": job_id }),
            )
            .await["ExportJobProperties"]
            .clone();
        assert_eq!(
            properties["JobStatus"],
            json!("COMPLETED"),
            "case {case:?}: a job whose output was written must settle COMPLETED, got {properties}"
        );
        let submit = timestamp(case, &properties, "SubmitTime");
        let end = timestamp(case, &properties, "EndTime");
        // The run span the record's own transition was armed for is what
        // separates the two, whenever the read happened.
        assert!(
            (end - submit - EXPORT_JOB_RUN.as_secs_f64()).abs() <= TOLERANCE.as_secs_f64(),
            "case {case:?}: EndTime must be SubmitTime plus the {EXPORT_JOB_RUN:?} run span, got {submit} and {end}"
        );
        reported.push((case, submit, end));
    }

    // The jobs ended in the order they were submitted, by the gap between the
    // submissions — not in the order they were read, which was the reverse.
    let (second_case, second_submit, second_end) = reported[0];
    let (first_case, first_submit, first_end) = reported[1];
    assert!(
        first_end < second_end,
        "{first_case:?} ended before {second_case:?}, so its EndTime must be earlier, got {first_end} and {second_end}"
    );
    assert!(
        ((second_end - first_end) - (second_submit - first_submit)).abs()
            <= TOLERANCE.as_secs_f64(),
        "the gap between the EndTimes must be the gap between the SubmitTimes, got {} and {}",
        second_end - first_end,
        second_submit - first_submit
    );
}
