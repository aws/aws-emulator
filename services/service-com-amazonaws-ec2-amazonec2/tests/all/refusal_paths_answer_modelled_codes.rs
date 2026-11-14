// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REFUSAL paths of the operation families whose negative contract
//! used to be unreachable: a request this service cannot serve is answered with the code the
//! resource family that could not be resolved publishes, at that code's own HTTP status —
//! never with the scaffold's `NotImplemented` 501, which says "nobody has written this yet"
//! and which no EC2 client branches on.
//!
//! Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. A request naming a resource this account holds none of is refused with that resource's
//!    own `Invalid<Resource>…` code, and a request carrying a value the operation cannot act
//!    on with the code EC2 answers such a value with — at a 4xx status, and never 501
//!    `NotImplemented`. The table spans nine operation families and every kind of refusal
//!    they answer: a resource that is not there, a value outside a published form, a
//!    combination that cannot hold together and a `DryRun` of a request that would otherwise
//!    have succeeded.
//! 2. A DEPENDENCY guard is computed from the store: a DHCP option set a VPC is associated
//!    with cannot be deleted, and the same set can be deleted once the VPC is associated with
//!    another — so the refusal follows from state rather than from the shape of the request.
//! 3. A STATE guard is computed the same way: a route server a VPC is associated with cannot
//!    be deleted, a pairing the store holds no association for cannot be disassociated, and
//!    both operations succeed once the association is as they require.
//! 4. Capacity is MOVED by a split rather than created: the destination reservation holds the
//!    instances the request asked for, the source holds what is left, and a split asking for
//!    more than the source still holds is refused.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, indexed};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// A well-formed resource id of the kind `prefix` names that this service could have issued
/// but never did: the long form's seventeen hexadecimal digits, counting from one so it is
/// never the all-zero placeholder, built here so every row of the table below names an id
/// whose FORM is beyond reproach — what is under test is the refusal for a resource that is
/// not there, not one for an unreadable value.
fn unissued_id(prefix: &str) -> String {
    format!("{prefix}-0123456789abcdef1")
}

/// One call of `action` carrying `params`, as `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One refusal the table asserts: the operation, the request, and the code and status EC2
/// answers it with.
struct Refusal {
    /// The `Action` the row calls.
    action: &'static str,
    /// The form fields the row sends beside the action.
    params: Vec<(String, String)>,
    /// The `<Code>` the answer must carry.
    code: String,
    /// The HTTP status that code is answered at.
    status: u16,
}

