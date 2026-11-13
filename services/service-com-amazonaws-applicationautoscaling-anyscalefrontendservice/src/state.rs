// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationautoscaling-anyscalefrontendservice service.
//!
//! A scalable target's identity in Application Auto Scaling is the triple
//! (`ServiceNamespace`, `ResourceId`, `ScalableDimension`) — the model documents that
//! "scalable targets are uniquely identified by the combination of resource ID, scalable
//! dimension, and namespace" — inside ONE Region, which is the endpoint the request was
//! sent to: this is a regional service, so [`TargetKey`] carries the Region beside that
//! triple and every map below is keyed by it. A scaling policy adds its `PolicyName`
//! ([`PolicyKey`]), and a scheduled action its own name ([`ScheduledActionKey`]), so both
//! are regional with the target they belong to.
//!
//! Both maps are `BTreeMap`s, so `Describe*` enumerates in a stable order and an opaque
//! `NextToken` only has to carry the last key a page emitted.
//!
//! Besides the state itself this module holds the helpers more than one operation needs:
//! ARN minting, the page/token codec and the shared validation messages. They live here
//! rather than in one operation's file so the six handlers of the target/policy lifecycle
//! cannot drift from each other.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Alarm, NotScaledReason, PredictiveScalingPolicyConfiguration, ScalableTarget, ScalingActivity,
    ScalingPolicy, ScheduledAction, StepScalingPolicyConfiguration, SuspendedState,
    TargetTrackingScalingPolicyConfiguration,
};

/// Executing a scaling policy whose CloudWatch alarm is in `ALARM`: the capacity it computes,
/// the resource it moves and the activity it records.
pub mod policy_execution;

/// Confirming a capacity movement's FULFILMENT in the service that owns the scaled
/// resource, which is what ends the activity that recorded the movement.
pub mod capacity_fulfilment;

/// The Amazon Keyspaces table a `cassandra` scalable target names, resolved in the service
/// that owns it.
pub mod keyspaces_tables;

/// The endpoint a `custom-resource` scalable target is, which is where that target's
/// capacity is read and moved.
pub mod custom_resource_endpoint;

/// The Lambda function version or alias a `lambda` scalable target names, resolved in the
/// service that owns it.
pub mod lambda_functions;

/// The resources of a scalable target this service reaches OVER THE WIRE, resolved as a
/// registration's precondition.
pub mod remote_resources;

/// The ONE in-process call this service makes to another emulated service.
pub mod in_process_calls;

/// The refusals whose wire identity this service's own error list does not express, and the
/// error channel that carries them.
pub mod wire_refusals;

/// The natural AWS identity of a scalable target, and the prefix of a scaling policy's
/// and a scheduled action's identity.
///
/// The identity opens with the REGION the target was registered in, because a scalable
/// target is a regional resource: the resource it scales lives in one Region, the ARN this
/// service mints for it names that Region, and the same `ResourceId` in another Region is
/// another resource with a scalable target of its own. Region first also makes the order
/// `BTreeMap` keeps a per-Region order, so a `Describe` of one Region walks a contiguous
/// run of keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetKey {
    pub region: String,
    pub service_namespace: String,
    pub resource_id: String,
    pub scalable_dimension: String,
}

/// A scaling policy's identity: its target plus its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyKey {
    pub target: TargetKey,
    pub policy_name: String,
}

/// A scheduled action's identity: its target plus its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduledActionKey {
    pub target: TargetKey,
    pub scheduled_action_name: String,
}

/// What `RegisterScalableTarget` persists, and what `DescribeScalableTargets`
/// reconstructs a `ScalableTarget` from.
///
/// `MinCapacity`/`MaxCapacity`/`RoleARN` are stored unconditionally rather than as
/// options because the modelled `ScalableTarget` requires all three: the first
/// registration must supply the two capacities (the model's own documentation: "This
/// property is required when registering a new scalable target") and `RoleARN` falls back
/// to the service-linked role the service uses when the caller names none.
#[derive(Debug, Clone)]
pub struct ScalableTargetRecord {
    pub arn: String,
    pub min_capacity: i32,
    pub max_capacity: i32,
    /// The capacity this service believes the scalable target currently sits at.
    ///
    /// Application Auto Scaling learns a resource's capacity from the service that owns it
    /// (ECS, DynamoDB, …) and moves it on the target's behalf, so for a namespace this
    /// emulator holds a store for this follows that resource's own capacity: every
    /// registration places it inside the registered window, and every movement is applied
    /// there ([`policy_execution`]). For a namespace whose owning service this emulator
    /// cannot read, this IS the capacity: a first registration adopts the window's floor —
    /// the registered minimum is the smallest capacity "inside of this range", and adopting
    /// it records no activity because nothing was moved — and every later movement is an
    /// action this service performed, so it is recorded as a scaling activity.
    pub current_capacity: i32,
    pub role_arn: String,
    pub creation_time: SystemTime,
    pub suspended_state: Option<SuspendedState>,
    pub tags: BTreeMap<String, String>,
    /// The scaling decision this target's policies last settled between them, while it is
    /// still the decision a policy triggered with them is compared against. See
    /// [`SettledArbitration`].
    pub settled_arbitration: Option<SettledArbitration>,
}

/// The ONE scaling decision a scalable target's policies settled for one triggering event:
/// the capacity the target held when it was taken, the capacity it was moved to, and the
/// policy that moved it.
///
/// "Multiple scaling policies can be in force at the same time for the same scalable target
/// […] Application Auto Scaling gives precedence to the policy that provides the largest
/// capacity for both scale out and scale in": a target moves ONCE for the policies triggered
/// together, to the largest capacity any of them asked for. Which means the capacity each of
/// them is computed from is the one the target held AS OF that decision — a policy whose
/// trigger is read after the decision was already taken is still asking for a movement of the
/// capacity that was in force when the event began, not of the capacity the winner has since
/// moved it to, so the two adjustments can never accumulate.
///
/// The record stands for as long as the movement that took it is still being waited on
/// ([`SettledArbitration::open_until`]), and any capacity movement of the target that is not a
/// policy's own ends it ([`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::record_capacity_change`]):
/// the capacity as of the decision is only meaningful while that is the capacity the target
/// was moved from.
#[derive(Debug, Clone)]
pub struct SettledArbitration {
    /// The capacity the target held when this decision was taken, which every policy
    /// triggered by the same event computes its movement from.
    pub base_capacity: i32,
    /// The capacity the decision moved the target to, which another policy of this target
    /// has to provide MORE than to move it again.
    pub settled_capacity: i32,
    /// The `PolicyName` of the policy whose movement settled it; its own later breaches are
    /// governed by its `Cooldown` rather than by this decision.
    pub policy_name: String,
    /// While this instant is still ahead, the decision is the one a policy triggered with it
    /// is compared against: it is "the amount of time, in seconds, to wait for a previous
    /// scaling activity to take effect" (`StepScalingPolicyConfiguration$Cooldown`) of the
    /// policy that took it, so a policy that states no wait at all leaves no decision for a
    /// later trigger to be compared against.
    pub open_until: SystemTime,
}

/// What `PutScheduledAction` persists: the action as `DescribeScheduledActions` reports
/// it, plus the bookkeeping the clock needs.
///
/// `last_applied` is the instant the action last moved the target's capacity bounds. It
/// is what makes a recurring schedule idempotent: the occurrence that fires is the first
/// one AFTER this instant, so materialising the same occurrence twice is impossible.
#[derive(Debug, Clone)]
pub struct ScheduledActionRecord {
    pub action: ScheduledAction,
    pub last_applied: Option<SystemTime>,
}

/// What `PutScalingPolicy` persists, and what `DescribeScalingPolicies` reconstructs a
/// `ScalingPolicy` from.
#[derive(Debug, Clone)]
pub struct ScalingPolicyRecord {
    pub policy_arn: String,
    pub policy_type: String,
    pub step_config: Option<StepScalingPolicyConfiguration>,
    pub target_tracking_config: Option<TargetTrackingScalingPolicyConfiguration>,
    pub predictive_config: Option<PredictiveScalingPolicyConfiguration>,
    pub alarms: Vec<Alarm>,
    pub creation_time: SystemTime,
    /// The alarm transitions this policy has already been EXECUTED for: the instant, in epoch
    /// seconds, each alarm last moved into `ALARM` for an execution this service performed,
    /// by alarm name.
    ///
    /// One transition fires one execution, so this is what makes a breach idempotent: a later
    /// pass performs an alarm's transition only when it is newer than the one recorded here
    /// (see [`policy_execution`]).
    pub executed_transitions: BTreeMap<String, f64>,
    /// When this policy last moved the capacity of the resource it scales, which is the
    /// instant its `Cooldown` is measured from.
    pub last_executed: Option<SystemTime>,
}

/// One entry of the scaling-activity log: a capacity movement this service performed,
/// or declined to perform, on a scalable target.
///
/// The status is NOT stored as a single current value. A movement is non-terminal when
/// it starts and terminal once the clock reaches [`ScalingActivityRecord::fulfilled_at`],
/// so the record keeps the terminal status it is owed and a reader derives the status it
/// reports from the instant it reads at. Nothing has to tick for an activity to complete.
#[derive(Debug, Clone)]
pub struct ScalingActivityRecord {
    /// The activity's unique identifier, as `ScalingActivity$ActivityId`.
    pub activity_id: String,
    /// Where this activity sits in the order the log recorded them, so activities that
    /// share a `StartTime` still have one newest-first order.
    pub sequence: u64,
    /// The scalable target the activity belongs to; its three members are what
    /// `DescribeScalingActivities` filters on.
    pub target: TargetKey,
    /// "A simple description of what action the scaling activity intends to accomplish."
    pub description: String,
    /// "A simple description of what caused the scaling activity to happen."
    pub cause: String,
    /// The capacity this movement SET on the target — the value after the target's registered
    /// window was enforced, which is the capacity the resource is driven to.
    ///
    /// It is what both messages the activity reports name: while the change is on its way to
    /// the owning service ([`awaiting_fulfilment_status_message`]) and once it has taken
    /// effect ([`fulfilled_status_message`]).
    pub capacity: i32,
    pub start_time: SystemTime,
    /// When the activity reaches [`ScalingActivityRecord::terminal_status_code`], or
    /// `None` for one that was already terminal when it was recorded (an activity that
    /// was not scaled never started).
    pub fulfilled_at: Option<SystemTime>,
    /// The status the activity ends in, one of `SCALING_ACTIVITY_STATUS_CODE_VALUES`.
    pub terminal_status_code: String,
    /// The message that goes with that terminal status.
    pub terminal_status_message: Option<String>,
    pub details: Option<String>,
    /// Why the activity was not scaled; empty for one that was. A non-empty list is
    /// what makes this a "not scaled activity", which a `Describe` hides unless the
    /// caller asks for it.
    ///
    /// Written by [`policy_execution`] for a scaling decision a registered bound WITHHELD:
    /// the capacity the policy asked for is past a bound the target already sits at, so
    /// nothing moved and the reason the model's `NotScaledReason$Code` names is what the
    /// log says about it.
    pub not_scaled_reasons: Vec<NotScaledReason>,
    /// Whether this activity is a capacity MOVEMENT of the scalable target — a change of
    /// the capacity the resource is driven to — rather than a record of something else this
    /// service did to the target (a scheduled action's dispatch, a decision a bound
    /// withheld).
    ///
    /// Only a movement can be taken over by a later movement, which is what
    /// [`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::override_unfulfilled_activities`]
    /// reads it for: an activity that moved no capacity has none a later movement could
    /// displace, so it keeps the status it was recorded with.
    pub movement: bool,
    /// Whether the service that OWNS the scaled resource has still to report the capacity
    /// this movement set as IN EFFECT before the activity may reach its terminal status.
    ///
    /// Setting a capacity and that capacity taking effect are two things: this service does
    /// the first and the owning service the second, which is what the activity's own message
    /// says it is waiting for ([`awaiting_fulfilment_status_message`]). So while this is set
    /// the activity is non-terminal however far past [`ACTIVITY_FULFILMENT`] the clock is,
    /// and the pass that reads the owning service clears it
    /// ([`capacity_fulfilment::settle_activity_fulfilments`]). It is set only for a namespace
    /// whose owning service reports an in-effect capacity at all
    /// ([`capacity_fulfilment::owner_confirms_fulfilment`]); for every other namespace the
    /// span is the whole of the wait.
    pub awaiting_owner: bool,
}

/// What one activity that is not a capacity movement says about itself:
/// [`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::record_activity`]'s
/// text, status and reasons.
///
/// The two members that are not free text are the ones the model constrains: `status_code`
/// is one of `SCALING_ACTIVITY_STATUS_CODE_VALUES`, and a non-empty `not_scaled_reasons`
/// makes the activity a not-scaled one, which a `Describe` hides unless the caller asked
/// for them.
#[derive(Debug, Clone)]
pub struct ActivityEntry {
    /// "A simple description of what action the scaling activity intends to accomplish."
    pub description: String,
    /// "A simple description of what caused the scaling activity to happen."
    pub cause: String,
    /// The capacity the activity is about: the bound a dispatch set, or the capacity a
    /// withheld decision found the target at.
    pub capacity: i32,
    /// The status the activity carries from the instant it was recorded.
    pub status_code: String,
    /// "A simple message about the current status of the scaling activity."
    pub status_message: Option<String>,
    /// Why the decision was not scaled; empty for an activity that did what it describes.
    pub not_scaled_reasons: Vec<NotScaledReason>,
}

/// The order in which this store's CloudWatch alarm writes happen.
///
/// The alarms a target's policies own live in ANOTHER service's store, which cannot be
/// written under this service's lock, so every alarm write is a three-step sequence: plan
/// under this lock, write with it released, record under it again. Two of those sequences
/// for one target may therefore interleave, and the one that writes LAST decides what
/// CloudWatch ends up holding — which, without this token, can be a plan computed from a
/// configuration that has since been overwritten, leaving an alarm set that belongs to no
/// stored policy.
///
/// So the whole sequence is taken in turn: a holder plans from the store it reads INSIDE its
/// turn, which makes the last write the one derived from the last configuration the store
/// holds. It is a lock of its own rather than this service's store lock, because the
/// CloudWatch write must still happen with every store lock released.
///
/// Clone-cheap (an `Arc` inside): a handler clones it out of the store
/// ([`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::alarm_sync`]) and then
/// takes its turn with the store lock dropped.
#[derive(Debug, Clone, Default)]
pub struct AlarmSyncToken(Arc<Mutex<()>>);

impl AlarmSyncToken {
    /// Wait for this token's turn, which lasts until the guard is dropped.
    ///
    /// Recovers from a poisoned lock exactly as the engine's own state lock does: the token
    /// guards an ORDER, not a value, so a handler that panicked mid-sequence leaves nothing
    /// behind for the next one to misread, and a long-lived emulator must keep serving.
    fn take_turn(&self) -> MutexGuard<'_, ()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Per-service state for com-amazonaws-applicationautoscaling-anyscalefrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState {
    /// Registered scalable targets, ordered by their natural identity.
    pub targets: BTreeMap<TargetKey, ScalableTargetRecord>,
    /// Scaling policies, ordered by (target, policy name).
    pub policies: BTreeMap<PolicyKey, ScalingPolicyRecord>,
    /// `ScalableTargetARN` -> the target it names, so an ARN-addressed operation
    /// (`TagResource` and friends) can resolve a `ResourceARN` back to a target.
    ///
    /// `DeregisterScalableTarget` takes an ARN out of this index as it removes the target, so
    /// an ARN whose target is no longer registered resolves to nothing here — which is the
    /// one answer an ARN-addressed operation has for every ARN that names no registered
    /// scalable target (`wire_refusals::WireRefusal::ScalableTargetNotFound`).
    pub arn_index: BTreeMap<String, TargetKey>,
    /// Scheduled actions, ordered by (target, action name). `DeregisterScalableTarget`
    /// cascades into this map, so a deregistered target leaves none behind.
    ///
    /// The record holds the modelled `ScheduledAction` as-is — every member of it is
    /// either part of the key or a value `PutScheduledAction` persisted verbatim, so
    /// `DescribeScheduledActions` has nothing to rebuild — plus the one thing the clock
    /// needs: when an occurrence of it was last applied.
    pub scheduled_actions: BTreeMap<ScheduledActionKey, ScheduledActionRecord>,
    /// The scaling-activity log, oldest first — every capacity movement this service
    /// performed, within the window it reports over. `DescribeScalingActivities` reads
    /// it newest first.
    pub activities: Vec<ScalingActivityRecord>,
    /// How many activities have ever been recorded. It only grows, so it both orders the
    /// log and makes each minted `ActivityId` unique.
    pub activities_recorded: u64,
    /// The targets an update is IN FLIGHT for right now, which is what makes a concurrent
    /// update of one target observable: `ConcurrentUpdateException` is documented as what
    /// AWS answers "if you request an update to an Application Auto Scaling resource that
    /// already has a pending update", so an update has to be pending for the span it
    /// takes, and a second request for the same target inside that span is refused
    /// instead of silently overwriting the first one's read-modify-write.
    ///
    /// Entered and left by [`Self::begin_target_update`] / [`Self::end_target_update`];
    /// a span, never a stored status, so nothing has to be cleaned up by a later caller.
    pub updating: BTreeSet<TargetKey>,
    /// Capacity movements this service has made that the service OWNING the scaled resource
    /// has not been told about yet, as the capacity each target was last moved to.
    ///
    /// Application Auto Scaling moves a resource's capacity through the owning service, whose
    /// store cannot be written under this service's lock, so a movement is queued here by the
    /// call that made it and delivered with the lock released
    /// ([`policy_execution::settle_policy_executions`]). One entry per target: the capacity a
    /// target was last moved to is the capacity it holds, so a second movement of the same
    /// target replaces the first rather than queueing behind it.
    pub pending_capacity: BTreeMap<TargetKey, i32>,
    /// Until when each queued movement is still OFFERED to the resource it moved.
    ///
    /// A resource reached over the wire can decline a movement it cannot take YET — it is
    /// being created, or another change of it is still settling — and a movement this service
    /// has already recorded as an activity is owed to that resource, so a declined delivery is
    /// offered again by the next pass rather than dropped
    /// ([`policy_execution::deliver_remote_capacity`]). The deadline is what keeps that
    /// bounded: past it the resource is one that is not taking the movement at all, and the
    /// queue does not grow a permanent entry per such target.
    ///
    /// One entry per queued movement, written and dropped with the queue itself.
    pub pending_capacity_until: BTreeMap<TargetKey, SystemTime>,
    /// When the capacity of each target whose resource is read OVER THE WIRE was last asked
    /// for, so the clock's passes do not re-ask the caller's own endpoint several times a
    /// second (`policy_execution::REMOTE_CAPACITY_READ_INTERVAL`), and so the reads one pass
    /// performs can start with the targets read longest ago.
    ///
    /// One entry per REGISTERED target: `policy_execution::refresh_remote_capacities` drops
    /// the entries of targets the store no longer holds before it reads, so a deregistered
    /// target leaves nothing behind.
    pub remote_capacity_read_at: BTreeMap<TargetKey, SystemTime>,
    /// Whether one in-process scaling clock is claimed for this store
    /// ([`policy_execution::drive_scaling_clock`]), which is what keeps a second from
    /// starting beside it.
    pub scaling_clock_running: bool,
    /// Which claim that is, so a clock that stands down releases only its own.
    pub scaling_clock_generation: u64,
    /// The running clock's task, kept so the claim can tell a clock that is still running
    /// from one whose task has ended without releasing it.
    pub scaling_clock: Option<::tokio::task::JoinHandle<()>>,
    /// The turn-taking token every CloudWatch alarm write of this store goes through, so
    /// the alarms CloudWatch holds for a target are the ones this store's policies want.
    /// Read through [`Self::alarm_sync`]; see [`AlarmSyncToken`].
    alarm_sync: AlarmSyncToken,
}

impl ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState {
    /// Record the scaling activity for a capacity movement this service has just made on
    /// `target`, and answer its `ActivityId`.
    ///
    /// The activity is non-terminal for at least [`ACTIVITY_FULFILMENT`], and reaches
    /// `Successful` once the capacity it set has taken effect: for a namespace whose owning
    /// service reports that, when that service reports it
    /// ([`capacity_fulfilment::settle_activity_fulfilments`]), and otherwise as soon as the
    /// span has elapsed. Unless a LATER movement of the same scalable target takes its
    /// capacity over first, which is what [`Self::override_unfulfilled_activities`] resolves.
    pub fn record_capacity_change(
        &mut self,
        target: &TargetKey,
        capacity: i32,
        cause: impl Into<String>,
        at: SystemTime,
    ) -> String {
        let sequence = self.activities_recorded;
        self.activities_recorded += 1;
        let activity_id = scaling_activity_id(sequence, target);
        // Whatever this movement is, the capacity it sets is the capacity the target holds
        // from here on — so a scaling decision its policies settled against an EARLIER
        // capacity is no longer the decision anything is compared against. The policy
        // execution that settles one records it again itself, after this call.
        if let Some(record) = self.targets.get_mut(target) {
            record.settled_arbitration = None;
        }
        // This movement is what the target is driven to from here on, so a movement of the
        // same target that had not been fulfilled yet is displaced by it, and this capacity
        // is the one that takes effect instead of the capacity it named.
        self.override_unfulfilled_activities(target, capacity, at);
        // The resource the target scales is owed this capacity: queued here, because the
        // owning service's store cannot be written under this service's lock.
        self.pending_capacity.insert(target.clone(), capacity);
        self.pending_capacity_until
            .insert(target.clone(), at + CAPACITY_DELIVERY_WINDOW);
        self.activities.push(ScalingActivityRecord {
            activity_id: activity_id.clone(),
            sequence,
            target: target.clone(),
            description: movement_description(target, capacity),
            cause: cause.into(),
            capacity,
            start_time: at,
            fulfilled_at: Some(at + ACTIVITY_FULFILMENT),
            terminal_status_code: ACTIVITY_SUCCESSFUL.to_string(),
            terminal_status_message: Some(fulfilled_status_message(target, capacity)),
            details: None,
            not_scaled_reasons: Vec::new(),
            movement: true,
            awaiting_owner: capacity_fulfilment::owner_confirms_fulfilment(
                &target.service_namespace,
            ),
        });
        // The emulator's own ceiling on the log: the oldest entries go first, exactly as
        // ageing out of the reported window would have dropped them.
        self.trim_activity_log();
        activity_id
    }

    /// Resolve every activity of `target` that has not reached fulfilment by `at` as
    /// [`ACTIVITY_OVERRIDDEN`], ended at that instant.
    ///
    /// A capacity movement is fulfilled when the capacity it set has taken effect, and only
    /// one capacity is in force per scalable target at a time: a movement recorded while an
    /// earlier one is still on its way to fulfilment REPLACES the capacity that earlier one
    /// named, which is therefore never reached. The model keeps `Overridden` beside
    /// `Successful` for exactly that outcome, so a displaced movement ends there and only a
    /// movement whose capacity took effect ends `Successful` — otherwise the log cannot tell
    /// a change that happened from one that was overtaken.
    ///
    /// A status is terminal once the clock has reached it: a movement already fulfilled at
    /// `at` keeps the status it reported, and an activity recorded in a terminal status
    /// (`fulfilled_at` of `None`) moved no capacity that could be taken over. The walk is
    /// newest-first and stops at the first activity of this target that was fulfilled,
    /// because everything recorded before it is terminal too — passing over an activity that
    /// starts after `at`, which this movement cannot have displaced.
    ///
    /// The displaced activity keeps the `Description` of the change IT intended, and its
    /// `StatusMessage` reports BOTH halves of the override ([`overridden_status_message`]):
    /// the capacity this activity itself applied, and `capacity` — the one the displacing
    /// movement set in its place. So a reader of the log learns what the overtaken movement
    /// had done as well as what became of the scalable target.
    fn override_unfulfilled_activities(
        &mut self,
        target: &TargetKey,
        capacity: i32,
        at: SystemTime,
    ) {
        for activity in self
            .activities
            .iter_mut()
            .rev()
            .filter(|activity| &activity.target == target)
        {
            // An activity that STARTS after this movement is not one this movement displaced:
            // the log is ordered by when each movement was recorded, and a movement can be
            // recorded for an instant in the past — the scheduled-action dispatcher records an
            // occurrence at the instant it was DUE, which is the instant it acted at.
            if activity.start_time > at {
                continue;
            }
            // An activity that moved no capacity has none a later movement could displace:
            // a scheduled action's dispatch, or a decision a registered bound withheld,
            // keeps the status it was recorded with.
            if !activity.movement {
                continue;
            }
            // A movement still waiting on the owning service is not terminal however far
            // past its own span the clock is, so it is one a later movement displaces.
            if !activity.awaiting_owner && activity.fulfilled_at.is_none_or(|due| due <= at) {
                break;
            }
            activity.awaiting_owner = false;
            activity.fulfilled_at = Some(at);
            activity.terminal_status_code = ACTIVITY_OVERRIDDEN.to_string();
            activity.terminal_status_message = Some(overridden_status_message(
                target,
                activity.capacity,
                capacity,
            ));
        }
    }

