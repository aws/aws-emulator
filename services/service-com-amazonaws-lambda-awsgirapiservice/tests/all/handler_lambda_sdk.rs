// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AWS LAMBDA surface a running handler reaches through the SDK its
//! runtime ships, and for the request CHAIN the calls it makes belong to.
//!
//! RULE: the AWS SDK a managed runtime supplies reaches AWS Lambda itself, so a handler's
//! `boto3.client("lambda")` call is served by the Lambda surface of this same emulator — an
//! invocation of another function or of itself included — and every such call carries the
//! INVOCATION'S OWN tracing context, which is what makes the hops of one request chain
//! recognisable as one chain. Lambda's recursive-loop guardrail is what reads that chain:
//! with `RecursiveLoop=Terminate` it "stops your function being invoked" once it detects the
//! loop and meters the drop, and with `Allow` it "doesn't take any action", so the chain runs
//! to whatever ceiling the function imposes on itself.
//!
//! Every row drives real HTTP requests through the router, against functions whose packages
//! RUN, and every answer a row asserts is the emulated service's own: a handler that could
//! not reach Lambda at all raises inside the run and the row fails.
//!
//! The rows differ along what the rule generalises over: the operation the handler calls
//! (an invocation, a configuration read, a listing), the invocation type it calls with
//! (synchronous, the queued event), the function it addresses (another one, itself), and the
//! `RecursiveLoop` the function is configured with.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState,
    MAX_REQUEST_CHAIN_INVOCATIONS,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `PutFunctionRecursionConfig` is bound.
const RECURSION_URI: &str = "/2024-08-31/functions";

/// The Region every call below is made in.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The request header `Invoke$InvocationType` is bound to, and its two values below.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";
/// See [`INVOCATION_TYPE_HEADER`].
const EVENT_TYPE: &str = "Event";

/// The namespace Lambda's own metrics live in, the series the guardrail's drop is metered on,
/// and the dimension it is reported per.
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const RECURSIVE_INVOCATIONS_DROPPED: &str = "RecursiveInvocationsDropped";
/// See [`NAMESPACE`].
const COUNT_UNIT: &str = "Count";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The `Period` every metric query reads at, in seconds.
const PERIOD_SECS: i64 = 60;
/// How far either side of now a metric query's window reaches, in seconds.
const WINDOW_SECS: f64 = 600.0;

/// The ceiling the self-invoking handler imposes on ITSELF, which is above the depth the
/// guardrail detects a loop at — so a chain that runs to this ceiling is one no guardrail
/// stopped, and a chain that stops short of it was stopped by one.
const SELF_IMPOSED_CEILING: u32 = MAX_REQUEST_CHAIN_INVOCATIONS + 4;

/// How long a row waits for work the CLOCK owes: a function going `Active`, a queued event's
/// delivery, the next hop of a chain, a datapoint.
///
/// A ceiling on PATIENCE, never a span anything here counts on: every wait below looks again
/// until what it waits for is there or this deadline passes.
const BUDGET: Duration = Duration::from_secs(300);

/// How long a row keeps looking at a chain that has stopped growing before it accepts that it
/// has stopped. Far longer than one hop takes on any host, every hop of which is a queued
/// event delivered and a handler run.
const QUIET_WINDOW: Duration = Duration::from_secs(20);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler every function below runs. Each branch makes ONE call through the SDK the
/// runtime ships and reports what that call answered, so every value a row asserts came back
/// from the emulated service rather than from this package.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    action = event["action"]
    client = boto3.client("lambda")
    if action == "invoke":
        answered = client.invoke(
            FunctionName=event["target"],
            InvocationType=event.get("type", "RequestResponse"),
            Payload=json.dumps(event.get("payload", {})).encode(),
        )
        body = ""
        payload = answered.get("Payload")
        if payload is not None:
            body = payload.read().decode()
        return {"status": answered["StatusCode"], "payload": body}
    if action == "configuration":
        held = client.get_function_configuration(FunctionName=event["target"])
        return {"name": held["FunctionName"], "runtime": held["Runtime"]}
    if action == "list":
        listed = client.list_functions()
        return {"names": sorted(held["FunctionName"] for held in listed["Functions"])}
    if action == "trace":
        return {"trace": os.environ.get("_X_AMZN_TRACE_ID", "")}
    if action == "nested":
        answered = client.invoke(
            FunctionName=event["target"],
            InvocationType="RequestResponse",
            Payload=json.dumps({"action": "trace"}).encode(),
        )
        return {
            "mine": os.environ.get("_X_AMZN_TRACE_ID", ""),
            "theirs": json.loads(answered["Payload"].read().decode())["trace"],
        }
    if action == "chain":
        depth = event["depth"]
        if depth < event["ceiling"]:
            client.invoke(
                FunctionName=context.function_name,
                InvocationType="Event",
                Payload=json.dumps(
                    {"action": "chain", "depth": depth + 1, "ceiling": event["ceiling"]}
                ).encode(),
            )
        return {"depth": depth}
    raise ValueError("the handler was given no action it serves: " + repr(action))