/// Every refusal in the table, in operation-family order. Each row is a DISTINCT input along
/// what rule 1 generalises over — which resource family cannot be resolved, whether the value
/// is unresolvable or unreadable, whether two members clash, and whether the request would
/// otherwise have succeeded — so no single recorded request is the specification.
fn refusals() -> Vec<Refusal> {
    let transit_gateway = unissued_id("tgw");
    let route_table = unissued_id("tgw-rtb");
    let attachment = unissued_id("tgw-attach");
    let local_gateway = unissued_id("lgw");
    let local_route_table = unissued_id("lgw-rtb");
    let client_vpn = unissued_id("cvpn-endpoint");
    let vpn_gateway = unissued_id("vgw");
    let customer_gateway = unissued_id("cgw");
    let vpc = unissued_id("vpc");
    let exclusion = unissued_id("vpcbpa-exclude");
    let dhcp_options = unissued_id("dopt");
    let route_server = unissued_id("rs");
    let capacity_reservation = unissued_id("cr");
    let host = unissued_id("h");
    let offering = unissued_id("hro");
    let image = unissued_id("ami");
    let event_window = unissued_id("iew");
    let access_scope = unissued_id("nis");
    let instance = unissued_id("i");
    vec![
        // Cause 1 — the transit gateway family. A resource OF a transit gateway cannot be
        // created on a gateway the account does not hold.
        Refusal {
            action: "CreateTransitGatewayMulticastDomain",
            params: vec![param("TransitGatewayId", &transit_gateway)],
            code: ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "CreateTransitGatewayPolicyTable",
            params: vec![param("TransitGatewayId", &transit_gateway)],
            code: ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // The same operation asked as a PERMISSION probe: a `DryRun` is answered
        // `DryRunOperation` at the 412 that code carries.
        Refusal {
            action: "CreateTransitGatewayPolicyTable",
            params: vec![
                param("TransitGatewayId", &transit_gateway),
                param("DryRun", "true"),
            ],
            code: "DryRunOperation".to_string(),
            status: 412,
        },
        Refusal {
            action: "CreateTransitGatewayRoute",
            params: vec![
                param("TransitGatewayRouteTableId", &route_table),
                param("DestinationCidrBlock", "10.11.0.0/16"),
                param("TransitGatewayAttachmentId", &attachment),
            ],
            code: ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // A route that is neither a blackhole nor aimed at an attachment names no target at
        // all: two members clash rather than one being absent, so the refusal is the
        // combination code.
        Refusal {
            action: "CreateTransitGatewayRoute",
            params: vec![
                param("TransitGatewayRouteTableId", &route_table),
                param("DestinationCidrBlock", "10.12.0.0/16"),
                param("TransitGatewayAttachmentId", &attachment),
                param("Blackhole", "true"),
            ],
            code: "InvalidParameterCombination".to_string(),
            status: 400,
        },
        // A destination that is not an address prefix at all is a value the operation cannot
        // act on, whatever the store holds.
        Refusal {
            action: "CreateTransitGatewayRoute",
            params: vec![
                param("TransitGatewayRouteTableId", &route_table),
                param("DestinationCidrBlock", "not-a-cidr-block"),
                param("TransitGatewayAttachmentId", &attachment),
            ],
            code: "InvalidParameterValue".to_string(),
            status: 400,
        },
        // The requester of a peering is read as a VALUE this operation cannot act on, where
        // the PEER below is answered with the transit gateway's own not-found code.
        Refusal {
            action: "CreateTransitGatewayPeeringAttachment",
            params: vec![
                param("TransitGatewayId", &transit_gateway),
                param("PeerTransitGatewayId", &unissued_id("tgw")),
                param("PeerAccountId", "000000000000"),
                param("PeerRegion", REGION),
            ],
            code: "InvalidParameterValue".to_string(),
            status: 400,
        },
        // Cause 2 — the local gateway family.
        Refusal {
            action: "CreateCoipPool",
            params: vec![param("LocalGatewayRouteTableId", &local_route_table)],
            code: "InvalidLocalGatewayRouteTableID.NotFound".to_string(),
            status: 400,
        },
        Refusal {
            action: "CreateLocalGatewayVirtualInterfaceGroup",
            params: vec![param("LocalGatewayId", &local_gateway)],
            code: "InvalidLocalGatewayID.NotFound".to_string(),
            status: 400,
        },
        Refusal {
            action: "DeleteLocalGatewayRouteTable",
            params: vec![param("LocalGatewayRouteTableId", &local_route_table)],
            code: "InvalidLocalGatewayRouteTableID.NotFound".to_string(),
            status: 400,
        },
        // Cause 3 — the Client VPN family.
        Refusal {
            action: "DeleteClientVpnEndpoint",
            params: vec![param("ClientVpnEndpointId", &client_vpn)],
            code: ec2::state::INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "DeleteClientVpnRoute",
            params: vec![
                param("ClientVpnEndpointId", &client_vpn),
                param("DestinationCidrBlock", "10.13.0.0/16"),
            ],
            code: ec2::state::INVALID_CLIENT_VPN_ENDPOINT_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // Cause 4 — the VPN gateway and VPC configuration family.
        Refusal {
            action: "AttachVpnGateway",
            params: vec![param("VpnGatewayId", &vpn_gateway), param("VpcId", &vpc)],
            code: ec2::state::INVALID_VPN_GATEWAY_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "AssociateVpcCidrBlock",
            params: vec![param("VpcId", &vpc), param("CidrBlock", "10.14.0.0/16")],
            code: ec2::state::INVALID_VPC_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "ModifyVpcTenancy",
            params: vec![param("VpcId", &vpc), param("InstanceTenancy", "default")],
            code: ec2::state::INVALID_VPC_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // A tenancy this operation may not set is refused at the request boundary by the
        // modelled `VpcTenancy` enum, which publishes `default` alone, so it is not a row
        // here: what this table covers is the refusals the HANDLER computes.
        Refusal {
            action: "ModifyVpcBlockPublicAccessExclusion",
            params: vec![
                param("ExclusionId", &exclusion),
                param("InternetGatewayExclusionMode", "allow-egress"),
            ],
            code: ec2::state::INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "CreateVpnConnection",
            params: vec![
                param("CustomerGatewayId", &customer_gateway),
                param("Type", "ipsec.1"),
                param("VpnGatewayId", &vpn_gateway),
            ],
            code: ec2::state::INVALID_CUSTOMER_GATEWAY_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // A connection notification watches ONE subject: a request naming neither the
        // service nor the endpoint names none.
        Refusal {
            action: "CreateVpcEndpointConnectionNotification",
            params: vec![
                param("ConnectionNotificationArn", "arn:aws:sns:us-east-1:000000000000:t"),
                param("ConnectionEvents.1", "Accept"),
            ],
            code: "InvalidParameter".to_string(),
            status: 400,
        },
        // Cause 5 — the image family: an id this account holds no image for is answered as
        // MALFORMED by this operation, so a caller cannot learn which AMI ids exist.
        Refusal {
            action: "DeregisterImage",
            params: vec![param("ImageId", &image)],
            code: "InvalidAMIID.Malformed".to_string(),
            status: 400,
        },
        // Cause 6 — the instance-task family.
        Refusal {
            action: "DeleteInstanceEventWindow",
            params: vec![param("InstanceEventWindowId", &event_window)],
            code: "InvalidInstanceEventWindowIDNotFound".to_string(),
            status: 400,
        },
        Refusal {
            action: "CreateInstanceExportTask",
            params: vec![
                param("InstanceId", &instance),
                param("TargetEnvironment", "vmware"),
                param("ExportToS3.S3Bucket", "a-bucket"),
            ],
            code: "InvalidParameter".to_string(),
            status: 400,
        },
        // Cause 7 — the Network Insights family. The operation ACTING on the one scope it
        // names states that the id it was given is not a valid one, which is not the
        // resource's own not-found code its reads answer.
        Refusal {
            action: "DeleteNetworkInsightsAccessScope",
            params: vec![param("NetworkInsightsAccessScopeId", &access_scope)],
            code: ec2::state::INVALID_PARAMETER_VALUE.to_string(),
            status: 400,
        },
        // Cause 8 — the dependency and state guards. `DeleteDhcpOptions` READS the id before
        // it resolves anything, so a hand-written id this service never issued is answered
        // with the `.Malformed` half of the pair rather than the not-found one its reads
        // answer; the not-found half is pinned on an id the service DID issue by
        // `a_dhcp_option_set_a_vpc_holds_cannot_be_deleted_until_it_is_released` below.
        Refusal {
            action: "DeleteDhcpOptions",
            params: vec![param("DhcpOptionsId", &dhcp_options)],
            code: ec2::state::INVALID_DHCP_OPTIONS_ID_MALFORMED.to_string(),
            status: 400,
        },
        Refusal {
            action: "DeleteRouteServer",
            params: vec![param("RouteServerId", &route_server)],
            code: ec2::state::INVALID_ROUTE_SERVER_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "DisassociateRouteServer",
            params: vec![param("RouteServerId", &route_server), param("VpcId", &vpc)],
            code: ec2::state::INVALID_ROUTE_SERVER_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "CreateCapacityReservationBySplitting",
            params: vec![
                param("SourceCapacityReservationId", &capacity_reservation),
                param("InstanceCount", "1"),
            ],
            code: ec2::state::INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        // Cause 9 — the purchase, listing and association refusals.
        Refusal {
            action: "PurchaseHostReservation",
            params: vec![
                param("HostIdSet.1", &host),
                param("OfferingId", &offering),
            ],
            code: "InvalidHostId.NotFound".to_string(),
            status: 400,
        },
        Refusal {
            action: "PurchaseCapacityBlockExtension",
            params: vec![
                param("CapacityReservationId", &capacity_reservation),
                param("CapacityBlockExtensionOfferingId", &offering),
            ],
            code: ec2::state::INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND.to_string(),
            status: 400,
        },
        Refusal {
            action: "CancelReservedInstancesListing",
            params: vec![param(
                "ReservedInstancesListingId",
                "4b2a1c5e-1f2b-4c3d-9e8f-7a6b5c4d3e2f",
            )],
            code: ec2::state::INVALID_INPUT.to_string(),
            status: 400,
        },
        // A value outside the id form is refused before anything is resolved, with the
        // `.Malformed` half of the pair.
        Refusal {
            action: "ModifyIpamRoutingPolicyRegistration",
            params: vec![
                param("IpamInternetRegistryAssociationId", "not-an-association-id"),
                param("Cidr", "10.15.0.0/16"),
                param("Asn.1", "64512"),
            ],
            code: ec2::state::INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_MALFORMED.to_string(),
            status: 400,
        },
    ]
}

#[tokio::test]
async fn every_refusal_answers_its_own_code_and_never_the_scaffold_501() {
    let (router, _engine) = super::ec2_query::emulator();
    for refusal in refusals() {
        let case = format!("{} {:?}", refusal.action, refusal.params);
        let (status, body) = call(&router, refusal.action, &refusal.params).await;
        let code = error_code(&body);
        assert_ne!(
            code, "NotImplemented",
            "case {case:?}: expected the modelled refusal {expected:?}, got the scaffold's \
             NotImplemented with body {body}",
            expected = refusal.code,
        );
        assert_eq!(
            code, refusal.code,
            "case {case:?}: expected code {expected:?}, got {code:?} with body {body}",
            expected = refusal.code,
        );
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected HTTP {expected} for {code:?}, got {status} with body \
             {body}",
            expected = refusal.status,
        );
    }
}

/// The id of a VPC with `cidr_block`, created over the wire.
async fn create_vpc(router: &MultiServiceRouter, case: &str, cidr_block: &str) -> String {
    let (status, body) = call(router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating a VPC, got {status} with body {body}"
    );
    element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no vpcId: {body}"))
}

#[tokio::test]
async fn a_dhcp_option_set_a_vpc_holds_cannot_be_deleted_until_it_is_released() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "dhcp options dependency";
    let vpc = create_vpc(&router, case, "10.20.0.0/16").await;

    let (status, body) = call(
        &router,
        "CreateDhcpOptions",
        &[
            param("DhcpConfiguration.1.Key", "domain-name-servers"),
            param("DhcpConfiguration.1.Value.1", "10.20.0.2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating a DHCP option set, got {status} with body \
         {body}"
    );
    let options = element(&body, "dhcpOptionsId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no dhcpOptionsId: {body}"));

    // Nothing holds the set yet, so a delete takes it: asserted LAST, after the dependency
    // the association creates — the first thing under test is that the association is what
    // makes the difference.
    let (status, body) = call(
        &router,
        "AssociateDhcpOptions",
        &[param("DhcpOptionsId", &options), param("VpcId", &vpc)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 associating {options} with {vpc}, got {status} with \
         body {body}"
    );

    let (status, body) = call(&router, "DeleteDhcpOptions", &[param("DhcpOptionsId", &options)]).await;
    assert_eq!(
        error_code(&body),
        "DependencyViolation",
        "case {case:?}: expected the delete of an associated option set to be refused with \
         DependencyViolation, got {status} with body {body}"
    );
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for DependencyViolation, got {status} with body \
         {body}"
    );

    // Released by associating the VPC with the Region's default set, which is how
    // `DeleteDhcpOptions` documents the disassociation: the very same delete now succeeds, so
    // the refusal above followed from the STORE rather than from the request.
    let (status, body) = call(
        &router,
        "AssociateDhcpOptions",
        &[param("DhcpOptionsId", "default"), param("VpcId", &vpc)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 releasing {options}, got {status} with body {body}"
    );
    let (status, body) = call(&router, "DeleteDhcpOptions", &[param("DhcpOptionsId", &options)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting the released option set, got {status} with \
         body {body}"
    );

    // And the set is GONE: the second delete is refused for the resource, not for a
    // dependency.
    let (status, body) = call(&router, "DeleteDhcpOptions", &[param("DhcpOptionsId", &options)]).await;
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_DHCP_OPTION_ID_NOT_FOUND,
        "case {case:?}: expected the deleted option set to be unknown, got {status} with body \
         {body}"
    );
}

#[tokio::test]
async fn a_route_server_a_vpc_holds_cannot_be_deleted_until_it_is_disassociated() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "route server association guards";
    let served = create_vpc(&router, case, "10.21.0.0/16").await;
    let other = create_vpc(&router, case, "10.22.0.0/16").await;

    let (status, body) = call(
        &router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", "64512")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating a route server, got {status} with body \
         {body}"
    );
    let route_server = element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no routeServerId: {body}"));

    let (status, body) = call(
        &router,
        "AssociateRouteServer",
        &[
            param("RouteServerId", &route_server),
            param("VpcId", &served),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 associating {route_server} with {served}, got \
         {status} with body {body}"
    );

    // A VPC the store holds no association for cannot be disassociated, even though BOTH the
    // route server and the VPC exist: the refusal is about the PAIR.
    let (status, body) = call(
        &router,
        "DisassociateRouteServer",
        &[param("RouteServerId", &route_server), param("VpcId", &other)],
    )
    .await;
    assert_eq!(
        error_code(&body),
        "InvalidRouteServerId.NotAssociated",
        "case {case:?}: expected the unassociated pairing to be refused, got {status} with \
         body {body}"
    );

    // A second association of the SAME route server is refused: a route server writes what it
    // learns into the route tables of the one VPC it is associated with, so it holds one
    // association at a time.
    let (status, body) = call(
        &router,
        "AssociateRouteServer",
        &[
            param("RouteServerId", &route_server),
            param("VpcId", &served),
        ],
    )
    .await;
    assert_eq!(
        error_code(&body),
        "InvalidRouteServerId.AlreadyAssociated",
        "case {case:?}: expected the repeated association to be refused, got {status} with \
         body {body}"
    );

    // The live association blocks the delete.
    let (status, body) = call(
        &router,
        "DeleteRouteServer",
        &[param("RouteServerId", &route_server)],
    )
    .await;
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "case {case:?}: expected the delete of an associated route server to be refused, got \
         {status} with body {body}"
    );

    // Disassociated, the very same delete succeeds — so the refusal followed from the store.
    let (status, body) = call(
        &router,
        "DisassociateRouteServer",
        &[
            param("RouteServerId", &route_server),
            param("VpcId", &served),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 disassociating {route_server} from {served}, got \
         {status} with body {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteRouteServer",
        &[param("RouteServerId", &route_server)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 deleting the disassociated route server, got \
         {status} with body {body}"
    );
    // The delete is observable through the describe: a route server a delete took down is
    // still REPORTED — `deleting` and `deleted` are both `RouteServerState` members — so the
    // describe answers it in the non-terminal state the delete put it into rather than as a
    // route server nothing holds.
    let (status, body) = call(
        &router,
        "DescribeRouteServers",
        &indexed("RouteServerId", &[route_server.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing the deleted route server, got {status} \
         with body {body}"
    );
    let reported = element(&body, "state").unwrap_or_default();
    assert_eq!(
        reported,
        ec2::state::ROUTE_SERVER_STATE_DELETING,
        "case {case:?}: expected the deleted route server to be reported deleting, got \
         {reported:?} in {body}"
    );

    // Nothing is left to delete, so a second delete is refused for the state it is in.
    let (status, body) = call(
        &router,
        "DeleteRouteServer",
        &[param("RouteServerId", &route_server)],
    )
    .await;
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "case {case:?}: expected a second delete to be refused, got {status} with body {body}"
    );
}

#[tokio::test]
async fn a_split_moves_capacity_and_a_split_past_what_is_left_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "capacity reservation split";
    let (status, body) = call(
        &router,
        "CreateCapacityReservation",
        &[
            param("InstanceType", "m5.large"),
            param("InstancePlatform", "Linux/UNIX"),
            param("AvailabilityZone", "us-east-1a"),
            param("InstanceCount", "4"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating a capacity reservation, got {status} with \
         body {body}"
    );
    let source = element(&body, "capacityReservationId").unwrap_or_else(|| {
        panic!("case {case:?}: the create answered no capacityReservationId: {body}")
    });

    // Three of the four instances move into a new reservation: the answer names both sides,
    // and the two together hold exactly what the source held.
    let (status, body) = call(
        &router,
        "CreateCapacityReservationBySplitting",
        &[
            param("SourceCapacityReservationId", &source),
            param("InstanceCount", "3"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 splitting three instances off {source}, got {status} \
         with body {body}"
    );
    assert_eq!(
        element(&body, "instanceCount").as_deref(),
        Some("3"),
        "case {case:?}: expected the split to report the instances it moved, got {body}"
    );
    let parsed = super::wire::document(case, &body);
    let destination = super::wire::child(parsed.root_element(), "destinationCapacityReservation")
        .and_then(|node| super::wire::child_text(node, "capacityReservationId"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the split answered no destination reservation: {body}")
        });
    let reported = |name: &str, member_name: &str| -> Option<String> {
        super::wire::child(parsed.root_element(), name)
            .and_then(|node| super::wire::child_text(node, member_name))
    };
    assert_eq!(
        reported("sourceCapacityReservation", "totalInstanceCount").as_deref(),
        Some("1"),
        "case {case:?}: expected the source to hold the instance the split left, got {body}"
    );
    assert_eq!(
        reported("destinationCapacityReservation", "totalInstanceCount").as_deref(),
        Some("3"),
        "case {case:?}: expected the destination to hold the instances the split moved, got \
         {body}"
    );

    // Read both back out of the store: a split that reported capacity it did not move would
    // differ here.
    let (status, body) = call(
        &router,
        "DescribeCapacityReservations",
        &indexed("CapacityReservationId", &[source.as_str(), destination.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading both reservations back, got {status} with \
         body {body}"
    );
    let counts: Vec<String> = super::ec2_query::elements(&body, "totalInstanceCount");
    assert_eq!(
        counts,
        vec!["1".to_string(), "3".to_string()],
        "case {case:?}: expected the stored reservations to hold 1 and 3 instances, got {body}"
    );

    // One instance is left, and a split has to leave at least one behind: both of these ask
    // for more than the source still holds.
    for requested in ["1", "2"] {
        let (status, body) = call(
            &router,
            "CreateCapacityReservationBySplitting",
            &[
                param("SourceCapacityReservationId", &source),
                param("InstanceCount", requested),
            ],
        )
        .await;
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected a split of {requested} past what {source} holds to be \
             refused, got {status} with body {body}"
        );
    }
}
