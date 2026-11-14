// SPDX-License-Identifier: CC0-1.0

//! The PHYSICAL IDENTIFIERS one stack's whole FLEET estate reports, and the outputs a client
//! reads them through: an Amazon EC2 Auto Scaling group, the launch template it launches from,
//! an Amazon EC2 instance and the network the three sit in, declared by ONE template.
//!
//! RULE: for every resource type whose owning service holds the resource, the
//! `PhysicalResourceId` a `CREATE_COMPLETE` stack reports is the identifier THAT service
//! answers its own read with — so a read of that service addressed by exactly that value
//! reports exactly that one resource — and every output the template declares over such a
//! resource carries a resolved `OutputValue` beside its `OutputKey`.
//!
//! Why it matters: a stack's `PhysicalResourceId` and its outputs are the only way a client
//! gets a created resource's identifier out of a deployment. An identifier the owning service
//! does not answer for (a `<stack name>-<logical id>-<unique id>` name where that service mints
//! an `i-`/`lt-`/`sg-` id) is refused by the very next call, in the neighbouring service rather
//! than here; and an output reported with a key and no value cannot be consumed at all.
//!
//! The rows differ along what the rule generalises over — the resource TYPE, and with it the
//! owning service (Amazon EC2, Amazon EC2 Auto Scaling), the request member the read addresses
//! the resource by, and the FORM of the identifier: one the owning service mints (the `vpc-`,
//! `subnet-`, `sg-`, `lt-` and `i-` ids, whose form is asserted rather than spelled, since no
//! request fixes it) and one CloudFormation decides, both where the template leaves the group
//! to be named and where it names the group itself.
//!
//! The estate is one stack rather than one per row on purpose: an Auto Scaling group that
//! launches from a launch template into a subnet of a VPC, beside an instance in that same
//! subnet, is how a template says "launch instances here", and reading every one of them out
//! of ONE completed stack is what says the whole estate is addressable.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_resource_provisioning::{outputs_of, poll_to_terminal};
use crate::stack_lifecycle::{
    autoscaling, call, create_stack, ec2, is_service_assigned_id, member, router_with_engine,
    xml_text, xml_texts,
};
use crate::stack_outputs::wire_outputs;

/// The stack the estate is deployed as.
const STACK: &str = "fleet-estate-ids";

/// The Availability Zone the estate's subnet is created in: the Region's first, which offers
/// every instance type the catalog publishes.
const ZONE: &str = "us-east-1a";

/// The public Systems Manager parameter the template resolves its AMI from — a name in AWS's
/// own `/aws/service/` hierarchy, which the published catalog answers for, so neither the
/// instance nor the launch template names an image id this suite invented.
const IMAGE_PARAMETER: &str =
    "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64";

/// The name the template gives the group it names itself — the row that is NOT left to
/// CloudFormation to name.
const DECLARED_GROUP_NAME: &str = "fleet-estate-named-group";

/// Which service owns the resource a row is about, and so which wire its read goes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Amazon EC2, whose reads take an `ec2Query` body and report an element per resource.
    Ec2,
    /// Amazon EC2 Auto Scaling, whose `DescribeAutoScalingGroups` takes the group's name.
    AutoScaling,
}

