// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteClientVpnRoute`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! The rule under test is that a DELETE TAKES THE ONE MANUALLY ADDED ROUTE NAMED BY THE
//! ENDPOINT, THE DESTINATION RANGE AND THE TARGET TRAFFIC LEAVES THROUGH OUT OF THAT
//! ENDPOINT'S ROUTE TABLE, answers it `deleting`, and leaves every other route of that table
//! — and of every other endpoint — where it is; a route the table does not hold under that
//! identity is refused rather than reported as deleted, and a route the association of a
//! target network added is refused because this action deletes only what a caller added.
//! Every table row below is a distinct input along what that generalises over: which
//! destination, how large a block, which target (a target network, the local network, or the
//! attachment of a Transit Gateway-based endpoint, which has no target of its own), which
//! endpoint, and — for the refusals — which part of the request names no deletable route.

use ec2::ops::types::{ClientVpnEndpoint, TransitGatewayConfigurationDescribeEndpointStructure};
use ec2::state::{
    ClientVpnTargetNetworkSpec as TargetNetworkSpec, ComAmazonawsEc2Amazonec2State as Ec2State,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};

/// The Region every call here is signed for. The endpoint and route stores are per account
/// rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action that adds a route.
const CREATE: &str = "CreateClientVpnRoute";

/// The ec2Query action under test.
const DELETE: &str = "DeleteClientVpnRoute";

/// The ec2Query action that reads a route table back.
const DESCRIBE: &str = "DescribeClientVpnRoutes";

/// A subnet id that is a target network of the seeded endpoint.
const TARGET_SUBNET_A: &str = "subnet-00000000000000001";

/// A second target network of the same endpoint, so one destination can be routed through
/// either of two paths.
const TARGET_SUBNET_B: &str = "subnet-00000000000000002";

/// The VPC the target networks sit in.
const TARGET_VPC: &str = "vpc-00000000000000001";

/// One `CreateClientVpnRoute` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, CREATE, params).await
}

/// One `DeleteClientVpnRoute` call carrying `params`.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DELETE, params).await
}

/// One `DescribeClientVpnRoutes` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The form fields naming one route: the endpoint, the destination and — when the row has one
/// — the target traffic leaves through. The same fields `CreateClientVpnRoute` and
/// `DeleteClientVpnRoute` name a route by, so one row drives both calls.
fn route_params(
    endpoint_id: &str,
    destination: &str,
    target: Option<&str>,
) -> Vec<(String, String)> {
    let mut params = vec![
        ("ClientVpnEndpointId".to_string(), endpoint_id.to_string()),
        ("DestinationCidrBlock".to_string(), destination.to_string()),
    ];
    if let Some(target) = target {
        params.push(("TargetVpcSubnetId".to_string(), target.to_string()));
    }
    params
}

/// The one `ClientVpnEndpointId` parameter every describe below carries.
fn endpoint_param(endpoint_id: &str) -> Vec<(String, String)> {
    vec![("ClientVpnEndpointId".to_string(), endpoint_id.to_string())]
}

