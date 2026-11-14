// SPDX-License-Identifier: CC0-1.0

//! Classic Elastic Load Balancing and Elastic Load Balancing v2 are TWO emulated
//! services behind ONE endpoint.
//!
//! Both models declare `endpointPrefix` / `arnNamespace` / `aws.auth#sigv4` name
//! `elasticloadbalancing` (`smithy_query(query="service_info")` on either), differing only
//! in `sdkId` and in the API version they declare (`2012-06-01` and `2015-12-01`), and
//! every AWS SDK for either API signs that one name. With the generated signing-name claim
//! on both, the router found TWO claimants for every Elastic Load Balancing request and
//! answered `500 AmbiguousService` — a code that exists in no AWS model, in a JSON body an
//! awsQuery client cannot parse — so neither API was reachable at all.
//!
//! The rules this suite pins:
//!
//! * a signing name shared by several emulated services resolves to EXACTLY ONE of them —
//!   here the Classic model, whose own identity IS that name;
//! * that front door serves BOTH API versions, as AWS's one Elastic Load Balancing
//!   endpoint does: the awsQuery envelope's `Version` names the API, so a `2012-06-01`
//!   request is answered out of the Classic model and a `2015-12-01` one out of the ELBv2
//!   model — which matters because TEN action names are declared by both models with
//!   different inputs and outputs — while a pair neither model resolves is refused `400
//!   InvalidAction` naming the action and the version;
//! * the ELBv2 model stays reachable under the routing key that names IT
//!   (`elasticloadbalancingv2`), by credential scope or by host label, and both front doors
//!   read and write the ONE ELBv2 store;
//! * whatever the answer is, it is the serving model's own awsQuery XML and never a
//!   router-level refusal.
//!
//! The rows run IN ORDER against one router over one engine, so a later row reads what an
//! earlier one wrote: each API creates a load balancer and reads it back, which the
//! ambiguity refusal made impossible — the request never reached a handler at all.
//!
//! This suite needs the WHOLE router: the ambiguity exists only when both services are
//! mounted, so a single-service test cannot see it. Assembling the router costs stack
//! (see `boot_serve.rs`), so it is built — and the requests driven — on an explicitly
//! sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The EC2 API version an ec2Query body carries, for the two subnets this suite places a
/// load balancer in.
const EC2_VERSION: &str = "2016-11-15";

/// The parameter values the rows below name their subnets by. The ids are EC2's to mint,
/// and this table is `'static`, so the driver substitutes the subnets the suite created
/// through EC2 for these two placeholders.
const FIRST_SUBNET: &str = "@subnet-1";
const SECOND_SUBNET: &str = "@subnet-2";

/// The same for the VPC and for a target group's ARN, which are the account's and Elastic
/// Load Balancing's to mint: the driver substitutes the VPC the suite created through EC2,
/// and the ARN the row named by [`HEALTH_GROUP_CASE`] was answered.
const VPC_ID: &str = "@vpc-id";
const TARGET_GROUP_ARN: &str = "@target-group-arn";

/// The row whose answer names the target group the target-health rows act on. Its ARN is
/// what [`TARGET_GROUP_ARN`] stands for.
const HEALTH_GROUP_CASE: &str = "elbv2 CreateTargetGroup for an ip target in the suite's VPC";

/// A well-formed target group ARN this emulator holds no target group for, for the row that
/// pins the health read's own modelled refusal.
const ABSENT_TARGET_GROUP_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:000000000000:targetgroup/router-no-such-tg/0123456789abcdef";

/// The API version the Classic model declares.
const CLASSIC_VERSION: &str = "2012-06-01";

