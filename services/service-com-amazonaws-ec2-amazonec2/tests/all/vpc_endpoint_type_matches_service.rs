// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the endpoint-type rule `CreateVpcEndpoint` holds a request to, driven
//! over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the `VpcEndpointType` a create asks for has to be one of the types the
//! endpoint service it names is available through ("the type of service",
//! `ServiceDetail$ServiceType`, as `DescribeVpcEndpointServices` reports it); a create of any
//! other type is refused with `InvalidParameter` naming the requested type and the service's
//! own types, and stores nothing — while every pairing the service does offer is created and
//! reads back through `DescribeVpcEndpoints`.
//!
//! Each row of the table below is a distinct input along what that rule generalises over:
//! which endpoint type was asked for, which types the service offers (gateway only, interface
//! only, both), and which SOURCE the service came from — the Region's published catalogue
//! under each of the service-name prefixes, and an endpoint service this account configured
//! itself, whose types its own load balancers decided.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, reached only over the wire.
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
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The VPC and the one subnet every endpoint below is created in.
struct Network {
    vpc_id: String,
    subnet_id: String,
}

/// Build that network through the API, so each create below names resources this emulator
/// really holds.
async fn network(router: &MultiServiceRouter) -> Network {
    let case = "the network";
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", "10.70.0.0/16")]).await;
    let vpc_id = member(case, &body, "vpcId");
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.70.1.0/24"),
            param("AvailabilityZone", "us-east-1a"),
        ],
    )
    .await;
    let subnet_id = member(case, &body, "subnetId");
    Network { vpc_id, subnet_id }
}

/// The types `DescribeVpcEndpointServices` reports for the one service `service_name`, in the
/// order it reports them — ASKED of the service rather than assumed by this test, so the
/// expected half of the message below comes from the same catalogue the create resolves
/// against.
async fn reported_service_types(
    case: &str,
    router: &MultiServiceRouter,
    service_name: &str,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpointServices",
        &indexed("ServiceName", &[service_name]),
    )
    .await;
    // The member is a LIST of structures, so the set element and each entry's own member
    // share the one name: the tree is walked rather than the text scanned, and only the
    // entries that carry text — the leaves — are the service's types.
    let document = super::wire::document(case, &body);
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "serviceType")
        .filter_map(|node| node.text().map(str::to_string))
        .filter(|service_type| !service_type.trim().is_empty())
        .collect()
}

/// The message EC2 refuses an endpoint of a type its service does not offer with.
fn mismatch_message(endpoint_type: &str, service_types: &[String]) -> String {
    format!(
        "Endpoint type ({endpoint_type}) does not match available service types ([{types}]).",
        types = service_types.join(", "),
    )
}

/// One row of the table: an endpoint type asked of a service of the Region's catalogue.
struct Row {
    case: &'static str,
    /// `VpcEndpointType`, or `None` to leave the member out — "Default: Gateway".
    endpoint_type: Option<&'static str>,
    /// The endpoint service the create names.
    service_name: &'static str,
    /// Whether the service offers that type, so the create succeeds.
    offered: bool,
}

