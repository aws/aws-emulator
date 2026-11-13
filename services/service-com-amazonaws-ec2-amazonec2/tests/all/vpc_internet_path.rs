// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a VPC's PATH TO THE INTERNET — `CreateInternetGateway`,
//! `AttachInternetGateway`, `CreateRouteTable`, `CreateRoute` and `AssociateRouteTable`,
//! read back through `DescribeInternetGateways` and `DescribeRouteTables` — driven over the
//! wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: **an account builds that path out of resources each of which is
//! stored where its own describe reports it** — an internet gateway is created attached to
//! nothing and then attaches to exactly ONE VPC, a route table is created for one VPC
//! carrying the local route every table comes with for each of that VPC's IPv4 blocks, a
//! route points ONE destination at ONE target of that table's own VPC, and a subnet is
//! associated with at most one route table of its own VPC. Nothing on that path is reported
//! that was not stored, and a second claim on either side of an attachment or an
//! association is refused rather than added beside the one that holds.
//!
//! The same rule covers a VPC's path to an Amazon Web Services service that is reached
//! WITHOUT the internet: a gateway `CreateVpcEndpoint` stores the endpoint and the one route
//! it is reached through in every route table it names, so the table a subnet is explicitly
//! associated with reports that route beside the local one — which is what a client reads
//! when it asks `DescribeRouteTables` which table its subnet uses.
//!
//! Every table row is a distinct input along what that rule generalises over — the
//! destination block's value, size and address family, how many host bits the request set,
//! the tag count, how many subnets an association covers, which side of a pair a
//! request collides with, the endpoint service a gateway endpoint is created for, how many
//! tables it names, how many endpoints one table is named by, and the order the estate is
//! built in — so any one recorded call is one row among many rather than the specification.

use super::ec2_query::{element, elements, error_code, error_message, filters};
use super::wire::{child, child_text, document, param, set_items, Param};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "subnetId")
}

/// An internet gateway carrying `tags`, as its id.
async fn create_internet_gateway(
    case: &str,
    router: &MultiServiceRouter,
    tags: &[(&str, &str)],
) -> String {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param("TagSpecification.1.ResourceType", "internet-gateway"));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    let body = ok(case, router, "CreateInternetGateway", &params).await;
    member(case, &body, "internetGatewayId")
}

/// A route table of `vpc_id`, as its id.
async fn create_route_table(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateRouteTable",
        &[param("VpcId", vpc_id)],
    )
    .await;
    member(case, &body, "routeTableId")
}

/// The `DescribeRouteTables` body for the one table `route_table_id` names.
async fn describe_route_table(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
) -> String {
    ok(
        case,
        router,
        "DescribeRouteTables",
        &[param("RouteTableId.1", route_table_id)],
    )
    .await
}

/// The `DescribeInternetGateways` body for the one gateway `internet_gateway_id` names.
async fn describe_internet_gateway(
    case: &str,
    router: &MultiServiceRouter,
    internet_gateway_id: &str,
) -> String {
    ok(
        case,
        router,
        "DescribeInternetGateways",
        &[param("InternetGatewayId.1", internet_gateway_id)],
    )
    .await
}

/// One row of a table whose middle column is a list of `(name, value)` pairs — the tags a
/// create asks for, or the parameters a refusal row sends beside the resource it names.
/// Named so the row type is a type rather than a nest of tuples.
type PairsRow<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

/// Whether `value` is an EC2 long resource id of `prefix`.
fn is_long_id(prefix: &str, value: &str) -> bool {
    super::wire::is_long_id(prefix, value)
}

// ---------------------------------------------------------------------------
// CreateInternetGateway / AttachInternetGateway
// ---------------------------------------------------------------------------

/// The rule: a created internet gateway is stored attached to NOTHING ("after creating the
/// internet gateway, you attach it to a VPC using AttachInternetGateway" —
/// `CreateInternetGateway`), carries the tags its specification asked for, and reads back
/// through `DescribeInternetGateways`; attaching it records the one attachment, which that
/// describe then reports and its `attachment.*` filters select on.
///
/// The rows differ in how many tags the create carries — none, one, several — which is what
/// the tag payload generalises over.
#[tokio::test]
async fn an_internet_gateway_is_created_unattached_and_attaches_to_one_vpc() {
    let router = emulator();
    // (case, the tags the create asks for, the VPC's block)
    let cases: [PairsRow<'_>; 3] = [
        ("no tags", &[], "10.20.0.0/16"),
        ("one tag", &[("Name", "edge")], "10.21.0.0/16"),
        (
            "several tags",
            &[("Name", "edge"), ("team", "platform"), ("env", "test")],
            "10.22.0.0/20",
        ),
    ];

    for (case, tags, vpc_cidr) in cases {
        let internet_gateway_id = create_internet_gateway(case, &router, tags).await;
        assert!(
            is_long_id("igw", &internet_gateway_id),
            "case {case:?}: expected an igw-… id, got {internet_gateway_id:?}"
        );

        // Nothing is attached yet, so the describe reports the gateway with no attachment.
        let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
        assert!(
            elements(&body, "vpcId").is_empty(),
            "case {case:?}: expected no attachment before the attach, got {body}"
        );
        for (key, value) in tags {
            let keys = elements(&body, "key");
            let values = elements(&body, "value");
            assert!(
                keys.iter().any(|held| held == key) && values.iter().any(|held| held == value),
                "case {case:?}: expected the declared tag {key:?}={value:?}, got {body}"
            );
        }

        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        ok(
            case,
            &router,
            "AttachInternetGateway",
            &[
                param("InternetGatewayId", &internet_gateway_id),
                param("VpcId", &vpc_id),
            ],
        )
        .await;

        // The attachment is what the describe now reports: the VPC it was made to, in the
        // one state an internet gateway's attachment is reported in ("for an internet
        // gateway, the state is `available` when attached to a VPC" —
        // `InternetGatewayAttachment$State`).
        let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
        assert_eq!(
            elements(&body, "vpcId"),
            vec![vpc_id.clone()],
            "case {case:?}: expected the attachment to name {vpc_id:?}, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec!["available".to_string()],
            "case {case:?}: expected the attachment reported available, got {body}"
        );

        // And the documented filters select on it.
        let filtered = ok(
            case,
            &router,
            "DescribeInternetGateways",
            &filters(&[
                ("attachment.vpc-id", &[&vpc_id]),
                ("attachment.state", &["available"]),
            ]),
        )
        .await;
        assert_eq!(
            elements(&filtered, "internetGatewayId"),
            vec![internet_gateway_id.clone()],
            "case {case:?}: expected the attachment filters to select the gateway, got \
             {filtered}"
        );
    }
}

