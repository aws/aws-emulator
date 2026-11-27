// SPDX-License-Identifier: CC0-1.0

//! What EC2 answers when a request names a resource of ANOTHER kind than the one it acts on —
//! the route table a policy table entry targets, the interface a grant is over, the user a
//! terminate selects connections by, the group a security group rule references — driven over
//! the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test, one per group of tests below:
//!
//! 1. A create of a transit gateway policy table ENTRY resolves the route table its entry
//!    TARGETS before the policy table the entry would belong to, and reports an id the account
//!    holds no route table for with that resource's own not-found class and the service's
//!    "does not exist" sentence for the kind of id it could not resolve. Only a value that is
//!    no route table id at all is malformed: an id in this resource's form that EC2 would never
//!    have MINTED is looked up like any other.
//! 2. A refusal names the resource the way the OPERATION names it: the grant over a network
//!    interface calls it the interface, both when it cannot read the id and when the account
//!    holds no interface under it.
//! 3. A terminate that selects a user's connections and finds none says so about the
//!    CONNECTIONS — there is nothing to terminate — rather than about the identity.
//! 4. A rule's referenced security group is resolved inside the rule's own network, and a
//!    reference to a group of ANOTHER network is refused with the sentence EC2 answers whenever
//!    one request names two resources of different networks — however the request spelled the
//!    reference, by id or by name. Only a group no network of the account holds is reported as
//!    a group that does not exist.
//! 5. A rule's port members admit the value that stands for every port and every number up to
//!    the highest the protocol has, and any other value is refused naming the WIRE member it
//!    arrived in and the range that member admits, while a TCP or UDP pair that ends before it
//!    starts is refused naming the range itself.
//!
//! Every table row is a distinct input along what its rule generalises over: which form an
//! unresolvable id takes (the two lengths EC2 mints, a length it does not, another kind's
//! prefix, no identifier at all), which of two unresolvable parents a request names, how a
//! reference is spelled, and how a user comes to hold no active connection. The recorded cases
//! are ONE row each.

use std::time::SystemTime;

