// SPDX-License-Identifier: CC0-1.0

//! The Elastic Load Balancing v2 TARGETS an Auto Scaling group's instances occupy in the
//! target groups it is attached to.
//!
//! The rule under test: an instance of a group that is attached to a target group IS a
//! target of that target group — `CreateAutoScalingGroup$TargetGroupARNs` is "the Amazon
//! Resource Names (ARN) of the target groups to associate with the Auto Scaling group.
//! Instances are registered as targets in a target group, and traffic is routed to the
//! target group", and `AttachLoadBalancerTargetGroups` adds the same association later
//! ("this operation is additive and does not detach existing target groups"), while a
//! detach "enters the Removing state while deregistering the instances in the group". So
//! every instance the group holds is registered with every target group it is attached to,
//! at that target group's own port and in the Availability Zone the instance runs in; an
//! instance that leaves the group, and an attachment that is removed, deregister; and
//! `DescribeAutoScalingGroups` reports the group's collections — its attachments included —
//! as lists, empty when the group holds nothing in them, rather than leaving the member out.
//!
//! The registrations are asserted where a client sees them: Elastic Load Balancing v2's own
//! `DescribeTargetHealth`, over its own wire, with all three services (Auto Scaling, Amazon
//! EC2 and Elastic Load Balancing v2) mounted on ONE engine — so a target asserted here is
//! one the owning service returns from its own read, and the instance ids are the account's
//! own Amazon EC2 instances.
//!
//! Registering the instance is PART of bringing it into service, so the same rule decides
//! what a launch does when an attachment can take no registration at all: a target group
//! that has been DELETED in Elastic Load Balancing v2 since it was attached stops the
//! launch that would have to register there — the group launches nothing and records a
//! failed scaling activity whose `StatusMessage` names the target groups that are gone
//! ("if an attempt to launch instances failed, the StatusCode value is Failed or Cancelled
//! and the StatusMessage element in the response indicates the cause of the failure",
//! `DescribeScalingActivities`) — and launches again as soon as the group holds no
//! attachment that cannot resolve.
//!
//! The tables below span what the rule generalises over: how many target groups a group is
//! attached to, how many instances it holds, how many zones they are spread across, the
//! target group's own port, whether the attachment came from the create or from a later
//! attach, whether the attachment is named as `TargetGroupARNs` or as an `elbv2` traffic
//! source, the target group's `TargetType`, the documented per-call bound on an attach, and
//! how many of a group's attachments have been deleted out of band.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as elbv2_service;

use crate::support::{
    borrowed, ec2_query, form_encode, p, query, seed_subnets, tag_value, tag_values, xml_unescape,
    SeededSubnet, REGION,
};

/// The API version an Elastic Load Balancing v2 client puts in its awsQuery body, read from
/// that service's own statement of it rather than written out here.
const ELBV2_VERSION: &str = elbv2_service::service::shared_endpoint::API_VERSION;

/// The most target groups ONE attach may name, from the model's own documentation for
/// `AttachLoadBalancerTargetGroupsType$TargetGroupARNs`: "You can specify up to 10 target
/// groups."
const MAX_TARGET_GROUPS_PER_CALL: usize = 10;

/// The most load-balancer attachments ONE group may HOLD, however many calls put them
/// there — the ceiling this process holds a group to, read from the crate rather than
/// written out here so the rows below cannot drift from the handlers.
const MAX_TARGET_GROUPS_PER_GROUP: usize = autoscaling::state::MAX_TARGET_GROUPS_PER_GROUP;

/// The `TargetTypeEnum` value of a target group whose targets are Amazon EC2 instances
/// (`TargetGroup$TargetType`: "`instance` - Targets are specified by instance ID").
const TARGET_TYPE_INSTANCE: &str = "instance";

/// The `TrafficSourceIdentifier$Type` of a traffic source whose `Identifier` is an Elastic
/// Load Balancing v2 target group ARN — "`elbv2` if `Identifier` is the ARN of an
/// Application Load Balancer, Gateway Load Balancer, or Network Load Balancer target group"
/// (`smithy_query(query="doc",shape="TrafficSourceIdentifier",member="Type")`).
const TRAFFIC_SOURCE_TYPE_ELBV2: &str = "elbv2";

/// One emulator with the three services on one engine: a group's instances are the
/// account's own Amazon EC2 instances, and the targets they occupy are Elastic Load
/// Balancing v2's own records, so one engine is what makes the three views one account.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(elbv2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// POST one awsQuery action to Elastic Load Balancing v2, the way a client does, and answer
/// its status and body.
async fn elbv2_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={ELBV2_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://elasticloadbalancingv2.{REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The VPC the seeded subnets are in, read back through Amazon EC2's own
/// `DescribeSubnets` — the service that owns the subnet is what says which VPC it is in.
async fn vpc_of(router: &MultiServiceRouter, subnet: &SeededSubnet) -> String {
    let (status, body) = ec2_query(router, "DescribeSubnets", &[p("SubnetId.1", &subnet.id)]).await;
    assert_eq!(
        status, 200,
        "DescribeSubnets for {:?} expected 200, got {status}: {body}",
        subnet.id
    );
    let vpc_id = tag_value(&body, "vpcId");
    assert!(
        !vpc_id.is_empty(),
        "DescribeSubnets must report the subnet's VPC, got {body}"
    );
    vpc_id
}

/// Create one Elastic Load Balancing v2 target group over the wire and answer the ARN that
/// service minted for it.
async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    port: i32,
    target_type: &str,
    vpc_id: &str,
) -> String {
    let (status, body) = elbv2_query(
        router,
        "CreateTargetGroup",
        &[
            p("Name", name),
            p("Protocol", "HTTP"),
            p("Port", port.to_string()),
            p("VpcId", vpc_id),
            p("TargetType", target_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTargetGroup for {name:?} expected 200, got {status}: {body}"
    );
    let arn = tag_value(&body, "TargetGroupArn");
    assert!(
        !arn.is_empty(),
        "CreateTargetGroup must report the target group's ARN, got {body}"
    );
    arn
}

/// Create one Amazon EC2 launch template over the wire, so the group launches from a
/// template the account actually holds.
async fn create_launch_template(router: &MultiServiceRouter, name: &str) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", "ami-0123456789abcdef0"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name:?} expected 200, got {status}: {body}"
    );
}

