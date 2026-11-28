// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the record kinds the refreshed model added: the `Dataset`
//! lifecycle, the medical coding job pair and the feedback a submission records.
//!
//! Declared as a child module of the suite whose RULE it extends — the clock-owed work every
//! handler of this service materialises — because `tests/all.rs` is this crate's declaration
//! list and the merge gate refuses a fix that edits it; migrating the crate to the
//! `automod` layout removes that file and is a change of its own rather than part of this
//! one.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than on one recorded request:
//!
//! 1. A `Dataset` is PERSISTED by its create and PROJECTED by its readers: `GetDataset` and
//!    `ListDatasets` report what the create wrote plus what the clock has since written, and
//!    the statuses the clock moves it through are the modelled ones — `CREATING` ->
//!    `ACTIVE`, and `DELETING` -> gone, which is what makes the terminal `DELETED` the
//!    status the model says no reader reports. The rows differ along the name, the
//!    configured paths and the filter the listing narrows by.
//! 2. `CreateDataset` is idempotent on `clientToken`, as its own documentation states: a
//!    retry answers with the dataset the first request made, a DIFFERENT request carrying
//!    that token is the modelled conflict, and a create carrying no token binds nothing.
//!    Past the count this service holds per domain, a create is the modelled quota refusal.
//! 3. A `DeleteDataset` is accepted only in the statuses the model names ("You can delete a
//!    dataset whose status is ACTIVE, CREATE_FAILED, or DELETE_FAILED"), is the modelled
//!    conflict in the two it does not, and is the modelled 404 for the status no reader
//!    reports. One row per value of the modelled `DatasetStatus`.
//! 4. The medical coding job pair MINTS, persists and projects a job, the clock moves it
//!    through the modelled `JobStatus` values to a terminal one, and a job is SCOPED to its
//!    domain. A `datasetId` the start names has to be a dataset OF THAT DOMAIN and ACTIVE.
//! 5. `SubmitFeedback` RECORDS what it accepts — asserted where the record lives, since the
//!    operation's response declares no members — and a repeat presenting a token already
//!    recorded is accepted without recording anything further.
//!
//! The harness (`Emulator`, `error_code`, `string_at`) and the two timestamp forms are the
//! sibling suites': one way of driving this service, not two. Where a completed coding job
//! reports the OBJECT it wrote, that object is read back through Amazon S3's own API in
//! `core/router/tests/all/connecthealth_datasets_and_medical_coding_jobs.rs`, because only
//! the assembled router mounts the service that owns it.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::ops::types::{
    DatasetConfig, MedicalCodingDatasetConfig,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, Dataset, DatasetStatus, MAX_DATASETS_PER_DOMAIN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The one `DatasetType` the model declares
/// (`smithy_query(query="shape",shape="DatasetType",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
const MEDICAL_CODING: &str = "MEDICAL_CODING";

/// How long a poll keeps LOOKING for a condition before it gives up: many times over the
/// spans this emulator's own lifecycles take (`state::DATASET_STEP_SPAN`,
/// `state::MEDICAL_CODING_JOB_STEP_SPAN`), so a loaded host has room while a fast one never
/// waits at all. The CONDITION is what every poll below waits for, never an interval.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// `POST`/`GET /domains/{domainId}/datasets` — `CreateDataset`'s and `ListDatasets`' route.
fn datasets_path(domain_id: &str) -> String {
    format!("/domains/{domain_id}/datasets")
}

/// `GET`/`DELETE /domains/{domainId}/datasets/{datasetId}`.
fn dataset_path(domain_id: &str, dataset_id: &str) -> String {
    format!("/domains/{domain_id}/datasets/{dataset_id}")
}

/// `POST /domain/{domainId}/medical-coding-job` — `StartMedicalCodingJob`'s route.
fn coding_jobs_path(domain_id: &str) -> String {
    format!("/domain/{domain_id}/medical-coding-job")
}

