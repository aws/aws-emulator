// SPDX-License-Identifier: CC0-1.0

//! The container HEALTH CHECK a task definition declares, driven end to end over the shared
//! wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, over the whole class rather than one request:
//!
//! > A container whose definition declares a `healthCheck` has that command EXECUTED while
//! > the container runs, and `DescribeTasks` reports the result: a command that exits zero
//! > takes the container to `HEALTHY` ("An exit code of 0 indicates success"), one that
//! > exits non-zero takes it to `UNHEALTHY` once it has failed `retries` times
//! > (`HealthCheck$retries`: "the number of times to retry a failed health check before the
//! > container is considered unhealthy"), and a `startPeriod` only SUPPRESSES that until the
//! > period runs out ("the optional grace period to provide containers time to bootstrap
//! > before failed health checks count towards the maximum number of retries"). A container
//! > with no health check reports `UNKNOWN` for ever. The TASK's own `healthStatus` is the
//! > reading of its ESSENTIAL containers (`Task$healthStatus`), and a container whose
//! > `dependsOn` names a `HEALTHY` condition does not START until the container it names is
//! > healthy (`ContainerDependency$condition`: `HEALTHY` "validates that the dependent
//! > container passes its Docker health check before permitting other containers to start").
//!
//! The table spans what the rule generalises over: the FORM the command takes (`CMD-SHELL`
//! and `CMD`, the two forms `HealthCheck$command` documents), the outcome it produces
//! (passing, failing), how many retries it takes to be believed, whether a grace period is
//! declared, and whether a health check is declared at all. One recording of one passing
//! check is one row of it.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, so a status that is not computed from persisted state cannot pass.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{cluster_with_capacity, list, ok, service, string};

/// The cluster every task below runs in.
const CLUSTER: &str = "healthchecks";

/// The interval every row's check runs at: the smallest `HealthCheck$interval` the member
/// documents ("You may specify between 5 and 300 seconds"), so a row's verdict lands inside
/// a poll budget without naming a value AWS refuses.
const INTERVAL_SECONDS: i64 = 5;

/// The `HealthCheck$startPeriod` the grace-period case declares, in the same documented
/// range ("You can specify between 0 and 300 seconds") and FOUR intervals long, so a failing
/// execution lands inside it well before it expires.
const START_PERIOD_SECONDS: i64 = INTERVAL_SECONDS * 4;

/// How often a poll re-reads the task it is waiting on.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How long a poll keeps trying: enough interval spans for the deepest row's verdict —
/// the grace period, then `retries` failing executions, then the dependent container's own
/// first check — with generous margin for a loaded test machine, since what is under test is
/// WHICH status is reached and not how fast.
const POLL_BUDGET: Duration = Duration::from_secs(120);

/// How many times an INVARIANT — something the rule says does not change — is looked at
/// before the case accepts it. Each look is a fresh read through the front door, which
/// materialises whatever the clock owes, so the invariant is checked against progress rather
/// than against a wait.
const LOOKS_FOR_AN_INVARIANT: usize = 5;

/// The command a container runs so that it stays up for as long as a row observes it: a
/// container's own program is what keeps its task running, and a task whose essential
/// container has exited is stopped ("The task stopped because an essential container
/// exited").
fn stays_up() -> Value {
    json!(["sh", "-c", "sleep 300"])
}

/// A container definition named `name` that stays up, carrying `health_check` when one is
/// given.
fn container_with(name: &str, health_check: Value) -> Value {
    let mut container = json!({
        "name": name,
        "image": "public.ecr.aws/amazonlinux/amazonlinux:latest",
        "memory": 128,
        "command": stays_up(),
    });
    if !health_check.is_null() {
        container["healthCheck"] = health_check;
    }
    container
}

/// One `healthCheck` object: the command array, the interval, how many retries a failure
/// takes to be believed, and the grace period when the row declares one.
fn health_check(command: Value, retries: i64, start_period: Option<i64>) -> Value {
    let mut check = json!({
        "command": command,
        "interval": INTERVAL_SECONDS,
        "timeout": 2,
        "retries": retries,
    });
    if let Some(seconds) = start_period {
        check["startPeriod"] = json!(seconds);
    }
    check
}

/// Register one revision of `family` with `containers` and run one task of it, answering the
/// task's ARN.
async fn run_one(
    router: &MultiServiceRouter,
    family: &str,
    containers: Value,
    case: &str,
) -> String {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({ "family": family, "containerDefinitions": containers }),
        case,
    )
    .await;
    let run = ok(
        router,
        "RunTask",
        json!({ "cluster": CLUSTER, "taskDefinition": family, "count": 1 }),
        case,
    )
    .await;
    let tasks = list(&run, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: RunTask must place the one task it was asked for, got {run}"
    );
    string(&tasks[0], "taskArn", case)
}

