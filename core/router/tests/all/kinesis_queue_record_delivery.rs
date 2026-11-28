// SPDX-License-Identifier: CC0-1.0

//! A Kinesis QUEUE on a data stream, end to end over the wire: the control plane that creates,
//! reads, lists, updates and deletes one, and the data plane that delivers the stream's records
//! through it, acknowledges them, redelivers what is not acknowledged, and moves what is never
//! acknowledged onto its dead-letter path.
//!
//! RULE: a queue is a stored resource of one account and Region, created on ONE data stream in
//! the on-demand capacity mode, and every value a queue operation reports is computed from that
//! stored record and the clock — the status its create, its update and its delete owe it, the
//! attributes with every documented default resolved (a 30-second visibility timeout, built-in
//! dead-letter handling with a `MaxReceiveCount` of 6), and the effective position in the
//! stream delivery begins at. Its data plane delivers the records the STREAM holds past the
//! queue's own cursor: a received record is in flight under a fresh receipt handle for its
//! visibility timeout, an acknowledged record is never delivered again, a record whose
//! visibility lapses is delivered again with a higher receive count, and a record received
//! `MaxReceiveCount` times without being acknowledged is no longer returned by
//! `ReceiveQueueRecords` and is served, acknowledged, made visible or restored through the
//! dead-letter operations that take the same queue ARN.
//!
//! These rules span two resources of one service — the stream holds the records, the queue holds
//! the delivery state — and this suite drives both over the real `awsJson1_1` wire (`POST /`
//! with `X-Amz-Target: Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so
//! every status, body and modelled error asserted here is the one a client sees.
//!
//! The rows vary what each rule generalises over: the attributes a create names, the payload and
//! partition key a producer writes, how many records are outstanding and what `Limit` asks for,
//! the `MaxReceiveCount` a queue runs with, the starting position delivery begins at, and the
//! class of request a queue operation cannot serve.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The service shape an `awsJson1_1` `X-Amz-Target` names an operation of this api under
/// (`smithy_query(query="service_info")` on its model).
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The capacity mode queues are supported on: "Queues are supported only on data streams in
/// on-demand capacity mode" (`CreateQueue`).
const ON_DEMAND: &str = "ON_DEMAND";

/// The other capacity mode, which a queue cannot be created on.
const PROVISIONED: &str = "PROVISIONED";

/// The status a freshly created queue reports (`QueueStatus`).
const CREATING: &str = "CREATING";

/// The status a created queue reaches, and the one it delivers records in.
const ACTIVE: &str = "ACTIVE";

/// The status a queue an accepted update armed reports (`UpdateQueue`).
const UPDATING: &str = "UPDATING";

/// The visibility timeout a queue created without one carries: "If you omit this attribute when
/// you create a queue, the queue uses 30 seconds"
/// (`QueueAttributes$VisibilityTimeoutSeconds`).
const DEFAULT_VISIBILITY_TIMEOUT: i64 = 30;

/// The receive count a queue created without a `DeadLetterConfiguration` carries: "A queue that
/// you create without DeadLetterConfiguration gets a MaxReceiveCount of 6"
/// (`DeadLetterConfiguration$MaxReceiveCount`).
const DEFAULT_MAX_RECEIVE_COUNT: i64 = 6;

/// `TargetType` `BUILT_IN`, which "enables" dead-letter handling
/// (`DeadLetterConfiguration$TargetType`).
const BUILT_IN: &str = "BUILT_IN";

/// `TargetType` `NONE`, which "disables it, and an unacknowledged record continues to be
/// redelivered until it is acknowledged or its retention period ends" (same member).
const NONE_TARGET: &str = "NONE";

/// How long a case keeps LOOKING for a state the service owes it before giving up. Generous on
/// purpose: what a case waits for is a CONDITION the emulator reaches on its own clock, never a
/// duration the case measures, so a slow or loaded host only makes it look again.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a case waits between two looks while polling for such a condition.
const BETWEEN_LOOKS: Duration = Duration::from_millis(25);

/// How long a receive that expects records waits for one: generous, because the call returns as
/// soon as a record is visible ("If a record becomes visible sooner, the call returns sooner",
/// `ReceiveQueueRecords`), so this bounds nothing a passing case waits out.
const WAIT_FOR_RECORDS: i64 = 10;

/// How long a receive that expects NO record waits: the shortest wait the model admits
/// (`ReceiveQueueRecordsWaitTimeSeconds` is `range 1..20`), since such a call necessarily waits
/// its whole wait out.
const WAIT_FOR_NOTHING: i64 = 1;

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, signed for [`REGION`] so the service resolves the request's
    /// Region the way it does in production (the SigV4 credential scope).
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Call `operation`, asserting a 200 and answering the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body {response}"
        );
        response
    }
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Look again until `holds` answers true, and fail once [`POLL_DEADLINE`] has passed.
///
/// What a case waits for is a CONDITION the emulator reaches on its own clock — a queue that
/// reports `ACTIVE`, a record that becomes visible again — so a case never measures a duration
/// and never sleeps for one.
async fn poll_until<F, Fut>(case: &str, what: &str, holds: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    loop {
        if holds().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case}: gave up waiting for {what}"
        );
        tokio::time::sleep(BETWEEN_LOOKS).await;
    }
}

/// One data stream in `mode`, created over the wire and waited for until the service reports it
/// `ACTIVE`; answers its `StreamARN`.
async fn stream(emulator: &Emulator, case: &str, name: &str, mode: &str) -> String {
    let mut request = json!({
        "StreamName": name,
        "StreamModeDetails": { "StreamMode": mode },
    });
    if mode == PROVISIONED {
        request["ShardCount"] = json!(1);
    }
    emulator.ok(case, "CreateStream", &request).await;
    let described = poll_until_stream_active(emulator, case, name).await;
    described["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a stream summary carries its ARN, got {described}"))
        .to_string()
}

