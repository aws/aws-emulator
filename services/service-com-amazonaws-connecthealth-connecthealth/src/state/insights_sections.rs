// SPDX-License-Identifier: CC0-1.0

//! The SECTIONS of the summary document a completed patient insights job stores: the fixed
//! vocabulary of insight sections the generator writes, and the narrative each one states
//! about the patient.
//!
//! # What a section is
//!
//! The summary object is not a modelled shape — `GetObject`'s body is a streaming payload,
//! so the object store serves back whatever bytes were stored — so the document's
//! composition is RECORDED behaviour: `PatientSummaries[].PatientSummary.Sections` carries
//! one entry per insight section, in the generator's fixed order
//! ([`SummarySection::ALL`]), and each entry carries
//!
//! * `SectionName`, the section's markdown HEADING — `# <SECTION_TOKEN>\n`
//!   ([`SummarySection::heading`]) over the fixed token vocabulary
//!   ([`SummarySection::token`]) — which is what a consumer walks the document by; and
//! * `ClinicalNarrative`, a list whose EVERY element carries BOTH `Text`, the narrative of
//!   that section, and `Evidence`, the document references that narrative cites — present
//!   and EMPTY when it cites none, so a reader walking the pairs never meets an absent
//!   member.
//!
//! # Where each narrative comes from
//!
//! What a narrative can state is bounded by what this emulator establishes about the
//! patient. The insights a clinical summarisation model composes are not here — this
//! emulator runs no such model — and nothing in this module composes prose that would read
//! like a summary no model wrote. What this service DOES hold about a patient is the VISITS
//! it has run insights jobs for in the same domain: one per `encounterContext` a start
//! carried, whose `encounterReason` is the "chief complaint for the visit"
//! (`smithy_query(query="shape",shape="PatientInsightsEncounterContext",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`).
//! So, over that record ([`PatientVisitRecord`]):
//!
//! * the overview section states the overview's own markdown subheading
//!   ([`OVERVIEW_NARRATIVE`]) and no prose under it, because the patient's clinical overview
//!   is a summarisation model's output and the identity of this summary travels in the
//!   document's own members instead;
//! * the since-last-visit section states WHEN the patient's previous visit with this
//!   provider was ([`previous_visit_narrative`]), or that no previous visit was found
//!   ([`FIRST_VISIT_NARRATIVE`]);
//! * the trends section states the chief complaint the patient has presented with at more
//!   than one of those visits ([`recurring_complaint_narrative`]), or that no active trend
//!   requires monitoring ([`NO_ACTIVE_TRENDS_NARRATIVE`]).
//!
//! Nothing here cites document EVIDENCE: these narratives are drawn from this service's own
//! visit record rather than from a clinical document, so every `Evidence` list is present
//! and empty.

use std::time::SystemTime;

use super::PatientInsightsJob;

/// One section of the summary document's fixed vocabulary.
///
/// The tokens and their ORDER are the generator's, so a consumer that walks the document by
/// heading finds every section it knows, at the place the document always carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarySection {
    /// The patient-and-encounter overview: the document's first section.
    PatientAndEncounterOverview,
    /// What has changed since the patient's previous visit with this provider.
    SinceLastVisit,
    /// The trends the patient's record supports monitoring.
    Trends,
}

impl SummarySection {
    /// Every section the generator writes, in the order the document carries them.
    pub const ALL: [Self; 3] = [
        Self::PatientAndEncounterOverview,
        Self::SinceLastVisit,
        Self::Trends,
    ];

    /// The token this section is named by.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::PatientAndEncounterOverview => "PATIENT_AND_ENCOUNTER_OVERVIEW",
            Self::SinceLastVisit => "SINCE_LAST_VISIT",
            Self::Trends => "TRENDS",
        }
    }

    /// The `SectionName` this section carries: its token as a markdown heading, which is the
    /// form every section of the document is named by.
    #[must_use]
    pub fn heading(self) -> String {
        format!("# {}\n", self.token())
    }

    /// The narrative text this section states for `job`, over the patient's visit `record`.
    #[must_use]
    pub fn narrative(self, job: &PatientInsightsJob, record: &PatientVisitRecord) -> String {
        match self {
            Self::PatientAndEncounterOverview => OVERVIEW_NARRATIVE.to_string(),
            Self::SinceLastVisit => record.since_last_visit_narrative(),
            Self::Trends => record.trends_narrative(&job.spec.encounter_context.encounter_reason),
        }
    }
}

/// The narrative the overview section states: the overview's own markdown subheading, with
/// no prose beneath it.
///
/// RECORDED behaviour, and the bound on what this emulator can say there: the
/// patient-and-encounter overview a reader expects under this heading is the product of a
/// clinical summarisation model, and this emulator runs none, so the section carries the
/// subheading of the overview and nothing composed to read like one. What the service does
/// establish about the patient and the encounter travels in the document's own members —
/// `JobId`, `PatientId`, `SummaryType` and `GeneratedAt`.
pub const OVERVIEW_NARRATIVE: &str = "## Overview\n";

/// The narrative the since-last-visit section states for a patient whose record holds no
/// earlier visit with this provider.
///
/// RECORDED behaviour: the whole sentence is fixed — it names nothing of the request and
/// nothing of the patient's record, because there is no previous visit to state anything
/// about.
pub const FIRST_VISIT_NARRATIVE: &str =
    "This is the patient's first visit with this provider (no previous visit found).\n";

