// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal `CreateLoadBalancer` answers for a security group it cannot give the load
//! balancer, driven end to end over the wire with EC2 — which owns the groups — mounted
//! beside this service.
//!
//! The rule asserted here: **a load balancer's security groups govern traffic to its network
//! interfaces, so a request naming one Amazon EC2 will not give it is refused as a
//! CONFIGURATION that cannot hold together — `InvalidConfigurationRequest` (HTTP 400) with
//! Elastic Load Balancing's own fixed sentence for the class, which names none of the ids —
//! whichever fault the EC2 read ran into, while the modelled "the specified security group
//! does not exist" (`InvalidSecurityGroup`) is kept for a value that is not a security group
//! id at all and so names no group in ANY account. Either way nothing is created.**
//!
//! `InvalidSecurityGroupException` is documented as "One or more of the specified security
//! groups do not exist", and `InvalidConfigurationRequestException` as the request being "not
//! valid" — so what decides the class is whether the REQUEST carried a group id at all, not
//! which sentence the EC2 read came back with: an id of the form EC2 mints that EC2 will not
//! resolve for this account, and a group of another VPC, are both groups this load balancer
//! cannot be given.
//!
//! The rows differ along what the rule generalises over: the FORM of the value (both id
//! lengths EC2 mints, and a value that carries no id at all), what EC2 answers about it (a
//! group it reports, an id it refuses as unresolvable, an id it refuses as unreadable), the
//! VPC the group governs traffic in, and how many groups the request names — crossed with the
//! two load-balancer families that carry a security group.
//!
//! The wire harness (`action`, `call`, `p`) comes from `provisioning_loop` and the EC2 front
//! door (`emulator_with_ec2`, `ec2_ok`, `reported`) from `target_security_group_health_check`;
//! nothing here restates them. The rule's own table over what EC2 answered lives in
//! `input_rejection_classes.rs`.

use smithy_server_runtime::MultiServiceRouter;

use crate::provisioning_loop::{action, call, p, Reply};
use crate::target_security_group_health_check::{ec2_ok, emulator_with_ec2, reported};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::INVALID_SECURITY_GROUPS;

/// The network every row runs in: one VPC with two subnets in different zones — what an
/// Application Load Balancer is placed across — plus a second VPC, which is where the group
/// of ANOTHER VPC comes from. The zones belong to the Region every request of this crate's
/// suites is signed for, and the ranges are this suite's own.
const VPC_CIDR: &str = "10.92.0.0/16";
const OTHER_VPC_CIDR: &str = "10.93.0.0/16";
const SUBNETS: [(&str, &str); 2] = [
    ("10.92.1.0/24", "us-west-2a"),
    ("10.92.2.0/24", "us-west-2b"),
];

/// A value of the long form EC2 mints ids in (`sg-` and 17 hexadecimal digits) that this
/// account holds no group under: EC2 refuses a read naming it, which is what the rule is
/// read through.
///
/// `pub(crate)` because the suite that asserts the WORDING of these refusals
/// (`all/security_group_refusal_wording.rs`) reads the same classes through the same values.
pub(crate) const UNHELD_LONG_ID: &str = "sg-0123456789abcdef0";

/// The same for the SHORT form EC2 also mints (`sg-` and 8 hexadecimal digits), because the
/// two forms are refused by EC2 under different faults and the class the load balancer
/// answers is the same for both.
pub(crate) const UNHELD_SHORT_ID: &str = "sg-12345678";

/// A value that is no security group id at all: it names no group in any account, so no
/// account could ever hold one under it.
pub(crate) const NOT_AN_ID: &str = "not-a-group";

/// The account's network for this suite, created through EC2's own API.
///
/// `pub(crate)` with [`seed_network`] because the suite that asserts the WORDING of these
/// refusals (`all/security_group_refusal_wording.rs`) needs the same network: one seeding
/// for the crate, so neither suite can drift on what EC2 holds.
pub(crate) struct Network {
    pub(crate) subnets: Vec<String>,
    /// A group of the VPC the load balancer is placed in.
    pub(crate) own_group: String,
    /// A group of ANOTHER VPC, which governs traffic the load balancer's interfaces never
    /// carry.
    pub(crate) other_vpc_group: String,
}

