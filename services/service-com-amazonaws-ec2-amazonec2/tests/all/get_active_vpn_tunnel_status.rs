// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetActiveVpnTunnelStatus`, driven over the wire the service actually
//! speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. THE STATUS REPORTED IS THE NAMED TUNNEL'S OWN NEGOTIATION: a tunnel is brought up with
//!    the LISTS of values it may negotiate with — the request's own, or everything its member
//!    publishes when the request named none — and what it has negotiated is the FIRST value of
//!    each of those lists, reported per tunnel, so two tunnels of one connection report what
//!    each of them was brought up with rather than one shared answer.
//! 2. A TUNNEL OF A LIVE CONNECTION IS A PROVISIONED TUNNEL: a tunnel's endpoints come up with
//!    the connection, so its provisioning status is the provisioned one from the moment the
//!    create answers and carries no reason, and the connection's own clock-owed
//!    `pending` -> `available` move leaves the tunnel's status exactly as it was.
//! 3. A REQUEST THAT NAMES NO TUNNEL OF A CONNECTION IS REFUSED, not answered with a status:
//!    an id the account holds no connection for, an outside address that is another
//!    connection's, nothing's, or no address at all, and an empty field in either member.
//!    A connection whose tunnels are being torn down has no active tunnel either.
//! 4. THE READ CHANGES NOTHING: two calls report the same status, the connection reads back
//!    through `DescribeVpnConnections` exactly as it did before, and `DryRun` reports the
//!    outcome without answering a status.
//!
//! Each table row is a distinct input along what those rules generalise over: which of a
//! tunnel's lists the request narrows, to how many values and to which of them, which of the
//! connection's two tunnels is narrowed, which address family the tunnels process, and which
//! form the refused value takes.

