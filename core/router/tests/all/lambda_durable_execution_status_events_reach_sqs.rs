// SPDX-License-Identifier: CC0-1.0

//! A Lambda DURABLE EXECUTION's status changes reach the SQS target of a rule on the
//! account's default EventBridge bus, over the ASSEMBLED router that verifies request
//! signatures — the configuration the emulator serves traffic in.
//!
//! The RULE under test: **every status a durable execution MOVES INTO is published once onto
//! the account's default EventBridge bus as one `aws.lambda` /
//! `Lambda Durable Execution Status Change` event whose `detail` names the execution, the
//! FUNCTION it belongs to — by the ARN Lambda reports for that function — and the status it
//! moved into, with the instants it started and ended; so a rule on that bus keyed on the
//! class, on one status or on that function delivers one envelope per status to its targets,
//! and the end instant the `STOPPED` envelope carries is the `StopTimestamp` the
//! `StopDurableExecution` call answered.**
//!
//! Sources: `smithy_query(query="shape",shape="ExecutionStatus")` for the statuses an
//! execution takes (`RUNNING`, `SUCCEEDED`, `FAILED`, `TIMED_OUT`, `STOPPED`);
//! `smithy_query(query="operation",operation="StopDurableExecution,GetDurableExecution")` for
//! the `StopTimestamp` a stop answers and the members a read of an execution reports. The
//! publication itself is in no model: it is recorded behaviour, and the names a rule filters
//! on are what this suite holds it to.
//!
//! Why it lives here rather than in the Lambda crate's own target: the crate-local suite
//! `durable_execution_status_events.rs` mounts Lambda, EventBridge and Amazon SQS by hand on
//! one engine with request authentication OFF and a role nothing holds, so it can see neither
//! the function a CloudFormation-deployed role really backs nor whether a publication the
//! durable-execution CLOCK makes — off the response path, after the call that started it was
//! answered — is accepted by a router that authenticates every request. Both halves are what
//! a client observes, and five crates have to agree for them to hold: CloudFormation deploys
//! the role and the function, IAM holds the role, Lambda runs the execution and publishes,
//! EventBridge routes the envelope and Amazon SQS holds what the target received.
//!
//! The rows differ along what the rule generalises over: HOW the execution reaches its
//! terminal status (a handler that returned, and an execution a caller STOPPED) and WHICH key
//! the watching rule is pinned on (the whole class, the function, one status).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on an explicitly sized thread.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element, outputs_of};

/// Keep this at or above the size `boot_serve.rs` uses: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call is signed for and addressed in, which is the Region whose default
/// bus the events are published onto.
const REGION: &str = "us-east-1";

/// The `source` and `detail-type` the events must carry, spelled out here rather than
/// imported from the implementation, so a wrong constant there cannot make this suite pass.
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and a
/// resource's status change is `<service> <resource> Status Change`, the form this tree's
/// other publishers carry (`Step Functions Execution Status Change`).
const EVENT_SOURCE: &str = "aws.lambda";
const DURABLE_EXECUTION_STATUS_CHANGE: &str = "Lambda Durable Execution Status Change";

/// The bus an Amazon Web Services service's own events go to.
const DEFAULT_BUS: &str = "default";

/// The service principal EventBridge delivers to an SQS target as, which the queue's own
/// policy grants ("for Amazon SQS queues, EventBridge relies on resource-based policies").
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The namespace the warm-up event is published under: the client's OWN, not the `aws.` one
/// reserved for AWS services.
const WARM_UP_SOURCE: &str = "com.example.durable-wiring-check";

/// The stack the execution role and the durable function are deployed by.
const STACK: &str = "xsvc-durable-events";

/// The `StackStatus` that stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// Where `Invoke` is bound, and the headers `Invoke$DurableExecutionName` and
/// `Invoke$DurableExecutionArn` are bound to.
const INVOCATIONS_URI: &str = "/2015-03-31/functions";
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The statuses this suite reads, out of the five `ExecutionStatus` admits.
const RUNNING: &str = "RUNNING";
const SUCCEEDED: &str = "SUCCEEDED";
const STOPPED: &str = "STOPPED";

/// The `DurableConfig$ExecutionTimeout` the function runs under, in seconds: far longer than
/// any row takes, so nothing here times out. ("The maximum time (in seconds) that a durable
/// execution can run before timing out", `@range(min: 1, max: 31622400)`.)
const EXECUTION_TIMEOUT: u64 = 3_600;

