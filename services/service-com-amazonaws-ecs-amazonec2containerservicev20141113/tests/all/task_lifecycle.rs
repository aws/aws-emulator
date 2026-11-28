// SPDX-License-Identifier: CC0-1.0

//! The ECS cluster / task-definition / task lifecycle, driven end to end over HTTP.
//!
//! Every case here goes through the shared wire harness in [`crate::ecs_wire`], the same
//! front door a client uses, so nothing reaches into the state struct and a response
//! that is not computed from persisted state cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, create_cluster, list, ok, poll_until, refused, register_capacity,
    service, string, with_members, ACCOUNT, SIGNED_REGION,
};

/// Long enough for one clock-owed task transition (`state::TRANSITION_SPAN` is 500 ms)
/// to fall due, with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(900);

/// A minimal, valid container definition.
pub(crate) fn container(name: &str, image: &str) -> Value {
    json!({ "name": name, "image": image, "memory": 128 })
}

/// Register one revision of `family` with a single container and return
/// `(taskDefinitionArn, revision)`.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    container_name: &str,
    case: &str,
) -> (String, i64) {
    let body = ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container(container_name, "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
    let definition = &body["taskDefinition"];
    (
        string(definition, "taskDefinitionArn", case),
        count(definition, "revision", case),
    )
}

#[tokio::test]
async fn cluster_create_read_and_delete_round_trip() {
    let router = service();

    // The rule generalises over the cluster NAME, so the table spans the forms the
    // `clusterName` doc allows ("Up to 255 letters (uppercase and lowercase), numbers,
    // underscores, and hyphens") plus the documented default when the member is absent.
    let long_name = "c".repeat(255);
    let cases: [(&str, Option<&str>, &str); 5] = [
        ("absent name defaults", None, "default"),
        ("plain", Some("demo"), "demo"),
        ("mixed case and digits", Some("Prod2Cluster"), "Prod2Cluster"),
        ("underscores and hyphens", Some("a_b-c"), "a_b-c"),
        ("255 characters", Some(long_name.as_str()), long_name.as_str()),
    ];

    for (case, requested, expected_name) in cases {
        let mut request = json!({
            "tags": [{ "key": "env", "value": "test" }],
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        });
        if let Some(name) = requested {
            request["clusterName"] = json!(name);
        }

        let created = ok(&router, "CreateCluster", request, case).await;
        let cluster = &created["cluster"];
        assert_eq!(
            string(cluster, "clusterName", case),
            expected_name,
            "case {case:?}: expected clusterName {expected_name:?}, got {cluster}"
        );
        assert_eq!(
            string(cluster, "clusterArn", case),
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{expected_name}"),
            "case {case:?}: unexpected clusterArn in {cluster}"
        );
        assert_eq!(
            string(cluster, "status", case),
            "ACTIVE",
            "case {case:?}: expected a new cluster to be ACTIVE, got {cluster}"
        );
        for counter in [
            "registeredContainerInstancesCount",
            "runningTasksCount",
            "pendingTasksCount",
            "activeServicesCount",
        ] {
            assert_eq!(
                count(cluster, counter, case),
                0,
                "case {case:?}: expected {counter} to be 0 on a new cluster, got {cluster}"
            );
        }

        // Read back through DescribeClusters: the stored record, not the request.
        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [expected_name], "include": ["TAGS", "SETTINGS"] }),
            case,
        )
        .await;
        assert!(
            list(&described, "failures", case).is_empty(),
            "case {case:?}: expected no failures describing a stored cluster, got {described}"
        );
        let read = &list(&described, "clusters", case)[0];
        assert_eq!(
            string(read, "clusterArn", case),
            string(cluster, "clusterArn", case),
            "case {case:?}: describe returned a different ARN than create: {read}"
        );
        assert_eq!(
            list(read, "tags", case),
            vec![json!({ "key": "env", "value": "test" })],
            "case {case:?}: expected the stored tags back, got {read}"
        );
        assert_eq!(
            list(read, "settings", case),
            vec![json!({ "name": "containerInsights", "value": "enabled" })],
            "case {case:?}: expected the stored settings back, got {read}"
        );

        // `include` gates what the optional fields CARRY: "If this field is omitted,
        // this information isn't included." The list-typed ones are still there, empty,
        // so a client cannot mistake "not asked for" for "none stored".
        let narrow = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [expected_name] }),
            case,
        )
        .await;
        let narrow_cluster = &list(&narrow, "clusters", case)[0];
        for gated in ["tags", "settings", "statistics"] {
            assert_eq!(
                list(narrow_cluster, gated, case),
                Vec::<Value>::new(),
                "case {case:?}: expected an empty {gated} without include, got \
                 {narrow_cluster}"
            );
        }
        assert!(
            narrow_cluster["configuration"].is_null(),
            "case {case:?}: expected configuration to be absent without include, got \
             {narrow_cluster}"
        );

        // A second create of the same name is the same cluster, not a duplicate.
        let again = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": expected_name }),
            case,
        )
        .await;
        assert_eq!(
            string(&again["cluster"], "clusterArn", case),
            string(cluster, "clusterArn", case),
            "case {case:?}: a repeated create must return the stored cluster, got {again}"
        );
        let after_repeat = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [expected_name] }),
            case,
        )
        .await;
        assert_eq!(
            list(&after_repeat, "clusters", case).len(),
            1,
            "case {case:?}: a repeated create must not add a second entry, got \
             {after_repeat}"
        );

        // Delete reports the cluster INACTIVE and leaves that tombstone in the store:
        // "Clusters with an INACTIVE status may remain discoverable in your account for a
        // period of time."
        let deleted = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": expected_name }),
            case,
        )
        .await;
        assert_eq!(
            string(&deleted["cluster"], "status", case),
            "INACTIVE",
            "case {case:?}: expected a deleted cluster to be INACTIVE, got {deleted}"
        );
        let after_delete = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [expected_name] }),
            case,
        )
        .await;
        assert!(
            list(&after_delete, "failures", case).is_empty(),
            "case {case:?}: a deleted cluster is described, not MISSING, got \
             {after_delete}"
        );
        let tombstone = &list(&after_delete, "clusters", case)[0];
        assert_eq!(
            string(tombstone, "status", case),
            "INACTIVE",
            "case {case:?}: expected the tombstone to read INACTIVE, got {after_delete}"
        );
        assert_eq!(
            string(tombstone, "clusterArn", case),
            string(cluster, "clusterArn", case),
            "case {case:?}: expected the tombstone to keep its ARN, got {after_delete}"
        );
        // `DeleteCluster` is `@idempotent`: deleting the tombstone is the same delete.
        let repeated = ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": expected_name }),
            case,
        )
        .await;
        assert_eq!(
            string(&repeated["cluster"], "status", case),
            "INACTIVE",
            "case {case:?}: a repeated delete reports the same tombstone, got {repeated}"
        );
        // The name is free again: a create over a tombstone is a new, ACTIVE cluster.
        let recreated = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": expected_name }),
            case,
        )
        .await;
        assert_eq!(
            string(&recreated["cluster"], "status", case),
            "ACTIVE",
            "case {case:?}: expected a create over a tombstone to be ACTIVE, got \
             {recreated}"
        );
        ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": expected_name }),
            case,
        )
        .await;
        // A name that names nothing at all is still refused.
        refused(
            &router,
            "DeleteCluster",
            json!({ "cluster": "a-cluster-that-was-never-created" }),
            "ClusterNotFoundException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn cluster_requests_outside_the_documented_rules_are_refused() {
    let router = service();

    // The name rule is "Up to 255 letters (uppercase and lowercase), numbers,
    // underscores, and hyphens are allowed" — tested on each bound and one past it.
    let too_long = "c".repeat(256);
    let name_cases: [(&str, &str); 5] = [
        ("empty", ""),
        ("slash", "demo/one"),
        ("space", "demo one"),
        ("dot", "demo.one"),
        ("256 characters", too_long.as_str()),
    ];
    for (case, name) in name_cases {
        refused(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // "The supported values are enhanced, enabled, and disabled."
    for value in ["enhanced", "enabled", "disabled"] {
        let case = format!("setting {value}");
        let body = ok(
            &router,
            "CreateCluster",
            json!({
                "clusterName": format!("settings-{value}"),
                "settings": [{ "name": "containerInsights", "value": value }],
            }),
            &case,
        )
        .await;
        assert_eq!(
            string(&body["cluster"], "status", &case),
            "ACTIVE",
            "case {case:?}: expected the documented setting value to be accepted, got {body}"
        );
    }
    for value in ["", "ENABLED", "on"] {
        refused(
            &router,
            "CreateCluster",
            json!({
                "clusterName": "settings-bad",
                "settings": [{ "name": "containerInsights", "value": value }],
            }),
            "InvalidParameterException",
            &format!("setting {value:?}"),
        )
        .await;
    }
}

#[tokio::test]
async fn task_definition_revisions_increment_per_family() {
    let router = service();

    // The rule generalises over the FAMILY, so several families are interleaved: each
    // one's revisions must count from 1 independently.
    let families = ["web", "worker-2", "batch_job"];
    for round in 1..=3_i64 {
        for family in families {
            let case = format!("{family} round {round}");
            let body = ok(
                &router,
                "RegisterTaskDefinition",
                json!({
                    "family": family,
                    "containerDefinitions": [container("app", "nginx")],
                }),
                &case,
            )
            .await;
            let definition = &body["taskDefinition"];
            assert_eq!(
                count(definition, "revision", &case),
                round,
                "case {case:?}: expected revision {round}, got {definition}"
            );
            assert_eq!(
                string(definition, "taskDefinitionArn", &case),
                format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{round}"
                ),
                "case {case:?}: unexpected taskDefinitionArn in {definition}"
            );
            assert_eq!(
                string(definition, "status", &case),
                "ACTIVE",
                "case {case:?}: expected a registered revision to be ACTIVE, got {definition}"
            );
            assert!(
                definition["registeredAt"].is_number(),
                "case {case:?}: expected a registeredAt timestamp, got {definition}"
            );
            assert_eq!(
                list(definition, "containerDefinitions", &case).len(),
                1,
                "case {case:?}: expected the stored container definitions back, got \
                 {definition}"
            );
        }
    }

    // Reading back: a bare family resolves to the latest revision, `family:revision`
    // and the full ARN to exactly that one.
    for family in families {
        let latest = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            family,
        )
        .await;
        assert_eq!(
            count(&latest["taskDefinition"], "revision", family),
            3,
            "case {family:?}: a bare family must resolve to the latest revision, got {latest}"
        );
        for revision in 1..=3_i64 {
            let case = format!("{family}:{revision}");
            for reference in [
                format!("{family}:{revision}"),
                format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}"),
            ] {
                let body = ok(
                    &router,
                    "DescribeTaskDefinition",
                    json!({ "taskDefinition": reference }),
                    &case,
                )
                .await;
                assert_eq!(
                    count(&body["taskDefinition"], "revision", &case),
                    revision,
                    "case {case:?}: reference {reference:?} resolved wrong: {body}"
                );
            }
        }
    }

    for reference in ["web:99", "nosuchfamily", "web:0"] {
        refused(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": reference }),
            "ClientException",
            reference,
        )
        .await;
    }
}