"#;

/// The handler the TARGET of an invocation runs: it answers the event it was handed, so what
/// a caller reads back is this handler's own answer.
const TARGET_SOURCE: &str = r#"
def lambda_handler(event, context):
    return {"echoed": event}
"#;

/// Lambda beside the service the guardrail's drop is metered in, on ONE engine.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One Lambda call, the way a client makes it.
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

/// One CloudWatch call.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("GraniteServiceVersion20100801.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
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

/// A function whose package RUNS `source`, brought to `Active`.
async fn given_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    source: &str,
) {
    let created = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }),
        &[],
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateFunction {name:?} answered {} {}",
        created.status,
        created.text()
    );
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = std::time::SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// One SYNCHRONOUS invocation of `function` with `event`, whose answer is the handler's own
/// document.
async fn invoked(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    event: &Value,
) -> Value {
    let answer = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        event,
        &[],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: Invoke answered {} {}",
        answer.status,
        answer.text()
    );
    let document = answer.json();
    assert!(
        document.get("errorMessage").is_none(),
        "case {case:?}: the handler raised: {document}"
    );
    document
}

/// RULE: a handler's own SDK call reaches the Lambda surface of this same emulator, over the
/// operations its model binds — so what the handler reads back is this service's own answer.
///
/// The rows differ along the operation the handler calls, the invocation type it calls with,
/// and which function it addresses.
#[tokio::test]
async fn a_handler_reaches_lambda_through_the_sdk_its_runtime_ships() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let case = "a handler calling Lambda";
    given_function(&router, &engine, case, "caller", HANDLER_SOURCE).await;
    given_function(&router, &engine, case, "callee", TARGET_SOURCE).await;

    // A synchronous invocation of another function: the answer is the target handler's own
    // document, and the status the invocation's own.
    let answered = invoked(
        &router,
        "a synchronous invocation of another function",
        "caller",
        &json!({
            "action": "invoke",
            "target": "callee",
            "type": "RequestResponse",
            "payload": { "seq": 7 },
        }),
    )
    .await;
    assert_eq!(
        answered["status"], json!(200),
        "case \"a synchronous invocation of another function\": expected the invoked status, got \
         {answered}"
    );
    let echoed: Value = serde_json::from_str(answered["payload"].as_str().unwrap_or_default())
        .unwrap_or(Value::Null);
    assert_eq!(
        echoed,
        json!({ "echoed": { "seq": 7 } }),
        "case \"a synchronous invocation of another function\": expected the target handler's own \
         answer, got {answered}"
    );

    // A configuration READ of the caller itself: the values are the ones this service holds.
    let read = invoked(
        &router,
        "a configuration read",
        "caller",
        &json!({ "action": "configuration", "target": "caller" }),
    )
    .await;
    assert_eq!(
        read,
        json!({ "name": "caller", "runtime": RUNTIME }),
        "case \"a configuration read\": expected this service's own answer, got {read}"
    );

    // A LISTING: the functions this store holds, and nothing else.
    let listed = invoked(
        &router,
        "a listing",
        "caller",
        &json!({ "action": "list" }),
    )
    .await;
    // The listing's own order: the handler sorts the names it read, so the row names the
    // set this store holds in that order.
    assert_eq!(
        listed,
        json!({ "names": ["callee", "caller"] }),
        "case \"a listing\": expected the functions this store holds, got {listed}"
    );

    // A QUEUED event for another function: the accepting status, and the event really
    // delivered to the target — which the target's own queue entry records.
    let queued = invoked(
        &router,
        "a queued event for another function",
        "caller",
        &json!({
            "action": "invoke",
            "target": "callee",
            "type": EVENT_TYPE,
            "payload": { "seq": 9 },
        }),
    )
    .await;
    assert_eq!(
        queued["status"], json!(202),
        "case \"a queued event for another function\": expected the accepted status, got {queued}"
    );
    let delivered = awaited_entries(
        &engine,
        "a queued event for another function",
        "callee",
        1,
    )
    .await;
    assert!(
        delivered.iter().all(|outcome| matches!(
            outcome,
            AsyncInvocationOutcome::Delivered { function_error: None }
        )),
        "case \"a queued event for another function\": expected the event delivered to the \
         target, got {delivered:?}"
    );
}

