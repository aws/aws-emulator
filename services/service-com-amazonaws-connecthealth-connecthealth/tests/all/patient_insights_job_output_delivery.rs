// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what a patient insights job READS and what it WRITES: the FHIR
//! patient it is started for, and the summary object its completion stores.
//!
//! Both of those live in services this one calls: the FHIR data store the request names
//! and the object store the request configures. So this suite mounts all three on ONE
//! engine, exactly as the assembled router mounts them, and every fact it asserts is
//! answered by the service that OWNS it — the patient is seeded through the data store's
//! own FHIR endpoint, and the summary object is read back through Amazon S3's own
//! `GetObject`.
//!
//! The RULES under test, each over a table rather than one recorded request:
//!
//! 1. A start RESOLVES `patientContext.patientId` against the FHIR server
//!    `inputDataConfig.fhirServer.fhirEndpoint` names: a patient that server holds is
//!    accepted, and a well-formed id it holds no patient under is the modelled 400
//!    `ValidationException` with the fixed sentence
//!    `state::PATIENT_NOT_IN_DATASTORE_MESSAGE`, with no job written — so an identical
//!    second attempt is refused identically. An endpoint that resolves to no data store at
//!    all is the same error with `state::DATASTORE_NOT_FOUND_MESSAGE`, while an endpoint
//!    nothing in this emulator answers for establishes nothing about the patient and the
//!    submission is accepted.
//! 2. A job that reaches the modelled terminal SUCCESS status has STORED its output where
//!    it reports it: the object at `insightsOutput.uri` exists in the configured bucket and
//!    carries the document this emulator can establish — `TotalCount`, the `PatientId` the
//!    job was started for and the `SummaryType` its `insightsType` names — whose sections
//!    each state their own narrative. For a patient on a first visit whose chief complaint
//!    recurs nowhere those narratives are FIXED: they are the same for two jobs started with
//!    different patients, encounters and output paths, because nothing of such a request goes
//!    into them. What the job reports in `statusDetails` is the document-processing report of
//!    its run.
//! 3. Those work products sit at `<s3OutputPath>/<jobId>/<patientId>/summary.json` and
//!    `.../processing-metadata.json`, so Amazon S3's own `GetObject` serves the summary
//!    with the modelled 200 and the object's metadata, the summary states which job ran,
//!    for which patient, when it was generated and that it carries no HCC coding, and
//!    nothing of the job is left at a bare prefix key. That summary carries ONE section per
//!    section of the generator's fixed vocabulary, in its fixed order, each named by its
//!    markdown heading and carrying narrative elements that each hold BOTH `Text` and
//!    `Evidence`; what a section's `Text` states is a statement over this service's own
//!    record of the patient's visits with this provider, and the delivered object carries
//!    the checksum its write declared.
//! 4. A job whose output the object store REFUSES does not report success: it reaches the
//!    modelled `FAILED` status, says so in `statusDetails`, and reports no output location.
//!
//! The rows differ along what each rule generalises over: which kind of identifier resolves
//! the patient (the FHIR resource's own logical id, and the business identifier it carries),
//! the shape of the configured output path, and the way an endpoint can fail to resolve.
//!
//! The harness is this suite's own: it mounts THREE services on one engine, so it drives
//! them by signing name rather than through the single-service `Emulator` the sibling suites
//! share, and its poll loop ([`poll_until_terminal`]) is the shared waiting rule
//! (`patient_insights_job_lifecycle::poll_until`) over that harness — the same condition and
//! the same generous deadline, reached through a different dispatcher.
//!
//! Every constraint written by hand here is the model's own
//! (`smithy_query(query="closure",operation="StartPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connecthealth_connecthealth::state::{
    previous_visit_narrative, recurring_complaint_narrative, SummarySection,
    DATASTORE_NOT_FOUND_MESSAGE, FIRST_VISIT_NARRATIVE, NO_ACTIVE_TRENDS_NARRATIVE,
    OUTPUT_NOT_STORED_DETAIL, OVERVIEW_NARRATIVE, PATIENT_NOT_IN_DATASTORE_MESSAGE,
};

use crate::domain_subscription_lifecycle::{error_code, string_at};

/// The Region every call of this suite is signed for. `us-east-1` is the Region a bucket
/// is created in without a location constraint, so the setup names one Region only.
const REGION: &str = "us-east-1";

/// How long a poll keeps looking before it gives up: many times over the spans this
/// emulator's own lifecycles take, so a loaded host has room while a fast one never waits.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// The modelled `insightsType` this suite starts jobs with, and the `SummaryType` the
/// stored summary document states for it: `InsightsType` declares `PRE_VISIT`
/// (`smithy_query(query="shape",shape="InsightsType")`), which the document states as the
/// kind of summary a reader sees.
const PRE_VISIT: (&str, &str) = ("PRE_VISIT", "Pre-visit");

/// One emulator with this service, the FHIR data store service and the object store all
/// mounted on the SAME engine, exactly as the assembled router mounts them.
struct Insights {
    router: MultiServiceRouter,
}

