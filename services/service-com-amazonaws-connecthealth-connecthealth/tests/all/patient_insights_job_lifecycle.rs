// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the patient insights job pair,
//! `StartPatientInsightsJob` -> `GetPatientInsightsJob`.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router's own dispatch — and asserts the RULE the two operations follow
//! rather than one recorded response:
//!
//! 1. A start PERSISTS the job and the reader PROJECTS the stored record: every member
//!    the get answers with is read back out of what the start wrote, for every shape of
//!    request the model allows, so the table's rows differ along what the rule
//!    generalises over (which input source is configured, how many sources, which
//!    optional members are supplied, the size of a bounded string, the enum value
//!    chosen).
//! 2. A job is SCOPED to its domain: only the (`domainId`, `jobId`) pair it was created
//!    under resolves it, and every other pair is the modelled
//!    `ResourceNotFoundException`.
//! 3. The STATUS a get reports is the one the clock has moved the job to — never zero
//!    milliseconds after the start, and terminal once it is terminal; a job that ran to
//!    completion reports the per-patient summary OBJECT it wrote under the path the start
//!    configured, and the document-processing report of that run in `statusDetails`.
//! 4. `clientToken` is an `@idempotencyToken`, which is a rule about how an SDK FILLS
//!    that member and not a promise that the service deduplicates on it: every accepted
//!    start mints its own job, whatever token it carries and however many times that
//!    token has been presented before.
//! 5. Every value of the modelled `JobStatus` enum is projected out of state, including
//!    the ones this emulator's own lifecycle does not reach.
//! 6. The identifiers and instants the pair reports have the FORM the model and the
//!    recorded wire give them: a minted `jobId` is the hyphenated 8-4-4-4-12 UUID form,
//!    and `creationTime` / `updatedTime` are `@timestampFormat("date-time")`, so they
//!    travel as RFC 3339 strings an SDK date-time reader parses.
//!
//! The constraints asserted here are the model's own
//! (`smithy_query(query="closure", operation="GetPatientInsightsJob")`), quoted beside
//! each constant.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::ops::types::{
    InputDataConfig, InsightsContext, InsightsOutput, OutputDataConfig,
    PatientInsightsEncounterContext, PatientInsightsPatientContext, UserContext,
    JOB_STATUS_VALUES, PRONOUNS_VALUES,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, patient_insights_output_uri, JobStatus, PatientInsightsJob, PatientInsightsJobSpec,
    MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN, SUMMARY_OBJECT_NAME,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// `JobId`: `@length 1..36`, with no pattern.
const JOB_ID_LEN: (usize, usize) = (1, 36);
/// The UUID form a minted `jobId` takes — RECORDED behaviour, the same form the sibling
/// identifier of this service (`ScribeSessionId`, whose modelled pattern IS that form)
/// carries: hexadecimal digits in 8-4-4-4-12 groups.
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
/// `PatientInsightsEncounterContext$encounterReason`: `@length 1..256`, pattern
/// `^[a-zA-Z0-9 .,-]+$`.
const ENCOUNTER_REASON_LEN: (usize, usize) = (1, 256);
/// `S3Sources`: `@length max 10`.
const MAX_S3_SOURCES: usize = 10;

/// `POST /domain/{domainId}/patient-insights-job` — `StartPatientInsightsJob`'s route.
///
/// `pub(crate)` so the sibling suites of this test target address this operation the way
/// this one does, rather than each growing its own copy of the route.
pub(crate) fn start_path(domain_id: &str) -> String {
    format!("/domain/{domain_id}/patient-insights-job")
}

/// `GET /domain/{domainId}/patient-insights-job/{jobId}` — `GetPatientInsightsJob`'s
/// route.
pub(crate) fn job_path(domain_id: &str, job_id: &str) -> String {
    format!("/domain/{domain_id}/patient-insights-job/{job_id}")
}

/// One member of a response body, which must be present.
fn member(case: &str, body: &Value, name: &str) -> Value {
    body.get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: member {name} must be present in {body}"))
}

