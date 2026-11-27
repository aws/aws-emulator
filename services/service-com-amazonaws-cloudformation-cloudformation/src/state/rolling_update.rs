// SPDX-License-Identifier: CC0-1.0

//! The UPDATE POLICY that asks for an Auto Scaling group's INSTANCES to be replaced a BATCH
//! at a time, and the state one such replacement is carried through in.
//!
//! ## The rule this module implements
//!
//! A template states how an update treats an Auto Scaling group with the `UpdatePolicy`
//! resource attribute, and one of its policies — `AutoScalingRollingUpdate` — asks for the
//! group's INSTANCES to be replaced rather than for the group to be: CloudFormation
//! terminates at most `MaxBatchSize` of the group's instances at a time, lets the group launch
//! their replacements from the configuration the update put in force, and starts the next
//! batch only once that batch has come through — never taking the group below
//! `MinInstancesInService` while it does. With `WaitOnResourceSignals` the batch is not
//! through when the replacements are running: each of them has to SIGNAL
//! ([`crate::ops::signal_resource`]) within `PauseTime`, and a batch short of the share
//! `MinSuccessfulInstancesPercent` asks for fails the update — which is then rolled back to
//! the configuration it started from. Without it, `PauseTime` is what the policy documents it
//! as there: the span CloudFormation pauses after each batch.
//!
//! So an update of such a group is not one call to the owning service: it is that call
//! followed by one batch per `MaxBatchSize` instances, each of which the stack's operation
//! stays `UPDATE_IN_PROGRESS` across. Each batch is itself in two halves — its instances leave
//! service BEFORE their replacements are launched, and the group holds the capacity they left
//! for the span a replacement takes to come up ([`BATCH_REPLACEMENT`]) — and that is what makes
//! the replacement OBSERVABLE through the owning service's own reads: a client polling the
//! group sees the in-service count dip by at most `MaxBatchSize`, held there long enough to be
//! read, never below `MinInstancesInService` and never above the group's desired capacity. It
//! is also what makes a bad revision catchable: nothing is replaced past the batch whose
//! instances reported their own bootstrap failed.
//!
//! This module is the MODEL of that: what the policy states ([`RollingUpdatePolicy`]), which
//! of a deployment's resources carry one ([`declared`]), the state one replacement in flight
//! is carried in ([`RollingUpdate`]) and the one step a pass of it owes
//! ([`RollingUpdate::next_step`]). Nothing here calls the owning service: the calls are
//! [`crate::state::provisioning::rolling_update`]'s, which is the one place this emulator
//! reaches another service from.
//!
//! The policy that asks for the GROUP ITSELF to be replaced is the other one
//! ([`super::replacing_update`]), and the two are mutually exclusive on one update: a group
//! being replaced has no instances of its own to roll.
//!
//! ## Bounds
//!
//! One record per Auto Scaling group of the operation in flight whose declaration carries the
//! policy, so the set is bounded by the resources a stack may hold
//! ([`super::MAX_CONCURRENT_RESOURCES`]), and each record holds at most the instance ids the
//! owning service lets one group have. Every record is dropped when the operation that opened
//! it concludes ([`super::StoredStack::begin_transition`]).

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use super::template;
use super::StoredResource;

/// The resource attribute the policies of an update live under.
const UPDATE_POLICY: &str = "UpdatePolicy";

/// The policy that asks for the group's instances to be replaced a batch at a time: "to
/// specify how CloudFormation handles rolling updates for an Auto Scaling group, use the
/// `AutoScalingRollingUpdate` policy".
const AUTOSCALING_ROLLING_UPDATE: &str = "AutoScalingRollingUpdate";

/// How many instances one batch replaces when the policy states no `MaxBatchSize`: "the
/// maximum number of instances that CloudFormation updates. Default: 1".
const DEFAULT_MAX_BATCH_SIZE: usize = 1;

/// How many instances stay in service when the policy states no `MinInstancesInService`: "the
/// minimum number of instances that must be in service within the Auto Scaling group while
/// CloudFormation updates old instances. Default: 0".
const DEFAULT_MIN_INSTANCES_IN_SERVICE: usize = 0;

/// The share of one batch that must signal success when the policy states no
/// `MinSuccessfulInstancesPercent`: "the percentage of instances that must signal success for
/// an update to succeed. Default: 100".
const DEFAULT_MIN_SUCCESSFUL_INSTANCES_PERCENT: u64 = 100;