    /// Record one activity of `target` that is NOT a capacity movement of it, and answer
    /// its `ActivityId`.
    ///
    /// Two things this service does to a scalable target are recorded here rather than by
    /// [`Self::record_capacity_change`]: a scheduled action's DISPATCH, which sets the
    /// target's capacity bounds and is what a client joins an action to the history it
    /// produced by, and a scaling decision a registered bound WITHHELD, which is the "not
    /// scaled activity" the model's `NotScaledReason` describes. Neither moves the capacity
    /// of the resource the target scales, so neither queues a delivery to the owning
    /// service, neither ends a scaling decision the target's policies settled, and neither
    /// takes an earlier movement over.
    ///
    /// The status is the one the entry states, and it is terminal from the instant the
    /// activity was recorded: what it reports was decided by this service alone, so there
    /// is no owning service for it to wait on.
    pub fn record_activity(
        &mut self,
        target: &TargetKey,
        at: SystemTime,
        entry: ActivityEntry,
    ) -> String {
        let sequence = self.activities_recorded;
        self.activities_recorded += 1;
        let activity_id = scaling_activity_id(sequence, target);
        self.activities.push(ScalingActivityRecord {
            activity_id: activity_id.clone(),
            sequence,
            target: target.clone(),
            description: entry.description,
            cause: entry.cause,
            capacity: entry.capacity,
            start_time: at,
            // "Not scaled activities are activities that aren't completed or started": one
            // that was not scaled never started, so it has no `EndTime`, while one that did
            // what it describes ended at the instant it was recorded.
            fulfilled_at: entry.not_scaled_reasons.is_empty().then_some(at),
            terminal_status_code: entry.status_code,
            terminal_status_message: entry.status_message,
            details: None,
            not_scaled_reasons: entry.not_scaled_reasons,
            movement: false,
            // What this entry reports was decided by this service alone, so there is no
            // owning service for it to wait on.
            awaiting_owner: false,
        });
        self.trim_activity_log();
        activity_id
    }

    /// The reason the newest activity of `target` was NOT scaled for, or `None` when that
    /// activity was scaled.
    ///
    /// What a repeated decision is compared against: a breach that keeps being withheld for
    /// the reason the log already states adds no further entry, while one withheld for
    /// another reason — and one taken after a movement of the same target — is a new thing
    /// to say about it.
    pub fn latest_not_scaled_reason(&self, target: &TargetKey) -> Option<&NotScaledReason> {
        self.activities
            .iter()
            .rev()
            .find(|activity| &activity.target == target)?
            .not_scaled_reasons
            .first()
    }

    /// Drop the oldest entries past the emulator's own ceiling on the log, exactly as
    /// ageing out of the reported window would have dropped them.
    fn trim_activity_log(&mut self) {
        let overflow = self.activities.len().saturating_sub(MAX_SCALING_ACTIVITIES);
        if overflow > 0 {
            self.activities.drain(..overflow);
        }
    }

    /// Mark an update of `target` as in flight, answering `false` when another update of
    /// the SAME target already is — the caller then refuses its request with the modelled
    /// `ConcurrentUpdateException`. Updates of DIFFERENT targets never collide.
    pub fn begin_target_update(&mut self, target: &TargetKey) -> bool {
        self.updating.insert(target.clone())
    }

    /// Release the in-flight marker [`Self::begin_target_update`] took.
    pub fn end_target_update(&mut self, target: &TargetKey) {
        self.updating.remove(target);
    }

    /// This store's alarm-sync token. See [`AlarmSyncToken`].
    pub fn alarm_sync(&self) -> AlarmSyncToken {
        self.alarm_sync.clone()
    }

    /// Drop the activities that have aged out of the window this service reports over.
    ///
    /// This is owed by the CLOCK, not by the read that calls it: an activity older than
    /// [`ACTIVITY_RETENTION`] was already outside the window at that same instant,
    /// whether or not anybody asked.
    pub fn expire_old_activities(&mut self, now: SystemTime) {
        // Before the window is wider than the clock there is nothing to expire.
        let Some(cutoff) = now.checked_sub(ACTIVITY_RETENTION) else {
            return;
        };
        self.activities
            .retain(|activity| activity.start_time >= cutoff);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>)
}

// ---------------------------------------------------------------------------
// Store bounds
// ---------------------------------------------------------------------------

/// How many scalable targets this store holds before a NEW registration is refused with
/// the modelled `LimitExceededException`.
///
/// This is the EMULATOR's own bound, not a claim about any AWS account quota: a
/// long-lived emulator serves unauthenticated callers, and `ResourceId` is a free-form
/// string, so without a ceiling N requests leave N resident records for ever. It is set
/// far above any realistic workflow, so nothing a test or a client does legitimately
/// reaches it. Bounding `targets` also bounds `arn_index` (one entry per target) and the
/// per-request vector a `Describe` builds.
pub const MAX_SCALABLE_TARGETS: usize = 10_000;

/// The same bound for scaling policies, whose records additionally carry the caller's
/// whole policy configuration. See [`MAX_SCALABLE_TARGETS`].
pub const MAX_SCALING_POLICIES: usize = 20_000;

/// The same bound for scheduled actions, which hang off a target the same way a policy
/// does and are keyed by a free-form name. See [`MAX_SCALABLE_TARGETS`].
pub const MAX_SCHEDULED_ACTIONS: usize = 20_000;

/// How many metric specifications a predictive scaling policy configuration carries.
///
/// The MODEL's own bound, stated in prose rather than as a `@length` on the list: "This is
/// an array, but we currently only support a single metric specification. That is, you can
/// specify a target value and a single metric pair, or a target value and one scaling
/// metric and one load metric" (`PredictiveScalingPolicyConfiguration$MetricSpecifications`).
/// A configuration carrying none describes no load to forecast at all, so the count is
/// exactly one — which is also what bounds the work a forecast of that policy does.
pub const MAX_PREDICTIVE_METRIC_SPECIFICATIONS: usize = 1;

/// How many scaling activities the log keeps before the oldest are dropped.
///
/// The same emulator-own bound as [`MAX_SCALABLE_TARGETS`], for the same reason: the log
/// grows with every capacity movement and is otherwise only trimmed by
/// [`ACTIVITY_RETENTION`], which a long-lived emulator may never reach.
pub const MAX_SCALING_ACTIVITIES: usize = 10_000;

/// The window `DescribeScalingActivities` reports over: the operation's own
/// documentation, "provides descriptive information about the scaling activities in the
/// specified namespace from the previous six weeks".
pub const ACTIVITY_RETENTION: Duration = Duration::from_secs(6 * 7 * 24 * 60 * 60);

/// The soonest after the call that starts it a scaling activity may reach its terminal
/// status.
///
/// CHOSEN, not documented: on AWS the fulfilment time is the target service's own. It is this
/// project's span for a transition the clock owes, so a caller that reads the activity log
/// straight after the call that moved capacity still observes the non-terminal activity — and
/// one that POLLS the log, which is how a client watches a scaling activity, still catches
/// that status between two reads rather than only ever seeing the terminal one.
///
/// It is the whole of the wait only for a movement nothing else can be read about. Where the
/// service that owns the scaled resource reports whether the capacity has taken effect, the
/// span is the floor and that report is what ends the activity
/// ([`capacity_fulfilment::settle_activity_fulfilments`]), so a capacity the owning service
/// never reaches is one its movement never reports as fulfilled.
pub const ACTIVITY_FULFILMENT: Duration = Duration::from_secs(3);

/// How long a capacity movement keeps being OFFERED to a resource that has not taken it.
///
/// The EMULATOR's own bound. A resource reached over the wire can decline a movement it cannot
/// take yet — one of this project's own transitions is still settling on it — and the movement
/// is owed to it, so a declined delivery is offered again by the next pass of the clock. This
/// bounds that: it is many times [`ACTIVITY_FULFILMENT`], so every transition this emulator
/// gives a resource elapses inside it, and short enough that a resource which is not taking
/// movements at all leaves no permanent entry in the queue.
pub const CAPACITY_DELIVERY_WINDOW: Duration = Duration::from_secs(30);

/// The non-terminal status a capacity movement carries until it is fulfilled, from the
/// model's `ScalingActivityStatusCode` enum (`SCALING_ACTIVITY_STATUS_CODE_VALUES`).
pub const ACTIVITY_IN_PROGRESS: &str = "InProgress";
/// The status a fulfilled capacity movement ends in. See [`ACTIVITY_IN_PROGRESS`].
pub const ACTIVITY_SUCCESSFUL: &str = "Successful";
/// The status a capacity movement ends in when a later movement of the same scalable target
/// took its capacity over before it was fulfilled
/// ([`ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState::override_unfulfilled_activities`]).
/// See [`ACTIVITY_IN_PROGRESS`].
pub const ACTIVITY_OVERRIDDEN: &str = "Overridden";
/// The status an activity ends in when what it describes did not happen: a scheduled
/// action's dispatch the target's registered bounds refused, and a scaling decision a
/// bound withheld. See [`ACTIVITY_IN_PROGRESS`].
pub const ACTIVITY_FAILED: &str = "Failed";

// ---------------------------------------------------------------------------
// Request-derived values
// ---------------------------------------------------------------------------

/// The region every ARN this service mints is scoped to: the one in the caller's SigV4
/// credential scope, or `us-east-1` when the request carries none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// A scalable target's ARN: `arn:aws:application-autoscaling:<region>:<account>:scalable-target/<id>`.
///
/// The Region is the target's own, which its identity carries ([`TargetKey`]), so the ARN
/// cannot name a Region the target is not registered in.
///
/// The opaque id is derived from the target's identity AND from the instant it was
/// registered at, because a scalable target is a RESOURCE and not just a name:
/// deregistering it retires that resource, and registering the same identity again mints
/// a new one, which carries a new ARN and none of the retired target's tags or policies.
/// Deriving the id from the registration instant is what gives the new resource its own
/// identity while keeping the ARN stable for every read of the target that holds it.
pub fn scalable_target_arn(key: &TargetKey, registered_at: SystemTime) -> String {
    let registered_nanos = registered_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = opaque_id(
        &[
            "scalable-target",
            &key.region,
            &key.service_namespace,
            &key.resource_id,
            &key.scalable_dimension,
            &registered_nanos.to_string(),
        ]
        .join("\u{1f}"),
        36,
    );
    ::emulator_server::emulator_arn(
        "application-autoscaling",
        &key.region,
        &format!("scalable-target/{id}"),
    )
}

/// A scaling policy's ARN, in the shape Application Auto Scaling documents:
/// `arn:aws:autoscaling:<region>:<account>:scalingPolicy:<id>:resource/<namespace>/<resource id>:policyName/<name>`.
pub fn scaling_policy_arn(key: &PolicyKey) -> String {
    let id = uuid_shaped_id(
        &[
            "scalingPolicy",
            &key.target.region,
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
        ]
        .join("\u{1f}"),
    );
    ::emulator_server::emulator_arn(
        "autoscaling",
        &key.target.region,
        &format!(
            "scalingPolicy:{id}:resource/{}/{}:policyName/{}",
            key.target.service_namespace, key.target.resource_id, key.policy_name
        ),
    )
}

/// A scheduled action's ARN, in the shape Application Auto Scaling documents:
/// `arn:aws:autoscaling:<region>:<account>:scheduledAction:<id>:resource/<namespace>/<resource id>:scheduledActionName/<name>`.
///
/// Derived from the action's identity for the same reason the policy ARN is: re-putting
/// the same scheduled action must not hand the caller a second ARN.
pub fn scheduled_action_arn(key: &ScheduledActionKey) -> String {
    let id = uuid_shaped_id(
        &[
            "scheduledAction",
            &key.target.region,
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.scheduled_action_name,
        ]
        .join("\u{1f}"),
    );
    ::emulator_server::emulator_arn(
        "autoscaling",
        &key.target.region,
        &format!(
            "scheduledAction:{id}:resource/{}/{}:scheduledActionName/{}",
            key.target.service_namespace, key.target.resource_id, key.scheduled_action_name
        ),
    )
}

/// A scaling activity's `ActivityId`.
///
/// Derived from the log sequence the activity was recorded at and the target it moved,
/// so it is unique (the sequence only grows), stable across reads, and reproducible
/// rather than per-run.
pub fn scaling_activity_id(sequence: u64, key: &TargetKey) -> String {
    uuid_shaped_id(
        &[
            "scalingActivity",
            &sequence.to_string(),
            &key.region,
            &key.service_namespace,
            &key.resource_id,
            &key.scalable_dimension,
        ]
        .join("\u{1f}"),
    )
}

/// The 8-4-4-4-12 identifier that sits inside an `autoscaling` sub-resource ARN,
/// derived from `seed` so it is reproducible rather than per-run.
fn uuid_shaped_id(seed: &str) -> String {
    hyphenated(&opaque_id(seed, ID_HEX_DIGITS))
}

/// How many hexadecimal digits an identifier of this service carries, and the grouping they
/// are written in: `8-4-4-4-12`, which is the 32 of them [`hyphenated`] reads.
const ID_HEX_DIGITS: usize = 32;

/// The identity of the scalable target itself, as a refusal about one of its sub-resources
/// names it: the identifier its own `ScalableTargetARN` ends with, in the grouping every
/// identifier of this service is written in.
///
/// Read off the ARN the target HOLDS rather than derived a second time, so the id a refusal
/// states is the one the caller already has: deregistering an identity and registering it
/// again retires one scalable target and mints another ([`scalable_target_arn`]), and the id
/// follows that resource rather than the triple it was registered under.
pub fn scalable_target_id(scalable_target_arn: &str) -> String {
    hyphenated(scalable_target_arn)
}

/// The last [`ID_HEX_DIGITS`] characters of `id` in the `8-4-4-4-12` grouping, or `id` itself
/// when it carries fewer than that many.
fn hyphenated(id: &str) -> String {
    let characters: Vec<char> = id.chars().collect();
    let Some(digits) = characters
        .len()
        .checked_sub(ID_HEX_DIGITS)
        .map(|from| &characters[from..])
    else {
        return id.to_string();
    };
    let group = |range: ::std::ops::Range<usize>| digits[range].iter().collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        group(0..8),
        group(8..12),
        group(12..16),
        group(16..20),
        group(20..ID_HEX_DIGITS),
    )
}

/// The service-linked role Application Auto Scaling uses for a namespace when the caller
/// names no `RoleARN`: the model documents that "if the service supports service-linked
/// roles, Application Auto Scaling uses a service-linked role, which it creates if it
/// does not yet exist".
///
/// The role is one PER NAMESPACE, not one per account: it is assumed through that
/// namespace's own service principal and named for the resource type it scales, so the
/// name carries that type as a suffix. See [`service_linked_role_name`].
pub fn service_linked_role_arn(service_namespace: &str) -> String {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

    // IAM's own two helpers, so the ARN a scalable target reports is the one the role IAM
    // holds carries ([`ensure_service_linked_role`]) rather than a second spelling of it.
    iam::role_arn(
        &iam::service_linked_role_path(&service_linked_role_principal(service_namespace)),
        &service_linked_role_name(service_namespace),
    )
}

/// The role a registered target is SCALED THROUGH, which is the one it reports: the
/// namespace's service-linked role wherever this service scales that namespace through one,
/// and the role the request named only where it does not.
///
/// `RoleARN` "is required for services that do not support service-linked roles ... If the
/// service supports service-linked roles, Application Auto Scaling uses a service-linked
/// role, which it creates if it does not yet exist" — so for a namespace that HAS one the
/// supplied role has no effect on who scales the resource, and reporting it would name a
/// principal that does not. Which namespaces have one is [`SERVICE_LINKED_ROLE_SUFFIXES`],
/// the published catalog, so this rule is read off that catalog rather than from a second
/// list.
///
/// Every namespace the model declares has a row there, and a namespace outside the enum is
/// refused by each handler before it reaches here — so the last arm, a namespace with no row
/// whose request named no role either, is only what makes this function total: it answers the
/// unsuffixed base role [`service_linked_role_name`] falls back to, which is what this
/// service already reported for such a registration.
pub fn effective_role_arn(service_namespace: &str, requested: Option<&str>) -> String {
    if has_service_linked_role(service_namespace) {
        return service_linked_role_arn(service_namespace);
    }
    requested.map_or_else(
        || service_linked_role_arn(service_namespace),
        ToString::to_string,
    )
}

/// Whether Application Auto Scaling scales `service_namespace` through a service-linked role
/// of its own — a row of the published catalog ([`SERVICE_LINKED_ROLE_SUFFIXES`]).
pub fn has_service_linked_role(service_namespace: &str) -> bool {
    service_linked_role_suffix(service_namespace).is_some()
}

/// The service principal a namespace's scalable targets are scaled through: the namespace's
/// own label in front of Application Auto Scaling's principal, which is the form every row of
/// IAM's published service-linked-role catalog takes for this service.
pub fn service_linked_role_principal(service_namespace: &str) -> String {
    format!("{service_namespace}.application-autoscaling.amazonaws.com")
}

/// The base name every Application Auto Scaling service-linked role starts with.
const SERVICE_LINKED_ROLE_BASE: &str = "AWSServiceRoleForApplicationAutoScaling";

/// The resource-type suffix each namespace's service-linked role name carries.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto
/// Scaling User Guide, which publishes one role per supported service, each named
/// `AWSServiceRoleForApplicationAutoScaling_<suffix>`. The rows are the model's own
/// `ServiceNamespace` value set (`SERVICE_NAMESPACE_VALUES`), so every modelled namespace
/// has one; a namespace outside the enum — which every handler refuses before reaching
/// here — falls back to the unsuffixed base name rather than to a guessed suffix.
const SERVICE_LINKED_ROLE_SUFFIXES: &[(&str, &str)] = &[
    ("appstream", "AppStreamFleet"),
    ("cassandra", "CassandraTable"),
    ("comprehend", "ComprehendEndpoint"),
    ("custom-resource", "CustomResource"),
    ("dynamodb", "DynamoDBTable"),
    ("ec2", "EC2SpotFleetRequest"),
    ("ecs", "ECSService"),
    ("elasticache", "ElastiCacheRG"),
    ("elasticmapreduce", "EMRInstanceGroup"),
    ("kafka", "KafkaCluster"),
    ("lambda", "LambdaConcurrency"),
    ("neptune", "NeptuneCluster"),
    ("rds", "RDSCluster"),
    ("sagemaker", "SageMakerEndpoint"),
    ("workspaces", "WorkSpacesPool"),
];

/// The name of the service-linked role a namespace's scalable targets are scaled through.
pub fn service_linked_role_name(service_namespace: &str) -> String {
    match service_linked_role_suffix(service_namespace) {
        Some(suffix) => format!("{SERVICE_LINKED_ROLE_BASE}_{suffix}"),
        None => SERVICE_LINKED_ROLE_BASE.to_string(),
    }
}

/// The resource-type label `service_namespace`'s service-linked role is named for. See
/// [`SERVICE_LINKED_ROLE_SUFFIXES`].
fn service_linked_role_suffix(service_namespace: &str) -> Option<&'static str> {
    SERVICE_LINKED_ROLE_SUFFIXES
        .iter()
        .find(|(namespace, _)| *namespace == service_namespace)
        .map(|(_, suffix)| *suffix)
}

/// The managed policy Application Auto Scaling attaches to the service-linked role of
/// `service_namespace`, as the ARN IAM files an AWS-managed policy under.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto Scaling
/// User Guide, which lists one policy per supported service, each named
/// `AWSApplicationAutoscaling<resource type>Policy` for the SAME resource-type label its role
/// name carries ([`SERVICE_LINKED_ROLE_SUFFIXES`]) and filed under the path IAM reserves for
/// the policies of a service-linked role, so no identity other than that role can take it.
/// `None` for a namespace with no published role, which has no published policy either.
pub fn service_linked_role_policy_arn(service_namespace: &str) -> Option<String> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

    let suffix = service_linked_role_suffix(service_namespace)?;
    Some(format!(
        "arn:aws:iam::{}:policy{}AWSApplicationAutoscaling{suffix}Policy",
        iam::AWS_MANAGED_POLICY_ACCOUNT,
        iam::SERVICE_LINKED_ROLE_PATH_PREFIX
    ))
}

/// Create the service-linked role `service_namespace`'s scalable targets are scaled through,
/// in IAM, unless the account already holds a role under that name.
///
/// "If the service supports service-linked roles, Application Auto Scaling uses a
/// service-linked role, which it creates if it does not yet exist": the FIRST registration in
/// a namespace is what brings that namespace's role into the account, so the `RoleARN` a
/// scalable target reports names a role a client can then read and inspect. The record is
/// written by IAM's own `insert_service_linked_role`, the one place a service-linked role's
/// record is built, so it is the record `CreateServiceLinkedRole` would have written — and the
/// permissions policy the service attaches to its OWN role is attached with it (which is why
/// this writes the attachment directly: that policy is not one an identity may attach for
/// itself).
///
/// Call it with THIS service's store NOT held: it takes IAM's. It does nothing when this
/// emulator holds no IAM store, when the account already holds the role, or when the account
/// is at IAM's own role quota, where a create would have been refused too.
pub fn ensure_service_linked_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    service_namespace: &str,
) {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

    let Some(store) = iam::service_state(engine) else {
        return;
    };
    let role_name = service_linked_role_name(service_namespace);
    let key = iam::name_key(&role_name);
    let mut store = store.lock();
    if store.roles.contains_key(&key) || store.roles.len() >= iam::MAX_ROLES_PER_ACCOUNT {
        return;
    }
    let principal = service_linked_role_principal(service_namespace);
    let role = iam::insert_service_linked_role(&mut store, &principal, &role_name, None);
    ::tracing::debug!(
        service_namespace = %service_namespace,
        role_arn = %role.arn,
        "the service-linked role this namespace is scaled through was created"
    );
    if let Some(policy_arn) = service_linked_role_policy_arn(service_namespace) {
        store
            .role_attached_policies
            .entry(key)
            .or_default()
            .insert(policy_arn);
    }
}

/// The smallest capacity a scalable dimension can be registered with.
///
/// A scalable dimension's capacity is the owning service's own provisioning unit, so its
/// floor is that service's documented minimum rather than zero for everything: the Amazon
/// DynamoDB Developer Guide ("Managing settings on DynamoDB provisioned capacity tables":
/// the smallest provisionable read or write capacity is one unit) and the Amazon Keyspaces
/// Developer Guide (the same for a provisioned-throughput table) both put that floor at
/// one unit, so auto scaling cannot be asked to take such a resource below it. A dimension
/// whose owning service documents no floor — a desired count, a replica count, a node
/// count — keeps zero, which is what lets a target scale all the way in.
const MIN_CAPACITY_FLOORS: &[(&str, i32)] = &[
    ("cassandra:table:ReadCapacityUnits", 1),
    ("cassandra:table:WriteCapacityUnits", 1),
    ("dynamodb:index:ReadCapacityUnits", 1),
    ("dynamodb:index:WriteCapacityUnits", 1),
    ("dynamodb:table:ReadCapacityUnits", 1),
    ("dynamodb:table:WriteCapacityUnits", 1),
];

/// The smallest capacity `scalable_dimension` accepts. See [`MIN_CAPACITY_FLOORS`].
pub fn min_capacity_floor(scalable_dimension: &str) -> i32 {
    MIN_CAPACITY_FLOORS
        .iter()
        .find(|(dimension, _)| *dimension == scalable_dimension)
        .map_or(0, |(_, floor)| *floor)
}

/// Which end of a capacity window a refusal is about, and how the service names it.
///
/// `ResourceCapacity` carries no `@range` in the model, so neither bound below is a
/// modelled constraint: both are rules this service enforces, and each is stated in the
/// service's own words rather than as a constraint clause naming the member path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityBound {
    /// `MinCapacity`, on whichever shape carries it.
    Minimum,
    /// `MaxCapacity`.
    Maximum,
}

