// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the STATE a VPC endpoint reports, driven over `ec2Query` (README.md
//! / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: the state of a VPC endpoint is reported in the vocabulary the model
//! publishes for the filter that selects on that very state — "`vpc-endpoint-state` - The
//! state of the endpoint (`pendingAcceptance` | `pending` | `available` | `deleting` |
//! `deleted` | `rejected` | `failed`)" (`DescribeVpcEndpointsRequest$Filters`) — so an
//! endpoint is reported in the state its type is CREATED in and in the one the clock has
//! carried it to, under exactly those spellings, and the `vpc-endpoint-state` filter selects
//! an endpoint by the value it reports. A client that waits for an endpoint to be in service
//! compares against that value, so the spelling is the contract.
//!
//! Each table row is a distinct input along what that rule generalises over: which endpoint
//! type (each one this service creates), which endpoint service the endpoint is to —
//! including the dedicated regional PrivateLink service a syslog ingestion endpoint is
//! created to — and how many subnets and security groups the endpoint was given. What varies
//! with the row is the state the endpoint STARTS in: a gateway endpoint stands up no endpoint
//! network interface, so it is in service as soon as it exists, while an endpoint that does
//! is `pending` until the clock has carried those interfaces into service. Every row ends
//! `available`.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, filters};
use super::wire::{param, Param};

/// The Region every call here is signed for, and the Region of the endpoint services below.
const REGION: &str = "us-east-1";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that a move the clock owed a created
/// endpoint is certainly overdue: the deadline the poll below gives up at, generous because
/// what is under test is the state the endpoint reaches rather than how fast it reaches it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll below waits between two reads of an endpoint's state.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

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

/// The state `DescribeVpcEndpoints` reports for the one endpoint `endpoint_id`.
async fn described_state(case: &str, router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &[param("VpcEndpointId.1", endpoint_id)],
    )
    .await;
    member(case, &body, "state")
}

/// The endpoint ids `DescribeVpcEndpoints` reports for the `vpc-endpoint-state` filter
/// `value`.
async fn ids_in_state(case: &str, router: &MultiServiceRouter, value: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeVpcEndpoints",
        &filters(&[("vpc-endpoint-state", &[value])]),
    )
    .await;
    elements(&body, "vpcEndpointId")
}

/// One row of the table: an endpoint of one type, to one endpoint service.
struct EndpointCase {
    case: &'static str,
    /// The `VpcEndpointType`, or `None` to leave it to the documented default (`Gateway`).
    endpoint_type: Option<&'static str>,
    /// The endpoint service the endpoint is created to.
    service_name: &'static str,
    /// How many of the network's subnets the create names.
    subnets: usize,
    /// Whether the create names the VPC's own security group.
    with_group: bool,
    /// The state the create answers and the describe reports until the clock moves it: an
    /// endpoint that stands up endpoint network interfaces starts `pending`, and one that
    /// stands up none is in service at once.
    created_state: &'static str,
}

/// The rows: every endpoint type this service creates, and the service names a client uses —
/// a Region's own service, the dedicated regional PrivateLink service a CloudWatch Logs
/// syslog ingestion endpoint is created to, and a partner service name.
fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            case: "an interface endpoint to the Region's syslog ingestion service",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.syslog-logs",
            subnets: 1,
            with_group: true,
            created_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
        EndpointCase {
            case: "an interface endpoint in two subnets, with no group of its own",
            endpoint_type: Some("Interface"),
            service_name: "com.amazonaws.us-east-1.logs",
            subnets: 2,
            with_group: false,
            created_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
        EndpointCase {
            case: "a gateway endpoint, the documented default type",
            endpoint_type: None,
            service_name: "com.amazonaws.us-east-1.s3",
            subnets: 0,
            with_group: false,
            created_state: ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
        },
        EndpointCase {
            case: "a Gateway Load Balancer endpoint to a partner service",
            endpoint_type: Some("GatewayLoadBalancer"),
            service_name: "com.amazonaws.vpce.us-east-1.vpce-svc-0123456789abcdef0",
            subnets: 1,
            with_group: false,
            created_state: ec2::state::VPC_ENDPOINT_STATE_PENDING,
        },
    ]
}