/// How long a batch is given when the policy states no `PauseTime` and asks for signals: "the
/// amount of time that CloudFormation pauses after making a change to a batch of instances …
/// Default: PT0S (zero seconds). If the `WaitOnResourceSignals` property is set to true, the
/// default is PT5M."
const DEFAULT_SIGNALLED_PAUSE_TIME: Duration = Duration::from_secs(300);

/// How long ONE batch of a replacement takes to bring its replacements up: the span between
/// the batch's instances leaving service and their replacements being launched.
///
/// A batch is a COMPOSITE of moves another service makes — the batch's instances go out of
/// service, the group launches a replacement for each, and each replacement comes into service
/// — and the whole point of the policy is that a client watching the group sees it happen:
/// "the minimum number of instances that must be in service … while CloudFormation updates old
/// instances" is a floor a reader can hold the update to only while there is something to
/// read. So the group holds the capacity the batch took out of service for this span, which is
/// the dip the floor is a floor under.
///
/// The value is CHOSEN, not an AWS-published figure, and is never slept for: it is stored as
/// the instant the batch's replacements come due and materialised by whichever pass finds it
/// past. It is the span this emulator gives a transition the CLOCK — rather than a caller —
/// brings about, which is what a batch is: the composite of the moves above, longer than any
/// one of the single lifecycle spans the owning service gives them, so a client polling the
/// group gets several looks at each batch rather than finding the whole group swapped between
/// two of them. A replacement of a group therefore takes about one such span per batch. Its
/// CEILING is the window the batch itself is bounded by ([`RollingUpdate::batch_deadline`]):
/// the span the policy gives a batch to come through ("`PauseTime` … the amount of time that
/// CloudFormation should wait for the Auto Scaling group to receive the required number of
/// signals") has to outlast it, or no batch could ever come through.
pub const BATCH_REPLACEMENT: Duration = Duration::from_secs(3);

/// What one `AutoScalingRollingUpdate` states, with every member a declaration leaves out at
/// the default that member documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollingUpdatePolicy {
    /// `MaxBatchSize`: "the maximum number of instances that CloudFormation updates".
    pub max_batch_size: usize,
    /// `MinInstancesInService`: "the minimum number of instances that must be in service
    /// within the Auto Scaling group while CloudFormation updates old instances".
    pub min_instances_in_service: usize,
    /// `MinSuccessfulInstancesPercent`: "the percentage of instances that must signal success
    /// for an update to succeed".
    pub min_successful_instances_percent: u64,
    /// `PauseTime`: the span CloudFormation pauses after a batch, which is the span it waits
    /// for that batch's SIGNALS in where the policy asks for them.
    pub pause_time: Duration,
    /// `WaitOnResourceSignals`: "specifies whether the Auto Scaling group waits on signals
    /// from new instances during an update".
    pub wait_on_resource_signals: bool,
}

impl RollingUpdatePolicy {
    /// The policy the declaration of `declared` states, or `None` where it states none.
    ///
    /// Read off the resource ATTRIBUTE, which is where the policies of an update live, and
    /// only for the type that attribute's policy governs — an Auto Scaling group.
    pub fn declared(declared: &template::DeclaredResource) -> Option<Self> {
        if declared.resource_type != template::AUTOSCALING_GROUP {
            return None;
        }
        let policy = declared
            .declaration
            .get(UPDATE_POLICY)?
            .get(AUTOSCALING_ROLLING_UPDATE)?;
        let wait_on_resource_signals = policy
            .get("WaitOnResourceSignals")
            .is_some_and(template::flag);
        Some(Self {
            max_batch_size: whole_number(policy.get("MaxBatchSize"))
                .and_then(|size| usize::try_from(size).ok())
                .filter(|size| *size > 0)
                .unwrap_or(DEFAULT_MAX_BATCH_SIZE),
            min_instances_in_service: whole_number(policy.get("MinInstancesInService"))
                .and_then(|size| usize::try_from(size).ok())
                .unwrap_or(DEFAULT_MIN_INSTANCES_IN_SERVICE),
            min_successful_instances_percent: whole_number(
                policy.get("MinSuccessfulInstancesPercent"),
            )
            .unwrap_or(DEFAULT_MIN_SUCCESSFUL_INSTANCES_PERCENT)
            .clamp(1, 100),
            pause_time: pause_time(policy.get("PauseTime"), wait_on_resource_signals),
            wait_on_resource_signals,
        })
    }

    /// How many of `replaced` instances must have signalled success for the batch that
    /// replaced them to count as through: the share `MinSuccessfulInstancesPercent` states,
    /// rounded UP, since a percentage of instances is a number of whole instances and the
    /// member is the MINIMUM.
    fn successes_required(&self, replaced: usize) -> usize {
        let required = (replaced as u64)
            .saturating_mul(self.min_successful_instances_percent)
            .div_ceil(100);
        usize::try_from(required).unwrap_or(replaced).min(replaced)
    }
}

