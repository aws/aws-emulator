// SPDX-License-Identifier: CC0-1.0

//! `ReportPlan`: the report plan every read of one reports, and the schedule block it
//! carries.
//!
//! This is the ONE `ReportPlan` type of this crate — `DescribeReportPlan` and
//! `ListReportPlans` both report a plan through it — so what it declares is what every
//! read of a plan puts on the wire.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers
//! with one member the published model snapshot does not declare, and that snapshot is
//! kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `ReportTiming` — the TIMING of the plan's report: how many hours apart two runs of
//!   it are and the hour of the day a run starts at. The snapshot declares neither the
//!   member nor a shape of that name.
//!
//! So the types are generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the nine members
//! the snapshot declares, with their own targets so every length and pattern constraint
//! is captured exactly as the snapshot spells it, and adds the one AWS carries. Nothing
//! about the wire is hand-written: the member names, the shape ids and the constraints
//! all come from that file.
//!
//! The VALUE of the added member is not declared here: it is the schedule a plan is
//! stored with ([`crate::state::stored_report_timing`]), so every read of one plan
//! reports the same timing.
//!
//! `tests/all/report_plan_local_members.rs` pins the member on the wire and the overlay
//! against the snapshot it restates; `tests/all/report_plan_wire_form.rs` pins the body
//! a read reports.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// When the report of a plan runs: how many hours apart two runs are, and the hour of
/// the day a run starts at.
///
/// The structure the member the snapshot does not declare targets. Each of its two
/// members may be absent on the wire — the overlay targets the service's own integer
/// shape without claiming a default for them — and a stored plan carries both, so every
/// read reports both ([`crate::state::stored_report_timing`]).
#[smithy_macros::smithy(shape = "com.amazonaws.backup#ReportTiming")]
#[derive(Debug, Clone, Default)]
pub struct ReportTiming {
    /// How many hours apart the report's runs are.
    pub report_frequency_hours: Option<i32>,
    /// The hour of the day, in UTC, the report's run starts at.
    pub report_start_hour: Option<i32>,
}

/// A report plan, as every read of one reports it: the modelled members in the model's
/// own order, then the one member AWS carries and the snapshot does not declare.
#[smithy_macros::smithy(shape = "com.amazonaws.backup#ReportPlan")]
#[derive(Debug, Clone, Default)]
pub struct ReportPlan {
    pub report_plan_arn: Option<String>,
    pub report_plan_name: Option<String>,
    pub report_plan_description: Option<String>,
    pub report_setting: Option<ReportSetting>,
    pub report_delivery_channel: Option<ReportDeliveryChannel>,
    pub deployment_status: Option<String>,
    pub creation_time: Option<::aws_smithy_types::DateTime>,
    pub last_attempted_execution_time: Option<::aws_smithy_types::DateTime>,
    pub last_successful_execution_time: Option<::aws_smithy_types::DateTime>,
    /// The member the snapshot does not declare: when this plan's report runs.
    pub report_timing: Option<ReportTiming>,
}
