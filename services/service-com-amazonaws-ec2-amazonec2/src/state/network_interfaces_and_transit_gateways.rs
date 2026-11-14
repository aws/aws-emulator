// SPDX-License-Identifier: CC0-1.0

//! The network-interface, address-transfer and transit-gateway stores, and the route-table
//! propagation and association decisions a transit gateway's writers share.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;
use crate::ops::types::{InstanceNetworkInterfaceAssociation, NetworkInterfaceAssociation};

/// Whether an interface of type `interface_type` has a source/destination check at all.
///
/// The check is about the IP traffic an interface sends and receives — "source/destination
/// checks … ensure that the instance is either the source or the destination of any traffic
/// that it receives" (`NetworkInterface$SourceDestCheck`) — so an interface type that
/// carries no IP addresses has none: "EFA-only network interfaces do not support IP
/// addresses" ([`NETWORK_INTERFACE_TYPE_EFA_ONLY`]). Every other type does.
///
/// The one definition of that rule, read by the operations that CHANGE the attribute
/// (`ModifyNetworkInterfaceAttribute` and `ResetNetworkInterfaceAttribute`), so they refuse
/// the same interfaces in the same words
/// ([`interface_type_cannot_change_source_dest_check`]).
pub fn interface_type_has_source_dest_check(interface_type: &str) -> bool {
    interface_type != NETWORK_INTERFACE_TYPE_EFA_ONLY
}

/// Refuse a change to the source/destination check of an interface whose type does not have
/// one ([`interface_type_has_source_dest_check`]): HTTP 400 `InvalidParameterValue`.
///
/// The sentence the real service answers there, whose one variable part is the interface's
/// TYPE, read off the stored record rather than from the request — the request names the
/// interface by id and never its type.
pub fn interface_type_cannot_change_source_dest_check<E>(interface_type: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Interface type: {interface_type} cannot modify source destination check"
    ))
}

/// The id prefix EC2 mints a network interface with, as
/// [`network_interface_id_is_well_formed`] reads one: `eni-`, carried here without the
/// separator [`ComAmazonawsEc2Amazonec2State::next_id`] adds.
pub const NETWORK_INTERFACE_ID_PREFIX: &str = "eni";

/// The id prefix EC2 mints the attachment of a network interface to an instance with, which
/// is what `NetworkInterfaceAttachment$AttachmentId` names (`eni-attach-…`, as the
/// `NetworkInterfaceAttachmentId` ids in EC2's own documentation read).
pub const NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX: &str = "eni-attach";

/// "The type of network interface. The default is interface"
/// (`CreateNetworkInterfaceRequest$InterfaceType`) — the `NetworkInterfaceType` member an
/// interface a caller creates without asking for another type carries
/// (`smithy_query(query="shape",shape="NetworkInterfaceType")`).
pub const NETWORK_INTERFACE_TYPE_INTERFACE: &str = "interface";

/// The type of the endpoint network interface an interface VPC endpoint is served by: that
/// enum's `vpc_endpoint` member ([`crate::ops::types::NETWORK_INTERFACE_TYPE_VALUES`]).
pub const NETWORK_INTERFACE_TYPE_VPC_ENDPOINT: &str = "vpc_endpoint";

/// The type of the endpoint network interface a Gateway Load Balancer endpoint is served
/// by: that enum's `gateway_load_balancer_endpoint` member.
pub const NETWORK_INTERFACE_TYPE_GATEWAY_LOAD_BALANCER_ENDPOINT: &str =
    "gateway_load_balancer_endpoint";

/// The status of a network interface nothing holds: the `NetworkInterfaceStatus` enum's
/// `available` member (`smithy_query(query="shape",shape="NetworkInterfaceStatus")`), which
/// is where an interface with no attachment sits — and the one state a delete accepts.
pub const NETWORK_INTERFACE_STATUS_AVAILABLE: &str = "available";

/// The status of a network interface something holds: the `NetworkInterfaceStatus` enum's
/// `in-use` member, which is where an interface with an attachment sits.
pub const NETWORK_INTERFACE_STATUS_IN_USE: &str = "in-use";

/// The `AttachmentStatus` of an attachment that has been taken away
/// (`smithy_query(query="shape",shape="AttachmentStatus")`): the one attachment status that
/// leaves the interface holding nothing, so it is the one a delete does not refuse.
pub const ATTACHMENT_STATUS_DETACHED: &str = "detached";

/// The `AttachmentStatus` of a finished attachment: the state an attachment the clock has
/// completed sits in.
pub const ATTACHMENT_STATUS_ATTACHED: &str = "attached";

/// The `AttachmentStatus` of an attachment that is being brought up: the `attaching` member
/// of that enum, which is where `AttachNetworkInterface` leaves one. NON-TERMINAL — the clock
/// owes it [`ATTACHMENT_STATUS_ATTACHED`] one [`TRANSITION_DELAY`] later
/// ([`ComAmazonawsEc2Amazonec2State::network_interface_attachment_move_at`]).
pub const ATTACHMENT_STATUS_ATTACHING: &str = "attaching";

/// The `AttachmentStatus` of an attachment that is being taken away: the `detaching` member
/// of that enum, which is where `DetachNetworkInterface` leaves one. NON-TERMINAL — the clock
/// owes the attachment its completion one [`TRANSITION_DELAY`] later
/// ([`ComAmazonawsEc2Amazonec2State::network_interface_attachment_move_at`]), at which point
/// the interface holds nothing at all.
pub const ATTACHMENT_STATUS_DETACHING: &str = "detaching";

/// The device index the PRIMARY network interface of an instance is attached at.
///
/// "The primary network interface is attached at device index 0"
/// (`InstanceNetworkInterfaceAttachment$DeviceIndex` / the EC2 User Guide's "Elastic network
/// interfaces"): it is how a client finds the interface it must modify among an instance's
/// interfaces, so the launch that creates it and anything that reads it name the one
/// constant.
pub const PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX: i32 = 0;

/// `InvalidNetworkInterface.InUse` (HTTP 400): the request asked for something that cannot
/// be done to a network interface while it is still attached. "The specified interface is
/// currently in use and cannot be deleted or attached to another instance. Ensure that you
/// have detached the network interface first" (EC2 API error codes,
/// "InvalidNetworkInterface.InUse").
pub const INVALID_NETWORK_INTERFACE_IN_USE: &str = "InvalidNetworkInterface.InUse";

/// Refuse an action on a network interface something still holds: HTTP 400
/// `InvalidNetworkInterface.InUse`. "You must detach the network interface before you can
/// delete it" (`DeleteNetworkInterface`).
pub fn network_interface_in_use<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_IN_USE,
        400,
        format!(
            "The network interface '{network_interface_id}' is currently in use and must be \
             detached first"
        ),
    )
}

/// `InvalidNetworkInterfaceAttachmentId.Malformed` (HTTP 400): the request named a value that
/// is not a network interface attachment id this service could have issued, so there is
/// nothing to look up.
pub const INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED: &str =
    "InvalidNetworkInterfaceAttachmentId.Malformed";

/// `InvalidAttachmentID.NotFound` (HTTP 400): a readable attachment id that names no
/// attachment the account holds — the not-found half of the pair whose malformed half is
/// [`INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED`].
pub const INVALID_ATTACHMENT_ID_NOT_FOUND: &str = "InvalidAttachmentID.NotFound";

/// Refuse a value that is not a network interface attachment id this service could have
/// issued: HTTP 400 [`INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED`], BEFORE any
/// attachment is looked up, so "not an id of this kind" is never reported as "no such
/// attachment".
///
/// The message is the resource-agnostic [`invalid_id_message`] template — `Invalid id:
/// "<value>"` — which is the wording recorded for a refused attachment id, and the one every
/// id kind of this service that has no sentence of its own carries. The one variable part is
/// the value the REQUEST sent, bounded by [`echoed_resource_id`].
pub fn invalid_network_interface_attachment_id_malformed<E>(attachment_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED,
        400,
        invalid_id_message(attachment_id),
    )
}

/// Refuse a readable attachment id the account holds no attachment for: HTTP 400
/// [`INVALID_ATTACHMENT_ID_NOT_FOUND`].
///
/// Worded the way this family of refusals words one about an id of a NAMED kind — the kind,
/// the value from the request in single quotes, and that it does not exist — which is the
/// sentence [`invalid_network_interface_id_not_found`] carries for the interface the
/// attachment is of.
pub fn invalid_attachment_id_not_found<E>(attachment_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ATTACHMENT_ID_NOT_FOUND,
        400,
        format!("The attachment ID '{attachment_id}' does not exist"),
    )
}

/// Refuse a detach of the PRIMARY network interface of an instance: HTTP 400
/// `OperationNotPermitted`.
///
/// "The primary network interface is attached at device index 0"
/// ([`PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX`]): an instance in a VPC always has one and it
/// goes with the instance, so it is not an attachment a caller may take down — whatever
/// `Force` says, since forcing is about an instance that is not answering rather than about
/// which attachments exist. The sentence names the two indexes of the attachment the request
/// selected, both read off the stored attachment rather than off the request.
pub fn primary_network_interface_cannot_be_detached<E>(
    device_index: i32,
    network_card_index: i32,
) -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "The network interface at device index {device_index} and networkcard index \
         {network_card_index} cannot be detached."
    ))
}

/// Refuse a detach of an attachment ANOTHER Amazon Web Services service made and manages:
/// HTTP 400 `OperationNotPermitted`.
///
/// "Indicates whether the network interface is being managed by Amazon Web Services"
/// (`NetworkInterface$RequesterManaged`): such an interface was created for a service's own
/// resource — an interface VPC endpoint's, say — so its attachment is not the account's to
/// take down, and the refusal is about the ATTACHMENT rather than about the interface's state.
///
/// The sentence names the KIND of attachment the caller may not manage — the id PREFIX of that
/// kind, [`NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX`] — which is the form EC2 words this class
/// of refusal in, rather than the one id the request happened to name: what the caller may not
/// do is manage attachments of that kind at all, so the text carries no per-request value.
pub fn attachment_is_not_the_accounts_to_manage<E>() -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "You are not allowed to manage '{NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX}' attachments."
    ))
}

// ---------------------------------------------------------------------------
// Trunk interfaces: the account-level entitlement EC2 gates them behind
// ---------------------------------------------------------------------------

/// Refuse a branch/trunk network-interface association to a caller whose account may not
/// use interface trunking: HTTP 400 `OperationNotPermitted`, naming the account the request
/// was made by.
///
/// Joining a `branch` network interface to a `trunk` one is not part of the EC2 API every
/// account may call: the capability is granted to an account out of band, by AWS, and an
/// account that does not hold it is refused the OPERATION rather than told anything about
/// the interfaces, the VLAN or the GRE key the request named. So this refusal is the answer
/// before any member of the request is interpreted — an id the boundary validator admitted
/// is neither resolved nor checked for shape — and a `DryRun` request reads it too: the dry
/// run asks whether the request would have succeeded, and this one would not.
///
/// `User` is the account the request was made by, which for this emulator is the one account
/// it serves ([`::engine::emulator_account_id`]) — nothing a request carries can name
/// another, and no operation of this API grants the capability, so every caller of a trunk
/// interface operation reads this.
///
/// One function rather than a literal per handler, so the three operations the capability
/// gates — the associate, its reader and the disassociate — cannot word one refusal three
/// ways.
pub fn trunk_interface_not_permitted<E>() -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "User {account} is not permitted to perform this operation",
        account = ::engine::emulator_account_id(),
    ))
}

// What one network interface is created with lives in [`NetworkInterfaceSpec`], beside the
// writer seam [`ComAmazonawsEc2Amazonec2State::insert_network_interface`] further up, so a
// create and a seeded fixture ask for an interface the same way.

/// What one attachment of a network interface to an instance is made with, beside everything
/// the store itself owns (the attachment's id, the owning account, the attach time and the
/// status it starts in).
///
/// The id is NOT here: it is minted by
/// [`ComAmazonawsEc2Amazonec2State::start_network_interface_attachment`], the way every other
/// stored id is, so no caller can choose one.
#[derive(Debug, Clone, Default)]
pub struct NetworkInterfaceAttachmentSpec {
    /// "The ID of the instance" (`AttachNetworkInterfaceRequest$InstanceId`) the interface is
    /// attached to, which `NetworkInterfaceAttachment$InstanceId` reports.
    pub instance_id: String,
    /// "The index of the device for the network interface attachment"
    /// (`AttachNetworkInterfaceRequest$DeviceIndex`).
    pub device_index: i32,
    /// "The index of the network card. … The default is network card index 0"
    /// (`AttachNetworkInterfaceRequest$NetworkCardIndex`).
    pub network_card_index: i32,
    /// "Configures ENA Express for the network interface that this action attaches to the
    /// instance" (`AttachNetworkInterfaceRequest$EnaSrdSpecification`), in the shape the
    /// attachment reports it (`NetworkInterfaceAttachment$EnaSrdSpecification`). `None` for an
    /// attach that asked for none, which is what AWS reports for one.
    pub ena_srd_specification: Option<AttachmentEnaSrdSpecification>,
    /// "The number of ENA queues to be created with the instance"
    /// (`AttachNetworkInterfaceRequest$EnaQueueCount`), which the attachment reports back.
    pub ena_queue_count: Option<i32>,
}

