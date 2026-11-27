// SPDX-License-Identifier: CC0-1.0

//! `ReportDefinition`: one Cost and Usage Report definition, as AWS carries it.
//!
//! This is the ONE `ReportDefinition` type of this crate — the shape
//! `PutReportDefinitionRequest`, `ModifyReportDefinitionRequest` and the
//! `ReportDefinitionList` of `DescribeReportDefinitions` all reach — so what it
//! declares is what every operation of this service parses, persists and
//! reports.
//!
//! It is declared here rather than beside the other shared shapes because AWS
//! answers with one member the published model snapshot does not declare, and
//! that snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `ReportCreationDate` — the date the definition was created, which every
//!   recorded `DescribeReportDefinitions` read-back carries. The snapshot
//!   declares neither the member nor a shape of that name.
//!
//! So the type is generated, like every other modelled shape, from a model file
//! — the small overlay `smithy-model/local-additions.json`, which restates the
//! thirteen members the snapshot declares, with their own targets so every
//! length, pattern and enum constraint is captured exactly as the snapshot
//! spells it, and adds the fourteenth. Nothing about the wire is hand-written:
//! the member names, the shape id and the constraints all come from that file.
//!
//! `tests/all/report_definition_local_members.rs` pins the member on the wire
//! and the overlay against the snapshot, shape by shape.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.costandusagereportservice#ReportDefinition")]
#[derive(Debug, Clone, Default)]
pub struct ReportDefinition {
    pub report_name: String,
    pub time_unit: String,
    pub format: String,
    pub compression: String,
    pub additional_schema_elements: Vec<String>,
    pub s3_bucket: String,
    pub s3_prefix: String,
    pub s3_region: String,
    pub additional_artifacts: Option<Vec<String>>,
    pub refresh_closed_reports: Option<bool>,
    pub report_versioning: Option<String>,
    pub billing_view_arn: Option<String>,
    pub report_status: Option<ReportStatus>,
    /// The member the snapshot does not declare: the date the service created
    /// this definition, stated in the form this service states a report
    /// timestamp in (`crate::state::report_timestamp`). An open string, because
    /// nothing publishes a constraint for it, and the service's own fact about
    /// the report rather than a caller's to submit — the read-back fills it from
    /// the stored creation instant
    /// (`crate::state::StoredReport::wire_definition`).
    pub report_creation_date: Option<String>,
}
