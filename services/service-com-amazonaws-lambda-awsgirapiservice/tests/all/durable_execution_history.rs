// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDurableExecutionHistory`: "Retrieves the execution history
//! for a durable execution, showing all the steps, callbacks, and events that occurred
//! during the execution."
//!
//! The rule under test: the operation reports, for the one durable execution its
//! `DurableExecutionArn` addresses, exactly the events that execution recorded — each one
//! as it was recorded, by the call or the clock transition that made it happen — in the
//! order they happened (reversed when `ReverseOrder` asks for it), cut into pages of at
//! most `MaxItems` events whose `NextMarker` is present only while events remain, with
//! each event's payload data left out when `IncludeExecutionData` is false. An ARN naming
//! no execution this emulator holds is refused rather than answered with an empty
//! history.
//!
//! Every assertion is over real HTTP requests through the router, so an event no call
//! recorded fails here. The rows differ along what the rule generalises over: how the
//! function is configured (with a `DurableConfig` and at each published `ExecutionTimeout`
//! bound, and without one at all), the payload the execution started from, how many
//! events the execution recorded, the direction asked for, the page size (one, several,
//! every event, more than there are, the documented default and the published maximum)
//! and the marker a page continues from, whether execution data is asked for, and which
//! part of a well-formed ARN names nothing.
//!
//! The clock-driven half (a running execution whose `ExecutionTimeout` has fallen due) is
//! exercised by moving the stored due instant into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::ops::types::{
    CallbackFailedDetails, ErrorObject, Event, EventError, EventResult, RetryDetails,
    StepSucceededDetails,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, DurableExecutionStart,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, send, zip_file,
    Answer, ACCOUNT, REGION, ROLE,
};

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The request header `Invoke$DurableExecutionName` is bound to.
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which a durable
/// function here names in its `DurableConfig`.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// The unpublished version label, as the model's `Version` pattern spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The bounds the model puts on `DurableConfig$ExecutionTimeout` (`@range(min: 1, max:
/// 31622400)`), from `smithy_query(query="shape", shape="DurableConfig")`.
const EXECUTION_TIMEOUT_MIN: i64 = 1;
const EXECUTION_TIMEOUT_MAX: i64 = 31_622_400;

/// The bounds the model puts on this operation's `MaxItems` (`@range(min: 0, max: 1000)`),
/// from `smithy_query(query="operation", operation="GetDurableExecutionHistory")`.
const MAX_ITEMS_MAX: i64 = 1_000;

/// The events one page reports when the caller names no `MaxItems`, which the member's own
/// documentation states: "The default is 100 and the maximum allowed is 1000. A value of 0
/// uses the default."
const DEFAULT_PAGE_SIZE: usize = 100;

/// The model's `EventType` values this emulator records, from
/// `smithy_query(query="shape", shape="EventType")`.
const EVENT_EXECUTION_STARTED: &str = "ExecutionStarted";
const EVENT_EXECUTION_TIMED_OUT: &str = "ExecutionTimedOut";
const EVENT_STEP_SUCCEEDED: &str = "StepSucceeded";
const EVENT_CALLBACK_FAILED: &str = "CallbackFailed";

/// `GetDurableExecutionHistory` on the execution `arn` addresses, with `query` appended
/// (`GET /2025-12-01/durable-executions/{DurableExecutionArn}/history`). The label is not
/// greedy, so the slashes the ARN carries are percent-encoded in the path, exactly as an
/// SDK encodes them.
async fn history(router: &MultiServiceRouter, arn: &str, query: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}/history{query}",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// `GetDurableExecution` on the same ARN, the implemented read the history is checked
/// against.
async fn get_execution(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2025-12-01/durable-executions/{}", arn.replace('/', "%2F")),
        &Value::Null,
    )
    .await
}

/// `Invoke` on `reference`, naming `execution` as the durable execution and carrying
/// `payload`.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    execution: &str,
    payload: &[u8],
) -> Answer {
    send(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{reference}/invocations"),
        "application/octet-stream",
        payload.to_vec(),
        &[
            ("x-amz-invocation-type", "Event".to_string()),
            (DURABLE_EXECUTION_NAME_HEADER, execution.to_string()),
        ],
    )
    .await
}

