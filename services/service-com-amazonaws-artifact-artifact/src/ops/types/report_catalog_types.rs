// SPDX-License-Identifier: CC0-1.0

//! `ReportSummary` and `ReportDetail`: the two projections of a published AWS Artifact
//! report, as AWS carries them.
//!
//! These are the ONE pair of report types this crate has — `ListReports` and
//! `ListReportVersions` project a catalog entry through the summary, `GetReportMetadata`
//! through the detail — so what they declare is what every report read of this service
//! reports.
//!
//! They are declared here rather than beside the other modelled shapes because AWS answers
//! those reads with one member the upstream model does not declare, and that model is kept
//! exactly as the update wrote it (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `visibility` — `PUBLIC` for a report AWS publishes in the catalog. The model declares
//!   neither the member nor a shape of that name.
//!
//! So the types are generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the members the model
//! declares, with their own targets so every length, pattern, range and enum constraint is
//! captured exactly as the model spells it, and adds the one it lacks. Nothing about the
//! wire is hand-written: the member names, the shape ids and the constraints all come from
//! that file, and the `date-time` form every timestamp member of this service takes comes
//! from the protocol this service serves (`crate::service`), not from a schema written out
//! here.
//!
//! `tests/all/report_catalog_local_members.rs` pins the member on the wire and the upstream
//! model's own member list, which these declarations must cover in full.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.artifact#ReportSummary")]
#[derive(Debug, Clone, Default)]
pub struct ReportSummary {
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: Option<String>,
    pub arn: Option<String>,
    pub version: Option<i64>,
    pub upload_state: Option<String>,
    pub description: Option<String>,
    pub period_start: Option<::aws_smithy_types::DateTime>,
    pub period_end: Option<::aws_smithy_types::DateTime>,
    pub series: Option<String>,
    pub category: Option<String>,
    pub company_name: Option<String>,
    pub product_name: Option<String>,
    pub status_message: Option<String>,
    pub acceptance_type: Option<String>,
    /// The member the upstream model does not declare. An open string: the value reported
    /// is the one the stored catalog record holds
    /// ([`crate::state::Report::visibility`]), so no value set is invented for it.
    pub visibility: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.artifact#ReportDetail")]
#[derive(Debug, Clone, Default)]
pub struct ReportDetail {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub period_start: Option<::aws_smithy_types::DateTime>,
    pub period_end: Option<::aws_smithy_types::DateTime>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub last_modified_at: Option<::aws_smithy_types::DateTime>,
    pub deleted_at: Option<::aws_smithy_types::DateTime>,
    pub state: Option<String>,
    pub arn: Option<String>,
    pub series: Option<String>,
    pub category: Option<String>,
    pub company_name: Option<String>,
    pub product_name: Option<String>,
    pub term_arn: Option<String>,
    pub version: Option<i64>,
    pub acceptance_type: Option<String>,
    pub sequence_number: Option<i64>,
    pub upload_state: Option<String>,
    pub status_message: Option<String>,
    /// The member the upstream model does not declare; see
    /// [`ReportSummary::visibility`].
    pub visibility: Option<String>,
}