/// What the identifier a row's resource is reported under must BE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identifier {
    /// One the owning service mints, of the form `<prefix>-` plus its hexadecimal part.
    Assigned(&'static str),
    /// The name the TEMPLATE states for the resource.
    Declared(&'static str),
    /// The name CloudFormation generates for a resource the template leaves unnamed, which
    /// begins with the stack's own name.
    Generated,
}

/// One resource of the estate: where the stack reports it, how the owning service is asked
/// about it, and what its identifier must be.
struct Row {
    case: &'static str,
    logical_id: &'static str,
    /// The output whose value is `Ref`-of this resource, which must be the same identifier.
    output_key: &'static str,
    owner: Owner,
    /// The read that owning service answers for one resource of the type.
    action: &'static str,
    /// The request member that read addresses ONE resource by.
    request_member: &'static str,
    /// The element that read reports the resource's own identifier in.
    reported_element: &'static str,
    identifier: Identifier,
}

/// Every resource of the estate, one row each.
///
/// The request members and reported elements are each service's own wire names: Amazon EC2
/// flattens a list member into indexed keys (`VpcId.1`) and reports an `ec2Query` answer's
/// members in camel case (`vpcId`), while Amazon EC2 Auto Scaling takes `AutoScalingGroupNames`
/// as an `awsQuery` list (`AutoScalingGroupNames.member.1`) and reports the group's own
/// `AutoScalingGroupName`.
const ROWS: &[Row] = &[
    Row {
        case: "the VPC the estate sits in",
        logical_id: "Vpc",
        output_key: "VpcId",
        owner: Owner::Ec2,
        action: "DescribeVpcs",
        request_member: "VpcId.1",
        reported_element: "vpcId",
        identifier: Identifier::Assigned("vpc"),
    },
    Row {
        case: "the subnet the group and the instance launch into",
        logical_id: "Subnet",
        output_key: "SubnetId",
        owner: Owner::Ec2,
        action: "DescribeSubnets",
        request_member: "SubnetId.1",
        reported_element: "subnetId",
        identifier: Identifier::Assigned("subnet"),
    },
    Row {
        case: "the security group the instance is launched with",
        logical_id: "Group",
        output_key: "GroupId",
        owner: Owner::Ec2,
        action: "DescribeSecurityGroups",
        request_member: "GroupId.1",
        reported_element: "groupId",
        identifier: Identifier::Assigned("sg"),
    },
    Row {
        case: "the launch template the group launches from",
        logical_id: "Lt",
        output_key: "LaunchTemplateId",
        owner: Owner::Ec2,
        action: "DescribeLaunchTemplates",
        request_member: "LaunchTemplateId.1",
        reported_element: "launchTemplateId",
        identifier: Identifier::Assigned("lt"),
    },
    Row {
        case: "the instance the template declares of its own",
        logical_id: "Instance",
        output_key: "InstanceId",
        owner: Owner::Ec2,
        action: "DescribeInstances",
        request_member: "InstanceId.1",
        reported_element: "instanceId",
        identifier: Identifier::Assigned("i"),
    },
    Row {
        case: "the Auto Scaling group the template leaves CloudFormation to name",
        logical_id: "Asg",
        output_key: "GroupName",
        owner: Owner::AutoScaling,
        action: "DescribeAutoScalingGroups",
        request_member: "AutoScalingGroupNames.member.1",
        reported_element: "AutoScalingGroupName",
        identifier: Identifier::Generated,
    },
    Row {
        case: "the Auto Scaling group the template names itself",
        logical_id: "NamedAsg",
        output_key: "NamedGroupName",
        owner: Owner::AutoScaling,
        action: "DescribeAutoScalingGroups",
        request_member: "AutoScalingGroupNames.member.1",
        reported_element: "AutoScalingGroupName",
        identifier: Identifier::Declared(DECLARED_GROUP_NAME),
    },
];

/// The estate's template: the network, the launch template, two Auto Scaling groups and one
/// instance, with an output over every one of them and over three attributes only the owning
/// services can answer.
fn template() -> String {
    let declared: Value = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "ImageId": {
                "Type": "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>",
                "Default": IMAGE_PARAMETER,
            }
        },
        "Resources": {
            "Vpc": {
                "Type": "AWS::EC2::VPC",
                "Properties": { "CidrBlock": "10.191.0.0/16" }
            },
            "Subnet": {
                "Type": "AWS::EC2::Subnet",
                "Properties": {
                    "VpcId": { "Ref": "Vpc" },
                    "CidrBlock": "10.191.1.0/24",
                    "AvailabilityZone": ZONE,
                }
            },
            "Group": {
                "Type": "AWS::EC2::SecurityGroup",
                "Properties": {
                    "GroupDescription": "the estate instances",
                    "VpcId": { "Ref": "Vpc" },
                }
            },
            "Lt": {
                "Type": "AWS::EC2::LaunchTemplate",
                "Properties": {
                    "LaunchTemplateData": {
                        "ImageId": { "Ref": "ImageId" },
                        "InstanceType": "t3.micro",
                    }
                }
            },
            "Asg": {
                "Type": "AWS::AutoScaling::AutoScalingGroup",
                "Properties": {
                    "MinSize": "1",
                    "MaxSize": "2",
                    "DesiredCapacity": "1",
                    "VPCZoneIdentifier": [{ "Ref": "Subnet" }],
                    "LaunchTemplate": {
                        "LaunchTemplateId": { "Ref": "Lt" },
                        "Version": { "Fn::GetAtt": ["Lt", "LatestVersionNumber"] },
                    }
                }
            },
            "NamedAsg": {
                "Type": "AWS::AutoScaling::AutoScalingGroup",
                "Properties": {
                    "AutoScalingGroupName": DECLARED_GROUP_NAME,
                    "MinSize": "0",
                    "MaxSize": "1",
                    "DesiredCapacity": "0",
                    "VPCZoneIdentifier": [{ "Ref": "Subnet" }],
                    "LaunchTemplate": {
                        "LaunchTemplateId": { "Ref": "Lt" },
                        "Version": { "Fn::GetAtt": ["Lt", "LatestVersionNumber"] },
                    }
                }
            },
            "Instance": {
                "Type": "AWS::EC2::Instance",
                "Properties": {
                    "ImageId": { "Ref": "ImageId" },
                    "InstanceType": "t3.micro",
                    "SubnetId": { "Ref": "Subnet" },
                    "SecurityGroupIds": [{ "Ref": "Group" }],
                }
            }
        },
        "Outputs": {
            "VpcId": { "Value": { "Ref": "Vpc" } },
            "SubnetId": { "Value": { "Ref": "Subnet" } },
            "GroupId": { "Value": { "Ref": "Group" } },
            "LaunchTemplateId": { "Value": { "Ref": "Lt" } },
            "InstanceId": { "Value": { "Ref": "Instance" } },
            "GroupName": { "Value": { "Ref": "Asg" } },
            "NamedGroupName": { "Value": { "Ref": "NamedAsg" } },
            // Three attributes no uniform naming rule could compute: the group's ARN and the
            // launch template's version number are the owning services' own, and the
            // instance's private address is not assigned until that service reports it up.
            "GroupArn": { "Value": { "Fn::GetAtt": ["Asg", "Arn"] } },
            "LaunchTemplateVersion": {
                "Value": { "Fn::GetAtt": ["Lt", "LatestVersionNumber"] }
            },
            "InstancePrivateIp": { "Value": { "Fn::GetAtt": ["Instance", "PrivateIp"] } },
        }
    });
    declared.to_string()
}

