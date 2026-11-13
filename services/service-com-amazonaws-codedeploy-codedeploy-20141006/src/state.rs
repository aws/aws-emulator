// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codedeploy-codedeploy-20141006 service.
//!
//! Every record a handler answers from lives here: applications (with their
//! deployment groups and their registered revisions), deployments, and the
//! deployment configurations. Handlers
//! reach it through [`with_state`], which is also where the two things that must
//! happen before ANY request is served happen: seeding the `CodeDeployDefault.*`
//! built-in deployment configurations once, and materialising the deployment
//! transitions the clock already owes — the stop a sounding alarm asks for, the
//! traffic increment a shift in flight owes its alias, and the status a
//! deployment has reached — and queueing the trigger notifications those
//! transitions owe the deployment group's Amazon SNS topics
//! ([`trigger_notifications`]), which the caller then publishes with this store
//! unlocked.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// The CloudWatch alarm gate a deployment in flight runs under: the one place a
/// stored `alarmConfiguration` is evaluated.
pub mod alarm_gate;

/// The rollback deployment a stopped deployment is given, and the `RollbackInfo`
/// linking the two records.
pub mod rollback;

/// What a revision's AppSpec DECLARES a deployment deploys: the Lambda function,
/// the alias traffic moves on and the two versions it moves between — the one
/// place that document is read.
pub mod appspec;

/// The Lambda alias a deployment moves, written where that alias LIVES: the
/// Lambda service's own function store.
pub mod lambda_alias;

/// The traffic INCREMENTS a deployment's resolved `trafficRoutingConfig`
/// describes: how much traffic each one moves, and when it falls due.
pub mod traffic_shift;

/// The traffic shift an Lambda deployment performs: the one place a declared
/// shift is carried out, increment by increment, and the one place a deployment
/// that could not carry it out is failed.
pub mod lambda_traffic_shift;

/// The in-process task that drives a deployment in flight while nobody is calling
/// this service.
pub mod deployment_clock;

/// The LIFECYCLE EVENT HOOKS a deployment runs through a Lambda validation
/// function: which ones its revision declares, and the gate the results they
/// report hold the deployment at.
pub mod lifecycle_hook;

/// The INVOCATION of one of those validation functions, in AWS Lambda itself.
pub mod hook_invocation;

/// The Amazon SNS notifications a deployment group's stored
/// `triggerConfigurations` publish: the one place a trigger is ACTED on.
pub mod trigger_notifications;

/// The service role a deployment group runs under, resolved against the
/// account's own IAM records: the one place a `serviceRoleArn` is asked about.
pub mod service_role;

/// Whether that service role AUTHORIZES the calls a deployment makes: the one
/// place a deployment is failed for the permissions its role withholds.
pub mod service_role_permissions;

/// The SHA256 digest of a revision that carries its content inline: the one place
/// that digest is computed, where such a revision is recorded.
pub mod revision_digest;

/// The revision a deployment deploys when it names an S3 object, FETCHED from the
/// object store that holds it: the one place a revision's identity is resolved and
/// the one place a deployment whose revision cannot be located is failed.
pub mod s3_revision;

use crate::ops::types::{
    AlarmConfiguration, AutoRollbackConfiguration, BlueGreenDeploymentConfiguration,
    DeploymentStyle, EC2DeploymentOptions, EC2TagFilter, EC2TagSet, ECSService, LoadBalancerInfo,
    MinimumHealthyHosts, OnPremisesTagSet, RevisionLocation, RollbackInfo, Tag, TagFilter,
    TargetInstances, TrafficRoutingConfig, TriggerConfig, ZonalConfig,
};

/// How long after `CreateDeployment` the emulator owes the deployment its start,
/// and then its terminal status.
///
/// CodeDeploy documents no duration for either step, so both spans are CHOSEN,
/// and neither is zero: the window between them is the one every client of a
/// deployment reads — a caller polls `GetDeployment` for `InProgress`, reads its
/// targets while they are being deployed to, and is refused a second deployment
/// to the same deployment group while this one is active — so it has to OUTLAST
/// the call that opened it and the next few calls a client makes. The start is the
/// lead-in a caller sees `Created` for; the deployment then runs for the span this
/// emulator gives a transition it owes a caller, which is where the work a real
/// deployment does — the agent on each target, the traffic shift, the validation
/// functions — would be spent.
const DEPLOYMENT_START_MILLIS: i64 = 250;
/// See [`DEPLOYMENT_START_MILLIS`]: three seconds after the deployment starts.
const DEPLOYMENT_COMPLETION_MILLIS: i64 = DEPLOYMENT_START_MILLIS + 3_000;

/// The statuses a deployment the emulator created moves through. All three are
/// members of the model's `DeploymentStatus` enum
/// (`smithy_query(query="shape",shape="DeploymentStatus")`: Created | Queued |
/// InProgress | Baking | Succeeded | Failed | Stopped | Ready).
pub const DEPLOYMENT_STATUS_CREATED: &str = "Created";
/// See [`DEPLOYMENT_STATUS_CREATED`].
pub const DEPLOYMENT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`DEPLOYMENT_STATUS_CREATED`].
pub const DEPLOYMENT_STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`DEPLOYMENT_STATUS_CREATED`]. The terminal status of a deployment whose
/// deployment group resolved no target to deploy to (see
/// [`deployment_targets`]), and the one a deployment reaches when something
/// that is part of it failed — a lifecycle event hook reporting a failed
/// validation, say.
pub const DEPLOYMENT_STATUS_FAILED: &str = "Failed";
/// See [`DEPLOYMENT_STATUS_CREATED`]: the status a deployment a caller stopped
/// holds.
pub const DEPLOYMENT_STATUS_STOPPED: &str = "Stopped";

/// The `DeploymentTargetType` of a target of an EC2/on-premises deployment
/// (`smithy_query(query="shape",shape="DeploymentTargetType")`: InstanceTarget |
/// LambdaTarget | ECSTarget | CloudFormationTarget).
pub const DEPLOYMENT_TARGET_TYPE_INSTANCE: &str = "InstanceTarget";
/// See [`DEPLOYMENT_TARGET_TYPE_INSTANCE`]: the type of a Lambda deployment's
/// target.
pub const DEPLOYMENT_TARGET_TYPE_LAMBDA: &str = "LambdaTarget";
/// See [`DEPLOYMENT_TARGET_TYPE_INSTANCE`]: the type of an Amazon ECS
/// deployment's target.
pub const DEPLOYMENT_TARGET_TYPE_ECS: &str = "ECSTarget";

/// The `TargetStatus` of a target the deployment has not finished with; see
/// [`target_status`]. The named statuses here are the ones this service MAPS a
/// deployment's progress onto; the catalogue of every value the member accepts
/// is the generated `crate::ops::types::TARGET_STATUS_VALUES`, which is what a
/// filter's value is checked against.
pub const TARGET_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`TARGET_STATUS_IN_PROGRESS`].
pub const TARGET_STATUS_PENDING: &str = "Pending";
/// See [`TARGET_STATUS_IN_PROGRESS`].
pub const TARGET_STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`TARGET_STATUS_IN_PROGRESS`].
pub const TARGET_STATUS_FAILED: &str = "Failed";
/// See [`TARGET_STATUS_IN_PROGRESS`].
pub const TARGET_STATUS_SKIPPED: &str = "Skipped";

/// The `TargetLabel` of a target in the ORIGINAL environment of a blue/green
/// deployment, which is the only label this service resolves; see
/// [`deployment_targets`]. The catalogue of every value the member accepts is
/// the generated `crate::ops::types::TARGET_LABEL_VALUES`.
pub const TARGET_LABEL_BLUE: &str = "Blue";

/// The two `LifecycleEventStatus` values a validation function may report
/// (`smithy_query(query="shape",shape="LifecycleEventStatus")` carries six:
/// Pending | InProgress | Succeeded | Failed | Skipped | Unknown — and
/// `PutLifecycleEventHookExecutionStatus`'s own documentation says "only
/// Succeeded and Failed can be passed successfully in your API call").
pub const LIFECYCLE_EVENT_STATUS_SUCCEEDED: &str = "Succeeded";
/// See [`LIFECYCLE_EVENT_STATUS_SUCCEEDED`].
pub const LIFECYCLE_EVENT_STATUS_FAILED: &str = "Failed";
/// See [`LIFECYCLE_EVENT_STATUS_SUCCEEDED`]: the status of a lifecycle event this
/// service has dispatched to its validation function and is awaiting the report
/// of ([`hook_event_status`]).
pub const LIFECYCLE_EVENT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`LIFECYCLE_EVENT_STATUS_IN_PROGRESS`]: a lifecycle event the deployment
/// has not reached yet.
pub const LIFECYCLE_EVENT_STATUS_PENDING: &str = "Pending";
/// See [`LIFECYCLE_EVENT_STATUS_IN_PROGRESS`]: a lifecycle event a deployment
/// that was stopped never ran.
pub const LIFECYCLE_EVENT_STATUS_SKIPPED: &str = "Skipped";
/// See [`LIFECYCLE_EVENT_STATUS_IN_PROGRESS`]: a lifecycle event this service
/// holds no status for.
pub const LIFECYCLE_EVENT_STATUS_UNKNOWN: &str = "Unknown";

/// The `ErrorCode` CodeDeploy reports on a deployment a lifecycle event hook
/// failed (`smithy_query(query="shape",shape="ErrorCode")`).
pub const ERROR_CODE_HOOK_EXECUTION_FAILURE: &str = "HOOK_EXECUTION_FAILURE";

/// Whether `status` is one a deployment never leaves.
///
/// The model's `DeploymentStatus` enum
/// (`smithy_query(query="shape",shape="DeploymentStatus")`) is Created | Queued |
/// InProgress | Baking | Succeeded | Failed | Stopped | Ready; the last three of
/// those are outcomes, and the rest are stages a deployment is still working
/// through. An operation that acts on a deployment in flight — stopping it,
/// continuing it — is what the modelled `DeploymentAlreadyCompletedException`
/// refuses once the deployment holds one of the outcomes.
pub fn deployment_is_terminal(status: &str) -> bool {
    matches!(
        status,
        DEPLOYMENT_STATUS_SUCCEEDED | DEPLOYMENT_STATUS_FAILED | DEPLOYMENT_STATUS_STOPPED
    )
}

/// The `ErrorCode` CodeDeploy reports on a deployment a CALLER stopped
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it; the AWS CodeDeploy
/// User Guide's "Error codes" documents `MANUAL_STOP` as the deployment having
/// been stopped by a user).
///
/// A stop is how a deployment ENDED, so the deployment attributes itself to it the
/// same way a deployment the alarm gate stopped attributes itself to
/// [`alarm_gate::ERROR_CODE_ALARM_ACTIVE`].
pub const ERROR_CODE_MANUAL_STOP: &str = "MANUAL_STOP";

/// The `ErrorInformation$message` that goes with [`ERROR_CODE_MANUAL_STOP`]: this
/// outcome's own fixed sentence, interpolating nothing — every deployment a caller
/// stopped ended the same way, and who stopped it is not something this service
/// reports here. CodeDeploy words it as the deployment having been cancelled on
/// request, in the lower case it answers it in.
pub const MANUAL_STOP_MESSAGE: &str = "the deployment is cancelled due to an abort request";

/// The `ErrorCode` a deployment that found nothing to deploy to carries
/// (`smithy_query(query="shape",shape="ErrorCode")` lists it; the AWS CodeDeploy
/// User Guide's "Error codes" documents `NO_INSTANCES` as "no instances were
/// found in your deployment group").
pub const ERROR_CODE_NO_INSTANCES: &str = "NO_INSTANCES";

/// The `ErrorInformation$message` that goes with [`ERROR_CODE_NO_INSTANCES`]:
/// this failure's own fixed sentence — what failed, where to look, and to try
/// again — interpolating nothing from the deployment or its group, since every
/// deployment that resolved no target failed the same way.
pub const NO_INSTANCES_MESSAGE: &str = "The deployment failed because no instances were found \
     for your deployment group. Check your deployment group settings to make sure the tags for \
     your Amazon EC2 instances or Auto Scaling groups correctly identify the instances you want \
     to deploy to, and then try again.";

/// The characters a minted deployment-id suffix is drawn from, and how many of
/// them one carries: CodeDeploy's `d-XXXXXXXXX` deployment ids.
const DEPLOYMENT_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
/// See [`DEPLOYMENT_ID_ALPHABET`].
const DEPLOYMENT_ID_SUFFIX_LEN: usize = 9;
/// See [`DEPLOYMENT_ID_ALPHABET`].
const DEPLOYMENT_ID_PREFIX: &str = "d-";

/// What a handler answers when the engine it was constructed with is not the
/// shared in-memory store.
///
/// Unreachable through the router, which mounts every service on the one shared
/// engine. CodeDeploy's model declares no server-fault error, so this is stated
/// as a refusal rather than invented as a 5xx — and NEVER by building a private
/// engine, which would silently drop the write.
pub const STATE_UNAVAILABLE: &str =
    "CodeDeploy state is unavailable: this service was mounted on an engine that does not \
     provide the shared in-memory store";

// ---------------------------------------------------------------------------
// The sentences CodeDeploy's modelled client errors carry
// ---------------------------------------------------------------------------
//
// RULE: every modelled client error of this service carries the sentence that
// belongs to the ERROR SHAPE, interpolating only the request value that error is
// about — the resource's own name or id, delimited from the prose by `: ` where
// the sentence ends in it — and no other request member; a refusal of an absent
// or empty member answers its `…RequiredException`'s own sentence rather than
// the constraint validator's generic clause. Every message a handler or
// `crate::service` answers is built HERE, so the same error cannot be worded two
// ways by two operations (`ApplicationDoesNotExistException` is raised by five of
// them) and a message cannot drift into naming a member it is not about.

/// `ApplicationDoesNotExistException` (HTTP 400): the application name after the
/// delimiter, and nothing else from the request.
pub fn application_not_found(application_name: &str) -> String {
    format!("No application found for name: {application_name}")
}

/// `DeploymentGroupDoesNotExistException` (HTTP 400): the group name alone — the
/// application the group was looked up in is NOT part of this sentence, which is
/// scoped to the group.
pub fn deployment_group_not_found(deployment_group_name: &str) -> String {
    format!("No Deployment Group found for name: {deployment_group_name}")
}

/// `DeploymentDoesNotExistException` (HTTP 400): the deployment id the request
/// asked for, inside the sentence rather than after a delimiter.
pub fn deployment_not_found(deployment_id: &str) -> String {
    format!("The deployment {deployment_id} could not be found")
}

/// `DeploymentNotStartedException` (HTTP 400): the deployment id the request
/// asked about, in the same form as [`deployment_not_found`] — an operation over
/// a deployment's targets is refused with this before the deployment has started
/// (see [`Deployment::is_started`]).
pub fn deployment_not_started(deployment_id: &str) -> String {
    format!("The deployment {deployment_id} has not started")
}

/// The INTERNAL state name of a deployment status.
///
/// CodeDeploy reports a deployment's progress to a client as a
/// `DeploymentStatus` member (`smithy_query(query="shape",shape="DeploymentStatus")`:
/// Created | Queued | InProgress | Baking | Succeeded | Failed | Stopped |
/// Ready), and names the same state INTERNALLY as an upper-snake-case token —
/// the form every value set this service puts on the wire uses (`DeploymentType`
/// is IN_PLACE | BLUE_GREEN). The two names differ for exactly one state: a
/// deployment a caller stopped, whose API status is `Stopped`, is `ABORTED`
/// internally, which is the internal state a refusal about a stopped deployment
/// reports.
///
/// Derived from the status rather than tabulated, so a status this service does
/// not itself drive a deployment through still has its internal name.
pub fn deployment_internal_state(status: &str) -> String {
    if status == DEPLOYMENT_STATUS_STOPPED {
        return DEPLOYMENT_INTERNAL_STATE_ABORTED.to_string();
    }
    let mut internal = String::with_capacity(status.len() + 2);
    for (index, character) in status.char_indices() {
        if index > 0 && character.is_uppercase() {
            internal.push('_');
        }
        internal.extend(character.to_uppercase());
    }
    internal
}

/// See [`deployment_internal_state`]: the internal state of a deployment a
/// caller stopped, the one whose internal name is not its API status.
pub const DEPLOYMENT_INTERNAL_STATE_ABORTED: &str = "ABORTED";

/// `DeploymentAlreadyCompletedException` (HTTP 400): the deployment id the
/// request named and the INTERNAL state the deployment settled in
/// ([`deployment_internal_state`]), which is the state a refusal about a
/// deployment's progress reports — so a caller learns which outcome it was
/// refused for, and every operation that acts on a deployment in flight words
/// that refusal the same way.
pub fn deployment_already_completed(deployment_id: &str, status: &str) -> String {
    format!(
        "The deployment {deployment_id} is already in state {}",
        deployment_internal_state(status)
    )
}

/// `DeploymentLimitExceededException` (HTTP 400): the deployment group a second
/// deployment was refused for, and the deployment of it that is still active.
///
/// A deployment group runs one deployment at a time, so the refusal names what is
/// in the way: the group — by the name the request carried AND by the id
/// CodeDeploy minted for it, which is how every refusal about a deployment group's
/// own state identifies it — and the deployment holding it.
pub fn deployment_already_active(
    deployment_group_name: &str,
    deployment_group_id: &str,
    active_deployment_id: &str,
) -> String {
    format!(
        "The Deployment Group '{deployment_group_name} (id={deployment_group_id})' already has \
         an active Deployment '{active_deployment_id}'"
    )
}

/// `LifecycleEventAlreadyCompletedException` (HTTP 400): the hook execution of a
/// deployment that has already reported its result, so its lifecycle event
/// completed with that result and there is none left to report.
pub fn lifecycle_event_already_completed(deployment_id: &str, hook_execution_id: &str) -> String {
    format!(
        "The lifecycle event of deployment {deployment_id} reported by hook execution \
         {hook_execution_id} has already completed"
    )
}

/// `LifecycleEventAlreadyCompletedException` (HTTP 400): the deployment itself
/// reached a terminal status, so every lifecycle event of it completed with the
/// deployment and none has a result left to report.
pub fn lifecycle_events_already_completed(deployment_id: &str) -> String {
    format!("The lifecycle events of deployment {deployment_id} have already completed with it")
}

/// `UnsupportedActionForDeploymentTypeException` (HTTP 400): the deployment id
/// the request named and the `DeploymentType` the deployment runs as
/// ([`Deployment::deployment_type`]), so a caller refused for its deployment's
/// type learns which type was rejected rather than only that one was.
pub fn unsupported_action_for_deployment_type(deployment_id: &str, deployment_type: &str) -> String {
    format!(
        "Operation is not supported for the deployment {deployment_id} of type {deployment_type}."
    )
}

/// `DeploymentTargetDoesNotExistException` (HTTP 400): the deployment the target
/// does not belong to, which is what the shape's own sentence says ("The
/// provided target ID does not belong to the attempted deployment"), and the
/// target itself identified by its ARN.
///
/// A refusal about a deployment TARGET names the target the way a target is
/// identified as a resource — by its ARN ([`deployment_target_arn`]) — rather
/// than by the id the request carried: the ARN is what lets a caller correlate
/// the rejection with the resource it asked about (its partition, region,
/// account and resource type) and match it against the ARNs the on-premises and
/// target readers hand it. An id no ARN follows from is quoted as it was sent,
/// bounded by [`echoed`], so the refusal still names what the caller asked for.
pub fn deployment_target_not_found(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
    region: &str,
    target_id: &str,
) -> String {
    let target = deployment_target_arn(state, &deployment.compute_platform, region, target_id)
        .unwrap_or_else(|| echoed(target_id));
    format!(
        "The target {target} is not part of deployment {}",
        deployment.deployment_id
    )
}

/// `InstanceDoesNotExistException` (HTTP 400): the instance id the request asked
/// for and the deployment whose group holds no such instance, which is what the
/// shape's own sentence says ("The specified instance does not exist in the
/// deployment group"). The instance-shaped refusal that answers exactly what
/// [`deployment_target_not_found`] answers for the target-shaped readers of the
/// same records, so the two cannot disagree about an instance neither can look
/// up; the id is bounded by [`echoed`].
pub fn deployment_instance_not_found(deployment_id: &str, instance_id: &str) -> String {
    format!(
        "The instance {} does not exist in the deployment group of the deployment {deployment_id}",
        echoed(instance_id)
    )
}

/// `InvalidDeploymentTargetIdException` (HTTP 400): the refusal every operation
/// taking a `targetId` answers a value outside the FORM a target of that compute
/// platform is reported under with, so the single-target reader and the batch
/// reader cannot word it differently. It quotes the id the caller sent, bounded
/// by [`echoed`].
pub fn malformed_target_id_message(target_id: &str) -> String {
    format!(
        "The deployment target ID {} is not in the valid format",
        echoed(target_id)
    )
}

/// `InvalidTargetFilterNameException` (HTTP 400): the shape's own sentence, with
/// the filter name the caller sent after the delimiter. The names a target
/// filter may carry are the members of `TargetFilterName`
/// (`smithy_query(query="shape",shape="TargetFilterName")`: TargetStatus |
/// ServerInstanceLabel).
pub fn invalid_target_filter_name(name: &str) -> String {
    format!("The target filter name is invalid: {}", echoed(name))
}

/// `InvalidInstanceStatusException` (HTTP 400): the shape's own sentence, with
/// the status the caller asked to filter on. The statuses that exist are the
/// members of `TargetStatus` (`crate::ops::types::TARGET_STATUS_VALUES`).
pub fn invalid_target_status(value: &str) -> String {
    format!(
        "The specified instance status does not exist: {}",
        echoed(value)
    )
}

/// `InvalidInstanceTypeException` (HTTP 400): the shape's own sentence, with the
/// label the caller asked to filter on. The labels that exist are the members of
/// `TargetLabel` (`crate::ops::types::TARGET_LABEL_VALUES`).
pub fn invalid_instance_label(value: &str) -> String {
    format!(
        "An invalid instance type was specified for instances in a blue/green deployment: {}",
        echoed(value)
    )
}

/// `InvalidDeploymentInstanceTypeException` (HTTP 400): the shape's own
/// sentence, interpolating nothing from the request.
///
/// An "instance type" here is the ENVIRONMENT an instance belongs to — the
/// `TargetLabel` of Blue or Green, as `InvalidInstanceTypeException` spells out
/// ("Valid values include 'Blue' for an original environment and 'Green' for a
/// replacement environment") — and only a blue/green deployment has two
/// environments to tell apart. So a request that selects targets by instance
/// type against an in-place deployment is refused with this rather than answered
/// with the empty page that would suggest the deployment simply has no such
/// target.
pub const INSTANCE_TYPE_ON_IN_PLACE_DEPLOYMENT: &str =
    "An instance type was specified for an in-place deployment. Instance types are supported \
     for blue/green deployments only";

/// Whether a deployment on `compute_platform` has INSTANCES to report.
///
/// The instance-shaped readers of a deployment's targets — the deprecated
/// `ListDeploymentInstances`, `GetDeploymentInstance` and
/// `BatchGetDeploymentInstances` — report instances, which only the EC2/
/// on-premises and Lambda compute platforms have: the model says so on the
/// operation itself
/// (`smithy_query(query="doc",shape="ListDeploymentInstances")`:
/// "ListDeploymentInstances throws an exception if it is used with a compute
/// platform other than EC2/On-premises or Lambda", and "The newer
/// BatchGetDeploymentTargets should be used instead because it works with all
/// compute types"). `ComputePlatform`
/// (`smithy_query(query="shape",shape="ComputePlatform")`: Server | Lambda |
/// ECS) is the catalogue, so `ECS` is the member this answers `false` for —
/// stated POSITIVELY so a platform outside the enum, which this emulator can
/// target no instance of either, is answered the same way.
pub fn platform_reports_instances(compute_platform: &str) -> bool {
    matches!(compute_platform, "Server" | "Lambda")
}

/// `InvalidComputePlatformException` (HTTP 400): the shape's own sentence, with
/// the platform of the deployment the request named.
///
/// What makes the platform invalid here is the OPERATION, not the deployment: an
/// Amazon ECS deployment is a perfectly good deployment that simply has no
/// instances, so the sentence says which platforms the call serves. See
/// [`platform_reports_instances`].
pub fn instances_not_reported_for_platform(compute_platform: &str) -> String {
    format!(
        "The computePlatform is invalid: instances are reported only for deployments on the \
         EC2/On-premises or Lambda compute platform, and the specified deployment is on {}",
        echoed(compute_platform)
    )
}

