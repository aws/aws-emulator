// SPDX-License-Identifier: CC0-1.0

//! `ManagedScaling`: the managed-scaling settings of an Auto Scaling group capacity
//! provider.
//!
//! This is the ONE `ManagedScaling` type of this crate — the shape `CreateCapacityProvider`
//! and `UpdateCapacityProvider` parse, and `DescribeCapacityProviders` reports, the settings
//! through.
//!
//! It is declared here rather than beside the other shared shapes because Amazon ECS answers
//! with one member the published model snapshot does not declare, and that snapshot is kept
//! as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `scalingApproach` — how Amazon ECS scales the group, which every recorded provider over
//!   a caller's Auto Scaling group reports.
//!
//! So the type is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every `@range` is captured exactly as the snapshot
//! spells it, and adds that one. Nothing about the wire is hand-written: the member names,
//! the shape id and the traits all come from that file.
//!
//! The value comes from the one rule every path that stores a provider runs
//! (`crate::state::stored_managed_scaling`), so a create, an update and a read cannot report
//! different settings for one provider.
//!
//! `skip_validation = ["targetCapacity"]`: the bound on that member (`@range` 1..100 on
//! `ManagedScalingTargetCapacity`) is judged by the OPERATIONS that take a managed-scaling
//! block (`crate::state::managed_scaling_target_capacity_violation`), because Amazon ECS
//! answers a utilization outside it in the class it answers a request whose VALUES it will
//! not take — `ClientException`, with one fixed sentence of its own — rather than in the
//! class the framework's boundary walk answers for a modelled bound. The bound is still
//! enforced, there; the two step sizes' own ranges stay with the walk, which is the class
//! they are refused in.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.ecs#ManagedScaling",
    skip_validation = ["targetCapacity"]
)]
#[derive(Debug, Clone, Default)]
pub struct ManagedScaling {
    pub status: Option<String>,
    pub target_capacity: Option<i32>,
    pub minimum_scaling_step_size: Option<i32>,
    pub maximum_scaling_step_size: Option<i32>,
    pub instance_warmup_period: Option<i32>,
    /// How Amazon ECS scales the group. Not in the snapshot: every recorded provider over a
    /// caller's Auto Scaling group reports it, and an open string because nothing publishes
    /// a value set for it.
    pub scaling_approach: Option<String>,
}
