// SPDX-License-Identifier: CC0-1.0

//! The endpoint service NAMES a caller can create a VPC endpoint to, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! THE RULE: an endpoint service name is read as the name of a service of the Region it
//! carries whenever it is one of the FORMS Amazon Web Services publishes a service under —
//! `com.amazonaws.<region>.<service>`, the partner form
//! `com.amazonaws.vpce.<region>.<service>`, and `aws.api.<region>.<service>` for the
//! services published under the `aws.api` label (Amazon EMR's session endpoint service,
//! `aws.api.<region>.emr-service-cell01`, is one). `CreateVpcEndpoint` creates an endpoint
//! to any of them, and `InvalidServiceName` is reserved for a value that is no endpoint
//! service name at all. A service Amazon Web Services offers under either prefix is also
//! in the Region's catalogue, so `DescribeVpcEndpointServices` and the create agree about
//! what exists.
//!
//! The rows span what the rule generalises over: which published form the name takes,
//! which Region it names, whether the endpoint is an interface, a gateway or a Gateway
//! Load Balancer one, and — for the refusals — each way a value can fail to be a service
//! name (the prefix, the Region label, the missing service label).

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Amazon EMR's session endpoint service of [`REGION`]: the service a client that runs
/// Spark Connect sessions on a cluster reaches them through.
const EMR_SESSION_SERVICE: &str = "aws.api.us-east-1.emr-service-cell01";

/// One `action` call, as its HTTP status and XML body.
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

/// One row: a service name of a published form, and the endpoint type an endpoint to it is
/// created with.
struct Case {
    case: &'static str,
    service_name: &'static str,
    /// The `VpcEndpointType`, or `None` to leave it to the documented default (`Gateway`).
    endpoint_type: Option<&'static str>,
    /// How many of the network's subnets the create names.
    subnets: usize,
    /// Whether the Region's catalogue must report the service: it does for a service
    /// Amazon Web Services offers, and not for a partner service nothing here configured.
    catalogued: bool,
}

const CASES: [Case; 5] = [
    Case {
        case: "the EMR session endpoint service, under the aws.api prefix",
        service_name: EMR_SESSION_SERVICE,
        endpoint_type: Some("Interface"),
        subnets: 1,
        catalogued: true,
    },
    Case {
        case: "an interface service under com.amazonaws",
        service_name: "com.amazonaws.us-east-1.elasticmapreduce",
        endpoint_type: Some("Interface"),
        subnets: 2,
        catalogued: true,
    },
    Case {
        case: "a dotted service label under com.amazonaws",
        service_name: "com.amazonaws.us-east-1.ecr.dkr",
        endpoint_type: Some("Interface"),
        subnets: 1,
        catalogued: true,
    },
    Case {
        case: "a gateway service, at the documented default endpoint type",
        service_name: "com.amazonaws.us-east-1.s3",
        endpoint_type: None,
        subnets: 0,
        catalogued: true,
    },
    Case {
        case: "a partner service name",
        service_name: "com.amazonaws.vpce.us-east-1.vpce-svc-0123456789abcdef0",
        endpoint_type: Some("GatewayLoadBalancer"),
        subnets: 1,
        catalogued: false,
    },
];

/// Every published form of a service name creates an endpoint to the service it names, and
/// a service Amazon Web Services offers is in the Region's catalogue beside it.
#[tokio::test]
async fn an_endpoint_is_created_to_a_service_of_every_published_name_form() {
    let router = super::ec2_query::emulator().0;

    // The network every endpoint below is created in: one VPC and one subnet per
    // Availability Zone.
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.90.0.0/16")],
        )
        .await;
        member("the network", &body, "vpcId")
    };
    let mut subnets: Vec<String> = Vec::new();
    for (index, zone) in ["us-east-1a", "us-east-1b"].iter().enumerate() {
        let body = ok(
            "the network",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &vpc_id),
                param("CidrBlock", &format!("10.90.{}.0/24", index + 1)),
                param("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(member("the network", &body, "subnetId"));
    }

    for row in &CASES {
        let case = row.case;
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
            // These rows are about the service NAME an endpoint is created to, so they ask
            // for no private hosted zone: a VPC you create has DNS hostnames off, which is
            // what an endpoint associating one needs.
            param("PrivateDnsEnabled", "false"),
        ];
        if let Some(endpoint_type) = row.endpoint_type {
            params.push(param("VpcEndpointType", endpoint_type));
        }
        for (ordinal, subnet_id) in subnets.iter().take(row.subnets).enumerate() {
            params.push(param(&format!("SubnetId.{}", ordinal + 1), subnet_id));
        }

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        assert_eq!(
            member(case, &body, "serviceName"),
            row.service_name,
            "case {case:?}: the endpoint names the service it was created to, got {body}"
        );
        // The Region the service name carries is the Region hosting the service.
        assert_eq!(
            member(case, &body, "serviceRegion"),
            REGION,
            "case {case:?}: the service's Region comes from its name, got {body}"
        );

        // The create ANSWERS the stored record, so the describe reports the same endpoint.
        let described = ok(
            case,
            &router,
            "DescribeVpcEndpoints",
            &[param("VpcEndpointId.1", &endpoint_id)],
        )
        .await;
        assert_eq!(
            element(&described, "serviceName").as_deref(),
            Some(row.service_name),
            "case {case:?}: the stored endpoint is the one that was created, got {described}"
        );

        // A service Amazon Web Services offers is in the Region's catalogue under the
        // very name the endpoint was created to.
        let (status, catalogue) = call(
            &router,
            "DescribeVpcEndpointServices",
            &indexed("ServiceName", &[row.service_name]),
        )
        .await;
        if row.catalogued {
            assert_eq!(
                status, 200,
                "case {case:?}: the Region offers this service, got {status} with {catalogue}"
            );
            assert_eq!(
                elements(&catalogue, "serviceName"),
                vec![row.service_name.to_string()],
                "case {case:?}: the catalogue reports the service by name, got {catalogue}"
            );
        } else {
            // Nothing configured a partner service here, so the Region offers none of
            // that name — which is the one thing `InvalidServiceName` is for.
            assert_eq!(
                error_code(&catalogue),
                "InvalidServiceName",
                "case {case:?}: a name no Region offers is refused, got {catalogue}"
            );
        }
    }
}

/// A value that is no endpoint service name at all is refused with `InvalidServiceName`,
/// whichever part of the name is at fault.
#[tokio::test]
async fn a_value_that_is_no_service_name_is_refused() {
    let router = super::ec2_query::emulator().0;
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.91.0.0/16")],
        )
        .await;
        member("the network", &body, "vpcId")
    };

    // Rows: a bare word, each published prefix misspelt, a Region label that is no Region
    // code, and a name carrying no service label at all.
    for value in [
        "not-a-service-name",
        "com.amazon.us-east-1.s3",
        "aws.apis.us-east-1.emr-service-cell01",
        "aws.api.not-a-region.emr-service-cell01",
        "aws.api.us-east-1.",
        "aws.api.us-east-1",
    ] {
        let (status, body) = call(
            &router,
            "CreateVpcEndpoint",
            &[
                param("VpcId", &vpc_id),
                param("ServiceName", value),
                param("VpcEndpointType", "Interface"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidServiceName",
            "case {value:?}: expected InvalidServiceName, got {body}"
        );
        assert!(
            body.contains(value),
            "case {value:?}: the refusal names the value at fault, got {body}"
        );
    }
}
