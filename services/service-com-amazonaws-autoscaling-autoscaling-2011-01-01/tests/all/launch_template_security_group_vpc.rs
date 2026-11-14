// SPDX-License-Identifier: CC0-1.0

//! The VPC an Auto Scaling group's SUBNETS and its launch template's SECURITY GROUPS must
//! share.
//!
//! RULE: `VPCZoneIdentifier` names "subnet IDs for a virtual private cloud (VPC) where
//! instances in the Auto Scaling group can be created"
//! (`smithy_query(query="doc",shape="CreateAutoScalingGroupType",member="VPCZoneIdentifier")`),
//! and a security group belongs to one VPC (`SecurityGroup$VpcId`). An instance of the group
//! sits in one of those subnets, so a security group of another VPC could never be applied
//! to it: Amazon EC2 Auto Scaling refuses the create or the update with `ValidationError`
//! and stores nothing, rather than keeping a group whose instances could not exist.
//!
//! The rows span what the rule generalises over: whether the template names the group by id
//! or by name, whether the two sit in one VPC or two, where the group names the template
//! (its own `LaunchTemplate` or a `MixedInstancesPolicy`), a group placed by Availability
//! Zone with no subnet at all (in no VPC to cross-check), a reference the account's own
//! Amazon EC2 state resolves to nothing (no evidence of a mismatch), and the same pairing
//! reached by an UPDATE of a group that was created well.
//!
//! The network is built through Amazon EC2 itself, over its own wire, so every id these
//! assertions carry is one EC2 issued.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::support::{borrowed, ec2_query, p, query, tag_value, xml_unescape};

/// The image and instance type every launch template in this suite carries.
const IMAGE_ID: &str = "ami-00000000000000051";
const INSTANCE_TYPE: &str = "t3.micro";

/// A security group name both VPCs of this suite hold a group under, and one only the other
/// VPC holds.
const SHARED_GROUP_NAME: &str = "asg-shared";
const FOREIGN_GROUP_NAME: &str = "asg-foreign-only";

/// A security group id of the published form the account holds no group for.
const ABSENT_GROUP_ID: &str = "sg-00000000000000999";

/// One emulator with both services on one engine: the subnets and the security groups an
/// Auto Scaling group is built from are the same account's Amazon EC2 resources.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// The one element of an ec2Query answer this suite reads, after asserting the call was
/// served.
async fn ec2_element(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
    element: &str,
) -> String {
    let owned: Vec<(String, String)> = params
        .iter()
        .map(|(key, value)| p(key, *value))
        .collect();
    let (status, body) = ec2_query(router, action, &owned).await;
    assert_eq!(
        status, 200,
        "{action} expected 200, got {status}: {body}"
    );
    let value = tag_value(&body, element);
    assert!(
        !value.is_empty(),
        "{action} must report its {element}, got {body}"
    );
    value
}

/// The two networks this suite needs: the group's own, and another one of the same account.
struct Networks {
    /// The VPC the group's subnets are in, that VPC's subnet, and a security group of it.
    vpc: String,
    subnet: String,
    own_group: String,
    /// The other VPC, and a security group of it.
    other_vpc: String,
    foreign_group: String,
}

async fn seed_networks(router: &MultiServiceRouter) -> Networks {
    let vpc = ec2_element(router, "CreateVpc", &[("CidrBlock", "10.21.0.0/16")], "vpcId").await;
    let subnet = ec2_element(
        router,
        "CreateSubnet",
        &[
            ("VpcId", vpc.as_str()),
            ("CidrBlock", "10.21.1.0/24"),
            ("AvailabilityZone", "us-west-2a"),
        ],
        "subnetId",
    )
    .await;
    let own_group = ec2_element(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", SHARED_GROUP_NAME),
            ("GroupDescription", "the groups own network"),
            ("VpcId", vpc.as_str()),
        ],
        "groupId",
    )
    .await;

    let other_vpc =
        ec2_element(router, "CreateVpc", &[("CidrBlock", "10.22.0.0/16")], "vpcId").await;
    let foreign_group = ec2_element(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", FOREIGN_GROUP_NAME),
            ("GroupDescription", "another network of the account"),
            ("VpcId", other_vpc.as_str()),
        ],
        "groupId",
    )
    .await;
    // The same NAME in the other VPC too: a security group name is unique within one VPC,
    // so one name may stand for a group in each.
    ec2_element(
        router,
        "CreateSecurityGroup",
        &[
            ("GroupName", SHARED_GROUP_NAME),
            ("GroupDescription", "the same name in another network"),
            ("VpcId", other_vpc.as_str()),
        ],
        "groupId",
    )
    .await;

    Networks {
        vpc,
        subnet,
        own_group,
        other_vpc,
        foreign_group,
    }
}

