// SPDX-License-Identifier: CC0-1.0

//! The launch types a task-definition projection reports, read through EVERY operation that
//! answers with one.
//!
//! The rule under test: `TaskDefinition$compatibilities` ("Amazon ECS validates the task
//! definition parameters with those supported by the launch type") is drawn from every member
//! of the `Compatibility` enum (`smithy_query(query="shape",
//! shape="com.amazonaws.ecs#Compatibility")` -> `EC2`, `FARGATE`, `EXTERNAL`,
//! `MANAGED_INSTANCES`), reported in the order a recorded ECS projection carries them —
//! `EXTERNAL`, `EC2`, `FARGATE`, `MANAGED_INSTANCES` — and filtered by the stored revision's
//! own parameters. The capacity Amazon ECS provisions and operates for a cluster is reported
//! for an `awsvpc` revision that left its launch types open, since a placement reaches that
//! capacity only through a capacity provider
//! ([`crate::managed_instances_compatibility`] tables that arm).
//!
//! The rows span what the rule generalises over: the network mode (each member of
//! `NetworkMode` plus the documented default), whether task-level `cpu` and `memory` are
//! declared, and what the registration ASKED to be validated against — including a request
//! that asks for managed-instances capacity, which is echoed back in
//! `requiresCompatibilities` unchanged.
//!
//! Each row is read back through all four projections of the SAME stored revision — the
//! registration's echo, `DescribeTaskDefinition`, `DeregisterTaskDefinition` and
//! `DeleteTaskDefinitions` — so a list one operation renders differently from the others
//! shows up as a difference between them, and the POSITION of each launch type is asserted,
//! not just its membership.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, service};
use crate::task_lifecycle::container;

/// A public image every container here starts from, which needs no principal to pull: the
/// image is not what the rule turns on.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// A legal Fargate task size, from `RegisterTaskDefinition$memory`'s documented cpu/memory
/// pairs.
const TASK_CPU: &str = "512";
const TASK_MEMORY: &str = "1024";

/// The launch type the `Compatibility` enum carries that a placement may not name outright.
const MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// Every launch type a projection may report, in the order one reports them.
const REPORTED_LAUNCH_TYPES: [&str; 4] = ["EXTERNAL", "EC2", "FARGATE", MANAGED_INSTANCES];

/// One row of the table: the request beside the family and the containers, and the launch
/// types the revision must report.
struct Row {
    case: &'static str,
    request: Value,
    expected: &'static [&'static str],
    /// What the registration asked to be validated against, which every projection echoes
    /// back unchanged.
    requires: &'static [&'static str],
}

/// The `compatibilities` of one projection, in the order it reports them.
fn compatibilities(definition: &Value, case: &str, source: &str) -> Vec<String> {
    list(definition, "compatibilities", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {source} reported a launch type that is not a string, \
                         got {value}"
                    )
                })
                .to_string()
        })
        .collect()
}

/// The `requiresCompatibilities` of one projection, which is absent when the registration
/// asked for nothing.
fn requires(definition: &Value, case: &str) -> Vec<String> {
    match definition.get("requiresCompatibilities") {
        None | Some(Value::Null) => Vec::new(),
        Some(_) => list(definition, "requiresCompatibilities", case)
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect(),
    }
}

/// Assert one projection reports exactly `row.expected`, in order, and echoes
/// `row.requires` unchanged.
fn assert_projection(definition: &Value, source: &str, row: &Row) {
    let case = row.case;
    let reported = compatibilities(definition, case, source);
    assert_eq!(
        reported, row.expected,
        "case {case:?}: {source} expected compatibilities {:?}, got {reported:?} in \
         {definition}",
        row.expected
    );
    assert!(
        reported
            .iter()
            .all(|launch_type| REPORTED_LAUNCH_TYPES.contains(&launch_type.as_str())),
        "case {case:?}: {source} must report only {REPORTED_LAUNCH_TYPES:?}, got {reported:?}"
    );
    let asked = requires(definition, case);
    assert_eq!(
        asked, row.requires,
        "case {case:?}: {source} expected requiresCompatibilities {:?}, got {asked:?} in \
         {definition}",
        row.requires
    );
}

