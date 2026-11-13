// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVpcEndpoint`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a modify applies the changes the request carries to the STORED
//! endpoint it names — the subnets it is served through and the endpoint network interface in
//! each, the security groups those interfaces carry, the route tables a gateway endpoint is
//! reached from and the route each of them holds for it, the policy attached to it and
//! whether a private hosted zone is associated with its VPC — leaves every member the request
//! does not carry as it was, answers `Return` `true`, and `DescribeVpcEndpoints` reports
//! exactly that record; a request it cannot serve is refused with the EC2 code for its cause
//! and changes nothing.
//!
//! Each table row is a distinct input along what that rule generalises over: which member is
//! changed, whether it is added or removed, how many ids one call carries, which endpoint type
//! holds the member, and which member of the request is at fault.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The endpoint service every interface endpoint below is created to — one the Region offers
/// interface endpoints to.
const INTERFACE_SERVICE: &str = "com.amazonaws.us-east-1.logs";

/// The endpoint service every gateway endpoint below is created to — one the Region publishes
/// a prefix list for, which is what a gateway endpoint's route is destined for.
const GATEWAY_SERVICE: &str = "com.amazonaws.us-east-1.s3";

/// How long a poll waits for a move the clock owes before giving up. Far past
/// [`ec2::state::TRANSITION_DELAY`], so a loaded or slow host cannot fail the assertion: the
/// loop waits for the CONDITION, and this is only its ceiling.
const MOVE_DEADLINE: Duration = Duration::from_secs(30);

