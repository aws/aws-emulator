// SPDX-License-Identifier: CC0-1.0

//! The elastic network interfaces a function's VPC attachment IS, in the inventory EC2 holds.
//!
//! "To connect a function to a VPC, Lambda creates an elastic network interface for each
//! subnet in your function's VPC configuration" ([`VpcConfig`](crate::ops::types::VpcConfig)):
//! the attachment is not a note on this service's record, it is those interfaces. They are
//! ordinary describable EC2 network interfaces — the model itself carries the
//! `NetworkInterfaceType` value one of them is of
//! (`smithy_query(query="shape",shape="NetworkInterfaceType")`: `lambda`) — so a client doing
//! an address-level diagnosis of its VPC finds them with `DescribeNetworkInterfaces`, and a
//! resource they hold a place in (the subnet they sit in, the security groups they carry)
//! cannot be deleted while they are there.
//!
//! Such an interface is ATTACHED to the service rather than to an instance of the account:
//! `NetworkInterface$Attachment` carries no `InstanceId` and names the owner AWS reports for
//! an attachment no instance holds ([`SERVICE_ATTACHMENT_OWNER`]), which is why the interface
//! is `in-use` while the function is attached. It is created IN the account, by Lambda acting
//! for the function, so `NetworkInterface$RequesterId` ("the alias or Amazon Web Services
//! account ID of the principal or service that created the network interface") names the
//! Lambda service principal rather than whoever made the call, while
//! `NetworkInterface$RequesterManaged` ("whether the network interface is being managed by
//! Amazon Web Services") is false, exactly as AWS reports one of these.
//!
//! Those interfaces are not this service's records: they live in the account's EC2 inventory,
//! the one `DescribeNetworkInterfaces` answers from, so they are written there rather than
//! kept in a copy of this store no reader of that inventory can see. The reverse —
//! [`release_function_interfaces`] — is exact: the interfaces carry the function's traffic, so
//! they do not outlive the attachment that asked for them.
//!
//! Resolving the attachment is also what tells a client WHICH VPC the function is attached to:
//! `VpcConfigResponse$VpcId` ("The ID of the VPC") is not a value the request carries, it is
//! the VPC the configured subnets belong to, which is EC2's own record of each subnet
//! ([`Subnet$VpcId`](::service_com_amazonaws_ec2_amazonec2::ops::types::Subnet)).
//!
//! Nothing is placed, and no VPC is reported, on evidence this emulator does not have: an
//! engine carrying no EC2 inventory, a subnet that inventory holds no record of, and a subnet
//! with no address left to take all leave the configuration as the caller wrote it and report
//! no id that was not looked up.
//!
//! LOCKING. The EC2 store lock is taken for one placement or release and dropped before
//! returning. The caller may hold this service's own store, so the order is always this
//! service and then EC2; EC2 never reaches this service, so no cycle exists.
//!
//! BOUNDS. What one request can ask for here is bounded before any of it is done: a
//! configuration attaches the function to at most [`MAX_ATTACHED_SUBNETS`] distinct subnets
//! carrying at most [`MAX_ATTACHED_SECURITY_GROUPS`] distinct groups — the ceilings the members
//! that name them publish — so neither the number of interfaces a call places nor the work it
//! does under the inventory's guard follows the length of the list it arrived with.

use ::std::collections::BTreeSet;

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::ops::types::VpcConfig;

/// "The Amazon Web Services account ID of the owner of the instance"
/// (`NetworkInterfaceAttachment$InstanceOwnerId`) for an attachment AWS itself made, which is
/// what an interface a service attached carries in place of an account id: no instance of the
/// account holds it.
pub const SERVICE_ATTACHMENT_OWNER: &str = "amazon-aws";

/// "The index of the device for the network interface attachment"
/// (`NetworkInterfaceAttachment$DeviceIndex`) of an interface a service attached, which is
/// never the primary device of an instance.
const SERVICE_ATTACHMENT_DEVICE_INDEX: i32 = 1;

/// The `NetworkInterfaceType` of a function's own interfaces, from the modelled EC2 enum's
/// `lambda` member.
pub const INTERFACE_TYPE: &str = "lambda";

/// How the account's EC2 inventory describes an interface of the function `function_name`, so
/// a caller reading that inventory can tell which function an interface belongs to: the
/// service and the function's own name. Fixed text apart from that name, and the same for
/// every interface of one function — a description is not an identifier.
#[must_use]
pub fn interface_description(function_name: &str) -> String {
    format!("AWS Lambda VPC ENI-{function_name}")
}

