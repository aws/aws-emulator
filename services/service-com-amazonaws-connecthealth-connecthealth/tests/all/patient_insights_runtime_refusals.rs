// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of how the patient-insights RUNTIME endpoint refuses a call.
//!
//! `StartPatientInsightsJob` and `GetPatientInsightsJob` are served by that endpoint
//! (`smithy_query(query="operation",operation="StartPatientInsightsJob,GetPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports `endpoint.hostPrefix` `runtime.` on both), and its refusals are its own. Three
//! rules, each asserted over a TABLE of inputs rather than one recorded request:
//!
//! 1. A START whose `domainId` resolves to no domain this account holds live in the
//!    Region is refused by that endpoint's AUTHORIZATION step: HTTP 401
//!    `AccessDeniedException` (a modelled error of the operation) with the fixed sentence
//!    `state::error_detail::DOMAIN_NOT_AUTHORIZED_MESSAGE` — carrying nothing read off the
//!    request, so probing ids through this endpoint tells a caller nothing about which
//!    ones exist. Every way a `domainId` can fail to resolve is a row: an id this service
//!    never allocated (both modelled prefixes, both ends of the modelled length), one it
//!    allocated and no longer holds, a live domain of ANOTHER Region, and one on its way
//!    out.
//! 2. A START whose `outputDataConfig.s3OutputPath` falls outside the rules its own shape
//!    declares is the modelled 400 `ValidationException` with that endpoint's fixed
//!    request-validation sentence (`state::error_detail::REQUEST_VALIDATION_FAILED_MESSAGE`),
//!    and it PERSISTS nothing: no job, and no binding of the `clientToken` it carried, so
//!    an identical retry is refused again rather than answered idempotently and a later
//!    valid request may still use that token. A path the shape DOES allow is accepted at
//!    the same time, so the rule is not asserted only on its refusing side.
//! 3. A `jobId` that resolves to no job under a domain that DOES resolve stays the
//!    modelled 404 `ResourceNotFoundException`, and names the job by the ARN this service
//!    would hold it under: `Job <jobArn> does not exist.`
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the two routes, the reader, the
//! minimal request body and the web-app setup that keeps a domain in `DELETING` are the
//! sibling suites': one way of driving this service, not two.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::error_detail::{
    DOMAIN_NOT_AUTHORIZED_MESSAGE, REQUEST_VALIDATION_FAILED_MESSAGE,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{self, MAX_S3_URI_LEN};

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{job_path, minimal_body, start_path};

/// The refusal the runtime endpoint's authorization step answers
/// (`smithy_query(query="operation",operation="StartPatientInsightsJob")`:
/// `AccessDeniedException`, `httpError` 401).
const NOT_AUTHORIZED: (u16, &str) = (401, "AccessDeniedException");

/// The refusal a value of the request outside its shape earns
/// (`smithy_query(query="operation",operation="StartPatientInsightsJob")`:
/// `ValidationException`, `httpError` 400).
const INVALID_REQUEST: (u16, &str) = (400, "ValidationException");

/// The refusal a resource that does not resolve under a domain that does earns
/// (`smithy_query(query="operation",operation="GetPatientInsightsJob")`:
/// `ResourceNotFoundException`, `httpError` 404).
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// How long [`poll_until_unheld`] keeps looking: many times over the span
/// `state::DOMAIN_DELETION_SPAN` owes, so a loaded host has room while a fast one never
/// waits at all.
const PURGE_DEADLINE: Duration = Duration::from_secs(30);

/// Look at `GET /domain/{domainId}` until this Region holds no record under it — the
/// purge the clock owes a deleted domain, materialised by whichever call reads the store
/// next — or fail once [`PURGE_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for, not an interval: the transition is owed at an
/// instant, so looking again is how a caller observes it, and a fixed wait would make the
/// outcome depend on how fast the host runs.
async fn poll_until_unheld(emulator: &Emulator, domain_id: &str) {
    let deadline = std::time::Instant::now() + PURGE_DEADLINE;
    loop {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        if status == 404 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the record under {domain_id} was still held after {PURGE_DEADLINE:?}, last read \
             {status} with {body}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The `Message` member of a response body, which an error body must carry.
fn detail(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the error body must carry a Message, got {body}"))
        .to_string()
}

/// A well-formed `DomainId` that names nothing: `prefix` padded with `filler` to `len`.
///
/// `DomainId` is `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="DomainId")`), so both modelled prefixes and both
/// ends of the modelled length are reachable this way.
fn unallocated_domain_id(prefix: &str, filler: char, len: usize) -> String {
    let mut id = prefix.to_string();
    while id.len() < len {
        id.push(filler);
    }
    id
}

/// Every job record and `clientToken` binding this service holds, read straight out of
/// the shared engine — the only way to assert that a refused start persisted NOTHING,
/// since this service models no operation that lists jobs.
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
                "job {}/{job_id} arn={} created={created}ns",
                domain.domain_id, job.arn
            ));
        }
        for (token, job_id) in &domain.patient_insights_job_ids_by_client_token {
            lines.insert(format!("token {}/{token} -> {job_id}", domain.domain_id));
        }
    }
    lines
}