/// A whole number a template value states, spelled as a number or as a string — both forms
/// deploy, exactly as they do for every other numeric member of a declaration
/// ([`super::wait_conditions`]).
fn whole_number(declared: Option<&::serde_json::Value>) -> Option<u64> {
    match declared? {
        ::serde_json::Value::Number(number) => number.as_u64(),
        ::serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// `PauseTime` as the declaration states it — "in the ISO8601 duration format" — bounded by
/// the ceiling every span a template asks for is held to
/// ([`super::wait_conditions::MAX_TIMEOUT`]), and defaulted as the member documents: nothing
/// at all, or [`DEFAULT_SIGNALLED_PAUSE_TIME`] where the policy asks for signals.
fn pause_time(declared: Option<&::serde_json::Value>, waits_on_signals: bool) -> Duration {
    let default = if waits_on_signals {
        DEFAULT_SIGNALLED_PAUSE_TIME
    } else {
        Duration::ZERO
    };
    declared
        .and_then(::serde_json::Value::as_str)
        .and_then(::service_common::iso8601_span::iso8601_span)
        .map_or(default, |stated| {
            stated.min(super::wait_conditions::MAX_TIMEOUT)
        })
}

/// One resource of the template being deployed whose declaration carries the policy: the
/// logical id it is declared under and what the policy states.
///
/// Computed where the template is READ ([`super::StoredStack::deploy_template`]), outside the
/// store lock, exactly as the resource-signal waits beside it are
/// ([`super::wait_conditions::declared`]); which of them an operation really has a
/// replacement to carry out for is decided when that operation opens
/// ([`super::StoredStack::begin_transition`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRollingUpdate {
    pub logical_id: String,
    pub policy: RollingUpdatePolicy,
}

/// Every resource of `template` whose declaration carries an `AutoScalingRollingUpdate`.
pub fn declared(template: &template::Template) -> Vec<DeclaredRollingUpdate> {
    template
        .resources
        .iter()
        .filter_map(|declared| {
            RollingUpdatePolicy::declared(declared).map(|policy| DeclaredRollingUpdate {
                logical_id: declared.logical_id.clone(),
                policy,
            })
        })
        .collect()
}

/// One Auto Scaling group whose instances the operation in flight is replacing a batch at a
/// time.
#[derive(Debug, Clone)]
pub struct RollingUpdate {
    /// The logical id the template declares the group under, which is what a signal from one
    /// of its instances names ("when you signal Amazon EC2 instances or Auto Scaling groups,
    /// specify the instance ID that you are signaling as the unique ID") and what the stack's
    /// events report the replacement on.
    pub logical_id: String,
    /// The group's type, which is what its events report it as.
    pub resource_type: String,
    /// The group as Amazon EC2 Auto Scaling addresses it: this resource's physical id.
    pub physical_resource_id: String,
    /// What the declaration's policy states.
    pub policy: RollingUpdatePolicy,
    /// The instances the group was holding when the replacement started — the ones carrying
    /// the configuration this update replaces, which is the set it has to work through. Empty
    /// until the first pass has read the group.
    pub original: BTreeSet<String>,
    /// The instances the batch in flight terminated, which the group is launching replacements
    /// for. Empty while no batch is in flight.
    pub batch: Vec<String>,
    /// The instant the batch in flight was started, which its own span runs from
    /// ([`RollingUpdatePolicy::pause_time`]).
    pub batch_started_at: SystemTime,
    /// The capacity the group was DECLARED with, which is the size the replacement leaves it
    /// at: read off the group when the replacement started.
    pub desired: usize,
    /// Whether the first pass has read the group yet, which is what says [`Self::original`] is
    /// the set it found rather than a group it has not looked at.
    pub started: bool,
    /// Whether every instance of [`Self::original`] has been replaced and its batch come
    /// through: the replacement is over, and the operation's completion is no longer held by
    /// it.
    pub finished: bool,
}

/// How the owning service reports the group a pass is replacing the instances of: what a
/// `DescribeAutoScalingGroups` answer says about it.
#[derive(Debug, Clone, Default)]
pub struct GroupReport {
    /// `AutoScalingGroup$DesiredCapacity`: "the desired size of the group", which is the
    /// number of instances the group holds in service when it is settled.
    pub desired_capacity: usize,
    /// `AutoScalingGroup$MaxSize`: "the maximum size of the group", which is the ceiling a
    /// replacement that has to launch before it terminates is bounded by.
    pub max_size: usize,
    /// Every instance the group reports, with its `Instance$LifecycleState`.
    pub instances: Vec<(String, String)>,
}

impl GroupReport {
    /// The ids of the instances the group reports IN SERVICE: "`InService` — the instance is
    /// running and in service" (`Instance$LifecycleState`).
    fn in_service(&self) -> Vec<&str> {
        self.instances
            .iter()
            .filter(|(_, state)| state == IN_SERVICE)
            .map(|(instance_id, _)| instance_id.as_str())
            .collect()
    }

    /// The ids of every instance the group still holds, whichever lifecycle state it is in: an
    /// instance a batch terminated is reported `Terminating` until it is gone, and the batch
    /// is not through while it is.
    fn held(&self) -> BTreeSet<&str> {
        self.instances
            .iter()
            .map(|(instance_id, _)| instance_id.as_str())
            .collect()
    }
}

/// The lifecycle state an instance serving in the group is in: "`InService` — the instance is
/// running and in service" (`Instance$LifecycleState`).
const IN_SERVICE: &str = "InService";

/// The signals the stack has recorded for the group, as the step below reads them: the unique
/// id of each signal, which for these is the instance's own id.
#[derive(Debug, Clone, Default)]
pub struct RecordedSignals {
    /// The instances that signalled SUCCESS.
    pub succeeded: BTreeSet<String>,
}

/// What one pass of a replacement owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchStep {
    /// Nothing: the batch in flight has not come through yet, so the next one does not start.
    Hold,
    /// Hold the group back from launching and terminate these instances of it, which is what
    /// takes the batch out of service before its replacements are launched.
    Replace(Vec<String>),
    /// Let the group launch again, now that the batch's instances are out of service and the
    /// span their replacements take to come up has passed, and ask it for the capacity it
    /// carries — the one it already holds — so it brings a replacement for each of them up from
    /// the configuration the update put in force.
    LaunchReplacements(usize),
    /// Ask the group to hold this many instances: more than it was declared with, so that it
    /// launches a replacement before an old instance is taken out — which is what a group that
    /// can spare none of its instances above `MinInstancesInService` needs before any batch of
    /// it can start.
    Grow(usize),
    /// Ask the group to hold the capacity it was declared with again, now that every instance
    /// of it carries the new configuration.
    Shrink(usize),
    /// Every instance the replacement found carries the new configuration: it is through.
    Finished,
    /// The replacement cannot be carried through, with the reason the update failed for.
    Failed(String),
}

