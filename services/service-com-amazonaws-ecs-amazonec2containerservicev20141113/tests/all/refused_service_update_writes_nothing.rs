// SPDX-License-Identifier: CC0-1.0

//! A refused `UpdateService` leaves a RUNNING service exactly as it was, and the next
//! update the operation accepts is the one that takes effect.
//!
//! The rule under test, over the whole class rather than one request:
//!
//! > A service "runs and maintains your desired number of tasks from a specified task
//! > definition" (`CreateService`), so `Service$runningCount` ("The number of tasks in the
//! > cluster that are in the RUNNING state") converges on the `Service$desiredCount` the
//! > caller asked for. An `UpdateService` the operation REFUSES writes nothing of itself:
//! > whatever the refusal's cause and whatever else the request carried, the service goes
//! > on wanting the count it already held, goes on holding the very tasks it was already
//! > running, and keeps every other member it stored — the count a refused request named
//! > is not applied just because that member on its own would have been acceptable. The
//! > next update the operation ACCEPTS is the one that moves the count, after which the
//! > scheduler converges there; so the Container Insights `RunningTaskCount` series of a
//! > cluster `containerInsights` is on for (`Unit` `Count`, dimensioned `ClusterName` and
//! > `ServiceName`) only ever reads a count that service actually held — never below zero
//! > and never above the largest count it was ever asked for.
//!
//! `UpdateService`'s own doc is the update half — "You can add to or subtract from the
//! number of instantiations of a task definition in a service by specifying the cluster
//! that the service is running in and a new desiredCount parameter" — and the refusals are
//! the ones its model declares: `InvalidParameterException` ("The specified parameter isn't
//! valid") and `ClientException` ("it might be specifying an identifier that isn't valid"),
//! both client errors at HTTP 400.
//!
//! The rows span what the rule generalises over: WHY the update is refused and WHAT ELSE
//! the refused request carried — a count below its bound at three distances from it, a
//! second member below its own bound, and an identifier that resolves to no revision, the
//! last two beside a `desiredCount` this operation would otherwise have taken. One run's
//! `desiredCount=-1` is a single row of that table.
//!
//! Nothing here reaches into a state struct: the cluster, the revision and the services are
//! created through Amazon ECS's own front door, the counts and the task set are read back
//! through `DescribeServices`, `ListTasks` and `DescribeTasks`, and the series through
//! CloudWatch's own `GetMetricStatistics`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, cloudwatch, count, list, ok, poll_until, refusal,
    seed_awsvpc_subnets, string, with_members,
};

/// The cluster every row's service runs in, and the task-definition family they all run.
const CLUSTER: &str = "refused-update";
const FAMILY: &str = "refusedupdate";

/// A family name NOTHING registers, which one row's refused update points at.
const ABSENT_FAMILY: &str = "refusedupdateabsent";

/// The image every container names and the program it runs: a wait with nothing else to do,
/// which is how the container of a maintained service stays up for as long as the rows read.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";
const STAYS_UP: &str = "sleep 3600";

/// The count every row's service is created with, and the count the ACCEPTED update raises
/// it to — so the series has a count below its ceiling as well as the ceiling itself.
const CREATED_COUNT: i64 = 1;
const RAISED_COUNT: i64 = 2;

/// The `healthCheckGracePeriodSeconds` every row's service is created with: a period of its
/// own rather than the member's documented default of zero ("If you do not specify a health
/// check grace period value, the default value of 0 is used"), so the row whose refused
/// update names a period below the bound can be seen to have changed nothing on that member
/// too.
const CREATED_GRACE_PERIOD: i64 = 120;

/// The namespace Amazon ECS publishes Container Insights metrics in, the service-level count
/// of running tasks, the dimensions it carries and the unit it is published in — spelled out
/// here rather than imported, so a wrong constant in the implementation cannot make this test
/// pass.
///
/// SOURCE: the CloudWatch User Guide's "Amazon ECS Container Insights metrics" table, the
/// `RunningTaskCount` row, whose dimension set is `ClusterName, ServiceName`.
const NAMESPACE: &str = "ECS/ContainerInsights";
const RUNNING_TASK_COUNT: &str = "RunningTaskCount";
const CLUSTER_DIMENSION: &str = "ClusterName";
const SERVICE_DIMENSION: &str = "ServiceName";
const COUNT_UNIT: &str = "Count";

/// The resolution those series are published at, in seconds.
const PERIOD_SECONDS: i64 = 60;

