// SPDX-License-Identifier: CC0-1.0

//! EXECUTING a scaling policy: what happens when the CloudWatch alarm a policy is wired to
//! is in `ALARM`.
//!
//! A step scaling policy is not evaluated by this service's API at all — it is driven by an
//! alarm, and the wiring is the alarm's own action list: "The actions to execute when this
//! alarm transitions to the ALARM state from any other state. Each action is specified as an
//! Amazon Resource Name (ARN)" (`PutMetricAlarmInput$AlarmActions`). So a policy fires when an
//! alarm that names its `PolicyARN` among those actions moves into `ALARM`, whoever created
//! that alarm.
//!
//! Two policy types are executed here, each answering the same question — what capacity does
//! this breach ask for — from what its own document states:
//!
//! * `StepScaling` states the adjustment to make: "A set of adjustments that enable you to
//!   scale based on the size of the alarm breach"
//!   (`StepScalingPolicyConfiguration$StepAdjustments`), interpreted by its `AdjustmentType`;
//! * `TargetTrackingScaling` states a target value for a metric, so the capacity it asks for
//!   is the answer of a control loop over the value its own alarm graded
//!   ([`super::target_tracking_capacity`]) — which is why only the pair that policy minted on
//!   the metric it tracks executes it.
//!
//! A `PredictiveScaling` policy is driven by a forecast rather than by an alarm, so no breach
//! executes one.
//!
//! What an execution does, in order:
//!
//! 1. read the breach the alarm reports — the metric value it graded, and the DIFFERENCE
//!    between that value and the alarm's threshold, which is what a step adjustment's
//!    interval is an interval of ("If the metric value is above the breach threshold, the
//!    lower bound is inclusive […] Otherwise, it's exclusive", and the mirror of that for the
//!    upper bound);
//! 2. compute the capacity the policy asks for, as its own configuration states it;
//! 3. CLAMP that capacity into the `MinCapacity`/`MaxCapacity` the scalable target carries at
//!    this instant — "Application Auto Scaling will not scale capacity to values that are
//!    outside of the minimum and maximum range" — which is a re-read of the registered
//!    window, not of the window that was in force when the policy was put. An adjustment that
//!    overshoots a bound is still PERFORMED, at the bound;
//! 4. set the capacity of the resource the target scales, in the service that owns it; and
//! 5. record the movement as a scaling activity, which `DescribeScalingActivities` reports.
//!
//! The pass is owed by the CLOCK and not by any request: the alarm transitioned at an instant
//! in the past, so the execution was due then, whether or not anybody called this service
//! afterwards. That is what lets a reporting operation materialise it
//! ([`settle_policy_executions`]) — and, because a client that breaches an alarm watches the
//! SCALED RESOURCE rather than this service, what the one bounded in-process task
//! ([`drive_scaling_clock`]) exists for: without it nothing would execute for a client that
//! makes no further Application Auto Scaling call.
//!
//! A SCHEDULED ACTION is owed by the clock in exactly that sense — its occurrence comes due at
//! an instant nobody has to call at — so the same pass materialises the occurrences the clock
//! has made due ([`super::apply_due_scheduled_actions`]) and delivers the capacity they moved,
//! and the same task keeps watching while the store holds one.
//!
//! One transition fires one execution: the instant the alarm last moved into `ALARM` is
//! recorded on the policy, so no later pass performs the same breach twice. A breach that is
//! WITHHELD rather than performed is the exception, and is left unconsumed — the alarm is
//! still breaching, so the first pass that may perform it does:
//!
//! * the policy's cooldown makes the breach wait for a previous activity to take effect;
//! * the target's `SuspendedState` suspends the direction the movement would be in ("while a
//!   suspension is in effect, all scale-out activities that are triggered by a scaling policy
//!   are suspended"), so lifting the suspension delivers the withheld movement with no new
//!   metric movement of its own.
//!
//! And of the policies of ONE scalable target triggered together, only ONE is performed:
//! "Application Auto Scaling gives precedence to the policy that provides the largest capacity
//! for both scale out and scale in", so the movements asked for inside one window
//! ([`ARBITRATION_WINDOW`]) are compared and the largest resulting capacity is the one the
//! target is moved to — the others are suppressed, each consumed without a movement and without
//! an activity of its own.
//!
//! That decision is the target's, not one pass's: it is recorded on the target
//! ([`super::SettledArbitration`]) with the capacity it was taken FROM, and for as long as the
//! movement that took it is still being waited on, a policy of the same target whose trigger is
//! read after it is compared against it — its movement computed from the capacity as of the
//! decision, performed only if it provides MORE capacity than the decision settled on, and
//! otherwise suppressed. So however far apart the alarms of one target transition, the capacity
//! it lands on is the largest any of its policies asked for and never the accumulation of two
//! of them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use super::{
    service_state, ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState, PolicyKey,
    TargetKey,
};
// Which namespaces' resources are reached over the wire, which names are theirs and which
// half of a two-part capacity a dimension scales: the ONE list of them
// ([`super::remote_resources`]), so a registration's resolution and this delivery cannot
// disagree about it.
use super::remote_resources::{
    is_remote_capacity_namespace as is_remote_namespace, reads_the_capacity, CASSANDRA_NAMESPACE,
    CUSTOM_RESOURCE_NAMESPACE, LAMBDA_NAMESPACE,
};
use crate::ops::types::{
    NotScaledReason, StepScalingPolicyConfiguration, TargetTrackingScalingPolicyConfiguration,
};

/// The policy type whose own configuration states the arithmetic of its adjustment, and so the
/// one an alarm breach can be executed from on the policy document alone.
pub const STEP_SCALING_POLICY_TYPE: &str = "StepScaling";

/// The policy type whose configuration states a TARGET for the metric its own alarms watch,
/// so the capacity a breach asks for is computed from the value the alarm graded.
pub const TARGET_TRACKING_POLICY_TYPE: &str = "TargetTrackingScaling";

/// `Cooldown`'s documented default: "The amount of time, in seconds, to wait for a previous
/// scaling activity to take effect. If not specified, the default value is 300."
pub const DEFAULT_COOLDOWN_SECS: i64 = 300;

/// How many policy executions ONE pass performs.
///
/// The pass's OWN bound, not a limit AWS publishes: the number of stored policies and the
/// number of alarms wired to them are each bounded by a store bound, but their product is
/// what a pass walks, and every execution of it writes an activity and a resource. What the
/// budget does not reach is not dropped — the transitions it leaves unrecorded are still
/// newer than what the policy remembers, so the next pass performs them.
const MAX_EXECUTIONS_PER_PASS: usize = 32;

/// How many alarms ONE policy remembers a transition instant for.
///
/// A policy is the action of as many alarms as a client wires it to, and an alarm that is
/// deleted leaves its entry behind, so the memory is bounded: past this many the OLDEST
/// transition is forgotten, which at worst performs one extra execution for an alarm nothing
/// has fired in that long.
const MAX_REMEMBERED_ALARMS_PER_POLICY: usize = 32;

/// How long the breaches of one scalable target are collected over before the policies they
/// trigger are arbitrated against each other ([`arbitration_windows`]).
///
/// The window is what makes "triggered at the same time" decidable here. The alarms that
/// trigger the policies of one target move into `ALARM` one after another — each on its own
/// evaluation, each read by whichever pass comes next — so a pass that performed the first
/// policy it found would have moved the capacity before it had seen the second trigger at all,
/// and the precedence the service gives the largest resulting capacity could never be applied
/// to it. The span is this emulator's own, since nothing in a stored alarm state says how wide
/// the service's is: wide enough that the triggers a client drives together through separate
/// requests, and the evaluations this emulator materialises for them one at a time, are read as
/// one event, and still far inside the time the real service takes to execute a policy at all.
///
/// Nothing waits it out that has nothing left to collect: the window closes as soon as every
/// policy of the target an alarm can fire has a breach in hand ([`arbitration_windows`]), so it
/// is only really waited out by a target a strict SUBSET of whose policies is triggered — and
/// then only for as long as that subset's alarms keep breaching, since the breach a pass holds
/// is read from the alarm's state rather than remembered.
const ARBITRATION_WINDOW: Duration = Duration::from_secs(8);

/// How long one scaling-clock wake-up waits before looking for breaches again.
///
/// A breach is not an instant this store can compute: it happens in CloudWatch, whenever a
/// datapoint or a forced state moves an alarm, so the clock re-reads rather than waiting for a
/// due instant of its own. The wait is what keeps that a yield rather than a spin.
const SCALING_CLOCK_WAIT: Duration = Duration::from_millis(250);

