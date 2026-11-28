// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopUploadJob`: the processing a stop ends, and the states a job
//! can be stopped from.
//!
//! The rules under test:
//!
//! 1. `StopUploadJob` "stops the processing of an upload job": for a job whose processing is
//!    under way it answers 200 with an empty body, and the job reports `STOPPED` — "the
//!    upload job has been manually stopped or terminated before completion" — from that
//!    call on, with the instant it stopped and the records the interrupted run had already
//!    accounted for. Those records are profiles of the domain, and the completion the clock
//!    owed the run never falls due, so the stopped answer is the job's final one.
//! 2. Only processing can be stopped: a job in any other status is refused with
//!    `BadRequestException` (400) and the service's own sentence for that state, and a
//!    domain or job that is not stored with `ResourceNotFoundException` (404) — neither
//!    changing what the job reports.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`; the upload-job helpers are shared
//! with `tests/all/upload_job_processing.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, error_message, item_ids, Emulator};
use crate::upload_job_processing::{
    create_job, field, reported, settled_job, start_job, upload_file,
};

/// The `UploadJobStatus` a job carries until a call starts it: "CREATED: The upload job has
/// been created, but has not started processing yet."
const CREATED: &str = "CREATED";

/// The `UploadJobStatus` of a job a call stopped: "STOPPED: The upload job has been manually
/// stopped or terminated before completion."
const STOPPED: &str = "STOPPED";

/// The sentence the service refuses a stop of a job whose processing is not under way with.
const NOT_STOPPABLE: &str = "Job status is not in IN_PROGRESS status and cannot be stopped";

/// `StopUploadJob`: `PUT /domains/{DomainName}/upload-jobs/{JobId}/stop`.
async fn stop_job(emulator: &Emulator, domain: &str, job_id: &str) -> (u16, Value) {
    emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/upload-jobs/{job_id}/stop"),
            None,
        )
        .await
}