impl RollingUpdate {
    /// Open the replacement one declaration asks of the operation starting at `now`.
    pub fn open(
        declared: &DeclaredRollingUpdate,
        resource: &StoredResource,
        now: SystemTime,
    ) -> Self {
        Self {
            logical_id: resource.logical_id.clone(),
            resource_type: resource.resource_type.clone(),
            physical_resource_id: resource.physical_resource_id.clone(),
            policy: declared.policy.clone(),
            original: BTreeSet::new(),
            batch: Vec::new(),
            batch_started_at: now,
            desired: 0,
            started: false,
            finished: false,
        }
    }

    /// The one step this replacement owes at `now`, given what the owning service reports
    /// about the group, the signals the stack has recorded for it, and whether the group's
    /// launches are the ones this replacement is holding back
    /// ([`super::StoredStack::withholds_group_launch`]).
    ///
    /// Pure: the caller is what makes the calls the step asks for and what records what they
    /// answered ([`crate::state::provisioning::rolling_update`]), so the decision is made the
    /// same way wherever it is read from. It is asked of a replacement whose first pass has
    /// already recorded what it found ([`super::StoredStack::record_rolling_update_start`]),
    /// which is the set it works through.
    pub fn next_step(
        &self,
        reported: &GroupReport,
        signals: &RecordedSignals,
        launch_withheld: bool,
        now: SystemTime,
    ) -> BatchStep {
        let held = reported.held();
        // The instances of the set this replacement is working through that the group still
        // holds: the ones it has not replaced yet.
        let outstanding: Vec<String> = self
            .original
            .iter()
            .filter(|instance_id| held.contains(instance_id.as_str()))
            .cloned()
            .collect();
        // A batch is in flight while the group still holds an instance it terminated: that
        // instance is reported `Terminating` until it is gone, and its replacement is not
        // launched before it is.
        let batch_in_flight = self
            .batch
            .iter()
            .any(|instance_id| held.contains(instance_id.as_str()));
        let in_service = reported.in_service();
        // How many of the instances this replacement found have been replaced already, which
        // is what the signals the batches have collected are counted against.
        let replaced = self.original.len().saturating_sub(outstanding.len());
        // While the group's launches are this replacement's to release, the batch is between
        // its two halves: its instances have been taken out of service and their replacements
        // are not launched yet, and NOTHING else of the replacement can make progress until
        // the group holds its capacity again. They are asked for once the batch's instances are
        // really gone — "the instances are not terminated immediately"
        // (`TerminateInstanceInAutoScalingGroup`), and a replacement of an instance the group
        // still holds is not a replacement — and once the span a replacement takes to come up
        // has passed ([`BATCH_REPLACEMENT`]), which is the dip this policy's floor is a floor
        // under.
        if launch_withheld {
            if batch_in_flight || !self.replacements_due(now) {
                return self.held_or_expired(now, replaced, signals);
            }
            return BatchStep::LaunchReplacements(reported.desired_capacity);
        }
        if batch_in_flight || !self.batch.is_empty() {
            // The batch is through when the group is back to its desired size with the
            // replacements in service and they have signalled what the policy asks for.
            let settled = !batch_in_flight && in_service.len() >= reported.desired_capacity;
            let signalled = !self.policy.wait_on_resource_signals
                || self.replacement_signals(signals) >= self.policy.successes_required(replaced);
            // "If you enable the `WaitOnResourceSignals` property, `PauseTime` is the amount
            // of time that CloudFormation should wait for the Auto Scaling group to receive
            // the required number of signals"; without it, `PauseTime` is the span
            // CloudFormation "pauses after making a change to a batch of instances to give
            // those instances time to start software applications", so the batch is not
            // through before it has elapsed.
            let paused = self.policy.wait_on_resource_signals || self.pause_elapsed(now);
            if !(settled && signalled && paused) {
                return self.held_or_expired(now, replaced, signals);
            }
        }
        if outstanding.is_empty() {
            // The capacity this replacement borrowed goes back: the group holds what its
            // declaration asks for, and the replacement is through once it does. Read off what
            // the owning service reports rather than off a flag, so a capacity this
            // replacement did not set is put back too.
            if reported.desired_capacity > self.desired {
                return BatchStep::Shrink(self.desired);
            }
            if in_service.len() > self.desired {
                return self.held_or_expired(now, replaced, signals);
            }
            return BatchStep::Finished;
        }
        // "The minimum number of instances that must be in service within the Auto Scaling
        // group while CloudFormation updates old instances": at most that many may be taken
        // out of service at once, and at most `MaxBatchSize` of them whatever the group holds.
        let wanted = self.policy.max_batch_size.min(outstanding.len());
        let spare = in_service
            .len()
            .saturating_sub(self.policy.min_instances_in_service);
        if spare < wanted {
            // The group cannot spare what the batch asks for, so it LAUNCHES first: the
            // capacity goes up — "this value must be less than the `MaxSize` of the Auto
            // Scaling group" is what makes room for it, and the ceiling is the group's own
            // `MaxSize` — and the batch starts once those instances are in service. Asked for
            // while the group is not holding it yet, so a capacity anything else put back is
            // asked for again rather than waited out.
            let room = reported.max_size.saturating_sub(self.desired);
            let capacity = self.desired + (wanted - spare).min(room);
            if capacity > self.desired && reported.desired_capacity < capacity {
                return BatchStep::Grow(capacity);
            }
        }
        let size = wanted.min(spare);
        if size == 0 {
            // The group has nothing spare to take out of service right now: it is still
            // bringing the last batch's replacements up, and the next batch starts once it
            // has.
            return self.held_or_expired(now, replaced, signals);
        }
        BatchStep::Replace(outstanding.into_iter().take(size).collect())
    }

