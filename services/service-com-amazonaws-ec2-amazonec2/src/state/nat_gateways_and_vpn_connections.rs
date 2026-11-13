// SPDX-License-Identifier: CC0-1.0

//! The secondary subnet and interface stores, the VPC block-public-access exclusions, the NAT
//! gateway, capacity-reservation fleet, customer gateway and VPN connection stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// EC2's error code for a request that named a secondary subnet id no secondary subnet of
/// the account has.
///
/// EC2 spells an id it cannot resolve `Invalid<Resource>ID.NotFound` —
/// [`INVALID_SUBNET_ID_NOT_FOUND`], [`INVALID_VPC_ID_NOT_FOUND`],
/// [`INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND`] — so the secondary subnet's own code follows
/// that form. EC2 models no error shapes at all, so it is answered through
/// [`crate::scaffolded::Scaffolded::service_error`] like every other code here.
pub const INVALID_SECONDARY_SUBNET_ID_NOT_FOUND: &str = "InvalidSecondarySubnetID.NotFound";

/// Refuse a request that named a secondary subnet id no secondary subnet of the account
/// has, the way [`invalid_subnet_id_not_found`] refuses an unknown subnet id: HTTP 400.
pub fn invalid_secondary_subnet_id_not_found<E>(secondary_subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SECONDARY_SUBNET_ID_NOT_FOUND,
        400,
        format!("The secondary subnet ID '{secondary_subnet_id}' does not exist"),
    )
}

/// The id prefix a secondary subnet carries.
///
/// The model publishes no `@pattern` for `SecondarySubnetId`
/// (`smithy_query(query="shape",shape="SecondarySubnetId")` reports a plain string), so it
/// publishes no spelling for one; the recorded wire corpus does. AWS answers
/// `secsub-00000000000000000` with `InvalidSecondarySubnetId.Malformed` rather than with the
/// `UnknownParameter`/`InvalidParameterValue` it answers a value it cannot place at all, and
/// the same recorded catalog probes `secnet-00000000000000000` and `afi-00000000000000000` —
/// prefixes EC2 certainly issues ([`SECONDARY_NETWORK_ID_PREFIX`]) — and gets each
/// resource's own `.Malformed` back too. What those probes share is the ALL-ZERO long-form
/// suffix, not an unknown prefix, so `secsub` is read as the prefix AWS issues for this
/// resource and the suffix as what makes the probe malformed (see
/// [`secondary_subnet_id_is_well_formed`]).
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints EC2's own
/// `<prefix>-<17 lowercase hex digits>` form under it, the way every other id of this store
/// is minted, so every id this store issues is one that reader accepts.
pub const SECONDARY_SUBNET_ID_PREFIX: &str = "secsub";

/// The id prefix of one IPv4 CIDR block association of a secondary subnet: CHOSEN, the model
/// publishing no pattern for `SecondarySubnetCidrAssociationId` either and no recording
/// probing one, and spelled as [`SECONDARY_SUBNET_ID_PREFIX`] extended by the association it
/// names — the way an ordinary subnet's CIDR associations are minted under
/// `subnet-cidr-assoc`.
pub const SECONDARY_SUBNET_CIDR_ASSOCIATION_ID_PREFIX: &str = "secsub-cidr-assoc";

/// The `ResourceType` a secondary subnet is named by — the resource part of its ARN, and the
/// word EC2 uses for the kind in the messages it builds about one, the way
/// [`SECONDARY_INTERFACE_RESOURCE`] is for a secondary interface and
/// [`SECONDARY_NETWORK_TAGGABLE_RESOURCE`] for a secondary network.
pub const SECONDARY_SUBNET_RESOURCE: &str = "secondary-subnet";

/// EC2's error code for a request naming something that is not a secondary subnet id at all,
/// in EC2's `Invalid<Resource>Id.Malformed` spelling — the code AWS answers before any
/// lookup, as the recorded wire corpus shows
/// (`secsub-00000000000000000` -> `InvalidSecondarySubnetId.Malformed`), and the same
/// spelling [`INVALID_SECONDARY_NETWORK_ID_MALFORMED`] carries.
pub const INVALID_SECONDARY_SUBNET_ID_MALFORMED: &str = "InvalidSecondarySubnetId.Malformed";

/// Whether `id` is a well-formed secondary subnet id — one this service could have ISSUED.
///
/// Read by the stricter of the two generic id readers
/// ([`issued_long_id_is_well_formed`]): the long EC2 resource id form only
/// (`<prefix>-<17 lowercase hex digits>`, leading `0`, not all `0`) under this resource's own
/// prefix ([`SECONDARY_SUBNET_ID_PREFIX`], whose doc carries the recorded evidence).
/// Secondary subnets are a resource EC2 only ever issued LONG ids for — there is no
/// eight-digit era behind them — and the all-zero suffix is what the recorded catalog shows
/// AWS calling malformed, under this prefix and under the other prefixes it probes beside it.
///
/// Anything else is [`INVALID_SECONDARY_SUBNET_ID_MALFORMED`] BEFORE any lookup, so a caller
/// can tell "not an id of this kind" from "no such secondary subnet".
pub fn secondary_subnet_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed(SECONDARY_SUBNET_ID_PREFIX, id)
}

/// Refuse something that is not a secondary subnet id at all: HTTP 400
/// [`INVALID_SECONDARY_SUBNET_ID_MALFORMED`], worded the way EC2 words a malformed id of a
/// resource kind ([`invalid_secondary_network_id_malformed`]) and built from the request's
/// own value.
pub fn invalid_secondary_subnet_id_malformed<E>(secondary_subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SECONDARY_SUBNET_ID_MALFORMED,
        400,
        format!("The {SECONDARY_SUBNET_RESOURCE} ID '{secondary_subnet_id}' is malformed"),
    )
}

/// Refuse a secondary subnet id the request named that this service cannot resolve.
///
/// The two questions EC2 asks in order, the way [`require_known_secondary_network`] asks them
/// of a secondary network id: is this an id of this resource kind at all
/// ([`secondary_subnet_id_is_well_formed`], answered
/// [`INVALID_SECONDARY_SUBNET_ID_MALFORMED`] before any lookup), and does the account hold a
/// secondary subnet with it ([`INVALID_SECONDARY_SUBNET_ID_NOT_FOUND`])? A caller branches on
/// the code, and the two mean different things: a malformed id is a permanent client bug,
/// while a missing one is a resource that is gone.
pub fn require_known_secondary_subnet<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    secondary_subnet_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !secondary_subnet_id_is_well_formed(secondary_subnet_id) {
        return Err(invalid_secondary_subnet_id_malformed(secondary_subnet_id));
    }
    if store.secondary_subnets.contains_key(secondary_subnet_id) {
        return Ok(());
    }
    Err(invalid_secondary_subnet_id_not_found(secondary_subnet_id))
}

/// The only secondary network type the model publishes: `SecondaryNetworkType`'s sole
/// member — "The type of the secondary network (`rdma`)"
/// (`DescribeSecondarySubnetsRequest$Filters`). The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_NETWORK_TYPE_VALUES`].
pub const SECONDARY_NETWORK_TYPE_RDMA: &str = "rdma";

/// A secondary subnet that has been created: `SecondarySubnetState`'s `create-complete`
/// member, the state a usable secondary subnet sits in. The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_SUBNET_STATE_VALUES`]; the states a creation or a
/// deletion passes through belong to the operations that perform them.
pub const SECONDARY_SUBNET_STATE_CREATE_COMPLETE: &str = "create-complete";

/// An IPv4 CIDR block that is associated with its secondary subnet:
/// `SecondarySubnetCidrBlockAssociationState`'s `associated` member. The whole catalog is
/// the generated [`crate::ops::types::SECONDARY_SUBNET_CIDR_BLOCK_ASSOCIATION_STATE_VALUES`].
pub const SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED: &str = "associated";

/// A secondary subnet AWS has been asked to delete and has not finished deleting:
/// `SecondarySubnetState`'s `delete-in-progress` member, the non-terminal state
/// `DeleteSecondarySubnet` answers with.
pub const SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A secondary subnet AWS has finished deleting: `SecondarySubnetState`'s `delete-complete`
/// member.
///
/// A subnet in either delete state is still REPORTED: `DescribeSecondarySubnets` documents
/// `delete-in-progress` and `delete-complete` among the values of its `state` filter
/// (`smithy_query(query="doc",shape="DescribeSecondarySubnetsRequest",member="Filters")`),
/// which no report could ever select on if a deleted subnet left the account's subnets.
pub const SECONDARY_SUBNET_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The IPv4 CIDR block of a secondary subnet that is being deleted:
/// `SecondarySubnetCidrBlockAssociationState`'s `disassociating` member.
pub const SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATING: &str = "disassociating";

/// The IPv4 CIDR block of a secondary subnet whose deletion has finished:
/// `SecondarySubnetCidrBlockAssociationState`'s `disassociated` member.
pub const SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATED: &str = "disassociated";

/// One IPv4 CIDR block of a secondary subnet, as the writer of one asks for it.
///
/// The association id is NOT here: it is minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`], the way every other stored
/// id is, so no writer can choose one.
#[derive(Debug, Clone)]
pub struct SecondarySubnetCidrSpec {
    /// "The IPv4 CIDR block" (`SecondarySubnetIpv4CidrBlockAssociation$CidrBlock`).
    pub cidr_block: String,
    /// The state of the association, as the model's
    /// `SecondarySubnetCidrBlockAssociationState` enum spells it — see
    /// [`SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED`].
    pub state: String,
    /// "The reason for the current state of the CIDR block association"
    /// (`SecondarySubnetIpv4CidrBlockAssociation$StateReason`), or `None` for an
    /// association that carries none.
    pub state_reason: Option<String>,
}

/// What one stored secondary subnet is created with, beside the ids the store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for an ordinary subnet: a
/// writer fills this in and [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`]
/// turns it into the modelled [`SecondarySubnet`] the describe reports, so the ids, the
/// ARN, the owning account and the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct SecondarySubnetSpec {
    /// "The ID of the secondary network" the subnet belongs to
    /// (`SecondarySubnet$SecondaryNetworkId`).
    pub secondary_network_id: String,
    /// "The type of the secondary network" (`SecondarySubnet$SecondaryNetworkType`), as the
    /// model's `SecondaryNetworkType` enum spells it — see
    /// [`SECONDARY_NETWORK_TYPE_RDMA`].
    pub secondary_network_type: String,
    /// "The Availability Zone of the secondary subnet"
    /// (`SecondarySubnet$AvailabilityZone`).
    pub availability_zone: String,
    /// "The ID of the Availability Zone of the secondary subnet"
    /// (`SecondarySubnet$AvailabilityZoneId`).
    pub availability_zone_id: String,
    /// The Region the subnet's ARN names. Not a member of the shape: it is what
    /// `engine::emulator_arn` needs to build `SecondarySubnetArn`.
    pub region: String,
    /// The IPv4 CIDR blocks associated with the subnet, in the order they are reported.
    pub ipv4_cidr_blocks: Vec<SecondarySubnetCidrSpec>,
    /// The subnet's state, as the model's `SecondarySubnetState` enum spells it — see
    /// [`SECONDARY_SUBNET_STATE_CREATE_COMPLETE`].
    pub state: String,
    /// "The reason for the current state of the secondary subnet"
    /// (`SecondarySubnet$StateReason`), or `None` for a subnet that carries none.
    pub state_reason: Option<String>,
    /// "The tags assigned to the secondary subnet" (`SecondarySubnet$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Is there room for another secondary subnet in this store
    /// ([`MAX_STORED_SECONDARY_SUBNETS`])?
    ///
    /// A create asks this so it can refuse with the quota code instead of overshooting the
    /// ceiling; [`Self::insert_secondary_subnet`] asks it again, so a writer that forgets
    /// the gate still cannot grow the store past it.
    pub fn has_secondary_subnet_capacity(&self) -> bool {
        self.secondary_subnets.len() < MAX_STORED_SECONDARY_SUBNETS
    }

    /// Store a secondary subnet, and answer the `SecondarySubnetId` it was given — or
    /// `None` when this store is already at [`MAX_STORED_SECONDARY_SUBNETS`], which a
    /// handler answers with [`secondary_subnet_limit_exceeded`].
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the subnet's id, the ARN built from it, the owning account, and the
    /// association id of each IPv4 CIDR block — so two writers cannot report the same
    /// secondary subnet differently. Like [`Self::insert_subnet`], the caller-chosen bytes
    /// the record holds are charged against the store's byte budget here — by
    /// [`secondary_subnet_payload_bytes`], which is the one definition of that charge and
    /// already carries the fixed per-record overhead, so a tag-free subnet is not free
    /// against the budget either — and the ceiling is checked BEFORE any id is minted, so a
    /// refused write consumes nothing at all.
    pub fn insert_secondary_subnet(&mut self, spec: SecondarySubnetSpec) -> Option<String> {
        if !self.has_secondary_subnet_capacity() {
            return None;
        }
        let secondary_subnet_id = self.next_id(SECONDARY_SUBNET_ID_PREFIX);
        let mut ipv4_cidr_block_associations = Vec::with_capacity(spec.ipv4_cidr_blocks.len());
        for block in spec.ipv4_cidr_blocks {
            let association_id = self.next_id(SECONDARY_SUBNET_CIDR_ASSOCIATION_ID_PREFIX);
            ipv4_cidr_block_associations.push(SecondarySubnetIpv4CidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(block.cidr_block),
                state: Some(block.state),
                state_reason: block.state_reason,
            });
        }
        let secondary_subnet = SecondarySubnet {
            secondary_subnet_id: Some(secondary_subnet_id.clone()),
            secondary_subnet_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{SECONDARY_SUBNET_RESOURCE}/{secondary_subnet_id}"),
            )),
            secondary_network_id: Some(spec.secondary_network_id),
            secondary_network_type: Some(spec.secondary_network_type),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            availability_zone_id: Some(spec.availability_zone_id),
            availability_zone: Some(spec.availability_zone),
            ipv4_cidr_block_associations: Some(ipv4_cidr_block_associations),
            state: Some(spec.state),
            state_reason: spec.state_reason,
            tags: Some(spec.tags),
        };
        // `secondary_subnet_payload_bytes` is the ONE definition of this charge and
        // already carries the fixed per-record overhead, so the call site adds nothing to
        // it: a removal path gives back exactly this number.
        self.add_stored_bytes(secondary_subnet_payload_bytes(&secondary_subnet));
        self.secondary_subnets
            .insert(secondary_subnet_id.clone(), secondary_subnet);
        Some(secondary_subnet_id)
    }

    /// What still depends on the secondary subnet `secondary_subnet_id`, as the ids of the
    /// depending resources, in id order.
    ///
    /// "A secondary subnet must not contain any secondary interfaces prior to deletion"
    /// (`DeleteSecondarySubnet`): an interface created IN the subnet holds a place in it the
    /// same way a network interface holds a place in an ordinary subnet
    /// ([`Self::subnet_dependencies`]), whether or not an instance currently holds the
    /// interface — the caller has to take the interface away first either way, so the status
    /// of its attachment is not what decides this.
    pub fn secondary_subnet_dependencies(&self, secondary_subnet_id: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .secondary_interfaces
            .iter()
            .filter(|(_, interface)| {
                interface.secondary_subnet_id.as_deref() == Some(secondary_subnet_id)
            })
            .map(|(secondary_interface_id, _)| secondary_interface_id.clone())
            .collect();
        found.sort();
        found
    }

    /// Start deleting the secondary subnet `secondary_subnet_id`, and answer it as the store
    /// then holds it — `None` when the account holds no such subnet.
    ///
    /// The deletion is a CLOCK-OWED move: the subnet is
    /// [`SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS`] with each of its IPv4 CIDR block
    /// associations [`SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATING`] until
    /// `delete_complete_at`, which is STORED rather than slept for, so a caller reading
    /// straight after the delete still sees the subnet going down;
    /// [`Self::materialize_due_secondary_subnet_deletions`] applies the terminal states once
    /// that instant has passed, whether or not anybody called again.
    ///
    /// The record STAYS in the store, because a subnet in either delete state is still
    /// reported ([`SECONDARY_SUBNET_STATE_DELETE_COMPLETE`]) — so its caller-chosen bytes
    /// stay charged to the store's budget, which still holds them, and it still counts
    /// against [`MAX_STORED_SECONDARY_SUBNETS`], which bounds the records this process holds.
    ///
    /// A subnet whose deletion has already been asked for keeps the states and the due
    /// instant that first delete gave it: a repeat of the call reports the deletion already
    /// under way, or the one that has finished, instead of restarting its clock.
    pub fn begin_secondary_subnet_deletion(
        &mut self,
        secondary_subnet_id: &str,
        delete_complete_at: Instant,
    ) -> Option<SecondarySubnet> {
        let subnet = self.secondary_subnets.get_mut(secondary_subnet_id)?;
        let deleting = matches!(
            subnet.state.as_deref(),
            Some(SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS | SECONDARY_SUBNET_STATE_DELETE_COMPLETE)
        );
        if !deleting {
            subnet.state = Some(SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS.to_string());
            for association in subnet.ipv4_cidr_block_associations.iter_mut().flatten() {
                association.state = Some(SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATING.to_string());
            }
            let subnet = subnet.clone();
            self.secondary_subnet_delete_complete_at
                .insert(secondary_subnet_id.to_string(), delete_complete_at);
            return Some(subnet);
        }
        Some(subnet.clone())
    }

    /// Apply the deletions of secondary subnets the clock already owes: a subnet a
    /// `DeleteSecondarySubnet` left [`SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS`] reached
    /// [`SECONDARY_SUBNET_STATE_DELETE_COMPLETE`], with each of its IPv4 CIDR block
    /// associations [`SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATED`], at the instant that call
    /// recorded for it — whether or not anybody called again.
    ///
    /// Called by [`Self::materialize_due_transitions`], the one place this store's clock-owed
    /// moves are applied, so every operation that materialises them gets this one too.
    pub(super) fn materialize_due_secondary_subnet_deletions(&mut self, now: Instant) {
        let secondary_subnets = &mut self.secondary_subnets;
        self.secondary_subnet_delete_complete_at
            .retain(|secondary_subnet_id, due_at| {
                if now < *due_at {
                    return true;
                }
                if let Some(subnet) = secondary_subnets.get_mut(secondary_subnet_id) {
                    if subnet.state.as_deref() == Some(SECONDARY_SUBNET_STATE_DELETE_IN_PROGRESS) {
                        subnet.state = Some(SECONDARY_SUBNET_STATE_DELETE_COMPLETE.to_string());
                        for association in
                            subnet.ipv4_cidr_block_associations.iter_mut().flatten()
                        {
                            association.state =
                                Some(SECONDARY_SUBNET_CIDR_STATE_DISASSOCIATED.to_string());
                        }
                    }
                }
                // The transition is applied once; nothing is owed on this subnet any more.
                false
            });
    }
}

