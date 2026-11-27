// SPDX-License-Identifier: CC0-1.0

//! What a patient insights job's RUN does OUTSIDE this service: it resolves the patient
//! the request names against the FHIR server the request names, and it writes the
//! insights document the finished job reports into the object store the request
//! configured.
//!
//! # Why a job reaches outside at all
//!
//! `StartPatientInsightsJob` takes `inputDataConfig.fhirServer.fhirEndpoint` — "FHIR
//! server endpoint URL for accessing patient data" — and
//! `patientContext.patientId`, "unique identifier of the patient"
//! (`smithy_query(query="shape",shape="FHIRServer,PatientInsightsPatientContext",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
//! The patient data a job runs on is therefore held by THAT server, not by this service,
//! and a job for a patient the server does not hold has no input: RECORDED behaviour is
//! that such a submission is refused synchronously, with the fixed sentence
//! [`PATIENT_NOT_IN_DATASTORE_MESSAGE`], and that the refusal registers nothing — a
//! second attempt against the same unseeded id is refused identically.
//!
//! `outputDataConfig.s3OutputPath` is "S3 URI where the insights output will be stored",
//! and `GetPatientInsightsJob` reports `insightsOutput.uri` once the job has run. So the
//! completion of a job is not the status field alone: RECORDED behaviour is that the
//! object at that URI EXISTS once the job reports the terminal success status, as does the
//! processing-metadata object beside it under the same per-patient prefix
//! ([`super::patient_insights_output_uri`]). The job's completion therefore leaves a
//! DELIVERY due ([`InsightsDelivery`]), which the job's own reader makes before it reports
//! ([`deliver_job_output`]) — so the read that finds the delivery owed writes the objects
//! before it answers, and a delivery the object store REFUSES takes the job to the
//! modelled failure status instead. The guarantee is that reader's: a SECOND read arriving
//! while the first still holds the claim is answered the terminal status the clock already
//! applied, whose objects the holder of the claim is in the middle of writing. One read of
//! a job polled to completion therefore finds its objects; two concurrent reads of the
//! same instant may not.
//!
//! What those objects CARRY is bounded by what this emulator can establish. The summary
//! document's STRUCTURE is the generator's: one entry per insight section, each naming its
//! section by a markdown heading and carrying narrative/evidence pairs — that composition,
//! and the narrative each section states, live in [`super::insights_sections`]. Its other
//! members are the ones this service derives: which job ran, which patient it ran for, when
//! it finished and which kind of summary was asked for. The insights a clinical
//! summarisation model composes are not here, and nothing composes prose that would read
//! like a summary no model wrote — each narrative states what this service's own record of
//! the patient establishes. What the job reports in `statusDetails` is the
//! document-processing report of the run ([`processing_status_detail`]), which is what the
//! recorded response carries there.
//!
//! # How it reaches them
//!
//! Both hops are the in-process callback into this same emulator
//! ([`smithy_server_runtime::OperationContext::callback`]): no socket is opened, no name
//! is resolved and nothing outside this process is reachable. The FHIR read is addressed
//! to THIS emulator's own base URL ([`smithy_server_runtime::OperationContext::endpoint`])
//! followed by the PATH of the endpoint the request named — a caller-supplied authority is
//! never dialled and never selects what answers — and it is signed for the data store
//! service that serves a FHIR endpoint here, through the workspace's one in-process signer
//! ([`service_common::in_process_sigv4`]). The object write goes through the workspace's
//! one Amazon S3 client construction ([`service_common::s3_client`]).
//!
//! # What an answer establishes
//!
//! Only what the server ANSWERED. A FHIR server that answers that it holds no such
//! patient is a refusal of the submission; a server that answers nothing at all — no FHIR
//! endpoint is mounted in this assembly, or the endpoint the request named is not one this
//! emulator serves — establishes NOTHING about the patient, so the submission is accepted
//! rather than refused on a question nobody answered. The same reading applies to the
//! write ([`service_common::s3_object_read`] states it for reads): an object store that
//! ANSWERS a refusal is a job that could not store its work product, and a call nothing
//! answered leaves the transition the clock owed standing.

use std::time::{Duration, Instant};

use smithy_server_runtime::{EmbeddedEmulator, OperationContext};

