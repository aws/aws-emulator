// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon MQ for RABBITMQ broker CONSUMES, asserted end
//! to end: the messages are published over the broker's own `amqps://` endpoint by an AMQP
//! 0-9-1 client of this emulator, Lambda's own poller takes them out of the queue the mapping
//! names, and the handler of a real deployment package forwards each record it was handed to a
//! queue the test reads — so a body that appears there is one the mapping really delivered.
//!
//! The RULE asserted over the table below, in one sentence: a mapping that reports `Enabled`
//! on a broker CONSUMES the one queue its `Queues` names on that broker — "Lambda reads items
//! from the event source and invokes the function" — taking at most `BatchSize` messages per
//! batch, handing its function exactly the ones its `FilterCriteria` admit under the
//! `rmqMessagesByQueue` key the queue and the broker's virtual host make, and acknowledging
//! the whole batch it took; a mapping whose queue the broker does not hold, and one whose
//! polling is paused, consume nothing at all — and a mapping that cannot consume the queue it
//! names reports that it could not reach its source as its `LastProcessingResult`, which is
//! how "this mapping delivers nothing" is told apart from "nothing was ever polled" — and that
//! reading is replaced by itself once the queue it names is declared.
//!
//! The rows differ along what the rule generalises over: how many messages are waiting, how
//! many one batch may hold, whether criteria decide which of them are handed over, whether
//! the queue the mapping names exists on the broker at all, and whether the mapping is
//! polling. A second case holds the rule to the broker's ENGINE: a mapping on an ActiveMQ
//! broker, whose batches are a different event, is left untouched while one on a RabbitMQ
//! broker of the same emulator delivers.
//!
//! Each row that must deliver NOTHING carries a CONTROL mapping of its own, on a queue of the
//! same broker that does exist: the control's own delivery is what tells "this mapping
//! delivers nothing" apart from "nothing in this pipeline delivers at all", which is the
//! distinction that makes the negative worth asserting.
//!
//! The envelope itself is asserted from INSIDE the run: the handler raises unless the event
//! names the RabbitMQ event source and the broker's own ARN, holds its records under exactly
//! the one expected key, carries no more records than one batch may hold, and reports each
//! record's `bodySize`, `contentType` and `redelivered` as the publish left them — so a
//! forwarded body is also proof of the form the batch was handed over in.

use std::sync::Arc;
use std::time::Duration;

use amq_protocol::frame::{gen_frame, parse_frame, AMQPContentHeader, AMQPFrame};
use amq_protocol::protocol::basic::AMQPProperties;
use amq_protocol::protocol::{basic, channel, connection, queue, AMQPClass};
use amq_protocol::types::{FieldTable, LongString, ShortString};
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED, LAST_PROCESSING_RESULT_NO_RECORDS,
    LAST_PROCESSING_RESULT_OK,
};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::event_source_mappings::{
    await_mapping_state, create_mapping, create_queue, create_request, create_running_function,
    get_mapping, mq_call, router_with_queues, taken_bodies, LOOK_AGAIN, RESULTS_QUEUE_URL,
    STATE_DISABLED, STATE_ENABLED,
};

/// The user every broker below is created with, and the password that user authenticates
/// with: a broker is created with its users inline, and this AMQP client dials as that user.
const BROKER_USER: &str = "mq-esm-user";
/// See [`BROKER_USER`].
const BROKER_PASSWORD: &str = "Str0ng-Esm-Pass";

/// The queue each row's mapping consumes, declared on the broker by the publisher below.
const SOURCE_QUEUE: &str = "esm-orders";

/// The queue a row's CONTROL mapping consumes, declared on the same broker.
const CONTROL_QUEUE: &str = "esm-control";

/// The queue name a row's mapping names when the rule under test is that a queue the broker
/// does NOT hold hands nothing over: nothing below declares it.
const ABSENT_QUEUE: &str = "esm-absent";

/// The virtual host every broker of this emulator serves, which is the one this client opens
/// and the one the `rmqMessagesByQueue` key names the queue inside.
const VIRTUAL_HOST: &str = "/";

/// The address the core binds its listeners on by default, which is the address a broker of
/// this emulator publishes and the only one this suite dials.
const LOOPBACK: &str = "127.0.0.1";

/// The `contentType` every publish below sets, which the handler asserts it was handed.
const CONTENT_TYPE: &str = "application/json";

/// The body a row's control publishes, whose delivery says the pipeline is live.
const CONTROL_BODY: &str = r#"{"control":"sentinel"}"#;

/// How long a row waits for work no caller controls — a broker reaching `RUNNING`, a function
/// going `Active`, a mapping settling, a batch reaching the handler and the handler's forward
/// arriving. Generous: a row fails only when the work did not happen at all, so a loaded host
/// cannot decide the outcome. Nothing here is asserted about elapsed time.
const BUDGET: Duration = Duration::from_secs(120);

