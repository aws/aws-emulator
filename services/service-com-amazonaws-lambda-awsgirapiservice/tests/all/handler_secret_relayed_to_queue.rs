// SPDX-License-Identifier: CC0-1.0

//! The two-hop pipeline a `python3.x` function's own deployment package drives: `import
//! boto3`, read a secret out of Secrets Manager, send what it read to an Amazon SQS queue —
//! asserted end to end over the wire, on ONE engine, through the services that OWN each half.
//!
//! RULE: a managed `python3.x` runtime ships the AWS SDK for Python on the handler's module
//! search path, pointed at this endpoint with the function's execution-role credentials, so a
//! handler that imports it INITIALISES and runs: `Invoke` answers HTTP 200 with no
//! `X-Amz-Function-Error` header and the handler's own document as the payload, and each call
//! the handler made took effect in the service that owns it — the value Secrets Manager holds
//! for the version the read named comes back, and the message the handler sent is in the
//! queue. A refusal one of those services answers the handler with is the SERVICE's own code,
//! and a handler that catches one still completes, leaving the other service's state exactly
//! as it was.
//!
//! What grounds it: "The execution role grants the function permission to use Amazon Web
//! Services services" (`FunctionConfiguration$Role`), and a function uses them by calling
//! them with the SDK a managed runtime supplies. `GetSecretValue` resolves its `SecretId` as
//! "the ARN or name of the secret", with "a complete ARN rather than a partial ARN"
//! recommended and the partial form accepted, and answers the `AWSCURRENT` version when
//! neither `VersionStage` nor `VersionId` is given (`GetSecretValueRequest`, Secrets Manager's
//! own Smithy model); a `PutSecretValue` "moves the staging label `AWSPREVIOUS` to the version
//! that `AWSCURRENT` was removed from" (`PutSecretValueRequest$VersionStages`). A send to a
//! queue the account does not hold is `QueueDoesNotExist`, which a query-compatible service
//! puts on the wire as `AWS.SimpleQueueService.NonExistentQueue` with HTTP 400
//! (`SendMessage`'s errors, Amazon SQS's own model), and a read of a secret nothing holds is
//! `ResourceNotFoundException` with HTTP 400 (`GetSecretValue`'s errors).
//!
//! The rows differ along what the rule generalises over: WHICH FORM the handler names the
//! secret by (its name, its complete ARN, its partial ARN), WHICH VERSION the read names (none
//! at all, a staging label — the current one and the one a rotation left behind — and a
//! version's own id), and WHICH HALF of the pipeline is asked for something that is not there
//! (a secret the account does not hold, a queue it does not hold). Every row runs the SAME
//! function, so the SDK's presence is asserted once per row by the run happening at all, and
//! each row asserts both halves: what the handler reported, and what the QUEUE holds
//! afterwards. The second suite drives the rule's OTHER half — the class that answers for a
//! module nothing provides — over several module names no managed runtime ships.
//!
//! What the neighbours pin and this suite does not repeat: `handler_secret_sdk` drives the
//! value members and the key authority of a secret read, `handler_query_compatible_error_code`
//! drives the code vocabulary a refusal is reported in, and
//! `handler_sdk_service_inventory` drives which service tokens the SDK answers to. What only
//! this one reaches is the whole relay: two services, one invocation, with the effect read
//! back where a client reads it.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, call_sqs, create_queue, router_on};
use crate::invoke_request_id::{is_request_id, one_invocation_stream_at_a_time};
// The one .zip writer this crate's suites package a Python handler with, exported by the
// suite that first needed it rather than copied here.
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for, and so the Region the secret, the queue and
/// the function belong to.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The function, the role it runs as, the secret it reads and the queue it relays to.
const FUNCTION: &str = "secret-to-queue-relay";
const ROLE_NAME: &str = "secret-to-queue-relay-role";
const SECRET_NAME: &str = "secret-to-queue-relay/credentials";
const QUEUE_NAME: &str = "secret-to-queue-relay-sink";

/// A secret name the account holds nothing under, which one row's read names.
const ABSENT_SECRET: &str = "secret-to-queue-relay/nothing-holds-this";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the relay and not how fast an interpreter
/// gets through the handler.
const FUNCTION_TIMEOUT: i64 = 120;

