// SPDX-License-Identifier: CC0-1.0

//! The PRIMARY network interface a LAUNCH creates for the instance it stores.
//!
//! "Each instance has a default network interface, called the primary network interface"
//! (`Instance$NetworkInterfaces` / the EC2 User Guide's "Elastic network interfaces"): an
//! instance in a VPC always has one, attached at device index 0, in the subnet the instance
//! runs in and carrying the instance's primary private IPv4 address. So the interface is a
//! resource the LAUNCH creates, which is why it is minted from
//! [`ComAmazonawsEc2Amazonec2State::insert_instance`] — the one place every launch path
//! stores its instance — rather than inside one operation's handler, exactly as the EBS
//! root volume beside it is (`super::launched_root_volumes`).
//!
//! Minting it there is also what makes the interface ONE record: the account's interface
//! store is then the single place "which interfaces does this instance have" is answered,
//! for `DescribeInstances` (through
//! [`ComAmazonawsEc2Amazonec2State::instance_network_interfaces`]) and for
//! `DescribeNetworkInterfaces` alike, so the two cannot disagree about one interface — and a
//! reporting operation needs to write nothing to answer with it.

use std::time::SystemTime;

use crate::ops::types::{GroupIdentifier, NetworkInterfaceAttachment};

use super::{
    ATTACHMENT_STATUS_ATTACHING, AMAZON_ASSIGNED_IP_OWNER_ID, ComAmazonawsEc2Amazonec2State,
    InstanceStateName, NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX,
    NETWORK_INTERFACE_TYPE_INTERFACE, NetworkInterfaceSpec,
    PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX, Subnet, region_of_zone_name, system_time_to_date_time,
    zone_id_for_zone_name,
};

impl ComAmazonawsEc2Amazonec2State {
    /// The subnet a launch places its instance in: the one the request named, or — "if you
    /// don't specify a subnet ID, we choose a default subnet from your default VPC for you"
    /// (`RunInstances`) — that Availability Zone's default subnet.
    ///
    /// `None` when neither resolves: a request naming a subnet this account does not hold,
    /// or an account with no default subnet in the zone. The ONE answer to "which subnet is
    /// this launch in", so the address the launch takes, the interface it creates and the
    /// VPC the instance reports all come from one subnet.
    pub fn launch_subnet(&self, subnet_id: Option<&str>, availability_zone: &str) -> Option<&Subnet> {
        match subnet_id {
            Some(subnet_id) => self.subnets.get(subnet_id),
            // "DefaultForAz - Indicates whether this is the default subnet for the
            // Availability Zone" (`Subnet$DefaultForAz`), which is exactly the subnet a
            // launch that names none is placed in.
            None => self.subnets.values().find(|subnet| {
                subnet.default_for_az == Some(true)
                    && subnet.availability_zone.as_deref() == Some(availability_zone)
            }),
        }
    }

