// SPDX-License-Identifier: CC0-1.0

//! The Parameter Store calls a RUNNING handler makes with the AWS SDK its runtime supplies,
//! end to end over the wire: Lambda and AWS Systems Manager on ONE engine, a `python3.x`
//! function whose code does `import boto3` and calls Parameter Store, and every parameter it
//! wrote read back where a client reads it — through that service's own `GetParameter`.
//!
//! The rule pinned here: a managed runtime ships the AWS SDK for the handler's language on its
//! import path, so a handler can call a service of this emulator with no dependency of its own,
//! and what the call answers is THAT service's own answer — a success carrying the operation's
//! output members and leaving the state that service then reports, or that service's modelled
//! refusal raised as `botocore.exceptions.ClientError` carrying its error code. A parameter a
//! handler writes is therefore the parameter Parameter Store holds, which is how the work one
//! invocation performed is observed from outside it.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the service is
//! called (a create, a create of another parameter TYPE, an overwrite, a read), whether the
//! call succeeds or is refused, and which modelled refusal it earns (a create of a name the
//! store already holds, a read of a name it does not hold). Each row asserts both halves — what
//! the handler read back, which is what it returns as the invocation's payload, and what
//! Parameter Store itself holds afterwards — so a call that answered without doing the work, or
//! did the work without answering, fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The function this suite runs every row through.
const FUNCTION: &str = "parameter-store-caller";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The parameter the handler creates and then overwrites, the one it creates with the other
/// type, the one the SUITE writes for the handler to read, and a name no parameter answers to.
const HANDLER_PARAMETER: &str = "/emulator/handler/plain";
const LIST_PARAMETER: &str = "/emulator/handler/list";
const SUITE_PARAMETER: &str = "/emulator/suite/written";
const ABSENT_PARAMETER: &str = "/emulator/suite/absent";

/// The value the suite writes for the handler to read back.
const SUITE_VALUE: &str = "written by the suite";

/// Lambda and AWS Systems Manager over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the parameter it
/// writes is the parameter this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Systems Manager call (`awsJson1_1`, `AmazonSSM.<Operation>`), as its status and parsed
/// body — the way a client makes one.
async fn ssm(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ssm.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ssm"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
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

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
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

/// The handler every row runs: it makes the ONE Parameter Store call the event names and
/// reports what that call answered, so the invocation's payload IS what the SDK gave the guest.
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
    client = boto3.client("ssm")
    method = getattr(client, event["method"])
    try:
        answered = method(**event["parameters"])
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"].get("Message"),
        }
    except botocore.exceptions.UnknownServiceError as error:
        return {"ok": False, "unknown_service": str(error)}
    return {
        "ok": True,
        "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        "answered": _reported({
            key: value for key, value in answered.items() if key != "ResponseMetadata"
        }),
    }
"#;

/// What the store must report for one parameter after a row ran: its value, its type and the
/// version the write produced.
struct Held {
    name: &'static str,
    value: &'static str,
    type_: &'static str,
    version: u64,
}

/// One row: the client method the handler calls, the input it carries, whether the call must
/// succeed, and what Parameter Store must hold once it has.
struct Row {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    /// The code the service refuses this call with, or `None` for a call it serves.
    refused_with: Option<&'static str>,
    /// What the handler must have read back, for a call that is served: the value at each
    /// named place of the output document it reports, asserted place by place — the members
    /// the service fills from its own state and clock (a parameter's ARN, its data type, the
    /// instant it was last modified) are that service's to compute, so no row states them.
    answered: Vec<(&'static [&'static str], Value)>,
    /// What the store must report afterwards, read through the service's own `GetParameter`.
    held: Option<Held>,
}

