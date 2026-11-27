// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-autoscaling-autoscaling-2011-01-01 service.
//!
//! The Auto Scaling control plane keeps two named collections — launch
//! configurations and Auto Scaling groups — plus the scaling activities the
//! capacity changes of those groups produce and the instance refreshes started on
//! them. Every handler in `src/ops/` reads and writes THIS struct through
//! [`service_state`]; nothing in this crate holds a second copy of a resource.
//!
//! # The clock this service owes
//!
//! These changes are owed by the clock rather than by a later request, so they are
//! stored as a DUE INSTANT and materialised by [`AutoScalingState::materialize_due`]
//! (called by every handler, including the reporting ones, which may only
//! materialise what the clock already made due):
//!
//! * an instance Amazon EC2 Auto Scaling just launched is reported `Pending` and
//!   becomes `InService` ([`LAUNCH_TRANSITION`], the span of a launch the group has
//!   already made); one it is scaling in is reported `Terminating` and then leaves the
//!   group ([`LIFECYCLE_TRANSITION`], the span every move OUT of service takes);
//! * a capacity change that asked to honour the group's cooldown waits for that
//!   cooldown to elapse before it is applied (`HonorCooldown`);
//! * a running instance refresh replaces the instances of its group ONE at a time, each
//!   replacement made in two moves a span apart — the window it runs under deciding
//!   whether the replacement is launched before the instance it replaces goes out of
//!   service or after, and the refresh waiting for the instance each move started to
//!   settle before it makes the next ([`refresh_plan`]) — waits the
//!   warmup it runs under after each
//!   replacement comes into service ([`StoredInstanceRefresh::instance_warmup`]), holds at
//!   each checkpoint threshold its progress reaches for the delay it runs under
//!   ([`StoredInstanceRefresh::checkpoint_hold`]) and,
//!   once it has replaced them all, waits the bake time it runs under — reported
//!   `Baking` while it does ([`StoredInstanceRefresh::bake_time`]) — before it succeeds
//!   ([`StoredInstanceRefresh`]);
//! * a refresh a caller cancelled is reported `Cancelling` and becomes `Cancelled`
//!   ([`INSTANCE_REFRESH_CANCELLATION`]);
//! * a refresh a caller rolled back replaces its OWN replacements back, one at a time and
//!   in the same two moves per replacement, and is reported `RollbackSuccessful` once it
//!   has replaced them all ([`AutoScalingState::roll_back_instance_refresh`]);
//! * the warm pool of a group holds the instances it maintains, so the difference
//!   between its size and that target is launched or taken out as the clock makes it
//!   due, and an instance it just launched is reported `Warmed:Pending` before it
//!   reaches the pool's own state ([`warm_pools`]);
//! * a group holds the capacity it is configured for OWED until its members provide it,
//!   so capacity a launch could not place is launched again as soon as nothing stands in
//!   the way of placing it ([`StoredGroup::owes_launch`]).
//!
//! # The instances a group holds
//!
//! An instance of a group is an Amazon EC2 instance OF THE ACCOUNT: a capacity change
//! launches it in the account's own Amazon EC2 store and terminates it there when the
//! group scales in ([`ec2_instances`]), so every id a group reports is one Amazon EC2
//! resolves. This struct holds the group's own view of it — its lifecycle state, its
//! health and the zone it was placed in — and nothing that Amazon EC2's record owns.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Activity, AvailabilityZoneDistribution, AvailabilityZoneImpairmentPolicy, BlockDeviceMapping,
    CapacityReservationSpecification, DesiredConfiguration, Instance, InstanceLifecyclePolicy,
    InstanceMaintenancePolicy, InstanceMetadataOptions, LaunchTemplateSpecification,
    MixedInstancesPolicy, Operator, RefreshPreferences, Tag, TrafficSourceIdentifier,
};

/// The Availability Zones of the subnets a request's `VPCZoneIdentifier` names, read
/// from the account's own Amazon EC2 store.
pub mod ec2_subnets;

/// The Amazon EC2 instances a capacity change launches and terminates, in the
/// account's own Amazon EC2 store: an instance id a group reports names an instance of
/// the account, not one of this service's own invention.
pub mod ec2_instances;

/// The rules a request that configures a group is taken against: which launch source it
/// may name, whether the zones and launch templates it names exist, and the cross-member
/// rules of the policies the group carries — shared by the create and the update, which
/// write the same group, and (for the launch template a group points at) by each LAUNCH
/// the group makes, which resolves that same reference again.
pub mod group_configuration;

/// The tags an Auto Scaling group carries: how a requested tag becomes a stored one, and
/// the one projection every read of a stored tag reports.
pub mod group_tags;

pub use group_tags::{
    named_tag_resources, propagates_at_launch, requested_keys_by_group, stored_tag, stored_tags,
    tag_description, tagged_group_name, too_many_tags_in_one_call_message, too_many_tags_message,
    DEFAULT_PROPAGATE_AT_LAUNCH, GROUP_RESOURCE_TYPE, MAX_TAGS_PER_CALL, MAX_TAGS_PER_GROUP,
};

/// The Elastic Load Balancing v2 targets a group's instances occupy in the target groups
/// it is attached to, in that service's own store: an instance of an attached group is a
/// target of that target group, not a reference this service keeps to itself.
pub mod elbv2_targets;

/// The Classic Load Balancer registrations a group's instances hold in the load balancers it
/// is attached to, in Elastic Load Balancing's own store: an instance of an attached group is
/// a registered instance of that load balancer, not a reference this service keeps to itself.
pub mod elb_classic_instances;

/// The IAM instance profile the launch templates of a group name, resolved against the
/// account's own IAM store: a reference Amazon EC2 stores verbatim and this service, as
/// the consumer of the template, is what resolves.
pub mod iam_instance_profiles;

/// The KMS key the volumes of a group's launch are encrypted under, resolved against the
/// account's own KMS store: a reference Amazon EC2 stores verbatim and this service, as the
/// consumer of the template, is what resolves — a key that produces no data key produces no
/// volume, so the launch places nothing.
pub mod volume_encryption_keys;

/// The service-linked role a group calls other Amazon Web Services under: the role the
/// request named, or the one this service binds the group to when it named none.
pub mod service_linked_role;

/// The security groups the launch templates of a group name, read against the VPC of the
/// subnets the group places its instances in: a group and a subnet of two VPCs could never
/// meet on one instance, so the request that names them is refused.
pub mod ec2_security_groups;

/// The instance lifecycle events this service publishes onto the account's default
/// EventBridge bus as a group launches and terminates its instances, and the clock pump
/// that puts them on the bus as they fall due.
pub mod lifecycle_events;

/// The lifecycle hooks of a group: the named collection `PutLifecycleHook` upserts into
/// and `DescribeLifecycleHooks` projects from, and the rules every writer of it shares.
pub mod lifecycle_hooks;

pub use lifecycle_hooks::{
    absent_lifecycle_hook_message, heartbeat_timeout_violation_message, put_lifecycle_hook,
    too_many_hooks_message, transition_missing_message, unknown_lifecycle_action_result_message,
    unknown_result_message, unknown_transition_message, unsupported_notification_target_message,
    validate_lifecycle_hook_members, LifecycleHookPut, LifecycleHookRefusal, LifecycleHooks,
    StoredLifecycleHook, DEFAULT_HEARTBEAT_TIMEOUT, DEFAULT_LIFECYCLE_ACTION_RESULT,
    LIFECYCLE_ACTION_CONTINUE, LIFECYCLE_ACTION_RESULTS, MAX_LIFECYCLE_HOOKS_PER_GROUP,
    NOTIFICATION_TARGET_VENDORS,
};

/// The lifecycle ACTIONS those hooks hold a group's instances at: the wait state an instance
/// is held in, the token a client completes the hold with, and the events that announce it.
pub mod lifecycle_actions;

pub use lifecycle_actions::{
    abandoned_launch_cause, abandoned_lifecycle_action_message, no_active_action_by_instance_message,
    no_active_action_by_token_message, no_active_action_under_hook_message, not_addressed_message,
    HeldStep, HeldTransition,
    LifecycleActionEnding, LifecycleActionKey, LifecycleActionRefusal, LifecycleActionResolution,
    PendingLifecycleActionEvent, StoredLifecycleAction, LAUNCH_LIFECYCLE_ACTION,
    LIFECYCLE_PENDING_PROCEED, LIFECYCLE_PENDING_WAIT, LIFECYCLE_TERMINATING_PROCEED,
    LIFECYCLE_TERMINATING_RETAINED, LIFECYCLE_TERMINATING_WAIT,
    MAX_PENDING_LIFECYCLE_ACTION_EVENTS, MAX_RETAINED_INSTANCES_PER_GROUP,
    TERMINATE_LIFECYCLE_ACTION, TRANSITION_LAUNCHING, TRANSITION_TERMINATING,
};

/// The warm pool of a group: the pre-initialized instances `PutWarmPool` has the group
/// maintain beside its own, which `DescribeWarmPool` projects from and the group's delete
/// takes with the group.
pub mod warm_pools;

/// Which instances a scale-in of a group takes out of service: the group's
/// `TerminationPolicies`, applied in the order it lists them.
pub mod termination_policies;

pub use warm_pools::{
    absent_warm_pool_message, too_large_warm_pool_message, warm_pool_change_cause,
    warmed_lifecycle_state, StoredWarmPool, WarmPoolPut, DEFAULT_WARM_POOL_MIN_SIZE,
    DEFAULT_WARM_POOL_STATE, MAX_GROUP_PREPARED_CAPACITY_UNSET, MAX_WARM_POOL_RECORDS,
    WARMED_PENDING, WARMED_STATE_PREFIX, WARMED_TERMINATING, WARM_POOL_IN_USE_MESSAGE,
};

/// The scaling processes a group has suspended: the collection `SuspendProcesses` writes
/// into and `DescribeAutoScalingGroups` projects from, and what a suspended process stops
/// the group from doing.
pub mod suspended_processes;

pub use suspended_processes::{
    is_suspendable_process, named_processes, resumed_convergence_cause, suspendable_processes,
    suspension_reason, unknown_process_message, ProcessRefusal, SuspendedProcesses,
    PROCESS_HEALTH_CHECK, PROCESS_LAUNCH, PROCESS_REMOVE_FROM_LOAD_BALANCER_LOW_PRIORITY,
    PROCESS_REPLACE_UNHEALTHY, PROCESS_TERMINATE,
};

/// The group metrics a group aggregates to CloudWatch: the collection
/// `EnableMetricsCollection` writes into, `DisableMetricsCollection` takes out of and
/// `DescribeAutoScalingGroups` projects from, and what a request may name.
pub mod metrics_collection;

pub use metrics_collection::{EnabledMetrics, MetricsRefusal};

/// The CloudWatch series those collected metrics are published into — the `AWS/AutoScaling`
/// namespace a client watches its groups through — and the clock they are published on.
pub mod group_metrics;

pub use group_metrics::{
    metric_value, series_key, GroupMetricPublisher, GROUP_NAME_DIMENSION, METRIC_NAMESPACE,
    PUBLISH_INTERVAL,
};

/// The scaling policies of a group — what `PutScalingPolicy` creates and
/// `DescribePolicies` reports — and the CloudWatch alarms a target-tracking policy is
/// driven by, which live in the account's own CloudWatch store.
pub mod scaling_policies;

pub use scaling_policies::{PolicyKey, StoredScalingPolicy};

/// The forecast a predictive scaling policy publishes — what
/// `GetPredictiveScalingForecast` reports — computed from the CloudWatch observations of the
/// metrics the policy names.
pub mod predictive_forecast;

pub use predictive_forecast::not_a_predictive_scaling_policy_message;

/// The recurring schedule a scheduled action states — the crontab expression its
/// `Recurrence` is written in and the IANA zone its `TimeZone` names — and the next instant
/// such a schedule fires at.
pub mod recurrence;

/// The scheduled scaling actions of a group: the named collection
/// `PutScheduledUpdateGroupAction` and `BatchPutScheduledUpdateGroupAction` upsert into and
/// `DescribeScheduledActions` projects from, and the rules every writer of it shares.
pub mod scheduled_actions;

pub use scheduled_actions::{
    invalid_recurrence_message, scheduled_action_already_exists_message, scheduled_action_arn,
    too_many_scheduled_action_names_in_one_call_message,
    too_many_scheduled_actions_in_one_call_message, too_many_scheduled_actions_message,
    ScheduledActionPut, ScheduledActionRefusal, ScheduledActions, StoredScheduledAction,
    ABSENT_SCHEDULED_ACTION_MESSAGE, ALREADY_EXISTS_ERROR_CODE, END_TIME_MESSAGE,
    INVALID_TIME_ZONE_MESSAGE,
    LIMIT_EXCEEDED_ERROR_CODE, MAX_SCHEDULED_ACTIONS_PER_CALL, MAX_SCHEDULED_ACTIONS_PER_GROUP,
    SIZE_REQUIRED_MESSAGE, START_TIME_IN_PAST_MESSAGE, VALIDATION_ERROR_CODE,
};

/// Performing one of those policies — `ExecutePolicy` — which policy a request names, the
/// capacity the policy's own adjustment moves the group to, and the cooldowns it honours
/// and leaves behind.
pub mod policy_execution;

pub use policy_execution::{
    MetricBreach, PolicyExecution, PolicyExecutionRefusal, PolicyExecutionRequest,
};

/// The Amazon SNS notification configurations of a group: the named collection
/// `PutNotificationConfiguration` upserts into and `DescribeNotificationConfigurations`
/// projects from, and the rules every writer of it shares.
pub mod notification_configurations;

pub use notification_configurations::{
    absent_notification_configuration_message, invalid_topic_arn_message, names_an_sns_topic,
    too_many_topics_message, unknown_notification_type_message, NotificationConfigurationRefusal,
    NotificationConfigurations, MAX_NOTIFICATION_TOPICS_PER_GROUP,
};

/// The Amazon SNS notifications this service publishes to the topics those configurations
/// name: the one that confirms a configuration, and the ones a group's instance launches
/// and terminations produce.
pub mod sns_notifications;

/// The TEST message a lifecycle hook's notification target is proved by as the hook is put,
/// and the refusal of a hook whose role may not publish to that target.
pub mod lifecycle_hook_test_notification;

pub use lifecycle_events::{
    drive_lifecycle_clock, FailedLifecycleStep, InstanceLifecycle, InstanceLifecycleChange,
    InstanceMove, LifecycleEndpoint, PendingLifecycleEvent, StepOutcome,
};

/// The instances a caller asks a group to launch outright — `LaunchInstances` — beside
/// the capacity changes that launch them to converge on a desired capacity: where such a
/// launch places them, what it answers, and the one launch a repeat of its client token
/// is.
pub mod requested_launches;

pub use requested_launches::{
    launch_suspended_message, RequestedLaunch, RequestedLaunchOutcome, RequestedPlacement,
    StoredLaunchRequest, INSUFFICIENT_CAPACITY_CODE, LAUNCH_BLOCKED_CODE,
    MAX_INSTANCES_PER_LAUNCH, MAX_RETAINED_LAUNCH_REQUESTS, RETRY_STRATEGIES,
    RETRY_STRATEGY_NONE, RETRY_WITH_GROUP_CONFIGURATION,
};

/// The two spellings of the Availability Zones a group places its instances in — their
/// NAMES and their IDS — which a group reports both of however the request named them.
pub mod availability_zone_ids;

pub use availability_zone_ids::{zone_id_of, zone_ids_of, zone_names_of};

/// The members of a group the SERVICE decides when the request leaves them out: the
/// option sets every group carries, each at the default its own documentation states.
pub mod group_defaults;

/// Parking a group's instances in `Standby` and returning them to service: the one
/// transition `EnterStandby` and `ExitStandby` share.
pub mod standby;

pub use standby::{StandbyMove, StandbyRefusal};

/// The refusal a move of a group's instances states when the group's own sizes do not admit
/// it: the one sentence `AttachInstances` and `EnterStandby` share.
pub mod group_size_bounds;

pub use group_size_bounds::{group_sizes_refuse_move_message, GroupSizeMove, GroupSizes};

/// Bringing the account's own Amazon EC2 instances into a group: the one transition
/// `AttachInstances` performs, and the rules an instance it takes has to meet.
pub mod attached_instances;

pub use attached_instances::{attach_description, AttachRefusal};

/// Taking an Auto Scaling group's own instances OUT of it without terminating them: the one
/// transition `DetachInstances` performs, and the rules an instance it releases has to meet.
pub mod detached_instances;

pub use detached_instances::{detach_description, DetachRefusal};

/// What the Elastic Load Balancing verdict on a group's instances MAKES OWED: the group's
/// own health check, armed on every member that verdict leaves reported unhealthy.
pub mod elb_health;

pub use elb_health::ELB_HEALTH_CHECK_FAILURE_REASON;

/// The preferences an instance refresh actually runs under: the ones the request stated,
/// each member the request left out at the default this service applies.
pub mod refresh_preferences;

/// The CloudWatch monitors a refresh's `AlarmSpecification` names, read from the account's
/// own CloudWatch store: a refresh cannot START while one of them is sounding.
pub mod refresh_alarms;

/// The window an instance refresh replaces a group's instances INSIDE — whether it surges
/// a replacement above the group's desired capacity or replaces in place — and the moves
/// one replacement is made of.
pub mod refresh_plan;

pub use refresh_plan::{
    RefreshReplacement, RefreshStop, ReplacementPhase, ReplacementWindow, STEPS_PER_REPLACEMENT,
};

/// WHICH members of a group an instance refresh replaces: the behaviour it runs under for a
/// member protected from scale in or parked in `Standby` — replace it, leave it alone, or
/// wait for the condition to be cleared.
pub mod refresh_members;

pub use refresh_members::{
    members_to_replace, MemberBehavior, MemberDisposition, WaitedCondition, MEMBER_CONDITION_WAIT,
};

/// The scaling policies a CloudWatch ALARM drives: the notifications this service receives
/// from the alarms whose actions name its policies, and what receiving one performs.
pub mod alarm_driven_policies;

/// The instances a group holds that the account's Amazon EC2 store no longer runs: the
/// health check that takes them out of the group and the convergence that replaces them.
pub mod ec2_health;

pub use ec2_health::{EC2_INSTANCE_GONE_REASON, UNHEALTHY_REPLACEMENT_REASON};

/// The health of a group's instances as a CALLER records it — `SetInstanceHealth` — and
/// when the group's health check acts on an instance recorded unhealthy.
pub mod instance_health;

pub use instance_health::{
    absent_instance_message, health_status_violation_message, HealthCheckDebt, HealthRefusal,
    HEALTH_STATUS_VALUES, UNHEALTHY_INSTANCE_REPLACEMENT, USER_HEALTH_CHECK_REASON,
};

/// The instance type a group's attribute-based launch source launches, and the unit the
/// group's `DesiredCapacityType` counts its capacity in.
pub mod instance_requirements;

/// What still HOLDS a group and what the clock is still settling on it: the two facts a
/// delete that was not asked to force itself tells apart.
pub mod group_teardown;

/// What ONE scaling activity records: the step it reports, the milestone that step is at,
/// and the placement of the instance it moved.
pub mod activity_records;

/// How a paginated read of this service pages: the `NextToken` it issues and resumes from,
/// the page one request's `MaxRecords` asks for, and the refusals an unusable token or page
/// size is answered with.
mod pagination;
pub use pagination::*;

/// The fixed reference-data catalogs this service publishes through its zero-input
/// describes, and the one reader of each that decides membership of it.
mod reference_catalogs;
pub use reference_catalogs::*;

pub use activity_records::{
    user_requested_instance_move_cause, ActivityMilestone, ActivityRecord, TAKEN_OUT_OF_SERVICE,
};

/// How long a lifecycle transition this service owes takes to come due.
///
/// Amazon EC2 Auto Scaling reports a freshly launched instance as `Pending` and an
/// instance being scaled in as `Terminating` before either settles, so a caller
/// that reads the group straight after a capacity change must still see the
/// transient state. The real wait depends on the instance; this emulator picks a
/// short, fixed span (chosen, not documented) and never sleeps for it.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// How long an instance a group LAUNCHED spends `Pending` before it is in service.
///
/// The launch itself is finished when the call that asked for it answers: the instances are
/// minted in Amazon EC2 and the group holds them already, so the span adds nothing but the
/// transient state Amazon EC2 Auto Scaling reports before an instance is in service. A caller
/// reading the group in the same instant as that call still sees it — which is the whole of
/// what a transient span is for — and nothing else waits on it.
///
/// That is also the FLOOR on the value: the span has to outlast the rest of the call that
/// owes it, because the launch is made part way through a capacity change and the group is
/// read by the NEXT call. A span of a few milliseconds is spent before the answer is even
/// written — the launch, the activities it records, the load balancers it settles — so the
/// state it exists to publish is already gone by the time anybody can ask for it, which is
/// the same as having no transient state at all. This is a span a read of the group lands
/// inside with room to spare, on a host doing other work as well.
///
/// It is shorter than the span the moves OUT of service take ([`LIFECYCLE_TRANSITION`]),
/// because a launch is what a CLIENT waits on: a client written against Amazon EC2 Auto
/// Scaling looks again at the interval the real service's provisioning work calls for — tens
/// of seconds — and probes before it sleeps, so a launch this service has already made and
/// then withholds from the read that follows the call costs that client one whole interval,
/// and a sequence that chains several launches (a scale-out, a scale-out the group's load
/// balancing blocks, and the launch that repairing the attachment owes) is not observable
/// inside the client's budget at all: several of them together still have to fit inside one
/// such interval, which is what caps this span from above. Chosen, not documented, and never
/// slept for.
pub const LAUNCH_TRANSITION: Duration = Duration::from_millis(150);

/// How long a group waits before ATTEMPTING an owed launch again, once the attempt it made
/// was one Amazon EC2 took back ([`StoredGroup::launch_retry_due`]).
///
/// A group maintains the capacity it is configured for, so a launch that placed nothing is
/// attempted again — but an attempt of this kind is an instance Amazon EC2 creates and
/// terminates, so it is owed at an INSTANT rather than made on every pass over the store.
/// The span is CHOSEN, not an AWS-published figure: it is far above the transition spans this
/// service owes, so a client reading the group sees one attempt rather than a stream of them,
/// and far below the patience of a client that repairs the key and waits for the group to
/// converge on its own. What the attempts of a group left pointing at a key that never becomes
/// usable can accumulate to is bounded where those instances live: the account's Amazon EC2
/// store caps how many instances it holds and charges each against its own byte budget, so a
/// launch that cannot be made can no more fill this process's memory than one that can.
pub const WITHDRAWN_LAUNCH_RETRY: Duration = Duration::from_secs(10);

/// `DefaultCooldown` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$DefaultCooldown`:
/// "Default: 300 seconds".
pub const DEFAULT_COOLDOWN_SECONDS: i32 = 300;

/// `HealthCheckGracePeriod` when the request omits it.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$HealthCheckGracePeriod`: "Default: 0 seconds".
pub const DEFAULT_HEALTH_CHECK_GRACE_PERIOD: i32 = 0;

/// `HealthCheckType` when the request omits it.
///
/// From the model's own documentation for `CreateAutoScalingGroup$HealthCheckType`:
/// "EC2 is the default health check and cannot be disabled."
pub const DEFAULT_HEALTH_CHECK_TYPE: &str = "EC2";

/// `TerminationPolicies` when the request omits it: the `Default` policy, which
/// the model documents as a valid `TerminationPolicies` value and which the
/// service applies to a group that configures none.
pub const DEFAULT_TERMINATION_POLICY: &str = termination_policies::DEFAULT;

// The three `LifecycleState` values this slice moves an instance through, spelled
// exactly as the model's `com.amazonaws.autoscaling#LifecycleState` enum does
// (smithy_query(query="shape",shape="LifecycleState")).
/// An instance that has been launched and is not yet in service.
pub const LIFECYCLE_PENDING: &str = "Pending";
/// An instance that is in service.
pub const LIFECYCLE_IN_SERVICE: &str = "InService";
/// An instance that is being scaled in.
pub const LIFECYCLE_TERMINATING: &str = "Terminating";
/// An instance on its way into `Standby`, the transient state `EnterStandby` starts.
pub const LIFECYCLE_ENTERING_STANDBY: &str = "EnteringStandby";
/// An instance parked out of service in `Standby`.
pub const LIFECYCLE_STANDBY: &str = "Standby";
/// An instance on its way out of the group WITHOUT being terminated, the transient state
/// `DetachInstances` starts.
pub const LIFECYCLE_DETACHING: &str = "Detaching";
/// An instance that has left the group without being terminated.
pub const LIFECYCLE_DETACHED: &str = "Detached";

/// The `HealthStatus` of an instance the service has no failed health check for.
pub const HEALTH_STATUS_HEALTHY: &str = "Healthy";

/// The `HealthStatus` of an instance a health check of its group has found out of service.
///
/// The spelling `SetInstanceHealth$HealthStatus` publishes for that state: "the health status
/// of the instance. Set to `Healthy` to have the instance remain in service. Set to
/// `Unhealthy` to have the instance be out of service."
pub const HEALTH_STATUS_UNHEALTHY: &str = "Unhealthy";

/// How long ONE instance replacement of an instance refresh takes to come due.
///
/// A replacement is a COMPOSITE of the moves this service owes an instance: the old
/// instance goes out of service, a new one is launched in its place, and the refresh moves
/// on only once that replacement is itself in service. So it is owed the span this emulator
/// gives a transition the clock — rather than a caller — brings about, which is longer than
/// the single lifecycle span one of those moves takes ([`LIFECYCLE_TRANSITION`]): chosen,
/// not documented, and never slept for. A refresh of a group holding several instances
/// therefore takes one span per instance plus one for its own completion, which is what
/// lets a caller polling it see it `InProgress` while it runs.
///
/// This is the span of the replacement ITSELF. What the refresh then waits before moving
/// on is the warmup it runs under, which the model documents
/// ([`StoredInstanceRefresh::instance_warmup`]).
pub const INSTANCE_REFRESH_REPLACEMENT: Duration = Duration::from_secs(3);

