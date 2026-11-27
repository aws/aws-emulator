// SPDX-License-Identifier: CC0-1.0

//! What a container's program reads as the TIME, and what a program whose contract is a
//! measured SPAN then reports — driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rules under test, over the whole class rather than one request:
//!
//! > A container's program is performed with the utilities whose whole effect is inside its own
//! > session, and reading the CLOCK is one of them: `date` writes the instant it ran at, in the
//! > form the invocation asks for (`+FORMAT`, and the standard forms `date(1)` documents:
//! > `-u`, `-R`, `-I[FMT]`, `-d @SECONDS`), on the civil calendar in UTC. The status such a
//! > program ends with is the `exitCode` its container reports ("the exit code returned from
//! > the container"), so a program that computes over two readings reports what it computed;
//! > a form the session renders nothing of is a command that did not run, so that run claims no
//! > status for the container at all.
//!
//! > A container whose `dependsOn` names a `HEALTHY` condition does not START until the
//! > container it names passes its health check (`ContainerDependency$condition`: `HEALTHY`
//! > "validates that the dependent container passes its Docker health check before permitting
//! > other containers to start"), while `START` "validates that a dependent container is
//! > started before permitting other containers to start" and asks nothing of that check. So
//! > one gate HOLDS a dependent for as long as the check takes to pass — a span the dependent's
//! > own program measures off the clock above and reports as its exit status — and holds it for
//! > ever when the check cannot pass, while the other does not hold it at all.
//!
//! Sources: `smithy_query(query="shape",shape="ContainerDependency")` for `condition`, and
//! `smithy_query(query="operation",operation="DescribeTasks")` for `Container$exitCode`,
//! `Container$lastStatus` and `Container$healthStatus`. The option and format spellings are
//! `date(1)`'s own: "`-d`, `--date=STRING` display time described by STRING", "`-I[FMT]`,
//! `--iso-8601[=FMT]`", "`-R`, `--rfc-email` output date and time in RFC 5322 format", "`-u`,
//! `--utc`".
//!
//! The rows of the first table differ along what that rule generalises over: WHICH form the
//! reading is asked for in (the epoch seconds, the parts of the calendar date, an ISO instant,
//! an ISO date, the standard's date-time), whether the program computes over ONE reading or
//! TWO, and the BOUNDARY — a form the session renders nothing of. The second table's rows
//! differ along the CONDITION the gate carries (`HEALTHY`, `START`) and whether the check it
//! names ever passes.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, poll_until, present_count, service, string};

/// The cluster every task below runs in.
const CLUSTER: &str = "clockreadings";

/// The image every container below names. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How long the gate of the measured row takes to become ready, in seconds: its program waits
/// this long before it writes the file its health check looks for, so the dependent it gates is
/// held AT LEAST this long. The lower bound this suite asserts, and the only tight limit in it.
const READY_SECONDS: i64 = 3;

/// `Container$lastStatus` members this suite names.
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// How many times the rows that assert an INVARIANT look again. Each look is a fresh
/// `DescribeTasks`, which materialises whatever the clock owes, so the invariant is asserted
/// against a store that has had every chance to move.
const LOOKS_FOR_AN_INVARIANT: usize = 5;

/// How long one of those looks waits before the next.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// What one row of the first table expects of the container it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reports {
    /// The program ran to its end and the container reports this exit status.
    Exits(i64),
    /// The session renders nothing of what the program asked for, so the run claims no status:
    /// the container reports no `exitCode` and its task is not stopped as one whose essential
    /// container exited.
    Nothing,
}

/// One row of the first table: the program the container runs, and what it must report.
struct Row {
    case: &'static str,
    program: &'static str,
    reports: Reports,
}

