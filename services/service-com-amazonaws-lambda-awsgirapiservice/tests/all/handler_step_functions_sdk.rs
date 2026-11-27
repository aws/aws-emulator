// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler reaches AWS Step Functions through, end to end over the
//! wire: Lambda and Step Functions on ONE engine, a `python3.x` function whose code does
//! `import boto3`, and the execution it started read back where a client reads it.
//!
//! The rule pinned here: the AWS SDK a managed runtime supplies can build a client for any
//! AWS service the handler names, so a token naming a service THIS emulator serves is
//! signed, routed to that service's own endpoint and answered by that service's own API —
//! a success carrying the operation's own output members, or that service's own modelled
//! refusal raised as `botocore.exceptions.ClientError`. A client the runtime's SDK has no
//! registry entry for would instead raise `UnknownServiceError` before anything is signed,
//! which is not an answer any service gave; every row below asserts that it did not.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the service is
//! called (a write that starts an execution, a read of the state machine, a list of its
//! executions), WHICH token the client is built with (the SDK's own name for the service
//! and the service's SDK id), and whether the call succeeds or is refused — with three
//! different modelled refusals, each named by the resource the call could not be made on.
//! The write is then asserted on the STEP FUNCTIONS side too, through that service's own
//! API, so a call that answered without starting an execution fails here.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_sfn_awsstepfunctions::state::EXECUTION_SETTLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the function's own
/// invocation — and therefore the client its handler builds — runs in.
const REGION: &str = "us-east-1";

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The role a state machine is created with, inside the modelled `Arn` length.
const STATE_MACHINE_ROLE: &str = "arn:aws:iam::000000000000:role/StepFunctionsRole";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `X-Amz-Target` prefix Step Functions names an operation under (its service shape).
const TARGET_PREFIX: &str = "AWSStepFunctions";

/// A definition that succeeds at once with a static result, so the execution the handler
/// starts reaches a terminal status the clock owes.
const HELLO_DEFINITION: &str =
    r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// The name of the state machine every row runs against.
const MACHINE_NAME: &str = "handler-called-machine";

/// The execution name the handler's own `start_execution` asks for.
const EXECUTION_NAME: &str = "started-by-the-handler";

/// The execution input the handler's own `start_execution` carries.
const EXECUTION_INPUT: &str = r#"{"from":"the handler"}"#;

