// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecs-amazonec2containerservicev20141113 service.
//!
//! The ECS task lifecycle, held in one place: clusters, container instances,
//! task-definition revisions, services and tasks. Every handler computes its response
//! from what is stored here — the cluster counters (`runningTasksCount`,
//! `pendingTasksCount`, `registeredContainerInstancesCount`, `activeServicesCount`) and
//! a service's own (`runningCount`, `pendingCount`) are DERIVED from the task,
//! container-instance and service maps rather than being bumped by whoever happened to
//! call, so they cannot drift from the records that actually exist.
//!
//! Resource ids are minted from monotonic counters held here, so every ARN this
//! service emits is both unique and recorded against the record it names.
//!
//! ## The clock-owed transitions
//!
//! `RunTask` answers with a task in `PENDING` — the status AWS reports for a task that
//! has been accepted but has not started yet (the `Task$createdAt` doc: "the time when
//! the task entered the `PENDING` state"). The move to `RUNNING` is owed by the CLOCK,
//! not by whoever reads next: [`TaskRecord::running_due_at`] records the instant it
//! falls due and [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]
//! materialises it.
//!
//! A task whose placement still needs capacity ACQUIRED for it is one step earlier in
//! that lifecycle: it is answered in `PROVISIONING` with no start owed at all (see
//! [`awaits_provisioned_capacity`]), and it crosses `PROVISIONING` -> `PENDING` only once
//! a container instance registers into its cluster —
//! [`ComAmazonawsEcsAmazonec2containerservicev20141113State::place_tasks_awaiting_capacity`]
//! records that instant in [`TaskRecord::pending_due_at`], and the same clock pass
//! materialises it.
//!
//! A task that is allocated its own elastic network interface (the `awsvpc` network mode)
//! is answered in `PROVISIONING` for a different reason: Amazon ECS has to create and
//! attach that interface before it can launch the task, which the clock does one
//! documented step at a time ([`INTERFACE_ATTACH_LIFECYCLE`]). Such a task is placed at the
//! instant its interface is in place ([`placement_schedule`]), and that instant is also
//! when it goes `CONNECTED` ([`TaskRecord::report_connected`]).
//!
//! `StopTask` is the same shape in reverse: the task goes to
//! `DEACTIVATING` with `stoppingAt` set (the `Task$stoppingAt` doc: "the time when the
//! task transitions from the `RUNNING` state to `STOPPING`") and reaches `STOPPED`
//! [`TRANSITION_SPAN`] later. Nothing sleeps: the due instant is stored and compared.
//!
//! A capacity provider Amazon ECS has to stand capacity UP for — a Managed Instances
//! provider, which runs EC2 instances on the caller's behalf — is answered in
//! [`CAPACITY_PROVIDER_PROVISIONING`] for the same reason, and its move to
//! [`CAPACITY_PROVIDER_ACTIVE`] with the create [`CAPACITY_PROVIDER_CREATE_COMPLETE`] is
//! owed by the clock ([`CapacityProviderRecord::active_due_at`]). A provider over an Auto
//! Scaling group the caller already has stands nothing up and is `ACTIVE` at once.
//!
//! A service's task set is placed by the CALL that changes what the service wants
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service`], driven by
//! `CreateService` and `UpdateService`), and each task it places then crosses `PENDING`
//! -> `RUNNING` on the same clock. A service whose deployment controller is not the ECS
//! one takes its tasks from a TASK SET instead
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_task_set`], driven by
//! `CreateTaskSet`), placed the same way off the same clock. Where the capacity a
//! placement needs does not exist, nothing is placed and the scheduler records WHY on the
//! service's event stream
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::placement_capacity_reason`]);
//! the desired count is the caller's and does not move because the scheduler could not
//! meet it. Keeping that count is CONTINUOUS work, not work one call does: a service that
//! holds fewer tasks than it wants — because one of its tasks was stopped, failed or ended
//! — has the shortfall placed by the clock pass
//! ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::maintain_service_task_counts`],
//! see [`service_steady_state`]), which is why a stopped SERVICE task is replaced and a
//! stopped STANDALONE task is not. The scheduler's steady-state work beyond that —
//! re-placing a daemon's task when a container instance registers later — is not modelled.
//!
//! ## The containers that RUN
//!
//! A task that reaches `RUNNING` is a task whose containers are running: the program each
//! container's `entryPoint`/`command` names is performed in this process
//! ([`container_shell`]), what it writes goes to the log driver its definition configures
//! ([`container_logs`]) and the status it ends with is that container's `exitCode`
//! ([`container_runtime`]). `StopTask` sends such a container the stop signal, which is
//! what its own `trap` handler answers. A container whose launch names no command at all
//! runs nothing: the image's own entry point is what would run, and this emulator holds no
//! image, so no output and no exit code is invented for it.
//!
//! WHAT "performed in this process" IS, and the boundary it keeps: the program's grammar is
//! read by the `brush-parser` crate, which builds a syntax tree and runs nothing, and the
//! tree is then walked by [`container_shell`]'s OWN session, which PERFORMS a fixed catalog
//! of session-local utilities whose whole effect is inside that session — its output, its
//! status, its variables and its signal handlers. It starts no host process (the executing
//! half of that project, `brush-core`, is deliberately not a dependency of this crate),
//! resolves no command name against the host's `PATH`, opens no host file and reaches no
//! network; a command naming anything outside the catalog, and every form and expansion the
//! session does not perform, fails closed with the status a shell answers for a command it
//! cannot run. Every bound on one run is held in code the program cannot reach: a step
//! budget, the deadline its session is constructed with (which also clamps every wait), a
//! nesting limit, a limit on what one word may expand to and a cap on the output it may
//! deliver. See that module's own documentation for each of them.
//!
//! ## The rollouts that WAIT
//!
//! A rollout whose deployment configuration carries a `PAUSE` lifecycle hook does not run
//! to its end on its own: `ContinueServiceDeployment` is what takes it on. Travelling to
//! the stage it pauses at is itself a span the clock settles
//! ([`ServiceDeploymentRecord::hook_due_at`]), and so is the end the hook action takes it
//! to ([`ServiceDeploymentRecord::complete_due_at`] for a `CONTINUE`,
//! [`ServiceDeploymentRecord::rollback_due_at`] for a `ROLLBACK`), and the clock pass
//! `advance_service_deployments` materialises each of them. A hook whose target type
//! invokes a Lambda function pauses nothing here: no function is invoked, so no rollout is
//! reported as waiting on work that is not being done.
//!
//! ## The transitions that are PUBLISHED
//!
//! Every status a task moves into is also an EventBridge event: `task_events` compares what
//! this store holds against what was last published and puts an `ECS Task State Change`
//! onto the account's default bus for each move, so a client can watch a task without
//! calling Amazon ECS again.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Attachment, Attribute, AutoScalingGroupProvider, AwsVpcConfiguration, CapacityProvider,
    CapacityProviderStrategyItem, Cluster, ClusterConfiguration, ClusterServiceConnectDefaults,
    ClusterSetting, Container, ContainerDefinition, ContainerDependency, ContainerImage,
    ContainerInstance,
    DaemonContainerDefinition,
    DaemonDeploymentConfiguration, DaemonSummary,
    DaemonTaskDefinition, DaemonTaskDefinitionSummary, DaemonVolume, Defaulted, Deployment,
    DeploymentCircuitBreaker, DeploymentConfiguration, DeploymentController,
    DeploymentLifecycleHookDetail,
    ECSExpressGatewayService, EphemeralStorage, ExpressGatewayContainer,
    ExpressGatewayScalingTarget, ExpressGatewayServiceConfiguration,
    ExpressGatewayServiceNetworkConfiguration, ExpressGatewayServiceStatus,
    Failure, HealthCheck, KeyValuePair, LoadBalancer, ManagedAgent, ManagedInstancesProvider,
    NetworkBinding,
    NetworkConfiguration,
    PlacementConstraint, PlacementStrategy, PlatformDevice, PortMapping, ProtectedTask,
    ProxyConfiguration,
    ResolvedConfiguration, Resource, Rollback, RuntimePlatform, Scale, Service,
    ServiceConnectConfiguration,
    ServiceCurrentRevisionSummary, ServiceDeployment, ServiceDeploymentAlarms,
    ServiceDeploymentBrief, ServiceDeploymentCircuitBreaker, ServiceEvent, ServiceRegistry,
    ServiceRevision, ServiceRevisionLoadBalancer, ServiceRevisionSummary,
    ServiceRevisionVpcLatticeConfiguration,
    ServiceVolumeConfiguration, Setting, Tag, Task, TaskDefinition,
    TaskDefinitionPlacementConstraint, TaskOverride, TaskSet,
    ThresholdConfiguration, UpdatedExpressGatewayService, VersionInfo, Volume,
    VpcLatticeConfiguration,
    CLUSTER_SETTING_NAME_VALUES,
};

/// How a `cluster` an operation cannot act in is WORDED: the two sentences behind the one
/// error code, and the function that chooses between them off the stored cluster record.
mod cluster_refusal;
pub use cluster_refusal::{
    cluster_not_active_message, cluster_refusal_message, names_an_inactive_cluster,
    CLUSTER_INACTIVE_FOR_TAGGING_MESSAGE, CLUSTER_INACTIVE_MESSAGE,
};

/// How an ARN a REQUEST supplies is read, and what it is refused for: the Region and the
/// account segments checked before the identifier is resolved.
mod request_arn;

/// The records a TASK is made of — its containers and the managed agents each of them
/// runs — and the projections they are reported through.
mod records;
pub use records::*;

/// What a `serviceRegistries` entry has to name: the rules the registry's own Cloud Map
/// DNS record type and the revision's network mode put on it, and the Cloud Map read each
/// one is decided against.
mod validation;
pub use validation::*;

/// The elastic network interface an `awsvpc` task is allocated: what it carries, and when
/// it is reported as attached.
mod task_interfaces;
pub use task_interfaces::*;

/// Whether a launched task can fetch the images its containers run from where it was
/// placed — read off the Amazon VPC networking the launch named — and the stop a task that
/// cannot earns.
mod image_pull;
pub use image_pull::*;

/// Whether Amazon ECS can assume the task execution role a launched task names, and the
/// sentence a task stopped because it cannot carries — the one reading of such a role's own
/// trust policy in the account's IAM records.
mod execution_role_trust;
pub use execution_role_trust::*;

/// WHICH roles a launch has Amazon ECS assume, the stop a task whose role it cannot assume
/// earns, and the one pass both halves of a launch are settled in.
mod launch_role_trust;
pub use launch_role_trust::*;

/// The Amazon VPC networking an `awsvpc` placement names, resolved against the task
/// definition it places: what a launch of such a revision has to say, and what a launch of
/// any other revision may not.
mod vpc_resolution;
pub use vpc_resolution::*;

/// The Auto Scaling group a capacity provider binds onto, resolved against Amazon EC2 Auto
/// Scaling's own records.
mod auto_scaling_groups;
pub use auto_scaling_groups::*;

/// The Cloud Map instances Amazon ECS registers for the tasks of a service that names a
/// `serviceRegistries` entry.
mod service_discovery;
pub use service_discovery::*;

/// The SCOPE a request's references have to be inside: the cluster an ARN of a
/// cluster-scoped resource carries, and what the call that sets a cluster's capacity-provider
/// association may leave behind.
mod resource_scoping;
pub use resource_scoping::*;

/// What a `Submit*StateChange` report may say, and whose report it can be.
mod agent_plane;
pub use agent_plane::*;

pub use request_arn::{
    foreign_arn_segment, is_cluster_arn, names_another_account, names_another_region,
    request_arn_resource_violation, request_arn_violation, ForeignArnSegment,
    ACCOUNT_IDS_MISMATCH_MESSAGE, INVALID_ARN_MESSAGE, INVALID_REGION_IN_ARN_MESSAGE,
};

/// The records a SERVICE is made of — the service itself, the deployments and service
/// revisions its rollouts mint, and the express-gateway configuration a managed service
/// carries — and the projections each of them is reported through.
mod service_records;
pub use service_records::*;

/// What a REQUEST is refused for, and the wording of each refusal: the cross-field rules
/// no model can express, the resource-not-found sentences, and the derived counters a
/// refusal is weighed against.
mod refusals;
pub use refusals::*;

/// The task sizes Amazon ECS validates a revision and its overrides against: the
/// documented Fargate `cpu`/`memory` catalog, and how a `cpu` or `memory` value is read
/// as the number of units it stands for.
mod task_size;
pub use task_size::*;

/// Which Fargate platform version a task of a given revision could be placed on: the
/// versions are published per operating system, so the pairing is read off the revision.
mod platform_versions;
pub use platform_versions::*;

/// The bounds a service's deployment has to meet: the two percentages that bound the same
/// count from opposite ends, and the grace period the scheduler waits out.
mod deployment_bounds;
pub use deployment_bounds::*;

/// What a PLACEMENT may override on the revision it runs: each container override
/// resolved against the stored container definitions, and the task-level overrides
/// weighed against the task size they replace.
mod task_overrides;
pub use task_overrides::*;

/// The whole in-memory store: every operation's reads and writes against the maps below.
mod store;

/// The deployment circuit breaker: the failed launches it counts for a rollout, the
/// terminal `FAILED` it moves that rollout to, and the rollback it starts.
mod circuit_breaker;
pub use circuit_breaker::*;

/// The Elastic Load Balancing v2 targets a load-balanced service's RUNNING tasks occupy,
/// and the drain a stopped task's target leaves through.
mod load_balancer_targets;
pub use load_balancer_targets::*;

/// The CLASSIC load balancer a `loadBalancers` entry attaches a service to: whether the
/// account holds it, and the network mode that kind of attachment can reach a task in.
mod classic_load_balancers;
pub use classic_load_balancers::*;

/// The `ECS Task State Change` events this service publishes as a task moves, and the
/// clock pump that publishes the transitions nobody is waiting on a call for.
mod task_events;
pub use task_events::*;

/// What a container's `command` DOES: the session one container's program runs in, and the
/// output and exit status running it produces.
mod container_shell;
pub use container_shell::*;

/// The `awslogs` log driver: where the output of a container that runs is delivered.
mod container_logs;
pub use container_logs::*;

/// The cross-field rules a registration's CONTAINER DEFINITIONS have to meet: the members
/// whose values the agent fetches under the task execution role, the log driver that needs
/// a log router beside it, and a resource limit whose halves bound each other.
mod container_credentials;
pub use container_credentials::*;

/// The container data plane: which containers are running, what signal a stop sends them,
/// and the exit code a run records.
mod container_runtime;
pub use container_runtime::*;

/// The container HEALTH CHECK: the probe a container definition declares, what running it
/// makes of `healthStatus`, and the containers whose start waits on it.
mod container_health;
pub use container_health::*;

/// The service scheduler's steady state: the replacement a service's stopped task earns,
/// and the standalone task that earns none.
mod service_steady_state;

/// The capacity an Amazon EC2 instance launched into a cluster gives it: the registration
/// the container agent on that instance makes.
mod ec2_capacity;
pub use ec2_capacity::*;

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as the model's
/// `aws.api#service` trait records it (`arnNamespace: "ecs"`).
pub const ARN_NAMESPACE: &str = "ecs";

/// The cluster an operation acts on when the request names none. The `CreateCluster`
/// `clusterName` doc: "If you don't specify a name for your cluster, you create a
/// cluster that's named default"; every other operation's `cluster` doc: "If you do not
/// specify a cluster, the default cluster is assumed."
pub const DEFAULT_CLUSTER_NAME: &str = "default";

/// The value a cluster setting takes when neither the cluster nor the account set one.
/// `ClusterSetting$value`: "The supported values are `enhanced`, `enabled`, and
/// `disabled`. ... If a cluster value is specified, it will override the
/// `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault" —
/// and CloudWatch Container Insights is OFF until one of those turns it on, so the
/// account default a cluster inherits here is `disabled`.
///
/// It is the SERVICE default for the `containerInsights` account setting (see
/// [`ACCOUNT_SETTINGS`]): a cluster that set nothing reports the account setting in
/// effect, which falls back to this value until `PutAccountSetting` or
/// `PutAccountSettingDefault` stores one — see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_effective_settings`].
pub const CLUSTER_SETTING_DEFAULT_VALUE: &str = "disabled";

/// One Amazon ECS ACCOUNT SETTING: the name `SettingName` defines, the value in effect
/// until somebody stores one, who manages it (`Setting$type`), and the values a write
/// of it accepts.
#[derive(Debug, Clone, Copy)]
pub struct AccountSettingSpec {
    /// The setting name, one of `SettingName`
    /// (`smithy_query(query="shape", shape="SettingName")`).
    pub name: &'static str,
    /// The value this setting has for an account that has never written it.
    pub default_value: &'static str,
    /// `Setting$type`: "Indicates whether Amazon Web Services manages the account
    /// setting, or if the user manages it" — one of `SettingType`.
    pub setting_type: &'static str,
    /// The values `PutAccountSetting`/`PutAccountSettingDefault` accept for this name,
    /// from the `value` member's doc.
    pub accepted_values: &'static [&'static str],
}

/// `Setting$type` for a setting the customer writes.
pub const SETTING_TYPE_USER: &str = "user";

/// `Setting$type` for a setting Amazon Web Services manages: "`aws_managed` account
/// settings are read-only, as Amazon Web Services manages such on the customer's
/// behalf. Currently, the `guardDutyActivate` account setting is the only one Amazon
/// Web Services manages."
pub const SETTING_TYPE_AWS_MANAGED: &str = "aws_managed";

/// The values a toggle account setting accepts. `PutAccountSetting$value`: "Accepted
/// values are `enabled`, `disabled`, `enhanced`, `on`, and `off`" — `enhanced` belongs
/// to `containerInsights` alone, which has its own entry below.
const SETTING_TOGGLE_VALUES: &[&str] = &["enabled", "disabled", "on", "off"];

/// The value an opt-in feature setting has until a write turns it on. Every such
/// setting's own doc in `PutAccountSetting$name` describes what changes "when
/// modified"/"if turned on", so the account starts out with the feature off.
const SETTING_OFF: &str = "disabled";

/// The value the ARN-and-resource-id-format settings report. Their shared doc in
/// `PutAccountSetting$name`: "You must turn on this setting to use Amazon ECS features
/// such as resource tagging" — and this emulator DOES tag every resource it creates and
/// emits the long ARN format for all of them, so the setting that behaviour requires is
/// on.
const SETTING_ON: &str = "enabled";