/// `RevisionDoesNotExistException` (HTTP 400): the fixed sentence for an
/// application revision no registry holds, interpolating NOTHING.
///
/// A revision is identified by a LOCATION (an S3 object, a GitHub commit, a
/// string or AppSpec content), not by a scalar id, so there is no identifier of
/// the revision to state; and this error is scoped to the revision, so the
/// application the revision was looked up in — a request member the error is not
/// about — stays out of it, the same way [`deployment_group_not_found`] does not
/// name the application its group was looked up in.
pub const REVISION_DOES_NOT_EXIST: &str = "No application revision found for revision.";

/// `DeploymentConfigDoesNotExistException` (HTTP 400): the configuration name
/// after the delimiter, in the same form as the other not-found sentences.
pub fn deployment_config_not_found(deployment_config_name: &str) -> String {
    format!("No deployment configuration found for name: {deployment_config_name}")
}

/// `DeploymentConfigAlreadyExistsException` (HTTP 400): the conflicting
/// configuration name FIRST, so a client can recover it by stripping the fixed
/// tail — the same shape as [`application_already_exists`], because it is the
/// same "this name is taken" refusal about another resource of this service.
pub fn deployment_config_already_exists(deployment_config_name: &str) -> String {
    format!("{deployment_config_name} already exists.")
}

/// `InvalidOperationException` (HTTP 400): the fixed sentence for a deletion
/// aimed at a deployment configuration CodeDeploy itself ships — "Predefined
/// configurations cannot be deleted"
/// (`smithy_query(query="doc",shape="DeleteDeploymentConfig")`).
///
/// The refusal is about the shipped CATALOG rather than about the name the
/// request carried — every member of that catalog is refused the same way — so
/// it interpolates nothing.
pub const PREDEFINED_DEPLOYMENT_CONFIG_UNDELETABLE: &str =
    "Can not delete a predefined deployment configuration";

/// `DeploymentConfigInUseException` (HTTP 400): the name of the deployment
/// configuration something still uses — "A deployment configuration cannot be
/// deleted if it is currently in use"
/// (`smithy_query(query="doc",shape="DeleteDeploymentConfig")`). See
/// [`ComAmazonawsCodedeployCodedeploy20141006State::deployment_config_is_in_use`]
/// for what counts as using one.
pub fn deployment_config_in_use(deployment_config_name: &str) -> String {
    format!(
        "The deployment configuration {deployment_config_name} is currently in use and cannot be \
         deleted"
    )
}

/// `ApplicationAlreadyExistsException` (HTTP 400): the conflicting name FIRST,
/// so a client can recover it by stripping the fixed tail.
pub fn application_already_exists(application_name: &str) -> String {
    format!("{application_name} already exists.")
}

/// `ApplicationAlreadyExistsException` (HTTP 400) as the RENAME answers it: the
/// name the request asked to become, which is the member that conflicts.
///
/// A rename and a creation are refused by the same error shape but are not the
/// same refusal: a creation's conflicting name is the one the caller asked to
/// create, while a rename's is the one it asked to move TO, so the sentence says
/// which it is.
pub fn application_rename_target_exists(new_application_name: &str) -> String {
    format!("The application name: {new_application_name} you want to update to already exist")
}

/// `DeploymentGroupAlreadyExistsException` (HTTP 400): the conflicting group
/// name, with the application referred to as "this application" rather than
/// named.
pub fn deployment_group_already_exists(deployment_group_name: &str) -> String {
    format!(
        "An Deployment Group with the name {deployment_group_name} already exists for this \
         application."
    )
}

/// `ApplicationNameRequiredException` (HTTP 400): the fixed sentence for an
/// application name the request did not give — absent or empty are the same
/// refusal.
pub const APPLICATION_NAME_MISSING: &str = "Application name is missing";

/// `DeploymentGroupNameRequiredException` (HTTP 400). See
/// [`APPLICATION_NAME_MISSING`]: one fixed sentence per required-member error,
/// naming the member in prose and interpolating nothing. CodeDeploy capitalises
/// the resource in its own prose, as `deployment_group_already_exists` does.
pub const DEPLOYMENT_GROUP_NAME_MISSING: &str = "Deployment Group name is missing";

/// `DeploymentConfigNameRequiredException` (HTTP 400). See
/// [`APPLICATION_NAME_MISSING`].
pub const DEPLOYMENT_CONFIG_NAME_MISSING: &str = "Deployment configuration name is missing";

/// `DeploymentIdRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const DEPLOYMENT_ID_MISSING: &str = "Deployment ID is missing";

/// `InstanceNameRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const INSTANCE_NAME_MISSING: &str = "Instance name is missing";

/// `GitHubAccountTokenNameRequiredException` (HTTP 400). See
/// [`APPLICATION_NAME_MISSING`].
pub const GITHUB_ACCOUNT_TOKEN_NAME_MISSING: &str = "Token name is missing";

/// `InstanceIdRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const INSTANCE_ID_MISSING: &str = "Instance ID is missing";

/// `DeploymentTargetIdRequiredException` (HTTP 400). See
/// [`APPLICATION_NAME_MISSING`].
pub const DEPLOYMENT_TARGET_ID_MISSING: &str = "Deployment target ID is missing";

/// `RoleRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const ROLE_MISSING: &str = "Service role ARN is missing";

/// `ResourceArnRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const RESOURCE_ARN_MISSING: &str = "Resource ARN is missing";

/// `RevisionRequiredException` (HTTP 400). See [`APPLICATION_NAME_MISSING`].
pub const REVISION_MISSING: &str = "Revision is missing";

/// `InvalidArnException` (HTTP 400): this error class's fixed prefix followed by
/// the ARN the caller sent, which is the one thing they have to change, quoted
/// whole — `ResourceArn` carries a modelled maximum length, so what a refusal
/// quotes back is already bounded.
///
/// Every operation that reads a `ResourceArn` answers an unparseable one from
/// here, so the prefix a client matches on is the same whichever it called.
pub fn malformed_resource_arn_message(resource_arn: &str) -> String {
    format!("Invalid ARN: {resource_arn}")
}

/// `ArnNotSupportedException` (HTTP 400) as the tag-WRITING operations word it:
/// a well-formed ARN naming a resource this service's tagging operations do not
/// act on, quoted as [`malformed_resource_arn_message`] quotes it.
pub fn resource_arn_not_supported_message(resource_arn: &str) -> String {
    format!("The ARN: {resource_arn} is not supported by CodeDeploy")
}

/// `ArnNotSupportedException` (HTTP 400) as the tag-READING operation words it:
/// the same refusal, by the same rule, in the sentence that operation answers.
pub fn resource_arn_not_a_codedeploy_arn_message(resource_arn: &str) -> String {
    format!("Not a CodeDeploy ARN: {resource_arn}")
}

/// `TagRequiredException` (HTTP 400): a tagging call whose request carries no
/// tag member AT ALL.
///
/// It answers an ABSENT member, which is why the boundary
/// ([`crate::service`]'s refusal table) is where it is chosen: a member the
/// request DID carry is a value the call applies, and a list of no tag changes
/// is a call that changes nothing rather than a call that named nothing.
pub const TAG_MISSING: &str = "A tag was not specified";

/// `InvalidTagsToAddException` (HTTP 400): the shape every tag of a tagging
/// request has to have; see [`tag_key_value`] for the rule.
pub const TAGS_TO_ADD_MALFORMED: &str =
    "Each tag must carry both a key and a value, and a key cannot be an empty string";

/// `InvalidTagsToAddException` (HTTP 400): the call would leave the resource
/// carrying more than [`MAX_TAGS_PER_RESOURCE`] tags.
pub fn too_many_tags_message(projected: usize) -> String {
    format!(
        "The maximum number of tags for a resource is {MAX_TAGS_PER_RESOURCE}; this call would \
         leave it with {projected}"
    )
}

/// `InvalidTagsToAddException` (HTTP 400): the shape every entry of a tag-KEY
/// list has to have; see [`tag_key`] for the rule.
pub const TAG_KEY_MALFORMED: &str = "A tag key cannot be an empty string";

/// `InstanceNotRegisteredException` (HTTP 400) as the single-instance READER
/// answers it: the host the request asked about, after the delimiter.
///
/// The shape is "The specified on-premises instance is not registered", which a
/// name that never held a registration and one whose registration has ended are
/// both in, so both are answered with this one sentence.
pub fn host_does_not_exist(instance_name: &str) -> String {
    format!("The requested host does not exist. Host: {instance_name}")
}

/// `InstanceNotRegisteredException` (HTTP 400) as the TAGGING operations answer
/// it: the host whose tags the request asked to change.
///
/// A reader is asked for a host's record and answers that there is none
/// ([`host_does_not_exist`]); a tagging call asks to change a REGISTERED host
/// and answers that this host is not one, so the two sentences differ in what
/// they state about the same name.
pub fn host_not_registered(instance_name: &str) -> String {
    format!("Host {instance_name} is not registered")
}

/// `IamArnRequiredException` (HTTP 400): the fixed sentence for a registration
/// that carried neither an IAM user ARN nor an IAM session ARN.
///
/// It is the sentence of the REFUSED ACTION — the registration could not be made,
/// and what to put in the request to make it — rather than the error shape's
/// documentation text, which describes the shape to a reader of the model and is
/// never what a caller is answered with.
pub const IAM_ARN_MISSING: &str = "The on-premises instance could not be registered because no \
     IAM ARN was included in the request. Include either an IAM session ARN or IAM user ARN in \
     the request, and then try again.";

/// `MultipleIamArnsProvidedException` (HTTP 400): the fixed sentence for a
/// registration that named BOTH an IAM user ARN and an IAM session ARN, where
/// exactly one identifies the instance.
///
/// The sibling of [`IAM_ARN_MISSING`], and worded the same way: the REFUSED
/// ACTION, then what to put in the request to make it. Neither ARN nor the
/// instance name is interpolated — the fault is that two were given, not what
/// either of them was.
pub const MULTIPLE_IAM_ARNS_PROVIDED: &str = "The on-premises instance could not be registered \
     because both an IAM session ARN and an IAM user ARN were included in the request. Include \
     only one IAM ARN in the request, and then try again.";

/// `IamUserArnAlreadyRegisteredException` (HTTP 400): the fixed sentence for a
/// registration whose IAM user ARN already backs a registered instance.
///
/// Worded like [`IAM_ARN_MISSING`] and [`MULTIPLE_IAM_ARNS_PROVIDED`] — the
/// refused action, then what to put in the request to make it — and
/// interpolating nothing: the ARN is the one the caller sent, and WHICH instance
/// holds the binding is this account's state rather than anything about this
/// request, so the refusal states neither.
pub const IAM_USER_ARN_ALREADY_REGISTERED: &str = "The on-premises instance could not be \
     registered because the request included an IAM user ARN that has already been used to \
     register an instance. Include either a different IAM user ARN or IAM session ARN in the \
     request, and then try again.";

/// `IamSessionArnAlreadyRegisteredException` (HTTP 400): the session-ARN half of
/// the same uniqueness rule. See [`IAM_USER_ARN_ALREADY_REGISTERED`].
pub const IAM_SESSION_ARN_ALREADY_REGISTERED: &str = "The on-premises instance could not be \
     registered because the request included an IAM session ARN that has already been used to \
     register a different instance. Include either a different IAM session ARN or IAM user ARN \
     in the request, and then try again.";

/// `InvalidLifecycleEventHookExecutionIdException` (HTTP 400): the fixed
/// sentence for a `lifecycleEventHookExecutionId` that names no hook execution
/// the deployment can take a result for — absent, empty, or an id this
/// deployment never issued are one refusal, so they answer one sentence.
///
/// It interpolates nothing: the id is the one the caller sent, and the
/// deployment it was reported against is the request's own `deploymentId`.
pub const LIFECYCLE_EVENT_HOOK_EXECUTION_ID_INVALID: &str =
    "The lifecycleEventHookExecutionId is invalid";

/// `InstanceNameAlreadyRegisteredException` (HTTP 400): the fixed sentence for a
/// host name a registration already holds, interpolating nothing — the name is
/// the one the request carried, so the refusal does not quote it back.
///
/// It covers both reasons the name is unavailable: a registration holds it now,
/// or one held it recently.
pub const INSTANCE_NAME_ALREADY_REGISTERED: &str =
    "Host name has already been registered or recently deregistered";

/// `InvalidAutoRollbackConfigException` (HTTP 400): the fixed sentence for an
/// automatic rollback that is enabled but names no triggering event, so it could
/// never fire. Shared by every operation that accepts an
/// `autoRollbackConfiguration`.
pub const AUTO_ROLLBACK_WITHOUT_EVENTS: &str =
    "Deployment Groups need to have at least one event specified when auto rollback is enabled";

/// `InvalidEC2TagCombinationException` (HTTP 400): a deployment group selects
/// Amazon EC2 instances by EITHER the flat tag-filter list or the grouped tag
/// set, so a request naming both forms is refused with this sentence.
///
/// The sentence is this error class's own, fixed: it states which two data types
/// the request specified and the remedy, names them as CodeDeploy's prose does,
/// and interpolates nothing — neither tag the request carried is part of why it
/// was refused. Both `CreateDeploymentGroup` and `UpdateDeploymentGroup` raise
/// the shape, so it is worded once, here.
pub const EC2_TAG_COMBINATION: &str =
    "The request specified both Ec2TagFilters and Ec2TagSet, but only one of these data types \
     can be used in a single call. Remove one tag specification type, and then try again.";

/// `InvalidOnPremisesTagCombinationException` (HTTP 400): the on-premises
/// selector's pair of the same rule, in the same sentence over its own two data
/// types. See [`EC2_TAG_COMBINATION`].
pub const ON_PREMISES_TAG_COMBINATION: &str =
    "The request specified both OnPremisesTagFilters and OnPremisesTagSet, but only one of \
     these data types can be used in a single call. Remove one tag specification type, and \
     then try again.";

/// The `DeploymentCreator` value for a deployment a client asked for, as opposed
/// to one an auto-scaling event or a rollback created
/// (`smithy_query(query="shape",shape="DeploymentCreator")`).
pub const DEPLOYMENT_CREATOR_USER: &str = "user";

/// The `DeploymentType` of a blue/green deployment
/// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE | BLUE_GREEN).
pub const DEPLOYMENT_TYPE_BLUE_GREEN: &str = "BLUE_GREEN";

/// The `DeploymentType` a deployment group gets when nothing asked for one
/// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE |
/// BLUE_GREEN) — see [`effective_deployment_style`].
pub const DEPLOYMENT_TYPE_IN_PLACE: &str = "IN_PLACE";

/// The `DeploymentOption` a deployment group gets when nothing asked for one
/// (`smithy_query(query="shape",shape="DeploymentOption")`: WITH_TRAFFIC_CONTROL
/// | WITHOUT_TRAFFIC_CONTROL) — see [`effective_deployment_style`].
pub const DEPLOYMENT_OPTION_WITHOUT_TRAFFIC_CONTROL: &str = "WITHOUT_TRAFFIC_CONTROL";

/// The `OutdatedInstancesStrategy` a deployment group behaves by when the
/// request left the member unset: the model's own documentation for it says "If
/// this option is set to `UPDATE` or is unspecified, CodeDeploy initiates one or
/// more 'auto-update outdated instances' deployments"
/// (`smithy_query(query="doc",shape="DeploymentGroupInfo",member="outdatedInstancesStrategy")`),
/// so an unspecified strategy IS `UPDATE` and the group reports it as such.
pub const OUTDATED_INSTANCES_STRATEGY_UPDATE: &str = "UPDATE";

/// The `InstanceAction` that terminates the original environment's instances
/// after a successful blue/green deployment
/// (`smithy_query(query="shape",shape="InstanceAction")`: TERMINATE |
/// KEEP_ALIVE).
pub const INSTANCE_ACTION_TERMINATE: &str = "TERMINATE";

/// The largest `terminationWaitTimeInMinutes` a deployment group may ask for, as
/// the model documents for the member
/// (`smithy_query(query="doc",shape="BlueInstanceTerminationOption",member="terminationWaitTimeInMinutes")`:
/// "The maximum setting is 2880 minutes (2 days)").
pub const MAX_TERMINATION_WAIT_MINUTES: i32 = 2880;

/// The `DeploymentCreator` value for the deployment CodeDeploy itself creates to
/// roll a deployment back
/// (`smithy_query(query="shape",shape="DeploymentCreator")`).
pub const DEPLOYMENT_CREATOR_ROLLBACK: &str = "codeDeployRollback";

/// The compute platform an application gets when `CreateApplication` omits one:
/// CodeDeploy's own default (`smithy_query(query="shape",shape="ComputePlatform")`
/// lists Server | Lambda | ECS).
pub const DEFAULT_COMPUTE_PLATFORM: &str = "Server";

/// The quotas this emulator enforces, with the modelled error each refusal
/// answers.
///
/// AWS publishes the first two in "Quotas in AWS CodeDeploy" (applications per
/// account per Region; deployment groups per application), so they keep their
/// documented values. Enforcing them is also what keeps the store from growing
/// without limit on repeated requests.
pub const MAX_APPLICATIONS: usize = 1_000;
/// See [`MAX_APPLICATIONS`].
pub const MAX_DEPLOYMENT_GROUPS_PER_APPLICATION: usize = 1_000;

/// How long a `description` this service accepts before answering the modelled
/// `DescriptionTooLongException`, counted in Unicode characters.
///
/// The model puts no `@length` on `Description`
/// (`smithy_query(query="shape",shape="Description")` is a bare string), so the
/// bound is a RUNTIME rule of the service — and the refusal states it: the
/// sentence a description past the bound is answered with
/// ([`description_too_long_message`]) ends in this figure, which is why the
/// number lives here once and is interpolated into the sentence rather than
/// written into it. `RegisterApplicationRevision` and `CreateDeployment` both
/// model `DescriptionTooLongException`, and both apply this bound. Characters,
/// not bytes, so a description of non-ASCII text is measured the way a caller
/// counts it.
pub const MAX_DESCRIPTION_CHARS: usize = 512;

/// Whether `description` is past [`MAX_DESCRIPTION_CHARS`].
pub fn description_too_long(description: &str) -> bool {
    description.chars().count() > MAX_DESCRIPTION_CHARS
}

/// What an operation's `description` is a comment ABOUT, which is what a
/// [`description_too_long_message`] names.
///
/// One variant per operation that carries a `description` and models
/// `DescriptionTooLongException`, each taken from that member's own
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Described {
    /// `RegisterApplicationRevision$description` —
    /// `smithy_query(query="doc",shape="RegisterApplicationRevisionInput",member="description")`:
    /// "A comment about the revision."
    Revision,
    /// `CreateDeployment$description` —
    /// `smithy_query(query="doc",shape="CreateDeploymentInput",member="description")`:
    /// "A comment about the deployment."
    Deployment,
}

impl Described {
    /// How the refusal's sentence names it.
    #[must_use]
    pub fn noun(self) -> &'static str {
        match self {
            Self::Revision => "Revision",
            Self::Deployment => "Deployment",
        }
    }
}

/// The message a `DescriptionTooLongException` carries: the fixed sentence for
/// the described resource, ending in the LIMIT
/// ([`MAX_DESCRIPTION_CHARS`]) — the one machine-readable part of the refusal
/// beyond the error code, so a caller that matches the sentence recovers the
/// bound it has to fit in.
///
/// It quotes NOTHING back from the request: how long the description the caller
/// sent is is something the caller already knows, and a length-of-input clause
/// would report a generic constraint failure where this service models an error
/// of its own.
pub fn description_too_long_message(described: Described) -> String {
    format!(
        "{} description has a maximum length of {MAX_DESCRIPTION_CHARS}",
        described.noun()
    )
}

/// How many deployment configurations one account's store holds, counting the
/// `CodeDeployDefault.*` built-ins.
///
/// CodeDeploy's published quotas state no number this emulator can cite for
/// deployment configurations, so this bound is CHOSEN. It is enforced because it
/// is what keeps the store from growing without limit on repeated
/// `CreateDeploymentConfig` calls, and the model declares the error for it
/// (`DeploymentConfigLimitExceededException`).
pub const MAX_DEPLOYMENT_CONFIGS: usize = 1_000;

/// How many deployments of history the store keeps.
///
/// CodeDeploy publishes no quota for retained deployment history and does not
/// refuse a deployment because its history is full, so this bound is CHOSEN and
/// is applied as RETENTION: creating a deployment past it drops the oldest
/// deployments, oldest first, so a caller cannot grow the store without limit.
pub const MAX_DEPLOYMENT_HISTORY: usize = 500;

/// How many trigger notifications may wait to be published at once; see
/// [`ComAmazonawsCodedeployCodedeploy20141006State::queue_trigger_notifications`].
///
/// EMULATOR-CHOSEN, and only a safety bound: the queue is drained by the publish
/// that follows the pass which filled it, so it holds the notifications of one
/// pass. It bounds a store nothing can publish from — no Amazon SNS mounted
/// beside this service, so no pass ever drains it — at a few lifecycle events for
/// each deployment the history retains.
pub const MAX_QUEUED_TRIGGER_NOTIFICATIONS: usize = 4 * MAX_DEPLOYMENT_HISTORY;

/// How many names or ids one `BatchGet*` request may carry.
///
/// The model states the figure on the refusal itself — every `BatchGet*`
/// operation of this service models `BatchLimitExceededException`, whose own
/// documentation
/// (`smithy_query(query="shape",shape="BatchLimitExceededException")`) reads "The
/// maximum number of names or IDs allowed for this request (100) was exceeded" —
/// so this is the published REQUEST-SIZE quota of the batch readers, and it lives
/// here, once, so no two of them can enforce a different one. The figure each
/// operation's own prose carries ("The maximum number of deployments that can be
/// returned is 25") is about how many records one call RETURNS, not about how
/// many entries a request is accepted with.
pub const MAX_BATCH_SIZE: usize = 100;

/// The message a `BatchLimitExceededException` carries: the request-size quota a
/// batch has to fit in, built from [`MAX_BATCH_SIZE`] so the figure a caller
/// reads back is the one that is enforced.
pub fn batch_limit_exceeded_message() -> String {
    format!("BatchGet size limit is {MAX_BATCH_SIZE}")
}

/// How many alarms the alarm configuration OF A DEPLOYMENT GROUP may carry.
///
/// The model states it on `AlarmConfiguration$alarms`
/// (`smithy_query(query="shape",shape="AlarmConfiguration")`): "A list of alarms
/// configured for the deployment or deployment group. A maximum of 10 alarms can
/// be added", and `AlarmsLimitExceededException` — the refusal past the quota —
/// names the same resource: "The maximum number of alarms for a deployment group
/// (10) was exceeded."
pub const MAX_ALARMS_PER_DEPLOYMENT_GROUP: usize = 10;

/// How many alarms ONE DEPLOYMENT's `overrideAlarmConfiguration` may carry.
///
/// A deployment's override is a different resource from the group's own
/// configuration, with its own quota: the deployment-group figure the model
/// states is scoped to the deployment group by both the member's documentation
/// and `AlarmsLimitExceededException`'s own sentence, so it does not bound what a
/// deployment may override its group's configuration with, which is the higher
/// per-deployment quota here.
pub const MAX_ALARMS_PER_DEPLOYMENT: usize = 50;

/// How many alarms one deployment group's stored alarm configuration KEEPS.
///
/// A group's alarm list is caller-chosen, is replaced whole on every update,
/// and is cloned into every read of the group, while the figure the model
/// documents ([`MAX_ALARMS_PER_DEPLOYMENT_GROUP`]) is not what an update is
/// admitted under ([`AlarmQuota`]). So — like [`MAX_REVISIONS_PER_APPLICATION`]
/// — this bound is CHOSEN rather than published: it is set far above the
/// documented figure, so every configuration the service admits is still
/// admitted, and it is what keeps one group from retaining a list of any length
/// and a read of it from answering one.
pub const MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP: usize = 200;

/// `AlarmsLimitExceededException` (HTTP 400) for a deployment group's own alarm
/// configuration: the error's own sentence, stating the count that refused the
/// request and the count the request named.
fn group_alarms_exceeded(limit: usize, specified: usize) -> String {
    format!(
        "The maximum number of alarms for a deployment group ({limit}) was exceeded: \
         {specified} alarms were specified"
    )
}

/// Which alarm-count gate an operation taking an alarm configuration applies:
/// the one a deployment group is SET UP under, the one an existing group is
/// UPDATED under, or one deployment's `overrideAlarmConfiguration`.
///
/// The three differ in the count a configuration is admitted up to and in the
/// sentence that states it, and in nothing else — every other rule about an
/// alarm configuration is the same for all of them — so
/// [`validate_alarm_configuration`] takes this rather than being written three
/// times.
///
/// A documented quota and an admission gate are NOT the same thing: the figure
/// the model documents bounds the resource, and whether a count past it is
/// refused as the request arrives is per operation
/// ([`AlarmQuota::too_many_alarms`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmQuota {
    /// The alarm configuration a deployment group is created with
    /// ([`MAX_ALARMS_PER_DEPLOYMENT_GROUP`]).
    DeploymentGroup,
    /// The alarm configuration an EXISTING deployment group is updated to,
    /// which is admitted past the documented figure and bounded only by what
    /// the store KEEPS ([`MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP`]): see
    /// [`AlarmQuota::too_many_alarms`].
    DeploymentGroupUpdate,
    /// One deployment's override of its group's configuration
    /// ([`MAX_ALARMS_PER_DEPLOYMENT`]).
    Deployment,
}

