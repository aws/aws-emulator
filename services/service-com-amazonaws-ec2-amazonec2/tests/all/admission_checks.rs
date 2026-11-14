// SPDX-License-Identifier: CC0-1.0

//! What this service ADMITS, across the family of operations that act on a resource only
//! while it resolves and is in a state that admits the call.
//!
//! The transport is the crate's shared [`super::ec2_query`] harness, so every call here is
//! an ec2Query request a client could have sent and every refusal is read out of the error
//! envelope the caller receives rather than out of the store.
//!
//! The rule under test is one sentence: an operation admits a request only while every
//! resource it names resolves and is in a state that admits the call — otherwise it is
//! refused, with that resource's own code, and nothing is written. Its readings here:
//!
//! 1. STATE: a root volume replacement runs on a `running` instance, so every other
//!    lifecycle state is refused `IncorrectInstanceState`;
//! 2. STATE: a delete acts on a resource that is there to delete, so once the deletion it
//!    asks for is under way a second delete is refused rather than answered as though it had
//!    deleted the resource again;
//! 3. REFERENCE: a value naming a resource the account does not hold — a Capacity
//!    Reservation Fleet, a Dedicated Host reservation offering, an Elastic IP address, a
//!    term no reservation is offered for — is refused with that resource's own code and
//!    wording;
//! 4. CROSS-REFERENCE: an endpoint of a route server is created in a subnet of the VPC that
//!    route server serves, and a subnet of any other VPC is refused;
//! 5. ATTRIBUTE: an interface endpoint's private hosted zone is admitted only while the VPC
//!    can resolve it, which is also what decides the default of the member asking for one;
//! 6. WRITE: withdrawing an Elastic IP address transfer leaves the account no transfer of
//!    that address to report.
//!
//! Every assertion is driven from a TABLE whose rows differ along what the rule generalises
//! over — the operation, the resource it names, the state that resource is in, the VPC
//! attributes, whether the member asking for private DNS is given — so no one row is the
//! specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::ops::describe_host_reservation_offerings::{
    HOST_RESERVATION_TERM_YEARS, SECONDS_PER_RESERVATION_YEAR,
};
use service_com_amazonaws_ec2_amazonec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, InstanceStateName,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, call, element, elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image a launch names. `RunInstances` only has to accept the id; which image an
/// instance was launched from is no part of any rule below.
const AMI: &str = "ami-0123456789abcdef0";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One EC2 emulator over its own engine. The engine is kept so an instance can be held in a
/// state no sequence of calls leaves it in long enough to call about.
struct Ec2 {
    router: MultiServiceRouter,
    engine: ::std::sync::Arc<engine::InMemoryEngine>,
}

impl Ec2 {
    fn new() -> Self {
        let (router, engine) = ec2_query::emulator();
        Self { router, engine }
    }

    async fn call(&self, action: &str, params: &[(String, String)]) -> (u16, String) {
        call(&self.router, REGION, action, params).await
    }