/// Look at the stream `name` until it reports `ACTIVE`, and answer the summary that said so.
async fn poll_until_stream_active(emulator: &Emulator, case: &str, name: &str) -> Value {
    poll_until(case, &format!("the stream {name} reports {ACTIVE}"), || async {
        let (status, response) = emulator
            .call("DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        status == 200
            && response["StreamDescriptionSummary"]["StreamStatus"].as_str() == Some(ACTIVE)
    })
    .await;
    emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await
}

/// Write one record to `stream_arn` under `partition_key`, carrying `payload`.
async fn put(emulator: &Emulator, case: &str, stream_arn: &str, partition_key: &str, payload: &[u8]) {
    emulator
        .ok(
            case,
            "PutRecord",
            &json!({
                "StreamARN": stream_arn,
                "PartitionKey": partition_key,
                "Data": aws_smithy_types::base64::encode(payload),
            }),
        )
        .await;
}

/// One queue on `stream_arn`, created over the wire with `attributes` and `starting_position`,
/// and waited for until it reports `ACTIVE`; answers its `QueueARN`.
async fn queue(
    emulator: &Emulator,
    case: &str,
    name: &str,
    stream_arn: &str,
    attributes: Option<Value>,
    starting_position: Option<Value>,
) -> String {
    let mut configuration = json!({ "StreamARN": stream_arn });
    if let Some(position) = starting_position {
        configuration["StartingPosition"] = position;
    }
    let mut request = json!({
        "QueueName": name,
        "StreamConfigurationList": [configuration],
    });
    if let Some(attributes) = attributes {
        request["QueueAttributes"] = attributes;
    }
    let created = emulator.ok(case, "CreateQueue", &request).await;
    let arn = created["QueueDescription"]["QueueARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a queue description carries its ARN, got {created}"))
        .to_string();
    poll_until_queue_active(emulator, case, &arn).await;
    arn
}

/// Look at the queue `arn` until it reports `ACTIVE` — the state its create or its last accepted
/// change owes it.
async fn poll_until_queue_active(emulator: &Emulator, case: &str, arn: &str) {
    poll_until(case, &format!("the queue {arn} reports {ACTIVE}"), || async {
        let (status, response) = emulator
            .call("DescribeQueue", &json!({ "QueueARN": arn }))
            .await;
        status == 200 && response["QueueDescription"]["QueueStatus"].as_str() == Some(ACTIVE)
    })
    .await;
}

/// One `ReceiveQueueRecords` call, answering the records it served.
async fn receive(
    emulator: &Emulator,
    case: &str,
    queue_arn: &str,
    limit: Option<i64>,
    wait: i64,
) -> Vec<Value> {
    let mut request = json!({ "QueueARN": queue_arn, "WaitTimeSeconds": wait });
    if let Some(limit) = limit {
        request["Limit"] = json!(limit);
    }
    let received = emulator.ok(case, "ReceiveQueueRecords", &request).await;
    records_of(case, &received)
}

/// One `ReceiveDeadLetterQueueRecords` call, answering the records it served.
async fn receive_dead_letter(
    emulator: &Emulator,
    case: &str,
    queue_arn: &str,
    wait: i64,
) -> Vec<Value> {
    let received = emulator
        .ok(
            case,
            "ReceiveDeadLetterQueueRecords",
            &json!({ "QueueARN": queue_arn, "WaitTimeSeconds": wait }),
        )
        .await;
    records_of(case, &received)
}

/// The `QueueRecords` list a receive answered.
fn records_of(case: &str, response: &Value) -> Vec<Value> {
    response["QueueRecords"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a receive answers a QueueRecords list, got {response}"))
        .clone()
}

/// The receipt handle one served record carries.
fn handle_of(case: &str, record: &Value) -> String {
    record["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a served record carries a receipt handle, got {record}"))
        .to_string()
}

/// The payload one served record carries, decoded from the base64 the wire carries a blob in.
fn payload_of(case: &str, record: &Value) -> Vec<u8> {
    let encoded = record["Data"].as_str().unwrap_or_default();
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case}: a record's Data is base64, got {error}"))
}

/// Acknowledge one record by its receipt handle, answering the whole response so a case can read
/// both reported lists.
async fn acknowledge(emulator: &Emulator, case: &str, queue_arn: &str, handle: &str) -> Value {
    emulator
        .ok(
            case,
            "AcknowledgeQueueRecords",
            &json!({
                "QueueARN": queue_arn,
                "Entries": [{ "ReceiptHandle": handle }],
            }),
        )
        .await
}

/// Make one record visible again at once: "Specify 0 to make a record visible in the queue
/// immediately" (`UpdateQueueRecordsVisibility`).
async fn make_visible(emulator: &Emulator, case: &str, queue_arn: &str, handle: &str) -> Value {
    emulator
        .ok(
            case,
            "UpdateQueueRecordsVisibility",
            &json!({
                "QueueARN": queue_arn,
                "Entries": [{ "ReceiptHandle": handle, "VisibilityTimeoutSeconds": 0 }],
            }),
        )
        .await
}