/// Whether `job_id` is the [`UUID_GROUPS`] form — matched field by field, rather than with
/// a regex engine (the sibling suites in this workspace do the same).
fn is_uuid_form(job_id: &str) -> bool {
    let groups: Vec<&str> = job_id.split('-').collect();
    groups.len() == UUID_GROUPS.len()
        && groups.iter().map(|group| group.len()).eq(UUID_GROUPS)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The instant a modelled `@timestampFormat("date-time")` member names, read the way an
/// SDK deserializer generated for one reads it: the member is an RFC 3339 STRING on the
/// wire, never the epoch-seconds number this protocol defaults to for a format-less
/// timestamp.
fn date_time_at(case: &str, body: &Value, name: &str) -> ::aws_smithy_types::DateTime {
    let value = member(case, body, name);
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {name} is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string rather than the protocol's epoch number, got {value}"
        )
    });
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {name} = {text:?} must parse as an RFC 3339 date-time: {error}")
        })
}

/// Whether `arn` is the `JobArn` of `job_id` under `domain_id`, as the modelled pattern
/// declares it:
/// `^arn:aws[-a-z]*:health-agent:[-a-z0-9]+:[0-9]{12}:domain/[-a-zA-Z0-9-]+/patient-insights-job/[-a-zA-Z0-9_/.]+$`
/// — matched by hand, field by field, rather than with a regex engine (the sibling
/// suites in this workspace do the same).
fn is_job_arn(arn: &str, region: &str, domain_id: &str, job_id: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some("aws"),
        Some("health-agent"),
        Some(arn_region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    arn_region == region
        && account_ok
        && resource == format!("domain/{domain_id}/patient-insights-job/{job_id}")
        && (JOB_ID_LEN.0..=JOB_ID_LEN.1).contains(&job_id.len())
}

/// A `StartPatientInsightsJob` body carrying only the required members, with the
/// parameters a row wants varied folded in by the caller.
///
/// `pub(crate)` so the sibling suites of this test target vary ONE member of the same
/// request rather than each growing its own copy of a valid body.
pub(crate) fn minimal_body() -> Value {
    json!({
        "patientContext": { "patientId": MINIMAL_PATIENT_ID },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Annual checkup" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": { "s3Sources": [ { "uri": "s3://clinic-input/patient-0001/" } ] },
        "outputDataConfig": { "s3OutputPath": "s3://clinic-output/insights/" },
    })
}

/// The patient [`minimal_body`] names, which is the id a completed job's output location
/// carries a prefix of.
///
/// `pub(crate)` so the sibling suites that assert that location spell the patient once.
pub(crate) const MINIMAL_PATIENT_ID: &str = "patient-0001";

/// `n` distinct S3 sources, for the row that fills the modelled `S3Sources` list.
fn s3_sources(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "uri": format!("s3://clinic-input/chart-{index}.json") }))
            .collect(),
    )
}

/// `StartPatientInsightsJob`, asserting the modelled success and returning its body.
async fn start_job(emulator: &Emulator, case: &str, domain_id: &str, body: &Value) -> Value {
    let (status, started) = emulator
        .send("POST", &start_path(domain_id), Some(body))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPatientInsightsJob must answer 200, got {status} with {started}"
    );
    started
}

/// `GetPatientInsightsJob`, asserting the modelled success and returning its body.
pub(crate) async fn get_job(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
) -> Value {
    let (status, fetched) = emulator
        .send("GET", &job_path(domain_id, job_id), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPatientInsightsJob must answer 200, got {status} with {fetched}"
    );
    fetched
}

/// A domain to start jobs in.
async fn domain_of(emulator: &Emulator, name: &str) -> String {
    string_at(
        &emulator.create_domain(&json!({ "name": name })).await,
        "domainId",
    )
}