use ec2::state::{
    DRY_RUN_OPERATION, IKE_VERSIONS, INCORRECT_STATE, INVALID_PARAMETER_VALUE,
    INVALID_VPN_CONNECTION_ID_NOT_FOUND, MISSING_PARAMETER, PHASE1_DH_GROUP_NUMBERS,
    PHASE2_DH_GROUP_NUMBERS, VPN_ENCRYPTION_ALGORITHMS, VPN_INTEGRITY_ALGORITHMS,
    VPN_STATE_AVAILABLE, VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::rbin_retention_rules::poll_until;
use super::wire::{child, child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The outside address of the customer gateway device every connection here runs to, from the
/// documentation range `TEST-NET-3` (RFC 5737), so no row names a routable address.
const CUSTOMER_GATEWAY_ADDRESS: &str = "203.0.113.12";

/// The BGP ASN of that device, inside the range `CreateCustomerGatewayRequest$BgpAsn`
/// publishes.
const CUSTOMER_GATEWAY_BGP_ASN: &str = "65001";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
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
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    first_text(case, &body, id_member)
}

/// The two devices a connection runs between: a customer gateway at `ip_address` and a
/// virtual private gateway, both of this account.
async fn gateway_pair(case: &str, router: &MultiServiceRouter, ip_address: &str) -> (String, String) {
    let customer_gateway_id = create_resource(
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
    .await;
    let vpn_gateway_id = create_resource(
        case,
        router,
        "CreateVpnGateway",
        &[param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1)],
        "vpnGatewayId",
    )
    .await;
    (customer_gateway_id, vpn_gateway_id)
}

/// The security parameters one tunnel has negotiated, as text in the form the wire carries
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Negotiated {
    ike_version: String,
    phase1_encryption_algorithm: String,
    phase2_encryption_algorithm: String,
    phase1_integrity_algorithm: String,
    phase2_integrity_algorithm: String,
    phase1_dh_group: String,
    phase2_dh_group: String,
}

/// What a tunnel the request narrowed no list for has negotiated: the FIRST value of each
/// list that tunnel's member publishes, read off the published catalogs this crate holds
/// (`VpnTunnelOptionsSpecification$Phase1EncryptionAlgorithms` and its siblings) rather than
/// copied into this suite.
fn published_firsts() -> Negotiated {
    Negotiated {
        ike_version: IKE_VERSIONS[0].to_string(),
        phase1_encryption_algorithm: VPN_ENCRYPTION_ALGORITHMS[0].to_string(),
        phase2_encryption_algorithm: VPN_ENCRYPTION_ALGORITHMS[0].to_string(),
        phase1_integrity_algorithm: VPN_INTEGRITY_ALGORITHMS[0].to_string(),
        phase2_integrity_algorithm: VPN_INTEGRITY_ALGORITHMS[0].to_string(),
        phase1_dh_group: PHASE1_DH_GROUP_NUMBERS[0].to_string(),
        phase2_dh_group: PHASE2_DH_GROUP_NUMBERS[0].to_string(),
    }
}

/// `published_firsts` with the members `narrowed` names replaced, so a row says only what it
/// changed about one tunnel.
fn with(narrowed: Negotiated, apply: impl FnOnce(&mut Negotiated)) -> Negotiated {
    let mut negotiated = narrowed;
    apply(&mut negotiated);
    negotiated
}

/// One tunnel as the create reported it: the address it is named by, and the first value of
/// each list it may negotiate with — which is what the status must report.
#[derive(Debug, Clone)]
struct CreatedTunnel {
    outside_ip_address: String,
    first_of_each_list: Negotiated,
}

/// The text values of every `<value>` inside the `name` set of `node`.
fn set_values(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<String> {
    set_items(node, name)
        .into_iter()
        .map(|item| child_text(item, "value").unwrap_or_default())
        .collect()
}

/// The first value of the `name` set of `node`, or the empty string when it carries none.
fn first_value(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    set_values(node, name).into_iter().next().unwrap_or_default()
}

/// One `CreateVpnConnection` carrying `extra`, requiring success: the connection's id and the
/// tunnels it was brought up with, in the order the response reports them.
async fn create_connection(
    case: &str,
    router: &MultiServiceRouter,
    extra: &[Param],
) -> (String, Vec<CreatedTunnel>) {
    let (customer_gateway_id, vpn_gateway_id) =
        gateway_pair(case, router, CUSTOMER_GATEWAY_ADDRESS).await;
    let mut params = vec![
        param("CustomerGatewayId", &customer_gateway_id),
        param("Type", ec2::state::GATEWAY_TYPE_IPSEC_1),
        param("VpnGatewayId", &vpn_gateway_id),
    ];
    params.extend(extra.iter().cloned());
    let (status, body) = invoke(router, "CreateVpnConnection", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpnConnection, got {status} with body \
         {body}"
    );
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "vpnConnectionId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no <vpnConnectionId> in {body}"));
    let vpn_connection_id = child_text(node, "vpnConnectionId").unwrap_or_default();
    let options = child(node, "options")
        .unwrap_or_else(|| panic!("case {case:?}: no <options> in {body}"));
    let tunnels: Vec<CreatedTunnel> = set_items(options, "tunnelOptionSet")
        .into_iter()
        .map(|item| CreatedTunnel {
            outside_ip_address: child_text(item, "outsideIpAddress").unwrap_or_default(),
            first_of_each_list: Negotiated {
                ike_version: first_value(item, "ikeVersionSet"),
                phase1_encryption_algorithm: first_value(item, "phase1EncryptionAlgorithmSet"),
                phase2_encryption_algorithm: first_value(item, "phase2EncryptionAlgorithmSet"),
                phase1_integrity_algorithm: first_value(item, "phase1IntegrityAlgorithmSet"),
                phase2_integrity_algorithm: first_value(item, "phase2IntegrityAlgorithmSet"),
                phase1_dh_group: first_value(item, "phase1DHGroupNumberSet"),
                phase2_dh_group: first_value(item, "phase2DHGroupNumberSet"),
            },
        })
        .collect();
    assert_eq!(
        tunnels.len(),
        2,
        "case {case:?}: a Site-to-Site VPN connection is brought up with two tunnels, got \
         {tunnels:?} from {body}"
    );
    (vpn_connection_id, tunnels)
}

