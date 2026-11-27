// SPDX-License-Identifier: CC0-1.0

//! The response document of the connecthealth patient-insights job pair, asserted on the
//! ASSEMBLED router — the arrangement a client (and the behavioural corpus) actually talks
//! to, where connecthealth and the object store it writes through are mounted on one
//! engine.
//!
//! Rule 1: the WIRE FORM of a timestamp is the one its own member declares. A member the
//! model marks `@timestampFormat("date-time")` — `creationTime` on
//! `StartPatientInsightsJob`, `creationTime` and `updatedTime` on `GetPatientInsightsJob`
//! — travels on this restJson1 service as an RFC 3339 / ISO-8601 date-time STRING, which
//! the SDK date-time reader parses; a member the same model leaves format-less — a
//! `Domain`'s `createdAt`, a `Subscription`'s `createdAt` and `lastUpdatedAt` — keeps the
//! protocol default of an epoch-seconds NUMBER
//! (`smithy_query(query="operation",operation="StartPatientInsightsJob,GetPatientInsightsJob,CreateDomain,CreateSubscription",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports `"timestampFormat": "date-time"` on the first three and nothing on the rest).
//!
//! Rule 2: a minted `jobId` is the UUID form — 36 characters in hyphenated 8-4-4-4-12
//! groups of hexadecimal digits, inside the modelled `JobId` bound of 1..36 — fresh for
//! every accepted start, and the `jobArn` the same response reports carries it.
//!
//! Rule 3: a job that ran to completion reports the per-patient summary OBJECT it wrote as
//! its `insightsOutput.uri`, and the document-processing report of that run in
//! `statusDetails` — the one patient record it summarised, counted whatever number of input
//! SOURCES the start configured for reading it — naming the processing-metadata object
//! beside the summary. BOTH objects are in the configured bucket, read back through Amazon
//! S3's own `GetObject` — the service that OWNS them — which is why this suite lives here
//! rather than in either service's crate.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over: the
//! shape of the configured output path, the identifier the patient is named by, how many
//! input sources the start configures, and where in its lifecycle the job is read.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so each test builds it —
//! and drives its requests — on a sized thread.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: assembling the whole router is what
/// every test below does first.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every request below is signed for. `us-east-1` is the Region a bucket is
/// created in without a location constraint, so the setup names one Region only.
const REGION: &str = "us-east-1";

/// The SigV4 signing name connecthealth is routed on — its `endpointPrefix` /
/// `arnNamespace`.
const HEALTH_AGENT: &str = "health-agent";

/// The SigV4 signing name Amazon S3 is routed on.
pub(crate) const S3: &str = "s3";

/// The bucket every job of this suite is configured to write into.
const BUCKET: &str = "clinic-insights-wire";

/// The file names of the two objects a completed job writes under its own per-patient
/// prefix: the summary its `insightsOutput.uri` addresses and the processing metadata its
/// `statusDetails` reports. RECORDED behaviour, stated here rather than read off the
/// service.
pub(crate) const SUMMARY_OBJECT: &str = "summary.json";
pub(crate) const PROCESSING_METADATA_OBJECT: &str = "processing-metadata.json";

/// How long a poll keeps LOOKING for a condition before it gives up: many times over the
/// spans this emulator's own lifecycles take, so a loaded host has room while a fast one
/// never waits at all.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll pauses between two reads: short enough that the condition is observed
/// as soon as it holds, long enough that the loop does not spin a core flat while it waits.
/// It is not a bound on anything — [`POLL_DEADLINE`] is.
const BETWEEN_READS: Duration = Duration::from_millis(20);

/// What one call answered: the HTTP status, the body's bytes and the body parsed as JSON
/// (`Value::Null` for a body that is empty or not JSON).
///
/// `pub(crate)` with the harness around it (`send`, `connecthealth`, `assembled`, `domain`,
/// `job_body`, `poll_until_terminal`), so the sibling suite that tears a configured output
/// bucket down (`s3_bucket_teardown_after_delivered_job_output`) drives this pair of
/// services exactly as this one does rather than keeping another copy of them.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) bytes: Vec<u8>,
    pub(crate) body: Value,
}

impl Answer {
    /// A string member the answer must carry.
    pub(crate) fn string_at(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("member {member} must be a string in {:?}", self.body))
            .to_string()
    }
}

/// Dispatch one request addressed to `signing_name` in [`REGION`], the way a signed SDK
/// call is: the credential scope is what names the service.
pub(crate) fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = match bytes.is_empty() {
        true => Value::Null,
        false => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    };
    Answer {
        status,
        bytes,
        body,
    }
}