/// How many times one scaling clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start, and whatever the
/// clock has not reached when it is spent is executed by the next clock (every call that
/// settles starts one if none is running) or by the next call that settles.
const MAX_SCALING_CLOCK_WAKE_UPS: usize = 2_400;

/// One alarm breach that has not been executed yet: which alarm, when it moved into `ALARM`,
/// and how far past its threshold the metric was.
#[derive(Debug, Clone)]
struct Breach {
    /// The alarm whose transition fires the policy, as its `AlarmName`.
    alarm_name: String,
    /// The instant it moved into `ALARM`, in epoch seconds — the identity of the transition,
    /// so the same one is executed once.
    transitioned_secs: f64,
    /// The alarm's own `Threshold`, which the difference below is measured from.
    threshold: f64,
    /// The difference between the metric and the alarm's `Threshold`, which is what a step
    /// adjustment's interval is an interval OF.
    difference: f64,
    /// Whether the metric was above the threshold rather than below it, which is what decides
    /// which end of a step's interval is inclusive.
    above: bool,
}

impl Breach {
    /// The metric value the alarm graded, which is what a target-tracking control loop
    /// computes a capacity from: the threshold plus the difference the alarm measured.
    ///
    /// An alarm whose state was forced rather than evaluated carries no datapoint, so the
    /// difference is the smallest one in the direction it breached ([`breach_of`]) and this
    /// answers the threshold itself — the metric is read as sitting exactly at the value the
    /// alarm compares against, which is the only thing such a state says.
    fn measured(&self) -> f64 {
        self.threshold + self.difference
    }
}

/// The configuration ONE firing policy computes its capacity from, by policy type.
#[derive(Debug, Clone)]
enum FiringConfig {
    /// `StepScaling`: the adjustments and how to interpret them.
    Step(StepScalingPolicyConfiguration),
    /// `TargetTrackingScaling`: the metric and the value to hold it at.
    TargetTracking(TargetTrackingScalingPolicyConfiguration),
}

/// One policy an alarm breach is to be executed on: the snapshot a pass takes under this
/// service's lock, so the CloudWatch read below happens with it released.
#[derive(Debug, Clone)]
struct FiringPolicy {
    key: PolicyKey,
    policy_arn: String,
    config: FiringConfig,
    /// The transitions this policy has already been executed for, by alarm name.
    executed: BTreeMap<String, f64>,
}

/// Perform every policy execution the clock has already made due.
///
/// Call it with NO store lock held: the pass reads this service's store, then CloudWatch's,
/// then the store of the service that owns a scaled resource, each on its own, so no request
/// holds two at once.
pub fn settle_policy_executions<Eng: Send + Sync + 'static>(engine: &Eng, region: &str) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // STEP 0, under this service's lock: every scheduled occurrence the clock has already
    // made due. A scheduled action fires at its own instant, with no alarm and no call
    // involved, and the capacity it moves is owed to the scaled resource exactly as a policy
    // execution's is — so it is materialised in the same pass, before the deliveries below.
    {
        let mut store = state.lock();
        super::apply_due_scheduled_actions(&mut store, SystemTime::now());
    }
    // A capacity movement another call made and the owning service has not been told about
    // yet goes first: it is older than anything this pass is about to perform.
    deliver_pending_capacity(engine, &state);
    // Then the movements already delivered whose capacity the owning service has since made
    // take effect: the activity each of them recorded ends at the instant that happened,
    // which was owed by that service's clock and not by this call.
    super::capacity_fulfilment::settle_activity_fulfilments(engine, region);
    // STEP 1, under this service's lock: which policies an alarm could be firing, and what
    // each of them has already been executed for.
    let candidates = { firing_candidates(&state.lock(), region) };
    if candidates.is_empty() {
        return;
    }
    // STEP 2, with every lock released but CloudWatch's: the breaches those policies' alarms
    // are reporting right now.
    let fired = unexecuted_breaches(engine, &candidates);
    if fired.is_empty() {
        return;
    }
    // STEP 3, in the service that OWNS each scaled resource: the capacity that resource holds
    // right now, which is the capacity an adjustment is computed from — "Application Auto
    // Scaling retrieves the current capacity of the resource".
    let targets: BTreeSet<TargetKey> = fired
        .iter()
        .map(|(policy, _)| policy.key.target.clone())
        .collect();
    let resolved: BTreeMap<TargetKey, i32> = targets
        .iter()
        .filter_map(|target| {
            resource_capacity(engine, target).map(|capacity| (target.clone(), capacity))
        })
        .collect();
    // STEP 4, under this service's lock again: the arithmetic, the clamp into the window the
    // target carries NOW, and the activity each movement is recorded as — which is also what
    // queues the capacity the resource is owed.
    {
        let mut store = state.lock();
        apply_breaches(&mut store, &fired, &resolved, SystemTime::now());
    }
    // STEP 5, with this service's lock released: the capacity each movement owes the resource
    // it scales, written in the service that owns it.
    deliver_pending_capacity(engine, &state);
}

/// Read and write what the resources this service scales OVER THE WIRE owe: the capacity each
/// of them holds right now, and the movements this store has queued for them.
///
/// Two namespaces are reached that way. A `custom-resource` scalable target IS an endpoint of
/// the caller's own application ([`super::custom_resource_endpoint`]), and an Amazon Keyspaces
/// table is read and moved by CALLING that service ([`super::keyspaces_tables`]), whose crate
/// cannot be named from here. So neither half of a decision about such a target can be taken
/// against a store: the capacity it is compared against has to be asked for, and the capacity
/// it moves to has to be sent. Both are calls, so they happen here — on the scaling clock,
/// which is the one caller of this service's own work that can await them — rather than in
/// [`settle_policy_executions`], which several handlers call with a store lock in hand.
///
/// The order is the order of a decision: the capacity is read BEFORE the occurrences and
/// breaches the pass materialises are decided, and the movements they queued are written
/// after.
pub async fn settle_remote_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    refresh_remote_capacities(&state, region, callback).await;
    settle_policy_executions(engine, region);
    deliver_remote_capacity(&state, region, callback).await;
}

/// How often the capacity of a resource this service reads OVER THE WIRE is re-read.
///
/// The EMULATOR's own bound, not a published one: the in-process clock wakes up several times
/// a second ([`SCALING_CLOCK_WAIT`]), and re-reading the caller's own endpoint that often
/// would make this service the busiest client of it. One read a second is far finer than the
/// shortest span any decision of this service is taken over, so nothing a decision depends on
/// is ever read stale by more than that.
const REMOTE_CAPACITY_READ_INTERVAL: Duration = Duration::from_secs(1);

/// How many of those reads ONE pass performs.
///
/// The pass's OWN bound, the sibling of [`MAX_EXECUTIONS_PER_PASS`] and for the same reason:
/// how many targets a store holds is the CALLER's choice, and each read of one is a signed
/// call with an answer to parse, so the work of a pass is fixed here rather than growing with
/// the registered-target count. Nothing starves: the targets whose capacity was read longest
/// ago go first, so what this pass leaves out is what the next pass takes.
const MAX_REMOTE_READS_PER_PASS: usize = 32;