/// How long a poll of a job's status keeps looking: many times over the spans the clock
/// owes a job (`state::PATIENT_INSIGHTS_JOB_STEP_SPAN`), so a loaded host has room while a
/// fast one never waits at all.
pub(crate) const JOB_POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// Look at a job through its own reader until `reached` holds of the `jobStatus` it
/// reports, and answer that reading — or fail once [`JOB_POLL_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for, never an interval: a transition is owed at an
/// instant, so looking again is how a caller observes it, and a fixed wait would make the
/// outcome depend on how fast the host runs.
///
/// `pub(crate)` so every suite of this target waits for a job the one way.
pub(crate) async fn poll_until(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
    reached: fn(&str) -> bool,
) -> Value {
    let deadline = std::time::Instant::now() + JOB_POLL_DEADLINE;
    loop {
        let fetched = get_job(emulator, case, domain_id, job_id).await;
        if reached(&string_at(&fetched, "jobStatus")) {
            return fetched;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the job never reached the status this row waits for within \
             {JOB_POLL_DEADLINE:?}, last read {fetched}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// Whether `status` is one of the modelled TERMINAL job statuses — what a job that has run
/// to its end reports (`JobStatus`: `SUCCEEDED`, `FAILED`).
pub(crate) fn is_terminal(status: &str) -> bool {
    status == "SUCCEEDED" || status == "FAILED"
}

/// Look at a job until it reports a terminal status.
pub(crate) async fn poll_until_terminal(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
) -> Value {
    poll_until(emulator, case, domain_id, job_id, is_terminal).await
}

/// The rows a start is driven with: one per shape of request the model allows, so the
/// round-trip is asserted over the CLASS of job parameters rather than one recorded
/// body. The first row is the minimal required set; the rest vary which input source is
/// configured, how many sources it carries, which optional members are supplied, the
/// size of a bounded string, and which enum value is chosen.
fn parameter_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![("required members only".to_string(), minimal_body())];

    let mut fhir = minimal_body();
    fhir["inputDataConfig"] = json!({
        "fhirServer": { "fhirEndpoint": "https://fhir.example.org/r4" },
    });
    rows.push(("FHIR server as the only input source".to_string(), fhir));

    let mut fhir_token = minimal_body();
    fhir_token["inputDataConfig"] = json!({
        "fhirServer": {
            "fhirEndpoint": "http://fhir.internal.example/r4",
            "oauthToken": "token-value",
        },
    });
    rows.push((
        "FHIR server with an OAuth token".to_string(),
        fhir_token,
    ));

    for count in [1, 2, MAX_S3_SOURCES] {
        let mut body = minimal_body();
        body["inputDataConfig"] = json!({ "s3Sources": s3_sources(count) });
        rows.push((format!("{count} S3 source(s)"), body));
    }

    let mut both = minimal_body();
    both["inputDataConfig"] = json!({
        "fhirServer": { "fhirEndpoint": "https://fhir.example.org/r4" },
        "s3Sources": s3_sources(3),
    });
    rows.push(("both input sources at once".to_string(), both));

    let mut neither = minimal_body();
    neither["inputDataConfig"] = json!({});
    rows.push((
        "an input config with neither source: nothing is invented for it".to_string(),
        neither,
    ));

    // `Pronouns`' whole catalog, from the modelled enum (`PRONOUNS_VALUES`), so no row
    // is keyed to one value — including `THEY_THEM`, which no example in the issue or
    // the model's docs mentions.
    for pronouns in PRONOUNS_VALUES {
        let mut body = minimal_body();
        body["patientContext"] = json!({
            "patientId": "patient-dob",
            "dateOfBirth": "1984-02-29",
            "pronouns": pronouns,
        });
        rows.push((format!("patient context with pronouns {pronouns}"), body));
    }

    let mut specialty = minimal_body();
    specialty["userContext"] = json!({
        "role": "CLINICIAN",
        "userId": "clinician-specialty",
        "specialty": "PRIMARY_CARE",
    });
    rows.push(("user context with a specialty".to_string(), specialty));

    // Both published ends of `encounterReason`'s `@length 1..256`.
    for len in [ENCOUNTER_REASON_LEN.0, ENCOUNTER_REASON_LEN.1] {
        let mut body = minimal_body();
        body["encounterContext"] = json!({ "encounterReason": "a".repeat(len) });
        rows.push((format!("encounterReason of {len} characters"), body));
    }

    let mut nested_output = minimal_body();
    nested_output["outputDataConfig"] =
        json!({ "s3OutputPath": "s3://clinic-output/2026/01/insights/run.json" });
    rows.push((
        "a nested output prefix".to_string(),
        nested_output,
    ));

    rows
}

#[tokio::test]
async fn a_started_job_is_persisted_and_read_back_member_for_member() {
    let region = "us-west-2";
    let emulator = Emulator::new(region);
    let domain_id = domain_of(&emulator, "clinic-insights").await;

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, body) in parameter_rows() {
        let case = case.as_str();
        let started = start_job(&emulator, case, &domain_id, &body).await;

        let job_id = string_at(&started, "jobId");
        assert!(
            (JOB_ID_LEN.0..=JOB_ID_LEN.1).contains(&job_id.len()),
            "case {case:?}: jobId must be {}..{} characters, got {job_id:?} ({} characters)",
            JOB_ID_LEN.0,
            JOB_ID_LEN.1,
            job_id.len()
        );
        assert!(
            is_uuid_form(&job_id),
            "case {case:?}: a minted jobId must be the {}-character 8-4-4-4-12 UUID form, \
             got {job_id:?} ({} characters)",
            JOB_ID_LEN.1,
            job_id.chars().count()
        );
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: every start must mint a fresh jobId, got the reused {job_id:?}"
        );
        let job_arn = string_at(&started, "jobArn");
        assert!(
            is_job_arn(&job_arn, region, &domain_id, &job_id),
            "case {case:?}: expected the JobArn of {job_id} under {domain_id}, got {job_arn:?}"
        );
        let creation_time = member(case, &started, "creationTime");
        // The start's own `creationTime` is modelled `@timestampFormat("date-time")`, so it
        // is an RFC 3339 string an SDK date-time reader parses.
        let created_at = date_time_at(case, &started, "creationTime");

        let fetched = get_job(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&fetched, "jobId"),
            job_id,
            "case {case:?}: the reader must report the stored jobId, got {fetched}"
        );
        assert_eq!(
            string_at(&fetched, "jobArn"),
            job_arn,
            "case {case:?}: the reader must report the stored jobArn, got {fetched}"
        );
        assert_eq!(
            member(case, &fetched, "creationTime"),
            creation_time,
            "case {case:?}: the reader must report the instant the start recorded, got {fetched}"
        );
        // And the reader's two timestamp members are date-time strings too, naming the same
        // instant the start reported.
        assert_eq!(
            date_time_at(case, &fetched, "creationTime"),
            created_at,
            "case {case:?}: the reader must report the instant the start recorded, got {fetched}"
        );
        // Nothing has transitioned yet: the span the clock owes is never zero, so a
        // caller reading straight after the start still sees the submitted job, whose
        // last change IS its creation.
        assert_eq!(
            string_at(&fetched, "jobStatus"),
            "SUBMITTED",
            "case {case:?}: a job read straight after the start must still be SUBMITTED, \
             got {fetched}"
        );
        assert_eq!(
            member(case, &fetched, "updatedTime"),
            creation_time,
            "case {case:?}: a job that has not transitioned was last updated when it was \
             created, got {fetched}"
        );
        assert_eq!(
            date_time_at(case, &fetched, "updatedTime"),
            created_at,
            "case {case:?}: a job that has not transitioned was last updated when it was \
             created, got {fetched}"
        );
        // No run produced insights, so there is no output to point at and nothing to
        // say about the status yet.
        assert!(
            fetched.get("insightsOutput").is_none(),
            "case {case:?}: a job that generated no insights must carry no insightsOutput, \
             got {fetched}"
        );
        assert!(
            fetched.get("statusDetails").is_none(),
            "case {case:?}: a freshly submitted job has no status detail, got {fetched}"
        );

        // The round trip: each parameter member the reader reports is byte-for-byte the
        // one the start was given — an absent optional member stays absent.
        for name in [
            "patientContext",
            "insightsContext",
            "encounterContext",
            "userContext",
            "inputDataConfig",
            "outputDataConfig",
        ] {
            assert_eq!(
                member(case, &fetched, name),
                body[name],
                "case {case:?}: {name} must read back as it was persisted, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn the_clock_moves_a_submitted_job_to_the_modelled_success_status() {
    let emulator = Emulator::new("eu-west-1");
    let domain_id = domain_of(&emulator, "clinic-lifecycle").await;
    let body = minimal_body();
    let started = start_job(&emulator, "lifecycle", &domain_id, &body).await;
    let job_id = string_at(&started, "jobId");
    let patient_id = string_at(&body["patientContext"], "patientId");
    let creation_time = member("lifecycle", &started, "creationTime");

    // Zero milliseconds after the start: still SUBMITTED.
    let fetched = get_job(&emulator, "straight after the start", &domain_id, &job_id).await;
    assert_eq!(
        string_at(&fetched, "jobStatus"),
        "SUBMITTED",
        "a job read straight after the start must be SUBMITTED, got {fetched}"
    );

    // Real elapsed time is required (the handlers read `SystemTime::now()`), so each
    // transition is waited for by LOOKING at the job until it has fallen due, never by
    // sleeping for a span: the CONDITION is what a caller observes, and a fixed wait would
    // make the outcome depend on how fast the host runs.
    let fetched = poll_until(
        &emulator,
        "one span in",
        &domain_id,
        &job_id,
        |status| status != "SUBMITTED",
    )
    .await;
    assert_eq!(
        string_at(&fetched, "jobStatus"),
        "IN_PROGRESS",
        "the first status after SUBMITTED must be the one the clock owes next, got {fetched}"
    );
    assert!(
        fetched.get("statusDetails").is_none(),
        "a job still running has nothing to report about its status, got {fetched}"
    );

    let done = poll_until_terminal(&emulator, "two spans in", &domain_id, &job_id).await;
    assert_eq!(
        string_at(&done, "jobStatus"),
        "SUCCEEDED",
        "a job that ran to completion must end in the modelled success status, got {done}"
    );
    // Where the output of THIS job's patient went, spelled out here rather than read back
    // off the service: the configured path, the job this service minted, the patient it ran
    // for, and the summary document's own name.
    let configured = string_at(&body["outputDataConfig"], "s3OutputPath");
    let prefix = format!("{}/{job_id}/{}", configured.trim_end_matches('/'), patient_id);
    assert_eq!(
        member("two spans in", &done, "insightsOutput"),
        json!({
            "uri": patient_insights_output_uri(
                &configured,
                &job_id,
                MINIMAL_PATIENT_ID,
                SUMMARY_OBJECT_NAME,
            )
            .expect("the configured path leaves room for the object location"),
        }),
        "a completed job must report its summary object under the configured path, got {done}"
    );
    // And what it reports about the run: the document-processing report, whose keys are
    // fixed and whose values are this job's own — the one patient record this job
    // summarised, and the metadata object sitting beside the summary. Spelled out here
    // rather than read back off the service.
    let detail: Value = ::serde_json::from_str(&string_at(&done, "statusDetails"))
        .unwrap_or_else(|error| panic!("statusDetails must be a JSON document: {error}, {done}"));
    assert_eq!(
        detail,
        json!({
            "documentProcessing": { "total": 1, "processed": 1 },
            "processingMetadataUri": format!("{prefix}/processing-metadata.json"),
        }),
        "a completed job must report the patient record it processed and where its \
         processing metadata went, got {done}"
    );
    assert_ne!(
        member("two spans in", &done, "updatedTime"),
        creation_time,
        "a transitioned job must report when it last changed, got {done}"
    );
    assert_eq!(
        member("two spans in", &done, "creationTime"),
        creation_time,
        "the creation instant must not move, got {done}"
    );

    // Terminal means terminal: two further reads report exactly the same record, so a
    // read never advances what a later caller sees.
    let again = get_job(&emulator, "after the run", &domain_id, &job_id).await;
    let and_again = get_job(&emulator, "after the run, twice", &domain_id, &job_id).await;
    assert_eq!(
        again, done,
        "a terminal job must read the same every time, got {again}"
    );
    assert_eq!(
        and_again, done,
        "a terminal job must read the same every time, got {and_again}"
    );
}

#[tokio::test]
async fn a_job_is_readable_only_under_its_own_domain() {
    let emulator = Emulator::new("us-east-1");
    let home = domain_of(&emulator, "clinic-home").await;
    let other = domain_of(&emulator, "clinic-other").await;
    let started = start_job(&emulator, "scoping", &home, &minimal_body()).await;
    let job_id = string_at(&started, "jobId");

    // The job is readable under the domain it was started in.
    let fetched = get_job(&emulator, "its own domain", &home, &job_id).await;
    assert_eq!(
        string_at(&fetched, "jobId"),
        job_id,
        "the job must be readable under its own domain, got {fetched}"
    );

    // Every other (domainId, jobId) pair is the modelled 404: a sibling domain, a
    // jobId nothing minted, and a domainId no domain holds.
    let unknown_job = "0".repeat(JOB_ID_LEN.1);
    let unknown_domain = "dom-0123456789abcdef0";
    for (case, domain, job, expected_in_message) in [
        ("a sibling domain", other.as_str(), job_id.as_str(), job_id.as_str()),
        ("a jobId nothing minted", home.as_str(), unknown_job.as_str(), unknown_job.as_str()),
        ("a jobId one character long", home.as_str(), "x", "x"),
        (
            "a domainId no domain holds",
            unknown_domain,
            job_id.as_str(),
            unknown_domain,
        ),
    ] {
        let (status, body) = emulator.send("GET", &job_path(domain, job), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
        let detail = string_at(&body, "message");
        assert!(
            detail.contains(expected_in_message),
            "case {case:?}: the detail must name the identifier that did not resolve \
             ({expected_in_message}), got {detail:?}"
        );
    }

    // And a domain on its way out holds nothing readable either.
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{home}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} with {deleted}");
    let (status, body) = emulator.send("GET", &job_path(&home, &job_id), None).await;
    assert_eq!(
        status, 404,
        "a job under a deleted domain must answer the modelled 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a job under a deleted domain got {body}"
    );
}

#[tokio::test]
async fn every_accepted_start_mints_its_own_job_whatever_token_it_carries() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-tokens").await;
    let elsewhere = domain_of(&emulator, "clinic-elsewhere").await;

    // One token, presented repeatedly with a byte-identical body, then with changed
    // parameters, beside a body carrying no token at all: the rows differ along whether a
    // token is carried, whether it has been presented before, whether the parameters
    // beside it changed, and which domain the start is addressed to.
    let token = "token-repeated";
    let mut repeated = minimal_body();
    repeated["clientToken"] = json!(token);
    let mut changed = repeated.clone();
    changed["patientContext"] = json!({ "patientId": "patient-9999" });
    let mut other_token = minimal_body();
    other_token["clientToken"] = json!("token-another");

    let rows: Vec<(&str, &str, Value)> = vec![
        ("a fresh token", domain_id.as_str(), repeated.clone()),
        ("the same token again", domain_id.as_str(), repeated.clone()),
        (
            "the same token a third time",
            domain_id.as_str(),
            repeated.clone(),
        ),
        (
            "the same token with changed parameters",
            domain_id.as_str(),
            changed,
        ),
        ("another token", domain_id.as_str(), other_token),
        ("no token at all", domain_id.as_str(), minimal_body()),
        (
            "no token at all, again",
            domain_id.as_str(),
            minimal_body(),
        ),
        (
            "the same token in another domain",
            elsewhere.as_str(),
            repeated.clone(),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, domain, body) in &rows {
        let started = start_job(&emulator, case, domain, body).await;
        let job_id = string_at(&started, "jobId");
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: every accepted start must mint its own job, got the reused \
             {job_id:?}"
        );
        // And the job it minted is the one it reports: the record reads back with the
        // parameters THIS call supplied, not an earlier call's.
        let fetched = get_job(&emulator, case, domain, &job_id).await;
        assert_eq!(
            member(case, &fetched, "patientContext"),
            body["patientContext"],
            "case {case:?}: the job must carry the parameters this start supplied, \
             got {fetched}"
        );
        assert_eq!(
            string_at(&fetched, "jobArn"),
            string_at(&started, "jobArn"),
            "case {case:?}: the stored job must report the ARN the start answered, \
             got {fetched}"
        );
    }
    assert_eq!(
        minted.len(),
        rows.len(),
        "every row must have started a job of its own, got {minted:?}"
    );
}

/// Seed one job directly into this service's store, in a status the emulator's own
/// lifecycle may not reach.
///
/// The way to pin a reader against a record no operation writes: the job is written
/// through the SHARED engine the service was mounted on, exactly as a handler would.
fn seed_job(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
    status: JobStatus,
    status_details: Option<&str>,
    insights_output: Option<&str>,
) {
    let now = SystemTime::now();
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let domain = store
        .live_domain_mut(emulator.region(), domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: a live domain to seed"));
    let job = PatientInsightsJob {
        job_id: job_id.to_string(),
        domain_id: domain_id.to_string(),
        arn: state::patient_insights_job_arn(&domain.arn, job_id),
        status,
        status_details: status_details.map(str::to_string),
        insights_output: insights_output.map(|uri| InsightsOutput {
            uri: uri.to_string(),
        }),
        created_at: now,
        updated_at: now,
        // Seeded in the status under test, so no transition is owed.
        next_transition_at: None,
        // Nothing is owed outside this service for a record seeded in its final status.
        output_delivery: None,
        spec: PatientInsightsJobSpec {
            patient_context: PatientInsightsPatientContext {
                patient_id: "patient-seeded".to_string(),
                date_of_birth: None,
                pronouns: None,
            },
            insights_context: InsightsContext {
                insights_type: "PRE_VISIT".to_string(),
            },
            encounter_context: PatientInsightsEncounterContext {
                encounter_reason: "Seeded encounter".to_string(),
            },
            user_context: UserContext {
                role: "CLINICIAN".to_string(),
                user_id: "clinician-seeded".to_string(),
                specialty: None,
            },
            input_data_config: InputDataConfig {
                fhir_server: None,
                s3_sources: None,
            },
            output_data_config: OutputDataConfig {
                s3_output_path: "s3://clinic-output/seeded/".to_string(),
            },
        },
    };
    domain.patient_insights_jobs.insert(job_id.to_string(), job);
}

#[tokio::test]
async fn the_reader_projects_every_modelled_job_status_out_of_state() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-statuses").await;

    // The whole `JobStatus` catalog, from the modelled enum (`JOB_STATUS_VALUES`), so
    // no status is special-cased — including `FAILED`, which this emulator's own
    // lifecycle does not reach, and `SUCCEEDED`, which carries the output of a run.
    for (index, wire_status) in JOB_STATUS_VALUES.iter().enumerate() {
        let case = format!("jobStatus {wire_status}");
        let status = JobStatus::from_wire(wire_status)
            .unwrap_or_else(|| panic!("case {case:?}: the state enum must know this wire value"));
        let job_id = format!("seeded-job-{index}");
        let (details, output) = match status {
            JobStatus::Succeeded => (None, Some("s3://clinic-output/seeded/insights.json")),
            JobStatus::Failed => (Some("The job could not be completed."), None),
            _ => (None, None),
        };
        seed_job(
            &emulator,
            &case,
            &domain_id,
            &job_id,
            status,
            details,
            output,
        );

        let fetched = get_job(&emulator, &case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&fetched, "jobStatus"),
            *wire_status,
            "case {case:?}: the reader must report the stored status, got {fetched}"
        );
        match details {
            Some(text) => assert_eq!(
                string_at(&fetched, "statusDetails"),
                text,
                "case {case:?}: the reader must report the stored status detail, got {fetched}"
            ),
            None => assert!(
                fetched.get("statusDetails").is_none(),
                "case {case:?}: a job with no status detail must carry none, got {fetched}"
            ),
        }
        match output {
            Some(uri) => assert_eq!(
                member(&case, &fetched, "insightsOutput"),
                json!({ "uri": uri }),
                "case {case:?}: the reader must report the stored output, got {fetched}"
            ),
            None => assert!(
                fetched.get("insightsOutput").is_none(),
                "case {case:?}: a job with no output must carry none, got {fetched}"
            ),
        }
    }
}

