// SPDX-License-Identifier: CC0-1.0

//! The REPORT of ONE lens review of a workload: the document `GetLensReviewReport` hands
//! back.
//!
//! The report is a `LensReviewReport`, whose `Base64String` is "the Base64-encoded string
//! representation of a lens review report" that "can be used to create a PDF file"
//! (`smithy_query(query="shape", shape="Base64String")`) — so what the member carries is a
//! PDF DOCUMENT, written through the one place this service writes one
//! ([`crate::state::report_pdf`]) so a caller that decodes it and saves it opens a document
//! rather than bytes shaped like one.
//!
//! Everything the document says is DERIVED from the reviewed workload's stored state on
//! every read — the workload's own members, the lens the catalog resolves for the version
//! the review is against, the review's notes, and the `Risk` roll-ups counted over the
//! lens's and each pillar's questions from the workload's stored answers. Nothing about a
//! review is stored twice, so a report read after a write reports that write, and a report
//! of the workload a milestone recorded reports the review that milestone froze.
//!
//! The projection lives beside the record types for the reason
//! [`WorkloadRecord::to_lens_review`] does: a response built from a stored record is built
//! in one place, so the review a caller READS and the review a report DESCRIBES cannot
//! disagree. The handler resolves the workload, the milestone and the lens and then reads
//! this.

use super::*;

use crate::ops::types::LensReviewReport;

/// The `LensReviewReport` of ONE lens review of `workload`.
///
/// `lens` is the lens as the request named it — an alias or an ARN — which is the spelling
/// the report echoes, exactly as [`WorkloadRecord::to_lens_review`] echoes it; `reviewed`
/// is what the lens catalog says about the lens for the version this review is against
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_at`]),
/// and `content` the content of the lenses the workload reviews
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`]),
/// which the roll-ups in the document are counted over. `milestone_number` is the
/// milestone the read was scoped to, when it was scoped to one.
pub fn lens_review_report(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> LensReviewReport {
    let document = report_document(workload, lens, reviewed, content, milestone_number);
    LensReviewReport {
        lens_alias: Some(lens.to_string()),
        lens_arn: Some(reviewed.lens_arn.clone()),
        base64_string: Some(::aws_smithy_types::base64::encode(&document)),
    }
}

/// The report document itself: the lines [`report_lines`] derives from the stored review,
/// set on as many pages as they take.
fn report_document(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> Vec<u8> {
    pdf_document(&report_lines(
        workload,
        lens,
        reviewed,
        content,
        milestone_number,
    ))
}

/// What the report SAYS, one line per entry: the workload under review, the lens the
/// review is of, the review's own notes, and the risk roll-up of the review as a whole and
/// of each pillar the lens is organized into.
///
/// Derived in full from the arguments, so the same review always reads the same way and a
/// report taken before and after a write differs exactly where the review does.
fn report_lines(
    workload: &WorkloadRecord,
    lens: &str,
    reviewed: &ReviewedLens,
    content: &ReviewedLensContent,
    milestone_number: Option<i32>,
) -> Vec<String> {
    let written = workload.lens_review_record(lens);
    let mut lines = vec![
        "AWS Well-Architected Tool - lens review report".to_string(),
        String::new(),
        format!(
            "Workload: {} ({})",
            workload.workload_name, workload.workload_id
        ),
        format!("Workload ARN: {}", workload.workload_arn),
        format!("Environment: {}", workload.environment),
        format!("Owner account: {}", workload.owner),
        format!(
            "Review owner: {}",
            workload.review_owner.as_deref().unwrap_or(UNRECORDED)
        ),
    ];
    // Named only when the read was scoped to a milestone: a report of the live review is a
    // report of no milestone.
    if let Some(number) = milestone_number {
        lines.push(format!("Milestone number: {number}"));
    }
    lines.push(format!("Last updated: {}", at(workload.updated_at)));
    lines.push(String::new());
    lines.push(format!(
        "Lens: {} ({})",
        reviewed.lens_name.as_deref().unwrap_or(UNRECORDED),
        lens_alias(lens)
    ));
    lines.push(format!("Lens ARN: {}", reviewed.lens_arn));
    lines.push(format!(
        "Lens version: {}",
        reviewed.lens_version.as_deref().unwrap_or(UNRECORDED)
    ));
    lines.push(format!(
        "Lens status: {}",
        reviewed.lens_status.as_deref().unwrap_or(UNRECORDED)
    ));

    lines.push(String::new());
    lines.push("Risk counts for this lens review".to_string());
    lines.extend(counted(&workload.lens_risk_counts(lens, content)));

    if let Some(notes) = written.and_then(|written| written.notes.as_deref()) {
        lines.push(String::new());
        lines.push("Lens review notes".to_string());
        lines.extend(body(notes));
    }

    for (pillar_id, pillar_name) in workload.lens_review_pillars(lens, content) {
        lines.push(String::new());
        lines.push(format!(
            "Pillar: {} ({pillar_id})",
            pillar_name.as_deref().unwrap_or(UNRECORDED)
        ));
        lines.extend(counted(&workload.pillar_risk_counts(
            lens,
            &pillar_id,
            content,
        )));
        if let Some(notes) = written.and_then(|written| written.pillar_notes.get(&pillar_id)) {
            lines.push("  Notes".to_string());
            lines.extend(body(notes).into_iter().map(|line| format!("  {line}")));
        }
    }

    lines
}

/// One risk roll-up as the document shows it: one indented line per `Risk` bucket, in the
/// bucket order the roll-up is held in, so every bucket the enum publishes is in the
/// report whether anything counts under it or not.
fn counted(counts: &BTreeMap<String, i32>) -> Vec<String> {
    counts
        .iter()
        .map(|(risk, count)| format!("  {risk}: {count}"))
        .collect()
}
