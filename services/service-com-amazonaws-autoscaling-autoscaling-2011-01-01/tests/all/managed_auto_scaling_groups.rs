// SPDX-License-Identifier: CC0-1.0

//! `IncludeManagedResources` — whether a read reports the Auto Scaling groups another
//! service MANAGES, and the resources that hang off them — over the awsQuery wire.
//!
//! THE RULE UNDER TEST: an Auto Scaling group created with an `Operator` is managed by that
//! service principal ("describes the entity that manages an Auto Scaling group",
//! `smithy_query(query="shape",shape="Operator")`), and every read of this service that
//! documents `IncludeManagedResources` says the same thing about it: "Specifies whether to
//! include managed Auto Scaling groups in the response. When set to `true` (default), the
//! response includes managed Auto Scaling groups"
//! (`smithy_query(query="doc",shape="AutoScalingGroupNamesType",member="IncludeManagedResources")`,
//! and the reads of a group's instances, tags, scaling policies, scheduled actions, scaling
//! activities and notification configurations each state it of what they report). So a read
//! that states nothing, and one that states `true`, report every group of the account and
//! every item recorded against it, while one that states `false` reports only what belongs to
//! the groups the account maintains itself — whatever else the request narrowed the read to.
//! Narrowing is all the member does: a read changes nothing a later caller sees.
//!
//! The tables span what the rule generalises over: WHICH of the seven reads is made, what the
//! member states (nothing at all, `true`, `false`), which service principal manages a group
//! (two different ones) and whether one manages it at all, and whether the request also
//! narrows the read to one group — by name, by an instance id, or by a tag filter.
//!
//! Every group here is created over the wire and read back through
//! `DescribeAutoScalingGroups`, so each row follows state the service stored rather than a
//! value this suite invented.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, create_launch_template, emulator, p, query, seed_subnets, tag_values, REGION,
};

/// How long a case waits for the instance a group launches to come into service before
/// calling it a failure. A BACKSTOP, not a wait: the helper below looks AGAIN until the
/// condition holds.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long one look waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The launch template every group of this suite launches from, created over Amazon EC2's own
/// wire so the groups launch from a template the account really holds.
const TEMPLATE: &str = "lt-managed-resources";

/// The `LifecycleState` an instance of a group reaches once it is in service.
const IN_SERVICE: &str = "InService";

/// A scheduled start time far enough ahead that the action stays scheduled: a scheduled
/// action is put with a start time in the future.
const SCHEDULED_START: &str = "2035-06-01T12:00:00Z";

/// One group of the account: its name, and the service principal that manages it where one
/// does.
struct Seeded {
    name: &'static str,
    /// `Operator$Principal` is "the service principal that is authorized to manage the Auto
    /// Scaling group"
    /// (`smithy_query(query="doc",shape="Operator",member="Principal")`), so a managed group
    /// names one and a group the account maintains itself names none.
    operator: Option<&'static str>,
}

/// The groups every row reads over: two managed by DIFFERENT service principals, and one the
/// account maintains itself, so no row can pass by keying on one principal.
const SEEDED: [Seeded; 3] = [
    Seeded {
        name: "asg-operated-by-container-service",
        operator: Some("ecs.amazonaws.com"),
    },
    Seeded {
        name: "asg-operated-by-kubernetes-service",
        operator: Some("eks.amazonaws.com"),
    },
    Seeded {
        name: "asg-self-managed",
        operator: None,
    },
];

/// Every group of the account, in the order a sorted read reports them.
fn every_group() -> Vec<String> {
    sorted(SEEDED.iter().map(|group| group.name))
}

/// The groups the account maintains itself: what a read stating `false` reports.
fn self_managed_groups() -> Vec<String> {
    sorted(
        SEEDED
            .iter()
            .filter(|group| group.operator.is_none())
            .map(|group| group.name),
    )
}

/// The groups a service principal manages.
fn managed_groups() -> Vec<String> {
    sorted(
        SEEDED
            .iter()
            .filter(|group| group.operator.is_some())
            .map(|group| group.name),
    )
}

/// A set of names in the one order every assertion here compares in.
fn sorted<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = names.map(str::to_string).collect();
    names.sort();
    names.dedup();
    names
}

/// One read of this service that documents `IncludeManagedResources`: the action a client
/// posts, and the member each reported item names its group under.
struct Read {
    action: &'static str,
    group_member: &'static str,
}

