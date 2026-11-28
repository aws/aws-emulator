// SPDX-License-Identifier: CC0-1.0

//! Tracing a Reachability Analyzer PATH: whether the destination is reachable from the source,
//! read off the security groups, the network ACLs and the routes the account actually holds.
//!
//! "Reachability Analyzer is a configuration analysis tool that enables you to perform
//! connectivity testing between a source resource and a destination resource in your virtual
//! private clouds" (Amazon VPC Reachability Analyzer guide): the verdict
//! `NetworkInsightsAnalysis$NetworkPathFound` is a function of the stored network, so it is
//! COMPUTED here from that state rather than reported as a result nothing produced. One
//! packet, described by the path (its protocol, its destination port and the two ends), is
//! taken through the components that can stop it, in the order a packet meets them:
//!
//! 1. the route the source subnet's route table has for the destination address — without one
//!    the packet is not forwarded at all;
//! 2. the OUTBOUND rules of the source's security groups, which are "stateful" and so decide
//!    the forward direction alone (Amazon VPC User Guide, "Security groups");
//! 3. the source subnet's network ACL in the outbound direction, which is "stateless" and
//!    evaluates its numbered rules "in order, starting with the lowest numbered rule" (same
//!    guide, "Network ACLs");
//! 4. the destination subnet's network ACL in the inbound direction, by the same rule;
//! 5. the INBOUND rules of the destination's security groups.
//!
//! The first component that admits nothing is what the analysis reports: the verdict is
//! `false` and the explanation names that component, so a caller learns WHERE the packet
//! stopped. A path this emulator holds no two ends for — an end that is not an instance or a
//! network interface of this account, two ends in different VPCs, or a path scoped by a filter
//! at either end — is not traced at all ([`PathTrace`] is `None`), and the analysis keeps the
//! honest failure [`PATH_ANALYSIS_UNSUPPORTED_STATUS_MESSAGE`] states rather than a verdict
//! about components nothing walked.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;
use crate::ops::types::{
    AnalysisAclRule, AnalysisComponent, AnalysisRouteTableRoute, AnalysisSecurityGroupRule,
    Explanation, GroupIdentifier, NetworkAclEntry, NetworkInsightsPath, PathComponent, PortRange,
    Route,
};

/// The protocol number IPv4 carries TCP under (IANA "Protocol Numbers"), which is how a
/// network ACL entry names the protocol — "the protocol number. A value of `-1` means all
/// protocols" (`NetworkAclEntry$Protocol`) — while a security group rule names it by the word
/// [`IP_PROTOCOL_TCP`].
pub const PROTOCOL_NUMBER_TCP: i32 = 6;

/// The protocol number IPv4 carries UDP under (see [`PROTOCOL_NUMBER_TCP`]).
pub const PROTOCOL_NUMBER_UDP: i32 = 17;

/// The protocol number an ACL entry names the protocol `protocol` by, or `None` for a word
/// that is no protocol of the modelled `Protocol` catalog (`tcp`, `udp` —
/// `smithy_query(query="shape",shape="Protocol")`).
pub fn protocol_number(protocol: &str) -> Option<i32> {
    if protocol.eq_ignore_ascii_case(IP_PROTOCOL_TCP) {
        return Some(PROTOCOL_NUMBER_TCP);
    }
    if protocol.eq_ignore_ascii_case(IP_PROTOCOL_UDP) {
        return Some(PROTOCOL_NUMBER_UDP);
    }
    None
}

/// ONE END of a path, resolved against this store.
///
/// A path is analyzed BETWEEN resources, and what carries a packet at either end is a NETWORK
/// INTERFACE: it holds the address the packet leaves from or arrives at, the subnet whose route
/// table and network ACL apply to it, and the security groups whose rules do. An instance is
/// resolved to its own primary interface, which is where its address and groups live
/// ([`ComAmazonawsEc2Amazonec2State::instance_security_groups`]).
#[derive(Debug, Clone)]
pub struct PathEndpoint {
    /// The resource the path named, as the id the analysis reports it by.
    pub component_id: String,
    /// The network interface the packet leaves from or arrives at.
    pub network_interface_id: String,
    /// Its private IPv4 address.
    pub address: String,
    /// The subnet that interface sits in.
    pub subnet_id: String,
    /// The VPC that subnet belongs to.
    pub vpc_id: String,
    /// The security groups applied to that interface.
    pub security_groups: Vec<GroupIdentifier>,
}