/// How long the cancellation of an instance refresh takes to come due.
///
/// A cancelled refresh is reported `Cancelling` before it is reported `Cancelled`, so
/// a caller that reads it straight after cancelling it still sees the transient
/// status. The span is this service's own transient span ([`LIFECYCLE_TRANSITION`]):
/// chosen, not documented, and never slept for.
pub const INSTANCE_REFRESH_CANCELLATION: Duration = LIFECYCLE_TRANSITION;

// The `InstanceRefreshStatus` values this slice moves an instance refresh through,
// spelled exactly as the model's `com.amazonaws.autoscaling#InstanceRefreshStatus`
// enum does (smithy_query(query="shape",shape="InstanceRefreshStatus")).
/// A refresh whose request was created but which has not started replacing yet.
pub const REFRESH_PENDING: &str = "Pending";
/// A refresh that is replacing the group's instances.
pub const REFRESH_IN_PROGRESS: &str = "InProgress";
/// A refresh that replaced every instance it set out to replace.
pub const REFRESH_SUCCESSFUL: &str = "Successful";
/// A refresh that has finished updating instances and is waiting out the bake time it
/// runs under before it is complete.
pub const REFRESH_BAKING: &str = "Baking";
/// A refresh that could not finish.
pub const REFRESH_FAILED: &str = "Failed";
/// A refresh whose cancellation has been asked for and has not completed.
pub const REFRESH_CANCELLING: &str = "Cancelling";
/// A refresh whose cancellation has completed.
pub const REFRESH_CANCELLED: &str = "Cancelled";
/// A refresh that is being rolled back: the replacements it made are themselves
/// being replaced, with the configuration the group was using before it started.
pub const REFRESH_ROLLBACK_IN_PROGRESS: &str = "RollbackInProgress";
/// A refresh whose rollback replaced every instance the refresh had replaced.
pub const REFRESH_ROLLBACK_SUCCESSFUL: &str = "RollbackSuccessful";
/// A refresh whose rollback could not finish.
pub const REFRESH_ROLLBACK_FAILED: &str = "RollbackFailed";

/// What `RollbackDetails$RollbackReason` reports for a rollback a CALLER asked for.
///
/// The member reports "the reason for this instance refresh rollback (for example,
/// whether a manual or automatic rollback was initiated)"
/// (`smithy_query(query="doc",shape="RollbackDetails",member="RollbackReason")`), and
/// every rollback this service starts is one a caller asked for: nothing here starts
/// one on its own.
pub const MANUAL_ROLLBACK_REASON: &str = "A manual rollback was initiated.";

/// The prefix an Amazon EC2 launch template's `ImageId` carries when it names a
/// Systems Manager parameter rather than an AMI id is Amazon EC2's own reading of that
/// member ([`ec2_instances::resolves_image_through_ssm_parameter`]), so nothing here
/// spells the form out a second time.
///
/// The `Version` of a launch template specification that means "whichever version is
/// the default when the instance is launched", from the model's own documentation for
/// `LaunchTemplateSpecification$Version`: "The version number, `$Latest`, or
/// `$Default`. … The default value is `$Default`" — so a specification carrying no
/// version at all uses this one.
pub const LAUNCH_TEMPLATE_VERSION_DEFAULT: &str = "$Default";

/// The `Version` that means "whichever version is the latest when the instance is
/// launched" (same documentation).
pub const LAUNCH_TEMPLATE_VERSION_LATEST: &str = "$Latest";

/// `Strategy` when a `StartInstanceRefresh` request omits it.
///
/// From the model's own documentation for `StartInstanceRefreshType$Strategy`: "The
/// default value is Rolling."
pub const DEFAULT_REFRESH_STRATEGY: &str = "Rolling";

/// The most instance refreshes this service retains; appending past it drops the
/// oldest.
///
/// A bound on this process, for the same reason as [`MAX_RETAINED_ACTIVITIES`]: a
/// group may be refreshed again and again and nothing removes a finished refresh, so
/// an unbounded log would grow for the life of the process. Amazon EC2 Auto Scaling
/// itself reports the refreshes "from the previous six weeks"
/// (`smithy_query(query="operation",operation="DescribeInstanceRefreshes")`), so a
/// retention bound is the shape of the real behaviour too.
pub const MAX_RETAINED_INSTANCE_REFRESHES: usize = 1_000;

// The `ScalingActivityStatusCode` values an activity this slice records moves
// through, from the model's `com.amazonaws.autoscaling#ScalingActivityStatusCode`.
/// A scaling activity that has started and not yet finished.
pub const ACTIVITY_IN_PROGRESS: &str = "InProgress";
/// A scaling activity whose instance exists and is on its way into service, but is not in
/// service yet — the step a return from `Standby` is at while it runs.
pub const ACTIVITY_PRE_IN_SERVICE: &str = "PreInService";
/// A scaling activity paused at a lifecycle hook: the step it is making is waiting for the
/// hold that hook has taken on the instance to resolve.
pub const ACTIVITY_MID_LIFECYCLE_ACTION: &str = "MidLifecycleAction";
/// A scaling activity that finished.
pub const ACTIVITY_SUCCESSFUL: &str = "Successful";
/// A scaling activity that did not place the instance it was started for.
pub const ACTIVITY_FAILED: &str = "Failed";
/// A scaling activity whose launch Amazon EC2 did not carry out — the launch source it would
/// have run cannot be resolved there, or the instance it launched was taken back before it
/// reached service — so the capacity change it was started for was cancelled rather than
/// attempted and failed.
pub const ACTIVITY_CANCELLED: &str = "Cancelled";

/// `Activity$AutoScalingGroupState` for a group that still exists.
pub const GROUP_STATE_IN_SERVICE: &str = "InService";
/// `Activity$AutoScalingGroupState` for a group that has been deleted.
pub const GROUP_STATE_DELETED: &str = "Deleted";

/// The sentence every failed launch of a group's instance ends with: Amazon EC2 Auto
/// Scaling states WHY the launch could not be made and then that the launch was not made,
/// so a client reads the cause and the outcome off one `StatusMessage`.
pub const LAUNCH_FAILED_SENTENCE: &str = "Launching EC2 instance failed.";

/// What the failed activity of a unit of capacity the ACCOUNT could not place reports as
/// its `StatusMessage` — the cause of the failure a client reads off the activity
/// ([`StoredActivity::to_activity`]) when nothing about the group itself stopped the
/// launch.
///
/// One sentence for every launch path, so a client reads the same cause whether the
/// capacity change, a replacement or the group's own create asked for the instance, and
/// it ends the way every launch failure does ([`LAUNCH_FAILED_SENTENCE`]): with the launch
/// that was not made.
pub const UNPLACED_INSTANCE_MESSAGE: &str =
    "The account could not place the instance. Launching EC2 instance failed.";

/// What the activity of a launch that FAILED describes: the launch Amazon EC2 Auto
/// Scaling set out to make, and the reason it did not — the activity's own
/// `StatusMessage`, named as the status reason.
///
/// The activity of a launch that SUCCEEDED names the instance it launched
/// (`lifecycle_events::InstanceLifecycle::description`); a launch that failed has no
/// instance to name, so it names the reason instead.
pub fn failed_launch_description(status_message: &str) -> String {
    format!("Launching a new EC2 instance. Status Reason: {status_message}")
}

/// Why a launch placed nothing, and the terminal `ScalingActivityStatusCode` the activity of
/// each unit it did not place ends in.
///
/// "If an attempt to launch instances failed, the StatusCode value is Failed or Cancelled and
/// the StatusMessage element in the response indicates the cause of the failure"
/// (`DescribeScalingActivities`' own documentation): WHICH of the two a launch records is a
/// property of the reason it did not place the instance — a launch Amazon EC2 Auto Scaling
/// itself could not make the attempt for FAILED, while one Amazon EC2 did not carry out (the
/// launch source it would have run resolves to nothing there, or the instance it launched
/// would never reach service and is taken back) was CANCELLED — so the reason and the code
/// travel together from the one place that judges the launch
/// ([`ec2_instances::launch_failure_reason`]) to the recorder.
#[derive(Debug, Clone)]
pub struct BlockedLaunch {
    /// The cause the activity reports under `Activity$StatusMessage`.
    pub status_message: String,
    /// The terminal status code the activity ends in.
    pub status_code: &'static str,
    /// Set for a launch Amazon EC2 ACCEPTS and then takes back: what the service that owns
    /// the volume's key said about it, which is the record Amazon EC2 keeps on the attempt it
    /// withdraws ([`ec2_instances::withdraw_unusable_key_attempt`]). Such a launch is MADE —
    /// the attempt is an instance of the account, terminated under Amazon EC2's own reason —
    /// while a reason carrying none is a launch no attempt was ever made for.
    pub withdrawn_attempt: Option<ec2_instances::VolumeKeyRefusal>,
}

impl BlockedLaunch {
    /// A launch that could not be MADE: Amazon EC2 Auto Scaling itself could not make the
    /// attempt, the group's configuration asking for an instance the account's own state
    /// cannot produce.
    #[must_use]
    pub fn failed(status_message: String) -> Self {
        Self {
            status_message,
            status_code: ACTIVITY_FAILED,
            withdrawn_attempt: None,
        }
    }

    /// A launch that produced no instance IN SERVICE: the launch source it would have run
    /// resolves to nothing in Amazon EC2, so no attempt was made at all, or the attempt that
    /// was made cannot be brought into service and is taken back — and the capacity change
    /// ends CANCELLED either way.
    #[must_use]
    pub fn cancelled(status_message: String) -> Self {
        Self {
            status_message,
            status_code: ACTIVITY_CANCELLED,
            withdrawn_attempt: None,
        }
    }

    /// A launch Amazon EC2 ACCEPTED and then took back: the instance it launched cannot come
    /// into service — a volume it boots cannot be created under the key it was to be
    /// encrypted under — so the attempt is withdrawn under that service's own state reason
    /// (`refusal`) and the capacity change ends CANCELLED, carrying why.
    #[must_use]
    pub fn cancelled_attempt(
        status_message: String,
        refusal: ec2_instances::VolumeKeyRefusal,
    ) -> Self {
        Self {
            status_message,
            status_code: ACTIVITY_CANCELLED,
            withdrawn_attempt: Some(refusal),
        }
    }
}

/// What the activity of a launch the group made to MEET its desired capacity records as
/// the reason it began: the difference between the capacity the group is configured for
/// and the capacity its members provide, which is what an instance a group short of its
/// capacity launches is launched in response to.
///
/// Reads as the subject of [`capacity_shortfall_cause`], the counterpart of
/// [`AutoScalingState::capacity_change_cause`] for a launch that moves no size: the
/// desired capacity stands where the change that asked for it left it, and what rises is
/// the capacity in service.
pub const CAPACITY_SHORTFALL_REASON: &str =
    "an instance was launched in response to a difference between desired and actual capacity";

/// The cause a launch that brought a group UP to its desired capacity records: what asked
/// for it, and the capacity it moved the group between — one sentence, the way Amazon EC2
/// Auto Scaling words this class of activity ("… in response to a difference between desired
/// and actual capacity, increasing the capacity from 1 to 2."), so the clause naming the
/// capacities follows the reason on a comma rather than on the colon
/// [`AutoScalingState::capacity_change_cause`] states a capacity CHANGE with.
pub fn capacity_shortfall_cause(from: i32, to: i32) -> String {
    format!("{CAPACITY_SHORTFALL_REASON}, increasing the capacity from {from} to {to}.")
}

/// What the CONVERGENCE a termination request triggers records as the reason it began: a
/// caller named the instance, which is what tells the activity log a manual termination
/// apart from the scale-in a capacity change chose for itself.
///
/// Reads as the subject of [`AutoScalingState::capacity_change_cause`] when the request
/// also asked for the desired capacity to be decremented, and as a sentence of its own
/// when it did not. This is the reason the launch a request WITHOUT that decrement leaves
/// the group owing records; the activity of each instance the request took out of service
/// carries the sentence this service words that class with
/// ([`user_requested_instance_move_cause`]).
pub const TERMINATE_INSTANCE_REASON: &str =
    "a user request explicitly terminated an instance of the group";

/// The most instances this emulator will hold for one Auto Scaling group, and so
/// the largest `MinSize`, `MaxSize` or `DesiredCapacity` any operation accepts.
///
/// A BOUND ON THIS PROCESS, not a value AWS documents. Every unit of capacity is
/// a stored instance plus a stored scaling activity, so a single well-formed
/// request carrying a capacity in the billions would exhaust this process's
/// memory and wedge every other caller: the bounds the operations check against
/// each other (`MinSize <= MaxSize`, `DesiredCapacity` inside them) are all
/// relative and cannot stop that. Amazon EC2 Auto Scaling bounds the same growth
/// with an adjustable account quota and answers `LimitExceededFault` past it;
/// this ceiling plays that role, at a size an in-memory emulator can hold.
pub const MAX_GROUP_CAPACITY: i32 = 1_000;

/// The capacity units one instance contributes to a group whose launch source states no
/// weight for its instance type.
///
/// "Usually, you set these sizes based on a specific number of instances. However, if you
/// configure a mixed instances policy that defines weights for the instance types, you must
/// specify these sizes with the same units that you use for weighting instances"
/// (`LaunchTemplateOverrides$WeightedCapacity`): an unweighted group's size IS its instance
/// count, which is that same accounting with every instance worth one unit.
pub const UNWEIGHTED_CAPACITY_UNITS: i32 = 1;

/// The least capacity units an override may say an instance type provides, from the member's
/// own documentation: "Value must be in the range of 1–999"
/// (`LaunchTemplateOverrides$WeightedCapacity`).
pub const MIN_WEIGHTED_CAPACITY: i32 = 1;

/// The most capacity units an override may say an instance type provides, from the same
/// sentence.
pub const MAX_WEIGHTED_CAPACITY: i32 = 999;

/// The capacity units a `WeightedCapacity` names, as a number.
///
/// The member is a STRING carrying a count — "the number of capacity units provided by each
/// instance type", whose "value must be in the range of 1–999"
/// (`LaunchTemplateOverrides$WeightedCapacity`, so between [`MIN_WEIGHTED_CAPACITY`] and
/// [`MAX_WEIGHTED_CAPACITY`]) — and what it names is what both the counting and the
/// reporting use: the number is taken EXACTLY as the override states it, never reshaped,
/// because a group reports that same string back on every read of a member and of its own
/// policy, so a capacity counted at some other number would contradict what the group says
/// it is counting in.
///
/// Text that names no number, and a number below the one unit an instance can least
/// contribute, name no weight: an instance of such an override contributes the
/// [`UNWEIGHTED_CAPACITY_UNITS`] every unweighted instance does, which is the one reading
/// under which a group's capacity can still be fulfilled at all.
///
/// THE ONE reading of that member in this crate, so the units a capacity change counts and
/// the units a read reports cannot drift apart.
pub fn weighted_capacity_units(weighted_capacity: &str) -> Option<i32> {
    weighted_capacity
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|units| *units >= MIN_WEIGHTED_CAPACITY)
}

/// The most Auto Scaling groups this emulator will hold, for the same reason as
/// [`MAX_GROUP_CAPACITY`]: a bound on this process, chosen at the order of the
/// service's own adjustable per-Region quota, past which `CreateAutoScalingGroup`
/// answers its modelled `LimitExceededFault`.
pub const MAX_GROUPS: usize = 500;

/// The launch-configuration limit this service REPORTS for the account, through
/// `DescribeAccountLimits`'s `MaxNumberOfLaunchConfigurations`.
///
/// Same order as the service's own adjustable per-Region quota, like
/// [`MAX_GROUPS`]. Nothing enforces it: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so the reported count stays 0 and no caller can
/// approach the limit.
pub const MAX_LAUNCH_CONFIGURATIONS: usize = 200;

/// How fast the account's API call-rate bucket refills, in calls per second, as
/// `DescribeAccountLimits` reports it under `ThrottlingLimits/AccountMaxCallRate`.
///
/// The subtree is RECORDED behaviour rather than modelled: the live awsQuery
/// response carries it while `DescribeAccountLimitsAnswer` declares only the four
/// counts (`smithy_query(query="shape", shape="DescribeAccountLimitsAnswer")`), so
/// the shapes it is written through are hand-authored beside that answer in
/// `ops/types/describe_account_limits_types.rs`.
///
/// The figure is what the account REPORTS, like [`MAX_GROUPS`]: a rate rather than a
/// counter, so nothing about it moves as calls arrive, and nothing in this emulator
/// paces a caller against it. A client that reads its own call budget before pacing
/// its calls needs a rate and a burst size here instead of a null.
pub const ACCOUNT_MAX_CALL_RATE_REFILL_RATE: f64 = 5.0;

/// How many calls the account's API call-rate bucket holds — its burst size — as
/// `DescribeAccountLimits` reports it under `ThrottlingLimits/AccountMaxCallRate`,
/// on the same footing as [`ACCOUNT_MAX_CALL_RATE_REFILL_RATE`].
pub const ACCOUNT_MAX_CALL_RATE_BUCKET_SIZE: i32 = 30;

/// The most target groups ONE call may name, which bounds both the legacy
/// target-group attach and the legacy detach.
///
/// From the model's own documentation for
/// `DetachLoadBalancerTargetGroupsType$TargetGroupARNs`: "You can specify up to 10
/// target groups." The member carries no modelled length, so the bound is the
/// service's own and is checked by the handler.
pub const MAX_TARGET_GROUPS_PER_CALL: usize = 10;

/// The most Classic Load Balancers ONE call may name, which bounds both the legacy
/// load-balancer attach and the legacy detach.
///
/// From the model's own documentation for `AttachLoadBalancersType$LoadBalancerNames`: "You
/// can specify up to 10 load balancers." The member carries no modelled length, so the bound
/// is the service's own and is checked by the handler.
pub const MAX_LOAD_BALANCERS_PER_CALL: usize = 10;

/// The most load-balancer attachments this emulator will hold for ONE Auto Scaling group —
/// its `TargetGroupARNs`, its `LoadBalancerNames`, and the traffic sources naming either —
/// however many calls put them there, counted per collection.
///
/// A BOUND ON THIS PROCESS, like [`MAX_GROUP_CAPACITY`], rather than a figure AWS documents:
/// [`MAX_TARGET_GROUPS_PER_CALL`] bounds what ONE attach may name and nothing else, so a run
/// of attaches would otherwise store caller-supplied ARNs on one group without limit, and
/// every instance of the group owes a registration in every one of them
/// ([`elbv2_targets`]) — the product is what has to be bounded, not either factor. Amazon
/// EC2 Auto Scaling bounds the same growth with an adjustable account quota and answers
/// `LimitExceededFault` past it; this ceiling plays that role, at a size an in-memory
/// emulator can hold, and well above the per-call bound so no single documented call is
/// refused by it.
pub const MAX_TARGET_GROUPS_PER_GROUP: usize = 50;

/// The most scaling activities this service retains; appending past it drops the
/// oldest.
///
/// Also a bound on this process: activities are appended by every capacity change
/// and never removed by one, so an unbounded log would grow for the life of the
/// process. Amazon EC2 Auto Scaling itself only reports activities from the last
/// six weeks, so a retention bound is the shape of the real behaviour too.
pub const MAX_RETAINED_ACTIVITIES: usize = 10_000;

/// How far back a read of the scaling activities reaches.
///
/// From the model's own documentation for `DescribeScalingActivitiesType$ActivityIds`:
/// "Only activities started within the last six weeks can be returned regardless of the
/// activity IDs specified" — stated again on each of the start-time filters, so it is a
/// window the read itself applies rather than a property of any one criterion.
pub const ACTIVITY_WINDOW: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// The most activity ids one read of the scaling activities may name.
///
/// From the model's own documentation for `DescribeScalingActivitiesType$ActivityIds`:
/// "Array Members: Maximum number of 50 IDs." The member carries no modelled length, so
/// the bound is the service's own and is checked by the handler.
pub const MAX_ACTIVITY_IDS_PER_CALL: usize = 50;

/// The most instance IDs ONE move of a group's instances may name.
///
/// From the model's own documentation for the `InstanceIds` member of every operation that
/// moves the instances it names — `EnterStandby`, `ExitStandby` and `AttachInstances`:
/// "You can specify up to 20 instances." The member carries no modelled length, so the
/// bound is the service's own and is checked by the handlers.
pub const MAX_MOVED_INSTANCE_IDS_PER_CALL: usize = 20;

/// The standard sentence this service states for a LIST member carrying more items than
/// the bound it has to satisfy: how many items the request wrote, the member as the WIRE
/// names it (lowerCamel, the spelling the boundary validator uses), and the bound it
/// missed.
///
/// THE ONE COPY of that sentence, the list counterpart of
/// [`constraint_violation_message`]: every per-call list bound this service checks in a
/// handler — because the model does not express it — is reported through here, so two of
/// them cannot drift into two different spellings of the same refusal.
pub fn too_many_items_message(count: usize, member: &str, limit: usize) -> String {
    format!(
        "1 validation error detected: Value with {count} items at '{member}' failed to \
         satisfy constraint: Member must have length less than or equal to {limit}"
    )
}

/// The message a move of a group's instances naming more instance IDs than
/// [`MAX_MOVED_INSTANCE_IDS_PER_CALL`] is refused with: the framework's own envelope for
/// a member value outside its bounds, naming the member as the wire names it and the bound
/// the request missed.
pub fn too_many_moved_instance_ids_message(count: usize) -> String {
    too_many_items_message(count, "instanceIds", MAX_MOVED_INSTANCE_IDS_PER_CALL)
}

/// The message a read of the scaling activities refuses more than
/// [`MAX_ACTIVITY_IDS_PER_CALL`] ids with: the standard sentence this service states for
/// a member value outside its bounds, naming the member as the wire names it and the
/// bound the request missed.
pub fn too_many_activity_ids_message(count: usize) -> String {
    too_many_items_message(count, "activityIds", MAX_ACTIVITY_IDS_PER_CALL)
}

/// The least `HealthCheckGracePeriod` a group may be configured with.
///
/// The member is "the amount of time, in seconds, that Amazon EC2 Auto Scaling waits
/// before checking the health status of an EC2 instance that has come into service"
/// (`CreateAutoScalingGroup$HealthCheckGracePeriod`): a wait is a length of time, so no
/// group holds a negative one, and the model states no range for the member
/// (`smithy_query(query="shape",shape="HealthCheckGracePeriod")` carries none) — so the
/// bound is the service's own and the handlers that write the member check it.
pub const MIN_HEALTH_CHECK_GRACE_PERIOD: i32 = 0;

/// The message a request stating a `HealthCheckGracePeriod` below
/// [`MIN_HEALTH_CHECK_GRACE_PERIOD`] is refused with.
///
/// Fixed text with NO variable part: the sentence states what the member has to be and
/// quotes neither the value the request supplied nor the bound it missed.
pub fn health_check_grace_period_violation_message() -> String {
    "Grace period must be a positive integer.".to_string()
}

/// A stored launch configuration — the `LaunchConfiguration` resource.
///
/// NOTHING in this crate constructs one: `CreateLaunchConfiguration` is refused
/// with `UnsupportedOperation`, so [`ComAmazonawsAutoscalingAutoscaling20110101State::launch_configurations`]
/// stays empty. This is the shape its readers project from
/// (`DescribeLaunchConfigurations`, a group's launch source), which is why they
/// report what the store holds instead of assuming it is empty.
#[derive(Debug, Clone)]
pub struct StoredLaunchConfiguration {
    pub name: String,
    pub arn: String,
    pub image_id: String,
    pub instance_type: String,
    pub key_name: Option<String>,
    pub security_groups: Vec<String>,
    pub classic_link_vpc_id: Option<String>,
    pub classic_link_vpc_security_groups: Vec<String>,
    pub user_data: Option<String>,
    pub kernel_id: Option<String>,
    pub ramdisk_id: Option<String>,
    pub spot_price: Option<String>,
    pub iam_instance_profile: Option<String>,
    pub block_device_mappings: Vec<BlockDeviceMapping>,
    pub metadata_options: Option<InstanceMetadataOptions>,
    /// `InstanceMonitoring$Enabled`. The model documents the default as `true`
    /// (detailed monitoring).
    pub instance_monitoring_enabled: bool,
    pub ebs_optimized: bool,
    pub associate_public_ip: Option<bool>,
    pub placement_tenancy: Option<String>,
    pub created_time: SystemTime,
}

/// One instance attached to a group.
///
/// [`StoredInstance::instance_id`] is the id of an Amazon EC2 instance OF THE ACCOUNT,
/// launched through [`ec2_instances`] by the capacity change that placed it, so every
/// id a group reports resolves in the account's own Amazon EC2 state.
#[derive(Debug, Clone)]
pub struct StoredInstance {
    pub instance_id: String,
    pub availability_zone: String,
    pub lifecycle_state: String,
    pub health_status: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub instance_type: Option<String>,
    pub image_id: Option<String>,
    pub protected_from_scale_in: bool,
    /// When this instance was launched, which is how old it is: what a scale-in's
    /// `TerminationPolicies` order the group's instances by
    /// ([`termination_policies`]). Not part of what the API reports for an instance.
    pub launched_at: SystemTime,
    /// Where this instance falls in the order the account's instances were launched in,
    /// minted once per instance from [`AutoScalingState::next_instance_launch_seq`]. The
    /// instances of ONE capacity change share [`Self::launched_at`] but not this, since
    /// Amazon EC2 launches them one after another — so the oldest of them is a single
    /// instance. Not part of what the API reports for an instance.
    pub launch_sequence: u64,
    /// When this instance's transient lifecycle state comes due, if it is in one.
    ///
    /// Unfilled while a lifecycle hook is holding the instance
    /// ([`Self::pending_lifecycle_actions`]): nothing the clock owes moves a held instance
    /// on, only the resolution of the hold does.
    pub transition_due: Option<SystemTime>,
    /// What the group's health check OWES this instance, for one a failed check has recorded
    /// unhealthy: the instant it is taken out of service and the group launches its
    /// replacement, and the sentence the check that failed records
    /// ([`instance_health::HealthCheckDebt`]). `None` for an instance no check has failed
    /// for, which is every instance the group launches.
    pub health_check_due: Option<HealthCheckDebt>,
    /// The lifecycle actions the group's own lifecycle hooks are holding this instance at,
    /// one per hook configured for the transition it is taking ([`lifecycle_actions`]).
    /// Empty for an instance no hook is holding, which is every instance of a group that
    /// configures no hook for its transition.
    pub pending_lifecycle_actions: Vec<StoredLifecycleAction>,
    /// The step the group is mid-way through for this instance while a hook holds it — the
    /// activity it recorded and the event it queued — neither of which is a fact until that
    /// hold resolves ([`lifecycle_actions::HeldStep`]).
    pub held_step: Option<HeldStep>,
    /// Whether this instance came into service while the group had the process that
    /// registers its instances with its load balancing suspended
    /// ([`StoredGroup::registers_with_load_balancers`]), so the group registers it with
    /// NONE of its attachments ([`elbv2_targets::wanted_of`]).
    ///
    /// Written once, when the group brought the instance into service, and never cleared:
    /// resuming the process registers the instances that come into service after it, not the
    /// ones that came into service while it was suspended
    /// ([`suspended_processes::PROCESS_ADD_TO_LOAD_BALANCER`]). `false` for every instance
    /// brought into service by a group running that process. Not part of what the API
    /// reports for an instance.
    pub load_balancer_registration_withheld: bool,
    /// WHERE this instance came from into the place it now stands, which is the `Origin` of
    /// the move that brought it there ([`AutoScalingState::instance_move`]).
    ///
    /// [`LifecycleEndpoint::Ec2`] for an instance the group launched from cold and for one
    /// a caller attached, and [`LifecycleEndpoint::WarmPool`] for one the group took out of
    /// its own warm pool — written by each move that changes which collection of the group
    /// holds the instance ([`warm_pools`]), so the two ends an event reports are the move
    /// the instance actually made. Not part of what the API reports for an instance.
    pub arrived_from: LifecycleEndpoint,
}