impl CapacityBound {
    /// How the service names this bound in a sentence.
    fn noun(self) -> &'static str {
        match self {
            Self::Minimum => "Minimum",
            Self::Maximum => "Maximum",
        }
    }

    /// How the service names the member a request SETS this bound through, in the text of
    /// the activity that records the setting: the member's own short name
    /// (`ScalableTargetAction$MinCapacity` is "min capacity"), as in "Setting min capacity
    /// to 4".
    fn action_member_phrase(self) -> &'static str {
        match self {
            Self::Minimum => "min capacity",
            Self::Maximum => "max capacity",
        }
    }

    /// How the service names this bound in the CAUSE of the capacity movement that
    /// enforcing it performed, as in "minimum capacity was set to 4": the bound's own noun,
    /// which is the subject of that sentence rather than a member of a request.
    fn enforcement_noun(self) -> &'static str {
        match self {
            Self::Minimum => "minimum",
            Self::Maximum => "maximum",
        }
    }
}

/// How this service states a capacity below the floor its dimension can be provisioned at.
///
/// The floor is not in the model (see [`MIN_CAPACITY_FLOORS`]), so the message is the only
/// place a caller learns it: the service names the bound it refused and the value that
/// bound may not go under. The INVERTED pair is a different rule, with its own sentence —
/// [`CAPACITY_RANGE_INVERTED_MESSAGE`].
pub fn capacity_below_floor_message(bound: CapacityBound, floor: i32) -> String {
    format!("{} capacity cannot be less than {floor}", bound.noun())
}

/// How this service states an operation addressed at a scalable target that does not exist.
///
/// `ObjectNotFoundException` is "thrown if the scalable target with the specified service
/// namespace, resource ID, and scalable dimension does not exist", and the sentence names
/// exactly that triple, built from the request. The VERB follows what the operation was
/// doing with the registration, which is how each operation's own recorded refusal words it:
/// an operation that needs an existing registration to proceed is told that no scalable
/// target is REGISTERED for the triple ([`target_not_registered_message`]), while one that
/// looked the registration up in order to remove it is told that none was FOUND
/// ([`target_not_found_message`]).
fn missing_target_message(target: &TargetKey, verb: &str) -> String {
    format!(
        "No scalable target {verb} for service namespace: {}, resource ID: {}, scalable \
         dimension: {}",
        target.service_namespace, target.resource_id, target.scalable_dimension
    )
}

/// The refusal of an operation that cannot proceed because nothing registered the scalable
/// target it addresses — "You cannot create a scaling policy until you have registered the
/// resource as a scalable target", and the same for a scheduled action. See
/// [`missing_target_message`].
pub fn target_not_registered_message(target: &TargetKey) -> String {
    missing_target_message(target, "registered")
}

/// The refusal of an operation that went looking for the registration itself and did not
/// find it, which is `DeregisterScalableTarget`. See [`missing_target_message`].
pub fn target_not_found_message(target: &TargetKey) -> String {
    missing_target_message(target, "found")
}

/// The refusal of an operation addressed at a scheduled action its scalable target does not
/// hold.
///
/// A scheduled action is a sub-resource OF a scalable target, so the sentence identifies the
/// target by the TARGET's own identifier ([`scalable_target_id`]) rather than by the triple
/// the request addressed it with, beside the namespace the request named and the action's own
/// name. An operation resolves the target before the action it holds, so an identity nothing
/// registered a target for is reported as the missing TARGET
/// ([`target_not_registered_message`]) — that refusal has a triple to name and this one has
/// no target to read an identifier off.
pub fn scheduled_action_not_found_message(
    service_namespace: &str,
    scalable_target_id: &str,
    scheduled_action_name: &str,
) -> String {
    format!(
        "No scheduled action found for service namespace: {service_namespace}, scalable \
         target ID: {scalable_target_id}, scheduled action name: {scheduled_action_name}"
    )
}

/// How this service refuses a registration whose resource cannot carry a scalable target at
/// all: the recorded envelope, with the REASON that resource's own state states.
///
/// The envelope is fixed and the reason is the variable part, so every such refusal reads the
/// same way whichever property of the resource made it unscalable — a table in a billing mode
/// that provisions no throughput being the one this emulator can resolve.
pub fn scalable_target_validation_failed(reason: &str) -> String {
    format!("Validation failed for scalable target. Reason: {reason}")
}

/// How this service refuses a registration that carries `Tags` for a target it already
/// holds.
///
/// `Tags` tags the target "when it is created", and "to tag an existing scalable target, use
/// the TagResource operation" — so the call asks for something this operation does not do,
/// and the refusal says which operation does. The sentence is fixed: the rule is about the
/// combination of a registered target and a tag, so there is no value of the request to echo.
pub const TAGGED_TARGET_ALREADY_EXISTS: &str =
    "The scalable target that you tried to tag already exists. To update tags on an existing \
     scalable target, use the TagResource API.";

/// How this service refuses a registration whose RESOURCE cannot carry the target the
/// request asked for: the resource and the dimension the request named, and the reason that
/// resource's own state states.
///
/// The envelope is fixed and the reason is the variable part, so every such refusal reads
/// the same way whichever property of the resource made it unscalable. It is the
/// resource-naming sibling of [`scalable_target_validation_failed`], which states a reason
/// about the TARGET and so has no resource of its own to name.
pub fn resource_validation_failed(
    resource_id: &str,
    scalable_dimension: &str,
    reason: &str,
) -> String {
    format!(
        "Validation failed for resource: {resource_id}, scalable dimension: \
         {scalable_dimension}. Reason: {reason}"
    )
}

/// `len` lowercase hex characters derived from `seed`, clamped to the 64 characters a
/// SHA-256 digest renders to so a caller asking for more gets the whole digest rather
/// than a panic.
fn opaque_id(seed: &str, len: usize) -> String {
    use sha2::Digest;
    let digest = hex::encode(sha2::Sha256::digest(seed.as_bytes()));
    debug_assert!(
        len <= digest.len(),
        "an opaque id cannot be longer than the {} characters of a SHA-256 digest",
        digest.len()
    );
    digest[..len.min(digest.len())].to_string()
}

// ---------------------------------------------------------------------------
// Output shapes, rebuilt from the records
// ---------------------------------------------------------------------------

/// A `SystemTime` as the modelled timestamp type.
pub fn timestamp_of(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The `ScalableTarget` a `Describe` returns for a stored record.
pub fn scalable_target_of(key: &TargetKey, record: &ScalableTargetRecord) -> ScalableTarget {
    ScalableTarget {
        service_namespace: key.service_namespace.clone(),
        resource_id: key.resource_id.clone(),
        scalable_dimension: key.scalable_dimension.clone(),
        min_capacity: record.min_capacity,
        max_capacity: record.max_capacity,
        // Only a predictive-scaling policy produces a predicted capacity, and this
        // emulator forecasts none, so the member stays absent rather than invented.
        predicted_capacity: None,
        role_arn: record.role_arn.clone(),
        creation_time: timestamp_of(record.creation_time),
        suspended_state: Some(reported_suspended_state(record.suspended_state.as_ref())),
        scalable_target_arn: Some(record.arn.clone()),
    }
}

/// The `SuspendedState` a report of a scalable target carries: all three flags the shape
/// declares, each `false` where nothing has suspended that activity.
///
/// A scaling activity is either suspended or it is not, and a target nobody has suspended
/// anything on has all three running — so each flag is reported as the boolean it is. A
/// member left out would make a caller that reads whether scaling is suspended guess, and
/// the same holds for a partial suspension: suspending one activity says nothing about the
/// other two, which are still running.
pub fn reported_suspended_state(stored: Option<&SuspendedState>) -> SuspendedState {
    let flag = |read: fn(&SuspendedState) -> Option<bool>| {
        Some(stored.and_then(read).unwrap_or(false))
    };
    SuspendedState {
        dynamic_scaling_in_suspended: flag(|state| state.dynamic_scaling_in_suspended),
        dynamic_scaling_out_suspended: flag(|state| state.dynamic_scaling_out_suspended),
        scheduled_scaling_suspended: flag(|state| state.scheduled_scaling_suspended),
    }
}

/// The `ScalingPolicy` a `Describe` returns for a stored record.
///
/// `Alarms` is left for the READER to fill in: the alarms associated with a policy are the ones
/// CloudWatch holds that name it ([`alarms_invoking_policies`]), not a value this record keeps,
/// and that read happens with this service's lock released.
pub fn scaling_policy_of(key: &PolicyKey, record: &ScalingPolicyRecord) -> ScalingPolicy {
    ScalingPolicy {
        policy_arn: record.policy_arn.clone(),
        policy_name: key.policy_name.clone(),
        service_namespace: key.target.service_namespace.clone(),
        resource_id: key.target.resource_id.clone(),
        scalable_dimension: key.target.scalable_dimension.clone(),
        policy_type: record.policy_type.clone(),
        step_scaling_policy_configuration: record.step_config.clone(),
        target_tracking_scaling_policy_configuration: record.target_tracking_config.clone(),
        predictive_scaling_policy_configuration: record.predictive_config.clone(),
        alarms: None,
        creation_time: timestamp_of(record.creation_time),
    }
}

/// The capacity a scalable dimension names, in the words a scaling activity's text uses.
///
/// A dimension "consists of the service namespace, resource type, and scaling property"
/// (the model's own documentation for the member), so the property is its last segment,
/// and splitting that at its word boundaries gives the noun the model's own
/// `DescribeScalingActivities` example carries: `ecs:service:DesiredCount` yields
/// "desired count", as in "Setting desired count to 1.". For a namespace whose own
/// activities name the resource the property belongs to, the resource type opens the noun
/// ([`RESOURCE_TYPE_IN_CAPACITY_NOUN`]).
pub fn capacity_noun(scalable_dimension: &str) -> String {
    // A namespace whose dimension does not NAME the capacity it scales says what it scales
    // in its own words instead.
    if let Some((_, noun)) = NAMESPACE_CAPACITY_NOUNS
        .iter()
        .find(|(namespace, _)| *namespace == dimension_namespace(scalable_dimension))
    {
        return (*noun).to_string();
    }
    let property = scalable_dimension
        .rsplit(':')
        .next()
        .unwrap_or(scalable_dimension);
    let mut words: Vec<String> = Vec::new();
    for character in property.chars() {
        match words.last_mut() {
            Some(word) if !character.is_uppercase() => word.push(character),
            _ => words.push(character.to_lowercase().to_string()),
        }
    }
    // The namespace whose activities name the resource the property belongs to opens the
    // noun with that resource type, which is the dimension's own middle segment.
    if RESOURCE_TYPE_IN_CAPACITY_NOUN.contains(&dimension_namespace(scalable_dimension)) {
        let resource_type = dimension_resource_type(scalable_dimension);
        if !resource_type.is_empty() {
            words.insert(0, resource_type.to_string());
        }
    }
    words.join(" ")
}

/// The namespaces whose scaling-activity text names the RESOURCE TYPE before the scaled
/// property.
///
/// The noun is the owning service's own: an Amazon Keyspaces activity reads "Setting table
/// read capacity units to 4." where a DynamoDB one reads "Setting read capacity units to 4."
/// for the same property, so which spelling a dimension gets follows its namespace rather
/// than being one rule for every dimension. A namespace with no row here names the property
/// alone, as the model's own `DescribeScalingActivities` example does for
/// `ecs:service:DesiredCount` ("Setting desired count to 1.").
const RESOURCE_TYPE_IN_CAPACITY_NOUN: &[&str] = &["cassandra"];

/// The capacity noun a namespace's activities use where the dimension names no capacity to
/// read one off.
///
/// `custom-resource:ResourceType:Property` is the one dimension whose resource type and
/// scaling property are PLACEHOLDERS — "Custom resources are not supported with a resource
/// type […] The unique identifier is defined by the service provider" (the model's own
/// `ResourceId` documentation) — so reading "property" off it would name nothing. The capacity
/// such a target scales is the one the custom-resource contract carries
/// ([`custom_resource_endpoint`]): the resource's desired capacity, which is what an activity
/// of that namespace says it is setting.
const NAMESPACE_CAPACITY_NOUNS: &[(&str, &str)] = &[("custom-resource", "desired capacity")];

/// The service namespace a `ScalableDimension` names: its first segment.
fn dimension_namespace(scalable_dimension: &str) -> &str {
    scalable_dimension.split(':').next().unwrap_or_default()
}

/// What a capacity movement of `target` to `capacity` is described as: "a simple description
/// of what action the scaling activity intends to accomplish", in the words the model's own
/// `DescribeScalingActivities` example carries ("Setting desired count to 1.").
pub fn movement_description(target: &TargetKey, capacity: i32) -> String {
    format!(
        "Setting {} to {capacity}.",
        capacity_noun(&target.scalable_dimension)
    )
}

/// The `StatusMessage` of a capacity movement that has TAKEN EFFECT on a scalable target:
/// the capacity the target was set to, and the service that owns the resource it was set
/// on, in the shape the model's own `DescribeScalingActivities` example carries
/// ("Successfully set desired count to 1. Change successfully fulfilled by ecs.").
///
/// One capacity, one template: the movement names the capacity IT set, and a movement a later
/// one took over reports that same clause beside what displaced it
/// ([`overridden_status_message`]).
pub fn fulfilled_status_message(target: &TargetKey, capacity: i32) -> String {
    format!(
        "Successfully set {} to {capacity}. Change successfully fulfilled by {}.",
        capacity_noun(&target.scalable_dimension),
        target.service_namespace
    )
}

/// The `StatusMessage` of a capacity movement a LATER movement of the same scalable target
/// displaced before the capacity it set was confirmed: the capacity this movement itself
/// applied, and the capacity that was put in its place.
///
/// Two clauses, because an overtaken movement is two facts: what it did — the same
/// "Successfully set …" clause a movement that was confirmed reports
/// ([`fulfilled_status_message`]), since this movement did apply that capacity — and that the
/// target was moved on from it before the change was confirmed, which names the capacity that
/// displaced it. What this activity INTENDED stays its `Description`
/// ([`movement_description`]).
pub fn overridden_status_message(target: &TargetKey, capacity: i32, changed_to: i32) -> String {
    format!(
        "Successfully set {noun} to {capacity}. Found it was later changed to {changed_to}.",
        noun = capacity_noun(&target.scalable_dimension)
    )
}

/// The `StatusMessage` of a capacity movement this service has PERFORMED whose change the
/// service owning the scaled resource has not confirmed yet: the capacity the movement set,
/// and the service the change is still waiting on, in the wording a recorded in-flight
/// activity of this service carries ("Successfully set desired count to 6. Waiting for change
/// to be fulfilled by ecs.").
///
/// The same two variables as [`fulfilled_status_message`], which is what the same activity
/// reports from its fulfilment instant on: a reader of either learns the capacity the
/// activity settled on and how far the fulfilment got.
pub fn awaiting_fulfilment_status_message(target: &TargetKey, capacity: i32) -> String {
    format!(
        "Successfully set {} to {capacity}. Waiting for change to be fulfilled by {}.",
        capacity_noun(&target.scalable_dimension),
        target.service_namespace
    )
}

// ---------------------------------------------------------------------------
// The text of an activity that is not a capacity movement
// ---------------------------------------------------------------------------

/// The `Cause` of the activity that records a scheduled action FIRING: the action's own
/// name, which is how a client joins a scheduled action to the history it produced.
///
/// A firing and the capacity movement enforcing it are two activities, and this is the one
/// the firing itself is recorded as: its cause is the action, so the sentence names the
/// action by the name it is addressed by and says nothing about capacity — the bound it set
/// is what its DESCRIPTION states ([`bounds_setting_description`]) and the movement is the
/// other activity ([`capacity_bound_enforcement_cause`]).
pub fn scheduled_action_dispatch_cause(scheduled_action_name: &str) -> String {
    format!("scheduled action name {scheduled_action_name} was triggered")
}

/// The bounds one setting of a scalable target's capacity window names, as a phrase: `min
/// capacity to 4`, and both bounds joined with "and" when one call sets both.
///
/// The members are named in the order `ScalableTargetAction` declares them, so a call that
/// sets both reads the same way whichever order the request happened to carry.
fn bounds_phrase(bounds: &[(CapacityBound, i32)]) -> String {
    bounds
        .iter()
        .map(|(bound, capacity)| format!("{} to {capacity}", bound.action_member_phrase()))
        .collect::<Vec<String>>()
        .join(" and ")
}

/// The `Description` of the activity that records a scheduled action firing: "a simple
/// description of what action the scaling activity intends to accomplish", which for a
/// firing is the capacity bounds the action sets. It is a phrase rather than a sentence, so
/// it carries no full stop.
pub fn bounds_setting_description(bounds: &[(CapacityBound, i32)]) -> String {
    format!("Setting {}", bounds_phrase(bounds))
}

/// The `StatusMessage` of a firing whose bounds the target now carries.
pub fn bounds_set_status_message(bounds: &[(CapacityBound, i32)]) -> String {
    format!("Successfully set {}", bounds_phrase(bounds))
}

/// The `StatusMessage` of a firing the target's registered window REFUSED, with the reason
/// it was refused for.
///
/// The reason is the one every refusal of an inverted capacity window carries
/// ([`CAPACITY_RANGE_INVERTED_MESSAGE`]) — the same sentence the operation that takes such
/// a window answers inline — because it is the same rule: the bounds the firing would leave
/// the target with are not a range.
pub fn bounds_refused_status_message(bounds: &[(CapacityBound, i32)], reason: &str) -> String {
    format!("Failed to set {}. Reason: {reason}", bounds_phrase(bounds))
}

/// The `Cause` of the capacity movement that ENFORCING a capacity bound performed: the
/// bound that was set and the value it was set to.
///
/// A movement of a target's capacity to a bound is caused by that bound being set, whoever
/// set it — a registration that narrows the window, or a scheduled action's firing — so
/// both record the same cause, and the activity naming the action is the dispatch beside it
/// ([`scheduled_action_dispatch_cause`]).
pub fn capacity_bound_enforcement_cause(bound: CapacityBound, capacity: i32) -> String {
    format!(
        "{} capacity was set to {capacity}",
        bound.enforcement_noun()
    )
}

/// The `ScalingActivity` a `Describe` returns for a stored record, as of `now`.
///
/// An activity that has not been fulfilled yet is reported in its non-terminal status with
/// no `EndTime`; once it has been it reports the terminal status it was recorded with, and
/// the `EndTime` is the instant of that fulfilment — so the same record answers the same
/// thing to every reader at the same clock reading.
///
/// Either way it carries a `StatusMessage` ("a simple message about the current status of the
/// scaling activity") naming the capacity the movement set: an activity that has started is
/// past the statuses that report no progress at all, so what it reports is how far the
/// fulfilment of that capacity got — awaited while the change is on its way
/// ([`awaiting_fulfilment_status_message`]), settled once it is terminal.
pub fn scaling_activity_of(record: &ScalingActivityRecord, now: SystemTime) -> ScalingActivity {
    // Non-terminal while the span it was given is still running, and while the service that
    // owns the scaled resource has still to make the capacity take effect: a movement waits
    // on both, and either one of them is enough to keep it in progress.
    let pending = record.awaiting_owner || record.fulfilled_at.is_some_and(|due| due > now);
    ScalingActivity {
        activity_id: record.activity_id.clone(),
        service_namespace: record.target.service_namespace.clone(),
        resource_id: record.target.resource_id.clone(),
        scalable_dimension: record.target.scalable_dimension.clone(),
        description: record.description.clone(),
        cause: record.cause.clone(),
        start_time: timestamp_of(record.start_time),
        end_time: if pending {
            None
        } else {
            record.fulfilled_at.map(timestamp_of)
        },
        status_code: if pending {
            ACTIVITY_IN_PROGRESS.to_string()
        } else {
            record.terminal_status_code.clone()
        },
        status_message: if pending {
            Some(awaiting_fulfilment_status_message(
                &record.target,
                record.capacity,
            ))
        } else {
            record.terminal_status_message.clone()
        },
        details: record.details.clone(),
        // The member is "only available when IncludeNotScaledActivities is true" AND
        // the activity is one that was not scaled; an activity that scaled carries none.
        not_scaled_reasons: (!record.not_scaled_reasons.is_empty())
            .then(|| record.not_scaled_reasons.clone()),
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `MaxResults`' documented bounds and default: "This value can be between 1 and 50. The
/// default value is 50."
pub const MIN_MAX_RESULTS: i32 = 1;
/// See [`MIN_MAX_RESULTS`].
pub const MAX_MAX_RESULTS: i32 = 50;
/// See [`MIN_MAX_RESULTS`].
pub const DEFAULT_MAX_RESULTS: i32 = 50;

/// The page size a `Describe` serves, or the `ValidationException` message for a
/// `MaxResults` outside its documented range.
pub fn page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
    if !(MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: \
             Member must be between {MIN_MAX_RESULTS} and {MAX_MAX_RESULTS}"
        ));
    }
    Ok(requested as usize)
}

/// The field separator inside a `NextToken`'s payload. A unit separator cannot appear in
/// a namespace, a resource id, a scalable dimension or a policy name, so splitting on it
/// recovers exactly the fields that were encoded.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// An opaque `NextToken` carrying the Region that issued it and the key a page ended on.
///
/// A page is a walk through ONE Region's resources, so the token names that Region
/// ([`decode_next_token`] refuses one issued by another).
pub fn encode_next_token(region: &str, fields: &[&str]) -> String {
    let mut payload = vec![region];
    payload.extend_from_slice(fields);
    hex::encode(payload.join(&TOKEN_SEPARATOR.to_string()))
}

/// How this service states a `NextToken` it did not issue.
///
/// `InvalidNextTokenException`'s own documentation is "The next token supplied was
/// invalid"; the sentence the service puts on the wire is this one, which names the member
/// as the API spells it. The token is not echoed: a caller gets back the token it already
/// holds, and the refusal is about the whole value rather than a part of it.
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// The fields a `NextToken` carries, or `None` when the token is not one this endpoint
/// issued — the caller answers that with `InvalidNextTokenException`.
///
/// A token issued by ANOTHER Region is not one this endpoint issued: the resources it walks
/// are regional, so resuming its key here would resume inside a different set. The Region
/// is part of the payload and is checked rather than returned.
pub fn decode_next_token(token: &str, region: &str, field_count: usize) -> Option<Vec<String>> {
    let bytes = hex::decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let mut fields: Vec<String> = text
        .split(TOKEN_SEPARATOR)
        .map(ToString::to_string)
        .collect();
    if fields.len() != field_count + 1 || fields.first().is_none_or(|issuer| issuer != region) {
        return None;
    }
    fields.remove(0);
    Some(fields)
}

/// Cut `items` (already filtered and in key order) down to one page.
///
/// Returns the page and, only when items remain beyond it, the `NextToken` that resumes
/// after the last item emitted — so the final page carries no token and a client's loop
/// terminates.
pub fn take_page<T>(
    mut items: Vec<T>,
    page_size: usize,
    token_of: impl Fn(&T) -> String,
) -> (Vec<T>, Option<String>) {
    if items.len() <= page_size {
        return (items, None);
    }
    items.truncate(page_size);
    let token = items.last().map(token_of);
    (items, token)
}

// ---------------------------------------------------------------------------
// The service's own refusal texts
// ---------------------------------------------------------------------------
//
// A refusal this SERVICE reaches (rather than one the protocol's boundary validator
// reaches from a modelled constraint) carries the service's own sentence: a statement of
// the rule that was broken, naming the request's own values where the rule is about them.
// It is never the boundary validator's "Value 'X' at 'member' failed to satisfy
// constraint: Member must …" template, which belongs to the modelled constraints the
// framework enforces, and it never names a value the caller did not supply (the request's
// region, say). The texts below are the ones several handlers share; a text only one
// handler reaches lives beside it.

/// The message every refusal of an inverted capacity window carries.
///
/// A capacity window is refused for ONE reason — "scaling policies will not scale capacity
/// to values that are outside of the minimum and maximum range", which is not a range when
/// the bounds are inverted — so every operation that takes a window answers it with the
/// same sentence: `RegisterScalableTarget`'s `MinCapacity`/`MaxCapacity` pair and
/// `PutScheduledAction`'s `ScalableTargetAction` alike. Also the text the sibling Auto
/// Scaling Plans service answers a `ScalingInstruction` with, for the same rule.
pub const CAPACITY_RANGE_INVERTED_MESSAGE: &str =
    "Maximum capacity cannot be less than minimum capacity";

