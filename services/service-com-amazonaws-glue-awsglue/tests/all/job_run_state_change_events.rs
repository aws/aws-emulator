// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EventBridge event a Glue job run publishes when it comes to rest.
//!
//! The rule under test: a job run that reaches a terminal `JobRunState` is published onto the
//! account's DEFAULT EventBridge bus as `source` `aws.glue` / `detail-type` `Glue Job State
//! Change`, whose `detail` carries that run's own `jobName`, `jobRunId`, the `state` it came
//! to rest in, how severe that state is for the job (`severity`) and — where the run carries
//! one — its `message`. EventBridge then routes it, so a rule whose `EventPattern` matches
//! delivers it to that rule's targets and a rule that does not match receives nothing, while
//! a target queue that has granted EventBridge nothing receives nothing either. A run still
//! in flight has come to rest at nothing and publishes nothing, and a run already published
//! is not published again.
//!
//! The recorded case — a `pythonshell` run that settles `SUCCEEDED`, delivered to an SQS
//! queue whose policy grants `events.amazonaws.com` and withheld from a sibling queue with no
//! such policy — is ONE row of the table below; the rows differ along the terminal state the
//! run reaches and WHY (a script that exits cleanly, a script that raises, a job naming no
//! script at all, a run a client stopped), which is what the rule generalises over.
//!
//! EventBridge, SQS and Amazon S3 are mounted BESIDE Glue on the SAME engine, so the
//! publication travels the real in-process callback, through the real bus, to a real SQS
//! target: nothing here reads the publisher's own queue. The due instant a run's terminal
//! state falls at is moved THROUGH THE STORE rather than slept for, each run is read back
//! until it HAS come to rest ([`crate::job_runs::run_at_rest`]), and the publication is made
//! by the very call that materialises the transition, so no case waits on the clock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    JOB_RUN_STATE_FAILED, JOB_RUN_STATE_STARTING, JOB_RUN_STATE_STOPPED, JOB_RUN_STATE_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_script_execution::{create_bucket, stage_script, SCRIPTS};
use crate::job_runs::{far_future, run_at_rest, set_due_instant};
use crate::wire::{ok, REGION};

/// The `source` and `detail-type` Glue publishes a job run's terminal state under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass.
const EVENT_SOURCE: &str = "aws.glue";
const JOB_STATE_CHANGE: &str = "Glue Job State Change";

/// The `detail.severity` of a run that came to rest having failed at nothing, and of one that
/// failed.
const SEVERITY_INFO: &str = "INFO";
const SEVERITY_ERROR: &str = "ERROR";

/// The rule that matches a Glue job run event, and the rule that cannot.
const MATCHING_RULE: &str = "glue-job-state-change";
const OTHER_RULE: &str = "unrelated-events";

/// A script that exits cleanly, which is what a succeeding Glue job does.
const EXITS_CLEANLY: &str = r#"
rows = [row for row in range(4)]
assert sum(rows) == 6, rows
"#;

/// A script that raises, so the run's own reason is the exception's type and message.
const RAISES: &str = r#"
raise ValueError("the input partition is malformed")
"#;

/// An emulator with Glue, Amazon S3, EventBridge AND SQS on one shared engine, plus that
/// engine.
///
/// Amazon S3 is mounted because a run reads its script from it; EventBridge and SQS because
/// the publication goes onto the account's default bus and a rule on that bus delivers it to
/// a queue.
fn emulator_with_a_bus() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = crate::wire::emulator_on(Arc::clone(&engine));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One `awsJson` RPC call against a service mounted beside Glue, the way a client makes it:
/// `POST /` with `X-Amz-Target: <prefix>.<operation>` and a credential scoped to
/// `signing_name`, which is what the router picks the service from.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDGLUEEVENTS/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one EventBridge operation.
async fn events(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "EventBridge {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// Call one SQS operation.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "SQS {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// An SQS queue called `name`, created through SQS itself; answers its url and the ARN a rule
/// names it by.
pub(crate) async fn given_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue must answer a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes must answer a QueueArn, got {attributes}"))
        .to_string();
    (url, arn)
}

/// Grant `events.amazonaws.com` the `sqs:SendMessage` a rule's target needs on the queue at
/// `url` — EventBridge makes that call as its own service principal and relies on the
/// queue's resource-based policy, so a queue that has granted it nothing receives nothing.
pub(crate) async fn grant_delivery(router: &MultiServiceRouter, url: &str, queue_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
}

/// Put a rule matching `pattern` on the default bus and point it at `targets`.
pub(crate) async fn given_rule(
    router: &MultiServiceRouter,
    name: &str,
    pattern: Value,
    targets: &[String],
) {
    events(
        router,
        "PutRule",
        json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
    )
    .await;
    let entries: Vec<Value> = targets
        .iter()
        .enumerate()
        .map(|(index, arn)| json!({ "Id": format!("target-{index}"), "Arn": arn }))
        .collect();
    let put = events(
        router,
        "PutTargets",
        json!({ "Rule": name, "Targets": entries }),
    )
    .await;
    assert_eq!(
        put["FailedEntryCount"], json!(0),
        "PutTargets must accept every target, got {put}"
    );
}

/// Every message the queue at `url` holds, drained.
pub(crate) async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("a delivered event must be JSON, got {body:?} ({error})"))
        })
        .collect()
}

