// SPDX-License-Identifier: CC0-1.0

//! The NETWORK a MicroVM's guest reaches: which outbound connection its
//! `egressNetworkConnectors` carry, and what the guest observes of the one they do.
//!
//! `RunMicrovmRequest$egressNetworkConnectors` is "the list of egress network connectors
//! to configure for the MicroVM", and a lambda-core connector carrying a
//! `NetworkConnectorVpcEgressConfiguration` — "Configuration for a VPC egress network
//! connector. Specifies the VPC subnets, security groups, network protocol, and
//! associated Lambda compute resource types" — is what gives a MicroVM a path into the
//! customer VPC. So the connectors a MicroVM is attached to are what DECIDE an outbound
//! connection its guest opens, and this module is that decision:
//!
//! * the destination must be an address of the account's own network — an EC2 network
//!   interface in a VPC one of the attached connectors places the MicroVM in — because a
//!   connector's path leads into that VPC and nowhere else;
//! * the connector carrying it must be `ACTIVE`: a connector that is not provisioned
//!   carries nothing;
//! * the destination's SECURITY GROUPS must admit the port from the connector's subnet
//!   ("the security groups that control traffic" is what the configuration's
//!   `SecurityGroupIds` are about, and an ingress rule is what admits a connection at the
//!   other end);
//! * every other destination is UNREACHED: nothing answers it, so the connection the
//!   guest opened never completes and its own timeout is what ends it. An address this
//!   emulator holds no resource at — a public address, an address of a VPC no attached
//!   connector reaches — is in that class, and NO host socket is opened for any of them:
//!   this module decides reachability from the emulated network and never connects out of
//!   the process.
//!
//! What the two owners of that network are ASKED, rather than answered here:
//!
//! * Amazon EC2 owns the subnets, the endpoint network interfaces and the security
//!   groups, and it is asked for them with its own reads (`DescribeSubnets`,
//!   `DescribeNetworkInterfaces`, `DescribeSecurityGroups`) over the in-process callback
//!   ([`::smithy_server_runtime::OperationContext::callback`]), which is the contract
//!   `service_common::network` states: no second answer to "which subnets does the
//!   account have" is held here, and no host name is resolved for any of it;
//! * lambda-core owns the connectors and publishes no SDK client, so its own records are
//!   read through the shared engine both services live in.
//!
//! A hop that could not be made at all (no EC2 mounted to ask, no connector store) shows
//! nothing about the destination, so the connection is unreached — never reported as
//! established on evidence this emulator does not have.
//!
//! The SOURCE address the guest sees on a carried connection is link-local
//! ([`LINK_LOCAL_PREFIX`]): the address the platform gives the guest's own interface,
//! assigned to the MicroVM when it starts ([`link_local_address`]), rather than an
//! address inside the customer's subnet — the connector's interfaces are what hold those.

use std::net::Ipv4Addr;
use std::time::SystemTime;

use ::ipnet::Ipv4Net;

/// The range a MicroVM's own interface address is assigned from — "link-local" being the
/// 169.254.0.0/16 block of RFC 3927, which is what a guest observes as the local address
/// of a connection carried by a connector.
pub const LINK_LOCAL_PREFIX: &str = "169.254.0.0/16";

/// The one link-local address of that block no interface is given: the address the
/// instance metadata service answers on, which AWS reserves.
const RESERVED_LINK_LOCAL: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// The first port of the IANA dynamic (ephemeral) range, which a guest's own outbound
/// connection takes its local port from.
const FIRST_EPHEMERAL_PORT: u16 = 49_152;

/// The last port of that range.
const LAST_EPHEMERAL_PORT: u16 = 65_535;

/// The `ipProtocol` of a security-group rule that admits every protocol, as EC2 spells
/// it: "Use -1 to specify all protocols".
const ALL_PROTOCOLS: &str = "-1";

/// The `ipProtocol` of a rule that admits TCP, which is what a connection a guest opens
/// is.
const TCP: &str = "tcp";

/// How many of a destination's security groups are read for one decision, so a
/// destination carrying many groups cannot make one connect an unbounded read.
const MAX_GROUPS_READ: usize = 32;