/// One connecthealth operation, over the restJson1 routes that model publishes.
pub(crate) fn connecthealth(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body
        .map(|body| serde_json::to_vec(&body).expect("a serializable body"))
        .unwrap_or_default();
    send(
        router,
        runtime,
        HEALTH_AGENT,
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    )
}

/// The router every test of this suite drives, with request authentication off (the
/// credential scope still names the service).
pub(crate) fn assembled() -> (MultiServiceRouter, tokio::runtime::Runtime) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = build_router(engine);
    router.set_request_authentication(false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    (router, runtime)
}

/// A `Domain` created through connecthealth's own `CreateDomain`, and the body that create
/// answered.
pub(crate) fn domain(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> (String, Value) {
    let created = connecthealth(router, runtime, "POST", "/domain", Some(json!({ "name": name })));
    assert_eq!(
        created.status, 201,
        "CreateDomain must answer 201, got {} with {:?}",
        created.status, created.body
    );
    (created.string_at("domainId"), created.body.clone())
}

/// A `StartPatientInsightsJob` body: the patient the job runs for, `sources` S3 input
/// sources and the output path it is configured with.
pub(crate) fn job_body(patient_id: &str, sources: usize, s3_output_path: &str) -> Value {
    let s3_sources: Vec<Value> = (0..sources)
        .map(|index| json!({ "uri": format!("s3://clinic-input/chart-{index}.json") }))
        .collect();
    json!({
        "patientContext": { "patientId": patient_id },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Annual checkup" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": { "s3Sources": s3_sources },
        "outputDataConfig": { "s3OutputPath": s3_output_path },
    })
}

/// The instant an RFC 3339 date-time string names, read the way an SDK deserializer
/// generated for a `@timestampFormat("date-time")` member reads it.
///
/// A member that went out in the protocol's own epoch-seconds form is a JSON number, which
/// is not a string at all, so this is also what tells the two forms apart.
fn date_time_at(case: &str, body: &Value, member: &str) -> DateTime {
    let value = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: member {member} must be present in {body}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 STRING rather than the protocol's epoch number, got {value}"
        )
    });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} = {text:?} must parse as an RFC 3339 date-time: {error}")
    })
}