/// Learn, for every target of this Region whose resource is reached OVER THE WIRE rather than
/// in a store this emulator holds, the capacity that resource holds — and keep it as the
/// capacity this service tracks for the target.
///
/// This RECORDS NO ACTIVITY and moves nothing: the capacity it writes is the one the resource
/// already had, which is a fact about that resource rather than a change this service made.
/// What it is for is the decisions taken afterwards: a scheduled action's firing and a
/// policy's breach both compare the capacity the resource holds against the window the target
/// carries, and for such a target that capacity can only be asked for.
async fn refresh_remote_capacities(
    state: &::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>,
    region: &str,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let now = SystemTime::now();
    let due: Vec<TargetKey> = {
        let mut store = state.lock();
        // A target that is no longer registered has no capacity to read, so its last-read
        // instant goes with it: the index holds one entry per registered target and no more.
        let registered: BTreeSet<TargetKey> = store.targets.keys().cloned().collect();
        store
            .remote_capacity_read_at
            .retain(|key, _| registered.contains(key));
        // An offer deadline belongs to a QUEUED movement, so one whose movement has been
        // taken, replaced or dropped — including a movement of a target that has since been
        // deregistered — leaves nothing to offer: the index holds one entry per queued
        // movement and no more.
        let queued: BTreeSet<TargetKey> = store.pending_capacity.keys().cloned().collect();
        store
            .pending_capacity_until
            .retain(|key, _| queued.contains(key));
        // Oldest read first, so the pass's budget cannot leave one target unread for ever.
        let mut due: Vec<(Option<SystemTime>, TargetKey)> = store
            .targets
            .keys()
            .filter(|key| key.region == region)
            .filter(|key| is_remote_namespace(&key.service_namespace))
            .filter(|key| {
                store
                    .remote_capacity_read_at
                    .get(*key)
                    .and_then(|read| read.checked_add(REMOTE_CAPACITY_READ_INTERVAL))
                    .is_none_or(|due| due <= now)
            })
            .map(|key| (store.remote_capacity_read_at.get(key).copied(), key.clone()))
            .collect();
        due.sort();
        due.truncate(MAX_REMOTE_READS_PER_PASS);
        due.into_iter().map(|(_, key)| key).collect()
    };
    for key in due {
        // The same read a registration of such a target resolves its resource with
        // ([`super::remote_resources`]), which is the one place that knows how each of those
        // namespaces is asked.
        let reported = super::remote_resources::read_remote_capacity(callback, region, &key).await;
        let mut store = state.lock();
        store.remote_capacity_read_at.insert(key.clone(), now);
        if let (Some(capacity), Some(target)) = (reported, store.targets.get_mut(&key)) {
            target.current_capacity = capacity;
        }
    }
}

/// Write every queued capacity movement whose resource is reached OVER THE WIRE, in the
/// Region that resource lives in.
///
/// The queue is the same one [`deliver_pending_capacity`] drains, and the two share it by
/// namespace: a movement this emulator can write into the owning service's store is taken
/// there, and one that has to be SENT to the service or the endpoint that owns the resource
/// is left queued for here. A delivered movement is gone from the queue, so no resource is
/// moved twice for one decision.
///
/// A movement the resource DECLINES is offered again by the next pass, until the resource
/// takes it or the offer's own window runs out (`super::CAPACITY_DELIVERY_WINDOW`): the
/// capacity is owed to that resource — this service has already recorded the movement as an
/// activity — and a resource that is still settling a change of its own declines until it has
/// settled. Past the window the resource is one that is not taking the movement at all, and
/// the refusal is reported where a developer can see it rather than passing silently.
async fn deliver_remote_capacity(
    state: &::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>,
    region: &str,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let pending: Vec<(TargetKey, i32)> = {
        let mut store = state.lock();
        let taken: Vec<TargetKey> = store
            .pending_capacity
            .keys()
            .filter(|key| key.region == region && is_remote_namespace(&key.service_namespace))
            .cloned()
            .collect();
        taken
            .into_iter()
            .filter_map(|key| {
                store
                    .pending_capacity
                    .remove(&key)
                    .map(|capacity| (key, capacity))
            })
            .collect()
    };
    for (target, capacity) in pending {
        let delivered = match target.service_namespace.as_str() {
            CUSTOM_RESOURCE_NAMESPACE => {
                super::custom_resource_endpoint::set_capacity(
                    callback,
                    region,
                    &target.resource_id,
                    capacity,
                )
                .await
            }
            CASSANDRA_NAMESPACE => {
                super::keyspaces_tables::set_table_capacity(
                    callback,
                    region,
                    &target.resource_id,
                    reads_the_capacity(&target.scalable_dimension),
                    capacity,
                )
                .await
            }
            LAMBDA_NAMESPACE => {
                super::lambda_functions::set_provisioned_concurrency(
                    callback,
                    region,
                    &target.resource_id,
                    capacity,
                )
                .await
            }
            _ => false,
        };
        if delivered {
            state.lock().pending_capacity_until.remove(&target);
            continue;
        }
        if offer_again(&mut state.lock(), &target, capacity, SystemTime::now()) {
            continue;
        }
        ::tracing::warn!(
            namespace = %target.service_namespace,
            resource = %target.resource_id,
            capacity,
            "the resource this scalable target scales did not take the capacity movement \
             this service performed"
        );
    }
}

/// Put a declined movement back on the queue, answering whether it was — which it is not once
/// the offer's window has run out, nor when a NEWER movement of the same target is already
/// queued, since that one is the capacity the target now holds.
fn offer_again(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    target: &TargetKey,
    capacity: i32,
    now: SystemTime,
) -> bool {
    if store.pending_capacity.contains_key(target) {
        return false;
    }
    if store
        .pending_capacity_until
        .get(target)
        .is_none_or(|until| *until <= now)
    {
        store.pending_capacity_until.remove(target);
        return false;
    }
    store.pending_capacity.insert(target.clone(), capacity);
    true
}

/// Write every queued capacity movement into the service that owns the resource it moved.
///
/// Application Auto Scaling moves a resource's capacity through the owning service, and that
/// write cannot happen under this service's lock, so a movement is QUEUED by the call that made
/// it (`record_capacity_change`) and delivered here. Delivery is at-most-once: a taken movement
/// is gone from the queue, and the capacity delivered for a target is the last one queued for
/// it, which is the capacity that target holds. Each movement is written in the REGION of the
/// target it moved, which is where that resource lives.
///
/// A movement whose resource is an ENDPOINT is left queued for
/// [`deliver_remote_capacity`], which can await the call it takes.
fn deliver_pending_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState>,
) {
    let pending = {
        let mut store = state.lock();
        // The movements whose resource this emulator writes in a store, taken out of the
        // queue; the rest stay queued for the delivery that can await a call
        // ([`deliver_remote_capacity`]).
        let taken: Vec<TargetKey> = store
            .pending_capacity
            .keys()
            .filter(|target| !is_remote_namespace(&target.service_namespace))
            .cloned()
            .collect();
        let mut local: Vec<(TargetKey, i32)> = Vec::with_capacity(taken.len());
        for target in taken {
            store.pending_capacity_until.remove(&target);
            if let Some(capacity) = store.pending_capacity.remove(&target) {
                local.push((target, capacity));
            }
        }
        local
    };
    for (target, capacity) in pending {
        set_resource_capacity(engine, &target.region, &target, capacity);
    }
}

/// Start the one in-process task that performs what the clock has already made due — the
/// alarm breaches this store's policies are wired to, and the occurrences of its scheduled
/// actions — and stands down once the store holds neither, or the wake-up cap is reached.
///
/// A client drives an alarm into `ALARM` in CloudWatch, or queues a scheduled action, and then
/// watches the resource that gets scaled — it makes no further Application Auto Scaling call —
/// so work that only the next call to this service could perform would never happen at all.
/// This is what makes the path observable without one. It does not change WHEN the work
/// happens: the task calls the same settle every handler calls, which performs what the stored
/// alarm states and schedules have already made due.
///
/// At most one clock runs per store (the store carries the claim, which a clock whose task has
/// ended without releasing it does not keep), and with no async runtime to spawn onto there is
/// no clock — the breaches are then executed by the next operation instead.
pub fn drive_scaling_clock<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    region: &str,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_scaling_clock(&mut state.lock()) else {
        return;
    };
    let engine = ::std::sync::Arc::clone(engine);
    let region = region.to_string();
    // The clock outlives the handler that started it, so it carries a CLONE of the callback
    // rather than the borrowed context it came from: the calls it makes on the caller's own
    // endpoint are the same in-process calls, made by the same identity.
    let callback = callback.clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a clock that ran out of work
        // has already released its claim inside the one critical section that observed it.
        let mut capped = true;
        for _ in 0..MAX_SCALING_CLOCK_WAKE_UPS {
            if !still_wanted_or_release(&mut state.lock(), generation) {
                capped = false;
                break;
            }
            // Every wake-up waits, which hands the runtime back between passes.
            ::tokio::time::sleep(SCALING_CLOCK_WAIT).await;
            settle_remote_resources(&*engine, &region, &callback).await;
        }
        if capped {
            // Only THIS clock's claim, so a clock started since is left holding its own.
            release_scaling_clock(&mut state.lock(), generation);
        }
    });
    set_scaling_clock(&mut claimed.lock(), generation, handle);
}

