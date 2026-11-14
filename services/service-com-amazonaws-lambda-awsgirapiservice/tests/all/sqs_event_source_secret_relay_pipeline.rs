// SPDX-License-Identifier: CC0-1.0

//! The queue -> function -> secret -> queue chain, driven by nothing but the event source
//! mapping: no client ever calls `Invoke` here.
//!
//! The RULE asserted over the table below, in one sentence: an `Enabled` event source mapping
//! on an Amazon SQS queue reads that queue and invokes its function with an `aws:sqs` envelope
//! carrying ONE record per message it took — at most `BatchSize` of them, each naming the
//! source queue and reporting the message as the queue holds it — and deletes exactly the
//! messages the invocation processed, so each message is handed to the handler once, the AWS
//! calls that handler makes take effect in the services that own them, and the source queue
//! drains. ("Creates a mapping between an event source and an Lambda Event Function. Lambda
//! reads items from the event source and invokes the function", `CreateEventSourceMapping`;
//! "BatchSize — the maximum number of records in each batch that Lambda pulls from your stream
//! or queue and sends to your function".)
//!
//! What makes the chain observable is work only a REAL run can do: the handler reads a secret
//! out of Secrets Manager with the SDK its managed runtime supplies and relays what it read,
//! beside the record it was handed, to a second queue. So everything this suite reads back was
//! written by an invocation the mapping's own poller started, out of the envelope that run was
//! given — a mapping that reports healthy and delivers nothing leaves the sink queue empty and
//! the source queue full, and every row fails.
//!
//! The rows differ along what the rule generalises over: how many messages are waiting, the
//! `BatchSize` they are read in (a ceiling above what is waiting, one below it, and one
//! message per batch), the form each record names the secret by (its name, its complete ARN
//! and its partial ARN — the forms `GetSecretValueRequest$SecretId` accepts) and the form the
//! bodies take (plain text, a JSON document and text that is not ASCII).
//!
//! The Lambda, Amazon SQS and mapping helpers are `event_source_mappings.rs`'s, the role
//! fixture is `event_source_mapping_role_access.rs`'s and the Secrets Manager wire is
//! `handler_secret_relayed_to_queue.rs`'s, so this suite drives the same wires as every other
//! suite over these surfaces.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    HOST_WAIT_CEILING, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_OK;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{allow, given_role, logs_only, QUEUE_READS};
use crate::event_source_mappings::{
    await_function_active, await_mapping_state, call, call_sqs, create_mapping, create_queue,
    get_mapping, queue_held, router_on, LOOK_AGAIN, STATE_ENABLED,
};
use crate::handler_secret_relayed_to_queue::{partial_arn, secrets};
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The runtime the relay is created with, and the module and attribute its package exposes its
/// handler as.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `eventSource` a record of a queue message carries.
const EVENT_SOURCE: &str = "aws:sqs";

/// How long one invocation of this handler is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the chain and not how fast an
/// interpreter gets through a handler.
const FUNCTION_TIMEOUT: i64 = 30;

/// How long each source queue hides a message it hands over for. A mapping is only made onto a
/// function the queue outlasts ("Queue visibility timeout: … is less than Function timeout:
/// …"), so this is above [`FUNCTION_TIMEOUT`].
const VISIBILITY_TIMEOUT: i64 = 60;

/// The secret the handler reads, the version id the create stakes it under (inside the
/// modelled 32..64 length of `ClientRequestTokenType`) and the value that version holds.
const SECRET_NAME: &str = "esm-secret-relay/credentials";
const SECRET_VERSION: &str = "esm-secret-relay-pipeline-version-000001";
const PASSWORD: &str = "relayed-through-the-poller";

/// The message attribute each message names its secret under, bound to the name the handler
/// reads it by so there is ONE spelling of it on both sides of the interpreter.
const SECRET_ATTRIBUTE: &str = "secret";

