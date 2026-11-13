// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the series a client reads to learn WHAT AN INVOCATION CAME TO:
//! `Duration`, `Errors` and `Throttles`, read back where a client reads them — the
//! `AWS/Lambda` series CloudWatch answers for, with CloudWatch mounted beside Lambda on one
//! engine.
//!
//! The RULE asserted over the tables below: Lambda publishes a datapoint about every
//! invocation it performs, in the `AWS/Lambda` namespace, dimensioned on the function
//! (`FunctionName`) and on the resource the call addressed (`FunctionName` plus `Resource`) —
//!
//! * `Duration`, in `Milliseconds`, carrying the time THAT run took, once per invocation that
//!   ran, whichever surface started it and whether or not the handler failed;
//! * `Errors`, in `Count`, one per invocation that came to a function error — the handler's
//!   own exception and the runtime's alike — and none for an invocation that returned;
//! * `Throttles`, in `Count`, one per invocation request refused because the function's
//!   concurrency pool had no room for it, which is a request that never reached the function
//!   and is therefore counted in `Throttles` instead of `Invocations`.
//!
//! So a client can read how long a function takes, chart its error rate and alarm on either,
//! and an alarm on `Errors` has the datapoints to transition on.
//!
//! The rows differ along what the rule generalises over: how many invocations are made, how
//! many of them fail, HOW each one fails (the handler raising, and the sandbox ending a run at
//! the function's own `Timeout`), how long the run takes, the invocation surface (the
//! synchronous call, the asynchronous one delivered off the response path, the
//! response-streaming call), and — for the throttle table — how many requests are refused and
//! on which surface.
//!
//! Every metric, dimension and unit name here is written as a literal, taken from the AWS
//! Lambda metric reference rather than from this crate's constants, so a series published
//! under a name no AWS reader would ask for fails here instead of agreeing with itself. The
//! Lambda-side harness (`send`, `Answer`, the activation window) is `function_lifecycle.rs`'s,
//! the deployment archive is `synchronous_invoke_run.rs`'s, and the CloudWatch call is
//! `kinesis_event_source_delivery.rs`'s.
//!
//! Every wait here is for a CONDITION, looked at again until it holds or a generous ceiling
//! passes — the series holding the measurement an accepted delivery owes — and every limit no
//! case is about (the `Timeout` a handler is given outside the one row that ends a run at it,
//! the ceiling on the wait itself) is set from what the emulator allows that work rather than
//! picked tight. So how fast the host runs decides nothing asserted here: a slow or loaded
//! host is waited for, and only a measurement that never appears fails a case.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    HOST_WAIT_CEILING, INIT_PHASE_LIMIT, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ASYNC_DELIVERY_DELAY, FIRST_ASYNC_RETRY_DELAY,
    MAX_ASYNC_DELIVERY_ATTEMPTS, SECOND_ASYNC_RETRY_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

// Waiting for an accepted asynchronous invocation to have been DELIVERED is the same wait the
// suite over that plane makes, so this one makes it with the helper that suite publishes: it
// looks at the entry the accepting call queued until the delivery attempt has settled, and
// waits on the delivery making progress rather than on a span of wall clock.
use crate::asynchronous_invoke_run::settled_newest;
use crate::function_lifecycle::{make_activation_due, send, Answer, ROLE};
use crate::kinesis_event_source_delivery::cloudwatch;
use crate::synchronous_invoke_run::package;

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The names that reference gives the series under test, and the units it reports them in.
const DURATION_SERIES: &str = "Duration";
/// See [`DURATION_SERIES`].
const ERRORS_SERIES: &str = "Errors";
/// See [`DURATION_SERIES`].
const THROTTLES_SERIES: &str = "Throttles";
/// See [`DURATION_SERIES`].
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`DURATION_SERIES`].
const MILLISECONDS_UNIT: &str = "Milliseconds";
/// See [`DURATION_SERIES`].
const COUNT_UNIT: &str = "Count";

/// The dimensions that reference reports them per: the function, and the resource of it an
/// invocation addressed.
const FUNCTION_DIMENSION: &str = "FunctionName";
/// See [`FUNCTION_DIMENSION`].
const RESOURCE_DIMENSION: &str = "Resource";

/// The function every row invokes.
const FUNCTION: &str = "measured";

