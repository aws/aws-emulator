// SPDX-License-Identifier: CC0-1.0

//! The default public DNS names a published endpoint service is ASSIGNED, driven over
//! `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `ServiceDetail$PrivateDnsNames` is "the private DNS names assigned to
//! the VPC endpoint service"
//! (`smithy_query(query="doc",shape="ServiceDetail",member="PrivateDnsNames")`), which is
//! EVERY default public name the service answers on — not the one
//! `ServiceDetail$PrivateDnsName` reports read twice. A service of the Region's catalog
//! answers on two: the host under the domain its own name family names, and the SAME host
//! under the dual-stack `api.aws` domain Amazon Web Services serves its APIs on over both
//! address families. A service whose name family is already the `api.aws` one answers on that
//! host once, there being no second domain for it, and a service that publishes no default
//! public name at all — a gateway-only service, or one published with no private DNS —
//! reports neither member.
//!
//! The rows span what the rule generalises over: a plain label, a DOTTED label whose host form
//! is its components reversed, a label reachable both through a gateway and through an
//! interface endpoint, a gateway-only one, one published with no private DNS at all, and the
//! two families named under the `aws.api` / `api.aws` labels — each asked about in two Regions
//! of different naming, so nothing about the answer is tied to one Region or one service.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};
use super::wire::param;

/// The domain a service of the `com.amazonaws` name family publishes its default public name
/// under.
const SERVICE_DOMAIN: &str = "amazonaws.com";

/// The dual-stack domain every service of the catalog is ALSO assigned a name under, and the
/// only domain the `aws.api` / `api.aws` families are named in.
const DUAL_STACK_DOMAIN: &str = "api.aws";

/// One published endpoint service, and the host its default public names are built on.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// The service name as the catalog publishes it, with `{region}` standing for the Region
    /// the call is made in.
    service_name: &'static str,
    /// The DNS host form of its label — the label's dotted components in reverse order — or
    /// `None` for a service that publishes no default public name at all.
    dns_host: Option<&'static str>,
    /// The domain its own name family names.
    family_domain: &'static str,
}

/// The services under test.
const CASES: [Case; 7] = [
    Case {
        service_name: "com.amazonaws.{region}.logs",
        dns_host: Some("logs"),
        family_domain: SERVICE_DOMAIN,
    },
    // A dotted label: `com.amazonaws.<region>.ecr.dkr` is the service reached at
    // `dkr.ecr.<region>.…`.
    Case {
        service_name: "com.amazonaws.{region}.ecr.dkr",
        dns_host: Some("dkr.ecr"),
        family_domain: SERVICE_DOMAIN,
    },
    // Reachable both ways: only the INTERFACE endpoint service of the label has a private
    // hosted zone to answer on, so exactly one name set is reported under the name.
    Case {
        service_name: "com.amazonaws.{region}.s3",
        dns_host: Some("s3"),
        family_domain: SERVICE_DOMAIN,
    },
    // Gateway only: reached through a prefix list in a route table, with no name to assign.
    Case {
        service_name: "com.amazonaws.{region}.dynamodb",
        dns_host: None,
        family_domain: SERVICE_DOMAIN,
    },
    // An interface service published with NO private DNS at all.
    Case {
        service_name: "com.amazonaws.{region}.iot.data",
        dns_host: None,
        family_domain: SERVICE_DOMAIN,
    },
    Case {
        service_name: "api.aws.{region}.cassandra-streams",
        dns_host: Some("cassandra-streams"),
        family_domain: DUAL_STACK_DOMAIN,
    },
    Case {
        service_name: "aws.api.{region}.emr-service-cell01",
        dns_host: Some("emr-service-cell01"),
        family_domain: DUAL_STACK_DOMAIN,
    },
];

/// The Regions the table is driven in.
const REGIONS: [&str; 2] = ["us-west-2", "eu-central-1"];

/// The names the service of `case` must report as assigned to it, in order.
fn expected_names(case: Case, region: &str) -> Vec<String> {
    let Some(host) = case.dns_host else {
        return Vec::new();
    };
    let mut names = vec![format!("{host}.{region}.{}", case.family_domain)];
    if case.family_domain != DUAL_STACK_DOMAIN {
        names.push(format!("{host}.{region}.{DUAL_STACK_DOMAIN}"));
    }
    names
}

