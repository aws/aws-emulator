// SPDX-License-Identifier: CC0-1.0

//! What `StartFHIRExportJob` mints, what `DescribeFHIRExportJob` reads back, and
//! what either refuses.
//!
//! The rules under test, each over a SET of inputs rather than one case:
//!
//! 1. a start MINTS one export job record on the data store it names, persisting
//!    every member the request carried, answers the minted `JobId` with the
//!    `SUBMITTED` status the record holds, and that ONE record is what
//!    `DescribeFHIRExportJob` and `ListFHIRExportJobs` both report;
//! 2. the start WRITES: the job's own folder and the data store's FHIR resources
//!    go into the Amazon S3 location `OutputDataConfig` names, one
//!    newline-delimited JSON file per resource type inside that folder, and the
//!    clock then settles the job `COMPLETED` — with `EndTime` stamped once, when
//!    the job ends; a destination that cannot be written ends the job in the
//!    modelled `FAILED` with the `Message` saying so;
//! 3. only a settled, usable (`ACTIVE`) data store can be exported from: an
//!    unknown or `DELETED` one is `ResourceNotFoundException`, one in a
//!    lifecycle transition is `ValidationException`, and a refused start mints
//!    nothing;
//! 4. one `ClientToken` starts one export job: a repeat of the same request
//!    answers out of the job it started — whatever status that job has reached
//!    by then — and the same token with a different request is refused;
//! 5. a job is addressed WITHIN its data store: `DescribeFHIRExportJob` refuses
//!    a `JobId` the named data store holds no job for, and refuses one held for
//!    ANOTHER data store.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_rest_and_validate_source`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised on BOTH wires: the awsJson1_0 control plane and the FHIR REST
//! endpoint the exported data store serves.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    datastore_in_transition_message, DatastoreAction, DatastoreStatus,
    DATASTORE_NOT_FOUND_MESSAGE, EXPORT_JOB_NOT_FOUND_MESSAGE, EXPORT_NOT_WRITTEN_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, PAST_TRANSITION, SETTLING_DEADLINE};
use crate::fhir_rest_and_validate_source::{created, HealthLake};

/// The `DataAccessRoleArn` every start below carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeExport";

/// A well-formed `DatastoreId` no data store is held for: 32 lowercase hex
/// characters, the form this service mints, so the refusal below is about the
/// data store not existing rather than about the identifier's shape.
const UNHELD_DATASTORE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// Dispatch one control-plane call that must succeed, returning its body.
async fn ok(service: &HealthLake, operation: &str, body: &Value) -> Value {
    let (status, payload) = service.rpc(operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status}: {payload}"
    );
    payload
}

/// A required string member of a response body.
fn string_at(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("the response must carry a string {member}, got {body}"))
        .to_string()
}

/// The `ExportJobProperties` `DescribeFHIRExportJob` reports for one job.
async fn describe_job(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    ok(
        service,
        "DescribeFHIRExportJob",
        &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
    )
    .await["ExportJobProperties"]
        .clone()
}

/// Look at `DescribeFHIRExportJob` until the job has ENDED — a job that reached a
/// terminal status is the one that reports an `EndTime` — and answer that record;
/// fail once [`SETTLING_DEADLINE`] has passed without it.
///
/// A transition this service owes is materialised by whichever call reads the
/// record next, so waiting for one means LOOKING again until the condition holds,
/// never sleeping for the span — which would make the outcome depend on how fast
/// the host runs.
async fn poll_until_job_ended(
    service: &HealthLake,
    datastore_id: &str,
    job_id: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLING_DEADLINE;
    loop {
        let described = describe_job(service, datastore_id, job_id).await;
        if described.get("EndTime").is_some() {
            return described;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the export job {job_id} did not end within {SETTLING_DEADLINE:?}, \
             last read {described}"
        );
        tokio::task::yield_now().await;
    }
}