/// How long a row watches the queues for the state the clock owes — the mapping settling, a
/// pass polling, the handler's own run and its send landing, the processed messages being
/// deleted — before the row is a failure.
///
/// A CEILING on patience taken from what the EMULATOR allows ONE attempt rather than picked
/// here: the wait for a share of this process's run capacity ([`PERMIT_WAIT`]) and the wait for
/// the run's own answer ([`HOST_WAIT_CEILING`]), with room for the several passes a row waits
/// for, because a whole-workspace run shares its cores with every other suite. Every assertion
/// below is unchanged by it.
const WATCH_BUDGET: Duration = PERMIT_WAIT
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(Duration::from_secs(30));

/// The handler every row runs: for each record of the batch it was handed, read the secret that
/// record names and relay what came back — beside the record's own envelope members and how
/// many records the batch held — to the queue its environment names.
///
/// Both clients are built at module scope, which is where a managed runtime's handler builds
/// them.
fn relay_handler() -> String {
    format!(
        r#"
import json
import os

import boto3

SECRETS = boto3.client("secretsmanager")
QUEUES = boto3.client("sqs")
SINK_URL = os.environ["SINK_URL"]


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    relayed = []
    for record in records:
        named = record["messageAttributes"]["{SECRET_ATTRIBUTE}"]["stringValue"]
        held = SECRETS.get_secret_value(SecretId=named)
        QUEUES.send_message(
            QueueUrl=SINK_URL,
            MessageBody=json.dumps(
                {{
                    "body": record["body"],
                    "message_id": record["messageId"],
                    "event_source": record["eventSource"],
                    "event_source_arn": record["eventSourceARN"],
                    "batch": len(records),
                    "secret": json.loads(held["SecretString"])["password"],
                }},
                sort_keys=True,
            ),
        )
        relayed.append(record["messageId"])
    return {{"relayed": relayed}}
"#
    )
}

/// Lambda over the SAME engine as every service the chain touches — Amazon SQS, where both
/// queues live, Secrets Manager, where the secret lives, the IAM store the execution role lives
/// in and the token service its session is minted by — which is what `core/router` assembles.
fn pipeline_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// How a row's records name the secret: the forms `GetSecretValueRequest$SecretId` accepts —
/// "the ARN or name of the secret", with the partial ARN accepted beside the complete one.
#[derive(Clone, Copy, Debug)]
enum Named {
    Name,
    CompleteArn,
    PartialArn,
}

/// One row: the messages waiting on its own source queue, the `BatchSize` they are read in, and
/// the form their records name the secret by.
struct Row {
    case: &'static str,
    bodies: Vec<String>,
    batch_size: i64,
    named: Named,
}

/// The catalog: one message and several, a batch ceiling above what is waiting and below it,
/// one message per batch, each form of the secret id, and bodies that are plain text, a JSON
/// document and text that is not ASCII.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one message, a batch ceiling of ten, the secret named by name",
            bodies: vec!["order-1".to_string()],
            batch_size: 10,
            named: Named::Name,
        },
        Row {
            case: "three json messages read in one batch, named by the complete ARN",
            bodies: (0..3)
                .map(|index| json!({ "order": index, "items": ["a", "b"] }).to_string())
                .collect(),
            batch_size: 10,
            named: Named::CompleteArn,
        },
        Row {
            case: "three messages that are not ASCII, read one per batch, named by a partial ARN",
            bodies: (0..3)
                .map(|index| format!("ünïcödé ⚑ commande n°{index}"))
                .collect(),
            batch_size: 1,
            named: Named::PartialArn,
        },
        Row {
            case: "four messages with a batch ceiling of two",
            bodies: (0..4).map(|index| format!("order-{index}")).collect(),
            batch_size: 2,
            named: Named::Name,
        },
    ]
}

/// The queue name a row's source queue is created under.
fn source_name(index: usize) -> String {
    format!("esm-secret-relay-source-{index}")
}

