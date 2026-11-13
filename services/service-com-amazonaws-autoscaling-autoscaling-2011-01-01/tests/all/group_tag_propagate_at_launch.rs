// SPDX-License-Identifier: CC0-1.0

//! Every tag an Auto Scaling group carries reports whether it propagates at launch.
//!
//! The RULE under test: `PropagateAtLaunch` "determines whether the tag is added to new
//! instances as they are launched in the group" (`Tag$PropagateAtLaunch` /
//! `TagDescription$PropagateAtLaunch`), and that decision is the SERVICE's: it is taken
//! when the tag is stored and reported on every read of the tag, so a tag created without
//! the member takes the service's default for it — the tag propagates — rather than
//! reporting nothing at all. One consequence is read here twice over — through the group's
//! own read (`DescribeAutoScalingGroups.AutoScalingGroups[].Tags[]`) and through the
//! account-wide read (`DescribeTags.Tags[]`) — and once more where the flag is acted on:
//! the instances a capacity change launches carry exactly the tags whose stored decision
//! is `true`.
//!
//! The rows span what the rule generalises over: a tag whose create states `true`, one
//! whose create states `false`, one whose create states nothing at all, and one that
//! states nothing and carries an empty value — so the table differs along both the form of
//! the request and the value of the flag, and the two stated rows pin that a stated
//! decision is kept while the unstated ones pin the default.

use std::sync::Arc;
use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::support::{borrowed, ec2_query, p, query, seed_subnets, tag_value, tag_values};

/// The `ResourceType` every tag of an Auto Scaling group is reported under, from the
/// model's own documentation for `TagDescription$ResourceType`: "The only supported value
/// is auto-scaling-group". Spelled out here rather than imported, so a wrong constant in
/// the implementation cannot make a test pass.
const GROUP_RESOURCE_TYPE: &str = "auto-scaling-group";

/// One tag of the group under test: how the create states it, and the decision the service
/// must then store and report for it.
struct Row {
    case: &'static str,
    key: &'static str,
    value: &'static str,
    /// `PropagateAtLaunch` as the create states it, or `None` when the create states none.
    requested: Option<bool>,
    /// The flag every read of the tag must report, and whether the tag reaches the
    /// instances the group launches.
    expected: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a tag whose create states PropagateAtLaunch=true",
        key: "env",
        value: "prod",
        requested: Some(true),
        expected: true,
    },
    Row {
        case: "a tag whose create states PropagateAtLaunch=false",
        key: "team",
        value: "platform",
        requested: Some(false),
        expected: false,
    },
    Row {
        case: "a tag whose create states no PropagateAtLaunch at all",
        key: "Customer-SG",
        value: "customer-identifier",
        requested: None,
        expected: true,
    },
    Row {
        case: "a tag that states no PropagateAtLaunch and carries an empty value",
        key: "unflagged-empty",
        value: "",
        requested: None,
        expected: true,
    },
];

/// The group every case reads, and the launch template its instances come from.
const GROUP: &str = "propagate-at-launch-group";
const TEMPLATE: &str = "propagate-at-launch-template";

/// One emulator: Auto Scaling and Amazon EC2 on one engine, because a group is created
/// from an EC2 launch template placed in EC2 subnets and launches EC2 instances.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Create the group the rows describe, with `desired_capacity` instances, through
/// `CreateAutoScalingGroup` over the wire — so every assertion below follows state the
/// service stored from a request a client could send.
async fn given_the_tagged_group(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    desired_capacity: i32,
) {
    let subnets = seed_subnets(engine, 1);
    let subnet = subnets.first().expect("one seeded subnet").id.clone();
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", TEMPLATE),
            p("LaunchTemplateData.ImageId", "ami-0abcdef1234567890"),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate expected 200, got {status}: {body}"
    );

    let mut params = vec![
        p("AutoScalingGroupName", GROUP),
        p("MinSize", "0"),
        p("MaxSize", desired_capacity.max(1).to_string()),
        p("DesiredCapacity", desired_capacity.to_string()),
        p("LaunchTemplate.LaunchTemplateName", TEMPLATE),
        p("VPCZoneIdentifier", subnet),
    ];
    for (position, row) in ROWS.iter().enumerate() {
        let member = position + 1;
        params.push(p(&format!("Tags.member.{member}.Key"), row.key));
        params.push(p(&format!("Tags.member.{member}.Value"), row.value));
        // The row that states nothing sends NO `PropagateAtLaunch` parameter at all —
        // which is the request shape whose stored decision is the rule under test.
        if let Some(requested) = row.requested {
            params.push(p(
                &format!("Tags.member.{member}.PropagateAtLaunch"),
                if requested { "true" } else { "false" },
            ));
        }
    }
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup expected 200, got {status}: {body}"
    );
}

/// The `(Key, Value, PropagateAtLaunch)` of every tag in one answered body, paired by
/// position.
///
/// A `TagDescription` carries all five members, so the per-member lists line up: an
/// absent `PropagateAtLaunch` on any tag shortens one list and is caught here rather than
/// silently shifting the pairs.
fn reported_tags(case: &str, body: &str, expected_count: usize) -> Vec<(String, String, String)> {
    let resource_ids = tag_values(body, "ResourceId");
    let resource_types = tag_values(body, "ResourceType");
    let keys = tag_values(body, "Key");
    let values = tag_values(body, "Value");
    let propagate = tag_values(body, "PropagateAtLaunch");
    for (member, reported) in [
        ("ResourceId", &resource_ids),
        ("ResourceType", &resource_types),
        ("Key", &keys),
        ("Value", &values),
        ("PropagateAtLaunch", &propagate),
    ] {
        assert_eq!(
            reported.len(),
            expected_count,
            "case {case:?}: expected {expected_count} tags each carrying {member}, got \
             {} of them in {body}",
            reported.len()
        );
    }
    for resource_id in &resource_ids {
        assert_eq!(
            resource_id, GROUP,
            "case {case:?}: expected every tag reported against {GROUP}, got {resource_id:?}"
        );
    }
    for resource_type in &resource_types {
        assert_eq!(
            resource_type, GROUP_RESOURCE_TYPE,
            "case {case:?}: expected ResourceType {GROUP_RESOURCE_TYPE}, got {resource_type:?}"
        );
    }
    keys.into_iter()
        .zip(values)
        .zip(propagate)
        .map(|((key, value), propagate)| (key, value, propagate))
        .collect()
}

