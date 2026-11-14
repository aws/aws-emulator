// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how many batches ONE pass of an Amazon SQS event source mapping's
//! poller hands to its function, and whether those invocations really run at once.
//!
//! RULE: a mapping configuring `ScalingConfig$MaximumConcurrency` — "limits the number of
//! concurrent instances that the Amazon SQS event source can invoke" — hands that many
//! batches of at most its `BatchSize` to its function AT ONCE, bounded by what the function
//! itself reserves ("the number of simultaneous executions to reserve for the function",
//! `PutFunctionConcurrency`); a mapping configuring none hands over one batch per pass, this
//! poller's own cadence.
//!
//! Both tests drive real HTTP requests through the production `MultiServiceRouter` with
//! Lambda, Amazon SQS and CloudWatch on ONE engine, and both make the poll passes THEMSELVES
//! (`crate::state::claim_poll_passes`): a pass answers once every batch it took has been
//! handed over, so what the test reads afterwards is the state that pass left rather than a
//! race against a background loop. The rows differ along what the rule generalises over: the
//! concurrency configured (none, each end of the published range, a value inside it), how many
//! messages one batch may hold, how long the backlog is, and what the function reserves.
//!
//! The counting test's function runs no package at all, so no batch is ever processed: what a
//! pass TOOK is then exactly what is missing from the queue's visible messages, which is the
//! quantity the rule is about. The overlap test's package does run, and the concurrency its
//! invocations reach is read where a client reads it — off Lambda's own
//! `ConcurrentExecutions` series, whose datapoints are the observations of that gauge at the
//! instants it rises.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, call, create_mapping, create_queue, create_running_function,
    create_timed_function, queue_message_counts, router_on, send_message, DrivenPolling,
    STATE_ENABLED,
};

/// The mapped function's `Timeout`, in seconds, and the queue's own `VisibilityTimeout`: the
/// same value, which is the pairing `CreateEventSourceMapping` accepts (a queue must outlast
/// the function it feeds). Several seconds, so a message a pass took is still out of the queue
/// while the row reads the counts that pass left.
const TIMEOUT_SECONDS: i64 = 30;

/// Where `PutFunctionConcurrency` is bound
/// (`PUT /2017-10-31/functions/{FunctionName}/concurrency`).
const CONCURRENCY_URI: &str = "/2017-10-31/functions";

/// The namespace Lambda publishes its own metrics in and the series the overlap test reads,
/// written as the AWS Lambda metric reference publishes them rather than as the crate's own
/// constants — so a series published under a name no AWS reader would ask for fails here.
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const CONCURRENT_SERIES: &str = "ConcurrentExecutions";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";
/// The unit a concurrency is measured in: it is a count of executions.
const COUNT_UNIT: &str = "Count";
/// The `Period` the query reads at: the one-minute granularity these series are stored at.
const PERIOD_SECS: i64 = 60;
/// How far either side of now the query's window reaches, in seconds. Generous, so the
/// datapoint the pass published is inside it however long the pass took.
const WINDOW_SECS: f64 = 600.0;

/// The Region every call below is made in.
const REGION: &str = "us-east-1";

/// How long a row waits for work the CLOCK owes — a function going `Active`, a mapping
/// settling into `Enabled`. A ceiling on patience, never a span anything counts on.
const BUDGET: Duration = Duration::from_secs(120);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long the handler of the overlap test holds its execution environment, in the seconds
/// `time.sleep` takes. Short, because the run capacity of this process is shared with every
/// other test beside it — and it is the HANDLER's own run, not a wait this test makes: a pass
/// that dispatched its batches one after another would take as many of these as it has
/// batches, and one that dispatches them together takes one.
const HELD_SECONDS: f64 = 0.4;

/// The least concurrency the overlap test accepts as evidence that the batches of one pass
/// really overlapped: two invocations of the function in flight at one instant.
const OVERLAP: f64 = 2.0;