/// How many secondary subnets this emulator stores before a write of another is refused.
///
/// CHOSEN, like [`MAX_SECONDARY_NETWORKS`] and [`MAX_STORED_RECORDS_PER_STORE`] and for
/// the same
/// reason the module gives for every other ceiling: nothing in this emulator reclaims a
/// stored secondary subnet, so this store needs a ceiling that is not the process's memory.
/// It is the same ceiling the other record stores keep.
pub const MAX_STORED_SECONDARY_SUBNETS: usize = MAX_STORED_RECORDS_PER_STORE;

/// Refuse a create that would take the store past [`MAX_STORED_SECONDARY_SUBNETS`]: HTTP
/// 400, with the same quota code family a secondary network's own ceiling answers with.
pub fn secondary_subnet_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(SECONDARY_NETWORK_LIMIT_EXCEEDED, 400, message)
}

/// The bytes one stored secondary subnet is charged: its tags, the CIDR block of each
/// association, and the state reasons, plus [`STORED_RECORD_OVERHEAD_BYTES`] for the
/// record itself.
/// The ids, the ARN, the owning account, the states and the network type are derived by
/// this module or bounded by the model, so they are fixed-size.
///
/// The fixed overhead is what makes a subnet written with no tags, no IPv4 CIDR block and
/// no state reason cost something: a count ceiling alone leaves a zero-payload record free
/// against [`MAX_STORED_BYTES`], so repeated writes would advance no budget at all.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a VPC endpoint connection: a writer charges exactly it, and a removal path must give
/// back the same number.
pub fn secondary_subnet_payload_bytes(secondary_subnet: &SecondarySubnet) -> usize {
    let association_bytes: usize = secondary_subnet
        .ipv4_cidr_block_associations
        .iter()
        .flatten()
        .map(|association| {
            association.cidr_block.as_deref().unwrap_or_default().len()
                + association.state_reason.as_deref().unwrap_or_default().len()
        })
        .sum();
    tag_list_bytes(secondary_subnet.tags.as_deref().unwrap_or_default())
        .saturating_add(association_bytes)
        .saturating_add(
            secondary_subnet
                .state_reason
                .as_deref()
                .unwrap_or_default()
                .len(),
        )
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

// ---------------------------------------------------------------------------
// Secondary interfaces
// ---------------------------------------------------------------------------

/// EC2's error code for a request that named a secondary interface id no secondary interface
/// of the account has.
///
/// EC2 spells an id it cannot resolve `Invalid<Resource>ID.NotFound` —
/// [`INVALID_SECONDARY_SUBNET_ID_NOT_FOUND`], [`INVALID_SUBNET_ID_NOT_FOUND`],
/// [`INVALID_VPC_ID_NOT_FOUND`] — so the secondary interface's own code follows that form.
/// EC2 models no error shapes at all, so it is answered through
/// [`crate::scaffolded::Scaffolded::service_error`] like every other code here.
pub const INVALID_SECONDARY_INTERFACE_ID_NOT_FOUND: &str = "InvalidSecondaryInterfaceID.NotFound";

/// Refuse a request that named a secondary interface id no secondary interface of the
/// account has, the way [`invalid_secondary_subnet_id_not_found`] refuses an unknown
/// secondary subnet id: HTTP 400.
pub fn invalid_secondary_interface_id_not_found<E>(secondary_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SECONDARY_INTERFACE_ID_NOT_FOUND,
        400,
        format!("The secondary interface ID '{secondary_interface_id}' does not exist"),
    )
}

/// The id prefix a secondary interface carries.
///
/// CHOSEN, on the same terms as [`SECONDARY_SUBNET_ID_PREFIX`]: the model publishes no
/// `@pattern` for `SecondaryInterfaceId`
/// (`smithy_query(query="shape",shape="SecondaryInterfaceId")` reports a plain string), so
/// the only form it has to satisfy is EC2's own `<prefix>-<hex>`, which
/// [`ComAmazonawsEc2Amazonec2State::next_id`] gives. The prefix follows the elastic network
/// interface's [`NETWORK_INTERFACE_ID_PREFIX`] with the resource's `s`econdary initial, the
/// way the secondary subnet's own prefix follows a subnet's.
pub const SECONDARY_INTERFACE_ID_PREFIX: &str = "seni";

/// The id prefix of the attachment of a secondary interface to an instance, which is what
/// `SecondaryInterfaceAttachment$AttachmentId` names. CHOSEN on the same terms as
/// [`SECONDARY_INTERFACE_ID_PREFIX`], following the ordinary interface attachment's
/// [`NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX`].
pub const SECONDARY_INTERFACE_ATTACHMENT_ID_PREFIX: &str = "seni-attach";

/// The only secondary interface type the model publishes: `SecondaryInterfaceType`'s sole
/// member — "The type of secondary interface (`secondary`)"
/// (`DescribeSecondaryInterfacesRequest$Filters`). The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_INTERFACE_TYPE_VALUES`], so an interface's type is derived
/// from it rather than chosen by a writer.
pub const SECONDARY_INTERFACE_TYPE_SECONDARY: &str = "secondary";

/// The status of a secondary interface nothing holds: `SecondaryInterfaceStatus`'s
/// `available` member — "The status of the secondary interface (`available` | `in-use`)"
/// (`DescribeSecondaryInterfacesRequest$Filters`). The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_INTERFACE_STATUS_VALUES`].
pub const SECONDARY_INTERFACE_STATUS_AVAILABLE: &str = "available";

/// The status of a secondary interface an instance holds: `SecondaryInterfaceStatus`'s
/// `in-use` member, where an interface with an attachment that has not been taken away sits.
pub const SECONDARY_INTERFACE_STATUS_IN_USE: &str = "in-use";

/// The `ResourceType` a secondary interface is named by — the resource part of its ARN, and
/// the word EC2 uses for the kind in its messages.
pub const SECONDARY_INTERFACE_RESOURCE: &str = "secondary-interface";

/// How many secondary interfaces this emulator stores before a write of another is refused.
///
/// CHOSEN, like [`MAX_STORED_SECONDARY_SUBNETS`] and for the reason the module gives for
/// every other ceiling: nothing in this emulator reclaims a stored secondary interface, so
/// this store needs a ceiling that is not the process's memory.
pub const MAX_STORED_SECONDARY_INTERFACES: usize = MAX_STORED_RECORDS_PER_STORE;

/// Refuse a create that would take the store past [`MAX_STORED_SECONDARY_INTERFACES`]: HTTP
/// 400, with the same quota code family a secondary network's own ceiling answers with.
pub fn secondary_interface_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(SECONDARY_NETWORK_LIMIT_EXCEEDED, 400, message)
}

/// What one attachment of a secondary interface to an instance is created with, beside the
/// ids and the owning account the store derives itself.
///
/// The attachment id is NOT here: it is minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_secondary_interface`], the way every other stored
/// id is, so no writer can choose one.
#[derive(Debug, Clone)]
pub struct SecondaryInterfaceAttachmentSpec {
    /// "The ID of the instance to which the secondary interface is attached"
    /// (`DescribeSecondaryInterfacesRequest$Filters`), which
    /// `SecondaryInterfaceAttachment$InstanceId` reports.
    pub instance_id: String,
    /// "The device index of the secondary interface"
    /// (`SecondaryInterfaceAttachment$DeviceIndex`).
    pub device_index: i32,
    /// The index of the network card the interface is on
    /// (`SecondaryInterfaceAttachment$NetworkCardIndex`).
    pub network_card_index: i32,
    /// Whether the interface is deleted when the instance terminates
    /// (`SecondaryInterfaceAttachment$DeleteOnTermination`).
    pub delete_on_termination: bool,
    /// "The attachment status (`attaching` | `attached` | `detaching` | `detached`)"
    /// (`DescribeSecondaryInterfacesRequest$Filters`), as the model's `AttachmentStatus` enum
    /// spells it — see [`ATTACHMENT_STATUS_ATTACHED`] and [`ATTACHMENT_STATUS_DETACHED`].
    pub status: String,
    /// When the interface was attached (`SecondaryInterfaceAttachment$AttachTime`), as a
    /// [`SystemTime`] the store renders into the modelled timestamp.
    pub attach_time: SystemTime,
}

/// What one stored secondary interface is created with, beside the facts the store derives
/// itself.
///
/// The spec of the resource, in the same shape [`SecondarySubnetSpec`] has for a secondary
/// subnet: a writer fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_secondary_interface`] turns it into the modelled
/// [`SecondaryInterface`] the describe reports, so the ids, the ARN, the MAC address, the
/// owning account, the interface type, the status and the byte charge are decided in exactly
/// one place.
#[derive(Debug, Clone)]
pub struct SecondaryInterfaceSpec {
    /// "The ID of the secondary subnet" the interface is in
    /// (`SecondaryInterface$SecondarySubnetId`).
    pub secondary_subnet_id: String,
    /// "The ID of the secondary network" the interface belongs to
    /// (`SecondaryInterface$SecondaryNetworkId`).
    pub secondary_network_id: String,
    /// "The type of the secondary network" (`SecondaryInterface$SecondaryNetworkType`), as
    /// the model's `SecondaryNetworkType` enum spells it — see
    /// [`SECONDARY_NETWORK_TYPE_RDMA`].
    pub secondary_network_type: String,
    /// "The Availability Zone of the secondary interface"
    /// (`SecondaryInterface$AvailabilityZone`).
    pub availability_zone: String,
    /// "The ID of the Availability Zone of the secondary interface"
    /// (`SecondaryInterface$AvailabilityZoneId`).
    pub availability_zone_id: String,
    /// The Region the interface's ARN names. Not a member of the shape: it is what
    /// `engine::emulator_arn` needs to build `SecondaryInterfaceArn`.
    pub region: String,
    /// "The private IPv4 address associated with the secondary interface"
    /// (`DescribeSecondaryInterfacesRequest$Filters`), in the order they are reported, and
    /// empty for an interface that holds none.
    pub private_ipv4_addresses: Vec<String>,
    /// "Indicates whether source/destination checking is enabled"
    /// (`SecondaryInterface$SourceDestCheck`) — [`DEFAULT_SOURCE_DEST_CHECK`] unless the
    /// writer turned it off.
    pub source_dest_check: bool,
    /// "The secondary interface attachment" (`SecondaryInterface$Attachment`), for an
    /// interface an instance holds; `None` for one attached to nothing. The `Status` the
    /// store derives follows from it, so a stored interface cannot claim to be `available`
    /// while an instance holds it.
    pub attachment: Option<SecondaryInterfaceAttachmentSpec>,
    /// "The tags assigned to the secondary interface" (`SecondaryInterface$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Is there room for another secondary interface in this store
    /// ([`MAX_STORED_SECONDARY_INTERFACES`])?
    ///
    /// A create asks this so it can refuse with the quota code instead of overshooting the
    /// ceiling; [`Self::insert_secondary_interface`] asks it again, so a writer that forgets
    /// the gate still cannot grow the store past it.
    pub fn has_secondary_interface_capacity(&self) -> bool {
        self.secondary_interfaces.len() < MAX_STORED_SECONDARY_INTERFACES
    }