/// `GET /domain/{domainId}/medical-coding-job/{jobId}` — `GetMedicalCodingJob`'s route.
fn coding_job_path(domain_id: &str, job_id: &str) -> String {
    format!("/domain/{domain_id}/medical-coding-job/{job_id}")
}

/// A `CreateDataset` body: the dataset's name, the configuration it is built from, and the
/// `clientToken` when the case carries one.
fn dataset_body(name: &str, input_path: &str, output_path: &str, token: Option<&str>) -> Value {
    let mut body = json!({
        "name": name,
        "config": {
            "medicalCoding": {
                "s3InputPath": input_path,
                "s3OutputPath": output_path,
            }
        },
    });
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    body
}

/// A `StartMedicalCodingJob` body: the clinical text, where its codes go, and the dataset
/// the case names.
fn coding_job_body(text: &str, s3_output_path: &str, dataset_id: Option<&str>) -> Value {
    let mut body = json!({
        "text": text,
        "outputDataConfig": { "s3OutputPath": s3_output_path },
    });
    if let Some(dataset_id) = dataset_id {
        body["datasetId"] = json!(dataset_id);
    }
    body
}

/// Look at a dataset through its own reader until it reports `status`, and answer that
/// reading — or fail once [`POLL_DEADLINE`] has passed.
async fn poll_until_dataset_status(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    dataset_id: &str,
    status: &str,
) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (code, body) = emulator
            .send("GET", &dataset_path(domain_id, dataset_id), None)
            .await;
        if code == 200 && body["status"].as_str() == Some(status) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the dataset still did not report {status:?} after {POLL_DEADLINE:?}, \
             last read {code} with {body}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// Look at a dataset through its own reader until this service holds none under its id — the
/// removal the clock owes a deleted dataset.
async fn poll_until_dataset_unheld(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    dataset_id: &str,
) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (code, body) = emulator
            .send("GET", &dataset_path(domain_id, dataset_id), None)
            .await;
        if code == 404 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the dataset was still held after {POLL_DEADLINE:?}, last read {code} \
             with {body}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// Look at a medical coding job through its own reader until it reports a terminal
/// `JobStatus`, and answer that reading.
async fn poll_until_job_terminal(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (code, body) = emulator
            .send("GET", &coding_job_path(domain_id, job_id), None)
            .await;
        assert_eq!(
            code, 200,
            "case {case:?}: GetMedicalCodingJob must answer 200, got {code} with {body}"
        );
        if matches!(
            body["jobStatus"].as_str(),
            Some("SUCCEEDED") | Some("FAILED")
        ) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job was still not terminal after {POLL_DEADLINE:?}, last read \
             {body}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The datasets of one domain the listing reports, by `datasetId`, following every
/// `nextToken` the service issues.
async fn listed_dataset_ids(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    query: &str,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let path = match token.as_deref() {
            Some(cursor) => format!("{}?{query}&nextToken={cursor}", datasets_path(domain_id)),
            None => format!("{}?{query}", datasets_path(domain_id)),
        };
        let (code, body) = emulator.send("GET", &path, None).await;
        assert_eq!(
            code, 200,
            "case {case:?}: ListDatasets must answer 200, got {code} with {body}"
        );
        let items = body["items"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListDatasets must report an items list, got {body}")
            })
            .clone();
        for item in &items {
            let id = item["datasetId"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every summary must carry a datasetId, got {item}")
            });
            assert!(
                ids.insert(id.to_string()),
                "case {case:?}: a dataset must be listed once, got {id:?} twice"
            );
        }
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
}