/// The message a request addressed to an ARN that resolves to no registered scalable target
/// carries.
///
/// The resource a tag operation addresses is the scalable target its `ResourceARN` names, and
/// either that target is registered or it is not: the refusal states exactly that, and the
/// ARN asked about is reported in the modelled `ResourceName` member rather than spliced into
/// the sentence. All three tag operations answer it for every ARN that resolves to no
/// registered target — one this service never minted and one whose target has since been
/// deregistered alike, since neither identifies a scalable target now — and they answer it
/// with the client-error status rather than with the 404 the shape's `@httpError` declares
/// (`wire_refusals::WireRefusal::ScalableTargetNotFound`).
pub const SCALABLE_TARGET_NOT_FOUND_MESSAGE: &str = "Scalable Target not found";

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

/// `Ok` when `value` is one of `allowed`, else the `ValidationException` message naming
/// the member, the offending value and the permitted set — the shape AWS's own
/// constraint refusals take.
pub fn check_enum(member: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    ))
}

/// `Ok` when a `Describe`'s dimension filter is accompanied by the resource it narrows.
///
/// The model documents the rule on the member itself: "If you specify a scalable
/// dimension, you must also specify a resource ID." A dimension names a capacity
/// property of ONE resource, so filtering on it alone is a filter with no subject.
pub fn check_dimension_requires_resource_id(
    resource_id: Option<&str>,
    scalable_dimension: Option<&str>,
) -> Result<(), String> {
    match (resource_id, scalable_dimension) {
        (None, Some(_)) => Err(DIMENSION_WITHOUT_RESOURCE_ID.to_string()),
        _ => Ok(()),
    }
}

/// How the service states that refusal: its own fixed sentence, naming neither the
/// member path nor the value — the rule is about which members were COMBINED, so there is
/// no one offending value to echo.
pub const DIMENSION_WITHOUT_RESOURCE_ID: &str =
    "Scalable dimension cannot be provided without a resource ID.";

/// The same rule for an operation whose resource filter is a LIST of resource ids.
///
/// `DescribeScalableTargets` narrows by `ResourceIds` rather than by one `ResourceId`, and
/// the model states the rule on that member too ("If you specify a scalable dimension, you
/// must also specify a resource ID"). An absent list and an EMPTY one each name no resource,
/// so a dimension filter sent with either is a filter with no subject — the same refusal as
/// its single-id siblings ([`check_dimension_requires_resource_id`]), worded for the member
/// that operation reads ([`DIMENSION_WITHOUT_RESOURCE_IDS`]).
pub fn check_dimension_requires_resource_ids(
    resource_ids: Option<&[String]>,
    scalable_dimension: Option<&str>,
) -> Result<(), String> {
    let named = resource_ids.and_then(|ids| ids.first()).map(String::as_str);
    match (named, scalable_dimension) {
        (None, Some(_)) => Err(DIMENSION_WITHOUT_RESOURCE_IDS.to_string()),
        _ => Ok(()),
    }
}

/// How that refusal is worded for the member a LIST-filtering operation reads: the same
/// sentence, naming `ResourceIds` the way the plural member is named.
pub const DIMENSION_WITHOUT_RESOURCE_IDS: &str =
    "Scalable dimension cannot be provided without resource IDs.";

/// `Ok` when `scalable_dimension` belongs to `service_namespace`.
///
/// A scalable dimension is not independent of the namespace it is named with: the
/// model's own documentation for the member says "this string consists of the service
/// namespace, resource type, and scaling property", so every value in the
/// `ScalableDimension` enum is `<service namespace>:<resource type>:<scaling property>`
/// and the pair is checkable from the dimension itself rather than from a hand-written
/// table. The two halves being individually legal is not enough — a dimension belongs to
/// exactly one namespace.
pub fn check_dimension_namespace(
    service_namespace: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    if scalable_dimension.split(':').next() == Some(service_namespace) {
        return Ok(());
    }
    Err(format!(
        "Value '{scalable_dimension}' at 'scalableDimension' failed to satisfy constraint: \
         Member must be a scalable dimension of the '{service_namespace}' service namespace"
    ))
}

/// The resource type a `ScalableDimension` names: its middle segment.
pub fn dimension_resource_type(scalable_dimension: &str) -> &str {
    scalable_dimension.split(':').nth(1).unwrap_or_default()
}

