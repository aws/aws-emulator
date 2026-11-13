// SPDX-License-Identifier: CC0-1.0

//! What a started experiment DOES: the actions its plan keeps, carried out against the
//! resources its targets resolved to, and the progress each one reports.
//!
//! An experiment is not a record that merely ages into `completed` — FIS "runs the actions
//! of an experiment" on real resources, which is the whole point of starting one: the
//! client's resiliency test observes the FAULT, not the status. So this module is the half
//! of the run that reaches outside FIS, and it is the one place that decides, for one
//! action, what carrying it out means.
//!
//! # What is carried out, and what is reported
//!
//! Every action of the run's plan ([`super::RunActions::carried_out`]) gets a RUN here, so
//! every one of them reports its own progress — `ExperimentAction$state`, `startTime` and
//! `endTime`, which is what a client reads `experiment.actions.<name>.state.status` for.
//! What that progress says is where the RUN is in the action's own span, which this
//! emulator knows because those spans are what the run's own window is computed from; it is
//! not a claim about the action's effect on another service.
//!
//! The EFFECT is the separate question, and only two classes of action have one this
//! emulator can carry out:
//!
//! * the action FIS performs by itself — `aws:fis:wait` "waits for the specified duration",
//!   and the duration is the emulator's own run clock, so the wait genuinely elapses
//!   ([`ActionEffect`] is `None` for it);
//! * an action whose whole documented effect is an API call this emulator can perform on
//!   the service that OWNS the targeted resource: `aws:ec2:reboot-instances`, "reboot the
//!   specified EC2 instances", which is carried out on the EC2 store of this same engine
//!   ([`ActionEffect::RebootInstances`]).
//!
//! Every other published action acts on a resource or a control this emulator does not
//! hold, so NOTHING is carried out for one ([`ActionEffect`] is `None` and no effect is ever
//! drained for it) and nothing here announces one: no injected fault, no instance stopped.
//! Its progress is still reported, because the experiment's own status already reports that
//! the run reached it — a run is `completed` once its actions' spans have elapsed — and AWS
//! reports `state` for every action of an experiment, so withholding it would hide a member
//! every client reads rather than withhold a claim.
//!
//! # The clock, not the call
//!
//! Every instant here is owed to the clock: an action starts when the run reaches it and
//! ends when its span has elapsed, whether or not anybody polls. The offsets come from the
//! SAME dependency walk the run's own span does ([`super::run_plan`]), compressed into that
//! span, so the last action of a run finishes exactly when the run does and an action that
//! waits for another starts after it.
//!
//! The effect itself cannot be performed where it falls due — the settle that makes it due
//! runs with this service's store guard held, and performing it needs the owning service's.
//! So a due effect is left on the record and drained by the caller
//! ([`materialise_action_effects`]), the same arrangement the run's log records are
//! delivered under ([`super::experiment_logs`]). It is marked carried out as it is drained,
//! so no poller can perform one effect twice.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use super::{action_span, finish_offset, ExperimentRecord, ExperimentStatus, TARGET_INFORMATION_ARN_KEY};
use crate::ops::types::{
    ExperimentAction, ExperimentActionState, ExperimentTarget, ExperimentTemplateAction,
    ResolvedTarget,
};

/// The published action id whose effect is a reboot of the EC2 instances its target
/// resolved to: "Reboot the specified EC2 instances", as this service's own action catalog
/// states it ([`super::action_catalog_entry`]).
pub const ACTION_ID_EC2_REBOOT_INSTANCES: &str = "aws:ec2:reboot-instances";

/// The published action id FIS carries out entirely by itself: "Wait for the specified
/// duration", which this emulator's run clock is what elapses.
pub const ACTION_ID_FIS_WAIT: &str = "aws:fis:wait";

/// The target resource type whose resources the EC2 instance actions act on, as
/// `ListTargetResourceTypes` publishes it: "An Amazon EC2 instance."
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "aws:ec2:instance";

