// SPDX-License-Identifier: CC0-1.0

//! The medical coding jobs of this service: what a `StartMedicalCodingJob` persists, the
//! lifecycle the clock moves one through, and the document a completed job leaves at the
//! `s3OutputPath` the start configured.
//!
//! A job is held UNDER the `Domain` it was started in (`Domain::medical_coding_jobs`), the
//! same scoping a patient insights job has: one domain's job is not reachable through
//! another's, and a domain's jobs go away with the domain.
//!
//! WHAT THE DOCUMENT CARRIES, and what it does not. The operation generates medical codes
//! (ICD-10 and CPT) for clinical text
//! (`smithy_query(query="doc",shape="StartMedicalCodingJob",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! which is the product of a coding model, and this emulator runs none. So the stored
//! document states the run — which job, over which text, with which context and dataset,
//! and when it finished — and reports an EMPTY code set, exactly as the sibling job's
//! summary states that it carries no risk-adjustment coding
//! ([`super::insights_run`]'s `HccIncluded`): a document that says it carries no codes,
//! rather than codes nothing produced. Nothing in it is read off the clinical text.
//!
//! The object itself is real: it is written into the caller's bucket through the one place
//! this crate writes into an object store ([`super::object_store_write`]), so the location
//! the job reports is a location the service that owns that bucket serves — and a job whose
//! write the store REFUSED reports the failure instead of a location nothing is at.

use std::time::{Duration, SystemTime};

use smithy_server_runtime::EmbeddedEmulator;

use crate::ops::types::{
    MedicalCodesEncounterContext, MedicalCodesPatientContext, MedicalCodingOutput,
    MedicalCodingOutputDataConfig,
};

use super::{service_state, to_date_time_text, GetMedicalCodingJobOutput, JobStatus};

/// How long a medical coding job sits in ONE non-terminal `JobStatus` before the transition
/// the clock owes it is materialised (`SUBMITTED` -> `IN_PROGRESS`, then `IN_PROGRESS` -> a
/// terminal status).
///
/// AWS documents no duration for a medical coding job, so the span is CHOSEN, and chosen as
/// the sibling job pair's is ([`super::PATIENT_INSIGHTS_JOB_STEP_SPAN`]): a caller that
/// reads straight after the start still observes `SUBMITTED` rather than a job that was
/// already finished by the time the start returned.
pub const MEDICAL_CODING_JOB_STEP_SPAN: Duration = Duration::from_millis(400);

/// The most medical coding jobs ONE `Domain` holds.
///
/// The EMULATOR's bound on this store's growth rather than a published quota, and the same
/// count the sibling job kind is bounded by
/// ([`super::MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN`]): `StartMedicalCodingJob` is the one
/// write that adds a job, so a caller looping it cannot grow the shared store without
/// limit.
pub const MAX_MEDICAL_CODING_JOBS_PER_DOMAIN: usize = 100;

/// The object a completed medical coding job stores its generated codes as, which is the
/// object its reported `medicalCodingOutput.uri` names.
pub const MEDICAL_CODES_OBJECT_NAME: &str = "medical-codes.json";

/// What a job whose output could not be stored reports about its status.
///
/// `statusDetails` targets `NonEmptyString`
/// (`smithy_query(query="shape",shape="GetMedicalCodingJobResponse")`), so the text is this
/// service's own; it names nothing read off the request.
pub const CODES_NOT_STORED_DETAIL: &str =
    "The medical coding output could not be written to the configured output location.";

/// The job parameters a `StartMedicalCodingJob` request carries — every member of that
/// request except the `domainId` it is scoped by and the `clientToken` that identifies the
/// request rather than the job.
///
/// Stored once and rendered back by `GetMedicalCodingJob`, which reports each of them: the
/// reader projects the STORED parameters, so what a start persisted is what a later get
/// reads.
#[derive(Debug, Clone)]
pub struct MedicalCodingJobSpec {
    /// The clinical text the job was started over. `@sensitive`, and reported by the
    /// operation's own reader as a `@required` member — so it is held, and held only here.
    pub text: String,
    pub patient_context: Option<MedicalCodesPatientContext>,
    pub encounter_context: Option<MedicalCodesEncounterContext>,
    pub output_data_config: MedicalCodingOutputDataConfig,
    /// The dataset the start named, which the job's codes are tailored to. `None` for a job
    /// that named none, which the model documents as using "the general medical coding
    /// knowledge base".
    pub dataset_id: Option<String>,
}