/// The caller-chosen bytes a stored network interface retains: its tags, its description
/// and the addresses it holds. The same quantity
/// [`ComAmazonawsEc2Amazonec2State::insert_network_interface`] charged against the store's
/// budget, so [`ComAmazonawsEc2Amazonec2State::remove_network_interface`] gives back
/// exactly what was taken.
///
/// The primary address is charged ONCE though the record names it twice: it is both
/// `PrivateIpAddress` and an entry of `PrivateIpAddresses`, so the SET is what is counted
/// when the record carries one and the scalar only when it does not.
pub fn network_interface_payload_bytes(interface: &NetworkInterface) -> usize {
    let address_bytes: usize = match interface.private_ip_addresses.as_deref() {
        Some(addresses) => addresses
            .iter()
            .map(|address| address.private_ip_address.as_deref().map_or(0, str::len))
            .sum(),
        None => interface.private_ip_address.as_deref().map_or(0, str::len),
    };
    tag_list_bytes(interface.tag_set.as_deref().unwrap_or_default())
        .saturating_add(interface.description.as_deref().map_or(0, str::len))
        .saturating_add(address_bytes)
}

/// The caller-chosen bytes a network interface a writer ASKS for will retain, from the spec
/// rather than from the stored record: the same three members
/// [`network_interface_payload_bytes`] counts on the record it becomes — the tags, the
/// description and the private addresses — so a create can ask the store's byte budget for
/// exactly what [`ComAmazonawsEc2Amazonec2State::insert_network_interface`] will charge and
/// [`ComAmazonawsEc2Amazonec2State::remove_network_interface`] will give back.
pub fn network_interface_spec_bytes(spec: &NetworkInterfaceSpec) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.description.len())
        .saturating_add(spec.private_ip_addresses.iter().map(String::len).sum())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Remove the network interface `network_interface_id` and answer it as it was last
    /// reported, or `None` when the store holds no such interface.
    ///
    /// The exact reverse of [`Self::insert_network_interface`]: the record's bytes go back
    /// to the store's budget, every private IPv4 address it held goes back to the one in-use
    /// pool ([`Self::release_private_ip`]), the subnet it sat in gets those addresses
    /// back in its `AvailableIpAddressCount`, and the `ClientToken` entry that named this
    /// interface goes with it, bytes and all — so a create followed by a delete leaves the
    /// store exactly where it started, an address a deleted interface held can be handed
    /// out again, and a token whose interface is gone names nothing rather than naming a
    /// record the store no longer holds.
    pub fn remove_network_interface(
        &mut self,
        network_interface_id: &str,
    ) -> Option<NetworkInterface> {
        let interface = self.network_interfaces.remove(network_interface_id)?;
        // The reverse of the index write in [`Self::insert_network_interface`]: the entry
        // goes with the record, so no instance goes on holding an interface the store no
        // longer has.
        if let Some(instance_id) = interface
            .attachment
            .as_ref()
            .and_then(|attachment| attachment.instance_id.as_deref())
        {
            if let Some(held) = self.network_interfaces_by_instance.get_mut(instance_id) {
                held.remove(network_interface_id);
                if held.is_empty() {
                    self.network_interfaces_by_instance.remove(instance_id);
                }
            }
        }
        self.release_stored_bytes(
            network_interface_payload_bytes(&interface)
                .saturating_add(STORED_RECORD_OVERHEAD_BYTES),
        );
        // The token a create recorded for this interface, and the bytes it was charged:
        // a token outliving the interface it names would make a replay of that create
        // resolve to a record the store does not hold.
        let tokens: Vec<String> = self
            .network_interface_client_tokens
            .iter()
            .filter(|(_, named)| named.as_str() == network_interface_id)
            .map(|(token, _)| token.clone())
            .collect();
        for token in tokens {
            self.release_stored_bytes(token.len());
            self.network_interface_client_tokens.remove(&token);
        }
        let released: Vec<String> = match interface.private_ip_addresses.as_deref() {
            Some(addresses) => addresses
                .iter()
                .filter_map(|address| address.private_ip_address.clone())
                .collect(),
            None => interface.private_ip_address.clone().into_iter().collect(),
        };
        for address in &released {
            self.release_private_ip(address);
        }
        if let Some(subnet) = interface
            .subnet_id
            .as_deref()
            .and_then(|subnet_id| self.subnets.get_mut(subnet_id))
        {
            let given_back = i32::try_from(released.len()).unwrap_or(i32::MAX);
            subnet.available_ip_address_count = Some(
                subnet
                    .available_ip_address_count
                    .unwrap_or_default()
                    .saturating_add(given_back),
            );
        }
        // A permission is a grant over THIS interface, so it cannot outlive it: each one
        // goes with the interface, bytes and all, the way the client token above does.
        for network_interface_permission_id in
            self.network_interface_permissions_over(network_interface_id)
        {
            self.remove_network_interface_permission(&network_interface_permission_id);
        }
        // And the move the clock owed this interface's attachment, which an interface the
        // store no longer holds cannot be owed: nothing is left pointing at a record that is
        // gone.
        self.network_interface_attachment_move_at
            .remove(network_interface_id);
        Some(interface)
    }

    /// The attachment that still holds the network interface `network_interface_id`, or
    /// `None` when nothing holds it — either because the store has no such interface or
    /// because its attachment has been taken away.
    ///
    /// A READ ONLY: this is what "you must detach the network interface before you can
    /// delete it" (`DeleteNetworkInterface`) asks, so every operation that must refuse a
    /// held interface asks it the same way.
    pub fn network_interface_attachment(
        &self,
        network_interface_id: &str,
    ) -> Option<&NetworkInterfaceAttachment> {
        self.network_interfaces
            .get(network_interface_id)?
            .attachment
            .as_ref()
            .filter(|attachment| !attachment_is_detached(attachment))
    }

    /// Whether `attachment_id` is a network interface attachment id this service could have
    /// ISSUED, which is what an operation that resolves a `NetworkInterfaceAttachmentId`
    /// reads one as.
    ///
    /// [`Self::id_was_issued`] under this resource's own
    /// [`NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX`]: AWS was recorded refusing a hand-written
    /// long-form `eni-attach-…` id — a value inside the generic EC2 id form that it never
    /// issued — with [`INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED`] BEFORE any
    /// attachment was looked up, so this id kind is read as the ids the service issues rather
    /// than as anything that merely looks hexadecimal. An id it DID issue is looked up, and is
    /// answered [`invalid_attachment_id_not_found`] when the attachment it named is gone.
    pub fn network_interface_attachment_id_is_well_formed(&self, attachment_id: &str) -> bool {
        self.id_was_issued(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX, attachment_id)
    }

    /// The network interface the attachment `attachment_id` holds, or `None` when no interface
    /// of the account carries that attachment.
    ///
    /// A READ ONLY, and the one way an operation that names an ATTACHMENT finds the interface
    /// it is of: an attachment is a member of the interface record rather than a record of its
    /// own, so the id is resolved by asking the interfaces. An attachment that has been taken
    /// away holds nothing and so names no interface, exactly as
    /// [`Self::network_interface_attachment`] reads one.
    pub fn network_interface_of_attachment(&self, attachment_id: &str) -> Option<&str> {
        self.network_interfaces
            .iter()
            .find_map(|(network_interface_id, interface)| {
                let attachment = interface.attachment.as_ref()?;
                if attachment_is_detached(attachment) {
                    return None;
                }
                (attachment.attachment_id.as_deref() == Some(attachment_id))
                    .then_some(network_interface_id.as_str())
            })
    }

    /// Start taking down the attachment the interface `network_interface_id` holds, and answer
    /// whether there was one to take down.
    ///
    /// "Detaches a network interface from an instance" (`DetachNetworkInterface`): the
    /// attachment moves to the NON-TERMINAL [`ATTACHMENT_STATUS_DETACHING`] now, and the clock
    /// owes its completion one [`TRANSITION_DELAY`] later — the same span, on the same
    /// monotonic clock, as every other move this store owes — so a caller that reads the
    /// interface straight afterwards still sees the transient state. The instant is RECORDED
    /// rather than slept for ([`Self::network_interface_attachment_move_at`]) and
    /// [`Self::materialize_due_transitions`] applies it.
    ///
    /// That instant is owed by the call that STARTED the move: it is written only when the
    /// attachment is not already on its way down, so a second detach of one already detaching
    /// does not postpone its completion — overwriting would let a caller that polls with a
    /// detach push the move out for as long as it kept asking.
    pub fn start_network_interface_detachment(&mut self, network_interface_id: &str) -> bool {
        let Some(interface) = self.network_interfaces.get_mut(network_interface_id) else {
            return false;
        };
        let Some(attachment) = interface.attachment.as_mut() else {
            return false;
        };
        if attachment.status.as_deref() == Some(ATTACHMENT_STATUS_DETACHED) {
            return false;
        }
        let already_detaching =
            attachment.status.as_deref() == Some(ATTACHMENT_STATUS_DETACHING);
        attachment.status = Some(ATTACHMENT_STATUS_DETACHING.to_string());
        if !already_detaching {
            self.network_interface_attachment_move_at.insert(
                network_interface_id.to_string(),
                PendingStateMove::starting_now(ATTACHMENT_STATUS_DETACHED),
            );
        }
        true
    }

    /// Attach the interface `network_interface_id` to an instance, and answer the
    /// `AttachmentId` the attachment was given — or `None` when the store holds no such
    /// interface.
    ///
    /// "Attaches a network interface to an instance" (`AttachNetworkInterface`): the attachment
    /// is made in the NON-TERMINAL [`ATTACHMENT_STATUS_ATTACHING`] and the clock owes it
    /// [`ATTACHMENT_STATUS_ATTACHED`] one [`TRANSITION_DELAY`] later, on the same terms as the
    /// detach above, so a caller that reads the interface straight afterwards still sees the
    /// transient state. The interface is `in-use` from this call on: an interface something
    /// holds can no more claim to be `available` than the one
    /// [`Self::insert_network_interface`] stores with an attachment can.
    ///
    /// The id is minted HERE rather than taken from a caller, the way every other stored id of
    /// this store is, and the instance's own index of the interfaces it holds
    /// ([`Self::network_interfaces_by_instance`]) is written with the attachment — so the
    /// instance report and `DescribeNetworkInterfaces` cannot disagree about which interfaces
    /// an instance has.
    pub fn start_network_interface_attachment(
        &mut self,
        network_interface_id: &str,
        spec: NetworkInterfaceAttachmentSpec,
    ) -> Option<String> {
        if !self.network_interfaces.contains_key(network_interface_id) {
            return None;
        }
        let attachment_id = self.next_id(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX);
        let interface = self.network_interfaces.get_mut(network_interface_id)?;
        interface.attachment = Some(NetworkInterfaceAttachment {
            attach_time: Some(system_time_to_date_time(SystemTime::now())),
            attachment_id: Some(attachment_id.clone()),
            // "If you attach a network interface to an existing instance, the
            // DeleteOnTermination attribute is set to false by default" (the EC2 User Guide's
            // "Elastic network interfaces"): only the primary interface a LAUNCH creates goes
            // with its instance.
            delete_on_termination: Some(false),
            device_index: Some(spec.device_index),
            network_card_index: Some(spec.network_card_index),
            instance_id: Some(spec.instance_id.clone()),
            instance_owner_id: Some(::engine::emulator_account_id().to_string()),
            status: Some(ATTACHMENT_STATUS_ATTACHING.to_string()),
            ena_srd_specification: spec.ena_srd_specification,
            ena_queue_count: spec.ena_queue_count,
        });
        interface.status = Some(NETWORK_INTERFACE_STATUS_IN_USE.to_string());
        self.network_interfaces_by_instance
            .entry(spec.instance_id)
            .or_default()
            .insert(network_interface_id.to_string());
        self.owe_network_interface_attachment_completion(network_interface_id);
        Some(attachment_id)
    }

    /// Record that the clock owes the attachment of the interface `network_interface_id`
    /// its completion: [`ATTACHMENT_STATUS_ATTACHED`] one [`TRANSITION_DELAY`] from now.
    ///
    /// The ONE place an attachment's completion is owed, so an attachment a LAUNCH makes
    /// (`super::launched_primary_interfaces` and `super::launched_interfaces`) and one
    /// `AttachNetworkInterface` makes reach `attached` over the same span, and a caller that
    /// reads either straight afterwards still sees the `attaching` state it was answered
    /// with. The instant is RECORDED rather than slept for and
    /// [`Self::settle_due_network_interface_attachment_moves`] applies it.
    pub(super) fn owe_network_interface_attachment_completion(
        &mut self,
        network_interface_id: &str,
    ) {
        self.network_interface_attachment_move_at.insert(
            network_interface_id.to_string(),
            PendingStateMove::starting_now(ATTACHMENT_STATUS_ATTACHED),
        );
    }

    /// Apply every move the clock owes a network interface's attachment that has come due by
    /// `now`.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation may
    /// ask for it: each move was owed at its own instant whether or not anybody called, and
    /// applying it writes nothing a caller could not already have been shown.
    pub(super) fn settle_due_network_interface_attachment_moves(&mut self, now: Instant) {
        let due: Vec<(String, String)> = self
            .network_interface_attachment_move_at
            .iter()
            .filter_map(|(network_interface_id, pending)| {
                pending
                    .due_by(now)
                    .map(|to| (network_interface_id.clone(), to.to_string()))
            })
            .collect();
        for (network_interface_id, to) in due {
            self.network_interface_attachment_move_at
                .remove(&network_interface_id);
            // A finished DETACHMENT is the attachment being gone rather than a state it sits
            // in, so it is applied by taking the attachment off the interface; every other
            // move is the attachment's own status reaching its terminal value.
            if to == ATTACHMENT_STATUS_DETACHED {
                self.complete_network_interface_detachment(&network_interface_id);
                continue;
            }
            if let Some(attachment) = self
                .network_interfaces
                .get_mut(&network_interface_id)
                .and_then(|interface| interface.attachment.as_mut())
            {
                attachment.status = Some(to);
            }
        }
    }

    /// Take the attachment off the interface `network_interface_id`: the completed half of
    /// [`Self::start_network_interface_detachment`].
    ///
    /// A finished detachment is the attachment being GONE rather than a state it sits in — the
    /// interface holds nothing, so its status is [`NETWORK_INTERFACE_STATUS_AVAILABLE`] again
    /// and the instance that held it no longer has it
    /// ([`Self::network_interfaces_by_instance`], the index
    /// [`Self::remove_network_interface`] keeps the same way). The interface itself stays:
    /// detaching an interface does not delete it.
    pub(super) fn complete_network_interface_detachment(&mut self, network_interface_id: &str) {
        let Some(interface) = self.network_interfaces.get_mut(network_interface_id) else {
            return;
        };
        let held_by = interface
            .attachment
            .as_ref()
            .and_then(|attachment| attachment.instance_id.clone());
        interface.attachment = None;
        interface.status = Some(NETWORK_INTERFACE_STATUS_AVAILABLE.to_string());
        if let Some(instance_id) = held_by {
            if let Some(held) = self.network_interfaces_by_instance.get_mut(&instance_id) {
                held.remove(network_interface_id);
                if held.is_empty() {
                    self.network_interfaces_by_instance.remove(&instance_id);
                }
            }
        }
    }
}