/// What a trace of one path found.
#[derive(Debug, Clone)]
pub struct PathTrace {
    /// "Indicates whether the destination is reachable from the source"
    /// (`NetworkInsightsAnalysis$NetworkPathFound`).
    pub network_path_found: bool,
    /// "The components in the path from source to destination"
    /// (`$ForwardPathComponents`): the hops this trace walked, in the order the packet meets
    /// them, with the explanation of a packet that stopped attached to the hop it stopped at.
    pub forward_path_components: Vec<PathComponent>,
    /// "The explanations" (`$Explanations`) — empty for a packet that got through.
    ///
    /// Each one names the COMPONENT that stopped the packet, the packet's own fields and the
    /// code the published catalog gives that cause ([`EXPLANATION_ENI_SG_RULES_MISMATCH`] and
    /// the two beside it), which is what a client reads to find the misconfiguration.
    pub explanations: Vec<Explanation>,
}

/// Which direction of a component's rules a packet is being read against, as
/// `Explanation$Direction` and `AnalysisSecurityGroupRule$Direction` name it.
const DIRECTION_EGRESS: &str = "egress";

/// The ingress direction (see [`DIRECTION_EGRESS`]).
const DIRECTION_INGRESS: &str = "ingress";

/// `Explanation$ExplanationCode` for a packet no SECURITY GROUP rule of the network interface
/// admits — "the explanations. For more information, see Reachability Analyzer explanation
/// codes" (`NetworkInsightsAnalysis$Explanations`), whose published catalog names this one
/// `ENI_SG_RULES_MISMATCH`: the rules of the interface's groups and the packet do not meet.
pub const EXPLANATION_ENI_SG_RULES_MISMATCH: &str = "ENI_SG_RULES_MISMATCH";

/// `Explanation$ExplanationCode` for a packet a NETWORK ACL entry denies, which that same
/// published catalog names `ACL_RULE`: the entry that matched first is the one reported beside
/// it.
pub const EXPLANATION_ACL_RULE: &str = "ACL_RULE";

/// `Explanation$ExplanationCode` for a packet the source subnet's route table forwards nowhere,
/// which that catalog names `NO_ROUTE_TO_DESTINATION`.
pub const EXPLANATION_NO_ROUTE_TO_DESTINATION: &str = "NO_ROUTE_TO_DESTINATION";

/// What one network ACL decided about a packet.
struct AclVerdict {
    /// The ACL that decided.
    acl_id: String,
    /// The numbered entry that matched first — `None` when the ACL holds no entry matching the
    /// packet at all, which admits nothing either.
    entry: Option<NetworkAclEntry>,
    /// Whether that entry admits the packet.
    admits: bool,
}

impl ComAmazonawsEc2Amazonec2State {
    /// The end of a path the value `named` names, resolved against this store — `None` for a
    /// value naming no instance or network interface of this account.
    ///
    /// "If the resource is in another account, you must specify an ARN"
    /// (`CreateNetworkInsightsPathRequest$Source`), so the value is read through the crate's one
    /// reader of a member that admits either form ([`arn_or_bare_resource_id`]) — the same
    /// reading `crate::ops::create_network_insights_path` validates a path's ends with.
    pub fn resolve_path_endpoint(&self, named: &str) -> Option<PathEndpoint> {
        let id = arn_or_bare_resource_id(named);
        if let Some(interface) = self.network_interfaces.get(id) {
            return Some(PathEndpoint {
                component_id: id.to_string(),
                network_interface_id: id.to_string(),
                address: interface.private_ip_address.clone()?,
                subnet_id: interface.subnet_id.clone()?,
                vpc_id: interface.vpc_id.clone()?,
                security_groups: interface.groups.clone().unwrap_or_default(),
            });
        }
        let instance = self.instances.get(id)?;
        let interfaces = self.instance_network_interfaces(id);
        let network_interface_id = interfaces
            .first()
            .and_then(|interface| interface.network_interface_id.clone())?;
        Some(PathEndpoint {
            component_id: id.to_string(),
            network_interface_id,
            address: instance.private_ip.clone(),
            subnet_id: self.instance_subnet_id_of(instance, &interfaces)?,
            vpc_id: self.instance_vpc_id_of(instance, &interfaces)?,
            security_groups: self.instance_security_groups_of(instance, &interfaces),
        })
    }