    /// [`BatchStep::Hold`], or the failure a batch that has run out of time earns.
    ///
    /// A batch waiting for SIGNALS runs for the span the policy stated
    /// ([`RollingUpdatePolicy::pause_time`]), which is what "CloudFormation should wait for the
    /// Auto Scaling group to receive the required number of signals" names; one waiting for the
    /// owning service to bring its replacements up is bounded by this emulator's own ceiling on
    /// such a wait ([`super::MAX_HELD_WAIT`]), exactly as every other wait for another service
    /// of it is ([`super::stabilising`]).
    fn held_or_expired(
        &self,
        now: SystemTime,
        replaced: usize,
        signals: &RecordedSignals,
    ) -> BatchStep {
        if self.batch_deadline() > now {
            return BatchStep::Hold;
        }
        if !self.policy.wait_on_resource_signals {
            return BatchStep::Failed(format!(
                "{} {} did not bring the replacements for the instances {} up",
                self.resource_type,
                self.physical_resource_id,
                self.batch.join(", ")
            ));
        }
        let required = self.policy.successes_required(replaced);
        BatchStep::Failed(format!(
            "Failed to receive {} resource signal(s) within the specified duration",
            required.saturating_sub(self.replacement_signals(signals))
        ))
    }

    /// The instant the batch in flight gives up at ([`Self::held_or_expired`]).
    fn batch_deadline(&self) -> SystemTime {
        let window = if self.policy.wait_on_resource_signals {
            self.policy.pause_time
        } else {
            self.policy.pause_time.saturating_add(super::MAX_HELD_WAIT)
        };
        self.batch_started_at
            .checked_add(window)
            .unwrap_or(self.batch_started_at)
    }