/// How long past a due instant this suite waits before reading. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// Lambda and AWS Step Functions over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the handler's SDK call is dispatched back into this same router, so the
/// execution it starts is the execution this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes
/// the request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Step Functions call (`awsJson1_0`, `AWSStepFunctions.<Operation>`) that must
/// answer 200.
async fn sfn(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{REGION}.amazonaws.com"))
        .header("authorization", authorization("states"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Step Functions {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function whose handler makes the calls, and wait out the provisioning
/// window.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(
                    package(HANDLER_SOURCE),
                ),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, answering the payload the run
/// produced and the `FunctionError` signal it carried, if any.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// The handler every row runs: it reports what the ONE SDK call it makes answered, so the
/// invocation's payload IS what the SDK gave the guest.
///
/// The client's own construction is reported too: a service the runtime's SDK has no
/// registry entry for raises `UnknownServiceError` there, before anything is signed, which
/// is what every row asserts did not happen.
const HANDLER_SOURCE: &str = r#"
import datetime

import boto3
import botocore.exceptions


def _reported(value):
    """`value` as JSON, which is what an invocation's payload has to be.

    An SDK hands a member the called service's model types as a timestamp over as a `datetime`,
    so a handler reporting the whole answer renders each instant in it as its ISO-8601 text.
    """
    if isinstance(value, datetime.datetime):
        return value.isoformat()
    if isinstance(value, dict):
        return {name: _reported(held) for name, held in value.items()}
    if isinstance(value, list):
        return [_reported(held) for held in value]
    return value


def lambda_handler(event, context):
    try:
        client = boto3.client(event["service"])
    except botocore.exceptions.UnknownServiceError as error:
        return {"ok": False, "unknown_service": str(error)}
    try:
        method = getattr(client, event["method"])
    except AttributeError as error:
        return {"ok": False, "attribute_error": str(error)}
    try:
        answered = method(**event["parameters"])
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "sentence": str(error),
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
            "modelled": isinstance(error, client.exceptions.from_code(
                error.response["Error"]["Code"])),
        }
    return {
        "ok": True,
        "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        "members": sorted(key for key in answered if key != "ResponseMetadata"),
        "answered": _reported({
            key: value for key, value in answered.items() if key != "ResponseMetadata"
        }),
    }
"#;

/// What one row asks the handler to do, and what must come of it.
struct Row {
    case: &'static str,
    /// The service token the handler builds its client with.
    service: &'static str,
    /// The client method it calls.
    method: &'static str,
    /// The operation that method names, which is what the call must address.
    operation: &'static str,
    /// The output members the answer must carry, for a row that succeeds — the ones the
    /// operation's own model states — or the modelled error code it must be refused with.
    expected: Expected,
}

/// What a row's call must answer.
enum Expected {
    /// A success carrying these output members.
    Members(&'static [&'static str]),
    /// A refusal naming this modelled error code.
    Refused(&'static str),
}

/// The rows, in the order they run: the write comes first, so the read and the list that
/// follow it report what it left behind.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a write: start_execution starts an execution of the state machine",
            // The AWS SDK for Python's own name for this service, which is what a handler
            // builds its client with.
            service: "stepfunctions",
            method: "start_execution",
            operation: "StartExecution",
            // `StartExecutionOutput`: executionArn and startDate, both required.
            expected: Expected::Members(&["executionArn", "startDate"]),
        },
        Row {
            // The same service under the token its own SDK id gives it, which is the other
            // name a client is built with.
            case: "a read under the service's SDK id: describe_state_machine",
            service: "sfn",
            method: "describe_state_machine",
            operation: "DescribeStateMachine",
            expected: Expected::Members(&[
                "stateMachineArn",
                "name",
                "definition",
                "roleArn",
                "type",
                "creationDate",
            ]),
        },
        Row {
            case: "a list: list_executions reports the execution the handler started",
            service: "stepfunctions",
            method: "list_executions",
            operation: "ListExecutions",
            expected: Expected::Members(&["executions"]),
        },
        Row {
            case: "a refused write: a state machine that is not there",
            service: "stepfunctions",
            method: "start_execution",
            operation: "StartExecution",
            expected: Expected::Refused("StateMachineDoesNotExist"),
        },
        Row {
            case: "a refused write: a state machine ARN that is not an ARN",
            service: "stepfunctions",
            method: "start_execution",
            operation: "StartExecution",
            expected: Expected::Refused("InvalidArn"),
        },
        Row {
            case: "a refused read: an execution that is not there",
            service: "sfn",
            method: "describe_execution",
            operation: "DescribeExecution",
            expected: Expected::Refused("ExecutionDoesNotExist"),
        },
    ]
}

/// The parameters each row's call carries, built against the state machine the rows run
/// over.
fn parameters(case: &str, machine_arn: &str) -> Value {
    match case {
        "a write: start_execution starts an execution of the state machine" => json!({
            "stateMachineArn": machine_arn,
            "name": EXECUTION_NAME,
            "input": EXECUTION_INPUT,
        }),
        "a refused write: a state machine that is not there" => json!({
            "stateMachineArn":
                format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:nowhere-near-here"),
        }),
        "a refused write: a state machine ARN that is not an ARN" => {
            json!({ "stateMachineArn": "not-a-valid-arn" })
        }
        "a refused read: an execution that is not there" => json!({
            "executionArn":
                format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:{MACHINE_NAME}:absent"),
        }),
        _ => json!({ "stateMachineArn": machine_arn }),
    }
}