/// The module and entry point every package below holds its handler at.
const HANDLER: &str = "lambda_function.lambda_handler";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `InvokeWithResponseStream` is bound.
const RESPONSE_STREAMING_URI: &str = "/2021-11-15/functions";

/// The request header `Invoke$InvocationType` is bound to.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds.
const WINDOW_SECS: f64 = 600.0;

/// The `Reason` a throttle that hit the function's own reservation names, one of the model's
/// `ThrottleReason` values (`smithy_query(query="shape", shape="ThrottleReason")`).
const RESERVED_REASON: &str = "ReservedFunctionConcurrentInvocationLimitExceeded";

/// The `x-amzn-errortype` a throttled invocation carries.
const THROTTLE_ERROR: &str = "TooManyRequestsException";

/// How long the series is waited for ONCE the delivery it measures has happened: the run
/// publishes its measurements before the attempt that made it settles, so this is a backstop
/// and not a wait on the delivery itself ([`settled_newest`] is that wait, and it bounds
/// nothing but the emulator going quiet about the event).
///
/// A ceiling on PATIENCE, never a span anything here counts on, and taken from what the
/// EMULATOR itself allows that work rather than picked here: the event waits its queueing
/// delay ([`ASYNC_DELIVERY_DELAY`]), then an attempt waits for a share of this process's run
/// capacity ([`PERMIT_WAIT`]), initializes an execution environment ([`INIT_PHASE_LIMIT`]) and
/// runs the handler ([`HOST_WAIT_CEILING`]) — with room for every attempt the retry policy has
/// for one event ([`MAX_ASYNC_DELIVERY_ATTEMPTS`], separated by the waits AWS publishes
/// between them, [`FIRST_ASYNC_RETRY_DELAY`] and [`SECOND_ASYNC_RETRY_DELAY`]). Each
/// of those spans is shared with every other suite of a whole-workspace run, so only a
/// delivery that never arrives at all fails the wait below; how fast the host runs decides
/// nothing this file asserts.
const DELIVERY_BUDGET: Duration = ASYNC_DELIVERY_DELAY
    .saturating_add(FIRST_ASYNC_RETRY_DELAY)
    .saturating_add(SECOND_ASYNC_RETRY_DELAY)
    .saturating_add(
        PERMIT_WAIT
            .saturating_add(INIT_PHASE_LIMIT)
            .saturating_add(HOST_WAIT_CEILING)
            .saturating_mul(MAX_ASYNC_DELIVERY_ATTEMPTS),
    );

/// How long each poll of that wait sleeps before it looks at the series again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `Timeout` every function created here is given, except where a row is ABOUT the
/// timeout.
///
/// Generous on purpose, because no assertion outside that one row is about it: a handler
/// stopped at this deadline is reported as a run that ERRORED, so a tight value would turn a
/// slow or loaded host into a `FunctionError` the handler never raised. Minutes for handlers
/// that return in milliseconds, and well inside the range the model allows the member
/// (`smithy_query(query="shape", shape="Timeout")`: 1 to 5400).
const GENEROUS_TIMEOUT_SECS: u32 = 300;

/// The handler every row runs: it sleeps for as long as the event asks, then fails the way
/// the event asks or returns a value.
const SOURCE: &str = r#"
import time


def lambda_handler(event, context):
    sleep_ms = event.get("sleep_ms") or 0
    if sleep_ms:
        time.sleep(sleep_ms / 1000.0)
    if event.get("raises"):
        raise RuntimeError("this invocation failed on purpose")
    return {"slept_ms": sleep_ms}
"#;