/// The two refusal classes this operation's model declares for the causes below, both client
/// errors at HTTP 400.
const INVALID_PARAMETER: &str = "InvalidParameterException";
const CLIENT_EXCEPTION: &str = "ClientException";

/// The sentence an UPDATE of a service carries for a `desiredCount` below zero (the service's
/// own `TASK_COUNT_SUBJECT` under `desired_count_negative_message`): fixed text naming what
/// the count counts and the bound it crossed, and nothing the request carried.
const TASK_COUNT_NEGATIVE: &str = "Task count can not be lesser than 0";

/// The sentence a `healthCheckGracePeriodSeconds` below zero carries (the service's own
/// `HEALTH_CHECK_GRACE_PERIOD_NEGATIVE_MESSAGE`).
const GRACE_PERIOD_NEGATIVE: &str = "healthCheckGracePeriod must be at least 0";

/// The sentence a `taskDefinition` that resolves to no stored revision carries on a call that
/// would put that revision to work (the service's own
/// `TASK_DEFINITION_NOT_FOUND_TO_RUN_MESSAGE`).
const TASK_DEFINITION_NOT_FOUND: &str = "TaskDefinition not found.";

/// The members of a service this suite asserts a refused update left alone: the count it
/// wants, the counts it holds, the revision it runs and the grace period it waits out — the
/// members the refused requests below name, plus the counts the scheduler keeps.
const UNCHANGED_MEMBERS: [&str; 5] = [
    "desiredCount",
    "runningCount",
    "pendingCount",
    "taskDefinition",
    "healthCheckGracePeriodSeconds",
];

/// One row: a service of the one cluster, the update that is refused on it, and the refusal
/// that update earns.
#[derive(Debug)]
struct Row {
    case: &'static str,
    service: &'static str,
    /// The members the refused update carries beside the cluster and the service.
    refused: Value,
    code: &'static str,
    message: &'static str,
}

/// The table: one row per CAUSE of refusal, the count-below-its-bound cause at three
/// distances from the bound, and the last two rows carrying a `desiredCount` the operation
/// would have taken on its own.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a count one below its bound",
            service: "refused-below-one",
            refused: json!({ "desiredCount": -1 }),
            code: INVALID_PARAMETER,
            message: TASK_COUNT_NEGATIVE,
        },
        Row {
            case: "a count far below its bound",
            service: "refused-below-many",
            refused: json!({ "desiredCount": -7 }),
            code: INVALID_PARAMETER,
            message: TASK_COUNT_NEGATIVE,
        },
        Row {
            case: "the smallest count the member carries at all",
            service: "refused-below-floor",
            refused: json!({ "desiredCount": i32::MIN }),
            code: INVALID_PARAMETER,
            message: TASK_COUNT_NEGATIVE,
        },
        Row {
            case: "a grace period below its own bound, beside a count this operation takes",
            service: "refused-grace",
            refused: json!({
                "desiredCount": RAISED_COUNT,
                "healthCheckGracePeriodSeconds": -1,
            }),
            code: INVALID_PARAMETER,
            message: GRACE_PERIOD_NEGATIVE,
        },
        Row {
            case: "a revision nothing registered, beside a count this operation takes",
            service: "refused-revision",
            refused: json!({
                "desiredCount": RAISED_COUNT,
                "taskDefinition": ABSENT_FAMILY,
            }),
            code: CLIENT_EXCEPTION,
            message: TASK_DEFINITION_NOT_FOUND,
        },
    ]
}

/// An emulator with Amazon ECS and CloudWatch — which owns the metric store the Container
/// Insights series are published into — over ONE shared engine, holding the Amazon VPC subnet
/// every `awsvpc` placement below names.
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
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One service as `DescribeServices` reports it, with no failure reported for the name.
async fn describe(router: &MultiServiceRouter, service: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": CLUSTER, "services": [service] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: DescribeServices must report no failure, got {body}"
    );
    list(&body, "services", case)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the service back, got {body}"))
}

/// That same service once it holds exactly `wanted` tasks with nothing still coming up,
/// waited for rather than slept for: where the task set is left is the clock's.
async fn converged(router: &MultiServiceRouter, row: &Row, wanted: i64) -> Value {
    poll_until(
        row.case,
        &format!("runningCount {wanted} with pendingCount 0"),
        || describe(router, row.service, row.case),
        |service| {
            service["runningCount"].as_i64() == Some(wanted)
                && service["pendingCount"].as_i64().unwrap_or(0) == 0
        },
    )
    .await
}