/// Where this job's codes document goes, and whether it has been stored.
///
/// Derived once, when the job is accepted, from the output location the request configured
/// and the job's own id, so the object lands at exactly the key the reported
/// `medicalCodingOutput.uri` names.
///
/// There is no claim on the work, unlike the sibling job's delivery: the write is a
/// `PutObject` of the same bytes at the same key, so two reads that both make it leave
/// exactly one object in exactly one state, and [`Self::stored`] only keeps a later read
/// from repeating work.
#[derive(Debug, Clone)]
pub struct CodesDelivery {
    /// The Region the object store is addressed in: the Region the start was signed for.
    pub region: String,
    /// The bucket the codes document is written into.
    pub bucket: String,
    /// The key it is written at — the key the reported `medicalCodingOutput.uri` names.
    pub key: String,
    /// Whether the object store has taken it.
    pub stored: bool,
}

/// A persisted medical coding job.
#[derive(Debug, Clone)]
pub struct MedicalCodingJob {
    pub job_id: String,
    pub domain_id: String,
    pub arn: String,
    /// The lifecycle status, as the model's `JobStatus` enum declares it — the SAME enum
    /// the patient insights job pair reports, so the two kinds of job of this service move
    /// through one vocabulary.
    pub status: JobStatus,
    /// What is reported about the CURRENT status; `None` while there is nothing to say about
    /// it. A job whose output the store refused reports that failure
    /// ([`CODES_NOT_STORED_DETAIL`]).
    pub status_details: Option<String>,
    /// Where this job's codes were written: filled by the terminal success transition from
    /// the location the start configured, and `None` until then.
    pub medical_coding_output: Option<MedicalCodingOutput>,
    pub created_at: SystemTime,
    /// When the record last changed — the instant of its last status transition, which is
    /// the creation instant for a job that has not moved yet.
    pub updated_at: SystemTime,
    /// When the next status transition the clock owes becomes due; `None` once the job is in
    /// a terminal status and owes none.
    pub next_transition_at: Option<SystemTime>,
    /// The delivery this job's completion makes, or `None` for a job with no output location
    /// to write to.
    pub output_delivery: Option<CodesDelivery>,
    /// The parameters the start persisted, reported back by the reader.
    pub spec: MedicalCodingJobSpec,
}

impl MedicalCodingJob {
    /// Where this job's codes document goes: UNDER the `s3OutputPath` the start configured,
    /// in a prefix of the job's own id, at [`MEDICAL_CODES_OBJECT_NAME`] — derived through
    /// the one rule this crate derives a job's output location with
    /// ([`super::job_output_uri`]), so the member the reader answers with and the key the
    /// delivery writes at cannot drift apart.
    ///
    /// `None` when no object location can be derived inside the bound the member it fills
    /// declares, which is a job with no output location to report.
    pub fn output_location(&self) -> Option<MedicalCodingOutput> {
        super::job_output_uri(
            &self.spec.output_data_config.s3_output_path,
            &[&self.job_id],
            MEDICAL_CODES_OBJECT_NAME,
        )
        .map(|uri| MedicalCodingOutput { uri })
    }

    /// The delivery this job's completion will make: the object it writes and where it goes,
    /// derived from the location the job itself reports through the workspace's one reading
    /// of an `s3://` URI — so the document lands at exactly the key
    /// `medicalCodingOutput.uri` names.
    pub fn owed_delivery(&self, region: &str) -> Option<CodesDelivery> {
        let (bucket, key) = ::service_common::s3_uri::object_address(&self.output_location()?.uri)?;
        Some(CodesDelivery {
            region: region.to_string(),
            bucket,
            key,
            stored: false,
        })
    }

    /// Apply every status transition the CLOCK has already made due.
    ///
    /// A non-terminal job owes exactly one transition at a time, one
    /// [`MEDICAL_CODING_JOB_STEP_SPAN`] after the previous one: `SUBMITTED` ->
    /// `IN_PROGRESS`, then `IN_PROGRESS` -> `SUCCEEDED`. Each transition is stamped with the
    /// instant it FELL DUE rather than the instant somebody happened to look, so the record
    /// reads the same however late the first read comes.
    ///
    /// Completing is what fills [`Self::medical_coding_output`] — the location is this job's
    /// own — and what leaves the DELIVERY due: the document the job writes at exactly that
    /// location, which the job's own reader makes before it answers, so a caller polling the
    /// job to completion finds it.
    pub(super) fn apply_due_transitions(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at.filter(|due| *due <= now) {
            match self.status {
                JobStatus::Submitted => {
                    self.status = JobStatus::InProgress;
                    self.next_transition_at = Some(due + MEDICAL_CODING_JOB_STEP_SPAN);
                }
                JobStatus::InProgress => {
                    self.status = JobStatus::Succeeded;
                    self.medical_coding_output = self.output_location();
                    self.next_transition_at = None;
                }
                // A terminal status owes nothing; a due instant left on one is dropped
                // rather than acted on.
                JobStatus::Failed | JobStatus::Succeeded => self.next_transition_at = None,
            }
            self.updated_at = due;
        }
    }

