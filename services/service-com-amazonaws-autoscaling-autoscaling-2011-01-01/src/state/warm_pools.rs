// SPDX-License-Identifier: CC0-1.0

//! The warm pool of an Auto Scaling group: the pre-initialized instances that sit
//! beside the group, and the rules every reader and writer of them shares.
//!
//! A warm pool is a SINGLE CHILD of one Auto Scaling group, so it is stored on
//! [`StoredGroup`] itself ([`StoredGroup::warm_pool`]): one record that `PutWarmPool`
//! upserts, `DescribeWarmPool` and the group's own reader project from, and the group's
//! delete takes with the group. Nothing in this crate holds a second copy.
//!
//! # The upsert
//!
//! `PutWarmPool` "creates or updates a warm pool for the specified Auto Scaling group"
//! (`smithy_query(query="operation",operation="PutWarmPool")`), and the model documents
//! the update as PER MEMBER: `MaxGroupPreparedCapacity` is removed by "[including] the
//! property but [specifying] -1 for the value", so a member the request omits keeps what
//! is stored rather than being reset. The documented defaults are therefore applied ON
//! CREATE only ([`DEFAULT_WARM_POOL_MIN_SIZE`], [`DEFAULT_WARM_POOL_STATE`]), and
//! [`WarmPoolPut`] keeps an omitted member apart from a supplied one.
//!
//! # The instances the pool holds
//!
//! A warm pool is "a pool of pre-initialized EC2 instances", so the pool this service
//! stores HOLDS instances: the account's own Amazon EC2 instances, launched from the
//! group's launch source through the one launch path every instance of a group is
//! launched through (`AutoScalingState::launch_into_group`), and terminated
//! where they live when the pool shrinks. They are kept apart from the group's own
//! instances because the two are reported apart: "to retrieve information about the
//! instances in a warm pool, you must call the DescribeWarmPool API"
//! (`DescribeAutoScalingGroups`'s own documentation).
//!
//! How many the pool holds is DYNAMIC — [`StoredWarmPool::target_size`] states the
//! documented rule — so the difference between the pool's size and that target is owed
//! by the clock rather than by any one request:
//! [`super::AutoScalingState::reconcile_warm_pools`] materialises it, and a freshly
//! launched warm-pool instance is reported [`WARMED_PENDING`] until
//! [`super::LIFECYCLE_TRANSITION`] takes it to the pool's own state.

use std::time::SystemTime;

use crate::ops::types::{InstanceReusePolicy, WarmPoolConfiguration};

use super::{
    GroupLaunch, LaunchDestination, StoredGroup, StoredInstance, AutoScalingState,
    GROUP_STATE_IN_SERVICE, LIFECYCLE_TRANSITION,
};

/// `MinSize` when a request that CREATES a warm pool omits it.
///
/// From the model's own documentation for `PutWarmPoolType$MinSize`: "Defaults to 0 if
/// not specified."
pub const DEFAULT_WARM_POOL_MIN_SIZE: i32 = 0;

/// `PoolState` when a request that CREATES a warm pool omits it.
///
/// From the model's own documentation for `PutWarmPoolType$PoolState`: "Default is
/// Stopped."
pub const DEFAULT_WARM_POOL_STATE: &str = "Stopped";

/// The `PoolState` that keeps the pool's instances RUNNING, from the model's own
/// `WarmPoolState` enum: the other two (`Stopped`, `Hibernated`) are states an instance
/// is stopped in, so this is the one that tells the two apart.
pub const WARM_POOL_STATE_RUNNING: &str = "Running";

/// The `MaxGroupPreparedCapacity` that REMOVES a stored one, from the model's own
/// documentation for that member: "To remove a value that you previously set, include
/// the property but specify -1 for the value."
pub const MAX_GROUP_PREPARED_CAPACITY_UNSET: i32 = -1;

