// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what Lambda does with an asynchronous event once its deliveries to
//! the function are OVER: the invocation RECORD it hands to the configured destination, and
//! the event it writes to the configured dead-letter target.
//!
//! RULE: the completion of an accepted asynchronous invocation delivers an invocation record
//! to the destination its qualifier's asynchronous-invocation configuration names for the
//! outcome the event reached — `DestinationConfig$OnSuccess` for a handler that returned and
//! `DestinationConfig$OnFailure` for one that failed — and writes the event payload itself to
//! the `DeadLetterConfig$TargetArn` of an event that failed; each delivery is metered only
//! when it FAILED (`DestinationDeliveryFailures`, `DeadLetterErrors`), and the event's own
//! drop is metered whatever became of those deliveries (`AsyncEventsDropped`).
//!
//! Every row drives real HTTP requests through one router carrying Lambda, Amazon SQS and
//! CloudWatch on ONE engine, so a record is asserted where a client reads it — off the queue
//! the configuration names — and a metric where a client reads it: in the series CloudWatch
//! reports for the function. The function's package RUNS, so what each record reports about
//! the response is the handler's own answer.
//!
//! The rows differ along what the rule generalises over: the outcome the event reached (a
//! handler that returned, a handler that raised), which destination slot the configuration
//! states (both, one, the other), the form of the event payload (an object, an array, text
//! that is no document), and whether the destination the configuration names still EXISTS.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `PutFunctionEventInvokeConfig` is bound.
const EVENT_INVOKE_CONFIG_URI: &str = "/2019-09-25/functions";

/// The Region every call below is made in, and so the Region of every ARN it mints.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `MaximumRetryAttempts` every configuration below states: none, the floor of the
/// member's own `@range(min: 0, max: 2)`. So each event has exactly ONE delivery attempt,
/// which is what makes the `approximateInvokeCount` of a record a known number.
const NO_RETRIES: i64 = 0;

/// The request header `Invoke$InvocationType` is bound to, and the value that queues the
/// event instead of waiting for the run.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";
/// See [`INVOCATION_TYPE_HEADER`].
const EVENT_TYPE: &str = "Event";

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference), and the
/// names and units that reference gives the series under test.
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const DEAD_LETTER_ERRORS: &str = "DeadLetterErrors";
/// See [`NAMESPACE`].
const DESTINATION_DELIVERY_FAILURES: &str = "DestinationDeliveryFailures";
/// See [`NAMESPACE`].
const ASYNC_EVENTS_DROPPED: &str = "AsyncEventsDropped";
/// See [`NAMESPACE`].
const COUNT_UNIT: &str = "Count";
/// The dimension Lambda reports those series per.
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The `Period` every metric query reads at, in seconds: the one-minute granularity Lambda's
/// own series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a metric query's window reaches, in seconds.
const WINDOW_SECS: f64 = 600.0;

/// How long a row waits for work the CLOCK owes — an event's delivery attempt, the record or
/// the dead letter that follows it, the datapoint it publishes.
///
/// A ceiling on PATIENCE, never a span anything here counts on: every wait below looks again
/// until the thing it waits for is there or this deadline passes, so a slow or loaded host is
/// waited for and only something that never arrives at all fails a row.
const BUDGET: Duration = Duration::from_secs(120);

/// How long a row keeps looking at a destination that must stay EMPTY before it accepts that
/// nothing was delivered there.
///
/// Generous on purpose: it is far longer than the delivery of a record that WAS owed takes on
/// any host — every such delivery a row asserts has already arrived by the time this window
/// opens — so a wrong delivery would be seen inside it.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler every function below runs: it RAISES when its event asks it to and returns
/// otherwise, so which outcome a completion reached is the handler's own doing.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    if isinstance(event, dict):
        if event.get("fail"):
            raise RuntimeError("the handler refused " + str(event.get("seq")))
        return {"handled": event}
    raise RuntimeError("the handler refused " + repr(event))
"#;

