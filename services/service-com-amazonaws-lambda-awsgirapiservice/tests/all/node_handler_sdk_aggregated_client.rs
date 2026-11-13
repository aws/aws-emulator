// SPDX-License-Identifier: CC0-1.0

//! The AGGREGATED client of the AWS SDK for JavaScript v3 a `nodejs*` runtime bundles:
//! Lambda and Amazon DynamoDB on ONE engine, a handler that builds `new DynamoDB({})` at
//! initialization and calls `putItem`/`getItem` on it, and every answer asserted against the
//! service that gave it.
//!
//! The rule pinned here: a package of the AWS SDK for JavaScript v3 exports the aggregated
//! client `<Name>` beside the bare `<Name>Client`, and the aggregated class carries one method
//! per operation of the service — the operation's own name with its first character lower
//! cased — which takes that operation's input and answers a promise of its output, or calls a
//! node callback passed after the input. It stands on the bare client, so a refusal is the same
//! instance of the same modelled error class, and the work a method does is the work the
//! service records.
//!
//! The rows differ along what the rule generalises over: which module system binds the class
//! (`require` of a CommonJS module, `import { … } from` of an ES module), whether the client is
//! built at initialization or inside the handler, which operation is called (a write, a read,
//! a write the service refuses), which form the call takes (awaited promise, node callback,
//! node callback after an options object), and whether a service answers the call at all.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` these functions carry: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// How long this suite leaves between two readings of a status the clock owes. Every wait below
/// looks again for the CONDITION rather than sleeping for a span, so nothing here waits on the
/// emulator's behalf.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a status the clock owes is waited for before the wait itself is the failure.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// Lambda and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the handler's SDK call is dispatched back into this same router, so the table it writes to
/// is the table this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
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

/// One Amazon DynamoDB call on the wire (`awsJson1_0`), as its status and parsed body.
async fn dynamodb(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (parts.status.as_u16(), parsed)
}

/// One Amazon DynamoDB call that must answer 200.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = dynamodb(router, operation, body).await;
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

/// A .zip deployment package holding `source` under `entry`, and nothing else: no
/// `node_modules`, so every `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(entry: &str, source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file(entry, options).expect("the entry starts");
        writer.write_all(source.as_bytes()).expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a function whose `Runtime` is `runtime` and whose package holds `source` as
/// `entry`, and wait out the provisioning window the create documents.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    entry: &str,
    source: &str,
    runtime: &str,
) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": "index.handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(entry, source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    // "The function is being created" until it is Active, which is the state an invocation
    // needs: the wait is for that CONDITION, read through the function's own configuration,
    // and not for a span of this suite's choosing.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, _headers, configuration) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &[],
        )
        .await;
        if status == 200 && configuration["State"] == json!("Active") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the function never became Active: {status} ({configuration})"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
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

