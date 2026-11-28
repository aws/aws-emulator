// SPDX-License-Identifier: CC0-1.0

//! What `TerminateJob` does to a job whose container is RUNNING, driven end to end across the
//! services that own the facts: Batch, Amazon ECS, Amazon VPC and CloudWatch Logs on one engine.
//!
//! The RULE under test: **a container job is observable in `RUNNING` for as long as its
//! container's program is running, and `TerminateJob` STOPS that program — the job reaches the
//! terminal `FAILED` status carrying the caller's `reason` verbatim as its `statusReason`, its
//! `startedAt`/`stoppedAt` bracket the attempt that was cut short, and the stream its attempt
//! names holds what the container wrote BEFORE it was stopped and nothing it would only have
//! written after.** A job the same call reaches before anything
//! placed it has no container to stop: the model splits the two — "Jobs that are in the
//! STARTING or RUNNING state are terminated, which causes them to transition to FAILED. Jobs
//! that have not progressed to the STARTING state are cancelled" (`TerminateJob`) — so such a
//! job ends `FAILED` with the same reason, reporting `isCancelled` rather than `isTerminated`
//! and no start instant at all.
//!
//! Sources: `smithy_query(query="operation",operation="TerminateJob,DescribeJobs")` for the
//! input (`jobId`, `reason`, both required), the empty 200 output and `JobDetail`'s `status`,
//! `statusReason`, `startedAt`, `stoppedAt`, `attempts`, `isCancelled` and `isTerminated`;
//! `smithy_query(query="shape",shape="JobStatus")` for the ladder's members;
//! `smithy_query(query="shape",shape="AttemptContainerDetail")` for the attempt's `taskArn` and
//! `logStreamName` ("Each container attempt receives a log stream name when they reach the
//! RUNNING status"). On the Amazon ECS side:
//! `smithy_query(query="operation",operation="DescribeTasks")` for the task whose `lastStatus`
//! says the container can write no more.
//!
//! The rows differ along what the rule generalises over: WHEN the caller ends the job (while its
//! container is running, before anything placed it, never), the LENGTH of the wait its program
//! declares, and the FORM of the reason the caller attaches (plain words against one carrying
//! punctuation, digits and non-ASCII). The row nobody ends is the control the other rows'
//! absent line is read against: it proves the line after the wait IS delivered when the program
//! is left to reach it, so its absence elsewhere is the termination's doing and not something
//! this emulator never writes.
//!
//! What is NOT asserted here, because the owning crates' own suites assert it: the placement
//! itself and the statuses either side of it
//! (`services/service-com-amazonaws-batch-awsbatchv20160810/tests/all/job_lifecycle.rs`), a job's
//! rest in `RUNNABLE` behind a queue with no capacity (that crate's `job_rests_in_runnable.rs`),
//! the members a settled attempt reports (`batch_fargate_job_runs_as_an_ecs_task.rs`), the
//! terminal status, the verbatim reason and the terminated/cancelled split on their own
//! (`batch_job_state_change_on_an_authenticating_router.rs`, which asserts them over the state
//! change the ending publishes) and `CancelJob`'s own split (that crate's
//! `tests/all/cancel_job.rs`). What is this suite's own is the RUNNING WINDOW a caller can act
//! in and the container that was really stopped inside it: the line the program wrote before its
//! wait is in the stream and the line after it is not, read against the control row where both
//! are.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The network every job below is placed in, built through Amazon EC2's own API: one VPC with
/// an attached internet gateway and a subnet whose route table sends the default route to it,
/// so a task given a public address reaches the registry its image comes from. Taken from the
/// sibling suite that already exports it rather than built again here.
use crate::batch_job_dependency_gate::network_of;

/// The Region every request below is signed for, and so the Region of every ARN minted for them.
const REGION: &str = "us-east-1";

/// The account this emulator mints its ARNs under, which the execution role below is named in.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `CEStatus`/`JQStatus` a compute environment and a job queue come to rest in. Source:
/// `smithy_query(query="shape",shape="CEStatus,JQStatus")`.
const VALID: &str = "VALID";

