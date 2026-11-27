// SPDX-License-Identifier: CC0-1.0

//! The QUERY one widget of a dashboard runs, as AWS carries it.
//!
//! This is the `QueryParameters` union every `Widget$configs` entry carries, with the two
//! query shapes only it reaches. They are declared here rather than beside the other
//! shapes because AWS answers with two union members the published model snapshot does not
//! declare, and that snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `budget` — the query the published budget-health panel runs: the account's own
//!   budgets, narrowed by an optional `filter` and by nothing else, so it models no time
//!   period, no granularity and no metrics.
//! * `costEfficiency` — the query the published cost-efficiency panel runs: a time range at
//!   a granularity, optionally grouped and filtered, whose series is a score rather than a
//!   `MetricName`.
//!
//! The snapshot declares neither member, and no shape of either query's name.
//!
//! So the types are generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the union's five
//! modelled members with their own targets, so each still delegates to the modelled query
//! shape in [`super::shared_types`], and adds the two AWS carries. Nothing about the wire
//! is hand-written: the member names, the shape ids and the constraints all come from that
//! file.
//!
//! `tests/all/local_query_parameters.rs` pins the members on the wire and the overlay
//! against the snapshot it restates.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// Which query a widget's configuration runs. A union carries exactly one member, so a
/// panel runs exactly one of these.
#[smithy_macros::smithy(union_for = "com.amazonaws.bcmdashboards#QueryParameters")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum QueryParameters {
    costAndUsage(CostAndUsageQuery),
    savingsPlansCoverage(SavingsPlansCoverageQuery),
    savingsPlansUtilization(SavingsPlansUtilizationQuery),
    reservationCoverage(ReservationCoverageQuery),
    reservationUtilization(ReservationUtilizationQuery),
    /// The member the snapshot does not declare, named `budget` on the wire — hence the
    /// capitalized variant, which is how an all-lowercase Smithy member name maps.
    Budget(BudgetQuery),
    /// The other member the snapshot does not declare.
    costEfficiency(CostEfficiencyQuery),
}

/// The `budget` query: the account's own budgets, selected by `filter` — every budget the
/// account holds when it selects none.
#[smithy_macros::smithy(shape = "com.amazonaws.bcmdashboards#BudgetQuery")]
#[derive(Debug, Clone, Default)]
pub struct BudgetQuery {
    pub filter: Option<Expression>,
}

/// The `costEfficiency` query: an efficiency score over a time period, at a granularity,
/// optionally grouped and filtered.
#[smithy_macros::smithy(shape = "com.amazonaws.bcmdashboards#CostEfficiencyQuery")]
#[derive(Debug, Clone, Default)]
pub struct CostEfficiencyQuery {
    pub time_range: DateTimeRange,
    pub granularity: Option<String>,
    pub group_by: Option<Vec<GroupDefinition>>,
    pub filter: Option<Expression>,
}
