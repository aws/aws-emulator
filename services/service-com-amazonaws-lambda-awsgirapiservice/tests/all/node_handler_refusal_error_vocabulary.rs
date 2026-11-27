// SPDX-License-Identifier: CC0-1.0

//! WHICH error vocabulary a refusal reaches an in-sandbox AWS SDK for JavaScript v3 consumer
//! under: Lambda, Amazon SQS and Amazon DynamoDB on ONE engine, a `nodejs22.x` handler that
//! imports the runtime's bundled `@aws-sdk/client-*` packages, and every refusal it catches
//! held against what the SAME request answers on the wire.
//!
//! The rule pinned here: a v3 client's error classes are the called service's own modelled
//! error shapes, so a refusal naming one of them is thrown as that class under that shape's
//! name; a refusal naming a shape the model declares none for has no class to be thrown as, so
//! it arrives as the package's base exception (`<Service>ServiceException`) carrying the code
//! the answer reported — and for a service whose refusals carry the LEGACY query vocabulary
//! beside the modelled shape (the `awsQueryCompatible` protocol modifier, which makes one
//! refusal name its modern shape in the body's `__type` and its legacy code in the
//! `x-amzn-query-error` header), that code is the legacy one, which is the vocabulary such a
//! service's consumers match on.
//!
//! The rows differ along what the rule generalises over: whether the called service's refusals
//! carry two vocabularies or one, whether its model declares a class for the shape the refusal
//! names, which operation is refused and which member's value earns the refusal. No row states
//! a code, a sentence or a status of its own — each is read off the wire answer to the same
//! request — and each row's only stated fact is whether the shape that answer names is one its
//! service's model declares.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's clients are built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` this function carries: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// Lambda, Amazon SQS and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the handler's SDK calls are dispatched back into this same router, so the
/// refusals it catches are the ones this suite reads off the wire.
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