    /// Store a secondary interface, and answer the `SecondaryInterfaceId` it was given — or
    /// `None` when this store is already at [`MAX_STORED_SECONDARY_INTERFACES`], which a
    /// handler answers with [`secondary_interface_limit_exceeded`].
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the interface's id, the ARN and the MAC address built from it, the owning
    /// account, the id of its attachment, the one `SecondaryInterfaceType` the model
    /// publishes, and the `available`/`in-use` status that follows from the attachment — so
    /// two writers cannot report the same interface differently. Like
    /// [`Self::insert_secondary_subnet`], the caller-chosen bytes the record holds are
    /// charged against the store's byte budget here, by
    /// [`secondary_interface_payload_bytes`], which is the one definition of that charge and
    /// already carries the fixed per-record overhead, so a tag-free interface is not free
    /// against the budget either — and the ceiling is checked BEFORE any id is minted, so a
    /// refused write consumes nothing at all.
    pub fn insert_secondary_interface(&mut self, spec: SecondaryInterfaceSpec) -> Option<String> {
        if !self.has_secondary_interface_capacity() {
            return None;
        }
        let secondary_interface_id = self.next_id(SECONDARY_INTERFACE_ID_PREFIX);
        // The MAC address is derived from the store's counter, the way an elastic network
        // interface's is, so it is the same across reads and no two interfaces share one.
        let mac_address = mac_address_for(self.id_counter);
        let attachment = spec.attachment.map(|attachment| SecondaryInterfaceAttachment {
            attachment_id: Some(self.next_id(SECONDARY_INTERFACE_ATTACHMENT_ID_PREFIX)),
            attach_time: Some(system_time_to_date_time(attachment.attach_time)),
            delete_on_termination: Some(attachment.delete_on_termination),
            device_index: Some(attachment.device_index),
            instance_id: Some(attachment.instance_id),
            // The instance an interface of this account is attached to is this account's
            // own: nothing in this emulator attaches one across accounts.
            instance_owner_id: Some(::engine::emulator_account_id().to_string()),
            network_card_index: Some(attachment.network_card_index),
            status: Some(attachment.status),
        });
        // "The status of the secondary interface (`available` | `in-use`)": an interface an
        // instance holds is `in-use`, one attached to nothing is `available`, and an
        // attachment that has reached `detached` holds nothing. Derived here rather than
        // taken from the writer, for the reason
        // [`Self::insert_network_interface`] derives an elastic network interface's.
        let held = attachment
            .as_ref()
            .is_some_and(|attachment| attachment.status.as_deref() != Some(ATTACHMENT_STATUS_DETACHED));
        let status = if held {
            SECONDARY_INTERFACE_STATUS_IN_USE
        } else {
            SECONDARY_INTERFACE_STATUS_AVAILABLE
        };
        let secondary_interface = SecondaryInterface {
            secondary_interface_id: Some(secondary_interface_id.clone()),
            secondary_interface_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{SECONDARY_INTERFACE_RESOURCE}/{secondary_interface_id}"),
            )),
            // The model publishes exactly one `SecondaryInterfaceType`, so the type of an
            // interface is a fact of the kind rather than something a writer picks.
            secondary_interface_type: Some(SECONDARY_INTERFACE_TYPE_SECONDARY.to_string()),
            secondary_subnet_id: Some(spec.secondary_subnet_id),
            secondary_network_id: Some(spec.secondary_network_id),
            secondary_network_type: Some(spec.secondary_network_type),
            availability_zone: Some(spec.availability_zone),
            availability_zone_id: Some(spec.availability_zone_id),
            mac_address: Some(mac_address),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            private_ipv4_addresses: Some(
                spec.private_ipv4_addresses
                    .into_iter()
                    .map(|address| SecondaryInterfaceIpv4Address {
                        private_ip_address: Some(address),
                    })
                    .collect(),
            ),
            source_dest_check: Some(spec.source_dest_check),
            status: Some(status.to_string()),
            attachment,
            tags: Some(spec.tags),
        };
        // `secondary_interface_payload_bytes` is the ONE definition of this charge and
        // already carries the fixed per-record overhead, so the call site adds nothing to
        // it: a removal path gives back exactly this number.
        self.add_stored_bytes(secondary_interface_payload_bytes(&secondary_interface));
        self.secondary_interfaces
            .insert(secondary_interface_id.clone(), secondary_interface);
        Some(secondary_interface_id)
    }
}

/// The bytes one stored secondary interface is charged: its tags, each private IPv4 address
/// it holds and the id of the instance its attachment names, plus
/// [`STORED_RECORD_OVERHEAD_BYTES`] for the record itself. The ids this module mints, the
/// ARN, the MAC address, the owning account, the statuses and the network type are derived
/// here or bounded by the model, so they are fixed-size.
///
/// The fixed overhead is what makes an interface written with no tags and no address cost
/// something: a count ceiling alone leaves a zero-payload record free against
/// [`MAX_STORED_BYTES`], so repeated writes would advance no budget at all.
///
/// The ONE definition of that charge, the way [`secondary_subnet_payload_bytes`] is for a
/// secondary subnet: a writer charges exactly it, and a removal path must give back the same
/// number.
pub fn secondary_interface_payload_bytes(secondary_interface: &SecondaryInterface) -> usize {
    let address_bytes: usize = secondary_interface
        .private_ipv4_addresses
        .iter()
        .flatten()
        .map(|address| address.private_ip_address.as_deref().unwrap_or_default().len())
        .sum();
    let attachment_bytes = secondary_interface
        .attachment
        .as_ref()
        .and_then(|attachment| attachment.instance_id.as_deref())
        .map_or(0, str::len);
    tag_list_bytes(secondary_interface.tags.as_deref().unwrap_or_default())
        .saturating_add(address_bytes)
        .saturating_add(attachment_bytes)
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

// ---------------------------------------------------------------------------
// VPC Block Public Access exclusions
// ---------------------------------------------------------------------------

/// The id prefix of a VPC BPA exclusion, as AWS mints one: `vpcbpa-exclude-<hex>`.
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const VPC_BPA_EXCLUSION_ID_PREFIX: &str = "vpcbpa-exclude";

/// An exclusion AWS is still putting in place: the `VpcBlockPublicAccessExclusionState`
/// enum's `create-in-progress` member, which is what
/// `CreateVpcBlockPublicAccessExclusion` answers with until [`TRANSITION_DELAY`] has
/// passed.
pub const VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// An exclusion that is in place: the `VpcBlockPublicAccessExclusionState` enum's
/// `create-complete` member, the state the clock owes a `create-in-progress` one.
pub const VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE: &str = "create-complete";

/// The `ResourceType` a `CreateVpcBlockPublicAccessExclusion` tag specification may name:
/// the exclusion it creates (the modelled `ResourceType` enum's
/// `vpc-block-public-access-exclusion` member).
pub const RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION: &str =
    "vpc-block-public-access-exclusion";

/// The largest number of VPC BPA exclusions this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each exclusion is
/// a record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_vpc_bpa_exclusion_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`vpc_bpa_exclusion_limit_exceeded`].
pub const MAX_VPC_BPA_EXCLUSIONS: usize = 1_000;

/// The code a create past [`MAX_VPC_BPA_EXCLUSIONS`] answers with.
///
/// EC2 names every per-resource quota refusal `<Resource>LimitExceeded`
/// ([`VPC_LIMIT_EXCEEDED`], [`SUBNET_LIMIT_EXCEEDED`], [`SECURITY_GROUP_LIMIT_EXCEEDED`],
/// [`ADDRESS_LIMIT_EXCEEDED`]), and EC2's model declares no error shapes at all, so this
/// resource's code follows that convention.
pub const VPC_BPA_EXCLUSION_LIMIT_EXCEEDED: &str = "VpcBlockPublicAccessExclusionLimitExceeded";

/// The code EC2 answers for an exclusion id the account holds no exclusion under.
///
/// The same `Invalid<Resource>Id.NotFound` convention as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`].
pub const INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND: &str =
    "InvalidVpcBlockPublicAccessExclusionId.NotFound";

/// What a VPC BPA exclusion is applied to: "a mode that can be applied to a single VPC or
/// subnet" (`VpcBlockPublicAccessExclusion`).
///
/// The modelled shape carries no member for it — the exclusion reports its own ARN, not
/// the excluded resource's id — so it is stored beside the record under the same id, the
/// way [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] holds what a [`Vpc`] has no member
/// for. Which resource an exclusion covers is what makes a second exclusion for the same
/// VPC or subnet a conflict, and what a delete has to know to release it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionTarget {
    /// "A VPC ID" (`CreateVpcBlockPublicAccessExclusionRequest$VpcId`).
    Vpc(String),
    /// "A subnet ID" (`CreateVpcBlockPublicAccessExclusionRequest$SubnetId`).
    Subnet(String),
}

impl ExclusionTarget {
    /// The id of the excluded VPC or subnet.
    pub fn resource_id(&self) -> &str {
        match self {
            Self::Vpc(id) | Self::Subnet(id) => id,
        }
    }

    /// The modelled `ResourceType` word for the excluded resource, so a refusal can name
    /// what it is as well as which.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::Vpc(_) => RESOURCE_TYPE_VPC,
            Self::Subnet(_) => RESOURCE_TYPE_SUBNET,
        }
    }
}

/// The move the clock owes one exclusion: a [`ClockTransition`] to the
/// `VpcBlockPublicAccessExclusionState` member it becomes, reported as the exclusion's
/// `LastUpdateTimestamp`.
///
/// The `to` is a `&'static str` so only a word from this module's own catalog constants can
/// be stored — never a request's text.
pub type ExclusionTransition = ClockTransition<&'static str>;

/// One VPC BPA exclusion: the modelled [`VpcBlockPublicAccessExclusion`] every operation
/// that reports one answers with, plus the two facts that shape carries no member for.
///
/// `CreateVpcBlockPublicAccessExclusion`, `DescribeVpcBlockPublicAccessExclusions`,
/// `ModifyVpcBlockPublicAccessExclusion` and `DeleteVpcBlockPublicAccessExclusion` all
/// answer with that one shape, so the one stored view is what all of them report and they
/// cannot describe one exclusion differently.
#[derive(Debug, Clone)]
pub struct VpcBlockPublicAccessExclusionRecord {
    /// The exclusion as the model shapes it, carrying the state reached so far.
    pub view: VpcBlockPublicAccessExclusion,
    /// The VPC or subnet this exclusion is applied to.
    pub target: ExclusionTarget,
    /// The move the clock owes this exclusion, while one is owed.
    pub pending: Option<ExclusionTransition>,
}

impl VpcBlockPublicAccessExclusionRecord {
    /// The exclusion's `VpcBlockPublicAccessExclusionState` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// The exclusion's own id.
    pub fn exclusion_id(&self) -> &str {
        self.view.exclusion_id.as_deref().unwrap_or_default()
    }
}

/// What a writer must say to store a VPC BPA exclusion; everything else about the record
/// is derived by [`ComAmazonawsEc2Amazonec2State::insert_vpc_block_public_access_exclusion`].
#[derive(Debug, Clone)]
pub struct VpcBlockPublicAccessExclusionSpec {
    /// The VPC or subnet the exclusion is applied to.
    pub target: ExclusionTarget,
    /// "The exclusion mode for internet gateway traffic"
    /// (`VpcBlockPublicAccessExclusion$InternetGatewayExclusionMode`), as a `&'static str`
    /// out of the modelled catalog ([`internet_gateway_exclusion_mode`]) so the stored
    /// word is never the request's own text.
    pub internet_gateway_exclusion_mode: &'static str,
    /// The Region the request was signed for, which the exclusion's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the exclusion.
    pub tags: Vec<Tag>,
}

/// The modelled `InternetGatewayExclusionMode` member `requested` names, or `None` when it
/// names none.
///
/// The catalog is the generated [`crate::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES`],
/// so this crate holds no second copy of it. Answering a `&'static str` out of that catalog
/// is what keeps request text out of the store.
pub fn internet_gateway_exclusion_mode(requested: &str) -> Option<&'static str> {
    crate::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode == requested)
}

/// Refuse a create that would take the store past [`MAX_VPC_BPA_EXCLUSIONS`]: HTTP 400
/// [`VPC_BPA_EXCLUSION_LIMIT_EXCEEDED`].
pub fn vpc_bpa_exclusion_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_BPA_EXCLUSION_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of VPC Block Public Access exclusions has been reached \
             ({MAX_VPC_BPA_EXCLUSIONS})"
        ),
    )
}

/// Refuse an exclusion id a request named that the account holds no exclusion under: HTTP
/// 400 [`INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND`].
pub fn invalid_vpc_bpa_exclusion_id_not_found<E>(exclusion_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND,
        400,
        format!("The VPC Block Public Access exclusion ID '{exclusion_id}' does not exist"),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another VPC BPA exclusion fits inside the store's ceiling
    /// ([`MAX_VPC_BPA_EXCLUSIONS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`vpc_bpa_exclusion_limit_exceeded`] when it answers `false`.
    pub fn has_vpc_bpa_exclusion_capacity(&self) -> bool {
        self.vpc_block_public_access_exclusions.len() < MAX_VPC_BPA_EXCLUSIONS
    }

    /// The id of the exclusion already applied to `target`, if the account holds one.
    ///
    /// An exclusion is "a mode that can be applied to a single VPC or subnet"
    /// (`VpcBlockPublicAccessExclusion`), so one resource carries one mode: a second
    /// exclusion for the same VPC or subnet would leave two modes in force on it with no
    /// rule saying which wins. A create asks this first and refuses rather than storing a
    /// conflict.
    pub fn vpc_bpa_exclusion_for_target(&self, target: &ExclusionTarget) -> Option<&str> {
        self.vpc_block_public_access_exclusions
            .values()
            .find(|record| record.target == *target)
            .map(VpcBlockPublicAccessExclusionRecord::exclusion_id)
    }

    /// Store a VPC BPA exclusion and answer it as it was stored.
    ///
    /// The one way an exclusion is stored, so the id it carries, the ARN it reports, the
    /// state it starts in, the move the clock owes it and the bytes it charges cannot be
    /// built two ways. An exclusion is answered in the non-terminal `create-in-progress`
    /// state, so the clock owes it `create-complete` one [`TRANSITION_DELAY`] later.
    ///
    /// What comes back is the stored view itself, so a create answers exactly what a later
    /// `DescribeVpcBlockPublicAccessExclusions` will report without reading the store a
    /// second time.
    pub fn insert_vpc_block_public_access_exclusion(
        &mut self,
        spec: VpcBlockPublicAccessExclusionSpec,
    ) -> VpcBlockPublicAccessExclusion {
        let exclusion_id = self.next_id(VPC_BPA_EXCLUSION_ID_PREFIX);
        // "When the exclusion was created"
        // (`VpcBlockPublicAccessExclusion$CreationTimestamp`); the exclusion has not
        // changed since, so that is also when it was last updated.
        let created_at = system_time_to_date_time(SystemTime::now());
        let view = VpcBlockPublicAccessExclusion {
            exclusion_id: Some(exclusion_id.clone()),
            internet_gateway_exclusion_mode: Some(spec.internet_gateway_exclusion_mode.to_string()),
            // "The ARN of the exclusion" (`VpcBlockPublicAccessExclusion$ResourceArn`),
            // which the `resource-arn` filter of the describe calls "the Amazon Resource
            // Name (ARN) of a exclusion" too: an EC2 ARN in the Region the request was
            // signed for, under the `vpc-block-public-access-exclusion` resource type the
            // modelled `ResourceType` catalog publishes for this resource.
            resource_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION}/{exclusion_id}"),
            )),
            state: Some(VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS.to_string()),
            // "The reason for the current exclusion state": nothing has gone wrong with
            // this one, so there is nothing to report.
            reason: None,
            creation_timestamp: Some(created_at),
            last_update_timestamp: Some(created_at),
            // "When the exclusion was deleted": it has not been.
            deletion_timestamp: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(tag_list_bytes(view.tags.as_deref().unwrap_or_default()));
        let stored = view.clone();
        self.vpc_block_public_access_exclusions.insert(
            exclusion_id,
            VpcBlockPublicAccessExclusionRecord {
                view,
                target: spec.target,
                pending: Some(ExclusionTransition::starting_now(
                    VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE,
                )),
            },
        );
        stored
    }
}

// =============================================================================
// NAT gateways
// =============================================================================

/// The id prefix every NAT gateway carries (`nat-…`), which is what lets an id of another
/// kind be answered as [`NAT_GATEWAY_MALFORMED`] before it is looked up.
pub const NAT_GATEWAY_ID_PREFIX: &str = "nat";

// The association of an Elastic IP address with a NAT gateway is named by the SAME id
// space as its association with any other resource — `Address$AssociationId` and
// `NatGatewayAddress$AssociationId` are one id, and
// `DisassociateNatGatewayAddressRequest$AssociationIds` takes it — so this module spells
// that prefix once, as [`ELASTIC_IP_ASSOCIATION_ID_PREFIX`] with the Elastic IP address
// constants above.

// The network interface a NAT gateway's addresses sit on is named by the SAME id space as
// every other network interface — "The ID of the network interface associated with the NAT
// gateway" (`NatGatewayAddress$NetworkInterfaceId`) — so this module spells that prefix
// once, as [`NETWORK_INTERFACE_ID_PREFIX`] with the network interface constants above.

/// "`available`: The NAT gateway is able to process traffic" (`NatGateway$State`) — the
/// one state of the `NatGatewayState` enum in which a NAT gateway is serving, and so the
/// one in which its addresses can be changed.
///
/// The enum's `failed` member is not spelled here: nothing in this emulator fails a gateway
/// it created, and the generated `crate::ops::types::NAT_GATEWAY_STATE_VALUES` is the catalog
/// of them all.
pub const NAT_GATEWAY_STATE_AVAILABLE: &str = "available";

/// "`pending`: The NAT gateway is being created and is not ready to process traffic"
/// (`NatGateway$State`): the NON-TERMINAL state `CreateNatGateway` answers with, which the
/// clock owes [`NAT_GATEWAY_STATE_AVAILABLE`].
pub const NAT_GATEWAY_STATE_PENDING: &str = "pending";