/// The events an answer reported, in the order it reported them.
fn events(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `EventType` of every event an answer reported, in order.
fn event_types(answer: &Answer) -> Vec<String> {
    events(answer)
        .iter()
        .map(|event| {
            event
                .get("EventType")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `EventId` of every event an answer reported, in order.
fn event_ids(answer: &Answer) -> Vec<i64> {
    events(answer)
        .iter()
        .map(|event| event.get("EventId").and_then(Value::as_i64).unwrap_or(-1))
        .collect()
}

/// The `NextMarker` an answer carries, when it carries one.
fn next_marker(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("NextMarker")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The refusal sentence an answer carries, under either spelling the model gives the
/// member.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// A member of one reported event, reached by a dotted path.
fn at<'a>(event: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = event;
    for step in path.split('.') {
        current = current.get(step)?;
    }
    Some(current)
}

/// Create the function `name`, bringing it to the state an invocation needs. `timeout`
/// gives it the `DurableConfig` that makes it a durable function, with that
/// `ExecutionTimeout`; `None` leaves it a plain function.
async fn create_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    timeout: Option<i64>,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(seconds) = timeout {
        body["DurableConfig"] = json!({ "KMSKeyArn": KMS_KEY, "ExecutionTimeout": seconds });
    }
    let created = create_function(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// Start one durable execution of `function` at `started`, through the state the `Invoke`
/// handler registers one in, answering the ARN it is addressed by.
fn start_execution(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    name: &str,
    payload: &[u8],
    started: SystemTime,
) -> String {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let record = store
        .functions
        .get_mut(function)
        .unwrap_or_else(|| panic!("case {name}: the store must hold the function {function}"));
    match record.start_durable_execution(name, VERSION_LATEST, payload, started) {
        DurableExecutionStart::Started(arn) => arn,
        other => panic!("case {name}: starting the execution must answer its ARN, got {other:?}"),
    }
}

/// Append one event of `event_type` at `at` to a stored execution's history, filled in by
/// `fill`.
///
/// This stands in for the operations that will record such an event once they are
/// implemented (a step completing, a callback failing): the rule under test is what the
/// read reports for a RECORDED event, whichever call recorded it, so the fixture records
/// events of several types rather than only the two this emulator writes today.
fn append_event(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    name: &str,
    event_type: &str,
    at: SystemTime,
    fill: impl FnOnce(&mut Event),
) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let execution = store
        .functions
        .get_mut(function)
        .and_then(|record| record.durable_executions.get_mut(name))
        .unwrap_or_else(|| panic!("case {name}: the function must hold the execution"));
    let mut event = execution.next_event(event_type, at);
    fill(&mut event);
    execution.history.push(event);
}

/// The whole-second instant `seconds` ago, which is when a fixture's execution started:
/// whole seconds so the timestamps a response carries compare exactly, and in the past so
/// the clock has already owed whatever that execution's timeout owes.
fn instant_secs_ago(seconds: u64) -> SystemTime {
    let epoch_seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock must be past the epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds - seconds)
}

/// One row of the recorded-history table: how the function is configured, the payload the
/// execution starts from, how long before now it started, and the events the history must
/// therefore report.
struct HistoryCase {
    case: &'static str,
    timeout: Option<i64>,
    payload: &'static [u8],
    started_secs_ago: u64,
    types: Vec<&'static str>,
}

impl HistoryCase {
    /// Whether this row's `ExecutionTimeout` has already fallen due at the instant the
    /// history is read, which is the transition the clock owes the execution.
    fn timed_out(&self) -> bool {
        self.timeout
            .is_some_and(|timeout| timeout < i64::try_from(self.started_secs_ago).unwrap_or(0))
    }
}

/// The rows: a durable function at each published `ExecutionTimeout` bound and a plain one
/// with no `DurableConfig` at all, started from an empty payload, a JSON object and a JSON
/// array, read while still running and after the configured timeout fell due.
fn history_cases() -> Vec<HistoryCase> {
    vec![
        HistoryCase {
            case: "running, a JSON object payload, the timeout the function configures",
            timeout: Some(3_600),
            payload: br#"{"order":"a-1"}"#,
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED],
        },
        HistoryCase {
            case: "running, no payload at all",
            timeout: Some(3_600),
            payload: b"",
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED],
        },
        HistoryCase {
            case: "running, the published maximum ExecutionTimeout",
            timeout: Some(EXECUTION_TIMEOUT_MAX),
            payload: br#"{"nested":{"deep":true}}"#,
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED],
        },
        HistoryCase {
            case: "a function with no DurableConfig, so the execution runs under no timeout",
            timeout: None,
            payload: br#"{"plain":true}"#,
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED],
        },
        HistoryCase {
            case: "the published minimum ExecutionTimeout, long since fallen due",
            timeout: Some(EXECUTION_TIMEOUT_MIN),
            payload: b"[]",
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED, EVENT_EXECUTION_TIMED_OUT],
        },
        HistoryCase {
            case: "an ExecutionTimeout that fell due after the execution ran a while",
            timeout: Some(60),
            payload: br#"{"order":"b-2"}"#,
            started_secs_ago: 600,
            types: vec![EVENT_EXECUTION_STARTED, EVENT_EXECUTION_TIMED_OUT],
        },
    ]
}