/// Every account setting this service has, keyed by the `SettingName` enum
/// (`smithy_query(query="shape", shape="SettingName")`). `ListAccountSettings` reports
/// one effective entry per row, and a write is checked against the row's
/// `accepted_values`.
///
/// A default_value is the value AWS documents for an account that never wrote the
/// setting; the citation is on each row.
pub const ACCOUNT_SETTINGS: &[AccountSettingSpec] = &[
    // "When modified, the Amazon Resource Name (ARN) and resource ID format of the
    // resource type ... is affected. ... You must turn on this setting to use Amazon ECS
    // features such as resource tagging."
    AccountSettingSpec {
        name: "serviceLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    AccountSettingSpec {
        name: "taskLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    AccountSettingSpec {
        name: "containerInstanceLongArnFormat",
        default_value: SETTING_ON,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "If `awsvpcTrunking` is turned on, any new container instances that support the
    // feature are launched have the increased ENI limits available to them" — an opt-in,
    // and this emulator applies no increased ENI limit to a registered instance.
    AccountSettingSpec {
        name: "awsvpcTrunking",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "To use Container Insights with enhanced observability, set the
    // `containerInsights` account setting to `enhanced`. To use Container Insights, set
    // the `containerInsights` account setting to `enabled`" — so the three values
    // `ClusterSetting$value` documents, off by default
    // ([`CLUSTER_SETTING_DEFAULT_VALUE`]).
    AccountSettingSpec {
        name: "containerInsights",
        default_value: CLUSTER_SETTING_DEFAULT_VALUE,
        setting_type: SETTING_TYPE_USER,
        accepted_values: &CLUSTER_SETTING_VALUES,
    },
    // Fargate FIPS-140 compliance is an opt-in an account turns on; this emulator serves
    // no FIPS endpoint.
    AccountSettingSpec {
        name: "fargateFIPSMode",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "Amazon ECS is introducing tagging authorization for resource creation. ...
    // Therefore, you must grant explicit permissions to use the `ecs:TagResource`
    // action" — an opt-in, and this emulator runs no extra tag authorization.
    AccountSettingSpec {
        name: "tagResourceAuthorization",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // `PutAccountSetting$value`: "When you specify `fargateTaskRetirementWaitPeriod` for
    // the `name`, the following are the valid values: `0` ... `7` ... `14`". The wait
    // Amazon Web Services applies until an account changes it is 7 calendar days
    // ("Amazon Web Services Fargate task maintenance", the doc this member links).
    AccountSettingSpec {
        name: "fargateTaskRetirementWaitPeriod",
        default_value: "7",
        setting_type: SETTING_TYPE_USER,
        accepted_values: &["0", "7", "14"],
    },
    // "The `guardDutyActivate` parameter is read-only in Amazon ECS and indicates
    // whether Amazon ECS Runtime Monitoring is enabled or disabled by your security
    // administrator ... Amazon GuardDuty controls this account setting on your behalf" —
    // so it is `aws_managed`, and nothing in this emulator turns Runtime Monitoring on.
    AccountSettingSpec {
        name: "guardDutyActivate",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_AWS_MANAGED,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
    // "The `defaultLogDriverMode` setting supports two values: `blocking` and
    // `non-blocking`. ... On June 25, 2025, Amazon ECS changed the default log driver
    // mode from `blocking` to `non-blocking`".
    AccountSettingSpec {
        name: "defaultLogDriverMode",
        default_value: "non-blocking",
        setting_type: SETTING_TYPE_USER,
        accepted_values: &["blocking", "non-blocking"],
    },
    // "Use `fargateEventWindows` to use EC2 Event Windows associated with Fargate tasks
    // to configure time windows for task retirement" — an opt-in, and this emulator
    // models no event window.
    AccountSettingSpec {
        name: "fargateEventWindows",
        default_value: SETTING_OFF,
        setting_type: SETTING_TYPE_USER,
        accepted_values: SETTING_TOGGLE_VALUES,
    },
];

/// The row [`ACCOUNT_SETTINGS`] holds for `name`, or `None` when the name is not an
/// account setting. A `name` member that reached a handler targets the closed
/// `SettingName` enum, so `None` means this catalog is missing a row the model defines —
/// which the crate's tests pin.
pub fn account_setting_spec(name: &str) -> Option<&'static AccountSettingSpec> {
    ACCOUNT_SETTINGS
        .iter()
        .find(|setting| setting.name == name)
}

/// The largest `maxResults` `ListAccountSettings` accepts, and the page size it uses
/// when the request gives none. That member's doc: "This value can be between 1 and 10.
/// If this parameter isn't used, then ListAccountSettings returns up to 10 results and a
/// nextToken value if applicable."
pub const ACCOUNT_SETTINGS_PAGE_SIZE: usize = 10;

/// How many DISTINCT principals this emulator holds explicit account settings for.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`]: `PutAccountSetting` takes
/// the principal from the request, so without a cap a caller could grow the in-memory
/// store without limit.
pub const MAX_ACCOUNT_SETTING_PRINCIPALS: usize = 64;

/// The longest `principalArn` an account-setting write may name. EMULATOR-CHOSEN, so one
/// accepted write cannot carry an arbitrarily long key into the store: IAM bounds a
/// principal's name at 64 characters and its path at 512, so no ARN Amazon Web Services
/// itself mints comes close.
pub const MAX_PRINCIPAL_ARN_LENGTH: usize = 1024;

/// What [`ComAmazonawsEcsAmazonec2containerservicev20141113State::put_account_setting`]
/// answers when storing a setting for a principal this emulator does not hold one for
/// yet would push it past [`MAX_ACCOUNT_SETTING_PRINCIPALS`]: nothing was written, so
/// the caller refuses the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountSettingCapacityExceeded;

/// How long a task takes to cross one lifecycle boundary (`PENDING` -> `RUNNING`,
/// `DEACTIVATING` -> `STOPPED`). EMULATOR-CHOSEN — AWS publishes no duration for it —
/// and deliberately non-zero so a caller that reads straight back still observes the
/// transient status the real service reports.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The most tasks one `RunTask` call may place. The `RunTask` `count` doc: "You can
/// specify up to 10 tasks for each call."
pub const MAX_RUN_TASK_COUNT: i32 = 10;

/// The most container instances one `StartTask` call may place a task on. The
/// `StartTask$containerInstances` doc: "You can specify up to 10 container instances."
pub const MAX_START_TASK_CONTAINER_INSTANCES: usize = 10;

/// A list operation's page size when the caller gives no `maxResults`. The
/// `ListTasks$maxResults` and `ListContainerInstances$maxResults` docs both say: "This
/// value can be between 1 and 100. If this parameter isn't used, then
/// <operation> returns up to 100 results".
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// The largest `maxResults` a list operation accepts, from the same doc sentence.
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The smallest `maxResults` a list operation accepts — the other end of the same
/// sentence ("This value can be between 1 and 100").
pub const MIN_LIST_PAGE_SIZE: i32 = 1;

/// The fewest tasks one `RunTask` call may place: the `count` doc asks for "the number of
/// instantiations of the specified task to place", so a call that would place none names a
/// count below the range.
pub const MIN_RUN_TASK_COUNT: i32 = 1;

/// `ListServices`'s page size when the caller gives no `maxResults`. Its own
/// `maxResults` doc differs from the sentence [`DEFAULT_LIST_PAGE_SIZE`] quotes: "This
/// value can be between 1 and 100. If this parameter isn't used, then ListServices
/// returns up to 10 results and a nextToken value if applicable."
pub const DEFAULT_LIST_SERVICES_PAGE_SIZE: usize = 10;

/// How many containers one task definition may describe. The Amazon ECS service quota
/// "Containers per task definition" is 10, and the cap is also what bounds the work and
/// the stored bytes ONE `RegisterTaskDefinition` can ask for — and, through it, the
/// containers every task of that revision mints and every describe renders.
pub const MAX_CONTAINER_DEFINITIONS: usize = 10;

/// How many load-balancer attachments one service may be given. The Amazon ECS service
/// quota "Target groups per service" is 5, and the cap is also what bounds the work ONE
/// `CreateService` or `UpdateService` can ask for: every entry names up to two target
/// groups that are looked up in the Elastic Load Balancing v2 service's store, so
/// without it a caller could repeat an entry up to the request-body cap and set both how
/// much work this request does and how long that other service's lock is held.
pub const MAX_LOAD_BALANCERS: usize = 5;

/// How many ids one describe request may name. Amazon ECS accepts at most 100 per
/// `DescribeClusters`/`DescribeTasks`/`DescribeContainerInstances` call, and the cap is
/// also what bounds the work
/// and the response bytes ONE request can ask this emulator for while it holds the
/// store lock: without it a caller could repeat a single id until the request-body cap
/// and multiply it into an arbitrarily large response.
pub const MAX_DESCRIBE_BATCH: usize = 100;

/// The longest a cluster name or a task-definition family may be. The `clusterName` and
/// `family` docs both say "Up to 255 letters (uppercase and lowercase), numbers,
/// underscores, and hyphens are allowed."
pub const MAX_NAME_LENGTH: usize = 255;

/// `stoppedReason` when `StopTask` is called without a `reason` — the wording Amazon ECS
/// records for an operator-initiated stop.
pub const DEFAULT_STOPPED_REASON: &str = "Task stopped by user";

/// How many clusters this emulator holds. EMULATOR-CHOSEN, so a caller cannot grow the
/// in-memory store without limit.
pub const MAX_CLUSTERS: usize = 64;

/// How many task-definition revisions this emulator holds, across every family.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`].
pub const MAX_TASK_DEFINITIONS: usize = 512;

/// How many tasks this emulator holds, across every cluster. EMULATOR-CHOSEN, for the
/// same reason as [`MAX_CLUSTERS`].
pub const MAX_TASKS: usize = 512;

/// The statuses a task's `lastStatus` takes in this emulator. `PENDING` and `RUNNING`
/// and `STOPPED` are the ECS task-lifecycle statuses; `DEACTIVATING` is the first
/// lifecycle status a stopping task takes (`Task$stoppingAt`: "the time when the task
/// transitions from the RUNNING state to STOPPING").
pub const STATUS_PENDING: &str = "PENDING";
/// The lifecycle status of a task Amazon ECS has accepted but still has to acquire
/// resources for before it can place it — the status that precedes [`STATUS_PENDING`],
/// which a task only reaches once it has been placed. See
/// [`awaits_provisioned_capacity`].
pub const STATUS_PROVISIONING: &str = "PROVISIONING";
/// See [`STATUS_PENDING`].
pub const STATUS_RUNNING: &str = "RUNNING";
/// See [`STATUS_PENDING`].
pub const STATUS_DEACTIVATING: &str = "DEACTIVATING";
/// See [`STATUS_PENDING`].
pub const STATUS_STOPPED: &str = "STOPPED";

/// `Task$connectivity` of a task whose networking is in place, from the model's
/// `Connectivity` enum (`smithy_query(query="shape",shape="Connectivity")`: `CONNECTED`,
/// `DISCONNECTED`). See [`TaskRecord::report_connected`].
pub const CONNECTIVITY_CONNECTED: &str = "CONNECTED";

/// The container lifecycle, in the order a container passes through it — the statuses
/// this service itself stores on a container ([`ContainerRecord::last_status`], moved by
/// the task's own clock-owed transitions), earliest first.
///
/// `SubmitContainerStateChange` is the AGENT reporting where a container has got to, and
/// the agent retries: a report may arrive twice, or out of order behind a later one. The
/// order below is what makes a stale report recognisable — see
/// [`container_stage_rank`].
pub const CONTAINER_LIFECYCLE: [&str; 4] = [
    STATUS_PENDING,
    STATUS_RUNNING,
    STATUS_DEACTIVATING,
    STATUS_STOPPED,
];

/// How far through [`CONTAINER_LIFECYCLE`] `status` is, or `None` for a status that
/// lifecycle does not name (a stage a newer agent reports, which this service stores as
/// reported rather than ranking against stages it does not know).
pub fn container_stage_rank(status: &str) -> Option<usize> {
    CONTAINER_LIFECYCLE
        .iter()
        .position(|stage| *stage == status)
}

/// The task lifecycle, in the order a task passes through it — the statuses this service
/// stores on a task ([`TaskRecord::last_status`]), earliest first.
///
/// `SubmitTaskStateChange` is the AGENT reporting where a TASK has got to, and the agent
/// retries exactly as it does for a container, so the order below is what makes a stale
/// task report recognisable — see [`task_stage_rank`].
pub const TASK_LIFECYCLE: [&str; 5] = [
    STATUS_PROVISIONING,
    STATUS_PENDING,
    STATUS_RUNNING,
    STATUS_DEACTIVATING,
    STATUS_STOPPED,
];

/// How far through [`TASK_LIFECYCLE`] `status` is, or `None` for a status that lifecycle
/// does not name (a stage a newer agent reports, which this service stores as reported
/// rather than ranking against stages it does not know).
pub fn task_stage_rank(status: &str) -> Option<usize> {
    TASK_LIFECYCLE.iter().position(|stage| *stage == status)
}

/// `SubmitContainerStateChangeResponse$acknowledgment` for a state change this service
/// has recorded: the agent is told the report was taken, which is what the operation's own
/// doc calls it ("Sent to acknowledge that a container changed states").
pub const STATE_CHANGE_ACKNOWLEDGMENT: &str = "ACK";

/// `InvalidParameterException$message` for a `containerName` that names no container of
/// the task the state change names. The sentence the agent plane answers for a reference
/// that resolves to nothing, one resource along from
/// [`TASK_NOT_FOUND_MESSAGE`]: the request's own value is not echoed.
pub const CONTAINER_NOT_FOUND_MESSAGE: &str = "The referenced container was not found";

/// `InvalidParameterException$message` for an `ExecuteCommand` whose `task` names no task
/// of the cluster: a fixed sentence stating what the caller has to change.
pub const EXECUTE_COMMAND_TASK_NOT_FOUND_MESSAGE: &str =
    "The specified task was not found. Specify a valid task ID and try again.";

/// `InvalidParameterException$message` for an `ExecuteCommand` whose `container` names no
/// container of the task, worded as the task sentence above is.
pub const EXECUTE_COMMAND_CONTAINER_NOT_FOUND_MESSAGE: &str =
    "The specified container was not found. Specify a valid container name and try again.";

/// `InvalidParameterException$message` for an `ExecuteCommand` on a task whose execute
/// command agent is not running — the condition the ECS Exec troubleshooting guidance
/// words, and the one that holds for every task here: a task of this emulator runs no
/// container, so no managed agent of it is running to open a session on.
pub const EXECUTE_COMMAND_AGENT_NOT_RUNNING_MESSAGE: &str =
    "The execute command failed because execute command was not enabled when the task was \
     run or the execute command agent isn't running. Wait and try again or run a new task \
     with execute command enabled and try again.";

/// `Cluster$attachmentsStatus` while the cluster's capacity-provider association is
/// being applied — the member's doc: "UPDATE_IN_PROGRESS: The available capacity
/// providers for the cluster are updating."
pub const ATTACHMENTS_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// `Cluster$attachmentsStatus` once it has been applied — the same doc:
/// "UPDATE_COMPLETE: The capacity providers have successfully updated."
pub const ATTACHMENTS_UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The message of the `UpdateInProgressException` a cluster mutation is refused with
/// while the cluster's attachments are updating. It states the precondition the
/// `Cluster$attachmentsStatus` doc describes (`UPDATE_IN_PROGRESS` -> `UPDATE_COMPLETE`
/// / `UPDATE_FAILED`), which is what the refusal is about; the HTTP status (400) and the
/// error code are what a client matches on.
pub const CLUSTER_BUSY_MESSAGE: &str = "The specified cluster is in a busy state. Cluster \
                                        attachments must be in UPDATE_COMPLETE or \
                                        UPDATE_FAILED state before they can be updated. \
                                        Wait and try again.";

/// How many capacity providers one cluster may hold, and so the longest
/// `capacityProviders` list and `defaultCapacityProviderStrategy` one
/// `PutClusterCapacityProviders` may carry (every strategy entry must name one of the
/// providers the same request associates). The Amazon ECS service quota "Capacity
/// providers per cluster" is 20, and the cap is also what bounds the work ONE request
/// can ask for: the association is checked list-against-list and against the stored
/// association, so an uncapped list would let one caller choose the cost of both
/// factors.
pub const MAX_CLUSTER_CAPACITY_PROVIDERS: usize = 20;

/// `CapacityProvider$status` for a provider that can be used in a cluster — that
/// member's doc: "Only capacity providers in an `ACTIVE` state can be used in a
/// cluster." A provider over a caller's OWN Auto Scaling group is answered under it at
/// once: the group already exists, so Amazon ECS has nothing to stand up for it and no
/// clock-owed transition leads here.
pub const CAPACITY_PROVIDER_ACTIVE: &str = "ACTIVE";

/// `CapacityProvider$status` for a provider whose capacity Amazon ECS is still standing
/// up (the status enum's own `PROVISIONING`, `smithy_query` query="shape"
/// shape="CapacityProviderStatus").
///
/// A Managed Instances provider is created over infrastructure Amazon ECS runs on the
/// caller's behalf — the instance launch template, the network it launches instances
/// into, the infrastructure role it manages them with — so its create is answered under
/// this NON-TERMINAL status and the move to [`CAPACITY_PROVIDER_ACTIVE`] is owed by the
/// clock ([`TRANSITION_SPAN`]). Until then it is not a provider a cluster can use.
pub const CAPACITY_PROVIDER_PROVISIONING: &str = "PROVISIONING";

/// `CapacityProvider$updateStatus` while the create of a provider Amazon ECS has to stand
/// up is still running — "The update status reason ... provides further details about the
/// update status for the capacity provider", and the create is the first such update.
pub const CAPACITY_PROVIDER_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `CapacityProvider$updateStatus` once that create has finished: the terminal status of
/// a created provider, reported from the instant it becomes usable.
pub const CAPACITY_PROVIDER_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// `CapacityProvider$status` for a deleted provider — that member's doc: "When a capacity
/// provider is successfully deleted, it has an `INACTIVE` status." A tombstone under that
/// status holds neither its name nor its Auto Scaling group against a new provider.
pub const CAPACITY_PROVIDER_INACTIVE: &str = "INACTIVE";

/// `CapacityProvider$type` for a provider that manages EC2 instances itself — that
/// member's doc: "For Amazon ECS Managed Instances, this value is `MANAGED_INSTANCES`".
pub const CAPACITY_PROVIDER_TYPE_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// `CapacityProvider$type` for a provider over one of the caller's OWN Auto Scaling
/// groups — the `CapacityProviderType` value that names that class of capacity, beside
/// `MANAGED_INSTANCES` and the two Fargate ones.
pub const CAPACITY_PROVIDER_TYPE_EC2_AUTOSCALING: &str = "EC2_AUTOSCALING";

/// The prefixes a capacity-provider name may not start with, from the
/// `CreateCapacityProvider$name` doc: "The name can't be prefixed with \"aws\", \"ecs\",
/// or \"fargate\"." Matched case-insensitively, since the name itself accepts both cases
/// and the reservation is on the word rather than on its spelling.
pub const CAPACITY_PROVIDER_RESERVED_PREFIXES: [&str; 3] = ["aws", "ecs", "fargate"];

/// How many capacity providers one `DescribeCapacityProviders` page carries: the most it
/// serves, and what it serves when the request names no `maxResults`. The member's own
/// doc: "This value can be between 1 and 10. If this parameter is not used, then
/// DescribeCapacityProviders returns up to 10 results" — so a request asking for a BIGGER
/// page is served this one rather than refused.
pub const DESCRIBE_CAPACITY_PROVIDERS_PAGE_SIZE: usize = 10;

/// How many capacity providers this emulator holds. EMULATOR-CHOSEN, for the same
/// reason as [`MAX_CLUSTERS`]: the store is in memory and a single caller must not be
/// able to grow it without bound. A create past it is refused with the modelled
/// `LimitExceededException` ("The limit for the resource was exceeded"), which is what a
/// client's quota handling keys on.
pub const MAX_CAPACITY_PROVIDERS: usize = 64;

/// The values `ClusterSetting$value` accepts, from that member's own doc: "The supported
/// values are `enhanced`, `enabled`, and `disabled`." The setting NAME is a closed enum
/// the boundary validator already refuses outside, but the value is a free string the
/// service constrains itself — so every operation that accepts a `ClusterSetting`
/// ([`crate::ops::create_cluster`], [`crate::ops::update_cluster`],
/// [`crate::ops::update_cluster_settings`]) checks it through
/// [`validate_cluster_settings`].
pub const CLUSTER_SETTING_VALUES: [&str; 3] = ["enhanced", "enabled", "disabled"];

/// `InvalidParameterException$message` for a `ClusterSetting` whose `value` is not one of
/// [`CLUSTER_SETTING_VALUES`]. Amazon ECS names the MEMBER of the setting structure that
/// is wrong in one fixed sentence, and echoes neither the value the request carried nor
/// the set it was checked against.
pub const INVALID_CLUSTER_SETTING_VALUE_MESSAGE: &str = "Invalid setting 'value'";


/// `ExecuteCommandConfiguration$logging` value that carries its own destination — the
/// member's doc: "OVERRIDE: Specify the logging details as a part of logConfiguration."
pub const EXECUTE_COMMAND_LOGGING_OVERRIDE: &str = "OVERRIDE";

/// `Cluster$status` for a live cluster.
pub const CLUSTER_ACTIVE: &str = "ACTIVE";
/// `Cluster$status` for a deleted cluster: "The cluster has been deleted."
pub const CLUSTER_INACTIVE: &str = "INACTIVE";

/// `ContainerInstance$status` for a registered instance: the member's doc, "The `ACTIVE`
/// status indicates that the container instance can accept tasks."
pub const CONTAINER_INSTANCE_ACTIVE: &str = "ACTIVE";
/// `ContainerInstance$status` for an instance taken out of service: "The `DRAINING`
/// indicates that new tasks aren't placed on the container instance".
pub const CONTAINER_INSTANCE_DRAINING: &str = "DRAINING";
/// `ContainerInstance$status` for a deregistered instance: "The instance then
/// transitions to an `INACTIVE` status." The status
/// `ListContainerInstances$status`'s doc excludes by default.
pub const CONTAINER_INSTANCE_INACTIVE: &str = "INACTIVE";

/// How many container instances this emulator holds, across every cluster.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_CLUSTERS`].
pub const MAX_CONTAINER_INSTANCES: usize = 512;

/// `ContainerInstance$agentUpdateStatus` for an agent update `UpdateContainerAgent` has
/// accepted but the instance has not finished: the first stage of the `AgentUpdateStatus`
/// enum (`PENDING`, `STAGING`, `STAGED`, `UPDATING`, `UPDATED`, `FAILED`), and the one the
/// `UpdateInProgressException` doc names as "a transitional stage, such as PENDING or
/// STAGING".
pub const AGENT_UPDATE_PENDING: &str = "PENDING";

/// `ContainerInstance$agentUpdateStatus` once the update the clock owed has landed: the
/// terminal successful stage of the same enum.
pub const AGENT_UPDATE_UPDATED: &str = "UPDATED";

/// The `AgentUpdateStatus` stages an update is still under way in, so a second
/// `UpdateContainerAgent` on the same instance is the `UpdateInProgressException` its doc
/// describes ("There's already a current Amazon ECS container agent update in progress on
/// the container instance"). The stages of the enum that are neither terminal success
/// (`UPDATED`) nor terminal failure (`FAILED`).
pub const AGENT_UPDATE_IN_FLIGHT: [&str; 4] = ["PENDING", "STAGING", "STAGED", "UPDATING"];

/// The container-agent version this emulator publishes as the update target: the version
/// an instance's agent is brought to by `UpdateContainerAgent`, and the one an instance
/// already reporting it has no update available for. EMULATOR-CHOSEN — no model member
/// carries it — and the single source for both halves of that rule.
pub const LATEST_CONTAINER_AGENT_VERSION: &str = "1.89.3";

/// `NoUpdateAvailableException$message` for an instance whose agent already runs
/// [`LATEST_CONTAINER_AGENT_VERSION`], as the error's own doc states the condition
/// ("There's no update available for this Amazon ECS container agent. This might be
/// because the agent is already running the latest version").
pub const NO_AGENT_UPDATE_AVAILABLE_MESSAGE: &str =
    "There is no update available for this Amazon ECS container agent.";

/// `MissingVersionException$message` for an instance that reported no agent version when
/// it registered, as the error's own doc states the condition ("Amazon ECS can't determine
/// the current version of the Amazon ECS container agent on the container instance and
/// doesn't have enough information to proceed with an update").
pub const MISSING_AGENT_VERSION_MESSAGE: &str =
    "Amazon ECS cannot determine the current version of the Amazon ECS container agent on \
     the container instance and does not have enough information to proceed with an update.";

/// `UpdateInProgressException$message` for an instance whose agent update has not finished
/// yet, as the error's own doc states the condition.
pub const AGENT_UPDATE_IN_PROGRESS_MESSAGE: &str =
    "There is already a current Amazon ECS container agent update in progress on the \
     specified container instance.";

/// `TaskDefinition$status` for a registered revision.
pub const TASK_DEFINITION_ACTIVE: &str = "ACTIVE";

/// `TaskDefinition$status` for a revision that has been deregistered — the
/// `DeregisterTaskDefinition` doc: "Upon deregistration, the task definition is marked
/// as `INACTIVE`." One of the `TaskDefinitionStatus` values
/// (`smithy_query(query="shape", shape="TaskDefinitionStatus")`), and the status a
/// revision keeps: "INACTIVE task definitions remain discoverable in your account
/// indefinitely."
pub const TASK_DEFINITION_INACTIVE: &str = "INACTIVE";

/// `TaskDefinition$status` for a revision whose DELETE has been asked for — the
/// `DeleteTaskDefinitions` doc: "When you delete a task definition revision, it is
/// immediately transitions from the `INACTIVE` to `DELETE_IN_PROGRESS`." The third
/// `TaskDefinitionStatus` value
/// (`smithy_query(query="shape", shape="TaskDefinitionStatus")`), and the status a
/// revision keeps while the deletion is outstanding: "A task definition revision will
/// stay in `DELETE_IN_PROGRESS` status until all the associated tasks and services have
/// been terminated", and while it is there "the task definition name is displayed in the
/// console and returned in the API" — so the revision stays stored and readable.
pub const TASK_DEFINITION_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `DaemonTaskDefinition$status` for a registered daemon revision. One of the
/// `DaemonTaskDefinitionStatus` values
/// (`smithy_query(query="shape", shape="DaemonTaskDefinitionStatus")`: `ACTIVE`,
/// `DELETE_IN_PROGRESS`, `DELETED`), and the one `RegisterDaemonTaskDefinition` answers
/// with: a registered revision is usable straight away, so no clock-owed transition
/// leads to it.
pub const DAEMON_TASK_DEFINITION_ACTIVE: &str = "ACTIVE";

/// `DaemonTaskDefinition$status` while the delete a `DeleteDaemonTaskDefinition` call
/// asked for is still running — the status a revision stands in from the instant the
/// delete was requested until it has been carried out, and the second of the
/// `DaemonTaskDefinitionStatus` values.
pub const DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";

/// `DaemonTaskDefinition$status` once a requested delete has been carried out — the
/// terminal `DaemonTaskDefinitionStatus` value, reached [`TRANSITION_SPAN`] after the
/// delete was requested and never left again.
pub const DAEMON_TASK_DEFINITION_DELETED: &str = "DELETED";

/// The `DaemonTaskDefinitionStatusFilter` value that narrows a daemon task-definition
/// listing to nothing at all — `ListDaemonTaskDefinitions$status`: "If you set this
/// parameter to `ALL`, all daemon task definitions are listed regardless of status." The
/// other two values of that enum are statuses a stored revision carries, so they are
/// matched against [`DaemonTaskDefinitionRecord::status`] directly.
pub const DAEMON_TASK_DEFINITION_STATUS_FILTER_ALL: &str = "ALL";

/// The one `DaemonTaskDefinitionRevisionFilter` value
/// ([`crate::ops::types::DAEMON_TASK_DEFINITION_REVISION_FILTER_VALUES`]) —
/// `ListDaemonTaskDefinitions$revision`: "Specify `LAST_REGISTERED` to return only the
/// last registered revision for each daemon task definition family."
pub const DAEMON_REVISION_FILTER_LAST_REGISTERED: &str = "LAST_REGISTERED";

/// The namespace mode a daemon revision records when the registration named none, for
/// `pidMode` and for `ipcMode` alike. Both members' docs: "The valid values are `none`
/// and `shared`. The default is `none`. If `none` is specified or no value is provided,
/// the daemon runs with its own PID/IPC namespace, isolated from other tasks" — so a
/// registration that named nothing and one that named this value describe the same
/// daemon, and the stored revision spells it once.
pub const DAEMON_NAMESPACE_MODE_NONE: &str = "none";

/// How many daemon task-definition revisions this emulator holds, across every family.
/// EMULATOR-CHOSEN, for the same reason as [`MAX_TASK_DEFINITIONS`].
pub const MAX_DAEMON_TASK_DEFINITIONS: usize = MAX_TASK_DEFINITIONS;

/// `DaemonStatus` for a daemon `CreateDaemon` has created. One of the `DaemonStatus`
/// values (`smithy_query(query="shape", shape="DaemonStatus")`: `ACTIVE`,
/// `DELETE_IN_PROGRESS`) and the one a create answers with: the daemon is there and
/// usable from that instant, so no clock-owed transition leads to it.
pub const DAEMON_ACTIVE: &str = "ACTIVE";

/// Whether `critical` is on for a daemon whose create named nothing. The member's doc:
/// "The default value is true."
pub const DAEMON_CRITICAL_DEFAULT: bool = true;

/// How many daemons this emulator holds, across every cluster. EMULATOR-CHOSEN, for the
/// same reason as [`MAX_SERVICES`].
pub const MAX_DAEMONS: usize = MAX_SERVICES;

/// `InvalidParameterException$message` for a `capacityProviderArns` that names a capacity
/// provider whose type is not [`CAPACITY_PROVIDER_TYPE_MANAGED_INSTANCES`].
///
/// The `CreateDaemon` doc: "ECS Managed Daemons is only supported for Amazon ECS Managed
/// Instances Capacity Providers" — a condition of the LIST as a whole, so the sentence
/// states the rule and names neither the entry at fault nor the type it carried.
pub const DAEMON_MANAGED_INSTANCES_REQUIRED_MESSAGE: &str =
    "All capacity providers must be of MANAGED_INSTANCES type for Daemon services";

/// The `SortOrder` value that REVERSES a listing's documented default order — the
/// `ListTaskDefinitions$sort` doc: "Setting this parameter to `DESC` reverses the sort
/// order on family name and revision." One of the two values the model's enum carries
/// ([`crate::ops::types::SORT_ORDER_VALUES`]); the other, `ASC`, is the order a request
/// that names no sort already gets.
pub const SORT_ORDER_DESC: &str = "DESC";

/// `Failure$reason` for an id that names nothing in this account. The ECS "API failure
/// reasons" list: `MISSING` — "the described resource could not be found".
pub const FAILURE_MISSING: &str = "MISSING";

/// `Failure$reason` for a task a task-protection call cannot act on. The
/// `UpdateTaskProtection` doc: "This operation is only supported for tasks belonging to
/// an Amazon ECS service. Invoking this operation for a standalone task will result in an
/// `TASK_NOT_VALID` failure."
pub const FAILURE_TASK_NOT_VALID: &str = "TASK_NOT_VALID";

/// How long a task stays protected when the request names no period.
/// `UpdateTaskProtection$expiresInMinutes`: "If you don't specify the time, then the task
/// is automatically protected for 120 minutes (2 hours)."
pub const DEFAULT_TASK_PROTECTION_MINUTES: i32 = 120;

/// The shortest protection period the request may name. Same member: "You can specify a
/// value from 1 minute to up to 2,880 minutes (48 hours)."
pub const MIN_TASK_PROTECTION_MINUTES: i32 = 1;

/// The longest protection period the request may name — the other end of the same
/// sentence.
pub const MAX_TASK_PROTECTION_MINUTES: i32 = 2_880;

/// How many tasks ONE `UpdateTaskProtection` may name.
/// `UpdateTaskProtection$tasks`: "A list of up to 10 task IDs or full ARN entries."
pub const MAX_UPDATE_TASK_PROTECTION_TASKS: usize = 10;

/// How many tasks ONE `GetTaskProtection` may name.
/// `GetTaskProtection$tasks`: "A list of up to 100 task IDs or full ARN entries."
pub const MAX_GET_TASK_PROTECTION_TASKS: usize = 100;

/// `InvalidParameterException$message` for an integer member whose value falls outside the
/// inclusive range its own doc states, or `None` when the value is inside it. Amazon ECS
/// names the member and the ONE bound the value crossed — "<member> must be at least
/// <min>" below the range, "<member> must be at most <max>" above it — and echoes neither
/// the value the request carried nor the bound it did not cross.
///
/// Spelled once for every bounded integer in this crate (`expiresInMinutes`, a page's
/// `maxResults`, a `RunTask` `count`), so the refusal cannot drift between the operations
/// that carry one.
pub fn integer_bound_message(member: &str, value: i32, min: i32, max: i32) -> Option<String> {
    if value < min {
        Some(format!("{member} must be at least {min}"))
    } else if value > max {
        Some(format!("{member} must be at most {max}"))
    } else {
        None
    }
}

/// `InvalidParameterException$message` for a `RunTask` `count` outside the inclusive range
/// its own doc states ("You can specify up to 10 tasks for each call"), or `None` when the
/// count is inside it.
///
/// Amazon ECS states a COUNT the way it states every other count of this API
/// ([`DESIRED_COUNT_NEGATIVE_MESSAGE`]): the count named in words, the one bound the value
/// crossed, and nothing of the value the request carried.
pub fn task_count_bound_message(count: i32, min: i32, max: i32) -> Option<String> {
    if count < min {
        Some(format!("Task count can not be lesser than {min}"))
    } else if count > max {
        Some(format!("Task count can not be greater than {max}"))
    } else {
        None
    }
}

/// `InvalidParameterException$message` for a `maxResults` outside the inclusive page-size
/// range its own doc states ("This value can be between 1 and 100"). Amazon ECS states the
/// RANGE the page size has to fall in and echoes neither the value the request carried nor
/// the member's spelling on the wire — only the two bounds, which are the operation's own
/// documented minimum and maximum page size.
pub fn page_size_range_message(min: i32, max: i32) -> String {
    format!("MaxResults must be a positive number between {min}-{max}.")
}

/// The sentence a value BELOW the smallest its own member documents is refused with: the
/// `subject` the refusal names that member by and the bound the value crossed, echoing
/// neither the value the request carried nor the other end of the range.
///
/// Spelled once for every member of this API bounded from below, so the refusal cannot
/// drift between them — [`DESIRED_COUNT_NEGATIVE_MESSAGE`] carries it for a service's
/// task count, and a page's `maxResults` carries it for a list.
pub fn below_minimum_message(subject: &str, min: i32) -> String {
    format!("{subject} can not be lesser than {min}")
}

/// The refusal a `maxResults` of one of this API's LONG-STANDING list operations earns
/// when it falls outside the inclusive page-size range `min..=max` its own doc states, or
/// `None` when it is inside it.
///
/// Spelled once for every such operation in this crate, so the refusal cannot drift
/// between those lists — each with its own documented range. A page BELOW the range is the
/// below-the-minimum refusal every such member answers ([`below_minimum_message`]); one
/// above it states the range the page size has to fall in. The operations added with the
/// daemon plane state that range at EITHER end instead, so they reach for
/// [`page_size_range_violation`].
pub fn page_size_violation(max_results: i32, min: i32, max: i32) -> Option<String> {
    if max_results < min {
        Some(below_minimum_message(&member_subject("maxResults"), min))
    } else if max_results > max {
        Some(page_size_range_message(min, max))
    } else {
        None
    }
}

/// The refusal a `maxResults` of a DAEMON-PLANE list earns when it falls outside the
/// inclusive page-size range `min..=max` its own doc states, or `None` when it is inside
/// it.
///
/// Both ends of one documented range are the same violation of one rule for these
/// operations, so both earn the one sentence that states that range
/// ([`page_size_range_message`]) with that operation's own bounds — unlike the
/// long-standing lists, whose low end names the bound the value crossed
/// ([`page_size_violation`]).
pub fn page_size_range_violation(max_results: i32, min: i32, max: i32) -> Option<String> {
    (max_results < min || max_results > max).then(|| page_size_range_message(min, max))
}

/// The launch type a task takes when the request names neither a `launchType` nor a
/// `capacityProviderStrategy` and the cluster has no default strategy: ECS runs the
/// task on the EC2 capacity registered to the cluster.
pub const DEFAULT_LAUNCH_TYPE: &str = "EC2";

/// `stopCode` for a stop the caller asked for, from the `TaskStopCode` enum.
pub const STOP_CODE_USER_INITIATED: &str = "UserInitiated";

/// `stopCode` for a task that stopped because the work its containers were placed to do
/// is finished: the `TaskStopCode` enum's `EssentialContainerExited`
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated) — "The task stopped because an essential container
/// exited" (Stopped tasks error codes, Amazon ECS Developer Guide).
pub const STOP_CODE_ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// The `stoppedReason` that goes with [`STOP_CODE_ESSENTIAL_CONTAINER_EXITED`]: Amazon
/// ECS's own words for a task whose essential container has exited.
pub const ESSENTIAL_CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// How long after it starts a STANDALONE task's containers have finished the work they
/// were placed to do, after which Amazon ECS stops the task
/// ([`TaskRecord::finish_work`]).
///
/// A task placed on its own is one run of a workload — nothing replaces it when it ends,
/// which is why "if you have an application that runs on a schedule or that runs to
/// completion" is a standalone task and "a long-running application" is a service
/// (`RunTask`, and `CreateService`: the scheduler "replaces tasks that fail or stop") — so
/// its containers' work finishes and the task reaches the terminal `STOPPED` status. A
/// task a service's scheduler placed takes no such instant: that scheduler keeps the
/// service's desired count in place, so the service always holds a task that is running.
///
/// How long the work takes is the workload's own business and AWS publishes no duration
/// for it, so the span is CHOSEN: long enough that a client that places a task and then
/// polls it observes it `RUNNING` — the status Amazon ECS reports for a task that is doing
/// its work — across several reads, and short enough that a client waiting for the task to
/// finish gets its answer inside an ordinary poll budget. The instant is STORED
/// ([`TaskRecord::work_done_due_at`]) and materialised by whoever next looks at the task or
/// by the clock pump; nothing sleeps for it.
pub const CONTAINER_WORK_SPAN: Duration = Duration::from_secs(10);

/// The network mode that gives each task its own elastic network interface, from the
/// `NetworkMode` enum. Named here because three derivations key off it: which launch
/// types a revision is compatible with, which container-instance attributes it
/// requires, and how a port mapping's `hostPort` is defaulted.
pub const NETWORK_MODE_AWSVPC: &str = "awsvpc";

/// The network mode that puts the containers on the host's own network namespace, from
/// the `NetworkMode` enum. Like [`NETWORK_MODE_AWSVPC`] it maps a container port
/// statically onto the same host port.
pub const NETWORK_MODE_HOST: &str = "host";

/// The network mode that leaves the containers without external connectivity, from the
/// `NetworkMode` enum. The `networkMode` doc: "If the network mode is set to none, you
/// cannot specify port mappings in your container definitions".
pub const NETWORK_MODE_NONE: &str = "none";

/// The network mode that puts the containers on a docker bridge and publishes their ports
/// on the host that runs them, from the `NetworkMode` enum — the mode a revision that names
/// none runs under on Amazon EC2 capacity.
pub const NETWORK_MODE_BRIDGE: &str = "bridge";

/// The ELBv2 `TargetType` that registers a target by IP address, from that service's
/// `TargetTypeEnum` (`smithy_query(query="shape", shape="TargetTypeEnum",
/// model_path="services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/smithy-model/com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.json")`:
/// `instance`, `ip`, `lambda`, `alb`).
pub const TARGET_TYPE_IP: &str = "ip";

/// The ELBv2 `TargetType` that registers a target by EC2 instance id, from the same
/// enum as [`TARGET_TYPE_IP`].
pub const TARGET_TYPE_INSTANCE: &str = "instance";

/// The `protocol` a port mapping takes when the registration names none. The
/// `PortMapping$protocol` doc: "Valid values are tcp and udp (case-sensitive). The
/// default is tcp."
pub const DEFAULT_PORT_PROTOCOL: &str = "tcp";

/// The `hostPort` a port mapping takes under a dynamic mapping strategy. The
/// `PortMapping$hostPort` doc, for the `bridge` network mode: "you can omit the hostPort
/// (or set it to 0) while specifying a containerPort and your container automatically
/// receives a port in the ephemeral port range" — the port is picked when the task is
/// placed, so the registered revision reports the `0` that asks for one.
pub const DYNAMIC_HOST_PORT: i32 = 0;

/// Every launch type a task-definition projection may report, in the order Amazon ECS
/// reports them in `TaskDefinition$compatibilities`. The membership is the `Compatibility`
/// enum (`smithy_query(query="shape", shape="com.amazonaws.ecs#Compatibility")`:
/// `EC2`, `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`); the ORDER is the one recorded ECS
/// responses carry, so a client that reads the list positionally reads the same launch
/// type the real service named. [`TaskDefinitionRecord::compatibilities`] filters it by
/// what the stored revision's parameters support.
pub const COMPATIBILITY_ORDER: [&str; 4] = [
    "EXTERNAL",
    "EC2",
    LAUNCH_TYPE_MANAGED_INSTANCES,
    LAUNCH_TYPE_FARGATE,
];

/// The launch types `Cluster$statistics` separates a cluster's counts by, each paired
/// with the segment its four statistic names carry (`running<segment>TasksCount`,
/// `pending<segment>TasksCount`, `active<segment>ServiceCount`,
/// `draining<segment>ServiceCount`).
///
/// The membership is the `LaunchType` enum (`smithy_query(query="shape",
/// shape="LaunchType")`: `EC2`, `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`), so every
/// launch type a task or a service can be placed with has its own counters: the
/// `Cluster$statistics` doc lists the first [`DOCUMENTED_STATISTICS_LAUNCH_TYPES`] of
/// them by name and introduces the list with "They include the following", so the
/// remaining launch types' counters are reported too. [`cluster_statistics`] fixes the
/// order the entries come in.
pub const STATISTICS_LAUNCH_TYPES: [(&str, &str); 4] = [
    ("EC2", "EC2"),
    ("FARGATE", "Fargate"),
    ("EXTERNAL", "External"),
    ("MANAGED_INSTANCES", "ManagedInstances"),
];

/// How many of [`STATISTICS_LAUNCH_TYPES`] the `Cluster$statistics` doc itself names.
/// Those launch types' counters come first, counter by counter, as the doc lists them.
const DOCUMENTED_STATISTICS_LAUNCH_TYPES: usize = 2;

/// The platform versions this service knows, as the `platformVersion` doc's "Fargate
/// platform versions" page publishes them: the Linux platform version `1.4.0` and the
/// Windows one `1.0.0`, plus the `LATEST` alias the member's own doc names ("If one isn't
/// specified, the LATEST platform version is used").
///
/// The earlier Linux versions (`1.3.0` and below) are RETIRED — that page keeps them only
/// under its deprecation notice — so a request that names one names no platform a task can
/// be placed on, which is what `PlatformUnknownException` ("The specified platform version
/// doesn't exist.") is modelled for, exactly as a value that was never a version is.
pub const PLATFORM_VERSIONS: [&str; 3] = [
    PLATFORM_VERSION_LATEST,
    LATEST_LINUX_PLATFORM_VERSION,
    LATEST_WINDOWS_PLATFORM_VERSION,
];

/// The `platformVersion` alias that asks for whatever the newest platform version is,
/// rather than naming one: "If one isn't specified, the LATEST platform version is used."
pub const PLATFORM_VERSION_LATEST: &str = "LATEST";

/// The Fargate platform version [`PLATFORM_VERSION_LATEST`] resolves to for a LINUX
/// task, as the `platformVersion` doc's "Fargate platform versions" page states: `1.4.0`
/// is the newest Linux platform version.
pub const LATEST_LINUX_PLATFORM_VERSION: &str = "1.4.0";

/// The Fargate platform version [`PLATFORM_VERSION_LATEST`] resolves to for a Windows
/// task: the same page publishes exactly one Windows platform version, `1.0.0`.
pub const LATEST_WINDOWS_PLATFORM_VERSION: &str = "1.0.0";

/// The launch type whose tasks run on Fargate, and so the one the Fargate-only members
/// (`platformFamily`, `platformVersion`) are reported for.
pub const LAUNCH_TYPE_FARGATE: &str = "FARGATE";

/// The launch type whose tasks run on capacity Amazon ECS manages for the caller, which a
/// placement request reaches only through a capacity provider — see
/// [`DEFAULT_VALIDATED_LAUNCH_TYPES`].
pub const LAUNCH_TYPE_MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The launch types Amazon ECS validates EVERY registration against, whether or not the
/// request named them in its `requiresCompatibilities`.
///
/// These are the launch types a placement request names outright: `RunTask$launchType` /
/// `CreateService$launchType` is "The infrastructure that you run your service on", whose
/// doc singles the remaining one out — "If you want to use Amazon ECS Managed Instances,
/// you must use the `capacityProviderStrategy` request parameter and omit the
/// `launchType` request parameter" — so a revision is validated against managed-instances
/// capacity only when its registration ASKED to be
/// (`RegisterTaskDefinition$requiresCompatibilities`: "The task launch type that Amazon
/// ECS validates the task definition against"), and a recorded `awsvpc` revision that
/// asked only for Fargate reports just `EC2` and `FARGATE`.
/// [`TaskDefinitionRecord::validated_against`] is where the two sources meet.
pub const DEFAULT_VALIDATED_LAUNCH_TYPES: [&str; 3] = ["EXTERNAL", "EC2", LAUNCH_TYPE_FARGATE];

/// The capacity providers Amazon ECS manages for every account in every Region, which
/// place tasks on Fargate: the `UpdateService$capacityProviderStrategy` doc — "When you
/// use Fargate, the capacity providers are FARGATE or FARGATE_SPOT". They are not created
/// by `CreateCapacityProvider` (their names are reserved,
/// [`CAPACITY_PROVIDER_RESERVED_PREFIXES`]) and exist whether or not anybody asked, so
/// they are resolved rather than stored ([`managed_capacity_provider`]).
pub const MANAGED_CAPACITY_PROVIDERS: [&str; 2] = ["FARGATE", "FARGATE_SPOT"];

/// `InvalidParameterException$message` for a request that names a capacity provider this
/// account holds none of — neither a created one nor one of
/// [`MANAGED_CAPACITY_PROVIDERS`]. `CapacityProviderStrategyItem`: "If specifying a
/// capacity provider that uses an Auto Scaling group, the capacity provider must already
/// be created." One fixed sentence that names no provider, answered by every operation
/// that resolves a provider by name.
pub const CAPACITY_PROVIDER_NOT_FOUND_MESSAGE: &str =
    "The specified capacity provider was not found. Specify a valid capacity provider and \
     try again.";

/// `InvalidParameterException$message` for a PLACEMENT whose `capacityProviderStrategy`
/// names a capacity provider that exists but is not one of the cluster's own.
///
/// A capacity provider has to be ASSOCIATED with a cluster before the cluster's placements
/// may use it: `PutClusterCapacityProviders` is what associates one ("the capacity
/// providers to associate with the cluster"), and `Cluster$capacityProviders` is the list it
/// leaves behind. One fixed sentence, naming neither the provider nor the cluster, with the
/// same remediation clause [`CAPACITY_PROVIDER_NOT_FOUND_MESSAGE`] closes on.
pub const CAPACITY_PROVIDER_NOT_ASSOCIATED_MESSAGE: &str =
    "The specified capacity provider strategy cannot contain a capacity provider that is \
     not associated with the cluster. Associate the capacity provider with the cluster or \
     specify a valid capacity provider and try again.";

/// `ResourceInUseException$message` for a `PutClusterCapacityProviders` that would drop a
/// capacity provider a workload of the cluster still depends on.
///
/// The operation's own rule — "you can only disassociate an existing capacity provider from
/// a cluster if it's not being used by any existing tasks" — stated as Amazon ECS states the
/// refusal: one fixed sentence naming neither the provider nor the cluster, like the two
/// above.
pub const CAPACITY_PROVIDER_IN_USE_MESSAGE: &str =
    "The specified capacity provider is in use and cannot be removed.";

/// `InvalidParameterException$message` for a PLACEMENT request — one that says where its
/// tasks run with a `capacityProviderStrategy` — naming a capacity provider this account
/// holds none of.
///
/// The same condition as [`CAPACITY_PROVIDER_NOT_FOUND_MESSAGE`], worded as Amazon ECS
/// words it when the provider was named by a strategy: the sentence NAMES the provider the
/// request asked for, which is the one value the caller has to change, and is otherwise
/// fixed. `reference` is the value as the request spelled it (a short name or an ARN), so
/// nothing but the request reaches the body.
pub fn capacity_provider_not_found_message(reference: &str) -> String {
    format!(
        "The specified capacity provider {reference} was not found. Specify a valid capacity \
         provider and try again."
    )
}

/// `InvalidParameterException$message` for a request that supplies BOTH a `launchType` and
/// a `capacityProviderStrategy`. The two members name the same thing two ways — where the
/// tasks run — so a request that carries both says it twice: "If a
/// `capacityProviderStrategy` is specified, the `launchType` parameter must be omitted."
/// One fixed sentence, naming neither member's value, answered by every operation that
/// takes the pair.
pub const LAUNCH_TYPE_WITH_CAPACITY_PROVIDER_STRATEGY_MESSAGE: &str =
    "Specifying both a launch type and capacity provider strategy is not supported. Remove one \
     and try again.";

/// The inclusive range `CapacityProviderStrategyItem$base` admits, from the member's own
/// `@range` trait and its doc ("The valid range is 0 to 100,000").
pub const CAPACITY_PROVIDER_STRATEGY_BASE_RANGE: (i64, i64) = (0, 100_000);

/// The inclusive range `CapacityProviderStrategyItem$weight` admits, from the member's own
/// `@range` trait and its doc ("The valid range is 0 to 1,000").
pub const CAPACITY_PROVIDER_STRATEGY_WEIGHT_RANGE: (i64, i64) = (0, 1_000);

/// `InvalidParameterException$message` for a capacity-provider-strategy entry whose
/// `base` or `weight` is outside the range that member admits, or `None` when `member` is
/// not one of those two.
///
/// Amazon ECS speaks in the strategy's own terms rather than in the model's: it names the
/// member, the offending value AS THE REQUEST SENT IT, and the whole range the member
/// takes — not the one bound that was crossed and no member path.
pub fn capacity_provider_strategy_value_message(member: &str, value: &str) -> Option<String> {
    let (min, max) = match member {
        "base" => CAPACITY_PROVIDER_STRATEGY_BASE_RANGE,
        "weight" => CAPACITY_PROVIDER_STRATEGY_WEIGHT_RANGE,
        _ => return None,
    };
    Some(format!(
        "The specified {member} value '{value}' is invalid. Specify an integer between \
         {min} and {max} and try again."
    ))
}

/// `InvalidParameterException$message` for a `tags` entry whose key or value carries a
/// character outside the set a tag admits. The `Tag` doc: "Generally allowed characters
/// are: letters, numbers, and spaces representable in UTF-8, and the following
/// characters: + - = . _ : / @" — the same set the `TagKey`/`TagValue` `@pattern` states.
/// ONE fixed sentence: it names the set, and neither the member that carried the
/// character nor the value it was in.
pub const INVALID_TAG_CHARACTERS_MESSAGE: &str =
    "Some tags contain invalid characters. Valid characters: UTF-8 letters, spaces, numbers \
     and _ . / = + - : @.";


/// The `OSFamily` a revision that declares no `runtimePlatform` runs on. `RuntimePlatform
/// $operatingSystemFamily` is "The operating system", whose `OSFamily` enum
/// (`smithy_query(query="shape", shape="OSFamily")`) spans `LINUX` and the Windows
/// Server families; a task definition that names none runs on Linux.
pub const DEFAULT_OS_FAMILY: &str = "LINUX";

/// The DISPLAY spelling of an `OSFamily` value, which is what `Service$platformFamily`
/// and `Deployment$platformFamily` report ("The operating system that your tasks in the
/// service run on").
///
/// The enum's own name, rendered for display: the `_`-separated words become `-`-separated
/// words, and a word that is all letters is capitalised (`LINUX` -> `Linux`,
/// `WINDOWS_SERVER_2019_FULL` -> `Windows-Server-2019-Full`). A word that is not all
/// letters is a release designation rather than a word and is kept exactly as the enum
/// spells it (`2019`, `20H2`), so every value of the enum — including the ones no request
/// here names — renders deterministically.
pub fn os_family_display(os_family: &str) -> String {
    os_family
        .split('_')
        .map(|word| {
            if !word.chars().all(|character| character.is_ascii_alphabetic()) {
                return word.to_string();
            }
            let mut characters = word.chars();
            match characters.next() {
                None => String::new(),
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &characters.as_str().to_lowercase()
                }
            }
        })
        .collect::<Vec<String>>()
        .join("-")
}

/// The platform version a DEPLOYMENT runs, resolved from the one the service asks for and
/// the operating-system family its tasks run on.
///
/// A service may ask for [`PLATFORM_VERSION_LATEST`], which names no version: the
/// deployment runs the newest platform version published for its operating system
/// ([`LATEST_LINUX_PLATFORM_VERSION`], [`LATEST_WINDOWS_PLATFORM_VERSION`]). A service
/// that named a version runs that one.
pub fn resolved_platform_version(requested: &str, os_family: &str) -> String {
    if requested != PLATFORM_VERSION_LATEST {
        return requested.to_string();
    }
    if os_family == DEFAULT_OS_FAMILY {
        LATEST_LINUX_PLATFORM_VERSION.to_string()
    } else {
        LATEST_WINDOWS_PLATFORM_VERSION.to_string()
    }
}

/// The message a `PlatformUnknownException` carries: the FIXED sentence Amazon ECS
/// answers for this condition, as the recorded refusal carries it (the error shape's
/// own doc words the same condition as "The specified platform version doesn't
/// exist."). It names nothing from the request.
pub const PLATFORM_UNKNOWN_MESSAGE: &str = "The specified platform does not exist.";

/// The launch types whose capacity is REGISTERED to the cluster, so a task placed with
/// one of them needs a container instance of the cluster to run on. The `launchType`
/// doc: "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
/// cluster. ... The EXTERNAL launch type runs your tasks on your on-premises server or
/// virtual machine (VM) capacity registered to your cluster." The other launch types run
/// on capacity Amazon Web Services manages, which the cluster does not hold.
pub const REGISTERED_CAPACITY_LAUNCH_TYPES: [&str; 2] = ["EC2", "EXTERNAL"];

/// The message the `InvalidParameterException` carries when a task would be placed on
/// capacity registered to a cluster that has none. A FIXED sentence naming nothing from
/// the request, as the recorded refusal carries it.
pub const NO_CONTAINER_INSTANCES_MESSAGE: &str =
    "No Container Instances were found in your cluster.";

/// The message the `InvalidParameterException` carries when the placement runs through a
/// CAPACITY PROVIDER whose capacity is not there: the provider the placement named is
/// what the caller has to scale, and a cluster can carry several of them, so the sentence
/// is built from the request and names that one provider.
pub fn empty_capacity_provider_message(provider_name: &str) -> String {
    format!("The specified capacity provider '{provider_name}' contains no container instances.")
}

/// The container-instance attribute an `awsvpc` task needs: its own elastic network
/// interface. One of the Amazon ECS container-agent capabilities (the ECS developer
/// guide's "awsvpc network mode" requirements), reported through
/// `TaskDefinition$requiresAttributes` so a client can check an instance before placing
/// a task on it.
pub const ATTRIBUTE_TASK_ENI: &str = "ecs.capability.task-eni";

/// The minimum Docker remote-API version an `awsvpc` task needs on the container
/// instance, as the agent publishes it (`com.amazonaws.ecs.capability.docker-remote-api.<version>`).
pub const ATTRIBUTE_DOCKER_REMOTE_API_1_18: &str =
    "com.amazonaws.ecs.capability.docker-remote-api.1.18";

/// The attribute a container needs to read a Secrets Manager secret into an environment
/// variable (`Secret$valueFrom` holding a `secretsmanager` ARN).
pub const ATTRIBUTE_SECRETS_ASM_ENVIRONMENT: &str =
    "ecs.capability.secrets.asm.environment-variables";

/// The same, for a Systems Manager Parameter Store parameter.
pub const ATTRIBUTE_SECRETS_SSM_ENVIRONMENT: &str =
    "ecs.capability.secrets.ssm.environment-variables";

/// The attribute a container needs to read a Secrets Manager secret into its log
/// driver's configuration (`LogConfiguration$secretOptions`).
pub const ATTRIBUTE_SECRETS_ASM_LOG_DRIVER: &str = "ecs.capability.secrets.asm.bootstrap.log-driver";

/// The same, for a Systems Manager Parameter Store parameter.
pub const ATTRIBUTE_SECRETS_SSM_LOG_DRIVER: &str = "ecs.capability.secrets.ssm.bootstrap.log-driver";

/// The attribute a container needs to pull an environment variable file out of Amazon
/// S3 (`EnvironmentFile$type` `s3`).
pub const ATTRIBUTE_ENV_FILES_S3: &str = "ecs.capability.env-files.s3";

/// The attribute a container instance needs to run a container with elevated privileges
/// (`ContainerDefinition$privileged`).
pub const ATTRIBUTE_PRIVILEGED_CONTAINER: &str = "com.amazonaws.ecs.capability.privileged-container";

/// The prefix of the attribute a container instance needs to run a named log driver;
/// the driver name (`LogConfiguration$logDriver`) completes it.
pub const ATTRIBUTE_LOGGING_DRIVER_PREFIX: &str = "com.amazonaws.ecs.capability.logging-driver.";

/// The prefix of the attribute a container instance needs to run a task's LOG ROUTER;
/// the router named by `FirelensConfiguration$type` ("The log router to use. The valid
/// values are `fluentd` or `fluentbit`") completes it, so each router kind is its own
/// capability.
pub const ATTRIBUTE_FIRELENS_PREFIX: &str = "ecs.capability.firelens.";

/// The attribute a container instance needs to let the task's EXECUTION ROLE carry the
/// credentials the `awslogs` log driver ships logs with — the driver is configured on the
/// agent's behalf, so the capability is required only where a revision pairs that driver
/// with a `TaskDefinition$executionRoleArn` ("the task execution role that grants the
/// Amazon ECS container agent permission to make Amazon Web Services API calls on your
/// behalf").
pub const ATTRIBUTE_EXECUTION_ROLE_AWSLOGS: &str = "ecs.capability.execution-role-awslogs";

/// The `LogConfiguration$logDriver` that ships container logs to CloudWatch Logs through
/// the agent, one of the `LogDriver` enum values.
pub const LOG_DRIVER_AWSLOGS: &str = "awslogs";

/// How many attributes ONE attribute write may carry.
/// `DeleteAttributes$attributes`: "You can specify up to 10 attributes for each
/// request" — the bound `PutAttributes$attributes` states for the same list ("You can
/// specify up to 10 attributes in a single call"), so both read it from here.
pub const MAX_ATTRIBUTES_PER_REQUEST: usize = 10;

/// `TargetNotFoundException$message`: the shape's own doc sentence, which names no
/// target (`smithy_query` query="shape" shape="TargetNotFoundException"). No recording
/// fixes this one; what the rule fixes is that the sentence is the SAME for every
/// request whose attribute names a resource the cluster does not hold.
pub const TARGET_NOT_FOUND_MESSAGE: &str = "The specified target wasn't found.";

/// `InvalidParameterException$message` for an `Attribute` of an attribute WRITE that
/// names no target at all. `DeleteAttributes$attributes`: "For custom attributes,
/// specify the attribute name and target ID" — an attribute with no `targetId` names no
/// resource to write to, so the refusal names the member that is missing.
pub const ATTRIBUTE_TARGET_ID_REQUIRED_MESSAGE: &str =
    "attributes.targetId is required to name the resource the attribute is on";

/// `InvalidParameterException$message` for an `Attribute` whose `targetId` is the short
/// form id of a resource with no `targetType` beside it. `Attribute$targetType`: "This
/// parameter is required if you use the short form ID for a resource instead of the full
/// ARN."
pub const ATTRIBUTE_TARGET_TYPE_REQUIRED_MESSAGE: &str =
    "attributes.targetType is required when targetId is the short form id of a resource";

/// `healthStatus` this emulator reports. The `Container$healthStatus` doc: "If health
/// checks aren't configured for this container in its task definition, then it reports
/// the health status as UNKNOWN." Nothing executes a container here, so no container
/// ever reports a health result and the status stays `UNKNOWN`.
pub const HEALTH_UNKNOWN: &str = "UNKNOWN";

/// `Service$status` for a live service — the member's doc: "The valid values are
/// `ACTIVE`, `DRAINING`, or `INACTIVE`."
pub const SERVICE_ACTIVE: &str = "ACTIVE";
/// See [`SERVICE_ACTIVE`]: the status of a service that is being drained away.
pub const SERVICE_DRAINING: &str = "DRAINING";
/// See [`SERVICE_ACTIVE`]: the status of a service that has been deleted.
pub const SERVICE_INACTIVE: &str = "INACTIVE";

/// `Deployment$status` for the service's current deployment — the member's doc:
/// "`PRIMARY`: The most recent deployment of a service."
pub const DEPLOYMENT_PRIMARY: &str = "PRIMARY";

/// `Deployment$status` for a deployment a newer one is replacing — the same doc:
/// "`ACTIVE`: A service deployment that still has running tasks, but are in the process of
/// being replaced with a new launch type, platform version, task definition, or Docker
/// image." A deployment with no task left is "completely replaced" (`INACTIVE`), so it is
/// no longer one of the service's deployments at all.
pub const DEPLOYMENT_ACTIVE: &str = "ACTIVE";

/// How many superseded deployments one service keeps at once.
///
/// A deployment leaves that list as soon as its replacement falls due, so a service holds
/// one or two of them in practice. The bound is what keeps a burst of updates inside a
/// single span from growing the record without limit; the NEWEST are kept, since they are
/// the ones still being replaced.
pub const MAX_SUPERSEDED_DEPLOYMENTS: usize = 10;

/// `Service$runStatus` for a service whose scheduler has nothing outstanding: everything
/// the service wants is in place.
///
/// The member is not in the Smithy model — it is one of the members the real service
/// returns beside the modelled ones — so its three values are the ones recorded responses
/// carry, and each reports what the scheduler still owes the service (see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_run_status`]).
pub const RUN_STATUS_STEADY_STATE: &str = "SteadyState";

/// See [`RUN_STATUS_STEADY_STATE`]: the scheduler still has work to do for this service —
/// a task or a task set it has to place or let go of.
pub const RUN_STATUS_MORE_WORK: &str = "MoreWork";

/// See [`RUN_STATUS_STEADY_STATE`]: the service record is the tombstone of a service whose
/// deletion has finished, which runs nothing at all.
pub const RUN_STATUS_TOMBSTONE: &str = "TOMBSTONE";

/// `Deployment$rolloutState` while the deployment is still placing tasks — the member's
/// doc: "When a service deployment is started, it begins in an `IN_PROGRESS` state."
pub const ROLLOUT_IN_PROGRESS: &str = "IN_PROGRESS";
/// `Deployment$rolloutState` once the service holds the tasks it wants — the same doc:
/// "When the service reaches a steady state, the deployment transitions to a
/// `COMPLETED` state."
pub const ROLLOUT_COMPLETED: &str = "COMPLETED";
/// `Deployment$rolloutState` of a rollout that will never hold them — the same doc: "If the
/// service fails to reach a steady state and circuit breaker is turned on, the deployment
/// transitions to a `FAILED` state. A deployment in `FAILED` state doesn't launch any new
/// tasks." The third and last member of `DeploymentRolloutState`
/// (`smithy_query(query="shape", shape="DeploymentRolloutState")` -> COMPLETED, FAILED,
/// IN_PROGRESS).
pub const ROLLOUT_FAILED: &str = "FAILED";

/// `ServiceDeploymentBrief$status` while the rollout this deployment records is still
/// under way — one of `ServiceDeploymentStatus`
/// (`smithy_query(query="shape", shape="ServiceDeploymentStatus")`), and the
/// service-deployment counterpart of the [`ROLLOUT_IN_PROGRESS`] the SAME rollout reports
/// on the service's own `Deployment`.
pub const SERVICE_DEPLOYMENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a deployment whose service
/// reached the steady state it was rolling out — the counterpart of
/// [`ROLLOUT_COMPLETED`].
pub const SERVICE_DEPLOYMENT_SUCCESSFUL: &str = "SUCCESSFUL";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a deployment that stopped
/// WITHOUT reaching that steady state, because a newer deployment replaced it or the
/// service it was rolling out was deleted. Those two are the only ways a rollout ends
/// early here: nothing else interrupts one, so no deployment is left open forever.
pub const SERVICE_DEPLOYMENT_STOPPED: &str = "STOPPED";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a rollout that is being UNDONE,
/// because the action taken on the lifecycle hook it paused at was
/// [`LIFECYCLE_HOOK_ACTION_ROLLBACK`] — "ROLLBACK - Rolls back the deployment to the
/// previous service revision."
pub const SERVICE_DEPLOYMENT_ROLLBACK_IN_PROGRESS: &str = "ROLLBACK_IN_PROGRESS";
/// See [`SERVICE_DEPLOYMENT_ROLLBACK_IN_PROGRESS`]: the status of a rollout whose rollback
/// has run its course, so the previous service revision is the one the service runs again.
pub const SERVICE_DEPLOYMENT_ROLLBACK_SUCCESSFUL: &str = "ROLLBACK_SUCCESSFUL";
/// See [`SERVICE_DEPLOYMENT_ROLLBACK_IN_PROGRESS`]: the status of a rollout whose rollback
/// could not be carried out, because there is no earlier service revision of the service
/// to put back.
pub const SERVICE_DEPLOYMENT_ROLLBACK_FAILED: &str = "ROLLBACK_FAILED";
/// See [`SERVICE_DEPLOYMENT_IN_PROGRESS`]: the status of a rollout a caller asked to stop
/// where it stands, from the instant the stop was asked for until it has been carried out
/// (`StopServiceDeployment`: "Stops an ongoing service deployment").
pub const SERVICE_DEPLOYMENT_STOP_REQUESTED: &str = "STOP_REQUESTED";
/// See [`SERVICE_DEPLOYMENT_STOP_REQUESTED`]: the same for a stop that puts the previous
/// service revision back — the rollback has been asked for and has not begun yet.
pub const SERVICE_DEPLOYMENT_ROLLBACK_REQUESTED: &str = "ROLLBACK_REQUESTED";

/// The `StopServiceDeploymentStopType` that undoes the rollout: "ROLLBACK - This option
/// rolls back the service deployment to the previous service revision. You can use this
/// option even if you didn't configure the service deployment for the rollback option."
///
/// Also the stop type a request that names none is stopped with, since
/// `StopServiceDeployment$stopType` documents it as the stop type the operation offers:
/// "The valid values are `ROLLBACK`."
pub const STOP_TYPE_ROLLBACK: &str = "ROLLBACK";

/// The other `StopServiceDeploymentStopType`: the rollout is stopped where it stands, with
/// no service revision put back, so the service keeps what the rollout had released by
/// then.
pub const STOP_TYPE_ABORT: &str = "ABORT";

/// The `DeploymentLifecycleHookTargetType` of a hook that PAUSES a rollout rather than
/// running custom logic: "PAUSE - Pauses the deployment at the specified lifecycle stage
/// until you call `ContinueServiceDeployment` to continue or roll back." The other target
/// type "invokes a Lambda function at the specified lifecycle stage", which nothing here
/// runs, so only a PAUSE hook stops a rollout in this emulator.
pub const LIFECYCLE_HOOK_TARGET_PAUSE: &str = "PAUSE";

/// `DeploymentLifecycleHookDetail$status` of a PAUSE hook the rollout has REACHED and that
/// is waiting for the action `ContinueServiceDeployment` takes — one of
/// `DeploymentLifecycleHookStatus`, and the state the operation's own doc describes: "the
/// deployment pauses and waits for an explicit action".
pub const LIFECYCLE_HOOK_AWAITING_ACTION: &str = "AWAITING_ACTION";

/// See [`LIFECYCLE_HOOK_AWAITING_ACTION`]: the status of a hook whose action has been
/// taken, so the wait is over.
pub const LIFECYCLE_HOOK_SUCCEEDED: &str = "SUCCEEDED";

/// See [`LIFECYCLE_HOOK_AWAITING_ACTION`]: the status of a hook whose wait never
/// completed, because the rollout it belonged to ended first — a newer deployment replaced
/// it, or the service it was rolling out was deleted.
pub const LIFECYCLE_HOOK_FAILED: &str = "FAILED";

/// See [`LIFECYCLE_HOOK_AWAITING_ACTION`]: the status of a hook nobody acted on in time —
/// `DeploymentLifecycleHookDetail$expiresAt`: "The time when the lifecycle hook times out.
/// If the hook has not been completed by this time, Amazon ECS takes the timeout action."
pub const LIFECYCLE_HOOK_TIMED_OUT: &str = "TIMED_OUT";

/// The `DeploymentLifecycleHookAction` that resumes a paused rollout: "CONTINUE - Proceeds
/// the deployment to the next lifecycle stage". Also the action a request that names none
/// takes, from `ContinueServiceDeployment$action`: "If no value is specified, the default
/// action is CONTINUE."
pub const LIFECYCLE_HOOK_ACTION_CONTINUE: &str = "CONTINUE";

/// The `DeploymentLifecycleHookAction` that undoes it: "ROLLBACK - Rolls back the
/// deployment to the previous service revision."
pub const LIFECYCLE_HOOK_ACTION_ROLLBACK: &str = "ROLLBACK";

/// How long Amazon ECS waits for a lifecycle hook that configured no timeout, from
/// `DeploymentLifecycleHookTimeoutConfiguration$timeoutInMinutes`: "The number of minutes
/// Amazon ECS waits for the lifecycle hook to complete before taking the timeout action.
/// Default: 1440 (24 hours)".
pub const DEFAULT_LIFECYCLE_HOOK_TIMEOUT_MINUTES: i32 = 1440;

/// What Amazon ECS does with a hook that configured no timeout action, from that
/// configuration's `action` doc: "Default: ROLLBACK".
pub const DEFAULT_LIFECYCLE_HOOK_TIMEOUT_ACTION: &str = LIFECYCLE_HOOK_ACTION_ROLLBACK;

/// The stages a deployment lifecycle hook can run at, in the order a rollout passes
/// through them.
///
/// The order is the one the `ServiceDeployment$lifecycleStage` doc lists the stages in
/// (`smithy_query(query="doc", shape="ServiceDeployment", member="lifecycleStage")`),
/// narrowed to the eight a hook may name (`DeploymentLifecycleHookStage`, whose generated
/// `DEPLOYMENT_LIFECYCLE_HOOK_STAGE_VALUES` lists the same eight alphabetically). A rollout
/// configured to pause at several of them reaches the earliest of those first.
pub const LIFECYCLE_HOOK_STAGE_ORDER: [&str; 8] = [
    "RECONCILE_SERVICE",
    "PRE_SCALE_UP",
    "POST_SCALE_UP",
    "TEST_TRAFFIC_SHIFT",
    "POST_TEST_TRAFFIC_SHIFT",
    "PRE_PRODUCTION_TRAFFIC_SHIFT",
    "PRODUCTION_TRAFFIC_SHIFT",
    "POST_PRODUCTION_TRAFFIC_SHIFT",
];

/// How far through [`LIFECYCLE_HOOK_STAGE_ORDER`] `stage` is, or `None` for a stage that
/// order does not name (one a newer model adds, which is then reached after every stage it
/// does name).
fn lifecycle_stage_index(stage: &str) -> Option<usize> {
    LIFECYCLE_HOOK_STAGE_ORDER
        .iter()
        .position(|named| *named == stage)
}

/// `ListServiceDeployments`'s page size when the caller gives no `maxResults`, from that
/// member's own doc: "This value can be between 1 and 100. If this parameter isn't used,
/// then ListServiceDeployments returns up to 20 results and a nextToken value if
/// applicable." The upper bound is the shared [`MAX_LIST_PAGE_SIZE`].
pub const DEFAULT_LIST_SERVICE_DEPLOYMENTS_PAGE_SIZE: usize = 20;

/// How many service deployments ONE `DescribeServiceDeployments` request may name, from
/// the `serviceDeploymentArns` doc: "The ARN of the service deployment. You can specify a
/// maximum of 20 ARNs."
pub const MAX_DESCRIBE_SERVICE_DEPLOYMENTS: usize = 20;

/// How many service revisions ONE `DescribeServiceRevisions` request may name, from the
/// `serviceRevisionArns` doc: "The ARN of the service revision. You can specify a maximum
/// of 20 ARNs."
pub const MAX_DESCRIBE_SERVICE_REVISIONS: usize = 20;

/// `ServiceDeploymentAlarms$status` and `ServiceDeploymentCircuitBreaker$status` for a
/// failure-detection mechanism the deployment's configuration did not turn on — one of
/// `ServiceDeploymentRollbackMonitorsStatus`, whose doc reads "Amazon ECS is not using
/// alarms for service deployment failures when the status is `DISABLED`".
pub const ROLLBACK_MONITORS_DISABLED: &str = "DISABLED";

/// See [`ROLLBACK_MONITORS_DISABLED`]: a mechanism that IS turned on, watching a rollout
/// that has not ended yet.
pub const ROLLBACK_MONITORS_MONITORING: &str = "MONITORING";

/// See [`ROLLBACK_MONITORS_DISABLED`]: a mechanism that was turned on and whose watch is
/// over, because the rollout it was watching has ended.
pub const ROLLBACK_MONITORS_MONITORING_COMPLETE: &str = "MONITORING_COMPLETE";

/// See [`ROLLBACK_MONITORS_DISABLED`]: a mechanism that was turned on and FIRED — the
/// remaining member of `ServiceDeploymentRollbackMonitorsStatus`
/// (`smithy_query(query="shape", shape="ServiceDeploymentRollbackMonitorsStatus")` ->
/// TRIGGERED, MONITORING, MONITORING_COMPLETE, DISABLED), reported by exactly the mechanism
/// that failed the rollout it was watching.
pub const ROLLBACK_MONITORS_TRIGGERED: &str = "TRIGGERED";

/// The `ThresholdType` that uses the threshold configuration's `value` as the failure
/// threshold itself: "For `COUNT`, Amazon ECS uses `value` directly as the threshold."
pub const THRESHOLD_COUNT: &str = "COUNT";

/// The `ThresholdType` that reads `value` as a percentage of the desired count and leaves
/// the result unbounded: it multiplies "`value` by the latest service desired count,
/// without applying the `3`-to-`200` bounds".
pub const THRESHOLD_UNBOUNDED_PERCENT: &str = "UNBOUNDED_PERCENT";

/// The lowest failure threshold a [`THRESHOLD_BOUNDED_PERCENT`] circuit breaker computes.
/// That `ThresholdType` value's doc: Amazon ECS "calculates the failure threshold by
/// multiplying `value` by the latest service desired count, then clamping the result to a
/// minimum of `3` and a maximum of `200`".
pub const MIN_CIRCUIT_BREAKER_THRESHOLD: i32 = 3;

/// See [`MIN_CIRCUIT_BREAKER_THRESHOLD`]: the highest threshold that clamp allows.
pub const MAX_CIRCUIT_BREAKER_THRESHOLD: i32 = 200;

/// How many service deployments this emulator keeps for ONE service. EMULATOR-CHOSEN, for
/// the same reason as [`MAX_SERVICE_EVENTS`]: every `UpdateService` that forces a new
/// deployment appends one, so without a bound a caller could grow the in-memory store
/// without limit. The newest are the ones kept.
pub const MAX_SERVICE_DEPLOYMENTS_PER_SERVICE: usize = 100;

/// `deploymentConfiguration.strategy` a service takes when the request names none. The
/// `CreateService` doc: "When you create a service which uses the ECS deployment
/// controller, you can choose between the following deployment strategies ... ROLLING:
/// ... the Amazon ECS service scheduler replaces the currently running tasks with new
/// tasks" — the rolling update is the ECS controller's own deployment strategy, and the
/// other three (`BLUE_GREEN`, `LINEAR`, `CANARY`) each need configuration the request
/// has to supply.
pub const DEPLOYMENT_STRATEGY_ROLLING: &str = "ROLLING";

/// `bakeTimeInMinutes` of a deployment configuration the request gave none for. The
/// member's doc: "The time period when both blue and green service revisions are running
/// simultaneously after the production traffic has shifted. You must provide this
/// parameter when you use the BLUE_GREEN deployment strategy" — a deployment with no
/// bake time configured bakes for no time at all.
pub const DEFAULT_BAKE_TIME_MINUTES: i32 = 0;

/// The inclusive range `bakeTimeInMinutes` admits. The sibling member that states the same
/// wait for a LINEAR deployment's steps publishes the bound — `LinearConfiguration
/// $stepBakeTimeInMinutes`: "Valid values are 0 to 1440 minutes (24 hours)" — and a bake
/// time is a wait of the same kind, so one day is as long as a deployment may bake. The
/// RECORDED refusal is what fixes it as this member's bound too: a create naming a bake time
/// outside it is answered 400 with the sentence [`bake_time_message`] renders.
pub const BAKE_TIME_RANGE_MINUTES: (i32, i32) = (0, 1440);

/// `InvalidParameterException$message` for a `bakeTimeInMinutes` outside
/// [`BAKE_TIME_RANGE_MINUTES`]: Amazon ECS states the whole range and the deployment
/// STRATEGY the configuration asked for, and echoes no value from the request.
fn bake_time_message(strategy: &str) -> String {
    let (min, max) = BAKE_TIME_RANGE_MINUTES;
    format!("The bake time must be between {min} and {max} for {strategy} deployment strategy.")
}

/// `thresholdConfiguration.type` of a deployment circuit breaker the request gave none
/// for. The `ThresholdConfiguration` doc: "By default, the threshold configuration uses
/// a type of BOUNDED_PERCENT with a value of 50."
pub const THRESHOLD_BOUNDED_PERCENT: &str = "BOUNDED_PERCENT";
/// See [`THRESHOLD_BOUNDED_PERCENT`]: the default `value` of that threshold.
pub const THRESHOLD_DEFAULT_VALUE: i32 = 50;
/// `resetOnHealthyTask` of a deployment circuit breaker the request gave none for. The
/// member's doc: "When set to true, a task that reaches a healthy state resets the
/// failure count to 0. ... The default is true."
pub const CIRCUIT_BREAKER_RESETS_ON_HEALTHY_TASK: bool = true;

/// `healthCheckGracePeriodSeconds` a CREATE takes when the request names none. The
/// member's doc: "If you do not specify a health check grace period value, the default
/// value of 0 is used."
pub const DEFAULT_HEALTH_CHECK_GRACE_PERIOD_SECONDS: i32 = 0;

/// The resource part of the ECS service-linked role's ARN. The `CreateService$role` doc:
/// "If your account has already created the Amazon ECS service-linked role, that role is
/// used for your service unless you specify a role here. The service-linked role is
/// required if ... the service is configured to use ... an external deployment
/// controller ... in which case you don't specify a role here."
pub const SERVICE_LINKED_ROLE_RESOURCE: &str =
    "role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS";

/// How many of a service's events a response carries. The `Service$events` doc: "The
/// event stream for your service. A maximum of 100 of the latest events are displayed."
pub const MAX_SERVICE_EVENTS: usize = 100;

/// `TaskSet$status` for a task set that is NOT serving production traffic — the status
/// every task set a client creates starts in. The member's doc: "PRIMARY: The task set
/// is serving production traffic. ACTIVE: The task set isn't serving production traffic",
/// and `UpdateServicePrimaryTaskSet` is what promotes one ("Modifies which task set in a
/// service is the primary task set").
pub const TASK_SET_ACTIVE: &str = "ACTIVE";

/// `TaskSet$status` for the task set that IS serving production traffic.
pub const TASK_SET_PRIMARY: &str = "PRIMARY";

/// `TaskSet$status` for a task set whose tasks are being taken away. The member's doc:
/// "DRAINING: The tasks in the task set are being stopped, and their corresponding targets
/// are being deregistered from their target group" — which is what the delete of the
/// service that holds the task set starts.
pub const TASK_SET_DRAINING: &str = "DRAINING";

/// `StabilityStatus` for a task set that has reached its steady state. The
/// `TaskSet$stabilityStatus` doc: the task set is in `STEADY_STATE` when "The task
/// `runningCount` is equal to the `computedDesiredCount`" and "The `pendingCount` is 0";
/// "If any of those conditions aren't met, the stability status returns STABILIZING."
pub const STABILITY_STATUS_STEADY_STATE: &str = "STEADY_STATE";

/// See [`STABILITY_STATUS_STEADY_STATE`]: the task set has not reached its steady state.
pub const STABILITY_STATUS_STABILIZING: &str = "STABILIZING";

/// The only unit a scale is measured in, from the `ScaleUnit` enum (`PERCENT`), and so
/// the unit a stored scale carries when the request named none.
pub const SCALE_UNIT_PERCENT: &str = "PERCENT";

/// The lowest `Scale$value` a task set may carry. The member's doc: "The value,
/// specified as a percent total of a service's `desiredCount`, to scale the task set.
/// Accepted values are numbers between 0 and 100."
pub const MIN_SCALE_PERCENT: f64 = 0.0;

/// The highest `Scale$value` a task set may carry — see [`MIN_SCALE_PERCENT`].
pub const MAX_SCALE_PERCENT: f64 = 100.0;

/// How many task sets one service may hold. The `CreateTaskSet` doc points at the quota
/// ("For information about the maximum number of task sets and other quotas, see Amazon
/// ECS service quotas"), where "Task sets per service" is 5.
pub const MAX_TASK_SETS_PER_SERVICE: usize = 5;

/// `TaskSetNotFoundException$message`, BUILT FROM THE REQUEST: Amazon ECS names the task
/// set the caller asked for and the service it looked for it in, so a client that
/// addressed the wrong service can tell which of the two was wrong. The id is the one the
/// reference named ([`task_set_id_from_reference`]), so an ARN reference reads as the id
/// it carries. Shared by every operation that addresses one task set, so none of them
/// words the class differently.
pub fn task_set_not_found_message(reference: &str, service_name: &str) -> String {
    format!(
        "Unable to find task set with id {} on service {service_name}.",
        task_set_id_from_reference(reference)
    )
}

/// `TaskSetNotFoundException$message` as `UpdateTaskSet` words the same condition, BUILT
/// FROM THE REQUEST: Amazon ECS states this refusal per operation, and the sentence this
/// one carries names the task set first and the service it was looked for in second. The
/// id is the one the reference named ([`task_set_id_from_reference`]), so an ARN reference
/// reads as the id it carries.
pub fn task_set_does_not_exist_message(reference: &str, service_name: &str) -> String {
    format!(
        "TaskSet: {} does not exist in Service: {service_name}.",
        task_set_id_from_reference(reference)
    )
}

/// `InvalidParameterException$message` for a write to a task set whose STORED status does
/// not take one: the status is read from state, so the caller is told which status stopped
/// the write and can tell "wait for it" from "it is gone".
pub fn task_set_status_not_updatable_message(status: &str) -> String {
    format!("Unable to update taskset with status = {status}")
}

/// Why the task set in `status` cannot be scaled, or `None` when a task set in it can.
///
/// `TaskSet$status`: "PRIMARY: The task set is serving production traffic. ACTIVE: The
/// task set isn't serving production traffic. DRAINING: The tasks in the task set are
/// being stopped" — a task set in either of the first two is one of the service's
/// deployment, so an update holds it at the percentage it asks for; a task set that is
/// being taken away is not one to hold at any percentage, and is still PRESENT, so the
/// refusal is about the parameter and not about a resource that is absent.
pub fn task_set_status_not_updatable(status: &str) -> Option<String> {
    (status != TASK_SET_PRIMARY && status != TASK_SET_ACTIVE)
        .then(|| task_set_status_not_updatable_message(status))
}

/// `InvalidParameterException$message` when `DeleteTaskSet` is asked to delete the task
/// set that is serving the service's production traffic without saying `force`.
///
/// `TaskSet$status` PRIMARY: "The task set is serving production traffic" — taking it away
/// would leave the service's traffic unserved, so the delete is refused unless the request
/// insists on it ("If `true`, you can delete a task set even if it hasn't been scaled down
/// to zero"). The sentence is the one the recorded refusal carries, and it names no
/// resource from the request.
pub const PRIMARY_TASK_SET_NOT_DELETABLE_MESSAGE: &str = "Primary TaskSet cannot be deleted";

/// `Task$stoppedReason` of a task the deletion of its task set stopped: the scheduler's
/// account of why the task was taken away, naming the task set whose deletion took it —
/// the same shape as the one a deleted service's tasks carry.
pub fn task_set_deleted_reason(task_set_id: &str) -> String {
    format!("Task set {task_set_id} was deleted")
}

/// `TaskSet$startedBy` for a task set CodeDeploy created: "If an CodeDeploy deployment
/// created the task set, the `startedBy` parameter is `CODE_DEPLOY`. If an external
/// deployment created the task set, the `startedBy` field isn't used."
pub const TASK_SET_STARTED_BY_CODE_DEPLOY: &str = "CODE_DEPLOY";

/// Why a `Scale` a request carries cannot be stored, or `None` when it can.
///
/// `Scale$value`: "Accepted values are numbers between 0 and 100" — a percentage of the
/// service's desired count, so a value outside that range describes no task set. The
/// bound is reported as the double it is, the way the refused percentage reads.
pub fn scale_violation(scale: &Scale) -> Option<String> {
    if scale.value > MAX_SCALE_PERCENT {
        return Some(format!(
            "TaskSet Scale value for percent can not be greater than {MAX_SCALE_PERCENT:.1}"
        ));
    }
    if scale.value < MIN_SCALE_PERCENT {
        return Some(format!(
            "TaskSet Scale value for percent can not be less than {MIN_SCALE_PERCENT:.1}"
        ));
    }
    None
}

/// The scale a task set is STORED with: the one the request carried, with the only unit
/// the `ScaleUnit` enum has filled in, and `Scale$value`'s own modelled default (`0`) for
/// a request that named no scale at all.
pub fn stored_scale(requested: Option<&Scale>) -> Scale {
    Scale {
        value: requested.map(|scale| scale.value).unwrap_or_default(),
        unit: Some(SCALE_UNIT_PERCENT.to_string()),
    }
}

/// The service event the scheduler writes when it wants to place a task and cannot.
///
/// The Amazon ECS service event message for a placement that finds no instance to run
/// on, whose `reason` sentence says WHICH capacity was missing — the same sentence
/// `RunTask` refuses with, since both describe the one condition.
pub fn unable_to_place_message(service_name: &str, reason: &str) -> String {
    format!(
        "(service {service_name}) was unable to place a task because no container instance \
         met all of its requirements. Reason: {reason} For more information, see the \
         Troubleshooting section of the Amazon ECS Developer Guide."
    )
}

/// The scheduling strategy a service takes when the request names none. The
/// `CreateService$desiredCount` doc — "This is required if schedulingStrategy is
/// `REPLICA` or isn't specified" — makes `REPLICA` the unspecified case.
pub const SCHEDULING_REPLICA: &str = "REPLICA";
/// See [`SCHEDULING_REPLICA`]: "The daemon scheduling strategy deploys exactly one task
/// on each active container instance".
pub const SCHEDULING_DAEMON: &str = "DAEMON";

/// The deployment controller a service takes when the request names none. The
/// `CreateService$deploymentController` doc: "If no deployment controller is specified,
/// the default value of ECS is used."
pub const DEPLOYMENT_CONTROLLER_ECS: &str = "ECS";

/// The deployment controller CodeDeploy drives, from the `DeploymentControllerType` enum.
/// Named here because the service it controls must carry a load balancer: CodeDeploy
/// shifts traffic between two target groups, which only a listener can reach.
pub const DEPLOYMENT_CONTROLLER_CODE_DEPLOY: &str = "CODE_DEPLOY";

/// The deployment controller a third party drives, from the `DeploymentControllerType`
/// enum. The `CreateTaskSet` doc: "This is used when a service uses the EXTERNAL
/// deployment controller type", so a service under this controller takes its tasks from
/// the task sets a client creates rather than from its own scheduler.
pub const DEPLOYMENT_CONTROLLER_EXTERNAL: &str = "EXTERNAL";

/// `propagateTags` when the request names none. The member's doc: "If no value is
/// specified, the tags aren't propagated. ... The default is NONE."
pub const PROPAGATE_TAGS_NONE: &str = "NONE";
/// See [`PROPAGATE_TAGS_NONE`]: propagate the SERVICE's tags to its tasks.
pub const PROPAGATE_TAGS_SERVICE: &str = "SERVICE";
/// See [`PROPAGATE_TAGS_NONE`]: propagate the task DEFINITION's tags to the task.
pub const PROPAGATE_TAGS_TASK_DEFINITION: &str = "TASK_DEFINITION";

/// `InvalidParameterException$message` for a placement that asks for the SERVICE tag set
/// on a task no service placed. `RunTask$propagateTags`: "An error will be received if
/// you specify the SERVICE option when running a task" — a task a client places itself
/// belongs to no service, so there is no service tag set to copy from, whichever
/// operation places it.
pub const PROPAGATE_TAGS_SERVICE_MESSAGE: &str =
    "propagateTags must not be SERVICE when running a task";

/// The tag set a task a CLIENT places carries: what `propagate_tags` copies from the
/// revision it runs, plus the `requested` tags the call carried.
///
/// "Specifies whether to propagate the tags from the task definition to the task. If no
/// value is specified, the tags aren't propagated" — so only `TASK_DEFINITION` copies
/// anything, and a key that is already on the task is not written twice: "each tag key
/// must be unique, and each tag key can have only one value". Shared by every operation
/// that places a standalone task, so two placements cannot tag one differently.
pub fn standalone_task_tags(
    propagate_tags: Option<&str>,
    definition: &TaskDefinitionRecord,
    requested: &[Tag],
) -> Vec<Tag> {
    let mut tags = Vec::new();
    if propagate_tags == Some(PROPAGATE_TAGS_TASK_DEFINITION) {
        tags.extend(definition.tags.iter().cloned());
    }
    for tag in requested {
        if !tags.iter().any(|existing: &Tag| existing.key == tag.key) {
            tags.push(tag.clone());
        }
    }
    tags
}

/// `availabilityZoneRebalancing` a CREATE takes when the request names none — the
/// member's doc: "For create service requests, when no value is specified for
/// AvailabilityZoneRebalancing, Amazon ECS defaults the value to ENABLED."
pub const AZ_REBALANCING_ENABLED: &str = "ENABLED";

/// `maximumPercent` of a `REPLICA` service's deployment configuration when the request
/// supplies none. The member's doc: "The default maximumPercent value for a service
/// using the REPLICA service scheduler is 200%."
pub const REPLICA_MAXIMUM_PERCENT: i32 = 200;
/// `minimumHealthyPercent` of a `REPLICA` service's deployment configuration when the
/// request supplies none. The member's doc: "The default value for a replica service
/// for minimumHealthyPercent is 100%."
pub const REPLICA_MINIMUM_HEALTHY_PERCENT: i32 = 100;
/// `maximumPercent` of a `DAEMON` service: a daemon runs exactly one task per container
/// instance, so a deployment cannot exceed the desired count.
pub const DAEMON_MAXIMUM_PERCENT: i32 = 100;
/// `minimumHealthyPercent` of a `DAEMON` service's deployment configuration when the
/// request supplies none. The member's doc: "The default minimumHealthyPercent value
/// for a service using the DAEMON service schedule is 0% for the CLI, the Amazon Web
/// Services SDKs, and the APIs".
pub const DAEMON_MINIMUM_HEALTHY_PERCENT: i32 = 0;

/// The smallest `minimumHealthyPercent` a deployment configuration may name. The member
/// is "a lower limit on the number of your service's tasks that must remain in the
/// RUNNING state during a deployment, AS A PERCENTAGE of the desiredCount": a percentage
/// of a count is never negative.
pub const MIN_MINIMUM_HEALTHY_PERCENT: i32 = 0;

/// The largest `minimumHealthyPercent` a deployment configuration may name: the member
/// bounds how much of the `desiredCount` has to stay RUNNING, and all of it is 100% —
/// which is also the value its own doc gives as the replica default ("The default value
/// for a replica service for minimumHealthyPercent is 100%"), the top of the range.
pub const MAX_MINIMUM_HEALTHY_PERCENT: i32 = 100;

/// `InvalidParameterException$message` for a `minimumHealthyPercent` outside its range.
/// Amazon ECS states the member and the ONE bound the value crossed, and echoes neither
/// the value the request carried nor the other bound.
fn minimum_healthy_percent_message(bound: &str, limit: i32) -> String {
    format!("minimumHealthyPercent must be at {bound} {limit}")
}

/// `InvalidParameterException$message` for a create that names a `taskDefinition` for a
/// service whose deployment controller takes the revision from a TASK SET instead.
///
/// "A task definition must be specified if the service uses either the ECS or CODE_DEPLOY
/// deployment controllers" — so under the remaining controller the member belongs to
/// `CreateTaskSet`, not here. One fixed sentence, naming nothing from the request.
pub const TASK_DEFINITION_MUST_BE_BLANK_MESSAGE: &str = "TaskDefinition must be blank.";

/// The smallest `maximumPercent` a deployment configuration may name. The member is "an
/// upper limit on the number of your service's tasks that are allowed in the RUNNING or
/// PENDING state during a deployment, as a percentage of the desiredCount": a deployment
/// that may not even run the count the service wants could never finish, so the limit
/// starts at all of it — 100%, which is what a DAEMON service's deployment is held to and
/// the floor the REPLICA default (200%) sits above. The RECORDED refusal is what fixes the
/// floor at 100 rather than at some other share: a create naming less is answered 400 with
/// the sentence [`maximum_percent_message`] renders.
pub const MIN_MAXIMUM_PERCENT: i32 = 100;

/// `InvalidParameterException$message` for a `maximumPercent` outside its range, worded as
/// the sibling percentage's refusal is: the member and the ONE bound the value crossed,
/// echoing neither the value the request carried nor the other bound.
fn maximum_percent_message(bound: &str, limit: i32) -> String {
    format!("maximumPercent must be at {bound} {limit}")
}

/// The refusal a requested `deploymentConfiguration` earns when a percentage it names is
/// outside the range that percentage admits, or `None` when every member is in range.
///
/// `maximumPercent` is an UPPER limit on the tasks a deployment may run, which its own doc
/// puts at or above the desired count ("a maximumPercent value of 200%"), so a value under
/// [`MIN_MAXIMUM_PERCENT`] is refused and a value over it is one the member is for.
/// `minimumHealthyPercent` is a share of the desired count, so it is bounded at both ends.
///
/// Spelled once for every operation that takes a deployment configuration, so a
/// configuration one of them refuses cannot be written by another.
pub fn deployment_configuration_violation(
    configuration: &DeploymentConfiguration,
) -> Option<String> {
    // "The time period when both blue and green service revisions are running
    // simultaneously": a wait the deployment makes, bounded like every other bake time the
    // configuration carries, and reported against the strategy the deployment runs under —
    // the one the request named, else the rolling update the ECS controller performs.
    if let Some(minutes) = configuration.bake_time_in_minutes {
        let (min, max) = BAKE_TIME_RANGE_MINUTES;
        if !(min..=max).contains(&minutes) {
            return Some(bake_time_message(
                configuration
                    .strategy
                    .as_deref()
                    .unwrap_or(DEPLOYMENT_STRATEGY_ROLLING),
            ));
        }
    }
    if let Some(percent) = configuration.maximum_percent {
        if percent < MIN_MAXIMUM_PERCENT {
            return Some(maximum_percent_message("least", MIN_MAXIMUM_PERCENT));
        }
    }
    let percent = configuration.minimum_healthy_percent?;
    if percent < MIN_MINIMUM_HEALTHY_PERCENT {
        return Some(minimum_healthy_percent_message(
            "least",
            MIN_MINIMUM_HEALTHY_PERCENT,
        ));
    }
    if percent > MAX_MINIMUM_HEALTHY_PERCENT {
        return Some(minimum_healthy_percent_message(
            "most",
            MAX_MINIMUM_HEALTHY_PERCENT,
        ));
    }
    None
}

/// `stopCode` for a task the service scheduler stopped (scaling in, or replacing the
/// task set of a superseded deployment), from the `TaskStopCode` enum.
pub const STOP_CODE_SERVICE_SCHEDULER: &str = "ServiceSchedulerInitiated";

/// `Task$stoppedReason` for a service task the scheduler stopped because the container
/// instance hosting it is being taken out of service.
///
/// `UpdateContainerInstancesState`: setting an instance to `DRAINING` is what makes Amazon
/// ECS stop the service tasks on it and start "replacement service tasks ... on other
/// container instances in the cluster", so the reason states the host, not the service.
/// Fixed text, naming nothing of the request.
pub const DRAINING_INSTANCE_STOP_REASON: &str =
    "Task stopped because the container instance it was running on is draining";


/// `resourceManagementType` of a service a client created through `CreateService`. The
/// member's doc: "Identifies whether an ECS Service is an Express Service managed by
/// ECS, or managed by the customer. The valid values are ECS and CUSTOMER."
pub const RESOURCE_MANAGEMENT_CUSTOMER: &str = "CUSTOMER";

/// How many services this emulator holds, across every cluster. EMULATOR-CHOSEN, for
/// the same reason as [`MAX_CLUSTERS`].
pub const MAX_SERVICES: usize = 64;

/// The longest a `clientToken` may be: "Up to 36 ASCII characters in the range of
/// 33-126 (inclusive) are allowed" — the `CreateService$clientToken` doc.
pub const MAX_CLIENT_TOKEN_LENGTH: usize = 36;

/// How many services one `DescribeServices` request may name. The member's doc: "You
/// may specify up to 10 services to describe in a single operation."
pub const MAX_DESCRIBE_SERVICES: usize = 10;

/// How many task-definition revisions one `DeleteTaskDefinitions` request may name. The
/// `DeleteTaskDefinitionsRequest$taskDefinitions` doc: "You can specify up to 10 task
/// definitions as a comma separated list."
pub const MAX_DELETE_TASK_DEFINITIONS: usize = 10;

/// How many tags ONE resource may carry, whoever wrote them. The `Tag` doc's first
/// restriction: "Maximum number of tags per resource - 50" — the same number the `Tags`
/// list's own length constraint caps a single request at, which is why it bites only
/// where a request ADDS to a set a resource already carries.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

// The message of a refusal whose condition is that a NAMED resource does not exist.
// Amazon ECS answers most of these with ONE fixed sentence that names no resource: the
// same text for every request that meets the condition, whatever cluster, service, task
// or task definition the request named. Every such refusal in this crate therefore
// carries the constant below rather than a sentence built from the request — a refusal
// ECS DERIVES from the request (a member that is blank or outside its documented
// constraint, or a container-instance reference that resolves to nothing — see
// [`container_instance_not_found_message`]) names the member or the value instead, and
// is built by a function rather than being a constant.

/// `ClusterNotFoundException$message` for a `cluster` that names no stored cluster at all.
pub const CLUSTER_NOT_FOUND_MESSAGE: &str = "Cluster not found.";

/// `ServiceNotFoundException$message` for a `service` that names no stored service.
pub const SERVICE_NOT_FOUND_MESSAGE: &str = "Service not found.";

/// `ServiceNotActiveException$message` for a `service` this store HOLDS in a status that
/// is not [`SERVICE_ACTIVE`] — the error shape's own doc: "The specified service isn't
/// active."
///
/// Like its neighbours above it is ONE fixed sentence for the condition: it names neither
/// the service nor the cluster the request carried, and it does not report WHICH
/// non-ACTIVE status ([`SERVICE_DRAINING`] or [`SERVICE_INACTIVE`]) the service holds, so
/// every operation that resolves a service and finds it not active answers the same text.
pub const SERVICE_NOT_ACTIVE_MESSAGE: &str = "Service was not ACTIVE.";

/// `InvalidParameterException$message` for a `task` that names no stored task.
pub const TASK_NOT_FOUND_MESSAGE: &str = "The referenced task was not found";

/// `InvalidParameterException$message` for a `DeleteAccountSetting` whose principal holds
/// no setting of the name it was asked to disable. The operation "disables an account
/// setting for a specified user, role, or the root user", so a principal that holds none
/// of that name has nothing to disable — one fixed sentence naming neither the setting nor
/// the principal the request carried.
pub const ACCOUNT_SETTING_NOT_FOUND_MESSAGE: &str = "Referenced setting doesn't exist";

/// `ClientException$message` for a `taskDefinition` that names no stored revision, as the
/// operations that READ a revision state it — `DescribeTaskDefinition`,
/// `DeregisterTaskDefinition` and a tagging call naming a revision by ARN.
///
/// An operation that resolves a reference in order to PUT the revision to work states the
/// same condition in its own words ([`TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE`]): the
/// sentence belongs to the call that was refused, not to the store it looked in.
pub const TASK_DEFINITION_NOT_FOUND_MESSAGE: &str = "Unable to describe task definition.";

/// `ClientException$message` for a `taskDefinition` that names no stored revision on a
/// call that would PUT that revision to work — run a task of it, or point a service or a
/// task set at it. ONE fixed sentence naming neither the family nor the revision the
/// request carried.
pub const TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE: &str = "TaskDefinition not found.";

/// `ClientException$message` for a `daemonTaskDefinition` that names no stored daemon
/// revision. The same condition as [`TASK_DEFINITION_NOT_FOUND_MESSAGE`] one resource
/// along, so like its neighbours it is ONE sentence whatever family and revision the
/// request named.
pub const DAEMON_TASK_DEFINITION_NOT_FOUND_MESSAGE: &str =
    "Unable to describe daemon task definition.";

/// `ClientException$message` for a `daemonTaskDefinitionArn` that names no stored daemon
/// revision on a call that would PUT that revision to work — create a daemon on it. The
/// daemon resource's form of [`TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE`]: one fixed
/// sentence naming neither the family nor the revision the request carried.
pub const DAEMON_TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE: &str =
    "DaemonTaskDefinition not found.";

/// `InvalidParameterException$message` for a `daemonTaskDefinitionArn` that names a
/// revision this store HOLDS in a status that can run no new work — the daemon
/// resource's form of [`TASK_DEFINITION_INACTIVE_MESSAGE`], for the statuses a requested
/// delete moves a revision through ([`DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS`],
/// [`DAEMON_TASK_DEFINITION_DELETED`]).
pub const DAEMON_TASK_DEFINITION_INACTIVE_MESSAGE: &str = "DaemonTaskDefinition is inactive";

/// `InvalidParameterException$message` for a `taskDefinition` that names a revision this
/// store holds in a status that cannot run new work — the `DeregisterTaskDefinition` doc:
/// "You can't use an `INACTIVE` task definition to run new tasks or create new services,
/// and you can't update an existing service to reference an `INACTIVE` task definition."
///
/// A condition that names no resource, so like its neighbours above it is ONE sentence
/// whatever family and revision the request named.
pub const TASK_DEFINITION_INACTIVE_MESSAGE: &str = "TaskDefinition is inactive";

/// Why a `taskDefinition` reference a call would put to WORK cannot be used — the two
/// conditions `usable_revision` tells apart, each with the sentence it carries.
///
/// They are two DIFFERENT faults, so they are two different modelled errors: a reference
/// that resolves to nothing names an identifier this store knows nothing about, while one
/// that resolves to a revision the store HOLDS but may no longer run names an existing
/// resource in a status this call cannot accept — a parameter whose value is wrong for the
/// call, not an unknown identifier. A caller branches on the error code to decide what to
/// do next (register the family, or point the request at a live revision), which is why
/// the two never collapse onto one code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDefinitionRefusal {
    /// The reference resolves to no stored revision at all: `ClientException`, with
    /// [`TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE`].
    NotFound,
    /// The reference resolves to a stored revision whose status is not
    /// [`TASK_DEFINITION_ACTIVE`]: `InvalidParameterException`, with
    /// [`TASK_DEFINITION_INACTIVE_MESSAGE`].
    Inactive,
}

impl TaskDefinitionRefusal {
    /// The sentence this refusal carries, the same one whichever operation resolved the
    /// reference.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotFound => TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE,
            Self::Inactive => TASK_DEFINITION_INACTIVE_MESSAGE,
        }
    }
}

