// SPDX-License-Identifier: CC0-1.0

//! The analysis pass one investigation runs, and the assessment it leaves behind.
//!
//! # What an investigation reports
//!
//! `Investigation.Summary` is "a structured summary of the investigation findings,
//! including affected resources, threat assessment, and recommended remediation
//! steps", `Risk` "a human-readable description of the assessed risk", `RiskLevel`
//! "the assessed risk level of the investigated threat" and `Confidence` "the
//! confidence level of the investigation's assessment"
//! (`smithy_query(shape="Investigation")`). So all four are CONCLUSIONS about the
//! activity GuardDuty observed in the account the investigation covers — which, in
//! this service, is the set of findings the detector holds.
//!
//! The pass therefore READS that evidence and derives the assessment from it: the
//! findings the detector held when the pass ended are gathered
//! ([`FindingEvidence`]), the risk level is the band the highest severity among them
//! sits in, the confidence follows how many of them corroborate that band, and the
//! summary and the risk sentence name the resources, the finding types and the
//! severities actually found. Nothing is drawn from the request beyond the prompt — which
//! selects which findings to look at and is the request the summary reports on — and an
//! account with no findings is assessed as exactly that rather than given an invented
//! narrative.
//!
//! The assessment is computed ONCE, when the clock ends the pass
//! ([`super::InvestigationRecord::settle`]), and stored on the investigation, so
//! every later read reports the same conclusions whatever the store holds by then.

use std::collections::BTreeSet;

use super::*;

/// `Product$Name` — "the name of the product" that produced an investigation
/// (`smithy_query(shape="Product")`). GuardDuty's investigations are produced by its
/// AI Analyst, and the name is the product's, so it carries nothing of the request.
pub const INVESTIGATION_PRODUCT_NAME: &str = "Amazon GuardDuty AI Analyst";

/// `Product$Feature` — "the specific feature within the product that produced the
/// investigation" (`smithy_query(shape="Product")`): the Investigation API itself.
pub const INVESTIGATION_PRODUCT_FEATURE: &str = "Investigation";

/// `InvestigationMetadata$Version` — "the version of the investigation engine that
/// produced the results" (`smithy_query(shape="InvestigationMetadata")`): the version
/// of the pass this module implements, which every investigation it produces carries.
pub const INVESTIGATION_ENGINE_VERSION: &str = "1.0.0";

/// `RiskLevel`: no assessable risk — the level an investigation reports when the
/// evidence it analysed carries no severity on GuardDuty's scale, including an
/// account with no findings at all. The enum is `Info | Low | Medium | High |
/// Critical` (`smithy_query(shape="RiskLevel")`).
pub const RISK_LEVEL_INFO: &str = "Info";

/// `RiskLevel` for evidence in GuardDuty's Low severity band. See
/// [`RISK_LEVEL_INFO`] for the enum.
pub const RISK_LEVEL_LOW: &str = "Low";

/// `RiskLevel` for evidence in GuardDuty's Medium severity band.
pub const RISK_LEVEL_MEDIUM: &str = "Medium";

/// `RiskLevel` for evidence in GuardDuty's High severity band.
pub const RISK_LEVEL_HIGH: &str = "High";

/// `RiskLevel` for evidence in GuardDuty's Critical severity band.
pub const RISK_LEVEL_CRITICAL: &str = "Critical";

// Every level above is a member of the modelled enum, read from the generated list
// so the two can never drift.
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RISK_LEVEL_VALUES,
    RISK_LEVEL_INFO
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RISK_LEVEL_VALUES,
    RISK_LEVEL_LOW
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RISK_LEVEL_VALUES,
    RISK_LEVEL_MEDIUM
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RISK_LEVEL_VALUES,
    RISK_LEVEL_HIGH
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RISK_LEVEL_VALUES,
    RISK_LEVEL_CRITICAL
));