#[tokio::test]
async fn task_definition_compatibilities_follow_network_mode_and_task_size() {
    let router = service();

    // The rule: a revision is compatible with EC2 always (every revision is validated
    // against the capacity of the caller's own account), with FARGATE
    // only when the network mode is `awsvpc` AND task-level cpu
    // and memory are set ("For Amazon ECS tasks on Fargate, the awsvpc network mode is
    // required"; cpu/memory "required for Fargate"), with EXTERNAL only when the
    // network mode is not `awsvpc`, and with the managed-instances capacity Amazon ECS
    // operates only when the network mode IS `awsvpc` (tabled in
    // [`crate::managed_instances_compatibility`], which also covers what a registration
    // that PINS its launch types reports).
    // The list is reported in the launch-type order ECS reports, EXTERNAL, EC2,
    // FARGATE, MANAGED_INSTANCES, so a positional read names the same launch type the real
    // service named. What a registration asks to be validated against never ADDS a launch
    // type to the derived list, and subtracts only the one no placement may name outright
    // (tabled in [`crate::task_definition_compatibility_validation`] and
    // [`crate::managed_instances_compatibility`]); none of these rows asks for anything.
    // `networkMode` is the closed `NetworkMode` enum (bridge, host, awsvpc,
    // none) plus the documented default: "If no network mode is specified, the default is
    // bridge" — which the derivation reads while the member itself carries no default, so
    // a revision that declared none reads back with no `networkMode` at all.
    /// One row of the compatibilities table.
    struct Row {
        case: &'static str,
        network_mode: Option<&'static str>,
        task_size: bool,
        /// The `networkMode` the revision must read back as, or `None` for the member
        /// staying absent.
        expected_mode: Option<&'static str>,
        expected: &'static [&'static str],
    }

    let cases = [
        Row {
            case: "absent stays absent, and derives the bridge-mode launch types",
            network_mode: None,
            task_size: false,
            expected_mode: None,
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge",
            network_mode: Some("bridge"),
            task_size: false,
            expected_mode: Some("bridge"),
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "bridge with task size",
            network_mode: Some("bridge"),
            task_size: true,
            expected_mode: Some("bridge"),
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "host",
            network_mode: Some("host"),
            task_size: false,
            expected_mode: Some("host"),
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "none",
            network_mode: Some("none"),
            task_size: false,
            expected_mode: Some("none"),
            expected: &["EXTERNAL", "EC2"],
        },
        Row {
            case: "awsvpc without task size",
            network_mode: Some("awsvpc"),
            task_size: false,
            expected_mode: Some("awsvpc"),
            expected: &["EC2", "MANAGED_INSTANCES"],
        },
        Row {
            case: "awsvpc with task size",
            network_mode: Some("awsvpc"),
            task_size: true,
            expected_mode: Some("awsvpc"),
            expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let Row {
            case,
            network_mode,
            task_size,
            expected_mode,
            expected,
        } = row;
        let mut request = json!({
            "family": format!("compat{index}"),
            "containerDefinitions": [container("app", "nginx")],
        });
        if let Some(mode) = network_mode {
            request["networkMode"] = json!(mode);
        }
        if task_size {
            request["cpu"] = json!("256");
            request["memory"] = json!("512");
        }

        let body = ok(&router, "RegisterTaskDefinition", request, case).await;
        let definition = &body["taskDefinition"];
        let reported_mode = definition.get("networkMode").and_then(Value::as_str);
        assert_eq!(
            reported_mode, expected_mode,
            "case {case:?}: expected networkMode {expected_mode:?}, got {definition}"
        );
        let actual: Vec<String> = list(definition, "compatibilities", case)
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected compatibilities {expected:?}, got {actual:?} in \
             {definition}"
        );
    }
}