/// Create the table `name`, keyed on `id` (S), and wait for the status a read and a write
/// need: "You can perform read and write operations only on an ACTIVE table."
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str) {
    served(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "id", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = served(router, case, "DescribeTable", &json!({ "TableName": name })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What the handler of the first test writes in the `via` attribute of the item it puts, so the
/// row the table holds is the row the aggregated method wrote.
const WROTE: &str = "the aggregated client";

/// The CommonJS handler: the aggregated client is destructured out of the bundled package and
/// BUILT AT INITIALIZATION, which is how a Lambda handler that reuses one client across
/// invocations is written, and every call it makes goes through one of its own methods.
const COMMONJS_HANDLER: &str = r#"
'use strict';

const {
    DynamoDB,
    DynamoDBClient,
    ConditionalCheckFailedException,
} = require('@aws-sdk/client-dynamodb');

const client = new DynamoDB({});

exports.handler = async (event) => {
    const report = {
        exported: DynamoDB.name,
        builtAtInit: client instanceof DynamoDBClient,
        method: typeof client.putItem,
    };
    const written = await client.putItem({
        TableName: event.table,
        Item: { id: { S: event.id }, via: { S: event.via } },
    });
    report.wroteStatus = written.$metadata.httpStatusCode;
    const read = await client.getItem({
        TableName: event.table,
        Key: { id: { S: event.id } },
    });
    report.read = read.Item;
    report.callback = await new Promise((resolve) => {
        client.getItem(
            { TableName: event.table, Key: { id: { S: event.id } } },
            (error, answered) => {
                resolve(
                    error === null || error === undefined
                        ? { ok: true, via: answered.Item.via.S }
                        : { ok: false, name: error.name }
                );
            }
        );
    });
    report.callbackAfterOptions = await new Promise((resolve) => {
        client.getItem(
            { TableName: event.table, Key: { id: { S: event.id } } },
            {},
            (error, answered) => {
                resolve(
                    error === null || error === undefined
                        ? { ok: true, via: answered.Item.via.S }
                        : { ok: false, name: error.name }
                );
            }
        );
    });
    try {
        await client.putItem({
            TableName: event.table,
            Item: { id: { S: event.id }, via: { S: 'overwritten' } },
            ConditionExpression: 'attribute_not_exists(id)',
        });
        report.refused = { raised: false };
    } catch (error) {
        report.refused = {
            raised: true,
            name: error.name,
            ctor: error.constructor.name,
            modelled: error instanceof ConditionalCheckFailedException,
            message: error.message,
            httpStatusCode: error.$metadata.httpStatusCode,
        };
    }
    return report;
};
"#;

/// A `require`d aggregated client built at initialization calls the service through its own
/// per-operation methods, in both the promise and the callback form, and a refusal arrives as
/// the modelled class — with the write asserted on the table itself.
#[tokio::test]
async fn an_aggregated_client_calls_the_service_through_one_method_per_operation() {
    let router = router();
    let case = "a CommonJS handler driving the aggregated client";
    let table = "aggregated-sdk-items";
    create_table(&router, case, table).await;
    create_function(
        &router,
        case,
        "aggregated-sdk-caller",
        "index.js",
        COMMONJS_HANDLER,
        RUNTIME,
    )
    .await;

    let id = "alpha";
    let (signal, payload) = invoke(
        &router,
        case,
        "aggregated-sdk-caller",
        &json!({ "table": table, "id": id, "via": WROTE }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    assert_eq!(
        payload["exported"],
        json!("DynamoDB"),
        "case {case:?}: expected the aggregated class to carry the name its package exports \
         it under, got {payload}"
    );
    assert_eq!(
        payload["builtAtInit"],
        json!(true),
        "case {case:?}: expected the aggregated client to stand on the bare one, got {payload}"
    );
    assert_eq!(
        payload["method"],
        json!("function"),
        "case {case:?}: expected a method for the operation, got {payload}"
    );
    assert_eq!(
        payload["wroteStatus"],
        json!(200),
        "case {case:?}: expected the write to report the answer's own status, got {payload}"
    );
    assert_eq!(
        payload["read"],
        json!({ "id": { "S": id }, "via": { "S": WROTE } }),
        "case {case:?}: expected the read to answer the item the write made, got {payload}"
    );
    assert_eq!(
        payload["callback"],
        json!({ "ok": true, "via": WROTE }),
        "case {case:?}: expected the callback form to be called with the same answer, got \
         {payload}"
    );
    assert_eq!(
        payload["callbackAfterOptions"],
        json!({ "ok": true, "via": WROTE }),
        "case {case:?}: expected the callback after an options object to be called with the \
         same answer, got {payload}"
    );

    // The refusal the handler reported, against what the SAME request answers on the wire. The
    // shape is the one `PutItem` declares for a `ConditionExpression` that does not hold
    // (`smithy_query(query="operation", operation="PutItem")` lists
    // `ConditionalCheckFailedException`, httpError 400); the message and the status are read
    // off the answer rather than written here, so the names a JS consumer sees are the
    // service's own.
    let (status, refused) = dynamodb(
        &router,
        "PutItem",
        &json!({
            "TableName": table,
            "Item": { "id": { "S": id }, "via": { "S": "overwritten" } },
            "ConditionExpression": "attribute_not_exists(id)",
        }),
    )
    .await;
    let shape = refused["__type"]
        .as_str()
        .unwrap_or_default()
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string();
    let message = refused["message"]
        .as_str()
        .or_else(|| refused["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        shape, "ConditionalCheckFailedException",
        "case {case:?}: the condition does not hold, so the wire must answer the shape the \
         model declares for that, got {refused}"
    );
    assert_eq!(
        status, 400,
        "case {case:?}: expected the status the model binds to that error, got {status} with \
         {refused}"
    );
    assert!(
        !message.is_empty(),
        "case {case:?}: expected the refusal to carry a message, got {refused}"
    );
    assert_eq!(
        payload["refused"]["raised"],
        json!(true),
        "case {case:?}: the condition no longer holds, so the second write must be refused, \
         got {payload}"
    );
    assert_eq!(
        payload["refused"]["name"],
        json!(shape),
        "case {case:?}: expected the modelled shape the answer names ({shape}), got {payload}"
    );
    assert_eq!(
        payload["refused"]["ctor"],
        json!(shape),
        "case {case:?}: expected the refusal to be an instance of the class that shape names, \
         got {payload}"
    );
    assert_eq!(
        payload["refused"]["modelled"],
        json!(true),
        "case {case:?}: expected `instanceof` the class the handler imported to answer for the \
         shape {shape}, got {payload}"
    );
    assert_eq!(
        payload["refused"]["message"],
        json!(message),
        "case {case:?}: expected the service's own message, got {payload}"
    );
    assert_eq!(
        payload["refused"]["httpStatusCode"],
        json!(status),
        "case {case:?}: expected the answer's own HTTP status, got {payload}"
    );

    // What the TABLE holds: the write the method made, and not the write the condition
    // refused.
    let held = served(
        &router,
        case,
        "GetItem",
        &json!({ "TableName": table, "Key": { "id": { "S": id } } }),
    )
    .await;
    assert_eq!(
        held["Item"],
        json!({ "id": { "S": id }, "via": { "S": WROTE } }),
        "case {case:?}: expected the item the aggregated method wrote, got {held}"
    );
}

/// The ES module handler: the aggregated class of each package is bound BY NAME, which an
/// `import { … } from` resolves when the module is linked, and each probe reports what the
/// class and its method answered.
const ES_MODULE_HANDLER: &str = r#"
import { DynamoDB } from "@aws-sdk/client-dynamodb";
import { Rekognition } from "@aws-sdk/client-rekognition";
import { SNS } from "@aws-sdk/client-sns";

const PACKAGES = { dynamodb: DynamoDB, rekognition: Rekognition, sns: SNS };

export const handler = async (event) => {
    const report = {};
    for (const probe of event.probes) {
        const Aggregated = PACKAGES[probe.service];
        const held = { bound: typeof Aggregated === "function" };
        if (held.bound) {
            const client = new Aggregated({});
            held.exported = Aggregated.name;
            held.method = typeof client[probe.method];
            try {
                await client[probe.method](probe.input);
                held.outcome = "answered";
            } catch (error) {
                held.outcome = error instanceof TypeError ? "no such member" : "raised";
                held.name = error.name;
            }
        }
        report[probe.case] = held;
    }
    return report;
};
"#;

/// One probe of the ES module table: the case, the package's service token, the aggregated
/// class name that package exports, the method called, its input, what `typeof` that member
/// must answer, and what the call must come to.
struct Probe {
    case: &'static str,
    service: &'static str,
    /// The class the SDK's package for that service exports the aggregated client under.
    exported: &'static str,
    /// The method the call is made through, which is the operation's own name with its first
    /// character lower cased.
    method: &'static str,
    input: fn(&str) -> Value,
    /// What `typeof client[method]` must answer: `function` for a name that is an operation of
    /// that service, `undefined` for a name that is not one.
    member: &'static str,
    /// What the call must come to: `raised` for one the service refused and for one no service
    /// of this suite's router answers, `answered` for one it served, `no such member` for a name
    /// the package carries no method for.
    outcome: &'static str,
}

/// The probes, one per thing the rule generalises over: a read of a table that is there, a read
/// of one that is not, a name that is no operation of a service whose model is held, a service
/// no emulator answers for here, and one whose model it holds and whose protocol is neither
/// of the JSON ones.
fn probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "a read of the table, through the aggregated method",
            service: "dynamodb",
            exported: "DynamoDB",
            method: "getItem",
            input: |table| json!({ "TableName": table, "Key": { "id": { "S": "alpha" } } }),
            member: "function",
            outcome: "answered",
        },
        Probe {
            case: "a read of a table that is not there",
            service: "dynamodb",
            exported: "DynamoDB",
            method: "getItem",
            input: |table| {
                json!({
                    "TableName": format!("{table}-absent"),
                    "Key": { "id": { "S": "alpha" } },
                })
            },
            member: "function",
            outcome: "raised",
        },
        Probe {
            // The methods of a service whose model this emulator holds are that model's
            // operations and no others, so a misspelling is the `undefined` the SDK's own
            // package answers rather than a call.
            case: "a name that is no operation of a service whose model is held",
            service: "dynamodb",
            exported: "DynamoDB",
            method: "getItemm",
            input: |table| json!({ "TableName": table, "Key": { "id": { "S": "alpha" } } }),
            member: "undefined",
            outcome: "no such member",
        },
        Probe {
            // The image analysis service is mounted nowhere in this suite's router, so the
            // call is one no service answers — which is the refusal the host reports rather
            // than a member that is not there.
            case: "an image analysis of a service this router mounts nothing for",
            service: "rekognition",
            exported: "Rekognition",
            method: "detectLabels",
            input: |_| json!({ "Image": { "Bytes": "" }, "MaxLabels": 10 }),
            member: "function",
            outcome: "raised",
        },
        Probe {
            // Amazon SNS's model declares `awsQuery`, which the host binds through its own
            // client seam: the method is the model's operation, and the call reaches the
            // service, which refuses a publication to a topic it does not hold.
            case: "a publication to a topic no topic answers to",
            service: "sns",
            exported: "SNS",
            method: "publish",
            input: |_| json!({ "TopicArn": "arn:aws:sns:us-east-1:000000000000:t", "Message": "m" }),
            member: "function",
            outcome: "raised",
        },
    ]
}

/// Every package of the bundled SDK exports its aggregated client under the name the SDK
/// publishes it by, and that class carries the method of each operation the called service's
/// model states — so a handler's call is refused by the host or the service that decides it
/// rather than by a member that is not there, while a name that is no operation of a modelled
/// service stays absent.
#[tokio::test]
async fn an_es_module_binds_the_aggregated_client_of_every_bundled_package_by_name() {
    let router = router();
    let case = "the aggregated clients, imported by name";
    let table = "aggregated-esm-items";
    create_table(&router, case, table).await;
    served(
        &router,
        case,
        "PutItem",
        &json!({ "TableName": table, "Item": { "id": { "S": "alpha" } } }),
    )
    .await;
    create_function(
        &router,
        case,
        "aggregated-esm-caller",
        "index.mjs",
        ES_MODULE_HANDLER,
        RUNTIME,
    )
    .await;

    let asked: Vec<Value> = probes()
        .iter()
        .map(|probe| {
            json!({
                "case": probe.case,
                "service": probe.service,
                "method": probe.method,
                "input": (probe.input)(table),
            })
        })
        .collect();
    let (signal, payload) = invoke(
        &router,
        case,
        "aggregated-esm-caller",
        &json!({ "probes": asked }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    for probe in probes() {
        let case = probe.case;
        let reported = &payload[case];
        assert_eq!(
            reported["bound"],
            json!(true),
            "case {case:?}: expected the import to bind the aggregated class, got {payload}"
        );
        assert_eq!(
            reported["exported"],
            json!(probe.exported),
            "case {case:?}: expected the class the SDK publishes the aggregated client under, \
             got {reported}"
        );
        assert_eq!(
            reported["method"],
            json!(probe.member),
            "case {case:?}: expected `typeof` the member {:?} to be {:?}, got {reported}",
            probe.method,
            probe.member
        );
        assert_eq!(
            reported["outcome"],
            json!(probe.outcome),
            "case {case:?}: expected the call to come to {:?}, got {reported}",
            probe.outcome
        );
    }
}

/// The `Runtime` identifiers this table drives, one per managed Node release of the model's
/// `Runtime` enum that bundles the SDK for JavaScript v3 (source for the value set:
/// `smithy_query(query="shape", shape="Runtime")`) — including the one the report named and
/// ones it did not.
const MANAGED_NODE_RUNTIMES: &[&str] = &["nodejs18.x", "nodejs20.x", "nodejs22.x", "nodejs24.x"];

/// The handler the runtime table runs: it builds the aggregated client at initialization and
/// reads the item through it, so the run exercises the package the runtime resolved and not
/// only the dispatch that started it.
const RUNTIME_HANDLER: &str = r#"
'use strict';

const { DynamoDB } = require('@aws-sdk/client-dynamodb');

const client = new DynamoDB({});

exports.handler = async (event) => {
    const read = await client.getItem({
        TableName: event.table,
        Key: { id: { S: event.id } },
    });
    return { runtime: process.env.AWS_EXECUTION_ENV, id: read.Item.id.S };
};
"#;

/// A function whose `Runtime` is a managed Node release is INVOKED — its package is run and
/// its value answered — rather than refused for the runtime it declares. One row per
/// identifier of that set: every row asserts the modelled success, which is the refusal's
/// absence, and the `InvalidRuntimeException` the report saw is named so a regression to it
/// reads as what it is.
#[tokio::test]
async fn a_managed_node_runtime_is_invoked_rather_than_refused_for_its_runtime() {
    let router = router();
    let case = "a managed Node runtime running its own package";
    let table = "runtime-dispatch-items";
    let id = "alpha";
    create_table(&router, case, table).await;
    served(
        &router,
        case,
        "PutItem",
        &json!({ "TableName": table, "Item": { "id": { "S": id } } }),
    )
    .await;
    for runtime in MANAGED_NODE_RUNTIMES {
        let name = "runtime-dispatch-caller";
        create_function(&router, runtime, name, "index.js", RUNTIME_HANDLER, runtime).await;
        let (status, headers, payload) = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/invocations"),
            json!({ "table": table, "id": id }).to_string().as_bytes(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {runtime:?}: expected the modelled success of an invocation, got {status} \
             with {payload}"
        );
        assert_eq!(
            payload["__type"],
            Value::Null,
            "case {runtime:?}: expected no modelled error for a runtime the model names, got \
             {payload}"
        );
        assert_eq!(
            headers.get("x-amz-function-error"),
            None,
            "case {runtime:?}: the handler returned, so no function error is reported, got \
             {payload}"
        );
        assert_eq!(
            payload["id"],
            json!(id),
            "case {runtime:?}: expected the value the run read through the bundled SDK, got \
             {payload}"
        );
        assert_eq!(
            payload["runtime"],
            json!(format!("AWS_Lambda_{runtime}")),
            "case {runtime:?}: expected the run to report the runtime it ran as, got {payload}"
        );
        let (deleted, _headers, body) =
            lambda(&router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[]).await;
        assert_eq!(
            deleted, 204,
            "case {runtime:?}: the function must be removable between rows, got {deleted} with \
             {body}"
        );
    }
}
