// SPDX-License-Identifier: CC0-1.0

//! What MAKING a Site-to-Site VPN connection consists of: the two tunnels the connection is
//! brought up with, the values of a tunnel the request may name and the ones this service
//! owns (the outside address, the inside CIDR block and the pre-shared key), the
//! connection-level options at the defaults the model publishes, the telemetry each tunnel
//! reports, the customer gateway configuration document built out of all of it, and the one
//! write path that stores the result.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there. The CONNECTION record itself, its states and its static routes
//! live in `nat_gateways_and_vpn_connections`; this module is only about minting one.

use super::*;
use crate::ops::types::{
    CloudWatchLogOptions, IKEVersionsListValue, Phase1DHGroupNumbersListValue,
    Phase1EncryptionAlgorithmsListValue, Phase1IntegrityAlgorithmsListValue,
    Phase2DHGroupNumbersListValue, Phase2EncryptionAlgorithmsListValue,
    Phase2IntegrityAlgorithmsListValue, TunnelOption, VgwTelemetry, VpnConnectionOptions,
    VpnConnectionOptionsSpecification, VpnTunnelLogOptions, VpnTunnelOptionsSpecification,
};

/// "A value of `VPN` indicates an Amazon Web Services VPN connection. A value of
/// `VPN-Classic` indicates an Amazon Web Services Classic VPN connection"
/// (`VpnConnection$Category`): a connection created through this API is the former, Classic
/// VPN being the EC2-Classic era this service does not create in.
pub const VPN_CONNECTION_CATEGORY_VPN: &str = "VPN";

/// The `ResourceType` a `CreateVpnConnection` tag specification may name: the VPN connection
/// it creates ("The tags to apply to the VPN connection",
/// `CreateVpnConnectionRequest$TagSpecifications`), which is the `ResourceType` enum's
/// `vpn-connection` member ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub const RESOURCE_TYPE_VPN_CONNECTION: &str = "vpn-connection";

/// The `GatewayAssociationState` of a connection whose association with the Amazon-side
/// gateway is still being made: that enum's `associating` member
/// ([`crate::ops::types::GATEWAY_ASSOCIATION_STATE_VALUES`]).
pub const GATEWAY_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// The same enum's `associated` member: the connection is associated with the Amazon-side
/// gateway it terminates on.
pub const GATEWAY_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// How many tunnels one Site-to-Site VPN connection is brought up with.
///
/// "Your Site-to-Site VPN connection consists of two tunnels" (Amazon Web Services
/// Site-to-Site VPN User Guide, which `CreateVpnConnection` points at for the connection's
/// own description), which is why `VpnConnectionOptionsSpecification$TunnelOptions` carries
/// the options "for a single VPN tunnel" per entry: a request may describe at most this many.
pub const TUNNELS_PER_VPN_CONNECTION: usize = 2;

/// The range a tunnel's inside IPv4 addresses come out of: "a size /30 CIDR block from the
/// `169.254.0.0/16` range" (`VpnTunnelOptionsSpecification$TunnelInsideCidr`).
pub const TUNNEL_INSIDE_CIDR_RANGE: &str = "169.254.0.0/16";

/// The one size a tunnel's inside IPv4 block has, from that same sentence.
pub const TUNNEL_INSIDE_CIDR_PREFIX_LENGTH: u8 = 30;

/// The inside IPv4 blocks `VpnTunnelOptionsSpecification$TunnelInsideCidr` publishes as
/// "reserved and cannot be used", in the order it lists them.
pub const RESERVED_TUNNEL_INSIDE_CIDRS: [&str; 7] = [
    "169.254.0.0/30",
    "169.254.1.0/30",
    "169.254.2.0/30",
    "169.254.3.0/30",
    "169.254.4.0/30",
    "169.254.5.0/30",
    "169.254.169.252/30",
];

/// The range a tunnel's inside IPv6 addresses come out of: "a size /126 CIDR block from the
/// local `fd00::/8` range" (`VpnTunnelOptionsSpecification$TunnelInsideIpv6Cidr`).
pub const TUNNEL_INSIDE_IPV6_CIDR_RANGE: &str = "fd00::/8";

/// The one size a tunnel's inside IPv6 block has, from that same sentence.
pub const TUNNEL_INSIDE_IPV6_CIDR_PREFIX_LENGTH: u8 = 126;

/// How many candidate blocks the inside-block scans below try before giving up: every block
/// of the published range, so the scan is exhaustive and still terminates.
///
/// `169.254.0.0/16` holds this many `/30`s (one per four addresses), and the IPv6 scan is
/// held to the same count rather than to the vastly larger `fd00::/8`, because a store that
/// is already holding this many tunnels has nothing to do with address space.
const MAX_TUNNEL_INSIDE_BLOCK_PROBES: u32 = 1 << 14;

/// "Indicate whether the VPN tunnels process IPv4 or IPv6 traffic … Default: `ipv4`"
/// (`VpnConnectionOptionsSpecification$TunnelInsideIpVersion`) — the `TunnelInsideIpVersion`
/// enum's `ipv4` member.
pub const TUNNEL_INSIDE_IP_VERSION_IPV4: &str = "ipv4";

/// The same enum's `ipv6` member.
pub const TUNNEL_INSIDE_IP_VERSION_IPV6: &str = "ipv6";

/// "The IPv4 CIDR on the customer gateway (on-premises) side of the VPN connection …
/// Default: `0.0.0.0/0`" (`VpnConnectionOptionsSpecification$LocalIpv4NetworkCidr`), which is
/// also `RemoteIpv4NetworkCidr`'s published default.
pub const DEFAULT_VPN_IPV4_NETWORK_CIDR: &str = "0.0.0.0/0";

/// "The IPv6 CIDR on the customer gateway (on-premises) side of the VPN connection …
/// Default: `::/0`" (`VpnConnectionOptionsSpecification$LocalIpv6NetworkCidr`), which is also
/// `RemoteIpv6NetworkCidr`'s published default.
pub const DEFAULT_VPN_IPV6_NETWORK_CIDR: &str = "::/0";

/// "Valid values: `PrivateIpv4` | `PublicIpv4` | `Ipv6`"
/// (`VpnConnectionOptionsSpecification$OutsideIpAddressType`), in the order published.
pub const OUTSIDE_IP_ADDRESS_TYPES: [&str; 3] = ["PrivateIpv4", "PublicIpv4", "Ipv6"];

/// That member's "Default: `PublicIpv4`".
pub const DEFAULT_OUTSIDE_IP_ADDRESS_TYPE: &str = "PublicIpv4";

/// The outside address type whose tunnels are addressed from the transport transit gateway
/// attachment's own VPC: "Required if `OutsideIpAddressType` is set to `PrivateIpv4`"
/// (`VpnConnectionOptionsSpecification$TransportTransitGatewayAttachmentId`).
pub const OUTSIDE_IP_ADDRESS_TYPE_PRIVATE_IPV4: &str = "PrivateIpv4";

/// The outside address type whose tunnels are addressed with IPv6.
pub const OUTSIDE_IP_ADDRESS_TYPE_IPV6: &str = "Ipv6";

/// "If no tunnel bandwidth was specified for the connection, `standard` is used as the
/// default value" (`VpnConnectionOptions$TunnelBandwidth`) — the `VpnTunnelBandwidth` enum's
/// `standard` member.
pub const DEFAULT_VPN_TUNNEL_BANDWIDTH: &str = "standard";

/// The `TelemetryStatus` a tunnel reports before either end has brought it up: the enum's
/// `DOWN` member (`smithy_query(query="shape",shape="TelemetryStatus")`).
pub const TELEMETRY_STATUS_DOWN: &str = "DOWN";

/// What `VgwTelemetry$StatusMessage` carries for a tunnel that is down — "if an error
/// occurs, a description of the error" — for the one state a created connection's tunnels are
/// in: neither end has negotiated IPSec yet, which is what the status says.
pub const TELEMETRY_STATUS_MESSAGE_DOWN: &str = "IPSEC IS DOWN";

/// "The lifetime for phase 1 of the IKE negotiation, in seconds … Default: `28800`"
/// (`VpnTunnelOptionsSpecification$Phase1LifetimeSeconds`).
pub const DEFAULT_PHASE1_LIFETIME_SECONDS: i32 = 28_800;

/// "Constraints: A value between 900 and 28,800" (same member).
pub const PHASE1_LIFETIME_SECONDS: ::std::ops::RangeInclusive<i32> = 900..=28_800;

