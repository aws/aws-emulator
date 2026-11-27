// SPDX-License-Identifier: CC0-1.0

//! WHY the Amazon ECS service scheduler could not launch a service's tasks, as the `ECS
//! Service Action` event it publishes reports it.
//!
//! The RULE under test: an action the scheduler publishes because it COULD NOT do what the
//! service asks of it carries, beside the action's name and its severity, the `reason` that
//! says what stopped it — the very sentence the service's own records hold for that failure,
//! which for a launch Amazon ECS could never make is the `Task$stoppedReason` of the stop that
//! launch earned ("The reason that the task was stopped"). An action that reports PROGRESS
//! explains nothing and carries no `reason` at all. So a client alerting on such an event
//! learns from the event alone what has to change, and the event cannot disagree with what
//! `DescribeTasks` reports about the task the scheduler lost.
//!
//! The rows span the class the rule generalises over: WHICH answer a registry gives the fetch
//! the launch makes, and how the reference is spelled — a tag the public gallery publishes no
//! release of, the same tag through the registry that gallery mirrors those images from and
//! through that registry's own host, and a repository the mirrored namespace's catalogue does
//! not list at all. The last row is the control the differential needs: the same service on a
//! reference the gallery DOES publish, whose scheduler reaches a steady state and explains
//! nothing.
//!
//! EventBridge and Amazon SQS are mounted BESIDE Amazon ECS on the SAME engine (the harness
//! [`crate::task_state_change_events::emulator`] builds), so a publication travels the real
//! in-process callback, through the real bus, to a real SQS target; nothing reads the
//! publisher's own queue. [`crate::ecs_service_events_reach_their_per_service_queues`]'s
//! sibling in the router crate drives the same rule over the whole assembled emulator.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{awsvpc_network_configuration, list, ok, string, ACCOUNT, SIGNED_REGION};
use crate::task_lifecycle::container;
use crate::task_state_change_events::{drain, emulator, given_queue, given_rule, grant_delivery};

/// The `source` and `detail-type` Amazon ECS publishes a service action under, and the two
/// actions under test with the severity each carries — spelled out here rather than imported,
/// so a wrong constant in the implementation cannot make this suite pass.
const EVENT_SOURCE: &str = "aws.ecs";
const SERVICE_ACTION: &str = "ECS Service Action";
const TASK_CONFIGURATION_FAILURE: &str = "SERVICE_TASK_CONFIGURATION_FAILURE";
const STEADY_STATE: &str = "SERVICE_STEADY_STATE";
const INFO: &str = "INFO";
const ERROR: &str = "ERROR";

/// The error class a container runtime reports an image it could not fetch under, which is the
/// fixed text every row's `reason` begins with.
const CANNOT_PULL: &str = "CannotPullContainerError";

/// The cluster every service below runs in.
const CLUSTER: &str = "failurereason";

/// How often a poll re-reads the queue, and how long it keeps trying: generous, since what is
/// asserted is that the event arrives at all rather than how fast the host runs.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLL_BUDGET: Duration = Duration::from_secs(60);

/// One row: the image its service's revision runs, and whether the scheduler can launch it.
struct Row {
    case: &'static str,
    /// The `Container$image` of the row's revision.
    image: &'static str,
    /// Whether the launch can be made — the control row is the one that can.
    launchable: bool,
}

/// The rows span which answer the fetch gets and how the reference is spelled.
///
/// SOURCE of the references: the repositories the public gallery this emulator serves publishes
/// (its mirror of the Docker Official Images, `public.ecr.aws/docker/library/<name>`), the
/// registry those images are mirrored FROM — which a reference names with or without that
/// registry's own `library` namespace, and which a container runtime resolves a reference
/// naming no registry at all against — and that namespace's published catalogue, whose
/// repository list is what says a repository exists.
const ROWS: &[Row] = &[
    Row {
        case: "a tag the gallery publishes no release of",
        image: "public.ecr.aws/docker/library/busybox:no-such-release",
        launchable: false,
    },
    Row {
        case: "the same tag spelled against the registry the gallery mirrors",
        image: "busybox:no-such-release",
        launchable: false,
    },
    Row {
        case: "the same tag spelled with that registry's own host and namespace",
        image: "docker.io/library/busybox:no-such-release",
        launchable: false,
    },
    Row {
        case: "a repository the mirrored namespace's catalogue does not list",
        image: "public.ecr.aws/docker/library/no-such-official-image:latest",
        launchable: false,
    },
    Row {
        case: "a release the gallery does publish",
        image: "public.ecr.aws/docker/library/busybox:latest",
        launchable: true,
    },
];

/// The ARN of the cluster this suite's services run in.
fn cluster_arn() -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{CLUSTER}")
}

/// The pattern the rule carries: every service action of this suite's cluster.
fn service_action_pattern() -> Value {
    json!({
        "source": [EVENT_SOURCE],
        "detail-type": [SERVICE_ACTION],
        "detail": {"clusterArn": [cluster_arn()]},
    })
}

