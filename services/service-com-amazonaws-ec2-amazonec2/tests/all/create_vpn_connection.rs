// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateVpnConnection`, read back through
//! `DescribeVpnConnections`, driven over the wire the service actually speaks: `ec2Query`,
//! an `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. A CREATE MAKES THE CONNECTION THE REQUEST DESCRIBES: between the customer gateway it
//!    names and the ONE Amazon-side gateway it names, with the two tunnels a Site-to-Site VPN
//!    connection is brought up with — each carrying its own outside address, its own inside
//!    CIDR block out of the published range and its own pre-shared key, either the request's
//!    values or the service's — the connection-level options at the defaults their
//!    documentation publishes, one telemetry entry per tunnel, and a customer gateway
//!    configuration document built out of all of it. It is answered in the non-terminal
//!    `pending` state the clock then owes `available`, and every member of it reads back
//!    through `DescribeVpnConnections`.
//! 2. A VALUE OUTSIDE WHAT A MEMBER PUBLISHES IS REFUSED rather than defaulted or stored —
//!    each published range at its bounds and one past them, each published value set, the
//!    pre-shared key charset, an inside block outside the published range or reserved or
//!    already in use, and more tunnels than a connection has.
//! 3. THE VALUES THE SERVICE OWNS ARE NEVER HANDED OUT TWICE: no two tunnels of the account
//!    share an inside block or an outside address.
//! 4. A REPEAT ANSWERS THE CONNECTION IT ALREADY MADE: "this is an idempotent operation"
//!    (`CreateVpnConnection`), so a second identical request stores no second connection,
//!    while a request naming another pair of gateways makes a new one.
//! 5. A GATEWAY THE ACCOUNT DOES NOT HOLD IS REFUSED and nothing is stored — the recorded
//!    case this operation's issue was filed about is ONE row of that table.
//!
//! Every table row is a distinct input along what those rules generalise over: which members
//! the request names, which family the tunnels carry, which Amazon-side gateway kind the
//! connection terminates on, how many tunnels the request describes, each published bound and
//! one past it, and how many connections the account already holds.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;

use super::ec2_query::{error_code, error_message};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The outside address of the customer gateway device every connection here runs to, from the
/// documentation range `TEST-NET-3` (RFC 5737), so no row names a routable address.
const CUSTOMER_GATEWAY_ADDRESS: &str = "203.0.113.12";

/// The BGP ASN of that device, inside the range `CreateCustomerGatewayRequest$BgpAsn`
/// publishes.
const CUSTOMER_GATEWAY_BGP_ASN: &str = "65001";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The text of the first element named `name` anywhere in `body`.
fn first_text(case: &str, body: &str, name: &str) -> String {
    let parsed = document(case, body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .and_then(|node| node.text().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One create of another resource this suite needs, requiring success, answering the id it
/// reported under `id_member`.
async fn create_resource(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    id_member: &str,
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    first_text(case, &body, id_member)
}

/// A customer gateway of the account, as `CreateCustomerGateway` makes one.
async fn customer_gateway(case: &str, router: &MultiServiceRouter, ip_address: &str) -> String {
    create_resource(
        case,
        router,
        "CreateCustomerGateway",
        &[
            param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
            param("IpAddress", ip_address),
            param("BgpAsn", CUSTOMER_GATEWAY_BGP_ASN),
        ],
        "customerGatewayId",
    )
    .await
}

/// A virtual private gateway of the account, as `CreateVpnGateway` makes one.
async fn vpn_gateway(case: &str, router: &MultiServiceRouter) -> String {
    create_resource(
        case,
        router,
        "CreateVpnGateway",
        &[param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1)],
        "vpnGatewayId",
    )
    .await
}

/// A transit gateway of the account, as `CreateTransitGateway` makes one.
async fn transit_gateway(case: &str, router: &MultiServiceRouter) -> String {
    create_resource(
        case,
        router,
        "CreateTransitGateway",
        &[],
        "transitGatewayId",
    )
    .await
}

/// The two devices a connection runs between: a customer gateway and a virtual private
/// gateway, both of this account.
async fn gateway_pair(case: &str, router: &MultiServiceRouter) -> (String, String) {
    let customer_gateway_id = customer_gateway(case, router, CUSTOMER_GATEWAY_ADDRESS).await;
    let vpn_gateway_id = vpn_gateway(case, router).await;
    (customer_gateway_id, vpn_gateway_id)
}

/// The form fields of one `CreateVpnConnection` naming `customer_gateway_id` and the
/// Amazon-side gateway `amazon_side` (keyed by the member it is named in), plus `extra`.
fn connection_params(
    customer_gateway_id: &str,
    amazon_side: (&str, &str),
    extra: &[Param],
) -> Vec<Param> {
    let mut params = vec![
        param("CustomerGatewayId", customer_gateway_id),
        param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
        param(amazon_side.0, amazon_side.1),
    ];
    params.extend(extra.iter().cloned());
    params
}

/// One tunnel as this suite compares them: every member the create reports for one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tunnel {
    outside_ip_address: String,
    inside_cidr: String,
    inside_ipv6_cidr: String,
    pre_shared_key: String,
    phase1_lifetime_seconds: String,
    phase2_lifetime_seconds: String,
    rekey_margin_time_seconds: String,
    rekey_fuzz_percentage: String,
    replay_window_size: String,
    dpd_timeout_seconds: String,
    dpd_timeout_action: String,
    startup_action: String,
    enable_tunnel_lifecycle_control: String,
    ike_versions: Vec<String>,
    phase1_encryption_algorithms: Vec<String>,
    phase2_encryption_algorithms: Vec<String>,
    phase1_integrity_algorithms: Vec<String>,
    phase1_dh_group_numbers: Vec<String>,
    phase2_dh_group_numbers: Vec<String>,
}

/// One reported connection: the members a create and a describe both report for one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Connection {
    id: String,
    state: String,
    gateway_association_state: String,
    category: String,
    connection_type: String,
    customer_gateway_id: String,
    vpn_gateway_id: String,
    transit_gateway_id: String,
    static_routes_only: String,
    enable_acceleration: String,
    tunnel_inside_ip_version: String,
    outside_ip_address_type: String,
    tunnel_bandwidth: String,
    local_ipv4_network_cidr: String,
    remote_ipv4_network_cidr: String,
    local_ipv6_network_cidr: String,
    remote_ipv6_network_cidr: String,
    tunnels: Vec<Tunnel>,
    telemetry: Vec<(String, String)>,
    configuration: String,
    tags: Vec<(String, String)>,
}