/// What a function's VPC configuration RESOLVED to, once the attachment it asks for has been
/// stood up: the VPC its subnets belong to, and the interfaces that now carry its traffic.
#[derive(Debug, Clone, Default)]
pub struct VpcAttachment {
    /// `VpcConfigResponse$VpcId`: the VPC the configured subnets belong to, or `None` when
    /// none of them resolves to one here (see the module docs).
    pub vpc_id: Option<String>,
    /// The `NetworkInterfaceId` of each interface the attachment holds, in the order of the
    /// subnets they sit in.
    pub network_interface_ids: Vec<String>,
}

/// Stand the attachment `vpc_config` asks for up: one service-managed interface in each
/// subnet it names, carrying the security groups it names, in the account's EC2 inventory.
///
/// Each interface takes a private address out of its own subnet's range through the EC2
/// store's one choice-and-take, so no two resources of a subnet can be handed the same
/// address. A configuration that attaches the function to nothing — no member, or the empty
/// lists that "remove the function's VPC configuration" — stands nothing up and resolves to
/// no VPC.
pub fn place_function_interfaces<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    function_name: &str,
    vpc_config: Option<&VpcConfig>,
) -> VpcAttachment {
    let Some(subnet_ids) = attached_subnets(vpc_config) else {
        return VpcAttachment::default();
    };
    let Some(state) = ec2::service_state(engine) else {
        return VpcAttachment::default();
    };
    let group_ids = security_group_ids(vpc_config);
    let mut store = state.lock();
    VpcAttachment {
        vpc_id: vpc_of(&store, &subnet_ids),
        network_interface_ids: place_in_subnets(
            &mut store,
            region,
            function_name,
            &subnet_ids,
            &group_ids,
        ),
    }
}

/// Make the interfaces a function holds agree with the configuration `vpc_config` it carries
/// NOW, and answer what the attachment resolves to afterwards.
///
/// A subnet the configuration KEPT keeps the interface already in it, address and id and all —
/// the function never stopped being reachable there — so only the delta moves: an interface in
/// a subnet the function has left is given back, a subnet it has just been attached to gets one
/// of its own, and every interface it keeps carries the groups the configuration names now,
/// because those groups govern the traffic of the interfaces rather than being a property of
/// one. A configuration that attaches the function to nothing gives every interface back.
///
/// `held` is what the function held before the call. On an engine with no inventory to read
/// nothing is reconciled and `held` stands, so a call cannot drop a reference to an interface
/// it could not resolve.
pub fn reconcile_function_interfaces<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    function_name: &str,
    vpc_config: Option<&VpcConfig>,
    held: &[String],
) -> VpcAttachment {
    let Some(state) = ec2::service_state(engine) else {
        return VpcAttachment {
            vpc_id: None,
            network_interface_ids: held.to_vec(),
        };
    };
    let subnet_ids = attached_subnets(vpc_config).unwrap_or_default();
    let group_ids = security_group_ids(vpc_config);
    let mut store = state.lock();
    let groups = group_identifiers(&store, &group_ids);
    let attached: BTreeSet<&str> = subnet_ids.iter().map(String::as_str).collect();
    let mut kept: Vec<String> = Vec::with_capacity(held.len());
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for id in held {
        let subnet = store
            .network_interfaces
            .get(id)
            .and_then(|interface| interface.subnet_id.clone())
            .filter(|subnet| attached.contains(subnet.as_str()));
        match subnet {
            Some(subnet) => {
                covered.insert(subnet);
                if let Some(interface) = store.network_interfaces.get_mut(id) {
                    interface.groups = Some(groups.clone());
                }
                kept.push(id.clone());
            }
            // An interface in a subnet this function has left, and one the inventory no
            // longer holds, are both references it does not keep.
            None => {
                let _removed = store.remove_network_interface(id);
            }
        }
    }
    let opening: Vec<String> = subnet_ids
        .iter()
        .filter(|subnet| !covered.contains(*subnet))
        .cloned()
        .collect();
    kept.extend(place_in_subnets(
        &mut store,
        region,
        function_name,
        &opening,
        &group_ids,
    ));
    VpcAttachment {
        vpc_id: vpc_of(&store, &subnet_ids),
        network_interface_ids: kept,
    }
}

/// Take the interfaces `network_interface_ids` names back out of the account's EC2 inventory —
/// the exact reverse of [`place_function_interfaces`], so the addresses they held can be handed
/// out again and the subnet and groups they held a place in can be deleted.
pub fn release_function_interfaces<Eng: Send + Sync + 'static>(
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

