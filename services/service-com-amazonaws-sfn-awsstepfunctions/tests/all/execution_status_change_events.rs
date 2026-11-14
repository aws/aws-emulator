// SPDX-License-Identifier: CC0-1.0

//! Step Functions publishes one EventBridge event per status an execution enters.
//!
//! The RULE under test: every status an execution of a STANDARD workflow MOVES INTO is
//! published once onto the account's default EventBridge bus as `source` `aws.states`
//! with `detail-type` `Step Functions Execution Status Change`, carrying the execution's
//! ARN in `resources` and, in the event `detail`, the same facts `DescribeExecution`
//! reports for it: `executionArn`, `stateMachineArn`, `name`, `status`, `startDate`,
//! `stopDate`, `input`/`inputDetails`, `output`/`outputDetails`, `error`, `cause` and the
//! redrive accounting. EventBridge then routes it: a rule whose `EventPattern` matches
//! delivers one message per event to its SQS queue target, which a client reads with
//! `ReceiveMessage`, and a rule that does not match receives nothing.
//!
//! Three bounds belong to the event rather than to the execution, and are asserted here
//! too:
//!
//! * a rule scoped to ONE `detail.status` receives that status and none of its siblings;
//! * an EXPRESS workflow publishes no execution status-change event at all, while a
//!   STANDARD twin of the same definition does;
//! * a datum too large for one event is OMITTED and its details member reports
//!   `"included": false` — "indicates whether input or output was included in the
//!   response"
//!   (`smithy_query(query="doc",shape="CloudWatchEventsExecutionDataDetails",member="included")`)
//!   — rather than being truncated into the event.
//!
//! The detail also reports whether `RedriveExecution` would redrive the execution, which
//! must be the same `redriveStatus` a `DescribeExecution` of it reports, so a client
//! reading the event decides what a describe would have told it.
//!
//! The rule is asserted over TABLES that vary what it generalises over — which terminal
//! status the execution reaches, which rule a status is scoped to, which workflow type
//! runs it, and how large the execution's data is — because the rule is about every
//! status of every execution, not about one shape of workflow. Nothing in these tests
//! calls Step Functions after the start except the stop and the redrive under test, and
//! the one describe whose redrive projection an event must agree with: the events are read
//! from the queue the rule targets, so an event that arrives was published by the service
//! carrying the execution on its own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::state::events::MAX_EVENT_DATA_BYTES;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// The `source` and the `detail-type` Step Functions publishes an execution status
/// change under, spelled out here rather than imported from the implementation, so a
/// wrong constant in the service cannot make a test pass.
pub(crate) const EVENT_SOURCE: &str = "aws.states";
pub(crate) const EXECUTION_STATUS_CHANGE: &str = "Step Functions Execution Status Change";

/// How long one poll of a rule's queue waits before reading again.
const POLL: Duration = Duration::from_millis(25);

/// How long a suite waits for the events a run owes before giving up.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How long a suite watches a queue that must stay EMPTY.
///
/// Long enough that an execution's own terminal status has come due and been published
/// for its twin, so an empty queue is a suppressed event rather than one still in
/// flight.
const SILENCE_WINDOW: Duration = Duration::from_secs(3);

/// A definition that succeeds immediately with a static result.
const SUCCEEDS: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// A definition whose `Fail` state ends the execution with its own error and cause.
const FAILS: &str = r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// The error and cause that `Fail` state carries, which the FAILED event must report.
const FAIL_ERROR: &str = "Boom";
const FAIL_CAUSE: &str = "on purpose";

/// A definition that keeps the execution RUNNING long enough to be stopped.
const WAITS: &str = r#"{"StartAt":"Hold","States":{"Hold":{"Type":"Wait","Seconds":30,"Next":"Done"},"Done":{"Type":"Pass","End":true}}}"#;

/// The source a callback task's own event is published under, and the output the reply
/// that quotes its token reports.
const TOKEN_SOURCE: &str = "com.example.tokens";
const REPLY_OUTPUT: &str = r#"{"approved":true}"#;

