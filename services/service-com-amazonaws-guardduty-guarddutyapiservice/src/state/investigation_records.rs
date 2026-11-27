// SPDX-License-Identifier: CC0-1.0

//! The RECORDS of one investigation: the steps of the reasoning the pass followed,
//! which `ListInvestigationRecords` reports.
//!
//! "Each record is one step in the reasoning that GuardDuty followed to reach the
//! results of the investigation" (`smithy_query(query="doc",shape="ListInvestigationRecords")`),
//! so the records are the steps THIS service's pass actually took
//! ([`InvestigationRecord::settle`]): it was asked a question, it fetched the findings
//! the subject had produced, it noted each of them, it assessed them, it recommended what
//! to do, it answered, and it took some time doing so.
//!
//! Every record is derived from what the pass STORED on the investigation — the evidence
//! it read, the assessment it reached and the scope it resolved — so two reads of one
//! investigation report the same steps, with the same identifiers, however the store has
//! moved on since. Nothing here writes, and no step is reported for work nothing ran: an
//! investigation whose pass has not ended has no steps, which is also what the operation
//! documents ("Records are returned only when the status is COMPLETED").

use super::*;

use crate::ops::types::list_investigation_records_types as wire;

/// `RecordType`: "A progress milestone, such as the request that started the
/// investigation or a response from GuardDuty"
/// (`smithy_query(query="doc",shape="InvestigationRecord",member="Type")`). The enum is
/// `UPDATE | FETCH | OBSERVATION | ANALYSIS | METADATA`.
pub const RECORD_TYPE_UPDATE: &str = "UPDATE";

/// `RecordType`: "Data that GuardDuty retrieved for the investigation". See
/// [`RECORD_TYPE_UPDATE`] for the enum.
pub const RECORD_TYPE_FETCH: &str = "FETCH";

/// `RecordType`: "A fact that GuardDuty noted about your environment".
pub const RECORD_TYPE_OBSERVATION: &str = "OBSERVATION";

/// `RecordType`: "An assessment that GuardDuty derived from the data".
pub const RECORD_TYPE_ANALYSIS: &str = "ANALYSIS";

/// `RecordType`: "Operational details about the investigation itself, such as how long
/// the investigation took".
pub const RECORD_TYPE_METADATA: &str = "METADATA";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECORD_TYPE_VALUES,
    RECORD_TYPE_UPDATE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECORD_TYPE_VALUES,
    RECORD_TYPE_FETCH
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECORD_TYPE_VALUES,
    RECORD_TYPE_OBSERVATION
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECORD_TYPE_VALUES,
    RECORD_TYPE_ANALYSIS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECORD_TYPE_VALUES,
    RECORD_TYPE_METADATA
));

/// `FetchType`: "GuardDuty findings" — the only kind of data this service's pass
/// retrieves (`smithy_query(query="doc",shape="FetchRecord",member="FetchType")`).
pub const FETCH_TYPE_FINDINGS: &str = "FINDINGS";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::FETCH_TYPE_VALUES,
    FETCH_TYPE_FINDINGS
));

/// `ObservationType`: the observation carries `FindingDetail`
/// (`smithy_query(query="doc",shape="ObservationRecord",member="ObservationType")`).
pub const OBSERVATION_TYPE_FINDING_DETAIL: &str = "FINDING_DETAIL";

/// `ObservationType`: the observation carries `Recommendations`. See
/// [`OBSERVATION_TYPE_FINDING_DETAIL`].
pub const OBSERVATION_TYPE_RECOMMENDATION: &str = "RECOMMENDATION";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::OBSERVATION_TYPE_VALUES,
    OBSERVATION_TYPE_FINDING_DETAIL
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::OBSERVATION_TYPE_VALUES,
    OBSERVATION_TYPE_RECOMMENDATION
));

/// `AnalysisType`: the analysis carries a `ThreatAssessment`
/// (`smithy_query(query="doc",shape="AnalysisRecord",member="AnalysisType")`).
pub const ANALYSIS_TYPE_THREAT_ASSESSMENT: &str = "THREAT_ASSESSMENT";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ANALYSIS_TYPE_VALUES,
    ANALYSIS_TYPE_THREAT_ASSESSMENT
));