use super::{
    service_state, summary_sections, ComAmazonawsConnecthealthConnecthealthState, JobStatus,
    PatientInsightsJob, PatientVisitRecord,
};

/// The FHIR resource type a patient is held as, which is the type a patient id is
/// resolved against (FHIR R4 `Patient`).
const PATIENT_RESOURCE_TYPE: &str = "Patient";

/// The FHIR search parameter a patient's business identifier is searched by (FHIR R4
/// `Patient.identifier`), used when the id the request carried is not the resource's own
/// logical id.
const IDENTIFIER_SEARCH_PARAMETER: &str = "identifier";

/// The SigV4 signing name of the service that serves a FHIR data store endpoint in this
/// emulator — the service whose own API hands a client the endpoint a request then names
/// here. The call is signed for it because the emulator verifies every request signature.
const FHIR_SIGNING_NAME: &str = "healthlake";

/// The media type a FHIR JSON representation travels as (FHIR R4 §2.2 "Content Types"),
/// which is what a FHIR server answers and what a FHIR client asks for.
const FHIR_JSON_CONTENT_TYPE: &str = "application/fhir+json";

/// The member a FHIR resource names its own type in, which is how an answer is told apart
/// from a response something other than a FHIR server wrote.
const FHIR_RESOURCE_TYPE_MEMBER: &str = "resourceType";

/// The FHIR resource a search answers with (FHIR R4 §3.1.1: a `searchset` Bundle).
const FHIR_BUNDLE: &str = "Bundle";

/// The FHIR resource a failure is stated as (FHIR R4 §2.4 `OperationOutcome`).
const FHIR_OPERATION_OUTCOME: &str = "OperationOutcome";

/// The member a `searchset` Bundle states its number of matches in (FHIR R4
/// `Bundle.total`).
const FHIR_BUNDLE_TOTAL_MEMBER: &str = "total";

/// The sentence a submission naming a patient the FHIR server does not hold is refused
/// with.
///
/// RECORDED behaviour: the WHOLE sentence is fixed — nothing of the request is
/// interpolated, which also keeps the `@sensitive` `patientId` out of the response
/// answering it.
pub const PATIENT_NOT_IN_DATASTORE_MESSAGE: &str = "Patient could not be found in datastore.";

/// The sentence a submission naming a FHIR endpoint that resolves to no data store is
/// refused with.
///
/// RECORDED behaviour, and the sibling of [`PATIENT_NOT_IN_DATASTORE_MESSAGE`]: the
/// endpoint member must name a data store that exists, and one that does not is refused
/// with this fixed text rather than with a statement about the patient, which nothing
/// could have been resolved against.
pub const DATASTORE_NOT_FOUND_MESSAGE: &str = "HealthLake datastore not found";

/// The most bytes of a FHIR answer this service reads.
///
/// The answer comes from another service of this emulator rather than from a caller, but
/// it is still a body this process buffers, so the read is bounded: a Patient resource and
/// a one-page search Bundle are orders of magnitude smaller.
const MAX_FHIR_ANSWER_BYTES: usize = 1 << 20;

/// How many matches the identifier search asks for: enough to establish that the data
/// store holds the patient, which is the only question asked of it.
const PATIENT_SEARCH_PAGE_SIZE: u32 = 1;

/// How long a CLAIMED delivery may be outstanding before another read may claim it again.
///
/// CHOSEN, and a liveness bound rather than a timeout on the work: the claim is made under
/// the state lock and reported back by the claimant, so a claimant that never reports back
/// — the read it was made on was dropped — would otherwise leave the job short of its
/// terminal status forever. It is measured on the MONOTONIC clock the claim is stamped
/// with, so a reclaim cannot race a claimant that is still working, not even across a
/// wall-clock jump.
const DELIVERY_RECLAIM_AFTER: Duration = Duration::from_secs(10);