/// The seven reads the member is modelled on
/// (`smithy_query(query="shape",shape="AutoScalingGroupNamesType,DescribeAutoScalingInstancesType,DescribeTagsType,DescribePoliciesType,DescribeScheduledActionsType,DescribeScalingActivitiesType,DescribeNotificationConfigurationsType")`).
const READS: [Read; 7] = [
    Read {
        action: "DescribeAutoScalingGroups",
        group_member: "AutoScalingGroupName",
    },
    Read {
        action: "DescribeAutoScalingInstances",
        group_member: "AutoScalingGroupName",
    },
    // A tag names the group it is on as the resource it describes.
    Read {
        action: "DescribeTags",
        group_member: "ResourceId",
    },
    Read {
        action: "DescribePolicies",
        group_member: "AutoScalingGroupName",
    },
    Read {
        action: "DescribeScheduledActions",
        group_member: "AutoScalingGroupName",
    },
    Read {
        action: "DescribeScalingActivities",
        group_member: "AutoScalingGroupName",
    },
    Read {
        action: "DescribeNotificationConfigurations",
        group_member: "AutoScalingGroupName",
    },
];

/// What a request states for `IncludeManagedResources`, and how a case names it.
const STATED: [(&str, Option<bool>); 3] = [
    ("the member left out altogether", None),
    ("the member stating true", Some(true)),
    ("the member stating false", Some(false)),
];

/// That member as the parameter a client sends, where the case states it at all.
fn member_params(stated: Option<bool>) -> Vec<(String, String)> {
    stated
        .map(|value| {
            vec![p(
                "IncludeManagedResources",
                if value { "true" } else { "false" },
            )]
        })
        .unwrap_or_default()
}

/// The groups one read reports items for, sorted and de-duplicated, so a read reporting
/// several items per group is compared as the set of groups it reported.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    read: &Read,
    extra: &[(String, String)],
) -> Vec<String> {
    let (status, body) = query(router, read.action, &borrowed(extra)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} expected HTTP 200, got {status} with {body:?}",
        read.action
    );
    sorted(tag_values(&body, read.group_member).iter().map(String::as_str))
}

/// The request members that narrow one read to the single group `group`, whose instance is
/// `instance_id`: each read's own way of asking for one group's items.
fn narrowed_to(read: &Read, group: &str, instance_id: &str) -> Vec<(String, String)> {
    match read.action {
        "DescribeAutoScalingGroups" | "DescribeNotificationConfigurations" => {
            vec![p("AutoScalingGroupNames.member.1", group)]
        }
        "DescribeAutoScalingInstances" => vec![p("InstanceIds.member.1", instance_id)],
        // The tag read selects by the tag's own members, of which the group it is on is one:
        // "auto-scaling-group" (`smithy_query(query="doc",shape="Filter",member="Name")`).
        "DescribeTags" => vec![
            p("Filters.member.1.Name", "auto-scaling-group"),
            p("Filters.member.1.Values.member.1", group),
        ],
        _ => vec![p("AutoScalingGroupName", group)],
    }
}

/// Create one group over the wire, placed in `zone`, carrying one tag and holding one
/// instance, and managed by its operator where it has one.
async fn create_group(router: &MultiServiceRouter, group: &Seeded, zone: &str) {
    let mut params = vec![
        p("AutoScalingGroupName", group.name),
        p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        p("MinSize", "1"),
        p("MaxSize", "2"),
        p("DesiredCapacity", "1"),
        p("AvailabilityZones.member.1", zone),
        p("Tags.member.1.Key", "owner"),
        p("Tags.member.1.Value", group.name),
        p("Tags.member.1.PropagateAtLaunch", "false"),
    ];
    if let Some(principal) = group.operator {
        params.push(p("Operator.Principal", principal));
    }
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {:?}: expected HTTP 200, got {status} with {body:?}",
        group.name
    );
}