/// The `PhysicalResourceId` the stack reports for one of its logical resources, read through
/// `DescribeStackResources` — the member a client takes a created resource's identifier from.
async fn physical_id(router: &MultiServiceRouter, case: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", STACK),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResources({logical_id}) must answer 200, got {status} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: {logical_id} must report a PhysicalResourceId, got {body}")
    })
}

/// Every resource the stack reports a FAILED status for, with the reason it carries — what a
/// case that expected the estate to deploy reports instead of the whole envelope.
async fn failures(router: &MultiServiceRouter) -> Vec<(String, String, String)> {
    let (status, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackEvents({STACK}) must answer 200, got {status} {body}"
    );
    crate::stack_lifecycle::xml_blocks(&body, "member")
        .into_iter()
        .map(|event| {
            let read = |tag: &str| xml_texts(&event, tag).into_iter().next().unwrap_or_default();
            (
                read("LogicalResourceId"),
                read("ResourceStatus"),
                read("ResourceStatusReason"),
            )
        })
        .filter(|(_, event_status, _)| event_status.ends_with("_FAILED"))
        .collect()
}

#[tokio::test]
async fn every_resource_of_a_fleet_stack_answers_to_the_identifier_the_stack_reports() {
    let (router, _engine) = router_with_engine();
    let (status, body) = create_stack(&router, STACK, &template(), &[]).await;
    assert_eq!(
        status, 200,
        "CreateStack must answer 200 — the stack reports the outcome — got {status} {body}"
    );
    let described = poll_to_terminal(&router, STACK, STACK).await;
    let settled = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        settled,
        "CREATE_COMPLETE",
        "the estate must deploy, got {settled:?} with the failures {:?}",
        failures(&router).await
    );

    // Every output the template declares carries a value: the half of the rule a client reads
    // the deployment through. `outputs_of` panics naming the key where one carries none.
    let published = outputs_of("the whole estate", &described);
    assert_eq!(
        published.len(),
        wire_outputs(&described).len(),
        "case \"the whole estate\": every declared output must be reported with a value, got \
         {described}"
    );
    let value_of = |key: &str| -> String {
        published
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| {
                panic!("case \"the whole estate\": no output named {key:?}, got {published:?}")
            })
    };

    for row in ROWS {
        let case = row.case;
        let physical = physical_id(&router, case, row.logical_id).await;

        // What the identifier IS: the owning service's own id for a type that mints one, the
        // declared name where the template states it, and a generated name — never the bare
        // logical id — where it does not.
        match row.identifier {
            Identifier::Assigned(prefix) => assert!(
                is_service_assigned_id(&physical, prefix),
                "case {case:?}: expected the {prefix}- identifier the owning service assigned \
                 as the PhysicalResourceId, got {physical:?}"
            ),
            Identifier::Declared(name) => assert_eq!(
                physical, name,
                "case {case:?}: expected the declared name {name:?}, got {physical:?}"
            ),
            Identifier::Generated => assert!(
                physical.starts_with(STACK) && physical != row.logical_id,
                "case {case:?}: expected a name CloudFormation generated for the unnamed \
                 resource, got {physical:?}"
            ),
        }

        // ...and the owning service answers for THAT value: a read addressed by it reports
        // exactly one resource, the resource itself.
        let params = [(row.request_member, physical.as_str())];
        let (status, reported) = match row.owner {
            Owner::Ec2 => ec2(&router, row.action, &params).await,
            Owner::AutoScaling => autoscaling(&router, row.action, &params).await,
        };
        assert_eq!(
            status, 200,
            "case {case:?}: the owning service must answer {}({physical}) with 200, got \
             {status} {reported}",
            row.action
        );
        let answered = xml_texts(&reported, row.reported_element);
        assert_eq!(
            answered,
            vec![physical.clone()],
            "case {case:?}: expected {}({physical}) to report exactly that one resource, got \
             {reported}",
            row.action
        );

        // The output over the resource is that same identifier, so a client that reads the
        // stack's outputs and a client that reads its resources call the owning service with
        // the same value.
        let published_value = value_of(row.output_key);
        assert_eq!(
            published_value, physical,
            "case {case:?}: expected the output {:?} to carry the resource's own identifier \
             {physical:?}, got {published_value:?}",
            row.output_key
        );
    }

    // The two attributes of the fleet resources that only their owning services can answer
    // resolve too: the group's ARN is Auto Scaling's own, and the launch template's latest
    // version number is Amazon EC2's.
    let group_arn = value_of("GroupArn");
    let group_name = value_of("GroupName");
    assert!(
        group_arn.contains(":autoscaling:") && group_arn.ends_with(&group_name),
        "case \"the whole estate\": expected the group's ARN to be Auto Scaling's own for \
         {group_name:?}, got {group_arn:?}"
    );
    let version = value_of("LaunchTemplateVersion");
    assert!(
        version.parse::<u64>().is_ok_and(|number| number >= 1),
        "case \"the whole estate\": expected the launch template's version number, got \
         {version:?}"
    );
    let private_ip = value_of("InstancePrivateIp");
    assert_eq!(
        private_ip.split('.').count(),
        4,
        "case \"the whole estate\": expected the address Amazon EC2 gave the instance, got \
         {private_ip:?}"
    );
}