/// "The lifetime for phase 2 of the IKE negotiation, in seconds … Default: `3600`"
/// (`VpnTunnelOptionsSpecification$Phase2LifetimeSeconds`).
pub const DEFAULT_PHASE2_LIFETIME_SECONDS: i32 = 3_600;

/// "Constraints: A value between 900 and 3,600. The value must be less than the value for
/// `Phase1LifetimeSeconds`" (same member) — the range; the relation to phase 1 is checked
/// beside it.
pub const PHASE2_LIFETIME_SECONDS: ::std::ops::RangeInclusive<i32> = 900..=3_600;

/// "The margin time, in seconds, before the phase 2 lifetime expires … Default: `270`"
/// (`VpnTunnelOptionsSpecification$RekeyMarginTimeSeconds`).
pub const DEFAULT_REKEY_MARGIN_TIME_SECONDS: i32 = 270;

/// "Constraints: A value between 60 and half of `Phase2LifetimeSeconds`" (same member) — the
/// lower bound; the upper one is derived from the phase 2 lifetime in force.
pub const MIN_REKEY_MARGIN_TIME_SECONDS: i32 = 60;

/// "The percentage of the rekey window … Default: `100`"
/// (`VpnTunnelOptionsSpecification$RekeyFuzzPercentage`).
pub const DEFAULT_REKEY_FUZZ_PERCENTAGE: i32 = 100;

/// "Constraints: A value between 0 and 100" (same member).
pub const REKEY_FUZZ_PERCENTAGE: ::std::ops::RangeInclusive<i32> = 0..=100;

/// "The number of packets in an IKE replay window … Default: `1024`"
/// (`VpnTunnelOptionsSpecification$ReplayWindowSize`).
pub const DEFAULT_REPLAY_WINDOW_SIZE: i32 = 1_024;

/// "Constraints: A value between 64 and 2048" (same member).
pub const REPLAY_WINDOW_SIZE: ::std::ops::RangeInclusive<i32> = 64..=2_048;

/// "The number of seconds after which a DPD timeout occurs … Default: `30`"
/// (`VpnTunnelOptionsSpecification$DPDTimeoutSeconds`).
pub const DEFAULT_DPD_TIMEOUT_SECONDS: i32 = 30;

/// "Constraints: A value greater than or equal to 30" (same member).
pub const MIN_DPD_TIMEOUT_SECONDS: i32 = 30;

/// "Valid Values: `clear` | `none` | `restart` … Default: `clear`"
/// (`VpnTunnelOptionsSpecification$DPDTimeoutAction`), in the order published.
pub const DPD_TIMEOUT_ACTIONS: [&str; 3] = ["clear", "none", "restart"];

/// That member's published default.
pub const DEFAULT_DPD_TIMEOUT_ACTION: &str = "clear";

/// "Valid Values: `add` | `start` … Default: `add`"
/// (`VpnTunnelOptionsSpecification$StartupAction`), in the order published.
pub const STARTUP_ACTIONS: [&str; 2] = ["add", "start"];

/// That member's published default.
pub const DEFAULT_STARTUP_ACTION: &str = "add";

/// "Valid values: `AES128` | `AES256` | `AES128-GCM-16` | `AES256-GCM-16`"
/// (`VpnTunnelOptionsSpecification$Phase1EncryptionAlgorithms`, and the same list for phase
/// 2), in the order published. A tunnel the request names none for permits every one of them.
pub const VPN_ENCRYPTION_ALGORITHMS: [&str; 4] =
    ["AES128", "AES256", "AES128-GCM-16", "AES256-GCM-16"];

/// "Valid values: `SHA1` | `SHA2-256` | `SHA2-384` | `SHA2-512`"
/// (`VpnTunnelOptionsSpecification$Phase1IntegrityAlgorithms`, and the same list for phase 2),
/// in the order published.
pub const VPN_INTEGRITY_ALGORITHMS: [&str; 4] = ["SHA1", "SHA2-256", "SHA2-384", "SHA2-512"];

/// "Valid values: 2 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 24"
/// (`VpnTunnelOptionsSpecification$Phase1DHGroupNumbers`), in the order published.
pub const PHASE1_DH_GROUP_NUMBERS: [i32; 12] = [2, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24];

/// "Valid values: 2 | 5 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 24"
/// (`VpnTunnelOptionsSpecification$Phase2DHGroupNumbers`) — phase 2 permits group 5 as well.
pub const PHASE2_DH_GROUP_NUMBERS: [i32; 13] =
    [2, 5, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24];

/// "Valid values: `ikev1` | `ikev2`" (`VpnTunnelOptionsSpecification$IKEVersions`), in the
/// order published. A tunnel the request names none for permits both.
pub const IKE_VERSIONS: [&str; 2] = ["ikev1", "ikev2"];

/// How long a pre-shared key may be: "Must be between 8 and 64 characters in length"
/// (`VpnTunnelOptionsSpecification$PreSharedKey`).
pub const PRE_SHARED_KEY_CHARS: ::std::ops::RangeInclusive<usize> = 8..=64;

/// The storage mode whose pre-shared keys the Site-to-Site VPN service holds itself:
/// "Standard" (stored in the Site-to-Site VPN service)" — the mode a request that names none
/// gets, and the mode the keys this module mints are in, since they are reported on the
/// connection's own tunnel options ([`crate::ops::types::TunnelOption`]).
pub const PRE_SHARED_KEY_STORAGE_STANDARD: &str = "Standard";

/// How long the pre-shared key this service generates is, when the request names none: the
/// longest the member's own range admits in its own charset, so a generated key is never the
/// weakest one the API would accept.
const GENERATED_PRE_SHARED_KEY_CHARS: usize = 64;

/// The characters a generated pre-shared key is drawn from: "alphanumeric characters" from
/// `VpnTunnelOptionsSpecification$PreSharedKey`'s charset, the one subset of it that is
/// case-independent of nothing and carries no `0` first character to avoid.
const PRE_SHARED_KEY_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz123456789";

/// EC2's error code for a create that would take the account past its VPN connection quota:
/// "You've reached the limit on the number of VPN connections that you can create" (EC2 API
/// error codes).
pub const VPN_CONNECTION_LIMIT_EXCEEDED: &str = "VpnConnectionLimitExceeded";

/// Refuse a create that would take the store past [`MAX_STORED_RECORDS_PER_STORE`] VPN
/// connections: HTTP 400 [`VPN_CONNECTION_LIMIT_EXCEEDED`], worded the way
/// [`vpn_gateway_limit_exceeded`] words the gateway's own ceiling.
pub fn vpn_connection_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPN_CONNECTION_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of VPN connections has been reached \
             ({MAX_STORED_RECORDS_PER_STORE})"
        ),
    )
}

/// Refuse a value a VPN option member does not take: HTTP 400 `InvalidParameterValue`, naming
/// the member, the value the request sent and what that member publishes instead — the
/// wording this crate already refuses a value outside a member's published set with
/// (`create_customer_gateway`'s `ipAddress`).
///
/// One site for every such refusal of this slice, so the sentences cannot drift apart.
pub fn invalid_vpn_option_value<E>(
    parameter: &str,
    value: &dyn ::std::fmt::Display,
    expected: &str,
) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value ({value}) for parameter {parameter} is invalid. Expected: {expected}."
    ))
}

/// The same refusal for a member whose VALUE must not be echoed back: the pre-shared key is
/// the secret the two ends authenticate with (`TunnelOption$PreSharedKey` is `@sensitive`), so
/// what is reported is the member and its published constraint, never the key the request
/// sent.
pub fn invalid_vpn_option_form<E>(parameter: &str, expected: &str) -> Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value for parameter {parameter} is invalid. Expected: {expected}."
    ))
}

/// Whether `value` is a pre-shared key the API accepts: "Allowed characters are alphanumeric
/// characters, periods (.), and underscores (_). Must be between 8 and 64 characters in
/// length and cannot start with zero (0)"
/// (`VpnTunnelOptionsSpecification$PreSharedKey`) — every clause of that sentence, in the one
/// place this crate reads a pre-shared key.
pub fn pre_shared_key_is_well_formed(value: &str) -> bool {
    PRE_SHARED_KEY_CHARS.contains(&value.chars().count())
        && !value.starts_with('0')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_'))
}

/// What the published constraint on a pre-shared key says, for the refusal of one that
/// breaks it.
pub fn pre_shared_key_expectation() -> String {
    format!(
        "{minimum} to {maximum} alphanumeric characters, periods (.) or underscores (_), not \
         starting with zero (0)",
        minimum = PRE_SHARED_KEY_CHARS.start(),
        maximum = PRE_SHARED_KEY_CHARS.end(),
    )
}