/// The function `Timeout`, in seconds: longer than the longest handler wait below, so no run
/// is cut off before the status its row is about is reached.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long the handler of the row that must still be RUNNING when it is stopped waits.
const LONG_WAIT_SECONDS: u64 = 30;

/// How long this suite waits for work the clock owes — a stack's resources coming up, a
/// function reaching `Active`, an envelope reaching a queue — and how often it looks while
/// waiting. The waits are on the CONDITION, and the deadline is generous so a loaded host
/// cannot decide the outcome.
const DEADLINE: Duration = Duration::from_secs(120);
const LOOK: Duration = Duration::from_millis(100);

/// The handler every row runs, written the way a durable function's handler is written: it
/// waits for the span its event names and reports it, so a row that must stop a RUNNING
/// execution has one to stop.
const HANDLER: &str = r#"
from aws_durable_execution_sdk_python import Duration, durable_execution


@durable_execution
def handler(event, context):
    waited = event.get("wait_seconds", 0)
    context.wait(Duration.from_seconds(waited))
    return {"waited": waited}
"#;

/// The credential every call below signs with: the pair this process publishes once request
/// authentication is on.
fn published() -> &'static (String, String) {
    static PUBLISHED: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    PUBLISHED.get_or_init(|| {
        let minted = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning request authentication on publishes this process's keys");
        (
            minted.access_key_id.clone(),
            minted.secret_access_key.clone(),
        )
    })
}