/// The `JobStatus` members this suite reads, spelled out here rather than imported from a
/// service crate, so a wrong constant in the implementation cannot make this suite pass. Source:
/// `smithy_query(query="shape",shape="JobStatus")`.
const STARTING: &str = "STARTING";
const RUNNING: &str = "RUNNING";
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";

/// The Amazon ECS `lastStatus` of a task that is over, after which its container can write
/// nothing more. Source: `smithy_query(query="shape",shape="Task",member="lastStatus")` on the
/// Amazon ECS model.
const STOPPED: &str = "STOPPED";

/// The image every job definition below runs, as the recorded scenario names it.
const IMAGE: &str = "public.ecr.aws/docker/library/busybox:1.36";

/// The log group the `awslogs` driver of every job below delivers to — the CALLER's own group,
/// created through CloudWatch Logs before any job runs, which is what the `awslogs-group` option
/// names.
const LOG_GROUP: &str = "/simparity/batch-terminate";

/// The `awslogs-stream-prefix` every job definition below carries: "the `awslogs-stream-prefix`
/// option allows you to associate a log stream with the specified prefix, the container name,
/// and the ID of the Amazon ECS task" (`LogConfiguration$options`).
const STREAM_PREFIX: &str = "batch";

/// The compute environment and the job queue in front of it that every row's job is submitted
/// to, EXCEPT the row that is ended before anything places it. One of each: the capacity is wide
/// enough to place every row's job at once, so no row's window waits behind another row's.
const ENVIRONMENT: &str = "batch-terminate-ce";
const QUEUE: &str = "batch-terminate-queue";

/// And the queue the row that is ended before anything places it is submitted to, over an
/// `UNMANAGED` environment: such an environment offers the queue no capacity of its own, so a job
/// on it RESTS in `RUNNABLE` until a caller ends it. That is what makes "has not progressed to
/// the STARTING state" a CONDITION this suite waits for rather than a race against the
/// placement.
const UNPLACEABLE_ENVIRONMENT: &str = "batch-terminate-unplaceable-ce";
const UNPLACEABLE_QUEUE: &str = "batch-terminate-unplaceable-queue";

/// The `JobStatus` such a job rests in.
const RUNNABLE: &str = "RUNNABLE";

/// How many vCPUs the environment offers, against the quarter of one each job asks for: enough
/// for every row of the table at the same time.
const MAX_VCPUS: u32 = 16;

/// How long a wait below gives the clock: far above the spans Batch, Amazon ECS and Amazon VPC
/// give the moves a placed job's task makes, and every wait looks again for the CONDITION rather
/// than sleeping a fixed stretch, so a loaded machine delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(240);

/// How long one look waits before the next.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// AWS Batch, Amazon ECS, Amazon EC2 and CloudWatch Logs over ONE shared engine, the way
/// `core/router/src/lib.rs` mounts every service: the task a placed job runs as is Amazon ECS's
/// resource, the interface that task is reached on is Amazon VPC's, and the stream its container
/// writes to is CloudWatch Logs'.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `service`, which is what routes the request. The key id and
/// signature are not credentials: the router above runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// The status and the parsed body of one response.
fn answered(response: http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).into_owned()))
    };
    (status, parsed)
}

/// One AWS Batch call: a `restJson1` service, so `POST <uri>` with a JSON body, which must answer
/// 200.
async fn batch(router: &MultiServiceRouter, case: &str, uri: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://batch.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("batch.{REGION}.amazonaws.com"))
        .header("authorization", authorization("batch"))
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Batch {uri}, got {status} with {parsed}"
    );
    parsed
}

/// One `awsJson1_1` call to the service signing as `service`, under `prefix.<operation>`.
async fn json_rpc(
    router: &MultiServiceRouter,
    case: &str,
    service: &str,
    prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(service))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {service} {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon ECS call.
async fn ecs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    json_rpc(
        router,
        case,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        body,
    )
    .await
}