/// One stored network interface as the INSTANCE that holds it reports it
/// (`Instance$NetworkInterfaces`, whose entries are `InstanceNetworkInterface`).
///
/// The same resource in the shape the instance report carries: the shared members are
/// copied straight across and nothing is derived a second time, so an interface cannot read
/// one way through `DescribeNetworkInterfaces` and another through `DescribeInstances`. The
/// members `InstanceNetworkInterface` has and the interface's own shape does not are the
/// attachment's, read off the attachment this projection was given — the one that holds it.
pub(super) fn instance_network_interface_view(
    interface: &NetworkInterface,
    attachment: &NetworkInterfaceAttachment,
) -> InstanceNetworkInterface {
    // The public address the interface holds, in the shape the INSTANCE report carries it
    // in: the same stored association, which has no member for the allocation or the
    // association id — those belong to the Elastic IP address record `DescribeAddresses`
    // reports — so one association cannot read one way here and another through
    // `DescribeNetworkInterfaces`.
    let association = |association: &NetworkInterfaceAssociation| InstanceNetworkInterfaceAssociation {
        carrier_ip: association.carrier_ip.clone(),
        customer_owned_ip: association.customer_owned_ip.clone(),
        ip_owner_id: association.ip_owner_id.clone(),
        public_dns_name: association.public_dns_name.clone(),
        public_ip: association.public_ip.clone(),
    };
    InstanceNetworkInterface {
        association: interface.association.as_ref().map(association),
        attachment: Some(InstanceNetworkInterfaceAttachment {
            attach_time: attachment.attach_time,
            attachment_id: attachment.attachment_id.clone(),
            delete_on_termination: attachment.delete_on_termination,
            device_index: attachment.device_index,
            status: attachment.status.clone(),
            network_card_index: attachment.network_card_index,
            // ENA Express is a setting of an attachment something asked for, which
            // nothing here does.
            ena_srd_specification: None,
            ena_queue_count: attachment.ena_queue_count,
        }),
        description: interface.description.clone(),
        groups: interface.groups.clone(),
        ipv6_addresses: interface.ipv6_addresses.as_ref().map(|addresses| {
            addresses
                .iter()
                .map(|address| InstanceIpv6Address {
                    ipv6_address: address.ipv6_address.clone(),
                    is_primary_ipv6: address.is_primary_ipv6,
                })
                .collect()
        }),
        mac_address: interface.mac_address.clone(),
        network_interface_id: interface.network_interface_id.clone(),
        owner_id: interface.owner_id.clone(),
        private_dns_name: interface.private_dns_name.clone(),
        private_ip_address: interface.private_ip_address.clone(),
        private_ip_addresses: interface.private_ip_addresses.as_ref().map(|addresses| {
            addresses
                .iter()
                .map(|address| InstancePrivateIpAddress {
                    association: address.association.as_ref().map(association),
                    primary: address.primary,
                    private_dns_name: address.private_dns_name.clone(),
                    private_ip_address: address.private_ip_address.clone(),
                })
                .collect()
        }),
        source_dest_check: interface.source_dest_check,
        status: interface.status.clone(),
        subnet_id: interface.subnet_id.clone(),
        vpc_id: interface.vpc_id.clone(),
        interface_type: interface.interface_type.clone(),
        ipv4_prefixes: interface.ipv4_prefixes.as_ref().map(|prefixes| {
            prefixes
                .iter()
                .map(|prefix| InstanceIpv4Prefix {
                    ipv4_prefix: prefix.ipv4_prefix.clone(),
                })
                .collect()
        }),
        ipv6_prefixes: interface.ipv6_prefixes.as_ref().map(|prefixes| {
            prefixes
                .iter()
                .map(|prefix| InstanceIpv6Prefix {
                    ipv6_prefix: prefix.ipv6_prefix.clone(),
                })
                .collect()
        }),
        connection_tracking_configuration: interface
            .connection_tracking_configuration
            .as_ref()
            .map(|tracking| ConnectionTrackingSpecificationResponse {
                tcp_established_timeout: tracking.tcp_established_timeout,
                udp_stream_timeout: tracking.udp_stream_timeout,
                udp_timeout: tracking.udp_timeout,
            }),
        operator: interface.operator.clone(),
    }
}

// ---------------------------------------------------------------------------
// Network interface permissions: what the account has granted another account
// over one of its network interfaces, and what reports it
// ---------------------------------------------------------------------------
//
// A permission is a resource of its own — "Describes a permission for a network
// interface" (`NetworkInterfacePermission`), minted with its own
// `NetworkInterfacePermissionId` and reported by `DescribeNetworkInterfacePermissions`.
// What the writer and the reporter share — the id mint, the state the clock owes, the
// capacity ceiling and the error codes a refusal carries — lives here, so the two cannot
// disagree about one permission.

/// The id prefix EC2 mints a network interface permission with, as
/// [`network_interface_permission_id_is_well_formed`] reads one (`eni-perm-…`, the form the
/// EC2 CLI and console report a `NetworkInterfacePermissionId` under), carried here without
/// the separator [`ComAmazonawsEc2Amazonec2State::next_id`] adds.
pub const NETWORK_INTERFACE_PERMISSION_ID_PREFIX: &str = "eni-perm";

/// A permission that has been recorded but is not in force yet: the
/// `NetworkInterfacePermissionStateCode` enum's `pending` member
/// (`smithy_query(query="shape",shape="NetworkInterfacePermissionStateCode")`), the
/// non-terminal state a permission sits in until [`TRANSITION_DELAY`] has passed.
pub const NETWORK_INTERFACE_PERMISSION_STATE_PENDING: &str = "pending";

/// A permission that is in force: that enum's `granted` member, the state the clock owes a
/// `pending` permission [`TRANSITION_DELAY`] after it was recorded.
pub const NETWORK_INTERFACE_PERMISSION_STATE_GRANTED: &str = "granted";

/// The largest number of network interface permissions this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_NETWORK_INTERFACES`], not an AWS quota: each
/// permission is a record in this process's memory, so the number a caller can create has to
/// be finite. [`ComAmazonawsEc2Amazonec2State::has_network_interface_permission_capacity`]
/// is the predicate a writer asks before inserting.
pub const MAX_NETWORK_INTERFACE_PERMISSIONS: usize = 1_000;

/// EC2's error code for a request that named a network interface permission id this account
/// holds no permission for, in the form EC2 names every such refusal with
/// (`InvalidNetworkInterfaceID.NotFound`, `InvalidCarrierGatewayID.NotFound`):
/// `Invalid<Kind>ID.NotFound`.
pub const INVALID_NETWORK_INTERFACE_PERMISSION_ID_NOT_FOUND: &str =
    "InvalidNetworkInterfacePermissionID.NotFound";

/// EC2's error code for a request whose network interface permission id is not an id of
/// that kind at all, answered BEFORE the id is looked up — the counterpart of
/// [`INVALID_NETWORK_INTERFACE_PERMISSION_ID_NOT_FOUND`], so a caller can tell "not an id of
/// this kind" from "no such permission" (the same pair
/// [`INVALID_NETWORK_INTERFACE_ID_MALFORMED`] and
/// [`INVALID_NETWORK_INTERFACE_ID_NOT_FOUND`] form for the interface itself).
pub const INVALID_NETWORK_INTERFACE_PERMISSION_ID_MALFORMED: &str =
    "InvalidNetworkInterfacePermissionID.Malformed";

/// Whether `id` is a well-formed network interface permission id:
/// [`NETWORK_INTERFACE_PERMISSION_ID_PREFIX`] followed by 8 or 17 lowercase hexadecimal
/// digits, the two EC2 resource id forms [`prefixed_id_is_well_formed`] reads.
pub fn network_interface_permission_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(NETWORK_INTERFACE_PERMISSION_ID_PREFIX, id)
}

/// Refuse a request that named something that is not a network interface permission id:
/// HTTP 400 `InvalidNetworkInterfacePermissionID.Malformed`, with the message EC2 builds
/// from the id the request named.
pub fn invalid_network_interface_permission_id_malformed<E>(
    network_interface_permission_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_PERMISSION_ID_MALFORMED,
        400,
        invalid_id_message(network_interface_permission_id),
    )
}

/// Refuse a request that named a network interface permission id this account holds no
/// permission for: HTTP 400 `InvalidNetworkInterfacePermissionID.NotFound`, with the message
/// EC2 builds from the id the request named — it spells the resource with the modelled
/// shape's own name (`NetworkInterfacePermission`), as the real service does.
pub fn invalid_network_interface_permission_id_not_found<E>(
    network_interface_permission_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_PERMISSION_ID_NOT_FOUND,
        400,
        format!(
            "The NetworkInterfacePermission ID '{network_interface_permission_id}' does not \
             exist"
        ),
    )
}

/// Refuse a grant to a party the service cannot confirm as an authorized grantee: HTTP 400
/// `OperationNotPermitted`, naming the account the request asked to grant to.
///
/// "Grants an Amazon Web Services-authorized account permission to attach the specified
/// network interface to an instance in their account" (`CreateNetworkInterfacePermission`):
/// the standing the grantee needs is given to an ACCOUNT by AWS, out of band, and no
/// operation of this API gives it — so a grantee the service cannot confirm holds it is
/// refused the operation and nothing is recorded, whichever interface and permission type
/// the request named.
///
/// This emulator serves one account ([`::engine::emulator_account_id`]), so that account is
/// the only grantee whose standing it can confirm, exactly as the one account bounds which
/// peer a transit gateway peering request may name. The sentence's one variable part is the
/// account the REQUEST named, bounded by [`echoed_resource_id`] like every other echo of a
/// caller-chosen value.
pub fn network_interface_permission_grantee_not_authorized<E>(aws_account_id: &str) -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "You are not authorized to grant permissions to account {}",
        echoed_resource_id(aws_account_id)
    ))
}

/// Refuse a grant over a network interface the account does not manage: HTTP 400
/// `OperationNotPermitted`, naming the interface the request asked to share.
///
/// "Indicates whether the network interface is being managed by Amazon Web Services"
/// (`NetworkInterface$RequesterManaged`): such an interface was created for another service's
/// own resource, so it is not the account's to share — the same reading
/// [`attachment_is_not_the_accounts_to_manage`] takes of its attachment, told of the
/// INTERFACE here because that is what a permission is over.
pub fn network_interface_cannot_be_shared<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::operation_not_permitted(format!(
        "The network interface '{network_interface_id}' is managed by Amazon Web Services and \
         cannot be shared"
    ))
}

/// What one network interface permission is granted with, beyond everything the service
/// itself derives ([`ComAmazonawsEc2Amazonec2State::insert_network_interface_permission`]
/// mints the id and the state the clock then moves).
///
/// The grantee is EITHER an account — "the Amazon Web Services account ID"
/// (`NetworkInterfacePermission$AwsAccountId`) — OR a service
/// (`NetworkInterfacePermission$AwsService`), which is why both are optional here: the
/// record carries the one the writer named.
#[derive(Debug, Clone, Default)]
pub struct NetworkInterfacePermissionSpec {
    /// "The ID of the network interface" the permission is over
    /// (`NetworkInterfacePermission$NetworkInterfaceId`).
    pub network_interface_id: String,
    /// "The Amazon Web Services account ID" the permission is granted to
    /// (`NetworkInterfacePermission$AwsAccountId`).
    pub aws_account_id: Option<String>,
    /// "The Amazon Web Services service" the permission is granted to
    /// (`NetworkInterfacePermission$AwsService`).
    pub aws_service: Option<String>,
    /// "The type of permission" (`NetworkInterfacePermission$Permission`), an
    /// `InterfacePermissionType` value
    /// ([`crate::ops::types::INTERFACE_PERMISSION_TYPE_VALUES`]).
    pub permission: String,
}