/// The rows.
///
/// The services are entries of the Region's published catalogue, which
/// `DescribeVpcEndpointServices` reports and this suite asks for each service's types rather
/// than hard-coding them: Amazon S3 is reachable both ways, Amazon DynamoDB only through a
/// gateway endpoint, and every other service of the catalogue only through an interface
/// endpoint — including the Amazon EMR session service, whose name sits under the `aws.api`
/// prefix rather than `com.amazonaws`, and the Amazon Keyspaces streams service under
/// `api.aws`.
const ROWS: &[Row] = &[
    Row {
        case: "a gateway endpoint to a service reachable both ways",
        endpoint_type: Some("Gateway"),
        service_name: "com.amazonaws.us-east-1.s3",
        offered: true,
    },
    Row {
        case: "an interface endpoint to that same service",
        endpoint_type: Some("Interface"),
        service_name: "com.amazonaws.us-east-1.s3",
        offered: true,
    },
    Row {
        case: "the defaulted type, which is Gateway, to a gateway-only service",
        endpoint_type: None,
        service_name: "com.amazonaws.us-east-1.dynamodb",
        offered: true,
    },
    Row {
        case: "an interface endpoint to a gateway-only service",
        endpoint_type: Some("Interface"),
        service_name: "com.amazonaws.us-east-1.dynamodb",
        offered: false,
    },
    Row {
        case: "a Gateway Load Balancer endpoint to a gateway-only service",
        endpoint_type: Some("GatewayLoadBalancer"),
        service_name: "com.amazonaws.us-east-1.dynamodb",
        offered: false,
    },
    Row {
        case: "a gateway endpoint to an interface-only service",
        endpoint_type: Some("Gateway"),
        service_name: "com.amazonaws.us-east-1.sqs",
        offered: false,
    },
    Row {
        case: "the defaulted type, which is Gateway, to an interface-only service",
        endpoint_type: None,
        service_name: "com.amazonaws.us-east-1.lambda",
        offered: false,
    },
    Row {
        case: "an interface endpoint to an interface-only service",
        endpoint_type: Some("Interface"),
        service_name: "com.amazonaws.us-east-1.sqs",
        offered: true,
    },
    Row {
        case: "a Gateway Load Balancer endpoint to an interface-only service",
        endpoint_type: Some("GatewayLoadBalancer"),
        service_name: "com.amazonaws.us-east-1.logs",
        offered: false,
    },
    Row {
        case: "an interface endpoint to a service named under the aws.api prefix",
        endpoint_type: Some("Interface"),
        service_name: "aws.api.us-east-1.emr-service-cell01",
        offered: true,
    },
    Row {
        case: "a gateway endpoint to a service named under the aws.api prefix",
        endpoint_type: Some("Gateway"),
        service_name: "aws.api.us-east-1.emr-service-cell01",
        offered: false,
    },
    Row {
        case: "a gateway endpoint to a service named under the api.aws prefix",
        endpoint_type: Some("Gateway"),
        service_name: "api.aws.us-east-1.cassandra-streams",
        offered: false,
    },
    Row {
        case: "an interface endpoint to a service named under the api.aws prefix",
        endpoint_type: Some("Interface"),
        service_name: "api.aws.us-east-1.cassandra-streams",
        offered: true,
    },
];

/// The form fields one create sends: a subnet only for the types that take one, since an
/// endpoint type that stands up no network interface refuses `SubnetIds` outright.
fn create_params(row: &Row, built: &Network) -> Vec<Param> {
    let mut params = vec![
        param("VpcId", &built.vpc_id),
        param("ServiceName", row.service_name),
        // These rows are about the endpoint TYPE a service offers, so they ask for no
        // private hosted zone: a VPC you create has DNS hostnames off, which is what an
        // endpoint associating one needs.
        param("PrivateDnsEnabled", "false"),
    ];
    if let Some(endpoint_type) = row.endpoint_type {
        params.push(param("VpcEndpointType", endpoint_type));
    }
    if matches!(row.endpoint_type, Some("Interface")) {
        params.push(param("SubnetId.1", &built.subnet_id));
    }
    params
}

/// Every row: a type the service offers is created and reads back, and a type it does not is
/// refused with `InvalidParameter` naming both halves of the mismatch, leaving no endpoint
/// behind.
#[tokio::test]
async fn an_endpoint_is_created_only_for_a_type_its_service_offers() {
    let router = emulator();
    let built = network(&router).await;

    for row in ROWS {
        let case = row.case;
        let requested = row.endpoint_type.unwrap_or("Gateway");
        let before = ok(case, &router, "DescribeVpcEndpoints", &[]).await;
        let before = elements(&before, "vpcEndpointId");

        let (status, body) = call(&router, "CreateVpcEndpoint", &create_params(row, &built)).await;
        let after = ok(case, &router, "DescribeVpcEndpoints", &[]).await;
        let after = elements(&after, "vpcEndpointId");

        if !row.offered {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a type the service does not offer, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_PARAMETER,
                "case {case:?}: expected {}, got {body}",
                ec2::state::INVALID_PARAMETER
            );
            let service_types = reported_service_types(case, &router, row.service_name).await;
            assert_eq!(
                error_message(&body),
                mismatch_message(requested, &service_types),
                "case {case:?}: expected the message to name the requested type and the \
                 service's own types {service_types:?}, got {body}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store nothing, the account holds \
                 {after:?} where it held {before:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a type the service offers, got {status} with \
             body {body}"
        );
        let endpoint_id = member(case, &body, "vpcEndpointId");
        assert_eq!(
            member(case, &body, "vpcEndpointType"),
            requested,
            "case {case:?}: the endpoint reports the type it was created with, got {body}"
        );
        // The round trip: the created endpoint is the record the store holds, read back by the
        // id the create minted.
        let read = ok(
            case,
            &router,
            "DescribeVpcEndpoints",
            &indexed("VpcEndpointId", &[&endpoint_id]),
        )
        .await;
        assert_eq!(
            member(case, &read, "serviceName"),
            row.service_name,
            "case {case:?}: the stored endpoint names the service it was created to, got {read}"
        );
        assert_eq!(
            member(case, &read, "vpcEndpointType"),
            requested,
            "case {case:?}: the stored endpoint reports its type, got {read}"
        );
        assert!(
            after.contains(&endpoint_id),
            "case {case:?}: expected the account to hold {endpoint_id:?}, it holds {after:?}"
        );
    }
}

