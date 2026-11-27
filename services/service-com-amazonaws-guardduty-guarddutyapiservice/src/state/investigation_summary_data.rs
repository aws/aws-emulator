// SPDX-License-Identifier: CC0-1.0

//! The STRUCTURED summary of one investigation: `Investigation$SummaryData`.
//!
//! `Summary` and `SummaryData` are two views of ONE stored conclusion. "When you set
//! `SummaryFormat` to `STRUCTURED`, the summary is returned in `SummaryData` instead"
//! (`smithy_query(query="doc",shape="Investigation",member="Summary")`), so everything
//! below is read off the assessment the pass stored ([`InvestigationAnalysis`]) and the
//! scope it resolved ([`InvestigationScopeRecord`]) — the same facts the text report
//! renders as prose — and nothing is recomputed from whatever the store holds when a
//! caller asks.
//!
//! "The fields that are present depend on the investigation type"
//! (`smithy_query(query="doc",shape="InvestigationSummaryData")`), which is what the
//! match on the scope's type below follows: a finding-scoped pass reports the finding it
//! analysed and the countermeasures for it, an account-wide one reports the analysis of
//! that account's findings, and an organization-wide one reports the analysis across the
//! organization's accounts.
//!
//! `ThreatAssessment` is NOT reported: its members are an attack progression over MITRE
//! ATT&CK tactics and techniques and a threat-actor profile, neither of which this
//! service's findings carry, and an optional member nothing is known for is absent rather
//! than invented.

use super::*;

/// How many of the analysed finding types `TopFindingTypes` reports. The modelled
/// list takes up to 50 entries (`smithy_query(shape="FindingTypeCounts")`), which is
/// the cap applied here.
const TOP_FINDING_TYPES_MAX: usize = 50;

/// The modelled maximum of `AccountAffectedResources`
/// (`smithy_query(shape="AccountAffectedResources")`).
const AFFECTED_RESOURCES_MAX: usize = 50;

/// The modelled maximum of `AccountSummaries`
/// (`smithy_query(shape="AccountSummaries")`).
const ACCOUNT_BREAKDOWN_MAX: usize = 20;

/// The modelled maximum of `InvestigationObservations`
/// (`smithy_query(shape="InvestigationObservations")`).
const OBSERVATIONS_MAX: usize = 25;

/// The modelled maximum of `Recommendations`
/// (`smithy_query(shape="Recommendations")`).
const RECOMMENDATIONS_MAX: usize = 25;

/// The modelled maximum of `Countermeasures`
/// (`smithy_query(shape="Countermeasures")`).
const COUNTERMEASURES_MAX: usize = 20;