impl AlarmQuota {
    /// The `AlarmsLimitExceededException` sentence a configuration of
    /// `specified` alarms is refused at admission with, or `None` where this
    /// gate admits it.
    ///
    /// A sentence is built from the constant it states, so the figure a caller
    /// reads back is the one that refused the request. A configuration an
    /// existing deployment group is updated to is admitted past the documented
    /// figure — that figure bounds the group's alarm monitoring rather than the
    /// writes of its configuration, so the update stores every well-formed
    /// entry it names instead of refusing the request and leaving the group on
    /// its previous alarm set — up to what one group KEEPS
    /// ([`MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP`]), which is the store's own
    /// bound and is stated by the same sentence.
    pub fn too_many_alarms(self, specified: usize) -> Option<String> {
        match self {
            Self::DeploymentGroup if specified > MAX_ALARMS_PER_DEPLOYMENT_GROUP => Some(
                group_alarms_exceeded(MAX_ALARMS_PER_DEPLOYMENT_GROUP, specified),
            ),
            Self::DeploymentGroupUpdate
                if specified > MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP =>
            {
                Some(group_alarms_exceeded(
                    MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP,
                    specified,
                ))
            }
            Self::Deployment if specified > MAX_ALARMS_PER_DEPLOYMENT => Some(format!(
                "Deployments cannot contain more than {MAX_ALARMS_PER_DEPLOYMENT} alarms."
            )),
            _ => None,
        }
    }
}

/// The longest alarm name an alarm configuration may carry, from the model's
/// `Alarm$name` (`smithy_query(query="shape",shape="Alarm")`): "Maximum length
/// is 255 characters. Each alarm name can be used only once in a list of
/// alarms."
pub const MAX_ALARM_NAME_LENGTH: usize = 255;

/// Why an alarm configuration cannot be accepted, as
/// [`validate_alarm_configuration`] answers it.
///
/// Two distinct refusals, because CodeDeploy models two: too many alarms is
/// `AlarmsLimitExceededException`, anything else about the configuration's form
/// is `InvalidAlarmConfigException`. Each caller maps these onto its OWN
/// operation's error enum.
#[derive(Debug)]
pub enum AlarmConfigurationRejection {
    /// More alarms than the operation's [`AlarmQuota`] admits; carries the
    /// sentence that states the count the request has to fit in.
    TooManyAlarms(String),
    /// Malformed in one of the ways `InvalidAlarmConfigException` documents; the
    /// string is the whole sentence, so the caller can fix the request from the
    /// message.
    Malformed(String),
}

/// Check an alarm configuration against the rules the model and the two alarm
/// errors state, so every operation that accepts one refuses the same requests.
///
/// `InvalidAlarmConfigException` documents the malformed cases: "The alarm name
/// is empty or null or exceeds the limit of 255 characters. Two alarms with the
/// same name have been specified. The alarm configuration is enabled, but the
/// alarm list is empty." A DISABLED configuration with no alarms is accepted —
/// that is how a caller turns alarm monitoring off.
pub fn validate_alarm_configuration(
    configuration: &AlarmConfiguration,
    quota: AlarmQuota,
) -> Result<(), AlarmConfigurationRejection> {
    let alarms = configuration.alarms.as_deref().unwrap_or_default();
    // The COUNT is gated only where the operation gates it; the rules below are
    // about the configuration's FORM and hold wherever one is accepted.
    if let Some(message) = quota.too_many_alarms(alarms.len()) {
        return Err(AlarmConfigurationRejection::TooManyAlarms(message));
    }
    if configuration.enabled && alarms.is_empty() {
        return Err(AlarmConfigurationRejection::Malformed(
            ALARM_CONFIGURATION_WITHOUT_ALARMS.to_string(),
        ));
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (index, alarm) in alarms.iter().enumerate() {
        // WHICH entry of the submitted list is at fault, as the caller numbers
        // them: a refusal about one entry of a list names its 1-based position,
        // which is the only thing that says which entry to fix when several
        // carry the same name or several names collide.
        let position = index + 1;
        let name = alarm.name.as_deref().unwrap_or_default();
        if name.is_empty() {
            return Err(AlarmConfigurationRejection::Malformed(
                invalid_alarm_configuration(&format!(
                    "alarm {position} was specified with no name"
                )),
            ));
        }
        if name.chars().count() > MAX_ALARM_NAME_LENGTH {
            return Err(AlarmConfigurationRejection::Malformed(
                invalid_alarm_configuration(&format!(
                    "the name of alarm {position} exceeds the maximum length of \
                     {MAX_ALARM_NAME_LENGTH} characters"
                )),
            ));
        }
        if !seen.insert(name) {
            return Err(AlarmConfigurationRejection::Malformed(duplicate_alarm_name(
                name, position,
            )));
        }
    }
    Ok(())
}

/// `InvalidAlarmConfigException` (HTTP 400) for the case the shape documents as
/// "Two alarms with the same name have been specified": the repeated NAME and
/// the 1-based POSITION, in the alarm list the request submitted, of the entry
/// that repeats it — the entry a caller has to drop.
fn duplicate_alarm_name(name: &str, position: usize) -> String {
    format!("Name {name} of Alarm {position} already exists.")
}

/// `InvalidAlarmConfigException` (HTTP 400): the fixed sentence for an alarm
/// configuration that is monitored but lists no alarm, which is the case the
/// shape documents as "The alarm configuration is enabled, but the alarm list is
/// empty".
pub const ALARM_CONFIGURATION_WITHOUT_ALARMS: &str =
    "Deployment Groups need to have at least one alarms attached if they are monitored.";

/// `InvalidAlarmConfigException` (HTTP 400) for every OTHER way the shape
/// documents an alarm configuration as malformed (a nameless alarm, an over-long
/// name): the error's own clause with `detail` saying which, so the caller can
/// fix the request from the message. `detail` names the 1-based position of the
/// offending entry, as [`duplicate_alarm_name`] does.
fn invalid_alarm_configuration(detail: &str) -> String {
    format!("The alarm configuration is invalid: {detail}")
}

/// How many tag groups one `ec2TagSet` / `onPremisesTagSet` may hold, the bound
/// `TagSetListLimitExceededException` answers.
///
/// The model states the figure: that error is "The number of tag groups included
/// in the tag set list exceeded the maximum allowed limit of 3"
/// (`smithy_query(query="shape",shape="TagSetListLimitExceededException")`).
pub const MAX_TAG_SET_GROUPS: usize = 3;

/// How many tag filters ONE selector list of a deployment group may hold —
/// `ec2TagFilters`, `onPremisesInstanceTagFilters`, or one group of either tag
/// set.
///
/// CodeDeploy states the figure for a tag type in
/// `InvalidTargetInstancesException` ("The limit of 10 tags for a tag type was
/// exceeded"). Enforcing it is also what bounds the work of resolving a
/// deployment's targets: every filter a group holds is matched against the tags
/// of every registered on-premises instance, so an unbounded filter list would
/// be unbounded work on each `CreateDeployment`.
pub const MAX_TAG_FILTERS_PER_LIST: usize = 10;

/// The refusal message for a tag filter list longer than this service accepts:
/// it names the member and the limit, which is what a caller has to fix.
///
/// Shared by every operation that takes a caller-chosen tag filter list — the
/// deployment-group selectors and `ListOnPremisesInstances`'s `tagFilters` — so
/// one bound is stated once.
pub fn too_many_tag_filters(member: &str) -> String {
    format!(
        "The number of tag filters in {member} exceeded the maximum allowed limit of {}",
        MAX_TAG_FILTERS_PER_LIST
    )
}

/// How many application revisions one application's registry keeps.
///
/// CodeDeploy publishes no quota for retained revisions and does not refuse a
/// registration because the registry is full, so — like
/// [`MAX_DEPLOYMENT_HISTORY`] — this bound is CHOSEN and is applied as
/// RETENTION: registering past it drops the oldest-registered revisions first,
/// so a caller cannot grow the store without limit.
pub const MAX_REVISIONS_PER_APPLICATION: usize = 500;

/// How many on-premises instance names one call may name, the bound
/// `InstanceLimitExceededException` ("The maximum number of allowed on-premises
/// instances in a single call was exceeded") answers.
///
/// CodeDeploy's API reference states a 100-name limit for the sibling batch
/// call over the same resource (`BatchGetOnPremisesInstances`), so the same 100
/// is applied here; for THIS call the number is CHOSEN to agree with that batch
/// limit, and enforcing it keeps one call's work bounded.
pub const MAX_INSTANCE_NAMES_PER_CALL: usize = 100;

/// How many tags one resource may carry.
///
/// CodeDeploy publishes no per-resource tag count this emulator can cite, so
/// this bound is CHOSEN at the 50 tags per resource AWS's own tagging guidance
/// uses; enforcing it is also what keeps a stored record from growing without
/// limit on repeated tagging calls.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many tags one on-premises instance may carry, the bound
/// `TagLimitExceededException` answers: [`MAX_TAGS_PER_RESOURCE`], the same
/// guidance applied to an instance, so the two tagging paths cannot disagree
/// about how many tags a resource holds.
pub const MAX_TAGS_PER_ON_PREMISES_INSTANCE: usize = MAX_TAGS_PER_RESOURCE;

/// The `deploymentStyle` a deployment group HAS, given what a request asked for.
///
/// A deployment group always runs one type of deployment with one traffic
/// option, so the member is never unset on a stored group: a request that
/// names neither gets CodeDeploy's own default — an in-place deployment with no
/// traffic control — and a request that names only one member gets the default
/// for the other. Materialised when the group is written rather than when it is
/// read, so every operation that acts on the group (a deployment resolving its
/// style, the blue/green waits) sees the same effective value the reader
/// reports.
pub fn effective_deployment_style(requested: Option<DeploymentStyle>) -> Option<DeploymentStyle> {
    let requested = requested.unwrap_or_default();
    Some(DeploymentStyle {
        deployment_type: Some(
            requested
                .deployment_type
                .unwrap_or_else(|| DEPLOYMENT_TYPE_IN_PLACE.to_string()),
        ),
        deployment_option: Some(requested.deployment_option.unwrap_or_else(|| {
            DEPLOYMENT_OPTION_WITHOUT_TRAFFIC_CONTROL.to_string()
        })),
    })
}

/// The `outdatedInstancesStrategy` a deployment group HAS, given what a request
/// asked for: see [`OUTDATED_INSTANCES_STRATEGY_UPDATE`] for why an unspecified
/// strategy is `UPDATE`, and [`effective_deployment_style`] for why the default
/// is materialised at write time.
pub fn effective_outdated_instances_strategy(requested: Option<String>) -> Option<String> {
    Some(requested.unwrap_or_else(|| OUTDATED_INSTANCES_STRATEGY_UPDATE.to_string()))
}

/// The bake time a deployment group that configured none runs with: no wait at
/// all. Every group reports a bake time, so a group with nothing configured
/// reports this rather than leaving the member out.
pub const NO_BAKE_TIME: i32 = 0;

/// What a deployment group reports as the execution role of its managed hooks
/// when it carries no such role: the sentinel CodeDeploy answers with, rather
/// than an absent member or an empty string.
pub const MANAGED_HOOK_EXECUTION_ROLE_NONE: &str = "NONE";

/// A tag's key and value, or `None` when the tag is not one CodeDeploy accepts.
///
/// The model states the rule for the tags of an on-premises instance: "Keys and
/// values are both required. Keys cannot be null or empty strings. Value-only
/// tags are not allowed."
/// (`smithy_query(query="doc",shape="AddTagsToOnPremisesInstancesInput",member="tags")`),
/// so a tag missing either member, or carrying an empty key, is refused with the
/// operation's `InvalidTagException`. An empty VALUE is accepted: only keys are
/// documented as unable to be empty.
pub fn tag_key_value(tag: &Tag) -> Option<(&str, &str)> {
    let key = tag.key.as_deref()?;
    let value = tag.value.as_deref()?;
    (!key.is_empty()).then_some((key, value))
}

/// `key` when it is a tag key CodeDeploy accepts, `None` otherwise.
///
/// A list of tag KEYS carries the same keys a list of tags carries, so the rule
/// [`tag_key_value`] applies to a tag's key applies to each entry of such a
/// list: a key cannot be an empty string. An empty entry therefore identifies no
/// tag of any resource, and is refused rather than silently skipped.
pub fn tag_key(key: &str) -> Option<&str> {
    (!key.is_empty()).then_some(key)
}

/// How many on-premises instance records the store keeps.
///
/// CodeDeploy publishes no quota for registered on-premises instances, so this
/// bound is CHOSEN, and it is what keeps a loop of registrations from growing
/// the store without limit. It is applied first as RETENTION — a record that is
/// no longer REGISTERED is dropped, oldest first, to make room for a new
/// registration — and only a registry whose every record is still registered
/// refuses a further registration, which is the one case no eviction can serve.
pub const MAX_ON_PREMISES_INSTANCES: usize = 1_000;

/// A registered application, with the deployment groups and the application
/// revisions that belong to it.
#[derive(Debug, Clone)]
pub struct Application {
    pub application_id: String,
    pub application_name: String,
    pub compute_platform: String,
    pub create_time_ms: i64,
    pub linked_to_git_hub: bool,
    pub git_hub_account_name: Option<String>,
    pub tags: Vec<Tag>,
    pub deployment_groups: HashMap<String, DeploymentGroup>,
    /// Creation order of this application's deployment group names, so a listing
    /// of them is deterministic and pages stably.
    pub deployment_group_order: Vec<String>,
    /// The revisions CodeDeploy knows for this application, by
    /// [`revision_key`]. CodeDeploy tracks revisions by REGISTRATION — it never
    /// probes the location — so a revision is here only because
    /// `RegisterApplicationRevision` registered it or a deployment used it.
    pub revisions: HashMap<String, ApplicationRevision>,
    /// Registration order of this application's revision keys, so a listing is
    /// deterministic and pages stably.
    pub revision_order: Vec<String>,
}

/// One revision of an application, as CodeDeploy's `GenericRevisionInfo`
/// reports it.
#[derive(Debug, Clone)]
pub struct ApplicationRevision {
    /// The location as the caller supplied it, carrying the digest of its inline
    /// content ([`revision_digest`]): nothing else about a revision is inferred
    /// into the stored record.
    pub revision: RevisionLocation,
    pub description: Option<String>,
    pub register_time_ms: i64,
    /// When a deployment first used this revision, and when it last did —
    /// `None` for a revision that was registered but never deployed.
    pub first_used_time_ms: Option<i64>,
    /// See [`ApplicationRevision::first_used_time_ms`].
    pub last_used_time_ms: Option<i64>,
}

impl Application {
    /// Register `revision` with this application, as
    /// `RegisterApplicationRevision` does.
    ///
    /// `registerTime` is when the revision first became known to the
    /// application and is not moved by a later registration of the same
    /// revision — a second registration only carries a new `description`, which
    /// is the member it exists to set.
    pub fn register_revision(
        &mut self,
        key: String,
        revision: RevisionLocation,
        description: Option<String>,
        now_ms: i64,
    ) {
        let entry = self.revision_entry(key, revision, now_ms);
        if description.is_some() {
            entry.description = description;
        }
    }

    /// Record that a deployment used `revision`, which is also how CodeDeploy
    /// learns a revision a deployment named but nobody registered.
    pub fn record_revision_use(
        &mut self,
        key: String,
        revision: RevisionLocation,
        now_ms: i64,
    ) {
        let entry = self.revision_entry(key, revision, now_ms);
        entry.first_used_time_ms = entry.first_used_time_ms.or(Some(now_ms));
        entry.last_used_time_ms = Some(now_ms);
    }

    /// This application's entry for `key`, inserted as newly registered when the
    /// registry does not hold it yet, dropping the oldest-registered revisions
    /// past [`MAX_REVISIONS_PER_APPLICATION`].
    fn revision_entry(
        &mut self,
        key: String,
        revision: RevisionLocation,
        now_ms: i64,
    ) -> &mut ApplicationRevision {
        if !self.revisions.contains_key(&key) {
            self.revisions.insert(
                key.clone(),
                ApplicationRevision {
                    // An inline revision carries the digest of its content,
                    // computed once here so every read of this registry entry
                    // reports it.
                    revision: revision_digest::with_content_digest(revision),
                    description: None,
                    register_time_ms: now_ms,
                    first_used_time_ms: None,
                    last_used_time_ms: None,
                },
            );
            self.revision_order.push(key.clone());
            if let Some(excess) = self
                .revision_order
                .len()
                .checked_sub(MAX_REVISIONS_PER_APPLICATION)
            {
                for oldest in self.revision_order.drain(..excess).collect::<Vec<_>>() {
                    self.revisions.remove(&oldest);
                }
            }
        }
        self.revisions
            .get_mut(&key)
            .expect("the entry was just inserted")
    }
}

/// A deployment group: every member `CreateDeploymentGroup` accepts, so the
/// group can be read back without losing any of them.
#[derive(Debug, Clone)]
pub struct DeploymentGroup {
    pub deployment_group_id: String,
    pub deployment_group_name: String,
    pub application_name: String,
    pub service_role_arn: String,
    pub deployment_config_name: String,
    pub compute_platform: String,
    pub ec2_tag_filters: Vec<EC2TagFilter>,
    pub on_premises_instance_tag_filters: Vec<TagFilter>,
    pub auto_scaling_groups: Vec<String>,
    pub trigger_configurations: Vec<TriggerConfig>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    pub auto_rollback_configuration: Option<AutoRollbackConfiguration>,
    pub outdated_instances_strategy: Option<String>,
    pub deployment_style: Option<DeploymentStyle>,
    pub blue_green_deployment_configuration: Option<BlueGreenDeploymentConfiguration>,
    pub load_balancer_info: Option<LoadBalancerInfo>,
    pub ec2_tag_set: Option<EC2TagSet>,
    pub on_premises_tag_set: Option<OnPremisesTagSet>,
    pub ecs_services: Vec<ECSService>,
    pub ec2_deployment_options: Option<EC2DeploymentOptions>,
    pub termination_hook_enabled: bool,
    /// The bake time the group runs with, in the units the reader reports it in.
    /// Neither `CreateDeploymentGroup` nor `UpdateDeploymentGroup` takes a
    /// bake-time member in the model this crate is built from, so a group holds
    /// [`NO_BAKE_TIME`] until one of them does.
    pub bake_time: i32,
    /// The execution role the group's managed hooks run under, or `None` for a
    /// group that has none — which is every group here, since no operation in
    /// this model revision takes one.
    pub managed_hook_execution_role_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub create_time_ms: i64,
    /// The Region the group was created in ([`request_region`]), which is the
    /// Region its triggers' notifications report ([`trigger_notifications`]).
    pub region: String,
}

/// A deployment. `revision` is held as the request supplied it, carrying the
/// digest of its inline content ([`revision_digest`]): nothing else about a
/// revision is inferred into the stored record, so reading a deployment back
/// returns the caller's own revision.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_id: String,
    pub application_name: String,
    pub deployment_group_name: Option<String>,
    /// The EFFECTIVE configuration: the request's, else the group's.
    pub deployment_config_name: String,
    pub revision: Option<RevisionLocation>,
    /// The identity of `revision` ([`revision_key`]), computed ONCE when the
    /// deployment was recorded. Deriving a group's target revision reads this
    /// rather than re-deriving it, so no later request re-hashes an inline
    /// revision's content.
    pub revision_key: Option<String>,
    pub previous_revision: Option<RevisionLocation>,
    pub status: String,
    pub creator: String,
    pub description: Option<String>,
    pub ignore_application_stop_failures: bool,
    pub update_outdated_instances_only: bool,
    pub file_exists_behavior: Option<String>,
    pub deployment_mode: Option<String>,
    pub auto_rollback_configuration: Option<AutoRollbackConfiguration>,
    pub target_instances: Option<TargetInstances>,
    pub override_alarm_configuration: Option<AlarmConfiguration>,
    pub ec2_deployment_options: Option<EC2DeploymentOptions>,
    pub deployment_style: Option<DeploymentStyle>,
    pub compute_platform: String,
    pub external_id: Option<String>,
    /// The rollback this deployment was given, once a caller stopped it asking
    /// for one: the rollback deployment's id when one was created, and the
    /// reason when there was nothing to roll back to.
    pub rollback_info: Option<RollbackInfo>,
    pub create_time_ms: i64,
    pub start_time_ms: Option<i64>,
    pub complete_time_ms: Option<i64>,
    /// The instants the clock owes this deployment its start and its terminal
    /// status; see [`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`].
    pub starts_at_ms: i64,
    /// See [`Deployment::starts_at_ms`].
    pub completes_at_ms: i64,
    /// Why this deployment failed, when it did, as CodeDeploy reports it on
    /// `DeploymentInfo.errorInformation`.
    pub error_information: Option<crate::ops::types::ErrorInformation>,
    /// The results reported for this deployment's lifecycle event hooks, in the
    /// order they were reported.
    pub lifecycle_hook_executions: Vec<LifecycleEventHookExecution>,
    /// The lifecycle event hooks this deployment RUNS, read ONCE out of its
    /// revision's AppSpec when the deployment was recorded, in the order its
    /// compute platform runs them.
    ///
    /// Each is a Lambda validation function this service invokes and waits for;
    /// which is what holds the deployment non-terminal, and the traffic shift
    /// back, until the function reports (see [`lifecycle_hook`]). Empty for a
    /// deployment whose revision declares no hook, and for every platform whose
    /// lifecycle events the CodeDeploy agent runs.
    pub hook_executions: Vec<lifecycle_hook::HookExecution>,
    /// The deployment targets the deployment group resolved for this deployment,
    /// in resolution order, as they were resolved WHEN it was created.
    ///
    /// A deployment deploys to the fleet its group selected at the moment it
    /// started, so this is a SNAPSHOT: deregistering an instance or retargeting
    /// the group afterwards cannot change which targets a past deployment went
    /// to. How many there are is also what decides the deployment's terminal
    /// status; see [`deployment_targets`], [`Deployment::target_count`] and
    /// [`terminal_status`].
    pub targets: Vec<DeploymentTargetSnapshot>,
    /// Whether this deployment terminates the instances of the original (blue)
    /// environment after traffic routing: true only for a blue/green deployment
    /// whose group chose the `TERMINATE` action, which is also exactly when the
    /// model says `instanceTerminationWaitTimeStarted` is reported
    /// (`smithy_query(query="doc",shape="DeploymentInfo",member="instanceTerminationWaitTimeStarted")`:
    /// "'false' if the KEEP_ALIVE option is specified").
    pub terminates_original_instances: bool,
    /// How much of the group's `terminationWaitTimeInMinutes` this deployment
    /// still owes before it terminates those instances and completes.
    ///
    /// Subtracting it from [`Deployment::completes_at_ms`] gives the instant
    /// traffic routing is complete, which is when the wait starts;
    /// `SkipWaitTimeForInstanceTermination` sets it to zero, which is what
    /// "overrides any specified wait time" means.
    pub termination_wait_millis: i64,
    /// Whether the termination wait has started, materialised by the clock; see
    /// [`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`].
    pub instance_termination_wait_time_started: bool,
    /// The traffic shift this deployment performs, read ONCE out of its revision
    /// when the deployment was recorded; `None` for a deployment whose revision
    /// declares none (see [`appspec::declared_shift`]).
    pub lambda_traffic_shift: Option<appspec::LambdaTrafficShift>,
    /// Whether this deployment still owes the FETCH of a revision that keeps its
    /// AppSpec outside the request — the `S3` revision form (see
    /// [`s3_revision`]). The fetch falls due when the deployment starts, and what
    /// the fetched object declares is what the deployment then performs; a
    /// deployment whose revision carries its content inline owes none.
    pub awaits_revision_fetch: bool,
    /// Whether the traffic this deployment had already shifted still owes its
    /// RESTORATION to the version it was shifting from.
    ///
    /// Set where a deployment is rolled back ([`rollback::record_rollback`]) and
    /// carried out where the alias is written
    /// ([`lambda_traffic_shift::apply_due_shifts`]): a rollback of a deployment
    /// that had moved some of the traffic puts the alias back the way that
    /// deployment found it, whether or not the group also had a revision to
    /// create a rollback DEPLOYMENT of.
    pub awaits_alias_restore: bool,
    /// How far that shift has got, which is also what keeps the clock from
    /// settling a deployment whose shift has not happened yet; see
    /// [`lambda_traffic_shift::TrafficShiftProgress`].
    pub traffic_shift: lambda_traffic_shift::TrafficShiftProgress,
    /// The increments that shift is carried out in, in the order they fall due:
    /// what the deployment's resolved `trafficRoutingConfig` describes (see
    /// [`traffic_shift::increments`]). The last of them is the whole move, so a
    /// deployment that shifts all its traffic at once carries exactly one.
    pub traffic_increments: Vec<traffic_shift::Increment>,
    /// How many of those increments have been carried out on the alias.
    pub traffic_increments_applied: usize,
    /// The Region the deployment was created in ([`request_region`]), which is
    /// the Region its group's trigger notifications report it in
    /// ([`trigger_notifications`]).
    pub region: String,
    /// The trigger events of this deployment that have already been queued for
    /// publication, so one lifecycle event is published once however many passes
    /// run over the record; see
    /// [`ComAmazonawsCodedeployCodedeploy20141006State::queue_due_trigger_notifications`].
    pub published_trigger_events: Vec<&'static str>,
}