/// The most subnets one attachment stands an interface up in: the published ceiling of the
/// member that names them (`VpcConfig$SubnetIds`, `@length(min: 0, max: 16)` —
/// `smithy_query(query="shape",shape="VpcConfig")`), so the work one request can ask for is
/// bounded by the model's own bound and not by the size of the body it arrived in.
pub const MAX_ATTACHED_SUBNETS: usize = 16;

/// The same for the groups those interfaces carry: the ceiling of `VpcConfig$SecurityGroupIds`
/// (`@length(min: 0, max: 5)`), from the same shape.
pub const MAX_ATTACHED_SECURITY_GROUPS: usize = 5;

/// The subnets `vpc_config` asks for an interface in, or `None` when it asks for no
/// attachment at all: "A list of VPC subnet IDs" naming none creates no interface, and
/// "to remove the function's VPC configuration, pass an empty list".
///
/// "Lambda creates an elastic network interface for each subnet in your function's VPC
/// configuration": a subnet is one subnet however many times the list names it, so the list is
/// taken as the DISTINCT subnets it names, in the order it first names each, and no more of
/// them than [`MAX_ATTACHED_SUBNETS`] — which is what keeps the placement below one bounded
/// piece of work per request rather than one the caller sizes.
fn attached_subnets(vpc_config: Option<&VpcConfig>) -> Option<Vec<String>> {
    let named = vpc_config.and_then(|vpc_config| vpc_config.subnet_ids.as_deref())?;
    let subnets = distinct(named, MAX_ATTACHED_SUBNETS);
    (!subnets.is_empty()).then_some(subnets)
}

/// "A list of VPC security group IDs" (`VpcConfig$SecurityGroupIds`) the configuration names,
/// which are the groups of the interfaces the attachment stands up — the distinct groups it
/// names, bounded by [`MAX_ATTACHED_SECURITY_GROUPS`], for the reasons above.
fn security_group_ids(vpc_config: Option<&VpcConfig>) -> Vec<String> {
    vpc_config
        .and_then(|vpc_config| vpc_config.security_group_ids.as_deref())
        .map(|group_ids| distinct(group_ids, MAX_ATTACHED_SECURITY_GROUPS))
        .unwrap_or_default()
}

/// The first `limit` DISTINCT entries of `ids`, in the order they first appear.
fn distinct(ids: &[String], limit: usize) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .take(limit)
        .cloned()
        .collect()
}

/// Those ids as the describe reports an interface's groups: the id the configuration gave,
/// and the group's own name when the inventory holds it.
///
/// The groups the function carries govern traffic to its network interfaces
/// (`VpcConfig$SecurityGroupIds`), so they are the groups of those interfaces — both when an
/// interface is placed and when a later configuration change replaces them.
fn group_identifiers(
    store: &ec2::ComAmazonawsEc2Amazonec2State,
    group_ids: &[String],
) -> Vec<ec2_types::GroupIdentifier> {
    group_ids
        .iter()
        .map(|group_id| ec2_types::GroupIdentifier {
            group_id: Some(group_id.clone()),
            group_name: store
                .security_groups
                .get(group_id.as_str())
                .and_then(|group| group.group_name.clone()),
        })
        .collect()
}

/// The VPC the subnets `subnet_ids` belong to, as EC2's own record of each one names it —
/// the FIRST that resolves, since the interfaces of one function live in one VPC. `None` when
/// the inventory holds none of them.
fn vpc_of(store: &ec2::ComAmazonawsEc2Amazonec2State, subnet_ids: &[String]) -> Option<String> {
    subnet_ids
        .iter()
        .find_map(|subnet_id| store.subnets.get(subnet_id.as_str()))
        .and_then(|subnet| subnet.vpc_id.clone())
}

