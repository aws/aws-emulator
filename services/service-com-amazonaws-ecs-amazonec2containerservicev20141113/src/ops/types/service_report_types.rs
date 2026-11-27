// SPDX-License-Identifier: CC0-1.0

//! `Service` and `Deployment`: the shapes every service write and read answers with.
//!
//! These are the ONE `Service` and `Deployment` types of this crate — the shapes
//! `CreateService`, `UpdateService`, `DeleteService` and `DescribeServices` report a service
//! and its deployments through — so what they declare is what every one of those operations
//! answers with.
//!
//! They are declared here rather than beside the other shared shapes because Amazon ECS
//! answers with members the published model snapshot does not declare, and that snapshot is
//! kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `Service$version`, `Service$runStatus` and `Service$extendDeploymentTimePeriod`;
//! * `Deployment$healthyTaskCount`, `Deployment$failedLaunchTaskCount`,
//!   `Deployment$replacedTaskCount`, `Deployment$totalFailedTasks`, `Deployment$scale` and
//!   `Deployment$stabilityStatus`.
//!
//! So the types are generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the members the
//! snapshot declares, with their own targets so every scalar's type, every list's element
//! and every `@default` is captured exactly as the snapshot spells it, and adds the ones
//! above. Nothing about the wire is hand-written: the member names, the shape ids and the
//! traits all come from that file.
//!
//! The values come from the one projection every path that reports a service runs
//! (`crate::state`'s service and deployment views), so a create, an update and a read
//! cannot report different values for one service.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#Service")]
#[derive(Debug, Clone, Default)]
pub struct Service {
    pub service_arn: Option<String>,
    pub service_name: Option<String>,
    pub cluster_arn: Option<String>,
    pub load_balancers: Option<Vec<LoadBalancer>>,
    pub service_registries: Option<Vec<ServiceRegistry>>,
    pub status: Option<String>,
    pub desired_count: i32,
    pub running_count: i32,
    pub pending_count: i32,
    pub launch_type: Option<String>,
    pub capacity_provider_strategy: Option<Vec<CapacityProviderStrategyItem>>,
    pub platform_version: Option<String>,
    pub platform_family: Option<String>,
    pub task_definition: Option<String>,
    pub deployment_configuration: Option<DeploymentConfiguration>,
    pub task_sets: Option<Vec<TaskSet>>,
    pub deployments: Option<Vec<Deployment>>,
    pub role_arn: Option<String>,
    pub events: Option<Vec<ServiceEvent>>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub current_service_deployment: Option<String>,
    pub current_service_revisions: Option<Vec<ServiceCurrentRevisionSummary>>,
    pub placement_constraints: Option<Vec<PlacementConstraint>>,
    pub placement_strategy: Option<Vec<PlacementStrategy>>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub health_check_grace_period_seconds: Option<i32>,
    pub scheduling_strategy: Option<String>,
    pub deployment_controller: Option<DeploymentController>,
    pub tags: Option<Vec<Tag>>,
    pub created_by: Option<String>,
    pub enable_ecs_managed_tags: bool,
    pub propagate_tags: Option<String>,
    pub enable_execute_command: bool,
    pub availability_zone_rebalancing: Option<String>,
    pub resource_management_type: Option<String>,
    /// The service's own version counter. Not in the snapshot: every recorded service read
    /// and write carries it.
    pub version: i64,
    /// What the scheduler still owes the service. Not in the snapshot: the recorded service
    /// reads and writes carry it for a service the scheduler has run.
    pub run_status: Option<String>,
    /// The extra time period the service's current deployment has been granted. Not in the
    /// snapshot: every recorded service read and write carries it.
    pub extend_deployment_time_period: i32,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ecs#Deployment")]
#[derive(Debug, Clone, Default)]
pub struct Deployment {
    pub id: Option<String>,
    pub status: Option<String>,
    pub task_definition: Option<String>,
    pub desired_count: i32,
    pub pending_count: i32,
    pub running_count: i32,
    pub failed_tasks: i32,
    /// The deployment's tasks that count as healthy. Not in the snapshot: every recorded
    /// deployment carries it.
    pub healthy_task_count: i32,
    /// The tasks of this deployment that never reached `RUNNING`. Not in the snapshot:
    /// every recorded deployment carries it.
    pub failed_launch_task_count: i32,
    /// The tasks of an earlier deployment this one replaced. Not in the snapshot: every
    /// recorded deployment carries it.
    pub replaced_task_count: i32,
    /// Every task this deployment lost while it still wanted it running. Not in the
    /// snapshot: every recorded deployment carries it.
    pub total_failed_tasks: i32,
    /// The share of the service's desired count this deployment is scaled to, as the
    /// percentage `Scale` measures. Not in the snapshot: every recorded deployment carries
    /// it.
    pub scale: Option<Scale>,
    /// Whether the deployment has reached a steady state. Not in the snapshot: every
    /// recorded deployment carries it.
    pub stability_status: Option<String>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub updated_at: Option<::aws_smithy_types::DateTime>,
    pub capacity_provider_strategy: Option<Vec<CapacityProviderStrategyItem>>,
    pub launch_type: Option<String>,
    pub platform_version: Option<String>,
    pub platform_family: Option<String>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub rollout_state: Option<String>,
    pub rollout_state_reason: Option<String>,
    pub service_connect_configuration: Option<ServiceConnectConfiguration>,
    pub service_connect_resources: Option<Vec<ServiceConnectServiceResource>>,
    pub volume_configurations: Option<Vec<ServiceVolumeConfiguration>>,
    pub fargate_ephemeral_storage: Option<DeploymentEphemeralStorage>,
    pub vpc_lattice_configurations: Option<Vec<VpcLatticeConfiguration>>,
}
