// SPDX-License-Identifier: CC0-1.0

//! The load balancer's OWN elastic network interfaces, in the inventory EC2 holds.
//!
//! A load balancer is reached at an ADDRESS inside the VPC it is placed in: Elastic Load
//! Balancing puts one service-managed elastic network interface in each subnet the load
//! balancer is attached to, takes a private address of that subnet for it — and, for a load
//! balancer whose nodes carry public addresses, an Amazon-assigned public address beside it
//! ([`place_in_subnets`]) — and those interfaces are ordinary describable EC2 network
//! interfaces — the model itself carries the
//! `NetworkInterfaceType` values a load balancer's interface is of
//! (`smithy_query(query="shape",shape="NetworkInterfaceType")`: `load_balancer`,
//! `network_load_balancer`) and marks one a service created and manages through
//! `NetworkInterface$RequesterId` / `NetworkInterface$RequesterManaged`. So a client doing an
//! address-level diagnosis of its VPC — which interface is the load balancer, which address
//! belongs to it — finds them with `DescribeNetworkInterfaces`.
//!
//! Those interfaces are not this service's records: they live in the account's EC2 inventory,
//! the one `DescribeNetworkInterfaces` answers from, so they are written there — exactly as
//! another service's endpoint addresses are — rather than kept in a copy of this store that
//! no reader of that inventory can see. [`release_load_balancer_interfaces`] is the exact
//! reverse: the interfaces carry the load balancer's traffic, so they do not outlive it.
//!
//! LOCKING. The EC2 store lock is taken for one placement or release and dropped before
//! returning. The caller may hold this service's own store, so the order is always this
//! service and then EC2; EC2 never reaches this service, so no cycle exists.

use ::std::collections::BTreeSet;

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use super::host_counts::load_balancer_dimension_value;
use super::{load_balancer_type_code, LoadBalancerRecord};

/// "The alias or Amazon Web Services account ID of the principal or service that created the
/// network interface" (`NetworkInterface$RequesterId`) for an interface Elastic Load
/// Balancing created: the alias the service is published under, which is also what the
/// `requester-id` filter of a describe selects it by.
pub const SERVICE_REQUESTER: &str = "amazon-elb";

/// The `NetworkInterfaceType` of an Application Load Balancer's own interfaces, from the
/// modelled enum's `load_balancer` member.
pub const INTERFACE_TYPE_APPLICATION: &str = "load_balancer";

/// The `NetworkInterfaceType` of a Network Load Balancer's own interfaces, from the same
/// enum's `network_load_balancer` member.
pub const INTERFACE_TYPE_NETWORK: &str = "network_load_balancer";

/// The `NetworkInterfaceType` of a Gateway Load Balancer's own interfaces, from the same
/// enum's `gateway_load_balancer` member.
pub const INTERFACE_TYPE_GATEWAY: &str = "gateway_load_balancer";

/// Which of those types the interfaces of a load balancer of `lb_type` are of — one per
/// FAMILY, as the enum names them.
#[must_use]
pub fn interface_type(lb_type: &str) -> &'static str {
    match load_balancer_type_code(lb_type) {
        "net" => INTERFACE_TYPE_NETWORK,
        "gwy" => INTERFACE_TYPE_GATEWAY,
        _ => INTERFACE_TYPE_APPLICATION,
    }
}

/// How the account's EC2 inventory describes an interface carrying the traffic of the load
/// balancer `record`, so a caller reading that inventory can tell which load balancer an
/// interface belongs to: `ELB` and the load balancer's own `<family>/<name>/<id>`.
#[must_use]
pub fn interface_description(record: &LoadBalancerRecord) -> String {
    format!("ELB {}", load_balancer_dimension_value(record))
}