/// "`deleting`: The NAT gateway is in the process of being terminated and may still be
/// processing traffic" (`NatGateway$State`): the non-terminal state `DeleteNatGateway` leaves
/// a gateway in, which the clock owes [`NAT_GATEWAY_STATE_DELETED`].
pub const NAT_GATEWAY_STATE_DELETING: &str = "deleting";

/// "`deleted`: The NAT gateway has been terminated and is no longer processing traffic"
/// (`NatGateway$State`): the terminal state of a deletion. The RECORD stays — that state is
/// what a caller polling a gateway it deleted is shown — and a gateway in it serves nothing.
pub const NAT_GATEWAY_STATE_DELETED: &str = "deleted";

/// The one `ResourceType` a `CreateNatGateway` tag specification may name: "The tags to
/// assign to the NAT gateway" (`CreateNatGatewayRequest$TagSpecifications`), spelled as the
/// modelled `ResourceType` enum spells it (`natgateway`, from
/// `crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const NAT_GATEWAY_TAGGABLE_RESOURCE: &str = "natgateway";

/// A NAT gateway that reaches the internet: the `ConnectivityType` enum's `public` member
/// (`NatGateway$ConnectivityType`). Only such a gateway has Elastic IP addresses —
/// "[Public NAT gateway only] The allocation ID of the Elastic IP address that's
/// associated with the NAT gateway" (`NatGatewayAddress$AllocationId`).
pub const CONNECTIVITY_TYPE_PUBLIC: &str = "public";

/// The `ConnectivityType` enum's other member, a NAT gateway that reaches other VPCs and
/// on-premises networks only.
pub const CONNECTIVITY_TYPE_PRIVATE: &str = "private";

/// "Indicates whether this is a zonal (single-AZ) or regional (multi-AZ) NAT gateway"
/// (`NatGateway$AvailabilityMode`): the `AvailabilityMode` enum's `zonal` member, the NAT
/// gateway that lives in the one Availability Zone of its subnet.
pub const AVAILABILITY_MODE_ZONAL: &str = "zonal";

/// The `AvailabilityMode` enum's `regional` member: "A regional NAT gateway is a single
/// NAT Gateway that works across multiple availability zones (AZs) in your VPC" — the one
/// kind for which `AssociateNatGatewayAddress` takes an Availability Zone, because such a
/// gateway "uses a separate EIP in each AZ"
/// (`AssociateNatGatewayAddressRequest$AvailabilityZone`).
pub const AVAILABILITY_MODE_REGIONAL: &str = "regional";

/// The transient `NatGatewayAddressStatus` an `AssociateNatGatewayAddress` leaves an
/// address in: `associating`.
pub const NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING: &str = "associating";

/// The transient `NatGatewayAddressStatus` of an address whose PRIVATE IPv4 address is
/// being put on the gateway's network interface: `assigning`, the `NatGatewayAddressStatus`
/// member that names an assignment rather than an association
/// (`crate::ops::types::NAT_GATEWAY_ADDRESS_STATUS_VALUES`). An address that associates no
/// Elastic IP address — a private NAT gateway's own address, and the address
/// `AssignPrivateNatGatewayAddress` adds — is in it until the clock owes it
/// [`NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED`].
pub const NAT_GATEWAY_ADDRESS_STATUS_ASSIGNING: &str = "assigning";

/// The terminal `NatGatewayAddressStatus` the clock owes an `associating` address:
/// `succeeded`.
pub const NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED: &str = "succeeded";

/// `NatGatewayNotFound` (HTTP 400): the request named a NAT gateway id this account holds
/// no NAT gateway for.
pub const NAT_GATEWAY_NOT_FOUND: &str = "NatGatewayNotFound";

/// `NatGatewayMalformed` (HTTP 400): the request named something that is not a NAT gateway
/// id at all, so there is nothing to look up.
pub const NAT_GATEWAY_MALFORMED: &str = "NatGatewayMalformed";

/// EC2's quota code for one NAT gateway too many (EC2 API "Error codes":
/// `NatGatewayLimitExceeded`).
pub const NAT_GATEWAY_LIMIT_EXCEEDED: &str = "NatGatewayLimitExceeded";

/// Refuse a NAT gateway create this emulator has no room for: HTTP 400
/// [`NAT_GATEWAY_LIMIT_EXCEEDED`], naming what is full.
pub fn nat_gateway_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(NAT_GATEWAY_LIMIT_EXCEEDED, 400, message.into())
}

// `Resource.AlreadyAssociated` — "one of the failures `NatGateway$FailureCode` itself
// names" for this family of requests — is the SAME code an Elastic IP address already
// associated with another resource is refused with, so this module spells it once, as
// [`RESOURCE_ALREADY_ASSOCIATED`] with the Elastic IP address codes above, and reaches it
// through [`resource_already_associated`].

/// How many Elastic IP addresses one public NAT gateway holds.
///
/// "By default, you can associate up to 2 Elastic IP addresses per public NAT gateway. You
/// can increase the limit by requesting a quota adjustment" (`AssociateNatGatewayAddress`)
/// — this emulator serves an account at that published default, so the third address is
/// refused the way a quota adjustment would be needed for.
///
/// What the count is taken over is CHOSEN for a REGIONAL gateway: the quota sentence names
/// no Availability Zone, while that gateway "uses a separate EIP in each AZ to handle
/// outbound NAT traffic from that AZ" (`AssociateNatGatewayAddressRequest$AvailabilityZone`),
/// and counting the whole gateway would leave a gateway serving three zones unable to hold
/// any address for its third — so this emulator counts a regional gateway's addresses per
/// zone and a single-zone gateway's over the gateway, which is the same thing for a gateway
/// that has one zone and is what every recorded unit exercises
/// ([`ComAmazonawsEc2Amazonec2State::nat_gateway_eip_count`]).
pub const MAX_EIPS_PER_PUBLIC_NAT_GATEWAY: usize = 2;

/// The move the clock owes one NAT gateway address: which gateway holds it, the terminal
/// `NatGatewayAddressStatus` it is going to, and when that falls due.
///
/// `to` is a `&'static str` so only a word of the modelled `NatGatewayAddressStatus`
/// catalog, out of this module's own constants, can ever be stored — never text from a
/// request. Applied by [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`],
/// which finds the address this move belongs to by the key it is stored under: the
/// `AssociationId` of an address that associates an Elastic IP address, and the private
/// address itself for one that associates none (see
/// [`ComAmazonawsEc2Amazonec2State::nat_gateway_address_transitions`]).
#[derive(Debug, Clone)]
pub struct NatGatewayAddressTransition {
    /// The `NatGatewayId` of the gateway whose address this move belongs to.
    pub nat_gateway_id: String,
    /// The status the address reaches at [`Self::due_at`].
    pub to: &'static str,
    /// The instant the move falls due, on the same monotonic clock and the same
    /// [`TRANSITION_DELAY`] as a `pending` VPC's.
    pub due_at: Instant,
}

impl NatGatewayAddressTransition {
    /// The move an operation starting NOW owes an address of `nat_gateway_id`: `to`, one
    /// [`TRANSITION_DELAY`] from now. The span is never zero, so a caller acting straight
    /// after the call still sees the transient status.
    pub fn starting_now(nat_gateway_id: &str, to: &'static str) -> Self {
        Self {
            nat_gateway_id: nat_gateway_id.to_string(),
            to,
            due_at: Instant::now() + TRANSITION_DELAY,
        }
    }
}

// =============================================================================
// Capacity Reservation Fleets
// =============================================================================

/// The id prefix every Capacity Reservation Fleet is minted with, the `crf-` of the
/// `CapacityReservationFleetId` AWS's own error text spells ("Capacity Reservation Fleet ID
/// 'crf-…'").
pub const CAPACITY_RESERVATION_FLEET_ID_PREFIX: &str = "crf";

/// The `ResourceType` a `CreateCapacityReservationFleet` tag specification may name: the
/// Fleet it creates (the modelled `ResourceType` enum's `capacity-reservation-fleet`
/// member). "The tags are automatically assigned to the Capacity Reservations in the Fleet"
/// (`CreateCapacityReservationFleetRequest$TagSpecifications`), so the one tag list is the
/// Fleet's and its reservations'.
pub const RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET: &str = "capacity-reservation-fleet";

/// "`submitted` - The Capacity Reservation Fleet request has been submitted and Amazon
/// Elastic Compute Cloud is preparing to create the Capacity Reservations"
/// (`CapacityReservationFleet$State`): where every Fleet starts, and a NON-terminal state,
/// so the clock owes it [`CAPACITY_RESERVATION_FLEET_STATE_ACTIVE`].
pub const CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED: &str = "submitted";

/// "`modifying` - The Capacity Reservation Fleet is being modified"
/// (`CapacityReservationFleet$State`).
pub const CAPACITY_RESERVATION_FLEET_STATE_MODIFYING: &str = "modifying";

/// "`active` - The Capacity Reservation Fleet has fulfilled its total target capacity and
/// it is attempting to maintain this capacity" (`CapacityReservationFleet$State`).
pub const CAPACITY_RESERVATION_FLEET_STATE_ACTIVE: &str = "active";

/// "`partially_fulfilled` - The Capacity Reservation Fleet has partially fulfilled its
/// total target capacity" (`CapacityReservationFleet$State`).
pub const CAPACITY_RESERVATION_FLEET_STATE_PARTIALLY_FULFILLED: &str = "partially_fulfilled";

/// "`expiring` - The Capacity Reservation Fleet has reach its end date and it is in the
/// process of expiring" (`CapacityReservationFleet$State`).
pub const CAPACITY_RESERVATION_FLEET_STATE_EXPIRING: &str = "expiring";

/// "`cancelling` - The Capacity Reservation Fleet is in the process of being cancelled"
/// (`CapacityReservationFleet$State`): where a cancellation puts a Fleet, and a NON-terminal
/// state, so the clock owes it [`CAPACITY_RESERVATION_FLEET_STATE_CANCELLED`].
pub const CAPACITY_RESERVATION_FLEET_STATE_CANCELLING: &str = "cancelling";

/// "The Capacity Reservation Fleet's status changes to `cancelled`"
/// (`CancelCapacityReservationFleets`): the end of a cancellation, and terminal.
pub const CAPACITY_RESERVATION_FLEET_STATE_CANCELLED: &str = "cancelled";

/// The states a Capacity Reservation Fleet can be cancelled FROM.
///
/// A cancellation moves a Fleet that is still doing something — one preparing its
/// reservations, one being modified, one holding capacity, one expiring — into
/// [`CAPACITY_RESERVATION_FLEET_STATE_CANCELLING`]. The states left out are the ones a Fleet
/// has already left: `cancelling` and `cancelled` (the cancellation has already happened),
/// and `expired` and `failed`, where "The Fleet can't create new Capacity Reservations"
/// (`CapacityReservationFleet$State`) and so there is nothing left to cancel.
pub const CAPACITY_RESERVATION_FLEET_CANCELLABLE_STATES: [&str; 5] = [
    CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
    CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
    CAPACITY_RESERVATION_FLEET_STATE_PARTIALLY_FULFILLED,
    CAPACITY_RESERVATION_FLEET_STATE_EXPIRING,
];

/// The states a Capacity Reservation Fleet can be MODIFIED from.
///
/// A modification "creates new Capacity Reservations, or modifies or cancels existing
/// Capacity Reservations in the Fleet to meet the new total target capacity"
/// (`ModifyCapacityReservationFleet`), so it runs only from a state in which the Fleet still
/// creates Capacity Reservations: one preparing them, one already being modified, and one
/// holding capacity in full or in part. The states left out are the ones in which "The Fleet
/// can't create new Capacity Reservations" (`CapacityReservationFleet$State`) — `expiring`
/// and `expired`, where the Fleet has reached its end date, `cancelling` and `cancelled`,
/// where it is being or has been cancelled, and `failed`.
pub const CAPACITY_RESERVATION_FLEET_MODIFIABLE_STATES: [&str; 4] = [
    CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
    CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
    CAPACITY_RESERVATION_FLEET_STATE_PARTIALLY_FULFILLED,
];

/// "Currently, only the `prioritized` allocation strategy is supported […] Valid values:
/// `prioritized`" (`CreateCapacityReservationFleetRequest$AllocationStrategy`), so it is
/// both the only value a request may name and the strategy a request that names none gets.
pub const FLEET_ALLOCATION_STRATEGY_PRIORITIZED: &str = "prioritized";

/// The priority a Capacity Reservation of a Fleet carries when the instance type
/// specification that asked for it assigned none.
///
/// `FleetCapacityReservation$Priority` is "The priority of the instance type in the Capacity
/// Reservation Fleet" — a member EC2 reports for EVERY reservation of a Fleet, whether or
/// not the request assigned one, so a specification that assigned none is reported with the
/// Fleet's default. Recorded behaviour for the value itself: EC2 answers `9999`, which is
/// above every priority a request can assign (the Instance type priority the User Guide
/// documents runs 0 … 999), and "A lower value indicates a high priority"
/// (`ReservationFleetInstanceSpecification$Priority`) — so a defaulted specification is
/// ordered behind every prioritized one, which is exactly how the `prioritized` strategy
/// already treats it ([`prioritized_fleet_specification`]).
pub const DEFAULT_FLEET_INSTANCE_PRIORITY: i32 = 9_999;

/// The capacity units one instance provides when a specification names no `Weight`.
///
/// "The number of capacity units provided by the specified instance type. This value,
/// together with the total target capacity that you specify for the Fleet determine the
/// number of instances for which the Fleet reserves capacity. Both values are based on units
/// that make sense for your workload"
/// (`ReservationFleetInstanceSpecification$Weight`) — the model publishes no default, so a
/// specification that names no weight is read in units of instances: one instance provides
/// one unit, which makes `TotalTargetCapacity` a count of instances.
pub const DEFAULT_FLEET_INSTANCE_WEIGHT: f64 = 1.0;

/// The largest number of instance type specifications one `CreateCapacityReservationFleet`
/// may carry.
///
/// A bound in the same style as [`MAX_FILTERS`], not an AWS quota: the model declares no
/// `@length` for `ReservationFleetInstanceSpecificationList`, and each entry is work and
/// memory this process does, so the number a caller can send has to be finite.
pub const MAX_FLEET_INSTANCE_TYPE_SPECIFICATIONS: usize = 100;

/// The largest number of Capacity Reservation Fleets this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each Fleet is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_capacity_reservation_fleet_capacity`] is the
/// predicate a writer asks before inserting; a create past it refuses with
/// [`capacity_reservation_fleet_limit_exceeded`].
pub const MAX_CAPACITY_RESERVATION_FLEETS: usize = 1_000;

/// The code a create past [`MAX_CAPACITY_RESERVATION_FLEETS`] answers with, following the
/// `<Resource>LimitExceeded` convention every per-resource quota refusal of this service
/// uses ([`VPC_LIMIT_EXCEEDED`], [`VPC_BPA_EXCLUSION_LIMIT_EXCEEDED`]).
pub const CAPACITY_RESERVATION_FLEET_LIMIT_EXCEEDED: &str = "CapacityReservationFleetLimitExceeded";

/// The code EC2 answers for something that is not a Capacity Reservation Fleet id at all,
/// BEFORE it looks the id up — so a caller can tell a typo from a Fleet the account does not
/// hold.
pub const INVALID_CAPACITY_RESERVATION_FLEET_ID_MALFORMED: &str =
    "InvalidCapacityReservationFleetId.Malformed";

/// The code EC2 answers for a well-formed Capacity Reservation Fleet id the account holds no
/// Fleet under.
pub const INVALID_CAPACITY_RESERVATION_FLEET_ID_NOT_FOUND: &str =
    "InvalidCapacityReservationFleetId.NotFound";