/// The target resource type whose resources the API-fault actions act on, as that same
/// listing publishes it: "An IAM role." — the principal whose calls a fault affects
/// ([`super::api_fault_injection`]).
pub const RESOURCE_TYPE_IAM_ROLE: &str = "aws:iam:role";

/// The ARN vendor an EC2 resource ARN names, and the resource type an instance's ARN
/// carries (`arn:aws:ec2:<region>:<account>:instance/<id>`) — read through the shared ARN
/// reader rather than by cutting the string up here.
const EC2_ARN_VENDOR: &str = "ec2";

/// See [`EC2_ARN_VENDOR`].
const EC2_INSTANCE_ARN_RESOURCE: &str = "instance";

/// The most resources ONE action's effect is carried out on.
///
/// In the spirit of [`super::action_permissions::MAX_ACTION_RESOURCES`]: a run's resolution
/// is already bounded by [`super::MAX_RESOLVED_TARGETS`], and this bounds the work one
/// action can hand the owning service on top of that, so no template can make a poll pay
/// more than a fixed amount of it.
pub const MAX_ACTION_EFFECT_RESOURCES: usize = 32;

/// The `ExperimentActionState$reason` of an action a stop of its experiment ended while it
/// was under way: FIS's own sentence for that cause, fixed text with no value of the
/// experiment, the action or its resources in it.
pub const ACTION_WAS_STOPPED: &str = "Action was stopped.";

/// How far one action of a run has got.
///
/// The values are `ExperimentActionStatus`'s (modelled
/// `pending | initiating | running | completed | cancelled | stopping | stopped | failed |
/// skipped`). Five of the nine are reachable here, and each one states something this
/// emulator KNOWS: an action is `pending` until the run reaches it, `running` while it is
/// being carried out, `completed` once it has been, and — for a run a stop ended early —
/// `stopped` for the action that was under way and `cancelled` for one the stop reached
/// before it ever started. `skipped` is the state of an action the run does not carry out
/// at all, which is decided before the run begins and reported from
/// [`super::ActionSkip::to_state`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRunStatus {
    /// The run has not reached this action yet.
    Pending,
    /// The action is being carried out.
    Running,
    /// The action has been carried out.
    Completed,
    /// The action was under way when the experiment was stopped.
    Stopped,
    /// The experiment was stopped before the action started.
    Cancelled,
}

impl ActionRunStatus {
    /// The wire value `ExperimentActionStatus` names this status with.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Cancelled => "cancelled",
        }
    }
}

/// What carrying one action out DOES outside FIS.
///
/// An enum rather than one call site, so the next action whose owning service this emulator
/// holds is added beside this one and every one of them is drained, bounded and marked the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionEffect {
    /// Reboot the EC2 instances these ids name — the effect of
    /// [`ACTION_ID_EC2_REBOOT_INSTANCES`], on the instances the action's target resolved to.
    RebootInstances(Vec<String>),
}

/// One action of a run, as the run carries it out.
///
/// Held beside the experiment's copy of the template's action (`ExperimentRecord::actions`,
/// which is what the template said) rather than inside it: this is what HAPPENED, and
/// [`ExperimentRecord::reported_actions`] is where the two are put together for a reader.
#[derive(Debug, Clone)]
pub struct ActionRun {
    /// When the run reaches this action, and so when its effect is carried out.
    pub start_due: SystemTime,
    /// When the action has finished, which is `start_due` plus the span its own `duration`
    /// names (nothing, for an action whose effect is a single call).
    pub end_due: SystemTime,
    pub status: ActionRunStatus,
    /// `ExperimentAction$startTime` — "the time that the action started", set when it did.
    pub start_time: Option<SystemTime>,
    /// `ExperimentAction$endTime` — "the time that the action ended", set when it did.
    pub end_time: Option<SystemTime>,
    /// What carrying this action out does outside FIS, or `None` for one FIS performs by
    /// itself. Kept after it has been carried out, so the record says what the run did.
    pub effect: Option<ActionEffect>,
    /// Whether [`Self::effect`] has been handed to the service that owns the resources.
    pub effect_carried_out: bool,
}

