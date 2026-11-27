// SPDX-License-Identifier: CC0-1.0

//! Rolling back the instance refresh a group has in progress.
//!
//! The rule under test: a rollback acts on the refresh its group has IN PROGRESS — it
//! answers that refresh's own id, moves it to `RollbackInProgress`, and the clock then
//! replaces the refresh's OWN replacements back, from the group's launch source, until
//! the refresh is reported `RollbackSuccessful` with nothing of its progress left; a
//! group with no refresh in progress has nothing to roll back, and a refresh whose
//! pre-refresh configuration is not pinned — no desired configuration, a launch template
//! version that floats, or an image resolved through a Systems Manager parameter — cannot
//! be reversed at all.
//!
//! The tables below span what that rule generalises over: how many instances the group
//! holds (none, one, several), how much of the refresh had already been done when the
//! rollback arrived, and — for the refusals — which of the situations the error classes
//! name the group and its refresh are in.

use std::sync::Arc;
use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    borrowed, ec2_query, new_engine, p, query, router_with, tag_value, tag_values, xml_unescape,
};

/// The launch template the groups in this suite launch from.
const LAUNCH_TEMPLATE: &str = "lt-rollback";

/// The launch template a refresh's desired configuration moves a group to.
const DESIRED_TEMPLATE: &str = "lt-rollback-next";

/// The launch template version a group that CAN be rolled back pins: a numbered version
/// names one configuration, which is what a rollback goes back to.
const PINNED_VERSION: &str = "1";

/// The zone the groups in this suite place their instances in.
const ZONE: &str = "us-west-2a";

/// Mint versions of the launch template `template` until it holds `version`, through Amazon
/// EC2's own writer.
///
/// A group launches from a version the launch template HAS — the service resolves the
/// specification against Amazon EC2's own store when the group is configured — so a row that
/// pins a version past the template's first needs that version to exist before the group is
/// created.
fn hold_version_up_to(engine: &Arc<engine::InMemoryEngine>, template: &str, version: i64) {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    let launch_template_id = store
        .launch_template_id_for_name(template)
        .unwrap_or_else(|| panic!("the account must hold a launch template named {template:?}"));
    while store
        .launch_templates
        .get(&launch_template_id)
        .is_some_and(|record| !record.versions.contains_key(&version))
    {
        store
            .insert_launch_template_version(
                &launch_template_id,
                ec2::LaunchTemplateVersionSpec {
                    created_by: engine::emulator_arn("iam", "", "root"),
                    create_time: std::time::SystemTime::now(),
                    version_description: None,
                    data: crate::support::seeded_launch_template_data(),
                },
            )
            .expect("the account must be able to hold another version of this template");
    }
}

/// The sentence a rollback of a group with nothing in progress is refused with.
fn nothing_in_progress_message(name: &str) -> String {
    format!("No in progress or baking Instance Refresh found for Auto Scaling group {name}.")
}

/// The sentence a rollback of a refresh carrying no desired configuration is refused
/// with.
const NO_DESIRED_CONFIGURATION_MESSAGE: &str =
    "The instance refresh cannot be rolled back because there is no desired configuration \
     specified for the instance refresh.";

/// The sentence a rollback of a group whose launch template version floats is refused
/// with: fixed text naming BOTH floating versions, because the refusal is about the group
/// using either of them rather than about the one it happens to name.
const FLOATING_VERSION_MESSAGE: &str =
    "This Auto Scaling group's launch template version isn't compatible with a rollback. \
     Rollbacks are not supported when the Auto Scaling group uses the launch template's \
     '$Latest' or '$Default' version.";

/// The sentence a rollback of a group whose launch template resolves its image through a
/// Systems Manager parameter is refused with.
const SSM_PARAMETER_MESSAGE: &str =
    "The instance refresh cannot be rolled back because the Auto Scaling group has a launch \
     template that uses a Systems Manager parameter instead of an AMI ID for the ImageId \
     property.";

/// One span of this service's clock, plus a margin: enough for one instance replacement
/// — of a refresh or of its rollback — or one lifecycle transition to have come due.
/// Derived from the service's own spans, so this and the implementation cannot drift
/// apart.
fn one_span() -> Duration {
    autoscaling::state::INSTANCE_REFRESH_REPLACEMENT
        .max(autoscaling::state::LIFECYCLE_TRANSITION)
        .max(autoscaling::state::INSTANCE_REFRESH_CANCELLATION)
        + Duration::from_millis(150)
}