impl StoredInstance {
    /// This instance as the API reports it — the `Instance` shape.
    ///
    /// THE ONE projection of a stored instance onto the wire shape, so the instances a
    /// group reports and the instances of its warm pool are reported identically.
    ///
    /// `weighted_capacity` is "the number of capacity units contributed by the instance
    /// based on its instance type" (`Instance$WeightedCapacity`), which is a fact about the
    /// GROUP's launch source rather than about the instance: the group reads it off its own
    /// weights for the instance's type ([`StoredGroup::instance_view`]), so a weight an
    /// update changes is reported changed on the instances already in service. Unfilled for
    /// an instance whose group states no weight for its type, which is how an unweighted
    /// group's members are reported.
    ///
    /// `AvailabilityZoneId` is "the Availability Zone ID where the instance was launched"
    /// (`Instance$AvailabilityZoneId`), which is the ID of the zone this instance is in:
    /// the two members are two spellings of ONE placement, so the id is read off the zone
    /// rather than stored beside it ([`availability_zone_ids`]).
    pub fn to_instance(&self, weighted_capacity: Option<String>) -> Instance {
        Instance {
            instance_id: self.instance_id.clone(),
            instance_type: self.instance_type.clone(),
            availability_zone: self.availability_zone.clone(),
            availability_zone_id: zone_id_of(&self.availability_zone),
            lifecycle_state: self.lifecycle_state.clone(),
            health_status: self.health_status.clone(),
            launch_configuration_name: self.launch_configuration_name.clone(),
            launch_template: self.launch_template.clone(),
            image_id: self.image_id.clone(),
            protected_from_scale_in: self.protected_from_scale_in,
            weighted_capacity,
        }
    }

    /// Whether this instance is parked out of service in `Standby`, or on its way there.
    ///
    /// "If you choose to decrement the desired capacity of the Auto Scaling group, the
    /// instances can enter standby as long as the desired capacity of the Auto Scaling
    /// group after the instances are placed into standby is equal to or greater than the
    /// minimum capacity of the group. If you choose not to decrement the desired capacity
    /// of the Auto Scaling group, the Auto Scaling group launches new instances to replace
    /// the instances on standby" (`EnterStandby`'s own documentation): a parked instance
    /// is one the group's capacity is NOT met by, which is what makes a replacement owed.
    pub fn is_in_standby(&self) -> bool {
        self.lifecycle_state == LIFECYCLE_ENTERING_STANDBY
            || self.lifecycle_state == LIFECYCLE_STANDBY
    }
}

/// A capacity change deferred until the group's cooldown has elapsed, because the
/// request asked for the cooldown to be honoured.
#[derive(Debug, Clone)]
pub struct PendingCapacity {
    pub desired_capacity: i32,
    pub due: SystemTime,
    pub cause: String,
    /// The cooldown, in seconds, this change leaves the group in once it is applied, where
    /// the change carries one of its OWN rather than the group's `DefaultCooldown`. `None`
    /// for a change that leaves the group's own default in force, which is what every
    /// deferral this service stores carries today: a capacity set outright
    /// (`SetDesiredCapacity`) states no cooldown of its own, and neither does the retry a
    /// launch owes ([`requested_launches`]).
    pub cooldown_override: Option<i32>,
}

/// One deferred capacity change the clock has made due, as the pass that applies them reads
/// it: which group it moves, the capacity it moves to inside the bounds in force, what it
/// records as the reason, the instant it came due, and the cooldown of its own it leaves
/// behind.
#[derive(Debug, Clone)]
struct DueCapacityChange {
    group_name: String,
    desired_capacity: i32,
    cause: String,
    due: SystemTime,
    cooldown_override: Option<i32>,
}

/// A stored Auto Scaling group — the `AutoScalingGroup` resource, as created and
/// updated.
#[derive(Debug, Clone)]
pub struct StoredGroup {
    pub name: String,
    pub arn: String,
    /// The Region the group was created in, which is the Region the instances it
    /// launches run in.
    pub region: String,
    pub launch_configuration_name: Option<String>,
    pub launch_template: Option<LaunchTemplateSpecification>,
    pub mixed_instances_policy: Option<MixedInstancesPolicy>,
    /// The instance type the group's ATTRIBUTE-BASED launch source selected, resolved once
    /// when the group was configured with it
    /// ([`instance_requirements::policy_requirements_type`]).
    ///
    /// `None` for a group whose launch source names its instance type outright or states no
    /// requirements at all, and `None` for one whose requirements no offered type satisfies
    /// — [`instance_requirements::selects_by_requirements`] tells those apart. Held here, as
    /// part of the configuration, so the requests that converge the group afterwards read a
    /// type rather than walking Amazon EC2's catalog again.
    pub requirements_instance_type: Option<String>,
    pub min_size: i32,
    pub max_size: i32,
    pub desired_capacity: i32,
    pub default_cooldown: i32,
    pub availability_zones: Vec<String>,
    pub availability_zone_ids: Option<Vec<String>>,
    pub load_balancer_names: Option<Vec<String>>,
    pub target_group_arns: Option<Vec<String>>,
    pub health_check_type: String,
    pub health_check_grace_period: Option<i32>,
    pub placement_group: Option<String>,
    pub vpc_zone_identifier: Option<String>,
    pub termination_policies: Vec<String>,
    pub new_instances_protected_from_scale_in: bool,
    /// The service-linked role the group calls other Amazon Web Services under. Every
    /// group has one — the request's own, or the account's default service-linked role
    /// for this service when the request named none ([`service_linked_role`]) — so this
    /// is a value rather than an option, and every read reports the role in force.
    pub service_linked_role_arn: String,
    pub max_instance_lifetime: Option<i32>,
    pub capacity_rebalance: Option<bool>,
    pub context: Option<String>,
    pub desired_capacity_type: Option<String>,
    pub default_instance_warmup: Option<i32>,
    pub traffic_sources: Option<Vec<TrafficSourceIdentifier>>,
    pub instance_maintenance_policy: Option<InstanceMaintenancePolicy>,
    pub availability_zone_distribution: Option<AvailabilityZoneDistribution>,
    pub availability_zone_impairment_policy: Option<AvailabilityZoneImpairmentPolicy>,
    pub capacity_reservation_specification: Option<CapacityReservationSpecification>,
    pub instance_lifecycle_policy: Option<InstanceLifecyclePolicy>,
    pub operator: Option<Operator>,
    pub deletion_protection: Option<String>,
    pub tags: Vec<Tag>,
    pub status: Option<String>,
    pub created_time: SystemTime,
    pub instances: Vec<StoredInstance>,
    /// End of the group's cooldown after its last applied capacity change.
    pub cooldown_until: Option<SystemTime>,
    /// When that cooldown BEGAN: the instant the capacity change it follows was applied.
    ///
    /// Kept beside the end instant because the SPAN in force is not the same for every caller:
    /// a specific simple scaling policy may carry a cooldown period of its own, which
    /// "overrides the default cooldown" (`PutScalingPolicyType$Cooldown`), so what such a
    /// policy's execution waits out is that span measured from here rather than the one the
    /// change left behind ([`Self::in_cooldown_of_span`]). Set and cleared together with
    /// [`Self::cooldown_until`], so a group inside a cooldown always knows both ends of it.
    pub cooldown_started: Option<SystemTime>,
    /// A capacity change waiting for [`Self::cooldown_until`].
    pub pending_capacity: Option<PendingCapacity>,
    /// Whether a launch into this group could NOT place what it was asked for, so the
    /// capacity the group is short of is still OWED.
    ///
    /// A group maintains the capacity it is configured for, so a launch that placed nothing
    /// does not abandon it: the group is marked here, and the capacity it is short of is
    /// launched again as soon as nothing stands in the way of placing it
    /// ([`AutoScalingState::materialize_due`]). HOW MUCH is never stored — it is counted
    /// from the group's own members at the instant the launch is made — so the mark says
    /// only that a launch was stopped.
    ///
    /// Set by the one launch path a group's own instances come from, and cleared both by a
    /// launch that placed everything it was asked for and by the group reaching its
    /// capacity. A launch of NOTHING leaves it alone, since it says nothing about what the
    /// group could place. So an instance that left the group AFTER a launch that worked —
    /// one a launch lifecycle hook abandoned, say — leaves no mark and is not relaunched
    /// here: that is the business of whatever took it out.
    pub owes_launch: bool,
    /// When the group may ATTEMPT an owed launch again, or `None` when nothing holds it back.
    ///
    /// Set by the one launch path a group's own instances come from, for the launches Amazon
    /// EC2 accepts and then takes the instance back from
    /// ([`ec2_instances::withdraw_unusable_key_attempt`]): each of those attempts creates and
    /// terminates an instance of the account, so the next one is owed at an INSTANT
    /// ([`WITHDRAWN_LAUNCH_RETRY`]) rather than made on every pass over the store. Cleared
    /// with [`Self::owes_launch`], and never consulted by a capacity change a CALLER asked
    /// for: a client that repairs the key and asks for the capacity again is served at once.
    ///
    /// It is an instant the CLOCK waits on while the group still owes a launch
    /// ([`lifecycle_events`]), so the attempt is made when it falls due whether or not
    /// anybody calls — a client watching the group through its event stream makes no further
    /// Auto Scaling call.
    pub launch_retry_due: Option<SystemTime>,
    /// The group's lifecycle hooks, keyed by `LifecycleHookName`: the ONE collection
    /// every reader and writer of a hook of this group reaches
    /// ([`lifecycle_hooks`]), so deleting the group takes its hooks with it.
    pub lifecycle_hooks: LifecycleHooks,
    /// The group's Amazon SNS notification configurations, keyed by `TopicARN`: the ONE
    /// collection every reader and writer of a configuration of this group reaches
    /// ([`notification_configurations`]), so deleting the group takes them with it.
    pub notification_configurations: NotificationConfigurations,
    /// The group's warm pool, once `PutWarmPool` has created one: the ONE record every
    /// reader and writer of this group's warm pool reaches ([`warm_pools`]), so deleting
    /// the group takes its warm pool — and the instances it holds — with it.
    pub warm_pool: Option<StoredWarmPool>,
    /// The scaling processes suspended for this group, keyed by process name: the ONE
    /// collection every reader and writer of a suspension of this group reaches
    /// ([`suspended_processes`]), so deleting the group takes its suspensions with it.
    pub suspended_processes: SuspendedProcesses,
    /// The group's scheduled scaling actions, keyed by `ScheduledActionName`: the ONE
    /// collection every reader and writer of a scheduled action of this group reaches
    /// ([`scheduled_actions`]), so deleting the group takes its scheduled actions with it.
    pub scheduled_actions: ScheduledActions,
    /// The group metrics collected for this group, keyed by metric name, each holding the
    /// granularity it is collected at: the ONE collection every reader and writer of this
    /// group's metrics collection reaches ([`metrics_collection`]), so deleting the group
    /// takes its metrics collection with it.
    pub enabled_metrics: EnabledMetrics,
    /// The registrations of this group's instances that the load balancing service REFUSED
    /// and this group has already recorded a failed scaling activity for
    /// ([`RefusedRegistrations`]).
    ///
    /// A refusal is recorded ONCE: the reconcile that owes the registration runs again on
    /// every write to the group and on every wake-up of the clock, and what a client reads
    /// off the group is one activity per refusal rather than one per pass. A mark is kept
    /// only while the refusal it stands for still stands ([`elbv2_targets::settle_group`]),
    /// so a client that takes the attachment off and makes it again reads the refusal again.
    pub refused_registrations: RefusedRegistrations,
}

/// One registration of a group that the load balancing service refused: the target group it
/// was owed in, and the instance of the group it was owed for.
pub type RefusedRegistrationKey = (String, String);

/// The registrations of one group that have already been recorded as refused, each once.
pub type RefusedRegistrations = BTreeSet<RefusedRegistrationKey>;

impl StoredGroup {
    /// The instances that count towards the group's capacity: everything that is
    /// not on its way out and not parked in `Standby`.
    ///
    /// A parked instance is out of service, so the group's desired capacity is not met by
    /// it: that is what makes `EnterStandby` without a decrement launch a replacement and
    /// `ExitStandby` need none ([`StoredInstance::is_in_standby`]).
    pub fn active_instances(&self) -> impl Iterator<Item = &StoredInstance> {
        self.instances
            .iter()
            .filter(|i| !i.is_leaving_group() && !i.is_in_standby())
    }

    /// How many instances count towards the group's capacity.
    ///
    /// NOT what the group's size parameters are stated in once its launch source weights
    /// its instance types: [`Self::active_capacity_units`] is the capacity those parameters
    /// are met in.
    pub fn active_instance_count(&self) -> i32 {
        i32::try_from(self.active_instances().count()).unwrap_or(i32::MAX)
    }

    /// The capacity units one instance of `instance_type` contributes to this group.
    ///
    /// "If you provide a list of instance types to use, you can specify the number of
    /// capacity units provided by each instance type … the capacity units count toward the
    /// desired capacity" (`LaunchTemplateOverrides$WeightedCapacity`): the weight belongs to
    /// the override that names the type, so an instance's weight is read off the group's own
    /// mixed instances policy rather than frozen on the instance when it was launched — a
    /// group whose weights an update rewrites counts and reports its members by the weights
    /// in force. `None` for a type no override of this group names a weight for, and for a
    /// group with no mixed instances policy at all: such an instance contributes the single
    /// [`UNWEIGHTED_CAPACITY_UNITS`] every unweighted instance does, and the member is left
    /// unfilled on every read of it.
    pub fn weighted_capacity_of(&self, instance_type: Option<&str>) -> Option<String> {
        let instance_type = instance_type?;
        self.mixed_instances_policy
            .as_ref()?
            .launch_template
            .as_ref()?
            .overrides
            .as_ref()?
            .iter()
            .find(|entry| entry.instance_type.as_deref() == Some(instance_type))?
            .weighted_capacity
            .clone()
    }

    /// The capacity units one instance of `instance_type` contributes to this group, as a
    /// number: the attribute its `DesiredCapacityType` counts capacity in, or — for a group
    /// counted in instances — the weight the group states for that type, or the single unit
    /// an unweighted instance contributes.
    ///
    /// "The unit of measurement for the value specified for desired capacity"
    /// (`CreateAutoScalingGroup$DesiredCapacityType`) decides which: a group counted in
    /// `vcpu` is fulfilled by its members' vCPUs and one counted in `memory-mib` by their
    /// memory, both read from Amazon EC2's own published facts for the type
    /// ([`instance_requirements::capacity_units`]).
    pub fn capacity_units_for_type(&self, instance_type: Option<&str>) -> i32 {
        instance_requirements::capacity_units(self.desired_capacity_type.as_deref(), instance_type)
            .or_else(|| {
                self.weighted_capacity_of(instance_type)
                    .as_deref()
                    .and_then(weighted_capacity_units)
            })
            .unwrap_or(UNWEIGHTED_CAPACITY_UNITS)
    }

    /// The `WeightedCapacity` a read of one of this group's instances reports for an
    /// instance of `instance_type`: "the number of capacity units contributed by the
    /// instance based on its instance type" (`Instance$WeightedCapacity`).
    ///
    /// The units this group COUNTS that instance for ([`Self::capacity_units_for_type`]),
    /// so a group counted in `vcpu` reports each member's vCPUs; `None` for an unweighted
    /// group counted in instances, whose members report no weight at all.
    pub fn reported_capacity_units(&self, instance_type: Option<&str>) -> Option<String> {
        if let Some(units) =
            instance_requirements::capacity_units(self.desired_capacity_type.as_deref(), instance_type)
        {
            return Some(units.to_string());
        }
        self.weighted_capacity_of(instance_type)
    }

    /// The capacity units `instance` contributes to this group, as a number: the weight the
    /// group states for its instance type, or the single unit an unweighted instance
    /// contributes.
    pub fn instance_capacity_units(&self, instance: &StoredInstance) -> i32 {
        self.capacity_units_for_type(instance.instance_type.as_deref())
    }

    /// This group's view of one of its instances — one of its own or one of its warm pool's
    /// — as the API reports it, carrying the capacity units that instance contributes.
    ///
    /// THE ONE place a read of this service fills `WeightedCapacity` in, so the weight the
    /// group's members report and the weight its capacity is counted in are the same weight.
    pub fn instance_view(&self, instance: &StoredInstance) -> Instance {
        instance.to_instance(self.reported_capacity_units(instance.instance_type.as_deref()))
    }

    /// The capacity the group's own members currently provide, in the units its
    /// `DesiredCapacity`, `MinSize` and `MaxSize` are stated in.
    ///
    /// "Every Auto Scaling group has three size parameters … if you configure a mixed
    /// instances policy that defines weights for the instance types, you must specify these
    /// sizes with the same units that you use for weighting instances"
    /// (`LaunchTemplateOverrides$WeightedCapacity`): the capacity a group holds is therefore
    /// the SUM of its members' weights, which for an unweighted group is their count.
    pub fn active_capacity_units(&self) -> i32 {
        self.active_instances().fold(0i32, |units, instance| {
            units.saturating_add(self.instance_capacity_units(instance))
        })
    }

    /// The capacity units the instances `instance_ids` names contribute, counting each id
    /// once: what the group's capacity falls by when they stop counting towards it, and
    /// rises by when they start.
    ///
    /// The ids are read into a SET first, so one pass over the group's members answers the
    /// whole call however many ids it names.
    pub fn capacity_units_of(&self, instance_ids: &[String]) -> i32 {
        let named: BTreeSet<&str> = instance_ids.iter().map(String::as_str).collect();
        self.instances
            .iter()
            .filter(|instance| named.contains(instance.instance_id.as_str()))
            .fold(0i32, |units, instance| {
                units.saturating_add(self.instance_capacity_units(instance))
            })
    }

    /// The instance type ONE launch of this group produces, as far as the group's own
    /// configuration states it: the type the first override of its mixed instances policy
    /// names, or the type the attribute-based selection of that override came to when the
    /// group was configured ([`Self::requirements_instance_type`]).
    ///
    /// `None` for a group whose launch source leaves the type to its launch template or its
    /// launch configuration, which are the other services' records rather than this group's
    /// own statement — and for one whose requirements admit no offered type, whose launches
    /// fail instead ([`ec2_instances::launch_failure_reason`]).
    pub fn launch_instance_type(&self) -> Option<String> {
        instance_requirements::launch_override(self.mixed_instances_policy.as_ref())
            .and_then(|entry| entry.instance_type.clone())
            .or_else(|| self.requirements_instance_type.clone())
    }

    /// The capacity units one instance a launch of this group makes will contribute.
    ///
    /// A launch from a mixed instances policy is made from the FIRST of the policy's
    /// overrides ([`ec2_instances`] resolves the instance type from it, the way an
    /// allocation strategy prefers the first type it is given), so what that override
    /// launches is what the instance it produces counts for: the attribute the group's
    /// `DesiredCapacityType` counts capacity in, or the weight the override carries. One
    /// unit for a group whose launch source states no weight.
    ///
    /// A group counted in an attribute whose launch source leaves the type to its launch
    /// template states no type of its own to count, so the units are read off a member it
    /// has already launched — the group's members all come from that one launch source, so
    /// what one of them provides is what the next will.
    pub fn launch_capacity_units(&self) -> i32 {
        let launch_instance_type = self.launch_instance_type().or_else(|| {
            self.active_instances()
                .next()
                .and_then(|instance| instance.instance_type.clone())
        });
        if let Some(units) = instance_requirements::capacity_units(
            self.desired_capacity_type.as_deref(),
            launch_instance_type.as_deref(),
        ) {
            return units;
        }
        instance_requirements::launch_override(self.mixed_instances_policy.as_ref())
            .and_then(|entry| entry.weighted_capacity.as_deref())
            .and_then(weighted_capacity_units)
            .unwrap_or(UNWEIGHTED_CAPACITY_UNITS)
    }

    /// Whether the group is inside the cooldown that followed its last capacity
    /// change.
    pub fn in_cooldown(&self, now: SystemTime) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }

    /// The same question for a caller whose own cooldown period stands in for the group's:
    /// whether `span_secs` has elapsed since the capacity change the group's cooldown follows.
    ///
    /// "A cooldown period, in seconds, that applies to a specific simple scaling policy. When a
    /// cooldown period is specified here, it overrides the default cooldown."
    /// (`PutScalingPolicyType$Cooldown`) — so an execution of a policy that states one waits for
    /// THAT period to complete (`ExecutePolicyType$HonorCooldown`, "waits for the cooldown
    /// period to complete before executing the policy"), counted from the same activity, and the
    /// group's `DefaultCooldown` decides nothing for it. A caller with no period of its own — a
    /// manual capacity change, a policy that states none — is answered by [`Self::in_cooldown`],
    /// the group's own.
    ///
    /// A group no capacity change has moved is inside no cooldown whatever span is asked about:
    /// there is no activity for one to follow.
    pub fn in_cooldown_of_span(&self, span_secs: Option<i32>, now: SystemTime) -> bool {
        let Some(span_secs) = span_secs else {
            return self.in_cooldown(now);
        };
        let Some(started) = self.cooldown_started else {
            return false;
        };
        now < started + Duration::from_secs(u64::from(span_secs.max(0).unsigned_abs()))
    }

    /// How many more instances this group can be given before it reaches
    /// [`MAX_GROUP_CAPACITY`].
    ///
    /// Counts the instances still `Terminating`: they remain stored until the
    /// clock retires them, so they occupy the ceiling too. Without that, a caller
    /// alternating the capacity faster than the transition comes due could stack
    /// a fresh ceiling's worth of instances on top of the ones still leaving. Counts
    /// the instances of the group's warm pool for the same reason: they are instances of
    /// the group too — "in the warm pool or in any state except Terminated for the Auto
    /// Scaling group" (`PutWarmPoolType$MaxGroupPreparedCapacity`) — so the ceiling
    /// bounds the two together.
    pub fn launch_headroom(&self) -> i32 {
        let held = self
            .instances
            .len()
            .saturating_add(self.warm_pool_instance_count());
        MAX_GROUP_CAPACITY.saturating_sub(i32::try_from(held).unwrap_or(i32::MAX))
    }

    /// How many instances this group must launch to reach `desired_capacity`.
    ///
    /// `desired_capacity` is a capacity in the group's own units, so the launches it needs
    /// are what it takes to FULFIL it rather than its difference from the instance count:
    /// "Amazon EC2 Auto Scaling launches instances until the desired capacity is totally
    /// fulfilled, even if this results in an overage … if there are two units remaining to
    /// fulfill capacity, and Amazon EC2 Auto Scaling can only launch an instance with a
    /// WeightedCapacity of five units, the instance is launched, and the desired capacity is
    /// exceeded by three units" (`LaunchTemplateOverrides$WeightedCapacity`) — so the
    /// shortfall is divided by the weight of the instance a launch produces and ROUNDED UP.
    /// For an unweighted group every instance is worth one unit, which is the difference
    /// this answered before any group had weights.
    pub fn launches_needed(&self, desired_capacity: i32) -> i32 {
        let shortfall = desired_capacity.saturating_sub(self.active_capacity_units());
        if shortfall <= 0 {
            return 0;
        }
        let units = self.launch_capacity_units().max(UNWEIGHTED_CAPACITY_UNITS);
        shortfall.saturating_add(units - 1) / units
    }

    /// The capacity units this group holds ABOVE `desired_capacity`, which is what a
    /// scale-in has to take out of service.
    ///
    /// Zero for a group whose members do not fulfil the capacity asked for: there is
    /// nothing surplus to take out of one that is still short.
    pub fn surplus_capacity_units(&self, desired_capacity: i32) -> i32 {
        self.active_capacity_units()
            .saturating_sub(desired_capacity)
            .max(0)
    }

    /// The target groups attached to this group, as the group itself holds them:
    /// empty for a group that has none, so a caller never distinguishes "no
    /// attachments" from an absent member.
    pub fn attached_target_groups(&self) -> &[String] {
        self.target_group_arns.as_deref().unwrap_or_default()
    }

    /// The Classic Load Balancers attached to this group, as the group itself holds them:
    /// empty for a group that has none, so a caller never distinguishes "no attachments"
    /// from an absent member.
    pub fn attached_load_balancers(&self) -> &[String] {
        self.load_balancer_names.as_deref().unwrap_or_default()
    }

    /// The instances `instance_ids` names that a termination would take out of service:
    /// each one ONCE, in request order.
    ///
    /// An id this group does not hold contributes nothing, an id the request repeats
    /// names one instance, and one already `Terminating` is already on its way out — so
    /// this is both what a termination acts on and what its decrement of the desired
    /// capacity counts. An instance the group RETAINED is taken: a retained instance
    /// "remain[s] until you call `TerminateInstanceInAutoScalingGroup`"
    /// (`RetentionTriggers$TerminateHookAbandon`), so that call is exactly what takes it.
    pub fn instances_to_terminate(&self, instance_ids: &[String]) -> Vec<String> {
        let mut taken: Vec<String> = Vec::new();
        for instance_id in instance_ids {
            let still_in_service = self.instances.iter().any(|instance| {
                instance.instance_id == *instance_id
                    && (!instance.is_leaving_group() || instance.is_retained())
            });
            if still_in_service && !taken.contains(instance_id) {
                taken.push(instance_id.clone());
            }
        }
        taken
    }
}