/// How long a wait for work the clock owes may take before it is a failure, and how often to
/// look while waiting. The deadline is generous so a loaded host cannot decide the outcome.
const BUDGET: Duration = Duration::from_secs(180);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The staging labels Secrets Manager keeps a version under: "Secrets Manager uses staging
/// labels to keep track of different versions during the rotation process"
/// (`GetSecretValueRequest$VersionStage`).
const CURRENT: &str = "AWSCURRENT";
const PREVIOUS: &str = "AWSPREVIOUS";

/// The refusals the two halves answer, with the status each carries: `ResourceNotFoundException`
/// (HTTP 400, `GetSecretValue`'s errors) and `QueueDoesNotExist`, whose query-compatible code is
/// `AWS.SimpleQueueService.NonExistentQueue` (HTTP 400, `SendMessage`'s errors).
const SECRET_NOT_FOUND: &str = "ResourceNotFoundException";
const QUEUE_NOT_FOUND: &str = "AWS.SimpleQueueService.NonExistentQueue";
const REFUSAL_STATUS: u64 = 400;

/// Hands out a `ClientRequestToken` no other call of this suite uses: a raw HTTP request to
/// Secrets Manager names the version it creates itself ("you must generate a
/// ClientRequestToken and include it in the request",
/// `CreateSecretRequest$ClientRequestToken`), and that token IS the version's id.
static TOKENS: AtomicU64 = AtomicU64::new(0);

/// A fresh token inside the modelled 32..64 length bound of `ClientRequestTokenType`.
fn fresh_token() -> String {
    format!("secret-to-queue-relay-version-{:0>6}", TOKENS.fetch_add(1, Ordering::Relaxed))
}

/// The handler every row runs: build both clients during INIT, read the secret the event
/// names, relay what came back to the queue the event names, and report either the send or
/// the refusal that stopped it.
///
/// Both clients are built at module scope, which is where a managed runtime's handler builds
/// them: a runtime with no SDK on the import path never reaches the handler at all.
const RELAY_HANDLER: &str = r#"
import json

import boto3
from botocore.exceptions import ClientError

SECRETS = boto3.client("secretsmanager")
QUEUES = boto3.client("sqs")


def lambda_handler(event, context):
    try:
        held = SECRETS.get_secret_value(SecretId=event["secret_id"], **event["version"])
    except ClientError as refused:
        return _refusal("secretsmanager", refused)
    relayed = {
        "arn": held["ARN"],
        "name": held["Name"],
        "version_id": held["VersionId"],
        "stages": sorted(held["VersionStages"]),
        "secret": json.loads(held["SecretString"]),
    }
    try:
        sent = QUEUES.send_message(
            QueueUrl=event["queue_url"], MessageBody=json.dumps(relayed, sort_keys=True)
        )
    except ClientError as refused:
        return _refusal("sqs", refused)
    report = {"ok": True, "message_id": sent["MessageId"]}
    report.update(relayed)
    return report


def _refusal(service, refused):
    return {
        "ok": False,
        "service": service,
        "code": refused.response["Error"]["Code"],
        "status": refused.response["ResponseMetadata"]["HTTPStatusCode"],
    }
"#;

/// Which of the secret's two versions a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// The version the create staked, which the second write moved `AWSPREVIOUS` onto.
    Rotated,
    /// The version the second write staked, which carries `AWSCURRENT`.
    Current,
}

impl Held {
    /// The value that version holds. A JSON document, which is the shape a handler reads a
    /// credential out of a secret as.
    fn value(self) -> Value {
        match self {
            Held::Rotated => json!({"username": "relay", "password": "rotated-away"}),
            Held::Current => json!({"username": "relay", "password": "in-force"}),
        }
    }

    /// The staging labels that version carries once both writes are in.
    fn stages(self) -> Value {
        match self {
            Held::Rotated => json!([PREVIOUS]),
            Held::Current => json!([CURRENT]),
        }
    }
}

/// How the event names the secret: the forms `GetSecretValueRequest$SecretId` accepts, plus a
/// name the account holds nothing under.
#[derive(Debug, Clone, Copy)]
enum Named {
    Name,
    CompleteArn,
    PartialArn,
    AbsentName,
}

/// Which version the read names, if any.
#[derive(Debug, Clone, Copy)]
enum Version {
    /// Neither member: "If you don't specify either a VersionStage or VersionId, Secrets
    /// Manager returns the AWSCURRENT version."
    Unnamed,
    /// A staging label, by `VersionStage`.
    Stage(&'static str),
    /// One version's own id, by `VersionId`.
    Id(Held),
}

/// Which queue the handler sends to.
#[derive(Debug, Clone, Copy)]
enum Sink {
    Held,
    Absent,
}

/// What a row must come to.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The relay ran: the version whose value must come back and reach the queue.
    Relayed(Held),
    /// The run completed reporting the refusal that service answered.
    Refused {
        service: &'static str,
        code: &'static str,
    },
}