/// Whether `id` is a well-formed Capacity Reservation Fleet id — one EC2 could have ISSUED.
///
/// The long-form rule ([`issued_long_id_is_well_formed`]) under this resource's own `crf`
/// prefix: `CapacityReservationFleetId` is a plain string in the model
/// (`smithy_query(query="shape",shape="CapacityReservationFleetId")`), so the form is the one
/// AWS's `InvalidCapacityReservationFleetId.Malformed` answers for anything outside —
/// including an id whose leading digit is above zero and the all-zero placeholder, both of
/// which the recorded units behind
/// `issues/.../20261023T102725Z-ec2-cancelcapacityreservationfleets-stub.md` and
/// `issues/.../20261023T102725Z-ec2-modifycapacityreservationfleet-stub.md` were answered
/// `InvalidCapacityReservationFleetId.Malformed` for.
pub fn capacity_reservation_fleet_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed(CAPACITY_RESERVATION_FLEET_ID_PREFIX, id)
}

/// Refuse something that is not a Capacity Reservation Fleet id: HTTP 400
/// [`INVALID_CAPACITY_RESERVATION_FLEET_ID_MALFORMED`].
pub fn invalid_capacity_reservation_fleet_id_malformed<E>(fleet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_FLEET_ID_MALFORMED,
        400,
        format!("Capacity Reservation Fleet ID '{fleet_id}' is malformed"),
    )
}

/// Why a well-formed Capacity Reservation Fleet id resolves to nothing.
///
/// ONE wording, so the request-level refusal `DescribeCapacityReservationFleets` answers and
/// the per-Fleet entry `CancelCapacityReservationFleets` reports in its
/// `FailedFleetCancellations` say the same thing about the same id.
pub fn capacity_reservation_fleet_not_found_message(fleet_id: &str) -> String {
    format!("The Capacity Reservation Fleet ID '{fleet_id}' does not exist")
}

/// Refuse a Capacity Reservation Fleet id the account holds no Fleet under: HTTP 400
/// [`INVALID_CAPACITY_RESERVATION_FLEET_ID_NOT_FOUND`].
pub fn invalid_capacity_reservation_fleet_id_not_found<E>(fleet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CAPACITY_RESERVATION_FLEET_ID_NOT_FOUND,
        400,
        capacity_reservation_fleet_not_found_message(fleet_id),
    )
}

/// Refuse a create that would take the store past [`MAX_CAPACITY_RESERVATION_FLEETS`]: HTTP
/// 400 [`CAPACITY_RESERVATION_FLEET_LIMIT_EXCEEDED`].
pub fn capacity_reservation_fleet_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        CAPACITY_RESERVATION_FLEET_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of Capacity Reservation Fleets has been reached \
             ({MAX_CAPACITY_RESERVATION_FLEETS})"
        ),
    )
}

/// The move the clock owes one Capacity Reservation Fleet: a [`ClockTransition`] to the
/// `CapacityReservationFleetState` member it becomes.
///
/// The `to` is a `&'static str` so only a word from this module's own catalog constants can
/// be stored — never a request's text.
pub type CapacityReservationFleetTransition = ClockTransition<&'static str>;

/// How long a Capacity Reservation Fleet takes to reach `active`.
///
/// A Fleet is a COMPOSITE resource: "`submitted` - The Capacity Reservation Fleet request
/// has been submitted and Amazon Elastic Compute Cloud is preparing to create the Capacity
/// Reservations", and only once those exist has it "fulfilled its total target capacity",
/// which is what `active` reports (`CapacityReservationFleet$State`). So the Fleet's own
/// move is owed after the Capacity Reservations it reserves through are themselves ready,
/// rather than at the single [`TRANSITION_DELAY`] a resource that waits on nothing takes:
/// a client flow that creates a Fleet and then describes, modifies or cancels it is still
/// inside that preparation, which is the state EC2 reports to such a flow, so the span is
/// wide enough to cover one.
pub const CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY: Duration = Duration::from_secs(2);

/// One Capacity Reservation Fleet: the modelled [`CapacityReservationFleet`] every operation
/// that reports one answers with, plus the facts that shape carries no member for — the
/// state word the Fleet moved out of, and the move the clock owes it.
///
/// `CreateCapacityReservationFleet`, `DescribeCapacityReservationFleets` and
/// `CancelCapacityReservationFleets` all report the same Fleet, so the one stored view is
/// what all of them answer from and they cannot describe one Fleet differently — the create
/// result's flat member set is a projection of this view
/// ([`CapacityReservationFleetRecord::view`]), not a second copy of it, taken as the submit
/// answers the Fleet.
#[derive(Debug, Clone)]
pub struct CapacityReservationFleetRecord {
    /// The Fleet as the model shapes it, carrying the state reached so far and the Capacity
    /// Reservations it reserves its capacity through.
    pub view: CapacityReservationFleet,
    /// The `CapacityReservationFleetState` the Fleet held BEFORE the one
    /// [`Self::view`] carries: the state word it last moved out of, which is what EC2
    /// reports as a Fleet's previous state. A Fleet that has not left the state it was
    /// created in carries that state here, there being no earlier one.
    ///
    /// A fact of the Fleet the modelled [`CapacityReservationFleet`] shape carries no member
    /// for, which is why it sits beside the view rather than in it: only the cancellation
    /// result reports it ([`CapacityReservationFleetCancellationState`]).
    pub previous_state: String,
    /// The move the clock owes this Fleet, while one is owed.
    pub pending: Option<CapacityReservationFleetTransition>,
}

impl CapacityReservationFleetRecord {
    /// The Fleet's `CapacityReservationFleetState` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// Move the Fleet's state word to `to`, remembering the word it leaves as the Fleet's
    /// previous state.
    ///
    /// The ONE place a stored Fleet's state word changes, so the state it reports and the
    /// state it reports as its previous one cannot drift apart: every move — the activation
    /// the clock owes, a modification, a cancellation and the cancellation's own end — goes
    /// through here.
    fn move_state_to(&mut self, to: &str) {
        let left = self.state().to_string();
        if left == to {
            return;
        }
        self.previous_state = left;
        self.view.state = Some(to.to_string());
    }

    /// The Fleet's own id.
    pub fn capacity_reservation_fleet_id(&self) -> &str {
        self.view
            .capacity_reservation_fleet_id
            .as_deref()
            .unwrap_or_default()
    }

    /// Start cancelling this Fleet, answering the Fleet's PREVIOUS state as it stood when
    /// the cancellation arrived ([`Self::previous_state`]) — or `None` when the state the
    /// Fleet is in is not one a cancellation runs from
    /// ([`CAPACITY_RESERVATION_FLEET_CANCELLABLE_STATES`]).
    ///
    /// "The Capacity Reservation Fleet's status changes to `cancelled`"
    /// (`CancelCapacityReservationFleets`) by way of `cancelling`, "the process of being
    /// cancelled" the state catalog names — so the caller is answered the transient state
    /// and the clock owes the terminal one. Any move that was owed before (a `submitted`
    /// Fleet's `active`) is REPLACED: the Fleet is being cancelled, not activated.
    ///
    /// What comes back is the state word the cancellation result reports as the Fleet's
    /// `previousFleetState` — "The previous state of the Capacity Reservation Fleet"
    /// (`CapacityReservationFleetCancellationState$PreviousFleetState`), the word the Fleet
    /// had moved out of by the time the call arrived — READ BEFORE this cancellation records
    /// its own move, so the pair the result carries is the Fleet's previous state and the
    /// state the cancellation puts it in rather than two words from the same move.
    pub fn begin_cancellation(&mut self) -> Option<String> {
        let interrupted = self.state().to_string();
        if !CAPACITY_RESERVATION_FLEET_CANCELLABLE_STATES.contains(&interrupted.as_str()) {
            return None;
        }
        let reported_previous = self.previous_state.clone();
        self.move_state_to(CAPACITY_RESERVATION_FLEET_STATE_CANCELLING);
        self.pending = Some(CapacityReservationFleetTransition::starting_now(
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
        ));
        Some(reported_previous)
    }

    /// Whether this Fleet's current state is one a modification runs from
    /// ([`CAPACITY_RESERVATION_FLEET_MODIFIABLE_STATES`]).
    pub fn is_modifiable(&self) -> bool {
        CAPACITY_RESERVATION_FLEET_MODIFIABLE_STATES.contains(&self.state())
    }

    /// Start modifying this Fleet.
    ///
    /// "`modifying` - The Capacity Reservation Fleet is being modified"
    /// (`CapacityReservationFleet$State`) is a NON-terminal state, so the caller is answered
    /// the transient state and the clock owes the Fleet `active`, where it "has fulfilled its
    /// total target capacity and it is attempting to maintain this capacity". Any move that
    /// was owed before (a `submitted` Fleet's `active`, or the `active` an earlier
    /// modification owed) is REPLACED, so the Fleet reaches `active` one span after the LAST
    /// call that changed it.
    ///
    /// Callers ask [`Self::is_modifiable`] first: this is the move itself, not the check.
    pub(super) fn begin_modification(&mut self) {
        self.move_state_to(CAPACITY_RESERVATION_FLEET_STATE_MODIFYING);
        self.pending = Some(CapacityReservationFleetTransition::starting_now(
            CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
        ));
    }

    /// Reserve `total_target_capacity` capacity units through this Fleet's Capacity
    /// Reservations, the Fleet's `prioritized` allocation strategy having chosen `chosen`.
    ///
    /// "When you modify the total target capacity of a Capacity Reservation Fleet, the Fleet
    /// automatically creates new Capacity Reservations, or modifies or cancels existing
    /// Capacity Reservations in the Fleet to meet the new total target capacity"
    /// (`ModifyCapacityReservationFleet`): the chosen specification's Capacity Reservation is
    /// MODIFIED in place — it keeps the id and the create date it was reserved under, so a
    /// caller holding that id still names the same reservation — and a specification the
    /// strategy no longer reserves on holds no Capacity Reservation at all. The instance
    /// count covering the new target, and the capacity it fulfils, are derived exactly as a
    /// create derives them ([`fleet_instance_count`], [`fleet_fulfilled_capacity`]), so one
    /// Fleet cannot report its capacity two ways.
    ///
    /// `minted_reservation_id` is the id for a Capacity Reservation this call CREATES (the
    /// chosen specification held none), minted by the caller because only the store mints
    /// ids; `now` dates that reservation.
    pub(super) fn reserve_target_capacity(
        &mut self,
        total_target_capacity: i32,
        chosen: Option<usize>,
        minted_reservation_id: Option<String>,
        now: SystemTime,
    ) {
        self.view.total_target_capacity = Some(total_target_capacity);
        let mut reservations = self
            .view
            .instance_type_specifications
            .take()
            .unwrap_or_default();
        for (index, reservation) in reservations.iter_mut().enumerate() {
            let weight = reservation
                .weight
                .filter(|weight| *weight > 0.0)
                .unwrap_or(DEFAULT_FLEET_INSTANCE_WEIGHT);
            let reserved_here = chosen == Some(index);
            let instance_count = if reserved_here {
                fleet_instance_count(total_target_capacity, weight)
            } else {
                0
            };
            reservation.weight = Some(weight);
            reservation.total_instance_count = Some(instance_count);
            reservation.fulfilled_capacity = Some(f64::from(instance_count) * weight);
            reservation.priority =
                Some(reservation.priority.unwrap_or(DEFAULT_FLEET_INSTANCE_PRIORITY));
            if reserved_here {
                if reservation.capacity_reservation_id.is_none() {
                    reservation.capacity_reservation_id = minted_reservation_id.clone();
                    reservation.create_date = Some(system_time_to_date_time(now));
                }
            } else {
                // The Capacity Reservation this specification held is CANCELLED, so there is
                // no reservation left to name or to date.
                reservation.capacity_reservation_id = None;
                reservation.create_date = None;
            }
        }
        self.view.total_fulfilled_capacity = Some(fleet_fulfilled_capacity(&reservations));
        self.view.instance_type_specifications = Some(reservations);
    }

    /// Apply the move the clock owed this Fleet.
    ///
    /// `active` is where the Fleet "has fulfilled its total target capacity and it is
    /// attempting to maintain this capacity" (`CapacityReservationFleet$State`): the capacity
    /// was reserved by the create, so reaching it changes the Fleet's state word and nothing
    /// else. `cancelled` is the end of a cancellation, which is the Fleet's own state word
    /// too: the Fleet's COMPOSITION — the instance type specifications it reserved on, the
    /// Capacity Reservations they held, the instances they counted and the capacity they
    /// fulfilled — is what EC2 keeps reporting for a cancelled Fleet, so an audit or
    /// teardown flow can still read what was reserved.
    pub(super) fn apply_due_move(&mut self, to: &'static str) {
        self.move_state_to(to);
        self.pending = None;
    }
}

/// How many instances one Capacity Reservation takes to cover `total_target_capacity` at
/// `weight` capacity units per instance: the quotient rounded UP, because an instance is not
/// reserved in fractions.
///
/// A count outside the modelled `Integer` member's range is clamped to its ends rather than
/// wrapping: `weight` is bounded below by the modelled `@range` (`min: 0.001`) and
/// `total_target_capacity` above by `Integer`, so the clamp is unreachable for any request
/// the boundary validator admits — it is there so the conversion cannot be lossy by
/// construction.
pub(super) fn fleet_instance_count(total_target_capacity: i32, weight: f64) -> i32 {
    let count = (f64::from(total_target_capacity) / weight).ceil();
    if !count.is_finite() || count <= 0.0 {
        return 0;
    }
    if count >= f64::from(i32::MAX) {
        return i32::MAX;
    }
    // Inside `i32`, and integral after `ceil`, so this conversion is exact.
    count as i32
}

/// "The capacity units that have been fulfilled"
/// (`CapacityReservationFleet$TotalFulfilledCapacity`): the sum of what each of the Fleet's
/// Capacity Reservations fulfils, so the Fleet's total and its reservations cannot disagree.
///
/// Folded from a POSITIVE zero rather than summed: `Sum` for `f64` folds from a negative
/// zero, so a Fleet holding no Capacity Reservation would report a capacity of `-0.0` — not
/// a value a capacity takes, nor one AWS writes.
pub fn fleet_fulfilled_capacity(reservations: &[FleetCapacityReservation]) -> f64 {
    reservations
        .iter()
        .filter_map(|reservation| reservation.fulfilled_capacity)
        .fold(0.0, |fulfilled, capacity| fulfilled + capacity)
}

/// Which of a Capacity Reservation Fleet's instance type specifications the Fleet reserves
/// its capacity on.
///
/// The `prioritized` allocation strategy "uses the priority that you assigned to each
/// instance type to determine which of the specified instance types to use" first, moving on
/// only when the capacity it asked for is not available
/// (`CreateCapacityReservationFleetRequest$AllocationStrategy`), and "A lower value indicates
/// a high priority" (`ReservationFleetInstanceSpecification$Priority`) — a specification that
/// assigned none coming after every one that did, and the earliest winning a tie
/// (`min_by_key` keeps the first). The capacity this emulator reserves is always available,
/// so the chosen specification covers the whole target and the others reserve nothing.
///
/// Generic over the carrier, so the create (which reads a validated request) and a later
/// modification (which reads the Fleet as stored) resolve the strategy through ONE rule.
pub(super) fn prioritized_fleet_specification<T>(
    specifications: &[T],
    priority_of: impl Fn(&T) -> Option<i32>,
) -> Option<usize> {
    specifications
        .iter()
        .enumerate()
        .min_by_key(|(_, specification)| priority_of(specification).unwrap_or(i32::MAX))
        .map(|(index, _)| index)
}

