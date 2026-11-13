// SPDX-License-Identifier: CC0-1.0

//! The tags an Auto Scaling group carries: how one is stored when a request creates it,
//! and the ONE projection every read of it reports.
//!
//! A tag of a group is a CHILD of that group ([`StoredGroup::tags`]), and three of its
//! members are the SERVICE's decision rather than the caller's: which resource the tag is
//! on (`ResourceId`, "the name of the Auto Scaling group"), what kind of resource that is
//! (`ResourceType`, "the only supported value is `auto-scaling-group`") and whether the
//! tag is "added to new instances as they are launched in the group"
//! (`PropagateAtLaunch`). Each is decided when the tag is stored, so each is present on
//! every read of the tag — a tag created without `PropagateAtLaunch` takes the service's
//! own default for the member ([`DEFAULT_PROPAGATE_AT_LAUNCH`]) and stores it, rather than
//! storing nothing at all.
//!
//! That makes [`stored_tag`] the one place a requested tag becomes a stored one and
//! [`tag_description`] the one place a stored tag becomes what the wire reports, so the
//! group's own read, the account-wide tag read and the propagation that happens at launch
//! all see one tag the same way.

use crate::ops::types::{Tag, TagDescription};

use super::StoredGroup;

/// The `ResourceType` a tag on an Auto Scaling group is reported under.
///
/// From the model's own documentation for `Tag$ResourceType`: "The only supported value is
/// `auto-scaling-group`."
pub const GROUP_RESOURCE_TYPE: &str = "auto-scaling-group";

/// Whether a tag created without `PropagateAtLaunch` is added to the instances the group
/// launches.
///
/// The flag "determines whether the tag is added to new instances as they are launched in
/// the group" (`Tag$PropagateAtLaunch`), and the model states no default for it
/// (`smithy_query(query="shape",shape="Tag")` carries no `@default` on the member), so the
/// default is the service's own. Observed AWS behaviour decides which it is: a tag passed
/// to `CreateAutoScalingGroup` with no `PropagateAtLaunch` reads back PROPAGATING on every
/// later read of the group and of the account's tags, so a `propagate-at-launch=false`
/// filter then selects only a tag whose creation stated `false`. (CloudFormation's own
/// `Tag` property for the type requires the member, so a declaration always states one;
/// this default is what an API caller that omits it gets.)
pub const DEFAULT_PROPAGATE_AT_LAUNCH: bool = true;

/// Whether the tag is added to the instances the group launches.
///
/// The stored decision, read through one rule: a tag that states nothing takes the
/// service's default ([`DEFAULT_PROPAGATE_AT_LAUNCH`]).
pub fn propagates_at_launch(tag: &Tag) -> bool {
    tag.propagate_at_launch
        .unwrap_or(DEFAULT_PROPAGATE_AT_LAUNCH)
}

/// The tag a group stores for one tag a request carries.
///
/// The caller's own members — the key and the value — are kept as written; the three the
/// service decides are filled in: the tag is reported against THIS group whatever resource
/// the request named, under the one supported resource type, and with the propagation
/// decision the request stated or [`DEFAULT_PROPAGATE_AT_LAUNCH`] when it stated none.
pub fn stored_tag(group_name: &str, tag: Tag) -> Tag {
    Tag {
        resource_id: Some(group_name.to_string()),
        resource_type: Some(GROUP_RESOURCE_TYPE.to_string()),
        key: tag.key,
        value: tag.value,
        propagate_at_launch: Some(
            tag.propagate_at_launch
                .unwrap_or(DEFAULT_PROPAGATE_AT_LAUNCH),
        ),
    }
}

/// The `TagDescription` the service reports for a stored tag — THE projection of a group
/// tag onto the wire, so the group's own read and the account-wide tag read report one tag
/// identically.
pub fn tag_description(tag: &Tag) -> TagDescription {
    TagDescription {
        resource_id: tag.resource_id.clone(),
        resource_type: tag.resource_type.clone(),
        key: Some(tag.key.clone()),
        value: tag.value.clone(),
        // A tag stored before this member was a stored decision still reports one: the
        // projection answers the default rather than an absent member, because the flag
        // decides inheritance for every tag of a group.
        propagate_at_launch: Some(
            tag.propagate_at_launch
                .unwrap_or(DEFAULT_PROPAGATE_AT_LAUNCH),
        ),
    }
}

impl StoredGroup {
    /// Every tag of this group as the wire reports it, in the order the group holds them.
    pub fn tag_descriptions(&self) -> Vec<TagDescription> {
        self.tags.iter().map(tag_description).collect()
    }
}