/// The lambda-core store the connectors a MicroVM names live in: that service's own
/// state struct, in the shared engine.
pub type ConnectorState =
    ::service_com_amazonaws_lambdacore_lambdacoreapiservice::state::ComAmazonawsLambdacoreLambdacoreapiserviceState;

/// The lambda-core store in the SAME shared engine, or `None` when the engine carries
/// none — a deployment with no connectors to ask about, which shows nothing about any
/// path.
pub fn connector_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ConnectorState>> {
    ::service_com_amazonaws_lambdacore_lambdacoreapiservice::state::service_state(engine)
}

/// What one outbound connection a guest opened came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectVerdict {
    /// Whether the connection was carried to the destination and established.
    pub connected: bool,
    /// The local address of an established connection: the MicroVM's own link-local
    /// address. `None` for a connection that never completed, which has no local end.
    pub local_address: Option<String>,
    /// The local port of an established connection, from the ephemeral range.
    pub local_port: Option<u16>,
}

impl ConnectVerdict {
    /// A connection nothing carried, so it never completed.
    pub fn unreached() -> Self {
        Self {
            connected: false,
            local_address: None,
            local_port: None,
        }
    }
}

/// The egress a MicroVM's guest opens its connections through: everything about the
/// MicroVM the decision reads, copied off its record so no store lock is held for it.
#[derive(Debug, Clone)]
pub struct GuestEgress {
    /// The region the MicroVM runs in, which its connectors and network belong to.
    pub region: String,
    /// The `egressNetworkConnectors` the MicroVM was run with, in the order it named
    /// them.
    pub connectors: Vec<String>,
    /// The MicroVM's own link-local address, which an established connection's local end
    /// carries.
    pub link_local_address: String,
}

/// The VPC egress one ACTIVE connector provides: where it places the MicroVM.
#[derive(Debug, Clone, Default)]
struct VpcEgress {
    /// The subnets the connector's interfaces live in.
    subnet_ids: Vec<String>,
    /// The security groups those interfaces carry, which a destination's ingress rule may
    /// admit by name.
    security_group_ids: Vec<String>,
}

/// The link-local address assigned to the MicroVM whose own sequence is `seq`.
///
/// One address of [`LINK_LOCAL_PREFIX`] per MicroVM of the store: the sequence indexes the
/// host parts that ARE interface addresses (both octets in `1..=254`, so neither the
/// network nor the broadcast part of an octet) with the one address AWS reserves
/// ([`RESERVED_LINK_LOCAL`]) left out of the indexing altogether — so two MicroVMs take
/// the same address only once the whole block has been handed out.
#[must_use]
pub fn link_local_address(seq: u64) -> String {
    // 254 usable values per octet (1..=254), less the one reserved address: that is how
    // many MicroVMs take a distinct address before the assignment wraps.
    const HOSTS: u64 = 254 * 254;
    const ASSIGNABLE: u64 = HOSTS - 1;
    let index = seq % ASSIGNABLE;
    // The reserved host part is not an index: every index at or past it names the host
    // part one above, which is what leaves the reserved address unassigned without
    // aliasing two indices onto one address.
    let host = if index >= reserved_host_part() {
        index + 1
    } else {
        index
    };
    host_address(host).to_string()
}

/// The host part of [`LINK_LOCAL_PREFIX`] that [`RESERVED_LINK_LOCAL`] occupies, as
/// [`host_address`] indexes them — read off that address, so the two cannot disagree.
fn reserved_host_part() -> u64 {
    let octets = RESERVED_LINK_LOCAL.octets();
    u64::from(octets[2].saturating_sub(1)) * 254 + u64::from(octets[3].saturating_sub(1))
}

/// The address the host part `host` of [`LINK_LOCAL_PREFIX`] spells, with both octets in
/// the `1..=254` range an interface address carries.
fn host_address(host: u64) -> Ipv4Addr {
    let high = u8::try_from(1 + host / 254).unwrap_or(1);
    let low = u8::try_from(1 + host % 254).unwrap_or(1);
    Ipv4Addr::new(169, 254, high, low)
}

