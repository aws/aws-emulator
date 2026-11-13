// SPDX-License-Identifier: CC0-1.0

//! WHEN an occurrence reaches its target, and what becomes of the schedule once its
//! series is over: end to end over the wire through the real `MultiServiceRouter`,
//! with Amazon SQS and IAM mounted beside EventBridge Scheduler on ONE engine, so
//! each row is judged where a client observes it — on the queue the schedule targets
//! and through the service's own `GetSchedule`.
//!
//! RULES, one per suite below:
//!
//! * An occurrence is invoked AT the instant its `ScheduleExpression` names, read in
//!   `ScheduleExpressionTimezone` (UTC when the schedule names none) and bounded
//!   below by `StartDate`, and never before it: with `FlexibleTimeWindow` `Mode=OFF`
//!   at that instant, and with `Mode=FLEXIBLE` at an instant inside
//!   `[instant, instant + MaximumWindowInMinutes]` — the window opens at the
//!   occurrence and extends forward only.
//! * Consecutive occurrences of a recurring expression are spaced BY THE SERVER at
//!   the cadence the expression states, independently of how often anything polls
//!   the target.
//! * `ActionAfterCompletion` is applied once the schedule "completes invoking the
//!   target" — a one-time `at(...)` schedule when its occurrence has been invoked, a
//!   recurring one when its `EndDate` has passed: `DELETE` removes the schedule, so
//!   `GetSchedule` answers 404, and `NONE` (or no member at all) leaves it
//!   resolvable.
//! * A schedule is identified by the `(group, name)` pair, so deleting a schedule
//!   group takes the schedules of THAT group — they stop resolving and no further
//!   occurrence of one is invoked — and leaves an identically named schedule in
//!   another group armed. A read of a schedule in a group that no longer exists
//!   reports the GROUP as missing.
//!
//! The waits here are the schedules' own: nothing is compressed, so a row states an
//! expression whose occurrence falls seconds — or, for the minute-grained cron and
//! rate forms, a minute — ahead and waits that long for it. What the occurrence
//! computation names over the whole class of expressions, zones, windows and
//! boundaries is asserted without a wait in `src/state/invocation.rs`'s own tests.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::invocation;
use smithy_server_runtime::MultiServiceRouter;

// The emulator, the SQS/IAM setup and the queue reader this crate's invocation
// suites share (`tests/all/schedule_invocation.rs`), so this one spells none again.
use crate::schedule_invocation::{
    at_expression_body, drain, emulator, epoch_seconds, flexible_time_window,
    given_execution_role, given_queue, given_schedule, peek, schedule_read, scheduler,
    seconds_from_now, POLL_INTERVAL,
};
use crate::wire_harness::{error_code, error_message};

/// How long a row waits for a delivery beyond the instant it is due at: room for the
/// clock's tick and for the in-process invocation itself.
const DELIVERY_SLACK: Duration = Duration::from_secs(10);

/// How long a row watches a target it expects NOTHING to reach, measured from past
/// the instant an occurrence would otherwise have been invoked at.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// How long a row waits for a completed schedule to stop resolving, and how long it
/// watches one that must keep resolving.
const COMPLETION_PATIENCE: Duration = Duration::from_secs(20);

/// The zone the zoned row writes its local literal in, chosen because its offset is
/// not a whole number of hours — a schedule read in the wrong zone lands nowhere
/// near the instant the literal denotes.
const EASTERN_ZONE: ::chrono_tz::Tz = ::chrono_tz::Asia::Kolkata;

/// One row of the timing table: an expression with the UTC instant it names, the
/// zone that expression is read in, and the flexible window the schedule asks for.
struct TimingRow {
    case: &'static str,
    /// The expression and the instant it names, built when the row RUNS so that
    /// instant is still ahead of the clock. It is derived by the test from the
    /// expression it wrote — never read back off the emulator.
    expression: fn() -> (String, SystemTime),
    timezone: Option<&'static str>,
    /// `MaximumWindowInMinutes` under `Mode=FLEXIBLE`, or `None` for `Mode=OFF`.
    window_minutes: Option<i64>,
}