/// The one receipt handle a reported `Successful` list carries, asserting nothing failed.
fn only_successful(case: &str, response: &Value) -> String {
    let successful = response["Successful"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a batch answers a Successful list, got {response}"));
    let failed = response["Failed"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a batch answers a Failed list, got {response}"));
    assert!(
        failed.is_empty(),
        "case {case}: expected every entry to succeed, got failed {failed:?}"
    );
    assert_eq!(
        successful.len(),
        1,
        "case {case}: expected one successful entry, got {successful:?}"
    );
    handle_of(case, &successful[0])
}

/// RULE: a create stores the queue under the name the request carried, scoped to the Region it
/// was signed for, mints the ARN it is addressed by (an identifier the service generates, not the
/// name), resolves every documented attribute default once, and reports `CREATING` until the
/// transition it armed falls due; a read, a listing and an update all report that same stored
/// record, and a delete takes it to `DELETING` and then out of the account.
///
/// The rows differ along the attributes the create names: none at all, a visibility timeout
/// alone, dead-letter handling turned off, and dead-letter handling with a count of its own.
#[tokio::test]
async fn a_queue_is_created_read_back_listed_updated_and_deleted() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-lifecycle", ON_DEMAND).await;

    // (case, the attributes the create names, the visibility timeout it must report, the
    // MaxReceiveCount it must report — `None` for dead-letter handling turned off).
    let rows: [(&str, Option<Value>, i64, Option<i64>); 4] = [
        ("no attributes", None, DEFAULT_VISIBILITY_TIMEOUT, Some(DEFAULT_MAX_RECEIVE_COUNT)),
        (
            "a visibility timeout alone",
            Some(json!({ "VisibilityTimeoutSeconds": 120 })),
            120,
            Some(DEFAULT_MAX_RECEIVE_COUNT),
        ),
        (
            "dead-letter handling off",
            Some(json!({ "DeadLetterConfiguration": { "TargetType": NONE_TARGET } })),
            DEFAULT_VISIBILITY_TIMEOUT,
            None,
        ),
        (
            "a dead-letter count of its own",
            Some(json!({
                "VisibilityTimeoutSeconds": 0,
                "DeadLetterConfiguration": { "TargetType": BUILT_IN, "MaxReceiveCount": 3 },
            })),
            0,
            Some(3),
        ),
    ];

    for (index, (case, attributes, visibility, max_receive_count)) in rows.into_iter().enumerate() {
        let name = format!("lifecycle-{index}");
        let created = emulator
            .ok(
                case,
                "CreateQueue",
                &json!({
                    "QueueName": &name,
                    "StreamConfigurationList": [{ "StreamARN": &stream_arn }],
                    "QueueAttributes": attributes.clone().unwrap_or(json!(null)),
                }),
            )
            .await;
        let description = &created["QueueDescription"];
        let arn = description["QueueARN"].as_str().unwrap_or_default().to_string();

        // "Upon receiving a CreateQueue request, Amazon Kinesis Data Streams immediately returns
        // a queue description with a QueueStatus of CREATING" (`CreateQueue`).
        assert_eq!(
            description["QueueStatus"].as_str(),
            Some(CREATING),
            "case {case}: expected a created queue to report {CREATING}, got {description}"
        );
        assert_eq!(
            description["QueueName"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the queue named {name}, got {description}"
        );
        // "The resource identifier in the ARN is generated when the queue is created and is not
        // derived from QueueName" (`QueueDescription$QueueARN`).
        let prefix = format!(
            "arn:aws:kinesis:{REGION}:{}:queue/",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert!(
            arn.starts_with(&prefix),
            "case {case}: expected a QueueARN under {prefix}, got {arn}"
        );
        let resource = arn.rsplit('/').next().unwrap_or_default();
        assert!(
            !resource.is_empty() && resource != name,
            "case {case}: expected the ARN's resource identifier to be generated rather than \
             {name}, got {arn}"
        );
        assert_eq!(
            description["StreamConfigurationList"][0]["StreamARN"].as_str(),
            Some(stream_arn.as_str()),
            "case {case}: expected the queue to report the stream it delivers from, got \
             {description}"
        );
        assert_eq!(
            description["QueueAttributes"]["VisibilityTimeoutSeconds"].as_i64(),
            Some(visibility),
            "case {case}: expected a visibility timeout of {visibility}, got {description}"
        );
        assert_eq!(
            description["QueueAttributes"]["DeadLetterConfiguration"]["MaxReceiveCount"].as_i64(),
            max_receive_count,
            "case {case}: expected a MaxReceiveCount of {max_receive_count:?}, got {description}"
        );
        assert_eq!(
            description["QueueAttributes"]["DeadLetterConfiguration"]["TargetType"].as_str(),
            Some(if max_receive_count.is_some() { BUILT_IN } else { NONE_TARGET }),
            "case {case}: expected the TargetType the configuration resolves to, got \
             {description}"
        );

        // The read reports the SAME stored record, once the transition the create armed has
        // fallen due.
        poll_until_queue_active(&emulator, case, &arn).await;
        let described = emulator
            .ok(case, "DescribeQueue", &json!({ "QueueARN": &arn }))
            .await;
        let read = &described["QueueDescription"];
        for member in [
            "QueueName",
            "QueueARN",
            "QueueCreationTimestamp",
            "QueueAttributes",
        ] {
            assert_eq!(
                read[member], description[member],
                "case {case}: expected {member} to read back as the create reported it, got \
                 {read} after {description}"
            );
        }
        assert_eq!(
            read["StreamConfigurationList"][0]["StreamARN"],
            description["StreamConfigurationList"][0]["StreamARN"],
            "case {case}: expected the stream it delivers from to read back unchanged, got \
             {read} after {description}"
        );
        // "While the queue is CREATING... for TRIM_HORIZON and LATEST, this member is absent
        // because the effective position is not yet determined. Once the queue is ACTIVE, the
        // response always includes this member" (`QueueStartingPosition$Timestamp`) — the one
        // member of a description that is NOT the same in the two states, because the effective
        // position is only determined once the queue is active.
        assert!(
            description["StreamConfigurationList"][0]["StartingPosition"]["Timestamp"]
                .is_null(),
            "case {case}: expected a CREATING queue with no configured instant to report none, \
             got {description}"
        );
        assert!(
            read["StreamConfigurationList"][0]["StartingPosition"]["Timestamp"]
                .as_f64()
                .is_some(),
            "case {case}: expected an ACTIVE queue to report its effective starting instant, \
             got {read}"
        );

        // An update writes the attributes it names and leaves the rest as they were.
        let updated = emulator
            .ok(
                case,
                "UpdateQueue",
                &json!({
                    "QueueARN": &arn,
                    "QueueAttributes": { "VisibilityTimeoutSeconds": 45 },
                }),
            )
            .await;
        let changed = &updated["QueueDescription"];
        assert_eq!(
            changed["QueueStatus"].as_str(),
            Some(UPDATING),
            "case {case}: expected an updated queue to report {UPDATING}, got {changed}"
        );
        assert_eq!(
            changed["QueueAttributes"]["VisibilityTimeoutSeconds"].as_i64(),
            Some(45),
            "case {case}: expected the visibility timeout the update named, got {changed}"
        );
        assert_eq!(
            changed["QueueAttributes"]["DeadLetterConfiguration"]["MaxReceiveCount"].as_i64(),
            max_receive_count,
            "case {case}: expected the dead-letter configuration to survive an update that \
             names none, got {changed}"
        );
        poll_until_queue_active(&emulator, case, &arn).await;

        // The delete arms the removal: `DELETING` first, gone from then on.
        emulator
            .ok(case, "DeleteQueue", &json!({ "QueueARN": &arn }))
            .await;
        let deleting = emulator
            .ok(case, "DescribeQueue", &json!({ "QueueARN": &arn }))
            .await;
        assert_eq!(
            deleting["QueueDescription"]["QueueStatus"].as_str(),
            Some("DELETING"),
            "case {case}: expected a deleted queue to report DELETING, got {deleting}"
        );
        poll_until(case, &format!("the queue {arn} to be gone"), || async {
            let (status, _) = emulator
                .call("DescribeQueue", &json!({ "QueueARN": &arn }))
                .await;
            status == 400
        })
        .await;
        let (status, gone) = emulator
            .call("DescribeQueue", &json!({ "QueueARN": &arn }))
            .await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected a read of a removed queue to be refused, got {status} with \
             {gone}"
        );
        assert_eq!(
            error_message(&gone),
            format!(
                "Queue {resource} under account {} not found.",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case}: expected the not-found sentence to name the queue and the account"
        );
    }
}

/// RULE: a listing reports a summary of each queue of the account and Region, ordered by name,
/// pages with the opaque cursor it hands back, and reports only the queues on the streams a
/// `StreamFilter` names; and a stream's `QueueCount` is how many queues this account holds on it.
#[tokio::test]
async fn a_listing_reports_each_queue_of_the_region_and_pages_and_filters_it() {
    let emulator = Emulator::new();
    let first_stream = stream(&emulator, "setup", "queue-listing-one", ON_DEMAND).await;
    let second_stream = stream(&emulator, "setup", "queue-listing-two", ON_DEMAND).await;

    // Three queues, created out of name order so the listing's order is its own rather than the
    // order of the creates: two on one stream, one on another.
    let names = ["listing-c", "listing-a", "listing-b"];
    let mut arns: Vec<(String, String)> = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        let on = if index == 2 { &second_stream } else { &first_stream };
        let arn = queue(&emulator, name, name, on, None, None).await;
        arns.push((name.to_string(), arn));
    }

    let listed = emulator.ok("every queue", "ListQueues", &json!({})).await;
    let summaries = listed["QueueSummaries"].as_array().cloned().unwrap_or_default();
    let reported: Vec<&str> = summaries
        .iter()
        .map(|summary| summary["QueueName"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        reported,
        vec!["listing-a", "listing-b", "listing-c"],
        "case \"every queue\": expected the queues ordered by name, got {listed}"
    );
    for summary in &summaries {
        assert_eq!(
            summary["QueueStatus"].as_str(),
            Some(ACTIVE),
            "case \"every queue\": expected each summary to report the stored status, got \
             {summary}"
        );
        assert!(
            summary["Streams"][0]["StreamARN"].as_str().is_some()
                && summary["Streams"][0]["StreamCreationTimestamp"].as_f64().is_some(),
            "case \"every queue\": expected each summary to identify its stream, got {summary}"
        );
    }
    assert!(
        listed["NextToken"].as_str().is_none(),
        "case \"every queue\": expected no cursor when every queue fits in the page, got {listed}"
    );

    // One queue per page: the cursor resumes the listing at the queue the page did not return.
    let first_page = emulator
        .ok("one per page", "ListQueues", &json!({ "MaxResults": 1 }))
        .await;
    let cursor = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"one per page\": expected a cursor, got {first_page}"))
        .to_string();
    assert_eq!(
        first_page["QueueSummaries"][0]["QueueName"].as_str(),
        Some("listing-a"),
        "case \"one per page\": expected the first page to hold the first queue by name, got \
         {first_page}"
    );
    let second_page = emulator
        .ok(
            "one per page",
            "ListQueues",
            &json!({ "MaxResults": 1, "NextToken": cursor }),
        )
        .await;
    assert_eq!(
        second_page["QueueSummaries"][0]["QueueName"].as_str(),
        Some("listing-b"),
        "case \"one per page\": expected the cursor to resume at the next queue, got \
         {second_page}"
    );

    // A cursor this service never issued names no page of this listing.
    let (status, refused) = emulator
        .call("ListQueues", &json!({ "NextToken": "not-a-cursor" }))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case \"a cursor the service never issued\": expected it refused, got {status} with \
         {refused}"
    );

    // The filter reports only the queues on the stream it names, whether or not it pins that
    // stream's creation instant.
    let stream_created_at = emulator
        .ok(
            "the filter",
            "DescribeStreamSummary",
            &json!({ "StreamARN": &second_stream }),
        )
        .await["StreamDescriptionSummary"]["StreamCreationTimestamp"]
        .as_f64()
        .expect("a stream summary carries its creation instant");
    for (case, filter) in [
        ("by stream", json!([{ "StreamARN": &second_stream }])),
        (
            "by stream and instant",
            json!([{ "StreamARN": &second_stream, "StreamCreationTimestamp": stream_created_at }]),
        ),
    ] {
        let filtered = emulator
            .ok(case, "ListQueues", &json!({ "StreamFilter": filter }))
            .await;
        let summaries = filtered["QueueSummaries"].as_array().cloned().unwrap_or_default();
        let names: Vec<&str> = summaries
            .iter()
            .map(|summary| summary["QueueName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec!["listing-b"],
            "case {case}: expected only the queue on that stream, got {filtered}"
        );
    }
    // A filter pinning ANOTHER instant of that stream names no queue: an ARN a delete released
    // and a later create took again is another stream.
    let other_instant = stream_created_at - 60.0;
    let none = emulator
        .ok(
            "by stream and another instant",
            "ListQueues",
            &json!({
                "StreamFilter": [{
                    "StreamARN": &second_stream,
                    "StreamCreationTimestamp": other_instant,
                }],
            }),
        )
        .await;
    assert_eq!(
        none["QueueSummaries"].as_array().map(Vec::len),
        Some(0),
        "case \"by stream and another instant\": expected no queue, got {none}"
    );

    // "The number of queues created on the data stream"
    // (`StreamDescriptionSummary$QueueCount`).
    for (case, stream_arn, expected) in [
        ("two queues", &first_stream, 2),
        ("one queue", &second_stream, 1),
    ] {
        let summary = emulator
            .ok(
                case,
                "DescribeStreamSummary",
                &json!({ "StreamARN": stream_arn }),
            )
            .await;
        assert_eq!(
            summary["StreamDescriptionSummary"]["QueueCount"].as_i64(),
            Some(expected),
            "case {case}: expected the stream to report {expected} queues, got {summary}"
        );
    }
}