/// The `LifecycleState` prefix every state of a warm-pool instance carries, from the
/// model's `com.amazonaws.autoscaling#LifecycleState` enum (`Warmed:Pending`,
/// `Warmed:Stopped`, `Warmed:Running`, `Warmed:Hibernated`, `Warmed:Terminating`).
pub const WARMED_STATE_PREFIX: &str = "Warmed:";

/// The `LifecycleState` of an instance the pool has just launched, before the pool's own
/// state comes due.
pub const WARMED_PENDING: &str = "Warmed:Pending";

/// The `LifecycleState` of an instance the pool is taking out.
pub const WARMED_TERMINATING: &str = "Warmed:Terminating";

/// What the failed activity of a warm-pool instance the ACCOUNT could not place records,
/// on the same footing as the group's own ([`super::UNPLACED_INSTANCE_MESSAGE`]) and
/// naming the collection the instance was to be launched into.
pub const UNPLACED_WARM_POOL_INSTANCE_MESSAGE: &str =
    "The account could not place the instance. Launching EC2 instance into warm pool failed.";

/// The largest `MaxRecords` `DescribeWarmPool` honours.
///
/// From the model's own documentation for `DescribeWarmPoolType$MaxRecords`: "The
/// maximum number of instances to return with this call. The maximum value is 50" — a
/// smaller ceiling than the name-ordered describes take, so this operation names its
/// own.
pub const MAX_WARM_POOL_RECORDS: i32 = 50;

/// The `LifecycleState` a settled instance of a pool in `pool_state` is reported in:
/// "the instance state to transition to after the lifecycle actions are complete"
/// (`WarmPoolConfiguration$PoolState`), under the prefix the enum spells warm-pool
/// states with.
pub fn warmed_lifecycle_state(pool_state: &str) -> String {
    format!("{WARMED_STATE_PREFIX}{pool_state}")
}

/// The message a request whose warm pool would hold more instances than this process
/// holds one group to is refused with, in the shape this service reports its other
/// per-group ceilings in: the size that missed the ceiling and the ceiling itself.
pub fn too_large_warm_pool_message(size: i32) -> String {
    format!(
        "The warm pool size {size} exceeds the limit of {} instances per Auto Scaling group",
        super::MAX_GROUP_CAPACITY
    )
}

/// The `Activity$Cause` the launches and terminations that bring a warm pool to its
/// target size record: what asked for them, and the sizes they moved the pool between —
/// the shape every cause this service records takes
/// ([`AutoScalingState::capacity_change_cause`]).
pub fn warm_pool_change_cause(from: i32, to: i32) -> String {
    format!(
        "a warm pool of the Auto Scaling group is being maintained: changing the warm pool size \
         from {from} to {to}."
    )
}

/// The `Activity$Description` a warm-pool launch records, naming the instance it
/// launched — the one wording for it in this crate.
pub fn warm_pool_launch_description(instance_id: &str) -> String {
    format!("Launching a new EC2 instance into warm pool: {instance_id}")
}

/// The `Activity$Description` a warm-pool termination records, in the same shape.
pub fn warm_pool_termination_description(instance_id: &str) -> String {
    format!("Terminating EC2 instance in warm pool: {instance_id}")
}

/// The `Activity$Description` of a warm-pool launch that FAILED: what the launch set out
/// to do and the reason it did not, on the same footing as the group's own
/// ([`super::failed_launch_description`]) \u2014 a launch that failed has no instance to name,
/// so it names the reason instead.
pub fn warm_pool_failed_launch_description(status_message: &str) -> String {
    format!("Launching a new EC2 instance into warm pool. Status Reason: {status_message}")
}