/// A pre-shared key this service generates for a tunnel the request named none for.
///
/// Drawn from the operating system's randomness, never from a counter, a clock or anything in
/// the request: the key is the secret the two tunnel endpoints authenticate with, so a value
/// another caller could predict would not be one. It is [`GENERATED_PRE_SHARED_KEY_CHARS`]
/// characters of [`PRE_SHARED_KEY_ALPHABET`], which satisfies
/// [`pre_shared_key_is_well_formed`] by construction — the alphabet carries no `0`, so no
/// generated key starts with one.
pub fn generated_pre_shared_key() -> String {
    use ::ssh_key::rand_core::RngCore;
    let mut bytes = [0u8; GENERATED_PRE_SHARED_KEY_CHARS];
    ::ssh_key::rand_core::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| {
            let index = usize::from(*byte) % PRE_SHARED_KEY_ALPHABET.len();
            char::from(PRE_SHARED_KEY_ALPHABET.get(index).copied().unwrap_or(b'A'))
        })
        .collect()
}

/// Where the outside address of each tunnel of one connection comes from, which is what
/// `OutsideIpAddressType` names: "the type of IP address assigned to the outside interface of
/// the customer gateway device"
/// (`VpnConnectionOptionsSpecification$OutsideIpAddressType`).
///
/// Resolved by the handler — which is what reads the transport attachment the private form
/// needs — and handed to [`ComAmazonawsEc2Amazonec2State::insert_created_vpn_connection`],
/// which mints one address per tunnel from it.
#[derive(Debug, Clone)]
pub enum TunnelOutsideAddressSource {
    /// `PublicIpv4`, the published default: an Amazon-owned public IPv4 address.
    AmazonPublicIpv4,
    /// `Ipv6`: an Amazon-owned IPv6 address.
    AmazonIpv6,
    /// `PrivateIpv4`: a private IPv4 address of the VPC the transport transit gateway
    /// attachment attaches, carried here as that VPC's IPv4 CIDR block.
    VpcPrivateIpv4(String),
}

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_created_vpn_connection`] needs that it
/// does not mint itself: what the REQUEST said, after every published bound has been applied
/// to it.
#[derive(Debug, Clone)]
pub struct VpnConnectionSpec {
    /// "The ID of the customer gateway" (`CreateVpnConnectionRequest$CustomerGatewayId`),
    /// already resolved against the store.
    pub customer_gateway_id: String,
    /// "The type of VPN connection (ipsec.1)" (`CreateVpnConnectionRequest$Type`).
    pub connection_type: String,
    /// The virtual private gateway the connection terminates on, when the request named one.
    pub vpn_gateway_id: Option<String>,
    /// The transit gateway it terminates on instead, when the request named that.
    pub transit_gateway_id: Option<String>,
    /// "The ID of the VPN concentrator to associate with the VPN connection"
    /// (`CreateVpnConnectionRequest$VpnConcentratorId`), when the request named one.
    pub vpn_concentrator_id: Option<String>,
    /// The connection-level options, at the defaults the model publishes for the members the
    /// request left out ([`vpn_connection_options`]). Its `TunnelOptions` member is filled in
    /// by the write path from `tunnels` below.
    pub options: VpnConnectionOptions,
    /// One entry per tunnel ([`TUNNELS_PER_VPN_CONNECTION`]), each carrying the request's own
    /// values and the published defaults ([`requested_tunnel_options`]). The three members
    /// this service owns — the outside address, and the inside block and pre-shared key the
    /// request named none — are left empty for the write path to mint.
    pub tunnels: Vec<TunnelOption>,
    /// Where each tunnel's outside address comes from.
    pub outside_address_source: TunnelOutsideAddressSource,
    /// The tags of a `vpn-connection` tag specification.
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored VPN connection holds: its tags, the network blocks the
/// request named, and per tunnel its inside blocks, its pre-shared key and the log group ARNs
/// it logs to. Everything else on the record is derived here or bounded by the model, so it is
/// fixed-size.
///
/// The ONE definition of that charge: `CreateVpnConnection` asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_created_vpn_connection`] charges exactly it.
pub fn vpn_connection_payload_bytes(spec: &VpnConnectionSpec) -> usize {
    let text = |value: &Option<String>| value.as_deref().map_or(0, str::len);
    let options = &spec.options;
    let mut bytes = tag_list_bytes(&spec.tags)
        .saturating_add(text(&options.local_ipv4_network_cidr))
        .saturating_add(text(&options.remote_ipv4_network_cidr))
        .saturating_add(text(&options.local_ipv6_network_cidr))
        .saturating_add(text(&options.remote_ipv6_network_cidr))
        .saturating_add(text(&options.transport_transit_gateway_attachment_id));
    for tunnel in &spec.tunnels {
        bytes = bytes
            .saturating_add(text(&tunnel.tunnel_inside_cidr))
            .saturating_add(text(&tunnel.tunnel_inside_ipv6_cidr))
            .saturating_add(tunnel.pre_shared_key.as_deref().map_or(
                GENERATED_PRE_SHARED_KEY_CHARS,
                str::len,
            ));
        if let Some(log_options) = tunnel
            .log_options
            .as_ref()
            .and_then(|options| options.cloud_watch_log_options.as_ref())
        {
            bytes = bytes
                .saturating_add(text(&log_options.log_group_arn))
                .saturating_add(text(&log_options.bgp_log_group_arn));
        }
    }
    bytes
}

/// The connection-level options a create stores, out of the options the request named.
///
/// Every member the request left out takes the default its own documentation publishes, so a
/// connection reports the configuration it is really running with rather than an empty
/// structure: `EnableAcceleration` and `StaticRoutesOnly` "Default: `false`",
/// `TunnelInsideIpVersion` "Default: `ipv4`", the four network CIDRs their published defaults,
/// `OutsideIpAddressType` "Default: `PublicIpv4`" and `TunnelBandwidth` `standard`. A value
/// outside what a member publishes is refused rather than defaulted, and the two network
/// CIDRs of each family are stored in canonical form, the way every CIDR member of this
/// service is ("We modify the specified CIDR block to its canonical form",
/// `CreateVpcRequest$CidrBlock`).
///
/// Only the pair of the family the tunnels carry is reported: the IPv6 pair describes nothing
/// about a connection whose tunnels process IPv4, and the model gives a connection one
/// `TunnelInsideIpVersion`.
pub fn vpn_connection_options<E>(
    requested: Option<&VpnConnectionOptionsSpecification>,
) -> Result<VpnConnectionOptions, Scaffolded<E>> {
    let default = VpnConnectionOptionsSpecification::default();
    let requested = requested.unwrap_or(&default);
    let inside_ip_version = requested
        .tunnel_inside_ip_version
        .clone()
        .unwrap_or_else(|| TUNNEL_INSIDE_IP_VERSION_IPV4.to_string());
    let ipv6_tunnels = inside_ip_version == TUNNEL_INSIDE_IP_VERSION_IPV6;
    let outside_ip_address_type = requested
        .outside_ip_address_type
        .clone()
        .unwrap_or_else(|| DEFAULT_OUTSIDE_IP_ADDRESS_TYPE.to_string());
    if !OUTSIDE_IP_ADDRESS_TYPES.contains(&outside_ip_address_type.as_str()) {
        return Err(invalid_vpn_option_value(
            "outsideIpAddressType",
            &outside_ip_address_type,
            &format!("one of {}", OUTSIDE_IP_ADDRESS_TYPES.join(", ")),
        ));
    }
    // "Required if `OutsideIpAddressType` is set to `PrivateIpv4`"
    // (`TransportTransitGatewayAttachmentId`): a tunnel addressed privately is addressed out
    // of that attachment's own network, so a request asking for one without naming it names
    // no network to take the address from.
    if outside_ip_address_type == OUTSIDE_IP_ADDRESS_TYPE_PRIVATE_IPV4
        && requested.transport_transit_gateway_attachment_id.is_none()
    {
        return Err(missing_parameter(format!(
            "The request must contain the parameter transportTransitGatewayAttachmentId when \
             outsideIpAddressType is {OUTSIDE_IP_ADDRESS_TYPE_PRIVATE_IPV4}"
        )));
    }
    let network_cidr = |parameter: &str, value: Option<&str>, default: &str| match value {
        None => Ok(Some(default.to_string())),
        Some(value) => {
            let canonical = if ipv6_tunnels {
                canonical_ipv6_cidr(value)
            } else {
                canonical_ipv4_cidr(value)
            };
            canonical.map(Some).map_err(|_| {
                invalid_vpn_option_value(
                    parameter,
                    &value,
                    if ipv6_tunnels {
                        "an IPv6 CIDR block"
                    } else {
                        "an IPv4 CIDR block"
                    },
                )
            })
        }
    };
    let (local_ipv4_network_cidr, remote_ipv4_network_cidr) = if ipv6_tunnels {
        (None, None)
    } else {
        (
            network_cidr(
                "localIpv4NetworkCidr",
                requested.local_ipv4_network_cidr.as_deref(),
                DEFAULT_VPN_IPV4_NETWORK_CIDR,
            )?,
            network_cidr(
                "remoteIpv4NetworkCidr",
                requested.remote_ipv4_network_cidr.as_deref(),
                DEFAULT_VPN_IPV4_NETWORK_CIDR,
            )?,
        )
    };
    let (local_ipv6_network_cidr, remote_ipv6_network_cidr) = if ipv6_tunnels {
        (
            network_cidr(
                "localIpv6NetworkCidr",
                requested.local_ipv6_network_cidr.as_deref(),
                DEFAULT_VPN_IPV6_NETWORK_CIDR,
            )?,
            network_cidr(
                "remoteIpv6NetworkCidr",
                requested.remote_ipv6_network_cidr.as_deref(),
                DEFAULT_VPN_IPV6_NETWORK_CIDR,
            )?,
        )
    } else {
        (None, None)
    };
    Ok(VpnConnectionOptions {
        enable_acceleration: Some(requested.enable_acceleration.unwrap_or(false)),
        static_routes_only: Some(requested.static_routes_only.unwrap_or(false)),
        local_ipv4_network_cidr,
        remote_ipv4_network_cidr,
        local_ipv6_network_cidr,
        remote_ipv6_network_cidr,
        outside_ip_address_type: Some(outside_ip_address_type),
        transport_transit_gateway_attachment_id: requested
            .transport_transit_gateway_attachment_id
            .clone(),
        tunnel_inside_ip_version: Some(inside_ip_version),
        // Filled in by the write path, which is what mints the per-tunnel values.
        tunnel_options: None,
        tunnel_bandwidth: Some(
            requested
                .tunnel_bandwidth
                .clone()
                .unwrap_or_else(|| DEFAULT_VPN_TUNNEL_BANDWIDTH.to_string()),
        ),
    })
}

