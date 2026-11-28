// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartUploadJob`: the processing a start commissions, the profiles
//! it ingests, and the states a job can be started from.
//!
//! The rules under test:
//!
//! 1. `StartUploadJob` "starts the processing of an upload job to ingest profile data": for
//!    a stored job in the one status processing starts from, it answers 200 and the job
//!    reports `IN_PROGRESS` until its processing falls due, then its terminal
//!    `UploadJobStatus`, its `CompletedAt` and a `ResultsSummary` that accounts for every
//!    record of the file written to the job's upload target — the records that created a
//!    profile, the records that updated one the job's `UniqueKey` columns matched, and the
//!    records that could not be processed. The profiles those records map are in the
//!    domain, so a `SearchProfiles` for a value the file carried finds them.
//! 2. Processing is started once: a job in any other status is refused with
//!    `BadRequestException` (400) and the service's own sentence for that state, and a
//!    domain or job that is not stored with `ResourceNotFoundException` (404) — neither
//!    changing what the job reports.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use md5::{Digest, Md5};
use serde_json::{json, Value};
use std::time::Duration;

use crate::domain_profile_lifecycle::{error_code, error_message, item_ids, Emulator};

/// The `UploadJobStatus` a job carries until a call starts it: "CREATED: The upload job has
/// been created, but has not started processing yet."
const CREATED: &str = "CREATED";

/// The `UploadJobStatus` of a job whose processing is under way: "IN_PROGRESS: The upload
/// job is currently in progress, ingesting and processing the profile data."
const IN_PROGRESS: &str = "IN_PROGRESS";

/// The `UploadJobStatus` of a job that ingested every record it read.
const SUCCEEDED: &str = "SUCCEEDED";

/// The `UploadJobStatus` of a job that ingested some of its records and failed others.
const PARTIALLY_SUCCEEDED: &str = "PARTIALLY_SUCCEEDED";

/// The `UploadJobStatus` of a job that ingested nothing: "FAILED: The upload job has failed
/// to complete."
const FAILED: &str = "FAILED";

/// The `StatusReason` of a job whose data could not be ingested: "VALIDATION_FAILURE: The
/// upload job has encountered an error or issue and was unable to complete the profile data
/// ingestion."
const VALIDATION_FAILURE: &str = "VALIDATION_FAILURE";

/// The sentence the service refuses a start of a job that is not in `CREATED` with.
const NOT_STARTABLE: &str = "Job status is not in CREATED status and cannot be started";

/// How long a poll for a clock-owed completion waits between looks. The completion is owed
/// by the clock, so the test looks again rather than sleeping out a span.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How many times such a poll looks before giving up — a deliberately generous deadline
/// (far longer than the span the service owes), so a loaded or slow host cannot fail a
/// test that a fast one passes.
const POLL_ATTEMPTS: usize = 2_000;

/// A `FieldMap` entry: `ObjectTypeField`'s three members, with a `ContentType` out of the
/// modelled `FieldContentType` enum (STRING | NUMBER | PHONE_NUMBER | EMAIL_ADDRESS |
/// NAME).
pub(crate) fn field(column: &str, content_type: &str, target: &str) -> Value {
    json!({ "ContentType": content_type, "Source": format!("_source.{column}"), "Target": target })
}

/// The three headers `GetUploadJobPathResponse$ClientToken` documents a write to the
/// published target must carry: "x-amz-server-side-encryption-customer-key: Client Token,
/// x-amz-server-side-encryption-customer-key-MD5: MD5 Client Token,
/// x-amz-server-side-encryption-customer-algorithm: AES256" — the digest computed here the
/// way a client computes it, out of the key that response handed back.
fn upload_headers(client_token: &str) -> Vec<(String, String)> {
    let key = ::aws_smithy_types::base64::decode(client_token).unwrap_or_default();
    vec![
        (
            "x-amz-server-side-encryption-customer-algorithm".to_string(),
            "AES256".to_string(),
        ),
        (
            "x-amz-server-side-encryption-customer-key".to_string(),
            client_token.to_string(),
        ),
        (
            "x-amz-server-side-encryption-customer-key-MD5".to_string(),
            ::aws_smithy_types::base64::encode(Md5::digest(&key)),
        ),
    ]
}