/// Every export job `ListFHIRExportJobs` reports for a data store.
async fn listed_jobs(service: &HealthLake, datastore_id: &str) -> Vec<Value> {
    ok(
        service,
        "ListFHIRExportJobs",
        &json!({ "DatastoreId": datastore_id }),
    )
    .await["ExportJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One `OutputDataConfig` the modelled union admits, built from the member
/// bounds `smithy_query(query="shape", shape="S3Configuration")` states:
/// `S3Uri` matches `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` (len
/// ..1024) and `KmsKeyId` matches the `EncryptionKeyID` pattern (a bare key id,
/// an `alias/...`, or a full KMS ARN; len 1..400).
///
/// `pub(crate)` beside [`start_request`] so the sibling suites of this test
/// target build a start the same way rather than each growing its own copy.
pub(crate) fn output_data_config(s3_uri: &str, kms_key_id: &str) -> Value {
    json!({ "S3Configuration": { "S3Uri": s3_uri, "KmsKeyId": kms_key_id } })
}

/// A `StartFHIRExportJob` request body for `datastore_id`.
///
/// `pub(crate)` so the sibling suites of this test target send the same start
/// this one does: `tests/all/idempotency_replay_precedes_state_gates.rs` repeats
/// one of these under one `ClientToken`.
pub(crate) fn start_request(datastore_id: &str, job_name: Option<&str>, output: &Value) -> Value {
    let mut request = json!({
        "DatastoreId": datastore_id,
        "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
        "OutputDataConfig": output,
    });
    if let Some(name) = job_name {
        request["JobName"] = json!(name);
    }
    request
}

/// `JobId` values are 32 lowercase hex characters, which satisfies the modelled
/// `JobId` length bound (1..32) and its alphabet.
fn assert_well_formed_job_id(case: &str, job_id: &str) {
    assert_eq!(
        job_id.len(),
        32,
        "case {case:?}: JobId must be 32 characters, got {job_id:?}"
    );
    assert!(
        job_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: JobId must be lowercase hex, got {job_id:?}"
    );
}

/// The start requests the round-trip table spans: the rule is that every member
/// a start carries is persisted and reported back, so the rows differ along
/// whether the optional `JobName` is set and along the FORM of the values the
/// modelled shapes admit — a bucket-root `S3Uri` against a prefixed and a nested
/// one, and each of the three `KmsKeyId` forms the modelled pattern spells
/// (bare key id, `alias/...`, full ARN).
fn round_trip_rows() -> Vec<(&'static str, Option<String>, Value)> {
    vec![
        (
            "no JobName, bucket-root S3Uri, bare key id",
            None,
            output_data_config(
                "s3://healthlake-export-bucket",
                "1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        ),
        (
            "ASCII JobName, prefixed S3Uri, key ARN",
            Some("nightly-export".to_string()),
            output_data_config(
                "s3://healthlake-export-bucket/nightly",
                "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        ),
        (
            "unicode JobName with spaces, nested S3Uri, key alias",
            Some("выгрузка 42".to_string()),
            output_data_config(
                "s3://healthlake-export-bucket/tenant/42/out",
                "alias/healthlake/export",
            ),
        ),
        (
            "JobName at the modelled 64-character maximum",
            Some("n".repeat(64)),
            output_data_config("s3://healthlake-export-bucket/max", "export-key_1"),
        ),
    ]
}

#[tokio::test]
async fn a_start_mints_one_job_and_describe_reports_every_member_back() {
    let service = HealthLake::new();
    let (datastore_id, _path) = service.active_datastore("exporting store").await;
    let mut minted: Vec<String> = Vec::new();

    for (case, job_name, output) in round_trip_rows() {
        let request = start_request(&datastore_id, job_name.as_deref(), &output);
        let started = ok(&service, "StartFHIRExportJob", &request).await;

        let job_id = string_at(&started, "JobId");
        assert_well_formed_job_id(case, &job_id);
        assert_eq!(
            started["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: a started job must be answered SUBMITTED, got {started}"
        );
        assert_eq!(
            started["DatastoreId"],
            json!(datastore_id),
            "case {case:?}: the start must report the data store it exports from, got {started}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every start must mint its own JobId, {job_id:?} was minted before"
        );
        minted.push(job_id.clone());

        // The record is read back through the operation that reports ONE job.
        let described = describe_job(&service, &datastore_id, &job_id).await;
        for (member, expected) in [
            ("JobId", json!(job_id)),
            ("JobStatus", json!("SUBMITTED")),
            ("DatastoreId", json!(datastore_id)),
            ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
            ("OutputDataConfig", output.clone()),
        ] {
            assert_eq!(
                described[member], expected,
                "case {case:?}: the described job must report {member} as {expected}, got {described}"
            );
        }
        assert_eq!(
            described.get("JobName").cloned(),
            job_name.clone().map(Value::from),
            "case {case:?}: the described job must report the JobName the start carried, got {described}"
        );
        assert!(
            described["SubmitTime"].is_number(),
            "case {case:?}: the described job must report a SubmitTime, got {described}"
        );
        assert!(
            described.get("EndTime").is_none(),
            "case {case:?}: a job that has not ended must report no EndTime, got {described}"
        );
        assert!(
            described.get("Message").is_none(),
            "case {case:?}: a job that has not ended must report no Message, got {described}"
        );

        // ONE record: what the listing reports for this job is exactly what the
        // describe reported.
        let jobs = listed_jobs(&service, &datastore_id).await;
        let listed = jobs
            .iter()
            .find(|job| job["JobId"] == json!(job_id))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must carry {job_id:?}, got {jobs:?}"));
        assert_eq!(
            listed, &described,
            "case {case:?}: the listing and the describe must report the SAME record"
        );
        assert_eq!(
            jobs.len(),
            minted.len(),
            "case {case:?}: each start must add exactly one job, got {jobs:?}"
        );
    }
}

/// One settle row: the resources the exported data store holds, where the export
/// writes, and what the destination must hold afterwards.
struct SettleRow {
    case: &'static str,
    /// `(resourceType, marker)` of each FHIR resource created on the data store
    /// before the export starts.
    resources: Vec<(&'static str, &'static str)>,
    /// The `S3Uri` the export writes under.
    s3_uri: String,
    /// The key prefix a client lists the output under, derived from the location
    /// above as a caller would write it.
    listed_prefix: &'static str,
    /// Whether the destination bucket exists at all.
    bucket_held: bool,
}

/// The bucket the export rows below write into, created through Amazon S3's own
/// `CreateBucket`, and one no bucket is ever created for.
const EXPORT_BUCKET: &str = "healthlake-export-bucket";
const UNHELD_BUCKET: &str = "healthlake-bucket-that-was-never-created";

/// The settle table. The rule is that a started export WRITES the data store's
/// FHIR resources into the location it was given and then settles `COMPLETED`,
/// so the rows differ along how many resources the store holds and of how many
/// types, along the FORM of the supplied location (bucket root, a prefix, a
/// nested prefix already ending in `/`) and along whether the destination can be
/// written at all.
fn settle_rows() -> Vec<SettleRow> {
    vec![
        SettleRow {
            case: "a data store holding no resource",
            resources: Vec::new(),
            s3_uri: format!("s3://{EXPORT_BUCKET}/empty"),
            listed_prefix: "empty/",
            bucket_held: true,
        },
        SettleRow {
            case: "one resource, the bucket root as the location",
            resources: vec![("Patient", "root")],
            s3_uri: format!("s3://{EXPORT_BUCKET}"),
            listed_prefix: "",
            bucket_held: true,
        },
        SettleRow {
            case: "several resources of one type, a prefixed location",
            resources: vec![
                ("Patient", "one"),
                ("Patient", "two"),
                ("Patient", "three"),
            ],
            s3_uri: format!("s3://{EXPORT_BUCKET}/while-active"),
            listed_prefix: "while-active/",
            bucket_held: true,
        },
        SettleRow {
            case: "resources of several types, a nested location already ending in a slash",
            resources: vec![
                ("Patient", "mixed"),
                ("Observation", "mixed"),
                ("Condition", "mixed"),
                ("Observation", "mixed-again"),
            ],
            s3_uri: format!("s3://{EXPORT_BUCKET}/tenant/42/out/"),
            listed_prefix: "tenant/42/out/",
            bucket_held: true,
        },
        SettleRow {
            case: "a destination bucket no object store holds",
            resources: vec![("Patient", "unwritable")],
            s3_uri: format!("s3://{UNHELD_BUCKET}/out"),
            listed_prefix: "out/",
            bucket_held: false,
        },
    ]
}

/// Every object under `prefix` of `bucket`, as `(key, body)`, read through
/// Amazon S3's own operations — so what is asserted is what the object store
/// really holds.
async fn written_objects(
    service: &HealthLake,
    bucket: &str,
    prefix: &str,
) -> Vec<(String, String)> {
    let listed = service
        .s3()
        .list_objects_v2()
        .bucket(bucket)
        .prefix(prefix)
        .send()
        .await;
    let Ok(listed) = listed else {
        return Vec::new();
    };
    let mut objects = Vec::new();
    for object in listed.contents() {
        let Some(key) = object.key() else { continue };
        let got = service
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("Amazon S3 must answer the object {key}: {error:?}"));
        let body = got
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("the object {key} must be readable: {error:?}"))
            .into_bytes();
        objects.push((
            key.to_string(),
            String::from_utf8_lossy(body.as_ref()).to_string(),
        ));
    }
    objects
}

#[tokio::test]
async fn the_clock_settles_a_started_job_once_its_output_is_written() {
    let service = HealthLake::with_object_store();
    let created_bucket = service
        .s3()
        .create_bucket()
        .bucket(EXPORT_BUCKET)
        .send()
        .await;
    assert!(
        created_bucket.is_ok(),
        "Amazon S3 must create the destination bucket, got {created_bucket:?}"
    );

    for (index, row) in settle_rows().into_iter().enumerate() {
        let case = row.case;
        // The modelled `DatastoreName` pattern admits no comma, so the store is
        // named for the row's position rather than for its sentence.
        let (datastore_id, path) = service
            .active_datastore(&format!("exporting store {index}"))
            .await;
        for (resource_type, marker) in &row.resources {
            created(
                &service,
                &path,
                resource_type,
                &json!({ "resourceType": resource_type, "language": marker }),
            )
            .await;
        }

        let started = ok(
            &service,
            "StartFHIRExportJob",
            &start_request(
                &datastore_id,
                Some("settling"),
                &output_data_config(&row.s3_uri, "export-key_1"),
            ),
        )
        .await;
        let job_id = string_at(&started, "JobId");

        // Before the span is up the job still reports the status it was started
        // with, so a caller describing it straight away sees the transient one.
        let pending = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            pending["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: before the transition falls due the job must report SUBMITTED, got {pending}"
        );
        assert!(
            pending.get("EndTime").is_none() && pending.get("Message").is_none(),
            "case {case:?}: a running job must report neither EndTime nor Message, got {pending}"
        );

        // The job ENDS when the transition the clock owes it falls due, which is a
        // condition the record reports for itself: the wait looks again until it
        // holds rather than sleeping for the span.
        let settled = poll_until_job_ended(&service, &datastore_id, &job_id).await;
        let (expected_status, expected_message) = if row.bucket_held {
            ("COMPLETED", None)
        } else {
            ("FAILED", Some(EXPORT_NOT_WRITTEN_MESSAGE.to_string()))
        };
        assert_eq!(
            settled["JobStatus"],
            json!(expected_status),
            "case {case:?}: once due the job must report {expected_status:?}, got {settled}"
        );
        assert_eq!(
            settled.get("Message").and_then(Value::as_str),
            expected_message.as_deref(),
            "case {case:?}: the ended job must report {expected_message:?}, got {settled}"
        );
        assert!(
            settled["EndTime"].is_number(),
            "case {case:?}: a job that reached a terminal status must report an EndTime, got {settled}"
        );

        // What the destination HOLDS: one newline-delimited file per resource
        // type the data store held, in a folder naming this job, each line a
        // resource of that type — and nothing at all for a store that held
        // none, or for a destination that could not be written.
        let bucket = if row.bucket_held {
            EXPORT_BUCKET
        } else {
            UNHELD_BUCKET
        };
        let folder = format!(
            "{}{datastore_id}-FHIR_EXPORT-{job_id}/",
            row.listed_prefix
        );
        // Scoped to THIS job's folder, so one row's objects are not the next row's:
        // the folder key names the job that wrote it.
        let objects = written_objects(&service, bucket, &folder).await;
        // And the rule a client reads the output by: the location the start was
        // given holds what the job wrote.
        let under_supplied_prefix =
            written_objects(&service, bucket, row.listed_prefix).await;
        assert_eq!(
            under_supplied_prefix.is_empty(),
            !row.bucket_held,
            "case {case:?}: a completed export must leave its objects under the location it \
             was given, got {:?}",
            under_supplied_prefix
                .iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>()
        );
        let mut expected_types: Vec<&str> = row
            .resources
            .iter()
            .map(|(resource_type, _)| *resource_type)
            .collect();
        expected_types.sort_unstable();
        expected_types.dedup();
        if !row.bucket_held {
            // A destination that could not be written holds nothing at all.
            assert!(
                objects.is_empty(),
                "case {case:?}: a failed export must leave no object, got {:?}",
                objects.iter().map(|(key, _)| key).collect::<Vec<_>>()
            );
            expected_types.clear();
        } else {
            assert_eq!(
                objects.len(),
                expected_types.len() + 1,
                "case {case:?}: the export must write its own folder and one object per \
                 resource type held ({expected_types:?}), got {:?}",
                objects.iter().map(|(key, _)| key).collect::<Vec<_>>()
            );
            // The job's folder comes first in the listing, as the empty-bodied key
            // ending in the delimiter, so the location holds the folder the job
            // created even when the data store held nothing to export.
            assert_eq!(
                objects[0],
                (folder.clone(), String::new()),
                "case {case:?}: the export must write its own folder at {folder:?}, got {:?}",
                objects[0]
            );
        }
        for (resource_type, (key, body)) in expected_types.iter().zip(objects.iter().skip(1)) {
            let expected_key = format!("{folder}{resource_type}/{resource_type}-0.ndjson");
            assert_eq!(
                key, &expected_key,
                "case {case:?}: the exported object must be written at {expected_key:?}, got {key:?}"
            );
            let held = row
                .resources
                .iter()
                .filter(|(held_type, _)| held_type == resource_type)
                .count();
            let lines: Vec<&str> = body.lines().collect();
            assert_eq!(
                lines.len(),
                held,
                "case {case:?}: {key} must hold one line per {resource_type} held, got {body:?}"
            );
            for line in lines {
                let resource: Value = serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every exported line must be FHIR JSON ({error}): {line}")
                });
                assert_eq!(
                    resource["resourceType"],
                    json!(resource_type),
                    "case {case:?}: an exported line must carry its own resourceType, got {resource}"
                );
                assert!(
                    resource["id"].is_string(),
                    "case {case:?}: an exported resource carries the id it is stored under, \
                     got {resource}"
                );
            }
        }

        // `EndTime` is stored when the job ends, not re-derived per read, so a
        // second read reports the same instant — and the listing agrees.
        let again = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: two reads of one ended job must report the same record"
        );
        let jobs = listed_jobs(&service, &datastore_id).await;
        assert_eq!(
            jobs,
            vec![settled],
            "case {case:?}: the listing must report the settled record, got {jobs:?}"
        );
    }
}

/// Assert one `StartFHIRExportJob` refusal: the HTTP fault, the modelled shape
/// and the sentence it carries.
///
/// A helper rather than a table row built up front, because two of the rows
/// below are about a TRANSIENT data store status: the request has to go out
/// while the record still holds it, so each case is prepared and then refused
/// immediately.
async fn assert_start_refused(
    service: &HealthLake,
    case: &str,
    datastore_id: &str,
    expected_code: &str,
    expected_message: &str,
) {
    let (status, payload) = service
        .rpc(
            "StartFHIRExportJob",
            &start_request(
                datastore_id,
                None,
                &output_data_config("s3://healthlake-export-bucket/refused", "export-key_1"),
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the start must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        expected_code,
        "case {case:?}: the refusal must be {expected_code}, got {payload}"
    );
    assert_eq!(
        error_message(&payload),
        expected_message,
        "case {case:?}: the refusal must state {expected_message:?}, got {payload}"
    );
}

#[tokio::test]
async fn an_export_is_refused_for_a_data_store_that_cannot_serve_it() {
    let service = HealthLake::new();
    let (live, _path) = service.active_datastore("live store").await;

    // A DatastoreId no record is held for at all.
    assert_start_refused(
        &service,
        "a DatastoreId no data store is held for",
        UNHELD_DATASTORE_ID,
        "ResourceNotFoundException",
        DATASTORE_NOT_FOUND_MESSAGE,
    )
    .await;

    // A data store that has only just been created is still CREATING, so the
    // start goes out before its transition falls due.
    let creating = string_at(
        &ok(
            &service,
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "creating store" }),
        )
        .await,
        "DatastoreId",
    );
    assert_start_refused(
        &service,
        "a data store still CREATING",
        &creating,
        "ValidationException",
        &datastore_in_transition_message(DatastoreStatus::Creating, DatastoreAction::ExportFrom),
    )
    .await;

    // A data store that has only just been deleted is still DELETING, and once
    // that transition falls due the record is terminally DELETED: in transition
    // first, then gone.
    let (deleting, _path) = service.active_datastore("deleting store").await;
    ok(
        &service,
        "DeleteFHIRDatastore",
        &json!({ "DatastoreId": deleting }),
    )
    .await;
    assert_start_refused(
        &service,
        "a data store still DELETING",
        &deleting,
        "ValidationException",
        &datastore_in_transition_message(DatastoreStatus::Deleting, DatastoreAction::ExportFrom),
    )
    .await;

    tokio::time::sleep(PAST_TRANSITION).await;
    assert_start_refused(
        &service,
        "a DELETED data store",
        &deleting,
        "ResourceNotFoundException",
        DATASTORE_NOT_FOUND_MESSAGE,
    )
    .await;

    // Every refusal above minted nothing: the one live data store still holds no
    // export job.
    let jobs = listed_jobs(&service, &live).await;
    assert!(
        jobs.is_empty(),
        "a refused start must mint no job, got {jobs:?}"
    );
}

#[tokio::test]
async fn one_client_token_starts_one_export_job() {
    // A started export WRITES its output, so the object store it writes through is
    // mounted and its destination bucket created: the rule under test is that one
    // token starts one job, and the job the repeat is answered with is one that did
    // its work.
    let service = HealthLake::with_object_store();
    let created_bucket = service
        .s3()
        .create_bucket()
        .bucket(EXPORT_BUCKET)
        .send()
        .await;
    assert!(
        created_bucket.is_ok(),
        "Amazon S3 must create the destination bucket, got {created_bucket:?}"
    );
    let (datastore_id, _path) = service.active_datastore("idempotent store").await;
    let output = output_data_config(&format!("s3://{EXPORT_BUCKET}/idempotent"), "export-key_1");
    let mut request = start_request(&datastore_id, Some("hourly"), &output);
    request["ClientToken"] = json!("token-one");

    let first = ok(&service, "StartFHIRExportJob", &request).await;
    let job_id = string_at(&first, "JobId");

    // The SAME request with the SAME token resolves to the job it started.
    let repeat = ok(&service, "StartFHIRExportJob", &request).await;
    assert_eq!(
        repeat, first,
        "a repeat of one request under one ClientToken must answer the job it started"
    );
    let jobs = listed_jobs(&service, &datastore_id).await;
    assert_eq!(
        jobs.len(),
        1,
        "one ClientToken must start exactly one job, got {jobs:?}"
    );

    // The same token with a DIFFERENT request is a different request, and is
    // refused rather than answered with the earlier job. The rows differ along
    // which member differs.
    for (case, member, value) in [
        ("a different JobName", "JobName", json!("nightly")),
        (
            "a different DataAccessRoleArn",
            "DataAccessRoleArn",
            json!("arn:aws:iam::111122223333:role/OtherExport"),
        ),
        (
            "a different OutputDataConfig",
            "OutputDataConfig",
            output_data_config(&format!("s3://{EXPORT_BUCKET}/elsewhere"), "export-key_1"),
        ),
    ] {
        let mut different = request.clone();
        different[member] = value;
        let (status, payload) = service.rpc("StartFHIRExportJob", &different).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reused ClientToken must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: a reused ClientToken must answer ValidationException, got {payload}"
        );
    }

    // A DIFFERENT token starts a second job of the same request.
    let mut second_token = request.clone();
    second_token["ClientToken"] = json!("token-two");
    let second = ok(&service, "StartFHIRExportJob", &second_token).await;
    assert_ne!(
        string_at(&second, "JobId"),
        job_id,
        "a distinct ClientToken must start its own job, got {second}"
    );

    // Once the clock has settled the first job, its token still resolves to it —
    // answered out of the STORED record, so the status is the one the job has
    // reached rather than the one it started in. The wait is for the job to have
    // ENDED, which the record reports for itself.
    poll_until_job_ended(&service, &datastore_id, &job_id).await;
    let settled_repeat = ok(&service, "StartFHIRExportJob", &request).await;
    assert_eq!(
        settled_repeat["JobId"],
        json!(job_id),
        "the token must still resolve to the job it started, got {settled_repeat}"
    );
    assert_eq!(
        settled_repeat["JobStatus"],
        json!("COMPLETED"),
        "a repeat must report the status the stored job has reached, got {settled_repeat}"
    );
    let jobs = listed_jobs(&service, &datastore_id).await;
    assert_eq!(
        jobs.len(),
        2,
        "the repeats above must have started no further job, got {jobs:?}"
    );
}