impl Insights {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_connecthealth_connecthealth::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_healthlake_healthlake::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// A handle to this emulator for driving it through an AWS SDK client — the same
    /// in-process seam a handler reaches another mounted service through.
    fn callback(&self) -> smithy_server_runtime::EmbeddedEmulator {
        smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone())
    }

    /// The object store's own client, built the way this service's own delivery builds it.
    fn s3(&self) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), REGION)
    }

    /// One request against `signing_name`'s endpoint, carrying `headers` and `body`.
    async fn dispatch(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Value) {
        let credential = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let mut builder = ::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", credential);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            ::serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One restJson1 call against this service (`health-agent`).
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body
            .map(|body| ::serde_json::to_vec(body).expect("a serializable body"))
            .unwrap_or_default();
        self.dispatch(
            "health-agent",
            method,
            path,
            &[("content-type", "application/json")],
            payload,
        )
        .await
    }

    /// One awsJson1_0 call against the FHIR data store's control plane.
    async fn datastore_rpc(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.dispatch(
            "healthlake",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.0"),
                ("x-amz-target", &format!("HealthLake.{operation}")),
            ],
            ::serde_json::to_vec(body).expect("a serializable body"),
        )
        .await
    }

    /// One FHIR interaction against a data store's own endpoint.
    async fn fhir(&self, method: &str, uri: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body
            .map(|body| ::serde_json::to_vec(body).expect("a serializable resource"))
            .unwrap_or_default();
        self.dispatch(
            "healthlake",
            method,
            uri,
            &[("content-type", "application/fhir+json")],
            payload,
        )
        .await
    }

    /// A data store, polled through its own reader until it is serving, and the endpoint it
    /// advertises for its FHIR data plane.
    async fn active_datastore(&self, name: &str) -> String {
        let (status, created) = self
            .datastore_rpc(
                "CreateFHIRDatastore",
                &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateFHIRDatastore must answer 200, got {status} with {created}"
        );
        let id = string_at(&created, "DatastoreId");
        // The CONDITION is what is waited for, not an interval: the transition is owed at
        // an instant, so looking again is how a caller observes it.
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let (status, described) = self
                .datastore_rpc("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
                .await;
            assert_eq!(
                status, 200,
                "DescribeFHIRDatastore must answer 200, got {status} with {described}"
            );
            let properties = &described["DatastoreProperties"];
            if properties["DatastoreStatus"].as_str() == Some("ACTIVE") {
                return string_at(properties, "DatastoreEndpoint");
            }
            assert!(
                Instant::now() < deadline,
                "the data store was still not serving after {POLL_DEADLINE:?}, last read \
                 {properties}"
            );
            ::tokio::task::yield_now().await;
        }
    }

    /// Seed one `Patient` into the data store `endpoint` serves, at the logical id
    /// `logical_id` and carrying `identifier` as its business identifier, and confirm the
    /// data store serves it back.
    async fn seed_patient(&self, endpoint: &str, logical_id: &str, resource: &Value) {
        let (status, stored) = self
            .fhir(
                "PUT",
                &format!("{}/Patient/{logical_id}", endpoint.trim_end_matches('/')),
                Some(resource),
            )
            .await;
        assert!(
            status == 200 || status == 201,
            "seeding Patient/{logical_id} must be accepted, got {status} with {stored}"
        );
        let (status, read) = self
            .fhir(
                "GET",
                &format!("{}/Patient/{logical_id}", endpoint.trim_end_matches('/')),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "the seeded Patient/{logical_id} must be readable, got {status} with {read}"
        );
    }

    /// A domain to start jobs in.
    async fn domain(&self, name: &str) -> String {
        let (status, created) = self.call("POST", "/domain", Some(&json!({ "name": name }))).await;
        assert_eq!(
            status, 201,
            "CreateDomain must answer 201, got {status} with {created}"
        );
        string_at(&created, "domainId")
    }
}

/// A `StartPatientInsightsJob` body for `patient_id` against `endpoint`, writing its
/// output under `s3_output_path`.
fn job_body(patient_id: &str, endpoint: &str, s3_output_path: &str) -> Value {
    job_body_for(patient_id, endpoint, s3_output_path, "Annual checkup")
}

/// The same body, for an encounter whose chief complaint is `reason` — the one input the
/// summary document's trends section is a statement over.
fn job_body_for(
    patient_id: &str,
    endpoint: &str,
    s3_output_path: &str,
    reason: &str,
) -> Value {
    json!({
        "patientContext": { "patientId": patient_id },
        "insightsContext": { "insightsType": PRE_VISIT.0 },
        "encounterContext": { "encounterReason": reason },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": {
            "fhirServer": { "fhirEndpoint": endpoint },
            "s3Sources": [ { "uri": "s3://clinic-insights/input.json" } ],
        },
        "outputDataConfig": { "s3OutputPath": s3_output_path },
    })
}

/// A minimal FHIR `Patient` resource carrying `identifier` as its business identifier, and
/// the demographics a row varies — which the job's stored document must NOT carry.
fn patient_resource(identifier: &str, name: Option<Value>, birth_date: Option<&str>) -> Value {
    let mut resource = json!({
        "resourceType": "Patient",
        "identifier": [ { "system": "urn:emulator:mrn", "value": identifier } ],
    });
    if let Some(name) = name {
        resource["name"] = json!([name]);
    }
    if let Some(birth_date) = birth_date {
        resource["birthDate"] = json!(birth_date);
        resource["gender"] = json!("female");
    }
    resource
}

