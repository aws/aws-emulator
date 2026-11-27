// SPDX-License-Identifier: CC0-1.0

//! WHICH endpoint values a running handler can mint an IAM database authentication token for:
//! a `python3.x` function that calls `generate_db_auth_token` with the value a control plane
//! published for the database it is about to dial.
//!
//! The rule pinned here: `generate_db_auth_token` is not an API call and not a lookup — the SDK
//! PRESIGNS `GET https://<DBHostname>:<Port>/?Action=connect&DBUser=<user>` in the client, so
//! the host it is handed travels into the token's address exactly as the caller wrote it. The
//! SDK neither resolves nor parses it, so WHICH FORM that value takes decides nothing: whatever
//! a caller read out of a control plane's endpoint member — a DNS name, an address literal, an
//! authority that already carries a port because the member it came from models none — is
//! minted a token addressed `<DBHostname>:<Port>`, which is then the password a login presents.
//! A caller is never answered "no token" for the shape of the address it named.
//!
//! The rows differ along what the rule generalises over: the FORM of the endpoint value (a DNS
//! name, a `host:port` authority of the kind a member carrying no port of its own publishes, an
//! address literal, a bracketed IPv6 literal, and a name spelling the DNS root), beside the
//! port completing it and the database user named.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// The Region every call below is signed for, which is the Region the token's credential scope
/// has to name.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `Role` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The function that mints the tokens.
const FUNCTION: &str = "db-auth-token-endpoint-forms";

/// How long the handler is given, as the function's own `Timeout`: well past what minting one
/// token needs on any host, because what is under test is WHICH value a token can be minted for
/// and not how fast an interpreter gets through one.
const FUNCTION_TIMEOUT: u64 = 60;

/// How long this suite leaves between two readings of a state the clock owes.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long a state the clock owes is waited for before the wait itself is the failure.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How many characters a SigV4 signature is: the hex of a SHA-256 HMAC.
const SIGNATURE_CHARS: usize = 64;

/// Lambda on its own engine: a token is minted by the RUNTIME in the client, so no other
/// service takes part in it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names Lambda, which is what routes the
/// request. Nothing in it is verified by a router built with authentication off.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
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
        .header("authorization", authorization())
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

/// The handler every row runs: it asks the RDS client for a token for the endpoint value the
/// event names, and reports what it got — including, as DATA rather than as a raised failure,
/// the refusal it was answered with instead of a token.
const HANDLER_SOURCE: &str = r#"
import boto3


def lambda_handler(event, context):
    client = boto3.client("rds")
    try:
        token = client.generate_db_auth_token(
            DBHostname=event["host"],
            Port=event["port"],
            DBUsername=event["user"],
        )
    except BaseException as error:
        return {"ok": False, "error": type(error).__name__, "message": str(error)}
    return {"ok": True, "token": token}
"#;

