// SPDX-License-Identifier: CC0-1.0

//! The LIFECYCLE EVENT HOOKS a deployment runs, and the gate they hold it at.
//!
//! A deployment of the Lambda or Amazon ECS compute platform runs the lifecycle
//! events its revision's AppSpec declares a hook for through a Lambda VALIDATION
//! FUNCTION: "Sets the result of a Lambda validation function. The function
//! validates lifecycle hooks during a deployment that uses the Lambda or Amazon
//! ECS compute platform […] Lambda validation functions return Succeeded or
//! Failed" (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatus")`).
//! So such a hook is not a record: CodeDeploy INVOKES the function with the
//! lifecycle event, and the result the function reports back under that event's
//! own execution id is what decides whether the deployment goes on.
//!
//! Three things follow, and they are the whole of this module:
//!
//! * a deployment HOLDS one hook execution per hook its revision declares, in the
//!   order its compute platform runs them ([`declared_executions`]);
//! * each is DISPATCHED in that order, and only once the one before it reported
//!   `Succeeded` ([`claim_due_hooks`] is that claim; the invocation itself is
//!   [`super::hook_invocation`]'s);
//! * the deployment waits at the gate those reports hold: traffic does not move
//!   while a hook that runs before it has not reported `Succeeded`
//!   ([`traffic_is_allowed`]), the deployment reaches no terminal status while any
//!   of them has not ([`hooks_are_complete`]), and a hook that reports `Failed`
//!   fails the deployment ([`super::rollback`] then rolls it back when its
//!   configuration asks for that).
//!
//! What the AppSpec DECLARES is [`super::appspec`]'s to read; this module is what
//! a deployment does with it.

use super::appspec::DeclaredHook;
use super::lambda_traffic_shift::TrafficShiftProgress;
use super::{
    deployment_is_terminal, ComAmazonawsCodedeployCodedeploy20141006State, Deployment,
    LIFECYCLE_EVENT_STATUS_FAILED, LIFECYCLE_EVENT_STATUS_SUCCEEDED,
};
use crate::ops::types::RevisionLocation;

/// The lifecycle event whose hook runs BEFORE traffic reaches the revision being
/// deployed, from this service's own documentation of the Lambda hooks ("the
/// available lifecycle hooks are BeforeAllowTraffic and AfterAllowTraffic").
pub const LIFECYCLE_EVENT_BEFORE_ALLOW_TRAFFIC: &str = "BeforeAllowTraffic";

/// See [`LIFECYCLE_EVENT_BEFORE_ALLOW_TRAFFIC`]: the one lifecycle event whose
/// hook runs AFTER traffic has moved, which is why it is the one hook this module
/// holds back until the shift is carried out.
pub const LIFECYCLE_EVENT_AFTER_ALLOW_TRAFFIC: &str = "AfterAllowTraffic";

/// The lifecycle event that MOVES the traffic, which sits between the two hooks
/// above: the AWS CodeDeploy User Guide's lifecycle events for an Lambda
/// deployment are BeforeAllowTraffic, AllowTraffic and AfterAllowTraffic, and only
/// the two around it can carry a validation function — "the available lifecycle
/// hooks are BeforeAllowTraffic and AfterAllowTraffic"
/// (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatus")`).
///
/// So it is not a hook a revision declares, but it IS a lifecycle event of the
/// deployment, and the one whose work this emulator performs: the alias move
/// ([`super::lambda_traffic_shift`]). [`allow_traffic_event`] reports it from that
/// work's own progress.
pub const LIFECYCLE_EVENT_ALLOW_TRAFFIC: &str = "AllowTraffic";

/// The lifecycle hooks a Lambda validation function reports on, per compute
/// platform, from this service's own documentation: "For Lambda deployments, the
/// available lifecycle hooks are BeforeAllowTraffic and AfterAllowTraffic. For
/// Amazon ECS deployments, the available lifecycle hooks are BeforeInstall,
/// AfterInstall, AfterAllowTestTraffic, BeforeAllowTraffic, and AfterAllowTraffic"
/// (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatus")`).
///
/// `None` for every other compute platform: EC2/on-premises lifecycle events are
/// run by the CodeDeploy agent rather than by a Lambda validation function, so
/// such a deployment issues no hook execution id and has no hook execution an id
/// could name. How many hooks a platform has is also how many hook executions one
/// deployment can ever have, and the ORDER here is the order they run in.
#[must_use]
pub fn platform_hooks(compute_platform: &str) -> Option<&'static [&'static str]> {
    match compute_platform {
        "Lambda" => Some(&[
            LIFECYCLE_EVENT_BEFORE_ALLOW_TRAFFIC,
            LIFECYCLE_EVENT_AFTER_ALLOW_TRAFFIC,
        ]),
        "ECS" => Some(&[
            "BeforeInstall",
            "AfterInstall",
            "AfterAllowTestTraffic",
            LIFECYCLE_EVENT_BEFORE_ALLOW_TRAFFIC,
            LIFECYCLE_EVENT_AFTER_ALLOW_TRAFFIC,
        ]),
        _ => None,
    }
}