/// Claim the single scaling clock for this store: the claim's GENERATION when the caller may
/// start one, `None` when one is really running.
///
/// The claim is taken over from a clock whose TASK HAS ALREADY ENDED while still holding it: a
/// clock releases its claim itself on the two ends it can see coming — nothing left to watch,
/// and the wake-up cap — so a claim still held by a task that is over belongs to a clock that
/// ended on neither. Without the take-over no later call could ever start one again, and every
/// breach would wait for a caller to execute it.
fn claim_scaling_clock(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
) -> Option<u64> {
    let ended = store
        .scaling_clock
        .as_ref()
        .is_some_and(::tokio::task::JoinHandle::is_finished);
    if store.scaling_clock_running && !ended {
        return None;
    }
    if ended {
        ::tracing::warn!(
            "the scaling clock this store holds the claim for has ended without releasing it; \
             the claim is taken over so the breaches it owes are still executed"
        );
    }
    store.scaling_clock_running = true;
    store.scaling_clock_generation = store.scaling_clock_generation.wrapping_add(1);
    Some(store.scaling_clock_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a clock that ended on the
/// wake-up cap cannot clear a claim a clock started since is holding.
fn release_scaling_clock(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    generation: u64,
) {
    if store.scaling_clock_generation == generation {
        store.scaling_clock_running = false;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood down
/// installs nothing and aborts nothing.
fn set_scaling_clock(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.scaling_clock_generation != generation {
        handle.abort();
        return;
    }
    if let Some(previous) = store.scaling_clock.replace(handle) {
        previous.abort();
    }
}

/// Whether this store still holds work a later pass could reach — a capacity movement a
/// resource is still owed, a policy an alarm could fire, or any scheduled action — and, when
/// it does not, RELEASES the clock's claim.
///
/// A queued movement counts because the resource it moves may be one only a CALL can reach
/// ([`deliver_remote_capacity`]): the capacity was decided and the activity recorded, and
/// until that call is made the resource has not been moved at all, so a clock that stood down
/// with one queued would leave the movement undelivered for ever.
///
/// A scheduled action counts whether or not its next occurrence is still ahead: whether one
/// is takes the schedule, the time zone and the clock to decide, per action, and this is asked
/// on every wake-up, so the cheap test is the one taken. What it costs is that a store holding
/// only SPENT one-shot actions keeps the clock waking up until the wake-up cap retires it,
/// which is bounded and does nothing each time.
///
/// Observing "nothing is wanted" and standing down happen in ONE critical section, which is
/// what makes the hand-over race-free: a write holds this same lock to store the policy BEFORE
/// it tries to claim the clock, so either this call sees that policy and the running clock
/// keeps it, or the write's claim finds the flag cleared and starts a clock of its own.
fn still_wanted_or_release(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    generation: u64,
) -> bool {
    let wanted = store
        .policies
        .values()
        .any(|record| is_alarm_driven(&record.policy_type))
        // A movement nothing has delivered yet is owed to the resource it moved, and for a
        // resource reached over the wire this clock is what delivers it.
        || !store.pending_capacity.is_empty()
        // A scheduled action is driven by the CLOCK rather than by a call: a client that
        // queues one watches the resource it scales, not this service, so without the clock
        // the occurrence would wait for an Application Auto Scaling call that never comes.
        || !store.scheduled_actions.is_empty();
    if !wanted {
        release_scaling_clock(store, generation);
    }
    wanted
}

/// Whether an alarm breach executes a policy of this type at all.
fn is_alarm_driven(policy_type: &str) -> bool {
    matches!(
        policy_type,
        STEP_SCALING_POLICY_TYPE | TARGET_TRACKING_POLICY_TYPE
    )
}

/// Every policy of this store an alarm breach can be executed on, with what it has already
/// been executed for.
///
/// A policy whose scalable target is gone is left out: the target is what carries the window a
/// movement is clamped into and the capacity it moves, so there is nothing to execute against.
/// So is one whose own configuration is absent, which states no capacity to move to. And so is
/// one of ANOTHER Region: its alarms and the resource it scales live in that Region, so a pass
/// for this one neither reads nor moves them.
fn firing_candidates(
    store: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    region: &str,
) -> Vec<FiringPolicy> {
    store
        .policies
        .iter()
        .filter(|(key, _)| key.target.region == region)
        .filter(|(key, _)| store.targets.contains_key(&key.target))
        .filter_map(|(key, record)| {
            let config = match record.policy_type.as_str() {
                STEP_SCALING_POLICY_TYPE => FiringConfig::Step(record.step_config.clone()?),
                TARGET_TRACKING_POLICY_TYPE => {
                    FiringConfig::TargetTracking(record.target_tracking_config.clone()?)
                }
                _ => return None,
            };
            Some(FiringPolicy {
                key: key.clone(),
                policy_arn: record.policy_arn.clone(),
                config,
                executed: record.executed_transitions.clone(),
            })
        })
        .collect()
}

/// The breaches `candidates` have not been executed for yet, read from the CloudWatch alarm
/// store.
///
/// An alarm fires a policy when it is IN `ALARM`, its actions are enabled ("Indicates whether
/// actions should be executed during any changes to the alarm state",
/// `MetricAlarm$ActionsEnabled`) and the policy's ARN is one of the actions that state
/// executes. Only a METRIC alarm can fire one: a step adjustment's interval is an interval of
/// the difference between the metric and the alarm's `Threshold`, and a composite alarm has
/// neither.
fn unexecuted_breaches<Eng: Send + Sync + 'static>(
    engine: &Eng,
    candidates: &[FiringPolicy],
) -> Vec<(FiringPolicy, Breach)> {
    use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

    let Some(store) = cloudwatch::state::service_state(engine) else {
        return Vec::new();
    };
    let wired: BTreeMap<&str, &FiringPolicy> = candidates
        .iter()
        .map(|policy| (policy.policy_arn.as_str(), policy))
        .collect();
    let store = store.lock();
    let mut fired: Vec<(FiringPolicy, Breach)> = Vec::new();
    for stored in store.alarms.values() {
        let alarm = &stored.alarm;
        if alarm.state_value.as_deref() != Some(cloudwatch::state::ALARM_STATE_ALARM) {
            continue;
        }
        if alarm.actions_enabled == Some(false) {
            continue;
        }
        let Some(alarm_name) = alarm.alarm_name.clone() else {
            continue;
        };
        let transitioned_secs = alarm
            .state_transitioned_timestamp
            .as_ref()
            .or(alarm.state_updated_timestamp.as_ref())
            .map_or(0.0, ::aws_smithy_types::DateTime::as_secs_f64);
        let Some((threshold, difference, above)) = breach_of(alarm) else {
            continue;
        };
        for action in alarm.alarm_actions.iter().flatten() {
            // An action names its policy by ARN, whether or not it carries the suffix that
            // says the policy minted the alarm (`super::policy_alarm_action`).
            let Some(policy) = wired.get(super::policy_arn_in_action(action)) else {
                continue;
            };
            // The transition this policy was last executed for; a newer one is a breach it
            // owes, and the same one is a breach it has already performed.
            if policy
                .executed
                .get(&alarm_name)
                .is_some_and(|executed| *executed >= transitioned_secs)
            {
                continue;
            }
            fired.push((
                (*policy).clone(),
                Breach {
                    alarm_name: alarm_name.clone(),
                    transitioned_secs,
                    threshold,
                    difference,
                    above,
                },
            ));
        }
    }
    // A total order, so what the pass's budget reaches does not depend on how the alarm store
    // happens to enumerate.
    fired.sort_by(|(left_policy, left), (right_policy, right)| {
        left_policy
            .key
            .cmp(&right_policy.key)
            .then_with(|| left.alarm_name.cmp(&right.alarm_name))
    });
    fired.truncate(MAX_EXECUTIONS_PER_PASS);
    fired
}

/// How far past its threshold the metric an alarm watches was, and whether it was ABOVE the
/// threshold rather than below it, beside the threshold the difference is measured from.
///
/// The size of a breach is a measurement, so it is read from the alarm's own explanation of the
/// state it is in — "An explanation for the alarm state, in JSON format"
/// (`MetricAlarm$StateReasonData`), whose `recentDatapoints` are the values the evaluation
/// graded, newest last. An alarm whose state was forced rather than evaluated reports no such
/// datapoint: the breach is then the SMALLEST one in the direction the alarm breached, the
/// threshold itself, which the alarm's `ComparisonOperator` names. An alarm with no static
/// threshold at all (one that compares against an anomaly-detection band) has no difference to
/// take, and so fires no step.
fn breach_of(
    alarm: &::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ops::types::MetricAlarm,
) -> Option<(f64, f64, bool)> {
    let threshold = alarm.threshold?;
    let measured = alarm
        .state_reason_data
        .as_deref()
        .and_then(|data| ::serde_json::from_str::<::serde_json::Value>(data).ok())
        .and_then(|document| {
            document
                .get("recentDatapoints")?
                .as_array()?
                .last()?
                .as_f64()
        });
    match measured {
        // The metric itself: which side of the threshold it is on is what the measurement
        // says, whichever comparison put the alarm in ALARM.
        Some(value) => {
            let difference = value - threshold;
            Some((threshold, difference, difference >= 0.0))
        }
        None => Some((
            threshold,
            0.0,
            breaches_above(alarm.comparison_operator.as_deref()?)?,
        )),
    }
}

/// Whether a `ComparisonOperator` puts the alarm in `ALARM` for a metric ABOVE its threshold,
/// or `None` for one that breaches on either side (a band), which names no direction.
///
/// The operators are the model's own `ComparisonOperator` values, read as the comparison each
/// makes between the datapoint and the threshold.
fn breaches_above(comparison_operator: &str) -> Option<bool> {
    match comparison_operator {
        "GreaterThanOrEqualToThreshold" | "GreaterThanThreshold" | "GreaterThanUpperThreshold" => {
            Some(true)
        }
        "LessThanThreshold" | "LessThanOrEqualToThreshold" | "LessThanLowerThreshold" => {
            Some(false)
        }
        _ => None,
    }
}

/// Perform each fired breach: the adjustment, the clamp and the activity, which is also what
/// queues the capacity each movement owes the resource it scales.
///
/// Everything here is read UNDER this service's lock at the instant the execution happens, so
/// the window a movement is clamped into is the one the target carries now.
///
/// The movements the policies of ONE scalable target ask for are ARBITRATED before any of them
/// is performed: "Multiple scaling policies can be in force at the same time for the same
/// scalable target […] there is a chance that multiple policies could conflict, instructing the
/// scalable target to scale out or in at the same time. Application Auto Scaling gives
/// precedence to the policy that provides the largest capacity for both scale out and scale
/// in." So of the movements asked for together, the one that yields the LARGEST resulting
/// capacity is performed and every other is fully suppressed — its transition is consumed, and
/// it moves no capacity and records no activity, because one scalable target was moved once, by
/// one policy.
///
/// A breach whose trigger is only read AFTER that decision was taken — the alarms of one target
/// transition one at a time, and each is read by whichever pass comes next — is decided against
/// the decision itself rather than against the capacity it left behind
/// ([`plan_breach`]), so the policies of one target never accumulate their adjustments.
fn apply_breaches(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    fired: &[(FiringPolicy, Breach)],
    resolved: &BTreeMap<TargetKey, i32>,
    now: SystemTime,
) {
    // Until a target's arbitration window has closed there is nothing of it to perform: a
    // policy triggered inside that window has to be decided against its siblings, not before
    // them.
    let windows = arbitration_windows(store, fired);
    // What each breach WOULD do, decided before this pass has performed anything: movements are
    // comparable only when each is computed from the capacity the resource holds at ONE
    // instant.
    let planned: Vec<Disposition> = fired
        .iter()
        .map(|(policy, breach)| plan_breach(store, policy, breach, resolved, now, &windows))
        .collect();
    let winners = arbitrate(fired, &planned);
    for (index, ((policy, breach), planned_here)) in fired.iter().zip(&planned).enumerate() {
        // Only a breach that ASKED for a movement can lose the arbitration. One that is being
        // withheld — by a cooldown, by a suspended direction, by a window that is still open —
        // has asked for nothing yet, so it keeps its transition and the first pass that may
        // perform it still does.
        let suppressed = matches!(planned_here, Disposition::Move(_))
            && winners
                .get(&policy.key.target)
                .is_some_and(|winner| *winner != index);
        if suppressed {
            // Suppressed by the policy that provides the larger capacity: the transition is
            // consumed, so no later pass performs it, and the breach reaches neither the
            // resource nor the activity log.
            remember_transition(store, &policy.key, &breach.alarm_name, breach.transitioned_secs);
            continue;
        }
        // Decided again under this lock, against what the movements already performed in this
        // pass have changed — exactly as a pass that found this breach alone would decide it.
        match plan_breach(store, policy, breach, resolved, now, &windows) {
            // Left unconsumed: the alarm is still breaching, so the first later pass that may
            // perform this breach does.
            Disposition::Hold => {}
            Disposition::Consume => {
                remember_transition(
                    store,
                    &policy.key,
                    &breach.alarm_name,
                    breach.transitioned_secs,
                );
            }
            Disposition::NotScaled { reason, consume } => {
                record_not_scaled(store, policy, breach, reason, now);
                if consume {
                    remember_transition(
                        store,
                        &policy.key,
                        &breach.alarm_name,
                        breach.transitioned_secs,
                    );
                }
            }
            Disposition::Move(movement) => {
                perform_movement(store, policy, breach, movement, now);
            }
        }
    }
}

/// What a pass does with one breach it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    /// Not now, and NOT consumed: the breach waits for a cooldown to run out, for a suspended
    /// direction to be resumed, for its target's arbitration window to close, or for the policy
    /// or target that went missing under it.
    Hold,
    /// Evaluated and done with: there is no capacity movement in it, so the transition is
    /// remembered and no later pass looks at it again.
    Consume,
    /// Evaluated, and WITHHELD with a reason: there is no capacity movement in it, and the
    /// decision is recorded as the not-scaled activity [`WithheldReason`] states.
    NotScaled {
        /// Why no capacity moved.
        reason: WithheldReason,
        /// Whether the transition is done with, or left for a later pass to perform once
        /// what withheld the movement no longer does.
        consume: bool,
    },
    /// Move the target's capacity, as [`Movement`] states it.
    Move(Movement),
}

/// Why a scaling decision moved no capacity, as the model's `NotScaledReason$Code` names it.
///
/// The three codes a decision of this emulator can be withheld for, out of the value set that
/// member declares (`AutoScalingAnticipatedFlapping`, `TargetServicePutResourceAsUnscalable`,
/// `AlreadyAtMaxCapacity`, `AlreadyAtMinCapacity`, `AlreadyAtDesiredCapacity`): the two ends
/// of the registered window the capacity may not be moved past, and the capacity the policy
/// asked for already being the one the target holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WithheldReason {
    /// The capacity asked for is above the registered `MaxCapacity`, which the target
    /// already sits at.
    Ceiling,
    /// The capacity asked for is below the registered `MinCapacity`, which the target
    /// already sits at.
    Floor,
    /// The capacity asked for is the one the target already holds.
    Unchanged,
}

impl WithheldReason {
    /// The `NotScaledReason$Code` value this reason is reported as.
    fn code(self) -> &'static str {
        match self {
            Self::Ceiling => "AlreadyAtMaxCapacity",
            Self::Floor => "AlreadyAtMinCapacity",
            Self::Unchanged => "AlreadyAtDesiredCapacity",
        }
    }

    /// The reason as the activity reports it: the code, the bound that withheld the movement
    /// and the capacity the target was found at.
    ///
    /// Only the bound this reason is ABOUT is named — the other end of the window had no part
    /// in withholding the movement, and `NotScaledReason`'s members are each optional.
    fn reported(self, min_capacity: i32, max_capacity: i32, current: i32) -> NotScaledReason {
        NotScaledReason {
            code: self.code().to_string(),
            max_capacity: matches!(self, Self::Ceiling).then_some(max_capacity),
            min_capacity: matches!(self, Self::Floor).then_some(min_capacity),
            current_capacity: Some(current),
        }
    }
}