/// `SendMessage` of `body` to the queue `url` addresses, naming `secret_id` as the secret the
/// record's handler is to read: the `MessageId` Amazon SQS minted for it.
async fn send_naming_secret(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    body: &str,
    secret_id: &str,
) -> String {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": body,
            "MessageAttributes": {
                SECRET_ATTRIBUTE: { "DataType": "String", "StringValue": secret_id },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
    sent["MessageId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no MessageId in {sent}"))
        .to_string()
}

/// Create the relay running as `role_arn`, pointed at the sink queue `sink_url` addresses, and
/// wait until it reports `Active`.
async fn create_relay(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    sink_url: &str,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { "SINK_URL": sink_url } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&relay_handler())),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction {name:?} to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// Every document the handler has relayed to the sink queue so far, keyed by the `body` it
/// reports — read until `wanted` of them have arrived or the budget is out.
///
/// The wait is for the DOCUMENTS, not for a span: a host that is slow to run a handler delays
/// this loop instead of weakening what it asserts.
async fn relayed_documents(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> BTreeMap<String, Value> {
    let mut relayed: BTreeMap<String, Value> = BTreeMap::new();
    let deadline = Instant::now() + WATCH_BUDGET;
    while relayed.len() < wanted && Instant::now() < deadline {
        let (status, answered) = call_sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                // Long enough that a document counted here is not handed over again while the
                // rest arrive.
                "VisibilityTimeout": 300,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ReceiveMessage on the sink queue to answer 200, got \
             {status} {answered}"
        );
        for message in answered["Messages"].as_array().cloned().unwrap_or_default() {
            let text = message["Body"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a relayed message with no Body"));
            let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a relayed document that is not JSON: {text} ({error})")
            });
            let body = document["body"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a relayed document carrying no body: {document}")
                })
                .to_string();
            relayed.insert(body, document);
        }
        if relayed.len() < wanted {
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
    relayed
}