    /// Trace the path `path_id` names, or `None` when the store holds no such path or this
    /// emulator cannot trace it.
    ///
    /// The entry point the clock uses when a running analysis reaches its end: the Region the
    /// prefix lists of the trace are read from is the path's own, which its ARN names.
    pub fn trace_stored_network_insights_path(&self, path_id: &str) -> Option<PathTrace> {
        let path = self.network_insights_paths.get(path_id)?;
        let region = path_region(path.network_insights_path_arn.as_deref());
        self.trace_network_insights_path(path, &region)
    }

    /// Trace the path `path` and answer what the stored network does to its packet, or `None`
    /// when this emulator cannot trace it (the module docs say which paths those are).
    ///
    /// A READ: every component is read and nothing is written, so two traces of one path over
    /// an unchanged network answer the same verdict. `region` is the Region the analysis was
    /// started in, which is where a prefix list a rule names is read from.
    pub fn trace_network_insights_path(
        &self,
        path: &NetworkInsightsPath,
        region: &str,
    ) -> Option<PathTrace> {
        // A path scoped by a filter analyzes a packet those filters describe rather than the
        // one the path's own members do, and this trace reads the path's members: such a path
        // is left untraced rather than answered with a verdict about a different packet.
        if path.filter_at_source.is_some() || path.filter_at_destination.is_some() {
            return None;
        }
        let source = self.resolve_path_endpoint(path.source.as_deref()?)?;
        let destination = self.resolve_path_endpoint(path.destination.as_deref()?)?;
        // Two ends in different VPCs are reached through a peering connection, a transit
        // gateway or a gateway of their own, none of which this trace walks.
        if source.vpc_id != destination.vpc_id {
            return None;
        }
        let protocol = path.protocol.as_deref()?;
        protocol_number(protocol)?;
        // "The IP address of the source" / "of the destination"
        // (`NetworkInsightsPath$SourceIp` / `$DestinationIp`): the address the packet carries
        // where the path names one, and the end's own address otherwise.
        let source_address = path
            .source_ip
            .clone()
            .filter(|address| !address.is_empty())
            .unwrap_or_else(|| source.address.clone());
        let destination_address = path
            .destination_ip
            .clone()
            .filter(|address| !address.is_empty())
            .unwrap_or_else(|| destination.address.clone());
        // "The destination port" (`$DestinationPort`): a path that names none analyzes every
        // port, so any rule on the protocol admits it.
        let port = path.destination_port;

        let mut components: Vec<PathComponent> = Vec::new();
        let mut explanations: Vec<Explanation> = Vec::new();

        // 1. The source interface itself, and the route its subnet has for the destination.
        components.push(self.endpoint_component(1, &source));
        let route = self.route_to_address(&source.subnet_id, &source.vpc_id, &destination_address);
        match route {
            Some((route_table_id, route)) => components.push(PathComponent {
                sequence_number: Some(2),
                component: Some(AnalysisComponent {
                    id: Some(route_table_id.clone()),
                    arn: None,
                    name: None,
                }),
                route_table_route: Some(analysis_route(&route)),
                ..PathComponent::default()
            }),
            None => {
                // Nothing forwards the packet out of the source subnet.
                let explanation = Explanation {
                    explanation_code: Some(EXPLANATION_NO_ROUTE_TO_DESTINATION.to_string()),
                    subnet: Some(component_of(&source.subnet_id)),
                    subnet_route_table: self
                        .subnet_route_table(&source.subnet_id, &source.vpc_id)
                        .map(|route_table_id| component_of(&route_table_id)),
                    vpc: Some(component_of(&source.vpc_id)),
                    address: Some(destination_address.clone()),
                    ..Explanation::default()
                };
                explanations.push(explanation.clone());
                components.push(PathComponent {
                    sequence_number: Some(2),
                    explanations: Some(vec![explanation]),
                    ..PathComponent::default()
                });
                return Some(PathTrace {
                    network_path_found: false,
                    forward_path_components: components,
                    explanations,
                });
            }
        }

        // 2. The source's own groups, outbound: "a security group has an outbound rule that
        //    allows all outbound traffic" unless the caller narrowed it.
        if self
            .security_groups_admit(
                &source.security_groups,
                true,
                &destination_address,
                &destination.security_groups,
                protocol,
                port,
                region,
            )
            .is_none()
        {
            let explanation = security_group_explanation(
                &source,
                DIRECTION_EGRESS,
                &destination_address,
                protocol,
                port,
            );
            explanations.push(explanation.clone());
            components.push(PathComponent {
                sequence_number: Some(3),
                explanations: Some(vec![explanation]),
                ..PathComponent::default()
            });
            return Some(PathTrace {
                network_path_found: false,
                forward_path_components: components,
                explanations,
            });
        }

        // 3. and 4. The two subnets' network ACLs, each in the direction the packet crosses it.
        for (sequence_number, subnet_id, egress, peer_address) in [
            (
                4,
                source.subnet_id.as_str(),
                true,
                destination_address.as_str(),
            ),
            (
                5,
                destination.subnet_id.as_str(),
                false,
                source_address.as_str(),
            ),
        ] {
            let Some(verdict) = self.network_acl_verdict(
                subnet_id,
                &source.vpc_id,
                egress,
                peer_address,
                protocol,
                port,
            ) else {
                continue;
            };
            if verdict.admits {
                continue;
            }
            let explanation = Explanation {
                explanation_code: Some(EXPLANATION_ACL_RULE.to_string()),
                acl: Some(component_of(&verdict.acl_id)),
                acl_rule: verdict.entry.as_ref().map(analysis_acl_rule),
                subnet: Some(component_of(subnet_id)),
                vpc: Some(component_of(&source.vpc_id)),
                direction: Some(
                    if egress {
                        DIRECTION_EGRESS
                    } else {
                        DIRECTION_INGRESS
                    }
                    .to_string(),
                ),
                address: Some(peer_address.to_string()),
                port,
                protocols: Some(vec![protocol.to_string()]),
                ..Explanation::default()
            };
            explanations.push(explanation.clone());
            components.push(PathComponent {
                sequence_number: Some(sequence_number),
                acl_rule: verdict.entry.as_ref().map(analysis_acl_rule),
                explanations: Some(vec![explanation]),
                ..PathComponent::default()
            });
            return Some(PathTrace {
                network_path_found: false,
                forward_path_components: components,
                explanations,
            });
        }

        // 5. The destination's own groups, inbound — the rule a caller authorizes to let the
        //    packet in.
        let admitted = self.security_groups_admit(
            &destination.security_groups,
            false,
            &source_address,
            &source.security_groups,
            protocol,
            port,
            region,
        );
        let mut destination_component = self.endpoint_component(6, &destination);
        match admitted {
            Some((group_id, atom)) => {
                destination_component.security_group_rule = Some(analysis_security_group_rule(
                    &group_id,
                    DIRECTION_INGRESS,
                    &atom,
                ));
                components.push(destination_component);
                Some(PathTrace {
                    network_path_found: true,
                    forward_path_components: components,
                    explanations,
                })
            }
            None => {
                let explanation = security_group_explanation(
                    &destination,
                    DIRECTION_INGRESS,
                    &source_address,
                    protocol,
                    port,
                );
                explanations.push(explanation.clone());
                destination_component.explanations = Some(vec![explanation]);
                components.push(destination_component);
                Some(PathTrace {
                    network_path_found: false,
                    forward_path_components: components,
                    explanations,
                })
            }
        }
    }