/// Look again until `DescribeVpnConnections` reports the connection in `state`.
///
/// Waits for the CONDITION the clock owes rather than for the span it owes it in, so a loaded
/// host cannot read the connection before the move has been applied.
async fn poll_until_connection_state(
    case: &str,
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    state: &str,
) {
    let waiting_for = format!("the VPN connection {vpn_connection_id} to reach {state}");
    poll_until(case, &waiting_for, || async {
        let (status, body) = invoke(
            router,
            "DescribeVpnConnections",
            &[param("VpnConnectionId.1", vpn_connection_id)],
        )
        .await;
        status == 200 && element(&body, "state").as_deref() == Some(state)
    })
    .await;
}

/// One tunnel's status as `GetActiveVpnTunnelStatus` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Status {
    negotiated: Negotiated,
    provisioning_status: String,
    provisioning_status_reason: Option<String>,
}

/// One `GetActiveVpnTunnelStatus` call, as its HTTP status and XML body.
async fn get_status(
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    vpn_tunnel_outside_ip_address: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![
        param("VpnConnectionId", vpn_connection_id),
        param("VpnTunnelOutsideIpAddress", vpn_tunnel_outside_ip_address),
    ];
    params.extend(extra.iter().cloned());
    invoke(router, "GetActiveVpnTunnelStatus", &params).await
}

/// The status of one tunnel, requiring success.
async fn status_of(
    case: &str,
    router: &MultiServiceRouter,
    vpn_connection_id: &str,
    outside_ip_address: &str,
) -> Status {
    let (http_status, body) = get_status(router, vpn_connection_id, outside_ip_address, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected HTTP 200 from GetActiveVpnTunnelStatus for \
         {outside_ip_address}, got {http_status} with body {body}"
    );
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "activeVpnTunnelStatus")
        .unwrap_or_else(|| panic!("case {case:?}: no <activeVpnTunnelStatus> in {body}"));
    let text = |name: &str| child_text(node, name).unwrap_or_default();
    Status {
        negotiated: Negotiated {
            ike_version: text("ikeVersion"),
            phase1_encryption_algorithm: text("phase1EncryptionAlgorithm"),
            phase2_encryption_algorithm: text("phase2EncryptionAlgorithm"),
            phase1_integrity_algorithm: text("phase1IntegrityAlgorithm"),
            phase2_integrity_algorithm: text("phase2IntegrityAlgorithm"),
            phase1_dh_group: text("phase1DHGroup"),
            phase2_dh_group: text("phase2DHGroup"),
        },
        provisioning_status: text("provisioningStatus"),
        provisioning_status_reason: child_text(node, "provisioningStatusReason"),
    }
}

// ---------------------------------------------------------------------------
// RULE 1: the status is the named tunnel's own negotiation
// ---------------------------------------------------------------------------

/// One row of the negotiation table: the tunnel options the create names, and what each of
/// the connection's two tunnels must then report.
struct NegotiationCase {
    /// What makes this row a different input.
    case: &'static str,
    /// The extra `CreateVpnConnection` fields, which is how a row narrows a tunnel's lists.
    extra: Vec<Param>,
    /// What the first and the second tunnel of the connection must report.
    expected: [Negotiated; 2],
}