/// `ClientException$message` for a `nextToken` a paginated read cannot resume from: a
/// value the operation never minted. ONE fixed sentence naming the member and no value.
///
/// Which CLASS and which sentence a cursor refusal takes is a property of the OPERATION,
/// not of the condition: this is the one the task-definition, attribute and
/// service-deployment listings answer (the corpus pins `ListTaskDefinitions` to it),
/// while `ListClusters` answers [`invalid_next_token_message`] under
/// `InvalidParameterException`.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid nextToken.";

/// `InvalidParameterException$message` for a caller-supplied list that names more entries
/// than one request may carry: Amazon ECS states the list by the noun the member's own doc
/// uses for its entries, and the bound, in one sentence — "<entries> can have at most <N>
/// items." — echoing none of the values the list held.
///
/// Spelled once for every bounded list in this crate, so the refusal cannot drift between
/// the operations that carry one.
pub fn at_most_items_message(entries: &str, max: usize) -> String {
    format!("{entries} can have at most {max} items.")
}

/// The same sentence for a caller-supplied STRING longer than the member documents.
pub fn at_most_characters_message(member: &str, max: usize) -> String {
    format!("{member} can have at most {max} characters.")
}

/// How many network bindings one container may report.
/// `NetworkBinding$containerPortRange`: "You can specify a maximum of 100 port ranges per
/// container."
pub const MAX_NETWORK_BINDINGS: usize = 100;

