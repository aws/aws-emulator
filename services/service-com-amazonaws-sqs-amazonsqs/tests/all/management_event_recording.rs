// SPDX-License-Identifier: CC0-1.0

//! Which of this service's calls are MANAGEMENT EVENTS of the account, read off the shared
//! event history the serving path writes them to.
//!
//! RULE: every CONTROL-PLANE call an account's APIs serve — one that creates, configures,
//! inspects or removes a resource — is recorded as a management event of the serving
//! service, carrying that service's `eventSource`, the Region the call was served in, the
//! operation's own name as `eventName`, the parameters it was dispatched with and
//! `readOnly` from whether the call reports state or changes it. Moving a message through a
//! queue that already exists changes no resource of the account and is not a management
//! event, so the per-message data plane is recorded nowhere.
//!
//! That record is the one document the account's two readers of its management events see:
//! CloudTrail's `LookupEvents` reports it, and — while a trail of the Region is logging
//! calls of its class — the record of a MUTATING call is also the `detail` of the `AWS API
//! Call via CloudTrail` event
//! published onto the account's default event bus, which is what an EventBridge rule on
//! `aws.sqs` matches. The fan-out itself crosses four services and is pinned in
//! `core/router/tests/all/cloudtrail_api_call_events_by_region.rs`; what is pinned HERE is
//! this service's own half: which calls are recorded, and what each record carries.
//!
//! The rows span every operation this service serves — the creates, the configuration
//! writes, the readers, the tag writes, the redrive tasks, the delete, and all seven of the
//! message data-plane operations — because the rule generalises over the KIND of call and
//! not over any one of them.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::support::{
    account, call_in, create_ok, emulator_over_engine, expected_arn, messages, ok, REGION,
};

/// The `eventSource` every record of this service must carry
/// (`service::management_events::EVENT_SOURCE`).
const EVENT_SOURCE: &str = "sqs.amazonaws.com";

/// The `eventType` CloudTrail stamps on the record of an API call.
const API_CALL_EVENT_TYPE: &str = "AwsApiCall";

/// How long a message is waited for when one is needed to drive the data-plane rows.
/// Generous: what is waited for is the message arriving, not a span of time.
const RECEIVE_DEADLINE: Duration = Duration::from_secs(30);

/// Whether a call is a management event of the account, and of which class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// Recorded, reporting state and changing none of it (`readOnly` true).
    Reports,
    /// Recorded, changing what the account holds (`readOnly` false).
    Mutates,
    /// Not a management event: recorded nowhere.
    Unrecorded,
}

/// Every record the account's event history holds, as the CloudTrail event document each
/// one is reported as.
fn recorded(engine: &engine::InMemoryEngine) -> Vec<Value> {
    let log = service_com_amazonaws_sqs_amazonsqs::service::management_events::management_event_log(
        engine,
    )
    .expect("the shared engine carries the account's management-event log");
    let held = log.lock();
    held.iter()
        .map(|event| event.to_cloudtrail_event_json())
        .collect()
}