/// One AWS JSON call to a service of this emulator, as its status, its headers and its parsed
/// body: a query-compatible service names the legacy code of a refusal in a header, so the
/// headers are part of the answer this suite reads.
async fn aws_json(
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
            "Timeout": 20,
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

/// The handler every row runs: it builds the client of the service the probe names, sends the
/// one command it names, and reports what the SDK threw — the class, the name, the message, the
/// status, and whether the package's base exception and the one modelled class each row may name
/// catch it.
const HANDLER_SOURCE: &str = r#"
import { SQSClient, QueueDoesNotExist, SQSServiceException } from "@aws-sdk/client-sqs";
import * as sqs from "@aws-sdk/client-sqs";
import {
    DynamoDBClient,
    ResourceNotFoundException,
    DynamoDBServiceException,
} from "@aws-sdk/client-dynamodb";
import * as dynamodb from "@aws-sdk/client-dynamodb";

const SERVICES = {
    sqs: {
        Client: SQSClient,
        package: sqs,
        base: SQSServiceException,
        modelled: { QueueDoesNotExist: QueueDoesNotExist },
    },
    dynamodb: {
        Client: DynamoDBClient,
        package: dynamodb,
        base: DynamoDBServiceException,
        modelled: { ResourceNotFoundException: ResourceNotFoundException },
    },
};

export const handler = async (event) => {
    const probes = {};
    for (const probe of event.probes) {
        const held = SERVICES[probe.service];
        const Command = held === undefined
            ? undefined
            : held.package[probe.operation + "Command"];
        if (held === undefined || Command === undefined) {
            probes[probe.case] = { bound: false };
            continue;
        }
        const client = new held.Client({ region: event.region });
        try {
            const answered = await client.send(new Command(probe.input));
            probes[probe.case] = {
                threw: false,
                httpStatusCode: answered.$metadata.httpStatusCode,
            };
        } catch (error) {
            const named = probe.names === undefined ? undefined : held.modelled[probe.names];
            probes[probe.case] = {
                threw: true,
                ctor: error.constructor.name,
                name: error.name,
                message: error.message,
                isBase: error instanceof held.base,
                isError: error instanceof Error,
                // Whether the class the row names — the one the service's model declares for
                // the shape it expects — catches what was thrown.
                isNamed: named === undefined ? null : error instanceof named,
                // Whether the package exports a class under the name the answer's own document
                // named, which is what decides the two vocabularies apart.
                exportsShape: typeof held.package[probe.shape],
                httpStatusCode: error.$metadata === undefined
                    ? null
                    : error.$metadata.httpStatusCode,
            };
        }
    }
    return { probes };
};
"#;

/// One probe: the service, the operation, the input, and the one stated fact — whether the
/// error shape the answer names is one that service's own model DECLARES.
struct Probe {
    case: &'static str,
    /// The token the client package of the called service is published under, which is what the
    /// handler builds its client from.
    service: &'static str,
    operation: &'static str,
    /// Whether the shape the refusal names is one the called service's model declares a class
    /// for.
    modelled: bool,
    /// The modelled class the handler checks `instanceof` against, where the row expects one.
    names: Option<&'static str>,
}

/// The rows: a refusal of a query-compatible service that its model DECLARES a shape for, two
/// that it declares none for (one per direction of the same member's range), and the same pair
/// for a service whose refusals carry ONE vocabulary.
const PROBES: &[Probe] = &[
    Probe {
        // `QueueDoesNotExist` is one of `GetQueueUrl`'s own errors, with the legacy code
        // `AWS.SimpleQueueService.NonExistentQueue`
        // (`smithy_query(query="operation", operation="GetQueueUrl")`).
        case: "a queue the service does not hold, read through a query-compatible service",
        service: "sqs",
        operation: "GetQueueUrl",
        modelled: true,
        names: Some("QueueDoesNotExist"),
    },
    Probe {
        // `SendMessage`'s own errors are `InvalidAddress`, `InvalidMessageContents`,
        // `InvalidSecurity`, the `Kms*` family, `QueueDoesNotExist`, `RequestThrottled` and
        // `UnsupportedOperation` (`smithy_query(query="operation", operation="SendMessage")`):
        // a value outside `DelaySeconds`'s own range is none of them.
        case: "a DelaySeconds above the range the model allows",
        service: "sqs",
        operation: "SendMessage",
        modelled: false,
        names: None,
    },
    Probe {
        case: "a DelaySeconds below the range the model allows",
        service: "sqs",
        operation: "SendMessage",
        modelled: false,
        names: None,
    },
    Probe {
        // `ResourceNotFoundException` is one of `GetItem`'s own errors
        // (`smithy_query(query="operation", operation="GetItem",
        // model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/\
        // com-amazonaws-dynamodb-dynamodb-20120810.json")`).
        case: "a table the service does not hold",
        service: "dynamodb",
        operation: "GetItem",
        modelled: true,
        names: Some("ResourceNotFoundException"),
    },
    Probe {
        // The refusal of a malformed request names no shape that model declares — DynamoDB's
        // model declares no `ValidationException` at all, which is why the service answers it
        // under its own namespace.
        case: "a table name shorter than the model's own bound",
        service: "dynamodb",
        operation: "GetItem",
        modelled: false,
        names: None,
    },
];

/// The input each probe carries, built against the queue and the table the run goes over.
fn input(case: &str, queue_url: &str, queue_name: &str, table: &str) -> Value {
    match case {
        "a queue the service does not hold, read through a query-compatible service" => {
            json!({ "QueueName": format!("{queue_name}-absent") })
        }
        "a DelaySeconds above the range the model allows" => json!({
            "QueueUrl": queue_url,
            "MessageBody": "delayed past the range",
            "DelaySeconds": 901,
        }),
        "a DelaySeconds below the range the model allows" => json!({
            "QueueUrl": queue_url,
            "MessageBody": "delayed before the range",
            "DelaySeconds": -1,
        }),
        "a table the service does not hold" => json!({
            "TableName": format!("{table}-absent"),
            "Key": { "id": { "S": "one" } },
        }),
        _ => json!({ "TableName": "ab", "Key": { "id": { "S": "one" } } }),
    }
}

/// The `X-Amz-Target` prefix each service names its operations under
/// (`smithy_query(query="service_info")`: the service shape's own name).
fn target_prefix(service: &str) -> &'static str {
    match service {
        "sqs" => "AmazonSQS",
        _ => "DynamoDB_20120810",
    }
}