/// What a patient insights job's output delivery still owes, and where it goes.
///
/// Derived once, when the job is accepted, from the output location the request configured
/// and the job's own id, so the objects land at exactly the keys the job's own location
/// members name — the summary at `insightsOutput.uri` and the processing metadata at the
/// `processingMetadataUri` of its `statusDetails`.
#[derive(Debug, Clone)]
pub struct InsightsDelivery {
    /// The Region the object store is addressed in: the Region the start was signed for.
    pub region: String,
    /// The bucket the output objects are written into.
    pub bucket: String,
    /// The key the summary object is written at — the key the reported
    /// `insightsOutput.uri` names.
    pub key: String,
    /// The key the processing-metadata object is written at, beside the summary under the
    /// same per-patient prefix; `None` when that sibling names no object address under the
    /// same bucket.
    pub metadata_key: Option<String>,
    /// How far this delivery has got.
    pub state: DeliveryState,
}

/// How far one job's output delivery has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// Nothing has been written yet, and no caller holds the work.
    Owed,
    /// A caller claimed the work at this (monotonic) instant and has not reported back.
    InFlight(Instant),
    /// The delivery was made, successfully or not; the job's terminal status states which.
    Done,
}

/// What asking the FHIR server about a patient established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatientResolution {
    /// The server holds the patient.
    Held,
    /// The server answered that it holds no such patient.
    Absent,
    /// The server answered that the endpoint resolves to no data store at all.
    NoDatastore,
    /// Nothing answered: the endpoint the request named is not one this emulator serves,
    /// or no FHIR server is mounted to ask. Nothing is established about the patient.
    Unanswered,
}

/// Resolve `patient_id` against the FHIR server `fhir_endpoint` names, through this
/// emulator's own in-process transport.
///
/// Two FHIR interactions, in the order a client resolves a patient by an id that may be
/// either kind of identifier: the `read` of `Patient/{id}` (the id as the resource's own
/// logical id), and, when that holds nothing, the type-level `search` on
/// `Patient?identifier=` (the id as the patient's business identifier). A `search` that
/// the server answers at all is also what tells an endpoint resolving to no data store
/// from one whose data store simply holds no such patient.
pub async fn resolve_patient(
    ctx: &OperationContext<'_>,
    region: &str,
    fhir_endpoint: &str,
    patient_id: &str,
) -> PatientResolution {
    let Some(base) = fhir_base(ctx, fhir_endpoint) else {
        return PatientResolution::Unanswered;
    };
    let read = fhir_get(
        ctx,
        region,
        &format!("{base}{PATIENT_RESOURCE_TYPE}/{}", uri_component(patient_id)),
    )
    .await;
    match read {
        FhirAnswer::Served(_) => return PatientResolution::Held,
        FhirAnswer::Unanswered => return PatientResolution::Unanswered,
        // A FHIR failure is not the end of the question: the id may be the patient's
        // business identifier rather than the resource's logical id, which is the search
        // below, and the same search is what distinguishes an absent data store.
        FhirAnswer::Refused(_) => {}
    }
    let searched = fhir_get(
        ctx,
        region,
        &format!(
            "{base}{PATIENT_RESOURCE_TYPE}?{IDENTIFIER_SEARCH_PARAMETER}={}&_count={PATIENT_SEARCH_PAGE_SIZE}",
            uri_component(patient_id)
        ),
    )
    .await;
    match searched {
        FhirAnswer::Served(bundle) => match holds_a_match(&bundle) {
            true => PatientResolution::Held,
            false => PatientResolution::Absent,
        },
        // A type-level search is an interaction every FHIR server serves, so the server
        // answering it with NOT FOUND is about the path it was sent to rather than about
        // the search: the endpoint names no data store. Any OTHER failure of the search —
        // a parameter the server does not support, a refusal to answer it, a fault — says
        // nothing about whether the data store exists OR whether it holds the patient, so
        // it establishes nothing and the submission is accepted.
        FhirAnswer::Refused(status) if status == FHIR_NOT_FOUND_STATUS => {
            PatientResolution::NoDatastore
        }
        FhirAnswer::Refused(_) | FhirAnswer::Unanswered => PatientResolution::Unanswered,
    }
}

/// What ONE FHIR interaction answered.
enum FhirAnswer {
    /// A FHIR server served a FHIR resource, and this is it.
    Served(::serde_json::Value),
    /// A FHIR server stated a failure (an `OperationOutcome`), with the status it carried:
    /// what that failure MEANS is the status's, so the reader of an answer keeps it.
    Refused(u16),
    /// Nothing that speaks FHIR answered at all.
    Unanswered,
}