/// The warm pool of one group, as it is stored.
///
/// The group is the key of [`super::AutoScalingState::groups`] this pool hangs off
/// rather than a field, so a pool cannot be stored against a group that disagrees with
/// its own.
#[derive(Debug, Clone)]
pub struct StoredWarmPool {
    /// `MinSize`, as requested or [`DEFAULT_WARM_POOL_MIN_SIZE`].
    pub min_size: i32,
    /// `MaxGroupPreparedCapacity`, absent for a pool whose size follows the group's
    /// maximum capacity — which is what the documented -1 leaves behind.
    pub max_group_prepared_capacity: Option<i32>,
    /// `PoolState`, as requested or [`DEFAULT_WARM_POOL_STATE`].
    pub pool_state: String,
    /// `InstanceReusePolicy`, absent for a pool the request named none for.
    pub instance_reuse_policy: Option<InstanceReusePolicy>,
    /// The instances the pool holds, in launch order: the account's own Amazon EC2
    /// instances, each reported under a `Warmed:` state of its own.
    pub instances: Vec<StoredInstance>,
    /// The shortfall this pool last recorded a failed launch for, so an account that
    /// cannot place an instance records one failure per CHANGE rather than one per call
    /// that re-attempted it.
    pub unplaced_reported: usize,
}

impl StoredWarmPool {
    /// This pool as the API reports it — the `WarmPoolConfiguration` shape.
    ///
    /// THE ONE projection of a stored warm pool onto the wire shape, so the pool
    /// `DescribeWarmPool` answers with and the one the group's own reader reports are
    /// identical. `Status` is left unfilled: it "[reports] the status of a warm pool that
    /// is marked for deletion", and a stored pool is not.
    pub fn to_configuration(&self) -> WarmPoolConfiguration {
        WarmPoolConfiguration {
            max_group_prepared_capacity: self.max_group_prepared_capacity,
            min_size: Some(self.min_size),
            pool_state: Some(self.pool_state.clone()),
            status: None,
            instance_reuse_policy: self.instance_reuse_policy.clone(),
        }
    }

    /// `WarmPoolSize`: "the current size of the warm pool"
    /// (`AutoScalingGroup$WarmPoolSize`) — the instances it holds, including the ones on
    /// their way out, which are still in the pool until the clock retires them.
    pub fn size(&self) -> i32 {
        i32::try_from(self.instances.len()).unwrap_or(i32::MAX)
    }

    /// How many instances the pool holds that are not on their way out, which is what a
    /// reconcile compares against [`Self::target_size`].
    pub fn live_size(&self) -> i32 {
        i32::try_from(
            self.instances
                .iter()
                .filter(|instance| instance.lifecycle_state != WARMED_TERMINATING)
                .count(),
        )
        .unwrap_or(i32::MAX)
    }

    /// How many instances this pool maintains, for a group whose desired capacity is
    /// `desired_capacity` and whose maximum is `max_size`.
    ///
    /// The documented rule, from `PutWarmPoolType$MaxGroupPreparedCapacity`: "If a value
    /// for MaxGroupPreparedCapacity is not specified, Amazon EC2 Auto Scaling launches
    /// and maintains the difference between the group's maximum capacity and its desired
    /// capacity. If you specify a value for MaxGroupPreparedCapacity, Amazon EC2 Auto
    /// Scaling uses the difference between the MaxGroupPreparedCapacity and the desired
    /// capacity instead" — and "if the desired capacity of the Auto Scaling group is
    /// higher than the MaxGroupPreparedCapacity, the capacity of the warm pool is 0,
    /// unless you specify a value for MinSize", which is the floor the pool's own minimum
    /// puts under the difference.
    pub fn target_size(&self, desired_capacity: i32, max_size: i32) -> i32 {
        let prepared = self.max_group_prepared_capacity.unwrap_or(max_size);
        prepared
            .saturating_sub(desired_capacity)
            .max(self.min_size)
            .max(0)
    }
}

