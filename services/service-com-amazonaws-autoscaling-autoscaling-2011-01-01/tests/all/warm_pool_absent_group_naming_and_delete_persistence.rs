// SPDX-License-Identifier: CC0-1.0

//! The warm-pool surface of an Auto Scaling group over the awsQuery wire, on the two points a
//! client branches on: WHICH group a refusal names, and whether a pool that was deleted stays
//! deleted.
//!
//! RULES under test:
//!
//! 1. **A warm-pool call naming a group the account holds none under is refused HTTP 400 with
//!    the wire code `ValidationError`, and the only variable part of Amazon EC2 Auto Scaling's
//!    sentence is the `AutoScalingGroupName` the request carried — taken verbatim, whatever
//!    name that is.** Every call addressed at a group's warm pool (the put, the read and the
//!    delete) resolves the group first, so all three state the same sentence about the same
//!    name.
//! 2. **A warm pool holds the instances it has actually PREPARED, so the pool a put stores
//!    reports its configuration with an EMPTY instance list and gains instances only
//!    afterwards, as the size rule has it maintain them.**
//! 3. **A `DeleteWarmPool` the service accepts REMOVES the pool: the group then reports no
//!    `WarmPoolConfiguration` and no instance, through the pool's own reader and through the
//!    group's, and it stays that way on the clock — nothing a later reconcile does brings the
//!    deleted pool back, however much the group's own capacity would have the pool maintain.**
//!
//! Why it matters: a refusal that drops the name cannot tell a client configuring pools across
//! many groups which name was wrong; a pool that reports depth it has not prepared makes an
//! empty pool indistinguishable from a populated one for every capacity and cost assertion;
//! and a delete that does not take effect leaves a client that deletes and then verifies — or
//! deletes and recreates with other settings — reading the stale pool forever.
//!
//! The rows differ along what the rules generalise over: for rule 1, the FORM of the group
//! name (the published minimum length, a plain name, punctuation, a space, the published
//! maximum length, and a name differing from a stored group only in case) crossed with the
//! three operations; for rules 2 and 3, how many instances the pool maintains and which half
//! of the documented size rule decides that (the group's own maximum, the pool's minimum), the
//! members the put carries, the pool state it settles its instances in, and whether the delete
//! needs the force the model publishes for a pool that holds instances.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, element_or_absent, emulator, p, query, tag_value, tag_values, xml_unescape,
};

/// How often a poll re-reads a warm pool.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: the span this service owes the preparation
/// of an instance and the one Amazon EC2 owes its own transitions, with ample margin for a
/// loaded machine. Derived from the services' own spans rather than written out, so they cannot
/// drift apart.
fn poll_budget() -> Duration {
    autoscaling::state::LIFECYCLE_TRANSITION.max(ec2::TRANSITION_DELAY) * 4
        + Duration::from_secs(20)
}

/// The zone the groups of this suite place their instances in, in the Region the harness signs
/// for.
const ZONE: &str = "us-west-2a";

/// The launch template the harness seeds for this crate's groups.
const LAUNCH_TEMPLATE: &str = "lt-1";

/// The published length bounds of an `AutoScalingGroupName`: the member targets
/// `XmlStringMaxLen255`, "length 1..255"
/// (`smithy_query(query="operation",operation="PutWarmPool")`), so a name of either bound is a
/// name the service resolves rather than refuses for its length.
const MIN_GROUP_NAME_LENGTH: usize = 1;
const MAX_GROUP_NAME_LENGTH: usize = 255;

/// The group this suite stores so that a request naming it in ANOTHER case is shown to resolve
/// no group: a name is matched exactly, and what the refusal echoes is the REQUEST's spelling.
const STORED_GROUP: &str = "asg-wp-naming-stored";

/// The whole sentence Amazon EC2 Auto Scaling refuses a warm-pool call naming a group the
/// account holds none under with: fixed text apart from the name the request carried.
///
/// Written out here rather than taken from the service's own builder — a test that re-used the
/// implementation's literal would assert nothing.
fn absent_group_message(group: &str) -> String {
    format!("AutoScalingGroup name not found - No such Auto Scaling group: {group}")
}

