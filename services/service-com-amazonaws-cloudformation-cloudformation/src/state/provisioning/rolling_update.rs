// SPDX-License-Identifier: CC0-1.0

//! Carrying out the BATCHED replacement of an Auto Scaling group's instances that an
//! `UpdatePolicy`'s `AutoScalingRollingUpdate` asks an update for: the calls one batch owes
//! Amazon EC2 Auto Scaling, pass after pass, until every instance carries the configuration
//! the update put in force.
//!
//! ## The rule this module carries
//!
//! What the policy asks for, and what each member of it decides, is the model beside the stack
//! ([`crate::state::rolling_update`]); this is the half that REACHES the owning service, which
//! nothing under the store's lock can do. One pass per open replacement:
//!
//! * it reads the group — the instances it holds, each with its lifecycle state, and the
//!   desired capacity that service reports — so the batch's progress is that service's own
//!   account of the group rather than a count this crate kept;
//! * it asks the model for the ONE step that is owed ([`RollingUpdate::next_step`]); and
//! * it carries that step out: holding the group back from launching and terminating a batch's
//!   instances so they leave service before their replacements are launched, letting it launch
//!   again once the span a replacement takes has passed ("if you do not specify the option to
//!   decrement the desired capacity, Amazon EC2 Auto Scaling launches instances to replace the
//!   ones that are terminated", `TerminateInstanceInAutoScalingGroup`), or recording the
//!   replacement as through, or failing the update the way a FAILURE signal fails one.
//!
//! The SIGNALS a batch waits for where `WaitOnResourceSignals` asks for them are not this
//! module's: they are recorded for the instances that sent them by the one pass that records
//! every instance's signal ([`super::instance_signals`]), and read here off the stack's own
//! signal log. A replacement whose instance reports its own bootstrap FAILED therefore fails
//! the update through the same path a client's own `SignalResource` call does — the update is
//! cancelled and the stack rolled back to the configuration it started from.
//!
//! It is driven from [`super::advance_stabilisations_in`] for the reason every other call
//! there is: the answers come from another service, and what this records was owed at the
//! instant that service brought the batch's replacements up, whether or not anybody called.
//!
//! ## Bounds
//!
//! One group read and at most the two calls one batch's half takes per open replacement per
//! pass, and a replacement is open only while an update of a group whose declaration carries
//! the policy is in flight — nothing at all for every other operation, beyond the one
//! resumption a group whose launches nobody is holding back any more owes
//! ([`release_stale_launch_holds`]). The batch is at most `MaxBatchSize` instances, and the
//! whole replacement works through the instances the owning service lets one group hold. Every
//! batch has an instant it gives up at ([`RollingUpdate::next_step`]), so no policy holds a
//! stack in progress for ever.

use super::*;

use crate::state::provisioner::compute_fleet;
use crate::state::rolling_update::{BatchStep, GroupReport, RollingUpdate};