/// The rows: nothing narrowed, every list of one tunnel narrowed to a single value, the two
/// tunnels narrowed differently, lists narrowed to SEVERAL values whose first is not the
/// published first, and tunnels that process IPv6 traffic. Every value a row names is a member
/// of the set its own member publishes, including entries no recorded request named.
fn negotiation_cases() -> Vec<NegotiationCase> {
    vec![
        NegotiationCase {
            case: "no list narrowed, so each tunnel may negotiate everything published",
            extra: Vec::new(),
            expected: [published_firsts(), published_firsts()],
        },
        NegotiationCase {
            case: "every list of the first tunnel narrowed to one value",
            extra: vec![
                param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev2"),
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                    "AES256",
                ),
                param(
                    "Options.TunnelOptions.1.Phase2EncryptionAlgorithm.1.Value",
                    "AES256-GCM-16",
                ),
                param(
                    "Options.TunnelOptions.1.Phase1IntegrityAlgorithm.1.Value",
                    "SHA2-512",
                ),
                param(
                    "Options.TunnelOptions.1.Phase2IntegrityAlgorithm.1.Value",
                    "SHA2-384",
                ),
                param("Options.TunnelOptions.1.Phase1DHGroupNumber.1.Value", "24"),
                param("Options.TunnelOptions.1.Phase2DHGroupNumber.1.Value", "5"),
            ],
            expected: [
                Negotiated {
                    ike_version: "ikev2".to_string(),
                    phase1_encryption_algorithm: "AES256".to_string(),
                    phase2_encryption_algorithm: "AES256-GCM-16".to_string(),
                    phase1_integrity_algorithm: "SHA2-512".to_string(),
                    phase2_integrity_algorithm: "SHA2-384".to_string(),
                    phase1_dh_group: "24".to_string(),
                    phase2_dh_group: "5".to_string(),
                },
                published_firsts(),
            ],
        },
        NegotiationCase {
            case: "the two tunnels narrowed differently, one list each",
            extra: vec![
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                    "AES128-GCM-16",
                ),
                param("Options.TunnelOptions.2.Phase1DHGroupNumber.1.Value", "20"),
            ],
            expected: [
                with(published_firsts(), |negotiated| {
                    negotiated.phase1_encryption_algorithm = "AES128-GCM-16".to_string();
                }),
                with(published_firsts(), |negotiated| {
                    negotiated.phase1_dh_group = "20".to_string();
                }),
            ],
        },
        NegotiationCase {
            case: "lists narrowed to several values, the first of each being the negotiated one",
            extra: vec![
                param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev2"),
                param("Options.TunnelOptions.1.IKEVersion.2.Value", "ikev1"),
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.1.Value",
                    "AES256",
                ),
                param(
                    "Options.TunnelOptions.1.Phase1EncryptionAlgorithm.2.Value",
                    "AES128",
                ),
                param("Options.TunnelOptions.1.Phase1DHGroupNumber.1.Value", "21"),
                param("Options.TunnelOptions.1.Phase1DHGroupNumber.2.Value", "2"),
                param(
                    "Options.TunnelOptions.2.Phase2IntegrityAlgorithm.1.Value",
                    "SHA2-256",
                ),
                param(
                    "Options.TunnelOptions.2.Phase2IntegrityAlgorithm.2.Value",
                    "SHA1",
                ),
            ],
            expected: [
                with(published_firsts(), |negotiated| {
                    negotiated.ike_version = "ikev2".to_string();
                    negotiated.phase1_encryption_algorithm = "AES256".to_string();
                    negotiated.phase1_dh_group = "21".to_string();
                }),
                with(published_firsts(), |negotiated| {
                    negotiated.phase2_integrity_algorithm = "SHA2-256".to_string();
                }),
            ],
        },
        NegotiationCase {
            case: "tunnels that process IPv6 traffic, the first one narrowed",
            extra: vec![
                param("Options.TunnelInsideIpVersion", "ipv6"),
                param(
                    "Options.TunnelOptions.1.Phase2EncryptionAlgorithm.1.Value",
                    "AES128-GCM-16",
                ),
                param("Options.TunnelOptions.1.IKEVersion.1.Value", "ikev2"),
            ],
            expected: [
                with(published_firsts(), |negotiated| {
                    negotiated.phase2_encryption_algorithm = "AES128-GCM-16".to_string();
                    negotiated.ike_version = "ikev2".to_string();
                }),
                published_firsts(),
            ],
        },
    ]
}