/// One SDK call per row, each asserted on what the guest read back — and the write
/// asserted again on the Step Functions side, where a client reads it.
#[tokio::test]
async fn a_handler_calls_step_functions_through_the_sdk_the_runtime_supplies() {
    let router = router();
    let case = "the state machine every row runs against";
    let created = sfn(
        &router,
        case,
        "CreateStateMachine",
        &json!({
            "name": MACHINE_NAME,
            "definition": HELLO_DEFINITION,
            "roleArn": STATE_MACHINE_ROLE,
        }),
    )
    .await;
    let machine_arn = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a stateMachineArn, got {created}"))
        .to_string();

    let function = "step-functions-caller";
    create_function(&router, case, function).await;

    let mut started = String::new();
    for row in rows() {
        let case = row.case;
        let (signal, payload) = invoke(
            &router,
            case,
            function,
            &json!({
                "service": row.service,
                "method": row.method,
                "parameters": parameters(case, &machine_arn),
            }),
        )
        .await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        // The defect this suite was written for: a token the runtime's SDK has no entry
        // for never reaches a service at all, so the handler reports a client it could not
        // build instead of an answer.
        assert!(
            payload["unknown_service"].is_null(),
            "case {case:?}: expected the client to be built for {:?}, got {payload}",
            row.service
        );
        assert!(
            payload["attribute_error"].is_null(),
            "case {case:?}: expected the client to carry a {:?} method, got {payload}",
            row.method
        );

        match row.expected {
            Expected::Members(members) => {
                assert_eq!(
                    payload["ok"],
                    json!(true),
                    "case {case:?}: expected the call to succeed, got {payload}"
                );
                assert_eq!(
                    payload["status"],
                    json!(200),
                    "case {case:?}: expected the SDK to report HTTP 200, got {payload}"
                );
                for member in members {
                    assert!(
                        !payload["answered"][*member].is_null(),
                        "case {case:?}: expected the answer to carry {member:?}, got {payload}"
                    );
                }
            }
            Expected::Refused(code) => {
                assert_eq!(
                    payload["ok"],
                    json!(false),
                    "case {case:?}: expected the call to be refused, got {payload}"
                );
                assert_eq!(
                    payload["code"],
                    json!(code),
                    "case {case:?}: expected the modelled error code {code:?}, got {payload}"
                );
                assert_eq!(
                    payload["status"],
                    json!(400),
                    "case {case:?}: expected the refusal's own status, got {payload}"
                );
                let message = payload["message"].as_str().unwrap_or_default();
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected the service's own message, got {payload}"
                );
                // The sentence botocore renders, so a handler that logs the exception logs
                // what a real client logs — naming the operation the method was turned
                // into.
                let sentence = payload["sentence"].as_str().unwrap_or_default();
                let expected =
                    format!("An error occurred ({code}) when calling the {} operation: ", row.operation);
                assert!(
                    sentence.starts_with(&expected),
                    "case {case:?}: expected the sentence to open {expected:?}, got {sentence:?}"
                );
                assert_eq!(
                    payload["modelled"],
                    json!(true),
                    "case {case:?}: expected the refusal to be the modelled error class for \
                     its code, got {payload}"
                );
            }
        }

        if row.operation == "StartExecution" && payload["ok"] == json!(true) {
            started = payload["answered"]["executionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        if row.operation == "ListExecutions" {
            let listed = payload["answered"]["executions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let arns: Vec<&str> = listed
                .iter()
                .filter_map(|entry| entry["executionArn"].as_str())
                .collect();
            assert!(
                arns.contains(&started.as_str()),
                "case {case:?}: expected the list to carry the execution the handler \
                 started ({started:?}), got {arns:?}"
            );
        }
    }

    // What STEP FUNCTIONS holds: the execution the call reported is the execution the
    // service started, with the name and input the handler asked for, carried on to the
    // terminal outcome its definition dictates once the clock makes it due.
    assert!(
        started.starts_with(&format!(
            "arn:aws:states:{REGION}:{ACCOUNT}:execution:{MACHINE_NAME}:"
        )),
        "case {case:?}: expected an execution ARN of the named machine, got {started:?}"
    );
    let described = sfn(
        &router,
        case,
        "DescribeExecution",
        &json!({ "executionArn": started }),
    )
    .await;
    assert_eq!(
        described["stateMachineArn"],
        json!(machine_arn),
        "case {case:?}: expected the execution to be of the created machine, got {described}"
    );
    assert_eq!(
        described["name"],
        json!(EXECUTION_NAME),
        "case {case:?}: expected the name the handler asked for, got {described}"
    );
    assert_eq!(
        described["input"],
        json!(EXECUTION_INPUT),
        "case {case:?}: expected the input the handler sent, got {described}"
    );

    tokio::time::sleep(EXECUTION_SETTLE_DELAY + SLACK).await;
    let settled = sfn(
        &router,
        case,
        "DescribeExecution",
        &json!({ "executionArn": started }),
    )
    .await;
    assert_eq!(
        settled["status"],
        json!("SUCCEEDED"),
        "case {case:?}: expected the execution to reach its terminal status, got {settled}"
    );
    let output: Value = settled["output"]
        .as_str()
        .map(|text| serde_json::from_str(text).expect("the output is JSON"))
        .unwrap_or_else(|| panic!("case {case:?}: expected an output in {settled}"));
    assert_eq!(
        output,
        json!({ "msg": "hi" }),
        "case {case:?}: expected the output the definition computes, got {output}"
    );
}