/// The result a validation function reported for one of a deployment's
/// lifecycle event hooks, through `PutLifecycleEventHookExecutionStatus`.
///
/// The execution id is the hook execution's IDENTITY: CodeDeploy hands it to the
/// validation function in the lifecycle event, and the function reports back
/// under it, so a second report under the same id is a second report of the same
/// lifecycle event.
#[derive(Debug, Clone)]
pub struct LifecycleEventHookExecution {
    pub lifecycle_event_hook_execution_id: String,
    /// Which of the deployment's lifecycle hooks this execution reported for, in
    /// the order its compute platform runs them.
    pub lifecycle_event_name: String,
    /// A `LifecycleEventStatus`: see [`LIFECYCLE_EVENT_STATUS_SUCCEEDED`].
    pub status: String,
    pub update_time_ms: i64,
}

/// One deployment target, as the deployment that resolved it holds it.
///
/// The id is the one CodeDeploy reports for a target of that compute platform,
/// which the model states per platform
/// (`smithy_query(query="doc",shape="BatchGetDeploymentTargetsInput",member="targetIds")`):
/// an EC2/on-premises instance id, a Lambda function name, or an Amazon ECS
/// `<clustername>:<servicename>` pair.
#[derive(Debug, Clone)]
pub struct DeploymentTargetSnapshot {
    /// The id this target is reported and looked up under.
    ///
    /// `None` for a target this emulator resolves but cannot NAME: a Lambda
    /// deployment's target is the function its revision's AppSpec names, and
    /// CodeDeploy's own state holds no revision content for this emulator to
    /// read it from (the same gap [`deployment_targets`] documents for EC2
    /// inventory). Such a target still counts towards the deployment's outcome;
    /// it is simply not one a client can list or fetch by id.
    pub target_id: Option<String>,
    /// A `DeploymentTargetType` (`smithy_query(query="shape",shape="DeploymentTargetType")`:
    /// InstanceTarget | LambdaTarget | ECSTarget | CloudFormationTarget).
    pub target_type: String,
    /// The `TargetLabel` this target carries, for a target of an EC2/on-premises
    /// deployment that HAS one; see [`deployment_targets`].
    pub instance_label: Option<String>,
}

impl Deployment {
    /// How many targets this deployment resolved, which is what decides its
    /// terminal status ([`terminal_status`]).
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Whether this deployment has STARTED, which is what
    /// `DeploymentNotStartedException` refuses an operation over its targets
    /// for: a deployment the clock has not yet owed its start
    /// ([`deployment_start_due`]) has deployed to nothing, so it has no target
    /// state to report.
    pub fn is_started(&self) -> bool {
        self.start_time_ms.is_some()
    }

    /// The instant this deployment's traffic routing is complete, which is when
    /// a termination wait starts and from which it is measured.
    pub fn traffic_routing_complete_at_ms(&self) -> i64 {
        self.completes_at_ms - self.termination_wait_millis
    }

    /// Whether this deployment has reached a terminal status, by the one
    /// definition of terminal this service has ([`deployment_is_terminal`]).
    pub fn is_complete(&self) -> bool {
        deployment_is_terminal(&self.status)
    }

    /// Whether this deployment is a blue/green one, which is what
    /// `SkipWaitTimeForInstanceTermination` and `ContinueDeployment` act on
    /// (`smithy_query(query="shape",shape="DeploymentType")`: IN_PLACE |
    /// BLUE_GREEN).
    pub fn is_blue_green(&self) -> bool {
        self.deployment_type() == DEPLOYMENT_TYPE_BLUE_GREEN
    }

    /// The `DeploymentType` this deployment runs as, which is the one its
    /// deployment group's effective `deploymentStyle` carries
    /// ([`effective_deployment_style`] materialises that member when the group is
    /// written, so a stored deployment always has one). A deployment that
    /// somehow carries no style at all is CodeDeploy's own default, an in-place
    /// deployment — stated rather than left absent, so an operation that refuses
    /// a type can always name it.
    pub fn deployment_type(&self) -> &str {
        self.deployment_style
            .as_ref()
            .and_then(|style| style.deployment_type.as_deref())
            .unwrap_or(DEPLOYMENT_TYPE_IN_PLACE)
    }
}

/// A deployment configuration — one of the `CodeDeployDefault.*` built-ins, or
/// one a client created.
#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub deployment_config_id: String,
    pub deployment_config_name: String,
    pub minimum_healthy_hosts: Option<MinimumHealthyHosts>,
    pub traffic_routing_config: Option<TrafficRoutingConfig>,
    pub zonal_config: Option<ZonalConfig>,
    pub compute_platform: String,
    pub create_time_ms: i64,
}

/// The `RegistrationStatus` of an on-premises instance that is still
/// registered, one of the two values the model's `RegistrationStatus` enum
/// carries (the generated `crate::ops::types::REGISTRATION_STATUS_VALUES`:
/// Deregistered | Registered).
pub const REGISTRATION_STATUS_REGISTERED: &str = "Registered";
/// See [`REGISTRATION_STATUS_REGISTERED`]: the status of an instance whose
/// registration has ended, whose record CodeDeploy keeps.
pub const REGISTRATION_STATUS_DEREGISTERED: &str = "Deregistered";

/// A registered on-premises instance.
///
/// The record OUTLIVES its registration: CodeDeploy keeps a deregistered
/// instance, with the instant it was deregistered, which is what lets
/// `ListOnPremisesInstances` answer the `Deregistered` registration status and a
/// batch read report a `deregisterTime`. The single-instance reader
/// (`GetOnPremisesInstance`) reports only an instance that is still registered:
/// a name whose registration is gone is its `InstanceNotRegisteredException`.
#[derive(Debug, Clone)]
pub struct OnPremisesInstance {
    pub instance_name: String,
    pub instance_arn: String,
    /// The ONE IAM ARN the registration was made with: an instance carries an
    /// IAM user ARN or an IAM session ARN, never both.
    pub iam_user_arn: Option<String>,
    /// See [`OnPremisesInstance::iam_user_arn`].
    pub iam_session_arn: Option<String>,
    pub register_time_ms: i64,
    /// `None` while the instance is registered; the instant it was deregistered
    /// once it is not.
    pub deregister_time_ms: Option<i64>,
    pub tags: Vec<Tag>,
}

impl OnPremisesInstance {
    /// Whether this instance is currently registered, as the model's
    /// `RegistrationStatus` (`smithy_query(query="shape",shape="RegistrationStatus")`:
    /// Registered | Deregistered) distinguishes the two.
    pub fn is_registered(&self) -> bool {
        self.deregister_time_ms.is_none()
    }

    /// How many tags this instance would carry once `tags` were added: a tag
    /// REPLACES the value of a key the instance already carries rather than
    /// adding a second entry for it, so only genuinely new keys count.
    ///
    /// Set-keyed rather than list-scanned, so the cost is linear in the number
    /// of keys rather than quadratic in it; both sides are bounded anyway
    /// ([`MAX_TAGS_PER_ON_PREMISES_INSTANCE`] stored, and the same bound on a
    /// request's tag list, which a tagging request is refused against BEFORE
    /// this runs).
    pub fn projected_tag_count(&self, tags: &[(&str, &str)]) -> usize {
        projected_tag_count(&self.tags, tags)
    }

    /// Drop the tag `key` holding exactly `value`, answering whether the
    /// instance carried it.
    ///
    /// A tag is identified by the PAIR the caller named — "the tag key-value
    /// pairs to remove"
    /// (`smithy_query(query="doc",shape="RemoveTagsFromOnPremisesInstancesInput",member="tags")`)
    /// — so a pair whose value is not the one the instance holds for that key
    /// names no tag of this instance and removes nothing. Matching is over the
    /// stored bytes, so a key or a value outside ASCII is removed by the same
    /// rule as any other.
    pub fn remove_tag(&mut self, key: &str, value: &str) -> bool {
        let before = self.tags.len();
        self.tags
            .retain(|tag| !(tag.key.as_deref() == Some(key) && tag.value.as_deref() == Some(value)));
        self.tags.len() != before
    }

    /// Set `key` to `value`, replacing the value of a key already present so an
    /// instance never carries two entries for one key.
    pub fn set_tag(&mut self, key: &str, value: &str) {
        set_tag(&mut self.tags, key, value);
    }
}

/// How many tags a record carrying `tags` would carry once `added` were applied:
/// a tag REPLACES the value of a key the record already carries rather than
/// adding a second entry for it, so only genuinely new keys count.
///
/// Set-keyed rather than list-scanned, so the cost is linear in the number of
/// keys rather than quadratic in it; both sides are bounded anyway
/// ([`MAX_TAGS_PER_RESOURCE`] stored, and the same bound on a request's tag
/// list, which a tagging request is refused against BEFORE this runs).
pub fn projected_tag_count(tags: &[Tag], added: &[(&str, &str)]) -> usize {
    let known: HashSet<&str> = tags.iter().filter_map(|tag| tag.key.as_deref()).collect();
    let new_keys: HashSet<&str> = added
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| !known.contains(key))
        .collect();
    tags.len() + new_keys.len()
}

/// Set `key` to `value` in `tags`, replacing the value a key already present
/// holds so no record carries two entries for one key.
///
/// The one rule every tagging operation of this service writes through, so the
/// tags of an on-premises instance and the tags of an application or deployment
/// group are merged the same way.
pub fn set_tag(tags: &mut Vec<Tag>, key: &str, value: &str) {
    match tags.iter_mut().find(|tag| tag.key.as_deref() == Some(key)) {
        Some(existing) => existing.value = Some(value.to_string()),
        None => tags.push(Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }),
    }
}

/// Drop the tag `key` from `tags`, whatever value it holds, answering whether
/// the record carried it.
///
/// A tag of a resource is identified here by its KEY ALONE — "The Tag objects
/// identified by the keys are disassociated from the resource"
/// (`smithy_query(query="doc",shape="UntagResourceInput",member="TagKeys")`) —
/// which is what distinguishes this from [`OnPremisesInstance::remove_tag`],
/// where the caller names the key-value PAIR to remove. A key the record does
/// not carry names no tag of it, so it removes nothing; the removal is
/// therefore idempotent and matching is over the stored bytes, so a key outside
/// ASCII is removed by the same rule as any other.
pub fn remove_tag_key(tags: &mut Vec<Tag>, key: &str) -> bool {
    let before = tags.len();
    tags.retain(|tag| tag.key.as_deref() != Some(key));
    tags.len() != before
}

/// Per-service state for com-amazonaws-codedeploy-codedeploy-20141006. A handler reaches it through [`with_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodedeployCodedeploy20141006State {
    /// Applications by `applicationName`.
    pub applications: HashMap<String, Application>,
    /// Creation order of application names, so `ListApplications` is
    /// deterministic and pages stably.
    pub application_order: Vec<String>,
    /// Deployments by `deploymentId`.
    pub deployments: HashMap<String, Deployment>,
    /// Creation order of deployment ids, so `ListDeployments` can answer
    /// newest-first deterministically and page stably.
    pub deployment_order: Vec<String>,
    /// The deployments the clock still owes a transition, creation-ordered. Only
    /// these are walked per request, so the cost of materialising what the clock
    /// owes does not grow with the deployment history.
    pending_deployments: Vec<String>,
    /// Deployment configurations by `deploymentConfigName`.
    pub deployment_configs: HashMap<String, DeploymentConfig>,
    /// The order the configurations entered the store — the `CodeDeployDefault.*`
    /// built-ins first, then each one a client created — so
    /// `ListDeploymentConfigs` is deterministic and pages stably.
    ///
    /// Maintained alongside [`Self::deployment_configs`] by
    /// [`Self::insert_deployment_config`], so the index can never name a
    /// configuration the map does not carry.
    pub deployment_config_order: Vec<String>,
    /// The names of this account's stored connections to GitHub accounts, in the
    /// order they were linked, so `ListGitHubAccountTokenNames` is deterministic
    /// and pages stably.
    ///
    /// EMPTY on a fresh store, and that is the faithful answer: CodeDeploy's API
    /// models no operation that CREATES a GitHub account connection — the
    /// connection is made by the GitHub OAuth handshake the CodeDeploy console
    /// drives, which this emulator cannot perform — so an account that has run no
    /// console handshake has no stored connection names. The two modelled
    /// operations over this store only READ it (`ListGitHubAccountTokenNames`) and
    /// REMOVE from it (`DeleteGitHubAccountToken`).
    pub git_hub_account_token_names: Vec<String>,
    /// On-premises instances by `instanceName`, registered and deregistered
    /// alike — a deregistered instance keeps its record.
    pub on_premises_instances: HashMap<String, OnPremisesInstance>,
    /// Registration order of on-premises instance names, so
    /// `ListOnPremisesInstances` is deterministic and pages stably.
    pub on_premises_instance_order: Vec<String>,
    /// The name of the instance each IAM ARN currently backs, so the
    /// duplicate-IAM-ARN refusal is a LOOKUP rather than a scan of the whole
    /// registry under the store lock.
    ///
    /// It carries an entry only while that instance is REGISTERED, which
    /// [`ComAmazonawsCodedeployCodedeploy20141006State::register_on_premises_instance`]
    /// maintains; an operation that deregisters an instance drops its entry
    /// through [`ComAmazonawsCodedeployCodedeploy20141006State::release_iam_arns_of`].
    iam_arn_owners: HashMap<String, String>,
    /// Counter behind every minted identifier, so two ids from one process can
    /// never collide.
    pub next_id: u64,
    /// Whether the `CodeDeployDefault.*` built-ins have been seeded.
    seeded: bool,
    /// Which task, if any, is driving this store's deployments while nobody calls
    /// this service; see [`deployment_clock`].
    deployment_clock: deployment_clock::DeploymentClock,
    /// The trigger notifications this store has queued and not yet published; see
    /// [`trigger_notifications`]. They are published with the store UNLOCKED, so
    /// they wait here between the pass that owed them and the publish.
    trigger_notifications: Vec<trigger_notifications::PendingNotification>,
    /// The deployments that may still owe a trigger notification, creation-ordered.
    /// Only these are walked per pass, so the cost of publishing what a deployment
    /// owes does not grow with the deployment history.
    notifying_deployments: Vec<String>,
}

/// One of the built-in deployment configurations CodeDeploy ships.
#[derive(Debug, Clone, Copy)]
struct BuiltinDeploymentConfig {
    name: &'static str,
    compute_platform: &'static str,
    /// `minimumHealthyHosts` as (type, value), for the EC2/on-premises built-ins.
    minimum_healthy_hosts: Option<(&'static str, i32)>,
    /// `trafficRoutingConfig`, for the Lambda and ECS built-ins.
    traffic_routing: Option<BuiltinTrafficRouting>,
}

/// How a built-in on the Lambda or Amazon ECS compute platform shifts traffic:
/// one of the model's `TrafficRoutingType` values
/// (`smithy_query(query="shape",shape="TrafficRoutingType")`: TimeBasedCanary |
/// TimeBasedLinear | AllAtOnce), carrying the percentage and the interval the
/// time-based ones shift it over.
#[derive(Debug, Clone, Copy)]
enum BuiltinTrafficRouting {
    /// The whole fleet of traffic at once, so there is no percentage or interval
    /// to carry.
    AllAtOnce,
    /// `timeBasedCanary`: `canaryPercentage` of the traffic shifted first, the
    /// rest `canaryInterval` minutes later. Both members are modelled in those
    /// units — "the percentage of traffic to shift in the first increment" and
    /// "the number of minutes between the first and second traffic shifts"
    /// (`smithy_query(query="shape",shape="TimeBasedCanary")`).
    TimeBasedCanary { percentage: i32, interval: i32 },
    /// `timeBasedLinear`: `linearPercentage` of the traffic shifted at the start
    /// of each increment, `linearInterval` minutes between increments
    /// (`smithy_query(query="shape",shape="TimeBasedLinear")`).
    TimeBasedLinear { percentage: i32, interval: i32 },
}

impl BuiltinTrafficRouting {
    /// This routing as the modelled `TrafficRoutingConfig` a reader answers.
    fn to_config(self) -> TrafficRoutingConfig {
        match self {
            Self::AllAtOnce => TrafficRoutingConfig {
                r#type: Some("AllAtOnce".to_string()),
                ..Default::default()
            },
            Self::TimeBasedCanary {
                percentage,
                interval,
            } => TrafficRoutingConfig {
                r#type: Some("TimeBasedCanary".to_string()),
                time_based_canary: Some(crate::ops::types::TimeBasedCanary {
                    canary_percentage: percentage,
                    canary_interval: interval,
                }),
                time_based_linear: None,
            },
            Self::TimeBasedLinear {
                percentage,
                interval,
            } => TrafficRoutingConfig {
                r#type: Some("TimeBasedLinear".to_string()),
                time_based_canary: None,
                time_based_linear: Some(crate::ops::types::TimeBasedLinear {
                    linear_percentage: percentage,
                    linear_interval: interval,
                }),
            },
        }
    }
}

/// The built-in deployment configurations CodeDeploy ships, with the
/// `minimumHealthyHosts` / `trafficRoutingConfig` the AWS CodeDeploy User Guide
/// documents for each ("Deployment configurations on an EC2/on-premises compute
/// platform" and "Deployment configurations on an AWS Lambda or Amazon ECS
/// compute platform").
///
/// Every one of them resolves for every account without being created, so the
/// whole catalog is seeded — not only the all-at-once entry of each platform:
/// the time-based (canary and linear) traffic-shifting configurations are what a
/// Lambda or Amazon ECS client names to deploy gradually, and no client creates
/// them first.
const BUILTIN_DEPLOYMENT_CONFIGS: &[BuiltinDeploymentConfig] = &[
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.OneAtATime",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("FLEET_PERCENT", 99)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.HalfAtATime",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("FLEET_PERCENT", 50)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.AllAtOnce",
        compute_platform: "Server",
        minimum_healthy_hosts: Some(("HOST_COUNT", 0)),
        traffic_routing: None,
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaAllAtOnce",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::AllAtOnce),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaCanary10Percent5Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 5,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaCanary10Percent10Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 10,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaCanary10Percent15Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 15,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaCanary10Percent30Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 30,
        }),
    },
    // The one-minute Lambda linear built-in is spelled "Minute", singular, as
    // the User Guide's table spells it; the others are "Minutes".
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery1Minute",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 1,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery2Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 2,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery3Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 3,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.LambdaLinear10PercentEvery10Minutes",
        compute_platform: "Lambda",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 10,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSAllAtOnce",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::AllAtOnce),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSCanary10Percent5Minutes",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 5,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSCanary10Percent15Minutes",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedCanary {
            percentage: 10,
            interval: 15,
        }),
    },
    // The Amazon ECS linear built-ins are both spelled "Minutes", the
    // one-minute one included, as the User Guide's table spells them.
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSLinear10PercentEvery1Minutes",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 1,
        }),
    },
    BuiltinDeploymentConfig {
        name: "CodeDeployDefault.ECSLinear10PercentEvery3Minutes",
        compute_platform: "ECS",
        minimum_healthy_hosts: None,
        traffic_routing: Some(BuiltinTrafficRouting::TimeBasedLinear {
            percentage: 10,
            interval: 3,
        }),
    },
];

/// Whether `name` is one of the deployment configurations CodeDeploy itself
/// ships ([`BUILTIN_DEPLOYMENT_CONFIGS`]), as opposed to one a client created.
///
/// A built-in is in the same store as a created configuration — it is read,
/// listed and named by a deployment group the same way — so what it is, rather
/// than where it is stored, is what the operations that may not change it (a
/// deletion) ask about. Matching is over the stored names, so a name that only
/// LOOKS like a built-in (a different `CodeDeployDefault.*` spelling a client
/// created) is a created configuration and is treated as one.
pub fn is_predefined_deployment_config(name: &str) -> bool {
    BUILTIN_DEPLOYMENT_CONFIGS
        .iter()
        .any(|builtin| builtin.name == name)
}

impl ComAmazonawsCodedeployCodedeploy20141006State {
    /// Whether the deployment configuration `name` is one something still uses,
    /// so that deleting it would leave a reference to a configuration that is
    /// not there.
    ///
    /// Two things use a configuration: a deployment group, whose stored
    /// `deploymentConfigName` is the configuration its deployments run under
    /// until the group is updated or deleted, and a deployment that has not yet
    /// reached a terminal status ([`deployment_is_terminal`]), which is running
    /// under the configuration right now. A deployment that already finished
    /// only RECORDS the configuration it ran under, as history, so it never
    /// blocks a deletion.
    pub fn deployment_config_is_in_use(&self, name: &str) -> bool {
        let named_by_a_group = self.applications.values().any(|application| {
            application
                .deployment_groups
                .values()
                .any(|group| group.deployment_config_name == name)
        });
        named_by_a_group
            || self.deployments.values().any(|deployment| {
                deployment.deployment_config_name == name
                    && !deployment_is_terminal(&deployment.status)
            })
    }

