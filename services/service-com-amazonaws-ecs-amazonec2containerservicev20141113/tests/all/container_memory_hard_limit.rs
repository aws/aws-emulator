// SPDX-License-Identifier: CC0-1.0

//! The MEMORY a task definition gives a container is a HARD limit on its run, driven end to
//! end over HTTP.
//!
//! RULE under test: `ContainerDefinition$memory` is "the amount (in MiB) of memory to present
//! to the container", and "if your container attempts to exceed the memory specified here, the
//! container is killed" — for a container that states none, the memory its TASK was registered
//! with is what it may use. So a container whose program holds more than that much is KILLED
//! part of the way through it: it reports the exit status a killed process carries and the
//! runtime's own reason for the kill, the task it was the essential container of is stopped
//! with `EssentialContainerExited`, and its log stream holds what it wrote BEFORE the kill and
//! nothing it would have written afterwards. A container that stays inside the limit runs to
//! its end and exits with its program's own status.
//!
//! The rows differ along what the rule generalises over: the limit the definition states,
//! WHERE it states it (on the container or on the task), and how much memory the program holds
//! — including the same program under two different limits, which is what makes the KILL the
//! limit's doing rather than the allocation's.
//!
//! CloudWatch Logs is mounted BESIDE Amazon ECS on the SAME engine, so what the `awslogs`
//! driver delivered is read back through that service's own `GetLogEvents`: nothing here reads
//! the producer's own buffer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, logs, ok, seed_awsvpc_subnets, string, SIGNED_REGION};

/// The log group every container below delivers to.
const LOG_GROUP: &str = "/emulator/ecs/memory-limits";

/// The `awslogs-stream-prefix` the task definitions below configure.
const STREAM_PREFIX: &str = "mem";

/// The one container of every task definition below.
const CONTAINER: &str = "app";

/// The image the containers start from. Nothing is fetched from it: what runs is the `command`
/// the revision states.
const IMAGE: &str = "public.ecr.aws/amazonlinux/amazonlinux:2023";

/// What the program writes BEFORE it allocates anything, which every row's container reaches.
const ALLOC_START: &str = "ALLOC_START";

/// What it writes once it has held everything it set out to hold — the completion marker a
/// container the runtime killed never reaches.
const ALLOC_DONE: &str = "ALLOC_DONE";

/// `Container$reason` of a container the runtime killed for going past its memory limit,
/// spelled out here rather than imported so a wrong constant in the implementation cannot make
/// a test pass. The container runtime's own sentence for such a kill, which is also what the
/// agent reports for one.
const OUT_OF_MEMORY_REASON: &str = "OutOfMemoryError: Container killed due to memory usage";

/// The `exitCode` of a container a `SIGKILL` ended: the status of a process a signal ended is
/// 128 plus the signal's number, and `SIGKILL` is signal 9.
const KILLED_EXIT_CODE: i64 = 137;

/// `Task$stopCode` of a task whose essential container has ended.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// How many bytes the literal each program starts its chunk from carries.
const CHUNK_SEED_BYTES: usize = 1024;

/// How many times the program DOUBLES that literal, so one chunk is
/// `CHUNK_SEED_BYTES << CHUNK_DOUBLINGS` bytes — 64 KiB, small enough that the number of
/// copies a row holds decides how near its limit it gets.
const CHUNK_DOUBLINGS: u32 = 6;

/// How many bytes one chunk comes to.
const CHUNK_BYTES: usize = CHUNK_SEED_BYTES << CHUNK_DOUBLINGS;

/// How many bytes one mebibyte is — the unit a memory limit is stated in.
const BYTES_PER_MIB: usize = 1024 * 1024;

/// How often a poll re-reads the emulator.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up. Generous: every poll ends the instant its
/// own condition holds, and this has to hold on a loaded machine as well as an idle one.
const POLL_BUDGET: Duration = Duration::from_secs(120);