/// One row of the stop table: the job that was started, and what the stop of it must leave
/// the job reporting.
struct Row {
    /// What this row is about.
    case: &'static str,
    /// The `Fields` the job maps its columns through.
    fields: Value,
    /// The `UniqueKey` columns the job de-dupes by.
    unique_key: &'static str,
    /// The file written to the job's upload target, or `None` for a job nothing was
    /// uploaded for.
    file: Option<&'static str>,
    /// `CreatedRecords`, `UpdatedRecords`, `FailedRecords` the stopped job must report, or
    /// `None` when the interrupted run accounted for no record at all.
    counters: Option<(i64, i64, i64)>,
    /// `(_email value, how many profiles it must reach)`: the search that proves the
    /// records the run wrote before the stop are profiles of the domain.
    search: (&'static str, usize),
}

/// A stop ends the processing of a started job: the job reports the stopped state with the
/// records the run had accounted for, and the completion the clock owed that run never
/// falls due — over a table of jobs differing in the number of records their file carried,
/// whether two records shared a unique key, whether a record could be processed at all, and
/// whether anything was uploaded for the job.
#[tokio::test]
async fn a_stop_ends_the_processing_and_the_job_reports_the_stopped_state() {
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
            file: Some(
                "email,first_name,last_name\n\
                 ada@example.com,Ada,Lovelace\n\
                 grace@example.com,Grace,Hopper\n\
                 alan@example.com,Alan,Turing\n",
            ),
            counters: Some((3, 0, 0)),
            search: ("grace@example.com", 1),
        },
        Row {
            case: "two records carrying the same unique key are one profile",
            fields: contacts.clone(),
            unique_key: "email",
            file: Some(
                "email,first_name,last_name\n\
                 ada@example.com,Ada,Byron\n\
                 ada@example.com,Ada,Lovelace\n",
            ),
            counters: Some((1, 1, 0)),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a record carrying no value for the unique-key column is failed",
            fields: contacts.clone(),
            unique_key: "email",
            file: Some(
                "email,first_name,last_name\n\
                 ada@example.com,Ada,Lovelace\n\
                 ,Grace,Hopper\n",
            ),
            counters: Some((1, 0, 1)),
            search: ("ada@example.com", 1),
        },
        Row {
            case: "a job nothing was uploaded for had no record to account for",
            fields: contacts.clone(),
            unique_key: "email",
            file: None,
            counters: None,
            search: ("ada@example.com", 0),
        },
    ];

    // Every row gets its own domain, so what a search finds is what that row's file
    // ingested.
    let mut stopped: Vec<(&Row, String, String, Value)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let domain = format!("stopdom{index}");
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
        if let Some(file) = row.file {
            upload_file(&emulator, &domain, &job_id, file).await;
        }

        let (status, answer) = start_job(&emulator, &domain, &job_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected StartUploadJob 200, got {status} {answer:?}",
            row.case
        );

        let (status, answer) = stop_job(&emulator, &domain, &job_id).await;
        assert_eq!(
            (status, &answer),
            (200, &json!({})),
            "case {:?}: expected StopUploadJob 200 with an empty body, got {status} {answer:?}",
            row.case
        );

        let (status, after) = emulator.get_upload_job(&domain, &job_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetUploadJob 200, got {status} {after:?}",
            row.case
        );
        let (created, updated, failed) = row
            .counters
            .map_or((None, None, None), |(created, updated, failed)| {
                (Some(created), Some(updated), Some(failed))
            });
        assert_eq!(
            reported(&after),
            (STOPPED, None, created, updated, failed),
            "case {:?}: expected {STOPPED} with no reason and ({created:?}, {updated:?}, \
             {failed:?}), got {after:?}",
            row.case
        );
        assert!(
            after.get("CompletedAt").is_some_and(Value::is_number),
            "case {:?}: a stopped job reports the instant its processing ended, got {after:?}",
            row.case
        );

        stopped.push((row, domain, job_id, after));
    }

    // A control job on the same clock: once ITS processing has fallen due, the span the
    // stopped jobs' runs were owed has passed too — so what they report now is final, and
    // no sleep is needed to establish it.
    emulator.create_domain("stopcontrol", 30).await;
    let control = create_job(
        &emulator,
        "stopcontrol",
        json!({
            "DisplayName": "control.csv",
            "Fields": { "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress") },
            "UniqueKey": "email",
        }),
    )
    .await;
    upload_file(&emulator, "stopcontrol", &control, "email\nada@example.com\n").await;
    start_job(&emulator, "stopcontrol", &control).await;
    settled_job(&emulator, "stopcontrol", &control).await;

    for (row, domain, job_id, at_the_stop) in &stopped {
        let case = row.case;
        let (status, later) = emulator.get_upload_job(domain, job_id).await;
        assert_eq!(
            (status, &later),
            (200, at_the_stop),
            "case {case:?}: the completion the stopped run was owed never falls due, expected \
             {at_the_stop:?}, got {status} {later:?}"
        );

        // The records the run wrote before the stop are PROFILES of the domain, so the index
        // answers for a value the file carried.
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

/// Only processing can be stopped: a job no call has started, a job whose processing has
/// fallen due and a job already stopped are all refused, and so is a reference to a domain
/// or a job that is not stored — none of them changing what the job reports.
#[tokio::test]
async fn a_job_is_stoppable_only_while_its_processing_is_under_way() {
    let emulator = Emulator::new();
    emulator.create_domain("stopstatedom", 30).await;

    let body = json!({
        "DisplayName": "states.csv",
        "Fields": { "email": field("email", "EMAIL_ADDRESS", "_profile.EmailAddress") },
        "UniqueKey": "email",
    });

    // The three states that are not processing: a job nothing has started, a job whose
    // processing has fallen due, and a job a call already stopped.
    let created = create_job(&emulator, "stopstatedom", body.clone()).await;
    let (_, created_report) = emulator.get_upload_job("stopstatedom", &created).await;
    assert_eq!(
        reported(&created_report).0,
        CREATED,
        "case \"a job nothing has started\": expected {CREATED}, got {created_report:?}"
    );

    let settled = create_job(&emulator, "stopstatedom", body.clone()).await;
    upload_file(
        &emulator,
        "stopstatedom",
        &settled,
        "email\ngrace@example.com\n",
    )
    .await;
    start_job(&emulator, "stopstatedom", &settled).await;
    settled_job(&emulator, "stopstatedom", &settled).await;

    let twice = create_job(&emulator, "stopstatedom", body.clone()).await;
    upload_file(
        &emulator,
        "stopstatedom",
        &twice,
        "email\nalan@example.com\n",
    )
    .await;
    start_job(&emulator, "stopstatedom", &twice).await;
    let (status, answer) = stop_job(&emulator, "stopstatedom", &twice).await;
    assert_eq!(
        status, 200,
        "case \"the first stop of a processing job\": expected 200, got {status} {answer:?}"
    );

    let refused: Vec<(&str, &String)> = vec![
        ("a job nothing has started", &created),
        ("a job whose processing has fallen due", &settled),
        ("a job a call already stopped", &twice),
    ];
    for (case, job_id) in refused {
        let before = emulator.get_upload_job("stopstatedom", job_id).await.1;
        let (status, answer) = stop_job(&emulator, "stopstatedom", job_id).await;
        assert_eq!(
            (status, error_code(&answer), error_message(&answer)),
            (400, "BadRequestException", NOT_STOPPABLE),
            "case {case:?}: expected 400 BadRequestException {NOT_STOPPABLE:?}, got {status} \
             {answer:?}"
        );
        let after = emulator.get_upload_job("stopstatedom", job_id).await.1;
        assert_eq!(
            before, after,
            "case {case:?}: a refused stop changes nothing, expected {before:?}, got {after:?}"
        );
    }

    // A reference to something not stored is the absence of that resource, reported in the
    // wording the upload-job operations use for it.
    let absent_job = "0".repeat(32);
    let absent: Vec<(&str, String, String, String)> = vec![
        (
            "a domain that is not stored",
            "nosuchdomain".to_string(),
            created.clone(),
            "Domain with name nosuchdomain could not be found".to_string(),
        ),
        (
            "a domain name of the maximum modelled length",
            "d".repeat(64),
            created.clone(),
            format!("Domain with name {} could not be found", "d".repeat(64)),
        ),
        (
            "a job the stored domain does not hold",
            "stopstatedom".to_string(),
            absent_job.clone(),
            format!("The upload job {absent_job} does not exist in domain stopstatedom"),
        ),
        (
            "a job id of the minimum modelled length",
            "stopstatedom".to_string(),
            "j".to_string(),
            "The upload job j does not exist in domain stopstatedom".to_string(),
        ),
    ];
    for (case, domain, job_id, message) in absent {
        let (status, answer) = stop_job(&emulator, &domain, &job_id).await;
        assert_eq!(
            (status, error_code(&answer), error_message(&answer)),
            (404, "ResourceNotFoundException", message.as_str()),
            "case {case:?}: expected 404 ResourceNotFoundException {message:?}, got {status} \
             {answer:?}"
        );
    }
}