/// `Confidence`: the assessment rests on no evidence at all, so there is nothing to
/// be confident about. The enum is `Unknown | Low | Medium | High`
/// (`smithy_query(shape="Confidence")`).
pub const CONFIDENCE_UNKNOWN: &str = "Unknown";

/// `Confidence` for an assessment a single finding supports. See
/// [`CONFIDENCE_UNKNOWN`] for the enum.
pub const CONFIDENCE_LOW: &str = "Low";

/// `Confidence` for an assessment two findings support.
pub const CONFIDENCE_MEDIUM: &str = "Medium";

/// `Confidence` for an assessment [`CORROBORATION_FOR_HIGH_CONFIDENCE`] or more
/// findings support.
pub const CONFIDENCE_HIGH: &str = "High";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::CONFIDENCE_VALUES,
    CONFIDENCE_UNKNOWN
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::CONFIDENCE_VALUES,
    CONFIDENCE_LOW
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::CONFIDENCE_VALUES,
    CONFIDENCE_MEDIUM
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::CONFIDENCE_VALUES,
    CONFIDENCE_HIGH
));

/// How many findings in the assessed risk band make the assessment a
/// [`CONFIDENCE_HIGH`] one.
///
/// GuardDuty documents no rule for `Confidence`, so the reading is CHOSEN and is the
/// plainest one the member admits: confidence in an assessment grows with the number
/// of findings that corroborate it, from none ([`CONFIDENCE_UNKNOWN`]) through one
/// and two to this many or more.
pub const CORROBORATION_FOR_HIGH_CONFIDENCE: usize = 3;

/// How many finding types the summary names before it counts the rest, so one
/// account's assessment cannot grow without bound as its findings do.
const SUMMARIZED_FINDING_TYPES: usize = 5;

/// The risk level evidence of `severity` sits in: GuardDuty's own severity bands —
/// "Low 1.0–3.9, Medium 4.0–6.9, High 7.0–8.9, Critical 9.0–10.0" (GuardDuty
/// "Severity levels for GuardDuty findings") — whose four names are also the
/// `RiskLevel` members, with [`RISK_LEVEL_INFO`] below the scale's own minimum
/// ([`FINDING_SEVERITY_MIN`]).
pub fn risk_level_of_severity(severity: f64) -> &'static str {
    match severity {
        severity if severity >= 9.0 => RISK_LEVEL_CRITICAL,
        severity if severity >= 7.0 => RISK_LEVEL_HIGH,
        severity if severity >= 4.0 => RISK_LEVEL_MEDIUM,
        severity if severity >= FINDING_SEVERITY_MIN => RISK_LEVEL_LOW,
        _ => RISK_LEVEL_INFO,
    }
}

/// The sort key an investigation that has reached no conclusion resolves to: below
/// every graded member, since the grade of an assessment nothing has made yet is not
/// a grade at all. See [`risk_level_rank`].
pub const UNRANKED: f64 = -1.0;

/// The `RiskLevel` members in ASCENDING order of risk, which is the order a sort on
/// the member means. The enum itself carries no order
/// (`smithy_query(shape="RiskLevel")`), but its members are GuardDuty's severity
/// bands, which are ordered.
pub const RISK_LEVELS_ASCENDING: [&str; 5] = [
    RISK_LEVEL_INFO,
    RISK_LEVEL_LOW,
    RISK_LEVEL_MEDIUM,
    RISK_LEVEL_HIGH,
    RISK_LEVEL_CRITICAL,
];

/// The `Confidence` members in ASCENDING order of confidence, for the same reason
/// [`RISK_LEVELS_ASCENDING`] is ordered.
pub const CONFIDENCES_ASCENDING: [&str; 4] = [
    CONFIDENCE_UNKNOWN,
    CONFIDENCE_LOW,
    CONFIDENCE_MEDIUM,
    CONFIDENCE_HIGH,
];