use ec2::ops::types::{ClientVpnEndpoint, INTERFACE_PERMISSION_TYPE_VALUES};
use ec2::state::{
    ClientVpnConnectionSpec, ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_GROUP_NOT_FOUND,
    INVALID_NETWORK_INTERFACE_ID_MALFORMED, INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
    INVALID_ROUTE_TABLE_ID_MALFORMED,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers a refused request with.
const REFUSED: u16 = 400;

/// The account a grant below is made to: twelve digits, as an account id is.
const GRANTEE: &str = "111122223333";

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for [`REGION`].
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
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

/// Assert the refusal `body` carries `code` and `message`, naming `case` and the body.
fn assert_refusal(case: &str, status: u16, body: &str, code: &str, message: &str) {
    assert_eq!(
        status, REFUSED,
        "case {case:?}: expected HTTP {REFUSED}, got {status} with body {body}"
    );
    assert_eq!(
        error_code(body),
        code,
        "case {case:?}: expected the code {code:?}, got body {body}"
    );
    assert_eq!(
        error_message(body),
        message,
        "case {case:?}: expected the message {message:?}, got body {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the route table a policy table entry targets
// ---------------------------------------------------------------------------

/// The code EC2 answers a `TargetRouteTableId` the account holds no route table for, spelled
/// out here rather than read from the service: a test that re-asserted the service's own
/// constant would pass however that constant were worded.
const TARGET_ROUTE_TABLE_NOT_FOUND: &str = "InvalidTransitGatewayRouteTableId.NotFound";

/// The code EC2 answers a policy table id it cannot read.
const POLICY_TABLE_MALFORMED: &str = "InvalidTransitGatewayPolicyTableId.Malformed";

/// The code EC2 answers a well-formed policy table id the account holds no table for.
const POLICY_TABLE_NOT_FOUND: &str = "InvalidTransitGatewayPolicyTableId.NotFound";

/// A transit gateway, one route table of it and one policy table of it, each minted by the
/// operation that mints one rather than by reaching into the store.
struct PolicyTable {
    router: MultiServiceRouter,
    policy_table_id: String,
    route_table_id: String,
}

/// The fixtures rule 1's tests run against.
async fn policy_table() -> PolicyTable {
    let case = "the policy table fixtures";
    let (router, _engine) = emulator();
    let gateway = ok(case, &router, "CreateTransitGateway", &[]).await;
    let transit_gateway_id = member(case, &gateway, "transitGatewayId");
    let route_table = ok(
        case,
        &router,
        "CreateTransitGatewayRouteTable",
        &[param("TransitGatewayId", &transit_gateway_id)],
    )
    .await;
    let route_table_id = member(case, &route_table, "transitGatewayRouteTableId");
    let policy_table = ok(
        case,
        &router,
        "CreateTransitGatewayPolicyTable",
        &[param("TransitGatewayId", &transit_gateway_id)],
    )
    .await;
    let policy_table_id = member(case, &policy_table, "transitGatewayPolicyTableId");
    PolicyTable {
        router,
        policy_table_id,
        route_table_id,
    }
}

/// The members one entry create carries.
fn entry(policy_table_id: &str, rule_number: &str, target_route_table_id: &str) -> Vec<Param> {
    vec![
        param("TransitGatewayPolicyTableId", policy_table_id),
        param("PolicyRuleNumber", rule_number),
        param("TargetRouteTableId", target_route_table_id),
    ]
}

/// The sentence EC2 answers a `TargetRouteTableId` it holds no route table for: fixed text
/// around the resource type's own word and the id the REQUEST sent.
fn target_not_found_message(target_route_table_id: &str) -> String {
    format!("The transit-gateway-route-table ID '{target_route_table_id}' does not exist")
}

/// RULE: a create of a policy table entry resolves the route table its entry TARGETS first —
/// before the policy table the entry would belong to, and so before that id's own form is read
/// — and words an id it holds no route table for in the route table's own not-found class. A
/// value that is no route table id at all is malformed; one merely in a length EC2 does not
/// mint is looked up.
#[tokio::test]
async fn an_entry_create_resolves_the_route_table_it_targets_first() {
    let fixtures = policy_table().await;
    let held_table = fixtures.policy_table_id.clone();
    let held_route_table = fixtures.route_table_id.clone();

    // (case, the policy table the request names, the route table it targets, code, message)
    let rows: Vec<(&str, String, &str, &str, String)> = vec![
        (
            "a target route table nothing holds, in the long form EC2 mints",
            held_table.clone(),
            "tgw-rtb-0123456789abcdef0",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0123456789abcdef0"),
        ),
        (
            "a target route table nothing holds, in the short form EC2 reads",
            held_table.clone(),
            "tgw-rtb-0badf00d",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0badf00d"),
        ),
        (
            "a target route table nothing holds, of a digit count EC2 never mints",
            held_table.clone(),
            "tgw-rtb-0123456789abcde",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0123456789abcde"),
        ),
        (
            "an unresolvable target beside a policy table id of no kind at all",
            "not-a-policy-table".to_string(),
            "tgw-rtb-00000000000000aaa",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-00000000000000aaa"),
        ),
        (
            "an unresolvable target beside a policy table id of a digit count EC2 never mints",
            "tgw-ptb-0123456789abcde".to_string(),
            "tgw-rtb-0123456789abcde",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0123456789abcde"),
        ),
        (
            "an unresolvable target beside a well-formed policy table id nothing holds",
            "tgw-ptb-0123456789abcdef0".to_string(),
            "tgw-rtb-0badf00d",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0badf00d"),
        ),
        (
            "a target that is no identifier at all",
            held_table.clone(),
            "not-a-route-table",
            INVALID_ROUTE_TABLE_ID_MALFORMED,
            "Invalid Transit Gateway Route Table id not-a-route-table.".to_string(),
        ),
        (
            "a target carrying the announcement kind that extends this prefix",
            held_table.clone(),
            "tgw-rtb-announce-0123456789abcdef0",
            INVALID_ROUTE_TABLE_ID_MALFORMED,
            "Invalid Transit Gateway Route Table id tgw-rtb-announce-0123456789abcdef0."
                .to_string(),
        ),
        (
            "a target carrying another resource kind's prefix",
            held_table.clone(),
            "vpc-0123456789abcdef0",
            INVALID_ROUTE_TABLE_ID_MALFORMED,
            "Invalid Transit Gateway Route Table id vpc-0123456789abcdef0.".to_string(),
        ),
        (
            "the policy table's own defect, once the target route table resolves",
            "not-a-policy-table".to_string(),
            held_route_table.as_str(),
            POLICY_TABLE_MALFORMED,
            "Invalid id: \"not-a-policy-table\"".to_string(),
        ),
        (
            "a well-formed policy table id nothing holds, once the target resolves",
            "tgw-ptb-0123456789abcdef0".to_string(),
            held_route_table.as_str(),
            POLICY_TABLE_NOT_FOUND,
            "The transit-gateway-policy-table ID 'tgw-ptb-0123456789abcdef0' does not exist"
                .to_string(),
        ),
    ];

    for (case, policy_table_id, target, code, message) in rows {
        let (status, body) = invoke(
            &fixtures.router,
            "CreateTransitGatewayPolicyTableEntry",
            &entry(&policy_table_id, "10", target),
        )
        .await;
        assert_refusal(case, status, &body, code, &message);
    }

    // Nothing above was written, and the ids the account really holds still create an entry
    // that the report of the table reads back: the resolution order above refuses nothing it
    // can resolve.
    let created = ok(
        "the entry the held ids create",
        &fixtures.router,
        "CreateTransitGatewayPolicyTableEntry",
        &entry(&held_table, "10", &held_route_table),
    )
    .await;
    assert_eq!(
        element(&created, "policyRuleNumber").as_deref(),
        Some("10"),
        "the created entry must carry the rule number the request named, got {created}"
    );
    let reported = ok(
        "the report of the table",
        &fixtures.router,
        "GetTransitGatewayPolicyTableEntries",
        &[param("TransitGatewayPolicyTableId", &held_table)],
    )
    .await;
    assert_eq!(
        elements(&reported, "policyRuleNumber"),
        vec!["10".to_string()],
        "the table must hold exactly the one entry that was served, got {reported}"
    );
    assert_eq!(
        elements(&reported, "targetRouteTableId"),
        vec![held_route_table.clone()],
        "the stored entry must target the route table the create named, got {reported}"
    );
}

/// RULE: the modify of an entry resolves the route table the entry WILL target through the same
/// one reading, so the two operations cannot word one dangling target two ways.
#[tokio::test]
async fn an_entry_modify_words_a_dangling_target_the_same_way() {
    let fixtures = policy_table().await;
    let created = ok(
        "the entry to modify",
        &fixtures.router,
        "CreateTransitGatewayPolicyTableEntry",
        &entry(&fixtures.policy_table_id, "5", &fixtures.route_table_id),
    )
    .await;
    assert_eq!(
        element(&created, "policyRuleNumber").as_deref(),
        Some("5"),
        "the entry must be created: {created}"
    );

    // (case, the route table the modify would retarget the entry to, code, message)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a target nothing holds, in the long form EC2 mints",
            "tgw-rtb-0123456789abcdef0",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0123456789abcdef0"),
        ),
        (
            "a target nothing holds, of a digit count EC2 never mints",
            "tgw-rtb-0123456789abcde",
            TARGET_ROUTE_TABLE_NOT_FOUND,
            target_not_found_message("tgw-rtb-0123456789abcde"),
        ),
        (
            "a target that is no identifier at all",
            "not-a-route-table",
            INVALID_ROUTE_TABLE_ID_MALFORMED,
            "Invalid Transit Gateway Route Table id not-a-route-table.".to_string(),
        ),
    ];
    for (case, target, code, message) in rows {
        let (status, body) = invoke(
            &fixtures.router,
            "ModifyTransitGatewayPolicyTableEntry",
            &[
                param("TransitGatewayPolicyTableId", &fixtures.policy_table_id),
                param("PolicyRuleNumber", "5"),
                param("TargetRouteTableId", target),
            ],
        )
        .await;
        assert_refusal(case, status, &body, code, &message);
    }

    // The entry still targets the route table it was created with: every refusal above wrote
    // nothing.
    let reported = ok(
        "the report after the refused modifies",
        &fixtures.router,
        "GetTransitGatewayPolicyTableEntries",
        &[param(
            "TransitGatewayPolicyTableId",
            &fixtures.policy_table_id,
        )],
    )
    .await;
    assert_eq!(
        elements(&reported, "targetRouteTableId"),
        vec![fixtures.route_table_id.clone()],
        "a refused modify must leave the entry targeting its own route table, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the interface a grant is over
// ---------------------------------------------------------------------------

/// RULE: the grant over a network interface names that resource the INTERFACE in both of its
/// refusals — the value it cannot read as an id, and the id the account holds no interface
/// under — so one operation words one resource one way.
#[tokio::test]
async fn a_grant_names_the_interface_the_same_way_in_both_refusals() {
    let (router, engine) = emulator();

    // (case, the interface the grant names, code, message)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "an interface id nothing holds, in the long form EC2 mints",
            "eni-0123456789abcdef0",
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
            "The interface ID 'eni-0123456789abcdef0' does not exist".to_string(),
        ),
        (
            "an interface id nothing holds, in the short form EC2 reads",
            "eni-0badf00d",
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
            "The interface ID 'eni-0badf00d' does not exist".to_string(),
        ),
        (
            "the all-zero interface id, which this resource reads as an id",
            "eni-00000000000000000",
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
            "The interface ID 'eni-00000000000000000' does not exist".to_string(),
        ),
        (
            "a value that is no identifier at all",
            "not-an-interface",
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
            "The interface ID 'not-an-interface' is malformed".to_string(),
        ),
        (
            "an id carrying another resource kind's prefix",
            "vpc-0123456789abcdef0",
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
            "The interface ID 'vpc-0123456789abcdef0' is malformed".to_string(),
        ),
        (
            "an interface id of a digit count EC2 never mints",
            "eni-0123456789abcde",
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
            "The interface ID 'eni-0123456789abcde' is malformed".to_string(),
        ),
    ];

    for (case, network_interface_id, code, message) in rows {
        let (status, body) = invoke(
            &router,
            "CreateNetworkInterfacePermission",
            &[
                param("NetworkInterfaceId", network_interface_id),
                param("AwsAccountId", GRANTEE),
                param("Permission", INTERFACE_PERMISSION_TYPE_VALUES[0]),
            ],
        )
        .await;
        assert_refusal(case, status, &body, code, &message);
    }

    // An interface the account really holds is granted, and the describe reports that grant:
    // the refusals above are about the ids, not about the operation.
    let subnet_id = {
        let state = engine.state::<Ec2State>();
        let store = state.lock();
        let vpc_id = store
            .default_vpc_id(REGION)
            .expect("a fresh account has its default VPC");
        store
            .subnets
            .iter()
            .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
            .map(|(subnet_id, _)| subnet_id.clone())
            .expect("the default VPC comes with its default subnets")
    };
    let interface = ok(
        "the interface the grant is over",
        &router,
        "CreateNetworkInterface",
        &[param("SubnetId", &subnet_id)],
    )
    .await;
    let interface_id = member("the interface the grant is over", &interface, "networkInterfaceId");
    let granted = ok(
        "the grant over a held interface",
        &router,
        "CreateNetworkInterfacePermission",
        &[
            param("NetworkInterfaceId", &interface_id),
            param("AwsAccountId", GRANTEE),
            param("Permission", INTERFACE_PERMISSION_TYPE_VALUES[0]),
        ],
    )
    .await;
    let permission_id = member(
        "the grant over a held interface",
        &granted,
        "networkInterfacePermissionId",
    );
    let described = ok(
        "the describe of the grant",
        &router,
        "DescribeNetworkInterfacePermissions",
        &[param("NetworkInterfacePermissionId.1", &permission_id)],
    )
    .await;
    assert_eq!(
        elements(&described, "networkInterfaceId"),
        vec![interface_id.clone()],
        "the grant must round-trip through the describe, got {described}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the user a terminate selects connections by
// ---------------------------------------------------------------------------

/// The code EC2 answers a `Username` the endpoint holds no active connection for.
const CONNECTION_USER_NOT_FOUND: &str = "InvalidClientVpnConnection.UserNotFound";

/// The sentence it carries: fixed text about the CONNECTIONS, then the name the REQUEST sent.
fn no_connections_message(username: &str) -> String {
    format!("No connections exists for the given username {username}")
}

/// One Client VPN endpoint, written the way `CreateClientVpnEndpoint` writes one.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            transport_protocol: Some("udp".to_string()),
            vpn_port: Some(443),
            ..Default::default()
        })
        .unwrap_or_else(|| panic!("the store must take the Client VPN endpoint {description:?}"))
}