#[tokio::test]
async fn a_domain_at_the_job_ceiling_refuses_a_further_start() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-ceiling").await;

    // One short of the ceiling: the next start is still accepted.
    for index in 0..(MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN - 1) {
        seed_job(
            &emulator,
            "below the ceiling",
            &domain_id,
            &format!("seeded-job-{index}"),
            JobStatus::Succeeded,
            None,
            None,
        );
    }
    let started = start_job(&emulator, "the last free slot", &domain_id, &minimal_body()).await;
    assert!(
        !string_at(&started, "jobId").is_empty(),
        "the start one short of the ceiling must be accepted, got {started}"
    );

    // At the ceiling the next start is refused, and the refusal names the count.
    let (status, refused) = emulator
        .send("POST", &start_path(&domain_id), Some(&minimal_body()))
        .await;
    assert_eq!(
        status, 400,
        "a start past the ceiling must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a start past the ceiling got {refused}"
    );
    let detail = string_at(&refused, "message");
    assert!(
        detail.contains(&MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN.to_string()),
        "the refusal must name the ceiling it enforced, got {detail:?}"
    );
}

/// RULE on the SCHEMA the two responses are serialized against: a member the model marks
/// `@timestampFormat("date-time")` reports that format and a member the model marks with
/// none reports none — which is what makes the codec write a string for the first kind and
/// the protocol's own epoch number for the rest.
///
/// This also pins the two hand-written output schemas — `GetPatientInsightsJobOutput` in
/// `src/ops/types/get_patient_insights_job_types.rs` and `StartPatientInsightsJobOutput` in
/// `src/state/timestamp_wire.rs`, each written out because the shape derive drops
/// `@timestampFormat` — against the model: every member's
/// wire name and index, in the order
/// `smithy_query(query="operation",operation="StartPatientInsightsJob,GetPatientInsightsJob")`
/// lists them. The members that are not timestamps are the control rows: they must report
/// NO format, so the assertion is about the trait rather than about every member schema.
#[test]
fn the_job_pairs_output_schemas_match_the_model_member_for_member() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_connecthealth_connecthealth::ops::types::GET_PATIENT_INSIGHTS_JOB_OUTPUT_SCHEMA;
    use service_com_amazonaws_connecthealth_connecthealth::state::START_PATIENT_INSIGHTS_JOB_OUTPUT_SCHEMA;

    // (wire name, whether the model marks the member @timestampFormat("date-time")).
    let start: &[(&str, bool)] = &[("jobArn", false), ("jobId", false), ("creationTime", true)];
    let get: &[(&str, bool)] = &[
        ("jobId", false),
        ("jobArn", false),
        ("jobStatus", false),
        ("creationTime", true),
        ("updatedTime", true),
        ("insightsOutput", false),
        ("statusDetails", false),
        ("patientContext", false),
        ("insightsContext", false),
        ("encounterContext", false),
        ("userContext", false),
        ("inputDataConfig", false),
        ("outputDataConfig", false),
    ];

    for (shape, schema, expected) in [
        (
            "StartPatientInsightsJobResponse",
            START_PATIENT_INSIGHTS_JOB_OUTPUT_SCHEMA,
            start,
        ),
        (
            "GetPatientInsightsJobResponse",
            GET_PATIENT_INSIGHTS_JOB_OUTPUT_SCHEMA,
            get,
        ),
    ] {
        // Read off the structure's own schema rather than off the member constants, so a
        // member left out of it is caught too.
        let named: Vec<Option<&str>> = schema
            .members()
            .iter()
            .map(|member| member.member_name())
            .collect();
        let wanted: Vec<Option<&str>> = expected.iter().map(|(name, _)| Some(*name)).collect();
        assert_eq!(
            named, wanted,
            "case {shape:?}: the schema must name every modelled member in the model's \
             order, got {named:?}"
        );

        for (index, (name, is_date_time)) in expected.iter().enumerate() {
            let member = schema.members()[index];
            assert_eq!(
                member.member_index(),
                Some(index),
                "case {shape:?}/{name:?}: the member must sit at the model's index {index}, \
                 got {:?}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|trait_| trait_.format());
            match is_date_time {
                true => assert!(
                    matches!(format, Some(TimestampFormat::DateTime)),
                    "case {shape:?}/{name:?}: the model marks this member \
                     @timestampFormat(\"date-time\"), so its schema must carry that format, \
                     got {format:?}"
                ),
                false => assert!(
                    format.is_none(),
                    "case {shape:?}/{name:?}: the model gives this member no \
                     @timestampFormat, got {format:?}"
                ),
            }
        }
    }
}