/// One service mounted on one engine beside the two services a completion is observed
/// through: the queue a record or a dead letter is delivered to, and the series the delivery
/// is metered on.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One Amazon SNS call, in the `awsQuery` form that service's protocol binds: the action and
/// its parameters as a form-encoded body, written with the same form serializer the SDK's own
/// signer reads a query string with.
async fn sns(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> (u16, String) {
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", "2010-03-31");
    for (name, value) in parameters {
        form.append_pair(name, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/sns/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text one XML element of an answer carries, for the one value a row reads back out of
/// Amazon SNS's own answer document.
fn element(document: &str, name: &str) -> Option<String> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let from = document.find(&opening)? + opening.len();
    let to = document[from..].find(&closing)? + from;
    Some(document[from..to].to_string())
}

/// A topic of this account, as the service that owns it reports its ARN.
async fn given_topic(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = sns(router, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic {name:?} answered {status} {created}"
    );
    element(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"))
}

/// The ARN of a topic of this account that was never created, which is a target a publish is
/// made to and REFUSED by.
fn missing_topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT}:{name}")
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One Lambda control-plane or data-plane call, the way a client makes it.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the request body serializes")
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(
            builder
                .body(bytes::Bytes::from(payload))
                .expect("a well-formed request"),
        )
        .await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().clone(),
    }
}

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`).
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "sqs",
        "AmazonSQS",
        "application/x-amz-json-1.0",
        operation,
        body,
    )
    .await
}

/// One CloudWatch call.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "monitoring",
        "GraniteServiceVersion20100801",
        "application/x-amz-json-1.0",
        operation,
        body,
    )
    .await
}

/// One AWS JSON call to the service `signing_name` names.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    content_type: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/\
                 {signing_name}/aws4_request, SignedHeaders=host, Signature=not-verified"
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

/// A queue of this account, as the queue itself reports it: its url and its ARN.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let (status, created) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue {name:?} answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// The ARN of a queue of this account that was never created, which is a destination nothing
/// can be delivered to — the shape `queue_url` addresses one by, so the delivery is made and
/// REFUSED rather than never attempted.
fn missing_queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// Every message waiting on the queue `url` addresses, taken off it so a later look sees
/// only what arrived since.
async fn drained(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let (status, received) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {received}"
    );
    let mut bodies = Vec::new();
    for message in received["Messages"].as_array().cloned().unwrap_or_default() {
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            let (status, deleted) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMessage answered {status} {deleted}"
            );
        }
        bodies.push(json!({
            "body": message["Body"].as_str().unwrap_or_default(),
            "attributes": message["MessageAttributes"].clone(),
        }));
    }
    bodies
}

/// A function whose package RUNS [`HANDLER_SOURCE`], brought to `Active`.
async fn given_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    dead_letter: Option<&str>,
) {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": RUNTIME,
        "Handler": HANDLER,
        "Code": {
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER_SOURCE)),
        },
    });
    if let Some(target) = dead_letter {
        body["DeadLetterConfig"] = json!({ "TargetArn": target });
    }
    let created = lambda(router, "POST", FUNCTIONS_URI, &body, &[]).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateFunction {name:?} answered {} {}",
        created.status,
        created.text()
    );
    make_active(engine);
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes.
fn make_active(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = std::time::SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// `PutFunctionEventInvokeConfig` on the function's unpublished version, stating no retries
/// and the destination slots `on_success` and `on_failure` name.
async fn given_event_invoke_config(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    on_success: Option<&str>,
    on_failure: Option<&str>,
) {
    let mut destinations = json!({});
    if let Some(destination) = on_success {
        destinations["OnSuccess"] = json!({ "Destination": destination });
    }
    if let Some(destination) = on_failure {
        destinations["OnFailure"] = json!({ "Destination": destination });
    }
    let answer = lambda(
        router,
        "PUT",
        &format!("{EVENT_INVOKE_CONFIG_URI}/{function}/event-invoke-config"),
        &json!({
            "MaximumRetryAttempts": NO_RETRIES,
            "DestinationConfig": destinations,
        }),
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutFunctionEventInvokeConfig answered {} {}",
        answer.status,
        answer.text()
    );
}

/// One asynchronous invocation of `function` with `event`, which the accepting call answers
/// `202` for.
async fn invoke_async(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    event: &Value,
) {
    let answer = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        event,
        &[(INVOCATION_TYPE_HEADER, EVENT_TYPE)],
    )
    .await;
    assert_eq!(
        answer.status, 202,
        "case {case:?}: an asynchronous Invoke answered {} {}",
        answer.status,
        answer.text()
    );
}

/// Wait until every event accepted for `function` has SETTLED — no entry of its queue is
/// still waiting for an attempt or being delivered — which is what the completion's own
/// deliveries follow.
async fn settled(engine: &Arc<engine::InMemoryEngine>, case: &str, function: &str) {
    use service_com_amazonaws_lambda_awsgirapiservice::state::AsyncInvocationOutcome;

    let deadline = Instant::now() + BUDGET;
    loop {
        let outcomes: Vec<AsyncInvocationOutcome> = {
            let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
            let store = state.lock();
            store
                .functions
                .get(function)
                .map(|held| {
                    held.async_invocations
                        .iter()
                        .map(|entry| entry.outcome.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let waiting = outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
            )
        });
        if !outcomes.is_empty() && !waiting {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: every event accepted for {function:?} must settle within \
             {BUDGET:?}, got {outcomes:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The event payloads of every asynchronous invocation accepted for `function`, read out of
/// the service's own state because no modelled operation reports an accepted asynchronous
/// invocation back out.
fn accepted_payloads(engine: &Arc<engine::InMemoryEngine>, function: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|held| {
            held.async_invocations
                .iter()
                .map(|entry| String::from_utf8_lossy(&entry.payload).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The messages on the queue `url` addresses once there are at least `wanted` of them, or a
/// failure naming what was there instead.
async fn awaited(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    let mut held: Vec<Value> = Vec::new();
    loop {
        held.extend(drained(router, case, url).await);
        if held.len() >= wanted {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted} message(s) on {url} within {BUDGET:?}, got {held:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Keep looking at the queue `url` addresses until [`QUIET_WINDOW`] has passed, failing the
/// moment anything is delivered there.
async fn stayed_empty(router: &MultiServiceRouter, case: &str, what: &str, url: &str) {
    let deadline = Instant::now() + QUIET_WINDOW;
    while Instant::now() < deadline {
        let held = drained(router, case, url).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected no {what} on {url}, got {held:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `Datapoints` `GetMetricStatistics` answers for the series `metric` names on the
/// function `function`.
async fn datapoints(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    function: &str,
) -> Vec<Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum"],
            "Unit": COUNT_UNIT,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics({metric}, {function}) answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The `Sum` of every datapoint of that series, which is the count the window holds.
fn summed(points: &[Value]) -> f64 {
    points
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Wait until the series `metric` names on `function` sums to at least `wanted`.
async fn awaited_sum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    function: &str,
    wanted: f64,
) -> f64 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let points = datapoints(router, case, metric, function).await;
        let sum = summed(&points);
        if sum >= wanted {
            return sum;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {metric} of {function} to reach {wanted} within {BUDGET:?}, \
             got {sum} over {points:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Which destination slot a row configures, and which one must receive the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// `DestinationConfig$OnSuccess`.
    Success,
    /// `DestinationConfig$OnFailure`.
    Failure,
}

/// One row of the destination table.
struct Row {
    case: &'static str,
    /// The function the row drives, which carries its own pair of queues.
    function: &'static str,
    /// Whether the event asks the handler to raise.
    fails: bool,
    /// Whether the configuration states the on-success slot, and the on-failure slot.
    states: (bool, bool),
    /// The slot that must receive the record, and `None` for a completion whose outcome the
    /// configuration states no destination for.
    expect: Option<Slot>,
    /// The `requestContext.condition` the record must carry, for a row that gets one.
    condition: &'static str,
    /// The `responseContext.functionError` it must carry, for a row that gets one.
    function_error: Option<&'static str>,
}

/// RULE: a completed asynchronous invocation delivers its invocation record to the
/// destination configured for the outcome it reached, and to no other — so a configuration
/// that states no destination for that outcome delivers nothing at all.
///
/// The rows differ along the outcome the event reached and which slots the configuration
/// states; each row's two queues are its own, so a record that went to the wrong slot, or to
/// another row's destination, fails here.
#[tokio::test]
async fn a_completion_delivers_its_record_to_the_slot_its_outcome_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let rows: &[Row] = &[
        Row {
            case: "the handler returned, with both slots configured",
            function: "completed-returned",
            fails: false,
            states: (true, true),
            expect: Some(Slot::Success),
            condition: "Success",
            function_error: None,
        },
        Row {
            case: "the handler raised, with both slots configured",
            function: "completed-raised",
            fails: true,
            states: (true, true),
            expect: Some(Slot::Failure),
            condition: "RetriesExhausted",
            function_error: Some("Unhandled"),
        },
        Row {
            case: "the handler returned, with only the on-failure slot configured",
            function: "completed-returned-failure-only",
            fails: false,
            states: (false, true),
            expect: None,
            condition: "Success",
            function_error: None,
        },
        Row {
            case: "the handler raised, with only the on-success slot configured",
            function: "completed-raised-success-only",
            fails: true,
            states: (true, false),
            expect: None,
            condition: "RetriesExhausted",
            function_error: Some("Unhandled"),
        },
    ];

    // Each row's own pair of queues, and its own function and configuration.
    let mut queues: Vec<(String, String)> = Vec::new();
    for row in rows {
        let (success_url, success_arn) =
            given_queue(&router, row.case, &format!("{}-success", row.function)).await;
        let (failure_url, failure_arn) =
            given_queue(&router, row.case, &format!("{}-failure", row.function)).await;
        given_function(&router, &engine, row.case, row.function, None).await;
        given_event_invoke_config(
            &router,
            row.case,
            row.function,
            row.states.0.then_some(success_arn.as_str()),
            row.states.1.then_some(failure_arn.as_str()),
        )
        .await;
        queues.push((success_url, failure_url));
        invoke_async(
            &router,
            row.case,
            row.function,
            &json!({ "fail": row.fails, "seq": 0 }),
        )
        .await;
    }

    for (row, (success_url, failure_url)) in rows.iter().zip(&queues) {
        settled(&engine, row.case, row.function).await;
        let (wanted, other) = match row.expect {
            Some(Slot::Success) => (Some(success_url), failure_url),
            Some(Slot::Failure) => (Some(failure_url), success_url),
            None => (None, failure_url),
        };
        if let Some(url) = wanted {
            let held = awaited(&router, row.case, url, 1).await;
            assert_eq!(
                held.len(),
                1,
                "case {:?}: expected exactly one record, got {held:?}",
                row.case
            );
            let record: Value = serde_json::from_str(held[0]["body"].as_str().unwrap_or_default())
                .unwrap_or_else(|error| {
                    panic!("case {:?}: the record is not a document: {error}", row.case)
                });
            // (the member the record carries, the value it must carry)
            let stated: &[(&str, Value)] = &[
                ("/version", json!("1.0")),
                (
                    "/requestContext/functionArn",
                    json!(format!(
                        "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{}",
                        row.function
                    )),
                ),
                ("/requestContext/condition", json!(row.condition)),
                // One attempt, because every configuration above states no retries.
                ("/requestContext/approximateInvokeCount", json!(1)),
                (
                    "/requestPayload",
                    json!({ "fail": row.fails, "seq": 0 }),
                ),
                ("/responseContext/statusCode", json!(200)),
                ("/responseContext/executedVersion", json!("$LATEST")),
            ];
            for (pointer, expected) in stated {
                let read = record.pointer(pointer).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    read, *expected,
                    "case {:?}: expected {pointer} to be {expected}, got {read} in {record}",
                    row.case
                );
            }
            let reported = record
                .pointer("/responseContext/functionError")
                .and_then(Value::as_str);
            assert_eq!(
                reported, row.function_error,
                "case {:?}: expected functionError {:?}, got {reported:?} in {record}",
                row.case, row.function_error
            );
            assert!(
                record
                    .pointer("/requestContext/requestId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty()),
                "case {:?}: expected the record to name the invocation, got {record}",
                row.case
            );
            // The handler's OWN answer is what the record reports as the response payload:
            // the document it returned, or the error document the raise produced.
            let payload = record.pointer("/responsePayload").cloned().unwrap_or(Value::Null);
            if row.fails {
                assert!(
                    payload
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .is_some_and(|message| message.contains("the handler refused")),
                    "case {:?}: expected the handler's own error document, got {payload}",
                    row.case
                );
            } else {
                assert_eq!(
                    payload,
                    json!({ "handled": { "fail": false, "seq": 0 } }),
                    "case {:?}: expected the handler's own answer, got {payload}",
                    row.case
                );
            }
        }
        stayed_empty(&router, row.case, "record", other).await;
        if row.expect.is_none() {
            stayed_empty(&router, row.case, "record", success_url).await;
        }
    }
}

/// One row of the dead-letter table: the event a function is invoked with, and the body the
/// dead-letter queue must then hold.
struct DeadLetterRow {
    case: &'static str,
    event: &'static str,
    delivered: bool,
}

/// RULE: an asynchronous event the function did not process is written to the function's
/// `DeadLetterConfig$TargetArn` as the event payload ITSELF, byte for byte, while an event a
/// handler returned from is written nowhere; the write is metered only when it FAILED
/// (`DeadLetterErrors`), and the event's drop is metered either way
/// (`AsyncEventsDropped`).
///
/// The rows differ along the form of the event payload — an object, an array, text that is no
/// document — and along whether the handler failed for it; one further function names a
/// dead-letter target that does not exist, which is the case the failure metric exists for.
#[tokio::test]
async fn a_dropped_event_reaches_the_dead_letter_target_and_only_a_failed_write_is_metered() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let case = "a function whose dead-letter queue is held";
    let (queue_url, queue_arn) = given_queue(&router, case, "dead-letters").await;
    given_function(&router, &engine, case, "dlq-held", Some(&queue_arn)).await;
    // No destination of any kind, so the dead-letter target is the only thing a completion
    // owes anything to.
    given_event_invoke_config(&router, case, "dlq-held", None, None).await;

    let rows: &[DeadLetterRow] = &[
        DeadLetterRow {
            case: "an object the handler refuses",
            event: r#"{"fail":true,"seq":0}"#,
            delivered: true,
        },
        DeadLetterRow {
            case: "another object the handler refuses",
            event: r#"{"fail":true,"seq":1,"note":"second"}"#,
            delivered: true,
        },
        DeadLetterRow {
            case: "an array, which no handler of this package processes",
            event: r#"[1,2,3]"#,
            delivered: true,
        },
        DeadLetterRow {
            case: "a quoted string, which no handler of this package processes",
            event: r#""plain text""#,
            delivered: true,
        },
        DeadLetterRow {
            case: "an object the handler returns from",
            event: r#"{"fail":false,"seq":2}"#,
            delivered: false,
        },
    ];
    for row in rows {
        let event: Value = serde_json::from_str(row.event).expect("the row's event is a document");
        invoke_async(&router, row.case, "dlq-held", &event).await;
    }
    settled(&engine, case, "dlq-held").await;

    let wanted: Vec<&DeadLetterRow> = rows.iter().filter(|row| row.delivered).collect();
    let held = awaited(&router, case, &queue_url, wanted.len()).await;
    let bodies: Vec<String> = held
        .iter()
        .map(|message| message["body"].as_str().unwrap_or_default().to_string())
        .collect();
    for row in &wanted {
        assert!(
            bodies.iter().any(|body| body == row.event),
            "case {:?}: expected the event {:?} in the dead-letter queue, got {bodies:?}",
            row.case,
            row.event
        );
    }
    assert_eq!(
        bodies.len(),
        wanted.len(),
        "case {case:?}: expected exactly the dropped events in the dead-letter queue, got \
         {bodies:?}"
    );
    // Every dead letter names the invocation it came from, which is how an operator ties it
    // back to the run that produced it.
    for message in &held {
        let named = message
            .pointer("/attributes/RequestID/StringValue")
            .and_then(Value::as_str);
        assert!(
            named.is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected every dead letter to name its invocation, got {message}"
        );
    }
    // A target that TOOK every event meters no failure at all, however many events were
    // dropped to it.
    let errors = datapoints(&router, case, DEAD_LETTER_ERRORS, "dlq-held").await;
    assert!(
        errors.is_empty(),
        "case {case:?}: a dead-letter target that took every event meters no \
         {DEAD_LETTER_ERRORS}, got {errors:?}"
    );
    // Each dropped event is one `AsyncEventsDropped` datapoint, whatever became of the write.
    let dropped = awaited_sum(
        &router,
        case,
        ASYNC_EVENTS_DROPPED,
        "dlq-held",
        wanted.len() as f64,
    )
    .await;
    assert_eq!(
        dropped,
        wanted.len() as f64,
        "case {case:?}: expected {} dropped event(s), got {dropped}",
        wanted.len()
    );

    // The same function configuration against a target that does NOT exist: nothing is
    // delivered and the failed write is metered.
    let missing = "a function whose dead-letter queue was never created";
    let target = missing_queue_arn("dead-letters-missing");
    given_function(&router, &engine, missing, "dlq-missing", Some(&target)).await;
    given_event_invoke_config(&router, missing, "dlq-missing", None, None).await;
    invoke_async(
        &router,
        missing,
        "dlq-missing",
        &json!({ "fail": true, "seq": 0 }),
    )
    .await;
    settled(&engine, missing, "dlq-missing").await;
    let failures = awaited_sum(&router, missing, DEAD_LETTER_ERRORS, "dlq-missing", 1.0).await;
    assert_eq!(
        failures, 1.0,
        "case {missing:?}: expected one {DEAD_LETTER_ERRORS} datapoint, got {failures}"
    );
    // And the queue that IS held holds nothing of that function's event.
    stayed_empty(&router, missing, "dead letter", &queue_url).await;

    // A dead-letter target may also be an Amazon SNS TOPIC ("the ARN of an Amazon SQS queue
    // or Amazon SNS topic"), and the same rule decides the metric: the publish to a topic the
    // service HOLDS is accepted and metered as nothing, and the one to a topic it does not
    // hold is refused and metered as one failure.
    let topic_held = "a function whose dead-letter target is a topic that is held";
    let topic_arn = given_topic(&router, topic_held, "dead-letter-topic").await;
    given_function(&router, &engine, topic_held, "dlt-held", Some(&topic_arn)).await;
    given_event_invoke_config(&router, topic_held, "dlt-held", None, None).await;
    let topic_missing = "a function whose dead-letter topic was never created";
    let missing_topic = missing_topic_arn("dead-letter-topic-missing");
    given_function(&router, &engine, topic_missing, "dlt-missing", Some(&missing_topic)).await;
    given_event_invoke_config(&router, topic_missing, "dlt-missing", None, None).await;
    for (case, function) in [(topic_held, "dlt-held"), (topic_missing, "dlt-missing")] {
        invoke_async(&router, case, function, &json!({ "fail": true, "seq": 0 })).await;
    }
    let refused = awaited_sum(
        &router,
        topic_missing,
        DEAD_LETTER_ERRORS,
        "dlt-missing",
        1.0,
    )
    .await;
    assert_eq!(
        refused, 1.0,
        "case {topic_missing:?}: expected one {DEAD_LETTER_ERRORS} datapoint, got {refused}"
    );
    // The held topic's function reached the same point — its event is dropped — and meters no
    // delivery failure for a publish the service took.
    awaited_sum(
        &router,
        topic_held,
        ASYNC_EVENTS_DROPPED,
        "dlt-held",
        1.0,
    )
    .await;
    let taken = datapoints(&router, topic_held, DEAD_LETTER_ERRORS, "dlt-held").await;
    assert!(
        taken.is_empty(),
        "case {topic_held:?}: a topic that took the event meters no {DEAD_LETTER_ERRORS}, got \
         {taken:?}"
    );
}

/// RULE: an invocation record the destination did not accept is metered as one
/// `DestinationDeliveryFailures`, and one the destination took is metered as none — so a
/// datapoint on that series means a record really was lost.
#[tokio::test]
async fn only_a_record_the_destination_refused_is_metered_as_a_delivery_failure() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // (case, the function, the destination its on-failure slot names, whether the record
    // reaches it)
    let held_case = "a destination queue that is held";
    let (held_url, held_arn) = given_queue(&router, held_case, "records-held").await;
    let missing_case = "a destination queue that was never created";
    let missing_arn = missing_queue_arn("records-missing");
    let rows: [(&str, &str, &str, bool); 2] = [
        (held_case, "destination-held", held_arn.as_str(), true),
        (
            missing_case,
            "destination-missing",
            missing_arn.as_str(),
            false,
        ),
    ];
    for (case, function, destination, _delivered) in rows {
        given_function(&router, &engine, case, function, None).await;
        given_event_invoke_config(&router, case, function, None, Some(destination)).await;
        invoke_async(&router, case, function, &json!({ "fail": true, "seq": 0 })).await;
    }
    for (case, function, _destination, delivered) in rows {
        settled(&engine, case, function).await;
        if delivered {
            let records = awaited(&router, case, &held_url, 1).await;
            assert_eq!(
                records.len(),
                1,
                "case {case:?}: expected the record on the destination, got {records:?}"
            );
            let failures = datapoints(&router, case, DESTINATION_DELIVERY_FAILURES, function).await;
            assert!(
                failures.is_empty(),
                "case {case:?}: a destination that took the record meters no \
                 {DESTINATION_DELIVERY_FAILURES}, got {failures:?}"
            );
        } else {
            let failures =
                awaited_sum(&router, case, DESTINATION_DELIVERY_FAILURES, function, 1.0).await;
            assert_eq!(
                failures, 1.0,
                "case {case:?}: expected one {DESTINATION_DELIVERY_FAILURES} datapoint, got \
                 {failures}"
            );
            stayed_empty(&router, case, "record", &held_url).await;
        }
    }
}

/// RULE: a destination of ANOTHER service's kind is delivered to with that service's own
/// write — a Lambda function destination is invoked ASYNCHRONOUSLY with the record as its
/// event — and a destination naming the function that just COMPLETED is withheld, because
/// delivering to it would queue another event for the same function for every event it
/// completes.
#[tokio::test]
async fn a_function_destination_is_invoked_unless_it_names_the_function_itself() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let other = "a destination naming another function";
    let itself = "a destination naming the function that completed";
    for (case, function) in [(other, "function-destination-source"), (itself, "function-destination-self")] {
        given_function(&router, &engine, case, function, None).await;
    }
    given_function(&router, &engine, other, "function-destination-sink", None).await;
    let sink_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:function-destination-sink");
    let own_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:function-destination-self");
    given_event_invoke_config(
        &router,
        other,
        "function-destination-source",
        None,
        Some(&sink_arn),
    )
    .await;
    given_event_invoke_config(
        &router,
        itself,
        "function-destination-self",
        None,
        Some(&own_arn),
    )
    .await;
    invoke_async(
        &router,
        other,
        "function-destination-source",
        &json!({ "fail": true, "seq": 0 }),
    )
    .await;
    invoke_async(
        &router,
        itself,
        "function-destination-self",
        &json!({ "fail": true, "seq": 0 }),
    )
    .await;

    // The sink is INVOKED with the record: the event it was accepted for is the record
    // document, naming the function whose invocation produced it.
    settled(&engine, other, "function-destination-source").await;
    let deadline = Instant::now() + BUDGET;
    let delivered = loop {
        let payloads = accepted_payloads(&engine, "function-destination-sink");
        if !payloads.is_empty() {
            break payloads;
        }
        assert!(
            Instant::now() < deadline,
            "case {other:?}: expected the record delivered to the sink function within {BUDGET:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    };
    assert_eq!(
        delivered.len(),
        1,
        "case {other:?}: expected exactly one record delivered, got {delivered:?}"
    );
    let record: Value = serde_json::from_str(&delivered[0]).unwrap_or_else(|error| {
        panic!("case {other:?}: the delivered event is not a record: {error}: {delivered:?}")
    });
    assert_eq!(
        record.pointer("/requestContext/functionArn").and_then(Value::as_str),
        Some(
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:function-destination-source")
                .as_str()
        ),
        "case {other:?}: expected the record of the source function's invocation, got {record}"
    );
    let failures = datapoints(
        &router,
        other,
        DESTINATION_DELIVERY_FAILURES,
        "function-destination-source",
    )
    .await;
    assert!(
        failures.is_empty(),
        "case {other:?}: a destination that took the record meters no \
         {DESTINATION_DELIVERY_FAILURES}, got {failures:?}"
    );

    // The function that is its own destination is delivered NOTHING: its queue still holds
    // only the one event the test accepted, and the withheld delivery is metered.
    settled(&engine, itself, "function-destination-self").await;
    let failures = awaited_sum(
        &router,
        itself,
        DESTINATION_DELIVERY_FAILURES,
        "function-destination-self",
        1.0,
    )
    .await;
    assert_eq!(
        failures, 1.0,
        "case {itself:?}: expected one {DESTINATION_DELIVERY_FAILURES} datapoint, got {failures}"
    );
    let quiet = Instant::now() + QUIET_WINDOW;
    while Instant::now() < quiet {
        let payloads = accepted_payloads(&engine, "function-destination-self");
        assert_eq!(
            payloads.len(),
            1,
            "case {itself:?}: expected the function's queue to hold only the event the test \
             accepted, got {payloads:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