/// The task as `DescribeTasks` reports it now.
async fn describe(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Value {
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    let tasks = list(&described, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: the task must be describable, got {described}"
    );
    tasks[0].clone()
}

/// The `healthStatus` the container called `name` reports in `task`.
fn container_health(task: &Value, name: &str, case: &str) -> String {
    list(task, "containers", case)
        .into_iter()
        .find(|container| container["name"].as_str() == Some(name))
        .map(|container| {
            container["healthStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no container {name:?} in {task}"))
}

/// The `lastStatus` the container called `name` reports in `task`.
fn container_status(task: &Value, name: &str, case: &str) -> String {
    list(task, "containers", case)
        .into_iter()
        .find(|container| container["name"].as_str() == Some(name))
        .map(|container| {
            container["lastStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no container {name:?} in {task}"))
}

/// Wait until `reached` holds of the described task, answering the task as it was then — and
/// failing with the last reading when the budget runs out.
async fn poll_until(
    router: &MultiServiceRouter,
    task_arn: &str,
    case: &str,
    wanted: &str,
    reached: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let task = describe(router, task_arn, case).await;
        if reached(&task) || Instant::now() >= deadline {
            assert!(
                reached(&task),
                "case {case:?}: expected {wanted}, got {task}"
            );
            return task;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the health-check table.
struct Row {
    case: &'static str,
    /// The task-definition family, and so the container the row's one container carries.
    family: &'static str,
    /// The `healthCheck` the container declares, or `Value::Null` for a container with none.
    check: Value,
    /// The `healthStatus` the container — and so the task, whose only container is
    /// essential — must reach.
    expected: &'static str,
}

/// The rows: every form `HealthCheck$command` documents, each with a passing and a failing
/// outcome, plus the container that declares no check at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a CMD-SHELL probe that exits zero",
            family: "healthshellpass",
            check: health_check(json!(["CMD-SHELL", "exit 0"]), 1, None),
            expected: "HEALTHY",
        },
        Row {
            case: "a CMD-SHELL probe that exits non-zero, believed after one try",
            family: "healthshellfail",
            check: health_check(json!(["CMD-SHELL", "exit 1"]), 1, None),
            expected: "UNHEALTHY",
        },
        Row {
            case: "a CMD probe that exits zero",
            family: "healthexecpass",
            check: health_check(json!(["CMD", "true"]), 2, None),
            expected: "HEALTHY",
        },
        Row {
            case: "a CMD probe that exits non-zero, believed after two tries",
            family: "healthexecfail",
            check: health_check(json!(["CMD", "false"]), 2, None),
            expected: "UNHEALTHY",
        },
        Row {
            case: "a probe whose command this container cannot run at all",
            family: "healthnocommand",
            check: health_check(json!(["CMD-SHELL", "/usr/bin/nothing-here"]), 1, None),
            expected: "UNHEALTHY",
        },
        Row {
            case: "a container that declares no health check",
            family: "healthnone",
            check: Value::Null,
            expected: "UNKNOWN",
        },
    ]
}

#[tokio::test]
async fn a_declared_health_check_runs_and_decides_the_container_and_task_health_status() {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "the cluster every row's task runs in").await;

    // Every row's task is placed up front, so the executions of all of them fall due at
    // once rather than one row's budget after another's.
    let rows = rows();
    let mut placed: Vec<String> = Vec::new();
    for row in &rows {
        let containers = json!([container_with("app", row.check.clone())]);
        placed.push(run_one(&router, row.family, containers, row.case).await);
    }

    for (row, task_arn) in rows.iter().zip(&placed) {
        let case = row.case;
        let expected = row.expected;
        // A container with no check never leaves UNKNOWN, so that row is read once the task
        // is running rather than waited on: there is no transition to wait for.
        let task = poll_until(
            &router,
            task_arn,
            case,
            "the declared check to decide the container's health",
            |task| {
                task["lastStatus"].as_str() == Some("RUNNING")
                    && container_health(task, "app", case) == expected
            },
        )
        .await;
        assert_eq!(
            container_health(&task, "app", case),
            expected,
            "case {case:?}: expected the container to report {expected}, got {task}"
        );
        // "If all essential containers in the task are reporting as HEALTHY, the task status
        // also reports as HEALTHY": this task's one container is essential by default.
        assert_eq!(
            task["healthStatus"].as_str(),
            Some(expected),
            "case {case:?}: the task reports the reading of its essential container, got \
             {task}"
        );
        // The probe reports on the container; it does not stop the task it runs in.
        assert_eq!(
            task["lastStatus"].as_str(),
            Some("RUNNING"),
            "case {case:?}: a health check does not move the task's own status, got {task}"
        );
    }
}

#[tokio::test]
async fn a_start_period_suppresses_unhealthy_only_until_it_expires() {
    let router = service();
    let case = "a failing probe under a grace period";
    cluster_with_capacity(&router, CLUSTER, case).await;

    // The SUBJECT declares a grace period several intervals long; the TWIN beside it runs
    // the SAME failing probe with the same single retry and NO grace period. The twin is
    // what makes the suppression observable without timing anything: the instant the twin
    // reports UNHEALTHY, a failing execution of that very check has been counted, so a
    // subject that is still "being evaluated" is one whose failures were suppressed — and an
    // implementation that ignored `startPeriod` would have flipped both at once.
    let subject = json!([container_with(
        "app",
        health_check(json!(["CMD-SHELL", "exit 1"]), 1, Some(START_PERIOD_SECONDS)),
    )]);
    let twin = json!([container_with(
        "app",
        health_check(json!(["CMD-SHELL", "exit 1"]), 1, None),
    )]);
    let subject_arn = run_one(&router, "healthgrace", subject, case).await;
    let twin_arn = run_one(&router, "healthnograce", twin, case).await;

    // 1. The twin's first failing execution counts at once, so it reaches UNHEALTHY.
    poll_until(
        &router,
        &twin_arn,
        "the twin with no grace period",
        "the failing probe to be counted at once without a grace period",
        |task| container_health(task, "app", "the twin with no grace period") == "UNHEALTHY",
    )
    .await;

    // 2. With the twin already unhealthy, the subject is still being evaluated: its own
    //    failures fall inside the period it asked for and count toward nothing. Asserted
    //    over repeated reads, each of which materialises whatever the clock owes.
    for look in 0..LOOKS_FOR_AN_INVARIANT {
        let task = describe(&router, &subject_arn, case).await;
        assert_eq!(
            container_health(&task, "app", case),
            "UNKNOWN",
            "case {case:?}: a failure inside the grace period counts toward nothing, so the \
             container is still being evaluated (look {look}), got {task}"
        );
        assert_eq!(
            task["healthStatus"].as_str(),
            Some("UNKNOWN"),
            "case {case:?}: the task reports what its essential container reports, got {task}"
        );
    }

    // 3. And the suppression ENDS with the period: once it has run out the failures count
    //    and the subject reaches UNHEALTHY too, with no further call from anybody.
    poll_until(
        &router,
        &subject_arn,
        case,
        "the grace period to expire and let the failures count",
        |task| container_health(task, "app", case) == "UNHEALTHY",
    )
    .await;
}

#[tokio::test]
async fn a_container_that_depends_on_a_healthy_container_waits_for_the_check_to_pass() {
    let router = service();
    let case = "a dependsOn HEALTHY gate";
    cluster_with_capacity(&router, CLUSTER, case).await;

    // `db` passes its check only after its own grace period, so the gate is observable: the
    // `app` container may not start until `db` is HEALTHY, and `other`, which depends on
    // nothing, starts with the task.
    let mut db = container_with("db", health_check(json!(["CMD-SHELL", "exit 0"]), 1, None));
    db["essential"] = json!(true);
    let mut app = container_with("app", Value::Null);
    app["dependsOn"] = json!([{ "containerName": "db", "condition": "HEALTHY" }]);
    let other = container_with("other", Value::Null);
    let task_arn = run_one(
        &router,
        "healthdependson",
        json!([db, app, other]),
        case,
    )
    .await;

    // Before the first execution of db's check there is nothing to open the gate, so app is
    // still PENDING while the task itself is RUNNING and the independent container is too.
    let running = poll_until(&router, &task_arn, case, "the task to be running", |task| {
        task["lastStatus"].as_str() == Some("RUNNING")
    })
    .await;
    assert_eq!(
        container_status(&running, "app", case),
        "PENDING",
        "case {case:?}: a container that waits on a HEALTHY condition does not start with \
         its task, got {running}"
    );
    assert_eq!(
        container_status(&running, "other", case),
        "RUNNING",
        "case {case:?}: a container that waits on nothing starts with its task, got {running}"
    );
    // The task's own reading is UNKNOWN while an essential container has produced no result.
    assert_eq!(
        running["healthStatus"].as_str(),
        Some("UNKNOWN"),
        "case {case:?}: a task whose essential container is still being evaluated reports \
         UNKNOWN, got {running}"
    );

    // Once db's check passes, the gate opens and app starts.
    let started = poll_until(
        &router,
        &task_arn,
        case,
        "the dependent container to start once its dependency is healthy",
        |task| container_status(task, "app", case) == "RUNNING",
    )
    .await;
    assert_eq!(
        container_health(&started, "db", case),
        "HEALTHY",
        "case {case:?}: the gate opens on the dependency's own HEALTHY reading, got {started}"
    );
}