/// Serve one call and assert what the account's event history then holds for it.
///
/// Answers the call's HTTP status and parsed body, so a row that needs a value the answer
/// carried (a receipt handle, a task handle) can go on using it. The status is NOT asserted
/// here: a refusal the operation reached is a served call and is recorded like any other, so
/// each caller asserts the status it expects itself.
async fn served(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    operation: &str,
    body: &Value,
    expected: Expected,
) -> (u16, Value) {
    let before = recorded(engine).len();
    let (status, answer) = call_in(router, REGION, operation, body).await;
    let after = recorded(engine);
    let added = &after[before..];
    if expected == Expected::Unrecorded {
        assert!(
            added.is_empty(),
            "case {case:?}: {operation} is no management event, so event history must hold \
             nothing for it, got {added:?}"
        );
        return (status, answer);
    }
    assert_eq!(
        added.len(),
        1,
        "case {case:?}: {operation} must be recorded exactly once, got {added:?}"
    );
    let record = &added[0];
    assert_eq!(
        record["eventName"],
        json!(operation),
        "case {case:?}: expected eventName {operation:?}, got {record}"
    );
    assert_eq!(
        record["eventSource"],
        json!(EVENT_SOURCE),
        "case {case:?}: expected eventSource {EVENT_SOURCE:?}, got {record}"
    );
    assert_eq!(
        record["awsRegion"],
        json!(REGION),
        "case {case:?}: expected awsRegion {REGION:?}, got {record}"
    );
    assert_eq!(
        record["recipientAccountId"],
        json!(account()),
        "case {case:?}: expected the serving account {:?}, got {record}",
        account()
    );
    assert_eq!(
        record["eventType"],
        json!(API_CALL_EVENT_TYPE),
        "case {case:?}: expected eventType {API_CALL_EVENT_TYPE:?}, got {record}"
    );
    assert_eq!(
        record["managementEvent"],
        json!(true),
        "case {case:?}: a control-plane call is a management event, got {record}"
    );
    assert_eq!(
        record["readOnly"],
        json!(expected == Expected::Reports),
        "case {case:?}: expected readOnly {}, got {record}",
        expected == Expected::Reports
    );
    assert!(
        !record["requestParameters"].is_null(),
        "case {case:?}: a dispatched call records the parameters it was dispatched with, \
         got {record}"
    );
    (status, answer)
}

/// The same, for a call that must also answer HTTP 200.
async fn served_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    operation: &str,
    body: &Value,
    expected: Expected,
) -> Value {
    let (status, answer) = served(router, engine, case, operation, body, expected).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {answer}"
    );
    answer
}