/// Seed a dataset in `status` straight into the store, the way no sequence of calls can:
/// `CREATING` and `DELETING` are statuses this emulator's own lifecycle leaves within one
/// span, so a case that asserts how a DELETE is answered in EVERY modelled status cannot
/// reach them by polling without depending on how fast the host runs.
///
/// The record owes NO transition (`next_transition_at: None`), so the status stands for the
/// call under test.
fn seed_dataset(emulator: &Emulator, domain_id: &str, dataset_id: &str, status: DatasetStatus) {
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let domain = store
        .held_domain_mut(emulator.region(), domain_id)
        .unwrap_or_else(|| panic!("the domain {domain_id} this case created"));
    let now = SystemTime::now();
    let arn = state::dataset_arn(&domain.arn, dataset_id);
    domain.datasets.insert(
        dataset_id.to_string(),
        Dataset {
            dataset_id: dataset_id.to_string(),
            domain_id: domain_id.to_string(),
            arn,
            name: format!("seeded {}", status.as_wire()),
            dataset_type: MEDICAL_CODING.to_string(),
            config: DatasetConfig::medicalCoding(MedicalCodingDatasetConfig {
                s3_input_path: "s3://clinic-coding-in/examples/".to_string(),
                s3_output_path: "s3://clinic-coding-out/scores/".to_string(),
            }),
            status,
            status_details: None,
            artifacts: None,
            created_at: now,
            updated_at: now,
            next_transition_at: None,
            client_token: None,
            creation_fingerprint: String::new(),
        },
    );
}