/// The flag reported for the tag keyed `key`, as the wire spells it.
fn reported_flag(tags: &[(String, String, String)], key: &str) -> Option<String> {
    tags.iter()
        .find(|(reported, _, _)| reported == key)
        .map(|(_, _, propagate)| propagate.clone())
}

/// The wire spelling of a Boolean, so an expectation is compared against the text a
/// client parses.
fn spelled(flag: bool) -> String {
    if flag { "true" } else { "false" }.to_string()
}

#[tokio::test]
async fn every_described_group_tag_reports_propagate_at_launch() {
    let (router, engine) = emulator();
    given_the_tagged_group(&router, &engine, 0).await;

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", GROUP)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups expected 200, got {status}: {body}"
    );
    let described = reported_tags(
        "the group's own read",
        &body,
        ROWS.len(),
    );
    for row in ROWS {
        let case = row.case;
        assert_eq!(
            reported_flag(&described, row.key).as_deref(),
            Some(spelled(row.expected).as_str()),
            "case {case:?}: expected the group's read to report PropagateAtLaunch {} for \
             {:?}, got {described:?}",
            row.expected,
            row.key
        );
    }
}

#[tokio::test]
async fn every_tag_the_account_wide_read_reports_carries_propagate_at_launch() {
    let (router, engine) = emulator();
    given_the_tagged_group(&router, &engine, 0).await;

    let (status, body) = query(&router, "DescribeTags", &[]).await;
    assert_eq!(status, 200, "DescribeTags expected 200, got {status}: {body}");
    let described = reported_tags("the account-wide read", &body, ROWS.len());
    for row in ROWS {
        let case = row.case;
        assert_eq!(
            reported_flag(&described, row.key).as_deref(),
            Some(spelled(row.expected).as_str()),
            "case {case:?}: expected DescribeTags to report PropagateAtLaunch {} for {:?}, \
             got {described:?}",
            row.expected,
            row.key
        );
    }
}

#[tokio::test]
async fn the_propagate_at_launch_filter_partitions_every_tag() {
    // The flag is stored for every tag, so the two Booleans PARTITION the account's tags:
    // each one is selected by exactly one of them, the tag whose create stated nothing
    // included — which the service's default puts on the `true` side.
    let (router, engine) = emulator();
    given_the_tagged_group(&router, &engine, 0).await;

    for wanted in [true, false] {
        let case = format!("the propagate-at-launch filter, {wanted}");
        let value = spelled(wanted);
        let (status, body) = query(
            &router,
            "DescribeTags",
            &[
                ("Filters.member.1.Name", "propagate-at-launch"),
                ("Filters.member.1.Values.member.1", value.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTags expected 200, got {status}: {body}"
        );
        let mut selected = tag_values(&body, "Key");
        selected.sort();
        let mut expected: Vec<String> = ROWS
            .iter()
            .filter(|row| row.expected == wanted)
            .map(|row| row.key.to_string())
            .collect();
        expected.sort();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the filter to select {expected:?}, got {selected:?} \
             in {body}"
        );
    }
}

#[tokio::test]
async fn only_the_tags_whose_stored_decision_is_true_reach_a_launched_instance() {
    let (router, engine) = emulator();
    given_the_tagged_group(&router, &engine, 1).await;

    // The instance the create's capacity launched, read off the group itself.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", GROUP)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups expected 200, got {status}: {body}"
    );
    let instances = tag_values(&body, "InstanceId");
    assert_eq!(
        instances.len(),
        1,
        "a group of one desired instance reports one instance, got {instances:?} in {body}"
    );
    let instance_id = instances
        .first()
        .expect("the group reports its one instance")
        .clone();

    // Amazon EC2's own view of that instance, polled for until it resolves: the launch is
    // the account's, so the tags are read where a client reads them. The deadline is
    // generous — the test waits for the CONDITION, and only gives up if it never holds.
    let deadline = Instant::now() + Duration::from_secs(30);
    let tags = loop {
        let (status, body) =
            ec2_query(&router, "DescribeInstances", &[p("InstanceId.1", &instance_id)]).await;
        if status == 200 && !tag_value(&body, "instanceId").is_empty() {
            let keys = tag_values(&body, "key");
            let values = tag_values(&body, "value");
            break keys
                .into_iter()
                .zip(values)
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<String>>();
        }
        assert!(
            Instant::now() < deadline,
            "Amazon EC2 never resolved {instance_id}, which the group reports as its \
             instance: last answer {status}: {body}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    for row in ROWS {
        let case = row.case;
        let pair = format!("{}={}", row.key, row.value);
        assert_eq!(
            tags.contains(&pair),
            row.expected,
            "case {case:?}: the tag {pair:?} reaches a launched instance only when its \
             stored PropagateAtLaunch is true ({}), got {tags:?}",
            row.expected
        );
    }
}