impl Connection {
    /// The connection with the two members the CLOCK moves taken out, which is what a create's
    /// answer and a later describe's must agree on.
    fn without_the_clock_owed_states(&self) -> Self {
        Self {
            state: String::new(),
            gateway_association_state: String::new(),
            ..self.clone()
        }
    }
}

/// The text values of every `<value>` inside the `name` set of `node`.
fn set_values(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<String> {
    set_items(node, name)
        .into_iter()
        .map(|item| child_text(item, "value").unwrap_or_default())
        .collect()
}

/// The one connection a response carries: the element with a `vpnConnectionId` child.
fn reported(case: &str, body: &str) -> Connection {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "vpnConnectionId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no VPN connection (no <vpnConnectionId>) in {body}"));
    let text = |name: &str| child_text(node, name).unwrap_or_default();
    let options = child(node, "options");
    let option_text = |name: &str| {
        options
            .and_then(|options| child_text(options, name))
            .unwrap_or_default()
    };
    let tunnels = options
        .map(|options| {
            set_items(options, "tunnelOptionSet")
                .into_iter()
                .map(|item| {
                    let member = |name: &str| child_text(item, name).unwrap_or_default();
                    Tunnel {
                        outside_ip_address: member("outsideIpAddress"),
                        inside_cidr: member("tunnelInsideCidr"),
                        inside_ipv6_cidr: member("tunnelInsideIpv6Cidr"),
                        pre_shared_key: member("preSharedKey"),
                        phase1_lifetime_seconds: member("phase1LifetimeSeconds"),
                        phase2_lifetime_seconds: member("phase2LifetimeSeconds"),
                        rekey_margin_time_seconds: member("rekeyMarginTimeSeconds"),
                        rekey_fuzz_percentage: member("rekeyFuzzPercentage"),
                        replay_window_size: member("replayWindowSize"),
                        dpd_timeout_seconds: member("dpdTimeoutSeconds"),
                        dpd_timeout_action: member("dpdTimeoutAction"),
                        startup_action: member("startupAction"),
                        enable_tunnel_lifecycle_control: member("enableTunnelLifecycleControl"),
                        ike_versions: set_values(item, "ikeVersionSet"),
                        phase1_encryption_algorithms: set_values(
                            item,
                            "phase1EncryptionAlgorithmSet",
                        ),
                        phase2_encryption_algorithms: set_values(
                            item,
                            "phase2EncryptionAlgorithmSet",
                        ),
                        phase1_integrity_algorithms: set_values(
                            item,
                            "phase1IntegrityAlgorithmSet",
                        ),
                        phase1_dh_group_numbers: set_values(item, "phase1DHGroupNumberSet"),
                        phase2_dh_group_numbers: set_values(item, "phase2DHGroupNumberSet"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let telemetry = set_items(node, "vgwTelemetry")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "outsideIpAddress").unwrap_or_default(),
                child_text(item, "status").unwrap_or_default(),
            )
        })
        .collect();
    let tags = set_items(node, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect();
    Connection {
        id: text("vpnConnectionId"),
        state: text("state"),
        gateway_association_state: text("gatewayAssociationState"),
        category: text("category"),
        connection_type: text("type"),
        customer_gateway_id: text("customerGatewayId"),
        vpn_gateway_id: text("vpnGatewayId"),
        transit_gateway_id: text("transitGatewayId"),
        static_routes_only: option_text("staticRoutesOnly"),
        enable_acceleration: option_text("enableAcceleration"),
        tunnel_inside_ip_version: option_text("tunnelInsideIpVersion"),
        outside_ip_address_type: option_text("outsideIpAddressType"),
        tunnel_bandwidth: option_text("tunnelBandwidth"),
        local_ipv4_network_cidr: option_text("localIpv4NetworkCidr"),
        remote_ipv4_network_cidr: option_text("remoteIpv4NetworkCidr"),
        local_ipv6_network_cidr: option_text("localIpv6NetworkCidr"),
        remote_ipv6_network_cidr: option_text("remoteIpv6NetworkCidr"),
        tunnels,
        telemetry,
        configuration: text("customerGatewayConfiguration"),
        tags,
    }
}

/// The connection `vpn_connection_id` names, as `DescribeVpnConnections` reports it.
async fn described(case: &str, router: &MultiServiceRouter, vpn_connection_id: &str) -> Connection {
    let (status, body) = call(
        router,
        "DescribeVpnConnections",
        &[param("VpnConnectionId.1", vpn_connection_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVpnConnections, got {status} with \
         body {body}"
    );
    reported(case, &body)
}

/// The connection `vpn_connection_id` names once the describe reports it in `expected`.
///
/// The move the clock owes a `pending` connection falls due on the clock, so the condition is
/// looked at again until it holds or a deadline generous enough for any host passes — never
/// waited out for a fixed time.
async fn described_once_in(
    case: &str,
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    expected: &str,
) -> Connection {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let reported = described(case, router, vpn_connection_id).await;
        if reported.state == expected || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// How many connections the store holds right now — what a refusal must have left untouched.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().vpn_connections.len()
}

/// The two usable addresses of a tunnel's inside block, as the native configuration names
/// them: the first address after the block's own goes to the Amazon side and the second to
/// the customer gateway device (Amazon Web Services Site-to-Site VPN User Guide, "tunnel
/// inside IP addresses").
///
/// Computed here from the block the response reported, so the document is checked against the
/// block rather than against the handler's own rendering of it.
fn usable_addresses(case: &str, block: &str) -> (String, String) {
    let (address, _) = block
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: {block:?} must be a CIDR block"));
    if let Ok(base) = address.parse::<std::net::Ipv4Addr>() {
        let base = u32::from(base);
        return (
            std::net::Ipv4Addr::from(base + 1).to_string(),
            std::net::Ipv4Addr::from(base + 2).to_string(),
        );
    }
    let base = address
        .parse::<std::net::Ipv6Addr>()
        .unwrap_or_else(|_| panic!("case {case:?}: {block:?} must be an IP CIDR block"));
    let base = u128::from(base);
    (
        std::net::Ipv6Addr::from(base + 1).to_string(),
        std::net::Ipv6Addr::from(base + 2).to_string(),
    )
}

/// Every assertion that holds for EVERY created connection, whatever the request named: the
/// two tunnels, their distinct addresses and blocks, the telemetry that matches them, and the
/// configuration document built out of them.
fn assert_the_connection_is_whole(case: &str, reported: &Connection, device_address: &str) {
    assert!(
        is_long_id(ec2::state::VPN_CONNECTION_ID_PREFIX, &reported.id),
        "case {case:?}: expected a server-minted {}- id, got {:?}",
        ec2::state::VPN_CONNECTION_ID_PREFIX,
        reported.id
    );
    assert_eq!(
        reported.category,
        ec2::state::VPN_CONNECTION_CATEGORY_VPN,
        "case {case:?}: expected the VPN category, got {:?}",
        reported.category
    );
    assert_eq!(
        reported.connection_type,
        ec2::state::GATEWAY_TYPE_IPSEC_1,
        "case {case:?}: expected the ipsec.1 type, got {:?}",
        reported.connection_type
    );
    assert_eq!(
        reported.tunnels.len(),
        ec2::state::TUNNELS_PER_VPN_CONNECTION,
        "case {case:?}: expected {} tunnels, got {:?}",
        ec2::state::TUNNELS_PER_VPN_CONNECTION,
        reported.tunnels
    );
    let ipv6_tunnels = reported.tunnel_inside_ip_version
        == ec2::state::TUNNEL_INSIDE_IP_VERSION_IPV6;
    let mut blocks: BTreeSet<&str> = BTreeSet::new();
    let mut addresses: BTreeSet<&str> = BTreeSet::new();
    for tunnel in &reported.tunnels {
        let block = if ipv6_tunnels {
            assert!(
                tunnel.inside_cidr.is_empty(),
                "case {case:?}: an ipv6 tunnel carries no inside IPv4 block, got {tunnel:?}"
            );
            &tunnel.inside_ipv6_cidr
        } else {
            assert!(
                tunnel.inside_ipv6_cidr.is_empty(),
                "case {case:?}: an ipv4 tunnel carries no inside IPv6 block, got {tunnel:?}"
            );
            &tunnel.inside_cidr
        };
        let inside_published = if ipv6_tunnels {
            ec2::state::ipv6_cidr_block_contains(
                ec2::state::TUNNEL_INSIDE_IPV6_CIDR_RANGE,
                block,
            )
        } else {
            ec2::state::ipv4_cidr_block_contains(ec2::state::TUNNEL_INSIDE_CIDR_RANGE, block)
        };
        assert!(
            inside_published,
            "case {case:?}: expected an inside block from the published range, got {block:?}"
        );
        assert!(
            !ec2::state::RESERVED_TUNNEL_INSIDE_CIDRS.contains(&block.as_str()),
            "case {case:?}: expected a block that is not reserved, got {block:?}"
        );
        assert!(
            blocks.insert(block.as_str()),
            "case {case:?}: the two tunnels must not share the inside block {block:?}"
        );
        assert!(
            ec2::state::pre_shared_key_is_well_formed(&tunnel.pre_shared_key),
            "case {case:?}: expected a pre-shared key of the published charset, got a \
             {}-character one",
            tunnel.pre_shared_key.chars().count()
        );
        assert!(
            !tunnel.outside_ip_address.is_empty(),
            "case {case:?}: expected an outside address on every tunnel, got {tunnel:?}"
        );
        assert!(
            addresses.insert(tunnel.outside_ip_address.as_str()),
            "case {case:?}: the two tunnels must not share the outside address {:?}",
            tunnel.outside_ip_address
        );
        // The configuration document is what the caller hands its network administrator, so
        // every tunnel's own values have to be in it: the address each end of the tunnel is
        // reached at, the two addresses inside it, and the key the two ends authenticate with.
        let (amazon_inside, customer_inside) = usable_addresses(case, block);
        for value in [
            amazon_inside.as_str(),
            customer_inside.as_str(),
            tunnel.outside_ip_address.as_str(),
            tunnel.pre_shared_key.as_str(),
        ] {
            assert!(
                reported.configuration.contains(value),
                "case {case:?}: expected the configuration document to carry {value:?}, got \
                 {:?}",
                reported.configuration
            );
        }
    }
    // One telemetry entry per tunnel, keyed to the address that tunnel reports, DOWN until an
    // end brings it up.
    let expected_telemetry: Vec<(String, String)> = reported
        .tunnels
        .iter()
        .map(|tunnel| {
            (
                tunnel.outside_ip_address.clone(),
                ec2::state::TELEMETRY_STATUS_DOWN.to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported.telemetry, expected_telemetry,
        "case {case:?}: expected one telemetry entry per tunnel, got {:?}",
        reported.telemetry
    );
    for value in [
        reported.id.as_str(),
        reported.customer_gateway_id.as_str(),
        device_address,
        CUSTOMER_GATEWAY_BGP_ASN,
    ] {
        assert!(
            reported.configuration.contains(value),
            "case {case:?}: expected the configuration document to carry {value:?}, got {:?}",
            reported.configuration
        );
    }
    assert!(
        roxmltree::Document::parse(&reported.configuration).is_ok(),
        "case {case:?}: the configuration document must be XML, got {:?}",
        reported.configuration
    );
}

/// One row of the create table: what makes it a different input, the extra form fields it
/// sends, and the connection-level members it must answer with.
struct CreateCase {
    case: &'static str,
    /// The Amazon-side gateway member this row names, `VpnGatewayId` unless it says otherwise.
    transit_gateway: bool,
    extra: Vec<Param>,
    /// `(member, value)` pairs the reported connection's own members must carry.
    expected: Vec<(&'static str, &'static str)>,
    /// The tunnel members both tunnels must carry, beyond what the shared assertions check.
    expected_tunnels: Vec<(&'static str, &'static str)>,
}

/// The value the reported connection carries for the member `name` of a row's expectations.
fn reported_member(reported: &Connection, name: &str) -> String {
    match name {
        "staticRoutesOnly" => reported.static_routes_only.clone(),
        "enableAcceleration" => reported.enable_acceleration.clone(),
        "tunnelInsideIpVersion" => reported.tunnel_inside_ip_version.clone(),
        "outsideIpAddressType" => reported.outside_ip_address_type.clone(),
        "tunnelBandwidth" => reported.tunnel_bandwidth.clone(),
        "localIpv4NetworkCidr" => reported.local_ipv4_network_cidr.clone(),
        "remoteIpv4NetworkCidr" => reported.remote_ipv4_network_cidr.clone(),
        "localIpv6NetworkCidr" => reported.local_ipv6_network_cidr.clone(),
        "remoteIpv6NetworkCidr" => reported.remote_ipv6_network_cidr.clone(),
        other => panic!("the table names no member {other:?} of a connection"),
    }
}

/// The value a tunnel carries for the member `name` of a row's expectations.
fn tunnel_member(tunnel: &Tunnel, name: &str) -> String {
    match name {
        "phase1LifetimeSeconds" => tunnel.phase1_lifetime_seconds.clone(),
        "phase2LifetimeSeconds" => tunnel.phase2_lifetime_seconds.clone(),
        "rekeyMarginTimeSeconds" => tunnel.rekey_margin_time_seconds.clone(),
        "rekeyFuzzPercentage" => tunnel.rekey_fuzz_percentage.clone(),
        "replayWindowSize" => tunnel.replay_window_size.clone(),
        "dpdTimeoutSeconds" => tunnel.dpd_timeout_seconds.clone(),
        "dpdTimeoutAction" => tunnel.dpd_timeout_action.clone(),
        "startupAction" => tunnel.startup_action.clone(),
        "enableTunnelLifecycleControl" => tunnel.enable_tunnel_lifecycle_control.clone(),
        other => panic!("the table names no member {other:?} of a tunnel"),
    }
}

#[tokio::test]
async fn a_create_makes_the_connection_the_request_describes() {
    let rows = vec![
        CreateCase {
            case: "no options at all: every member at its published default",
            transit_gateway: false,
            extra: Vec::new(),
            expected: vec![
                ("staticRoutesOnly", "false"),
                ("enableAcceleration", "false"),
                ("tunnelInsideIpVersion", "ipv4"),
                ("outsideIpAddressType", "PublicIpv4"),
                ("tunnelBandwidth", "standard"),
                ("localIpv4NetworkCidr", "0.0.0.0/0"),
                ("remoteIpv4NetworkCidr", "0.0.0.0/0"),
                ("localIpv6NetworkCidr", ""),
                ("remoteIpv6NetworkCidr", ""),
            ],
            expected_tunnels: vec![
                ("phase1LifetimeSeconds", "28800"),
                ("phase2LifetimeSeconds", "3600"),
                ("rekeyMarginTimeSeconds", "270"),
                ("rekeyFuzzPercentage", "100"),
                ("replayWindowSize", "1024"),
                ("dpdTimeoutSeconds", "30"),
                ("dpdTimeoutAction", "clear"),
                ("startupAction", "add"),
                ("enableTunnelLifecycleControl", "false"),
            ],
        },
        CreateCase {
            case: "static routes only, acceleration on, narrowed networks, large bandwidth",
            transit_gateway: false,
            extra: vec![
                param("Options.StaticRoutesOnly", "true"),
                param("Options.EnableAcceleration", "true"),
                param("Options.LocalIpv4NetworkCidr", "192.168.0.0/16"),
                param("Options.RemoteIpv4NetworkCidr", "10.0.0.0/8"),
                param("Options.TunnelBandwidth", "large"),
            ],
            expected: vec![
                ("staticRoutesOnly", "true"),
                ("enableAcceleration", "true"),
                ("tunnelBandwidth", "large"),
                ("localIpv4NetworkCidr", "192.168.0.0/16"),
                ("remoteIpv4NetworkCidr", "10.0.0.0/8"),
            ],
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "both tunnels described, with the lifetimes at their published ceilings",
            transit_gateway: false,
            extra: vec![
                param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.11.0/30"),
                param("Options.TunnelOptions.1.PreSharedKey", "first.tunnel_key1"),
                param("Options.TunnelOptions.1.Phase1LifetimeSeconds", "28800"),
                param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "3600"),
                param("Options.TunnelOptions.1.RekeyMarginTimeSeconds", "1800"),
                param("Options.TunnelOptions.1.RekeyFuzzPercentage", "100"),
                param("Options.TunnelOptions.1.ReplayWindowSize", "2048"),
                param("Options.TunnelOptions.2.TunnelInsideCidr", "169.254.12.0/30"),
                param("Options.TunnelOptions.2.PreSharedKey", "second.tunnel_key2"),
                param("Options.TunnelOptions.2.Phase1LifetimeSeconds", "28800"),
                param("Options.TunnelOptions.2.Phase2LifetimeSeconds", "3600"),
                param("Options.TunnelOptions.2.RekeyMarginTimeSeconds", "1800"),
                param("Options.TunnelOptions.2.RekeyFuzzPercentage", "100"),
                param("Options.TunnelOptions.2.ReplayWindowSize", "2048"),
            ],
            expected: Vec::new(),
            expected_tunnels: vec![
                ("phase1LifetimeSeconds", "28800"),
                ("phase2LifetimeSeconds", "3600"),
                ("rekeyMarginTimeSeconds", "1800"),
                ("replayWindowSize", "2048"),
            ],
        },
        CreateCase {
            case: "both tunnels at the FLOOR of every published range",
            transit_gateway: false,
            extra: vec![
                param("Options.TunnelOptions.1.Phase1LifetimeSeconds", "901"),
                param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "900"),
                param("Options.TunnelOptions.1.RekeyMarginTimeSeconds", "60"),
                param("Options.TunnelOptions.1.RekeyFuzzPercentage", "0"),
                param("Options.TunnelOptions.1.ReplayWindowSize", "64"),
                param("Options.TunnelOptions.1.DPDTimeoutSeconds", "30"),
                param("Options.TunnelOptions.2.Phase1LifetimeSeconds", "901"),
                param("Options.TunnelOptions.2.Phase2LifetimeSeconds", "900"),
                param("Options.TunnelOptions.2.RekeyMarginTimeSeconds", "60"),
                param("Options.TunnelOptions.2.RekeyFuzzPercentage", "0"),
                param("Options.TunnelOptions.2.ReplayWindowSize", "64"),
                param("Options.TunnelOptions.2.DPDTimeoutSeconds", "30"),
            ],
            expected: Vec::new(),
            expected_tunnels: vec![
                ("phase1LifetimeSeconds", "901"),
                ("phase2LifetimeSeconds", "900"),
                ("rekeyMarginTimeSeconds", "60"),
                ("rekeyFuzzPercentage", "0"),
                ("replayWindowSize", "64"),
                ("dpdTimeoutSeconds", "30"),
            ],
        },
        CreateCase {
            case: "one tunnel described, the other left to its defaults",
            transit_gateway: false,
            extra: vec![
                param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.21.0/30"),
                param("Options.TunnelOptions.1.DPDTimeoutAction", "restart"),
                param("Options.TunnelOptions.1.StartupAction", "start"),
                param("Options.TunnelOptions.1.EnableTunnelLifecycleControl", "true"),
            ],
            expected: Vec::new(),
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "tunnels that process IPv6 traffic, with narrowed IPv6 networks",
            transit_gateway: false,
            extra: vec![
                param("Options.TunnelInsideIpVersion", "ipv6"),
                param("Options.LocalIpv6NetworkCidr", "2001:db8:1::/64"),
                param("Options.RemoteIpv6NetworkCidr", "2001:db8:2::/64"),
            ],
            expected: vec![
                ("tunnelInsideIpVersion", "ipv6"),
                ("localIpv6NetworkCidr", "2001:db8:1::/64"),
                ("remoteIpv6NetworkCidr", "2001:db8:2::/64"),
                ("localIpv4NetworkCidr", ""),
                ("remoteIpv4NetworkCidr", ""),
            ],
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "a connection that terminates on a transit gateway",
            transit_gateway: true,
            extra: Vec::new(),
            expected: vec![("staticRoutesOnly", "false")],
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "the algorithm and version lists narrowed by the request",
            transit_gateway: false,
            extra: vec![
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                    "AES256",
                ),
                param(
                    "Options.TunnelOptions.1.Phase1IntegrityAlgorithm.1.Value",
                    "SHA2-512",
                ),
                param("Options.TunnelOptions.1.Phase1DHGroupNumber.1.Value", "24"),
                param("Options.TunnelOptions.1.Phase2DHGroupNumber.1.Value", "5"),
                param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev2"),
            ],
            expected: Vec::new(),
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "tunnels whose outside interface is addressed with IPv6",
            transit_gateway: false,
            extra: vec![param("Options.OutsideIpAddressType", "Ipv6")],
            expected: vec![("outsideIpAddressType", "Ipv6")],
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "the standard pre-shared key storage mode, named explicitly",
            transit_gateway: false,
            extra: vec![param(
                "PreSharedKeyStorage",
                ec2::state::PRE_SHARED_KEY_STORAGE_STANDARD,
            )],
            expected: Vec::new(),
            expected_tunnels: Vec::new(),
        },
        CreateCase {
            case: "the tags of a vpn-connection tag specification",
            transit_gateway: false,
            extra: vec![
                param(
                    "TagSpecification.1.ResourceType",
                    ec2::state::RESOURCE_TYPE_VPN_CONNECTION,
                ),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "branch-office"),
            ],
            expected: Vec::new(),
            expected_tunnels: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.case;
        // One emulator per row, so a row's own store holds only what it created.
        let (router, engine) = super::ec2_query::emulator();
        let customer_gateway_id = customer_gateway(case, &router, CUSTOMER_GATEWAY_ADDRESS).await;
        let amazon_side = if row.transit_gateway {
            ("TransitGatewayId", transit_gateway(case, &router).await)
        } else {
            ("VpnGatewayId", vpn_gateway(case, &router).await)
        };
        let (status, body) = call(
            &router,
            "CreateVpnConnection",
            &connection_params(
                &customer_gateway_id,
                (amazon_side.0, &amazon_side.1),
                &row.extra,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateVpnConnection, got {status} with \
             body {body}"
        );
        let created = reported(case, &body);
        assert_the_connection_is_whole(case, &created, CUSTOMER_GATEWAY_ADDRESS);
        assert_eq!(
            created.state,
            ec2::state::VPN_STATE_PENDING,
            "case {case:?}: a created connection is answered pending, got {:?}",
            created.state
        );
        assert_eq!(
            created.gateway_association_state,
            ec2::state::GATEWAY_ASSOCIATION_STATE_ASSOCIATING,
            "case {case:?}: a created connection is associating with its gateway, got {:?}",
            created.gateway_association_state
        );
        assert_eq!(
            created.customer_gateway_id, customer_gateway_id,
            "case {case:?}: expected the customer gateway the request named, got {:?}",
            created.customer_gateway_id
        );
        let (reported_gateway, other_gateway) = if row.transit_gateway {
            (&created.transit_gateway_id, &created.vpn_gateway_id)
        } else {
            (&created.vpn_gateway_id, &created.transit_gateway_id)
        };
        assert_eq!(
            reported_gateway, &amazon_side.1,
            "case {case:?}: expected the Amazon-side gateway the request named, got \
             {reported_gateway:?}"
        );
        assert!(
            other_gateway.is_empty(),
            "case {case:?}: a connection terminates on ONE Amazon-side gateway, got \
             {other_gateway:?} as well"
        );
        for (name, expected) in &row.expected {
            let actual = reported_member(&created, name);
            assert_eq!(
                &actual, expected,
                "case {case:?}: expected {name} {expected:?}, got {actual:?}"
            );
        }
        for tunnel in &created.tunnels {
            for (name, expected) in &row.expected_tunnels {
                let actual = tunnel_member(tunnel, name);
                assert_eq!(
                    &actual, expected,
                    "case {case:?}: expected {name} {expected:?} on every tunnel, got \
                     {actual:?}"
                );
            }
        }
        // Everything but the two members the clock moves reads back through the describe.
        let described = described(case, &router, &created.id).await;
        assert_eq!(
            described.without_the_clock_owed_states(),
            created.without_the_clock_owed_states(),
            "case {case:?}: the describe must report the connection the create stored"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: the create must have stored exactly one connection"
        );
        // The move the clock owes it: `available`, with the gateway association made.
        let settled = described_once_in(case, &router, &created.id, ec2::state::VPN_STATE_AVAILABLE)
            .await;
        assert_eq!(
            settled.state,
            ec2::state::VPN_STATE_AVAILABLE,
            "case {case:?}: the clock owes a pending connection available, got {:?}",
            settled.state
        );
        assert_eq!(
            settled.gateway_association_state,
            ec2::state::GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
            "case {case:?}: an available connection is associated with its gateway, got {:?}",
            settled.gateway_association_state
        );
        assert_eq!(
            settled.tunnels, created.tunnels,
            "case {case:?}: the tunnels must not change as the connection comes up"
        );
    }
}

#[tokio::test]
async fn the_lists_a_request_narrows_are_the_lists_the_tunnel_reports() {
    let case = "the request permits one algorithm, one group and one IKE version";
    let (router, _engine) = super::ec2_query::emulator();
    let (customer_gateway_id, vpn_gateway_id) = gateway_pair(case, &router).await;
    let (status, body) = call(
        &router,
        "CreateVpnConnection",
        &connection_params(
            &customer_gateway_id,
            ("VpnGatewayId", &vpn_gateway_id),
            &[
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                    "AES256",
                ),
                param(
                    "Options.TunnelOptions.1.Phase2EncryptionAlgorithm.1.Value",
                    "AES128-GCM-16",
                ),
                param(
                    "Options.TunnelOptions.1.Phase1IntegrityAlgorithm.1.Value",
                    "SHA2-512",
                ),
                param("Options.TunnelOptions.1.Phase1DHGroupNumber.1.Value", "24"),
                param("Options.TunnelOptions.1.Phase2DHGroupNumber.1.Value", "5"),
                param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev2"),
            ],
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let created = reported(case, &body);
    let narrowed = created
        .tunnels
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the connection must carry a first tunnel"));
    assert_eq!(
        narrowed.phase1_encryption_algorithms,
        vec!["AES256".to_string()],
        "case {case:?}: expected the one phase 1 algorithm the request permitted"
    );
    assert_eq!(
        narrowed.phase2_encryption_algorithms,
        vec!["AES128-GCM-16".to_string()],
        "case {case:?}: expected the one phase 2 algorithm the request permitted"
    );
    assert_eq!(
        narrowed.phase1_integrity_algorithms,
        vec!["SHA2-512".to_string()],
        "case {case:?}: expected the one integrity algorithm the request permitted"
    );
    assert_eq!(
        narrowed.phase1_dh_group_numbers,
        vec!["24".to_string()],
        "case {case:?}: expected the one phase 1 group the request permitted"
    );
    assert_eq!(
        narrowed.phase2_dh_group_numbers,
        vec!["5".to_string()],
        "case {case:?}: expected the one phase 2 group the request permitted"
    );
    assert_eq!(
        narrowed.ike_versions,
        vec!["ikev2".to_string()],
        "case {case:?}: expected the one IKE version the request permitted"
    );
    // The tunnel the request said nothing about permits everything the tunnel supports.
    let defaulted = created
        .tunnels
        .get(1)
        .unwrap_or_else(|| panic!("case {case:?}: the connection must carry a second tunnel"));
    assert_eq!(
        defaulted.ike_versions,
        ec2::state::IKE_VERSIONS
            .iter()
            .map(|version| (*version).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: a tunnel the request narrowed nothing on permits every IKE version"
    );
    assert_eq!(
        defaulted.phase1_encryption_algorithms,
        ec2::state::VPN_ENCRYPTION_ALGORITHMS
            .iter()
            .map(|algorithm| (*algorithm).to_string())
            .collect::<Vec<String>>(),
        "case {case:?}: a tunnel the request narrowed nothing on permits every algorithm"
    );
}

/// One row of the refusal table: what makes it a different input, the extra form fields, and
/// the EC2 code the refusal must carry.
struct RefusalCase {
    case: &'static str,
    extra: Vec<Param>,
    code: &'static str,
}

#[tokio::test]
async fn a_value_outside_what_a_member_publishes_is_refused_and_stores_nothing() {
    let rows = vec![
        RefusalCase {
            case: "a phase 1 lifetime past the published ceiling",
            extra: vec![param("Options.TunnelOptions.1.Phase1LifetimeSeconds", "28801")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a phase 1 lifetime at the floor of its range, which no phase 2 fits under",
            extra: vec![param("Options.TunnelOptions.1.Phase1LifetimeSeconds", "900")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a phase 2 lifetime below the published floor",
            extra: vec![param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "899")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a phase 2 lifetime past the published ceiling",
            extra: vec![param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "3601")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a phase 2 lifetime that is not below the phase 1 lifetime",
            extra: vec![
                param("Options.TunnelOptions.1.Phase1LifetimeSeconds", "3600"),
                param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "3600"),
            ],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a rekey margin above half the phase 2 lifetime",
            extra: vec![
                param("Options.TunnelOptions.1.Phase2LifetimeSeconds", "900"),
                param("Options.TunnelOptions.1.RekeyMarginTimeSeconds", "451"),
            ],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a rekey margin below the published floor",
            extra: vec![param("Options.TunnelOptions.1.RekeyMarginTimeSeconds", "59")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a rekey fuzz percentage past the published ceiling",
            extra: vec![param("Options.TunnelOptions.1.RekeyFuzzPercentage", "101")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a replay window below the published floor",
            extra: vec![param("Options.TunnelOptions.1.ReplayWindowSize", "63")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a replay window past the published ceiling",
            extra: vec![param("Options.TunnelOptions.1.ReplayWindowSize", "2049")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a DPD timeout below the published floor",
            extra: vec![param("Options.TunnelOptions.1.DPDTimeoutSeconds", "29")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a DPD timeout action outside the published set",
            extra: vec![param("Options.TunnelOptions.1.DPDTimeoutAction", "stop")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a startup action outside the published set",
            extra: vec![param("Options.TunnelOptions.1.StartupAction", "restart")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an IKE version outside the published set",
            extra: vec![param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev3")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an encryption algorithm outside the published set",
            extra: vec![param(
                "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                "AES512",
            )],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an integrity algorithm outside the published set",
            extra: vec![param(
                "Options.TunnelOptions.1.Phase2IntegrityAlgorithm.1.Value",
                "MD5",
            )],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a phase 1 group number only phase 2 publishes",
            extra: vec![param("Options.TunnelOptions.1.Phase1DHGroupNumber.1.Value", "5")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a group number neither phase publishes",
            extra: vec![param("Options.TunnelOptions.1.Phase2DHGroupNumber.1.Value", "3")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a pre-shared key shorter than the published minimum",
            extra: vec![param("Options.TunnelOptions.1.PreSharedKey", "short12")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a pre-shared key longer than the published maximum",
            extra: vec![param("Options.TunnelOptions.1.PreSharedKey", &"k".repeat(65))],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a pre-shared key starting with zero",
            extra: vec![param("Options.TunnelOptions.1.PreSharedKey", "0abcdefgh")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a pre-shared key carrying a character outside the published charset",
            extra: vec![param("Options.TunnelOptions.1.PreSharedKey", "key-with-dash")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside block the published range reserves",
            extra: vec![param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.1.0/30")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside block outside the published range",
            extra: vec![param("Options.TunnelOptions.1.TunnelInsideCidr", "10.0.0.0/30")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside block of the wrong size",
            extra: vec![param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.8.0/29")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside block whose host bits are set, so it names no /30",
            extra: vec![param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.8.1/30")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside block that is not a CIDR block at all",
            extra: vec![param("Options.TunnelOptions.1.TunnelInsideCidr", "one sixty nine")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside IPv6 block on a connection whose tunnels process IPv4",
            extra: vec![param(
                "Options.TunnelOptions.1.TunnelInsideIpv6Cidr",
                "fd00::/126",
            )],
            code: ec2::state::INVALID_PARAMETER_COMBINATION,
        },
        RefusalCase {
            case: "an inside IPv4 block on a connection whose tunnels process IPv6",
            extra: vec![
                param("Options.TunnelInsideIpVersion", "ipv6"),
                param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.9.0/30"),
            ],
            code: ec2::state::INVALID_PARAMETER_COMBINATION,
        },
        RefusalCase {
            case: "an inside IPv6 block outside the published range",
            extra: vec![
                param("Options.TunnelInsideIpVersion", "ipv6"),
                param(
                    "Options.TunnelOptions.1.TunnelInsideIpv6Cidr",
                    "2001:db8::/126",
                ),
            ],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an inside IPv6 block of the wrong size",
            extra: vec![
                param("Options.TunnelInsideIpVersion", "ipv6"),
                param("Options.TunnelOptions.1.TunnelInsideIpv6Cidr", "fd00::/120"),
            ],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "more tunnels than the connection has",
            extra: vec![
                param("Options.TunnelOptions.1.TunnelInsideCidr", "169.254.31.0/30"),
                param("Options.TunnelOptions.2.TunnelInsideCidr", "169.254.32.0/30"),
                param("Options.TunnelOptions.3.TunnelInsideCidr", "169.254.33.0/30"),
            ],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a local network block that is not a CIDR block",
            extra: vec![param("Options.LocalIpv4NetworkCidr", "the office")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an outside address type outside the published set",
            extra: vec![param("Options.OutsideIpAddressType", "Ipv5")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a private outside address type with no transport attachment named",
            extra: vec![param("Options.OutsideIpAddressType", "PrivateIpv4")],
            code: ec2::state::MISSING_PARAMETER,
        },
        RefusalCase {
            case: "a transport attachment the account does not hold",
            extra: vec![
                param("Options.OutsideIpAddressType", "PrivateIpv4"),
                param(
                    "Options.TransportTransitGatewayAttachmentId",
                    "tgw-attach-00000000000000001",
                ),
            ],
            code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a pre-shared key storage mode this service does not hold keys in",
            extra: vec![param("PreSharedKeyStorage", "SecretsManager")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a pre-shared key storage mode outside the published set",
            extra: vec![param("PreSharedKeyStorage", "Vault")],
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "a VPN concentrator the account does not hold",
            extra: vec![param("VpnConcentratorId", "vpnc-00000000000000001")],
            code: ec2::state::INVALID_VPN_CONCENTRATOR_ID_NOT_FOUND,
        },
    ];

    let (router, engine) = super::ec2_query::emulator();
    let (customer_gateway_id, vpn_gateway_id) =
        gateway_pair("the gateways every row names", &router).await;
    for row in rows {
        let case = row.case;
        let (status, body) = call(
            &router,
            "CreateVpnConnection",
            &connection_params(
                &customer_gateway_id,
                ("VpnGatewayId", &vpn_gateway_id),
                &row.extra,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from CreateVpnConnection, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got the body {body}",
            row.code
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

#[tokio::test]
async fn a_refusal_of_a_pre_shared_key_never_echoes_the_key() {
    let case = "the key is the secret the two ends authenticate with";
    let secret = "0-not-a-valid-key";
    let (router, _engine) = super::ec2_query::emulator();
    let (customer_gateway_id, vpn_gateway_id) = gateway_pair(case, &router).await;
    let (status, body) = call(
        &router,
        "CreateVpnConnection",
        &connection_params(
            &customer_gateway_id,
            ("VpnGatewayId", &vpn_gateway_id),
            &[param("Options.TunnelOptions.1.PreSharedKey", secret)],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert!(
        !body.contains(secret),
        "case {case:?}: the refusal must not echo the pre-shared key, got {body}"
    );
    assert!(
        error_message(&body).contains("preSharedKey"),
        "case {case:?}: the refusal must name the member, got {body}"
    );
}

#[tokio::test]
async fn an_inside_block_another_tunnel_uses_is_refused_and_minted_blocks_are_distinct() {
    let case = "three connections of one account";
    let (router, engine) = super::ec2_query::emulator();
    let vpn_gateway_id = vpn_gateway(case, &router).await;
    let mut blocks: Vec<String> = Vec::new();
    let mut addresses: Vec<String> = Vec::new();
    // Three different customer gateways, so each create makes a connection of its own rather
    // than answering the one before it.
    for octet in 1..=3u8 {
        let customer_gateway_id =
            customer_gateway(case, &router, &format!("203.0.113.{octet}")).await;
        let (status, body) = call(
            &router,
            "CreateVpnConnection",
            &connection_params(
                &customer_gateway_id,
                ("VpnGatewayId", &vpn_gateway_id),
                &[],
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for connection {octet}, got {status} with \
             body {body}"
        );
        let created = reported(case, &body);
        assert_the_connection_is_whole(case, &created, &format!("203.0.113.{octet}"));
        for tunnel in &created.tunnels {
            blocks.push(tunnel.inside_cidr.clone());
            addresses.push(tunnel.outside_ip_address.clone());
        }
    }
    let distinct_blocks: BTreeSet<&String> = blocks.iter().collect();
    assert_eq!(
        distinct_blocks.len(),
        blocks.len(),
        "case {case:?}: no two tunnels may share an inside block, got {blocks:?}"
    );
    let distinct_addresses: BTreeSet<&String> = addresses.iter().collect();
    assert_eq!(
        distinct_addresses.len(),
        addresses.len(),
        "case {case:?}: no two tunnels may share an outside address, got {addresses:?}"
    );
    assert_eq!(
        stored_count(&engine),
        3,
        "case {case:?}: the account must hold the three connections it created"
    );

    // A fourth create naming a block one of those tunnels already uses is refused.
    let held = blocks
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the first connection must carry a block"));
    let customer_gateway_id = customer_gateway(case, &router, "203.0.113.9").await;
    let (status, body) = call(
        &router,
        "CreateVpnConnection",
        &connection_params(
            &customer_gateway_id,
            ("VpnGatewayId", &vpn_gateway_id),
            &[param("Options.TunnelOptions.1.TunnelInsideCidr", &held)],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a block another tunnel uses must be refused, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PARAMETER_VALUE,
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        3,
        "case {case:?}: a refused create must store nothing"
    );
}

#[tokio::test]
async fn a_repeat_of_the_same_request_answers_the_connection_it_already_made() {
    let case = "two identical requests";
    let (router, engine) = super::ec2_query::emulator();
    let (customer_gateway_id, vpn_gateway_id) = gateway_pair(case, &router).await;
    let params = connection_params(
        &customer_gateway_id,
        ("VpnGatewayId", &vpn_gateway_id),
        &[param("Options.StaticRoutesOnly", "true")],
    );
    let (status, body) = call(&router, "CreateVpnConnection", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the first create, got {status} with body {body}"
    );
    let first = reported(case, &body);
    let (status, body) = call(&router, "CreateVpnConnection", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a repeat must not be an error, got {status} with body {body}"
    );
    let again = reported(case, &body);
    assert_eq!(
        again.id, first.id,
        "case {case:?}: a repeat answers the connection already running between the two \
         gateways"
    );
    assert_eq!(
        again.tunnels, first.tunnels,
        "case {case:?}: a repeat must not re-key the tunnels of the connection it answers"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case {case:?}: a repeat must store no second connection"
    );

    // A request naming ANOTHER customer gateway is a different connection, and is made.
    let other_gateway_id = customer_gateway(case, &router, "203.0.113.77").await;
    let (status, body) = call(
        &router,
        "CreateVpnConnection",
        &connection_params(&other_gateway_id, ("VpnGatewayId", &vpn_gateway_id), &[]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for the second pair, got {status} with body {body}"
    );
    let other = reported(case, &body);
    assert_ne!(
        other.id, first.id,
        "case {case:?}: another pair of gateways is another connection"
    );
    assert_eq!(
        stored_count(&engine),
        2,
        "case {case:?}: the account must hold both connections"
    );
}

#[tokio::test]
async fn a_gateway_the_account_does_not_hold_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (customer_gateway_id, vpn_gateway_id) =
        gateway_pair("the gateways the account does hold", &router).await;
    // (case, the fields that replace the well-formed pair, the HTTP status, the code)
    let rows: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a customer gateway id the account holds no gateway for",
            vec![
                param("CustomerGatewayId", "cgw-00000000000000003"),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("VpnGatewayId", &vpn_gateway_id),
            ],
            400,
            ec2::state::INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "a virtual private gateway id the account holds no gateway for",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("VpnGatewayId", "vgw-0123456789abcdef0"),
            ],
            400,
            ec2::state::INVALID_VPN_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "a transit gateway id the account holds no gateway for",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("TransitGatewayId", "tgw-0123456789abcdef0"),
            ],
            400,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "both Amazon-side gateways named at once",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("VpnGatewayId", &vpn_gateway_id),
                param("TransitGatewayId", "tgw-0123456789abcdef0"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "neither Amazon-side gateway named",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
            ],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "an empty customer gateway id, which names no gateway",
            vec![
                param("CustomerGatewayId", ""),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("VpnGatewayId", &vpn_gateway_id),
            ],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a connection type the service does not make",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", "ipsec.2"),
                param("VpnGatewayId", &vpn_gateway_id),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a request that would have succeeded",
            vec![
                param("CustomerGatewayId", &customer_gateway_id),
                param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
                param("VpnGatewayId", &vpn_gateway_id),
                param("DryRun", "true"),
            ],
            // The status EC2 answers a dry run with: "Request would have succeeded, but
            // DryRun flag is set" is a precondition failure, not a bad request.
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];
    for (case, params, expected_status, code) in rows {
        let (status, body) = call(&router, "CreateVpnConnection", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from CreateVpnConnection, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got the body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused create must store nothing"
        );
    }
}