/// How a group names the target groups it is created attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachedAs {
    /// `TargetGroupARNs`, the member `AttachLoadBalancerTargetGroups` also writes.
    TargetGroupArns,
    /// `TrafficSources`, each entry an `elbv2` traffic source whose `Identifier` is the
    /// target group's ARN.
    Elbv2TrafficSources,
    /// Attached by a later `AttachLoadBalancerTargetGroups` call rather than by the create.
    LaterAttach,
}

/// Create one group over the awsQuery wire: placed by `VPCZoneIdentifier` over `subnets`,
/// holding `desired` instances, attached to `target_groups` the way `attached_as` says.
async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    subnets: &[SeededSubnet],
    desired: usize,
    target_groups: &[String],
    attached_as: AttachedAs,
) {
    let ids: Vec<&str> = subnets.iter().map(|subnet| subnet.id.as_str()).collect();
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", template),
        p("MinSize", "0"),
        p("MaxSize", "8"),
        p("DesiredCapacity", desired.to_string()),
        p("VPCZoneIdentifier", ids.join(",")),
    ];
    for (index, arn) in target_groups.iter().enumerate() {
        match attached_as {
            AttachedAs::TargetGroupArns => params.push(p(
                format!("TargetGroupARNs.member.{}", index + 1).as_str(),
                arn.clone(),
            )),
            AttachedAs::Elbv2TrafficSources => {
                params.push(p(
                    format!("TrafficSources.member.{}.Identifier", index + 1).as_str(),
                    arn.clone(),
                ));
                params.push(p(
                    format!("TrafficSources.member.{}.Type", index + 1).as_str(),
                    "elbv2",
                ));
            }
            AttachedAs::LaterAttach => {}
        }
    }
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {name:?} must succeed, got {status} with {body:?}"
    );
    if attached_as == AttachedAs::LaterAttach && !target_groups.is_empty() {
        let (status, code, message) = attach(router, name, target_groups).await;
        assert_eq!(
            status, 200,
            "attaching {} target group(s) to {name:?} must succeed, got {status} with \
             {code:?} {message:?}",
            target_groups.len()
        );
    }
}

/// Post one attach and answer its status, wire error code and message.
async fn attach(
    router: &MultiServiceRouter,
    name: &str,
    arns: &[String],
) -> (u16, String, String) {
    let mut params = vec![p("AutoScalingGroupName", name)];
    for (index, arn) in arns.iter().enumerate() {
        params.push(p(
            format!("TargetGroupARNs.member.{}", index + 1).as_str(),
            arn.clone(),
        ));
    }
    let (status, body) = query(router, "AttachLoadBalancerTargetGroups", &borrowed(&params)).await;
    (status, tag_value(&body, "Code"), tag_value(&body, "Message"))
}

/// Post one detach and answer its status.
async fn detach(router: &MultiServiceRouter, name: &str, arns: &[String]) -> u16 {
    let mut params = vec![p("AutoScalingGroupName", name)];
    for (index, arn) in arns.iter().enumerate() {
        params.push(p(
            format!("TargetGroupARNs.member.{}", index + 1).as_str(),
            arn.clone(),
        ));
    }
    let (status, _) = query(router, "DetachLoadBalancerTargetGroups", &borrowed(&params)).await;
    status
}

/// One `DescribeAutoScalingGroups` body for the named group.
async fn described(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read-back of {name:?} must succeed, got {status} with {body:?}"
    );
    body
}

/// The XML inside the first element named `tag`, or `None` when the element is absent.
///
/// Attribute-tolerant, because the envelope stamps the document's namespace onto the first
/// element of a list.
fn element_inner<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let inner_start = after.find('>')? + 1;
    let inner = &after[inner_start..];
    let end = inner.find(&close)?;
    Some(&inner[..end])
}

/// Whether the body names the element `tag` at all — present and empty included, which is
/// how an empty list crosses the wire (`<Tag></Tag>`).
fn names_element(xml: &str, tag: &str) -> bool {
    [format!("<{tag}>"), format!("<{tag} "), format!("<{tag}/>")]
        .iter()
        .any(|needle| xml.contains(needle.as_str()))
}

/// Each `<member>…</member>` of `xml`, in document order. The lists these are read out of
/// hold no nested member list.
fn members(xml: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<member") {
        let after = &rest[start..];
        let Some(inner_start) = after.find('>').map(|at| at + 1) else {
            break;
        };
        let inner = &after[inner_start..];
        let Some(end) = inner.find("</member>") else {
            break;
        };
        found.push(&inner[..end]);
        rest = &inner[end + "</member>".len()..];
    }
    found
}

/// The target groups the group reports under its own `TargetGroupARNs`, in the order it
/// holds them.
async fn attached_target_groups(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let body = described(router, name).await;
    assert!(
        names_element(&body, "TargetGroupARNs"),
        "a described group must report its TargetGroupARNs, got {body}"
    );
    let inner = element_inner(&body, "TargetGroupARNs").unwrap_or_default();
    tag_values(inner, "member")
}

/// Every instance the group reports, as `(instance id, Availability Zone)` in the order
/// reported.
async fn group_instances(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let body = described(router, name).await;
    let inner = element_inner(&body, "Instances").unwrap_or_default();
    members(inner)
        .into_iter()
        .map(|member| {
            (
                tag_value(member, "InstanceId"),
                tag_value(member, "AvailabilityZone"),
            )
        })
        .collect()
}

/// One target Elastic Load Balancing v2 reports as registered with a target group: its id,
/// the port it receives traffic on and the zone it is in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReportedTarget {
    id: String,
    port: String,
    availability_zone: String,
}

/// The targets Elastic Load Balancing v2 reports as registered with `arn`, sorted, read
/// through its own `DescribeTargetHealth`.
async fn registered_targets(router: &MultiServiceRouter, arn: &str) -> Vec<ReportedTarget> {
    let (status, body) =
        elbv2_query(router, "DescribeTargetHealth", &[p("TargetGroupArn", arn)]).await;
    assert_eq!(
        status, 200,
        "DescribeTargetHealth for {arn:?} expected 200, got {status}: {body}"
    );
    let inner = element_inner(&body, "TargetHealthDescriptions").unwrap_or_default();
    let mut targets: Vec<ReportedTarget> = members(inner)
        .into_iter()
        .map(|member| ReportedTarget {
            id: tag_value(member, "Id"),
            port: tag_value(member, "Port"),
            availability_zone: tag_value(member, "AvailabilityZone"),
        })
        .collect();
    targets.sort();
    targets
}

/// What the group's instances must be reported as, in the target group on `port`.
fn expected_targets(instances: &[(String, String)], port: i32) -> Vec<ReportedTarget> {
    let mut expected: Vec<ReportedTarget> = instances
        .iter()
        .map(|(id, zone)| ReportedTarget {
            id: id.clone(),
            port: port.to_string(),
            availability_zone: zone.clone(),
        })
        .collect();
    expected.sort();
    expected
}