/// The scaling property a `ScalableDimension` names: its last segment.
pub fn dimension_scaling_property(scalable_dimension: &str) -> &str {
    scalable_dimension.split(':').nth(2).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The ResourceId a ScalableDimension requires
// ---------------------------------------------------------------------------

/// The `ResourceId` form each (service namespace, resource type) pair documents.
///
/// `ResourceId` and `ScalableDimension` are not independent members: the dimension "consists
/// of the service namespace, resource type, and scaling property" and the resource id
/// "consists of the resource type and unique identifier", so the resource type the dimension
/// names fixes the shape the resource id has to take. Every row is the example the model's own
/// `ResourceId` documentation gives for that resource type
/// (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#RegisterScalableTargetRequest",member="ResourceId")`),
/// rewritten as a template: `{...}` is one non-empty segment carrying neither `:` nor `/`,
/// and `{...*}` is the whole non-empty remainder.
///
/// `custom-resource` has no row on purpose — the same documentation says "Custom resources are
/// not supported with a resource type […] The unique identifier is defined by the service
/// provider", so there is no shape to check.
const RESOURCE_ID_FORMS: &[(&str, &str, &str)] = &[
    ("appstream", "fleet", "fleet/{fleet name}"),
    ("cassandra", "table", "keyspace/{keyspace}/table/{table}"),
    (
        "comprehend",
        "document-classifier-endpoint",
        "arn:{partition}:comprehend:{region}:{account}:document-classifier-endpoint/{name*}",
    ),
    (
        "comprehend",
        "entity-recognizer-endpoint",
        "arn:{partition}:comprehend:{region}:{account}:entity-recognizer-endpoint/{name*}",
    ),
    ("dynamodb", "table", "table/{table name}"),
    ("dynamodb", "index", "table/{table name}/index/{index name}"),
    ("ec2", "spot-fleet-request", "spot-fleet-request/{request id}"),
    ("ecs", "service", "service/{cluster name}/{service name}"),
    ("elasticache", "cache-cluster", "cache-cluster/{cluster name}"),
    (
        "elasticache",
        "replication-group",
        "replication-group/{replication group name}",
    ),
    (
        "elasticmapreduce",
        "instancegroup",
        "instancegroup/{cluster id}/{instance group id}",
    ),
    (
        "kafka",
        "broker-storage",
        "arn:{partition}:kafka:{region}:{account}:cluster/{name*}",
    ),
    (
        "lambda",
        "function",
        "function:{function name}:{version or alias}",
    ),
    ("neptune", "cluster", "cluster:{cluster name}"),
    ("rds", "cluster", "cluster:{cluster name}"),
    (
        "sagemaker",
        "inference-component",
        "inference-component/{inference component name}",
    ),
    (
        "sagemaker",
        "variant",
        "endpoint/{endpoint name}/variant/{variant name}",
    ),
    ("workspaces", "workspacespool", "workspacespool/{pool id}"),
];

/// How this service refuses a (`ServiceNamespace`, `ResourceId`, `ScalableDimension`) triple
/// whose three members do not describe ONE resource between them.
///
/// The three identify the scalable target together — the dimension "consists of the service
/// namespace, resource type, and scaling property" and the resource id "consists of the
/// resource type and unique identifier" — so a resource id of another resource type than the
/// dimension names is not a resource the service supports scaling, which is what the recorded
/// refusal says. It is FIXED text: it interpolates no member of the request, because it is the
/// combination rather than any one value that is unsupported.
pub const UNSUPPORTED_TRIPLE_MESSAGE: &str =
    "Unsupported service namespace, resource type or scalable dimension";

/// `Ok` when `resource_id` has the form the resource type in `scalable_dimension` documents,
/// else [`UNSUPPORTED_TRIPLE_MESSAGE`].
pub fn check_resource_id_form(
    service_namespace: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    let resource_type = dimension_resource_type(scalable_dimension);
    let Some((_, _, template)) = RESOURCE_ID_FORMS
        .iter()
        .find(|(namespace, candidate, _)| *namespace == service_namespace && *candidate == resource_type)
    else {
        return Ok(());
    };
    if matches_resource_id_form(template, resource_id) {
        return Ok(());
    }
    Err(UNSUPPORTED_TRIPLE_MESSAGE.to_string())
}

/// The function name and qualifier a `lambda` `ResourceId` names: "The resource type is
/// `function` and the unique identifier is the function name with a function version or
/// alias name suffix that is not `$LATEST`. Example: `function:my-function:prod`" (the
/// model's own `ResourceId` documentation).
pub fn split_lambda_resource_id(resource_id: &str) -> Option<(&str, &str)> {
    let rest = resource_id.strip_prefix("function:")?;
    let (function_name, qualifier) = rest.split_once(':')?;
    (!function_name.is_empty() && !qualifier.is_empty() && !qualifier.contains(':'))
        .then_some((function_name, qualifier))
}

/// The qualifier naming a Lambda function's unpublished version, which is the one
/// qualifier no provisioned concurrency can be allocated to.
const LAMBDA_UNPUBLISHED_VERSION: &str = "$LATEST";

/// `Ok` when the resource `resource_id` names can CARRY the capacity `scalable_dimension`
/// scales, else the `ValidationException` message saying which resource it has to name.
///
/// A resource id of the documented form ([`check_resource_id_form`]) still has to name a
/// resource the capacity exists on, and for one dimension that is a rule about the id
/// itself rather than about the resource's state: `lambda:function:ProvisionedConcurrency`
/// scales a function's provisioned concurrency, which is allocated to a published version
/// or an alias and never to the unpublished version — the model's own `ResourceId`
/// documentation requires "a function version or alias name suffix that is not `$LATEST`",
/// and Lambda itself refuses a configuration on it ("Provisioned Concurrency Configs cannot
/// be applied to unpublished function versions"). So a target naming it could never be
/// enforced, and the registration is refused instead of stored.
///
/// One arm per dimension whose capacity has such a rule; a dimension with none is answered
/// on the form alone. An id of a shape this cannot read is left to
/// [`check_resource_id_form`], which is the check that owns the shape.
pub fn check_provisionable_resource(
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    match scalable_dimension {
        "lambda:function:ProvisionedConcurrency" => {
            match split_lambda_resource_id(resource_id) {
                // The refusal names the QUALIFIER and nothing else: what is unsupported is
                // that qualifier, not the function it qualifies or the dimension that would
                // have scaled it ([`unsupported_qualifier_message`]).
                Some((_, qualifier)) if qualifier == LAMBDA_UNPUBLISHED_VERSION => {
                    Err(unsupported_qualifier_message(qualifier))
                }
                // An id this cannot read names no qualifier at all, so there is no
                // unsupported qualifier to report: it is the triple that describes no one
                // resource, which is the refusal [`check_resource_id_form`] owns.
                None => Err(UNSUPPORTED_TRIPLE_MESSAGE.to_string()),
                Some(_) => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// How this service refuses a resource id whose QUALIFIER names no capacity the dimension
/// can scale.
///
/// The sentence is about the qualifier alone — it names neither the resource it qualifies nor
/// the dimension the request asked to scale — because the qualifier is the whole of what is
/// unsupported: every resource of the namespace carries the same set of them, so which
/// resource was named adds nothing.
pub fn unsupported_qualifier_message(qualifier: &str) -> String {
    format!("{qualifier} is not a supported qualifier for Auto Scaling")
}

/// Whether `value` matches one [`RESOURCE_ID_FORMS`] template.
fn matches_resource_id_form(template: &str, value: &str) -> bool {
    /// The characters that separate a resource id's segments, so a `{...}` placeholder
    /// stands for exactly one segment rather than swallowing the rest of the id.
    const SEPARATORS: [char; 2] = [':', '/'];

    let mut template = template;
    let mut value = value;
    while !template.is_empty() {
        if let Some(after_brace) = template.strip_prefix('{') {
            let Some(end) = after_brace.find('}') else {
                return false;
            };
            let whole_remainder = after_brace[..end].ends_with('*');
            template = &after_brace[end + 1..];
            if whole_remainder {
                // A `{...*}` placeholder is only ever the last thing in a template.
                return template.is_empty() && !value.is_empty();
            }
            let taken = match template.chars().next() {
                Some(delimiter) => match value.find(delimiter) {
                    Some(at) => at,
                    None => return false,
                },
                None => value.len(),
            };
            if taken == 0 || value[..taken].contains(SEPARATORS) {
                return false;
            }
            value = &value[taken..];
        } else {
            let literal_len = template.find('{').unwrap_or(template.len());
            let Some(rest) = value.strip_prefix(&template[..literal_len]) else {
                return false;
            };
            value = rest;
            template = &template[literal_len..];
        }
    }
    value.is_empty()
}

// ---------------------------------------------------------------------------
// The resource a scalable target names, resolved in the service that owns it
// ---------------------------------------------------------------------------
//
// LOCK SCOPE. The helpers below, and the alarm apply/retire helpers after them, take
// ANOTHER service's store (DynamoDB's, CloudWatch's). Every one of them is called with
// THIS service's store NOT held: a handler resolves the resource before it locks, and the
// alarm work is split into plan (under this lock) / apply (no lock) / record (under this
// lock again). So no request holds two of these stores at once, and there is no lock order
// to get wrong.

/// The provisioned capacity of the resource a scalable target names, resolved in the
/// service that OWNS that resource.
///
/// One arm per namespace whose resource store this emulator holds — the same "resolve the
/// resource" rule the `ResourceId` form rows express, in one place, so adding the next
/// namespace is adding an arm here rather than a branch at each call site:
///
/// * `Ok(Some(capacity))` — the resource resolved, and this is its capacity for the
///   property the dimension names;
/// * `Ok(None)` — this emulator holds no store for that namespace, so there is nothing to
///   resolve and the registration is answered on its own terms;
/// * `Err(message)` — the resource cannot carry this target, as a `ValidationException`
///   message naming the member and why.
///
/// The resource is resolved in the target's own Region, which its identity carries: a table
/// of that name in another Region is not the resource this target scales.
pub fn resolve_target_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &TargetKey,
) -> Result<Option<i64>, String> {
    match target.service_namespace.as_str() {
        "dynamodb" => resolve_dynamodb_capacity(
            engine,
            &target.region,
            &target.resource_id,
            &target.scalable_dimension,
        ),
        "ecs" => resolve_ecs_capacity(engine, &target.resource_id),
        _ => Ok(None),
    }
}

/// The DynamoDB table, and global secondary index, a `dynamodb` `ResourceId` names.
pub fn split_dynamodb_resource_id(resource_id: &str) -> Option<(&str, Option<&str>)> {
    let rest = resource_id.strip_prefix("table/")?;
    match rest.split_once("/index/") {
        Some((table, index)) => {
            (!table.is_empty() && !index.is_empty()).then_some((table, Some(index)))
        }
        None => (!rest.is_empty()).then_some((rest, None)),
    }
}

/// The ECS cluster and service an `ecs` `ResourceId` names: "The resource type is `service`
/// and the unique identifier is the cluster name and service name. Example:
/// `service/my-cluster/my-service`" (the model's own `ResourceId` documentation).
pub fn split_ecs_resource_id(resource_id: &str) -> Option<(&str, &str)> {
    let rest = resource_id.strip_prefix("service/")?;
    let (cluster, service) = rest.split_once('/')?;
    (!cluster.is_empty() && !service.is_empty() && !service.contains('/'))
        .then_some((cluster, service))
}

/// The `desiredCount` the ECS store holds for the service an `ecs` `ResourceId` names, or
/// the `ValidationException` message for a service that cannot carry the target.
///
/// A scalable target is a capacity dimension OF A RESOURCE, so `ecs:service:DesiredCount` —
/// "the task count of an ECS service" — needs that service to exist: one that does not has
/// no task count to scale, and neither has one that has been deleted, whose count is no
/// longer maintained. The same rule the DynamoDB resolver applies one namespace over
/// ([`resolve_dynamodb_capacity`]), against the store that owns ECS services; a process
/// where that store is not reachable resolves nothing rather than refusing.
pub fn resolve_ecs_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    resource_id: &str,
) -> Result<Option<i64>, String> {
    use ::service_com_amazonaws_ecs_amazonec2containerservicev20141113 as ecs;

    let (cluster, service_name) = split_ecs_resource_id(resource_id).ok_or_else(|| {
        format!(
            "ResourceId not valid for the ecs service namespace: {resource_id} must name an ECS \
             service as service/<cluster>/<service>"
        )
    })?;
    let Some(store) = ecs::state::service_state(engine) else {
        return Ok(None);
    };
    let store = store.lock();
    let service = store
        .services
        .get(&(cluster.to_string(), service_name.to_string()))
        .filter(|service| service.is_active())
        .ok_or_else(|| format!("ECS service doesn't exist: {resource_id}"))?;
    Ok(Some(i64::from(service.desired_count)))
}

/// The provisioned capacity the DynamoDB store holds for what a `dynamodb` scalable target
/// names, or the `ValidationException` message for a resource that cannot carry that target.
///
/// A scalable target is a capacity dimension OF A RESOURCE, so registering one has to resolve
/// that resource in the service that owns it: a table that does not exist has no capacity to
/// scale, and neither has one in `PAY_PER_REQUEST` (on-demand) billing mode, whose throughput
/// "ProvisionedThroughput cannot be specified when BillingMode is PAY_PER_REQUEST" — there is
/// no provisioned value for a policy to move. The same holds one level down for a global
/// secondary index, which must exist on the table the resource id names.
///
/// This is the one namespace whose backing service this emulator can resolve. For a namespace
/// whose owning service holds no resource store here there is nothing to resolve, so the
/// registration is accepted on its own terms; extending the check is a matter of adding the
/// resolver, not of changing the rule.
pub fn resolve_dynamodb_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<Option<i64>, String> {
    use ::service_com_amazonaws_dynamodb_dynamodb_20120810 as dynamodb;

    let malformed = || {
        format!(
            "ResourceId not valid for the dynamodb service namespace: {resource_id} must name a \
             DynamoDB table or global secondary index"
        )
    };
    let (table_name, index_name) = split_dynamodb_resource_id(resource_id).ok_or_else(malformed)?;
    let Some(store) = dynamodb::state::service_state(engine) else {
        // Not the shared in-memory engine, so there is no store to resolve against: the
        // resource is UNRESOLVABLE rather than refused (a reason about this process, not
        // about the request) and rather than resolved to a capacity of zero.
        return Ok(None);
    };
    let store = store.lock();
    let now = SystemTime::now();
    // A DynamoDB table name is unique per REGION, which is why the store is keyed by it:
    // the target resolves against the region its own request is signed for. A table in
    // another region is one this request's region does not hold, which is the same
    // refusal as a table that was never created and as one being deleted — the refusal
    // names the resource the request asked for, never the region it was looked for in.
    let table = store
        .table(region, table_name)
        .ok()
        .filter(|table| !matches!(table.status(now), dynamodb::state::TableStatus::Deleting))
        .ok_or_else(|| format!("DynamoDB table does not exist: {resource_id}"))?;
    if table.provisioned_throughput.is_none() {
        return Err(scalable_target_validation_failed(&format!(
            "{} table mode is not scalable.",
            table.billing_mode
        )));
    }
    let reads = dimension_scaling_property(scalable_dimension) == "ReadCapacityUnits";
    match index_name {
        None => Ok(Some(table.provisioned_throughput.as_ref().map_or(
            0,
            |throughput| {
                if reads {
                    throughput.read_capacity_units
                } else {
                    throughput.write_capacity_units
                }
            },
        ))),
        Some(index_name) => {
            let index = table
                .global_secondary_indexes
                .iter()
                .find(|index| index.name == index_name)
                .ok_or_else(|| format!("DynamoDB index does not exist: {resource_id}"))?;
            Ok(Some(index.provisioned_throughput.as_ref().map_or(
                0,
                |throughput| {
                    if reads {
                        throughput.read_capacity_units
                    } else {
                        throughput.write_capacity_units
                    }
                },
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// The CloudWatch alarms a target-tracking policy owns
// ---------------------------------------------------------------------------

/// Which of a target-tracking policy's alarms this is.
///
/// A target-tracking policy is driven by alarms, not by the policy document: one alarm per
/// direction on the metric being tracked, and — for a resource whose tracked metric counts
/// capacity consumed rather than a percentage — one more per direction on the resource's
/// provisioned capacity, because the utilisation thresholds are only valid for the capacity
/// they were computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmKind {
    /// Scale out: the tracked metric is above the target.
    AlarmHigh,
    /// Scale in: the tracked metric is below the target.
    AlarmLow,
    /// Scale out: the resource's provisioned capacity moved up.
    ProvisionedCapacityHigh,
    /// Scale in: the resource's provisioned capacity moved down.
    ProvisionedCapacityLow,
}

impl AlarmKind {
    /// Every kind, so a sync pass can retire the alarms a policy no longer wants.
    pub const ALL: [Self; 4] = [
        Self::AlarmHigh,
        Self::AlarmLow,
        Self::ProvisionedCapacityHigh,
        Self::ProvisionedCapacityLow,
    ];

    /// The name segment this kind contributes to its alarm's name, under the namespace
    /// whose resource the policy scales.
    ///
    /// The pair on the TRACKED metric is named for the direction it drives, the same
    /// wherever the policy lives. The pair that watches the resource's PROVISIONED
    /// capacity belongs to the owning service's own integration, so it is named as that
    /// service names it ([`capacity_alarm_rules`]).
    pub fn name_segment(self, service_namespace: &str) -> &'static str {
        match self {
            Self::AlarmHigh => "AlarmHigh",
            Self::AlarmLow => "AlarmLow",
            Self::ProvisionedCapacityHigh => capacity_alarm_rules(service_namespace).high,
            Self::ProvisionedCapacityLow => capacity_alarm_rules(service_namespace).low,
        }
    }

    /// Whether this alarm drives scale-out (rather than scale-in).
    pub fn scales_out(self) -> bool {
        matches!(self, Self::AlarmHigh | Self::ProvisionedCapacityHigh)
    }

    /// Whether this alarm watches the resource's provisioned capacity rather than the
    /// tracked metric.
    pub fn watches_capacity(self) -> bool {
        matches!(
            self,
            Self::ProvisionedCapacityHigh | Self::ProvisionedCapacityLow
        )
    }
}

/// How the service that OWNS a scaled resource mints the pair of alarms that watch that
/// resource's provisioned capacity: what each is called, and whether a scaling direction
/// that cannot act withholds its one.
#[derive(Debug, Clone, Copy)]
struct CapacityAlarmRules {
    /// The name segment of the alarm that watches the capacity moving UP.
    high: &'static str,
    /// The one that watches it moving DOWN.
    low: &'static str,
    /// Whether the alarm of a direction goes with that direction when it is disabled or
    /// suspended, rather than being minted regardless.
    follows_direction: bool,
}

/// The capacity-alarm rules of one namespace.
///
/// A catalog with one row per namespace that HAS such alarms — the two whose tracked
/// metric counts capacity units consumed, which is the only case where the utilisation
/// thresholds depend on the resource's provisioned capacity — each row recorded from that
/// service's own auto scaling integration:
///
/// * DynamoDB's are the bare `ProvisionedCapacity*`, each minted beside the tracked
///   metric's alarm for the same direction, so a direction that cannot act has neither;
/// * Amazon Keyspaces' carry that service's prefix and are minted as a pair, whatever
///   `DisableScaleIn` or a suspension says about scaling in, because they report a
///   threshold computed from a capacity the resource no longer has rather than drive a
///   direction.
///
/// Any other namespace mints no capacity alarm at all, so its row is never reached; the
/// DynamoDB spelling is the fallback the service's own naming started from.
fn capacity_alarm_rules(service_namespace: &str) -> CapacityAlarmRules {
    match service_namespace {
        "cassandra" => CapacityAlarmRules {
            high: "CassandraProvisionedCapacityHigh",
            low: "CassandraProvisionedCapacityLow",
            follows_direction: false,
        },
        _ => CapacityAlarmRules {
            high: "ProvisionedCapacityHigh",
            low: "ProvisionedCapacityLow",
            follows_direction: true,
        },
    }
}

/// One alarm a target-tracking policy wants to exist in CloudWatch.
#[derive(Debug, Clone)]
pub struct AlarmSpec {
    pub kind: AlarmKind,
    pub name: String,
    pub metric_namespace: String,
    pub metric_name: String,
    pub statistic: String,
    /// The unit the watched series is measured in, when the policy's metric specification
    /// names one. `None` leaves the alarm unit-less, which is CloudWatch's own "any unit":
    /// a predefined metric type publishes no unit, so none is invented for it.
    pub unit: Option<String>,
    pub dimensions: Vec<(String, String)>,
    pub period: i32,
    pub evaluation_periods: i32,
    pub threshold: f64,
    pub comparison_operator: String,
}

/// What CloudWatch metric a `PredefinedMetricType` tracks.
///
/// Hand-written from AWS's published "Predefined metrics for target tracking scaling
/// policies" table (Application Auto Scaling User Guide), one row per value of the model's
/// `MetricType` enum (`METRIC_TYPE_VALUES`): the namespace and metric name the alarms watch,
/// the statistic they aggregate with, and whether the metric counts units CONSUMED over the
/// alarm's period — the one case where the target utilisation is not itself the threshold.
struct PredefinedMetric {
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
    /// `true` when the metric is a count of capacity units consumed over the period, so a
    /// target expressed as a percentage of provisioned capacity becomes
    /// `target% * period * provisioned units`; `false` when the metric is already the
    /// quantity the target value is expressed in.
    consumed_units: bool,
    /// The alarm period, in seconds. 60 for every metric but the high-resolution ones,
    /// which the model documents as using "20-second CloudWatch metrics […] evaluate
    /// metrics every 20 seconds".
    period: i32,
    /// The `ScalableDimension` values this metric is published FOR.
    ///
    /// A predefined metric is a metric of ONE service's resources, so it only describes
    /// that service's dimensions: the published table above names the service (and, where
    /// it splits them, the capacity property) each metric belongs to, and the model itself
    /// says of the one metric that spans two services that "ALBRequestCountPerTarget -
    /// This metric type applies only to Spot Fleets and ECS services". Where the guide
    /// publishes a metric for a service without splitting it by capacity property, every
    /// dimension of that service is listed rather than a finer split being invented.
    dimensions: &'static [&'static str],
    /// The window a `TargetValue` must lie in, where the owning service publishes one.
    ///
    /// `None` where it publishes none: the target is then expressed in whatever the metric
    /// is measured in (a request count per target, a connection count) and nothing bounds
    /// it. `Some` for the provisioned-throughput utilisation metrics, whose target is a
    /// percentage of provisioned capacity inside a published window.
    target_range: Option<(f64, f64)>,
}

/// DynamoDB's provisioned READ capacity: a table's, and a global secondary index's.
const DYNAMODB_READ: &[&str] = &[
    "dynamodb:table:ReadCapacityUnits",
    "dynamodb:index:ReadCapacityUnits",
];
/// DynamoDB's provisioned WRITE capacity.
const DYNAMODB_WRITE: &[&str] = &[
    "dynamodb:table:WriteCapacityUnits",
    "dynamodb:index:WriteCapacityUnits",
];
/// Amazon Keyspaces' provisioned read capacity.
const CASSANDRA_READ: &[&str] = &["cassandra:table:ReadCapacityUnits"];
/// Amazon Keyspaces' provisioned write capacity.
const CASSANDRA_WRITE: &[&str] = &["cassandra:table:WriteCapacityUnits"];
/// An ECS service's task count.
const ECS_SERVICE: &[&str] = &["ecs:service:DesiredCount"];
/// A Spot Fleet request's target capacity.
const SPOT_FLEET: &[&str] = &["ec2:spot-fleet-request:TargetCapacity"];
/// "ALBRequestCountPerTarget - This metric type applies only to Spot Fleets and ECS
/// services" (`PredefinedMetricSpecification$PredefinedMetricType`).
const ALB_REQUEST_COUNT: &[&str] = &[
    "ecs:service:DesiredCount",
    "ec2:spot-fleet-request:TargetCapacity",
];
/// An Aurora read-replica count.
const RDS_CLUSTER: &[&str] = &["rds:cluster:ReadReplicaCount"];
/// A Neptune read-replica count.
const NEPTUNE_CLUSTER: &[&str] = &["neptune:cluster:ReadReplicaCount"];
/// An AppStream 2.0 fleet's capacity.
const APPSTREAM_FLEET: &[&str] = &["appstream:fleet:DesiredCapacity"];
/// Comprehend's two endpoint kinds, which publish the same inference metric.
const COMPREHEND_ENDPOINTS: &[&str] = &[
    "comprehend:document-classifier-endpoint:DesiredInferenceUnits",
    "comprehend:entity-recognizer-endpoint:DesiredInferenceUnits",
];
/// A Lambda function's provisioned concurrency.
const LAMBDA_CONCURRENCY: &[&str] = &["lambda:function:ProvisionedConcurrency"];
/// An MSK cluster's broker storage.
const KAFKA_STORAGE: &[&str] = &["kafka:broker-storage:VolumeSize"];
/// Every ElastiCache dimension: the guide publishes the engine-CPU and memory metrics for
/// the service without splitting them by capacity property.
const ELASTICACHE: &[&str] = &[
    "elasticache:cache-cluster:Nodes",
    "elasticache:replication-group:NodeGroups",
    "elasticache:replication-group:Replicas",
];
/// A SageMaker endpoint variant's instance count.
const SAGEMAKER_VARIANT_INSTANCES: &[&str] = &["sagemaker:variant:DesiredInstanceCount"];
/// A SageMaker endpoint variant's provisioned concurrency.
const SAGEMAKER_VARIANT_CONCURRENCY: &[&str] = &["sagemaker:variant:DesiredProvisionedConcurrency"];
/// A SageMaker inference component's copy count.
const SAGEMAKER_INFERENCE_COMPONENT: &[&str] = &["sagemaker:inference-component:DesiredCopyCount"];
/// A WorkSpaces pool's user sessions.
const WORKSPACES_POOL: &[&str] = &["workspaces:workspacespool:DesiredUserSessions"];

/// The window a provisioned-throughput utilisation target must lie in.
///
/// Both services whose scalable dimension IS a provisioned read/write throughput publish the
/// same one, and recorded behaviour of each is that both bounds are inclusive: a target
/// utilisation of ten percent of the provisioned capacity is accepted, and so is ninety.
const PROVISIONED_UTILIZATION_RANGE: (f64, f64) = (10.0, 90.0);

/// The window a Lambda provisioned-concurrency utilisation target must lie in.
///
/// `ProvisionedConcurrencyUtilization` is "the fraction of the allocated provisioned
/// concurrency that is in use", so this target is a FRACTION rather than a percentage, and
/// the AWS Lambda Developer Guide's auto scaling walkthrough states the window it may lie in
/// as 0.1 to 0.9 (its own example target is 0.7). A percentage-style value is outside it, so
/// it is refused rather than driving provisioned concurrency at a utilisation Lambda can
/// never reach.
const LAMBDA_CONCURRENCY_UTILIZATION_RANGE: (f64, f64) = (0.1, 0.9);

const PREDEFINED_METRICS: &[PredefinedMetric] = &[
    metric("DynamoDBReadCapacityUtilization", "AWS/DynamoDB", "ConsumedReadCapacityUnits", "Sum", true, 60, DYNAMODB_READ, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("DynamoDBWriteCapacityUtilization", "AWS/DynamoDB", "ConsumedWriteCapacityUnits", "Sum", true, 60, DYNAMODB_WRITE, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("CassandraReadCapacityUtilization", "AWS/Cassandra", "ConsumedReadCapacityUnits", "Sum", true, 60, CASSANDRA_READ, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("CassandraWriteCapacityUtilization", "AWS/Cassandra", "ConsumedWriteCapacityUnits", "Sum", true, 60, CASSANDRA_WRITE, Some(PROVISIONED_UTILIZATION_RANGE)),
    metric("ALBRequestCountPerTarget", "AWS/ApplicationELB", "RequestCountPerTarget", "Sum", false, 60, ALB_REQUEST_COUNT, None),
    metric("ECSServiceAverageCPUUtilization", "AWS/ECS", "CPUUtilization", "Average", false, 60, ECS_SERVICE, None),
    metric("ECSServiceAverageMemoryUtilization", "AWS/ECS", "MemoryUtilization", "Average", false, 60, ECS_SERVICE, None),
    metric("ECSServiceAverageCPUUtilizationHighResolution", "AWS/ECS", "CPUUtilization", "Average", false, 20, ECS_SERVICE, None),
    metric("ECSServiceAverageMemoryUtilizationHighResolution", "AWS/ECS", "MemoryUtilization", "Average", false, 20, ECS_SERVICE, None),
    metric("EC2SpotFleetRequestAverageCPUUtilization", "AWS/EC2Spot", "CPUUtilization", "Average", false, 60, SPOT_FLEET, None),
    metric("EC2SpotFleetRequestAverageNetworkIn", "AWS/EC2Spot", "NetworkIn", "Average", false, 60, SPOT_FLEET, None),
    metric("EC2SpotFleetRequestAverageNetworkOut", "AWS/EC2Spot", "NetworkOut", "Average", false, 60, SPOT_FLEET, None),
    metric("RDSReaderAverageCPUUtilization", "AWS/RDS", "CPUUtilization", "Average", false, 60, RDS_CLUSTER, None),
    metric("RDSReaderAverageDatabaseConnections", "AWS/RDS", "DatabaseConnections", "Average", false, 60, RDS_CLUSTER, None),
    metric("NeptuneReaderAverageCPUUtilization", "AWS/Neptune", "CPUUtilization", "Average", false, 60, NEPTUNE_CLUSTER, None),
    metric("AppStreamAverageCapacityUtilization", "AWS/AppStream", "CapacityUtilization", "Average", false, 60, APPSTREAM_FLEET, None),
    metric("ComprehendInferenceUtilization", "AWS/Comprehend", "InferenceUtilization", "Average", false, 60, COMPREHEND_ENDPOINTS, None),
    metric("LambdaProvisionedConcurrencyUtilization", "AWS/Lambda", "ProvisionedConcurrencyUtilization", "Average", false, 60, LAMBDA_CONCURRENCY, Some(LAMBDA_CONCURRENCY_UTILIZATION_RANGE)),
    metric("KafkaBrokerStorageUtilization", "AWS/Kafka", "KafkaDataLogsDiskUsed", "Average", false, 60, KAFKA_STORAGE, None),
    metric("ElastiCacheEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCachePrimaryEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheReplicaEngineCPUUtilization", "AWS/ElastiCache", "EngineCPUUtilization", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseMemoryUsagePercentage", "AWS/ElastiCache", "DatabaseMemoryUsagePercentage", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseMemoryUsageCountedForEvictPercentage", "AWS/ElastiCache", "DatabaseMemoryUsageCountedForEvictPercentage", "Average", false, 60, ELASTICACHE, None),
    metric("ElastiCacheDatabaseCapacityUsageCountedForEvictPercentage", "AWS/ElastiCache", "DatabaseCapacityUsageCountedForEvictPercentage", "Average", false, 60, ELASTICACHE, None),
    metric("SageMakerVariantInvocationsPerInstance", "AWS/SageMaker", "InvocationsPerInstance", "Sum", false, 60, SAGEMAKER_VARIANT_INSTANCES, None),
    metric("SageMakerVariantProvisionedConcurrencyUtilization", "AWS/SageMaker", "ProvisionedConcurrencyUtilization", "Average", false, 60, SAGEMAKER_VARIANT_CONCURRENCY, None),
    metric("SageMakerVariantConcurrentRequestsPerModelHighResolution", "AWS/SageMaker", "ConcurrentRequestsPerModel", "Average", false, 20, SAGEMAKER_VARIANT_INSTANCES, None),
    metric("SageMakerInferenceComponentInvocationsPerCopy", "AWS/SageMaker", "InvocationsPerCopy", "Sum", false, 60, SAGEMAKER_INFERENCE_COMPONENT, None),
    metric("SageMakerInferenceComponentConcurrentRequestsPerCopyHighResolution", "AWS/SageMaker", "ConcurrentRequestsPerCopy", "Average", false, 20, SAGEMAKER_INFERENCE_COMPONENT, None),
    metric("WorkSpacesAverageUserSessionsCapacityUtilization", "AWS/WorkSpaces", "UserSessionsCapacityUtilization", "Average", false, 60, WORKSPACES_POOL, None),
];

/// One [`PREDEFINED_METRICS`] row, so the table above reads as a table.
#[allow(clippy::too_many_arguments)]
const fn metric(
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
    consumed_units: bool,
    period: i32,
    dimensions: &'static [&'static str],
    target_range: Option<(f64, f64)>,
) -> PredefinedMetric {
    PredefinedMetric {
        metric_type,
        namespace,
        metric_name,
        statistic,
        consumed_units,
        period,
        dimensions,
        target_range,
    }
}

/// The metric a `PredefinedMetricType` tracks, when it is one this emulator knows.
fn predefined_metric(metric_type: &str) -> Option<&'static PredefinedMetric> {
    PREDEFINED_METRICS
        .iter()
        .find(|metric| metric.metric_type == metric_type)
}

/// The `ScalableDimension` values `metric_type` is published for, when it is a metric this
/// service scales on. See [`PredefinedMetric::dimensions`].
pub fn predefined_metric_dimensions(metric_type: &str) -> Option<&'static [&'static str]> {
    predefined_metric(metric_type).map(|metric| metric.dimensions)
}

/// The window `metric_type`'s target value must lie in, where its owning service publishes
/// one. See [`PredefinedMetric::target_range`].
pub fn predefined_metric_target_range(metric_type: &str) -> Option<(f64, f64)> {
    predefined_metric(metric_type).and_then(|metric| metric.target_range)
}

/// How this service refuses a target value outside the window its predefined metric
/// publishes: the metric type, both bounds of that window and the value the request offered,
/// every one of them read off the request and the catalog.
///
/// The bounds and the offered value are written as the DOUBLES they are — the member is a
/// double, so a whole number is stated with its fractional part — which is how the service
/// words this refusal.
pub fn target_value_outside_range_message(
    metric_type: &str,
    (low, high): (f64, f64),
    offered: f64,
) -> String {
    format!(
        "For predefined metric type {metric_type}, target value must be between '{low:?}' and \
         '{high:?}', but was '{offered:?}'."
    )
}

/// Every `PredefinedMetricType` published FOR `scalable_dimension`, in the order the
/// catalog (and the model's `MetricType` enum) declares them.
///
/// The inverse of [`PredefinedMetric::dimensions`], read off the one catalog so the set a
/// refusal enumerates is the set the alarm planner can actually mint alarms from.
pub fn supported_predefined_metrics(scalable_dimension: &str) -> Vec<&'static str> {
    PREDEFINED_METRICS
        .iter()
        .filter(|metric| metric.dimensions.contains(&scalable_dimension))
        .map(|metric| metric.metric_type)
        .collect()
}

/// How this service states a predefined metric type that `scalable_dimension` is not
/// scaled on.
///
/// The sentence ENUMERATES what the dimension does support, which is what makes the
/// refusal recoverable: a caller reads the metric types it may choose from out of the
/// message. Both halves are built from the request and the catalog — never from one
/// recorded example — and a dimension the catalog publishes no metric for is told that
/// instead of being handed an empty list.
pub fn unsupported_predefined_metric_message(scalable_dimension: &str) -> String {
    let supported = supported_predefined_metrics(scalable_dimension);
    if supported.is_empty() {
        return format!(
            "Scalable dimension {scalable_dimension} does not support \
             PredefinedMetricSpecification."
        );
    }
    format!(
        "Scalable dimension {scalable_dimension} only supports the following predefined metric \
         types: {}",
        supported.join(", ")
    )
}

/// How this service states a customized metric on a dimension that is not scaled from one.
/// See [`customized_metric_supported`].
pub fn unsupported_customized_metric_message(scalable_dimension: &str) -> String {
    format!(
        "Scalable dimension {scalable_dimension} does not support CustomizedMetricSpecification."
    )
}

/// Whether a target-tracking policy on `scalable_dimension` may track a CUSTOMIZED metric.
///
/// A customized metric is a series of the caller's own choosing, which presupposes that the
/// resource's capacity can be driven from an arbitrary signal. The two services whose
/// scalable dimension IS a provisioned read/write throughput — DynamoDB and Amazon
/// Keyspaces — scale it from their own consumed-capacity utilisation and from nothing else,
/// and refuse a customized specification (recorded AWS behaviour: a customized metric on a
/// DynamoDB dimension is answered 400 `ValidationException`).
pub fn customized_metric_supported(scalable_dimension: &str) -> bool {
    !matches!(
        scalable_dimension.split(':').next(),
        Some("dynamodb" | "cassandra")
    )
}

// ---------------------------------------------------------------------------
// Predictive scaling: the series a forecast is computed from
// ---------------------------------------------------------------------------

/// One CloudWatch time series, as a forecast reads it: the series' identity plus the
/// statistic its observations are aggregated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricSeries {
    pub namespace: String,
    pub metric_name: String,
    pub statistic: String,
    pub dimensions: Vec<(String, String)>,
}

/// What CloudWatch series a `PredictiveScalingMetricType` names.
///
/// Hand-written from the model's own list of "the predefined metrics […] available for
/// predictive scaling"
/// (`smithy_query(query="shape",shape="PredictiveScalingPredefinedMetricPairSpecification")`),
/// one row per value: the namespace and metric name, and the statistic the LOAD series
/// aggregates the metric with — a "Total" or request-count metric is the load SUMMED over
/// the interval, while an "Average" metric is the per-capacity-unit utilisation and is
/// averaged.
struct PredictiveMetric {
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
}

/// The CloudWatch namespace of the load-balancer metrics among them, whose series is
/// dimensioned by the target group a `ResourceLabel` names rather than by the scalable
/// target's own resource.
const APPLICATION_ELB_NAMESPACE: &str = "AWS/ApplicationELB";

const PREDICTIVE_METRICS: &[PredictiveMetric] = &[
    predictive_metric("ECSServiceAverageCPUUtilization", "AWS/ECS", "CPUUtilization", "Average"),
    predictive_metric("ECSServiceAverageMemoryUtilization", "AWS/ECS", "MemoryUtilization", "Average"),
    predictive_metric("ECSServiceCPUUtilization", "AWS/ECS", "CPUUtilization", "Sum"),
    predictive_metric("ECSServiceMemoryUtilization", "AWS/ECS", "MemoryUtilization", "Sum"),
    predictive_metric("ECSServiceTotalCPUUtilization", "AWS/ECS", "CPUUtilization", "Sum"),
    predictive_metric("ECSServiceTotalMemoryUtilization", "AWS/ECS", "MemoryUtilization", "Sum"),
    predictive_metric("ALBRequestCount", APPLICATION_ELB_NAMESPACE, "RequestCount", "Sum"),
    predictive_metric("ALBRequestCountPerTarget", APPLICATION_ELB_NAMESPACE, "RequestCountPerTarget", "Sum"),
    predictive_metric("TotalALBRequestCount", APPLICATION_ELB_NAMESPACE, "RequestCount", "Sum"),
];

/// One [`PREDICTIVE_METRICS`] row, so the table above reads as a table.
const fn predictive_metric(
    metric_type: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
) -> PredictiveMetric {
    PredictiveMetric {
        metric_type,
        namespace,
        metric_name,
        statistic,
    }
}

/// The series a predefined predictive-scaling metric type names FOR this scalable target,
/// or `None` when the type is not one of [`PREDICTIVE_SCALING_METRIC_TYPE_VALUES`].
///
/// A service metric is dimensioned by the scalable target's own resource
/// ([`target_metric_dimensions`]); a load-balancer metric is dimensioned by the load
/// balancer, and the per-target one additionally by the target group, which is what the
/// metric specification's `ResourceLabel` "uniquely identifies".
pub fn predictive_metric_series(
    target: &TargetKey,
    metric_type: &str,
    resource_label: Option<&str>,
) -> Option<MetricSeries> {
    let row = PREDICTIVE_METRICS
        .iter()
        .find(|metric| metric.metric_type == metric_type)?;
    let dimensions = if row.namespace == APPLICATION_ELB_NAMESPACE {
        resource_label_dimensions(resource_label, row.metric_name == "RequestCountPerTarget")
    } else {
        target_metric_dimensions(target)
    };
    Some(MetricSeries {
        namespace: row.namespace.to_string(),
        metric_name: row.metric_name.to_string(),
        statistic: row.statistic.to_string(),
        dimensions,
    })
}

/// The CloudWatch dimensions a `ResourceLabel` names.
///
/// The label is documented as "the resource label […] in the format
/// app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>",
/// which is exactly the `LoadBalancer` dimension followed by the `TargetGroup` dimension of
/// Application Load Balancer metrics. A label that is not in that form names no series, so
/// it contributes no dimension rather than a guessed one.
fn resource_label_dimensions(resource_label: Option<&str>, per_target: bool) -> Vec<(String, String)> {
    let Some((load_balancer, target_group)) =
        resource_label.and_then(|label| label.split_once("/targetgroup/"))
    else {
        return Vec::new();
    };
    let mut dimensions = vec![("LoadBalancer".to_string(), load_balancer.to_string())];
    if per_target {
        dimensions.push((
            "TargetGroup".to_string(),
            format!("targetgroup/{target_group}"),
        ));
    }
    dimensions
}

/// The CloudWatch dimensions that identify the resource `target` names, by its key rather
/// than by its three members. See [`metric_dimensions`].
fn target_metric_dimensions(target: &TargetKey) -> Vec<(String, String)> {
    metric_dimensions(
        &target.service_namespace,
        &target.resource_id,
        &target.scalable_dimension,
    )
}

/// The METRIC a target-tracking configuration tracks, as a comparable key.
///
/// Two configurations have the same signature when they track the same series: the same
/// predefined metric type (with the same resource label, which is part of WHICH series a
/// per-target metric is), or the same customized namespace, metric name, statistic, unit
/// and dimension set. The target value is deliberately not part of it — two policies
/// steering one metric towards two values are exactly the conflict the uniqueness rule
/// refuses.
pub fn metric_signature(config: &TargetTrackingScalingPolicyConfiguration) -> String {
    if let Some(predefined) = &config.predefined_metric_specification {
        return format!(
            "predefined\u{1f}{}\u{1f}{}",
            predefined.predefined_metric_type,
            predefined.resource_label.as_deref().unwrap_or_default()
        );
    }
    match &config.customized_metric_specification {
        Some(customized) => {
            let mut dimensions: Vec<String> = customized
                .dimensions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|dimension| format!("{}={}", dimension.name, dimension.value))
                .collect();
            dimensions.sort();
            format!(
                "customized\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                customized.namespace.as_deref().unwrap_or_default(),
                customized.metric_name.as_deref().unwrap_or_default(),
                customized.statistic.as_deref().unwrap_or_default(),
                customized.unit.as_deref().unwrap_or_default(),
                dimensions.join(",")
            )
        }
        // A configuration with neither specification is refused before this is reached; it
        // tracks no series at all.
        None => "none".to_string(),
    }
}

/// How the pair of alarms on the metric a policy TRACKS is evaluated: how many periods a
/// breach has to last in each direction, and how far below the policy's target value the
/// scale-in alarm's own target sits.
#[derive(Debug, Clone, Copy)]
struct TrackedAlarmRules {
    /// Periods the metric has to stay past the target before the scale-out alarm fires.
    scale_out_evaluation_periods: i32,
    /// Periods below the scale-in threshold before the scale-in alarm fires.
    scale_in_evaluation_periods: i32,
    /// The buffer that separates the scale-in alarm's target from the policy's own.
    scale_in_buffer: ScaleInBuffer,
}

/// How far below the policy's `TargetValue` the target the SCALE-IN alarm is computed from
/// sits.
///
/// Two forms, not one number: a buffer of percentage POINTS is an offset on the target
/// utilisation, and so keeps its width whatever the target is, while a FRACTION of the
/// target narrows as the target falls. Which form an integration uses is its own, so the
/// two are kept apart rather than one being expressed as the other.
#[derive(Debug, Clone, Copy)]
enum ScaleInBuffer {
    /// This many percentage points under the target utilisation: the scale-in alarm is
    /// computed from `TargetValue - points`.
    PercentagePoints(f64),
    /// This fraction of the target value.
    FractionOfTarget(f64),
}

/// How far below the policy's target value the scale-in alarm of a consumed-units metric is
/// buffered, in PERCENTAGE POINTS of the resource's provisioned capacity.
///
/// Recorded behaviour of the auto scaling the two provisioned-throughput services mint: a
/// policy tracking 40 percent of the provisioned capacity scales in below 20 percent of it,
/// and one tracking 70 percent below 50 — the same twenty-point offset at either target,
/// rather than a proportion of it. A target at the bottom of the published window
/// ([`PROVISIONED_UTILIZATION_RANGE`]) therefore buffers to a negative threshold, which no
/// sum of consumed units falls below: such a policy adds capacity and never removes it.
const CONSUMED_UNITS_SCALE_IN_POINTS: f64 = 20.0;

/// The rules for the pair on a tracked metric, which follow WHAT that metric measures.
///
/// Scaling in is slower than scaling out in both rows — the asymmetry target tracking is
/// built on, "scaling in is more gradual than scaling out" — and each row is the behaviour
/// of the integration that mints such a pair:
///
/// * a metric that already IS the quantity the target names (a utilisation percentage, a
///   count per target): recorded behaviour — the scale-out alarm fires after three periods
///   above the target value and the scale-in alarm sits a tenth below it, over fifteen;
/// * a metric that COUNTS the capacity units consumed over the period, which is how the two
///   provisioned-throughput services publish their utilisation: their auto scaling adds
///   capacity after two periods above the target and removes it after fifteen below a target
///   [`CONSUMED_UNITS_SCALE_IN_POINTS`] percentage points under it (the Amazon DynamoDB
///   Developer Guide's "Managing throughput capacity automatically with DynamoDB auto
///   scaling", which scales out on a two-minute breach and in on a fifteen-minute one).
fn tracked_alarm_rules(consumed_units: bool) -> TrackedAlarmRules {
    if consumed_units {
        TrackedAlarmRules {
            scale_out_evaluation_periods: 2,
            scale_in_evaluation_periods: 15,
            scale_in_buffer: ScaleInBuffer::PercentagePoints(CONSUMED_UNITS_SCALE_IN_POINTS),
        }
    } else {
        TrackedAlarmRules {
            scale_out_evaluation_periods: 3,
            scale_in_evaluation_periods: 15,
            scale_in_buffer: ScaleInBuffer::FractionOfTarget(0.9),
        }
    }
}

/// The period of the two alarms that watch the resource's provisioned capacity, and how
/// many of them must breach. Also this emulator's choice; the capacity of a resource moves
/// far less often than its utilisation, so these alarms are coarser.
const CAPACITY_ALARM_PERIOD: i32 = 300;
const CAPACITY_ALARM_EVALUATION_PERIODS: i32 = 3;

/// The prefix every alarm a target-tracking policy mints carries, which is what lets a
/// client find them with `DescribeAlarms`' `AlarmNamePrefix`.
const ALARM_NAME_PREFIX: &str = "TargetTracking";

/// CloudWatch's `AlarmName` length bound ("len 1..255" on `PutMetricAlarm$AlarmName`), which
/// a resource id of up to 1600 characters would otherwise blow through.
const MAX_ALARM_NAME_LEN: usize = 255;

/// The name of one of a policy's alarms, in the grammar the service mints one by:
/// `TargetTracking-<resource id>-<kind>-<id>`, where the trailing id is an 8-4-4-4-12
/// identifier ([`uuid_shaped_id`]).
///
/// The GRAMMAR is the requirement, not the id: a client identifies which of a policy's
/// alarms it holds by reading the kind out of the name, so every segment has to be
/// separable — an id that carried no group separators would run into the kind before it.
pub fn policy_alarm_name(key: &PolicyKey, kind: AlarmKind) -> String {
    let kind = kind.name_segment(&key.target.service_namespace);
    let id = uuid_shaped_id(
        &[
            "alarm",
            &key.target.region,
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
            kind,
        ]
        .join("\u{1f}"),
    );
    let suffix = format!("-{kind}-{id}");
    let room = MAX_ALARM_NAME_LEN
        .saturating_sub(ALARM_NAME_PREFIX.len() + 1 + suffix.len())
        .min(key.target.resource_id.len());
    let resource = &key.target.resource_id[..floor_char_boundary(&key.target.resource_id, room)];
    // A resource id long enough to leave no room at all is left out rather than rendered
    // as an empty segment between two separators.
    if resource.is_empty() {
        return format!("{ALARM_NAME_PREFIX}{suffix}");
    }
    format!("{ALARM_NAME_PREFIX}-{resource}{suffix}")
}

/// What separates a scaling policy's ARN from the id of the alarm set minted for it, inside
/// one of that policy's alarm actions. See [`policy_alarm_action`].
const ALARM_ACTION_CREATED_BY: &str = ":createdBy/";

/// The ACTION one of a policy's alarms executes when it moves into `ALARM`: the policy's own
/// ARN, followed by the id of the alarm set that policy minted —
/// `<policy ARN>:createdBy/<id>`.
///
/// The action is the wiring that makes a target-tracking policy alarm-driven, so it names
/// the policy; the suffix says the alarm was minted BY that policy, which is what lets a
/// client tell a policy's own alarms from one it wired up itself by matching the
/// `<policy ARN>:createdBy/` prefix. One id per POLICY — its alarms were minted together —
/// derived from the policy's identity so it survives a re-put rather than changing per call.
pub fn policy_alarm_action(policy_arn: &str, key: &PolicyKey) -> String {
    let id = uuid_shaped_id(
        &[
            "createdBy",
            &key.target.region,
            &key.target.service_namespace,
            &key.target.resource_id,
            &key.target.scalable_dimension,
            &key.policy_name,
        ]
        .join("\u{1f}"),
    );
    format!("{policy_arn}{ALARM_ACTION_CREATED_BY}{id}")
}

/// The scaling policy an alarm action names: an action is either a policy's bare ARN — which
/// is how a client wires its OWN alarm to a step scaling policy — or that ARN with the
/// minting suffix [`policy_alarm_action`] adds, and both name the same policy.
pub fn policy_arn_in_action(action: &str) -> &str {
    match action.split_once(ALARM_ACTION_CREATED_BY) {
        Some((policy_arn, _)) => policy_arn,
        None => action,
    }
}

/// The largest index at or below `at` that `text` can be split on.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The CloudWatch dimensions that identify the resource a scalable target names.
///
/// One row per resource type this emulator can name, each from that service's own
/// CloudWatch metric dimensions: a DynamoDB table metric is dimensioned by `TableName` (and a
/// global secondary index additionally by `GlobalSecondaryIndexName`), an ECS service metric by
/// `ClusterName` and `ServiceName`, and so on. A resource type with no row contributes no
/// dimensions rather than a guessed one.
fn metric_dimensions(
    service_namespace: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Vec<(String, String)> {
    let named = |name: &str, value: &str| vec![(name.to_string(), value.to_string())];
    let segments: Vec<&str> = resource_id.split(['/', ':']).collect();
    match (service_namespace, dimension_resource_type(scalable_dimension)) {
        ("dynamodb", "table") => match split_dynamodb_resource_id(resource_id) {
            Some((table, _)) => named("TableName", table),
            None => Vec::new(),
        },
        ("dynamodb", "index") => match split_dynamodb_resource_id(resource_id) {
            Some((table, Some(index))) => vec![
                ("TableName".to_string(), table.to_string()),
                ("GlobalSecondaryIndexName".to_string(), index.to_string()),
            ],
            _ => Vec::new(),
        },
        ("ecs", "service") => match split_ecs_resource_id(resource_id) {
            Some((cluster, service)) => vec![
                ("ClusterName".to_string(), cluster.to_string()),
                ("ServiceName".to_string(), service.to_string()),
            ],
            None => Vec::new(),
        },
        ("cassandra", "table") => match segments.as_slice() {
            ["keyspace", keyspace, "table", table] => vec![
                ("Keyspace".to_string(), (*keyspace).to_string()),
                ("TableName".to_string(), (*table).to_string()),
            ],
            _ => Vec::new(),
        },
        ("ec2", "spot-fleet-request") => segments
            .get(1)
            .map(|id| named("FleetRequestId", id))
            .unwrap_or_default(),
        ("elasticmapreduce", "instancegroup") => match segments.as_slice() {
            ["instancegroup", cluster, group] => vec![
                ("JobFlowId".to_string(), (*cluster).to_string()),
                ("InstanceGroupId".to_string(), (*group).to_string()),
            ],
            _ => Vec::new(),
        },
        ("appstream", "fleet") => segments
            .get(1)
            .map(|name| named("Fleet", name))
            .unwrap_or_default(),
        ("lambda", "function") => match segments.as_slice() {
            ["function", name, qualifier] => vec![
                ("FunctionName".to_string(), (*name).to_string()),
                (
                    "Resource".to_string(),
                    format!("{name}:{qualifier}"),
                ),
            ],
            _ => Vec::new(),
        },
        ("rds" | "neptune", "cluster") => segments
            .get(1)
            .map(|name| named("DBClusterIdentifier", name))
            .unwrap_or_default(),
        ("sagemaker", "variant") => match segments.as_slice() {
            ["endpoint", endpoint, "variant", variant] => vec![
                ("EndpointName".to_string(), (*endpoint).to_string()),
                ("VariantName".to_string(), (*variant).to_string()),
            ],
            _ => Vec::new(),
        },
        ("sagemaker", "inference-component") => segments
            .get(1)
            .map(|name| named("InferenceComponentName", name))
            .unwrap_or_default(),
        ("elasticache", "cache-cluster") => segments
            .get(1)
            .map(|name| named("CacheClusterId", name))
            .unwrap_or_default(),
        ("elasticache", "replication-group") => segments
            .get(1)
            .map(|name| named("ReplicationGroupId", name))
            .unwrap_or_default(),
        ("workspaces", "workspacespool") => segments
            .get(1)
            .map(|id| named("WorkspacesPoolId", id))
            .unwrap_or_default(),
        // An MSK target is named by its cluster ARN (`…:cluster/<name>/<uuid>`), and the
        // service's own metrics are dimensioned by that cluster's NAME.
        ("kafka", "broker-storage") => resource_id
            .split_once(":cluster/")
            .and_then(|(_, rest)| rest.split('/').next())
            .filter(|name| !name.is_empty())
            .map(|name| named("Cluster Name", name))
            .unwrap_or_default(),
        // A Comprehend endpoint is named by its own ARN, which is also the dimension its
        // metrics carry.
        ("comprehend", "document-classifier-endpoint" | "entity-recognizer-endpoint") => {
            named("EndpointArn", resource_id)
        }
        _ => Vec::new(),
    }
}

/// The SERIES a target-tracking policy's own pair of alarms watches, and the shape of the
/// quantity it measures.
///
/// The one place a policy's metric specification is turned into a series, so the alarms that
/// watch it and the capacity its control loop computes
/// ([`target_tracking_capacity`]) can never read the specification two ways.
#[derive(Debug, Clone)]
struct TrackedSeries {
    metric_namespace: String,
    metric_name: String,
    statistic: String,
    /// The unit the series is measured in, where the specification names one.
    unit: Option<String>,
    dimensions: Vec<(String, String)>,
    /// The alarm period, in seconds, which is also the window a consumed-units metric is
    /// summed over.
    period: i32,
    /// Whether the metric COUNTS the capacity units consumed over that period rather than
    /// already being the quantity the target value is expressed in.
    consumed_units: bool,
}

/// The series `config` tracks, or `None` for a configuration that names no single one.
///
/// A customised specification that carries only a metric math query names no single series,
/// so there is no static-threshold alarm to mint and no measurement to compute a capacity
/// from.
///
/// A customised specification describes the series COMPLETELY — namespace, metric name,
/// dimensions, statistic and unit — so every one of those five is carried onto the alarms; an
/// alarm that aggregated the series differently from the specification would measure a
/// different signal from the one the policy declared.
fn tracked_series(
    key: &PolicyKey,
    config: &TargetTrackingScalingPolicyConfiguration,
) -> Option<TrackedSeries> {
    let predefined = config
        .predefined_metric_specification
        .as_ref()
        .and_then(|specification| predefined_metric(&specification.predefined_metric_type));
    if let Some(metric) = predefined {
        return Some(TrackedSeries {
            metric_namespace: metric.namespace.to_string(),
            metric_name: metric.metric_name.to_string(),
            statistic: metric.statistic.to_string(),
            // The published predefined-metric table names no unit for any row.
            unit: None,
            dimensions: metric_dimensions(
                &key.target.service_namespace,
                &key.target.resource_id,
                &key.target.scalable_dimension,
            ),
            period: metric.period,
            consumed_units: metric.consumed_units,
        });
    }
    let specification = config.customized_metric_specification.as_ref()?;
    Some(TrackedSeries {
        metric_namespace: specification.namespace.clone()?,
        metric_name: specification.metric_name.clone()?,
        statistic: specification
            .statistic
            .clone()
            .unwrap_or_else(|| "Average".to_string()),
        unit: specification.unit.clone(),
        dimensions: specification
            .dimensions
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|dimension| (dimension.name, dimension.value))
            .collect(),
        // A customised series is published on the resolution the caller publishes it at,
        // which the specification does not state, so the alarms watch it on CloudWatch's
        // own standard period.
        period: 60,
        consumed_units: false,
    })
}

/// The alarms a target-tracking policy wants to exist right now.
///
/// Empty for any other policy type: only target tracking is alarm-driven.
///
/// `provisioned_capacity` is the resource's current provisioned capacity, which the
/// utilisation thresholds of a consumed-units metric are recomputed from on every pass — the
/// threshold is a function of the target, the alarm period and that capacity, never a copy of
/// the target value.
///
/// Which alarms are wanted is what `DisableScaleIn` decides: scale-in is then "disabled and
/// the target tracking scaling policy won't remove capacity", so that direction has no alarm
/// to trigger it. A SUSPENDED direction keeps its alarm — a suspension withholds the scaling
/// activity a breach would perform, not the watching of the metric, so a suspended target
/// reports the same alarms as an unsuspended one and the breach is delivered once the
/// suspension is lifted ([`policy_execution`]).
pub fn target_tracking_alarms(
    key: &PolicyKey,
    record: &ScalingPolicyRecord,
    provisioned_capacity: i64,
) -> Vec<AlarmSpec> {
    if record.policy_type != policy_execution::TARGET_TRACKING_POLICY_TYPE {
        return Vec::new();
    }
    let Some(config) = record.target_tracking_config.as_ref() else {
        return Vec::new();
    };
    let Some(series) = tracked_series(key, config) else {
        return Vec::new();
    };
    let TrackedSeries {
        metric_namespace,
        metric_name,
        statistic,
        unit,
        dimensions,
        period,
        consumed_units,
    } = series;

    let scale_in_disabled = config.disable_scale_in.unwrap_or(false);

    let capacity_rules = capacity_alarm_rules(&key.target.service_namespace);
    let tracked_rules = tracked_alarm_rules(consumed_units);
    // The threshold of the tracked metric's alarm for one direction: the target utilisation
    // that direction watches ([`direction_target_value`]), expressed in what the metric
    // MEASURES — for a metric that counts units consumed over the period that is
    // `target% * period * provisioned units`, and for a metric that already is the quantity
    // the target names the target itself.
    let utilisation_threshold = |scales_out: bool| {
        let target = direction_target_value(config.target_value, tracked_rules, scales_out);
        if consumed_units {
            target / 100.0 * f64::from(period) * provisioned_capacity as f64
        } else {
            target
        }
    };

    let mut alarms = Vec::new();
    for kind in AlarmKind::ALL {
        // A direction that cannot act has no alarm to trigger it. Whether that takes the
        // capacity alarm of the same direction with it is the owning service's own
        // mechanics ([`capacity_alarm_rules`]), since that alarm reports a stale
        // threshold rather than drive a scaling direction.
        let follows_direction = !kind.watches_capacity() || capacity_rules.follows_direction;
        if follows_direction && !kind.scales_out() && scale_in_disabled {
            continue;
        }
        // The provisioned-capacity pair exists only where the thresholds depend on the
        // capacity: it is what tells the policy its utilisation thresholds are stale.
        if kind.watches_capacity() && !consumed_units {
            continue;
        }
        let (metric_namespace, metric_name, statistic, unit, period, threshold, comparison_operator) =
            if kind.watches_capacity() {
                (
                    metric_namespace.clone(),
                    provisioned_capacity_metric_name(&key.target.scalable_dimension),
                    "Average".to_string(),
                    // A different series from the tracked metric — the resource's own
                    // provisioned capacity — so the specification's unit does not describe
                    // it.
                    None,
                    CAPACITY_ALARM_PERIOD,
                    provisioned_capacity as f64,
                    if kind.scales_out() {
                        "GreaterThanThreshold"
                    } else {
                        "LessThanThreshold"
                    }
                    .to_string(),
                )
            } else {
                (
                    metric_namespace.clone(),
                    metric_name.clone(),
                    statistic.clone(),
                    unit.clone(),
                    period,
                    utilisation_threshold(kind.scales_out()),
                    if kind.scales_out() {
                        "GreaterThanThreshold"
                    } else {
                        "LessThanThreshold"
                    }
                    .to_string(),
                )
            };
        alarms.push(AlarmSpec {
            kind,
            name: policy_alarm_name(key, kind),
            metric_namespace,
            metric_name,
            statistic,
            unit,
            dimensions: dimensions.clone(),
            period,
            evaluation_periods: if kind.watches_capacity() {
                CAPACITY_ALARM_EVALUATION_PERIODS
            } else if kind.scales_out() {
                tracked_rules.scale_out_evaluation_periods
            } else {
                tracked_rules.scale_in_evaluation_periods
            },
            threshold,
            comparison_operator,
        });
    }
    alarms
}

/// The target utilisation the alarm of one DIRECTION is computed from: the policy's own
/// `TargetValue` for scale-out, and that value buffered by the integration's own scale-in
/// buffer for scale-in.
///
/// Scale-out is driven at the target itself — the metric is above the target, so the policy
/// adds capacity — and scale-in only once the metric has fallen a buffer BELOW it, which is
/// what keeps a metric sitting near the target from flapping between the two directions.
fn direction_target_value(target_value: f64, rules: TrackedAlarmRules, scales_out: bool) -> f64 {
    if scales_out {
        return target_value;
    }
    match rules.scale_in_buffer {
        ScaleInBuffer::PercentagePoints(points) => target_value - points,
        ScaleInBuffer::FractionOfTarget(fraction) => target_value * fraction,
    }
}

/// Which of a target-tracking policy's OWN alarms `alarm_name` is, or `None` for an alarm
/// that policy did not mint.
///
/// A target-tracking policy is executed by the pair it minted on the metric it tracks: that
/// is the pair whose threshold the policy's target value decides, so a breach of one of them
/// names both the direction and the measurement the control loop needs. An alarm a client
/// wired to the policy's ARN itself, and the pair that watches the provisioned capacity the
/// thresholds were computed from, drive no capacity movement.
pub fn policy_alarm_kind(key: &PolicyKey, alarm_name: &str) -> Option<AlarmKind> {
    AlarmKind::ALL
        .into_iter()
        .find(|kind| policy_alarm_name(key, *kind) == alarm_name)
}

/// The capacity a target-tracking policy's control loop asks for, from the metric value its
/// alarm graded, or `None` when the configuration computes none.
///
/// A target-tracking policy carries no adjustment: it states a target value for a metric, so
/// the capacity it asks for is the LEAST capacity at which that metric would sit at or below
/// the target — "Application Auto Scaling […] scales the capacity to keep the metric at, or
/// close to, the specified target value". Which makes the arithmetic follow what the metric
/// MEASURES:
///
/// * a metric that counts the capacity units consumed over the alarm's period measures the
///   load in absolute units, and the target names what fraction of the provisioned capacity
///   that load may fill, so the capacity it asks for is
///   `measured / (TargetValue% * period)` — independent of the capacity the resource holds
///   now;
/// * a metric that already is the quantity the target names (a utilisation percentage, a
///   count per capacity unit) varies inversely with the capacity serving it, so the same load
///   at `wanted` units would read `measured * current / wanted`, and the capacity that puts
///   that at the target is `current * measured / TargetValue`.
///
/// The answer is taken UP: a capacity a fraction of a unit short of the target would leave
/// the metric past it. It is the capacity the policy asks for, before the registered
/// `MinCapacity`/`MaxCapacity` window is applied to it by the caller.
pub fn target_tracking_capacity(
    key: &PolicyKey,
    config: &TargetTrackingScalingPolicyConfiguration,
    measured: f64,
    current: i32,
) -> Option<i32> {
    let series = tracked_series(key, config)?;
    let target = config.target_value;
    // A target that is not a positive number scales nothing: it names no quantity to hold
    // the metric at, and nothing can be divided by it.
    if !measured.is_finite() || !target.is_finite() || target <= 0.0 {
        return None;
    }
    let wanted = if series.consumed_units {
        let per_unit = target / 100.0 * f64::from(series.period);
        if per_unit <= 0.0 {
            return None;
        }
        measured / per_unit
    } else {
        if current <= 0 {
            return None;
        }
        f64::from(current) * measured / target
    };
    if !wanted.is_finite() {
        return None;
    }
    Some(
        wanted
            .ceil()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
    )
}

/// The metric that reports the provisioned capacity the utilisation thresholds were computed
/// from, for a dimension whose tracked metric counts consumed units.
fn provisioned_capacity_metric_name(scalable_dimension: &str) -> String {
    if dimension_scaling_property(scalable_dimension) == "ReadCapacityUnits" {
        "ProvisionedReadCapacityUnits".to_string()
    } else {
        "ProvisionedWriteCapacityUnits".to_string()
    }
}

/// The alarm work one target's policies need: what each policy's alarm set should be, and
/// under which ARN its alarms are wired.
///
/// A plan is computed under THIS service's lock and applied with NO store lock held, which
/// is what keeps a cross-service write out of this service's critical section.
#[derive(Debug, Default)]
pub struct AlarmPlan {
    /// The region the alarms' own ARNs are scoped to.
    pub region: String,
    /// One entry per policy of the target: its key, its ARN, and the alarms it wants.
    pub policies: Vec<(PolicyKey, String, Vec<AlarmSpec>)>,
}

/// STEP 1 of the alarm sync, under this service's lock ONLY: what every policy of `target`
/// wants its alarm set to be.
///
/// The alarms are scoped to the target's own Region, which its identity carries.
///
/// `resolved_capacity` is the resource's own provisioned capacity, read from the service
/// that owns it BEFORE this lock was taken. `None` falls back to the target's
/// `MinCapacity`, which Application Auto Scaling keeps a scalable resource at or above, in
/// two cases: a resource that cannot be read any more (deleted under the target), and a
/// namespace whose store this emulator cannot resolve at all — so a Keyspaces
/// (`cassandra`) target's consumed-units threshold stays pinned to `MinCapacity` until
/// that store has a resolver, which is a row in [`resolve_target_resource`].
pub fn plan_target_alarms(
    state: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    target: &TargetKey,
    resolved_capacity: Option<i64>,
) -> AlarmPlan {
    let provisioned_capacity = resolved_capacity.unwrap_or_else(|| {
        state
            .targets
            .get(target)
            .map_or(0, |record| i64::from(record.min_capacity))
    });
    let policies = state
        .policies
        .iter()
        .filter(|(key, _)| &key.target == target)
        .map(|(key, record)| {
            (
                key.clone(),
                record.policy_arn.clone(),
                target_tracking_alarms(key, record, provisioned_capacity),
            )
        })
        .collect();
    AlarmPlan {
        region: target.region.clone(),
        policies,
    }
}

/// STEP 2, with NO store lock held: make CloudWatch hold exactly the alarms `plan` wants,
/// and answer what it now holds per policy.
pub fn apply_target_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    plan: &AlarmPlan,
) -> Vec<(PolicyKey, Vec<Alarm>)> {
    plan.policies
        .iter()
        .map(|(key, policy_arn, wanted)| {
            (
                key.clone(),
                apply_alarms(engine, &plan.region, key, policy_arn, wanted),
            )
        })
        .collect()
}

/// STEP 3, under this service's lock again: record on each policy the alarms CloudWatch now
/// holds for it, so `DescribeScalingPolicies` and `PutScalingPolicy` report them.
///
/// Answers the policies that are GONE — deleted while the lock was released — whose alarms
/// CloudWatch now holds for nothing. The caller retires those once it has dropped the lock
/// again, so the window the split opens cannot leave an orphaned alarm behind.
#[must_use]
pub fn record_minted_alarms(
    state: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    minted: Vec<(PolicyKey, Vec<Alarm>)>,
) -> Vec<PolicyKey> {
    let mut orphaned = Vec::new();
    for (key, alarms) in minted {
        match state.policies.get_mut(&key) {
            Some(record) => record.alarms = alarms,
            None if !alarms.is_empty() => orphaned.push(key),
            None => {}
        }
    }
    orphaned
}

/// Retire what a policy that is going away owns in CloudWatch, and UN-WIRE it from every alarm
/// that invokes it.
///
/// Call it with NO store lock held. A deleted policy leaves two different things behind, and
/// each goes on its own terms:
///
/// * the alarms the policy MINTED exist only because the policy does — "deleting a target
///   tracking scaling policy deletes the CloudWatch alarms that it created" — and only a
///   target-tracking policy mints any, which is why the caller passes the deleted policy's
///   type;
/// * every other alarm that invokes it is the CALLER's and survives — "deleting a step scaling
///   policy deletes the underlying alarm ACTION, but does not delete the CloudWatch alarm" — so
///   what goes from it is the wiring: the deleted policy's ARN is removed from the action lists
///   that name it, and the alarm keeps its metric, its statistic, its threshold, its comparison
///   operator and its other actions. An alarm left naming a policy that no longer exists would
///   carry an action that can never fire, which a client cannot tell from a live wiring.
///
/// The policy's ARN is derived from its identity ([`scaling_policy_arn`]), which is the ARN its
/// record carried, and an action names its policy whether or not it carries the suffix saying
/// the policy minted the alarm ([`policy_arn_in_action`]).
pub fn retire_policy_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    key: &PolicyKey,
    policy_type: &str,
) {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

    let Some(store) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let policy_arn = scaling_policy_arn(key);
    let mut store = store.lock();
    if policy_type == TARGET_TRACKING_POLICY_TYPE {
        for kind in AlarmKind::ALL {
            store.alarms.remove(&policy_alarm_name(key, kind));
        }
    }
    for stored in store.alarms.values_mut() {
        let alarm = &mut stored.alarm;
        for actions in [
            alarm.alarm_actions.as_mut(),
            alarm.ok_actions.as_mut(),
            alarm.insufficient_data_actions.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            actions.retain(|action| policy_arn_in_action(action) != policy_arn.as_str());
        }
    }
}

/// The handle a handler holds on this service's store, as [`service_state`] answers it.
///
/// Named so the alarm-sync helpers below can take the HANDLE rather than a guard: they lock
/// and unlock the store several times in one sequence, which is what keeps the CloudWatch
/// write out of this service's critical section.
pub type StateHandle =
    ::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>;

/// Make CloudWatch hold exactly the alarms `target`'s policies want, taking this store's
/// turn at alarm writes ([`AlarmSyncToken`]) for the WHOLE plan → apply → record sequence.
///
/// The one way this service writes a target's alarms, so every call that changes what those
/// alarms should be — putting a policy, registering the target, writing a managed target —
/// leaves CloudWatch holding the alarm set computed from the policies this store holds, and
/// never one computed from a configuration that has since been overwritten. The plan is read
/// INSIDE the turn for exactly that reason: the holder that writes last is the one that read
/// last.
///
/// Call it with NO store lock held. `resolved_capacity` is the capacity the thresholds are
/// computed from, read from the service that owns the resource before any lock was taken;
/// `None` falls back to the target's own floor (see [`plan_target_alarms`]). A policy deleted
/// while this sequence ran owns nothing any more, so the alarms just written for it are
/// retired rather than left behind.
pub fn sync_target_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &StateHandle,
    target: &TargetKey,
    resolved_capacity: Option<i64>,
) {
    let token = state.lock().alarm_sync();
    let _turn = token.take_turn();
    let plan = plan_target_alarms(&state.lock(), target, resolved_capacity);
    let minted = apply_target_alarms(engine, &plan);
    let orphaned = record_minted_alarms(&mut state.lock(), minted);
    for policy_key in &orphaned {
        retire_policy_alarms(engine, policy_key, TARGET_TRACKING_POLICY_TYPE);
    }
}