/// One instance type a Capacity Reservation Fleet reserves capacity for, as a writer has
/// already validated it: the words come out of the modelled catalogs rather than from the
/// request's text, and the zone is one the signed Region has.
#[derive(Debug, Clone)]
pub struct CapacityReservationFleetInstanceSpec {
    /// "The instance type for which the Capacity Reservation Fleet reserves capacity"
    /// (`ReservationFleetInstanceSpecification$InstanceType`), a member of the modelled
    /// `InstanceType` enum.
    pub instance_type: &'static str,
    /// "The type of operating system for which the Capacity Reservation Fleet reserves
    /// capacity" (`$InstancePlatform`), a member of the modelled
    /// `CapacityReservationInstancePlatform` enum.
    pub instance_platform: &'static str,
    /// The Availability Zone the capacity is reserved in, and its id: "A Capacity
    /// Reservation Fleet can't span Availability Zones", so every specification of one
    /// Fleet carries the same zone.
    pub availability_zone: String,
    /// The id of [`Self::availability_zone`], resolved against the signed Region's zones.
    pub availability_zone_id: String,
    /// "The number of capacity units provided by the specified instance type" (`$Weight`),
    /// defaulted to [`DEFAULT_FLEET_INSTANCE_WEIGHT`].
    pub weight: f64,
    /// "The priority to assign to the instance type […] A lower value indicates a high
    /// priority" (`$Priority`), or `None` when the request assigned none.
    pub priority: Option<i32>,
    /// "Indicates whether the Capacity Reservation Fleet supports EBS-optimized instances
    /// types" (`$EbsOptimized`).
    pub ebs_optimized: bool,
}

/// What a writer must say to store a Capacity Reservation Fleet; everything else about the
/// record — the Fleet's id, its ARN, the state it starts in, the Capacity Reservations it
/// reserves through, the move the clock owes it and the bytes it charges — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation_fleet`].
#[derive(Debug, Clone)]
pub struct CapacityReservationFleetSpec {
    /// The Region the request was signed for, which the Fleet's ARN names.
    pub region: String,
    /// The allocation strategy the Fleet reserves capacity by, as a `&'static str` out of
    /// this module's catalog ([`FLEET_ALLOCATION_STRATEGY_PRIORITIZED`]).
    pub allocation_strategy: &'static str,
    /// The instance matching criteria every Capacity Reservation of the Fleet inherits, out
    /// of the GENERATED catalog of the modelled `FleetInstanceMatchCriteria` enum
    /// (`crate::ops::types::FLEET_INSTANCE_MATCH_CRITERIA_VALUES`), whose single member
    /// `open` is also the default — "Currently, Capacity Reservation Fleets support `open`
    /// instance matching criteria only" (`CapacityReservationFleet$InstanceMatchCriteria`).
    pub instance_match_criteria: &'static str,
    /// The tenancy every Capacity Reservation of the Fleet inherits, out of the GENERATED
    /// catalog of the modelled `FleetCapacityReservationTenancy` enum
    /// (`crate::ops::types::FLEET_CAPACITY_RESERVATION_TENANCY_VALUES`), whose single member
    /// is "`default` - The Capacity Reservation Fleet is created on hardware that is shared
    /// with other Amazon Web Services accounts" (`CapacityReservationFleet$Tenancy`).
    pub tenancy: &'static str,
    /// "The total number of capacity units to be reserved by the Capacity Reservation
    /// Fleet" (`CreateCapacityReservationFleetRequest$TotalTargetCapacity`).
    pub total_target_capacity: i32,
    /// "The date and time at which the Capacity Reservation Fleet expires"
    /// (`$EndDate`), or `None` for a Fleet the request gave no end date.
    pub end_date: Option<::aws_smithy_types::DateTime>,
    /// The instance types the Fleet may reserve capacity for, in the order the request gave
    /// them.
    pub instance_specs: Vec<CapacityReservationFleetInstanceSpec>,
    /// The tags the request's `TagSpecifications` assign to the Fleet.
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Capacity Reservation Fleet fits inside the store's ceiling
    /// ([`MAX_CAPACITY_RESERVATION_FLEETS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses with
    /// [`capacity_reservation_fleet_limit_exceeded`] when it answers `false`.
    pub fn has_capacity_reservation_fleet_capacity(&self) -> bool {
        self.capacity_reservation_fleets.len() < MAX_CAPACITY_RESERVATION_FLEETS
    }

    /// The Capacity Reservations one Fleet reserves its target capacity through: one per
    /// instance type specification the request named, in the order it named them.
    ///
    /// "Information about the instance types for which to reserve the capacity"
    /// (`CapacityReservationFleet$InstanceTypeSpecifications`) — every specification is
    /// reported, carrying what the Fleet reserved on it. WHICH of them that is follows the
    /// allocation strategy, resolved by [`prioritized_fleet_specification`].
    ///
    /// How many instances covering the target takes is the target capacity divided by what
    /// one instance of that type provides, rounded UP: "This value, together with the total
    /// target capacity that you specify for the Fleet determine the number of instances for
    /// which the Fleet reserves capacity" (`$Weight`), and an instance is not reserved in
    /// fractions — so the capacity fulfilled is that count times the weight, which is at
    /// least the target. A specification nothing was reserved on holds no Capacity
    /// Reservation, so it reports neither an id nor a create date: there is no reservation to
    /// name.
    pub(super) fn fleet_capacity_reservations(
        &mut self,
        spec: &CapacityReservationFleetSpec,
        created_at: SystemTime,
    ) -> Vec<FleetCapacityReservation> {
        // The specification the strategy reserves on, by the one rule both the create and a
        // later modification resolve it through.
        let chosen = prioritized_fleet_specification(&spec.instance_specs, |instance_spec| {
            instance_spec.priority
        });
        let mut reservations: Vec<FleetCapacityReservation> =
            Vec::with_capacity(spec.instance_specs.len());
        for (index, instance_spec) in spec.instance_specs.iter().enumerate() {
            let weight = if instance_spec.weight > 0.0 {
                instance_spec.weight
            } else {
                DEFAULT_FLEET_INSTANCE_WEIGHT
            };
            let reserved_here = chosen == Some(index);
            let total_instance_count = if reserved_here {
                fleet_instance_count(spec.total_target_capacity, weight)
            } else {
                0
            };
            reservations.push(FleetCapacityReservation {
                capacity_reservation_id: reserved_here
                    .then(|| self.next_id(CAPACITY_RESERVATION_ID_PREFIX)),
                availability_zone: Some(instance_spec.availability_zone.clone()),
                availability_zone_id: Some(instance_spec.availability_zone_id.clone()),
                instance_type: Some(instance_spec.instance_type.to_string()),
                instance_platform: Some(instance_spec.instance_platform.to_string()),
                total_instance_count: Some(total_instance_count),
                fulfilled_capacity: Some(f64::from(total_instance_count) * weight),
                ebs_optimized: Some(instance_spec.ebs_optimized),
                create_date: reserved_here.then(|| system_time_to_date_time(created_at)),
                weight: Some(weight),
                // Every reservation of a Fleet reports a priority; one whose specification
                // assigned none reports the Fleet's default
                // ([`DEFAULT_FLEET_INSTANCE_PRIORITY`]).
                priority: Some(
                    instance_spec
                        .priority
                        .unwrap_or(DEFAULT_FLEET_INSTANCE_PRIORITY),
                ),
            });
        }
        reservations
    }

    /// Store a Capacity Reservation Fleet and answer it as it was stored.
    ///
    /// The one way a Fleet is stored, so the id it carries, the ARN it reports, the state it
    /// starts in, the Capacity Reservations it reserves through, the move the clock owes it
    /// and the bytes it charges cannot be built two ways.
    ///
    /// The CALL reserves the capacity: the Capacity Reservations the Fleet's allocation
    /// strategy chooses, and the capacity they fulfil, are stored by this write, so a reader
    /// of the Fleet reports them from here on. What the Fleet still owes the clock is its own
    /// STATE word: a Fleet is stored in the non-terminal `submitted`, where "Amazon Elastic
    /// Compute Cloud is preparing to create the Capacity Reservations"
    /// (`CapacityReservationFleet$State`), and reaches `active`
    /// one [`CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY`] later.
    ///
    /// What comes back is the stored view itself, so what a later
    /// `DescribeCapacityReservationFleets` reports is read off this one record without
    /// reading the store a second time.
    pub fn insert_capacity_reservation_fleet(
        &mut self,
        spec: CapacityReservationFleetSpec,
    ) -> CapacityReservationFleet {
        let fleet_id = self.next_id(CAPACITY_RESERVATION_FLEET_ID_PREFIX);
        let created_at = SystemTime::now();
        let reservations = self.fleet_capacity_reservations(&spec, created_at);
        let view = CapacityReservationFleet {
            capacity_reservation_fleet_id: Some(fleet_id.clone()),
            // "The ARN of the Capacity Reservation Fleet"
            // (`CapacityReservationFleet$CapacityReservationFleetArn`): an EC2 ARN in the
            // Region the request was signed for, under the resource type the modelled
            // `ResourceType` catalog publishes for this resource.
            capacity_reservation_fleet_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET}/{fleet_id}"),
            )),
            state: Some(CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED.to_string()),
            total_target_capacity: Some(spec.total_target_capacity),
            // "The capacity units that have been fulfilled" — what the Capacity Reservations
            // this call created fulfil, summed in one place.
            total_fulfilled_capacity: Some(fleet_fulfilled_capacity(&reservations)),
            tenancy: Some(spec.tenancy.to_string()),
            end_date: spec.end_date,
            create_time: Some(system_time_to_date_time(created_at)),
            instance_match_criteria: Some(spec.instance_match_criteria.to_string()),
            allocation_strategy: Some(spec.allocation_strategy.to_string()),
            instance_type_specifications: Some(reservations),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(tag_list_bytes(view.tags.as_deref().unwrap_or_default()));
        let stored = view.clone();
        self.capacity_reservation_fleets.insert(
            fleet_id,
            CapacityReservationFleetRecord {
                view,
                previous_state: CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED.to_string(),
                pending: Some(CapacityReservationFleetTransition::due_in(
                    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
                    CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY,
                )),
            },
        );
        stored
    }

    /// Apply a `ModifyCapacityReservationFleet` to the Fleet stored under `fleet_id`.
    ///
    /// The one way a stored Fleet is modified, so what a modification changes about it — the
    /// capacity its Capacity Reservations reserve, when it expires, the state it reports and
    /// the move the clock then owes it — cannot be applied two ways.
    ///
    /// The CALL reserves the new capacity: "the Fleet automatically creates new Capacity
    /// Reservations, or modifies or cancels existing Capacity Reservations in the Fleet to
    /// meet the new total target capacity" (`ModifyCapacityReservationFleet`), so the
    /// reservations are facts of this call rather than of a later instant. What the Fleet
    /// still owes the clock is its own STATE word: it is answered in the non-terminal
    /// `modifying` and reaches `active` one [`TRANSITION_DELAY`] later.
    ///
    /// Answers whether the Fleet was modified: `false` when the id names no Fleet of the
    /// account, or names one in a state a modification does not run from
    /// ([`CAPACITY_RESERVATION_FLEET_MODIFIABLE_STATES`]). A caller resolves the Fleet and its
    /// state first — through the record's own [`CapacityReservationFleetRecord::is_modifiable`]
    /// — and words those two refusals itself; this guard is what keeps the store from being
    /// written behind them.
    pub fn modify_capacity_reservation_fleet(
        &mut self,
        fleet_id: &str,
        change: &CapacityReservationFleetModification,
    ) -> bool {
        let Some(record) = self.capacity_reservation_fleets.get(fleet_id) else {
            return false;
        };
        if !record.is_modifiable() {
            return false;
        }
        // Which specification the Fleet reserves on, and the id for a Capacity Reservation
        // this call has to CREATE there — minted before the record is borrowed mutably,
        // because only the store mints ids.
        let reservations = record
            .view
            .instance_type_specifications
            .as_deref()
            .unwrap_or_default();
        let chosen =
            prioritized_fleet_specification(reservations, |reservation| reservation.priority);
        let needs_reservation_id = chosen.is_some_and(|index| {
            reservations
                .get(index)
                .is_some_and(|reservation| reservation.capacity_reservation_id.is_none())
        });
        let minted_reservation_id = (change.total_target_capacity.is_some()
            && needs_reservation_id)
            .then(|| self.next_id(CAPACITY_RESERVATION_ID_PREFIX));

        let now = SystemTime::now();
        let Some(record) = self.capacity_reservation_fleets.get_mut(fleet_id) else {
            return false;
        };
        if let Some(total_target_capacity) = change.total_target_capacity {
            record.reserve_target_capacity(
                total_target_capacity,
                chosen,
                minted_reservation_id,
                now,
            );
        }
        match change.end_date {
            CapacityReservationFleetEndDate::AsStored => {}
            // "The date and time at which the Capacity Reservation Fleet expires. When the
            // Capacity Reservation Fleet expires, its state changes to `expired`"
            // (`ModifyCapacityReservationFleetRequest$EndDate`).
            CapacityReservationFleetEndDate::MovedTo(end_date) => {
                record.view.end_date = Some(end_date);
            }
            // "If you remove the end date, the Capacity Reservation Fleet does not expire and
            // it remains active until you explicitly cancel it"
            // (`$RemoveEndDate`), so the Fleet then reports no end date at all.
            CapacityReservationFleetEndDate::Removed => record.view.end_date = None,
        }
        record.begin_modification();
        true
    }

    /// Apply every Capacity Reservation Fleet move the clock owes, as of `now`.
    ///
    /// Called by [`Self::materialize_due_transitions`], which is the one place every owed
    /// move is applied from.
    pub(super) fn materialize_due_capacity_reservation_fleet_moves(&mut self, now: Instant) {
        for record in self.capacity_reservation_fleets.values_mut() {
            let due = record
                .pending
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .map(|pending| pending.to);
            if let Some(to) = due {
                record.apply_due_move(to);
            }
        }
    }
}

/// What a `ModifyCapacityReservationFleet` does to a Fleet's end date.
///
/// Three outcomes rather than an `Option`, because "Indicates whether to remove the end date
/// from the Capacity Reservation Fleet"
/// (`ModifyCapacityReservationFleetRequest$RemoveEndDate`) and a request that names neither
/// member ask for different things: taking the end date away, and leaving it as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CapacityReservationFleetEndDate {
    /// The request named neither `EndDate` nor `RemoveEndDate`, so the Fleet keeps the end
    /// date it holds.
    AsStored,
    /// "The date and time at which the Capacity Reservation Fleet expires"
    /// (`ModifyCapacityReservationFleetRequest$EndDate`).
    MovedTo(::aws_smithy_types::DateTime),
    /// "Indicates whether to remove the end date from the Capacity Reservation Fleet"
    /// (`$RemoveEndDate`): the Fleet then "does not expire and it remains active until you
    /// explicitly cancel it".
    Removed,
}

/// What a `ModifyCapacityReservationFleet` changes about a stored Fleet, as its handler has
/// already validated it.
#[derive(Debug, Clone, Copy)]
pub struct CapacityReservationFleetModification {
    /// "The total number of capacity units to be reserved by the Capacity Reservation Fleet"
    /// (`ModifyCapacityReservationFleetRequest$TotalTargetCapacity`), or `None` for a request
    /// that leaves the Fleet's capacity where it is.
    pub total_target_capacity: Option<i32>,
    /// What the request does to the Fleet's end date.
    pub end_date: CapacityReservationFleetEndDate,
}

// =============================================================================
// Customer gateways
// =============================================================================

/// The id prefix a customer gateway's `CustomerGatewayId` carries (`cgw-0123456789abcdef0`),
/// which [`ComAmazonawsEc2Amazonec2State::next_id`] mints the long form of.
pub const CUSTOMER_GATEWAY_ID_PREFIX: &str = "cgw";

/// The `ResourceType` a `CreateCustomerGateway` tag specification may name: the customer
/// gateway it creates ("The tags to apply to the customer gateway",
/// `CreateCustomerGatewayRequest$TagSpecifications`), which is the `ResourceType` enum's
/// `customer-gateway` member ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub const RESOURCE_TYPE_CUSTOMER_GATEWAY: &str = "customer-gateway";

/// The ASN a customer gateway gets when the request names neither `BgpAsn` nor
/// `BgpAsnExtended`: "Default: 65000" (`CreateCustomerGatewayRequest$BgpAsn`).
pub const DEFAULT_CUSTOMER_GATEWAY_BGP_ASN: i64 = 65_000;