/// The capacity movement one breach asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Movement {
    /// The capacity to move the target to.
    capacity: i32,
    /// The capacity this movement was computed FROM: the one the target held as of the scaling
    /// decision this breach belongs to, which is what the movements of that one decision are
    /// all computed from, so two of them can never accumulate.
    base: i32,
    /// How long the decision this movement settles is the one a policy triggered with it is
    /// compared against — this policy's own wait for a previous scaling activity to take
    /// effect ([`cooldown_of`]).
    settled_for: Duration,
}

/// What this pass would do with one breach, from the store as it stands.
///
/// Read-only, so the same decision can be taken twice: once to arbitrate the movements the
/// policies of one target ask for against each other, and once to perform the one that won.
fn plan_breach(
    store: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    policy: &FiringPolicy,
    breach: &Breach,
    resolved: &BTreeMap<TargetKey, i32>,
    now: SystemTime,
    windows: &BTreeMap<TargetKey, SystemTime>,
) -> Disposition {
    // The policy may have been deleted, and the target deregistered, since the snapshot.
    let Some(record) = store.policies.get(&policy.key) else {
        return Disposition::Hold;
    };
    // What this policy has been executed for is read AGAIN under this lock: a pass that
    // overlapped this one may have performed the same breach between the snapshot and
    // here, and one transition is ONE execution.
    if record
        .executed_transitions
        .get(&breach.alarm_name)
        .is_some_and(|executed| *executed >= breach.transitioned_secs)
    {
        return Disposition::Hold;
    }
    // Which of a target-tracking policy's own alarms this breach is of, which is what
    // names the direction it drives before any arithmetic: that policy is executed by
    // the pair it minted on the metric it tracks, one alarm per direction.
    let tracked_kind = match &policy.config {
        FiringConfig::Step(_) => None,
        FiringConfig::TargetTracking(_) => {
            match super::policy_alarm_kind(&policy.key, &breach.alarm_name) {
                Some(kind) if !kind.watches_capacity() => Some(kind),
                // The pair that watches the provisioned capacity reports a stale
                // threshold rather than drive a direction, and an alarm this policy did
                // not mint states no target for a metric at all: neither moves capacity.
                _ => return Disposition::Consume,
            }
        }
    };
    // A target whose policies are arbitrated against each other performs nothing while the
    // window they are collected over is still open.
    if windows
        .get(&policy.key.target)
        .is_some_and(|closes| now < *closes)
    {
        return Disposition::Hold;
    }
    // "The amount of time, in seconds, to wait for a previous scaling activity to take
    // effect": a breach inside that span of this policy's own last execution WAITS — the
    // alarm is still breaching, so the breach is left unperformed rather than consumed,
    // and the first pass after the wait is over performs it.
    let waiting = record
        .last_executed
        .and_then(|last| last.checked_add(cooldown_of(record, tracked_kind)))
        .is_some_and(|due| now < due);
    if waiting {
        return Disposition::Hold;
    }
    let Some(target) = store.targets.get(&policy.key.target) else {
        return Disposition::Hold;
    };
    // The window as a WINDOW, whichever order the two bounds are stored in: the capacity
    // is placed "inside of this range", and a floor above the ceiling is no range at all,
    // so it is read as the point the two meet at rather than asserted against. Nothing in
    // this pass may depend on a stored invariant it did not establish itself.
    let (min_capacity, max_capacity) = (
        target.min_capacity.min(target.max_capacity),
        target.max_capacity.max(target.min_capacity),
    );
    let current = resolved
        .get(&policy.key.target)
        .copied()
        .unwrap_or(target.current_capacity);
    // The scaling decision this target's policies have already settled, while it is still the
    // one a policy triggered with them is compared against, and while it was taken by ANOTHER
    // policy — this policy's own breaches wait for its own cooldown above, which is the span
    // that decision stands for.
    let settled = target
        .settled_arbitration
        .as_ref()
        .filter(|settled| now < settled.open_until)
        .filter(|settled| settled.policy_name != policy.key.policy_name);
    // What this breach asks a movement OF: the capacity the target held as of the decision its
    // trigger belongs to. "Application Auto Scaling gives precedence to the policy that
    // provides the largest capacity" — so a policy whose trigger is read after the decision was
    // taken still asks for a movement of the capacity that decision was taken from, and the
    // capacity the winner already moved is part of what it asked for rather than something to
    // add to.
    let base = settled.map_or(current, |settled| settled.base_capacity);

    // The capacity the policy asks for, as its OWN configuration states it.
    let wanted = match &policy.config {
        FiringConfig::Step(config) => adjusted_capacity(config, breach, base),
        FiringConfig::TargetTracking(config) => {
            super::target_tracking_capacity(&policy.key, config, breach.measured(), base)
        }
    };
    let Some(wanted) = wanted else {
        // The configuration names no capacity for this breach, so the transition is
        // evaluated and done with.
        return Disposition::Consume;
    };
    let capacity = wanted.max(min_capacity).min(max_capacity);
    if capacity == current {
        // The decision asks for no movement at all: either the policy asked for the
        // capacity the target already holds, or it asked PAST a bound the target already
        // sits at — "Application Auto Scaling will not scale capacity to values that are
        // outside of the minimum and maximum range" — so the registered window withheld it.
        // Nothing moves, and the reason the model's `NotScaledReason$Code` names is what the
        // log says about the decision.
        let reason = if wanted > max_capacity {
            WithheldReason::Ceiling
        } else if wanted < min_capacity {
            WithheldReason::Floor
        } else {
            WithheldReason::Unchanged
        };
        return Disposition::NotScaled {
            reason,
            // A decision a BOUND withheld is left unconsumed: the alarm is still breaching,
            // so the first pass after that bound is raised delivers the movement it was
            // owed. One the metric itself asked nothing of is done with — only a new
            // measurement, which arrives as a new transition, could change it.
            consume: reason == WithheldReason::Unchanged,
        };
    }
    // A policy that does not provide MORE capacity than the decision its target has already
    // settled is the one precedence is given against: it is fully suppressed, so its
    // transition is consumed and it moves no capacity and records no activity of its own —
    // one scalable target was moved once, by the policy that provided the largest capacity.
    if settled.is_some_and(|settled| capacity <= settled.settled_capacity) {
        return Disposition::Consume;
    }
    // "DynamicScalingInSuspended […] Whether scale in by a target tracking scaling policy
    // or a step scaling policy is suspended" — and the same for scale out: a suspended
    // direction performs no movement at all. The breach is WITHHELD, not consumed: the
    // alarm is still breaching, so the first pass after the suspension is lifted delivers
    // the movement it was owed, with no new metric movement of its own.
    let scaling_out = capacity > current;
    let direction_suspended = target.suspended_state.as_ref().is_some_and(|suspended| {
        if scaling_out {
            suspended.dynamic_scaling_out_suspended == Some(true)
        } else {
            suspended.dynamic_scaling_in_suspended == Some(true)
        }
    });
    if direction_suspended {
        return Disposition::Hold;
    }
    Disposition::Move(Movement {
        capacity,
        base,
        settled_for: cooldown_of(record, tracked_kind),
    })
}

