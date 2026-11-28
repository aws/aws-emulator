// SPDX-License-Identifier: CC0-1.0

//! Managed-instances capacity against the launch types a registered revision reports, read
//! back through both projections of the stored revision.
//!
//! The rule under test: `TaskDefinition$compatibilities` ("Amazon ECS validates the task
//! definition parameters with those supported by the launch type") is DERIVED from the
//! revision's OWN parameters, one predicate per member of the `Compatibility` enum
//! (`smithy_query(query="shape", shape="com.amazonaws.ecs#Compatibility")` -> `EC2`,
//! `FARGATE`, `EXTERNAL`, `MANAGED_INSTANCES`). The member this suite is about, the capacity
//! Amazon ECS provisions and operates for a cluster, is reported when both of these hold of
//! the stored revision:
//!
//! * its task runs under `awsvpc` networking, the mode that allocates a task its own elastic
//!   network interface ("If the network mode is `awsvpc`, the task is allocated an elastic
//!   network interface"), so a revision running in the documented default, `bridge`, `host`
//!   or `none` reports nothing about it;
//! * the registration left its launch types OPEN, or named this one in
//!   `requiresCompatibilities` ("The task launch types the task definition was validated
//!   against"). The launch types a placement may name OUTRIGHT are validated for whatever
//!   the registration pinned, because any placement can ask for them; this capacity is
//!   reached only through a capacity provider (`RunTask$launchType`: "If you want to use
//!   Amazon ECS Managed Instances, you must use the `capacityProviderStrategy` request
//!   parameter and omit the `launchType` request parameter"), so a registration pinned to
//!   other launch types was validated against those and not against this.
//!
//! The rows span what the rule generalises over:
//!
//! * the NETWORK MODE: the documented default, each member of the `NetworkMode` enum, and
//!   `awsvpc` with and without a task size;
//! * what the registration PINNED (`requiresCompatibilities`): nothing, another launch type
//!   alone, managed instances alone, and managed instances beside another launch type;
//! * whether the ACCOUNT holds a managed-instances capacity provider, which the derivation
//!   does not read: the same rows are registered against an account that created one and
//!   against one that created none, and both report the same list;
//! * a revision Fargate does NOT support (an Amazon ECR image with no `executionRoleArn`),
//!   where only the Fargate answer changes — the capacity Amazon ECS operates for the
//!   cluster runs container instances of its own, whose role the agent pulls under;
//! * and both projections of the same stored revision — the registration's echo and
//!   `DescribeTaskDefinition` — so a list rendered off the request rather than off the record
//!   shows up as a difference between them.
//!
//! The POSITION of each launch type is asserted, not just its membership: a client that reads
//! the list positionally must name the same launch type the real service named.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::managed_instances;
use crate::ecs_wire::{create_cluster, list, ok, refusal, service};
use crate::task_lifecycle::container;

/// A public image every container here starts from, which needs no principal to pull.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// An image held in this account's own Amazon ECR registry, which Amazon ECS pulls AS the task
/// execution role ("the task execution role that grants the Amazon ECS container agent
/// permission to make Amazon Web Services API calls on your behalf").
const ECR_IMAGE: &str = "000000000000.dkr.ecr.us-west-2.amazonaws.com/app:latest";

/// A legal Fargate task size, from `RegisterTaskDefinition$memory`'s documented cpu/memory
/// pairs.
const TASK_CPU: &str = "512";
const TASK_MEMORY: &str = "1024";

/// The launch type this suite is about.
const MANAGED_INSTANCES: &str = "MANAGED_INSTANCES";

/// The `compatibilities` of one projection, in the order it reports them.
fn compatibilities(definition: &Value, case: &str) -> Vec<String> {
    list(definition, "compatibilities", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a string launch type, got {value}")
                })
                .to_string()
        })
        .collect()
}

/// Assert `definition` reports exactly `expected`, in that order.
fn reports(definition: &Value, source: &str, expected: &[&str], case: &str) {
    let actual = compatibilities(definition, case);
    assert_eq!(
        actual, expected,
        "case {case:?}: {source} expected compatibilities {expected:?}, got {actual:?} in \
         {definition}"
    );
}

/// Give the account a managed-instances capacity provider: a cluster, and a provider of that
/// class scoped to it — the only way an account comes to hold that capacity.
async fn create_managed_instances_capacity(
    router: &MultiServiceRouter,
    cluster: &str,
    provider: &str,
    case: &str,
) {
    create_cluster(router, cluster, case).await;
    ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": provider,
            "cluster": cluster,
            "managedInstancesProvider": managed_instances(),
        }),
        case,
    )
    .await;
}

