// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK for JavaScript v3 a `nodejs*` runtime BUNDLES, imported as an ES module by a
//! handler that ships nothing of its own: Lambda, Amazon SQS and Amazon DynamoDB on ONE
//! engine, an `index.mjs` whose first lines are
//! `import { SQSClient, GetQueueUrlCommand, QueueDoesNotExist } from '@aws-sdk/client-sqs'`
//! and `import manifest from '@aws-sdk/client-sqs/package.json' with { type: 'json' }`, and
//! every probe it makes asserted against the service that answered it.
//!
//! The rule pinned here: AWS's managed Node.js runtimes ship the AWS SDK for JavaScript v3 in
//! the execution environment, so a handler resolves an `@aws-sdk/*` package — and that
//! package's own `package.json` subpath — with nothing in the deployment package, and an ES
//! module's `import { … } from` of one binds the names that package exports: its client, a
//! command class per operation the service binds, and an error class per error shape it
//! declares. A refusal whose answer names a shape that model DECLARES is thrown as an instance
//! of the class that shape names, under the modelled Smithy name of the answer's `__type`; one
//! naming a shape it declares none for has no class to be thrown as, so it arrives as the
//! service's own base exception under the code the answer reported — for Amazon SQS, whose
//! refusals carry the legacy query vocabulary beside the modelled shape, the legacy code of the
//! answer's own `x-amzn-query-error` header. Either way the answer's HTTP status is in
//! `$metadata`. A specifier the runtime cannot resolve at all fails the initialization phase as
//! `Runtime.ImportModuleError`.
//!
//! The rows differ along what the rule generalises over: which operation is called, whether
//! the service serves or refuses it, which error shape the refusal names, whether the service's
//! model declares a class for it, which package of the SDK is imported, and which form an
//! unresolvable specifier takes. No row states an error code or a status of its own: for every
//! refusal the handler's report is compared with what the SAME request answers on the wire, so
//! the names a JS consumer sees are the emulated service's own.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` these functions carry: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// Lambda, Amazon SQS and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the handler's SDK call is dispatched back into this same router, so the queue
/// and the table it reaches are the ones this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
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

/// One AWS JSON call to a service of this emulator, as its status and parsed body.
async fn aws_json(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _headers, parsed) = aws_json_answer(router, service, target, body).await;
    (status, parsed)
}