/// What the SAME request answers on the wire: the shape its document names in `__type`, the
/// legacy code its `x-amzn-query-error` header names (empty for a service that carries one
/// vocabulary), its message and its status.
async fn refusal_on_the_wire(
    router: &MultiServiceRouter,
    probe: &Probe,
    body: &Value,
) -> (String, String, String, u16) {
    let case = probe.case;
    let (status, headers, answered) = aws_json(
        router,
        probe.service,
        &format!("{}.{}", target_prefix(probe.service), probe.operation),
        body,
    )
    .await;
    assert!(
        status >= 400,
        "case {case:?}: this probe is a refusal, so the same request must be refused on the \
         wire, got {status} with {answered}"
    );
    let shape = answered["__type"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the answer to name its error shape in __type, got \
                    {answered}")
        })
        .split('#')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
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

/// The base exception class the package of `service` exports, which is the class a refusal with
/// no modelled class of its own is thrown as: the client's own name without its `Client`
/// suffix, with `ServiceException` after it.
fn base_exception(service: &str) -> &'static str {
    match service {
        "sqs" => "SQSServiceException",
        _ => "DynamoDBServiceException",
    }
}

/// RULE: the class and the code a refusal reaches a bundled v3 client under are the called
/// service's own — the modelled class for a shape its model declares, and otherwise the
/// package's base exception under the code the answer reported, which for a query-compatible
/// service is the legacy code its header carried.
#[tokio::test]
async fn a_refusal_reaches_a_bundled_v3_client_under_the_vocabulary_its_service_answers() {
    let router = router();
    let case = "the vocabulary a bundled v3 client reports a refusal under";
    let queue_name = "error-vocabulary-probe";
    let table = "error-vocabulary-items";

    let (status, _headers, created) = aws_json(
        &router,
        "sqs",
        "AmazonSQS.CreateQueue",
        &json!({ "QueueName": queue_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateQueue, got {status} ({created})"
    );
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();

    let (status, _headers, table_created) = aws_json(
        &router,
        "dynamodb",
        "DynamoDB_20120810.CreateTable",
        &json!({
            "TableName": table,
            "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "id", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateTable, got {status} ({table_created})"
    );

    create_function(&router, case, "error-vocabulary-caller", HANDLER_SOURCE).await;

    // What the wire answers for every row, read BEFORE the handler runs so each row's
    // expectation is the service's own answer and not a code this suite states.
    let mut expected = Vec::new();
    for probe in PROBES {
        let body = input(probe.case, &queue_url, queue_name, table);
        let answered = refusal_on_the_wire(&router, probe, &body).await;
        expected.push((probe, body, answered));
    }

    let asked: Vec<Value> = expected
        .iter()
        .map(|(probe, body, (shape, _legacy, _message, _status))| {
            json!({
                "case": probe.case,
                "service": probe.service,
                "operation": probe.operation,
                "input": body,
                "names": probe.names,
                "shape": shape,
            })
        })
        .collect();
    let (signal, payload) = invoke(
        &router,
        case,
        "error-vocabulary-caller",
        &json!({ "region": REGION, "probes": asked }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );

    for (probe, _body, (shape, legacy, message, status)) in &expected {
        let case = probe.case;
        let reported = &payload["probes"][case];
        assert!(
            reported.is_object(),
            "case {case:?}: expected the handler to report this probe, got {payload}"
        );
        assert_eq!(
            reported["threw"],
            json!(true),
            "case {case:?}: expected the call to be refused, got {reported}"
        );
        // WHICH vocabulary the client reports it under, and WHICH class it is thrown as.
        let (expected_name, expected_class) = if probe.modelled {
            (shape.clone(), shape.clone())
        } else {
            (
                if legacy.is_empty() {
                    shape.clone()
                } else {
                    legacy.clone()
                },
                base_exception(probe.service).to_string(),
            )
        };
        assert_eq!(
            reported["name"],
            json!(expected_name),
            "case {case:?}: expected the code a v3 consumer reports (the wire answer named \
             shape {shape:?}, legacy code {legacy:?}) to be {expected_name:?}, got {reported}"
        );
        assert_eq!(
            reported["ctor"],
            json!(expected_class),
            "case {case:?}: expected the refusal to be thrown as {expected_class:?}, got \
             {reported}"
        );
        assert_eq!(
            reported["isBase"],
            json!(true),
            "case {case:?}: expected every refusal to be an instance of \
             {:?}, got {reported}",
            base_exception(probe.service)
        );
        assert_eq!(
            reported["isError"],
            json!(true),
            "case {case:?}: expected the refusal to be a real Error, got {reported}"
        );
        assert_eq!(
            reported["message"],
            json!(message),
            "case {case:?}: expected the service's own sentence, got {reported}"
        );
        assert_eq!(
            reported["httpStatusCode"],
            json!(status),
            "case {case:?}: expected the answer's own HTTP status, got {reported}"
        );
        // The package exports a class under the shape's name only where that service's model
        // declares one, which is what makes the two vocabularies differ at all.
        assert_eq!(
            reported["exportsShape"],
            json!(if probe.modelled { "function" } else { "undefined" }),
            "case {case:?}: expected the package to export a class for {shape:?} only where \
             the model declares one, got {reported}"
        );
        match probe.names {
            Some(named) => assert_eq!(
                reported["isNamed"],
                json!(true),
                "case {case:?}: expected the modelled class {named:?} to catch it, got {reported}"
            ),
            None => {
                assert!(
                    !probe.modelled,
                    "case {case:?}: a row naming no modelled class is a refusal with no class \
                     of its own, got {reported}"
                );
                // A service carrying TWO vocabularies reports the LEGACY one here, which is a
                // different code from the modern shape its body named; a service carrying one
                // reports that single code.
                if !legacy.is_empty() {
                    assert_ne!(
                        reported["name"],
                        json!(shape),
                        "case {case:?}: expected the code reported for an unmodelled refusal \
                         to be the legacy one rather than the modern shape {shape:?}, got \
                         {reported}"
                    );
                }
            }
        }
    }

    // Nothing the refused probes asked for was done: the queue holds no message and the table
    // holds no item, read where the owning services hold them.
    let (status, _headers, drained) = aws_json(
        &router,
        "sqs",
        "AmazonSQS.ReceiveMessage",
        &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ReceiveMessage, got {status} ({drained})"
    );
    assert!(
        drained["Messages"].as_array().is_none_or(|held| held.is_empty()),
        "case {case:?}: every send was refused, so the queue must hold nothing, got {drained}"
    );
}

/// RULE, read the other way round: the SAME refusal reaches the raw wire with BOTH carriers —
/// the modern shape in the body and the legacy code in the header — whether or not the request
/// asked for the legacy vocabulary, so the client's choice of vocabulary is the client's and
/// not the service's.
#[tokio::test]
async fn a_query_compatible_refusal_carries_both_vocabularies_on_the_wire() {
    let router = router();
    let case = "both carriers of one query-compatible refusal";
    let queue_name = "error-carriers-probe";
    let (status, _headers, created) = aws_json(
        &router,
        "sqs",
        "AmazonSQS.CreateQueue",
        &json!({ "QueueName": queue_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateQueue, got {status} ({created})"
    );
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();

    // One row per refusal, each read for both carriers: the two refusals differ in whether the
    // service's model declares a shape for them.
    let rows: &[(&str, &str, Value)] = &[
        (
            "a refusal the model declares a shape for",
            "GetQueueUrl",
            json!({ "QueueName": format!("{queue_name}-absent") }),
        ),
        (
            "a refusal the model declares no shape for",
            "SendMessage",
            json!({
                "QueueUrl": queue_url,
                "MessageBody": "delayed past the range",
                "DelaySeconds": 901,
            }),
        ),
    ];
    for (row, operation, body) in rows {
        let (status, headers, answered) =
            aws_json(&router, "sqs", &format!("AmazonSQS.{operation}"), body).await;
        assert!(
            status >= 400,
            "case {row:?}: expected a refusal, got {status} with {answered}"
        );
        assert!(
            answered["__type"].as_str().is_some_and(|named| !named.is_empty()),
            "case {row:?}: expected the modern shape in __type, got {answered}"
        );
        let carried = headers
            .get("x-amzn-query-error")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut parts = carried.split(';');
        let code = parts.next().unwrap_or_default();
        let fault = parts.next().unwrap_or_default();
        assert!(
            !code.is_empty() && !fault.is_empty(),
            "case {row:?}: expected the legacy code and its fault as <code>;<fault>, got \
             {carried:?}"
        );
    }
}