/// Look at a job through its own reader until it reports a terminal `JobStatus`, or fail
/// once [`POLL_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for rather than an interval, so the outcome does not
/// depend on how fast the host runs.
async fn poll_until_terminal(
    emulator: &Insights,
    case: &str,
    domain_id: &str,
    job_id: &str,
) -> Value {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, fetched) = emulator
            .call(
                "GET",
                &format!("/domain/{domain_id}/patient-insights-job/{job_id}"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPatientInsightsJob must answer 200, got {status} with {fetched}"
        );
        match string_at(&fetched, "jobStatus").as_str() {
            "SUCCEEDED" | "FAILED" => return fetched,
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job was still not terminal after {POLL_DEADLINE:?}, last read \
             {fetched}"
        );
        ::tokio::task::yield_now().await;
    }
}

/// The bucket and key an `s3://` URI names, read the way this service's own delivery reads
/// one.
fn object_address(case: &str, uri: &str) -> (String, String) {
    ::service_common::s3_uri::object_address(uri)
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} must name an object"))
}

/// One row of the summary table: how the patient is seeded and where the job's output goes.
struct Row {
    case: &'static str,
    /// The logical id the `Patient` resource is stored at.
    logical_id: &'static str,
    /// The id the job is started with — either the resource's logical id or the business
    /// identifier it carries.
    patient_id: &'static str,
    /// The `Patient.name` entry the record carries, if any.
    name: Option<Value>,
    /// The `Patient.birthDate` the record carries, if any.
    birth_date: Option<&'static str>,
    /// The output path the start configures.
    s3_output_path: &'static str,
}

fn summary_rows() -> Vec<Row> {
    vec![
        Row {
            case: "resolved by the resource's own logical id, into a prefix",
            logical_id: "patient-logical-0001",
            patient_id: "patient-logical-0001",
            name: Some(json!({ "text": "Ada Lovelace" })),
            birth_date: Some("1815-12-10"),
            s3_output_path: "s3://clinic-insights/output/",
        },
        Row {
            case: "resolved by the business identifier the record carries",
            logical_id: "patient-logical-0002",
            patient_id: "mrn-77810",
            name: Some(json!({ "given": ["Grace"], "family": "Hopper" })),
            birth_date: Some("1906-12-09"),
            s3_output_path: "s3://clinic-insights",
        },
        Row {
            case: "a record carrying no demographics at all, into a deep prefix",
            logical_id: "patient-logical-0003",
            patient_id: "patient-logical-0003",
            name: None,
            birth_date: None,
            s3_output_path: "s3://clinic-insights/2026/01/insights/",
        },
    ]
}

#[tokio::test]
async fn a_succeeded_job_has_stored_its_output_where_it_reports_it() {
    let emulator = Insights::new();
    emulator
        .s3()
        .create_bucket()
        .bucket("clinic-insights")
        .send()
        .await
        .expect("the output bucket must be created");
    let endpoint = emulator.active_datastore("clinic-insights-store").await;
    let domain_id = emulator.domain("clinic-insights-delivery").await;

    let mut narrated: Vec<String> = Vec::new();
    for row in summary_rows() {
        let case = row.case;
        emulator
            .seed_patient(
                &endpoint,
                row.logical_id,
                &patient_resource(row.patient_id, row.name.clone(), row.birth_date),
            )
            .await;

        let (status, started) = emulator
            .call(
                "POST",
                &format!("/domain/{domain_id}/patient-insights-job"),
                Some(&job_body(row.patient_id, &endpoint, row.s3_output_path)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a start for a seeded patient must be accepted, got {status} with \
             {started}"
        );
        let job_id = string_at(&started, "jobId");

        let done = poll_until_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&done, "jobStatus"),
            "SUCCEEDED",
            "case {case:?}: a job whose output was stored must report the modelled success \
             status, got {done}"
        );
        let uri = string_at(&done["insightsOutput"], "uri");
        assert_eq!(
            uri,
            format!(
                "{}/{job_id}/{}/summary.json",
                row.s3_output_path.trim_end_matches('/'),
                row.patient_id
            ),
            "case {case:?}: a completed job must report the summary object of its patient \
             under the configured path, got {done}"
        );
        let (bucket, key) = object_address(case, &uri);

        // The object the job REPORTS, read back through the service that owns it.
        let fetched = emulator
            .s3()
            .get_object()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the object at {uri} must exist: {error}")
            });
        let body = fetched
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the object must be readable: {error}"))
            .into_bytes();
        let document: Value = ::serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("case {case:?}: the document must be JSON: {error}"));

        assert_eq!(
            document["TotalCount"].as_u64(),
            Some(1),
            "case {case:?}: the document must count the one patient it is about, got {document}"
        );
        let summaries = document["PatientSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the document must carry PatientSummaries, \
                 got {document}"));
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: the document must hold one patient summary, got {document}"
        );
        let summary = &summaries[0]["PatientSummary"];
        assert_eq!(
            summary["PatientId"].as_str(),
            Some(row.patient_id),
            "case {case:?}: the document must be about the patient the job was started for, \
             got {document}"
        );
        assert_eq!(
            summary["SummaryType"].as_str(),
            Some(PRE_VISIT.1),
            "case {case:?}: the summary type must follow the insights type asked for, \
             got {document}"
        );
        let sections = summary["Sections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the document must carry Sections, got \
                 {document}"));
        assert!(
            !sections.is_empty(),
            "case {case:?}: the document must carry at least one section, got {document}"
        );
        let narratives: Vec<&str> = sections
            .iter()
            .filter_map(|section| section["ClinicalNarrative"].as_array())
            .flatten()
            .filter_map(|narrative| narrative["Text"].as_str())
            .filter(|text| !text.trim().is_empty())
            .collect();
        assert!(
            !narratives.is_empty(),
            "case {case:?}: at least one section must carry a non-empty clinical narrative, \
             got {document}"
        );
        // Each section states its own narrative, and no narrative carries anything of the
        // request or of the patient's record for a patient on a FIRST visit whose chief
        // complaint recurs nowhere — which every row of this table is.
        let joined = narratives.join(" ");
        for leaked in [
            Some("Annual checkup"),
            row.birth_date,
            row.name
                .as_ref()
                .and_then(|name| name["text"].as_str()),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !joined.contains(leaked),
                "case {case:?}: the narrative must carry nothing of the request or the \
                 patient's record ({leaked}), got {joined:?}"
            );
        }
        narrated.push(joined);

        // What the job REPORTS about the run: the document-processing report, whose keys are
        // fixed and whose counts are the ONE patient record this job summarised (this body
        // configures two input SOURCES for reading it — a FHIR server and an S3 source — and
        // the count follows the patient, not the sources), naming the metadata object beside
        // the summary. The stored objects themselves are asserted by the sibling test of
        // this suite.
        let detail: Value = ::serde_json::from_str(&string_at(&done, "statusDetails"))
            .unwrap_or_else(|error| {
                panic!("case {case:?}: statusDetails must be a JSON document: {error}, got {done}")
            });
        assert_eq!(
            detail,
            json!({
                "documentProcessing": { "total": 1, "processed": 1 },
                "processingMetadataUri": format!(
                    "{}/{job_id}/{}/processing-metadata.json",
                    row.s3_output_path.trim_end_matches('/'),
                    row.patient_id
                ),
            }),
            "case {case:?}: a completed job must report the patient record it processed and \
             where its processing metadata went, got {done}"
        );

        // Reading the job again reports the same record: the delivery is made once and a
        // later read neither re-reports nor changes it.
        let again = poll_until_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            again, done,
            "case {case:?}: a terminal job must read the same every time, got {again}"
        );
    }

    // And what each section STATES is the same for every row: each of these patients is on a
    // first visit whose chief complaint recurs at no other visit, so the since-last-visit and
    // trends sections state exactly that, whatever patient, encounter, demographics and
    // output path the row ran with. What they state once the patient's record DOES hold an
    // earlier visit is the sibling test's
    // ([`each_insight_section_states_its_own_narrative_over_the_patients_visit_record`]).
    assert!(
        narrated.windows(2).all(|pair| pair[0] == pair[1]),
        "every job must store the same narrative for a patient with no earlier visit, \
         got {narrated:?}"
    );
    assert_eq!(
        narrated.first().map(String::as_str),
        Some(
            [
                OVERVIEW_NARRATIVE,
                FIRST_VISIT_NARRATIVE,
                NO_ACTIVE_TRENDS_NARRATIVE
            ]
            .join(" ")
            .as_str()
        ),
        "the stored narratives must be the ones this service states per section, \
         got {narrated:?}"
    );
}