/// Wait until the clock has certainly brought `steps` spans due.
async fn past_spans(steps: u32) {
    tokio::time::sleep(one_span() * (steps + 1)).await;
}

/// Create one group holding `capacity` instances from `template` at `version`, and wait
/// until they are placed.
async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    capacity: i32,
    template: &str,
    version: Option<&str>,
) {
    let capacity = capacity.to_string();
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p("LaunchTemplate.LaunchTemplateName", template),
        p("MinSize", "0"),
        p("MaxSize", capacity.as_str()),
        p("DesiredCapacity", capacity.as_str()),
        p("AvailabilityZones.member.1", ZONE),
    ];
    if let Some(version) = version {
        params.push(p("LaunchTemplate.Version", version));
    }
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
    past_spans(0).await;
}

/// Start one refresh on `name`, with a desired configuration when `desired` asks for it,
/// and answer the id the service tracks it by.
async fn start_refresh(router: &MultiServiceRouter, name: &str, desired: bool) -> String {
    let mut params = vec![p("AutoScalingGroupName", name)];
    if desired {
        params.push(p(
            "DesiredConfiguration.LaunchTemplate.LaunchTemplateName",
            DESIRED_TEMPLATE,
        ));
        params.push(p("DesiredConfiguration.LaunchTemplate.Version", PINNED_VERSION));
    }
    let (status, body) = query(router, "StartInstanceRefresh", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "StartInstanceRefresh for {name} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "InstanceRefreshId");
    assert!(!id.is_empty(), "a started refresh carries its id, got {body}");
    id
}

/// Ask for the rollback of the refresh `name` has in progress.
async fn rollback(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    query(
        router,
        "RollbackInstanceRefresh",
        &[("AutoScalingGroupName", name)],
    )
    .await
}

/// What `DescribeInstanceRefreshes` reports for the newest refresh of a group, including
/// the rollback details of a refresh somebody rolled back.
#[derive(Debug, Clone)]
struct ReportedRefresh {
    instance_refresh_id: String,
    status: String,
    percentage_complete: i32,
    instances_to_update: i32,
    end_time: String,
    rollback_reason: String,
    rollback_start_time: String,
    percentage_complete_on_rollback: String,
    instances_to_update_on_rollback: String,
}

/// Describe one group's refreshes and read the newest one out of the answer.
async fn newest_refresh(router: &MultiServiceRouter, name: &str) -> ReportedRefresh {
    let (status, body) = query(
        router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstanceRefreshes for {name} expected 200, got {status}: {body}"
    );
    let number = |tag: &str| -> i32 {
        let text = tag_value(&body, tag);
        text.parse()
            .unwrap_or_else(|_| panic!("{tag} must be a number, got {text:?} in {body}"))
    };
    ReportedRefresh {
        instance_refresh_id: tag_value(&body, "InstanceRefreshId"),
        status: tag_value(&body, "Status"),
        percentage_complete: number("PercentageComplete"),
        instances_to_update: number("InstancesToUpdate"),
        end_time: tag_value(&body, "EndTime"),
        rollback_reason: tag_value(&body, "RollbackReason"),
        rollback_start_time: tag_value(&body, "RollbackStartTime"),
        percentage_complete_on_rollback: tag_value(&body, "PercentageCompleteOnRollback"),
        instances_to_update_on_rollback: tag_value(&body, "InstancesToUpdateOnRollback"),
    }
}

/// The instance ids one group reports.
async fn group_instance_ids(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceId")
}

/// How long a poll below waits for the condition it looks for, and how often it looks
/// again.
///
/// The deadline is generous — what a row waits for is a chain of this service's own spans,
/// one per replacement the rollback owes — and nothing here is asserted about how long
/// anything took: the test waits for the CONDITION, and the deadline only stops a hang.
const DEADLINE: Duration = Duration::from_secs(300);
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// Look at the group's newest refresh until it reports `wanted`, and answer it then.
///
/// The spans a rollback owes come due on the clock, so the condition is LOOKED FOR rather
/// than waited out: a row is as quick as the service is, and a slow host only means more
/// looks.
async fn settled_refresh(
    router: &MultiServiceRouter,
    name: &str,
    wanted: &str,
    case: &str,
) -> ReportedRefresh {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let reported = newest_refresh(router, name).await;
        if reported.status == wanted {
            return reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the refresh of {name} never reached {wanted}, last read \
             {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Look at the group until it reports `capacity` instances, so a row reads the instances it
/// started from rather than a group still placing them.
async fn placed_instance_ids(
    router: &MultiServiceRouter,
    name: &str,
    capacity: usize,
    case: &str,
) -> Vec<String> {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let placed = group_instance_ids(router, name).await;
        if placed.len() == capacity {
            return placed;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {name} never placed its {capacity} instance(s), holds {placed:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The launch template name one group reports.
async fn group_launch_template(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    tag_value(&body, "LaunchTemplateName")
}

/// One row of the rollback table.
struct Row {
    case: &'static str,
    /// How many instances the group holds when the refresh starts.
    capacity: i32,
    /// How many of this service's spans elapse between starting the refresh and rolling
    /// it back: `0` rolls back one that has replaced nothing yet.
    spans_before_rollback: u32,
}

/// Every row: the rollback acts on the refresh in progress, replaces back what it had
/// done and leaves the group on the configuration it started from.
///
/// ONE ROW PER TEST, each on its own emulator, so the rows run beside one another rather
/// than one after the other: a row waits for the replacements its own capacity owes, and a
/// single test holding every row's waits end to end is the slowest thing in this crate.
const ROWS: &[Row] = &[
    Row {
        case: "a group holding no instance: the refresh replaced nothing to put back",
        capacity: 0,
        spans_before_rollback: 0,
    },
    Row {
        case: "one instance, rolled back before any replacement came due",
        capacity: 1,
        spans_before_rollback: 0,
    },
    Row {
        case: "two instances, rolled back once a replacement had been made",
        capacity: 2,
        spans_before_rollback: 1,
    },
    Row {
        case: "four instances, rolled back part way through",
        capacity: 4,
        spans_before_rollback: 2,
    },
    Row {
        case: "three instances, rolled back straight away",
        capacity: 3,
        spans_before_rollback: 0,
    },
];

#[tokio::test]
async fn rolling_back_a_refresh_of_an_empty_group_replaces_nothing_back() {
    rolling_back_replaces_its_replacements_back(0).await;
}

#[tokio::test]
async fn rolling_back_a_refresh_before_a_replacement_came_due_replaces_nothing_back() {
    rolling_back_replaces_its_replacements_back(1).await;
}

#[tokio::test]
async fn rolling_back_a_refresh_after_a_replacement_replaces_it_back() {
    rolling_back_replaces_its_replacements_back(2).await;
}

#[tokio::test]
async fn rolling_back_a_refresh_part_way_through_replaces_its_replacements_back() {
    rolling_back_replaces_its_replacements_back(3).await;
}

#[tokio::test]
async fn rolling_back_a_refresh_straight_away_replaces_nothing_back() {
    rolling_back_replaces_its_replacements_back(4).await;
}

/// Drive [`ROWS`]`[index]`: start a refresh, roll it back, and assert the rollback put back
/// every replacement the refresh had made and left the group where it started.
async fn rolling_back_replaces_its_replacements_back(index: usize) {
    let row = &ROWS[index];
    {
        // One emulator per row: each row is its own account, so the ids of one row never
        // explain another's.
        let router = router_with(new_engine());
        let name = format!("asg-rollback-{index}");
        create_group(
            &router,
            &name,
            row.capacity,
            LAUNCH_TEMPLATE,
            Some(PINNED_VERSION),
        )
        .await;
        let before = placed_instance_ids(
            &router,
            &name,
            usize::try_from(row.capacity).expect("a capacity is a count"),
            row.case,
        )
        .await;
        let started = start_refresh(&router, &name, true).await;
        if row.spans_before_rollback > 0 {
            tokio::time::sleep(one_span() * row.spans_before_rollback).await;
        }

        // What the refresh had done when the rollback arrived: the instances it had
        // launched are the ones the group holds that it did not hold before.
        let running = newest_refresh(&router, &name).await;
        let held_at_rollback = group_instance_ids(&router, &name).await;
        let launched_by_refresh: Vec<String> = held_at_rollback
            .iter()
            .filter(|id| !before.contains(id))
            .cloned()
            .collect();

        let (status, body) = rollback(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: RollbackInstanceRefresh expected 200, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "InstanceRefreshId"),
            started,
            "case {:?}: the answer names the refresh that was in progress, got {body}",
            row.case
        );

        // The rollback is owed a span of its own, so a caller reading straight after it
        // still sees the transient status — and the details of the rollback it started.
        let rolling_back = newest_refresh(&router, &name).await;
        assert_eq!(
            rolling_back.status, "RollbackInProgress",
            "case {:?}: a just-rolled-back refresh is being rolled back, got {rolling_back:?}",
            row.case
        );
        assert_eq!(
            rolling_back.instance_refresh_id, started,
            "case {:?}: the refresh keeps the id it was started under, got {rolling_back:?}",
            row.case
        );
        assert_eq!(
            rolling_back.rollback_reason,
            autoscaling::state::MANUAL_ROLLBACK_REASON,
            "case {:?}: the rollback reports why it was started, got {rolling_back:?}",
            row.case
        );
        assert!(
            !rolling_back.rollback_start_time.is_empty(),
            "case {:?}: the rollback reports when it began, got {rolling_back:?}",
            row.case
        );
        let on_rollback_percentage: i32 = rolling_back
            .percentage_complete_on_rollback
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: {rolling_back:?} reports a percentage", row.case));
        let on_rollback_to_update: i32 = rolling_back
            .instances_to_update_on_rollback
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: {rolling_back:?} reports a count", row.case));
        // "Indicates the value of PercentageComplete at the time the rollback started"
        // and the same for InstancesToUpdate: the refresh's progress only grows while it
        // runs, so what the rollback recorded is at least what the read before it saw.
        assert!(
            (running.percentage_complete..=100).contains(&on_rollback_percentage),
            "case {:?}: expected the progress at the rollback to be at least \
             {} and at most 100, got {rolling_back:?}",
            row.case,
            running.percentage_complete
        );
        assert!(
            (0..=running.instances_to_update).contains(&on_rollback_to_update),
            "case {:?}: expected at most {} instances left to update at the rollback, \
             got {rolling_back:?}",
            row.case,
            running.instances_to_update
        );
        if row.spans_before_rollback == 0 {
            assert_eq!(
                (on_rollback_percentage, on_rollback_to_update),
                (0, row.capacity),
                "case {:?}: a refresh rolled back before replacing anything had made no \
                 progress, got {rolling_back:?}",
                row.case
            );
        }
        // "If you roll back the instance refresh, InstancesToUpdate shows you the number
        // of instances that were not yet updated by the instance refresh": the rollback
        // does not replace those, so the count stands while it runs.
        assert_eq!(
            rolling_back.instances_to_update, on_rollback_to_update,
            "case {:?}: the instances the refresh never updated are not the rollback's to \
             replace, got {rolling_back:?}",
            row.case
        );

        // A refresh being rolled back still holds its group, and asking for the rollback
        // again finds the one already running rather than starting a second.
        let (status, body) = query(
            &router,
            "StartInstanceRefresh",
            &[("AutoScalingGroupName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a refresh being rolled back still holds its group, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "InstanceRefreshInProgress",
            "case {:?}: expected the InstanceRefreshInProgress code, got {body}",
            row.case
        );
        let (status, body) = rollback(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: a second rollback finds the one running, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "InstanceRefreshId"),
            started,
            "case {:?}: the second rollback names the same refresh, got {body}",
            row.case
        );

        // Once every replacement the rollback owed has come due, it has succeeded and
        // nothing of the refresh's progress is left. Each replacement the rollback makes
        // takes as many of its steps as one of the refresh's own, so the rollback is LOOKED
        // FOR until it reports that status rather than waited out for a fixed time.
        let rolled_back = settled_refresh(&router, &name, "RollbackSuccessful", row.case).await;
        assert_eq!(
            rolled_back.status, "RollbackSuccessful",
            "case {:?}: expected the rollback to have succeeded, got {rolled_back:?}",
            row.case
        );
        assert_eq!(
            rolled_back.percentage_complete, 0,
            "case {:?}: the progress of a rolled-back refresh has gone back to zero, \
             got {rolled_back:?}",
            row.case
        );
        assert!(
            !rolled_back.end_time.is_empty(),
            "case {:?}: a rolled-back refresh reports when it ended, got {rolled_back:?}",
            row.case
        );
        assert_eq!(
            rolled_back.instances_to_update, on_rollback_to_update,
            "case {:?}: the rollback replaced none of the instances the refresh never \
             updated, got {rolled_back:?}",
            row.case
        );

        // The group holds its capacity, and not one of the instances the refresh had
        // launched: each of those was replaced back.
        let after = group_instance_ids(&router, &name).await;
        assert_eq!(
            after.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {:?}: the group holds its capacity after the rollback, got {after:?}",
            row.case
        );
        let kept: Vec<&String> = after
            .iter()
            .filter(|id| launched_by_refresh.contains(id))
            .collect();
        assert!(
            kept.is_empty(),
            "case {:?}: expected none of the refresh's replacements {launched_by_refresh:?} \
             to be left, got {kept:?}",
            row.case
        );

        // The group never moved to the configuration the refresh was taking it to: that
        // is what the rollback restored it from.
        let template = group_launch_template(&router, &name).await;
        assert_eq!(
            template, LAUNCH_TEMPLATE,
            "case {:?}: expected the group to launch from {LAUNCH_TEMPLATE}, got {template}",
            row.case
        );

        // A rolled-back refresh is terminal, so the group is free to be refreshed again.
        let next = start_refresh(&router, &name, true).await;
        assert_ne!(
            next, started,
            "case {:?}: the refresh started after the rollback is its own, got {next}",
            row.case
        );
    }
}

/// What has happened to the group's refresh by the time the rollback arrives.
#[derive(Debug, Clone, Copy)]
enum Before {
    /// Nobody has ever started a refresh on the group.
    NoRefresh,
    /// A refresh is still in progress.
    InProgress,
    /// The group's only refresh has succeeded.
    Finished,
    /// The group's only refresh was cancelled.
    Cancelled,
}

/// One row of the refusal table.
struct Refusal {
    case: &'static str,
    /// The `Version` the group's launch template names, or `None` to leave it unnamed.
    group_version: Option<&'static str>,
    /// Whether the refresh carries a desired configuration.
    desired_configuration: bool,
    before: Before,
    /// The wire error code expected, or `None` for a rollback the service accepts.
    code: Option<&'static str>,
    /// The message expected, for a rollback the service refuses.
    message: Option<String>,
}

/// A rollback with nothing in progress to reverse, or no pinned configuration to reverse
/// TO, is refused — and the neighbouring request the service accepts is accepted.
#[tokio::test]
async fn a_rollback_with_nothing_to_reverse_is_refused() {
    let name_of = |index: usize| format!("asg-irreversible-{index}");
    let rows = [
        Refusal {
            case: "a group nobody has ever refreshed",
            group_version: Some(PINNED_VERSION),
            desired_configuration: false,
            before: Before::NoRefresh,
            code: Some("ActiveInstanceRefreshNotFound"),
            message: Some(nothing_in_progress_message(&name_of(0))),
        },
        Refusal {
            case: "a group whose only refresh has succeeded",
            group_version: Some(PINNED_VERSION),
            desired_configuration: true,
            before: Before::Finished,
            code: Some("ActiveInstanceRefreshNotFound"),
            message: Some(nothing_in_progress_message(&name_of(1))),
        },
        Refusal {
            case: "a group whose only refresh was cancelled",
            group_version: Some(PINNED_VERSION),
            desired_configuration: true,
            before: Before::Cancelled,
            code: Some("ActiveInstanceRefreshNotFound"),
            message: Some(nothing_in_progress_message(&name_of(2))),
        },
        Refusal {
            case: "a refresh started without a desired configuration",
            group_version: Some(PINNED_VERSION),
            desired_configuration: false,
            before: Before::InProgress,
            code: Some("IrreversibleInstanceRefresh"),
            message: Some(NO_DESIRED_CONFIGURATION_MESSAGE.to_string()),
        },
        Refusal {
            case: "a group whose launch template names no version, so it uses $Default",
            group_version: None,
            desired_configuration: true,
            before: Before::InProgress,
            code: Some("IrreversibleInstanceRefresh"),
            message: Some(FLOATING_VERSION_MESSAGE.to_string()),
        },
        Refusal {
            case: "a group that names the $Default version itself",
            group_version: Some("$Default"),
            desired_configuration: true,
            before: Before::InProgress,
            code: Some("IrreversibleInstanceRefresh"),
            message: Some(FLOATING_VERSION_MESSAGE.to_string()),
        },
        Refusal {
            case: "a group that names the $Latest version",
            group_version: Some("$Latest"),
            desired_configuration: true,
            before: Before::InProgress,
            code: Some("IrreversibleInstanceRefresh"),
            message: Some(FLOATING_VERSION_MESSAGE.to_string()),
        },
        Refusal {
            case: "a numbered version and a desired configuration: the rollback is served",
            group_version: Some(PINNED_VERSION),
            desired_configuration: true,
            before: Before::InProgress,
            code: None,
            message: None,
        },
        Refusal {
            case: "another numbered version, named as a later version of the template",
            group_version: Some("7"),
            desired_configuration: true,
            before: Before::InProgress,
            code: None,
            message: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let engine = new_engine();
        let router = router_with(Arc::clone(&engine));
        let name = name_of(index);
        // A row that pins a numbered version needs the template to hold it.
        if let Some(version) = row
            .group_version
            .and_then(|version| version.parse::<i64>().ok())
        {
            hold_version_up_to(&engine, LAUNCH_TEMPLATE, version);
        }
        create_group(&router, &name, 1, LAUNCH_TEMPLATE, row.group_version).await;
        match row.before {
            Before::NoRefresh => {}
            Before::InProgress => {
                start_refresh(&router, &name, row.desired_configuration).await;
            }
            Before::Finished => {
                start_refresh(&router, &name, row.desired_configuration).await;
                past_spans(3).await;
                let finished = newest_refresh(&router, &name).await;
                assert_eq!(
                    finished.status, "Successful",
                    "case {:?}: the refresh must have finished first, got {finished:?}",
                    row.case
                );
            }
            Before::Cancelled => {
                start_refresh(&router, &name, row.desired_configuration).await;
                let (status, body) = query(
                    &router,
                    "CancelInstanceRefresh",
                    &[("AutoScalingGroupName", name.as_str())],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the cancellation expected 200, got {status}: {body}",
                    row.case
                );
                past_spans(2).await;
                let cancelled = newest_refresh(&router, &name).await;
                assert_eq!(
                    cancelled.status, "Cancelled",
                    "case {:?}: the refresh must have been cancelled first, got {cancelled:?}",
                    row.case
                );
            }
        }

        let (status, body) = rollback(&router, &name).await;
        match (row.code, row.message.as_deref()) {
            (None, _) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the rollback to be served, got {status}: {body}",
                    row.case
                );
                assert!(
                    !tag_value(&body, "InstanceRefreshId").is_empty(),
                    "case {:?}: a served rollback names the refresh it acts on, got {body}",
                    row.case
                );
            }
            (Some(code), expected) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected the rollback to be refused, got {status}: {body}",
                    row.case
                );
                assert_eq!(
                    tag_value(&body, "Code"),
                    code,
                    "case {:?}: expected the {code} code, got {body}",
                    row.case
                );
                let expected = expected.expect("a refused row names the message it expects");
                assert_eq!(
                    xml_unescape(&tag_value(&body, "Message")),
                    expected,
                    "case {:?}: expected the message {expected:?}, got {body}",
                    row.case
                );
            }
        }
    }
}

/// A rollback of a group the account does not hold is refused by its name, whatever the
/// name looks like.
#[tokio::test]
async fn a_rollback_of_an_absent_group_is_refused_by_its_name() {
    let router = router_with(new_engine());

    // Names differing in length, case and character set, each of which the boundary
    // validator admits, so only the handler can refuse them.
    for name in [
        "a",
        "asg-does-not-exist",
        "ASG-UPPER-CASE",
        "asg.name_with-separators 1",
        "グループ-ünïcode",
    ] {
        let (status, body) = rollback(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {name:?}: expected the ValidationError code, got {body}"
        );
        let expected =
            format!("AutoScalingGroup name not found - Auto Scaling group with name {name} not found");
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            expected,
            "case {name:?}: expected the message {expected:?}, got {body}"
        );
    }
}

/// A rollback in progress is itself cancellable: `CancelInstanceRefresh` "cancels an
/// instance refresh or rollback that is in progress", so the call that stops a refresh
/// stops a rollback too, leaving what it had put back exactly as it stood.
#[tokio::test]
async fn a_rollback_in_progress_can_itself_be_cancelled() {
    let router = router_with(new_engine());
    let name = "asg-rollback-cancelled";
    create_group(&router, name, 3, LAUNCH_TEMPLATE, Some(PINNED_VERSION)).await;
    let started = start_refresh(&router, name, true).await;
    past_spans(1).await;

    let (status, body) = rollback(&router, name).await;
    assert_eq!(
        status, 200,
        "RollbackInstanceRefresh expected 200, got {status}: {body}"
    );
    let rolling_back = newest_refresh(&router, name).await;
    assert_eq!(
        rolling_back.status, "RollbackInProgress",
        "the refresh must be rolling back before it is cancelled, got {rolling_back:?}"
    );

    let (status, body) = query(
        &router,
        "CancelInstanceRefresh",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "cancelling a rollback expected 200, got {status}: {body}"
    );
    assert_eq!(
        tag_value(&body, "InstanceRefreshId"),
        started,
        "the cancellation names the refresh being rolled back, got {body}"
    );

    let cancelling = newest_refresh(&router, name).await;
    assert_eq!(
        cancelling.status, "Cancelling",
        "a just-cancelled rollback is being cancelled, got {cancelling:?}"
    );
    past_spans(2).await;
    let cancelled = newest_refresh(&router, name).await;
    assert_eq!(
        cancelled.status, "Cancelled",
        "expected the cancellation to have come due, got {cancelled:?}"
    );
    assert_eq!(
        cancelled.percentage_complete, cancelling.percentage_complete,
        "a cancellation puts nothing further back, got {cancelled:?} after {cancelling:?}"
    );
}

/// One emulator with BOTH services on one engine: a group's launch template lives in the
/// same account's Amazon EC2 store, which is where the image it launches is read from.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The refresh this suite starts names the desired configuration's launch template, which
    // the account has to hold like any other launch source.
    crate::support::seed_launch_templates(&engine, &[DESIRED_TEMPLATE]);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A Systems Manager parameter the account's own Amazon EC2 image catalogue resolves,
/// read FROM that catalogue so this suite names no image of its own.
fn resolvable_ssm_parameter(engine: &Arc<engine::InMemoryEngine>) -> String {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let store = state.lock();
    store
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .keys()
        .next()
        .cloned()
        .expect("the account's image catalogue publishes at least one parameter")
}

/// An AMI of the account's own Amazon EC2 catalogue, for the control template.
fn catalogued_image_id(engine: &Arc<engine::InMemoryEngine>) -> String {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let store = state.lock();
    store
        .public_ssm_parameter_image_ids(engine::emulator_account_id())
        .values()
        .next()
        .cloned()
        .expect("the account's image catalogue publishes at least one image")
}

/// Create one Amazon EC2 launch template over the wire, with the image id given.
async fn create_launch_template(router: &MultiServiceRouter, name: &str, image_id: &str) {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id),
            // A group that is not a mixed instances group takes the type its instances are
            // launched as from the version it launches from, so every template these groups
            // launch from states one.
            p(
                "LaunchTemplateData.InstanceType",
                crate::support::SEEDED_INSTANCE_TYPE,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
}

/// A group whose launch template resolves its image through a Systems Manager parameter
/// cannot be rolled back, while the same group on a template naming an AMI id can: the
/// parameter is what is not pinned, not the template.
#[tokio::test]
async fn a_group_whose_image_comes_from_a_parameter_cannot_be_rolled_back() {
    let (router, engine) = emulator();
    let parameter = resolvable_ssm_parameter(&engine);
    let image_id = catalogued_image_id(&engine);
    let aliased = format!("{}{parameter}", ec2::SSM_IMAGE_ALIAS_PREFIX);

    let cases = [
        ("asg-ssm-image", "lt-ssm-image", aliased.as_str(), true),
        ("asg-ami-image", "lt-ami-image", image_id.as_str(), false),
    ];

    for (name, template, image, refused) in cases {
        create_launch_template(&router, template, image).await;
        create_group(&router, name, 1, template, Some(PINNED_VERSION)).await;
        start_refresh(&router, name, true).await;

        let (status, body) = rollback(&router, name).await;
        if refused {
            assert_eq!(
                status, 400,
                "case {name}: a parameter-resolved image cannot be rolled back, \
                 got {status}: {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                "IrreversibleInstanceRefresh",
                "case {name}: expected the IrreversibleInstanceRefresh code, got {body}"
            );
            assert_eq!(
                xml_unescape(&tag_value(&body, "Message")),
                SSM_PARAMETER_MESSAGE,
                "case {name}: expected the parameter sentence, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {name}: an AMI id is pinned, so the rollback is served, \
                 got {status}: {body}"
            );
            assert_eq!(
                newest_refresh(&router, name).await.status,
                "RollbackInProgress",
                "case {name}: the served rollback is under way, got {body}"
            );
        }
    }
}