#[tokio::test]
async fn a_dataset_is_persisted_read_back_listed_and_deleted() {
    let emulator = Emulator::new("us-west-2");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-datasets" }))
            .await,
        "domainId",
    );

    // The rows differ along what the rule generalises over: the name (both ends of what a
    // caller plausibly sends, in the characters `BoundedSafeString`'s pattern
    // `^[\x20-\x21\x23\x24\x2A-\x3A\x3F-\x7E]*$` admits), and the shape of the configured
    // input and output locations.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "bucket roots",
            "coding-examples",
            "s3://clinic-coding-in",
            "s3://clinic-coding-out",
        ),
        (
            "deep prefixes and a name of several words",
            "Clinic 9 examples, v2 - reviewed",
            "s3://clinic-coding-in/2026/01/",
            "s3://clinic-coding-out/scores",
        ),
        (
            "the shortest name the shape allows",
            "x",
            "s3://clinic.coding-in9/examples/",
            "s3://clinic-coding-out/x/",
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, name, input_path, output_path) in &rows {
        let (code, created) = emulator
            .send(
                "POST",
                &datasets_path(&domain_id),
                Some(&dataset_body(name, input_path, output_path, None)),
            )
            .await;
        assert_eq!(
            code, 201,
            "case {case:?}: CreateDataset must answer 201, got {code} with {created}"
        );
        let dataset_id = string_at(&created, "datasetId");
        assert!(
            !dataset_id.is_empty()
                && dataset_id.len() <= 64
                && dataset_id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "case {case:?}: a minted datasetId must satisfy the modelled ^[a-z0-9]{{1,64}}$, \
             got {dataset_id:?}"
        );
        assert!(
            minted.insert(dataset_id.clone()),
            "case {case:?}: every create must mint a fresh datasetId, got the reused \
             {dataset_id:?}"
        );
        let arn = string_at(&created, "arn");
        assert!(
            arn.ends_with(&format!("domain/{domain_id}/dataset/{dataset_id}")),
            "case {case:?}: the ARN must name the dataset under its own domain, got {arn:?}"
        );
        assert_eq!(
            (created["type"].as_str(), created["status"].as_str()),
            (Some(MEDICAL_CODING), Some("CREATING")),
            "case {case:?}: a new dataset is of the type its config gives it and has the \
             status the model documents, got {created}"
        );
        // A `Dataset`'s instants declare no `@timestampFormat`, so they keep the protocol's
        // epoch NUMBER — the control row for the job pair's date-time strings.
        assert!(
            created["createdAt"].is_number(),
            "case {case:?}: createdAt declares no @timestampFormat, so it stays the \
             protocol's epoch number, got {created}"
        );

        // The reader PROJECTS the stored record: what the create wrote, plus the members
        // only the reader reports.
        let (code, fetched) = emulator
            .send("GET", &dataset_path(&domain_id, &dataset_id), None)
            .await;
        assert_eq!(
            code, 200,
            "case {case:?}: GetDataset must answer 200, got {code} with {fetched}"
        );
        assert_eq!(
            (
                fetched["arn"].as_str(),
                fetched["datasetId"].as_str(),
                fetched["name"].as_str(),
                fetched["type"].as_str(),
            ),
            (
                Some(arn.as_str()),
                Some(dataset_id.as_str()),
                Some(*name),
                Some(MEDICAL_CODING),
            ),
            "case {case:?}: the reader must report what the create persisted, got {fetched}"
        );
        assert_eq!(
            fetched["config"],
            json!({
                "medicalCoding": {
                    "s3InputPath": input_path,
                    "s3OutputPath": output_path,
                }
            }),
            "case {case:?}: the reader must report the configuration the create was given, \
             got {fetched}"
        );
        assert!(
            fetched["artifacts"].is_null(),
            "case {case:?}: a dataset nothing evaluated reports no artifacts, got {fetched}"
        );

        // The transition the CLOCK owes a new dataset, waited for by LOOKING for it.
        let built = poll_until_dataset_status(&emulator, case, &domain_id, &dataset_id, "ACTIVE")
            .await;
        assert!(
            built["updatedAt"].as_f64().unwrap_or_default()
                >= built["createdAt"].as_f64().unwrap_or_default(),
            "case {case:?}: a built dataset's last change must not precede its creation, got \
             {built}"
        );

        // And the listing reports it — unfiltered, and under the type it has. A page size of
        // one is asked for, so the cursor this listing issues is followed as well.
        for (filter, query) in [
            ("unfiltered", "maxResults=1"),
            ("filtered by its own type", "maxResults=1&type=MEDICAL_CODING"),
        ] {
            let listed = listed_dataset_ids(&emulator, case, &domain_id, query).await;
            assert!(
                listed.contains(&dataset_id),
                "case {case:?} ({filter}): the listing must report the dataset, got {listed:?}"
            );
        }
    }

    // A delete takes the dataset out of every reader: the status it answers with is the
    // modelled `DELETING`, and the removal the clock then owes is what makes the terminal
    // status the one no reader reports.
    let doomed = minted.iter().next().cloned().expect("a dataset to delete");
    let (code, deleted) = emulator
        .send("DELETE", &dataset_path(&domain_id, &doomed), None)
        .await;
    assert_eq!(
        (code, deleted["status"].as_str()),
        (202, Some("DELETING")),
        "DeleteDataset must answer 202 with the modelled DELETING, got {code} with {deleted}"
    );
    assert_eq!(
        string_at(&deleted, "datasetId"),
        doomed,
        "the delete must answer about the dataset it was addressed to, got {deleted}"
    );
    poll_until_dataset_unheld(&emulator, "a deleted dataset", &domain_id, &doomed).await;
    let listed = listed_dataset_ids(&emulator, "a deleted dataset", &domain_id, "maxResults=100")
        .await;
    assert!(
        !listed.contains(&doomed),
        "a deleted dataset must leave the listing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_dataset_delete_is_accepted_only_in_the_statuses_the_model_names() {
    let emulator = Emulator::new("eu-west-1");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-dataset-statuses" }))
            .await,
        "domainId",
    );

    // One row per value of the modelled `DatasetStatus`
    // (`smithy_query(query="shape",shape="DatasetStatus")`): the three the operation's own
    // documentation names as deletable, the two it does not, and the one no reader reports.
    let rows: Vec<(DatasetStatus, u16, &str)> = vec![
        (DatasetStatus::Active, 202, ""),
        (DatasetStatus::CreateFailed, 202, ""),
        (DatasetStatus::DeleteFailed, 202, ""),
        (DatasetStatus::Creating, 409, "ConflictException"),
        (DatasetStatus::Deleting, 409, "ConflictException"),
        (DatasetStatus::Deleted, 404, "ResourceNotFoundException"),
    ];

    for (index, (status, expected_code, expected_error)) in rows.iter().enumerate() {
        let case = format!("a delete of a dataset in {}", status.as_wire());
        let dataset_id = format!("seeded{index}");
        seed_dataset(&emulator, &domain_id, &dataset_id, *status);
        let (code, answered) = emulator
            .send("DELETE", &dataset_path(&domain_id, &dataset_id), None)
            .await;
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected HTTP {expected_code}, got {code} with {answered}"
        );
        if expected_error.is_empty() {
            assert_eq!(
                answered["status"].as_str(),
                Some("DELETING"),
                "case {case:?}: an accepted delete must report the modelled DELETING, got \
                 {answered}"
            );
            continue;
        }
        assert_eq!(
            error_code(&answered),
            *expected_error,
            "case {case:?}: expected {expected_error}, got {answered}"
        );
        let stated = string_at(&answered, "message");
        assert!(
            !stated.is_empty(),
            "case {case:?}: the refusal must say what was wrong, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn create_dataset_answers_a_retry_with_the_dataset_the_first_request_made() {
    let emulator = Emulator::new("ap-south-1");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-dataset-tokens" }))
            .await,
        "domainId",
    );

    let first = emulator
        .send(
            "POST",
            &datasets_path(&domain_id),
            Some(&dataset_body(
                "tokened",
                "s3://clinic-coding-in/a/",
                "s3://clinic-coding-out/a/",
                Some("token-one"),
            )),
        )
        .await;
    assert_eq!(
        first.0, 201,
        "the first create must be accepted, got {} with {}",
        first.0, first.1
    );
    let first_id = string_at(&first.1, "datasetId");

    let (code, retried) = emulator
        .send(
            "POST",
            &datasets_path(&domain_id),
            Some(&dataset_body(
                "tokened",
                "s3://clinic-coding-in/a/",
                "s3://clinic-coding-out/a/",
                Some("token-one"),
            )),
        )
        .await;
    assert_eq!(
        (code, string_at(&retried, "datasetId").as_str()),
        (201, first_id.as_str()),
        "case \"a retry of the same request\": the create must answer with the dataset the \
         first request made, got {code} with {retried}"
    );

    // One row per parameter a different request can differ in under the same token.
    for (case, body) in [
        (
            "a different name under the same token",
            dataset_body(
                "tokened-elsewhere",
                "s3://clinic-coding-in/a/",
                "s3://clinic-coding-out/a/",
                Some("token-one"),
            ),
        ),
        (
            "a different input location under the same token",
            dataset_body(
                "tokened",
                "s3://clinic-coding-in/b/",
                "s3://clinic-coding-out/a/",
                Some("token-one"),
            ),
        ),
        (
            "a different output location under the same token",
            dataset_body(
                "tokened",
                "s3://clinic-coding-in/a/",
                "s3://clinic-coding-out/b/",
                Some("token-one"),
            ),
        ),
    ] {
        let (code, refused) = emulator
            .send("POST", &datasets_path(&domain_id), Some(&body))
            .await;
        assert_eq!(
            (code, error_code(&refused).as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected the modelled conflict, got {code} with {refused}"
        );
    }

    // No token binds nothing: each create mints its own dataset.
    let mut untokened = BTreeSet::new();
    for attempt in 0..2 {
        let (code, created) = emulator
            .send(
                "POST",
                &datasets_path(&domain_id),
                Some(&dataset_body(
                    "untokened",
                    "s3://clinic-coding-in/c/",
                    "s3://clinic-coding-out/c/",
                    None,
                )),
            )
            .await;
        assert_eq!(
            code, 201,
            "case \"an untokened create\" (attempt {attempt}): got {code} with {created}"
        );
        assert!(
            untokened.insert(string_at(&created, "datasetId")),
            "case \"an untokened create\": each one must mint its own dataset, got the reused \
             {created}"
        );
    }

    // The store is bounded, and the bound is the one the service states: a create past it is
    // the modelled quota refusal rather than a record.
    for index in (1 + untokened.len())..MAX_DATASETS_PER_DOMAIN {
        let (code, created) = emulator
            .send(
                "POST",
                &datasets_path(&domain_id),
                Some(&dataset_body(
                    &format!("filler-{index}"),
                    "s3://clinic-coding-in/fill/",
                    "s3://clinic-coding-out/fill/",
                    None,
                )),
            )
            .await;
        assert_eq!(
            code, 201,
            "case \"filling the domain to its dataset bound\" (index {index}): got {code} with \
             {created}"
        );
    }
    let (code, refused) = emulator
        .send(
            "POST",
            &datasets_path(&domain_id),
            Some(&dataset_body(
                "one too many",
                "s3://clinic-coding-in/fill/",
                "s3://clinic-coding-out/fill/",
                None,
            )),
        )
        .await;
    assert_eq!(
        (code, error_code(&refused).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"one dataset past the bound\": expected the modelled quota refusal, got {code} \
         with {refused}"
    );
    let stated = string_at(&refused, "message");
    assert!(
        stated.contains(&MAX_DATASETS_PER_DOMAIN.to_string()),
        "case \"one dataset past the bound\": the refusal must name the count it enforced, got \
         {stated:?}"
    );
}

#[tokio::test]
async fn the_medical_coding_job_pair_persists_and_projects_a_job() {
    let emulator = Emulator::new("us-east-2");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-coding" }))
            .await,
        "domainId",
    );

    // An ACTIVE dataset to code against, built by the clock exactly as a caller waits for
    // it.
    let (code, dataset) = emulator
        .send(
            "POST",
            &datasets_path(&domain_id),
            Some(&dataset_body(
                "coding-examples",
                "s3://clinic-coding-in/examples/",
                "s3://clinic-coding-out/scores/",
                None,
            )),
        )
        .await;
    assert_eq!(
        code, 201,
        "CreateDataset must answer 201, got {code} with {dataset}"
    );
    let dataset_id = string_at(&dataset, "datasetId");
    poll_until_dataset_status(
        &emulator,
        "the dataset the jobs code against",
        &domain_id,
        &dataset_id,
        "ACTIVE",
    )
    .await;

    // The rows differ along what the rule generalises over: whether a dataset is named,
    // which optional context members the start carries, and the shape of the configured
    // output path.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no dataset and no context: the general knowledge base",
            coding_job_body(
                "Patient reports a sore throat for three days.",
                "s3://clinic-coding-out",
                None,
            ),
        ),
        (
            "a dataset, both contexts and a deep output prefix",
            {
                let mut body = coding_job_body(
                    "Established patient, 20 minute telehealth visit.",
                    "s3://clinic-coding-out/2026/01/codes/",
                    Some(&dataset_id),
                );
                body["patientContext"] = json!({
                    "sex": "FEMALE",
                    "status": "ESTABLISHED",
                    "dateOfBirth": "1979-04-02",
                    "insurance": "Example Health PPO",
                });
                body["encounterContext"] = json!({
                    "encounterType": "TELEHEALTH",
                    "encounterReason": "Sore throat",
                    "afterHours": true,
                    "encounterDate": "2026-01-14",
                    "encounterDurationMinutes": 20,
                });
                body
            },
        ),
        (
            "a client token and one context only",
            {
                let mut body = coding_job_body(
                    "New patient, in person, 45 minutes.",
                    "s3://clinic-coding-out/codes",
                    None,
                );
                body["clientToken"] = json!("coding-token-one");
                body["encounterContext"] = json!({ "encounterType": "IN_PERSON" });
                body
            },
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, body) in &rows {
        let (code, started) = emulator
            .send("POST", &coding_jobs_path(&domain_id), Some(body))
            .await;
        assert_eq!(
            code, 200,
            "case {case:?}: StartMedicalCodingJob must answer 200, got {code} with {started}"
        );
        let job_id = string_at(&started, "jobId");
        assert!(
            !job_id.is_empty() && job_id.chars().count() <= 36,
            "case {case:?}: a minted jobId must be inside the modelled JobId bound of 1..36, \
             got {job_id:?}"
        );
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: every accepted start must mint a fresh jobId, got the reused \
             {job_id:?}"
        );
        let job_arn = string_at(&started, "jobArn");
        assert!(
            job_arn.ends_with(&format!("domain/{domain_id}/medical-coding-job/{job_id}")),
            "case {case:?}: the jobArn must carry the id this start minted, got {job_arn:?}"
        );
        // `creationTime` is modelled `@timestampFormat("date-time")`, so it travels as an
        // RFC 3339 STRING rather than as the protocol's epoch number.
        let created_at = started["creationTime"].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: creationTime is modelled @timestampFormat(\"date-time\"), so it \
                 must be an RFC 3339 string, got {started}"
            )
        });

        // The reader PROJECTS the stored record, including the parameters the start
        // persisted.
        let (code, fetched) = emulator
            .send("GET", &coding_job_path(&domain_id, &job_id), None)
            .await;
        assert_eq!(
            code, 200,
            "case {case:?}: GetMedicalCodingJob must answer 200, got {code} with {fetched}"
        );
        assert_eq!(
            (
                fetched["jobId"].as_str(),
                fetched["jobArn"].as_str(),
                fetched["text"].as_str(),
                fetched["datasetId"].as_str(),
                fetched["creationTime"].as_str(),
            ),
            (
                Some(job_id.as_str()),
                Some(job_arn.as_str()),
                body["text"].as_str(),
                body["datasetId"].as_str(),
                Some(created_at),
            ),
            "case {case:?}: the reader must report what the start persisted, got {fetched}"
        );
        for member in ["outputDataConfig", "patientContext", "encounterContext"] {
            assert_eq!(
                fetched[member], body[member],
                "case {case:?}: {member} must be reported exactly as the start persisted it, \
                 got {fetched}"
            );
        }
        assert!(
            matches!(
                fetched["jobStatus"].as_str(),
                Some("SUBMITTED") | Some("IN_PROGRESS") | Some("SUCCEEDED")
            ),
            "case {case:?}: a started job reports a modelled JobStatus, got {fetched}"
        );

        // The transitions the CLOCK owes, waited for by LOOKING for them, and the location
        // the completed job reports — derived from the path the start configured and this
        // job's own id. The OBJECT at that location is asserted in the router suite, where
        // the service that owns it is mounted.
        let done = poll_until_job_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            done["jobStatus"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: a completed job must report the modelled success status, got {done}"
        );
        let configured = body["outputDataConfig"]["s3OutputPath"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            done["medicalCodingOutput"],
            json!({
                "uri": format!(
                    "{}/{job_id}/{}",
                    configured.trim_end_matches('/'),
                    state::MEDICAL_CODES_OBJECT_NAME
                ),
            }),
            "case {case:?}: a completed job must report the codes location it wrote, got {done}"
        );
        assert!(
            done["updatedTime"].as_str().is_some(),
            "case {case:?}: updatedTime is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string, got {done}"
        );
    }

    // A job is SCOPED to its domain: only the (domainId, jobId) pair it was started under
    // resolves it.
    let sibling = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-coding-sibling" }))
            .await,
        "domainId",
    );
    let stranded = minted.iter().next().cloned().expect("a job to address");
    let (code, refused) = emulator
        .send("GET", &coding_job_path(&sibling, &stranded), None)
        .await;
    assert_eq!(
        (code, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a job addressed through another domain\": expected the modelled 404, got {code} \
         with {refused}"
    );

    // And the dataset a start names has to be one of the domain's own, and ACTIVE.
    let creating = "seededcreating";
    seed_dataset(&emulator, &domain_id, creating, DatasetStatus::Creating);
    for (case, named, expected) in [
        (
            "a datasetId nothing minted",
            "nosuchdataset".to_string(),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a dataset that is not yet ACTIVE",
            creating.to_string(),
            (409, "ConflictException"),
        ),
    ] {
        let (code, refused) = emulator
            .send(
                "POST",
                &coding_jobs_path(&domain_id),
                Some(&coding_job_body(
                    "Patient reports a sore throat.",
                    "s3://clinic-coding-out",
                    Some(&named),
                )),
            )
            .await;
        assert_eq!(
            (code, error_code(&refused).as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {code} with {refused}"
        );
    }
}

#[tokio::test]
async fn submit_feedback_records_what_it_accepts_and_ignores_a_repeated_token() {
    let emulator = Emulator::new("eu-central-1");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-feedback" }))
            .await,
        "domainId",
    );
    let (code, subscription) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(
        code, 201,
        "CreateSubscription must answer 201, got {code} with {subscription}"
    );
    let subscription_id = string_at(&subscription, "subscriptionId");

    /// How many submissions the domain holds, read straight out of the shared engine — the
    /// only way to assert what an operation whose response declares no members
    /// established.
    fn recorded(emulator: &Emulator, domain_id: &str) -> usize {
        let store = state::service_state(&**emulator.engine()).expect("this service's own store");
        let store = store.lock();
        store
            .held_domain(emulator.region(), domain_id)
            .map(|domain| domain.feedback.len())
            .unwrap_or_default()
    }

    let feedback_path = format!("/domain/{domain_id}/feedback");
    // `ArtifactSourceId` is `@length 36..36` with the UUID pattern, so every row names one;
    // the rows differ along the artifact type, whether a subscription, a generated artifact
    // and comments are carried, and whether a client token is.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a clinical document, with every optional member",
            json!({
                "artifactSourceId": "4f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8",
                "artifactType": "CLINICAL_DOC",
                "signedArtifactS3Uri": "s3://clinic-artifacts/signed/note.pdf",
                "generatedArtifactS3Uri": "s3://clinic-artifacts/generated/note.pdf",
                "subscriptionId": subscription_id,
                "comments": "The note matched the encounter.",
                "clientToken": "feedback-token-one",
            }),
        ),
        (
            "medical codes, required members only",
            json!({
                "artifactSourceId": "0123abcd-4567-89ef-0123-456789abcdef",
                "artifactType": "MEDICAL_CODES",
                "signedArtifactS3Uri": "s3://clinic-artifacts/signed/codes.json",
            }),
        ),
        (
            "medical codes under a subscription, no token",
            json!({
                "artifactSourceId": "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
                "artifactType": "MEDICAL_CODES",
                "signedArtifactS3Uri": "s3://clinic-artifacts/signed/codes-2.json",
                "subscriptionId": subscription_id,
                "comments": "Two codes were missing.",
            }),
        ),
    ];

    for (case, body) in &rows {
        let before = recorded(&emulator, &domain_id);
        let (code, accepted) = emulator.send("POST", &feedback_path, Some(body)).await;
        assert_eq!(
            code, 202,
            "case {case:?}: SubmitFeedback must answer 202, got {code} with {accepted}"
        );
        assert_eq!(
            recorded(&emulator, &domain_id),
            before + 1,
            "case {case:?}: an accepted submission must be recorded"
        );
    }

    // A repeat of the tokened submission is ignored and still accepted, which is what this
    // operation's own documentation states of that member.
    let before = recorded(&emulator, &domain_id);
    let (code, repeated) = emulator
        .send("POST", &feedback_path, Some(&rows[0].1))
        .await;
    assert_eq!(
        code, 202,
        "case \"a repeated clientToken\": the repeat must still be accepted, got {code} with \
         {repeated}"
    );
    assert_eq!(
        recorded(&emulator, &domain_id),
        before,
        "case \"a repeated clientToken\": the repeat must record nothing further"
    );

    // The two resources a submission names have to resolve: the domain, and the
    // subscription when it names one.
    let (code, refused) = emulator
        .send(
            "POST",
            "/domain/dom-0123456789abcdef0123/feedback",
            Some(&rows[1].1),
        )
        .await;
    assert_eq!(
        (code, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a domain that resolves to nothing\": expected the modelled 404, got {code} with \
         {refused}"
    );

    let mut names_a_stranger = rows[1].1.clone();
    names_a_stranger["subscriptionId"] = json!("sub-000000000000000000000");
    let (code, refused) = emulator
        .send("POST", &feedback_path, Some(&names_a_stranger))
        .await;
    assert_eq!(
        (code, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a subscription the domain does not hold\": expected the modelled 404, got \
         {code} with {refused}"
    );
}