/// Register `request` under `family` and read the revision back, answering what each
/// projection reported.
async fn register_and_describe(
    router: &MultiServiceRouter,
    family: &str,
    mut request: Value,
    case: &str,
) -> Vec<(&'static str, Value)> {
    request["family"] = json!(family);
    let registered = ok(router, "RegisterTaskDefinition", request, case).await;
    let described = describe(router, family, case).await;
    vec![
        (
            "RegisterTaskDefinition",
            registered["taskDefinition"].clone(),
        ),
        ("DescribeTaskDefinition", described),
    ]
}

/// The stored revision `reference` names, as `DescribeTaskDefinition` reports it.
async fn describe(router: &MultiServiceRouter, reference: &str, case: &str) -> Value {
    let described = ok(
        router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": reference }),
        case,
    )
    .await;
    described["taskDefinition"].clone()
}

/// (case, the request beside the family and containers, the launch types the revision
/// reports).
fn rows() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        (
            "the documented default network mode",
            json!({}),
            vec!["EXTERNAL", "EC2"],
        ),
        (
            "bridge",
            json!({"networkMode": "bridge"}),
            vec!["EXTERNAL", "EC2"],
        ),
        (
            "host",
            json!({"networkMode": "host"}),
            vec!["EXTERNAL", "EC2"],
        ),
        (
            "none",
            json!({"networkMode": "none"}),
            vec!["EXTERNAL", "EC2"],
        ),
        (
            "awsvpc without a task size",
            json!({"networkMode": "awsvpc"}),
            vec!["EC2", MANAGED_INSTANCES],
        ),
        (
            "awsvpc with a task size",
            json!({"networkMode": "awsvpc", "cpu": TASK_CPU, "memory": TASK_MEMORY}),
            vec!["EC2", "FARGATE", MANAGED_INSTANCES],
        ),
        // A mode other than `awsvpc` rules the launch type out however the registration
        // pinned its launch types.
        (
            "bridge pinned to managed instances alone",
            json!({"networkMode": "bridge", "requiresCompatibilities": [MANAGED_INSTANCES]}),
            vec!["EXTERNAL", "EC2"],
        ),
        // A pin that names OTHER launch types is the validation the registration asked for,
        // and this launch type is not among them.
        (
            "awsvpc pinned to Fargate alone",
            json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": ["FARGATE"],
            }),
            vec!["EC2", "FARGATE"],
        ),
        (
            "awsvpc pinned to the caller's own capacity alone",
            json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": ["EC2"],
            }),
            vec!["EC2", "FARGATE"],
        ),
        (
            "awsvpc pinned to managed instances alone",
            json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": [MANAGED_INSTANCES],
            }),
            vec!["EC2", "FARGATE", MANAGED_INSTANCES],
        ),
        (
            "awsvpc pinned to Fargate and managed instances",
            json!({
                "networkMode": "awsvpc",
                "cpu": TASK_CPU,
                "memory": TASK_MEMORY,
                "requiresCompatibilities": ["FARGATE", MANAGED_INSTANCES],
            }),
            vec!["EC2", "FARGATE", MANAGED_INSTANCES],
        ),
        (
            "awsvpc at no task size, pinned to managed instances",
            json!({
                "networkMode": "awsvpc",
                "requiresCompatibilities": [MANAGED_INSTANCES],
            }),
            vec!["EC2", MANAGED_INSTANCES],
        ),
    ]
}

/// RULE: a revision reports the capacity Amazon ECS operates for a cluster when its task runs
/// under `awsvpc` networking and the registration did not pin its launch types to others —
/// in an account that created no capacity provider of that class.
#[tokio::test]
async fn the_launch_type_follows_the_network_mode_and_what_the_registration_pinned() {
    let router = service();

    for (index, (case, mut request, expected)) in rows().into_iter().enumerate() {
        request["containerDefinitions"] = json!([container("app", IMAGE)]);
        for (source, definition) in
            register_and_describe(&router, &format!("mifam{index}"), request, case).await
        {
            reports(&definition, source, &expected, case);
        }
    }
}

/// RULE, the other side of it: the derivation does not read the account's capacity providers,
/// so the same rows registered in an account that HOLDS a managed-instances capacity provider
/// report exactly the same list.
#[tokio::test]
async fn the_reported_list_does_not_turn_on_the_capacity_the_account_holds() {
    let router = service();
    create_managed_instances_capacity(&router, "mifleetcluster", "mifleet", "setup").await;

    for (index, (case, mut request, expected)) in rows().into_iter().enumerate() {
        request["containerDefinitions"] = json!([container("app", IMAGE)]);
        for (source, definition) in
            register_and_describe(&router, &format!("miheld{index}"), request, case).await
        {
            reports(&definition, source, &expected, case);
        }
    }
}