/// A router carrying Amazon ECS and CloudWatch Logs over ONE shared engine — the two services
/// a container's output travels through.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// The program one row's container runs: it writes the start marker, builds one chunk by
/// doubling a literal, holds `copies` of that chunk in variables of its own, and writes the
/// completion marker only if it got that far.
///
/// Every value it holds is its own — no file, no network and no other process is involved — so
/// what the container occupies is `(copies + 1)` chunks, which is what its limit is weighed
/// against.
fn program(copies: usize) -> Vec<String> {
    let mut lines = vec![
        format!("echo {ALLOC_START}"),
        format!("c={}", "x".repeat(CHUNK_SEED_BYTES)),
        "i=0".to_string(),
        format!("while [ $i -lt {CHUNK_DOUBLINGS} ]; do c=\"$c$c\"; i=$((i+1)); done"),
    ];
    for copy in 0..copies {
        lines.push(format!("v{copy}=$c"));
    }
    lines.push(format!("echo {ALLOC_DONE}"));
    vec!["sh".to_string(), "-c".to_string(), lines.join("; ")]
}

/// How many bytes of its own values the program of `copies` copies holds at its peak.
fn held_bytes(copies: usize) -> usize {
    CHUNK_BYTES.saturating_mul(copies.saturating_add(1))
}

/// One row: where the limit is stated, what it is, and how much the program holds.
struct Row {
    case: &'static str,
    /// The memory limit, in MiB.
    limit_mib: usize,
    /// Whether the TASK states that limit rather than the container, which is what a container
    /// stating none of its own is held to.
    on_the_task: bool,
    /// How many copies of the chunk the program holds beside the chunk itself.
    copies: usize,
}

/// The rows of the table.
const ROWS: &[Row] = &[
    Row {
        case: "a program that holds more than its container's own memory",
        limit_mib: 4,
        on_the_task: false,
        copies: 64,
    },
    Row {
        // The SAME program as the row above, under a limit that admits what it holds.
        case: "the same program under a limit twice as high",
        limit_mib: 8,
        on_the_task: false,
        copies: 64,
    },
    Row {
        case: "a program that holds well inside the same small limit",
        limit_mib: 4,
        on_the_task: false,
        copies: 32,
    },
    Row {
        case: "a container stating no memory of its own, held to the task's",
        limit_mib: 4,
        on_the_task: true,
        copies: 64,
    },
];

impl Row {
    /// Whether this row's program asks for more memory than its container is given, which is
    /// what the runtime kills the container for.
    fn killed(&self) -> bool {
        held_bytes(self.copies) > self.limit_mib.saturating_mul(BYTES_PER_MIB)
    }
}

/// Register one revision of `family` for `row` and answer its ARN.
async fn register(router: &MultiServiceRouter, family: &str, row: &Row) -> String {
    let mut container = json!({
        "name": CONTAINER,
        "image": IMAGE,
        "command": program(row.copies),
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": LOG_GROUP,
                "awslogs-region": SIGNED_REGION,
                "awslogs-stream-prefix": STREAM_PREFIX,
            },
        },
    });
    let mut request = json!({ "family": family, "containerDefinitions": [] });
    if row.on_the_task {
        request["cpu"] = json!("256");
        request["memory"] = json!(row.limit_mib.to_string());
    } else {
        container["memory"] = json!(row.limit_mib);
    }
    request["containerDefinitions"] = json!([container]);
    let body = ok(router, "RegisterTaskDefinition", request, row.case).await;
    string(&body["taskDefinition"], "taskDefinitionArn", row.case)
}

/// Launch one standalone task of `task_definition` in `cluster` and answer its ARN.
async fn run_task(
    router: &MultiServiceRouter,
    cluster: &str,
    task_definition: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": task_definition,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected RunTask to place one task, got {body}"
    );
    string(&tasks[0], "taskArn", case)
}

