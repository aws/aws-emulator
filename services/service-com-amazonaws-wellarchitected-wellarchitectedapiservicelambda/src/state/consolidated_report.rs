// SPDX-License-Identifier: CC0-1.0

//! The CONSOLIDATED report over the workloads an account can see: the metrics
//! `GetConsolidatedReport` hands back, and the document it hands them back as when the
//! request asks for one.
//!
//! "Get a consolidated report of your workloads" (`GetConsolidatedReport` doc): one
//! `ConsolidatedReportMetric` per reported workload, DERIVED from that workload's stored
//! state on every read — its identity and the instant it last changed, the lenses applied
//! to it, and the `Risk` roll-ups of the workload, of each of its lens reviews and of each
//! pillar those lenses are organized into, counted over the workload's stored answers
//! through the same helpers a read of one lens review counts with
//! ([`WorkloadRecord::risk_counts`], [`WorkloadRecord::lens_risk_counts`],
//! [`WorkloadRecord::pillar_risk_counts`]). Nothing is stored twice, so a metric read after
//! a write reports that write and cannot disagree with `GetLensReview` about the same
//! review.
//!
//! The projection lives beside the record types for the reason
//! [`WorkloadRecord::to_lens_review`] does: a response built from a stored record is built
//! in one place. The handler resolves which workloads the report covers and pages over
//! them.

use super::*;

use crate::ops::types::{
    BestPractice, ConsolidatedReportMetric, LensMetric, PillarMetric, QuestionMetric,
};

/// `MetricType$WORKLOAD`: "the metric type of a metric in the consolidated report.
/// Currently only WORKLOAD metric types are supported"
/// (`smithy_query(query="shape", shape="ConsolidatedReportMetric")`), and the enum
/// publishes that one member (`smithy_query(query="shape", shape="MetricType")`), so every
/// metric of this report carries it.
pub const METRIC_TYPE_WORKLOAD: &str = "WORKLOAD";

/// `ReportFormat$JSON` — the format whose report is the metrics themselves: "for JSON,
/// Metrics is returned" (`GetConsolidatedReportInput$Format` doc).
pub const REPORT_FORMAT_JSON: &str = "JSON";

/// `ReportFormat$PDF` — the format whose report is a document: "for PDF, Base64String is
/// returned" (`GetConsolidatedReportInput$Format` doc).
pub const REPORT_FORMAT_PDF: &str = "PDF";

/// The ceiling `GetConsolidatedReport` serves: its `MaxResults` member is
/// `@range(min: 1, max: 15)`
/// (`smithy_query(query="operation", operation="GetConsolidatedReport")`), its own bound
/// rather than the one most of this service's paginated reads carry
/// ([`MAX_RESULTS_CEILING`]).
pub const CONSOLIDATED_REPORT_MAX_RESULTS_CEILING: i32 = 15;

impl ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// Whether the consolidated report a request from `caller` asks for covers `workload`.
    ///
    /// A consolidated report is a report of "your workloads", and `IncludeSharedResources`
    /// is "set to true to have shared resources included in the report"
    /// (`GetConsolidatedReportInput$IncludeSharedResources` doc): the account's OWN
    /// workloads are always covered, and a workload another account owns is covered only
    /// when the request asks for shared resources AND that account's share of it has been
    /// accepted by this one — a share still waiting on its invitation
    /// ([`DEFAULT_SHARE_STATUS`]) has given the caller nothing to report yet, and one that
    /// was rejected has given it nothing at all.
    pub fn consolidated_report_covers(
        &self,
        workload: &WorkloadRecord,
        caller: &str,
        include_shared_resources: bool,
    ) -> bool {
        if workload.owner == caller {
            return true;
        }
        include_shared_resources
            && self
                .workload_share_with(&workload.workload_id, caller)
                .is_some_and(|share| share.status == SHARE_STATUS_ACCEPTED)
    }

    /// The `ConsolidatedReportMetric` of ONE workload the report covers.
    ///
    /// Every member is derived from the stored workload and the lens catalog on each read:
    /// the workload's identity and `UpdatedAt` from the record, `LensesAppliedCount` from
    /// the lenses it applies — each lens counted once however many spellings named it
    /// ([`WorkloadRecord::lens_reviews`]) — and the roll-ups from its stored answers,
    /// counted over the content of the lenses it reviews for the version each review is
    /// against ([`Self::reviewed_lens_content`]).
    pub fn consolidated_report_metric(
        &self,
        workload: &WorkloadRecord,
    ) -> ConsolidatedReportMetric {
        let content = self.reviewed_lens_content(&workload.reviewed_lens_versions());
        // "The metrics for the lenses in the workload", in the order the workload applies
        // its lenses — the order a read of the workload reports its `Lenses` in.
        let lenses: Vec<LensMetric> = workload
            .lens_reviews()
            .into_iter()
            .map(|(_, lens)| lens_metric(workload, lens, self.lens_arn_of(lens), &content))
            .collect();
        ConsolidatedReportMetric {
            metric_type: Some(METRIC_TYPE_WORKLOAD.to_string()),
            risk_counts: Some(risk_counts_to_map(&workload.risk_counts(&content))),
            workload_id: Some(workload.workload_id.clone()),
            workload_name: Some(workload.workload_name.clone()),
            workload_arn: Some(workload.workload_arn.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(workload.updated_at)),
            // "The total number of lenses applied to the workload": the lens reviews the
            // metric reports, so the count and the list cannot disagree.
            lenses_applied_count: Some(i32::try_from(lenses.len()).unwrap_or(i32::MAX)),
            lenses: Some(lenses),
        }
    }
}