/// Carry every open batched replacement of this Region's updating stacks one step further.
///
/// Called by [`super::advance_stabilisations_in`] beside the pass that records the signals a
/// batch waits for ([`super::instance_signals`]), which runs first: a batch its replacements
/// have signalled is through in the same pass rather than one later.
pub(super) async fn settle_rolling_updates(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<crate::state::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    // Which replacements are open, read under a lock of its own: the reads below reach the
    // owning service's own handlers, which take that service's lock, so a client reading THIS
    // service must not be blocked behind them.
    let open: Vec<(String, Vec<RollingUpdate>)> = {
        let store = state.lock();
        store
            .stacks
            .iter()
            .filter(|stack| stack.region == region)
            .filter_map(|stack| {
                let plans = stack.open_rolling_updates();
                (!plans.is_empty()).then(|| (stack.stack_id.clone(), plans))
            })
            .collect()
    };
    for (stack_id, plans) in open {
        for plan in plans {
            // BOXED, like every other owning-service call a pass of this module's parent
            // makes: the read is another service's whole handler, and this function holds its
            // state while the rest of the pass runs
            // ([`super::advance_stabilisations_in`]).
            let read = Box::pin(compute_fleet::group_instance_states(
                ctx,
                region,
                &plan.physical_resource_id,
            ));
            // One unanswered read is no evidence about the group — a service is allowed to
            // throttle or be briefly unreachable — so the replacement holds and the next pass
            // asks again; only its own window running out ends it
            // ([`RollingUpdate::next_step`]).
            let Ok(Some((desired_capacity, max_size, instances))) = read.await else {
                continue;
            };
            let reported = GroupReport {
                desired_capacity,
                max_size,
                instances,
            };
            let now = SystemTime::now();
            // The step this replacement owes, decided over what that service reports and the
            // signals the stack has recorded — with what the FIRST pass found recorded before
            // the decision is made, so the set of instances the replacement works through, and
            // the capacity it leaves the group at, are the ones the step is decided over.
            let decided = {
                let mut store = state.lock();
                let Some(stack) = store.resolve_mut(region, &stack_id) else {
                    continue;
                };
                let found: Vec<String> = reported
                    .instances
                    .iter()
                    .map(|(instance_id, _)| instance_id.clone())
                    .collect();
                stack.record_rolling_update_start(&plan.logical_id, &found, desired_capacity);
                let signals = stack.recorded_signals(&plan.logical_id);
                let withheld = stack.withholds_group_launch(&plan.physical_resource_id);
                stack
                    .open_rolling_updates()
                    .into_iter()
                    .find(|open| open.logical_id == plan.logical_id)
                    .map(|open| open.next_step(&reported, &signals, withheld, now))
            };
            let Some(step) = decided else {
                continue;
            };
            match step {
                // The batch in flight has not come through yet: nothing is owed, and the next
                // pass asks the owning service again.
                BatchStep::Hold => {}
                BatchStep::Replace(batch) => {
                    // CLAIMED first, under one lock and before any call: the batch this pass is
                    // about to take out of service, and the hold on the group's launches that
                    // keeps their replacements from being launched before them. A pass of this
                    // module runs beside every other one, and a batch two of them both decided
                    // would take more instances out of service than `MaxBatchSize` allows
                    // ([`crate::state::StoredStack::record_rolling_update_batch`]).
                    {
                        let mut store = state.lock();
                        let Some(stack) = store.resolve_mut(region, &stack_id) else {
                            continue;
                        };
                        stack.withhold_group_launch(&plan.physical_resource_id);
                        stack.record_rolling_update_batch(
                            &plan.logical_id,
                            batch.clone(),
                            SystemTime::now(),
                        );
                    }
                    // The group is held back from LAUNCHING before anything is terminated, so
                    // the batch's instances leave service before their replacements are
                    // launched — which is what the dip a reader of the group sees is. Every
                    // call is made with the store RELEASED, exactly as every other call this
                    // module makes.
                    //
                    // A call that is refused gives the CLAIM back, so the next pass asks for the
                    // same batch again. The hold on the group's launches stays recorded either
                    // way: a suspension this service may have made is one it owes a resumption
                    // for ([`crate::state::StoredStack::stale_withheld_group_launches`]).
                    let withheld = Box::pin(compute_fleet::withhold_group_launches(
                        ctx,
                        region,
                        &plan.physical_resource_id,
                    ))
                    .await;
                    if withheld.is_err() {
                        release_claimed_batch(state, region, &stack_id, &plan.logical_id);
                        continue;
                    }
                    let terminated = Box::pin(compute_fleet::terminate_group_instances(
                        ctx,
                        region,
                        &plan.physical_resource_id,
                        &batch,
                    ))
                    .await;
                    if terminated.is_err() {
                        release_claimed_batch(state, region, &stack_id, &plan.logical_id);
                    }
                }
                // The batch's instances are out of service and the span their replacements take
                // to come up has passed: the group is let launch again and asked for the
                // capacity it holds, which is what brings those replacements up — "a user
                // request resumed the scaling processes of the group, bringing its instances to
                // the desired capacity". Recorded only once that service has accepted BOTH, so
                // a refused call leaves the launches held back and the next pass asks again.
                BatchStep::LaunchReplacements(capacity) => {
                    let resumed = Box::pin(compute_fleet::resume_group_launches(
                        ctx,
                        region,
                        &plan.physical_resource_id,
                    ))
                    .await;
                    if resumed.is_err() {
                        continue;
                    }
                    // ...and the capacity the group holds is asked for again, which is what the
                    // group converges on: "a group maintains the capacity it is configured for"
                    // is what makes the launch due, and the capacity asked for here is the one
                    // the group already reports, so the call changes nothing about the group's
                    // SIZE and leaves the convergence as the whole of its effect. Made because a
                    // resumption converges a group only from the capacity it holds at that
                    // instant: without it a group whose launch was withheld while something else
                    // of it was still settling would be left short of the capacity it is
                    // configured for, with its replacement never launched.
                    let resized = Box::pin(compute_fleet::set_group_capacity(
                        ctx,
                        region,
                        &plan.physical_resource_id,
                        capacity,
                    ))
                    .await;
                    if resized.is_err() {
                        continue;
                    }
                    let mut store = state.lock();
                    if let Some(stack) = store.resolve_mut(region, &stack_id) {
                        stack.release_group_launch(&plan.physical_resource_id);
                    }
                }
                // The group has to LAUNCH before it can terminate, or has the capacity it
                // borrowed to give back: either way the call is the one that sets the size the
                // group holds. What the next pass does about it follows from what that service
                // then reports the group's capacity as, so nothing of it is recorded here.
                BatchStep::Grow(capacity) | BatchStep::Shrink(capacity) => {
                    let _resized = Box::pin(compute_fleet::set_group_capacity(
                        ctx,
                        region,
                        &plan.physical_resource_id,
                        capacity,
                    ))
                    .await;
                }
                BatchStep::Finished => {
                    let mut store = state.lock();
                    if let Some(stack) = store.resolve_mut(region, &stack_id) {
                        stack.finish_rolling_update(&plan.logical_id);
                    }
                }
                // The replacement cannot be carried through, so the update fails and is rolled
                // back to the configuration it started from. The revert itself is the one path
                // every rollback this service owes goes through
                // ([`crate::state::settle_due_rollbacks`]), which the operation driving this
                // pass performs for itself.
                BatchStep::Failed(reason) => {
                    let mut store = state.lock();
                    if let Some(stack) = store.resolve_mut(region, &stack_id) {
                        let _revert =
                            stack.fail_rolling_update(&plan.logical_id, &reason, SystemTime::now());
                    }
                }
            }
        }
    }
    // ...and every suspension this service is still holding a group's launches back with
    // although no replacement is working through that group any more: an operation superseded
    // or cancelled between a batch's two halves would otherwise leave the group never
    // launching into the capacity it is configured for again. Last, so a batch this pass just
    // started is not undone by it.
    release_stale_launch_holds(ctx, state, region).await;
}