    /// Seed the built-in deployment configurations, once per store.
    fn ensure_seeded(&mut self, now_ms: i64) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        for builtin in BUILTIN_DEPLOYMENT_CONFIGS {
            let id = self.mint_uuid("deployment-config");
            self.insert_deployment_config(DeploymentConfig {
                deployment_config_id: id,
                deployment_config_name: builtin.name.to_string(),
                minimum_healthy_hosts: builtin.minimum_healthy_hosts.map(|(kind, value)| {
                    MinimumHealthyHosts {
                        r#type: Some(kind.to_string()),
                        value,
                    }
                }),
                traffic_routing_config: builtin
                    .traffic_routing
                    .map(BuiltinTrafficRouting::to_config),
                zonal_config: None,
                compute_platform: builtin.compute_platform.to_string(),
                create_time_ms: now_ms,
            });
        }
    }

    /// Store `config` under its name, appending it to
    /// [`Self::deployment_config_order`] the first time that name is stored.
    ///
    /// The one way a configuration enters the store, so the listing index and the
    /// map stay in step: a built-in seeded here and a configuration a client
    /// created both go through it, and re-storing a name already present keeps
    /// the position it already holds rather than listing it twice.
    pub fn insert_deployment_config(&mut self, config: DeploymentConfig) {
        let name = config.deployment_config_name.clone();
        if self.deployment_configs.insert(name.clone(), config).is_none() {
            self.deployment_config_order.push(name);
        }
    }

    /// Drop the deployment configuration stored under `name`, and its position in
    /// [`Self::deployment_config_order`] with it.
    ///
    /// The counterpart of [`Self::insert_deployment_config`], and the one way a
    /// configuration LEAVES the store, so the listing index and the map stay in
    /// step: dropping the map entry alone would leave the name in the index, and
    /// re-creating a configuration under that name would then list it twice and
    /// grow the index on every create/delete cycle. Removing a name the store
    /// does not carry does nothing.
    pub fn remove_deployment_config(&mut self, name: &str) {
        self.deployment_configs.remove(name);
        self.deployment_config_order
            .retain(|stored| stored != name);
    }

    /// The names of the stored deployment configurations, in the order they
    /// entered the store: `ListDeploymentConfigs`'s listing.
    ///
    /// Filtered by what [`Self::deployment_configs`] still carries, so a
    /// configuration dropped from the map is gone from the listing too and the
    /// listing can never name a record a reader would answer as absent.
    pub fn deployment_config_names(&self) -> Vec<String> {
        self.deployment_config_order
            .iter()
            .filter(|name| self.deployment_configs.contains_key(*name))
            .cloned()
            .collect()
    }

    /// The tags the resource a `ResourceArn` named carries, for reading.
    ///
    /// The tags of a resource live ON that resource's own record, so a tagging
    /// operation and the operation that creates or reads the resource see one
    /// list and not two copies of it.
    pub fn tags_of(&self, resource: &TaggedResource) -> Result<&[Tag], MissingResource> {
        match resource {
            TaggedResource::Application(name) => self
                .applications
                .get(name)
                .map(|application| application.tags.as_slice())
                .ok_or_else(|| MissingResource::Application(name.clone())),
            TaggedResource::DeploymentGroup {
                application_name,
                deployment_group_name,
            } => {
                let application = self
                    .applications
                    .get(application_name)
                    .ok_or_else(|| MissingResource::Application(application_name.clone()))?;
                application
                    .deployment_groups
                    .get(deployment_group_name)
                    .map(|group| group.tags.as_slice())
                    .ok_or_else(|| {
                        MissingResource::DeploymentGroup(deployment_group_name.clone())
                    })
            }
        }
    }

    /// The tags of the resource a `ResourceArn` named, for writing; see
    /// [`Self::tags_of`].
    pub fn tags_of_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut Vec<Tag>, MissingResource> {
        match resource {
            TaggedResource::Application(name) => self
                .applications
                .get_mut(name)
                .map(|application| &mut application.tags)
                .ok_or_else(|| MissingResource::Application(name.clone())),
            TaggedResource::DeploymentGroup {
                application_name,
                deployment_group_name,
            } => {
                let application = self
                    .applications
                    .get_mut(application_name)
                    .ok_or_else(|| MissingResource::Application(application_name.clone()))?;
                application
                    .deployment_groups
                    .get_mut(deployment_group_name)
                    .map(|group| &mut group.tags)
                    .ok_or_else(|| {
                        MissingResource::DeploymentGroup(deployment_group_name.clone())
                    })
            }
        }
    }

    /// Materialise every deployment transition that was already due at `now_ms`:
    /// `Created` -> `InProgress` once it was owed a start, then `InProgress` ->
    /// its terminal status ([`terminal_status`]) once it was owed completion.
    ///
    /// This is the one write a reporting operation performs: each transition was
    /// owed by the clock at this instant whether or not anybody called, so
    /// applying it here cannot show a caller anything a later caller would not
    /// also see.
    ///
    /// Only the deployments still awaiting a transition are walked, and each
    /// drops out of that set once it is terminal, so the per-request cost is
    /// bounded by how many deployments are in flight rather than by how many
    /// have ever been created.
    fn advance_clock(&mut self, now_ms: i64) {
        let deployments = &mut self.deployments;
        // The deployments this pass settled in a FAILED status, which is what an
        // automatic rollback configured for a deployment failure then acts on:
        // collected here and rolled back below, once the walk is over.
        let mut failed: Vec<String> = Vec::new();
        self.pending_deployments.retain(|id| {
            let Some(deployment) = deployments.get_mut(id) else {
                // Dropped from the history; there is nothing left to settle.
                return false;
            };
            // A deployment that already reached an outcome — it completed, or a
            // caller stopped it — is owed nothing further, so the clock never
            // moves it again.
            if deployment.complete_time_ms.is_some() {
                return false;
            }
            if deployment.start_time_ms.is_none() && deployment.starts_at_ms <= now_ms {
                deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
                deployment.start_time_ms = Some(deployment.starts_at_ms);
            }
            // A blue/green deployment that terminates the original instances
            // reports its termination wait as started once traffic routing is
            // complete, which the clock owes at the same instant whoever asks.
            if deployment.terminates_original_instances
                && deployment.traffic_routing_complete_at_ms() <= now_ms
            {
                deployment.instance_termination_wait_time_started = true;
            }
            if deployment.start_time_ms.is_some()
                && deployment.complete_time_ms.is_none()
                && deployment.completes_at_ms <= now_ms
                // A deployment whose traffic shift is being carried out right now
                // is NOT settled: the outcome it reaches has to follow the shift,
                // so the pass carrying it out writes back what it came to first
                // (see `lambda_traffic_shift::apply_due_shifts`), and the next
                // pass settles it.
                && deployment.traffic_shift
                    != lambda_traffic_shift::TrafficShiftProgress::InFlight
                // Nor is a deployment whose lifecycle event hooks have not all
                // reported `Succeeded`: the result of a validation function is
                // what the lifecycle event it validates completed with, so a
                // deployment whose hook has not reported stays in flight however
                // long the function takes (see `lifecycle_hook`).
                && lifecycle_hook::hooks_are_complete(deployment)
            {
                let status = terminal_status(deployment.target_count());
                deployment.status = status.to_string();
                deployment.complete_time_ms = Some(deployment.completes_at_ms);
                if status == DEPLOYMENT_STATUS_FAILED {
                    failed.push(id.clone());
                }
            }
            deployment.complete_time_ms.is_none()
        });
        for deployment_id in failed {
            rollback::roll_back_on_failure(self, &deployment_id, now_ms);
        }
    }

    /// The deployment of `deployment_group_name` that is still in flight, if any.
    ///
    /// A deployment group runs ONE deployment at a time, so this is what
    /// `CreateDeployment` refuses a second one against that group for. Only the
    /// deployments the clock still owes a transition are walked, so the cost is
    /// bounded by how many are in flight rather than by the history's length.
    pub fn active_deployment_of(
        &self,
        application_name: &str,
        deployment_group_name: &str,
    ) -> Option<&Deployment> {
        self.pending_deployments
            .iter()
            .filter_map(|id| self.deployments.get(id))
            .find(|deployment| {
                deployment.application_name == application_name
                    && deployment.deployment_group_name.as_deref() == Some(deployment_group_name)
                    && !deployment_is_terminal(&deployment.status)
            })
    }

    /// Whether any deployment is still in flight, which is what the deployment
    /// clock keeps ticking for ([`deployment_clock`]).
    pub fn deployments_in_flight(&self) -> bool {
        !self.pending_deployments.is_empty()
    }

    /// Record `deployment` as this store's newest, dropping the oldest history
    /// past [`MAX_DEPLOYMENT_HISTORY`] so the store stays bounded however many
    /// deployments a caller creates.
    pub fn record_deployment(&mut self, deployment: Deployment) {
        let mut deployment = deployment;
        // An inline revision carries the digest of its content, computed once
        // here so every read of this deployment's revision reports it.
        deployment.revision = deployment.revision.map(revision_digest::with_content_digest);
        deployment.previous_revision = deployment
            .previous_revision
            .map(revision_digest::with_content_digest);
        let id = deployment.deployment_id.clone();
        self.deployments.insert(id.clone(), deployment);
        self.deployment_order.push(id.clone());
        self.pending_deployments.push(id.clone());
        self.notifying_deployments.push(id);
        let Some(excess) = self
            .deployment_order
            .len()
            .checked_sub(MAX_DEPLOYMENT_HISTORY)
        else {
            return;
        };
        let dropped: Vec<String> = self.deployment_order.drain(..excess).collect();
        for oldest in dropped {
            self.deployments.remove(&oldest);
            self.pending_deployments.retain(|pending| pending != &oldest);
            self.notifying_deployments
                .retain(|pending| pending != &oldest);
        }
    }

    /// Queue every trigger notification the deployments of this store have come
    /// to owe: one per lifecycle event each of them has REACHED and not yet been
    /// published for, against its deployment group's stored trigger list.
    ///
    /// Runs on every pass through the store, right after the transitions the
    /// clock owed have been materialised ([`Self::advance_clock`]), so an event
    /// is owed its notification at the instant the deployment reaches it however
    /// it got there — the clock settling it, a caller stopping it, or a failed
    /// validation function failing it. Each event is marked on the deployment as
    /// it is queued, so no pass can queue it twice and a deployment drops out of
    /// the walked set once it is terminal and every event it reached is queued.
    ///
    /// Only QUEUED here: the publish itself reaches Amazon SNS's own handler, so
    /// it happens with this store unlocked ([`trigger_notifications::deliver`]).
    fn queue_due_trigger_notifications(&mut self) {
        if self.notifying_deployments.is_empty() {
            return;
        }
        // Phase one: which (deployment, event) pairs are newly due. Reads the
        // deployment records only, so the walked set can be pruned in place.
        let deployments = &self.deployments;
        let mut due: Vec<(String, &'static str)> = Vec::new();
        self.notifying_deployments.retain(|id| {
            let Some(deployment) = deployments.get(id) else {
                // Dropped from the history; there is nothing left to publish.
                return false;
            };
            let reached = trigger_notifications::events_reached(deployment);
            for event in &reached {
                if !deployment.published_trigger_events.contains(event) {
                    due.push((id.clone(), event));
                }
            }
            // A deployment that has reached an outcome reaches no further event,
            // so once every event it reached is queued it is owed nothing more.
            !deployment_is_terminal(&deployment.status)
        });
        // Phase two: the notifications those events owe, against the group each
        // deployment went to as it stands at this instant.
        let mut notifications = Vec::new();
        for (id, event) in &due {
            let Some(deployment) = self.deployments.get(id) else {
                continue;
            };
            let group = deployment
                .deployment_group_name
                .as_deref()
                .and_then(|group_name| {
                    self.applications
                        .get(&deployment.application_name)
                        .and_then(|application| application.deployment_groups.get(group_name))
                });
            let Some(group) = group else {
                continue;
            };
            notifications.extend(trigger_notifications::owed_for_event(
                group,
                deployment,
                event,
            ));
        }
        // Phase three: the events are published-or-dropped from here, whether or
        // not the group held a trigger for them, so no later pass reconsiders
        // them.
        for (id, event) in due {
            if let Some(deployment) = self.deployments.get_mut(&id) {
                deployment.published_trigger_events.push(event);
            }
        }
        self.queue_trigger_notifications(notifications);
    }

    /// Queue `notifications` for publication, dropping what does not fit under
    /// [`MAX_QUEUED_TRIGGER_NOTIFICATIONS`]: the queue is drained by the very next
    /// publish, so the bound is only there to keep a store that has nothing to
    /// publish to (no Amazon SNS mounted beside this service) from growing.
    pub fn queue_trigger_notifications(
        &mut self,
        notifications: Vec<trigger_notifications::PendingNotification>,
    ) {
        for notification in notifications {
            if self.trigger_notifications.len() >= MAX_QUEUED_TRIGGER_NOTIFICATIONS {
                ::tracing::debug!(
                    topic = %notification.topic_arn,
                    "the queue of trigger notifications waiting to be published is full; this \
                     notification is dropped"
                );
                return;
            }
            self.trigger_notifications.push(notification);
        }
    }

    /// Take every queued trigger notification, leaving the queue empty, so each
    /// one is published exactly once.
    pub fn take_trigger_notifications(
        &mut self,
    ) -> Vec<trigger_notifications::PendingNotification> {
        ::std::mem::take(&mut self.trigger_notifications)
    }

    /// Re-key the application stored under `current` as `new`, carrying every
    /// record that references it by name with it. Renaming a `current` that no
    /// application is stored under does nothing: whether the application exists
    /// is the caller's to report, because it is the caller that raises the
    /// modelled `ApplicationDoesNotExistException` for it.
    ///
    /// It is the SAME resource under a new name, so its identity is untouched —
    /// `applicationId`, compute platform, tags and creation time all stay — and
    /// everything that points at it by name moves: its deployment groups, and
    /// the deployments it ran (`deployment_group_info` matches a deployment to
    /// its group by application name, so a deployment left under the old name
    /// would disappear from the group's last-deployment reporting).
    ///
    /// The creation position in [`Self::application_order`] is kept rather than
    /// re-appended, so renaming does not reorder `ListApplications`.
    pub fn rename_application(&mut self, current: &str, new: &str) {
        if current == new {
            return;
        }
        let Some(mut application) = self.applications.remove(current) else {
            return;
        };
        application.application_name = new.to_string();
        for group in application.deployment_groups.values_mut() {
            group.application_name = new.to_string();
        }
        for deployment in self.deployments.values_mut() {
            if deployment.application_name == current {
                deployment.application_name = new.to_string();
            }
        }
        for stored in &mut self.application_order {
            if stored == current {
                *stored = new.to_string();
            }
        }
        self.applications.insert(new.to_string(), application);
    }

    /// Settle `deployment_id` in the terminal `status` at `now_ms`, with the
    /// error information CodeDeploy reports for it, and answer whether the
    /// deployment was there to settle.
    ///
    /// A deployment the clock had not yet started also gets the start it was
    /// owed, and drops out of the set the clock still owes a transition, so
    /// nothing can later overwrite the terminal status written here.
    pub fn settle_deployment(
        &mut self,
        deployment_id: &str,
        status: &str,
        error_information: Option<crate::ops::types::ErrorInformation>,
        now_ms: i64,
    ) -> bool {
        let Some(deployment) = self.deployments.get_mut(deployment_id) else {
            return false;
        };
        deployment.status = status.to_string();
        deployment.start_time_ms = Some(deployment.start_time_ms.unwrap_or(now_ms));
        deployment.complete_time_ms = Some(now_ms);
        deployment.error_information = error_information;
        self.pending_deployments
            .retain(|pending| pending != deployment_id);
        true
    }

    /// The instance `iam_arn` currently backs, if any.
    ///
    /// A LOOKUP, not a scan: the per-registration work does not grow with how
    /// many instances the store holds, so one caller's registrations cannot
    /// lengthen every other caller's time under the store lock.
    pub fn instance_registered_with(&self, iam_arn: &str) -> Option<&OnPremisesInstance> {
        self.iam_arn_owners
            .get(iam_arn)
            .and_then(|name| self.on_premises_instances.get(name))
            .filter(|instance| instance.is_registered())
    }

    /// The names of the on-premises instances a listing answers for one
    /// `registrationStatus` and one tag filter list, in REGISTRATION order so
    /// the listing is deterministic and pages stably.
    ///
    /// A host's REGISTRATION is what the fleet listing reports, so the default
    /// listing answers the hosts that hold one: deregistering a host ends its
    /// registration, and the record CodeDeploy keeps for it is surfaced by the
    /// `Deregistered` registration status — the status whose own purpose is to ask
    /// for the retired records — rather than by the unrestricted listing. That is
    /// the behaviour measured on the real service: after a successful
    /// deregistration the host is gone from the unfiltered listing while every
    /// other registered host is still in it.
    ///
    /// Both members RESTRICT what the registry would otherwise answer, and the tag
    /// filters "are used to restrict the on-premises instance names returned"
    /// (`smithy_query(query="doc",shape="ListOnPremisesInstancesInput",member="tagFilters")`)
    /// — a record is reported with the tags it still carries, which is what lets a
    /// retired record be selected by tag under the `Deregistered` status.
    pub fn on_premises_instance_names(
        &self,
        registration_status: Option<&str>,
        tag_filters: &[TagFilter],
    ) -> Vec<String> {
        self.on_premises_instance_order
            .iter()
            .filter_map(|name| self.on_premises_instances.get(name))
            .filter(|instance| match registration_status {
                None | Some(REGISTRATION_STATUS_REGISTERED) => instance.is_registered(),
                Some(REGISTRATION_STATUS_DEREGISTERED) => !instance.is_registered(),
                // A status outside the modelled enum is refused at the request
                // boundary, so it never reaches here; selecting nothing for it
                // is the conservative answer if it ever did.
                Some(_) => false,
            })
            .filter(|instance| tag_filters_select(tag_filters, &instance.tags))
            .map(|instance| instance.instance_name.clone())
            .collect()
    }

    /// Record `instance` as this store's newest registration, keeping the
    /// registry bounded by [`MAX_ON_PREMISES_INSTANCES`].
    ///
    /// `false` means the registry is full of instances that are all still
    /// REGISTERED, so nothing can be evicted to make room and the caller must
    /// refuse the request rather than grow the store. Re-registering a name the
    /// store already holds replaces that record and needs no room at all.
    pub fn register_on_premises_instance(&mut self, instance: OnPremisesInstance) -> bool {
        let name = instance.instance_name.clone();
        let replacing = self.on_premises_instances.contains_key(&name);
        if !replacing {
            self.evict_deregistered_instances(MAX_ON_PREMISES_INSTANCES - 1);
            if self.on_premises_instances.len() >= MAX_ON_PREMISES_INSTANCES {
                return false;
            }
            self.on_premises_instance_order.push(name.clone());
        }
        // The IAM ARN this name used to hold is free again; the new one is now
        // taken. Both are single-key operations on the index.
        self.release_iam_arns_of(&name);
        for arn in [&instance.iam_user_arn, &instance.iam_session_arn]
            .into_iter()
            .flatten()
        {
            self.iam_arn_owners.insert(arn.clone(), name.clone());
        }
        self.on_premises_instances.insert(name, instance);
        true
    }

    /// Free the IAM ARNs the instance named `name` holds, so they back no
    /// registered instance any more. Called when its registration ends — by a
    /// re-registration here, and by an operation that deregisters it.
    pub fn release_iam_arns_of(&mut self, name: &str) {
        let Some(instance) = self.on_premises_instances.get(name) else {
            return;
        };
        let held: Vec<String> = [&instance.iam_user_arn, &instance.iam_session_arn]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        for arn in held {
            if self.iam_arn_owners.get(&arn).is_some_and(|owner| owner == name) {
                self.iam_arn_owners.remove(&arn);
            }
        }
    }

    /// Drop on-premises instance records that are no longer REGISTERED, oldest
    /// first, until the registry holds at most `target` of them.
    ///
    /// Retention, not a reporting-time mutation: it only ever drops records a
    /// caller already deregistered, and it runs on the registering call that
    /// needs the room.
    fn evict_deregistered_instances(&mut self, target: usize) {
        if self.on_premises_instances.len() <= target {
            return;
        }
        let mut excess = self.on_premises_instances.len() - target;
        let instances = &mut self.on_premises_instances;
        let owners = &mut self.iam_arn_owners;
        self.on_premises_instance_order.retain(|name| {
            let Some(instance) = instances.get(name) else {
                // Not in the registry at all: the order list is stale here.
                return false;
            };
            if excess == 0 || instance.is_registered() {
                return true;
            }
            for arn in [&instance.iam_user_arn, &instance.iam_session_arn]
                .into_iter()
                .flatten()
            {
                owners.remove(arn);
            }
            instances.remove(name);
            excess -= 1;
            false
        });
    }

    /// Mint an identifier shaped like the UUID CodeDeploy answers with for an
    /// application, a deployment group or a deployment configuration.
    pub fn mint_uuid(&mut self, kind: &str) -> String {
        mint_uuid_from(&mut self.next_id, kind)
    }

    /// Record a connection to a GitHub account under `token_name`, answering
    /// whether it was added (a name the store already holds is left where it is,
    /// so the linking order stays stable and no name is stored twice).
    pub fn record_git_hub_account_token(&mut self, token_name: &str) -> bool {
        if self
            .git_hub_account_token_names
            .iter()
            .any(|stored| stored == token_name)
        {
            return false;
        }
        self.git_hub_account_token_names
            .push(token_name.to_string());
        true
    }

    /// Remove the connection to a GitHub account named `token_name`, answering
    /// whether the store held one.
    pub fn remove_git_hub_account_token(&mut self, token_name: &str) -> bool {
        let before = self.git_hub_account_token_names.len();
        self.git_hub_account_token_names
            .retain(|stored| stored != token_name);
        self.git_hub_account_token_names.len() != before
    }

    /// Mint a `d-XXXXXXXXX` deployment id: the `d-` prefix and nine characters
    /// from CodeDeploy's uppercase-alphanumeric deployment-id alphabet.
    pub fn mint_deployment_id(&mut self) -> String {
        let digest = self.digest("deployment");
        let suffix: String = digest
            .iter()
            .take(DEPLOYMENT_ID_SUFFIX_LEN)
            .map(|byte| {
                DEPLOYMENT_ID_ALPHABET[usize::from(*byte) % DEPLOYMENT_ID_ALPHABET.len()] as char
            })
            .collect();
        format!("{DEPLOYMENT_ID_PREFIX}{suffix}")
    }

    /// The bytes every minted id is cut from: this store's monotonic counter
    /// (unique within the process) folded with a per-process seed (so two
    /// processes do not mint the same ids), hashed so an id discloses neither.
    fn digest(&mut self, kind: &str) -> [u8; 32] {
        digest_from(&mut self.next_id, kind)
    }

    /// Mint the id of one lifecycle event hook execution: the opaque token
    /// CodeDeploy hands a validation function in the lifecycle event and that the
    /// function reports its result back under
    /// (`smithy_query(query="shape",shape="LifecycleEventHookExecutionId")` states
    /// no form for it), cut from this store's own minting.
    pub fn mint_hook_execution_id(&mut self) -> String {
        hex::encode(self.digest("lifecycle-event-hook-execution"))
    }
}

/// [`ComAmazonawsCodedeployCodedeploy20141006State::mint_uuid`] over the id
/// counter alone, so a handler holding a record of this store borrowed can still
/// mint — which is what lets it mint only AFTER every refusal has been answered.
pub fn mint_uuid_from(counter: &mut u64, kind: &str) -> String {
    let hex = hex::encode(digest_from(counter, kind));
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// See [`ComAmazonawsCodedeployCodedeploy20141006State::digest`].
fn digest_from(counter: &mut u64, kind: &str) -> [u8; 32] {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    hasher.finalize().into()
}

/// A seed drawn once per process, so ids minted by two emulator processes from
/// the same counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The instant the clock owes a deployment created at `create_time_ms` its start.
pub fn deployment_start_due(create_time_ms: i64) -> i64 {
    create_time_ms + DEPLOYMENT_START_MILLIS
}

/// The instant the clock owes a deployment created at `create_time_ms` its
/// terminal status.
pub fn deployment_completion_due(create_time_ms: i64) -> i64 {
    create_time_ms + DEPLOYMENT_COMPLETION_MILLIS
}

/// What a deployment of `group` owes the original (blue) environment: whether it
/// terminates its instances at all, and how long it waits before doing so.
///
/// Only a blue/green deployment group has an original environment to terminate
/// (`DeploymentStyle.deploymentType`), and only its `TERMINATE` action
/// terminates it; the wait is the group's own
/// `terminationWaitTimeInMinutes` — a duration the caller asked for, so it keeps
/// its value — held to the range the model documents for that member
/// ([`MAX_TERMINATION_WAIT_MINUTES`]).
pub fn termination_wait(group: &DeploymentGroup) -> (bool, i64) {
    let blue_green = group_is_blue_green(group);
    let option = group
        .blue_green_deployment_configuration
        .as_ref()
        .and_then(|config| config.terminate_blue_instances_on_deployment_success.as_ref());
    let terminates = blue_green
        && option
            .and_then(|option| option.action.as_deref())
            .is_some_and(|action| action == INSTANCE_ACTION_TERMINATE);
    if !terminates {
        return (false, 0);
    }
    let minutes = option
        .map(|option| option.termination_wait_time_in_minutes)
        .unwrap_or_default()
        .clamp(0, MAX_TERMINATION_WAIT_MINUTES);
    (true, i64::from(minutes) * 60_000)
}

/// Epoch milliseconds as the modelled `timestamp` type.
pub fn timestamp(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis)
}

/// A modelled `timestamp` as epoch milliseconds, for comparing a request's time
/// range against a stored record. `None` when the value cannot be expressed in
/// milliseconds at all — answer that as a refusal rather than as a filter that
/// silently matches nothing.
pub fn timestamp_millis(value: &::aws_smithy_types::DateTime) -> Option<i64> {
    value.to_millis().ok()
}

/// The deployment configuration CodeDeploy applies when neither the request nor
/// the deployment group names one: the default built-in for the compute platform.
pub fn default_deployment_config_for(compute_platform: &str) -> &'static str {
    match compute_platform {
        "Lambda" => "CodeDeployDefault.LambdaAllAtOnce",
        "ECS" => "CodeDeployDefault.ECSAllAtOnce",
        _ => "CodeDeployDefault.OneAtATime",
    }
}

/// Whether `id` is shaped like a deployment id at all.
///
/// CodeDeploy mints deployment ids as `d-` plus nine characters of its
/// uppercase-alphanumeric alphabet (see
/// [`ComAmazonawsCodedeployCodedeploy20141006State::mint_deployment_id`]), and
/// the model puts no `@pattern` on `DeploymentId`
/// (`smithy_query(query="shape",shape="DeploymentId")`), so the format a request
/// must satisfy is the one the service itself issues. Anything else is not an id
/// of anything and is what `InvalidDeploymentIdException` describes: an
/// operation taking a `deploymentId` gates the FORMAT before it looks the
/// deployment up, so "that is not an id" and "no deployment has that id" stay
/// two different answers to a client that branches on them.
pub fn deployment_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(DEPLOYMENT_ID_PREFIX) else {
        return false;
    };
    suffix.len() == DEPLOYMENT_ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|byte| DEPLOYMENT_ID_ALPHABET.contains(&byte))
}

