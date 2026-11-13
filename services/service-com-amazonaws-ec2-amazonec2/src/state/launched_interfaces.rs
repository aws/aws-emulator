// SPDX-License-Identifier: CC0-1.0

//! The network interfaces a launch was TOLD to give the instance it stores.
//!
//! "The network interfaces to associate with the instance"
//! (`RunInstancesRequest$NetworkInterfaces`): a launch that names them launches its instance
//! with exactly those interfaces, each attached at the device index its entry names, instead
//! of with the single primary interface a launch that names none gets
//! ([`super::launched_primary_interfaces`]). Both are minted from
//! [`ComAmazonawsEc2Amazonec2State::insert_instance`] — the one place every launch path
//! stores its instance — so the account's interface store stays the single place "which
//! interfaces does this instance have" is answered, for `DescribeInstances` and for
//! `DescribeNetworkInterfaces` alike.
//!
//! An entry that names an interface by id ATTACHES that existing interface rather than
//! creating one, through the same seam `AttachNetworkInterface` goes through
//! ([`ComAmazonawsEc2Amazonec2State::start_network_interface_attachment`]), so an interface
//! attached by a launch and one attached by a later call cannot be recorded differently.

use std::time::SystemTime;

use crate::ops::types::{
    AttachmentEnaSrdSpecification, GroupIdentifier, NetworkInterfaceAssociation,
    NetworkInterfaceAttachment,
};

use super::{
    ATTACHMENT_STATUS_ATTACHING, ComAmazonawsEc2Amazonec2State, DEFAULT_SECURITY_GROUP_NAME,
    NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX, NetworkInterfaceAttachmentSpec, NetworkInterfaceSpec,
    PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX, public_dns_name, system_time_to_date_time,
};

/// The `IpOwnerId` of a public IPv4 address AWS assigned itself, as opposed to an Elastic IP
/// the account allocated.
///
/// The one word the reporting side of this fact reads
/// ([`crate::ops::describe_network_interface_attribute::AMAZON_ASSIGNED_IP_OWNER_ID`]), named
/// here so a launch that assigns such an address and the attribute that reports it cannot
/// spell its owner two ways.
pub use crate::ops::describe_network_interface_attribute::AMAZON_ASSIGNED_IP_OWNER_ID;

/// One network interface a launch was told to give each instance it mints, with everything
/// about it already resolved: the subnet it sits in, the groups applied to it and the
/// addresses taken for it.
///
/// The handler resolves this against the account's subnets, groups and interfaces and TAKES
/// the addresses before anything is stored, so a launch that cannot be given what it asked
/// for leaves no half-built instance behind; what is left here is the writing of it.
#[derive(Debug, Clone, Default)]
pub struct LaunchedInterface {
    /// "The position of the network interface in the attachment order. A primary network
    /// interface has a device index of 0" (`InstanceNetworkInterfaceSpecification$DeviceIndex`).
    pub device_index: i32,
    /// "The index of the network card … The primary network interface must be assigned to
    /// network card index 0" (`$NetworkCardIndex`).
    pub network_card_index: i32,
    /// An EXISTING interface of the account this entry attaches (`$NetworkInterfaceId`);
    /// `None` for one the launch creates.
    pub network_interface_id: Option<String>,
    /// The subnet the interface sits in, and that subnet's VPC, zone, zone id and Region —
    /// one subnet, read once, so the interface, the addresses taken for it and the instance
    /// cannot disagree about where it is.
    pub subnet_id: String,
    pub vpc_id: String,
    pub availability_zone: String,
    pub availability_zone_id: String,
    pub region: String,
    /// "The description of the network interface" (`$Description`) — empty for one the
    /// launch described not at all, which is what AWS reports for such an interface.
    pub description: String,
    /// "The type of network interface" (`$InterfaceType`).
    pub interface_type: String,
    /// The security groups applied to the interface, as a describe reports them.
    pub groups: Vec<GroupIdentifier>,
    /// The private IPv4 addresses the interface holds, PRIMARY FIRST, already taken out of
    /// its subnet's pool. Empty for an interface that carries none, and for an existing
    /// interface, which keeps the addresses it already holds.
    pub private_ip_addresses: Vec<String>,
    /// "If set to true, the interface is deleted when the instance is terminated"
    /// (`$DeleteOnTermination`).
    pub delete_on_termination: bool,
    /// "Indicates whether to assign a public IPv4 address to an instance you launch in a
    /// VPC" (`$AssociatePublicIpAddress`).
    pub associate_public_ip_address: bool,
    /// "Specifies the ENA Express settings for the network interface that's attached to the
    /// instance" (`$EnaSrdSpecification`), as the attachment reports them.
    pub ena_srd_specification: Option<AttachmentEnaSrdSpecification>,
    /// "The number of ENA queues to be created with the instance" (`$EnaQueueCount`).
    pub ena_queue_count: Option<i32>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// The default security group of the VPC `vpc_id`, as the `GroupIdentifier` a report
    /// carries — `None` when this store holds no such group.
    ///
    /// "Default: Amazon EC2 uses the default security group"
    /// (`RunInstancesRequest$SecurityGroupIds`) — the group "your VPC automatically comes
    /// with" (the Amazon VPC User Guide's "Default security groups for your VPCs") — so an
    /// interface created without groups gets it rather than none. The ONE reading of that
    /// default, so every path that creates an interface applies the same group to one.
    pub fn default_security_group_of_vpc(&self, vpc_id: &str) -> Option<GroupIdentifier> {
        self.security_groups
            .values()
            .find(|group| {
                group.vpc_id.as_deref() == Some(vpc_id)
                    && group.group_name.as_deref() == Some(DEFAULT_SECURITY_GROUP_NAME)
            })
            .map(|group| GroupIdentifier {
                group_id: group.group_id.clone(),
                group_name: group.group_name.clone(),
            })
    }