    /// Whether the span the batch in flight takes to bring its replacements up has passed
    /// ([`BATCH_REPLACEMENT`]), counted from the instant the batch was taken out of service.
    fn replacements_due(&self, now: SystemTime) -> bool {
        self.batch_started_at
            .checked_add(BATCH_REPLACEMENT)
            .is_none_or(|due| due <= now)
    }

    /// Whether the span the policy asks CloudFormation to PAUSE for after a batch has elapsed,
    /// which is what `PauseTime` states where the policy does not ask for signals.
    fn pause_elapsed(&self, now: SystemTime) -> bool {
        self.batch_started_at
            .checked_add(self.policy.pause_time)
            .is_none_or(|elapsed_at| elapsed_at <= now)
    }

    /// How many of the REPLACEMENTS the batches of this update launched have signalled
    /// success.
    ///
    /// Counted over the instances the stack recorded a signal for that are NOT among the ones
    /// this replacement found: those signalled the operation that created them, and a
    /// replacement's own signal is the one this update waits on.
    fn replacement_signals(&self, signals: &RecordedSignals) -> usize {
        signals
            .succeeded
            .iter()
            .filter(|instance_id| !self.original.contains(*instance_id))
            .count()
    }
}

// ---------------------------------------------------------------------------
// What the stack holds about the replacements of the operation in flight
// ---------------------------------------------------------------------------

impl super::StoredStack {
    /// The replacements the operation now opening has to carry out: one per Auto Scaling group
    /// of the deployment whose declaration carries an `AutoScalingRollingUpdate` and whose
    /// LAUNCH CONFIGURATION this deployment changes in place.
    ///
    /// Only an UPDATE carries one out: the policy "specifies how CloudFormation handles
    /// rolling updates for an Auto Scaling group", and a group this deployment CREATES — one
    /// it adds, and the new group of one it REPLACES
    /// ([`super::replacing_update`]) — has no instances of an earlier configuration to work
    /// through, so its own `CreationPolicy` is the whole of what its create waits at
    /// ([`super::wait_conditions`]). The CHANGES the policy applies to are the ones AWS
    /// documents the two Auto Scaling update policies for — the launch configuration the group
    /// launches from — read through the one place that question is answered
    /// ([`super::replacing_update::changes_the_launch_configuration`]), so this and the
    /// replacing policy cannot disagree about what such a change is.
    pub(super) fn opening_rolling_updates(
        &self,
        now: SystemTime,
        in_progress: &str,
    ) -> Vec<RollingUpdate> {
        if in_progress != super::status::UPDATE_IN_PROGRESS {
            return Vec::new();
        }
        let Some(deployment) = &self.pending_deployment else {
            return Vec::new();
        };
        // The records the stack HAS right now, which is what the declaration being deployed is
        // compared against: a deployment is in flight, so the stack's own records are the
        // configuration in force before it.
        let in_force: Vec<StoredResource> = self
            .resources
            .iter()
            .filter(|deployed| super::provisioning::already_deployed(&deployed.resource))
            .map(|deployed| deployed.resource.clone())
            .collect();
        deployment
            .rolling_updates
            .iter()
            .filter_map(|declared| {
                let resource = deployment
                    .resources
                    .iter()
                    .find(|resource| resource.logical_id == declared.logical_id)?;
                if resource.physical_resource_id.is_empty() {
                    return None;
                }
                // A group this deployment creates, or replaces, is not one whose instances it
                // rolls: the record no longer carries the identity in force.
                if !in_force.iter().any(|existing| {
                    super::provisioning::is_same_resource(existing, resource)
                }) {
                    return None;
                }
                if !super::replacing_update::changes_the_launch_configuration(
                    &deployment.resources,
                    &in_force,
                    &declared.logical_id,
                ) {
                    return None;
                }
                Some(RollingUpdate::open(declared, resource, now))
            })
            .collect()
    }

