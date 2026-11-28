// SPDX-License-Identifier: CC0-1.0

//! Which launch types a registered revision reports as validated against, driven end to
//! end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `TaskDefinition$compatibilities` ("Amazon ECS validates the task
//! definition parameters with those supported by the launch type") reports a launch type
//! when the stored revision's parameters meet that launch type's documented requirements,
//! whether or not the
//! registration asked to be validated against it in its
//! `requiresCompatibilities` ("The task launch type that Amazon ECS validates the task
//! definition against"), which is reported separately and unchanged. The launch types the
//! derived list is drawn from are every member of the `Compatibility` enum, reported in the
//! order a recorded ECS projection carries them — `EXTERNAL`, `EC2`, `FARGATE`,
//! `MANAGED_INSTANCES`. The one exception to "whether or not the registration asked" is the
//! managed-instances capacity no placement may name outright, which a registration that
//! pinned its launch types without naming it was not validated against
//! ([`crate::managed_instances_compatibility`] is that half of the rule).
//!
//! Which of those launch types a PLACEMENT may name is the narrower question: a
//! `launchType` names the infrastructure outright, and managed-instances capacity is not
//! reachable that way (`CreateService$launchType`: "If you want to use Amazon ECS Managed
//! Instances, you must use the capacityProviderStrategy request parameter and omit the
//! launchType request parameter"), so the last suite drives `RunTask` through the gate
//! the same derived list feeds.
//!
//! Every case goes through the HTTP front door, so a list that is not derived from the
//! stored revision cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cluster_with_capacity, list, ok, refusal, service, string,
};
use crate::task_lifecycle::container;

/// The image every container here starts from; irrelevant to the rule, so one value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// A legal Fargate task size (`RegisterTaskDefinition$memory`'s documented list of
/// cpu/memory pairs), so a row that declares a task size declares one Fargate accepts.
const TASK_CPU: &str = "512";
const TASK_MEMORY: &str = "1024";

/// One row of the derivation table.
struct Row {
    case: &'static str,
    /// The `networkMode` the registration declares, or `None` for the documented default
    /// (`bridge`).
    network_mode: Option<&'static str>,
    /// Whether the registration declares task-level `cpu` and `memory`.
    task_size: bool,
    /// What the registration asks to be validated against, `requiresCompatibilities`.
    requires: &'static [&'static str],
    /// The launch types the revision must report, in the order ECS reports them.
    expected: &'static [&'static str],
}

/// Register one row's revision and return the body `RegisterTaskDefinition` answered.
async fn register(router: &MultiServiceRouter, family: &str, row: &Row, case: &str) -> Value {
    let mut request = json!({
        "family": family,
        "containerDefinitions": [container("app", IMAGE)],
    });
    if let Some(mode) = row.network_mode {
        request["networkMode"] = json!(mode);
    }
    if row.task_size {
        request["cpu"] = json!(TASK_CPU);
        request["memory"] = json!(TASK_MEMORY);
    }
    if !row.requires.is_empty() {
        request["requiresCompatibilities"] = json!(row.requires);
    }
    ok(router, "RegisterTaskDefinition", request, case).await
}