/// The tunnels a create brings up, out of the per-tunnel options the request named.
///
/// A connection has [`TUNNELS_PER_VPN_CONNECTION`] tunnels whatever the request says, so the
/// answer always carries that many: the request describes the first tunnels and the rest take
/// the published defaults. A request describing more tunnels than the connection has is
/// refused rather than silently truncated.
///
/// Every member a tunnel reports is either the request's own value, held to the constraints
/// that member publishes, or the default it publishes — except the three this service owns
/// (the outside address, and the inside block and pre-shared key of a tunnel the request named
/// none for), which the write path mints and which are left empty here. `inside_ip_version` is
/// the connection's, which decides which of the two inside blocks a tunnel carries.
pub fn requested_tunnel_options<E>(
    requested: Option<&[VpnTunnelOptionsSpecification]>,
    inside_ip_version: &str,
) -> Result<Vec<TunnelOption>, Scaffolded<E>> {
    let requested = requested.unwrap_or_default();
    if requested.len() > TUNNELS_PER_VPN_CONNECTION {
        return Err(invalid_vpn_option_value(
            "tunnelOptions",
            &requested.len(),
            &format!("at most {TUNNELS_PER_VPN_CONNECTION} tunnels"),
        ));
    }
    let ipv6_tunnels = inside_ip_version == TUNNEL_INSIDE_IP_VERSION_IPV6;
    let mut tunnels = Vec::with_capacity(TUNNELS_PER_VPN_CONNECTION);
    for index in 0..TUNNELS_PER_VPN_CONNECTION {
        let default = VpnTunnelOptionsSpecification::default();
        let tunnel = requested.get(index).unwrap_or(&default);
        tunnels.push(tunnel_option(tunnel, ipv6_tunnels)?);
    }
    Ok(tunnels)
}

