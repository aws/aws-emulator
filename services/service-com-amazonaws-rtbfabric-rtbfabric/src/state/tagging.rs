// SPDX-License-Identifier: CC0-1.0

//! The `/tags/{resourceArn}` surface's state: WHICH stored resource a
//! caller-supplied ARN names, and the tags that resource holds.
//!
//! `ListTagsForResource`, `TagResource` and `UntagResource` are three operations on
//! one surface, so the reading of the ARN and the lookup of the record it addresses
//! live here once rather than three times: the read and the two writes then answer
//! about exactly the same set of resources, and cannot drift apart about which ARN
//! names what.
//!
//! Which records this surface can address is whatever the store holds once the clock
//! has been settled, so each of the three handlers calls
//! [`settle_all`](super::ComAmazonawsRtbfabricRtbfabricState::settle_all) before it
//! resolves the ARN: a record whose release the clock has already completed is gone
//! for this surface exactly as it is for the resource's own read.
//!
//! Tags are not a store of their own — every taggable record already carries its own
//! `tags` map, written by the create that minted it — so this module only addresses
//! them. That is also what keeps the surface bounded: a tag map lives and dies with
//! its resource, and the resources are bounded by
//! [`GATEWAY_QUOTA`](super::GATEWAY_QUOTA) and [`LINK_QUOTA`](super::LINK_QUOTA),
//! while [`TAGS_PER_RESOURCE_QUOTA`] bounds how many tags one record holds.

use std::collections::HashMap;

use super::ComAmazonawsRtbfabricRtbfabricState;

/// The ARN namespace (an `aws.api#service$arnNamespace`) every ARN this service's
/// tagging surface admits carries, as the modelled `RtbTaggableResourceArn` pattern
/// pins it (`^arn:aws:rtbfabric:…`).
pub const ARN_VENDOR: &str = "rtbfabric";

/// The partition the same pattern pins, and so the only one an ARN naming a resource
/// of this service carries.
const ARN_PARTITION: &str = "aws";

/// The resource-field segment that introduces a gateway id.
const GATEWAY_SEGMENT: &str = "gateway";
/// The resource-field segment that introduces a link id.
const LINK_SEGMENT: &str = "link";
/// The resource-field segment that introduces a routing-rule id.
const ROUTING_RULE_SEGMENT: &str = "routing-rule";

/// How many tags one resource holds before `TagResource` refuses to add another.
///
/// AWS's tagging limit is 50 tags per resource, and this service's own
/// `TagKeyList` is bounded at 50 members too
/// (`smithy_query(query="shape", shape="TagKeyList")`), so 50 is the bound a
/// single resource's tag map is held to. It is also what stops a caller growing one
/// record without end, since the keys and values are caller-chosen.
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The one application of [`TAGS_PER_RESOURCE_QUOTA`]: `Ok(())` when a record holding
/// `resulting_tags` tags is within the bound, otherwise the message the refusal states.
///
/// The bound is a property of the RESOURCE rather than of one operation, so every call
/// that writes a record's `tags` map is held to it — the creates that mint a record
/// carrying the caller's map, and `TagResource` on a record that already holds one.
/// Checked against how many tags the record would hold AFTERWARDS and BEFORE anything
/// is written, so a refused call leaves the record as it was and no record is ever
/// persisted holding more tags than can be assigned to it.
pub fn tag_count_within_quota(resulting_tags: usize) -> Result<(), String> {
    if resulting_tags > TAGS_PER_RESOURCE_QUOTA {
        return Err(format!(
            "a resource holds at most {TAGS_PER_RESOURCE_QUOTA} tags"
        ));
    }
    Ok(())
}

/// [`tag_count_within_quota`] for the optional `tags` map a create carries: the record
/// a create mints holds no tags of its own beforehand, so the map the call sent is the
/// whole of what that record would hold.
pub fn created_tags_within_quota(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    tag_count_within_quota(tags.map_or(0, HashMap::len))
}