/// Every envelope of `held` whose `detail.eventName` is `event_name`.
fn named<'held>(held: &'held [Value], event_name: &str) -> Vec<&'held Value> {
    held.iter()
        .filter(|event| event["detail"]["eventName"] == json!(event_name))
        .collect()
}

/// The `stoppedReason` Amazon ECS reports for the tasks of the service `service` that have
/// stopped, as `ListTasks` and `DescribeTasks` answer for them.
async fn stopped_reasons(router: &MultiServiceRouter, case: &str, service: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListTasks",
        json!({"cluster": CLUSTER, "serviceName": service, "desiredStatus": "STOPPED"}),
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
        json!({"cluster": CLUSTER, "tasks": arns}),
        case,
    )
    .await;
    list(&described, "tasks", case)
        .iter()
        .filter_map(|task| task["stoppedReason"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn a_scheduler_failure_action_carries_the_reason_the_records_hold() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // A fresh emulator per row, so one row's services cannot decide another's.
        let router = emulator();
        let (url, queue_arn) = given_queue(&router, case, "ecs-service-actions").await;
        given_rule(
            &router,
            case,
            "ecs-service-action",
            service_action_pattern(),
            &queue_arn,
        )
        .await;
        grant_delivery(&router, case, &url, &queue_arn).await;

        ok(
            &router,
            "CreateCluster",
            json!({"clusterName": CLUSTER}),
            case,
        )
        .await;
        let family = format!("reasonfamily{index}");
        ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": &family,
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                // A documented Fargate task size: 256 CPU units, 512 MiB.
                "cpu": "256",
                "memory": "512",
                "containerDefinitions": [container("web", row.image)],
            }),
            case,
        )
        .await;
        let service = format!("reasonservice{index}");
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": &service,
                "taskDefinition": &family,
                "launchType": "FARGATE",
                "desiredCount": 1,
                "networkConfiguration": awsvpc_network_configuration(),
            }),
            case,
        )
        .await;
        let service_arn = string(&created["service"], "serviceArn", case);

        // ONLY the queue is polled while the action is waited for: a client watching a service
        // through EventBridge makes no further Amazon ECS call.
        let awaited = if row.launchable {
            STEADY_STATE
        } else {
            TASK_CONFIGURATION_FAILURE
        };
        let started = Instant::now();
        let mut held: Vec<Value> = Vec::new();
        while started.elapsed() < POLL_BUDGET && named(&held, awaited).is_empty() {
            held.extend(drain(&router, case, &url).await);
            if named(&held, awaited).is_empty() {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
        let delivered = named(&held, awaited);
        assert!(
            !delivered.is_empty(),
            "case {case:?}: expected the action {awaited} for {service_arn}, got {held:?}"
        );

        for envelope in &delivered {
            assert_eq!(
                envelope["resources"],
                json!([&service_arn]),
                "case {case:?}: expected resources to name only {service_arn}, got {envelope}"
            );
            assert_eq!(
                envelope["detail"]["clusterArn"],
                json!(cluster_arn()),
                "case {case:?}: expected the cluster the scheduler acted in, got {envelope}"
            );
        }

        if row.launchable {
            // The control: an action that reports progress explains nothing, and the service
            // whose launches were made loses none.
            for envelope in &delivered {
                assert_eq!(
                    envelope["detail"]["eventType"],
                    json!(INFO),
                    "case {case:?}: expected eventType {INFO}, got {envelope}"
                );
                assert_eq!(
                    envelope["detail"].get("reason"),
                    None,
                    "case {case:?}: a steady state explains nothing, got {envelope}"
                );
            }
            assert!(
                named(&held, TASK_CONFIGURATION_FAILURE).is_empty(),
                "case {case:?}: a service whose launches are made loses none, got {held:?}"
            );
            continue;
        }

        // The reason the event carries is the one the records hold: the `stoppedReason` of the
        // stop the lost launch earned, which `DescribeTasks` answers with for that very task.
        let reported = stopped_reasons(&router, case, &service).await;
        for envelope in &delivered {
            assert_eq!(
                envelope["detail"]["eventType"],
                json!(ERROR),
                "case {case:?}: expected eventType {ERROR}, got {envelope}"
            );
            let reason = envelope["detail"]["reason"].as_str().unwrap_or_default();
            assert!(
                reason.starts_with(CANNOT_PULL),
                "case {case:?}: expected detail.reason to begin {CANNOT_PULL:?}, got {reason:?}"
            );
            assert!(
                reason.contains(row.image),
                "case {case:?}: expected detail.reason to name the image {:?} the revision \
                 asked for, got {reason:?}",
                row.image
            );
            assert!(
                reported.iter().any(|held| held == reason),
                "case {case:?}: expected detail.reason to be a stoppedReason DescribeTasks \
                 reports ({reported:?}), got {reason:?}"
            );
        }
        assert!(
            named(&held, STEADY_STATE).is_empty(),
            "case {case:?}: a service whose launches cannot be made reaches no steady state, \
             got {held:?}"
        );
    }
}
