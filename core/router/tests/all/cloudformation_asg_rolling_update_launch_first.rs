// SPDX-License-Identifier: CC0-1.0

//! The row of the rolling-update pacing rule where the group can spare NOTHING: it launches
//! before it terminates.
//!
//! The RULE is the one `cloudformation_asg_rolling_update_pacing.rs` states and whose runner
//! this row is driven through — a rolling update is PACED, and the pacing is what a reader of
//! the group sees. What this row adds is the case where the floor the policy states leaves the
//! group nothing to spare: `MinInstancesInService` equals the capacity the group is declared
//! with, so the replacement cannot take an instance out of service until the group holds one
//! MORE than it was declared with ("this value must be less than the `MaxSize` of the Auto
//! Scaling group"). The group therefore works at a capacity above its declared one for the
//! length of the replacement, the dip each batch holds is a dip below THAT capacity rather than
//! below the declared one, and the group is back to what it was declared with once the update
//! has completed.
//!
//! Why a client cares: this is the shape a zero-downtime deploy of a small group takes — every
//! instance it has must keep serving — and it is the one case where the group's own `MaxSize`
//! decides whether the paced replacement can proceed at all.

use crate::cloudformation_asg_rolling_update_pacing::{a_watched_rolling_update, Row};

#[tokio::test]
async fn a_group_that_can_spare_no_instance_launches_before_it_terminates() {
    a_watched_rolling_update(&Row {
        case: "a group that must keep both its instances in service launches before terminating",
        stack: "rolling-pacing-launch-first",
        desired: 2,
        max_size: 3,
        max_batch_size: 1,
        min_instances_in_service: 2,
        wait_on_signals: true,
        batch: 1,
        // The floor leaves the group nothing to spare, so the replacement launches ahead: it
        // works at one instance more than the group was declared with, which its `MaxSize`
        // leaves room for.
        working: 3,
        batches: 2,
    })
    .await;
}