impl ActionRun {
    /// The `ExperimentActionState` this action reports.
    ///
    /// The reason of an action a STOP ended is FIS's own sentence for that cause
    /// ([`ACTION_WAS_STOPPED`]) — fixed text, with nothing of the experiment, the action or
    /// the resources substituted into it, exactly as the experiment's own halt reason is
    /// ([`super::STOPPED_BY_STOP_CONDITION`]). FIS documents no wording for the other
    /// statuses of `ExperimentActionState$reason` ("the reason for the state") and none is
    /// recorded, so those stay explanatory strings of this emulator's own — the same choice
    /// [`super::ActionSkip::to_state`] makes — while the status is the modelled value.
    pub fn to_state(&self) -> ExperimentActionState {
        ExperimentActionState {
            status: Some(self.status.as_wire().to_string()),
            reason: Some(super::clipped_reason(
                match self.status {
                    ActionRunStatus::Pending => "The action has not started yet",
                    ActionRunStatus::Running => "The action is running",
                    ActionRunStatus::Completed => "The action was completed",
                    ActionRunStatus::Stopped => ACTION_WAS_STOPPED,
                    ActionRunStatus::Cancelled => {
                        "The action was cancelled because the experiment was stopped before it \
                         started"
                    }
                }
                .to_string(),
            )),
        }
    }
}

/// The runs the experiment's plan gives the actions it CARRIES OUT, keyed by action name.
///
/// One run per action of the plan: every one of them reports its own progress, and the
/// `effect` an action has only where this emulator can carry one out (see the module docs).
///
/// `run_started_at` is when the experiment becomes `running` and `run_span` is how long it
/// is running for, both as the start computed them, so the actions are laid out across
/// exactly the window the experiment reports: the modelled offsets of the dependency walk
/// ([`super::run_plan`] uses the same one) are compressed into that span, which keeps their
/// ORDER (an action that waits for another starts no earlier than it finishes) and makes the
/// last action end when the run does.
///
/// Empty for a run that ends `failed` without getting going: the caller passes no actions
/// for one, because nothing is carried out and so nothing is claimed.
///
/// Empty for a run that ends `failed` without getting going: the caller passes no actions
/// for one, because nothing is carried out and so nothing is claimed.
pub fn plan_action_runs(
    carried_out: &HashMap<String, ExperimentTemplateAction>,
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
    run_started_at: SystemTime,
    run_span: Duration,
) -> BTreeMap<String, ActionRun> {
    // The dependency walk, once for the whole plan: each action's own span parsed up front
    // and each finish offset memoised, exactly as the run's span is computed.
    let spans: HashMap<&str, Duration> = carried_out
        .iter()
        .map(|(name, action)| (name.as_str(), action_span(action)))
        .collect();
    let mut finish: HashMap<&str, Duration> = HashMap::with_capacity(carried_out.len());
    let mut offsets: BTreeMap<&str, (Duration, Duration)> = BTreeMap::new();
    let mut modelled_span = Duration::ZERO;
    for name in carried_out.keys() {
        let ends = finish_offset(name.as_str(), carried_out, &spans, &mut finish);
        let starts = ends.saturating_sub(spans.get(name.as_str()).copied().unwrap_or_default());
        modelled_span = modelled_span.max(ends);
        offsets.insert(name.as_str(), (starts, ends));
    }
    offsets
        .into_iter()
        .filter_map(|(name, (starts, ends))| {
            let action = carried_out.get(name)?;
            let effect = action_effect(action, targets, resolved);
            let start_due = run_started_at
                .checked_add(compress(starts, modelled_span, run_span))
                .unwrap_or(run_started_at);
            let end_due = run_started_at
                .checked_add(compress(ends, modelled_span, run_span))
                .unwrap_or(start_due);
            Some((
                name.to_string(),
                ActionRun {
                    start_due,
                    // Never before it started: the compression is monotonic, and this is the
                    // floor that holds even where the arithmetic above saturated.
                    end_due: end_due.max(start_due),
                    status: ActionRunStatus::Pending,
                    start_time: None,
                    end_time: None,
                    effect,
                    effect_carried_out: false,
                },
            ))
        })
        .collect()
}