/// Lambda and CloudWatch on one engine: the series Lambda publishes is CloudWatch's, so it is
/// read in-process through CloudWatch's own surface — the shape `core/router/src/lib.rs`
/// mounts them in.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// Now, in the epoch seconds a stored datapoint is stamped with.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Create the function every row invokes, with the `Timeout` the row needs, and bring it to
/// `Active`.
async fn create_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    timeout_secs: u32,
) {
    let created = send(
        router,
        "POST",
        FUNCTIONS_URI,
        "application/json",
        serde_json::to_vec(&json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": timeout_secs,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(SOURCE)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateFunction must succeed, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `PutFunctionConcurrency` (`PUT /2017-10-31/functions/{FunctionName}/concurrency`).
async fn reserve_concurrency(router: &MultiServiceRouter, case: &str, reserved: i64) {
    let answer = send(
        router,
        "PUT",
        &format!("/2017-10-31/functions/{FUNCTION}/concurrency"),
        "application/json",
        serde_json::to_vec(&json!({ "ReservedConcurrentExecutions": reserved }))
            .expect("the concurrency body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutFunctionConcurrency({reserved}) must succeed, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// `DeleteFunctionConcurrency`, which gives the function its account-level capacity back.
async fn release_concurrency(router: &MultiServiceRouter, case: &str) {
    let answer = send(
        router,
        "DELETE",
        &format!("/2017-10-31/functions/{FUNCTION}/concurrency"),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case {case:?}: DeleteFunctionConcurrency must succeed, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// One invocation of the function with `event`, on the surface `surface` names.
async fn invoke(
    router: &MultiServiceRouter,
    surface: Surface,
    invocation_type: &str,
    event: &Value,
) -> Answer {
    let uri = match surface {
        Surface::Invoke => format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        Surface::ResponseStream => {
            format!("{RESPONSE_STREAMING_URI}/{FUNCTION}/response-streaming-invocations")
        }
    };
    send(
        router,
        "POST",
        &uri,
        "application/json",
        event.to_string().into_bytes(),
        &[(INVOCATION_TYPE_HEADER, invocation_type.to_string())],
    )
    .await
}

/// The dimension set of the series reported per function.
fn function_dimensions() -> Value {
    json!([{ "Name": FUNCTION_DIMENSION, "Value": FUNCTION }])
}

/// The dimension set of the series reported per invoked resource: the function itself for an
/// unqualified invocation.
fn resource_dimensions() -> Value {
    json!([
        { "Name": FUNCTION_DIMENSION, "Value": FUNCTION },
        { "Name": RESOURCE_DIMENSION, "Value": FUNCTION },
    ])
}

/// The `Datapoints` `GetMetricStatistics` answers for the series `metric` and `dimensions`
/// identify, selected by the unit the AWS Lambda metric reference gives that series.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    unit: &str,
    dimensions: &Value,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum", "SampleCount", "Minimum", "Maximum"],
            "Unit": unit,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics({metric}, {dimensions}) answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The total of one statistic over every datapoint of a series.
fn total(points: &[Value], statistic: &str) -> f64 {
    points
        .iter()
        .filter_map(|point| point[statistic].as_f64())
        .sum()
}

/// The smallest `Minimum` any datapoint of a series reports, or `None` for a series with no
/// datapoint at all.
fn smallest(points: &[Value]) -> Option<f64> {
    points
        .iter()
        .filter_map(|point| point["Minimum"].as_f64())
        .min_by(f64::total_cmp)
}

/// Every series `ListMetrics` discovers in [`NAMESPACE`], as `(MetricName, dimension pairs)`
/// with the dimensions ordered by name.
async fn listed_series(
    router: &MultiServiceRouter,
    case: &str,
) -> Vec<(String, Vec<(String, String)>)> {
    let (status, listed) =
        cloudwatch(router, "ListMetrics", &json!({ "Namespace": NAMESPACE })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {listed}"
    );
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|series| {
            let mut dimensions: Vec<(String, String)> = series["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            dimensions.sort();
            (
                series["MetricName"].as_str().unwrap_or_default().to_string(),
                dimensions,
            )
        })
        .collect()
}

/// Which invocation surface a row drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    /// `Invoke`.
    Invoke,
    /// `InvokeWithResponseStream`.
    ResponseStream,
}

/// One invocation a row makes: how long its handler sleeps, and whether it raises.
#[derive(Clone, Copy, Debug)]
struct Run {
    surface: Surface,
    sleep_ms: u64,
    raises: bool,
}

/// One row of the outcome table: the invocations it makes, and what the two series must then
/// report about them.
struct Row {
    case: &'static str,
    /// The function's `Timeout`, in seconds.
    timeout_secs: u32,
    runs: &'static [Run],
    /// How many measurements `Duration` must hold: one per invocation that ran.
    duration_samples: f64,
    /// The floor every `Duration` datapoint must clear, in milliseconds.
    least_duration_ms: f64,
    /// What `Errors` must sum to; 0 means the series must hold no datapoint at all.
    error_sum: f64,
}

/// The rows. Every expectation is written out rather than computed from the runs, so a
/// publisher that counted calls and one that counted invocations cannot both pass.
const ROWS: &[Row] = &[
    Row {
        case: "one invocation that returns a value",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[Run {
            surface: Surface::Invoke,
            sleep_ms: 0,
            raises: false,
        }],
        duration_samples: 1.0,
        least_duration_ms: 0.0,
        error_sum: 0.0,
    },
    Row {
        case: "one invocation whose handler raises",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[Run {
            surface: Surface::Invoke,
            sleep_ms: 0,
            raises: true,
        }],
        duration_samples: 1.0,
        least_duration_ms: 0.0,
        error_sum: 1.0,
    },
    Row {
        case: "a run that takes a measurable span",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[Run {
            surface: Surface::Invoke,
            sleep_ms: 300,
            raises: false,
        }],
        duration_samples: 1.0,
        least_duration_ms: 300.0,
        error_sum: 0.0,
    },
    Row {
        case: "three invocations, two of them failing",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: true,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: true,
            },
        ],
        duration_samples: 3.0,
        least_duration_ms: 0.0,
        error_sum: 2.0,
    },
    Row {
        case: "five invocations, none of them failing",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
            Run {
                surface: Surface::Invoke,
                sleep_ms: 0,
                raises: false,
            },
        ],
        duration_samples: 5.0,
        least_duration_ms: 0.0,
        error_sum: 0.0,
    },
    Row {
        case: "a response-streaming invocation, which is an invocation of the function too",
        timeout_secs: GENEROUS_TIMEOUT_SECS,
        runs: &[Run {
            surface: Surface::ResponseStream,
            sleep_ms: 0,
            raises: false,
        }],
        duration_samples: 1.0,
        least_duration_ms: 0.0,
        error_sum: 0.0,
    },
    Row {
        case: "a run the sandbox ended at the function's own Timeout",
        timeout_secs: 1,
        runs: &[Run {
            surface: Surface::Invoke,
            sleep_ms: 4_000,
            raises: false,
        }],
        duration_samples: 1.0,
        least_duration_ms: 1_000.0,
        error_sum: 1.0,
    },
];

/// RULE: every invocation that RAN publishes one `Duration` measurement in `Milliseconds`
/// carrying the time that run took, and one `Errors` count when it came to a function error —
/// in both the function's series and the addressed resource's.
#[tokio::test]
async fn every_run_publishes_its_duration_and_every_failure_publishes_an_error() {
    for row in ROWS {
        // One account per row, so the only invocations in the window are this row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        create_function(&router, &engine, case, row.timeout_secs).await;

        for (index, run) in row.runs.iter().enumerate() {
            let event = json!({ "sleep_ms": run.sleep_ms, "raises": run.raises });
            let answer = invoke(&router, run.surface, "RequestResponse", &event).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: run {index} of {run:?} must answer HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        for dimensions in [function_dimensions(), resource_dimensions()] {
            let durations =
                datapoints(&router, case, DURATION_SERIES, MILLISECONDS_UNIT, &dimensions).await;
            let samples = total(&durations, "SampleCount");
            assert_eq!(
                samples, row.duration_samples,
                "case {case:?}: {DURATION_SERIES} of {dimensions} must hold \
                 {} measurement(s), got {samples} over {durations:?}",
                row.duration_samples
            );
            let least = smallest(&durations);
            assert!(
                least.is_some_and(|least| least >= row.least_duration_ms),
                "case {case:?}: every {DURATION_SERIES} datapoint of {dimensions} must be at \
                 least {} ms, got {least:?} over {durations:?}",
                row.least_duration_ms
            );
            assert!(
                durations
                    .iter()
                    .all(|point| point["Unit"].as_str() == Some(MILLISECONDS_UNIT)),
                "case {case:?}: every {DURATION_SERIES} datapoint must carry the unit \
                 {MILLISECONDS_UNIT:?}, got {durations:?}"
            );

            let errors = datapoints(&router, case, ERRORS_SERIES, COUNT_UNIT, &dimensions).await;
            if row.error_sum == 0.0 {
                assert!(
                    errors.is_empty(),
                    "case {case:?}: nothing failed, so {ERRORS_SERIES} of {dimensions} must \
                     hold no datapoint at all, got {errors:?}"
                );
            } else {
                let sum = total(&errors, "Sum");
                assert_eq!(
                    sum, row.error_sum,
                    "case {case:?}: {ERRORS_SERIES} of {dimensions} must sum to {}, got {sum} \
                     over {errors:?}",
                    row.error_sum
                );
            }
        }

        // The series are DISCOVERABLE, which is the read a client makes before it alarms on
        // them, and a row in which nothing failed registers no error series.
        let listed = listed_series(&router, case).await;
        let duration_series = (
            DURATION_SERIES.to_string(),
            vec![(FUNCTION_DIMENSION.to_string(), FUNCTION.to_string())],
        );
        assert!(
            listed.contains(&duration_series),
            "case {case:?}: ListMetrics must discover {duration_series:?}, got {listed:?}"
        );
        let has_errors = listed.iter().any(|series| series.0 == ERRORS_SERIES);
        assert_eq!(
            has_errors,
            row.error_sum > 0.0,
            "case {case:?}: an {ERRORS_SERIES} series must be discoverable only when an \
             invocation failed, got {listed:?}"
        );

        // Reading the series changes nothing a later reader sees.
        let again = datapoints(
            &router,
            case,
            DURATION_SERIES,
            MILLISECONDS_UNIT,
            &function_dimensions(),
        )
        .await;
        assert_eq!(
            total(&again, "SampleCount"),
            row.duration_samples,
            "case {case:?}: a second read of {DURATION_SERIES} must report the same \
             measurements, got {again:?}"
        );
    }
}

/// One row of the throttle table: how many requests are refused, and on which surface.
struct ThrottleRow {
    case: &'static str,
    surface: Surface,
    /// How many requests are made against a reservation of nothing.
    refused: u32,
}

/// The rows: one refusal, the two the recorded case drives, four, and the same against the
/// response-streaming surface, which draws on the SAME pool.
const THROTTLE_ROWS: &[ThrottleRow] = &[
    ThrottleRow {
        case: "one request refused",
        surface: Surface::Invoke,
        refused: 1,
    },
    ThrottleRow {
        case: "two requests refused",
        surface: Surface::Invoke,
        refused: 2,
    },
    ThrottleRow {
        case: "four requests refused",
        surface: Surface::Invoke,
        refused: 4,
    },
    ThrottleRow {
        case: "requests refused on the response-streaming surface",
        surface: Surface::ResponseStream,
        refused: 3,
    },
];

/// RULE: every invocation request the function's concurrency pool has no room for is answered
/// `TooManyRequestsException` and published as one `Throttles` count — and, having never
/// reached the function, is counted in neither `Invocations` nor `Duration`.
#[tokio::test]
async fn every_throttled_request_is_counted_in_the_throttles_series() {
    for row in THROTTLE_ROWS {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        create_function(&router, &engine, case, GENEROUS_TIMEOUT_SECS).await;
        // "A reservation of zero admits nothing at all": every request below is refused.
        reserve_concurrency(&router, case, 0).await;

        for nth in 1..=row.refused {
            let answer = invoke(
                &router,
                row.surface,
                "RequestResponse",
                &json!({ "sleep_ms": 0, "raises": false }),
            )
            .await;
            assert_eq!(
                answer.status, 429,
                "case {case:?}: request {nth} must be refused with HTTP 429, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains(THROTTLE_ERROR),
                "case {case:?}: request {nth} must be refused as {THROTTLE_ERROR}, got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.json().get("Reason").and_then(Value::as_str),
                Some(RESERVED_REASON),
                "case {case:?}: request {nth} must name the function-level limit, got {}",
                answer.json()
            );
        }

        let expected = f64::from(row.refused);
        for dimensions in [function_dimensions(), resource_dimensions()] {
            let throttles =
                datapoints(&router, case, THROTTLES_SERIES, COUNT_UNIT, &dimensions).await;
            let sum = total(&throttles, "Sum");
            assert_eq!(
                sum, expected,
                "case {case:?}: {THROTTLES_SERIES} of {dimensions} must sum to {expected}, got \
                 {sum} over {throttles:?}"
            );
            assert!(
                throttles
                    .iter()
                    .all(|point| point["Unit"].as_str() == Some(COUNT_UNIT)),
                "case {case:?}: every {THROTTLES_SERIES} datapoint must carry the unit \
                 {COUNT_UNIT:?}, got {throttles:?}"
            );
        }
        for (series, unit) in [
            (INVOCATIONS_SERIES, COUNT_UNIT),
            (DURATION_SERIES, MILLISECONDS_UNIT),
        ] {
            let points = datapoints(&router, case, series, unit, &function_dimensions()).await;
            assert!(
                points.is_empty(),
                "case {case:?}: a throttled request never reached the function, so {series} \
                 must hold no datapoint, got {points:?}"
            );
        }

        // With the reservation released the next request IS admitted, so the absences above
        // are the throttle's and not the harness's — and the throttles already published stay
        // where they are.
        release_concurrency(&router, case).await;
        let answer = invoke(
            &router,
            row.surface,
            "RequestResponse",
            &json!({ "sleep_ms": 0, "raises": false }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the request after the reservation was released must be served, got \
             {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let durations = datapoints(
            &router,
            case,
            DURATION_SERIES,
            MILLISECONDS_UNIT,
            &function_dimensions(),
        )
        .await;
        assert_eq!(
            total(&durations, "SampleCount"),
            1.0,
            "case {case:?}: the one admitted invocation must be the only {DURATION_SERIES} \
             measurement, got {durations:?}"
        );
        let throttles = datapoints(
            &router,
            case,
            THROTTLES_SERIES,
            COUNT_UNIT,
            &function_dimensions(),
        )
        .await;
        assert_eq!(
            total(&throttles, "Sum"),
            expected,
            "case {case:?}: {THROTTLES_SERIES} must still sum to {expected}, got {throttles:?}"
        );
    }
}

/// RULE (the surface the answer says nothing about the run): an ASYNCHRONOUS invocation is
/// delivered off the response path, so the measurements of its run appear once that delivery
/// has happened rather than when the accepting call is answered.
#[tokio::test]
async fn an_asynchronous_invocation_publishes_its_run_once_it_has_been_delivered() {
    let case = "an asynchronous invocation delivered off the response path";
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    create_function(&router, &engine, case, GENEROUS_TIMEOUT_SECS).await;

    let accepted = invoke(
        &router,
        Surface::Invoke,
        "Event",
        &json!({ "sleep_ms": 0, "raises": false }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case {case:?}: an asynchronous invocation must be accepted with HTTP 202, got {} {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );

    // The DELIVERY is waited for first, on the entry the accepting call queued: that is the
    // work the measurements are about, and the emulator's own record of it says when it has
    // happened — so the series below is read once the run it measures has really run, rather
    // than after a span of wall clock in which it may or may not have.
    let delivered = settled_newest(&engine, case, FUNCTION).await;
    assert_eq!(
        delivered.outcome,
        AsyncInvocationOutcome::Delivered {
            function_error: None
        },
        "case {case:?}: the accepted event must be delivered to the function and its handler \
         must return, got {:?}",
        delivered.outcome
    );

    // Polled rather than slept on: the datapoint appears when the run it measures has
    // happened, which the wait above is for. The wait is for that CONDITION — the series
    // holding a measurement — looked at again until it holds or [`DELIVERY_BUDGET`] passes,
    // so a host that takes longer over the publication is waited for rather than failed.
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let durations = loop {
        let durations = datapoints(
            &router,
            case,
            DURATION_SERIES,
            MILLISECONDS_UNIT,
            &function_dimensions(),
        )
        .await;
        if total(&durations, "SampleCount") > 0.0 {
            break durations;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the delivered invocation must publish a {DURATION_SERIES} \
             measurement, and none arrived inside {DELIVERY_BUDGET:?}, leaving {durations:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let samples = total(&durations, "SampleCount");
    assert_eq!(
        samples, 1.0,
        "case {case:?}: the delivered invocation must publish exactly one {DURATION_SERIES} \
         measurement, got {samples} over {durations:?}"
    );
    let errors = datapoints(
        &router,
        case,
        ERRORS_SERIES,
        COUNT_UNIT,
        &function_dimensions(),
    )
    .await;
    assert!(
        errors.is_empty(),
        "case {case:?}: the delivered handler returned, so {ERRORS_SERIES} must hold no \
         datapoint, got {errors:?}"
    );
}