/// One emulator: Step Functions, EventBridge, SQS and Lambda mounted on ONE shared engine,
/// with the router that dispatches to all four.
///
/// EventBridge and SQS are mounted BESIDE Step Functions because an execution's status
/// changes are published onto the default event bus through the in-process callback and a
/// rule on that bus delivers them to its SQS queue target: with neither mounted there is
/// nowhere to publish, so the path under test would not be exercised at all. Lambda is
/// mounted for the sibling suite that drives a rule's FUNCTION target
/// ([`Self::lambda`]). Each service claims only the requests scoped to its own signing
/// name, so no Step Functions request below is routed any differently for their presence.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", &host)
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc("states", "AWSStepFunctions", "1.0", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// Call one EventBridge operation, requiring a 200.
    pub(crate) async fn events(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc("events", "AWSEvents", "1.1", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// Call one SQS operation, requiring a 200.
    pub(crate) async fn sqs(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc("sqs", "AmazonSQS", "1.0", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One Lambda call, whose protocol is `restJson1` rather than an AWS JSON RPC, as its
    /// status and parsed body: how the sibling suite creates the function an EventBridge
    /// rule's target invokes and grants that service the invocation.
    pub(crate) async fn lambda(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let host = format!("lambda.{REGION}.amazonaws.com");
        let payload = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", &host)
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// An SQS queue called `name`, carrying the grant a rule's target needs — EventBridge
/// makes the target's `sqs:SendMessage` as its own service principal and "relies on
/// resource-based policies" for a queue (`PutTargets`), so a queue that has granted it
/// nothing receives nothing. Answers its url and the ARN a rule names it by.
pub(crate) async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator
        .sqs("CreateQueue", &json!({ "QueueName": name }))
        .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = emulator
        .sqs(
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    emulator
        .sqs(
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
    (url, arn)
}

/// A rule called `name` on the default bus, matching `pattern`, with its own queue as its
/// only target. Answers the queue's url, which is where the events it matches arrive.
pub(crate) async fn given_rule(emulator: &Emulator, name: &str, pattern: &Value) -> String {
    let (url, queue_arn) = given_queue(emulator, &format!("{name}-queue")).await;
    emulator
        .events(
            "PutRule",
            &json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
        )
        .await;
    emulator
        .events(
            "PutTargets",
            &json!({ "Rule": name, "Targets": [{ "Id": "t1", "Arn": queue_arn }] }),
        )
        .await;
    url
}

/// Create a state machine of `sm_type` and answer its ARN.
pub(crate) async fn given_machine(
    emulator: &Emulator,
    name: &str,
    definition: &str,
    sm_type: &str,
) -> String {
    let created = emulator
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition,
                "roleArn": ROLE_ARN,
                "type": sm_type,
            }),
        )
        .await;
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"))
        .to_string()
}

/// Start an execution and answer its ARN.
pub(crate) async fn given_execution(
    emulator: &Emulator,
    state_machine_arn: &str,
    name: &str,
    input: &str,
) -> String {
    let started = emulator
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": state_machine_arn, "name": name, "input": input }),
        )
        .await;
    started["executionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no executionArn in {started}"))
        .to_string()
}

/// Read `url` until it has delivered `want` events or the deadline passes, and answer
/// every event it delivered, oldest first.
///
/// A message is left out of the queue once read, so events accumulate across polls.
async fn delivered(emulator: &Emulator, url: &str, want: usize, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut events: Vec<Value> = Vec::new();
    while events.len() < want && Instant::now() < deadline {
        events.extend(read_once(emulator, url, case).await);
        if events.len() < want {
            ::tokio::time::sleep(POLL).await;
        }
    }
    events
}

/// Every event `url` holds right now, oldest first.
pub(crate) async fn read_once(emulator: &Emulator, url: &str, case: &str) -> Vec<Value> {
    let received = emulator
        .sqs(
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0 }),
        )
        .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let body = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(body).unwrap_or_else(|error| {
                panic!("case {case}: the delivered body is not JSON ({error}): {body}")
            })
        })
        .collect()
}

