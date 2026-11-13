// SPDX-License-Identifier: CC0-1.0

//! The THREE-HOP fan-out pipeline, end to end with no client action after the publication:
//! a message published to an Amazon SNS topic is enqueued on the Amazon SQS queue an
//! `sqs`-protocol subscription fans it out to, the Lambda event source mapping on that queue
//! polls it, invokes the mapped function with it, and deletes it once the invocation
//! succeeded — and the AWS call the function's own handler makes is served by this emulator,
//! so the effect of the delivery is readable through the called service's own API.
//!
//! The RULE asserted over the table below, in one sentence: an `Enabled` event source
//! mapping on an Amazon SQS queue reads the messages that queue HOLDS, whoever put them
//! there ("Lambda reads items from the event source and invokes the function"), hands its
//! function one `aws:sqs` record per message whose `body` is that message's body exactly as
//! the queue holds it — the notification envelope for a message an SNS subscription
//! delivered, the published message itself for a raw-delivery subscription or a client's own
//! send — and deletes a message only once the invocation it was handed to has succeeded, so
//! a queue whose batches are all processed drains.
//!
//! Nothing here invokes the function: after the publication (or the send) this suite only
//! READS — the table the handler writes, the queue's own message counts and the mapping's
//! `LastProcessingResult`. Every item it reads back was written by a run the poller started,
//! out of the envelope that run was handed, so a mapping that reports healthy and delivers
//! nothing leaves the table empty and the row fails, which is exactly the symptom this
//! pipeline was filed for.
//!
//! The rows differ along what the rule generalises over: HOW each message reached the queue
//! (a publication through an `sqs`-protocol subscription, the same with
//! `RawMessageDelivery`, a client's own `SendMessage`, and a queue holding messages from
//! both routes at once), how many messages there are, and the `BatchSize` they are read in.
//! Every expected value is computed here from what the row published or sent, never read
//! back out of the answer being checked.
//!
//! The Lambda, Amazon SQS and mapping helpers are `event_source_mappings.rs`'s, and the role
//! fixture is `event_source_mapping_role_access.rs`'s, so every suite over this surface
//! drives the same wire.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_OK;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{
    allow, element, form_encode, given_role, logs_only, QUEUE_READS,
};
use crate::event_source_mappings::{
    await_function_active, await_mapping_state, call, call_sqs, create_mapping, create_queue,
    get_mapping, queue_held, router_on, send_message, LOOK_AGAIN, STATE_ENABLED,
};
use crate::synchronous_invoke_run::package;

/// The Region every call below signs for, and the Region every ARN carries.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The runtime every function below is created with, and the module and attribute its
/// package exposes its handler as.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long one invocation of this handler is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the delivery and not how fast an
/// interpreter gets through a handler.
const FUNCTION_TIMEOUT: i64 = 30;

/// How long each source queue hides a message it hands over for. A mapping is only made
/// onto a function the queue outlasts ("Queue visibility timeout: … is less than Function
/// timeout: …"), so this is above [`FUNCTION_TIMEOUT`].
const VISIBILITY_TIMEOUT: i64 = 60;

/// How long a wait for work nothing asked for — the subscription's delivery, the mapping
/// settling, a batch polled, the handler's own write landing, the queue draining — is given
/// before the row is a failure. Well above the poller's interval, the state-settle span and
/// one interpreted run of the handler, so a row fails only when the work did not happen at
/// all.
const BUDGET: Duration = Duration::from_secs(120);

/// The attribute the sink item is keyed by, and the three the handler stamps it with. None
/// of the four is read by this emulator: they are the handler's own, asserted here.
const KEY_ATTRIBUTE: &str = "pk";
/// See [`KEY_ATTRIBUTE`].
const DELIVERY_ATTRIBUTE: &str = "delivery";
/// See [`KEY_ATTRIBUTE`].
const QUEUE_ATTRIBUTE: &str = "queueArn";
/// See [`KEY_ATTRIBUTE`].
const MESSAGE_ID_ATTRIBUTE: &str = "messageId";

/// What the handler stamps [`DELIVERY_ATTRIBUTE`] with for a body that is an SNS
/// notification envelope, and for one that is the published message itself.
const DELIVERED_AS_NOTIFICATION: &str = "notification";
/// See [`DELIVERED_AS_NOTIFICATION`].
const DELIVERED_RAW: &str = "raw";

/// The service principal Amazon SNS delivers to a subscribed queue as, which is the identity
/// a queue policy has to grant for a notification to reach the queue ("you must ensure that
/// the Amazon SQS queue policy allows the Amazon SNS topic to send messages to the queue").
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The member of the published message the handler keys its item by.
const ORDER_ID: &str = "order_id";