#[tokio::test]
async fn a_patient_the_fhir_server_does_not_hold_is_refused() {
    let emulator = Insights::new();
    let endpoint = emulator.active_datastore("clinic-refusal-store").await;
    let domain_id = emulator.domain("clinic-insights-refusal").await;

    // The positive control: a patient the data store does hold is accepted, so every
    // refusal below is answered by a service that can resolve a patient at all.
    emulator
        .seed_patient(
            &endpoint,
            "patient-seeded",
            &patient_resource("patient-seeded", None, None),
        )
        .await;
    let (status, accepted) = emulator
        .call(
            "POST",
            &format!("/domain/{domain_id}/patient-insights-job"),
            Some(&job_body("patient-seeded", &endpoint, "s3://clinic-insights/out/")),
        )
        .await;
    assert_eq!(
        status, 200,
        "the control start must be accepted, got {status} with {accepted}"
    );

    // One row per way a patient can fail to resolve, and per form of a well-formed id the
    // data store holds nothing under.
    let unseeded: Vec<(&str, String)> = vec![
        ("an id nothing was ever written under", "patient-unseeded".to_string()),
        ("a UUID-shaped id", "11111111-2222-3333-4444-555555555555".to_string()),
        ("a one-character id", "x".to_string()),
        (
            "an id carrying characters a URI reserves",
            "mrn/77810?tenant=two".to_string(),
        ),
    ];
    for (case, patient_id) in &unseeded {
        // Twice: the refusal registers nothing, so the second attempt is refused in
        // exactly the same words.
        for attempt in ["first attempt", "second attempt"] {
            let (status, refused) = emulator
                .call(
                    "POST",
                    &format!("/domain/{domain_id}/patient-insights-job"),
                    Some(&job_body(patient_id, &endpoint, "s3://clinic-insights/out/")),
                )
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ValidationException"),
                "case {case:?} ({attempt}): expected the modelled 400 ValidationException, \
                 got {status} with {refused}"
            );
            assert_eq!(
                string_at(&refused, "message"),
                PATIENT_NOT_IN_DATASTORE_MESSAGE,
                "case {case:?} ({attempt}): got {refused}"
            );
        }
    }

    // An endpoint of the same shape naming a data store that does not exist: there is
    // nothing to resolve a patient against, which the service says instead.
    let absent_store = format!("{}/datastore/{}/r4/", endpoint.split("/datastore/").next().unwrap_or_default(), "0".repeat(32));
    let (status, refused) = emulator
        .call(
            "POST",
            &format!("/domain/{domain_id}/patient-insights-job"),
            Some(&job_body(
                "patient-seeded",
                &absent_store,
                "s3://clinic-insights/out/",
            )),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a data store that does not exist\": expected the modelled 400 \
         ValidationException, got {status} with {refused}"
    );
    assert_eq!(
        string_at(&refused, "message"),
        DATASTORE_NOT_FOUND_MESSAGE,
        "case \"a data store that does not exist\": got {refused}"
    );

    // An endpoint nothing in this emulator answers for: nothing is established about the
    // patient, so the submission is accepted rather than refused on a question nobody
    // answered.
    for (case, foreign) in [
        ("a foreign FHIR server", "https://fhir.example.org/r4"),
        ("a path no endpoint of this emulator serves", "https://fhir.example.org/not-fhir"),
    ] {
        let (status, started) = emulator
            .call(
                "POST",
                &format!("/domain/{domain_id}/patient-insights-job"),
                Some(&job_body("patient-unseeded", foreign, "s3://clinic-insights/out/")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a start whose FHIR server nothing answers for must be accepted, \
             got {status} with {started}"
        );
    }
}