/// Watch `url` for [`SILENCE_WINDOW`] and answer everything it delivered in that time —
/// empty for a rule nothing matched.
async fn nothing_delivered(emulator: &Emulator, url: &str, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + SILENCE_WINDOW;
    let mut events: Vec<Value> = Vec::new();
    while Instant::now() < deadline {
        events.extend(read_once(emulator, url, case).await);
        ::tokio::time::sleep(POLL).await;
    }
    events
}

/// The `detail` of every event of `execution_arn` among `events`, oldest first, with the
/// envelope of each one asserted: a Step Functions execution status change names this
/// service as its source, the status change as its detail type, and the execution it is
/// about in `resources`.
fn details_of(events: &[Value], execution_arn: &str, case: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event["detail"]["executionArn"].as_str() == Some(execution_arn))
        .map(|event| {
            assert_eq!(
                event["source"].as_str(),
                Some(EVENT_SOURCE),
                "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["detail-type"].as_str(),
                Some(EXECUTION_STATUS_CHANGE),
                "case {case:?}: expected detail-type {EXECUTION_STATUS_CHANGE}, got {event}"
            );
            assert_eq!(
                event["resources"],
                json!([execution_arn]),
                "case {case:?}: expected the execution in resources, got {event}"
            );
            event["detail"].clone()
        })
        .collect()
}