/// The `LensMetric` of ONE lens review of `workload`: "a metric for a particular lens in a
/// workload".
///
/// `lens` is the lens as the workload records it, which is the spelling its `LensArn` is
/// resolved from, and `content` is as [`Self::consolidated_report_metric`] resolves it. The
/// pillars are the ones the review itself reports a summary for
/// ([`WorkloadRecord::lens_review_pillars`]), so a pillar in the metric is a pillar of the
/// review.
fn lens_metric(
    workload: &WorkloadRecord,
    lens: &str,
    lens_arn: String,
    content: &ReviewedLensContent,
) -> LensMetric {
    LensMetric {
        lens_arn: Some(lens_arn),
        risk_counts: Some(risk_counts_to_map(
            &workload.lens_risk_counts(lens, content),
        )),
        pillars: Some(
            workload
                .lens_review_pillars(lens, content)
                .into_iter()
                .map(|(pillar_id, _)| pillar_metric(workload, lens, &pillar_id, content))
                .collect(),
        ),
    }
}

/// The `PillarMetric` of ONE pillar of one lens review: "a metric for a particular pillar
/// in a lens".
///
/// `Questions` is "the questions that have been identified as risks in the pillar"
/// (`smithy_query(query="shape", shape="PillarMetric")`), so the pillar's questions are
/// SELECTED down to the ones the review identified a risk in ([`identified_risk`]) — in the
/// lens's own question order — while `RiskCounts` counts every question the pillar asks.
fn pillar_metric(
    workload: &WorkloadRecord,
    lens: &str,
    pillar_id: &str,
    content: &ReviewedLensContent,
) -> PillarMetric {
    PillarMetric {
        pillar_id: Some(pillar_id.to_string()),
        risk_counts: Some(risk_counts_to_map(&workload.pillar_risk_counts(
            lens,
            pillar_id,
            content,
        ))),
        questions: Some(
            content
                .of(lens)
                .pillar_questions(pillar_id)
                .filter_map(|question| {
                    question_metric(question, workload.answer(lens, &question.question_id))
                })
                .collect(),
        ),
    }
}

/// The `QuestionMetric` of ONE question the review identified a risk in, and nothing for a
/// question it did not ([`identified_risk`]).
///
/// `BestPractices` is "the best practices, or choices, that have been identified as
/// contributing to risk in a question"
/// (`smithy_query(query="shape", shape="QuestionMetric")`), and a best practice contributes
/// to a question's risk while the answer leaves it out: the choices the answer records
/// [`CHOICE_STATUS_UNSELECTED`] for, in the lens's own choice order. A choice the answer
/// records as in place, or as not applying to this review, contributes nothing.
fn question_metric(
    question: &ReviewQuestion,
    answer: Option<&AnswerRecord>,
) -> Option<QuestionMetric> {
    let risk = identified_risk(question, answer)?;
    let answer = answer?;
    let best_practices = question
        .best_practices
        .iter()
        .zip(answer.reported_choice_answers(question))
        .filter(|(_, reported)| reported.status == CHOICE_STATUS_UNSELECTED)
        .map(|(choice, _)| BestPractice {
            choice_id: Some(choice.id.clone()),
            choice_title: Some(choice.title.clone()),
        })
        .collect();
    Some(QuestionMetric {
        question_id: Some(question.question_id.clone()),
        risk: Some(risk.to_string()),
        best_practices: Some(best_practices),
    })
}