/// RULE: a queue delivers the records its stream holds past its own cursor — payload, partition
/// key and arrival instant as the producer wrote them — each under a fresh receipt handle and at
/// most `Limit` at a time; an acknowledged record is never delivered again, and acknowledging a
/// record leaves the stream's own copy of it readable.
///
/// The rows differ along the payload a producer wrote (plain text, bytes outside ASCII, an empty
/// blob, a larger payload) and the partition key it wrote under.
#[tokio::test]
async fn a_queue_delivers_its_streams_records_and_an_acknowledged_record_is_not_delivered_again() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-delivery", ON_DEMAND).await;
    let queue_arn = queue(
        &emulator,
        "setup",
        "delivery",
        &stream_arn,
        // TRIM_HORIZON so the queue delivers what the stream already holds, which is what lets
        // one queue serve every row below.
        None,
        Some(json!({ "Type": "TRIM_HORIZON" })),
    )
    .await;

    // (case, partition key, payload).
    let rows: [(&str, &str, Vec<u8>); 4] = [
        ("plain text", "orders", b"first-payload".to_vec()),
        ("bytes outside ASCII", "orders", "payload-\u{00e9}\u{4e2d}".as_bytes().to_vec()),
        ("an empty blob", "empty", Vec::new()),
        ("a larger payload", "bulk", vec![b'x'; 4096]),
    ];
    for (case, partition_key, payload) in &rows {
        put(&emulator, case, &stream_arn, partition_key, payload).await;
    }

    // `Limit` bounds one receive: "The queue never returns more records than this value"
    // (`ReceiveQueueRecordsInput$Limit`).
    let one = receive(&emulator, "a limit of one", &queue_arn, Some(1), WAIT_FOR_RECORDS).await;
    assert_eq!(
        one.len(),
        1,
        "case \"a limit of one\": expected one record, got {one:?}"
    );
    // That record is in flight now, so the rest of the rows are served by the receives below.
    let mut served: Vec<Value> = one;
    while served.len() < rows.len() {
        let more = receive(&emulator, "the rest", &queue_arn, None, WAIT_FOR_RECORDS).await;
        assert!(
            !more.is_empty(),
            "case \"the rest\": expected the queue to serve the records still outstanding, got \
             {served:?} so far"
        );
        served.extend(more);
    }

    let mut handles: Vec<String> = Vec::new();
    for (case, partition_key, payload) in &rows {
        let record = served
            .iter()
            .find(|record| payload_of(case, record) == *payload)
            .unwrap_or_else(|| {
                panic!("case {case}: expected the queue to deliver the record, got {served:?}")
            });
        assert_eq!(
            record["PartitionKey"].as_str(),
            Some(*partition_key),
            "case {case}: expected the partition key the producer wrote, got {record}"
        );
        assert_eq!(
            record["ReceiveCount"].as_i64(),
            Some(1),
            "case {case}: expected a first delivery to report a receive count of 1, got {record}"
        );
        let record_id = record["RecordId"].as_str().unwrap_or_default();
        assert!(
            !record_id.is_empty()
                && record_id
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
            "case {case}: expected a RecordId inside its modelled pattern, got {record}"
        );
        assert!(
            record["ApproximateArrivalTimestamp"].as_f64().is_some()
                && record["FirstReceiveTimestamp"].as_f64().is_some(),
            "case {case}: expected both reported instants, got {record}"
        );
        let handle = handle_of(case, record);
        assert!(
            !handles.contains(&handle),
            "case {case}: expected a receipt handle of its own for each delivery, got {handle} \
             twice"
        );
        handles.push(handle);
    }

    // Acknowledging each record stops its delivery for good.
    for (index, (case, _, _)) in rows.iter().enumerate() {
        let acknowledged = acknowledge(&emulator, case, &queue_arn, &handles[index]).await;
        assert_eq!(
            only_successful(case, &acknowledged),
            handles[index],
            "case {case}: expected the acknowledgement to report the handle it was given"
        );
    }
    let after = receive(
        &emulator,
        "every record acknowledged",
        &queue_arn,
        None,
        WAIT_FOR_NOTHING,
    )
    .await;
    assert!(
        after.is_empty(),
        "case \"every record acknowledged\": expected no record to be delivered again, got \
         {after:?}"
    );

    // "Acknowledging a record does not affect the record in the data stream"
    // (`AcknowledgeQueueRecords`): the stream still serves every payload to its own readers,
    // across every shard its on-demand capacity mode laid down.
    let shards = emulator
        .ok(
            "the stream's own copy",
            "ListShards",
            &json!({ "StreamARN": &stream_arn }),
        )
        .await;
    let shard_ids: Vec<String> = shards["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|shard| shard["ShardId"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut on_the_stream = 0usize;
    for shard_id in &shard_ids {
        let iterator = emulator
            .ok(
                "the stream's own copy",
                "GetShardIterator",
                &json!({
                    "StreamARN": &stream_arn,
                    "ShardId": shard_id,
                    "ShardIteratorType": "TRIM_HORIZON",
                }),
            )
            .await["ShardIterator"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let page = emulator
            .ok(
                "the stream's own copy",
                "GetRecords",
                &json!({ "ShardIterator": iterator }),
            )
            .await;
        on_the_stream += page["Records"].as_array().map(Vec::len).unwrap_or_default();
    }
    assert_eq!(
        on_the_stream,
        rows.len(),
        "case \"the stream's own copy\": expected every record to stay readable on the stream \
         after being acknowledged, got {on_the_stream} across {shard_ids:?}"
    );

    // An entry naming a handle the queue did not issue is reported in `Failed`, not refused.
    let failed = acknowledge(
        &emulator,
        "a handle the queue never issued",
        &queue_arn,
        "not-a-handle",
    )
    .await;
    assert_eq!(
        failed["Successful"].as_array().map(Vec::len),
        Some(0),
        "case \"a handle the queue never issued\": expected nothing acknowledged, got {failed}"
    );
    assert_eq!(
        failed["Failed"][0]["ErrorCode"].as_str(),
        Some("ResourceNotFoundException"),
        "case \"a handle the queue never issued\": expected the entry reported under the \
         not-found code, got {failed}"
    );
    assert_eq!(
        failed["Failed"][0]["ReceiptHandle"].as_str(),
        Some("not-a-handle"),
        "case \"a handle the queue never issued\": expected the entry to name the handle it \
         carried, got {failed}"
    );
}

/// RULE: a record that is not acknowledged before its visibility timeout elapses becomes visible
/// in the queue again and is delivered with a higher receive count; once it has been received
/// `MaxReceiveCount` times without being acknowledged it is a dead-letter record, which
/// `ReceiveQueueRecords` no longer returns and the dead-letter operations serve, restore and
/// discard.
///
/// The rows differ along the `MaxReceiveCount` the queue runs with, which is how many deliveries
/// a record gets before it becomes a dead-letter record.
#[tokio::test]
async fn an_unacknowledged_record_is_redelivered_and_then_becomes_a_dead_letter_record() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-dead-letter", ON_DEMAND).await;

    for max_receive_count in [1_i64, 3] {
        let case = &format!("a MaxReceiveCount of {max_receive_count}");
        let name = format!("dead-letter-{max_receive_count}");
        let queue_arn = queue(
            &emulator,
            case,
            &name,
            &stream_arn,
            Some(json!({
                "DeadLetterConfiguration": {
                    "TargetType": BUILT_IN,
                    "MaxReceiveCount": max_receive_count,
                },
            })),
            None,
        )
        .await;
        put(&emulator, case, &stream_arn, "redelivery", b"unacknowledged").await;

        // Each delivery raises the receive count, and the record is made visible again at once
        // rather than waited out — the visibility timeout is the queue's to decide, and this is
        // the operation a consumer uses to end one early.
        for delivery in 1..=max_receive_count {
            let served = receive(&emulator, case, &queue_arn, None, WAIT_FOR_RECORDS).await;
            assert_eq!(
                served.len(),
                1,
                "case {case}: expected delivery {delivery} to serve the record, got {served:?}"
            );
            assert_eq!(
                served[0]["ReceiveCount"].as_i64(),
                Some(delivery),
                "case {case}: expected delivery {delivery} to report that receive count, got \
                 {:?}",
                served[0]
            );
            let handle = handle_of(case, &served[0]);
            let changed = make_visible(&emulator, case, &queue_arn, &handle).await;
            assert_eq!(
                only_successful(case, &changed),
                handle,
                "case {case}: expected the visibility change to report the handle it was given"
            );
        }

        // Past the ceiling the record is a dead-letter record: the receive path no longer
        // returns it.
        let past = receive(&emulator, case, &queue_arn, None, WAIT_FOR_NOTHING).await;
        assert!(
            past.is_empty(),
            "case {case}: expected the receive path to stop returning the record, got {past:?}"
        );

        // The dead-letter path serves it, carrying the same data and partition key.
        let dead = receive_dead_letter(&emulator, case, &queue_arn, WAIT_FOR_RECORDS).await;
        assert_eq!(
            dead.len(),
            1,
            "case {case}: expected the dead-letter path to serve the record, got {dead:?}"
        );
        assert_eq!(
            payload_of(case, &dead[0]),
            b"unacknowledged".to_vec(),
            "case {case}: expected the dead-letter record to carry the data it was written \
             with, got {:?}",
            dead[0]
        );
        assert_eq!(
            dead[0]["PartitionKey"].as_str(),
            Some("redelivery"),
            "case {case}: expected the dead-letter record to carry its partition key, got {:?}",
            dead[0]
        );
        let dead_handle = handle_of(case, &dead[0]);

        // A restore clears the dead-letter state and resets the receive count, so the receive
        // path returns it again.
        let restored = emulator
            .ok(
                case,
                "RestoreDeadLetterQueueRecords",
                &json!({
                    "QueueARN": &queue_arn,
                    "Entries": [{ "ReceiptHandle": &dead_handle }],
                }),
            )
            .await;
        assert_eq!(
            only_successful(case, &restored),
            dead_handle,
            "case {case}: expected the restore to report the handle it was given"
        );
        let again = receive(&emulator, case, &queue_arn, None, WAIT_FOR_RECORDS).await;
        assert_eq!(
            again.len(),
            1,
            "case {case}: expected a restored record to be delivered again, got {again:?}"
        );
        assert_eq!(
            again[0]["ReceiveCount"].as_i64(),
            Some(1),
            "case {case}: expected a restored record's receive count to start again, got {:?}",
            again[0]
        );

        // Take it back to the dead-letter path and discard it there: the dead-letter path then
        // serves nothing either.
        let mut handle = handle_of(case, &again[0]);
        for _ in 1..=max_receive_count {
            make_visible(&emulator, case, &queue_arn, &handle).await;
            let served = receive(&emulator, case, &queue_arn, None, WAIT_FOR_NOTHING).await;
            if served.is_empty() {
                break;
            }
            handle = handle_of(case, &served[0]);
        }
        make_visible(&emulator, case, &queue_arn, &handle).await;
        let dead_again = receive_dead_letter(&emulator, case, &queue_arn, WAIT_FOR_RECORDS).await;
        assert_eq!(
            dead_again.len(),
            1,
            "case {case}: expected the record on the dead-letter path again, got {dead_again:?}"
        );
        let discarded = emulator
            .ok(
                case,
                "AcknowledgeDeadLetterQueueRecords",
                &json!({
                    "QueueARN": &queue_arn,
                    "Entries": [{ "ReceiptHandle": handle_of(case, &dead_again[0]) }],
                }),
            )
            .await;
        assert!(
            discarded["Failed"].as_array().map(Vec::is_empty).unwrap_or(false),
            "case {case}: expected the dead-letter acknowledgement to succeed, got {discarded}"
        );
        let nothing = receive_dead_letter(&emulator, case, &queue_arn, WAIT_FOR_NOTHING).await;
        assert!(
            nothing.is_empty(),
            "case {case}: expected an acknowledged dead-letter record to be gone, got \
             {nothing:?}"
        );
    }
}