/// The rows, each a program written the way a task definition writes one: a `sh -c` line that
/// reads the clock and ends with the status its own test of that reading chose. A row that
/// finds what it asked for exits zero; the status it would exit with otherwise is its own, so a
/// row that fails says which test of the reading failed.
///
/// The instants the rows NAME are values of the civil calendar rather than of any one run:
/// 1500000000 seconds after the epoch is 2017-07-14T02:40:00Z, a Friday, and 1700000000 is an
/// instant in the past of every run of this suite.
const ROWS: &[Row] = &[
    Row {
        // `+%s` is the epoch seconds, and the reading is a real one: it stands past an instant
        // that is in the past of every run. Nothing is asserted about how far past.
        case: "the epoch seconds, which are a reading past a past instant",
        program: "at=$(date +%s); if test \"$at\" -gt 1700000000; then exit 0; else exit 1; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // The PARTS of the calendar date agree with the date itself, so the specifiers are
        // rendered off ONE instant rather than each off something of its own.
        case: "the parts of the calendar date, which agree with the date",
        program: "whole=$(date -u +%Y-%m-%d); \
                  parts=$(date -u +%Y)-$(date -u +%m)-$(date -u +%d); \
                  if test \"$whole\" = \"$parts\"; then exit 0; else exit 2; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // An instant the invocation NAMES, written in the `strftime` form it asked for.
        case: "an instant the invocation names, written as an ISO instant",
        program: "at=$(date -u -d @1500000000 +%Y-%m-%dT%H:%M:%SZ); \
                  if test \"$at\" = \"2017-07-14T02:40:00Z\"; then exit 0; else exit 3; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // The same instant in the ISO form the `-I` option names, whose default precision is
        // the date alone.
        case: "an instant the invocation names, written as an ISO date",
        program: "at=$(date -u -d @1500000000 -I); \
                  if test \"$at\" = \"2017-07-14\"; then exit 0; else exit 4; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // And in the form `-R` names, which carries the day of the week and the offset.
        case: "an instant the invocation names, written as the standard's date-time",
        program: "at=$(date -u -d @1500000000 -R); \
                  if test \"$at\" = \"Fri, 14 Jul 2017 02:40:00 +0000\"; then exit 0; \
                  else exit 5; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // TWO readings, and the program computes over them: the later one is never before the
        // earlier one, and their difference is a number the program can exit with.
        case: "two readings, computed over",
        program: "first=$(date +%s); second=$(date +%s); \
                  if test \"$second\" -ge \"$first\"; then exit $((second - first)); \
                  else exit 6; fi",
        reports: Reports::Exits(0),
    },
    Row {
        // THE BOUNDARY: an option this session reads nothing of. The program that named it ran
        // none of what the image holds for it, so no status of the container's own was produced
        // and none is reported.
        case: "an option the session renders nothing of",
        program: "date -Q; exit 0",
        reports: Reports::Nothing,
    },
];

/// `RegisterTaskDefinition` one family whose single essential container runs `program`.
async fn register_one(router: &MultiServiceRouter, family: &str, program: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "memory": 128,
                "essential": true,
                "command": ["sh", "-c", program],
            }],
        }),
        case,
    )
    .await;
}

/// `RunTask` one task of `family` in [`CLUSTER`], and answer its ARN.
async fn run_task(router: &MultiServiceRouter, family: &str, case: &str) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": family }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    let task = tasks
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected a task back, got {body}"));
    string(task, "taskArn", case)
}

/// The one task `DescribeTasks` reports for `arn`.
async fn described(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task {arn} back, got {body}"))
}

/// The container `name` of a described task.
fn container_of(task: &Value, name: &str, case: &str) -> Value {
    list(task, "containers", case)
        .into_iter()
        .find(|container| container["name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: expected a container {name}, got {task}"))
}

/// What container `name` reports as its exit status, and `None` for one that reports none.
fn exit_code_of(task: &Value, name: &str, case: &str) -> Option<i64> {
    present_count(&container_of(task, name, case), "exitCode", case)
}

/// What container `name` reports as its `lastStatus`.
fn status_of(task: &Value, name: &str, case: &str) -> String {
    string(&container_of(task, name, case), "lastStatus", case)
}