/// The document a `GetConsolidatedReport` that asks for [`REPORT_FORMAT_PDF`] reports:
/// `metrics` — the same metrics the JSON format returns for that page — set as a PDF
/// document through the one place this service writes one ([`crate::state::report_pdf`]).
///
/// The document SAYS what the metrics say, so the two formats of one page cannot disagree,
/// and a page of no workloads is still a document a reader opens.
pub fn consolidated_report_document(metrics: &[ConsolidatedReportMetric]) -> Vec<u8> {
    pdf_document(&consolidated_report_lines(metrics))
}

/// What the consolidated report document SHOWS, one line per entry: the workloads this page
/// reports, each with its identity, its roll-up, and the lenses, pillars and at-risk
/// questions of its metric.
fn consolidated_report_lines(metrics: &[ConsolidatedReportMetric]) -> Vec<String> {
    let mut lines = vec![
        "AWS Well-Architected Tool - consolidated report".to_string(),
        String::new(),
        format!("Workloads reported: {}", metrics.len()),
    ];
    for metric in metrics {
        lines.push(String::new());
        lines.extend(body(&format!(
            "Workload: {} ({})",
            shown_text(&metric.workload_name),
            shown_text(&metric.workload_id)
        )));
        lines.push(format!(
            "Workload ARN: {}",
            shown_text(&metric.workload_arn)
        ));
        lines.push(format!("Metric type: {}", shown_text(&metric.metric_type)));
        lines.push(format!(
            "Last updated: {}",
            metric
                .updated_at
                .as_ref()
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
                .unwrap_or_else(|| UNRECORDED.to_string())
        ));
        lines.push(format!(
            "Lenses applied: {}",
            metric
                .lenses_applied_count
                .map_or_else(|| UNRECORDED.to_string(), |count| count.to_string())
        ));
        lines.push("Risk counts for this workload".to_string());
        lines.extend(counted(metric.risk_counts.as_ref(), "  "));
        for lens in metric.lenses.iter().flatten() {
            lines.push(String::new());
            lines.push(format!("  Lens ARN: {}", shown_text(&lens.lens_arn)));
            lines.push("  Risk counts for this lens review".to_string());
            lines.extend(counted(lens.risk_counts.as_ref(), "    "));
            for pillar in lens.pillars.iter().flatten() {
                lines.push(format!("  Pillar: {}", shown_text(&pillar.pillar_id)));
                lines.extend(counted(pillar.risk_counts.as_ref(), "    "));
                for question in pillar.questions.iter().flatten() {
                    lines.extend(body(&format!(
                        "    Question {} at risk: {}",
                        shown_text(&question.question_id),
                        shown_text(&question.risk)
                    )));
                    for best_practice in question.best_practices.iter().flatten() {
                        lines.extend(
                            body(&format!(
                                "Best practice left out: {} ({})",
                                shown_text(&best_practice.choice_title),
                                shown_text(&best_practice.choice_id)
                            ))
                            .into_iter()
                            .map(|line| format!("      {line}")),
                        );
                    }
                }
            }
        }
    }
    lines
}

/// One risk roll-up as the document shows it: one `indent`ed line per `Risk` the enum
/// publishes, in the enum's own order, so every bucket is in the document whether anything
/// counts under it or not and two reports of the same roll-up read identically.
fn counted(counts: Option<&HashMap<String, i32>>, indent: &str) -> Vec<String> {
    RISK_VALUES
        .iter()
        .map(|risk| {
            let count = counts
                .and_then(|counts| counts.get(*risk))
                .copied()
                .unwrap_or_default();
            format!("{indent}{risk}: {count}")
        })
        .collect()
}

/// One optional member of a metric as the document shows it, and [`UNRECORDED`] for a
/// member the metric carries no value for.
fn shown_text(member: &Option<String>) -> &str {
    member.as_deref().unwrap_or(UNRECORDED)
}