    /// Give the instance `instance_id` the interfaces `interfaces` describes, and answer
    /// whether every one of them was stored.
    ///
    /// `false` means the interface store filled up part way through, so the instance holds
    /// fewer interfaces than the launch asked for: the handler asked the store's ceilings
    /// before the first write, under the one lock it holds, so it answers that with the
    /// refusal those ceilings earn rather than reporting interfaces nothing holds.
    ///
    /// Each entry is created or attached through the one seam for its kind, so nothing about
    /// an interface is derived twice: a created one goes in through
    /// [`Self::insert_network_interface`] attached at its device index, and an existing one
    /// goes through [`Self::start_network_interface_attachment`]. Either way the attachment
    /// is `attaching` and the clock owes it its completion
    /// ([`Self::owe_network_interface_attachment_completion`]).
    pub(super) fn mint_launched_interfaces(
        &mut self,
        instance_id: &str,
        interfaces: &[LaunchedInterface],
    ) -> bool {
        let mut stored_all = true;
        for interface in interfaces {
            let network_interface_id = match interface.network_interface_id.clone() {
                // An interface the account already holds is attached where it is: its
                // subnet, addresses and groups are its own, and only the attachment is new.
                Some(existing) => {
                    if self
                        .start_network_interface_attachment(
                            &existing,
                            NetworkInterfaceAttachmentSpec {
                                instance_id: instance_id.to_string(),
                                device_index: interface.device_index,
                                network_card_index: interface.network_card_index,
                                ena_srd_specification: interface.ena_srd_specification.clone(),
                                ena_queue_count: interface.ena_queue_count,
                            },
                        )
                        .is_none()
                    {
                        stored_all = false;
                        continue;
                    }
                    existing
                }
                None => {
                    let attachment = NetworkInterfaceAttachment {
                        attach_time: Some(system_time_to_date_time(SystemTime::now())),
                        attachment_id: Some(self.next_id(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX)),
                        // "If set to true, the interface is deleted when the instance is
                        // terminated" (`DeleteOnTermination`) — what the launch asked for,
                        // which for an interface a launch creates defaults to true.
                        delete_on_termination: Some(interface.delete_on_termination),
                        device_index: Some(interface.device_index),
                        network_card_index: Some(interface.network_card_index),
                        instance_id: Some(instance_id.to_string()),
                        instance_owner_id: Some(::engine::emulator_account_id().to_string()),
                        // An attachment is BEING MADE when the call that makes it answers,
                        // exactly as it is for the primary interface of a launch that named
                        // none and for `AttachNetworkInterface`; the completion the clock
                        // owes it is recorded below.
                        status: Some(ATTACHMENT_STATUS_ATTACHING.to_string()),
                        ena_srd_specification: interface.ena_srd_specification.clone(),
                        ena_queue_count: interface.ena_queue_count,
                    };
                    let Some(created) = self.insert_network_interface(NetworkInterfaceSpec {
                        subnet_id: interface.subnet_id.clone(),
                        vpc_id: interface.vpc_id.clone(),
                        availability_zone: interface.availability_zone.clone(),
                        availability_zone_id: interface.availability_zone_id.clone(),
                        region: interface.region.clone(),
                        description: interface.description.clone(),
                        interface_type: interface.interface_type.clone(),
                        private_ip_addresses: interface.private_ip_addresses.clone(),
                        groups: interface.groups.clone(),
                        attachment: Some(attachment),
                        // The caller's own instance holds it; no other service manages it.
                        requester: None,
                        tags: Vec::new(),
                    }) else {
                        stored_all = false;
                        continue;
                    };
                    // Through the one seam an owed attachment completion is recorded by, so
                    // an interface a launch creates and attaches reaches `attached` over the
                    // same span as one attached by a later call.
                    self.owe_network_interface_attachment_completion(&created);
                    created
                }
            };
            // "Indicates whether to assign a public IPv4 address to an instance you launch
            // in a VPC. The public IP address can only be assigned to a network interface for
            // eth0, and can only be assigned to a new network interface, not an existing one"
            // (`AssociatePublicIpAddress`) — so the address goes to the interface the launch
            // created at device index 0, and the instance reports it as its own public
            // address.
            if interface.associate_public_ip_address
                && interface.network_interface_id.is_none()
                && interface.device_index == PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX
            {
                let assigned =
                    self.assign_amazon_public_ipv4(&network_interface_id, &interface.region);
                if let Some(instance) = self.instances.get_mut(instance_id) {
                    instance.public_ip = assigned;
                }
            }
        }
        stored_all
    }