/// RULE: a container's program reads the clock with `date`, in the form the invocation asks
/// for, and the status it computes out of those readings is the `exitCode` its container
/// reports — while a form the session renders nothing of leaves the run claiming nothing.
#[tokio::test]
async fn a_container_program_reads_the_clock_in_the_form_it_asks_for() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "clock readings").await;

    // Every row's task is launched FIRST and read afterwards: the runs happen in parallel in
    // the one emulator, so the suite waits for the longest of them rather than for their sum.
    let mut launched: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("clockreading{index}");
        register_one(&router, &family, row.program, row.case).await;
        launched.push((row, run_task(&router, &family, row.case).await));
    }

    for (row, arn) in &launched {
        let case = row.case;
        let Reports::Exits(status) = row.reports else {
            continue;
        };
        let task = poll_until(
            case,
            "the task to stop on the status its program computed",
            || described(&router, arn, case),
            |task| task["lastStatus"] == json!(STOPPED),
        )
        .await;
        assert_eq!(
            exit_code_of(&task, "app", case),
            Some(status),
            "case {case:?}: expected the container to report exitCode {status}, got {task}"
        );
    }

    // The BOUNDARY row: the program named a form this session renders nothing of, so the run
    // claims no status for the container. Asserted over repeated reads, each of which
    // materialises whatever the clock owes.
    for (row, arn) in &launched {
        let case = row.case;
        if row.reports != Reports::Nothing {
            continue;
        }
        let running = poll_until(
            case,
            &format!("the container to reach {RUNNING}"),
            || described(&router, arn, case),
            |task| status_of(task, "app", case) == RUNNING,
        )
        .await;
        assert_eq!(
            exit_code_of(&running, "app", case),
            None,
            "case {case:?}: a run that reached a form the session performs nothing of claims no \
             status for its container, got {running}"
        );
        for look in 0..LOOKS_FOR_AN_INVARIANT {
            tokio::time::sleep(LOOK_INTERVAL).await;
            let task = described(&router, arn, case).await;
            assert_eq!(
                exit_code_of(&task, "app", case),
                None,
                "case {case:?}: no status is reported for work nothing ran (look {look}), got \
                 {task}"
            );
            assert_eq!(
                string(&task, "lastStatus", case),
                RUNNING,
                "case {case:?}: a task whose essential container produced no outcome is not \
                 stopped as one whose essential container exited (look {look}), got {task}"
            );
        }
    }
}

/// One row of the second table: the condition the gate carries, whether the gate's check ever
/// passes, and what the container it gates then does.
struct Gate {
    case: &'static str,
    /// `ContainerDependency$condition`.
    condition: &'static str,
    /// Whether the gate's program ever writes the file its health check looks for.
    becomes_ready: bool,
    /// Whether the container it gates ever starts.
    starts: bool,
}

const GATES: &[Gate] = &[
    Gate {
        // The check PASSES, after the gate's own program has taken its time over it: the
        // dependent starts then, and the span it was held is what its program measures.
        case: "a HEALTHY gate whose check passes after a delay",
        condition: "HEALTHY",
        becomes_ready: true,
        starts: true,
    },
    Gate {
        // The same gate with a check that never passes: `HEALTHY` asks for that check, so the
        // dependent never starts at all.
        case: "a HEALTHY gate whose check never passes",
        condition: "HEALTHY",
        becomes_ready: false,
        starts: false,
    },
    Gate {
        // `START` asks nothing of the check, so that same never-passing gate holds nothing: the
        // dependent starts as soon as the gate is running.
        case: "a START gate whose check never passes",
        condition: "START",
        becomes_ready: false,
        starts: true,
    },
];

/// The gate container's program: it stamps the shared volume with the instant it started and —
/// for a row whose check is to pass — writes the file that check looks for after
/// [`READY_SECONDS`]. It then stays up, so nothing but the dependent's own exit stops the task.
fn gate_program(becomes_ready: bool) -> String {
    let ready = if becomes_ready {
        format!("sleep {READY_SECONDS}; touch /shared/ready; ")
    } else {
        String::new()
    };
    format!("date +%s > /shared/at; {ready}sleep infinity")
}

/// The dependent's program: it waits for the stamp to be THERE rather than for a span of its
/// own, then measures the span between that stamp and its own reading of the clock and exits
/// with it — so the wait its gate held it for is what its container reports.
const DEPENDENT_PROGRAM: &str = "while test ! -f /shared/at; do sleep 1; done; \
                                 now=$(date +%s); at=$(cat /shared/at); exit $((now - at))";