#[tokio::test]
async fn the_history_reports_every_event_the_execution_recorded() {
    for case in history_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-history";
        create_durable_function(&router, &engine, function, case.timeout).await;

        let name = "recorded";
        let started = instant_secs_ago(case.started_secs_ago);
        let arn = start_execution(&engine, function, name, case.payload, started);

        let answer = history(&router, &arn, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: the history of a stored execution must answer 200, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            event_types(&answer),
            case.types,
            "case {:?}: expected the events {:?}, got {:?}",
            case.case,
            case.types,
            event_types(&answer)
        );
        // "The unique identifier for this event. Event IDs increment sequentially."
        let expected_ids: Vec<i64> = (1..=case.types.len() as i64).collect();
        assert_eq!(
            event_ids(&answer),
            expected_ids,
            "case {:?}: expected the event ids {expected_ids:?}, got {:?}",
            case.case,
            event_ids(&answer)
        );
        assert_eq!(
            next_marker(&answer),
            None,
            "case {:?}: a page holding every event must carry no NextMarker, got {:?}",
            case.case,
            next_marker(&answer)
        );

        // The opening event carries "the input payload provided for the durable
        // execution" and "the maximum amount of time that the durable execution is
        // allowed to run, in seconds" — the function's own `ExecutionTimeout`, or none at
        // all for a function whose configuration names none.
        let reported = events(&answer);
        let started = reported
            .first()
            .unwrap_or_else(|| panic!("case {:?}: the history must open with an event", case.case));
        let expected_payload = String::from_utf8_lossy(case.payload).to_string();
        let payload = at(started, "ExecutionStartedDetails.Input.Payload")
            .and_then(Value::as_str)
            .map(str::to_string);
        if expected_payload.is_empty() {
            assert_eq!(
                payload, None,
                "case {:?}: an execution started from no payload must report none, got {payload:?}",
                case.case
            );
        } else {
            assert_eq!(
                payload.as_deref(),
                Some(expected_payload.as_str()),
                "case {:?}: expected the stored payload {expected_payload:?}, got {payload:?}",
                case.case
            );
        }
        let timeout = at(started, "ExecutionStartedDetails.ExecutionTimeout").and_then(Value::as_i64);
        assert_eq!(
            timeout,
            Some(case.timeout.unwrap_or(0)),
            "case {:?}: expected the configured ExecutionTimeout {:?}, got {timeout:?}",
            case.case,
            case.timeout.unwrap_or(0)
        );

        // Every event is timed at the instant it happened, which is what the execution
        // itself reports for the same two moments.
        let read_back = get_execution(&router, &arn).await;
        assert_eq!(
            read_back.status,
            200,
            "case {:?}: GetDurableExecution must answer 200, got {} {}",
            case.case,
            read_back.status,
            String::from_utf8_lossy(&read_back.body)
        );
        let start_timestamp = read_back.json().get("StartTimestamp").and_then(Value::as_f64);
        let started_at = started.get("EventTimestamp").and_then(Value::as_f64);
        assert_eq!(
            started_at, start_timestamp,
            "case {:?}: the opening event must be timed at the instant the execution \
             started ({start_timestamp:?}), got {started_at:?}",
            case.case
        );
        if case.timed_out() {
            let end_timestamp = read_back.json().get("EndTimestamp").and_then(Value::as_f64);
            let closed_at = reported
                .get(1)
                .and_then(|event| event.get("EventTimestamp"))
                .and_then(Value::as_f64);
            assert_eq!(
                closed_at, end_timestamp,
                "case {:?}: the timeout event must be timed at the instant the timeout fell \
                 due ({end_timestamp:?}), got {closed_at:?}",
                case.case
            );
            let status = read_back.string("Status");
            assert_eq!(
                status.as_deref(),
                Some("TIMED_OUT"),
                "case {:?}: the execution the timeout closed must report TIMED_OUT, got \
                 {status:?}",
                case.case
            );
        }

        // Reading the history changes nothing a later caller sees.
        let again = history(&router, &arn, "").await;
        assert_eq!(
            again.json(),
            answer.json(),
            "case {:?}: a second read must report the same history, got {}",
            case.case,
            String::from_utf8_lossy(&again.body)
        );
    }
}