/// Where `level` sits on [`RISK_LEVELS_ASCENDING`], or [`UNRANKED`] for a value that
/// is no member of the enum.
pub fn risk_level_rank(level: &str) -> f64 {
    rank_in(&RISK_LEVELS_ASCENDING, level)
}

/// Where `confidence` sits on [`CONFIDENCES_ASCENDING`], or [`UNRANKED`] for a value
/// that is no member of the enum.
pub fn confidence_rank(confidence: &str) -> f64 {
    rank_in(&CONFIDENCES_ASCENDING, confidence)
}

/// The position of `value` in `scale`, as the numeric key a sort compares.
fn rank_in(scale: &[&str], value: &str) -> f64 {
    scale
        .iter()
        .position(|member| *member == value)
        .map_or(UNRANKED, |index| index as f64)
}

/// One finding as the analysis pass reads it: the facts about it the assessment is
/// derived from, copied off the stored [`FindingRecord`] so the pass can be computed
/// while the investigation it belongs to is being written.
#[derive(Debug, Clone, PartialEq)]
pub struct FindingEvidence {
    /// The finding's own id, which is what a prompt names a finding by.
    pub id: String,
    /// `Finding$Type`, the threat the finding reports.
    pub r#type: String,
    /// `Finding$Severity`, on GuardDuty's 1.0–10.0 scale.
    pub severity: f64,
    /// `Resource$ResourceType`, the kind of resource the activity affected.
    pub resource_type: String,
    /// When the finding was produced, so a pass covers the findings that existed
    /// when it ended rather than whatever the store holds when somebody reads it.
    pub created_at_millis: i64,
}

impl FindingEvidence {
    /// Read one stored finding as evidence.
    pub fn of(finding: &FindingRecord) -> Self {
        Self {
            id: finding.id.clone(),
            r#type: finding.r#type.clone(),
            severity: finding.severity,
            resource_type: finding.resource_type.clone(),
            created_at_millis: finding.created_at_millis,
        }
    }
}

/// The conclusions one ended analysis pass reached: the four assessment members of
/// `Investigation` plus the `InvestigationSummary$Title` a listing reports, all
/// derived from the evidence the pass read and stored on the investigation — together
/// with that evidence itself, so every later projection of the pass (the structured
/// summary `SummaryFormat=STRUCTURED` asks for, and the records of the reasoning
/// `ListInvestigationRecords` reports) answers about the findings THIS pass read
/// rather than about whatever the store holds when somebody asks.
#[derive(Debug, Clone, PartialEq)]
pub struct InvestigationAnalysis {
    /// `Investigation$RiskLevel`.
    pub risk_level: String,
    /// `Investigation$Risk`, within the modelled `len 0..1024`.
    pub risk: String,
    /// `Investigation$Confidence`.
    pub confidence: String,
    /// `Investigation$Summary`.
    pub summary: String,
    /// `InvestigationSummary$Title`, within the modelled `len 0..1024`.
    pub title: String,
    /// The findings the pass read, in the order it read them ([`by_severity`]).
    pub evidence: Vec<FindingEvidence>,
}

/// The modelled maximum of `RiskDetails`, which `Investigation$Risk` targets
/// (`len 0..1024`, `smithy_query(shape="RiskDetails")`).
const RISK_MAX: usize = 1024;

/// The modelled maximum of `InvestigationTitle`, which `InvestigationSummary$Title`
/// targets (`len 0..1024`, `smithy_query(shape="InvestigationSummary")`).
const TITLE_MAX: usize = 1024;