/// How much of a caller-supplied value a refusal message quotes back.
///
/// A refusal that names the value the caller sent is what lets them fix it, but
/// `DeploymentId` carries no modelled length, so the value is bounded here
/// rather than echoed whole.
const ECHOED_VALUE_CHARS: usize = 64;

/// A caller-supplied value as a refusal message quotes it: itself when short,
/// else its first [`ECHOED_VALUE_CHARS`] characters and an ellipsis.
pub fn echoed(value: &str) -> String {
    let mut shown: String = value.chars().take(ECHOED_VALUE_CHARS).collect();
    if shown.chars().count() < value.chars().count() {
        shown.push('…');
    }
    shown
}

/// The refusal message every operation answers `InvalidDeploymentIdException`
/// with, so the operations that gate the same format cannot word it differently.
/// It quotes the id the caller sent, bounded by [`echoed`].
pub fn malformed_deployment_id_message(deployment_id: &str) -> String {
    format!(
        "Specified DeploymentId is not in the valid format: {}",
        echoed(deployment_id)
    )
}

/// The deployment targets `group` selects, as this emulator can resolve them,
/// in resolution order.
///
/// CodeDeploy resolves a deployment's targets from its deployment group, and a
/// deployment that resolves none runs to `Failed` rather than reporting success
/// for a revision that reached nothing (the AWS CodeDeploy User Guide's "Error
/// codes" gives that failure the [`ERROR_CODE_NO_INSTANCES`] code). What this
/// emulator can resolve, per compute platform
/// (`smithy_query(query="shape",shape="ComputePlatform")`), and the id each
/// target is reported under (the format the model states per platform, see
/// [`DeploymentTargetSnapshot`]):
///
/// * `ECS`: the group's `ecsServices` — one target per cluster/service pair,
///   reported as `<clustername>:<servicename>`.
/// * `Lambda`: a Lambda deployment has exactly one target, the function the
///   revision's AppSpec names — whatever the revision holds, there is one of
///   them — so a Lambda group never resolves none. That function's NAME lives
///   inside the revision, so the target is resolved without an id here and NAMED
///   from the revision's own AppSpec ([`name_lambda_targets`]); a revision that
///   declares no function leaves it unnamed (see
///   [`DeploymentTargetSnapshot::target_id`]).
/// * `Server` (EC2/on-premises): the instances the group selects, reported under
///   their instance ids. The instances this emulator HOLDS are the on-premises
///   ones registered with it, so they are resolved from that registry through
///   the group's `onPremisesInstanceTagFilters` and `onPremisesTagSet` (see
///   [`group_selects_instance`]). An `ec2TagFilter`, an `ec2TagSet` or an Auto
///   Scaling group names EC2 inventory, which CodeDeploy's state does not keep
///   and which this emulator reads from no other service, so those selectors
///   resolve nothing.
///
/// A target of a blue/green deployment group carries the `TargetLabel` of the
/// environment it belongs to: the instances the group selects are the ORIGINAL
/// environment, which the model's `InstanceTarget$instanceLabel` calls `BLUE`
/// ("whether the instance is an original target (BLUE) or a replacement target
/// (GREEN)"). The replacement environment is provisioned from EC2 inventory this
/// emulator does not keep, so it resolves no `GREEN` target; an in-place
/// deployment has no two environments, so its targets carry no label at all.
pub fn deployment_targets(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    group: &DeploymentGroup,
) -> Vec<DeploymentTargetSnapshot> {
    match group.compute_platform.as_str() {
        "ECS" => group
            .ecs_services
            .iter()
            .map(|service| DeploymentTargetSnapshot {
                target_id: Some(format!(
                    "{}:{}",
                    service.cluster_name.clone().unwrap_or_default(),
                    service.service_name.clone().unwrap_or_default()
                )),
                target_type: DEPLOYMENT_TARGET_TYPE_ECS.to_string(),
                instance_label: None,
            })
            .collect(),
        // Lambda: one target per deployment, named inside the revision; see the
        // note above.
        "Lambda" => vec![DeploymentTargetSnapshot {
            target_id: None,
            target_type: DEPLOYMENT_TARGET_TYPE_LAMBDA.to_string(),
            instance_label: None,
        }],
        // Server (EC2/on-premises), which is what `computePlatform` holds here
        // when it is neither of the two above — the member is the modelled enum
        // Server | Lambda | ECS. A value outside that enum could not reach the
        // store, and would resolve nothing either: a platform this emulator
        // cannot target has no targets to report.
        _ => {
            let instance_label = group_is_blue_green(group).then(|| TARGET_LABEL_BLUE.to_string());
            state
                .on_premises_instance_order
                .iter()
                .filter_map(|name| state.on_premises_instances.get(name))
                // A deregistered instance keeps its record but is no longer part
                // of the fleet, so a deployment cannot target it.
                .filter(|instance| instance.is_registered())
                .filter(|instance| group_selects_instance(group, &instance.tags))
                .map(|instance| DeploymentTargetSnapshot {
                    target_id: Some(instance.instance_name.clone()),
                    target_type: DEPLOYMENT_TARGET_TYPE_INSTANCE.to_string(),
                    instance_label: instance_label.clone(),
                })
                .collect()
        }
    }
}

/// Name the LAMBDA targets of a deployment from the shift its revision declares.
///
/// A Lambda deployment's target is reported under "a Lambda function name"
/// (`smithy_query(query="doc",shape="BatchGetDeploymentTargetsInput",member="targetIds")`),
/// and the function a Lambda deployment deploys to is the one its revision's
/// AppSpec names ([`appspec::declared_shift`]) — so the target is NAMED here,
/// once, when the deployment is recorded. A revision that declares none leaves it
/// unnamed: this service holds no other record of which function the deployment
/// went to, and a target it cannot name is not one a client can fetch by id.
pub fn name_lambda_targets(
    targets: &mut [DeploymentTargetSnapshot],
    declared: Option<&appspec::LambdaTrafficShift>,
) {
    let Some(declared) = declared else {
        return;
    };
    for target in targets
        .iter_mut()
        .filter(|target| target.target_type == DEPLOYMENT_TARGET_TYPE_LAMBDA)
    {
        target.target_id = Some(declared.function_name.clone());
    }
}

/// How many deployment targets `group` selects, as this emulator can resolve
/// them: the targets it resolves ([`deployment_targets`]), counted.
#[cfg(test)]
fn deployment_target_count(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    group: &DeploymentGroup,
) -> usize {
    deployment_targets(state, group).len()
}

/// Whether `group` deploys blue/green, which is what gives its targets an
/// environment label (`smithy_query(query="shape",shape="DeploymentType")`:
/// IN_PLACE | BLUE_GREEN).
fn group_is_blue_green(group: &DeploymentGroup) -> bool {
    group
        .deployment_style
        .as_ref()
        .and_then(|style| style.deployment_type.as_deref())
        == Some(DEPLOYMENT_TYPE_BLUE_GREEN)
}

/// The `TargetStatus` each target of `deployment` carries.
///
/// A deployment's targets are where its revision is being installed, so a
/// target's status follows the deployment's own progress: this emulator
/// deploys to every target of a deployment together, so each of them is as far
/// along as the deployment is. `TargetStatus`
/// (`smithy_query(query="shape",shape="TargetStatus")`: Pending | InProgress |
/// Succeeded | Failed | Skipped | Unknown | Ready) maps onto `DeploymentStatus`
/// one for one here: a deployment still running has `InProgress` targets, a
/// finished one has targets that `Succeeded` or `Failed` with it, and a
/// deployment a caller STOPPED leaves its targets `Skipped` — the revision was
/// never finished on them. A deployment the clock has not started yet has
/// `Pending` targets, which no listing reaches: an operation over the targets of
/// a deployment that has not started is refused with
/// `DeploymentNotStartedException` (see [`Deployment::is_started`]).
pub fn target_status(deployment: &Deployment) -> &'static str {
    match deployment.status.as_str() {
        DEPLOYMENT_STATUS_SUCCEEDED => TARGET_STATUS_SUCCEEDED,
        DEPLOYMENT_STATUS_FAILED => TARGET_STATUS_FAILED,
        DEPLOYMENT_STATUS_STOPPED => TARGET_STATUS_SKIPPED,
        _ if deployment.is_started() => TARGET_STATUS_IN_PROGRESS,
        _ => TARGET_STATUS_PENDING,
    }
}

/// Why a value is not an id a target of `compute_platform` could ever be
/// reported under.
///
/// The FORM of a target id is per compute platform, as the model states it
/// (`smithy_query(query="doc",shape="BatchGetDeploymentTargetsInput",member="targetIds")`):
/// an EC2/on-premises instance id or a Lambda function name is a resource name,
/// while an Amazon ECS target is "pairs of Amazon ECS clusters and services
/// specified using the format `<clustername>:<servicename>`". A value outside
/// the form names no target of ANY deployment on that platform, which is what
/// `InvalidDeploymentTargetIdException` describes — so every operation taking a
/// `targetId` gates the form here before it looks the target up, keeping "that
/// is not a target id" and "no target of this deployment has that id" two
/// different answers, exactly as [`deployment_id_is_well_formed`] does for a
/// deployment id.
pub fn check_deployment_target_id(
    compute_platform: &str,
    target_id: &str,
) -> Result<(), NameRefusal> {
    if target_id.trim().is_empty() {
        return Err(NameRefusal::Missing);
    }
    // The platform whose targets are named by a PAIR; the other platforms name
    // a single resource, so the resource-name form answers for them.
    if compute_platform == "ECS" {
        let mut parts = target_id.split(':');
        let (Some(cluster), Some(service), None) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(NameRefusal::Malformed);
        };
        return if is_resource_name(cluster) && is_resource_name(service) {
            Ok(())
        } else {
            Err(NameRefusal::Malformed)
        };
    }
    check_instance_name(target_id)
}

/// Whether `value` has the form of a CodeDeploy resource name: at least one
/// character, at most [`MAX_NAME_LENGTH`] of them, all from the service's
/// resource-name character set (see [`is_name_character`]).
fn is_resource_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_NAME_LENGTH
        && value.chars().all(is_name_character)
}

/// The target of `deployment` reported under `target_id`, if it holds one.
///
/// The deployment's targets are the SNAPSHOT its group resolved when it was
/// created, so this is the one lookup every reader of a single target goes
/// through: a target id the snapshot does not hold belongs to no target of this
/// deployment, whatever the fleet looks like now. A target this emulator
/// resolved but cannot NAME ([`DeploymentTargetSnapshot::target_id`]) is reached
/// by no id.
pub fn find_deployment_target<'a>(
    deployment: &'a Deployment,
    target_id: &str,
) -> Option<&'a DeploymentTargetSnapshot> {
    deployment
        .targets
        .iter()
        .find(|target| target.target_id.as_deref() == Some(target_id))
}

/// The target of `deployment` a reader of ONE target can answer under
/// `target_id` right now, if there is one.
///
/// A deployment's per-target RECORDS come into existence as the deployment
/// resolves its targets, which this service owes at the instant it starts
/// ([`Deployment::is_started`]): until then the deployment holds no record a
/// reader can answer under any id. So a read of one target is keyed on the
/// record alone — either it is there to answer, or it is not — and an id the
/// deployment's snapshot DOES hold a target under is simply unreadable for that
/// window, answered exactly as an id no target of this deployment is reported
/// under ([`deployment_target_not_found`]). That keeps one code for the whole
/// not-yet-readable window, so a client polling a target retries on that one
/// code and stops on the answer.
pub fn readable_deployment_target<'a>(
    deployment: &'a Deployment,
    target_id: &str,
) -> Option<&'a DeploymentTargetSnapshot> {
    if !deployment.is_started() {
        return None;
    }
    find_deployment_target(deployment, target_id)
}

/// The instant a target of `deployment` last changed state.
///
/// A target's state is its deployment's progress ([`target_status`]), so it last
/// changed when the deployment last did: when it completed, else when it
/// started, else when it was created. DERIVED from the deployment's own
/// timestamps rather than stored a second time, so no reader can see a target
/// status and a `lastUpdatedAt` that disagree, and two reads of a target nothing
/// happened to answer the same instant.
pub fn target_last_updated_at_ms(deployment: &Deployment) -> i64 {
    deployment
        .complete_time_ms
        .or(deployment.start_time_ms)
        .unwrap_or(deployment.create_time_ms)
}

/// The lifecycle events of `deployment`, as the modelled `LifecycleEvent` list a
/// target carries.
///
/// CodeDeploy reports a target's lifecycle events as the events its deployment ran
/// on that target. Three kinds of event are held here, and all three are reported,
/// in the order the deployment runs them:
///
/// * the hooks the revision DECLARED, which this service dispatched to a Lambda
///   validation function ([`lifecycle_hook`]) — each as far as its own execution
///   has got ([`hook_event_status`]), in the order its platform runs them;
/// * the event that MOVES the traffic, for a deployment that performs a traffic
///   shift: work this service does itself rather than through a function, so it is
///   reported from that shift's own progress
///   ([`lifecycle_hook::allow_traffic_event`]) and sits between the hook that runs
///   before traffic and the one that runs after it;
/// * a result reported against a hook execution this deployment did not mint,
///   which is a report of one of the platform's lifecycle events made directly
///   rather than by a function this service invoked.
///
/// Nothing here runs the EC2/on-premises hooks the CodeDeploy agent runs, so a
/// deployment that declared no hook, performs no shift and nothing reported for
/// carries no lifecycle event rather than events nothing ran. A report says what
/// the lifecycle event ended as and when it was made, which is when that event
/// ENDED; when it started is not something a report carries, so it is left absent.
fn target_lifecycle_events(
    deployment: &Deployment,
) -> Option<Vec<crate::ops::types::LifecycleEvent>> {
    let hook_event = |hook: &lifecycle_hook::HookExecution| crate::ops::types::LifecycleEvent {
        lifecycle_event_name: Some(hook.lifecycle_event_name.clone()),
        diagnostics: None,
        start_time: None,
        end_time: hook.update_time_ms.map(timestamp),
        status: Some(hook_event_status(deployment, hook).to_string()),
    };
    let before_traffic = deployment
        .hook_executions
        .iter()
        .filter(|hook| !hook.runs_after_traffic())
        .map(hook_event);
    // The traffic move itself, for a deployment that performs one: the alias write
    // this service carries out ([`lambda_traffic_shift`]). The instant it ended is
    // the instant the deployment it completed ended, which is the only instant
    // this service holds for it.
    let allow_traffic = lifecycle_hook::allow_traffic_event(deployment).map(|status| {
        crate::ops::types::LifecycleEvent {
            lifecycle_event_name: Some(lifecycle_hook::LIFECYCLE_EVENT_ALLOW_TRAFFIC.to_string()),
            diagnostics: None,
            start_time: None,
            end_time: (status == LIFECYCLE_EVENT_STATUS_SUCCEEDED)
                .then(|| timestamp(target_last_updated_at_ms(deployment))),
            status: Some(status.to_string()),
        }
    });
    let after_traffic = deployment
        .hook_executions
        .iter()
        .filter(|hook| hook.runs_after_traffic())
        .map(hook_event);
    // A report that landed on a declared hook is already one of the events above,
    // so it is not reported twice.
    let reported = deployment
        .lifecycle_hook_executions
        .iter()
        .filter(|execution| {
            !deployment.hook_executions.iter().any(|hook| {
                hook.lifecycle_event_hook_execution_id.as_deref()
                    == Some(execution.lifecycle_event_hook_execution_id.as_str())
            })
        })
        .map(|execution| crate::ops::types::LifecycleEvent {
            lifecycle_event_name: Some(execution.lifecycle_event_name.clone()),
            diagnostics: None,
            start_time: None,
            end_time: Some(timestamp(execution.update_time_ms)),
            status: Some(execution.status.clone()),
        });
    let events: Vec<crate::ops::types::LifecycleEvent> = before_traffic
        .chain(allow_traffic)
        .chain(after_traffic)
        .chain(reported)
        .collect();
    non_empty(&events)
}

/// The `LifecycleEventStatus` one declared hook's lifecycle event carries
/// (`smithy_query(query="shape",shape="LifecycleEventStatus")`: Failed |
/// InProgress | Pending | Skipped | Succeeded | Unknown).
///
/// The result its validation function reported, when it reported one: that result
/// IS what the lifecycle event completed with. Otherwise the event is as far along
/// as the deployment has taken it — dispatched to the function and awaiting its
/// report (`InProgress`), not dispatched yet (`Pending`), or ended with the
/// deployment itself: a deployment that FAILED for a hook that reported nothing
/// has a failed lifecycle event, and one a caller stopped never ran the rest of
/// them (`Skipped`).
fn hook_event_status(
    deployment: &Deployment,
    hook: &lifecycle_hook::HookExecution,
) -> &'static str {
    if let Some(reported) = hook.status.as_deref() {
        return match reported {
            LIFECYCLE_EVENT_STATUS_FAILED => LIFECYCLE_EVENT_STATUS_FAILED,
            LIFECYCLE_EVENT_STATUS_SUCCEEDED => LIFECYCLE_EVENT_STATUS_SUCCEEDED,
            // Only the two results above are reportable, so anything else is a
            // status this service does not hold for the event.
            _ => LIFECYCLE_EVENT_STATUS_UNKNOWN,
        };
    }
    match deployment.status.as_str() {
        DEPLOYMENT_STATUS_FAILED => LIFECYCLE_EVENT_STATUS_FAILED,
        DEPLOYMENT_STATUS_STOPPED => LIFECYCLE_EVENT_STATUS_SKIPPED,
        DEPLOYMENT_STATUS_SUCCEEDED => LIFECYCLE_EVENT_STATUS_SUCCEEDED,
        _ if hook.lifecycle_event_hook_execution_id.is_some() => {
            LIFECYCLE_EVENT_STATUS_IN_PROGRESS
        }
        _ => LIFECYCLE_EVENT_STATUS_PENDING,
    }
}

/// What a Lambda deployment's target reports about the function it deploys:
/// `LambdaFunctionInfo`, built from the shift its revision's AppSpec DECLARED
/// ([`appspec::declared_shift`]) and how far that shift has got.
///
/// `None` for a deployment whose revision declares none — there is then no
/// function, alias or version this service holds for the target. The weight is
/// "the percentage of production traffic that the target version of a Lambda
/// function receives"
/// (`smithy_query(query="doc",shape="LambdaFunctionInfo",member="targetVersionWeight")`),
/// which is the percentage the last increment carried out put there: nothing of
/// it is on the target version before the first increment, and all of it once the
/// move is complete.
fn lambda_function_info(deployment: &Deployment) -> Option<crate::ops::types::LambdaFunctionInfo> {
    let declared = deployment.lambda_traffic_shift.as_ref()?;
    let applied = deployment
        .traffic_increments_applied
        .checked_sub(1)
        .and_then(|index| deployment.traffic_increments.get(index))
        .map_or(0, |increment| increment.percent);
    Some(crate::ops::types::LambdaFunctionInfo {
        function_name: Some(declared.function_name.clone()),
        function_alias: Some(declared.alias_name.clone()),
        current_version: declared.current_version.clone(),
        target_version: Some(declared.target_version.clone()),
        target_version_weight: f64::from(applied),
    })
}

/// One target of `deployment` as the modelled `DeploymentTarget`.
///
/// The ONE builder every reader of a deployment target answers from
/// (`BatchGetDeploymentTargets`, `GetDeploymentTarget`), so the batch reader and
/// the single-target reader cannot report the same target differently. Which
/// member of `DeploymentTarget` carries the target is its
/// `DeploymentTargetType`, and everything in it comes from the stored
/// deployment: its id and the snapshot its group resolved, the status its
/// progress gives the target ([`target_status`]), and the instant that status
/// last changed ([`target_last_updated_at_ms`]).
///
/// What this emulator does NOT hold, it does not report: a Lambda target's
/// `lambdaFunctionInfo` names the function and the versions inside the
/// revision's AppSpec, which CodeDeploy's state holds no content of, and an ECS
/// target's `taskSetsInfo` and ARN are Amazon ECS's own records, which this
/// emulator reads from no other service.
pub fn deployment_target_view(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
    target: &DeploymentTargetSnapshot,
) -> crate::ops::types::DeploymentTarget {
    let deployment_id = Some(deployment.deployment_id.clone());
    let target_id = target.target_id.clone();
    let status = Some(target_status(deployment).to_string());
    let last_updated_at = Some(timestamp(target_last_updated_at_ms(deployment)));
    let lifecycle_events = target_lifecycle_events(deployment);
    let mut view = crate::ops::types::DeploymentTarget {
        deployment_target_type: Some(target.target_type.clone()),
        instance_target: None,
        lambda_target: None,
        ecs_target: None,
        cloud_formation_target: None,
    };
    match target.target_type.as_str() {
        DEPLOYMENT_TARGET_TYPE_ECS => {
            view.ecs_target = Some(crate::ops::types::ECSTarget {
                deployment_id,
                target_id,
                target_arn: None,
                last_updated_at,
                lifecycle_events,
                status,
                task_sets_info: None,
            });
        }
        DEPLOYMENT_TARGET_TYPE_LAMBDA => {
            view.lambda_target = Some(crate::ops::types::LambdaTarget {
                deployment_id,
                target_id,
                target_arn: None,
                status,
                last_updated_at,
                lifecycle_events,
                lambda_function_info: lambda_function_info(deployment),
            });
        }
        // An EC2/on-premises instance target, which is what a snapshot of this
        // service's own fleet resolves to; see `deployment_targets`.
        _ => {
            view.instance_target = Some(crate::ops::types::InstanceTarget {
                deployment_id,
                target_arn: instance_target_arn(state, target),
                target_id,
                status,
                last_updated_at,
                lifecycle_events,
                instance_label: target.instance_label.clone(),
            });
        }
    }
    view
}

/// One instance target of `deployment` as the modelled `InstanceSummary`.
///
/// The instance-shaped view of the very record [`deployment_target_view`]
/// projects — the model says so twice over ("InstanceSummary is deprecated, use
/// DeploymentTarget instead", "InstanceStatus is deprecated, use TargetStatus
/// instead") — so both views are computed from the SAME stored deployment and
/// the same derivations: the status its progress gives the target
/// ([`target_status`]), the instant that status last changed
/// ([`target_last_updated_at_ms`]), and the lifecycle event results reported
/// against the deployment ([`target_lifecycle_events`]). The environment the
/// instance belongs to is the `TargetLabel` the snapshot resolved, whose members
/// `InstanceType` repeats (`smithy_query(query="shape",shape="InstanceType")`:
/// Blue | Green), so an instance of an in-place deployment — which has one
/// environment — carries none.
pub fn instance_summary(
    deployment: &Deployment,
    target: &DeploymentTargetSnapshot,
) -> crate::ops::types::InstanceSummary {
    crate::ops::types::InstanceSummary {
        deployment_id: Some(deployment.deployment_id.clone()),
        instance_id: target.target_id.clone(),
        status: Some(target_status(deployment).to_string()),
        last_updated_at: Some(timestamp(target_last_updated_at_ms(deployment))),
        lifecycle_events: target_lifecycle_events(deployment),
        instance_type: target.instance_label.clone(),
    }
}

/// The `targetArn` of an instance target: the ARN of the instance itself.
///
/// The instances this emulator holds are the on-premises ones registered with
/// it, and their ARN is the one the registry MINTED at registration
/// ([`instance_arn`]) rather than one rebuilt here, so a target's ARN and the
/// `instanceArn` the on-premises readers answer are the same string. An instance
/// the registry no longer holds has no ARN to report.
fn instance_target_arn(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    target: &DeploymentTargetSnapshot,
) -> Option<String> {
    on_premises_instance_arn(state, target.target_id.as_deref()?)
}

/// The ARN the on-premises registry holds for `instance_name`, if it holds a
/// record of that name at all.
///
/// The ONE place a stored instance's ARN is read from, so every answer that
/// names an on-premises instance by ARN — a target's `targetArn`, the
/// `instanceArn` the on-premises readers report, the refusal that names a target
/// — carries the string the registry MINTED ([`instance_arn`]) rather than one
/// rebuilt from the name. A deregistered record keeps its ARN, which is why this
/// does not filter on registration: the registration it names happened.
fn on_premises_instance_arn(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    instance_name: &str,
) -> Option<String> {
    state
        .on_premises_instances
        .get(instance_name)
        .map(|instance| instance.instance_arn.clone())
}