    /// Whether this job still owes the write its completion makes.
    pub(super) fn delivery_is_due(&self) -> bool {
        self.status == JobStatus::Succeeded
            && self
                .output_delivery
                .as_ref()
                .is_some_and(|delivery| !delivery.stored)
    }

    /// Record what the write established about this job's codes document.
    ///
    /// A document the store TOOK is where the job says it is, and the terminal success status
    /// the clock already applied stands. One the store REFUSED makes the job `FAILED`,
    /// states that failure in `statusDetails` and clears the output location: a job whose
    /// document is not where it would report it did not succeed. One NOTHING answered
    /// establishes nothing about the write, so it is left owed — the next read of this job
    /// makes it, which is what keeps the object in place for an assembly where an object
    /// store is mounted later.
    ///
    /// The last-changed instant is NOT moved: it is the instant the job's transition fell
    /// due, so the record reads the same however late the read that made the write came.
    pub(super) fn settle_delivery(&mut self, stored: Option<bool>) {
        let Some(stored) = stored else {
            return;
        };
        if let Some(delivery) = self.output_delivery.as_mut() {
            delivery.stored = true;
        }
        if stored {
            return;
        }
        self.status = JobStatus::Failed;
        self.status_details = Some(CODES_NOT_STORED_DETAIL.to_string());
        self.medical_coding_output = None;
    }

    /// This record as the `GetMedicalCodingJobResponse` its reader answers with: every
    /// member read back out of what the start persisted and what the clock has since
    /// written.
    pub fn get_output(&self) -> GetMedicalCodingJobOutput {
        GetMedicalCodingJobOutput {
            job_id: self.job_id.clone(),
            job_arn: self.arn.clone(),
            job_status: self.status.as_wire().to_string(),
            creation_time: Some(super::to_timestamp(self.created_at)),
            updated_time: Some(super::to_timestamp(self.updated_at)),
            text: self.spec.text.clone(),
            patient_context: self.spec.patient_context.clone(),
            encounter_context: self.spec.encounter_context.clone(),
            output_data_config: self.spec.output_data_config.clone(),
            dataset_id: self.spec.dataset_id.clone(),
            medical_coding_output: self.medical_coding_output.clone(),
            status_details: self.status_details.clone(),
        }
    }
}

/// The document a completed job stores at the location it reports.
///
/// The model declares no shape for it — `medicalCodingOutput` carries only the URI — so its
/// member names are this service's own, stated here once. Every VALUE is read off the job's
/// own record: which job ran, when it finished, the dataset its codes were tailored to, how
/// much text it was given, and the code sets it produced — EMPTY, with the count stated,
/// because this emulator runs no coding model (see this module's own documentation).
/// Nothing is read off the clinical text, which is `@sensitive` and appears nowhere in the
/// document.
fn codes_document(job: &MedicalCodingJob) -> Vec<u8> {
    let document = ::serde_json::json!({
        "JobId": job.job_id,
        "GeneratedAt": to_date_time_text(job.updated_at),
        "DatasetId": job.spec.dataset_id,
        "InputCharacterCount": job.spec.text.chars().count(),
        "MedicalCodes": {
            "Icd10Codes": [],
            "CptCodes": [],
            "TotalCount": 0,
        },
    });
    // A document this service built from values it holds, so it serializes.
    ::serde_json::to_vec(&document).unwrap_or_default()
}

/// Make the write the job `job_id` under `domain_id` owes, when the clock has completed it.
///
/// Called by the job's own reader before it reports: the write was owed from the instant the
/// job's completion fell due, whether or not anybody polled, and making it here is what
/// keeps the object in place by the time a read reports the job terminal. A job that owes no
/// write, or has had it made, is left exactly as it was found. This service's store is not
/// held across the call into the service that owns the bucket.
pub async fn deliver_medical_codes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    region: &str,
    domain_id: &str,
    job_id: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let owed = {
        let store = state.lock();
        store
            .live_domain(region, domain_id)
            .and_then(|domain| domain.medical_coding_jobs.get(job_id))
            .filter(|job| job.delivery_is_due())
            .and_then(|job| {
                job.output_delivery
                    .clone()
                    .map(|delivery| (delivery, codes_document(job)))
            })
    };
    let Some((delivery, document)) = owed else {
        return;
    };
    let stored = super::object_store_write::write_objects(
        callback,
        &delivery.region,
        &delivery.bucket,
        &[(delivery.key, document)],
    )
    .await;
    let mut store = state.lock();
    if let Some(job) = store
        .live_domain_mut(region, domain_id)
        .and_then(|domain| domain.medical_coding_jobs.get_mut(job_id))
    {
        job.settle_delivery(stored);
    }
}