/// `CreateUploadJob`, asserting it succeeded, and returning the minted `JobId`.
pub(crate) async fn create_job(emulator: &Emulator, domain: &str, body: Value) -> String {
    let (status, created) = emulator.create_upload_job(domain, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {body:?}: expected CreateUploadJob 200, got {status} {created:?}"
    );
    created
        .get("JobId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {body:?}: no JobId in {created:?}"))
        .to_string()
}

/// Write `file` to the upload target `GetUploadJobPath` publishes for one job — the write
/// the operation documents, performed exactly as a client performs it.
pub(crate) async fn upload_file(emulator: &Emulator, domain: &str, job_id: &str, file: &str) {
    let (status, path) = emulator.get_upload_job_path(domain, job_id).await;
    assert_eq!(
        status, 200,
        "case {job_id:?}: expected GetUploadJobPath 200, got {status} {path:?}"
    );
    let url = path
        .get("Url")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {job_id:?}: no Url in {path:?}"))
        .to_string();
    let client_token = path
        .get("ClientToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {job_id:?}: no ClientToken in {path:?}"))
        .to_string();
    let headers = upload_headers(&client_token);
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, answer) = emulator
        .send_raw("PUT", &url, &borrowed, file.as_bytes().to_vec())
        .await;
    assert_eq!(
        status, 200,
        "case {job_id:?}: expected the upload target to take the file, got {status} {answer:?}"
    );
}

/// `StartUploadJob`: `PUT /domains/{DomainName}/upload-jobs/{JobId}`.
pub(crate) async fn start_job(emulator: &Emulator, domain: &str, job_id: &str) -> (u16, Value) {
    emulator
        .send("PUT", &format!("/domains/{domain}/upload-jobs/{job_id}"), None)
        .await
}

