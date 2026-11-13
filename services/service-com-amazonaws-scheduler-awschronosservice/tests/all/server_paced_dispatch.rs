// SPDX-License-Identifier: CC0-1.0

//! The three properties that make a schedule's firing the SERVER's: end to end over
//! the wire through the real `MultiServiceRouter`, with Amazon SQS and IAM mounted
//! beside EventBridge Scheduler on ONE engine, so every instant is judged where a
//! client observes it — on the queue the schedule targets.
//!
//! Each suite here takes a rule `tests/all/occurrence_timing.rs` states for ONE case
//! and holds it over the class: both published bounds of the flexible window and two
//! schedules sharing one window length; both FORMS a one-minute cadence is stated in;
//! and a cascade judged on a RECURRING schedule over a SHARED target, so the sibling
//! group's schedule has to deliver an occurrence of its own series after the deleted
//! group is gone. The harness, and every assertion those suites already make, is
//! theirs — this one spells no second copy of either.
//!
//! RULES, one per suite below:
//!
//! * An occurrence of a schedule whose `FlexibleTimeWindow` is `Mode=FLEXIBLE` is
//!   invoked ONCE, at a server-chosen instant inside
//!   `[occurrence, occurrence + MaximumWindowInMinutes]` — clamped at BOTH ends: never
//!   before the instant the expression names, and never after the window closes. With
//!   `Mode=OFF` that window is empty, so the invocation is the occurrence itself.
//! * Consecutive occurrences of a recurring expression are spaced BY THE SERVER at the
//!   cadence the expression states: each occurrence carries its own scheduled time, one
//!   period on from the last, and no delivery is made before the instant it is
//!   scheduled for — however often a client polls the target.
//! * A schedule's identity is the `(group, name)` pair, so `DeleteScheduleGroup`
//!   cascades to the schedules of THAT group only: an identically named schedule in
//!   another group keeps firing — with its own `Target.Input` — at occurrences after
//!   the sibling group is gone.
//!
//! The waits here are the schedules' own: nothing is compressed, so a suite states an
//! expression whose occurrence falls seconds — or, for the minute-grained `rate(...)`
//! and `cron(...)` forms, a minute — ahead and polls for it until it arrives. Each
//! assertion on an instant is against an instant the SERVER named (the occurrence the
//! payload carries), never against how long the host took, and the deadline that ends
//! a poll is generous.
//!
//! What the occurrence and window computation names over the whole class of
//! expressions, zones and window lengths is asserted without a wait in
//! `src/state/invocation.rs`'s own tests.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::invocation;
use smithy_server_runtime::MultiServiceRouter;


// The emulator, the SQS/IAM setup, the queue reader and the schedule writer this
// crate's invocation suites share (`tests/all/schedule_invocation.rs`), so this one
// spells none again.
use crate::schedule_invocation::{
    at_expression_body, bodies, drain, emulator, flexible_time_window, given_execution_role,
    given_queue, given_schedule, schedule_read, scheduler, seconds_from_now, POLL_INTERVAL,
};
use crate::wire_harness::{error_code, error_message};

/// How far ahead of the clock the window suite states its one occurrence, PER ROW: the
/// occurrence has to still be ahead once every row's queue, role and schedule has been
/// written, and the lead is what gives a loaded host room for that setup. Nothing is
/// asserted about how long the setup took — the suite checks the occurrence is still
/// ahead of it and says so plainly if it is not.
const OCCURRENCE_LEAD_PER_ROW: u64 = 6;

/// The cadence every recurring expression below states, and the shortest one a
/// `rate(...)` expression can state at all ("rate(value unit)" over minutes, hours and
/// days).
const PERIOD: Duration = Duration::from_secs(60);

/// How long past an instant a suite keeps looking for the delivery due at it: room for
/// the clock's tick and for the in-process invocation, and the only slack the FLEXIBLE
/// window's own bound is given.
const DELIVERY_SLACK: Duration = Duration::from_secs(20);