/// `offset` within a modelled run of `modelled_span`, placed in a real run of `run_span`.
///
/// Proportional, so the ORDER and the simultaneity of the modelled offsets survive the
/// compression; `Duration::ZERO` for a plan whose actions name no duration at all, where
/// every action starts with the run (and ends with it, since its own offset is zero too).
fn compress(offset: Duration, modelled_span: Duration, run_span: Duration) -> Duration {
    if modelled_span.is_zero() {
        return Duration::ZERO;
    }
    run_span.mul_f64(offset.as_secs_f64() / modelled_span.as_secs_f64())
}

/// What carrying `action` out does outside FIS, or `None` when this emulator carries nothing
/// out for it.
///
/// The resources are the ones the action's own targets RESOLVED to ("the targets for the
/// action", by the name the template gave them), read off the experiment's stored resolution
/// — the same entries `ListExperimentResolvedTargets` reports — so the action acts on
/// exactly what the client was told it would act on, and on nothing a target did not
/// identify.
pub fn action_effect(
    action: &ExperimentTemplateAction,
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
) -> Option<ActionEffect> {
    match action.action_id.as_deref()? {
        ACTION_ID_EC2_REBOOT_INSTANCES => Some(ActionEffect::RebootInstances(
            action_instance_ids(action, targets, resolved),
        )),
        _ => None,
    }
}