/// The structured summary one ended pass serves: [`InvestigationAnalysis`] and the
/// scope it resolved, projected onto `InvestigationSummaryData`.
pub fn investigation_summary_data(
    analysis: &InvestigationAnalysis,
    scope: &InvestigationScopeRecord,
) -> crate::ops::types::InvestigationSummaryData {
    let evidence = by_severity(&analysis.evidence);
    let finding_scoped = scope.investigation_type == INVESTIGATION_TYPE_FINDING;
    let account_wide = !finding_scoped;

    crate::ops::types::InvestigationSummaryData {
        // "For a FINDING investigation, the type of the finding that the
        // investigation analysed": the type of the very finding the scope names, so
        // the two members cannot disagree about which finding was investigated.
        finding_type: scope.finding_id.as_deref().and_then(|id| {
            evidence
                .iter()
                .find(|finding| finding.id == id)
                .map(|finding| finding.r#type.clone())
        }),
        threat_assessment: None,
        countermeasures: finding_scoped.then(|| countermeasures(analysis)),
        account_analysis: (scope.investigation_type == INVESTIGATION_TYPE_ACCOUNT).then(|| {
            crate::ops::types::AccountAnalysis {
                findings_summary: Some(crate::ops::types::FindingsSummary {
                    total_findings: Some(count(evidence.len())),
                    severity_breakdown: Some(severity_breakdown(&evidence)),
                    top_finding_types: Some(top_finding_types(&evidence)),
                    // "The number of findings in the time window": the pass groups
                    // its evidence by no time window, so nothing is reported for
                    // one rather than a single group invented around the whole span.
                    by_time_window: None,
                }),
                // The MITRE ATT&CK tactics the findings are associated with, which
                // this service's findings do not carry.
                tactic_coverage: None,
                affected_resources: Some(affected_resources(&evidence)),
            }
        }),
        organization_analysis: (scope.investigation_type == INVESTIGATION_TYPE_ORGANIZATION)
            .then(|| crate::ops::types::OrganizationAnalysis {
                findings_summary: Some(crate::ops::types::OrganizationFindingsSummary {
                    total_findings: Some(count(evidence.len())),
                    severity_breakdown: Some(severity_breakdown(&evidence)),
                    top_finding_types: Some(top_finding_types(&evidence)),
                    by_time_window: None,
                    // The organization as the pass read it: the administrator's own
                    // account plus the member accounts the detector held then.
                    total_account_count: Some(count(scope.organization_accounts + 1)),
                    affected_account_count: Some(count(account_breakdown(analysis, scope).len())),
                }),
                account_breakdown: Some(account_breakdown(analysis, scope)),
            }),
        // The key observations of the pass, whatever it covered: the conclusion it
        // reached (its title and the risk sentence behind it) and the resources the
        // analysed findings implicate.
        key_observations: Some(crate::ops::types::KeyObservations {
            title: Some(analysis.title.clone()),
            narrative: Some(analysis.risk.clone()),
            observations: Some(
                affected_resource_lines(&evidence)
                    .into_iter()
                    .take(OBSERVATIONS_MAX)
                    .collect(),
            ),
        }),
        // "For an ACCOUNT or ORGANIZATION investigation, the recommended actions"
        // (`smithy_query(query="doc",shape="InvestigationSummaryData",member="Recommendations")`),
        // which a finding-scoped pass reports as its countermeasures instead.
        recommendations: account_wide.then(|| recommendations(analysis)),
    }
}

/// The pass's recommendations, as the modelled `Recommendation` list: one entry per
/// step the assessment calls for ([`remediation`]).
pub fn recommendations(analysis: &InvestigationAnalysis) -> Vec<crate::ops::types::Recommendation> {
    remediation(analysis.evidence.is_empty(), &analysis.risk_level)
        .iter()
        .take(RECOMMENDATIONS_MAX)
        .map(|step| crate::ops::types::Recommendation {
            title: Some(step.action.to_string()),
            rationale: Some(step.rationale.to_string()),
            category: Some(step.category.to_string()),
            priority: Some(step.priority.to_string()),
            // "CLI commands that you can run to carry out the recommendation": the
            // step is stated in terms of the operator's own resources, which this
            // service holds no identifiers for, so no command is invented for it.
            cli_commands: None,
        })
        .collect()
}

/// The same steps as [`recommendations`], grouped the way a finding-scoped summary
/// reports them: "the actions that you can take for one defensive tactic"
/// (`smithy_query(shape="Countermeasure")`), one entry per `DefenseTactic` the steps
/// carry, in the order the steps are recommended in.
fn countermeasures(analysis: &InvestigationAnalysis) -> Vec<crate::ops::types::Countermeasure> {
    let steps = remediation(analysis.evidence.is_empty(), &analysis.risk_level);
    let mut tactics: Vec<&str> = Vec::new();
    for step in steps {
        if !tactics.contains(&step.defense_tactic) {
            tactics.push(step.defense_tactic);
        }
    }
    tactics
        .into_iter()
        .take(COUNTERMEASURES_MAX)
        .map(|tactic| crate::ops::types::Countermeasure {
            defense_tactic: Some(tactic.to_string()),
            description: Some(format!(
                "What the {} assessment of this finding calls for under the {tactic} tactic.",
                analysis.risk_level
            )),
            actions: Some(
                steps
                    .iter()
                    .filter(|step| step.defense_tactic == tactic)
                    .map(|step| crate::ops::types::CountermeasureAction {
                        title: Some(step.action.to_string()),
                        description: Some(step.rationale.to_string()),
                        priority: Some(step.priority.to_string()),
                        // The defensive technique, the MITRE techniques the action
                        // mitigates, the commands that carry it out, and the outcome
                        // of having carried it out: none of them is a fact the pass
                        // has, and the action has not been carried out at all, so
                        // each optional member is absent.
                        defense_technique: None,
                        mitigates: None,
                        cli_commands: None,
                        disposition_id: None,
                        disposition: None,
                        affected_resources: None,
                    })
                    .collect(),
            ),
        })
        .collect()
}

/// "The number of findings at each severity level"
/// (`smithy_query(shape="SeverityBreakdown")`): the analysed findings counted into
/// GuardDuty's own severity bands, the same bands the assessed `RiskLevel` names
/// ([`risk_level_of_severity`]).
fn severity_breakdown(evidence: &[&FindingEvidence]) -> crate::ops::types::SeverityBreakdown {
    let in_band = |band: &str| {
        count(
            evidence
                .iter()
                .filter(|finding| risk_level_of_severity(finding.severity) == band)
                .count(),
        )
    };
    crate::ops::types::SeverityBreakdown {
        critical: Some(in_band(RISK_LEVEL_CRITICAL)),
        high: Some(in_band(RISK_LEVEL_HIGH)),
        medium: Some(in_band(RISK_LEVEL_MEDIUM)),
        low: Some(in_band(RISK_LEVEL_LOW)),
        info: Some(in_band(RISK_LEVEL_INFO)),
    }
}

/// `TopFindingTypes`: each analysed `Finding$Type` and how many findings carry it,
/// most findings first ([`finding_type_counts`]).
fn top_finding_types(evidence: &[&FindingEvidence]) -> Vec<crate::ops::types::FindingTypeCount> {
    finding_type_counts(evidence)
        .into_iter()
        .take(TOP_FINDING_TYPES_MAX)
        .map(|(finding_type, found)| crate::ops::types::FindingTypeCount {
            finding_type: Some(finding_type),
            count: Some(count(found)),
        })
        .collect()
}

/// The resources the analysed findings implicate: one entry per
/// `Resource$ResourceType` and how many findings affect it.
///
/// `Identifier` is absent: a GuardDuty finding this service holds carries the KIND of
/// resource the activity affected and not an identifier for it, so there is none to
/// report.
fn affected_resources(
    evidence: &[&FindingEvidence],
) -> Vec<crate::ops::types::AccountAffectedResource> {
    resource_type_counts(evidence)
        .into_iter()
        .take(AFFECTED_RESOURCES_MAX)
        .map(
            |(resource_type, findings)| crate::ops::types::AccountAffectedResource {
                identifier: None,
                r#type: Some(resource_type),
                finding_count: Some(count(findings)),
            },
        )
        .collect()
}

/// `AccountBreakdown`: the accounts of the organization the pass found findings in,
/// with how many each holds, most findings first.
///
/// Every finding the pass read was produced in the account the investigation covers
/// ([`FindingEvidence`] comes off that account's detector), so an organization-wide
/// pass that read findings reports exactly that one account, and one that read none
/// reports no account at all — which is what `AffectedAccountCount` counts.
fn account_breakdown(
    analysis: &InvestigationAnalysis,
    scope: &InvestigationScopeRecord,
) -> Vec<crate::ops::types::AccountSummary> {
    let Some(account) = scope.admin_account_id.clone() else {
        return Vec::new();
    };
    if analysis.evidence.is_empty() {
        return Vec::new();
    }
    let mut breakdown = vec![crate::ops::types::AccountSummary {
        account_id: Some(account),
        finding_count: Some(count(analysis.evidence.len())),
    }];
    breakdown.truncate(ACCOUNT_BREAKDOWN_MAX);
    breakdown
}

/// One count, as the modelled `Integer` members carry it. Saturating rather than
/// wrapping: a count this store cannot reach is still a count.
fn count(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}