/// RULE: the call a handler makes carries the INVOCATION'S OWN tracing context, so the hop it
/// produces belongs to the same request chain — which is the chain the recursive-loop
/// guardrail counts.
#[tokio::test]
async fn a_handlers_own_call_stays_in_its_invocations_request_chain() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let case = "a nested invocation";
    given_function(&router, &engine, case, "outer", HANDLER_SOURCE).await;
    given_function(&router, &engine, case, "inner", HANDLER_SOURCE).await;

    // (case, the tracing header the OUTER invocation is called with)
    let supplied = "Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1";
    let rows: &[(&str, Option<&str>)] = &[
        ("a caller that named no chain at all", None),
        ("a caller that named its own chain", Some(supplied)),
    ];
    for (case, header) in rows {
        let headers: Vec<(&str, &str)> = match header {
            Some(value) => vec![("x-amzn-trace-id", value)],
            None => Vec::new(),
        };
        let answer = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/outer/invocations"),
            &json!({ "action": "nested", "target": "inner" }),
            &headers,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: Invoke answered {} {}",
            answer.status,
            answer.text()
        );
        let document = answer.json();
        let mine = root_of(document["mine"].as_str().unwrap_or_default());
        let theirs = root_of(document["theirs"].as_str().unwrap_or_default());
        assert!(
            !mine.is_empty(),
            "case {case:?}: expected the outer invocation to run inside a chain, got {document}"
        );
        assert_eq!(
            theirs, mine,
            "case {case:?}: expected the nested invocation in the same chain as the outer one, \
             got {document}"
        );
        if let Some(value) = header {
            assert_eq!(
                mine,
                root_of(value),
                "case {case:?}: expected the caller's own chain continued, got {document}"
            );
        }
    }
}

/// The `Root` field of a tracing header, which is the chain it names.
fn root_of(header: &str) -> String {
    header
        .split(';')
        .filter_map(|field| field.split_once('='))
        .find(|(name, _)| *name == "Root")
        .map(|(_, value)| value.to_string())
        .unwrap_or_default()
}

