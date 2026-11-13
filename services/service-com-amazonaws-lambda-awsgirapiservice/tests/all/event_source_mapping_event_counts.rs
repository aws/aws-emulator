// SPDX-License-Identifier: CC0-1.0

//! The per-mapping EVENT-COUNT metrics of an event source mapping on an Amazon SQS queue,
//! end to end over the wire with Amazon SQS and CloudWatch mounted beside Lambda on ONE
//! engine — so the messages are sent where a client sends them and the series are read back
//! where a client reads them (`GetMetricStatistics` for one series and its datapoints,
//! `GetMetricData` for several series in one answer).
//!
//! The RULE asserted over the tables below: while an event source mapping's `MetricsConfig`
//! selects `EventCount` ("the metrics you want your event source mapping to produce […]
//! `EventCount` to receive metrics related to the number of events processed by your event
//! source mapping"), every pass of that mapping's poller publishes what it did with the
//! events it took out of the source — how many it polled, how many its `FilterCriteria`
//! rejected, how many an invocation of its function accepted and how many it deleted from
//! the source — into the `AWS/Lambda` namespace, each series dimensioned on the mapping's
//! own `UUID` and carrying the unit `Count`, with a 0-valued poll count for a minute in
//! which the mapping polled nothing. A mapping whose `MetricsConfig` does not select
//! `EventCount` — one carrying none and one whose `Metrics` list is empty — publishes none
//! of them, however much its poller is doing, including when it is the TWIN of an opted-in
//! mapping doing exactly the same filtering work beside it.
//!
//! Two invariants hold in every row, whatever the poller came to, and are asserted as such
//! rather than against one run's numbers: an event is deleted from the source only because
//! the filter rejected it or because an invocation accepted it (`Deleted = FilteredOut +
//! Invoked`), and no more events are invoked than the filter admitted (`Invoked ≤ Polled −
//! FilteredOut`).
//!
//! The rows differ along what the rule generalises over: how many messages are waiting,
//! whether a mapping filters at all, whether its pattern names fields inside a JSON body or
//! matches a plain-text one, how many of the waiting messages the filter admits, the
//! `BatchSize` they are polled in, and which `EventSourceMappingMetric` values the
//! `MetricsConfig` selects.
//!
//! The counting table makes the poll passes of its own mapping
//! ([`crate::event_source_mappings::DrivenPolling`]) and awaits each of them, so the counts it
//! reads are the ones those passes published; the other tables wait for CONDITIONS and never
//! for a span: every barrier of a row — a count published, a queue drained, a mapping removed
//! — is looked for again until it holds or the generous ceiling [`BUDGET`] passes, nothing
//! asserts on how long it took, the four counts the invariants relate are read as ONE answer
//! ([`family_sums`]), and the row retires its mapping once it is read ([`retire_mapping`]) —
//! so one wake-up of the single poll loop serving the engine does the work of the row that is
//! running and not also the work of every mapping an earlier row left polling.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::metrics::{
    COUNT_UNIT, DELETED_EVENT_COUNT_METRIC, EVENT_SOURCE_MAPPING_UUID_DIMENSION,
    FILTERED_OUT_EVENT_COUNT_METRIC, INVOKED_EVENT_COUNT_METRIC, METRIC_NAMESPACE,
    POLLED_EVENT_COUNT_METRIC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, call_sqs, create_mapping, create_queue, create_timed_function, get_mapping,
    queue_attribute, queue_held, router_on, DrivenPolling, IN_FLIGHT_MESSAGES, MAPPINGS_URI,
};

/// The region every call below signs for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The `State` a mapping reports once it is polling its event source.
const STATE_ENABLED: &str = "Enabled";

/// How long a wait for work the mapping has to do — a batch polled, a queue drained, a
/// datapoint appearing, a retired mapping removed — is given before the row is a failure.
///
/// A CEILING on a wait, not a span anything here waits out: every wait below looks again
/// until its condition holds and returns the instant it does, and no row asserts on how long
/// that took. The work behind a condition is a handful of the poll loop's wake-ups — one
/// mapping polls at a time, since a row retires its own ([`retire_mapping`]) — so the ceiling
/// is an order of magnitude above what that costs, far above the poll interval and the
/// state-settle span with whatever a loaded host adds to them, and still near enough that a
/// row which really does no work reports it rather than holding the suite up.
const BUDGET: Duration = Duration::from_secs(60);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How far either side of now the series below are read, in seconds: a window wide enough to
/// hold every datapoint a row's passes publish, at the one-minute resolution they are stored
/// at.
const WINDOW_SECONDS: f64 = 600.0;