/// How long a container state change's `reason` may be. `Container$reason`: "A short (255
/// max characters) human-readable string to provide additional details about a running or
/// stopped container."
pub const MAX_CONTAINER_REASON_LENGTH: usize = 255;

/// How many MANAGED AGENT entries one agent report may carry, and so the most a container
/// could hold.
///
/// A managed agent is one Amazon ECS runs inside the container for a feature the task asked
/// for, and `ManagedAgentStateChange$managedAgentName` targets the model's `ManagedAgentName`
/// enum — so a report can only ever name an agent that enum lists, and the record for one is
/// minted once. This bounds the caller-sized LIST itself, which is the work a single report
/// can ask for: EMULATOR-CHOSEN, for the same reason as [`MAX_TASKS`], and a few times the
/// one agent the model names.
pub const MAX_MANAGED_AGENTS: usize = 8;

/// `InvalidParameterException$message` for a `desiredCount` below the zero its own doc
/// bounds it at ("The desired number of instantiations of the task definition to keep
/// running on the service"). A fixed sentence: Amazon ECS names WHAT THE COUNT COUNTS —
/// the tasks the service keeps running — rather than the member that carried it, and the
/// bound the value crossed, echoing neither the number the request held nor how this
/// emulator read it. Same sentence for the create and for the update of a service: the
/// bound belongs to the count, not to the call.
pub const DESIRED_COUNT_NEGATIVE_MESSAGE: &str = "Task count can not be lesser than 0";

