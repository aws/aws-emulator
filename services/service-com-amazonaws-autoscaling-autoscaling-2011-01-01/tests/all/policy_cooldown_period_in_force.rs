// SPDX-License-Identifier: CC0-1.0

//! WHICH cooldown period an execution of a simple scaling policy waits out.
//!
//! The RULE under test: **the cooldown period in force for an execution of a `SimpleScaling`
//! policy is the policy's OWN — "A cooldown period, in seconds, that applies to a specific
//! simple scaling policy. When a cooldown period is specified here, it overrides the default
//! cooldown." (`smithy_query(query="doc",shape="PutScalingPolicyType",member="Cooldown")`) —
//! whenever the policy states one, and the group's `DefaultCooldown` only when it does not. It
//! is ONE period on both sides of the execution: the span an execution that "waits for the
//! cooldown period to complete before executing the policy"
//! (`smithy_query(query="doc",shape="ExecutePolicyType",member="HonorCooldown"`) waits out,
//! measured from the group's last capacity change, and the span the performed policy leaves
//! behind. So a policy whose own period has already elapsed since that change is performed even
//! while the group's longer default would still be running, and one whose own period is the
//! longer of the two is held back even while the group's default has elapsed; a group no change
//! has moved is inside no period at all.**
//!
//! `execute_policy.rs` pins what a running cooldown then blocks and what the performed policy
//! leaves behind; what this suite pins is which period is in force in the first place. The rows
//! differ along what the rule generalises over: the period the policy states (none, zero, one
//! longer than the group's), the group's own `DefaultCooldown`, and what started the period the
//! execution is judged against (the capacity the group was created at, a later change through
//! the group's own writer, or nothing at all).

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{borrowed, new_engine, p, query, router_with, tag_value, xml_unescape, REGION};

/// The launch source every group here is created from — a launch TEMPLATE the shared harness
/// seeds into the account's own Amazon EC2 store.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// A period far longer than this test can run, so what the suite observes is decided by the
/// behaviour under test rather than by how fast the host is.
const LONG_COOLDOWN: i32 = 3_600;

/// The sentence an execution blocked by a running cooldown is refused with: fixed text, naming
/// neither the group, nor the policy, nor the period.
const COOLDOWN_IN_PROGRESS_MESSAGE: &str =
    "Scaling activity is in progress and blocks the execution of this policy";

/// Create one group at the capacity `desired`, with `default_cooldown` as its own period.
async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    desired: i32,
    default_cooldown: i32,
) {
    let params = vec![
        p("AutoScalingGroupName", name),
        p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
        p("MinSize", "0"),
        p("MaxSize", "4"),
        p("DesiredCapacity", desired.to_string()),
        p("DefaultCooldown", default_cooldown.to_string()),
        p("AvailabilityZones.member.1", format!("{REGION}a")),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the group must be created, got {status} with {body}"
    );
}

/// Put one `ChangeInCapacity` simple policy of +1 on `group`, stating `cooldown` as its own
/// period when the row gives one.
async fn put_policy(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    cooldown: Option<i32>,
) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", "scale-out"),
        p("PolicyType", "SimpleScaling"),
        p("AdjustmentType", "ChangeInCapacity"),
        p("ScalingAdjustment", "1"),
    ];
    if let Some(cooldown) = cooldown {
        params.push(p("Cooldown", cooldown.to_string()));
    }
    let (status, body) = query(router, "PutScalingPolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the policy must be put, got {status} with {body}"
    );
}

/// Move the group's desired capacity through its own writer, which honours no period.
async fn set_capacity(router: &MultiServiceRouter, case: &str, group: &str, desired: i32) {
    let params = vec![
        p("AutoScalingGroupName", group),
        p("DesiredCapacity", desired.to_string()),
        p("HonorCooldown", "false"),
    ];
    let (status, body) = query(router, "SetDesiredCapacity", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the capacity must be set, got {status} with {body}"
    );
}

/// Ask for the policy once, honouring the cooldown, and answer whether the service served it —
/// checking that an ask it did not serve is the cooldown refusal and nothing else.
async fn honoured_execution_is_served(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> bool {
    let params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", "scale-out"),
        p("HonorCooldown", "true"),
    ];
    let (status, body) = query(router, "ExecutePolicy", &borrowed(&params)).await;
    if status == 200 {
        return true;
    }
    assert_eq!(
        status, 400,
        "case {case:?}: an ask the service does not serve must be the cooldown refusal, got \
         {status} with {body}"
    );
    assert!(
        body.contains("<Code>ScalingActivityInProgress</Code>"),
        "case {case:?}: expected the ScalingActivityInProgress code, got {body}"
    );
    let message = xml_unescape(&tag_value(&body, "Message"));
    assert_eq!(
        message, COOLDOWN_IN_PROGRESS_MESSAGE,
        "case {case:?}: expected the cooldown sentence, got {message:?}"
    );
    false
}

/// The `DesiredCapacity` the group reports right now.
async fn desired_capacity(router: &MultiServiceRouter, case: &str, group: &str) -> String {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups expected 200, got {status} with {body}"
    );
    tag_value(&body, "DesiredCapacity")
}

