// SPDX-License-Identifier: CC0-1.0

//! The security groups a load balancer CARRIES, driven with both services on one engine.
//!
//! One rule is under test: a load balancer of a family whose load balancers are always
//! governed by a security group carries one from the moment it exists, so a create that
//! names no `SecurityGroups` is placed in the DEFAULT security group of the VPC its
//! subnets are in — the group EC2 created with that VPC — while a create that names groups
//! carries exactly those, and a family that may carry none reports no `SecurityGroups`
//! member at all (the member is an optional list, so an absent one and an empty one are
//! different answers to whether this load balancer has security groups).
//!
//! Which group is a VPC's default one is EC2's to say, so every row here runs against VPCs
//! created through EC2 itself and compares what `DescribeLoadBalancers` reports against
//! what EC2 answers for `group-name=default` in that VPC. That is why this suite lives
//! here rather than in the ELBv2 crate: with no EC2 to ask, nothing is known about the
//! VPC's groups and none is attached.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. The request plumbing is
//! `elbv2_network_resolution.rs`'s, reused rather than written twice.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{attach_internet_gateway, ec2_ok, elbv2, BOOT_STACK_SIZE};
use crate::query_wire::{block_of, element, members_of};

/// One VPC of the account, with the two subnets a load balancer is placed in and the id EC2
/// reports for its own default security group.
struct Vpc {
    subnets: Vec<String>,
    /// The id EC2 answers for `group-name=default` in this VPC.
    default_group: String,
    /// A group of this VPC a request can name instead.
    named_group: String,
}

/// Create one VPC with two subnets and a group of its own, and read the id EC2 gives its
/// default security group.
///
/// The VPC is given an internet gateway, which an internet-facing load balancer of the rows
/// below needs ([`attach_internet_gateway`]).
fn seed_vpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    label: &str,
    prefix: &str,
) -> Vpc {
    let vpc = element(
        &ec2_ok(
            router,
            runtime,
            "CreateVpc",
            &[("CidrBlock", &format!("{prefix}.0.0/16"))],
        ),
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (index, zone) in ["us-west-2a", "us-west-2b"].into_iter().enumerate() {
        subnets.push(element(
            &ec2_ok(
                router,
                runtime,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", &format!("{prefix}.{}.0/24", index + 1)),
                    ("AvailabilityZone", zone),
                ],
            ),
            "subnetId",
        ));
    }
    // What EC2 itself calls the default group of this VPC — never a value this suite made
    // up, and read through EC2's own filters.
    let default_group = element(
        &ec2_ok(
            router,
            runtime,
            "DescribeSecurityGroups",
            &[
                ("Filter.1.Name", "vpc-id"),
                ("Filter.1.Value.1", vpc.as_str()),
                ("Filter.2.Name", "group-name"),
                ("Filter.2.Value.1", "default"),
            ],
        ),
        "groupId",
    );
    attach_internet_gateway(router, runtime, &vpc);
    let named_group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", &format!("elbv2-groups-{label}")),
                ("GroupDescription", "a group a request names"),
                ("VpcId", vpc.as_str()),
            ],
        ),
        "groupId",
    );
    Vpc {
        subnets,
        default_group,
        named_group,
    }
}

/// The `SecurityGroups` a load balancer reports, in the order it reports them, read back
/// through `DescribeLoadBalancers` rather than off the create's own answer — `None` when it
/// reports no such member at all, which is the answer for a load balancer that has none.
fn reported_groups(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    arn: &str,
) -> Option<Vec<String>> {
    let read = elbv2(
        router,
        runtime,
        "DescribeLoadBalancers",
        &[("LoadBalancerArns.member.1", arn)],
    );
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from the read, got {read:?}"
    );
    block_of(&read.text, "SecurityGroups")
        .map(|block| members_of(block).into_iter().map(str::to_string).collect())
}

/// Which groups a row expects, resolved once both VPCs exist.
enum Expected {
    /// The default security group of the VPC the load balancer is placed in.
    VpcDefault,
    /// The groups the request named, in that order.
    AsNamed,
    /// No `SecurityGroups` member at all, which is what a load balancer with no security
    /// group reports.
    None,
}

/// One row: the case, the family and scheme the load balancer is created with (`None` where
/// the family carries no scheme — "you can't specify a scheme for a Gateway Load Balancer"),
/// how many of the VPC's own groups the request names, and what it must then report.
struct Row {
    case: &'static str,
    lb_type: &'static str,
    scheme: Option<&'static str>,
    /// Whether the request names the VPC's own group (`CreateSecurityGroup`'s id).
    names_group: bool,
    expected: Expected,
}

