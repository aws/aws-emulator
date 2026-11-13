// SPDX-License-Identifier: CC0-1.0

//! What a pipe does with a batch it CANNOT get through its stages, end to end over the wire.
//!
//! Pipes, Kinesis, Amazon SQS, Lambda and (for the DynamoDB-stream rows) DynamoDB and
//! DynamoDB Streams are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! giving up on a batch is asserted where a client sees it: on the queue the pipe's
//! `DeadLetterConfig` names, and in the stream positions the pipe reads on from.
//!
//! The RULES asserted, each over a TABLE whose rows differ along what the rule generalises
//! over:
//!
//! 1. A STREAM-sourced pipe attempts a batch it cannot move `MaximumRetryAttempts` times
//!    beyond the first and then DISCARDS it — "discard records after the specified number of
//!    retries" — and the batch's record goes to the destination the same source parameters
//!    configure: one message pointing at the discarded batch (the pipe that gave up, why it
//!    gave up, and the shard and run of sequence numbers it covered) and carrying none of the
//!    records' own payloads. The rows differ along which stage fails and why, how many
//!    retries the pipe allows, and whether the failed stage's resource named an error of its
//!    own.
//! 2. The attempts a batch gets are the retries the pipe allows BEYOND THE FIRST: a pipe
//!    allowing none hands the batch to its enrichment once, one allowing two hands it over
//!    three times, and no attempt is made after the batch is discarded. The rows differ along
//!    the retries configured.
//! 3. A pipe that allows INFINITE retries — the member's documented default of -1, and a
//!    configuration that names no member at all — discards nothing: its destination stays
//!    empty and the batch stays on its source. The rows differ along how the infinite value
//!    is expressed.
//! 4. A discarded batch is not read again: the pipe's source moves past it, so the records
//!    written behind it are attempted in their turn and pointed at by their own dead letter.
//! 5. The pointer is reported under the member of the SOURCE KIND it was read from, so a
//!    DynamoDB-stream batch is never described as a Kinesis one.
//!
//! The member names are the model's own
//! (`smithy_query(query="shape", shape="PipeSourceKinesisStreamParameters,PipeSourceDynamoDBStreamParameters,DeadLetterConfig")`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{collect_at_least, collect_during, given_queue, rpc, LOOK_AGAIN};
use crate::pipe_lambda_target::given_function;
use crate::pipe_lifecycle::{call, describe, Reply, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies: no IAM
/// store is mounted, so no stage is refused on the strength of one.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-dead-letter";

/// How long a wait for work nothing asked for — the stream becoming readable, the pipe
/// settling, the attempts at a batch being spent, the dead letter landing — is given before
/// the row is a failure. Well above the poll interval, the state-settle span, the backoff
/// between attempts and one interpreted run of a handler, so a row fails only when the work
/// did not happen at all.
const BUDGET: Duration = Duration::from_secs(90);

/// How long a row watches a destination that must stay EMPTY. A pipe that was going to
/// discard its batch would have: it is many times the poll interval and the backoff between
/// the first attempts at one batch.
const QUIET: Duration = Duration::from_secs(5);

/// The `CurrentState` a pipe that is reading its source rests in.
const RUNNING: &str = "RUNNING";

/// What the record of a discarded batch reports it was discarded FOR.
const RETRIES_EXHAUSTED: &str = "RetryAttemptsExhausted";

/// The members a dead letter points at a discarded batch under, by the kind of stream the
/// batch was read from.
const KINESIS_BATCH_INFO: &str = "KinesisBatchInfo";
/// See [`KINESIS_BATCH_INFO`].
const DYNAMODB_BATCH_INFO: &str = "DDBStreamBatchInfo";

/// A handler that always raises, so every attempt at the batch it is handed fails and the
/// invocation reports an error of the function's own.
const ALWAYS_RAISES: &str = r#"
def lambda_handler(event, context):
    raise RuntimeError("this enrichment never succeeds")
"#;

/// The environment variable the counting handler below reads the queue it reports each
/// attempt on from. It is the handler's own: nothing in the emulator reads it.
const ATTEMPTS_QUEUE_VARIABLE: &str = "ATTEMPTS_QUEUE";

/// A handler that reports the attempt it is serving on a queue of its own and THEN raises,
/// so one message on that queue is one attempt the pipe made at the batch.
fn counting_handler() -> String {
    format!(
        r#"
import os

import boto3

ATTEMPTS_QUEUE = os.environ["{ATTEMPTS_QUEUE_VARIABLE}"]
CLIENT = boto3.client("sqs")


def lambda_handler(event, context):
    CLIENT.send_message(QueueUrl=ATTEMPTS_QUEUE, MessageBody="attempt")
    raise RuntimeError("this enrichment never succeeds")
"#
    )
}

/// Pipes and every service a stream-sourced pipe with a dead-letter destination reaches, on
/// one engine, as `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// One `awsJson` RPC call against Kinesis.
async fn kinesis(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "kinesis", "Kinesis_20131202", operation, body).await
}