/// The API version the ELBv2 model declares.
const V2_VERSION: &str = "2015-12-01";

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// How a request names the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 credential scope naming this routing key — the signal the router reads
    /// first, and what every AWS SDK sends.
    Signed(&'static str),
    /// A host-style endpoint and no credentials at all: the last routing signal, which an
    /// anonymous request still carries.
    Host(&'static str),
}

/// One call: how it is addressed, the awsQuery `Action` and `Version` it names, and what
/// the ONE model that owns that pair must answer.
struct Call {
    case: &'static str,
    addressed: Addressed,
    action: &'static str,
    /// The `Version` the envelope carries, or `None` to send no `Version` field at all.
    version: Option<&'static str>,
    /// The form parameters beyond `Action` and `Version`, form-encoded into the body by the
    /// driver — as the ec2 setup calls beside them are — so a value carrying the `:` and `/`
    /// of an ARN crosses the wire intact.
    params: &'static [(&'static str, &'static str)],
    expected_status: u16,
    /// The awsQuery `<Code>` the answer must carry, when the row pins one.
    expected_code: Option<&'static str>,
    /// Elements the answer must carry, for a row that pins WHAT was reported rather than
    /// only the status.
    expected_body_contains: &'static [&'static str],
}

#[test]
fn classic_elb_and_elbv2_route_apart_under_one_signing_name() {
    std::thread::Builder::new()
        .name("elb-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                // See the sibling `elbv2_subnet_vpc_membership` suite: the IO and TIME
                // drivers a service's background work needs, without which tokio panics.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // Two subnets in different Availability Zones, created through EC2 — which
            // owns them — because the ELBv2 `CreateLoadBalancer` rows below are placements:
            // the subnets a request names are resolved against EC2, which refuses an id the
            // account holds no subnet for, and an Application Load Balancer must be placed
            // in at least two zones.
            let vpc = element(
                &ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", "10.60.0.0/16")]),
                "vpcId",
            );
            let subnets: Vec<String> = [("10.60.1.0/24", "us-east-1a"), ("10.60.2.0/24", "us-east-1b")]
                .into_iter()
                .map(|(cidr, zone)| {
                    element(
                        &ec2(
                            &router,
                            &runtime,
                            "CreateSubnet",
                            &[
                                ("VpcId", vpc.as_str()),
                                ("CidrBlock", cidr),
                                ("AvailabilityZone", zone),
                            ],
                        ),
                        "subnetId",
                    )
                })
                .collect();

            // The rows differ along all three axes that decide what answers: how the
            // request is addressed (the shared signing name, the ELBv2 model's own key, a
            // host-style endpoint instead of a credential scope), the `Version` it names
            // (this model's, the other model's, none, and one neither declares), and the
            // action — one only the Classic model declares, one only the ELBv2 model
            // declares, and `CreateLoadBalancer`/`DescribeLoadBalancers`, which BOTH
            // declare with different inputs and outputs.
            let calls: [Call; 29] = [
                Call {
                    // The request the issue reports as `500 AmbiguousService`: a Classic
                    // DescribeLoadBalancers on the shared signing name.
                    case: "classic DescribeLoadBalancers with nothing created",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeLoadBalancers",
                    version: Some(CLASSIC_VERSION),
                    params: &[],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<DescribeLoadBalancersResponse",
                        "<DescribeLoadBalancersResult>",
                        "<LoadBalancerDescriptions",
                    ],
                },
                Call {
                    case: "classic CreateLoadBalancer",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateLoadBalancer",
                    version: Some(CLASSIC_VERSION),
                    params: &[
                        ("LoadBalancerName", "router-classic-elb"),
                        ("AvailabilityZones.member.1", "us-east-1a"),
                        ("Listeners.member.1.Protocol", "HTTP"),
                        ("Listeners.member.1.LoadBalancerPort", "80"),
                        ("Listeners.member.1.InstancePort", "8080"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    // The Classic model's own `CreateAccessPointOutput`: the result element
                    // and the one member it carries. The VALUE of that member is the
                    // address this emulator answers the load balancer's traffic on, which
                    // is the Classic crate's own suite to pin (`classic_data_plane.rs`);
                    // what this suite is about is WHICH model answered.
                    expected_body_contains: &["<CreateLoadBalancerResult>", "<DNSName>"],
                },
                Call {
                    // …and the read back, which reports the record the create persisted.
                    case: "classic DescribeLoadBalancers reads the created load balancer back",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeLoadBalancers",
                    version: Some(CLASSIC_VERSION),
                    params: &[("LoadBalancerNames.member.1", "router-classic-elb")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<LoadBalancerName>router-classic-elb</LoadBalancerName>",
                        "<LoadBalancerPort>80</LoadBalancerPort>",
                    ],
                },
                Call {
                    case: "classic ConfigureHealthCheck",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "ConfigureHealthCheck",
                    version: Some(CLASSIC_VERSION),
                    params: &[
                        ("LoadBalancerName", "router-classic-elb"),
                        ("HealthCheck.Target", "HTTP:8080/health"),
                        ("HealthCheck.Interval", "10"),
                        ("HealthCheck.Timeout", "5"),
                        ("HealthCheck.UnhealthyThreshold", "2"),
                        ("HealthCheck.HealthyThreshold", "3"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<Target>HTTP:8080/health</Target>"],
                },
                Call {
                    case: "classic DescribeLoadBalancerAttributes reports the defaults",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeLoadBalancerAttributes",
                    version: Some(CLASSIC_VERSION),
                    params: &[("LoadBalancerName", "router-classic-elb")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<IdleTimeout>60</IdleTimeout>",
                        "elb.http.desyncmitigationmode",
                    ],
                },
                Call {
                    case: "classic DeleteLoadBalancer",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DeleteLoadBalancer",
                    version: Some(CLASSIC_VERSION),
                    params: &[("LoadBalancerName", "router-classic-elb")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<DeleteLoadBalancerResponse"],
                },
                Call {
                    // The documented idempotent repeat: the model declares no error for
                    // this operation, so deleting a name nothing holds succeeds too.
                    case: "classic DeleteLoadBalancer repeated",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DeleteLoadBalancer",
                    version: Some(CLASSIC_VERSION),
                    params: &[("LoadBalancerName", "router-classic-elb")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<DeleteLoadBalancerResponse"],
                },
                Call {
                    // The ELBv2 model, addressed by the routing key that names IT: the
                    // SAME action name as the Classic create above, with its own input
                    // members and its own `<LoadBalancers>` output.
                    case: "elbv2 CreateLoadBalancer addressed by the ELBv2 key",
                    addressed: Addressed::Signed("elasticloadbalancingv2"),
                    action: "CreateLoadBalancer",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-v2-elb"),
                        ("Subnets.member.1", FIRST_SUBNET),
                        // Two subnets, because an Application Load Balancer "must specify
                        // subnets from at least two Availability Zones".
                        ("Subnets.member.2", SECOND_SUBNET),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<CreateLoadBalancerResponse",
                        "<LoadBalancers>",
                        "<LoadBalancerName>router-v2-elb</LoadBalancerName>",
                    ],
                },
                Call {
                    case: "elbv2 CreateTargetGroup addressed by the ELBv2 key",
                    addressed: Addressed::Signed("elasticloadbalancingv2"),
                    action: "CreateTargetGroup",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-v2-tg"),
                        ("Protocol", "HTTP"),
                        ("Port", "80"),
                        ("VpcId", "vpc-12345678"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<TargetGroupName>router-v2-tg</TargetGroupName>"],
                },
                Call {
                    // The target group the previous row created, read back: the ELBv2
                    // model is not just reachable, it is serving its own state.
                    case: "elbv2 DescribeTargetGroups reads the created target group back",
                    addressed: Addressed::Signed("elasticloadbalancingv2"),
                    action: "DescribeTargetGroups",
                    version: Some(V2_VERSION),
                    params: &[("Names.member.1", "router-v2-tg")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<TargetGroupName>router-v2-tg</TargetGroupName>",
                        "<VpcId>vpc-12345678</VpcId>",
                    ],
                },
                Call {
                    // A target group for `ip` targets in the VPC the suite created through
                    // EC2, which is what lets the registration below resolve the address it
                    // names against that VPC's subnets.
                    case: HEALTH_GROUP_CASE,
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateTargetGroup",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-health-tg"),
                        ("Protocol", "HTTP"),
                        ("Port", "80"),
                        ("VpcId", VPC_ID),
                        ("TargetType", "ip"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<TargetGroupName>router-health-tg</TargetGroupName>",
                        "<TargetGroupArn>",
                    ],
                },
                Call {
                    // Targets registered with that group through the SHARED signing name,
                    // which is the only name an AWS SDK ELBv2 client signs: the write half
                    // of the deploy-then-verify loop a client runs against a target group.
                    case: "elbv2 RegisterTargets on the shared signing name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "RegisterTargets",
                    version: Some(V2_VERSION),
                    params: &[
                        ("TargetGroupArn", TARGET_GROUP_ARN),
                        // An address inside the first subnet the suite created
                        // (`10.60.1.0/24`, `us-east-1a`), on a port of its own.
                        ("Targets.member.1.Id", "10.60.1.17"),
                        ("Targets.member.1.Port", "8080"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<RegisterTargetsResponse"],
                },
                Call {
                    // …and the read half, the action the issue behind these rows reports as
                    // `InvalidAction`: the health of that group's targets, reported from
                    // what the registration above persisted — the address, the port it was
                    // registered on and the zone of the subnet that holds it — in this
                    // model's own document.
                    case: "elbv2 DescribeTargetHealth on the shared signing name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetHealth",
                    version: Some(V2_VERSION),
                    params: &[("TargetGroupArn", TARGET_GROUP_ARN)],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<DescribeTargetHealthResponse",
                        "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/",
                        "<Id>10.60.1.17</Id>",
                        "<Port>8080</Port>",
                        "<AvailabilityZone>us-east-1a</AvailabilityZone>",
                        "<TargetHealth>",
                    ],
                },
                Call {
                    // The same action for a target group nothing holds: the operation's OWN
                    // modelled refusal (`TargetGroupNotFound`, HTTP 400), which is what
                    // tells a client the group is absent rather than the action.
                    case: "elbv2 DescribeTargetHealth for a target group nothing holds",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetHealth",
                    version: Some(V2_VERSION),
                    params: &[("TargetGroupArn", ABSENT_TARGET_GROUP_ARN)],
                    expected_status: 400,
                    expected_code: Some("TargetGroupNotFound"),
                    expected_body_contains: &["<ErrorResponse", ABSENT_TARGET_GROUP_ARN],
                },
                Call {
                    // The same action for a target group nothing holds: the operation's OWN
                    // modelled refusal (`TargetGroupNotFound`, HTTP 400), which is what
                    // tells a client the group is absent rather than the action.
                    case: "elbv2 DescribeTargetHealth for a target group nothing holds",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetHealth",
                    version: Some(V2_VERSION),
                    params: &[("TargetGroupArn", ABSENT_TARGET_GROUP_ARN)],
                    expected_status: 400,
                    expected_code: Some("TargetGroupNotFound"),
                    expected_body_contains: &["<ErrorResponse", ABSENT_TARGET_GROUP_ARN],
                },
                Call {
                    // The ELBv2 load balancer created above: the same action name as the
                    // Classic describe, answered from the OTHER model's shapes.
                    case: "elbv2 DescribeLoadBalancers addressed by the ELBv2 key",
                    addressed: Addressed::Signed("elasticloadbalancingv2"),
                    action: "DescribeLoadBalancers",
                    version: Some(V2_VERSION),
                    params: &[("Names.member.1", "router-v2-elb")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<LoadBalancerName>router-v2-elb</LoadBalancerName>",
                        "<LoadBalancerArn>",
                    ],
                },
                Call {
                    // An anonymous request addressed at the ELBv2 host: the routing key
                    // comes from the host label instead of a credential scope, and reaches
                    // that model too.
                    case: "elbv2 DescribeTargetGroups addressed by host",
                    addressed: Addressed::Host("elasticloadbalancingv2.us-east-1.amazonaws.com"),
                    action: "DescribeTargetGroups",
                    version: Some(V2_VERSION),
                    params: &[("Names.member.1", "router-v2-tg")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<TargetGroupName>router-v2-tg</TargetGroupName>"],
                },
                Call {
                    // An anonymous request addressed at the Elastic Load Balancing
                    // endpoint host: the routing key comes from the host label instead of
                    // a credential scope, and must still resolve to the one owner.
                    case: "classic DescribeLoadBalancers addressed by host",
                    addressed: Addressed::Host("elasticloadbalancing.us-east-1.amazonaws.com"),
                    action: "DescribeLoadBalancers",
                    version: Some(CLASSIC_VERSION),
                    params: &[],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &["<DescribeLoadBalancersResponse"],
                },
                Call {
                    // The ELBv2 API version on the SHARED name, for one of the ten action
                    // names BOTH models declare: served out of the model whose version
                    // the envelope names, in that model's own document — the ELBv2
                    // `<LoadBalancers>` output and the `2015-12-01` namespace, not this
                    // front door's own shapes.
                    case: "the ELBv2 API version on the shared signing name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateLoadBalancer",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-shared-v2-elb"),
                        ("Subnets.member.1", FIRST_SUBNET),
                        ("Subnets.member.2", SECOND_SUBNET),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<CreateLoadBalancerResponse",
                        "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/",
                        "<LoadBalancers>",
                        "<LoadBalancerName>router-shared-v2-elb</LoadBalancerName>",
                    ],
                },
                Call {
                    // The call the issue reports: an action only the ELBv2 model declares,
                    // on the shared signing name every AWS SDK ELBv2 client signs.
                    case: "CreateTargetGroup on the shared signing name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateTargetGroup",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-shared-tg"),
                        ("Protocol", "HTTP"),
                        ("Port", "80"),
                        ("VpcId", "vpc-12345678"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<CreateTargetGroupResponse",
                        "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/",
                        "<TargetGroupName>router-shared-tg</TargetGroupName>",
                    ],
                },
                Call {
                    // The documented idempotent repeat — "if you attempt to create
                    // multiple target groups with the same settings, each call succeeds" —
                    // reported as the stored group, through the same front door.
                    case: "the idempotent CreateTargetGroup replay on the shared name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateTargetGroup",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-shared-tg"),
                        ("Protocol", "HTTP"),
                        ("Port", "80"),
                        ("VpcId", "vpc-12345678"),
                    ],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<TargetGroupName>router-shared-tg</TargetGroupName>",
                        "<VpcId>vpc-12345678</VpcId>",
                    ],
                },
                Call {
                    // A repeat of the same NAME with different settings is the modelled
                    // refusal, as its `awsQueryError` code.
                    case: "a conflicting CreateTargetGroup replay on the shared name",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "CreateTargetGroup",
                    version: Some(V2_VERSION),
                    params: &[
                        ("Name", "router-shared-tg"),
                        ("Protocol", "HTTPS"),
                        ("Port", "80"),
                        ("VpcId", "vpc-12345678"),
                    ],
                    expected_status: 400,
                    expected_code: Some("DuplicateTargetGroupName"),
                    expected_body_contains: &["<ErrorResponse"],
                },
                Call {
                    // The two front doors serve ONE store: the group created on the shared
                    // name is read back under the ELBv2 model's own routing key.
                    case: "the ELBv2 key reads back the group created on the shared name",
                    addressed: Addressed::Signed("elasticloadbalancingv2"),
                    action: "DescribeTargetGroups",
                    version: Some(V2_VERSION),
                    params: &[("Names.member.1", "router-shared-tg")],
                    expected_status: 200,
                    expected_code: None,
                    expected_body_contains: &[
                        "<TargetGroupName>router-shared-tg</TargetGroupName>",
                        "<VpcId>vpc-12345678</VpcId>",
                    ],
                },
                Call {
                    // No `Version` at all, naming an action only the ELBv2 model declares:
                    // a request that states no API version is served as this front door's
                    // own, which has no such operation, so its own routing refusal answers
                    // — in its awsQuery envelope, not the router's JSON.
                    case: "an ELBv2-only action on the shared name with no Version field",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetGroups",
                    version: None,
                    params: &[("Names.member.1", "router-v2-tg")],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &["<ErrorResponse", CLASSIC_VERSION],
                },
                Call {
                    // The mirror of the row above: an action only the ELBv2 model
                    // declares, asked for at the CLASSIC version. The version names this
                    // front door's own model, which has no such operation, so the pair
                    // resolves to no operation either way round.
                    case: "an ELBv2-only action at the Classic version",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetGroups",
                    version: Some(CLASSIC_VERSION),
                    params: &[("Names.member.1", "router-shared-tg")],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    // The refusal names BOTH halves of the pair it could not resolve.
                    expected_body_contains: &[
                        "<ErrorResponse",
                        "DescribeTargetGroups",
                        CLASSIC_VERSION,
                    ],
                },
                Call {
                    // A Classic action asked for at the ELBv2 version: the version names
                    // the ELBv2 model, which declares no such action, so the pair names no
                    // operation at all — refused by that model, in its own envelope.
                    case: "a Classic-only action at the ELBv2 version",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "ConfigureHealthCheck",
                    version: Some(V2_VERSION),
                    params: &[("LoadBalancerName", "router-classic-elb")],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    // The refusal names BOTH halves of the pair it could not resolve.
                    expected_body_contains: &["<ErrorResponse", "ConfigureHealthCheck", V2_VERSION],
                },
                Call {
                    // A version no Elastic Load Balancing model declares: refused the same
                    // way, naming the pair it could not resolve.
                    case: "an API version neither model declares",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeLoadBalancers",
                    version: Some("2099-01-01"),
                    params: &[],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &["<ErrorResponse", "2099-01-01"],
                },
                Call {
                    // An action NEITHER model declares, at this front door's own version:
                    // the version resolves to the Classic model, whose routing table has
                    // no such operation, so the pair resolves to nothing — the one case
                    // `InvalidAction` is actually reserved for.
                    case: "an action neither model declares, at the Classic version",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetGardens",
                    version: Some(CLASSIC_VERSION),
                    params: &[],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &[
                        "<ErrorResponse",
                        "DescribeTargetGardens",
                        CLASSIC_VERSION,
                    ],
                },
                Call {
                    // The same unknown action at the ELBv2 version: the other model
                    // resolves it to nothing in exactly the same way.
                    case: "an action neither model declares, at the ELBv2 version",
                    addressed: Addressed::Signed("elasticloadbalancing"),
                    action: "DescribeTargetGardens",
                    version: Some(V2_VERSION),
                    params: &[],
                    expected_status: 400,
                    expected_code: Some("InvalidAction"),
                    expected_body_contains: &[
                        "<ErrorResponse",
                        "DescribeTargetGardens",
                        V2_VERSION,
                    ],
                },
            ];

            // The ARN of the first target group a row creates, which the rows that act on
            // that group name by placeholder: Elastic Load Balancing mints its id, so no
            // `'static` table can carry it.
            let mut target_group_arn = String::new();

            for call in calls {
                let mut body = format!("Action={}", call.action);
                if let Some(version) = call.version {
                    body.push_str(&format!("&Version={version}"));
                }
                for (name, value) in call.params {
                    // A row names a subnet by placeholder, because EC2 minted the id.
                    let value = match *value {
                        FIRST_SUBNET => subnets[0].as_str(),
                        SECOND_SUBNET => subnets[1].as_str(),
                        VPC_ID => vpc.as_str(),
                        TARGET_GROUP_ARN => target_group_arn.as_str(),
                        literal => literal,
                    };
                    body.push_str(&format!("&{name}={}", form_encode(value)));
                }
                let mut builder = http::Request::builder()
                    .method("POST")
                    .uri("http://localhost/")
                    .header("content-type", "application/x-www-form-urlencoded");
                builder = match call.addressed {
                    Addressed::Signed(signing_name) => builder
                        .header("host", "localhost")
                        .header("authorization", authorization(signing_name)),
                    Addressed::Host(host) => builder.header("host", host),
                };
                let request = builder
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let case = call.case;

                // Neither routing failure may happen: `AmbiguousService` (500, several
                // claimants) nor `UnknownService` (404, none).
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got \
                     {status} with {rendered}"
                );
                // Whatever the answer is, it is an awsQuery envelope — XML, which a
                // query-protocol client can parse — and never the router's JSON `__type`
                // document.
                assert!(
                    rendered.trim_start().starts_with('<') && !rendered.contains("__type"),
                    "case {case:?}: an awsQuery client must get XML, got {status} with \
                     {rendered}"
                );
                assert_eq!(
                    status, call.expected_status,
                    "case {case:?}: the owning model must answer HTTP {}, got {status} \
                     with {rendered}",
                    call.expected_status
                );
                if let Some(expected) = call.expected_code {
                    let code = rendered
                        .split_once("<Code>")
                        .and_then(|(_, rest)| rest.split_once("</Code>"))
                        .map(|(code, _)| code.to_string());
                    assert_eq!(
                        code.as_deref(),
                        Some(expected),
                        "case {case:?}: expected the error code {expected}, got {status} \
                         with {rendered}"
                    );
                }
                for expected in call.expected_body_contains {
                    assert!(
                        rendered.contains(expected),
                        "case {case:?}: expected {expected:?} in the owning model's \
                         answer, got {status} with {rendered}"
                    );
                }

                // The target group the health rows act on: its ARN carries the id Elastic
                // Load Balancing minted, so it is read off the create's answer rather than
                // carried by this `'static` table.
                if case == HEALTH_GROUP_CASE {
                    target_group_arn = element(&rendered, "TargetGroupArn");
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("Classic Elastic Load Balancing and ELBv2 must route apart");
}

/// Drive one ec2Query action against the EC2 model and answer its body, which must
/// succeed: these are the suite's SETUP calls, for the network the ELBv2 rows place a load
/// balancer in.
fn ec2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={EC2_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("host", "localhost")
        .header("authorization", authorization("ec2"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {rendered}"
    );
    rendered
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// How a row of the claim table below names the service it is for.
#[derive(Debug, Clone, Copy)]
enum Claiming {
    /// A SigV4 `Authorization` scope naming this routing key.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming `elasticloadbalancing`.
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// The claim table, one layer BELOW the dispatch table above: it asks each service's
/// [`smithy_server_runtime::Service::can_handle`] directly, so a row pins both that the
/// right service claims a request and that NO other does — the condition the router turns
/// into `500 AmbiguousService` when it fails.
///
/// Every awsQuery request is `POST /`: neither model binds a `smithy.api#http` trait, so
/// the path carries no discriminator and the route key decides alone. The rows span the
/// three signals that resolve it, both keys, and a `Host` that agrees with, disagrees with
/// or says nothing about the target.
#[test]
fn exactly_one_service_claims_each_request_for_the_elasticloadbalancing_key() {
    use smithy_server_runtime::Service;

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let v2 =
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            engine,
        );

    /// A presigned credential scope naming the shared signing name, with its `/`
    /// separators encoded.
    const PRESIGNED: &str =
        "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Felasticloadbalancing%2Faws4_request";

    // (case, how the request is addressed, its Host, the services that must claim it, by
    // routing key)
    let rows: [(&str, Claiming, &str, &[&str]); 8] = [
        (
            "a signed request on the Elastic Load Balancing host",
            Claiming::Signed("elasticloadbalancing"),
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            "a signed request on a bare emulator endpoint",
            Claiming::Signed("elasticloadbalancing"),
            "localhost:9297",
            &["elasticloadbalancing"],
        ),
        (
            "a presigned request in the query string",
            Claiming::Presigned,
            "elasticloadbalancing.eu-west-2.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            "an anonymous request addressed to the Elastic Load Balancing host",
            Claiming::Anonymous,
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            "a request signed for the ELBv2 model's own key",
            Claiming::Signed("elasticloadbalancingv2"),
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancingv2"],
        ),
        (
            "an anonymous request addressed to the ELBv2 host",
            Claiming::Anonymous,
            "elasticloadbalancingv2.us-east-1.amazonaws.com",
            &["elasticloadbalancingv2"],
        ),
        (
            // The scope decides, not the host: a request signed for the shared name on
            // the ELBv2 host is the Classic front door's.
            "a signed request on the ELBv2 host: the scope decides",
            Claiming::Signed("elasticloadbalancing"),
            "elasticloadbalancingv2.us-east-1.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            // Claimed by neither, which is the router's `404 UnknownService` and NOT an
            // ambiguity.
            "an anonymous request addressed to an unrelated host",
            Claiming::Anonymous,
            "sqs.us-east-1.amazonaws.com",
            &[],
        ),
    ];

    for (case, claiming, host, expected) in rows {
        let uri = match claiming {
            Claiming::Presigned => format!("/?{PRESIGNED}"),
            _ => "/".to_string(),
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", host);
        if let Claiming::Signed(signing_name) = claiming {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            ("elasticloadbalancing", classic.can_handle(&parts)),
            ("elasticloadbalancingv2", v2.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} (POST / to host {host}, {claiming:?}): expected it to be \
             claimed by exactly {expected:?}, got {claimants:?}"
        );
    }
}