/// `ThreatConfidence` members, in the order [`CONFIDENCES_ASCENDING`] grades the
/// pass's own `Confidence`: the two scales are the same grading under two names
/// (`smithy_query(shape="ThreatConfidence")` is `LOW | MEDIUM | HIGH | VERY_HIGH`,
/// `smithy_query(shape="Confidence")` is `Unknown | Low | Medium | High`), so the
/// assessment's confidence is reported here as the member that grades alike and is
/// ABSENT for [`CONFIDENCE_UNKNOWN`], which `ThreatConfidence` has no member for.
fn threat_confidence(confidence: &str) -> Option<String> {
    let member = match confidence {
        CONFIDENCE_LOW => "LOW",
        CONFIDENCE_MEDIUM => "MEDIUM",
        CONFIDENCE_HIGH => "HIGH",
        _ => return None,
    };
    debug_assert!(enum_carries(
        crate::ops::types::shared_types::THREAT_CONFIDENCE_VALUES,
        member
    ));
    Some(member.to_string())
}

/// The modelled maximum of `FetchItemIds`
/// (`smithy_query(shape="FetchItemIds")`): the identifiers one fetch result lists,
/// beside a `TotalCount` that "can be larger".
const FETCH_ITEM_IDS_MAX: usize = 50;

/// The maximum number of rows a fetch asks for: `FetchRowLimit` is `range 1..1000`
/// (`smithy_query(shape="FetchQuery")`), and the pass asks for the whole of what the
/// subject produced, so it asks for that maximum.
const FETCH_ROW_LIMIT: i32 = 1000;

/// How many of the findings a pass read it notes one by one.
///
/// The pass reads its evidence worst finding first ([`by_severity`]), so these are the
/// worst findings of the investigation. The bound is the one the fetch record's own
/// `ItemIds` carries ([`FETCH_ITEM_IDS_MAX`], the modelled maximum of that list), which keeps the two halves of one step consistent: the step reports at most
/// that many identifiers, and at most that many findings are noted individually. It is
/// also what bounds the whole derivation below — a fixed number of steps plus one per
/// noted finding — whatever the account holds.
pub const OBSERVED_FINDINGS_MAX: usize = FETCH_ITEM_IDS_MAX;