#[tokio::test]
async fn a_start_whose_domain_does_not_resolve_is_refused_by_the_endpoints_authorizer() {
    let home = "us-west-2";
    let elsewhere = "eu-central-1";
    let emulator = Emulator::new(home);

    // The positive control: a live domain in the signed Region, whose start is accepted.
    // Every refusal below is therefore answered by a service that does hold domains and
    // does serve this operation.
    let live = emulator
        .create_domain(&json!({ "name": "clinic-runtime-live" }))
        .await;
    let live_domain = string_at(&live, "domainId");
    let (status, started) = emulator
        .send("POST", &start_path(&live_domain), Some(&minimal_body()))
        .await;
    assert_eq!(
        status, 200,
        "the control start must be accepted, got {status} with {started}"
    );
    let control_job = string_at(&started, "jobId");

    // An id this service allocated and no longer holds: created, deleted, and purged once
    // the deletion the clock owed fell due.
    let purged = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-runtime-purged" }))
            .await,
        "domainId",
    );
    let (status, body) = emulator
        .send("DELETE", &format!("/domain/{purged}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} with {body}");

    // A domain whose deletion cannot finish inside the deleting call (it owns a web
    // application), so it is still HELD, in `DELETING`, when the row addresses it.
    let deleting = string_at(
        &emulator
            .create_domain(&json!({
                "name": "clinic-runtime-deleting",
                "webAppSetupConfiguration": web_app_setup(),
            }))
            .await,
        "domainId",
    );
    let (status, body) = emulator
        .send("DELETE", &format!("/domain/{deleting}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} with {body}");

    // A live domain of ANOTHER Region: it resolves through its own endpoint and nowhere
    // else, so to this Region's runtime endpoint it is an id that names no domain.
    let other_region = string_at(
        &emulator
            .create_domain_in(elsewhere, &json!({ "name": "clinic-runtime-elsewhere" }))
            .await,
        "domainId",
    );

    // The purge the clock owes the deleted domain is waited for by LOOKING for it: the
    // record is gone when the Region holds nothing under its id, which is what the row
    // below addresses. Never by sleeping for the span — a wait measured in wall-clock time
    // passes on one machine and fails on a loaded or a faster one.
    poll_until_unheld(&emulator, &purged).await;

    // One row per way a `domainId` can fail to resolve to a live domain of this Region.
    let rows: Vec<(String, String)> = vec![
        (
            "a never-allocated id at the shortest modelled length".to_string(),
            unallocated_domain_id("dom-", 'a', 20),
        ),
        (
            "a never-allocated id at the longest modelled length".to_string(),
            unallocated_domain_id("dom-", 'z', 25),
        ),
        (
            "a never-allocated id of the other modelled prefix".to_string(),
            unallocated_domain_id("hai-", '0', 21),
        ),
        (
            "a never-allocated id of digits".to_string(),
            unallocated_domain_id("dom-", '3', 24),
        ),
        (
            "an id this service allocated and no longer holds".to_string(),
            purged.clone(),
        ),
        (
            "a domain on its way out".to_string(),
            deleting.clone(),
        ),
        (
            "a live domain of another Region".to_string(),
            other_region.clone(),
        ),
    ];

    for (case, domain_id) in &rows {
        let (status, refused) = emulator
            .send("POST", &start_path(domain_id), Some(&minimal_body()))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            NOT_AUTHORIZED,
            "case {case:?} ({domain_id}): expected {NOT_AUTHORIZED:?}, got {status} with \
             {refused}"
        );
        let stated = detail(case, &refused);
        assert_eq!(
            stated, DOMAIN_NOT_AUTHORIZED_MESSAGE,
            "case {case:?}: expected the endpoint's fixed sentence \
             {DOMAIN_NOT_AUTHORIZED_MESSAGE:?}, got {stated:?}"
        );
        // Nothing read off the request may appear anywhere in the body: a sentence that
        // named the requested id would tell an unauthorized caller which ids exist.
        let rendered = refused.to_string();
        for (member, value) in [
            ("domainId", domain_id.as_str()),
            ("patientId", "patient-0001"),
            ("s3OutputPath", "s3://clinic-output/insights/"),
        ] {
            assert!(
                !rendered.contains(value),
                "case {case:?}: the refusal must not echo the request's {member}, got \
                 {rendered}"
            );
        }
    }

    // The control job is untouched by any of it: the refusals above neither served nor
    // disturbed a resource of the domain that does resolve.
    let (status, fetched) = emulator
        .send("GET", &job_path(&live_domain, &control_job), None)
        .await;
    assert_eq!(
        (status, string_at(&fetched, "jobId").as_str()),
        (200, control_job.as_str()),
        "the control job must still be readable, got {status} with {fetched}"
    );
}