/// The warm pool members ONE request carries, each `Some` only where the request named
/// it.
///
/// This is what keeps an omitted member apart from a supplied one, which is the whole of
/// the update rule: see this module's own documentation.
#[derive(Debug, Clone, Default)]
pub struct WarmPoolPut {
    pub min_size: Option<i32>,
    pub max_group_prepared_capacity: Option<i32>,
    pub pool_state: Option<String>,
    pub instance_reuse_policy: Option<InstanceReusePolicy>,
}

impl WarmPoolPut {
    /// The sizes this request would leave the pool bounded by, for the caller to check
    /// against the ceiling this process holds one group to: a pool maintains at least its
    /// own minimum, and at most the prepared capacity it is given.
    pub fn requested_sizes(&self) -> Vec<i32> {
        self.min_size
            .into_iter()
            .chain(
                self.max_group_prepared_capacity
                    .filter(|value| *value != MAX_GROUP_PREPARED_CAPACITY_UNSET),
            )
            .collect()
    }
}

impl StoredGroup {
    /// The warm pool this group holds, if it holds one.
    pub fn warm_pool(&self) -> Option<&StoredWarmPool> {
        self.warm_pool.as_ref()
    }

    /// The instances this group's warm pool holds: empty for a group with no warm pool,
    /// so a reader never has to know which of the two it is looking at.
    pub fn warm_pool_instances(&self) -> &[StoredInstance] {
        self.warm_pool
            .as_ref()
            .map_or(&[][..], |pool| pool.instances.as_slice())
    }

    /// How many instances this group's warm pool holds.
    pub fn warm_pool_instance_count(&self) -> usize {
        self.warm_pool_instances().len()
    }

    /// `WarmPoolSize` for this group, absent for a group with no warm pool.
    pub fn warm_pool_size(&self) -> Option<i32> {
        self.warm_pool.as_ref().map(StoredWarmPool::size)
    }

    /// How many instances this group's warm pool maintains, against the group's own
    /// capacity ([`StoredWarmPool::target_size`]); 0 for a group with no warm pool.
    pub fn warm_pool_target_size(&self) -> i32 {
        self.warm_pool
            .as_ref()
            .map_or(0, |pool| pool.target_size(self.desired_capacity, self.max_size))
    }

    /// Create or update this group's warm pool, per the update rule in this module's own
    /// documentation.
    ///
    /// A pool state the request CHANGES is owed by the instances the pool already holds:
    /// each settled one re-enters [`WARMED_PENDING`] and reaches the new state when
    /// [`LIFECYCLE_TRANSITION`] is due, exactly as a freshly launched one does, so a
    /// caller that reads the pool straight after the change still sees the transition.
    pub fn put_warm_pool(&mut self, put: &WarmPoolPut, now: SystemTime) {
        match self.warm_pool.as_mut() {
            // An UPDATE: every member the request omits keeps the value the pool already
            // carries, and the documented -1 clears the stored prepared capacity.
            Some(pool) => {
                if let Some(min_size) = put.min_size {
                    pool.min_size = min_size;
                }
                if let Some(prepared) = put.max_group_prepared_capacity {
                    pool.max_group_prepared_capacity =
                        (prepared != MAX_GROUP_PREPARED_CAPACITY_UNSET).then_some(prepared);
                }
                if let Some(policy) = put.instance_reuse_policy.clone() {
                    pool.instance_reuse_policy = Some(policy);
                }
                if let Some(pool_state) = put.pool_state.clone() {
                    if pool_state != pool.pool_state {
                        pool.pool_state = pool_state;
                        for instance in &mut pool.instances {
                            if instance.lifecycle_state != WARMED_TERMINATING {
                                instance.lifecycle_state = WARMED_PENDING.to_string();
                                instance.transition_due = Some(now + LIFECYCLE_TRANSITION);
                            }
                        }
                    }
                }
            }
            // A CREATE: the members the request omits take their documented defaults, and
            // the pool starts empty — the instances it maintains are owed by the clock.
            None => {
                self.warm_pool = Some(StoredWarmPool {
                    min_size: put.min_size.unwrap_or(DEFAULT_WARM_POOL_MIN_SIZE),
                    max_group_prepared_capacity: put
                        .max_group_prepared_capacity
                        .filter(|value| *value != MAX_GROUP_PREPARED_CAPACITY_UNSET),
                    pool_state: put
                        .pool_state
                        .clone()
                        .unwrap_or_else(|| DEFAULT_WARM_POOL_STATE.to_string()),
                    instance_reuse_policy: put.instance_reuse_policy.clone(),
                    instances: Vec::new(),
                    unplaced_reported: 0,
                });
            }
        }
    }
}