/// Give the batch a pass claimed but could not take out of service back, so the next pass asks
/// for the same one again
/// ([`crate::state::StoredStack::release_rolling_update_batch`]).
fn release_claimed_batch(
    state: &::engine::ServiceState<crate::state::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
    stack_id: &str,
    logical_id: &str,
) {
    let mut store = state.lock();
    if let Some(stack) = store.resolve_mut(region, stack_id) {
        stack.release_rolling_update_batch(logical_id);
    }
}

/// Let every group this service is needlessly holding the launches of launch again.
///
/// The record is the stack's ([`crate::state::StoredStack::stale_withheld_group_launches`]), so
/// it outlives the replacement that made the suspension; an entry is dropped once that service
/// has accepted the resumption, or once the stack no longer holds the group at all — there is
/// nothing left to resume for a group its stack has let go.
async fn release_stale_launch_holds(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<crate::state::ComAmazonawsCloudformationCloudformationState>,
    region: &str,
) {
    let stale: Vec<(String, Vec<String>)> = {
        let store = state.lock();
        store
            .stacks
            .iter()
            .filter(|stack| stack.region == region)
            .filter_map(|stack| {
                let groups = stack.stale_withheld_group_launches();
                (!groups.is_empty()).then(|| (stack.stack_id.clone(), groups))
            })
            .collect()
    };
    for (stack_id, groups) in stale {
        for group in groups {
            let resumed =
                Box::pin(compute_fleet::resume_group_launches(ctx, region, &group)).await;
            let mut store = state.lock();
            let Some(stack) = store.resolve_mut(region, &stack_id) else {
                continue;
            };
            let held_by_the_stack = stack
                .current_resources()
                .iter()
                .any(|resource| resource.physical_resource_id == group);
            if resumed.is_ok() || !held_by_the_stack {
                stack.release_group_launch(&group);
            }
        }
    }
}
