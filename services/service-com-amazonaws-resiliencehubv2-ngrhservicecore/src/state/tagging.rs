// SPDX-License-Identifier: CC0-1.0

//! The tag side of this service's store: the one projection every `TagMap` body is built
//! with, and the one write path a tag request reaches a tagged resource through.
//!
//! Tags are not a resource of their own — they are a map carried BY a resource (a policy's
//! [`PolicyRecord::tags`](super::PolicyRecord), a service's
//! [`ServiceRecord::tags`](super::ServiceRecord)) — so `TagResource` addresses one by ARN
//! and writes into the record that holds it ([`ComAmazonawsResiliencehubv2NgrhservicecoreState::resource_tags_mut`]).
//! Every reader of that resource projects the SAME stored map through [`tag_map_view`], so
//! a tag write is visible to the next read of the resource and the two cannot drift.

use std::collections::{BTreeMap, HashMap};

use super::{
    arn_resource_id, ComAmazonawsResiliencehubv2NgrhservicecoreState, POLICY_RESOURCE,
    POLICY_RESOURCE_TYPE, SERVICE_RESOURCE, SERVICE_RESOURCE_TYPE,
};

/// How many tags one resource in this store may hold.
///
/// `TagMap`'s own modelled `@length 1..50` maximum, which the model states for the map a
/// single request carries and for the map a resource reports. A request that would take a
/// resource past it is refused rather than silently truncated or partly applied.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Stored tags as the modelled `TagMap` the wire carries.
///
/// The member is ALWAYS reported: a resource's tag map is the set of tags it carries, and
/// a resource carrying none has the EMPTY map rather than an unknown one, so a client that
/// enumerates or diffs tags never has to tell "no tags" from "not told". Shared by every
/// tagged resource in this store, so one rule covers them all.
pub(crate) fn tag_map_view(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The `resourceType` a refusal for `arn` reports: the kind of resource its resource
/// segment (`<kind>/<id>`) names, under the name the model's shape for that resource
/// carries — `Policy` for a `policy/…` ARN, `Service` for a `service/…` one.
///
/// An ARN naming any other kind is reported under the kind it names, so the refusal says
/// what the caller addressed rather than claiming it addressed one of the two resources
/// this store holds.
pub fn tagged_resource_type(arn: &str) -> &str {
    let resource_id = arn_resource_id(arn);
    match resource_id.split('/').next() {
        Some(POLICY_RESOURCE) => POLICY_RESOURCE_TYPE,
        Some(SERVICE_RESOURCE) => SERVICE_RESOURCE_TYPE,
        Some(other) => other,
        None => resource_id,
    }
}

impl ComAmazonawsResiliencehubv2NgrhservicecoreState {
    /// The stored tag map of the resource `arn` names, to write into, or `None` when the
    /// ARN names no resource this store holds.
    ///
    /// The ARN is resolved against the resources themselves rather than parsed into a
    /// kind: a tag write lands on the record a read of the same ARN answers from, so no
    /// write can reach a resource that is not there and no read can miss one that is.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(service) = self.services.get_mut(arn) {
            return Some(&mut service.tags);
        }
        self.policies.get_mut(arn).map(|policy| &mut policy.tags)
    }
}