/// RULE: the starting position a create configures decides which of the stream's records the
/// queue delivers — records that arrive after the queue is created for `LATEST`, everything the
/// stream still retains for `TRIM_HORIZON`, and records at or after the instant for
/// `AT_TIMESTAMP` — and once the queue is `ACTIVE` a read reports the EFFECTIVE instant that
/// position resolves to.
#[tokio::test]
async fn the_starting_position_decides_which_records_a_queue_delivers() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-positions", ON_DEMAND).await;

    // An instant every record of this stream arrives after, and one every record arrives before:
    // the two ends `AT_TIMESTAMP` is read at, in the epoch seconds the wire carries a timestamp
    // as.
    let before_everything = 1.0_f64;
    let after_everything = 4_000_000_000.0_f64;

    // (case, the starting position, whether the record written BEFORE the create is delivered,
    // whether the record written AFTER it is).
    let rows: [(&str, Option<Value>, bool, bool); 5] = [
        ("no position at all", None, false, true),
        ("LATEST", Some(json!({ "Type": "LATEST" })), false, true),
        ("TRIM_HORIZON", Some(json!({ "Type": "TRIM_HORIZON" })), true, true),
        (
            "AT_TIMESTAMP before every record",
            Some(json!({ "Type": "AT_TIMESTAMP", "Timestamp": before_everything })),
            true,
            true,
        ),
        (
            "AT_TIMESTAMP after every record",
            Some(json!({ "Type": "AT_TIMESTAMP", "Timestamp": after_everything })),
            false,
            false,
        ),
    ];

    for (index, (case, position, serves_earlier, serves_later)) in rows.into_iter().enumerate() {
        let earlier = format!("earlier-{index}");
        let later = format!("later-{index}");
        put(&emulator, case, &stream_arn, "positions", earlier.as_bytes()).await;
        let name = format!("position-{index}");
        let queue_arn = queue(&emulator, case, &name, &stream_arn, None, position.clone()).await;
        put(&emulator, case, &stream_arn, "positions", later.as_bytes()).await;

        // Everything this queue delivers, drained until it serves nothing more.
        let mut delivered: Vec<String> = Vec::new();
        loop {
            let served = receive(&emulator, case, &queue_arn, None, WAIT_FOR_NOTHING).await;
            if served.is_empty() {
                break;
            }
            for record in &served {
                delivered.push(String::from_utf8_lossy(&payload_of(case, record)).to_string());
                let handle = handle_of(case, record);
                acknowledge(&emulator, case, &queue_arn, &handle).await;
            }
        }
        assert_eq!(
            delivered.contains(&earlier),
            serves_earlier,
            "case {case}: expected the record written before the create to be delivered = \
             {serves_earlier}, got {delivered:?}"
        );
        assert_eq!(
            delivered.contains(&later),
            serves_later,
            "case {case}: expected the record written after the create to be delivered = \
             {serves_later}, got {delivered:?}"
        );

        // "Once the queue is ACTIVE, the response always includes this member"
        // (`QueueStartingPosition$Timestamp`), and for `AT_TIMESTAMP` it is "the timestamp you
        // provided or the earliest time the stream can be read from, whichever is later".
        let described = emulator
            .ok(case, "DescribeQueue", &json!({ "QueueARN": &queue_arn }))
            .await;
        let reported = &described["QueueDescription"]["StreamConfigurationList"][0]
            ["StartingPosition"];
        let expected_type = position
            .as_ref()
            .and_then(|position| position["Type"].as_str())
            .unwrap_or("LATEST")
            .to_string();
        assert_eq!(
            reported["Type"].as_str(),
            Some(expected_type.as_str()),
            "case {case}: expected the position type the create configured, got {described}"
        );
        let effective = reported["Timestamp"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected an ACTIVE queue to report its effective instant, got {described}")
        });
        if case == "AT_TIMESTAMP after every record" {
            assert_eq!(
                effective, after_everything,
                "case {case}: expected the instant the create named, got {described}"
            );
        } else {
            assert!(
                effective > before_everything,
                "case {case}: expected the effective instant to be at or after the earliest \
                 time the stream can be read from, got {described}"
            );
        }
    }
}