/// `request`, signed for `signing_name` in [`REGION`] the way an SDK signs one.
fn signed(mut request: http::Request<Vec<u8>>, signing_name: &str) -> http::Request<Bytes> {
    let (key_id, secret) = published();
    let identity = Credentials::new(key_id, secret, None, None, "test").into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    let (parts, payload) = request.into_parts();
    http::Request::from_parts(parts, Bytes::from(payload))
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.into_bytes())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, service)).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One RPC call (`awsJson`) of the service whose signing name is `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    json_version: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", target)
        .body(body.to_string().into_bytes())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, signing_name)).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge call, which must answer 200.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "events",
        "1.1",
        &format!("AWSEvents.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon SQS call, which must answer 200.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        "sqs",
        "1.0",
        &format!("AmazonSQS.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Amazon SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call: its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
    headers: &[(&str, String)],
) -> (u16, http::HeaderMap, Value) {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let payload = body.map(Value::to_string).unwrap_or_default();
    let request = builder
        .body(payload.into_bytes())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, "lambda")).await;
    let (parts, payload) = response.into_parts();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The template deployed: the execution role Lambda assumes, and the durable function that
/// runs on it. CloudFormation archives the inline source into the deployment package, which
/// is how a function whose handler is written here comes to exist at all.
fn stack_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ExecutionRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "write-its-own-logs",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "DurableFunction": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["ExecutionRole", "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": {"ZipFile": HANDLER},
                },
            },
        },
        "Outputs": {
            "FunctionName": {"Value": {"Ref": "DurableFunction"}},
            "FunctionArn": {"Value": {"Fn::GetAtt": ["DurableFunction", "Arn"]}},
        },
    })
    .to_string()
}

/// The `StackStatus` `stack` reports now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStacks",
        &[("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack:?}) must succeed, got {status} with {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Deploy the stack, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let template = stack_template();
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({STACK:?}) must be accepted, got {status} with {body}"
    );
    let giving_up_at = Instant::now() + DEADLINE;
    let mut reported = stack_status(router, STACK).await;
    while reported.0 != CREATE_COMPLETE && Instant::now() < giving_up_at {
        tokio::time::sleep(LOOK).await;
        reported = stack_status(router, STACK).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// Wait until Lambda reports `name` `Active` with no update in flight, which is the state an
/// invocation needs; answers the configuration it last read.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    let giving_up_at = Instant::now() + DEADLINE;
    let mut last = Value::Null;
    while Instant::now() < giving_up_at {
        let (_, _, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
            &[],
        )
        .await;
        let active = configuration["State"].as_str() == Some("Active");
        let settled = configuration["LastUpdateStatus"]
            .as_str()
            .is_none_or(|status| status == "Successful");
        if active && settled {
            return configuration;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {name:?} Active, last reported {last}");
}

/// A queue called `name` whose resource policy admits EventBridge; answers its url and ARN.
async fn queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        router,
        "GetQueueAttributes",
        &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": {"Service": EVENTS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    sqs(
        router,
        "SetQueueAttributes",
        &json!({"QueueUrl": &url, "Attributes": {"Policy": policy}}),
    )
    .await;
    (url, arn)
}

/// An ENABLED rule called `name` on the default bus carrying `pattern`, with the queue
/// `queue_arn` as its one target.
async fn rule(router: &MultiServiceRouter, name: &str, pattern: &Value, queue_arn: &str) {
    events(
        router,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": DEFAULT_BUS,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    let targeted = events(
        router,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": DEFAULT_BUS,
            "Targets": [{"Id": "t1", "Arn": queue_arn}],
        }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "setup: every target must be accepted, got {targeted}"
    );
}

/// Receive everything waiting on the queue at `url` and delete it; answers the envelopes its
/// message bodies carried.
async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let received = sqs(
        router,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let mut envelopes = Vec::new();
    for message in received["Messages"].as_array().cloned().unwrap_or_default() {
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
        let raw = message["Body"].as_str().unwrap_or_default();
        envelopes.push(serde_json::from_str::<Value>(raw).unwrap_or(Value::Null));
    }
    envelopes
}

/// Every envelope filed under the execution it is about and the status it reports.
type Collected = BTreeMap<(String, String), Vec<Value>>;

/// File each envelope in `envelopes` under its execution and status.
fn file(into: &mut Collected, envelopes: Vec<Value>) {
    for envelope in envelopes {
        let key = (
            envelope["detail"]["durableExecutionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            envelope["detail"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        into.entry(key).or_default().push(envelope);
    }
}

/// Poll the queue at `url` until the envelope reporting `status` for `arn` has arrived,
/// filing everything that arrives meanwhile. Fails the case when nothing delivers it in time.
async fn collect_until(
    router: &MultiServiceRouter,
    url: &str,
    into: &mut Collected,
    case: &str,
    arn: &str,
    status: &str,
) {
    let wanted = (arn.to_string(), status.to_string());
    let giving_up_at = Instant::now() + DEADLINE;
    while Instant::now() < giving_up_at {
        if into.contains_key(&wanted) {
            return;
        }
        file(into, drain(router, url).await);
        if into.contains_key(&wanted) {
            return;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!(
        "case {case:?}: no {status} status-change event was delivered for {arn} within \
         {DEADLINE:?}; the queue had delivered {:?}",
        into.keys().collect::<Vec<_>>()
    );
}

/// Prove the wiring from EventBridge to the queue at `url` delivers at all on this
/// authenticating router, the way a client proves it: a rule on a source of the client's OWN,
/// one event published under it, and the envelope read back off the queue. Leaves the queue
/// empty again, so what arrives later is only what Lambda published.
async fn prove_wiring(router: &MultiServiceRouter, name: &str, url: &str, arn: &str) {
    rule(router, name, &json!({"source": [WARM_UP_SOURCE]}), arn).await;
    let published = events(
        router,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": DEFAULT_BUS,
                "Source": WARM_UP_SOURCE,
                "DetailType": "wiring check",
                "Detail": json!({"check": name}).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "setup: the warm-up publication must be accepted, got {published}"
    );
    let giving_up_at = Instant::now() + DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    while arrived.is_empty() && Instant::now() < giving_up_at {
        arrived.extend(drain(router, url).await);
        if arrived.is_empty() {
            tokio::time::sleep(LOOK).await;
        }
    }
    let sources: Vec<&str> = arrived
        .iter()
        .map(|envelope| envelope["source"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        sources,
        vec![WARM_UP_SOURCE],
        "setup: the wiring to {url} must deliver the client's own event before any Lambda \
         event is expected on it, got {arrived:?}"
    );
}

/// Start one durable execution of `function` named `execution`, whose handler waits
/// `wait_seconds`; answers the `DurableExecutionArn` the accepted invocation reported.
async fn start_execution(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    execution: &str,
    wait_seconds: u64,
) -> String {
    let (status, headers, body) = lambda(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{function}/invocations"),
        Some(&json!({"wait_seconds": wait_seconds})),
        &[
            ("x-amz-invocation-type", "Event".to_string()),
            (DURABLE_EXECUTION_NAME_HEADER, execution.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected 202 from an asynchronous Invoke, got {status} with {body}"
    );
    headers
        .get(DURABLE_EXECUTION_ARN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the accepted invocation must answer an execution ARN")
        })
}

/// One row: how its execution reaches its terminal status.
struct Row {
    case: &'static str,
    /// The execution's name, which is also what its handler is told to wait.
    execution: &'static str,
    wait_seconds: u64,
    /// Whether the row STOPS the execution once it is running, rather than letting the run
    /// decide it.
    stopped: bool,
    /// The status the execution must end in.
    terminal: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an execution whose handler returned",
        execution: "returns",
        wait_seconds: 0,
        stopped: false,
        terminal: SUCCEEDED,
    },
    Row {
        case: "an execution a caller stopped",
        execution: "stopped",
        wait_seconds: LONG_WAIT_SECONDS,
        stopped: true,
        terminal: STOPPED,
    },
];

/// RULE: on the assembled router that verifies request signatures, every status a durable
/// execution moves into reaches the SQS target of a rule on the default bus keyed on the
/// class, on the function or on that status, and the `STOPPED` envelope's end instant is the
/// `StopTimestamp` the stop answered.
#[test]
fn every_durable_execution_status_change_reaches_its_rule_s_queue() {
    std::thread::Builder::new()
        .name("lambda-durable-status-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(drive());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread runs to completion");
}

/// Every row, against ONE assembled router.
async fn drive() {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The whole suite runs with request authentication ON, as the emulator serves traffic:
    // every call below carries the signature this process's keys produce, and so does the
    // publication Lambda makes through the emulator's own callback.
    router.set_request_authentication(true);

    let outputs = deployed(&router).await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack must publish the function's name"));
    // The handle a CLIENT holds for the function: the ARN Lambda answered for it, which is
    // what a rule watching that function is written with.
    let function_arn = outputs
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack must publish the function's ARN"));
    let configuration = wait_until_active(&router, &function).await;
    assert_eq!(
        configuration["FunctionArn"].as_str(),
        Some(function_arn.as_str()),
        "setup: the ARN the stack published must be the one Lambda reports, got {configuration}"
    );

    // The function is made DURABLE, which is what makes an invocation of it start a durable
    // execution with an execution timeout of its own.
    let (status, _, updated) = lambda(
        &router,
        "PUT",
        &format!("/2015-03-31/functions/{function}/configuration"),
        Some(&json!({"DurableConfig": {"ExecutionTimeout": EXECUTION_TIMEOUT}})),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: UpdateFunctionConfiguration must succeed, got {status} with {updated}"
    );
    wait_until_active(&router, &function).await;

    // Three consumers, each proven reachable from the bus before any Lambda event is expected
    // on it, and then pinned to its own key: the whole class of durable-execution status
    // changes, the FUNCTION the executions belong to, and ONE status.
    let (class_url, class_arn) = queue(&router, "durable-status-changes").await;
    prove_wiring(&router, "durable-wiring-class", &class_url, &class_arn).await;
    let (function_url, function_queue_arn) = queue(&router, "durable-status-of-function").await;
    prove_wiring(
        &router,
        "durable-wiring-function",
        &function_url,
        &function_queue_arn,
    )
    .await;
    let (running_url, running_arn) = queue(&router, "durable-status-running-only").await;
    prove_wiring(
        &router,
        "durable-wiring-running",
        &running_url,
        &running_arn,
    )
    .await;

    rule(
        &router,
        "durable-class-rule",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DURABLE_EXECUTION_STATUS_CHANGE],
        }),
        &class_arn,
    )
    .await;
    rule(
        &router,
        "durable-function-rule",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DURABLE_EXECUTION_STATUS_CHANGE],
            "detail": {"functionArn": [&function_arn]},
        }),
        &function_queue_arn,
    )
    .await;
    rule(
        &router,
        "durable-running-rule",
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DURABLE_EXECUTION_STATUS_CHANGE],
            "detail": {"status": [RUNNING]},
        }),
        &running_arn,
    )
    .await;

    let mut by_class: Collected = Collected::new();
    let mut by_function: Collected = Collected::new();
    let mut by_status: Collected = Collected::new();
    let mut started: Vec<(&Row, String, Option<f64>)> = Vec::new();

    for row in ROWS {
        let arn = start_execution(&router, row.case, &function, row.execution, row.wait_seconds)
            .await;
        // The start owes a RUNNING envelope, which is waited FOR: the publication happens on
        // the store's own clock rather than inside the call that was answered.
        collect_until(&router, &class_url, &mut by_class, row.case, &arn, RUNNING).await;
        let mut stop_timestamp = None;
        if row.stopped {
            let (status, _, stopped) = lambda(
                &router,
                "POST",
                &format!(
                    "/2025-12-01/durable-executions/{}/stop",
                    arn.replace('/', "%2F")
                ),
                Some(&json!({
                    "ErrorType": "OperatorStopped",
                    "ErrorMessage": "stopped by the suite",
                })),
                &[],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from StopDurableExecution, got {status} with {stopped}",
                row.case
            );
            stop_timestamp = Some(
                stopped["StopTimestamp"].as_f64().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: StopDurableExecution must answer a StopTimestamp, got {stopped}",
                        row.case
                    )
                }),
            );
        }
        started.push((row, arn, stop_timestamp));
    }

    for (row, arn, stop_timestamp) in &started {
        collect_until(
            &router,
            &class_url,
            &mut by_class,
            row.case,
            arn,
            row.terminal,
        )
        .await;

        for status in [RUNNING, row.terminal] {
            let envelopes = by_class
                .get(&((*arn).clone(), status.to_string()))
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                envelopes.len(),
                1,
                "case {:?}: expected exactly one {status} envelope for {arn}, got {} \
                 ({envelopes:?})",
                row.case,
                envelopes.len()
            );
            let envelope = &envelopes[0];
            assert_eq!(
                envelope["source"].as_str(),
                Some(EVENT_SOURCE),
                "case {:?}: expected the {status} envelope to come from {EVENT_SOURCE}, got \
                 {envelope}",
                row.case
            );
            assert_eq!(
                envelope["detail-type"].as_str(),
                Some(DURABLE_EXECUTION_STATUS_CHANGE),
                "case {:?}: expected the {status} envelope's detail-type to be \
                 {DURABLE_EXECUTION_STATUS_CHANGE}, got {envelope}",
                row.case
            );
            let detail = &envelope["detail"];
            assert_eq!(
                detail["durableExecutionName"].as_str(),
                Some(row.execution),
                "case {:?}: expected the {status} detail to name the execution, got {detail}",
                row.case
            );
            assert_eq!(
                detail["functionName"].as_str(),
                Some(function.as_str()),
                "case {:?}: expected the {status} detail to name the function, got {detail}",
                row.case
            );
            assert_eq!(
                detail["functionArn"].as_str(),
                Some(function_arn.as_str()),
                "case {:?}: expected the {status} detail to name the function by the ARN \
                 Lambda reports for it, got {detail}",
                row.case
            );
            let begun = detail["startTimestamp"].as_i64();
            assert!(
                begun.is_some_and(|instant| instant > 0),
                "case {:?}: expected a start instant in the {status} detail, got {detail}",
                row.case
            );
            let ended = detail["endTimestamp"].as_i64();
            if status == RUNNING {
                assert!(
                    ended.is_none(),
                    "case {:?}: an execution that is still running ended at no instant, got \
                     {detail}",
                    row.case
                );
            } else {
                assert!(
                    ended.is_some_and(|instant| Some(instant) >= begun),
                    "case {:?}: expected an end instant at or after the start on the {status} \
                     envelope, got {detail}",
                    row.case
                );
            }
            // The stop's own answer and the TERMINAL envelope report ONE instant: the one
            // the execution ended at.
            if let (Some(answered), true) = (stop_timestamp, status == row.terminal) {
                let answered_millis = (answered * 1_000.0).round() as i64;
                assert_eq!(
                    ended,
                    Some(answered_millis),
                    "case {:?}: expected the {status} envelope's end instant to be the \
                     StopTimestamp the stop answered ({answered}), got {detail}",
                    row.case
                );
            }
        }
    }

    // The rule keyed on the FUNCTION receives every status of every execution of it, and the
    // rule keyed on ONE status receives that status and nothing else.
    for (row, arn, _) in &started {
        for status in [RUNNING, row.terminal] {
            collect_until(
                &router,
                &function_url,
                &mut by_function,
                row.case,
                arn,
                status,
            )
            .await;
        }
        collect_until(
            &router,
            &running_url,
            &mut by_status,
            row.case,
            arn,
            RUNNING,
        )
        .await;
    }
    file(&mut by_status, drain(&router, &running_url).await);
    let mut statuses: Vec<String> = by_status
        .keys()
        .map(|(_, status)| status.clone())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();
    statuses.sort();
    assert_eq!(
        statuses,
        vec![RUNNING.to_string()],
        "expected the rule keyed on {RUNNING} to receive that status and nothing else, got \
         {statuses:?}"
    );
}