impl InvestigationAnalysis {
    /// Run the pass: assess `evidence` — the findings this investigation covered — as
    /// the answer to the request `prompt` asked, for the account and Region the
    /// investigation was initiated for.
    ///
    /// The evidence is taken in SEVERITY order, highest first, with the finding id
    /// breaking ties, so the assessment is a function of the findings alone and not
    /// of the order a map happened to yield them in.
    pub fn of_evidence(
        evidence: &[FindingEvidence],
        prompt: &str,
        account: &str,
        region: &str,
    ) -> Self {
        let ordered = by_severity(evidence);

        let highest = ordered.first().map(|finding| finding.severity);
        let risk_level = highest.map_or(RISK_LEVEL_INFO, risk_level_of_severity);
        // How much of the evidence sits in the band the assessment reports, which is
        // what the confidence grades: zero only for a pass that read no findings at
        // all, since the worst finding is always in its own band.
        let corroborating = ordered
            .iter()
            .filter(|finding| risk_level_of_severity(finding.severity) == risk_level)
            .count();
        let confidence = match corroborating {
            0 => CONFIDENCE_UNKNOWN,
            1 => CONFIDENCE_LOW,
            count if count < CORROBORATION_FOR_HIGH_CONFIDENCE => CONFIDENCE_MEDIUM,
            _ => CONFIDENCE_HIGH,
        };

        Self {
            risk_level: risk_level.to_string(),
            risk: clamped(risk_sentence(&ordered, risk_level), RISK_MAX),
            confidence: confidence.to_string(),
            summary: summary_text(&ordered, prompt, risk_level, account, region),
            title: clamped(title_text(&ordered, risk_level), TITLE_MAX),
            evidence: ordered.iter().map(|finding| (*finding).clone()).collect(),
        }
    }

    /// The `InvestigationMetadata` an investigation this pass produced reports: the
    /// product, its feature and the engine version above, which identify the producer
    /// and carry nothing of any one investigation.
    pub fn metadata() -> crate::ops::types::InvestigationMetadata {
        crate::ops::types::InvestigationMetadata {
            version: INVESTIGATION_ENGINE_VERSION.to_string(),
            product: crate::ops::types::Product {
                name: INVESTIGATION_PRODUCT_NAME.to_string(),
                feature: Some(INVESTIGATION_PRODUCT_FEATURE.to_string()),
            },
        }
    }
}

/// The findings `prompt` NAMES out of `evidence`: the ones whose id appears in it,
/// in the order the pass takes them (worst first).
///
/// `TriggerPrompt` is "the natural-language prompt that initiated this investigation"
/// (`smithy_query(shape="Investigation")`), and the documented prompts range from a
/// whole-organization request to one naming a single finding
/// (`smithy_query(query="doc",shape="CreateInvestigation")`), so a prompt that names
/// findings the account holds is a request to investigate THOSE findings. Nothing else
/// about the prompt enters the assessment: a prompt that names no stored finding is a
/// request to assess the subject's activity as a whole, which is what the caller
/// ([`super::InvestigationRecord::settle`]) falls back to.
pub fn named_evidence(prompt: &str, evidence: &[FindingEvidence]) -> Vec<FindingEvidence> {
    by_severity(evidence)
        .into_iter()
        .filter(|finding| prompt_names(prompt, &finding.id))
        .cloned()
        .collect()
}

/// The worst finding among `evidence` — the one the pass assesses from — or `None`
/// for no evidence at all. The order is [`by_severity`]'s, so the worst finding is
/// the same one the assessment and every projection of it name.
pub fn worst_of(evidence: &[FindingEvidence]) -> Option<&FindingEvidence> {
    by_severity(evidence).into_iter().next()
}

