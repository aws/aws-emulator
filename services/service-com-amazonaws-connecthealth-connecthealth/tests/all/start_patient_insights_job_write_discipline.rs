// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHAT `StartPatientInsightsJob` writes.
//!
//! The sibling suite (`patient_insights_job_lifecycle`) pins what an ACCEPTED start
//! persists and what `GetPatientInsightsJob` projects back out of it. This one pins the
//! write discipline around it, asserted over tables rather than one request:
//!
//! 1. A start writes a `PatientInsightsJob` record ONLY when it is accepted: whatever
//!    refuses it — an identifier that resolves to no live domain, a domain addressed
//!    through another Region's endpoint, a domain on its way out, a request the model
//!    rejects, or a `clientToken` already bound to different parameters — the store's
//!    jobs are exactly the records, with exactly the identifiers, creation instants,
//!    parameters and token bindings, that they were before the call. Which refusal each
//!    earns is the patient-insights runtime endpoint's: a `domainId` it cannot resolve a
//!    live domain for is its authorization failure (401 `AccessDeniedException`), and a
//!    value of the request outside its shape is the modelled 400 `ValidationException`.
//! 2. A start is IDEMPOTENT on `clientToken` at every point of a job's life: a retry
//!    answers the identifiers and creation instant of the job the first call stored,
//!    starts no second job, and resets nothing the clock has since moved on — so the
//!    record a reader sees is identical before and after the retry.
//!
//! The job's own store is inspected through the SHARED engine the service is mounted on,
//! which is the only way to assert that a refused call wrote NOTHING (this service models
//! no operation that lists jobs).
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the two routes, the reader and the
//! web-app setup that keeps a domain in `DELETING` are the sibling suites': one way of
//! driving this service, not two.
//!
//! Constraints quoted beside each row are the model's own
//! (`smithy_query(query="closure",operation="StartPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, PATIENT_INSIGHTS_JOB_STEP_SPAN,
};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{get_job, start_path};

/// A `StartPatientInsightsJob` body carrying the required members only.
fn job_request() -> Value {
    json!({
        "patientContext": { "patientId": "patient-7781" },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Medication review" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-7781" },
        "inputDataConfig": { "s3Sources": [ { "uri": "s3://clinic-input/patient-7781/" } ] },
        "outputDataConfig": { "s3OutputPath": "s3://clinic-output/insights/" },
    })
}

/// A domain to start jobs in, created through `CreateDomain` with the request given (the
/// sibling suite's helper takes a name; a row here needs a web app as well).
async fn domain_with(emulator: &Emulator, request: &Value) -> String {
    string_at(&emulator.create_domain(request).await, "domainId")
}

/// Everything `StartPatientInsightsJob` OWNS in this service's store, read straight out
/// of the shared engine: one line per job record and one per `clientToken` binding.
///
/// Across EVERY domain, including one in `DELETING`: a refused start must write nothing
/// anywhere, not merely nothing under the domain it named. Each line carries what the
/// START decided and the clock never touches — the identifiers, the ARN, the creation
/// instant and the digest of the parameters the job was stored with — so a refusal that
/// overwrote an existing record in place is caught too, while the status and
/// last-updated instant the CLOCK owns are deliberately left out (those move on their
/// own, call or no call).
fn stored_jobs(emulator: &Emulator) -> BTreeSet<String> {
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let store = store.lock();
    let mut lines = BTreeSet::new();
    for domain in store.domains.values() {
        for (job_id, job) in &domain.patient_insights_jobs {
            let created = job
                .created_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            lines.insert(format!(
                "job {}/{job_id} arn={} created={created}ns spec={}",
                domain.domain_id,
                job.arn,
                job.spec.fingerprint()
            ));
        }
        for (token, job_id) in &domain.patient_insights_job_ids_by_client_token {
            lines.insert(format!(
                "token {}/{token} -> {job_id}",
                domain.domain_id
            ));
        }
    }
    lines
}