/// The same room for a schedule that asks for NO window, whose invocation is the
/// occurrence ITSELF: generous multiples of the clock's tick, the in-process call and
/// one poll, and still far short of the shortest window a schedule could ask for — so a
/// schedule invoked as though it had one does not pass as one invoked at its
/// occurrence.
const AT_THE_OCCURRENCE_SLACK: Duration = Duration::from_secs(10);

/// How long a suite keeps watching a target after the deliveries it expected have
/// arrived, so an invocation this emulator would have made twice has had its chance to
/// show. Nothing is asserted about how long anything took: the window is only how long
/// the absence is watched for.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// How long the cascade suite waits for a deleted group's removal to be materialised.
const GROUP_PATIENCE: Duration = Duration::from_secs(30);

/// One row of the window table: the window the schedule asks for, and whether the
/// suite can wait that window out.
struct WindowRow {
    case: &'static str,
    /// `MaximumWindowInMinutes` under `Mode=FLEXIBLE`, or `None` for `Mode=OFF`.
    window_minutes: Option<i64>,
    /// Whether the suite waits for the invocation itself. A window longer than a
    /// suite's whole run holds an instant the suite cannot sit out, so such a row is
    /// held to the bound it CAN be judged by — that nothing is invoked before the
    /// occurrence — and to the window a client reads back.
    awaited: bool,
}

/// The rows differ along the mode, along the window's own length — including both
/// published bounds of `MaximumWindowInMinutes` (`@range min 1 max 1440`) — and, for
/// one length, along the SCHEDULE, since which instant inside the window is chosen is
/// the server's and is not the same for two schedules.
const WINDOW_ROWS: &[WindowRow] = &[
    WindowRow {
        case: "no window at all, so the invocation is the occurrence itself",
        window_minutes: None,
        awaited: true,
    },
    WindowRow {
        case: "the shortest window a schedule can ask for",
        window_minutes: Some(1),
        awaited: true,
    },
    WindowRow {
        case: "the shortest window asked for by another schedule",
        window_minutes: Some(1),
        awaited: true,
    },
    WindowRow {
        case: "the longest window a schedule can ask for",
        window_minutes: Some(1_440),
        awaited: false,
    },
];

/// One row of the cadence table: a recurring expression stating a one-minute cadence.
struct CadenceRow {
    case: &'static str,
    expression: &'static str,
}

/// The rows differ along the FORM the cadence is stated in — a rate over the period
/// and a cron expression naming every minute of every hour — both of which name one
/// occurrence per [`PERIOD`].
const CADENCE_ROWS: &[CadenceRow] = &[
    CadenceRow {
        case: "a rate of one minute",
        expression: "rate(1 minute)",
    },
    CadenceRow {
        case: "a cron expression naming every minute",
        expression: "cron(* * * * ? *)",
    },
];

/// A `Target.Input` carrying the schedule's own marker and the occurrence's scheduled
/// time, which is what identifies a delivery as that schedule's and that occurrence's.
fn target_input(marker: &str) -> String {
    json!({ "marker": marker, "at": "<aws.scheduler.scheduled-time>" }).to_string()
}

/// The ARN the service named one schedule by.
fn schedule_arn(case: &str, created: &Value) -> String {
    created["ScheduleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ScheduleArn in {created}"))
        .to_string()
}

/// One delivered body as the payload it is.
fn payload_of(case: &str, body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("case {case:?}: the delivered body is not JSON ({error}): {body:?}")
    })
}

/// The occurrence one payload carries as its scheduled time, as a clock reading, or
/// `None` when it carries none in the published form — read by the service's own
/// reverse of the attribute it writes, so this suite re-states that format nowhere.
fn scheduled_time(payload: &Value) -> Option<SystemTime> {
    invocation::scheduled_time_of(payload["at"].as_str()?)
}