/// A stored scaling activity — the `Activity` resource a capacity change produces.
#[derive(Debug, Clone)]
pub struct StoredActivity {
    pub activity_id: String,
    pub auto_scaling_group_name: String,
    pub auto_scaling_group_arn: Option<String>,
    pub description: Option<String>,
    pub cause: String,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub status_code: String,
    /// Why the activity did not do what it set out to, for an activity that failed:
    /// `Activity$StatusMessage`, which the description names as the status reason.
    pub status_message: Option<String>,
    pub progress: i32,
    pub details: Option<String>,
    /// When this activity finishes, if it has not finished yet.
    pub completes_at: Option<SystemTime>,
    /// The terminal `ScalingActivityStatusCode` the clock finishes this activity in, for one
    /// whose step did not do what it set out to.
    ///
    /// An activity the clock completes is [`ACTIVITY_SUCCESSFUL`] unless the step it records
    /// was settled as one that did not carry through
    /// ([`AutoScalingState::settle_held_step`]), which is what keeps the activity log and
    /// the event announcing the same step in agreement.
    pub completes_as: Option<&'static str>,
}

impl StoredActivity {
    /// This activity as the API reports it — the `Activity` shape.
    ///
    /// THE ONE projection of a stored activity onto the wire shape, so the activity an
    /// operation answers with and a later read of the same activity report it
    /// identically.
    ///
    /// `group_state` is the `AutoScalingGroupState` to report, or `None` to leave that
    /// optional member out. The state of the group is reported only by a read that
    /// reports the state of groups at all — the one a caller asked to be told about
    /// deleted groups on — and there it is read off whether the account still holds the
    /// group ([`AutoScalingState::activity_group_state`]), so a group deleted since the
    /// activity was recorded is reported deleted on every activity of its own.
    pub fn to_activity(&self, group_state: Option<&str>) -> Activity {
        Activity {
            activity_id: self.activity_id.clone(),
            auto_scaling_group_name: self.auto_scaling_group_name.clone(),
            description: self.description.clone(),
            cause: self.cause.clone(),
            start_time: to_date_time(self.start_time),
            end_time: self.end_time.map(to_date_time),
            status_code: self.status_code.clone(),
            // "If an attempt to launch instances failed, the StatusCode value is Failed or
            // Cancelled and the StatusMessage element in the response indicates the cause
            // of the failure" (`DescribeScalingActivities`'s own documentation): the
            // reason the failed activity recorded IS that cause, stated apart from the
            // description of what the activity set out to do, and an activity that did not
            // fail has none.
            status_message: self.status_message.clone(),
            progress: Some(self.progress),
            details: self.details.clone(),
            auto_scaling_group_state: group_state.map(str::to_string),
            auto_scaling_group_arn: self.auto_scaling_group_arn.clone(),
        }
    }
}

/// A stored instance refresh — the `InstanceRefresh` resource `StartInstanceRefresh`
/// creates and `DescribeInstanceRefreshes` reports.
///
/// A refresh replaces the instances of ONE group, one at a time: the instances it has
/// still to replace are [`Self::pending_instance_ids`], the one it is part way through is
/// [`Self::in_flight`], and the clock brings the next move due at [`Self::next_step_due`]
/// ([`AutoScalingState::advance_instance_refreshes`]). The progress a reader sees is
/// what those replacements have actually done.
#[derive(Debug, Clone)]
pub struct StoredInstanceRefresh {
    pub instance_refresh_id: String,
    pub auto_scaling_group_name: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    /// `RefreshStrategy`, as requested or [`DEFAULT_REFRESH_STRATEGY`].
    pub strategy: String,
    pub preferences: Option<RefreshPreferences>,
    pub desired_configuration: Option<DesiredConfiguration>,
    /// How many instances the refresh set out to replace, which is what its
    /// percentage complete is a percentage OF.
    pub instances_at_start: i32,
    /// The instances it has still to replace, in the order it replaces them. Once a
    /// rollback is under way this holds what the ROLLBACK has still to replace — the
    /// replacements the refresh had made, which the rollback replaces back.
    pub pending_instance_ids: Vec<String>,
    /// The instances the refresh LAUNCHED as replacements, in replacement order: the
    /// only instances of the group the refresh changed, and so exactly what a rollback
    /// has to replace back.
    pub replaced_instance_ids: Vec<String>,
    /// `PercentageComplete`.
    pub percentage_complete: i32,
    /// `InstancesToUpdate`.
    pub instances_to_update: i32,
    /// The rollback of this refresh, once one has been asked for: `RollbackDetails`.
    pub rollback: Option<StoredRollback>,
    /// The replacement it is part way through, and the phase that replacement has reached:
    /// a refresh makes each replacement in two moves, with the window it runs under
    /// deciding their order ([`refresh_plan`]).
    pub in_flight: Option<RefreshReplacement>,
    /// When the next MOVE — the next half of a replacement, the first move of the next
    /// one, or the refresh's own completion — comes due;
    /// `None` once it has reached a terminal status.
    pub next_step_due: Option<SystemTime>,
}

/// The rollback of an instance refresh — the `RollbackDetails` a refresh reports once
/// one has been asked for: why it was started, when it started, and the progress the
/// refresh had made at that instant.
#[derive(Debug, Clone)]
pub struct StoredRollback {
    /// `RollbackReason`.
    pub reason: String,
    /// `RollbackStartTime`.
    pub start_time: SystemTime,
    /// `PercentageCompleteOnRollback`: "the value of PercentageComplete at the time the
    /// rollback started".
    pub percentage_complete_on_rollback: i32,
    /// `InstancesToUpdateOnRollback`: "the value of InstancesToUpdate at the time the
    /// rollback started".
    pub instances_to_update_on_rollback: i32,
}

/// Why a group's instance refresh could not be rolled back, for the operation to
/// answer the error class each situation belongs to.
#[derive(Debug)]
pub enum RollbackRefusal {
    /// The group has no instance refresh in progress, and no rollback either, so there
    /// is nothing for a rollback to act on.
    NoneInProgress,
    /// The refresh in progress cannot be reversed; the sentence says why.
    Irreversible(String),
}

impl StoredInstanceRefresh {
    /// Whether this refresh has not reached a terminal status, which is what makes it
    /// the group's ACTIVE refresh: a group may have only one at a time, and one whose
    /// cancellation has not completed still holds its group.
    pub fn is_active(&self) -> bool {
        self.is_in_progress()
            || self.is_baking()
            || self.status == REFRESH_CANCELLING
            || self.is_rolling_back()
    }

    /// Whether this refresh is BAKING: it has finished updating instances and is waiting
    /// out the bake time it runs under, which is not a terminal status —
    /// "`Baking` - Waiting the specified bake time after an instance refresh has finished
    /// updating instances" (`smithy_query(query="doc",shape="InstanceRefresh",
    /// member="Status")`).
    pub fn is_baking(&self) -> bool {
        self.status == REFRESH_BAKING
    }

    /// Whether this refresh is BEING ROLLED BACK: the status that means its own
    /// replacements are being replaced back, which is a rollback a later call finds
    /// already running rather than one it starts.
    pub fn is_rolling_back(&self) -> bool {
        self.status == REFRESH_ROLLBACK_IN_PROGRESS
    }

    /// Whether a cancellation acts on this refresh: `CancelInstanceRefresh` "cancels an
    /// instance refresh OR ROLLBACK that is in progress"
    /// (`smithy_query(query="operation",operation="CancelInstanceRefresh")`), so the
    /// replacements a rollback is part way through are stopped by the same call that
    /// stops a refresh's own — and so is the wait a refresh that has finished replacing
    /// is still owed, which the sentence a group with nothing to cancel is refused with
    /// names beside the rest.
    pub fn is_cancellable(&self) -> bool {
        self.is_in_progress() || self.is_baking() || self.is_rolling_back()
    }

    /// Whether this refresh is IN PROGRESS: the statuses that mean it is still
    /// replacing instances.
    pub fn is_in_progress(&self) -> bool {
        self.status == REFRESH_PENDING || self.status == REFRESH_IN_PROGRESS
    }

    /// How long this refresh waits after a replacement it made has come into service
    /// before it moves on.
    ///
    /// "A time period, in seconds, during which an instance refresh waits before moving on
    /// to replacing the next instance after a new instance enters the `InService` state"
    /// (`smithy_query(query="doc",shape="RefreshPreferences",member="InstanceWarmup")`), so
    /// the wait is the refresh's OWN effective `InstanceWarmup` — the value the request
    /// stated, or the group's `DefaultInstanceWarmup`/`HealthCheckGracePeriod` the model
    /// falls back to, resolved once when the refresh was created
    /// ([`refresh_preferences::effective`]). A refresh running under no warmup at all waits
    /// nothing beyond the replacement itself, which is the span this service owes every
    /// transient state ([`INSTANCE_REFRESH_REPLACEMENT`]).
    pub fn instance_warmup(&self) -> Duration {
        let seconds = self
            .preferences
            .as_ref()
            .and_then(|preferences| preferences.instance_warmup)
            .unwrap_or(0)
            .max(0);
        Duration::from_secs(u64::from(seconds.unsigned_abs()))
    }

    /// How long this refresh waits, once it has finished updating instances, before it is
    /// complete.
    ///
    /// "The amount of time, in seconds, to wait at the end of an instance refresh before
    /// the instance refresh is considered complete"
    /// (`smithy_query(query="doc",shape="RefreshPreferences",member="BakeTime")`), so the
    /// wait is the `BakeTime` the request stated, in the seconds it stated it in: a
    /// documented wait keeps the value the caller asked for. A refresh that stated none,
    /// or stated none to wait, bakes for nothing and is complete as soon as it has
    /// replaced every instance it set out to.
    pub fn bake_time(&self) -> Duration {
        let seconds = self
            .preferences
            .as_ref()
            .and_then(|preferences| preferences.bake_time)
            .unwrap_or(0)
            .max(0);
        Duration::from_secs(u64::from(seconds.unsigned_abs()))
    }

    /// How long this refresh waits at a checkpoint it has reached before it carries on.
    ///
    /// "The amount of time, in seconds, to wait after a checkpoint before continuing"
    /// (`smithy_query(query="doc",shape="RefreshPreferences",member="CheckpointDelay")`), so
    /// the wait is the `CheckpointDelay` the refresh runs under, in the seconds it was
    /// stated in: a documented wait keeps the value the caller asked for. A refresh that
    /// stated thresholds and no delay waits the hour the same member documents for it
    /// ([`refresh_preferences::DEFAULT_CHECKPOINT_DELAY`]), and one that stated no
    /// threshold has no checkpoint to wait at at all.
    pub fn checkpoint_delay(&self) -> Duration {
        let preferences = self.preferences.as_ref();
        let seconds = match preferences.and_then(|preferences| preferences.checkpoint_delay) {
            Some(stated) => stated.max(0),
            None if preferences
                .and_then(|preferences| preferences.checkpoint_percentages.as_ref())
                .is_some() =>
            {
                refresh_preferences::DEFAULT_CHECKPOINT_DELAY
            }
            None => 0,
        };
        Duration::from_secs(u64::from(seconds.unsigned_abs()))
    }

    /// The wait this refresh owes before its NEXT step, given that the step just
    /// completed carried it from `from` percent complete to `to`.
    ///
    /// "Threshold values for each checkpoint in ascending order. Each number must be
    /// unique" (`smithy_query(query="doc",shape="RefreshPreferences",
    /// member="CheckpointPercentages")`): a threshold is reached by the step whose
    /// replacement carried the refresh's progress up to or past it, and the refresh then
    /// holds at that checkpoint — still replacing, so still reported `InProgress` at the
    /// percentage it reached — for the delay it runs under ([`Self::checkpoint_delay`])
    /// before continuing. A step that reached no threshold, and every step of a refresh
    /// that states none, waits nothing beyond the replacement itself.
    pub fn checkpoint_hold(&self, from: i32, to: i32) -> Duration {
        let reached = self
            .preferences
            .as_ref()
            .and_then(|preferences| preferences.checkpoint_percentages.as_ref())
            .is_some_and(|thresholds| {
                thresholds
                    .iter()
                    .any(|threshold| from < *threshold && *threshold <= to)
            });
        if reached {
            self.checkpoint_delay()
        } else {
            Duration::ZERO
        }
    }
}

/// Per-service state for com-amazonaws-autoscaling-autoscaling-2011-01-01. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAutoscalingAutoscaling20110101State {
    /// Launch configurations, keyed by `LaunchConfigurationName`.
    ///
    /// Empty for the life of the process: `CreateLaunchConfiguration` is refused
    /// with `UnsupportedOperation` because the account this emulator models cannot
    /// create a launch configuration at all. The collection stays here because
    /// `DescribeLaunchConfigurations`, `DeleteLaunchConfiguration` and a group's
    /// `LaunchConfigurationName` all answer FROM it rather than from a constant,
    /// so each of them reports what the store holds.
    pub launch_configurations: BTreeMap<String, StoredLaunchConfiguration>,
    /// Auto Scaling groups, keyed by `AutoScalingGroupName`.
    pub groups: BTreeMap<String, StoredGroup>,
    /// The scaling policies of those groups, keyed by the group they belong to and their
    /// name within it ([`PolicyKey`]), so the collection iterates group by group and, in
    /// a group, policy name by policy name. A group's policies are removed with the
    /// group: a policy only exists against a group that does.
    pub scaling_policies: BTreeMap<PolicyKey, StoredScalingPolicy>,
    /// Every scaling activity this service has recorded, oldest first.
    pub activities: Vec<StoredActivity>,
    /// Every instance refresh this service has recorded, oldest first.
    pub instance_refreshes: Vec<StoredInstanceRefresh>,
    /// Source of activity ids.
    pub next_activity_seq: u64,
    /// Source of the opaque id segment of a resource ARN.
    pub next_resource_seq: u64,
    /// Source of the launch order an instance of a group keeps
    /// ([`StoredInstance::launch_sequence`]).
    pub next_instance_launch_seq: u64,
    /// The instance lifecycle events this store has not published to EventBridge yet,
    /// oldest first, each due at the instant the step it reports completes
    /// ([`lifecycle_events`]). At most [`lifecycle_events::MAX_PENDING_EVENTS`] entries,
    /// the oldest dropped first.
    pending_lifecycle_events: VecDeque<PendingLifecycleEvent>,
    /// The lifecycle-action events this store has not published to EventBridge yet, oldest
    /// first, each due from the instant the hold it announces started
    /// ([`lifecycle_actions`]). At most [`MAX_PENDING_LIFECYCLE_ACTION_EVENTS`] entries, the
    /// oldest dropped first.
    pending_lifecycle_action_events: VecDeque<PendingLifecycleActionEvent>,
    /// Whether a clock pump ([`drive_lifecycle_clock`]) is already waiting on this store's
    /// due instants, so at most one runs per store.
    lifecycle_clock_pump_running: bool,
    /// The generation of the running pump's claim, so a pump releases its OWN claim and
    /// never a later pump's.
    lifecycle_clock_pump_generation: u64,
    /// The running pump's task handle, kept so the pump it replaces is aborted rather than
    /// merely forgotten.
    lifecycle_clock_pump: Option<::tokio::task::JoinHandle<()>>,
    /// What each launch request's client token answered, oldest first: a repeat of a token
    /// is answered with what its first request produced rather than launching again
    /// ([`requested_launches`]). At most [`MAX_RETAINED_LAUNCH_REQUESTS`] entries, the
    /// oldest dropped first.
    launch_requests: Vec<StoredLaunchRequest>,
    /// Where the CloudWatch series of each collecting group has got to: the minute each
    /// group's collected metrics are next published at, and when that minute comes due
    /// ([`group_metrics`]). A group that collects nothing has no schedule here.
    group_metrics: GroupMetricPublisher,
}

/// A shorter name for this service's state, for use inside the crate.
pub type AutoScalingState = ComAmazonawsAutoscalingAutoscaling20110101State;

/// Render `seq` in the 8-4-4-4-12 shape AWS resource ids use, so an id this
/// emulator mints has the form a client expects while staying deterministic
/// within a process (version nibble 4, variant nibble 8, as RFC 4122 lays them out).
fn uuid_shaped(seq: u64) -> String {
    format!("{:08x}-0000-4000-8000-{:012x}", seq & 0xffff_ffff, seq)
}

impl AutoScalingState {
    /// Mint the next activity id.
    pub fn mint_activity_id(&mut self) -> String {
        self.next_activity_seq += 1;
        uuid_shaped(self.next_activity_seq)
    }

    /// Mint an opaque resource id: the segment an Auto Scaling ARN carries, and the
    /// id an instance refresh is tracked by. ONE sequence, so no two resources of
    /// this service are ever minted the same id.
    pub fn mint_resource_id(&mut self) -> String {
        self.next_resource_seq += 1;
        uuid_shaped(self.next_resource_seq)
    }