/// One hook execution of a deployment: which lifecycle event, which validation
/// function, and how far that execution has got.
///
/// The execution id is minted when the hook is DISPATCHED, because it is what
/// CodeDeploy hands the function in the lifecycle event and what the function
/// reports back under; a hook not yet dispatched has none.
#[derive(Debug, Clone)]
pub struct HookExecution {
    /// Which of the deployment's lifecycle events this execution is for.
    pub lifecycle_event_name: String,
    /// The validation function the revision's AppSpec names for it.
    pub function_name: String,
    /// The id this execution reports under, minted at dispatch.
    pub lifecycle_event_hook_execution_id: Option<String>,
    /// The `LifecycleEventStatus` the function reported, once it has reported one.
    pub status: Option<String>,
    /// When that report arrived.
    pub update_time_ms: Option<i64>,
}

impl HookExecution {
    /// Whether this hook runs after traffic has moved, which is the one hook the
    /// traffic shift is not gated on and that is not dispatched until the shift is
    /// carried out.
    #[must_use]
    pub fn runs_after_traffic(&self) -> bool {
        self.lifecycle_event_name == LIFECYCLE_EVENT_AFTER_ALLOW_TRAFFIC
    }

    /// Whether this execution reported `Succeeded`, which is what lets the
    /// deployment past it.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.status.as_deref() == Some(LIFECYCLE_EVENT_STATUS_SUCCEEDED)
    }

    /// Whether the function has reported a result for this execution at all.
    #[must_use]
    pub fn reported(&self) -> bool {
        self.status.is_some()
    }
}

/// The `LifecycleEventStatus` of the lifecycle event that moves the traffic, for a
/// deployment that performs a shift — `None` for one that performs none, which has
/// no traffic to allow.
///
/// The event is as far along as the SHIFT is: every increment carried out
/// (`Succeeded`), the deployment failed before that (`Failed`), a caller or an
/// alarm stopped it so the rest of the move never ran (`Skipped`), the first
/// increment under way or already applied (`InProgress`), and otherwise not
/// reached yet (`Pending`).
#[must_use]
pub fn allow_traffic_event(deployment: &Deployment) -> Option<&'static str> {
    deployment.lambda_traffic_shift.as_ref()?;
    let moved = !deployment.traffic_increments.is_empty()
        && deployment.traffic_increments_applied >= deployment.traffic_increments.len();
    if moved {
        return Some(super::LIFECYCLE_EVENT_STATUS_SUCCEEDED);
    }
    Some(match deployment.status.as_str() {
        super::DEPLOYMENT_STATUS_FAILED => super::LIFECYCLE_EVENT_STATUS_FAILED,
        super::DEPLOYMENT_STATUS_STOPPED => super::LIFECYCLE_EVENT_STATUS_SKIPPED,
        // A deployment that reached `Succeeded` moved every increment, so the
        // branch above has already answered for it.
        _ if deployment.traffic_increments_applied > 0 => {
            super::LIFECYCLE_EVENT_STATUS_IN_PROGRESS
        }
        _ if traffic_is_allowed(deployment) && deployment.is_started() => {
            super::LIFECYCLE_EVENT_STATUS_IN_PROGRESS
        }
        _ => super::LIFECYCLE_EVENT_STATUS_PENDING,
    })
}

/// The hook executions `revision` declares for a deployment of
/// `compute_platform`, in the order that platform runs them.
///
/// Only a platform whose lifecycle events are validated by a Lambda function has
/// them ([`platform_hooks`]), and only an event that platform runs is kept: a
/// document naming an event the platform has no hook for declares no hook
/// execution of this deployment, so nothing is invoked for it. The event's name is
/// matched without regard to case and the hook takes the platform's own spelling
/// of it, which is the name the lifecycle event is reported under.
#[must_use]
pub fn declared_executions(
    compute_platform: &str,
    revision: &RevisionLocation,
) -> Vec<HookExecution> {
    executions_of(
        compute_platform,
        &super::appspec::declared_hooks_in_revision(revision),
    )
}