/// The wire error code a tagging call whose `resourceArn` names no resource of this
/// service is answered with.
///
/// Not one of the error shapes the model declares for these operations: the recorded
/// answer to a syntactically valid ARN naming a gateway that does not exist is this
/// code with [`TAGGING_UNRESOLVED_MESSAGE`], so the modelled `ResourceNotFoundException`
/// the documentation suggests is not what the service serves here.
pub const TAGGING_UNRESOLVED_CODE: &str = "InternalServerErrorException";

/// The HTTP status that answer carries.
pub const TAGGING_UNRESOLVED_STATUS: u16 = 500;

/// The message that answer states: fixed text, naming neither the ARN asked for nor
/// the resource it addressed.
pub const TAGGING_UNRESOLVED_MESSAGE: &str = "Internal server error.";

/// The refusal a tagging call naming no stored resource is answered with — the one
/// construction of it, so the three operations on this surface answer identically.
pub fn tagging_unresolved<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_UNRESOLVED_CODE,
        TAGGING_UNRESOLVED_STATUS,
        TAGGING_UNRESOLVED_MESSAGE,
    )
}

/// What a tagging ARN addresses: the ids its resource field carries, read against
/// the one grammar the modelled `RtbTaggableResourceArn` pattern states
/// (`gateway/<gatewayId>`, optionally `/link/<linkId>`, optionally
/// `/routing-rule/<ruleId>`).
///
/// Which of these this store can actually hold is [`resource_tags`]'s table, the
/// same one [`resource_tags_mut`] writes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'a> {
    /// A requester or responder gateway: `gateway/<gatewayId>`.
    Gateway {
        /// The `gatewayId` the ARN names.
        gateway_id: &'a str,
    },
    /// A link of the store, of whichever kind:
    /// `gateway/<gatewayId>/link/<linkId>`. One grammar, because one ARN form names
    /// a link whether its far end is another gateway or an address outside the
    /// fabric.
    Link {
        /// The gateway the link is addressed through: either end of a link between
        /// two gateways, or the gateway an external link was created on.
        gateway_id: &'a str,
        /// The `linkId` the ARN names.
        link_id: &'a str,
    },
    /// A routing rule on a link:
    /// `gateway/<gatewayId>/link/<linkId>/routing-rule/<ruleId>`.
    LinkRoutingRule {
        /// One of the link's two ends.
        gateway_id: &'a str,
        /// The link the rule belongs to.
        link_id: &'a str,
        /// The `ruleId` the ARN names.
        rule_id: &'a str,
    },
}

/// The resource `resource_arn` addresses in the account and Region this request was
/// made against, or `None` when it addresses none.
///
/// The ARN is read through the one cross-service ARN parse, so the vendor field says
/// whose resource is named. A tagging call is answered about this emulator's own
/// account ([`::engine::emulator_account_id`]) in the Region the request addressed:
/// an ARN naming another partition, another account or another Region names a
/// resource this store never holds, which is the same answer as one naming a gateway
/// that was never created.
pub fn tagged_resource<'a>(resource_arn: &'a str, region: &str) -> Option<TaggedResource<'a>> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_VENDOR)?;
    if arn.partition != ARN_PARTITION
        || arn.region != region
        || arn.account != ::engine::emulator_account_id()
    {
        return None;
    }

    let segments: Vec<&str> = arn.resource.split('/').collect();
    Some(match segments[..] {
        [GATEWAY_SEGMENT, gateway_id] => TaggedResource::Gateway { gateway_id },
        [GATEWAY_SEGMENT, gateway_id, LINK_SEGMENT, link_id] => TaggedResource::Link {
            gateway_id,
            link_id,
        },
        [GATEWAY_SEGMENT, gateway_id, LINK_SEGMENT, link_id, ROUTING_RULE_SEGMENT, rule_id] => {
            TaggedResource::LinkRoutingRule {
                gateway_id,
                link_id,
                rule_id,
            }
        }
        _ => return None,
    })
}