    /// Assign the interface `network_interface_id` a public IPv4 address out of Amazon's own
    /// pool, and answer the address — `None` when the pool's search was exhausted or the
    /// store holds no such interface.
    ///
    /// The address is the store's own mint ([`Self::mint_elastic_ip`], the one place a public
    /// IPv4 address of this store comes from — a caller cannot specify which address AWS
    /// assigns it), and what the interface then holds is an association whose owner is
    /// [`AMAZON_ASSIGNED_IP_OWNER_ID`] and which carries neither an allocation nor an
    /// association id: those belong to an Elastic IP the account allocated and associated.
    /// `DescribeNetworkInterfaces`, `DescribeNetworkInterfaceAttribute`'s
    /// `associatePublicIpAddress` and the instance's own report all read that one record.
    pub(super) fn assign_amazon_public_ipv4(
        &mut self,
        network_interface_id: &str,
        region: &str,
    ) -> Option<String> {
        if !self.network_interfaces.contains_key(network_interface_id) {
            return None;
        }
        let public_ip = self.mint_elastic_ip()?;
        let public_dns = public_dns_name(&public_ip, region);
        let interface = self.network_interfaces.get_mut(network_interface_id)?;
        interface.association = Some(NetworkInterfaceAssociation {
            allocation_id: None,
            association_id: None,
            ip_owner_id: Some(AMAZON_ASSIGNED_IP_OWNER_ID.to_string()),
            public_dns_name: Some(public_dns.clone()),
            public_ip: Some(public_ip.clone()),
            customer_owned_ip: None,
            carrier_ip: None,
        });
        // "The public DNS name" of the interface follows the public address it holds, which is
        // what makes the two one fact rather than two.
        interface.public_dns_name = Some(public_dns);
        Some(public_ip)
    }
}