/// The frame size this client negotiates, which is the one the broker offers.
const FRAME_MAX: usize = 131_072;

/// How long one frame read waits. A session that is owed a frame gets it immediately; this
/// only bounds a test that would otherwise hang.
const FRAME_WAIT: Duration = Duration::from_secs(30);

/// The `Timeout` every function below runs inside, in seconds.
const FUNCTION_TIMEOUT: i64 = 10;

/// The handler every function below runs: it asserts the shape of the envelope Lambda hands a
/// RabbitMQ batch over as, and forwards each record's own message to the queue its environment
/// names — so a body on that queue is one that reached the handler inside a well-formed event.
const FORWARDING_HANDLER: &str = r#"
import base64
import os

import boto3


def lambda_handler(event, context):
    if event["eventSource"] != "aws:rmq":
        raise ValueError("eventSource is " + repr(event.get("eventSource")))
    if event["eventSourceArn"] != os.environ["BROKER_ARN"]:
        raise ValueError("eventSourceArn is " + repr(event.get("eventSourceArn")))
    by_queue = event["rmqMessagesByQueue"]
    key = os.environ["QUEUE_KEY"]
    if list(by_queue) != [key]:
        raise ValueError("rmqMessagesByQueue holds " + repr(list(by_queue)))
    records = by_queue[key]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    if len(records) > int(os.environ["MAX_RECORDS"]):
        raise ValueError("a batch of " + str(len(records)) + " records")
    client = boto3.client("sqs")
    url = os.environ["RESULTS_QUEUE_URL"]
    for record in records:
        data = base64.b64decode(record["data"])
        properties = record["basicProperties"]
        if properties["bodySize"] != len(data):
            raise ValueError("bodySize is " + repr(properties["bodySize"]))
        if properties["contentType"] != os.environ["CONTENT_TYPE"]:
            raise ValueError("contentType is " + repr(properties["contentType"]))
        if record["redelivered"] is not False:
            raise ValueError("redelivered is " + repr(record["redelivered"]))
        client.send_message(QueueUrl=url, MessageBody=data.decode())
    return {"forwarded": len(records)}
"#;

/// One row of the table.
struct Row {
    case: &'static str,
    /// The queue the row's mapping names in its `Queues`.
    mapped_queue: &'static str,
    /// `Enabled`, as the create states it.
    enabled: bool,
    /// `BatchSize`, as the create states it.
    batch_size: i64,
    /// The `FilterCriteria.Filters[].Pattern` values, in order; empty for a mapping carrying
    /// no criteria at all.
    patterns: &'static [&'static str],
    /// The bodies published to [`SOURCE_QUEUE`], in order.
    published: &'static [&'static str],
    /// The bodies the handler must be handed, as a set.
    delivered: &'static [&'static str],
    /// How many messages [`SOURCE_QUEUE`] must still hold once the row has settled, for a row
    /// whose mapping consumes nothing of it.
    left_on_the_queue: Option<u32>,
    /// The readings the mapping's `LastProcessingResult` may settle on, and none for a
    /// mapping no pass was ever made for. A mapping that consumed its queue reports `OK` for
    /// the batch it handed over and then that no record was processed as it keeps polling the
    /// queue it drained, so both are a settled reading of the same row; a mapping that cannot
    /// consume its queue reports the connect failure alone; a mapping whose polling is paused
    /// has made no pass, so it reports nothing at all.
    settled_results: &'static [&'static str],
}

