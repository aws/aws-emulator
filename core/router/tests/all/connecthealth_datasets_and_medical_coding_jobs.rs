// SPDX-License-Identifier: CC0-1.0

//! The OBJECT a completed connecthealth medical coding job leaves behind, asserted on the
//! ASSEMBLED router — the arrangement a client actually talks to, where connecthealth and
//! the object store it writes through are mounted on one engine.
//!
//! The RULE under test: a job that ran to completion reports the codes document it wrote as
//! `medicalCodingOutput.uri`, and that object IS THERE — read back through Amazon S3's own
//! `GetObject`, the service that OWNS it, which is why this suite lives here rather than in
//! either service's crate. The document states the RUN (which job, when it finished, the
//! dataset its codes were tailored to, how much text it was given) and reports an empty code
//! set, because this emulator runs no coding model and so says it carries no codes rather
//! than codes nothing produced.
//!
//! The rows differ along what the rule generalises over: whether the start names a dataset,
//! and the shape of the configured output path — a bucket root, a deep prefix, and a prefix
//! with no trailing separator.
//!
//! Everything about the pair that one service alone answers — the ids and ARNs it mints, the
//! parameters it persists and projects, the statuses the clock moves a job through, the
//! `@timestampFormat("date-time")` form of its instants, the scoping of a job to its domain
//! and the gating of the dataset it names — is asserted in the owning crate's own suite
//! (`services/service-com-amazonaws-connecthealth-connecthealth/tests/all/shared_due_materialisation/datasets_and_medical_coding_jobs.rs`).
//!
//! The harness (the signed dispatch, the assembled router, the domain every case is scoped
//! by) is the sibling suite's: one way of driving this service, not two. Assembling the
//! whole router costs stack, so the test builds it, and drives its requests, on a sized
//! thread.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connecthealth_connecthealth::state::MEDICAL_CODES_OBJECT_NAME;

use super::connecthealth_patient_insights_job_response_wire_form::{
    assembled, connecthealth, domain, send, BOOT_STACK_SIZE, S3,
};

/// The bucket every job of this suite is configured to write into.
const BUCKET: &str = "clinic-coding-out";

/// How long a poll keeps LOOKING for a condition before it gives up: many times over the
/// spans this emulator's own lifecycles take, so a loaded host has room while a fast one
/// never waits at all.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll pauses between two reads: short enough that the condition is observed as
/// soon as it holds, long enough that the loop does not spin a core flat while it waits. It
/// is not a bound on anything — [`POLL_DEADLINE`] is.
const BETWEEN_READS: Duration = Duration::from_millis(20);

/// `POST /domains/{domainId}/datasets` — `CreateDataset`'s route.
fn datasets_path(domain_id: &str) -> String {
    format!("/domains/{domain_id}/datasets")
}

/// `GET /domains/{domainId}/datasets/{datasetId}` — the dataset reader's route.
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

/// Look at a dataset through its own reader until it reports `ACTIVE` — the status the clock
/// owes a dataset the create accepted, and the one the model requires of a dataset a coding
/// job codes against.
fn poll_until_dataset_active(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    domain_id: &str,
    dataset_id: &str,
) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let fetched = connecthealth(
            router,
            runtime,
            "GET",
            &dataset_path(domain_id, dataset_id),
            None,
        );
        if fetched.status == 200 && fetched.body["status"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the dataset was still not ACTIVE after {POLL_DEADLINE:?}, last read {} with {:?}",
            fetched.status,
            fetched.body
        );
        std::thread::sleep(BETWEEN_READS);
    }
}

/// Look at a medical coding job through its own reader until it reports a terminal
/// `JobStatus`, and answer that reading — or fail once [`POLL_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for, never an interval: a transition is owed at an
/// instant, so looking again is how a caller observes it.
fn poll_until_job_terminal(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    domain_id: &str,
    job_id: &str,
) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let fetched = connecthealth(
            router,
            runtime,
            "GET",
            &coding_job_path(domain_id, job_id),
            None,
        );
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetMedicalCodingJob must answer 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        if matches!(
            fetched.body["jobStatus"].as_str(),
            Some("SUCCEEDED") | Some("FAILED")
        ) {
            return fetched.body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job was still not terminal after {POLL_DEADLINE:?}, last read \
             {:?}",
            fetched.body
        );
        std::thread::sleep(BETWEEN_READS);
    }
}