/// How long a delivery is waited for, and how often the queue is looked at.
///
/// The publication is the service's own clock pump's rather than the answering call's, so
/// what is waited for is the CONDITION — the event on the queue — and the deadline is
/// generous, so a loaded or a fast host reaches the same verdict. Nothing here asserts on how
/// long it took.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);
const DELIVERY_POLL: Duration = Duration::from_millis(20);

/// The ONE event delivered to the queue at `url`, waited for until it is there.
pub(crate) async fn delivered_event(router: &MultiServiceRouter, case: &str, url: &str) -> Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let messages = drain(router, url).await;
        if let Some(first) = messages.first() {
            assert_eq!(
                messages.len(),
                1,
                "case {case:?}: expected one delivered event, got {messages:?}"
            );
            return first.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no event reached the target queue before the deadline"
        );
        tokio::time::sleep(DELIVERY_POLL).await;
    }
}

/// One row: a job whose run comes to rest, and the event that must report it.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The script to stage, or `None` for a job that names no `ScriptLocation` at all.
    script: Option<&'static str>,
    /// Whether a client stops the run while it is still in flight — which is also the row
    /// that pins that a run still in flight publishes nothing.
    stopped: bool,
    /// The `JobRunState` the run must come to rest in, which is the `state` the event carries.
    state: &'static str,
    /// The `severity` the event must carry.
    severity: &'static str,
    /// Whether the event must carry the run's own `message`.
    carries_message: bool,
}

/// The rows: every terminal state a run of this emulator reaches, and the event each one
/// publishes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a script that exits cleanly",
            script: Some(EXITS_CLEANLY),
            stopped: false,
            state: JOB_RUN_STATE_SUCCEEDED,
            severity: SEVERITY_INFO,
            carries_message: false,
        },
        Row {
            case: "a script that raises",
            script: Some(RAISES),
            stopped: false,
            state: JOB_RUN_STATE_FAILED,
            severity: SEVERITY_ERROR,
            carries_message: true,
        },
        Row {
            case: "a job naming no script at all",
            script: None,
            stopped: false,
            state: JOB_RUN_STATE_FAILED,
            severity: SEVERITY_ERROR,
            carries_message: true,
        },
        Row {
            // The run is held in flight and stopped there, so this row needs no script: a
            // stopped run comes to rest having failed at nothing, whatever it was running.
            case: "a run a client stopped",
            script: None,
            stopped: true,
            state: JOB_RUN_STATE_STOPPED,
            severity: SEVERITY_INFO,
            carries_message: false,
        },
    ]
}

