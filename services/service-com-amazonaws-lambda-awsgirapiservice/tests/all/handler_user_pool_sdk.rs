// SPDX-License-Identifier: CC0-1.0

//! The Amazon Cognito user-pool calls a RUNNING handler makes with the AWS SDK its runtime
//! supplies, end to end over the wire: Lambda and Amazon Cognito Identity Provider on ONE
//! engine, a `python3.x` function whose code does `import boto3` and administers a user pool,
//! and every user it touched read back where a client reads it — through that service's own
//! `AdminGetUser` and `ListUsers`.
//!
//! The rule pinned here: a managed runtime ships the AWS SDK for the handler's language on its
//! import path, so a handler can call a service of this emulator with no dependency of its own,
//! and what the call answers is THAT service's own answer — a success carrying the operation's
//! output members and leaving the state that service then reports, or that service's modelled
//! refusal raised as `botocore.exceptions.ClientError` carrying its error code and message. A
//! user a handler creates is therefore the user the pool holds, which is how the work one
//! invocation performed is observed from outside it.
//!
//! The rows differ along what the rule generalises over: WHICH operation of the service is
//! called (a pool read, a user create, a user read, a user update, a listing), WHICH spelling of
//! the service's SDK id the client is built with (the endpoint-prefix token the Python SDK names
//! it by, the hyphenated token the JavaScript SDK publishes its package under, and the same id
//! with its words run together), whether the call succeeds or is refused, and which modelled
//! refusal it earns (a user the pool does not hold, a pool the account does not hold). Each row
//! asserts both halves — what the handler read back, which is what it returns as the
//! invocation's payload, and what the user pool itself holds afterwards — so a call that
//! answered without doing the work, or did the work without answering, fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The poll-until helper this crate's suites wait a created function out of `Pending` with,
// exported by the suite that first needed it rather than written again here: it LOOKS AGAIN
// until the function reports `Active`, so nothing here waits a fixed time.
use crate::event_source_mappings::await_function_active;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The function this suite runs every row through.
const FUNCTION: &str = "user-pool-caller";

/// The pool the suite creates for the handler to administer.
const POOL_NAME: &str = "handler-administered-pool";

/// The user the SUITE creates for the handler to read, list and disable, the user the HANDLER
/// creates itself, a name no user of the pool answers to, and a well-formed pool id no pool
/// answers to (`UserPoolIdType`'s `^[\w-]+_[0-9a-zA-Z]+$`).
const SUITE_USER: &str = "suite-written-user";
const HANDLER_USER: &str = "handler-written-user";
const ABSENT_USER: &str = "no-such-user";
const ABSENT_POOL: &str = "us-east-1_absentpool";

/// The status Amazon Cognito gives a user an administrator created: "the user must change their
/// password to complete sign-in" (`UserStatusType`, which `AdminCreateUser` leaves a created
/// user in).
const INVITED_STATUS: &str = "FORCE_CHANGE_PASSWORD";

/// Lambda and Amazon Cognito Identity Provider over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the user it creates is the user this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice
            ::EmulatedService::new(engine),
    );
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

/// One Amazon Cognito call (`awsJson1_1`, `AWSCognitoIdentityProviderService.<Operation>`), as
/// its status and parsed body — the way a client makes one.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{REGION}.amazonaws.com"))
        .header("authorization", authorization("cognito-idp"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
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

/// The handler every row runs: it builds a client for the service token the event names, makes
/// the ONE user-pool call it names, and reports what that call answered — so the invocation's
/// payload IS what the SDK gave the guest.
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
    client = boto3.client(event["token"])
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

/// What the pool must report for one user after a row ran: its status and whether it is enabled.
struct Held {
    username: &'static str,
    status: &'static str,
    enabled: bool,
}

/// One row: the token the handler builds its client with, the client method it calls, the input
/// it carries, whether the call must succeed, and what the pool must hold once it has.
struct Row {
    case: &'static str,
    /// What the handler passes to `boto3.client(...)`.
    token: &'static str,
    method: &'static str,
    /// The input, with `{pool}` standing for the id of the pool the suite created.
    parameters: Value,
    /// The code the service refuses this call with, or `None` for a call it serves.
    refused_with: Option<&'static str>,
    /// The message that refusal carries, where it names nothing the suite cannot state.
    refused_message: Option<String>,
    /// What the handler must have read back, for a call that is served: the value at each named
    /// place of the output document it reports, asserted place by place — the members the
    /// service fills from its own state and clock (a user's `sub`, the instant it was created)
    /// are that service's to compute, so no row states them.
    answered: Vec<(&'static [&'static str], Value)>,
    /// The usernames the call must have reported, for a listing.
    listed: Option<&'static [&'static str]>,
    /// What the pool must report afterwards, read through the service's own `AdminGetUser`.
    held: Option<Held>,
}