/// The job once its processing has fallen due: looked at again until it reports something
/// other than `IN_PROGRESS`, which is the condition the test waits on.
pub(crate) async fn settled_job(emulator: &Emulator, domain: &str, job_id: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..POLL_ATTEMPTS {
        let (status, body) = emulator.get_upload_job(domain, job_id).await;
        assert_eq!(
            status, 200,
            "case {job_id:?}: expected GetUploadJob 200, got {status} {body:?}"
        );
        if body.get("Status").and_then(Value::as_str) != Some(IN_PROGRESS) {
            return body;
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {job_id:?}: the job never left {IN_PROGRESS}, last read {last:?}");
}

/// The `Status`, `StatusReason` and the three `ResultsSummary` counters of one read.
pub(crate) fn reported(body: &Value) -> (&str, Option<&str>, Option<i64>, Option<i64>, Option<i64>) {
    (
        body.get("Status").and_then(Value::as_str).unwrap_or(""),
        body.get("StatusReason").and_then(Value::as_str),
        body.pointer("/ResultsSummary/CreatedRecords")
            .and_then(Value::as_i64),
        body.pointer("/ResultsSummary/UpdatedRecords")
            .and_then(Value::as_i64),
        body.pointer("/ResultsSummary/FailedRecords")
            .and_then(Value::as_i64),
    )
}

/// One row of the ingest table: the file a job was given and what the run of it must come
/// to.
struct Row {
    /// What this row is about.
    case: &'static str,
    /// The `Fields` the job maps its columns through.
    fields: Value,
    /// The `UniqueKey` columns the job de-dupes by.
    unique_key: &'static str,
    /// The file written to the job's upload target.
    file: &'static str,
    /// The terminal `UploadJobStatus` the run must reach.
    status: &'static str,
    /// The `StatusReason` it must report.
    status_reason: Option<&'static str>,
    /// `CreatedRecords`, `UpdatedRecords`, `FailedRecords`.
    counters: (i64, i64, i64),
    /// `(_email value, how many profiles it must reach)`: the search that proves the
    /// records landed as profiles.
    search: (&'static str, usize),
}

/// A start ingests the job's file into the domain's profiles and the job reports what the
/// run came to — over a table of files differing in the number of records, the columns they
/// carry, whether two records share a unique key, and whether a record can be processed at
/// all.
#[tokio::test]
async fn a_start_ingests_the_file_and_reports_what_the_run_came_to() {
    let emulator = Emulator::new();

    let contacts = json!({
        "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
        "first_name": field("first_name", "NAME", "_profile.FirstName"),
        "last_name": field("last_name", "NAME", "_profile.LastName"),
    });

    let rows: Vec<Row> = vec![
        Row {
            case: "one record per profile",
            fields: contacts.clone(),
            unique_key: "email",
            file: "email,first_name,last_name\n\
                   ada@example.com,Ada,Lovelace\n\
                   grace@example.com,Grace,Hopper\n\
                   alan@example.com,Alan,Turing\n",
            status: SUCCEEDED,
            status_reason: None,
            counters: (3, 0, 0),
            search: ("grace@example.com", 1),
        },
        Row {
            case: "two records carrying the same unique key are one profile",
            fields: contacts.clone(),
            unique_key: "email",
            file: "email,first_name,last_name\n\
                   ada@example.com,Ada,Byron\n\
                   ada@example.com,Ada,Lovelace\n",
            status: SUCCEEDED,
            status_reason: None,
            counters: (1, 1, 0),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a record carrying no value for the unique-key column is failed",
            fields: contacts.clone(),
            unique_key: "email",
            file: "email,first_name,last_name\n\
                   ada@example.com,Ada,Lovelace\n\
                   ,Grace,Hopper\n",
            status: PARTIALLY_SUCCEEDED,
            status_reason: None,
            counters: (1, 0, 1),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a column the field map does not name is not ingested",
            fields: contacts.clone(),
            unique_key: "email",
            file: "email,first_name,last_name,loyalty_tier\n\
                   ada@example.com,Ada,Lovelace,gold\n",
            status: SUCCEEDED,
            status_reason: None,
            counters: (1, 0, 0),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a unique key naming several columns",
            fields: json!({
                "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress"),
                "account": field("account", "STRING", "_profile.AccountNumber"),
                "first_name": field("first_name", "NAME", "_profile.FirstName"),
            }),
            unique_key: "email,account",
            file: "email,account,first_name\n\
                   ada@example.com,ACCT-1,Ada\n\
                   ada@example.com,ACCT-1,Augusta\n\
                   grace@example.com,ACCT-2,Grace\n",
            status: SUCCEEDED,
            status_reason: None,
            counters: (2, 1, 0),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a unique key naming a column the file does not carry ingests nothing",
            fields: contacts.clone(),
            unique_key: "customer_ref",
            file: "email,first_name,last_name\n\
                   ada@example.com,Ada,Lovelace\n\
                   grace@example.com,Grace,Hopper\n",
            status: FAILED,
            status_reason: Some(VALIDATION_FAILURE),
            counters: (0, 0, 2),
            search: ("ada@example.com", 0),
        },
        Row {
            case: "a file holding one record whose every mapped column is empty",
            fields: contacts.clone(),
            unique_key: "email",
            file: "email,first_name,last_name\n\
                   ,,\n",
            status: FAILED,
            status_reason: Some(VALIDATION_FAILURE),
            counters: (0, 0, 1),
            search: ("ada@example.com", 0),
        },
    ];

    // Every row gets its own domain, so what a search finds is what that row's file
    // ingested; and every job is started before any is waited on, because the completion
    // each one owes runs on the same clock.
    let mut started: Vec<(&Row, String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let domain = format!("ingestdom{index}");
        emulator.create_domain(&domain, 30).await;
        let job_id = create_job(
            &emulator,
            &domain,
            json!({
                "DisplayName": format!("{}.csv", row.case.len()),
                "Fields": row.fields.clone(),
                "UniqueKey": row.unique_key,
            }),
        )
        .await;
        upload_file(&emulator, &domain, &job_id, row.file).await;

        let (status, answer) = start_job(&emulator, &domain, &job_id).await;
        assert_eq!(
            (status, &answer),
            (200, &json!({})),
            "case {:?}: expected StartUploadJob 200 with an empty body, got {status} {answer:?}",
            row.case
        );
        started.push((row, domain, job_id));
    }

    for (row, domain, job_id) in &started {
        let case = row.case;
        let settled = settled_job(&emulator, domain, job_id).await;
        let (created, updated, failed) = row.counters;
        assert_eq!(
            reported(&settled),
            (
                row.status,
                row.status_reason,
                Some(created),
                Some(updated),
                Some(failed)
            ),
            "case {case:?}: expected {} with {:?} and ({created}, {updated}, {failed}), got \
             {settled:?}",
            row.status,
            row.status_reason
        );
        assert!(
            settled.get("CompletedAt").is_some_and(|value| value.is_number()),
            "case {case:?}: a job whose processing has fallen due reports CompletedAt, got \
             {settled:?}"
        );

        // The records the run ingested are PROFILES of the domain, so the index answers
        // for a value the file carried.
        let (email, matches) = row.search;
        let (status, found) = emulator
            .search(domain, json!({ "KeyName": "_email", "Values": [email] }), "")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected SearchProfiles 200, got {status} {found:?}"
        );
        assert_eq!(
            item_ids(&found).len(),
            matches,
            "case {case:?}: expected {email} to reach {matches} profile(s), got {found:?}"
        );
    }
}

/// A started job reports the processing state until the completion the clock owes it, and
/// its answer is stable from then on.
#[tokio::test]
async fn a_started_job_reports_the_processing_until_its_completion_falls_due() {
    let emulator = Emulator::new();
    emulator.create_domain("spandom", 30).await;

    let job_id = create_job(
        &emulator,
        "spandom",
        json!({
            "DisplayName": "span.csv",
            "Fields": { "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress") },
            "UniqueKey": "email",
        }),
    )
    .await;
    upload_file(
        &emulator,
        "spandom",
        &job_id,
        "email\nada@example.com\ngrace@example.com\n",
    )
    .await;

    let (_, before) = emulator.get_upload_job("spandom", &job_id).await;
    assert_eq!(
        reported(&before),
        (CREATED, None, None, None, None),
        "case \"before the start\": expected {CREATED} and no counter, got {before:?}"
    );

    let (status, answer) = start_job(&emulator, "spandom", &job_id).await;
    assert_eq!(
        status, 200,
        "case \"the start\": expected 200, got {status} {answer:?}"
    );

    // The span the service owes is never zero, so the transient state is observable to a
    // caller that reads the job back straight away.
    let (_, immediately) = emulator.get_upload_job("spandom", &job_id).await;
    assert_eq!(
        reported(&immediately),
        (IN_PROGRESS, None, None, None, None),
        "case \"straight after the start\": expected {IN_PROGRESS} with no counter, got \
         {immediately:?}"
    );
    assert_eq!(
        immediately.get("CompletedAt"),
        None,
        "case \"straight after the start\": a job that is still processing has not \
         completed, got {immediately:?}"
    );

    let settled = settled_job(&emulator, "spandom", &job_id).await;
    assert_eq!(
        reported(&settled),
        (SUCCEEDED, None, Some(2), Some(0), Some(0)),
        "case \"after the span\": expected {SUCCEEDED} with two created records, got {settled:?}"
    );
    assert_eq!(
        settled.get("CreatedAt"),
        before.get("CreatedAt"),
        "case \"after the span\": the instant the job was created does not move, expected \
         {:?}, got {:?}",
        before.get("CreatedAt"),
        settled.get("CreatedAt")
    );

    // A completion is a property of the job, not of the call that read it: two reads of a
    // settled job are identical.
    let (status, again) = emulator.get_upload_job("spandom", &job_id).await;
    assert_eq!(
        (status, &again),
        (200, &settled),
        "case \"a second read after the span\": expected the same envelope, got {again:?}"
    );
}