/// RULE: `RecursiveLoop` decides what the guardrail DOES with a loop it detects — `Allow`
/// takes no action, so a self-invoking chain runs to the ceiling the function imposes on
/// itself and nothing is metered; `Terminate` stops the chain at the guardrail's own depth,
/// records the invocation it stopped and meters one `RecursiveInvocationsDropped` for it.
#[tokio::test]
async fn a_self_invoking_chain_runs_under_allow_and_is_stopped_under_terminate() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // (case, the function, the `RecursiveLoop` it is configured with)
    let rows: [(&str, &str, &str); 2] = [
        ("a chain the guardrail allows", "chain-allow", "Allow"),
        ("a chain the guardrail terminates", "chain-terminate", "Terminate"),
    ];
    for (case, function, recursive_loop) in rows {
        given_function(&router, &engine, case, function, HANDLER_SOURCE).await;
        let configured = lambda(
            &router,
            "PUT",
            &format!("{RECURSION_URI}/{function}/recursion-config"),
            &json!({ "RecursiveLoop": recursive_loop }),
            &[],
        )
        .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: PutFunctionRecursionConfig answered {} {}",
            configured.status,
            configured.text()
        );
        let accepted = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/invocations"),
            &json!({ "action": "chain", "depth": 0, "ceiling": SELF_IMPOSED_CEILING }),
            &[(INVOCATION_TYPE_HEADER, EVENT_TYPE)],
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the accepting call answered {} {}",
            accepted.status,
            accepted.text()
        );
    }

    // The chain the guardrail ALLOWS runs to the handler's own ceiling: one accepted event
    // per hop, the first one included, and none of them stopped.
    let allowed_hops = usize::try_from(SELF_IMPOSED_CEILING).unwrap_or_default() + 1;
    let allowed = awaited_entries(
        &engine,
        "a chain the guardrail allows",
        "chain-allow",
        allowed_hops,
    )
    .await;
    assert_eq!(
        allowed.len(),
        allowed_hops,
        "case \"a chain the guardrail allows\": expected the chain to run to its own ceiling of \
         {SELF_IMPOSED_CEILING}, got {} hop(s)",
        allowed.len()
    );
    assert!(
        allowed
            .iter()
            .all(|outcome| !matches!(outcome, AsyncInvocationOutcome::Failed(_))),
        "case \"a chain the guardrail allows\": expected every hop delivered, got {allowed:?}"
    );
    let dropped = datapoints(
        &router,
        "a chain the guardrail allows",
        RECURSIVE_INVOCATIONS_DROPPED,
        "chain-allow",
    )
    .await;
    assert!(
        dropped.is_empty(),
        "case \"a chain the guardrail allows\": a guardrail that took no action meters no \
         {RECURSIVE_INVOCATIONS_DROPPED}, got {dropped:?}"
    );

    // The chain it TERMINATES is stopped at the guardrail's own depth: short of the handler's
    // ceiling, with the stopped invocation recorded and one datapoint metered for it.
    let stopped_case = "a chain the guardrail terminates";
    let stopped = settled_chain(&engine, stopped_case, "chain-terminate").await;
    assert!(
        stopped.len() < allowed_hops,
        "case {stopped_case:?}: expected the chain stopped short of the handler's own ceiling of \
         {SELF_IMPOSED_CEILING}, got {} hop(s)",
        stopped.len()
    );
    assert!(
        stopped
            .iter()
            .any(|outcome| matches!(outcome, AsyncInvocationOutcome::Failed(_))),
        "case {stopped_case:?}: expected the stopped invocation recorded, got {stopped:?}"
    );
    let dropped = awaited_sum(
        &router,
        stopped_case,
        RECURSIVE_INVOCATIONS_DROPPED,
        "chain-terminate",
        1.0,
    )
    .await;
    assert!(
        dropped >= 1.0,
        "case {stopped_case:?}: expected at least one {RECURSIVE_INVOCATIONS_DROPPED} datapoint, \
         got {dropped}"
    );
}

/// The outcomes of every event accepted for `function`, once there are at least `wanted` of
/// them and none is still waiting for an attempt.
async fn awaited_entries(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    function: &str,
    wanted: usize,
) -> Vec<AsyncInvocationOutcome> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let outcomes = entries(engine, function);
        let waiting = outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
            )
        });
        if outcomes.len() >= wanted && !waiting {
            return outcomes;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {wanted} settled event(s) for {function:?} within {BUDGET:?}, \
             got {outcomes:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The outcomes of every event accepted for `function` once the chain has STOPPED growing:
/// nothing is still waiting for an attempt and nothing new arrives for [`QUIET_WINDOW`].
async fn settled_chain(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    function: &str,
) -> Vec<AsyncInvocationOutcome> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let outcomes = awaited_entries(engine, case, function, 1).await;
        let quiet = Instant::now() + QUIET_WINDOW;
        let mut grew = false;
        while Instant::now() < quiet {
            tokio::time::sleep(LOOK_AGAIN).await;
            if entries(engine, function).len() != outcomes.len() {
                grew = true;
                break;
            }
        }
        if !grew {
            return outcomes;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the chain on {function:?} to stop growing within {BUDGET:?}"
        );
    }
}

/// The outcomes of every event accepted for `function`, read out of the service's own state
/// because no modelled operation reports an accepted asynchronous invocation back out.
fn entries(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
) -> Vec<AsyncInvocationOutcome> {
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
}

/// The `Datapoints` `GetMetricStatistics` answers for the series `metric` names on
/// `function`.
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
        let sum: f64 = points
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum();
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