/// Retire what each `(policy, type)` pair that is going away owns in CloudWatch, taking this
/// store's turn at alarm writes ([`AlarmSyncToken`]) first.
///
/// The removal half of [`sync_target_alarms`], in the same turn order: a delete that ran
/// between another call's plan and its write would otherwise be undone by that write, and the
/// retired policy's alarms would outlive it. Call it with NO store lock held.
pub fn retire_policies_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &StateHandle,
    policies: &[(PolicyKey, String)],
) {
    if policies.is_empty() {
        return;
    }
    let token = state.lock().alarm_sync();
    let _turn = token.take_turn();
    for (policy_key, policy_type) in policies {
        retire_policy_alarms(engine, policy_key, policy_type);
    }
}
///
/// A policy's `Alarms` are "the CloudWatch alarms associated with the scaling policy"
/// (`ScalingPolicy$Alarms`), and what associates an alarm with a policy is the alarm's own
/// action list: "the actions to execute when this alarm transitions to the ALARM state […]
/// specified as an Amazon Resource Name (ARN)" (`PutMetricAlarmInput$AlarmActions`). So the
/// association is CloudWatch's state and is read from there rather than from what was sent to
/// this service — which is also why reporting it can fail on a CloudWatch read at all
/// (`FailedResourceAccessException`: "unable to retrieve the alarms associated with a scaling
/// policy"). It covers both the alarms a target-tracking policy minted for itself and an alarm
/// a client wired to a policy of its own.
///
/// Call it with NO store lock held. The ARN each action names is read with the minting suffix
/// stripped ([`policy_arn_in_action`]), and every policy's alarms are ordered by name so a read
/// reports them in a stable order. `region` names an alarm that carries no ARN of its own.
pub fn alarms_invoking_policies<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> BTreeMap<String, Vec<Alarm>> {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

    let mut invoking: BTreeMap<String, Vec<Alarm>> = BTreeMap::new();
    let Some(store) = cloudwatch::state::service_state(engine) else {
        return invoking;
    };
    let store = store.lock();
    for stored in store.alarms.values() {
        let Some(alarm_name) = stored.alarm.alarm_name.clone() else {
            continue;
        };
        let alarm_arn = stored
            .alarm
            .alarm_arn
            .clone()
            .unwrap_or_else(|| cloudwatch::state::alarm_arn(region, &alarm_name));
        for action in stored.alarm.alarm_actions.iter().flatten() {
            let listed = invoking
                .entry(policy_arn_in_action(action).to_string())
                .or_default();
            // One alarm is one entry, however many of its actions name the same policy.
            if listed.iter().any(|held| held.alarm_name == alarm_name) {
                continue;
            }
            listed.push(Alarm {
                alarm_name: alarm_name.clone(),
                alarm_arn: alarm_arn.clone(),
            });
        }
    }
    drop(store);
    for listed in invoking.values_mut() {
        listed.sort_by(|left, right| left.alarm_name.cmp(&right.alarm_name));
    }
    invoking
}