/// `at` as the body of an `at(...)` expression read in `zone`: the LOCAL date and
/// time of that instant there, which is what `ScheduleExpressionTimezone` says an
/// expression states ("the timezone in which the scheduling expression is
/// evaluated").
fn local_expression_body(at: SystemTime, zone: ::chrono_tz::Tz) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .with_timezone(&zone)
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// The first minute boundary at least `ahead` seconds from now: the instant a client
/// pins a `cron(...)` expression to.
fn next_minute_boundary(ahead: u64) -> SystemTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut boundary = seconds - seconds % 60 + 60;
    while boundary < seconds + ahead {
        boundary += 60;
    }
    SystemTime::UNIX_EPOCH + Duration::from_secs(boundary)
}

/// A `cron(...)` expression pinned to the minute `at` falls on: the six documented
/// fields "Minutes Hours Day-of-month Month Day-of-week Year", so the next
/// occurrence is that instant.
fn cron_pinned_to(at: SystemTime) -> String {
    let at = ::chrono::DateTime::<::chrono::Utc>::from(at);
    format!("cron({} {} * * ? *)", at.format("%-M"), at.format("%-H"))
}

/// The rows differ along the FORM of the expression (a one-time instant, a cron
/// expression pinned to a minute boundary), along the zone the expression is read in
/// (none, UTC, an eastern zone whose offset is not a whole hour), and along the
/// flexible window the schedule asks for.
const TIMING_ROWS: &[TimingRow] = &[
    TimingRow {
        case: "a one-time instant in UTC, invoked at the instant itself",
        expression: || {
            let at = seconds_from_now(5);
            (format!("at({})", at_expression_body(at)), at)
        },
        timezone: Some("UTC"),
        window_minutes: None,
    },
    TimingRow {
        case: "a one-time instant written as a local time in an eastern zone",
        expression: || {
            let at = seconds_from_now(6);
            (
                format!("at({})", local_expression_body(at, EASTERN_ZONE)),
                at,
            )
        },
        timezone: Some("Asia/Kolkata"),
        window_minutes: None,
    },
    TimingRow {
        case: "a cron expression pinned to a minute boundary of the hour",
        expression: || {
            let at = next_minute_boundary(15);
            (cron_pinned_to(at), at)
        },
        timezone: None,
        window_minutes: None,
    },
    TimingRow {
        case: "a one-time instant inside the shortest flexible window",
        expression: || {
            let at = seconds_from_now(5);
            (format!("at({})", at_expression_body(at)), at)
        },
        timezone: None,
        window_minutes: Some(1),
    },
];