#[tokio::test]
async fn task_definition_requests_outside_the_documented_rules_are_refused() {
    let router = service();

    let too_long = "f".repeat(256);
    for (case, family) in [
        ("empty family", ""),
        ("slash in family", "web/one"),
        ("256 characters", too_long.as_str()),
    ] {
        refused(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": [container("app", "nginx")] }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // Each row is a container-definition or launch-type rule the real service enforces.
    let cases: [(&str, Value); 7] = [
        (
            "no containers",
            json!({ "family": "bad", "containerDefinitions": [] }),
        ),
        (
            "container without a name",
            json!({ "family": "bad", "containerDefinitions": [{ "image": "nginx", "memory": 128 }] }),
        ),
        (
            "container without an image",
            json!({ "family": "bad", "containerDefinitions": [{ "name": "app", "memory": 128 }] }),
        ),
        (
            "container with no memory and no task memory",
            json!({ "family": "bad", "containerDefinitions": [{ "name": "app", "image": "nginx" }] }),
        ),
        (
            "duplicate container names",
            json!({
                "family": "bad",
                "containerDefinitions": [container("app", "nginx"), container("app", "redis")],
            }),
        ),
        (
            "FARGATE without awsvpc",
            json!({
                "family": "bad",
                "containerDefinitions": [container("app", "nginx")],
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "bridge",
                "cpu": "256",
                "memory": "512",
            }),
        ),
        (
            "FARGATE without task size",
            json!({
                "family": "bad",
                "containerDefinitions": [container("app", "nginx")],
                "requiresCompatibilities": ["FARGATE"],
                "networkMode": "awsvpc",
            }),
        ),
    ];
    for (case, request) in cases {
        refused(
            &router,
            "RegisterTaskDefinition",
            request,
            "ClientException",
            case,
        )
        .await;
    }

    // The task-level `memory` satisfies the per-container requirement, so this is the
    // accepted counterpart of the "no memory" row above.
    let body = ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "tasklevelmemory",
            "memory": "512",
            "containerDefinitions": [{ "name": "app", "image": "nginx" }],
        }),
        "task-level memory",
    )
    .await;
    assert_eq!(
        count(&body["taskDefinition"], "revision", "task-level memory"),
        1,
        "expected task-level memory to satisfy the container memory rule, got {body}"
    );

    // "Containers per task definition" is 10 — the bound tested on itself and one past
    // it.
    let containers: Vec<Value> = (0..10)
        .map(|index| container(&format!("c{index}"), "nginx"))
        .collect();
    let at_bound = ok(
        &router,
        "RegisterTaskDefinition",
        json!({ "family": "tencontainers", "containerDefinitions": containers.clone() }),
        "ten containers",
    )
    .await;
    assert_eq!(
        list(&at_bound["taskDefinition"], "containerDefinitions", "ten containers").len(),
        10,
        "expected all ten stored container definitions back, got {at_bound}"
    );
    let mut too_many = containers;
    too_many.push(container("c10", "nginx"));
    refused(
        &router,
        "RegisterTaskDefinition",
        json!({ "family": "elevencontainers", "containerDefinitions": too_many }),
        "ClientException",
        "eleven containers",
    )
    .await;
}