    /// Whether the operation in flight is still working through one of its replacements, which
    /// holds its terminal status back exactly as an unsatisfied resource-signal wait does
    /// ([`Self::settle`]).
    pub(super) fn rolling_update_in_flight(&self) -> bool {
        self.pending.is_some()
            && self.status == super::status::UPDATE_IN_PROGRESS
            && self.rolling_updates.iter().any(|plan| !plan.finished)
    }

    /// The replacements of the operation in flight that are not through yet, for the pass that
    /// carries them out ([`super::provisioning::rolling_update`]).
    ///
    /// Empty while that operation is still CARRYING OUT its deployment: the change the update
    /// asked the owning service for is what the batches replace the group's instances ONTO, so
    /// nothing is terminated before that call has been made
    /// ([`super::StoredStack::carrying_out_its_deployment`]).
    pub fn open_rolling_updates(&self) -> Vec<RollingUpdate> {
        if !self.rolling_update_in_flight() || self.carrying_out_a_deployment() {
            return Vec::new();
        }
        self.rolling_updates
            .iter()
            .filter(|plan| !plan.finished)
            .cloned()
            .collect()
    }

    /// The signals this stack has recorded for one of its resources, as a replacement reads
    /// them: the unique id of each SUCCESS, which for an instance's own signal is that
    /// instance's id.
    pub fn recorded_signals(&self, logical_id: &str) -> RecordedSignals {
        RecordedSignals {
            succeeded: self
                .signals
                .iter()
                .filter(|signal| {
                    signal.logical_resource_id == logical_id
                        && signal.status == super::resource_signal_status::SUCCESS
                })
                .map(|signal| signal.unique_id.clone())
                .collect(),
        }
    }

    /// Record what the FIRST pass of a replacement found: the instances the group was holding,
    /// which are the ones carrying the configuration this update replaces, and the capacity it
    /// was holding them at, which is the size the replacement leaves it at.
    ///
    /// Recorded once — the set is what the replacement works through, and an instance the
    /// group launched after it started is a REPLACEMENT rather than one of them.
    pub fn record_rolling_update_start(
        &mut self,
        logical_id: &str,
        found: &[String],
        desired: usize,
    ) {
        let Some(plan) = self.open_rolling_update_mut(logical_id) else {
            return;
        };
        if plan.started {
            return;
        }
        plan.original = found.iter().cloned().collect();
        plan.desired = desired;
        plan.started = true;
    }

    /// Record the batch a pass has just CLAIMED, which it is about to take out of service and
    /// which the group then launches replacements for.
    ///
    /// Claimed BEFORE the calls that carry it out, under the same lock the step was decided in:
    /// a pass of this module runs beside every other one (every operation that reports a
    /// stack's progress drives one), and two passes that both decided the same batch would take
    /// more instances out of service than `MaxBatchSize` allows — and could lift the hold on
    /// the group's launches while the other was still taking its batch out. A pass whose calls
    /// are then refused gives the claim back ([`Self::release_rolling_update_batch`]).
    pub fn record_rolling_update_batch(
        &mut self,
        logical_id: &str,
        batch: Vec<String>,
        now: SystemTime,
    ) {
        if let Some(plan) = self.open_rolling_update_mut(logical_id) {
            plan.batch = batch;
            plan.batch_started_at = now;
        }
    }

    /// Give back the batch a pass claimed but could not take out of service, so the next pass
    /// asks for the same one again.
    pub fn release_rolling_update_batch(&mut self, logical_id: &str) {
        if let Some(plan) = self.open_rolling_update_mut(logical_id) {
            plan.batch.clear();
        }
    }

    /// Record that a replacement has worked through every instance it found, so the operation's
    /// completion is no longer held by it.
    pub fn finish_rolling_update(&mut self, logical_id: &str) {
        if let Some(plan) = self.open_rolling_update_mut(logical_id) {
            plan.batch.clear();
            plan.finished = true;
        }
    }