/// The rule's refusal half: "you can attach only one internet gateway to a VPC at a time"
/// (Amazon VPC User Guide, "Connect to the internet using an internet gateway"), and a
/// gateway attaches one VPC — so a second claim on EITHER side is refused with EC2's
/// `Resource.AlreadyAssociated` and nothing is added beside the attachment that holds,
/// while an id naming no resource is that resource's own not-found refusal.
///
/// The rows differ in which side of the pair the request collides with and in which id
/// names nothing.
#[tokio::test]
async fn a_second_claim_on_either_side_of_an_internet_gateway_attachment_is_refused() {
    let router = emulator();
    let case = "the attachment that holds";
    let vpc_id = create_vpc(case, &router, "10.23.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.24.0.0/16").await;
    let internet_gateway_id = create_internet_gateway(case, &router, &[]).await;
    let other_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &internet_gateway_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;

    // (case, the gateway, the VPC, the error code, what the message has to name)
    let cases = [
        (
            "the same gateway again",
            internet_gateway_id.as_str(),
            vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            internet_gateway_id.as_str(),
        ),
        (
            "the gateway attached elsewhere",
            internet_gateway_id.as_str(),
            other_vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            vpc_id.as_str(),
        ),
        (
            "a second gateway for the VPC",
            other_gateway_id.as_str(),
            vpc_id.as_str(),
            "Resource.AlreadyAssociated",
            internet_gateway_id.as_str(),
        ),
        (
            "a gateway id naming nothing",
            "igw-00000000000000000",
            vpc_id.as_str(),
            "InvalidInternetGatewayID.NotFound",
            "igw-00000000000000000",
        ),
        (
            "a VPC id naming nothing",
            other_gateway_id.as_str(),
            "vpc-00000000000000000",
            "InvalidVpcID.NotFound",
            "vpc-00000000000000000",
        ),
    ];

    for (case, gateway, vpc, expected_code, named) in cases {
        let (status, body) = call(
            &router,
            "AttachInternetGateway",
            &[param("InternetGatewayId", gateway), param("VpcId", vpc)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            error_message(&body).contains(named),
            "case {case:?}: expected the message to name {named:?}, got {}",
            error_message(&body)
        );
    }

    // Nothing was added beside the attachment that holds.
    let body = describe_internet_gateway(case, &router, &internet_gateway_id).await;
    assert_eq!(
        elements(&body, "vpcId"),
        vec![vpc_id.clone()],
        "expected the one attachment to survive every refusal, got {body}"
    );
}

// ---------------------------------------------------------------------------
// CreateRouteTable
// ---------------------------------------------------------------------------

/// The rule: a route table is created FOR one VPC and comes with the local route every
/// table has ("every route table contains a local route for communication within the VPC",
/// whose target is `local` — Amazon VPC User Guide, "Route tables"), and that table reads
/// back through `DescribeRouteTables` with the routes it holds. The VPC's own MAIN table —
/// the one "your VPC automatically comes with" — is reported beside it, with the implicit
/// association `main` marks.
///
/// The rows differ in the VPC's block value and size, which is what the local route's
/// destination is read from.
#[tokio::test]
async fn a_route_table_is_created_for_its_vpc_with_the_local_route_it_comes_with() {
    let router = emulator();
    // (case, the VPC's block)
    let cases = [
        ("a /16", "10.30.0.0/16"),
        ("a /20", "10.31.0.0/20"),
        ("a /24 outside the private 10 block", "192.168.7.0/24"),
    ];

    for (case, vpc_cidr) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        assert!(
            is_long_id("rtb", &route_table_id),
            "case {case:?}: expected an rtb-… id, got {route_table_id:?}"
        );

        let body = describe_route_table(case, &router, &route_table_id).await;
        assert_eq!(
            element(&body, "vpcId").as_deref(),
            Some(vpc_id.as_str()),
            "case {case:?}: expected the table to name its VPC, got {body}"
        );
        assert_eq!(
            elements(&body, "destinationCidrBlock"),
            vec![vpc_cidr.to_string()],
            "case {case:?}: expected the local route for {vpc_cidr}, got {body}"
        );
        assert_eq!(
            elements(&body, "gatewayId"),
            vec!["local".to_string()],
            "case {case:?}: expected the local route's target to be local, got {body}"
        );
        assert_eq!(
            elements(&body, "origin"),
            vec!["CreateRouteTable".to_string()],
            "case {case:?}: expected the local route's origin, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec!["active".to_string()],
            "case {case:?}: expected the local route active, got {body}"
        );
        // A table a caller created is not the main one, so it carries no association at all
        // until one is made.
        assert!(
            elements(&body, "routeTableAssociationId").is_empty(),
            "case {case:?}: expected no association on a fresh table, got {body}"
        );

        // The VPC's own main table is there beside it, with the implicit association.
        let of_vpc = ok(
            case,
            &router,
            "DescribeRouteTables",
            &filters(&[("vpc-id", &[&vpc_id]), ("association.main", &["true"])]),
        )
        .await;
        let main_tables = elements(&of_vpc, "routeTableId");
        assert_eq!(
            main_tables.len(),
            2,
            "case {case:?}: expected the main table reported with its association's own \
             routeTableId, got {of_vpc}"
        );
        assert!(
            !main_tables.contains(&route_table_id),
            "case {case:?}: expected the created table not to be the main one, got {of_vpc}"
        );
    }

    // A VPC id naming no VPC is a refusal rather than a table in no VPC.
    let (status, body) = call(
        &router,
        "CreateRouteTable",
        &[param("VpcId", "vpc-00000000000000000")],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with body {body}");
    assert_eq!(
        error_code(&body),
        "InvalidVpcID.NotFound",
        "expected the VPC not-found refusal, got {body}"
    );
}

/// The same rule one address family along: "routes to IPv4 and IPv6 addresses within a VPC"
/// are both local routes (Amazon VPC User Guide, "Route tables"), so a table of a VPC that
/// has an IPv6 block associated comes with a local route for THAT block as well as for its
/// IPv4 one.
#[tokio::test]
async fn a_route_table_of_a_dual_stack_vpc_comes_with_a_local_route_per_address_family() {
    let router = emulator();
    let case = "an Amazon-provided IPv6 block";
    let body = ok(
        case,
        &router,
        "CreateVpc",
        &[
            param("CidrBlock", "10.70.0.0/16"),
            param("AmazonProvidedIpv6CidrBlock", "true"),
        ],
    )
    .await;
    let vpc_id = member(case, &body, "vpcId");
    let ipv6_cidr_block = member(case, &body, "ipv6CidrBlock");

    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert_eq!(
        elements(&read, "destinationCidrBlock"),
        vec!["10.70.0.0/16".to_string()],
        "case {case:?}: expected the IPv4 local route, got {read}"
    );
    assert_eq!(
        elements(&read, "destinationIpv6CidrBlock"),
        vec![ipv6_cidr_block.clone()],
        "case {case:?}: expected a local route for the VPC's IPv6 block, got {read}"
    );
    // Both are the table's own local routes, so both carry that origin and the `local`
    // target — one per block rather than one route carrying two destinations.
    assert_eq!(
        elements(&read, "origin"),
        vec!["CreateRouteTable".to_string(), "CreateRouteTable".to_string()],
        "case {case:?}: expected both local routes to carry the create's origin, got {read}"
    );
    assert_eq!(
        elements(&read, "gatewayId"),
        vec!["local".to_string(), "local".to_string()],
        "case {case:?}: expected both local routes to target local, got {read}"
    );
}

// ---------------------------------------------------------------------------
// CreateRoute
// ---------------------------------------------------------------------------

/// The rule: a route matches ONE destination — "we modify the specified CIDR block to its
/// canonical form" (`CreateRouteRequest$DestinationCidrBlock`) — and points it at ONE
/// target of the route table's own VPC, and the route reads back through
/// `DescribeRouteTables`.
///
/// The rows differ in the destination's value, its size, how many host bits the request set
/// and its address family, which is what the canonical form generalises over.
#[tokio::test]
async fn create_route_stores_one_route_per_destination_against_an_attached_gateway() {
    let router = emulator();
    let case = "the VPC the routes are in";
    let vpc_id = create_vpc(case, &router, "10.40.0.0/16").await;
    let internet_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &internet_gateway_id),
            param("VpcId", &vpc_id),
        ],
    )
    .await;

    // (case, the member the destination travels in, the requested value, its canonical form)
    let cases = [
        (
            "every IPv4 address",
            "DestinationCidrBlock",
            "0.0.0.0/0",
            "0.0.0.0/0",
        ),
        (
            "a /24",
            "DestinationCidrBlock",
            "192.0.2.0/24",
            "192.0.2.0/24",
        ),
        (
            "host bits set",
            "DestinationCidrBlock",
            "198.51.100.77/26",
            "198.51.100.64/26",
        ),
        (
            "a single address",
            "DestinationCidrBlock",
            "203.0.113.9/32",
            "203.0.113.9/32",
        ),
        (
            "every IPv6 address",
            "DestinationIpv6CidrBlock",
            "::/0",
            "::/0",
        ),
        (
            "an IPv6 block with host bits set",
            "DestinationIpv6CidrBlock",
            "2001:db8:1234::5/48",
            "2001:db8:1234::/48",
        ),
    ];

    for (case, member_name, requested, canonical) in cases {
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let body = ok(
            case,
            &router,
            "CreateRoute",
            &[
                param("RouteTableId", &route_table_id),
                param(member_name, requested),
                param("GatewayId", &internet_gateway_id),
            ],
        )
        .await;
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected CreateRoute to answer true, got {body}"
        );

        let read = describe_route_table(case, &router, &route_table_id).await;
        let destination_element = if member_name == "DestinationCidrBlock" {
            "destinationCidrBlock"
        } else {
            "destinationIpv6CidrBlock"
        };
        assert!(
            elements(&read, destination_element).contains(&canonical.to_string()),
            "case {case:?}: expected the route for {canonical} in the table, got {read}"
        );
        assert!(
            elements(&read, "gatewayId").contains(&internet_gateway_id),
            "case {case:?}: expected the route's target to be the attached gateway, got \
             {read}"
        );
        assert!(
            elements(&read, "origin").contains(&"CreateRoute".to_string()),
            "case {case:?}: expected the route's origin to be CreateRoute, got {read}"
        );

        // One destination has one target, so the same destination again is refused rather
        // than replacing the route that holds.
        let (status, duplicate) = call(
            &router,
            "CreateRoute",
            &[
                param("RouteTableId", &route_table_id),
                param(member_name, requested),
                param("GatewayId", &internet_gateway_id),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate destination refused, got {status} with \
             body {duplicate}"
        );
        assert_eq!(
            error_code(&duplicate),
            "RouteAlreadyExists",
            "case {case:?}: expected RouteAlreadyExists, got {duplicate}"
        );
        assert!(
            error_message(&duplicate).contains(canonical),
            "case {case:?}: expected the refusal to name {canonical}, got {}",
            error_message(&duplicate)
        );
    }
}

