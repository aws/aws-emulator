// SPDX-License-Identifier: CC0-1.0

//! Every store this service grows per request is bounded.
//!
//! A record a caller creates stays in memory for the life of the process, so each store
//! has a ceiling: the create that reaches it exactly is served, and the next one is
//! refused with the operation's modelled client error and stores nothing.
//!
//! Each store is filled to one short of its ceiling through the state API — doing it one
//! request at a time would make each case tens of thousands of HTTP calls long — while
//! the two cases that matter, the last create that fits and the first that does not, go
//! through the same front door a client uses.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_datasync_fmrsservice::state;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// Long enough for a task's `CREATING` -> `AVAILABLE` transition to fall due
/// (`state::TRANSITION_SPAN` is 300 ms), with margin for a loaded test machine.
const ONE_SPAN: Duration = Duration::from_millis(500);

/// A router with just this service mounted, and the engine its state lives in.
fn service_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// A call that must succeed, returning the ARN it reports under `member`.
async fn created(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    member: &str,
    case: &str,
) -> String {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {parsed}"))
        .to_string()
}

/// A call that must be refused with HTTP 400 `InvalidRequestException` reporting the
/// ceiling `ceiling`.
async fn refused_at_ceiling(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    ceiling: usize,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    assert!(
        parsed["__type"]
            .as_str()
            .unwrap_or_default()
            .contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
    let message = parsed["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&ceiling.to_string()),
        "case {case:?}: expected the refusal to name the ceiling {ceiling}, got {message:?}"
    );
}

/// An ARN for a filler record. The `f`-prefixed id cannot collide with the ARNs the
/// state mints, whose sequence counts up from one.
fn filler_arn(resource: &str, index: usize) -> String {
    format!("arn:aws:datasync:{SIGNED_REGION}:{ACCOUNT}:{resource}/{resource}-f{index:016x}")
}

/// `CreateLocation*` serves creates up to `state::MAX_LOCATIONS` stored locations and
/// refuses past it.
#[tokio::test]
async fn the_location_store_is_bounded() {
    let (router, engine) = service_with_engine();
    let request = serde_json::json!({
        "ContainerUrl": "https://bounded.blob.core.windows.net/bounded-container",
        "AuthenticationType": "NONE",
    });

    {
        let stored = state::service_state(&*engine).expect("this service's state store");
        let mut store = stored.lock();
        for index in 0..state::MAX_LOCATIONS - 1 {
            let arn = filler_arn("location", index);
            store.locations.insert(
                arn.clone(),
                state::Location {
                    arn,
                    // The filler records sit before every minted location in creation
                    // order, which is what `ListLocations` pages over.
                    seq: 0,
                    location_uri: "azure-blob://filler.blob.core.windows.net/filler/".to_string(),
                    subdirectory: None,
                    agent_arns: Vec::new(),
                    tags: Vec::new(),
                    creation_time: SystemTime::now(),
                    config: state::LocationConfig::AzureBlob(state::AzureBlobLocationConfig {
                        container_url: "https://filler.blob.core.windows.net/filler".to_string(),
                        authentication_type: "NONE".to_string(),
                        blob_type: "BLOCK".to_string(),
                        access_tier: "HOT".to_string(),
                        secret: state::LocationSecret::None,
                        federated_identity: None,
                    }),
                },
            );
        }
    }

    // The create that reaches the ceiling exactly is served.
    let _ = created(
        &router,
        "CreateLocationAzureBlob",
        request.clone(),
        "LocationArn",
        "at the ceiling",
    )
    .await;

    // The next one is refused, and nothing is stored for it.
    refused_at_ceiling(
        &router,
        "CreateLocationAzureBlob",
        request,
        state::MAX_LOCATIONS,
        "past the ceiling",
    )
    .await;
    let stored = state::service_state(&*engine).expect("this service's state store");
    assert_eq!(
        stored.lock().locations.len(),
        state::MAX_LOCATIONS,
        "case \"past the ceiling\": a refused create must store nothing"
    );
}

/// `CreateTask` serves creates up to `state::MAX_TASKS` stored tasks and refuses past it.
#[tokio::test]
async fn the_task_store_is_bounded() {
    let (router, engine) = service_with_engine();

    let location = |bucket: &str| {
        serde_json::json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
            "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
        })
    };
    let source = created(
        &router,
        "CreateLocationS3",
        location("bounded-source"),
        "LocationArn",
        "setup",
    )
    .await;
    let destination = created(
        &router,
        "CreateLocationS3",
        location("bounded-destination"),
        "LocationArn",
        "setup",
    )
    .await;
    let request = serde_json::json!({
        "SourceLocationArn": source,
        "DestinationLocationArn": destination,
    });

    {
        let stored = state::service_state(&*engine).expect("this service's state store");
        let mut store = stored.lock();
        for index in 0..state::MAX_TASKS - 1 {
            let arn = filler_arn("task", index);
            store.tasks.insert(
                arn.clone(),
                filler_task(&arn, index as u64),
            );
        }
    }

    let _ = created(&router, "CreateTask", request.clone(), "TaskArn", "at the ceiling").await;
    refused_at_ceiling(
        &router,
        "CreateTask",
        request,
        state::MAX_TASKS,
        "past the ceiling",
    )
    .await;
    let stored = state::service_state(&*engine).expect("this service's state store");
    assert_eq!(
        stored.lock().tasks.len(),
        state::MAX_TASKS,
        "case \"past the ceiling\": a refused create must store nothing"
    );
}

