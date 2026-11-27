// SPDX-License-Identifier: CC0-1.0

//! A service's `AWS/ECS` utilization series follows the tasks it is HOLDING: it is published
//! while the service runs one and stops the moment it runs none.
//!
//! RULE: `Service$runningCount` is "The number of tasks in the cluster that are in the RUNNING
//! state", and it is DERIVED from the tasks a service's scheduler holds — a service that
//! "runs and maintains your desired number of tasks from a specified task definition"
//! (`CreateService`) reports that number once its tasks are running, reports fewer as soon as
//! one of them is asked to stop, and reports it again once the scheduler has replaced it. The
//! `AWS/ECS` `CPUUtilization` and `MemoryUtilization` of that service measure the containers of
//! those very tasks, so the series on the `ClusterName` + `ServiceName` pair gains datapoints
//! for exactly as long as the service holds a running task: it stops gaining them when the
//! count falls to zero — however it falls, and without the datapoints already published being
//! lost — and gains them again when the count comes back.
//!
//! The rows differ along what the rule generalises over: HOW the count a service holds changes.
//! It falls to zero because the caller asked for no task (`UpdateService$desiredCount`) or
//! because the service was deleted ("the service status moves from DRAINING to INACTIVE",
//! `DeleteService`); it comes back because the caller asked for a task again, or because the
//! scheduler replaced one a caller stopped ("if any of your tasks fail or stop, the Amazon ECS
//! service scheduler launches another instance of your task definition to replace it"). Every
//! row's service is a Fargate service of its own cluster with `containerInsights` disabled, the
//! shape a client brings up when it wants per-service CPU and memory series, so each row's
//! series is read on its own.
//!
//! Two crates have to agree for this (ECS meters the tasks its services hold, CloudWatch stores
//! every series and answers for it) and the `awsvpc` placements resolve a subnet in a third, so
//! the suite lives here and drives the emulator through the harness of the sibling suites over
//! the same two stores ([`crate::ecs_service_utilization_metrics`],
//! [`crate::ecs_service_utilization_setting_independence`]).
//!
//! Nothing here reaches into a store: the clusters, services and tasks are created, scaled,
//! stopped and deleted through the ECS front door, and every datapoint is read back through
//! CloudWatch's own `GetMetricStatistics`.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// One harness for one rule: the two services mounted on one engine, the calls a client makes,
// and the names and units the series carries.
use crate::ecs_service_utilization_metrics::{
    cloudwatch, ecs, emulator, CLUSTER_DIMENSION, CPU_UTILIZATION, IMAGE, MEMORY_UTILIZATION,
    NAMESPACE, PERCENT, SERVICE_DIMENSION, STAYS_UP, SUBNET,
};

/// The two metrics every row reads: a service publishes both while it holds a running task.
const METRICS: [&str; 2] = [CPU_UTILIZATION, MEMORY_UTILIZATION];

/// How often a poll re-reads what it is waiting on.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long one wait keeps trying before it gives up. Each wait gets this budget of its OWN,
/// never one shared between two: Amazon ECS creates and attaches the interface an `awsvpc` task
/// is allocated one documented step at a time, starts it, and only then has a task to measure,
/// so a correct run is far inside this and a broken one is reported promptly. Nothing here
/// asserts on elapsed time.
const DEADLINE: Duration = Duration::from_secs(120);

/// How many further looks an assertion that a series has STOPPED gaining datapoints takes.
/// Together with [`POLL_INTERVAL`] a window far longer than the spacing a series that is
/// catching up publishes at, so what a row asserts is what held rather than what was read too
/// early.
const QUIET_LOOKS: usize = 20;

/// The largest difference between two readings of one series' observations that is still the
/// same reading: a `SampleCount` counts whole observations, so one more datapoint moves the
/// total by at least one.
const HALF_AN_OBSERVATION: f64 = 0.5;

/// How the count a row's service holds falls to zero.
#[derive(Debug, Clone, Copy)]
enum Ends {
    /// The caller asks for no task at all.
    ScaledToZero,
    /// The service is deleted while it still holds its task.
    Deleted,
}

/// How the count a row's service holds comes back.
#[derive(Debug, Clone, Copy)]
enum Returns {
    /// The caller asks for a task again.
    Restored,
    /// A caller stops the task, and the scheduler replaces it.
    Replaced,
}

/// One row: the cluster and service it brings up, and what happens to the count it holds.
struct Row<Change> {
    case: &'static str,
    cluster: &'static str,
    service: &'static str,
    change: Change,
}

const ENDINGS: [Row<Ends>; 2] = [
    Row {
        case: "a service the caller scales down to no task",
        cluster: "follow-scaled-to-zero",
        service: "orders",
        change: Ends::ScaledToZero,
    },
    Row {
        case: "a service the caller deletes while it is running",
        cluster: "follow-deleted",
        service: "checkout",
        change: Ends::Deleted,
    },
];