/// The ARN of the deployment target `target_id` names on `compute_platform`.
///
/// A deployment target is a resource of ANOTHER service — an instance, a
/// function, an Amazon ECS service — and CodeDeploy identifies it by its ARN
/// wherever it names it as a resource, so this is the identifier form a refusal
/// about a target uses. The ARN follows from the id and the platform whose FORM
/// that id is in ([`check_deployment_target_id`], which gates the form before
/// anything calls this), in the region the request was signed for
/// ([`request_region`]) and the emulator's own account:
///
/// * an on-premises instance this service holds a registration for: the ARN that
///   registration minted ([`on_premises_instance_arn`]), so the refusal and the
///   `instanceArn` every on-premises reader answers are the same string;
/// * an Amazon EC2 instance, which is what a `Server` target id in the form the
///   service reserves for an instance id names
///   ([`check_on_premises_instance_name`]): `ec2`'s documented instance ARN,
///   `arn:<partition>:ec2:<region>:<account>:instance/<instance-id>`;
/// * a Lambda function, which a `Lambda` target id names:
///   `arn:<partition>:lambda:<region>:<account>:function:<function-name>`;
/// * an Amazon ECS service, which an `ECS` target id names as the
///   `<clustername>:<servicename>` pair:
///   `arn:<partition>:ecs:<region>:<account>:service/<clustername>/<servicename>`.
///
/// This is a rendering of the identifier the CALLER sent, not a claim that the
/// resource exists — which is why it needs no record and why it is NOT what a
/// target's `targetArn` is reported from: that member says which resource a
/// deployment deployed to, so it is answered from the record this service holds
/// (see [`instance_target_arn`]) and left absent where it holds none.
///
/// `None` where no ARN follows from the id alone: an on-premises host name no
/// registration exists for, because that ARN names a REGISTRATION (see
/// [`instance_arn`]) and this service mints none for a host it never registered.
pub fn deployment_target_arn(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    compute_platform: &str,
    region: &str,
    target_id: &str,
) -> Option<String> {
    match compute_platform {
        // The platform whose targets are named by a PAIR; see
        // `check_deployment_target_id`.
        "ECS" => {
            let (cluster, service) = target_id.split_once(':')?;
            if cluster.is_empty() || service.is_empty() {
                return None;
            }
            Some(::engine::emulator_arn(
                "ecs",
                region,
                &format!("service/{cluster}/{service}"),
            ))
        }
        "Lambda" => Some(::engine::emulator_arn(
            "lambda",
            region,
            &format!("function:{target_id}"),
        )),
        // Server (EC2/on-premises), as everywhere else in this module: the
        // platform `compute_platform` holds when it is neither of the two above.
        // An id in the RESERVED form is an Amazon EC2 instance id — no
        // on-premises host can hold that form — and any other id is an
        // on-premises host name, whose ARN only the registry has.
        _ => {
            if matches!(
                check_on_premises_instance_name(target_id),
                Err(NameRefusal::Reserved)
            ) {
                return Some(::engine::emulator_arn(
                    "ec2",
                    region,
                    &format!("instance/{target_id}"),
                ));
            }
            on_premises_instance_arn(state, target_id)
        }
    }
}

/// Whether a deployment group selects an on-premises instance carrying `tags`.
///
/// The two on-premises selectors a group can carry work as the AWS CodeDeploy
/// User Guide's "Tagging instances for deployments" describes: a
/// `onPremisesInstanceTagFilters` list includes an instance that matches ANY of
/// its filters, while an `onPremisesTagSet` includes one that matches at least
/// one filter in EVERY one of its groups. A group carrying neither selects no
/// on-premises instance at all.
fn group_selects_instance(group: &DeploymentGroup, tags: &[Tag]) -> bool {
    let by_filters = group
        .on_premises_instance_tag_filters
        .iter()
        .any(|filter| tag_filter_matches(filter, tags));
    let by_tag_set = group
        .on_premises_tag_set
        .as_ref()
        .and_then(|set| set.on_premises_tag_set_list.as_ref())
        .is_some_and(|groups| {
            !groups.is_empty()
                && groups.iter().all(|filters| {
                    filters
                        .iter()
                        .any(|filter| tag_filter_matches(filter, tags))
                })
        });
    by_filters || by_tag_set
}

/// Whether any of `tags` satisfies one on-premises tag filter.
///
/// `TagFilterType` (`smithy_query(query="shape",shape="TagFilterType")`:
/// KEY_ONLY | VALUE_ONLY | KEY_AND_VALUE) says WHICH of the filter's members has
/// to match: `KEY_AND_VALUE` needs BOTH, so a filter asking for it while naming
/// only one of them describes no tag and selects nothing. A filter that states no
/// type can only mean the members it does name, and one that names neither
/// member selects nothing either.
fn tag_filter_matches(filter: &TagFilter, tags: &[Tag]) -> bool {
    let key_matches = |tag: &Tag| filter.key.is_some() && tag.key == filter.key;
    let value_matches = |tag: &Tag| filter.value.is_some() && tag.value == filter.value;
    match filter.r#type.as_deref() {
        Some("KEY_ONLY") => tags.iter().any(key_matches),
        Some("VALUE_ONLY") => tags.iter().any(value_matches),
        Some("KEY_AND_VALUE") => {
            filter.key.is_some()
                && filter.value.is_some()
                && tags.iter().any(|tag| key_matches(tag) && value_matches(tag))
        }
        // A filter that states no type: the members it does name.
        _ => {
            (filter.key.is_some() || filter.value.is_some())
                && tags.iter().any(|tag| {
                    (filter.key.is_none() || key_matches(tag))
                        && (filter.value.is_none() || value_matches(tag))
                })
        }
    }
}

/// Whether a list of on-premises instance tag filters selects an instance
/// carrying `tags`.
///
/// An EMPTY list is no restriction at all, so it selects every instance; a
/// non-empty one selects an instance that matches ANY of its filters — the same
/// rule a deployment group's `onPremisesInstanceTagFilters` follows (see
/// [`group_selects_instance`]), which is the one semantics CodeDeploy gives a
/// flat list of tag filters. Each filter is matched by [`tag_filter_matches`],
/// so a filter that describes no tag (`KEY_AND_VALUE` naming one member, or a
/// filter naming neither) selects nothing through it.
pub fn tag_filters_select(filters: &[TagFilter], tags: &[Tag]) -> bool {
    filters.is_empty() || filters.iter().any(|filter| tag_filter_matches(filter, tags))
}

/// The terminal status a deployment with `target_count` targets reaches:
/// `Succeeded` when its revision had somewhere to go, `Failed` when the
/// deployment group resolved nothing to deploy to.
fn terminal_status(target_count: usize) -> &'static str {
    if target_count == 0 {
        DEPLOYMENT_STATUS_FAILED
    } else {
        DEPLOYMENT_STATUS_SUCCEEDED
    }
}

/// Whether `config` asks for automatic rollback without naming an event that
/// would trigger one, which is one of the cases
/// `InvalidAutoRollbackConfigException` describes ("automatic rollback is
/// enabled, but an invalid triggering event type or no event types were
/// listed").
pub fn auto_rollback_is_enabled_without_events(config: &AutoRollbackConfiguration) -> bool {
    config.enabled && config.events.as_ref().is_none_or(Vec::is_empty)
}

/// `config` as it is STORED: `None` when it carries no information.
///
/// A disabled automatic rollback listing no events says nothing a caller did not
/// already get by omitting the member, so it is normalised away at the store
/// rather than kept and echoed — a round-tripped deployment-group or deployment
/// document then holds exactly the members that carry information, and absent
/// and information-free requests read back the same. A configuration that lists
/// events is kept whether or not it is enabled: the events are information.
pub fn normalized_auto_rollback_configuration(
    config: Option<AutoRollbackConfiguration>,
) -> Option<AutoRollbackConfiguration> {
    config.filter(|config| config.enabled || config.events.as_ref().is_some_and(|e| !e.is_empty()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodedeployCodedeploy20141006State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodedeployCodedeploy20141006State>)
}

/// This service's ARN namespace
/// (`smithy_query(query="service_info")`: `arnNamespace` `codedeploy`).
const ARN_SERVICE: &str = "codedeploy";

/// The region an emitted ARN carries when the request carries no SigV4
/// credential scope to take one from (an anonymous or host-routed request).
const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of an on-premises instance:
/// `arn:aws:codedeploy:<region>:<account>:instance/<instanceName>_<registrationId>`.
///
/// The CodeDeploy User Guide shows the name in the resource part
/// ("arn:aws:codedeploy:us-west-2:111122223333:instance/AssetTag12010298EX"), and
/// the ARN CodeDeploy MINTS carries the registration's own identity after it: the
/// ARN names a REGISTRATION, not a host name, so registering a name again mints a
/// new one and the two registrations of one name are told apart by it. The
/// account segment comes from the engine, never from a literal.
pub fn instance_arn(region: &str, instance_name: &str, registration_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("instance/{instance_name}_{registration_id}"),
    )
}

/// Why CodeDeploy will not accept a name a caller sent for an on-premises
/// instance. Each operation maps it to the modelled error IT declares, so the
/// two refusals stay one rule shared by every on-premises operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRefusal {
    /// The caller sent no name at all (absent, empty, or only whitespace).
    Missing,
    /// The caller sent a name outside the form CodeDeploy accepts.
    Malformed,
    /// The caller sent a name in a form CodeDeploy RESERVES, so no resource can
    /// ever carry it; see [`check_on_premises_instance_name`].
    Reserved,
}

/// `InvalidInstanceNameException` (HTTP 400): the sentence that refusal carries
/// for each way a name can fail [`check_on_premises_instance_name`], so every
/// operation taking an on-premises instance name refuses one in the same words.
pub fn invalid_instance_name_message(refusal: NameRefusal) -> &'static str {
    match refusal {
        NameRefusal::Reserved => INSTANCE_NAME_RESERVED_PREFIX,
        // A name the caller did not give is its own `…RequiredException`, which
        // every caller answers before it reaches here; stated rather than
        // panicked on so the mapping is total.
        NameRefusal::Missing | NameRefusal::Malformed => INSTANCE_NAME_INVALID_CHARACTERS,
    }
}

/// See [`invalid_instance_name_message`]: a name outside the service's
/// resource-name character set, or longer than [`MAX_NAME_LENGTH`].
pub const INSTANCE_NAME_INVALID_CHARACTERS: &str = "Instance name contains invalid characters.";

/// See [`invalid_instance_name_message`] and
/// [`check_on_premises_instance_name`]: a name in the form reserved for Amazon
/// EC2 instance ids.
pub const INSTANCE_NAME_RESERVED_PREFIX: &str = "Host name cannot start with \"i-\"";

/// The prefix every Amazon EC2 instance id carries, which is therefore the
/// prefix no on-premises HOST NAME may carry: CodeDeploy identifies an
/// EC2/on-premises deployment target by that id, so a host registered under an
/// id-shaped name could not be told from an EC2 instance.
const EC2_INSTANCE_ID_PREFIX: &str = "i-";

/// The longest resource name CodeDeploy accepts, as the model states it for
/// every name member it constrains (`smithy_query(query="shape",shape="ApplicationName")`:
/// `@length(min: 1, max: 100)`).
pub const MAX_NAME_LENGTH: usize = 100;

/// The characters a CodeDeploy resource name is made of, as the model states it
/// for every name member it constrains
/// (`smithy_query(query="shape",shape="ApplicationName")`: pattern
/// `^[A-Za-z0-9+=,.@_-]*$`).
fn is_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || "+=,.@_-".contains(character)
}

/// Check `name` against the form CodeDeploy accepts for an on-premises instance
/// name: at least one character, at most [`MAX_NAME_LENGTH`], all of them from
/// the service's resource-name character set.
///
/// `InstanceName` carries no `@length` or `@pattern` of its own in the model, so
/// the boundary validator lets every string through and this is where the form
/// is decided.
pub fn check_instance_name(name: &str) -> Result<(), NameRefusal> {
    if name.trim().is_empty() {
        return Err(NameRefusal::Missing);
    }
    if name.chars().count() > MAX_NAME_LENGTH || !name.chars().all(is_name_character) {
        return Err(NameRefusal::Malformed);
    }
    Ok(())
}

/// Check `name` against the form CodeDeploy accepts for the name of an
/// ON-PREMISES INSTANCE: [`check_instance_name`]'s form, and not one of the
/// names the service RESERVES.
///
/// The reserved form is the Amazon EC2 instance id
/// ([`EC2_INSTANCE_ID_PREFIX`]), which `InvalidInstanceNameException` ("The
/// on-premises instance name was specified in an invalid format") is the refusal
/// for: no on-premises host can ever hold such a name, so every operation that
/// takes an `instanceName` of an on-premises instance — the registering,
/// deregistering, reading and tagging ones alike — refuses it HERE, before it
/// asks the registry anything, so a caller that passed an EC2 instance id by
/// mistake is never told its call succeeded.
///
/// A deployment TARGET id is not a host name: an EC2/on-premises deployment
/// reports its instance targets BY that id, so [`check_deployment_target_id`]
/// goes through [`check_instance_name`] and this reservation does not apply to
/// it.
pub fn check_on_premises_instance_name(name: &str) -> Result<(), NameRefusal> {
    check_instance_name(name)?;
    // Matched case-insensitively: the names this service accepts differ in case,
    // so a host could otherwise take the reserved form under another casing.
    if name
        .get(..EC2_INSTANCE_ID_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(EC2_INSTANCE_ID_PREFIX))
    {
        return Err(NameRefusal::Reserved);
    }
    Ok(())
}

/// Whether `candidate` is an ARN of `service` naming a `resource_type` resource.
///
/// Six colon-separated fields — `arn:<partition>:<service>:<region>:<account>:<resource>`
/// — is the ARN form AWS documents, with the resource part opening on the
/// resource type. IAM and STS ARNs are global, so the region field is allowed to
/// be empty; the partition, the account and the resource identifier are not.
fn is_arn_of(candidate: &str, service: &str, resource_type: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(found_service),
        Some(_region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    !partition.is_empty()
        && found_service == service
        && !account.is_empty()
        && resource
            .strip_prefix(resource_type)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|identifier| !identifier.is_empty())
}

/// Whether `candidate` is an IAM user ARN — `arn:aws:iam::<account>:user/<name>`,
/// the form `RegisterOnPremisesInstance$iamUserArn` documents ("The ARN of the
/// user to associate with the on-premises instance").
pub fn is_iam_user_arn(candidate: &str) -> bool {
    is_arn_of(candidate, "iam", "user")
}

/// Whether `candidate` is an IAM session ARN —
/// `arn:aws:sts::<account>:assumed-role/<role>/<session>`, the form an STS
/// assumed-role session carries, which is what
/// `RegisterOnPremisesInstance$iamSessionArn` associates with the instance.
pub fn is_iam_session_arn(candidate: &str) -> bool {
    is_arn_of(candidate, "sts", "assumed-role")
}

/// Whether `candidate` is an IAM role ARN —
/// `arn:<partition>:iam::<account>:role/<name>`, the form the IAM User Guide
/// gives for a role resource and the only thing a deployment group's
/// `serviceRoleArn` ("A service role Amazon Resource Name (ARN) that allows
/// CodeDeploy to act on the user's behalf",
/// `smithy_query(query="doc",shape="CreateDeploymentGroupInput",member="serviceRoleArn")`)
/// can name.
pub fn is_iam_role_arn(candidate: &str) -> bool {
    is_arn_of(candidate, "iam", "role")
}

/// The prefix every ARN opens with, whatever it names.
const ARN_PREFIX: &str = "arn:";

/// Why CodeDeploy will not take a value as a deployment group's service role.
///
/// The operations that SET one — creating a group, and replacing a group's role
/// — share this verdict, so a role the one accepts is a role the other accepts,
/// and each maps it to the modelled error IT declares for the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRefusal {
    /// The request carried no role at all: absent, empty, or only whitespace.
    Missing,
    /// The value is not an ARN: it does not even open with [`ARN_PREFIX`].
    NotAnArn,
    /// The value IS an ARN, but not one naming an IAM role.
    NotAnIamRole,
}

/// Read `candidate` as a deployment group's service role ARN.
///
/// The syntax is checked BEFORE anything is stored: a deployment group whose
/// service role is not a role ARN could never deploy, so the value is refused at
/// the call that named it rather than kept and failed far from there. The model
/// puts no `@pattern` on `Role` (`smithy_query(query="shape",shape="Role")`), so
/// the form a value must satisfy is the documented ARN form — and for a service
/// role, an IAM role's ([`is_iam_role_arn`]).
pub fn check_service_role_arn(candidate: &str) -> Result<(), RoleRefusal> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return Err(RoleRefusal::Missing);
    }
    if !candidate.starts_with(ARN_PREFIX) {
        return Err(RoleRefusal::NotAnArn);
    }
    if !is_iam_role_arn(candidate) {
        return Err(RoleRefusal::NotAnIamRole);
    }
    Ok(())
}

/// `InvalidRoleException` (HTTP 400): the sentence that refusal carries for each
/// way a service role ARN can fail [`check_service_role_arn`], so every
/// operation taking one refuses it in the same words. The value the caller sent
/// is quoted back bounded by [`echoed`], since `Role` carries no modelled
/// maximum length.
///
/// A value that is not an ARN at all is refused by the rule every ARN is read
/// by, which is the sentence AWS states for it; the other two are about the ARN
/// naming the wrong thing, which is what the error's own documented sentence
/// ("The service role ARN was specified in an invalid format") describes.
pub fn invalid_role_message(refusal: RoleRefusal, candidate: &str) -> String {
    match refusal {
        RoleRefusal::Missing => {
            "The service role ARN was specified in an invalid format: it is empty".to_string()
        }
        RoleRefusal::NotAnArn => {
            format!("ARNs must start with '{ARN_PREFIX}': {}", echoed(candidate))
        }
        RoleRefusal::NotAnIamRole => format!(
            "The service role ARN {} was specified in an invalid format",
            echoed(candidate)
        ),
    }
}

/// The resource-type segment of an application's ARN:
/// `arn:aws:codedeploy:<region>:<account>:application:<applicationName>`, the
/// form the CodeDeploy User Guide gives for an application resource.
const ARN_TYPE_APPLICATION: &str = "application";

/// The resource-type segment of a deployment group's ARN:
/// `…:deploymentgroup:<applicationName>/<deploymentGroupName>` — a group is
/// named INSIDE its application, which is why its identifier carries both names.
const ARN_TYPE_DEPLOYMENT_GROUP: &str = "deploymentgroup";

/// The resource-type segment of a deployment configuration's ARN:
/// `…:deploymentconfig:<deploymentConfigName>`.
const ARN_TYPE_DEPLOYMENT_CONFIG: &str = "deploymentconfig";

/// The ARN of an application; see [`ARN_TYPE_APPLICATION`]. The account segment
/// comes from the engine, never from a literal.
pub fn application_arn(region: &str, application_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_APPLICATION}:{application_name}"),
    )
}

/// The ARN of a deployment group; see [`ARN_TYPE_DEPLOYMENT_GROUP`].
pub fn deployment_group_arn(
    region: &str,
    application_name: &str,
    deployment_group_name: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_DEPLOYMENT_GROUP}:{application_name}/{deployment_group_name}"),
    )
}

/// The ARN of a deployment configuration; see [`ARN_TYPE_DEPLOYMENT_CONFIG`].
pub fn deployment_config_arn(region: &str, deployment_config_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_DEPLOYMENT_CONFIG}:{deployment_config_name}"),
    )
}

/// A resource this service's `ResourceArn` operations act on, with the identity
/// its ARN carries: the CodeDeploy resource kinds that hold tags of their own,
/// which an ARN naming any other kind of resource is refused against
/// ([`ArnRefusal::Unsupported`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// An application, by name.
    Application(String),
    /// A deployment group, by its application's name and its own.
    DeploymentGroup {
        application_name: String,
        deployment_group_name: String,
    },
}

/// Why CodeDeploy will not read a `ResourceArn` as one of its taggable
/// resources. Each operation maps it to the modelled error IT declares, so every
/// operation taking a `ResourceArn` refuses one by the same rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnRefusal {
    /// The value is not in the ARN format, or the identifier its resource part
    /// should carry is missing: `InvalidArnException`.
    Malformed,
    /// A well-formed ARN naming something else — another service's resource, or
    /// a CodeDeploy resource kind that is not tagged through these operations:
    /// `ArnNotSupportedException`.
    Unsupported,
}

/// Read `candidate` as the ARN of a resource this service tags.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and for
/// the taggable CodeDeploy kinds the resource part is
/// `<type>:<identifier>`. The region and the account are NOT matched against
/// this emulator's own: an ARN identifies the resource by the name it carries,
/// which is what the lookup uses.
pub fn parse_tagged_resource_arn(candidate: &str) -> Result<TaggedResource, ArnRefusal> {
    let mut fields = candidate.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(service),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return Err(ArnRefusal::Malformed);
    };
    if partition.is_empty() || resource.is_empty() {
        return Err(ArnRefusal::Malformed);
    }
    if service != ARN_SERVICE {
        return Err(ArnRefusal::Unsupported);
    }
    // Every taggable kind's resource part opens on its type; a CodeDeploy
    // resource whose ARN is not of that shape — an on-premises instance's
    // `instance/<name>`, say — is tagged through its own operations, so it is
    // not a resource THESE operations act on.
    let Some((kind, identifier)) = resource.split_once(':') else {
        return Err(ArnRefusal::Unsupported);
    };
    match kind {
        ARN_TYPE_APPLICATION => Ok(TaggedResource::Application(named(identifier)?)),
        ARN_TYPE_DEPLOYMENT_GROUP => {
            let (application_name, deployment_group_name) =
                identifier.split_once('/').ok_or(ArnRefusal::Malformed)?;
            Ok(TaggedResource::DeploymentGroup {
                application_name: named(application_name)?,
                deployment_group_name: named(deployment_group_name)?,
            })
        }
        // Every OTHER CodeDeploy resource type — a deployment configuration, a
        // deployment, an on-premises instance — is a resource these operations
        // do not act on, whether or not the store holds it: the resource type
        // alone decides, so it is refused before anything is looked up.
        _ => Err(ArnRefusal::Unsupported),
    }
}

/// One name out of an ARN's resource part: an ARN of a known kind that carries
/// no name for it names no resource, so it is not in a valid format.
fn named(identifier: &str) -> Result<String, ArnRefusal> {
    if identifier.is_empty() {
        return Err(ArnRefusal::Malformed);
    }
    Ok(identifier.to_string())
}

/// The resource a `ResourceArn` named that the store does not hold, so each
/// operation can answer it with the modelled error it declares for that kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingResource {
    /// No application of this name.
    Application(String),
    /// The application exists, but holds no deployment group of this name.
    DeploymentGroup(String),
}

/// A stored on-premises instance as the modelled `InstanceInfo`.
pub fn instance_info(instance: &OnPremisesInstance) -> crate::ops::types::InstanceInfo {
    crate::ops::types::InstanceInfo {
        instance_name: Some(instance.instance_name.clone()),
        iam_session_arn: instance.iam_session_arn.clone(),
        iam_user_arn: instance.iam_user_arn.clone(),
        instance_arn: Some(instance.instance_arn.clone()),
        register_time: Some(timestamp(instance.register_time_ms)),
        deregister_time: instance.deregister_time_ms.map(timestamp),
        // The tags an instance carries are reported as the list it HOLDS, empty
        // included: an instance registered with no tags and one whose tags were
        // all removed hold the same thing, so they read back the same way and a
        // client can index the member instead of handling absent and empty
        // separately.
        tags: Some(instance.tags.clone()),
    }
}

/// How many items one page of a listing carries.
///
/// CodeDeploy's list operations are `@paginated` on `nextToken` alone — neither
/// the model nor the API reference states a page size — so this one is CHOSEN.
pub const PAGE_SIZE: usize = 100;

/// Cut `items` into the page `token` asks for, with the token for the next page.
///
/// `None` means the token is not one this service issued, or names an item this
/// listing no longer carries: answer it with the operation's
/// `InvalidNextTokenException`. The token names the LAST item already returned,
/// not a position, so a second call continues after that item however many items
/// have been created in the meantime — an offset into a newest-first listing
/// would shift under every new record and repeat or skip one.
pub fn paginate(items: &[String], token: Option<&str>) -> Option<(Vec<String>, Option<String>)> {
    let start = match token {
        Some(token) => {
            let last_returned = decode_page_token(token)?;
            items.iter().position(|item| *item == last_returned)? + 1
        }
        None => 0,
    };
    if start > items.len() {
        return None;
    }
    let end = (start + PAGE_SIZE).min(items.len());
    let next = (end < items.len())
        .then(|| items.get(end - 1).map(|last| encode_page_token(last)))
        .flatten();
    Some((items[start..end].to_vec(), next))
}

/// Render the last item of a page as the opaque continuation token a client
/// echoes back.
///
/// Tagged and hex-rendered so it cannot be mistaken for — or hand-built from —
/// an item identifier.
fn encode_page_token(last_returned: &str) -> String {
    format!("p{}", hex::encode(last_returned))
}