#[tokio::test]
async fn run_describe_stop_and_delete_is_one_consistent_story() {
    let router = service();
    let cluster_arn = cluster_with_capacity(&router, "demo", "lifecycle").await;
    let (definition_arn, revision) = register(&router, "web", "app", "lifecycle").await;
    assert_eq!(revision, 1, "the first revision of a family is 1");

    // The rule generalises over `count`: 1, 2 and the documented maximum of 10.
    for requested in [1_i64, 2, 10] {
        let case = format!("count {requested}");
        let run = ok(
            &router,
            "RunTask",
            json!({
                "cluster": "demo",
                "taskDefinition": "web",
                "count": requested,
                "startedBy": "suite",
                "tags": [{ "key": "owner", "value": "suite" }],
            }),
            &case,
        )
        .await;
        let tasks = list(&run, "tasks", &case);
        assert_eq!(
            tasks.len() as i64,
            requested,
            "case {case:?}: expected {requested} tasks, got {run}"
        );
        assert!(
            list(&run, "failures", &case).is_empty(),
            "case {case:?}: expected no failures, got {run}"
        );

        let mut arns = Vec::new();
        for task in &tasks {
            let arn = string(task, "taskArn", &case);
            assert!(
                arn.starts_with(&format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/demo/"
                )),
                "case {case:?}: unexpected taskArn {arn:?}"
            );
            assert_eq!(
                string(task, "clusterArn", &case),
                cluster_arn,
                "case {case:?}: expected the stored cluster ARN, got {task}"
            );
            assert_eq!(
                string(task, "taskDefinitionArn", &case),
                definition_arn,
                "case {case:?}: expected the stored task-definition ARN, got {task}"
            );
            assert_eq!(
                string(task, "desiredStatus", &case),
                "RUNNING",
                "case {case:?}: a run task is desired RUNNING, got {task}"
            );
            assert_eq!(
                string(task, "lastStatus", &case),
                "PENDING",
                "case {case:?}: a freshly placed task has not started yet, got {task}"
            );
            assert_eq!(
                string(task, "launchType", &case),
                "EC2",
                "case {case:?}: expected the default launch type, got {task}"
            );
            assert_eq!(
                string(task, "group", &case),
                "family:web",
                "case {case:?}: expected the documented default group, got {task}"
            );
            assert_eq!(
                string(task, "startedBy", &case),
                "suite",
                "case {case:?}: expected startedBy back, got {task}"
            );
            let containers = list(task, "containers", &case);
            assert_eq!(
                containers.len(),
                1,
                "case {case:?}: expected one container per stored definition, got {task}"
            );
            assert_eq!(
                string(&containers[0], "name", &case),
                "app",
                "case {case:?}: container name must come from the stored definition, got {task}"
            );
            assert_eq!(
                string(&containers[0], "taskArn", &case),
                arn,
                "case {case:?}: the container must name its own task, got {task}"
            );
            assert!(
                string(&containers[0], "containerArn", &case).starts_with(&format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container/demo/"
                )),
                "case {case:?}: unexpected containerArn in {task}"
            );
            arns.push(arn);
        }
        assert_eq!(
            arns.iter().collect::<std::collections::BTreeSet<_>>().len(),
            arns.len(),
            "case {case:?}: every task must get its own ARN, got {arns:?}"
        );

        // The counters the cluster reports are the tasks that exist.
        let pending_view = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": ["demo"] }),
            &case,
        )
        .await;
        let cluster = &list(&pending_view, "clusters", &case)[0];
        assert_eq!(
            count(cluster, "pendingTasksCount", &case),
            requested,
            "case {case:?}: expected {requested} pending tasks, got {cluster}"
        );
        assert_eq!(
            count(cluster, "runningTasksCount", &case),
            0,
            "case {case:?}: nothing has started yet, got {cluster}"
        );

        // A cluster holding tasks that have not stopped cannot be deleted.
        refused(
            &router,
            "DeleteCluster",
            json!({ "cluster": "demo" }),
            "ClusterContainsTasksException",
            &case,
        )
        .await;

        // The clock, not a reader, is what starts the tasks.
        std::thread::sleep(ONE_SPAN);
        let described = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": "demo", "tasks": arns.clone(), "include": ["TAGS"] }),
            &case,
        )
        .await;
        let read = list(&described, "tasks", &case);
        assert_eq!(
            read.len(),
            arns.len(),
            "case {case:?}: expected every stored task back, got {described}"
        );
        for task in &read {
            assert_eq!(
                string(task, "lastStatus", &case),
                "RUNNING",
                "case {case:?}: the owed transition must have fallen due, got {task}"
            );
            assert!(
                task["startedAt"].is_number(),
                "case {case:?}: a started task records startedAt, got {task}"
            );
            assert_eq!(
                list(task, "tags", &case),
                vec![json!({ "key": "owner", "value": "suite" })],
                "case {case:?}: expected the stored tags with include TAGS, got {task}"
            );
            assert_eq!(
                string(&list(task, "containers", &case)[0], "lastStatus", &case),
                "RUNNING",
                "case {case:?}: the containers follow the task, got {task}"
            );
        }

        // Without `include`, the tags themselves are not included: the member is the empty
        // list every described task carries, never the tag set the task holds.
        let untagged = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": "demo", "tasks": [arns[0].clone()] }),
            &case,
        )
        .await;
        assert_eq!(
            list(&untagged, "tasks", &case)[0]["tags"],
            json!([]),
            "case {case:?}: the tags a task holds must not be included without include \
             TAGS, got {untagged}"
        );

        let running_view = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": ["demo"], "include": ["STATISTICS"] }),
            &case,
        )
        .await;
        let cluster = &list(&running_view, "clusters", &case)[0];
        assert_eq!(
            count(cluster, "runningTasksCount", &case),
            requested,
            "case {case:?}: expected {requested} running tasks, got {cluster}"
        );
        let statistics = list(cluster, "statistics", &case);
        let running_ec2 = statistics
            .iter()
            .find(|entry| entry["name"] == json!("runningEC2TasksCount"))
            .unwrap_or_else(|| panic!("case {case:?}: expected runningEC2TasksCount in {cluster}"));
        assert_eq!(
            running_ec2["value"],
            json!(requested.to_string()),
            "case {case:?}: statistics must agree with the stored tasks, got {cluster}"
        );

        // ListTasks sees them, and its default filter is desiredStatus RUNNING.
        let listed = ok(&router, "ListTasks", json!({ "cluster": "demo" }), &case).await;
        let mut listed_arns: Vec<String> = list(&listed, "taskArns", &case)
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        listed_arns.sort();
        let mut expected_arns = arns.clone();
        expected_arns.sort();
        assert_eq!(
            listed_arns, expected_arns,
            "case {case:?}: ListTasks must report exactly the stored tasks"
        );

        // Stop them: desiredStatus moves at once, lastStatus follows the clock.
        for arn in &arns {
            let stopped = ok(
                &router,
                "StopTask",
                json!({ "cluster": "demo", "task": arn, "reason": "done" }),
                &case,
            )
            .await;
            let task = &stopped["task"];
            assert_eq!(
                string(task, "desiredStatus", &case),
                "STOPPED",
                "case {case:?}: StopTask must record the desired stop, got {stopped}"
            );
            assert_eq!(
                string(task, "lastStatus", &case),
                "DEACTIVATING",
                "case {case:?}: a stopping task has not stopped yet, got {stopped}"
            );
            assert_eq!(
                string(task, "stoppedReason", &case),
                "done",
                "case {case:?}: expected the given reason, got {stopped}"
            );
            assert_eq!(
                string(task, "stopCode", &case),
                "UserInitiated",
                "case {case:?}: expected the user-initiated stop code, got {stopped}"
            );
            assert!(
                task["stoppingAt"].is_number(),
                "case {case:?}: expected a stoppingAt timestamp, got {stopped}"
            );
        }

        std::thread::sleep(ONE_SPAN);
        let after_stop = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": "demo", "tasks": arns.clone() }),
            &case,
        )
        .await;
        for task in list(&after_stop, "tasks", &case) {
            assert_eq!(
                string(&task, "lastStatus", &case),
                "STOPPED",
                "case {case:?}: a later describe must show STOPPED, got {task}"
            );
            assert!(
                task["stoppedAt"].is_number(),
                "case {case:?}: a stopped task records stoppedAt, got {task}"
            );
        }

        // The default ListTasks filter is desiredStatus RUNNING, so the stopped tasks
        // drop out of it and appear under the STOPPED filter instead.
        let still_running = ok(&router, "ListTasks", json!({ "cluster": "demo" }), &case).await;
        assert!(
            list(&still_running, "taskArns", &case).is_empty(),
            "case {case:?}: stopped tasks must leave the default filter, got {still_running}"
        );
        let stopped_list = ok(
            &router,
            "ListTasks",
            json!({ "cluster": "demo", "desiredStatus": "STOPPED" }),
            &case,
        )
        .await;
        assert_eq!(
            list(&stopped_list, "taskArns", &case).len(),
            arns.len(),
            "case {case:?}: expected the stopped tasks under the STOPPED filter, got \
             {stopped_list}"
        );

        // "Although you can filter results based on a desired status of PENDING, this
        // doesn't return any results."
        let pending_filter = ok(
            &router,
            "ListTasks",
            json!({ "cluster": "demo", "desiredStatus": "PENDING" }),
            &case,
        )
        .await;
        assert!(
            list(&pending_filter, "taskArns", &case).is_empty(),
            "case {case:?}: a PENDING desiredStatus filter returns nothing, got \
             {pending_filter}"
        );

        // Drain the cluster for the next row.
        ok(
            &router,
            "DeleteCluster",
            json!({ "cluster": "demo" }),
            &case,
        )
        .await;
        create_cluster(&router, "demo", &case).await;
        // The delete released the container instance with the cluster, so the cluster
        // this row runs its tasks in gets its own capacity back.
        register_capacity(&router, "demo", &case).await;
    }
}