/// Read the task again until it has STOPPED — waiting for the CONDITION rather than for a
/// span, since the run of a container's program takes as long as the program takes.
async fn stopped_task(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let body = ok(
            router,
            "DescribeTasks",
            json!({ "cluster": cluster, "tasks": [task_arn] }),
            case,
        )
        .await;
        let tasks = list(&body, "tasks", case);
        let task = tasks
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected one described task, got {body}"));
        if task["lastStatus"].as_str() == Some("STOPPED") {
            return task;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the task never stopped within {POLL_BUDGET:?}, its last status was \
             {:?}",
            task["lastStatus"].as_str()
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The stream the `awslogs` driver delivers one container of one task to:
/// `<prefix>/<container name>/<task id>`, the naming that driver's own
/// `awslogs-stream-prefix` documentation states.
fn stream_of(task_arn: &str) -> String {
    let task_id = task_arn.rsplit('/').next().unwrap_or_default();
    format!("{STREAM_PREFIX}/{CONTAINER}/{task_id}")
}

/// The messages CloudWatch Logs holds in `stream`, oldest first. A stream it holds nothing for
/// reads as no messages.
async fn delivered(router: &MultiServiceRouter, stream: &str) -> Vec<String> {
    let (status, body) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": LOG_GROUP,
            "logStreamName": stream,
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a container that holds more memory than its definition gives it is KILLED — it exits
/// with the status a killed process carries, reports the kill as its reason, stops the task it
/// is essential to, and leaves in its log stream exactly what it wrote before the kill — while
/// one that stays inside that limit runs to its end and exits with its program's own status.
#[tokio::test]
async fn a_container_that_goes_past_its_declared_memory_is_killed_part_way_through() {
    let router = emulator();
    let cluster = "memory-limits";
    cluster_with_capacity(&router, cluster, "memory limits").await;
    let (status, body) = logs(
        &router,
        "CreateLogGroup",
        json!({ "logGroupName": LOG_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateLogGroup to answer 200, got {status} with {body}"
    );

    // Every row's task is launched FIRST and read afterwards, so the runs happen together in
    // the one emulator rather than one after another.
    let mut launched: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let family = format!("memory-limit-{index}");
        let definition = register(&router, &family, row).await;
        launched.push((row, run_task(&router, cluster, &definition, row.case).await));
    }

    for (row, task_arn) in &launched {
        let case = row.case;
        let task = stopped_task(&router, cluster, task_arn, case).await;
        let containers = list(&task, "containers", case);
        let container = containers
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected one container, got {task}"));

        let expected_exit = if row.killed() { KILLED_EXIT_CODE } else { 0 };
        assert_eq!(
            container["exitCode"].as_i64(),
            Some(expected_exit),
            "case {case:?}: a container holding {} bytes under a {} MiB limit must exit \
             {expected_exit}, got {container}",
            held_bytes(row.copies),
            row.limit_mib
        );
        if row.killed() {
            assert_eq!(
                container["reason"].as_str(),
                Some(OUT_OF_MEMORY_REASON),
                "case {case:?}: a killed container reports the kill, got {container}"
            );
        } else {
            assert!(
                container["reason"].is_null(),
                "case {case:?}: a container that ran to its end reports no additional detail, \
                 got {container}"
            );
        }
        // The container ENDED either way, so the task it is essential to is stopped with it.
        assert_eq!(
            task["stopCode"].as_str(),
            Some(ESSENTIAL_CONTAINER_EXITED),
            "case {case:?}: expected the task to stop with {ESSENTIAL_CONTAINER_EXITED}, got \
             {task}"
        );

        let messages = delivered(&router, &stream_of(task_arn)).await;
        assert!(
            messages.iter().any(|message| message == ALLOC_START),
            "case {case:?}: every container reaches the start marker, got {messages:?}"
        );
        let completed = messages.iter().any(|message| message == ALLOC_DONE);
        assert_eq!(
            completed,
            !row.killed(),
            "case {case:?}: a killed container never writes the completion marker and one that \
             ran to its end always does, got {messages:?}"
        );
    }
}
