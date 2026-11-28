// SPDX-License-Identifier: CC0-1.0

//! What `DescribeRouteTables` reports as a ROUTE'S STATE, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeRouteTables`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: a route's state is a fact of its TARGET'S AVAILABILITY — "the
//! `blackhole` state indicates that the route's target isn't available (for example, the
//! specified gateway isn't attached to the VPC, or the specified NAT instance has been
//! terminated)" (`Route$State`) — so a read reports `blackhole` for every route whose target
//! is not available at the instant it answers and `active` for every route whose target is,
//! whenever and however that target stopped or started being available. The route itself is
//! untouched by its target going away: "deleting a NAT gateway does not delete any NAT gateway
//! routes in your route tables" (`DeleteNatGateway`), so the route keeps its destination and
//! the member that names its target, and reads `active` again once the target is available
//! again.
//!
//! The rows differ along what the rule generalises over: WHICH KIND of resource the target is
//! and which member of the reported route carries it, and WHICH call takes that target out of
//! service — a detach, a delete, a termination — so no row is the specification. The local
//! route every table comes with is the control: its target is the VPC itself, which no row
//! takes away.
//!
//! The `route.state` filter selects on the same value the response carries ("the state of a
//! route in the table" — `DescribeRouteTablesRequest$Filters`), and it selects whole TABLES, so
//! a client finds the tables holding a broken route by asking for `blackhole` and a table whose
//! routes are all live is not among them.

use std::collections::BTreeMap;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{described_instance_state, element, filters};
use super::rbin_retention_rules::poll_until;
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch names. Which image an instance ran from has no bearing on whether a
/// route through it carries traffic.
const AMI: &str = "ami-0123456789abcdef0";

/// The block of the VPC every row's table routes for, and the subnet-sized block the resources
/// the rows point at live in. The VPC's own block is the destination of the table's local
/// route, which is the control row below.
const VPC_CIDR: &str = "10.70.0.0/16";
const SUBNET_CIDR: &str = "10.70.1.0/24";

/// The block of the VPC the peering connection of one row peers with, which is also that row's
/// route destination — a peering route sends the peer's own space across it.
const PEER_VPC_CIDR: &str = "10.71.0.0/16";