/// The caller-chosen bytes a stored network interface permission retains: the grantee the
/// writer named. Everything else on the record — the id, the interface id, the permission
/// type and the state — is derived by this module or bounded by the model, so it is
/// fixed-size.
///
/// The ONE definition of that charge, the way [`network_interface_payload_bytes`] is for an
/// interface: a writer asks it before the write,
/// [`ComAmazonawsEc2Amazonec2State::insert_network_interface_permission`] charges exactly it
/// and [`ComAmazonawsEc2Amazonec2State::remove_network_interface_permission`] gives exactly
/// it back.
pub fn network_interface_permission_payload_bytes(
    aws_account_id: Option<&str>,
    aws_service: Option<&str>,
) -> usize {
    aws_account_id
        .map_or(0, str::len)
        .saturating_add(aws_service.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another network interface permission fits inside the store's ceiling
    /// ([`MAX_NETWORK_INTERFACE_PERMISSIONS`]).
    ///
    /// The same contract as [`Self::has_network_interface_capacity`]: a writer asks this
    /// first and refuses when it answers `false`.
    pub fn has_network_interface_permission_capacity(&self) -> bool {
        self.network_interface_permissions.len() < MAX_NETWORK_INTERFACE_PERMISSIONS
    }

    /// Record a permission over a network interface and answer the
    /// `NetworkInterfacePermissionId` it was given, or `None` when the store is full.
    ///
    /// The ONE writer seam, the way [`Self::insert_network_interface`] is for an interface:
    /// everything the service owns is derived here rather than taken from a caller — the id
    /// and the `pending` state the clock then moves to `granted` one [`TRANSITION_DELAY`]
    /// later — so two writers cannot report one permission differently. The caller-chosen
    /// bytes the record retains ([`network_interface_permission_payload_bytes`]) are charged
    /// against the store's byte budget here, so a writer cannot store them uncharged, and
    /// both ceilings are enforced here rather than trusted to the caller.
    pub fn insert_network_interface_permission(
        &mut self,
        spec: NetworkInterfacePermissionSpec,
    ) -> Option<String> {
        if !self.has_network_interface_permission_capacity()
            || self.network_interface_permissions.len() >= MAX_STORED_RECORDS_PER_STORE
        {
            return None;
        }
        let payload_bytes = network_interface_permission_payload_bytes(
            spec.aws_account_id.as_deref(),
            spec.aws_service.as_deref(),
        );
        if self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let network_interface_permission_id = self.next_id(NETWORK_INTERFACE_PERMISSION_ID_PREFIX);
        let permission = NetworkInterfacePermission {
            network_interface_permission_id: Some(network_interface_permission_id.clone()),
            network_interface_id: Some(spec.network_interface_id),
            aws_account_id: spec.aws_account_id,
            aws_service: spec.aws_service,
            permission: Some(spec.permission),
            // "Information about the state of the permission"
            // (`NetworkInterfacePermission$PermissionState`): a permission starts out
            // `pending` and the clock owes it `granted`, so a caller that reads straight
            // after the write still sees the transient state.
            permission_state: Some(NetworkInterfacePermissionState {
                state: Some(NETWORK_INTERFACE_PERMISSION_STATE_PENDING.to_string()),
                status_message: None,
            }),
        };
        self.add_stored_bytes(payload_bytes.saturating_add(STORED_RECORD_OVERHEAD_BYTES));
        self.network_interface_permission_granted_at.insert(
            network_interface_permission_id.clone(),
            due_instant(TRANSITION_DELAY),
        );
        self.network_interface_permissions
            .insert(network_interface_permission_id.clone(), permission);
        Some(network_interface_permission_id)
    }

    /// Remove the permission `network_interface_permission_id` and answer it as it was last
    /// reported, or `None` when the store holds no such permission.
    ///
    /// The exact reverse of [`Self::insert_network_interface_permission`]: the record's
    /// bytes go back to the store's budget and the move the clock still owed it goes with
    /// it, so a grant followed by its removal leaves the store exactly where it started.
    pub fn remove_network_interface_permission(
        &mut self,
        network_interface_permission_id: &str,
    ) -> Option<NetworkInterfacePermission> {
        let permission = self
            .network_interface_permissions
            .remove(network_interface_permission_id)?;
        self.release_stored_bytes(
            network_interface_permission_payload_bytes(
                permission.aws_account_id.as_deref(),
                permission.aws_service.as_deref(),
            )
            .saturating_add(STORED_RECORD_OVERHEAD_BYTES),
        );
        self.network_interface_permission_granted_at
            .remove(network_interface_permission_id);
        Some(permission)
    }

    /// The permission the store already holds over `network_interface_id` for the account
    /// `aws_account_id` and the permission type `permission`, if it holds one.
    ///
    /// A READ ONLY, beside the writer seam above because it answers the question the writer
    /// has to ask first: "you can grant permission to a single Amazon Web Services account
    /// only, and only one account at a time" (`CreateNetworkInterfacePermission`), so one
    /// grantee's grant of one permission type over one interface is ONE record — asking for
    /// it again names the grant that is already there rather than a second one beside it.
    pub fn network_interface_permission_granted_to(
        &self,
        network_interface_id: &str,
        aws_account_id: &str,
        permission: &str,
    ) -> Option<&NetworkInterfacePermission> {
        self.network_interface_permissions.values().find(|held| {
            held.network_interface_id.as_deref() == Some(network_interface_id)
                && held.aws_account_id.as_deref() == Some(aws_account_id)
                && held.permission.as_deref() == Some(permission)
        })
    }

    /// The ids of every permission the store holds over the network interface
    /// `network_interface_id`, in the store's own id order.
    ///
    /// A READ ONLY: what a writer asks to find the grants that are already over one
    /// interface, and what [`Self::remove_network_interface`] asks to take them with it.
    pub fn network_interface_permissions_over(&self, network_interface_id: &str) -> Vec<String> {
        self.network_interface_permissions
            .iter()
            .filter(|(_, permission)| {
                permission.network_interface_id.as_deref() == Some(network_interface_id)
            })
            .map(|(network_interface_permission_id, _)| network_interface_permission_id.clone())
            .collect()
    }
}

/// Whether `attachment` holds nothing any more: its `Status` has reached
/// [`ATTACHMENT_STATUS_DETACHED`]. Every other status — including an attachment that
/// carries none at all — still holds the interface.
pub(super) fn attachment_is_detached(attachment: &NetworkInterfaceAttachment) -> bool {
    attachment.status.as_deref() == Some(ATTACHMENT_STATUS_DETACHED)
}

/// Refuse an association the resource already has with something else, which the request
/// did not allow to be replaced: HTTP 400 `Resource.AlreadyAssociated`.
pub fn resource_already_associated<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(RESOURCE_ALREADY_ASSOCIATED, 400, message)
}

/// Refuse an allocation that would take the store past [`MAX_STORED_ADDRESSES`]: HTTP 400
/// `AddressLimitExceeded`.
pub fn address_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(ADDRESS_LIMIT_EXCEEDED, 400, message)
}

/// The network border group a Region's own addresses are advertised from.
///
/// "A unique set of Availability Zones, Local Zones, or Wavelength Zones from which Amazon
/// Web Services advertises IP addresses" (`AllocateAddressRequest$NetworkBorderGroup`):
/// every Availability Zone of a Region shares one border group named for the Region, and a
/// Local or Wavelength Zone has its own. This emulator's zones are all
/// `availability-zone`s ([`availability_zones_in_region`]), so a Region has exactly this
/// one border group.
pub fn network_border_group_of_region(region: &str) -> String {
    region.to_string()
}

// ---------------------------------------------------------------------------
// Elastic IP address transfers
// ---------------------------------------------------------------------------

/// A transfer the source account has started that has neither been accepted nor expired:
/// the `AddressTransferStatus` enum's `pending` member
/// (`smithy_query(query="shape",shape="AddressTransferStatus")`).
pub const ADDRESS_TRANSFER_STATUS_PENDING: &str = "pending";

/// A transfer offer the source account has withdrawn: the `AddressTransferStatus` enum's
/// `disabled` member, which is what "Disables Elastic IP address transfer"
/// (`DisableAddressTransfer`) leaves behind.
pub const ADDRESS_TRANSFER_STATUS_DISABLED: &str = "disabled";

/// A transfer the transfer account has taken: the `AddressTransferStatus` enum's
/// `accepted` member.
pub const ADDRESS_TRANSFER_STATUS_ACCEPTED: &str = "accepted";

/// How long a transfer offer stands before it expires on its own.
///
/// DOCUMENTED, not chosen: "When the source account starts the transfer, the transfer
/// account has seven days to accept the Elastic IP address transfer … After seven days,
/// the transfer expires and ownership of the Elastic IP address returns to the source
/// account" (`DescribeAddressTransfers`). It is also the instant
/// `AddressTransfer$TransferOfferExpirationTimestamp` reports.
pub const ADDRESS_TRANSFER_OFFER_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long an accepted transfer stays visible to the source account.
///
/// DOCUMENTED, not chosen: "Accepted transfers are visible to the source account for 14
/// days after the transfers have been accepted" (`DescribeAddressTransfers`).
pub const ADDRESS_TRANSFER_ACCEPTED_VISIBILITY: Duration =
    Duration::from_secs(14 * 24 * 60 * 60);

/// One Elastic IP address transfer the source account can see: the modelled
/// [`AddressTransfer`] every operation that reports one answers with, plus when the record
/// stops being one of that account's transfers — the one fact that shape carries no member
/// for.
///
/// Built in exactly one place ([`ComAmazonawsEc2Amazonec2State::offer_address_transfer`]),
/// so the instant beside the record and the timestamps on it cannot disagree.
#[derive(Debug, Clone)]
pub struct AddressTransferRecord {
    /// The transfer as every operation that reports one answers with it.
    pub transfer: AddressTransfer,
    /// When this record stops being one of the source account's transfers: the end of the
    /// offer window for a transfer that was never accepted, and
    /// [`ADDRESS_TRANSFER_ACCEPTED_VISIBILITY`] after the acceptance for one that was. On
    /// the wall clock, because both spans are documented in days and are the same instants
    /// the record's own timestamps report.
    pub visible_until: SystemTime,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Start the transfer of the Elastic IP address `allocation_id` to
    /// `transfer_account_id`, as of `at`, and answer the `AllocationId` the record is keyed
    /// by.
    ///
    /// "Enables Elastic IP address transfer" (`EnableAddressTransfer`): the offer is
    /// `pending` and stands for [`ADDRESS_TRANSFER_OFFER_WINDOW`], which is the instant
    /// `TransferOfferExpirationTimestamp` reports and the instant the record stops being
    /// visible. An address has at most one transfer at a time, so starting one on an
    /// address that already carries a record REPLACES it: the call says what the account
    /// wants the address's transfer to be, and the model declares no error for an address
    /// that already has one.
    ///
    /// No byte charge is made for the record: it is keyed by an id of [`Self::addresses`],
    /// so there can be no more of them than there are stored addresses, and the only
    /// caller-supplied value it retains is an account id, which is
    /// [`AWS_ACCOUNT_ID_DIGITS`] digits.
    pub fn offer_address_transfer(
        &mut self,
        allocation_id: &str,
        public_ip: Option<&str>,
        transfer_account_id: &str,
        at: SystemTime,
    ) -> String {
        let expires_at = at + ADDRESS_TRANSFER_OFFER_WINDOW;
        let record = AddressTransferRecord {
            transfer: AddressTransfer {
                public_ip: public_ip.map(str::to_string),
                allocation_id: Some(allocation_id.to_string()),
                transfer_account_id: Some(transfer_account_id.to_string()),
                transfer_offer_expiration_timestamp: Some(system_time_to_date_time(expires_at)),
                // Nothing has accepted the offer: "The timestamp when the Elastic IP
                // address transfer was accepted"
                // (`AddressTransfer$TransferOfferAcceptedTimestamp`) names no instant yet.
                transfer_offer_accepted_timestamp: None,
                address_transfer_status: Some(ADDRESS_TRANSFER_STATUS_PENDING.to_string()),
            },
            visible_until: expires_at,
        };
        self.address_transfers
            .insert(allocation_id.to_string(), record);
        allocation_id.to_string()
    }