    /// Remove every scaling policy of `group_name` and answer them, so the caller can
    /// retire the CloudWatch alarms they held once this store's lock is released.
    ///
    /// A policy only exists against a group that exists, so deleting the group deletes
    /// its policies: nothing a later put or read of the same name sees is left behind.
    pub fn take_group_policies(
        &mut self,
        group_name: &str,
    ) -> BTreeMap<PolicyKey, StoredScalingPolicy> {
        let keys: Vec<PolicyKey> = self
            .scaling_policies
            .keys()
            .filter(|key| key.auto_scaling_group_name == group_name)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| {
                self.scaling_policies
                    .remove(&key)
                    .map(|policy| (key, policy))
            })
            .collect()
    }

    /// Store one instance refresh under a freshly minted id and answer that id,
    /// keeping the log bounded.
    ///
    /// The ONE writer of [`Self::instance_refreshes`], so every refresh's id comes
    /// from one minter and [`MAX_RETAINED_INSTANCE_REFRESHES`] is applied in one
    /// place: `instance_refresh_id` on the way in is ignored.
    pub fn push_instance_refresh(&mut self, refresh: StoredInstanceRefresh) -> String {
        let instance_refresh_id = self.mint_resource_id();
        self.instance_refreshes.push(StoredInstanceRefresh {
            instance_refresh_id: instance_refresh_id.clone(),
            ..refresh
        });
        // Keep the log bounded: the oldest refresh is the first to go.
        if self.instance_refreshes.len() > MAX_RETAINED_INSTANCE_REFRESHES {
            let excess = self.instance_refreshes.len() - MAX_RETAINED_INSTANCE_REFRESHES;
            self.instance_refreshes.drain(..excess);
        }
        instance_refresh_id
    }

    /// The activity `activity_id` names, if this store still holds it.
    ///
    /// An operation that reports the activity it just recorded reads the STORED record
    /// back rather than building a second copy of it, so what it answers with and what a
    /// later read of the log reports are one activity.
    pub fn activity(&self, activity_id: &str) -> Option<&StoredActivity> {
        self.activities
            .iter()
            .find(|activity| activity.activity_id == activity_id)
    }

    /// The group that holds the instance `instance_id`, if one does.
    ///
    /// An Amazon EC2 instance is part of at most one Auto Scaling group, so a request
    /// that names an instance without naming its group is answered from the group the
    /// instance itself belongs to.
    pub fn group_holding_instance(&self, instance_id: &str) -> Option<&StoredGroup> {
        self.groups.values().find(|group| {
            group
                .instances
                .iter()
                .any(|instance| instance.instance_id == instance_id)
        })
    }

    /// The group's ACTIVE instance refresh, if it has one: the one still running.
    pub fn active_instance_refresh(&self, group_name: &str) -> Option<&StoredInstanceRefresh> {
        self.instance_refreshes
            .iter()
            .find(|refresh| refresh.auto_scaling_group_name == group_name && refresh.is_active())
    }

    /// Cancel the group's in-progress instance refresh at `now`, answering the id of
    /// the refresh the cancellation acted on, or `None` when the group has none in
    /// progress.
    ///
    /// A cancellation starts no further replacement and rolls none back, so the
    /// progress the refresh had already made stands: it moves to `Cancelling` and the
    /// clock brings it to `Cancelled` ([`INSTANCE_REFRESH_CANCELLATION`]). The replacement
    /// it was part way through is FINISHED rather than abandoned, so the group is left
    /// holding the capacity it is configured for ([`refresh_plan`]) — and a cancellation of
    /// a ROLLBACK finishes the replacement that rollback was part way through, from the
    /// launch source the rollback was restoring. With
    /// `wait_for_transitioning_instances`, the cancellation is owed only once the group's
    /// in-flight launches and terminations have themselves settled; without it, the
    /// cancellation does not wait for them.
    pub fn cancel_instance_refresh<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        wait_for_transitioning_instances: bool,
        now: SystemTime,
    ) -> Option<String> {
        let index = self.instance_refreshes.iter().position(|refresh| {
            refresh.auto_scaling_group_name == group_name && refresh.is_cancellable()
        })?;
        // Before the in-flight launches and terminations are counted below, so the
        // cancellation of a refresh that waits for them waits for this one too. The refresh
        // being cancelled may itself be a ROLLBACK — "cancels an instance refresh or
        // rollback that is in progress" ([`StoredInstanceRefresh::is_cancellable`]) — which
        // the settling reads off the refresh's own status, so the replacement it finishes is
        // made from the launch source that refresh was replacing from.
        self.settle_stopped_replacement(engine, index, RefreshStop::Cancellation, now);
        // The in-flight launches and terminations of the group: the instances whose
        // transient lifecycle state has not come due yet.
        let in_flight_settled = if wait_for_transitioning_instances {
            self.groups
                .get(group_name)
                .into_iter()
                .flat_map(|group| group.instances.iter())
                .filter_map(|instance| instance.transition_due)
                .chain([now])
                .max()
                .unwrap_or(now)
        } else {
            now
        };
        let refresh = self.instance_refreshes.get_mut(index)?;
        refresh.status = REFRESH_CANCELLING.to_string();
        refresh.next_step_due = Some(in_flight_settled + INSTANCE_REFRESH_CANCELLATION);
        Some(refresh.instance_refresh_id.clone())
    }

    /// Start the rollback of the group's instance refresh at `now`, answering the id of
    /// the refresh the rollback acts on.
    ///
    /// A rollback "cancels an instance refresh that is in progress and rolls back any
    /// changes that it made": the replacements the refresh had made are themselves
    /// replaced, from the group's own launch source — the configuration it was using
    /// before the refresh started, since a refresh's desired configuration reaches the
    /// group only once that refresh has succeeded. The replacements are owed by the
    /// clock, one per [`INSTANCE_REFRESH_REPLACEMENT`], exactly as the refresh's own
    /// were ([`Self::advance_instance_refreshes`]).
    ///
    /// A rollback already under way is the one this call finds: it answers that
    /// refresh's id and restarts nothing, so asking twice replaces no instance twice.
    pub fn roll_back_instance_refresh<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        now: SystemTime,
    ) -> Result<String, RollbackRefusal> {
        if let Some(refresh) = self
            .instance_refreshes
            .iter()
            .find(|refresh| refresh.auto_scaling_group_name == group_name && refresh.is_rolling_back())
        {
            return Ok(refresh.instance_refresh_id.clone());
        }
        // Whether the GROUP's own configuration can be rolled back to at all is answered
        // BEFORE the question of whether there is a refresh to roll back: a group whose
        // launch template version floats pins no version to go back TO, so no refresh of
        // it could ever be reversed, whichever one a caller asks about.
        if let Some(reason) = self.group_rollback_refusal_reason(engine, group_name) {
            return Err(RollbackRefusal::Irreversible(reason));
        }
        // The refresh a rollback acts on: one still replacing instances, or one waiting
        // out the bake time it runs under — the two statuses the sentence this operation
        // refuses a group with nothing to roll back with names.
        let Some(index) = self.instance_refreshes.iter().position(|refresh| {
            refresh.auto_scaling_group_name == group_name
                && (refresh.is_in_progress() || refresh.is_baking())
        }) else {
            return Err(RollbackRefusal::NoneInProgress);
        };
        if let Some(reason) = self.irreversible_reason(engine, group_name, index) {
            return Err(RollbackRefusal::Irreversible(reason));
        }

        // The replacement the refresh was part way through when the rollback arrived is
        // settled FIRST, so what the rollback then has to replace back is read off a group
        // holding exactly the capacity it is configured for: the gap an in-place replacement
        // had opened is filled, and the surge another had launched is taken back out and is
        // no longer one of the refresh's own replacements ([`refresh_plan`]).
        self.settle_stopped_replacement(engine, index, RefreshStop::Rollback, now);

        // What the rollback has to replace back: the replacements the refresh made that
        // the group still holds. One the group has since lost is not replaced again —
        // the rollback restores the configuration, not a capacity change somebody else
        // made.
        let held: Vec<String> = self
            .groups
            .get(group_name)
            .into_iter()
            .flat_map(StoredGroup::active_instances)
            .map(|instance| instance.instance_id.clone())
            .collect();
        let Some(refresh) = self.instance_refreshes.get_mut(index) else {
            return Err(RollbackRefusal::NoneInProgress);
        };
        refresh.pending_instance_ids = refresh
            .replaced_instance_ids
            .iter()
            .filter(|instance_id| held.contains(instance_id))
            .cloned()
            .collect();
        refresh.rollback = Some(StoredRollback {
            reason: MANUAL_ROLLBACK_REASON.to_string(),
            start_time: now,
            percentage_complete_on_rollback: refresh.percentage_complete,
            instances_to_update_on_rollback: refresh.instances_to_update,
        });
        refresh.status = REFRESH_ROLLBACK_IN_PROGRESS.to_string();
        refresh.next_step_due = Some(now + INSTANCE_REFRESH_REPLACEMENT);
        Ok(refresh.instance_refresh_id.clone())
    }

    /// Why the refresh at `index` cannot be rolled back, when it cannot: the sentence
    /// the refusal carries, or `None` for a refresh a rollback can reverse.
    ///
    /// The situation this answers is the refresh's OWN: "a desired configuration was not
    /// found" (`smithy_query(query="shape",shape="IrreversibleInstanceRefreshFault")`) —
    /// without one the refresh changed nothing to go back from. The two situations that are
    /// facts of the GROUP's launch source instead are
    /// [`Self::group_rollback_refusal_reason`], which the rollback answers ahead of this.
    fn irreversible_reason<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        group_name: &str,
        index: usize,
    ) -> Option<String> {
        let refresh = self.instance_refreshes.get(index)?;
        if refresh.desired_configuration.is_none() {
            return Some(ROLLBACK_WITHOUT_DESIRED_CONFIGURATION_MESSAGE.to_string());
        }
        self.group_rollback_refusal_reason(engine, group_name)
    }

    /// Why no refresh of `group_name` could be rolled back, when none could: the
    /// situations that are facts of the GROUP's own launch source rather than of any one
    /// refresh.
    ///
    /// "An incompatible launch template (uses a Systems Manager parameter instead of an
    /// AMI ID) or launch template version (`$Latest` or `$Default`) is present on the Auto
    /// Scaling group" (`smithy_query(query="shape",shape="IrreversibleInstanceRefreshFault")`):
    /// neither pins a configuration a rollback could go back to, whichever refresh is asked
    /// about — which is why a rollback is refused for them before the group is even looked
    /// at for a refresh in progress.
    fn group_rollback_refusal_reason<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        group_name: &str,
    ) -> Option<String> {
        let group = self.groups.get(group_name)?;
        let specification = launch_template_of(
            group.launch_template.as_ref(),
            group.mixed_instances_policy.as_ref(),
        )?;
        let version = specification
            .version
            .as_deref()
            .unwrap_or(LAUNCH_TEMPLATE_VERSION_DEFAULT);
        if version == LAUNCH_TEMPLATE_VERSION_DEFAULT || version == LAUNCH_TEMPLATE_VERSION_LATEST {
            return Some(FLOATING_LAUNCH_TEMPLATE_VERSION_MESSAGE.to_string());
        }
        // What the template's own version says the instances run, from the account's
        // Amazon EC2 store: an image named through a Systems Manager parameter is not an
        // AMI id, so the image the group was running before the refresh is not pinned
        // either.
        ec2_instances::resolves_image_through_ssm_parameter(engine, specification)
            .then(|| SSM_PARAMETER_IMAGE_ID_MESSAGE.to_string())
    }

    /// Materialise everything the clock has already made due at `now`: deferred
    /// capacity changes whose cooldown has elapsed, the scheduled actions whose occurrence
    /// has come due, the replacements a running
    /// instance refresh owes, instance lifecycle transitions, the capacity a group was
    /// left owing, activity
    /// completions, and the size each group's warm pool maintains.
    ///
    /// Every handler calls this first — including the reporting ones, which may
    /// write exactly this much and nothing else, because each of these changes was
    /// owed at `now` whether or not anybody called.
    ///
    /// `engine` is the shared engine the handler holds: a deferred capacity change
    /// launches and terminates the account's own Amazon EC2 instances when it comes
    /// due, exactly as the change a request applies straight away does.
    pub fn materialize_due<Eng: Send + Sync + 'static>(&mut self, engine: &Eng, now: SystemTime) {
        let due_changes: Vec<DueCapacityChange> = self
            .groups
            .values()
            .filter_map(|group| {
                group
                    .pending_capacity
                    .as_ref()
                    .filter(|pending| pending.due <= now)
                    .map(|pending| DueCapacityChange {
                        group_name: group.name.clone(),
                        // The group's bounds may have been narrowed while the
                        // change waited, and a group never holds a capacity
                        // outside its own bounds, so the deferred value is
                        // brought inside the ones in force when it is applied.
                        desired_capacity: capacity_within_bounds(
                            pending.desired_capacity,
                            group.min_size,
                            group.max_size,
                        ),
                        cause: pending.cause.clone(),
                        due: pending.due,
                        cooldown_override: pending.cooldown_override,
                    })
            })
            .collect();
        // Applied AT the instant it came due, not at the instant somebody looked:
        // the instances it launches and the cooldown it starts are anchored there,
        // so the passes below retire whatever that instant has itself made due and
        // the state a caller sees does not depend on when they read.
        for change in due_changes {
            self.apply_capacity(
                engine,
                &change.group_name,
                change.desired_capacity,
                &change.cause,
                change.due,
            );
            // A change carrying a cooldown of its OWN leaves the group in that one rather
            // than in the group's `DefaultCooldown` the convergence above started, whether
            // it was applied straight away or waited for this instant.
            self.override_cooldown(&change.group_name, change.cooldown_override, change.due);
        }
        // The scheduled actions whose occurrence the clock has reached: a stored action is a
        // capacity change owed at ITS instant, so it is performed here and the actions that
        // are no longer scheduled stop being held ([`scheduled_actions`]). Before the
        // alarm-driven executions below, so the cooldown an occurrence left behind is in force
        // when one of them is weighed against it, and before the convergence passes, so the
        // instances the occurrence asks for are part of this same pass.
        self.run_due_scheduled_actions(engine, now);
        // The alarm notifications this store's scaling policies have been handed since the
        // last pass: an alarm that moved owed the policy its action names, so the policy is
        // performed here ([`alarm_driven_policies`]). After the deferred changes above, so the
        // cooldown a change the clock already owed left behind is in force when an
        // alarm-driven execution is weighed against it, and before the convergence passes
        // below, so the instances such an execution asks for are part of this same pass.
        self.receive_alarm_notifications(engine, now);
        // The members the groups have lost to Amazon EC2 itself, and the replacements the
        // groups are then short of: the health check that notices them is owed on the clock
        // like everything else in this pass ([`ec2_health`]). Before the refreshes below, so
        // a refresh reads the instances its group actually holds.
        self.reconcile_terminated_instances(engine, now);
        // And the members a failed health check has recorded unhealthy, which the group's
        // own check acts on as the clock makes it due ([`instance_health`]): the same
        // replacement, owed for the same reason, whichever check failed.
        self.reconcile_unhealthy_instances(engine, now);
        // Before the lifecycle pass below, so an instance a replacement launched at
        // an instant already past leaves its transient state in this same call.
        self.advance_instance_refreshes(engine, now);
        // The holds the clock has timed out: a lifecycle action nobody completed before its
        // own deadline resolves to the hook's `DefaultResult`, which is what keeps an
        // instance from waiting for a client that never came back. Before the lifecycle pass
        // below for the same reason as the refreshes: the wait ended at its own deadline, so
        // whatever that ending itself made due is retired in this same call rather than
        // waiting for another reader.
        self.expire_lifecycle_actions(engine, now);

        for group in self.groups.values_mut() {
            for instance in &mut group.instances {
                if instance.transition_due.is_some_and(|due| due <= now) {
                    instance.transition_due = None;
                    if instance.lifecycle_state == LIFECYCLE_PENDING
                        || instance.lifecycle_state == LIFECYCLE_PENDING_PROCEED
                    {
                        instance.lifecycle_state = LIFECYCLE_IN_SERVICE.to_string();
                    } else if instance.lifecycle_state == LIFECYCLE_ENTERING_STANDBY {
                        // An instance `EnterStandby` started parking has finished parking.
                        instance.lifecycle_state = LIFECYCLE_STANDBY.to_string();
                    } else if instance.lifecycle_state == LIFECYCLE_DETACHING {
                        // An instance `DetachInstances` started releasing has been released:
                        // it is no longer a member of the group, which the retain below is
                        // what carries out.
                        instance.lifecycle_state = LIFECYCLE_DETACHED.to_string();
                    }
                }
            }
            // An instance whose span out of the group has elapsed has left it. One a
            // termination hook is still holding has NOT: its wait is what the group is
            // waiting on, and resolving that hold is what starts the span
            // ([`lifecycle_actions`]). Neither has one the group RETAINED: its termination
            // was abandoned under a retention trigger that keeps it, so it stays a member
            // until a caller takes it out.
            group.instances.retain(|i| {
                !i.is_leaving_group()
                    || i.transition_due.is_some()
                    || i.is_held_by_lifecycle_hook()
                    || i.is_retained()
            });
        }
        // The capacity the groups are still OWED, launched now that nothing stands in the
        // way of placing it ([`Self::converge_capacity`]). After the pass above, so the
        // shortfall is counted from the instances this instant leaves each group with.
        self.converge_capacity(engine, now);
        // The warm pools the groups maintain: the transitions of their instances, and the
        // difference between each pool's size and the size it maintains, which the clock
        // owes exactly as it owes a group's own ([`warm_pools`]).
        self.reconcile_warm_pools(engine, now);
        // The health a group that checks its instances with `ELB` reads off the load
        // balancer: a derived value, recomputed from what that service reports right now,
        // after the pass above has brought this instant's instances into service.
        self.reconcile_elb_instance_health(engine, now);
        // And what that verdict makes owed: the group's own health check, armed on every
        // member it leaves reported unhealthy, which the pass above acts on as the clock
        // makes it due ([`elb_health`], one debt with [`instance_health`]). After the fold,
        // so what it reads is this instant's verdict.
        self.arm_elb_health_check_debt(now);

        for activity in &mut self.activities {
            if activity.completes_at.is_some_and(|at| at <= now) {
                activity.end_time = activity.completes_at;
                activity.completes_at = None;
                activity.status_code = activity
                    .completes_as
                    .unwrap_or(ACTIVITY_SUCCESSFUL)
                    .to_string();
                activity.progress = 100;
            }
        }
        // And the minute each collecting group owes CloudWatch: the group metrics it
        // aggregates, published from the state this pass has just brought up to date
        // ([`group_metrics`]). Last, so a datapoint reports the instances this instant left in
        // service rather than the ones it was about to move.
        self.publish_due_group_metrics(engine, now);
    }

    /// Recompute the `HealthStatus` of the instances of every group that checks their health
    /// with Elastic Load Balancing, from what that service reports for their registrations.
    ///
    /// A group whose `HealthCheckType` names `ELB` checks its instances with "Elastic Load
    /// Balancing health checks", so the health of such an instance is DERIVED: it is
    /// [`HEALTH_STATUS_UNHEALTHY`] exactly while a load balancer the group is attached to
    /// reports that instance's registration as failing its health check, and
    /// [`HEALTH_STATUS_HEALTHY`] otherwise — nothing is stored beside the instance to say so,
    /// and the value is recomputed from that service's own verdict every time the clock is
    /// read. A group that checks its instances any other way derives nothing here and is left
    /// exactly as it stands.
    ///
    /// Whichever FAMILY of load balancer is checking them: the verdict of an Elastic Load
    /// Balancing v2 target group the group is attached to
    /// ([`elbv2_targets::elb_unhealthy_instances`]) and the verdict of a Classic Load Balancer
    /// it is attached to ([`elb_classic_instances::elb_unhealthy_instances`]) are the same
    /// `ELB` health check, so an instance either of them reports as failing is one this
    /// group's own health check has found out of service.
    ///
    /// `HealthCheckGracePeriod` is "the amount of time, in seconds, that Amazon EC2 Auto
    /// Scaling waits before checking the health status of an EC2 instance that has come into
    /// service and marking it unhealthy due to a failed health check", so an instance inside
    /// that window is not marked on a failed check however the load balancer reports it; the
    /// member's own default is [`DEFAULT_HEALTH_CHECK_GRACE_PERIOD`], and the window runs
    /// from the instant the instance came into service ([`LAUNCH_TRANSITION`] after it was
    /// launched). An instance on its way out of the group is leaving whatever its targets
    /// report.
    ///
    /// A derived value the clock owes, which is why a REPORTING operation materialises it:
    /// the load balancer's verdict is the same at a given instant whether or not anybody
    /// called ([`Self::materialize_due`]).
    pub fn reconcile_elb_instance_health<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        // Read once for every group, under the other stores' locks alone and with no group of
        // this store borrowed across the write below. A group attached to both families
        // carries the UNION of what the two report: an instance failing the checks of either
        // load balancer is one the group's `ELB` health check has found out of service, while a
        // group neither answer names is one no such load balancer checks and is left alone.
        let mut verdicts =
            elbv2_targets::elb_unhealthy_instances(engine, self.groups.values(), now);
        for (group_name, unhealthy) in
            elb_classic_instances::elb_unhealthy_instances(engine, self.groups.values(), now)
        {
            verdicts.entry(group_name).or_default().extend(unhealthy);
        }
        for (group_name, unhealthy) in verdicts {
            let Some(group) = self.groups.get_mut(&group_name) else {
                continue;
            };
            // Reaching a verdict about a member is the group's `HealthCheck` process, so a
            // group that has suspended it derives nothing from the load balancer and leaves
            // the health it reports exactly as it stands ([`suspended_processes`]).
            if !group.performs_health_checks() {
                continue;
            }
            let grace = Duration::from_secs(
                u64::try_from(
                    group
                        .health_check_grace_period
                        .unwrap_or(DEFAULT_HEALTH_CHECK_GRACE_PERIOD)
                        .max(0),
                )
                .unwrap_or(0),
            );
            for instance in &mut group.instances {
                // An instance on its way out of the group — terminating, or held at a
                // termination hook — is leaving whatever its targets report, through the one
                // reading of that ([`StoredInstance::is_leaving_group`]).
                if instance.is_leaving_group() {
                    continue;
                }
                // A member the account's Amazon EC2 store no longer runs has been found out of
                // service by a check of its own ([`ec2_health`]), which no verdict about the
                // target it used to occupy takes back: the instance is gone whatever that
                // target reports.
                if instance
                    .health_check_due
                    .is_some_and(|debt| debt.reason == EC2_INSTANCE_GONE_REASON)
                {
                    continue;
                }
                // Checked only once the instance has come into service ([`LAUNCH_TRANSITION`]
                // after the launch placed it) and the group's grace period after that has
                // elapsed.
                let checked_from = instance.launched_at + LAUNCH_TRANSITION + grace;
                if now < checked_from {
                    continue;
                }
                instance.health_status = if unhealthy.contains(&instance.instance_id) {
                    HEALTH_STATUS_UNHEALTHY
                } else {
                    HEALTH_STATUS_HEALTHY
                }
                .to_string();
            }
        }
    }

    /// Set a group's desired capacity and move its instances to match: launch what
    /// is missing (`Pending`, in service once [`LAUNCH_TRANSITION`] is due) and
    /// scale in what is surplus (`Terminating`, gone once [`LIFECYCLE_TRANSITION`] is due).
    /// Each launch and each termination records its own scaling activity, and the group
    /// enters its cooldown.
    ///
    /// The instances are the account's OWN Amazon EC2 instances: the launch goes
    /// through [`ec2_instances::launch`], which mints them in the account's Amazon EC2
    /// store from the group's launch source, and the scale-in terminates those same
    /// instances there — so every id the group reports resolves in Amazon EC2. A unit
    /// of capacity Amazon EC2 could not place records a failed activity instead of an
    /// instance, since there is no instance to report.
    ///
    /// A group that has suspended the process the change would use moves no instance
    /// for it: [`PROCESS_LAUNCH`] is what adds instances on a scale-out and
    /// [`PROCESS_TERMINATE`] is what removes them on a scale-in, so while one is
    /// suspended the group records the capacity asked for and leaves its instances where
    /// they are ([`suspended_processes`]).
    ///
    /// A scale-out of a group whose warm pool has prepared instances CONSUMES them before
    /// anything is launched ([`Self::consume_warm_pool`]): a pre-initialized instance comes
    /// into service under its own id and the pool's depth falls by the number taken, so a
    /// cold launch is made only for the capacity the pool could not cover.
    ///
    /// Does nothing when `group_name` names no group. The capacity is clamped to
    /// [`MAX_GROUP_CAPACITY`] here as well as refused by the handlers, so the
    /// allocation below is bounded however it is reached.
    pub fn apply_capacity<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        desired_capacity: i32,
        cause: &str,
        now: SystemTime,
    ) {
        let desired_capacity = desired_capacity.clamp(0, MAX_GROUP_CAPACITY);
        let Some(group) = self.groups.get(group_name) else {
            return;
        };
        let arn = group.arn.clone();
        let zoneless = group.availability_zones.is_empty();
        let cooldown = group.default_cooldown.max(0);
        // The capacity the group holds as this change is applied, which is what says whether
        // this change is a scaling activity at all ([`Self::cooldown_until`] below).
        let held_capacity = group.desired_capacity;
        // And the capacity its members actually MEET, which is the difference the
        // convergence below acts on — and what the activity of each instance it moves states
        // it moved the group from ([`Self::convergence_cause`]).
        let met_capacity = group.active_capacity_units();
        let headroom = group.launch_headroom();
        // A scale-out is the `Launch` process and a scale-in is the `Terminate` process, so
        // a group that has either suspended does not do that half of the change: the
        // capacity the caller asked for is still what the group holds, while the instances
        // stay as they are ([`suspended_processes`]).
        let launch_suspended = group.is_process_suspended(PROCESS_LAUNCH);
        let terminate_suspended = group.is_process_suspended(PROCESS_TERMINATE);

        let to_launch = if zoneless || launch_suspended {
            // Without a zone to place an instance in, nothing can be launched; the
            // handlers refuse such a request before reaching here.
            0
        } else {
            // What it takes to FULFIL the capacity asked for, counted in the group's own
            // units ([`StoredGroup::launches_needed`]), and bounded by the ceiling AND by
            // what the group can still hold, so neither one request nor a run of them can
            // grow the stored instances past MAX_GROUP_CAPACITY. The handlers refuse a
            // change that does not fit; this is the floor under them.
            group
                .launches_needed(desired_capacity)
                .clamp(0, MAX_GROUP_CAPACITY)
                .min(headroom)
        };
        // And what the group holds above that capacity, in those same units: a scale-in
        // takes the surplus out of service rather than a number of instances, so a member
        // worth several units covers several units of the surplus.
        let surplus_units = if terminate_suspended {
            0
        } else {
            group.surplus_capacity_units(desired_capacity)
        };
        let due = now + LIFECYCLE_TRANSITION;
        // What an instance this change brings INTO service settles at, which is the span a
        // launch the group has already made owes ([`LAUNCH_TRANSITION`]).
        let launch_due = now + LAUNCH_TRANSITION;

        // A group whose warm pool has already prepared instances draws on THOSE before it
        // launches anything: a prepared instance leaves the pool under its own id and comes
        // into service through `Pending` ([`Self::consume_warm_pool`]), so the scale-out
        // only launches what the pool could not cover.
        let needed = usize::try_from(to_launch).unwrap_or(0);
        let promoted = self.consume_warm_pool(engine, group_name, needed, now, launch_due);
        let wanted = needed.saturating_sub(promoted.len());
        let launch = self.launch_into_group(
            engine,
            &GroupLaunch {
                group_name,
                count: wanted,
                launched_at: now,
                due: launch_due,
                // A capacity change launches from the group's own launch source.
                desired_configuration: None,
                destination: LaunchDestination::Group,
                // And places across the whole of the group's own configuration.
                placement: None,
            },
        );
        let launched_ids = launch.launched_ids.clone();
        let unplaced_message = launch.unplaced_message(UNPLACED_INSTANCE_MESSAGE);
        let unplaced_status_code = launch.unplaced_status_code();
        let unplaced_steps = launch.unplaced_steps(&unplaced_message);

        let group = self
            .groups
            .get_mut(group_name)
            .expect("the group was present a moment ago");
        group.desired_capacity = desired_capacity;
        group.pending_capacity = None;
        // The capacity a group holds is one half of the difference its warm pool maintains
        // (`StoredWarmPool::target_size`), so THIS change is what owes the preparation of
        // whatever the pool is now short of ([`warm_pools`]) — from this instant, not from
        // whenever the pool is next read.
        group.owe_warm_pool_preparation(now);
        // A cooldown FOLLOWS a scaling activity — it is what "helps to ensure that your Auto
        // Scaling group doesn't launch or terminate additional instances before the previous
        // scaling activity takes effect" (`AutoScalingGroup$DefaultCooldown`'s own
        // documentation of the period) — so a change that leaves the group's capacity exactly
        // where it already was starts none: a group created at the capacity it asks for, a
        // request that re-states the capacity the group holds, a convergence that had nothing
        // to converge. Without this, a group that has never scaled would be inside a cooldown
        // from the moment it was created, and the first policy performed on it would be
        // blocked by an activity nothing made.
        if held_capacity != desired_capacity {
            group.cooldown_started = Some(now);
            group.cooldown_until =
                Some(now + Duration::from_secs(u64::from(cooldown.unsigned_abs())));
        }

        let terminated_ids = self.scale_in_selection(group_name, surplus_units, now);
        // A warm pool whose own reuse policy asks for it takes those instances BACK rather
        // than letting them be terminated — "whether instances in the Auto Scaling group can
        // be returned to the warm pool on scale in" (`InstanceReusePolicy$ReuseOnScaleIn`) —
        // as far as the pool has room for them ([`Self::reuse_scaled_in_instances`]); the
        // rest leave the group as every other scale-in's do.
        let reuse = self.reuse_scaled_in_instances(group_name, &terminated_ids, now);
        self.terminate_group_instances(engine, group_name, &reuse.terminated, now, due);

        for instance_id in promoted.into_iter().chain(launched_ids) {
            // The activity of an instance the CONVERGENCE brought into service states both
            // what asked for the capacity and the step the group took to meet it — one step
            // whether the instance was launched for it or taken out of the group's warm
            // pool, since either way the group has an instance coming into service.
            let cause = Self::convergence_cause(
                cause,
                InstanceLifecycle::Launched,
                now,
                met_capacity,
                desired_capacity,
            );
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Launched,
                    group_name,
                    group_arn: &arn,
                    instance_id: &instance_id,
                    cause: &cause,
                    start_time: now,
                    due: launch_due,
                },
            );
        }
        for instance_id in reuse.terminated {
            let cause = Self::convergence_cause(
                cause,
                InstanceLifecycle::Terminated,
                now,
                met_capacity,
                desired_capacity,
            );
            self.record_instance_lifecycle_change(
                engine,
                &InstanceLifecycleChange {
                    kind: InstanceLifecycle::Terminated,
                    group_name,
                    group_arn: &arn,
                    instance_id: &instance_id,
                    cause: &cause,
                    start_time: now,
                    due,
                },
            );
        }
        for instance_id in &reuse.returned {
            // An instance the group's warm pool took back was taken out of service by this
            // same convergence, so its activity states that step — and its description states
            // what the group did with it, which is a return rather than a termination
            // ([`warm_pools::warm_pool_return_description`]).
            let cause = Self::convergence_cause(
                cause,
                InstanceLifecycle::Terminated,
                now,
                met_capacity,
                desired_capacity,
            );
            self.record_activity(
                engine,
                &ActivityRecord {
                    group_name,
                    group_arn: &arn,
                    description: warm_pools::warm_pool_return_description(instance_id),
                    cause,
                    start_time: now,
                    completes_at: Some(due),
                    milestone: ActivityMilestone::InProgress,
                    instance_id: Some(instance_id),
                },
            );
        }
        for (instance_id, description) in unplaced_steps {
            // A unit of capacity the group set out to place and did not is as much a step of
            // the group as one it placed: the activity records why, and the event reporting
            // it goes out under the launch's UNSUCCESSFUL detail-type
            // ([`AutoScalingState::record_failed_lifecycle_step`]).
            self.record_failed_lifecycle_step(
                engine,
                &FailedLifecycleStep {
                    kind: InstanceLifecycle::Launched,
                    group_name,
                    group_arn: &arn,
                    instance_id: instance_id.as_deref(),
                    description,
                    status_message: unplaced_message.clone(),
                    status_code: unplaced_status_code,
                    cause,
                    at: now,
                },
            );
        }
    }

    /// Launch the capacity each group is still OWED, now that nothing stands in the way of
    /// placing it.
    ///
    /// A group's `DesiredCapacity` is the capacity it MAINTAINS rather than an instruction
    /// one call carries out — "Amazon EC2 Auto Scaling launches instances until the desired
    /// capacity is totally fulfilled" (`LaunchTemplateOverrides$WeightedCapacity`) — so
    /// capacity a launch COULD NOT PLACE stays owed ([`StoredGroup::owes_launch`]) and is
    /// launched as soon as it can be: a group whose launch was stopped by an attachment it
    /// could not register its instance into reaches its capacity once the client has
    /// repaired that attachment, with no further capacity change from anybody.
    ///
    /// A debt the CLOCK owes, which is why [`Self::materialize_due`] retires it and a
    /// REPORTING operation materialises it: the launch was owed at `now` whether or not
    /// anybody called.
    ///
    /// HOW MUCH is counted HERE, from the group's own members against the capacity it is
    /// configured for ([`StoredGroup::launches_needed`]) rather than from anything stored
    /// when the launch failed — so a group that is owed a launch converges on the capacity
    /// it is short of at this instant, whatever has happened to it since. A group whose
    /// members already provide that capacity is simply no longer owed anything. Neither is
    /// anything launched for a group whose `Launch` process is suspended (that process is
    /// what adds instances to a group, [`suspended_processes`]), one with no zone to place an
    /// instance in, or one still holding a capacity change for its cooldown, which is the
    /// change that will converge it.
    ///
    /// An attempt that places NOTHING records no activity: the capacity stays owed and is
    /// attempted again, while the failure a client reads is the one the call that asked for
    /// the capacity recorded for it.
    fn converge_capacity<Eng: Send + Sync + 'static>(&mut self, engine: &Eng, now: SystemTime) {
        // A group that is no longer short of capacity owes no launch: it owes one only while
        // its members do not provide the capacity it is configured for.
        for group in self.groups.values_mut() {
            if group.owes_launch && group.launches_needed(group.desired_capacity) <= 0 {
                group.owes_launch = false;
                // Nothing is owed, so nothing is waiting for the instant the next attempt
                // was owed at either.
                group.launch_retry_due = None;
            }
        }
        // What each group is owed, read before anything is launched so no group is borrowed
        // across the launch below.
        let owed: Vec<(String, String, usize, i32, i32)> = self
            .groups
            .values()
            .filter(|group| {
                group.owes_launch
                    && group.pending_capacity.is_none()
                    && !group.availability_zones.is_empty()
                    && !group.is_process_suspended(PROCESS_LAUNCH)
                    // A group whose last attempt Amazon EC2 took back tries again when that
                    // attempt is DUE, not on every pass ([`StoredGroup::launch_retry_due`]).
                    && group.launch_retry_due.is_none_or(|due| due <= now)
            })
            .filter_map(|group| {
                // The same shortfall a capacity change launches, bounded the same way
                // ([`Self::apply_capacity`]): what it takes to fulfil the capacity, and
                // never more than the group can still hold.
                let needed = group
                    .launches_needed(group.desired_capacity)
                    .clamp(0, MAX_GROUP_CAPACITY)
                    .min(group.launch_headroom());
                let count = usize::try_from(needed).unwrap_or(0);
                (count > 0).then(|| {
                    (
                        group.name.clone(),
                        group.arn.clone(),
                        count,
                        group.active_capacity_units(),
                        group.desired_capacity,
                    )
                })
            })
            .collect();
        for (name, arn, count, from, desired) in owed {
            // Every instance this pass brings into service settles at the launch span
            // ([`LAUNCH_TRANSITION`]), exactly as the capacity change that asked for the
            // capacity would have settled it.
            let due = now + LAUNCH_TRANSITION;
            // The capacity a group is owed is met from its warm pool first, exactly as a
            // capacity change meets it ([`Self::consume_warm_pool`]): the instances the pool
            // has prepared are the group's to bring into service, so a shortfall the pool
            // can cover is covered without a cold launch.
            let promoted = self.consume_warm_pool(engine, &name, count, now, due);
            let launch = self.launch_into_group(
                engine,
                &GroupLaunch {
                    group_name: &name,
                    count: count.saturating_sub(promoted.len()),
                    launched_at: now,
                    due,
                    // The capacity a group maintains is launched from the group's own launch
                    // source and placed across the whole of its configuration, exactly as
                    // the capacity change that asked for it would have launched it.
                    desired_configuration: None,
                    destination: LaunchDestination::Group,
                    placement: None,
                },
            );
            let cause = capacity_shortfall_cause(from, desired);
            for instance_id in promoted.into_iter().chain(launch.launched_ids) {
                self.record_instance_lifecycle_change(
                    engine,
                    &InstanceLifecycleChange {
                        kind: InstanceLifecycle::Launched,
                        group_name: &name,
                        group_arn: &arn,
                        instance_id: &instance_id,
                        cause: &cause,
                        start_time: now,
                        due,
                    },
                );
            }
        }
    }

    /// Leave the group in the cooldown `seconds` names, counted from `applied_at` — the
    /// cooldown the CHANGE carries rather than the group's `DefaultCooldown`, which
    /// [`Self::apply_capacity`] has just started.
    ///
    /// THE ONE place a capacity change overrides that default, so an execution applied at
    /// once and one the clock applied when its deferral came due leave the same cooldown
    /// behind. Does nothing for a change that carries none, and nothing for a name no group
    /// answers to.
    pub fn override_cooldown(
        &mut self,
        group_name: &str,
        seconds: Option<i32>,
        applied_at: SystemTime,
    ) {
        let Some((seconds, group)) = seconds.zip(self.groups.get_mut(group_name)) else {
            return;
        };
        group.cooldown_started = Some(applied_at);
        group.cooldown_until =
            Some(applied_at + Duration::from_secs(u64::from(seconds.max(0).unsigned_abs())));
    }

    /// Take the instances `instance_ids` names out of service because a CALLER asked
    /// for those instances by id, and answer the scaling activities that did it.
    ///
    /// A termination request is not a capacity change: it names the instances to
    /// terminate, so the `TerminationPolicies` and the scale-in protection that decide
    /// WHICH instances a capacity change picks do not apply to them. Each named instance
    /// the group still holds is reported `Terminating` until [`LIFECYCLE_TRANSITION`] is
    /// due and the same Amazon EC2 instance is terminated where it lives, and each
    /// records its own activity.
    ///
    /// `should_decrement` is what the request says about the group's size: with it the
    /// desired capacity goes down by the capacity this call took out of service — the
    /// instances' own weights, in the units the group's size is stated in
    /// ([`StoredGroup::capacity_units_of`]) — so nothing replaces them; without it the
    /// capacity stands and the group launches a replacement for each, which is the
    /// convergence [`Self::apply_capacity`] performs for every change. The decrement is held
    /// at the group's own `MinSize`, because a group never holds a desired capacity below the
    /// minimum it is configured with — the floor under the caller, which refuses a
    /// request that would take the capacity under it
    /// ([`min_size_breach_message`]) rather than quietly applying less than it asked
    /// for.
    ///
    /// An id the group does not hold, an id the request repeats, and one already on its
    /// way out contribute nothing: the first is refused by the caller, and neither of the
    /// others takes an instance out of service twice or decrements the capacity twice.
    /// Answers no activity when the call has nothing left to take out of service, since
    /// nothing was owed.
    pub fn terminate_instances_in_group<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        instance_ids: &[String],
        should_decrement: bool,
        now: SystemTime,
    ) -> Vec<StoredActivity> {
        let Some(group) = self.groups.get(group_name) else {
            return Vec::new();
        };
        let arn = group.arn.clone();
        let desired_capacity = group.desired_capacity;
        let (min_size, max_size) = (group.min_size, group.max_size);
        // Each instance ONCE, in request order: an id a request repeats names one
        // instance, and one already `Terminating` is already leaving.
        let taken = group.instances_to_terminate(instance_ids);
        if taken.is_empty() {
            return Vec::new();
        }

        // What those instances contribute to the group's capacity, in the units its size is
        // stated in: a member of a weighted group takes its own weight's worth of capacity
        // with it, which is what the decrement below takes off.
        let units = group.capacity_units_of(&taken);
        let new_desired_capacity = if should_decrement {
            capacity_within_bounds(desired_capacity.saturating_sub(units), min_size, max_size)
        } else {
            desired_capacity
        };
        // The clause the CONVERGENCE this request triggers records as what asked for the
        // capacity it launches ([`TERMINATE_INSTANCE_REASON`]), naming the capacities the
        // request moved the group between when it moved them.
        let convergence_cause = if should_decrement {
            Self::capacity_change_cause(
                now,
                TERMINATE_INSTANCE_REASON,
                desired_capacity,
                new_desired_capacity,
            )
        } else {
            Self::reason_cause(now, TERMINATE_INSTANCE_REASON)
        };

        let due = now + LIFECYCLE_TRANSITION;
        self.terminate_group_instances(engine, group_name, &taken, now, due);
        // Each termination is recorded — and its event queued — through THE ONE path a
        // lifecycle step of a group's instance goes down, so a termination a caller named
        // is announced exactly like one a capacity change chose. The activity it stored is
        // what this call answers with, read back by the id that path minted.
        //
        // Its CAUSE is the one this class of activity is worded with: the instance the
        // request named, taken out of service at that request
        // ([`user_requested_instance_move_cause`]), with the capacities the group moved
        // between when the request decremented its capacity. One cause per instance,
        // because the sentence names the instance its own activity is about.
        let activity_ids: Vec<String> = taken
            .iter()
            .map(|instance_id| {
                let instance_cause = user_requested_instance_move_cause(
                    now,
                    instance_id,
                    TAKEN_OUT_OF_SERVICE,
                    desired_capacity,
                    new_desired_capacity,
                );
                self.record_instance_lifecycle_change(
                    engine,
                    &InstanceLifecycleChange {
                        kind: InstanceLifecycle::Terminated,
                        group_name,
                        group_arn: &arn,
                        instance_id,
                        cause: &instance_cause,
                        start_time: now,
                        due,
                    },
                )
            })
            .collect();
        let activities: Vec<StoredActivity> = activity_ids
            .iter()
            .filter_map(|activity_id| self.activity(activity_id).cloned())
            .collect();
        // The group then converges on the capacity the request leaves it with: with the
        // decrement there is nothing to replace, and without it every instance this call
        // took out of service is replaced — the same convergence, and the same cooldown,
        // as every other change to the group's size.
        self.apply_capacity(engine, group_name, new_desired_capacity, &convergence_cause, now);
        activities
    }

    /// Launch instances into the group through the account's own Amazon EC2 store and
    /// give the group its own view of each, answering the ids launched and how many
    /// of the ones asked for could not be placed.
    ///
    /// THE ONE launch path: a capacity change and an instance refresh's replacement
    /// both reach Amazon EC2 through here, so one group's instances are launched from
    /// one resolution of its launch source however the launch was asked for. A launch
    /// Amazon EC2 could not place gives the group no instance — an id it reported
    /// would resolve nowhere — and is counted for the caller to record.
    ///
    /// A launch into the GROUP also owes each of its attachments a registration, so an
    /// attachment whose target group Elastic Load Balancing v2 no longer holds stops it
    /// before anything is minted: nothing is placed and the sentence the caller records for
    /// each unplaced unit names that target group
    /// ([`elbv2_targets::unresolved_attachments`]). An attachment that RESOLVES and would
    /// refuse the registration — a target group whose VPC does not hold the instance — stops
    /// it once the attempt has been made: that instance is taken back and the sentence names
    /// the target group and that service's own refusal
    /// ([`elbv2_targets::refused_launch_registrations`]). The ATTACHMENT itself stays exactly as
    /// the client left it — it is the client's to remove
    /// (`DetachLoadBalancerTargetGroups`), and a group that reported an attachment the
    /// client never detached cannot refuse that detach.
    ///
    /// A launch into the group that could not place what it was asked for leaves the group
    /// OWING it ([`StoredGroup::owes_launch`]): a group maintains the capacity it is
    /// configured for, so the capacity nothing was placed for is launched again as soon as
    /// nothing stands in the way of placing it ([`Self::converge_capacity`]). A launch that
    /// placed everything it was asked for leaves the group owing nothing, and a launch of
    /// NOTHING leaves the mark exactly as it was, since it says nothing about what the group
    /// could place. A launch into a warm pool owes the group nothing either — that pool's
    /// size is maintained by its own reconcile ([`warm_pools`]).
    fn launch_into_group<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        request: &GroupLaunch<'_>,
    ) -> GroupLaunchOutcome {
        let outcome = self.place_launch(engine, request);
        if request.destination == LaunchDestination::Group && request.count > 0 {
            if let Some(group) = self.groups.get_mut(request.group_name) {
                group.owes_launch = outcome.unplaced > 0;
                if !group.owes_launch {
                    // A launch that placed everything owes no further attempt, so nothing is
                    // waiting for the instant the next one was owed at.
                    group.launch_retry_due = None;
                }
            }
        }
        outcome
    }

    /// Place one launch: the whole of [`Self::launch_into_group`] except the capacity it
    /// leaves owed, which that call records for every one of the ways this one answers.
    fn place_launch<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        request: &GroupLaunch<'_>,
    ) -> GroupLaunchOutcome {
        let Some(group) = self.groups.get(request.group_name) else {
            return GroupLaunchOutcome::unplaced(request.count, None);
        };
        // Registering the instance is PART of bringing it into service, so an attachment
        // Elastic Load Balancing v2 can take no registration into stops the launch that
        // would have to register there: the group launches nothing and records the refusal
        // that names the target group instead. A warm pool's own instances, which no
        // attachment takes, are not held to it, and neither is a group that registers
        // nothing because the process that registers is suspended
        // ([`StoredGroup::registers_with_load_balancers`]) — there is then no registration
        // for that attachment to fail.
        //
        // The attachment STAYS: the group keeps reporting what the client attached until the
        // client detaches it, so the refusal is the whole of what the launch did.
        if request.count > 0
            && request.destination == LaunchDestination::Group
            && group.registers_with_load_balancers()
        {
            let unresolved = elbv2_targets::unresolved_attachments(engine, group);
            if !unresolved.is_empty() {
                let reason = elbv2_targets::launch_blocked_message(&unresolved);
                return GroupLaunchOutcome::unplaced(
                    request.count,
                    Some(BlockedLaunch::failed(reason)),
                );
            }
        }
        let region = group.region.clone();
        // The role the group makes this launch under, which is the principal the authorities
        // over what the launch references are weighed for ([`volume_encryption_keys`]).
        let service_linked_role_arn = group.service_linked_role_arn.clone();
        // Where the instances go: the placement the CALLER named, when a launch request
        // named one of the group's own ([`requested_launches`]), and otherwise the whole of
        // the group's configuration — which is what every launch the group makes for itself
        // places from.
        let (zones, vpc_zone_identifier) = match request.placement {
            Some(placement) => (
                placement.zones.clone(),
                placement.vpc_zone_identifier.clone(),
            ),
            None => (
                group.availability_zones.clone(),
                group.vpc_zone_identifier.clone(),
            ),
        };
        let tags = group.tags.clone();
        let protected = group.new_instances_protected_from_scale_in;
        // Whether an instance this launch brings INTO SERVICE is registered with the group's
        // load balancing, which is the group's own `AddToLoadBalancer`
        // ([`StoredGroup::registers_with_load_balancers`]). A launch into the warm pool
        // brings nothing into service — the instance is registered, or not, by the call that
        // takes it out of the pool ([`warm_pools`]) — so it is marked there instead.
        let withhold_registration = request.destination == LaunchDestination::Group
            && !group.registers_with_load_balancers();
        // The attachments an instance this launch brings INTO SERVICE has to be registered
        // in, read off the group before anything is launched: a launch Elastic Load Balancing
        // v2 would refuse the registration of brings nothing into service
        // ([`elbv2_targets::refused_launch_registrations`], below). A launch into the warm pool
        // owes none of them, and neither does a group whose registering process is suspended
        // — there is then no registration for an attachment to refuse.
        let registration_attachments: Vec<String> = if request.destination
            == LaunchDestination::Group
            && group.registers_with_load_balancers()
        {
            elbv2_targets::attached_arns(group)
                .into_iter()
                .map(str::to_string)
                .collect()
        } else {
            Vec::new()
        };
        // Everything the group already holds, its warm pool included: the placement of a
        // fresh instance follows how many the group has placed so far.
        let placed = group.instances.len() + group.warm_pool_instance_count();
        let headroom = usize::try_from(group.launch_headroom()).unwrap_or(0);
        // What the instances run: the desired configuration of the instance refresh
        // that asked for them, when one did — its replacements run the new
        // configuration — and otherwise the group's own launch source.
        let (launch_configuration_name, launch_template, mixed_instances_policy) =
            match request.desired_configuration {
                Some(desired) => (
                    None,
                    desired.launch_template.clone(),
                    desired.mixed_instances_policy.clone(),
                ),
                None => (
                    group.launch_configuration_name.clone(),
                    group.launch_template.clone(),
                    group.mixed_instances_policy.clone(),
                ),
            };
        let launch_configuration = launch_configuration_name
            .as_ref()
            .and_then(|name| self.launch_configurations.get(name))
            .cloned();
        // The type this launch's source states, where it states one: the group's own stored
        // resolution of it ([`StoredGroup::launch_instance_type`]), or — for the replacements
        // of a refresh moving the group to another launch source — that source's own, which
        // is resolved here because the group has never been configured with it.
        let selected_instance_type = match request.desired_configuration {
            Some(_) => instance_requirements::launch_override(mixed_instances_policy.as_ref())
                .and_then(instance_requirements::override_instance_type),
            None => group.launch_instance_type(),
        };
        // And whether that source chooses its type by ATTRIBUTES, which is what makes a
        // source with no type to launch a launch that fails rather than one made from some
        // other type.
        let selects_by_requirements =
            instance_requirements::selects_by_requirements(mixed_instances_policy.as_ref());

        // Never more than the group can still hold, so neither one request nor a run
        // of them can grow the stored instances past MAX_GROUP_CAPACITY.
        let wanted = request.count.min(headroom);
        let launch_request = ec2_instances::LaunchRequest {
            region: &region,
            group_name: request.group_name,
            count: wanted,
            launched_at: request.launched_at,
            placed,
            zones: &zones,
            vpc_zone_identifier: vpc_zone_identifier.as_deref(),
            launch_configuration: launch_configuration.as_ref(),
            launch_template: launch_template.as_ref(),
            mixed_instances_policy: mixed_instances_policy.as_ref(),
            selected_instance_type: selected_instance_type.as_deref(),
            selects_by_requirements,
            tags: &tags,
            service_linked_role_arn: &service_linked_role_arn,
        };
        // A launch is made only from a configuration that CAN produce an instance, asked
        // at the instant of the launch rather than when the group was configured: a group
        // launching from a floating launch-template version launches from whatever that
        // version says now, so a configuration the account's own stores cannot satisfy
        // places nothing and records the reason — in the terminal status code that reason
        // ends an activity in — instead of reporting an instance that could not exist
        // ([`ec2_instances::launch_failure_reason`]).
        if wanted > 0 {
            if let Some(blocked) = ec2_instances::launch_failure_reason(engine, &launch_request) {
                // A reason Amazon EC2 ACCEPTS the launch for and then withdraws the instance
                // for is a launch that WAS made: the attempt is minted where every instance
                // of the account lives and taken back there, under that service's own state
                // reason ([`ec2_instances::withdraw_unusable_key_attempt`]), so a client
                // watching Amazon EC2 sees the instance it would have got terminated with the
                // reason it was terminated for. None of them is an instance of the group:
                // they never reach service, so the group reports none and the capacity stays
                // owed.
                let Some(refusal) = blocked.withdrawn_attempt.clone() else {
                    return GroupLaunchOutcome::unplaced(request.count, Some(blocked));
                };
                // Each of those attempts is an instance Amazon EC2 creates and terminates, so
                // the group makes one when the next one is DUE rather than on every pass over
                // the store ([`StoredGroup::launch_retry_due`]): a launch the clock asks for
                // while the previous attempt is still the current one places nothing and
                // records the same reason, without creating a second instance for it.
                if self
                    .groups
                    .get(request.group_name)
                    .and_then(|group| group.launch_retry_due)
                    .is_some_and(|due| due > request.launched_at)
                {
                    return GroupLaunchOutcome::unplaced(request.count, Some(blocked));
                }
                let withdrawn =
                    ec2_instances::withdraw_unusable_key_attempt(engine, &launch_request, &refusal);
                if let Some(group) = self.groups.get_mut(request.group_name) {
                    group.launch_retry_due = Some(request.launched_at + WITHDRAWN_LAUNCH_RETRY);
                }
                return GroupLaunchOutcome::withdrawn(request.count, blocked, withdrawn);
            }
        }
        let launched = ec2_instances::launch(engine, &launch_request);
        // Registering the instance is PART of bringing it into service, so an attachment
        // Elastic Load Balancing v2 would REFUSE that registration into stops the launch
        // there: the instance Amazon EC2 made is taken back where every instance of the
        // account lives, the group holds none of them — none ever reaches service, so the
        // capacity it was asked for stays owed — and what the caller records for each unit
        // nothing was placed for is that service's own refusal, naming the target group
        // ([`elbv2_targets::refused_launch_message`]).
        //
        // The ATTACHMENT stays: the group keeps reporting what the client attached until the
        // client detaches it, so the cancelled launch is the whole of what happened.
        if !registration_attachments.is_empty() {
            let attempts: Vec<String> = launched
                .iter()
                .map(|instance| instance.instance_id.clone())
                .collect();
            let refused = elbv2_targets::refused_launch_registrations(
                engine,
                &registration_attachments,
                &attempts,
            );
            if let Some(message) = elbv2_targets::refused_launch_message(&refused) {
                ec2_instances::terminate(engine, &attempts, request.launched_at);
                // Each of those attempts is an instance Amazon EC2 creates and this service
                // takes back, so the group makes one when the next one is DUE rather than on
                // every pass over the store ([`StoredGroup::launch_retry_due`]), exactly as a
                // launch Amazon EC2 itself withdraws is retried.
                if let Some(group) = self.groups.get_mut(request.group_name) {
                    group.launch_retry_due = Some(request.launched_at + WITHDRAWN_LAUNCH_RETRY);
                }
                return GroupLaunchOutcome::withdrawn(
                    request.count,
                    BlockedLaunch::cancelled(message),
                    attempts,
                );
            }
        }
        let unplaced = request.count.saturating_sub(launched.len());
        // Amazon EC2 launches the instances of one capacity change one after another, so
        // each of them is a little newer than the last even though they were asked for
        // together: the launch ORDER is minted here, once per instance, from the store's
        // own source, and is what a scale-in's age-ordered policies read
        // ([`termination_policies`]).
        let first_sequence = self.next_instance_launch_seq;
        self.next_instance_launch_seq =
            first_sequence.saturating_add(u64::try_from(launched.len()).unwrap_or(0));
        let launched: Vec<StoredInstance> = launched
            .into_iter()
            .enumerate()
            .map(|(offset, instance)| StoredInstance {
                instance_id: instance.instance_id,
                availability_zone: instance.availability_zone,
                lifecycle_state: request.destination.pending_state().to_string(),
                health_status: HEALTH_STATUS_HEALTHY.to_string(),
                launch_configuration_name: launch_configuration_name.clone(),
                // The launch-template version the instance was ACTUALLY launched from,
                // as Amazon EC2 resolved it for this launch: a group whose specification
                // names `$Latest` or `$Default` launches from whichever version that word
                // named at the instant of the launch, so the instance reports that
                // version number rather than the word the group floats on. A launch
                // source naming no template the account holds resolves to nothing, and
                // the instance reports the specification as the group carries it.
                launch_template: instance
                    .launch_template
                    .clone()
                    .or_else(|| launch_template.clone()),
                // What Amazon EC2 launched it as, so the two services report one
                // instance the same way.
                instance_type: instance.instance_type,
                image_id: instance.image_id,
                protected_from_scale_in: protected,
                launched_at: request.launched_at,
                launch_sequence: first_sequence
                    .saturating_add(u64::try_from(offset).unwrap_or(u64::MAX)),
                // The instance settles at the instant the launch that placed it is due,
                // which the caller asking for the launch states: a launch into the GROUP is
                // due one launch transition out ([`LAUNCH_TRANSITION`]), and one into a warm
                // pool at the span that pool's own preparation takes ([`warm_pools`]).
                transition_due: Some(request.due),
                // A freshly launched instance has failed no health check, so the group's
                // check owes nothing on it.
                health_check_due: None,
                // A launch into the GROUP may be held by the group's own launch lifecycle
                // hooks, which is applied once the instances are placed below
                // ([`lifecycle_actions`]).
                pending_lifecycle_actions: Vec::new(),
                held_step: None,
                load_balancer_registration_withheld: withhold_registration,
                // A LAUNCH takes the instance from Amazon EC2 into wherever this request
                // places it, whether that is the group itself or the group's warm pool: it
                // is Amazon EC2 the instance has just come from either way.
                arrived_from: LifecycleEndpoint::Ec2,
            })
            .collect();
        let launched_ids: Vec<String> = launched
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect();
        // Where the launched instances are PLACED, which decides whether the group holds
        // them at all: a destination the group cannot hold them in (a warm pool the group
        // no longer has) leaves no record pointing at them, so the instances Amazon EC2
        // minted are shut down there rather than left running for nobody.
        let placed = match (request.destination, self.groups.get_mut(request.group_name)) {
            (LaunchDestination::Group, Some(group)) => {
                group.instances.extend(launched);
                true
            }
            (LaunchDestination::WarmPool, Some(group)) => match group.warm_pool.as_mut() {
                Some(pool) => {
                    pool.instances.extend(launched);
                    true
                }
                None => false,
            },
            (_, None) => false,
        };
        if !placed {
            ec2_instances::terminate(engine, &launched_ids, request.launched_at);
            return GroupLaunchOutcome::unplaced(request.count, None);
        }
        // An instance launched INTO THE GROUP is held by every launch lifecycle hook the
        // group configures, which is where the hold a client completes starts
        // ([`lifecycle_actions`]). A warm pool's own instances are not instances of the
        // group, so nothing holds them here.
        if request.destination == LaunchDestination::Group {
            self.hold_lifecycle_transition(
                engine,
                request.group_name,
                HeldTransition::Launching,
                &launched_ids,
                request.launched_at,
            );
        }
        GroupLaunchOutcome {
            launched_ids,
            unplaced,
            // Nothing was withdrawn: this launch placed what Amazon EC2 gave it, and a unit
            // it did not place is one the account had no room for rather than an attempt
            // that was taken back.
            withdrawn_ids: Vec::new(),
            blocked: None,
        }
    }

    /// The instances a scale-in of `surplus_units` capacity units takes out of the group.
    ///
    /// The group's own `TerminationPolicies` select them, applied in the order the
    /// group lists them ([`termination_policies`]), against the account's launch
    /// configurations — which one of those policies measures an instance's
    /// configuration age against — at the instant `now` the scale-in happens. An
    /// instance protected from scale-in is never chosen: that is what the protection
    /// means.
    fn scale_in_selection(
        &self,
        group_name: &str,
        surplus_units: i32,
        now: SystemTime,
    ) -> Vec<String> {
        let Some(group) = self.groups.get(group_name) else {
            return Vec::new();
        };
        termination_policies::scale_in_selection(
            group,
            &self.launch_configurations,
            surplus_units,
            now,
        )
    }

    /// Take the instances `instance_ids` names out of the group: each is reported
    /// `Terminating` until `due`, and the same Amazon EC2 instance is terminated
    /// where it lives, so the account sees it shut down rather than vanish from one
    /// service's view alone.
    ///
    /// A group holding a lifecycle hook for the terminating transition HOLDS each of them
    /// instead ([`AutoScalingState::hold_lifecycle_transition`]): such an instance waits in
    /// that transition's wait state, and is reported `Terminating` — and shut down in Amazon
    /// EC2 — only once the hold resolves, because a hook that cannot stop a termination can
    /// still delay it.
    ///
    /// An id the group does not hold, or one already on its way out, changes nothing.
    fn terminate_group_instances<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        instance_ids: &[String],
        now: SystemTime,
        due: SystemTime,
    ) {
        let held = self.hold_lifecycle_transition(
            engine,
            group_name,
            HeldTransition::Terminating,
            instance_ids,
            now,
        );
        // What this call actually takes out of service: a named instance the group still
        // holds, that no termination hook is holding and that is not already on its way out.
        // An instance a LAUNCH hook was holding is among them — the hold above discarded that
        // hold, because the launch it was waiting for is over.
        let mut leaving: Vec<String> = Vec::new();
        if let Some(group) = self.groups.get_mut(group_name) {
            for instance in &mut group.instances {
                if instance_ids.contains(&instance.instance_id)
                    && !held.contains(&instance.instance_id)
                    && (!instance.is_leaving_group() || instance.is_retained())
                {
                    instance.lifecycle_state = LIFECYCLE_TERMINATING.to_string();
                    instance.transition_due = Some(due);
                    leaving.push(instance.instance_id.clone());
                }
            }
        }
        ec2_instances::terminate(engine, &leaving, now);
    }

    /// Materialise every instance-refresh replacement the clock has made due at
    /// `now`, and finish each refresh that has none left.
    ///
    /// A refresh makes one MOVE per [`INSTANCE_REFRESH_REPLACEMENT`] ([`refresh_plan`]), so
    /// a refresh nobody has read for a while catches up over the moves it was owed,
    /// each anchored at the instant it came due rather than at the instant somebody
    /// looked. The loop therefore terminates: every pass either retires a move of the
    /// replacement in flight, starts the next one, moves a refresh that has replaced them
    /// all into the wait it owes at
    /// the end — which comes due strictly later — or brings a refresh to a terminal
    /// status, which clears its due instant.
    pub fn advance_instance_refreshes<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        loop {
            let due: Vec<(usize, SystemTime)> = self
                .instance_refreshes
                .iter()
                .enumerate()
                .filter_map(|(index, refresh)| {
                    refresh
                        .next_step_due
                        .filter(|due| *due <= now)
                        .map(|due| (index, due))
                })
                .collect();
            if due.is_empty() {
                return;
            }
            for (index, due) in due {
                self.advance_one_instance_refresh(engine, index, due);
            }
        }
    }

    /// Retire the one move the refresh at `index` owes at `due`: complete its
    /// cancellation, make the next move of the replacement it is part way through
    /// ([`refresh_plan`]) — or of the replacement a rollback is putting back — or, with
    /// none left and nothing left to wait, finish it.
    fn advance_one_instance_refresh<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        index: usize,
        due: SystemTime,
    ) {
        let Some(refresh) = self.instance_refreshes.get(index) else {
            return;
        };
        let group_name = refresh.auto_scaling_group_name.clone();
        let desired_configuration = refresh.desired_configuration.clone();
        let cancelling = refresh.status == REFRESH_CANCELLING;
        let rolling_back = refresh.is_rolling_back();
        let baking = refresh.is_baking();
        let bake_time = refresh.bake_time();

        // A cancellation replaces nothing further and rolls nothing back, so the only
        // step a refresh being cancelled owes is the end of that cancellation — which
        // leaves the progress its replacements made, and the instances it never got to,
        // exactly as they stood.
        if cancelling {
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = REFRESH_CANCELLED.to_string();
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        }

        // A refresh whose group is gone has no instances left to replace and cannot
        // finish: the group it was started for no longer exists.
        if !self.groups.contains_key(&group_name) {
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = if rolling_back {
                    REFRESH_ROLLBACK_FAILED.to_string()
                } else {
                    REFRESH_FAILED.to_string()
                };
                refresh.status_reason = Some(
                    "The Auto Scaling group was deleted while the instance refresh was in \
                     progress."
                        .to_string(),
                );
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        }

        // The move the replacement it is part way through owes, or the one that starts the
        // next instance's: a refresh makes each replacement in two moves, with the window it
        // runs under deciding their order ([`refresh_plan`]). `false` says it has none left,
        // which is what the rest of this call brings to a terminal status.
        if self.advance_refresh_replacement(engine, index, due) {
            return;
        }

        if rolling_back {
            // Every replacement the refresh had made has been replaced back, so the
            // rollback has restored the group to the configuration it was using
            // before the refresh started. "PercentageComplete does not include
            // instances that are replaced during a rollback. This value gradually
            // goes back down to zero during a rollback"
            // (smithy_query(query="doc",shape="InstanceRefresh",
            // member="PercentageComplete")), so it has reached zero here.
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = REFRESH_ROLLBACK_SUCCESSFUL.to_string();
                refresh.percentage_complete = 0;
                refresh.end_time = Some(due);
                refresh.next_step_due = None;
            }
            return;
        }
        // "Baking - Waiting the specified bake time after an instance refresh has
        // finished updating instances" (smithy_query(query="doc",
        // shape="InstanceRefresh",member="Status")), so a refresh that has replaced
        // every instance it set out to is NOT complete while the wait it runs under is
        // still owed ([`StoredInstanceRefresh::bake_time`]): it reports every instance
        // updated and bakes, and the end of that wait is what completes it. A refresh
        // asked to wait nothing has nothing to wait and finishes here.
        if !baking && !bake_time.is_zero() {
            if let Some(refresh) = self.instance_refreshes.get_mut(index) {
                refresh.status = REFRESH_BAKING.to_string();
                refresh.percentage_complete = 100;
                refresh.instances_to_update = 0;
                refresh.next_step_due = Some(due + bake_time);
            }
            return;
        }
        // Every instance the refresh set out to replace has been replaced, so the
        // refresh has succeeded — and the model documents that the group's own
        // settings then reflect the desired configuration it ran with.
        if let Some(desired) = desired_configuration {
            if let Some(group) = self.groups.get_mut(&group_name) {
                if desired.launch_template.is_some() {
                    group.launch_template = desired.launch_template.clone();
                    group.launch_configuration_name = None;
                }
                if desired.mixed_instances_policy.is_some() {
                    // The group spreads its capacity under the distribution the
                    // refresh's policy states, each member it left out at the default
                    // the group's own writers fill in ([`group_defaults`]).
                    group.mixed_instances_policy = group_defaults::mixed_instances_policy(
                        desired.mixed_instances_policy.clone(),
                    );
                    group.launch_configuration_name = None;
                }
            }
        }
        if let Some(refresh) = self.instance_refreshes.get_mut(index) {
            refresh.status = REFRESH_SUCCESSFUL.to_string();
            refresh.percentage_complete = 100;
            refresh.instances_to_update = 0;
            refresh.end_time = Some(due);
            refresh.next_step_due = None;
        }
    }

    /// The cause the replacements of an instance refresh record: when the replacement was
    /// made and which refresh asked for it.
    pub fn instance_refresh_cause(at: SystemTime, instance_refresh_id: &str) -> String {
        Self::reason_cause(
            at,
            &format!(
                "an instance refresh is replacing the instances of the group: instance refresh \
                 {instance_refresh_id}"
            ),
        )
    }

    /// The cause the replacements of an instance refresh ROLLBACK record: which
    /// refresh's rollback asked for them, so the activity log tells the replacements a
    /// refresh made apart from the ones that put them back.
    pub fn instance_refresh_rollback_cause(at: SystemTime, instance_refresh_id: &str) -> String {
        Self::reason_cause(
            at,
            &format!(
                "an instance refresh rollback is replacing the instances of the group: instance \
                 refresh {instance_refresh_id}"
            ),
        )
    }
}

