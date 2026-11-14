// SPDX-License-Identifier: CC0-1.0

//! WHICH Availability Zone each registered instance is in, as EC2 reports it.
//!
//! An instance is EC2's resource and its placement is EC2's to say, so the zone a
//! registered instance is in is READ from that service's own store rather than kept in a
//! second copy here — the same seam a health check reads whether anything stands behind a
//! registration through ([`super::instance_checks`]).
//!
//! What it decides: `CrossZoneLoadBalancing$Enabled` is the attribute that says whether a
//! load balancer node spreads requests beyond its own zone, so a node of a load balancer
//! with the attribute off may send a request only to the registered instances in the zone
//! that node is in — which is a fact about each instance, not about the load balancer.

use std::collections::{BTreeMap, BTreeSet};

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The Availability Zone EC2 reports for each of `instance_ids`, keyed by instance id.
///
/// The account's EC2 inventory is read ONCE for the whole set, so the cost does not grow
/// with the number of registrations times the size of the inventory. An id EC2 holds no
/// instance for, and every id at all where there is no EC2 to ask, carries no entry: the
/// zone is left unknown rather than invented.
pub fn observe_instance_zones<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_ids: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    if instance_ids.is_empty() {
        return BTreeMap::new();
    }
    let Some(inventory) = ec2::service_state(engine) else {
        return BTreeMap::new();
    };
    let store = inventory.lock();
    instance_ids
        .iter()
        .filter_map(|instance_id| {
            let instance = store.instances.get(instance_id)?;
            let zone = instance.placement_az.clone();
            (!zone.is_empty()).then_some((instance_id.clone(), zone))
        })
        .collect()
}