#[tokio::test]
async fn the_status_reports_what_the_named_tunnel_was_brought_up_to_negotiate() {
    for NegotiationCase {
        case,
        extra,
        expected,
    } in negotiation_cases()
    {
        let (router, _engine) = super::ec2_query::emulator();
        let (vpn_connection_id, tunnels) = create_connection(case, &router, &extra).await;

        for (index, tunnel) in tunnels.iter().enumerate() {
            let address = &tunnel.outside_ip_address;
            assert!(
                !address.is_empty(),
                "case {case:?}: tunnel {index} was brought up with no outside address: \
                 {tunnel:?}"
            );
            let actual = status_of(case, &router, &vpn_connection_id, address).await;
            let wanted = &expected[index];
            assert_eq!(
                &actual.negotiated, wanted,
                "case {case:?}: tunnel {index} at {address} must report {wanted:?}, got \
                 {actual:?}"
            );
            // The same answer, read off the lists the create itself reported for THIS tunnel:
            // the negotiated value is the first value of each list the tunnel carries.
            assert_eq!(
                actual.negotiated, tunnel.first_of_each_list,
                "case {case:?}: tunnel {index} at {address} must report the first value of \
                 each list it carries ({:?}), got {:?}",
                tunnel.first_of_each_list, actual.negotiated
            );
        }

        // Per tunnel, not per connection: a row that narrowed one tunnel's lists reports a
        // different negotiation for each of the two.
        if expected[0] != expected[1] {
            let first = status_of(case, &router, &vpn_connection_id, &tunnels[0].outside_ip_address)
                .await;
            let second =
                status_of(case, &router, &vpn_connection_id, &tunnels[1].outside_ip_address).await;
            assert_ne!(
                first.negotiated, second.negotiated,
                "case {case:?}: the two tunnels were brought up differently, so they cannot \
                 report one negotiation: {first:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a tunnel of a live connection is a provisioned tunnel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tunnel_of_a_live_connection_reports_a_provisioned_tunnel() {
    let case = "a tunnel of a connection being brought up";
    let (router, _engine) = super::ec2_query::emulator();
    let (vpn_connection_id, tunnels) = create_connection(case, &router, &[]).await;
    let address = tunnels[0].outside_ip_address.clone();

    // A tunnel's endpoints are provisioned with the connection itself, so a caller reading
    // straight after the create \u2014 while the CONNECTION is still coming up \u2014 is told the tunnel
    // is provisioned, with no reason to carry.
    let created = status_of(case, &router, &vpn_connection_id, &address).await;
    assert_eq!(
        created.provisioning_status, VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE,
        "case {case:?}: expected {VPN_TUNNEL_PROVISIONING_STATUS_AVAILABLE} straight after the \
         create, got {created:?}"
    );
    assert_eq!(
        created.provisioning_status_reason, None,
        "case {case:?}: a provisioned tunnel has no reason to report, got {created:?}"
    );

    // The connection's own `pending` -> `available` move falls due on the clock, with nothing
    // but time passing; this waits for that CONDITION rather than for a span.
    poll_until_connection_state(case, &router, &vpn_connection_id, VPN_STATE_AVAILABLE).await;

    // The tunnel's status is the same across it: that move is about the CONNECTION.
    let settled = status_of(case, &router, &vpn_connection_id, &address).await;
    assert_eq!(
        settled, created,
        "case {case:?}: expected the tunnel to report {created:?} once the connection is in \
         service, got {settled:?}"
    );
    assert_eq!(
        settled.negotiated, tunnels[0].first_of_each_list,
        "case {case:?}: expected {:?}, got {:?}",
        tunnels[0].first_of_each_list, settled.negotiated
    );
}

// ---------------------------------------------------------------------------
// RULE 3: what the operation refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the two members as the request sends them, and the EC2 code
/// and sentence it must be refused with. `{id}` in a value stands for the connection the
/// account holds, and in a message for whichever value was sent.
struct RefusalCase {
    /// What makes this row a different input.
    case: &'static str,
    /// `VpnConnectionId` as the request sends it — the held connection's id when `None`.
    vpn_connection_id: Option<&'static str>,
    /// `VpnTunnelOutsideIpAddress` as the request sends it — the held tunnel's address when
    /// `None`.
    outside_ip_address: Option<&'static str>,
    /// The `<Code>` EC2 answers.
    code: &'static str,
}

/// The rows: every form a value that names no tunnel of a connection of the account can take,
/// in either member, plus the empty field in each, which names nothing at all.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a long-form connection id nothing holds",
            vpn_connection_id: Some("vpn-0ffffffffffffffff"),
            outside_ip_address: None,
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a short-form connection id nothing holds",
            vpn_connection_id: Some("vpn-0abcd123"),
            outside_ip_address: None,
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a connection id of another resource kind",
            vpn_connection_id: Some("vgw-0123456789abcdef0"),
            outside_ip_address: None,
            code: INVALID_VPN_CONNECTION_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an empty connection id",
            vpn_connection_id: Some(""),
            outside_ip_address: None,
            code: MISSING_PARAMETER,
        },
        RefusalCase {
            case: "an outside address no tunnel of the account has",
            vpn_connection_id: None,
            outside_ip_address: Some("198.51.100.77"),
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an IPv6 outside address on a connection whose tunnels are addressed IPv4",
            vpn_connection_id: None,
            outside_ip_address: Some("2001:db8::1"),
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an outside address that is no address at all",
            vpn_connection_id: None,
            outside_ip_address: Some("the first tunnel"),
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            case: "an empty outside address",
            vpn_connection_id: None,
            outside_ip_address: Some(""),
            code: MISSING_PARAMETER,
        },
    ]
}