/// One tunnel as it will be stored, out of the options `requested` named for it.
fn tunnel_option<E>(
    requested: &VpnTunnelOptionsSpecification,
    ipv6_tunnels: bool,
) -> Result<TunnelOption, Scaffolded<E>> {
    // The inside block of the family the connection's tunnels carry. A block of the OTHER
    // family describes no address this tunnel has, so it is refused rather than stored
    // alongside: the connection has one `TunnelInsideIpVersion`.
    let (tunnel_inside_cidr, tunnel_inside_ipv6_cidr) = match (
        ipv6_tunnels,
        requested.tunnel_inside_cidr.as_deref(),
        requested.tunnel_inside_ipv6_cidr.as_deref(),
    ) {
        (false, _, Some(_)) => {
            return Err(invalid_parameter_combination(
                "tunnelInsideIpv6Cidr names the inside addresses of a tunnel that processes \
                 IPv6 traffic, so it cannot be given for a connection whose \
                 tunnelInsideIpVersion is ipv4",
            ))
        }
        (true, Some(_), _) => {
            return Err(invalid_parameter_combination(
                "tunnelInsideCidr names the inside addresses of a tunnel that processes IPv4 \
                 traffic, so it cannot be given for a connection whose tunnelInsideIpVersion \
                 is ipv6",
            ))
        }
        (false, Some(requested), None) => (Some(requested_tunnel_inside_cidr(requested)?), None),
        (true, None, Some(requested)) => {
            (None, Some(requested_tunnel_inside_ipv6_cidr(requested)?))
        }
        // The service's own block, minted by the write path.
        (_, None, None) => (None, None),
    };
    if let Some(pre_shared_key) = requested.pre_shared_key.as_deref() {
        if !pre_shared_key_is_well_formed(pre_shared_key) {
            return Err(invalid_vpn_option_form(
                "preSharedKey",
                &pre_shared_key_expectation(),
            ));
        }
    }
    let phase1_lifetime_seconds = requested
        .phase1_lifetime_seconds
        .unwrap_or(DEFAULT_PHASE1_LIFETIME_SECONDS);
    if !PHASE1_LIFETIME_SECONDS.contains(&phase1_lifetime_seconds) {
        return Err(bounded_value(
            "phase1LifetimeSeconds",
            phase1_lifetime_seconds,
            *PHASE1_LIFETIME_SECONDS.start(),
            *PHASE1_LIFETIME_SECONDS.end(),
        ));
    }
    let phase2_lifetime_seconds = requested
        .phase2_lifetime_seconds
        .unwrap_or(DEFAULT_PHASE2_LIFETIME_SECONDS);
    if !PHASE2_LIFETIME_SECONDS.contains(&phase2_lifetime_seconds) {
        return Err(bounded_value(
            "phase2LifetimeSeconds",
            phase2_lifetime_seconds,
            *PHASE2_LIFETIME_SECONDS.start(),
            *PHASE2_LIFETIME_SECONDS.end(),
        ));
    }
    // "The value must be less than the value for `Phase1LifetimeSeconds`"
    // (`Phase2LifetimeSeconds`): phase 2 is renegotiated inside the phase 1 session, so a
    // lifetime that outlives it is no lifetime of this tunnel.
    if phase2_lifetime_seconds >= phase1_lifetime_seconds {
        return Err(invalid_vpn_option_value(
            "phase2LifetimeSeconds",
            &phase2_lifetime_seconds,
            &format!("a value below phase1LifetimeSeconds ({phase1_lifetime_seconds})"),
        ));
    }
    let rekey_margin_time_seconds = requested
        .rekey_margin_time_seconds
        .unwrap_or(DEFAULT_REKEY_MARGIN_TIME_SECONDS);
    // "A value between 60 and half of `Phase2LifetimeSeconds`" (`RekeyMarginTimeSeconds`):
    // the upper bound is the lifetime in force on THIS tunnel, not a constant.
    let max_rekey_margin_time_seconds = phase2_lifetime_seconds / 2;
    if rekey_margin_time_seconds < MIN_REKEY_MARGIN_TIME_SECONDS
        || rekey_margin_time_seconds > max_rekey_margin_time_seconds
    {
        return Err(bounded_value(
            "rekeyMarginTimeSeconds",
            rekey_margin_time_seconds,
            MIN_REKEY_MARGIN_TIME_SECONDS,
            max_rekey_margin_time_seconds,
        ));
    }
    let rekey_fuzz_percentage = requested
        .rekey_fuzz_percentage
        .unwrap_or(DEFAULT_REKEY_FUZZ_PERCENTAGE);
    if !REKEY_FUZZ_PERCENTAGE.contains(&rekey_fuzz_percentage) {
        return Err(bounded_value(
            "rekeyFuzzPercentage",
            rekey_fuzz_percentage,
            *REKEY_FUZZ_PERCENTAGE.start(),
            *REKEY_FUZZ_PERCENTAGE.end(),
        ));
    }
    let replay_window_size = requested
        .replay_window_size
        .unwrap_or(DEFAULT_REPLAY_WINDOW_SIZE);
    if !REPLAY_WINDOW_SIZE.contains(&replay_window_size) {
        return Err(bounded_value(
            "replayWindowSize",
            replay_window_size,
            *REPLAY_WINDOW_SIZE.start(),
            *REPLAY_WINDOW_SIZE.end(),
        ));
    }
    let dpd_timeout_seconds = requested
        .dpd_timeout_seconds
        .unwrap_or(DEFAULT_DPD_TIMEOUT_SECONDS);
    if dpd_timeout_seconds < MIN_DPD_TIMEOUT_SECONDS {
        return Err(invalid_vpn_option_value(
            "dpdTimeoutSeconds",
            &dpd_timeout_seconds,
            &format!("a value of at least {MIN_DPD_TIMEOUT_SECONDS}"),
        ));
    }
    let dpd_timeout_action = requested
        .dpd_timeout_action
        .clone()
        .unwrap_or_else(|| DEFAULT_DPD_TIMEOUT_ACTION.to_string());
    if !DPD_TIMEOUT_ACTIONS.contains(&dpd_timeout_action.as_str()) {
        return Err(one_of_value(
            "dpdTimeoutAction",
            &dpd_timeout_action,
            &DPD_TIMEOUT_ACTIONS,
        ));
    }
    let startup_action = requested
        .startup_action
        .clone()
        .unwrap_or_else(|| DEFAULT_STARTUP_ACTION.to_string());
    if !STARTUP_ACTIONS.contains(&startup_action.as_str()) {
        return Err(one_of_value(
            "startupAction",
            &startup_action,
            &STARTUP_ACTIONS,
        ));
    }
    Ok(TunnelOption {
        // Minted by the write path: an outside address is the service's to assign.
        outside_ip_address: None,
        tunnel_inside_cidr,
        tunnel_inside_ipv6_cidr,
        pre_shared_key: requested.pre_shared_key.clone(),
        phase1_lifetime_seconds: Some(phase1_lifetime_seconds),
        phase2_lifetime_seconds: Some(phase2_lifetime_seconds),
        rekey_margin_time_seconds: Some(rekey_margin_time_seconds),
        rekey_fuzz_percentage: Some(rekey_fuzz_percentage),
        replay_window_size: Some(replay_window_size),
        dpd_timeout_seconds: Some(dpd_timeout_seconds),
        dpd_timeout_action: Some(dpd_timeout_action),
        phase1_encryption_algorithms: Some(permitted_text(
            "phase1EncryptionAlgorithm",
            requested
                .phase1_encryption_algorithms
                .as_deref()
                .map(|values| values.iter().map(|value| value.value.clone()).collect()),
            &VPN_ENCRYPTION_ALGORITHMS,
        )?
        .into_iter()
        .map(|value| Phase1EncryptionAlgorithmsListValue { value: Some(value) })
        .collect()),
        phase2_encryption_algorithms: Some(permitted_text(
            "phase2EncryptionAlgorithm",
            requested
                .phase2_encryption_algorithms
                .as_deref()
                .map(|values| values.iter().map(|value| value.value.clone()).collect()),
            &VPN_ENCRYPTION_ALGORITHMS,
        )?
        .into_iter()
        .map(|value| Phase2EncryptionAlgorithmsListValue { value: Some(value) })
        .collect()),
        phase1_integrity_algorithms: Some(permitted_text(
            "phase1IntegrityAlgorithm",
            requested
                .phase1_integrity_algorithms
                .as_deref()
                .map(|values| values.iter().map(|value| value.value.clone()).collect()),
            &VPN_INTEGRITY_ALGORITHMS,
        )?
        .into_iter()
        .map(|value| Phase1IntegrityAlgorithmsListValue { value: Some(value) })
        .collect()),
        phase2_integrity_algorithms: Some(permitted_text(
            "phase2IntegrityAlgorithm",
            requested
                .phase2_integrity_algorithms
                .as_deref()
                .map(|values| values.iter().map(|value| value.value.clone()).collect()),
            &VPN_INTEGRITY_ALGORITHMS,
        )?
        .into_iter()
        .map(|value| Phase2IntegrityAlgorithmsListValue { value: Some(value) })
        .collect()),
        phase1_dh_group_numbers: Some(permitted_numbers(
            "phase1DHGroupNumber",
            requested
                .phase1_dh_group_numbers
                .as_deref()
                .map(|values| values.iter().map(|value| value.value).collect()),
            &PHASE1_DH_GROUP_NUMBERS,
        )?
        .into_iter()
        .map(|value| Phase1DHGroupNumbersListValue { value: Some(value) })
        .collect()),
        phase2_dh_group_numbers: Some(permitted_numbers(
            "phase2DHGroupNumber",
            requested
                .phase2_dh_group_numbers
                .as_deref()
                .map(|values| values.iter().map(|value| value.value).collect()),
            &PHASE2_DH_GROUP_NUMBERS,
        )?
        .into_iter()
        .map(|value| Phase2DHGroupNumbersListValue { value: Some(value) })
        .collect()),
        ike_versions: Some(permitted_text(
            "ikeVersion",
            requested
                .ike_versions
                .as_deref()
                .map(|values| values.iter().map(|value| value.value.clone()).collect()),
            &IKE_VERSIONS,
        )?
        .into_iter()
        .map(|value| IKEVersionsListValue { value: Some(value) })
        .collect()),
        startup_action: Some(startup_action),
        log_options: Some(VpnTunnelLogOptions {
            // "Options for logging VPN tunnel activity" (`LogOptions`), as the request asked
            // for them, with logging off when it asked for nothing — the default of a
            // `log_enabled` nothing set.
            cloud_watch_log_options: Some(tunnel_log_options(requested)),
        }),
        enable_tunnel_lifecycle_control: Some(
            requested.enable_tunnel_lifecycle_control.unwrap_or(false),
        ),
    })
}

/// The CloudWatch logging a tunnel reports, out of what the request asked for: each member as
/// given, and logging OFF when the request asked for none — "status of tunnel endpoint
/// lifecycle control feature" and the log flags are booleans a connection reports either way.
fn tunnel_log_options(requested: &VpnTunnelOptionsSpecification) -> CloudWatchLogOptions {
    let requested = requested
        .log_options
        .as_ref()
        .and_then(|options| options.cloud_watch_log_options.clone())
        .unwrap_or_default();
    CloudWatchLogOptions {
        log_enabled: Some(requested.log_enabled.unwrap_or(false)),
        log_group_arn: requested.log_group_arn,
        log_output_format: requested.log_output_format,
        bgp_log_enabled: Some(requested.bgp_log_enabled.unwrap_or(false)),
        bgp_log_group_arn: requested.bgp_log_group_arn,
        bgp_log_output_format: requested.bgp_log_output_format,
    }
}

/// Refuse a number outside the range its member publishes: the one wording for every such
/// bound of this slice.
fn bounded_value<E>(parameter: &str, value: i32, minimum: i32, maximum: i32) -> Scaffolded<E> {
    invalid_vpn_option_value(
        parameter,
        &value,
        &format!("a value between {minimum} and {maximum}"),
    )
}

/// Refuse a value outside the set its member publishes: the one wording for every such set of
/// this slice.
fn one_of_value<E>(parameter: &str, value: &str, permitted: &[&str]) -> Scaffolded<E> {
    invalid_vpn_option_value(
        parameter,
        &value,
        &format!("one of {}", permitted.join(", ")),
    )
}