/// What a group is asked to launch: how many instances, when their transient state
/// comes due, where they are placed, and the configuration they run when it is not the
/// group's own.
#[derive(Debug)]
struct GroupLaunch<'a> {
    /// The group the instances belong to.
    group_name: &'a str,
    /// How many instances to launch.
    count: usize,
    /// The instant the launch happens, which is how old each instance it launches is.
    launched_at: SystemTime,
    /// When the transient state each launched instance is reported in comes due.
    due: SystemTime,
    /// The desired configuration of the instance refresh that asked for the launch,
    /// when one did: its replacements run the new configuration before the group's
    /// own settings are updated to it.
    desired_configuration: Option<&'a DesiredConfiguration>,
    /// Which of the group's two instance collections the launch places into.
    destination: LaunchDestination,
    /// Where the launch places its instances, when the CALLER named a placement narrower
    /// than the group's own configuration (`LaunchInstances`): a subset of the group's
    /// zones and subnets, resolved and checked against the group before it reaches here
    /// ([`requested_launches`]). `None` places from the group's own configuration, which
    /// is what every launch the group makes for itself does.
    placement: Option<&'a RequestedPlacement>,
}

/// What one launch into a group produced: the instances it placed, how many units of
/// the capacity it was asked for nothing was placed for, and why — when the reason is
/// the group's own configuration rather than the account's capacity.
#[derive(Debug)]
struct GroupLaunchOutcome {
    /// The ids of the instances that were launched, in launch order.
    launched_ids: Vec<String>,
    /// How many of the instances asked for were not placed.
    unplaced: usize,
    /// The ids of the attempts Amazon EC2 ACCEPTED and took back, in launch order: a launch
    /// blocked for a reason Amazon EC2 withdraws the instance for leaves one attempt per unit
    /// of capacity it could not place ([`BlockedLaunch::cancelled_attempt`]). Empty for every
    /// launch no attempt was made for.
    withdrawn_ids: Vec<String>,
    /// The reason each unplaced unit's terminal activity records, and the code it ends in,
    /// when something about the group stopped the launch outright.
    blocked: Option<BlockedLaunch>,
}