#[tokio::test]
async fn a_refused_start_writes_no_job_record() {
    let home = "us-west-2";
    let elsewhere = "eu-central-1";
    let emulator = Emulator::new(home);
    let domain_id = domain_with(&emulator, &json!({ "name": "clinic-write-discipline" })).await;

    // One accepted start, so the refusals below are asserted against a store that
    // already holds a record: a refused call must leave THAT record alone too, rather
    // than being asserted only against an empty store.
    let mut accepted = job_request();
    accepted["clientToken"] = json!("token-write-discipline");
    let (status, started) = emulator
        .send("POST", &start_path(&domain_id), Some(&accepted))
        .await;
    assert_eq!(
        status, 200,
        "the first start must be accepted, got {status} with {started}"
    );
    let job_id = string_at(&started, "jobId");

    // A domain whose deletion cannot finish inside the deleting call, so it is still
    // held in `DELETING` when the row below addresses it.
    let deleting = domain_with(
        &emulator,
        &json!({ "name": "clinic-deleting", "webAppSetupConfiguration": web_app_setup() }),
    )
    .await;
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{deleting}"), None)
        .await;
    assert_eq!(
        status, 200,
        "DeleteDomain must be accepted, got {status} with {deleted}"
    );

    let before = stored_jobs(&emulator);
    assert_eq!(
        before.len(),
        2,
        "the accepted start must have written one record and one token binding, got {before:?}"
    );
    assert!(
        before
            .iter()
            .any(|line| line.contains(&format!("job {domain_id}/{job_id} "))),
        "the accepted start's record must be held under its domain, got {before:?}"
    );

    // A reused token with different parameters: the same token, one changed member.
    let mut conflicting = accepted.clone();
    conflicting["patientContext"] = json!({ "patientId": "patient-0002" });
    // A request the MODEL rejects: `outputDataConfig` is @required, and `DomainId`
    // carries `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`.
    let mut incomplete = job_request();
    incomplete
        .as_object_mut()
        .expect("an object body")
        .remove("outputDataConfig");

    // The rows differ along what refuses the start: which identifier fails to resolve,
    // which Region the request is signed for, the domain's own lifecycle state, the
    // modelled shape of the request, and the idempotency token's binding.
    let rows: Vec<(&str, &str, String, Value, u16, &str)> = vec![
        (
            "a domainId no domain holds",
            home,
            "dom-0123456789abcdef0".to_string(),
            job_request(),
            401,
            "AccessDeniedException",
        ),
        (
            "a live domain of another Region",
            elsewhere,
            domain_id.clone(),
            job_request(),
            401,
            "AccessDeniedException",
        ),
        (
            "a domain on its way out",
            home,
            deleting.clone(),
            job_request(),
            401,
            "AccessDeniedException",
        ),
        (
            "a domainId outside the modelled pattern",
            home,
            "clinic-not-an-id-0000".to_string(),
            job_request(),
            400,
            "ValidationException",
        ),
        (
            "a request missing a required member",
            home,
            domain_id.clone(),
            incomplete,
            400,
            "ValidationException",
        ),
        (
            "a clientToken already bound to other parameters",
            home,
            domain_id.clone(),
            conflicting,
            409,
            "ConflictException",
        ),
    ];

    for (case, region, domain, body, expected_status, expected_code) in rows {
        let (status, refused) = emulator
            .send_in(region, "POST", &start_path(&domain), Some(&body))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected the modelled {expected_status}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );
        let after = stored_jobs(&emulator);
        assert_eq!(
            after, before,
            "case {case:?}: a refused start must write no job record and change none, \
             expected {before:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_idempotent_retry_answers_the_stored_job_and_resets_nothing() {
    let emulator = Emulator::new("ap-southeast-2");
    let domain_id = domain_with(&emulator, &json!({ "name": "clinic-idempotent-clock" })).await;

    // Real elapsed time is required (the handlers read `SystemTime::now()`), and the
    // workspace `tokio` dependency does not enable the `time` feature, so these are
    // plain blocking sleeps. Half a span past each boundary, which is the point of a
    // status's window furthest from either edge: the transitions the clock owes have
    // fallen due and the next one is not.
    let slack = PATIENT_INSIGHTS_JOB_STEP_SPAN / 2;

    // The rows differ along HOW FAR the clock has moved the job when the retry arrives:
    // none of its spans elapsed, one, or both — the whole modelled lifecycle this
    // emulator's own jobs walk (`JobStatus`: SUBMITTED -> IN_PROGRESS -> SUCCEEDED).
    let rows: [(&str, u32, &str); 3] = [
        ("no span elapsed", 0, "SUBMITTED"),
        ("one span elapsed", 1, "IN_PROGRESS"),
        ("both spans elapsed", 2, "SUCCEEDED"),
    ];

    for (index, (case, spans, expected_status)) in rows.into_iter().enumerate() {
        let mut body = job_request();
        body["clientToken"] = json!(format!("token-{case}"));
        let (status, first) = emulator
            .send("POST", &start_path(&domain_id), Some(&body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first start must be accepted, got {status} with {first}"
        );
        let job_id = string_at(&first, "jobId");

        if spans > 0 {
            std::thread::sleep(PATIENT_INSIGHTS_JOB_STEP_SPAN * spans + slack);
        }

        // Where the clock has taken the job by the time the retry arrives.
        let before = get_job(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&before, "jobStatus"),
            expected_status,
            "case {case:?}: the clock must have moved the job to {expected_status}, got {before}"
        );
        if expected_status == "SUCCEEDED" {
            assert!(
                before.get("insightsOutput").is_some(),
                "case {case:?}: a completed job must report where its output went, got {before}"
            );
        }

        // The retry: the job the first call stored, answered again.
        let (status, retried) = emulator
            .send("POST", &start_path(&domain_id), Some(&body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry must be accepted, got {status} with {retried}"
        );
        assert_eq!(
            retried, first,
            "case {case:?}: a retry must answer the job the first call stored ({first}), \
             got {retried}"
        );

        // And it reset nothing: for the terminal row the record reads exactly as it did
        // before the retry, status, status detail and last-updated instant included. For
        // the non-terminal rows the clock may legitimately have moved the job on in
        // between, so what must not change is what the START owns — the identifiers, the
        // creation instant and the parameters.
        let after = get_job(&emulator, case, &domain_id, &job_id).await;
        if expected_status == "SUCCEEDED" {
            assert_eq!(
                after, before,
                "case {case:?}: a retry must leave a terminal record untouched ({before}), \
                 got {after}"
            );
        } else {
            for name in [
                "jobId",
                "jobArn",
                "creationTime",
                "patientContext",
                "insightsContext",
                "encounterContext",
                "userContext",
                "inputDataConfig",
                "outputDataConfig",
            ] {
                assert_eq!(
                    after.get(name),
                    before.get(name),
                    "case {case:?}: a retry must not change {name}, expected {before}, \
                     got {after}"
                );
            }
        }

        // No second job: the store holds one record and one token binding per accepted
        // start, and nothing more.
        let held = stored_jobs(&emulator);
        assert_eq!(
            held.len(),
            2 * (index + 1),
            "case {case:?}: a retry must start no second job, expected {} line(s) of stored \
             state, got {held:?}",
            2 * (index + 1)
        );
        assert!(
            held.iter()
                .any(|line| line.contains(&format!("job {domain_id}/{job_id} "))),
            "case {case:?}: the job the start minted must be held under its domain, \
             got {held:?}"
        );
    }
}