/// RULE: a `HEALTHY` condition holds the container that waits on it until the gate's check
/// passes — a span the dependent's own program measures off the clock and reports as its exit
/// status — and holds it for ever when that check cannot pass, while a `START` condition asks
/// nothing of the check at all.
#[tokio::test]
async fn a_dependent_waits_for_the_condition_its_gate_names() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "gated starts").await;

    let mut launched: Vec<(&Gate, String)> = Vec::new();
    for (index, gate) in GATES.iter().enumerate() {
        let family = format!("clockgate{index}");
        ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": &family,
                "volumes": [{ "name": "shared", "host": {} }],
                "containerDefinitions": [
                    {
                        "name": "gate",
                        "image": IMAGE,
                        "memory": 128,
                        // Not essential: its exit must not be what stops the task — and it does
                        // not exit at all.
                        "essential": false,
                        "command": ["sh", "-c", gate_program(gate.becomes_ready)],
                        "mountPoints": [
                            { "sourceVolume": "shared", "containerPath": "/shared" },
                        ],
                        // The check runs INSIDE the gate, so it looks for the file that
                        // container's own program writes. The interval is the smallest the
                        // member admits ("You may specify between 5 and 300 seconds").
                        "healthCheck": {
                            "command": ["CMD-SHELL", "test -f /shared/ready"],
                            "interval": 5,
                            "timeout": 2,
                            "retries": 1,
                        },
                    },
                    {
                        "name": "app",
                        "image": IMAGE,
                        "memory": 128,
                        "essential": true,
                        "command": ["sh", "-c", DEPENDENT_PROGRAM],
                        "mountPoints": [
                            { "sourceVolume": "shared", "containerPath": "/shared" },
                        ],
                        "dependsOn": [
                            { "containerName": "gate", "condition": gate.condition },
                        ],
                    },
                ],
            }),
            gate.case,
        )
        .await;
        launched.push((gate, run_task(&router, &family, gate.case).await));
    }

    for (gate, arn) in &launched {
        let case = gate.case;
        if !gate.starts {
            continue;
        }
        let task = poll_until(
            case,
            "the gated container to start and report the wait it measured",
            || described(&router, arn, case),
            |task| exit_code_of(task, "app", case).is_some(),
        )
        .await;
        let measured = exit_code_of(&task, "app", case).unwrap_or_default();
        // A gate whose check has to pass FIRST cannot have let its dependent start before the
        // span its own program spends on becoming ready; one that holds nothing has no floor
        // but zero.
        let floor = if gate.becomes_ready { READY_SECONDS } else { 0 };
        assert!(
            measured >= floor,
            "case {case:?}: expected the measured wait to be at least {floor} seconds, got \
             {measured} in {task}"
        );
    }

    // The row whose condition can never be met: the dependent never starts. Asserted over
    // repeated reads, each of which materialises whatever the clock owes — and the gate it
    // waits on IS running, so what holds it is the condition rather than the gate.
    for (gate, arn) in &launched {
        let case = gate.case;
        if gate.starts {
            continue;
        }
        let running = poll_until(
            case,
            "the gate itself to be running",
            || described(&router, arn, case),
            |task| status_of(task, "gate", case) == RUNNING,
        )
        .await;
        assert_eq!(
            status_of(&running, "app", case),
            PENDING,
            "case {case:?}: a container whose condition is not met does not start, got {running}"
        );
        for look in 0..LOOKS_FOR_AN_INVARIANT {
            tokio::time::sleep(LOOK_INTERVAL).await;
            let task = described(&router, arn, case).await;
            assert_eq!(
                status_of(&task, "app", case),
                PENDING,
                "case {case:?}: the condition is still not met, so the container is still not \
                 started (look {look}), got {task}"
            );
            assert_eq!(
                exit_code_of(&task, "app", case),
                None,
                "case {case:?}: a container that never started reports no exit status (look \
                 {look}), got {task}"
            );
        }
    }
}