/// The ARNs of the tasks of one row's service that are wanted RUNNING, sorted, with every one
/// of them asserted to BE running.
async fn running_tasks(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let case = row.case;
    // "The default status filter is RUNNING, which shows tasks that Amazon ECS has set the
    // desired status to RUNNING."
    let listed = ok(
        router,
        "ListTasks",
        json!({
            "cluster": CLUSTER,
            "serviceName": row.service,
            "desiredStatus": "RUNNING",
        }),
        case,
    )
    .await;
    let arns: Vec<Value> = list(&listed, "taskArns", case);
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
    let mut held = Vec::new();
    for task in list(&described, "tasks", case) {
        assert_eq!(
            task["lastStatus"].as_str(),
            Some("RUNNING"),
            "case {case:?}: a task counted as running reports RUNNING, got {task}"
        );
        assert_eq!(
            task["launchType"].as_str(),
            Some("FARGATE"),
            "case {case:?}: the task runs the launch type the service was created with, got \
             {task}"
        );
        held.push(string(&task, "taskArn", case));
    }
    held.sort();
    held
}

/// Epoch seconds, for the window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Every `RunningTaskCount` datapoint CloudWatch holds for one row's service, read the way a
/// client reads it: the cluster-and-service dimension pair, at the one-minute period those
/// series are published at.
async fn datapoints(router: &MultiServiceRouter, service: &str) -> Value {
    let now = epoch_seconds();
    let stats = cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": RUNNING_TASK_COUNT,
            "Dimensions": [
                { "Name": CLUSTER_DIMENSION, "Value": CLUSTER },
                { "Name": SERVICE_DIMENSION, "Value": service },
            ],
            "StartTime": now - 1800.0,
            "EndTime": now + 60.0,
            "Period": PERIOD_SECONDS,
            "Statistics": ["Maximum", "Minimum"],
        }),
    )
    .await;
    json!({ "Datapoints": stats["Datapoints"] })
}

/// The text spellings of the values one refused request carried, so the sentence it earns can
/// be asserted to carry none of them.
fn carried_values(row: &Row) -> Vec<String> {
    let mut carried = vec![row.service.to_string()];
    if let Value::Object(members) = &row.refused {
        for (member, value) in members {
            carried.push(member.clone());
            match value {
                Value::String(text) => carried.push(text.clone()),
                other => carried.push(other.to_string()),
            }
        }
    }
    carried
}