/// A created endpoint reports the state its type is created in, the clock carries the one
/// that owes a move to `available`, and both values are the ones the filter's published list
/// spells — for every row, and read back through the describe and through the filter.
#[tokio::test]
async fn an_endpoint_reports_its_state_in_the_published_vocabulary() {
    let router = super::ec2_query::emulator().0;

    // The network every endpoint below is created in: one VPC, one subnet per Availability
    // Zone, and the VPC's own security group.
    let vpc_id = {
        let body = ok(
            "the network",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.80.0.0/16")],
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
                param("CidrBlock", &format!("10.80.{}.0/24", index + 1)),
                param("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(member("the network", &body, "subnetId"));
    }
    let group_id = {
        let body = ok(
            "the network",
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &vpc_id),
                param("GroupName", "syslog-endpoint"),
                param("GroupDescription", "the syslog ingestion endpoint"),
            ],
        )
        .await;
        member("the network", &body, "groupId")
    };

    let mut created: Vec<(&str, String, &str)> = Vec::new();
    for row in endpoint_cases() {
        let case = row.case;
        let mut params = vec![
            param("VpcId", &vpc_id),
            param("ServiceName", row.service_name),
            // These rows are about the STATE an endpoint is created in, so they ask for no
            // private hosted zone: a VPC you create has DNS hostnames off, which is what an
            // endpoint associating one needs.
            param("PrivateDnsEnabled", "false"),
        ];
        if let Some(endpoint_type) = row.endpoint_type {
            params.push(param("VpcEndpointType", endpoint_type));
        }
        for (ordinal, subnet_id) in subnets.iter().take(row.subnets).enumerate() {
            params.push(param(&format!("SubnetId.{}", ordinal + 1), subnet_id));
        }
        if row.with_group {
            params.push(param("SecurityGroupId.1", &group_id));
        }

        let body = ok(case, &router, "CreateVpcEndpoint", &params).await;
        let endpoint_id = member(case, &body, "vpcEndpointId");
        // The create ANSWERS the stored record, so the state it reports is the stored one.
        assert_eq!(
            member(case, &body, "state"),
            row.created_state,
            "case {case:?}: expected the create to answer {state:?}, got {body}",
            state = row.created_state
        );
        assert_eq!(
            described_state(case, &router, &endpoint_id).await,
            row.created_state,
            "case {case:?}: the describe reports the state the create answered"
        );
        created.push((case, endpoint_id, row.created_state));
    }

    // Every value reported here is one the published list spells — the catalog is
    // `VPC_ENDPOINT_STATE_VALUES`, read off `DescribeVpcEndpointsRequest$Filters`.
    for state in [
        ec2::state::VPC_ENDPOINT_STATE_PENDING,
        ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
    ] {
        assert!(
            ec2::state::VPC_ENDPOINT_STATE_VALUES.contains(&state),
            "case the vocabulary: {state:?} is not one of {:?}",
            ec2::state::VPC_ENDPOINT_STATE_VALUES
        );
    }

    // The filter selects an endpoint by the value it reports, so each endpoint is selected by
    // the state it was created in and by no other.
    for state in [
        ec2::state::VPC_ENDPOINT_STATE_PENDING,
        ec2::state::VPC_ENDPOINT_STATE_AVAILABLE,
    ] {
        let selected = ids_in_state("the created-state filter", &router, state).await;
        let expected: Vec<&String> = created
            .iter()
            .filter(|(_, _, created_state)| *created_state == state)
            .map(|(_, endpoint_id, _)| endpoint_id)
            .collect();
        assert_eq!(
            selected.len(),
            expected.len(),
            "case the {state:?} filter: expected {expected:?}, got {selected:?}"
        );
        for endpoint_id in expected {
            assert!(
                selected.contains(endpoint_id),
                "case the {state:?} filter: expected it to select {endpoint_id:?}, got \
                 {selected:?}"
            );
        }
    }

    // The move the clock owed each endpoint that was not in service yet falls due on its own,
    // with nothing called in between to make it happen — so each is READ until it reports the
    // state it was owed rather than read once after a span of this test's choosing.
    for (case, endpoint_id, _) in &created {
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let state = described_state(case, &router, endpoint_id).await;
            if state == ec2::state::VPC_ENDPOINT_STATE_AVAILABLE {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: the clock owed {endpoint_id} its available state, last read \
                 {state:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    let available = ids_in_state("the available filter", &router, "available").await;
    assert_eq!(
        available.len(),
        created.len(),
        "case the available filter: every endpoint is in service now, got {available:?}"
    );
    assert!(
        ids_in_state("the pending filter", &router, "pending")
            .await
            .is_empty(),
        "case the pending filter: no endpoint is pending once the clock has moved them"
    );

    // "Filter values are case-sensitive" (`Filter$Values`): the capitalized member name of
    // the shared `State` enum is not the value this attribute reports, so it selects nothing.
    assert!(
        ids_in_state("a capitalized filter value", &router, "Available")
            .await
            .is_empty(),
        "case a capitalized filter value: the reported state is the published lower-case one"
    );
}