/// The `compatibilities` of one projection, as a list of strings.
fn compatibilities(definition: &Value, case: &str) -> Vec<String> {
    list(definition, "compatibilities", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string launch type, got {value}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn compatibilities_report_only_the_launch_types_the_revision_supports() {
    let router = service();

    // The rows differ along everything the rule generalises over: the network mode
    // (`NetworkMode`: bridge, host, awsvpc, none, plus the documented default), whether
    // task-level cpu and memory are declared, and what the registration asked to be
    // validated against — including asking for a launch type the parameters rule out.
    let rows: [Row; 10] = [
        Row {
            case: "default network mode, asking for nothing",
            network_mode: None,
            task_size: false,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge asking for EC2",
            network_mode: Some("bridge"),
            task_size: false,
            requires: &["EC2"],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge asking for managed instances",
            network_mode: Some("bridge"),
            task_size: false,
            requires: &["EC2", "MANAGED_INSTANCES"],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "host with a task size",
            network_mode: Some("host"),
            task_size: true,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "none",
            network_mode: Some("none"),
            task_size: false,
            requires: &[],
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "awsvpc without a task size, asking for nothing",
            network_mode: Some("awsvpc"),
            task_size: false,
            requires: &[],
            expected: &["EC2", "MANAGED_INSTANCES"],
        },
        Row {
            case: "awsvpc with a task size, asking for nothing",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &[],
            expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
        },
        Row {
            case: "awsvpc with a task size, asking for Fargate",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &["FARGATE"],
            expected: &["EC2", "FARGATE"],
        },
        Row {
            case: "awsvpc without a task size, asking for managed instances",
            network_mode: Some("awsvpc"),
            task_size: false,
            requires: &["MANAGED_INSTANCES"],
            expected: &["EC2", "MANAGED_INSTANCES"],
        },
        Row {
            case: "awsvpc with a task size, asking for every launch type awsvpc allows",
            network_mode: Some("awsvpc"),
            task_size: true,
            requires: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
            expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let family = format!("compatfam{index}");
        let registered = register(&router, &family, row, case).await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;

        // Both projections of the SAME stored revision, so a list rendered off the
        // request rather than off the record shows up as a difference between them.
        for (source, definition) in [
            ("RegisterTaskDefinition", &registered["taskDefinition"]),
            ("DescribeTaskDefinition", &described["taskDefinition"]),
        ] {
            let actual = compatibilities(definition, case);
            assert_eq!(
                actual, row.expected,
                "case {case:?}: {source} expected compatibilities {:?}, got {actual:?} in \
                 {definition}",
                row.expected
            );
            // What the REQUEST asked for is reported separately and unchanged: the
            // derived list is not written back onto it, nor it onto the derived list.
            let requested: Vec<String> = list(definition, "requiresCompatibilities", case)
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                requested, row.requires,
                "case {case:?}: {source} expected requiresCompatibilities {:?}, got \
                 {requested:?} in {definition}",
                row.requires
            );
        }
    }
}

#[tokio::test]
async fn the_derived_list_ignores_the_shape_of_the_container_list() {
    let router = service();

    // One Fargate-shaped revision per container list, each registered twice: once asking
    // to be validated against Fargate alone, once asking for managed instances as well.
    // The rows differ along the container COUNT and the members the containers carry — a
    // log driver that delivers to the task's log router, the router that serves it
    // (`FirelensConfiguration`), a task volume with a mount point — none of which is a
    // launch-type requirement, so the list a row derives turns only on what the
    // registration pinned, never on the shape of its container list.
    let app_image = IMAGE;
    let rows: [(&str, Value, Value); 4] = [
        (
            "one plain container",
            json!([container("app", app_image)]),
            json!([]),
        ),
        (
            "an awsfirelens app beside a fluent-bit router",
            json!([
                {
                    "name": "app",
                    "image": app_image,
                    "memory": 128,
                    "logConfiguration": { "logDriver": "awsfirelens" },
                },
                {
                    "name": "log-router",
                    "image": "public.ecr.aws/aws-observability/aws-for-fluent-bit:stable",
                    "memory": 128,
                    "firelensConfiguration": { "type": "fluentbit" },
                },
            ]),
            json!([]),
        ),
        (
            "two containers sharing a task volume",
            json!([
                {
                    "name": "writer",
                    "image": app_image,
                    "memory": 128,
                    "mountPoints": [{ "sourceVolume": "scratch", "containerPath": "/scratch" }],
                },
                {
                    "name": "reader",
                    "image": app_image,
                    "memory": 128,
                    "mountPoints": [
                        { "sourceVolume": "scratch", "containerPath": "/scratch", "readOnly": true },
                    ],
                },
            ]),
            json!([{ "name": "scratch" }]),
        ),
        (
            "three containers, one of them not essential",
            json!([
                container("app", app_image),
                container("sidecar", app_image),
                { "name": "optional", "image": app_image, "memory": 128, "essential": false },
            ]),
            json!([]),
        ),
    ];

    // (what the registration asks to be validated against, what the revision must report).
    let asks: [(&[&str], &[&str]); 2] = [
        (&["FARGATE"], &["EC2", "FARGATE"]),
        (
            &["FARGATE", "MANAGED_INSTANCES"],
            &["EC2", "FARGATE", "MANAGED_INSTANCES"],
        ),
    ];

    for (index, (shape, containers, volumes)) in rows.iter().enumerate() {
        for (ask_index, (requires, expected)) in asks.iter().enumerate() {
            let case = format!("{shape} asking for {requires:?}");
            let case = case.as_str();
            let family = format!("shapefam{index}x{ask_index}");
            let mut request = json!({
                "family": family,
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "containerDefinitions": containers,
                "requiresCompatibilities": requires,
            });
            if !volumes.as_array().is_some_and(|list| list.is_empty()) {
                request["volumes"] = volumes.clone();
            }
            let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
            let described = ok(
                &router,
                "DescribeTaskDefinition",
                json!({ "taskDefinition": family }),
                case,
            )
            .await;
            for (source, definition) in [
                ("RegisterTaskDefinition", &registered["taskDefinition"]),
                ("DescribeTaskDefinition", &described["taskDefinition"]),
            ] {
                let actual = compatibilities(definition, case);
                assert_eq!(
                    actual, *expected,
                    "case {case:?}: {source} expected compatibilities {expected:?}, got \
                     {actual:?} in {definition}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_placement_may_name_only_a_launch_type_the_revision_reports() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, "gate", case).await;

    // Two revisions of the SAME shape — `awsvpc` at a legal Fargate task size — that
    // differ only in whether the registration asked to be validated against
    // managed-instances capacity: BOTH report that launch type among the ones they were
    // validated against, and neither may be placed by NAMING it, which is the narrower
    // question the gate puts (`CreateService$launchType`).
    let plain = Row {
        case: "asking for nothing",
        network_mode: Some("awsvpc"),
        task_size: true,
        requires: &[],
        expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
    };
    let validated = Row {
        case: "asking for managed instances",
        network_mode: Some("awsvpc"),
        task_size: true,
        requires: &["MANAGED_INSTANCES"],
        expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
    };
    register(&router, "gateplain", &plain, case).await;
    register(&router, "gatevalidated", &validated, case).await;

    // (case, the family, the launch type asked for, whether the placement is accepted).
    let rows: [(&str, &str, &str, bool); 6] = [
        ("plain on EC2", "gateplain", "EC2", true),
        ("plain on FARGATE", "gateplain", "FARGATE", true),
        (
            "plain on MANAGED_INSTANCES",
            "gateplain",
            "MANAGED_INSTANCES",
            false,
        ),
        ("plain on EXTERNAL", "gateplain", "EXTERNAL", false),
        (
            "validated on MANAGED_INSTANCES",
            "gatevalidated",
            "MANAGED_INSTANCES",
            false,
        ),
        ("validated on FARGATE", "gatevalidated", "FARGATE", true),
    ];

    for (case, family, launch_type, accepted) in rows {
        let request = json!({
            "cluster": "gate",
            "taskDefinition": family,
            "launchType": launch_type,
            "networkConfiguration": awsvpc_network_configuration(),
        });
        if accepted {
            let run = ok(&router, "RunTask", request, case).await;
            let tasks = list(&run, "tasks", case);
            assert_eq!(
                tasks.len(),
                1,
                "case {case:?}: a launch type the revision reports places the task, got \
                 {run}"
            );
            let placed = string(&tasks[0], "launchType", case);
            assert_eq!(
                placed, launch_type,
                "case {case:?}: expected the task to record launchType {launch_type}, got \
                 {placed:?}"
            );
            continue;
        }
        let (code, message) = refusal(&router, "RunTask", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message,
            format!("Task definition does not support launch_type {launch_type}."),
            "case {case:?}: the refusal must name the launch type asked for, got {message:?}"
        );
    }
}