/// Write one Client VPN endpoint, the way `CreateClientVpnEndpoint` would, and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            vpc_id: Some(TARGET_VPC.to_string()),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Write one Client VPN endpoint that is attached to a Transit Gateway rather than to a VPC,
/// whose routes leave through that attachment rather than through a target subnet.
fn seed_transit_gateway_endpoint(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("a Transit Gateway-based endpoint".to_string()),
            client_cidr_block: Some("10.200.0.0/22".to_string()),
            transit_gateway_configuration: Some(
                TransitGatewayConfigurationDescribeEndpointStructure {
                    transit_gateway_id: Some("tgw-00000000000000001".to_string()),
                    transit_gateway_attachment_id: Some(
                        "tgw-attach-00000000000000001".to_string(),
                    ),
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Associate `subnets` with `endpoint_id`, the way `AssociateClientVpnTargetNetwork` would,
/// so a route may be added through them. No VPC range is named, so the association brings no
/// route of its own with it.
fn seed_target_networks(engine: &engine::InMemoryEngine, endpoint_id: &str, subnets: &[&str]) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for subnet in subnets {
        guard
            .insert_client_vpn_target_network(TargetNetworkSpec {
                client_vpn_endpoint_id: endpoint_id.to_string(),
                target_network_id: Some((*subnet).to_string()),
                vpc_id: Some(TARGET_VPC.to_string()),
                security_groups: Vec::new(),
                ..Default::default()
            })
            .unwrap_or_else(|| panic!("the store must take the association for {subnet}"));
    }
}

/// Associate one subnet with `endpoint_id` AND the route to `vpc_cidr_block` that association
/// adds by itself — the `associate` origin this action may not delete.
fn seed_target_network_with_route(
    engine: &engine::InMemoryEngine,
    endpoint_id: &str,
    subnet: &str,
    vpc_cidr_block: &str,
) {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_target_network(TargetNetworkSpec {
            client_vpn_endpoint_id: endpoint_id.to_string(),
            target_network_id: Some(subnet.to_string()),
            vpc_id: Some(TARGET_VPC.to_string()),
            security_groups: Vec::new(),
            vpc_cidr_block: Some(vpc_cidr_block.to_string()),
            ..Default::default()
        })
        .expect("the store must take the association and the route it adds");
}

/// How many routes the store holds, whichever endpoint they belong to — what the "nothing was
/// deleted" and "the route is gone" assertions read.
fn stored_routes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().client_vpn_routes.len()
}

/// One row of the route table under test: a destination and the target it is routed through.
struct RouteRow {
    case: &'static str,
    destination: &'static str,
    target: Option<&'static str>,
}

/// The routes the delete table below is driven against. They differ along every axis the rule
/// generalises over: the size of the destination block (a /0 default route, a /8, a /16 and a
/// single /32 host), which target the traffic leaves through (either target network of the
/// endpoint, or the local network), and — the last two rows — one destination routed through
/// two different targets, which is two routes that are deleted one at a time.
fn route_rows() -> Vec<RouteRow> {
    vec![
        RouteRow {
            case: "internet access through the first target network",
            destination: "0.0.0.0/0",
            target: Some(TARGET_SUBNET_A),
        },
        RouteRow {
            case: "a /8 block through the first target network",
            destination: "172.0.0.0/8",
            target: Some(TARGET_SUBNET_A),
        },
        RouteRow {
            case: "a single host through the second target network",
            destination: "192.168.5.7/32",
            target: Some(TARGET_SUBNET_B),
        },
        RouteRow {
            case: "the local network",
            destination: "10.100.0.0/22",
            target: Some("local"),
        },
        RouteRow {
            case: "a peered VPC through the first target network",
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
        },
        RouteRow {
            case: "the same peered VPC through the second target network",
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_B),
        },
    ]
}

/// Add every row of `rows` to `endpoint_id`, asserting each add is served.
async fn add_every_row(router: &MultiServiceRouter, endpoint_id: &str, rows: &[RouteRow]) {
    for row in rows {
        let (status, body) =
            create(router, &route_params(endpoint_id, row.destination, row.target)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the add expected HTTP 200, got {status} with body {body}",
            case = row.case
        );
    }
}

/// The (destination, target) pairs `DescribeClientVpnRoutes` reports for `endpoint_id`, in the
/// order it pages over them. Every route of these tests carries a target, so the two element
/// lists line up pair for pair.
async fn reported_routes(
    router: &MultiServiceRouter,
    endpoint_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = describe(router, &endpoint_param(endpoint_id)).await;
    assert_eq!(
        status, 200,
        "the report expected HTTP 200, got {status} with body {body}"
    );
    let destinations = elements(&body, "destinationCidr");
    let targets = elements(&body, "targetSubnet");
    assert_eq!(
        destinations.len(),
        targets.len(),
        "the report: expected one target per destination, got {body}"
    );
    destinations.into_iter().zip(targets).collect()
}

// ---------------------------------------------------------------------------
// What a delete takes out, and what it leaves
// ---------------------------------------------------------------------------

/// Every row is deleted by the endpoint, destination and target that name it: the call answers
/// that route `deleting`, the endpoint's route table no longer reports it, and every route not
/// named — including the same destination through the other target — is still reported.
#[tokio::test]
async fn each_added_route_is_deleted_by_the_identity_that_names_it() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "another endpoint of the same account");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);
    seed_target_networks(&engine, &other_endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);

    let rows = route_rows();
    add_every_row(&router, &endpoint, &rows).await;
    // The same table on a second endpoint, so every assertion below also says that a delete
    // reaches only the endpoint the request named.
    add_every_row(&router, &other_endpoint, &rows).await;
    assert_eq!(
        stored_routes(&engine),
        2 * rows.len(),
        "expected one stored route per added row on each endpoint, got {held}",
        held = stored_routes(&engine)
    );

    let mut remaining: Vec<(String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.destination.to_string(),
                row.target.unwrap_or_default().to_string(),
            )
        })
        .collect();
    for row in &rows {
        let (status, body) =
            delete(&router, &route_params(&endpoint, row.destination, row.target)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = row.case
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some("deleting"),
            "case {case:?}: a deleted route is answered deleting, got {body}",
            case = row.case
        );

        remaining.retain(|(destination, target)| {
            (destination.as_str(), target.as_str())
                != (row.destination, row.target.unwrap_or_default())
        });
        let reported = reported_routes(&router, &endpoint).await;
        let mut expected = remaining.clone();
        expected.sort();
        let mut actual = reported.clone();
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the routes not named to stay, got {reported:?}",
            case = row.case
        );
        assert_eq!(
            stored_routes(&engine),
            rows.len() + remaining.len(),
            "case {case:?}: expected the delete to take out exactly one route, got {held} \
             stored routes",
            case = row.case,
            held = stored_routes(&engine)
        );
    }

    // The second endpoint's route table is untouched by every delete above.
    let reported = reported_routes(&router, &other_endpoint).await;
    assert_eq!(
        reported.len(),
        rows.len(),
        "the other endpoint: expected its own route table to be untouched, got {reported:?}"
    );
}