/// The local port one established connection takes.
///
/// From the IANA dynamic range, assigned per connection by a counter of this process:
/// the port is the local end of a socket rather than anything a caller names, so it needs
/// to differ between connections and nothing more.
fn ephemeral_port() -> u16 {
    static NEXT: ::std::sync::atomic::AtomicU32 = ::std::sync::atomic::AtomicU32::new(0);
    let span = u32::from(LAST_EPHEMERAL_PORT - FIRST_EPHEMERAL_PORT) + 1;
    let taken = NEXT.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed) % span;
    FIRST_EPHEMERAL_PORT.saturating_add(u16::try_from(taken).unwrap_or_default())
}

/// What a connection the guest opened to `host`:`port` comes to.
///
/// A READ of the network: it changes nothing of EC2's or lambda-core's records beyond the
/// transitions their own clocks already owed.
pub async fn resolve(
    connectors: Option<&::engine::ServiceState<ConnectorState>>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    egress: &GuestEgress,
    host: &str,
    port: i32,
) -> ConnectVerdict {
    // The destination is an ADDRESS: this emulator resolves no names, so a host that is
    // not one addresses nothing it holds.
    let Ok(destination) = host.parse::<Ipv4Addr>() else {
        return ConnectVerdict::unreached();
    };
    let Ok(port) = u16::try_from(port) else {
        return ConnectVerdict::unreached();
    };
    let attached = active_vpc_egress(connectors, egress);
    if attached.is_empty() {
        return ConnectVerdict::unreached();
    }

    let client = ec2_client(callback, &egress.region);
    // Where the connectors place the MicroVM: the VPC of each of their subnets, with the
    // subnet's own range, which is the source range a destination's rule admits.
    let mut reachable: Vec<(String, Ipv4Net, Vec<String>)> = Vec::new();
    for connector in &attached {
        for subnet in describe_subnets(&client, &connector.subnet_ids).await {
            let (Some(vpc), Some(cidr)) = (subnet.vpc_id.clone(), subnet.cidr_block.as_deref())
            else {
                continue;
            };
            let Ok(range) = cidr.parse::<Ipv4Net>() else {
                continue;
            };
            reachable.push((vpc, range, connector.security_group_ids.clone()));
        }
    }

    // The destination's own interface, in whichever of those VPCs holds it.
    let interfaces = describe_interfaces(&client, &destination.to_string()).await;
    for interface in &interfaces {
        let Some(vpc) = interface.vpc_id.as_deref() else {
            continue;
        };
        let Some((_, source_range, source_groups)) =
            reachable.iter().find(|(reached, _, _)| reached == vpc)
        else {
            continue;
        };
        let groups: Vec<String> = interface
            .groups()
            .iter()
            .filter_map(|group| group.group_id.clone())
            .take(MAX_GROUPS_READ)
            .collect();
        if !admits(&client, &groups, port, *source_range, source_groups).await {
            // The path leads to the destination and its own groups drop the connection,
            // which the guest observes exactly as an address nothing answers at.
            continue;
        }
        return ConnectVerdict {
            connected: true,
            local_address: Some(egress.link_local_address.clone()),
            local_port: Some(ephemeral_port()),
        };
    }
    ConnectVerdict::unreached()
}