/// One row: how the group and the policy are shaped, and whether the honoured execution is
/// served.
struct Row<'a> {
    case: &'a str,
    /// The group's own `DefaultCooldown`.
    group_cooldown: i32,
    /// The capacity the group is created at — a create at a capacity is the activity a
    /// cooldown follows.
    created_at: i32,
    /// A later change through the group's own writer, which is another such activity.
    moved_to: Option<i32>,
    /// The period the policy states, or none at all.
    policy_cooldown: Option<i32>,
    served: bool,
}

#[tokio::test]
async fn the_period_in_force_is_the_policys_own_whenever_it_states_one() {
    let rows: &[Row<'_>] = &[
        Row {
            case: "a policy stating no period waits the group's own, which is still running",
            group_cooldown: LONG_COOLDOWN,
            created_at: 1,
            moved_to: None,
            policy_cooldown: None,
            served: false,
        },
        Row {
            case: "a policy stating zero overrides that long default and waits for nothing",
            group_cooldown: LONG_COOLDOWN,
            created_at: 1,
            moved_to: None,
            policy_cooldown: Some(0),
            served: true,
        },
        Row {
            case: "a policy stating a long period overrides a default of zero and is held back",
            group_cooldown: 0,
            created_at: 1,
            moved_to: None,
            policy_cooldown: Some(LONG_COOLDOWN),
            served: false,
        },
        Row {
            case: "a group no change has moved is inside no period at all",
            group_cooldown: LONG_COOLDOWN,
            created_at: 0,
            moved_to: None,
            policy_cooldown: None,
            served: true,
        },
        Row {
            case: "the period is measured from the group's own last change, and zero has elapsed",
            group_cooldown: LONG_COOLDOWN,
            created_at: 0,
            moved_to: Some(2),
            policy_cooldown: Some(0),
            served: true,
        },
        Row {
            case: "a policy stating no period waits the group's own after that change too",
            group_cooldown: LONG_COOLDOWN,
            created_at: 0,
            moved_to: Some(2),
            policy_cooldown: None,
            served: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router_with(new_engine());
        let group = format!("in-force-{index}");

        create_group(&router, case, &group, row.created_at, row.group_cooldown).await;
        put_policy(&router, case, &group, row.policy_cooldown).await;
        if let Some(moved_to) = row.moved_to {
            set_capacity(&router, case, &group, moved_to).await;
        }

        let answered = honoured_execution_is_served(&router, case, &group).await;
        assert_eq!(
            answered, row.served,
            "case {case:?}: expected the honoured execution to be served = {}, got {answered}",
            row.served
        );

        // A served execution applied its one adjustment and a refused one kept nothing back,
        // so the capacity the group reports says the same thing the answer did.
        let held_before = row.moved_to.unwrap_or(row.created_at);
        let expected = if row.served {
            held_before + 1
        } else {
            held_before
        };
        let reported = desired_capacity(&router, case, &group).await;
        assert_eq!(
            reported,
            expected.to_string(),
            "case {case:?}: expected DesiredCapacity {expected}, got {reported}"
        );
    }
}

#[tokio::test]
async fn one_period_governs_both_sides_of_an_execution() {
    // The same period decides what an execution waits out and what it leaves behind, so a
    // policy stating zero is served however often it is asked for — each ask applying its own
    // adjustment — while the group's long default never comes into it.
    let case = "a policy stating zero asked for three times on a group with a long default";
    let router = router_with(new_engine());
    let group = "both-sides";

    create_group(&router, case, group, 0, LONG_COOLDOWN).await;
    put_policy(&router, case, group, Some(0)).await;
    for expected in 1..=3 {
        let served = honoured_execution_is_served(&router, case, group).await;
        assert!(
            served,
            "case {case:?}: ask {expected} must be served, since the period the last one left \
             behind is the policy's own zero"
        );
        let reported = desired_capacity(&router, case, group).await;
        assert_eq!(
            reported,
            expected.to_string(),
            "case {case:?}: after ask {expected} the group must hold {expected}, got {reported}"
        );
    }

    // And a policy stating the long period leaves THAT behind, so the ask after a served one is
    // held back — the same span on both sides.
    let case = "a policy stating a long period, asked for twice";
    let router = router_with(new_engine());
    let group = "both-sides-long";
    create_group(&router, case, group, 0, 0).await;
    put_policy(&router, case, group, Some(LONG_COOLDOWN)).await;
    let first = honoured_execution_is_served(&router, case, group).await;
    assert!(
        first,
        "case {case:?}: the first ask is served, since no change of the group has started a \
         period for the policy's own span to be measured from"
    );
    let second = honoured_execution_is_served(&router, case, group).await;
    assert!(
        !second,
        "case {case:?}: the second ask waits out the period the first left behind"
    );
    let reported = desired_capacity(&router, case, group).await;
    assert_eq!(
        reported, "1",
        "case {case:?}: only the served ask moved the capacity, got {reported}"
    );
}
