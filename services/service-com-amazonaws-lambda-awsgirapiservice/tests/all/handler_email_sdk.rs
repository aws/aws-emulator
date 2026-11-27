// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a RUNNING handler reaches Amazon SES API v2 through, end to end over the
//! wire: Lambda and SES v2 on ONE engine, a `python3.x` function whose code does
//! `import boto3` and builds a client for the email service, and what each call did read
//! back where a client reads it.
//!
//! RULE: a managed runtime's bundled SDK can build a client for, and call, any service this
//! emulator serves, over that service's own protocol — so what a handler reads back is that
//! service's own answer (the operation's output members on a success, its own error code and
//! message raised as `botocore.exceptions.ClientError` on a refusal) and never a
//! client-side report that the service is unknown.
//!
//! The rows differ along what the rule generalises over: WHICH binding class the operation
//! uses (a body-only create, a create whose output carries no member at all, a read
//! addressed by one path label, a read taking no input, a list bound to its own URI, a send
//! whose input nests several structures), and whether the call is served or refused — and,
//! for a refusal, which of the service's own error codes comes back. Each served row is
//! asserted on BOTH sides: what the guest read back, and what SES v2 itself holds, read
//! through its own API.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `MAX_CONCURRENT_RUNS` invocations at once for the whole process.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long a wait on a transition the CLOCK owes may take before the case gives up.
/// Generous: the condition is what is waited on, not this bound, and a loaded host may
/// take far longer than the span itself to get round to the work.
const SETTLE_DEADLINE: Duration = Duration::from_secs(180);

/// How often such a wait looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `State`'s value for a function an invocation may reach.
const ACTIVE: &str = "Active";

/// The identity the served send is made from: created by the first row, verified once the
/// verification SES owes it is due.
const SENDER: &str = "sender@example.com";

/// An identity no row ever creates, which is what the refused send is made from.
const STRANGER: &str = "stranger@example.net";

/// The configuration set the rows create and read back.
const CONFIGURATION_SET: &str = "handler-sdk-set";

/// Lambda and Amazon SES API v2 over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the identity it
/// creates is the identity this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
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

/// One request dispatched at `host`, as its status, headers and parsed body.
async fn dispatch(
    router: &MultiServiceRouter,
    service: &str,
    host: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.to_string())
        .header("authorization", authorization(service))
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

/// One Lambda `restJson1` call.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    dispatch(
        router,
        "lambda",
        &format!("lambda.{REGION}.amazonaws.com"),
        method,
        uri,
        body,
    )
    .await
}

/// One SES v2 call made as a CLIENT makes it, to read back what the handler's calls did.
async fn email(router: &MultiServiceRouter, case: &str, method: &str, uri: &str) -> Value {
    let (status, _headers, parsed) = dispatch(
        router,
        "ses",
        &format!("email.{REGION}.amazonaws.com"),
        method,
        uri,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {method} {uri}, got {status} ({parsed})"
    );
    parsed
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

/// The handler every row runs: it builds a client for the service the event names, calls the
/// method the event names with the parameters the event carries, and reports what came back —
/// so the invocation's payload IS what the SDK gave the guest.
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
        answered = getattr(client, event["method"])(**event["parameters"])
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "sentence": str(error),
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

/// What must come of one row's call.
enum Outcome {
    /// The service served it, and the answer must carry these output members.
    Served(&'static [&'static str]),
    /// The service refused it, with this error code of its own.
    Refused(&'static str),
}

/// One SDK call the handler makes, and what must come of it.
struct Row {
    case: &'static str,
    /// The client method the handler calls, which names the operation the call addresses.
    method: &'static str,
    /// The keyword arguments it calls that method with: the operation's input document.
    parameters: fn() -> Value,
    outcome: Outcome,
}

/// The rows, in the order they run: each one's state is the state the next reads, so the
/// chain is what the service answered rather than what this suite assumed.
///
/// Every operation named here is one SES API v2's model binds
/// (`smithy_query(query="operation",operation="CreateEmailIdentity,GetEmailIdentity,\
/// CreateConfigurationSet,GetConfigurationSet,ListEmailIdentities,GetAccount,SendEmail")`),
/// and the member list each served row expects is that operation's own output shape's.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a body-only create: CreateEmailIdentity",
            method: "create_email_identity",
            parameters: || json!({ "EmailIdentity": SENDER }),
            outcome: Outcome::Served(&["DkimAttributes", "IdentityType", "VerifiedForSendingStatus"]),
        },
        Row {
            case: "a read addressed by one path label: GetEmailIdentity",
            method: "get_email_identity",
            parameters: || json!({ "EmailIdentity": SENDER }),
            outcome: Outcome::Served(&["IdentityType", "VerificationStatus"]),
        },
        Row {
            case: "a create whose output carries no member: CreateConfigurationSet",
            method: "create_configuration_set",
            parameters: || json!({ "ConfigurationSetName": CONFIGURATION_SET }),
            outcome: Outcome::Served(&[]),
        },
        Row {
            case: "a read addressed by one path label: GetConfigurationSet",
            method: "get_configuration_set",
            parameters: || json!({ "ConfigurationSetName": CONFIGURATION_SET }),
            outcome: Outcome::Served(&["ConfigurationSetName"]),
        },
        Row {
            case: "a list bound to its own URI: ListEmailIdentities",
            method: "list_email_identities",
            parameters: || json!({}),
            outcome: Outcome::Served(&["EmailIdentities"]),
        },
        Row {
            case: "a read taking no input at all: GetAccount",
            method: "get_account",
            parameters: || json!({}),
            outcome: Outcome::Served(&["SendingEnabled"]),
        },
        Row {
            case: "a send whose input nests several structures: SendEmail",
            method: "send_email",
            parameters: || message_from(SENDER),
            outcome: Outcome::Served(&["MessageId"]),
        },
        Row {
            case: "a send from an identity that was never created",
            method: "send_email",
            parameters: || message_from(STRANGER),
            outcome: Outcome::Refused("MessageRejected"),
        },
        Row {
            case: "a read of a configuration set that is not there",
            method: "get_configuration_set",
            parameters: || json!({ "ConfigurationSetName": "no-such-set" }),
            outcome: Outcome::Refused("NotFoundException"),
        },
    ]
}

