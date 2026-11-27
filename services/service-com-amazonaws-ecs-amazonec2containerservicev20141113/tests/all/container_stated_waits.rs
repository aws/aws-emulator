// SPDX-License-Identifier: CC0-1.0

//! What a container's program does AFTER a wait it names — and what the task it runs in then
//! reports.
//!
//! RULE under test: **a wait a container's program names is spent, wherever it stands in that
//! program, and the program then carries on from it: the step after the wait is taken, the
//! program reaches its own end, and the status it ends with is the `exitCode` its container
//! reports and the stop its task takes (`EssentialContainerExited`, "the task stopped because
//! an essential container exited"). A wait with no end of its own, and one the program goes
//! round a loop on, name no end for the program to carry on to, so those keep the container up
//! instead.**
//!
//! This is the other half of the rule [`crate::container_program_waits`] pins: there the wait
//! is minutes long, so the task is still RUNNING while the program waits it out; here the wait
//! is brief, so the same program is read past it and what the container reports is its own
//! program's outcome. Neither half depends on where the wait stands, which is the axis this
//! suite's rows vary.
//!
//! Sources: `smithy_query(query="operation",operation="DescribeTasks")` for `Task$lastStatus`,
//! `Task$stopCode` and `Container$exitCode` ("the exit code returned from the container");
//! `smithy_query(query="shape",shape="TaskStopCode")` for the stop code of a task whose
//! essential container exited. POSIX states what the wait means for the program: "the `sleep`
//! utility shall suspend execution for at least the integral number of seconds specified by the
//! `time` operand".
//!
//! The rows differ along what the rule generalises over: WHERE the wait stands in the program
//! (between two steps, among several, inside a branch the program carries on from, before the
//! second half of an and-or list, as the last thing the program does, as the whole program),
//! the FORM the program is written in (a shell program and the argument vector the runtime
//! exec's), how the duration is WRITTEN (whole and fractional seconds), the STATUS the program
//! then ends with (success and several failures), and the BOUNDARY — the waits that name no end
//! for a program to carry on to.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, poll_until, present_count, service, string};

/// The cluster every task below runs in.
const CLUSTER: &str = "statedwaits";

/// The image every container below names. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// How long the rows that must GO ON running are watched, re-read inside the loop that spends
/// it. Their waits name no end of their own, so a run that honoured one is still running at the
/// end of this window however slowly the host got there.
const WATCH_WINDOW: Duration = Duration::from_secs(2);

/// How often that window re-reads.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);

/// `Task$lastStatus` members this suite names.
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task whose essential container exited.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// What one row's program does once its wait is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ends {
    /// The program carries on from the wait to its own end: the task stops with this exit
    /// status.
    Exits(i64),
    /// The program's wait names no end of its own, so the container stays up and the task
    /// goes on RUNNING through [`WATCH_WINDOW`].
    RunsOn,
}

/// One row: the `command` the container states and what the task then does.
struct Row {
    case: &'static str,
    command: &'static [&'static str],
    ends: Ends,
}

const ROWS: &[Row] = &[
    Row {
        case: "a wait between two steps",
        command: &["/bin/sh", "-c", "sleep 1; exit 7"],
        ends: Ends::Exits(7),
    },
    Row {
        case: "a wait among several steps",
        command: &["/bin/sh", "-c", "echo one; sleep 1; echo two; exit 0"],
        ends: Ends::Exits(0),
    },
    Row {
        // The wait stands inside a compound form, and what follows it is outside that form:
        // the program carries on from the wait to the step after the branch.
        case: "a wait inside a branch the program carries on from",
        command: &["/bin/sh", "-c", "if true; then sleep 1; fi; exit 3"],
        ends: Ends::Exits(3),
    },
    Row {
        // An and-or list: the second half of it is the step the program carries on to.
        case: "a wait before the second half of an and-or list",
        command: &["/bin/sh", "-c", "sleep 1 && exit 5"],
        ends: Ends::Exits(5),
    },
    Row {
        // The wait is the last thing the program has to do, and it names an end of its own:
        // the program comes to its end there, with the status the wait answered.
        case: "a wait the program ends on",
        command: &["/bin/sh", "-c", "echo up; sleep 1"],
        ends: Ends::Exits(0),
    },
    Row {
        // The container's WHOLE program, as the vector the runtime exec's.
        case: "a wait the program states as its whole argument vector",
        command: &["sleep", "1"],
        ends: Ends::Exits(0),
    },
    Row {
        // The same program in its SHELL spelling: one reading, so one outcome.
        case: "a wait the program states as its whole shell program",
        command: &["/bin/sh", "-c", "sleep 1"],
        ends: Ends::Exits(0),
    },
    Row {
        case: "a wait written in fractional seconds",
        command: &["/bin/sh", "-c", "sleep 0.5; exit 2"],
        ends: Ends::Exits(2),
    },
    Row {
        case: "a wait the program goes round a loop on",
        command: &["sh", "-c", "while true; do sleep 1; done"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait with no end of its own",
        command: &["sh", "-c", "echo up; sleep infinity"],
        ends: Ends::RunsOn,
    },
];

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
    let tasks = list(&body, "tasks", case);
    tasks
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task {arn} back, got {body}"))
}