/// Several of the poller's wake-ups: long enough that a pass which was going to publish
/// anything has published it.
const SEVERAL_PASSES: Duration = Duration::from_secs(2);

/// How long the queues below hide a message they hand over for, in seconds. Above the
/// mapped function's own `Timeout` (which is what makes the mapping acceptable at all) and
/// short enough that a message no consumer deleted comes back inside a row's budget.
const VISIBILITY_TIMEOUT: i64 = 2;

/// The mapped function's `Timeout`, in seconds.
const FUNCTION_TIMEOUT: i64 = 1;

/// The four metrics of the event-count family, which is the whole of what `EventCount`
/// selects.
const EVENT_COUNT_FAMILY: [&str; 4] = [
    POLLED_EVENT_COUNT_METRIC,
    FILTERED_OUT_EVENT_COUNT_METRIC,
    INVOKED_EVENT_COUNT_METRIC,
    DELETED_EVENT_COUNT_METRIC,
];

/// Lambda, Amazon SQS and CloudWatch on ONE engine: the mapping's event source is a queue
/// and the series it publishes are CloudWatch's, so both are reached in-process — the shape
/// `core/router/src/lib.rs` mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// The same three services, with the poll passes of the engine's mappings the suite's own
/// ([`DrivenPolling`]): the counting table makes each pass itself and awaits it, so the
/// counts it reads are the ones the passes it made published rather than whatever the
/// background loop had got to by the instant the table looked.
fn driven_emulator() -> (MultiServiceRouter, DrivenPolling) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    let driven = DrivenPolling::claiming(&engine, &router);
    (router, driven)
}

/// One CloudWatch call, the way a client makes it.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Now, as epoch seconds: the instant a read's window is centred on.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The dimensions the series of the mapping `uuid` names carry, as a client states them.
fn mapping_dimensions(uuid: &str) -> Value {
    json!([{ "Name": EVENT_SOURCE_MAPPING_UUID_DIMENSION, "Value": uuid }])
}

/// The datapoints CloudWatch holds for one metric of the mapping `uuid` names, as a client
/// reads them: one per minute, over a window wide enough to hold the whole row.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    uuid: &str,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": mapping_dimensions(uuid),
            "StartTime": now - WINDOW_SECONDS,
            "EndTime": now + WINDOW_SECONDS,
            "Period": 60,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics for {metric} answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// What one metric of the mapping `uuid` names sums to over the whole window.