/// One row of the registration table: how many subnets the group is placed over, how many
/// instances it holds, how many target groups it is attached to, those target groups' port,
/// and how the attachment was made.
struct RegistrationRow {
    case: &'static str,
    subnets: usize,
    desired: usize,
    target_groups: usize,
    port: i32,
    attached_as: AttachedAs,
}

fn registration_rows() -> Vec<RegistrationRow> {
    vec![
        RegistrationRow {
            case: "one target group, one instance, one subnet",
            subnets: 1,
            desired: 1,
            target_groups: 1,
            port: 80,
            attached_as: AttachedAs::TargetGroupArns,
        },
        RegistrationRow {
            case: "two target groups, two instances across two subnets",
            subnets: 2,
            desired: 2,
            target_groups: 2,
            port: 8080,
            attached_as: AttachedAs::TargetGroupArns,
        },
        RegistrationRow {
            case: "three target groups, three instances, attached by a later attach",
            subnets: 2,
            desired: 3,
            target_groups: 3,
            port: 443,
            attached_as: AttachedAs::LaterAttach,
        },
        RegistrationRow {
            case: "one target group named as an elbv2 traffic source",
            subnets: 1,
            desired: 2,
            target_groups: 1,
            port: 9000,
            attached_as: AttachedAs::Elbv2TrafficSources,
        },
        RegistrationRow {
            case: "a target group of a group that holds no instance",
            subnets: 1,
            desired: 0,
            target_groups: 1,
            port: 80,
            attached_as: AttachedAs::TargetGroupArns,
        },
        RegistrationRow {
            case: "the documented per-call maximum of target groups at once",
            subnets: 2,
            desired: 1,
            target_groups: MAX_TARGET_GROUPS_PER_CALL,
            port: 1,
            attached_as: AttachedAs::LaterAttach,
        },
    ]
}

/// Every row: each of the group's instances is a target of EVERY target group it is attached
/// to, at that target group's own port and in the zone the instance runs in.
#[tokio::test]
async fn the_instances_of_an_attached_group_are_targets_of_its_target_groups() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-registration").await;

    for (row, case) in registration_rows().into_iter().enumerate() {
        let name = format!("asg-registration-{row}");
        let mut arns = Vec::new();
        for index in 0..case.target_groups {
            arns.push(
                create_target_group(
                    &router,
                    &format!("tg-{row}-{index}"),
                    case.port,
                    TARGET_TYPE_INSTANCE,
                    &vpc_id,
                )
                .await,
            );
        }
        create_group(
            &router,
            &name,
            "lt-registration",
            &subnets[..case.subnets],
            case.desired,
            &arns,
            case.attached_as,
        )
        .await;

        let instances = group_instances(&router, &name).await;
        assert_eq!(
            instances.len(),
            case.desired,
            "case {:?}: expected the group to hold {} instance(s), got {instances:?}",
            case.case,
            case.desired
        );
        let expected = expected_targets(&instances, case.port);
        for arn in &arns {
            let actual = registered_targets(&router, arn).await;
            assert_eq!(
                actual, expected,
                "case {:?}: expected {arn:?} to hold the targets {expected:?}, got {actual:?}",
                case.case
            );
        }
    }
}

/// A target group a group is attached to and then detached from deregisters the group's
/// instances, and the attachment itself is gone from what the group reports — while a target
/// group it is still attached to keeps every one of them.
#[tokio::test]
async fn a_detached_target_group_deregisters_the_groups_instances() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-detach").await;

    let kept = create_target_group(&router, "tg-kept", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    let dropped =
        create_target_group(&router, "tg-dropped", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    create_group(
        &router,
        "asg-detach",
        "lt-detach",
        &subnets,
        2,
        &[kept.clone(), dropped.clone()],
        AttachedAs::TargetGroupArns,
    )
    .await;

    let instances = group_instances(&router, "asg-detach").await;
    let expected = expected_targets(&instances, 80);
    for arn in [&kept, &dropped] {
        let actual = registered_targets(&router, arn).await;
        assert_eq!(
            actual, expected,
            "before the detach, {arn:?} must hold {expected:?}, got {actual:?}"
        );
    }

    let status = detach(&router, "asg-detach", std::slice::from_ref(&dropped)).await;
    assert_eq!(status, 200, "the detach must succeed, got {status}");

    let actual = registered_targets(&router, &dropped).await;
    assert!(
        actual.is_empty(),
        "the detached target group must hold no target of the group, got {actual:?}"
    );
    let actual = registered_targets(&router, &kept).await;
    assert_eq!(
        actual, expected,
        "the target group still attached must keep {expected:?}, got {actual:?}"
    );
    let attached = attached_target_groups(&router, "asg-detach").await;
    assert_eq!(
        attached,
        vec![kept.clone()],
        "the group must report only the attachment it still holds, got {attached:?}"
    );
}

/// An instance that leaves the group is no longer one of its targets: a scale-in takes the
/// instances it terminated out of every target group the group is attached to, and leaves
/// the ones still in service registered.
#[tokio::test]
async fn a_scale_in_deregisters_the_instances_that_left_the_group() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-scale-in").await;

    let arn = create_target_group(&router, "tg-scale-in", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    create_group(
        &router,
        "asg-scale-in",
        "lt-scale-in",
        &subnets,
        3,
        std::slice::from_ref(&arn),
        AttachedAs::TargetGroupArns,
    )
    .await;

    let launched = group_instances(&router, "asg-scale-in").await;
    let actual = registered_targets(&router, &arn).await;
    assert_eq!(
        actual,
        expected_targets(&launched, 80),
        "every launched instance must be a target, got {actual:?}"
    );

    for remaining in [2usize, 1, 0] {
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", "asg-scale-in"),
                ("DesiredCapacity", &remaining.to_string()),
                ("HonorCooldown", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "scaling to {remaining} must succeed, got {status} with {body:?}"
        );

        let actual = registered_targets(&router, &arn).await;
        assert_eq!(
            actual.len(),
            remaining,
            "case {remaining} instance(s) left: expected that many targets, got {actual:?}"
        );
        for target in &actual {
            assert!(
                launched.iter().any(|(id, _)| *id == target.id),
                "case {remaining} instance(s) left: {target:?} is not one of the instances \
                 this group launched ({launched:?})"
            );
        }
    }
}

/// A target group whose targets are NOT instances receives none of the group's instances:
/// an Amazon EC2 instance is a target of an `instance` target group alone, so attaching an
/// `ip` target group is served and registers nothing in it.
#[tokio::test]
async fn a_target_group_whose_targets_are_not_instances_receives_none() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-target-type").await;

    // The `TargetType` values this table spans, from the model's own
    // `TargetTypeEnum` (`smithy_query(query="shape",shape="TargetTypeEnum")` of the
    // Elastic Load Balancing v2 model): the instance type takes the group's instances, and
    // the ones addressed otherwise take none.
    let rows = [
        (TARGET_TYPE_INSTANCE, true),
        ("ip", false),
        ("lambda", false),
    ];
    for (row, (target_type, takes_instances)) in rows.into_iter().enumerate() {
        let name = format!("asg-target-type-{row}");
        let arn = create_target_group(
            &router,
            &format!("tg-type-{row}"),
            80,
            target_type,
            &vpc_id,
        )
        .await;
        create_group(
            &router,
            &name,
            "lt-target-type",
            &subnets,
            1,
            std::slice::from_ref(&arn),
            AttachedAs::TargetGroupArns,
        )
        .await;

        let instances = group_instances(&router, &name).await;
        let expected = if takes_instances {
            expected_targets(&instances, 80)
        } else {
            Vec::new()
        };
        let actual = registered_targets(&router, &arn).await;
        assert_eq!(
            actual, expected,
            "case {target_type:?}: expected the target group to hold {expected:?}, got \
             {actual:?}"
        );
        // Whatever the target type, the attachment itself is the group's own and is
        // reported.
        let attached = attached_target_groups(&router, &name).await;
        assert_eq!(
            attached,
            vec![arn],
            "case {target_type:?}: expected the attachment to be reported, got {attached:?}"
        );
    }
}