/// One row of the table.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    named: Named,
    version: Version,
    sink: Sink,
    expected: Expected,
}

/// The rows: every form a read may name its secret by, every way it may name a version, and
/// each half of the relay asked for something that is not there.
const ROWS: &[Row] = &[
    Row {
        case: "by name, naming no version: the current value reaches the queue",
        named: Named::Name,
        version: Version::Unnamed,
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Current),
    },
    Row {
        case: "by the secret's complete ARN",
        named: Named::CompleteArn,
        version: Version::Unnamed,
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Current),
    },
    Row {
        case: "by a partial ARN, which carries none of the six trailing characters",
        named: Named::PartialArn,
        version: Version::Unnamed,
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Current),
    },
    Row {
        case: "by name, naming the current staging label",
        named: Named::Name,
        version: Version::Stage(CURRENT),
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Current),
    },
    Row {
        case: "by name, naming the staging label the second write left behind",
        named: Named::Name,
        version: Version::Stage(PREVIOUS),
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Rotated),
    },
    Row {
        case: "by name, naming the rotated version's own id",
        named: Named::Name,
        version: Version::Id(Held::Rotated),
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Rotated),
    },
    Row {
        case: "by the complete ARN, naming the current version's own id",
        named: Named::CompleteArn,
        version: Version::Id(Held::Current),
        sink: Sink::Held,
        expected: Expected::Relayed(Held::Current),
    },
    Row {
        case: "a secret the account holds nothing under: the read is refused and nothing is \
               relayed",
        named: Named::AbsentName,
        version: Version::Unnamed,
        sink: Sink::Held,
        expected: Expected::Refused {
            service: "secretsmanager",
            code: SECRET_NOT_FOUND,
        },
    },
    Row {
        case: "a queue the account holds nothing under: the secret was read and the send is \
               refused",
        named: Named::Name,
        version: Version::Unnamed,
        sink: Sink::Absent,
        expected: Expected::Refused {
            service: "sqs",
            code: QUEUE_NOT_FOUND,
        },
    },
];