impl GroupLaunchOutcome {
    /// A launch that placed nothing, for `blocked`'s reason when it carries one.
    fn unplaced(count: usize, blocked: Option<BlockedLaunch>) -> Self {
        Self {
            launched_ids: Vec::new(),
            unplaced: count,
            withdrawn_ids: Vec::new(),
            blocked,
        }
    }

    /// A launch that placed nothing because Amazon EC2 took back every attempt it made:
    /// `withdrawn_ids` are those attempts, which the activity of each unplaced unit names.
    fn withdrawn(count: usize, blocked: BlockedLaunch, withdrawn_ids: Vec<String>) -> Self {
        Self {
            launched_ids: Vec::new(),
            unplaced: count,
            withdrawn_ids,
            blocked: Some(blocked),
        }
    }

    /// What the activity of each unplaced unit records: the reason
    /// the launch itself carries, or `account` — what an account that could not place the
    /// instance records — when it carries none.
    fn unplaced_message(&self, account: &str) -> String {
        self.blocked
            .as_ref()
            .map_or_else(|| account.to_string(), |blocked| blocked.status_message.clone())
    }

    /// The terminal status code those activities end in: the one the launch's own reason
    /// carries, and [`ACTIVITY_FAILED`] for a unit the account could not place — which is
    /// an attempt that failed.
    fn unplaced_status_code(&self) -> &'static str {
        self.blocked
            .as_ref()
            .map_or(ACTIVITY_FAILED, |blocked| blocked.status_code)
    }

    /// What the activity of each unplaced unit DESCRIBES and the ATTEMPT it was made for, one
    /// per unit: the instance of the attempt where Amazon EC2 made one and took it back — the
    /// launch was made, so the activity names the instance it launched, exactly as a launch
    /// that carried through does ([`InstanceLifecycle::description`]) — and the reason the
    /// launch was not made where no attempt was made at all
    /// ([`failed_launch_description`]), which names no instance.
    fn unplaced_steps(&self, message: &str) -> Vec<(Option<String>, String)> {
        (0..self.unplaced)
            .map(|index| match self.withdrawn_ids.get(index) {
                Some(instance_id) => (
                    Some(instance_id.clone()),
                    InstanceLifecycle::Launched.description(instance_id),
                ),
                None => (None, failed_launch_description(message)),
            })
            .collect()
    }
}

/// Where a launch places what it launched: the group's own capacity, or the warm pool
/// that sits beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchDestination {
    /// The group's own instances, reported `Pending` until their span is due.
    Group,
    /// The group's warm pool, reported `Warmed:Pending` until it is
    /// ([`warm_pools`]).
    WarmPool,
}

impl LaunchDestination {
    /// The `LifecycleState` an instance launched into this collection is reported in
    /// until its transition comes due.
    fn pending_state(self) -> &'static str {
        match self {
            Self::Group => LIFECYCLE_PENDING,
            Self::WarmPool => WARMED_PENDING,
        }
    }
}

/// The values a `HealthCheckType` may name, from the model's own documentation
/// for `CreateAutoScalingGroup$HealthCheckType`: "A comma-separated value string
/// of one or more health check types. The valid values are EC2, EBS, ELB, and
/// VPC_LATTICE."
///
/// Held in the order the service's own refusal lists them
/// ([`invalid_health_check_type_message`]), the set being what decides a value and the
/// order mattering only to that sentence.
pub const HEALTH_CHECK_TYPES: &[&str] = &["EC2", "ELB", "EBS", "VPC_LATTICE"];

/// The message the service refuses a `HealthCheckType` outside [`HEALTH_CHECK_TYPES`] with,
/// shared by every operation that sets one.
///
/// The member is a free string the service validates itself
/// (`smithy_query(query="shape",shape="XmlStringMaxLen32")` carries no value list), and the
/// refusal is the one machine-readable statement of the set it accepts: the FIXED sentence
/// `Valid health check types are: [` … `].`, naming the valid types and nothing from the
/// request.
pub fn invalid_health_check_type_message() -> String {
    format!(
        "Valid health check types are: [{}].",
        HEALTH_CHECK_TYPES.join(", ")
    )
}

/// Whether `health_check_type` is a comma-separated list of [`HEALTH_CHECK_TYPES`].
pub fn is_valid_health_check_type(health_check_type: &str) -> bool {
    !health_check_type.is_empty()
        && health_check_type
            .split(',')
            .all(|part| HEALTH_CHECK_TYPES.contains(&part.trim()))
}

/// The policies a `TerminationPolicies` entry may name, from the model's own
/// documentation for `CreateAutoScalingGroup$TerminationPolicies`: "Valid values:
/// Default | AllocationStrategy | ClosestToNextInstanceHour | NewestInstance |
/// OldestInstance | OldestLaunchConfiguration | OldestLaunchTemplate |
/// arn:aws:lambda:region:account-id:function:my-function:my-alias".
pub const TERMINATION_POLICIES: &[&str] = &[
    termination_policies::DEFAULT,
    termination_policies::ALLOCATION_STRATEGY,
    termination_policies::CLOSEST_TO_NEXT_INSTANCE_HOUR,
    termination_policies::NEWEST_INSTANCE,
    termination_policies::OLDEST_INSTANCE,
    termination_policies::OLDEST_LAUNCH_CONFIGURATION,
    termination_policies::OLDEST_LAUNCH_TEMPLATE,
];

/// The prefix of the custom-policy form that documentation ends with: a Lambda
/// function ARN.
pub const TERMINATION_POLICY_LAMBDA_PREFIX: &str = "arn:aws:lambda:";

/// Whether `policy` is one of [`TERMINATION_POLICIES`] or a Lambda function ARN.
pub fn is_valid_termination_policy(policy: &str) -> bool {
    TERMINATION_POLICIES.contains(&policy) || policy.starts_with(TERMINATION_POLICY_LAMBDA_PREFIX)
}

/// `capacity` brought inside the bounds `min_size`..`max_size` a group holds — a group
/// never holds a capacity outside its own bounds, and this is the ONE place that is
/// applied.
///
/// The ceiling is taken as the HIGHER of the pair rather than assumed to be `max_size`,
/// so no caller has to establish the order of the two first: `Ord::clamp` panics when its
/// low bound is above its high one, and the pair is stored state read long after the
/// request that wrote it. Which pairs a group may be WRITTEN with is a rule of the
/// operations that write one, stated where the group is stored, and never a precondition
/// a later read has to re-derive.
pub fn capacity_within_bounds(capacity: i32, min_size: i32, max_size: i32) -> i32 {
    capacity.clamp(min_size, min_size.max(max_size))
}

/// The message the service refuses a `TerminationPolicies` naming policies it does not
/// accept with, shared by every operation that sets a group's policies.
///
/// The member is a free string the service validates itself — the documentation lists the
/// values in prose and the list's own target states no enum
/// (`smithy_query(query="doc",shape="UpdateAutoScalingGroupType",member="TerminationPolicies")`)
/// — so the refusal is the one machine-readable statement of the set it accepts: the
/// rejected value(s) of the request, bracketed, then the FIXED sentence naming the policy
/// TYPES ([`TERMINATION_POLICY_TYPES`]) in the order that catalog reports them, which is
/// where the custom form is named by its type rather than by any one function's ARN.
///
/// Not public: [`termination_policies_refusal`] is the ONE entry point to this refusal, so
/// no handler can assemble the sentence a second, differing way.
fn invalid_termination_policy_message(rejected: &[&str]) -> String {
    format!(
        "[{}] is not a valid termination policy. Valid termination policies are: [{}].",
        rejected.join(", "),
        TERMINATION_POLICY_TYPES.join(", ")
    )
}

/// The refusal a request that would set `requested` as a group's termination policies earns
/// ([`invalid_termination_policy_message`]), or `None` when every entry names a policy this
/// service accepts ([`is_valid_termination_policy`]).
///
/// The request is refused as a WHOLE — the group keeps the policies it had — so every
/// rejected entry is named, in request order, rather than only the first one found. That
/// reading of the bracketed part is a generalisation of a refusal carrying ONE rejected
/// value, which is the only shape the sentence is attested in: a request whose list carries
/// several unaccepted values is one this service has nothing closer to go on for.
pub fn termination_policies_refusal<'a, I>(requested: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let rejected: Vec<&str> = requested
        .into_iter()
        .filter(|policy| !is_valid_termination_policy(policy))
        .collect();
    (!rejected.is_empty()).then(|| invalid_termination_policy_message(&rejected))
}

/// Whether `name` satisfies the character rule the model documents for
/// `AutoScalingGroupName`: "The name can contain any ASCII character 33 to 126
/// including most punctuation characters, digits, and upper and lowercased
/// letters. You cannot use a colon (:) in the name."
pub fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| matches!(c, '!'..='~') && c != ':')
}

/// The smallest non-zero `MaxInstanceLifetime` the service accepts.
///
/// From the model's own documentation for
/// `CreateAutoScalingGroup$MaxInstanceLifetime`: "If specified, the value must be
/// either 0 or a number equal to or greater than 86,400 seconds (1 day)."
pub const MIN_MAX_INSTANCE_LIFETIME: i32 = 86_400;

/// Whether `seconds` satisfies that rule.
pub fn is_valid_max_instance_lifetime(seconds: i32) -> bool {
    seconds == 0 || seconds >= MIN_MAX_INSTANCE_LIFETIME
}

/// The message the service refuses a request whose maximum group size is below its
/// minimum with, shared by every operation that sets a group's bounds: the two bounds
/// in the order it reports them, and nothing else from the request.
pub fn max_below_min_message(max_size: i32, min_size: i32) -> String {
    format!("Max bound, {max_size}, must be greater than or equal to min bound, {min_size}")
}

/// The message the service refuses a request whose minimum group size is below zero with,
/// shared by every operation that sets a group's bounds.
///
/// A bound is a COUNT of instances, so the service states the bound it was handed and the
/// requirement that bound has to meet, in the same shape as the relational refusal above
/// ([`max_below_min_message`]): the fixed text `Min bound, ` and `, must be non-negative`
/// around the one value the request carried, and nothing else from the request.
pub fn negative_min_size_message(min_size: i32) -> String {
    format!("Min bound, {min_size}, must be non-negative")
}

/// The message the service refuses a request whose desired capacity falls outside the
/// bounds that request leaves the group with, shared by every operation that sets those
/// three sizes together.
///
/// "Usually, you set these sizes based on a specific number of instances"
/// (`CreateAutoScalingGroup`): the three sizes describe one group, so the service states the
/// capacity it was given and the two bounds it has to sit between, and nothing else from the
/// request. The sentence is FIXED — `Desired capacity:`, ` must be between the specified min
/// size:` and ` and max size:` — with the three values the only variable parts.
///
/// An operation that sets the capacity ALONE words its own refusal around its own name
/// (`SetDesiredCapacity`), the bound it missed being a bound of the stored group rather than
/// one the same request carried.
pub fn desired_capacity_out_of_bounds_message(
    desired_capacity: i32,
    min_size: i32,
    max_size: i32,
) -> String {
    format!(
        "Desired capacity:{desired_capacity} must be between the specified min \
         size:{min_size} and max size:{max_size}"
    )
}

/// The first of `sizes` past [`MAX_GROUP_CAPACITY`], if any.
///
/// What a caller passes here is the capacity a request would have a group RUN — the
/// instances it is asked to hold — and never the bounds it may later grow within: the
/// ceiling is a ceiling on STORED INSTANCES, so a group whose `MaxSize` is a large
/// headroom value holds no instance for that headroom and is created and updated like any
/// other, while the capacity it is actually driven to is held to this ceiling. Every
/// operation that sets a group's sizes checks that capacity through this, so no request
/// can leave a group running more instances than this process holds one to, and the
/// refusal can name the offending value.
pub fn size_past_ceiling(sizes: &[i32]) -> Option<i32> {
    sizes.iter().copied().find(|size| *size > MAX_GROUP_CAPACITY)
}

/// The first of `counts` past [`MAX_TARGET_GROUPS_PER_GROUP`], if any.
///
/// Every operation that adds to a group's load-balancer attachments checks the count it
/// would leave the group holding through this, so neither one request nor a run of them can
/// store more attachments than this process holds a group to, and the refusal can name the
/// count that missed the ceiling.
pub fn attachment_count_past_ceiling(counts: &[usize]) -> Option<usize> {
    counts
        .iter()
        .copied()
        .find(|count| *count > MAX_TARGET_GROUPS_PER_GROUP)
}

/// The message a request that would leave one group holding more load-balancer attachments
/// than [`MAX_TARGET_GROUPS_PER_GROUP`] is refused with, in the same shape this service
/// states its other per-collection ceilings in: the fixed sentence and the ceiling.
pub fn too_many_attached_target_groups_message() -> String {
    format!(
        "You have already reached the limit of {MAX_TARGET_GROUPS_PER_GROUP} target groups per \
         Auto Scaling group"
    )
}

/// The message a request that would leave one group holding more Classic Load Balancer
/// attachments than [`MAX_TARGET_GROUPS_PER_GROUP`] is refused with, in the same shape this
/// service states its other per-collection ceilings in: the fixed sentence and the ceiling.
pub fn too_many_attached_load_balancers_message() -> String {
    format!(
        "You have already reached the limit of {MAX_TARGET_GROUPS_PER_GROUP} load balancers per \
         Auto Scaling group"
    )
}

/// Bring every load-balancing registration the group `group_name` owes in line with what it
/// now holds: its targets in the Elastic Load Balancing v2 target groups it is attached to
/// ([`elbv2_targets`]), and its registrations with the Classic Load Balancers it is attached
/// to ([`elb_classic_instances`]).
///
/// THE ONE call every write that moved a group's instances or its attachments makes for it,
/// so neither kind of attachment is settled by some operations and not others. Called with
/// NO guard of this store held: each reconcile takes this store's lock to read what the group
/// owes, releases it, and only then writes the other service's store.
pub fn settle_group_load_balancers<Eng: Send + Sync + 'static>(engine: &Eng, group_name: &str) {
    elbv2_targets::settle_group(engine, group_name);
    elb_classic_instances::settle_group(engine, group_name);
}

/// Settle every group this store holds, ONE GROUP AT A TIME: what the clock pump owes, whose
/// wake-up can have moved an instance of any group.
///
/// The group names are read under this store's lock and the guard released before the first
/// reconcile, so the registrations of one group is the most this pass ever computes at once,
/// however many groups the store holds (itself bounded by [`MAX_GROUPS`]).
pub fn settle_every_group_load_balancers<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let names: Vec<String> = state.lock().groups.keys().cloned().collect();
    for name in names {
        settle_group_load_balancers(engine, &name);
    }
}

/// Whether the account holds an Auto Scaling group of `name`, resolved under a lock this
/// call takes and RELEASES.
///
/// THE ONE way an operation resolves the group it changes BEFORE it reads the resources its
/// other members name: those resources belong to another service, and reaching that
/// service's store with this one's guard held would have one request holding two stores. So
/// the attach family asks this first, refuses a name no group answers to in its own
/// absent-group sentence, and only then resolves what it was asked to attach — a group
/// deleted between the two is caught by the write's own lookup, which is the same race any
/// two calls of one group run.
pub fn holds_group<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    now: ::std::time::SystemTime,
) -> bool {
    let Some(store) = service_state(engine) else {
        return false;
    };
    let mut state = store.lock();
    state.materialize_due(engine, now);
    state.groups.contains_key(name)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAutoscalingAutoscaling20110101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAutoscalingAutoscaling20110101State>)
}

/// The region a request is signed for, which is the region the resources it
/// creates live in. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope (an unsigned call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's resources live in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured audit record a state-changing operation owes: the
/// operation, the resource it changed, how the call ended, and which credential
/// was presented for it.
///
/// Only identifiers are recorded. The presented access key id names the caller;
/// the signature, the secret and the request body never appear here, and neither
/// does any other header. The subscriber stamps the time.
pub fn audit(parts: &::http::request::Parts, operation: &str, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "Auto Scaling state change"
    );
}

/// The most resource identifiers ONE audit record names.
///
/// A record is written per call, and a call that changes several resources is still one
/// record: the identifiers are capped so the record's size is the emulator's choice rather
/// than the caller's.
pub const MAX_AUDITED_RESOURCES: usize = 10;

/// The `resource` field of an audit record for a call that changed SEVERAL resources: how
/// many it changed, and at most [`MAX_AUDITED_RESOURCES`] of their identifiers.
///
/// THE ONE COPY for every operation of this service that changes a set of resources at
/// once, so no record a caller sized is written.
pub fn audited_resources(resources: &[String]) -> String {
    let named = resources
        .iter()
        .take(MAX_AUDITED_RESOURCES)
        .cloned()
        .collect::<Vec<String>>()
        .join(",");
    if resources.len() > MAX_AUDITED_RESOURCES {
        let withheld = resources.len() - MAX_AUDITED_RESOURCES;
        format!(
            "{} resources: {named} (+{withheld} more)",
            resources.len()
        )
    } else {
        named
    }
}