/// The rows, in the order they run: each later row stands on what the earlier ones wrote, which
/// is what makes the overwrite and the already-exists refusal about stored state.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create: the handler writes a String parameter",
            method: "put_parameter",
            parameters: json!({
                "Name": HANDLER_PARAMETER,
                "Value": "first",
                "Type": "String",
            }),
            refused_with: None,
            answered: vec![(&["Version"], json!(1))],
            held: Some(Held {
                name: HANDLER_PARAMETER,
                value: "first",
                type_: "String",
                version: 1,
            }),
        },
        Row {
            case: "a create of another parameter type: the handler writes a StringList",
            method: "put_parameter",
            parameters: json!({
                "Name": LIST_PARAMETER,
                "Value": "alpha,beta,gamma",
                "Type": "StringList",
            }),
            refused_with: None,
            answered: vec![(&["Version"], json!(1))],
            held: Some(Held {
                name: LIST_PARAMETER,
                value: "alpha,beta,gamma",
                type_: "StringList",
                version: 1,
            }),
        },
        Row {
            case: "an overwrite: the handler writes the same name again with Overwrite",
            method: "put_parameter",
            parameters: json!({
                "Name": HANDLER_PARAMETER,
                "Value": "second",
                "Type": "String",
                "Overwrite": true,
            }),
            refused_with: None,
            // "An overwrite APPENDS a version rather than replacing the stored one", so the
            // write the handler made is the parameter's second version.
            answered: vec![(&["Version"], json!(2))],
            held: Some(Held {
                name: HANDLER_PARAMETER,
                value: "second",
                type_: "String",
                version: 2,
            }),
        },
        Row {
            case: "a refused create: the name is already held and the call does not overwrite",
            method: "put_parameter",
            parameters: json!({
                "Name": HANDLER_PARAMETER,
                "Value": "third",
                "Type": "String",
            }),
            refused_with: Some("ParameterAlreadyExists"),
            answered: Vec::new(),
            // The refused write changed nothing: the parameter still holds what the overwrite
            // above left on it.
            held: Some(Held {
                name: HANDLER_PARAMETER,
                value: "second",
                type_: "String",
                version: 2,
            }),
        },
        Row {
            case: "a read: the handler reads the parameter the suite wrote",
            method: "get_parameter",
            parameters: json!({ "Name": SUITE_PARAMETER }),
            refused_with: None,
            answered: vec![
                (&["Parameter", "Name"], json!(SUITE_PARAMETER)),
                (&["Parameter", "Type"], json!("String")),
                (&["Parameter", "Value"], json!(SUITE_VALUE)),
                (&["Parameter", "Version"], json!(1)),
            ],
            held: None,
        },
        Row {
            case: "a refused read: no parameter answers to the name",
            method: "get_parameter",
            parameters: json!({ "Name": ABSENT_PARAMETER }),
            refused_with: Some("ParameterNotFound"),
            answered: Vec::new(),
            held: None,
        },
    ]
}

/// RULE: a handler reaches AWS Systems Manager through the SDK its runtime supplies, so what
/// its Parameter Store call answers is that service's own answer and what it writes is what
/// that service then reports.
#[tokio::test]
async fn a_running_handler_reads_and_writes_parameter_store_through_its_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The parameter one row READS, written where a client writes one — so the value the handler
    // reads back is this service's own record and not something the run produced.
    let (status, written) = ssm(
        &router,
        "PutParameter",
        &json!({ "Name": SUITE_PARAMETER, "Value": SUITE_VALUE, "Type": "String" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from PutParameter, got {status} with {written}"
    );

    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 20,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    // "During this time, you can't invoke or modify the function" (`CreateFunction`): the
    // created function leaves `Pending` by the clock, so every row is invoked past that span.
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    for row in rows() {
        let case = row.case;
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
            json!({ "method": row.method, "parameters": row.parameters })
                .to_string()
                .as_bytes(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
        );
        let signal = headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            signal, None,
            "case {case:?}: expected no FunctionError, got {signal:?} with {payload}"
        );

        match row.refused_with {
            None => {
                assert_eq!(
                    payload["ok"],
                    json!(true),
                    "case {case:?}: expected the handler's call to be served, got {payload}"
                );
                assert_eq!(
                    payload["status"],
                    json!(200),
                    "case {case:?}: expected the SDK to report HTTP 200, got {payload}"
                );
                for (place, expected) in &row.answered {
                    let mut read = &payload["answered"];
                    for member in *place {
                        read = &read[member];
                    }
                    assert_eq!(
                        read, expected,
                        "case {case:?}: expected the output member {place:?} to be {expected}, \
                         got {payload}"
                    );
                }
            }
            Some(code) => {
                assert_eq!(
                    payload["ok"],
                    json!(false),
                    "case {case:?}: expected the service to refuse the handler's call, got \
                     {payload}"
                );
                assert_eq!(
                    payload["code"],
                    json!(code),
                    "case {case:?}: expected the error code {code:?}, got {payload}"
                );
            }
        }

        // The other half: what Parameter Store itself holds, read where a client reads it.
        if let Some(held) = row.held {
            let (status, reported) =
                ssm(&router, "GetParameter", &json!({ "Name": held.name })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetParameter, got {status} ({reported})"
            );
            assert_eq!(
                reported["Parameter"]["Value"],
                json!(held.value),
                "case {case:?}: expected the store to hold {:?}, got {reported}",
                held.value
            );
            assert_eq!(
                reported["Parameter"]["Type"],
                json!(held.type_),
                "case {case:?}: expected the stored type {:?}, got {reported}",
                held.type_
            );
            assert_eq!(
                reported["Parameter"]["Version"],
                json!(held.version),
                "case {case:?}: expected the stored version {}, got {reported}",
                held.version
            );
        }
    }
}