/// The same rule for the OTHER source of endpoint services: one this account configured, whose
/// available types are the ones its own load balancers decided — a Gateway Load Balancer
/// service takes a `GatewayLoadBalancer` endpoint and refuses the two other types, naming the
/// type it does offer.
#[tokio::test]
async fn the_rule_holds_for_an_endpoint_service_this_account_configured() {
    let router = emulator();
    let built = network(&router).await;
    let case = "a Gateway Load Balancer service of this account";

    // The service, configured through the API: "before you create an endpoint service, you
    // must create one of the following for your service" — the load balancer behind it, whose
    // KIND is what decides the service's type.
    let body = ok(
        case,
        &router,
        "CreateVpcEndpointServiceConfiguration",
        &[param(
            "GatewayLoadBalancerArn.1",
            &format!(
                "arn:aws:elasticloadbalancing:{REGION}:{account}:loadbalancer/gwy/mirror/\
                 0123456789abcdef",
                account = engine::emulator_account_id(),
            ),
        )],
    )
    .await;
    let service_name = member(case, &body, "serviceName");
    let service_types = reported_service_types(case, &router, &service_name).await;
    assert_eq!(
        service_types,
        vec!["GatewayLoadBalancer".to_string()],
        "case {case:?}: the configured service must offer the type its load balancer decides, \
         got {service_types:?}"
    );

    // `(the case, the requested type, whether the service offers it)`.
    let rows: [(&str, &str, bool); 3] = [
        ("an interface endpoint to it", "Interface", false),
        ("a gateway endpoint to it", "Gateway", false),
        (
            "a Gateway Load Balancer endpoint to it",
            "GatewayLoadBalancer",
            true,
        ),
    ];

    for (case, endpoint_type, offered) in rows {
        let mut params = vec![
            param("VpcId", &built.vpc_id),
            param("ServiceName", &service_name),
            param("VpcEndpointType", endpoint_type),
        ];
        if endpoint_type != "Gateway" {
            params.push(param("SubnetId.1", &built.subnet_id));
        }
        let (status, body) = call(&router, "CreateVpcEndpoint", &params).await;
        if !offered {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a type the service does not offer, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_PARAMETER,
                "case {case:?}: expected {}, got {body}",
                ec2::state::INVALID_PARAMETER
            );
            assert_eq!(
                error_message(&body),
                mismatch_message(endpoint_type, &service_types),
                "case {case:?}: expected the message to name the requested type and the \
                 service's own types {service_types:?}, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the type the service offers, got {status} \
             with body {body}"
        );
        assert_eq!(
            member(case, &body, "serviceName"),
            service_name,
            "case {case:?}: the endpoint names the service it was created to, got {body}"
        );
    }
}

/// A service name of the PARTNER form names a service another account or a Marketplace partner
/// offers, which this emulator holds no catalogue of: there is no type list to hold such a
/// create to, so a name of that form is read for its form alone and an endpoint of either type
/// that takes a subnet is created to it.
#[tokio::test]
async fn a_partner_service_name_is_held_to_its_form_rather_than_to_a_type_list() {
    let router = emulator();
    let built = network(&router).await;

    for (case, endpoint_type) in [
        ("an interface endpoint to a partner service", "Interface"),
        (
            "a Gateway Load Balancer endpoint to a partner service",
            "GatewayLoadBalancer",
        ),
    ] {
        let body = ok(
            case,
            &router,
            "CreateVpcEndpoint",
            &[
                param("VpcId", &built.vpc_id),
                param(
                    "ServiceName",
                    &format!("com.amazonaws.vpce.{REGION}.vpce-svc-0a1b2c3d4e5f6a7b8"),
                ),
                param("VpcEndpointType", endpoint_type),
                param("SubnetId.1", &built.subnet_id),
            ],
        )
        .await;
        assert_eq!(
            member(case, &body, "vpcEndpointType"),
            endpoint_type,
            "case {case:?}: the endpoint reports the type it was created with, got {body}"
        );
    }
}