#[tokio::test]
async fn an_invocation_records_the_history_of_the_execution_it_starts() {
    // The payloads differ along what the rule generalises over: an empty body, a JSON
    // object, a JSON array and a scalar.
    for (case, payload) in [
        ("no payload", &b""[..]),
        ("a JSON object", br#"{"customer":"c-9"}"#),
        ("a JSON array", br#"[1,2,3]"#),
        ("a JSON scalar", br#""plain-text""#),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-invoked";
        create_durable_function(&router, &engine, function, Some(3_600)).await;

        let name = "started-by-invoke";
        let accepted = invoke(&router, function, name, payload).await;
        assert_eq!(
            accepted.status,
            202,
            "case {case:?}: an accepted asynchronous invocation must answer 202, got {} {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        let arn = accepted
            .header(DURABLE_EXECUTION_ARN_HEADER)
            .unwrap_or_default();
        assert!(
            !arn.is_empty(),
            "case {case:?}: the invocation must answer the started execution's ARN"
        );

        let answer = history(&router, &arn, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the history of the started execution must answer 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            event_types(&answer),
            vec![EVENT_EXECUTION_STARTED.to_string()],
            "case {case:?}: a freshly started execution must have recorded exactly its \
             start, got {:?}",
            event_types(&answer)
        );
        let reported = events(&answer);
        let started = reported
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the history must open with an event"));
        let expected = String::from_utf8_lossy(payload).to_string();
        let reported_payload = at(started, "ExecutionStartedDetails.Input.Payload")
            .and_then(Value::as_str)
            .map(str::to_string);
        let expected_payload = (!expected.is_empty()).then_some(expected.clone());
        assert_eq!(
            reported_payload, expected_payload,
            "case {case:?}: expected the payload the invocation carried \
             {expected_payload:?}, got {reported_payload:?}"
        );
    }
}

/// How many events [`seeded_history`] records, and the type of each.
fn seeded_types() -> Vec<String> {
    vec![
        EVENT_EXECUTION_STARTED.to_string(),
        EVENT_STEP_SUCCEEDED.to_string(),
        EVENT_CALLBACK_FAILED.to_string(),
        EVENT_EXECUTION_TIMED_OUT.to_string(),
    ]
}

/// The payload the seeded step records as its result.
const STEP_RESULT: &str = r#"{"charged":true}"#;

/// The payload the seeded execution starts from.
const SEEDED_INPUT: &str = r#"{"order":"c-3"}"#;

/// The error message the seeded callback records.
const CALLBACK_ERROR: &str = "the callback was not resolved before its timeout";

/// The error type the seeded callback records beside that message, inside the same error
/// payload.
const CALLBACK_ERROR_TYPE: &str = "CallbackTimeout";

/// The `ExecutionTimeout` the seeded function configures, which falls due after the step
/// and the callback the fixture records.
const SEEDED_TIMEOUT: i64 = 60;

/// A function holding one execution whose history records four events of four types: the
/// start, a step that succeeded, a callback that failed, and the timeout the clock owed.
async fn seeded_history() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter, String) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-paged";
    create_durable_function(&router, &engine, function, Some(SEEDED_TIMEOUT)).await;

    // The execution started long enough ago that its `ExecutionTimeout` has already fallen
    // due, so the closing event is one the clock owed rather than one anybody wrote.
    let base = instant_secs_ago(600);
    let name = "paged";
    let arn = start_execution(&engine, function, name, SEEDED_INPUT.as_bytes(), base);
    append_event(
        &engine,
        function,
        name,
        EVENT_STEP_SUCCEEDED,
        base + Duration::from_secs(10),
        |event| {
            event.id = Some("charge".to_string());
            event.step_succeeded_details = Some(StepSucceededDetails {
                result: EventResult {
                    payload: Some(STEP_RESULT.to_string()),
                    truncated: Some(false),
                },
                retry_details: RetryDetails {
                    current_attempt: 1,
                    next_attempt_delay_seconds: None,
                },
            });
        },
    );
    append_event(
        &engine,
        function,
        name,
        EVENT_CALLBACK_FAILED,
        base + Duration::from_secs(20),
        |event| {
            event.id = Some("await-shipment".to_string());
            event.callback_failed_details = Some(CallbackFailedDetails {
                error: EventError {
                    payload: Some(ErrorObject {
                        error_message: Some(CALLBACK_ERROR.to_string()),
                        error_type: Some(CALLBACK_ERROR_TYPE.to_string()),
                        error_data: None,
                        stack_trace: None,
                    }),
                    truncated: Some(false),
                },
            });
        },
    );
    // The first read materialises the timeout the clock owed, which is what appends the
    // closing event; every page below therefore sees the same four events.
    let settled = history(&router, &arn, "").await;
    assert_eq!(
        settled.status,
        200,
        "the seeded history must read back, got {} {}",
        settled.status,
        String::from_utf8_lossy(&settled.body)
    );
    (engine, router, arn)
}

/// One row of the paging table: the query it sends, the event ids the page must report and
/// the marker it must answer.
struct PageCase {
    case: &'static str,
    query: &'static str,
    ids: Vec<i64>,
    marker: Option<&'static str>,
}

#[tokio::test]
async fn the_history_pages_over_every_event_without_losing_one() {
    let (_engine, router, arn) = seeded_history().await;
    let every: Vec<i64> = (1..=4).collect();
    let cases = vec![
        PageCase {
            case: "no MaxItems at all, so the documented default holds every event",
            query: "",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "MaxItems=0, which the member documents as the default",
            query: "?MaxItems=0",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "one event per page",
            query: "?MaxItems=1",
            ids: vec![1],
            marker: Some("1"),
        },
        PageCase {
            case: "two events per page",
            query: "?MaxItems=2",
            ids: vec![1, 2],
            marker: Some("2"),
        },
        PageCase {
            case: "a page as large as the history",
            query: "?MaxItems=4",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "a page larger than the history",
            query: "?MaxItems=50",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "the published maximum page size",
            query: "?MaxItems=1000",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "reverse chronological order",
            query: "?ReverseOrder=true",
            ids: vec![4, 3, 2, 1],
            marker: None,
        },
        PageCase {
            case: "chronological order asked for explicitly",
            query: "?ReverseOrder=false",
            ids: every.clone(),
            marker: None,
        },
        PageCase {
            case: "the newest two events first",
            query: "?ReverseOrder=true&MaxItems=2",
            ids: vec![4, 3],
            marker: Some("3"),
        },
        PageCase {
            case: "continuing after the second event",
            query: "?MaxItems=2&Marker=2",
            ids: vec![3, 4],
            marker: None,
        },
        PageCase {
            case: "continuing after the third event, one at a time",
            query: "?MaxItems=1&Marker=3",
            ids: vec![4],
            marker: None,
        },
        PageCase {
            case: "continuing after the last event there is",
            query: "?Marker=4",
            ids: Vec::new(),
            marker: None,
        },
        PageCase {
            case: "continuing after the newest event, in reverse order",
            query: "?ReverseOrder=true&Marker=3",
            ids: vec![2, 1],
            marker: None,
        },
    ];

    for case in cases {
        let answer = history(&router, &arn, case.query).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected 200, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            event_ids(&answer),
            case.ids,
            "case {:?}: expected the event ids {:?}, got {:?}",
            case.case,
            case.ids,
            event_ids(&answer)
        );
        assert_eq!(
            next_marker(&answer).as_deref(),
            case.marker,
            "case {:?}: expected the NextMarker {:?}, got {:?}",
            case.case,
            case.marker,
            next_marker(&answer)
        );
    }

    // Walking the pages one event at a time reports every event exactly once, in order,
    // and stops: the invariant the marker exists for.
    let mut walked: Vec<i64> = Vec::new();
    let mut query = "?MaxItems=1".to_string();
    for _ in 0..=seeded_types().len() {
        let answer = history(&router, &arn, &query).await;
        assert_eq!(
            answer.status,
            200,
            "case \"walk\": expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        walked.extend(event_ids(&answer));
        match next_marker(&answer) {
            Some(marker) => query = format!("?MaxItems=1&Marker={marker}"),
            None => break,
        }
    }
    assert_eq!(
        walked, every,
        "case \"walk\": paging one event at a time must report every event exactly once, \
         got {walked:?}"
    );
    assert!(
        DEFAULT_PAGE_SIZE >= every.len(),
        "case \"walk\": the fixture must be smaller than the default page size, got \
         {DEFAULT_PAGE_SIZE}"
    );
}

#[tokio::test]
async fn the_history_refuses_a_page_it_cannot_cut() {
    let (_engine, router, arn) = seeded_history().await;
    // The rows: a page size one past each published bound of `MaxItems` (`@range(min: 0,
    // max: 1000)`), and a marker naming no event of this execution.
    for (case, query, invalid_marker) in [
        (
            "one past the published maximum page size",
            "?MaxItems=1001".to_string(),
            false,
        ),
        (
            "one below the published minimum page size",
            "?MaxItems=-1".to_string(),
            false,
        ),
        (
            "a marker naming an event id the execution has not reached",
            "?Marker=99".to_string(),
            true,
        ),
        (
            "a marker that is not an event id at all",
            "?Marker=not-an-id".to_string(),
            true,
        ),
        (
            "a marker far past every event the execution could have recorded",
            format!("?Marker={}", MAX_ITEMS_MAX + 1),
            true,
        ),
    ] {
        let answer = history(&router, &arn, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected the refusal of a request no page satisfies, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            events(&answer).is_empty(),
            "case {case:?}: a refusal must report no events, got {:?}",
            events(&answer)
        );
        if invalid_marker {
            assert!(
                answer
                    .error_code()
                    .contains("InvalidParameterValueException"),
                "case {case:?}: expected the modelled InvalidParameterValueException, got {:?}",
                answer.error_code()
            );
            // A marker is a position this listing handed out, so a value it never issued
            // is refused with the one fixed sentence this service words for that — it
            // carries nothing of the request.
            let expected = "Invalid marker value from user.";
            assert_eq!(
                message(&answer),
                expected,
                "case {case:?}: expected the refusal {expected:?}, got {:?}",
                message(&answer)
            );
        }
    }
}

#[tokio::test]
async fn the_history_leaves_out_execution_data_when_asked() {
    let (_engine, router, arn) = seeded_history().await;
    // "Set to true to include data, or false to exclude it for a more compact response.
    // The default is true."
    for (case, query, data) in [
        ("no IncludeExecutionData at all, so the documented default holds", "", true),
        ("execution data asked for", "?IncludeExecutionData=true", true),
        ("execution data excluded", "?IncludeExecutionData=false", false),
    ] {
        let answer = history(&router, &arn, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            event_types(&answer),
            seeded_types(),
            "case {case:?}: every recorded event must still be reported, got {:?}",
            event_types(&answer)
        );
        let reported = events(&answer);
        // The execution's data — its input, a step's result and a callback's error
        // payload, every part of that error object included — each reported only when the
        // data is asked for.
        for (path, expected) in [
            ("ExecutionStartedDetails.Input.Payload", SEEDED_INPUT),
            ("StepSucceededDetails.Result.Payload", STEP_RESULT),
            (
                "CallbackFailedDetails.Error.Payload.ErrorMessage",
                CALLBACK_ERROR,
            ),
            (
                "CallbackFailedDetails.Error.Payload.ErrorType",
                CALLBACK_ERROR_TYPE,
            ),
        ] {
            let found = reported
                .iter()
                .filter_map(|event| at(event, path))
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<String>>();
            if data {
                assert_eq!(
                    found,
                    vec![expected.to_string()],
                    "case {case:?}: expected {path} to report the stored payload, got {found:?}"
                );
            } else {
                assert!(
                    found.is_empty(),
                    "case {case:?}: expected {path} to be left out, got {found:?}"
                );
            }
        }
        // Everything that is not payload data stays, whichever way the request asks:
        // excluding data makes the response more compact, not empty.
        for (path, expected) in [
            (
                "ExecutionStartedDetails.ExecutionTimeout",
                json!(SEEDED_TIMEOUT),
            ),
            ("StepSucceededDetails.RetryDetails.CurrentAttempt", json!(1)),
            ("CallbackFailedDetails.Error.Truncated", json!(false)),
            ("Id", json!("charge")),
        ] {
            let found = reported
                .iter()
                .filter_map(|event| at(event, path))
                .cloned()
                .collect::<Vec<Value>>();
            assert!(
                found.contains(&expected),
                "case {case:?}: expected {path} to report {expected} whatever the request \
                 asks of the data, got {found:?}"
            );
        }
        for event in &reported {
            assert!(
                event.get("EventId").is_some() && event.get("EventTimestamp").is_some(),
                "case {case:?}: every event must keep its id and its timestamp, got {event}"
            );
        }
    }
}

#[tokio::test]
async fn the_history_refuses_an_execution_it_does_not_hold() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-missing";
    create_durable_function(&router, &engine, function, Some(3_600)).await;
    let base = instant_secs_ago(600);
    let held = start_execution(&engine, function, "held", b"{}", base);

    // The rows: each part of a well-formed `DurableExecutionArn` pointed at something this
    // emulator does not hold — the function, the version, the execution's name and its id.
    let cases = vec![
        (
            "a function this emulator does not hold",
            held.replace(&format!("function:{function}:"), "function:no-such-function:"),
        ),
        (
            "a version of the function nobody published",
            held.replace(":$LATEST/", ":7/"),
        ),
        (
            "an execution name the function never started",
            held.replace("/held/", "/never-started/"),
        ),
        (
            "an execution id this emulator never minted",
            format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:{VERSION_LATEST}\
                 /durable-execution/held/0123456789abcdef0123456789abcdef"
            ),
        ),
    ];

    for (case, arn) in cases {
        let answer = history(&router, &arn, "").await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: an ARN naming no execution must be refused, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            message(&answer),
            format!("Durable execution not found: {arn}"),
            "case {case:?}: expected the refusal to name the execution the request asked \
             about, got {:?}",
            message(&answer)
        );
        assert!(
            events(&answer).is_empty(),
            "case {case:?}: a refusal must report no events, got {:?}",
            events(&answer)
        );
    }

    // The execution the fixture does hold still reads back, so the refusals above are
    // about the ARN rather than about the store.
    let answer = history(&router, &held, "").await;
    assert_eq!(
        answer.status,
        200,
        "case \"the execution the function holds\": expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}