    /// Withdraw the transfer offer on `allocation_id`, answering the offer as it was
    /// withdrawn — or `None` when there was no `pending` one to withdraw.
    ///
    /// "Disables Elastic IP address transfer" (`DisableAddressTransfer`): only a `pending`
    /// offer can be withdrawn — an accepted transfer has already moved the address, and a
    /// disabled one is already withdrawn — so this answers `None` for every other state and
    /// the caller refuses the request with [`address_transfer_not_available`].
    ///
    /// The withdrawn offer is REMOVED from the store: "describes an Elastic IP address
    /// transfer" (`DescribeAddressTransfers`) reports the transfers of the account, and an
    /// offer the account has taken back is no longer one of them — it stands for nothing, has
    /// no account left to accept it and no expiry left to run out. So the record goes with the
    /// call that withdraws it, and the answer this hands back, which carries the
    /// [`ADDRESS_TRANSFER_STATUS_DISABLED`] word the call reports, is the LAST view of it.
    /// An address whose offer was withdrawn can be offered again, which
    /// [`Self::offer_address_transfer`] records as the fresh offer it is.
    pub fn withdraw_address_transfer(&mut self, allocation_id: &str) -> Option<AddressTransfer> {
        let record = self.address_transfers.get(allocation_id)?;
        if record.transfer.address_transfer_status.as_deref()
            != Some(ADDRESS_TRANSFER_STATUS_PENDING)
        {
            return None;
        }
        let mut withdrawn = self.address_transfers.remove(allocation_id)?.transfer;
        withdrawn.address_transfer_status = Some(ADDRESS_TRANSFER_STATUS_DISABLED.to_string());
        Some(withdrawn)
    }

