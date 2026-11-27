// SPDX-License-Identifier: CC0-1.0

//! What a wait a container's PROGRAM names means for the task it runs in, driven end to end
//! over the shared wire harness in [`crate::ecs_wire`] so every value asserted is read back
//! through `DescribeTasks`.
//!
//! RULE under test: **a wait a container's program names is time that container is UP. "The
//! `sleep` utility shall suspend execution for at least the integral number of seconds
//! specified by the `time` operand" (POSIX), so the program has not reached its next step —
//! nor its own end, nor the status that end carries — while one of its waits is outstanding:
//! the task goes on reporting `lastStatus` `RUNNING` with no `stopCode`, `stoppedReason` or
//! `stoppedAt`, and its container reports no `exitCode`. Only when the program really ENDS
//! does the task stop, with `EssentialContainerExited` and the status that program exited
//! with. None of this depends on WHERE the wait stands in the program, on which spelling the
//! program is written in, or on who placed the task.**
//!
//! Sources: `smithy_query(query="operation",operation="DescribeTasks")` for `Task$lastStatus`
//! ("the last known status for the task"), `Task$stoppedReason` ("the reason that the task was
//! stopped"), `Task$stopCode` and `Container$exitCode` ("the exit code returned from the
//! container"); `smithy_query(query="shape",shape="TaskStopCode")` for
//! `EssentialContainerExited`, the code of a task that "stopped because an essential container
//! exited".
//!
//! How much wall clock ONE such wait may spend is this emulator's own bound, not the request's,
//! so every row here names a wait minutes long and every reading is taken far inside that
//! bound: what the rows are about is that the wait is outstanding — the container up, the task
//! running, no status claimed for a program that has not ended — and never how long anything
//! took.
//!
//! The rows differ along what the rule generalises over: WHERE the wait stands in the program
//! (between two steps, among several, inside a branch the program carries on from, before the
//! second half of an and-or list, as the last thing the program does, as the whole program,
//! inside a loop), the FORM the program is written in (a shell program, the argument vector the
//! runtime exec's, an `entryPoint`/`command` pair), HOW LONG the wait names (an hour, twenty
//! minutes, fifteen, ten, fractional seconds, no end at all, and a brief one), and the BOUNDARY
//! — a program that really ends, after a wait of its own and with no wait at all, which is the
//! only thing that earns a stop record.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    assert_absent, cluster_with_capacity, list, ok, poll_until, present_count, service, string,
};

/// The cluster every task below runs in.
const CLUSTER: &str = "programwaits";

/// The image every container below names. Nothing is fetched from it: what runs is the program
/// the revision states.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` members this suite names.
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task whose essential container exited, and the sentence Amazon ECS
/// words that stop with. Spelled out here rather than imported, so a wrong wording in the
/// implementation cannot make this suite pass.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";
const ESSENTIAL_CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// How long the rows whose wait is still outstanding are watched once every row that ENDS has
/// ended, re-read inside the loop that spends it: a wait that was collapsed instead of spent
/// would have ended its program, so a task still running read after read is one whose program
/// really is waiting. Every such row's wait is minutes long, far beyond this window.
const WATCH_WINDOW: Duration = Duration::from_secs(2);

/// How often that window re-reads.
const WATCH_INTERVAL: Duration = Duration::from_millis(100);

/// What one row's program does with the wait it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ends {
    /// The wait is still outstanding for far longer than this suite runs, so the container is
    /// up and the task goes on RUNNING with no exit status reported for it.
    RunsOn,
    /// The program reaches its own end and exits with this status, which is the one thing that
    /// earns a stop record.
    Exits(i64),
}

/// One row: the program its single essential container is given, and what the task then does.
struct Row {
    case: &'static str,
    /// `ContainerDefinition$entryPoint`, which the runtime exec's before the command.
    entry_point: Option<&'static [&'static str]>,
    /// `ContainerDefinition$command`.
    command: &'static [&'static str],
    ends: Ends,
}