/// The VPC egress of every ACTIVE connector the MicroVM is attached to, read from the
/// lambda-core store in the shared engine.
///
/// A connector that is not `ACTIVE`, one of another type, and an identifier that store
/// holds no connector for (an AWS-managed connector of lambda's own namespace, among
/// them) each provide no VPC egress and so carry nothing.
fn active_vpc_egress(
    connectors: Option<&::engine::ServiceState<ConnectorState>>,
    egress: &GuestEgress,
) -> Vec<VpcEgress> {
    let Some(state) = connectors else {
        ::tracing::warn!(
            "a MicroVM's egress connectors could not be resolved: this emulator carries no \
             lambda-core store to ask"
        );
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut store = state.lock();
    egress
        .connectors
        .iter()
        .filter_map(|identifier| {
            // Reading the connector materialises the transitions its own clock already
            // owed — the provisioning that makes it ACTIVE among them — which were due at
            // this instant whether or not anybody connected.
            let (connector, qualified) = store.settled_mut(identifier, now)?;
            if connector.state
                != ::service_com_amazonaws_lambdacore_lambdacoreapiservice::state::STATE_ACTIVE
            {
                return None;
            }
            let version = connector.version(qualified)?;
            let ::service_com_amazonaws_lambdacore_lambdacoreapiservice::ops::types::NetworkConnectorConfiguration::VpcEgressConfiguration(vpc) =
                &version.configuration;
            Some(VpcEgress {
                subnet_ids: vpc.subnet_ids.clone().unwrap_or_default(),
                security_group_ids: vpc.security_group_ids.clone().unwrap_or_default(),
            })
        })
        .collect()
}

/// Whether any of the destination's security groups ADMITS a connection on `port` from
/// `source_range`, or from a group the connector's interfaces carry.
///
/// An ingress rule admits it when it covers the protocol (TCP, or every protocol), covers
/// the port, and names a source the connection comes from: a range that contains the
/// connector's subnet, or one of the connector's own security groups.
async fn admits(
    client: &::aws_sdk_ec2::Client,
    groups: &[String],
    port: u16,
    source_range: Ipv4Net,
    source_groups: &[String],
) -> bool {
    if groups.is_empty() {
        return false;
    }
    let described = match client
        .describe_security_groups()
        .set_group_ids(Some(groups.to_vec()))
        .send()
        .await
    {
        Ok(described) => described,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the security groups of a MicroVM connection's destination could not be read"
            );
            return false;
        }
    };
    described.security_groups().iter().any(|group| {
        group.ip_permissions().iter().any(|rule| {
            covers_protocol(rule.ip_protocol())
                && covers_port(rule.from_port(), rule.to_port(), port)
                && names_source(rule, source_range, source_groups)
        })
    })
}

/// Whether a rule's protocol covers a TCP connection.
fn covers_protocol(protocol: Option<&str>) -> bool {
    protocol.is_some_and(|protocol| protocol == TCP || protocol == ALL_PROTOCOLS)
}

/// Whether a rule's port range covers `port` — a rule naming no range covering every
/// port, which is the form a rule for every protocol takes.
fn covers_port(from: Option<i32>, to: Option<i32>, port: u16) -> bool {
    match (from, to) {
        (Some(from), Some(to)) => {
            let port = i32::from(port);
            from <= port && port <= to
        }
        _ => true,
    }
}

/// Whether a rule names the source of a connection the connector carries: a range
/// containing the connector's subnet, or one of the connector's security groups.
fn names_source(
    rule: &::aws_sdk_ec2::types::IpPermission,
    source_range: Ipv4Net,
    source_groups: &[String],
) -> bool {
    let by_range = rule.ip_ranges().iter().any(|range| {
        range
            .cidr_ip()
            .and_then(|cidr| cidr.parse::<Ipv4Net>().ok())
            .is_some_and(|admitted| admitted.contains(&source_range))
    });
    let by_group = rule.user_id_group_pairs().iter().any(|pair| {
        pair.group_id()
            .is_some_and(|group| source_groups.iter().any(|carried| carried == group))
    });
    by_range || by_group
}

/// The subnets `subnet_ids` names, as EC2 answers for them.
async fn describe_subnets(
    client: &::aws_sdk_ec2::Client,
    subnet_ids: &[String],
) -> Vec<::aws_sdk_ec2::types::Subnet> {
    if subnet_ids.is_empty() {
        return Vec::new();
    }
    match client
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await
    {
        Ok(described) => described.subnets().to_vec(),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a MicroVM's egress connector names could not be read"
            );
            Vec::new()
        }
    }
}