#[tokio::test]
async fn a_job_whose_output_the_store_refuses_reports_the_modelled_failure() {
    let emulator = Insights::new();
    let endpoint = emulator.active_datastore("clinic-failure-store").await;
    let domain_id = emulator.domain("clinic-insights-failure").await;
    emulator
        .seed_patient(
            &endpoint,
            "patient-failure",
            &patient_resource("patient-failure", None, None),
        )
        .await;

    // One bucket this emulator holds, and one it does not: the rows differ along whether
    // the configured output location is one the object store will take an object at.
    emulator
        .s3()
        .create_bucket()
        .bucket("clinic-held-output")
        .send()
        .await
        .expect("the held bucket must be created");
    let rows: [(&str, &str, &str); 2] = [
        (
            "a bucket the object store holds",
            "s3://clinic-held-output/out/",
            "SUCCEEDED",
        ),
        (
            "a bucket the object store does not hold",
            "s3://clinic-no-such-output/out/",
            "FAILED",
        ),
    ];

    for (case, s3_output_path, expected) in rows {
        let (status, started) = emulator
            .call(
                "POST",
                &format!("/domain/{domain_id}/patient-insights-job"),
                Some(&job_body("patient-failure", &endpoint, s3_output_path)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the start must be accepted, got {status} with {started}"
        );
        let job_id = string_at(&started, "jobId");
        let done = poll_until_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&done, "jobStatus"),
            expected,
            "case {case:?}: expected the job to report {expected}, got {done}"
        );
        if expected == "FAILED" {
            assert_eq!(
                string_at(&done, "statusDetails"),
                OUTPUT_NOT_STORED_DETAIL,
                "case {case:?}: a job whose output was refused must say so, got {done}"
            );
            assert!(
                done.get("insightsOutput").is_none(),
                "case {case:?}: a job with no stored output must report no location, \
                 got {done}"
            );
        } else {
            let uri = string_at(&done["insightsOutput"], "uri");
            let (bucket, key) = object_address(case, &uri);
            emulator
                .s3()
                .get_object()
                .bucket(&bucket)
                .key(&key)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the object at {uri} must exist: {error}")
                });
        }
    }
}

/// One row of the output-object table: how the patient is seeded, which id the job is
/// started with, and where its output goes.
struct ObjectRow {
    case: &'static str,
    /// The logical id the `Patient` resource is stored at.
    logical_id: &'static str,
    /// The id the job is started with — either the resource's logical id or the business
    /// identifier it carries — which is also the id its output prefix carries.
    patient_id: &'static str,
    /// The output path the start configures.
    s3_output_path: &'static str,
}

/// The rows: every shape of configured output path the modelled pattern allows beside
/// every form of patient id a start may carry, because the rule derives one location from
/// both.
fn object_rows() -> Vec<ObjectRow> {
    vec![
        ObjectRow {
            case: "a prefix with a trailing separator, resolved by the logical id",
            logical_id: "patient-logical-1001",
            patient_id: "patient-logical-1001",
            s3_output_path: "s3://clinic-objects/output/",
        },
        ObjectRow {
            case: "a bucket root with no key, resolved by the business identifier",
            logical_id: "patient-logical-1002",
            patient_id: "mrn-55021",
            s3_output_path: "s3://clinic-objects",
        },
        ObjectRow {
            case: "a deep prefix, with a UUID-shaped patient id",
            logical_id: "3f9d41c6-5b7a-4e21-9d64-1b0c8e77a512",
            patient_id: "3f9d41c6-5b7a-4e21-9d64-1b0c8e77a512",
            s3_output_path: "s3://clinic-objects/2026/02/insights/",
        },
        ObjectRow {
            case: "a one-segment prefix with no trailing separator, with a \
                   one-character patient id",
            logical_id: "x",
            patient_id: "x",
            s3_output_path: "s3://clinic-objects/runs",
        },
    ]
}

/// The bucket every row of the table below writes into.
const OBJECT_BUCKET: &str = "clinic-objects";