/// The status a FHIR server answers for a path or a resource it holds nothing at (FHIR R4
/// §3.1.0.3: `404 Not Found` for "unknown resource"), which is the one failure of a
/// type-level search that is about the ENDPOINT rather than about the search.
const FHIR_NOT_FOUND_STATUS: u16 = 404;

/// The base URL one FHIR interaction is addressed at: THIS emulator's own base, followed
/// by the PATH of the endpoint the request named, ending in exactly one separator.
///
/// The authority a caller supplied is deliberately dropped: the hop is in-process, so what
/// answers is decided by this emulator's routing and never by a host in the request, and
/// nothing can be dialled. `None` for a member that is not a URL at all.
fn fhir_base(ctx: &OperationContext<'_>, fhir_endpoint: &str) -> Option<String> {
    let endpoint = ::url::Url::parse(fhir_endpoint.trim()).ok()?;
    let path = endpoint.path().trim_end_matches('/').to_string();
    let here = ctx.endpoint();
    Some(format!("{}{path}/", here.as_str().trim_end_matches('/')))
}

/// Send one signed FHIR `GET` to `url` over the in-process callback and read what it
/// answered.
async fn fhir_get(ctx: &OperationContext<'_>, region: &str, url: &str) -> FhirAnswer {
    use ::aws_smithy_runtime_api::client::http::HttpConnector;

    let host = ::url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string));
    let mut builder = ::http::Request::builder()
        .method("GET")
        .uri(url)
        .header("accept", FHIR_JSON_CONTENT_TYPE);
    if let Some(host) = host.as_deref() {
        builder = builder.header("host", host);
    }
    let Ok(mut request) = builder.body(Vec::new()) else {
        return FhirAnswer::Unanswered;
    };
    // The emulator verifies every request signature, so this call carries one too: the
    // credential the framework hands this service, signed through the workspace's one
    // in-process signer, under the signing name a FHIR data store endpoint is served on.
    if ::service_common::in_process_sigv4::sign_in_process_request(
        &mut request,
        FHIR_SIGNING_NAME,
        region,
        ctx,
    )
    .await
    .is_err()
    {
        return FhirAnswer::Unanswered;
    }
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let Ok(request) = ::aws_smithy_runtime_api::http::Request::try_from(request) else {
        return FhirAnswer::Unanswered;
    };
    let connector = ctx.callback().http_connector();
    let Ok(answer) = HttpConnector::call(&connector, request).await else {
        return FhirAnswer::Unanswered;
    };
    let status = answer.status().as_u16();
    let served = answer.status().is_success();
    let Some(bytes) = answer.body().bytes() else {
        return FhirAnswer::Unanswered;
    };
    if bytes.len() > MAX_FHIR_ANSWER_BYTES {
        return FhirAnswer::Unanswered;
    }
    // A call the front door refused reached no service at all, so it establishes nothing —
    // told from an addressed service's own answer by the workspace's one reading of that
    // refusal.
    if ::service_common::in_process_refusal::is_transport_refusal(bytes) {
        return FhirAnswer::Unanswered;
    }
    let Ok(document) = ::serde_json::from_slice::<::serde_json::Value>(bytes) else {
        return FhirAnswer::Unanswered;
    };
    // Only a FHIR document is an answer about the patient: a body written by anything
    // other than a FHIR server establishes nothing.
    let resource_type = document
        .get(FHIR_RESOURCE_TYPE_MEMBER)
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    match (served, resource_type.as_str()) {
        (false, FHIR_OPERATION_OUTCOME) => FhirAnswer::Refused(status),
        (true, PATIENT_RESOURCE_TYPE | FHIR_BUNDLE) => FhirAnswer::Served(document),
        _ => FhirAnswer::Unanswered,
    }
}

/// Whether a `searchset` Bundle reports a match.
///
/// `Bundle.total` is "the total number of matches" and `Bundle.entry[]` carries the
/// matches this page holds, so a Bundle that states no match and carries no entry is the
/// server saying it holds none.
fn holds_a_match(bundle: &::serde_json::Value) -> bool {
    let total = bundle
        .get(FHIR_BUNDLE_TOTAL_MEMBER)
        .and_then(::serde_json::Value::as_u64)
        .unwrap_or_default();
    let entries = bundle
        .get("entry")
        .and_then(::serde_json::Value::as_array)
        .map(|entries| entries.len())
        .unwrap_or_default();
    total > 0 || entries > 0
}