/// The network interfaces holding the private address `address`, as EC2 answers for them.
///
/// The filter is EC2's own `private-ip-address`, "the private IPv4 address or addresses of
/// the network interface", so the answer is whichever interface of the account holds the
/// destination.
async fn describe_interfaces(
    client: &::aws_sdk_ec2::Client,
    address: &str,
) -> Vec<::aws_sdk_ec2::types::NetworkInterface> {
    let by_address = ::aws_sdk_ec2::types::Filter::builder()
        .name("private-ip-address")
        .values(address)
        .build();
    match client
        .describe_network_interfaces()
        .filters(by_address)
        .send()
        .await
    {
        Ok(described) => described.network_interfaces().to_vec(),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the destination of a MicroVM connection could not be resolved against EC2"
            );
            Vec::new()
        }
    }
}

/// The Amazon EC2 client this module asks, over the emulator's own in-process callback.
///
/// The `endpoint_url` is an ADDRESS the in-process router reads rather than a host any
/// resolver looks up, so this client reaches the emulator's own EC2 and nothing else.
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given.
fn ec2_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a MicroVM's own interface address is one of the link-local block, never the
    /// address AWS reserves in it, and two MicroVMs of one store never share one.
    #[test]
    fn every_assigned_address_is_a_usable_link_local_one() {
        let block: Ipv4Net = LINK_LOCAL_PREFIX
            .parse()
            .expect("the link-local prefix is a prefix");
        // The sequence values spanned include the ones on either side of the reserved
        // address and the ones that would land on an octet's network and broadcast parts,
        // and they stay inside one pass over the block so every one must differ.
        let rows: Vec<u64> = (0..600)
            .chain(reserved_host_part() - 2..reserved_host_part() + 2)
            .chain([254 * 254 - 2])
            .collect();
        let mut seen: Vec<String> = Vec::new();
        for seq in rows {
            let assigned = link_local_address(seq);
            let address: Ipv4Addr = assigned
                .parse()
                .unwrap_or_else(|error| panic!("case {seq}: {assigned:?} is an address: {error}"));
            assert!(
                block.contains(&address),
                "case {seq}: expected an address of {LINK_LOCAL_PREFIX}, got {address}"
            );
            assert_ne!(
                address, RESERVED_LINK_LOCAL,
                "case {seq}: the reserved link-local address is never assigned"
            );
            for octet in [address.octets()[2], address.octets()[3]] {
                assert!(
                    (1..=254).contains(&octet),
                    "case {seq}: expected an interface address, got {address}"
                );
            }
            seen.push(assigned);
        }
        let count = seen.len();
        let mut distinct = seen;
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            count,
            "expected a distinct address per MicroVM, got {} of {count}",
            distinct.len()
        );
    }

    /// One row of the ingress-rule table: the case, the rule's protocol, its port range,
    /// the port a connection asks for, and whether the rule admits it.
    type RuleRow<'a> = (&'a str, Option<&'a str>, Option<i32>, Option<i32>, u16, bool);

    /// RULE: a rule admits a connection only when it covers TCP and the port.
    #[test]
    fn a_rule_admits_only_the_protocol_and_port_it_covers() {
        // (case, protocol, from, to, the port asked for, admitted)
        let rows: &[RuleRow] = &[
            ("tcp on the exact port", Some("tcp"), Some(443), Some(443), 443, true),
            ("tcp over a range", Some("tcp"), Some(1), Some(1024), 443, true),
            ("tcp below the range", Some("tcp"), Some(8000), Some(9000), 443, false),
            ("every protocol and port", Some("-1"), None, None, 443, true),
            ("udp on the port", Some("udp"), Some(443), Some(443), 443, false),
            ("no protocol at all", None, Some(443), Some(443), 443, false),
            ("tcp at the range's low bound", Some("tcp"), Some(443), Some(8080), 443, true),
            ("tcp at the range's high bound", Some("tcp"), Some(80), Some(443), 443, true),
            ("tcp one past the high bound", Some("tcp"), Some(80), Some(442), 443, false),
        ];
        for (case, protocol, from, to, port, expected) in rows {
            let actual = covers_protocol(*protocol) && covers_port(*from, *to, *port);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected admitted={expected}, got {actual}"
            );
        }
    }
}