/// Create a launch template whose data names `security_group` under `member`.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    member: &str,
    security_group: &str,
) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
            p(&format!("LaunchTemplateData.{member}"), security_group),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
    name.to_string()
}

/// Which security group the template names, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// The id of a group of the group's own VPC.
    OwnId,
    /// The id of a group of the OTHER VPC.
    ForeignId,
    /// The id of no group the account holds.
    AbsentId,
    /// A name both VPCs hold a group under.
    SharedName,
    /// A name only the other VPC holds a group under.
    ForeignName,
}

impl Named {
    /// The `LaunchTemplateData` member the reference is carried under: the id list is
    /// flattened under its member's own `xmlName` (`SecurityGroupId`) and the name list
    /// under `SecurityGroup`, as `RequestLaunchTemplateData` publishes them.
    fn member(self) -> &'static str {
        match self {
            Self::OwnId | Self::ForeignId | Self::AbsentId => "SecurityGroupId.1",
            Self::SharedName | Self::ForeignName => "SecurityGroup.1",
        }
    }

    /// The value the template carries for this reference.
    fn value(self, networks: &Networks) -> String {
        match self {
            Self::OwnId => networks.own_group.clone(),
            Self::ForeignId => networks.foreign_group.clone(),
            Self::AbsentId => ABSENT_GROUP_ID.to_string(),
            Self::SharedName => SHARED_GROUP_NAME.to_string(),
            Self::ForeignName => FOREIGN_GROUP_NAME.to_string(),
        }
    }
}

/// Where the group names the launch template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Own,
    MixedPolicy,
}

impl Source {
    fn member(self) -> &'static str {
        match self {
            Self::Own => "LaunchTemplate.LaunchTemplateName",
            Self::MixedPolicy => "MixedInstancesPolicy.LaunchTemplate.\
                                  LaunchTemplateSpecification.LaunchTemplateName",
        }
    }
}

/// What the create must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The group is created.
    Created,
    /// The group is refused: a security group of the template is not in the VPC the group
    /// launches into.
    Refused,
}

struct Row {
    case: &'static str,
    named: Named,
    source: Source,
    /// Whether the group names subnets at all; a group that names none is placed by zone.
    placed_by_subnet: bool,
    outcome: Outcome,
}

const ROWS: &[Row] = &[
    Row {
        case: "a template whose security group is in the group's own VPC",
        named: Named::OwnId,
        source: Source::Own,
        placed_by_subnet: true,
        outcome: Outcome::Created,
    },
    Row {
        case: "a template whose security group is in another VPC",
        named: Named::ForeignId,
        source: Source::Own,
        placed_by_subnet: true,
        outcome: Outcome::Refused,
    },
    Row {
        case: "the same pairing named through a mixed instances policy",
        named: Named::ForeignId,
        source: Source::MixedPolicy,
        placed_by_subnet: true,
        outcome: Outcome::Refused,
    },
    Row {
        case: "a group placed by Availability Zone, in no VPC to cross-check",
        named: Named::ForeignId,
        source: Source::Own,
        placed_by_subnet: false,
        outcome: Outcome::Created,
    },
    Row {
        case: "a security group id the account holds no group for",
        named: Named::AbsentId,
        source: Source::Own,
        placed_by_subnet: true,
        outcome: Outcome::Created,
    },
    Row {
        case: "a name the group's own VPC holds a group under",
        named: Named::SharedName,
        source: Source::Own,
        placed_by_subnet: true,
        outcome: Outcome::Created,
    },
    Row {
        case: "a name only another VPC holds a group under",
        named: Named::ForeignName,
        source: Source::Own,
        placed_by_subnet: true,
        outcome: Outcome::Refused,
    },
];