/// The rule's refusal half for a route: a destination the request leaves out or states
/// twice names no route, a target naming no resource of the account is that resource's own
/// not-found refusal, and a gateway attached to ANOTHER VPC cannot carry this table's
/// traffic.
#[tokio::test]
async fn create_route_refuses_a_destination_or_a_target_it_cannot_resolve() {
    let router = emulator();
    let case = "the route table under test";
    let vpc_id = create_vpc(case, &router, "10.41.0.0/16").await;
    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let other_vpc_id = create_vpc(case, &router, "10.42.0.0/16").await;
    let foreign_gateway_id = create_internet_gateway(case, &router, &[]).await;
    ok(
        case,
        &router,
        "AttachInternetGateway",
        &[
            param("InternetGatewayId", &foreign_gateway_id),
            param("VpcId", &other_vpc_id),
        ],
    )
    .await;
    let unattached_gateway_id = create_internet_gateway(case, &router, &[]).await;

    // (case, the parameters beside the route table, the expected error code)
    let cases: [PairsRow<'_>; 6] = [
        (
            "no destination at all",
            &[("GatewayId", "")],
            "MissingParameter",
        ),
        (
            "no target at all",
            &[("DestinationCidrBlock", "0.0.0.0/0")],
            "MissingParameter",
        ),
        (
            "two destinations",
            &[
                ("DestinationCidrBlock", "0.0.0.0/0"),
                ("DestinationIpv6CidrBlock", "::/0"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a destination that is no CIDR block",
            &[("DestinationCidrBlock", "not-a-block")],
            "InvalidParameterValue",
        ),
        (
            "a gateway id naming nothing",
            &[
                ("DestinationCidrBlock", "0.0.0.0/0"),
                ("GatewayId", "igw-00000000000000000"),
            ],
            "InvalidGatewayID.NotFound",
        ),
        (
            "a gateway attached to another VPC",
            &[("DestinationCidrBlock", "0.0.0.0/0")],
            "InvalidParameterValue",
        ),
    ];

    for (index, (case, extra, expected_code)) in cases.into_iter().enumerate() {
        let mut params = vec![param("RouteTableId", &route_table_id)];
        for (name, value) in extra {
            if value.is_empty() {
                continue;
            }
            params.push(param(name, value));
        }
        // The last row's target is the gateway of the OTHER VPC, named here because its id
        // is only known at run time.
        if index == 5 {
            params.push(param("GatewayId", &foreign_gateway_id));
        }
        let (status, body) = call(&router, "CreateRoute", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // An unattached gateway is in the same position as one of another VPC: it carries no
    // traffic for this table's network.
    let (status, body) = call(
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", &route_table_id),
            param("DestinationCidrBlock", "0.0.0.0/0"),
            param("GatewayId", &unattached_gateway_id),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an unattached gateway refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "expected the different-networks refusal, got {body}"
    );

    // A route table id naming nothing is refused too.
    let (status, body) = call(
        &router,
        "CreateRoute",
        &[
            param("RouteTableId", "rtb-00000000000000000"),
            param("DestinationCidrBlock", "0.0.0.0/0"),
            param("GatewayId", &unattached_gateway_id),
        ],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with body {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRouteTableID.NotFound",
        "expected the route table not-found refusal, got {body}"
    );

    // Nothing above stored a route: the table still holds only the local one.
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert_eq!(
        elements(&read, "destinationCidrBlock"),
        vec!["10.41.0.0/16".to_string()],
        "expected only the local route after every refusal, got {read}"
    );
}

// ---------------------------------------------------------------------------
// AssociateRouteTable
// ---------------------------------------------------------------------------

/// The rule: a subnet is associated with a route table of ITS OWN VPC, the association is
/// reported on that table by `DescribeRouteTables` under the id the associate answered, and
/// "each subnet in your VPC must be associated with a route table" — with exactly one, so a
/// second association for the same subnet is refused rather than added beside it.
///
/// The rows differ in how many subnets one table covers ("a route table can be associated
/// with multiple subnets" — `AssociateRouteTable`), which is what the association set
/// generalises over.
#[tokio::test]
async fn associate_route_table_binds_each_subnet_to_one_table_of_its_own_vpc() {
    let router = emulator();
    // (case, the VPC's block, the subnet blocks one table is associated with)
    let cases: [(&str, &str, &[&str]); 3] = [
        ("one subnet", "10.50.0.0/16", &["10.50.1.0/24"]),
        (
            "two subnets",
            "10.51.0.0/16",
            &["10.51.1.0/24", "10.51.2.0/24"],
        ),
        (
            "three subnets",
            "10.52.0.0/16",
            &["10.52.1.0/24", "10.52.2.0/24", "10.52.3.0/24"],
        ),
    ];

    for (case, vpc_cidr, subnet_cidrs) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let mut associations = Vec::new();
        let mut subnet_ids = Vec::new();
        for subnet_cidr in subnet_cidrs {
            let subnet_id = create_subnet(case, &router, &vpc_id, subnet_cidr).await;
            let body = ok(
                case,
                &router,
                "AssociateRouteTable",
                &[
                    param("RouteTableId", &route_table_id),
                    param("SubnetId", &subnet_id),
                ],
            )
            .await;
            let association_id = member(case, &body, "associationId");
            assert!(
                is_long_id("rtbassoc", &association_id),
                "case {case:?}: expected an rtbassoc-… id, got {association_id:?}"
            );
            assert_eq!(
                element(&body, "state").as_deref(),
                Some("associated"),
                "case {case:?}: expected the association reported associated, got {body}"
            );
            associations.push(association_id);
            subnet_ids.push(subnet_id);
        }

        // Every association is on the table the associate named, under the id it answered.
        let read = describe_route_table(case, &router, &route_table_id).await;
        assert_eq!(
            elements(&read, "routeTableAssociationId"),
            associations,
            "case {case:?}: expected the table to report every association, got {read}"
        );
        assert_eq!(
            elements(&read, "subnetId"),
            subnet_ids,
            "case {case:?}: expected the table to report every associated subnet, got {read}"
        );

        // The documented filter selects the table by one of its associated subnets.
        let filtered = ok(
            case,
            &router,
            "DescribeRouteTables",
            &filters(&[("association.subnet-id", &[subnet_ids[0].as_str()])]),
        )
        .await;
        assert!(
            elements(&filtered, "routeTableId").contains(&route_table_id),
            "case {case:?}: expected the association filter to select the table, got \
             {filtered}"
        );

        // A second association for a subnet that already has one is refused.
        let second_table_id = create_route_table(case, &router, &vpc_id).await;
        let (status, body) = call(
            &router,
            "AssociateRouteTable",
            &[
                param("RouteTableId", &second_table_id),
                param("SubnetId", &subnet_ids[0]),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second association refused, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "Resource.AlreadyAssociated",
            "case {case:?}: expected Resource.AlreadyAssociated, got {body}"
        );
        assert!(
            error_message(&body).contains(&route_table_id),
            "case {case:?}: expected the refusal to name the table that holds it, got {}",
            error_message(&body)
        );
    }
}

/// The rule's cross-VPC half: "associates a subnet IN YOUR VPC … with a route table IN YOUR
/// VPC" — a route table routes for one VPC, so a subnet of another is refused, as is an id
/// naming no resource at all.
#[tokio::test]
async fn associate_route_table_refuses_a_subnet_of_another_vpc_or_an_id_naming_nothing() {
    let router = emulator();
    let case = "the two VPCs";
    let vpc_id = create_vpc(case, &router, "10.60.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.61.0.0/16").await;
    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let foreign_subnet_id = create_subnet(case, &router, &other_vpc_id, "10.61.1.0/24").await;
    let subnet_id = create_subnet(case, &router, &vpc_id, "10.60.1.0/24").await;

    // (case, the table, the subnet, the expected error code)
    let cases = [
        (
            "a subnet of another VPC",
            route_table_id.as_str(),
            foreign_subnet_id.as_str(),
            "InvalidParameterValue",
        ),
        (
            "a route table id naming nothing",
            "rtb-00000000000000000",
            subnet_id.as_str(),
            "InvalidRouteTableID.NotFound",
        ),
        (
            "a subnet id naming nothing",
            route_table_id.as_str(),
            "subnet-00000000000000000",
            "InvalidSubnetID.NotFound",
        ),
    ];

    for (case, table, subnet, expected_code) in cases {
        let (status, body) = call(
            &router,
            "AssociateRouteTable",
            &[param("RouteTableId", table), param("SubnetId", subnet)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing above associated anything: the table still carries no association.
    let read = describe_route_table(case, &router, &route_table_id).await;
    assert!(
        elements(&read, "routeTableAssociationId").is_empty(),
        "expected no association after every refusal, got {read}"
    );
}

// ---------------------------------------------------------------------------
// DisassociateRouteTable / DeleteRouteTable
// ---------------------------------------------------------------------------

/// The rule: the path comes apart the way it was built — "after you perform this action, the
/// subnet no longer uses the routes in the route table" (`DisassociateRouteTable`), and "you
/// must disassociate the route table from any subnets before you can delete it. You can't
/// delete the main route table" (`DeleteRouteTable`) — so a table a subnet still uses is
/// refused, a table nothing uses is removed, and its own describe then reports it for nobody.
///
/// The rows differ in how many subnets the table covers, which is what the set a delete has
/// to find empty generalises over.
#[tokio::test]
async fn a_route_table_is_deleted_once_every_subnet_is_disassociated_from_it() {
    let router = emulator();
    // (case, the VPC's block, the subnet blocks associated with the one table)
    let cases: [(&str, &str, &[&str]); 3] = [
        ("no subnet", "10.70.0.0/16", &[]),
        ("one subnet", "10.71.0.0/16", &["10.71.1.0/24"]),
        (
            "two subnets",
            "10.72.0.0/16",
            &["10.72.1.0/24", "10.72.2.0/24"],
        ),
    ];

    for (case, vpc_cidr, subnet_cidrs) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let route_table_id = create_route_table(case, &router, &vpc_id).await;
        let mut associations = Vec::new();
        for subnet_cidr in subnet_cidrs {
            let subnet_id = create_subnet(case, &router, &vpc_id, subnet_cidr).await;
            let body = ok(
                case,
                &router,
                "AssociateRouteTable",
                &[
                    param("RouteTableId", &route_table_id),
                    param("SubnetId", &subnet_id),
                ],
            )
            .await;
            associations.push(member(case, &body, "associationId"));
        }

        // A table a subnet still uses has a dependency, so the delete is refused rather than
        // carried out under that subnet.
        if !associations.is_empty() {
            let (status, body) = call(
                &router,
                "DeleteRouteTable",
                &[param("RouteTableId", &route_table_id)],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the delete of an associated table refused, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DependencyViolation",
                "case {case:?}: expected DependencyViolation, got {body}"
            );
        }

        for association_id in &associations {
            ok(
                case,
                &router,
                "DisassociateRouteTable",
                &[param("AssociationId", association_id)],
            )
            .await;
            // A disassociate is not idempotent: the association it took away names nothing
            // now, which is what a caller that disassociates twice has to be able to tell.
            let (status, body) = call(
                &router,
                "DisassociateRouteTable",
                &[param("AssociationId", association_id)],
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a second disassociate refused, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidAssociationID.NotFound",
                "case {case:?}: expected InvalidAssociationID.NotFound, got {body}"
            );
        }

        // The table reports no association at all now, and the subnets go on using the VPC's
        // main table, which is still there.
        let read = describe_route_table(case, &router, &route_table_id).await;
        assert!(
            elements(&read, "routeTableAssociationId").is_empty(),
            "case {case:?}: expected no association left on the table, got {read}"
        );

        ok(
            case,
            &router,
            "DeleteRouteTable",
            &[param("RouteTableId", &route_table_id)],
        )
        .await;
        let (status, body) = call(
            &router,
            "DescribeRouteTables",
            &[param("RouteTableId.1", &route_table_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted table to be reported for nobody, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRouteTableID.NotFound",
            "case {case:?}: expected InvalidRouteTableID.NotFound, got {body}"
        );

        // "You can't delete the main route table": the table the VPC came with is refused,
        // and it is still the VPC's after the refusal.
        let main_table_id = {
            let read = ok(
                case,
                &router,
                "DescribeRouteTables",
                &filters(&[("vpc-id", &[vpc_id.as_str()])]),
            )
            .await;
            elements(&read, "routeTableId")
                .into_iter()
                .next()
                .unwrap_or_default()
        };
        let (status, body) = call(
            &router,
            "DeleteRouteTable",
            &[param("RouteTableId", &main_table_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete of the main table refused, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DependencyViolation",
            "case {case:?}: expected DependencyViolation for the main table, got {body}"
        );
        let read = describe_route_table(case, &router, &main_table_id).await;
        assert!(
            elements(&read, "routeTableId").contains(&main_table_id),
            "case {case:?}: expected the main table still held after the refusal, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateVpcEndpoint with route tables / DeleteVpcEndpoints
// ---------------------------------------------------------------------------

/// The rule: "we automatically add a route to the route table with a destination of the prefix
/// list for the service and a target of the gateway endpoint" (Amazon Web Services PrivateLink
/// User Guide, "Gateway endpoints") — so a gateway endpoint created for route tables of its own
/// VPC puts that one route in EVERY table it names and in no table it does not, and "when you
/// delete a gateway endpoint, we delete the endpoint routes in the route tables for the
/// endpoint" (`DeleteVpcEndpoints`) takes them away again.
///
/// The rows differ in how many of the VPC's tables the endpoint names, which is what the
/// association set generalises over.
#[tokio::test]
async fn a_gateway_endpoint_routes_every_table_it_names_through_itself() {
    let router = emulator();
    /// The endpoint service of this Region a gateway endpoint is created for, spelled the way
    /// a gateway endpoint's service name is (`com.amazonaws.<region>.<service>`), and the one
    /// `DescribePrefixLists` publishes a prefix list for.
    const SERVICE_NAME: &str = "com.amazonaws.us-east-1.s3";

    // (case, the VPC's block, how many of its three tables the endpoint names)
    let cases: [(&str, &str, usize); 3] = [
        ("one table", "10.80.0.0/16", 1),
        ("two tables", "10.81.0.0/16", 2),
        ("three tables", "10.82.0.0/16", 3),
    ];

    for (case, vpc_cidr, named) in cases {
        let vpc_id = create_vpc(case, &router, vpc_cidr).await;
        let tables = {
            let mut tables = Vec::new();
            for _ in 0..3 {
                tables.push(create_route_table(case, &router, &vpc_id).await);
            }
            tables
        };
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", SERVICE_NAME),
            param("VpcEndpointType", "Gateway"),
        ];
        for (index, route_table_id) in tables.iter().take(named).enumerate() {
            params.push(param(&format!("RouteTableId.{}", index + 1), route_table_id));
        }
        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        // "(Gateway endpoint) The IDs of the route tables associated with the endpoint" — the
        // set the create answers with, read out of that member's own list.
        let reported_tables = elements(
            &element(&body, "routeTableIdSet")
                .unwrap_or_else(|| panic!("case {case:?}: no routeTableIdSet in {body}")),
            "item",
        );
        assert_eq!(
            reported_tables,
            tables[..named].to_vec(),
            "case {case:?}: expected the endpoint to report the tables it was created for, got \
             {body}"
        );

        for (index, route_table_id) in tables.iter().enumerate() {
            let read = describe_route_table(case, &router, route_table_id).await;
            let through_the_endpoint = elements(&read, "gatewayId")
                .into_iter()
                .filter(|target| target == &endpoint_id)
                .count();
            let expected = usize::from(index < named);
            assert_eq!(
                through_the_endpoint, expected,
                "case {case:?}: expected {expected} route(s) through {endpoint_id} in table \
                 {route_table_id}, got {read}"
            );
            // The destination is the SERVICE's prefix list, which that service publishes under
            // the endpoint service's own name.
            if expected == 1 {
                let prefix_list_id = element(&read, "destinationPrefixListId")
                    .unwrap_or_else(|| panic!("case {case:?}: no prefix list route in {read}"));
                let published = ok(
                    case,
                    &router,
                    "DescribePrefixLists",
                    &[param("PrefixListId.1", &prefix_list_id)],
                )
                .await;
                assert_eq!(
                    element(&published, "prefixListName").as_deref(),
                    Some(SERVICE_NAME),
                    "case {case:?}: expected the route's prefix list to be {SERVICE_NAME}'s, got \
                     {published}"
                );
            }
            // The local route every table comes with is untouched by the endpoint.
            assert!(
                elements(&read, "gatewayId").contains(&"local".to_string()),
                "case {case:?}: expected the table to keep its local route, got {read}"
            );
        }

        // The endpoint's deletion takes its routes with it, and the endpoint is then reported
        // for nobody.
        let deleted = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert!(
            elements(&deleted, "code").is_empty(),
            "case {case:?}: expected the delete to report nothing unsuccessful, got {deleted}"
        );
        for route_table_id in &tables {
            let read = describe_route_table(case, &router, route_table_id).await;
            assert!(
                !elements(&read, "gatewayId").contains(&endpoint_id),
                "case {case:?}: expected the endpoint's route gone from {route_table_id}, got \
                 {read}"
            );
        }
        let (status, body) = call(
            &router,
            "DescribeVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted endpoint reported for nobody, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcEndpointId.NotFound",
            "case {case:?}: expected InvalidVpcEndpointId.NotFound, got {body}"
        );
        // A second delete of the same endpoint reports it as one it could not delete rather
        // than refusing the call: "information about the VPC endpoints that were not
        // successfully deleted". The per-item code a bulk delete words that cause with is its
        // own, asserted with the sentence beside it by
        // [`super::delete_vpc_endpoints`].
        let again = ok(
            case,
            &router,
            "DeleteVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            elements(&again, "code"),
            vec!["InvalidVpcEndpoint.NotFound".to_string()],
            "case {case:?}: expected the second delete to report the endpoint unsuccessful, got \
             {again}"
        );
    }
}

/// The rule's own-VPC half: a gateway endpoint is only reached through route tables of ITS
/// VPC, so a table of another VPC — or an id naming no table at all — is refused, and the
/// endpoint is not created.
#[tokio::test]
async fn a_gateway_endpoint_refuses_a_route_table_outside_its_own_vpc() {
    let router = emulator();
    let case = "the two VPCs";
    let vpc_id = create_vpc(case, &router, "10.90.0.0/16").await;
    let other_vpc_id = create_vpc(case, &router, "10.91.0.0/16").await;
    let foreign_table_id = create_route_table(case, &router, &other_vpc_id).await;

    // (case, the route table the endpoint names, the expected error code)
    let cases = [
        (
            "a route table of another VPC",
            foreign_table_id.as_str(),
            "InvalidParameterValue",
        ),
        (
            "a route table id naming nothing",
            "rtb-00000000000000000",
            "InvalidRouteTableID.NotFound",
        ),
    ];

    for (case, route_table_id, expected_code) in cases {
        let (status, body) = call(
            &router,
            "CreateVpcEndpoint",
            &[
                param("VpcId", &vpc_id),
                param("ServiceName", "com.amazonaws.us-east-1.s3"),
                param("VpcEndpointType", "Gateway"),
                param("RouteTableId.1", route_table_id),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing above created an endpoint: the VPC holds none.
    let read = ok(
        case,
        &router,
        "DescribeVpcEndpoints",
        &filters(&[("vpc-id", &[vpc_id.as_str()])]),
    )
    .await;
    assert!(
        element(&read, "vpcEndpointId").is_none(),
        "expected no endpoint after every refusal, got {read}"
    );
}

// ---------------------------------------------------------------------------
// The private estate: a subnet's own route table, read through its association
// ---------------------------------------------------------------------------

/// One route of a route table, narrowed to the members the estate tests assert on.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReportedRoute {
    destination_cidr_block: Option<String>,
    destination_prefix_list_id: Option<String>,
    gateway_id: Option<String>,
    origin: Option<String>,
    state: Option<String>,
}

/// One association of a route table, narrowed the same way.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReportedAssociation {
    route_table_association_id: Option<String>,
    subnet_id: Option<String>,
    main: Option<String>,
    state: Option<String>,
}

/// One route table a `DescribeRouteTables` reported, with its routes and associations.
#[derive(Debug, Default)]
struct ReportedTable {
    route_table_id: String,
    vpc_id: Option<String>,
    routes: Vec<ReportedRoute>,
    associations: Vec<ReportedAssociation>,
}

/// Every route table a `DescribeRouteTables` body reports, read through the document so a
/// `<state>` of a route is never confused with one of an association: the `<routeTableSet>`
/// entries, each with its `<routeSet>` and `<associationSet>` entries
/// (`RouteTable$Routes`/`$Associations`, xmlName `routeSet`/`associationSet`).
fn reported_tables(case: &str, body: &str) -> Vec<ReportedTable> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "routeTableSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| ReportedTable {
            route_table_id: child_text(item, "routeTableId").unwrap_or_default(),
            vpc_id: child_text(item, "vpcId"),
            routes: set_items(item, "routeSet")
                .into_iter()
                .map(|route| ReportedRoute {
                    destination_cidr_block: child_text(route, "destinationCidrBlock"),
                    destination_prefix_list_id: child_text(route, "destinationPrefixListId"),
                    gateway_id: child_text(route, "gatewayId"),
                    origin: child_text(route, "origin"),
                    state: child_text(route, "state"),
                })
                .collect(),
            associations: set_items(item, "associationSet")
                .into_iter()
                .map(|association| ReportedAssociation {
                    route_table_association_id: child_text(
                        association,
                        "routeTableAssociationId",
                    ),
                    subnet_id: child_text(association, "subnetId"),
                    main: child_text(association, "main"),
                    state: child(association, "associationState")
                        .and_then(|state| child_text(state, "state")),
                })
                .collect(),
        })
        .collect()
}

/// The route tables `DescribeRouteTables` reports for the subnet `subnet_id`, selected the
/// way a client asks which table a subnet uses: the documented `association.subnet-id`
/// filter ("The ID of the subnet involved in the association", `DescribeRouteTables$Filters`).
async fn tables_of_subnet(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
) -> Vec<ReportedTable> {
    let body = ok(
        case,
        router,
        "DescribeRouteTables",
        &filters(&[("association.subnet-id", &[subnet_id])]),
    )
    .await;
    reported_tables(case, &body)
}

/// The routes of `table` split into the ones inside the VPC (target `local`, the route every
/// table comes with) and the ones OUT of it (every other target).
fn local_and_outbound(table: &ReportedTable) -> (Vec<&ReportedRoute>, Vec<&ReportedRoute>) {
    table
        .routes
        .iter()
        .partition(|route| route.gateway_id.as_deref() == Some("local"))
}

/// The id of the prefix list this Region publishes for the endpoint service `service_name`,
/// read back through `DescribePrefixLists`' own `prefix-list-name` filter rather than derived
/// here: the route a gateway endpoint adds has THAT list as its destination.
async fn published_prefix_list_id(
    case: &str,
    router: &MultiServiceRouter,
    service_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        "DescribePrefixLists",
        &filters(&[("prefix-list-name", &[service_name])]),
    )
    .await;
    let published = elements(&body, "prefixListId");
    assert_eq!(
        published.len(),
        1,
        "case {case:?}: expected one prefix list published for {service_name}, got {body}"
    );
    published[0].clone()
}

/// A gateway endpoint of `vpc_id` to `service_name`, created for `route_tables`, as its id.
async fn create_gateway_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    service_name: &str,
    route_tables: &[String],
) -> String {
    let mut params = vec![
        param("VpcId", vpc_id),
        param("ServiceName", service_name),
        param("VpcEndpointType", "Gateway"),
    ];
    for (index, route_table_id) in route_tables.iter().enumerate() {
        params.push(param(&format!("RouteTableId.{}", index + 1), route_table_id));
    }
    let body = ok(case, router, "CreateVpcEndpoint", &params).await;
    member(case, &body, "vpcEndpointId")
}

/// Associate `subnet_id` with `route_table_id`, as the association's id.
async fn associate(
    case: &str,
    router: &MultiServiceRouter,
    route_table_id: &str,
    subnet_id: &str,
) -> String {
    let body = ok(
        case,
        router,
        "AssociateRouteTable",
        &[
            param("RouteTableId", route_table_id),
            param("SubnetId", subnet_id),
        ],
    )
    .await;
    member(case, &body, "associationId")
}

/// A `/24` inside the `/16` block `vpc_cidr`, numbered by `index` — one subnet block per
/// subnet a row of the estate table asks for. A row whose block is not a `/16` of four
/// octets panics here rather than yielding a block of some other network.
fn subnet_block(vpc_cidr: &str, index: usize) -> String {
    let network = vpc_cidr
        .strip_suffix("/16")
        .unwrap_or_else(|| panic!("the row's VPC block {vpc_cidr:?} must be a /16"));
    let mut octets = network.split('.');
    let first = octets
        .next()
        .unwrap_or_else(|| panic!("the row's VPC block {vpc_cidr:?} must carry four octets"));
    let second = octets
        .next()
        .unwrap_or_else(|| panic!("the row's VPC block {vpc_cidr:?} must carry four octets"));
    format!("{first}.{second}.{}.0/24", index + 1)
}

/// A VPC endpoint as `DescribeVpcEndpoints` reports it, narrowed to the members the estate
/// tests read it back by.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReportedEndpoint {
    vpc_endpoint_type: Option<String>,
    state: Option<String>,
    vpc_id: Option<String>,
    route_table_ids: Vec<String>,
}

/// The endpoint a row of the estate table reads back through `DescribeVpcEndpoints`.
async fn described_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> ReportedEndpoint {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &[param("VpcEndpointId.1", endpoint_id)],
    )
    .await;
    let parsed = document(case, &body);
    let endpoint = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "vpcEndpointId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no endpoint in {body}"));
    ReportedEndpoint {
        vpc_endpoint_type: child_text(endpoint, "vpcEndpointType"),
        state: child_text(endpoint, "state"),
        vpc_id: child_text(endpoint, "vpcId"),
        route_table_ids: set_items(endpoint, "routeTableIdSet")
            .into_iter()
            .map(|item| item.text().unwrap_or_default().to_string())
            .collect(),
    }
}

/// One row of the private-estate table: a VPC, the endpoint service the gateway endpoint is
/// created for, how many subnets are explicitly associated with the table that endpoint
/// names, and whether the endpoint is created BEFORE those associations or after them.
struct EstateRow {
    case: &'static str,
    vpc_cidr: &'static str,
    service: &'static str,
    associated: usize,
    endpoint_first: bool,
}

/// The rule a client reading its private estate depends on: a gateway endpoint's route —
/// "a destination of the prefix list for the service and a target of the gateway endpoint"
/// (Amazon Web Services PrivateLink User Guide, "Gateway endpoints") — is a route OF every
/// route table the endpoint names, so the table a subnet is EXPLICITLY associated with
/// reports, beside the local route the table came with, exactly one route out of the VPC per
/// gateway endpoint named for that table, and a table no endpoint named reports none. The
/// read is the one a client makes: `DescribeRouteTables` narrowed by `association.subnet-id`,
/// which selects on explicit associations only ("A subnet ID is not returned for an implicit
/// association", `RouteTableAssociation$SubnetId`), so a subnet left on its VPC's main table
/// is selected by no such filter.
///
/// The rows differ along what the rule generalises over: the endpoint service (the two a
/// Region publishes a gateway-endpoint prefix list for, read out of `DescribePrefixLists`),
/// how many subnets share the endpoint's table, and the ORDER the estate is built in — the
/// route is the endpoint's either way, so associating a subnet after the endpoint exists
/// reports the same table as associating it before.
#[tokio::test]
async fn a_subnets_own_table_reports_the_gateway_endpoint_as_its_route_out_of_the_vpc() {
    let router = emulator();
    let rows = [
        EstateRow {
            case: "s3, one subnet, the endpoint created after the association",
            vpc_cidr: "10.100.0.0/16",
            service: "s3",
            associated: 1,
            endpoint_first: false,
        },
        EstateRow {
            case: "dynamodb, two subnets, the endpoint created before the associations",
            vpc_cidr: "10.101.0.0/16",
            service: "dynamodb",
            associated: 2,
            endpoint_first: true,
        },
        EstateRow {
            case: "s3, three subnets, the endpoint created before the associations",
            vpc_cidr: "10.102.0.0/16",
            service: "s3",
            associated: 3,
            endpoint_first: true,
        },
        EstateRow {
            case: "dynamodb, one subnet, the endpoint created after the association",
            vpc_cidr: "10.103.0.0/16",
            service: "dynamodb",
            associated: 1,
            endpoint_first: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let service_name = format!("com.amazonaws.{REGION}.{}", row.service);
        let vpc_id = create_vpc(case, &router, row.vpc_cidr).await;
        // The table the endpoint is reached through, a second table of the same VPC that no
        // endpoint names, and the subnets of each — plus one subnet left on the VPC's main
        // table, which no explicit association covers.
        let endpoint_table = create_route_table(case, &router, &vpc_id).await;
        let other_table = create_route_table(case, &router, &vpc_id).await;
        let mut endpoint_subnets = Vec::new();
        for index in 0..row.associated {
            endpoint_subnets
                .push(create_subnet(case, &router, &vpc_id, &subnet_block(row.vpc_cidr, index)).await);
        }
        let other_subnet =
            create_subnet(case, &router, &vpc_id, &subnet_block(row.vpc_cidr, row.associated)).await;
        let unassociated_subnet = create_subnet(
            case,
            &router,
            &vpc_id,
            &subnet_block(row.vpc_cidr, row.associated + 1),
        )
        .await;

        let tables_for_endpoint = vec![endpoint_table.clone()];
        let endpoint_id = if row.endpoint_first {
            let endpoint_id =
                create_gateway_endpoint(case, &router, &vpc_id, &service_name, &tables_for_endpoint)
                    .await;
            for subnet_id in &endpoint_subnets {
                associate(case, &router, &endpoint_table, subnet_id).await;
            }
            endpoint_id
        } else {
            for subnet_id in &endpoint_subnets {
                associate(case, &router, &endpoint_table, subnet_id).await;
            }
            create_gateway_endpoint(case, &router, &vpc_id, &service_name, &tables_for_endpoint)
                .await
        };
        associate(case, &router, &other_table, &other_subnet).await;

        let prefix_list_id = published_prefix_list_id(case, &router, &service_name).await;

        // Every subnet on the endpoint's table reads back that one table, with the endpoint's
        // route as its only route out of the VPC.
        for subnet_id in &endpoint_subnets {
            let reported = tables_of_subnet(case, &router, subnet_id).await;
            let ids: Vec<&str> = reported
                .iter()
                .map(|table| table.route_table_id.as_str())
                .collect();
            assert_eq!(
                ids,
                vec![endpoint_table.as_str()],
                "case {case:?}: expected {subnet_id} to read back only {endpoint_table}, got \
                 {ids:?} from {reported:?}"
            );
            let table = &reported[0];
            assert_eq!(
                table.vpc_id.as_deref(),
                Some(vpc_id.as_str()),
                "case {case:?}: expected the table reported for {vpc_id}, got {table:?}"
            );
            let association = table
                .associations
                .iter()
                .find(|association| association.subnet_id.as_deref() == Some(subnet_id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no association for {subnet_id} on {table:?}")
                });
            assert_eq!(
                association.main.as_deref(),
                Some("false"),
                "case {case:?}: expected the association explicit rather than the main one, got \
                 {association:?}"
            );
            assert_eq!(
                association.state.as_deref(),
                Some("associated"),
                "case {case:?}: expected the association reported associated, got {association:?}"
            );
            assert!(
                association
                    .route_table_association_id
                    .as_deref()
                    .is_some_and(|id| is_long_id("rtbassoc", id)),
                "case {case:?}: expected an rtbassoc-… association id, got {association:?}"
            );

            let (local, outbound) = local_and_outbound(table);
            assert_eq!(
                local.len(),
                1,
                "case {case:?}: expected the one local route of {vpc_cidr}, got {local:?}",
                vpc_cidr = row.vpc_cidr
            );
            assert_eq!(
                local[0].destination_cidr_block.as_deref(),
                Some(row.vpc_cidr),
                "case {case:?}: expected the local route for the VPC's block, got {:?}",
                local[0]
            );
            assert_eq!(
                outbound.len(),
                1,
                "case {case:?}: expected exactly one route out of the VPC, got {outbound:?}"
            );
            let route = outbound[0];
            assert_eq!(
                route.destination_prefix_list_id.as_deref(),
                Some(prefix_list_id.as_str()),
                "case {case:?}: expected the route's destination to be {service_name}'s published \
                 prefix list {prefix_list_id}, got {route:?}"
            );
            assert_eq!(
                route.destination_cidr_block, None,
                "case {case:?}: expected a prefix-list route to carry no CIDR destination, got \
                 {route:?}"
            );
            assert_eq!(
                route.gateway_id.as_deref(),
                Some(endpoint_id.as_str()),
                "case {case:?}: expected the route's target to be {endpoint_id}, got {route:?}"
            );
            assert_eq!(
                route.origin.as_deref(),
                Some("CreateRoute"),
                "case {case:?}: expected the endpoint's route reported as added to the table, got \
                 {route:?}"
            );
            assert_eq!(
                route.state.as_deref(),
                Some("active"),
                "case {case:?}: expected the endpoint's route active, got {route:?}"
            );
        }

        // The subnet on the table no endpoint named has a table of its own, and no route out.
        let reported = tables_of_subnet(case, &router, &other_subnet).await;
        let ids: Vec<&str> = reported
            .iter()
            .map(|table| table.route_table_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![other_table.as_str()],
            "case {case:?}: expected {other_subnet} to read back only {other_table}, got {ids:?}"
        );
        let (_, outbound) = local_and_outbound(&reported[0]);
        assert!(
            outbound.is_empty(),
            "case {case:?}: expected no route out of the VPC on {other_table}, got {outbound:?}"
        );

        // The subnet nobody associated is on its VPC's main table, whose association carries
        // no subnet id, so the association filter selects nothing for it.
        let reported = tables_of_subnet(case, &router, &unassociated_subnet).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected no explicit association for {unassociated_subnet}, got \
             {reported:?}"
        );

        // The endpoint itself reads back as the gateway endpoint of that VPC, for that table.
        let read_back = described_endpoint(case, &router, &endpoint_id).await;
        assert_eq!(
            read_back,
            ReportedEndpoint {
                vpc_endpoint_type: Some("Gateway".to_string()),
                state: Some("available".to_string()),
                vpc_id: Some(vpc_id.clone()),
                route_table_ids: vec![endpoint_table.clone()],
            },
            "case {case:?}: expected the endpoint read back as the available Gateway endpoint of \
             {vpc_id} for {endpoint_table}, got {read_back:?}"
        );
    }
}

/// The same rule over the COUNT of endpoints on ONE table: a table named by several gateway
/// endpoints carries one route out of the VPC per endpoint, each to that endpoint's own
/// service prefix list, and "when you delete a gateway endpoint, we delete the endpoint routes
/// in the route tables for the endpoint" (`DeleteVpcEndpoints`) leaves exactly the routes of
/// the endpoints that remain — read, as a client reads it, through the subnet's association.
#[tokio::test]
async fn a_subnets_table_reports_one_route_out_per_gateway_endpoint_it_is_named_by() {
    let router = emulator();
    let case = "two gateway endpoints on one table";
    let vpc_cidr = "10.104.0.0/16";
    let vpc_id = create_vpc(case, &router, vpc_cidr).await;
    let route_table_id = create_route_table(case, &router, &vpc_id).await;
    let subnet_id = create_subnet(case, &router, &vpc_id, &subnet_block(vpc_cidr, 0)).await;
    associate(case, &router, &route_table_id, &subnet_id).await;

    // The services this Region publishes a gateway-endpoint prefix list for
    // (`DescribePrefixLists`, read back below), one endpoint each.
    let services = ["s3", "dynamodb"];
    let mut endpoints = Vec::new();
    for service in services {
        let service_name = format!("com.amazonaws.{REGION}.{service}");
        let endpoint_id = create_gateway_endpoint(
            case,
            &router,
            &vpc_id,
            &service_name,
            std::slice::from_ref(&route_table_id),
        )
        .await;
        let prefix_list_id = published_prefix_list_id(case, &router, &service_name).await;
        endpoints.push((endpoint_id, prefix_list_id));

        // After each create the subnet's table carries one route out per endpoint so far.
        let reported = tables_of_subnet(case, &router, &subnet_id).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the subnet's one table, got {reported:?}"
        );
        let (_, outbound) = local_and_outbound(&reported[0]);
        let mut pairs: Vec<(Option<&str>, Option<&str>)> = outbound
            .iter()
            .map(|route| {
                (
                    route.destination_prefix_list_id.as_deref(),
                    route.gateway_id.as_deref(),
                )
            })
            .collect();
        pairs.sort();
        let mut expected: Vec<(Option<&str>, Option<&str>)> = endpoints
            .iter()
            .map(|(endpoint_id, prefix_list_id)| {
                (Some(prefix_list_id.as_str()), Some(endpoint_id.as_str()))
            })
            .collect();
        expected.sort();
        assert_eq!(
            pairs, expected,
            "case {case:?}: expected one route out of the VPC per endpoint named for the table, \
             got {outbound:?}"
        );
    }

    // Deleting the first endpoint takes its route and only its route.
    let (deleted_endpoint, deleted_prefix_list) = endpoints.remove(0);
    let body = ok(
        case,
        &router,
        "DeleteVpcEndpoints",
        &[param("VpcEndpointId.1", &deleted_endpoint)],
    )
    .await;
    assert!(
        elements(&body, "code").is_empty(),
        "case {case:?}: expected the delete to report nothing unsuccessful, got {body}"
    );
    let reported = tables_of_subnet(case, &router, &subnet_id).await;
    let (_, outbound) = local_and_outbound(&reported[0]);
    let mut remaining: Vec<(Option<&str>, Option<&str>)> = outbound
        .iter()
        .map(|route| {
            (
                route.destination_prefix_list_id.as_deref(),
                route.gateway_id.as_deref(),
            )
        })
        .collect();
    remaining.sort();
    let mut expected: Vec<(Option<&str>, Option<&str>)> = endpoints
        .iter()
        .map(|(endpoint_id, prefix_list_id)| {
            (Some(prefix_list_id.as_str()), Some(endpoint_id.as_str()))
        })
        .collect();
    expected.sort();
    assert_eq!(
        remaining, expected,
        "case {case:?}: expected only the routes of the endpoints that remain, got {outbound:?}"
    );
    assert!(
        !remaining.contains(&(
            Some(deleted_prefix_list.as_str()),
            Some(deleted_endpoint.as_str())
        )),
        "case {case:?}: expected the deleted endpoint's route gone, got {outbound:?}"
    );
}