/// The tags of the link `link_id` addressed through `gateway_id`, whichever of the
/// store's link records holds it.
///
/// One id names at most one link — every link id this service mints is unique within
/// the store — so the three link maps are asked in turn and the first that holds it
/// answers.
fn link_tags<'a>(
    state: &'a ComAmazonawsRtbfabricRtbfabricState,
    gateway_id: &str,
    link_id: &str,
) -> Option<&'a HashMap<String, String>> {
    if let Some(link) = state.find_link(gateway_id, link_id) {
        return Some(&link.tags);
    }
    if let Some(link) = state.find_outbound_external_link(gateway_id, link_id) {
        return Some(&link.tags);
    }
    state
        .find_inbound_external_link(gateway_id, link_id)
        .map(|link| &link.tags)
}

/// [`link_tags`]'s map, to be changed in place: the same three maps in the same
/// order, so a tag write and the read beside it address the same link record.
fn link_tags_mut<'a>(
    state: &'a mut ComAmazonawsRtbfabricRtbfabricState,
    gateway_id: &str,
    link_id: &str,
) -> Option<&'a mut HashMap<String, String>> {
    if state.find_link(gateway_id, link_id).is_some() {
        return state
            .find_link_mut(gateway_id, link_id)
            .map(|link| &mut link.tags);
    }
    if state
        .find_outbound_external_link(gateway_id, link_id)
        .is_some()
    {
        return state
            .find_outbound_external_link_mut(gateway_id, link_id)
            .map(|link| &mut link.tags);
    }
    state
        .find_inbound_external_link_mut(gateway_id, link_id)
        .map(|link| &mut link.tags)
}

/// The tags the addressed record holds, or `None` when this store holds no such
/// record.
///
/// The table of what can be addressed at all: a gateway of either kind, a link of
/// any kind the store holds — one between two gateways read from either of its two
/// ends, or an external link of either direction read through the gateway it was
/// created on — and a routing rule read through its own link from either of that
/// link's ends. Every record that carries a caller-assigned `tags` map is reachable
/// here, since a tag map is managed through this surface whichever call assigned it.
/// An ARN naming a record this store does not hold is answered exactly as an ARN
/// naming a gateway that was never created is.
pub fn resource_tags<'a>(
    state: &'a ComAmazonawsRtbfabricRtbfabricState,
    resource: TaggedResource<'_>,
) -> Option<&'a HashMap<String, String>> {
    match resource {
        TaggedResource::Gateway { gateway_id } => state
            .requester_gateways
            .get(gateway_id)
            .map(|gateway| &gateway.tags)
            .or_else(|| {
                state
                    .responder_gateways
                    .get(gateway_id)
                    .map(|gateway| &gateway.tags)
            }),
        TaggedResource::Link {
            gateway_id,
            link_id,
        } => link_tags(state, gateway_id, link_id),
        TaggedResource::LinkRoutingRule {
            gateway_id,
            link_id,
            rule_id,
        } => state
            .find_routing_rule(gateway_id, link_id, rule_id)
            .map(|rule| &rule.tags),
    }
}

/// The tags of the addressed record, to be changed in place; `None` when this store
/// holds no such record.
///
/// Addresses exactly the records [`resource_tags`] reports, so a tag write and the
/// read that follows it cannot disagree about which resource was named.
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsRtbfabricRtbfabricState,
    resource: TaggedResource<'_>,
) -> Option<&'a mut HashMap<String, String>> {
    match resource {
        TaggedResource::Gateway { gateway_id } => {
            // One id names a gateway of ONE kind, so the requester map is asked first
            // and the responder map only for an id it does not hold.
            match state.requester_gateways.get_mut(gateway_id) {
                Some(gateway) => Some(&mut gateway.tags),
                None => state
                    .responder_gateways
                    .get_mut(gateway_id)
                    .map(|gateway| &mut gateway.tags),
            }
        }
        TaggedResource::Link {
            gateway_id,
            link_id,
        } => link_tags_mut(state, gateway_id, link_id),
        TaggedResource::LinkRoutingRule {
            gateway_id,
            link_id,
            rule_id,
        } => state
            .find_routing_rule_mut(gateway_id, link_id, rule_id)
            .map(|rule| &mut rule.tags),
    }
}