/// Create or update every alarm in `wanted` in the CloudWatch store, delete the ones this
/// policy no longer wants, and answer the modelled `Alarm` list for the policy record.
fn apply_alarms<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key: &PolicyKey,
    policy_arn: &str,
    wanted: &[AlarmSpec],
) -> Vec<Alarm> {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
    use cloudwatch::ops::types::{Dimension, MetricAlarm};

    let Some(store) = cloudwatch::state::service_state(engine) else {
        return Vec::new();
    };
    let mut store = store.lock();
    let now = cloudwatch::state::now_secs();

    for kind in AlarmKind::ALL {
        if wanted.iter().all(|spec| spec.kind != kind) {
            store.alarms.remove(&policy_alarm_name(key, kind));
        }
    }

    let mut minted = Vec::new();
    for spec in wanted {
        let arn = cloudwatch::state::alarm_arn(region, &spec.name);
        // An alarm that already exists keeps the state an evaluation gave it — including
        // the explanation that state was decided from, which is where the datapoints the
        // evaluation graded are reported (`MetricAlarm$StateReasonData`) — and the tags a
        // caller put on it; only its configuration is rewritten, exactly as PutMetricAlarm
        // does.
        let existing = store.alarms.get(&spec.name).map(|stored| {
            (
                stored.alarm.state_value.clone(),
                stored.alarm.state_reason.clone(),
                stored.alarm.state_reason_data.clone(),
                stored.alarm.state_updated_timestamp,
                stored.alarm.state_transitioned_timestamp,
                stored.tags.clone(),
            )
        });
        let (
            state_value,
            state_reason,
            state_reason_data,
            state_updated,
            state_transitioned,
            tags,
        ) = existing.unwrap_or_else(|| {
            (
                Some("INSUFFICIENT_DATA".to_string()),
                Some("Unchecked: initial alarm creation".to_string()),
                None,
                Some(cloudwatch::state::to_timestamp(now)),
                Some(cloudwatch::state::to_timestamp(now)),
                Vec::new(),
            )
        });
        // `CreationId` names the write that CREATED the alarm, so an alarm this policy
        // already minted keeps the identity it was created with and a fresh one mints its
        // own, exactly as PutMetricAlarm does.
        let creation_id = store
            .alarms
            .get(&spec.name)
            .map(|stored| stored.creation_id.clone())
            .unwrap_or_else(|| cloudwatch::state::alarm_creation_id(now));
        let alarm = MetricAlarm {
            alarm_name: Some(spec.name.clone()),
            alarm_arn: Some(arn.clone()),
            alarm_description: Some(format!(
                "DO NOT EDIT OR DELETE. For TargetTrackingScaling policy {} of scalable target \
                 {} {} {}.",
                key.policy_name,
                key.target.service_namespace,
                key.target.resource_id,
                key.target.scalable_dimension
            )),
            alarm_configuration_updated_timestamp: Some(cloudwatch::state::to_timestamp(now)),
            actions_enabled: Some(true),
            // The alarm's one action names the scaling policy it executes, with the
            // minting suffix that says the policy created it ([`policy_alarm_action`]):
            // that is the wiring which makes a target-tracking policy alarm-driven.
            alarm_actions: Some(vec![policy_alarm_action(policy_arn, key)]),
            state_value,
            state_reason,
            state_reason_data,
            state_updated_timestamp: state_updated,
            state_transitioned_timestamp: state_transitioned,
            metric_name: Some(spec.metric_name.clone()),
            namespace: Some(spec.metric_namespace.clone()),
            statistic: Some(spec.statistic.clone()),
            unit: spec.unit.clone(),
            dimensions: Some(
                spec.dimensions
                    .iter()
                    .map(|(name, value)| Dimension {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            ),
            period: Some(spec.period),
            evaluation_periods: Some(spec.evaluation_periods),
            threshold: Some(spec.threshold),
            comparison_operator: Some(spec.comparison_operator.clone()),
            ..MetricAlarm::default()
        };
        store.alarms.insert(
            spec.name.clone(),
            cloudwatch::state::StoredAlarm {
                alarm,
                tags,
                creation_id,
                // The alarm CloudWatch evaluates from the stored metric data, the same span
                // after this write as for one `PutMetricAlarm` created: "the alarm state is
                // immediately set to INSUFFICIENT_DATA. The alarm is then evaluated and its
                // state is set appropriately."
                evaluation_due_secs: now
                    + cloudwatch::state::alarm_evaluation::EVALUATION_DELAY_SECS,
                // Nothing has forced this alarm's state, so no forced span is standing that
                // its evaluation must not cut short.
                forced_state_until_secs: 0.0,
            },
        );
        minted.push(Alarm {
            alarm_name: spec.name.clone(),
            alarm_arn: arn,
        });
    }
    minted
}

// ---------------------------------------------------------------------------
// The targets another service manages on its own resources' behalf
// ---------------------------------------------------------------------------

/// The one policy type whose configuration IS a tracked metric and a target value, and
/// whose alarms this service owns.
pub const TARGET_TRACKING_POLICY_TYPE: &str = "TargetTrackingScaling";

/// What the service that OWNS a scalable resource wants this service to hold for one of
/// that resource's capacity dimensions: the capacity window, and the target-tracking
/// policy that steers it.
#[derive(Debug, Clone)]
pub struct ManagedTarget {
    /// The scalable target's `MinCapacity`.
    pub min_capacity: i32,
    /// Its `MaxCapacity`.
    pub max_capacity: i32,
    /// The `PolicyName` the owning service gives the policy it manages, which is what
    /// makes that policy addressable — and re-settable — through this service's own API.
    pub policy_name: String,
    /// The policy's `TargetTrackingScalingPolicyConfiguration`.
    pub target_tracking: TargetTrackingScalingPolicyConfiguration,
}

/// Materialise, for the service that owns the resource, the scalable target and the
/// target-tracking policy one of its capacity dimensions is scaled by — or retire both
/// when that dimension is no longer scaled (`wanted` is `None`).
///
/// Several services implement their own auto scaling settings ON TOP of Application Auto
/// Scaling: the settings a caller configures through the owning service's API are
/// scalable targets and scaling policies of THIS service, which is why that API documents
/// `application-autoscaling:DescribeScalableTargets` /
/// `application-autoscaling:DescribeScalingPolicies` as the permissions needed to read
/// them back. So the owning service writes them here instead of keeping a second,
/// disconnected copy, and a caller sees them through `DescribeScalableTargets` and
/// `DescribeScalingPolicies` exactly as it sees one it registered itself.
///
/// Everything this service's own `RegisterScalableTarget`/`PutScalingPolicy` guarantee
/// holds for a target written this way: the ARNs and creation times are minted once per
/// identity and survive a re-write, a capacity window that moves the resource's capacity
/// records a scaling activity, the target-tracking policy's CloudWatch alarms are
/// materialised, and retiring the target cascades into its policies, its scheduled
/// actions and those alarms. Call it with NO store lock of either service held; it does
/// nothing when this service's store is not reachable from `engine`.
pub fn apply_managed_target<Eng: Send + Sync + 'static>(
    engine: &Eng,
    key: &TargetKey,
    wanted: Option<&ManagedTarget>,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    // What the CloudWatch side owes once this service's lock is released: whether the
    // target's alarm set is to be re-materialised, and the policies whose alarms are to be
    // retired.
    let mut resync = false;
    let mut retired: Vec<(PolicyKey, String)> = Vec::new();
    {
        let mut guard = state.lock();
        let store = &mut *guard;
        match wanted {
            // "Deregistering a scalable target deletes the scaling policies and the
            // scheduled actions that are associated with it" — the same cascade, because
            // this is the same resource going away.
            None => {
                if let Some(removed) = store.targets.remove(key) {
                    store.arn_index.remove(&removed.arn);
                    retired = store
                        .policies
                        .iter()
                        .filter(|(policy_key, _)| &policy_key.target == key)
                        .map(|(policy_key, record)| {
                            (policy_key.clone(), record.policy_type.clone())
                        })
                        .collect();
                    store.policies.retain(|policy_key, _| &policy_key.target != key);
                    store
                        .scheduled_actions
                        .retain(|action_key, _| &action_key.target != key);
                }
            }
            Some(wanted) => {
                // The window as a window: the owning service validates its own settings,
                // and this service "will not scale capacity to values that are outside of
                // the minimum and maximum range", which inverted bounds are not.
                let min_capacity = wanted.min_capacity.min(wanted.max_capacity);
                let max_capacity = wanted.max_capacity.max(wanted.min_capacity);
                // The emulator's own store bound is what a first registration is held to,
                // exactly as `RegisterScalableTarget` is; past it the owning service's
                // resource simply carries no scalable target.
                let room = store.targets.len() < MAX_SCALABLE_TARGETS;
                let moved = match store.targets.get_mut(key) {
                    // An existing target keeps its ARN, its creation time, its tags and
                    // its suspended state: this is an update of that resource, not a new
                    // one. Its capacity is placed back inside the new window.
                    Some(existing) => {
                        existing.min_capacity = min_capacity;
                        existing.max_capacity = max_capacity;
                        let moved = existing.current_capacity.clamp(min_capacity, max_capacity);
                        let cause = (moved != existing.current_capacity).then(|| {
                            if moved > existing.current_capacity {
                                capacity_bound_enforcement_cause(
                                    CapacityBound::Minimum,
                                    min_capacity,
                                )
                            } else {
                                capacity_bound_enforcement_cause(
                                    CapacityBound::Maximum,
                                    max_capacity,
                                )
                            }
                        });
                        existing.current_capacity = moved;
                        cause.map(|cause| (moved, cause))
                    }
                    // Past the store bound the owning service's resource simply carries no
                    // scalable target.
                    None if !room => None,
                    None => {
                        let arn = scalable_target_arn(key, now);
                        store.arn_index.insert(arn.clone(), key.clone());
                        store.targets.insert(
                            key.clone(),
                            ScalableTargetRecord {
                                arn,
                                min_capacity,
                                max_capacity,
                                // A first registration adopts the window's floor rather
                                // than moving anything, so it records no activity.
                                current_capacity: min_capacity,
                                role_arn: service_linked_role_arn(&key.service_namespace),
                                creation_time: now,
                                suspended_state: None,
                                tags: BTreeMap::new(),
                                settled_arbitration: None,
                            },
                        );
                        None
                    }
                };
                if let Some((capacity, cause)) = moved {
                    store.record_capacity_change(key, capacity, cause, now);
                }
                // The policy exists only against a registered target, so it is written
                // only where one is.
                if store.targets.contains_key(key) {
                    let policy_key = PolicyKey {
                        target: key.clone(),
                        policy_name: wanted.policy_name.clone(),
                    };
                    // The ARN and the creation time are minted once per policy identity
                    // and survive every later write of it; the store bound is checked on
                    // the create path only, so an update still lands at the ceiling.
                    let identity = match store.policies.get(&policy_key) {
                        Some(existing) => {
                            Some((existing.policy_arn.clone(), existing.creation_time))
                        }
                        None if store.policies.len() >= MAX_SCALING_POLICIES => None,
                        None => Some((scaling_policy_arn(&policy_key), now)),
                    };
                    // What the policy has already been executed for survives a re-write of
                    // it too: this is the same policy, so a breach it has performed must not
                    // be performed again.
                    let (executed_transitions, last_executed) = store
                        .policies
                        .get(&policy_key)
                        .map(|existing| {
                            (
                                existing.executed_transitions.clone(),
                                existing.last_executed,
                            )
                        })
                        .unwrap_or_default();
                    if let Some((policy_arn, creation_time)) = identity {
                        store.policies.insert(
                            policy_key,
                            ScalingPolicyRecord {
                                policy_arn,
                                policy_type: TARGET_TRACKING_POLICY_TYPE.to_string(),
                                step_config: None,
                                target_tracking_config: Some(wanted.target_tracking.clone()),
                                predictive_config: None,
                                // Filled in by the alarm sync below, which is what creates
                                // them in CloudWatch.
                                alarms: Vec::new(),
                                creation_time,
                                executed_transitions,
                                last_executed,
                            },
                        );
                    }
                    // The alarm set is re-materialised with this service's lock released,
                    // in this store's turn at alarm writes, so what CloudWatch ends up
                    // holding is computed from the policy this write left behind.
                    resync = true;
                }
            }
        }
    }
    // With this service's lock released: the CloudWatch writes, exactly as
    // `PutScalingPolicy` and `DeregisterScalableTarget` sequence them.
    if resync {
        sync_target_alarms(engine, &state, key, None);
    }
    retire_policies_alarms(engine, &state, &retired);
}

/// The capacity window and the target-tracking configuration this service holds for
/// `key`, for the service that OWNS the resource to report back: `None` when no scalable
/// target is registered for that dimension, so there are no settings to report.
///
/// The configuration is the one of the target's target-tracking policies that
/// [`apply_managed_target`] writes under `policy_name`, falling back to any other
/// target-tracking policy of the same target — a caller may steer the same dimension with
/// a policy it put there itself, and that policy is then what the dimension is scaled by.
pub fn managed_target_settings(
    state: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    key: &TargetKey,
    policy_name: &str,
) -> Option<(i32, i32, Option<TargetTrackingScalingPolicyConfiguration>)> {
    let target = state.targets.get(key)?;
    let tracking = state
        .policies
        .iter()
        .filter(|(policy_key, record)| {
            &policy_key.target == key && record.policy_type == TARGET_TRACKING_POLICY_TYPE
        })
        .min_by_key(|(policy_key, _)| policy_key.policy_name != policy_name)
        .and_then(|(_, record)| record.target_tracking_config.clone());
    Some((target.min_capacity, target.max_capacity, tracking))
}

// ---------------------------------------------------------------------------
// Scheduled actions
// ---------------------------------------------------------------------------

/// A scheduled action's parsed `Schedule`.
///
/// "The following formats are supported: At expressions - at(yyyy-mm-ddThh:mm:ss), Rate
/// expressions - rate(value unit), Cron expressions - cron(fields)" — three forms, so three
/// variants, and anything else is not a schedule at all.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// One occurrence, at this LOCAL date and time: "at and cron expressions use Universal
    /// Coordinated Time (UTC) by default" — by default, so the wall-clock reading the
    /// expression carries is resolved in the action's own time zone, not stored as an
    /// instant.
    At(::chrono::NaiveDateTime),
    /// An occurrence every `period`.
    Rate(::std::time::Duration),
    /// An occurrence at every instant the cron expression names. Boxed because a parsed
    /// cron expression is an order of magnitude larger than the other two forms.
    Cron(Box<::cron::Schedule>),
}

/// The units a `rate(value unit)` expression accepts, and what each is worth in seconds:
/// "For rate expressions, value is a positive integer and unit is minute | minutes | hour
/// | hours | day | days."
///
/// The one place the unit set lives — [`parse_schedule`] reads membership and the
/// multiplier from it, and the suites pin the set through it.
pub const RATE_UNITS: &[(&str, u64)] = &[
    ("minute", 60),
    ("minutes", 60),
    ("hour", 3_600),
    ("hours", 3_600),
    ("day", 86_400),
    ("days", 86_400),
];

/// How many whitespace-separated fields a `cron(…)` schedule carries: "The cron format
/// consists of six fields separated by white spaces: [Minutes] [Hours] [Day_of_Month]
/// [Month] [Day_of_Week] [Year]."
pub const CRON_FIELDS: usize = 6;

/// The largest instant this service computes with: 9999-12-31T23:59:59Z, the end of the
/// range a modelled timestamp can be rendered in.
///
/// Every request-derived instant and interval is bounded by it, so no caller-chosen value
/// can reach an arithmetic operation that is not representable.
pub const MAX_TIMESTAMP_SECS: u64 = 253_402_300_799;

/// The longest interval a `rate(value unit)` expression may name.
///
/// The EMULATOR's own bound, not a documented AWS one: `value` is an arbitrary integer and
/// an interval has to be addable to an instant, so the magnitude is refused here rather
/// than carried into instant arithmetic that cannot represent it. A hundred years is far
/// past any interval a scheduled action is useful at.
pub const MAX_RATE_SECS: u64 = 100 * 365 * 86_400;

/// How this service states a missing `Schedule` on the create path: the member is required
/// when the action is created, and an absent member has no value to echo.
pub const SCHEDULE_MUST_BE_SET: &str = "Schedule must be set";

/// How this service states a scheduling window whose boundaries are in the wrong order.
pub const START_TIME_BEFORE_END_TIME: &str = "StartTime must be before EndTime";

/// The detail a `Schedule` that is not one of the three documented forms is refused with.
///
/// The three forms are the model's own ("At expressions - at(yyyy-mm-ddThh:mm:ss), Rate
/// expressions - rate(value unit), Cron expressions - cron(fields)"), and the service states
/// them back as the syntax a schedule expression has to have: a caller whose expression is
/// not a schedule at all is told the grammar, not which member failed which constraint. A
/// detail is a sentence of its own, so it carries its own full stop ([`refuse_schedule`]).
pub const SCHEDULE_SYNTAX_DETAIL: &str = "Schedule expressions must have the following syntax: \
     rate(<number>\\s?(minutes?|hours?|days?)), cron(<cron_expression>) or \
     at(yyyy-MM-dd'T'HH:mm:ss).";

/// The six cron fields, in the order the service documents them, as one phrase \u2014 what a
/// refusal names when the expression does not carry that many.
pub const CRON_FIELD_LAYOUT: &str = "[Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]";

/// The name of each cron field, in order: "The cron format consists of six fields separated
/// by white spaces: [Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]."
pub const CRON_FIELD_NAMES: [&str; CRON_FIELDS] = [
    "Minutes",
    "Hours",
    "Day_of_Month",
    "Month",
    "Day_of_Week",
    "Year",
];

/// Wrap one parse detail as the `ValidationException` message a schedule expression is
/// refused with: the service's own sentence for "this is not a schedule", with the detail of
/// what stopped the parse.
///
/// The detail is a statement in its own right and carries its own punctuation, so the
/// envelope adds none: a cron body the parser refused is stated as the parser states it
/// ([`cron_parse_failure`]), down to the quoted token it ends on.
fn refuse_schedule(detail: &str) -> String {
    format!("Invalid schedule expression. Details: {detail}")
}