/// One CloudWatch Logs call.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    json_rpc(router, case, "logs", "Logs_20140328", operation, body).await
}

/// WHEN, if ever, the caller ends one row's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Once the job is observed `RUNNING`: its container's program is running, so the call
    /// terminates it.
    WhileItRuns,
    /// Before anything placed it: there is no container to stop, so the call cancels it.
    BeforeItIsPlaced,
    /// Never: the program runs to its own end. The control the rows above are read against.
    Never,
}

/// One row of the table: how long its container's program waits between the two lines it writes,
/// when the caller ends it, and with which reason.
struct Row {
    case: &'static str,
    /// The seconds the program WAITS between the line it writes first and the line it writes
    /// last.
    waits_for: u32,
    ended: Ended,
    /// The `reason` the caller attaches to the job — "a message to attach to the job that
    /// explains the reason for terminating it. This message is returned by future DescribeJobs
    /// operations on the job" (`TerminateJob$reason`).
    reason: &'static str,
}

/// The table. Its rows differ along what the rule generalises over — see the module header.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a running container stopped a long wait into its program",
            waits_for: 240,
            ended: Ended::WhileItRuns,
            reason: "the operator stopped this job to free the queue",
        },
        Row {
            // A shorter wait, and a reason whose FORM is nothing like the row above's: the member
            // is returned verbatim whatever a caller writes in it.
            case: "a running container stopped a shorter wait in, with a punctuated reason",
            waits_for: 90,
            ended: Ended::WhileItRuns,
            reason: "cancelled: budget spent (attempt 7) — see note #42",
        },
        Row {
            // The other half of the model's own split: the job rests in `RUNNABLE` behind a queue
            // with no capacity, so nothing has been placed, nothing is stopped and the job is
            // CANCELLED with the same reason.
            case: "a job ended while it rested, before anything placed it",
            waits_for: 240,
            ended: Ended::BeforeItIsPlaced,
            reason: "no longer needed",
        },
        Row {
            // The control: left alone, the program reaches the line after its wait, so that line
            // IS delivered to the stream. What the rows above assert absent is absent because
            // their program was stopped.
            case: "a container left to run its program to the end",
            waits_for: 3,
            ended: Ended::Never,
            reason: "unused: this row is never ended by a caller",
        },
    ]
}

/// The line one row's container writes BEFORE its program waits, and the one it writes after.
/// Distinct per row, so no row can be read out of another row's stream.
fn first_line(index: usize) -> String {
    format!("START={index}")
}

/// See [`first_line`].
fn last_line(index: usize) -> String {
    format!("DONE={index}")
}

/// Register one row's job definition and answer its name: a FARGATE container job whose program
/// writes a line, waits, and writes another, delivering its output through the `awslogs` driver
/// to the caller's own log group.
async fn register(router: &MultiServiceRouter, row: &Row, index: usize) -> String {
    let name = format!("batch-terminate-definition-{index}");
    let program = format!(
        "echo {}; sleep {}; echo {}",
        first_line(index),
        row.waits_for,
        last_line(index)
    );
    batch(
        router,
        row.case,
        "/v1/registerjobdefinition",
        &json!({
            "jobDefinitionName": &name,
            "type": "container",
            "platformCapabilities": ["FARGATE"],
            "containerProperties": {
                "image": IMAGE,
                "command": ["sh", "-c", &program],
                // "For jobs that run on Fargate resources, you must provide an execution role"
                // (`ContainerProperties$executionRoleArn`) — the role the image pull and the log
                // driver are made as.
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batchExecutionRole"),
                "resourceRequirements": [
                    {"type": "VCPU", "value": "0.25"},
                    {"type": "MEMORY", "value": "512"},
                ],
                "networkConfiguration": {"assignPublicIp": "ENABLED"},
                "logConfiguration": {
                    "logDriver": "awslogs",
                    "options": {
                        "awslogs-group": LOG_GROUP,
                        "awslogs-region": REGION,
                        "awslogs-stream-prefix": STREAM_PREFIX,
                    },
                },
            },
        }),
    )
    .await;
    name
}

