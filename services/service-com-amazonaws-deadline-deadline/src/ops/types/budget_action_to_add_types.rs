// SPDX-License-Identifier: CC0-1.0

//! The `BudgetActionToAdd` a budget request carries each of its actions as.
//!
//! The shape is declared from this service's own model, against the same target, with one
//! local decision on it: its `thresholdPercentage` is refused in this service's body
//! validator's words rather than the framework's, so `skip_validation` leaves that
//! member's modeled `@range` to the handler
//! ([`crate::state::check_budget_actions`]) — the way the two request shapes that carry
//! the list already leave their own `@length` and `@range` members to it
//! ([`super::create_budget_types`], [`super::update_budget_types`], the latter declaring
//! `BudgetActionToRemove` — the same member on the removal side — the same way). Every
//! other constraint on the shape, `description`'s `@length` included, is captured from
//! the model and enforced at the boundary exactly as the generator captures it, so
//! nothing about the wire is hand-written here.
//!
//! It is declared in THIS module rather than beside the other shapes more than one
//! operation reaches because `skip_validation` is a choice of this crate that the model
//! cannot express: `shared_types` is rendered from the model and is rewritten whenever the
//! upstream model changes, which drops the choice and lets the framework's generic
//! constraint sentence reach a caller in place of this service's finding list.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-deadline-deadline.json");

/// "The budget action to add": the action type, the percentage of the budget it fires at,
/// and an optional description.
#[smithy_macros::smithy(shape = "com.amazonaws.deadline#BudgetActionToAdd", skip_validation = ["thresholdPercentage"])]
#[derive(Debug, Clone, Default)]
pub struct BudgetActionToAdd {
    pub r#type: String,
    pub threshold_percentage: f32,
    pub description: Option<String>,
}