/// Put one service-managed interface in each subnet `record` is attached to, and answer the
/// `NetworkInterfaceId` of each one placed.
///
/// Each interface takes a private address out of its own subnet's range through the EC2
/// store's one choice-and-take, so no two resources of a subnet can be handed the same
/// address and no address a load balancer holds can be handed to anything else. A subnet the
/// inventory does not hold, and one whose range has no free address left, place no interface
/// rather than one carrying an invented address.
///
/// Nothing is placed on an engine that holds no such inventory: the interfaces are that
/// service's records, and this path never makes a second home for them.
pub fn place_load_balancer_interfaces<Eng: Send + Sync + 'static>(
    engine: &Eng,
    record: &LoadBalancerRecord,
) -> Vec<String> {
    let Some(state) = ec2::service_state(engine) else {
        return Vec::new();
    };
    let zones: Vec<&super::AvailabilityZoneRecord> = record.availability_zones.iter().collect();
    let mut store = state.lock();
    place_in_subnets(&mut store, record, &zones)
}

/// Make the interfaces `record` holds agree with the subnets it is attached to NOW, and
/// answer the `NetworkInterfaceId` of each one it holds afterwards.
///
/// A subnet the call RETAINED keeps the interface already in it, address and id and all — the
/// load balancer never stopped being reachable there — so only the delta moves: an interface
/// in a subnet the load balancer has left is given back, and a subnet it has just been
/// attached to gets one of its own ([`place_load_balancer_interfaces`]).
///
/// `held` is what the record held before the call. On an engine with no inventory to read
/// nothing is reconciled and `held` stands, so a call cannot drop a reference to an interface
/// it could not resolve.
pub fn reconcile_load_balancer_interfaces<Eng: Send + Sync + 'static>(
    engine: &Eng,
    record: &LoadBalancerRecord,
    held: &[String],
) -> Vec<String> {
    let Some(state) = ec2::service_state(engine) else {
        return held.to_vec();
    };
    let mut store = state.lock();
    let subnet_of = |store: &ec2::ComAmazonawsEc2Amazonec2State, id: &String| -> Option<String> {
        store
            .network_interfaces
            .get(id)
            .and_then(|interface| interface.subnet_id.clone())
    };
    let attached: BTreeSet<&str> = record
        .availability_zones
        .iter()
        .map(|zone| zone.subnet_id.as_str())
        .collect();
    let mut kept: Vec<String> = Vec::with_capacity(held.len());
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for id in held {
        match subnet_of(&store, id).filter(|subnet| attached.contains(subnet.as_str())) {
            Some(subnet) => {
                covered.insert(subnet);
                kept.push(id.clone());
            }
            // An interface in a subnet this load balancer has left, and one the inventory no
            // longer holds, are both references it does not keep.
            None => {
                let _removed = store.remove_network_interface(id);
            }
        }
    }
    let opening: Vec<&super::AvailabilityZoneRecord> = record
        .availability_zones
        .iter()
        .filter(|zone| !covered.contains(&zone.subnet_id))
        .collect();
    kept.extend(place_in_subnets(&mut store, record, &opening));
    kept
}