/// `StartTaskExecution` serves starts up to `state::MAX_TASK_EXECUTIONS` stored
/// executions and refuses past it — a refusal about the store, not about the task's
/// status, which the second task here is in no position to earn.
#[tokio::test]
async fn the_task_execution_store_is_bounded() {
    let (router, engine) = service_with_engine();

    let location = |bucket: &str| {
        serde_json::json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
            "S3Config": { "BucketAccessRoleArn": "arn:aws:iam::000000000000:role/datasync" },
        })
    };
    let source = created(
        &router,
        "CreateLocationS3",
        location("execution-source"),
        "LocationArn",
        "setup",
    )
    .await;
    let destination = created(
        &router,
        "CreateLocationS3",
        location("execution-destination"),
        "LocationArn",
        "setup",
    )
    .await;
    let task = |name: &str| {
        serde_json::json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": name,
        })
    };
    let first = created(&router, "CreateTask", task("first"), "TaskArn", "setup").await;
    let second = created(&router, "CreateTask", task("second"), "TaskArn", "setup").await;

    {
        let stored = state::service_state(&*engine).expect("this service's state store");
        let mut store = stored.lock();
        let filler_task_arn = filler_arn("task", 0);
        for index in 0..state::MAX_TASK_EXECUTIONS - 1 {
            let arn = format!("{filler_task_arn}/execution/exec-f{index:016x}");
            store
                .task_executions
                .insert(arn.clone(), filler_execution(&arn, &filler_task_arn));
        }
    }

    // Both tasks leave `CREATING` on the clock, so neither start is refused for status.
    tokio::time::sleep(ONE_SPAN).await;

    let _ = created(
        &router,
        "StartTaskExecution",
        serde_json::json!({ "TaskArn": first }),
        "TaskExecutionArn",
        "at the ceiling",
    )
    .await;
    refused_at_ceiling(
        &router,
        "StartTaskExecution",
        serde_json::json!({ "TaskArn": second }),
        state::MAX_TASK_EXECUTIONS,
        "past the ceiling",
    )
    .await;

    let stored = state::service_state(&*engine).expect("this service's state store");
    let store = stored.lock();
    assert_eq!(
        store.task_executions.len(),
        state::MAX_TASK_EXECUTIONS,
        "case \"past the ceiling\": a refused start must store nothing"
    );
    let unstarted = store
        .tasks
        .get(&second)
        .expect("the second task is still stored");
    assert!(
        unstarted.current_task_execution_arn.is_none() && unstarted.execution_arns.is_empty(),
        "case \"past the ceiling\": a refused start must leave the task with no execution, \
         got {unstarted:?}"
    );
}

/// A stored transfer task that is past `CREATING`, for filling the task store.
fn filler_task(arn: &str, seq: u64) -> state::Task {
    let now = SystemTime::now();
    state::Task {
        arn: arn.to_string(),
        seq,
        name: None,
        status: state::TaskStatus::Available,
        available_at: now,
        source_location_arn: filler_arn("location", 0),
        destination_location_arn: filler_arn("location", 1),
        cloud_watch_log_group_arn: None,
        options: state::default_options("BASIC"),
        includes: Vec::new(),
        excludes: Vec::new(),
        schedule: None,
        manifest_config: None,
        task_report_config: None,
        task_mode: "BASIC".to_string(),
        tags: Vec::new(),
        creation_time: now,
        current_task_execution_arn: None,
        execution_arns: Vec::new(),
    }
}

/// A stored task execution, for filling the task-execution store.
fn filler_execution(arn: &str, task_arn: &str) -> state::TaskExecution {
    let now = SystemTime::now();
    state::TaskExecution {
        arn: arn.to_string(),
        seq: 0,
        task_arn: task_arn.to_string(),
        status: state::TaskExecutionStatus::Launching,
        options: state::default_options("BASIC"),
        includes: Vec::new(),
        excludes: Vec::new(),
        manifest_config: None,
        task_report_config: None,
        tags: Vec::new(),
        task_mode: "BASIC".to_string(),
        start_time: now,
        launch_time: None,
        end_time: None,
        schedule: state::ExecutionSchedule::starting_at(now),
        counters: state::ExecutionCounters::default(),
        progress: state::ExecutionProgress::default(),
    }}