/// Create that network through EC2's own API, so every group the rows name is a record EC2's
/// own readers answer with.
pub(crate) async fn seed_network(router: &MultiServiceRouter) -> Network {
    let created = ec2_ok(router, "setup", "CreateVpc", &[p("CidrBlock", VPC_CIDR)]).await;
    let vpc = reported("setup", &created, "vpcId");
    let mut subnets: Vec<String> = Vec::new();
    for (cidr, zone) in SUBNETS {
        let created = ec2_ok(
            router,
            "setup",
            "CreateSubnet",
            &[
                p("VpcId", &vpc),
                p("CidrBlock", cidr),
                p("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(reported("setup", &created, "subnetId"));
    }
    let created = ec2_ok(router, "setup", "CreateVpc", &[p("CidrBlock", OTHER_VPC_CIDR)]).await;
    let other_vpc = reported("setup", &created, "vpcId");
    let mut groups: Vec<String> = Vec::new();
    for (index, in_vpc) in [vpc.as_str(), other_vpc.as_str()].into_iter().enumerate() {
        let created = ec2_ok(
            router,
            "setup",
            "CreateSecurityGroup",
            &[
                p("VpcId", in_vpc),
                p("GroupName", &format!("refusal-group-{index}")),
                // `CreateSecurityGroupRequest$Description` travels under its own `xmlName`,
                // `GroupDescription`.
                p("GroupDescription", "a group of this suite"),
            ],
        )
        .await;
        groups.push(reported("setup", &created, "groupId"));
    }
    Network {
        subnets,
        own_group: groups[0].clone(),
        other_vpc_group: groups[1].clone(),
    }
}

/// What a row expects of the create: served, or refused with one error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The create is served and the load balancer is readable.
    Served,
    /// The create is refused 400 with this awsQuery error code, and — where the class has a
    /// sentence of its own that states the whole rule — that sentence, which names none of
    /// the request's ids. The refusal for a value that is no group id at all carries the
    /// fault the EC2 read ran into instead, which names the value: that message is the
    /// read's, not a sentence this service composes, so only the class is asserted for it.
    Refused(&'static str, Option<&'static str>),
}

/// Which groups a row names: the values it sends for `SecurityGroups.member.N`, resolved
/// against the seeded network.
enum Groups {
    /// The group of the load balancer's own VPC.
    Own,
    /// The group of the other VPC.
    OtherVpc,
    /// Literal values the request carries as given.
    Literal(&'static [&'static str]),
    /// The own group, followed by literal values.
    OwnPlus(&'static [&'static str]),
}

impl Groups {
    /// The values the request sends, in order.
    fn values(&self, network: &Network) -> Vec<String> {
        match self {
            Self::Own => vec![network.own_group.clone()],
            Self::OtherVpc => vec![network.other_vpc_group.clone()],
            Self::Literal(values) => values.iter().map(|value| (*value).to_string()).collect(),
            Self::OwnPlus(values) => std::iter::once(network.own_group.clone())
                .chain(values.iter().map(|value| (*value).to_string()))
                .collect(),
        }
    }
}

/// One row: the case, the load balancer `Type` and `Name`, the groups the request names and
/// what the create must answer.
struct Row {
    case: &'static str,
    lb_type: &'static str,
    name: &'static str,
    groups: Groups,
    expected: Expected,
}

/// Ask for the load balancer `row` describes in `network`.
async fn create(router: &MultiServiceRouter, row: &Row, network: &Network) -> Reply {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", row.name));
    form.push(p("Type", row.lb_type));
    // Internal, so the placement needs no internet gateway on the VPC: what is under test
    // is the security groups, and an internet-facing placement is refused for the gateway
    // the seeded VPC deliberately has none of (`internet_facing_placement.rs` owns that
    // rule).
    form.push(p("Scheme", "internal"));
    for (index, subnet) in network.subnets.iter().enumerate() {
        form.push(p(&format!("Subnets.member.{}", index + 1), subnet));
    }
    for (index, group) in row.groups.values(network).iter().enumerate() {
        form.push(p(&format!("SecurityGroups.member.{}", index + 1), group));
    }
    call(router, &form).await
}

/// What `DescribeLoadBalancers` answers for the load balancer `name` names.
async fn describe(router: &MultiServiceRouter, name: &str) -> Reply {
    let mut form = action("DescribeLoadBalancers");
    form.push(p("Names.member.1", name));
    call(router, &form).await
}

