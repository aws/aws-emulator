// SPDX-License-Identifier: CC0-1.0

//! What ONE tunnel of a Site-to-Site VPN connection reports about itself: the tunnel a
//! request names by its outside IP address, the security parameters that tunnel has
//! negotiated, and the provisioning status it is in.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there. MAKING a connection and its tunnels lives in `vpn_connection_creation`;
//! the connection record itself and its states live in `nat_gateways_and_vpn_connections`.
//!
//! A tunnel is brought up with the LISTS of values it may negotiate with — "valid values"
//! per phase for the encryption and integrity algorithms, the Diffie-Hellman group numbers
//! and the IKE versions (`VpnTunnelOptionsSpecification$Phase1EncryptionAlgorithms` and its
//! siblings) — while what it has negotiated is ONE value per parameter. The negotiated value
//! is the FIRST of the tunnel's permitted list, which is the value the connection's customer
//! gateway configuration document already publishes for that tunnel
//! (`VpnConnection$CustomerGatewayConfiguration`), so both readers of a tunnel's negotiated
//! parameters read them here and cannot disagree about one tunnel.

use super::*;
use crate::ops::types::{ActiveVpnTunnelStatus, TunnelOption};

/// The `VpnTunnelProvisioningStatus` of a tunnel whose provisioning has finished: that enum's
/// `available` member (`smithy_query(query="shape",shape="VpnTunnelProvisioningStatus")`).
///
/// The status a tunnel of a live connection carries: a tunnel's endpoints are provisioned with
/// the connection itself ([`active_vpn_tunnel_status`]).
pub const VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE: &str = "available";

/// Refuse a VPN connection id this account holds no connection for, as the TUNNEL surface of
/// that resource answers it: HTTP 400 [`INVALID_VPN_CONNECTION_ID_NOT_FOUND`] naming the
/// resource `vpn-connection`.
///
/// NOT [`invalid_vpn_connection_id_not_found`], and the recorded wire corpus is why: the
/// operations that act on the CONNECTION name the resource `vpnConnection` in this sentence,
/// while the tunnel surface names it `vpn-connection`. A client reads the sentence it is
/// actually answered, so the two wordings of this one code are kept apart rather than folded
/// into one. The id is the request's own.
pub fn invalid_vpn_connection_id_on_tunnel_surface<E>(vpn_connection_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        "vpn-connection ID",
        vpn_connection_id,
    )
}

/// Refuse an outside IP address the named connection has no tunnel at: HTTP 400
/// `InvalidParameterValue`, worded by the one place this crate refuses a value a VPN member
/// does not take ([`invalid_vpn_option_value`]).
///
/// Every value that names no tunnel of the connection is answered the same way — an address
/// of another connection's tunnel, an address of nothing at all, and a value that is not an
/// address — because what the request got wrong is the same thing in each case: the member
/// takes "the external IP address of the VPN tunnel"
/// (`GetActiveVpnTunnelStatusRequest$VpnTunnelOutsideIpAddress`) of the connection it also
/// names. The message is built from the request: the value it sent and the connection it
/// asked about.
pub fn invalid_vpn_tunnel_outside_ip_address<E>(
    vpn_connection_id: &str,
    value: &str,
) -> Scaffolded<E> {
    invalid_vpn_option_value(
        "vpnTunnelOutsideIpAddress",
        &value,
        &format!(
            "the outside IP address of a tunnel of the VPN connection {vpn_connection_id}"
        ),
    )
}

/// The tunnels of one connection, and the one way a tunnel of it is named.
impl VpnConnectionRecord {
    /// "The tunnel options for the VPN connection" (`VpnConnectionOptions$TunnelOptions`) —
    /// the tunnels this connection was brought up with, in the order they were.
    pub fn tunnels(&self) -> &[TunnelOption] {
        self.view
            .options
            .as_ref()
            .and_then(|options| options.tunnel_options.as_deref())
            .unwrap_or_default()
    }

    /// The tunnel of this connection whose outside interface is addressed
    /// `outside_ip_address`, if it has one.
    ///
    /// The outside address is what every tunnel-scoped operation of this service names a
    /// tunnel by ("The external IP address of the VPN tunnel",
    /// `GetActiveVpnTunnelStatusRequest$VpnTunnelOutsideIpAddress`), and this store hands no
    /// address to two tunnels, so it identifies at most one tunnel of the account.
    pub fn tunnel(&self, outside_ip_address: &str) -> Option<&TunnelOption> {
        self.tunnels()
            .iter()
            .find(|tunnel| tunnel.outside_ip_address.as_deref() == Some(outside_ip_address))
    }
}

