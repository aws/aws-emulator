// SPDX-License-Identifier: CC0-1.0

//! Task SCALE-IN PROTECTION and the identifiers a task-protection call carries, driven end
//! to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, each over the whole class rather than one request:
//!
//! > 1. A task whose caller has turned scale-in protection on is NOT selected when the
//! >    scheduler reduces a service's task set: `UpdateTaskProtection` "protects an Amazon
//! >    ECS task from being terminated by scale-in events from Service Auto Scaling or
//! >    deployments", so an unprotected task is stopped in its place — and when the service
//! >    holds no unprotected task to stop, the reduction waits until protection is taken off
//! >    (or lapses) rather than killing a protected one.
//! > 2. A request that carries a task ARN whose cluster differs from the request's own
//! >    `cluster` names two scopes at once, which is a bad IDENTIFIER and not a resource
//! >    that could not be found: both `GetTaskProtection` and `UpdateTaskProtection` refuse
//! >    it with `InvalidParameterException` and `Invalid identifier: cluster identifiers
//! >    mismatch`, and they do so before either identifier is RESOLVED — so the answer is
//! >    the same whether the cluster the request names exists or not. A request whose
//! >    identifiers agree is resolved, and a cluster this account does not hold is then the
//! >    `ClusterNotFoundException` that condition states.
//!
//! The first table spans WHICH of a service's tasks is protected (the one the scheduler
//! would otherwise pick, one it would not, and all of them) and the second spans the two
//! operations, both forms of the cluster reference, and whether the named cluster exists.
//!
//! Nothing here reaches into the state struct: every value asserted is read back through
//! `DescribeTasks`, `ListTasks` and `GetTaskProtection`.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, list, ok, refusal, register_definition, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// The cluster every service below runs in, and a second cluster that EXISTS so a
/// cross-cluster identifier can be weighed against a cluster that resolves.
const CLUSTER: &str = "protectscale";
const OTHER_CLUSTER: &str = "protectelsewhere";

/// A cluster name this account holds nothing under, for the rows whose cluster does not
/// resolve at all.
const MISSING_CLUSTER: &str = "protectnosuchcluster";

/// The task-definition family every service below runs.
const FAMILY: &str = "protectfamily";

/// How often a poll re-reads the surface it is waiting on, and how long it keeps trying:
/// the task transitions a scale-in takes, with generous margin for a loaded test machine.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLL_BUDGET: Duration = Duration::from_secs(30);

/// AWS's message for a request whose two identifiers name different clusters.
const IDENTIFIERS_MISMATCH: &str = "Invalid identifier: cluster identifiers mismatch";

/// The ARNs of the service's tasks that are still wanted running, oldest first — a task ARN
/// ends with the id it was minted under, so sorting them orders the task set by age.
async fn live_task_arns(router: &MultiServiceRouter, service_name: &str, case: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListTasks",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "desiredStatus": "RUNNING",
        }),
        case,
    )
    .await;
    let mut arns: Vec<String> = list(&listed, "taskArns", case)
        .into_iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect();
    arns.sort();
    arns
}