/// One path segment or query value, percent-encoded so every character a modelled
/// identifier may carry addresses the resource it names instead of changing the URI it
/// sits in. The server percent-decodes each segment and each value it reads, so an encoded
/// value arrives as it was sent.
fn uri_component(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, URI_COMPONENT).to_string()
}

/// The characters that travel as themselves in a path segment or a query value: RFC 3986's
/// unreserved set. Everything else is escaped, so no value can carry a separator of the
/// URI it sits in.
const URI_COMPONENT: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Make the output delivery the job `job_id` under `domain_id` owes, when the clock has
/// completed it and nobody else holds the work.
///
/// Called by the job's own reader before it reports: the delivery was owed from the
/// instant the job's completion fell due, whether or not anybody polled, and making it
/// here is what keeps both objects in place by the time a read reports the job terminal. A
/// job that owes no delivery, has had it made, or whose delivery is held by another caller
/// is left exactly as it was found.
pub async fn deliver_job_output<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    region: &str,
    domain_id: &str,
    job_id: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The guard is released before anything is written, so this service's store is never
    // held across a call into another service's.
    let claimed = {
        let mut store = state.lock();
        claim(&mut store, region, domain_id, job_id)
    };
    let Some(claim) = claimed else {
        return;
    };
    let stored = write_output(callback, &claim).await;
    let mut store = state.lock();
    if let Some(job) = store
        .live_domain_mut(region, domain_id)
        .and_then(|domain| domain.patient_insights_jobs.get_mut(job_id))
    {
        job.settle_delivery(stored);
    }
}

/// One claimed delivery: where the objects go and what they carry.
struct ClaimedDelivery {
    /// The Region the object store is addressed in.
    region: String,
    /// The bucket the objects are written into.
    bucket: String,
    /// The objects this delivery writes, in order: each one's key and the document it
    /// carries, both built from the job's own stored record.
    objects: Vec<(String, Vec<u8>)>,
}

/// Claim the delivery of the job named, or `None` when it owes none, when it has already
/// been made, or when another caller holds a claim on it that is not stale.
///
/// The claim is the write that makes a delivery exactly once: it moves the job's delivery
/// to [`DeliveryState::InFlight`] under the state lock, so only one caller is ever handed
/// the work and a poll in flight is not duplicated by the next one.
fn claim(
    store: &mut ComAmazonawsConnecthealthConnecthealthState,
    region: &str,
    domain_id: &str,
    job_id: &str,
) -> Option<ClaimedDelivery> {
    let domain = store.live_domain_mut(region, domain_id)?;
    let job = domain.patient_insights_jobs.get(job_id)?;
    if !job.delivery_is_due() {
        return None;
    }
    // What the patient's VISITS with this provider establish, read off the domain's own jobs
    // before the claim takes the job itself ([`PatientVisitRecord`]): the narratives of the
    // summary document are statements over that record.
    let visits = PatientVisitRecord::of(job, domain.patient_insights_jobs.values());
    let summary = summary_document(job, &visits);
    let metadata = processing_metadata_document(job);
    let job = domain.patient_insights_jobs.get_mut(job_id)?;
    let delivery = job.output_delivery.as_mut()?;
    let mut objects = vec![(delivery.key.clone(), summary)];
    if let Some(metadata_key) = delivery.metadata_key.clone() {
        objects.push((metadata_key, metadata));
    }
    delivery.state = DeliveryState::InFlight(Instant::now());
    Some(ClaimedDelivery {
        region: delivery.region.clone(),
        bucket: delivery.bucket.clone(),
        objects,
    })
}

/// The media type the output documents are stored with: they are JSON documents, and a
/// client reads the summary straight off the location the job reports.
const SUMMARY_CONTENT_TYPE: &str = "application/json";