    /// The allocation id of the stored transfer that has `public_ip` ON OFFER, or `None`
    /// when no transfer of this store does.
    ///
    /// The transfer surface's one lookup by ADDRESS: "The Elastic IP address you are
    /// accepting for transfer" (`AcceptAddressTransferRequest$Address`) names the address
    /// rather than the allocation id a record is keyed by. Only a `pending` offer is
    /// answered — a `disabled` one has been withdrawn and an `accepted` one has already been
    /// taken, so neither is on offer — which is also what decides there is nothing to
    /// accept. A scan of the records themselves, like [`Self::allocation_id_for_public_ip`]
    /// and bounded the same way: a stored address carries at most one transfer.
    pub fn address_transfer_on_offer(&self, public_ip: &str) -> Option<String> {
        self.address_transfers
            .iter()
            .find(|(_, record)| {
                record.transfer.public_ip.as_deref() == Some(public_ip)
                    && record.transfer.address_transfer_status.as_deref()
                        == Some(ADDRESS_TRANSFER_STATUS_PENDING)
            })
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// Take the transfer offer on `allocation_id` as of `at`, answering whether there was a
    /// `pending` one to take.
    ///
    /// "Accepts an Elastic IP address transfer" (`AcceptAddressTransfer`): the record moves
    /// to `accepted`, `TransferOfferAcceptedTimestamp` names the instant it was taken, and
    /// the record stays visible for [`ADDRESS_TRANSFER_ACCEPTED_VISIBILITY`] from that
    /// instant instead of until the offer would have run out — "Accepted transfers are
    /// visible to the source account for 14 days after the transfers have been accepted"
    /// (`DescribeAddressTransfers`). Only an offer that is still `pending` can be taken, so
    /// this answers `false` for every other state and the caller refuses the request with
    /// [`address_transfer_not_available`], exactly as [`Self::withdraw_address_transfer`]
    /// does. Storing no caller-chosen bytes, it makes no byte charge.
    pub fn accept_address_transfer(&mut self, allocation_id: &str, at: SystemTime) -> bool {
        let Some(record) = self.address_transfers.get_mut(allocation_id) else {
            return false;
        };
        if record.transfer.address_transfer_status.as_deref()
            != Some(ADDRESS_TRANSFER_STATUS_PENDING)
        {
            return false;
        }
        record.transfer.address_transfer_status =
            Some(ADDRESS_TRANSFER_STATUS_ACCEPTED.to_string());
        record.transfer.transfer_offer_accepted_timestamp = Some(system_time_to_date_time(at));
        record.visible_until = at + ADDRESS_TRANSFER_ACCEPTED_VISIBILITY;
        true
    }

    /// Drop every transfer record the clock has taken past
    /// [`AddressTransferRecord::visible_until`].
    ///
    /// Reached through [`Self::materialize_due_transitions`], so a reporting operation may
    /// apply it: "After seven days, the transfer expires and ownership of the Elastic IP
    /// address returns to the source account", and an accepted transfer is visible "for 14
    /// days after the transfers have been accepted" (`DescribeAddressTransfers`) — each was
    /// already owed at `now` whether or not anybody asked.
    pub fn expire_due_address_transfers(&mut self, now: SystemTime) {
        self.address_transfers
            .retain(|_allocation_id, record| now < record.visible_until);
    }
}

/// Refuse a request the Elastic IP address transfer surface has no transfer to act on:
/// HTTP 400 `InvalidAddress.NotFound`.
///
/// Worded by the one writer of that code ([`invalid_address_not_found`]), so the transfer
/// surface and every other operation that cannot resolve an address answer one sentence.
pub fn address_transfer_not_available<E>(address: &str) -> Scaffolded<E> {
    invalid_address_not_found(address)
}

// ---------------------------------------------------------------------------
// Transit gateways
// ---------------------------------------------------------------------------

/// A transit gateway AWS is still creating: the `TransitGatewayState` enum's `pending`
/// member, which is what `CreateTransitGateway` answers with until [`TRANSITION_DELAY`]
/// has passed ("After the transit gateway enters the available state, you can attach your
/// VPCs and VPN connections to the transit gateway" — `CreateTransitGateway`).
pub const TRANSIT_GATEWAY_STATE_PENDING: &str = "pending";

/// A transit gateway that has finished coming up: the `TransitGatewayState` enum's
/// `available` member.
pub const TRANSIT_GATEWAY_STATE_AVAILABLE: &str = "available";

/// A transit gateway whose modification is being carried out: the `TransitGatewayState`
/// enum's `modifying` member, which is what `ModifyTransitGateway` answers with until
/// [`TRANSITION_DELAY`] has passed and the clock owes the gateway
/// [`TRANSIT_GATEWAY_STATE_AVAILABLE`] again.
pub const TRANSIT_GATEWAY_STATE_MODIFYING: &str = "modifying";

/// A transit gateway whose deletion has been asked for: the `TransitGatewayState` enum's
/// `deleting` member (`smithy_query(query="shape",shape="TransitGatewayState")`), which is what
/// `DeleteTransitGateway` answers with until the clock's own
/// [`ComAmazonawsEc2Amazonec2State::transit_gateway_delete_at`] instant removes the gateway.
pub const TRANSIT_GATEWAY_STATE_DELETING: &str = "deleting";

/// VPC Encryption Control being turned ON for a transit gateway: the
/// `EncryptionStateValue` enum's `enabling` member, the transient state a modification that
/// asked for encryption support answers with.
pub const ENCRYPTION_STATE_ENABLING: &str = "enabling";

/// VPC Encryption Control in force: that enum's `enabled` member.
pub const ENCRYPTION_STATE_ENABLED: &str = "enabled";

/// VPC Encryption Control being turned OFF: that enum's `disabling` member.
pub const ENCRYPTION_STATE_DISABLING: &str = "disabling";

/// VPC Encryption Control off: that enum's `disabled` member.
pub const ENCRYPTION_STATE_DISABLED: &str = "disabled";

/// A transit gateway route table still coming up with its gateway: the
/// `TransitGatewayRouteTableState` enum's `pending` member.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING: &str = "pending";

/// A transit gateway route table that is usable: the `TransitGatewayRouteTableState`
/// enum's `available` member.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE: &str = "available";

/// A transit gateway option turned ON: the `enable` member every one of the option enums
/// (`DnsSupportValue`, `VpnEcmpSupportValue`, …) is one of two values of.
pub const OPTION_ENABLE: &str = "enable";

/// A transit gateway option turned OFF: those enums' other member.
pub const OPTION_DISABLE: &str = "disable";

/// The `ResourceType` a `CreateTransitGateway` tag specification may name: the transit
/// gateway it creates.
pub const RESOURCE_TYPE_TRANSIT_GATEWAY: &str = "transit-gateway";

/// The `ResourceType` a `CreateTransitGatewayRouteTable` tag specification may name: the
/// transit gateway route table it creates ("The tags to apply to the transit gateway route
/// table", `CreateTransitGatewayRouteTableRequest$TagSpecifications`), which is the
/// `ResourceType` enum's `transit-gateway-route-table` member.
pub const RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE: &str = "transit-gateway-route-table";

/// The ASN a transit gateway gets when the request names none: "The default is `64512`"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const DEFAULT_AMAZON_SIDE_ASN: i64 = 64_512;

/// The 16-bit private ASN range a transit gateway's Amazon-side ASN may come from: "The
/// range is 64512 to 65534 for 16-bit ASNs"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const AMAZON_SIDE_ASN_16_BIT: ::std::ops::RangeInclusive<i64> = 64_512..=65_534;

/// The 32-bit private ASN range: "and 4200000000 to 4294967294 for 32-bit ASNs"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const AMAZON_SIDE_ASN_32_BIT: ::std::ops::RangeInclusive<i64> = 4_200_000_000..=4_294_967_294;

/// The Amazon-side ASN a create asks for, or [`DEFAULT_AMAZON_SIDE_ASN`] when it asks for
/// none — refusing anything outside the two published private ranges rather than defaulting
/// it, so a typo never reads as "the default".
///
/// The ONE place that rule lives: both operations that let a caller choose an Amazon-side
/// ASN publish the same two ranges and the same default — "If you're using a 16-bit ASN, it
/// must be in the 64512 to 65534 range. If you're using a 32-bit ASN, it must be in the
/// 4200000000 to 4294967294 range. Default: 64512" (`CreateVpnGatewayRequest$AmazonSideAsn`,
/// and `TransitGatewayRequestOptions$AmazonSideAsn` in the same words) — so neither can
/// accept an ASN the other refuses.
pub fn private_amazon_side_asn<E>(requested: Option<i64>) -> Result<i64, Scaffolded<E>> {
    match requested {
        None => Ok(DEFAULT_AMAZON_SIDE_ASN),
        Some(asn)
            if AMAZON_SIDE_ASN_16_BIT.contains(&asn) || AMAZON_SIDE_ASN_32_BIT.contains(&asn) =>
        {
            Ok(asn)
        }
        Some(asn) => Err(invalid_parameter_value(format!(
            "Value ({asn}) for parameter amazonSideAsn is invalid. \
             The range is {min_16} to {max_16} for 16-bit ASNs and \
             {min_32} to {max_32} for 32-bit ASNs.",
            min_16 = AMAZON_SIDE_ASN_16_BIT.start(),
            max_16 = AMAZON_SIDE_ASN_16_BIT.end(),
            min_32 = AMAZON_SIDE_ASN_32_BIT.start(),
            max_32 = AMAZON_SIDE_ASN_32_BIT.end(),
        ))),
    }
}

/// The longest IPv4 prefix a transit gateway CIDR block may have: "Must be a size /24 CIDR
/// block or larger for IPv4" (`TransitGatewayRequestOptions$TransitGatewayCidrBlocks`) —
/// larger block, so a prefix no longer than this.
pub const MAX_TRANSIT_GATEWAY_IPV4_PREFIX: u8 = 24;

/// The longest IPv6 prefix a transit gateway CIDR block may have: "or a size /64 CIDR block
/// or larger for IPv6" (same member).
pub const MAX_TRANSIT_GATEWAY_IPV6_PREFIX: u8 = 64;

/// The largest number of transit gateways this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each gateway is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_transit_gateway_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`TRANSIT_GATEWAY_LIMIT_EXCEEDED`].
pub const MAX_TRANSIT_GATEWAYS: usize = 1_000;

/// EC2's error code for a create that would take the account past its transit gateway
/// quota.
pub const TRANSIT_GATEWAY_LIMIT_EXCEEDED: &str = "TransitGatewayLimitExceeded";

/// EC2's error code for a request naming a transit gateway id this account holds no
/// transit gateway for.
pub const INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND: &str = "InvalidTransitGatewayID.NotFound";

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`] needs that it does
/// not derive itself: what the REQUEST said, and the Region it was signed for.
#[derive(Debug, Clone)]
pub struct TransitGatewaySpec {
    /// "A description of the transit gateway" (`CreateTransitGatewayRequest$Description`),
    /// absent when the request gave none.
    pub description: Option<String>,
    /// The Region the request was signed for, which the gateway's ARN names.
    pub region: String,
    /// When the gateway was created, as the creation time every record of it reports.
    pub created_at: SystemTime,
    /// The resolved options ([`transit_gateway_options`]); the two default-route-table ids
    /// are filled in by the insert, which is what creates that table.
    pub options: TransitGatewayOptions,
    /// The tags of a `transit-gateway` tag specification.
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored transit gateway holds: its tags, its description and
/// the CIDR blocks the request asked for. Everything else on the record — the ids, the
/// ARN, the account, the state, the option flags — is derived by this module or bounded by
/// the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateTransitGateway` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn transit_gateway_payload_bytes(transit_gateway: &TransitGateway) -> usize {
    let tags = tag_list_bytes(transit_gateway.tags.as_deref().unwrap_or_default());
    let description = transit_gateway.description.as_deref().map_or(0, str::len);
    let cidr_blocks: usize = transit_gateway
        .options
        .as_ref()
        .and_then(|options| options.transit_gateway_cidr_blocks.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|block| block.len())
        .sum();
    tags.saturating_add(description).saturating_add(cidr_blocks)
}

/// A requested transit gateway CIDR block in canonical form, of whichever family it names.
///
/// "One or more IPv4 or IPv6 CIDR blocks for the transit gateway. Must be a size /24 CIDR
/// block or larger for IPv4, or a size /64 CIDR block or larger for IPv6"
/// (`TransitGatewayRequestOptions$TransitGatewayCidrBlocks`): the family is decided by the
/// value itself, and each family's published minimum block size is the bound. The block is
/// read by the same two readers the VPC path uses, so this crate keeps ONE rule for what a
/// CIDR block is and only the published block size differs.
pub fn canonical_transit_gateway_cidr(value: &str) -> Result<String, CidrProblem> {
    if value.contains(':') {
        let canonical = canonical_ipv6_cidr(value)?;
        let (_, prefix_length) = canonical.split_once('/').ok_or(CidrProblem::Malformed)?;
        let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
        if prefix_length > MAX_TRANSIT_GATEWAY_IPV6_PREFIX {
            return Err(CidrProblem::PrefixLength);
        }
        return Ok(canonical);
    }
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if prefix_length > MAX_TRANSIT_GATEWAY_IPV4_PREFIX {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// The transit gateway CIDR blocks a request asked for, each in the canonical form
/// [`canonical_transit_gateway_cidr`] reads it into — refusing anything that is not a CIDR
/// block at all, or that is smaller than the published minimum for its family, and naming
/// the member the request carried it in.
///
/// The ONE reader of that member for every operation that carries one
/// (`TransitGatewayRequestOptions$TransitGatewayCidrBlocks`,
/// `ModifyTransitGatewayOptions$AddTransitGatewayCidrBlocks` and its `Remove…` twin), so a
/// create and a modification cannot disagree about what a block is or about how one outside
/// the published sizes is refused.
pub fn canonical_transit_gateway_cidr_blocks<E>(
    requested: Option<&[String]>,
    member: &str,
) -> Result<Vec<String>, Scaffolded<E>> {
    let mut blocks: Vec<String> = Vec::new();
    for requested_block in requested.unwrap_or_default() {
        match canonical_transit_gateway_cidr(requested_block) {
            Ok(canonical) => blocks.push(canonical),
            Err(CidrProblem::Malformed) => {
                return Err(invalid_parameter_value(format!(
                    "Value ({requested_block}) for parameter {member} is \
                     invalid. This is not a valid CIDR block."
                )))
            }
            Err(CidrProblem::PrefixLength) => {
                return Err(invalid_parameter_value(format!(
                    "The CIDR '{requested_block}' is invalid. Must be a size \
                     /{ipv4} CIDR block or larger for IPv4, or a size /{ipv6} CIDR block \
                     or larger for IPv6.",
                    ipv4 = MAX_TRANSIT_GATEWAY_IPV4_PREFIX,
                    ipv6 = MAX_TRANSIT_GATEWAY_IPV6_PREFIX,
                )))
            }
        }
    }
    Ok(blocks)
}

/// The options one transit gateway is created with: what the request asked for, each
/// member it left out at the default its own documentation publishes.
///
/// This is the ONE place those defaults live, so `CreateTransitGateway` and every later
/// reader of a gateway's options cannot disagree about a gateway nobody modified. The
/// defaults, from `TransitGatewayRequestOptions`'s own member docs:
///
/// * `AutoAcceptSharedAttachments` — "Disabled by default".
/// * `DefaultRouteTableAssociation` — "Enabled by default".
/// * `DefaultRouteTablePropagation` — "Enabled by default".
/// * `VpnEcmpSupport` — "Enabled by default".
/// * `DnsSupport` — "Enabled by default".
/// * `SecurityGroupReferencingSupport` — "This option is disabled by default".
/// * `MulticastSupport` — multicast is a gateway the caller asks for, so `disable`.
/// * `AmazonSideAsn` — [`DEFAULT_AMAZON_SIDE_ASN`].
///
/// A value outside the published range or an unknown enum member is REFUSED rather than
/// defaulted, so a typo never reads as "the default". The two default-route-table ids are
/// left absent: only [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`], which
/// creates that table, can name it.
pub fn transit_gateway_options<E>(
    requested: Option<&TransitGatewayRequestOptions>,
) -> Result<TransitGatewayOptions, Scaffolded<E>> {
    let asn = private_amazon_side_asn(requested.and_then(|options| options.amazon_side_asn))?;
    let cidr_blocks = canonical_transit_gateway_cidr_blocks(
        requested.and_then(|options| options.transit_gateway_cidr_blocks.as_deref()),
        "transitGatewayCidrBlocks",
    )?;
    Ok(TransitGatewayOptions {
        amazon_side_asn: Some(asn),
        // "One or more IPv4 or IPv6 CIDR blocks for the transit gateway": a gateway the
        // request asked for none for carries no member at all rather than an empty list.
        transit_gateway_cidr_blocks: (!cidr_blocks.is_empty()).then_some(cidr_blocks),
        auto_accept_shared_attachments: Some(transit_gateway_option(
            requested.and_then(|options| options.auto_accept_shared_attachments.as_deref()),
            "autoAcceptSharedAttachments",
            OPTION_DISABLE,
        )?),
        default_route_table_association: Some(transit_gateway_option(
            requested.and_then(|options| options.default_route_table_association.as_deref()),
            "defaultRouteTableAssociation",
            OPTION_ENABLE,
        )?),
        // Filled in by the insert, which is what creates the default route table.
        association_default_route_table_id: None,
        default_route_table_propagation: Some(transit_gateway_option(
            requested.and_then(|options| options.default_route_table_propagation.as_deref()),
            "defaultRouteTablePropagation",
            OPTION_ENABLE,
        )?),
        propagation_default_route_table_id: None,
        vpn_ecmp_support: Some(transit_gateway_option(
            requested.and_then(|options| options.vpn_ecmp_support.as_deref()),
            "vpnEcmpSupport",
            OPTION_ENABLE,
        )?),
        dns_support: Some(transit_gateway_option(
            requested.and_then(|options| options.dns_support.as_deref()),
            "dnsSupport",
            OPTION_ENABLE,
        )?),
        security_group_referencing_support: Some(transit_gateway_option(
            requested.and_then(|options| options.security_group_referencing_support.as_deref()),
            "securityGroupReferencingSupport",
            OPTION_DISABLE,
        )?),
        multicast_support: Some(transit_gateway_option(
            requested.and_then(|options| options.multicast_support.as_deref()),
            "multicastSupport",
            OPTION_DISABLE,
        )?),
        // "Defines if the Transit Gateway supports VPC Encryption Control"
        // (`TransitGatewayOptions$EncryptionSupport`): nothing in this request asks for
        // VPC Encryption Control, and no operation that would turn it on is implemented,
        // so no encryption state is reported rather than an invented one.
        encryption_support: None,
    })
}

/// One `enable`/`disable` transit gateway option: the requested value if the request
/// carried one, otherwise `default`.
///
/// Every one of these members targets an enum whose only members are [`OPTION_ENABLE`] and
/// [`OPTION_DISABLE`] (`smithy_query(query="shape",shape="DnsSupportValue")`), which the
/// boundary validator already refuses anything outside — so this is the backstop that
/// keeps a value no enum has out of the store whatever reaches the handler, and it never
/// lets a typo read as the default.
pub(super) fn transit_gateway_option<E>(
    requested: Option<&str>,
    member: &str,
    default: &str,
) -> Result<String, Scaffolded<E>> {
    match requested {
        None => Ok(default.to_string()),
        Some(value) => transit_gateway_option_value(value, member),
    }
}

/// One `enable`/`disable` transit gateway option VALUE a request carried, refused when it
/// is neither.
///
/// The ONE reader of such a value, for the create that defaults an omitted member
/// ([`transit_gateway_option`]) and for the modification that leaves an omitted member as it
/// was alike, so the two cannot disagree about which values an option admits.
pub fn transit_gateway_option_value<E>(value: &str, member: &str) -> Result<String, Scaffolded<E>> {
    if value == OPTION_ENABLE || value == OPTION_DISABLE {
        return Ok(value.to_string());
    }
    Err(invalid_parameter_value(format!(
        "Value ({value}) for parameter {member} is invalid. \
         Valid values are {OPTION_ENABLE} and {OPTION_DISABLE}."
    )))
}

/// What `ModifyTransitGateway` asks of one stored transit gateway: the members the REQUEST
/// carried, each absent when the request left it out — "the modified options are applied to
/// new transit gateway attachments only" (`ModifyTransitGateway`), so a member the request
/// does not carry leaves the stored value as it was rather than returning to a default.
///
/// Every value here has already been read by the readers a create holds the same members to
/// ([`private_amazon_side_asn`], [`canonical_transit_gateway_cidr_blocks`],
/// [`transit_gateway_option_value`]), so what reaches the store is a value the create path
/// would have accepted too.
#[derive(Debug, Clone, Default)]
pub struct TransitGatewayModification {
    /// "The ID of the transit gateway" (`ModifyTransitGatewayRequest$TransitGatewayId`).
    pub transit_gateway_id: String,
    /// "The description for the transit gateway"
    /// (`ModifyTransitGatewayRequest$Description`).
    pub description: Option<String>,
    /// "Adds IPv4 or IPv6 CIDR blocks for the transit gateway"
    /// (`ModifyTransitGatewayOptions$AddTransitGatewayCidrBlocks`), canonicalised.
    pub add_transit_gateway_cidr_blocks: Vec<String>,
    /// "Removes CIDR blocks for the transit gateway"
    /// (`ModifyTransitGatewayOptions$RemoveTransitGatewayCidrBlocks`), canonicalised.
    pub remove_transit_gateway_cidr_blocks: Vec<String>,
    /// The Amazon-side ASN the request asked for.
    pub amazon_side_asn: Option<i64>,
    /// "Enable or disable automatic acceptance of attachment requests."
    pub auto_accept_shared_attachments: Option<String>,
    /// "Enable or disable automatic association with the default association route table."
    pub default_route_table_association: Option<String>,
    /// "The ID of the default association route table."
    pub association_default_route_table_id: Option<String>,
    /// Whether "resource attachments automatically propagate routes to the default
    /// propagation route table".
    pub default_route_table_propagation: Option<String>,
    /// "The ID of the default propagation route table."
    pub propagation_default_route_table_id: Option<String>,
    /// "Enable or disable Equal Cost Multipath Protocol support."
    pub vpn_ecmp_support: Option<String>,
    /// "Enable or disable DNS support."
    pub dns_support: Option<String>,
    /// Whether a security group may be referenced "across VPCs attached to a transit
    /// gateway".
    pub security_group_referencing_support: Option<String>,
    /// "Enable or disable encryption support for VPC Encryption Control"
    /// (`ModifyTransitGatewayOptions$EncryptionSupport`), as the `enable`/`disable` value
    /// the request asked for rather than as the state it leads to.
    pub encryption_support: Option<String>,
}

impl TransitGatewayModification {
    /// `transit_gateway` as this modification leaves it: every member the request carried
    /// replaces the stored value, and every member it did not carry keeps it.
    ///
    /// The ONE definition of what a modification does to a gateway, so the pre-check of the
    /// store's byte budget and
    /// [`ComAmazonawsEc2Amazonec2State::modify_transit_gateway`]'s write cannot disagree
    /// about the record the call leaves behind. The members only the STORE can decide are
    /// deliberately not set here: the `modifying` state, the default route table a newly
    /// enabled default role needs, and the id each default role ends up naming.
    ///
    /// The CIDR blocks are the stored set with the removals taken out and the additions put
    /// in, in that order — "Removes CIDR blocks for the transit gateway"
    /// (`RemoveTransitGatewayCidrBlocks`) and "Adds IPv4 or IPv6 CIDR blocks for the transit
    /// gateway" (`AddTransitGatewayCidrBlocks`) describe the set the gateway is LEFT with,
    /// so a block named twice is held once and a block named in both lists stays on the
    /// gateway.
    pub fn applied_to(&self, transit_gateway: &TransitGateway) -> TransitGateway {
        /// Replace `stored` with the value the request carried, when it carried one.
        fn replace(stored: &mut Option<String>, requested: &Option<String>) {
            if let Some(value) = requested {
                *stored = Some(value.clone());
            }
        }

        let mut applied = transit_gateway.clone();
        replace(&mut applied.description, &self.description);
        let mut options = applied.options.unwrap_or_default();
        if let Some(asn) = self.amazon_side_asn {
            options.amazon_side_asn = Some(asn);
        }
        replace(
            &mut options.auto_accept_shared_attachments,
            &self.auto_accept_shared_attachments,
        );
        replace(
            &mut options.default_route_table_association,
            &self.default_route_table_association,
        );
        replace(
            &mut options.association_default_route_table_id,
            &self.association_default_route_table_id,
        );
        replace(
            &mut options.default_route_table_propagation,
            &self.default_route_table_propagation,
        );
        replace(
            &mut options.propagation_default_route_table_id,
            &self.propagation_default_route_table_id,
        );
        replace(&mut options.vpn_ecmp_support, &self.vpn_ecmp_support);
        replace(&mut options.dns_support, &self.dns_support);
        replace(
            &mut options.security_group_referencing_support,
            &self.security_group_referencing_support,
        );

        let mut blocks: Vec<String> = options.transit_gateway_cidr_blocks.unwrap_or_default();
        // Set membership rather than a scan per block, so the work this costs grows with
        // the number of blocks the request names rather than with its square.
        let removals: BTreeSet<&str> = self
            .remove_transit_gateway_cidr_blocks
            .iter()
            .map(String::as_str)
            .collect();
        blocks.retain(|block| !removals.contains(block.as_str()));
        let mut held: BTreeSet<String> = blocks.iter().cloned().collect();
        for block in &self.add_transit_gateway_cidr_blocks {
            if held.insert(block.clone()) {
                blocks.push(block.clone());
            }
        }
        // A gateway that holds no block carries no member at all rather than an empty list,
        // exactly as one created without any does.
        options.transit_gateway_cidr_blocks = (!blocks.is_empty()).then_some(blocks);

        // "Enable or disable encryption support for VPC Encryption Control"
        // (`EncryptionSupport`): the request asks for the END state, and the gateway reports
        // the state it is IN (`EncryptionStateValue`), so an enable starts `enabling` and a
        // disable `disabling` — and asking for the state the gateway is already in or
        // already reaching changes nothing.
        if let Some(requested) = self.encryption_support.as_deref() {
            let reached = options
                .encryption_support
                .as_ref()
                .and_then(|support| support.encryption_state.as_deref());
            let next = match (requested, reached) {
                (
                    OPTION_ENABLE,
                    None | Some(ENCRYPTION_STATE_DISABLED | ENCRYPTION_STATE_DISABLING),
                ) => Some(ENCRYPTION_STATE_ENABLING),
                (
                    OPTION_DISABLE,
                    Some(ENCRYPTION_STATE_ENABLED | ENCRYPTION_STATE_ENABLING),
                ) => Some(ENCRYPTION_STATE_DISABLING),
                _ => None,
            };
            if let Some(next) = next {
                options.encryption_support = Some(EncryptionSupport {
                    encryption_state: Some(next.to_string()),
                    // "A message describing the encryption state"
                    // (`EncryptionSupport$StateMessage`): nothing has gone wrong with this
                    // transition, so there is nothing to describe.
                    state_message: None,
                });
            }
        }

        applied.options = Some(options);
        applied
    }
}

/// Whether `options` — the options a transit gateway is being left with — would have the
/// store MINT the gateway's default transit gateway route table: a default role is enabled
/// and the gateway holds no route table in EITHER role.
///
/// "If `defaultRouteTablePropagation` is set to `enable`, Amazon Web Services Transit
/// Gateway will create the default transit gateway route table"
/// (`ModifyTransitGatewayOptions$DefaultRouteTablePropagation`), and "Either
/// `defaultRouteTableAssociation` or `defaultRouteTablePropagation` must be set to `enable`
/// for Amazon Web Services Transit Gateway to create the default transit gateway route
/// table" (`TransitGatewayOptions$DefaultRouteTableAssociation`) — so ONE table serves both
/// roles, and a gateway that already holds one needs no further table whichever role holds
/// it. The predicate is public because the handler asks it about the store's route table
/// ceiling BEFORE the write that
/// [`ComAmazonawsEc2Amazonec2State::modify_transit_gateway`] then makes, and both ask it
/// rather than each deciding for itself.
pub fn needs_default_transit_gateway_route_table(options: Option<&TransitGatewayOptions>) -> bool {
    let Some(options) = options else {
        return false;
    };
    let associates = options.default_route_table_association.as_deref() == Some(OPTION_ENABLE);
    let propagates = options.default_route_table_propagation.as_deref() == Some(OPTION_ENABLE);
    (associates || propagates)
        && options.association_default_route_table_id.is_none()
        && options.propagation_default_route_table_id.is_none()
}

/// Refuse a request that named a transit gateway id no transit gateway of the account has:
/// HTTP 400 `InvalidTransitGatewayID.NotFound`.
pub fn invalid_transit_gateway_id_not_found<E>(transit_gateway_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        400,
        format!("Transit Gateway {transit_gateway_id} was deleted or does not exist."),
    )
}

/// Refuse a create that would take the store past [`MAX_TRANSIT_GATEWAYS`]: HTTP 400
/// `TransitGatewayLimitExceeded`.
pub fn transit_gateway_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        TRANSIT_GATEWAY_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of transit gateways has been reached ({MAX_TRANSIT_GATEWAYS})"
        ),
    )
}