/// RULE: a patient insights job that reaches the modelled terminal success status has
/// WRITTEN its work products into the configured output location, at
/// `<s3OutputPath>/<jobId>/<patientId>/summary.json` and `.../processing-metadata.json` —
/// so Amazon S3's own `GetObject` serves the summary the job reports with the modelled 200
/// and the object's metadata, and nothing of the job is left at a bare prefix key.
///
/// The rows differ along what the rule derives a location from: the shape of the
/// configured path (a bucket root, a prefix with and without a trailing separator, a deep
/// prefix) and the form of the patient id (a logical id, a business identifier, a
/// UUID-shaped id, a one-character id).
#[tokio::test]
async fn a_succeeded_job_has_written_both_output_objects_where_a_client_reads_them() {
    let emulator = Insights::new();
    emulator
        .s3()
        .create_bucket()
        .bucket(OBJECT_BUCKET)
        .send()
        .await
        .expect("the output bucket must be created");
    let endpoint = emulator.active_datastore("clinic-objects-store").await;
    let domain_id = emulator.domain("clinic-insights-objects").await;

    for row in object_rows() {
        let case = row.case;
        emulator
            .seed_patient(
                &endpoint,
                row.logical_id,
                &patient_resource(row.patient_id, None, None),
            )
            .await;

        let (status, started) = emulator
            .call(
                "POST",
                &format!("/domain/{domain_id}/patient-insights-job"),
                Some(&job_body(row.patient_id, &endpoint, row.s3_output_path)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the start must be accepted, got {status} with {started}"
        );
        let job_id = string_at(&started, "jobId");

        let done = poll_until_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&done, "jobStatus"),
            "SUCCEEDED",
            "case {case:?}: a job whose output was stored must report the modelled success \
             status, got {done}"
        );

        // Where the rule says the objects are, spelled out here from the row's own values
        // rather than read back off the service.
        let prefix = format!(
            "{}/{job_id}/{}",
            row.s3_output_path.trim_end_matches('/'),
            row.patient_id
        );
        let summary_uri = format!("{prefix}/summary.json");
        let metadata_uri = format!("{prefix}/processing-metadata.json");
        assert_eq!(
            string_at(&done["insightsOutput"], "uri"),
            summary_uri,
            "case {case:?}: the job must report the summary object it wrote, got {done}"
        );

        // The summary object, served by the service that OWNS it.
        let (summary, headers) = get_object(&emulator, case, &summary_uri).await;
        assert_eq!(
            headers.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: the summary object must be served as the JSON document it is"
        );
        assert!(
            headers.etag.as_deref().is_some_and(|etag| !etag.is_empty()),
            "case {case:?}: the served object must carry an entity tag, got {:?}",
            headers.etag
        );
        assert!(
            headers.last_modified,
            "case {case:?}: the served object must carry when it was last modified"
        );

        let generated_at = string_at(&summary, "GeneratedAt");
        assert!(
            is_date_time(&generated_at),
            "case {case:?}: GeneratedAt must be a date-time, got {generated_at:?}"
        );
        assert_eq!(
            summary["TotalCount"].as_u64(),
            Some(1),
            "case {case:?}: the document must count the one patient it is about, got {summary}"
        );
        assert_eq!(
            summary["HccIncluded"].as_bool(),
            Some(false),
            "case {case:?}: the document must state that it carries no HCC coding, \
             got {summary}"
        );
        let patient_summary = &summary["PatientSummaries"][0]["PatientSummary"];
        for (member, expected) in [
            ("JobId", job_id.as_str()),
            ("PatientId", row.patient_id),
            ("SummaryType", PRE_VISIT.1),
            ("GeneratedAt", generated_at.as_str()),
        ] {
            assert_eq!(
                patient_summary[member].as_str(),
                Some(expected),
                "case {case:?}: PatientSummary.{member} must be {expected:?}, got {summary}"
            );
        }
        assert!(
            patient_summary["Sections"]
                .as_array()
                .is_some_and(|sections| !sections.is_empty()),
            "case {case:?}: the document must carry its narrative sections, got {summary}"
        );

        // And the processing-metadata object beside it, which states the RUN.
        let (metadata, headers) = get_object(&emulator, case, &metadata_uri).await;
        assert_eq!(
            headers.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: the processing-metadata object must be served as the JSON \
             document it is"
        );
        assert_eq!(
            string_at(&metadata, "jobId"),
            job_id,
            "case {case:?}: the processing document must name the job that ran, got {metadata}"
        );
        assert_eq!(
            string_at(&metadata, "patientId"),
            row.patient_id,
            "case {case:?}: the processing document must name the patient it ran for, \
             got {metadata}"
        );
        for member in ["total", "processed"] {
            assert_eq!(
                metadata["documentProcessing"][member].as_u64(),
                Some(1),
                "case {case:?}: documentProcessing.{member} must count the one record the job \
                 ran over, got {metadata}"
            );
        }

        // Exactly those two objects, and nothing at a bare prefix key: a client that
        // deletes what the job wrote empties the location it configured.
        let (_, summary_key) = object_address(case, &summary_uri);
        let (_, metadata_key) = object_address(case, &metadata_uri);
        let key_prefix = summary_key.trim_end_matches("summary.json").to_string();
        let listed = emulator
            .s3()
            .list_objects_v2()
            .bucket(OBJECT_BUCKET)
            .prefix(&key_prefix)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the listing must answer: {error}"));
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        let mut expected = vec![metadata_key, summary_key];
        expected.sort();
        assert_eq!(
            keys, expected,
            "case {case:?}: the job must leave exactly its two output objects under \
             {key_prefix:?}, got {keys:?}"
        );
    }
}