/// Record every delivery the queue at `url` holds into `seen`, by the occurrence it
/// belongs to, keeping the instant this suite FIRST saw each one.
///
/// The reading is taken after the queue has answered, so an instant recorded for a
/// delivery is never earlier than the delivery itself.
async fn record_deliveries(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    marker: &str,
    seen: &mut BTreeMap<SystemTime, SystemTime>,
) {
    let held = bodies(router, url).await;
    let observed = SystemTime::now();
    for body in held {
        let payload = payload_of(case, &body);
        assert_eq!(
            payload["marker"].as_str(),
            Some(marker),
            "case {case:?}: expected the schedule's own marker {marker:?}, got {payload}"
        );
        let scheduled = scheduled_time(&payload).unwrap_or_else(|| {
            panic!("case {case:?}: the delivery carries no scheduled time: {payload}")
        });
        seen.entry(scheduled).or_insert(observed);
    }
}

/// Every row: the one occurrence of a schedule asking for a flexible window is invoked
/// exactly once, at an instant inside the window that opens AT the occurrence — never
/// before the instant the expression names, and never after the window closes — and the
/// target is told that occurrence's own instant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_occurrence_is_invoked_inside_the_window_that_opens_at_it() {
    let emulator = emulator();
    // One target per row, so a delivery is attributable to the row that asked for it.
    let mut targets = Vec::new();
    for (index, row) in WINDOW_ROWS.iter().enumerate() {
        let (url, queue_arn) =
            given_queue(&emulator, row.case, &format!("window-target-{index}")).await;
        let role =
            given_execution_role(&emulator, row.case, &format!("window-role-{index}"), &queue_arn)
                .await;
        targets.push((url, queue_arn, role));
    }

    // Every row states the SAME occurrence, computed once the queues and the roles are
    // in place so only the schedule writes — four in-process calls — stand between the
    // reading and the instant itself, with a lead per row besides.
    let nominal = seconds_from_now(
        OCCURRENCE_LEAD_PER_ROW * u64::try_from(WINDOW_ROWS.len()).unwrap_or(1),
    );
    for (index, row) in WINDOW_ROWS.iter().enumerate() {
        let (_, queue_arn, role) = &targets[index];
        let name = format!("window-{index}");
        let marker = format!("window-{index}");
        given_schedule(
            &emulator,
            row.case,
            &name,
            json!({
                "ScheduleExpression": format!("at({})", at_expression_body(nominal)),
                "ScheduleExpressionTimezone": "UTC",
                "FlexibleTimeWindow": flexible_time_window(row.window_minutes),
                "State": "ENABLED",
                "Target": { "Arn": queue_arn, "RoleArn": role, "Input": target_input(&marker) },
            }),
        )
        .await;
        // The window the invocation below is bounded by is the one a client reads back.
        let (status, read) = schedule_read(&emulator, "default", &name).await;
        assert_eq!(
            status, 200,
            "case {}: GetSchedule answered {status} {read}",
            row.case
        );
        assert_eq!(
            (
                read["State"].as_str(),
                read["FlexibleTimeWindow"]["Mode"].as_str(),
                read["FlexibleTimeWindow"]["MaximumWindowInMinutes"].as_i64(),
            ),
            (
                Some("ENABLED"),
                Some(match row.window_minutes {
                    Some(_) => "FLEXIBLE",
                    None => "OFF",
                }),
                row.window_minutes,
            ),
            "case {}: expected the window the schedule asked for, got {read}",
            row.case
        );
    }

    // The window's LOWER end: until the occurrence's own instant, no target has been
    // reached, whichever window its schedule asks for.
    while SystemTime::now() < nominal {
        for (index, row) in WINDOW_ROWS.iter().enumerate() {
            let held = bodies(&emulator, &targets[index].0).await;
            assert!(
                held.is_empty(),
                "case {}: expected nothing before the occurrence at {nominal:?}, got {held:?}",
                row.case
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // The window's UPPER end: each awaited row's invocation arrives inside its own
    // window, and no row is invoked twice for the one occurrence it states.
    let longest = WINDOW_ROWS
        .iter()
        .filter(|row| row.awaited)
        .filter_map(|row| row.window_minutes)
        .map(|minutes| Duration::from_secs(u64::try_from(minutes).unwrap_or_default() * 60))
        .max()
        .unwrap_or_default();
    let deadline = nominal + longest + DELIVERY_SLACK;
    let mut first_seen: Vec<Option<SystemTime>> = vec![None; WINDOW_ROWS.len()];
    // The instant the last new delivery was seen, which is what [`QUIET_WINDOW`] is
    // measured from.
    let mut settled = SystemTime::now();
    loop {
        for (index, row) in WINDOW_ROWS.iter().enumerate() {
            let held = bodies(&emulator, &targets[index].0).await;
            let observed = SystemTime::now();
            assert!(
                held.len() <= 1,
                "case {}: expected the one occurrence to be invoked once, got {held:?}",
                row.case
            );
            if let Some(body) = held.first() {
                let payload = payload_of(row.case, body);
                assert_eq!(
                    (
                        payload["marker"].as_str(),
                        payload["at"].as_str(),
                    ),
                    (
                        Some(format!("window-{index}").as_str()),
                        Some(invocation::scheduled_time_text(nominal).as_str()),
                    ),
                    "case {}: expected the schedule's own marker and the occurrence at {:?}, \
                     got {payload}",
                    row.case,
                    invocation::scheduled_time_text(nominal)
                );
                if first_seen[index].is_none() {
                    first_seen[index] = Some(observed);
                    settled = observed;
                }
            }
        }
        let awaited_seen = WINDOW_ROWS
            .iter()
            .enumerate()
            .all(|(index, row)| !row.awaited || first_seen[index].is_some());
        let now = SystemTime::now();
        // Every awaited row has been invoked and nothing has arrived for a while: the
        // window has nothing left to show, so the suite stops watching it.
        if now >= deadline || (awaited_seen && now >= settled + QUIET_WINDOW) {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, row) in WINDOW_ROWS.iter().enumerate() {
        if !row.awaited {
            continue;
        }
        let window = Duration::from_secs(
            u64::try_from(row.window_minutes.unwrap_or_default()).unwrap_or_default() * 60,
        );
        // A schedule asking for no window is held to the occurrence itself, not to the
        // room a FLEXIBLE one's bound is given.
        let slack = match row.window_minutes {
            Some(_) => DELIVERY_SLACK,
            None => AT_THE_OCCURRENCE_SLACK,
        };
        let observed = first_seen[index].unwrap_or_else(|| {
            panic!(
                "case {}: nothing was invoked inside the window of {window:?} that opened at \
                 {nominal:?}",
                row.case
            )
        });
        let spread = observed.duration_since(nominal).unwrap_or_default();
        assert!(
            observed >= nominal && spread <= window + slack,
            "case {}: the invocation was observed {spread:?} from the occurrence at {nominal:?}, \
             outside the window of {window:?} it may be invoked in",
            row.case
        );
    }
}

/// Every row: the occurrences of a recurring expression are spaced by the cadence the
/// expression states — each one period on from the last — and none is delivered before
/// the instant it is scheduled for, however fast the target is polled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_occurrence_is_delivered_at_its_own_instant_one_period_on_from_the_last() {
    let emulator = emulator();
    let mut rows: Vec<(String, String, BTreeMap<SystemTime, SystemTime>)> = Vec::new();
    for (index, row) in CADENCE_ROWS.iter().enumerate() {
        let (url, queue_arn) =
            given_queue(&emulator, row.case, &format!("cadence-target-{index}")).await;
        let role = given_execution_role(
            &emulator,
            row.case,
            &format!("cadence-role-{index}"),
            &queue_arn,
        )
        .await;
        let marker = format!("cadence-{index}");
        given_schedule(
            &emulator,
            row.case,
            &format!("cadence-{index}"),
            json!({
                "ScheduleExpression": row.expression,
                "FlexibleTimeWindow": { "Mode": "OFF" },
                "State": "ENABLED",
                "Target": { "Arn": queue_arn, "RoleArn": role, "Input": target_input(&marker) },
            }),
        )
        .await;
        rows.push((url, marker, BTreeMap::new()));
    }

    // Two occurrences of a one-minute cadence fall inside two periods of the write,
    // whether the series opens at the write (a rate) or at the next minute of the hour
    // (the cron form); the rest is room for the clock's tick and the delivery.
    let deadline = SystemTime::now() + PERIOD * 2 + DELIVERY_SLACK;
    loop {
        let mut collected = true;
        for (index, (url, marker, seen)) in rows.iter_mut().enumerate() {
            record_deliveries(&emulator, url, CADENCE_ROWS[index].case, marker, seen).await;
            collected &= seen.len() >= 2;
        }
        if collected || SystemTime::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for (index, (_, _, seen)) in rows.iter().enumerate() {
        let case = CADENCE_ROWS[index].case;
        let observed: Vec<(SystemTime, SystemTime)> =
            seen.iter().map(|(scheduled, at)| (*scheduled, *at)).collect();
        assert!(
            observed.len() >= 2,
            "case {case:?}: expected at least two occurrences of {:?}, got {observed:?}",
            CADENCE_ROWS[index].expression
        );
        for pair in observed.windows(2) {
            let gap = pair[1].0.duration_since(pair[0].0).unwrap_or_default();
            assert_eq!(
                gap, PERIOD,
                "case {case:?}: expected consecutive occurrences {PERIOD:?} apart, got {gap:?} \
                 between {:?} and {:?}",
                pair[0].0, pair[1].0
            );
        }
        for (scheduled, at) in &observed {
            assert!(
                at >= scheduled,
                "case {case:?}: the occurrence at {scheduled:?} was already delivered at {at:?}, \
                 before the instant it is scheduled for"
            );
        }
    }
}

/// Deleting a schedule group cascades to the schedules of THAT group only: the
/// identically named schedule of a sibling group keeps resolving and keeps firing — with
/// its own `Target.Input` — at an occurrence after the deleted group is gone, and
/// nothing of the deleted group is invoked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sibling_groups_same_named_schedule_keeps_firing_after_the_cascade() {
    let case = "one schedule name in two groups, over one shared target";
    let emulator = emulator();
    // ONE queue for both schedules, so what tells the two firings apart is the Input
    // each schedule states and nothing else.
    let (url, queue_arn) = given_queue(&emulator, case, "cascade-target").await;
    let role = given_execution_role(&emulator, case, "cascade-role", &queue_arn).await;
    let (cascaded, kept) = ("tenant-a", "tenant-b");
    for group in [cascaded, kept] {
        let (status, created) =
            scheduler(&emulator, "POST", &format!("/schedule-groups/{group}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateScheduleGroup {group} answered {status} {created}"
        );
    }

    // The SAME name in both groups, each with its own marker.
    let name = "shared-name";
    let mut arns = BTreeMap::new();
    for group in [cascaded, kept] {
        let arn = schedule_arn(
            case,
            &given_schedule(
                &emulator,
                case,
                name,
                json!({
                    "GroupName": group,
                    "ScheduleExpression": "rate(1 minute)",
                    "FlexibleTimeWindow": { "Mode": "OFF" },
                    "State": "ENABLED",
                    "Target": { "Arn": queue_arn, "RoleArn": role, "Input": target_input(group) },
                }),
            )
            .await,
        );
        assert!(
            arn.ends_with(&format!(":schedule/{group}/{name}")),
            "case {case:?}: expected the ARN of {name:?} in {group:?} to carry the group then \
             the name, got {arn:?}"
        );
        arns.insert(group, arn);
    }
    assert_ne!(
        arns[cascaded], arns[kept],
        "case {case:?}: expected the two groups' schedules to be two resources, got {:?} twice",
        arns[cascaded]
    );

    // Each is listed under its OWN group only, and the name resolves in neither the
    // group it does not live in nor the default group.
    for group in [cascaded, kept] {
        let (status, listed) = scheduler(
            &emulator,
            "GET",
            &format!("/schedules?ScheduleGroup={group}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListSchedules {group} answered {status} {listed}"
        );
        let held: Vec<(Option<&str>, Option<&str>)> = listed["Schedules"]
            .as_array()
            .map(|schedules| {
                schedules
                    .iter()
                    .map(|schedule| (schedule["Name"].as_str(), schedule["GroupName"].as_str()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            held,
            vec![(Some(name), Some(group))],
            "case {case:?}: expected {group:?} to list its own schedule only, got {listed}"
        );
    }
    let (status, refused) = schedule_read(&emulator, "default", name).await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            error_message(&refused).as_str()
        ),
        (
            404,
            "ResourceNotFoundException",
            format!("Schedule {name} does not exist.").as_str()
        ),
        "case {case:?}: expected the default group to hold no schedule of that name, got \
         {status} {refused}"
    );

    // Both fire their own payload first: the positive control the cascade is judged
    // against. Each poll looks again for the CONDITION; the deadline only bounds how
    // long the suite looks, and what is asserted is whether both payloads arrived.
    let deadline = SystemTime::now() + PERIOD + DELIVERY_SLACK;
    let mut markers: Vec<String> = Vec::new();
    let mut both_fired = false;
    while !both_fired && SystemTime::now() < deadline {
        markers = bodies(&emulator, &url)
            .await
            .iter()
            .map(|body| payload_of(case, body)["marker"].as_str().unwrap_or_default().to_string())
            .collect();
        both_fired = [cascaded, kept]
            .iter()
            .all(|group| markers.iter().any(|marker| marker == group));
        if !both_fired {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    assert!(
        both_fired,
        "case {case:?}: expected both groups' schedules to deliver their own payload, got \
         {markers:?}"
    );
    drain(&emulator, &url).await;

    let (status, deleted) = scheduler(
        &emulator,
        "DELETE",
        &format!("/schedule-groups/{cascaded}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteScheduleGroup answered {status} {deleted}"
    );
    // The removal is answered and then materialised, so the instant the deleted group
    // is GONE is the one the surviving schedule's next occurrence is judged against.
    let deadline = SystemTime::now() + GROUP_PATIENCE;
    let mut removed = None;
    let mut group_read = Value::Null;
    while removed.is_none() && SystemTime::now() < deadline {
        let (status, read) = scheduler(
            &emulator,
            "GET",
            &format!("/schedule-groups/{cascaded}"),
            None,
        )
        .await;
        group_read = read;
        if status == 404 {
            removed = Some(SystemTime::now());
        } else {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    let removed = removed.unwrap_or_else(|| {
        panic!("case {case:?}: the deleted group never stopped resolving: {group_read}")
    });

    // The sibling group's schedule is untouched by the cascade.
    let (status, read) = schedule_read(&emulator, kept, name).await;
    assert_eq!(
        (status, read["Arn"].as_str(), read["State"].as_str()),
        (200, Some(arns[kept].as_str()), Some("ENABLED")),
        "case {case:?}: expected {kept:?}'s schedule to keep resolving unchanged, got {status} \
         {read}"
    );

    // And it keeps firing: an occurrence of ITS series, scheduled after the deleted
    // group was gone, reaches the target — while nothing of the deleted group's
    // schedule is invoked again.
    let deadline = SystemTime::now() + PERIOD * 2 + DELIVERY_SLACK;
    let mut payloads: Vec<Value> = Vec::new();
    let mut fired_after_removal = false;
    while !fired_after_removal && SystemTime::now() < deadline {
        payloads = bodies(&emulator, &url)
            .await
            .iter()
            .map(|body| payload_of(case, body))
            .collect();
        for payload in &payloads {
            assert_ne!(
                payload["marker"].as_str(),
                Some(cascaded),
                "case {case:?}: expected the deleted group's schedule to be disarmed, got \
                 {payload}"
            );
        }
        fired_after_removal = payloads.iter().any(|payload| {
            payload["marker"].as_str() == Some(kept)
                && scheduled_time(payload).is_some_and(|scheduled| scheduled > removed)
        });
        if !fired_after_removal {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    assert!(
        fired_after_removal,
        "case {case:?}: expected an occurrence of {kept:?}'s schedule scheduled after the \
         instant the deleted group was gone, got {payloads:?}"
    );
}