#[tokio::test]
async fn run_task_resolves_every_documented_task_definition_reference() {
    let router = service();
    cluster_with_capacity(&router, "demo", "references").await;
    let (first_arn, _) = register(&router, "web", "app", "references").await;
    let (second_arn, second_revision) = register(&router, "web", "sidecar", "references").await;
    assert_eq!(second_revision, 2, "the second revision of a family is 2");

    // "The family and revision (family:revision) or full ARN of the task definition to
    // run. If a revision isn't specified, the latest ACTIVE revision is used."
    let cases: [(&str, &str, &str); 5] = [
        ("bare family", "web", second_arn.as_str()),
        ("family:1", "web:1", first_arn.as_str()),
        ("family:2", "web:2", second_arn.as_str()),
        ("full ARN", second_arn.as_str(), second_arn.as_str()),
        ("wildcard revision", "web:*", second_arn.as_str()),
    ];
    for (case, reference, expected) in cases {
        let run = ok(
            &router,
            "RunTask",
            json!({ "cluster": "demo", "taskDefinition": reference }),
            case,
        )
        .await;
        let task = &list(&run, "tasks", case)[0];
        assert_eq!(
            string(task, "taskDefinitionArn", case),
            expected,
            "case {case:?}: reference {reference:?} resolved wrong: {run}"
        );
    }

    // A reference that names nothing, or is not a task-definition reference at all.
    for (case, reference) in [
        ("unknown family", "nosuchfamily"),
        ("unknown revision", "web:9"),
        ("not a reference", "web/one"),
    ] {
        refused(
            &router,
            "RunTask",
            json!({ "cluster": "demo", "taskDefinition": reference }),
            "ClientException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn run_and_stop_requests_outside_the_documented_rules_are_refused() {
    let router = service();
    create_cluster(&router, "demo", "refusals").await;
    register(&router, "web", "app", "refusals").await;

    // "An unknown cluster must fail" — on the operations that take one.
    for operation in ["RunTask", "ListTasks", "DescribeTasks", "StopTask"] {
        let mut request = json!({ "cluster": "nosuchcluster" });
        request["taskDefinition"] = json!("web");
        request["tasks"] = json!(["0".repeat(32)]);
        request["task"] = json!("0".repeat(32));
        refused(
            &router,
            operation,
            request,
            "ClusterNotFoundException",
            operation,
        )
        .await;
    }

    // "You can specify up to 10 tasks for each call" — each bound and one past it.
    for (case, requested) in [("zero", 0_i64), ("negative", -1), ("eleven", 11)] {
        refused(
            &router,
            "RunTask",
            json!({ "cluster": "demo", "taskDefinition": "web", "count": requested }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // "If a launchType is specified, the capacityProviderStrategy parameter must be
    // omitted."
    refused(
        &router,
        "RunTask",
        json!({
            "cluster": "demo",
            "taskDefinition": "web",
            "launchType": "FARGATE",
            "capacityProviderStrategy": [{ "capacityProvider": "FARGATE", "weight": 1 }],
        }),
        "InvalidParameterException",
        "launchType with a capacity provider strategy",
    )
    .await;

    // "An error will be received if you specify the SERVICE option when running a
    // task."
    refused(
        &router,
        "RunTask",
        json!({ "cluster": "demo", "taskDefinition": "web", "propagateTags": "SERVICE" }),
        "InvalidParameterException",
        "propagateTags SERVICE",
    )
    .await;

    // An unknown task cannot be stopped.
    for (case, reference) in [
        ("short id", "0".repeat(32)),
        (
            "full ARN",
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/demo/{}", "f".repeat(32)),
        ),
    ] {
        refused(
            &router,
            "StopTask",
            json!({ "cluster": "demo", "task": reference }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // "This value can be between 1 and 100" — each bound, and one past each.
    for (case, max_results) in [("zero", 0_i64), ("101", 101)] {
        refused(
            &router,
            "ListTasks",
            json!({ "cluster": "demo", "maxResults": max_results }),
            "InvalidParameterException",
            case,
        )
        .await;
    }
    for (case, max_results) in [("one", 1_i64), ("hundred", 100)] {
        let body = ok(
            &router,
            "ListTasks",
            json!({ "cluster": "demo", "maxResults": max_results }),
            case,
        )
        .await;
        assert!(
            list(&body, "taskArns", case).is_empty(),
            "case {case:?}: an empty cluster lists no tasks, got {body}"
        );
    }
}

#[tokio::test]
async fn describe_tasks_reports_missing_for_ids_that_name_nothing() {
    let router = service();
    create_cluster(&router, "demo", "missing").await;
    register_capacity(&router, "demo", "missing").await;
    create_cluster(&router, "other", "missing").await;
    register(&router, "web", "app", "missing").await;
    let run = ok(
        &router,
        "RunTask",
        json!({ "cluster": "demo", "taskDefinition": "web" }),
        "missing",
    )
    .await;
    let known = string(&list(&run, "tasks", "missing")[0], "taskArn", "missing");

    // The rule generalises over the FORM of the id: a short id, a full ARN, and an ARN
    // of a task that lives in a different cluster are all "not in this cluster".
    let unknown_id = "9".repeat(32);
    let unknown_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/demo/{unknown_id}");
    let cases: [(&str, String); 4] = [
        ("bogus short id", "bogus".to_string()),
        ("unknown id", unknown_id.clone()),
        ("unknown ARN", unknown_arn.clone()),
        ("task of another cluster", known.clone()),
    ];
    for (case, reference) in cases {
        let cluster = if case == "task of another cluster" {
            "other"
        } else {
            "demo"
        };
        let body = ok(
            &router,
            "DescribeTasks",
            json!({ "cluster": cluster, "tasks": [reference.clone()] }),
            case,
        )
        .await;
        assert!(
            list(&body, "tasks", case).is_empty(),
            "case {case:?}: expected no task back, got {body}"
        );
        let failures = list(&body, "failures", case);
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: expected one failure, got {body}"
        );
        assert_eq!(
            string(&failures[0], "reason", case),
            "MISSING",
            "case {case:?}: expected reason MISSING, got {body}"
        );
        let expected_arn = if reference.starts_with("arn:") {
            reference.clone()
        } else {
            // An id that names no task is echoed as the ARN of that id ALONE: the
            // cluster segment of a task ARN names the cluster a stored task runs in.
            // See [`crate::describe_failure_identifiers`].
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{reference}")
        };
        assert_eq!(
            string(&failures[0], "arn", case),
            expected_arn,
            "case {case:?}: expected the failure to name the task ARN, got {body}"
        );
    }

    // A mixed request answers both halves from the same state.
    let mixed = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": "demo", "tasks": [known.clone(), unknown_arn.clone()] }),
        "mixed",
    )
    .await;
    assert_eq!(
        list(&mixed, "tasks", "mixed").len(),
        1,
        "expected the one stored task back, got {mixed}"
    );
    assert_eq!(
        list(&mixed, "failures", "mixed").len(),
        1,
        "expected one MISSING failure beside it, got {mixed}"
    );
}

#[tokio::test]
async fn describe_batches_are_bounded_and_deduplicated() {
    let router = service();
    create_cluster(&router, "demo", "batches").await;
    register_capacity(&router, "demo", "batches").await;
    register(&router, "web", "app", "batches").await;
    let run = ok(
        &router,
        "RunTask",
        json!({ "cluster": "demo", "taskDefinition": "web" }),
        "batches",
    )
    .await;
    let known = string(&list(&run, "tasks", "batches")[0], "taskArn", "batches");

    // One request names at most 100 ids — the bound tested on itself and one past it,
    // for both describe batches. The ids are deliberately outside the range the store
    // mints, so every one of them is MISSING.
    let hundred_tasks: Vec<String> = (0..100).map(|index| format!("f{index:031x}")).collect();
    let hundred_clusters: Vec<String> = (0..100).map(|index| format!("c{index}")).collect();
    for (case, operation, member, ids) in [
        ("100 tasks", "DescribeTasks", "tasks", hundred_tasks.clone()),
        (
            "100 clusters",
            "DescribeClusters",
            "clusters",
            hundred_clusters.clone(),
        ),
    ] {
        let mut request = json!({ "cluster": "demo" });
        request[member] = json!(ids);
        let body = ok(&router, operation, request, case).await;
        assert_eq!(
            list(&body, "failures", case).len(),
            100,
            "case {case:?}: expected a MISSING failure per distinct id, got {body}"
        );
    }
    for (case, operation, member, mut ids) in [
        ("101 tasks", "DescribeTasks", "tasks", hundred_tasks.clone()),
        (
            "101 clusters",
            "DescribeClusters",
            "clusters",
            hundred_clusters.clone(),
        ),
    ] {
        ids.push("one-too-many".to_string());
        let mut request = json!({ "cluster": "demo" });
        request[member] = json!(ids);
        refused(
            &router,
            operation,
            request,
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // The same id repeated names the same resource, so it is answered once: a request
    // cannot be multiplied into a larger response than the ids it distinctly names.
    let repeated = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": "demo", "tasks": vec![known.clone(); 50] }),
        "repeated task",
    )
    .await;
    assert_eq!(
        list(&repeated, "tasks", "repeated task").len(),
        1,
        "a repeated task id is answered once, got {repeated}"
    );
    let repeated_clusters = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": vec!["demo".to_string(); 50] }),
        "repeated cluster",
    )
    .await;
    assert_eq!(
        list(&repeated_clusters, "clusters", "repeated cluster").len(),
        1,
        "a repeated cluster name is answered once, got {repeated_clusters}"
    );
    let repeated_missing = ok(
        &router,
        "DescribeTasks",
        json!({ "cluster": "demo", "tasks": vec!["9".repeat(32); 50] }),
        "repeated missing",
    )
    .await;
    assert_eq!(
        list(&repeated_missing, "failures", "repeated missing").len(),
        1,
        "a repeated unknown id yields one MISSING failure, got {repeated_missing}"
    );
}

#[tokio::test]
async fn list_tasks_filters_and_pages_over_stored_tasks() {
    let router = service();
    cluster_with_capacity(&router, "demo", "filters").await;
    cluster_with_capacity(&router, "other", "filters").await;
    // `web` is registered Fargate-compatible (`awsvpc` plus task-level cpu and memory),
    // so the row that runs it under the FARGATE launch type asks for a placement the
    // revision was validated against; `worker` stays EC2-only.
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "web",
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("app", "public.ecr.aws/nginx/nginx:latest")],
        }),
        "filters",
    )
    .await;
    register(&router, "worker", "app", "filters").await;

    // Three tasks that differ along every filter the slice supports, plus one in a
    // different cluster that must never appear.
    let rows: [(&str, &str, &str, &str); 3] = [
        ("web-ci", "web", "ci", "EC2"),
        ("web-cron", "web", "cron", "FARGATE"),
        ("worker-ci", "worker", "ci", "EC2"),
    ];
    let mut arns = std::collections::HashMap::new();
    for (case, family, started_by, launch_type) in rows {
        let mut request = json!({
            "cluster": "demo",
            "taskDefinition": family,
            "startedBy": started_by,
            "launchType": launch_type,
        });
        if family == "web" {
            // `web` is an `awsvpc` revision, so its launch carries the network
            // configuration its elastic network interface takes.
            request["networkConfiguration"] = crate::ecs_wire::awsvpc_network_configuration();
        }
        let run = ok(&router, "RunTask", request, case).await;
        arns.insert(
            case,
            string(&list(&run, "tasks", case)[0], "taskArn", case),
        );
    }
    ok(
        &router,
        "RunTask",
        json!({
            "cluster": "other",
            "taskDefinition": "web",
            "networkConfiguration": crate::ecs_wire::awsvpc_network_configuration(),
        }),
        "other cluster",
    )
    .await;

    let filters: [(&str, Value, Vec<&str>); 6] = [
        ("no filter", json!({}), vec!["web-ci", "web-cron", "worker-ci"]),
        ("family web", json!({ "family": "web" }), vec!["web-ci", "web-cron"]),
        ("family worker", json!({ "family": "worker" }), vec!["worker-ci"]),
        ("startedBy ci", json!({ "startedBy": "ci" }), vec!["web-ci", "worker-ci"]),
        ("launchType FARGATE", json!({ "launchType": "FARGATE" }), vec!["web-cron"]),
        // Two filters together, neither of them one that has to be the only filter a
        // listing carries (`crate::task_listing_filters` drives those): the desired status
        // every listing filters on, beside the family.
        (
            "family and desiredStatus",
            json!({ "family": "web", "desiredStatus": "RUNNING" }),
            vec!["web-ci", "web-cron"],
        ),
    ];
    for (case, filter, expected) in filters {
        let mut request = json!({ "cluster": "demo" });
        for (key, value) in filter.as_object().expect("the filter is an object") {
            request[key] = value.clone();
        }
        let body = ok(&router, "ListTasks", request, case).await;
        let mut actual: Vec<String> = list(&body, "taskArns", case)
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        actual.sort();
        let mut want: Vec<String> = expected
            .iter()
            .map(|row| arns[*row].clone())
            .collect();
        want.sort();
        assert_eq!(
            actual, want,
            "case {case:?}: expected {expected:?} for this filter, got {body}"
        );
    }

    // Paging walks every task exactly once, whatever the page size.
    for page_size in [1_i64, 2, 3] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "cluster": "demo", "maxResults": page_size });
            if let Some(next) = &token {
                request["nextToken"] = json!(next);
            }
            let body = ok(&router, "ListTasks", request, &case).await;
            let page = list(&body, "taskArns", &case);
            assert!(
                page.len() as i64 <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {body}"
            );
            seen.extend(
                page.iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string()),
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        seen.sort();
        let mut want: Vec<String> = arns.values().cloned().collect();
        want.sort();
        assert_eq!(
            seen, want,
            "case {case:?}: paging must visit every stored task exactly once"
        );
    }

    // A name that matches no service in the cluster names nothing — a listing by service
    // resolves the service it reports on. An id that names no registered container instance
    // is a FILTER that matches nothing instead, which the listing answers as the empty
    // result it is ([`crate::task_listing_filters`] pins that).
    refused(
        &router,
        "ListTasks",
        json!({ "cluster": "demo", "serviceName": "nosuchservice" }),
        "ServiceNotFoundException",
        "serviceName filter",
    )
    .await;
    let listed = ok(
        &router,
        "ListTasks",
        json!({ "cluster": "demo", "containerInstance": "nosuchinstance" }),
        "containerInstance filter",
    )
    .await;
    assert!(
        list(&listed, "taskArns", "containerInstance filter").is_empty(),
        "case \"containerInstance filter\": an id no instance carries matches no task, got \
         {listed}"
    );
}