impl AutoScalingState {
    /// Bring every group's warm pool to the size it maintains, and settle the warm-pool
    /// transitions the clock has made due at `now`.
    ///
    /// Called from [`AutoScalingState::materialize_due`], so it runs on every handler —
    /// including the reporting ones, which may write exactly this much: the difference
    /// between a pool's size and its target was owed at `now` whether or not anybody
    /// called, because "the size of the warm pool is dynamic"
    /// (`PutWarmPoolType$MaxGroupPreparedCapacity`) and Amazon EC2 Auto Scaling
    /// "launches and maintains" it.
    pub fn reconcile_warm_pools<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        now: SystemTime,
    ) {
        let names: Vec<String> = self
            .groups
            .values()
            .filter(|group| group.warm_pool.is_some())
            .map(|group| group.name.clone())
            .collect();
        for name in names {
            self.settle_warm_pool(engine, &name, now);
            self.resize_warm_pool(engine, &name, now);
        }
    }

    /// Retire the warm-pool transitions `now` has made due: an instance whose
    /// [`WARMED_PENDING`] span has elapsed is reported in the pool's own state, and one
    /// whose [`WARMED_TERMINATING`] span has elapsed has left the pool.
    ///
    /// Being in the pool's state is what the pool's instances are FOR, so the Amazon EC2
    /// instance is held there too: a pool that is not `Running` holds instances Amazon
    /// EC2 reports stopped, and one that is holds them running
    /// ([`super::ec2_instances::set_running`]). Every settled instance is re-asserted on
    /// each pass rather than only at the instant it settles, because that move is the
    /// pool's to own however long Amazon EC2's own clock takes over it — and the call is
    /// idempotent, so an instance already there is left alone.
    fn settle_warm_pool<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        now: SystemTime,
    ) {
        let Some(pool) = self
            .groups
            .get_mut(group_name)
            .and_then(|group| group.warm_pool.as_mut())
        else {
            return;
        };
        let warmed = warmed_lifecycle_state(&pool.pool_state);
        let running = pool.pool_state == WARM_POOL_STATE_RUNNING;
        for instance in &mut pool.instances {
            if instance.transition_due.is_some_and(|due| due <= now) {
                instance.transition_due = None;
                if instance.lifecycle_state == WARMED_PENDING {
                    instance.lifecycle_state = warmed.clone();
                }
            }
        }
        pool.instances.retain(|instance| {
            instance.lifecycle_state != WARMED_TERMINATING || instance.transition_due.is_some()
        });
        let settled: Vec<String> = pool
            .instances
            .iter()
            .filter(|instance| instance.lifecycle_state == warmed)
            .map(|instance| instance.instance_id.clone())
            .collect();
        super::ec2_instances::set_running(engine, &settled, running);
    }

    /// Launch what the pool is missing and take out what it holds beyond its target,
    /// recording the scaling activity each of those changes produces.
    ///
    /// The instances are the account's OWN Amazon EC2 instances, on the same footing as a
    /// capacity change's: the launch goes through the one launch path
    /// (`AutoScalingState::launch_into_group`) and the termination shuts the same
    /// instances down where they live, so every id the pool reports resolves in Amazon
    /// EC2. A unit Amazon EC2 could not place records a failed activity instead of an
    /// instance, since there is no instance to report.
    fn resize_warm_pool<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        group_name: &str,
        now: SystemTime,
    ) {
        let Some(group) = self.groups.get(group_name) else {
            return;
        };
        let Some(pool) = group.warm_pool.as_ref() else {
            return;
        };
        let arn = group.arn.clone();
        let zoneless = group.availability_zones.is_empty();
        let held = pool.live_size();
        let target = group.warm_pool_target_size();
        // Without a zone to place an instance in, nothing can be launched; a group
        // always holds at least one, so this is the floor under the launch rather than a
        // case any request reaches.
        let to_launch = if zoneless {
            0
        } else {
            (target - held).max(0).min(group.launch_headroom())
        };
        let surplus = (held - target).max(0);
        let taken: Vec<String> = pool
            .instances
            .iter()
            .rev()
            .filter(|instance| instance.lifecycle_state != WARMED_TERMINATING)
            .take(usize::try_from(surplus).unwrap_or(0))
            .map(|instance| instance.instance_id.clone())
            .collect();
        if to_launch == 0 && taken.is_empty() {
            // Nothing is owed, so nothing can be unplaced: the next shortfall is a change
            // from none and records its own failure.
            if let Some(pool) = self
                .groups
                .get_mut(group_name)
                .and_then(|group| group.warm_pool.as_mut())
            {
                pool.unplaced_reported = 0;
            }
            return;
        }

        let cause = warm_pool_change_cause(held, target);
        let due = now + LIFECYCLE_TRANSITION;
        let launch = self.launch_into_group(
            engine,
            &GroupLaunch {
                group_name,
                count: usize::try_from(to_launch).unwrap_or(0),
                due,
                // A warm pool launches from the group's own launch source.
                desired_configuration: None,
                destination: LaunchDestination::WarmPool,
            },
        );
        let launched_ids = launch.launched_ids.clone();
        let unplaced_message = launch.unplaced_message(UNPLACED_WARM_POOL_INSTANCE_MESSAGE);
        let unplaced = launch.unplaced;
        if !taken.is_empty() {
            if let Some(pool) = self
                .groups
                .get_mut(group_name)
                .and_then(|group| group.warm_pool.as_mut())
            {
                for instance in &mut pool.instances {
                    if taken.contains(&instance.instance_id) {
                        instance.lifecycle_state = WARMED_TERMINATING.to_string();
                        instance.transition_due = Some(due);
                    }
                }
            }
            super::ec2_instances::terminate(engine, &taken);
        }

        for instance_id in &launched_ids {
            self.record_activity(
                group_name,
                &arn,
                warm_pool_launch_description(instance_id),
                cause.clone(),
                GROUP_STATE_IN_SERVICE,
                now,
                Some(due),
            );
        }
        for instance_id in &taken {
            self.record_activity(
                group_name,
                &arn,
                warm_pool_termination_description(instance_id),
                cause.clone(),
                GROUP_STATE_IN_SERVICE,
                now,
                Some(due),
            );
        }
        // A shortfall Amazon EC2 could not place is re-attempted on the next pass, so the
        // failed launch is recorded when the shortfall CHANGES rather than once per call
        // that re-attempted it: an account that can place nothing would otherwise fill the
        // activity log with one identical failure per request and evict the group's real
        // history.
        let reported = if let Some(pool) = self
            .groups
            .get_mut(group_name)
            .and_then(|group| group.warm_pool.as_mut())
        {
            let reported = pool.unplaced_reported;
            pool.unplaced_reported = unplaced;
            reported
        } else {
            unplaced
        };
        if unplaced != reported {
            for _ in 0..unplaced {
                self.record_failed_activity(
                    group_name,
                    &arn,
                    warm_pool_failed_launch_description(&unplaced_message),
                    unplaced_message.clone(),
                    cause.clone(),
                    now,
                );
            }
        }
    }
}