/// The rows, in the order they run: each later row stands on what the earlier ones wrote, which
/// is what makes the listing and the disable about stored state.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pool read, with the token the Python SDK names the service by",
            token: "cognito-idp",
            method: "describe_user_pool",
            parameters: json!({ "UserPoolId": "{pool}" }),
            refused_with: None,
            refused_message: None,
            answered: vec![
                (&["UserPool", "Name"], json!(POOL_NAME)),
                (&["UserPool", "Id"], json!("{pool}")),
            ],
            listed: None,
            held: None,
        },
        Row {
            case: "a user create: the handler invites a user of its own",
            token: "cognito-idp",
            method: "admin_create_user",
            parameters: json!({ "UserPoolId": "{pool}", "Username": HANDLER_USER }),
            refused_with: None,
            refused_message: None,
            answered: vec![
                (&["User", "Username"], json!(HANDLER_USER)),
                (&["User", "UserStatus"], json!(INVITED_STATUS)),
                (&["User", "Enabled"], json!(true)),
            ],
            listed: None,
            held: Some(Held {
                username: HANDLER_USER,
                status: INVITED_STATUS,
                enabled: true,
            }),
        },
        Row {
            case: "a user read, by the hyphenated token the JavaScript SDK publishes",
            token: "cognito-identity-provider",
            method: "admin_get_user",
            parameters: json!({ "UserPoolId": "{pool}", "Username": SUITE_USER }),
            refused_with: None,
            refused_message: None,
            answered: vec![
                (&["Username"], json!(SUITE_USER)),
                (&["UserStatus"], json!(INVITED_STATUS)),
                (&["Enabled"], json!(true)),
            ],
            listed: None,
            held: None,
        },
        Row {
            case: "a listing, by the same SDK id with its words run together",
            token: "cognitoidentityprovider",
            method: "list_users",
            parameters: json!({ "UserPoolId": "{pool}" }),
            refused_with: None,
            refused_message: None,
            answered: Vec::new(),
            listed: Some(&[HANDLER_USER, SUITE_USER]),
            held: None,
        },
        Row {
            case: "a user update: the handler disables the user the suite created",
            token: "cognito-idp",
            method: "admin_disable_user",
            parameters: json!({ "UserPoolId": "{pool}", "Username": SUITE_USER }),
            refused_with: None,
            refused_message: None,
            answered: Vec::new(),
            listed: None,
            held: Some(Held {
                username: SUITE_USER,
                status: INVITED_STATUS,
                enabled: false,
            }),
        },
        Row {
            case: "a refused read: no user of the pool answers to the name",
            token: "cognito-idp",
            method: "admin_get_user",
            parameters: json!({ "UserPoolId": "{pool}", "Username": ABSENT_USER }),
            refused_with: Some("UserNotFoundException"),
            refused_message: Some("User does not exist.".to_string()),
            answered: Vec::new(),
            listed: None,
            held: None,
        },
        Row {
            case: "a refused read: no pool of the account answers to the id",
            token: "cognito-idp",
            method: "admin_get_user",
            parameters: json!({ "UserPoolId": ABSENT_POOL, "Username": SUITE_USER }),
            refused_with: Some("ResourceNotFoundException"),
            refused_message: Some(format!("User pool {ABSENT_POOL} does not exist.")),
            answered: Vec::new(),
            listed: None,
            held: None,
        },
    ]
}

/// `held`, with every `{pool}` standing in it replaced by the id of the pool the suite created:
/// the id is minted by the service, so no row can state it.
fn addressed(held: &Value, pool: &str) -> Value {
    match held {
        Value::String(text) => Value::String(text.replace("{pool}", pool)),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, value)| (name.clone(), addressed(value, pool)))
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| addressed(item, pool)).collect())
        }
        other => other.clone(),
    }
}

/// RULE: a handler reaches Amazon Cognito user pools through the SDK its runtime supplies, so
/// what its call answers is that service's own answer and what it writes is what that service
/// then reports.
#[tokio::test]
async fn a_running_handler_administers_a_user_pool_through_its_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The pool every row addresses and the user one row READS, created where a client creates
    // them — so what the handler reads back is this service's own record and not something the
    // run produced.
    let (status, created_pool) = cognito(&router, "CreateUserPool", &json!({ "PoolName": POOL_NAME }))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from CreateUserPool, got {status} with {created_pool}"
    );
    let pool = created_pool["UserPool"]["Id"]
        .as_str()
        .expect("the created pool carries an id")
        .to_string();
    let (status, created_user) = cognito(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": &pool, "Username": SUITE_USER }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from AdminCreateUser, got {status} with {created_user}"
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
    // created function leaves `Pending` by the clock, so every row is invoked once it reports
    // `Active` — the condition, looked for again until it holds.
    await_function_active(&router, "the function every row runs", FUNCTION).await;

    for row in rows() {
        let case = row.case;
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
            json!({
                "token": row.token,
                "method": row.method,
                "parameters": addressed(&row.parameters, &pool),
            })
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
                    let expected = addressed(expected, &pool);
                    assert_eq!(
                        *read, expected,
                        "case {case:?}: expected the output member {place:?} to be {expected}, \
                         got {payload}"
                    );
                }
                if let Some(listed) = row.listed {
                    let mut reported: Vec<String> = payload["answered"]["Users"]
                        .as_array()
                        .map(|users| {
                            users
                                .iter()
                                .filter_map(|user| user["Username"].as_str())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    reported.sort();
                    assert_eq!(
                        reported, listed,
                        "case {case:?}: expected the listing to report {listed:?}, got {payload}"
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
                if let Some(message) = &row.refused_message {
                    assert_eq!(
                        payload["message"],
                        json!(message),
                        "case {case:?}: expected the message {message:?}, got {payload}"
                    );
                }
            }
        }

        // The other half: what the user pool itself holds, read where a client reads it.
        if let Some(held) = row.held {
            let (status, reported) = cognito(
                &router,
                "AdminGetUser",
                &json!({ "UserPoolId": &pool, "Username": held.username }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from AdminGetUser, got {status} ({reported})"
            );
            assert_eq!(
                reported["Username"],
                json!(held.username),
                "case {case:?}: expected the pool to hold {:?}, got {reported}",
                held.username
            );
            assert_eq!(
                reported["UserStatus"],
                json!(held.status),
                "case {case:?}: expected the stored status {:?}, got {reported}",
                held.status
            );
            assert_eq!(
                reported["Enabled"],
                json!(held.enabled),
                "case {case:?}: expected the stored Enabled to be {}, got {reported}",
                held.enabled
            );
        }
    }
}