    /// The route table the subnet `subnet_id` routes through: the one it is explicitly
    /// associated with, or the main table of its VPC — "each subnet in your VPC must be
    /// associated with a route table […] if a subnet is not explicitly associated with a route
    /// table, it is implicitly associated with the main route table" (Amazon VPC User Guide,
    /// "Route tables").
    pub fn subnet_route_table(&self, subnet_id: &str, vpc_id: &str) -> Option<String> {
        self.route_table_associated_with_subnet(subnet_id)
            .or_else(|| self.main_route_table_of_vpc(vpc_id))
    }

    /// The route the subnet `subnet_id` forwards `address` through, with the table it is in.
    ///
    /// "When determining how to route traffic, we use the route with the most specific match"
    /// (`CreateRoute`), so the matching route with the LONGEST prefix wins; a route whose
    /// target is not available carries nothing ([`ROUTE_STATE_BLACKHOLE`]) and is not one —
    /// asked of the target the one way a report of the route asks it
    /// ([`Self::route_carries_traffic`]), so the route a packet follows and the state a
    /// describe reports cannot disagree.
    pub fn route_to_address(
        &self,
        subnet_id: &str,
        vpc_id: &str,
        address: &str,
    ) -> Option<(String, Route)> {
        let route_table_id = self.subnet_route_table(subnet_id, vpc_id)?;
        let route_table = self.route_tables.get(&route_table_id)?;
        route_table
            .routes
            .iter()
            .flatten()
            .filter(|route| self.route_carries_traffic(route_table.vpc_id.as_deref(), route))
            .filter_map(|route| {
                let block = route.destination_cidr_block.as_deref()?;
                if !cidr_block_contains_address(block, address) {
                    return None;
                }
                let prefix_length = block.split_once('/')?.1.parse::<u32>().ok()?;
                Some((prefix_length, route))
            })
            .max_by_key(|(prefix_length, _)| *prefix_length)
            .map(|(_, route)| (route_table_id.clone(), route.clone()))
    }