#[tokio::test]
async fn a_request_that_names_no_tunnel_of_a_connection_is_refused() {
    let case = "the connection the refusals are asked against";
    let (router, _engine) = super::ec2_query::emulator();
    let (held_id, tunnels) = create_connection(case, &router, &[]).await;
    let held_address = tunnels[0].outside_ip_address.clone();
    // A second connection of the same account, so "an address of another connection's tunnel"
    // is a row about resolution rather than about an address nothing has.
    let (_other_id, other_tunnels) = create_connection("another connection", &router, &[]).await;
    let other_address = other_tunnels[0].outside_ip_address.clone();

    for RefusalCase {
        case,
        vpn_connection_id,
        outside_ip_address,
        code,
    } in refusal_cases()
    {
        let requested_id = vpn_connection_id.unwrap_or(&held_id);
        let requested_address = outside_ip_address.unwrap_or(&held_address);
        let (status, body) = get_status(&router, requested_id, requested_address, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected the code {code}, got {actual} in {body}"
        );
        let expected = if requested_id.is_empty() {
            "The request must contain the parameter vpnConnectionId".to_string()
        } else if requested_address.is_empty() {
            "The request must contain the parameter vpnTunnelOutsideIpAddress".to_string()
        } else if code == INVALID_VPN_CONNECTION_ID_NOT_FOUND {
            format!("The vpn-connection ID '{requested_id}' does not exist")
        } else {
            format!(
                "Value ({requested_address}) for parameter vpnTunnelOutsideIpAddress is \
                 invalid. Expected: the outside IP address of a tunnel of the VPN connection \
                 {requested_id}."
            )
        };
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?} in {body}"
        );
    }

    // The tunnel of ANOTHER connection of the account: the address exists, but not on the
    // connection the request named, so it names no tunnel of it either.
    let case = "the outside address of another connection's tunnel";
    let (status, body) = get_status(&router, &held_id, &other_address, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INVALID_PARAMETER_VALUE,
        "case {case:?}: expected the code {INVALID_PARAMETER_VALUE}, got {actual} in {body}"
    );

    // And the pair that does resolve is still answered after all those refusals.
    let actual = status_of(case, &router, &held_id, &held_address).await;
    assert_eq!(
        actual.negotiated, tunnels[0].first_of_each_list,
        "case {case:?}: expected {:?}, got {:?}",
        tunnels[0].first_of_each_list, actual.negotiated
    );
}