    /// One call that must succeed, and the body it answered.
    async fn ok(&self, case: &str, action: &str, params: &[(String, String)]) -> String {
        let (status, body) = self.call(action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        body
    }

    /// Launch one instance and answer the id this account issued it.
    async fn launch(&self, case: &str) -> String {
        let body = self
            .ok(
                case,
                "RunInstances",
                &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
            )
            .await;
        element(&body, "instanceId").unwrap_or_else(|| {
            panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
        })
    }

    /// Hold one instance still in `state`.
    ///
    /// The transient lifecycle states cannot be held across a call through the operations
    /// that start them — the clock moves the instance on whether or not anybody calls — so
    /// the state is written into the store the way the sibling suites seed the branches no
    /// sequence of calls can hold still.
    fn park(&self, case: &str, instance_id: &str, state: InstanceStateName) {
        let store = self.engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard.instances.get_mut(instance_id).unwrap_or_else(|| {
            panic!("case {case:?}: the store must hold the launched instance {instance_id}")
        });
        instance.state = state;
        instance.pending_transition = None;
    }
}

/// The deadline a poll below gives up at: generous, because what is under test is the state a
/// resource reaches rather than how fast it reaches it.
const MOVE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll below waits between two reads of a resource's state.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

impl Ec2 {
    /// Wait until the `state` member of what `action` reports for `params` reads `wanted`.
    ///
    /// Polls for that STATE rather than sleeping for the span the clock owes the move, so a
    /// loaded machine cannot read the resource before the move has been applied.
    async fn wait_until_state(
        &self,
        case: &str,
        action: &str,
        params: &[(String, String)],
        wanted: &str,
    ) {
        let deadline = tokio::time::Instant::now() + MOVE_DEADLINE;
        loop {
            let body = self.ok(case, action, params).await;
            let reported = element(&body, "state");
            if reported.as_deref() == Some(wanted) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case:?}: expected {action} to report the state {wanted:?}, last read \
                 {reported:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1: a root volume replacement runs on a `running` instance
// ---------------------------------------------------------------------------

/// "Replaces the EBS-backed root volume for a `running` instance"
/// (`CreateReplaceRootVolumeTask`): the row for every other `InstanceStateName` is a refusal,
/// and the task the running row starts is reported back by
/// `DescribeReplaceRootVolumeTasks`.
#[tokio::test]
async fn a_root_volume_replacement_is_admitted_only_from_the_running_state() {
    // Every state the instance lifecycle has, which is the whole catalog of
    // `InstanceStateName` — one row each, so the rule is asserted over the states rather
    // than over the one the recorded probe happened to find.
    let refused = [
        ("still coming up", InstanceStateName::Pending),
        ("stopping", InstanceStateName::Stopping),
        ("stopped", InstanceStateName::Stopped),
        ("shutting down", InstanceStateName::ShuttingDown),
        ("terminated", InstanceStateName::Terminated),
    ];

    for (case, state) in refused {
        let ec2 = Ec2::new();
        let instance_id = ec2.launch(case).await;
        ec2.park(case, &instance_id, state);

        let (status, body) = ec2
            .call(
                "CreateReplaceRootVolumeTask",
                &[p("InstanceId", instance_id.clone())],
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from CreateReplaceRootVolumeTask, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectInstanceState",
            "case {case:?}: expected the wrong-state code, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            "Instance is not in the running state.",
            "case {case:?}: expected the sentence EC2 words that code with, got body {body}"
        );

        // A refused request started nothing: the account holds no task for the instance.
        let reported = ec2.ok(case, "DescribeReplaceRootVolumeTasks", &[]).await;
        assert!(
            elements(&reported, "replaceRootVolumeTaskId").is_empty(),
            "case {case:?}: a refused request must start no task, got {reported}"
        );
    }

    // The running row: the same request is admitted, and the task it started is reported.
    let case = "running";
    let ec2 = Ec2::new();
    let instance_id = ec2.launch(case).await;
    ec2.park(case, &instance_id, InstanceStateName::Running);
    let body = ec2
        .ok(
            case,
            "CreateReplaceRootVolumeTask",
            &[p("InstanceId", instance_id.clone())],
        )
        .await;
    let task_id = element(&body, "replaceRootVolumeTaskId").unwrap_or_else(|| {
        panic!("case {case:?}: the create must report the task it started: {body}")
    });
    let reported = ec2.ok(case, "DescribeReplaceRootVolumeTasks", &[]).await;
    assert_eq!(
        elements(&reported, "replaceRootVolumeTaskId"),
        vec![task_id],
        "case {case:?}: the task must read back through the describe, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a delete acts on a resource that is there to delete
// ---------------------------------------------------------------------------

/// A repeat of a delete is refused rather than answered as though it had deleted the
/// resource a second time — while the deletion is still under way and once it has completed.
///
/// The rows are two resources whose delete is clock-owed (an EC2 Instance Connect Endpoint
/// and a virtual private gateway) and, for each, the two states its own deletion passes
/// through; each carries the code that resource's refusal uses.
#[tokio::test]
async fn a_second_delete_of_a_resource_already_going_away_is_refused() {
    // (case, the create, the delete, the id member, the code for the repeat while the
    //  deletion is under way, the code once it has completed)
    let ec2 = Ec2::new();

    // An endpoint needs a subnet, which needs a VPC: the network every row below is built
    // in.
    let vpc = ec2
        .ok("a VPC", "CreateVpc", &[p("CidrBlock", "10.70.0.0/16")])
        .await;
    let vpc_id = element(&vpc, "vpcId").expect("CreateVpc reports the VPC it minted");
    let subnet = ec2
        .ok(
            "a subnet",
            "CreateSubnet",
            &[p("VpcId", vpc_id.clone()), p("CidrBlock", "10.70.1.0/24")],
        )
        .await;
    let subnet_id = element(&subnet, "subnetId").expect("CreateSubnet reports the subnet");

    let endpoint = ec2
        .ok(
            "an instance connect endpoint",
            "CreateInstanceConnectEndpoint",
            &[p("SubnetId", subnet_id.clone())],
        )
        .await;
    let endpoint_id = element(&endpoint, "instanceConnectEndpointId")
        .expect("CreateInstanceConnectEndpoint reports the endpoint it minted");

    let gateway = ec2
        .ok(
            "a virtual private gateway",
            "CreateVpnGateway",
            &[p("Type", "ipsec.1")],
        )
        .await;
    let gateway_id =
        element(&gateway, "vpnGatewayId").expect("CreateVpnGateway reports the gateway");

    // (case, the delete, its parameters, the id member a describe narrows by, the describe,
    //  the state the deletion reaches, the code for a repeat while it is under way, the code
    //  once it has completed)
    type DeleteRow = (
        &'static str,
        &'static str,
        Vec<(String, String)>,
        Vec<(String, String)>,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    );
    let rows: [DeleteRow; 2] = [
        (
            "an EC2 Instance Connect Endpoint",
            "DeleteInstanceConnectEndpoint",
            vec![p("InstanceConnectEndpointId", endpoint_id.clone())],
            vec![p("InstanceConnectEndpointId.1", endpoint_id.clone())],
            "DescribeInstanceConnectEndpoints",
            "delete-complete",
            "IncorrectState",
            "InvalidInstanceConnectEndpointId.NotFound",
        ),
        (
            "a virtual private gateway",
            "DeleteVpnGateway",
            vec![p("VpnGatewayId", gateway_id.clone())],
            vec![p("VpnGatewayId.1", gateway_id.clone())],
            "DescribeVpnGateways",
            "deleted",
            "IncorrectState",
            "InvalidVpnGatewayID.NotFound",
        ),
    ];

    for (case, action, params, _, _, _, under_way, _) in &rows {
        // The first delete is admitted: the resource is there to delete.
        let _ = ec2.ok(case, action, params).await;

        // A second delete while the deletion is under way: the resource is in no state to be
        // deleted.
        let (status, body) = ec2.call(action, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a repeated {action}, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            *under_way,
            "case {case:?}: expected the wrong-state code while the deletion is under way, \
             got body {body}"
        );
    }

    // And once the deletion the clock owed each of them has been applied — read until the
    // resource reports it, rather than waited out — the resources are gone: a delete naming
    // one is refused as the resource not being there at all.
    for (case, action, params, narrow_by, describe, deleted_state, _, completed) in &rows {
        ec2.wait_until_state(case, describe, narrow_by, deleted_state)
            .await;
        let (status, body) = ec2.call(action, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {action} once the deletion completed, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *completed,
            "case {case:?}: expected the not-found code once the deletion completed, got \
             body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a value naming no resource is refused with that resource's own code
// ---------------------------------------------------------------------------

/// Each row names a resource — or a reservation term — the account does not hold, and is
/// refused with that resource's own code and the wording EC2 answers it with; the positive
/// rows beside them pin that the same operations serve a value that does resolve.
#[tokio::test]
async fn a_value_naming_no_resource_is_refused_with_that_resources_own_code() {
    let ec2 = Ec2::new();

    // A well-formed id of each kind that this account holds nothing under.
    let unknown_fleet = "crf-0a1b2c3d4e5f60718";
    let unknown_offering = "hro-0a1b2c3d4e5f60718";
    let unknown_allocation = "eipalloc-0a1b2c3d4e5f60718";
    // A term no Dedicated Host reservation is offered for: "the number of seconds specified
    // must be the number of seconds in a year (365x24x60x60) times one of the supported
    // durations (1 or 3)" (`DescribeHostReservationOfferingsRequest$MinDuration`).
    let bad_duration = (SECONDS_PER_RESERVATION_YEAR / 2).to_string();

    // (case, action, parameters, code, message)
    type RefusalRow<'a> = (
        &'a str,
        &'a str,
        Vec<(String, String)>,
        &'a str,
        String,
    );
    let rows: Vec<RefusalRow<'_>> = vec![
        (
            "a Capacity Reservation Fleet id the account holds no Fleet under",
            "CancelCapacityReservationFleets",
            vec![p("CapacityReservationFleetId.1", unknown_fleet)],
            "InvalidCapacityReservationFleetId.NotFound",
            format!("The Capacity Reservation Fleet ID '{unknown_fleet}' does not exist"),
        ),
        (
            "an offering id this catalog does not publish",
            "DescribeHostReservationOfferings",
            vec![p("OfferingId", unknown_offering)],
            "InvalidHostReservationOfferingId.Malformed",
            format!("The specified host reservation Offering ID '{unknown_offering}' is not valid."),
        ),
        (
            "an allocation id the account holds no address under",
            "DisableAddressTransfer",
            vec![p("AllocationId", unknown_allocation)],
            "InvalidElasticIpID.NotFound",
            format!("The allocation ID '{unknown_allocation}' does not exist"),
        ),
        (
            "a minimum duration that is not a term reservations are offered for",
            "DescribeHostReservationOfferings",
            vec![p("MinDuration", bad_duration.clone())],
            "InvalidParameterValue",
            format!(
                "Invalid value '{bad_duration}' for minDuration. It must be the number of \
                 seconds in a year ({SECONDS_PER_RESERVATION_YEAR}) times one of the \
                 supported durations (1 or 3)"
            ),
        ),
        (
            "a maximum duration that is not a term reservations are offered for",
            "DescribeHostReservationOfferings",
            vec![p("MaxDuration", bad_duration.clone())],
            "InvalidParameterValue",
            format!(
                "Invalid value '{bad_duration}' for maxDuration. It must be the number of \
                 seconds in a year ({SECONDS_PER_RESERVATION_YEAR}) times one of the \
                 supported durations (1 or 3)"
            ),
        ),
    ];

    for (case, action, params, code, message) in rows {
        let (status, body) = ec2.call(action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got body {body}"
        );
    }

    // The positive rows: every term reservations ARE offered for is served, and an offering
    // id this catalog publishes resolves to that one offering.
    for years in HOST_RESERVATION_TERM_YEARS {
        let seconds = (years * SECONDS_PER_RESERVATION_YEAR).to_string();
        let case = format!("the {years}-year term");
        let body = ec2
            .ok(
                &case,
                "DescribeHostReservationOfferings",
                &[p("MinDuration", seconds.clone()), p("MaxDuration", seconds)],
            )
            .await;
        let reported = elements(&body, "duration");
        assert!(
            !reported.is_empty(),
            "case {case:?}: a published term must report offerings, got {body}"
        );
        assert!(
            reported
                .iter()
                .all(|duration| duration == &(years * SECONDS_PER_RESERVATION_YEAR).to_string()),
            "case {case:?}: only that term's offerings may be reported, got {reported:?}"
        );
    }

    let case = "an offering id this catalog publishes";
    let body = ec2.ok(case, "DescribeHostReservationOfferings", &[]).await;
    let published = elements(&body, "offeringId");
    let offering_id = published
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the catalog must publish an offering: {body}"))
        .clone();
    let body = ec2
        .ok(
            case,
            "DescribeHostReservationOfferings",
            &[p("OfferingId", offering_id.clone())],
        )
        .await;
    assert_eq!(
        elements(&body, "offeringId"),
        vec![offering_id],
        "case {case:?}: an id that resolves must report that one offering, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a route server endpoint sits in the VPC its route server serves
// ---------------------------------------------------------------------------

/// "An Amazon Web Services-managed component inside a subnet" of the VPC the route server
/// is associated with (`RouteServerEndpoint`): a subnet of that VPC is admitted, and a
/// subnet of any other VPC is refused `InvalidParameterValue` with nothing written.
#[tokio::test]
async fn a_route_server_endpoint_must_sit_in_the_vpc_its_route_server_serves() {
    let ec2 = Ec2::new();

    // Two VPCs, each with a subnet, so the refusal is about which VPC the subnet is in
    // rather than about a subnet that does not exist.
    let mut subnets: Vec<(String, String)> = Vec::new();
    for (index, cidr) in ["10.80.0.0/16", "10.81.0.0/16"].into_iter().enumerate() {
        let case = format!("VPC {index}");
        let body = ec2.ok(&case, "CreateVpc", &[p("CidrBlock", cidr)]).await;
        let vpc_id = element(&body, "vpcId")
            .unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"));
        let subnet_cidr = cidr.replace("0.0/16", "1.0/24");
        let body = ec2
            .ok(
                &case,
                "CreateSubnet",
                &[p("VpcId", vpc_id.clone()), p("CidrBlock", subnet_cidr)],
            )
            .await;
        let subnet_id = element(&body, "subnetId")
            .unwrap_or_else(|| panic!("case {case:?}: no subnetId in {body}"));
        subnets.push((vpc_id, subnet_id));
    }
    let (served_vpc, served_subnet) = subnets[0].clone();
    let (other_vpc, other_subnet) = subnets[1].clone();

    let body = ec2
        .ok("a route server", "CreateRouteServer", &[p("AmazonSideAsn", "65030")])
        .await;
    let route_server_id =
        element(&body, "routeServerId").expect("CreateRouteServer reports the route server");
    let _ = ec2
        .ok(
            "the association",
            "AssociateRouteServer",
            &[
                p("RouteServerId", route_server_id.clone()),
                p("VpcId", served_vpc.clone()),
            ],
        )
        .await;

    let case = "a subnet of another VPC";
    let (status, body) = ec2
        .call(
            "CreateRouteServerEndpoint",
            &[
                p("RouteServerId", route_server_id.clone()),
                p("SubnetId", other_subnet.clone()),
            ],
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from CreateRouteServerEndpoint, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got body {body}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "Invalid value '{other_subnet}' for subnetId. The subnet is not in vpc \
             '{served_vpc}', which route server '{route_server_id}' is associated with"
        ),
        "case {case:?}: expected the sentence naming both networks, got body {body}"
    );
    let reported = ec2
        .ok(case, "DescribeRouteServerEndpoints", &[])
        .await;
    assert!(
        elements(&reported, "routeServerEndpointId").is_empty(),
        "case {case:?}: a refused create must write no endpoint, got {reported}"
    );
    assert!(
        !other_vpc.is_empty(),
        "the other VPC must have been created for this row to mean anything"
    );

    let case = "a subnet of the VPC the route server serves";
    let body = ec2
        .ok(
            case,
            "CreateRouteServerEndpoint",
            &[
                p("RouteServerId", route_server_id.clone()),
                p("SubnetId", served_subnet.clone()),
            ],
        )
        .await;
    let endpoint_id = element(&body, "routeServerEndpointId").unwrap_or_else(|| {
        panic!("case {case:?}: the create must report the endpoint it minted: {body}")
    });
    let reported = ec2
        .ok(case, "DescribeRouteServerEndpoints", &[])
        .await;
    assert_eq!(
        elements(&reported, "routeServerEndpointId"),
        vec![endpoint_id],
        "case {case:?}: the endpoint must read back through the describe, got {reported}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: private DNS is admitted only while the VPC can resolve it
// ---------------------------------------------------------------------------

/// "To use a private hosted zone, you must set the following VPC attributes to true:
/// enableDnsHostnames and enableDnsSupport" (`CreateVpcEndpointRequest$PrivateDnsEnabled`),
/// and the member's own default is ON for a service that publishes a private DNS name — so
/// the rows span both VPC attribute settings and all three things a request can say about
/// the member.
#[tokio::test]
async fn an_interface_endpoints_private_dns_follows_the_vpcs_attributes() {
    // (case, DNS hostnames on, what the request says about PrivateDnsEnabled, expected
    //  status, the privateDnsEnabled the created endpoint reports)
    type PrivateDnsRow = (&'static str, bool, Option<bool>, u16, Option<&'static str>);
    let rows: [PrivateDnsRow; 6] = [
        (
            "a VPC without DNS hostnames, saying nothing",
            false,
            None,
            400,
            None,
        ),
        (
            "a VPC without DNS hostnames, asking for private DNS",
            false,
            Some(true),
            400,
            None,
        ),
        (
            "a VPC without DNS hostnames, asking for no private DNS",
            false,
            Some(false),
            200,
            Some("false"),
        ),
        (
            "a VPC with DNS hostnames, saying nothing",
            true,
            None,
            200,
            Some("true"),
        ),
        (
            "a VPC with DNS hostnames, asking for private DNS",
            true,
            Some(true),
            200,
            Some("true"),
        ),
        (
            "a VPC with DNS hostnames, asking for no private DNS",
            true,
            Some(false),
            200,
            Some("false"),
        ),
    ];

    for (case, hostnames, asked, expected_status, expected_report) in rows {
        let ec2 = Ec2::new();
        let body = ec2
            .ok(case, "CreateVpc", &[p("CidrBlock", "10.90.0.0/16")])
            .await;
        let vpc_id =
            element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"));
        let body = ec2
            .ok(
                case,
                "CreateSubnet",
                &[p("VpcId", vpc_id.clone()), p("CidrBlock", "10.90.1.0/24")],
            )
            .await;
        let subnet_id = element(&body, "subnetId")
            .unwrap_or_else(|| panic!("case {case:?}: no subnetId in {body}"));
        if hostnames {
            let _ = ec2
                .ok(
                    case,
                    "ModifyVpcAttribute",
                    &[
                        p("VpcId", vpc_id.clone()),
                        p("EnableDnsHostnames.Value", "true"),
                    ],
                )
                .await;
        }

        let mut params = vec![
            p("VpcId", vpc_id.clone()),
            p("ServiceName", format!("com.amazonaws.{REGION}.logs")),
            p("VpcEndpointType", "Interface"),
            p("SubnetId.1", subnet_id.clone()),
        ];
        if let Some(asked) = asked {
            params.push(p("PrivateDnsEnabled", if asked { "true" } else { "false" }));
        }
        let (status, body) = ec2.call("CreateVpcEndpoint", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from CreateVpcEndpoint, got \
             {status} with body {body}"
        );
        match expected_report {
            None => {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameter",
                    "case {case:?}: expected InvalidParameter, got body {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!(
                        "Enabling private DNS requires both enableDnsSupport and \
                         enableDnsHostnames VPC attributes set to true for {vpc_id}"
                    ),
                    "case {case:?}: expected the sentence naming both attributes, got {body}"
                );
                let reported = ec2.ok(case, "DescribeVpcEndpoints", &[]).await;
                assert!(
                    elements(&reported, "vpcEndpointId").is_empty(),
                    "case {case:?}: a refused create must write no endpoint, got {reported}"
                );
            }
            Some(reported) => {
                assert_eq!(
                    element(&body, "privateDnsEnabled").as_deref(),
                    Some(reported),
                    "case {case:?}: expected privateDnsEnabled {reported:?}, got {body}"
                );
                let read_back = ec2.ok(case, "DescribeVpcEndpoints", &[]).await;
                assert_eq!(
                    element(&read_back, "privateDnsEnabled").as_deref(),
                    Some(reported),
                    "case {case:?}: the endpoint must read back with the same private DNS \
                     association, got {read_back}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: a withdrawn address transfer is no transfer of the account
// ---------------------------------------------------------------------------

/// "Disables Elastic IP address transfer" (`DisableAddressTransfer`): the call answers the
/// offer it withdrew, and the account then has no transfer of that address to report — while
/// an address whose offer still stands is reported, and an address that never carried a
/// reverse DNS record has none to reset.
#[tokio::test]
async fn withdrawing_an_address_transfer_leaves_no_transfer_to_report() {
    let ec2 = Ec2::new();

    // Two addresses, so the describe below is narrowed by what each one carries rather than
    // by the account holding exactly one address.
    let mut addresses: Vec<(String, String)> = Vec::new();
    for index in 0..2 {
        let case = format!("address {index}");
        let body = ec2.ok(&case, "AllocateAddress", &[]).await;
        let allocation_id = element(&body, "allocationId")
            .unwrap_or_else(|| panic!("case {case:?}: no allocationId in {body}"));
        let public_ip = element(&body, "publicIp")
            .unwrap_or_else(|| panic!("case {case:?}: no publicIp in {body}"));
        addresses.push((allocation_id, public_ip));
    }
    let (withdrawn_id, _) = addresses[0].clone();
    let (standing_id, _) = addresses[1].clone();

    for (allocation_id, _) in &addresses {
        let _ = ec2
            .ok(
                "the offer",
                "EnableAddressTransfer",
                &[
                    p("AllocationId", allocation_id.clone()),
                    p("TransferAccountId", "111111111111"),
                ],
            )
            .await;
    }
    let reported = ec2.ok("both offers", "DescribeAddressTransfers", &[]).await;
    assert_eq!(
        elements(&reported, "allocationId").len(),
        2,
        "both offers must be reported while they stand, got {reported}"
    );

    let body = ec2
        .ok(
            "the withdrawal",
            "DisableAddressTransfer",
            &[p("AllocationId", withdrawn_id.clone())],
        )
        .await;
    assert_eq!(
        element(&body, "addressTransferStatus").as_deref(),
        Some("disabled"),
        "the withdrawal answers the offer it took back, got {body}"
    );

    let reported = ec2
        .ok("after the withdrawal", "DescribeAddressTransfers", &[])
        .await;
    assert_eq!(
        elements(&reported, "allocationId"),
        vec![standing_id.clone()],
        "a withdrawn offer is no transfer of the account, got {reported}"
    );

    // And the address whose offer was withdrawn can be offered again, because the record is
    // gone rather than merely marked.
    let body = ec2
        .ok(
            "a second offer",
            "EnableAddressTransfer",
            &[
                p("AllocationId", withdrawn_id.clone()),
                p("TransferAccountId", "222222222222"),
            ],
        )
        .await;
    assert_eq!(
        element(&body, "addressTransferStatus").as_deref(),
        Some("pending"),
        "a fresh offer on the same address must stand, got {body}"
    );
}

/// An address that never carried a reverse DNS record has none to reset: "resets the
/// attribute of the specified IP address" acts on the record the address carries, so a reset
/// of an address with no record is refused and the one with a record is served.
#[tokio::test]
async fn resetting_a_reverse_dns_record_an_address_never_had_is_refused() {
    let ec2 = Ec2::new();
    let body = ec2.ok("an address", "AllocateAddress", &[]).await;
    let allocation_id =
        element(&body, "allocationId").expect("AllocateAddress reports the allocation");
    let public_ip = element(&body, "publicIp").expect("AllocateAddress reports the address");

    let case = "an address that never carried a record";
    let (status, body) = ec2
        .call(
            "ResetAddressAttribute",
            &[
                p("AllocationId", allocation_id.clone()),
                p("Attribute", "domain-name"),
            ],
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from ResetAddressAttribute, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidAllocationID.NotFound",
        "case {case:?}: expected the not-found code, got body {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("No ptr record found for {public_ip} ."),
        "case {case:?}: expected the sentence naming the address, got body {body}"
    );

    // Once the address carries a record, the reset is admitted and takes it away.
    let case = "an address carrying a record";
    let _ = ec2
        .ok(
            case,
            "ModifyAddressAttribute",
            &[
                p("AllocationId", allocation_id.clone()),
                p("DomainName", "mail.example.com"),
            ],
        )
        .await;
    // The record the modify started is in effect once the clock has applied it — read until
    // the address carries no outstanding update, rather than waiting a span of this test's
    // choosing, because one record takes at most one outstanding request at a time.
    let deadline = tokio::time::Instant::now() + MOVE_DEADLINE;
    loop {
        let reported = ec2
            .ok(
                case,
                "DescribeAddressesAttribute",
                &[p("AllocationId.1", allocation_id.clone())],
            )
            .await;
        if element(&reported, "status").is_none() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the record the modify set must come due, last read {reported}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let body = ec2
        .ok(
            case,
            "ResetAddressAttribute",
            &[
                p("AllocationId", allocation_id.clone()),
                p("Attribute", "domain-name"),
            ],
        )
        .await;
    assert_eq!(
        element(&body, "allocationId").as_deref(),
        Some(allocation_id.as_str()),
        "case {case:?}: the reset answers the address it acted on, got {body}"
    );
}