#[tokio::test]
async fn the_control_plane_is_recorded_and_the_message_plane_is_not() {
    let (router, engine) = emulator_over_engine();

    // The queue every row below acts on, and a second one whose redrive policy names it as
    // its dead-letter target — the only kind of source a message movement task takes.
    let dead_letter = "recorded-dead-letter";
    let dead_letter_url = create_ok(&router, dead_letter, &json!({})).await;
    let dead_letter_arn = expected_arn(dead_letter);
    let source_url = create_ok(
        &router,
        "recorded-source",
        &json!({
            "RedrivePolicy": json!({
                "deadLetterTargetArn": dead_letter_arn,
                "maxReceiveCount": "1",
            })
            .to_string(),
        }),
    )
    .await;

    // The creates: a call that mints a resource of the account.
    served_ok(
        &router,
        &engine,
        "a queue is created",
        "CreateQueue",
        &json!({ "QueueName": "recorded-subject" }),
        Expected::Mutates,
    )
    .await;
    let url = ok(
        &router,
        "GetQueueUrl",
        json!({ "QueueName": "recorded-subject" }),
    )
    .await["QueueUrl"]
        .as_str()
        .expect("the queue has a url")
        .to_string();

    // The readers: each reports state and changes none of it.
    served_ok(
        &router,
        &engine,
        "a queue url is resolved",
        "GetQueueUrl",
        &json!({ "QueueName": "recorded-subject" }),
        Expected::Reports,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a queue's attributes are read",
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["All"] }),
        Expected::Reports,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "the account's queues are listed",
        "ListQueues",
        &json!({}),
        Expected::Reports,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a queue's tags are listed",
        "ListQueueTags",
        &json!({ "QueueUrl": url }),
        Expected::Reports,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a dead-letter queue's sources are listed",
        "ListDeadLetterSourceQueues",
        &json!({ "QueueUrl": dead_letter_url }),
        Expected::Reports,
    )
    .await;

    // The configuration and tag writes.
    served_ok(
        &router,
        &engine,
        "a queue is configured",
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "DelaySeconds": "1" } }),
        Expected::Mutates,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a queue is tagged",
        "TagQueue",
        &json!({ "QueueUrl": url, "Tags": { "owner": "audit" } }),
        Expected::Mutates,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a queue's tag is removed",
        "UntagQueue",
        &json!({ "QueueUrl": url, "TagKeys": ["owner"] }),
        Expected::Mutates,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a principal is permitted",
        "AddPermission",
        &json!({
            "QueueUrl": url,
            "Label": "a-grant",
            "AWSAccountIds": [account()],
            "Actions": ["SendMessage"],
        }),
        Expected::Mutates,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a permission is revoked",
        "RemovePermission",
        &json!({ "QueueUrl": url, "Label": "a-grant" }),
        Expected::Mutates,
    )
    .await;

    // The message data plane: seven operations, none of them a management event.
    let sent = served_ok(
        &router,
        &engine,
        "a message is sent",
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": "one" }),
        Expected::Unrecorded,
    )
    .await;
    assert!(
        sent["MessageId"].is_string(),
        "setup: the send must mint a message id, got {sent}"
    );
    served_ok(
        &router,
        &engine,
        "a batch of messages is sent",
        "SendMessageBatch",
        &json!({
            "QueueUrl": url,
            "Entries": [{ "Id": "one", "MessageBody": "two" }],
        }),
        Expected::Unrecorded,
    )
    .await;

    // Wait for the sends to come over, so the rows that need a receipt handle have one.
    // The CONDITION is the message arriving; the deadline only bounds a wait that cannot be
    // satisfied.
    let deadline = Instant::now() + RECEIVE_DEADLINE;
    let mut handles: Vec<String> = Vec::new();
    while handles.len() < 2 && Instant::now() < deadline {
        let received = served_ok(
            &router,
            &engine,
            "messages are received",
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1 }),
            Expected::Unrecorded,
        )
        .await;
        for message in messages(&received) {
            handles.push(
                message["ReceiptHandle"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a received message carries a handle: {message}"))
                    .to_string(),
            );
        }
    }
    assert_eq!(
        handles.len(),
        2,
        "setup: both sent messages must come over, got {handles:?}"
    );

    served_ok(
        &router,
        &engine,
        "a message's visibility is changed",
        "ChangeMessageVisibility",
        &json!({
            "QueueUrl": url,
            "ReceiptHandle": handles[0],
            "VisibilityTimeout": 0,
        }),
        Expected::Unrecorded,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a batch of visibilities is changed",
        "ChangeMessageVisibilityBatch",
        &json!({
            "QueueUrl": url,
            "Entries": [{ "Id": "one", "ReceiptHandle": handles[1], "VisibilityTimeout": 0 }],
        }),
        Expected::Unrecorded,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a message is deleted",
        "DeleteMessage",
        &json!({ "QueueUrl": url, "ReceiptHandle": handles[0] }),
        Expected::Unrecorded,
    )
    .await;
    served_ok(
        &router,
        &engine,
        "a batch of messages is deleted",
        "DeleteMessageBatch",
        &json!({
            "QueueUrl": url,
            "Entries": [{ "Id": "one", "ReceiptHandle": handles[1] }],
        }),
        Expected::Unrecorded,
    )
    .await;

    // Purging a queue is a control-plane call on the queue, not a message operation.
    served_ok(
        &router,
        &engine,
        "a queue is purged",
        "PurgeQueue",
        &json!({ "QueueUrl": url }),
        Expected::Mutates,
    )
    .await;

    // The redrive tasks: started and listed on the dead-letter queue, cancelled by the
    // handle the start answered. A task the service has already finished refuses the
    // cancel, and that refusal is a served call too — so the row asserts the RECORD and
    // leaves the status to the operation.
    let started = served_ok(
        &router,
        &engine,
        "a message movement task is started",
        "StartMessageMoveTask",
        &json!({ "SourceArn": dead_letter_arn, "DestinationArn": expected_arn("recorded-source") }),
        Expected::Mutates,
    )
    .await;
    let task_handle = started["TaskHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("the start must answer a task handle, got {started}"))
        .to_string();
    served_ok(
        &router,
        &engine,
        "the movement tasks of a source are listed",
        "ListMessageMoveTasks",
        &json!({ "SourceArn": dead_letter_arn }),
        Expected::Reports,
    )
    .await;
    served(
        &router,
        &engine,
        "a message movement task is cancelled",
        "CancelMessageMoveTask",
        &json!({ "TaskHandle": task_handle }),
        Expected::Mutates,
    )
    .await;

    // And the delete, which removes the resource the rest of the rows acted on.
    served_ok(
        &router,
        &engine,
        "a queue is deleted",
        "DeleteQueue",
        &json!({ "QueueUrl": source_url }),
        Expected::Mutates,
    )
    .await;
}
