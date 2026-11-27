// SPDX-License-Identifier: CC0-1.0

//! `Task`: the shape every task write and read answers with.
//!
//! This is the ONE `Task` type of this crate — the shape `RunTask`, `StartTask`, `StopTask`
//! and `DescribeTasks` report a task through — so what it declares is what every one of
//! those operations answers with.
//!
//! It is declared here rather than beside the other shared shapes because Amazon ECS answers
//! with two members the published model snapshot does not declare, and that snapshot is kept
//! as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `provisionedCpu` and `provisionedMemory` — the CPU-unit and MiB strings the task was
//!   provisioned with.
//!
//! So the type is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every scalar's type, every list's element and every
//! `@default` is captured exactly as the snapshot spells it, and adds the two. Nothing about
//! the wire is hand-written: the member names, the shape id and the traits all come from
//! that file.
//!
//! The values come from `crate::state`'s own projection of a task record, so every operation
//! that reports a task reports the same ones.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#Task")]
#[derive(Debug, Clone, Default)]
pub struct Task {
    pub attachments: Option<Vec<Attachment>>,
    pub attributes: Option<Vec<Attribute>>,
    pub availability_zone: Option<String>,
    pub capacity_provider_name: Option<String>,
    pub cluster_arn: Option<String>,
    pub connectivity: Option<String>,
    pub connectivity_at: Option<::aws_smithy_types::DateTime>,
    pub container_instance_arn: Option<String>,
    pub containers: Option<Vec<Container>>,
    pub cpu: Option<String>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub desired_status: Option<String>,
    pub enable_execute_command: bool,
    pub execution_stopped_at: Option<::aws_smithy_types::DateTime>,
    pub group: Option<String>,
    pub health_status: Option<String>,
    pub inference_accelerators: Option<Vec<InferenceAccelerator>>,
    pub last_status: Option<String>,
    pub launch_type: Option<String>,
    pub memory: Option<String>,
    pub overrides: Option<TaskOverride>,
    pub platform_version: Option<String>,
    pub platform_family: Option<String>,
    /// The CPU units the task was provisioned with. Not in the snapshot: the recorded task
    /// reads carry it.
    pub provisioned_cpu: Option<String>,
    /// The memory, in MiB, the task was provisioned with. Not in the snapshot: the recorded
    /// task reads carry it.
    pub provisioned_memory: Option<String>,
    pub pull_started_at: Option<::aws_smithy_types::DateTime>,
    pub pull_stopped_at: Option<::aws_smithy_types::DateTime>,
    pub started_at: Option<::aws_smithy_types::DateTime>,
    pub started_by: Option<String>,
    pub stop_code: Option<String>,
    pub stopped_at: Option<::aws_smithy_types::DateTime>,
    pub stopped_reason: Option<String>,
    pub stopping_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Option<Vec<Tag>>,
    pub task_arn: Option<String>,
    pub task_definition_arn: Option<String>,
    pub version: i64,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub fargate_ephemeral_storage: Option<TaskEphemeralStorage>,
    pub trusted_execution_summary: Option<TrustedExecutionSummary>,
}