    /// Fail the update because one of its replacements cannot be carried through, and answer
    /// what that failure leaves for the services that own the stack's resources.
    ///
    /// The same report a FAILURE signal makes ([`super::StoredStack::record_resource_signal`]):
    /// the GROUP's own event carries why the replacement stopped, the stack's reason is the
    /// summary naming it, and the update is rolled back to the configuration it started from
    /// unless the caller asked for the failed state to be kept.
    pub fn fail_rolling_update(
        &mut self,
        logical_id: &str,
        reason: &str,
        now: SystemTime,
    ) -> Option<super::wait_conditions::ExpiredWaitOwes> {
        if !self.rolling_update_in_flight() {
            return None;
        }
        let plan = self
            .rolling_updates
            .iter()
            .find(|plan| plan.logical_id == logical_id && !plan.finished)?
            .clone();
        // Nothing of this replacement is owed any longer: the update it belongs to is over.
        self.finish_rolling_update(logical_id);
        let held = self
            .current_resources()
            .into_iter()
            .find(|resource| resource.logical_id == logical_id);
        let resource_properties = held.as_ref().and_then(StoredResource::properties_blob);
        let operation_id = self
            .last_operation()
            .map(|operation| operation.operation_id.clone());
        let client_request_token = self.client_request_token.clone();
        self.record_resource_event(
            now,
            logical_id,
            Some(plan.physical_resource_id.clone()),
            &plan.resource_type,
            super::resource_status::UPDATE_FAILED,
            Some(reason.to_string()),
            resource_properties,
            operation_id,
            client_request_token,
        );
        self.fail_reported_resource_event(
            logical_id,
            super::resource_status::UPDATE_FAILED,
            reason,
        );
        let summary =
            super::resource_failure_summary(super::failed_action::UPDATE, [logical_id]);
        if self.disable_rollback {
            self.fail_transition(now, super::status::UPDATE_FAILED, summary);
            return None;
        }
        self.fail_transition_superseded(now, super::status::UPDATE_FAILED, summary.clone());
        self.begin_owed_update_rollback(now, summary);
        Some(super::wait_conditions::ExpiredWaitOwes::Revert)
    }

    /// Whether the `Launch` process of the group `group` — addressed as that service addresses
    /// it, by the group's name — is one THIS service has suspended for a batch of a rolling
    /// update, and so owes a resumption for.
    ///
    /// What a suspension CloudFormation did not make says about the group is that service's
    /// business, which is why the question is answered off this record rather than off what the
    /// group reports: a `Launch` a client suspended is not one this replacement resumes.
    pub fn withholds_group_launch(&self, group: &str) -> bool {
        self.withheld_group_launches.contains(group)
    }

    /// Record that this service has held the group's launches back.
    pub fn withhold_group_launch(&mut self, group: &str) {
        self.withheld_group_launches.insert(group.to_string());
    }

    /// Record that it no longer does, because the group has been let launch again.
    pub fn release_group_launch(&mut self, group: &str) {
        self.withheld_group_launches.remove(group);
    }

    /// Every group whose launches this service is still holding back although no replacement of
    /// this stack is working through it any more.
    ///
    /// A suspension is CloudFormation's to LIFT whatever became of the operation that made it:
    /// an operation superseded or cancelled while a batch was between its halves would
    /// otherwise leave the group holding its launches back for good, which is a group that
    /// never reaches the capacity it is configured for again. The record outlives the
    /// replacement ([`super::StoredStack::rolling_updates`] is reopened by every operation)
    /// exactly so that this question can be asked.
    pub fn stale_withheld_group_launches(&self) -> Vec<String> {
        if self.withheld_group_launches.is_empty() {
            return Vec::new();
        }
        // A replacement that is still OPEN releases the hold itself, at the instant its batch
        // is owed its replacements ([`RollingUpdate::next_step`]) — so nothing is stale while
        // the operation that made the hold is still in flight and holds a record for that
        // group. Read off the records themselves rather than off the replacements a PASS would
        // carry ([`Self::open_rolling_updates`], which answers nothing while the operation is
        // carrying its deployment out): a hold released while the batch's instances were still
        // leaving the group would launch their replacements early, which is the dip this policy
        // exists to make observable.
        let in_flight = self.rolling_update_in_flight();
        self.withheld_group_launches
            .iter()
            .filter(|group| {
                !in_flight
                    || !self
                        .rolling_updates
                        .iter()
                        .any(|plan| !plan.finished && plan.physical_resource_id == **group)
            })
            .cloned()
            .collect()
    }

    /// The record of one replacement that is still open, reached mutably.
    fn open_rolling_update_mut(&mut self, logical_id: &str) -> Option<&mut RollingUpdate> {
        if !self.rolling_update_in_flight() {
            return None;
        }
        self.rolling_updates
            .iter_mut()
            .find(|plan| plan.logical_id == logical_id && !plan.finished)
    }

    /// Whether the deployment WALK of the operation in flight is still running: the record of
    /// what it asked the owning services for has not come back yet
    /// ([`super::StoredStack::carrying_out_its_deployment`]), so what the group's instances are
    /// to be replaced onto may not be in force there yet.
    fn carrying_out_a_deployment(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.work_in_flight)
    }
}