/// One row of the counting table: what the mapping and the function ask for, how long the
/// backlog is, and how many messages one pass must then take out of the queue.
struct Row {
    case: &'static str,
    /// `ScalingConfig$MaximumConcurrency`, or `None` for a mapping that configures none.
    maximum_concurrency: Option<i64>,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from your
    /// stream or queue".
    batch_size: usize,
    /// `ReservedConcurrentExecutions`, or `None` for a function that reserves nothing.
    reserved: Option<i64>,
    /// How many messages are waiting when the pass is made.
    waiting: u32,
    /// How many of them one pass must take out of the queue.
    takes: u32,
}

/// The catalog: a mapping that configures no concurrency, one at each end of the published
/// range of `MaximumConcurrency` (`smithy_query(query="shape", shape="MaximumConcurrency")`:
/// `@range(min: 2, max: 1000)`), one inside it, one whose batches hold several messages, one
/// whose backlog is shorter than its concurrency, and one whose function reserves less
/// concurrency than the mapping asks for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no configured concurrency",
            maximum_concurrency: None,
            batch_size: 1,
            reserved: None,
            waiting: 6,
            takes: 1,
        },
        Row {
            case: "the published minimum concurrency",
            maximum_concurrency: Some(2),
            batch_size: 1,
            reserved: None,
            waiting: 6,
            takes: 2,
        },
        Row {
            case: "a concurrency inside the published range",
            maximum_concurrency: Some(4),
            batch_size: 1,
            reserved: None,
            waiting: 6,
            takes: 4,
        },
        Row {
            case: "a concurrency of three over batches of two",
            maximum_concurrency: Some(3),
            batch_size: 2,
            reserved: None,
            waiting: 8,
            takes: 6,
        },
        Row {
            case: "a backlog shorter than the configured concurrency",
            maximum_concurrency: Some(4),
            batch_size: 1,
            reserved: None,
            waiting: 2,
            takes: 2,
        },
        Row {
            case: "the published maximum concurrency over a backlog of five",
            maximum_concurrency: Some(1_000),
            batch_size: 1,
            reserved: None,
            waiting: 5,
            takes: 5,
        },
        Row {
            case: "a function reserving less than the mapping asks for",
            maximum_concurrency: Some(4),
            batch_size: 1,
            reserved: Some(2),
            waiting: 6,
            takes: 2,
        },
    ]
}

/// Lambda beside Amazon SQS and CloudWatch on ONE engine, with the poll passes of its
/// mappings this test's own to make.
fn driven_router() -> (MultiServiceRouter, DrivenPolling, Arc<engine::InMemoryEngine>) {
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
    (router, driven, engine)
}

/// `PutFunctionConcurrency` reserving `reserved` simultaneous executions for `function`.
async fn reserve(router: &MultiServiceRouter, case: &str, function: &str, reserved: i64) {
    let answer = call(
        router,
        "PUT",
        &format!("{CONCURRENCY_URI}/{function}/concurrency"),
        &json!({ "ReservedConcurrentExecutions": reserved }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: PutFunctionConcurrency({reserved}) answered {} {}",
        answer.status,
        answer.text()
    );
}

/// The mapping a row asks for, created and settled into `Enabled`.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    queue_arn: &str,
    batch_size: usize,
    maximum_concurrency: Option<i64>,
) {
    let mut request = json!({
        "FunctionName": function,
        "EventSourceArn": queue_arn,
        "BatchSize": batch_size,
        "Enabled": true,
    });
    if let Some(concurrency) = maximum_concurrency {
        request["ScalingConfig"] = json!({ "MaximumConcurrency": concurrency });
    }
    let created = create_mapping(router, &request).await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: CreateEventSourceMapping answered {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    await_mapping_state(router, case, &uuid, STATE_ENABLED).await;
}

/// RULE: one pass hands over as many batches as the mapping's configured concurrency admits,
/// bounded by what its function reserves, and one batch for a mapping configuring none.
#[tokio::test]
async fn one_pass_hands_over_as_many_batches_as_the_configured_concurrency_admits() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, mut driven, _engine) = driven_router();
        let function = format!("esm-concurrency-fn-{index}");
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-concurrency-queue-{index}"),
            &json!({ "VisibilityTimeout": TIMEOUT_SECONDS.to_string() }),
        )
        .await;
        create_timed_function(&router, case, &function, TIMEOUT_SECONDS).await;
        if let Some(reserved) = row.reserved {
            reserve(&router, case, &function, reserved).await;
        }
        given_mapping(
            &router,
            case,
            &function,
            &arn,
            row.batch_size,
            row.maximum_concurrency,
        )
        .await;
        for message in 0..row.waiting {
            send_message(
                &router,
                case,
                &url,
                &json!({ "mark": message }).to_string(),
                None,
            )
            .await;
        }

        driven.poll_once().await;

        // Nothing this function is handed can be processed — its package runs no handler — so
        // every message the pass took is still held by the queue, out of sight: what is
        // VISIBLE is what the pass left behind.
        let counts = queue_message_counts(&router, case, &url).await;
        let taken = row.waiting.saturating_sub(counts.0);
        assert_eq!(
            taken, row.takes,
            "case {case:?}: one pass must take {} of the {} message(s) waiting, got {counts:?} \
             as (visible, in flight, delayed)",
            row.takes, row.waiting
        );
    }
}