/// The body of the first message the queue at `url` holds, with the instant the test
/// first SAW it — or `None` when nothing arrived inside `patience`.
///
/// The poll interval is short compared with every instant a row states, so a
/// delivery made before its due instant is observed before that instant too.
async fn awaited_delivery(
    router: &MultiServiceRouter,
    url: &str,
    patience: Duration,
) -> Option<(SystemTime, Value)> {
    let began = Instant::now();
    loop {
        if let Some(message) = peek(router, url).await.first() {
            let body = message["Body"].as_str().unwrap_or_default().to_string();
            return Some((
                SystemTime::now(),
                serde_json::from_str(&body).unwrap_or(Value::String(body)),
            ));
        }
        if began.elapsed() >= patience {
            return None;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every row: the occurrence reaches the target at the instant the expression names
/// and never before it, inside the window the schedule's `FlexibleTimeWindow` opens
/// at that instant, and the target is told that same instant as its scheduled time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_occurrence_is_invoked_before_the_instant_its_expression_names() {
    for (index, row) in TIMING_ROWS.iter().enumerate() {
        let case = row.case;
        let (expression, nominal) = (row.expression)();
        let emulator = emulator();
        let (url, queue_arn) = given_queue(&emulator, case, &format!("timing-{index}")).await;
        let role =
            given_execution_role(&emulator, case, &format!("timing-role-{index}"), &queue_arn)
                .await;
        let input = json!({
            "marker": format!("timing-{index}"),
            "at": "<aws.scheduler.scheduled-time>",
        })
        .to_string();
        let mut body = json!({
            "ScheduleExpression": expression,
            "FlexibleTimeWindow": flexible_time_window(row.window_minutes),
            "State": "ENABLED",
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": input },
        });
        if let Some(timezone) = row.timezone {
            body["ScheduleExpressionTimezone"] = json!(timezone);
        }
        given_schedule(&emulator, case, &format!("timing-{index}"), body).await;

        let window = Duration::from_secs(
            u64::try_from(row.window_minutes.unwrap_or_default()).unwrap_or_default() * 60,
        );
        let patience = nominal.duration_since(SystemTime::now()).unwrap_or_default()
            + window
            + DELIVERY_SLACK;
        let delivery = awaited_delivery(&emulator, &url, patience).await;
        let (observed, delivered) = delivery.unwrap_or_else(|| {
            panic!("case {case:?}: the expression {expression:?} delivered nothing inside {patience:?}")
        });
        assert!(
            observed >= nominal,
            "case {case:?}: the invocation was observed at {observed:?}, before the instant \
             {nominal:?} the expression {expression:?} names"
        );
        let spread = observed.duration_since(nominal).unwrap_or_default();
        assert!(
            spread <= window + DELIVERY_SLACK,
            "case {case:?}: the invocation was observed {spread:?} after the instant it names, \
             outside the window of {window:?} it may be invoked in"
        );
        assert_eq!(
            delivered["at"].as_str(),
            Some(invocation::scheduled_time_text(nominal).as_str()),
            "case {case:?}: expected the scheduled time {:?}, got {delivered}",
            invocation::scheduled_time_text(nominal)
        );
    }
}

/// A recurring series is paced BY THE SERVER: consecutive occurrences of a
/// `rate(...)` expression are one period apart, both on the scheduled time the target
/// is told and on the clock the deliveries are observed on, however often the target
/// is polled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_occurrences_are_spaced_by_the_cadence_the_expression_states() {
    let case = "a rate of one minute, polled every tenth of a second";
    // The shortest cadence a `rate(...)` expression can state ("minute | minutes |
    // hour | hours | day | days"), which is what bounds this row's own wait.
    let period = Duration::from_secs(60);
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, case, "cadence-target").await;
    let role = given_execution_role(&emulator, case, "cadence-role", &queue_arn).await;
    let input = json!({ "at": "<aws.scheduler.scheduled-time>" }).to_string();
    given_schedule(
        &emulator,
        case,
        "cadence",
        json!({
            "ScheduleExpression": "rate(1 minute)",
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "State": "ENABLED",
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": input },
        }),
    )
    .await;

    let mut observations = Vec::new();
    for occurrence in 0..2 {
        let delivery = awaited_delivery(&emulator, &url, period + DELIVERY_SLACK).await;
        let (observed, delivered) = delivery.unwrap_or_else(|| {
            panic!(
                "case {case:?}: occurrence {occurrence} was not delivered inside one period plus \
                 {DELIVERY_SLACK:?}"
            )
        });
        observations.push((observed, delivered["at"].as_str().unwrap_or_default().to_string()));
        drain(&emulator, &url).await;
    }

    let gap = observations[1]
        .0
        .duration_since(observations[0].0)
        .unwrap_or_default();
    assert!(
        gap >= period - DELIVERY_SLACK && gap <= period + DELIVERY_SLACK,
        "case {case:?}: expected consecutive deliveries one period ({period:?}) apart, got \
         {gap:?} between {:?} and {:?}",
        observations[0],
        observations[1]
    );
    assert_ne!(
        observations[0].1, observations[1].1,
        "case {case:?}: expected two occurrences with their own scheduled times, got {:?} twice",
        observations[0].1
    );
}

/// One row of the completion table: what the schedule's series is, what it states
/// after completion, and whether it still resolves once the series is over.
struct CompletionRow {
    case: &'static str,
    /// `true` for a recurring schedule bounded by an `EndDate`, `false` for a
    /// one-time `at(...)` schedule.
    recurring: bool,
    /// `ActionAfterCompletion`, or `None` for a request that omits the member.
    action: Option<&'static str>,
    /// Whether `GetSchedule` still resolves the schedule after it has completed.
    resolves_after: bool,
}