    /// Give the instance `instance_id` the primary network interface its launch creates —
    /// see the module docs for why it is minted here.
    ///
    /// Everything about the interface comes off the instance and the subnet it is placed in
    /// ([`Self::launch_subnet`]): the subnet's VPC, zone and zone id, the instance's own
    /// primary private IPv4 address, and the security groups the launch gave the instance
    /// (or, for a launch that named none, its VPC's default group, which is what
    /// "Default: Amazon EC2 uses the default security group"
    /// (`RunInstancesRequest$SecurityGroupIds`) gives it). The attachment is at device index
    /// 0, is deleted with the instance — "when you launch an instance, the primary network
    /// interface is deleted on termination" — and is held by this instance.
    ///
    /// Nothing is minted when the launch is in no subnet this store holds, or when the
    /// interface store is already full: the instance is still stored in each case, exactly
    /// as it was before, the same way the root volume beside it is skipped.
    pub(super) fn mint_launched_primary_interface(&mut self, instance_id: &str) {
        let Some(instance) = self.instances.get(instance_id) else {
            return;
        };
        let private_ip = instance.private_ip.clone();
        let named_groups: Vec<(Option<String>, Option<String>)> = instance.security_groups.clone();
        let Some(subnet) = self.launch_subnet(
            instance.subnet_id.as_deref(),
            instance.placement_az.as_str(),
        ) else {
            return;
        };
        let subnet_id = subnet.subnet_id.clone().unwrap_or_default();
        let vpc_id = subnet.vpc_id.clone().unwrap_or_default();
        let availability_zone = subnet.availability_zone.clone().unwrap_or_default();
        let availability_zone_id = subnet
            .availability_zone_id
            .clone()
            .or_else(|| zone_id_for_zone_name(&availability_zone))
            .unwrap_or_default();
        // The Region the interface's private DNS names are built for is the one its zone
        // belongs to, read through the ONE reading of that ([`region_of_zone_name`]), as the
        // launched root volume beside it reads it — so an instance and its interface can
        // never be placed in two Regions.
        let region = region_of_zone_name(&availability_zone).to_string();
        // "Indicates whether instances launched in this subnet receive a public IPv4
        // address" (`Subnet$MapPublicIpOnLaunch`) — which is the attribute "if launching
        // into a default subnet, the default value is true"
        // (`InstanceNetworkInterfaceSpecification$AssociatePublicIpAddress`) reads off. A
        // launch that names no interface at all asks for nothing either way, so the subnet
        // it is placed in decides, and the address is assigned to the eth0 this mints just
        // as it is for an interface specification that asked.
        let maps_public_ip_on_launch = subnet.map_public_ip_on_launch == Some(true);
        if !self.has_network_interface_capacity() {
            return;
        }
        // Each group the launch named, completed to the id AND the name a report carries
        // ([`Self::group_identifier_of`]) — or, for a launch that named none, the VPC's own
        // default group, through the one reading of that default
        // ([`Self::default_security_group_of_vpc`]).
        let groups: Vec<GroupIdentifier> = if named_groups.is_empty() {
            self.default_security_group_of_vpc(&vpc_id)
                .into_iter()
                .collect()
        } else {
            named_groups
                .iter()
                .map(|(group_id, group_name)| {
                    self.group_identifier_of(
                        Some(vpc_id.as_str()),
                        group_id.as_deref(),
                        group_name.as_deref(),
                    )
                })
                .collect()
        };
        let attachment_id = self.next_id(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX);
        let attachment = NetworkInterfaceAttachment {
            attach_time: Some(system_time_to_date_time(SystemTime::now())),
            attachment_id: Some(attachment_id),
            // "Indicates whether the network interface is deleted when the instance is
            // terminated" (`InstanceNetworkInterfaceAttachment$DeleteOnTermination`): the
            // primary interface of a launch goes with its instance.
            delete_on_termination: Some(true),
            device_index: Some(PRIMARY_NETWORK_INTERFACE_DEVICE_INDEX),
            network_card_index: Some(0),
            instance_id: Some(instance_id.to_string()),
            instance_owner_id: Some(::engine::emulator_account_id().to_string()),
            // "attaching" (`AttachmentStatus`): an attachment is BEING MADE when the call
            // that makes it answers, so the launch's own interface is attaching and the
            // clock owes it `attached` below — the same reading an attachment
            // `AttachNetworkInterface` makes takes
            // ([`ComAmazonawsEc2Amazonec2State::start_network_interface_attachment`]), so a
            // caller that reads the instance straight after its launch still sees the
            // transient state it waits on.
            status: Some(ATTACHMENT_STATUS_ATTACHING.to_string()),
            // ENA Express is a setting of an attachment something asked for; a launch here
            // asks for none.
            ena_srd_specification: None,
            ena_queue_count: None,
        };
        // Through the ONE writer seam every interface of this store is written by, so the
        // id, the MAC address, the private DNS names and the `in-use` status an attached
        // interface reports are derived in exactly one place.
        let network_interface_id = self.insert_network_interface(NetworkInterfaceSpec {
            subnet_id,
            vpc_id,
            availability_zone,
            availability_zone_id,
            region: region.clone(),
            // AWS reports an empty description for an interface nothing described.
            description: String::new(),
            interface_type: NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
            private_ip_addresses: vec![private_ip],
            groups,
            attachment: Some(attachment),
            // The caller's own instance holds it; no other service manages it.
            requester: None,
            // A launch tags the interfaces its own `TagSpecifications` name; nothing tags
            // this record here.
            tags: Vec::new(),
        });
        // The completion the clock owes that attachment, recorded through the one seam
        // every attachment of this store records it through.
        if let Some(network_interface_id) = network_interface_id.as_deref() {
            self.owe_network_interface_attachment_completion(network_interface_id);
        }
        // The address goes to the interface this created, out of Amazon's own pool, through
        // the one place such an address is assigned ([`Self::assign_amazon_public_ipv4`]) —
        // so the instance, its interface's association and `DescribeNetworkInterfaces` all
        // report the one record.
        if maps_public_ip_on_launch {
            if let Some(network_interface_id) = network_interface_id {
                let assigned = self.assign_amazon_public_ipv4(&network_interface_id, &region);
                if let Some(instance) = self.instances.get_mut(instance_id) {
                    instance.public_ip = assigned;
                }
            }
        }
    }