#[tokio::test]
async fn cluster_and_task_references_accept_a_name_or_an_arn() {
    let router = service();
    let cluster_arn = cluster_with_capacity(&router, "demo", "references").await;
    register(&router, "web", "app", "references").await;

    // Every `cluster` member's doc: "The short name or full Amazon Resource Name (ARN)
    // of the cluster". The rule generalises over the reference FORM, so both forms are
    // exercised on each operation that takes one.
    for (case, reference) in [("name", "demo".to_string()), ("ARN", cluster_arn.clone())] {
        let run = ok(
            &router,
            "RunTask",
            json!({ "cluster": reference, "taskDefinition": "web" }),
            case,
        )
        .await;
        let task = &list(&run, "tasks", case)[0];
        let arn = string(task, "taskArn", case);
        let task_id = arn
            .rsplit('/')
            .next()
            .expect("a task ARN ends in its id")
            .to_string();

        // The same task, reached by id and by ARN.
        for (inner, task_reference) in [("id", task_id.clone()), ("ARN", arn.clone())] {
            let described = ok(
                &router,
                "DescribeTasks",
                json!({ "cluster": reference, "tasks": [task_reference] }),
                inner,
            )
            .await;
            assert_eq!(
                string(&list(&described, "tasks", inner)[0], "taskArn", inner),
                arn,
                "case {case:?}/{inner:?}: expected the same stored task back, got {described}"
            );
        }

        let stopped = ok(
            &router,
            "StopTask",
            json!({ "cluster": reference, "task": task_id }),
            case,
        )
        .await;
        assert_eq!(
            string(&stopped["task"], "stoppedReason", case),
            "Task stopped by user",
            "case {case:?}: expected the default stopped reason, got {stopped}"
        );
        assert!(
            list(&stopped["task"], "tags", case).is_empty(),
            "case {case:?}: StopTask deletes the task's tags, got {stopped}"
        );
    }
}