/// A `StartPatientInsightsJob` body whose output location is `s3_output_path`, carrying
/// `client_token` so what a refusal bound (or did not bind) can be asserted.
fn body_with_output(s3_output_path: &str, client_token: &str) -> Value {
    let mut body = minimal_body();
    body["outputDataConfig"] = json!({ "s3OutputPath": s3_output_path });
    body["clientToken"] = json!(client_token);
    body
}

/// An `s3://` URI of exactly `len` characters, built on a bucket the modelled pattern
/// allows and padded in its key.
fn s3_uri_of_length(len: usize) -> String {
    let prefix = "s3://clinic-output/";
    let mut uri = prefix.to_string();
    while uri.chars().count() < len {
        uri.push('k');
    }
    uri
}

#[tokio::test]
async fn an_output_location_outside_its_shape_is_refused_and_persists_nothing() {
    let emulator = Emulator::new("eu-west-1");
    let domain_id = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-output-validation" }))
            .await,
        "domainId",
    );

    // One accepted start first, so every refusal below is asserted against a store that
    // already holds a record rather than against an empty one.
    let (status, accepted) = emulator
        .send(
            "POST",
            &start_path(&domain_id),
            Some(&body_with_output("s3://clinic-output/insights/", "token-accepted")),
        )
        .await;
    assert_eq!(
        status, 200,
        "the control start must be accepted, got {status} with {accepted}"
    );
    let before = stored_jobs(&emulator);

    // The rows come from `S3Uri`'s own rules
    // (`smithy_query(query="shape",shape="S3Uri,OutputDataConfig")`: `@length max 1024`,
    // pattern `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` — an `s3://` scheme and a
    // bucket of 3 to 63 characters of lowercase letters and digits, which dots and hyphens
    // may appear inside but not at either end), one row per way a value can fall outside
    // them, including each published bound and one character past it.
    let refused_rows: Vec<(String, String)> = vec![
        ("not a URI at all".to_string(), "not-an-s3-uri".to_string()),
        (
            "another scheme".to_string(),
            "https://clinic-output/insights/".to_string(),
        ),
        ("the scheme alone".to_string(), "s3://".to_string()),
        (
            "a bucket one character short of the published minimum".to_string(),
            "s3://ab".to_string(),
        ),
        (
            "a bucket one character past the published maximum".to_string(),
            format!("s3://{}", "b".repeat(64)),
        ),
        (
            "an uppercase bucket".to_string(),
            "s3://Clinic-Output/insights/".to_string(),
        ),
        (
            "a bucket ending in a separator character".to_string(),
            "s3://clinic-output./insights/".to_string(),
        ),
        (
            "one character past the modelled length bound".to_string(),
            s3_uri_of_length(MAX_S3_URI_LEN + 1),
        ),
        (
            "an empty output location".to_string(),
            String::new(),
        ),
    ];

    for (case, path) in &refused_rows {
        // Twice with the SAME token: a refused request binds nothing, so the retry is
        // refused in exactly the same words rather than answered with a job.
        for attempt in ["first attempt", "retry of the same request"] {
            let (status, refused) = emulator
                .send(
                    "POST",
                    &start_path(&domain_id),
                    Some(&body_with_output(path, "token-refused")),
                )
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                INVALID_REQUEST,
                "case {case:?} ({attempt}): expected {INVALID_REQUEST:?}, got {status} \
                 with {refused}"
            );
            let stated = detail(case, &refused);
            assert_eq!(
                stated, REQUEST_VALIDATION_FAILED_MESSAGE,
                "case {case:?} ({attempt}): expected the endpoint's fixed sentence \
                 {REQUEST_VALIDATION_FAILED_MESSAGE:?}, got {stated:?}"
            );
        }
        let after = stored_jobs(&emulator);
        assert_eq!(
            after, before,
            "case {case:?}: a refused start must persist no job and no token binding, \
             expected {before:?}, got {after:?}"
        );
    }

    // The token the refusals carried was never bound, so a VALID request presenting it is
    // a new job rather than an idempotent replay of something that was refused.
    let (status, started) = emulator
        .send(
            "POST",
            &start_path(&domain_id),
            Some(&body_with_output("s3://clinic-output/after/", "token-refused")),
        )
        .await;
    assert_eq!(
        status, 200,
        "a valid request reusing the refused token must be accepted, got {status} with \
         {started}"
    );
    let job_id = string_at(&started, "jobId");
    let (status, fetched) = emulator
        .send("GET", &job_path(&domain_id, &job_id), None)
        .await;
    assert_eq!(
        (status, string_at(&fetched, "jobId").as_str()),
        (200, job_id.as_str()),
        "the job that request started must be readable, got {status} with {fetched}"
    );

    // The other side of the same rule: every output location the shape DOES allow is
    // accepted — the published bucket-name bounds, the characters allowed inside a bucket
    // name, a bare bucket with no key, and a URI at exactly the modelled length bound.
    let accepted_rows: Vec<(String, String)> = vec![
        (
            "a bucket at the published minimum".to_string(),
            "s3://abc".to_string(),
        ),
        (
            "a bucket at the published maximum".to_string(),
            format!("s3://{}", "b".repeat(63)),
        ),
        (
            "dots and hyphens inside the bucket name".to_string(),
            "s3://clinic.out-put9/insights/".to_string(),
        ),
        (
            "a key path of several segments".to_string(),
            "s3://clinic-output/2026/01/insights/run.json".to_string(),
        ),
        (
            "exactly the modelled length bound".to_string(),
            s3_uri_of_length(MAX_S3_URI_LEN),
        ),
    ];

    for (index, (case, path)) in accepted_rows.iter().enumerate() {
        let (status, started) = emulator
            .send(
                "POST",
                &start_path(&domain_id),
                Some(&body_with_output(path, &format!("token-accepted-{index}"))),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an output location the shape allows must be accepted, got \
             {status} with {started}"
        );
        let configured = string_at(
            &emulator
                .send(
                    "GET",
                    &job_path(&domain_id, &string_at(&started, "jobId")),
                    None,
                )
                .await
                .1["outputDataConfig"],
            "s3OutputPath",
        );
        assert_eq!(
            &configured, path,
            "case {case:?}: the stored job must report the configured location, got \
             {configured:?}"
        );
    }
}