/// The integrity checksum the write of an output document DECLARES, which is the checksum
/// the object store records for the object and reports back on a read that enables the
/// checksum mode (`GetObjectOutput$ChecksumCRC32`, under the `FULL_OBJECT` type a one-shot
/// write stores).
///
/// A service writes into a caller's bucket through an AWS SDK, and an SDK computes a CRC32
/// checksum for every upload it makes unless the caller asks for another algorithm (the
/// default request checksum calculation of every current SDK), so the objects a finished job
/// leaves behind carry that checksum and a client that downloads one can verify the bytes it
/// got. Named here rather than left to the transport: the in-process hop every emulated
/// service shares computes a checksum only where one is asked for
/// (`service_common::s3_client`), so this is the SERVICE stating what its write carries.
const OUTPUT_CHECKSUM_ALGORITHM: ::aws_sdk_s3::types::ChecksumAlgorithm =
    ::aws_sdk_s3::types::ChecksumAlgorithm::Crc32;

/// Write the objects of one claimed delivery, and report whether the object store took
/// them.
///
/// `Some(true)` when the store took every object, `Some(false)` when it ANSWERED a refusal
/// of one — the bucket does not exist, or the write is not permitted — which is a job that
/// could not store its work product. `None` when nothing answered the write at all: no
/// object store is mounted in this assembly, so nothing is established and the job keeps
/// the transition the clock owed it.
async fn write_output(callback: &EmbeddedEmulator, claim: &ClaimedDelivery) -> Option<bool> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let client = ::service_common::s3_client::s3_client(callback, &claim.region);
    for (key, document) in &claim.objects {
        let written = client
            .put_object()
            .bucket(&claim.bucket)
            .key(key)
            .content_type(SUMMARY_CONTENT_TYPE)
            .checksum_algorithm(OUTPUT_CHECKSUM_ALGORITHM)
            .body(::aws_sdk_s3::primitives::ByteStream::from(document.clone()))
            .send()
            .await;
        match written {
            Ok(_) => {}
            Err(error) => match error.code() {
                // The store ANSWERED: this job's output could not be stored where it was
                // configured to go.
                Some(code) => {
                    ::tracing::warn!(
                        bucket = %claim.bucket,
                        code = %code,
                        "a patient insights job's output was refused by the emulator's Amazon S3"
                    );
                    return Some(false);
                }
                // Nothing answered the write — the same reading
                // `service_common::s3_object_read` gives a read nothing answered.
                None => {
                    ::tracing::warn!(
                        bucket = %claim.bucket,
                        "nothing answered a patient insights job's output write: this emulator \
                         has no Amazon S3 to store it in"
                    );
                    return None;
                }
            },
        }
    }
    Some(true)
}

/// The `SummaryType` the summary document states for the `insightsType` a job was started
/// with.
///
/// `InsightsType` declares one value, `PRE_VISIT`
/// (`smithy_query(query="shape",shape="InsightsType",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
/// and the summary document states that kind of summary in the words a reader of the
/// document sees — RECORDED behaviour: `Pre-visit`. A value the model may add later is
/// stated as it was asked for rather than silently as a pre-visit summary.
fn summary_type(insights_type: &str) -> String {
    match insights_type {
        "PRE_VISIT" => "Pre-visit".to_string(),
        other => other.to_string(),
    }
}

/// The document a completed job stores at the location it reports.
///
/// Its SECTIONS are the generator's fixed vocabulary, one entry per insight section, each
/// naming its section by a markdown heading and carrying `Text`/`Evidence` narrative pairs
/// ([`summary_sections`] composes them over the patient's visit record). The rest of the
/// document is the members this service DERIVES — `TotalCount` (this job is about the one
/// patient it was started for), `GeneratedAt` (the instant the job completed), `HccIncluded`
/// ([`HCC_INCLUDED`]), `JobId`, `PatientId` (the id the start resolved) and `SummaryType`
/// (from the modelled `insightsType`).
///
/// The member names are RECORDED: `TotalCount`, `GeneratedAt`, `HccIncluded` and
/// `PatientSummaries[].PatientSummary` with `JobId`, `PatientId`, `SummaryType`,
/// `GeneratedAt` and `Sections[]` of `SectionName` and
/// `ClinicalNarrative[].Text`/`.Evidence`.
fn summary_document(job: &PatientInsightsJob, visits: &PatientVisitRecord) -> Vec<u8> {
    let spec = &job.spec;
    // The instant the job's completion FELL DUE, read off the record, so the document a
    // job stored states the same instant however late the read that stored it came.
    let generated_at = super::to_date_time_text(job.updated_at);
    let document = ::serde_json::json!({
        "TotalCount": PATIENTS_PER_JOB,
        "GeneratedAt": generated_at,
        "HccIncluded": HCC_INCLUDED,
        "PatientSummaries": [
            {
                "PatientSummary": {
                    "JobId": job.job_id,
                    "PatientId": spec.patient_context.patient_id,
                    "SummaryType": summary_type(&spec.insights_context.insights_type),
                    "GeneratedAt": generated_at,
                    "Sections": summary_sections(job, visits),
                }
            }
        ],
    });
    // A document this service built from values it holds, so it serializes.
    ::serde_json::to_vec(&document).unwrap_or_default()
}