/// The sentence Amazon EC2 Auto Scaling refuses a launch template whose security groups are
/// not in the group's own network with. It is FIXED: it names neither the security group,
/// the subnet nor the VPC, a template being able to carry several groups.
const EXPECTED_MESSAGE: &str = "One or more security groups in the launch template are not \
                                linked to the VPC that the Auto Scaling group will launch \
                                instances in.";

#[tokio::test]
async fn a_launch_templates_security_groups_must_sit_in_the_groups_own_vpc() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let networks = seed_networks(&router).await;
        let template = create_launch_template(
            &router,
            &format!("lt-network-{index}"),
            row.named.member(),
            &row.named.value(&networks),
        )
        .await;

        let name = format!("network-{index}");
        let mut params = vec![
            p("AutoScalingGroupName", name.clone()),
            p(row.source.member(), template.clone()),
            p("MinSize", "0"),
            p("MaxSize", "1"),
        ];
        if row.placed_by_subnet {
            params.push(p("VPCZoneIdentifier", networks.subnet.clone()));
        } else {
            params.push(p("AvailabilityZones.member.1", "us-west-2a"));
        }
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;

        if row.outcome == Outcome::Created {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be accepted, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert_eq!(
            (
                tag_value(&body, "Code"),
                xml_unescape(&tag_value(&body, "Message"))
            ),
            ("ValidationError".to_string(), EXPECTED_MESSAGE.to_string()),
            "case {case:?}: expected the foreign-security-group refusal, got {body}"
        );
        let message = xml_unescape(&tag_value(&body, "Message"));
        for value in [
            row.named.value(&networks),
            networks.subnet.clone(),
            networks.vpc.clone(),
            networks.other_vpc.clone(),
        ] {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the sentence names no resource of the request, got {message}"
            );
        }

        // A refused create stores no group.
        let (status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {described}"
        );
        assert!(
            !described.contains(name.as_str()),
            "case {case:?}: a refused create leaves no group behind, got {described}"
        );
    }
}

#[tokio::test]
async fn an_update_onto_a_foreign_security_group_is_refused_and_changes_nothing() {
    let case = "a group moved onto a template whose security group is in another VPC";
    let router = emulator();
    let networks = seed_networks(&router).await;
    let good = create_launch_template(
        &router,
        "lt-update-own",
        Named::OwnId.member(),
        &Named::OwnId.value(&networks),
    )
    .await;
    let bad = create_launch_template(
        &router,
        "lt-update-foreign",
        Named::ForeignId.member(),
        &Named::ForeignId.value(&networks),
    )
    .await;

    let params = [
        p("AutoScalingGroupName", "updated"),
        p("LaunchTemplate.LaunchTemplateName", good.clone()),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("VPCZoneIdentifier", networks.subnet.clone()),
    ];
    let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup expected 200, got {status}: {body}"
    );

    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "updated"),
            ("LaunchTemplate.LaunchTemplateName", bad.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the update to be refused, got {status}: {body}"
    );
    assert_eq!(
        xml_unescape(&tag_value(&body, "Message")),
        EXPECTED_MESSAGE,
        "case {case:?}: expected the foreign-security-group refusal, got {body}"
    );

    // The group is still on the template it was created with.
    let (_, described) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "updated")],
    )
    .await;
    assert_eq!(
        tag_value(&described, "LaunchTemplateName"),
        good,
        "case {case:?}: a refused update leaves the group's launch template alone, \
         got {described}"
    );
}