/// Look again until the compute environment `name` reports `VALID`.
async fn until_environment_valid(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(
            router,
            "setup",
            "/v1/describecomputeenvironments",
            &json!({"computeEnvironments": [name]}),
        )
        .await;
        if body["computeEnvironments"][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the compute environment never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the job queue `name` reports `VALID`.
async fn until_queue_valid(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(
            router,
            "setup",
            "/v1/describejobqueues",
            &json!({"jobQueues": [name]}),
        )
        .await;
        if body["jobQueues"][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the job queue never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The one `JobDetail` `DescribeJobs` reports for `job_id`.
async fn described(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let body = batch(router, case, "/v1/describejobs", &json!({"jobs": [job_id]})).await;
    let jobs = body["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        jobs.len(),
        1,
        "case {case:?}: expected exactly one described job for {job_id}, got {body}"
    );
    jobs[0].clone()
}

/// Whether `status` is one of the two terminal `JobStatus` members.
fn is_terminal(status: Option<&str>) -> bool {
    matches!(status, Some(SUCCEEDED) | Some(FAILED))
}

/// Look again until `job_id` reports a terminal `JobStatus`, and answer the detail it reports
/// then. Nothing here sleeps for a span: it waits for the CONDITION.
async fn until_terminal(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let detail = described(router, case, job_id).await;
        if is_terminal(detail["status"].as_str()) {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status, last {detail}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// End `job_id` with `reason`, through `TerminateJob` — which models an EMPTY output, so the 200
/// the helper asserts is the whole answer.
async fn terminate(router: &MultiServiceRouter, case: &str, job_id: &str, reason: &str) {
    let answer = batch(
        router,
        case,
        "/v1/terminatejob",
        &json!({"jobId": job_id, "reason": reason}),
    )
    .await;
    assert!(
        answer.is_null() || answer == json!({}),
        "case {case:?}: TerminateJob models an empty output, got {answer}"
    );
}

/// Look again until Amazon ECS reports `task_arn` `STOPPED`: after that the container that ran
/// the program can write nothing more, which is what makes the absence of a line an answer
/// rather than a race.
async fn until_task_stopped(router: &MultiServiceRouter, case: &str, task_arn: &str) {
    let cluster = task_arn
        .split('/')
        .nth(1)
        .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {task_arn}"))
        .to_string();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let tasks = ecs(
            router,
            case,
            "DescribeTasks",
            &json!({"cluster": &cluster, "tasks": [task_arn]}),
        )
        .await;
        let failures = tasks["failures"].as_array().cloned().unwrap_or_default();
        assert!(
            failures.is_empty(),
            "case {case:?}: Amazon ECS must resolve the task the job reports, got failures \
             {failures:?}"
        );
        if tasks["tasks"][0]["lastStatus"].as_str() == Some(STOPPED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the task the job ran as never reached {STOPPED}, last {tasks}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The messages CloudWatch Logs holds in `stream`, oldest first.
async fn stream_messages(router: &MultiServiceRouter, case: &str, stream: &str) -> Vec<String> {
    let events = logs(
        router,
        case,
        "GetLogEvents",
        &json!({
            "logGroupName": LOG_GROUP,
            "logStreamName": stream,
            "startFromHead": true,
        }),
    )
    .await;
    events["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .collect()
}

/// Look again until `stream` holds `wanted`, and answer every message it holds then. The driver
/// delivers a line as the container writes it, so this waits for the line rather than assuming
/// one call sees it.
async fn until_stream_holds(
    router: &MultiServiceRouter,
    case: &str,
    stream: &str,
    wanted: &str,
) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let messages = stream_messages(router, case, stream).await;
        if messages.iter().any(|message| message == wanted) {
            return messages;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted:?} in the stream {stream:?}, got {messages:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What one row's job is, once it has been submitted.
struct Submitted {
    job_id: String,
    /// The status the job reported just before the caller ended it, for the row whose job is
    /// ended before anything places it.
    status_when_ended: Option<String>,
    /// The log stream and the task its attempt named when it was observed `RUNNING`.
    stream: Option<String>,
    task_arn: Option<String>,
    /// The instant it was observed to have started at.
    started_at: Option<i64>,
}

#[tokio::test]
async fn terminating_a_running_job_stops_its_container_and_records_the_callers_reason() {
    let router = router();
    let (subnet, security_group) = network_of(&router).await;
    // The caller's own log group, created before any job runs: the `awslogs-group` option names
    // it, so this is where every row's container output is delivered.
    logs(
        &router,
        "setup",
        "CreateLogGroup",
        &json!({"logGroupName": LOG_GROUP}),
    )
    .await;
    batch(
        &router,
        "setup",
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": ENVIRONMENT,
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                "maxvCpus": MAX_VCPUS,
                "subnets": [&subnet],
                "securityGroupIds": [&security_group],
            },
        }),
    )
    .await;
    batch(
        &router,
        "setup",
        "/v1/createjobqueue",
        &json!({
            "jobQueueName": QUEUE,
            "priority": 1,
            "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": ENVIRONMENT}],
        }),
    )
    .await;
    // And the queue with no capacity of its own behind it, for the row that is ended before
    // anything places its job: an `UNMANAGED` environment offers the queue none, so a job on it
    // rests in `RUNNABLE`.
    batch(
        &router,
        "setup",
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": UNPLACEABLE_ENVIRONMENT,
            "type": "UNMANAGED",
        }),
    )
    .await;
    batch(
        &router,
        "setup",
        "/v1/createjobqueue",
        &json!({
            "jobQueueName": UNPLACEABLE_QUEUE,
            "priority": 1,
            "computeEnvironmentOrder": [
                {"order": 1, "computeEnvironment": UNPLACEABLE_ENVIRONMENT},
            ],
        }),
    )
    .await;
    for (environment, queue) in [
        (ENVIRONMENT, QUEUE),
        (UNPLACEABLE_ENVIRONMENT, UNPLACEABLE_QUEUE),
    ] {
        until_environment_valid(&router, environment).await;
        until_queue_valid(&router, queue).await;
    }

    let rows = rows();
    let mut submitted: BTreeMap<usize, Submitted> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let definition = register(&router, row, index).await;
        // The row ended before anything places it goes to the queue with no capacity behind it;
        // every other row goes to the one that can place it.
        let queue = if row.ended == Ended::BeforeItIsPlaced {
            UNPLACEABLE_QUEUE
        } else {
            QUEUE
        };
        let answer = batch(
            &router,
            row.case,
            "/v1/submitjob",
            &json!({
                "jobName": format!("batch-terminate-job-{index}"),
                "jobQueue": queue,
                "jobDefinition": &definition,
            }),
        )
        .await;
        let job_id = answer["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a jobId in {answer}", row.case))
            .to_string();
        let record = Submitted {
            job_id,
            status_when_ended: None,
            stream: None,
            task_arn: None,
            started_at: None,
        };
        submitted.insert(index, record);
    }

    // The row ended before anything places it is ended once its job is OBSERVED resting in
    // `RUNNABLE`: the queue it was submitted to offers no capacity, so that is where it stays
    // until a caller ends it — which is what makes "has not progressed to the STARTING state" a
    // condition waited for rather than a race against the placement.
    for (index, row) in rows.iter().enumerate() {
        if row.ended != Ended::BeforeItIsPlaced {
            continue;
        }
        let record = submitted
            .get_mut(&index)
            .expect("every row was submitted above");
        let deadline = Instant::now() + DEADLINE;
        loop {
            let detail = described(&router, row.case, &record.job_id).await;
            if detail["status"].as_str() == Some(RUNNABLE) {
                record.status_when_ended = Some(RUNNABLE.to_string());
                terminate(&router, row.case, &record.job_id, row.reason).await;
                break;
            }
            assert!(
                !is_terminal(detail["status"].as_str()),
                "case {:?}: a job on a queue with no capacity rests in {RUNNABLE} until a caller \
                 ends it; it reached a terminal status on its own: {detail}",
                row.case
            );
            assert!(
                Instant::now() < deadline,
                "case {:?}: the job never came to rest in {RUNNABLE}, last {detail}",
                row.case
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    // Every row ended WHILE RUNNING is ended as soon as it is OBSERVED running: one loop over
    // the jobs still waiting for that window, so no row's window is spent waiting behind
    // another's. A job that reached a terminal status without ever being seen RUNNING is the
    // failure this loop reports — such a job has no running window for a caller to act in.
    let mut waiting: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.ended == Ended::WhileItRuns)
        .map(|(index, _)| index)
        .collect();
    let deadline = Instant::now() + DEADLINE;
    while !waiting.is_empty() {
        let mut still = Vec::new();
        for index in waiting {
            let row = &rows[index];
            let record = submitted
                .get_mut(&index)
                .expect("every row was submitted above");
            let detail = described(&router, row.case, &record.job_id).await;
            if detail["status"].as_str() == Some(RUNNING) {
                // "Each container attempt receives a log stream name when they reach the RUNNING
                // status" (`AttemptContainerDetail$logStreamName`), and a task ARN when it
                // reaches STARTING — so both are on the attempt by now.
                let attempt = detail["attempts"][0].clone();
                record.stream = attempt["container"]["logStreamName"]
                    .as_str()
                    .map(str::to_string);
                record.task_arn = attempt["container"]["taskArn"].as_str().map(str::to_string);
                record.started_at = detail["startedAt"].as_i64().filter(|at| *at > 0);
                assert!(
                    record.stream.is_some() && record.task_arn.is_some(),
                    "case {:?}: a RUNNING job's attempt must name its log stream and its task, \
                     got {detail}",
                    row.case
                );
                terminate(&router, row.case, &record.job_id, row.reason).await;
                continue;
            }
            assert!(
                !is_terminal(detail["status"].as_str()),
                "case {:?}: the job was never observed {RUNNING} — it reached a terminal status \
                 first, so a caller has no window to terminate a running job in: {detail}",
                row.case
            );
            still.push(index);
        }
        waiting = still;
        if !waiting.is_empty() {
            assert!(
                Instant::now() < deadline,
                "cases {waiting:?}: no running window was ever observed for them"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let record = &submitted[&index];
        let detail = until_terminal(&router, case, &record.job_id).await;
        let status = detail["status"].as_str();
        let reason = detail["statusReason"].as_str();
        match row.ended {
            Ended::Never => {
                // The control: nothing ended it, so the program reached the line after its wait
                // and the container exited of its own accord.
                assert_eq!(
                    status,
                    Some(SUCCEEDED),
                    "case {case:?}: a container left to finish exits 0, so its job \
                     {SUCCEEDED}: {detail}"
                );
                let stream = detail["attempts"][0]["container"]["logStreamName"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the attempt must name its log stream, {detail}")
                    })
                    .to_string();
                let messages = until_stream_holds(&router, case, &stream, &last_line(index)).await;
                assert_eq!(
                    messages,
                    vec![first_line(index), last_line(index)],
                    "case {case:?}: a program left to run writes the line before its wait AND \
                     the line after it, got {messages:?}"
                );
            }
            Ended::BeforeItIsPlaced => {
                // "Jobs that have not progressed to the STARTING state are cancelled": the
                // status the job was RESTING in when the call reached it is one of those.
                let when = record.status_when_ended.clone().unwrap_or_default();
                assert_eq!(
                    when, RUNNABLE,
                    "case {case:?}: this row ends a job that has NOT progressed to {STARTING}; it \
                     reported {when:?}"
                );
                assert_eq!(
                    status,
                    Some(FAILED),
                    "case {case:?}: a cancelled job ends {FAILED}: {detail}"
                );
                assert_eq!(
                    reason,
                    Some(row.reason),
                    "case {case:?}: expected the caller's reason verbatim, got {reason:?}"
                );
                assert_eq!(
                    (
                        detail["isCancelled"].as_bool(),
                        detail["isTerminated"].as_bool()
                    ),
                    (Some(true), Some(false)),
                    "case {case:?}: a job that had not progressed to STARTING is CANCELLED, not \
                     terminated: {detail}"
                );
                assert!(
                    detail["startedAt"].as_i64().is_none_or(|at| at == 0),
                    "case {case:?}: a job whose container never started reports no startedAt: \
                     {detail}"
                );
                assert!(
                    detail["attempts"]
                        .as_array()
                        .is_none_or(|attempts| attempts.is_empty()),
                    "case {case:?}: a job nothing placed records no attempt: {detail}"
                );
            }
            Ended::WhileItRuns => {
                assert_eq!(
                    status,
                    Some(FAILED),
                    "case {case:?}: a terminated job transitions to {FAILED}: {detail}"
                );
                assert_eq!(
                    reason,
                    Some(row.reason),
                    "case {case:?}: expected the caller's reason verbatim, got {reason:?}"
                );
                assert_eq!(
                    (
                        detail["isCancelled"].as_bool(),
                        detail["isTerminated"].as_bool()
                    ),
                    (Some(false), Some(true)),
                    "case {case:?}: a job in RUNNING is TERMINATED, not cancelled: {detail}"
                );
                // The attempt is BRACKETED: a job terminated while running reports both the
                // instant it started at and the instant it stopped at, in that order
                // (`AttemptDetail$startedAt` is "when the attempt transitioned from the STARTING
                // state to the RUNNING state" and `$stoppedAt` "when the attempt transitioned
                // from the RUNNING state to a terminal state"). No DURATION is asserted of it:
                // the emulator performs a declared wait in compressed time, so the wall clock
                // between the two instants says nothing about the wait the program named, and a
                // bound written against that wait would hold whatever the terminate did. What
                // says the container was stopped PARTWAY through its program is the line it
                // never wrote, read below against the control row that writes it.
                let started = detail["startedAt"].as_i64().filter(|at| *at > 0);
                let stopped = detail["stoppedAt"].as_i64().filter(|at| *at > 0);
                let (Some(started), Some(stopped)) = (started, stopped) else {
                    panic!(
                        "case {case:?}: expected both startedAt and stoppedAt on a job \
                         terminated while running: {detail}"
                    );
                };
                assert!(
                    started <= stopped,
                    "case {case:?}: expected startedAt {started} at or before stoppedAt \
                     {stopped}"
                );
                assert_eq!(
                    record.started_at,
                    Some(started),
                    "case {case:?}: the instant the job was observed to have started at is the \
                     one it still reports: {detail}"
                );
                // And the stream holds what the container WROTE before it was stopped, and not
                // the line its program would only have reached after the wait. Read once the
                // task is over, after which nothing can write to the stream at all.
                let task_arn = record.task_arn.clone().unwrap_or_default();
                until_task_stopped(&router, case, &task_arn).await;
                let stream = record.stream.clone().unwrap_or_default();
                // The stream the driver opened is named as its options state: "the
                // `awslogs-stream-prefix` option allows you to associate a log stream with the
                // specified prefix, the container name, and the ID of the Amazon ECS task that
                // the container belongs to" (`LogConfiguration$options`) — so the first segment
                // is the prefix the configuration asked for and the last is the id of the task
                // the attempt reports.
                let segments: Vec<&str> = stream.split('/').collect();
                let task_id = task_arn.rsplit('/').next().unwrap_or_default();
                assert!(
                    segments.len() == 3
                        && segments[0] == STREAM_PREFIX
                        && !segments[1].is_empty()
                        && segments[2] == task_id,
                    "case {case:?}: expected the stream {stream:?} to be \
                     {STREAM_PREFIX}/<container>/{task_id}"
                );
                let messages = until_stream_holds(&router, case, &stream, &first_line(index)).await;
                assert_eq!(
                    messages,
                    vec![first_line(index)],
                    "case {case:?}: expected only the line written before the wait in \
                     {stream:?}, got {messages:?}"
                );
            }
        }
    }
}