/// `evidence` in the order a pass reads it: highest severity first, with the finding
/// id breaking ties, so every conclusion drawn from it is a function of the findings
/// alone and not of the order a map happened to yield them in.
pub fn by_severity(evidence: &[FindingEvidence]) -> Vec<&FindingEvidence> {
    let mut ordered: Vec<&FindingEvidence> = evidence.iter().collect();
    ordered.sort_by(|a, b| {
        b.severity
            .partial_cmp(&a.severity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    ordered
}

/// Does `prompt` name `id`? The id is a whole WORD of the prompt — a run of
/// alphanumerics — so a prompt mentioning a longer identifier that merely starts
/// with the id does not select it.
fn prompt_names(prompt: &str, id: &str) -> bool {
    prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word == id)
}

/// `Investigation$Risk`: one sentence about the evidence — how much of it there is,
/// what it affects and the severity the assessed level comes from.
fn risk_sentence(evidence: &[&FindingEvidence], risk_level: &str) -> String {
    let Some(highest) = evidence.first() else {
        return format!(
            "No GuardDuty findings had been produced for this account and Region when the \
             investigation ran, so no threat activity could be assessed and the risk is \
             {risk_level}."
        );
    };
    format!(
        "{risk_level} risk: {findings} over {types} and {resources}. The highest severity \
         analysed is {severity} ({risk_level}), reported by {top}.",
        findings = counted(evidence.len(), "finding", "findings"),
        types = counted(distinct(evidence.iter().map(|f| f.r#type.as_str())).len(), "finding type", "finding types"),
        resources = counted(
            distinct(evidence.iter().map(|f| f.resource_type.as_str())).len(),
            "affected resource type",
            "affected resource types"
        ),
        severity = format_number(highest.severity),
        top = highest.r#type,
    )
}

/// `Investigation$Summary`: "a structured summary of the investigation findings,
/// including affected resources, threat assessment, and recommended remediation steps"
/// (`smithy_query(shape="Investigation")`).
///
/// The report the pass produced, with a HEADED SECTION per part the documentation names —
/// the REQUEST it answers and the environment and evidence the pass covered, the resources
/// the analysed findings affect, the assessment, and the remediation steps the assessed
/// level calls for — so a reader can show it as it stands and a consumer can find a part
/// by its heading.
///
/// It opens with the `TriggerPrompt` this investigation was initiated with, which is what
/// makes the report THIS investigation's rather than any other's: two investigations of
/// different requests over the same account do not read alike. Every count, resource type,
/// severity and finding type in it is read off the findings the pass analysed, and nothing
/// in it is narrated beyond them.
fn summary_text(
    evidence: &[&FindingEvidence],
    prompt: &str,
    risk_level: &str,
    account: &str,
    region: &str,
) -> String {
    let mut report = format!(
        "## Investigation summary\nGuardDuty investigated the request \"{prompt}\" and analysed \
         {findings} in account {account} ({region}), assessing the activity as {risk_level} \
         risk.\n\n## Affected resources\n{resources}\n\n## Threat assessment\n{assessment}\n\n## \
         Recommended remediation\n",
        findings = counted(evidence.len(), "finding", "findings"),
        resources = affected_resources(evidence),
        assessment = threat_assessment(evidence, risk_level),
    );
    for (step, remediation) in remediation(evidence.is_empty(), risk_level).iter().enumerate() {
        report.push_str(&format!("{}. {}\n", step + 1, remediation.action));
    }
    report
}

/// The summary's affected-resources section: one line per `Resource$ResourceType` the
/// analysed findings report, with how many of them affect it, worst finding first.
fn affected_resources(evidence: &[&FindingEvidence]) -> String {
    let lines = affected_resource_lines(evidence);
    if lines.is_empty() {
        return "None: the pass analysed no findings, so no resource was implicated.".to_string();
    }
    lines
        .into_iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<String>>()
        .join("\n")
}

/// One line per `Resource$ResourceType` the analysed findings report, with how many of
/// them affect it, worst finding first — the affected resources as a LIST, which the
/// text report renders as its own section and the structured summary carries as the
/// pass's key observations.
pub fn affected_resource_lines(evidence: &[&FindingEvidence]) -> Vec<String> {
    resource_type_counts(evidence)
        .into_iter()
        .map(|(resource_type, findings)| {
            format!(
                "{resource_type}: {}",
                counted(findings, "finding", "findings")
            )
        })
        .collect()
}

/// Each `Resource$ResourceType` the analysed findings report and how many of them
/// affect it, worst finding first — the one count both the text report and the
/// structured summary's affected-resource list read.
pub fn resource_type_counts(evidence: &[&FindingEvidence]) -> Vec<(String, usize)> {
    let counts = counts_of(evidence.iter().map(|finding| finding.resource_type.as_str()));
    distinct(evidence.iter().map(|finding| finding.resource_type.as_str()))
        .into_iter()
        .map(|resource_type| {
            let findings = counts.get(resource_type.as_str()).copied().unwrap_or(0);
            (resource_type, findings)
        })
        .collect()
}

/// Each `Finding$Type` the analysed findings report and how many of them carry it,
/// most findings first with the type breaking ties — what `TopFindingTypes` reports.
pub fn finding_type_counts(evidence: &[&FindingEvidence]) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = counts_of(evidence.iter().map(|f| f.r#type.as_str()))
        .into_iter()
        .map(|(r#type, count)| (r#type.to_string(), count))
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

/// The summary's threat-assessment section: the severity the assessed level comes from
/// and the finding types it came from, highest severity first, with a count of the types
/// past [`SUMMARIZED_FINDING_TYPES`] so one account's assessment cannot grow without
/// bound as its findings do.
fn threat_assessment(evidence: &[&FindingEvidence], risk_level: &str) -> String {
    let Some(highest) = evidence.first() else {
        return format!(
            "No GuardDuty findings had been produced for this account and Region when the \
             pass ran, so there was no threat activity to assess and the risk is {risk_level}."
        );
    };
    // Counted in ONE pass, then listed in the order the evidence is already in (highest
    // severity first), so the assessment is linear in the findings the pass read however
    // many types they carry.
    let counts = counts_of(evidence.iter().map(|finding| finding.r#type.as_str()));
    let mut listed: BTreeSet<&str> = BTreeSet::new();
    let mut named: Vec<String> = Vec::new();
    for finding in evidence {
        if !listed.insert(finding.r#type.as_str()) || named.len() >= SUMMARIZED_FINDING_TYPES {
            continue;
        }
        named.push(format!(
            "{} (severity {}, {})",
            finding.r#type,
            format_number(finding.severity),
            counted(
                counts.get(finding.r#type.as_str()).copied().unwrap_or(0),
                "finding",
                "findings"
            )
        ));
    }
    let rest = counts.len().saturating_sub(named.len());
    if rest > 0 {
        named.push(format!(
            "and {}",
            counted(rest, "further finding type", "further finding types")
        ));
    }
    format!(
        "{risk_level} risk, assessed from the highest severity analysed ({severity}, \
         {risk_level}), reported by {top}. The pass analysed {findings} over {types}:\n{lines}",
        severity = format_number(highest.severity),
        top = highest.r#type,
        findings = counted(evidence.len(), "finding", "findings"),
        types = counted(counts.len(), "finding type", "finding types"),
        lines = named
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<String>>()
            .join("\n"),
    )
}

/// How many times each value of `values` occurs, counted in one pass.
pub fn counts_of<'a>(values: impl Iterator<Item = &'a str>) -> BTreeMap<&'a str, usize> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
}

/// One step the pass recommends: what to do, why, and how the modelled members
/// classify it.
///
/// One catalogue for every view of the pass's recommendations — the numbered steps the
/// text report prints, the `Countermeasure` list a FINDING investigation's structured
/// summary carries, and the `Recommendation` list an account-wide one carries — so no
/// two of them can recommend different things for one assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemediationStep {
    /// What to do, as the report's numbered step and the action's own title.
    pub action: &'static str,
    /// Why the assessment calls for it.
    pub rationale: &'static str,
    /// `DefenseTactic`, the kind of defence the step is
    /// (`smithy_query(shape="DefenseTactic")`).
    pub defense_tactic: &'static str,
    /// `ActionPriority`, how soon it is called for
    /// (`smithy_query(shape="ActionPriority")`).
    pub priority: &'static str,
    /// `RecommendationCategory`, the kind of recommendation it is
    /// (`smithy_query(shape="RecommendationCategory")`).
    pub category: &'static str,
}

/// `DefenseTactic`: cut the affected resource off from the activity.
pub const DEFENSE_TACTIC_ISOLATE: &str = "Isolate";

/// `DefenseTactic`: take the actor's access away. See [`DEFENSE_TACTIC_ISOLATE`].
pub const DEFENSE_TACTIC_EVICT: &str = "Evict";

/// `DefenseTactic`: look for the activity rather than act on it.
pub const DEFENSE_TACTIC_DETECT: &str = "Detect";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DEFENSE_TACTIC_VALUES,
    DEFENSE_TACTIC_ISOLATE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DEFENSE_TACTIC_VALUES,
    DEFENSE_TACTIC_EVICT
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DEFENSE_TACTIC_VALUES,
    DEFENSE_TACTIC_DETECT
));

/// `ActionPriority`: the step is called for now.
pub const ACTION_PRIORITY_IMMEDIATE: &str = "IMMEDIATE";

/// `ActionPriority`: the step is called for soon. See [`ACTION_PRIORITY_IMMEDIATE`].
pub const ACTION_PRIORITY_SHORT_TERM: &str = "SHORT_TERM";

/// `ActionPriority`: the step is a standing practice rather than a response.
pub const ACTION_PRIORITY_STRATEGIC: &str = "STRATEGIC";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ACTION_PRIORITY_VALUES,
    ACTION_PRIORITY_IMMEDIATE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ACTION_PRIORITY_VALUES,
    ACTION_PRIORITY_SHORT_TERM
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ACTION_PRIORITY_VALUES,
    ACTION_PRIORITY_STRATEGIC
));

/// `RecommendationCategory`: make the environment harder to act against.
pub const RECOMMENDATION_HARDENING: &str = "HARDENING";

/// `RecommendationCategory`: look into the activity further. See
/// [`RECOMMENDATION_HARDENING`].
pub const RECOMMENDATION_FURTHER_INVESTIGATION: &str = "FURTHER_INVESTIGATION";

/// `RecommendationCategory`: suppress what has been confirmed as expected.
pub const RECOMMENDATION_SUPPRESSION_RULE: &str = "SUPPRESSION_RULE";

/// `RecommendationCategory`: keep watching.
pub const RECOMMENDATION_MONITORING: &str = "MONITORING";

/// `RecommendationCategory`: turn a protection on.
pub const RECOMMENDATION_ENABLE_PROTECTION: &str = "ENABLE_PROTECTION";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECOMMENDATION_CATEGORY_VALUES,
    RECOMMENDATION_HARDENING
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECOMMENDATION_CATEGORY_VALUES,
    RECOMMENDATION_FURTHER_INVESTIGATION
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECOMMENDATION_CATEGORY_VALUES,
    RECOMMENDATION_SUPPRESSION_RULE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECOMMENDATION_CATEGORY_VALUES,
    RECOMMENDATION_MONITORING
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RECOMMENDATION_CATEGORY_VALUES,
    RECOMMENDATION_ENABLE_PROTECTION
));

/// The summary's remediation steps, which follow the assessed level: the higher the
/// risk, the more they ask of the operator, and an account with nothing to assess is
/// asked for nothing.
pub fn remediation(no_evidence: bool, risk_level: &str) -> &'static [RemediationStep] {
    if no_evidence {
        return &[RemediationStep {
            action: "Keep GuardDuty enabled so later activity is analysed.",
            rationale: "The pass read no findings, so there is nothing to respond to; coverage is \
                        what makes later activity analysable.",
            defense_tactic: DEFENSE_TACTIC_DETECT,
            priority: ACTION_PRIORITY_STRATEGIC,
            category: RECOMMENDATION_ENABLE_PROTECTION,
        }];
    }
    match risk_level {
        RISK_LEVEL_CRITICAL | RISK_LEVEL_HIGH => &[
            RemediationStep {
                action: "Isolate the affected resources.",
                rationale: "The assessed level means the activity is under way, so cutting the \
                            affected resources off limits what it can reach.",
                defense_tactic: DEFENSE_TACTIC_ISOLATE,
                priority: ACTION_PRIORITY_IMMEDIATE,
                category: RECOMMENDATION_HARDENING,
            },
            RemediationStep {
                action: "Revoke the credentials used against them.",
                rationale: "Credentials used in the analysed activity keep working until they are \
                            revoked.",
                defense_tactic: DEFENSE_TACTIC_EVICT,
                priority: ACTION_PRIORITY_IMMEDIATE,
                category: RECOMMENDATION_HARDENING,
            },
            RemediationStep {
                action: "Escalate to your incident response process.",
                rationale: "Activity at this level is handled by the process that owns incidents, \
                            not by the finding alone.",
                defense_tactic: DEFENSE_TACTIC_DETECT,
                priority: ACTION_PRIORITY_IMMEDIATE,
                category: RECOMMENDATION_FURTHER_INVESTIGATION,
            },
        ],
        RISK_LEVEL_MEDIUM => &[
            RemediationStep {
                action: "Review the activity on the affected resources.",
                rationale: "The assessed level leaves open whether the activity was expected, \
                            which only a review of the resources settles.",
                defense_tactic: DEFENSE_TACTIC_DETECT,
                priority: ACTION_PRIORITY_SHORT_TERM,
                category: RECOMMENDATION_FURTHER_INVESTIGATION,
            },
            RemediationStep {
                action: "Confirm whether it was expected before archiving the findings.",
                rationale: "Archiving activity that was not confirmed hides the next occurrence \
                            of it.",
                defense_tactic: DEFENSE_TACTIC_DETECT,
                priority: ACTION_PRIORITY_SHORT_TERM,
                category: RECOMMENDATION_SUPPRESSION_RULE,
            },
        ],
        _ => &[
            RemediationStep {
                action: "Monitor the affected resources.",
                rationale: "At this level the activity calls for watching rather than for a \
                            response.",
                defense_tactic: DEFENSE_TACTIC_DETECT,
                priority: ACTION_PRIORITY_STRATEGIC,
                category: RECOMMENDATION_MONITORING,
            },
            RemediationStep {
                action: "Archive the findings once the activity is confirmed as expected.",
                rationale: "Confirmed-expected activity that stays unarchived crowds out the \
                            findings that are not.",
                defense_tactic: DEFENSE_TACTIC_DETECT,
                priority: ACTION_PRIORITY_STRATEGIC,
                category: RECOMMENDATION_SUPPRESSION_RULE,
            },
        ],
    }
}

/// `InvestigationSummary$Title`: the short title a listing reports — the assessed
/// level and what it was assessed from.
fn title_text(evidence: &[&FindingEvidence], risk_level: &str) -> String {
    match evidence.first() {
        None => "No findings available to investigate".to_string(),
        Some(highest) => format!(
            "{risk_level} risk: {} affecting {}",
            highest.r#type, highest.resource_type
        ),
    }
}

/// The distinct values of `values`, in first-seen order — which, for evidence
/// ordered by severity, puts the resource type of the worst finding first.
pub fn distinct<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for value in values {
        if !value.is_empty() && seen.insert(value) {
            ordered.push(value.to_string());
        }
    }
    ordered
}

/// `count` with the noun that agrees with it.
pub fn counted(count: usize, singular: &str, plural: &str) -> String {
    match count {
        1 => format!("1 {singular}"),
        _ => format!("{count} {plural}"),
    }
}

/// `text` within `max` characters, cut on a character boundary so a multi-byte
/// character is never split — the guard that keeps a derived sentence inside the
/// modelled length of the member that carries it.
pub fn clamped(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    text.chars().take(max).collect()
}