#[tokio::test]
async fn a_job_that_resolves_to_nothing_is_named_by_the_arn_it_would_hold() {
    let emulator = Emulator::new("ap-south-1");
    let created = emulator
        .create_domain(&json!({ "name": "clinic-job-lookup" }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let domain_arn = string_at(&created, "arn");

    // A real job in the domain, and one in a SIBLING domain, so a jobId that exists
    // somewhere else is a row of its own.
    let (status, started) = emulator
        .send("POST", &start_path(&domain_id), Some(&minimal_body()))
        .await;
    assert_eq!(status, 200, "the control start got {status} with {started}");
    let job_id = string_at(&started, "jobId");

    let sibling = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-job-sibling" }))
            .await,
        "domainId",
    );
    let (status, elsewhere) = emulator
        .send("POST", &start_path(&sibling), Some(&minimal_body()))
        .await;
    assert_eq!(status, 200, "the sibling start got {status} with {elsewhere}");
    let sibling_job = string_at(&elsewhere, "jobId");

    // `JobId` is `@length 1..36` (`smithy_query(query="shape",shape="JobId")`), so the
    // rows span both ends of that bound as well as an id this service minted under
    // another domain.
    let rows: Vec<(String, String)> = vec![
        ("a one-character jobId".to_string(), "x".to_string()),
        (
            "a jobId at the longest modelled length".to_string(),
            "0".repeat(36),
        ),
        (
            "a named jobId nothing minted".to_string(),
            "fake-job-000000".to_string(),
        ),
        (
            "a jobId minted under a sibling domain".to_string(),
            sibling_job.clone(),
        ),
    ];

    for (case, requested) in &rows {
        let (status, refused) = emulator
            .send("GET", &job_path(&domain_id, requested), None)
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            NOT_FOUND,
            "case {case:?} ({requested}): expected {NOT_FOUND:?}, got {status} with \
             {refused}"
        );
        let stated = detail(case, &refused);
        let expected = format!(
            "Job {} does not exist.",
            state::patient_insights_job_arn(&domain_arn, requested)
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The job that does exist is unaffected by any of those lookups.
    let (status, fetched) = emulator
        .send("GET", &job_path(&domain_id, &job_id), None)
        .await;
    assert_eq!(
        (status, string_at(&fetched, "jobId").as_str()),
        (200, job_id.as_str()),
        "the real job must stay readable, got {status} with {fetched}"
    );
}