/// Whether `job_id` is the UUID form: 36 characters, hexadecimal digits in 8-4-4-4-12
/// groups separated by `-`. Matched field by field rather than with a regex engine, as the
/// sibling suites of this workspace match a modelled pattern.
fn is_uuid_form(job_id: &str) -> bool {
    let groups: Vec<&str> = job_id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Look at a job through its own reader until it reports a terminal `JobStatus`, and answer
/// that reading — or fail once [`POLL_DEADLINE`] has passed.
///
/// The CONDITION is what is waited for, never an interval: a transition is owed at an
/// instant, so looking again is how a caller observes it. The pause between reads is only
/// so the loop does not saturate a core while it waits; it bounds nothing.
pub(crate) fn poll_until_terminal(
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
            &format!("/domain/{domain_id}/patient-insights-job/{job_id}"),
            None,
        );
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetPatientInsightsJob must answer 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        if matches!(
            fetched.string_at("jobStatus").as_str(),
            "SUCCEEDED" | "FAILED"
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
fn the_job_pairs_date_time_members_travel_as_strings_and_its_ids_are_uuids() {
    std::thread::Builder::new()
        .name("connecthealth-insights-wire-form".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let (router, runtime) = assembled();
            let (domain_id, created) = domain(&router, &runtime, "clinic-insights-wire");

            // The same model's format-less timestamps, for contrast: a `Domain`'s
            // `createdAt` and a `Subscription`'s instants declare no `@timestampFormat`, so
            // they keep the protocol's epoch-seconds NUMBER. The rule is per MEMBER, not
            // per service.
            assert!(
                created["createdAt"].is_number(),
                "a Domain's createdAt declares no @timestampFormat, so it stays the \
                 protocol's epoch number, got {created}"
            );
            let subscription = connecthealth(
                &router,
                &runtime,
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(json!({})),
            );
            assert_eq!(
                subscription.status, 201,
                "CreateSubscription must answer 201, got {} with {:?}",
                subscription.status, subscription.body
            );
            // A freshly created subscription reports no `lastUpdatedAt` at all (no
            // transition has stamped one), so the member under test here is the one it does
            // carry.
            assert!(
                subscription.body["createdAt"].is_number(),
                "a Subscription's createdAt declares no @timestampFormat, so it stays the \
                 protocol's epoch number, got {:?}",
                subscription.body
            );

            // The rows differ along what the rule generalises over: the patient the job
            // names, how many input sources it is configured with, and the shape of the
            // configured output path.
            let rows: Vec<(&str, &str, usize, &str)> = vec![
                (
                    "one source, into a bucket root",
                    "patient-0001",
                    1,
                    "s3://clinic-insights-wire",
                ),
                (
                    "two sources, into a prefix",
                    "mrn-77810",
                    2,
                    "s3://clinic-insights-wire/output/",
                ),
                (
                    "three sources, into a deep prefix, patient named with the characters an \
                     identifier may carry",
                    "patient.beta_2",
                    3,
                    "s3://clinic-insights-wire/2026/01/insights",
                ),
            ];

            let mut minted: BTreeSet<String> = BTreeSet::new();
            for (case, patient_id, sources, path) in &rows {
                let started = connecthealth(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/domain/{domain_id}/patient-insights-job"),
                    Some(job_body(patient_id, *sources, path)),
                );
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: StartPatientInsightsJob must answer 200, got {} with {:?}",
                    started.status, started.body
                );

                // Rule 2: the id this service minted is the UUID form, it is fresh, and the
                // ARN the same response reports carries it.
                let job_id = started.string_at("jobId");
                assert!(
                    is_uuid_form(&job_id),
                    "case {case:?}: a minted jobId must be the 36-character 8-4-4-4-12 UUID \
                     form, got {job_id:?} ({} characters)",
                    job_id.chars().count()
                );
                assert!(
                    minted.insert(job_id.clone()),
                    "case {case:?}: every accepted start must mint a fresh jobId, got the \
                     reused {job_id:?}"
                );
                let job_arn = started.string_at("jobArn");
                assert!(
                    job_arn.ends_with(&format!(
                        "domain/{domain_id}/patient-insights-job/{job_id}"
                    )),
                    "case {case:?}: the jobArn must carry the id this start minted, got \
                     {job_arn:?}"
                );

                // Rule 1 on the start's own response, and then on the reader's: the
                // creation instant is an RFC 3339 string in both, and it names the same
                // instant.
                let created_at = date_time_at(case, &started.body, "creationTime");
                let fetched = connecthealth(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/domain/{domain_id}/patient-insights-job/{job_id}"),
                    None,
                );
                assert_eq!(
                    fetched.status, 200,
                    "case {case:?}: GetPatientInsightsJob must answer 200, got {} with {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    date_time_at(case, &fetched.body, "creationTime"),
                    created_at,
                    "case {case:?}: the reader must report the instant the start recorded, \
                     got {:?}",
                    fetched.body
                );
                let updated_at = date_time_at(case, &fetched.body, "updatedTime");
                assert_eq!(
                    updated_at, created_at,
                    "case {case:?}: a job that has not transitioned was last updated when it \
                     was created, got {:?}",
                    fetched.body
                );

                // And once the clock has moved it, the instants are still date-time strings
                // and the last change is not before the creation.
                let done = poll_until_terminal(&router, &runtime, case, &domain_id, &job_id);
                let created_again = date_time_at(case, &done, "creationTime");
                let moved = date_time_at(case, &done, "updatedTime");
                assert_eq!(
                    created_again, created_at,
                    "case {case:?}: the creation instant must not move, got {done}"
                );
                assert!(
                    moved.as_nanos() >= created_at.as_nanos(),
                    "case {case:?}: a transitioned job's last change must not precede its \
                     creation, got {done}"
                );
            }
        })
        .expect("spawn the insights wire-form thread")
        .join()
        .expect("the job pair's date-time members must travel as strings");
}