/// RULE: a queue request the service cannot serve is refused under the class its model declares
/// for that fault, and nothing is stored: the request's own faults first (a list that does not
/// name exactly one stream, a starting position whose members do not go together, built-in
/// dead-letter handling with no receive count), then the name and the ARNs it names (a name the
/// Region already holds, a stream or a queue this account does not hold), then what the stored
/// stream says (a capacity mode queues are not supported on).
#[tokio::test]
async fn a_queue_request_the_service_cannot_serve_is_refused_under_its_modelled_class() {
    let emulator = Emulator::new();
    let on_demand = stream(&emulator, "setup", "queue-refusals", ON_DEMAND).await;
    let provisioned = stream(&emulator, "setup", "queue-refusals-provisioned", PROVISIONED).await;
    let held = queue(&emulator, "setup", "already-held", &on_demand, None, None).await;
    let absent_stream = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/queue-refusals-absent",
        engine::EMULATOR_ACCOUNT_ID
    );
    let absent_queue = format!(
        "arn:aws:kinesis:{REGION}:{}:queue/0123456789abcdef0123456789abcdef",
        engine::EMULATOR_ACCOUNT_ID
    );

    // (case, operation, request, the modelled error class it earns).
    let rows: [(&str, &str, Value, &str); 10] = [
        (
            "two stream configurations",
            "CreateQueue",
            json!({
                "QueueName": "two-streams",
                "StreamConfigurationList": [
                    { "StreamARN": &on_demand },
                    { "StreamARN": &on_demand },
                ],
            }),
            "ValidationException",
        ),
        (
            "AT_TIMESTAMP with no instant",
            "CreateQueue",
            json!({
                "QueueName": "no-instant",
                "StreamConfigurationList": [{
                    "StreamARN": &on_demand,
                    "StartingPosition": { "Type": "AT_TIMESTAMP" },
                }],
            }),
            "ValidationException",
        ),
        (
            "LATEST with an instant",
            "CreateQueue",
            json!({
                "QueueName": "instant-not-taken",
                "StreamConfigurationList": [{
                    "StreamARN": &on_demand,
                    "StartingPosition": { "Type": "LATEST", "Timestamp": 1.0 },
                }],
            }),
            "ValidationException",
        ),
        (
            // An instant so far from the epoch that no millisecond count names it: it names a
            // position in no stream, and is refused rather than read as another instant.
            "AT_TIMESTAMP with an instant no epoch millisecond names",
            "CreateQueue",
            json!({
                "QueueName": "instant-past-the-epoch-range",
                "StreamConfigurationList": [{
                    "StreamARN": &on_demand,
                    "StartingPosition": {
                        "Type": "AT_TIMESTAMP",
                        "Timestamp": 9_000_000_000_000_000_000_i64,
                    },
                }],
            }),
            "ValidationException",
        ),
        (
            "BUILT_IN with no receive count",
            "CreateQueue",
            json!({
                "QueueName": "no-receive-count",
                "StreamConfigurationList": [{ "StreamARN": &on_demand }],
                "QueueAttributes": { "DeadLetterConfiguration": { "TargetType": BUILT_IN } },
            }),
            "ValidationException",
        ),
        (
            "a provisioned stream",
            "CreateQueue",
            json!({
                "QueueName": "provisioned",
                "StreamConfigurationList": [{ "StreamARN": &provisioned }],
            }),
            "ValidationException",
        ),
        (
            "a stream this account does not hold",
            "CreateQueue",
            json!({
                "QueueName": "absent-stream",
                "StreamConfigurationList": [{ "StreamARN": &absent_stream }],
            }),
            "ResourceNotFoundException",
        ),
        (
            "a name the Region already holds",
            "CreateQueue",
            json!({
                "QueueName": "already-held",
                "StreamConfigurationList": [{ "StreamARN": &on_demand }],
            }),
            "ResourceInUseException",
        ),
        (
            "a read of a queue this account does not hold",
            "DescribeQueue",
            json!({ "QueueARN": &absent_queue }),
            "ResourceNotFoundException",
        ),
        (
            "a receive from a queue this account does not hold",
            "ReceiveQueueRecords",
            json!({ "QueueARN": &absent_queue, "WaitTimeSeconds": WAIT_FOR_NOTHING }),
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, refused) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected),
            "case {case}: expected {operation} refused with {expected}, got {status} with \
             {refused}"
        );
        assert!(
            !error_message(&refused).is_empty(),
            "case {case}: expected the refusal to carry a message, got {refused}"
        );
    }

    // An update of a queue that is not ready for one is the ACTIVE-state refusal, and the stored
    // attributes are left exactly as they were.
    let creating = emulator
        .ok(
            "an update of a queue still being created",
            "CreateQueue",
            &json!({
                "QueueName": "still-creating",
                "StreamConfigurationList": [{ "StreamARN": &on_demand }],
            }),
        )
        .await;
    let creating_arn = creating["QueueDescription"]["QueueARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, refused) = emulator
        .call(
            "UpdateQueue",
            &json!({
                "QueueARN": &creating_arn,
                "QueueAttributes": { "VisibilityTimeoutSeconds": 10 },
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ResourceInUseException"),
        "case \"an update of a queue still being created\": expected the ACTIVE-state refusal, \
         got {status} with {refused}"
    );
    poll_until_queue_active(&emulator, "an update of a queue still being created", &creating_arn)
        .await;
    let described = emulator
        .ok(
            "an update of a queue still being created",
            "DescribeQueue",
            &json!({ "QueueARN": &creating_arn }),
        )
        .await;
    assert_eq!(
        described["QueueDescription"]["QueueAttributes"]["VisibilityTimeoutSeconds"].as_i64(),
        Some(DEFAULT_VISIBILITY_TIMEOUT),
        "case \"an update of a queue still being created\": expected the refused update to \
         change nothing, got {described}"
    );

    // The queue the setup created is untouched by every refusal above.
    let still_there = emulator
        .ok("the stored queue", "DescribeQueue", &json!({ "QueueARN": &held }))
        .await;
    assert_eq!(
        still_there["QueueDescription"]["QueueStatus"].as_str(),
        Some(ACTIVE),
        "case \"the stored queue\": expected the refusals to leave it exactly as it was, got \
         {still_there}"
    );
}

/// RULE: a queue on a data stream is a dependency of that stream — the stream cannot be deleted
/// and cannot be switched to the provisioned capacity mode while it carries one, and both calls
/// are served once the queues are gone.
#[tokio::test]
async fn a_streams_queues_stand_in_the_way_of_deleting_it_or_provisioning_it() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-dependency", ON_DEMAND).await;
    let queue_arn = queue(&emulator, "setup", "dependency", &stream_arn, None, None).await;

    for (case, operation, request) in [
        (
            "a delete of the stream",
            "DeleteStream",
            json!({ "StreamARN": &stream_arn }),
        ),
        (
            "a switch to the provisioned mode",
            "UpdateStreamMode",
            json!({
                "StreamARN": &stream_arn,
                "StreamModeDetails": { "StreamMode": PROVISIONED },
            }),
        ),
    ] {
        let (status, refused) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceInUseException"),
            "case {case}: expected the queue to stand in the way, got {status} with {refused}"
        );
        assert!(
            error_message(&refused).contains("queues"),
            "case {case}: expected the refusal to name the queues standing in the way, got \
             {refused}"
        );
    }

    // Once the queue is gone, the same calls are served.
    emulator
        .ok("the queue deleted", "DeleteQueue", &json!({ "QueueARN": &queue_arn }))
        .await;
    poll_until("the queue deleted", "the queue to be gone", || async {
        let (status, _) = emulator
            .call("DescribeQueue", &json!({ "QueueARN": &queue_arn }))
            .await;
        status == 400
    })
    .await;
    emulator
        .ok(
            "the queue deleted",
            "UpdateStreamMode",
            &json!({
                "StreamARN": &stream_arn,
                "StreamModeDetails": { "StreamMode": PROVISIONED },
            }),
        )
        .await;
    poll_until_stream_active(&emulator, "the queue deleted", "queue-dependency").await;
    emulator
        .ok(
            "the queue deleted",
            "DeleteStream",
            &json!({ "StreamARN": &stream_arn }),
        )
        .await;
}