#[tokio::test]
async fn a_job_is_described_only_within_the_data_store_that_holds_it() {
    let service = HealthLake::new();
    let (exporting, _path) = service.active_datastore("described store").await;
    let (other, _path) = service.active_datastore("other store").await;
    let output = output_data_config("s3://healthlake-export-bucket/described", "export-key_1");
    let started = ok(
        &service,
        "StartFHIRExportJob",
        &start_request(&exporting, Some("hourly"), &output),
    )
    .await;
    let job_id = string_at(&started, "JobId");

    // The rows differ along which half of the address is wrong: the data store,
    // the job, or neither (the job belongs to another data store).
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a DatastoreId no data store is held for",
            UNHELD_DATASTORE_ID.to_string(),
            job_id.clone(),
            "ResourceNotFoundException",
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a JobId the data store holds no job for",
            exporting.clone(),
            UNHELD_DATASTORE_ID.to_string(),
            "ResourceNotFoundException",
            EXPORT_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a job held for ANOTHER data store",
            other.clone(),
            job_id.clone(),
            "ResourceNotFoundException",
            EXPORT_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (case, datastore_id, asked_job_id, expected_code, expected_message) in rows {
        let (status, payload) = service
            .rpc(
                "DescribeFHIRExportJob",
                &json!({ "DatastoreId": datastore_id, "JobId": asked_job_id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the describe must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_code,
            "case {case:?}: the refusal must be {expected_code}, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {case:?}: the refusal must state {expected_message:?}, got {payload}"
        );
    }

    // The job IS described under the data store that holds it.
    let described = describe_job(&service, &exporting, &job_id).await;
    assert_eq!(
        described["JobId"],
        json!(job_id),
        "the job must be described under its own data store, got {described}"
    );
}