/// Lambda, Amazon SNS, Amazon SQS, Amazon DynamoDB and IAM over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the publication is made where a client makes it,
/// the queue the subscription delivers to is the queue the mapping polls, the role Lambda
/// polls as is the one IAM holds, and the handler's own SDK call is dispatched back into this
/// same router — so the table this suite reads is the table the handler wrote.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to Amazon SNS that must be served, as the XML it answered.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/sns/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SNS {action}, got {status} / {text}"
    );
    text
}

/// One DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must be served.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{DYNAMODB_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} {parsed}"
    );
    parsed
}

/// The sink table the handler writes, waited out to `ACTIVE` — "you can perform read and
/// write operations only on an ACTIVE table" — and answered as its ARN.
async fn given_table(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{ "AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH" }],
            "AttributeDefinitions": [
                { "AttributeName": KEY_ATTRIBUTE, "AttributeType": "S" },
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TableArn in {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, case, "DescribeTable", &json!({ "TableName": name })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table {name:?} never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The queue policy a client that fans a topic out to a queue writes: `sqs:SendMessage` to
/// the Amazon SNS service principal on this queue, scoped to the publishing topic by
/// `aws:SourceArn`.
fn grant_to_the_topic(queue_arn: &str, topic_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowDeliveryFromTheTopic",
            "Effect": "Allow",
            "Principal": { "Service": SNS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string()
}

/// A topic called `name`, with the queue `queue_arn` names subscribed to it over the `sqs`
/// protocol — raw-delivery when the row asks for it — and the queue's own `Policy` written to
/// admit that topic's deliveries. Answers the topic's ARN.
async fn given_topic_fanning_out(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_url: &str,
    queue_arn: &str,
    raw_delivery: bool,
) -> String {
    let created = sns(router, case, "CreateTopic", &[("Name", name)]).await;
    let topic_arn = element(&created, "TopicArn");

    let (status, written) = call_sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": queue_url,
            "Attributes": { "Policy": grant_to_the_topic(queue_arn, &topic_arn) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SetQueueAttributes to answer 200, got {status} {written}"
    );

    let mut params = vec![
        ("TopicArn", topic_arn.as_str()),
        ("Protocol", "sqs"),
        ("Endpoint", queue_arn),
        ("ReturnSubscriptionArn", "true"),
    ];
    if raw_delivery {
        // "RawMessageDelivery – When set to true, enables raw message delivery to Amazon SQS
        // endpoints", so the queue holds the published message itself.
        params.push(("Attributes.entry.1.key", "RawMessageDelivery"));
        params.push(("Attributes.entry.1.value", "true"));
    }
    let subscribed = sns(router, case, "Subscribe", &params).await;
    let subscription_arn = element(&subscribed, "SubscriptionArn");
    assert!(
        subscription_arn.starts_with(&topic_arn),
        "case {case:?}: expected the subscription to belong to {topic_arn:?}, got \
         {subscription_arn:?}"
    );
    topic_arn
}

/// The handler every row runs: for each record of the batch it was handed it reads that
/// record's own body — unwrapping the notification envelope when the body is one — and writes
/// ONE item to the sink table through the SDK the runtime supplies, stamping it with the
/// envelope members the record arrived with.
///
/// Every value it writes comes from the record or from its own environment, so an item in
/// the table is evidence that the message reached the handler in the shape Lambda hands an
/// Amazon SQS batch over in.
fn handler_source() -> String {
    format!(
        r#"
import json
import os

import boto3

TABLE = os.environ["SINK_TABLE"]
QUEUE_ARN = os.environ["QUEUE_ARN"]
CLIENT = boto3.client("dynamodb")


def lambda_handler(event, context):
    written = []
    for record in event["Records"]:
        if record["eventSource"] != "aws:sqs":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != QUEUE_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        if not record["messageId"]:
            raise ValueError("a record carries no messageId")
        body = json.loads(record["body"])
        notification = body.get("Type") == "Notification"
        message = json.loads(body["Message"]) if notification else body
        CLIENT.put_item(
            TableName=TABLE,
            Item={{
                "{KEY_ATTRIBUTE}": {{"S": message["{ORDER_ID}"]}},
                "{DELIVERY_ATTRIBUTE}": {{
                    "S": "{DELIVERED_AS_NOTIFICATION}" if notification else "{DELIVERED_RAW}"
                }},
                "{QUEUE_ATTRIBUTE}": {{"S": record["eventSourceARN"]}},
                "{MESSAGE_ID_ATTRIBUTE}": {{"S": record["messageId"]}},
            }},
        )
        written.append(message["{ORDER_ID}"])
    return {{"written": written}}
"#
    )
}

/// A function whose package RUNS [`handler_source`], running as `role_arn` and told through
/// its environment which table to write and which queue its records must come from, waited
/// out to `Active`.
async fn given_handler_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
    table: &str,
    queue_arn: &str,
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
            "Environment": {
                "Variables": { "SINK_TABLE": table, "QUEUE_ARN": queue_arn },
            },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&handler_source())),
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

/// Publish `message` on the topic `topic_arn` names.
async fn publish(router: &MultiServiceRouter, case: &str, topic_arn: &str, message: &str) {
    sns(
        router,
        case,
        "Publish",
        &[("TopicArn", topic_arn), ("Message", message)],
    )
    .await;
}

/// The item the sink table holds under `key`, read with `ConsistentRead` as a client polling
/// for the effect reads it.
async fn sink_item(router: &MultiServiceRouter, case: &str, table: &str, key: &str) -> Value {
    let read = dynamodb(
        router,
        case,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { KEY_ATTRIBUTE: { "S": key } },
            "ConsistentRead": true,
        }),
    )
    .await;
    read["Item"].clone()
}