const ROWS: &[Row] = &[
    Row {
        case: "a wait between two steps of a shell program",
        entry_point: None,
        command: &["/bin/sh", "-c", "sleep 600; exit 7"],
        ends: Ends::RunsOn,
    },
    Row {
        // The reported shape: a long-lived workload that says it is up and then waits.
        case: "a wait among several steps of a shell program",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo one; sleep 1200; echo two; exit 0"],
        ends: Ends::RunsOn,
    },
    Row {
        // The wait stands inside a compound form and the program has a step after it: the
        // program is still at the wait, so the step after it has not happened.
        case: "a wait inside a branch the program carries on from",
        entry_point: None,
        command: &["/bin/sh", "-c", "if true; then sleep 900; fi; exit 3"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait before the second half of an and-or list",
        entry_point: None,
        command: &["/bin/sh", "-c", "sleep 3600 && exit 5"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait a shell program ends on, after a step that wrote",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo up; sleep 1800"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait in fractional seconds among steps of a shell program",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo up; sleep 240.5; exit 2"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait that is the whole shell program",
        entry_point: None,
        command: &["/bin/sh", "-c", "sleep 3600"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait that is the whole argument vector",
        entry_point: None,
        command: &["sleep", "600"],
        ends: Ends::RunsOn,
    },
    Row {
        // The vector the runtime exec's is the entry point followed by the command.
        case: "the same wait spelled as an entry point and its argument",
        entry_point: Some(&["sleep"]),
        command: &["900"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait with no end of its own after a step that wrote",
        entry_point: None,
        command: &["sh", "-c", "echo up; sleep infinity"],
        ends: Ends::RunsOn,
    },
    Row {
        case: "a wait the program goes round a loop on",
        entry_point: None,
        command: &["sh", "-c", "while true; do sleep 1; done"],
        ends: Ends::RunsOn,
    },
    Row {
        // The boundary: the program comes to its own end, after a wait short enough that this
        // suite sees the end rather than the waiting.
        case: "a brief wait between two steps of a shell program",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo up; sleep 1; exit 7"],
        ends: Ends::Exits(7),
    },
    Row {
        case: "a brief wait a shell program ends on",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo up; sleep 1"],
        ends: Ends::Exits(0),
    },
    Row {
        case: "a brief wait that is the whole argument vector",
        entry_point: None,
        command: &["sleep", "1"],
        ends: Ends::Exits(0),
    },
    Row {
        case: "a program with no wait in it at all",
        entry_point: None,
        command: &["/bin/sh", "-c", "echo leaving; exit 4"],
        ends: Ends::Exits(4),
    },
];

/// Register one revision of `family` running the row's program, place one task of it on its
/// own, and answer that task's ARN.
async fn place(router: &MultiServiceRouter, family: &str, row: &Row) -> String {
    let case = row.case;
    let mut container = json!({
        "name": "app",
        "image": IMAGE,
        // "at least one of 'memory' or 'memoryReservation' must be specified" for a container
        // of a revision this service places.
        "memory": 128,
        "essential": true,
        "command": row.command,
    });
    if let Some(entry_point) = row.entry_point {
        container["entryPoint"] = json!(entry_point);
    }
    ok(
        router,
        "RegisterTaskDefinition",
        json!({ "family": family, "containerDefinitions": [container] }),
        case,
    )
    .await;
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
        .unwrap_or_else(|| panic!("case {case:?}: expected RunTask to place one task, got {body}"));
    string(task, "taskArn", case)
}

/// One task, as `DescribeTasks` reports it.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    list(&body, "tasks", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task {task_arn} back, got {body}"))
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

/// Everything the rule says of a task whose program is still waiting: it is up, it is still
/// wanted up, nothing about a stop of it is reported and its container reports no status its
/// program has not produced.
fn assert_still_waiting(task: &Value, row_case: &str, when: &str) {
    let case = format!("{row_case}, {when}");
    let case = case.as_str();
    assert_eq!(
        string(task, "lastStatus", case),
        RUNNING,
        "case {case:?}: a container whose program is waiting is up, so its task goes on \
         running, got {task}"
    );
    assert_eq!(
        string(task, "desiredStatus", case),
        RUNNING,
        "case {case:?}: expected the task to be wanted {RUNNING}, got {task}"
    );
    for member in ["stopCode", "stoppedReason", "stoppedAt", "stoppingAt"] {
        assert_absent(task, member, case);
    }
    assert_eq!(
        exit_code_of(task, case),
        None,
        "case {case:?}: no exit status is reported for a program that has not ended, got {task}"
    );
}

/// RULE: a wait a container's program names is time that container is up, so the task goes on
/// RUNNING until the program really ends — and only that end earns a stop record carrying the
/// status the program exited with.
#[tokio::test]
async fn a_task_runs_while_its_container_waits() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "waits a container's program names").await;

    // Every row's task is placed FIRST and read afterwards: the runs happen in parallel in the
    // one emulator, so the suite waits for the longest of them rather than for their sum.
    let mut placed: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("programwait{index}");
        placed.push((row, place(&router, &family, row).await));
    }

    // The BOUNDARY rows: the program reached its own end, so the task stopped on the status it
    // exited with, under the stop an essential container's exit earns.
    for (row, task_arn) in &placed {
        let Ends::Exits(status) = row.ends else {
            continue;
        };
        let case = row.case;
        let task = poll_until(
            case,
            "the task to stop on the status its program exited with",
            || describe(&router, task_arn, case),
            |task| task["lastStatus"] == json!(STOPPED),
        )
        .await;
        assert_eq!(
            string(&task, "stopCode", case),
            ESSENTIAL_CONTAINER_EXITED,
            "case {case:?}: the task stopped because its essential container exited, got {task}"
        );
        assert_eq!(
            string(&task, "stoppedReason", case),
            ESSENTIAL_CONTAINER_EXITED_REASON,
            "case {case:?}: expected the sentence Amazon ECS words that stop with, got {task}"
        );
        assert_eq!(
            exit_code_of(&task, case),
            Some(status),
            "case {case:?}: the container reports the status its own program exited with, got \
             {task}"
        );
    }

    // The rows whose wait is still outstanding: each task reached RUNNING and is STILL there,
    // now that every row whose program ends has ended — a collapsed wait would have ended
    // these programs too, on the same pass.
    let waiting: Vec<(&Row, &String)> = placed
        .iter()
        .filter(|(row, _)| row.ends == Ends::RunsOn)
        .map(|(row, task_arn)| (*row, task_arn))
        .collect();
    for (row, task_arn) in &waiting {
        let case = row.case;
        let task = poll_until(
            case,
            &format!("the task to reach {RUNNING}"),
            || describe(&router, task_arn, case),
            |task| {
                // A task that STOPPED is settled too: the poll ends on it so the assertion
                // below reports the stop rather than timing out on it.
                task["lastStatus"] == json!(RUNNING) || task["lastStatus"] == json!(STOPPED)
            },
        )
        .await;
        assert_still_waiting(&task, row.case, "once its start has fallen due");
    }

    // And still there read after read, through a window every one of those waits outlives.
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        tokio::time::sleep(WATCH_INTERVAL).await;
        for (row, task_arn) in &waiting {
            let task = describe(&router, task_arn, row.case).await;
            assert_still_waiting(&task, row.case, "on a later read");
        }
    }
}