/// Every terminal state a job run reaches is published onto the default bus as a `Glue Job
/// State Change` event, delivered to the matching rule's granted target — and withheld from
/// the queue that granted EventBridge nothing and from the rule that does not match.
#[tokio::test]
async fn a_job_run_that_comes_to_rest_publishes_a_glue_job_state_change() {
    let (router, engine) = emulator_with_a_bus();
    create_bucket(&router).await;
    let (granted_url, granted_arn) = given_queue(&router, "glue-job-events").await;
    let (ungranted_url, ungranted_arn) = given_queue(&router, "glue-job-events-no-policy").await;
    let (unrelated_url, unrelated_arn) = given_queue(&router, "unrelated-events").await;
    grant_delivery(&router, &granted_url, &granted_arn).await;
    grant_delivery(&router, &unrelated_url, &unrelated_arn).await;
    given_rule(
        &router,
        MATCHING_RULE,
        json!({ "source": [EVENT_SOURCE], "detail-type": [JOB_STATE_CHANGE] }),
        &[granted_arn, ungranted_arn],
    )
    .await;
    given_rule(
        &router,
        OTHER_RULE,
        json!({ "source": ["aws.ecs"], "detail-type": ["ECS Task State Change"] }),
        &[unrelated_arn],
    )
    .await;

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let job_name = format!("job_event_{index}");
        let mut command = json!({ "Name": "pythonshell", "PythonVersion": "3.9" });
        if let Some(script) = row.script {
            let location = stage_script(&router, &format!("{SCRIPTS}{job_name}.py"), script).await;
            command["ScriptLocation"] = json!(location);
        }
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": job_name,
                "Role": "arn:aws:iam::000000000000:role/glue-job-events",
                "Command": command,
            }),
        )
        .await;

        // The run is held in flight first, for the row that stops it there: a run that has
        // come to rest at nothing publishes nothing, and the stop needs a run to reach.
        let started = ok(&router, "StartJobRun", &json!({ "JobName": job_name })).await;
        let run_id = started["JobRunId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: StartJobRun must answer a JobRunId"))
            .to_string();
        if row.stopped {
            set_due_instant(&engine, &job_name, far_future());
            let in_flight = ok(
                &router,
                "GetJobRun",
                &json!({ "JobName": job_name, "RunId": run_id }),
            )
            .await;
            assert_eq!(
                in_flight["JobRun"]["JobRunState"].as_str().unwrap_or_default(),
                JOB_RUN_STATE_STARTING,
                "case {case:?}: the run must still be in flight to be stopped, got {in_flight}"
            );
            let delivered = drain(&router, &granted_url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a run still in flight publishes nothing, got {delivered:?}"
            );
            ok(
                &router,
                "BatchStopJobRun",
                &json!({ "JobName": job_name, "JobRunIds": [run_id.clone()] }),
            )
            .await;
        }
        // The run is read back until it HAS come to rest: whichever call finds a run still
        // owed its script's ONE execution performs it — the start, or a later read when no
        // interpreter was free for the start — and that execution records an owed span of its
        // own, so a single read made after the start's span is forced due can still report a
        // run in flight.
        let run = run_at_rest(&router, &engine, case, &job_name, &run_id).await;
        assert_eq!(
            run["JobRunState"].as_str().unwrap_or_default(),
            row.state,
            "case {case:?}: expected the run to come to rest in {}, got {run}",
            row.state
        );

        // The clock pump publishes what it materialises, so the event is waited for rather
        // than expected at an instant.
        let event = delivered_event(&router, case, &granted_url).await;
        assert_eq!(
            event["source"], json!(EVENT_SOURCE),
            "case {case:?}: expected source {EVENT_SOURCE:?}, got {event}"
        );
        assert_eq!(
            event["detail-type"], json!(JOB_STATE_CHANGE),
            "case {case:?}: expected detail-type {JOB_STATE_CHANGE:?}, got {event}"
        );
        let detail = &event["detail"];
        assert_eq!(
            detail["jobName"], json!(job_name),
            "case {case:?}: the detail must name the job, got {detail}"
        );
        assert_eq!(
            detail["jobRunId"], json!(run_id),
            "case {case:?}: the detail must name the run, got {detail}"
        );
        assert_eq!(
            detail["state"], json!(row.state),
            "case {case:?}: expected state {:?}, got {detail}",
            row.state
        );
        assert_eq!(
            detail["severity"], json!(row.severity),
            "case {case:?}: expected severity {:?}, got {detail}",
            row.severity
        );
        // The reason the event reports is the run's own `ErrorMessage` — the same one
        // `GetJobRun` answers with — and a run that failed at nothing reports none.
        assert_eq!(
            detail.get("message").and_then(Value::as_str),
            run.get("ErrorMessage").and_then(Value::as_str),
            "case {case:?}: the event's message must be the run's own, got {detail} for {run}"
        );
        assert_eq!(
            detail.get("message").is_some(),
            row.carries_message,
            "case {case:?}: expected a message={}, got {detail}",
            row.carries_message
        );

        // Read again: the run has already been published, so nothing is published twice.
        ok(
            &router,
            "GetJobRun",
            &json!({ "JobName": job_name, "RunId": run_id }),
        )
        .await;
        let again = drain(&router, &granted_url).await;
        assert!(
            again.is_empty(),
            "case {case:?}: a run already published is not published again, got {again:?}"
        );
    }

    // The negative half of the delivery contract: the sibling target that granted
    // EventBridge nothing received none of the events, and neither did the rule that does
    // not match them.
    let withheld = drain(&router, &ungranted_url).await;
    assert!(
        withheld.is_empty(),
        "a target queue with no resource policy receives nothing, got {withheld:?}"
    );
    let unrelated = drain(&router, &unrelated_url).await;
    assert!(
        unrelated.is_empty(),
        "a rule that does not match receives nothing, got {unrelated:?}"
    );
}