/// One message, as `SendEmail` takes one: a destination, and a simple content with a subject
/// and a body, each its own nested structure.
///
/// The destination is a mailbox of the SES mailbox simulator, which belongs to no account
/// and needs no identity: this account is in the sandbox, where only verified identities and
/// the simulator may be sent TO, and what these rows are about is the SDK call the handler
/// makes.
fn message_from(sender: &str) -> Value {
    json!({
        "FromEmailAddress": sender,
        "Destination": { "ToAddresses": ["success@simulator.amazonses.com"] },
        "Content": {
            "Simple": {
                "Subject": { "Data": "sent by a handler" },
                "Body": { "Text": { "Data": "the handler's own message" } },
            },
        },
    })
}

/// Invoke `function` with `event` and answer the payload the run produced beside the
/// `FunctionError` signal it carried, if any.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
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

/// Every binding class of the service's protocol, one invocation per row, each asserted on
/// what the guest read back — and then on what SES v2 itself holds.
#[tokio::test]
async fn a_handler_calls_the_email_service_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = emulator();
    let function = "email-sdk-caller";
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    // The activation is owed by the CLOCK, so this looks again until the function reports the
    // state an invocation needs instead of sleeping for a fixed span.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, _headers, configuration) = lambda(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{function}/configuration"),
            &[],
        )
        .await;
        assert_eq!(
            status, 200,
            "expected HTTP 200 from GetFunctionConfiguration, got {status} ({configuration})"
        );
        if configuration["State"] == json!(ACTIVE) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected the activation the clock owes to fall due, last read {configuration}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let mut sent_message_id = String::new();
    for row in rows() {
        let case = row.case;
        let (signal, payload) = invoke(
            &router,
            case,
            function,
            &json!({
                "service": "sesv2",
                "method": row.method,
                "parameters": (row.parameters)(),
            }),
        )
        .await;
        // The call reaches the service, so nothing about it surfaces as the handler's own
        // unhandled exception.
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );
        assert_eq!(
            payload["unknown_service"],
            Value::Null,
            "case {case:?}: the runtime's SDK must reach the service, got {payload}"
        );

        match row.outcome {
            Outcome::Served(members) => {
                assert_eq!(
                    payload["ok"],
                    json!(true),
                    "case {case:?}: expected the call to be served, got {payload}"
                );
                assert_eq!(
                    payload["status"],
                    json!(200),
                    "case {case:?}: expected the SDK to report HTTP 200, got {payload}"
                );
                let answered = &payload["answered"];
                for member in members {
                    assert!(
                        answered.get(member).is_some(),
                        "case {case:?}: expected the output member {member:?}, got {payload}"
                    );
                }
                if members.is_empty() {
                    assert_eq!(
                        payload["members"],
                        json!([]),
                        "case {case:?}: expected an answer carrying no output member, got \
                         {payload}"
                    );
                }
            }
            Outcome::Refused(code) => {
                assert_eq!(
                    payload["ok"],
                    json!(false),
                    "case {case:?}: expected the service to refuse the call, got {payload}"
                );
                assert_eq!(
                    payload["code"],
                    json!(code),
                    "case {case:?}: expected the service's own error code, got {payload}"
                );
                let message = payload["message"].as_str().unwrap_or_default();
                assert!(
                    !message.is_empty(),
                    "case {case:?}: expected the service's own message, got {payload}"
                );
                // The sentence botocore renders names the operation the method was turned
                // into, so a handler that logs the exception logs what a real client logs.
                let sentence = payload["sentence"].as_str().unwrap_or_default();
                let opening = format!("An error occurred ({code}) when calling the ");
                assert!(
                    sentence.starts_with(&opening),
                    "case {case:?}: expected the sentence to open {opening:?}, got {sentence:?}"
                );
            }
        }

        // What each row's own answer says about the resource it addressed.
        match case {
            "a read addressed by one path label: GetEmailIdentity" => assert_eq!(
                payload["answered"]["IdentityType"],
                json!("EMAIL_ADDRESS"),
                "case {case:?}: expected the identity the label addressed, got {payload}"
            ),
            "a read addressed by one path label: GetConfigurationSet" => assert_eq!(
                payload["answered"]["ConfigurationSetName"],
                json!(CONFIGURATION_SET),
                "case {case:?}: expected the set the label addressed, got {payload}"
            ),
            "a list bound to its own URI: ListEmailIdentities" => assert!(
                payload["answered"]["EmailIdentities"]
                    .as_array()
                    .is_some_and(|listed| listed
                        .iter()
                        .any(|identity| identity["IdentityName"] == json!(SENDER))),
                "case {case:?}: expected the created identity among the listed ones, got \
                 {payload}"
            ),
            "a send whose input nests several structures: SendEmail" => {
                sent_message_id = payload["answered"]["MessageId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !sent_message_id.is_empty(),
                    "case {case:?}: expected the message id the service minted, got {payload}"
                );
            }
            _ => {}
        }

        // The identity the first row created is verified once the verification SES owes it
        // is due, which is what the served send below is made from. The transition is owed
        // by the CLOCK, so this looks again until the identity reports it.
        if case == "a body-only create: CreateEmailIdentity" {
            let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
            loop {
                let read = email(
                    &router,
                    case,
                    "GET",
                    &format!("/v2/email/identities/{SENDER}"),
                )
                .await;
                if read["VerifiedForSendingStatus"] == json!(true) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "case {case:?}: expected the verification the clock owes to fall due, last \
                     read {read}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }

    // What SES V2 itself holds: the work the calls reported is the work that happened.
    let case = "the service's own view of what the handler's calls did";
    let identity = email(
        &router,
        case,
        "GET",
        &format!("/v2/email/identities/{SENDER}"),
    )
    .await;
    assert_eq!(
        identity["VerifiedForSendingStatus"],
        json!(true),
        "case {case:?}: expected the identity the handler created, verified, got {identity}"
    );
    let set = email(
        &router,
        case,
        "GET",
        &format!("/v2/email/configuration-sets/{CONFIGURATION_SET}"),
    )
    .await;
    assert_eq!(
        set["ConfigurationSetName"],
        json!(CONFIGURATION_SET),
        "case {case:?}: expected the configuration set the handler created, got {set}"
    );
    let insights = {
        // Message Insights is served under the account's Virtual Deliverability Manager
        // configuration, so the switch goes on before the report is read.
        let (status, _headers, answered) = dispatch(
            &router,
            "ses",
            &format!("email.{REGION}.amazonaws.com"),
            "PUT",
            "/v2/email/account/vdm",
            br#"{"VdmAttributes":{"VdmEnabled":"ENABLED"}}"#,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the VDM switch to be accepted, got {status} ({answered})"
        );
        email(
            &router,
            case,
            "GET",
            &format!("/v2/email/insights/{sent_message_id}"),
        )
        .await
    };
    assert_eq!(
        insights["MessageId"],
        json!(sent_message_id.clone()),
        "case {case:?}: expected the service to hold the message the handler sent, got \
         {insights}"
    );
}