#[tokio::test]
async fn a_security_group_the_load_balancer_cannot_be_given_is_a_configuration_refusal() {
    let rows: [Row; 9] = [
        Row {
            case: "a group of the load balancer's own VPC",
            lb_type: "application",
            name: "sgclass-own-group",
            groups: Groups::Own,
            expected: Expected::Served,
        },
        Row {
            case: "a long-form id this account holds no group under",
            lb_type: "application",
            name: "sgclass-unheld-long",
            groups: Groups::Literal(&[UNHELD_LONG_ID]),
            expected: Expected::Refused(
                "InvalidConfigurationRequest",
                Some(INVALID_SECURITY_GROUPS),
            ),
        },
        Row {
            case: "a short-form id this account holds no group under",
            lb_type: "application",
            name: "sgclass-unheld-short",
            groups: Groups::Literal(&[UNHELD_SHORT_ID]),
            expected: Expected::Refused(
                "InvalidConfigurationRequest",
                Some(INVALID_SECURITY_GROUPS),
            ),
        },
        Row {
            case: "a group of another VPC",
            lb_type: "application",
            name: "sgclass-other-vpc",
            groups: Groups::OtherVpc,
            expected: Expected::Refused(
                "InvalidConfigurationRequest",
                Some(INVALID_SECURITY_GROUPS),
            ),
        },
        Row {
            case: "a held group beside an id this account holds no group under",
            lb_type: "application",
            name: "sgclass-mixed-unheld",
            groups: Groups::OwnPlus(&[UNHELD_LONG_ID]),
            expected: Expected::Refused(
                "InvalidConfigurationRequest",
                Some(INVALID_SECURITY_GROUPS),
            ),
        },
        Row {
            case: "a value that is no security group id at all",
            lb_type: "application",
            name: "sgclass-not-an-id",
            groups: Groups::Literal(&[NOT_AN_ID]),
            expected: Expected::Refused("InvalidSecurityGroup", None),
        },
        Row {
            case: "a held group beside a value that is no security group id at all",
            lb_type: "application",
            name: "sgclass-mixed-not-an-id",
            groups: Groups::OwnPlus(&[NOT_AN_ID]),
            expected: Expected::Refused("InvalidSecurityGroup", None),
        },
        // The same two classes on the other family that carries a security group: the rule is
        // about the groups, not about what is being placed.
        Row {
            case: "a Network Load Balancer naming an id this account holds no group under",
            lb_type: "network",
            name: "sgclass-nlb-unheld",
            groups: Groups::Literal(&[UNHELD_LONG_ID]),
            expected: Expected::Refused(
                "InvalidConfigurationRequest",
                Some(INVALID_SECURITY_GROUPS),
            ),
        },
        Row {
            case: "a Network Load Balancer naming a value that is no security group id at all",
            lb_type: "network",
            name: "sgclass-nlb-not-an-id",
            groups: Groups::Literal(&[NOT_AN_ID]),
            expected: Expected::Refused("InvalidSecurityGroup", None),
        },
    ];

    let router = emulator_with_ec2();
    let network = seed_network(&router).await;
    for row in &rows {
        let case = row.case;
        let reply = create(&router, row, &network).await;
        match row.expected {
            Expected::Served => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the create to be served, got {} with {}",
                    reply.status, reply.body
                );
                let read = describe(&router, row.name).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected the created load balancer to be reported, got \
                     {} with {}",
                    read.status, read.body
                );
            }
            Expected::Refused(code, message) => {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (400, code),
                    "case {case:?}: expected HTTP 400 {code}, got {} with {}",
                    reply.status,
                    reply.body
                );
                if let Some(message) = message {
                    assert_eq!(
                        reply.text("Message").as_deref(),
                        Some(message),
                        "case {case:?}: expected the sentence {message:?}, got {}",
                        reply.body
                    );
                    // That sentence states the class, never the ids the request carried.
                    for named in row.groups.values(&network) {
                        assert!(
                            !reply.text("Message").unwrap_or_default().contains(&named),
                            "case {case:?}: expected the refusal to name no group id, got {}",
                            reply.body
                        );
                    }
                }
                let read = describe(&router, row.name).await;
                assert_eq!(
                    (read.status, read.error_code().as_str()),
                    (400, "LoadBalancerNotFound"),
                    "case {case:?}: a refused create must store nothing, got {} with {}",
                    read.status,
                    read.body
                );
            }
        }
    }
}