/// The ASN range `BgpAsn` carries: "Valid values: 1 to 2,147,483,647"
/// (`CreateCustomerGatewayRequest$BgpAsn`).
pub const CUSTOMER_GATEWAY_BGP_ASN: ::std::ops::RangeInclusive<i64> = 1..=2_147_483_647;

/// The ASN range `BgpAsnExtended` carries: "Valid values: 2,147,483,648 to 4,294,967,295"
/// (`CreateCustomerGatewayRequest$BgpAsnExtended`) — "If the ASN is larger than 2,147,483,647,
/// you must use BgpAsnExtended", which is what makes the two ranges disjoint.
pub const CUSTOMER_GATEWAY_BGP_ASN_EXTENDED: ::std::ops::RangeInclusive<i64> =
    2_147_483_648..=4_294_967_295;

/// How many characters a customer gateway device name may carry: "Length Constraints: Up to
/// 255 characters" (`CreateCustomerGatewayRequest$DeviceName`), which the model publishes as
/// prose rather than as a `@length` the boundary validator could apply.
pub const MAX_CUSTOMER_GATEWAY_DEVICE_NAME_CHARS: usize = 255;

/// Refuse an ASN a customer gateway member does not take: HTTP 400 `InvalidParameterValue`,
/// naming the member and the value the request sent.
///
/// The service classifies an ASN outside the range its member publishes as a FORMAT problem
/// — the value is not one that member can carry — so the wording states that and not the
/// range: the range is the member's published documentation, which a refusal does not
/// repeat. One site for both members, so the two cannot drift.
pub(super) fn invalid_asn_format<E>(parameter: &str, asn: i64) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value ({asn}) for parameter {parameter} is invalid. Invalid Format."
    ))
}

/// The ASN a customer gateway create asks for, as the gateway reports it: the member the
/// request named it in, holding each to the range that member publishes, or
/// [`DEFAULT_CUSTOMER_GATEWAY_BGP_ASN`] when it named neither.
///
/// "You must specify either BgpAsn or BgpAsnExtended when creating the customer gateway. If
/// the ASN is larger than 2,147,483,647, you must use BgpAsnExtended"
/// (`CreateCustomerGatewayRequest$BgpAsn`): the two members are two halves of ONE ASN, so a
/// request naming both names two ASNs for one device and is refused rather than having one
/// silently win. The answer is the pair the gateway reports — the modelled `CustomerGateway`
/// carries the ASN as text, in whichever of its two members the ASN's size puts it.
pub fn customer_gateway_bgp_asn<E>(
    bgp_asn: Option<i32>,
    bgp_asn_extended: Option<i64>,
) -> Result<(Option<String>, Option<String>), Scaffolded<E>> {
    match (bgp_asn, bgp_asn_extended) {
        (Some(_), Some(_)) => Err(invalid_parameter_combination(
            "Either BgpAsn or BgpAsnExtended may be specified for a customer gateway, not \
             both",
        )),
        (Some(asn), None) => {
            let asn = i64::from(asn);
            if !CUSTOMER_GATEWAY_BGP_ASN.contains(&asn) {
                return Err(invalid_asn_format("bgpAsn", asn));
            }
            Ok((Some(asn.to_string()), None))
        }
        (None, Some(asn)) => {
            if !CUSTOMER_GATEWAY_BGP_ASN_EXTENDED.contains(&asn) {
                return Err(invalid_asn_format("bgpAsnExtended", asn));
            }
            Ok((None, Some(asn.to_string())))
        }
        (None, None) => Ok((Some(DEFAULT_CUSTOMER_GATEWAY_BGP_ASN.to_string()), None)),
    }
}

/// The code EC2 answers a request naming a customer gateway id this account holds no gateway
/// for.
///
/// There is no second refusal for an id that is not SHAPED like a `CustomerGatewayId`:
/// `smithy_query(query="shape",shape="CustomerGatewayId")` is a plain string with no pattern,
/// so nothing here knows the form of an id this service did not mint, and the recorded wire
/// corpus shows both the short (`cgw-` plus 8 hexadecimal digits) and the long (17) form of an
/// id nothing holds answered with this one code. Every value that resolves to no gateway is
/// therefore answered the same way, as it is on the VPN connection's static-route surface
/// ([`invalid_vpn_connection_id`]).
pub const INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND: &str = "InvalidCustomerGatewayID.NotFound";

/// Refuse a request that named a customer gateway id this account holds no gateway for: HTTP
/// 400 [`INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND`], worded by the one place this module words a
/// missing resource id ([`id_not_found`]).
pub fn invalid_customer_gateway_id_not_found<E>(customer_gateway_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND,
        "customerGateway ID",
        customer_gateway_id,
    )
}

/// EC2's error code for a create that would take the account past its customer gateway quota.
pub const CUSTOMER_GATEWAY_LIMIT_EXCEEDED: &str = "CustomerGatewayLimitExceeded";

/// Refuse a create that would take the store past [`MAX_STORED_RECORDS_PER_STORE`]: HTTP 400
/// [`CUSTOMER_GATEWAY_LIMIT_EXCEEDED`], on the contract of [`vpn_gateway_limit_exceeded`].
pub fn customer_gateway_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        CUSTOMER_GATEWAY_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of customer gateways has been reached \
             ({MAX_STORED_RECORDS_PER_STORE})"
        ),
    )
}

/// One stored customer gateway: the modelled [`CustomerGateway`] every operation that reports
/// one answers with, plus the move the clock owes it while its state is not a terminal one.
///
/// "The current state of the customer gateway (`pending | available | deleting | deleted`)"
/// (`CustomerGateway$State`) is the same four words the `VpnState` enum publishes, so this
/// section holds no second copy of them: [`VPN_STATE_PENDING`], [`VPN_STATE_AVAILABLE`],
/// [`VPN_STATE_DELETING`] and [`VPN_STATE_DELETED`] are the words a gateway's state is
/// spelled with.
#[derive(Debug, Clone, Default)]
pub struct CustomerGatewayRecord {
    /// The gateway as the model shapes it — what every reporting operation answers with.
    pub view: CustomerGateway,
    /// The state the clock owes this gateway while it is in a non-terminal one. `None` once
    /// nothing is owed.
    pub pending: Option<PendingStateMove>,
}

impl CustomerGatewayRecord {
    /// The gateway's own `CustomerGatewayId`.
    pub fn customer_gateway_id(&self) -> &str {
        self.view.customer_gateway_id.as_deref().unwrap_or_default()
    }

    /// The gateway's `CustomerGateway$State` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// The caller-chosen bytes this record holds, through the one definition of that charge
    /// ([`customer_gateway_payload_bytes`]).
    pub fn payload_bytes(&self) -> usize {
        customer_gateway_payload_bytes(&self.view)
    }

    /// Take this gateway out of service: move it to the terminal [`VPN_STATE_DELETED`] and
    /// leave nothing owed.
    ///
    /// `deleted` is a `CustomerGateway$State` word, so a deleted gateway is still a gateway the
    /// API reports rather than a record that vanishes. The move is immediate and NOT a span the
    /// clock owes: a customer gateway is a RECORD of a device the customer operates, with
    /// nothing to provision or tear down, which is also why a create answers one already in
    /// service. One already gone is left exactly as it is, so repeating the request reports no
    /// second deletion.
    ///
    /// Reach it through [`ComAmazonawsEc2Amazonec2State::require_customer_gateway_mut`], the one
    /// place an operation resolves the gateway it is about.
    pub fn delete(&mut self) {
        self.view.state = Some(VPN_STATE_DELETED.to_string());
        self.pending = None;
    }
}

/// The caller-chosen bytes one stored customer gateway holds: its tags and each text member a
/// request chooses the value of. Everything else on the record — the id and the state — is
/// derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateCustomerGateway` asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_customer_gateway`] charges exactly it.
pub fn customer_gateway_payload_bytes(view: &CustomerGateway) -> usize {
    let text = |value: &Option<String>| value.as_deref().map_or(0, str::len);
    tag_list_bytes(view.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&view.ip_address))
        .saturating_add(text(&view.device_name))
        .saturating_add(text(&view.certificate_arn))
        .saturating_add(text(&view.bgp_asn))
        .saturating_add(text(&view.bgp_asn_extended))
}

/// The move the clock owes a customer gateway whose state is `state`, or `None` when that
/// state is already terminal.
///
/// The ONE place that rule is written for this resource: a gateway reported in a non-terminal
/// state still owes the move to its terminal one, and that move falls due at its own instant
/// whether or not anybody calls again — `pending` becomes `available`, `deleting` becomes
/// `deleted`.
pub(super) fn owed_customer_gateway_move(state: &str) -> Option<PendingStateMove> {
    match state {
        VPN_STATE_PENDING => Some(PendingStateMove::starting_now(VPN_STATE_AVAILABLE)),
        VPN_STATE_DELETING => Some(PendingStateMove::starting_now(VPN_STATE_DELETED)),
        _ => None,
    }
}

/// The store's customer gateways, and the one way one is written.
///
/// [`Self::has_customer_gateway_capacity`] and [`Self::insert_customer_gateway`] are that
/// write path, reached from `CreateCustomerGateway` — the one operation that mints a gateway —
/// and from the test harness that seeds one in a state a create does not answer with.
impl ComAmazonawsEc2Amazonec2State {
    /// Whether another customer gateway fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_vpn_connection_capacity`].
    pub fn has_customer_gateway_capacity(&self) -> bool {
        self.customer_gateways.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The customer gateway `customer_gateway_id` names, if this account holds one.
    pub fn customer_gateway(&self, customer_gateway_id: &str) -> Option<&CustomerGatewayRecord> {
        self.customer_gateways.get(customer_gateway_id)
    }

    /// The customer gateway `customer_gateway_id` names, or the one refusal every operation
    /// that resolves such an id answers for a value it cannot resolve:
    /// [`invalid_customer_gateway_id_not_found`].
    pub fn require_customer_gateway<E>(
        &self,
        customer_gateway_id: &str,
    ) -> Result<&CustomerGatewayRecord, Scaffolded<E>> {
        self.customer_gateways
            .get(customer_gateway_id)
            .ok_or_else(|| invalid_customer_gateway_id_not_found(customer_gateway_id))
    }

    /// The customer gateway `customer_gateway_id` names, for a writer — the same refusal
    /// [`Self::require_customer_gateway`] answers for an id it cannot resolve.
    pub fn require_customer_gateway_mut<E>(
        &mut self,
        customer_gateway_id: &str,
    ) -> Result<&mut CustomerGatewayRecord, Scaffolded<E>> {
        self.customer_gateways
            .get_mut(customer_gateway_id)
            .ok_or_else(|| invalid_customer_gateway_id_not_found(customer_gateway_id))
    }

    /// The `VpnConnectionId` of a VPN connection still USING the customer gateway
    /// `customer_gateway_id` names, if the account holds one.
    ///
    /// "You must delete the VPN connection before you can delete the customer gateway"
    /// (`DeleteCustomerGateway`): a connection in a terminal or outgoing state no longer uses
    /// its gateway, so only one in [`VPN_STATE_PENDING`] or [`VPN_STATE_AVAILABLE`] counts. The
    /// ONE place that question is asked, so the delete and any later reader of the dependency
    /// cannot disagree about it.
    pub fn vpn_connection_using_customer_gateway(&self, customer_gateway_id: &str) -> Option<&str> {
        self.vpn_connections
            .values()
            .find(|record| {
                record.view.customer_gateway_id.as_deref() == Some(customer_gateway_id)
                    && matches!(record.state(), VPN_STATE_PENDING | VPN_STATE_AVAILABLE)
            })
            .map(VpnConnectionRecord::vpn_connection_id)
    }

    /// The customer gateway an IDENTICAL create would be answered with: "An identical request
    /// returns information about the existing customer gateway; it doesn't create a new
    /// customer gateway" (`CreateCustomerGateway`).
    ///
    /// What makes two requests identical is what that documentation names: "To create more
    /// than one customer gateway with the same VPN type, IP address, and BGP ASN, specify a
    /// unique device name for each customer gateway" — the type, the outside address, the ASN
    /// and the device name together. A gateway on its way out or already gone is NOT one such
    /// a request can be answered with, so only a gateway in [`VPN_STATE_PENDING`] or
    /// [`VPN_STATE_AVAILABLE`] is matched: a create that repeats a deleted gateway's
    /// description creates a new one.
    pub fn customer_gateway_like(&self, view: &CustomerGateway) -> Option<&CustomerGatewayRecord> {
        self.customer_gateways.values().find(|record| {
            matches!(record.state(), VPN_STATE_PENDING | VPN_STATE_AVAILABLE)
                && record.view.r#type == view.r#type
                && record.view.ip_address == view.ip_address
                && record.view.bgp_asn == view.bgp_asn
                && record.view.bgp_asn_extended == view.bgp_asn_extended
                && record.view.device_name == view.device_name
        })
    }

    /// Store a customer gateway and answer the `CustomerGatewayId` it was given — or `None`
    /// when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was stored.
    ///
    /// The id is minted here rather than taken from `view`, the way
    /// [`Self::insert_vpn_connection`] mints a connection id, and the move the clock owes a
    /// gateway written in a non-terminal state is recorded by [`owed_customer_gateway_move`]
    /// rather than slept for. The caller-chosen bytes the record holds are charged on top of
    /// the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
    pub fn insert_customer_gateway(&mut self, mut view: CustomerGateway) -> Option<String> {
        if !self.has_customer_gateway_capacity() {
            return None;
        }
        let customer_gateway_id = self.next_id(CUSTOMER_GATEWAY_ID_PREFIX);
        view.customer_gateway_id = Some(customer_gateway_id.clone());
        let pending = owed_customer_gateway_move(view.state.as_deref().unwrap_or_default());
        let record = CustomerGatewayRecord { view, pending };
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + record.payload_bytes());
        self.customer_gateways
            .insert(customer_gateway_id.clone(), record);
        Some(customer_gateway_id)
    }
}

// =============================================================================
// Site-to-Site VPN connections
// =============================================================================

/// The id prefix a Site-to-Site VPN connection's `VpnConnectionId` carries.
pub const VPN_CONNECTION_ID_PREFIX: &str = "vpn";

/// A VPN connection or static route being brought up: the `VpnState` enum's `pending`
/// member (`smithy_query(query="shape",shape="VpnState")`).
pub const VPN_STATE_PENDING: &str = "pending";

/// A VPN connection or static route in service: the `VpnState` enum's `available` member.
pub const VPN_STATE_AVAILABLE: &str = "available";

/// A VPN connection or static route being taken down: the `VpnState` enum's `deleting`
/// member.
pub const VPN_STATE_DELETING: &str = "deleting";

/// A VPN connection or static route that is gone: the `VpnState` enum's `deleted` member.
pub const VPN_STATE_DELETED: &str = "deleted";

/// "Indicates how the routes were provided" (`VpnStaticRoute$Source`) for a route a caller
/// added itself: the single member the `VpnStaticRouteSource` enum publishes.
pub const VPN_STATIC_ROUTE_SOURCE_STATIC: &str = "Static";

/// The code EC2 answers for a VPN connection id this account holds no connection for.
pub const INVALID_VPN_CONNECTION_ID: &str = "InvalidVpnConnectionId";

/// The code EC2 answers a request that DELETES the connection a VPN connection id names when
/// this account holds no connection for it.
///
/// NOT [`INVALID_VPN_CONNECTION_ID`], and the recorded wire corpus is why: the STATIC ROUTE
/// surface of this resource answers `InvalidVpnConnectionId` with its own wording
/// ([`invalid_vpn_connection_id`]), while the delete of the connection itself answers this
/// `.NotFound` code worded the way this module words every missing resource id. A client
/// branches on the code it is actually answered, so the two are kept apart rather than folded
/// into one.
pub const INVALID_VPN_CONNECTION_ID_NOT_FOUND: &str = "InvalidVpnConnectionID.NotFound";

/// The code EC2 answers for a request that would take a VPN connection past the number of
/// static routes it can hold — EC2's own `RouteLimitExceeded`, "you've reached the limit on
/// the number of routes that you can add" (EC2 API error codes).
pub const ROUTE_LIMIT_EXCEEDED: &str = "RouteLimitExceeded";