/// The `lastStatus` of every task in `arns`, in the order given.
async fn statuses(router: &MultiServiceRouter, arns: &[String], case: &str) -> Vec<String> {
    if arns.is_empty() {
        return Vec::new();
    }
    let described = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": arns }),
        case,
    )
    .await;
    let tasks = list(&described, "tasks", case);
    arns.iter()
        .map(|arn| {
            tasks
                .iter()
                .find(|task| task["taskArn"].as_str() == Some(arn.as_str()))
                .and_then(|task| task["lastStatus"].as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create one service of `desired` tasks and wait until that many are running, answering
/// their ARNs oldest first.
async fn service_running(
    router: &MultiServiceRouter,
    service_name: &str,
    desired: i64,
    case: &str,
) -> Vec<String> {
    ok(
        router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": service_name,
            "taskDefinition": FAMILY,
            "desiredCount": desired,
            "launchType": "EC2",
        }),
        case,
    )
    .await;
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let arns = live_task_arns(router, service_name, case).await;
        let held = statuses(router, &arns, case).await;
        let running = held.iter().filter(|status| *status == "RUNNING").count() as i64;
        if running == desired || Instant::now() >= deadline {
            assert_eq!(
                running, desired,
                "case {case:?}: expected {desired} running task(s), got {held:?}"
            );
            return arns;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Turn protection on or off for `tasks` of `service_name`'s cluster, asserting the call
/// reports what it stored.
async fn protect(
    router: &MultiServiceRouter,
    tasks: &[String],
    enabled: bool,
    case: &str,
) {
    let updated = ok(
        router,
        "UpdateTaskProtection",
        json!({
            "cluster": CLUSTER,
            "tasks": tasks,
            "protectionEnabled": enabled,
        }),
        case,
    )
    .await;
    let reported = list(&updated, "protectedTasks", case);
    assert_eq!(
        reported.len(),
        tasks.len(),
        "case {case:?}: every task named must be answered, got {updated}"
    );
    for task in &reported {
        assert_eq!(
            task["protectionEnabled"].as_bool(),
            Some(enabled),
            "case {case:?}: the protection the call stored is what it reports, got {updated}"
        );
    }
}

/// Reduce the service's desired count, asserting the call keeps the count it was given.
async fn scale_to(router: &MultiServiceRouter, service_name: &str, desired: i64, case: &str) {
    let updated = ok(
        router,
        "UpdateService",
        json!({
            "cluster": CLUSTER,
            "service": service_name,
            "desiredCount": desired,
        }),
        case,
    )
    .await;
    assert_eq!(
        updated["service"]["desiredCount"].as_i64(),
        Some(desired),
        "case {case:?}: the update keeps the count the caller asked for, got {updated}"
    );
}

/// Wait until the tasks the service still holds running are exactly `wanted`, answering what
/// it held at the end.
async fn settled_on(
    router: &MultiServiceRouter,
    arns: &[String],
    wanted: &[String],
    case: &str,
) -> Vec<String> {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let held = statuses(router, arns, case).await;
        let running: Vec<String> = arns
            .iter()
            .zip(&held)
            .filter(|(_, status)| *status == "RUNNING")
            .map(|(arn, _)| arn.clone())
            .collect();
        if running == wanted || Instant::now() >= deadline {
            assert_eq!(
                running, wanted,
                "case {case:?}: expected exactly the unprotected tasks to be stopped, got \
                 {held:?}"
            );
            return held;
        }
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the protection table: how many tasks the service runs, which of them (by age
/// index) are protected, how far it is scaled in, and which of them must survive.
struct Row {
    case: &'static str,
    service_name: &'static str,
    desired: i64,
    protected: &'static [usize],
    scale_to: i64,
    /// The indexes of the tasks that must still be running when the scale-in has settled.
    survivors: &'static [usize],
}

const ROWS: [Row; 3] = [
    Row {
        // Scaling in stops the NEWEST task of the deployment first, so protecting that one
        // is the row where the protection decides the selection.
        case: "the task the scheduler would otherwise stop is protected",
        service_name: "protectnewest",
        desired: 3,
        protected: &[2],
        scale_to: 2,
        survivors: &[0, 2],
    },
    Row {
        case: "a task the scheduler would not have stopped is protected",
        service_name: "protectoldest",
        desired: 3,
        protected: &[0],
        scale_to: 2,
        survivors: &[0, 1],
    },
    Row {
        // With every task protected there is no unprotected task to stop in their place, so
        // the reduction waits and the service keeps running more tasks than it wants.
        case: "every task is protected, so the reduction waits",
        service_name: "protectall",
        desired: 2,
        protected: &[0, 1],
        scale_to: 1,
        survivors: &[0, 1],
    },
];

#[tokio::test]
async fn a_protected_task_is_never_the_one_a_scale_in_stops() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    register_definition(&router, FAMILY, case).await;

    for row in &ROWS {
        let case = row.case;
        let arns = service_running(&router, row.service_name, row.desired, case).await;
        let protected: Vec<String> = row
            .protected
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        protect(&router, &protected, true, case).await;
        scale_to(&router, row.service_name, row.scale_to, case).await;
        let survivors: Vec<String> = row
            .survivors
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        settled_on(&router, &arns, &survivors, case).await;

        // And the protection a caller takes OFF makes the task eligible again: the
        // reduction the scheduler could not make before is made without a further call.
        if row.survivors.len() as i64 > row.scale_to {
            let case = "protection taken off lets the waiting reduction through";
            let released: Vec<String> = row
                .survivors
                .iter()
                .rev()
                .take(row.survivors.len() - row.scale_to as usize)
                .map(|index| arns[*index].clone())
                .collect();
            protect(&router, &released, false, case).await;
            let kept: Vec<String> = row
                .survivors
                .iter()
                .filter(|index| !released.contains(&arns[**index]))
                .map(|index| arns[*index].clone())
                .collect();
            settled_on(&router, &arns, &kept, case).await;
        }
    }
}

/// One row of the identifier table: the operation, the `cluster` the request names, and
/// whether the task is named by an ARN of ANOTHER cluster.
struct IdentifierRow {
    case: &'static str,
    operation: &'static str,
    cluster: &'static str,
    /// Whether the task entry is an ARN scoped to [`CLUSTER`] (a cluster that is not the one
    /// the request names), rather than the bare task id.
    foreign_arn: bool,
    expected_code: &'static str,
    expected_message: &'static str,
}

const IDENTIFIER_ROWS: [IdentifierRow; 6] = [
    IdentifierRow {
        case: "GetTaskProtection against another EXISTING cluster",
        operation: "GetTaskProtection",
        cluster: OTHER_CLUSTER,
        foreign_arn: true,
        expected_code: "InvalidParameterException",
        expected_message: IDENTIFIERS_MISMATCH,
    },
    IdentifierRow {
        case: "UpdateTaskProtection against another EXISTING cluster",
        operation: "UpdateTaskProtection",
        cluster: OTHER_CLUSTER,
        foreign_arn: true,
        expected_code: "InvalidParameterException",
        expected_message: IDENTIFIERS_MISMATCH,
    },
    IdentifierRow {
        // The identifiers disagree whatever the cluster resolves to, so the answer is the
        // identifier's refusal and not the cluster's.
        case: "GetTaskProtection against a cluster that does not exist",
        operation: "GetTaskProtection",
        cluster: MISSING_CLUSTER,
        foreign_arn: true,
        expected_code: "InvalidParameterException",
        expected_message: IDENTIFIERS_MISMATCH,
    },
    IdentifierRow {
        case: "UpdateTaskProtection against a cluster that does not exist",
        operation: "UpdateTaskProtection",
        cluster: MISSING_CLUSTER,
        foreign_arn: true,
        expected_code: "InvalidParameterException",
        expected_message: IDENTIFIERS_MISMATCH,
    },
    IdentifierRow {
        // Nothing disagrees here — the entry carries no cluster of its own — so the cluster
        // is resolved, and this one does not exist.
        case: "GetTaskProtection by bare task id against a cluster that does not exist",
        operation: "GetTaskProtection",
        cluster: MISSING_CLUSTER,
        foreign_arn: false,
        expected_code: "ClusterNotFoundException",
        expected_message: "Cluster not found.",
    },
    IdentifierRow {
        case: "UpdateTaskProtection by bare task id against a cluster that does not exist",
        operation: "UpdateTaskProtection",
        cluster: MISSING_CLUSTER,
        foreign_arn: false,
        expected_code: "ClusterNotFoundException",
        expected_message: "Cluster not found.",
    },
];

#[tokio::test]
async fn a_task_protection_call_whose_identifiers_disagree_is_refused_for_the_identifier() {
    let router = service();
    let case = "setup";
    cluster_with_capacity(&router, CLUSTER, case).await;
    cluster_with_capacity(&router, OTHER_CLUSTER, case).await;
    register_definition(&router, FAMILY, case).await;
    let arns = service_running(&router, "protectidentity", 1, case).await;
    let task_arn = arns
        .first()
        .cloned()
        .expect("the service runs the one task it was asked for");
    let task_id = task_arn
        .rsplit('/')
        .next()
        .expect("a task ARN ends with the task id")
        .to_string();

    // The call that AGREES with itself is served, so the refusals below are about the
    // identifiers and not about the task.
    let served = ok(
        &router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [task_arn.clone()] }),
        "the cluster that holds the task",
    )
    .await;
    assert_eq!(
        list(&served, "protectedTasks", case).len(),
        1,
        "case {case:?}: the task's own cluster answers for it, got {served}"
    );

    for row in &IDENTIFIER_ROWS {
        let case = row.case;
        let entry = if row.foreign_arn {
            task_arn.clone()
        } else {
            task_id.clone()
        };
        let mut request = json!({ "cluster": row.cluster, "tasks": [entry] });
        if row.operation == "UpdateTaskProtection" {
            request["protectionEnabled"] = json!(true);
        }
        let (code, message) = refusal(&router, row.operation, request, case).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (row.expected_code, row.expected_message),
            "case {case:?}: expected the identifier refusal, got {code} with {message:?}"
        );
    }

    // A task ARN of a cluster that does not exist is read the same way: the identifier is
    // what disagrees with the request's `cluster`, so the refusal names neither cluster.
    let case = "an ARN scoped to a cluster nothing holds";
    let elsewhere =
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task/{MISSING_CLUSTER}/{task_id}");
    let (code, message) = refusal(
        &router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [elsewhere] }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidParameterException", IDENTIFIERS_MISMATCH),
        "case {case:?}: expected the identifier refusal, got {code} with {message:?}"
    );

    // And the protection the served call reported is still readable, so no refusal above
    // wrote anything.
    let read = ok(
        &router,
        "GetTaskProtection",
        json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        case,
    )
    .await;
    let protected = list(&read, "protectedTasks", case);
    assert_eq!(
        protected
            .first()
            .and_then(|task| task["protectionEnabled"].as_bool()),
        Some(false),
        "case {case:?}: a refused update stored nothing, got {read}"
    );
    assert!(
        !string(&protected[0], "taskArn", case).is_empty(),
        "case {case:?}: the reported task names itself, got {read}"
    );
}