/// RULE: a refused update writes nothing, and the accepted one that follows it takes effect.
#[tokio::test]
async fn a_refused_update_leaves_the_service_and_its_tasks_as_they_were() {
    let router = emulator();
    let rows = rows();

    ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": CLUSTER,
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "the cluster every row's service runs in",
    )
    .await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "essential": true,
                "command": ["sh", "-c", STAYS_UP],
            }],
        }),
        "the revision every row's service runs",
    )
    .await;

    // Every row's service is created up front, so the clock-owed work of all of them falls
    // due at once rather than one row's budget after another's.
    for row in &rows {
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": row.service,
                "taskDefinition": FAMILY,
                "desiredCount": CREATED_COUNT,
                "launchType": "FARGATE",
                "networkConfiguration": awsvpc_network_configuration(),
                "healthCheckGracePeriodSeconds": CREATED_GRACE_PERIOD,
            }),
            row.case,
        )
        .await;
        assert_eq!(
            created["service"]["desiredCount"].as_i64(),
            Some(CREATED_COUNT),
            "case {:?}: the create keeps the count the caller asked for, got {created}",
            row.case
        );
        assert_eq!(
            created["service"]["healthCheckGracePeriodSeconds"].as_i64(),
            Some(CREATED_GRACE_PERIOD),
            "case {:?}: the create keeps the grace period the caller asked for, got {created}",
            row.case
        );
    }

    // What each service is BEFORE a refusal reaches it: converged on the count it was created
    // with, holding one task per task it wants.
    let mut before: Vec<(Value, Vec<String>)> = Vec::new();
    for row in &rows {
        let case = row.case;
        let service = converged(&router, row, CREATED_COUNT).await;
        let tasks = running_tasks(&router, row).await;
        assert_eq!(
            tasks.len() as i64,
            CREATED_COUNT,
            "case {case:?}: the service holds one task per task it wants, got {tasks:?}"
        );
        before.push((service, tasks));
    }

    // The refusal itself, and the state it must not have touched.
    for (row, (was, ran)) in rows.iter().zip(&before) {
        let case = row.case;
        let mut request = json!({ "cluster": CLUSTER, "service": row.service });
        with_members(&mut request, row.refused.clone());
        let (code, message) = refusal(&router, "UpdateService", request, case).await;
        assert_eq!(
            code, row.code,
            "case {case:?}: expected a {} from UpdateService, got {code:?} with {message:?}",
            row.code
        );
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the fixed sentence {:?}, got {message:?}",
            row.message
        );
        for carried in carried_values(row) {
            assert!(
                !message.contains(&carried),
                "case {case:?}: the sentence is fixed text and must not carry {carried:?}, \
                 got {message:?}"
            );
        }

        // Nothing of the refused update was written: the service still wants the count it
        // held, still holds the tasks it was running, and keeps the revision and the grace
        // period it stored — the count a refused request named is not applied on its own.
        let now = describe(&router, row.service, case).await;
        for member in UNCHANGED_MEMBERS {
            assert_eq!(
                now[member], was[member],
                "case {case:?}: a refused update must leave {member} as it was, got {now}"
            );
        }
        assert_eq!(
            count(&now, "desiredCount", case),
            CREATED_COUNT,
            "case {case:?}: the refused update must not move the count, got {now}"
        );
        assert_eq!(
            count(&now, "healthCheckGracePeriodSeconds", case),
            CREATED_GRACE_PERIOD,
            "case {case:?}: the refused update must not move the grace period, got {now}"
        );
        assert_eq!(
            &running_tasks(&router, row).await,
            ran,
            "case {case:?}: the very tasks it was running are the ones still running"
        );
    }

    // And the next update the operation ACCEPTS is the one that takes effect.
    for row in &rows {
        let updated = ok(
            &router,
            "UpdateService",
            json!({
                "cluster": CLUSTER,
                "service": row.service,
                "desiredCount": RAISED_COUNT,
            }),
            row.case,
        )
        .await;
        assert_eq!(
            updated["service"]["desiredCount"].as_i64(),
            Some(RAISED_COUNT),
            "case {:?}: the accepted update keeps the count it asked for, got {updated}",
            row.case
        );
    }

    for (row, (_, ran)) in rows.iter().zip(&before) {
        let case = row.case;
        let service = converged(&router, row, RAISED_COUNT).await;
        assert_eq!(
            count(&service, "desiredCount", case),
            RAISED_COUNT,
            "case {case:?}: the converged service wants the raised count, got {service}"
        );
        let holds = running_tasks(&router, row).await;
        assert_eq!(
            holds.len() as i64,
            RAISED_COUNT,
            "case {case:?}: the raised service holds one task per task it wants, got {holds:?}"
        );
        // Raising the count ADDS tasks: the ones the service was already running are still
        // the ones it runs, since `desiredCount` alone starts no new deployment.
        for held in ran {
            assert!(
                holds.contains(held),
                "case {case:?}: expected the task {held} to go on running, got {holds:?}"
            );
        }
    }

    // The series of each service reaches the count it converged on, and never reads a count
    // that service did not hold.
    for row in &rows {
        let case = row.case;
        let reached = poll_until(
            case,
            &format!("a {RUNNING_TASK_COUNT} datapoint reading {RAISED_COUNT}"),
            || datapoints(&router, row.service),
            |body| {
                body["Datapoints"]
                    .as_array()
                    .is_some_and(|points| {
                        points
                            .iter()
                            .any(|point| point["Maximum"].as_f64() == Some(RAISED_COUNT as f64))
                    })
            },
        )
        .await;
        for point in list(&reached, "Datapoints", case) {
            assert_eq!(
                point["Unit"],
                json!(COUNT_UNIT),
                "case {case:?}: expected {RUNNING_TASK_COUNT} in {COUNT_UNIT}, got {point}"
            );
            for statistic in ["Maximum", "Minimum"] {
                let read = point[statistic].as_f64().unwrap_or(f64::NAN);
                assert!(
                    (0.0..=RAISED_COUNT as f64).contains(&read),
                    "case {case:?}: a {RUNNING_TASK_COUNT} {statistic} reads a count the \
                     service held, between 0 and {RAISED_COUNT}, got {point}"
                );
            }
        }
    }
}