/// The rows: one per shape of the consumption under test.
const ROWS: &[Row] = &[
    // The simplest delivery: one message waiting on the queue the mapping names.
    Row {
        case: "one-message-on-the-queue-the-mapping-names",
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[],
        published: &[r#"{"order":"one"}"#],
        delivered: &[r#"{"order":"one"}"#],
        left_on_the_queue: None,
        settled_results: DRAINED_RESULTS,
    },
    // Several messages with room for them all in one batch.
    Row {
        case: "three-messages-with-room-for-them-all",
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[],
        published: &[
            r#"{"order":"one"}"#,
            r#"{"order":"two"}"#,
            r#"{"order":"three"}"#,
        ],
        delivered: &[
            r#"{"order":"one"}"#,
            r#"{"order":"two"}"#,
            r#"{"order":"three"}"#,
        ],
        left_on_the_queue: None,
        settled_results: DRAINED_RESULTS,
    },
    // The same messages with a batch that holds ONE: every message is still delivered, one
    // batch at a time, which the handler's own bound on a batch's size is the proof of.
    Row {
        case: "three-messages-one-to-a-batch",
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 1,
        patterns: &[],
        published: &[
            r#"{"order":"one"}"#,
            r#"{"order":"two"}"#,
            r#"{"order":"three"}"#,
        ],
        delivered: &[
            r#"{"order":"one"}"#,
            r#"{"order":"two"}"#,
            r#"{"order":"three"}"#,
        ],
        left_on_the_queue: None,
        settled_results: DRAINED_RESULTS,
    },
    // Criteria naming a field INSIDE the message decide which of the batch is handed over,
    // and the queue is drained of both: a message the criteria rejected is consumed too,
    // because nothing else would take it.
    Row {
        case: "criteria-admit-one-message-of-two",
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[r#"{"data":{"city":["Seattle"]}}"#],
        published: &[r#"{"city":"Dublin"}"#, r#"{"city":"Seattle"}"#],
        delivered: &[r#"{"city":"Seattle"}"#],
        left_on_the_queue: Some(0),
        settled_results: DRAINED_RESULTS,
    },
    // The recorded negative: a mapping naming a queue the broker does not hold is accepted,
    // reports `Enabled` and delivers nothing — while the control on a queue that does exist
    // delivers, so the pipeline is demonstrably live.
    Row {
        case: "a-queue-the-broker-does-not-hold",
        mapped_queue: ABSENT_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[],
        published: &[r#"{"order":"one"}"#],
        delivered: &[],
        left_on_the_queue: Some(1),
        settled_results: &[LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED],
    },
    // Polling paused: "when false, Lambda pauses polling and invocation", so the messages
    // stay on the queue while the control delivers.
    Row {
        case: "polling-paused",
        mapped_queue: SOURCE_QUEUE,
        enabled: false,
        batch_size: 10,
        patterns: &[],
        published: &[r#"{"order":"one"}"#, r#"{"order":"two"}"#],
        delivered: &[],
        left_on_the_queue: Some(2),
        settled_results: &[],
    },
];

/// The readings a mapping that CONSUMED its queue settles on: `OK` for the batch it handed
/// over, and that no record was processed as it keeps polling the queue it drained.
const DRAINED_RESULTS: &[&str] =
    &[LAST_PROCESSING_RESULT_OK, LAST_PROCESSING_RESULT_NO_RECORDS];

/// RULE: a mapping that reports `Enabled` on an Amazon MQ for RabbitMQ broker consumes the one
/// queue its `Queues` names on that broker, hands its function exactly the messages its
/// `FilterCriteria` admit — at most `BatchSize` to a batch, under the one
/// `rmqMessagesByQueue` key the queue and the virtual host make — and acknowledges the whole
/// batch it took; a mapping whose queue the broker does not hold consumes nothing and reports
/// that it could not reach its source, and one whose polling is paused consumes nothing at
/// all.
///
/// One test over the whole table because each row stands up a broker, a function whose package
/// really runs and a mapping, and the rule is one rule: the rows are the inputs it generalises
/// over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mapping_consumes_the_broker_queue_it_names() {
    for row in ROWS {
        let case = row.case;
        let router = router_with_queues();
        let (broker_arn, host, port) = running_rabbitmq_broker(&router, case).await;

        // The queues this row's messages are published to, declared over the broker's own
        // wire: the source queue, and the control's when the row needs one.
        let needs_control = row.delivered.is_empty();
        let mut publisher = Publisher::open(case, &host, port).await;
        publisher.declare_queue(SOURCE_QUEUE).await;
        if needs_control {
            publisher.declare_queue(CONTROL_QUEUE).await;
        }

        // The mapping under test: its own sink queue, its own function and its own mapping.
        let results = sink_queue(&router, case, "esm-results").await;
        let function = "mq-esm-consumer";
        given_consumer(
            &router,
            case,
            function,
            &broker_arn,
            row.mapped_queue,
            row.batch_size,
            &results,
        )
        .await;
        let uuid = given_mapping(&router, case, function, &broker_arn, row).await;
        let settled = if row.enabled {
            STATE_ENABLED
        } else {
            STATE_DISABLED
        };
        await_mapping_state(&router, case, &uuid, settled).await;

        // The control mapping of a row that must deliver nothing, on a queue of the same
        // broker that DOES exist.
        let control = if needs_control {
            let control_results = sink_queue(&router, case, "esm-control-results").await;
            let control_function = "mq-esm-control";
            given_consumer(
                &router,
                case,
                control_function,
                &broker_arn,
                CONTROL_QUEUE,
                1,
                &control_results,
            )
            .await;
            let control_row = Row {
                case,
                mapped_queue: CONTROL_QUEUE,
                enabled: true,
                batch_size: 1,
                patterns: &[],
                published: &[],
                delivered: &[],
                left_on_the_queue: None,
                settled_results: DRAINED_RESULTS,
            };
            let control_uuid =
                given_mapping(&router, case, control_function, &broker_arn, &control_row).await;
            await_mapping_state(&router, case, &control_uuid, STATE_ENABLED).await;
            Some(control_results)
        } else {
            None
        };

        // The row's own messages go first, so a control message published afterwards cannot
        // be delivered before the mapping under test has had a pass over its own source.
        for body in row.published {
            publisher.publish(SOURCE_QUEUE, body).await;
        }

        if let Some(control_results) = &control {
            // TWO control deliveries, each from a publish of its own: the pipeline has made
            // passes since this row's messages were published, so "nothing was delivered" is
            // a reading of this mapping and not of a pipeline that had not started yet.
            for round in 0..2 {
                publisher.publish(CONTROL_QUEUE, CONTROL_BODY).await;
                let seen = delivered_within_budget(&router, case, control_results, 1).await;
                assert_eq!(
                    seen,
                    vec![CONTROL_BODY.to_string()],
                    "case {case:?}: the control mapping on {CONTROL_QUEUE:?} must deliver its \
                     sentinel in round {round}, got {seen:?}"
                );
            }
        }

        // What the mapping delivered: exactly the bodies the rule admits, as a set.
        let mut wanted: Vec<String> = row.delivered.iter().map(|body| (*body).to_string()).collect();
        wanted.sort();
        let mut seen = delivered_within_budget(&router, case, &results, wanted.len()).await;
        seen.sort();
        assert_eq!(
            seen, wanted,
            "case {case:?}: expected the bodies {wanted:?} to be delivered, got {seen:?}"
        );

        // And what the broker's queue still holds, for a row whose mapping consumed none of
        // it or all of it.
        if let Some(expected) = row.left_on_the_queue {
            let held = publisher.queue_depth(SOURCE_QUEUE).await;
            assert_eq!(
                held, expected,
                "case {case:?}: expected {SOURCE_QUEUE:?} to still hold {expected} message(s), \
                 got {held}"
            );
        }

        // What the mapping's own reading of its last pass settled on: a pass is recorded once
        // the invocation it handed the batch to has been decided, which is after the handler's
        // own forward arrived, so the reading is looked for until it is one the rule admits
        // rather than read at an instant of this row's choosing. A mapping no pass was made
        // for reports nothing, which is read only after its control has delivered twice.
        let reported = processing_result_until(&router, &uuid, row.settled_results).await;
        if row.settled_results.is_empty() {
            assert_eq!(
                reported, None,
                "case {case:?}: a mapping that never polled must report no processing result, \
                 got {reported:?}"
            );
        } else {
            assert!(
                reported
                    .as_deref()
                    .is_some_and(|result| row.settled_results.contains(&result)),
                "case {case:?}: expected the mapping to report one of {:?} as its \
                 LastProcessingResult, got {reported:?}",
                row.settled_results
            );
        }

        publisher.close().await;
    }
}

/// The body published once the queue the mapping names has been declared, whose delivery is
/// what says the mapping recovered rather than keeping the reading of its broken source.
const RECOVERED_BODY: &str = r#"{"order":"declared-later"}"#;

/// RULE, read over time: the connect failure a mapping records is a reading of its SOURCE as
/// that source stands, so a queue declared on the broker AFTER the mapping was made replaces
/// that reading by itself — the mapping, still `Enabled` and still polling, then consumes what
/// the queue holds and reports the attempt it made on it.
///
/// This is the other half of the failure reading: a mapping that cannot consume its source
/// reports the connect failure for as long as the fault lasts and no longer, which is what
/// makes the reading a reading of state rather than a verdict the mapping keeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_declared_after_the_mapping_replaces_the_connect_failure() {
    let case = "a queue declared after the mapping was made";
    let router = router_with_queues();
    let (broker_arn, host, port) = running_rabbitmq_broker(&router, case).await;

    let results = sink_queue(&router, case, "esm-recovery-results").await;
    let function = "mq-esm-recovery-consumer";
    given_consumer(&router, case, function, &broker_arn, SOURCE_QUEUE, 10, &results).await;
    let row = Row {
        case,
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[],
        published: &[],
        delivered: &[],
        left_on_the_queue: None,
        settled_results: &[LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED],
    };
    let uuid = given_mapping(&router, case, function, &broker_arn, &row).await;
    await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

    // Nothing has declared the queue yet, so there is nothing on this broker for the mapping to
    // consume and that is what it reports.
    let reported = processing_result_until(&router, &uuid, row.settled_results).await;
    assert_eq!(
        reported.as_deref(),
        Some(LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED),
        "case {case:?}: a mapping naming a queue the broker does not hold must report the \
         connect failure, got {reported:?}"
    );

    // The queue is declared over the broker's own wire, by a client of it, and a message
    // published to it.
    let mut publisher = Publisher::open(case, &host, port).await;
    publisher.declare_queue(SOURCE_QUEUE).await;
    publisher.publish(SOURCE_QUEUE, RECOVERED_BODY).await;

    let seen = delivered_within_budget(&router, case, &results, 1).await;
    assert_eq!(
        seen,
        vec![RECOVERED_BODY.to_string()],
        "case {case:?}: the mapping must deliver what the queue holds once it is declared, got \
         {seen:?}"
    );
    let after = processing_result_until(&router, &uuid, DRAINED_RESULTS).await;
    assert!(
        after
            .as_deref()
            .is_some_and(|result| DRAINED_RESULTS.contains(&result)),
        "case {case:?}: the reading of the broken source must be replaced by the reading of a \
         pass over the queue, expected one of {DRAINED_RESULTS:?}, got {after:?}"
    );

    publisher.close().await;
}

/// RULE: what a BATCH of an Amazon MQ broker is handed over as is the broker's own ENGINE's
/// event, so the reader of a RabbitMQ broker's queue consumes a RabbitMQ broker and no other:
/// a mapping on an ACTIVEMQ broker is left untouched — nothing of it is delivered and no
/// processing attempt is recorded for it, because no pass over it is ever made — while a
/// mapping on a RabbitMQ broker of the same emulator delivers, which is what tells the two
/// apart from a pipeline that consumes nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activemq_brokers_mapping_is_not_consumed_by_the_rabbitmq_reader() {
    let case = "a mapping on an ActiveMQ broker beside one on a RabbitMQ broker";
    let router = router_with_queues();
    let (rabbit_arn, host, port) = running_rabbitmq_broker(&router, case).await;
    // The other engine Amazon MQ serves, created through that service's own `CreateBroker`:
    // this emulator opens a data plane for it too, which is exactly what makes the engine the
    // question this case asserts.
    let (activemq_arn, _described) = running_broker(
        &router,
        case,
        "esm-activemq-broker",
        &json!({ "engineType": "ACTIVEMQ", "hostInstanceType": "mq.t3.micro" }),
    )
    .await;

    // The mapping under test, on the ActiveMQ broker, naming the same queue the RabbitMQ
    // control consumes — so only the broker's engine distinguishes the two.
    let results = sink_queue(&router, case, "esm-activemq-results").await;
    let function = "mq-esm-activemq-consumer";
    given_consumer(&router, case, function, &activemq_arn, SOURCE_QUEUE, 10, &results).await;
    let row = Row {
        case,
        mapped_queue: SOURCE_QUEUE,
        enabled: true,
        batch_size: 10,
        patterns: &[],
        published: &[],
        delivered: &[],
        left_on_the_queue: None,
        settled_results: &[],
    };
    let uuid = given_mapping(&router, case, function, &activemq_arn, &row).await;
    await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;

    // The control, on the RabbitMQ broker: its own queue, function, sink and mapping.
    let mut publisher = Publisher::open(case, &host, port).await;
    publisher.declare_queue(CONTROL_QUEUE).await;
    let control_results = sink_queue(&router, case, "esm-control-results").await;
    let control_function = "mq-esm-control";
    given_consumer(
        &router,
        case,
        control_function,
        &rabbit_arn,
        CONTROL_QUEUE,
        1,
        &control_results,
    )
    .await;
    let control_row = Row {
        case,
        mapped_queue: CONTROL_QUEUE,
        enabled: true,
        batch_size: 1,
        patterns: &[],
        published: &[],
        delivered: &[],
        left_on_the_queue: None,
        settled_results: DRAINED_RESULTS,
    };
    let control_uuid =
        given_mapping(&router, case, control_function, &rabbit_arn, &control_row).await;
    await_mapping_state(&router, case, &control_uuid, STATE_ENABLED).await;

    // TWO control deliveries, each from a publish of its own, so what is read about the
    // ActiveMQ mapping below is read after the pipeline has demonstrably made passes.
    for round in 0..2 {
        publisher.publish(CONTROL_QUEUE, CONTROL_BODY).await;
        let seen = delivered_within_budget(&router, case, &control_results, 1).await;
        assert_eq!(
            seen,
            vec![CONTROL_BODY.to_string()],
            "case {case:?}: the control mapping on the RabbitMQ broker must deliver its \
             sentinel in round {round}, got {seen:?}"
        );
    }

    let delivered = delivered_within_budget(&router, case, &results, 0).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: the mapping on the ActiveMQ broker must deliver nothing, got \
         {delivered:?}"
    );
    let reported = get_mapping(&router, &uuid).await.string("LastProcessingResult");
    assert_eq!(
        reported, None,
        "case {case:?}: the mapping on the ActiveMQ broker must report no processing attempt, \
         because no pass over it is made at all, got {reported:?}"
    );
    let state = get_mapping(&router, &uuid).await.string("State");
    assert_eq!(
        state.as_deref(),
        Some(STATE_ENABLED),
        "case {case:?}: the mapping is still the one the create accepted, got {state:?}"
    );

    publisher.close().await;
}

/// A sink queue named off `label`, whose url is what a handler forwards to.
async fn sink_queue(router: &MultiServiceRouter, case: &str, label: &str) -> String {
    let (url, _arn) = create_queue(router, case, label, &json!({})).await;
    url
}

/// Create the function a row's mapping invokes: [`FORWARDING_HANDLER`] with the envelope it
/// asserts bound to this row's broker, queue and batch size, and wait until it is `Active`.
async fn given_consumer(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    broker_arn: &str,
    queue_name: &str,
    batch_size: i64,
    results_url: &str,
) {
    create_running_function(
        router,
        case,
        name,
        FORWARDING_HANDLER,
        FUNCTION_TIMEOUT,
        &json!({
            RESULTS_QUEUE_URL: results_url,
            "BROKER_ARN": broker_arn,
            "QUEUE_KEY": format!("{queue_name}::{VIRTUAL_HOST}"),
            "MAX_RECORDS": batch_size.to_string(),
            "CONTENT_TYPE": CONTENT_TYPE,
        }),
    )
    .await;
}

/// Create the mapping `row` describes on `broker_arn` and answer its `UUID`.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    broker_arn: &str,
    row: &Row,
) -> String {
    let mut extras = json!({
        "Queues": [row.mapped_queue],
        "BatchSize": row.batch_size,
        "Enabled": row.enabled,
    });
    if !row.patterns.is_empty() {
        extras["FilterCriteria"] = json!({
            "Filters": row
                .patterns
                .iter()
                .map(|pattern| json!({ "Pattern": pattern }))
                .collect::<Vec<Value>>(),
        });
    }
    let created = create_mapping(
        router,
        &create_request(function, Some(broker_arn), &extras),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
        created.status,
        created.text()
    );
    created.string("UUID").unwrap_or_else(|| {
        panic!(
            "case {case:?}: CreateEventSourceMapping must answer a UUID, got {}",
            created.text()
        )
    })
}

/// Look at the sink queue `url` until it has handed over `wanted` bodies or [`BUDGET`] has
/// passed, and answer every body it handed over.
///
/// Waits for the CONDITION, never for a fixed span: a row that wants nothing delivered looks
/// once and reports what is there, which is read only after its control has delivered.
async fn delivered_within_budget(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<String> {
    let deadline = Instant::now() + BUDGET;
    let mut bodies: Vec<String> = Vec::new();
    loop {
        bodies.extend(taken_bodies(router, case, url).await);
        if bodies.len() >= wanted || Instant::now() >= deadline {
            return bodies;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Look at the mapping `uuid` names until its `LastProcessingResult` is one of `wanted`,
/// answering what it reported last. `wanted` holding nothing answers the one reading there is.
///
/// Waits for the CONDITION, never for a fixed span: recording what a pass came to happens
/// after the invocation that pass made has been decided, which is a span no caller controls.
async fn processing_result_until(
    router: &MultiServiceRouter,
    uuid: &str,
    wanted: &[&str],
) -> Option<String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reported = get_mapping(router, uuid).await.string("LastProcessingResult");
        let settled = reported
            .as_deref()
            .is_some_and(|result| wanted.contains(&result));
        if settled || wanted.is_empty() || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create one RabbitMQ broker with one user through Amazon MQ's own `CreateBroker`, wait until
/// it reports `RUNNING`, and answer `(brokerArn, host, port)` for the `amqps://` endpoint it
/// publishes.
async fn running_rabbitmq_broker(
    router: &MultiServiceRouter,
    case: &str,
) -> (String, String, u16) {
    let (broker_arn, described) = running_broker(
        router,
        case,
        "esm-broker",
        &json!({
            "engineType": "RABBITMQ",
            "engineVersion": "3.13",
            "hostInstanceType": "mq.m5.large",
        }),
    )
    .await;
    let endpoints: Vec<String> = described["brokerInstances"][0]["endpoints"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let amqps = endpoints
        .iter()
        .find(|endpoint| endpoint.starts_with("amqps://"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a RUNNING RabbitMQ broker must publish an amqps:// endpoint, got \
                 {endpoints:?}"
            )
        });
    let authority = amqps.trim_start_matches("amqps://");
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected an amqps endpoint naming a port, got {authority:?}")
    });
    let port: u16 = port.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: expected an amqps endpoint naming a port, got {authority:?}")
    });
    (broker_arn, host.to_string(), port)
}

/// Create the broker `name` with one user and whatever `engine` states of its engine, wait
/// until Amazon MQ reports it `RUNNING`, and answer its `brokerArn` and the whole reading of
/// it.
async fn running_broker(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    engine: &Value,
) -> (String, Value) {
    let mut body = json!({
        "brokerName": name,
        "deploymentMode": "SINGLE_INSTANCE",
        "publiclyAccessible": false,
        "users": [{ "username": BROKER_USER, "password": BROKER_PASSWORD }],
    });
    for (member, value) in engine.as_object().expect("the engine is stated as members") {
        body[member.clone()] = value.clone();
    }
    let created = mq_call(router, "POST", "/v1/brokers", Some(&body)).await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: expected CreateBroker {name:?} to answer 200, got {} {}",
        created.status,
        created.text()
    );
    let broker_arn = created
        .string("brokerArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateBroker answered no brokerArn"));
    let broker_id = created
        .string("brokerId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateBroker answered no brokerId"));
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = mq_call(router, "GET", &format!("/v1/brokers/{broker_id}"), None).await;
        let read = described.json();
        if read["brokerState"].as_str() == Some("RUNNING") {
            return (broker_arn, read);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the broker {name:?} never reached RUNNING, last answer {}",
            described.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One AMQP 0-9-1 session on a broker's own socket: the publisher every row puts its messages
/// on the broker through, and the reader of how many a queue still holds.
///
/// The frames are built and parsed with the same `amq-protocol` codec the broker's wire is
/// served with, which is what a client library does: nothing here reaches into any service's
/// state.
struct Publisher {
    case: String,
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    buffered: Vec<u8>,
}

impl Publisher {
    /// Open a TLS connection to the broker's published address — an `amqps://` endpoint is TLS
    /// from the first byte — run the AMQP handshake on it as the broker's own user, and open a
    /// channel.
    ///
    /// The address dialled is the one the control plane published for the broker, which the
    /// core binds on the loopback interface by default: that is asserted before anything is
    /// dialled, and the dial itself names the loopback literal, so this client can only ever
    /// reach a port THIS process opened.
    async fn open(case: &str, host: &str, port: u16) -> Self {
        assert_eq!(
            host, LOOPBACK,
            "case {case:?}: the published endpoint must be this machine's loopback address, got \
             {host:?}"
        );
        let tcp = TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {host}:{port} must accept a connection, got {error}")
            });
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("the provider supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptTheEmulatorsCertificate(provider)))
            .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let server_name =
            rustls::pki_types::ServerName::try_from("localhost").expect("localhost is a name");
        let tls = connector
            .connect(server_name, tcp)
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the broker's socket must complete a TLS handshake, got {error}")
            });
        let mut session = Self {
            case: case.to_string(),
            stream: tls,
            buffered: Vec::new(),
        };
        session.handshake().await;
        session
    }

    /// The client half of the AMQP 0-9-1 handshake, plus the one channel this session uses.
    async fn handshake(&mut self) {
        self.write_bytes(&[b'A', b'M', b'Q', b'P', 0, 0, 9, 1]).await;
        match self.read_frame().await {
            AMQPFrame::Method(_, AMQPClass::Connection(connection::AMQPMethod::Start(_))) => {}
            other => panic!(
                "case {:?}: expected connection.start, got {other:?}",
                self.case
            ),
        }
        let mut response = Vec::new();
        response.push(0u8);
        response.extend_from_slice(BROKER_USER.as_bytes());
        response.push(0u8);
        response.extend_from_slice(BROKER_PASSWORD.as_bytes());
        self.send(AMQPFrame::Method(
            0,
            AMQPClass::Connection(connection::AMQPMethod::StartOk(connection::StartOk {
                client_properties: FieldTable::default(),
                mechanism: ShortString::from("PLAIN"),
                response: LongString::from(response),
                locale: ShortString::from("en_US"),
            })),
        ))
        .await;
        match self.read_frame().await {
            AMQPFrame::Method(_, AMQPClass::Connection(connection::AMQPMethod::Tune(_))) => {}
            other => panic!("case {:?}: expected connection.tune, got {other:?}", self.case),
        }
        self.send(AMQPFrame::Method(
            0,
            AMQPClass::Connection(connection::AMQPMethod::TuneOk(connection::TuneOk {
                channel_max: 0,
                frame_max: u32::try_from(FRAME_MAX).unwrap_or(u32::MAX),
                // Heartbeats off: a session here is open for a moment, and nothing is
                // asserted against the clock.
                heartbeat: 0,
            })),
        ))
        .await;
        self.send(AMQPFrame::Method(
            0,
            AMQPClass::Connection(connection::AMQPMethod::Open(connection::Open {
                virtual_host: ShortString::from(VIRTUAL_HOST),
            })),
        ))
        .await;
        match self.read_frame().await {
            AMQPFrame::Method(_, AMQPClass::Connection(connection::AMQPMethod::OpenOk(_))) => {}
            other => panic!(
                "case {:?}: expected connection.open-ok, got {other:?}",
                self.case
            ),
        }
        self.send(AMQPFrame::Method(
            1,
            AMQPClass::Channel(channel::AMQPMethod::Open(channel::Open::default())),
        ))
        .await;
        match self.read_frame().await {
            AMQPFrame::Method(1, AMQPClass::Channel(channel::AMQPMethod::OpenOk(_))) => {}
            other => panic!(
                "case {:?}: expected channel.open-ok, got {other:?}",
                self.case
            ),
        }
    }

    /// Declare a durable queue, as a client that wants it to survive a restart does.
    async fn declare_queue(&mut self, name: &str) {
        let answer = self.declare(name, false).await;
        assert!(
            matches!(
                answer,
                AMQPFrame::Method(_, AMQPClass::Queue(queue::AMQPMethod::DeclareOk(_)))
            ),
            "case {:?}: expected queue.declare-ok for {name:?}, got {answer:?}",
            self.case
        );
    }

    /// How many messages the queue `name` holds right now, as a passive declaration reports
    /// it — which is the one reading of a queue's depth a client has.
    async fn queue_depth(&mut self, name: &str) -> u32 {
        match self.declare(name, true).await {
            AMQPFrame::Method(_, AMQPClass::Queue(queue::AMQPMethod::DeclareOk(ok))) => {
                ok.message_count
            }
            other => panic!(
                "case {:?}: expected a passive queue.declare-ok for {name:?}, got {other:?}",
                self.case
            ),
        }
    }

    /// One `queue.declare`, passive or not, and the frame it is answered with.
    async fn declare(&mut self, name: &str, passive: bool) -> AMQPFrame {
        self.send(AMQPFrame::Method(
            1,
            AMQPClass::Queue(queue::AMQPMethod::Declare(queue::Declare {
                queue: ShortString::from(name),
                passive,
                durable: true,
                exclusive: false,
                auto_delete: false,
                nowait: false,
                arguments: FieldTable::default(),
            })),
        ))
        .await;
        self.read_frame().await
    }

    /// Publish `body` to the queue `queue_name` names, through the default exchange, which
    /// routes a message to the queue its routing key names.
    async fn publish(&mut self, queue_name: &str, body: &str) {
        let bytes = body.as_bytes();
        self.send(AMQPFrame::Method(
            1,
            AMQPClass::Basic(basic::AMQPMethod::Publish(basic::Publish {
                exchange: ShortString::from(""),
                routing_key: ShortString::from(queue_name),
                mandatory: false,
                immediate: false,
            })),
        ))
        .await;
        self.send(AMQPFrame::Header(
            1,
            AMQPContentHeader {
                class_id: 60,
                body_size: bytes.len() as u64,
                properties: AMQPProperties::default()
                    .with_content_type(ShortString::from(CONTENT_TYPE)),
            },
        ))
        .await;
        for chunk in bytes.chunks(FRAME_MAX - 8) {
            self.send(AMQPFrame::Body(1, chunk.to_vec())).await;
        }
    }

    /// Send one frame.
    async fn send(&mut self, frame: AMQPFrame) {
        let (bytes, _written) = gen_frame(&frame)(Vec::new().into())
            .unwrap_or_else(|error| {
                panic!("case {:?}: {frame:?} must serialize, got {error}", self.case)
            })
            .into_inner();
        self.write_bytes(&bytes).await;
    }

    async fn write_bytes(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap_or_else(|error| {
            panic!(
                "case {:?}: the session must be writable, got {error}",
                self.case
            )
        });
        self.stream.flush().await.unwrap_or_else(|error| {
            panic!("case {:?}: the session must flush, got {error}", self.case)
        });
    }

    /// The next whole frame off the session, skipping heartbeats as a client does.
    async fn read_frame(&mut self) -> AMQPFrame {
        let deadline = Instant::now() + FRAME_WAIT;
        loop {
            let parsed = match parse_frame(self.buffered.as_slice()) {
                Ok((rest, frame)) => Some((self.buffered.len() - rest.len(), frame)),
                Err(error) => {
                    assert!(
                        error.is_incomplete(),
                        "case {:?}: the broker must answer frames, got {error:?}",
                        self.case
                    );
                    None
                }
            };
            if let Some((consumed, frame)) = parsed {
                self.buffered.drain(..consumed);
                if matches!(frame, AMQPFrame::Heartbeat) {
                    continue;
                }
                return frame;
            }
            let mut chunk = vec![0u8; 16 * 1024];
            let read = tokio::time::timeout_at(deadline, self.stream.read(&mut chunk))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "case {:?}: the broker must answer the frame it owes",
                        self.case
                    )
                })
                .unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the session must be readable, got {error}",
                        self.case
                    )
                });
            assert!(
                read > 0,
                "case {:?}: the broker closed the session with a frame still owed",
                self.case
            );
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    /// Close the connection the way a client does.
    async fn close(mut self) {
        self.send(AMQPFrame::Method(
            0,
            AMQPClass::Connection(connection::AMQPMethod::Close(connection::Close {
                reply_code: 200,
                reply_text: ShortString::from("Normal shutdown"),
                class_id: 0,
                method_id: 0,
            })),
        ))
        .await;
        let _ = self.read_frame().await;
    }
}

/// The verifier a client of the emulator's OWN socket uses: the certificate is the one the
/// core generated for its listeners, self-signed and naming loopback, and this client is the
/// emulator's own test connecting to a port the same process opened — so there is no authority
/// to check it against.
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