#[test]
fn a_completed_job_stores_the_objects_its_response_reports() {
    std::thread::Builder::new()
        .name("connecthealth-insights-objects".to_string())
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
            let (domain_id, _) = domain(&router, &runtime, "clinic-insights-objects");

            // The rows differ along what rule 3 generalises over: the shape of the
            // configured output path, the identifier the patient is named by, and how many
            // input sources the start configures for reading that patient's record.
            let rows: Vec<(&str, &str, usize, &str)> = vec![
                (
                    "a bucket root and one input source",
                    "patient-0001",
                    1,
                    "s3://clinic-insights-wire",
                ),
                (
                    "a prefix with a trailing separator and two input sources",
                    "mrn-77810",
                    2,
                    "s3://clinic-insights-wire/output/",
                ),
                (
                    "a deep prefix and three input sources",
                    "patient.beta_2",
                    3,
                    "s3://clinic-insights-wire/2026/01/insights",
                ),
            ];

            for (case, patient_id, sources, path) in &rows {
                let started = connecthealth(
                    &router,
                    &runtime,
                    "POST",
                    &format!("/domain/{domain_id}/patient-insights-job"),
                    Some(job_body(patient_id, *sources, path)),
                );
                assert_eq!(
                    started.status, 200,
                    "case {case:?}: StartPatientInsightsJob must answer 200, got {} with {:?}",
                    started.status, started.body
                );
                let job_id = started.string_at("jobId");
                let done = poll_until_terminal(&router, &runtime, case, &domain_id, &job_id);
                assert_eq!(
                    done["jobStatus"].as_str(),
                    Some("SUCCEEDED"),
                    "case {case:?}: a job whose output was stored must report the modelled \
                     success status, got {done}"
                );

                // The per-patient prefix the two objects of this job sit under, by the rule:
                // the configured path with any trailing separator collapsed, then the job
                // and the patient it ran for.
                let prefix = format!("{}/{job_id}/{patient_id}", path.trim_end_matches('/'));
                let summary_uri = format!("{prefix}/{SUMMARY_OBJECT}");
                let metadata_uri = format!("{prefix}/{PROCESSING_METADATA_OBJECT}");
                assert_eq!(
                    done["insightsOutput"],
                    json!({ "uri": summary_uri }),
                    "case {case:?}: a completed job must report the summary object it wrote, \
                     got {done}"
                );

                // What it reports about the run: the fixed keys, the count of the ONE
                // patient record the job summarised — which is what the count follows, so a
                // row configuring three input SOURCES for reading that one record reports
                // the same count as a row configuring one — and the metadata object beside
                // the summary.
                let detail: Value = serde_json::from_str(
                    done["statusDetails"].as_str().unwrap_or_else(|| {
                        panic!("case {case:?}: a completed job must report statusDetails, \
                             got {done}")
                    }),
                )
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: statusDetails must be a JSON document: {error}, \
                         got {done}")
                });
                assert_eq!(
                    detail,
                    json!({
                        "documentProcessing": { "total": 1, "processed": 1 },
                        "processingMetadataUri": metadata_uri,
                    }),
                    "case {case:?}: a completed job must report the one patient record it \
                     summarised and where its processing metadata went, got {done}"
                );

                // And both objects are where it says they are, read back through the service
                // that OWNS them.
                let summary = object(&router, &runtime, case, &summary_uri);
                assert_eq!(
                    summary["TotalCount"].as_u64(),
                    Some(1),
                    "case {case:?}: the summary document must count the one patient it is \
                     about, got {summary}"
                );
                assert_eq!(
                    summary["PatientSummaries"][0]["PatientSummary"]["PatientId"].as_str(),
                    Some(*patient_id),
                    "case {case:?}: the summary document must be about the patient the job \
                     ran for, got {summary}"
                );
                let metadata = object(&router, &runtime, case, &metadata_uri);
                assert_eq!(
                    (
                        metadata["jobId"].as_str(),
                        metadata["patientId"].as_str(),
                        metadata["documentProcessing"]["total"].as_u64(),
                        metadata["documentProcessing"]["processed"].as_u64(),
                    ),
                    (Some(job_id.as_str()), Some(*patient_id), Some(1), Some(1)),
                    "case {case:?}: the stored metadata document must state the run the job \
                     reports, got {metadata}"
                );
            }
        })
        .expect("spawn the insights object thread")
        .join()
        .expect("a completed job must store the objects its response reports");
}

/// The JSON document at an `s3://` URI, read through Amazon S3's own `GetObject` on the
/// assembled router.
fn object(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    uri: &str,
) -> Value {
    let rest = uri
        .strip_prefix("s3://")
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} must be an s3:// URI"));
    let (bucket, key) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {uri:?} must name an object"));
    let fetched = send(
        router,
        runtime,
        S3,
        "GET",
        &format!("/{bucket}/{key}"),
        &[],
        Vec::new(),
    );
    assert_eq!(
        fetched.status, 200,
        "case {case:?}: the object at {uri} must exist, got {} with {:?}",
        fetched.status,
        String::from_utf8_lossy(&fetched.bytes)
    );
    serde_json::from_slice(&fetched.bytes).unwrap_or_else(|error| {
        panic!("case {case:?}: the object at {uri} must be a JSON document: {error}")
    })
}