// ---------------------------------------------------------------------------
// Transit gateway route tables, and the route propagations into them
// ---------------------------------------------------------------------------

/// The id prefix every transit gateway route table is minted with, the one
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`] gives the default route table
/// it creates.
///
/// A route table ANNOUNCEMENT id extends this prefix
/// ([`TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX`]), so the prefix alone cannot
/// separate the two kinds; it is the hex suffix [`prefixed_id_is_well_formed`] requires that
/// does, exactly as [`vpc_endpoint_id_is_well_formed`] separates the three `vpce-` kinds.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX: &str = "tgw-rtb";

/// `InvalidRouteTableID.NotFound` (HTTP 400): the request named a route table id this
/// account holds no transit gateway route table for. EC2 answers the route table's own code
/// here rather than a transit-gateway-specific one.
pub const INVALID_ROUTE_TABLE_ID_NOT_FOUND: &str = "InvalidRouteTableID.NotFound";

/// `InvalidRouteTableId.Malformed` (HTTP 400): the request named something that is not a
/// transit gateway route table id at all, so there is nothing to look up.
pub const INVALID_ROUTE_TABLE_ID_MALFORMED: &str = "InvalidRouteTableId.Malformed";

/// Refuse a request that named a transit gateway route table id no route table of the
/// account has: HTTP 400 [`INVALID_ROUTE_TABLE_ID_NOT_FOUND`].
///
/// Worded like [`invalid_transit_gateway_id_not_found`], the refusal the gateway itself
/// answers, so one family of resources reads one way.
pub fn invalid_transit_gateway_route_table_id_not_found<E>(route_table_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        400,
        format!("Transit Gateway Route Table {route_table_id} was deleted or does not exist."),
    )
}

/// Refuse a request that named something that is not a transit gateway route table id at
/// all: HTTP 400 [`INVALID_ROUTE_TABLE_ID_MALFORMED`], BEFORE the id is looked up, so "not
/// an id of this kind" is never reported as "no such route table".
///
/// The ONE place this resource's malformed refusal is worded, for every operation that
/// resolves a `TransitGatewayRouteTableId`: like [`invalid_transit_gateway_route_table_id_not_found`]
/// beside it, the message NAMES the resource and the id the request gave rather than using
/// the resource-agnostic wording of [`require_well_formed_id`] — which is the wording EC2
/// itself answers a malformed id of this resource with.
pub fn require_well_formed_transit_gateway_route_table_id<E>(
    route_table_id: &str,
) -> Result<(), Scaffolded<E>> {
    if prefixed_id_is_well_formed(TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX, route_table_id) {
        return Ok(());
    }
    Err(Scaffolded::service_error(
        INVALID_ROUTE_TABLE_ID_MALFORMED,
        400,
        format!("Invalid Transit Gateway Route Table id {route_table_id}."),
    ))
}

/// A propagation whose enable is being carried out: the `TransitGatewayPropagationState`
/// enum's `enabling` member ([`crate::ops::types::TRANSIT_GATEWAY_PROPAGATION_STATE_VALUES`]).
pub const PROPAGATION_STATE_ENABLING: &str = "enabling";

/// A propagation in force: that enum's `enabled` member.
pub const PROPAGATION_STATE_ENABLED: &str = "enabled";

/// A propagation whose disable is being carried out: that enum's `disabling` member.
pub const PROPAGATION_STATE_DISABLING: &str = "disabling";

/// What the clock owes one stored route propagation next.
///
/// The `disabled` member of `TransitGatewayPropagationState` is deliberately not a move:
/// once the disable has been carried out the pair does not propagate at all, which is not a
/// propagation of that route table — the same reading as [`FastLaunchMove::Discontinued`],
/// where the state catalog has no word for "not configured" either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationMove {
    /// `enabling` -> `enabled`: the attachment's routes are in the table.
    Enabled,
    /// `disabling` -> reported no longer: the propagation is gone.
    Removed,
}

/// The move the clock owes one route propagation: a [`ClockTransition`] to the next
/// [`PropagationMove`].
pub type PropagationTransition = ClockTransition<PropagationMove>;

/// One attachment's propagation of routes into one transit gateway route table.
///
/// Flattened rather than holding a modelled shape, because the two shapes that report a
/// propagation carry different member sets — `TransitGatewayPropagation` names the route
/// table, `TransitGatewayRouteTablePropagation` does not, being reported under it — so
/// [`Self::view`] and [`Self::route_table_view`] build each from the one stored record and
/// the two reports cannot disagree.
#[derive(Debug, Clone)]
pub struct TransitGatewayPropagationRecord {
    /// The propagation route table the routes go into.
    pub transit_gateway_route_table_id: String,
    /// The attachment whose routes propagate.
    pub transit_gateway_attachment_id: String,
    /// The resource that attachment attaches, which a report of the propagation names as
    /// its `ResourceId`; absent for an attachment that carries no resource id.
    pub resource_id: Option<String>,
    /// That resource's `TransitGatewayAttachmentResourceType`.
    pub resource_type: String,
    /// The route table announcement the propagation was asked for through, when it was.
    pub transit_gateway_route_table_announcement_id: Option<String>,
    /// The propagation's state, a member of `TransitGatewayPropagationState`.
    pub state: String,
    /// The move the clock owes this propagation, while one is owed.
    pub pending: Option<PropagationTransition>,
}

impl TransitGatewayPropagationRecord {
    /// The propagation as `EnableTransitGatewayRouteTablePropagation` and
    /// `DisableTransitGatewayRouteTablePropagation` answer with it.
    pub fn view(&self) -> TransitGatewayPropagation {
        TransitGatewayPropagation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            transit_gateway_route_table_id: Some(self.transit_gateway_route_table_id.clone()),
            state: Some(self.state.clone()),
            transit_gateway_route_table_announcement_id: self
                .transit_gateway_route_table_announcement_id
                .clone(),
        }
    }

    /// The propagation as `GetTransitGatewayRouteTablePropagations` reports it: the same
    /// record without the route table id, which that request named.
    pub fn route_table_view(&self) -> TransitGatewayRouteTablePropagation {
        TransitGatewayRouteTablePropagation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            state: Some(self.state.clone()),
            transit_gateway_route_table_announcement_id: self
                .transit_gateway_route_table_announcement_id
                .clone(),
        }
    }

    /// Put the propagation into `transient` now and owe it `to` [`TRANSITION_DELAY`] later.
    ///
    /// The span is never zero, so a caller acting straight after the enabling or disabling
    /// call still sees the transient state and the refusals that go with it.
    pub fn begin_move(&mut self, transient: &str, to: PropagationMove) {
        self.state = transient.to_string();
        self.pending = Some(PropagationTransition::starting_now(to));
    }
}

/// What a propagation request asks of one (route table, attachment) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationDecision {
    /// "Enables the specified attachment to propagate routes to the specified propagation
    /// route table" (`EnableTransitGatewayRouteTablePropagation`).
    Enable,
    /// "Disables the specified resource attachment from propagating routes to the specified
    /// propagation route table" (`DisableTransitGatewayRouteTablePropagation`).
    Disable,
}

/// The attachment, the route table announcement and the propagation route table one
/// propagation request names: the members
/// `Enable`/`DisableTransitGatewayRouteTablePropagationRequest` share.
#[derive(Debug, Clone, Copy)]
pub struct PropagationRequest<'a> {
    /// "The ID of the propagation route table" (`$TransitGatewayRouteTableId`), the one
    /// required member.
    pub route_table_id: &'a str,
    /// "The ID of the attachment" (`$TransitGatewayAttachmentId`), when the request names
    /// one.
    pub attachment_id: Option<&'a str>,
    /// "The ID of the route table announcement" (`$TransitGatewayRouteTableAnnouncementId`),
    /// when the request names one.
    pub announcement_id: Option<&'a str>,
}

/// Act on the route propagation `request` names, and answer it as it now stands.
///
/// The ONE place the rule the enabling and the disabling operation share is written, so
/// they cannot disagree about one pair:
///
/// * the propagation route table must be one the account holds, or there is nothing to
///   propagate into ([`invalid_transit_gateway_route_table_id_not_found`]);
/// * the attachment whose routes propagate is the one the request names, or — for a request
///   that names a route table announcement instead — the peering attachment that
///   announcement was made for, and a request that names neither has not said what to
///   propagate;
/// * that attachment must be one the account holds
///   ([`INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND`]);
/// * an enable needs a pair that does not propagate yet and a disable needs one that does,
///   so the other way round each is refused with `IncorrectState` and NOTHING is written;
/// * the pair then moves into the TRANSIENT state of the move asked for, with the clock
///   owing it the end of it ([`PropagationMove`]).
///
/// The CALLER refuses a malformed id first, before its own `DryRun` arm, so nothing here
/// depends on the request's form.
pub fn decide_transit_gateway_route_table_propagation<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    request: PropagationRequest<'_>,
    decision: PropagationDecision,
) -> Result<TransitGatewayPropagation, Scaffolded<E>> {
    // The clock first: a propagation whose move has fallen due is already enabled, or gone,
    // so whether this call is admissible is asked of the state the pair is ACTUALLY in
    // rather than of the one it was left in.
    store.materialize_due_transitions();
    let route_table_id = request.route_table_id;
    if !store
        .transit_gateway_route_tables
        .contains_key(route_table_id)
    {
        return Err(invalid_transit_gateway_route_table_id_not_found(
            route_table_id,
        ));
    }

    // "The ID of the route table announcement": an announcement is a propagation of the
    // peering attachment it was made for, so the announcement names the attachment when the
    // request does not — and when the request names both they must be the same attachment,
    // rather than one silently winning over the other.
    let announced: Option<String> = match request.announcement_id {
        None => None,
        Some(announcement_id) => store
            .transit_gateway_route_table_announcements
            .get(announcement_id)
            .ok_or_else(|| {
                id_not_found(
                    INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
                    "transit gateway route table announcement ID",
                    announcement_id,
                )
            })?
            .view
            .peering_attachment_id
            .clone(),
    };
    let attachment_id = match (request.attachment_id, announced.as_deref()) {
        (Some(named), Some(announced)) if named != announced => {
            return Err(Scaffolded::invalid_parameter_combination(format!(
                "The attachment '{named}' is not the attachment the route table \
                 announcement was made for ('{announced}')"
            )))
        }
        (Some(named), _) => named.to_string(),
        (None, Some(announced)) => announced.to_string(),
        (None, None) => {
            return Err(Scaffolded::missing_parameter(
                "The request must name the attachment whose routes propagate, either as \
                 transitGatewayAttachmentId or through a route table announcement",
            ))
        }
    };

    let attachment = store
        .transit_gateway_attachments
        .get(&attachment_id)
        .ok_or_else(|| invalid_transit_gateway_attachment_id_not_found(&attachment_id))?;
    let resource_id = attachment.view.resource_id().map(str::to_string);
    let resource_type = attachment.view.resource_type();
    let key = (route_table_id.to_string(), attachment_id.clone());

    match decision {
        PropagationDecision::Enable => {
            // A pair that already propagates, or is still finishing a disable, has no
            // enable to perform: the request is refused and nothing is written, which is
            // how the first call's write is observable to a later one.
            if let Some(existing) = store.transit_gateway_propagations.get(&key) {
                return Err(incorrect_state(format!(
                    "The attachment '{attachment_id}' already propagates routes to the \
                     transit gateway route table '{route_table_id}' in state '{state}'",
                    state = existing.state,
                )));
            }
            if !store.has_transit_gateway_propagation_capacity() {
                return Err(resource_limit_exceeded(format!(
                    "This emulator holds {} route propagations, which is as many as it \
                     stores",
                    store.transit_gateway_propagations.len()
                )));
            }
            if store.would_exceed_stored_bytes(STORED_RECORD_OVERHEAD_BYTES) {
                return Err(resource_limit_exceeded(format!(
                    "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                     caller-supplied data, so it cannot store another route propagation",
                    store.stored_bytes()
                )));
            }
            let record = TransitGatewayPropagationRecord {
                transit_gateway_route_table_id: route_table_id.to_string(),
                transit_gateway_attachment_id: attachment_id,
                resource_id,
                resource_type: resource_type.to_string(),
                transit_gateway_route_table_announcement_id: request
                    .announcement_id
                    .map(str::to_string),
                // The TRANSIENT state of an enable, with the clock owing it `enabled`: the
                // span is never zero, so a caller acting straight after this call still
                // sees the pair enabling.
                state: PROPAGATION_STATE_ENABLING.to_string(),
                pending: Some(PropagationTransition::starting_now(
                    PropagationMove::Enabled,
                )),
            };
            let view = record.view();
            store.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
            store.transit_gateway_propagations.insert(key, record);
            Ok(view)
        }
        PropagationDecision::Disable => {
            let record = store
                .transit_gateway_propagations
                .get_mut(&key)
                .filter(|record| record.state != PROPAGATION_STATE_DISABLING)
                .ok_or_else(|| {
                    incorrect_state(format!(
                        "The attachment '{attachment_id}' does not propagate routes to the \
                         transit gateway route table '{route_table_id}'"
                    ))
                })?;
            record.begin_move(PROPAGATION_STATE_DISABLING, PropagationMove::Removed);
            Ok(record.view())
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route propagation fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_propagation_capacity(&self) -> bool {
        self.transit_gateway_propagations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The route propagations of one transit gateway route table, in attachment-id order —
    /// the stable order `GetTransitGatewayRouteTablePropagations` reports and pages over.
    ///
    /// The store is keyed by (route table, attachment), so this is a RANGE over the keys of
    /// one route table rather than a scan of every propagation stored: the cost is
    /// proportional to the propagations of the table asked about.
    ///
    /// A read, so it writes nothing: the caller applies the clock-owed moves
    /// ([`Self::materialize_due_transitions`]) before asking, exactly as every describe of
    /// this module does.
    pub fn transit_gateway_route_table_propagations<'a>(
        &'a self,
        route_table_id: &'a str,
    ) -> impl Iterator<Item = &'a TransitGatewayPropagationRecord> {
        self.transit_gateway_propagations
            .range((route_table_id.to_string(), String::new())..)
            .take_while(move |((table, _), _)| table == route_table_id)
            .map(|(_, record)| record)
    }
}

// ---------------------------------------------------------------------------
// The attachments associated with a transit gateway route table
// ---------------------------------------------------------------------------

/// One attachment's association with one transit gateway route table.
///
/// Flattened rather than holding a modelled shape, because the shapes that report an
/// association carry different member sets — `TransitGatewayAssociation` names the route
/// table, `TransitGatewayRouteTableAssociation` does not, being reported under the table it
/// was asked about — so [`Self::view`] and [`Self::route_table_view`] build each from the one
/// stored record and the reports cannot disagree about one association. The same arrangement
/// as [`TransitGatewayPropagationRecord`] above, whose two reports differ the same way.
#[derive(Debug, Clone)]
pub struct TransitGatewayRouteTableAssociationRecord {
    /// The route table the attachment is associated with.
    pub transit_gateway_route_table_id: String,
    /// The associated attachment.
    pub transit_gateway_attachment_id: String,
    /// The resource that attachment attaches, which a report of the association names as its
    /// `ResourceId`; absent for an attachment that carries no resource id.
    pub resource_id: Option<String>,
    /// That resource's `TransitGatewayAttachmentResourceType`.
    pub resource_type: String,
    /// The association's state, a member of `TransitGatewayAssociationState`.
    pub state: String,
    /// The move the clock owes this association, while one is owed.
    pub pending: Option<PendingStateMove>,
}

impl TransitGatewayRouteTableAssociationRecord {
    /// The association as `AssociateTransitGatewayRouteTable` answers with it, naming the
    /// route table it is an association with.
    pub fn view(&self) -> TransitGatewayAssociation {
        TransitGatewayAssociation {
            transit_gateway_route_table_id: Some(self.transit_gateway_route_table_id.clone()),
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            state: Some(self.state.clone()),
        }
    }

    /// The association as `GetTransitGatewayRouteTableAssociations` reports it, under the
    /// route table the request asked about — which is why that shape names no route table.
    pub fn route_table_view(&self) -> TransitGatewayRouteTableAssociation {
        TransitGatewayRouteTableAssociation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            state: Some(self.state.clone()),
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route table association fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_propagation_capacity`].
    pub fn has_transit_gateway_association_capacity(&self) -> bool {
        self.transit_gateway_associations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The associations of one transit gateway route table, in attachment-id order — the
    /// stable order `GetTransitGatewayRouteTableAssociations` reports and pages over.
    ///
    /// A RANGE over the keys of one route table rather than a scan of every association
    /// stored, exactly as [`Self::transit_gateway_route_table_propagations`] is: the cost is
    /// proportional to the associations of the table asked about.
    ///
    /// A read, so it writes nothing: the caller applies the clock-owed moves
    /// ([`Self::materialize_due_transitions`]) before asking, as every report of this module
    /// does.
    pub fn transit_gateway_route_table_associations<'a>(
        &'a self,
        route_table_id: &'a str,
    ) -> impl Iterator<Item = &'a TransitGatewayRouteTableAssociationRecord> {
        self.transit_gateway_associations
            .range((route_table_id.to_string(), String::new())..)
            .take_while(move |((table, _), _)| table == route_table_id)
            .map(|(_, record)| record)
    }

    /// The route table association `transit_gateway_attachment_id` already has, if any.
    ///
    /// "You can associate only one route table with an attachment"
    /// (`AssociateTransitGatewayRouteTable`), so this answers at most one record; it is a
    /// scan of the associations the account holds, which is bounded by
    /// [`MAX_STORED_RECORDS_PER_STORE`] like every other store here — the same shape as
    /// [`Self::policy_table_of_attachment`], the policy table's own answer to this question.
    pub fn transit_gateway_route_table_association_of_attachment(
        &self,
        transit_gateway_attachment_id: &str,
    ) -> Option<&TransitGatewayRouteTableAssociationRecord> {
        self.transit_gateway_associations
            .values()
            .find(|record| record.transit_gateway_attachment_id == transit_gateway_attachment_id)
    }
}