/// Convert a stored instant to the wire timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Convert a wire timestamp a request states to a stored instant: the inverse of
/// [`to_date_time`].
///
/// An instant at or before the epoch — which no clock reports and no request this service
/// answers states — reads as the epoch itself rather than failing the call.
#[must_use]
pub fn from_date_time(time: ::aws_smithy_types::DateTime) -> SystemTime {
    let since_epoch = time.as_secs_f64();
    if !since_epoch.is_finite() || since_epoch <= 0.0 {
        return SystemTime::UNIX_EPOCH;
    }
    Duration::try_from_secs_f64(since_epoch)
        .ok()
        .and_then(|since_epoch| SystemTime::UNIX_EPOCH.checked_add(since_epoch))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// `at` as RFC 3339 UTC text truncated to the second, which is the precision this service
/// states an instant to inside a sentence a client reads — the reason a process reports
/// for being suspended ([`suspended_processes::suspension_reason`]) and the instant a
/// scaling activity's cause names.
pub fn whole_seconds_text(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(seconds).unwrap_or(i64::MAX))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// `at` as RFC 3339 UTC text at MILLISECOND precision, which is the precision Amazon EC2
/// Auto Scaling states an instant to inside a document it PUBLISHES: the messages its own
/// documentation shows a notification topic receiving carry a thousandths fraction and
/// nothing finer (`"Time": "2016-09-30T19:00:36.414Z"`, `"StartTime":
/// "2016-09-30T19:00:11.44Z"`, with a trailing zero of that fraction left off).
///
/// THE ONE rendering of an instant inside a document this service renders itself — the
/// Amazon SNS notifications of [`sns_notifications`] and the EventBridge event details of
/// [`lifecycle_events`] — so the two reports of one lifecycle step state its instants
/// identically. The instant is read in whole milliseconds rather than through the
/// floating-point conversion the wire SHAPES take ([`to_date_time`]), whose remainder prints
/// as a sub-millisecond fraction that no document of this service's own carries and that a
/// client reading the document with a date-time parser bounded at microseconds — the bound
/// of the one in every Python standard library — cannot read at all.
pub fn millisecond_text(at: SystemTime) -> String {
    let millis = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The ARN of an Auto Scaling group, in the form the service reports:
/// `arn:aws:autoscaling:{region}:{account}:autoScalingGroup:{id}:autoScalingGroupName/{name}`.
pub fn group_arn(region: &str, resource_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        "autoscaling",
        region,
        &format!("autoScalingGroup:{resource_id}:autoScalingGroupName/{name}"),
    )
}

/// The fixed text every refusal of a request naming a group this service does not hold
/// starts with. What follows it is the DETAIL the operation reports, which is that
/// operation's own.
const ABSENT_GROUP_PREFIX: &str = "AutoScalingGroup name not found - ";

/// The absent-group refusal whose detail names the group as the resource and the name
/// it was asked for, unquoted.
///
/// The name is the only part taken from the request; the rest of the sentence,
/// punctuation included, is fixed text.
pub fn absent_group_message(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}AutoScalingGroup {name} not found")
}

/// The fixed text every refusal of a request naming a launch configuration the account
/// holds none under starts with. What follows it is the DETAIL the operation reports,
/// which is that operation's own — exactly as the absent-group prefix
/// ([`ABSENT_GROUP_PREFIX`]) is followed by each operation's own detail.
const ABSENT_LAUNCH_CONFIGURATION_PREFIX: &str = "Launch configuration name not found - ";

/// The refusal a request that REFERENCES a launch configuration — one that launches a
/// group from it rather than acting on the configuration itself — is answered with when
/// the account holds none under that name: the fixed prefix above, then the detail stating
/// that no configuration of that name exists.
///
/// The name is the only part taken from the request; the rest of the sentence, the colon
/// after `name` included, is fixed text.
pub fn absent_launch_configuration_message(name: &str) -> String {
    format!(
        "{ABSENT_LAUNCH_CONFIGURATION_PREFIX}A launch configuration with the name: {name} \
         does not exist"
    )
}

/// The refusal an operation whose own SUBJECT is the launch configuration states when the
/// account holds none under the name it was asked for: the fixed prefix above, then the
/// detail naming the resource and the name, unquoted, followed by `not found` — the form
/// the group-scoped delete states for its own resource too
/// ([`absent_group_message`]).
///
/// The name is the only part taken from the request; the rest of the sentence is fixed
/// text.
pub fn launch_configuration_not_found_message(name: &str) -> String {
    format!("{ABSENT_LAUNCH_CONFIGURATION_PREFIX}Launch configuration {name} not found")
}

/// The message Amazon EC2 Auto Scaling refuses a legacy target-group detach that
/// names a target group the group does not hold with, which names those target
/// groups.
///
/// The refusal is for the request as a WHOLE: a call that mixes attached target
/// groups with one that is not attached detaches none of them, so the message says
/// what was wrong with the request rather than reporting a partial result.
pub fn unattached_target_groups_message(arns: &[String]) -> String {
    format!(
        "Trying to remove Target Groups that are not part of the group: {}",
        arns.join(", ")
    )
}

/// The message a call naming more target groups than
/// [`MAX_TARGET_GROUPS_PER_CALL`] is refused with, which quotes the bound the
/// caller has to come back under.
pub fn too_many_target_groups_message() -> String {
    format!(
        "Trying to update too many Target Groups at once. The limit is \
         {MAX_TARGET_GROUPS_PER_CALL}"
    )
}

/// The message a call naming more Classic Load Balancers than
/// [`MAX_LOAD_BALANCERS_PER_CALL`] is refused with, which quotes the bound the caller has to
/// come back under.
///
/// The same sentence this service refuses too many target groups at once with
/// ([`too_many_target_groups_message`]), over the resource THIS call updates: fixed text
/// apart from that bound, which is the documented per-call limit rather than a value from the
/// request.
pub fn too_many_load_balancers_message() -> String {
    format!(
        "Trying to update too many Load Balancers at once. The limit is \
         {MAX_LOAD_BALANCERS_PER_CALL}"
    )
}

/// The message Amazon EC2 Auto Scaling refuses a legacy load-balancer detach that names a
/// load balancer the group does not hold with, which names those load balancers.
///
/// The refusal is for the request as a WHOLE: a call that mixes attached load balancers with
/// one that is not attached detaches none of them, so the message says what was wrong with
/// the request rather than reporting a partial result.
pub fn unattached_load_balancers_message(names: &[String]) -> String {
    format!(
        "Trying to remove Load Balancers that are not part of the group: {}",
        names.join(", ")
    )
}

/// The most instance IDs ONE `SetInstanceProtection` call may name.
///
/// From the model's own documentation for `SetInstanceProtection`: "If you exceed
/// your maximum limit of instance IDs, which is 50 per Auto Scaling group, the call
/// fails." The member carries no modelled length, so the bound is the service's own
/// and is checked by the handler.
///
/// How many ids ONE request may carry is a fact of the REQUEST's shape, so the refusal
/// past this bound is the service-common `ValidationError` — the same class the other
/// per-call bound of this service answers with ([`MAX_TARGET_GROUPS_PER_CALL`]). The
/// modelled `LimitExceededFault` states something else entirely ("You have already
/// reached a limit for your Amazon EC2 Auto Scaling resources",
/// `smithy_query(query="shape",shape="LimitExceededFault")`), so it stays reserved for
/// the account's own resource limits.
pub const MAX_INSTANCE_IDS_PER_CALL: usize = 50;

/// The message a call naming more instance IDs than [`MAX_INSTANCE_IDS_PER_CALL`] is
/// refused with, which quotes the bound the caller has to come back under.
///
/// Fixed text apart from that bound, which is the documented per-call limit rather than
/// a value from the request: the sentence says how many instances one call may protect
/// at a time, so the caller splits its ids into calls of that size.
pub fn too_many_instance_ids_message() -> String {
    format!(
        "Exceeded maximum allowed instances to protect simultaneously; can provide at most \
         {MAX_INSTANCE_IDS_PER_CALL} instances."
    )
}

/// The clause both of this service's "that instance is not part of that group" refusals
/// are built around: an instance-scoped request is resolved against the group the
/// request NAMES, so the refusal names the instance AND that group, and one sentence
/// speaks of one instance id.
///
/// THE ONE COPY of that fixed text; each operation supplies the subject its own refusal
/// opens with ([`instance_not_in_group_message`],
/// [`the_instance_not_in_group_message`]) and the two variable parts, which both
/// come from the request.
const NOT_PART_OF_GROUP_CLAUSE: &str = " is not part of Auto Scaling group ";

/// The message an instance-scoped request that names an instance the group does not hold
/// is refused with, in the subject form that OPENS WITH THE ARTICLE.
///
/// One sentence for the offending instance id, terminated by a period: the fixed text is
/// `The instance ` / [`NOT_PART_OF_GROUP_CLAUSE`] / `.`, and the two variable parts — the
/// instance id and the group name — both come from the request.
///
/// Same class as [`instance_not_in_group_message`], which a TERMINATION answers in its
/// own operation's wording; the refusal is for the request as a WHOLE either way, so a
/// call that mixes instances of the group with one that is not part of it changes none
/// of them. The operations that answer it are the ones that MOVE an instance of a group
/// without terminating it: a protection update and a standby move.
pub fn the_instance_not_in_group_message(instance_id: &str, group_name: &str) -> String {
    format!("The instance {instance_id}{NOT_PART_OF_GROUP_CLAUSE}{group_name}.")
}

/// The message a TERMINATION request that names an instance which is not part of the
/// Auto Scaling group it is terminated from is refused with.
///
/// One sentence per offending instance, as the refusal class is worded: the fixed text
/// is `Instance ` / ` is not part of Auto Scaling group ` / `.`, and the two variable
/// parts are the instance id and the group name, both from the request — so this is the
/// answer only where the request NAMED a group to resolve the instance against. A request
/// that names no group identifies no group the instance could be part of, and is refused
/// with [`instance_managed_by_no_group_message`] instead: a refusal never reports a group
/// the request did not supply.
///
/// Same class as [`the_instance_not_in_group_message`], in the subject this
/// operation's own refusal opens with.
pub fn instance_not_in_group_message(instance_id: &str, group_name: &str) -> String {
    format!("Instance {instance_id}{NOT_PART_OF_GROUP_CLAUSE}{group_name}.")
}

/// The fixed text every refusal of a request naming an instance Amazon EC2 Auto Scaling
/// does not manage starts with. What follows it is the DETAIL the operation reports, which
/// is that operation's own — the same `<subject> not found - <detail>` shape this service's
/// absent-group refusals carry ([`ABSENT_GROUP_PREFIX`]).
pub const ABSENT_INSTANCE_PREFIX: &str = "Instance Id not found - ";

/// The message an instance-scoped request that names NO Auto Scaling group is refused with
/// when no group of the account manages the instance it names.
///
/// Such a request carries one instance id and nothing else that identifies a group, so the
/// refusal is about the INSTANCE: the subject is not findable among the instances Amazon EC2
/// Auto Scaling manages, whether the account holds no Amazon EC2 instance of that id at all
/// or holds one that is simply in no group. The detail form here is the TERMINATION's own —
/// `No managed instance found for instance ID: ` after the prefix — and its ONE variable
/// part is the instance id the request carried; the health status's own detail form is
/// [`instance_health::absent_instance_message`].
pub fn instance_managed_by_no_group_message(instance_id: &str) -> String {
    format!("{ABSENT_INSTANCE_PREFIX}No managed instance found for instance ID: {instance_id}")
}

/// The message a request that would bring an instance into a group which is ALREADY an
/// instance of an Auto Scaling group is refused with.
///
/// An Amazon EC2 instance belongs to at most one Auto Scaling group, so the sentence names
/// the instance and the group that holds it — which may be the group the request itself
/// named, when a caller attaches the same instance twice. The fixed text is `Instance ` /
/// [`ALREADY_PART_OF_GROUP_CLAUSE`] / `.`; it is the converse of
/// [`instance_not_in_group_message`], and worded the same way.
pub fn instance_already_in_group_message(instance_id: &str, group_name: &str) -> String {
    format!("Instance {instance_id}{ALREADY_PART_OF_GROUP_CLAUSE}{group_name}.")
}

/// The fixed clause [`instance_already_in_group_message`] is built around, stated once.
const ALREADY_PART_OF_GROUP_CLAUSE: &str = " is already part of Auto Scaling group ";

/// The message a termination request that names no instance at all is refused with: the
/// operation terminates the instances a request names, so one of the two members that
/// name them has to be there.
///
/// Same shape as the create's own "Valid requests must contain …" refusal: fixed text
/// naming the members a valid request may use, with nothing taken from the request.
pub const NO_INSTANCE_TO_TERMINATE_MESSAGE: &str =
    "Valid requests must contain either the InstanceId or the InstanceIds parameter.";

/// The message a termination request naming instances through `InstanceIds` without the
/// group that holds them is refused with.
///
/// From the model's own documentation for
/// `TerminateInstanceInAutoScalingGroupType$InstanceIds`: "This parameter requires that
/// you also specify AutoScalingGroupName." Fixed text, with nothing taken from the
/// request.
pub const GROUP_NAME_REQUIRED_WITH_INSTANCE_IDS_MESSAGE: &str =
    "Valid requests must contain the AutoScalingGroupName parameter when the InstanceIds \
     parameter is specified.";

/// The message a termination that asks to decrement the desired capacity below the
/// group's own `MinSize` is refused with, and what to change instead.
///
/// A group's desired capacity never goes below the minimum it is configured with, so the
/// decrement is refused rather than applied in part. A request naming ONE instance can
/// only cross the minimum when the desired capacity is already AT it — the capacity is
/// never under it — which is the sentence's first form, and the size the two bounds share
/// is its only variable part. A request naming several states the two sizes it crossed
/// and how many instances it asked to terminate; the rest of the sentence is the same
/// fixed text.
pub fn min_size_breach_message(desired_capacity: i32, min_size: i32, terminating: usize) -> String {
    let remedy = "Either set shouldDecrementDesiredCapacity flag to false or lower group's \
                  min size.";
    if desired_capacity == min_size {
        format!(
            "Currently, desiredSize equals minSize ({min_size}). Terminating instance without \
             replacement will violate group's min size constraint. {remedy}"
        )
    } else {
        format!(
            "Currently, desiredSize is {desired_capacity} and minSize is {min_size}. \
             Terminating {terminating} instances without replacement will violate group's min \
             size constraint. {remedy}"
        )
    }
}

/// Which of the two bounds a group is configured with a requested capacity crossed, and
/// the value of that bound as the group holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityBound {
    /// The group's `MinSize`, which the requested capacity is below.
    Min(i32),
    /// The group's `MaxSize`, which the requested capacity is above.
    Max(i32),
}

/// The sentence a capacity change is refused with when the capacity it asks for falls
/// outside the bounds the group is configured with.
///
/// Fixed text around exactly TWO variable parts — the capacity the request named and the
/// bound of the group it crossed — and nothing else: the sentence ends in a period after
/// `AutoScalingGroup`, naming neither the group nor the account nor any other value, so a
/// caller that classifies the bounds error by this sentence sees the same text for every
/// group.
#[must_use]
pub fn capacity_outside_bounds_message(desired_capacity: i32, bound: CapacityBound) -> String {
    let (relation, limit) = match bound {
        CapacityBound::Min(min_size) => ("below min", min_size),
        CapacityBound::Max(max_size) => ("above max", max_size),
    };
    format!(
        "New SetDesiredCapacity value {desired_capacity} is {relation} value {limit} for the \
         AutoScalingGroup."
    )
}

/// The sentence a capacity change is refused with when the capacity it asks for is not a
/// count at all.
///
/// A negative capacity is refused for its SIGN rather than against either bound: no group's
/// minimum is what such a request missed, so the service states its own name, the value it
/// was handed and that the value is negative, and stops there.
#[must_use]
pub fn negative_capacity_message(desired_capacity: i32) -> String {
    format!("New SetDesiredCapacity value {desired_capacity} is negative.")
}

/// The sentence Amazon EC2 Auto Scaling states for `ScalingActivityInProgressFault` when
/// the instances an earlier change of the group is still taking out of service are what
/// blocks this one.
///
/// THE ONE COPY for this service: every operation that changes a group's size — the
/// desired-capacity set, the group update and the policy execution — answers that error
/// class with this sentence, so one class of one service has one wording. The group's name
/// is its only variable part and comes from the request.
#[must_use]
pub fn instances_still_terminating_message(group_name: &str) -> String {
    format!(
        "The AutoScalingGroup {group_name} still holds instances from an earlier scaling \
         activity: retry once they have finished terminating"
    )
}

/// The sentence Amazon EC2 Auto Scaling states for `InstanceRefreshInProgressFault`:
/// the error class says that a refresh is already running on the group, not which one,
/// so the sentence has NO variable part.
///
/// THE ONE COPY for this service: every operation the model declares that error for —
/// starting a second refresh, and putting a warm pool while one is running — answers it
/// with this, so one error class of one service has one sentence.
pub const INSTANCE_REFRESH_IN_PROGRESS_MESSAGE: &str =
    "An Instance Refresh is already in progress and blocks the execution of this Instance Refresh.";

/// The absent-group refusal whose detail QUOTES the name it was asked for.
pub fn absent_group_message_quoting_name(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}AutoScalingGroup '{name}' not found")
}

/// The absent-group refusal whose detail spells the resource out as the service's own
/// prose name for it.
pub fn absent_group_message_in_prose(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}Auto Scaling group with name {name} not found")
}

/// The absent-group refusal whose detail states that no such group exists, the detail
/// form `DescribeWarmPool` serves.
///
/// Separate from the forms above because the detail is each operation's own: the name is
/// the only part taken from the request, and the rest of the sentence, punctuation
/// included, is fixed text.
pub fn absent_group_message_as_no_such_group(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}No such Auto Scaling group: {name}")
}

/// The absent-group refusal whose detail field is the asked-for name ALONE, the detail
/// form `PutNotificationConfiguration` serves.
///
/// The name is the only part taken from the request; the prefix is the fixed text every
/// absent-group refusal of this service opens with.
pub fn absent_group_message_as_name(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}{name}")
}

/// The absent-group refusal whose detail LABELS the resource and then states the name it
/// was asked for was not found — the detail form `AttachInstances` serves.
///
/// Separate from the forms above because the detail is each operation's own: the name is
/// the only part taken from the request, interpolated bare after the label, and the rest
/// of the sentence, punctuation included, is fixed text.
pub fn absent_group_message_labelling_name(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}AutoScalingGroup: {name} not found")
}

/// The absent-group refusal whose detail states that the group could not be FOUND under
/// the name asked for, quoted — the detail form `DescribeLoadBalancerTargetGroups`
/// serves.
///
/// The name is the only part taken from the request; the rest, punctuation included, is
/// fixed text.
pub fn absent_group_message_as_not_findable(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}Could not find Auto Scaling group named '{name}'")
}

/// The refusal `PutScalingPolicy` and the two writers of a group's metrics collection
/// (`EnableMetricsCollection`, `DisableMetricsCollection`) answer a group the account does
/// not hold with: ONE sentence about the group, naming it, and NOT built on
/// [`ABSENT_GROUP_PREFIX`] at all.
///
/// Separate from the forms above because this refusal is the whole message rather than a
/// detail behind that prefix: the name is its only variable part, and the rest,
/// punctuation included, is fixed text.
pub fn absent_group_message_as_group_not_found(name: &str) -> String {
    format!("Group {name} not found")
}

/// That same sentence CLOSED with a full stop — the form `GetPredictiveScalingForecast`
/// serves.
///
/// Built on the sentence above rather than written out again, because the two differ in
/// nothing but the punctuation each operation's own refusal ends with: the name is the only
/// variable part of either.
pub fn absent_group_message_as_group_not_found_sentence(name: &str) -> String {
    format!("{}.", absent_group_message_as_group_not_found(name))
}

/// The absent-group refusal whose detail field the operation leaves unfilled, so it
/// travels as the literal token `null`: the whole message is fixed text and no part of
/// the request appears in it. Its subject is the GROUP the request named, which is why the
/// detail may stand unfilled here — a refusal about an INSTANCE names that instance
/// ([`instance_managed_by_no_group_message`]) rather than leaving a slot of its own empty.
pub const ABSENT_GROUP_MESSAGE_WITHOUT_DETAIL: &str = "AutoScalingGroup name not found - null";

/// The absent-group refusal whose detail states that no group of that name exists, spelled
/// the way `SuspendProcesses` spells it — the same detail as
/// [`absent_group_message_as_no_such_group`] in the lower-case, abbreviated form that
/// operation's own refusal states.
///
/// Separate from the forms above because the detail is each operation's own: the name is the
/// only part taken from the request, and the rest, punctuation included, is fixed text.
pub fn absent_group_message_as_lower_case_no_such_group(name: &str) -> String {
    format!("{ABSENT_GROUP_PREFIX}no such group: {name}")
}

/// The fixed text the absent-group refusals of a few operations open with INSTEAD: the
/// resource word with no `name` after it, so the opening is about the GROUP that was not
/// found rather than about the name it was asked for.
const ABSENT_GROUP_SUBJECT_PREFIX: &str = "AutoScalingGroup not found - ";

/// That opening with the detail that no such group exists — the form `LaunchInstances`
/// serves.
///
/// The name is the only part taken from the request; the rest, punctuation included, is
/// fixed text.
pub fn absent_group_message_as_subject_without_such_group(name: &str) -> String {
    format!("{ABSENT_GROUP_SUBJECT_PREFIX}No such Auto Scaling group: {name}")
}

/// That opening with the detail naming the resource as two words and then the name asked
/// for — the form the write that creates or updates a group's tags serves.
///
/// The name is the only part taken from the request; the rest, punctuation included, is
/// fixed text.
pub fn absent_group_message_as_subject_naming_group(name: &str) -> String {
    format!("{ABSENT_GROUP_SUBJECT_PREFIX}AutoScaling Group {name} not found")
}

/// The sentence a rollback of a refresh that carries no desired configuration is
/// refused with: the first of the situations the error class names, and the whole of
/// the message — the refusal is about the refresh's own configuration, so no part of
/// the request appears in it.
pub const ROLLBACK_WITHOUT_DESIRED_CONFIGURATION_MESSAGE: &str =
    "The instance refresh cannot be rolled back because there is no desired configuration \
     specified for the instance refresh.";

/// The sentence a rollback of a group whose launch template version floats is refused
/// with: the pre-refresh configuration is not pinned to a version a rollback could go
/// back to.
///
/// Fixed text with no variable part: it names BOTH floating versions, because the refusal
/// is about the group using either of them rather than about the one it happens to name.
pub const FLOATING_LAUNCH_TEMPLATE_VERSION_MESSAGE: &str =
    "This Auto Scaling group's launch template version isn't compatible with a rollback. \
     Rollbacks are not supported when the Auto Scaling group uses the launch template's \
     '$Latest' or '$Default' version.";

/// The sentence a rollback of a group whose launch template resolves its image through
/// a Systems Manager parameter is refused with.
pub const SSM_PARAMETER_IMAGE_ID_MESSAGE: &str =
    "The instance refresh cannot be rolled back because the Auto Scaling group has a launch \
     template that uses a Systems Manager parameter instead of an AMI ID for the ImageId \
     property.";

/// The launch template specification a launch source names: the one it carries
/// directly, or the one its mixed instances policy carries.
pub fn launch_template_of<'a>(
    launch_template: Option<&'a LaunchTemplateSpecification>,
    mixed_instances_policy: Option<&'a MixedInstancesPolicy>,
) -> Option<&'a LaunchTemplateSpecification> {
    launch_template.or_else(|| {
        mixed_instances_policy
            .and_then(|policy| policy.launch_template.as_ref())
            .and_then(|template| template.launch_template_specification.as_ref())
    })
}

/// Whether two specifications name the same launch template and version, as WRITTEN.
///
/// All there is to go on for a template the account does not hold, and therefore the
/// fallback the resolving comparison in [`ec2_instances::instances_not_running`] takes:
/// that one is what a caller asks, because the same launch-template version can be written
/// in several ways.
pub fn same_launch_template(
    left: Option<&LaunchTemplateSpecification>,
    right: Option<&LaunchTemplateSpecification>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.launch_template_id == right.launch_template_id
                && left.launch_template_name == right.launch_template_name
                && left.version == right.version
        }
        (None, None) => true,
        _ => false,
    }
}

/// The instance types a launch source names EXPLICITLY, beside the launch template it
/// launches them from: the `InstanceType` of each override of a mixed instances policy.
///
/// An instance refresh skips "instances that match the desired configuration"
/// (`smithy_query(query="doc",shape="RefreshPreferences",member="SkipMatching")`), and a
/// configuration that names its own instance types is matched on them too: an instance
/// of a type the configuration does not name does not run that configuration, whatever
/// template it was launched from. A launch source that names none leaves the type to the
/// template, so every instance launched from that template matches on type.
///
/// A SET, built once: the types come from the request, so a membership test that scanned
/// them would cost the group's size times the request's length.
pub fn declared_instance_types(
    mixed_instances_policy: Option<&MixedInstancesPolicy>,
) -> BTreeSet<String> {
    mixed_instances_policy
        .and_then(|policy| policy.launch_template.as_ref())
        .and_then(|template| template.overrides.as_ref())
        .map(|overrides| {
            overrides
                .iter()
                .filter_map(|entry| entry.instance_type.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Join the values a refusal names into the comma-separated list a service message
/// carries, so no refusal shows a caller Rust's own debug formatting.
///
/// THE ONE COPY of that join: every refusal of this service that names a set of values
/// states it through here, whether it states them on their own or bracketed
/// ([`stated_value_set`]).
pub fn joined_values(values: impl Iterator<Item = String>) -> String {
    values.collect::<Vec<String>>().join(", ")
}

/// The admissible value set a refusal states, in the order the service publishes it:
/// the values comma-separated inside brackets, which is the shape such a set is stated
/// in on the wire.
pub fn stated_value_set<'a>(values: impl Iterator<Item = &'a str>) -> String {
    format!(
        "[{}]",
        joined_values(values.map(::std::string::ToString::to_string))
    )
}

/// The standard sentence this service states for ONE member value outside the bounds it
/// has to satisfy: the value as the request wrote it, the member as the WIRE names it
/// (lowerCamel, the spelling the boundary validator uses), and the constraint it missed.
///
/// THE ONE COPY of that sentence: every bound this service checks in a handler — because
/// the model does not express it — is reported through here, so two of them cannot drift
/// into two different spellings of the same refusal.
pub fn constraint_violation_message(
    value: impl ::std::fmt::Display,
    member: &str,
    constraint: &str,
) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {constraint}"
    )
}