/// How long each turn of a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The emulator the tests drive, with the engine handle for the few members only the store
/// holds (an endpoint network interface's private address).
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
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

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The text of every `<item>` of the first set element named `set`.
///
/// Read through the XML parser rather than by substring, so the items of ONE set are never
/// confused with the identically-shaped items of another set of the same response.
fn set_items(case: &str, body: &str, set: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `name` member of every `<item>` of the first set element named `set` — for a set whose
/// items are structures (`dnsEntrySet`, `groupSet`).
fn set_members(case: &str, body: &str, set: &str, name: &str) -> Vec<String> {
    let document = super::wire::document(case, body);
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set)
        .map(|node| {
            node.children()
                .filter(|item| item.is_element() && item.tag_name().name() == "item")
                .filter_map(|item| super::wire::child(item, name))
                .map(|found| found.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block` in the Availability Zone `zone`, as its id.
///
/// The zone is named rather than left to the service: "we do not necessarily select a
/// different zone for each subnet" (`CreateSubnetRequest$AvailabilityZone`), and an endpoint's
/// one-interface-per-zone rule is about which zones its subnets are in.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", zone),
        ],
    )
    .await;
    member(case, &body, "subnetId")
}

/// The one VPC, with one subnet per Availability Zone letter in `zones`, each test builds its
/// endpoints in.
struct Network {
    vpc_id: String,
    /// The subnets, in the order their zones were given.
    subnets: Vec<String>,
}

/// Create the VPC and its subnets through the API, so every endpoint below is modified against
/// resources this emulator actually stores.
async fn network(case: &str, router: &MultiServiceRouter, zones: &[&str]) -> Network {
    let vpc_id = create_vpc(case, router, "10.60.0.0/16").await;
    let mut subnets = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        let cidr_block = format!("10.60.{}.0/24", index + 1);
        subnets.push(create_subnet(case, router, &vpc_id, &cidr_block, zone).await);
    }
    Network { vpc_id, subnets }
}

/// A security group of `vpc_id` named `group_name`, as its id.
async fn create_security_group(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", group_name),
            param("GroupDescription", "a group the modify suite created"),
        ],
    )
    .await;
    member(case, &body, "groupId")
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

/// An interface endpoint of `vpc_id` in `subnets`, as its id.
async fn create_interface_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    subnets: &[&str],
) -> String {
    let mut params = vec![
        param("VpcId", vpc_id),
        param("ServiceName", INTERFACE_SERVICE),
        param("VpcEndpointType", "Interface"),
    ];
    params.extend(indexed("SubnetId", subnets));
    let body = ok(case, router, "CreateVpcEndpoint", &params).await;
    member(case, &body, "vpcEndpointId")
}

/// A gateway endpoint of `vpc_id`, as its id.
async fn create_gateway_endpoint(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateVpcEndpoint",
        &[
            param("VpcId", vpc_id),
            param("ServiceName", GATEWAY_SERVICE),
            param("VpcEndpointType", "Gateway"),
        ],
    )
    .await;
    member(case, &body, "vpcEndpointId")
}

/// Turn on the two VPC attributes a private hosted zone needs: "to use private DNS, the VPC
/// must have DNS hostnames and DNS resolution enabled"
/// (`CreateVpcEndpointRequest$PrivateDnsEnabled`), the way a client does.
async fn enable_vpc_dns(case: &str, router: &MultiServiceRouter, vpc_id: &str) {
    for attribute in ["EnableDnsSupport", "EnableDnsHostnames"] {
        ok(
            case,
            router,
            "ModifyVpcAttribute",
            &[
                param("VpcId", vpc_id),
                param(&format!("{attribute}.Value"), "true"),
            ],
        )
        .await;
    }
}

/// The body `DescribeVpcEndpoints` answers for the one endpoint `endpoint_id`.
async fn describe_endpoint(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[endpoint_id]),
    )
    .await
}

/// One `ModifyVpcEndpoint` call on `endpoint_id` that has to succeed, with `Return` `true`
/// asserted — "Returns true if the request succeeds" (`ModifyVpcEndpointResult$Return`).
async fn modify(case: &str, router: &MultiServiceRouter, endpoint_id: &str, params: &[Param]) {
    let mut all = vec![param("VpcEndpointId", endpoint_id)];
    all.extend(params.iter().cloned());
    let body = ok(case, router, "ModifyVpcEndpoint", &all).await;
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: expected <return>true</return>, got {body}"
    );
}

/// Wait until the endpoint reports `available` again, which is what the clock owes one whose
/// endpoint network interfaces this call stood up or tore down.
///
/// A poll for the CONDITION, not a wait for a span: it looks again until the state holds or a
/// generous deadline passes, so the assertion cannot depend on how fast the host runs.
async fn settled(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let deadline = std::time::Instant::now() + MOVE_DEADLINE;
    loop {
        let described = describe_endpoint(case, router, endpoint_id).await;
        if element(&described, "state").as_deref()
            == Some(ec2::state::VPC_ENDPOINT_STATE_AVAILABLE)
        {
            return described;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected the clock to owe {endpoint_id} its available state, \
             got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// The subnets an endpoint is served through
// ---------------------------------------------------------------------------

/// What one row of the subnet table asks for, and what the endpoint must then report.
struct SubnetCase {
    case: &'static str,
    /// Indexes into the network's subnets to add.
    add: &'static [usize],
    /// Indexes into the network's subnets to remove.
    remove: &'static [usize],
    /// The subnets, by index, the endpoint must then be served through — in order.
    served: &'static [usize],
}

/// The rows, applied in order to ONE endpoint: an addition, the endpoint emptied, two
/// additions in one call, an addition and a removal together, and every subnet removed.
fn subnet_cases() -> Vec<SubnetCase> {
    vec![
        SubnetCase {
            case: "one subnet added in a free zone",
            add: &[1],
            remove: &[],
            served: &[0, 1],
        },
        SubnetCase {
            case: "every subnet removed, leaving the endpoint served through none",
            add: &[],
            remove: &[0, 1],
            served: &[],
        },
        SubnetCase {
            case: "two subnets added in one call",
            add: &[0, 2],
            remove: &[],
            served: &[0, 2],
        },
        SubnetCase {
            case: "a subnet removed and the zone it freed taken by another",
            add: &[1],
            remove: &[0],
            served: &[2, 1],
        },
        SubnetCase {
            case: "every remaining subnet removed",
            add: &[],
            remove: &[2, 1],
            served: &[],
        },
    ]
}

/// Every row is applied, and the endpoint reports exactly the subnets it leaves — one endpoint
/// network interface per subnet and one zonal DNS name per subnet beside the regional one —
/// read back through `DescribeVpcEndpoints`, so the answer is the record the store holds.
#[tokio::test]
async fn the_subnets_an_endpoint_serves_follow_the_additions_and_removals() {
    let (router, _engine) = emulator();
    let built = network(
        "the subnet table",
        &router,
        &["us-east-1a", "us-east-1b", "us-east-1c"],
    )
    .await;
    let endpoint_id = create_interface_endpoint(
        "the subnet table",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;

    for row in subnet_cases() {
        let case = row.case;
        let mut params: Vec<Param> = Vec::new();
        let added: Vec<&str> = row
            .add
            .iter()
            .map(|index| built.subnets[*index].as_str())
            .collect();
        let removed: Vec<&str> = row
            .remove
            .iter()
            .map(|index| built.subnets[*index].as_str())
            .collect();
        params.extend(indexed("AddSubnetId", &added));
        params.extend(indexed("RemoveSubnetId", &removed));
        modify(case, &router, &endpoint_id, &params).await;

        let described = settled(case, &router, &endpoint_id).await;
        let expected: Vec<String> = row
            .served
            .iter()
            .map(|index| built.subnets[*index].clone())
            .collect();
        assert_eq!(
            set_items(case, &described, "subnetIdSet"),
            expected,
            "case {case:?}: expected the endpoint to serve those subnets, got {described}"
        );
        // "An endpoint network interface is created … in an Availability Zone"
        // (`CreateVpcEndpointRequest$SubnetIds`): one per subnet, so an added subnet brings
        // one and a removed one takes its own away.
        assert_eq!(
            set_items(case, &described, "networkInterfaceIdSet").len(),
            expected.len(),
            "case {case:?}: expected one endpoint network interface per subnet, got {described}"
        );
        // "(Interface endpoint) The DNS entries for the endpoint" (`DnsEntries`): the
        // endpoint's regional name plus one per zone it is served in.
        let dns_names = set_members(case, &described, "dnsEntrySet", "dnsName");
        assert_eq!(
            dns_names.len(),
            expected.len() + 1,
            "case {case:?}: expected the regional name and one per zone, got {dns_names:?}"
        );
        assert!(
            dns_names
                .iter()
                .all(|name| name.starts_with(&format!("{endpoint_id}."))
                    || name.starts_with(&format!("{endpoint_id}-"))),
            "case {case:?}: expected every name to be the endpoint's own, got {dns_names:?}"
        );
    }
}

/// `SubnetConfiguration$Ipv4` places the endpoint network interface of a subnet being ADDED on
/// the address the request chose, out of that subnet's own assignable range.
#[tokio::test]
async fn a_subnet_configuration_places_an_added_interface_on_the_address_it_names() {
    // The engine handle is kept: the address an interface took is a member of the interface,
    // and `DescribeNetworkInterfaces` is still a scaffold, so the store is where it can be
    // read at all.
    let (router, engine) = emulator();
    let built = network(
        "subnet configurations",
        &router,
        &["us-east-1a", "us-east-1b", "us-east-1c"],
    )
    .await;
    let endpoint_id = create_interface_endpoint(
        "subnet configurations",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;

    // Rows: two addresses of two different subnets' assignable ranges, so the one reported is
    // the one asked for rather than whichever the service would have chosen.
    for (index, address) in [(1usize, "10.60.2.42"), (2usize, "10.60.3.200")] {
        let case = address;
        modify(
            case,
            &router,
            &endpoint_id,
            &[
                param("AddSubnetId.1", &built.subnets[index]),
                param("SubnetConfiguration.1.SubnetId", &built.subnets[index]),
                param("SubnetConfiguration.1.Ipv4", address),
            ],
        )
        .await;
        let described = settled(case, &router, &endpoint_id).await;
        let interfaces = set_items(case, &described, "networkInterfaceIdSet");
        let taken: Vec<String> = interfaces
            .iter()
            .filter_map(|network_interface_id| {
                engine
                    .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
                    .lock()
                    .network_interfaces
                    .get(network_interface_id)
                    .and_then(|interface| interface.private_ip_address.clone())
            })
            .collect();
        assert!(
            taken.iter().any(|held| held == address),
            "case {case:?}: expected an interface on the address the request named, got {taken:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The security groups the endpoint's interfaces carry
// ---------------------------------------------------------------------------

/// Adding and removing security groups changes what the endpoint reports AND what its endpoint
/// network interfaces carry, so the two cannot drift apart.
#[tokio::test]
async fn the_security_groups_follow_the_additions_and_removals() {
    let (router, engine) = emulator();
    let built = network("the group table", &router, &["us-east-1a"]).await;
    let first = create_security_group("the group table", &router, &built.vpc_id, "endpoint-one").await;
    let second =
        create_security_group("the group table", &router, &built.vpc_id, "endpoint-two").await;
    let endpoint_id = create_interface_endpoint(
        "the group table",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;
    // The group the create gave it: "we use the default security group for the VPC"
    // (`CreateVpcEndpointRequest$SecurityGroupIds`).
    let described = describe_endpoint("the group table", &router, &endpoint_id).await;
    let default_group = set_members("the group table", &described, "groupSet", "groupId")
        .first()
        .cloned()
        .unwrap_or_default();
    assert!(
        !default_group.is_empty(),
        "expected the create to have given the endpoint its VPC's default group, got {described}"
    );

    // Rows: one group added, a second added, the first removed, and two changed in one call.
    let rows: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        (
            "one group added",
            vec![param("AddSecurityGroupId.1", &first)],
            vec![default_group.clone(), first.clone()],
        ),
        (
            "a second group added",
            vec![param("AddSecurityGroupId.1", &second)],
            vec![default_group.clone(), first.clone(), second.clone()],
        ),
        (
            "one group removed",
            vec![param("RemoveSecurityGroupId.1", &first)],
            vec![default_group.clone(), second.clone()],
        ),
        (
            "one added and one removed in the same call",
            vec![
                param("AddSecurityGroupId.1", &first),
                param("RemoveSecurityGroupId.1", &default_group),
            ],
            vec![second.clone(), first.clone()],
        ),
    ];
    for (case, params, expected) in rows {
        modify(case, &router, &endpoint_id, &params).await;
        let described = describe_endpoint(case, &router, &endpoint_id).await;
        assert_eq!(
            set_members(case, &described, "groupSet", "groupId"),
            expected,
            "case {case:?}: expected those security groups, got {described}"
        );
        // Every endpoint network interface carries the endpoint's groups: "the security groups
        // to associate with the endpoint network interfaces" (`AddSecurityGroupIds`).
        for network_interface_id in set_items(case, &described, "networkInterfaceIdSet") {
            let carried: Vec<String> = engine
                .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
                .lock()
                .network_interfaces
                .get(&network_interface_id)
                .and_then(|interface| interface.groups.clone())
                .unwrap_or_default()
                .iter()
                .filter_map(|group| group.group_id.clone())
                .collect();
            assert_eq!(
                carried, expected,
                "case {case:?}: expected the interface {network_interface_id} to carry the \
                 endpoint's groups, got {carried:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The policy attached to the endpoint
// ---------------------------------------------------------------------------

/// The policy a request names replaces the one the endpoint carries, and `ResetPolicy` puts
/// back "the default policy [which] allows full access to the service" (`ResetPolicy`).
#[tokio::test]
async fn the_policy_is_replaced_by_the_one_named_and_reset_to_the_default() {
    let (router, _engine) = emulator();
    let built = network("the policy table", &router, &["us-east-1a"]).await;
    let endpoint_id = create_gateway_endpoint("the policy table", &router, &built.vpc_id).await;
    let default_policy = {
        let described = describe_endpoint("the policy table", &router, &endpoint_id).await;
        super::ec2_query::unescaped(&member("the policy table", &described, "policyDocument"))
    };

    // Rows: two different documents the request names, then a reset back to the default.
    let chosen = r#"{"Statement":[{"Action":"s3:GetObject","Effect":"Allow","Principal":"*","Resource":"*"}]}"#;
    let narrower = r#"{"Statement":[{"Action":"s3:ListBucket","Effect":"Deny","Principal":"*","Resource":"*"}]}"#;
    let rows: Vec<(&str, Vec<Param>, String)> = vec![
        (
            "a policy the request names",
            vec![param("PolicyDocument", chosen)],
            chosen.to_string(),
        ),
        (
            "another policy over the first",
            vec![param("PolicyDocument", narrower)],
            narrower.to_string(),
        ),
        (
            "a reset back to the default policy",
            vec![param("ResetPolicy", "true")],
            default_policy.clone(),
        ),
    ];
    for (case, params, expected) in rows {
        modify(case, &router, &endpoint_id, &params).await;
        let described = describe_endpoint(case, &router, &endpoint_id).await;
        assert_eq!(
            super::ec2_query::unescaped(&member(case, &described, "policyDocument")),
            expected,
            "case {case:?}: expected that policy document, got {described}"
        );
        // A gateway endpoint stands up no endpoint network interface, so a policy change owes
        // it no move at all: it stays in service throughout.
        assert_eq!(
            element(&described, "state").as_deref(),
            Some(ec2::state::VPC_ENDPOINT_STATE_AVAILABLE),
            "case {case:?}: expected the endpoint to stay available, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// The route tables a gateway endpoint is reached from
// ---------------------------------------------------------------------------

/// Associating a route table adds the endpoint's route to it and disassociating one takes that
/// route away — "we automatically add a route to the route table with a destination of the
/// prefix list for the service and a target of the gateway endpoint" (Amazon Web Services
/// PrivateLink User Guide, "Gateway endpoints").
#[tokio::test]
async fn a_gateway_endpoints_route_tables_and_their_routes_follow_the_request() {
    let (router, _engine) = emulator();
    let built = network("the route table table", &router, &["us-east-1a"]).await;
    let first = create_route_table("the route table table", &router, &built.vpc_id).await;
    let second = create_route_table("the route table table", &router, &built.vpc_id).await;
    let endpoint_id =
        create_gateway_endpoint("the route table table", &router, &built.vpc_id).await;
    // The destination of the endpoint's route: the prefix list the Region publishes for the
    // service the endpoint is to, read from the one catalogue the emulator reports from.
    let prefix_list_id = ec2::state::service_prefix_lists_in_region(REGION)
        .into_iter()
        .find(|published| published.prefix_list_name == GATEWAY_SERVICE)
        .map(|published| published.prefix_list_id)
        .unwrap_or_else(|| panic!("expected {REGION} to publish a prefix list for the service"));

    /// Whether the route table `route_table_id` holds a route to `endpoint_id`.
    async fn routes_to(
        case: &str,
        router: &MultiServiceRouter,
        route_table_id: &str,
        endpoint_id: &str,
        prefix_list_id: &str,
    ) -> bool {
        let body = ok(
            case,
            router,
            "DescribeRouteTables",
            &indexed("RouteTableId", &[route_table_id]),
        )
        .await;
        elements(&body, "gatewayId").iter().any(|held| held == endpoint_id)
            && elements(&body, "destinationPrefixListId")
                .iter()
                .any(|held| held == prefix_list_id)
    }

    // Rows: one table associated, a second associated, the first disassociated, and the last
    // one disassociated too.
    let rows: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        (
            "one route table associated",
            vec![param("AddRouteTableId.1", &first)],
            vec![first.clone()],
        ),
        (
            "a second route table associated",
            vec![param("AddRouteTableId.1", &second)],
            vec![first.clone(), second.clone()],
        ),
        (
            "the first route table disassociated",
            vec![param("RemoveRouteTableId.1", &first)],
            vec![second.clone()],
        ),
        (
            "the last route table disassociated",
            vec![param("RemoveRouteTableId.1", &second)],
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        modify(case, &router, &endpoint_id, &params).await;
        let described = describe_endpoint(case, &router, &endpoint_id).await;
        assert_eq!(
            set_items(case, &described, "routeTableIdSet"),
            expected,
            "case {case:?}: expected those route tables, got {described}"
        );
        for route_table_id in [first.as_str(), second.as_str()] {
            let holds = routes_to(case, &router, route_table_id, &endpoint_id, &prefix_list_id)
                .await;
            let associated = expected.iter().any(|held| held == route_table_id);
            assert_eq!(
                holds, associated,
                "case {case:?}: expected the table {route_table_id} to hold the endpoint's \
                 route only while associated (associated {associated}, holds {holds})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The private hosted zone associated with the VPC
// ---------------------------------------------------------------------------

/// `PrivateDnsEnabled` associates "the default public DNS name for the service for the Region"
/// (`CreateVpcEndpointRequest$PrivateDnsEnabled`) with the VPC, and turning it off takes that
/// name away again.
#[tokio::test]
async fn private_dns_is_enabled_and_disabled_with_the_name_it_resolves() {
    let (router, _engine) = emulator();
    let built = network("the private DNS table", &router, &["us-east-1a"]).await;
    enable_vpc_dns("the private DNS table", &router, &built.vpc_id).await;
    let endpoint_id = create_interface_endpoint(
        "the private DNS table",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;
    // The public name private DNS resolves inside the VPC, derived from the service the
    // endpoint is to rather than written out: `logs.us-east-1.amazonaws.com`.
    let public_name = format!(
        "{service}.{REGION}.{suffix}",
        service = INTERFACE_SERVICE
            .rsplit_once('.')
            .map(|(_, service)| service)
            .unwrap_or_default(),
        suffix = ec2::state::SERVICE_DNS_SUFFIX,
    );

    // Rows: on, on again (idempotent), off, and off again.
    for (case, asked, enabled) in [
        ("private DNS turned on", "true", true),
        ("private DNS asked for again", "true", true),
        ("private DNS turned off", "false", false),
        ("private DNS off again", "false", false),
    ] {
        modify(
            case,
            &router,
            &endpoint_id,
            &[param("PrivateDnsEnabled", asked)],
        )
        .await;
        let described = describe_endpoint(case, &router, &endpoint_id).await;
        assert_eq!(
            element(&described, "privateDnsEnabled").as_deref(),
            Some(if enabled { "true" } else { "false" }),
            "case {case:?}: expected privateDnsEnabled {enabled}, got {described}"
        );
        let dns_names = set_members(case, &described, "dnsEntrySet", "dnsName");
        assert_eq!(
            dns_names.contains(&public_name),
            enabled,
            "case {case:?}: expected the service's public name present only while private \
             DNS is on, got {dns_names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a modify refuses
// ---------------------------------------------------------------------------

/// The one-subnet-per-zone refusal names the subnet the REQUEST asked for and states the rule
/// it breaks, whichever operation places the subnet — "you can specify only one subnet per
/// Availability Zone" (Amazon Web Services PrivateLink User Guide, "Interface endpoints").
#[tokio::test]
async fn the_duplicate_zone_refusal_names_the_subnet_the_request_asked_for() {
    let (router, _engine) = emulator();
    let built = network(
        "the duplicate zone message",
        &router,
        &["us-east-1a", "us-east-1a", "us-east-1b"],
    )
    .await;
    let endpoint_id = create_interface_endpoint(
        "the duplicate zone message",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;
    /// The sentence EC2 answers this refusal with, built from the subnet the request named.
    fn expected(subnet_id: &str) -> String {
        format!(
            "Found another VPC endpoint subnet in the availability zone of {subnet_id}. VPC \
             endpoint subnets should be in different availability zones supported by the VPC \
             endpoint service."
        )
    }

    // Rows: the modify that adds a subnet into the zone the endpoint already fills, and a
    // create that names two subnets of one zone at once — the same rule, so the same sentence,
    // each naming the subnet ITS request asked for.
    let rows: Vec<(&str, &str, Vec<Param>, String)> = vec![
        (
            "a modify adding a subnet in an occupied zone",
            "ModifyVpcEndpoint",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[1]),
            ],
            expected(&built.subnets[1]),
        ),
        (
            "a modify whose second added subnet shares a zone with its first",
            "ModifyVpcEndpoint",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[2]),
                param("AddSubnetId.2", &built.subnets[1]),
            ],
            expected(&built.subnets[1]),
        ),
        (
            "a create naming two subnets of one zone",
            "CreateVpcEndpoint",
            vec![
                param("VpcId", &built.vpc_id),
                param("ServiceName", INTERFACE_SERVICE),
                param("VpcEndpointType", "Interface"),
                param("SubnetId.1", &built.subnets[0]),
                param("SubnetId.2", &built.subnets[1]),
            ],
            expected(&built.subnets[1]),
        ),
    ];
    for (case, action, params, message) in rows {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DUPLICATE_SUBNETS_IN_SAME_ZONE,
            "case {case:?}: expected DuplicateSubnetsInSameZone, got {body}"
        );
        assert_eq!(
            super::ec2_query::error_message(&body),
            message,
            "case {case:?}: expected that refusal sentence, got {body}"
        );
    }
}

/// Every refusal this operation makes, each with the EC2 code and HTTP status it answers, and
/// the endpoint left exactly as it was for all of them.
///
/// The rows differ along what makes the request impossible to serve: the resource it names, the
/// form of the value it carries, the member it combines with the endpoint's type, and whether
/// the change contradicts the state the endpoint is already in. The request shapes this slice
/// holds no state for are rows too — a 501 says "nobody has written this yet", which is a
/// different claim from a refusal.
#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_with_the_code_for_its_cause() {
    let (router, _engine) = emulator();
    let built = network(
        "the refusal table",
        &router,
        &["us-east-1a", "us-east-1a", "us-east-1b"],
    )
    .await;
    let other_vpc = create_vpc("another VPC", &router, "10.90.0.0/16").await;
    let other_subnet =
        create_subnet("another VPC", &router, &other_vpc, "10.90.1.0/24", "us-east-1c").await;
    let other_route_table = create_route_table("another VPC", &router, &other_vpc).await;
    let route_table = create_route_table("the refusal table", &router, &built.vpc_id).await;
    let group = create_security_group("the refusal table", &router, &built.vpc_id, "refused").await;
    let endpoint_id = create_interface_endpoint(
        "the refusal table",
        &router,
        &built.vpc_id,
        &[built.subnets[0].as_str()],
    )
    .await;
    let gateway_id = create_gateway_endpoint("the refusal table", &router, &built.vpc_id).await;
    // An interface endpoint served through no subnet yet, so a call that adds two subnets of
    // ONE zone is judged on those two alone rather than on a zone the endpoint already fills.
    let unplaced_id =
        create_interface_endpoint("the refusal table", &router, &built.vpc_id, &[]).await;
    let before = describe_endpoint("the refusal table", &router, &endpoint_id).await;

    /// A VPC endpoint id of the right shape that no endpoint of the account has.
    const ABSENT_ENDPOINT: &str = "vpce-0000000000000beef";
    /// A subnet id of the right shape that no subnet of the account has.
    const ABSENT_SUBNET: &str = "subnet-0000000000000beef";
    /// A security group id of the right shape that no group of the account has.
    const ABSENT_GROUP: &str = "sg-0000000000000beef";
    /// A route table id of the right shape that no table of the account has.
    const ABSENT_ROUTE_TABLE: &str = "rtb-0000000000000beef";

    // A policy one character past the published VPC endpoint policy quota, and valid JSON
    // otherwise — so its size is the only thing wrong with it.
    let oversize_policy = {
        let (opening, closing) = (r#"{"Statement":""#, r#""}"#);
        let padding = "x".repeat(
            ec2::state::MAX_VPC_ENDPOINT_POLICY_CHARS + 1 - opening.len() - closing.len(),
        );
        format!("{opening}{padding}{closing}")
    };

    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a value that is not a VPC endpoint id at all",
            vec![
                param("VpcEndpointId", "not-an-endpoint"),
                param("ResetPolicy", "true"),
            ],
            400,
            ec2::state::INVALID_VPC_ENDPOINT_ID_MALFORMED,
        ),
        (
            "an endpoint id no endpoint of the account has",
            vec![
                param("VpcEndpointId", ABSENT_ENDPOINT),
                param("ResetPolicy", "true"),
            ],
            400,
            ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND,
        ),
        (
            "a second subnet in a zone the endpoint is already served in",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[1]),
            ],
            400,
            ec2::state::DUPLICATE_SUBNETS_IN_SAME_ZONE,
        ),
        (
            "two added subnets of one zone in the same call",
            vec![
                param("VpcEndpointId", &unplaced_id),
                param("AddSubnetId.1", &built.subnets[0]),
                param("AddSubnetId.2", &built.subnets[1]),
            ],
            400,
            ec2::state::DUPLICATE_SUBNETS_IN_SAME_ZONE,
        ),
        (
            "a subnet no subnet of the account has",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", ABSENT_SUBNET),
            ],
            400,
            ec2::state::INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a subnet of another VPC",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &other_subnet),
            ],
            400,
            ec2::state::INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a subnet the endpoint does not serve removed",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("RemoveSubnetId.1", &built.subnets[2]),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one subnet both added and removed",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[2]),
                param("RemoveSubnetId.1", &built.subnets[2]),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a security group no group of the account has",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSecurityGroupId.1", ABSENT_GROUP),
            ],
            400,
            ec2::state::INVALID_GROUP_NOT_FOUND,
        ),
        (
            "a value that is not a security group id at all",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSecurityGroupId.1", "not-a-group"),
            ],
            400,
            ec2::state::INVALID_GROUP_ID_MALFORMED,
        ),
        (
            "a security group the endpoint does not carry removed",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("RemoveSecurityGroupId.1", &group),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "security groups on a gateway endpoint",
            vec![
                param("VpcEndpointId", &gateway_id),
                param("AddSecurityGroupId.1", &group),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "route tables on an interface endpoint",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddRouteTableId.1", &route_table),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "subnets on a gateway endpoint",
            vec![
                param("VpcEndpointId", &gateway_id),
                param("AddSubnetId.1", &built.subnets[2]),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a route table no table of the account has",
            vec![
                param("VpcEndpointId", &gateway_id),
                param("AddRouteTableId.1", ABSENT_ROUTE_TABLE),
            ],
            400,
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table of another VPC",
            vec![
                param("VpcEndpointId", &gateway_id),
                param("AddRouteTableId.1", &other_route_table),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a route table the endpoint is not associated with disassociated",
            vec![
                param("VpcEndpointId", &gateway_id),
                param("RemoveRouteTableId.1", &route_table),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a policy document that is not JSON",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("PolicyDocument", "not json at all"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a policy document past the published quota",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("PolicyDocument", oversize_policy.as_str()),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a reset and a policy document in the same call",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("ResetPolicy", "true"),
                param("PolicyDocument", r#"{"Statement":[]}"#),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a subnet configuration for a subnet the endpoint is not in",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("SubnetConfiguration.1.SubnetId", &built.subnets[2]),
                param("SubnetConfiguration.1.Ipv4", "10.60.3.10"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "an address outside the subnet's assignable range",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[2]),
                param("SubnetConfiguration.1.SubnetId", &built.subnets[2]),
                param("SubnetConfiguration.1.Ipv4", "10.60.9.10"),
            ],
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a modify that would have succeeded",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("AddSubnetId.1", &built.subnets[2]),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "an endpoint reached over IPv6",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("IpAddressType", "dualstack"),
            ],
            501,
            "NotImplemented",
        ),
        (
            "DNS records for addresses this slice cannot assign",
            vec![
                param("VpcEndpointId", &endpoint_id),
                param("DnsOptions.DnsRecordIpType", "dualstack"),
            ],
            501,
            "NotImplemented",
        ),
    ];

    for (case, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, "ModifyVpcEndpoint", &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code_expected,
            "case {case:?}: expected the code {code_expected}, got {body}"
        );
        let after = describe_endpoint(case, &router, &endpoint_id).await;
        for element_name in [
            "state",
            "policyDocument",
            "privateDnsEnabled",
            "vpcEndpointType",
        ] {
            assert_eq!(
                element(&after, element_name),
                element(&before, element_name),
                "case {case:?}: a refused modify must leave <{element_name}> alone: {after}"
            );
        }
        assert_eq!(
            set_items(case, &after, "subnetIdSet"),
            set_items(case, &before, "subnetIdSet"),
            "case {case:?}: a refused modify must leave the endpoint's subnets alone: {after}"
        );
        assert_eq!(
            set_items(case, &after, "networkInterfaceIdSet"),
            set_items(case, &before, "networkInterfaceIdSet"),
            "case {case:?}: a refused modify must leave the endpoint's interfaces alone: \
             {after}"
        );
    }
}