/// The object metadata a `GetObject` answer carries, as far as this suite asserts on it.
struct ObjectMetadata {
    content_type: Option<String>,
    etag: Option<String>,
    last_modified: bool,
    /// The CRC32 checksum the store reports for the object, which is the algorithm this
    /// service's own write of it declares.
    checksum_crc32: Option<String>,
    /// The checksum under the algorithm the store applies to a write that declared none, so
    /// a read can tell the two apart.
    checksum_crc64_nvme: Option<String>,
    /// The bytes the read served, so the reported checksum can be recomputed over them.
    bytes: Vec<u8>,
}

/// `GetObject` of the object `uri` names, through the service that owns it, asserting the
/// modelled success and answering the JSON document it served with that object's metadata.
async fn get_object(emulator: &Insights, case: &str, uri: &str) -> (Value, ObjectMetadata) {
    let (bucket, key) = object_address(case, uri);
    let fetched = emulator
        .s3()
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the object at {uri} must exist: {error}"));
    let content_type = fetched.content_type().map(str::to_string);
    let etag = fetched.e_tag().map(str::to_string);
    let last_modified = fetched.last_modified().is_some();
    let checksum_crc32 = fetched.checksum_crc32().map(str::to_string);
    let checksum_crc64_nvme = fetched.checksum_crc64_nvme().map(str::to_string);
    let body = fetched
        .body
        .collect()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the object must be readable: {error}"))
        .into_bytes();
    let metadata = ObjectMetadata {
        content_type,
        etag,
        last_modified,
        checksum_crc32,
        checksum_crc64_nvme,
        bytes: body.to_vec(),
    };
    let document: Value = ::serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("case {case:?}: the document must be JSON: {error}"));
    (document, metadata)
}

/// Whether `text` is an RFC 3339 / ISO 8601 date-time, read by the same parser a generated
/// client reads a date-time member with.
fn is_date_time(text: &str) -> bool {
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .is_ok()
}

/// The bucket and the output path the section table below writes its summaries into.
const SECTIONS_BUCKET: &str = "clinic-section-insights";
/// See [`SECTIONS_BUCKET`].
const SECTIONS_OUTPUT_PATH: &str = "s3://clinic-section-insights/output/";

/// One visit of the section table: whose visit it is, what it was for, and what the patient's
/// record holds by the time the job for it runs.
struct Visit {
    case: &'static str,
    /// The patient the job is started for.
    patient_id: &'static str,
    /// The chief complaint the start carries — the one input the trends section is a
    /// statement over.
    reason: &'static str,
    /// Whether this patient's record already holds an earlier visit with this provider when
    /// this one is started.
    follows_a_previous_visit: bool,
    /// How many of this patient's visits this chief complaint is the reason for, counting
    /// this one.
    complaint_visits: usize,
}

/// The visits the section table runs, in order: the rows differ along what the per-section
/// rule generalises over — which patient the visit belongs to, whether that patient's record
/// already holds an earlier visit with this provider, and whether the chief complaint of this
/// visit recurs across those visits.
fn section_visits() -> Vec<Visit> {
    vec![
        Visit {
            case: "a patient's first visit",
            patient_id: "patient-sections-0001",
            reason: "Annual checkup",
            follows_a_previous_visit: false,
            complaint_visits: 1,
        },
        Visit {
            case: "another patient's first visit, while the first patient already has one",
            patient_id: "patient-sections-0002",
            reason: "Shortness of breath",
            follows_a_previous_visit: false,
            complaint_visits: 1,
        },
        Visit {
            case: "a second visit, for a different chief complaint",
            patient_id: "patient-sections-0001",
            reason: "Rash on forearm",
            follows_a_previous_visit: true,
            complaint_visits: 1,
        },
        Visit {
            case: "a third visit, for the chief complaint of the first one",
            patient_id: "patient-sections-0001",
            reason: "Annual checkup",
            follows_a_previous_visit: true,
            complaint_visits: 2,
        },
    ]
}

/// The instant a start's reported `creationTime` names, whichever form the modelled timestamp
/// travels in: the `date-time` text the member declares, or the epoch-seconds number a JSON
/// protocol gives a timestamp.
fn creation_instant(case: &str, started: &Value) -> SystemTime {
    let reported = &started["creationTime"];
    if let Some(seconds) = reported.as_f64() {
        return SystemTime::UNIX_EPOCH + Duration::from_secs_f64(seconds);
    }
    let text = reported.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: the start must report when the job was created, got {started}")
    });
    let parsed =
        ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: creationTime {text:?} must be a date-time: {error}")
            });
    SystemTime::try_from(parsed).unwrap_or_else(|error| {
        panic!("case {case:?}: creationTime {text:?} must name an instant: {error}")
    })
}