/// The `LifecycleState` an instance of a pool in `pool_state` settles in, under the prefix the
/// model's own `LifecycleState` enum spells warm-pool states with (`Warmed:Stopped`,
/// `Warmed:Running`, `Warmed:Hibernated`).
fn warmed(pool_state: &str) -> String {
    format!("Warmed:{pool_state}")
}

/// Create one group placed by Availability Zone alone, with the bounds a case needs.
async fn create_group(router: &MultiServiceRouter, name: &str, max_size: i32, desired: i32) {
    let params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
        p("MinSize", "0"),
        p("MaxSize", max_size.to_string()),
        p("DesiredCapacity", desired.to_string()),
        p("AvailabilityZones.member.1", ZONE),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {name:?}: expected 200, got {status} with {body}"
    );
}

/// The warm pool of `group` as the pool's own reader reports it NOW.
#[derive(Debug, Default, PartialEq, Eq)]
struct Reported {
    /// Whether the read names a `WarmPoolConfiguration` at all.
    held: bool,
    min_size: String,
    pool_state: String,
    instance_ids: Vec<String>,
    lifecycle_states: Vec<String>,
}

/// Read `group`'s warm pool once, through `DescribeWarmPool`.
async fn describe_pool(case: &str, router: &MultiServiceRouter, group: &str) -> Reported {
    let (status, body) = query(
        router,
        "DescribeWarmPool",
        &[("AutoScalingGroupName", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeWarmPool({group}) expected 200, got {status} with {body}"
    );
    let configuration = element_or_absent(&body, "WarmPoolConfiguration");
    Reported {
        held: configuration.is_some(),
        min_size: tag_value(&body, "MinSize"),
        pool_state: tag_value(&body, "PoolState"),
        instance_ids: tag_values(&body, "InstanceId"),
        lifecycle_states: tag_values(&body, "LifecycleState"),
    }
}

/// Read `group`'s warm pool until it reports `wanted` instances, every one of them settled in
/// `state`, or a generous deadline passes — the CONDITION, never a fixed wait.
async fn pool_settled(
    case: &str,
    router: &MultiServiceRouter,
    group: &str,
    wanted: usize,
    state: &str,
) -> Reported {
    let deadline = Instant::now() + poll_budget();
    loop {
        let reported = describe_pool(case, router, group).await;
        let settled = reported.instance_ids.len() == wanted
            && reported.lifecycle_states.iter().all(|held| held == state);
        if settled || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What the GROUP's own reader says about `group`'s warm pool: whether it names a
/// `WarmPoolConfiguration`, and the `WarmPoolSize` it reports beside it.
async fn group_view_of_the_pool(
    case: &str,
    router: &MultiServiceRouter,
    group: &str,
) -> (bool, String) {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAutoScalingGroups({group}) expected 200, got {status} with {body}"
    );
    (
        element_or_absent(&body, "WarmPoolConfiguration").is_some(),
        tag_value(&body, "WarmPoolSize"),
    )
}

/// RULE 1: every call addressed at the warm pool of a group the account holds none under is
/// refused with the one absent-group sentence, whose only variable part is the name the request
/// carried — so the refusal always says WHICH group was missing.
#[tokio::test]
async fn a_warm_pool_call_names_the_absent_group_it_was_asked_for() {
    let (router, _engine) = emulator();
    // A group the account DOES hold, so the case that differs from it only in case is shown to
    // resolve nothing: the lookup is exact, and the echo is the request's own spelling.
    create_group(&router, STORED_GROUP, 1, 0).await;

    // The rows differ along the FORM of the name: both published length bounds, punctuation,
    // and a space — the member's pattern admits every printable character — plus the
    // case-shifted spelling of a name the account does hold.
    let longest = "w".repeat(MAX_GROUP_NAME_LENGTH);
    let shortest = "w".repeat(MIN_GROUP_NAME_LENGTH);
    let case_shifted = STORED_GROUP.to_uppercase();
    let names = [
        ("the published minimum length", shortest.as_str()),
        ("a plain name", "asg-wp-naming-absent"),
        ("punctuation", "asg.wp_naming-absent.0"),
        ("a space", "asg wp naming absent"),
        ("the published maximum length", longest.as_str()),
        ("a stored name in another case", case_shifted.as_str()),
    ];
    // Every operation addressed at ONE group's warm pool: the put that configures it, the read
    // that reports it, and the delete that takes it away.
    let actions = ["PutWarmPool", "DescribeWarmPool", "DeleteWarmPool"];

    for (form, name) in names {
        for action in actions {
            let case = format!("{action} naming {form}");
            let (status, body) = query(&router, action, &[("AutoScalingGroupName", name)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the call to be refused, got {status} with {body}"
            );
            let code = tag_value(&body, "Code");
            let message = xml_unescape(&tag_value(&body, "Message"));
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("ValidationError", absent_group_message(name).as_str()),
                "case {case:?}: expected the absent-group refusal naming {name:?}, got {body}"
            );
        }
    }

    // The group the account holds is untouched by any of that: the case-shifted rows resolved
    // no group, so none of them configured, read or deleted a pool on it.
    let case = "the stored group";
    assert_eq!(
        describe_pool(case, &router, STORED_GROUP).await,
        Reported::default(),
        "case {case:?}: expected the stored group to still hold no warm pool"
    );
}

/// One row of the teardown table: the group's bounds, the pool the put asks for, how many
/// instances the documented size rule then has the pool maintain, and whether taking it down
/// needs the force the model publishes.
struct Row {
    case: &'static str,
    group: &'static str,
    max_size: i32,
    desired: i32,
    /// The members the put carries beyond the group's name; empty asks for the documented
    /// defaults.
    put: Vec<(String, String)>,
    /// The `MinSize` the stored pool then reports, as the wire spells it.
    reported_min_size: &'static str,
    /// The `PoolState` the stored pool then reports.
    reported_pool_state: &'static str,
    maintained: usize,
}

/// RULES 2 and 3: the pool a put stores has prepared nothing within that call, so the read that
/// follows reports the configuration and an EMPTY instance list; the instances it maintains
/// appear afterwards; and the delete that takes the pool away leaves the group reporting no
/// pool at all — through both readers, and for good.
///
/// "For good" is read off the service rather than off a clock: after each delete this suite
/// puts a pool on a WITNESS group created afterwards and waits for that pool to prepare its own
/// instance, which is what says the span a preparation comes due in has passed since the
/// delete. The deleted pool is then re-read: a delete that had not taken effect, or a reconcile
/// that brought the pool back, shows up there.
#[tokio::test]
async fn a_deleted_warm_pool_is_gone_from_both_readers_and_stays_gone() {
    let (router, _engine) = emulator();
    // The rows differ along which half of the size rule decides the pool's depth, the members
    // the put carries, the state its instances settle in, and therefore whether the delete
    // needs the force.
    let rows = [
        Row {
            case: "the documented defaults, bounded by the group's own maximum",
            group: "asg-wp-gone-defaults",
            max_size: 1,
            desired: 0,
            put: Vec::new(),
            // "Defaults to 0 if not specified" (`PutWarmPoolType$MinSize`) and "Default is
            // Stopped" (`PutWarmPoolType$PoolState`).
            reported_min_size: "0",
            reported_pool_state: "Stopped",
            maintained: 1,
        },
        Row {
            case: "the documented defaults on a group at its own maximum: a pool of none",
            group: "asg-wp-gone-empty",
            max_size: 2,
            desired: 2,
            put: Vec::new(),
            reported_min_size: "0",
            reported_pool_state: "Stopped",
            maintained: 0,
        },
        Row {
            case: "the pool's own minimum as the floor, in another published pool state",
            group: "asg-wp-gone-minimum",
            max_size: 3,
            desired: 0,
            put: vec![
                p("MinSize", "2"),
                p("MaxGroupPreparedCapacity", "2"),
                p("PoolState", "Running"),
            ],
            reported_min_size: "2",
            reported_pool_state: "Running",
            maintained: 2,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        create_group(&router, row.group, row.max_size, row.desired).await;
        let mut params = vec![p("AutoScalingGroupName", row.group)];
        params.extend(row.put.iter().cloned());
        let (status, body) = query(&router, "PutWarmPool", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutWarmPool expected 200, got {status} with {body}"
        );

        // RULE 2: the pool is stored, and it has prepared NOTHING in the call that stored it.
        assert_eq!(
            describe_pool(case, &router, row.group).await,
            Reported {
                held: true,
                min_size: row.reported_min_size.to_string(),
                pool_state: row.reported_pool_state.to_string(),
                instance_ids: Vec::new(),
                lifecycle_states: Vec::new(),
            },
            "case {case:?}: expected the stored pool to report no instance yet"
        );

        // ...and what it maintains appears afterwards, settled in the pool's own state.
        let settled = pool_settled(
            case,
            &router,
            row.group,
            row.maintained,
            &warmed(row.reported_pool_state),
        )
        .await;
        assert_eq!(
            (settled.instance_ids.len(), settled.lifecycle_states.clone()),
            (
                row.maintained,
                vec![warmed(row.reported_pool_state); row.maintained]
            ),
            "case {case:?}: expected the pool to prepare {} instance(s) settled in {:?}, got \
             {settled:?}",
            row.maintained,
            warmed(row.reported_pool_state)
        );
        // The group's own reader agrees on the depth, which is the same stored pool read from
        // the other side.
        let (named, size) = group_view_of_the_pool(case, &router, row.group).await;
        assert_eq!(
            (named, size.as_str()),
            (true, row.maintained.to_string().as_str()),
            "case {case:?}: expected the group's own reader to report a pool of {}",
            row.maintained
        );

        // RULE 3: the delete takes the pool — with the force the model publishes where the
        // pool holds instances to protect, and without it where it holds none.
        let mut delete = vec![p("AutoScalingGroupName", row.group)];
        if row.maintained > 0 {
            delete.push(p("ForceDelete", "true"));
        }
        let (status, body) = query(&router, "DeleteWarmPool", &borrowed(&delete)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteWarmPool expected 200, got {status} with {body}"
        );
        assert_eq!(
            describe_pool(case, &router, row.group).await,
            Reported::default(),
            "case {case:?}: expected the deleted pool to be gone from the pool's own reader"
        );
        let (named, size) = group_view_of_the_pool(case, &router, row.group).await;
        assert_eq!(
            (named, size.as_str()),
            (false, ""),
            "case {case:?}: expected the group's own reader to report no pool either"
        );

        // ...and it stays gone: the witness below is created AFTER the delete, so a pool of its
        // own that has prepared an instance says the span a preparation comes due in has passed
        // since then. The group the delete emptied still has the headroom its maximum gives it,
        // so a delete that had not taken effect — or a reconcile that re-created the pool —
        // would be reporting a pool again by now.
        let witness = format!("asg-wp-gone-witness-{index}");
        create_group(&router, &witness, 1, 0).await;
        let witness_params = vec![
            p("AutoScalingGroupName", witness.clone()),
            p("MinSize", "1"),
            p("MaxGroupPreparedCapacity", "1"),
            p("PoolState", "Stopped"),
        ];
        let (status, body) = query(&router, "PutWarmPool", &borrowed(&witness_params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutWarmPool for the witness expected 200, got {status} with {body}"
        );
        let held = pool_settled(case, &router, &witness, 1, &warmed("Stopped")).await;
        assert_eq!(
            held.instance_ids.len(),
            1,
            "case {case:?}: expected the witness pool to prepare its instance, got {held:?}"
        );
        assert_eq!(
            describe_pool(case, &router, row.group).await,
            Reported::default(),
            "case {case:?}: expected the deleted pool to still be gone once a preparation span \
             has passed"
        );
    }
}
