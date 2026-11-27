// SPDX-License-Identifier: CC0-1.0

//! The POSITIONAL PARAMETERS a container's `sh -c` program is invoked with, driven end to end
//! over the wire.
//!
//! RULE under test: **the words a container's argument vector carries AFTER a `sh -c` program
//! text are that program's positional parameters — POSIX (`sh`): with `-c`, "command_name …
//! shall be assigned to special parameter 0" and the operands after it "shall be assigned to
//! the positional parameters" from `1` on, while an invocation with "no operands after
//! command_string" leaves "the name that the shell was invoked by" as `$0`.** So `$0` is the
//! first of those words, `$1`, `$2`, … are the ones after it, `$#` counts them without the
//! name, `$@` and `$*` are those operands, and a parameter the invocation gave nothing at
//! expands to nothing, the way an unset parameter does. The container's program is
//! `ContainerDefinition$entryPoint` followed by `$command`, so the words may be split across
//! the two members; what the program writes is delivered to its `awslogs` stream
//! (`LogConfiguration$logDriver` `awslogs` "sends container logs to Amazon CloudWatch Logs")
//! and `Container$exitCode` is "the exit code returned from the container".
//!
//! Why it matters: a vector that passes its values as operands (`sh -c 'exit "$0"' 3`) is the
//! documented way a parameterised program is handed its arguments, so a session that drops them
//! runs a DIFFERENT program from the one the vector named — with the shell's own name, or
//! nothing at all, where the caller's value belonged.
//!
//! The rows differ along what the rule generalises over: WHICH parameter the program reads
//! (`$0`, `$1`, a position nothing was given, `$#`, `$@`, `$*`), HOW MANY operands the
//! invocation carries (none, a name only, a name and several arguments), WHERE those words
//! stand (all in `command`, split across `entryPoint` and `command`), and what the program
//! does with the value (writes it, or exits with it).
//!
//! CloudWatch Logs is mounted BESIDE Amazon ECS on the SAME engine, so what is asserted is what
//! that service holds for the stream, read back through its own `GetLogEvents`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, logs, ok, present_count, seed_awsvpc_subnets, string,
    SIGNED_REGION,
};

/// The log group every container below delivers to.
const LOG_GROUP: &str = "/emulator/ecs/positional-parameters";

/// The `awslogs-stream-prefix` the task definitions below configure.
const STREAM_PREFIX: &str = "operands";

/// The one container of every task definition below.
const CONTAINER: &str = "app";

/// The image the containers start from: one this emulator holds no image of, so what each row
/// observes is its container's own program.
const IMAGE: &str = "public.ecr.aws/amazonlinux/amazonlinux:2023";

/// How often a poll looks again, and how long it keeps looking: far past the clock-owed
/// transitions a task takes to start and to stop, so a loaded machine delays a row instead of
/// breaking it. Only the loop's CONDITION ends a wait.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// Amazon ECS and CloudWatch Logs over ONE shared engine, holding the subnet an `awsvpc`
/// placement of this suite names.
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

/// The messages CloudWatch Logs holds in `stream`, oldest first.
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

/// The stream the `awslogs` driver delivers one container of one task to:
/// `<prefix>/<container name>/<task id>`, spelled out here rather than imported.
fn stream_of(task_arn: &str) -> String {
    let task_id = task_arn.rsplit('/').next().unwrap_or_default();
    format!("{STREAM_PREFIX}/{CONTAINER}/{task_id}")
}

/// What one row's container is launched with, and what its program must then have done.
struct Row {
    case: &'static str,
    /// `ContainerDefinition$entryPoint`, empty for a definition that states none.
    entry_point: &'static [&'static str],
    /// `ContainerDefinition$command`.
    command: &'static [&'static str],
    /// The lines its `awslogs` stream must hold, in order.
    delivered: &'static [&'static str],
    /// The status its program ends with, which is the container's `exitCode`.
    exit: i64,
}