/// The task ARNs one `RunTask` placed, in the order it reported them.
async fn run_task_arns(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let body = ok(router, "RunTask", request, case).await;
    list(&body, "tasks", case)
        .iter()
        .map(|task| string(task, "taskArn", case))
        .collect()
}

/// `base` with every member of `extra` written over it, so a row states only the member it
/// changes ([`crate::ecs_wire::with_members`] is the shared writer).
fn merged(base: &Value, extra: Value) -> Value {
    let mut merged = base.clone();
    with_members(&mut merged, extra);
    merged
}

#[tokio::test]
async fn one_client_token_performs_a_run_once() {
    // `RunTask$clientToken` is "An identifier that you provide to ensure the idempotency of
    // the request", so the run it names is performed at most ONCE: a repeat carrying the
    // same token and the same arguments is answered with the tasks the first call placed,
    // and a repeat that asks for something else under that token is the conflict the
    // operation documents ("The provided clientToken is already in use with a different
    // RunTask request. The resourceIds are the existing task ARNs which are already
    // associated with the clientToken").
    let router = service();
    let cluster_arn = cluster_with_capacity(&router, "idempotent", "setup").await;
    cluster_with_capacity(&router, "elsewhere", "setup").await;
    register(&router, "tokened", "app", "setup").await;

    let run = |cluster: &str, token: &str| {
        json!({
            "cluster": cluster,
            "taskDefinition": "tokened",
            "clientToken": token,
            "startedBy": "the-pipeline",
            "group": "family:tokened",
        })
    };

    let first_request = run("idempotent", "token-one");
    let case = "the first run under a token";
    let placed = run_task_arns(&router, first_request.clone(), case).await;
    assert_eq!(
        placed.len(),
        1,
        "case {case:?}: expected one task, got {placed:?}"
    );

    // A repeat is the same run whether it spells the arguments the way the first one did or
    // the way the handler resolves them: the cluster by name or by ARN, and `count` absent
    // or set to the `1` it defaults to.
    let repeats = [
        ("an identical repeat", json!({})),
        ("the cluster spelled as an ARN", json!({ "cluster": cluster_arn })),
        ("the defaulted count stated outright", json!({ "count": 1 })),
    ];
    for (case, extra) in repeats {
        let replayed = run_task_arns(&router, merged(&first_request, extra), case).await;
        assert_eq!(
            replayed, placed,
            "case {case:?}: a repeat of one run must answer that run's tasks, got {replayed:?}"
        );
    }

    // A repeat that asks for something ELSE under the token is refused, whichever member it
    // differs in, and the refusal carries the ARNs the token is already associated with.
    let conflicts = [
        ("a different count", json!({ "count": 2 })),
        ("a different group", json!({ "group": "family:other" })),
        ("a different startedBy", json!({ "startedBy": "a-human" })),
        (
            "tags the first run did not carry",
            json!({ "tags": [{ "key": "env", "value": "test" }] }),
        ),
        (
            "overrides the first run did not carry",
            json!({ "overrides": { "containers": [{ "name": "app", "cpu": 64 }] } }),
        ),
    ];
    for (case, extra) in conflicts {
        let refusal = refused(
            &router,
            "RunTask",
            merged(&first_request, extra),
            "ConflictException",
            case,
        )
        .await;
        assert_eq!(
            string(&refusal, "message", case),
            "The RunTask request could not be processed due to conflicts. The provided \
             clientToken is already in use with a different request.",
            "case {case:?}: unexpected conflict message in {refusal}"
        );
        let reported: Vec<String> = list(&refusal, "resourceIds", case)
            .iter()
            .map(|id| {
                id.as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected ARNs, got {refusal}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, placed,
            "case {case:?}: the conflict must report the tasks the token already names, \
             got {refusal}"
        );
    }

    // What the token does NOT dedupe: another token, the same token in another cluster, and
    // a run that carries no token at all.
    let distinct = [
        ("another token", run("idempotent", "token-two")),
        ("the same token in another cluster", run("elsewhere", "token-one")),
        (
            "no token at all",
            json!({ "cluster": "idempotent", "taskDefinition": "tokened" }),
        ),
        (
            "no token at all, again",
            json!({ "cluster": "idempotent", "taskDefinition": "tokened" }),
        ),
    ];
    let mut seen = placed.clone();
    for (case, request) in distinct {
        let placed = run_task_arns(&router, request, case).await;
        assert_eq!(
            placed.len(),
            1,
            "case {case:?}: expected one task, got {placed:?}"
        );
        assert!(
            !seen.contains(&placed[0]),
            "case {case:?}: expected a run of its own, got {} again",
            placed[0]
        );
        seen.extend(placed);
    }

    // And concurrent retries of one run: they are serialised on the store, so the token is
    // recorded by whichever of them gets there first and the rest answer that run's task.
    let case = "five concurrent runs sharing one token";
    let shared = run("idempotent", "token-concurrent");
    let (one, two, three, four, five) = tokio::join!(
        run_task_arns(&router, shared.clone(), case),
        run_task_arns(&router, shared.clone(), case),
        run_task_arns(&router, shared.clone(), case),
        run_task_arns(&router, shared.clone(), case),
        run_task_arns(&router, shared.clone(), case),
    );
    let mut concurrent: Vec<String> = [one, two, three, four, five].concat();
    concurrent.sort();
    concurrent.dedup();
    assert_eq!(
        concurrent.len(),
        1,
        "case {case:?}: expected every racer to answer one task, got {concurrent:?}"
    );
}

/// The `{key, value}` pairs of a reported tag list, sorted so two readers of one tag set
/// compare equal whatever order their maps happened to yield.
fn sorted_tags(tags: &[Value], case: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .map(|tag| (string(tag, "key", case), string(tag, "value", case)))
        .collect();
    pairs.sort();
    pairs
}

/// The tags stored on `task`, as `DescribeTasks` reports them.
async fn task_tags(
    router: &MultiServiceRouter,
    cluster: &str,
    task: &str,
    case: &str,
) -> Vec<(String, String)> {
    let read = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task], "include": ["TAGS"] }),
        case,
    )
    .await;
    let described = list(&read, "tasks", case);
    let task = described
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the task back, got {read}"));
    sorted_tags(&list(task, "tags", case), case)
}