/// The values a tunnel permits for one of its algorithm or version lists: the request's own,
/// each held to the published set, or the whole published set when the request named none —
/// "one or more encryption algorithms that are permitted for the VPN tunnel" is a permission
/// list, so naming none permits everything the tunnel supports.
fn permitted_text<E>(
    parameter: &str,
    requested: Option<Vec<Option<String>>>,
    published: &[&str],
) -> Result<Vec<String>, Scaffolded<E>> {
    let Some(requested) = requested else {
        return Ok(published.iter().map(|value| (*value).to_string()).collect());
    };
    let mut permitted = Vec::with_capacity(requested.len());
    for value in requested {
        let value = value.unwrap_or_default();
        if !published.contains(&value.as_str()) {
            return Err(one_of_value(parameter, &value, published));
        }
        if !permitted.contains(&value) {
            permitted.push(value);
        }
    }
    if permitted.is_empty() {
        return Ok(published.iter().map(|value| (*value).to_string()).collect());
    }
    Ok(permitted)
}

/// [`permitted_text`] for the two lists whose values are Diffie-Hellman group NUMBERS.
fn permitted_numbers<E>(
    parameter: &str,
    requested: Option<Vec<Option<i32>>>,
    published: &[i32],
) -> Result<Vec<i32>, Scaffolded<E>> {
    let Some(requested) = requested else {
        return Ok(published.to_vec());
    };
    let mut permitted = Vec::with_capacity(requested.len());
    for value in requested {
        let value = value.unwrap_or_default();
        if !published.contains(&value) {
            return Err(invalid_vpn_option_value(
                parameter,
                &value,
                &format!(
                    "one of {}",
                    published
                        .iter()
                        .map(i32::to_string)
                        .collect::<Vec<String>>()
                        .join(", ")
                ),
            ));
        }
        if !permitted.contains(&value) {
            permitted.push(value);
        }
    }
    if permitted.is_empty() {
        return Ok(published.to_vec());
    }
    Ok(permitted)
}

/// A requested inside IPv4 block, held to what its member publishes: "a size /30 CIDR block
/// from the `169.254.0.0/16` range", and none of the blocks that sentence lists as "reserved
/// and cannot be used".
///
/// The block must be the block itself rather than an address inside it — a value whose host
/// bits are set names no `/30` — so the canonical form of the request's value has to be the
/// value, unlike the CIDR members of this service that AWS canonicalises for the caller.
pub fn requested_tunnel_inside_cidr<E>(value: &str) -> Result<String, Scaffolded<E>> {
    let expected = format!(
        "a size /{TUNNEL_INSIDE_CIDR_PREFIX_LENGTH} CIDR block from the \
         {TUNNEL_INSIDE_CIDR_RANGE} range that is not reserved"
    );
    let Ok(canonical) = canonical_ipv4_cidr(value) else {
        return Err(invalid_vpn_option_value("tunnelInsideCidr", &value, &expected));
    };
    let sized = canonical
        .split_once('/')
        .and_then(|(_, prefix_length)| prefix_length.parse::<u8>().ok())
        == Some(TUNNEL_INSIDE_CIDR_PREFIX_LENGTH);
    if canonical != value
        || !sized
        || !ipv4_cidr_block_contains(TUNNEL_INSIDE_CIDR_RANGE, &canonical)
        || RESERVED_TUNNEL_INSIDE_CIDRS.contains(&canonical.as_str())
    {
        return Err(invalid_vpn_option_value("tunnelInsideCidr", &value, &expected));
    }
    Ok(canonical)
}

/// A requested inside IPv6 block, held to what its member publishes: "a size /126 CIDR block
/// from the local `fd00::/8` range", read the same way its IPv4 twin is.
pub fn requested_tunnel_inside_ipv6_cidr<E>(value: &str) -> Result<String, Scaffolded<E>> {
    let expected = format!(
        "a size /{TUNNEL_INSIDE_IPV6_CIDR_PREFIX_LENGTH} CIDR block from the \
         {TUNNEL_INSIDE_IPV6_CIDR_RANGE} range"
    );
    let Ok(canonical) = canonical_ipv6_cidr(value) else {
        return Err(invalid_vpn_option_value(
            "tunnelInsideIpv6Cidr",
            &value,
            &expected,
        ));
    };
    let sized = canonical
        .split_once('/')
        .and_then(|(_, prefix_length)| prefix_length.parse::<u8>().ok())
        == Some(TUNNEL_INSIDE_IPV6_CIDR_PREFIX_LENGTH);
    if !sized || !ipv6_cidr_block_contains(TUNNEL_INSIDE_IPV6_CIDR_RANGE, &canonical) {
        return Err(invalid_vpn_option_value(
            "tunnelInsideIpv6Cidr",
            &value,
            &expected,
        ));
    }
    Ok(canonical)
}

/// Refuse an inside block another tunnel of the account already uses: "any specified CIDR
/// blocks must be unique across all VPN connections that use the same virtual private
/// gateway" (`VpnTunnelOptionsSpecification$TunnelInsideCidr`), which this store holds to
/// across every connection it has.
pub fn tunnel_inside_cidr_in_use<E>(parameter: &str, value: &str) -> Scaffolded<E> {
    invalid_vpn_option_value(
        parameter,
        &value,
        "a CIDR block no other VPN tunnel of this account uses",
    )
}

/// Every text member of a stored tunnel, for the scans below: the two inside blocks.
fn tunnel_inside_blocks(record: &VpnConnectionRecord) -> impl Iterator<Item = &str> {
    record
        .view
        .options
        .iter()
        .flat_map(|options| options.tunnel_options.iter())
        .flatten()
        .flat_map(|tunnel| {
            tunnel
                .tunnel_inside_cidr
                .as_deref()
                .into_iter()
                .chain(tunnel.tunnel_inside_ipv6_cidr.as_deref())
        })
}

impl ComAmazonawsEc2Amazonec2State {
    /// The inside blocks, of either family, every tunnel this account holds is using.
    ///
    /// Read off the stored connections themselves rather than from a second index, so an
    /// inside block is in use exactly while a connection reports it.
    pub fn tunnel_inside_blocks_in_use(&self) -> BTreeSet<String> {
        self.vpn_connections
            .values()
            .flat_map(tunnel_inside_blocks)
            .map(str::to_string)
            .collect()
    }

    /// The VPN connection already running between the two gateways a create names, if this
    /// account holds one.
    ///
    /// "This is an idempotent operation. If you perform the operation more than once, Amazon
    /// EC2 doesn't return an error" (`CreateVpnConnection`): what identifies a connection is
    /// the pair of endpoints it runs between — the customer gateway and the ONE Amazon-side
    /// gateway — so a request naming a pair a connection already runs between is answered
    /// with that connection rather than with a second one. A connection on its way out
    /// (`deleting`, `deleted`) is no longer running between them, so it is not that answer.
    pub fn vpn_connection_between(
        &self,
        customer_gateway_id: &str,
        vpn_gateway_id: Option<&str>,
        transit_gateway_id: Option<&str>,
    ) -> Option<&VpnConnectionRecord> {
        self.vpn_connections.values().find(|record| {
            let view = &record.view;
            view.customer_gateway_id.as_deref() == Some(customer_gateway_id)
                && view.vpn_gateway_id.as_deref() == vpn_gateway_id
                && view.transit_gateway_id.as_deref() == transit_gateway_id
                && matches!(record.state(), VPN_STATE_PENDING | VPN_STATE_AVAILABLE)
        })
    }

    /// The next inside block of `family` no tunnel of this account uses, having taken the
    /// blocks in `taken` as used too — so the two tunnels of ONE create cannot be given the
    /// same block.
    ///
    /// The scan walks the published range in order and stops after
    /// [`MAX_TUNNEL_INSIDE_BLOCK_PROBES`] candidates, so it cannot spin; `None` is a store
    /// with no block left, which the caller answers rather than storing a tunnel without one.
    /// Reserved blocks are skipped, so a minted block is one
    /// [`requested_tunnel_inside_cidr`] would also accept.
    fn mint_tunnel_inside_block(&self, ipv6: bool, taken: &BTreeSet<String>) -> Option<String> {
        (0..MAX_TUNNEL_INSIDE_BLOCK_PROBES)
            .map(|index| {
                if ipv6 {
                    let base = (0xfd00u128 << 112) + u128::from(index) * 4;
                    format!(
                        "{}/{TUNNEL_INSIDE_IPV6_CIDR_PREFIX_LENGTH}",
                        ::std::net::Ipv6Addr::from(base)
                    )
                } else {
                    format!(
                        "169.254.{}.{}/{TUNNEL_INSIDE_CIDR_PREFIX_LENGTH}",
                        index / 64,
                        (index % 64) * 4,
                    )
                }
            })
            .find(|candidate| {
                !RESERVED_TUNNEL_INSIDE_CIDRS.contains(&candidate.as_str())
                    && !taken.contains(candidate)
            })
    }