/// An attach is ADDITIVE and holds each target group once: the ARNs it names join the ones
/// the group already holds, an ARN already attached changes nothing, and the group's
/// instances are targets of every one of them.
#[tokio::test]
async fn an_attach_is_additive_and_holds_each_target_group_once() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-additive").await;

    let mut arns = Vec::new();
    for index in 0..3 {
        arns.push(
            create_target_group(
                &router,
                &format!("tg-additive-{index}"),
                80,
                TARGET_TYPE_INSTANCE,
                &vpc_id,
            )
            .await,
        );
    }
    create_group(
        &router,
        "asg-additive",
        "lt-additive",
        &subnets,
        2,
        &arns[..1],
        AttachedAs::TargetGroupArns,
    )
    .await;
    let instances = group_instances(&router, "asg-additive").await;
    let expected = expected_targets(&instances, 80);

    // Each row: what the attach names, and what the group must then hold.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one more target group",
            vec![arns[1].clone()],
            arns[..2].to_vec(),
        ),
        (
            "the two it already holds, again",
            arns[..2].to_vec(),
            arns[..2].to_vec(),
        ),
        (
            "one it holds beside one it does not",
            vec![arns[1].clone(), arns[2].clone()],
            arns.clone(),
        ),
        (
            "the same new target group twice in one call",
            vec![arns[2].clone(), arns[2].clone()],
            arns.clone(),
        ),
    ];
    for (case, named, held) in rows {
        let (status, code, message) = attach(&router, "asg-additive", &named).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {code:?} {message:?}"
        );
        let attached = attached_target_groups(&router, "asg-additive").await;
        assert_eq!(
            attached, held,
            "case {case:?}: expected the group to hold {held:?}, got {attached:?}"
        );
        for arn in &attached {
            let actual = registered_targets(&router, arn).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {arn:?} to hold {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The documented per-call bound: an attach naming the maximum is served, and one naming a
/// single target group more is refused `ValidationError` without attaching anything.
#[tokio::test]
async fn an_attach_past_the_documented_per_call_maximum_is_refused() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-bound").await;

    let mut arns = Vec::new();
    for index in 0..=MAX_TARGET_GROUPS_PER_CALL {
        arns.push(
            create_target_group(
                &router,
                &format!("tg-bound-{index}"),
                80,
                TARGET_TYPE_INSTANCE,
                &vpc_id,
            )
            .await,
        );
    }
    create_group(
        &router,
        "asg-bound",
        "lt-bound",
        &subnets,
        1,
        &[],
        AttachedAs::TargetGroupArns,
    )
    .await;

    // One past the bound: refused, and nothing is attached.
    let (status, code, message) = attach(&router, "asg-bound", &arns).await;
    assert_eq!(
        status, 400,
        "an attach of {} target groups must be refused, got {status} with {message:?}",
        arns.len()
    );
    assert_eq!(
        code, "ValidationError",
        "expected the ValidationError code past the bound, got {code:?} {message:?}"
    );
    let expected = format!(
        "Trying to update too many Target Groups at once. The limit is \
         {MAX_TARGET_GROUPS_PER_CALL}"
    );
    assert_eq!(
        message, expected,
        "expected the message {expected:?} past the bound, got {message:?}"
    );
    let attached = attached_target_groups(&router, "asg-bound").await;
    assert!(
        attached.is_empty(),
        "the refusal must attach nothing, got {attached:?}"
    );

    // AT the bound: served, and every named target group is attached.
    let at_bound: Vec<String> = arns[..MAX_TARGET_GROUPS_PER_CALL].to_vec();
    let (status, code, message) = attach(&router, "asg-bound", &at_bound).await;
    assert_eq!(
        status, 200,
        "an attach of {MAX_TARGET_GROUPS_PER_CALL} target groups must succeed, got {status} \
         with {code:?} {message:?}"
    );
    let attached = attached_target_groups(&router, "asg-bound").await;
    assert_eq!(
        attached, at_bound,
        "expected every named target group to be attached, got {attached:?}"
    );
}

/// An attach naming a group the account does not hold is refused like every other
/// group-scoped mutation of this service, and names the group that is missing — and the
/// group that does exist keeps its attachments.
#[tokio::test]
async fn an_attach_of_an_absent_group_is_refused() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-absent").await;

    let arn = create_target_group(&router, "tg-absent", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    create_group(
        &router,
        "asg-present",
        "lt-absent",
        &subnets,
        1,
        std::slice::from_ref(&arn),
        AttachedAs::TargetGroupArns,
    )
    .await;

    for name in ["asg-absent", "ASG-Present", "asg-present-but-longer"] {
        let (status, code, message) = attach(&router, name, std::slice::from_ref(&arn)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {name:?}: expected the ValidationError code, got {code:?} {message:?}"
        );
        // The absent-group refusal this service's group-scoped mutations answer with: the
        // fixed sentence plus the name the request asked for.
        let expected =
            format!("AutoScalingGroup name not found - AutoScalingGroup {name} not found");
        assert_eq!(
            message, expected,
            "case {name:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    let attached = attached_target_groups(&router, "asg-present").await;
    assert_eq!(
        attached,
        vec![arn],
        "the group that exists must keep its attachment, got {attached:?}"
    );
}

/// A described group reports its collections as LISTS: empty for a group that holds nothing
/// in them, and carrying exactly what it holds otherwise. A client reads
/// `AutoScalingGroups[].TargetGroupARNs` off the group it describes, so the member is there
/// whether or not the group is attached to anything.
#[tokio::test]
async fn a_described_group_reports_its_collections_as_lists() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-collections").await;

    let arn = create_target_group(&router, "tg-collections", 80, TARGET_TYPE_INSTANCE, &vpc_id)
        .await;

    // The collection members of `AutoScalingGroup` a described group carries
    // (`smithy_query(query="shape",shape="AutoScalingGroup")`): the attachments, the
    // suspended processes, the enabled metrics, the traffic sources, the termination
    // policies, the zones and the tags.
    let collections = [
        "TargetGroupARNs",
        "LoadBalancerNames",
        "SuspendedProcesses",
        "EnabledMetrics",
        "TrafficSources",
        "TerminationPolicies",
        "AvailabilityZones",
        "Tags",
    ];

    for (case, attachments) in [
        ("a group attached to nothing", Vec::new()),
        ("a group attached to one target group", vec![arn.clone()]),
    ] {
        let name = format!("asg-collections-{}", attachments.len());
        create_group(
            &router,
            &name,
            "lt-collections",
            &subnets,
            0,
            &attachments,
            AttachedAs::TargetGroupArns,
        )
        .await;
        let body = described(&router, &name).await;
        for collection in collections {
            assert!(
                names_element(&body, collection),
                "case {case:?}: expected the described group to report {collection}, got {body}"
            );
        }
        let attached = attached_target_groups(&router, &name).await;
        assert_eq!(
            attached, attachments,
            "case {case:?}: expected TargetGroupARNs to carry {attachments:?}, got {attached:?}"
        );
    }
}

/// The ARN of the `index`-th target group this suite names without creating it: a target
/// group ARN as Elastic Load Balancing spells one, built through the emulator's own ARN
/// writer rather than written out. The attachment bound is about how many ARNs ONE group may
/// HOLD, which no target group has to exist for — the registrations the preceding suites
/// assert are what needs a target group that does.
fn target_group_arn(index: usize) -> String {
    engine::emulator_arn(
        "elasticloadbalancing",
        REGION,
        &format!("targetgroup/tg-bulk-{index}/{index:016x}"),
    )
}

/// The ceiling on what ONE group may HOLD, whatever call put the attachments there: a run of
/// attaches is served up to it and refused past it, a create naming more than it is refused,
/// and the group is left holding exactly what was accepted.
///
/// The rows walk the ceiling itself and the first count past it from both directions — a
/// create that names them outright, and a sequence of attaches that accumulates to them —
/// because the per-call bound says nothing about a run of calls.
#[tokio::test]
async fn one_group_holds_at_most_the_ceiling_of_attachments() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    create_launch_template(&router, "lt-ceiling").await;

    // The message both handlers refuse past the ceiling with, read from the crate so the
    // rows cannot drift from it.
    let refusal = autoscaling::state::too_many_attached_target_groups_message();

    // A run of attaches: served while the group would hold at most the ceiling, refused as
    // soon as it would hold one more.
    create_group(
        &router,
        "asg-ceiling",
        "lt-ceiling",
        &subnets,
        1,
        &[],
        AttachedAs::TargetGroupArns,
    )
    .await;
    let mut held = 0usize;
    while held < MAX_TARGET_GROUPS_PER_GROUP {
        let batch: Vec<String> = (held..(held + MAX_TARGET_GROUPS_PER_CALL)
            .min(MAX_TARGET_GROUPS_PER_GROUP))
            .map(target_group_arn)
            .collect();
        let (status, code, message) = attach(&router, "asg-ceiling", &batch).await;
        assert_eq!(
            status, 200,
            "case {held} already held: attaching {} more must be served, got {status} with \
             {code:?} {message:?}",
            batch.len()
        );
        held += batch.len();
    }
    let attached = attached_target_groups(&router, "asg-ceiling").await;
    assert_eq!(
        attached.len(),
        MAX_TARGET_GROUPS_PER_GROUP,
        "expected the group to hold the ceiling of attachments, got {} of them",
        attached.len()
    );

    let (status, code, message) = attach(
        &router,
        "asg-ceiling",
        std::slice::from_ref(&target_group_arn(MAX_TARGET_GROUPS_PER_GROUP)),
    )
    .await;
    assert_eq!(
        status, 400,
        "one attachment past the ceiling must be refused, got {status} with {code:?} \
         {message:?}"
    );
    assert_eq!(
        code, "ValidationError",
        "expected the ValidationError code past the ceiling, got {code:?} {message:?}"
    );
    assert_eq!(
        message, refusal,
        "expected the message {refusal:?} past the ceiling, got {message:?}"
    );
    let attached = attached_target_groups(&router, "asg-ceiling").await;
    assert_eq!(
        attached.len(),
        MAX_TARGET_GROUPS_PER_GROUP,
        "the refusal must attach nothing, got {} attachments",
        attached.len()
    );

    // A create naming them outright: the ceiling is served and one more is refused with the
    // modelled limit error, under either collection the attachment is named in.
    let rows: Vec<(&str, usize, AttachedAs, u16, &str)> = vec![
        (
            "a create naming the ceiling as TargetGroupARNs",
            MAX_TARGET_GROUPS_PER_GROUP,
            AttachedAs::TargetGroupArns,
            200,
            "",
        ),
        (
            "a create naming one more as TargetGroupARNs",
            MAX_TARGET_GROUPS_PER_GROUP + 1,
            AttachedAs::TargetGroupArns,
            400,
            "LimitExceeded",
        ),
        (
            "a create naming one more as elbv2 traffic sources",
            MAX_TARGET_GROUPS_PER_GROUP + 1,
            AttachedAs::Elbv2TrafficSources,
            400,
            "LimitExceeded",
        ),
    ];
    for (row, (case, count, attached_as, expected_status, expected_code)) in
        rows.into_iter().enumerate()
    {
        let name = format!("asg-ceiling-create-{row}");
        let arns: Vec<String> = (0..count).map(target_group_arn).collect();
        let mut params = vec![
            p("AutoScalingGroupName", name.clone()),
            p("LaunchTemplate.LaunchTemplateName", "lt-ceiling"),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("DesiredCapacity", "0"),
            p("VPCZoneIdentifier", subnets[0].id.clone()),
        ];
        for (index, arn) in arns.iter().enumerate() {
            match attached_as {
                AttachedAs::Elbv2TrafficSources => {
                    params.push(p(
                        format!("TrafficSources.member.{}.Identifier", index + 1).as_str(),
                        arn.clone(),
                    ));
                    params.push(p(
                        format!("TrafficSources.member.{}.Type", index + 1).as_str(),
                        "elbv2",
                    ));
                }
                _ => params.push(p(
                    format!("TargetGroupARNs.member.{}", index + 1).as_str(),
                    arn.clone(),
                )),
            }
        }
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body:?}"
        );
        if expected_status == 200 {
            let attached = attached_target_groups(&router, &name).await;
            let expected = if attached_as == AttachedAs::TargetGroupArns {
                count
            } else {
                0
            };
            assert_eq!(
                attached.len(),
                expected,
                "case {case:?}: expected {expected} attachment(s) reported, got {}",
                attached.len()
            );
            continue;
        }
        let code = tag_value(&body, "Code");
        let message = tag_value(&body, "Message");
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code} code, got {code:?} {message:?}"
        );
        assert_eq!(
            message, refusal,
            "case {case:?}: expected the message {refusal:?}, got {message:?}"
        );
    }
}