/// Associate the transit gateway attachment `transit_gateway_attachment_id` with the transit
/// gateway route table `transit_gateway_route_table_id`, and answer the association as it now
/// stands.
///
/// The rule `AssociateTransitGatewayRouteTable` follows, in [`crate::state`] so the reports of
/// what it wrote cannot disagree with it:
///
/// * the route table must be one the account holds, or there is nothing to associate with
///   ([`invalid_transit_gateway_route_table_id_not_found`]);
/// * the attachment must be one the account holds
///   ([`invalid_transit_gateway_attachment_id_not_found`]);
/// * "you can associate only one route table with an attachment", so an attachment that
///   already has an association is refused with EC2's own `Resource.AlreadyAssociated` and
///   NOTHING is written, whether the table it already belongs to is this one or another;
/// * the new association starts in the TRANSIENT `associating` state with the clock owing it
///   `associated` — the span is never zero, so a caller acting straight after this call still
///   sees `associating`.
///
/// The association's `ResourceId` and `ResourceType` are read off the STORED attachment ("the
/// ID of the resource" / "the resource type", `TransitGatewayAssociation$ResourceId` /
/// `$ResourceType`), never off the request, which names neither. The CALLER refuses a
/// malformed id of either kind first, so nothing here depends on the request's form.
pub fn associate_transit_gateway_route_table<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    transit_gateway_route_table_id: &str,
    transit_gateway_attachment_id: &str,
) -> Result<TransitGatewayAssociation, Scaffolded<E>> {
    // The clock first: an association whose move has fallen due is already `associated`, so
    // what this call is asked against is the state the store is ACTUALLY in.
    store.materialize_due_transitions();

    // The route table the request names, before the attachment: the operation's members name
    // the table first, and a request naming two unknown resources is told about the first one.
    if !store
        .transit_gateway_route_tables
        .contains_key(transit_gateway_route_table_id)
    {
        return Err(invalid_transit_gateway_route_table_id_not_found(
            transit_gateway_route_table_id,
        ));
    }

    // "The ID of the attachment" (`AssociateTransitGatewayRouteTableRequest$
    // TransitGatewayAttachmentId`): an id naming no attachment of the account is refused with
    // the attachment's own NotFound code rather than silently associating nothing.
    let attachment = store
        .transit_gateway_attachments
        .get(transit_gateway_attachment_id)
        .ok_or_else(|| {
            invalid_transit_gateway_attachment_id_not_found(transit_gateway_attachment_id)
        })?;
    let resource_id = attachment.view.resource_id().map(str::to_string);
    let resource_type = attachment.view.resource_type().to_string();

    // One attachment, at most one route table — so a second association is refused and
    // nothing is written, which is how the first association stays observable to a later
    // caller.
    if let Some(held) = store.transit_gateway_route_table_association_of_attachment(
        transit_gateway_attachment_id,
    ) {
        return Err(resource_already_associated(format!(
            "The transit gateway attachment '{transit_gateway_attachment_id}' is already \
             associated with the transit gateway route table '{holder}'",
            holder = held.transit_gateway_route_table_id,
        )));
    }

    if !store.has_transit_gateway_association_capacity() {
        return Err(resource_limit_exceeded(format!(
            "This emulator holds {} transit gateway route table associations, which is as \
             many as it stores",
            store.transit_gateway_associations.len()
        )));
    }
    // An association retains no caller-chosen text at all — every string on it is either a
    // word this module chose or an id read back out of another store — so the only charge is
    // the fixed per-record overhead every stored record costs.
    if store.would_exceed_stored_bytes(STORED_RECORD_OVERHEAD_BYTES) {
        return Err(resource_limit_exceeded(format!(
            "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of caller-supplied \
             data, so it cannot store another transit gateway route table association",
            store.stored_bytes()
        )));
    }

    let record = TransitGatewayRouteTableAssociationRecord {
        transit_gateway_route_table_id: transit_gateway_route_table_id.to_string(),
        transit_gateway_attachment_id: transit_gateway_attachment_id.to_string(),
        resource_id,
        resource_type,
        state: TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING.to_string(),
        pending: Some(PendingStateMove::starting_now(
            TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
        )),
    };
    let view = record.view();
    store.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
    store.transit_gateway_associations.insert(
        (
            transit_gateway_route_table_id.to_string(),
            transit_gateway_attachment_id.to_string(),
        ),
        record,
    );
    Ok(view)
}