/// The narrative the since-last-visit section states for a patient whose record holds an
/// earlier visit with this provider: WHEN that visit was.
///
/// The fixed text is the statement; the one variable part is the calendar day of the
/// previous visit, read off this service's own record of it ([`super::to_date_text`]).
#[must_use]
pub fn previous_visit_narrative(previous_visit_at: SystemTime) -> String {
    format!(
        "The patient's previous visit with this provider was on {}.\n",
        super::to_date_text(previous_visit_at)
    )
}

/// The narrative the trends section states for a patient whose visits support no trend.
///
/// RECORDED behaviour: a fixed sentence, which is what a trends section with nothing to
/// monitor states.
pub const NO_ACTIVE_TRENDS_NARRATIVE: &str = "No active trends requiring monitoring.\n";

/// The narrative the trends section states for a chief complaint the patient has presented
/// with at more than one visit with this provider: the complaint, and how many of the
/// patient's visits it was the reason for.
///
/// The fixed text is the statement; the variable parts are the chief complaint of the visit
/// this summary is for and the number of visits it was recorded at, both read off this
/// service's own record of those visits. This is the ONE narrative that carries a value the
/// request supplied, and only for a patient whose record shows the complaint recurring: a
/// document for a first visit, or for a complaint recorded once, states nothing of the
/// request at all. The value is safe to carry inside the document's markdown because
/// `encounterReason` admits `^[a-zA-Z0-9 .,-]+$`
/// (`smithy_query(query="shape",shape="PatientInsightsEncounterContext",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
/// which the request boundary enforces, so no heading or line break of the document's own
/// syntax can arrive in it.
#[must_use]
pub fn recurring_complaint_narrative(reason: &str, visits: usize) -> String {
    format!("{reason}: recorded at {visits} visits with this provider.\n")
}

/// How many of a patient's visits one chief complaint has to be the reason for before the
/// trends section states it: a complaint recorded at ONE visit is that visit's own reason for
/// being, and a trend is what recurs across visits.
const VISITS_A_TREND_RECURS_OVER: usize = 2;

/// What this service's own record establishes about the VISITS the patient a job ran for has
/// had with the same provider — the domain the job was started in.
///
/// A visit is one patient insights job started for that patient in that domain: every start
/// carries the `encounterContext` of the encounter it is summarising for, so the jobs a
/// domain holds for one patient ARE that patient's visit record here. A job counts whatever
/// status it reached, because what makes the visit a visit is the encounter the start
/// recorded, not how the run that summarised it ended.
///
/// Only visits at or before the job's own count, ordered by the instant each was started and
/// by job id for two started at the same instant. So the document a delivery writes states
/// the same record however late the write is made, and a visit started after this one never
/// changes a summary that was already generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatientVisitRecord {
    /// When the patient's most recent EARLIER visit with this provider was; `None` when the
    /// record holds none, which is the patient's first visit.
    previous_visit_at: Option<SystemTime>,
    /// How many of those visits — this one included — were for the same chief complaint.
    visits_with_this_complaint: usize,
}

impl PatientVisitRecord {
    /// Read the record of the patient `job` ran for out of `visits`, the insights jobs the
    /// domain `job` belongs to holds.
    #[must_use]
    pub fn of<'a>(
        job: &PatientInsightsJob,
        visits: impl IntoIterator<Item = &'a PatientInsightsJob>,
    ) -> Self {
        let here = (job.created_at, job.job_id.as_str());
        let mut record = Self {
            previous_visit_at: None,
            visits_with_this_complaint: 0,
        };
        for visit in visits {
            if visit.spec.patient_context.patient_id != job.spec.patient_context.patient_id {
                continue;
            }
            let started = (visit.created_at, visit.job_id.as_str());
            if started > here {
                continue;
            }
            if started < here {
                record.previous_visit_at = record.previous_visit_at.max(Some(visit.created_at));
            }
            if visit.spec.encounter_context.encounter_reason
                == job.spec.encounter_context.encounter_reason
            {
                record.visits_with_this_complaint += 1;
            }
        }
        record
    }

    /// The narrative the since-last-visit section states for this record.
    #[must_use]
    pub fn since_last_visit_narrative(&self) -> String {
        match self.previous_visit_at {
            Some(previous_visit_at) => previous_visit_narrative(previous_visit_at),
            None => FIRST_VISIT_NARRATIVE.to_string(),
        }
    }

    /// The narrative the trends section states for this record, whose visit is for the chief
    /// complaint `reason`.
    #[must_use]
    pub fn trends_narrative(&self, reason: &str) -> String {
        match self.visits_with_this_complaint >= VISITS_A_TREND_RECURS_OVER {
            true => recurring_complaint_narrative(reason, self.visits_with_this_complaint),
            false => NO_ACTIVE_TRENDS_NARRATIVE.to_string(),
        }
    }
}

/// The `Sections` list the summary document of `job` carries: one entry per section of the
/// fixed vocabulary, in its fixed order, each naming its section by its markdown heading and
/// carrying the narrative that section states over the patient's visit `record`.
///
/// Each section states ONE narrative, which is what the recorded document carries, and every
/// narrative element carries both of the pair a consumer reads — its `Text` and its
/// `Evidence`, which is present and empty because these narratives cite no document.
pub(super) fn summary_sections(
    job: &PatientInsightsJob,
    record: &PatientVisitRecord,
) -> Vec<::serde_json::Value> {
    SummarySection::ALL
        .iter()
        .map(|section| {
            ::serde_json::json!({
                "SectionName": section.heading(),
                "ClinicalNarrative": [
                    {
                        "Text": section.narrative(job, record),
                        "Evidence": ::serde_json::Value::Array(Vec::new()),
                    }
                ],
            })
        })
        .collect()
}