/// The two states a route is reported in, as the modelled `RouteState` enum spells them, taken
/// from the service's own constants rather than copied into this suite.
const ACTIVE: &str = ec2::state::ROUTE_STATE_ACTIVE;
const BLACKHOLE: &str = ec2::state::ROUTE_STATE_BLACKHOLE;

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Every route the table `route_table_id` is reported with, keyed by the destination it matches
/// on, each as the scalar members its `<item>` carries.
///
/// Read through the XML tree out of the table's OWN `<routeSet>`, so an `<item>` of the table's
/// association or tag sets can never be read as a route and a `<state>` is the route's own.
async fn reported_routes(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let body = ok(
        case,
        router,
        "DescribeRouteTables",
        &[param("RouteTableId.1", route_table_id)],
    )
    .await;
    let parsed = document(case, &body);
    let table = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "routeTableId").as_deref() == Some(route_table_id)
                && child(*node, "routeSet").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: {route_table_id} is not in the report {body}"));
    set_items(table, "routeSet")
        .into_iter()
        .filter_map(|route| {
            let members: BTreeMap<String, String> = route
                .children()
                .filter(roxmltree::Node::is_element)
                .map(|field| {
                    (
                        field.tag_name().name().to_string(),
                        field.text().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let destination = members
                .get("destinationCidrBlock")
                .or_else(|| members.get("destinationIpv6CidrBlock"))?
                .clone();
            Some((destination, members))
        })
        .collect()
}

/// The state the table reports for the route matching `destination`, or `None` when it reports
/// no such route at all.
async fn reported_state(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    destination: &str,
) -> Option<String> {
    reported_routes(case, router, route_table_id)
        .await
        .get(destination)
        .and_then(|route| route.get("state").cloned())
}

/// One `CreateRoute` of `destination` pointed at the `target_member` target `target`, which has
/// to be stored.
async fn create_route(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    destination: &str,
    target_member: &str,
    target: &str,
) {
    // An IPv6 destination is carried by its own member: the egress-only internet gateway of the
    // last row is "[IPv6 traffic only]" (`CreateRouteRequest$EgressOnlyInternetGatewayId`).
    let destination_member = if destination.contains(':') {
        "DestinationIpv6CidrBlock"
    } else {
        "DestinationCidrBlock"
    };
    let body = ok(
        case,
        router,
        "CreateRoute",
        &[
            param("RouteTableId", route_table_id),
            param(destination_member, destination),
            param(target_member, target),
        ],
    )
    .await;
    assert!(
        body.contains("<return>true</return>"),
        "case {case:?}: expected the create of the route to {destination} to report true, got \
         {body}"
    );
}

/// What takes one row's target out of service, or puts it back — each one real call of the EC2
/// surface, so nothing here writes a route's state by hand.
#[derive(Debug, Clone)]
enum Event {
    /// `AttachInternetGateway` / `DetachInternetGateway`: the attachment is what makes the
    /// gateway able to carry this VPC's traffic.
    SetInternetGatewayAttachment {
        internet_gateway_id: String,
        vpc_id: String,
        attached: bool,
    },
    /// `DeleteNatGateway`.
    DeleteNatGateway { nat_gateway_id: String },
    /// `TerminateInstances` — "the specified NAT instance has been terminated"
    /// (`Route$State`).
    TerminateInstance { instance_id: String },
    /// `DetachNetworkInterface`: an interface carries traffic only while something holds it.
    DetachNetworkInterface { attachment_id: String },
    /// `AttachNetworkInterface`, giving the interface back to the instance that held it.
    AttachNetworkInterface {
        network_interface_id: String,
        instance_id: String,
        device_index: u32,
    },
    /// `DeleteVpcPeeringConnection`.
    DeleteVpcPeeringConnection { vpc_peering_connection_id: String },
    /// `DeleteEgressOnlyInternetGateway`.
    DeleteEgressOnlyInternetGateway {
        egress_only_internet_gateway_id: String,
    },
}

/// Make `event` happen, over the wire.
async fn perform(case: &str, router: &MultiServiceRouter, event: &Event) {
    let (action, params): (&str, Vec<Param>) = match event {
        Event::SetInternetGatewayAttachment {
            internet_gateway_id,
            vpc_id,
            attached,
        } => (
            if *attached {
                "AttachInternetGateway"
            } else {
                "DetachInternetGateway"
            },
            vec![
                param("InternetGatewayId", internet_gateway_id),
                param("VpcId", vpc_id),
            ],
        ),
        Event::DeleteNatGateway { nat_gateway_id } => (
            "DeleteNatGateway",
            vec![param("NatGatewayId", nat_gateway_id)],
        ),
        Event::TerminateInstance { instance_id } => (
            "TerminateInstances",
            vec![param("InstanceId.1", instance_id)],
        ),
        Event::DetachNetworkInterface { attachment_id } => (
            "DetachNetworkInterface",
            vec![param("AttachmentId", attachment_id)],
        ),
        Event::AttachNetworkInterface {
            network_interface_id,
            instance_id,
            device_index,
        } => (
            "AttachNetworkInterface",
            vec![
                param("NetworkInterfaceId", network_interface_id),
                param("InstanceId", instance_id),
                param("DeviceIndex", &device_index.to_string()),
            ],
        ),
        Event::DeleteVpcPeeringConnection {
            vpc_peering_connection_id,
        } => (
            "DeleteVpcPeeringConnection",
            vec![param("VpcPeeringConnectionId", vpc_peering_connection_id)],
        ),
        Event::DeleteEgressOnlyInternetGateway {
            egress_only_internet_gateway_id,
        } => (
            "DeleteEgressOnlyInternetGateway",
            vec![param(
                "EgressOnlyInternetGatewayId",
                egress_only_internet_gateway_id,
            )],
        ),
    };
    ok(case, router, action, &params).await;
}

/// One row: a route of the table, the kind of target it points at, and the calls that take that
/// target out of service and (where there is a way back) put it back.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    /// The destination the row's route matches on, which is what addresses it in the report.
    destination: &'static str,
    /// The member `CreateRoute` names this kind of target under.
    requested_target_member: &'static str,
    /// The member of the reported route that names it, which must still be there once the
    /// target has gone.
    reported_target_member: &'static str,
    /// The id both members carry.
    target: String,
    /// The call that makes the target unavailable.
    unavailable: Event,
    /// The call that makes it available again, for the kinds that have one.
    available_again: Option<Event>,
}

/// A VPC of `cidr_block`, a subnet of it where one is asked for, and a route table of it.
async fn network(
    case: &str,
    router: &MultiServiceRouter,
    cidr_block: &str,
    subnet_cidr: Option<&str>,
) -> (String, Option<String>, String) {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    let vpc_id = member(case, &body, "vpcId");
    let mut subnet_id = None;
    if let Some(subnet_cidr) = subnet_cidr {
        let body = ok(
            case,
            router,
            "CreateSubnet",
            &[param("VpcId", &vpc_id), param("CidrBlock", subnet_cidr)],
        )
        .await;
        subnet_id = Some(member(case, &body, "subnetId"));
    }
    let body = ok(case, router, "CreateRouteTable", &[param("VpcId", &vpc_id)]).await;
    let route_table_id = member(case, &body, "routeTableId");
    (vpc_id, subnet_id, route_table_id)
}

/// One instance launched into `subnet_id`, reported `running` — the state a NAT instance
/// carries traffic in. Waits for the CONDITION the clock owes rather than for a span.
async fn running_instance(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("SubnetId", subnet_id),
        ],
    )
    .await;
    let instance_id = member(case, &body, "instanceId");
    await_running(case, router, &instance_id).await;
    instance_id
}