/// The document a completed job stores beside its summary, at
/// [`super::PROCESSING_METADATA_OBJECT_NAME`].
///
/// It states the RUN rather than the insights: which job ran, for which patient, when it
/// finished, and how much of its input it got through — all of it read off the job's own
/// record. Nothing here is prose and nothing is generated: the counts are
/// [`PATIENTS_PER_JOB`], the one patient record the job ran for.
fn processing_metadata_document(job: &PatientInsightsJob) -> Vec<u8> {
    let document = ::serde_json::json!({
        "jobId": job.job_id,
        "patientId": job.spec.patient_context.patient_id,
        "generatedAt": super::to_date_time_text(job.updated_at),
        "documentProcessing": {
            "total": PATIENTS_PER_JOB,
            "processed": PATIENTS_PER_JOB,
        },
    });
    ::serde_json::to_vec(&document).unwrap_or_default()
}

/// How many patient records ONE patient insights job runs over: the one
/// `patientContext.patientId` the start named, which is the count the summary document
/// states as `TotalCount` and the processing document as the documents it got through.
const PATIENTS_PER_JOB: u64 = 1;

/// Whether the stored summary document carries hierarchical-condition-category coding.
///
/// RECORDED behaviour is that the document states this of itself. It is FALSE here because
/// HCC coding is the product of a risk-adjustment model, and this emulator runs none, so the
/// document says it carries none rather than claiming coding nothing produced.
const HCC_INCLUDED: bool = false;

/// What a job that ran to completion reports about its status: a JSON document stating how
/// much of its input it processed and where its processing metadata was written.
///
/// RECORDED behaviour: the keys are FIXED — `documentProcessing` with `total` and
/// `processed`, and `processingMetadataUri` — while every value is this job's own. A job
/// that ran to completion processed every patient record it was started for
/// ([`PATIENTS_PER_JOB`]), and the metadata location is the one this job reports and writes
/// ([`PatientInsightsJob::processing_metadata_uri`]).
///
/// `None` for a job with no metadata location to report, which is the same job that reports
/// no output location at all (a configured path with no room for an object under it): there
/// is nothing to state about objects that were never addressed.
///
/// `statusDetails` is "information about the status of a job" and targets `NonEmptyString`
/// (`smithy_query(query="operation",operation="GetPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
/// which this document satisfies.
///
/// The document is rendered through a serialized STRUCT rather than a JSON map, so the
/// members travel in the order the recorded document carries them whatever feature set the
/// JSON crate is built with.
pub fn processing_status_detail(job: &PatientInsightsJob) -> Option<String> {
    let detail = ProcessingStatusDetail {
        document_processing: DocumentProcessing {
            total: PATIENTS_PER_JOB,
            // A completed run processed every record it was given; a job that could not
            // process them does not reach the success status this detail belongs to.
            processed: PATIENTS_PER_JOB,
        },
        processing_metadata_uri: job.processing_metadata_uri()?,
    };
    // A document this service built from values it holds, so it serializes.
    ::serde_json::to_string(&detail).ok()
}

/// The document a completed job reports in `statusDetails`, in the recorded member order.
#[derive(Debug, ::serde::Serialize)]
struct ProcessingStatusDetail {
    #[serde(rename = "documentProcessing")]
    document_processing: DocumentProcessing,
    #[serde(rename = "processingMetadataUri")]
    processing_metadata_uri: String,
}