/// The records of one investigation, oldest first — the order the operation reports
/// them in ("in the order that GuardDuty created them, oldest first").
///
/// EMPTY while the investigation's pass has not ended: there is no reasoning to report
/// for work that has not run. A record's `RecordId` is derived from the investigation's
/// own id and the step's position, so it names the same step on every read.
pub fn investigation_record_steps(
    investigation: &InvestigationRecord,
) -> Vec<wire::InvestigationRecord> {
    let (Some(analysis), Some(scope), Some(ended_at)) = (
        investigation.analysis.as_ref(),
        investigation.scope.as_ref(),
        investigation.ended_at_millis,
    ) else {
        return Vec::new();
    };
    let evidence = by_severity(&analysis.evidence);
    let mut steps: Vec<Step> = Vec::new();

    // The request that started the investigation: the milestone the pass began at,
    // which is the instant the create minted it. "UPDATE records never include
    // supporting data."
    steps.push(Step {
        r#type: RECORD_TYPE_UPDATE,
        title: "Investigation requested".to_string(),
        narrative: Some(format!(
            "Account {account} asked GuardDuty to investigate \"{prompt}\".",
            account = investigation.triggered_by,
            prompt = investigation.trigger_prompt,
        )),
        created_at_millis: investigation.started_at_millis,
        supporting_data: None,
    });

    // What the pass retrieved: the findings of the subject it resolved, as the
    // request it made and the identifiers it got back.
    steps.push(Step {
        r#type: RECORD_TYPE_FETCH,
        title: "Retrieved the findings for the investigation".to_string(),
        narrative: Some(format!(
            "GuardDuty read {findings} for {subject}.",
            findings = counted(evidence.len(), "finding", "findings"),
            subject = subject_sentence(scope),
        )),
        created_at_millis: ended_at,
        supporting_data: Some(wire::SupportingData::Fetch(wire::FetchRecord {
            fetch_type: FETCH_TYPE_FINDINGS.to_string(),
            query: Some(wire::FetchQuery {
                criteria: None,
                // `FetchTimeWindow$DurationInMinutes` is `range 1..120`, which no
                // window an account-wide pass queries fits in, so the request's own
                // window is reported by the investigation's `Scope` instead of being
                // clipped to something the pass did not ask for.
                time_window: None,
                row_limit: Some(FETCH_ROW_LIMIT),
                account_id: Some(investigation.triggered_by.clone()),
                // The threat indicator looked up and its type, and the finding type
                // whose documentation was retrieved: members of the fetch kinds this
                // pass does not make.
                indicator: None,
                indicator_type: None,
                finding_type: None,
                finding_id: scope.finding_id.clone(),
            }),
            result: Some(wire::FetchResult {
                item_ids: Some(
                    evidence
                        .iter()
                        .take(FETCH_ITEM_IDS_MAX)
                        .map(|finding| finding.id.clone())
                        .collect(),
                ),
                total_count: Some(evidence.len() as i64),
                // The detector configuration, the reputation verdict and the
                // finding-type documentation belong to the other fetch kinds.
                features: None,
                verdict: None,
                documentation: None,
            }),
        })),
    });

    // One observation per finding the pass read: the fact it noted about the
    // environment, in the order it read them, up to [`OBSERVED_FINDINGS_MAX`].
    for finding in evidence.iter().take(OBSERVED_FINDINGS_MAX) {
        steps.push(Step {
            r#type: RECORD_TYPE_OBSERVATION,
            title: format!("Noted the {} finding", finding.r#type),
            narrative: Some(format!(
                "{} of severity {} affecting {}.",
                finding.r#type,
                format_number(finding.severity),
                finding.resource_type,
            )),
            created_at_millis: ended_at,
            supporting_data: Some(wire::SupportingData::Observation(wire::ObservationRecord {
                observation_type: OBSERVATION_TYPE_FINDING_DETAIL.to_string(),
                finding_detail: Some(wire::FindingDetail {
                    finding_id: Some(finding.id.clone()),
                    finding_type: Some(finding.r#type.clone()),
                    severity: Some(format_number(finding.severity)),
                    account_id: Some(investigation.triggered_by.clone()),
                    region: Some(investigation.region.clone()),
                    activity_window: Some(wire::ActivityWindow {
                        first_seen_at: Some(::aws_smithy_types::DateTime::from_millis(
                            finding.created_at_millis,
                        )),
                        last_seen_at: Some(::aws_smithy_types::DateTime::from_millis(
                            finding.created_at_millis,
                        )),
                    }),
                }),
                ..observation_payload()
            })),
        });
    }

    // The assessment the pass derived from them.
    steps.push(Step {
        r#type: RECORD_TYPE_ANALYSIS,
        title: format!("Assessed the activity as {} risk", analysis.risk_level),
        narrative: Some(analysis.risk.clone()),
        created_at_millis: ended_at,
        supporting_data: Some(wire::SupportingData::Analysis(wire::AnalysisRecord {
            analysis_type: ANALYSIS_TYPE_THREAT_ASSESSMENT.to_string(),
            threat_assessment: Some(wire::InvestigationThreatAssessment {
                threat_confidence: threat_confidence(&analysis.confidence),
                assessment: Some(analysis.risk.clone()),
                // The reputation of an indicator, the classification of the threat and
                // the four context members below are facts this service's findings do
                // not carry.
                reputation: None,
                classification: None,
                authentication_context: None,
                rds_control_plane_activity: None,
                ip_account_activity: None,
                credential_compromise: None,
            }),
            kubernetes_policy_violation: None,
            s3_analysis: None,
            processes: None,
            account_analysis: None,
            indicators: None,
        })),
    });

    // What it recommends be done about it — the one catalogue every view of the
    // pass's recommendations reads ([`recommendations`]).
    steps.push(Step {
        r#type: RECORD_TYPE_OBSERVATION,
        title: "Recommended what to do about the activity".to_string(),
        narrative: None,
        created_at_millis: ended_at,
        supporting_data: Some(wire::SupportingData::Observation(wire::ObservationRecord {
            observation_type: OBSERVATION_TYPE_RECOMMENDATION.to_string(),
            recommendations: Some(recommendations(analysis)),
            ..observation_payload()
        })),
    });

    // GuardDuty's answer: the milestone the pass ended at.
    steps.push(Step {
        r#type: RECORD_TYPE_UPDATE,
        title: "Investigation completed".to_string(),
        narrative: Some(format!(
            "{title} GuardDuty assessed the activity as {level} risk with {confidence} confidence.",
            title = analysis.title,
            level = analysis.risk_level,
            confidence = analysis.confidence,
        )),
        created_at_millis: ended_at,
        supporting_data: None,
    });

    // How long it took: the one operational detail the pass has.
    steps.push(Step {
        r#type: RECORD_TYPE_METADATA,
        title: "Investigation timing".to_string(),
        narrative: None,
        created_at_millis: ended_at,
        supporting_data: Some(wire::SupportingData::Metadata(wire::MetadataRecord {
            total_time_milliseconds: Some(
                ended_at.saturating_sub(investigation.started_at_millis).max(0),
            ),
        })),
    });

    steps
        .into_iter()
        .enumerate()
        .map(|(position, step)| step.into_record(&investigation.id, position))
        .collect()
}

/// An `ObservationRecord` with no payload member set, which each observation above
/// fills the ONE member its `ObservationType` names: "the record contains at most one
/// of the payload members, the one that matches `ObservationType`"
/// (`smithy_query(query="doc",shape="ObservationRecord")`).
fn observation_payload() -> wire::ObservationRecord {
    wire::ObservationRecord::default()
}

/// What the pass retrieved findings FOR, as the fetch record's narrative names it:
/// the subject the scope resolved.
fn subject_sentence(scope: &InvestigationScopeRecord) -> String {
    match scope.investigation_type.as_str() {
        INVESTIGATION_TYPE_FINDING => format!(
            "the finding {}",
            scope.finding_id.as_deref().unwrap_or_default()
        ),
        INVESTIGATION_TYPE_ORGANIZATION => format!(
            "the organization administered by account {}",
            scope.admin_account_id.as_deref().unwrap_or_default()
        ),
        _ => format!(
            "account {}",
            scope.account_id.as_deref().unwrap_or_default()
        ),
    }
}

/// One step of the pass, before it is given its position and its identifier.
struct Step {
    r#type: &'static str,
    title: String,
    narrative: Option<String>,
    created_at_millis: i64,
    supporting_data: Option<wire::SupportingData>,
}

impl Step {
    /// The modelled `InvestigationRecord` for this step of the investigation
    /// `investigation_id` names, at `position` in its reasoning.
    ///
    /// The identifier is the investigation's own id and that position, so it names
    /// THIS step of THIS investigation and names it again on every later read.
    fn into_record(self, investigation_id: &str, position: usize) -> wire::InvestigationRecord {
        wire::InvestigationRecord {
            record_id: record_id(investigation_id, position),
            r#type: self.r#type.to_string(),
            title: clamped(self.title, RECORD_TITLE_MAX),
            narrative: self
                .narrative
                .map(|narrative| clamped(narrative, NARRATIVE_MAX)),
            created_at: ::aws_smithy_types::DateTime::from_millis(self.created_at_millis),
            supporting_data: self.supporting_data,
        }
    }
}

/// The modelled maximum of `RecordTitle` (`len 0..1024`,
/// `smithy_query(shape="InvestigationRecord")`).
const RECORD_TITLE_MAX: usize = 1024;

/// The modelled maximum of `Narrative` (`len 0..4096`,
/// `smithy_query(shape="InvestigationRecord")`).
const NARRATIVE_MAX: usize = 4096;

/// The modelled maximum of `RecordId` (`len 1..64`,
/// `smithy_query(shape="RecordId")`).
const RECORD_ID_MAX: usize = 64;

/// How a record is IDENTIFIED: the investigation's id and the step's position in its
/// reasoning, which is what makes the identifier stable across reads and unique within
/// the investigation. Clamped to the modelled length of `RecordId`, taking the
/// position (which is what distinguishes two records) rather than the prefix.
fn record_id(investigation_id: &str, position: usize) -> String {
    let suffix = format!("-{position:04}");
    let room = RECORD_ID_MAX.saturating_sub(suffix.len());
    let mut id: String = investigation_id.chars().take(room).collect();
    id.push_str(&suffix);
    id
}