/// How often a poll re-reads the group.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: the lifecycle span this service owes a
/// transition, with ample margin for a loaded test machine. Read from the service's own span
/// rather than written out, so the two cannot drift apart.
fn poll_budget() -> Duration {
    autoscaling::state::LIFECYCLE_TRANSITION + Duration::from_secs(10)
}

/// The `LifecycleState` of an instance that is in service, from the model's own
/// `LifecycleState` enum (`smithy_query(query="shape",shape="LifecycleState")`).
const LIFECYCLE_IN_SERVICE: &str = "InService";

/// The `ScalingActivityStatusCode` of an activity that did not place what it set out to,
/// from the model's own `ScalingActivityStatusCode` enum
/// (`smithy_query(query="shape",shape="ScalingActivityStatusCode")`).
const ACTIVITY_FAILED: &str = "Failed";

/// Post one `SetDesiredCapacity` and answer its status, wire error code and message.
async fn set_desired_capacity(
    router: &MultiServiceRouter,
    name: &str,
    desired: usize,
) -> (u16, String, String) {
    let (status, body) = query(
        router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", name),
            ("DesiredCapacity", &desired.to_string()),
            ("HonorCooldown", "false"),
        ],
    )
    .await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Delete one target group through Elastic Load Balancing v2's own wire — the service that