/// The ids of the EC2 instances `action`'s targets resolved to, at most
/// [`MAX_ACTION_EFFECT_RESOURCES`] of them, in the order the resolution reports them.
///
/// WHICH resolved entries the action is about is [`action_target_arns`]'s rule, shared with
/// every other action that acts on what its targets resolved to; what this adds is the one
/// reading of an ARN as an INSTANCE id: a value that is not an EC2 instance ARN names no
/// instance at all, and the ARN is read by [`::service_common::arn`], the shared reader,
/// rather than by cutting the string up here.
fn action_instance_ids(
    action: &ExperimentTemplateAction,
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
) -> Vec<String> {
    let arns = action_target_arns(
        action.targets.as_ref(),
        targets,
        resolved,
        RESOURCE_TYPE_EC2_INSTANCE,
    );
    arns.iter()
        .filter(|arn| ::service_common::arn::is_resource_arn_of(arn, EC2_ARN_VENDOR))
        .filter_map(|arn| {
            ::service_common::arn::resource_id_of_type(arn, EC2_INSTANCE_ARN_RESOURCE)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// The ARNs of the `resource_type` resources an action's own targets resolved to, at most
/// [`MAX_ACTION_EFFECT_RESOURCES`] of them, in the order the resolution reports them.
///
/// `action_targets` is the action's `targets` map — the target NAME it refers to under each
/// of its own keys — which is the only part of an action this reading needs, so it serves an
/// action held as a template's and one held as an experiment's alike.
///
/// The ONE reading of "which resources is this action about" in this service, so the
/// instances a reboot acts on ([`action_instance_ids`]) and the roles an API fault affects
/// ([`super::api_fault_injection`]) are selected by the same rule: an entry contributes only
/// when the action named its target, the entry is of the resource type the action acts on,
/// and the entry carries the ARN its `targetInformation` reports — so an action acts on
/// exactly what `ListExperimentResolvedTargets` told the client it would act on.
pub fn action_target_arns(
    action_targets: Option<&HashMap<String, String>>,
    targets: &HashMap<String, ExperimentTarget>,
    resolved: &[ResolvedTarget],
    resource_type: &str,
) -> Vec<String> {
    let named: BTreeMap<&str, &str> = action_targets
        .into_iter()
        .flatten()
        .map(|(key, name)| (key.as_str(), name.as_str()))
        .collect();
    let mut arns: Vec<String> = Vec::new();
    for entry in resolved {
        if arns.len() >= MAX_ACTION_EFFECT_RESOURCES {
            break;
        }
        let Some(target_name) = entry.target_name.as_deref() else {
            continue;
        };
        if !named.values().any(|name| *name == target_name) {
            continue;
        }
        // The resource type the entry carries is the TARGET's, so a target of another type
        // under this action's key contributes nothing.
        let entry_type = entry
            .resource_type
            .as_deref()
            .or_else(|| targets.get(target_name)?.resource_type.as_deref());
        if entry_type != Some(resource_type) {
            continue;
        }
        let Some(arn) = entry
            .target_information
            .as_ref()
            .and_then(|information| information.get(TARGET_INFORMATION_ARN_KEY))
            .filter(|arn| !arn.is_empty())
        else {
            continue;
        };
        arns.push(arn.clone());
    }
    arns
}

impl ExperimentRecord {
    /// Advance each action of this run to where the clock has got it at `now`, and reconcile
    /// the actions with a run that has ENDED.
    ///
    /// Called by [`ExperimentRecord::settle`] with the experiment's own status already
    /// settled, so what it reads is where the run stands: a run that completed has carried
    /// every one of its actions out, one a stop ended early carried out only what it had
    /// reached, and a run still going is somewhere between its actions' offsets.
    pub(super) fn settle_actions(&mut self, now: SystemTime) {
        let stopped = matches!(
            self.status,
            ExperimentStatus::Stopping | ExperimentStatus::Stopped | ExperimentStatus::Failed
        );
        let completed = self.status == ExperimentStatus::Completed;
        for run in self.action_runs.values_mut() {
            if stopped {
                // The stop reached the action before the clock did: an action under way was
                // stopped with the experiment, and one that had not started never will.
                // Either way nothing further is carried out for it, so its effect is
                // dropped rather than left for a later drain to perform.
                match run.status {
                    ActionRunStatus::Pending => {
                        run.status = ActionRunStatus::Cancelled;
                        run.effect_carried_out = true;
                    }
                    ActionRunStatus::Running => run.status = ActionRunStatus::Stopped,
                    _ => {}
                }
                continue;
            }
            if run.status == ActionRunStatus::Pending && (completed || now >= run.start_due) {
                run.status = ActionRunStatus::Running;
                run.start_time = Some(run.start_due);
            }
            if run.status == ActionRunStatus::Running && (completed || now >= run.end_due) {
                run.status = ActionRunStatus::Completed;
                run.end_time = Some(run.end_due);
            }
        }
    }

    /// The `actions` map this experiment reports: the template's own actions, each carrying
    /// the state of the run it had.
    ///
    /// The two halves meet here and nowhere else — the stored action is what the template
    /// said ([`super::experiment_action`], which also holds the `skipped` state an action the
    /// run does not carry out was given), and the run is what happened — so a reader cannot
    /// be told a progress the record does not hold.
    pub fn reported_actions(&self) -> HashMap<String, ExperimentAction> {
        let mut actions = self.actions.clone();
        for (name, run) in &self.action_runs {
            let Some(action) = actions.get_mut(name) else {
                continue;
            };
            action.state = Some(run.to_state());
            action.start_time = run.start_time.map(::aws_smithy_types::DateTime::from);
            action.end_time = run.end_time.map(::aws_smithy_types::DateTime::from);
        }
        actions
    }
}

impl super::ComAmazonawsFisFaultinjectionsimulatorState {
    /// Take every action effect the store holds and has not carried out yet.
    ///
    /// Call it right after settling, with the effects that settle made due still on the
    /// records, and then carry out what it hands back with the state guard released
    /// ([`carry_out_action_effects`]). An effect leaves here marked carried out, so two
    /// callers cannot perform one twice — the same contract
    /// [`take_pending_log_deliveries`](Self::take_pending_log_deliveries) holds its records
    /// to.
    ///
    /// The walk is over the whole experiment map, which is bounded by
    /// [`MAX_EXPERIMENTS`](super::MAX_EXPERIMENTS), and over each experiment's actions, which
    /// are bounded by [`MAX_ACTIONS_PER_TEMPLATE`](super::MAX_ACTIONS_PER_TEMPLATE): every
    /// operation that settles already walks or indexes that same map.
    pub fn take_pending_action_effects(&mut self) -> Vec<ActionEffect> {
        let mut effects = Vec::new();
        for record in self.experiments.values_mut() {
            for run in record.action_runs.values_mut() {
                if run.effect_carried_out || run.status == ActionRunStatus::Pending {
                    continue;
                }
                run.effect_carried_out = true;
                if let Some(effect) = run.effect.clone() {
                    effects.push(effect);
                }
            }
        }
        effects
    }
}

/// Carry out `effects` on the services that own the resources they name, as of `now`.
///
/// The EC2 store is a different store in the same engine, reached through EC2's OWN
/// accessor — so this writes the very store that service's handlers read — and no EC2
/// handler reads this one, so holding its guard here cannot cycle. The guard is taken once
/// for the whole batch and released when this returns.
///
/// An instance the EC2 store does not hold, or one with no running operating system to
/// restart, is not rebooted and nothing is invented for it: FIS does not own EC2's resources
/// and a target ARN is not a claim that the resource behind it exists.
pub fn carry_out_action_effects<Eng: Send + Sync + 'static>(
    engine: &Eng,
    effects: &[ActionEffect],
    now: SystemTime,
) {
    if effects.is_empty() {
        return;
    }
    let Some(store) = ec2::service_state(engine) else {
        // This engine carries no EC2 store, so there is nothing for the effect to act on.
        ::tracing::warn!(
            effects = effects.len(),
            "no EC2 store is mounted on this engine, so the experiment's EC2 actions reach \
             nothing"
        );
        return;
    };
    let mut store = store.lock();
    for effect in effects {
        match effect {
            ActionEffect::RebootInstances(instance_ids) => {
                for instance_id in instance_ids {
                    if !store.reboot_instance_guest(instance_id, now) {
                        // Nothing was rebooted, and nothing is invented for it: the target
                        // named a resource this emulator holds no running guest for.
                        ::tracing::debug!(
                            %instance_id,
                            "the experiment's reboot action names an instance this emulator \
                             holds no running guest for, so nothing is rebooted for it"
                        );
                    }
                }
            }
        }
    }
}

/// Carry out every action effect the clock has made due at `now`, for every experiment this
/// store holds, and publish what its running actions INJECT into another service's API.
///
/// The one call a reporting operation makes for this: it settles the experiments (the settle
/// is what reaches an action and makes its effect due), takes the effects off the records
/// and RELEASES this service's guard before performing them, since performing one takes the
/// owning service's guard.
///
/// A write a REPORTING operation performs, and only one the clock already owed: an action
/// the run reached was carried out at that instant whether or not anybody polled, which is
/// exactly why a client that starts an experiment and then waits for it finds the fault
/// injected.
///
/// The API faults go the same way, as the whole live set of each experiment's leases
/// ([`super::api_fault_injection`]): an action that is running has its fault published where
/// the affected service reads it, and one the run has left behind has it withdrawn.
pub fn materialise_action_effects<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    // The alarm states the experiments' stop conditions are judged against, read out of the
    // CloudWatch store before this store's guard is taken, exactly as every other operation
    // that settles reads them ([`super::stop_conditions`]).
    let breaches = super::stop_conditions::AlarmBreaches::read(engine);
    let (effects, injections) = {
        let mut store = state.lock();
        store.settle_experiments(now, &breaches);
        (
            store.take_pending_action_effects(),
            store.live_api_fault_leases(),
        )
    };
    carry_out_action_effects(engine, &effects, now);
    for (experiment_id, leases) in injections {
        super::api_fault_injection::publish_api_faults(engine, &experiment_id, leases, now);
    }
}