    /// The first rule of `groups` that admits the packet, as the group it is in and the rule
    /// itself — `None` when no rule of any of them does.
    ///
    /// `is_egress` picks the direction of the rules read, `peer_address` is the address at the
    /// OTHER end of the packet (where it is going for an outbound rule, where it comes from for
    /// an inbound one) and `peer_groups` the groups of that end, which is what a rule naming
    /// another security group as its source matches on.
    #[allow(clippy::too_many_arguments)]
    pub fn security_groups_admit(
        &self,
        groups: &[GroupIdentifier],
        is_egress: bool,
        peer_address: &str,
        peer_groups: &[GroupIdentifier],
        protocol: &str,
        port: Option<i32>,
        region: &str,
    ) -> Option<(String, SecurityGroupRuleAtom)> {
        for group in groups {
            let Some(group_id) = group.group_id.as_deref() else {
                continue;
            };
            for (_, atom) in self.held_security_group_rules(group_id, is_egress) {
                if self.rule_admits(&atom, peer_address, peer_groups, protocol, port, region) {
                    return Some((group_id.to_string(), atom));
                }
            }
        }
        None
    }

    /// Does the one security group rule `atom` admit the packet?
    ///
    /// Each of the three things a rule states is read in turn: the protocol — "specifying `-1`
    /// […] allows traffic on all ports" (`IpPermission$IpProtocol`) — the port range, for the
    /// protocols that have one, and the rule's single source
    /// ([`SecurityGroupRuleSource`]), which is an address range, a prefix list the Region
    /// publishes or holds, or another security group.
    fn rule_admits(
        &self,
        atom: &SecurityGroupRuleAtom,
        peer_address: &str,
        peer_groups: &[GroupIdentifier],
        protocol: &str,
        port: Option<i32>,
        region: &str,
    ) -> bool {
        if atom.ip_protocol != ALL_IP_PROTOCOLS && !atom.ip_protocol.eq_ignore_ascii_case(protocol) {
            return false;
        }
        if let Some(port) = port {
            if protocol_admits_port_range(&atom.ip_protocol) {
                let from = atom.from_port.unwrap_or(ALL_PORTS);
                let to = atom.to_port.unwrap_or(ALL_PORTS);
                if from != ALL_PORTS && !(from..=to).contains(&port) {
                    return false;
                }
            }
        }
        match &atom.source {
            SecurityGroupRuleSource::Ipv4(cidr) => cidr_block_contains_address(cidr, peer_address),
            // The packet this trace follows carries an IPv4 address, which is in no IPv6 range.
            SecurityGroupRuleSource::Ipv6(_) => false,
            SecurityGroupRuleSource::PrefixList(prefix_list_id) => self
                .visible_prefix_list_entries(region, prefix_list_id)
                .is_some_and(|view| {
                    view.entries.iter().any(|entry| {
                        entry
                            .cidr
                            .as_deref()
                            .is_some_and(|cidr| cidr_block_contains_address(cidr, peer_address))
                    })
                }),
            // "The ID of the security group" a rule names as its source
            // (`UserIdGroupPair$GroupId`), or the name in a default VPC: the packet matches
            // when the OTHER end is in that group, however the rule spells it.
            SecurityGroupRuleSource::ReferencedGroup(named) => peer_groups.iter().any(|group| {
                group.group_id.as_deref() == Some(named.as_str())
                    || group.group_name.as_deref() == Some(named.as_str())
            }),
        }
    }