/// The same call, with the answer's HEADERS as well: a query-compatible service names the
/// legacy error code of a refusal in one of them.
async fn aws_json_answer(
    router: &MultiServiceRouter,
    service: &str,
    target: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(service))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", target)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) =
        aws_json(router, "sqs", &format!("AmazonSQS.{operation}"), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon DynamoDB call that must answer 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = aws_json(
        router,
        "dynamodb",
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} with {parsed}"
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

/// A .zip deployment package holding `source` as `index.mjs` — one ES module, and nothing
/// else: no `node_modules`, so every `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file("index.mjs", options).expect("the entry starts");
        writer.write_all(source.as_bytes()).expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a `nodejs22.x` function whose code is `source` and wait out the provisioning window.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str, source: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
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

/// `Invoke` the function synchronously with `event`, answering the `FunctionError` signal the
/// response carried and the payload the run produced.
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

/// The handler the SQS rows run: an ES module importing the bundled client by NAME, which is
/// what a JS v3 consumer writes, and reporting per probe what the SDK gave it.
///
/// The probes are named by the event, so one package serves every row.
const SQS_HANDLER: &str = r#"
import {
    SQSClient,
    GetQueueUrlCommand,
    ReceiveMessageCommand,
    SendMessageCommand,
    GetQueueAttributesCommand,
    QueueDoesNotExist,
    SQSServiceException,
} from "@aws-sdk/client-sqs";
import manifest from "@aws-sdk/client-sqs/package.json" with { type: "json" };

const COMMANDS = {
    GetQueueUrl: GetQueueUrlCommand,
    ReceiveMessage: ReceiveMessageCommand,
    SendMessage: SendMessageCommand,
    GetQueueAttributes: GetQueueAttributesCommand,
};

export const handler = async (event) => {
    const client = new SQSClient({ region: event.region });
    const probes = {};
    for (const probe of event.probes) {
        const Command = COMMANDS[probe.operation];
        if (Command === undefined) {
            probes[probe.case] = { bound: false };
            continue;
        }
        try {
            const answered = await client.send(new Command(probe.input));
            probes[probe.case] = {
                ok: true,
                httpStatusCode: answered.$metadata.httpStatusCode,
                members: Object.keys(answered)
                    .filter((name) => name !== "$metadata")
                    .sort(),
            };
        } catch (error) {
            probes[probe.case] = {
                ok: false,
                ctor: error.constructor.name,
                name: error.name,
                message: error.message,
                isQueueDoesNotExist: error instanceof QueueDoesNotExist,
                isServiceException: error instanceof SQSServiceException,
                isError: error instanceof Error,
                httpStatusCode: error.$metadata === undefined
                    ? null
                    : error.$metadata.httpStatusCode,
            };
        }
    }
    return {
        package: manifest.name,
        version: manifest.version,
        client: SQSClient.name,
        probes,
    };
};
"#;

/// One probe the handler makes: the operation, the input, and whether the service serves it.
struct Probe {
    case: &'static str,
    /// The operation the command addresses, which is the command's own name without its
    /// suffix.
    operation: &'static str,
    /// Whether the service must refuse it.
    refused: bool,
    /// Whether the shape the refusal names is one Amazon SQS's own model DECLARES — so the
    /// package exports a class for it and a v3 client throws that class — or one it declares
    /// none for, which a client has only the base exception for. `None` for a row the service
    /// SERVES, which names no error shape at all.
    modelled: Option<bool>,
}

/// The probes, one per thing the rule generalises over: a read refused for a queue that is
/// not there, a read of a queue that is, a write refused on a value outside the model's
/// range, and a write that is served.
fn probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "GetQueueUrl on a name no queue has",
            operation: "GetQueueUrl",
            refused: true,
            // `QueueDoesNotExist` is one of `GetQueueUrl`'s own errors
            // (`smithy_query(query="operation", operation="GetQueueUrl")`).
            modelled: Some(true),
        },
        Probe {
            case: "ReceiveMessage on a URL no queue has",
            operation: "ReceiveMessage",
            refused: true,
            modelled: Some(true),
        },
        Probe {
            case: "GetQueueAttributes on a URL no queue has",
            operation: "GetQueueAttributes",
            refused: true,
            modelled: Some(true),
        },
        Probe {
            // `DelaySeconds` is `@range(min: 0, max: 900)`
            // (`smithy_query(query="operation", operation="SendMessage")`), so 901 is a value
            // the service refuses whatever queue it names. The refusal it answers names no
            // shape that model declares — `SendMessage`'s errors are `InvalidAddress`,
            // `InvalidMessageContents`, `InvalidSecurity`, the `Kms*` family,
            // `QueueDoesNotExist`, `RequestThrottled` and `UnsupportedOperation`, and none of
            // them is it — so a v3 client has no class for it.
            case: "SendMessage with a DelaySeconds past the range the model allows",
            operation: "SendMessage",
            refused: true,
            modelled: Some(false),
        },
        Probe {
            case: "GetQueueUrl on the queue's own name",
            operation: "GetQueueUrl",
            refused: false,
            modelled: None,
        },
        Probe {
            case: "SendMessage to the queue",
            operation: "SendMessage",
            refused: false,
            modelled: None,
        },
        Probe {
            case: "GetQueueAttributes on the queue",
            operation: "GetQueueAttributes",
            refused: false,
            modelled: None,
        },
    ]
}

/// The body of the message the served `SendMessage` probe puts on the queue, read back off the
/// queue afterwards.
const SENT_BODY: &str = "sent by an ES module handler";

/// The input each probe carries, built against the queue the run goes over.
fn input(case: &str, queue_url: &str, queue_name: &str) -> Value {
    match case {
        "GetQueueUrl on a name no queue has" => json!({ "QueueName": format!("{queue_name}-absent") }),
        "ReceiveMessage on a URL no queue has" => {
            json!({ "QueueUrl": format!("{queue_url}-absent") })
        }
        "GetQueueAttributes on a URL no queue has" => json!({
            "QueueUrl": format!("{queue_url}-absent"),
            "AttributeNames": ["QueueArn"],
        }),
        "SendMessage with a DelaySeconds past the range the model allows" => json!({
            "QueueUrl": queue_url,
            "MessageBody": "delayed too long",
            "DelaySeconds": 901,
        }),
        "GetQueueUrl on the queue's own name" => json!({ "QueueName": queue_name }),
        "SendMessage to the queue" => {
            json!({ "QueueUrl": queue_url, "MessageBody": SENT_BODY })
        }
        _ => json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    }
}