/// `InvalidParameterException$message` for a service whose `schedulingStrategy` needs a
/// `desiredCount` the request left out — the member's own doc: "This is required if
/// schedulingStrategy is REPLICA or isn't specified."
///
/// A fixed sentence stating the missing member, spelled as the shape names it: a required
/// member nothing supplied has no value to report, so the refusal carries none.
pub const DESIRED_COUNT_MISSING_MESSAGE: &str = "DesiredCount is missing";

/// `ClientException$message` for a registration whose `containerDefinitions` repeat a
/// container name. The name identifies the container within the revision — a
/// `loadBalancers` entry, a `dependsOn` entry and a `volumesFrom` entry all name one by it
/// — so one name can stand for only one container. A fixed sentence naming neither the
/// repeated name nor its position.
pub const DUPLICATE_CONTAINER_NAME_MESSAGE: &str =
    "Container names must be unique within a task definition.";

/// The smallest number of CPU units a container definition may reserve. `cpu` is "The
/// number of `cpu` units reserved for the container", a COUNT of units, and its own doc
/// describes what the agent does with "CPU values below 2 (including null)" and with "Null
/// and zero CPU values" — so reserving none is a value the member takes, while a count
/// below none names no number of units at all.
pub const MIN_CONTAINER_CPU_UNITS: i32 = 0;

/// The smallest amount of memory, in MiB, a container definition may name. `memory` is
/// "The amount (in MiB) of memory to present to the container" and `memoryReservation`
/// "The soft limit (in MiB) of memory to reserve for the container": an amount that is
/// presented to or reserved for a running container is at least one MiB of it, so neither
/// member names an amount at or below none.
pub const MIN_CONTAINER_MEMORY_MIB: i32 = 1;

/// `InvalidParameterException$message`/`ClientException$message` for a container
/// definition whose numeric reservation names a value its unit does not admit. Amazon ECS
/// names the offending MEMBER and the container the request gave it on, and carries
/// neither the value nor the bound.
pub fn invalid_container_setting_message(member: &str, container_name: &str) -> String {
    format!("Invalid '{member}' setting for container '{container_name}'.")
}

/// `ClientException$message` for a container definition whose SOFT memory limit is above
/// its HARD one. The `memoryReservation` doc: a container "can consume more memory when
/// it needs to, up to either the hard limit specified with the `memory` parameter", and
/// "if you specify both, `memory` must be greater than `memoryReservation`" — so a
/// reservation above the hard limit reserves memory the container may never use. The
/// sentence names the container the request gave the pair on and the two members, and
/// carries neither value; the limit it states is the one Amazon ECS enforces, so a pair
/// that is EQUAL is accepted.
pub fn container_memory_below_reservation_message(container_name: &str) -> String {
    format!(
        "Invalid setting for container '{container_name}'. 'memory' must be greater than \
         or equal to 'memoryReservation'."
    )
}

/// `ClientException$message` for a container definition that leaves a member a runnable
/// container cannot be built without — its `name`, which identifies it inside the
/// revision, or its `image`, which is what runs — unset or empty. Amazon ECS states the
/// member of the container structure in ONE fixed sentence per member, naming no position
/// in the list and echoing none of the request's values.
pub fn container_member_required_message(member: &str) -> String {
    format!("Container.{member} should not be null or empty.")
}

