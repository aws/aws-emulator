// SPDX-License-Identifier: CC0-1.0

//! One gateway's rules: the record a rule is stored as, the clock-owed status it
//! reports, and the response envelopes every operation that reports one renders.
//!
//! One concern, kept beside the gateway record that holds the rules rather than
//! in any one `ops/` file: a rule is addressed only THROUGH its gateway
//! (`/gateways/{gatewayIdentifier}/rules/{ruleId}`), so the create and every
//! later read answer from this one shape and cannot drift.

use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, Condition, CreateGatewayRuleOutput, GetGatewayRuleOutput, SystemManagedBlock,
};

use super::timestamp;

/// `GatewayRuleStatus` members this service drives. The enum's full value list
/// is `CREATING | ACTIVE | UPDATING | DELETING`
/// (`smithy_query shape=GatewayRuleStatus`).
pub const GATEWAY_RULE_STATUS_CREATING: &str = "CREATING";
pub const GATEWAY_RULE_STATUS_ACTIVE: &str = "ACTIVE";

/// How long a freshly created rule stays in its transient status (`CREATING`)
/// before the clock owes it `ACTIVE`.
///
/// A rule is a child of the gateway that evaluates it, so it settles on the same
/// span the gateway family uses ([`super::GATEWAY_SETTLE_DELAY`]): one chosen
/// span for the whole family rather than a second one only this resource would
/// wait out.
pub const GATEWAY_RULE_SETTLE_DELAY: Duration = super::GATEWAY_SETTLE_DELAY;

/// The resource noun [`super::resource_not_found`] names for one rule of a
/// gateway.
pub const RESOURCE_GATEWAY_RULE: &str = "rule";

/// How many rules one gateway will hold.
///
/// `CreateGatewayRule` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's per-gateway quota. The number is NOT a fidelity
/// claim about that quota: it is the emulator's own ceiling, set generously so
/// no realistic workflow meets it, so that a caller looping on create cannot
/// grow this process's memory without bound.
pub const MAX_RULES_PER_GATEWAY: usize = 1_000;

/// One rule configured on a gateway: what the caller submitted for it, and the
/// transition to `ACTIVE` the clock owes it.
///
/// Every operation that reports a rule renders it from this record, so a create
/// and a later read cannot disagree about its priority, its conditions, its
/// actions or its status.
#[derive(Debug, Clone)]
pub struct GatewayRuleRecord {
    pub rule_id: String,
    pub priority: i32,
    /// The conditions the caller submitted, absent for a rule that matches every
    /// request — `conditions` is optional on the create and optional on every
    /// response, so a rule created without any reports none.
    pub conditions: Option<Vec<Condition>>,
    pub actions: Vec<Action>,
    pub description: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status until [`settles_at`](Self::settles_at) passes.
    pub transient_status: String,
    /// The status from [`settles_at`](Self::settles_at) onwards.
    pub settled_status: String,
    /// The instant the transient status is owed its transition. Stored, never
    /// slept on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// The system or process that manages this rule, for a rule an automated
    /// process created; `None` for one a caller created.
    ///
    /// `system` is "System-managed metadata for rules created by automated
    /// processes" (`smithy_query doc shape=CreateGatewayRuleResponse
    /// member=system`), and no operation this service models lets a caller
    /// supply it, so a rule a caller created carries none.
    pub managed_by: Option<String>,
}

impl GatewayRuleRecord {
    /// The record a create stores for one submitted rule, under the id the
    /// create minted for it.
    pub fn written(
        rule_id: String,
        priority: i32,
        conditions: Option<Vec<Condition>>,
        actions: Vec<Action>,
        description: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            rule_id,
            priority,
            conditions,
            actions,
            description,
            created_at: now,
            updated_at: now,
            // A new rule is created, not active: the transition to ACTIVE is
            // owed by the clock, not by this call.
            transient_status: GATEWAY_RULE_STATUS_CREATING.to_string(),
            settled_status: GATEWAY_RULE_STATUS_ACTIVE.to_string(),
            settles_at: now + GATEWAY_RULE_SETTLE_DELAY,
            // Nothing but an automated process manages a rule, and no operation
            // this service models lets a caller claim one.
            managed_by: None,
        }
    }

    /// The status this rule reports at `now`: the transient one until the
    /// transition the clock owes comes due, the settled one afterwards.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The `system` block this rule reports, for a rule an automated process
    /// manages, and `None` for one a caller created.
    pub fn system(&self) -> Option<SystemManagedBlock> {
        self.managed_by
            .as_ref()
            .map(|managed_by| SystemManagedBlock {
                managed_by: managed_by.clone(),
            })
    }

    /// This rule as `CreateGatewayRule`'s response envelope. `gateway_arn` is the
    /// owning gateway's, which the response reports and the rule itself does not
    /// store twice.
    pub fn create_output(&self, gateway_arn: &str, now: SystemTime) -> CreateGatewayRuleOutput {
        CreateGatewayRuleOutput {
            rule_id: self.rule_id.clone(),
            gateway_arn: gateway_arn.to_string(),
            priority: self.priority,
            conditions: self.conditions.clone(),
            actions: self.actions.clone(),
            description: self.description.clone(),
            created_at: timestamp(self.created_at),
            status: self.status_at(now).to_string(),
            system: self.system(),
        }
    }

    /// This rule as `GetGatewayRule`'s response envelope — the same members
    /// [`Self::create_output`] renders, read off the same record, plus the
    /// `updatedAt` only a read declares, so a read cannot differ from the create
    /// that stored it.
    pub fn get_output(&self, gateway_arn: &str, now: SystemTime) -> GetGatewayRuleOutput {
        let created = self.create_output(gateway_arn, now);
        GetGatewayRuleOutput {
            rule_id: created.rule_id,
            gateway_arn: created.gateway_arn,
            priority: created.priority,
            conditions: created.conditions,
            actions: created.actions,
            description: created.description,
            created_at: created.created_at,
            status: created.status,
            system: created.system,
            updated_at: Some(timestamp(self.updated_at)),
        }
    }
}