async fn metric_sum(router: &MultiServiceRouter, case: &str, metric: &str, uuid: &str) -> f64 {
    datapoints(router, case, metric, uuid)
        .await
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Every metric of the family, summed, in the order [`EVENT_COUNT_FAMILY`] names them — read
/// as ONE answer.
///
/// The relations asserted between the four counts hold of what one pass did, so the four
/// series are read in a single call (`GetMetricData` "can retrieve as many as 500 different
/// metrics in a single request") rather than one after another: a family assembled from four
/// answers taken at four instants, while the mapping's poller is still polling its queue,
/// belongs to no one instant, so whether it satisfied those relations would depend on how
/// much the poller got done between the reads.
async fn family_sums(router: &MultiServiceRouter, case: &str, uuid: &str) -> Vec<f64> {
    let now = now_secs();
    let queries: Vec<Value> = EVENT_COUNT_FAMILY
        .iter()
        .enumerate()
        .map(|(index, metric)| {
            json!({
                "Id": query_id(index),
                "MetricStat": {
                    "Metric": {
                        "Namespace": METRIC_NAMESPACE,
                        "MetricName": metric,
                        "Dimensions": mapping_dimensions(uuid),
                    },
                    "Period": 60,
                    "Stat": "Sum",
                },
            })
        })
        .collect();
    let (status, data) = cloudwatch(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": now - WINDOW_SECONDS,
            "EndTime": now + WINDOW_SECONDS,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricData for the event-count family answered {status} {data}"
    );
    EVENT_COUNT_FAMILY
        .iter()
        .enumerate()
        .map(|(index, metric)| {
            let id = query_id(index);
            let result = data["MetricDataResults"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|result| result["Id"] == json!(id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: GetMetricData must answer the {metric} query, got {data}")
                });
            result["Values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_f64())
                .sum()
        })
        .collect()
}

/// The `Id` the `index`th metric of the family is queried under, as `MetricDataQuery$Id`
/// spells one: "must start with a lowercase letter".
fn query_id(index: usize) -> String {
    format!("m{index}")
}

/// Wait until one metric of the mapping `uuid` names sums to at least `wanted`, answering
/// what it summed to.
async fn wait_for_sum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    uuid: &str,
    wanted: f64,
) -> f64 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let summed = metric_sum(router, case, metric, uuid).await;
        if summed >= wanted || Instant::now() >= deadline {
            return summed;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until one metric of the mapping `uuid` names has a datapoint, answering them.
async fn wait_for_datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    uuid: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let points = datapoints(router, case, metric, uuid).await;
        if !points.is_empty() || Instant::now() >= deadline {
            return points;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the queue `url` addresses holds exactly `wanted` messages, answering what it
/// held.
///
/// What it holds is read from ONE `GetQueueAttributes` answer ([`queue_held`]): the poller
/// moves a message between the visible, in-flight and delayed counts as it delivers it and
/// as its visibility lapses, so a total assembled from counts read in separate answers
/// belongs to no one instant.
async fn wait_for_held(router: &MultiServiceRouter, case: &str, url: &str, wanted: u32) -> u32 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let holding = queue_held(router, case, url).await;
        if holding == wanted || Instant::now() >= deadline {
            return holding;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the queue `url` addresses reports `wanted` messages in flight, answering
/// what it reported.
async fn wait_for_in_flight(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: &str,
) -> Option<String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reported = queue_attribute(router, case, url, IN_FLIGHT_MESSAGES).await;
        if reported.as_deref() == Some(wanted) || Instant::now() >= deadline {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `SendMessage` of `body` to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// Wait until the mapping `uuid` names reports `Enabled`.
async fn wait_for_enabled(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        if reading.string("State").as_deref() == Some(STATE_ENABLED) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping never reported {STATE_ENABLED}: {}",
            reading.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Make poll passes until the series `metric` of the mapping `uuid` names sums to at least
/// `wanted`, answering what it sums to.
///
/// Each pass is awaited and publishes what it did before it answers, so the loop turns once
/// per pass rather than once per span of wall clock, and [`BUDGET`] is a backstop against a
/// mapping that polls nothing at all.
async fn poll_until_sum(
    router: &MultiServiceRouter,
    driven: &mut DrivenPolling,
    case: &str,
    metric: &str,
    uuid: &str,
    wanted: f64,
) -> f64 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let summed = metric_sum(router, case, metric, uuid).await;
        if summed >= wanted || Instant::now() >= deadline {
            return summed;
        }
        driven.poll_once().await;
    }
}

/// Make poll passes until the queue `url` addresses holds `wanted` messages, answering what
/// it holds.
///
/// Each pass is awaited, so the loop turns once per pass the mapping makes rather than once
/// per span of wall clock, and [`BUDGET`] is a backstop against a mapping that polls nothing
/// at all. A message whose hold lapsed between two passes is polled again by the next one,
/// which is the poller's own answer to it.
async fn poll_until_held(
    router: &MultiServiceRouter,
    driven: &mut DrivenPolling,
    case: &str,
    url: &str,
    wanted: u32,
) -> u32 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let holding = queue_held(router, case, url).await;
        if holding == wanted || Instant::now() >= deadline {
            return holding;
        }
        driven.poll_once().await;
    }
}

/// A queue and a function for one row, and the mapping between them: its UUID and the
/// queue's url.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    extras: &Value,
) -> (String, String) {
    let (url, arn) = create_queue(
        router,
        case,
        &format!("{name}-queue"),
        &json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT.to_string() }),
    )
    .await;
    let function = format!("{name}-fn");
    create_timed_function(router, case, &function, FUNCTION_TIMEOUT).await;
    let mut body = json!({ "FunctionName": function, "EventSourceArn": arn });
    for (member, value) in extras.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    let created = create_mapping(router, &body).await;
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
    wait_for_enabled(router, case, &uuid).await;
    (uuid, url)
}

