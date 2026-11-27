// SPDX-License-Identifier: CC0-1.0

//! `PointInTimeRecoveryDescription`: the point-in-time recovery settings of a table, as AWS
//! carries them.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers with
//! one member the published model snapshot does not declare, and that snapshot is kept as
//! shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `PointInTimeRecoveryWindowDays` — the recovery window in force, which the
//!   continuous-backups readers carry beside `RecoveryPeriodInDays`, the member the snapshot
//!   names the same setting by.
//!
//! So the type is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every range and enum value is captured exactly as the
//! snapshot spells it, and adds the one AWS carries — targeting the same shape as the member
//! beside it, since it is the same number of days.
//!
//! What the members CARRY is not decided here: it is one stored setting
//! ([`crate::state`]'s continuous-backups record), so the two can never disagree about how
//! far back a table may be restored.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The point-in-time recovery settings of one table.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#PointInTimeRecoveryDescription")]
#[derive(Debug, Clone, Default)]
pub struct PointInTimeRecoveryDescription {
    pub point_in_time_recovery_status: Option<String>,
    /// The member the snapshot does not declare: the same recovery window, under the name
    /// AWS reports beside the modelled one.
    pub point_in_time_recovery_window_days: Option<i32>,
    pub recovery_period_in_days: Option<i32>,
    pub earliest_restorable_date_time: Option<::aws_smithy_types::DateTime>,
    pub latest_restorable_date_time: Option<::aws_smithy_types::DateTime>,
}