/// Put one simple scaling policy on `group`.
async fn put_policy(router: &MultiServiceRouter, group: &str) {
    let params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", format!("{group}-policy")),
        p("PolicyType", "SimpleScaling"),
        p("AdjustmentType", "ChangeInCapacity"),
        p("ScalingAdjustment", "1"),
    ];
    let (status, body) = query(router, "PutScalingPolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "putting a policy on {group:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// Put one scheduled action on `group`.
async fn put_scheduled_action(router: &MultiServiceRouter, group: &str) {
    let params = vec![
        p("AutoScalingGroupName", group),
        p("ScheduledActionName", format!("{group}-action")),
        p("StartTime", SCHEDULED_START),
        p("DesiredCapacity", "1"),
    ];
    let (status, body) = query(
        router,
        "PutScheduledUpdateGroupAction",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "putting a scheduled action on {group:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// Configure `group` to notify one Amazon SNS topic of the account.
async fn put_notification_configuration(router: &MultiServiceRouter, group: &str) {
    let topic = format!(
        "arn:aws:sns:{REGION}:{}:{group}-topic",
        engine::emulator_account_id()
    );
    let params = vec![
        p("AutoScalingGroupName", group),
        p("TopicARN", topic),
        p("NotificationTypes.member.1", "autoscaling:TEST_NOTIFICATION"),
    ];
    let (status, body) = query(router, "PutNotificationConfiguration", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "configuring notifications on {group:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// Look AGAIN until `group` reports one instance in service, and answer that instance's id
/// and the principal the group reports as its operator.
async fn settled(router: &MultiServiceRouter, group: &str) -> (String, Vec<String>) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, body) = query(
            router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group)],
        )
        .await;
        assert_eq!(
            status, 200,
            "reading {group:?} back: expected HTTP 200, got {status} with {body:?}"
        );
        let states = tag_values(&body, "LifecycleState");
        if states.len() == 1 && states[0] == IN_SERVICE {
            let instance_id = tag_values(&body, "InstanceId")
                .first()
                .cloned()
                .unwrap_or_default();
            assert!(
                !instance_id.is_empty(),
                "the member of {group:?} must report its instance id, got {body:?}"
            );
            return (instance_id, tag_values(&body, "Principal"));
        }
        assert!(
            Instant::now() < deadline,
            "expected {group:?} to report one instance in service, got {states:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The account every case reads: the three groups above, each carrying a tag, an instance in
/// service, a scaling policy, a scheduled action and a notification configuration — so each
/// of the seven reads has an item of every group to report — and the instance id of each.
async fn account() -> (MultiServiceRouter, Vec<(String, String)>) {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 1);
    let zone = subnets.first().expect("one seeded subnet").zone.clone();
    create_launch_template(&router, TEMPLATE).await;

    // Every group is created before any of them is waited on, so the account settles in about
    // one span rather than one per group.
    for group in &SEEDED {
        create_group(&router, group, &zone).await;
    }
    let mut instances: Vec<(String, String)> = Vec::new();
    for group in &SEEDED {
        put_policy(&router, group.name).await;
        put_scheduled_action(&router, group.name).await;
        put_notification_configuration(&router, group.name).await;
        let (instance_id, principals) = settled(&router, group.name).await;
        // The operator the create stated is what the group reports, which is what makes it a
        // managed group at all: a group created without one reports none.
        assert_eq!(
            principals,
            group.operator.map(str::to_string).into_iter().collect::<Vec<String>>(),
            "group {:?}: expected the stored operator principal to round trip, got {principals:?}",
            group.name
        );
        instances.push((group.name.to_string(), instance_id));
    }
    (router, instances)
}

/// The instance id of one group of that account.
fn instance_of(instances: &[(String, String)], group: &str) -> String {
    instances
        .iter()
        .find(|(name, _)| name == group)
        .map(|(_, instance_id)| instance_id.clone())
        .expect("every seeded group holds one instance")
}

#[tokio::test]
async fn include_managed_resources_decides_which_groups_every_read_reports() {
    let (router, _instances) = account().await;

    for read in &READS {
        for (stated_case, stated) in STATED {
            let case = format!("{} with {stated_case}", read.action);
            let expected = if stated == Some(false) {
                self_managed_groups()
            } else {
                every_group()
            };
            let actual = reported(&router, &case, read, &member_params(stated)).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the read to report items for {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_narrowed_to_a_managed_group_reports_it_only_when_asked() {
    let (router, instances) = account().await;

    for read in &READS {
        for group in managed_groups() {
            let instance_id = instance_of(&instances, &group);
            let narrowed = narrowed_to(read, &group, &instance_id);

            // Narrowed to that one group and asked for the managed resources, the read
            // reports it — whether the member says so or says nothing.
            for (stated_case, stated) in [STATED[0], STATED[1]] {
                let case = format!("{} narrowed to {group} with {stated_case}", read.action);
                let mut params = narrowed.clone();
                params.extend(member_params(stated));
                let actual = reported(&router, &case, read, &params).await;
                assert_eq!(
                    actual,
                    vec![group.clone()],
                    "case {case:?}: expected the read to report items for {group:?}, got {actual:?}"
                );
            }

            // Asked to leave the managed resources out, the same narrowed read reports
            // nothing: the member narrows the read whatever else it was narrowed by.
            let case = format!("{} narrowed to {group} with the member stating false", read.action);
            let mut params = narrowed.clone();
            params.extend(member_params(Some(false)));
            let actual = reported(&router, &case, read, &params).await;
            assert!(
                actual.is_empty(),
                "case {case:?}: expected the read to report no item, got {actual:?}"
            );
        }

        // A read is a read: one that left the managed resources out changes nothing the next
        // caller sees.
        let case = format!("{} read again after a read stating false", read.action);
        let actual = reported(&router, &case, read, &[]).await;
        assert_eq!(
            actual,
            every_group(),
            "case {case:?}: expected the read to report items for {:?}, got {actual:?}",
            every_group()
        );
    }
}