    /// Delete every network interface whose attachment is to an instance that has reached
    /// `terminated` and is marked to be deleted with it.
    ///
    /// The reverse of the mint above, and the clock's: a termination's own span is what
    /// takes the instance to `terminated` ([`Self::materialize_due_transitions`], the one
    /// caller), and the interface goes at that same instant — "indicates whether the network
    /// interface is deleted when the instance is terminated"
    /// (`InstanceNetworkInterfaceAttachment$DeleteOnTermination`). Through
    /// [`Self::remove_network_interface`], so the addresses it held go back to its subnet
    /// and nothing goes on referring to it.
    ///
    /// The instance's own primary private address is released by that same caller, at the
    /// instant the termination COMPLETES rather than here: this sweep sees every instance
    /// that has ever reached `terminated`, so freeing an address from it would free one a
    /// new holder has since been given.
    ///
    /// An interface whose attachment says otherwise is left exactly where it is: it outlives
    /// the instance it was attached to, which is what `DeleteOnTermination` `false` means.
    pub(super) fn delete_interfaces_of_terminated_instances(&mut self) {
        let terminated: Vec<String> = self
            .instances
            .values()
            .filter(|instance| instance.state == InstanceStateName::Terminated)
            .map(|instance| instance.instance_id.clone())
            .collect();
        if terminated.is_empty() {
            return;
        }
        let doomed: Vec<String> = self
            .network_interfaces
            .iter()
            .filter(|(_, interface)| {
                interface.attachment.as_ref().is_some_and(|attachment| {
                    attachment.delete_on_termination == Some(true)
                        && attachment
                            .instance_id
                            .as_ref()
                            .is_some_and(|held| terminated.contains(held))
                })
            })
            .map(|(network_interface_id, _)| network_interface_id.clone())
            .collect();
        for network_interface_id in doomed {
            // A public IPv4 address AWS assigned the interface itself goes with the
            // interface — "Amazon Web Services charges for all public IPv4 addresses"
            // (`AssociatePublicIpAddress`), and nothing holds it once the interface it was
            // assigned to is gone — so the instance that reported it as its own public
            // address reports none. An Elastic IP the account allocated is NOT released
            // here: it outlives the instance, which is what an allocation is.
            let amazon_assigned = self
                .network_interfaces
                .get(&network_interface_id)
                .and_then(|interface| interface.association.as_ref())
                .filter(|association| {
                    association.ip_owner_id.as_deref() == Some(AMAZON_ASSIGNED_IP_OWNER_ID)
                })
                .and_then(|association| association.public_ip.clone());
            let held_by = self
                .network_interfaces
                .get(&network_interface_id)
                .and_then(|interface| interface.attachment.as_ref())
                .and_then(|attachment| attachment.instance_id.clone());
            self.remove_network_interface(&network_interface_id);
            if let (Some(public_ip), Some(instance_id)) = (amazon_assigned, held_by) {
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    if instance.public_ip.as_deref() == Some(public_ip.as_str()) {
                        instance.public_ip = None;
                    }
                }
            }
        }
    }
}