/// The exit status the one container of a described task reports, and `None` for a container
/// that reported none.
fn exit_code_of(task: &Value, case: &str) -> Option<i64> {
    let containers = list(task, "containers", case);
    let container = containers
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected one container, got {task}"));
    present_count(container, "exitCode", case)
}

/// RULE: a program carries on from a wait it spent, wherever that wait stood, and the task
/// stops on the status that program ended with; a wait that names no end of its own keeps the
/// container up instead.
#[tokio::test]
async fn a_program_carries_on_from_the_wait_it_states() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "waits a program states").await;

    // Every row's task is launched FIRST and read afterwards: the runs happen in parallel in
    // the one emulator, so the suite waits for the longest of them rather than for their sum.
    let mut launched: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("statedwait{index}");
        ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": &family,
                "containerDefinitions": [{
                    "name": "app",
                    "image": IMAGE,
                    "memory": 128,
                    "essential": true,
                    "command": row.command,
                }],
            }),
            row.case,
        )
        .await;
        launched.push((row, run_task(&router, &family, row.case).await));
    }

    // The rows whose program carries on from its wait: the task stops on the status that
    // program exited with.
    for (row, arn) in &launched {
        let Ends::Exits(status) = row.ends else {
            continue;
        };
        let case = row.case;
        let task = poll_until(
            case,
            "the task to stop on the status its program exited with",
            || described(&router, arn, case),
            |task| task["lastStatus"] == json!(STOPPED),
        )
        .await;
        assert_eq!(
            string(&task, "stopCode", case),
            ESSENTIAL_CONTAINER_EXITED,
            "case {case:?}: the task stopped because its essential container exited, got {task}"
        );
        assert_eq!(
            exit_code_of(&task, case),
            Some(status),
            "case {case:?}: the container reports the status its own program exited with, got \
             {task}"
        );
    }

    // The rows whose wait names no end of its own: each task reaches `RUNNING` and STAYS
    // there. Watched in ONE loop over every such row, so the window is spent once
    // rather than once per row, and re-read INSIDE it, so what the assertion rests on is the
    // condition holding read after read.
    let waiting: Vec<(&Row, &String)> = launched
        .iter()
        .filter(|(row, _)| row.ends == Ends::RunsOn)
        .map(|(row, arn)| (*row, arn))
        .collect();
    for (row, arn) in &waiting {
        let case = row.case;
        poll_until(
            case,
            &format!("the task to reach {RUNNING}"),
            || described(&router, arn, case),
            |task| task["lastStatus"] == json!(RUNNING) || task["lastStatus"] == json!(STOPPED),
        )
        .await;
    }
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        tokio::time::sleep(WATCH_INTERVAL).await;
        for (row, arn) in &waiting {
            let case = row.case;
            let task = described(&router, arn, case).await;
            assert_eq!(
                string(&task, "lastStatus", case),
                RUNNING,
                "case {case:?}: a container whose wait names no end of its own stays up, so \
                 its task goes on running, got {task}"
            );
            assert_eq!(
                exit_code_of(&task, case),
                None,
                "case {case:?}: no exit status is reported for a program that has not ended, \
                 got {task}"
            );
        }
    }
}