/// `ClientException$message` for a registration whose `containerDefinitions` describe no
/// container at all: "A list of container definitions ... that describe the different
/// containers that make up your task" — a task is made of containers, so an empty list
/// describes no task. One fixed sentence, carrying nothing of the request.
pub const EMPTY_CONTAINER_LIST_MESSAGE: &str = "Container list cannot be empty.";

/// The first of ONE container definition's numeric resource reservations that names a
/// value outside the range its unit admits, stated as
/// [`invalid_container_setting_message`] states it, or `None` when every one of them is in
/// range.
///
/// The members are examined in a FIXED order — the CPU count, then the hard memory amount,
/// then the soft one, then the two memory amounts AGAINST each other — so the refusal a
/// definition earns never depends on anything but its values. Spelled once for every
/// registration that carries container definitions, so the refusal cannot drift between
/// the plain and the daemon task definition.
pub fn container_reservation_violation(
    container_name: &str,
    cpu: i32,
    memory: Option<i32>,
    memory_reservation: Option<i32>,
) -> Option<String> {
    if cpu < MIN_CONTAINER_CPU_UNITS {
        return Some(invalid_container_setting_message("cpu", container_name));
    }
    for (member, amount) in [
        ("memory", memory),
        ("memoryReservation", memory_reservation),
    ] {
        if amount.is_some_and(|mib| mib < MIN_CONTAINER_MEMORY_MIB) {
            return Some(invalid_container_setting_message(member, container_name));
        }
    }
    // Each is in range on its own; they are only usable TOGETHER when the hard limit is
    // at least the soft one.
    if let (Some(memory), Some(reservation)) = (memory, memory_reservation) {
        if memory < reservation {
            return Some(container_memory_below_reservation_message(container_name));
        }
    }
    None
}

/// `InvalidParameterException$message` for an `UpdateService` that would change the
/// service's deployment controller while the service has no `PRIMARY` task set: the
/// controller that is taking the service over has to have one to take over, so the refusal
/// states the precondition and what the caller has to do about it.
pub const DEPLOYMENT_CONTROLLER_PRIMARY_TASK_SET_REQUIRED_MESSAGE: &str =
    "Updating the deployment controller requires a primary taskset in the service. Please \
     ensure your service has a primary taskset and try again.";

/// `ClientException$message` for a `taskDefinition` that is not a task-definition
/// reference at all — the condition [`parse_task_definition_reference`] answers `None`
/// for.
///
/// FIXED text, carrying nothing from the request: every form the member documents ("The
/// family and revision (family:revision) or full ARN of the task definition") carries a
/// FAMILY, and a value that is none of those forms is one whose family part is outside
/// the character set a family name is spelled in, which is what Amazon ECS states.
///
/// Every operation that resolves a `taskDefinition` reference reports this ONE sentence,
/// so the refusal cannot drift between the operation that reads a revision and the ones
/// that use it.
pub const TASK_DEFINITION_REFERENCE_MESSAGE: &str = "Family contains invalid characters.";

/// `ClientException$message` for a `daemonTaskDefinition` that is not a daemon
/// task-definition reference at all — the condition
/// [`parse_daemon_task_definition_reference`] answers `None` for. The DAEMON resource
/// states this condition in its own words (the sentence this crate's daemon suites pin),
/// which is why it is not the fixed [`TASK_DEFINITION_REFERENCE_MESSAGE`] the plain
/// resource answers.
pub fn daemon_task_definition_reference_message(reference: &str) -> String {
    revision_reference_message(
        "daemonTaskDefinition",
        DAEMON_TASK_DEFINITION_RESOURCE_TYPE,
        reference,
    )
}

/// `InvalidParameterException$message` for a `DeleteDaemonTaskDefinition` whose
/// `daemonTaskDefinition` identifies no daemon task definition at all. A fixed sentence
/// about the identifier the call acts on, carrying none of the request's values.
pub const DAEMON_TASK_DEFINITION_INVALID_MESSAGE: &str = "The Daemon Task Definition is invalid";

/// `InvalidParameterException$message` for a reference to a daemon task definition whose
/// FAMILY part is not a family name at all. A fixed sentence: it names the part of the
/// reference that is wrong and the pattern that part has to match — the character set
/// [`is_valid_name`] accepts, over one or more characters — and carries none of the
/// request's values.
pub const DAEMON_TASK_DEFINITION_FAMILY_PATTERN_MESSAGE: &str =
    r"Invalid daemon task definition: family must match ^[a-zA-Z0-9\-_]+$";

/// The refusal a reference to a daemon task definition earns when the FAMILY it names is
/// not a family name, or `None` when the family part is well-formed.
///
/// A reference is "The family for the latest ACTIVE revision, family and revision
/// (family:revision) for a specific revision in the family, or full Amazon Resource Name
/// (ARN) of the daemon task definition" — every form carries a FAMILY, and a family is "Up
/// to 255 letters (uppercase and lowercase), numbers, underscores, and hyphens". A value
/// whose family part is outside that set names no family, so it is refused as invalid
/// input stating the pattern a family has to match, BEFORE the store is consulted for a
/// revision.
///
/// `None` is also the answer for a value that is an ARN of some OTHER resource: its family
/// part is not what is wrong with it, so it earns the refusal for a reference that names
/// no daemon task definition ([`daemon_task_definition_reference_message`]).
pub fn daemon_task_definition_family_violation(reference: &str) -> Option<&'static str> {
    match split_revision_reference(reference, DAEMON_TASK_DEFINITION_RESOURCE_TYPE) {
        Some((family, _)) if !is_valid_name(family) => {
            Some(DAEMON_TASK_DEFINITION_FAMILY_PATTERN_MESSAGE)
        }
        _ => None,
    }
}

/// `InvalidParameterException$message` for a `taskDefinition` that names no revision
/// where the call acts on ONE revision — the `DeregisterTaskDefinition$taskDefinition`
/// doc: "You must specify a revision."
///
/// FIXED text, carrying nothing of the request: what the value misses is the revision,
/// whatever it spelled instead.
pub const TASK_DEFINITION_REVISION_MISSING_MESSAGE: &str = "Revision is missing";

/// `ClientException$message` for a `daemonTaskDefinition` that names a whole FAMILY where
/// the call needs the ONE revision it acts on — the `DeleteDaemonTaskDefinition$daemonTaskDefinition`
/// doc: "The family and revision (family:revision) or full Amazon Resource Name (ARN) of
/// the daemon task definition to delete", which unlike the reading operations' member
/// does not offer the family on its own.
pub fn daemon_task_definition_revision_required_message(reference: &str) -> String {
    revision_required_message(
        "daemonTaskDefinition",
        DAEMON_TASK_DEFINITION_RESOURCE_TYPE,
        reference,
    )
}

/// The sentence a reference that names a FAMILY is refused with where the call needs one
/// revision: the `member` it was given in, the forms that do name a revision and the
/// value that names none.
///
/// Spelled once for both resources whose references take those forms, so a call that acts
/// on one revision refuses a family the same way whichever resource it works on.
fn revision_required_message(member: &str, resource_type: &str, reference: &str) -> String {
    format!(
        "{member} must name a revision (family:revision or a {resource_type} ARN); \
         got {reference:?}"
    )
}

/// The sentence a reference that names no revision of `resource_type` is refused with:
/// the `member` it was given in, the forms that member documents and the value that is
/// none of them.
///
/// Spelled once for every member of this shape, so the two resources whose references
/// are "a family, a family:revision or a full ARN" refuse a value the same way.
fn revision_reference_message(member: &str, resource_type: &str, reference: &str) -> String {
    format!(
        "{member} must be a family, family:revision or {resource_type} ARN; \
         got {reference:?}"
    )
}

/// The container instance ID a `containerInstance`/`containerInstances` entry names: the
/// last segment of the value when it is an ARN
/// (`arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`), and the value
/// itself when the member carries the id — "The container instance ID or full ARN of the
/// container instance" are the two forms the member documents, and both name the same
/// instance.
pub fn container_instance_id_from_reference(reference: &str) -> &str {
    if reference.starts_with("arn:") {
        reference.rsplit('/').next().unwrap_or(reference)
    } else {
        reference
    }
}

/// `InvalidParameterException$message` for a `containerInstance` reference that names no
/// instance registered in the cluster, BUILT FROM THE REQUEST: Amazon ECS names the
/// container instance the request referred to — "Referenced container instance <id> not
/// found." — so a caller deregistering a batch can tell which entry was rejected. The
/// corpus records this refusal for `DeregisterContainerInstance` with a reference given
/// as a full ARN, and the recorded sentence carries the instance ID alone, which is why
/// the reference is reduced to its id by [`container_instance_id_from_reference`] rather
/// than echoed whole.
///
/// Every refusal in this crate whose condition is "this container-instance reference
/// resolves to no registered instance" goes through this one function, so the sentence
/// and the value it names are the same wherever the condition is met.
pub fn container_instance_not_found_message(reference: &str) -> String {
    format!(
        "Referenced container instance {} not found.",
        container_instance_id_from_reference(reference)
    )
}

/// `ClusterContainsServicesException$message`: the sentence the wire corpus records for
/// this refusal, which names no cluster. It is the RECORDED text rather than the shape's
/// doc paraphrase ("You can't delete a cluster that contains services."), because the
/// recording is what the wire carries.
pub const CLUSTER_CONTAINS_SERVICES_MESSAGE: &str =
    "The Cluster cannot be deleted while Services are active.";

/// `ClusterContainsTasksException$message`: the shape's own doc sentence, which names no
/// cluster (`smithy_query` query="shape" shape="ClusterContainsTasksException"). No
/// recording fixes this one; what the rule fixes is that the sentence is the SAME for
/// every request that meets the condition.
pub const CLUSTER_CONTAINS_TASKS_MESSAGE: &str = "You can't delete a cluster that has active tasks.";

/// `ClientException$message` when `DeleteCluster` is asked to delete a cluster that still
/// carries a CLUSTER-SCOPED capacity provider (an Amazon ECS Managed Instances provider,
/// which "can only be used within their associated cluster"). The sentence is fixed text,
/// naming neither the cluster nor the provider, and the refusal is a `ClientException`
/// rather than `ClusterContainsCapacityProviderException`: that shape is what an
/// ACCOUNT-scoped association earns, while a cluster-scoped provider is a resource of the
/// cluster itself.
pub const CLUSTER_CONTAINS_CLUSTER_SCOPED_PROVIDER_MESSAGE: &str =
    "The Cluster cannot be deleted while Cluster Scoped Capacity Providers are attached \
     to the cluster.";

/// `InvalidParameterException$message` when `DeleteService` is asked to delete a service
/// that is still scaled up. The operation's doc: "You can delete a service if you have no
/// running tasks in it and the desired task count is zero" — a condition of the service,
/// so the sentence names no resource either.
pub const SERVICE_SCALED_ABOVE_ZERO_MESSAGE: &str =
    "The service cannot be stopped while it is scaled above 0.";

/// The attachment id an `attachmentArn` names: the last segment of the value when it is
/// an ARN (`arn:aws:ecs:<region>:<account>:attachment/<id>`), and the value itself
/// otherwise — `Attachment$id` is "the unique identifier for the attachment", and both
/// forms name the same attachment.
pub fn attachment_id_from_reference(reference: &str) -> &str {
    if reference.starts_with("arn:") {
        reference.rsplit('/').next().unwrap_or(reference)
    } else {
        reference
    }
}

/// `InvalidParameterException$message` for an `attachmentArn` that names no stored
/// attachment, BUILT FROM THE REQUEST: Amazon ECS names the attachment the report
/// referred to, so an agent acknowledging a batch can tell which entry was rejected. The
/// sentence carries the attachment ID alone, which is why the reference is reduced to its
/// id by [`attachment_id_from_reference`] rather than echoed whole.
pub fn attachment_not_found_message(reference: &str) -> String {
    format!(
        "Attachment with id {} does not exist",
        attachment_id_from_reference(reference)
    )
}

/// `InvalidParameterException$message` for a launch of an `awsvpc` task definition that
/// carries no `networkConfiguration`. The condition names no resource — it is a property
/// of the pair (network mode, network configuration) — so every refusal of it, from
/// whichever operation launches the task, carries this one sentence.
pub const AWSVPC_NETWORK_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "Network Configuration must be provided when networkMode 'awsvpc' is specified.";

/// `InvalidParameterException$message` for a `CODE_DEPLOY`-controlled service that names
/// no load balancer. The `DeploymentController$type` doc describes the blue/green
/// deployment CodeDeploy drives by shifting traffic between two target groups, which
/// needs a load balancer in front of them.
pub const CODE_DEPLOY_LOAD_BALANCER_REQUIRED_MESSAGE: &str =
    "When specifying the CODE_DEPLOY deployment controller type, the service requires an \
     Application Load Balancer or Network Load Balancer.";

/// One capacity provider, as `CreateCapacityProvider` created it.
///
/// A provider is a named binding onto the capacity that runs tasks: either a caller's
/// own Auto Scaling group (`autoScalingGroupProvider`) or capacity Amazon ECS manages
/// (`managedInstancesProvider`). It is what a `capacityProviderStrategy` entry names,
/// and what `PutClusterCapacityProviders` associates with a cluster.
#[derive(Debug, Clone)]
pub struct CapacityProviderRecord {
    pub name: String,
    pub arn: String,
    /// `CapacityProvider$status`: [`CAPACITY_PROVIDER_ACTIVE`] for a provider that was
    /// created and not deleted, and [`CAPACITY_PROVIDER_PROVISIONING`] while Amazon ECS
    /// is still standing the provider's own capacity up.
    pub status: String,
    /// `CapacityProvider$updateStatus`: where the create, update or delete of this
    /// provider has got to, and `None` when nothing is in flight for it.
    pub update_status: Option<String>,
    /// When the create of a provider Amazon ECS had to stand up falls due, after which
    /// the provider is [`CAPACITY_PROVIDER_ACTIVE`] and its create is
    /// [`CAPACITY_PROVIDER_CREATE_COMPLETE`]. Cleared when it is materialised, so it
    /// happens once.
    pub active_due_at: Option<SystemTime>,
    /// `CapacityProvider$cluster`: the cluster a MANAGED-INSTANCES provider is scoped to
    /// ("Managed instances capacity providers are cluster-scoped"). `None` for an Auto
    /// Scaling group provider, which the account holds rather than one cluster.
    pub cluster: Option<String>,
    pub auto_scaling_group_provider: Option<AutoScalingGroupProvider>,
    pub managed_instances_provider: Option<ManagedInstancesProvider>,
    /// `CapacityProvider$type`: the class of capacity this provider binds onto, as a
    /// `CapacityProviderType` value — [`CAPACITY_PROVIDER_TYPE_MANAGED_INSTANCES`] for a
    /// managed-instances provider, [`CAPACITY_PROVIDER_TYPE_EC2_AUTOSCALING`] for one over
    /// a caller's own Auto Scaling group, and the Fargate class itself for each provider
    /// Amazon ECS manages for every account ([`MANAGED_CAPACITY_PROVIDERS`]).
    pub provider_type: Option<String>,
    pub tags: Vec<Tag>,
}

impl CapacityProviderRecord {
    /// The `CapacityProvider` every capacity-provider operation reports, built from what
    /// is stored. `include_tags` is what `DescribeCapacityProviders$include` gates
    /// (`CapacityProviderField` declares exactly `TAGS`); a create reports the tags it
    /// was given, having no `include` member to narrow them with.
    ///
    /// `tags` is reported WHATEVER the request asked for: the provider's own tags when the
    /// request asked for them, and the empty list otherwise — "If this field is omitted,
    /// tags aren't included in the response" takes the tags out of the list, not the list
    /// out of the response, so a client reading `tags()` on a provider it did not ask the
    /// tags of gets an empty list rather than an absent member.
    ///
    /// `updateStatus`/`updateStatusReason` report an update in flight: a create Amazon ECS
    /// had to stand the provider's capacity up for is one, so such a provider carries
    /// [`CAPACITY_PROVIDER_CREATE_IN_PROGRESS`] until the create lands and
    /// [`CAPACITY_PROVIDER_CREATE_COMPLETE`] from then on, while nothing is in flight for
    /// a provider over a group the caller already had. `updateStatusReason` stays unset:
    /// it states why an update did not do what it was asked, and these all did.
    pub fn view(&self, include_tags: bool) -> CapacityProvider {
        CapacityProvider {
            capacity_provider_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            cluster: self.cluster.clone(),
            status: Some(self.status.clone()),
            auto_scaling_group_provider: self.auto_scaling_group_provider.clone(),
            managed_instances_provider: self.managed_instances_provider.clone(),
            update_status: self.update_status.clone(),
            update_status_reason: None,
            tags: Some(if include_tags {
                self.tags.clone()
            } else {
                Vec::new()
            }),
            r#type: self.provider_type.clone(),
        }
    }

    /// The Auto Scaling group this provider scales, as the ARN or the group name its
    /// `autoScalingGroupProvider` named ("The Amazon Resource Name (ARN) that identifies
    /// the Auto Scaling group, or the Auto Scaling group name"). `None` for a provider
    /// built on managed instances, which has no group of the caller's to collide with.
    pub fn auto_scaling_group(&self) -> Option<&str> {
        self.auto_scaling_group_provider
            .as_ref()
            .map(|provider| provider.auto_scaling_group_arn.as_str())
    }

    /// This provider's binding, as [`capacity_provider_binding_fingerprint`] renders it,
    /// so a repeated create can be compared against what is already stored.
    pub fn binding_fingerprint(&self) -> String {
        capacity_provider_binding_fingerprint(
            self.cluster.as_deref(),
            self.auto_scaling_group_provider.as_ref(),
            self.managed_instances_provider.as_ref(),
        )
    }
}

/// A capacity provider's BINDING, rendered as the string two creates of one name are
/// compared on.
///
/// The binding is what the provider IS: the cluster it is scoped to, plus either the Auto
/// Scaling group it scales (with that group's managed-scaling, termination-protection and
/// draining configuration) or the managed-instances configuration Amazon ECS runs for it.
/// That configuration is a deep tree — a launch template, its network, storage and
/// instance requirements — and these shapes carry a derived `Debug` rather than an
/// equality, so rendering it is what stands in for comparing it member by member: the two
/// sides are the same shape in the same member order, so equal renderings are equal
/// bindings. The Auto Scaling group is rendered through [`auto_scaling_group_key`],
/// because naming one group by ARN and by its bare name names the same group.
///
/// Tags are deliberately NOT part of it: they are metadata a create applies and
/// `TagResource`/`UntagResource` change afterwards, not part of the capacity the provider
/// binds onto.
pub fn capacity_provider_binding_fingerprint(
    cluster: Option<&str>,
    auto_scaling_group_provider: Option<&AutoScalingGroupProvider>,
    managed_instances_provider: Option<&ManagedInstancesProvider>,
) -> String {
    let group = auto_scaling_group_provider.map(|provider| {
        let mut normalised = provider.clone();
        normalised.auto_scaling_group_arn =
            auto_scaling_group_key(&provider.auto_scaling_group_arn).to_string();
        normalised
    });
    format!("cluster={cluster:?} group={group:?} managed={managed_instances_provider:?}")
}

/// The `ClientException` message a create earns when the name it asks for is held by a
/// provider bound onto DIFFERENT capacity. A fixed sentence naming nothing from the
/// request, as the recorded refusal carries it: it tells the caller to change the
/// provider that is there rather than to create it again.
pub const CAPACITY_PROVIDER_EXISTS_MESSAGE: &str =
    "The specified capacity provider already exists. To change the configuration of an \
     existing capacity provider, update the capacity provider.";

/// The `ClientException` message a create earns when the Auto Scaling group it asks for
/// is already backing another capacity provider. A fixed sentence naming nothing from the
/// request, as the recorded refusal carries it: it names the MEMBER at fault and what a
/// retry has to carry, not which provider holds the group.
pub const AUTO_SCALING_GROUP_IN_USE_MESSAGE: &str =
    "The specified Auto Scaling group ARN is already being used by another capacity \
     provider. Specify a unique Auto Scaling group ARN and try again.";

/// A cluster, as `CreateCluster` created it and every cluster-mutating operation
/// (`UpdateCluster`, `UpdateClusterSettings`, `PutClusterCapacityProviders`) has left it.
/// A DELETED cluster is still one of these, with `status` [`CLUSTER_INACTIVE`]:
/// `DeleteCluster` tombstones the record rather than dropping it, because "Clusters with
/// an INACTIVE status may remain discoverable in your account for a period of time."
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub tags: Vec<Tag>,
    pub settings: Vec<ClusterSetting>,
    pub configuration: Option<ClusterConfiguration>,
    pub capacity_providers: Vec<String>,
    pub default_capacity_provider_strategy: Vec<CapacityProviderStrategyItem>,
    /// The default Service Connect namespace, as `CreateCluster` or `UpdateCluster` set
    /// it. `None` once a request clears it with an empty namespace name.
    pub service_connect_defaults: Option<ClusterServiceConnectDefaults>,
    /// `Cluster$attachmentsStatus`: `None` until a capacity-provider association is
    /// applied to this cluster, then [`ATTACHMENTS_UPDATE_IN_PROGRESS`] until the clock
    /// makes [`ATTACHMENTS_UPDATE_COMPLETE`] due.
    pub attachments_status: Option<String>,
    /// When the attachments update this cluster is in the middle of falls due. Stored,
    /// never slept on: [`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]
    /// materialises it.
    pub attachments_update_due_at: Option<SystemTime>,
}

impl ClusterRecord {
    /// Apply the settings of a request to the stored cluster.
    ///
    /// `ClusterSettings` is keyed by `ClusterSetting$name`, so a request carries the
    /// settings it wants to CHANGE: one the request names replaces the stored value of
    /// that name, and one it does not name keeps it. Shared by `UpdateClusterSettings`
    /// ("Modifies the settings to use for a cluster") and `UpdateCluster$settings` ("The
    /// cluster settings for your cluster"), so both write the same way, and keyed per
    /// cluster because the record is.
    pub fn apply_settings(&mut self, settings: &[ClusterSetting]) {
        for setting in settings {
            match self
                .settings
                .iter_mut()
                .find(|stored| stored.name == setting.name)
            {
                Some(stored) => stored.value = setting.value.clone(),
                None => self.settings.push(setting.clone()),
            }
        }
    }

    /// Start the attachments update that associating capacity providers with this
    /// cluster owes, due [`TRANSITION_SPAN`] from `now`.
    ///
    /// `Cluster$attachmentsStatus` reports `UPDATE_IN_PROGRESS` while "the available
    /// capacity providers for the cluster are updating" and `UPDATE_COMPLETE` once "the
    /// capacity providers have successfully updated" — a transition the CLOCK owes, so a
    /// caller that reads straight back still sees the in-progress state.
    pub fn start_attachments_update(&mut self, now: SystemTime) {
        self.attachments_status = Some(ATTACHMENTS_UPDATE_IN_PROGRESS.to_string());
        self.attachments_update_due_at = Some(now + TRANSITION_SPAN);
    }

    /// Whether this cluster's attachments are mid-update, which is what the busy-state
    /// refusal of a cluster mutation is about: "Cluster attachments must be in
    /// UPDATE_COMPLETE or UPDATE_FAILED state before they can be updated."
    pub fn attachments_updating(&self) -> bool {
        self.attachments_status.as_deref() == Some(ATTACHMENTS_UPDATE_IN_PROGRESS)
    }

    /// The resources attached to the cluster. `Cluster$attachments`: "When using a
    /// capacity provider with a cluster, the capacity provider and associated resources
    /// are returned as cluster attachments" — the Auto Scaling group and managed
    /// instance infrastructure a capacity provider provisions. This emulator provisions
    /// no such resource (`CreateCapacityProvider` mints none), so an associated capacity
    /// provider attaches nothing and the list is empty rather than invented.
    pub fn attachments(&self) -> Vec<Attachment> {
        Vec::new()
    }
}