/// The rows of the table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the documented default network mode",
            request: json!({}),
            expected: &["EXTERNAL", "EC2"],
            requires: &[],
        },
        Row {
            case: "bridge",
            request: json!({ "networkMode": "bridge" }),
            expected: &["EXTERNAL", "EC2"],
            requires: &[],
        },
        Row {
            case: "host",
            request: json!({ "networkMode": "host" }),
            expected: &["EXTERNAL", "EC2"],
            requires: &[],
        },
        Row {
            case: "none",
            request: json!({ "networkMode": "none" }),
            expected: &["EXTERNAL", "EC2"],
            requires: &[],
        },
        Row {
            case: "awsvpc without a task size",
            request: json!({ "networkMode": "awsvpc" }),
            expected: &["EC2", "MANAGED_INSTANCES"],
            requires: &[],
        },
        Row {
            case: "awsvpc with a task size",
            request: json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
            }),
            expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
            requires: &[],
        },
        Row {
            case: "asking to be validated against the caller's own capacity",
            request: json!({
                "networkMode": "bridge",
                "requiresCompatibilities": ["EC2"],
            }),
            expected: &["EXTERNAL", "EC2"],
            requires: &["EC2"],
        },
        Row {
            case: "asking to be validated against managed-instances capacity",
            request: json!({
                "networkMode": "bridge",
                "requiresCompatibilities": ["MANAGED_INSTANCES"],
            }),
            expected: &["EXTERNAL", "EC2"],
            requires: &["MANAGED_INSTANCES"],
        },
        Row {
            case: "asking to be validated against Fargate alone",
            request: json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": ["FARGATE"],
            }),
            expected: &["EC2", "FARGATE"],
            requires: &["FARGATE"],
        },
        Row {
            case: "asking to be validated against Fargate and managed-instances capacity",
            request: json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": ["FARGATE", "MANAGED_INSTANCES"],
            }),
            expected: &["EC2", "FARGATE", "MANAGED_INSTANCES"],
            requires: &["FARGATE", "MANAGED_INSTANCES"],
        },
    ]
}

/// The stored revision `reference` names, as `DescribeTaskDefinition` reports it.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    ok(
        router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": reference }),
        case,
    )
    .await["taskDefinition"]
        .clone()
}

/// RULE: every projection of a stored revision reports the launch types the revision's own
/// parameters were validated against, drawn from every launch type the `Compatibility` enum
/// carries and in the order a recorded projection reports them.
#[tokio::test]
async fn every_task_definition_projection_reports_the_same_launch_types() {
    let router = service();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let family = format!("ltproj{index}");
        let mut request = row.request.clone();
        request["family"] = json!(family);
        request["containerDefinitions"] = json!([container("app", IMAGE)]);

        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        assert_projection(&registered["taskDefinition"], "RegisterTaskDefinition", &row);

        let described = describe(&router, &family, case).await;
        assert_projection(&described, "DescribeTaskDefinition", &row);

        // The revision is read back through the two operations that END its life as well:
        // both answer with the projection of the revision they just moved, so neither may
        // re-derive the list from the request it was given.
        let revision = format!("{family}:1");
        let deregistered = ok(
            &router,
            "DeregisterTaskDefinition",
            json!({ "taskDefinition": revision }),
            case,
        )
        .await;
        assert_projection(
            &deregistered["taskDefinition"],
            "DeregisterTaskDefinition",
            &row,
        );

        let deleted = ok(
            &router,
            "DeleteTaskDefinitions",
            json!({ "taskDefinitions": [revision.clone()] }),
            case,
        )
        .await;
        let definitions = list(&deleted, "taskDefinitions", case);
        assert_eq!(
            definitions.len(),
            1,
            "case {case:?}: DeleteTaskDefinitions must answer with the revision it deleted, \
             got {deleted}"
        );
        assert_projection(&definitions[0], "DeleteTaskDefinitions", &row);
    }
}

/// RULE, read the other way round: across the whole table no projection names a launch type
/// outside the four the `Compatibility` enum carries, and each of the three that depend on
/// the revision's parameters is reported by some row — so the table pins a derivation
/// rather than a constant list.
#[tokio::test]
async fn the_reported_launch_types_are_drawn_from_the_modelled_four() {
    let router = service();
    let mut seen: Vec<String> = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let family = format!("ltdrawn{index}");
        let mut request = row.request.clone();
        request["family"] = json!(family);
        request["containerDefinitions"] = json!([container("app", IMAGE)]);
        let registered = ok(&router, "RegisterTaskDefinition", request, case).await;
        let reported = compatibilities(
            &registered["taskDefinition"],
            case,
            "RegisterTaskDefinition",
        );
        for launch_type in reported {
            if !seen.contains(&launch_type) {
                seen.push(launch_type);
            }
        }
    }

    for launch_type in REPORTED_LAUNCH_TYPES {
        assert!(
            seen.iter().any(|named| named == launch_type),
            "the table must read back {launch_type:?} on some row, got {seen:?}"
        );
    }
    assert!(
        seen.iter()
            .all(|named| REPORTED_LAUNCH_TYPES.contains(&named.as_str())),
        "no row may report a launch type outside {REPORTED_LAUNCH_TYPES:?}, got {seen:?}"
    );
}
