// SPDX-License-Identifier: CC0-1.0

//! `TaskDefinition`: the shape every task-definition write and read answers with.
//!
//! This is the ONE `TaskDefinition` type of this crate — the shape `RegisterTaskDefinition`,
//! `DescribeTaskDefinition`, `DeregisterTaskDefinition` and `DeleteTaskDefinitions` report a
//! revision through — so what it declares is what every one of those operations answers
//! with.
//!
//! It is declared here rather than beside the other shared shapes because Amazon ECS answers
//! the calls that MOVE a revision's status with one member the published model snapshot does
//! not declare, and that snapshot is kept as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `previousStatus` — the status the revision held when the call arrived, beside the
//!   `status` it holds now.
//!
//! So the type is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every scalar's type, every list's element and every
//! `@default` is captured exactly as the snapshot spells it, and adds that one. Nothing
//! about the wire is hand-written: the member names, the shape id and the traits all come
//! from that file.
//!
//! The value comes from `crate::state`'s own projection of a revision
//! (`TaskDefinitionRecord::transition_view`), so a call that moved nothing reports none.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#TaskDefinition")]
#[derive(Debug, Clone, Default)]
pub struct TaskDefinition {
    pub task_definition_arn: Option<String>,
    pub container_definitions: Option<Vec<ContainerDefinition>>,
    pub family: Option<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub network_mode: Option<String>,
    pub revision: i32,
    pub volumes: Option<Vec<Volume>>,
    pub status: Option<String>,
    pub requires_attributes: Option<Vec<Attribute>>,
    pub placement_constraints: Option<Vec<TaskDefinitionPlacementConstraint>>,
    pub compatibilities: Option<Vec<String>>,
    pub runtime_platform: Option<RuntimePlatform>,
    pub requires_compatibilities: Option<Vec<String>>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub inference_accelerators: Option<Vec<InferenceAccelerator>>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub proxy_configuration: Option<ProxyConfiguration>,
    pub registered_at: Option<::aws_smithy_types::DateTime>,
    pub deregistered_at: Option<::aws_smithy_types::DateTime>,
    pub delete_requested_at: Option<::aws_smithy_types::DateTime>,
    pub registered_by: Option<String>,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub enable_fault_injection: Option<bool>,
    pub trusted_execution_configuration: Option<TrustedExecutionConfiguration>,
    /// The status the revision held before the call that answers with it. Not in the
    /// snapshot: the recorded deregistrations and deletions carry it, and an open string
    /// because nothing publishes a value set for it.
    pub previous_status: Option<String>,
}