/// How many static routes one VPN connection holds here.
///
/// A capacity bound in the style of [`MAX_VPCS`], not a published quota: each route is a
/// record in this process's memory, so the number a caller can add to one connection has to
/// be finite. A create past it answers [`vpn_route_limit_exceeded`] rather than growing the
/// connection without limit.
pub const MAX_VPN_STATIC_ROUTES: usize = 100;

/// Refuse a VPN connection id this account holds no connection for: HTTP 400
/// [`INVALID_VPN_CONNECTION_ID`].
///
/// The message is built from the request — the id it asked about — and the account the
/// lookup was made in, which is this emulator's own ([`::engine::emulator_account_id`]).
pub fn invalid_vpn_connection_id<E>(vpn_connection_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPN_CONNECTION_ID,
        400,
        format!(
            "The specified VpnConnection ({vpn_connection_id}) for owner ({owner}) \
             does not exist",
            owner = ::engine::emulator_account_id(),
        ),
    )
}

/// Refuse a request that named a VPN connection id this account holds no connection for, as
/// the operations that act on the CONNECTION ITSELF answer it: HTTP 400
/// [`INVALID_VPN_CONNECTION_ID_NOT_FOUND`], worded by the one place this module words a
/// missing resource id ([`id_not_found`]).
///
/// Reached through [`ComAmazonawsEc2Amazonec2State::require_vpn_connection_mut`], which is
/// what those operations resolve an id through.
pub fn invalid_vpn_connection_id_not_found<E>(vpn_connection_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        "vpnConnection ID",
        vpn_connection_id,
    )
}

/// Refuse a VPN connection id this account holds no connection for, as the DELETE of one of
/// its static routes answers it: HTTP 400 [`INVALID_VPN_CONNECTION_ID`], naming the id the
/// request asked about and nothing else.
///
/// The SAME code as [`invalid_vpn_connection_id`] with one clause fewer, and the recorded
/// wire corpus is why: EC2 names the account the lookup was made in only when the static
/// route is being CREATED, so the two wordings of this one code are kept apart rather than
/// folded into one. The id is the request's own.
pub fn invalid_vpn_connection_id_on_route_delete<E>(vpn_connection_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPN_CONNECTION_ID,
        400,
        format!("The specified VpnConnection ({vpn_connection_id}) does not exist"),
    )
}

/// Refuse a destination block the named VPN connection carries no static route for: HTTP 400
/// [`INVALID_ROUTE_NOT_FOUND`], the code this crate already answers a route it cannot resolve
/// inside the resource that holds it with, worded the way that class is worded here
/// ([`local_gateway_route_not_found`]) — the destination the request named and the connection
/// it is not a route of.
pub fn vpn_static_route_not_found<E>(
    vpn_connection_id: &str,
    destination_cidr_block: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_NOT_FOUND,
        400,
        format!(
            "The route '{destination_cidr_block}' does not exist in the VPN connection \
             '{vpn_connection_id}'"
        ),
    )
}

/// Refuse a static route create that would take one connection past
/// [`MAX_VPN_STATIC_ROUTES`]: HTTP 400 [`ROUTE_LIMIT_EXCEEDED`].
pub fn vpn_route_limit_exceeded<E>(vpn_connection_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        ROUTE_LIMIT_EXCEEDED,
        400,
        format!(
            "The VPN connection '{vpn_connection_id}' has reached the maximum number of \
             static routes ({MAX_VPN_STATIC_ROUTES})"
        ),
    )
}

/// A requested VPN static route destination block in canonical form, of whichever address
/// family it names.
///
/// `CreateVpnConnectionRouteRequest$DestinationCidrBlock` is "The CIDR block associated with
/// the local subnet of the customer network" and publishes NO block size, so the family is
/// decided by the value itself and no prefix-length rule is imposed on it — the same
/// reasoning as [`canonical_ipv6_cidr`]'s. The block is read by the same two readers the VPC
/// path uses, so this crate keeps ONE rule for what a CIDR block is, and the host bits are
/// cleared the way every other CIDR member of this service stores its block ("We modify the
/// specified CIDR block to its canonical form", `CreateVpcRequest$CidrBlock`,
/// `CreateSubnetRequest$CidrBlock`) — which is also what makes the destination block
/// identify ONE route of a connection.
pub fn canonical_vpn_static_route_cidr(value: &str) -> Result<String, CidrProblem> {
    if value.contains(':') {
        return canonical_ipv6_cidr(value);
    }
    canonical_ipv4_cidr(value)
}

/// One Site-to-Site VPN connection: the modelled [`VpnConnection`] every operation that
/// reports one answers with, plus the move the clock owes each static route that is not in a
/// terminal state yet.
///
/// The routes live on the view, in its own `Routes` member, so the operation that adds one
/// and the operation that reports them cannot disagree about a connection; only the DUE
/// INSTANT of a route's `pending` -> `available` move, which `VpnStaticRoute` carries no
/// member for, is held beside it, keyed by the same destination CIDR block.
#[derive(Debug, Clone, Default)]
pub struct VpnConnectionRecord {
    /// The connection as the model shapes it, carrying the routes reported so far.
    pub view: VpnConnection,
    /// The move the clock owes one static route, by destination CIDR block, while one is
    /// owed. A route that has reached its terminal state has no entry here.
    pub route_moves: BTreeMap<String, PendingStateMove>,
    /// The `VpnState` the clock owes the CONNECTION itself while it is in a non-terminal one:
    /// `deleted` after a delete has been asked for. `None` once nothing is owed.
    pub pending: Option<PendingStateMove>,
}

impl VpnConnectionRecord {
    /// The connection's `VpnState` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// The connection's own `VpnConnectionId`.
    pub fn vpn_connection_id(&self) -> &str {
        self.view.vpn_connection_id.as_deref().unwrap_or_default()
    }

    /// "The static routes associated with the VPN connection" (`VpnConnection$Routes`).
    pub fn routes(&self) -> &[VpnStaticRoute] {
        self.view.routes.as_deref().unwrap_or_default()
    }

    /// The static route for `destination_cidr_block`, if this connection has one.
    pub fn route(&self, destination_cidr_block: &str) -> Option<&VpnStaticRoute> {
        self.routes()
            .iter()
            .find(|route| route.destination_cidr_block.as_deref() == Some(destination_cidr_block))
    }

    /// The caller-chosen bytes this record holds: its tags and the destination block of
    /// each static route.
    pub fn payload_bytes(&self) -> usize {
        let routes: usize = self
            .routes()
            .iter()
            .map(|route| route.destination_cidr_block.as_deref().unwrap_or_default().len())
            .sum();
        tag_list_bytes(self.view.tags.as_deref().unwrap_or_default()).saturating_add(routes)
    }

    /// Start this connection's deletion: move it into the non-terminal `deleting` state, the
    /// `VpnState` of a connection whose tunnels are being torn down, and record the
    /// [`VPN_STATE_DELETED`] the clock then owes it one [`TRANSITION_DELAY`] later.
    ///
    /// `deleting` and `deleted` are both `VpnState` members, so a connection a delete has
    /// been asked for is still a connection the API reports rather than a record that
    /// vanishes. One already on its way out is left exactly as it is — the deletion this call
    /// asks for is the one already under way, so repeating the request neither reports a
    /// second deletion nor postpones the first by restarting its due instant. The static
    /// routes it carries keep the state their own operations left them in: what a delete
    /// moves is the connection, which is what a caller branches on.
    ///
    /// Reach it through [`ComAmazonawsEc2Amazonec2State::require_vpn_connection_mut`], the
    /// one place an operation resolves the connection it is about.
    pub fn begin_deletion(&mut self) {
        if matches!(self.state(), VPN_STATE_DELETING | VPN_STATE_DELETED) {
            return;
        }
        self.view.state = Some(VPN_STATE_DELETING.to_string());
        self.pending = Some(PendingStateMove::starting_now(VPN_STATE_DELETED));
    }

    /// Put the static route for `destination_cidr_block` into this connection and answer it
    /// as it now stands.
    ///
    /// A route of a connection is identified by its destination block, so a connection that
    /// already carries one for that block is left exactly as it is — including the move the
    /// clock already owes it, which repeating the request must not push further out. A route
    /// this call adds is `pending`, the non-terminal `VpnState` of a route being brought up,
    /// with the clock owing it [`VPN_STATE_AVAILABLE`] one [`TRANSITION_DELAY`] later.
    ///
    /// Reach it through [`ComAmazonawsEc2Amazonec2State::put_vpn_static_route`], which also
    /// charges the bytes the route holds against the store's budget.
    pub(super) fn put_static_route(&mut self, destination_cidr_block: &str) -> VpnStaticRoute {
        if let Some(route) = self.route(destination_cidr_block) {
            return route.clone();
        }
        let route = VpnStaticRoute {
            destination_cidr_block: Some(destination_cidr_block.to_string()),
            source: Some(VPN_STATIC_ROUTE_SOURCE_STATIC.to_string()),
            state: Some(VPN_STATE_PENDING.to_string()),
        };
        self.view
            .routes
            .get_or_insert_with(Vec::new)
            .push(route.clone());
        self.route_moves.insert(
            destination_cidr_block.to_string(),
            PendingStateMove::starting_now(VPN_STATE_AVAILABLE),
        );
        route
    }

    /// Start the deletion of this connection's static route for `destination_cidr_block` and
    /// answer it as it now stands — `None` when the connection carries no route for that
    /// block, in which case nothing was written.
    ///
    /// `deleting` and `deleted` are both `VpnState` members, so a route a delete has been
    /// asked for is left in the TRANSIENT state with the clock owing it the terminal one, the
    /// same arrangement as the connection's own ([`Self::begin_deletion`]). Reaching
    /// [`VPN_STATE_DELETED`] takes the route OUT of the connection's route set
    /// ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]), which is what gives
    /// the bytes it held back to the store's budget: a deleted route is one the connection no
    /// longer carries. A route already on its way out is left exactly as it is, so repeating
    /// the request neither reports a second deletion nor postpones the first by restarting its
    /// due instant.
    ///
    /// Reach it through
    /// [`ComAmazonawsEc2Amazonec2State::begin_vpn_static_route_deletion`], the one write path
    /// for it.
    pub(super) fn begin_static_route_deletion(
        &mut self,
        destination_cidr_block: &str,
    ) -> Option<VpnStaticRoute> {
        let route = self.view.routes.iter_mut().flatten().find(|route| {
            route.destination_cidr_block.as_deref() == Some(destination_cidr_block)
        })?;
        if matches!(
            route.state.as_deref().unwrap_or_default(),
            VPN_STATE_DELETING | VPN_STATE_DELETED
        ) {
            return Some(route.clone());
        }
        route.state = Some(VPN_STATE_DELETING.to_string());
        let deleting = route.clone();
        self.route_moves.insert(
            destination_cidr_block.to_string(),
            PendingStateMove::starting_now(VPN_STATE_DELETED),
        );
        Some(deleting)
    }
}

/// The store's VPN connections, and the one way a connection and a static route are
/// written. Kept in this section, beside the record they write, rather than in one of the
/// module's shared `impl` blocks.
///
/// [`Self::has_vpn_connection_capacity`] and [`Self::insert_vpn_connection`] are the one
/// write path for a CONNECTION, and nothing in `src/ops/` reaches them yet: the operation
/// that will is `CreateVpnConnection`, still a scaffold under its own issue, so today the
/// only caller is the test harness that seeds a connection for the route and describe
/// operations. A reader should not take their existence for "an operation already mints
/// connections".
impl ComAmazonawsEc2Amazonec2State {
    /// Whether another VPN connection fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`] — asked by the writer that mints a
    /// connection, which is `CreateVpnConnection` once it is implemented.
    pub fn has_vpn_connection_capacity(&self) -> bool {
        self.vpn_connections.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The VPN connection `vpn_connection_id` names, if this account holds one.
    pub fn vpn_connection(&self, vpn_connection_id: &str) -> Option<&VpnConnectionRecord> {
        self.vpn_connections.get(vpn_connection_id)
    }

    /// The VPN connection `vpn_connection_id` names, for a writer — or the one refusal the
    /// operations that act on the CONNECTION ITSELF answer for a value they cannot resolve:
    /// [`invalid_vpn_connection_id_not_found`].
    ///
    /// The static-route surface of the same resource resolves its `VpnConnectionId` through
    /// [`Self::vpn_connection`] and [`invalid_vpn_connection_id`] instead, which is a
    /// deliberate second wording rather than a violated invariant: the recorded corpus shows
    /// EC2 answering the two surfaces with different codes.
    ///
    /// There is no second refusal for an id that is not shaped like a `VpnConnectionId`:
    /// `smithy_query(query="shape",shape="VpnConnectionId")` is a plain string with no
    /// pattern, so nothing here knows the form of an id this service did not mint, and every
    /// value that resolves to no connection is answered the same way — as it is on that
    /// static-route surface.
    pub fn require_vpn_connection_mut<E>(
        &mut self,
        vpn_connection_id: &str,
    ) -> Result<&mut VpnConnectionRecord, Scaffolded<E>> {
        self.vpn_connections
            .get_mut(vpn_connection_id)
            .ok_or_else(|| invalid_vpn_connection_id_not_found(vpn_connection_id))
    }

    /// Store a VPN connection and answer the `VpnConnectionId` it was given — or `None`
    /// when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was stored.
    ///
    /// The id is minted here rather than taken from `view`, the way
    /// [`Self::insert_transit_gateway_attachment`] mints an attachment id, and the
    /// caller-chosen bytes the record holds are charged on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. Its first production
    /// caller will be `CreateVpnConnection`, which will also charge the members only a
    /// create can set; until then it is reached from the test harness alone.
    pub fn insert_vpn_connection(&mut self, mut view: VpnConnection) -> Option<String> {
        if !self.has_vpn_connection_capacity() {
            return None;
        }
        let vpn_connection_id = self.next_id(VPN_CONNECTION_ID_PREFIX);
        view.vpn_connection_id = Some(vpn_connection_id.clone());
        let record = VpnConnectionRecord {
            view,
            route_moves: BTreeMap::new(),
            // A connection is stored in the state its writer gave it, with nothing owed: the
            // move a delete owes is recorded by [`VpnConnectionRecord::begin_deletion`].
            pending: None,
        };
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + record.payload_bytes());
        self.vpn_connections
            .insert(vpn_connection_id.clone(), record);
        Some(vpn_connection_id)
    }

    /// Put the static route for `destination_cidr_block` into the connection
    /// `vpn_connection_id` names and answer it as it now stands — or `None` when this
    /// account holds no such connection, in which case nothing was written.
    ///
    /// The ONE place a static route is stored, so the state it starts in, the move the clock
    /// owes it and the bytes it charges cannot be built two ways.
    pub fn put_vpn_static_route(
        &mut self,
        vpn_connection_id: &str,
        destination_cidr_block: &str,
    ) -> Option<VpnStaticRoute> {
        let record = self.vpn_connections.get_mut(vpn_connection_id)?;
        let held = record.route(destination_cidr_block).is_some();
        let route = record.put_static_route(destination_cidr_block);
        if !held {
            // A route already held was not stored a second time, so it charges nothing more.
            self.add_stored_bytes(destination_cidr_block.len());
        }
        Some(route)
    }

    /// Start the deletion of the static route for `destination_cidr_block` on the connection
    /// `vpn_connection_id` names and answer it as it now stands — or `None` when this account
    /// holds no such connection or the connection carries no route for that block, in which
    /// case nothing was written.
    ///
    /// The ONE place a static route's deletion is started, so the state it moves to and the
    /// instant its removal falls due cannot be built two ways — the other end of the life
    /// [`Self::put_vpn_static_route`] begins.
    pub fn begin_vpn_static_route_deletion(
        &mut self,
        vpn_connection_id: &str,
        destination_cidr_block: &str,
    ) -> Option<VpnStaticRoute> {
        self.vpn_connections
            .get_mut(vpn_connection_id)?
            .begin_static_route_deletion(destination_cidr_block)
    }
}