/// Read back a token [`encode_page_token`] produced; `None` for anything else.
fn decode_page_token(token: &str) -> Option<String> {
    let encoded = token.strip_prefix('p')?;
    String::from_utf8(hex::decode(encoded).ok()?).ok()
}

/// A stored application as the modelled `ApplicationInfo`.
pub fn application_info(application: &Application) -> crate::ops::types::ApplicationInfo {
    crate::ops::types::ApplicationInfo {
        application_id: Some(application.application_id.clone()),
        application_name: Some(application.application_name.clone()),
        create_time: Some(timestamp(application.create_time_ms)),
        linked_to_git_hub: application.linked_to_git_hub,
        git_hub_account_name: application.git_hub_account_name.clone(),
        compute_platform: Some(application.compute_platform.clone()),
    }
}

/// A stored deployment configuration as the modelled `DeploymentConfigInfo`.
pub fn deployment_config_info(
    config: &DeploymentConfig,
) -> crate::ops::types::DeploymentConfigInfo {
    crate::ops::types::DeploymentConfigInfo {
        deployment_config_id: Some(config.deployment_config_id.clone()),
        deployment_config_name: Some(config.deployment_config_name.clone()),
        minimum_healthy_hosts: config.minimum_healthy_hosts.clone(),
        create_time: Some(timestamp(config.create_time_ms)),
        compute_platform: Some(config.compute_platform.clone()),
        traffic_routing_config: config.traffic_routing_config.clone(),
        zonal_config: config.zonal_config.clone(),
    }
}

/// A stored deployment as the modelled `DeploymentInfo`.
pub fn deployment_info(deployment: &Deployment) -> crate::ops::types::DeploymentInfo {
    crate::ops::types::DeploymentInfo {
        application_name: Some(deployment.application_name.clone()),
        deployment_group_name: deployment.deployment_group_name.clone(),
        deployment_config_name: Some(deployment.deployment_config_name.clone()),
        deployment_id: Some(deployment.deployment_id.clone()),
        previous_revision: deployment.previous_revision.clone(),
        revision: deployment.revision.clone(),
        status: Some(deployment.status.clone()),
        error_information: deployment_error_information(deployment),
        create_time: Some(timestamp(deployment.create_time_ms)),
        start_time: deployment.start_time_ms.map(timestamp),
        complete_time: deployment.complete_time_ms.map(timestamp),
        deployment_overview: None,
        description: deployment.description.clone(),
        creator: Some(deployment.creator.clone()),
        ignore_application_stop_failures: deployment.ignore_application_stop_failures,
        auto_rollback_configuration: deployment.auto_rollback_configuration.clone(),
        update_outdated_instances_only: deployment.update_outdated_instances_only,
        rollback_info: deployment.rollback_info.clone(),
        deployment_style: deployment.deployment_style.clone(),
        target_instances: deployment.target_instances.clone(),
        instance_termination_wait_time_started: deployment
            .instance_termination_wait_time_started,
        blue_green_deployment_configuration: None,
        load_balancer_info: None,
        additional_deployment_status_info: None,
        file_exists_behavior: deployment.file_exists_behavior.clone(),
        deployment_mode: deployment.deployment_mode.clone(),
        deployment_status_messages: None,
        compute_platform: Some(deployment.compute_platform.clone()),
        external_id: deployment.external_id.clone(),
        related_deployments: None,
        override_alarm_configuration: deployment.override_alarm_configuration.clone(),
        ec2_deployment_options: deployment.ec2_deployment_options.clone(),
    }
}

/// Why a failed deployment failed, as the modelled `ErrorInformation`.
///
/// A failure the service RECORDED on the deployment — a lifecycle event hook
/// reporting a failed validation, say — is reported as it was recorded; anything
/// else is DERIVED from the deployment's own record (the terminal status the
/// clock gave it and the targets its group resolved) rather than stored a second
/// time where it could disagree with the status beside it.
fn deployment_error_information(
    deployment: &Deployment,
) -> Option<crate::ops::types::ErrorInformation> {
    if let Some(recorded) = deployment.error_information.clone() {
        return Some(recorded);
    }
    (deployment.status == DEPLOYMENT_STATUS_FAILED && deployment.target_count() == 0).then(|| {
        crate::ops::types::ErrorInformation {
            code: Some(ERROR_CODE_NO_INSTANCES.to_string()),
            message: Some(NO_INSTANCES_MESSAGE.to_string()),
        }
    })
}

/// A stored deployment group as the modelled `DeploymentGroupInfo`.
///
/// `lastAttemptedDeployment` and `lastSuccessfulDeployment` are DERIVED from the
/// deployment index — the newest deployment of this group, and the newest one
/// that succeeded — rather than kept as a second copy that could disagree with
/// it. `targetRevision` is the revision of that last attempt, which is what
/// CodeDeploy reports as the group's target.
pub fn deployment_group_info(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    group: &DeploymentGroup,
) -> crate::ops::types::DeploymentGroupInfo {
    let of_group = |deployment: &&Deployment| {
        deployment.application_name == group.application_name
            && deployment.deployment_group_name.as_deref()
                == Some(group.deployment_group_name.as_str())
    };
    let newest = |predicate: &dyn Fn(&&Deployment) -> bool| {
        state
            .deployment_order
            .iter()
            .rev()
            .filter_map(|id| state.deployments.get(id))
            .find(|deployment| of_group(deployment) && predicate(deployment))
            .cloned()
    };
    let last_attempted = newest(&|_| true);
    let last_successful = newest(&|deployment| deployment.status == DEPLOYMENT_STATUS_SUCCEEDED);
    let as_last_deployment = |deployment: &Deployment| crate::ops::types::LastDeploymentInfo {
        deployment_id: Some(deployment.deployment_id.clone()),
        status: Some(deployment.status.clone()),
        end_time: deployment.complete_time_ms.map(timestamp),
        create_time: Some(timestamp(deployment.create_time_ms)),
    };

    crate::ops::types::DeploymentGroupInfo {
        application_name: Some(group.application_name.clone()),
        deployment_group_id: Some(group.deployment_group_id.clone()),
        deployment_group_name: Some(group.deployment_group_name.clone()),
        deployment_config_name: Some(group.deployment_config_name.clone()),
        // The group's instance-selection and trigger lists are reported as the
        // lists the group HOLDS, empty included: a request that sent one empty
        // and a request that omitted it gave the group the same thing, so they
        // read back the same way and a client can index the member instead of
        // handling absent and empty separately.
        ec2_tag_filters: Some(group.ec2_tag_filters.clone()),
        on_premises_instance_tag_filters: Some(group.on_premises_instance_tag_filters.clone()),
        auto_scaling_groups: Some(
            group
                .auto_scaling_groups
                .iter()
                .map(|name| crate::ops::types::AutoScalingGroup {
                    name: Some(name.clone()),
                    ..Default::default()
                })
                .collect(),
        ),
        service_role_arn: Some(group.service_role_arn.clone()),
        target_revision: last_attempted
            .as_ref()
            .and_then(|deployment| deployment.revision.clone()),
        trigger_configurations: Some(group.trigger_configurations.clone()),
        alarm_configuration: group.alarm_configuration.clone(),
        auto_rollback_configuration: group.auto_rollback_configuration.clone(),
        deployment_style: group.deployment_style.clone(),
        outdated_instances_strategy: group.outdated_instances_strategy.clone(),
        blue_green_deployment_configuration: group.blue_green_deployment_configuration.clone(),
        load_balancer_info: group.load_balancer_info.clone(),
        last_successful_deployment: last_successful.as_ref().map(as_last_deployment),
        last_attempted_deployment: last_attempted.as_ref().map(as_last_deployment),
        ec2_tag_set: group.ec2_tag_set.clone(),
        on_premises_tag_set: group.on_premises_tag_set.clone(),
        compute_platform: Some(group.compute_platform.clone()),
        // `ecsServices` is reported only when the group has any: the model scopes
        // the member to one compute platform ("This applies only to deployment
        // groups that use the Amazon ECS compute platform"), so an empty list is
        // not something every group holds.
        ecs_services: non_empty(&group.ecs_services),
        ec2_deployment_options: group.ec2_deployment_options.clone(),
        termination_hook_enabled: group.termination_hook_enabled,
        // The bake time and the managed-hook configuration are reported for
        // EVERY group, whether or not it configured either: a group that
        // configured no bake time reports no wait at all, and a group with no
        // managed-hook execution role reports the sentinel for "none" rather
        // than leaving the structure out.
        bake_time: group.bake_time,
        managed_hook_config: Some(crate::ops::types::ManagedHookConfig {
            execution_role_arn: Some(
                group
                    .managed_hook_execution_role_arn
                    .clone()
                    .unwrap_or_else(|| MANAGED_HOOK_EXECUTION_ROLE_NONE.to_string()),
            ),
        }),
    }
}

/// A stored list as an optional modelled list: absent when nothing was stored,
/// for a member a resource does not always carry.
fn non_empty<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// A stored revision as the modelled `GenericRevisionInfo`, with the deployment
/// groups this revision is the current target revision of.
///
/// `deploymentGroups` is DERIVED from the deployment index — the groups whose
/// last attempted deployment deployed this revision — rather than kept as a
/// second copy that could disagree with it. `targets` is the one
/// [`target_revision_keys`] map the caller built for this request, so projecting
/// a page of revisions walks the index ONCE however many revisions it carries.
/// It is reported as the list the revision HAS, empty included: every revision
/// of every application has such a set of groups, so a client can index the
/// member rather than handling absent and empty separately.
pub fn generic_revision_info(
    application: &Application,
    targets: &HashMap<String, String>,
    key: &str,
) -> crate::ops::types::GenericRevisionInfo {
    let revision = application.revisions.get(key);
    crate::ops::types::GenericRevisionInfo {
        description: revision.and_then(|revision| revision.description.clone()),
        deployment_groups: Some(deployment_groups_targeting(application, targets, key)),
        first_used_time: revision
            .and_then(|revision| revision.first_used_time_ms)
            .map(timestamp),
        last_used_time: revision
            .and_then(|revision| revision.last_used_time_ms)
            .map(timestamp),
        register_time: revision.map(|revision| timestamp(revision.register_time_ms)),
    }
}

/// The names of `application`'s deployment groups whose target revision — the
/// revision of their last attempted deployment, from the `targets` map — is the
/// revision `key` names, in the application's own group order so the answer is
/// deterministic.
pub fn deployment_groups_targeting(
    application: &Application,
    targets: &HashMap<String, String>,
    key: &str,
) -> Vec<String> {
    application
        .deployment_group_order
        .iter()
        .filter(|group| targets.get(group.as_str()).map(String::as_str) == Some(key))
        .cloned()
        .collect()
}

/// The target revision of each of `application`'s deployment groups that has
/// one: the [`revision_key`] of its last attempted deployment, by group name.
///
/// A group's target revision is the revision of its last attempted deployment,
/// which is what `DeploymentGroupInfo.targetRevision` reports; it is DERIVED
/// from the deployment index rather than kept as a second copy that could
/// disagree with it.
///
/// ONE reverse pass over the deployment index answers every group at once — the
/// newest deployment of a group is the first one the pass meets — and it reads
/// each deployment's STORED revision key, so nothing here re-hashes a revision's
/// content. Build it once per request and hand it to the projection.
pub fn target_revision_keys(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    application: &Application,
) -> HashMap<String, String> {
    let mut targets: HashMap<String, String> = HashMap::new();
    for deployment in state
        .deployment_order
        .iter()
        .rev()
        .filter_map(|id| state.deployments.get(id))
    {
        if deployment.application_name != application.application_name {
            continue;
        }
        let (Some(group), Some(key)) = (
            deployment.deployment_group_name.clone(),
            deployment.revision_key.clone(),
        ) else {
            continue;
        };
        targets.entry(group).or_insert(key);
    }
    targets
}

/// The identity of an application revision: which artifact it names, not how it
/// is packaged.
///
/// `None` for a `RevisionLocation` that names no location at all — the model
/// holds one location per `revisionType`
/// (`smithy_query(query="shape",shape="RevisionLocation")`: s3Location,
/// gitHubLocation, string, appSpecContent) and `revisionType` itself only says
/// which of them to read — which is what `InvalidRevisionException` describes.
///
/// `s3Location.bundleType` is NOT part of the identity: it says how to unpack
/// the object, not which object it is. The inline `string` and `appSpecContent`
/// revisions are identified by the digest of their content, so two requests
/// carrying the same AppSpec name the same revision however they were sent.
pub fn revision_key(revision: &RevisionLocation) -> Option<String> {
    if let Some(s3) = &revision.s3_location {
        let field = |value: &Option<String>| value.clone().unwrap_or_default();
        return Some(format!(
            "S3|{}|{}|{}|{}",
            field(&s3.bucket),
            field(&s3.key),
            field(&s3.version),
            field(&s3.e_tag)
        ));
    }
    if let Some(github) = &revision.git_hub_location {
        let field = |value: &Option<String>| value.clone().unwrap_or_default();
        return Some(format!(
            "GitHub|{}|{}",
            field(&github.repository),
            field(&github.commit_id)
        ));
    }
    if let Some(raw) = &revision.string {
        return Some(format!(
            "String|{}",
            content_digest(raw.content.as_deref().unwrap_or_default())
        ));
    }
    if let Some(app_spec) = &revision.app_spec_content {
        return Some(format!(
            "AppSpecContent|{}",
            content_digest(app_spec.content.as_deref().unwrap_or_default())
        ));
    }
    None
}

/// The hex SHA-256 of an inline revision's content, which is what identifies it.
fn content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Run `body` against this service's store, with the built-ins seeded and every
/// clock-owed deployment transition already applied.
///
/// EVERY handler goes through here, so no handler can observe an unseeded store,
/// a deployment that should already have completed, one its deployment group's
/// service role never authorized, one the alarms its group polls have already
/// stopped, or one reporting success for a traffic shift that was never carried
/// out. `None` means the engine is not the shared in-memory one
/// — answer it with a modelled server error.
pub fn with_state<Eng: Send + Sync + 'static, R>(
    engine: &Eng,
    body: impl FnOnce(&mut ComAmazonawsCodedeployCodedeploy20141006State) -> R,
) -> Option<R> {
    let state = service_state(engine)?;
    // The revision of every deployment that has reached for one is resolved
    // FIRST: what a fetched AppSpec declares is what the deployment performs, so
    // it is read before the role is asked about those calls and before any
    // traffic moves — and a deployment whose revision cannot be located fails
    // here, having done nothing.
    s3_revision::resolve_due_revisions(engine);
    // The service role the deployments in flight run as is consulted next: a
    // deployment its role does not authorize must have no effect at all, so it is
    // failed before any alarm is polled and before any traffic moves.
    service_role_permissions::refuse_unauthorized_deployments(engine);
    // The alarm gate reads CloudWatch's own alarm store, so it runs with THIS
    // store unlocked — the two are never held at once — and BEFORE the clock
    // settles anything: a deployment the alarms had already stopped must not
    // reach a successful outcome at the instant its stop was owed.
    alarm_gate::poll_alarms(engine);
    // The traffic shift writes AWS Lambda's own function store, with this store
    // unlocked for the same reason, and runs after the gate and before the clock:
    // a deployment the alarms stopped shifts no traffic, and no deployment is
    // given a successful outcome before the increments its revision and its
    // configuration described have happened.
    lambda_traffic_shift::apply_due_shifts(engine);
    let mut guard = state.lock();
    let now = now_ms();
    guard.ensure_seeded(now);
    guard.advance_clock(now);
    let answered = body(&mut guard);
    // The notifications this pass came to owe the deployment groups' triggers:
    // what the clock materialised above, and what the operation itself did — a
    // stop, a failed validation function — so one place sees every lifecycle
    // event of the pass. Queued here and published by the caller with this store
    // unlocked (`trigger_notifications::deliver`).
    guard.queue_due_trigger_notifications();
    Some(answered)
}

#[cfg(test)]
mod tests {
    //! What [`deployment_target_count`] resolves, over a store built in place.
    //!
    //! These tables span the selector forms exhaustively and cheaply — every
    //! `TagFilterType`, a filter naming only one member, an empty tag set, a
    //! deregistered instance — where building the same row over the wire costs a
    //! registration, a tagging call and a deployment each. The rule they pin is
    //! also asserted end-to-end, through the operations a client calls, in
    //! `tests/all/codedeploy_deployment_outcome.rs`.

    use super::*;

    /// A deployment group of `platform` carrying no selector at all; each case
    /// below sets the one member it is about.
    fn group(platform: &str) -> DeploymentGroup {
        DeploymentGroup {
            deployment_group_id: "dgid".to_string(),
            deployment_group_name: "dg".to_string(),
            application_name: "app".to_string(),
            service_role_arn: "arn:aws:iam::123456789012:role/CodeDeployRole".to_string(),
            deployment_config_name: "CodeDeployDefault.OneAtATime".to_string(),
            compute_platform: platform.to_string(),
            ec2_tag_filters: Vec::new(),
            on_premises_instance_tag_filters: Vec::new(),
            auto_scaling_groups: Vec::new(),
            trigger_configurations: Vec::new(),
            alarm_configuration: None,
            auto_rollback_configuration: None,
            outdated_instances_strategy: None,
            deployment_style: None,
            blue_green_deployment_configuration: None,
            load_balancer_info: None,
            ec2_tag_set: None,
            on_premises_tag_set: None,
            ecs_services: Vec::new(),
            ec2_deployment_options: None,
            termination_hook_enabled: false,
            bake_time: NO_BAKE_TIME,
            managed_hook_execution_role_arn: None,
            tags: Vec::new(),
            create_time_ms: 0,
            region: "us-east-1".to_string(),
        }
    }

    fn tag(key: &str, value: &str) -> Tag {
        Tag {
            key: Some(key.to_string()),
            value: Some(value.to_string()),
        }
    }

    /// One on-premises tag filter: `kind` is a `TagFilterType` value, or `None`
    /// for a filter that states no type.
    fn filter(key: Option<&str>, value: Option<&str>, kind: Option<&str>) -> TagFilter {
        TagFilter {
            key: key.map(str::to_string),
            value: value.map(str::to_string),
            r#type: kind.map(str::to_string),
        }
    }

    /// Put one on-premises instance in the registry.
    fn register(
        state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
        name: &str,
        tags: Vec<Tag>,
        deregistered: bool,
    ) {
        let registration_id = mint_uuid_from(&mut state.next_id, "on-premises-registration");
        state.register_on_premises_instance(OnPremisesInstance {
            instance_name: name.to_string(),
            instance_arn: instance_arn("us-east-1", name, &registration_id),
            iam_user_arn: Some(format!("arn:aws:iam::123456789012:user/{name}")),
            iam_session_arn: None,
            register_time_ms: 1,
            deregister_time_ms: deregistered.then_some(2),
            tags,
        });
    }

    /// RULE: an EC2/on-premises deployment group targets the REGISTERED
    /// on-premises instances its `onPremisesInstanceTagFilters` (any filter) or
    /// its `onPremisesTagSet` (one filter from every group) select.
    #[test]
    fn a_server_group_targets_the_on_premises_instances_it_selects() {
        // Rows differ along the selector the group carries, the `TagFilterType`
        // it uses, and whether the instance's tags satisfy it.
        struct Case {
            case: &'static str,
            filters: Vec<TagFilter>,
            tag_set: Option<Vec<Vec<TagFilter>>>,
            expected: usize,
        }
        let cases = vec![
            Case {
                case: "no selector at all",
                filters: Vec::new(),
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE matching both instances",
                filters: vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "KEY_AND_VALUE matching neither",
                filters: vec![filter(Some("Env"), Some("staging"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE naming only the key",
                filters: vec![filter(Some("Env"), None, Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_AND_VALUE naming only the value",
                filters: vec![filter(None, Some("prod"), Some("KEY_AND_VALUE"))],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "KEY_ONLY, which ignores the value",
                filters: vec![filter(Some("Rack"), Some("ignored"), Some("KEY_ONLY"))],
                tag_set: None,
                expected: 1,
            },
            Case {
                case: "VALUE_ONLY, which ignores the key",
                filters: vec![filter(Some("ignored"), Some("seven"), Some("VALUE_ONLY"))],
                tag_set: None,
                expected: 1,
            },
            Case {
                case: "no type, so the members the filter names",
                filters: vec![filter(Some("Env"), Some("prod"), None)],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "a filter naming nothing",
                filters: vec![filter(None, None, None)],
                tag_set: None,
                expected: 0,
            },
            Case {
                case: "two filters, either of which includes an instance",
                filters: vec![
                    filter(Some("Rack"), Some("seven"), Some("KEY_AND_VALUE")),
                    filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE")),
                ],
                tag_set: None,
                expected: 2,
            },
            Case {
                case: "a tag set whose every group matches one instance",
                filters: Vec::new(),
                tag_set: Some(vec![
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    vec![filter(Some("Rack"), Some("seven"), Some("KEY_AND_VALUE"))],
                ]),
                expected: 1,
            },
            Case {
                case: "a tag set with a group nothing matches",
                filters: Vec::new(),
                tag_set: Some(vec![
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    vec![filter(Some("Rack"), Some("nine"), Some("KEY_AND_VALUE"))],
                ]),
                expected: 0,
            },
            Case {
                case: "an empty tag set",
                filters: Vec::new(),
                tag_set: Some(Vec::new()),
                expected: 0,
            },
        ];

        for case in cases {
            let mut state = ComAmazonawsCodedeployCodedeploy20141006State::default();
            register(
                &mut state,
                "rack-seven",
                vec![tag("Env", "prod"), tag("Rack", "seven")],
                false,
            );
            register(&mut state, "plain", vec![tag("Env", "prod")], false);
            // Deregistered, and tagged exactly like the first: it is a record,
            // not a target.
            register(
                &mut state,
                "retired",
                vec![tag("Env", "prod"), tag("Rack", "seven")],
                true,
            );

            let mut group = group("Server");
            group.on_premises_instance_tag_filters = case.filters;
            group.on_premises_tag_set = case.tag_set.map(|list| OnPremisesTagSet {
                on_premises_tag_set_list: Some(list),
            });
            let got = deployment_target_count(&state, &group);
            assert_eq!(
                got, case.expected,
                "case {:?}: expected {} target(s), got {got}",
                case.case, case.expected
            );
        }
    }

    /// RULE: an EC2 selector resolves nothing here — CodeDeploy's state keeps no
    /// EC2 inventory — while an ECS group's targets are its `ecsServices` and a
    /// Lambda group has exactly one target.
    #[test]
    fn the_other_selectors_and_platforms_resolve_their_own_targets() {
        let mut state = ComAmazonawsCodedeployCodedeploy20141006State::default();
        register(&mut state, "rack-seven", vec![tag("Env", "prod")], false);

        let ec2_filter = EC2TagFilter {
            key: Some("Env".to_string()),
            value: Some("prod".to_string()),
            r#type: Some("KEY_AND_VALUE".to_string()),
        };
        let mut by_ec2_filter = group("Server");
        by_ec2_filter.ec2_tag_filters = vec![ec2_filter.clone()];
        let mut by_ec2_tag_set = group("Server");
        by_ec2_tag_set.ec2_tag_set = Some(EC2TagSet {
            ec2_tag_set_list: Some(vec![vec![ec2_filter]]),
        });
        let mut by_auto_scaling = group("Server");
        by_auto_scaling.auto_scaling_groups = vec!["asg-web".to_string()];
        let mut one_service = group("ECS");
        one_service.ecs_services = vec![ECSService {
            cluster_name: Some("cluster".to_string()),
            service_name: Some("web".to_string()),
        }];
        let mut two_services = group("ECS");
        two_services.ecs_services = vec![
            ECSService {
                cluster_name: Some("cluster".to_string()),
                service_name: Some("web".to_string()),
            },
            ECSService {
                cluster_name: Some("cluster".to_string()),
                service_name: Some("api".to_string()),
            },
        ];

        let cases: &[(&str, DeploymentGroup, usize)] = &[
            ("an ec2 tag filter", by_ec2_filter, 0),
            ("an ec2 tag set", by_ec2_tag_set, 0),
            ("an auto scaling group", by_auto_scaling, 0),
            ("an ecs group with no service", group("ECS"), 0),
            ("an ecs group with one service", one_service, 1),
            ("an ecs group with two services", two_services, 2),
            ("a lambda group", group("Lambda"), 1),
        ];

        for (case, group, expected) in cases {
            let got = deployment_target_count(&state, group);
            assert_eq!(
                got, *expected,
                "case {case:?}: expected {expected} target(s), got {got}"
            );
            assert_eq!(
                terminal_status(got),
                if *expected == 0 {
                    DEPLOYMENT_STATUS_FAILED
                } else {
                    DEPLOYMENT_STATUS_SUCCEEDED
                },
                "case {case:?}: expected the terminal status its targets imply, got {got} target(s)"
            );
        }
    }
}