/// RULE: a queue carries ONE stored tag set — the tags its create assigned, which the
/// resource-tagging operations then add to, change and remove through the queue's ARN, exactly as
/// they do for every other kind of Kinesis resource.
#[tokio::test]
async fn a_queues_tags_are_the_set_the_resource_tagging_operations_reach() {
    let emulator = Emulator::new();
    let stream_arn = stream(&emulator, "setup", "queue-tags", ON_DEMAND).await;
    let created = emulator
        .ok(
            "a create that assigns tags",
            "CreateQueue",
            &json!({
                "QueueName": "tagged",
                "StreamConfigurationList": [{ "StreamARN": &stream_arn }],
                "Tags": { "Owner": "payments", "Environment": "test" },
            }),
        )
        .await;
    let queue_arn = created["QueueDescription"]["QueueARN"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    poll_until_queue_active(&emulator, "a create that assigns tags", &queue_arn).await;

    // (case, the tags the queue must carry) after each step below.
    let reported = |case: &str, response: &Value| -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = response["Tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a Tags list, got {response}"))
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    };

    let listed = emulator
        .ok(
            "a create that assigns tags",
            "ListTagsForResource",
            &json!({ "ResourceARN": &queue_arn }),
        )
        .await;
    assert_eq!(
        reported("a create that assigns tags", &listed),
        vec![
            ("Environment".to_string(), "test".to_string()),
            ("Owner".to_string(), "payments".to_string()),
        ],
        "case \"a create that assigns tags\": expected the tags the create carried, got {listed}"
    );

    emulator
        .ok(
            "an assignment through the queue's ARN",
            "TagResource",
            &json!({
                "ResourceARN": &queue_arn,
                "Tags": { "Owner": "billing", "Team": "stream" },
            }),
        )
        .await;
    let after_assignment = emulator
        .ok(
            "an assignment through the queue's ARN",
            "ListTagsForResource",
            &json!({ "ResourceARN": &queue_arn }),
        )
        .await;
    assert_eq!(
        reported("an assignment through the queue's ARN", &after_assignment),
        vec![
            ("Environment".to_string(), "test".to_string()),
            ("Owner".to_string(), "billing".to_string()),
            ("Team".to_string(), "stream".to_string()),
        ],
        "case \"an assignment through the queue's ARN\": expected the assignment to overwrite \
         one key and leave the rest, got {after_assignment}"
    );

    emulator
        .ok(
            "an unassignment through the queue's ARN",
            "UntagResource",
            &json!({ "ResourceARN": &queue_arn, "TagKeys": ["Owner", "absent"] }),
        )
        .await;
    let after_removal = emulator
        .ok(
            "an unassignment through the queue's ARN",
            "ListTagsForResource",
            &json!({ "ResourceARN": &queue_arn }),
        )
        .await;
    assert_eq!(
        reported("an unassignment through the queue's ARN", &after_removal),
        vec![
            ("Environment".to_string(), "test".to_string()),
            ("Team".to_string(), "stream".to_string()),
        ],
        "case \"an unassignment through the queue's ARN\": expected the named key removed and a \
         key the queue does not carry ignored, got {after_removal}"
    );
}