/// Perform one movement: consume the transition, set the capacity, record the activity — which
/// is what queues the capacity the resource is owed — record the scaling decision it settles for
/// the target's policies, and start this policy's cooldown.
fn perform_movement(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    policy: &FiringPolicy,
    breach: &Breach,
    movement: Movement,
    now: SystemTime,
) {
    let Movement {
        capacity,
        base,
        settled_for,
    } = movement;
    // This transition has now been performed, so no later pass performs it again.
    remember_transition(store, &policy.key, &breach.alarm_name, breach.transitioned_secs);
    let cause = breach_cause(policy, breach);
    // The instant THIS movement is made at, which is not the instant the pass started:
    // the pass performs the breaches it found one after another, and two movements that
    // share an instant have no order in time at all — so the one a later movement
    // displaces could not be told from the one that displaced it. Never earlier than the
    // instant the pass read, so the order the pass performs movements in is the order
    // their instants are in.
    let moved_at = SystemTime::now().max(now);
    store.record_capacity_change(&policy.key.target, capacity, cause, moved_at);
    if let Some(target) = store.targets.get_mut(&policy.key.target) {
        target.current_capacity = capacity;
        // The scaling decision this target's policies have now settled: recorded AFTER the
        // movement, which clears the decision an earlier capacity was settled against, and
        // carrying the capacity that decision was taken FROM — so a policy of this target
        // whose trigger is read later is compared against this one instead of adding to it.
        target.settled_arbitration = moved_at
            .checked_add(settled_for)
            .map(|open_until| super::SettledArbitration {
                base_capacity: base,
                settled_capacity: capacity,
                policy_name: policy.key.policy_name.clone(),
                open_until,
            });
    }
    if let Some(record) = store.policies.get_mut(&policy.key) {
        record.last_executed = Some(moved_at);
    }
}