/// Place one interface of `record` in each subnet `zones` names, inside an inventory guard
/// the caller already holds.
///
/// The one placement both [`place_load_balancer_interfaces`] and
/// [`reconcile_load_balancer_interfaces`] perform, so a load balancer's first interfaces and
/// the ones a later subnet change opens are stored identically.
fn place_in_subnets(
    store: &mut ec2::ComAmazonawsEc2Amazonec2State,
    record: &LoadBalancerRecord,
    zones: &[&super::AvailabilityZoneRecord],
) -> Vec<String> {
    let interface_type = interface_type(&record.lb_type);
    let description = interface_description(record);
    // Whether this load balancer's NODES carry public addresses, which is the one reading of
    // that scheme rule ([`super::internet_facing_placement::placement_needs_internet_gateway`],
    // which refuses such a placement in a VPC whose public addresses route nowhere).
    let public_nodes = super::internet_facing_placement::placement_needs_internet_gateway(
        record.family(),
        record.scheme.as_deref().unwrap_or_default(),
    );
    // The groups the load balancer carries govern traffic to its network interfaces
    // (`CreateLoadBalancer$SecurityGroups`), so they are the groups of those interfaces.
    let groups: Vec<ec2_types::GroupIdentifier> = record
        .security_groups
        .iter()
        .map(|group_id| ec2_types::GroupIdentifier {
            group_id: Some(group_id.clone()),
            group_name: None,
        })
        .collect();
    let mut placed: Vec<String> = Vec::with_capacity(zones.len());
    for zone in zones {
        let Some(subnet) = store.subnets.get(&zone.subnet_id) else {
            continue;
        };
        let (vpc_id, availability_zone, availability_zone_id, cidr_block) = (
            subnet.vpc_id.clone().unwrap_or_default(),
            subnet
                .availability_zone
                .clone()
                .unwrap_or_else(|| zone.zone_name.clone()),
            subnet.availability_zone_id.clone().unwrap_or_default(),
            subnet.cidr_block.clone(),
        );
        let Some(cidr_block) = cidr_block else {
            continue;
        };
        let Some(address) = store.mint_private_ip_in_cidr(&cidr_block) else {
            continue;
        };
        let spec = ec2::NetworkInterfaceSpec {
            subnet_id: zone.subnet_id.clone(),
            vpc_id,
            availability_zone,
            availability_zone_id,
            region: record.region.clone(),
            description: description.clone(),
            interface_type: interface_type.to_string(),
            private_ip_addresses: vec![address.clone()],
            groups: groups.clone(),
            // A load balancer's interface is attached to no instance of the account: the
            // load balancer itself holds it, which is what `RequesterManaged` reports.
            attachment: None,
            requester: Some(SERVICE_REQUESTER.to_string()),
            // The load balancer carries the create's tags; its interfaces are not the
            // resource the request tagged.
            tags: Vec::new(),
            // A load balancer's interface here is reached over IPv4 alone, so it asks for
            // no IPv6 space.
            ..Default::default()
        };
        match store.insert_network_interface(spec) {
            Some(network_interface_id) => {
                // "The nodes of an Internet-facing load balancer have public IP addresses"
                // (`CreateLoadBalancerInput$Scheme`), and a node IS one of these interfaces —
                // so each interface of such a load balancer holds an Amazon-assigned public
                // address beside its private one, taken through EC2's own one place a public
                // IPv4 address of the account comes from. An internal load balancer's nodes
                // "have only private IP addresses", so theirs hold none, and a Gateway Load
                // Balancer carries no scheme at all.
                if public_nodes {
                    let _assigned =
                        store.assign_amazon_public_ipv4(&network_interface_id, &record.region);
                }
                placed.push(network_interface_id);
            }
            None => {
                // The inventory holds no more interfaces, so the address this one would
                // have held goes back to the subnet it came out of.
                store.release_private_ip(&address);
            }
        }
    }
    placed
}

/// Give the interfaces `network_interface_ids` names the security groups `group_ids`, which
/// is what a change to the load balancer's groups changes about them.
///
/// A load balancer's groups govern traffic to its network interfaces
/// (`SetSecurityGroups$SecurityGroups`), so the groups those interfaces report are the groups
/// the load balancer carries now — the interfaces themselves are not replaced, so each keeps
/// the address it holds. An interface the inventory no longer holds is left alone.
///
/// LOCKING: as [`place_load_balancer_interfaces`].
pub fn apply_load_balancer_security_groups<Eng: Send + Sync + 'static>(
    engine: &Eng,
    network_interface_ids: &[String],
    group_ids: &[String],
) {
    if network_interface_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let groups: Vec<ec2_types::GroupIdentifier> = group_ids
        .iter()
        .map(|group_id| ec2_types::GroupIdentifier {
            group_id: Some(group_id.clone()),
            group_name: None,
        })
        .collect();
    let mut store = state.lock();
    for network_interface_id in network_interface_ids {
        if let Some(interface) = store.network_interfaces.get_mut(network_interface_id) {
            interface.groups = Some(groups.clone());
        }
    }
}

/// Take the interfaces `network_interface_ids` names back out of the account's EC2 inventory
/// — the exact reverse of [`place_load_balancer_interfaces`], so the addresses they held can
/// be handed out again.
pub fn release_load_balancer_interfaces<Eng: Send + Sync + 'static>(
    engine: &Eng,
    network_interface_ids: &[String],
) {
    if network_interface_ids.is_empty() {
        return;
    }
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for network_interface_id in network_interface_ids {
        let _removed = store.remove_network_interface(network_interface_id);
    }
}