const ROWS: &[Row] = &[
    Row {
        case: "$0 is the name the invocation gave the program",
        entry_point: &[],
        command: &["sh", "-c", "echo name=$0", "runner"],
        delivered: &["name=runner"],
        exit: 0,
    },
    Row {
        // The name such a shell is invoked under is `sh`, spelled out here rather than
        // imported so a wrong constant in the implementation cannot make the row pass.
        case: "an invocation with no operands leaves the shell's own name as $0",
        entry_point: &[],
        command: &["sh", "-c", "echo name=$0"],
        delivered: &["name=sh"],
        exit: 0,
    },
    Row {
        case: "$1 and $2 are the arguments after that name",
        entry_point: &[],
        command: &["sh", "-c", "echo args=$1,$2", "runner", "alpha", "beta"],
        delivered: &["args=alpha,beta"],
        exit: 0,
    },
    Row {
        case: "a position the invocation gave nothing at expands to nothing",
        entry_point: &[],
        command: &["sh", "-c", "echo third=$3", "runner", "alpha"],
        delivered: &["third="],
        exit: 0,
    },
    Row {
        case: "$# counts the arguments and not the name",
        entry_point: &[],
        command: &["sh", "-c", "echo count=$#", "runner", "alpha", "beta"],
        delivered: &["count=2"],
        exit: 0,
    },
    Row {
        case: "a program given a name only has no arguments to count",
        entry_point: &[],
        command: &["sh", "-c", "echo count=$#", "runner"],
        delivered: &["count=0"],
        exit: 0,
    },
    Row {
        case: "$@ and $* are those arguments",
        entry_point: &[],
        command: &[
            "sh",
            "-c",
            "echo all=$@; echo star=$*",
            "runner",
            "alpha",
            "beta",
        ],
        delivered: &["all=alpha beta", "star=alpha beta"],
        exit: 0,
    },
    Row {
        case: "the words may be split across entryPoint and command",
        entry_point: &["sh", "-c", "echo name=$0 first=$1"],
        command: &["runner", "alpha"],
        delivered: &["name=runner first=alpha"],
        exit: 0,
    },
    Row {
        case: "the status a program takes from the name it was invoked under",
        entry_point: &[],
        command: &["sh", "-c", "exit \"$0\"", "3"],
        delivered: &[],
        exit: 3,
    },
    Row {
        case: "the status a program takes from one of its arguments",
        entry_point: &[],
        command: &["sh", "-c", "exit \"$1\"", "runner", "4"],
        delivered: &[],
        exit: 4,
    },
    Row {
        case: "a program that reports its operands and then exits with one of them",
        entry_point: &[],
        command: &[
            "sh",
            "-c",
            "echo ran=$0 with=$1; exit \"$1\"",
            "runner",
            "5",
        ],
        delivered: &["ran=runner with=5"],
        exit: 5,
    },
];

/// RULE: the words after a container's `sh -c` program text are that program's positional
/// parameters — `$0` the name it was invoked under, `$1` onwards its arguments, `$#` their
/// count and `$@`/`$*` the arguments themselves — so what the container writes and the status
/// it exits with are the ones that program computes from them.
#[tokio::test]
async fn a_containers_program_reads_the_operands_its_vector_gave_it() {
    let router = emulator();
    let cluster = "positional-parameters";
    cluster_with_capacity(&router, cluster, "positional parameters").await;
    let (status, body) = logs(
        &router,
        "CreateLogGroup",
        json!({ "logGroupName": LOG_GROUP }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateLogGroup answered {status} with {body}"
    );

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let family = format!("positional-parameters-{index}");
        let mut container = json!({
            "name": CONTAINER,
            "image": IMAGE,
            "memory": 128,
            "command": row.command,
            "logConfiguration": {
                "logDriver": "awslogs",
                "options": {
                    "awslogs-group": LOG_GROUP,
                    "awslogs-region": SIGNED_REGION,
                    "awslogs-stream-prefix": STREAM_PREFIX,
                },
            },
        });
        if !row.entry_point.is_empty() {
            container["entryPoint"] = json!(row.entry_point);
        }
        let definition = ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": &family,
                "containerDefinitions": [container],
            }),
            case,
        )
        .await;
        let definition_arn = string(&definition["taskDefinition"], "taskDefinitionArn", case);
        let launched = ok(
            &router,
            "RunTask",
            json!({
                "cluster": cluster,
                "taskDefinition": definition_arn,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
        let tasks = list(&launched, "tasks", case);
        assert_eq!(
            tasks.len(),
            1,
            "case {case:?}: expected RunTask to place one task, got {launched}"
        );
        let task_arn = string(&tasks[0], "taskArn", case);
        let stream = stream_of(&task_arn);

        // The container's program ends on its own: a wait for the CONDITION — an exit status
        // being reported — and never for a span of the clock.
        let deadline = Instant::now() + POLL_BUDGET;
        let exit_code = loop {
            let described = ok(
                &router,
                "DescribeTasks",
                json!({ "cluster": cluster, "tasks": [&task_arn] }),
                case,
            )
            .await;
            let described_tasks = list(&described, "tasks", case);
            let containers = list(&described_tasks[0], "containers", case);
            if let Some(code) = present_count(&containers[0], "exitCode", case) {
                break code;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the container never reported an exit status, last {described}"
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            exit_code, row.exit,
            "case {case:?}: expected the program to exit {}, got {exit_code}",
            row.exit
        );

        // What it WROTE, as the log service holds it: a wait for the CONDITION, because the
        // delivery of the last line and the exit status are two writes.
        let expected: Vec<String> = row
            .delivered
            .iter()
            .map(|line| (*line).to_string())
            .collect();
        let deadline = Instant::now() + POLL_BUDGET;
        loop {
            let held = delivered(&router, &stream).await;
            if held == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {stream} to hold {expected:?}, got {held:?}"
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}
