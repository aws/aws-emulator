// SPDX-License-Identifier: CC0-1.0

//! WHEN a scheduled action fires, and what a client that only watches the SCALED RESOURCE
//! sees of it.
//!
//! The rules under test:
//!
//! * a scheduled action's occurrence takes effect at its own instant, moving the capacity of
//!   the resource the target scales in the service that owns that resource — no further
//!   Application Auto Scaling call is needed for it, because nothing the clock owes waits for
//!   a caller;
//! * the instant is the one the action's own `Timezone` makes it, not the UTC reading of the
//!   wall clock the expression carries;
//! * occurrences take effect in the order of the INSTANTS they are due at, so the capacity a
//!   target is left at is the one the latest-scheduled occurrence set, whatever order the
//!   actions were created in or named with;
//! * a capacity movement is reported as a NON-TERMINAL scaling activity until it is fulfilled,
//!   so a client POLLING `DescribeScalingActivities` observes that status before `Successful`;
//! * the action stays listed by `DescribeScheduledActions` after it has fired.
//!
//! Every suite spans more than the one case the issue cited: the resource owner (a DynamoDB
//! table's provisioned capacity and an ECS service's desired count), the schedule form (`at`
//! and `cron`), the time zone the schedule is read in (ahead of UTC and behind it), and the
//! creation order of a queue of actions.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::ACTIVITY_FULFILMENT;

use super::emulator::{at_schedule_in, Emulator};

/// The window between queueing an occurrence and its instant. Nothing in the service sleeps
/// for it: the due instant is stored, and the clock materialises it once it passes.
const WINDOW: Duration = Duration::from_secs(3);

/// How long a case keeps polling for an occurrence to have taken effect: well past [`WINDOW`]
/// plus the quarter-second the in-process clock waits between passes.
const POLL_FOR: Duration = Duration::from_secs(20);

/// How long a case polls for an occurrence of a CRON expression, whose finest resolution is a
/// minute: the next minute boundary is up to sixty seconds away whenever the suite runs.
const POLL_FOR_CRON: Duration = Duration::from_secs(90);

/// How long a poll sleeps between reads.
const POLL_EVERY: Duration = Duration::from_millis(200);

/// Poll `read` until it answers `wanted` or `budget` runs out, and answer what it last said.
async fn poll_until<F, Fut>(wanted: i64, budget: Duration, read: F) -> i64
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = i64>,
{
    let deadline = SystemTime::now() + budget;
    let mut last = read().await;
    while last != wanted && SystemTime::now() < deadline {
        tokio::time::sleep(POLL_EVERY).await;
        last = read().await;
    }
    last
}

/// One occurrence that moves the capacity of a resource this emulator owns the service of.
struct FiringRow {
    case: &'static str,
    /// The dimension the target scales, which decides the owning service.
    dimension: &'static str,
    /// The time zone the schedule's wall clock is read in, or `None` for the default (UTC).
    ///
    /// A zone that is not UTC is what makes the rule observable: the expression carries the
    /// LOCAL reading of the instant under test, so reading it as UTC instead names an instant
    /// hours away and the occurrence does not fall inside the poll window at all.
    timezone: Option<&'static str>,
    /// The `MinCapacity` the action sets, above the registered floor so the occurrence is
    /// visible as a capacity movement of the resource.
    scheduled_min: i64,
}