/// A route of a Transit Gateway-based endpoint leaves through the endpoint's attachment rather
/// than through a target subnet, so it is named — and deleted — by a request that carries no
/// `TargetVpcSubnetId` at all. Deleting it a second time is refused rather than served.
#[tokio::test]
async fn a_route_with_no_target_subnet_is_deleted_by_a_request_that_names_none() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_transit_gateway_endpoint(&engine);

    let destination = "10.40.0.0/16";
    let (status, body) = create(&router, &route_params(&endpoint, destination, None)).await;
    assert_eq!(
        status, 200,
        "the add: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete(&router, &route_params(&endpoint, destination, None)).await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some("deleting"),
        "the delete: a deleted route is answered deleting, got {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        0,
        "the delete: expected the route table to be empty, got {held} stored routes",
        held = stored_routes(&engine)
    );
    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "the report: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "destinationCidr").is_empty(),
        "the report: expected no route to be left, got {body}"
    );

    let (status, body) = delete(&router, &route_params(&endpoint, destination, None)).await;
    assert_eq!(
        status, 400,
        "the repeat: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidClientVpnRouteNotFound",
        "the repeat: expected the route not-found code, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Route {destination} does not exist in Client VPN endpoint {endpoint}"),
        "the repeat: expected the refusal to name the route the request named, got {body}"
    );
}

// ---------------------------------------------------------------------------
// What a delete refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: a request naming no deletable route, and the code EC2
/// answers it with.
struct RefusalRow {
    case: &'static str,
    endpoint_id: String,
    destination: &'static str,
    target: Option<&'static str>,
    status: u16,
    code: &'static str,
}

/// A request that names no deletable route is refused with the code for the part of it that
/// cannot be served, and the stored route table is left exactly as it was.
#[tokio::test]
async fn a_request_that_names_no_deletable_route_is_refused_and_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(&engine, "an endpoint with no route of its own");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A, TARGET_SUBNET_B]);

    let destination = "10.20.0.0/16";
    let (status, body) = create(
        &router,
        &route_params(&endpoint, destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the add: expected HTTP 200, got {status} with body {body}"
    );

    let rows = vec![
        RefusalRow {
            case: "a destination the endpoint has no route for",
            endpoint_id: endpoint.clone(),
            destination: "10.99.0.0/16",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidClientVpnRouteNotFound",
        },
        RefusalRow {
            case: "the stored destination through another target",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_B),
            status: 400,
            code: "InvalidClientVpnRouteNotFound",
        },
        RefusalRow {
            case: "the stored destination with no target named",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.0/16",
            target: None,
            status: 400,
            code: "InvalidClientVpnRouteNotFound",
        },
        RefusalRow {
            case: "another endpoint's route table",
            endpoint_id: other_endpoint.clone(),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidClientVpnRouteNotFound",
        },
        RefusalRow {
            case: "a destination that is not a CIDR block",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.0",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a destination whose host bits are set",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.1/16",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a destination with a prefix no IPv4 block has",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.0/33",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a target that is not a subnet id",
            endpoint_id: endpoint.clone(),
            destination: "10.20.0.0/16",
            target: Some(TARGET_VPC),
            status: 400,
            code: "InvalidSubnetID.Malformed",
        },
        RefusalRow {
            case: "no endpoint named at all",
            endpoint_id: String::new(),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "MissingParameter",
        },
        RefusalRow {
            case: "no destination named at all",
            endpoint_id: endpoint.clone(),
            destination: "",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "MissingParameter",
        },
        RefusalRow {
            case: "an endpoint this account holds none of",
            endpoint_id: "cvpn-endpoint-0123456789abcdef0".to_string(),
            destination: "10.20.0.0/16",
            target: Some(TARGET_SUBNET_A),
            status: 400,
            code: "InvalidClientVpnEndpointId.NotFound",
        },
    ];

    for row in &rows {
        let (status, body) = delete(
            &router,
            &route_params(&row.endpoint_id, row.destination, row.target),
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}",
            case = row.case,
            expected = row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {expected:?}, got {body}",
            case = row.case,
            expected = row.code
        );
        assert_eq!(
            stored_routes(&engine),
            1,
            "case {case:?}: a refused delete takes out nothing, got {held} stored routes",
            case = row.case,
            held = stored_routes(&engine)
        );
    }

    // And the route every row left alone is still deletable by the identity that names it.
    let (status, body) = delete(
        &router,
        &route_params(&endpoint, destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_routes(&engine),
        0,
        "the delete: expected the route table to be empty, got {held} stored routes",
        held = stored_routes(&engine)
    );
}