/// The security parameters one tunnel has negotiated: ONE value per parameter, out of the
/// lists of values the tunnel may negotiate with.
///
/// Every member is `None` exactly when the tunnel carries no permitted value for that
/// parameter at all, so a reader reports what the tunnel holds rather than a filled-in
/// default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NegotiatedTunnelParameters {
    /// "The version of the Internet Key Exchange (IKE) protocol being used"
    /// (`ActiveVpnTunnelStatus$IkeVersion`).
    pub ike_version: Option<String>,
    /// "The encryption algorithm negotiated in Phase 1 IKE negotiations"
    /// (`ActiveVpnTunnelStatus$Phase1EncryptionAlgorithm`).
    pub phase1_encryption_algorithm: Option<String>,
    /// The same for Phase 2 (`ActiveVpnTunnelStatus$Phase2EncryptionAlgorithm`).
    pub phase2_encryption_algorithm: Option<String>,
    /// "The integrity algorithm negotiated in Phase 1 IKE negotiations"
    /// (`ActiveVpnTunnelStatus$Phase1IntegrityAlgorithm`).
    pub phase1_integrity_algorithm: Option<String>,
    /// The same for Phase 2 (`ActiveVpnTunnelStatus$Phase2IntegrityAlgorithm`).
    pub phase2_integrity_algorithm: Option<String>,
    /// "The Diffie-Hellman group number being used in Phase 1 IKE negotiations"
    /// (`ActiveVpnTunnelStatus$Phase1DHGroup`).
    pub phase1_dh_group: Option<i32>,
    /// The same for Phase 2 (`ActiveVpnTunnelStatus$Phase2DHGroup`).
    pub phase2_dh_group: Option<i32>,
}

/// What `tunnel` has negotiated, read off the lists it was brought up with.
///
/// The ONE place this crate decides which of a tunnel's permitted values is the negotiated
/// one: the first of each list, which for a tunnel the request named no list for is the first
/// value that member publishes ([`VPN_ENCRYPTION_ALGORITHMS`],
/// [`VPN_INTEGRITY_ALGORITHMS`], [`PHASE1_DH_GROUP_NUMBERS`], [`PHASE2_DH_GROUP_NUMBERS`],
/// [`IKE_VERSIONS`]) and for a tunnel brought up with a narrowed list is the first value of
/// that list. So narrowing a list to one value makes that value the negotiated one, and the
/// configuration document the connection hands its network administrator describes the same
/// negotiation this reports.
pub fn negotiated_tunnel_parameters(tunnel: &TunnelOption) -> NegotiatedTunnelParameters {
    NegotiatedTunnelParameters {
        ike_version: tunnel
            .ike_versions
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.clone()),
        phase1_encryption_algorithm: tunnel
            .phase1_encryption_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.clone()),
        phase2_encryption_algorithm: tunnel
            .phase2_encryption_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.clone()),
        phase1_integrity_algorithm: tunnel
            .phase1_integrity_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.clone()),
        phase2_integrity_algorithm: tunnel
            .phase2_integrity_algorithms
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value.clone()),
        phase1_dh_group: tunnel
            .phase1_dh_group_numbers
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value),
        phase2_dh_group: tunnel
            .phase2_dh_group_numbers
            .as_deref()
            .and_then(|values| values.first())
            .and_then(|value| value.value),
    }
}

/// The status of `tunnel` as a tunnel of the connection `record` holds — `None` when that
/// connection is in no state in which a tunnel of it is provisioned at all, in which case
/// there is no active tunnel to report and the caller refuses the request.
///
/// A tunnel's own endpoints are provisioned WITH the connection: a tunnel of a connection that
/// has just been created reports [`VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE`], so nothing here
/// is owed on the clock. The connection's own `pending` -> `available` move is about the
/// CONNECTION, and whether the tunnel has brought an IPSec session up is what the tunnel's
/// telemetry reports (`VgwTelemetry$Status`), not this member. So
/// "the reason for the current provisioning status"
/// (`ActiveVpnTunnelStatus$ProvisioningStatusReason`) is carried by no status this operation
/// reports: the member says why a status is not the provisioned one, and nothing here
/// un-provisions a tunnel of a live connection. The negotiated parameters are the tunnel's own
/// ([`negotiated_tunnel_parameters`]): what it was brought up to negotiate with.
pub fn active_vpn_tunnel_status(
    record: &VpnConnectionRecord,
    tunnel: &TunnelOption,
) -> Option<ActiveVpnTunnelStatus> {
    let (provisioning_status, provisioning_status_reason) = match record.state() {
        VPN_STATE_PENDING | VPN_STATE_AVAILABLE => {
            (VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE, None)
        }
        // `deleting` and `deleted` are the states of a connection whose tunnels are being, or
        // have been, torn down: it holds no ACTIVE tunnel for this to be the status of.
        _ => return None,
    };
    let negotiated = negotiated_tunnel_parameters(tunnel);
    Some(ActiveVpnTunnelStatus {
        ike_version: negotiated.ike_version,
        phase1_encryption_algorithm: negotiated.phase1_encryption_algorithm,
        phase2_encryption_algorithm: negotiated.phase2_encryption_algorithm,
        phase1_integrity_algorithm: negotiated.phase1_integrity_algorithm,
        phase2_integrity_algorithm: negotiated.phase2_integrity_algorithm,
        phase1_dh_group: negotiated.phase1_dh_group,
        phase2_dh_group: negotiated.phase2_dh_group,
        provisioning_status: Some(provisioning_status.to_string()),
        provisioning_status_reason,
    })
}