/// Lambda over the SAME engine as the two services the handler calls, plus the IAM store the
/// execution role lives in and the token service its session is minted by — exactly what
/// `core/router` assembles. A handler's own SDK call is dispatched back into this same router,
/// so the secret it reads and the queue it sends to are the ones this suite wrote.
fn relay_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One Secrets Manager operation over its `awsJson1_1` wire, made as the account's own
/// credential — the secret's owner, whose calls no row's grant decides.
///
/// `pub(crate)` because the suite over the SAME relay reached through an event source mapping
/// writes its secret through this one wire too
/// (`tests/all/sqs_event_source_secret_relay_pipeline.rs`), rather than carrying a second
/// spelling of it.
pub(crate) async fn secrets(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let host = format!("secretsmanager.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/secretsmanager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected Secrets Manager {operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// Create the relay function, running as `role_arn`, and wait until it is `Active`.
async fn create_relay(router: &MultiServiceRouter, case: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(RELAY_HANDLER)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction {FUNCTION:?} to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, FUNCTION).await;
}

/// `Invoke` the relay with `event`, as the `X-Amz-Function-Error` signal it carried and the
/// payload the run produced.
async fn invoke_relay(
    router: &MultiServiceRouter,
    case: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event,
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    (
        answered.header("x-amz-function-error"),
        answered.json(),
    )
}

/// Take everything the queue hands over, deleting each message so the next row starts from an
/// empty queue, and answer the bodies as the documents they carry.
///
/// `wanted` is how many the caller expects: the loop looks again until it has them or the
/// generous deadline passes, so a host that is slow to make a message available cannot decide
/// the outcome, and a caller expecting none takes one look and reports what it found.
async fn drained(router: &MultiServiceRouter, case: &str, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + BUDGET;
    let mut held = Vec::new();
    loop {
        let (status, answered) = call_sqs(
            router,
            "ReceiveMessage",
            &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ReceiveMessage to answer 200, got {status} {answered}"
        );
        let handed = answered["Messages"].as_array().cloned().unwrap_or_default();
        for message in &handed {
            let body = message["Body"].as_str().unwrap_or_default();
            held.push(serde_json::from_str(body).unwrap_or(Value::String(body.to_string())));
            let (status, deleted) = call_sqs(
                router,
                "DeleteMessage",
                &json!({
                    "QueueUrl": url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteMessage to answer 200, got {status} {deleted}"
            );
        }
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The partial form of a secret ARN: the complete one without the hyphen and the six
/// characters Secrets Manager appends to the name ("Finding a secret from a partial ARN",
/// `GetSecretValueRequest$SecretId`).
///
/// `pub(crate)` for the same reason as [`secrets`]: the suite driving this relay through an
/// event source mapping names its secret by this form too.
pub(crate) fn partial_arn(arn: &str) -> String {
    arn[..arn.len().saturating_sub(7)].to_string()
}

/// A well-formed execution role ARN no IAM store holds a record of, which is what the import
/// rows run as: nothing about a role bears on whether a module imports.
const UNKNOWN_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module names the import rows ask for, none of which a managed `python3.x` runtime
/// ships and none of which the packages below carry: a popular dependency a package has to
/// ship itself, an HTTP client no managed runtime puts on the path, and the SDK's own
/// predecessor, whose name differs from the one the runtime does supply.
const ABSENT_MODULES: &[(&str, &str)] = &[
    ("a dependency the package would have to ship", "pandas"),
    ("an HTTP client no managed runtime ships", "requests"),
    ("a near miss on the name the runtime does supply", "boto"),
];

/// The error class a module that does not import is reported as, and the signal such an
/// invocation carries.
const IMPORT_MODULE_ERROR: &str = "Runtime.ImportModuleError";
const UNHANDLED: &str = "Unhandled";

/// RULE, the other half: `Runtime.ImportModuleError` is reserved for a module neither the
/// deployment package nor the runtime provides — the invocation is still answered HTTP 200,
/// carrying the `Unhandled` signal and an error document naming the handler's module and the
/// module that was missing. So the class that answers for `boto3` on a runtime without the
/// SDK answers here, and for the SDK's own modules it does not.
#[tokio::test]
async fn an_import_of_a_module_nothing_provides_is_the_import_module_error() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Every function is created first, so the windows their creates opened run down together
    // rather than one after another.
    for (index, (case, missing)) in ABSENT_MODULES.iter().enumerate() {
        let source = format!(
            "import {missing}\n\n\ndef lambda_handler(event, context):\n    return {{\"ok\": True}}\n"
        );
        let created = call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &json!({
                "FunctionName": absent_import_function(index),
                "Role": UNKNOWN_ROLE,
                "Runtime": "python3.12",
                "Handler": HANDLER,
                "Timeout": FUNCTION_TIMEOUT,
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(&source)),
                },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected CreateFunction to answer 201, got {} {}",
            created.status,
            created.text()
        );
    }

    for (index, (case, missing)) in ABSENT_MODULES.iter().enumerate() {
        let name = absent_import_function(index);
        await_function_active(&router, case, &name).await;
        let answered = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/invocations"),
            &json!({}),
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: a run that failed to import is still answered 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error").as_deref(),
            Some(UNHANDLED),
            "case {case:?}: expected the Unhandled signal, got {:?}",
            answered.header("x-amz-function-error")
        );
        let document = answered.json();
        assert_eq!(
            document["errorType"],
            json!(IMPORT_MODULE_ERROR),
            "case {case:?}: expected the import error class, got {document}"
        );
        assert_eq!(
            document["errorMessage"],
            json!(format!(
                "Unable to import module 'lambda_function': No module named '{missing}'"
            )),
            "case {case:?}: expected the message to name the handler's module and the module \
             that was missing, got {document}"
        );
        assert!(
            document["stackTrace"].is_array(),
            "case {case:?}: expected the error document to carry a stack, got {document}"
        );
    }
}

/// The function one import row runs.
fn absent_import_function(index: usize) -> String {
    format!("absent-import-{index}")
}

/// RULE: a managed `python3.x` runtime supplies the AWS SDK on the handler's import path with
/// the function's execution-role credentials, so a handler that imports it, reads a secret and
/// sends a message RUNS — the invocation is answered 200 with no function error and the
/// handler's own document — and each call took effect in the service that owns it: the value
/// held for the version the read named comes back, and the message is in the queue. A refusal
/// either service answers is reported with that service's own code, and leaves the other
/// service's state as it was.
#[tokio::test]
async fn a_handler_relays_the_secret_it_reads_to_its_queue() {
    let _stream = one_invocation_stream_at_a_time().await;
    let setup = "setup";
    let router = relay_router();

    // The queue the relay sends to, and a url of the same form naming a queue the account
    // holds nothing under.
    let (queue_url, queue_arn) = create_queue(&router, setup, QUEUE_NAME, &json!({})).await;
    let absent_url = format!("{queue_url}-absent");

    // The role the function runs as: a least-privilege relay's own grants, read and send.
    let role_arn = given_role(
        &router,
        setup,
        ROLE_NAME,
        Some(json!([
            {
                "Effect": "Allow",
                "Action": "secretsmanager:GetSecretValue",
                "Resource": "*",
            },
            {"Effect": "Allow", "Action": "sqs:SendMessage", "Resource": queue_arn},
        ])),
    )
    .await;

    // The secret, with two versions: the create stakes the first, and the write that follows
    // takes `AWSCURRENT` onto the second and moves `AWSPREVIOUS` onto the first.
    let rotated_version = fresh_token();
    let created = secrets(
        &router,
        setup,
        "CreateSecret",
        &json!({
            "Name": SECRET_NAME,
            "ClientRequestToken": rotated_version,
            "SecretString": Held::Rotated.value().to_string(),
        }),
    )
    .await;
    let secret_arn = created["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {setup:?}: CreateSecret reported no ARN in {created}"))
        .to_string();
    let current_version = fresh_token();
    secrets(
        &router,
        setup,
        "PutSecretValue",
        &json!({
            "SecretId": SECRET_NAME,
            "ClientRequestToken": current_version,
            "SecretString": Held::Current.value().to_string(),
        }),
    )
    .await;

    create_relay(&router, setup, &role_arn).await;

    for row in ROWS {
        let case = row.case;
        let secret_id = match row.named {
            Named::Name => SECRET_NAME.to_string(),
            Named::CompleteArn => secret_arn.clone(),
            Named::PartialArn => partial_arn(&secret_arn),
            Named::AbsentName => ABSENT_SECRET.to_string(),
        };
        let version = match row.version {
            Version::Unnamed => json!({}),
            Version::Stage(label) => json!({"VersionStage": label}),
            Version::Id(Held::Rotated) => json!({"VersionId": rotated_version}),
            Version::Id(Held::Current) => json!({"VersionId": current_version}),
        };
        let url = match row.sink {
            Sink::Held => queue_url.clone(),
            Sink::Absent => absent_url.clone(),
        };
        let event = json!({
            "secret_id": secret_id,
            "version": version,
            "queue_url": url,
        });

        let (signal, payload) = invoke_relay(&router, case, &event).await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler must run to completion, got the function error \
             {signal:?} with {payload}"
        );

        match row.expected {
            Expected::Relayed(held) => {
                let version_id = match held {
                    Held::Rotated => &rotated_version,
                    Held::Current => &current_version,
                };
                let relayed = json!({
                    "arn": secret_arn,
                    "name": SECRET_NAME,
                    "version_id": version_id,
                    "stages": held.stages(),
                    "secret": held.value(),
                });
                let mut expected = relayed.clone();
                expected["ok"] = json!(true);
                expected["message_id"] = payload["message_id"].clone();
                assert_eq!(
                    payload, expected,
                    "case {case:?}: expected the handler's own report {expected}, got {payload}"
                );
                // The id the send answered is the queue's own, which is a UUID like every
                // other identifier this emulator mints.
                let message_id = payload["message_id"].as_str().unwrap_or_default();
                assert!(
                    is_request_id(message_id),
                    "case {case:?}: expected the MessageId Amazon SQS minted, got \
                     {message_id:?} in {payload}"
                );
                // And what the QUEUE holds: the one document the handler sent, read back
                // where a client reads it.
                let messages = drained(&router, case, &queue_url, 1).await;
                assert_eq!(
                    messages,
                    vec![relayed.clone()],
                    "case {case:?}: expected the queue to hold the relayed document \
                     {relayed}, got {messages:?}"
                );
            }
            Expected::Refused { service, code } => {
                assert_eq!(
                    payload,
                    json!({
                        "ok": false,
                        "service": service,
                        "code": code,
                        "status": REFUSAL_STATUS,
                    }),
                    "case {case:?}: expected the handler to report the refusal {service} \
                     answered, got {payload}"
                );
                // A refused call relayed nothing: the queue is as it was.
                let messages = drained(&router, case, &queue_url, 0).await;
                assert!(
                    messages.is_empty(),
                    "case {case:?}: expected the queue to stay empty, got {messages:?}"
                );
            }
        }
    }
}