/// The catalog: both resource owners this emulator holds a store for, and a zone on either
/// side of UTC — so what is pinned is "the occurrence takes effect on the resource, at the
/// instant its own zone makes it", not one schedule against one service.
const FIRING_ROWS: &[FiringRow] = &[
    FiringRow {
        case: "a table's read capacity, in the default zone",
        dimension: "dynamodb:table:ReadCapacityUnits",
        timezone: None,
        scheduled_min: 7,
    },
    FiringRow {
        case: "a table's write capacity, in the default zone",
        dimension: "dynamodb:table:WriteCapacityUnits",
        timezone: None,
        scheduled_min: 4,
    },
    FiringRow {
        case: "an ECS service's desired count, in the default zone",
        dimension: "ecs:service:DesiredCount",
        timezone: None,
        scheduled_min: 3,
    },
    FiringRow {
        case: "a zone behind UTC",
        dimension: "dynamodb:table:ReadCapacityUnits",
        timezone: Some("Pacific/Tahiti"),
        scheduled_min: 5,
    },
    FiringRow {
        case: "a zone ahead of UTC",
        dimension: "dynamodb:table:WriteCapacityUnits",
        timezone: Some("Asia/Tokyo"),
        scheduled_min: 6,
    },
    FiringRow {
        case: "a zone that IS UTC, named",
        dimension: "ecs:service:DesiredCount",
        timezone: Some("UTC"),
        scheduled_min: 2,
    },
];

#[tokio::test]
async fn an_occurrence_moves_the_scaled_resource_with_no_further_call() {
    for row in FIRING_ROWS {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator.given_target(row.dimension, 1, 20).await;

        let mut action = target.clone();
        action["ScheduledActionName"] = json!("raise-the-floor");
        action["Schedule"] = json!(match row.timezone {
            Some(zone) => at_schedule_in_zone(WINDOW, zone),
            None => at_schedule_in(WINDOW).0,
        });
        if let Some(timezone) = row.timezone {
            action["Timezone"] = json!(timezone);
        }
        action["ScalableTargetAction"] = json!({"MinCapacity": row.scheduled_min});
        let (status, body) = emulator.call("PutScheduledAction", action).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScheduledAction to answer 200, got {status} {body:?}"
        );

        // From here the suite makes no Application Auto Scaling call until the capacity has
        // moved: it watches the resource the way a client does, in the service that owns it.
        let reached = poll_until(row.scheduled_min, POLL_FOR, || {
            capacity_of(&emulator, &target)
        })
        .await;
        assert_eq!(
            reached, row.scheduled_min,
            "case {case:?}: expected the occurrence to move the resource's capacity to {}, got \
             {reached}",
            row.scheduled_min
        );

        // And the action is still listed afterwards: firing does not consume it.
        assert_eq!(
            listed_actions(&emulator, &target).await,
            vec!["raise-the-floor".to_string()],
            "case {case:?}: expected the fired action to stay listed"
        );
    }
}

#[tokio::test]
async fn a_cron_occurrence_fires_at_the_next_minute_it_names() {
    const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";
    /// The `MinCapacity` the occurrence sets, above the registered floor.
    const SCHEDULED_MIN: i64 = 8;

    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    let mut action = target.clone();
    action["ScheduledActionName"] = json!("every-minute");
    // Every minute of every day: a cron expression names minutes, so the soonest occurrence
    // is the next minute boundary.
    action["Schedule"] = json!("cron(* * * * ? *)");
    action["ScalableTargetAction"] = json!({"MinCapacity": SCHEDULED_MIN});
    let (status, body) = emulator.call("PutScheduledAction", action).await;
    assert_eq!(
        status, 200,
        "case \"a cron occurrence\": expected PutScheduledAction to answer 200, got {status} \
         {body:?}"
    );

    let reached = poll_until(SCHEDULED_MIN, POLL_FOR_CRON, || {
        capacity_of(&emulator, &target)
    })
    .await;
    assert_eq!(
        reached, SCHEDULED_MIN,
        "case \"a cron occurrence\": expected the table's read capacity to reach \
         {SCHEDULED_MIN}, got {reached}"
    );
    assert_eq!(
        listed_actions(&emulator, &target).await,
        vec!["every-minute".to_string()],
        "case \"a cron occurrence\": expected the fired action to stay listed"
    );
}

/// One action of a queue: when it is due, relative to the others, and the floor it sets.
struct QueuedRow {
    case: &'static str,
    /// The name the action is created under, which is NOT the order it fires in.
    name: &'static str,
    /// How far ahead of the put the occurrence is.
    due_in: Duration,
    /// The `MinCapacity` it sets.
    scheduled_min: i64,
}