    /// What the network ACL of the subnet `subnet_id` decides about the packet, or `None` when
    /// this store holds no ACL for that subnet — nothing then filters it.
    ///
    /// "Network ACLs are stateless […] we evaluate the rules in order, starting with the lowest
    /// numbered rule" and "as soon as a rule matches traffic, it's applied regardless of any
    /// higher-numbered rule that might contradict it" (Amazon VPC User Guide, "Network ACLs"),
    /// so the FIRST matching entry in rule-number order decides, in the one direction the
    /// packet crosses the ACL.
    fn network_acl_verdict(
        &self,
        subnet_id: &str,
        vpc_id: &str,
        egress: bool,
        peer_address: &str,
        protocol: &str,
        port: Option<i32>,
    ) -> Option<AclVerdict> {
        let (acl_id, acl) = self
            .network_acls
            .iter()
            .find(|(_, acl)| {
                acl.associations
                    .iter()
                    .flatten()
                    .any(|association| association.subnet_id.as_deref() == Some(subnet_id))
            })
            .or_else(|| {
                self.network_acls.iter().find(|(_, acl)| {
                    acl.vpc_id.as_deref() == Some(vpc_id) && acl.is_default == Some(true)
                })
            })?;
        let mut entries: Vec<&NetworkAclEntry> = acl
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.egress.unwrap_or(false) == egress)
            .collect();
        entries.sort_by_key(|entry| entry.rule_number.unwrap_or(i32::MAX));
        let matched = entries
            .into_iter()
            .find(|entry| acl_entry_matches(entry, peer_address, protocol, port));
        Some(AclVerdict {
            acl_id: acl_id.clone(),
            entry: matched.cloned(),
            admits: matched
                .is_some_and(|entry| entry.rule_action.as_deref() == Some(RULE_ACTION_ALLOW)),
        })
    }

    /// One end of the path as a hop of the forward path: the interface the packet is at, the
    /// subnet it sits in and that subnet's VPC.
    fn endpoint_component(&self, sequence_number: i32, endpoint: &PathEndpoint) -> PathComponent {
        PathComponent {
            sequence_number: Some(sequence_number),
            component: Some(component_of(&endpoint.component_id)),
            attached_to: Some(component_of(&endpoint.network_interface_id)),
            subnet: Some(component_of(&endpoint.subnet_id)),
            vpc: Some(component_of(&endpoint.vpc_id)),
            ..PathComponent::default()
        }
    }
}

/// Does the network ACL entry `entry` match the packet?
///
/// "The protocol number. A value of `-1` means all protocols"
/// (`NetworkAclEntry$Protocol`), and an entry on a protocol with ports matches only inside the
/// range it names ("the range of ports the rule applies to", `$PortRange`); the entry's CIDR
/// block has to hold the address at the other end of the packet.
fn acl_entry_matches(
    entry: &NetworkAclEntry,
    peer_address: &str,
    protocol: &str,
    port: Option<i32>,
) -> bool {
    let entry_protocol = entry.protocol.as_deref().unwrap_or(ALL_IP_PROTOCOLS);
    if entry_protocol != ALL_IP_PROTOCOLS && Some(entry_protocol) != protocol_number(protocol).map(|number| number.to_string()).as_deref() {
        return false;
    }
    if let Some(port) = port {
        if let Some(range) = entry.port_range.as_ref() {
            let from = range.from.unwrap_or(0);
            let to = range.to.unwrap_or(MAX_PORT);
            if !(from..=to).contains(&port) {
                return false;
            }
        }
    }
    entry
        .cidr_block
        .as_deref()
        .is_some_and(|block| cidr_block_contains_address(block, peer_address))
}

/// The resource `id` as the [`AnalysisComponent`] a path component or an explanation names it
/// by: the id alone, every component this trace walks being a resource of this account whose
/// ARN the stored record does not carry.
fn component_of(id: &str) -> AnalysisComponent {
    AnalysisComponent {
        id: Some(id.to_string()),
        arn: None,
        name: None,
    }
}