/// One `awsJson` RPC call against DynamoDB.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "dynamodb", "DynamoDB_20120810", operation, body).await
}

/// A one-shard stream called `name`, waited out until it is ACTIVE; answers its ARN.
async fn given_stream(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = kinesis(
        router,
        "CreateStream",
        &json!({ "StreamName": name, "ShardCount": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateStream answered {status} {body}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, described) =
            kinesis(router, "DescribeStreamSummary", &json!({ "StreamName": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: DescribeStreamSummary answered {status} {described}"
        );
        let summary = &described["StreamDescriptionSummary"];
        let arn = summary["StreamARN"].as_str().unwrap_or_default().to_string();
        if summary["StreamStatus"].as_str() == Some("ACTIVE") && !arn.is_empty() {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: expected the stream to reach ACTIVE with an ARN, got {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A streamed table called `name`, waited out until it is ACTIVE; answers the ARN of its
/// stream, which is what a pipe names as its `Source`.
async fn given_streamed_table(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateTable answered {status} {body}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, described) =
            dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        assert_eq!(
            status, 200,
            "case {name}: DescribeTable answered {status} {described}"
        );
        let stream = described["Table"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") && !stream.is_empty() {
            return stream;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: expected the table to reach ACTIVE with a stream, got {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Put one record carrying `payload` on `stream`; answers the sequence number the stream
/// named it by, which is what a dead letter's pointer has to bound.
async fn put_record(
    router: &MultiServiceRouter,
    stream: &str,
    case: &str,
    payload: &str,
) -> String {
    let (status, body) = kinesis(
        router,
        "PutRecord",
        &json!({
            "StreamARN": stream,
            // A record's `Data` is bytes, which travel base64-encoded on the wire.
            "Data": aws_smithy_types::base64::encode(payload.as_bytes()),
            "PartitionKey": "orders",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRecord answered {status} {body}"
    );
    body["SequenceNumber"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no SequenceNumber in {body}"))
        .to_string()
}

/// Write one item to `table`, so its stream carries one change.
async fn put_item(router: &MultiServiceRouter, table: &str, case: &str, payload: &str) {
    let (status, body) = dynamodb(
        router,
        "PutItem",
        &json!({
            "TableName": table,
            "Item": {"pk": {"S": "order-1"}, "marker": {"S": payload}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutItem answered {status} {body}"
    );
}

/// `SourceParameters` for a Kinesis-stream source read from its oldest record one record at
/// a time, with the dead-letter destination and retry budget the row configures.
fn kinesis_source_parameters(dead_letter: Option<&str>, retries: Option<i64>) -> Value {
    let mut stream = json!({ "StartingPosition": "TRIM_HORIZON", "BatchSize": 1 });
    if let Some(arn) = dead_letter {
        stream["DeadLetterConfig"] = json!({ "Arn": arn });
    }
    if let Some(retries) = retries {
        stream["MaximumRetryAttempts"] = json!(retries);
    }
    json!({ "KinesisStreamParameters": stream })
}

/// The same for a DynamoDB-stream source.
fn stream_source_parameters(dead_letter: &str, retries: i64) -> Value {
    json!({
        "DynamoDBStreamParameters": {
            "StartingPosition": "TRIM_HORIZON",
            "BatchSize": 1,
            "MaximumRetryAttempts": retries,
            "DeadLetterConfig": { "Arn": dead_letter },
        }
    })
}

/// Create the pipe `name` under `body` and wait until it reports it RUNS; answers its ARN.
async fn running_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> String {
    let created: Reply = call(
        router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreatePipe answered {} {:?}",
        created.status, created.body
    );
    let arn = created.string("Arn");
    assert!(
        !arn.is_empty(),
        "case {name:?}: CreatePipe must answer the pipe's ARN, got {:?}",
        created.body
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = describe(router, name).await;
        if described.string("CurrentState") == RUNNING {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {name:?}: expected the pipe to reach {RUNNING}, got {described:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The dead letters waiting on the queue at `url`, as the documents their bodies spell.
async fn dead_letters(router: &MultiServiceRouter, url: &str, case: &str, want: usize) -> Vec<Value> {
    collect_at_least(router, url, want, BUDGET)
        .await
        .iter()
        .map(|body| {
            serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: a dead letter is a JSON document ({error}): {body:?}")
            })
        })
        .collect()
}

/// What makes a row's batch fail, which decides which STAGE of the pipe gives up on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// An enrichment that is there and always raises: every attempt reaches it and it
    /// reports an error of its own.
    EnrichmentRaises,
    /// An enrichment ARN naming a function the account does not hold, so the enrichment
    /// stage's own call cannot be made.
    EnrichmentAbsent,
    /// A target naming a function the account does not hold: the batch gets through the
    /// enrichment (there is none) and the delivery is what fails.
    TargetAbsent,
}

/// One row of the dead-letter table.
struct Row {
    case: &'static str,
    failure: Failure,
    /// `MaximumRetryAttempts`: the attempts the pipe allows BEYOND the first.
    retries: i64,
    /// The stage the record of the discarded batch must name.
    stage: &'static str,
    /// Whether the failed stage's own resource named an error of its own.
    named_an_error: bool,
}

/// RULE 1: a stream-sourced pipe attempts a batch it cannot move `MaximumRetryAttempts`
/// times beyond the first and then discards it, sending the record of it to the destination
/// its `DeadLetterConfig` names — a pointer at the discarded batch, never the records.
#[tokio::test]
async fn a_batch_that_spends_its_retries_is_pointed_at_on_the_dead_letter_queue() {
    let rows: [Row; 3] = [
        Row {
            case: "an enrichment that always raises, discarded on its first failure",
            failure: Failure::EnrichmentRaises,
            retries: 0,
            stage: "ENRICHMENT",
            named_an_error: true,
        },
        Row {
            case: "an enrichment that cannot be invoked, two retries",
            failure: Failure::EnrichmentAbsent,
            retries: 2,
            stage: "ENRICHMENT",
            named_an_error: false,
        },
        Row {
            case: "a target that cannot be invoked, one retry",
            failure: Failure::TargetAbsent,
            retries: 1,
            stage: "TARGET",
            named_an_error: false,
        },
    ];
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let stream = given_stream(&router, &format!("dlq-source-{index}")).await;
        let (dlq_url, dlq_arn) = given_queue(&router, &format!("dlq-{index}")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("dlq-target-{index}")).await;
        let (enrichment, target) = match row.failure {
            Failure::EnrichmentRaises => (
                Some(
                    given_function(
                        &router,
                        case,
                        &format!("dlq-enrichment-{index}"),
                        ALWAYS_RAISES,
                        json!({}),
                    )
                    .await,
                ),
                target_arn.clone(),
            ),
            Failure::EnrichmentAbsent => (
                Some(format!(
                    "arn:aws:lambda:{REGION}:000000000000:function:never-created-{index}"
                )),
                target_arn.clone(),
            ),
            Failure::TargetAbsent => (
                None,
                format!("arn:aws:lambda:{REGION}:000000000000:function:absent-target-{index}"),
            ),
        };
        let mut body = json!({
            "Source": stream,
            "Target": target,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": kinesis_source_parameters(Some(&dlq_arn), Some(row.retries)),
        });
        if let Some(enrichment) = &enrichment {
            body["Enrichment"] = json!(enrichment);
        }
        let pipe = format!("dlq-pipe-{index}");
        let pipe_arn = running_pipe(&router, &pipe, body).await;

        // The configuration the pipe gives up under is the one it reports holding.
        let described = describe(&router, &pipe).await;
        let reported = &described.member("SourceParameters")["KinesisStreamParameters"];
        assert_eq!(
            reported["DeadLetterConfig"]["Arn"],
            json!(dlq_arn),
            "case {case:?}: the pipe must report the destination it was given, got {reported}"
        );
        assert_eq!(
            reported["MaximumRetryAttempts"],
            json!(row.retries),
            "case {case:?}: the pipe must report the retries it was given, got {reported}"
        );

        let payload = format!("payload-of-{index}");
        let sequence_number = put_record(&router, &stream, case, &payload).await;
        let letters = dead_letters(&router, &dlq_url, case, 1).await;
        let letter = letters.first().unwrap_or_else(|| {
            panic!("case {case:?}: expected a dead letter for the discarded batch, got none")
        });
        let pointer = &letter[KINESIS_BATCH_INFO];
        assert_eq!(
            pointer["streamArn"],
            json!(stream),
            "case {case:?}: expected the stream the batch was read off, got {letter}"
        );
        assert_eq!(
            pointer["startSequenceNumber"],
            json!(sequence_number),
            "case {case:?}: expected the discarded record's own sequence number, got {letter}"
        );
        assert_eq!(
            pointer["endSequenceNumber"],
            json!(sequence_number),
            "case {case:?}: expected the batch's last sequence number, got {letter}"
        );
        assert_eq!(
            pointer["batchSize"],
            json!(1),
            "case {case:?}: expected the discarded batch's size, got {letter}"
        );
        assert!(
            pointer["shardId"]
                .as_str()
                .is_some_and(|shard| shard.starts_with("shardId-")),
            "case {case:?}: expected the shard the batch was read from, got {letter}"
        );
        assert_eq!(
            letter["context"]["pipeArn"],
            json!(pipe_arn),
            "case {case:?}: expected the pipe that gave up, got {letter}"
        );
        assert_eq!(
            letter["context"]["condition"],
            json!(RETRIES_EXHAUSTED),
            "case {case:?}: expected the spent retries as the condition, got {letter}"
        );
        assert_eq!(
            letter["context"]["stage"],
            json!(row.stage),
            "case {case:?}: expected the stage that gave up, got {letter}"
        );
        assert_eq!(
            letter["context"]["approximateInvokeCount"],
            json!(row.retries + 1),
            "case {case:?}: expected {} attempt(s) at the batch, got {letter}",
            row.retries + 1
        );
        assert_eq!(
            letter["version"],
            json!("1.0"),
            "case {case:?}: expected the message envelope's version, got {letter}"
        );
        assert!(
            letter["timestamp"]
                .as_str()
                .is_some_and(|at| at.contains('T')),
            "case {case:?}: expected the instant the batch was discarded, got {letter}"
        );
        let named = letter["responseContext"]["functionError"]
            .as_str()
            .is_some_and(|error| !error.is_empty());
        assert_eq!(
            named, row.named_an_error,
            "case {case:?}: the {} stage's resource named an error={}, got {letter}",
            row.stage, row.named_an_error
        );
        assert!(
            !letter.to_string().contains(&payload),
            "case {case:?}: a dead letter points at the batch and carries no record payload, \
             got {letter}"
        );
        // The batch never reached the target queue: the pipe gave up on it.
        if row.failure != Failure::TargetAbsent {
            let delivered = collect_during(&router, &target_url, QUIET).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a discarded batch is delivered nowhere, got {delivered:?}"
            );
        }
    }
}

/// RULE 2: the attempts a batch gets are the retries the pipe's source parameters allow
/// BEYOND THE FIRST, and no attempt is made at it once it has been discarded.
///
/// One message on the attempts queue is one run of the enrichment handler, which the handler
/// itself reports before raising — so the count is the pipe's own behaviour and not this
/// suite's reading of it.
#[tokio::test]
async fn a_batch_is_attempted_once_for_the_first_try_and_once_per_retry() {
    // (case, the `MaximumRetryAttempts` the row configures)
    let rows: [(&str, i64); 3] = [
        ("no retry at all", 0),
        ("one retry", 1),
        ("two retries", 2),
    ];
    for (index, (case, retries)) in rows.iter().enumerate() {
        let router = emulator();
        let stream = given_stream(&router, &format!("attempts-source-{index}")).await;
        let (dlq_url, dlq_arn) = given_queue(&router, &format!("attempts-dlq-{index}")).await;
        let (attempts_url, _) = given_queue(&router, &format!("attempts-count-{index}")).await;
        let (_, target_arn) = given_queue(&router, &format!("attempts-target-{index}")).await;
        let enrichment = given_function(
            &router,
            case,
            &format!("attempts-enrichment-{index}"),
            &counting_handler(),
            json!({ ATTEMPTS_QUEUE_VARIABLE: attempts_url }),
        )
        .await;
        let body = json!({
            "Source": stream,
            "Target": target_arn,
            "Enrichment": enrichment,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": kinesis_source_parameters(Some(&dlq_arn), Some(*retries)),
        });
        running_pipe(&router, &format!("attempts-pipe-{index}"), body).await;
        put_record(&router, &stream, case, "payload-to-attempt").await;

        // The dead letter is written once the attempts are spent, so by the time it is here
        // every attempt the pipe was going to make has been made.
        let letters = dead_letters(&router, &dlq_url, case, 1).await;
        assert_eq!(
            letters.len(),
            1,
            "case {case:?}: expected one dead letter for the discarded batch, got {letters:?}"
        );
        let wanted = usize::try_from(retries + 1).expect("the row's attempt count is positive");
        let made = collect_at_least(&router, &attempts_url, wanted, QUIET).await;
        assert_eq!(
            made.len(),
            wanted,
            "case {case:?}: expected {wanted} attempt(s) at the batch, got {made:?}"
        );
        let after = collect_during(&router, &attempts_url, QUIET).await;
        assert!(
            after.is_empty(),
            "case {case:?}: a discarded batch is attempted no further, got {after:?}"
        );
    }
}

/// RULE 3: a pipe that allows infinite retries discards nothing — its destination stays
/// empty and the batch stays on its source for the next attempt.
#[tokio::test]
async fn a_pipe_that_allows_infinite_retries_discards_nothing() {
    // (case, the `MaximumRetryAttempts` the row configures)
    let rows: [(&str, Option<i64>); 2] = [
        ("no maximum-retry-attempts member at all", None),
        // "The default value is -1, which sets the maximum number of retries to infinite."
        ("the documented infinite value", Some(-1)),
    ];
    for (index, (case, retries)) in rows.iter().enumerate() {
        let router = emulator();
        let stream = given_stream(&router, &format!("forever-source-{index}")).await;
        let (dlq_url, dlq_arn) = given_queue(&router, &format!("forever-dlq-{index}")).await;
        let (_, target_arn) = given_queue(&router, &format!("forever-target-{index}")).await;
        let body = json!({
            "Source": stream,
            "Target": target_arn,
            "Enrichment": format!(
                "arn:aws:lambda:{REGION}:000000000000:function:never-created-forever-{index}"
            ),
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": kinesis_source_parameters(Some(&dlq_arn), *retries),
        });
        running_pipe(&router, &format!("forever-pipe-{index}"), body).await;
        put_record(&router, &stream, case, "payload-that-stays").await;
        let letters = collect_during(&router, &dlq_url, QUIET).await;
        assert!(
            letters.is_empty(),
            "case {case:?}: a pipe retrying for ever discards nothing, got {letters:?}"
        );
    }
}

/// RULE 4: a discarded batch is not read again — the source moves past it, so the record
/// written behind it is attempted in its turn and pointed at by its own dead letter.
#[tokio::test]
async fn the_source_moves_past_a_discarded_batch() {
    let case = "two records discarded one after the other";
    let router = emulator();
    let stream = given_stream(&router, "moves-on-source").await;
    let (dlq_url, dlq_arn) = given_queue(&router, "moves-on-dlq").await;
    let (_, target_arn) = given_queue(&router, "moves-on-target").await;
    let body = json!({
        "Source": stream,
        "Target": target_arn,
        "Enrichment": format!(
            "arn:aws:lambda:{REGION}:000000000000:function:never-created-moves-on"
        ),
        "RoleArn": ROLE,
        "DesiredState": "RUNNING",
        "SourceParameters": kinesis_source_parameters(Some(&dlq_arn), Some(0)),
    });
    running_pipe(&router, "moves-on-pipe", body).await;

    let first = put_record(&router, &stream, case, "first-payload").await;
    let letters = dead_letters(&router, &dlq_url, case, 1).await;
    let pointed_at = letters[0][KINESIS_BATCH_INFO]["startSequenceNumber"].clone();
    assert_eq!(
        pointed_at,
        json!(first),
        "case {case:?}: expected the first record to be pointed at, got {:?}",
        letters[0]
    );

    let second = put_record(&router, &stream, case, "second-payload").await;
    assert_ne!(
        second, first,
        "case {case:?}: the stream must name the two records differently"
    );
    // The second dead letter is the second batch: the pipe read on from past the first one
    // rather than attempting it again for ever.
    let deadline = Instant::now() + BUDGET;
    loop {
        let more = collect_at_least(&router, &dlq_url, 1, BUDGET).await;
        let pointers: Vec<Value> = more
            .iter()
            .map(|body| {
                serde_json::from_str::<Value>(body)
                    .unwrap_or_else(|error| panic!("case {case:?}: {error}: {body:?}"))
            })
            .collect();
        if pointers
            .iter()
            .any(|letter| letter[KINESIS_BATCH_INFO]["startSequenceNumber"] == json!(second))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a dead letter for the record behind the discarded one, \
             got {pointers:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE 5: the pointer is reported under the member of the source kind the batch was read
/// from, so a DynamoDB-stream batch is never described as a Kinesis one.
#[tokio::test]
async fn a_dynamodb_stream_batch_is_pointed_at_under_its_own_member() {
    let case = "a dynamodb-stream batch the enrichment could not be invoked for";
    let router = emulator();
    let stream = given_streamed_table(&router, "dlq-table").await;
    let (dlq_url, dlq_arn) = given_queue(&router, "dlq-table-letters").await;
    let (_, target_arn) = given_queue(&router, "dlq-table-target").await;
    let body = json!({
        "Source": stream,
        "Target": target_arn,
        "Enrichment": format!(
            "arn:aws:lambda:{REGION}:000000000000:function:never-created-for-the-table"
        ),
        "RoleArn": ROLE,
        "DesiredState": "RUNNING",
        "SourceParameters": stream_source_parameters(&dlq_arn, 0),
    });
    let pipe_arn = running_pipe(&router, "dlq-table-pipe", body).await;

    let payload = "item-payload";
    put_item(&router, "dlq-table", case, payload).await;
    let letters = dead_letters(&router, &dlq_url, case, 1).await;
    let letter = letters.first().unwrap_or_else(|| {
        panic!("case {case:?}: expected a dead letter for the discarded change, got none")
    });
    assert!(
        letter.get(KINESIS_BATCH_INFO).is_none(),
        "case {case:?}: a stream batch is not described as a Kinesis one, got {letter}"
    );
    let pointer = &letter[DYNAMODB_BATCH_INFO];
    assert_eq!(
        pointer["streamArn"],
        json!(stream),
        "case {case:?}: expected the table's stream, got {letter}"
    );
    assert_eq!(
        pointer["batchSize"],
        json!(1),
        "case {case:?}: expected the discarded batch's size, got {letter}"
    );
    assert!(
        pointer["shardId"]
            .as_str()
            .is_some_and(|shard| !shard.is_empty()),
        "case {case:?}: expected the shard the change was read from, got {letter}"
    );
    for member in ["startSequenceNumber", "endSequenceNumber"] {
        assert!(
            pointer[member].as_str().is_some_and(|at| !at.is_empty()),
            "case {case:?}: expected {member} to bound the discarded batch, got {letter}"
        );
    }
    assert_eq!(
        letter["context"]["pipeArn"],
        json!(pipe_arn),
        "case {case:?}: expected the pipe that gave up, got {letter}"
    );
    assert!(
        !letter.to_string().contains(payload),
        "case {case:?}: a dead letter carries no item image, got {letter}"
    );
}