/// The RULE: the summary object a completed job stores carries ONE entry per section of the
/// generator's fixed vocabulary, in its fixed order, each naming its section by the markdown
/// heading `# <SECTION_TOKEN>\n` and carrying a `ClinicalNarrative` list whose every element
/// has BOTH `Text` — the narrative that section states over this service's own record of the
/// patient's visits with this provider — and `Evidence`, present and empty when the narrative
/// cites no document. And the object the write delivered carries the checksum that write
/// declared, which a read with the checksum mode enabled reports back.
///
/// The table runs one job per visit, so each row is asserted against the record the rows
/// before it left: a first visit, another patient's first visit, a second visit for a
/// different chief complaint, and a third whose chief complaint recurs.
#[tokio::test]
async fn each_insight_section_states_its_own_narrative_over_the_patients_visit_record() {
    let emulator = Insights::new();
    emulator
        .s3()
        .create_bucket()
        .bucket(SECTIONS_BUCKET)
        .send()
        .await
        .expect("the output bucket must be created");
    let endpoint = emulator.active_datastore("clinic-sections-store").await;
    let domain_id = emulator.domain("clinic-insights-sections").await;
    for patient in ["patient-sections-0001", "patient-sections-0002"] {
        emulator
            .seed_patient(&endpoint, patient, &patient_resource(patient, None, None))
            .await;
    }

    // When each patient's most recent visit was started, learned from the start's own answer
    // rather than from the clock, so the date a narrative states is the date of the visit the
    // service recorded.
    let mut last_visit: HashMap<&str, SystemTime> = HashMap::new();
    for visit in section_visits() {
        let case = visit.case;
        let (status, started) = emulator
            .call(
                "POST",
                &format!("/domain/{domain_id}/patient-insights-job"),
                Some(&job_body_for(
                    visit.patient_id,
                    &endpoint,
                    SECTIONS_OUTPUT_PATH,
                    visit.reason,
                )),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a start for a seeded patient must be accepted, got {status} with \
             {started}"
        );
        let job_id = string_at(&started, "jobId");
        let previous_visit_at = last_visit.insert(visit.patient_id, creation_instant(case, &started));
        assert_eq!(
            previous_visit_at.is_some(),
            visit.follows_a_previous_visit,
            "case {case:?}: the table must describe the record this visit runs against"
        );

        let done = poll_until_terminal(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&done, "jobStatus"),
            "SUCCEEDED",
            "case {case:?}: a job whose output was stored must report the modelled success \
             status, got {done}"
        );
        let uri = string_at(&done["insightsOutput"], "uri");
        let (document, object) = get_object(&emulator, case, &uri).await;

        // What each section of this document must state, in the document's own order.
        let expected: Vec<String> = vec![
            OVERVIEW_NARRATIVE.to_string(),
            match previous_visit_at {
                Some(previous_visit_at) => previous_visit_narrative(previous_visit_at),
                None => FIRST_VISIT_NARRATIVE.to_string(),
            },
            match visit.complaint_visits > 1 {
                true => recurring_complaint_narrative(visit.reason, visit.complaint_visits),
                false => NO_ACTIVE_TRENDS_NARRATIVE.to_string(),
            },
        ];
        let sections = document["PatientSummaries"][0]["PatientSummary"]["Sections"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the document must carry its Sections, got {document}")
            });
        assert_eq!(
            sections.len(),
            SummarySection::ALL.len(),
            "case {case:?}: the document must carry one entry per insight section, got \
             {document}"
        );
        for ((section, entry), expected_text) in
            SummarySection::ALL.iter().zip(sections).zip(&expected)
        {
            let token = section.token();
            assert_eq!(
                entry["SectionName"].as_str(),
                Some(section.heading().as_str()),
                "case {case:?}: section {token} must be named by its markdown heading, got \
                 {entry}"
            );
            let narratives = entry["ClinicalNarrative"].as_array().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: section {token} must carry a ClinicalNarrative list, got \
                     {entry}"
                )
            });
            assert!(
                !narratives.is_empty(),
                "case {case:?}: section {token} must state at least one narrative, got {entry}"
            );
            for narrative in narratives {
                assert!(
                    narrative["Text"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty()),
                    "case {case:?}: every narrative of section {token} must carry its Text, \
                     got {entry}"
                );
                assert_eq!(
                    narrative["Evidence"].as_array().map(Vec::len),
                    Some(0),
                    "case {case:?}: every narrative of section {token} must carry its \
                     Evidence list, present and empty while it cites no document, got {entry}"
                );
            }
            assert_eq!(
                narratives[0]["Text"].as_str(),
                Some(expected_text.as_str()),
                "case {case:?}: section {token} must state {expected_text:?}, got {entry}"
            );
        }

        // The object the delivery wrote carries the checksum that write DECLARED — the CRC32
        // of the bytes this read served, not the algorithm the store applies to a write that
        // declared none — so a client can verify the document it just downloaded.
        let mut digest = ::crc32fast::Hasher::new();
        digest.update(&object.bytes);
        let expected_checksum = ::aws_smithy_types::base64::encode(digest.finalize().to_be_bytes());
        assert_eq!(
            object.checksum_crc32.as_deref(),
            Some(expected_checksum.as_str()),
            "case {case:?}: the read must report the CRC32 of the {} bytes it served, got {:?}",
            object.bytes.len(),
            object.checksum_crc32
        );
        assert_eq!(
            object.checksum_crc64_nvme, None,
            "case {case:?}: the object must carry the checksum its write declared rather than \
             the store's default, got {:?}",
            object.checksum_crc64_nvme
        );
        // And the store RETAINED it: a `HeadObject` of the same key reports the same value,
        // rather than each response computing one of its own.
        let (bucket, key) = object_address(case, &uri);
        let head = emulator
            .s3()
            .head_object()
            .bucket(&bucket)
            .key(&key)
            .checksum_mode(::aws_sdk_s3::types::ChecksumMode::Enabled)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: a HEAD of the summary object must answer: {error}")
            });
        assert_eq!(
            head.checksum_crc32().map(str::to_string),
            object.checksum_crc32,
            "case {case:?}: a HEAD of the object must report the checksum the read reported"
        );
    }
}
