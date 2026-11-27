// SPDX-License-Identifier: CC0-1.0

//! What a container's health-check PROBE is performed with — driven end to end over the shared
//! wire harness in [`crate::ecs_wire`].
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A container's health check RUNS INSIDE that container ("The Amazon ECS container agent
//! > only monitors and reports on the health checks specified in the task definition"), so it is
//! > performed in the same session the container's own `command` is, with the same utilities and
//! > off the same CLOCK: a probe written as a question about the instant it ran at is answered,
//! > and the status it answers with is read as `HealthCheck$command` documents it — "An exit
//! > code of 0 indicates success, and non-zero exit code indicates failure" — so such a probe
//! > takes its container to `HEALTHY` or, once it has failed `retries` times, to `UNHEALTHY`.
//!
//! This is the probe's half of the rule [`crate::container_clock_readings`] pins for a
//! container's own program, and it matters for the same reason: the gate a `dependsOn HEALTHY`
//! condition opens is opened by a probe, so a probe that cannot be performed is a gate that
//! never opens.
//!
//! Sources: `smithy_query(query="shape",shape="HealthCheck")` for `command` ("The string array
//! must start with `CMD` to run the command arguments directly, or `CMD-SHELL` to run the
//! command with the container's default shell"), `interval`, `timeout` and `retries`;
//! `smithy_query(query="shape",shape="HealthStatus")` for the readings; and `date(1)` for the
//! option and format spellings ("`-d`, `--date=STRING`", "`-I[FMT]`, `--iso-8601[=FMT]`", "`-u`,
//! `--utc`").
//!
//! The rows differ along what the rule generalises over: the FORM the command takes (`CMD-SHELL`
//! and `CMD`, the two forms the member documents), WHICH reading of the clock the question is
//! about (the epoch seconds, an instant the probe names, the calendar year), and the VERDICT the
//! question reaches (passing, failing).
//!
//! The instants the rows name are values of the civil calendar rather than of any one run:
//! 1500000000 seconds after the epoch is 2017-07-14 on that calendar, and 1700000000 is an
//! instant in the past of every run of this suite.
//!
//! Nothing here reaches into the state struct: every reading asserted comes back through
//! `DescribeTasks`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, poll_until, service, string};

/// The cluster every task below runs in.
const CLUSTER: &str = "clockprobes";

/// The image every container below names. Nothing is fetched from it: what runs is the
/// `command` the revision states.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The program the probed container itself runs: it stays up, so the probe has a running
/// container to be performed in for as long as the row needs.
const STAYS_UP: &[&str] = &["sh", "-c", "sleep infinity"];

/// `HealthCheck$interval` every row declares: the smallest the member admits ("You may specify
/// between 5 and 300 seconds"), so a row's verdict lands inside the suite's poll budget without
/// naming a value AWS refuses.
const INTERVAL_SECONDS: i64 = 5;

/// `HealthCheck$timeout` every row declares: the smallest the member admits ("You may specify
/// between 2 and 60 seconds").
const TIMEOUT_SECONDS: i64 = 2;

/// `HealthCheck$retries` every row declares: the smallest the member admits ("You may specify
/// between 1 and 10 retries"), so a failing row is believed on its first execution.
const RETRIES: i64 = 1;

/// One row: the probe command, and the reading its container must reach.
struct Row {
    case: &'static str,
    command: &'static [&'static str],
    reaches: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        // The shell form, over the reading a script stamps its work with: the epoch seconds
        // stand past an instant that is in the past of every run.
        case: "a shell-form probe about the epoch seconds",
        command: &["CMD-SHELL", "test \"$(date +%s)\" -gt 1700000000"],
        reaches: "HEALTHY",
    },
    Row {
        // The exec form, whose "command arguments" are run directly: the reading is of an
        // instant the probe NAMES, and the status is the one the program itself answered.
        case: "an exec-form probe reading an instant it names",
        command: &["CMD", "date", "-u", "-d", "@1500000000", "-I"],
        reaches: "HEALTHY",
    },
    Row {
        // The same reading, asked a question it answers NO to: the calendar year is past the
        // one named, so the probe exits non-zero and the container is believed unhealthy once
        // its single retry is spent.
        case: "a probe whose question about the calendar year is answered no",
        command: &["CMD-SHELL", "test \"$(date -u +%Y)\" -lt 2000"],
        reaches: "UNHEALTHY",
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
    list(&body, "tasks", case)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task {arn} back, got {body}"))
}

/// What the one container of a described task reports as its `healthStatus`.
fn health_of(task: &Value, case: &str) -> String {
    let containers = list(task, "containers", case);
    let container = containers
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected one container, got {task}"));
    string(container, "healthStatus", case)
}

/// RULE: a health check is performed in the container's own session, off the same clock its
/// program reads, and the status the question answers with is the reading the container
/// reports.
#[tokio::test]
async fn a_probe_reads_the_clock_of_the_container_it_runs_in() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "probes over the clock").await;

    // Every row's task is launched FIRST and read afterwards: the probes run in parallel in the
    // one emulator, so the suite waits for the longest of them rather than for their sum.
    let mut launched: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("clockprobe{index}");
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
                    "command": STAYS_UP,
                    "healthCheck": {
                        "command": row.command,
                        "interval": INTERVAL_SECONDS,
                        "timeout": TIMEOUT_SECONDS,
                        "retries": RETRIES,
                    },
                }],
            }),
            row.case,
        )
        .await;
        launched.push((row, run_task(&router, &family, row.case).await));
    }

    for (row, arn) in &launched {
        let case = row.case;
        let wanted = row.reaches;
        let task = poll_until(
            case,
            &format!("the container to report {wanted}"),
            || described(&router, arn, case),
            |task| health_of(task, case) == wanted,
        )
        .await;
        // The TASK's own reading is the reading of its essential containers, and this task has
        // exactly one.
        assert_eq!(
            string(&task, "healthStatus", case),
            wanted,
            "case {case:?}: a task reports what its essential container reports, got {task}"
        );
    }
}