/// owns it is what takes it away, so the attachment the Auto Scaling group still holds is
/// left naming a target group that no longer exists.
async fn delete_target_group(router: &MultiServiceRouter, arn: &str) {
    let (status, body) =
        elbv2_query(router, "DeleteTargetGroup", &[p("TargetGroupArn", arn)]).await;
    assert_eq!(
        status, 200,
        "DeleteTargetGroup for {arn:?} expected 200, got {status}: {body}"
    );
}

/// One scaling activity as a client reads it: what it set out to do, its status code and
/// the message a failed one carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedActivity {
    description: String,
    status_code: String,
    status_message: String,
}

/// Every scaling activity the group reports, through `DescribeScalingActivities`.
async fn scaling_activities(router: &MultiServiceRouter, name: &str) -> Vec<ReportedActivity> {
    let (status, body) = query(
        router,
        "DescribeScalingActivities",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeScalingActivities for {name:?} expected 200, got {status}: {body}"
    );
    let inner = element_inner(&body, "Activities").unwrap_or_default();
    members(inner)
        .into_iter()
        .map(|member| ReportedActivity {
            description: xml_unescape(&tag_value(member, "Description")),
            status_code: tag_value(member, "StatusCode"),
            status_message: xml_unescape(&tag_value(member, "StatusMessage")),
        })
        .collect()
}

/// The activities of the group that FAILED, in the order it reports them.
async fn failed_activities(router: &MultiServiceRouter, name: &str) -> Vec<ReportedActivity> {
    scaling_activities(router, name)
        .await
        .into_iter()
        .filter(|activity| activity.status_code == ACTIVITY_FAILED)
        .collect()
}

/// Every instance the group reports, as `(instance id, LifecycleState)` in the order
/// reported.
async fn instance_states(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let body = described(router, name).await;
    let inner = element_inner(&body, "Instances").unwrap_or_default();
    members(inner)
        .into_iter()
        .map(|member| {
            (
                tag_value(member, "InstanceId"),
                tag_value(member, "LifecycleState"),
            )
        })
        .collect()
}