/// [`declared_executions`] for a deployment whose AppSpec document was fetched
/// rather than carried by the request — a revision that names an S3 object (see
/// [`super::s3_revision`]) — so both read one document the same way.
#[must_use]
pub fn declared_executions_in_app_spec(
    compute_platform: &str,
    content: &str,
) -> Vec<HookExecution> {
    executions_of(
        compute_platform,
        &super::appspec::declared_hooks_in_app_spec(content),
    )
}

/// The hook executions `declared` describes for a deployment of
/// `compute_platform`; see [`declared_executions`].
fn executions_of(compute_platform: &str, declared: &[DeclaredHook]) -> Vec<HookExecution> {
    let Some(order) = platform_hooks(compute_platform) else {
        return Vec::new();
    };
    order
        .iter()
        .filter_map(|event| {
            declared
                .iter()
                .find(|hook: &&DeclaredHook| {
                    hook.lifecycle_event_name.eq_ignore_ascii_case(event)
                })
                .map(|hook| HookExecution {
                    lifecycle_event_name: (*event).to_string(),
                    function_name: hook.function_name.clone(),
                    lifecycle_event_hook_execution_id: None,
                    status: None,
                    update_time_ms: None,
                })
        })
        .collect()
}

/// Whether `deployment` may move traffic: every hook of its that runs before
/// traffic does has reported `Succeeded`.
///
/// A deployment that declares no such hook may move traffic straight away, which
/// is every deployment whose revision carries no `Hooks` section.
#[must_use]
pub fn traffic_is_allowed(deployment: &Deployment) -> bool {
    deployment
        .hook_executions
        .iter()
        .filter(|hook| !hook.runs_after_traffic())
        .all(HookExecution::succeeded)
}

/// Whether every hook `deployment` declares has reported `Succeeded`, which is
/// what its own terminal status waits on.
#[must_use]
pub fn hooks_are_complete(deployment: &Deployment) -> bool {
    deployment.hook_executions.iter().all(HookExecution::succeeded)
}

/// Whether a hook of `deployment` reported `Failed`, which is what fails it.
#[must_use]
pub fn a_hook_failed(deployment: &Deployment) -> bool {
    deployment
        .hook_executions
        .iter()
        .any(|hook| hook.status.as_deref() == Some(LIFECYCLE_EVENT_STATUS_FAILED))
}

/// Whether `deployment` has moved the traffic its revision declared, so the hooks
/// that run after traffic are due.
///
/// A deployment that declares no shift has none to wait for; one that does is past
/// it only once every increment has been carried out
/// ([`TrafficShiftProgress::Applied`]).
fn traffic_has_moved(deployment: &Deployment) -> bool {
    deployment.lambda_traffic_shift.is_none()
        || deployment.traffic_shift == TrafficShiftProgress::Applied
}

/// One hook execution claimed for dispatch, with everything the invocation needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    /// The deployment the hook belongs to.
    pub deployment_id: String,
    /// The lifecycle event it is for, which is what a failure names.
    pub lifecycle_event_name: String,
    /// The validation function to invoke.
    pub function_name: String,
    /// The execution id the lifecycle event carries, which is what the function
    /// reports back under.
    pub lifecycle_event_hook_execution_id: String,
}