/// RULE, over time: the derived list is the validation the REGISTRATION performed, so reading a
/// revision back never rewrites it — a revision registered before the account held any
/// managed-instances capacity provider reports the same list after one exists, and so does the
/// next revision of the same family.
#[tokio::test]
async fn the_derived_list_is_the_one_the_registration_validated() {
    let router = service();
    let case = "a revision registered before a capacity provider existed";
    let family = "mitimeline";
    let request = json!({
        "networkMode": "awsvpc",
        "cpu": TASK_CPU,
        "memory": TASK_MEMORY,
        "containerDefinitions": [container("app", IMAGE)],
    });
    let expected = ["EC2", "FARGATE", MANAGED_INSTANCES];

    let before = ok(
        &router,
        "RegisterTaskDefinition",
        {
            let mut first = request.clone();
            first["family"] = json!(family);
            first
        },
        case,
    )
    .await;
    reports(
        &before["taskDefinition"],
        "RegisterTaskDefinition",
        &expected,
        case,
    );

    create_managed_instances_capacity(&router, "milatercluster", "milater", case).await;

    let reread = describe(&router, &format!("{family}:1"), case).await;
    reports(
        &reread,
        "DescribeTaskDefinition after a provider was created",
        &expected,
        case,
    );

    let case = "the next revision, registered against an account holding a provider";
    for (source, definition) in register_and_describe(&router, family, request, case).await {
        reports(&definition, source, &expected, case);
    }
}

/// RULE, where FARGATE drops out: a revision whose image can be pulled only AS the task
/// execution role, and that names none, is not validated against Fargate — Amazon ECS's own
/// Fargate capacity has no container instance whose role the agent could pull under — while the
/// caller's own capacity and the instances Amazon ECS runs for the cluster still support it.
/// Naming the execution role puts Fargate back and moves nothing else.
#[tokio::test]
async fn an_image_needing_a_principal_drops_only_fargate() {
    let router = service();
    create_managed_instances_capacity(&router, "mipullcluster", "mipull", "setup").await;

    // (case, the executionRoleArn the registration names, the launch types it must report).
    let rows: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        (
            "an Amazon ECR image with no execution role",
            None,
            vec!["EC2", MANAGED_INSTANCES],
        ),
        (
            "the same image with an execution role",
            Some("arn:aws:iam::000000000000:role/ecsTaskExecutionRole"),
            vec!["EC2", "FARGATE", MANAGED_INSTANCES],
        ),
    ];

    for (index, (case, execution_role, expected)) in rows.into_iter().enumerate() {
        let mut request = json!({
            "networkMode": "awsvpc",
            "cpu": TASK_CPU,
            "memory": TASK_MEMORY,
            "containerDefinitions": [container("app", ECR_IMAGE)],
        });
        if let Some(role) = execution_role {
            request["executionRoleArn"] = json!(role);
        }
        for (source, definition) in
            register_and_describe(&router, &format!("mipullfam{index}"), request, case).await
        {
            reports(&definition, source, &expected, case);
        }
    }
}

/// RULE, the narrower question a PLACEMENT puts: reporting the launch type is not permission
/// to NAME it. `RunTask$launchType` — "If you want to use Amazon ECS Managed Instances, you
/// must use the `capacityProviderStrategy` request parameter and omit the `launchType`
/// request parameter" — so a request that names it is refused however compatible the revision
/// is with that capacity, while the launch types a placement may name outright are accepted
/// exactly when the revision reports them.
#[tokio::test]
async fn naming_the_launch_type_in_a_placement_stays_refused() {
    let router = service();
    let case = "setup";
    create_managed_instances_capacity(&router, "migatecluster", "migate", case).await;

    // Two revisions of the same `awsvpc` shape that differ only in what they pinned, so one
    // reports the launch type and the other does not.
    for (family, requires) in [
        ("migateopen", json!([])),
        ("migatepinned", json!(["FARGATE"])),
    ] {
        let mut request = json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": TASK_CPU,
            "memory": TASK_MEMORY,
            "containerDefinitions": [container("app", IMAGE)],
        });
        if !requires.as_array().is_some_and(|asked| asked.is_empty()) {
            request["requiresCompatibilities"] = requires;
        }
        ok(&router, "RegisterTaskDefinition", request, case).await;
    }

    for (case, family) in [
        ("a revision that reports the launch type", "migateopen"),
        ("a revision that does not", "migatepinned"),
    ] {
        let (code, message) = refusal(
            &router,
            "RunTask",
            json!({
                "cluster": "migatecluster",
                "taskDefinition": family,
                "launchType": MANAGED_INSTANCES,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the modelled refusal, got {code:?} with {message:?}"
        );
        assert_eq!(
            message,
            format!("Task definition does not support launch_type {MANAGED_INSTANCES}."),
            "case {case:?}: the refusal must name the launch type asked for, got {message:?}"
        );
    }
}