/// Parse a `Schedule`, or the `ValidationException` message for one that is not a schedule.
pub fn parse_schedule(schedule: &str) -> Result<Schedule, String> {
    let body = |keyword: &str| {
        schedule
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
    };

    if let Some(instant) = body("at") {
        let parsed = ::chrono::NaiveDateTime::parse_from_str(instant.trim(), "%Y-%m-%dT%H:%M:%S")
            .map_err(|_| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        return Ok(Schedule::At(parsed));
    }
    if let Some(rate) = body("rate") {
        let mut parts = rate.split_whitespace();
        let (Some(value), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(refuse_schedule(SCHEDULE_SYNTAX_DETAIL));
        };
        let value: u64 = value
            .parse()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        let seconds = RATE_UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, seconds)| *seconds)
            .ok_or_else(|| refuse_schedule(SCHEDULE_SYNTAX_DETAIL))?;
        // The magnitude is refused here, where the caller is told why, rather than carried
        // into instant arithmetic that cannot represent it. The EMULATOR's own bound, so the
        // detail says what the bound is.
        let total = value
            .checked_mul(seconds)
            .filter(|total| *total <= MAX_RATE_SECS)
            .ok_or_else(|| {
                refuse_schedule(&format!(
                    "the schedule interval must not exceed {MAX_RATE_SECS} seconds."
                ))
            })?;
        return Ok(Schedule::Rate(::std::time::Duration::from_secs(total)));
    }
    if let Some(fields) = body("cron") {
        return Ok(Schedule::Cron(Box::new(
            parse_cron(fields).map_err(|detail| refuse_schedule(&detail))?,
        )));
    }
    Err(refuse_schedule(SCHEDULE_SYNTAX_DETAIL))
}

/// The time zone an `at` or `cron` expression is read in: "specifies the time zone used when
/// setting a scheduled action by using an at or cron expression. If a time zone is not
/// provided, UTC is used by default. Valid values are the canonical names of the IANA time
/// zones", so the name is resolved against the IANA zone database itself rather than against
/// a hand-written list, and an unknown name is the `ValidationException` message.
pub fn schedule_timezone(timezone: Option<&str>) -> Result<::chrono_tz::Tz, String> {
    let Some(timezone) = timezone else {
        return Ok(::chrono_tz::UTC);
    };
    timezone
        .parse::<::chrono_tz::Tz>()
        // The service's own sentence, built from the request: the fixed phrase plus the
        // name it could not resolve, which is the whole of what the caller got wrong.
        .map_err(|_| format!("Invalid schedule timezone: {timezone}"))
}

/// Parse the six-field cron expression the service documents into a [`cron::Schedule`].
///
/// "The cron format consists of six fields separated by white spaces: [Minutes] [Hours]
/// [Day_of_Month] [Month] [Day_of_Week] [Year]" — a minute-resolution expression, which the
/// `cron` crate reads in its seven-field form (seconds first), so a literal `0` seconds field
/// is prepended. A `?` ("no specific value") in the day-of-month or day-of-week field is the
/// same statement as `*` in the grammar the crate parses; `?` in any OTHER field states
/// nothing, so it is a token that stops the parse.
///
/// Every refusal is the parser's own statement inside the envelope a refused cron body is
/// reported in ([`cron_parse_failure`]), and every POSITION it names is an offset into the
/// `cron(…)` body. Which token stopped the parse is decided field by field: each field is
/// parsed in its own place (the other five held at `*`, which every field accepts), so the
/// field that cannot be read is the one reported rather than the whole expression.
fn parse_cron(body: &str) -> Result<::cron::Schedule, String> {
    let fields = whitespace_fields(body);
    for (index, (at, field)) in fields.iter().enumerate().take(CRON_FIELDS) {
        if !cron_field_parses(index, field) {
            return Err(cron_parse_failure(*at, &illegal_characters(field)));
        }
    }
    if let Some((at, field)) = fields.get(CRON_FIELDS) {
        return Err(cron_parse_failure(
            *at,
            &format!(
                "Unexpected token '{}' after the {} field, the CRON format consists of \
                 {CRON_FIELDS} fields ({CRON_FIELD_LAYOUT}).",
                field.to_uppercase(),
                CRON_FIELD_NAMES[CRON_FIELDS - 1]
            ),
        ));
    }
    if fields.len() < CRON_FIELDS {
        return Err(cron_parse_failure(
            body.chars().count(),
            &format!(
                "Unexpected end of expression, the {} field is missing, the CRON format \
                 consists of {CRON_FIELDS} fields ({CRON_FIELD_LAYOUT}).",
                CRON_FIELD_NAMES[fields.len()]
            ),
        ));
    }
    let normalised: Vec<String> = ::std::iter::once("0".to_string())
        .chain(
            fields
                .iter()
                .enumerate()
                .map(|(index, (_, field))| normalised_cron_field(index, field).to_string()),
        )
        .collect();
    // Every field parses in its own place, so what is left is a combination of fields the
    // grammar does not allow, which belongs to no single token: the parse stops where the
    // expression begins.
    normalised
        .join(" ")
        .parse::<::cron::Schedule>()
        .map_err(|_| cron_parse_failure(0, UNSUPPORTED_CRON_COMBINATION))
}

/// How the parser states where a refused `cron(…)` body stopped it: the offset into that
/// BODY, counted from zero, and its own statement of what stopped it.
///
/// One envelope for every cron refusal, so a caller that surfaces or parses the detail reads
/// the same two parts whichever field was at fault.
fn cron_parse_failure(position: usize, detail: &str) -> String {
    format!("Invalid CRON expression: error at position {position}, details: {detail}")
}

/// The parser's statement about a token it cannot read where that token stands.
///
/// The expression is read in UPPER CASE — a named month or day of week is one of a fixed set
/// of names, matched whatever case it was written in — so the token is quoted as the parser
/// saw it rather than as the caller typed it.
fn illegal_characters(token: &str) -> String {
    format!(
        "Illegal characters for this position: '{}'",
        token.to_uppercase()
    )
}

/// The parser's statement about six fields that each parse where they stand but do not
/// combine into one expression, which belongs to no single token of the body.
const UNSUPPORTED_CRON_COMBINATION: &str = "Unsupported combination of CRON fields.";

/// The whitespace-separated fields of `text`, each with the CHARACTER position it starts at \u2014
/// the position a parse refusal reports, which `split_whitespace` alone does not carry.
fn whitespace_fields(text: &str) -> Vec<(usize, &str)> {
    let mut fields = Vec::new();
    let mut start: Option<(usize, usize)> = None;
    for (position, (at, character)) in text.char_indices().enumerate() {
        match (character.is_whitespace(), start) {
            (false, None) => start = Some((position, at)),
            (true, Some((position, from))) => {
                fields.push((position, &text[from..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some((position, from)) = start {
        fields.push((position, &text[from..]));
    }
    fields
}

/// What the `cron` crate's grammar reads a field as: `?` ("no specific value") is `*` in the
/// day-of-month and day-of-week fields, and means nothing anywhere else.
fn normalised_cron_field(index: usize, field: &str) -> &str {
    /// The indexes of the day-of-month and day-of-week fields in [`CRON_FIELD_NAMES`].
    const NO_SPECIFIC_VALUE_FIELDS: [usize; 2] = [2, 4];

    if field == "?" && NO_SPECIFIC_VALUE_FIELDS.contains(&index) {
        "*"
    } else {
        field
    }
}

/// Whether `field` is a legal cron field in the place `index` names, judged by parsing it
/// THERE with every other field held at `*`.
fn cron_field_parses(index: usize, field: &str) -> bool {
    // The crate's seven-field form: seconds, then the six the service documents.
    let mut expression = ["0", "*", "*", "*", "*", "*", "*"];
    expression[index + 1] = normalised_cron_field(index, field);
    expression
        .join(" ")
        .parse::<::cron::Schedule>()
        .is_ok()
}

/// Apply every scheduled action whose occurrence the clock has already made due.
///
/// "At the scheduled time, if the current capacity is below the minimum capacity, Application
/// Auto Scaling scales out to the minimum capacity. If the current capacity is above the
/// maximum capacity, Application Auto Scaling scales in to the maximum capacity" — the action
/// moves the scalable target's own bounds, and it was due at its scheduled instant whether or
/// not anybody called, which is why a reporting operation may materialise it.
///
/// The occurrence that fires is the first one after the action was last applied (or after it
/// was created), bounded by `StartTime` and `EndTime` when it carries them: "when you specify
/// start and end times with a recurring schedule using a cron expression or rates, they form
/// the boundaries for when the recurring action starts and stops". An occurrence whose target
/// has scheduled scaling SUSPENDED is not initiated, and not replayed when it resumes.
///
/// Occurrences are initiated in the order of the instants they were due at ([`due_occurrence`]
/// decides them all before any is applied), which is the order they took effect in.
pub fn apply_due_scheduled_actions(
    state: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    now: SystemTime,
) {
    // WHICH occurrences are due is decided for every stored action first, and they are
    // initiated afterwards in the order of the INSTANTS they were due at — the order the
    // clock reached them, which is the order they took effect in, whatever order the actions
    // were created or named in. Two actions queued against one target therefore leave it at
    // the bounds the later-scheduled of the two set, and the activity each recorded carries
    // the instant it acted at.
    let mut due: Vec<(SystemTime, ScheduledActionKey)> = state
        .scheduled_actions
        .iter()
        .filter_map(|(key, record)| {
            due_occurrence(record, now).map(|occurrence| (occurrence, key.clone()))
        })
        .collect();
    due.sort();
    for (occurrence, key) in due {
        let Some(record) = state.scheduled_actions.get(&key) else {
            continue;
        };
        let action = record.action.scalable_target_action.clone();
        // "Set the value to true if you don't want Application Auto Scaling to add or remove
        // capacity by initiating scheduled actions" (`SuspendedState$ScheduledScalingSuspended`):
        // while scheduled scaling is suspended on the target, a due occurrence is not
        // initiated at all, so it moves neither the target's bounds nor its capacity. It is
        // still CONSUMED below rather than held back, because the instant has passed: an
        // occurrence missed under a suspension is not run when scaling resumes, which is
        // also why the call that resumes scaling materialises what the clock owed BEFORE it
        // clears the flag.
        let scheduled_scaling_suspended = state.targets.get(&key.target).is_some_and(|target| {
            target
                .suspended_state
                .as_ref()
                .and_then(|suspended| suspended.scheduled_scaling_suspended)
                .unwrap_or(false)
        });
        if !scheduled_scaling_suspended {
            // The bounds this occurrence sets, in the order `ScalableTargetAction` declares
            // them. An action that sets neither initiates nothing, so there is nothing to
            // record for it.
            let bounds: Vec<(CapacityBound, i32)> = action
                .iter()
                .flat_map(|action| {
                    [
                        (CapacityBound::Minimum, action.min_capacity),
                        (CapacityBound::Maximum, action.max_capacity),
                    ]
                })
                .filter_map(|(bound, capacity)| capacity.map(|capacity| (bound, capacity)))
                .collect();
            if !bounds.is_empty() {
                // The activities are recorded at the instant the occurrence was DUE, not at
                // the instant somebody happened to read it: what they describe was owed then.
                initiate_scheduled_action(state, &key, &bounds, occurrence);
            }
        }
        if let Some(record) = state.scheduled_actions.get_mut(&key) {
            record.last_applied = Some(occurrence);
        }
    }
}

/// Initiate ONE occurrence of a scheduled action: set the bounds it names on its scalable
/// target, enforce them on the target's capacity, and record what each of those two steps
/// did.
///
/// A firing is recorded as TWO activities, because two things happened. The DISPATCH is the
/// action firing: its cause is the action ([`scheduled_action_dispatch_cause`]) and its
/// description is the bounds it sets ([`bounds_setting_description`]). The ENFORCEMENT is
/// the capacity movement those bounds cause — "at the scheduled time, if the current
/// capacity is below the minimum capacity, Application Auto Scaling scales out to the
/// minimum capacity. If the current capacity is above the maximum capacity, Application Auto
/// Scaling scales in to the maximum capacity" — recorded by the one recorder every capacity
/// movement of this service goes through, caused by the bound that was set
/// ([`capacity_bound_enforcement_cause`]), and present only when the capacity really moved.
/// The dispatch is recorded FIRST, so a log read newest-first reports the movement and then
/// the firing that caused it.
///
/// The bounds the firing would leave the target with have to be a RANGE: "Application Auto
/// Scaling will not scale capacity to values that are outside of the minimum and maximum
/// range", and a minimum above the maximum is no range. Such an occurrence is refused at
/// dispatch — the target keeps the bounds and the capacity it had, and the dispatch activity
/// settles `Failed` carrying the same reason the operations that take a capacity window
/// answer inline ([`CAPACITY_RANGE_INVERTED_MESSAGE`]) — rather than applied as the point
/// the two bounds meet at, which would drive the resource past a bound its own registration
/// states.
fn initiate_scheduled_action(
    state: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    key: &ScheduledActionKey,
    bounds: &[(CapacityBound, i32)],
    occurrence: SystemTime,
) {
    let Some(target) = state.targets.get(&key.target) else {
        return;
    };
    // The window the firing would leave the target with: the bound it sets, beside the one
    // it does not, which the target keeps.
    let mut min_capacity = target.min_capacity;
    let mut max_capacity = target.max_capacity;
    let current_capacity = target.current_capacity;
    for (bound, capacity) in bounds {
        match bound {
            CapacityBound::Minimum => min_capacity = *capacity,
            CapacityBound::Maximum => max_capacity = *capacity,
        }
    }
    let dispatch_cause = scheduled_action_dispatch_cause(&key.scheduled_action_name);
    let description = bounds_setting_description(bounds);
    if min_capacity > max_capacity {
        state.record_activity(
            &key.target,
            occurrence,
            ActivityEntry {
                description,
                cause: dispatch_cause,
                capacity: current_capacity,
                status_code: ACTIVITY_FAILED.to_string(),
                status_message: Some(bounds_refused_status_message(
                    bounds,
                    CAPACITY_RANGE_INVERTED_MESSAGE,
                )),
                not_scaled_reasons: Vec::new(),
            },
        );
        return;
    }
    let moved = current_capacity.clamp(min_capacity, max_capacity);
    if let Some(target) = state.targets.get_mut(&key.target) {
        target.min_capacity = min_capacity;
        target.max_capacity = max_capacity;
        target.current_capacity = moved;
    }
    state.record_activity(
        &key.target,
        occurrence,
        ActivityEntry {
            description,
            cause: dispatch_cause,
            capacity: moved,
            status_code: ACTIVITY_SUCCESSFUL.to_string(),
            status_message: Some(bounds_set_status_message(bounds)),
            not_scaled_reasons: Vec::new(),
        },
    );
    if moved != current_capacity {
        // WHICH bound the movement enforced is which end of the window the capacity was
        // outside of: a capacity below the floor is scaled out to it, one above the ceiling
        // is scaled in to it.
        let bound = if moved > current_capacity {
            CapacityBound::Minimum
        } else {
            CapacityBound::Maximum
        };
        state.record_capacity_change(
            &key.target,
            moved,
            capacity_bound_enforcement_cause(bound, moved),
            occurrence,
        );
    }
}

/// The instant the LAST occurrence of `record` that the clock has already made due was due
/// at, or `None` when it has none to initiate yet.
///
/// Every occurrence of one action sets the same capacity bounds, so an action whose target
/// was not read for several periods owes the bounds its LAST due occurrence set, not its
/// first — which is why the walk runs to the last one and reports its instant, and why
/// `last_applied` ends there.
///
/// Every step is fallible on purpose: an action whose schedule, time zone or instants cannot
/// be computed with yields nothing, so no stored record can make a pass fail.
fn due_occurrence(record: &ScheduledActionRecord, now: SystemTime) -> Option<SystemTime> {
    let schedule = parse_schedule(&record.action.schedule).ok()?;
    let timezone = schedule_timezone(record.action.timezone.as_deref()).ok()?;
    let since = record
        .last_applied
        .unwrap_or_else(|| system_time_of(&record.action.creation_time));
    let start = record.action.start_time.as_ref().map(system_time_of);
    let end = record.action.end_time.as_ref().map(system_time_of);
    let mut window_start = match start {
        Some(start) if start > since => start,
        _ => since,
    };
    let mut applied = None;
    // The walk is bounded by [`MAX_DUE_OCCURRENCES_PER_PASS`], so a fine-grained schedule
    // left alone for a long time cannot make one request walk for ever; what is left is
    // picked up by the next pass.
    for _ in 0..MAX_DUE_OCCURRENCES_PER_PASS {
        // A schedule with no further occurrence ENDS the walk; it does not discard the
        // occurrences already found, which an `at(...)` expression would otherwise lose.
        let Some(occurrence) = next_occurrence(&schedule, timezone, window_start) else {
            break;
        };
        if occurrence > now || end.is_some_and(|end| occurrence > end) {
            break;
        }
        applied = Some(occurrence);
        window_start = occurrence;
    }
    applied
}

/// How many occurrences of ONE action a single pass materialises.
///
/// Every occurrence of an action sets the same capacity bounds, so walking them changes
/// nothing after the first — the walk exists only to leave `last_applied` at the last due
/// instant. The bound keeps the work per request a constant: a `rate(1 minute)` action
/// whose target is read a year later catches up over several passes instead of walking
/// half a million instants in one.
const MAX_DUE_OCCURRENCES_PER_PASS: usize = 1_000;

/// The first instant `schedule` fires at strictly after `after`, read in `timezone`.
///
/// `None` when the schedule names no further occurrence, or none that is representable:
/// every arithmetic step here is checked, because the schedule and the instant it is
/// computed from both come from a request.
fn next_occurrence(
    schedule: &Schedule,
    timezone: ::chrono_tz::Tz,
    after: SystemTime,
) -> Option<SystemTime> {
    use ::chrono::TimeZone;

    let local_after = timezone.from_utc_datetime(&datetime_of(after)?.naive_utc());
    match schedule {
        // A local wall-clock reading is one instant per zone — and, in the hour a zone
        // skips, none at all, which is a schedule with no occurrence rather than a guess.
        Schedule::At(at) => {
            let at = instant_of(timezone.from_local_datetime(at).earliest()?)?;
            (at > after).then_some(at)
        }
        Schedule::Rate(period) => after.checked_add(*period),
        Schedule::Cron(cron) => instant_of(cron.after(&local_after).next()?),
    }
}

/// `at` as a UTC date and time, or `None` when it is past the range one can represent.
fn datetime_of(at: SystemTime) -> Option<::chrono::DateTime<::chrono::Utc>> {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs()
        .min(MAX_TIMESTAMP_SECS);
    ::chrono::DateTime::from_timestamp(i64::try_from(seconds).ok()?, 0)
}

/// A zoned date and time as a `SystemTime`, or `None` when it is outside the representable
/// range.
fn instant_of<Tz: ::chrono::TimeZone>(at: ::chrono::DateTime<Tz>) -> Option<SystemTime> {
    let seconds = u64::try_from(at.timestamp()).ok()?;
    (seconds <= MAX_TIMESTAMP_SECS)
        .then(|| SystemTime::UNIX_EPOCH.checked_add(::std::time::Duration::from_secs(seconds)))
        .flatten()
}

/// The instant `seconds` epoch seconds names, clamped to the range this service computes in.
///
/// A second count reaches this service from a request, and from the state another service
/// stored, so neither end of it is trusted: a value below the epoch reads as the epoch and one
/// past [`MAX_TIMESTAMP_SECS`] (or not a finite number at all) as that bound, rather than
/// reaching a conversion that cannot represent it.
pub fn instant_of_epoch_secs(seconds: f64) -> SystemTime {
    if !seconds.is_finite() || seconds <= 0.0 {
        return SystemTime::UNIX_EPOCH;
    }
    let seconds = if seconds >= MAX_TIMESTAMP_SECS as f64 {
        MAX_TIMESTAMP_SECS
    } else {
        seconds as u64
    };
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A modelled timestamp as a `SystemTime`, clamped the same way
/// ([`instant_of_epoch_secs`]).
pub fn system_time_of(stamp: &::aws_smithy_types::DateTime) -> SystemTime {
    instant_of_epoch_secs(stamp.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// How many tags one scalable target holds.
///
/// `TagResource`'s own documentation points at "Tagging support for Application Auto
/// Scaling" and at "User-defined tag restrictions" for "the rules that apply to tag keys
/// and tag values"; those restrictions cap a resource at 50 tags. The model puts no
/// `@length` on `TagMap`, so this bound is enforced by the handlers rather than at the
/// boundary.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix Amazon Web Services reserves for its own tags: a caller may
/// neither set nor remove a key in it. `TagKey` carries no `@pattern`, so the reserved
/// namespace is enforced by the handlers.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The prefix is matched case-insensitively, which is deliberately a SUPERSET of the
/// documented rule rather than a modelled constraint: tag keys themselves are
/// case-sensitive ("both case-sensitive strings"), and AWS documents the reserved prefix
/// in lower case only, so `AWS:`-prefixed keys are refused here on the conservative
/// reading that a key cannot leave the reserved namespace by changing the case of the
/// prefix alone.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// `Ok` when no key in `keys` is in the reserved namespace, else the `ValidationException`
/// message every write to that namespace is refused with.
///
/// The keys are judged BEFORE anything is written, so a batch holding one reserved key is
/// refused whole and no key from it lands. The refusal is one fixed sentence
/// ([`SYSTEM_TAG_KEY_REFUSED`]) naming no key at all: every key in the namespace is refused
/// for the same reason, so a batch is refused with the same message whatever order its keys
/// arrive in.
pub fn check_tag_keys_unreserved<'a>(
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    if keys.into_iter().any(is_reserved_tag_key) {
        return Err(SYSTEM_TAG_KEY_REFUSED.to_string());
    }
    Ok(())
}

/// How this service states a write to the reserved tag namespace.
///
/// A key in it is a SYSTEM tag, and the refusal is about who the caller is rather than
/// about the key's shape: the sentence names neither the key nor the member, because every
/// key in that namespace is refused for the same reason.
pub const SYSTEM_TAG_KEY_REFUSED: &str =
    "Caller is an end user and not allowed to mutate system tags";

/// `Ok` when a batch's entry count is within [`MAX_TAGS_PER_RESOURCE`], else the
/// `ValidationException` message a request too large for ANY resource is refused with —
/// input validation, independent of which resource the request addresses.
pub fn check_tag_count(count: usize) -> Result<(), String> {
    if count <= MAX_TAGS_PER_RESOURCE {
        return Ok(());
    }
    Err(tag_set_too_large_message())
}

/// How this service states a tag set over [`MAX_TAGS_PER_RESOURCE`].
///
/// The limit is on the RESULTANT tag set — which a single batch already exceeds on its own,
/// whatever the resource holds — and it is this service's own bound (`TagMap` carries no
/// `@length`), so the sentence states the number, interpolated from the one constant that
/// holds it rather than written into the text twice.
pub fn tag_set_too_large_message() -> String {
    format!(
        "The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags."
    )
}

/// `TagKey`'s own `@length(min: 1, max: 128)`, and `TagValue`'s `@length(min: 0, max:
/// 256)` — the shapes `TagMap`'s key and value target.
///
/// A map's KEYS are not reached by the boundary validator (it judges a map's values, so
/// an over-long tag VALUE never arrives here, while an over-long tag KEY does), so these
/// two bounds are checked by the handlers. Lengths are in Unicode code points, as Smithy
/// `@length` counts a string.
pub const MIN_TAG_KEY_CHARS: usize = 1;
/// See [`MIN_TAG_KEY_CHARS`].
pub const MAX_TAG_KEY_CHARS: usize = 128;
/// See [`MIN_TAG_KEY_CHARS`].
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// `Ok` when `key` is within `TagKey`'s modelled length, else the `ValidationException`
/// message naming `member`, the key and the bounds.
pub fn check_tag_key(member: &str, key: &str) -> Result<(), String> {
    let chars = key.chars().count();
    if (MIN_TAG_KEY_CHARS..=MAX_TAG_KEY_CHARS).contains(&chars) {
        return Ok(());
    }
    Err(format!(
        "Value '{key}' at '{member}' failed to satisfy constraint: Member must have \
         length between {MIN_TAG_KEY_CHARS} and {MAX_TAG_KEY_CHARS}, inclusive"
    ))
}

/// `Ok` when `value` is within `TagValue`'s modelled length, else the
/// `ValidationException` message naming the key it belongs to and the bound.
pub fn check_tag_value(member: &str, key: &str, value: &str) -> Result<(), String> {
    let chars = value.chars().count();
    if chars <= MAX_TAG_VALUE_CHARS {
        return Ok(());
    }
    Err(format!(
        "Value at '{member}.{key}' failed to satisfy constraint: Member must have length \
         less than or equal to {MAX_TAG_VALUE_CHARS}"
    ))
}
