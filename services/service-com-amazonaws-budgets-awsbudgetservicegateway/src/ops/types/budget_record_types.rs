// SPDX-License-Identifier: CC0-1.0

//! The BUDGET, its NOTIFICATIONS and their SUBSCRIBERS, as AWS carries them.
//!
//! These are the three shapes every operation of this crate parses, persists and reports
//! a budget through: the `Budget` of `CreateBudget`, `UpdateBudget`, `DescribeBudget` and
//! `DescribeBudgets`, the `Notification` of the notification operations and of
//! `DescribeBudgetNotificationsForAccount`, and the `Subscriber` of both the notification
//! and the budget-action operations — so what they declare is what every read reports.
//!
//! They are declared here rather than beside the other shared shapes because AWS answers
//! with three members the published model snapshot does not declare, and that snapshot is
//! kept EXACTLY as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `Budget$CreatedAt` — the instant the budget came into being, which the recorded reads
//!   of a budget carry on both read paths.
//! * `Notification$ConsecutiveBreachCount` — how many budget periods in a row the spend the
//!   notification watches has passed its threshold.
//! * `Subscriber$VerificationStatus` — whether the address has been confirmed, which the
//!   recorded reads of a budget ACTION's subscribers carry.
//!
//! So the types are generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the members the
//! snapshot declares, with their own targets so every length, pattern, range and enum
//! constraint is captured exactly as the snapshot spells it, and adds the three AWS
//! carries. Nothing about the wire is hand-written: the member names, the shape ids and
//! the constraints all come from that file.
//!
//! The VALUES of the three are not declared here: each is owned by the service rather than
//! by the caller, so the write paths drop whatever a request carried for it
//! ([`crate::state::stamp_service_owned`], [`crate::state::stamp_notification_owned`],
//! [`crate::state::drop_subscriber_derived`]) and the reads report what this service
//! stamped or counted ([`crate::state::consecutive_breach_count`],
//! [`crate::state::action_subscribers`]).
//!
//! `tests/all/recorded_response_members.rs` pins all three on the wire and the overlay
//! against the snapshot it restates.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// A budget, as every operation of this service reads and reports one.
#[smithy_macros::smithy(shape = "com.amazonaws.budgets#Budget")]
#[derive(Debug, Clone, Default)]
pub struct Budget {
    pub budget_name: String,
    pub budget_limit: Option<Spend>,
    pub planned_budget_limits: Option<std::collections::HashMap<String, Spend>>,
    pub cost_filters: Option<std::collections::HashMap<String, Vec<String>>>,
    pub cost_types: Option<CostTypes>,
    pub time_unit: String,
    pub time_period: Option<TimePeriod>,
    pub calculated_spend: Option<CalculatedSpend>,
    pub budget_type: String,
    pub last_updated_time: Option<::aws_smithy_types::DateTime>,
    pub auto_adjust_data: Option<AutoAdjustData>,
    pub filter_expression: Option<Expression>,
    pub metrics: Option<Vec<String>>,
    pub billing_view_arn: Option<String>,
    pub health_status: Option<HealthStatus>,
    /// The member the snapshot does not declare: the instant the budget was created. A
    /// write that establishes a budget stamps it and an update leaves it alone, so every
    /// read of one budget reports the same instant.
    pub created_at: Option<::aws_smithy_types::DateTime>,
}

/// One notification of a budget, as every operation of this service reads and reports one.
#[smithy_macros::smithy(shape = "com.amazonaws.budgets#Notification")]
#[derive(Debug, Clone, Default)]
pub struct Notification {
    pub notification_type: String,
    pub comparison_operator: String,
    pub threshold: f64,
    pub threshold_type: Option<String>,
    pub notification_state: Option<String>,
    /// The member the snapshot does not declare: how many budget periods in a row the
    /// spend this notification watches has passed its threshold, counted off the budget's
    /// own figures on every read.
    pub consecutive_breach_count: Option<i32>,
}

/// One subscriber of a notification or of a budget action.
#[smithy_macros::smithy(shape = "com.amazonaws.budgets#Subscriber")]
#[derive(Debug, Clone, Default)]
pub struct Subscriber {
    pub subscription_type: String,
    pub address: String,
    /// The member the snapshot does not declare: whether the address has been confirmed.
    /// An open string, because nothing publishes the value set; the one state this
    /// service's own records reach is [`crate::state::SUBSCRIBER_VERIFICATION_PENDING`].
    pub verification_status: Option<String>,
}