/// The body `DescribeVpcEndpointServices` answers in `region` for exactly one service name.
async fn described(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    service_name: &str,
) -> String {
    let (status, body) = call(
        router,
        region,
        "DescribeVpcEndpointServices",
        &indexed("ServiceName", &[service_name]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for {service_name}, got {status} with body {body}"
    );
    body
}

/// Rule: every default public name the service answers on is reported as assigned to it, the
/// one `PrivateDnsName` carries first.
#[tokio::test]
async fn a_published_service_reports_every_name_assigned_to_it() {
    let (router, _engine) = super::ec2_query::emulator();

    for region in REGIONS {
        for entry in CASES {
            let service_name = entry.service_name.replace("{region}", region);
            let case = format!("{service_name} in {region}");
            let body = described(&case, &router, region, &service_name).await;
            let expected = expected_names(entry, region);
            // One `<privateDnsNameSet>` per reported endpoint service that publishes a name:
            // read each set's own names rather than every name in the body, so the two
            // endpoint services a label reachable both ways carries stay apart.
            let reported: Vec<Vec<String>> = elements(&body, "privateDnsNameSet")
                .iter()
                .map(|set| elements(set, "privateDnsName"))
                .collect();
            if expected.is_empty() {
                assert!(
                    reported.is_empty(),
                    "case {case:?}: a service publishing no default public name reports no \
                     privateDnsNameSet, got {reported:?}"
                );
                assert!(
                    element(&body, "privateDnsName").is_none(),
                    "case {case:?}: and no privateDnsName either, got {body}"
                );
                continue;
            }
            assert_eq!(
                reported,
                vec![expected.clone()],
                "case {case:?}: expected the assigned names {expected:?}, got {reported:?}"
            );
            assert_eq!(
                element(&body, "privateDnsName").as_deref(),
                expected.first().map(String::as_str),
                "case {case:?}: privateDnsName must report the first of {expected:?}, got {body}"
            );
        }
    }
}

/// Rule: the name an interface endpoint's private hosted zone answers on is still the FIRST of
/// the names assigned to the service — the second name is assigned to the service, not to the
/// zone the endpoint associates.
#[tokio::test]
async fn an_interface_endpoint_answers_on_the_services_first_assigned_name() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = REGIONS[0];
    let case = "the network";
    let (status, body) = call(
        &router,
        region,
        "CreateVpc",
        &[param("CidrBlock", "10.91.0.0/16")],
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected a VPC, got {body}");
    let vpc_id = element(&body, "vpcId").expect("the created VPC carries an id");
    // A VPC you create has DNS hostnames off, and a private hosted zone needs them on.
    let (status, body) = call(
        &router,
        region,
        "ModifyVpcAttribute",
        &[
            param("VpcId", &vpc_id),
            param("EnableDnsHostnames.Value", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected the attribute set, got {body}");
    let (status, body) = call(
        &router,
        region,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", "10.91.1.0/24"),
            param("AvailabilityZone", &format!("{region}a")),
        ],
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected a subnet, got {body}");
    let subnet_id = element(&body, "subnetId").expect("the created subnet carries an id");

    for entry in CASES.iter().filter(|entry| entry.dns_host.is_some()) {
        let service_name = entry.service_name.replace("{region}", region);
        let case = format!("an endpoint to {service_name}");
        let expected = expected_names(*entry, region);
        let primary = expected.first().expect("the row publishes a name");
        let (status, body) = call(
            &router,
            region,
            "CreateVpcEndpoint",
            &[
                param("VpcId", &vpc_id),
                param("ServiceName", &service_name),
                param("VpcEndpointType", "Interface"),
                param("PrivateDnsEnabled", "true"),
                param("SubnetId.1", &subnet_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the endpoint to be created, got {status} with {body}"
        );
        let names = elements(&body, "dnsName");
        assert!(
            names.iter().any(|name| name == primary),
            "case {case:?}: expected the service's first assigned name {primary:?} among \
             {names:?}"
        );
    }
}