/// The rows differ along the series that completes (a one-time instant, a recurring
/// expression whose `EndDate` passes) and along the action the schedule states —
/// both modelled values (`NONE | DELETE`) and no member at all.
const COMPLETION_ROWS: &[CompletionRow] = &[
    CompletionRow {
        case: "a one-time schedule that deletes itself",
        recurring: false,
        action: Some("DELETE"),
        resolves_after: false,
    },
    CompletionRow {
        case: "a one-time schedule that keeps itself",
        recurring: false,
        action: Some("NONE"),
        resolves_after: true,
    },
    CompletionRow {
        case: "a one-time schedule stating no action at all",
        recurring: false,
        action: None,
        resolves_after: true,
    },
    CompletionRow {
        case: "a recurring schedule that deletes itself at its EndDate",
        recurring: true,
        action: Some("DELETE"),
        resolves_after: false,
    },
    CompletionRow {
        case: "a recurring schedule that keeps itself past its EndDate",
        recurring: true,
        action: Some("NONE"),
        resolves_after: true,
    },
];

/// Every row: the action the schedule states is applied once it has completed
/// invoking its target — `DELETE` removes it, so the read is refused exactly as a
/// deleted schedule's is, and `NONE` (or no member) leaves it resolvable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stated_action_is_applied_once_the_schedule_has_completed() {
    for (index, row) in COMPLETION_ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let (url, queue_arn) = given_queue(&emulator, case, &format!("completed-{index}")).await;
        let role = given_execution_role(
            &emulator,
            case,
            &format!("completed-role-{index}"),
            &queue_arn,
        )
        .await;
        let name = format!("completed-{index}");
        // A recurring row's window closes before its first occurrence, so it
        // completes at its `EndDate` with its last occurrence behind it; a one-time
        // row completes at the occurrence it invokes.
        let end_date = seconds_from_now(6);
        let mut body = json!({
            "ScheduleExpression": if row.recurring {
                "rate(1 minute)".to_string()
            } else {
                format!("at({})", at_expression_body(seconds_from_now(4)))
            },
            "FlexibleTimeWindow": { "Mode": "OFF" },
            "State": "ENABLED",
            "Target": { "Arn": queue_arn, "RoleArn": role, "Input": format!("{{\"n\":{index}}}") },
        });
        if row.recurring {
            body["EndDate"] = json!(epoch_seconds(end_date));
        }
        if let Some(action) = row.action {
            body["ActionAfterCompletion"] = json!(action);
        }
        given_schedule(&emulator, case, &name, body).await;

        if row.recurring {
            // Nothing is invoked at all: the window holds no occurrence of a
            // one-minute cadence.
            tokio::time::sleep(Duration::from_secs(8)).await;
            let held = peek(&emulator, &url).await;
            assert!(
                held.is_empty(),
                "case {case:?}: expected no occurrence inside a window shorter than the \
                 cadence, got {held:?}"
            );
        } else {
            let delivery = awaited_delivery(&emulator, &url, DELIVERY_SLACK * 2).await;
            assert!(
                delivery.is_some(),
                "case {case:?}: the one occurrence was never delivered, so the schedule never \
                 completed"
            );
        }

        if row.resolves_after {
            tokio::time::sleep(COMPLETION_PATIENCE / 2).await;
            let (status, read) = schedule_read(&emulator, "default", &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the completed schedule to keep resolving, got {status} \
                 {read}"
            );
        } else {
            let began = Instant::now();
            loop {
                let (status, read) = schedule_read(&emulator, "default", &name).await;
                if status == 404 {
                    assert_eq!(
                        (error_code(&read).as_str(), error_message(&read).as_str()),
                        (
                            "ResourceNotFoundException",
                            format!("Schedule {name} does not exist.").as_str()
                        ),
                        "case {case:?}: expected the refusal of a schedule that is gone, got \
                         {read}"
                    );
                    break;
                }
                assert!(
                    began.elapsed() < COMPLETION_PATIENCE,
                    "case {case:?}: the completed schedule still resolved after \
                     {COMPLETION_PATIENCE:?}: {read}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

/// Deleting a schedule group takes the schedules of THAT group — they stop resolving
/// and the occurrence none of them reached yet is never invoked — while the
/// identically named schedule in another group stays armed and is invoked. A read of
/// a schedule in the group that is gone reports the GROUP as missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_group_disarms_its_own_members_only() {
    let case = "the same schedule name in two groups, one group deleted";
    let emulator = emulator();
    let (cascaded_url, cascaded_arn) = given_queue(&emulator, case, "cascade-target").await;
    let (kept_url, kept_arn) = given_queue(&emulator, case, "sibling-target").await;
    let cascaded_role =
        given_execution_role(&emulator, case, "cascade-role", &cascaded_arn).await;
    let kept_role = given_execution_role(&emulator, case, "sibling-role", &kept_arn).await;

    for group in ["cascaded", "kept"] {
        let (status, created) =
            scheduler(&emulator, "POST", &format!("/schedule-groups/{group}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup {group} answered {status} {created}"
        );
    }
    // Both schedules carry the same NAME in different groups, and one instant far
    // enough ahead that the group is deleted before either occurrence is due.
    let name = "shared-name";
    let nominal = seconds_from_now(8);
    for (group, queue_arn, role, marker) in [
        ("cascaded", &cascaded_arn, &cascaded_role, "cascaded"),
        ("kept", &kept_arn, &kept_role, "kept"),
    ] {
        given_schedule(
            &emulator,
            case,
            name,
            json!({
                "GroupName": group,
                "ScheduleExpression": format!("at({})", at_expression_body(nominal)),
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": { "Arn": queue_arn, "RoleArn": role, "Input": marker },
            }),
        )
        .await;
    }

    let (status, deleted) =
        scheduler(&emulator, "DELETE", "/schedule-groups/cascaded", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteScheduleGroup answered {status} {deleted}"
    );
    // The group's removal is answered and then materialised, so the read below waits
    // for the group itself to stop resolving.
    let began = Instant::now();
    loop {
        let (status, read) = scheduler(&emulator, "GET", "/schedule-groups/cascaded", None).await;
        if status == 404 {
            break;
        }
        assert!(
            began.elapsed() < COMPLETION_PATIENCE,
            "case {case:?}: the deleted group still resolved after {COMPLETION_PATIENCE:?}: \
             {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // The member no longer resolves, and the refusal names the GROUP the request
    // named: the group was resolved first, and it is gone.
    let (status, refused) = schedule_read(&emulator, "cascaded", name).await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            error_message(&refused).as_str()
        ),
        (
            404,
            "ResourceNotFoundException",
            "Schedule group cascaded does not exist."
        ),
        "case {case:?}: expected the group's own not-found refusal, got {status} {refused}"
    );
    // The sibling group's identically named schedule is untouched, and ITS occurrence
    // is invoked.
    let (status, read) = schedule_read(&emulator, "kept", name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the sibling group's schedule to keep resolving, got {status} \
         {read}"
    );
    let patience = nominal
        .duration_since(SystemTime::now())
        .unwrap_or_default()
        + DELIVERY_SLACK;
    let delivery = awaited_delivery(&emulator, &kept_url, patience).await;
    let (_, delivered) = delivery.unwrap_or_else(|| {
        panic!("case {case:?}: the sibling group's schedule was never invoked")
    });
    assert_eq!(
        delivered.as_str(),
        Some("kept"),
        "case {case:?}: expected the sibling schedule's own Input, got {delivered}"
    );
    // The cascaded member's occurrence has come and gone with the group removed, so
    // nothing was invoked for it.
    tokio::time::sleep(QUIET_WINDOW).await;
    let held = peek(&emulator, &cascaded_url).await;
    assert!(
        held.is_empty(),
        "case {case:?}: expected the cascaded group's member to be disarmed, got {held:?}"
    );
}