/// How much of a job's input it processed, as the recorded `documentProcessing` member
/// states it.
#[derive(Debug, ::serde::Serialize)]
struct DocumentProcessing {
    total: u64,
    processed: u64,
}

impl PatientInsightsJob {
    /// The delivery this job's completion will make: the objects it writes and where they
    /// go, derived from the locations the job itself reports
    /// ([`PatientInsightsJob::output_location`],
    /// [`PatientInsightsJob::processing_metadata_uri`]) through the workspace's one reading
    /// of an `s3://` URI — so each object lands at exactly the key its own location names,
    /// and the summary lands at the key `insightsOutput.uri` reports.
    ///
    /// `None` for a job whose configured output location names no object address at all:
    /// there is nothing to deliver, and nothing is written at a bare prefix instead.
    pub fn owed_delivery(&self, region: &str) -> Option<InsightsDelivery> {
        let (bucket, key) =
            ::service_common::s3_uri::object_address(&self.output_location()?.uri)?;
        // The sibling object goes into the SAME bucket as the summary: a location naming
        // another bucket, or no object at all, is not one this delivery writes.
        let metadata_key = self
            .processing_metadata_uri()
            .and_then(|uri| ::service_common::s3_uri::object_address(&uri))
            .filter(|(metadata_bucket, _)| *metadata_bucket == bucket)
            .map(|(_, metadata_key)| metadata_key);
        Some(InsightsDelivery {
            region: region.to_string(),
            bucket,
            key,
            metadata_key,
            state: DeliveryState::Owed,
        })
    }

    /// Whether this job still owes the delivery its completion makes.
    ///
    /// A job owes it from the instant the clock completed it — the terminal transition is
    /// what leaves the delivery due — until the delivery has been made. One another caller
    /// claimed is not due either, until that claim is stale ([`DELIVERY_RECLAIM_AFTER`]),
    /// which is how a claimant that never reported back leaves the object deliverable
    /// instead of never written.
    pub(super) fn delivery_is_due(&self) -> bool {
        self.status == JobStatus::Succeeded
            && self
                .output_delivery
                .as_ref()
                .is_some_and(|delivery| match delivery.state {
                    DeliveryState::Owed => true,
                    DeliveryState::InFlight(claimed_at) => {
                        claimed_at.elapsed() >= DELIVERY_RECLAIM_AFTER
                    }
                    DeliveryState::Done => false,
                })
    }

    /// Record what this delivery established about the job's output objects.
    ///
    /// A delivery whose objects the store TOOK is MADE: each object is now where the job
    /// says it is and the terminal success status the clock already applied stands, with the
    /// `statusDetails` that transition stated ([`processing_status_detail`]) left exactly as
    /// it was — the delivery confirms that report rather than replacing it. One the store
    /// REFUSED makes the job `FAILED`, restates `statusDetails` as that failure and clears
    /// the output location: a job whose output object is not where it would report it did
    /// not succeed.
    ///
    /// One NOTHING answered establishes nothing about the write, so it is left OWED rather
    /// than marked made: the next read of this job makes it, which is what keeps the object
    /// in place for an assembly where an object store is mounted later.
    ///
    /// The last-changed instant is NOT moved: it is the instant the job's transition fell
    /// due, so the record reads the same however late the read that made the delivery came.
    pub(super) fn settle_delivery(&mut self, stored: Option<bool>) {
        let Some(stored) = stored else {
            if let Some(delivery) = self.output_delivery.as_mut() {
                delivery.state = DeliveryState::Owed;
            }
            return;
        };
        if let Some(delivery) = self.output_delivery.as_mut() {
            delivery.state = DeliveryState::Done;
        }
        if stored {
            return;
        }
        self.status = JobStatus::Failed;
        self.status_details = Some(OUTPUT_NOT_STORED_DETAIL.to_string());
        self.insights_output = None;
    }
}

/// What a job whose output could not be stored reports about its status.
///
/// `statusDetails` is "information about the status of a job" and targets `NonEmptyString`
/// (`smithy_query(query="operation",operation="GetPatientInsightsJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
/// so the text is this service's own; it names nothing read off the request.
pub const OUTPUT_NOT_STORED_DETAIL: &str =
    "The insights output could not be written to the configured output location.";