/// Place one interface of the function `function_name` in each subnet `subnet_ids` names,
/// inside an inventory guard the caller already holds, and answer the id of each one placed.
///
/// The one placement both [`place_function_interfaces`] and [`reconcile_function_interfaces`]
/// perform, so a function's first interfaces and the ones a later configuration change opens
/// are stored identically. A subnet the inventory does not hold, one with no CIDR block to
/// take an address out of, and one whose range has no free address left place no interface
/// rather than one carrying an invented address. A placement is asked BEFORE each write
/// whether the inventory holds room for another interface at all, so a full inventory stops
/// the placement rather than being discovered by a write that could not be made.
fn place_in_subnets(
    store: &mut ec2::ComAmazonawsEc2Amazonec2State,
    region: &str,
    function_name: &str,
    subnet_ids: &[String],
    group_ids: &[String],
) -> Vec<String> {
    let groups = group_identifiers(store, group_ids);
    let mut placed: Vec<String> = Vec::with_capacity(subnet_ids.len());
    for subnet_id in subnet_ids {
        if !store.has_network_interface_capacity() {
            break;
        }
        let Some(subnet) = store.subnets.get(subnet_id.as_str()) else {
            continue;
        };
        let (vpc_id, availability_zone, availability_zone_id, cidr_block) = (
            subnet.vpc_id.clone().unwrap_or_default(),
            subnet.availability_zone.clone().unwrap_or_default(),
            subnet.availability_zone_id.clone().unwrap_or_default(),
            subnet.cidr_block.clone(),
        );
        let Some(cidr_block) = cidr_block else {
            continue;
        };
        let Some(address) = store.mint_private_ip_in_cidr(&cidr_block) else {
            continue;
        };
        // The attachment: the function holds the interface, and no instance of the account
        // does — so it names the owner AWS reports for one a service attached and carries no
        // instance, which is what makes the interface `in-use` rather than `available`. The id
        // is minted by the store that mints every other id of this inventory.
        let attachment = ec2_types::NetworkInterfaceAttachment {
            attach_time: Some(ec2::system_time_to_date_time(::std::time::SystemTime::now())),
            attachment_id: Some(store.next_id(ec2::NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX)),
            // "Indicates whether the network interface is deleted when the instance is
            // terminated": no instance terminates this one — it is given back when the
            // attachment that asked for it is.
            delete_on_termination: Some(false),
            device_index: Some(SERVICE_ATTACHMENT_DEVICE_INDEX),
            network_card_index: Some(0),
            instance_id: None,
            instance_owner_id: Some(SERVICE_ATTACHMENT_OWNER.to_string()),
            status: Some(ec2::ATTACHMENT_STATUS_ATTACHED.to_string()),
            // ENA Express is a setting of an attachment something asked for; this asks for
            // none.
            ena_srd_specification: None,
            ena_queue_count: None,
        };
        let spec = ec2::NetworkInterfaceSpec {
            subnet_id: subnet_id.clone(),
            vpc_id,
            availability_zone,
            availability_zone_id,
            region: region.to_string(),
            description: interface_description(function_name),
            interface_type: INTERFACE_TYPE.to_string(),
            private_ip_addresses: vec![address.clone()],
            groups: groups.clone(),
            attachment: Some(attachment),
            // The interface is created IN this account, by Lambda acting for the function,
            // so it is managed by no service of its own: `RequesterManaged` is false, which
            // is what AWS reports for one of these. WHICH principal created it is named
            // below, because this seam attributes an unmanaged interface to the ambient
            // caller and this one was not created by whoever made the call.
            requester: None,
            // The function carries the create's tags; its interfaces are not the resource the
            // request tagged.
            tags: Vec::new(),
        };
        match store.insert_network_interface(spec) {
            Some(network_interface_id) => {
                // "The alias or Amazon Web Services account ID of the principal or service
                // that created the network interface" (`NetworkInterface$RequesterId`): this
                // one was created by LAMBDA, on the function's behalf — the service principal
                // a function's execution role lets assume it
                // ([`crate::state::EXECUTION_ROLE_PRINCIPAL`]) — and not by the caller whose
                // request happened to apply the configuration, nor by any session of one
                // function.
                if let Some(interface) = store.network_interfaces.get_mut(&network_interface_id) {
                    interface.requester_id =
                        Some(crate::state::EXECUTION_ROLE_PRINCIPAL.to_string());
                }
                placed.push(network_interface_id);
            }
            None => {
                // The inventory holds no more interfaces, so the address this one would have
                // held goes back to the subnet it came out of.
                store.release_private_ip(&address);
            }
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::{
        attached_subnets, interface_description, security_group_ids, MAX_ATTACHED_SECURITY_GROUPS,
        MAX_ATTACHED_SUBNETS,
    };
    use crate::ops::types::VpcConfig;

    /// A subnet id of the published form, numbered so a case can ask for as many distinct ones
    /// as it likes.
    fn subnet(index: usize) -> String {
        format!("subnet-{index:017x}")
    }

    /// One case of the table below: its name, the configuration, and the subnets an interface
    /// is placed in.
    type Row = (&'static str, Option<VpcConfig>, Option<Vec<String>>);

    /// A configuration asks for an interface only where it names a subnet to create one in,
    /// however it spells "no subnet" — and for one interface per DISTINCT subnet it names, no
    /// more of them than the member's own ceiling.
    #[test]
    fn only_a_configuration_naming_a_subnet_asks_for_an_interface() {
        let rows: [Row; 8] = [
            ("no configuration at all", None, None),
            ("no member at all", Some(VpcConfig::default()), None),
            (
                "an empty list, which removes the configuration",
                Some(VpcConfig {
                    subnet_ids: Some(Vec::new()),
                    security_group_ids: Some(vec!["sg-0a1b2c3d4e5f60718".to_string()]),
                    ipv6_allowed_for_dual_stack: None,
                }),
                None,
            ),
            (
                "one subnet",
                Some(VpcConfig {
                    subnet_ids: Some(vec![subnet(1)]),
                    security_group_ids: None,
                    ipv6_allowed_for_dual_stack: None,
                }),
                Some(vec![subnet(1)]),
            ),
            (
                "two subnets, in the order they were named",
                Some(VpcConfig {
                    subnet_ids: Some(vec![subnet(2), subnet(1)]),
                    security_group_ids: Some(Vec::new()),
                    ipv6_allowed_for_dual_stack: Some(true),
                }),
                Some(vec![subnet(2), subnet(1)]),
            ),
            (
                "one subnet named twice: one subnet, so one interface",
                Some(VpcConfig {
                    subnet_ids: Some(vec![subnet(1), subnet(1)]),
                    security_group_ids: None,
                    ipv6_allowed_for_dual_stack: None,
                }),
                Some(vec![subnet(1)]),
            ),
            (
                "a repeat between two distinct subnets, each kept where it first appeared",
                Some(VpcConfig {
                    subnet_ids: Some(vec![subnet(2), subnet(1), subnet(2)]),
                    security_group_ids: None,
                    ipv6_allowed_for_dual_stack: None,
                }),
                Some(vec![subnet(2), subnet(1)]),
            ),
            (
                "one distinct subnet past the member's ceiling: the ceiling stands",
                Some(VpcConfig {
                    subnet_ids: Some(
                        (0..=MAX_ATTACHED_SUBNETS).map(subnet).collect(),
                    ),
                    security_group_ids: None,
                    ipv6_allowed_for_dual_stack: None,
                }),
                Some((0..MAX_ATTACHED_SUBNETS).map(subnet).collect()),
            ),
        ];
        for (case, vpc_config, expected) in rows {
            let asked = attached_subnets(vpc_config.as_ref());
            assert_eq!(
                asked, expected,
                "case {case:?}: expected {expected:?}, got {asked:?}"
            );
        }
    }

    /// One case of the group table: its name, the groups the configuration names, and the
    /// groups the interfaces carry.
    type GroupRow = (&'static str, Option<Vec<String>>, Vec<String>);

    /// The groups those interfaces carry are read off the same configuration, on the same
    /// terms: the distinct groups it names, bounded by that member's own ceiling, and "no
    /// groups" is the empty list rather than an absent answer.
    #[test]
    fn the_groups_are_the_distinct_ones_the_configuration_names() {
        let group = |index: usize| format!("sg-{index:017x}");
        // (case, the groups the configuration names, the groups the interfaces carry)
        let rows: [GroupRow; 5] = [
            ("no member at all", None, Vec::new()),
            ("an empty list", Some(Vec::new()), Vec::new()),
            (
                "two groups, in the order they were named",
                Some(vec![group(2), group(1)]),
                vec![group(2), group(1)],
            ),
            (
                "one group named twice",
                Some(vec![group(1), group(1)]),
                vec![group(1)],
            ),
            (
                "one distinct group past the member's ceiling",
                Some((0..=MAX_ATTACHED_SECURITY_GROUPS).map(group).collect()),
                (0..MAX_ATTACHED_SECURITY_GROUPS).map(group).collect(),
            ),
        ];
        for (case, named, expected) in rows {
            let carried = security_group_ids(Some(&VpcConfig {
                subnet_ids: None,
                security_group_ids: named,
                ipv6_allowed_for_dual_stack: None,
            }));
            assert_eq!(
                carried, expected,
                "case {case:?}: expected {expected:?}, got {carried:?}"
            );
        }
    }

    /// The description is fixed text apart from the function's own name.
    #[test]
    fn the_description_names_the_service_and_the_function() {
        for function_name in ["orders", "a-function-with-a-longer-name"] {
            let expected = format!("AWS Lambda VPC ENI-{function_name}");
            let worded = interface_description(function_name);
            assert_eq!(
                worded, expected,
                "case {function_name:?}: expected {expected:?}, got {worded:?}"
            );
        }
    }
}