/// The `Cause` of every activity one alarm breach produces: "a simple description of what
/// caused the scaling activity to happen", which is the alarm that moved into `ALARM` and the
/// policy its action list named. The same cause whether the decision moved capacity or was
/// withheld — the breach is what caused the service to decide at all.
fn breach_cause(policy: &FiringPolicy, breach: &Breach) -> String {
    format!(
        "monitor alarm {} in state ALARM triggered policy {}",
        breach.alarm_name, policy.key.policy_name
    )
}

/// What this service DESCRIBES a decision taken on an alarm breach as, for a decision that
/// moved no capacity: the fixed text a not-scaled activity carries, which names the trigger
/// rather than a movement — there is none to name.
const NOT_SCALED_DESCRIPTION: &str = "Attempting to scale due to alarm triggered";

/// Record the decision a breach produced no movement for as the not-scaled activity the
/// model describes: `StatusCode` `Failed`, the fixed description, and the reason code beside
/// the bound that withheld the movement and the capacity the target was found at.
///
/// A breach that keeps breaching is decided again by every pass, so the SAME reason is
/// recorded once: the entry is added only when it is not what the target's newest activity
/// already says. A movement of the target since, or a decision withheld for another reason,
/// is a new thing to say about it and is recorded.
fn record_not_scaled(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    policy: &FiringPolicy,
    breach: &Breach,
    reason: WithheldReason,
    now: SystemTime,
) {
    let Some(target) = store.targets.get(&policy.key.target) else {
        return;
    };
    let reported = reason.reported(
        target.min_capacity.min(target.max_capacity),
        target.max_capacity.max(target.min_capacity),
        target.current_capacity,
    );
    let already_said = store
        .latest_not_scaled_reason(&policy.key.target)
        .is_some_and(|held| {
            held.code == reported.code
                && held.max_capacity == reported.max_capacity
                && held.min_capacity == reported.min_capacity
                && held.current_capacity == reported.current_capacity
        });
    if already_said {
        return;
    }
    let capacity = reported.current_capacity.unwrap_or_default();
    store.record_activity(
        &policy.key.target,
        SystemTime::now().max(now),
        super::ActivityEntry {
            description: NOT_SCALED_DESCRIPTION.to_string(),
            cause: breach_cause(policy, breach),
            capacity,
            status_code: super::ACTIVITY_FAILED.to_string(),
            status_message: None,
            not_scaled_reasons: vec![reported],
        },
    );
}

/// When each target's arbitration window closes, for the targets whose breaches are arbitrated
/// at all.
///
/// Two policies are triggered "at the same time" when the alarms that trigger them move into
/// `ALARM` inside one window, so the window is measured from the OLDEST transition this pass
/// found for the target: every policy triggered inside it is then decided in one pass against
/// the others, rather than one policy per pass as the alarms happen to be read.
///
/// Only a target with more than one policy an alarm can fire has a window: with a single policy
/// there is no second movement to give precedence to, so its breach is performed as soon as a
/// pass finds it. And a target every one of whose alarm-driven policies this pass holds a breach
/// of has none either: the movements they ask for are ALL here to be compared, so there is
/// nothing left for the window to collect and waiting it out would only delay the one movement
/// they settle on.
fn arbitration_windows(
    store: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    fired: &[(FiringPolicy, Breach)],
) -> BTreeMap<TargetKey, SystemTime> {
    // Which policies of each target this pass holds a breach of, which is what says how much of
    // that target's own set of alarm-driven policies has been collected.
    let mut triggered: BTreeMap<TargetKey, BTreeSet<PolicyKey>> = BTreeMap::new();
    for (policy, _) in fired {
        triggered
            .entry(policy.key.target.clone())
            .or_default()
            .insert(policy.key.clone());
    }
    let mut windows: BTreeMap<TargetKey, SystemTime> = BTreeMap::new();
    for (policy, breach) in fired {
        let alarm_driven = alarm_driven_policies(store, &policy.key.target);
        if alarm_driven < 2 {
            continue;
        }
        if triggered
            .get(&policy.key.target)
            .is_some_and(|keys| keys.len() >= alarm_driven)
        {
            continue;
        }
        let Some(closes) =
            super::instant_of_epoch_secs(breach.transitioned_secs).checked_add(ARBITRATION_WINDOW)
        else {
            continue;
        };
        windows
            .entry(policy.key.target.clone())
            .and_modify(|held| *held = (*held).min(closes))
            .or_insert(closes);
    }
    windows
}

/// How many policies of `target` an alarm breach can be executed on at all, which is how many
/// movements of it could conflict.
fn alarm_driven_policies(
    store: &ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    target: &TargetKey,
) -> usize {
    store
        .policies
        .iter()
        .filter(|(key, record)| &key.target == target && is_alarm_driven(&record.policy_type))
        .count()
}

/// The breach of each target to perform: the index into `fired` of the movement that yields the
/// largest resulting capacity, for a target more than one POLICY asks a movement of.
///
/// A target only one policy asks a movement of gets no entry — there is nothing to give
/// precedence over, so nothing is suppressed. Ties keep the first of the pass's own total order
/// over (policy, alarm), so the arbitration is deterministic rather than decided by how the
/// alarm store happens to enumerate.
fn arbitrate(
    fired: &[(FiringPolicy, Breach)],
    planned: &[Disposition],
) -> BTreeMap<TargetKey, usize> {
    let mut best: BTreeMap<TargetKey, (i32, usize)> = BTreeMap::new();
    let mut asking: BTreeMap<TargetKey, BTreeSet<PolicyKey>> = BTreeMap::new();
    for (index, ((policy, _), disposition)) in fired.iter().zip(planned).enumerate() {
        let Disposition::Move(Movement { capacity, .. }) = *disposition else {
            continue;
        };
        asking
            .entry(policy.key.target.clone())
            .or_default()
            .insert(policy.key.clone());
        best.entry(policy.key.target.clone())
            .and_modify(|held| {
                if capacity > held.0 {
                    *held = (capacity, index);
                }
            })
            .or_insert((capacity, index));
    }
    best.into_iter()
        .filter(|(target, _)| asking.get(target).is_some_and(|keys| keys.len() > 1))
        .map(|(target, (_, index))| (target, index))
        .collect()
}

/// The wait a policy's own configuration gives a breach after this policy's last execution.
///
/// One cooldown per policy type, and for target tracking one per DIRECTION: "The amount of
/// time, in seconds, to wait for a previous scale-out activity to take effect"
/// (`TargetTrackingScalingPolicyConfiguration$ScaleOutCooldown`) and "The amount of time, in
/// seconds, after a scale-in activity completes before another scale-in activity can start"
/// (`$ScaleInCooldown`), against the one `StepScalingPolicyConfiguration$Cooldown` a step
/// policy carries for both. A member the request left out falls back to the only default the
/// model states for a cooldown, [`DEFAULT_COOLDOWN_SECS`]; the target-tracking members
/// document theirs as a per-service table outside the model, which this emulator has no
/// recording of.
fn cooldown_of(
    record: &super::ScalingPolicyRecord,
    tracked_kind: Option<super::AlarmKind>,
) -> Duration {
    let configured = match tracked_kind {
        Some(kind) => record.target_tracking_config.as_ref().and_then(|config| {
            if kind.scales_out() {
                config.scale_out_cooldown
            } else {
                config.scale_in_cooldown
            }
        }),
        None => record.step_config.as_ref().and_then(|config| config.cooldown),
    };
    Duration::from_secs(
        configured
            .map_or(DEFAULT_COOLDOWN_SECS, i64::from)
            .max(0)
            .unsigned_abs(),
    )
}

/// Record on a policy that it has been executed for the transition of `alarm_name` at
/// `transitioned_secs`, forgetting the oldest transition it remembers once the memory is full.
fn remember_transition(
    store: &mut ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState,
    key: &PolicyKey,
    alarm_name: &str,
    transitioned_secs: f64,
) {
    let Some(record) = store.policies.get_mut(key) else {
        return;
    };
    record
        .executed_transitions
        .insert(alarm_name.to_string(), transitioned_secs);
    while record.executed_transitions.len() > MAX_REMEMBERED_ALARMS_PER_POLICY {
        let Some(oldest) = record
            .executed_transitions
            .iter()
            .min_by(|(left_name, left), (right_name, right)| {
                left.total_cmp(right).then_with(|| left_name.cmp(right_name))
            })
            .map(|(name, _)| name.clone())
        else {
            break;
        };
        record.executed_transitions.remove(&oldest);
    }
}