/// Wait until the queue `url` addresses holds nothing, answering what it last held.
async fn drained(router: &MultiServiceRouter, case: &str, url: &str) -> u32 {
    let deadline = Instant::now() + WATCH_BUDGET;
    loop {
        let held = queue_held(router, case, url).await;
        if held == 0 || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: an `Enabled` mapping on a queue hands its function one `aws:sqs` record per message it
/// took, at most `BatchSize` of them, and deletes exactly the messages the invocation
/// processed — so each message reaches the handler once, the calls that handler makes take
/// effect in the services that own them, and the source queue drains.
#[tokio::test]
async fn a_mapping_on_a_queue_relays_each_message_through_a_run_of_its_function() {
    let setup = "setup";
    let router = pipeline_router();
    let rows = rows();

    // The sink queue the handler relays to, and one source queue per row.
    let (sink_url, sink_arn) = create_queue(&router, setup, "esm-secret-relay-sink", &json!({})).await;
    let mut sources: Vec<(String, String)> = Vec::new();
    for index in 0..rows.len() {
        sources.push(
            create_queue(
                &router,
                setup,
                &source_name(index),
                &json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT.to_string() }),
            )
            .await,
        );
    }

    // The secret the handler reads, held under one version.
    let created = secrets(
        &router,
        setup,
        "CreateSecret",
        &json!({
            "Name": SECRET_NAME,
            "ClientRequestToken": SECRET_VERSION,
            "SecretString": json!({ "username": "relay", "password": PASSWORD }).to_string(),
        }),
    )
    .await;
    let secret_arn = created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {setup:?}: CreateSecret reported no ARN in {created}"))
        .to_string();

    // The execution role: the queue reads the poller makes on each source queue
    // (`AWSLambdaSQSQueueExecutionRole`'s own grant), the secret read and the send the handler
    // makes, and nothing else.
    let mut statements = vec![
        logs_only(),
        json!({
            "Effect": "Allow",
            "Action": "secretsmanager:GetSecretValue",
            "Resource": "*",
        }),
        allow("sqs", &["SendMessage"], &sink_arn),
    ];
    for (_, arn) in &sources {
        statements.push(allow("sqs", &QUEUE_READS, arn));
    }
    let role_arn = given_role(
        &router,
        setup,
        "esm-secret-relay-role",
        Some(Value::Array(statements)),
    )
    .await;

    let function = "esm-secret-relay-fn";
    create_relay(&router, setup, function, &role_arn, &sink_url).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (source_url, source_arn) = &sources[index];
        let secret_id = match row.named {
            Named::Name => SECRET_NAME.to_string(),
            Named::CompleteArn => secret_arn.clone(),
            Named::PartialArn => partial_arn(&secret_arn),
        };

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": source_arn,
                "BatchSize": row.batch_size,
                "Enabled": true,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

        // Nothing invokes the function: the messages are sent, and everything after this is a
        // READ of what the mapping's own poller brought about.
        let mut sent_ids: BTreeMap<String, String> = BTreeMap::new();
        for body in &row.bodies {
            sent_ids.insert(
                body.clone(),
                send_naming_secret(&router, case, source_url, body, &secret_id).await,
            );
        }

        let relayed = relayed_documents(&router, case, &sink_url, row.bodies.len()).await;
        assert_eq!(
            relayed.len(),
            row.bodies.len(),
            "case {case:?}: every message sent must reach the handler as a record of a batch \
             the mapping delivered, got {} of {} ({:?})",
            relayed.len(),
            row.bodies.len(),
            relayed.keys().collect::<Vec<_>>()
        );

        for body in &row.bodies {
            let document = relayed.get(body).unwrap_or_else(|| {
                panic!("case {case:?}: nothing was relayed for the body {body:.48?}")
            });
            // The record was about THAT message, as the queue holds it.
            let expected_id = sent_ids.get(body).cloned().unwrap_or_default();
            assert_eq!(
                document["message_id"].as_str(),
                Some(expected_id.as_str()),
                "case {case:?}: expected the record of {body:.48?} to carry the sent message's \
                 id {expected_id:?}, got {document}"
            );
            // And it named the queue it came from.
            assert_eq!(
                (
                    document["event_source"].as_str(),
                    document["event_source_arn"].as_str(),
                ),
                (Some(EVENT_SOURCE), Some(source_arn.as_str())),
                "case {case:?}: expected the record of {body:.48?} to name its source queue, \
                 got {document}"
            );
            // The handler really ran, and its own call reached the service that owns the
            // secret: what the relay carries is the value Secrets Manager holds.
            assert_eq!(
                document["secret"].as_str(),
                Some(PASSWORD),
                "case {case:?}: expected the relay of {body:.48?} to carry the value the secret \
                 holds, got {document}"
            );
            // "The maximum number of records in each batch that Lambda pulls from your stream
            // or queue and sends to your function."
            let batch = document["batch"].as_i64().unwrap_or_default();
            assert!(
                (1..=row.batch_size).contains(&batch),
                "case {case:?}: a batch holds one record per message and at most the \
                 BatchSize of {}, got a batch of {batch} for {body:.48?} in {document}",
                row.batch_size
            );
        }

        // The messages the invocations processed were deleted, so the queue the mapping polls
        // drains.
        let held = drained(&router, case, source_url).await;
        assert_eq!(
            held, 0,
            "case {case:?}: every message was processed, so the source queue must drain, got \
             {held} still held"
        );

        // And the mapping says what became of the batches it delivered: "the result of the
        // event source mapping's last processing attempt".
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.string("LastProcessingResult").as_deref(),
            Some(LAST_PROCESSING_RESULT_OK),
            "case {case:?}: expected the mapping to report {LAST_PROCESSING_RESULT_OK}, got {} {}",
            reading.status,
            reading.text()
        );
    }
}