    /// The outside address of one tunnel, from where the connection's `OutsideIpAddressType`
    /// says it comes.
    ///
    /// A public one is an Amazon-owned address, taken through the one minter of those
    /// ([`Self::mint_elastic_ip`]) so no address is handed out twice; an IPv6 one is the first
    /// address of an Amazon-provided block, from the one minter of those; a private one comes
    /// out of the transport attachment's own VPC, through the one scan every VPC address
    /// comes out of ([`Self::mint_private_ip_in_cidr`]).
    fn mint_tunnel_outside_address(
        &mut self,
        source: &TunnelOutsideAddressSource,
    ) -> Option<String> {
        match source {
            TunnelOutsideAddressSource::AmazonPublicIpv4 => self.mint_elastic_ip(),
            TunnelOutsideAddressSource::AmazonIpv6 => {
                let block = self.mint_amazon_ipv6_cidr();
                block.split_once('/').map(|(address, _)| address.to_string())
            }
            TunnelOutsideAddressSource::VpcPrivateIpv4(cidr_block) => {
                self.mint_private_ip_in_cidr(cidr_block)
            }
        }
    }

    /// Store the VPN connection a create describes and answer the `VpnConnectionId` it was
    /// given — or `None` when the store is full or has no tunnel address or inside block left,
    /// in which case NOTHING was stored.
    ///
    /// The ONE write path for a created connection, so everything this service owns is
    /// derived here rather than taken from a caller: the id, the two tunnels' outside
    /// addresses, the inside block and pre-shared key of a tunnel the request named none for,
    /// the telemetry each tunnel reports, the customer gateway configuration document, the
    /// `pending` state a connection is brought up in with the `available` the clock then owes
    /// it, and the gateway association that comes up with it. The caller-chosen bytes are
    /// charged against the store's byte budget here too, so a writer cannot store them
    /// uncharged.
    pub fn insert_created_vpn_connection(&mut self, spec: VpnConnectionSpec) -> Option<String> {
        if !self.has_vpn_connection_capacity() {
            return None;
        }
        let charge = STORED_RECORD_OVERHEAD_BYTES.saturating_add(vpn_connection_payload_bytes(&spec));
        let ipv6_tunnels = spec.options.tunnel_inside_ip_version.as_deref()
            == Some(TUNNEL_INSIDE_IP_VERSION_IPV6);
        // The blocks already in use, plus the ones this create hands to its own tunnels: two
        // tunnels of one connection never share an inside block.
        let mut taken = self.tunnel_inside_blocks_in_use();
        let mut tunnels: Vec<TunnelOption> = Vec::with_capacity(spec.tunnels.len());
        for mut tunnel in spec.tunnels {
            let block = match (ipv6_tunnels, tunnel.tunnel_inside_cidr.clone(), tunnel.tunnel_inside_ipv6_cidr.clone()) {
                (false, Some(block), _) | (true, _, Some(block)) => block,
                _ => self.mint_tunnel_inside_block(ipv6_tunnels, &taken)?,
            };
            taken.insert(block.clone());
            if ipv6_tunnels {
                tunnel.tunnel_inside_ipv6_cidr = Some(block);
            } else {
                tunnel.tunnel_inside_cidr = Some(block);
            }
            if tunnel.pre_shared_key.is_none() {
                tunnel.pre_shared_key = Some(generated_pre_shared_key());
            }
            tunnel.outside_ip_address =
                Some(self.mint_tunnel_outside_address(&spec.outside_address_source)?);
            tunnels.push(tunnel);
        }
        // The telemetry of each tunnel, keyed to the address it was just given: a created
        // connection's tunnels are DOWN until one end negotiates IPSec, carry no route yet,
        // and their last status change is the instant they came into being.
        let last_status_change = system_time_to_date_time(SystemTime::now());
        let vgw_telemetry: Vec<VgwTelemetry> = tunnels
            .iter()
            .map(|tunnel| VgwTelemetry {
                accepted_route_count: Some(0),
                last_status_change: Some(last_status_change),
                outside_ip_address: tunnel.outside_ip_address.clone(),
                status: Some(TELEMETRY_STATUS_DOWN.to_string()),
                status_message: Some(TELEMETRY_STATUS_MESSAGE_DOWN.to_string()),
                certificate_arn: None,
            })
            .collect();
        let mut options = spec.options;
        options.tunnel_options = Some(tunnels);
        let vpn_connection_id = self.next_id(VPN_CONNECTION_ID_PREFIX);
        let mut view = VpnConnection {
            vpn_connection_id: Some(vpn_connection_id.clone()),
            category: Some(VPN_CONNECTION_CATEGORY_VPN.to_string()),
            customer_gateway_id: Some(spec.customer_gateway_id),
            vpn_gateway_id: spec.vpn_gateway_id,
            transit_gateway_id: spec.transit_gateway_id,
            vpn_concentrator_id: spec.vpn_concentrator_id,
            r#type: Some(spec.connection_type),
            // The connection is being brought up, which is the non-terminal `VpnState` a
            // create answers; the clock owes it `available` one TRANSITION_DELAY later, off
            // the same monotonic clock as every other move this store owes.
            state: Some(VPN_STATE_PENDING.to_string()),
            // The association with the Amazon-side gateway is being made at the same time,
            // and reaches `associated` when the connection does.
            gateway_association_state: Some(GATEWAY_ASSOCIATION_STATE_ASSOCIATING.to_string()),
            options: Some(options),
            // "The static routes associated with the VPN connection" (`Routes`): a connection
            // is created with none — `CreateVpnConnectionRoute` is what adds one.
            routes: Some(Vec::new()),
            vgw_telemetry: Some(vgw_telemetry),
            tags: Some(spec.tags),
            customer_gateway_configuration: None,
            core_network_arn: None,
            core_network_attachment_arn: None,
            pre_shared_key_arn: None,
        };
        // The configuration document is built from the connection as it now stands, together
        // with the two devices it runs between, so what the caller hands its network
        // administrator describes the tunnels this store really holds.
        view.customer_gateway_configuration = Some(self.customer_gateway_configuration(&view));
        self.add_stored_bytes(charge);
        self.vpn_connections.insert(
            vpn_connection_id.clone(),
            VpnConnectionRecord {
                view,
                route_moves: BTreeMap::new(),
                pending: Some(PendingStateMove::starting_now(VPN_STATE_AVAILABLE)),
            },
        );
        Some(vpn_connection_id)
    }