#[test]
fn a_completed_medical_coding_job_stores_the_object_its_response_reports() {
    std::thread::Builder::new()
        .name("connecthealth-medical-coding-objects".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let (router, runtime) = assembled();
            let created = send(
                &router,
                &runtime,
                S3,
                "PUT",
                &format!("/{BUCKET}"),
                &[],
                Vec::new(),
            );
            assert_eq!(
                created.status, 200,
                "CreateBucket must answer 200, got {} with {:?}",
                created.status, created.body
            );
            let (domain_id, _) = domain(&router, &runtime, "clinic-coding-objects");

            // An ACTIVE dataset to code against, created and waited for the way a caller
            // does, so the row that names one names a dataset this service really holds.
            let dataset = connecthealth(
                &router,
                &runtime,
                "POST",
                &datasets_path(&domain_id),
                Some(json!({
                    "name": "coding-examples",
                    "config": {
                        "medicalCoding": {
                            "s3InputPath": "s3://clinic-coding-in/examples/",
                            "s3OutputPath": format!("s3://{BUCKET}/scores/"),
                        }
                    },
                })),
            );
            assert_eq!(
                dataset.status, 201,
                "CreateDataset must answer 201, got {} with {:?}",
                dataset.status, dataset.body
            );
            let dataset_id = dataset.string_at("datasetId");
            poll_until_dataset_active(&router, &runtime, &domain_id, &dataset_id);

            let rows: Vec<(&str, Option<&str>, String)> = vec![
                (
                    "no dataset, into a bucket root",
                    None,
                    format!("s3://{BUCKET}"),
                ),
                (
                    "a dataset, into a prefix with a trailing separator",
                    Some(dataset_id.as_str()),
                    format!("s3://{BUCKET}/2026/01/codes/"),
                ),
                (
                    "a dataset, into a prefix with no trailing separator",
                    Some(dataset_id.as_str()),
                    format!("s3://{BUCKET}/codes"),
                ),
            ];

            let mut written: BTreeSet<String> = BTreeSet::new();
            for (case, dataset, output_path) in &rows {
                let mut body = json!({
                    "text": "Established patient, 20 minute telehealth visit.",
                    "outputDataConfig": { "s3OutputPath": output_path },
                });
                if let Some(dataset) = dataset {
                    body["datasetId"] = json!(dataset);
                }
                let started = connecthealth(
                    &router,
                    &runtime,
                    "POST",
                    &coding_jobs_path(&domain_id),
                    Some(body),
                );
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: StartMedicalCodingJob must answer 200, got {} with {:?}",
                    started.status, started.body
                );
                let job_id = started.string_at("jobId");

                let done = poll_until_job_terminal(&router, &runtime, case, &domain_id, &job_id);
                assert_eq!(
                    done["jobStatus"].as_str(),
                    Some("SUCCEEDED"),
                    "case {case:?}: a job whose output was stored must report the modelled \
                     success status, got {done}"
                );

                // The key the document sits under, by the rule: the configured path with any
                // trailing separator collapsed, then the job's own id and the object name.
                let expected_uri = format!(
                    "{}/{job_id}/{MEDICAL_CODES_OBJECT_NAME}",
                    output_path.trim_end_matches('/')
                );
                assert_eq!(
                    done["medicalCodingOutput"],
                    json!({ "uri": expected_uri }),
                    "case {case:?}: a completed job must report the codes object it wrote, got \
                     {done}"
                );
                assert!(
                    written.insert(expected_uri.clone()),
                    "case {case:?}: two jobs must not write over one another, got the reused \
                     {expected_uri:?}"
                );

                // And the object is where it says it is, read back through the service that
                // OWNS it.
                let key = expected_uri
                    .strip_prefix(&format!("s3://{BUCKET}/"))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {expected_uri} must name an object in {BUCKET}")
                    });
                let object = send(
                    &router,
                    &runtime,
                    S3,
                    "GET",
                    &format!("/{BUCKET}/{key}"),
                    &[],
                    Vec::new(),
                );
                assert_eq!(
                    object.status, 200,
                    "case {case:?}: the object at {expected_uri} must exist, got {} with {}",
                    object.status,
                    String::from_utf8_lossy(&object.bytes)
                );
                let document: Value =
                    serde_json::from_slice(&object.bytes).unwrap_or_else(|error| {
                        panic!("case {case:?}: the stored document must be JSON: {error}")
                    });
                assert_eq!(
                    (
                        document["JobId"].as_str(),
                        document["DatasetId"].as_str(),
                        document["MedicalCodes"]["TotalCount"].as_u64(),
                        document["InputCharacterCount"].as_u64(),
                    ),
                    (
                        Some(job_id.as_str()),
                        *dataset,
                        Some(0),
                        Some("Established patient, 20 minute telehealth visit.".len() as u64),
                    ),
                    "case {case:?}: the stored document must state the run this job reports, \
                     got {document}"
                );
                assert!(
                    document["GeneratedAt"].as_str().is_some(),
                    "case {case:?}: the stored document must state when the run finished, got \
                     {document}"
                );
            }
        })
        .expect("spawn the medical coding object thread")
        .join()
        .expect("a completed medical coding job must store the object its response reports");
}