/// One client connection of `endpoint_id`, as that endpoint's own data plane records one.
fn seed_connection(
    engine: &engine::InMemoryEngine,
    endpoint_id: &str,
    username: &str,
    client_ip: &str,
) -> String {
    let connection = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_connection(ClientVpnConnectionSpec {
            client_vpn_endpoint_id: endpoint_id.to_string(),
            username: Some(username.to_string()),
            common_name: Some(username.to_string()),
            client_ip: Some(client_ip.to_string()),
            established_at: Some(SystemTime::now()),
        })
        .unwrap_or_else(|| panic!("the store must take a connection of {endpoint_id}"));
    connection
        .connection_id
        .unwrap_or_else(|| panic!("a stored connection of {endpoint_id} must carry an id"))
}

/// RULE: a terminate that selects by `Username` and finds no active connection says that the
/// endpoint holds no CONNECTIONS for that name — there is nothing to terminate — rather than
/// that the user does not exist, whichever way the name comes to select nothing.
#[tokio::test]
async fn a_terminate_by_user_reports_the_connections_it_found_none_of() {
    let (router, engine) = emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the account");
    let connection = seed_connection(&engine, &endpoint, "stays.connected", "10.100.0.11");
    seed_connection(&engine, &endpoint, "disconnects", "10.100.0.12");
    seed_connection(&engine, &other_endpoint, "elsewhere", "10.100.0.13");

    // The user whose only connection this call ends, so that the SAME name selects nothing
    // afterwards.
    let ended = ok(
        "the first terminate",
        &router,
        "TerminateClientVpnConnections",
        &[
            param("ClientVpnEndpointId", &endpoint),
            param("Username", "disconnects"),
        ],
    )
    .await;
    assert_eq!(
        element(&ended, "username").as_deref(),
        Some("disconnects"),
        "the terminate must report the user whose connections it ended: {ended}"
    );

    // (case, the name the terminate selects by)
    let rows: Vec<(&str, &str)> = vec![
        ("a name the endpoint has never seen", "never.connected"),
        (
            "a name whose only connection this endpoint already terminated",
            "disconnects",
        ),
        (
            "a name connected to ANOTHER endpoint of the account",
            "elsewhere",
        ),
        (
            "a name that is the id of a connection, which is no user",
            connection.as_str(),
        ),
        (
            "a name differing only in case from a connected one",
            "STAYS.CONNECTED",
        ),
    ];
    for (case, username) in rows {
        let (status, body) = invoke(
            &router,
            "TerminateClientVpnConnections",
            &[
                param("ClientVpnEndpointId", &endpoint),
                param("Username", username),
            ],
        )
        .await;
        assert_refusal(
            case,
            status,
            &body,
            CONNECTION_USER_NOT_FOUND,
            &no_connections_message(username),
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the port numbers a rule's range may name
// ---------------------------------------------------------------------------

/// The highest TCP or UDP port, and the highest ICMP type — the two ceilings a rule's port
/// members are held to, spelled out here rather than read from the service.
const HIGHEST_PORT: i32 = 65_535;
const HIGHEST_ICMP_TYPE: i32 = 255;

/// The value that stands for every port, type and code.
const ALL_PORTS: i32 = -1;

/// The sentence EC2 answers a TCP or UDP pair that ends before it starts: fixed text around
/// the two ends as the request sent them, naming neither member.
fn invalid_port_range_message(from_port: i32, to_port: i32) -> String {
    format!("Invalid TCP/UDP port range({from_port}:{to_port})")
}

/// The sentence EC2 answers a port, ICMP type or ICMP code outside what that member holds: the
/// WIRE member the value arrived in, with no entry number in front of it, and the range that
/// member admits.
fn port_out_of_range_message(value: i32, member: &str, highest: i32) -> String {
    format!("Invalid value '{value}' for {member}. Value must be between {ALL_PORTS} and {highest}.")
}

/// RULE: a rule's `FromPort` and `ToPort` admit the value that stands for all of them and every
/// number up to the highest the protocol has — ports for TCP and UDP, an ICMP type and code for
/// ICMP — and a value outside that range is refused `InvalidParameterValue` naming the WIRE
/// member it arrived in, while a TCP or UDP pair that ends before it starts is refused naming
/// the range itself. The published bounds themselves, and the value that stands for all of
/// them, are served.
#[tokio::test]
async fn a_port_outside_the_range_its_protocol_admits_is_refused_naming_that_member() {
    let case = "the group whose rules carry the ports";
    let (router, _engine) = emulator();
    let created = ok(
        case,
        &router,
        "CreateVpc",
        &[param("CidrBlock", "10.62.0.0/16")],
    )
    .await;
    let vpc_id = member(case, &created, "vpcId");
    let group = ok(
        case,
        &router,
        "CreateSecurityGroup",
        &[
            param("VpcId", &vpc_id),
            param("GroupName", "ports"),
            param("GroupDescription", "a group whose rules carry port ranges"),
        ],
    )
    .await;
    let group_id = member(case, &group, "groupId");

    // (case, protocol, fromPort, toPort, the refusal or `None` when the rule is stored). The
    // rows walk each published bound, one value past it, and the value that stands for all.
    let rows: Vec<(&str, &str, i32, i32, Option<String>)> = vec![
        (
            "the highest TCP port there is",
            "tcp",
            HIGHEST_PORT,
            HIGHEST_PORT,
            None,
        ),
        ("the lowest TCP port there is", "tcp", 0, 0, None),
        (
            "every TCP port, named by the value that stands for all of them",
            "tcp",
            ALL_PORTS,
            ALL_PORTS,
            None,
        ),
        (
            "one TCP port past the highest there is, as fromPort",
            "tcp",
            HIGHEST_PORT + 1,
            HIGHEST_PORT + 1,
            Some(port_out_of_range_message(
                HIGHEST_PORT + 1,
                "fromPort",
                HIGHEST_PORT,
            )),
        ),
        (
            "one TCP port past the highest there is, as toPort alone",
            "tcp",
            22,
            HIGHEST_PORT + 1,
            Some(port_out_of_range_message(
                HIGHEST_PORT + 1,
                "toPort",
                HIGHEST_PORT,
            )),
        ),
        (
            "a TCP port one below the value that stands for all of them",
            "tcp",
            ALL_PORTS - 1,
            22,
            Some(port_out_of_range_message(
                ALL_PORTS - 1,
                "fromPort",
                HIGHEST_PORT,
            )),
        ),
        (
            "a TCP range that ends before it starts",
            "tcp",
            23,
            22,
            Some(invalid_port_range_message(23, 22)),
        ),
        (
            "the highest ICMP type there is",
            "icmp",
            HIGHEST_ICMP_TYPE,
            HIGHEST_ICMP_TYPE,
            None,
        ),
        (
            "one ICMP type past the highest there is",
            "icmp",
            HIGHEST_ICMP_TYPE + 1,
            ALL_PORTS,
            Some(port_out_of_range_message(
                HIGHEST_ICMP_TYPE + 1,
                "fromPort",
                HIGHEST_ICMP_TYPE,
            )),
        ),
        (
            "an ICMP code past the highest there is",
            "icmp",
            8,
            HIGHEST_ICMP_TYPE + 1,
            Some(port_out_of_range_message(
                HIGHEST_ICMP_TYPE + 1,
                "toPort",
                HIGHEST_ICMP_TYPE,
            )),
        ),
    ];

    // One source range per row, so a stored rule is never refused as a duplicate of an
    // earlier row's rule.
    let mut host = 0;
    let mut served = 0;
    for (case, protocol, from_port, to_port, refusal) in rows {
        host += 1;
        let params = vec![
            param("GroupId", &group_id),
            param("IpPermissions.1.IpProtocol", protocol),
            param("IpPermissions.1.FromPort", &from_port.to_string()),
            param("IpPermissions.1.ToPort", &to_port.to_string()),
            param(
                "IpPermissions.1.IpRanges.1.CidrIp",
                &format!("10.70.{host}.0/24"),
            ),
        ];
        let (status, body) = invoke(&router, "AuthorizeSecurityGroupIngress", &params).await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the rule to be stored, got {status} with body {body}"
                );
                served += 1;
            }
            Some(message) => {
                assert_refusal(case, status, &body, "InvalidParameterValue", &message);
            }
        }
    }

    // Exactly the served rows were stored: a refused port range writes no rule.
    let described = ok(
        "the rules the group holds",
        &router,
        "DescribeSecurityGroupRules",
        &[
            param("Filter.1.Name", "group-id"),
            param("Filter.1.Value.1", &group_id),
        ],
    )
    .await;
    let ingress = elements(&described, "isEgress")
        .into_iter()
        .filter(|is_egress| is_egress == "false")
        .count();
    assert_eq!(
        ingress, served,
        "expected the group to hold exactly the {served} rules that were served, got {described}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the group a rule references
// ---------------------------------------------------------------------------

/// The sentence EC2 answers a request naming two resources of different networks, spelled out
/// here rather than read from the service.
const DIFFERENT_NETWORKS: &str = "You have specified two resources that belong to different \
                                  networks.";

/// RULE: a rule's referenced security group is resolved inside the rule's OWN network, and a
/// reference to a group of another network is refused with EC2's different-networks sentence
/// however the request spelled it — by id or by name — because which network the referenced
/// group is in is a fact of that group, not of the spelling. Only a group no network of the
/// account holds is reported as a group that does not exist.
#[tokio::test]
async fn a_referenced_group_of_another_network_is_refused_in_those_words() {
    let case = "two networks";
    let (router, _engine) = emulator();

    let first = ok(case, &router, "CreateVpc", &[param("CidrBlock", "10.60.0.0/16")]).await;
    let first_vpc = member(case, &first, "vpcId");
    let second = ok(case, &router, "CreateVpc", &[param("CidrBlock", "10.61.0.0/16")]).await;
    let second_vpc = member(case, &second, "vpcId");

    // (the VPC, the name) of each group: one group to authorize, one source in its own
    // network, and one source in the other network under a name of its own.
    let mut groups = Vec::new();
    for (vpc_id, group_name) in [
        (&first_vpc, "subject"),
        (&first_vpc, "source-at-home"),
        (&second_vpc, "source-abroad"),
    ] {
        let created = ok(
            group_name,
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", vpc_id),
                param("GroupName", group_name),
                param("GroupDescription", "a group a rule may reference"),
            ],
        )
        .await;
        groups.push(member(group_name, &created, "groupId"));
    }
    let [subject, at_home, abroad] = [
        groups[0].clone(),
        groups[1].clone(),
        groups[2].clone(),
    ];

    // (case, the Groups.1 member the reference is spelled with, its value, the refusal or
    // `None` when the rule is stored)
    let rows: Vec<(&str, &str, String, Option<String>)> = vec![
        (
            "a group of the rule's own network, by id",
            "GroupId",
            at_home.clone(),
            None,
        ),
        (
            "a group of the rule's own network, by name",
            "GroupName",
            "source-at-home".to_string(),
            None,
        ),
        (
            "a group of another network, by id",
            "GroupId",
            abroad.clone(),
            Some(DIFFERENT_NETWORKS.to_string()),
        ),
        (
            "a group of another network, by name",
            "GroupName",
            "source-abroad".to_string(),
            Some(DIFFERENT_NETWORKS.to_string()),
        ),
        (
            "a group of another network, by name differing only in case",
            "GroupName",
            "SOURCE-ABROAD".to_string(),
            Some(DIFFERENT_NETWORKS.to_string()),
        ),
        (
            "a well-formed group id no network of the account holds",
            "GroupId",
            "sg-0123456789abcdef0".to_string(),
            Some("The security group 'sg-0123456789abcdef0' does not exist".to_string()),
        ),
        (
            "a group name no network of the account holds",
            "GroupName",
            "no-such-group".to_string(),
            Some("The security group 'no-such-group' does not exist".to_string()),
        ),
    ];

    // One port per row, so a stored rule is never refused for being a duplicate of an earlier
    // row's — which would be a different rule than this one.
    let mut port = 2200;
    let mut stored: Vec<String> = Vec::new();
    for (case, spelled_as, value, refusal) in rows {
        port += 1;
        let params = vec![
            param("GroupId", &subject),
            param("IpPermissions.1.IpProtocol", "tcp"),
            param("IpPermissions.1.FromPort", &port.to_string()),
            param("IpPermissions.1.ToPort", &port.to_string()),
            param(&format!("IpPermissions.1.Groups.1.{spelled_as}"), &value),
        ];
        let (status, body) = invoke(&router, "AuthorizeSecurityGroupIngress", &params).await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the rule to be stored, got {status} with body {body}"
                );
                stored.push(port.to_string());
            }
            Some(message) => {
                assert_refusal(case, status, &body, INVALID_GROUP_NOT_FOUND, &message);
            }
        }
    }

    // Each refusal stored nothing: the group holds exactly the rules the rows above were
    // served, each identified by the port only it used.
    let described = ok(
        case,
        &router,
        "DescribeSecurityGroupRules",
        &[
            param("Filter.1.Name", "group-id"),
            param("Filter.1.Value.1", &subject),
        ],
    )
    .await;
    let mut found: Vec<String> = elements(&described, "fromPort")
        .into_iter()
        .filter(|port| port != "-1")
        .collect();
    found.sort();
    stored.sort();
    assert_eq!(
        found, stored,
        "expected the group to hold exactly the rules that were served, got {described}"
    );
}