/// The explanation of a packet no security group rule of `endpoint` admits: the groups that
/// were read, the direction they were read in, and the packet's own fields.
fn security_group_explanation(
    endpoint: &PathEndpoint,
    direction: &str,
    peer_address: &str,
    protocol: &str,
    port: Option<i32>,
) -> Explanation {
    Explanation {
        explanation_code: Some(EXPLANATION_ENI_SG_RULES_MISMATCH.to_string()),
        network_interface: Some(component_of(&endpoint.network_interface_id)),
        security_groups: Some(
            endpoint
                .security_groups
                .iter()
                .filter_map(|group| group.group_id.as_deref())
                .map(component_of)
                .collect(),
        ),
        subnet: Some(component_of(&endpoint.subnet_id)),
        vpc: Some(component_of(&endpoint.vpc_id)),
        direction: Some(direction.to_string()),
        address: Some(peer_address.to_string()),
        port,
        protocols: Some(vec![protocol.to_string()]),
        ..Explanation::default()
    }
}

/// The stored route `route` as the [`AnalysisRouteTableRoute`] a path component reports it by.
fn analysis_route(route: &Route) -> AnalysisRouteTableRoute {
    AnalysisRouteTableRoute {
        destination_cidr: route.destination_cidr_block.clone(),
        destination_prefix_list_id: route.destination_prefix_list_id.clone(),
        egress_only_internet_gateway_id: route.egress_only_internet_gateway_id.clone(),
        gateway_id: route.gateway_id.clone(),
        instance_id: route.instance_id.clone(),
        nat_gateway_id: route.nat_gateway_id.clone(),
        network_interface_id: route.network_interface_id.clone(),
        origin: route.origin.clone(),
        transit_gateway_id: route.transit_gateway_id.clone(),
        vpc_peering_connection_id: route.vpc_peering_connection_id.clone(),
        state: route.state.clone(),
        carrier_gateway_id: route.carrier_gateway_id.clone(),
        core_network_arn: route.core_network_arn.clone(),
        local_gateway_id: route.local_gateway_id.clone(),
    }
}

/// The ACL entry `entry` as the [`AnalysisAclRule`] an explanation reports it by.
fn analysis_acl_rule(entry: &NetworkAclEntry) -> AnalysisAclRule {
    AnalysisAclRule {
        cidr: entry.cidr_block.clone(),
        egress: entry.egress,
        port_range: entry.port_range.clone(),
        protocol: entry.protocol.clone(),
        rule_action: entry.rule_action.clone(),
        rule_number: entry.rule_number,
    }
}

/// The security group rule `atom` as the [`AnalysisSecurityGroupRule`] a path component reports
/// the rule that admitted the packet by.
fn analysis_security_group_rule(
    group_id: &str,
    direction: &str,
    atom: &SecurityGroupRuleAtom,
) -> AnalysisSecurityGroupRule {
    let (cidr, prefix_list_id) = match &atom.source {
        SecurityGroupRuleSource::Ipv4(cidr) | SecurityGroupRuleSource::Ipv6(cidr) => {
            (Some(cidr.clone()), None)
        }
        SecurityGroupRuleSource::PrefixList(prefix_list_id) => {
            (None, Some(prefix_list_id.clone()))
        }
        SecurityGroupRuleSource::ReferencedGroup(_) => (None, None),
    };
    AnalysisSecurityGroupRule {
        cidr,
        direction: Some(direction.to_string()),
        security_group_id: Some(group_id.to_string()),
        port_range: atom.from_port.zip(atom.to_port).map(|(from, to)| PortRange {
            from: Some(from),
            to: Some(to),
        }),
        prefix_list_id,
        protocol: Some(atom.ip_protocol.clone()),
    }
}

/// The Region the ARN `arn` names, or [`DEFAULT_REGION`] for a value that names none.
///
/// Read through the crate's one ARN reader ([`arn_region`]), so the field this takes is the
/// field the published format gives it; a stored path's ARN names the Region its create was
/// signed for, and so does a stored analysis's
/// ([`super::access_scope_analysis`] reads it for the same reason).
pub(super) fn path_region(arn: Option<&str>) -> String {
    arn.and_then(arn_region)
        .unwrap_or(DEFAULT_REGION)
        .to_string()
}
