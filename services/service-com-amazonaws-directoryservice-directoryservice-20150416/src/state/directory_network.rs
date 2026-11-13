// SPDX-License-Identifier: CC0-1.0

//! WHERE A DIRECTORY'S SERVERS SIT ON THE NETWORK: the subnets a create places them in, and
//! the address each one answers DNS at.
//!
//! `DirectoryVpcSettings$SubnetIds`' own documentation says "Directory Service creates a
//! directory server and a DNS server in each of these subnets", and
//! `DirectoryDescription$DnsIpAddrs` reports "the IP addresses of the DNS servers for the
//! directory" — for a Simple AD or a Managed Microsoft AD directory, the addresses of those
//! very servers. A server placed in a subnet holds an address OUT OF THAT SUBNET'S OWN CIDR
//! block, so the member cannot be answered from the request alone: the request names subnet
//! IDENTIFIERS, and which block each one carries is a fact of the account's network.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. So the ONE answer to what
//! block a subnet carries, which zone it sits in and whether the account has it at all is
//! EC2 — asked the way any emulated service reaches another, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process (`services/common/src/network.rs` states
//! that contract). [`super::ec2_subnets`] is that hop; this module is what a create KEEPS of
//! the answer and what it counts out of it.
//!
//! What a create keeps is [`SubnetNetwork`], one per subnet it could resolve, and every
//! address this service reports is counted out of those blocks ([`server_address`]) — so the
//! addresses a directory serves DNS at sit inside the subnets the caller chose, and the
//! directory's controllers, its Availability Zones and its DNS addresses all come from the
//! one record.
//!
//! A hop that could NOT be made is evidence of nothing, and this one GATES no create: a
//! directory is stored with the `VpcSettings` the request named whatever EC2 says, because
//! this service reports that placement back as the request spelled it. An identifier EC2
//! reports no subnet for therefore simply contributes no block, and the directory serves no
//! address for it rather than one invented here.

/// One subnet of a directory's placement, as Amazon EC2 — the service that owns subnets —
/// reported it to the create that placed the directory's servers.
///
/// Only what this service REPORTS about the placement is kept: the block the servers'
/// addresses are counted out of and the zone the subnet sits in, which
/// `DirectoryVpcSettingsDescription$AvailabilityZones` ("the list of Availability Zones that
/// the directory is in") and `DomainController$AvailabilityZone` are. Nothing here is
/// derived from the Region name or from the request.
#[derive(Debug, Clone)]
pub struct SubnetNetwork {
    /// The subnet's own identifier, as EC2 reports it — the same identifier the create's
    /// settings block named it by.
    pub subnet_id: String,
    /// The subnet's IPv4 CIDR block, as EC2 reports it; `None` for a subnet it reports none
    /// for, whose servers then hold no IPv4 address.
    pub cidr_block: Option<String>,
    /// The Availability Zone the subnet sits in, as EC2 reports it.
    pub availability_zone: Option<String>,
    /// The VPC the subnet belongs to, as EC2 reports it — the network the directory's
    /// servers are really on, which is not the same claim as the `VpcId` the create's
    /// settings block named.
    ///
    /// What it is for: the security group this service provisions for those servers is a
    /// group IN that VPC ([`super::ec2_security_groups`]), so the create reads the VPC off
    /// the subnets the owner of subnets resolved rather than off the request.
    pub vpc_id: Option<String>,
}

/// How many addresses at the START of a subnet's CIDR block carry no server of the account.
///
/// From AWS's own addressing rules for a VPC subnet: the first four addresses and the last
/// address of every subnet CIDR block are reserved, so the first address a resource of the
/// account can hold is the fifth one in the block.
const RESERVED_LEADING_ADDRESSES: u32 = 4;

/// The address the `ordinal`-th server a directory places in `cidr_block` holds: the
/// `ordinal`-th assignable address of that block.
///
/// THE one place this service turns a subnet's block into a server's address, so a
/// directory's DNS addresses and its domain controllers' addresses cannot disagree, and
/// every address reported sits inside the subnet the server was placed in. Counted from the
/// first assignable address of the block ([`RESERVED_LEADING_ADDRESSES`]), so two servers of
/// one directory in one subnet never share an address.
///
/// `None` when `cidr_block` is not an IPv4 CIDR block, or when the block has no `ordinal`-th
/// assignable address — a block too small to carry that many servers.
#[must_use]
pub fn server_address(cidr_block: &str, ordinal: u32) -> Option<String> {
    let block: ::ipnet::Ipv4Net = cidr_block.parse().ok()?;
    let first = u32::from(block.network()).checked_add(RESERVED_LEADING_ADDRESSES)?;
    let address = first.checked_add(ordinal)?;
    // The last address of the block is reserved too, so an address that reaches it is past
    // what the block can carry.
    if address >= u32::from(block.broadcast()) {
        return None;
    }
    Some(::std::net::Ipv4Addr::from(address).to_string())
}