/// Look again until `instance_id` is reported `running`.
async fn await_running(case: &str, router: &MultiServiceRouter, instance_id: &str) {
    poll_until(case, "the launched instance to be running", || async {
        described_instance_state(router, REGION, instance_id).await == "running"
    })
    .await;
}

/// A private NAT gateway of `subnet_id`, as its id.
///
/// The one form of NAT gateway whose create states no Elastic IP address: "you cannot specify
/// an Elastic IP address with a private NAT gateway"
/// (`CreateNatGatewayRequest$AllocationId`).
async fn private_nat_gateway(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateNatGateway",
        &[
            param("SubnetId", subnet_id),
            param("ConnectivityType", "private"),
        ],
    )
    .await;
    member(case, &body, "natGatewayId")
}

/// The rule: every route a read reports is in the state its target's availability puts it in,
/// for every kind of target and whichever call took that target out of service — and a target
/// that becomes available again makes its route `active` again, while the table's local route
/// and the other rows' routes never move.
#[tokio::test]
async fn every_reported_route_reads_the_availability_of_its_target() {
    let (router, _engine) = super::ec2_query::emulator();
    let (vpc_id, subnet_id, route_table_id) = network(
        "the network the rows route for",
        &router,
        VPC_CIDR,
        Some(SUBNET_CIDR),
    )
    .await;
    let subnet_id = subnet_id.expect("the network was asked for a subnet");

    // An internet gateway attached to that VPC.
    let body = ok("the internet gateway", &router, "CreateInternetGateway", &[]).await;
    let internet_gateway_id = member("the internet gateway", &body, "internetGatewayId");
    let attach_gateway = Event::SetInternetGatewayAttachment {
        internet_gateway_id: internet_gateway_id.clone(),
        vpc_id: vpc_id.clone(),
        attached: true,
    };
    perform("the internet gateway", &router, &attach_gateway).await;

    let nat_gateway_id = private_nat_gateway("the NAT gateway", &router, &subnet_id).await;

    // A NAT instance, and a SECOND instance holding the elastic network interface of the row
    // below — so terminating the first takes nothing else out of service.
    let nat_instance_id = running_instance("the NAT instance", &router, &subnet_id).await;
    let interface_holder =
        running_instance("the instance holding the interface", &router, &subnet_id).await;
    let body = ok(
        "the elastic network interface",
        &router,
        "CreateNetworkInterface",
        &[param("SubnetId", &subnet_id)],
    )
    .await;
    let network_interface_id = member("the elastic network interface", &body, "networkInterfaceId");
    // Device index 1: the index-0 attachment a launch makes is the instance's primary
    // interface, which cannot be detached.
    let device_index = 1;
    let body = ok(
        "the elastic network interface",
        &router,
        "AttachNetworkInterface",
        &[
            param("NetworkInterfaceId", &network_interface_id),
            param("InstanceId", &interface_holder),
            param("DeviceIndex", &device_index.to_string()),
        ],
    )
    .await;
    let attachment_id = member("the elastic network interface", &body, "attachmentId");

    // A peering connection with a second VPC of the account.
    let (peer_vpc_id, _, _) = network("the peer network", &router, PEER_VPC_CIDR, None).await;
    let body = ok(
        "the peering connection",
        &router,
        "CreateVpcPeeringConnection",
        &[param("VpcId", &vpc_id), param("PeerVpcId", &peer_vpc_id)],
    )
    .await;
    let vpc_peering_connection_id =
        member("the peering connection", &body, "vpcPeeringConnectionId");

    // An egress-only internet gateway of the VPC, the one target of this table reached over
    // IPv6.
    let body = ok(
        "the egress-only internet gateway",
        &router,
        "CreateEgressOnlyInternetGateway",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    let egress_only_internet_gateway_id = member(
        "the egress-only internet gateway",
        &body,
        "egressOnlyInternetGatewayId",
    );

    // Source: `Route`'s own target members (`Route$GatewayId`, `$NatGatewayId`, `$InstanceId`,
    // `$NetworkInterfaceId`, `$VpcPeeringConnectionId`, `$EgressOnlyInternetGatewayId`) against
    // the call of the EC2 surface that takes each kind of target out of service.
    let rows: Vec<Row> = vec![
        Row {
            case: "an internet gateway detached from the VPC the table routes for",
            destination: "0.0.0.0/0",
            requested_target_member: "GatewayId",
            reported_target_member: "gatewayId",
            target: internet_gateway_id.clone(),
            unavailable: Event::SetInternetGatewayAttachment {
                internet_gateway_id: internet_gateway_id.clone(),
                vpc_id: vpc_id.clone(),
                attached: false,
            },
            available_again: Some(attach_gateway.clone()),
        },
        Row {
            case: "a NAT gateway the account has deleted",
            destination: "198.51.100.0/24",
            requested_target_member: "NatGatewayId",
            reported_target_member: "natGatewayId",
            target: nat_gateway_id.clone(),
            unavailable: Event::DeleteNatGateway {
                nat_gateway_id: nat_gateway_id.clone(),
            },
            available_again: None,
        },
        Row {
            case: "a NAT instance that has been terminated",
            destination: "203.0.113.0/24",
            requested_target_member: "InstanceId",
            reported_target_member: "instanceId",
            target: nat_instance_id.clone(),
            unavailable: Event::TerminateInstance {
                instance_id: nat_instance_id.clone(),
            },
            available_again: None,
        },
        Row {
            case: "an elastic network interface detached from its instance",
            destination: "192.0.2.0/24",
            requested_target_member: "NetworkInterfaceId",
            reported_target_member: "networkInterfaceId",
            target: network_interface_id.clone(),
            unavailable: Event::DetachNetworkInterface {
                attachment_id: attachment_id.clone(),
            },
            available_again: Some(Event::AttachNetworkInterface {
                network_interface_id: network_interface_id.clone(),
                instance_id: interface_holder.clone(),
                device_index,
            }),
        },
        Row {
            case: "a VPC peering connection the account has deleted",
            destination: PEER_VPC_CIDR,
            requested_target_member: "VpcPeeringConnectionId",
            reported_target_member: "vpcPeeringConnectionId",
            target: vpc_peering_connection_id.clone(),
            unavailable: Event::DeleteVpcPeeringConnection {
                vpc_peering_connection_id: vpc_peering_connection_id.clone(),
            },
            available_again: None,
        },
        Row {
            case: "an egress-only internet gateway the account has deleted",
            destination: "::/0",
            requested_target_member: "EgressOnlyInternetGatewayId",
            reported_target_member: "egressOnlyInternetGatewayId",
            target: egress_only_internet_gateway_id.clone(),
            unavailable: Event::DeleteEgressOnlyInternetGateway {
                egress_only_internet_gateway_id: egress_only_internet_gateway_id.clone(),
            },
            available_again: None,
        },
    ];

    // Every row's route, pointed at a target that IS available: the table reports them all
    // `active`, beside the local route it came with.
    for row in &rows {
        create_route(
            row.case,
            &router,
            &route_table_id,
            row.destination,
            row.requested_target_member,
            &row.target,
        )
        .await;
    }
    let mut expected: Vec<(&str, &str)> =
        rows.iter().map(|row| (row.destination, ACTIVE)).collect();
    assert_states("every target available", &router, &route_table_id, &expected).await;
    assert_local_route_is_active("every target available", &router, &route_table_id).await;

    // One row at a time, the target goes out of service: that row's route reads `blackhole`,
    // keeps the member naming the target it still points at, and no other row's route moves.
    for (index, row) in rows.iter().enumerate() {
        perform(row.case, &router, &row.unavailable).await;
        expected[index].1 = BLACKHOLE;
        await_states(row.case, &router, &route_table_id, &expected).await;
        assert_target_is_still_named(row, &router, &route_table_id).await;
        assert_local_route_is_active(row.case, &router, &route_table_id).await;
    }

    // And a target that becomes available again makes its route carry traffic again.
    for (index, row) in rows.iter().enumerate() {
        let Some(event) = row.available_again.as_ref() else {
            continue;
        };
        perform(row.case, &router, event).await;
        expected[index].1 = ACTIVE;
        await_states(row.case, &router, &route_table_id, &expected).await;
        assert_target_is_still_named(row, &router, &route_table_id).await;
    }
}

/// Look again until the table reports `expected`, then assert it does.
///
/// A target goes out of service through the call that takes it away, and one of those — an
/// interface's detach — completes at an instant the clock owes rather than inside the call, so
/// the CONDITION is waited for rather than a span.
async fn await_states(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    expected: &[(&str, &str)],
) {
    poll_until(case, "every route to read the state of its target", || async {
        let reported = reported_routes(case, router, route_table_id).await;
        expected.iter().all(|(destination, state)| {
            reported
                .get(*destination)
                .and_then(|route| route.get("state"))
                .map(String::as_str)
                == Some(*state)
        })
    })
    .await;
    assert_states(case, router, route_table_id, expected).await;
}

/// Assert, destination by destination, that the table reports `expected` right now.
async fn assert_states(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    expected: &[(&str, &str)],
) {
    let reported = reported_routes(case, router, route_table_id).await;
    for (destination, state) in expected {
        let read = reported
            .get(*destination)
            .and_then(|route| route.get("state").cloned());
        assert_eq!(
            read.as_deref(),
            Some(*state),
            "case {case:?}: the route to {destination} must read {state:?}, got {read:?} in \
             {reported:?}"
        );
    }
}

/// A route whose target went away is still the route it was: it keeps the member naming that
/// target, so a client can see WHAT is unavailable rather than only that something is.
async fn assert_target_is_still_named(
    row: &Row,
    router: &MultiServiceRouter,
    route_table_id: &str,
) {
    let reported = reported_routes(row.case, router, route_table_id).await;
    let route = reported.get(row.destination).unwrap_or_else(|| {
        panic!(
            "case {:?}: the route to {} must still be reported, got {reported:?}",
            row.case, row.destination
        )
    });
    let named = route.get(row.reported_target_member).cloned();
    assert_eq!(
        named.as_deref(),
        Some(row.target.as_str()),
        "case {:?}: the route must still carry its target in <{}>, got {named:?} in {route:?}",
        row.case,
        row.reported_target_member
    );
}

/// The local route every table comes with is `active` whatever happened to the rows' targets:
/// its target is the VPC itself ("every route table contains a local route for communication
/// within the VPC"), which nothing here takes away.
async fn assert_local_route_is_active(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
) {
    let state = reported_state(case, router, route_table_id, VPC_CIDR).await;
    assert_eq!(
        state.as_deref(),
        Some(ACTIVE),
        "case {case:?}: the table's local route must stay {ACTIVE:?}, got {state:?}"
    );
}

/// The `route.state` filter selects on the state the response carries, and it selects whole
/// TABLES: the table holding the broken route is the one a request for `blackhole` answers, and
/// a table whose every route is live is not among them while still being among the tables a
/// request for `active` answers.
#[tokio::test]
async fn the_route_state_filter_selects_the_tables_holding_a_route_in_that_state() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the table that holds the broken route";
    let (vpc_id, subnet_id, broken_table) =
        network(case, &router, VPC_CIDR, Some(SUBNET_CIDR)).await;
    let subnet_id = subnet_id.expect("the network was asked for a subnet");
    let body = ok(
        case,
        &router,
        "CreateRouteTable",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    let live_table = member(case, &body, "routeTableId");

    let nat_gateway_id = private_nat_gateway(case, &router, &subnet_id).await;
    create_route(
        case,
        &router,
        &broken_table,
        "198.51.100.0/24",
        "NatGatewayId",
        &nat_gateway_id,
    )
    .await;

    // While the gateway is there, no table holds a blackhole route and both hold a live one.
    let selected = filtered_tables(case, &router, BLACKHOLE).await;
    assert!(
        selected.is_empty(),
        "case {case:?}: no table holds a route whose target is unavailable yet, got {selected:?}"
    );
    let live = filtered_tables(case, &router, ACTIVE).await;
    for table in [&broken_table, &live_table] {
        assert!(
            live.contains(table),
            "case {case:?}: expected {table} among the tables holding an {ACTIVE} route, got \
             {live:?}"
        );
    }

    // The gateway goes, and exactly the table holding the route to it is the one a request for
    // `blackhole` answers — the other table's local route is untouched.
    perform(
        case,
        &router,
        &Event::DeleteNatGateway {
            nat_gateway_id: nat_gateway_id.clone(),
        },
    )
    .await;
    await_filtered_tables(case, &router, BLACKHOLE, &[broken_table.as_str()]).await;
    let still_live = filtered_tables(case, &router, ACTIVE).await;
    for table in [&broken_table, &live_table] {
        assert!(
            still_live.contains(table),
            "case {case:?}: expected {table} to still hold an {ACTIVE} route (its local route), \
             got {still_live:?}"
        );
    }
}

/// Look again until `route.state` = `state` selects exactly `expected`, then assert it does.
async fn await_filtered_tables(
    case: &str,
    router: &MultiServiceRouter,
    state: &str,
    expected: &[&str],
) {
    poll_until(case, "the filter to select the tables it must", || async {
        filtered_tables(case, router, state).await == expected
    })
    .await;
    let selected = filtered_tables(case, router, state).await;
    assert_eq!(
        selected, expected,
        "case {case:?}: route.state {state:?} must select {expected:?}, got {selected:?}"
    );
}

/// The ids of the route tables `DescribeRouteTables` selects with `route.state` = `state`,
/// sorted so two reads compare.
async fn filtered_tables(case: &str, router: &MultiServiceRouter, state: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeRouteTables",
        &filters(&[("route.state", &[state])]),
    )
    .await;
    let parsed = document(case, &body);
    let mut ids: Vec<String> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "routeSet").is_some())
        .filter_map(|table| child_text(table, "routeTableId"))
        .collect();
    ids.sort();
    ids
}