impl ClusterRecord {
    /// The value stored for one setting NAME on this cluster, or `None` when the cluster
    /// never set it — in which case the cluster inherits the account setting of that
    /// name (see
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_effective_settings`]).
    pub fn setting_value(&self, name: &str) -> Option<String> {
        self.settings
            .iter()
            .find(|setting| setting.name.as_deref() == Some(name))
            .and_then(|setting| setting.value.clone())
    }
}

/// A container instance, as `RegisterContainerInstance` left it: the EC2 or external
/// instance that runs the ECS agent and offers the cluster its capacity.
#[derive(Debug, Clone)]
pub struct ContainerInstanceRecord {
    pub arn: String,
    /// The container-instance id: the last ARN segment, and what a caller may name
    /// instead of the ARN.
    pub instance_id: String,
    pub cluster_name: String,
    pub ec2_instance_id: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub agent_connected: bool,
    pub version_info: Option<VersionInfo>,
    /// `ContainerInstance$agentUpdateStatus`: "The status of the most recent agent update.
    /// If an update wasn't ever requested, this value is `NULL`" — so it stays `None`
    /// until an `UpdateContainerAgent` asks for one.
    pub agent_update_status: Option<String>,
    /// When the accepted agent update falls due, and `None` once the clock has
    /// materialised it. The update is owed by the CLOCK from the instant
    /// `UpdateContainerAgent` accepted it, not by whoever reads the instance next.
    pub agent_update_due_at: Option<SystemTime>,
    /// `totalResources`, as the agent reported it at registration —
    /// `ContainerInstance$registeredResources` is "the amount of each resource that was
    /// available on the container instance when the container agent registered it".
    pub registered_resources: Vec<Resource>,
    pub attributes: Vec<Attribute>,
    /// The GPU and Neuron devices the agent reported. Recorded because registration is
    /// what reports them; `ContainerInstance` models no member for them, so nothing
    /// renders them back.
    pub platform_devices: Vec<PlatformDevice>,
    pub tags: Vec<Tag>,
    pub registered_at: DateTime,
    pub version: i64,
}

impl ContainerInstanceRecord {
    /// The `ContainerInstance` every container-instance operation reports, built from
    /// what is stored and from `hosted`, the RUNNING/PENDING split of the tasks placed on
    /// this instance — which is why the one projection every operation goes through is
    /// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::container_instance_view`],
    /// the reader that counts them.
    ///
    /// `remainingResources` is "the remaining CPU and memory that wasn't already
    /// allocated to tasks": what a task allocates is the reservation its own containers
    /// hold (`Container$cpu`, `$memory`, `$memoryReservation`), which every task reports
    /// for itself, and no pass over the task map subtracts those from the instance, so
    /// the remainder is the set the registration reported.
    ///
    /// `healthStatus` stays absent: nothing runs an agent health check here, so there is
    /// no check result to report. `agentUpdateStatus` is reported only once an update was
    /// requested ("If an update wasn't ever requested, this value is NULL").
    fn view(&self, fields: ContainerInstanceFields, hosted: ServiceTally) -> ContainerInstance {
        ContainerInstance {
            container_instance_arn: Some(self.arn.clone()),
            ec2_instance_id: Some(self.ec2_instance_id.clone()),
            capacity_provider_name: None,
            version: self.version,
            version_info: self.version_info.clone(),
            remaining_resources: Some(self.registered_resources.clone()),
            registered_resources: Some(self.registered_resources.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            agent_connected: self.agent_connected,
            // "The number of tasks on the container instance that are in the RUNNING
            // status" and the same for `PENDING`: read off the tasks placed on this
            // instance, so a counter cannot drift from the task records that exist.
            running_tasks_count: hosted.running,
            pending_tasks_count: hosted.pending,
            agent_update_status: self.agent_update_status.clone(),
            attributes: Some(self.attributes.clone()),
            registered_at: Some(self.registered_at),
            // Attachments are created for a trunk elastic network interface, which this
            // emulator never provisions, so a registered instance has none.
            attachments: Some(Vec::new()),
            tags: if fields.tags {
                Some(self.tags.clone())
            } else {
                None
            },
            health_status: None,
        }
    }

    /// Whether this instance counts towards the cluster's registered capacity.
    /// `Cluster$registeredContainerInstancesCount`: "This includes container instances
    /// in both ACTIVE and DRAINING status."
    pub fn is_registered(&self) -> bool {
        self.status == CONTAINER_INSTANCE_ACTIVE || self.status == CONTAINER_INSTANCE_DRAINING
    }

    /// Whether a task may be placed on this instance now. "Only instances connected to an
    /// agent can accept task placement requests", and a `DRAINING` instance is one Amazon
    /// ECS is taking out of service ("prevents new tasks from being scheduled for
    /// placement on the container instance"), so neither is capacity a placement can use.
    pub fn accepts_task_placement(&self) -> bool {
        self.status == CONTAINER_INSTANCE_ACTIVE && self.agent_connected
    }

    /// The agent version this instance reported when it registered, if it reported one at
    /// all. A blank version names none: `MissingVersionException` is exactly the condition
    /// "Amazon ECS can't determine the current version of the ... container agent".
    pub fn reported_agent_version(&self) -> Option<&str> {
        self.version_info
            .as_ref()
            .and_then(|info| info.agent_version.as_deref())
            .map(str::trim)
            .filter(|version| !version.is_empty())
    }

    /// Whether an agent update accepted for this instance has not finished yet
    /// ([`AGENT_UPDATE_IN_FLIGHT`]).
    pub fn agent_update_in_flight(&self) -> bool {
        self.agent_update_status
            .as_deref()
            .is_some_and(|status| AGENT_UPDATE_IN_FLIGHT.contains(&status))
    }
}

/// Which optional container-instance fields a response carries, as
/// `DescribeContainerInstances$include` (the `ContainerInstanceField` enum: `TAGS`,
/// `CONTAINER_INSTANCE_HEALTH`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContainerInstanceFields {
    pub tags: bool,
}

impl ContainerInstanceFields {
    /// Every optional field — what the operation that REGISTERED the instance returns,
    /// since the caller just supplied them and `RegisterContainerInstance` has no
    /// `include` member to narrow them with.
    pub fn all() -> Self {
        Self { tags: true }
    }

    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ContainerInstanceField` enum.
    ///
    /// `CONTAINER_INSTANCE_HEALTH` selects nothing: no health check runs in this
    /// emulator, so `ContainerInstance$healthStatus` has no result to carry.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            if field == "TAGS" {
                fields.tags = true;
            }
        }
        fields
    }
}

/// A task-definition revision, as `RegisterTaskDefinition` stored it.
#[derive(Debug, Clone)]
pub struct TaskDefinitionRecord {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub container_definitions: Vec<ContainerDefinition>,
    pub network_mode: String,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub volumes: Vec<Volume>,
    pub placement_constraints: Vec<TaskDefinitionPlacementConstraint>,
    pub requires_compatibilities: Vec<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub proxy_configuration: Option<ProxyConfiguration>,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub runtime_platform: Option<RuntimePlatform>,
    pub enable_fault_injection: Option<bool>,
    pub tags: Vec<Tag>,
    /// The principal that registered this revision, resolved when the registration ran
    /// — not when it is read back.
    pub registered_by: String,
    pub registered_at: DateTime,
    pub deregistered_at: Option<DateTime>,
    /// "The Unix timestamp for the time when the task definition delete was requested" —
    /// absent until `DeleteTaskDefinitions` asks for one, and kept as the instant THAT
    /// call ran.
    pub delete_requested_at: Option<DateTime>,
}

/// The container-definition collection members a task-definition projection always
/// MATERIALISES: a registration that declared none of them reads back with each member
/// present as an EMPTY LIST, never with the key omitted, so a client may index them
/// unconditionally (`definition["environment"]` is `[]`, not a missing key).
///
/// Recorded ECS behavior, and what AWS's own published `DescribeTaskDefinition` example
/// response shows for a container that declares none of them (`portMappings`,
/// `environment`, `mountPoints`, `volumesFrom` each rendered as `[]`); `systemControls`
/// is the same shape. Members with no empty form are NOT invented: a structure such as
/// `logConfiguration` or a scalar such as `user` stays absent when it was not declared,
/// and a declared value is reported exactly as it was stored.
fn container_definition_view(stored: &ContainerDefinition) -> ContainerDefinition {
    ContainerDefinition {
        port_mappings: Some(stored.port_mappings.clone().unwrap_or_default()),
        environment: Some(stored.environment.clone().unwrap_or_default()),
        mount_points: Some(stored.mount_points.clone().unwrap_or_default()),
        volumes_from: Some(stored.volumes_from.clone().unwrap_or_default()),
        system_controls: Some(stored.system_controls.clone().unwrap_or_default()),
        ..stored.clone()
    }
}

/// Whether `value_from` names a Secrets Manager secret rather than a Systems Manager
/// parameter. The `Secret$valueFrom` doc: "the full ARN of the Secrets Manager secret or
/// the full ARN or name of the Systems Manager Parameter Store parameter", so the
/// service that holds it is the ARN's own service segment.
fn is_secrets_manager_reference(value_from: &str) -> bool {
    value_from
        .split(':')
        .nth(2)
        .is_some_and(|service| service == "secretsmanager")
}

/// The container-instance attributes ONE container of a revision requires, appended to
/// `out`. `has_execution_role` is whether the REVISION carries an `executionRoleArn`,
/// which some container features need beside themselves. See
/// [`TaskDefinitionRecord::requires_attributes`] for the rule.
fn container_required_attributes(
    container: &ContainerDefinition,
    has_execution_role: bool,
    out: &mut Vec<String>,
) {
    for secret in container.secrets.iter().flatten() {
        out.push(
            if is_secrets_manager_reference(&secret.value_from) {
                ATTRIBUTE_SECRETS_ASM_ENVIRONMENT
            } else {
                ATTRIBUTE_SECRETS_SSM_ENVIRONMENT
            }
            .to_string(),
        );
    }
    for file in container.environment_files.iter().flatten() {
        if file.r#type.eq_ignore_ascii_case("s3") {
            out.push(ATTRIBUTE_ENV_FILES_S3.to_string());
        }
    }
    if container.privileged == Some(true) {
        out.push(ATTRIBUTE_PRIVILEGED_CONTAINER.to_string());
    }
    if let Some(firelens) = container.firelens_configuration.as_ref() {
        out.push(format!("{ATTRIBUTE_FIRELENS_PREFIX}{}", firelens.r#type));
    }
    if let Some(logging) = container.log_configuration.as_ref() {
        out.push(format!(
            "{ATTRIBUTE_LOGGING_DRIVER_PREFIX}{}",
            logging.log_driver
        ));
        if has_execution_role && logging.log_driver == LOG_DRIVER_AWSLOGS {
            out.push(ATTRIBUTE_EXECUTION_ROLE_AWSLOGS.to_string());
        }
        for secret in logging.secret_options.iter().flatten() {
            out.push(
                if is_secrets_manager_reference(&secret.value_from) {
                    ATTRIBUTE_SECRETS_ASM_LOG_DRIVER
                } else {
                    ATTRIBUTE_SECRETS_SSM_LOG_DRIVER
                }
                .to_string(),
            );
        }
    }
}

/// The `ContainerDefinition$user` Amazon ECS materialises on a task's LOG ROUTER — the
/// container that carries a `firelensConfiguration`. The FireLens router runs as the root
/// user, which `user` names by its uid, as the recorded registration carries it.
pub const FIRELENS_ROUTER_USER: &str = "0";

/// Whether `network_mode` maps a container port onto the SAME host port — a static
/// mapping.
///
/// `PortMapping$hostPort`: under `awsvpc` and `host` "the hostPort can either be left
/// blank or set to the same value as the containerPort", while `bridge` binds an open
/// host port of its own choosing. Read both where a mapping is rendered and where a
/// registration is checked, so one rule decides both.
pub fn is_static_port_mapping(network_mode: &str) -> bool {
    network_mode == NETWORK_MODE_AWSVPC || network_mode == NETWORK_MODE_HOST
}

/// The `hostPort` and `protocol` Amazon ECS fills in for a port mapping the
/// registration left partly blank, under `network_mode`.
///
/// `PortMapping$protocol`: "The default is tcp." `PortMapping$hostPort`: under `awsvpc`
/// and `host` "the hostPort can either be left blank or set to the same value as the
/// containerPort" (a STATIC mapping), while under `bridge` omitting it "(or set it to 0)"
/// asks for a port out of the ephemeral range (a DYNAMIC mapping), which is picked when
/// the task is placed. A mapping that declares a `containerPortRange` instead carries no
/// single host port — the doc: "leave this field empty" — so nothing is filled in for it.
/// The `none` network mode never reaches here with a mapping: it "cannot specify port
/// mappings", so `RegisterTaskDefinition` refuses one.
fn defaulted_port_mapping(mapping: &PortMapping, network_mode: &str) -> PortMapping {
    let static_mapping = is_static_port_mapping(network_mode);
    let host_port = match (mapping.host_port, mapping.container_port) {
        (Some(declared), _) => Some(declared),
        (None, _) if mapping.container_port_range.is_some() => None,
        (None, Some(container_port)) if static_mapping => Some(container_port),
        (None, Some(_)) => Some(DYNAMIC_HOST_PORT),
        (None, None) => None,
    };
    PortMapping {
        host_port,
        protocol: Some(
            mapping
                .protocol
                .clone()
                .unwrap_or_else(|| DEFAULT_PORT_PROTOCOL.to_string()),
        ),
        ..mapping.clone()
    }
}

/// The container definitions a registration STORES: the caller's own, with the members
/// Amazon ECS fills in at registration time filled in, so every later read of the
/// revision reports the same rendered definition.
///
/// Two members are rendered here: a port mapping's ([`defaulted_port_mapping`]), and the
/// `user` of a LOG ROUTER. A container that carries a `firelensConfiguration` is the
/// task's log router ("The FireLens configuration for the container. This is used to
/// specify and configure a log router for container logs"), which Amazon ECS runs as root
/// — [`FIRELENS_ROUTER_USER`] — so the registered definition carries that user when the
/// caller named none. Nothing else is invented for a member the caller left out.
pub fn defaulted_container_definitions(
    containers: &[ContainerDefinition],
    network_mode: &str,
) -> Vec<ContainerDefinition> {
    containers
        .iter()
        .map(|container| ContainerDefinition {
            port_mappings: container.port_mappings.as_ref().map(|mappings| {
                mappings
                    .iter()
                    .map(|mapping| defaulted_port_mapping(mapping, network_mode))
                    .collect()
            }),
            user: match (&container.user, &container.firelens_configuration) {
                (Some(user), _) => Some(user.clone()),
                (None, Some(_)) => Some(FIRELENS_ROUTER_USER.to_string()),
                (None, None) => None,
            },
            ..container.clone()
        })
        .collect()
}

impl TaskDefinitionRecord {
    /// The launch types this revision was validated against — `TaskDefinition$
    /// compatibilities`: "Amazon ECS validates the task definition parameters with
    /// those supported by the launch type."
    ///
    /// Derived from the STORED revision and reported in [`COMPATIBILITY_ORDER`]. A launch
    /// type is reported when BOTH halves hold:
    ///
    /// * the revision's parameters meet that launch type's documented requirements —
    ///   Fargate needs `awsvpc` ("For Amazon ECS tasks on Fargate, the awsvpc network
    ///   mode is required") plus task-level `cpu` and `memory` ("This field is required
    ///   for Fargate"); ECS Managed Instances place tasks on AWS-managed capacity that
    ///   gives each task its own network interface, so they likewise need `awsvpc`;
    ///   `awsvpc` is in turn not available to the external (on-premises) launch type; EC2
    ///   supports every network mode;
    /// * Amazon ECS validated the revision against it at all
    ///   ([`TaskDefinitionRecord::validated_against`]) — every launch type a placement
    ///   may name outright, plus whatever the registration asked for.
    pub fn compatibilities(&self) -> Vec<String> {
        let awsvpc = self.network_mode == NETWORK_MODE_AWSVPC;
        COMPATIBILITY_ORDER
            .iter()
            .filter(|launch_type| match **launch_type {
                "EC2" => true,
                "EXTERNAL" => !awsvpc,
                "MANAGED_INSTANCES" => awsvpc,
                "FARGATE" => awsvpc && self.cpu.is_some() && self.memory.is_some(),
                // Every launch type is classified above by its own documented
                // requirements, so one added to [`COMPATIBILITY_ORDER`] later has to be
                // classified deliberately rather than inheriting a neighbour's rule.
                _ => false,
            })
            .filter(|launch_type| self.validated_against(launch_type))
            .map(|launch_type| (*launch_type).to_string())
            .collect()
    }

    /// Whether Amazon ECS validated this revision against `launch_type` at all, before
    /// its parameters are weighed against that launch type's requirements.
    ///
    /// A registration is validated against the launch types a placement can ask for
    /// outright ([`DEFAULT_VALIDATED_LAUNCH_TYPES`]), and against the ones it asked to be
    /// validated against itself — `requiresCompatibilities` is "The task launch type that
    /// Amazon ECS validates the task definition against", so a stored revision whose
    /// registration named one was validated against it. A launch type that is in neither
    /// (managed-instances capacity, which a placement reaches only through a capacity
    /// provider) was never validated for, so it is not reported however the revision is
    /// shaped — and `requiresCompatibilities`, what the REQUEST asked for, is reported
    /// separately and unchanged.
    fn validated_against(&self, launch_type: &str) -> bool {
        DEFAULT_VALIDATED_LAUNCH_TYPES.contains(&launch_type)
            || self
                .requires_compatibilities
                .iter()
                .any(|requested| requested == launch_type)
    }

    /// The container-instance attributes a task of this revision requires —
    /// `TaskDefinition$requiresAttributes`: "The container instance attributes required
    /// by your task."
    ///
    /// DERIVED from the stored revision: every feature it declares that only a container
    /// instance publishing a capability can serve contributes that capability's
    /// attribute — the `awsvpc` network mode needs [`ATTRIBUTE_TASK_ENI`] (and with it
    /// the Docker remote-API version that carries task networking), a `secrets` or
    /// `secretOptions` reference needs the attribute of the secret store it names, an
    /// S3 `environmentFiles` entry needs [`ATTRIBUTE_ENV_FILES_S3`], a privileged
    /// container needs [`ATTRIBUTE_PRIVILEGED_CONTAINER`], a named `logDriver` needs
    /// that driver's attribute, a `firelensConfiguration` needs the log-router
    /// capability of the router kind it names ([`ATTRIBUTE_FIRELENS_PREFIX`]) and the
    /// `awslogs` driver on a revision that also carries an `executionRoleArn` needs
    /// [`ATTRIBUTE_EXECUTION_ROLE_AWSLOGS`]. Each name is FIXED text completed only by
    /// the feature it is derived from, never by the account, the family or the revision.
    /// The names are the Amazon ECS container-agent
    /// capabilities, which is what an instance registers them as, so
    /// `ListAttributes`/`DescribeContainerInstances` and this list speak the same
    /// vocabulary.
    ///
    /// Reported deduplicated and sorted by name, so a revision reads back the same list
    /// however its containers were ordered. A revision that declares no such feature
    /// requires nothing and the member stays ABSENT (`None`) rather than reporting an
    /// empty list.
    pub fn requires_attributes(&self) -> Option<Vec<Attribute>> {
        let mut names = Vec::new();
        if self.network_mode == NETWORK_MODE_AWSVPC {
            names.push(ATTRIBUTE_DOCKER_REMOTE_API_1_18.to_string());
            names.push(ATTRIBUTE_TASK_ENI.to_string());
        }
        let has_execution_role = self.execution_role_arn.is_some();
        for container in &self.container_definitions {
            container_required_attributes(container, has_execution_role, &mut names);
        }
        names.sort();
        names.dedup();
        if names.is_empty() {
            return None;
        }
        Some(
            names
                .into_iter()
                .map(|name| Attribute {
                    name,
                    ..Attribute::default()
                })
                .collect(),
        )
    }