/// A route a target network association added carries the `associate` origin, and this action
/// deletes only what a caller added: the delete is refused for it, the route stays in the
/// table, and a route the caller DID add on the same endpoint is still deleted.
#[tokio::test]
async fn a_route_an_association_added_is_not_deleted_while_an_added_one_is() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    let associated_destination = "10.30.0.0/16";
    seed_target_network_with_route(
        &engine,
        &endpoint,
        TARGET_SUBNET_A,
        associated_destination,
    );

    let added_destination = "10.50.0.0/16";
    let (status, body) = create(
        &router,
        &route_params(&endpoint, added_destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the add: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete(
        &router,
        &route_params(&endpoint, associated_destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 400,
        "the automatically added route: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "UnsupportedOperation",
        "the automatically added route: expected the unsupported code, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "Route {associated_destination} in Client VPN endpoint {endpoint} was \
             automatically added when associating a subnet and cannot be deleted"
        ),
        "the automatically added route: expected the refusal to name the route, got {body}"
    );
    let reported = reported_routes(&router, &endpoint).await;
    assert!(
        reported.contains(&(
            associated_destination.to_string(),
            TARGET_SUBNET_A.to_string()
        )),
        "the automatically added route: expected it to stay in the table, got {reported:?}"
    );

    // The route the caller added, through the SAME target of the SAME endpoint, is deleted:
    // what decides is the origin the route carries, not the endpoint or the target.
    let (status, body) = delete(
        &router,
        &route_params(&endpoint, added_destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the added route: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_routes(&router, &endpoint).await;
    assert_eq!(
        reported,
        vec![(
            associated_destination.to_string(),
            TARGET_SUBNET_A.to_string()
        )],
        "the added route: expected only the automatically added route to be left, got \
         {reported:?}"
    );
}

/// A dry run reports the permission outcome and deletes NOTHING, whichever route it names.
#[tokio::test]
async fn a_dry_run_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A]);

    let destination = "10.70.0.0/16";
    let (status, body) = create(
        &router,
        &route_params(&endpoint, destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the add: expected HTTP 200, got {status} with body {body}"
    );

    for case in ["the stored route", "a route the table does not hold"] {
        let named = if case == "the stored route" {
            destination
        } else {
            "10.71.0.0/16"
        };
        let mut params = route_params(&endpoint, named, Some(TARGET_SUBNET_A));
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected the dry-run code, got {body}"
        );
        assert_eq!(
            stored_routes(&engine),
            1,
            "case {case:?}: a dry run deletes nothing, got {held} stored routes",
            held = stored_routes(&engine)
        );
    }
}

/// A `ClientToken` names the route it added, so the delete that takes that route out takes the
/// token with it: the same request may be replayed with the same token and adds the route
/// again rather than replaying into a route the store no longer holds.
#[tokio::test]
async fn the_client_token_of_a_deleted_route_is_forgotten_with_it() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "the endpoint under test");
    seed_target_networks(&engine, &endpoint, &[TARGET_SUBNET_A]);

    let destination = "10.80.0.0/16";
    let mut params = route_params(&endpoint, destination, Some(TARGET_SUBNET_A));
    params.push((
        "ClientToken".to_string(),
        "a-token-that-names-one-route".to_string(),
    ));

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the add: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = delete(
        &router,
        &route_params(&endpoint, destination, Some(TARGET_SUBNET_A)),
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the replay after the delete: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some("creating"),
        "the replay after the delete: expected a newly added route, got {body}"
    );
    let reported = reported_routes(&router, &endpoint).await;
    assert_eq!(
        reported,
        vec![(destination.to_string(), TARGET_SUBNET_A.to_string())],
        "the replay after the delete: expected the route to be added once, got {reported:?}"
    );
}