/// A queue created in REVERSE schedule order, and named so that name order runs the other way
/// round too, so neither creation order nor name order can be mistaken for schedule order.
const QUEUE: &[QueuedRow] = &[
    QueuedRow {
        case: "created first, due last",
        name: "a-late",
        due_in: Duration::from_secs(5),
        scheduled_min: 9,
    },
    QueuedRow {
        case: "created second, due in the middle",
        name: "b-middle",
        due_in: Duration::from_secs(4),
        scheduled_min: 6,
    },
    QueuedRow {
        case: "created last, due first",
        name: "c-early",
        due_in: Duration::from_secs(3),
        scheduled_min: 2,
    },
];

#[tokio::test]
async fn queued_occurrences_take_effect_in_schedule_order() {
    const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";

    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    // Every occurrence's instant is read off ONE moment, before the first put: the expressions
    // this suite writes carry a wall clock of one-second resolution, so a schedule computed
    // between two puts is measured from a later moment and the spacing the queue relies on
    // would narrow — or invert — by however long the puts before it took on the host. Reading
    // them together keeps the queue's order the SCHEDULE's, whatever the machine is doing.
    let schedules: Vec<String> = QUEUE
        .iter()
        .map(|row| at_schedule_in(row.due_in).0)
        .collect();
    for (row, schedule) in QUEUE.iter().zip(&schedules) {
        let mut action = target.clone();
        action["ScheduledActionName"] = json!(row.name);
        action["Schedule"] = json!(schedule);
        action["ScalableTargetAction"] = json!({"MinCapacity": row.scheduled_min});
        let (status, body) = emulator.call("PutScheduledAction", action).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutScheduledAction to answer 200, got {status} {body:?}",
            row.case
        );
    }

    // The occurrence due LAST is the one whose floor stands, so that is what the resource and
    // the target both report once every instant has passed.
    let last = QUEUE
        .iter()
        .max_by_key(|row| row.due_in)
        .expect("a queued action");
    let reached = poll_until(last.scheduled_min, POLL_FOR, || {
        capacity_of(&emulator, &target)
    })
    .await;
    assert_eq!(
        reached, last.scheduled_min,
        "case {:?}: expected the latest-scheduled occurrence to be the one in force, got \
         {reached}",
        last.case
    );
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": target["ServiceNamespace"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the target's bounds\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"][0]["MinCapacity"],
        json!(last.scheduled_min),
        "case \"the target's bounds\": expected the latest-scheduled floor, got {body:?}"
    );

    // Every occurrence is in the activity log, and the log is ordered by the instant each was
    // due at — newest first, which is the reverse of the order the queue fired in.
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            json!({"ServiceNamespace": target["ServiceNamespace"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the activity log\": expected 200, got {status} {body:?}"
    );
    let causes: Vec<String> = body["ScalingActivities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|activity| activity["Cause"].as_str())
        .map(str::to_string)
        .collect();
    for row in QUEUE {
        assert!(
            causes.iter().any(|cause| cause.contains(row.name)),
            "case {:?}: expected an activity naming the action that caused it, got {causes:?}",
            row.case
        );
    }
    let start_times: Vec<f64> = body["ScalingActivities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|activity| activity["StartTime"].as_f64())
        .collect();
    assert!(
        start_times.windows(2).all(|pair| pair[0] >= pair[1]),
        "case \"the activity log\": expected the activities newest first, got {start_times:?}"
    );
}

#[tokio::test]
async fn a_movement_an_occurrence_made_is_non_terminal_before_it_is_fulfilled() {
    const DIMENSION: &str = "dynamodb:table:WriteCapacityUnits";
    /// The non-terminal status a capacity movement carries until it is fulfilled, from the
    /// model's own `ScalingActivityStatusCode` value set.
    const IN_PROGRESS: &str = "InProgress";
    /// The status a fulfilled movement ends in.
    const SUCCESSFUL: &str = "Successful";

    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    let (schedule, _) = at_schedule_in(WINDOW);
    let mut action = target.clone();
    action["ScheduledActionName"] = json!("raise-the-floor");
    action["Schedule"] = json!(schedule);
    action["ScalableTargetAction"] = json!({"MinCapacity": 8});
    let (status, body) = emulator.call("PutScheduledAction", action).await;
    assert_eq!(
        status, 200,
        "case \"the occurrence\": expected PutScheduledAction to answer 200, got {status} {body:?}"
    );

    // A client POLLS the activity log, so the non-terminal status has to be observable between
    // two reads rather than only inside the one call that made the movement.
    let deadline = SystemTime::now() + POLL_FOR;
    let mut statuses: Vec<String> = Vec::new();
    while SystemTime::now() < deadline {
        if let Some(status_code) = newest_activity_status(&emulator, &target).await {
            if statuses.last() != Some(&status_code) {
                statuses.push(status_code.clone());
            }
            if status_code == SUCCESSFUL {
                break;
            }
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
    assert_eq!(
        statuses,
        vec![IN_PROGRESS.to_string(), SUCCESSFUL.to_string()],
        "case \"a polled movement\": expected the activity to pass through {IN_PROGRESS} before \
         {SUCCESSFUL}, got {statuses:?}"
    );
    // A terminal status is terminal: waiting out another whole fulfilment span changes nothing.
    tokio::time::sleep(ACTIVITY_FULFILMENT).await;
    assert_eq!(
        newest_activity_status(&emulator, &target).await,
        Some(SUCCESSFUL.to_string()),
        "case \"a fulfilled movement\": expected the terminal status to stand"
    );
}

/// An `at(yyyy-mm-ddThh:mm:ss)` expression naming the instant `due_in` from now as the WALL
/// CLOCK of `zone` reads it.
///
/// The counterpart of the harness's UTC [`at_schedule_in`]: an expression is read in the
/// action's own zone, so the same instant is a different expression per zone.
fn at_schedule_in_zone(due_in: Duration, zone: &str) -> String {
    let zone: chrono_tz::Tz = zone.parse().expect("a canonical IANA zone name");
    let seconds = (SystemTime::now() + due_in)
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs();
    let utc = chrono::DateTime::from_timestamp(
        i64::try_from(seconds).expect("a representable instant"),
        0,
    )
    .expect("a representable instant");
    format!(
        "at({})",
        utc.with_timezone(&zone).format("%Y-%m-%dT%H:%M:%S")
    )
}

/// The capacity the service that OWNS the resource `target` scales reports for it.
async fn capacity_of(emulator: &Emulator, target: &Value) -> i64 {
    let dimension = target["ScalableDimension"].as_str().unwrap_or_default();
    let resource_id = target["ResourceId"].as_str().unwrap_or_default();
    match target["ServiceNamespace"].as_str().unwrap_or_default() {
        "dynamodb" => {
            let reads = dimension.ends_with("ReadCapacityUnits");
            emulator.provisioned_capacity(resource_id, reads).await
        }
        "ecs" => {
            let (cluster, service) = resource_id
                .trim_start_matches("service/")
                .split_once('/')
                .expect("an ECS resource id names its cluster and its service");
            emulator.ecs_desired_count(cluster, service).await
        }
        other => panic!("case {other:?}: this suite reads no capacity for that namespace"),
    }
}

/// The names `DescribeScheduledActions` reports in `target`'s namespace.
async fn listed_actions(emulator: &Emulator, target: &Value) -> Vec<String> {
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": target["ServiceNamespace"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the listed actions\": expected 200, got {status} {body:?}"
    );
    body["ScheduledActions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|action| action["ScheduledActionName"].as_str())
        .map(str::to_string)
        .collect()
}

/// The `StatusCode` of the newest scaling activity of `target`'s namespace, if there is one.
async fn newest_activity_status(emulator: &Emulator, target: &Value) -> Option<String> {
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            json!({"ServiceNamespace": target["ServiceNamespace"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the activity log\": expected 200, got {status} {body:?}"
    );
    body["ScalingActivities"][0]["StatusCode"]
        .as_str()
        .map(str::to_string)
}