/// The `status` of each detail, in order.
fn statuses(details: &[Value]) -> Vec<String> {
    details
        .iter()
        .map(|detail| detail["status"].as_str().unwrap_or("<none>").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Every status an execution enters is published once
// ---------------------------------------------------------------------------

/// One row: a run, the statuses it must publish, and what its terminal event's `error`
/// and `cause` must carry.
struct Run {
    case: &'static str,
    definition: &'static str,
    /// Whether the run is ended by `StopExecution`, which carries its own error and
    /// cause, rather than by its own definition.
    stopped: bool,
    statuses: &'static [&'static str],
    error: Option<&'static str>,
    cause: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// A task that publishes its own token
// ---------------------------------------------------------------------------

/// A `Task` state that publishes an event is a client of the same bus: the event it
/// publishes reaches the rule's target with the document the state built as its
/// `detail` — including the task token a callback task carries — and the reply that
/// quotes that token finishes the execution, whose terminal status is then published
/// too.
///
/// The RULE this pins beside the publication itself: a member the invoked operation
/// types as a STRING goes on the wire as the JSON text of the document the state built,
/// wherever in that request the member sits. `PutEventsRequestEntry$Detail` is such a
/// member — a String documented as "a valid JSON object" — nested inside the entry list,
/// and a request that carried it as an object would be refused by EventBridge instead of
/// publishing anything.
#[tokio::test]
async fn a_task_that_publishes_its_token_is_resumed_by_the_reply_that_quotes_it() {
    let emulator = Emulator::new();
    let token_events = given_rule(
        &emulator,
        "token-events",
        &json!({ "source": [TOKEN_SOURCE] }),
    )
    .await;
    let status_changes = given_rule(
        &emulator,
        "token-producer-status",
        &json!({ "source": [EVENT_SOURCE] }),
    )
    .await;

    // The task publishes one event whose detail carries the token the context object
    // (`$$.Task.Token`) holds, and waits for a reply quoting it.
    let definition = json!({
        "StartAt": "Notify",
        "States": {
            "Notify": {
                "Type": "Task",
                "Resource": "arn:aws:states:::events:putEvents.waitForTaskToken",
                "Parameters": {
                    "Entries": [{
                        "Source": TOKEN_SOURCE,
                        "DetailType": "Token",
                        "Detail": { "token.$": "$$.Task.Token", "note": "please approve" },
                    }],
                },
                "End": true,
            },
        },
    })
    .to_string();
    let machine = given_machine(&emulator, "token-producer", &definition, "STANDARD").await;
    let execution = given_execution(&emulator, &machine, "producer", "{}").await;

    let case = "the token event";
    let published = delivered(&emulator, &token_events, 1, case).await;
    let event = published
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: no event carrying a task token arrived"));
    assert_eq!(
        event["detail"]["note"].as_str(),
        Some("please approve"),
        "case {case:?}: the event carries the document the state built, got {event}"
    );
    let token = event["detail"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no task token in {event}"))
        .to_string();

    // The execution is RUNNING until a reply quotes that token, which is the one the
    // event really carried.
    let running = emulator
        .sfn("DescribeExecution", &json!({ "executionArn": &execution }))
        .await;
    assert_eq!(
        running["status"].as_str(),
        Some("RUNNING"),
        "case {case:?}: a callback task waits for its reply, got {running}"
    );
    emulator
        .sfn(
            "SendTaskSuccess",
            &json!({ "taskToken": token, "output": REPLY_OUTPUT }),
        )
        .await;

    let details = details_of(
        &delivered(&emulator, &status_changes, 2, case).await,
        &execution,
        case,
    );
    assert_eq!(
        statuses(&details),
        ["RUNNING", "SUCCEEDED"],
        "case {case:?}: the reply finishes the execution, got {details:?}"
    );
    let terminal = details
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
    assert_eq!(
        terminal["output"].as_str(),
        Some(REPLY_OUTPUT),
        "case {case:?}: the terminal event carries the reply's output, got {terminal:?}"
    );
}

/// Every status an execution moves into is published once, in the order the execution
/// entered them, with the execution's own identity and data in the event's detail — over
/// a table of runs that reach each terminal status a different way.
#[tokio::test]
async fn every_status_an_execution_enters_is_published_once() {
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "states-status-changes",
        &json!({ "source": [EVENT_SOURCE] }),
    )
    .await;

    let rows = [
        Run {
            case: "a run that succeeds",
            definition: SUCCEEDS,
            stopped: false,
            statuses: &["RUNNING", "SUCCEEDED"],
            error: None,
            cause: None,
        },
        Run {
            case: "a run that reaches a Fail state",
            definition: FAILS,
            stopped: false,
            statuses: &["RUNNING", "FAILED"],
            error: Some(FAIL_ERROR),
            cause: Some(FAIL_CAUSE),
        },
        Run {
            case: "a run a stop aborts",
            definition: WAITS,
            stopped: true,
            statuses: &["RUNNING", "ABORTED"],
            error: Some("StoppedByOperator"),
            cause: Some("the operator stopped it"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine = given_machine(
            &emulator,
            &format!("status-events-{index}"),
            row.definition,
            "STANDARD",
        )
        .await;
        let input = json!({ "row": index }).to_string();
        let execution = given_execution(&emulator, &machine, "run", &input).await;
        if row.stopped {
            // The stop is the call that moves this execution, and it carries the error
            // and cause the ABORTED event must echo.
            emulator
                .sfn(
                    "StopExecution",
                    &json!({
                        "executionArn": &execution,
                        "error": row.error,
                        "cause": row.cause,
                    }),
                )
                .await;
        }
        let events = delivered(&emulator, &url, row.statuses.len(), case).await;
        let details = details_of(&events, &execution, case);
        assert_eq!(
            statuses(&details),
            row.statuses,
            "case {case:?}: expected the statuses {:?}, got {details:?}",
            row.statuses
        );

        let terminal = details
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
        assert_eq!(
            terminal["executionArn"].as_str(),
            Some(execution.as_str()),
            "case {case:?}: expected the execution's own ARN, got {terminal:?}"
        );
        assert_eq!(
            terminal["stateMachineArn"].as_str(),
            Some(machine.as_str()),
            "case {case:?}: expected the state machine's ARN, got {terminal:?}"
        );
        assert_eq!(
            terminal["name"].as_str(),
            Some("run"),
            "case {case:?}: expected the execution's name, got {terminal:?}"
        );
        assert_eq!(
            terminal["input"].as_str(),
            Some(input.as_str()),
            "case {case:?}: expected the execution's input, got {terminal:?}"
        );
        assert_eq!(
            terminal["inputDetails"],
            json!({ "included": true }),
            "case {case:?}: expected the input to be reported as included, got {terminal:?}"
        );
        assert_eq!(
            terminal["error"].as_str(),
            row.error,
            "case {case:?}: expected error {:?}, got {terminal:?}",
            row.error
        );
        assert_eq!(
            terminal["cause"].as_str(),
            row.cause,
            "case {case:?}: expected cause {:?}, got {terminal:?}",
            row.cause
        );
        // An execution that has finished reports when it did, and one that produced no
        // output reports none and no details for one.
        assert!(
            terminal["stopDate"].as_i64().unwrap_or_default()
                >= terminal["startDate"].as_i64().unwrap_or_default(),
            "case {case:?}: expected a stopDate at or after the startDate, got {terminal:?}"
        );
        match row.error {
            None => assert!(
                terminal["output"].is_string()
                    && terminal["outputDetails"] == json!({ "included": true }),
                "case {case:?}: expected the output to be reported as included, got {terminal:?}"
            ),
            Some(_) => assert!(
                terminal["output"].is_null() && terminal["outputDetails"].is_null(),
                "case {case:?}: an execution with no output reports none, got {terminal:?}"
            ),
        }
        // The first event is the start, which has no terminal facts yet.
        let running = details
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
        assert!(
            running["stopDate"].is_null() && running["error"].is_null(),
            "case {case:?}: a RUNNING execution carries no terminal facts, got {running:?}"
        );
        assert_eq!(
            running["redriveCount"].as_i64(),
            Some(0),
            "case {case:?}: an execution nothing has redriven reports redriveCount 0, got \
             {running:?}"
        );
        assert_eq!(
            running["redriveDate"],
            Value::Null,
            "case {case:?}: an execution nothing has redriven reports no redriveDate, got \
             {running:?}"
        );
        // A RUNNING execution is one `RedriveExecution` refuses, and the event says so
        // with the reason that call's own documentation publishes.
        assert_eq!(
            running["redriveStatus"].as_str(),
            Some("NOT_REDRIVABLE"),
            "case {case:?}: a RUNNING execution cannot be redriven, got {running:?}"
        );
        assert_eq!(
            running["redriveStatusReason"].as_str(),
            Some("Execution is RUNNING and cannot be redriven"),
            "case {case:?}: expected the published reason, got {running:?}"
        );
        // The redrive projection the event carries is the one a describe of the same
        // execution reports: both are the store's own predicate.
        let described = emulator
            .sfn("DescribeExecution", &json!({ "executionArn": &execution }))
            .await;
        assert_eq!(
            terminal["redriveStatus"].as_str(),
            described["redriveStatus"].as_str(),
            "case {case:?}: the event and the describe must agree on redriveStatus, got \
             {terminal:?} and {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// One status per rule
// ---------------------------------------------------------------------------

/// A rule scoped to one `detail.status` receives that status and none of its siblings,
/// over a table that scopes a rule to each status a succeeding and a failing run move
/// through.
#[tokio::test]
async fn a_rule_scoped_to_one_status_receives_only_that_status() {
    let emulator = Emulator::new();
    let scoped = ["RUNNING", "SUCCEEDED", "FAILED"];
    let mut queues = Vec::new();
    for status in scoped {
        let url = given_rule(
            &emulator,
            &format!("states-{}", status.to_lowercase()),
            &json!({
                "source": [EVENT_SOURCE],
                "detail-type": [EXECUTION_STATUS_CHANGE],
                "detail": { "status": [status] },
            }),
        )
        .await;
        queues.push((status, url));
    }

    let succeeding = given_machine(&emulator, "per-status-succeeds", SUCCEEDS, "STANDARD").await;
    let failing = given_machine(&emulator, "per-status-fails", FAILS, "STANDARD").await;
    let succeeded = given_execution(&emulator, &succeeding, "ok", "{}").await;
    let failed = given_execution(&emulator, &failing, "bang", "{}").await;

    // Two executions move through three statuses between them: RUNNING twice, and one
    // each of SUCCEEDED and FAILED.
    let expected: &[(&str, Vec<&str>)] = &[
        ("RUNNING", vec!["RUNNING", "RUNNING"]),
        ("SUCCEEDED", vec!["SUCCEEDED"]),
        ("FAILED", vec!["FAILED"]),
    ];
    for (status, url) in &queues {
        let want = expected
            .iter()
            .find(|(scoped, _)| scoped == status)
            .map(|(_, wanted)| wanted.len())
            .unwrap_or_default();
        let events = delivered(&emulator, url, want, status).await;
        let mut seen: Vec<String> = events
            .iter()
            .map(|event| {
                assert_eq!(
                    event["detail-type"].as_str(),
                    Some(EXECUTION_STATUS_CHANGE),
                    "case {status:?}: expected a status-change event, got {event}"
                );
                let about = event["detail"]["executionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    about == succeeded || about == failed,
                    "case {status:?}: an event about an execution this test never started: \
                     {event}"
                );
                event["detail"]["status"]
                    .as_str()
                    .unwrap_or("<none>")
                    .to_string()
            })
            .collect();
        seen.sort();
        let mut wanted: Vec<String> = expected
            .iter()
            .find(|(scoped, _)| scoped == status)
            .map(|(_, wanted)| wanted.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        wanted.sort();
        assert_eq!(
            seen, wanted,
            "case {status:?}: a rule scoped to one status receives only it, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// EXPRESS publishes nothing
// ---------------------------------------------------------------------------

/// An EXPRESS workflow publishes no execution status-change event, while a STANDARD twin
/// of the same definition publishes every status it enters.
#[tokio::test]
async fn an_express_execution_publishes_no_status_change_while_a_standard_twin_does() {
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "states-both-types",
        &json!({ "source": [EVENT_SOURCE] }),
    )
    .await;

    let standard = given_machine(&emulator, "twin-standard", SUCCEEDS, "STANDARD").await;
    let express = given_machine(&emulator, "twin-express", SUCCEEDS, "EXPRESS").await;
    let standard_execution = given_execution(&emulator, &standard, "twin", "{}").await;
    let express_execution = given_execution(&emulator, &express, "twin", "{}").await;

    // The STANDARD twin's own statuses are what tells a suppressed Express event apart
    // from one still in flight: by the time both of them have arrived, the Express
    // execution has finished too.
    let events = delivered(&emulator, &url, 2, "the STANDARD twin").await;
    let standard_details = details_of(&events, &standard_execution, "the STANDARD twin");
    assert_eq!(
        statuses(&standard_details),
        ["RUNNING", "SUCCEEDED"],
        "case \"the STANDARD twin\": expected both of its statuses, got {standard_details:?}"
    );

    let mut all = events;
    all.extend(nothing_delivered(&emulator, &url, "the EXPRESS twin").await);
    let express_details = details_of(&all, &express_execution, "the EXPRESS twin");
    assert!(
        express_details.is_empty(),
        "case \"the EXPRESS twin\": an EXPRESS workflow publishes no execution status \
         change, got {express_details:?}"
    );
}

// ---------------------------------------------------------------------------
// The event's size bound
// ---------------------------------------------------------------------------

/// A datum too large for one event is omitted and reported as not included, while one
/// that fits is carried — over a table that varies the size of the datum against the room
/// the event leaves for it, at the bound and one byte past it.
#[tokio::test]
async fn a_datum_too_large_for_one_event_is_omitted_and_reported_as_not_included() {
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "states-oversize-data",
        &json!({ "source": [EVENT_SOURCE] }),
    )
    .await;

    // The room one event leaves for one datum, which is the bound under test: a JSON
    // string of `MAX_EVENT_DATA_BYTES - 2` characters is a document of exactly that many
    // bytes, and one character more is a document one byte past it.
    let at_bound = MAX_EVENT_DATA_BYTES - 2;
    let rows: &[(&str, usize, bool)] = &[
        ("an input of a few bytes", 8, true),
        ("an input of 64 KiB", 64 * 1024, true),
        ("an input of exactly the event's room for it", at_bound, true),
        ("an input one byte past that room", at_bound + 1, false),
        // The size the recorded case this issue was filed about starts with.
        ("an input of ~250 KiB", 250 * 1024, false),
    ];
    for (index, (case, size, included)) in rows.iter().enumerate() {
        let machine = given_machine(
            &emulator,
            &format!("oversize-input-{index}"),
            SUCCEEDS,
            "STANDARD",
        )
        .await;
        let input = format!("\"{}\"", "x".repeat(*size));
        let execution = given_execution(&emulator, &machine, "sized", &input).await;
        let events = delivered(&emulator, &url, 2, case).await;
        let details = details_of(&events, &execution, case);
        let running = details
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
        assert_eq!(
            running["inputDetails"],
            json!({ "included": included }),
            "case {case:?}: expected inputDetails.included {included} for an input of {} \
             bytes, got {}",
            input.len(),
            running["inputDetails"]
        );
        match included {
            true => assert_eq!(
                running["input"].as_str(),
                Some(input.as_str()),
                "case {case:?}: an input the event has room for is carried, got {} bytes",
                running["input"].as_str().unwrap_or_default().len()
            ),
            false => assert!(
                running.get("input").is_none(),
                "case {case:?}: an input too large for the event is omitted, not truncated \
                 into it, got {} bytes",
                running["input"].as_str().unwrap_or_default().len()
            ),
        }
    }

    // The same bound on the OUTPUT: a `Pass` state whose result is a string of this size
    // ends the execution with an output no event has room for.
    let oversize_output = format!(
        r#"{{"StartAt":"Big","States":{{"Big":{{"Type":"Pass","Result":"{}","End":true}}}}}}"#,
        "y".repeat(250 * 1024)
    );
    let machine = given_machine(&emulator, "oversize-output", &oversize_output, "STANDARD").await;
    let case = "an output of ~250 KiB";
    let execution = given_execution(&emulator, &machine, "sized", "{}").await;
    let events = delivered(&emulator, &url, 2, case).await;
    let details = details_of(&events, &execution, case);
    let terminal = details
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
    assert_eq!(
        terminal["status"].as_str(),
        Some("SUCCEEDED"),
        "case {case:?}: expected the terminal event, got {terminal:?}"
    );
    assert_eq!(
        terminal["outputDetails"],
        json!({ "included": false }),
        "case {case:?}: expected outputDetails.included false, got {}",
        terminal["outputDetails"]
    );
    assert!(
        terminal["output"].is_null(),
        "case {case:?}: an output too large for the event is omitted, not truncated into it, \
         got {} bytes",
        terminal["output"].as_str().unwrap_or_default().len()
    );

    // The room is SHARED, because one event carries both data: an input and an output
    // that each fit on their own but not together leave the second one out.
    let half = MAX_EVENT_DATA_BYTES / 2 + 1_024;
    let both = format!(
        r#"{{"StartAt":"Half","States":{{"Half":{{"Type":"Pass","Result":"{}","End":true}}}}}}"#,
        "z".repeat(half)
    );
    let machine = given_machine(&emulator, "both-data", &both, "STANDARD").await;
    let case = "an input and an output that do not fit one event together";
    let input = format!("\"{}\"", "w".repeat(half));
    let execution = given_execution(&emulator, &machine, "sized", &input).await;
    let events = delivered(&emulator, &url, 2, case).await;
    let details = details_of(&events, &execution, case);
    let terminal = details
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: no event was delivered"));
    assert_eq!(
        terminal["inputDetails"],
        json!({ "included": true }),
        "case {case:?}: the input is charged first, so it is carried, got {}",
        terminal["inputDetails"]
    );
    assert_eq!(
        terminal["outputDetails"],
        json!({ "included": false }),
        "case {case:?}: the output is charged against what is left, got {}",
        terminal["outputDetails"]
    );
}

// ---------------------------------------------------------------------------
// A task's own document-valued member
// ---------------------------------------------------------------------------

/// A member the invoked operation types as a STRING carries the JSON TEXT of the document
/// a `Task` state's `Parameters` built, wherever in the request that member sits: nested
/// inside a list for `events:putEvents`' `Entries[].Detail` (asserted by the token suite
/// above) and at the top level for `sqs:sendMessage`' s `MessageBody`, which that
/// operation's model types as a String.
#[tokio::test]
async fn a_document_the_invoked_operation_types_as_a_string_is_sent_as_its_json_text() {
    let emulator = Emulator::new();
    let (url, _) = given_queue(&emulator, "task-messages").await;
    let body = json!({ "greeting": "hello", "nested": { "count": 2 } });
    let definition = json!({
        "StartAt": "Send",
        "States": {
            "Send": {
                "Type": "Task",
                "Resource": "arn:aws:states:::sqs:sendMessage",
                "Parameters": { "QueueUrl": &url, "MessageBody": body.clone() },
                "End": true,
            },
        },
    })
    .to_string();
    let machine = given_machine(&emulator, "message-sender", &definition, "STANDARD").await;
    let case = "a MessageBody written as a document";
    given_execution(&emulator, &machine, "sender", "{}").await;

    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut sent: Vec<String> = Vec::new();
    while sent.is_empty() && Instant::now() < deadline {
        let received = emulator
            .sqs(
                "ReceiveMessage",
                &json!({ "QueueUrl": &url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0 }),
            )
            .await;
        for message in received["Messages"].as_array().cloned().unwrap_or_default() {
            sent.push(message["Body"].as_str().unwrap_or_default().to_string());
        }
        ::tokio::time::sleep(POLL).await;
    }
    let delivered = sent
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the task's message never arrived"));
    assert_eq!(
        serde_json::from_str::<Value>(delivered).ok(),
        Some(body.clone()),
        "case {case:?}: the message body is the JSON text of the document the state built, \
         got {delivered}"
    );
}

// ---------------------------------------------------------------------------
// A redrive publishes its own statuses
// ---------------------------------------------------------------------------

/// A redriven execution publishes a further status change, carrying the redrive
/// accounting the re-run reached, and then the status the re-run ended on.
#[tokio::test]
async fn a_redriven_execution_publishes_its_redrive_accounting() {
    let emulator = Emulator::new();
    let url = given_rule(
        &emulator,
        "states-redrives",
        &json!({ "source": [EVENT_SOURCE] }),
    )
    .await;

    let machine = given_machine(&emulator, "redriven", FAILS, "STANDARD").await;
    let execution = given_execution(&emulator, &machine, "attempt", "{}").await;
    let first = delivered(&emulator, &url, 2, "the first attempt").await;
    assert_eq!(
        statuses(&details_of(&first, &execution, "the first attempt")),
        ["RUNNING", "FAILED"],
        "case \"the first attempt\": expected the run to fail, got {first:?}"
    );

    // "You can redrive executions if your original execution meets the following
    // conditions: [it] has not succeeded": the attempt above failed, so this one redrives
    // it — the only call this test makes after the start.
    ::tokio::time::sleep(PAST_THE_EXECUTION_DELAY).await;
    emulator
        .sfn("RedriveExecution", &json!({ "executionArn": &execution }))
        .await;

    let redriven = delivered(&emulator, &url, 2, "the redrive").await;
    let details = details_of(&redriven, &execution, "the redrive");
    assert_eq!(
        statuses(&details),
        ["RUNNING", "FAILED"],
        "case \"the redrive\": expected the re-run's own statuses, got {details:?}"
    );
    for detail in &details {
        assert_eq!(
            detail["redriveCount"].as_i64(),
            Some(1),
            "case \"the redrive\": expected redriveCount 1, got {detail:?}"
        );
        assert!(
            detail["redriveDate"].as_i64().unwrap_or_default()
                >= detail["startDate"].as_i64().unwrap_or_default(),
            "case \"the redrive\": expected a redriveDate at or after the startDate, got \
             {detail:?}"
        );
    }
    let terminal = details
        .last()
        .unwrap_or_else(|| panic!("case \"the redrive\": no event was delivered"));
    assert_eq!(
        terminal["error"].as_str(),
        Some(FAIL_ERROR),
        "case \"the redrive\": the re-run reports the Fail state's error, got {terminal:?}"
    );
}