    /// The `TaskDefinition` every task-definition operation returns, built from what is
    /// stored — never from the request asking for it. The container definitions go
    /// through [`container_definition_view`], so the collections ECS materialises come
    /// back as empty lists rather than as missing keys.
    pub fn view(&self) -> TaskDefinition {
        TaskDefinition {
            task_definition_arn: Some(self.arn.clone()),
            container_definitions: Some(
                self.container_definitions
                    .iter()
                    .map(container_definition_view)
                    .collect(),
            ),
            family: Some(self.family.clone()),
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            network_mode: Some(self.network_mode.clone()),
            revision: self.revision,
            volumes: Some(self.volumes.clone()),
            status: Some(self.status.clone()),
            requires_attributes: self.requires_attributes(),
            placement_constraints: Some(self.placement_constraints.clone()),
            compatibilities: Some(self.compatibilities()),
            runtime_platform: self.runtime_platform.clone(),
            // "The task launch types the task definition was validated against" is
            // `compatibilities`, which Amazon ECS DERIVES; `requiresCompatibilities` is
            // what the REGISTRATION asked for, so a registration that asked for nothing
            // surfaces nothing rather than an explicit empty list the caller never sent.
            requires_compatibilities: (!self.requires_compatibilities.is_empty())
                .then(|| self.requires_compatibilities.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            inference_accelerators: None,
            pid_mode: self.pid_mode.clone(),
            ipc_mode: self.ipc_mode.clone(),
            proxy_configuration: self.proxy_configuration.clone(),
            registered_at: Some(self.registered_at),
            deregistered_at: self.deregistered_at,
            delete_requested_at: self.delete_requested_at,
            // "The principal that registered the task definition", as it was resolved
            // when the registration ran.
            registered_by: Some(self.registered_by.clone()),
            ephemeral_storage: self.ephemeral_storage.clone(),
            enable_fault_injection: self.enable_fault_injection,
        }
    }
}

/// A DAEMON task-definition revision, as `RegisterDaemonTaskDefinition` stored it.
///
/// Its own resource beside [`TaskDefinitionRecord`]: "A daemon task definition is a
/// template that describes the containers that form a daemon", registered under its own
/// families, revisions and ARNs, and described by its own operation — so a daemon
/// revision and a plain revision of the same family name are two different records.
#[derive(Debug, Clone)]
pub struct DaemonTaskDefinitionRecord {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub container_definitions: Vec<DaemonContainerDefinition>,
    pub volumes: Vec<DaemonVolume>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    /// The namespace modes in effect for this revision, with the default each member
    /// documents already applied ([`DAEMON_NAMESPACE_MODE_NONE`]), so every read reports
    /// the mode the daemon actually runs in.
    pub pid_mode: String,
    pub ipc_mode: String,
    /// "The metadata that you apply to the daemon task definition": stored with the
    /// revision, which is the only place a later read could get it from.
    pub tags: Vec<Tag>,
    /// The principal that registered this revision, resolved when the registration ran
    /// — not when it is read back.
    pub registered_by: String,
    pub registered_at: DateTime,
    /// "The Unix timestamp for the time when the daemon task definition delete was
    /// requested" — absent until a delete is asked for.
    pub delete_requested_at: Option<DateTime>,
    /// When [`DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS`] ->
    /// [`DAEMON_TASK_DEFINITION_DELETED`] falls due; `None` both for a revision no delete
    /// was asked for and for one whose delete has already been materialised. Stored and
    /// compared, never slept on
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::advance_clock`]).
    pub deleted_due_at: Option<SystemTime>,
}

impl DaemonTaskDefinitionRecord {
    /// Record the delete a `DeleteDaemonTaskDefinition` call asked for at `now`: the
    /// revision enters [`DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS`], keeps the instant
    /// the delete was requested, and owes the clock its completion
    /// [`TRANSITION_SPAN`] later — so a caller reading straight after the call still sees
    /// the delete running.
    pub fn request_delete(&mut self, now: SystemTime) {
        self.status = DAEMON_TASK_DEFINITION_DELETE_IN_PROGRESS.to_string();
        self.delete_requested_at = Some(DateTime::from(now));
        self.deleted_due_at = Some(now + TRANSITION_SPAN);
    }

    /// The `DaemonTaskDefinition` every daemon task-definition operation returns, built
    /// from what is stored — never from the request asking for it.
    pub fn view(&self) -> DaemonTaskDefinition {
        DaemonTaskDefinition {
            daemon_task_definition_arn: Some(self.arn.clone()),
            family: Some(self.family.clone()),
            revision: self.revision,
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            container_definitions: Some(self.container_definitions.clone()),
            // A registration that declared no volumes reads back with an EMPTY LIST, the
            // same shape a plain revision's `volumes` takes, so a client may index it
            // unconditionally.
            volumes: Some(self.volumes.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            status: Some(self.status.clone()),
            registered_at: Some(self.registered_at),
            delete_requested_at: self.delete_requested_at,
            registered_by: Some(self.registered_by.clone()),
            pid_mode: Some(self.pid_mode.clone()),
            ipc_mode: Some(self.ipc_mode.clone()),
        }
    }

    /// The `DaemonTaskDefinitionSummary` a listing reports for this revision — "A summary
    /// of a daemon task definition", built from the same stored values as [`Self::view`]
    /// so a listing and a describe of one revision can never disagree.
    pub fn summary(&self) -> DaemonTaskDefinitionSummary {
        DaemonTaskDefinitionSummary {
            arn: Some(self.arn.clone()),
            registered_at: Some(self.registered_at),
            registered_by: Some(self.registered_by.clone()),
            delete_requested_at: self.delete_requested_at,
            status: Some(self.status.clone()),
        }
    }
}

/// A DAEMON, as `CreateDaemon` created it.
///
/// "A daemon deploys cross-cutting software agents such as security monitoring,
/// telemetry, and logging independently across your Amazon ECS infrastructure": it is a
/// resource of its own cluster, built on the daemon task definition it runs and the
/// capacity providers whose container instances it runs on — which is why those two are
/// stored as what the create RESOLVED them to rather than as the references it carried.
#[derive(Debug, Clone)]
pub struct DaemonRecord {
    pub name: String,
    pub arn: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    /// `DaemonStatus`: [`DAEMON_ACTIVE`] for a daemon that was created and not deleted.
    pub status: String,
    /// The daemon task-definition REVISION this daemon runs, as its ARN — the revision
    /// the create resolved, so a later registration in the same family does not move it.
    pub daemon_task_definition_arn: String,
    /// The capacity providers the daemon deploys onto, by NAME (the key
    /// `capacity_providers` holds them under), in the order the request named them.
    pub capacity_provider_names: Vec<String>,
    /// `deploymentConfiguration`: "Optional deployment parameters that control how the
    /// daemon rolls out updates". `None` when the create named none, which is what the
    /// member being optional means.
    pub deployment_configuration: Option<DaemonDeploymentConfiguration>,
    /// `propagateTags`, one of the `DaemonPropagateTags` values. "If you don't specify a
    /// value, the tags aren't propagated" — the same `NONE` [`PROPAGATE_TAGS_NONE`]
    /// spells for the service-level member.
    pub propagate_tags: String,
    pub enable_ecs_managed_tags: bool,
    pub enable_execute_command: bool,
    /// `critical`: "If the critical parameter of a daemon is true, and the daemon task
    /// fails, stops, or becomes unhealthy, Amazon ECS drains the container instance",
    /// defaulted at creation ([`DAEMON_CRITICAL_DEFAULT`]) so every read reports it.
    pub critical: bool,
    pub tags: Vec<Tag>,
    /// The `clientToken` the create carried, so a repeat of the SAME create is answered
    /// with the stored daemon instead of a second one.
    pub client_token: Option<String>,
    /// `CreateDaemonResponse$deploymentArn`: "The Amazon Resource Name (ARN) of the
    /// initial daemon deployment", minted with the daemon.
    pub deployment_arn: String,
    pub created_at: DateTime,
    /// `DaemonSummary$updatedAt`: "when the daemon was last updated" — the creation
    /// instant until something updates the daemon.
    pub updated_at: DateTime,
}

impl DaemonRecord {
    /// The `DaemonSummary` a listing reports for this daemon — "A summary of a daemon",
    /// built from the stored record so a listing cannot drift from what was created.
    pub fn summary(&self) -> DaemonSummary {
        DaemonSummary {
            daemon_arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
        }
    }
}

/// A task, as `RunTask` created it and `StopTask`/the clock left it.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_arn: String,
    /// The task id: the last ARN segment, and what a caller may name instead of the ARN.
    pub task_id: String,
    pub cluster_name: String,
    pub cluster_arn: String,
    /// The container instance that HOSTS this task (`Task$containerInstanceArn`: "The ARN
    /// of the container instances that host the task"), and `None` for a task no
    /// placement put on one. A placement the CALLER directs at named container instances
    /// (`StartTask$containerInstances`: "the container instances where you would like to
    /// place your task") records the instance it named; a placement left to the scheduler
    /// carries no instance, because nothing here chooses which of a cluster's instances
    /// would run it.
    pub container_instance_arn: Option<String>,
    pub task_definition_arn: String,
    pub family: String,
    pub last_status: String,
    pub desired_status: String,
    pub containers: Vec<ContainerRecord>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub overrides: Option<TaskOverride>,
    pub launch_type: Option<String>,
    pub capacity_provider_name: Option<String>,
    pub platform_version: Option<String>,
    pub started_by: Option<String>,
    pub group: String,
    /// The service whose scheduler placed this task, if any. `None` for a standalone
    /// `RunTask`/`StartTask` task, so a service's counts are read off the tasks its own
    /// scheduler placed rather than off whatever shares its `group`.
    pub service_name: Option<String>,
    pub tags: Vec<Tag>,
    pub enable_execute_command: bool,
    pub created_at: DateTime,
    pub started_at: Option<DateTime>,
    pub stopping_at: Option<DateTime>,
    pub stopped_at: Option<DateTime>,
    pub execution_stopped_at: Option<DateTime>,
    /// `Task$pullStartedAt`: "The Unix timestamp for the time when the container image
    /// pull began" — an instant only the AGENT observes, so it is absent until a report
    /// carries it (`SubmitTaskStateChange$pullStartedAt`).
    pub pull_started_at: Option<DateTime>,
    /// `Task$pullStoppedAt`: "The Unix timestamp for the time when the container image
    /// pull completed", reported the same way.
    pub pull_stopped_at: Option<DateTime>,
    pub stopped_reason: Option<String>,
    pub stop_code: Option<String>,
    pub version: i64,
    /// When `PROVISIONING` -> `PENDING` falls due: set when the capacity this task was
    /// parked waiting for registers into its cluster, or — for a task that was allocated
    /// its own elastic network interface — when that interface is in place
    /// ([`placement_schedule`]). `None` both for a task that was placed straight away and
    /// for one still waiting for capacity to appear.
    pub pending_due_at: Option<SystemTime>,
    /// When `PENDING` -> `RUNNING` falls due; `None` once it has been materialised, and
    /// `None` for a task that has not been placed yet.
    pub running_due_at: Option<SystemTime>,
    /// When `DEACTIVATING` -> `STOPPED` falls due; `None` once materialised.
    pub stopped_due_at: Option<SystemTime>,
    /// When the scale-in protection `UpdateTaskProtection` set on this task expires, and
    /// `None` for a task nobody has protected (or whose protection the clock has already
    /// cleared). `ProtectedTask$protectionEnabled` is READ from it: "Task-protection, by
    /// default, expires after 2 hours at which point Amazon ECS clears the
    /// `protectionEnabled` property", which the clock pass does — the expiry was owed at
    /// that instant whether or not anybody called.
    pub protection_expires_at: Option<SystemTime>,
    /// The resources Amazon ECS allocated to this task: the elastic network interface an
    /// `awsvpc` launch gets ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::mint_task_attachments`]),
    /// each in the state the clock has taken it to or the agent last acknowledged through
    /// `SubmitAttachmentStateChanges`. Empty for a task whose network mode allocates none.
    pub attachments: Vec<AttachmentRecord>,
    /// `Task$connectivity`: "The connectivity status of a task", from the model's
    /// `Connectivity` enum (`CONNECTED`, `DISCONNECTED`), and `None` for a task whose
    /// networking is not in place yet ([`Self::report_connected`]).
    pub connectivity: Option<String>,
    /// `Task$connectivityAt`: "The Unix timestamp for the time when the task last went
    /// into `CONNECTED` status", and `None` for a task that has not.
    pub connectivity_at: Option<DateTime>,
    /// The `awsvpc` networking this task was LAUNCHED with
    /// (`NetworkConfiguration$awsvpcConfiguration`: the subnets its interface is placed
    /// in and whether it is given a public address), and `None` for a launch that named
    /// none — a task that shares the networking of the host that runs it.
    ///
    /// Kept on the task because it is the placement THIS task runs under: a later update
    /// of the service that placed it moves the service's configuration, not the networking
    /// an already-launched task was given. It is what the image fetch is decided against
    /// ([`TaskLaunch`]).
    pub awsvpc_configuration: Option<AwsVpcConfiguration>,
    /// Why Amazon ECS cannot launch the containers of this task: a role the launch
    /// resolved that it may not assume ([`settle_task_launches`]), or an image no path
    /// from this placement reaches ([`evaluate_image_pulls`]). It is the `stoppedReason`
    /// Amazon ECS stops the task with at the instant it would otherwise have started
    /// ([`TaskRecord::fail_launch`]), and `None` for a task nothing is known to block.
    pub launch_failure: Option<String>,
    /// Whether the roles THIS launch resolved have been put to the account's IAM records
    /// already ([`evaluate_launch_role_assumptions`]) — `false` for a task that has only
    /// just been placed, whoever placed it.
    ///
    /// Recorded on the task rather than inferred from what the step left behind, because the
    /// step that follows it is decided for whichever tasks the store holds when IT starts: a
    /// task placed after the assume's pass had collected its launches carries no refusal yet,
    /// and that is not the same thing as having been assumed. The image fetch waits for this
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::launches_awaiting_image_pull`])
    /// and so, through it, does the task's start ([`TaskRecord::launch_steps_taken`]): Amazon
    /// ECS pulls an image AS the execution role, so nothing is fetched for a launch whose
    /// roles it has not assumed.
    pub launch_roles_assumed: bool,
    /// The task execution role this task's image pull is made as — the one the placement
    /// resolved ([`resolved_execution_role_arn`]) — and `None` for a task whose revision
    /// and overrides name none.
    ///
    /// Kept on the task because it is the principal THIS task was launched with: a later
    /// registration of another revision does not change what the pull of an already
    /// launched task was decided against.
    pub execution_role_arn: Option<String>,
    /// When the work this task's containers were placed to do is finished, after which the
    /// task stops ([`Self::finish_work`]) — set when the task STARTS, and `None` for a
    /// task that has not started, for one that is already on its way out, and for a task a
    /// service's scheduler placed (see [`CONTAINER_WORK_SPAN`]).
    pub work_done_due_at: Option<SystemTime>,
}

impl TaskRecord {
    /// The `Task` every task-returning operation reports, built from what is stored.
    ///
    /// `include_tags` follows `DescribeTasks$include`: "Specifies whether you want to
    /// see the resource tags for the task. If TAGS is specified, the tags are included
    /// in the response. If this field is omitted, tags aren't included."
    pub fn view(&self, include_tags: bool) -> Task {
        Task {
            // `Task$attachments`: "The Elastic Network Adapter that's associated with the
            // task if the task uses the `awsvpc` network mode" — the attachments this task
            // actually holds, so a task that was allocated none reports an empty list
            // rather than one invented.
            attachments: Some(self.attachments.iter().map(AttachmentRecord::view).collect()),
            cluster_arn: Some(self.cluster_arn.clone()),
            connectivity: self.connectivity.clone(),
            connectivity_at: self.connectivity_at,
            container_instance_arn: self.container_instance_arn.clone(),
            containers: Some(
                self.containers
                    .iter()
                    .map(|container| container.view(&self.attachments))
                    .collect(),
            ),
            cpu: self.cpu.clone(),
            created_at: Some(self.created_at),
            desired_status: Some(self.desired_status.clone()),
            enable_execute_command: self.enable_execute_command,
            execution_stopped_at: self.execution_stopped_at,
            group: Some(self.group.clone()),
            health_status: Some(self.health_status().to_string()),
            last_status: Some(self.last_status.clone()),
            launch_type: self.launch_type.clone(),
            capacity_provider_name: self.capacity_provider_name.clone(),
            platform_version: self.platform_version.clone(),
            memory: self.memory.clone(),
            overrides: self.overrides.clone(),
            pull_started_at: self.pull_started_at,
            pull_stopped_at: self.pull_stopped_at,
            started_at: self.started_at,
            started_by: self.started_by.clone(),
            stop_code: self.stop_code.clone(),
            stopped_at: self.stopped_at,
            stopped_reason: self.stopped_reason.clone(),
            stopping_at: self.stopping_at,
            tags: if include_tags {
                Some(self.tags.clone())
            } else {
                None
            },
            task_arn: Some(self.task_arn.clone()),
            task_definition_arn: Some(self.task_definition_arn.clone()),
            version: self.version,
            ..Task::default()
        }
    }

    /// Whether this task still occupies its cluster: anything that has not reached
    /// `STOPPED` yet.
    pub fn is_active(&self) -> bool {
        self.last_status != STATUS_STOPPED
    }

    /// The `healthStatus` this task reports.
    ///
    /// `Task$healthStatus`: "The health status for the task. It's determined by the health of
    /// the essential containers in the task. If all essential containers in the task are
    /// reporting as `HEALTHY`, the task status also reports as `HEALTHY`. If any essential
    /// containers in the task are reporting as `UNHEALTHY` or `UNKNOWN`, the task status also
    /// reports as `UNHEALTHY` or `UNKNOWN`" — read off the containers' own readings in the
    /// priority order the `HealthCheck` doc publishes ([`aggregate_health_status`]), so the
    /// task and its containers can never disagree.
    pub fn health_status(&self) -> &'static str {
        aggregate_health_status(
            self.containers
                .iter()
                .filter(|container| container.essential)
                .map(ContainerRecord::health_status),
        )
    }

    /// Whether this task is one Amazon ECS has accepted but not PLACED: its capacity
    /// provider still has to acquire the capacity to place it on, so nothing is owed on it
    /// at all ([`awaits_provisioned_capacity`]).
    ///
    /// Read off what the clock owes rather than off a flag: a task that has been placed is
    /// owed either its `PENDING` ([`Self::pending_due_at`]) or its start
    /// ([`Self::running_due_at`]), and one that has already reached a later status is past
    /// the question.
    pub fn is_awaiting_placement(&self) -> bool {
        self.last_status == STATUS_PROVISIONING
            && self.pending_due_at.is_none()
            && self.running_due_at.is_none()
    }

    /// Whether this task is one the `PENDING` task counts count: Amazon ECS has accepted
    /// it and is still bringing it up, so it has not started.
    ///
    /// `Service$pendingCount` and `Cluster$pendingTasksCount` are "the number of tasks in
    /// the cluster that are in the `PENDING` state" — the tasks on their way up. A task
    /// Amazon ECS still has a provisioning step to take for ([`STATUS_PROVISIONING`]: the
    /// capacity it has to be placed on, or the interface it has to be given) is on its way
    /// up too, and counting it anywhere else would report a service that is bringing its
    /// tasks up as holding none at all.
    pub fn is_pending(&self) -> bool {
        self.last_status == STATUS_PENDING || self.last_status == STATUS_PROVISIONING
    }

    /// The `ProtectedTask` a task-protection operation reports for this task: the
    /// protection it holds, read off the expiry the clock keeps.
    pub fn protection_view(&self) -> ProtectedTask {
        ProtectedTask {
            task_arn: Some(self.task_arn.clone()),
            // "If scale-in protection is turned on for a task, the value is true.
            // Otherwise, it is false."
            protection_enabled: self.protection_expires_at.is_some(),
            // "The epoch time when protection for the task will expire" — absent for a
            // task that is not protected, which has no expiry to report.
            expiration_date: self.protection_expires_at.map(DateTime::from),
        }
    }

    /// Whether this task is one a task-protection call may act on: "This operation is
    /// only supported for tasks belonging to an Amazon ECS service."
    pub fn is_protectable(&self) -> bool {
        self.service_name.is_some()
    }

    /// Whether scale-in protection is turned ON for this task right now.
    ///
    /// `ProtectedTask$protectionEnabled`: "If scale-in protection is turned on for a task,
    /// the value is true" — which is the expiry the record holds, the same value
    /// [`Self::protection_view`] reports, so what the scheduler passes over
    /// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::stop_unprotected_service_tasks`])
    /// is exactly what `GetTaskProtection` answers. An expiry the clock has already run past
    /// is cleared by the clock pass, so a lapsed protection protects nothing.
    pub fn is_protected(&self) -> bool {
        self.protection_expires_at.is_some()
    }

    /// Report this task `CONNECTED` as of `at`, if the networking it runs with is in place
    /// and it is not reported connected already.
    ///
    /// `Task$connectivity` is "the connectivity status of a task" and `connectivityAt`
    /// "the time when the task last went into `CONNECTED` status". A task that is
    /// allocated its own elastic network interface is reachable exactly when that
    /// interface is attached, so the instant every interface it holds reports
    /// [`ATTACHMENT_STATUS_ATTACHED`] is the instant it went `CONNECTED`; a task that was
    /// allocated no interface of its own shares the networking of the host that runs it,
    /// so it is connected once it has STARTED there.
    ///
    /// Answers whether anything changed, which is what makes the task's `version` counter
    /// move ("Every time a task experiences a change ... the version counter is
    /// incremented").
    pub fn report_connected(&mut self, at: SystemTime) -> bool {
        if self.connectivity.is_some() {
            return false;
        }
        let connected = if self.holds_interface() {
            self.interfaces_attached()
        } else {
            self.last_status == STATUS_RUNNING
        };
        if !connected {
            return false;
        }
        self.connectivity = Some(CONNECTIVITY_CONNECTED.to_string());
        self.connectivity_at = Some(DateTime::from(at));
        true
    }

    /// Whether this task was allocated an elastic network interface of its own, which is
    /// what the `awsvpc` network mode gets it.
    pub fn holds_interface(&self) -> bool {
        self.attachments
            .iter()
            .any(AttachmentRecord::is_elastic_network_interface)
    }

    /// Whether every elastic network interface this task was allocated is in place —
    /// vacuously so for a task that was allocated none, which shares the networking of the
    /// host that runs it.
    ///
    /// Amazon ECS attaches that interface BEFORE it launches the task ("PROVISIONING:
    /// Amazon ECS has to perform additional steps before the task is launched. For
    /// example, for tasks that use the `awsvpc` network mode, the elastic network
    /// interface needs to be provisioned", the ECS task lifecycle), so this is what the
    /// task's own `PROVISIONING` -> `PENDING` transition waits for.
    pub fn interfaces_attached(&self) -> bool {
        self.attachments
            .iter()
            .filter(|attachment| attachment.is_elastic_network_interface())
            .all(AttachmentRecord::is_attached)
    }

    /// Whether the work this task was placed to do is still going: it is running, and it is
    /// still wanted running.
    ///
    /// What decides whether the end of that work ([`Self::work_done_due_at`]) is still owed:
    /// a task something else has already stopped — a caller's `StopTask`, a scheduler's
    /// stop, an agent's report — is on its way out for THAT reason, so the instant its work
    /// would have finished at is not a transition anything will materialise, and the clock
    /// pump does not wait for it ([`next_task_due_at`]).
    pub fn work_is_running(&self) -> bool {
        self.last_status == STATUS_RUNNING && self.desired_status == STATUS_RUNNING
    }

    /// Whether an ESSENTIAL container of this task is still performing its program, given
    /// the container ARNs whose runs are still going (`running`).
    ///
    /// What the end of a standalone task's work waits on: the stop it earns is
    /// [`STOP_CODE_ESSENTIAL_CONTAINER_EXITED`] — "The task stopped because an essential
    /// container exited" — so a task whose essential container is STILL RUNNING its program
    /// ([`container_runtime`]) has not earned it, however long ago the span that end was
    /// scheduled at fell due. A container whose launch names no program at all runs nothing,
    /// so it holds nothing open.
    pub fn essential_programs_running(&self, running: &HashSet<String>) -> bool {
        self.containers
            .iter()
            .filter(|container| container.essential)
            .any(|container| running.contains(&container.container_arn))
    }

    /// Stop this task because the work its containers were placed to do is finished, as of
    /// `at`.
    ///
    /// The stop a task that RAN earns, as against the one a task that could not be launched
    /// at all earns ([`TaskRecord::fail_launch`]): the task keeps the `startedAt` it
    /// reached, and the stop carries [`STOP_CODE_ESSENTIAL_CONTAINER_EXITED`] with the
    /// reason Amazon ECS words for that code. The task is on its way out from this instant
    /// — `desiredStatus` is `STOPPED`, which is what a `ListTasks` narrowed to stopped
    /// tasks reports it under — and `STOPPED` itself falls due one [`TRANSITION_SPAN`]
    /// later, the same two-step stop every other stop of a task is recorded as.
    ///
    /// No container is given an `exitCode`: `Container$exitCode` is "the exit code returned
    /// from the container", and this emulator starts no container process, so nothing
    /// observed an exit status to report. The agent's own report
    /// (`SubmitContainerStateChange`) stays the only writer of one.
    pub fn finish_work(&mut self, at: SystemTime) {
        self.work_done_due_at = None;
        self.desired_status = STATUS_STOPPED.to_string();
        // The first status of a stop: "DEACTIVATING" is where a task that is on its way out
        // is, with `stoppingAt` recorded — the same two-step stop `StopTask` records.
        self.last_status = STATUS_DEACTIVATING.to_string();
        self.stopped_reason = Some(ESSENTIAL_CONTAINER_EXITED_REASON.to_string());
        self.stop_code = Some(STOP_CODE_ESSENTIAL_CONTAINER_EXITED.to_string());
        self.stopping_at = Some(DateTime::from(at));
        self.stopped_due_at = Some(at + TRANSITION_SPAN);
        for container in &mut self.containers {
            container.advance_to(STATUS_DEACTIVATING);
        }
        self.version += 1;
    }
}

/// Per-service state for com-amazonaws-ecs-amazonec2containerservicev20141113. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// `clusterName` -> the cluster.
    pub clusters: HashMap<String, ClusterRecord>,
    /// `capacityProviderName` -> the capacity provider. Provider names are unique within
    /// the account ("The name of the capacity provider" identifies it, and a
    /// `capacityProviderStrategy` entry names one by that name alone), which is exactly
    /// this key. Distinct from `ClusterRecord::capacity_providers`, which only records
    /// WHICH of these a cluster is associated with.
    pub capacity_providers: HashMap<String, CapacityProviderRecord>,
    /// `(family, revision)` -> the registered revision.
    pub task_definitions: HashMap<(String, i32), TaskDefinitionRecord>,
    /// `family` -> the revision the next `RegisterTaskDefinition` for it will take.
    /// Kept beside `task_definitions` so a revision number is never reused, even after
    /// a revision is deregistered (`TaskDefinition$revision`: "the revision value
    /// always increases by one. This is even if you deregistered previous revisions").
    pub next_revision: HashMap<String, i32>,
    /// `(family, revision)` -> the registered DAEMON task-definition revision. Daemon
    /// task definitions are their own resource (see [`DaemonTaskDefinitionRecord`]), so
    /// they are held beside `task_definitions` rather than in it: the same family name
    /// can carry both, and a reference of one kind never resolves to the other's record.
    pub daemon_task_definitions: HashMap<(String, i32), DaemonTaskDefinitionRecord>,
    /// `family` -> the revision the next `RegisterDaemonTaskDefinition` for it will
    /// take. Kept beside `daemon_task_definitions` for the reason `next_revision` is
    /// kept beside `task_definitions`: a revision number is never reused, even after the
    /// revision that held it is deleted.
    pub next_daemon_revision: HashMap<String, i32>,
    /// `daemonArn` -> the daemon. A daemon ARN carries the cluster that hosts it and the
    /// daemon's own name (`daemon/<cluster>/<name>`), so this one key is what makes a
    /// daemon name unique within its cluster and free in every other.
    pub daemons: HashMap<String, DaemonRecord>,
    /// `taskArn` -> the task.
    pub tasks: HashMap<String, TaskRecord>,
    /// `containerInstanceArn` -> the registered container instance.
    pub container_instances: HashMap<String, ContainerInstanceRecord>,
    /// `(clusterName, serviceName)` -> the service. Service names are unique within a
    /// cluster ("Service names must be unique within a cluster, but you can have
    /// similarly named services in multiple clusters"), which is exactly this key.
    pub services: HashMap<(String, String), ServiceRecord>,
    /// `(clusterName, serviceName, taskSetId)` -> the task set. A task set belongs to one
    /// service of one cluster — the three members every task-set operation addresses one
    /// by — and `CreateTaskSet` mints an id that is unique across them all.
    pub task_sets: HashMap<(String, String, String), TaskSetRecord>,
    /// `serviceDeploymentArn` -> the service deployment. A service's rollouts are its
    /// HISTORY, so a record outlives the rollout it names (`ListServiceDeployments` lists
    /// finished deployments as well as the one under way) and the key is the ARN the
    /// listing reports.
    pub service_deployments: HashMap<String, ServiceDeploymentRecord>,
    /// `serviceRevisionArn` -> the service revision. `ServiceRevision` is "a record of the
    /// workload configuration Amazon ECS is attempting to deploy", captured by the call
    /// that releases it, so an entry is a SNAPSHOT: a later update releases another
    /// revision rather than rewriting this one.
    pub service_revisions: HashMap<String, ServiceRevisionRecord>,
    /// `(clusterName, serviceName)` -> the EXPRESS service. Keyed like `services`
    /// because an Express service's name is unique within its cluster the same way, and
    /// held BESIDE them because the two are different resources: an Express service is
    /// managed by Amazon ECS (it owns a load balancer, target groups and scaling
    /// policies) and is addressed only by the Express operations.
    pub express_gateway_services: HashMap<(String, String), ExpressGatewayServiceRecord>,
    /// `(principalArn, settingName)` -> the account setting value stored for that ONE
    /// principal by `PutAccountSetting`. An entry here shadows the account-wide default
    /// for that principal alone (`ListAccountSettings$effectiveSettings`: "the account
    /// settings for the root user or the default setting for the `principalArn`").
    pub principal_account_settings: HashMap<(String, String), String>,
    /// `settingName` -> the ACCOUNT-WIDE value, as `PutAccountSettingDefault`
    /// ("Modifies an account setting for all users on an account for whom no individual
    /// account setting has been specified") or a `PutAccountSetting` on the root user
    /// stored it. A name with no entry takes its [`ACCOUNT_SETTINGS`] default.
    pub account_setting_defaults: HashMap<String, String>,
    /// Monotonic source of task ids, so every task ARN is distinct.
    next_task_seq: u64,
    /// Monotonic source of container ids, so every container ARN is distinct.
    next_container_seq: u64,
    /// Monotonic source of container-instance ids, so every container-instance ARN and
    /// every minted instance id is distinct.
    next_container_instance_seq: u64,
    /// Monotonic source of service-deployment ids, so every deployment of every service
    /// is distinct.
    next_deployment_seq: u64,
    /// Monotonic source of service-deployment ids, so every service deployment of every
    /// service is distinct.
    next_service_deployment_seq: u64,
    /// Monotonic source of lifecycle-hook ids, so every PAUSE hook of every rollout is
    /// distinct.
    next_lifecycle_hook_seq: u64,
    /// Monotonic source of service-revision ids, so every revision of every service is
    /// distinct.
    next_service_revision_seq: u64,
    /// Monotonic source of service-event ids, so every event of every service is
    /// distinct.
    next_service_event_seq: u64,
    /// Monotonic source of task-set ids, so every task set of every service is distinct.
    next_task_set_seq: u64,
    /// Monotonic source of daemon-deployment ids, so every deployment of every daemon is
    /// distinct.
    next_daemon_deployment_seq: u64,
    /// Monotonic source of attachment ids, so every attachment of every task is distinct.
    next_attachment_seq: u64,
    /// Monotonic source of the names Amazon ECS generates for an Express service whose
    /// request named none, so every generated name is distinct.
    next_express_gateway_seq: u64,
    /// The task state changes this store has observed and not published to EventBridge
    /// yet, oldest first — see [`task_events`].
    pending_task_events: VecDeque<TaskStateChange>,
    /// `taskArn` -> the `(lastStatus, desiredStatus)` pair last published for that task, so
    /// a transition is recognised once however it was written and published exactly once
    /// ([`Self::record_task_state_changes`]).
    published_task_status: HashMap<String, TaskStatusPair>,
    /// Whether one task clock pump is already running against this store
    /// ([`drive_task_clock`]).
    task_clock_pump_running: bool,
    /// Which claim the running pump holds, so a pump releases its own and never a later
    /// pump's (see [`Self::release_task_clock_pump`]).
    task_clock_pump_generation: u64,
    /// The running pump's task handle, kept so the pump it replaces is aborted rather than
    /// merely forgotten (see [`Self::set_task_clock_pump`]).
    task_clock_pump: Option<::tokio::task::JoinHandle<()>>,
    /// `containerArn` -> the RUN behind that container of that task: the program this
    /// emulator is performing for it and the signals a stop sends it (see
    /// [`container_runtime`]). An entry exists only while the container's program is being
    /// run, so the map is the set of containers that are really executing.
    container_runs: HashMap<String, ContainerRun>,
    /// `containerArn` -> the in-process task performing that container's HEALTH-CHECK probe
    /// (see [`container_health`]). Kept for the same reason the runs above keep theirs: a
    /// probe whose container is gone is ABORTED rather than merely forgotten, so no probe
    /// outlives the container it reports on or holds its slot against
    /// [`MAX_HEALTH_PROBES_IN_FLIGHT`].
    health_probes: HashMap<String, ::tokio::task::JoinHandle<()>>,
}