/// The rows span what the rule generalises over: the family (an Application Load Balancer
/// must have a group; a Network Load Balancer may have none; a Gateway Load Balancer
/// carries none at all), the scheme, and whether the request named a group of its own.
const ROWS: &[Row] = &[
    Row {
        case: "an internet-facing application load balancer naming no group",
        lb_type: "application",
        scheme: Some("internet-facing"),
        names_group: false,
        expected: Expected::VpcDefault,
    },
    Row {
        case: "an internal application load balancer naming no group",
        lb_type: "application",
        scheme: Some("internal"),
        names_group: false,
        expected: Expected::VpcDefault,
    },
    Row {
        case: "an application load balancer naming a group of its own VPC",
        lb_type: "application",
        scheme: Some("internet-facing"),
        names_group: true,
        expected: Expected::AsNamed,
    },
    Row {
        case: "a network load balancer naming no group",
        lb_type: "network",
        scheme: Some("internet-facing"),
        names_group: false,
        expected: Expected::None,
    },
    Row {
        case: "a network load balancer naming a group of its own VPC",
        lb_type: "network",
        scheme: Some("internal"),
        names_group: true,
        expected: Expected::AsNamed,
    },
    Row {
        case: "a gateway load balancer naming no group",
        lb_type: "gateway",
        scheme: None,
        names_group: false,
        expected: Expected::None,
    },
];

#[test]
fn a_load_balancer_carries_the_groups_it_was_created_with_or_its_vpcs_default() {
    std::thread::Builder::new()
        .name("elbv2-load-balancer-security-groups".to_string())
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

            // TWO VPCs, each with its own default group: a load balancer reports the
            // default group of the VPC IT is in, so one constant could not satisfy both.
            let first = seed_vpc(&router, &runtime, "first", "10.70");
            let second = seed_vpc(&router, &runtime, "second", "10.71");
            assert_ne!(
                first.default_group, second.default_group,
                "setup: each VPC has its own default security group, got {:?} twice",
                first.default_group
            );

            for (index, vpc) in [&first, &second].into_iter().enumerate() {
                for (row_index, row) in ROWS.iter().enumerate() {
                    let name = format!("lb-groups-{index}-{row_index}");
                    let mut params = vec![
                        ("Name", name.as_str()),
                        ("Type", row.lb_type),
                        ("Subnets.member.1", vpc.subnets[0].as_str()),
                        ("Subnets.member.2", vpc.subnets[1].as_str()),
                    ];
                    if let Some(scheme) = row.scheme {
                        params.push(("Scheme", scheme));
                    }
                    if row.names_group {
                        params.push(("SecurityGroups.member.1", vpc.named_group.as_str()));
                    }
                    let created = elbv2(&router, &runtime, "CreateLoadBalancer", &params);
                    assert_eq!(
                        created.status, 200,
                        "case {:?} in VPC {index}: expected the create to be served, got \
                         {created:?}",
                        row.case
                    );
                    let arn = element(&created.text, "LoadBalancerArn");

                    let wanted: Option<Vec<String>> = match row.expected {
                        Expected::VpcDefault => Some(vec![vpc.default_group.clone()]),
                        Expected::AsNamed => Some(vec![vpc.named_group.clone()]),
                        Expected::None => None,
                    };
                    let case = format!("{} in VPC {index}", row.case);
                    assert_eq!(
                        reported_groups(&router, &runtime, &case, &arn),
                        wanted,
                        "case {case:?}: expected the load balancer to report {wanted:?}"
                    );
                    // The create answered what the read reports: one load balancer, one
                    // group list — or no such member on either, for a load balancer with
                    // no security group.
                    let answered: Option<Vec<String>> = block_of(&created.text, "SecurityGroups")
                        .map(|block| members_of(block).into_iter().map(str::to_string).collect());
                    assert_eq!(
                        answered, wanted,
                        "case {case:?}: the create must answer the groups a later read \
                         reports, got {answered:?}"
                    );
                }
            }

            // A caller that REPLACES the groups of a load balancer that took its VPC's
            // default one gets exactly what it named: the default is what a create fills,
            // not a group the service keeps adding.
            let name = "lb-groups-replaced";
            let created = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", name),
                    ("Subnets.member.1", first.subnets[0].as_str()),
                    ("Subnets.member.2", first.subnets[1].as_str()),
                ],
            );
            assert_eq!(
                created.status, 200,
                "setup: the load balancer to reconfigure must be created, got {created:?}"
            );
            let arn = element(&created.text, "LoadBalancerArn");
            assert_eq!(
                reported_groups(&router, &runtime, "before the replacement", &arn),
                Some(vec![first.default_group.clone()]),
                "case \"before the replacement\": it took its VPC's default group"
            );
            let replaced = elbv2(
                &router,
                &runtime,
                "SetSecurityGroups",
                &[
                    ("LoadBalancerArn", arn.as_str()),
                    ("SecurityGroups.member.1", first.named_group.as_str()),
                ],
            );
            assert_eq!(
                replaced.status, 200,
                "case \"the replacement\": expected 200, got {replaced:?}"
            );
            assert_eq!(
                reported_groups(&router, &runtime, "after the replacement", &arn),
                Some(vec![first.named_group.clone()]),
                "case \"after the replacement\": a read reports the groups the caller set"
            );
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