/// The error shape and status the SAME request answers with on the wire, which is what the
/// handler's report is compared against: the shape of the answer's `__type`
/// (`<namespace>#<shape>`), the LEGACY code it names in `x-amzn-query-error` (`<code>;<fault>`),
/// its message, and the HTTP status.
async fn refusal_on_the_wire(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> (String, String, String, u16) {
    let (status, headers, answered) =
        aws_json_answer(router, "sqs", &format!("AmazonSQS.{operation}"), body).await;
    assert!(
        status >= 400,
        "case {case:?}: this probe is a refusal, so the same request must be refused on the \
         wire, got {status} with {answered}"
    );
    let shape = answered["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the answer to name its error shape \
                                   in __type, got {answered}"))
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string();
    let legacy = headers
        .get("x-amzn-query-error")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let message = answered["message"]
        .as_str()
        .or_else(|| answered["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    (shape, legacy, message, status)
}

/// The bundled client, imported by name from an ES module, calls the service and reports its
/// own answer — per probe, with every refusal compared against the wire.
#[tokio::test]
async fn an_es_module_handler_imports_the_bundled_sdk_and_reads_the_services_own_answers() {
    let router = router();
    let case = "an ES module handler driving the bundled client";
    let queue_name = "esm-sdk-probe";
    let created = sqs(&router, case, "CreateQueue", &json!({ "QueueName": queue_name })).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();

    create_function(&router, case, "esm-sdk-caller", SQS_HANDLER).await;
    let asked: Vec<Value> = probes()
        .iter()
        .map(|probe| {
            json!({
                "case": probe.case,
                "operation": probe.operation,
                "input": input(probe.case, &queue_url, queue_name),
            })
        })
        .collect();
    let (signal, payload) = invoke(
        &router,
        case,
        "esm-sdk-caller",
        &json!({ "region": REGION, "probes": asked }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    assert_eq!(
        payload["package"],
        json!("@aws-sdk/client-sqs"),
        "case {case:?}: expected the imported manifest to name the package, got {payload}"
    );
    assert!(
        payload["version"]
            .as_str()
            .is_some_and(|version| version.starts_with("3.")),
        "case {case:?}: expected the manifest to state the SDK generation it implements, got \
         {payload}"
    );
    assert_eq!(
        payload["client"],
        json!("SQSClient"),
        "case {case:?}: expected the client class to carry the name its package exports it \
         under, got {payload}"
    );

    for probe in probes() {
        let case = probe.case;
        let reported = &payload["probes"][case];
        assert!(
            reported.is_object(),
            "case {case:?}: expected the handler to report this probe, got {payload}"
        );
        assert_eq!(
            reported["ok"],
            json!(!probe.refused),
            "case {case:?}: expected ok={}, got {reported}",
            !probe.refused
        );
        if !probe.refused {
            assert!(
                reported["members"]
                    .as_array()
                    .is_some_and(|members| !members.is_empty()),
                "case {case:?}: expected the answer to carry the operation's own output \
                 members, got {reported}"
            );
            assert_eq!(
                reported["httpStatusCode"],
                json!(200),
                "case {case:?}: expected the served call to report HTTP 200, got {reported}"
            );
            continue;
        }
        let body = input(case, &queue_url, queue_name);
        let (shape, legacy, message, status) =
            refusal_on_the_wire(&router, case, probe.operation, &body).await;
        // WHICH vocabulary a JS consumer reads the refusal under: the modelled shape when the
        // service's model declares a class for it, and otherwise — having no class to throw —
        // the legacy code the answer's own header carried, on the package's base exception.
        let (expected_name, expected_class) = if probe.modelled == Some(true) {
            (shape.clone(), shape.clone())
        } else {
            assert!(
                !legacy.is_empty(),
                "case {case:?}: this service answers the legacy code of every refusal in \
                 x-amzn-query-error, got none beside the shape {shape:?}"
            );
            (legacy.clone(), "SQSServiceException".to_string())
        };
        assert_eq!(
            reported["name"],
            json!(expected_name),
            "case {case:?}: expected the code a v3 consumer reports for the answer's shape \
             ({shape}) to be {expected_name}, got {reported}"
        );
        assert_eq!(
            reported["ctor"],
            json!(expected_class),
            "case {case:?}: expected the refusal to be an instance of {expected_class}, got \
             {reported}"
        );
        assert_eq!(
            reported["message"],
            json!(message),
            "case {case:?}: expected the service's own message, got {reported}"
        );
        assert_eq!(
            reported["httpStatusCode"],
            json!(status),
            "case {case:?}: expected the answer's own HTTP status, got {reported}"
        );
        assert_eq!(
            reported["isQueueDoesNotExist"],
            json!(shape == "QueueDoesNotExist"),
            "case {case:?}: expected `instanceof QueueDoesNotExist` to answer for the shape \
             {shape} and nothing else, got {reported}"
        );
        assert_eq!(
            reported["isServiceException"],
            json!(true),
            "case {case:?}: expected every refusal to be an instance of the service's own base \
             exception, got {reported}"
        );
        assert_eq!(
            reported["isError"],
            json!(true),
            "case {case:?}: expected the refusal to be an Error, got {reported}"
        );
    }

    // What the QUEUE holds: the one probe that was served as a write is the one message on it.
    let drained = sqs(
        &router,
        case,
        "ReceiveMessage",
        &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    let held: Vec<&str> = drained["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message["Body"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        held,
        vec![SENT_BODY],
        "expected the served write and nothing else on the queue, got {drained}"
    );
}

/// The document client and the value conversions, imported by name from an ES module: one
/// item written and read back through them, and the item read where a client reads it.
const DOCUMENT_HANDLER: &str = r#"
import { DynamoDBClient } from "@aws-sdk/client-dynamodb";
import { DynamoDBDocumentClient, PutCommand, GetCommand } from "@aws-sdk/lib-dynamodb";
import { marshall, unmarshall } from "@aws-sdk/util-dynamodb";

export const handler = async (event) => {
    const document = DynamoDBDocumentClient.from(new DynamoDBClient({ region: event.region }));
    await document.send(new PutCommand({ TableName: event.table, Item: event.item }));
    const read = await document.send(new GetCommand({
        TableName: event.table,
        Key: { id: event.item.id },
    }));
    return {
        item: read.Item,
        roundTripped: unmarshall(marshall(event.item)),
    };
};
"#;

/// Every package of the SDK this runtime bundles is importable as an ES module by the names it
/// exports — the client, the document client and its commands, and the value conversions —
/// and what they write is what the owning service holds.
#[tokio::test]
async fn an_es_module_handler_imports_every_bundled_package_by_name() {
    let router = router();
    let case = "the document client and the value conversions, imported by name";
    let table = "esm-sdk-items";
    dynamodb(
        &router,
        case,
        "CreateTable",
        &json!({
            "TableName": table,
            "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "id", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    create_function(&router, case, "esm-document-caller", DOCUMENT_HANDLER).await;
    // "You can perform read and write operations only on an ACTIVE table", so the write waits
    // for the status the clock owes rather than racing it.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let described =
            dynamodb(&router, case, "DescribeTable", &json!({ "TableName": table })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(SLACK).await;
    }
    let item = json!({ "id": "one", "count": 3, "tags": ["a", "b"], "live": true });
    let (signal, payload) = invoke(
        &router,
        case,
        "esm-document-caller",
        &json!({ "region": REGION, "table": table, "item": item }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    assert_eq!(
        payload["item"], item,
        "case {case:?}: expected the document client to read back the item it wrote, got \
         {payload}"
    );
    assert_eq!(
        payload["roundTripped"], item,
        "case {case:?}: expected the value conversions to round-trip the item, got {payload}"
    );
    // What the TABLE holds, read where a client reads it.
    let read = dynamodb(
        &router,
        case,
        "GetItem",
        &json!({ "TableName": table, "Key": { "id": { "S": "one" } } }),
    )
    .await;
    assert_eq!(
        read["Item"]["count"],
        json!({ "N": "3" }),
        "case {case:?}: expected the item on the table to carry what the handler wrote, got \
         {read}"
    );
}

/// A handler whose import names a module this runtime cannot resolve fails the way node's own
/// ES module resolver reports it: a BARE specifier names the package that is nowhere and a
/// PATH specifier names the file that was looked for, each beside the module the import was
/// written in, and neither under the class a failed `require` is reported under — one row per
/// form an unresolvable specifier takes.
#[tokio::test]
async fn an_import_of_a_module_the_runtime_cannot_resolve_fails_as_the_resolvers_own_error() {
    // (case, the specifier, the message node's resolver states for it)
    let cases: &[(&str, &str, &str)] = &[
        (
            "a scoped package no runtime supplies",
            "@aws-sdk/lib-absent",
            "Cannot find package '@aws-sdk/lib-absent' imported from /var/task/index.mjs",
        ),
        (
            "a bare package the deployment package does not hold",
            "left-pad",
            "Cannot find package 'left-pad' imported from /var/task/index.mjs",
        ),
        (
            "a relative module the deployment package does not hold",
            "./absent.mjs",
            "Cannot find module '/var/task/absent.mjs' imported from /var/task/index.mjs",
        ),
        (
            "a directory of the package that is not there",
            "./lib/absent/",
            "Cannot find module '/var/task/lib/absent' imported from /var/task/index.mjs",
        ),
    ];
    let router = router();
    for (index, (case, specifier, stated)) in cases.iter().enumerate() {
        let name = format!("esm-absent-{index}");
        let source =
            format!("import held from \"{specifier}\";\nexport const handler = async () => held;\n");
        create_function(&router, case, &name, &source).await;
        let (signal, payload) = invoke(&router, case, &name, &json!({})).await;
        assert_eq!(
            signal.as_deref(),
            Some("Unhandled"),
            "case {case:?}: expected the function error signal, got {signal:?} with {payload}"
        );
        assert_eq!(
            payload["errorType"],
            json!("Error"),
            "case {case:?}: expected the class node's module resolver reports a resolution \
             failure under, got {payload}"
        );
        assert_eq!(
            payload["errorMessage"],
            json!(stated),
            "case {case:?}: expected {stated:?}, got {payload}"
        );
    }
}