/// Read the group until it holds `wanted` instances and every one of them is in service, or
/// a generous deadline passes — the CONDITION, never a fixed wait. Answers what it last read.
async fn settled_instances(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    wanted: usize,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + poll_budget();
    loop {
        let held = instance_states(router, name).await;
        let settled = held.len() == wanted
            && held
                .iter()
                .all(|(_, state)| state == LIFECYCLE_IN_SERVICE);
        if settled || Instant::now() >= deadline {
            assert_eq!(
                held.len(),
                wanted,
                "case {case:?}: expected {name:?} to hold {wanted} instance(s), got {held:?}"
            );
            for (instance_id, state) in &held {
                assert_eq!(
                    state, LIFECYCLE_IN_SERVICE,
                    "case {case:?}: expected {instance_id} of {name:?} to reach \
                     {LIFECYCLE_IN_SERVICE}, got {held:?}"
                );
            }
            return held;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The ids of the targets Elastic Load Balancing v2 reports in `arn`, sorted — read through
/// its own `DescribeTargetHealth`.
async fn registered_ids(router: &MultiServiceRouter, arn: &str) -> Vec<String> {
    let mut ids: Vec<String> = registered_targets(router, arn)
        .await
        .into_iter()
        .map(|target| target.id)
        .collect();
    ids.sort();
    ids
}

/// The ids of the instances the group reports, sorted — what every target group it is
/// attached to must hold.
fn instance_ids(instances: &[(String, String)]) -> Vec<String> {
    let mut ids: Vec<String> = instances.iter().map(|(id, _)| id.clone()).collect();
    ids.sort();
    ids
}

/// The target groups the group holds an attachment to, in EITHER form the attachment takes
/// — its own `TargetGroupARNs` and an `elbv2` traffic source — which is what a registration
/// of its instances follows.
async fn attached_identifiers(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let body = described(router, name).await;
    let mut held = tag_values(
        element_inner(&body, "TargetGroupARNs").unwrap_or_default(),
        "member",
    );
    for source in members(element_inner(&body, "TrafficSources").unwrap_or_default()) {
        if tag_value(source, "Type") == TRAFFIC_SOURCE_TYPE_ELBV2 {
            held.push(tag_value(source, "Identifier"));
        }
    }
    held
}

/// The `DesiredCapacity` the group reports.
async fn reported_desired_capacity(router: &MultiServiceRouter, name: &str) -> String {
    tag_value(&described(router, name).await, "DesiredCapacity")
}

/// The `StatusMessage` the failed activity of a launch blocked by deleted target groups
/// carries: Elastic Load Balancing's own `TargetGroupNotFound` refusal for the identifiers
/// a call names — one identifier, or several, under the plural noun, with the service,
/// status and code an AWS SDK renders beside it — followed by the launch Amazon EC2 Auto
/// Scaling therefore did not make.
fn blocked_message(unresolved: &[String]) -> String {
    format!(
        "Target groups '{}' not found (Service: ElasticLoadBalancingV2, Status Code: 400, Error \
         Code: TargetGroupNotFound). Launching EC2 instance failed.",
        unresolved.join(", ")
    )
}

/// What the activity of that launch DESCRIBES: the launch it set out to make and the
/// status reason it failed with — the two members `DescribeScalingActivities` reports
/// apart, where the activity of a launch that succeeded names the instance it launched.
fn blocked_description(unresolved: &[String]) -> String {
    format!(
        "Launching a new EC2 instance. Status Reason: {}",
        blocked_message(unresolved)
    )
}

/// One row of the deleted-attachment table: how many target groups the group is attached to,
/// which of them are deleted out of band, how the attachment was made, how many instances the
/// group holds before the scale-out, and the capacity the scale-out asks for.
struct DeletedAttachmentRow {
    case: &'static str,
    target_groups: usize,
    /// The indexes, into the row's own target groups, of the ones deleted before the
    /// scale-out. Empty for a row whose every attachment still resolves.
    deleted: &'static [usize],
    attached_as: AttachedAs,
    held: usize,
    desired: usize,
}

fn deleted_attachment_rows() -> Vec<DeletedAttachmentRow> {
    vec![
        DeletedAttachmentRow {
            case: "the only attached target group is deleted",
            target_groups: 1,
            deleted: &[0],
            attached_as: AttachedAs::TargetGroupArns,
            held: 1,
            desired: 2,
        },
        DeletedAttachmentRow {
            case: "one of two attached target groups is deleted",
            target_groups: 2,
            deleted: &[1],
            attached_as: AttachedAs::TargetGroupArns,
            held: 1,
            desired: 2,
        },
        DeletedAttachmentRow {
            case: "both attached target groups are deleted, scaling out by two",
            target_groups: 2,
            deleted: &[0, 1],
            attached_as: AttachedAs::TargetGroupArns,
            held: 2,
            desired: 4,
        },
        DeletedAttachmentRow {
            case: "the deleted target group is named as an elbv2 traffic source",
            target_groups: 1,
            deleted: &[0],
            attached_as: AttachedAs::Elbv2TrafficSources,
            held: 1,
            desired: 2,
        },
        DeletedAttachmentRow {
            case: "the deleted target group was attached by a later attach",
            target_groups: 1,
            deleted: &[0],
            attached_as: AttachedAs::LaterAttach,
            held: 1,
            desired: 3,
        },
        DeletedAttachmentRow {
            case: "every attached target group still exists",
            target_groups: 2,
            deleted: &[],
            attached_as: AttachedAs::TargetGroupArns,
            held: 1,
            desired: 2,
        },
        DeletedAttachmentRow {
            case: "the group is attached to nothing at all",
            target_groups: 0,
            deleted: &[],
            attached_as: AttachedAs::TargetGroupArns,
            held: 1,
            desired: 3,
        },
    ]
}

/// Every row: a scale-out of a group holding an attachment whose target group is GONE
/// launches nothing and records one failed activity per instance it was asked for, naming
/// the target groups that are missing — while the group's stored capacity still moves, the
/// instances it already holds stay in service and registered, and the attachment that can
/// never resolve again is no longer one the group holds. A row whose every attachment
/// resolves scales out as it always did.
#[tokio::test]
async fn a_scale_out_is_refused_while_an_attached_target_group_is_gone() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-deleted").await;

    for (row, case) in deleted_attachment_rows().into_iter().enumerate() {
        let name = format!("asg-deleted-{row}");
        let mut arns = Vec::new();
        for index in 0..case.target_groups {
            arns.push(
                create_target_group(
                    &router,
                    &format!("tg-deleted-{row}-{index}"),
                    80,
                    TARGET_TYPE_INSTANCE,
                    &vpc_id,
                )
                .await,
            );
        }
        create_group(
            &router,
            &name,
            "lt-deleted",
            &subnets,
            case.held,
            &arns,
            case.attached_as,
        )
        .await;
        let before = settled_instances(&router, case.case, &name, case.held).await;
        // Every attachment holds the instances the group launched before anything was
        // deleted, so what follows is a change the delete made and not a group that never
        // registered.
        for arn in &arns {
            let actual = registered_ids(&router, arn).await;
            assert_eq!(
                actual,
                instance_ids(&before),
                "case {:?}: expected {arn:?} to hold the group's instances before the delete, \
                 got {actual:?}",
                case.case
            );
        }

        let gone: Vec<String> = case
            .deleted
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        for arn in &gone {
            delete_target_group(&router, arn).await;
        }
        let kept: Vec<String> = arns
            .iter()
            .filter(|arn| !gone.contains(arn))
            .cloned()
            .collect();

        let (status, code, message) = set_desired_capacity(&router, &name, case.desired).await;
        assert_eq!(
            status, 200,
            "case {:?}: the capacity change itself must be served, got {status} with {code:?} \
             {message:?}",
            case.case
        );
        // The capacity the group was asked for is the capacity it holds, whether or not the
        // launch it owes could be made: the request was accepted.
        let reported = reported_desired_capacity(&router, &name).await;
        assert_eq!(
            reported,
            case.desired.to_string(),
            "case {:?}: expected the group to report DesiredCapacity {}, got {reported:?}",
            case.case,
            case.desired
        );

        let failed = failed_activities(&router, &name).await;
        if gone.is_empty() {
            // Nothing blocks the launch, so the scale-out happens and nothing failed.
            let after = settled_instances(&router, case.case, &name, case.desired).await;
            assert!(
                failed.is_empty(),
                "case {:?}: expected no failed activity, got {failed:?}",
                case.case
            );
            for arn in &kept {
                let actual = registered_ids(&router, arn).await;
                assert_eq!(
                    actual,
                    instance_ids(&after),
                    "case {:?}: expected {arn:?} to hold every instance after the scale-out, got \
                     {actual:?}",
                    case.case
                );
            }
            continue;
        }

        // Blocked: nothing was launched, and the instances the group already holds are
        // exactly the ones it still reports, still in service.
        let after = instance_states(&router, &name).await;
        assert_eq!(
            after, before,
            "case {:?}: expected the group to still hold {before:?} and launch nothing, got \
             {after:?}",
            case.case
        );
        let wanted = blocked_message(&gone);
        let wanted_description = blocked_description(&gone);
        assert_eq!(
            failed.len(),
            case.desired - case.held,
            "case {:?}: expected one failed activity per instance the change asked for, got \
             {failed:?}",
            case.case
        );
        for activity in &failed {
            assert_eq!(
                activity.status_message, wanted,
                "case {:?}: expected the failed activity to report {wanted:?}, got {activity:?}",
                case.case
            );
            assert_eq!(
                activity.description, wanted_description,
                "case {:?}: expected the failed activity to describe {wanted_description:?}, got \
                 {activity:?}",
                case.case
            );
        }
        // The attachments that DO resolve are untouched by the refusal: they hold the
        // instances the group already had, and nothing was registered for a launch that
        // never happened.
        for arn in &kept {
            let actual = registered_ids(&router, arn).await;
            assert_eq!(
                actual,
                instance_ids(&before),
                "case {:?}: expected {arn:?} to hold only the instances the group already had, \
                 got {actual:?}",
                case.case
            );
        }
        // The attachment that can never resolve again is no longer one the group holds,
        // exactly as a detach of it would leave the group, while every attachment that
        // does resolve is still there.
        let attached = attached_identifiers(&router, &name).await;
        for arn in &gone {
            assert!(
                !attached.contains(arn),
                "case {:?}: expected the group to hold no attachment to the deleted {arn:?}, \
                 got {attached:?}",
                case.case
            );
        }
        for arn in &kept {
            assert!(
                attached.contains(arn),
                "case {:?}: expected the group to keep its attachment to {arn:?}, got \
                 {attached:?}",
                case.case
            );
        }
    }
}

/// The refusal stands until the client asks again: while the group holds an attachment
/// Elastic Load Balancing v2 cannot resolve the scale-out launches nothing, and once that
/// attachment has ended with the target group it named — and a fresh target group is
/// attached — the same scale-out launches the instance and registers it there.
#[tokio::test]
async fn a_scale_out_resumes_once_no_attachment_is_missing() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    let vpc_id = vpc_of(&router, &subnets[0]).await;
    create_launch_template(&router, "lt-resumes").await;

    let gone = create_target_group(&router, "tg-resumes", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    create_group(
        &router,
        "asg-resumes",
        "lt-resumes",
        &subnets,
        1,
        std::slice::from_ref(&gone),
        AttachedAs::TargetGroupArns,
    )
    .await;
    let before = settled_instances(&router, "before the delete", "asg-resumes", 1).await;

    delete_target_group(&router, &gone).await;
    let (status, code, message) = set_desired_capacity(&router, "asg-resumes", 2).await;
    assert_eq!(
        status, 200,
        "the blocked capacity change must be served, got {status} with {code:?} {message:?}"
    );
    let blocked = failed_activities(&router, "asg-resumes").await;
    let wanted = blocked_message(std::slice::from_ref(&gone));
    assert_eq!(
        blocked.len(),
        1,
        "the blocked scale-out must record one failed activity, got {blocked:?}"
    );
    assert_eq!(
        blocked[0].status_message, wanted,
        "expected the failed activity to report {wanted:?}, got {:?}",
        blocked[0]
    );
    let held = instance_states(&router, "asg-resumes").await;
    assert_eq!(
        held, before,
        "the blocked scale-out must launch nothing, got {held:?}"
    );
    // Nothing launches for that change afterwards either: the refusal was the whole of what
    // it did, so the group stays exactly as it left it and reports the refusal once. Watched
    // over a budget several times the span this service owes a transition, so a launch the
    // clock could still have made falls inside it.
    let quiet = Instant::now() + autoscaling::state::LIFECYCLE_TRANSITION * 8;
    while Instant::now() < quiet {
        tokio::time::sleep(POLL_INTERVAL).await;
        let held = instance_states(&router, "asg-resumes").await;
        assert_eq!(
            held, before,
            "the refused scale-out must launch nothing afterwards either, got {held:?}"
        );
        let still = failed_activities(&router, "asg-resumes").await;
        assert_eq!(
            still, blocked,
            "the refused scale-out must report its refusal once, got {still:?}"
        );
    }
    // The attachment that could not resolve has ended with the target group it named.
    let attached = attached_identifiers(&router, "asg-resumes").await;
    assert!(
        attached.is_empty(),
        "expected the group to hold no attachment to the deleted target group, got \
         {attached:?}"
    );

    // A fresh target group takes its place and the client asks for the capacity again:
    // nothing is in the way now, so the launch is made and the instance is registered there.
    let fresh = create_target_group(&router, "tg-fresh", 80, TARGET_TYPE_INSTANCE, &vpc_id).await;
    let (status, code, message) = attach(&router, "asg-resumes", std::slice::from_ref(&fresh)).await;
    assert_eq!(
        status, 200,
        "attaching the fresh target group must succeed, got {status} with {code:?} {message:?}"
    );
    let (status, code, message) = set_desired_capacity(&router, "asg-resumes", 2).await;
    assert_eq!(
        status, 200,
        "the capacity change asked for again must be served, got {status} with {code:?} \
         {message:?}"
    );

    let after = settled_instances(&router, "after the fresh attachment", "asg-resumes", 2).await;
    let actual = registered_ids(&router, &fresh).await;
    assert_eq!(
        actual,
        instance_ids(&after),
        "expected the fresh target group to hold every instance of the group, got {actual:?}"
    );
    let failed = failed_activities(&router, "asg-resumes").await;
    assert_eq!(
        failed, blocked,
        "the resumed scale-out must record no further failure, got {failed:?}"
    );
}