const RETURNS: [Row<Returns>; 2] = [
    Row {
        case: "a service the caller scales back up to one task",
        cluster: "follow-restored",
        service: "billing",
        change: Returns::Restored,
    },
    Row {
        case: "a service whose stopped task the scheduler replaces",
        cluster: "follow-replaced",
        service: "shipping",
        change: Returns::Replaced,
    },
];

/// Epoch seconds, for the window a statistics read asks about.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Bring up one row's cluster, the Fargate revision its service runs and the service itself,
/// asked for one task.
async fn create_service(router: &MultiServiceRouter, cluster: &str, service: &str) {
    ecs(
        router,
        "CreateCluster",
        json!({
            "clusterName": cluster,
            // The shape a client that wants per-service series brings up: the base namespace
            // carries them whatever this setting says.
            "settings": [{ "name": "containerInsights", "value": "disabled" }],
        }),
    )
    .await;
    ecs(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": service,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [{
                "name": "app",
                "image": IMAGE,
                "memory": 128,
                "essential": true,
                "command": STAYS_UP,
            }],
        }),
    )
    .await;
    ecs(
        router,
        "CreateService",
        json!({
            "cluster": cluster,
            "serviceName": service,
            "taskDefinition": service,
            "desiredCount": 1,
            "launchType": "FARGATE",
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [SUBNET],
                    "securityGroups": [],
                    "assignPublicIp": "ENABLED",
                }
            },
        }),
    )
    .await;
}

/// The dimension pair that names one row's service — the whole identity of its series.
fn service_dimensions(cluster: &str, service: &str) -> Value {
    json!([
        { "Name": CLUSTER_DIMENSION, "Value": cluster },
        { "Name": SERVICE_DIMENSION, "Value": service },
    ])
}

/// How many observations CloudWatch holds for `metric` on one row's pair, with the datapoints
/// it holds.
///
/// The OBSERVATIONS rather than the datapoint count: a series that reopens stamps its datapoints
/// afresh, so two of them can land in one period of the read below and be answered as one
/// datapoint whose `SampleCount` carries both.
async fn observations(
    router: &MultiServiceRouter,
    cluster: &str,
    service: &str,
    metric: &str,
) -> (f64, Vec<Value>) {
    let now = epoch_seconds();
    let stats = cloudwatch(
        router,
        "GetMetricStatistics",
        json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": service_dimensions(cluster, service),
            // Wide enough to hold the whole run-up a series opens with.
            "StartTime": now - 1_800.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Minimum", "Maximum", "Average", "SampleCount"],
        }),
    )
    .await;
    let points: Vec<Value> = stats["Datapoints"].as_array().cloned().unwrap_or_default();
    let held = points
        .iter()
        .map(|point| point["SampleCount"].as_f64().unwrap_or_default())
        .sum();
    (held, points)
}

/// The `runningCount` `DescribeServices` reports for one row's service.
async fn running_count(router: &MultiServiceRouter, cluster: &str, service: &str) -> i64 {
    let described = ecs(
        router,
        "DescribeServices",
        json!({ "cluster": cluster, "services": [service] }),
    )
    .await;
    described["services"][0]["runningCount"]
        .as_i64()
        .unwrap_or_default()
}