#[tokio::test]
async fn a_connection_whose_tunnels_are_going_away_has_no_active_tunnel() {
    let case = "a tunnel of a connection a delete is taking away";
    let (router, _engine) = super::ec2_query::emulator();
    let (vpn_connection_id, tunnels) = create_connection(case, &router, &[]).await;
    let address = tunnels[0].outside_ip_address.clone();

    // Answered while the connection is there.
    status_of(case, &router, &vpn_connection_id, &address).await;

    let (status, body) = invoke(
        &router,
        "DeleteVpnConnection",
        &[param("VpnConnectionId", &vpn_connection_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteVpnConnection, got {status} with body \
         {body}"
    );

    let (status, body) = get_status(&router, &vpn_connection_id, &address, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 once the tunnels are being torn down, got {status} \
         with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, INCORRECT_STATE,
        "case {case:?}: expected the code {INCORRECT_STATE}, got {actual} in {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the read changes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_read_changes_nothing_a_later_caller_sees() {
    let case = "two reads of one tunnel";
    let (router, _engine) = super::ec2_query::emulator();
    let (vpn_connection_id, tunnels) = create_connection(case, &router, &[]).await;
    let address = tunnels[0].outside_ip_address.clone();

    /// The connection as `DescribeVpnConnections` reports it, as one comparable text.
    async fn described(
        case: &str,
        router: &MultiServiceRouter,
        vpn_connection_id: &str,
    ) -> String {
        let (status, body) = invoke(
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
        let parsed = document(case, &body);
        let node = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "vpnConnectionId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no connection in {body}"));
        node.descendants()
            .filter(|node| node.is_element())
            .map(|node| {
                format!(
                    "{}={};",
                    node.tag_name().name(),
                    node.text().unwrap_or_default()
                )
            })
            .collect()
    }

    let before = described(case, &router, &vpn_connection_id).await;
    let first = status_of(case, &router, &vpn_connection_id, &address).await;
    let second = status_of(case, &router, &vpn_connection_id, &address).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of one tunnel report the same status, got {first:?} then \
         {second:?}"
    );
    let after = described(case, &router, &vpn_connection_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: the read must leave the connection exactly as it was"
    );

    // "Checks whether you have the required permissions for the action, without actually
    // making the request" (`DryRun`): the outcome, never a status.
    let case = "a dry run of a read that would have succeeded";
    let (status, body) = get_status(
        &router,
        &vpn_connection_id,
        &address,
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, DRY_RUN_OPERATION,
        "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {actual} in {body}"
    );
    assert!(
        !body.contains("activeVpnTunnelStatus"),
        "case {case:?}: a dry run answers no status, got {body}"
    );

    // A dry run naming a pair that resolves to no tunnel is refused for the pair: the flag
    // asks whether the caller is permitted, not whether the request is about something that
    // exists.
    let case = "a dry run naming an address nothing holds";
    let (status, body) = get_status(
        &router,
        &vpn_connection_id,
        "198.51.100.200",
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: the permission question is answered before the lookup, got {status} \
         with body {body}"
    );
    let actual = error_code(&body);
    assert_eq!(
        actual, DRY_RUN_OPERATION,
        "case {case:?}: expected the code {DRY_RUN_OPERATION}, got {actual} in {body}"
    );

    // The status the dry run asked about is still there afterwards.
    let actual = status_of(case, &router, &vpn_connection_id, &address).await;
    assert_eq!(
        actual, first,
        "case {case:?}: a dry run writes nothing, got {actual:?} after {first:?}"
    );
}