/// Wait until the sink table holds an item under `key`, answering it.
async fn wait_for_sink_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let item = sink_item(router, case, table, key).await;
        if !item.is_null() {
            return item;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping must hand the message to its function, whose handler \
             writes the sink item under {key:?}, but no item ever appeared"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every key the sink table holds an item under, read through DynamoDB's own `Scan`.
async fn sink_keys(router: &MultiServiceRouter, case: &str, table: &str) -> Vec<String> {
    let scanned = dynamodb(
        router,
        case,
        "Scan",
        &json!({ "TableName": table, "ConsistentRead": true }),
    )
    .await;
    let mut keys: Vec<String> = scanned["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item[KEY_ATTRIBUTE]["S"].as_str().map(str::to_string))
        .collect();
    keys.sort();
    keys
}

/// Wait until the queue `url` addresses holds nothing at all, answering what it last held.
async fn wait_until_drained(router: &MultiServiceRouter, case: &str, url: &str) -> u32 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let held = queue_held(router, case, url).await;
        if held == 0 || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One message a row puts on its queue: the `order_id` it carries, whether it gets there by
/// being PUBLISHED to the topic rather than sent to the queue, and what the handler must
/// stamp the item it writes for it with.
struct Message {
    order_id: String,
    published: bool,
    delivery: &'static str,
}

/// One row: how the queue comes to hold its messages, and how they are read.
struct Row {
    case: &'static str,
    /// How many messages reach the queue through the topic's `sqs` subscription.
    published: usize,
    /// How many the client sends to the queue itself.
    sent: usize,
    /// Whether the subscription is a raw-delivery one, which puts the published message on
    /// the queue instead of the notification envelope.
    raw_delivery: bool,
    /// The `BatchSize` the mapping reads with.
    batch_size: i64,
}

impl Row {
    /// Each message this row's queue comes to hold: a notification the subscription wraps
    /// reaches the handler as the envelope, a raw delivery and a client's own send as the
    /// published message itself.
    fn messages(&self) -> Vec<Message> {
        let wrapped = if self.raw_delivery {
            DELIVERED_RAW
        } else {
            DELIVERED_AS_NOTIFICATION
        };
        let mut messages: Vec<Message> = (0..self.published)
            .map(|index| Message {
                order_id: format!("{}-published-{index}", self.case),
                published: true,
                delivery: wrapped,
            })
            .collect();
        messages.extend((0..self.sent).map(|index| Message {
            order_id: format!("{}-sent-{index}", self.case),
            published: false,
            delivery: DELIVERED_RAW,
        }));
        messages
    }
}

/// The body a message carrying `order_id` travels as, both when it is published to the topic
/// and when it is sent to the queue.
fn message_body(order_id: &str) -> String {
    json!({ ORDER_ID: order_id }).to_string()
}

/// RULE: an `Enabled` mapping on an Amazon SQS queue reads the messages that queue holds
/// whoever enqueued them, invokes the mapped function with the `aws:sqs` envelope of each —
/// whose `body` is the message's body as the queue holds it — and deletes a message only on
/// a successful invocation, so the queue drains; and the SDK call that invocation's handler
/// makes is served by this emulator, so the item it wrote is in the table a client reads.
#[tokio::test]
async fn a_publication_traverses_the_subscription_and_the_mapping_to_its_functions_own_write() {
    let rows = [
        // The recorded shape: one publication, fanned out to the queue by an `sqs`
        // subscription, read by a mapping that may take a whole batch.
        Row {
            case: "one-notification",
            published: 1,
            sent: 0,
            raw_delivery: false,
            batch_size: 10,
        },
        // Several publications, so the mapping has more than one message to account for.
        Row {
            case: "three-notifications",
            published: 3,
            sent: 0,
            raw_delivery: false,
            batch_size: 10,
        },
        // A raw-delivery subscription beside a client's own send: the queue holds the
        // published message itself either way, and both are delivered.
        Row {
            case: "raw-delivery-and-a-clients-own-send",
            published: 1,
            sent: 1,
            raw_delivery: true,
            batch_size: 10,
        },
        // The two routes at once, read ONE message per batch: the mapping accounts for every
        // message the queue holds whoever enqueued it, over as many batches as it takes.
        Row {
            case: "both-routes-one-message-per-batch",
            published: 1,
            sent: 1,
            raw_delivery: false,
            batch_size: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        // The names are indexed rather than spelled out of the case, which would run past
        // the length `FunctionName` and `RoleName` admit.
        let table = format!("pipeline-sink-{index}");
        let function = format!("pipeline-consumer-{index}");

        let (queue_url, queue_arn) = create_queue(
            &router,
            case,
            &format!("pipeline-queue-{index}"),
            &json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT.to_string() }),
        )
        .await;
        let table_arn = given_table(&router, case, &table).await;
        // Lambda polls the queue acting as the function's execution role, and the handler
        // writes the table as itself: the role carries both grants, as a client that wires
        // this pipeline together gives it.
        let role = given_role(
            &router,
            case,
            &format!("pipeline-role-{index}"),
            Some(json!([
                logs_only(),
                allow("sqs", &QUEUE_READS, &queue_arn),
                allow("dynamodb", &["PutItem"], &table_arn),
            ])),
        )
        .await;
        let topic_arn = given_topic_fanning_out(
            &router,
            case,
            &format!("pipeline-topic-{index}"),
            &queue_url,
            &queue_arn,
            row.raw_delivery,
        )
        .await;
        given_handler_function(&router, case, &function, &role, &table, &queue_arn).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": queue_arn,
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

        // The one data-plane action of the row: each message is published to the topic, or
        // sent to the queue, and nothing else is asked of the emulator afterwards.
        let messages = row.messages();
        for message in &messages {
            let body = message_body(&message.order_id);
            if message.published {
                publish(&router, case, &topic_arn, &body).await;
            } else {
                send_message(&router, case, &queue_url, &body, None).await;
            }
        }

        for message in &messages {
            let order_id = message.order_id.as_str();
            let delivery = message.delivery;
            let item = wait_for_sink_item(&router, case, &table, order_id).await;
            assert_eq!(
                item[KEY_ATTRIBUTE],
                json!({ "S": order_id }),
                "case {case:?}: expected the item to be keyed by {order_id:?}, got {item}"
            );
            // What the handler read out of the record's own body: the envelope an `sqs`
            // subscription wraps a notification in, or the published message itself.
            assert_eq!(
                item[DELIVERY_ATTRIBUTE],
                json!({ "S": delivery }),
                "case {case:?}: expected the record's body to reach the handler as {delivery:?}, \
                 got {item}"
            );
            assert_eq!(
                item[QUEUE_ATTRIBUTE],
                json!({ "S": queue_arn }),
                "case {case:?}: expected the envelope to name {queue_arn:?} as its \
                 eventSourceARN, got {item}"
            );
            let message_id = item[MESSAGE_ID_ATTRIBUTE]["S"].as_str().unwrap_or_default();
            assert!(
                !message_id.is_empty(),
                "case {case:?}: expected the record to carry the message's own id, got {item}"
            );
        }

        // And nothing else: no item is written for a message nobody published or sent.
        let mut expected: Vec<String> = messages
            .iter()
            .map(|message| message.order_id.clone())
            .collect();
        expected.sort();
        let held = sink_keys(&router, case, &table).await;
        assert_eq!(
            held, expected,
            "case {case:?}: expected the sink to hold exactly {expected:?}, got {held:?}"
        );

        // A message leaves the queue once the invocation it was handed to succeeded, so a
        // queue whose every batch was processed holds nothing.
        let remaining = wait_until_drained(&router, case, &queue_url).await;
        assert_eq!(
            remaining, 0,
            "case {case:?}: expected the mapping to delete every message it processed, the \
             queue still holds {remaining}"
        );
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            reading.string("LastProcessingResult").as_deref(),
            Some(LAST_PROCESSING_RESULT_OK),
            "case {case:?}: expected the mapping to report {LAST_PROCESSING_RESULT_OK} for the \
             attempt that consumed its batch, got {}",
            reading.text()
        );
    }
}