/// The highest `ConcurrentExecutions` datapoint CloudWatch reports for `function`, which is
/// the greatest concurrency that function was SEEN to hold.
async fn observed_concurrency(router: &MultiServiceRouter, case: &str, function: &str) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            "GraniteServiceVersion20100801.GetMetricStatistics",
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(
            json!({
                "Namespace": NAMESPACE,
                "MetricName": CONCURRENT_SERIES,
                "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
                "StartTime": now - WINDOW_SECS,
                "EndTime": now + WINDOW_SECS,
                "Period": PERIOD_SECS,
                "Statistics": ["Maximum"],
                "Unit": COUNT_UNIT,
            })
            .to_string(),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {answered}"
    );
    answered["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Maximum"].as_f64())
                .fold(0.0, f64::max)
        })
        .unwrap_or_default()
}

/// RULE, the other half: the batches one pass hands over are IN FLIGHT TOGETHER, so a mapping
/// configured for several concurrent instances really runs several at once — which is what a
/// client sizing that setting for a drain rate is asking for.
///
/// The handler holds its environment while the others start, and the concurrency the function
/// reached is read off its own `ConcurrentExecutions` series: a pass that handed its batches
/// over one after another would have published no reading above one.
#[tokio::test]
async fn the_batches_of_one_pass_are_in_flight_together() {
    let case = "four batches of one message, a concurrency of four";
    let (router, mut driven, _engine) = driven_router();
    let function = "esm-concurrency-overlap";
    let (url, arn) = create_queue(
        &router,
        case,
        "esm-concurrency-overlap-queue",
        &json!({ "VisibilityTimeout": TIMEOUT_SECONDS.to_string() }),
    )
    .await;
    // The handler holds its environment for the span its event names, which is what makes the
    // overlap of the batches of one pass observable.
    create_running_function(
        &router,
        case,
        function,
        "import time\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   time.sleep(float(event['Records'][0]['body'] or 0))\n\
         \x20   return {'records': len(event['Records'])}\n",
        TIMEOUT_SECONDS,
        &json!({}),
    )
    .await;
    given_mapping(&router, case, function, &arn, 1, Some(4)).await;
    for _ in 0..4 {
        send_message(&router, case, &url, &HELD_SECONDS.to_string(), None).await;
    }

    driven.poll_once().await;

    // Every message the pass handed over was processed, so the queue is empty: the batches
    // were really run and not merely taken.
    let deadline = Instant::now() + BUDGET;
    loop {
        let counts = queue_message_counts(&router, case, &url).await;
        if counts == (0, 0, 0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: every message the pass handed over must be processed, got \
             {counts:?} as (visible, in flight, delayed)"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    let observed = observed_concurrency(&router, case, function).await;
    assert!(
        observed >= OVERLAP,
        "case {case:?}: expected the function to be seen holding at least {OVERLAP} concurrent \
         executions, got {observed}"
    );
}