/// Create the function and wait for the state an invocation needs, reading the CONDITION back
/// through the function's own configuration rather than sleeping for a span.
async fn plant_the_function(router: &MultiServiceRouter) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": FUNCTION_TIMEOUT,
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
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, _headers, configuration) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{FUNCTION}/configuration"),
            &[],
        )
        .await;
        if status == 200 && configuration["State"] == json!("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the function never became Active: {status} ({configuration})"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `Invoke` the function with one row's event, answering the payload the run produced.
async fn mint(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    assert_eq!(
        headers.get("x-amz-function-error"),
        None,
        "case {case:?}: the handler must run to completion, got {payload}"
    );
    payload
}

/// The fields the query string of a token carries, by name.
fn fields(token: &str) -> Vec<(String, String)> {
    let query = token.split_once("/?").map(|(_, query)| query).unwrap_or("");
    form_urlencoded::parse(query.as_bytes())
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The one value of the field `name`.
fn field(token: &str, name: &str) -> Option<String> {
    fields(token)
        .into_iter()
        .find(|(held, _)| held == name)
        .map(|(_, value)| value)
}

/// One row: the endpoint value a caller read, the port completing it, and the database user.
struct Row {
    case: &'static str,
    host: &'static str,
    port: u32,
    user: &'static str,
}

/// The rows: the forms an endpoint value a caller reads out of a control plane takes.
const ROWS: &[Row] = &[
    Row {
        case: "a DNS endpoint name, as AWS publishes one",
        host: "orders.proxy-abcdef012345.us-east-1.rds.amazonaws.com",
        port: 3306,
        user: "app_user",
    },
    Row {
        // The form a member that models NO PORT of its own publishes when the address behind it
        // is reached on a port the resource was assigned: the whole value is the host a client
        // was handed, and it completes it with the port its engine is dialled on.
        case: "a host:port authority, read out of a member that models no port",
        host: "127.0.0.1:43211",
        port: 3306,
        user: "app_user",
    },
    Row {
        case: "an address literal and the PostgreSQL port",
        host: "127.0.0.1",
        port: 5432,
        user: "reporting",
    },
    Row {
        case: "an IPv6 literal, bracketed as an authority writes one",
        host: "[::1]",
        port: 3306,
        user: "app_user",
    },
    Row {
        case: "a name spelling the DNS root, and a user name a query string escapes",
        host: "orders.proxy-abcdef012345.us-east-1.rds.amazonaws.com.",
        port: 65_535,
        user: "iam user+1",
    },
];

/// RULE: the token a handler mints is addressed to the endpoint value it named, whatever form
/// that value takes — the SDK presigns the address it was handed rather than interpreting it.
#[tokio::test]
async fn a_handler_mints_a_token_for_the_endpoint_value_it_was_handed() {
    let _stream = one_invocation_stream_at_a_time().await;
    let router = router();
    plant_the_function(&router).await;

    for row in ROWS {
        let case = row.case;
        let payload = mint(
            &router,
            case,
            &json!({ "host": row.host, "port": row.port, "user": row.user }),
        )
        .await;
        assert_eq!(
            payload["ok"],
            json!(true),
            "case {case:?}: expected a token for {}:{}, got {payload}",
            row.host,
            row.port
        );
        let token = payload["token"].as_str().unwrap_or_default().to_string();

        // It addresses the endpoint, the port and the user the handler named.
        let addressed = format!("{}:{}/?", row.host, row.port);
        assert!(
            token.starts_with(&addressed),
            "case {case:?}: expected the token to address {addressed:?}, got {token:?}"
        );
        assert_eq!(
            field(&token, "Action").as_deref(),
            Some("connect"),
            "case {case:?}: expected a token for the connect action, got {token:?}"
        );
        assert_eq!(
            field(&token, "DBUser").as_deref(),
            Some(row.user),
            "case {case:?}: expected the token to name the database user, got {token:?}"
        );

        // And it is a presigned SigV4 request: the algorithm, a credential scope naming the
        // `rds-db` signing service in the client's own Region, and a signature.
        assert_eq!(
            field(&token, "X-Amz-Algorithm").as_deref(),
            Some("AWS4-HMAC-SHA256"),
            "case {case:?}: expected a SigV4 presigned request, got {token:?}"
        );
        let credential = field(&token, "X-Amz-Credential").unwrap_or_default();
        let scope: Vec<&str> = credential.split('/').collect();
        assert_eq!(
            (scope.len(), scope.get(2).copied(), scope.get(3).copied()),
            (5, Some(REGION), Some("rds-db")),
            "case {case:?}: expected a credential scope naming rds-db in {REGION}, got \
             {credential:?}"
        );
        let signature = field(&token, "X-Amz-Signature").unwrap_or_default();
        assert!(
            signature.len() == SIGNATURE_CHARS
                && signature.chars().all(|character| character.is_ascii_hexdigit()),
            "case {case:?}: expected a SigV4 signature, got {signature:?}"
        );
        // The ADDRESS is what the signature covers of the endpoint: a canonical request carries
        // no authority, so the one header the token is signed over is the `host` the client
        // composed — exactly one of it, whatever form that host takes.
        assert_eq!(
            field(&token, "X-Amz-SignedHeaders").as_deref(),
            Some("host"),
            "case {case:?}: expected the host to be the one signed header, got {token:?}"
        );
    }
}