/// A job nothing was uploaded for has no profile data to ingest, so its run fails
/// validation and accounts for no record.
#[tokio::test]
async fn a_job_with_no_uploaded_file_fails_validation() {
    let emulator = Emulator::new();
    emulator.create_domain("nofiledom", 30).await;

    let job_id = create_job(
        &emulator,
        "nofiledom",
        json!({
            "DisplayName": "never-uploaded.csv",
            "Fields": { "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress") },
            "UniqueKey": "email",
        }),
    )
    .await;

    let (status, answer) = start_job(&emulator, "nofiledom", &job_id).await;
    assert_eq!(
        status, 200,
        "case \"a start with nothing uploaded\": expected 200, got {status} {answer:?}"
    );

    let settled = settled_job(&emulator, "nofiledom", &job_id).await;
    assert_eq!(
        reported(&settled),
        (FAILED, Some(VALIDATION_FAILURE), None, None, None),
        "case \"a start with nothing uploaded\": expected {FAILED} {VALIDATION_FAILURE} with \
         no counter, got {settled:?}"
    );
    assert!(
        settled
            .get("ResultsSummary")
            .is_some_and(Value::is_object),
        "case \"a start with nothing uploaded\": the summary is part of every stored job, \
         got {settled:?}"
    );
}

/// Processing is started once: a job in any status but `CREATED` is refused, and so is a
/// reference to a domain or a job that is not stored — none of them changing what the job
/// reports.
#[tokio::test]
async fn a_job_is_startable_only_from_the_created_status() {
    let emulator = Emulator::new();
    emulator.create_domain("statedom", 30).await;

    let body = json!({
        "DisplayName": "twice.csv",
        "Fields": { "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress") },
        "UniqueKey": "email",
    });

    // One job started and still processing, and one whose processing has fallen due: the
    // two states a started job passes through.
    let processing = create_job(&emulator, "statedom", body.clone()).await;
    upload_file(&emulator, "statedom", &processing, "email\nada@example.com\n").await;
    let settled = create_job(&emulator, "statedom", body.clone()).await;
    upload_file(
        &emulator,
        "statedom",
        &settled,
        "email\ngrace@example.com\n",
    )
    .await;
    for job_id in [&processing, &settled] {
        let (status, answer) = start_job(&emulator, "statedom", job_id).await;
        assert_eq!(
            status, 200,
            "case {job_id:?}: expected the first StartUploadJob to be served, got {status} \
             {answer:?}"
        );
    }
    let settled_report = settled_job(&emulator, "statedom", &settled).await;

    let refused: Vec<(&str, &String)> = vec![
        ("a job whose processing is under way", &processing),
        ("a job whose processing has fallen due", &settled),
    ];
    for (case, job_id) in refused {
        let before = emulator.get_upload_job("statedom", job_id).await.1;
        let (status, answer) = start_job(&emulator, "statedom", job_id).await;
        assert_eq!(
            (status, error_code(&answer), error_message(&answer)),
            (400, "BadRequestException", NOT_STARTABLE),
            "case {case:?}: expected 400 BadRequestException {NOT_STARTABLE:?}, got {status} \
             {answer:?}"
        );
        let after = emulator.get_upload_job("statedom", job_id).await.1;
        assert_eq!(
            reported(&before),
            reported(&after),
            "case {case:?}: a refused start changes nothing, expected {before:?}, got {after:?}"
        );
    }

    // The settled job still reports the run it completed: a refused start neither restarts
    // nor re-counts it.
    assert_eq!(
        reported(&settled_job(&emulator, "statedom", &settled).await),
        reported(&settled_report),
        "case \"a settled job after a refused start\": expected {settled_report:?}"
    );

    // A reference to something not stored is the absence of that resource, reported in the
    // wording the upload-job operations use for it.
    let absent_job = "0".repeat(32);
    let absent: Vec<(&str, String, String, String)> = vec![
        (
            "a domain that is not stored",
            "nosuchdomain".to_string(),
            processing.clone(),
            "Domain with name nosuchdomain could not be found".to_string(),
        ),
        (
            "a domain name of the maximum modelled length",
            "d".repeat(64),
            processing.clone(),
            format!("Domain with name {} could not be found", "d".repeat(64)),
        ),
        (
            "a job the stored domain does not hold",
            "statedom".to_string(),
            absent_job.clone(),
            format!("The upload job {absent_job} does not exist in domain statedom"),
        ),
        (
            "a job id of the minimum modelled length",
            "statedom".to_string(),
            "j".to_string(),
            "The upload job j does not exist in domain statedom".to_string(),
        ),
    ];
    for (case, domain, job_id, message) in absent {
        let (status, answer) = start_job(&emulator, &domain, &job_id).await;
        assert_eq!(
            (status, error_code(&answer), error_message(&answer)),
            (404, "ResourceNotFoundException", message.as_str()),
            "case {case:?}: expected 404 ResourceNotFoundException {message:?}, got {status} \
             {answer:?}"
        );
    }
}