/// Wait until `DescribeServices` reports `wanted` tasks of one row's service in the `RUNNING`
/// state.
async fn wait_running(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    service: &str,
    wanted: i64,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let running = running_count(router, cluster, service).await;
        if running == wanted {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected runningCount {wanted}, got {running}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the row's series holds MORE observations of `metric` than `floor`, and answer the
/// total it then holds.
async fn wait_observations_above(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    service: &str,
    metric: &str,
    floor: f64,
) -> f64 {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (held, points) = observations(router, cluster, service, metric).await;
        if held > floor + HALF_AN_OBSERVATION {
            // Every datapoint of a utilization series is a percentage of what the service's
            // task definition reserves, published in `Percent`.
            for point in &points {
                assert_eq!(
                    point["Unit"].as_str(),
                    Some(PERCENT),
                    "case {case:?}: expected {metric} in {PERCENT}, got {point}"
                );
                let minimum = point["Minimum"].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a Minimum for {metric}, got {point}")
                });
                let maximum = point["Maximum"].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a Maximum for {metric}, got {point}")
                });
                assert!(
                    (0.0..=100.0).contains(&minimum)
                        && (0.0..=100.0).contains(&maximum)
                        && minimum <= maximum,
                    "case {case:?}: expected {metric} within 0..100, got {point}"
                );
            }
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected more than {floor} {metric} observation(s) on {}, got \
             {held} in {points:?}",
            service_dimensions(cluster, service)
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What each of the two series of one row holds right now.
async fn held_observations(
    router: &MultiServiceRouter,
    cluster: &str,
    service: &str,
) -> [(&'static str, f64); 2] {
    let mut held = [(CPU_UTILIZATION, 0.0); 2];
    for (slot, metric) in held.iter_mut().zip(METRICS) {
        let (total, _) = observations(router, cluster, service, metric).await;
        *slot = (metric, total);
    }
    held
}

/// The ARN of the one task a row's service is holding.
async fn running_task(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    service: &str,
) -> String {
    let listed = ecs(
        router,
        "ListTasks",
        json!({
            "cluster": cluster,
            "serviceName": service,
            "desiredStatus": "RUNNING",
        }),
    )
    .await;
    let arns: Vec<Value> = listed["taskArns"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        arns.len(),
        1,
        "case {case:?}: expected the service to hold one task, got {listed}"
    );
    arns[0].as_str().unwrap_or_default().to_string()
}

/// RULE: a service that stops holding a running task stops gaining utilization datapoints —
/// however its count fell to zero — and the datapoints it already published stay readable.
#[tokio::test]
async fn a_service_holding_no_running_task_gains_no_further_datapoint() {
    let router = emulator();
    // Every row's service is created up front, so the clock-owed placements of all of them fall
    // due at once rather than one row's wait after another's.
    for row in &ENDINGS {
        create_service(&router, row.cluster, row.service).await;
    }

    for row in &ENDINGS {
        let case = row.case;
        // The count the scheduler was asked to maintain, and the series that count produces.
        wait_running(&router, case, row.cluster, row.service, 1).await;
        for metric in METRICS {
            wait_observations_above(&router, case, row.cluster, row.service, metric, 0.0).await;
        }

        match row.change {
            Ends::ScaledToZero => {
                ecs(
                    &router,
                    "UpdateService",
                    json!({
                        "cluster": row.cluster,
                        "service": row.service,
                        "desiredCount": 0,
                    }),
                )
                .await;
            }
            Ends::Deleted => {
                ecs(
                    &router,
                    "DeleteService",
                    json!({
                        "cluster": row.cluster,
                        "service": row.service,
                        "force": true,
                    }),
                )
                .await;
            }
        }
        // "a task that has been asked to stop is no longer one of them": the count falls as soon
        // as the service's task is on its way out, whichever call asked for that.
        wait_running(&router, case, row.cluster, row.service, 0).await;

        // From that instant the service is measured by nothing, so its series is frozen where it
        // stood — asserted over a WINDOW, because a datapoint that was still owed would arrive a
        // moment later.
        let frozen = held_observations(&router, row.cluster, row.service).await;
        for (metric, total) in frozen {
            assert!(
                total > 0.0,
                "case {case:?}: expected the {metric} series published while the service ran to \
                 stay readable, got {total}"
            );
        }
        for _ in 0..QUIET_LOOKS {
            ::tokio::time::sleep(POLL_INTERVAL).await;
            for (metric, total) in frozen {
                let (held, points) = observations(&router, row.cluster, row.service, metric).await;
                assert!(
                    (held - total).abs() < HALF_AN_OBSERVATION,
                    "case {case:?}: expected the {metric} series to stay at {total} \
                     observation(s) once the service held no running task, got {held} in \
                     {points:?}"
                );
            }
        }
    }
}

/// RULE: a service whose running count comes back is measured again, and its series — the same
/// `ClusterName` + `ServiceName` pair — gains datapoints beyond the ones it held while it was
/// not running, however the count came back.
#[tokio::test]
async fn a_service_whose_running_count_comes_back_publishes_again() {
    let router = emulator();
    for row in &RETURNS {
        create_service(&router, row.cluster, row.service).await;
    }

    for row in &RETURNS {
        let case = row.case;
        wait_running(&router, case, row.cluster, row.service, 1).await;
        for metric in METRICS {
            wait_observations_above(&router, case, row.cluster, row.service, metric, 0.0).await;
        }

        // Each row takes the service's one task away, and the reading is taken once it is gone:
        // what grows after that can only have been measured on the task the service holds NEXT.
        match row.change {
            Returns::Restored => {
                ecs(
                    &router,
                    "UpdateService",
                    json!({
                        "cluster": row.cluster,
                        "service": row.service,
                        "desiredCount": 0,
                    }),
                )
                .await;
            }
            Returns::Replaced => {
                let task = running_task(&router, case, row.cluster, row.service).await;
                ecs(
                    &router,
                    "StopTask",
                    json!({
                        "cluster": row.cluster,
                        "task": task,
                        "reason": "the caller stopped it",
                    }),
                )
                .await;
            }
        }
        wait_running(&router, case, row.cluster, row.service, 0).await;
        let frozen = held_observations(&router, row.cluster, row.service).await;

        // ...and the count comes back: the caller asks for a task again, or the scheduler
        // replaces the one that stopped without being asked at all.
        if matches!(row.change, Returns::Restored) {
            ecs(
                &router,
                "UpdateService",
                json!({
                    "cluster": row.cluster,
                    "service": row.service,
                    "desiredCount": 1,
                }),
            )
            .await;
        }
        wait_running(&router, case, row.cluster, row.service, 1).await;

        for (metric, total) in frozen {
            let grown =
                wait_observations_above(&router, case, row.cluster, row.service, metric, total)
                    .await;
            assert!(
                grown > total,
                "case {case:?}: expected the {metric} series to grow past {total} observation(s) \
                 once the service held a running task again, got {grown}"
            );
        }
    }
}