    /// "The configuration information for the VPN connection's customer gateway (in the
    /// native XML format)" (`VpnConnection$CustomerGatewayConfiguration`), built out of the
    /// connection `view` and the two devices it runs between.
    ///
    /// Every value in it is read off this store — the connection's own ids and type, each
    /// tunnel's outside address, the two usable addresses of its inside block, its pre-shared
    /// key and the IKE and IPSec parameters it was created with, the customer gateway's
    /// outside address and BGP ASN, and the Amazon-side ASN of the virtual private gateway —
    /// so the document describes the tunnels the store holds rather than a template. The
    /// first usable address of a tunnel's inside block is the Amazon side's and the second is
    /// the customer gateway's.
    fn customer_gateway_configuration(&self, view: &VpnConnection) -> String {
        let customer_gateway = view
            .customer_gateway_id
            .as_deref()
            .and_then(|customer_gateway_id| self.customer_gateways.get(customer_gateway_id));
        let customer_gateway_address = customer_gateway
            .and_then(|record| record.view.ip_address.clone())
            .unwrap_or_default();
        let customer_gateway_asn = customer_gateway
            .and_then(|record| {
                record
                    .view
                    .bgp_asn
                    .clone()
                    .or_else(|| record.view.bgp_asn_extended.clone())
            })
            .unwrap_or_default();
        let amazon_side_asn = view
            .vpn_gateway_id
            .as_deref()
            .and_then(|vpn_gateway_id| self.vpn_gateways.get(vpn_gateway_id))
            .and_then(|record| record.view.amazon_side_asn)
            .map(|asn| asn.to_string())
            .unwrap_or_default();
        // "Indicates whether the VPN connection uses static routes only. Static routes must
        // be used for devices that don't support BGP" (`StaticRoutesOnly`), which is what the
        // document's own attributes element says about the connection.
        let static_routes_only = view
            .options
            .as_ref()
            .and_then(|options| options.static_routes_only)
            .unwrap_or(false);
        let attributes = if static_routes_only {
            "NoBGPVPNConnection"
        } else {
            "BGPVPNConnection"
        };
        let mut document = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let id = xml_text(view.vpn_connection_id.as_deref().unwrap_or_default());
        let customer_gateway_id =
            xml_text(view.customer_gateway_id.as_deref().unwrap_or_default());
        // The Amazon-side gateway of the connection, whichever kind it terminates on.
        let gateway_id = xml_text(
            view.vpn_gateway_id
                .as_deref()
                .or(view.transit_gateway_id.as_deref())
                .unwrap_or_default(),
        );
        let connection_type = xml_text(view.r#type.as_deref().unwrap_or_default());
        for line in [
            format!("<vpn_connection id=\"{id}\">"),
            format!("  <customer_gateway_id>{customer_gateway_id}</customer_gateway_id>"),
            format!("  <vpn_gateway_id>{gateway_id}</vpn_gateway_id>"),
            format!("  <vpn_connection_type>{connection_type}</vpn_connection_type>"),
            format!("  <vpn_connection_attributes>{attributes}</vpn_connection_attributes>"),
        ] {
            document.push_str(&line);
            document.push('\n');
        }
        for tunnel in view
            .options
            .iter()
            .flat_map(|options| options.tunnel_options.iter())
            .flatten()
        {
            document.push_str(&ipsec_tunnel_element(
                tunnel,
                &customer_gateway_address,
                &customer_gateway_asn,
                &amazon_side_asn,
            ));
        }
        document.push_str("</vpn_connection>\n");
        document
    }
}

/// One `<ipsec_tunnel>` of the configuration document: the two ends of the tunnel, the
/// pre-shared key they authenticate with, and the IKE and IPSec parameters the tunnel carries.
fn ipsec_tunnel_element(
    tunnel: &TunnelOption,
    customer_gateway_address: &str,
    customer_gateway_asn: &str,
    amazon_side_asn: &str,
) -> String {
    let inside_block = tunnel
        .tunnel_inside_cidr
        .as_deref()
        .or(tunnel.tunnel_inside_ipv6_cidr.as_deref())
        .unwrap_or_default();
    let (amazon_inside_address, customer_inside_address) = inside_addresses(inside_block);
    let prefix_length = inside_block
        .split_once('/')
        .map(|(_, prefix_length)| prefix_length)
        .unwrap_or_default();
    let phase1_lifetime = tunnel.phase1_lifetime_seconds.unwrap_or_default();
    let phase2_lifetime = tunnel.phase2_lifetime_seconds.unwrap_or_default();
    let first = |values: Option<&String>| xml_text(values.map(String::as_str).unwrap_or_default());
    let phase1_encryption = first(
        tunnel
            .phase1_encryption_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.as_ref()),
    );
    let phase2_encryption = first(
        tunnel
            .phase2_encryption_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.as_ref()),
    );
    let phase1_integrity = first(
        tunnel
            .phase1_integrity_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.as_ref()),
    );
    let phase2_integrity = first(
        tunnel
            .phase2_integrity_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.as_ref()),
    );
    let phase1_dh_group = tunnel
        .phase1_dh_group_numbers
        .as_deref()
        .and_then(|values| values.first())
        .and_then(|value| value.value)
        .unwrap_or_default();
    let phase2_dh_group = tunnel
        .phase2_dh_group_numbers
        .as_deref()
        .and_then(|values| values.first())
        .and_then(|value| value.value)
        .unwrap_or_default();
    let ike_version = first(
        tunnel
            .ike_versions
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.as_ref()),
    );
    let customer_outside = xml_text(customer_gateway_address);
    let amazon_outside = xml_text(tunnel.outside_ip_address.as_deref().unwrap_or_default());
    let customer_gateway_asn = xml_text(customer_gateway_asn);
    let amazon_side_asn = xml_text(amazon_side_asn);
    let pre_shared_key = xml_text(tunnel.pre_shared_key.as_deref().unwrap_or_default());
    let dpd_timeout = tunnel.dpd_timeout_seconds.unwrap_or_default();
    let dpd_action = xml_text(tunnel.dpd_timeout_action.as_deref().unwrap_or_default());
    // One element per line, in the order the native configuration lays them out: the two
    // ends of the tunnel, then what they negotiate with.
    let lines: Vec<String> = vec![
        "  <ipsec_tunnel>".to_string(),
        "    <customer_gateway>".to_string(),
        "      <tunnel_outside_address>".to_string(),
        format!("        <ip_address>{customer_outside}</ip_address>"),
        "      </tunnel_outside_address>".to_string(),
        "      <tunnel_inside_address>".to_string(),
        format!("        <ip_address>{customer_inside_address}</ip_address>"),
        format!("        <network_cidr>{prefix_length}</network_cidr>"),
        "      </tunnel_inside_address>".to_string(),
        "      <bgp>".to_string(),
        format!("        <asn>{customer_gateway_asn}</asn>"),
        "      </bgp>".to_string(),
        "    </customer_gateway>".to_string(),
        "    <vpn_gateway>".to_string(),
        "      <tunnel_outside_address>".to_string(),
        format!("        <ip_address>{amazon_outside}</ip_address>"),
        "      </tunnel_outside_address>".to_string(),
        "      <tunnel_inside_address>".to_string(),
        format!("        <ip_address>{amazon_inside_address}</ip_address>"),
        format!("        <network_cidr>{prefix_length}</network_cidr>"),
        "      </tunnel_inside_address>".to_string(),
        "      <bgp>".to_string(),
        format!("        <asn>{amazon_side_asn}</asn>"),
        "      </bgp>".to_string(),
        "    </vpn_gateway>".to_string(),
        "    <ike>".to_string(),
        format!(
            "      <authentication_protocol>{phase1_integrity}</authentication_protocol>"
        ),
        format!("      <encryption_protocol>{phase1_encryption}</encryption_protocol>"),
        format!("      <lifetime>{phase1_lifetime}</lifetime>"),
        format!(
            "      <perfect_forward_secrecy>group{phase1_dh_group}</perfect_forward_secrecy>"
        ),
        format!("      <ike_version>{ike_version}</ike_version>"),
        format!("      <pre_shared_key>{pre_shared_key}</pre_shared_key>"),
        "    </ike>".to_string(),
        "    <ipsec>".to_string(),
        format!(
            "      <authentication_protocol>{phase2_integrity}</authentication_protocol>"
        ),
        format!("      <encryption_protocol>{phase2_encryption}</encryption_protocol>"),
        format!("      <lifetime>{phase2_lifetime}</lifetime>"),
        format!(
            "      <perfect_forward_secrecy>group{phase2_dh_group}</perfect_forward_secrecy>"
        ),
        "      <dead_peer_detection>".to_string(),
        format!("        <interval>{dpd_timeout}</interval>"),
        format!("        <action>{dpd_action}</action>"),
        "      </dead_peer_detection>".to_string(),
        "    </ipsec>".to_string(),
        "  </ipsec_tunnel>".to_string(),
    ];
    let mut element = lines.join("\n");
    element.push('\n');
    element
}

/// The two usable addresses of a tunnel's inside block: the Amazon side's, which is the first
/// address after the block's own, and the customer gateway's, the one after that.
///
/// A block this store cannot read yields no addresses rather than a guess.
fn inside_addresses(block: &str) -> (String, String) {
    let Some((address, _)) = block.split_once('/') else {
        return (String::new(), String::new());
    };
    if let Ok(base) = address.parse::<::std::net::Ipv4Addr>() {
        let base = u32::from(base);
        return (
            ::std::net::Ipv4Addr::from(base.saturating_add(1)).to_string(),
            ::std::net::Ipv4Addr::from(base.saturating_add(2)).to_string(),
        );
    }
    if let Ok(base) = address.parse::<Ipv6Addr>() {
        let base = u128::from(base);
        return (
            Ipv6Addr::from(base.saturating_add(1)).to_string(),
            Ipv6Addr::from(base.saturating_add(2)).to_string(),
        );
    }
    (String::new(), String::new())
}

/// `text` as the character data of an XML element.
///
/// Every value the configuration document carries comes from a charset this module already
/// holds its input to — an id this store minted, an address, an ASN, a pre-shared key of
/// `PreSharedKey`'s own charset — so nothing in it needs escaping today. It goes through this
/// one function anyway, so a member added to the document later cannot put a caller's `<` into
/// it.
fn xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}