/// Claim the hook execution of every deployment that is due one at `now_ms`:
/// mint its execution id, so the store records it as dispatched, and answer what
/// the invocation needs.
///
/// A hook is due once the deployment has STARTED, every hook before it reported
/// `Succeeded`, and — for the hook that runs after traffic — the shift has been
/// carried out. Minting the id under this same lock is what claims it: a hook that
/// already carries one is never dispatched again, however many passes run.
pub fn claim_due_hooks(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<Dispatch> {
    let pending: Vec<String> = state.pending_deployments.clone();
    let mut claimed = Vec::new();
    for deployment_id in pending {
        let Some(deployment) = state.deployments.get(&deployment_id) else {
            continue;
        };
        let Some(index) = due_hook(deployment, now_ms) else {
            continue;
        };
        let execution_id = state.mint_hook_execution_id();
        let Some(deployment) = state.deployments.get_mut(&deployment_id) else {
            continue;
        };
        let Some(hook) = deployment.hook_executions.get_mut(index) else {
            continue;
        };
        hook.lifecycle_event_hook_execution_id = Some(execution_id.clone());
        claimed.push(Dispatch {
            deployment_id,
            lifecycle_event_name: hook.lifecycle_event_name.clone(),
            function_name: hook.function_name.clone(),
            lifecycle_event_hook_execution_id: execution_id,
        });
    }
    claimed
}

/// The position of the hook execution `deployment` owes a dispatch at `now_ms`, or
/// `None` when it owes none; see [`claim_due_hooks`].
fn due_hook(deployment: &Deployment, now_ms: i64) -> Option<usize> {
    if deployment.complete_time_ms.is_some()
        || deployment_is_terminal(&deployment.status)
        || deployment.starts_at_ms > now_ms
    {
        return None;
    }
    let (index, hook) = deployment
        .hook_executions
        .iter()
        .enumerate()
        .find(|(_, hook)| !hook.succeeded())?;
    // The hooks run in order, so the first one that has not succeeded is the only
    // one that can be due — and it is not due while it is already running, has
    // already reported, or is one that waits for traffic to move.
    if hook.lifecycle_event_hook_execution_id.is_some() || hook.reported() {
        return None;
    }
    if hook.runs_after_traffic() && !traffic_has_moved(deployment) {
        return None;
    }
    Some(index)
}

/// Record on `deployment_id` the result its hook execution `execution_id`
/// reported, answering the lifecycle event it was for.
///
/// `None` when the id names no dispatched hook execution of that deployment, and
/// [`AlreadyReported`] when that execution has already reported a result: a hook
/// execution reports once, because its lifecycle event completed with that result.
pub fn record_report(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    execution_id: &str,
    status: &str,
    now_ms: i64,
) -> Option<Result<String, AlreadyReported>> {
    let deployment = state.deployments.get_mut(deployment_id)?;
    let hook = deployment.hook_executions.iter_mut().find(|hook| {
        hook.lifecycle_event_hook_execution_id.as_deref() == Some(execution_id)
    })?;
    if hook.reported() {
        return Some(Err(AlreadyReported));
    }
    hook.status = Some(status.to_string());
    hook.update_time_ms = Some(now_ms);
    Some(Ok(hook.lifecycle_event_name.clone()))
}

/// That a hook execution had already reported its result; see [`record_report`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyReported;

/// The sentence `errorInformation.message` carries for a deployment a validation
/// function failed, naming the lifecycle event it was validating and the hook
/// execution that reported it.
#[must_use]
pub fn reported_failure_message(lifecycle_event_name: &str, execution_id: &str) -> String {
    format!(
        "The {lifecycle_event_name} lifecycle event hook execution {execution_id} reported \
         {LIFECYCLE_EVENT_STATUS_FAILED}"
    )
}

/// The sentence a deployment whose validation function reported NOTHING carries:
/// the function ran, or could not be run, and the lifecycle event it was
/// validating has no result — which is not a result to deploy on.
#[must_use]
pub fn no_result_message(lifecycle_event_name: &str, function_name: &str) -> String {
    format!(
        "The {lifecycle_event_name} lifecycle event failed: the validation function \
         {function_name} reported no result for it"
    )
}

/// Fail `deployment_id` for a lifecycle event hook, with the error information
/// CodeDeploy reports for one, and roll it back when its configuration asks for
/// that on a deployment failure.
///
/// The ONE place a hook failure ends a deployment, so a result a function reported
/// and a result it never reported end it the same way. A deployment that already
/// reached an outcome keeps it: the hook was validating a deployment that is over.
pub fn fail_for_hook(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    message: String,
    now_ms: i64,
) -> bool {
    let Some(deployment) = state.deployments.get(deployment_id) else {
        return false;
    };
    if deployment.complete_time_ms.is_some() || deployment_is_terminal(&deployment.status) {
        return false;
    }
    let settled = state.settle_deployment(
        deployment_id,
        super::DEPLOYMENT_STATUS_FAILED,
        Some(crate::ops::types::ErrorInformation {
            code: Some(super::ERROR_CODE_HOOK_EXECUTION_FAILURE.to_string()),
            message: Some(message),
        }),
        now_ms,
    );
    if settled {
        super::rollback::roll_back_on_failure(state, deployment_id, now_ms);
    }
    settled
}