/// Retire the mapping `uuid` names when a row is done with it: `DeleteEventSourceMapping`,
/// and a wait until the clock has removed it.
///
/// A mapping left `Enabled` goes on polling its queue and invoking its function for the whole
/// of the test. A row that did not retire its mapping would leave every later row's poll
/// loop doing its work as well — one wake-up's work, and with it how long a later row's
/// conditions take to hold, would grow with how many rows had already run, which is exactly
/// the dependence on how much the host has to spare that a row must not have.
async fn retire_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let deleted = call(
        router,
        "DELETE",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        202,
        "case {case:?}: expected DeleteEventSourceMapping to answer 202, got {} {}",
        deleted.status,
        deleted.text()
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        if reading.status == 404 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the retired mapping was never removed, GetEventSourceMapping \
             still answers {} {}",
            reading.status,
            reading.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the counting table: what the mapping filters with, and what is waiting on its
/// queue.
struct CountRow {
    case: &'static str,
    /// The `Pattern` of each `FilterCriteria` filter; empty for a mapping that filters
    /// nothing.
    patterns: Vec<&'static str>,
    /// Each message body to send, beside whether the row's patterns admit it.
    bodies: Vec<(&'static str, bool)>,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from
    /// your stream or queue".
    batch_size: i64,
}

/// The catalog: no filter at all, a pattern naming fields inside a JSON body, a pattern
/// matching a plain-text body, and a pattern no waiting message matches — over one message
/// and several, polled one per batch and ten at a time.
fn count_rows() -> Vec<CountRow> {
    vec![
        CountRow {
            case: "no filter criteria, three messages",
            patterns: Vec::new(),
            bodies: vec![
                (r#"{"kind":"keep"}"#, true),
                ("plain text", true),
                (r#"{"kind":"drop"}"#, true),
            ],
            batch_size: 10,
        },
        CountRow {
            case: "a json body pattern admitting one of four",
            patterns: vec![r#"{"body":{"kind":["keep"]}}"#],
            bodies: vec![
                (r#"{"kind":"keep"}"#, true),
                (r#"{"kind":"drop"}"#, false),
                (r#"{"kind":"other"}"#, false),
                ("not json at all", false),
            ],
            batch_size: 10,
        },
        CountRow {
            case: "a plain-text body pattern, one message per batch",
            patterns: vec![r#"{"body":["keep"]}"#],
            bodies: vec![("keep", true), ("drop", false)],
            batch_size: 1,
        },
        CountRow {
            case: "two patterns, every waiting message rejected",
            patterns: vec![r#"{"body":{"kind":["keep"]}}"#, r#"{"body":["keep"]}"#],
            bodies: vec![(r#"{"kind":"drop"}"#, false), ("drop", false)],
            batch_size: 10,
        },
    ]
}

/// RULE: while a mapping's `MetricsConfig` selects `EventCount`, each pass of its poller
/// publishes per mapping what it did with the events it polled — the events taken out of the
/// queue, the events its `FilterCriteria` rejected, the events an invocation accepted and
/// the events deleted from the queue — and an event is deleted only because the filter
/// rejected it or because an invocation accepted it.
#[tokio::test]
async fn the_event_count_family_reports_what_each_poll_did() {
    let (router, mut driven) = driven_emulator();
    for (index, row) in count_rows().iter().enumerate() {
        let case = row.case;
        let filters: Vec<Value> = row
            .patterns
            .iter()
            .map(|pattern| json!({ "Pattern": pattern }))
            .collect();
        let mut extras = json!({
            "BatchSize": row.batch_size,
            "MetricsConfig": { "Metrics": ["EventCount"] },
        });
        if !filters.is_empty() {
            extras["FilterCriteria"] = json!({ "Filters": filters });
        }
        let (uuid, url) =
            given_mapping(&router, case, &format!("event-counts-{index}"), &extras).await;
        for (body, _) in &row.bodies {
            send_message(&router, case, &url, body).await;
        }

        let sent = u32::try_from(row.bodies.len()).unwrap_or(u32::MAX);
        let admitted = u32::try_from(row.bodies.iter().filter(|(_, keep)| *keep).count())
            .unwrap_or(u32::MAX);
        let rejected = sent - admitted;
        // The first barrier: passes are made until every message that was sent has been
        // polled, each pass publishing what it did before it answers.
        let polled = poll_until_sum(
            &router,
            &mut driven,
            case,
            POLLED_EVENT_COUNT_METRIC,
            &uuid,
            f64::from(sent),
        )
        .await;
        assert!(
            polled >= f64::from(sent),
            "case {case:?}: every one of the {sent} messages sent must be counted as polled, \
             got a {POLLED_EVENT_COUNT_METRIC} of {polled}"
        );
        // And until the filter has decided every one of them: a message the filter rejected
        // leaves the queue — nothing else would — so the queue is left holding exactly the
        // messages the filter admitted.
        let holding = poll_until_held(&router, &mut driven, case, &url, admitted).await;
        assert_eq!(
            holding, admitted,
            "case {case:?}: the queue must be left holding only the {admitted} messages the \
             filter admits, got {holding}"
        );

        // Every series of the family is published, a zero included: a client cannot tell a
        // poller that filtered nothing out from one that is not publishing otherwise.
        for metric in EVENT_COUNT_FAMILY {
            let points = datapoints(&router, case, metric, &uuid).await;
            assert!(
                !points.is_empty(),
                "case {case:?}: {metric} must be published for this mapping, got no datapoints"
            );
            for point in &points {
                assert_eq!(
                    point["Unit"],
                    json!(COUNT_UNIT),
                    "case {case:?}: a {metric} datapoint must carry the unit {COUNT_UNIT}, \
                     got {point}"
                );
            }
        }

        let sums = family_sums(&router, case, &uuid).await;
        let (polled, filtered_out, invoked, deleted) = (sums[0], sums[1], sums[2], sums[3]);
        // The reading the relations below are taken from carries the same poll count the
        // row's first barrier was met on, so none of them can be satisfied by a family of
        // zeros.
        assert!(
            polled >= f64::from(sent),
            "case {case:?}: the family must count every one of the {sent} messages sent as \
             polled, got {sums:?}"
        );
        // A rejected message is deleted, so it is never polled again: the count of rejected
        // events is exactly what the row sent and the filter did not admit.
        assert_eq!(
            filtered_out,
            f64::from(rejected),
            "case {case:?}: the filter rejects {rejected} of the {sent} messages, got a \
             {FILTERED_OUT_EVENT_COUNT_METRIC} of {filtered_out}"
        );
        // The invariants, which hold whatever the invocation came to.
        assert_eq!(
            deleted,
            filtered_out + invoked,
            "case {case:?}: an event is deleted only because the filter rejected it or an \
             invocation accepted it, got {deleted} deleted against {filtered_out} filtered \
             out and {invoked} invoked"
        );
        assert!(
            invoked <= polled - filtered_out,
            "case {case:?}: no more events can be invoked than the filter admitted, got \
             {invoked} invoked against {polled} polled and {filtered_out} filtered out"
        );
        // The row is read: its mapping is retired, so the next row's poller polls its queue
        // and no other.
        retire_mapping(&router, case, &uuid).await;
    }
}

/// One row of the selection table: what `MetricsConfig` the mapping is created with, and
/// whether that selects the event-count family.
struct SelectionRow {
    case: &'static str,
    /// The `MetricsConfig` member as the request states it, or `Value::Null` for a request
    /// that states none.
    metrics_config: Value,
    /// Whether the family is published for such a mapping.
    published: bool,
}

/// The catalog of `MetricsConfig` values an Amazon SQS mapping can be created with, over the
/// `EventSourceMappingMetric` enum the model declares (`EventCount`, `ErrorCount`,
/// `KafkaMetrics` — `smithy_query(query="shape", shape="EventSourceMappingMetric")`): no
/// configuration at all, an empty selection, and the one value of the enum that is not scoped
/// to the Kafka event sources. The other two are refused on a queue source altogether, which
/// is the table in `event_source_mapping_metrics_scope`.
fn selection_rows() -> Vec<SelectionRow> {
    vec![
        SelectionRow {
            case: "no MetricsConfig",
            metrics_config: Value::Null,
            published: false,
        },
        SelectionRow {
            case: "an empty Metrics list",
            metrics_config: json!({ "Metrics": [] }),
            published: false,
        },
        SelectionRow {
            case: "EventCount alone",
            metrics_config: json!({ "Metrics": ["EventCount"] }),
            published: true,
        },
    ]
}

/// RULE: the mapping's `MetricsConfig` is the switch for the family — a mapping that selects
/// `EventCount` publishes it and a mapping that does not publishes none of it, however much
/// its poller is doing.
#[tokio::test]
async fn only_a_metrics_config_selecting_event_count_publishes_the_family() {
    let router = emulator();
    for (index, row) in selection_rows().iter().enumerate() {
        let case = row.case;
        let mut extras = json!({ "BatchSize": 10 });
        if !row.metrics_config.is_null() {
            extras["MetricsConfig"] = row.metrics_config.clone();
        }
        let (uuid, url) =
            given_mapping(&router, case, &format!("metrics-switch-{index}"), &extras).await;
        send_message(&router, case, &url, r#"{"kind":"keep"}"#).await;
        // The poller takes the message out of the queue, which is the work the family would
        // report: once the message is in flight there has been a pass to report.
        let in_flight = wait_for_in_flight(&router, case, &url, "1").await;
        assert_eq!(
            in_flight.as_deref(),
            Some("1"),
            "case {case:?}: the mapping's poller must take the message out of the queue, got \
             {in_flight:?}"
        );
        tokio::time::sleep(SEVERAL_PASSES).await;

        for metric in EVENT_COUNT_FAMILY {
            let points = if row.published {
                wait_for_datapoints(&router, case, metric, &uuid).await
            } else {
                datapoints(&router, case, metric, &uuid).await
            };
            assert_eq!(
                !points.is_empty(),
                row.published,
                "case {case:?}: this selection {} publish {metric}, got {} datapoints",
                if row.published {
                    "must"
                } else {
                    "must not"
                },
                points.len()
            );
        }
    }
}

/// RULE: a mapping that polled nothing says so — one 0-valued poll count for the minute,
/// which is what makes an empty window mean "the source was empty" rather than "the mapping
/// is not publishing".
#[tokio::test]
async fn an_idle_mapping_publishes_a_zero_valued_poll_count() {
    let case = "an enabled mapping on an empty queue";
    let router = emulator();
    let (uuid, url) = given_mapping(
        &router,
        case,
        "idle-poll-count",
        &json!({ "MetricsConfig": { "Metrics": ["EventCount"] } }),
    )
    .await;
    let points = wait_for_datapoints(&router, case, POLLED_EVENT_COUNT_METRIC, &uuid).await;
    assert!(
        !points.is_empty(),
        "case {case:?}: an idle mapping must still publish its poll count, got no datapoints"
    );
    // Several passes later it is still one datapoint per minute, not one per pass: the
    // series is published at the one-minute storage resolution, so that is the resolution an
    // idle mapping reports at.
    tokio::time::sleep(SEVERAL_PASSES).await;
    for point in datapoints(&router, case, POLLED_EVENT_COUNT_METRIC, &uuid).await {
        assert_eq!(
            point["Sum"],
            json!(0.0),
            "case {case:?}: nothing was polled, so the minute's poll count must be 0, got \
             {point}"
        );
        assert_eq!(
            point["SampleCount"],
            json!(1.0),
            "case {case:?}: an idle mapping publishes one poll count per minute, got {point}"
        );
    }
    let holding = queue_held(&router, case, &url).await;
    assert_eq!(
        holding, 0,
        "case {case:?}: nothing was sent to the queue, so it must hold nothing, got {holding}"
    );
}

/// One row of the twin table: the filter the two mappings share, and what is sent to each of
/// their queues.
struct TwinRow {
    case: &'static str,
    /// The `Pattern` of each `FilterCriteria` filter both mappings carry.
    patterns: Vec<&'static str>,
    /// Each message body sent to BOTH queues, beside whether the row's patterns admit it.
    bodies: Vec<(&'static str, bool)>,
}

/// The catalog of twinned work: a JSON-body filter admitting some of the messages, one
/// admitting none of them, and a plain-text filter admitting all but one — so the contrast is
/// asserted over a filter that rejects some, all and one of what arrives.
fn twin_rows() -> Vec<TwinRow> {
    vec![
        TwinRow {
            case: "a json body filter admitting one of four",
            patterns: vec![r#"{"body":{"kind":["keep"]}}"#],
            bodies: vec![
                (r#"{"kind":"keep"}"#, true),
                (r#"{"kind":"drop"}"#, false),
                (r#"{"kind":"other"}"#, false),
                ("not json at all", false),
            ],
        },
        TwinRow {
            case: "a json body filter admitting none of three",
            patterns: vec![r#"{"body":{"kind":["keep"]}}"#],
            bodies: vec![
                (r#"{"kind":"drop"}"#, false),
                (r#"{"kind":"other"}"#, false),
                (r#"{"kind":"later"}"#, false),
            ],
        },
        TwinRow {
            case: "a plain-text filter rejecting one of two",
            patterns: vec![r#"{"body":["keep"]}"#],
            bodies: vec![("keep", true), ("drop", false)],
        },
    ]
}

/// RULE: the per-mapping event counts are published for the mapping whose `MetricsConfig`
/// selects `EventCount` and for no other mapping — two mappings filtering the same messages
/// with the same criteria, one opted in and one not, do the same polling, filtering and
/// deleting, and only the opted-in one has a series at CloudWatch.
#[tokio::test]
async fn only_the_opted_in_twin_of_two_filtering_mappings_publishes_its_event_counts() {
    let router = emulator();
    for (index, row) in twin_rows().iter().enumerate() {
        let case = row.case;
        let filters: Vec<Value> = row
            .patterns
            .iter()
            .map(|pattern| json!({ "Pattern": pattern }))
            .collect();
        // The two mappings differ in exactly ONE member: whether the `MetricsConfig` selects
        // the event counts. Everything else — the filter, the batch size, the function, the
        // messages — is the same, which is what makes the contrast the selection's own.
        let shared = json!({
            "BatchSize": 10,
            "FilterCriteria": { "Filters": filters },
        });
        let mut opted_in = shared.clone();
        opted_in["MetricsConfig"] = json!({ "Metrics": ["EventCount"] });
        let (metered_uuid, metered_url) =
            given_mapping(&router, case, &format!("twin-metered-{index}"), &opted_in).await;
        let (silent_uuid, silent_url) =
            given_mapping(&router, case, &format!("twin-silent-{index}"), &shared).await;

        for (body, _) in &row.bodies {
            send_message(&router, case, &metered_url, body).await;
            send_message(&router, case, &silent_url, body).await;
        }

        let sent = u32::try_from(row.bodies.len()).unwrap_or(u32::MAX);
        let admitted =
            u32::try_from(row.bodies.iter().filter(|(_, keep)| *keep).count()).unwrap_or(u32::MAX);
        let rejected = sent - admitted;

        // The opted-in mapping reports what its pass did with the events it polled.
        let polled = wait_for_sum(
            &router,
            case,
            POLLED_EVENT_COUNT_METRIC,
            &metered_uuid,
            f64::from(sent),
        )
        .await;
        assert!(
            polled >= f64::from(sent),
            "case {case:?}: every one of the {sent} messages sent to the opted-in mapping's \
             queue must be counted as polled, got a {POLLED_EVENT_COUNT_METRIC} of {polled}"
        );
        let filtered_out = wait_for_sum(
            &router,
            case,
            FILTERED_OUT_EVENT_COUNT_METRIC,
            &metered_uuid,
            f64::from(rejected),
        )
        .await;
        assert_eq!(
            filtered_out,
            f64::from(rejected),
            "case {case:?}: the filter rejects {rejected} of the {sent} messages, got a \
             {FILTERED_OUT_EVENT_COUNT_METRIC} of {filtered_out}"
        );

        // Both mappings did the same work: a rejected message leaves its queue, so each
        // queue is left holding only what the filter admitted.
        for (label, url) in [("opted-in", &metered_url), ("opt-out twin", &silent_url)] {
            let holding = wait_for_held(&router, case, url, admitted).await;
            assert_eq!(
                holding, admitted,
                "case {case:?}: the {label} mapping must leave its queue holding only the \
                 {admitted} messages its filter admits, got {holding}"
            );
        }

        // And the twin, which did that work with no selection, has no series at all.
        tokio::time::sleep(SEVERAL_PASSES).await;
        for metric in EVENT_COUNT_FAMILY {
            let points = datapoints(&router, case, metric, &silent_uuid).await;
            assert!(
                points.is_empty(),
                "case {case:?}: the opt-out twin selects no metric, so it must publish no \
                 {metric}, got {} datapoint(s)",
                points.len()
            );
        }
        // The opted-in mapping's own series count its OWN queue's events and none of its
        // twin's: the twin rejected the same messages on the same filter, so a series fed by
        // both would report twice the rejections.
        let sums = family_sums(&router, case, &metered_uuid).await;
        assert_eq!(
            sums[1],
            f64::from(rejected),
            "case {case:?}: the opted-in mapping's filtered-out count must stay at the \
             {rejected} events its own filter rejected, got {sums:?}"
        );
        // An event is deleted only because the filter rejected it or an invocation accepted
        // it, which holds whatever the invocation came to.
        assert_eq!(
            sums[3],
            sums[1] + sums[2],
            "case {case:?}: an event is deleted only because the filter rejected it or an \
             invocation accepted it, got {sums:?}"
        );
    }
}

/// RULE: emptying a mapping's `MetricsConfig` stops the family — the mapping stays
/// `Enabled` and keeps draining its queue, the totals already published stay where they
/// are, and the work done after the change adds no datapoint.
#[tokio::test]
async fn an_emptied_metrics_config_freezes_the_series_while_the_queue_is_still_drained() {
    let case = "MetricsConfig emptied on an enabled mapping";
    let router = emulator();
    // Every message this row sends is one the filter rejects, so every one of them is
    // counted, deleted and drained — the counting is exact and the draining is observable
    // without anything having to run a deployment package.
    let (uuid, url) = given_mapping(
        &router,
        case,
        "frozen-event-counts",
        &json!({
            "BatchSize": 10,
            "FilterCriteria": { "Filters": [{ "Pattern": r#"{"body":{"kind":["keep"]}}"# }] },
            "MetricsConfig": { "Metrics": ["EventCount"] },
        }),
    )
    .await;
    let counted = 3u32;
    for index in 0..counted {
        send_message(&router, case, &url, &json!({ "kind": index }).to_string()).await;
    }
    let polled = wait_for_sum(
        &router,
        case,
        POLLED_EVENT_COUNT_METRIC,
        &uuid,
        f64::from(counted),
    )
    .await;
    assert_eq!(
        polled,
        f64::from(counted),
        "case {case:?}: the {counted} messages sent must be counted as polled exactly once \
         each, got a {POLLED_EVENT_COUNT_METRIC} of {polled}"
    );
    let holding = wait_for_held(&router, case, &url, 0).await;
    assert_eq!(
        holding, 0,
        "case {case:?}: the filter rejected every message, so the queue must have drained, \
         got {holding}"
    );

    // The switch is turned off: "the metrics you want your event source mapping to produce"
    // is now nothing.
    let updated = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ "MetricsConfig": { "Metrics": [] } }),
    )
    .await;
    assert_eq!(
        updated.status,
        202,
        "case {case:?}: expected UpdateEventSourceMapping to answer 202, got {} {}",
        updated.status,
        updated.text()
    );
    wait_for_enabled(&router, case, &uuid).await;
    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        reading.json().get("MetricsConfig"),
        Some(&json!({ "Metrics": [] })),
        "case {case:?}: the emptied selection must be read back empty, got {}",
        reading.text()
    );

    // Whatever was already in flight when the change landed has had time to publish, so
    // what the series hold now is the frozen total.
    tokio::time::sleep(SEVERAL_PASSES).await;
    let frozen = family_sums(&router, case, &uuid).await;

    let after = 4u32;
    for index in 0..after {
        send_message(
            &router,
            case,
            &url,
            &json!({ "kind": format!("after-{index}") }).to_string(),
        )
        .await;
    }
    // The mapping is still polling, filtering and deleting: the queue drains again.
    let holding = wait_for_held(&router, case, &url, 0).await;
    assert_eq!(
        holding, 0,
        "case {case:?}: an emptied MetricsConfig stops the metrics, not the polling, so the \
         {after} further messages must drain too, got {holding}"
    );
    tokio::time::sleep(SEVERAL_PASSES).await;

    let still = family_sums(&router, case, &uuid).await;
    assert_eq!(
        still, frozen,
        "case {case:?}: no datapoint may be added once the selection is empty, so the totals \
         must stay at {frozen:?}, got {still:?}"
    );
    assert_eq!(
        still.first().copied(),
        Some(f64::from(counted)),
        "case {case:?}: the total stays at the {counted} events counted while the selection \
         held EventCount, got {still:?}"
    );
}