/// One row of the managed-tag table: the cluster the run places into, the members it adds
/// to the bare request, and the `{key, value}` pairs the task must end up carrying.
struct ManagedTagRow {
    case: &'static str,
    cluster: &'static str,
    extra: fn() -> Value,
    expect: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn ecs_managed_tags_tag_a_standalone_task_with_the_cluster_that_runs_it() {
    // `RunTask$enableECSManagedTags` is "Specifies whether to use Amazon ECS managed tags
    // for the task": with it on, Amazon ECS tags the task with the resources that task
    // belongs to, under the `aws:` prefix it keeps for its own tags. A task a client runs
    // belongs to no service, so the cluster it runs in is the one it is tagged with —
    // beside whatever `propagateTags` copied and whatever the request carried.
    let router = service();
    for cluster in ["managedtags", "othertags"] {
        cluster_with_capacity(&router, cluster, "setup").await;
    }
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": "managed",
            "containerDefinitions": [container("app", "public.ecr.aws/nginx/nginx:latest")],
            "tags": [{ "key": "from", "value": "definition" }],
        }),
        "setup",
    )
    .await;

    // The rows differ along whether the managed tags were asked for, what else the task
    // carries (nothing, the revision's tags, the request's own), and WHICH cluster runs it
    // — so the value of the managed tag is read off the cluster rather than being a
    // constant.
    let rows = [
        ManagedTagRow {
            case: "managed tags off, nothing else to carry",
            cluster: "managedtags",
            extra: || json!({}),
            expect: &[],
        },
        ManagedTagRow {
            case: "managed tags off with the revision's tags propagated",
            cluster: "managedtags",
            extra: || json!({ "propagateTags": "TASK_DEFINITION" }),
            expect: &[("from", "definition")],
        },
        ManagedTagRow {
            case: "managed tags on, nothing else to carry",
            cluster: "managedtags",
            extra: || json!({ "enableECSManagedTags": true }),
            expect: &[("aws:ecs:clusterName", "managedtags")],
        },
        ManagedTagRow {
            case: "managed tags on, beside the propagated and the requested tags",
            cluster: "managedtags",
            extra: || {
                json!({
                    "enableECSManagedTags": true,
                    "propagateTags": "TASK_DEFINITION",
                    "tags": [{ "key": "owner", "value": "platform" }],
                })
            },
            expect: &[
                ("aws:ecs:clusterName", "managedtags"),
                ("from", "definition"),
                ("owner", "platform"),
            ],
        },
        ManagedTagRow {
            case: "managed tags on in another cluster, whose name is the value",
            cluster: "othertags",
            extra: || json!({ "enableECSManagedTags": true }),
            expect: &[("aws:ecs:clusterName", "othertags")],
        },
    ];

    for row in &rows {
        let case = row.case;
        let request = merged(
            &json!({ "cluster": row.cluster, "taskDefinition": "managed" }),
            (row.extra)(),
        );
        let placed = run_task_arns(&router, request, case).await;
        let mut expected: Vec<(String, String)> = row
            .expect
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            task_tags(&router, row.cluster, &placed[0], case).await,
            expected,
            "case {case:?}: unexpected tag set on the task {} placed", placed[0]
        );
    }
}

#[tokio::test]
async fn ecs_managed_tags_tag_a_services_task_with_the_cluster_and_the_service() {
    // The other half of the same member (`CreateService$enableECSManagedTags`): a task a
    // SERVICE placed belongs to the cluster and to that service, so it carries a managed
    // tag for each.
    let router = service();
    let cluster = "servicetags";
    let case = "a service with its managed tags on";
    cluster_with_capacity(&router, cluster, case).await;
    register(&router, "servicetagged", "app", case).await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": "tagged",
            "taskDefinition": "servicetagged",
            "desiredCount": 1,
            "launchType": "EC2",
            "enableECSManagedTags": true,
            "propagateTags": "SERVICE",
            "tags": [{ "key": "owner", "value": "platform" }],
        }),
        case,
    )
    .await;

    // The task is the SCHEDULER's, so the suite waits for the placement to be made rather
    // than for a span.
    let listed = poll_until(
        case,
        "the service's task to be placed",
        || {
            ok(
                &router,
                "ListTasks",
                json!({ "cluster": cluster, "serviceName": "tagged" }),
                case,
            )
        },
        |body| {
            body["taskArns"]
                .as_array()
                .is_some_and(|arns| arns.len() == 1)
        },
    )
    .await;
    let arns = list(&listed, "taskArns", case);
    let task = arns[0]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {listed}"));
    assert_eq!(
        task_tags(&router, cluster, task, case).await,
        vec![
            ("aws:ecs:clusterName".to_string(), cluster.to_string()),
            ("aws:ecs:serviceName".to_string(), "tagged".to_string()),
            ("owner".to_string(), "platform".to_string()),
        ],
        "case {case:?}: expected the service's propagated tag beside both managed tags"
    );
}