/// The capacity a step scaling policy's configuration asks for, from the capacity the resource
/// holds now and the breach that fired it — before the registered window is applied to it.
///
/// `None` when the configuration names no adjustment for this breach: no step whose interval
/// holds it, or no `AdjustmentType` to interpret the step by ("AdjustmentType is required if
/// you are adding a new step scaling policy configuration").
fn adjusted_capacity(
    config: &StepScalingPolicyConfiguration,
    breach: &Breach,
    current: i32,
) -> Option<i32> {
    let adjustment_type = config.adjustment_type.as_deref()?;
    let adjustment = applicable_adjustment(config, breach)?;
    let capacity = match adjustment_type {
        // "A positive value adds to the current capacity while a negative number removes from
        // the current capacity."
        "ChangeInCapacity" => i64::from(current) + i64::from(adjustment),
        // "For exact capacity, you must specify a non-negative value" — the adjustment IS the
        // capacity.
        "ExactCapacity" => i64::from(adjustment),
        "PercentChangeInCapacity" => {
            i64::from(current) + percentage_magnitude(config, current, adjustment)
        }
        // Not one of the modelled `AdjustmentType` values, so it interprets no step.
        _ => return None,
    };
    Some(capacity.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

/// The `ScalingAdjustment` of the step whose interval the breach falls in.
///
/// A step's interval is of the DIFFERENCE between the metric and the alarm's threshold, and
/// which end is inclusive follows the direction of the breach: above the threshold "the lower
/// bound is inclusive (the metric must be greater than or equal to the threshold plus the lower
/// bound)" and the upper bound is exclusive; below it the lower bound is exclusive and the
/// upper bound inclusive. A null bound is unbounded in that direction, and so is one that is
/// not a finite number — nothing can be compared against it.
fn applicable_adjustment(
    config: &StepScalingPolicyConfiguration,
    breach: &Breach,
) -> Option<i32> {
    let finite = |bound: Option<f64>| bound.filter(|value| value.is_finite());
    config
        .step_adjustments
        .iter()
        .flatten()
        .find(|step| {
            let lower = finite(step.metric_interval_lower_bound).unwrap_or(f64::NEG_INFINITY);
            let upper = finite(step.metric_interval_upper_bound).unwrap_or(f64::INFINITY);
            if breach.above {
                breach.difference >= lower && breach.difference < upper
            } else {
                breach.difference > lower && breach.difference <= upper
            }
        })
        .map(|step| step.scaling_adjustment)
}

/// How much a `PercentChangeInCapacity` step moves the capacity by.
///
/// The percentage is OF the capacity the resource holds, so the number of units it comes to is
/// a fraction: it is taken toward zero, and never to nothing — a non-zero percentage moves the
/// capacity by at least one unit in the direction it names, or the policy would report a
/// movement it never made. `MinAdjustmentMagnitude` raises that floor: "The minimum value to
/// scale by when the adjustment type is PercentChangeInCapacity. For example, suppose that you
/// create a step scaling policy to scale out an Amazon ECS service by 25 percent and you
/// specify a MinAdjustmentMagnitude of 2. If the service has 4 tasks and the scaling policy is
/// performed, 25 percent of 4 is 1. However, because you specified a MinAdjustmentMagnitude of
/// 2, Application Auto Scaling scales out the service by 2 tasks."
fn percentage_magnitude(
    config: &StepScalingPolicyConfiguration,
    current: i32,
    adjustment: i32,
) -> i64 {
    if adjustment == 0 {
        return 0;
    }
    let exact = f64::from(current) * f64::from(adjustment) / 100.0;
    let floor = i64::from(config.min_adjustment_magnitude.unwrap_or(1).max(1));
    let magnitude = (exact.abs().trunc() as i64).max(floor);
    magnitude * i64::from(adjustment.signum())
}

/// The capacity the service that OWNS the scaled resource holds for the property a scalable
/// dimension names, or `None` when this emulator cannot read it.
///
/// "Application Auto Scaling retrieves the current capacity of the resource": an adjustment is
/// computed from the resource's OWN capacity, not from what this service last set. Read
/// through the one resolver a registration validates the resource with
/// ([`super::resolve_target_resource`]), in the target's own Region, so both see the same
/// capacity; for a namespace whose resource store this emulator does not hold — and for a
/// resource that resolver refuses — there is nothing to read, and the capacity this service
/// tracks for the target is what the adjustment is computed from instead.
pub fn resource_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &TargetKey,
) -> Option<i32> {
    super::resolve_target_resource(engine, target)
        .ok()
        .flatten()
        .and_then(|capacity| i32::try_from(capacity).ok())
}

/// Set the capacity of the resource a scalable target scales, in the service that owns it, and
/// answer whether that service holds the resource at all.
///
/// Application Auto Scaling does not hold the capacity of anything: it moves the resource's own
/// capacity through the owning service ("Application Auto Scaling scales […] the ECS service's
/// desired count"), which is what a client reads back there. One arm per namespace whose
/// resource store this emulator holds AND whose resources a policy of this kind can move; a
/// namespace whose resource is reached over the wire is written by
/// [`deliver_remote_capacity`] instead, and for any other namespace the movement is recorded
/// as the scaling activity it is and the capacity this service tracks for the target is all
/// there is to move.
fn set_resource_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    target: &TargetKey,
    capacity: i32,
) -> bool {
    match target.service_namespace.as_str() {
        "ecs" => set_ecs_desired_count(engine, region, &target.resource_id, capacity),
        "dynamodb" => set_dynamodb_capacity(engine, region, target, capacity),
        _ => false,
    }
}

/// Set the provisioned read or write capacity of the DynamoDB table, or global secondary
/// index, a `dynamodb` `ResourceId` names.
///
/// This is what the four `dynamodb:*:*CapacityUnits` dimensions scale, and the bookkeeping a
/// capacity move owes is the DynamoDB store's own, so it is that store's helper that applies
/// it ([`dynamodb::state::ComAmazonawsDynamodbDynamodb20120810State::scale_provisioned_capacity`]) —
/// the same one an `UpdateTable` that moves capacity goes through.
fn set_dynamodb_capacity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    target: &TargetKey,
    capacity: i32,
) -> bool {
    use ::service_com_amazonaws_dynamodb_dynamodb_20120810 as dynamodb;

    let Some((table_name, index_name)) = super::split_dynamodb_resource_id(&target.resource_id)
    else {
        return false;
    };
    let Some(store) = dynamodb::state::service_state(engine) else {
        return false;
    };
    let reads = super::dimension_scaling_property(&target.scalable_dimension) == "ReadCapacityUnits";
    let moved = store.lock().scale_provisioned_capacity(
        region,
        table_name,
        index_name,
        reads,
        i64::from(capacity),
        SystemTime::now(),
    );
    moved
}

/// Set the `desiredCount` of the ECS service an `ecs` `ResourceId` names, and put the ECS
/// scheduler to work on the task count it now wants.
///
/// This is what `ecs:service:DesiredCount` scales: "The task count of an ECS service". The
/// bookkeeping a count change owes is the ECS scheduler's own, so it is the ECS store's own
/// helper that does it ([`ecs::state::ComAmazonawsEcsAmazonec2containerservicev20141113State::scale_service_to`]),
/// the one a client's `UpdateService` calls for a request that changes that count.
///
/// Where this differs from `UpdateService`, and why: that handler REFUSES the whole request and
/// restores the service when the placement would push this emulator past its own task ceiling,
/// because it has a caller to answer. An alarm breach has none — the capacity was already
/// decided by the policy and recorded as a scaling activity — so the count stands and the
/// ceiling bounds only what the scheduler places, which the service's own event stream records.
fn set_ecs_desired_count<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_id: &str,
    capacity: i32,
) -> bool {
    use ::service_com_amazonaws_ecs_amazonec2containerservicev20141113 as ecs;

    let Some((cluster, service_name)) = super::split_ecs_resource_id(resource_id) else {
        return false;
    };
    let Some(store) = ecs::state::service_state(engine) else {
        return false;
    };
    let mut guard = store.lock();
    let key = (cluster.to_string(), service_name.to_string());
    match guard.scale_service_to(&key, capacity, region, SystemTime::now()) {
        Ok(moved) => moved,
        Err(_) => {
            ::tracing::warn!(
                cluster = %cluster,
                service = %service_name,
                desired_count = capacity,
                "the ECS service's desired count was set, but this emulator's task ceiling \
                 left the scheduler nothing to place it with"
            );
            true
        }
    }
}
