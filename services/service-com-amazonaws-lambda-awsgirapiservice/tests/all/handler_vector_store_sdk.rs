// SPDX-License-Identifier: CC0-1.0

//! The vector-store calls a RUNNING handler makes with the AWS SDK its runtime supplies, end
//! to end over the wire: Lambda, the vector store, IAM and the token service on ONE engine, a
//! `python3.x` function whose code builds a client at module scope, and every bucket, index
//! and vector it wrote read back where a client reads them — through that service's own reads.
//!
//! The RULE pinned here: a managed runtime ships the AWS SDK for the handler's language with
//! the service model of every service the runtime's call seam binds, so a handler builds a
//! client for one with no dependency of its own and what the call answers is THAT service's
//! own answer — the operation's output members and the state the service then reports, or that
//! service's modelled refusal (`AccessDeniedException` when the execution role lacks the
//! action, `NotFoundException` for a name it does not hold) raised as
//! `botocore.exceptions.ClientError` carrying its error code. A token the SDK carries no model
//! for is the complement: the client is told the service is unknown and no request is made.
//!
//! The rows differ along what the rule generalises over: the SHAPE of the input each call
//! carries (a required scalar, an enum, an integer, a map of text, a list of structures whose
//! member is a UNION of floats, a document) and of the output it answers (a minted ARN, the
//! empty output of an operation with no output member, a list of structures carrying a union
//! and a document, a list carrying a timestamp, an enum the service decides), and whether the
//! call is served, refused by the service, refused for want of a grant, or one the SDK has no
//! model to make at all.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]), and it makes THREE
//! invocations in all — the served calls, the refused calls, and the call made once the grant
//! is withdrawn — because an invocation of its own costs this target a share of the run budget
//! every other invocation suite draws on.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The IAM call every suite in this target writes a role with, and the one .zip writer and
// function helpers a running handler is packaged and awaited with — owned by the suites that
// first needed them rather than copied.
use crate::event_source_mapping_role_access::iam as iam_call;
use crate::event_source_mappings::{await_function_active, call, router_on};
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The account every ARN below names, which is the account this emulator serves.
const ACCOUNT: &str = "000000000000";

/// The execution role the function runs as, the function's own name, and the inline identity
/// policy the role carries the grant in.
const ROLE_NAME: &str = "lambda-vector-writer";
const FUNCTION: &str = "vector-caller";
const INLINE_POLICY: &str = "inline";

/// The AWS-managed policy a Lambda execution role is created with: it grants the log writes a
/// function makes and nothing else, so a role carrying only it has no grant on the vector
/// store.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The vector bucket and index the handler creates, and the index the refused read names.
const BUCKET: &str = "handler-embeddings";
const INDEX: &str = "orders";
const ABSENT_INDEX: &str = "no-such-index";

/// The index the handler creates: `dataType` and `distanceMetric` are the enum values the
/// model declares for them (`CreateIndex$dataType` is one of `float32`;
/// `CreateIndex$distanceMetric` one of `cosine | euclidean`), and `dimension` the element count
/// every vector in the index then has to carry.
const DATA_TYPE: &str = "float32";
const DISTANCE_METRIC: &str = "cosine";
const DIMENSION: u64 = 3;

/// The tag the create carries, which is what makes the create need the tagging permission
/// beside its own.
const TAG_KEY: &str = "team";
const TAG_VALUE: &str = "search";

/// The three vectors the handler writes, each as its key and its `float32` data: unit vectors
/// along the index's three dimensions, so which one is nearest to a query is decided by the
/// query and not by the order they were written in.
const VECTORS: [(&str, [f64; 3]); 3] = [
    ("v-a", [1.0, 0.0, 0.0]),
    ("v-b", [0.0, 1.0, 0.0]),
    ("v-c", [0.0, 0.0, 1.0]),
];

/// The metadata member the handler puts on the first vector, and the value it carries.
const METADATA_KEY: &str = "source";
const METADATA_VALUE: &str = "orders";

/// The query the nearest-neighbour search carries, and which of the written vectors it is
/// nearest to: the query leans along the LAST vector's dimension, so a search that answered the
/// order the vectors were written in rather than the order their distances put them in would
/// answer the wrong key first.
const QUERY_VECTOR: [f64; 3] = [0.0, 0.1, 0.9];
const NEAREST: usize = 2;

/// How many of the index's vectors the search asks for.
const TOP_K: u64 = 2;

/// The `MemorySize` the function is created with, in MB: a heap a vector workload is given
/// rather than the smallest the model allows, because the handler's first call to a service is
/// what reads that service's own model (`smithy_query(query="shape", shape="MemorySize")`:
/// `@range(min: 128, max: 32768)`).
const MEMORY_SIZE_MB: u32 = 512;

/// A token no AWS service is published under, which is what a client built for a service the
/// runtime's SDK carries no model for is told about.
const UNKNOWN_SERVICE: &str = "no-such-service";

/// The error codes the vector store refuses with: a name it does not hold, and a call the
/// caller's policies do not permit (`NotFoundException`, `AccessDeniedException`, both
/// errors this service's model declares).
const NOT_FOUND: &str = "NotFoundException";
const ACCESS_DENIED: &str = "AccessDeniedException";

/// Lambda, the vector store, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the index it writes is the index this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(Arc::clone(&engine)),
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

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. Nothing here is signed: the suite's own calls present the account's own published
/// credential.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `restJson1` call to the vector store, as the status it answered and the document it
/// carried. Every operation of this service binds `POST /<OperationName>` with its input as
/// the JSON body.
async fn vectors_call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/{operation}"))
        .header("host", format!("s3vectors.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3vectors"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    // A body that is not the document this protocol answers is a regression in its own right,
    // so it fails here rather than reaching an assertion about a missing member.
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!(
                "setup: expected a JSON document from the vector store's {operation}, got \
                 {error}: {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The same, for a call that must be served.
async fn vectors(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = vectors_call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from the vector store's {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One IAM call that must be served.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) {
    let (status, text) = iam_call(router, action, params).await;
    assert_eq!(
        status, 200,
        "setup: expected HTTP 200 from IAM {action}, got {status} / {text}"
    );
}

/// The handler every invocation runs: it makes the calls the event names, in order, and
/// reports what each one answered — so the invocation's payload IS what the SDK gave the
/// guest.
///
/// The vector-store client is built at MODULE scope, which is where a real function builds
/// one: a runtime whose SDK carries no model for the service fails there, before any handler
/// code runs. A call naming another service builds its own client, which is how the token with
/// no model at all is reached.
const HANDLER_SOURCE: &str = r#"
import datetime

import boto3
import botocore.exceptions

VECTORS = boto3.client("s3vectors")


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
    results = []
    for asked in event["calls"]:
        named = asked.get("service")
        try:
            client = VECTORS if named is None else boto3.client(named)
            answered = getattr(client, asked["method"])(**asked["parameters"])
        except botocore.exceptions.UnknownServiceError as error:
            results.append({"ok": False, "unknown_service": str(error)})
            continue
        except botocore.exceptions.ClientError as error:
            results.append(
                {
                    "ok": False,
                    "code": error.response["Error"]["Code"],
                    "message": error.response["Error"].get("Message"),
                }
            )
            continue
        except botocore.exceptions.SdkCallNotMade as error:
            results.append({"ok": False, "not_made": str(error)})
            continue
        results.append(
            {
                "ok": True,
                "status": answered["ResponseMetadata"]["HTTPStatusCode"],
                "answered": _reported({
                    name: value
                    for name, value in answered.items()
                    if name != "ResponseMetadata"
                }),
            }
        )
    return {"results": results}
"#;

/// One served row: the client method the handler calls, the input it carries, the output
/// members whose value is known, and the ones the service itself decides.
struct Row {
    case: &'static str,
    method: &'static str,
    parameters: Value,
    /// Each known output member: where it sits in the answered document, and its value.
    answered: Vec<(&'static [&'static str], Value)>,
    /// Each output member the service itself fills in, which must be there and carry a value.
    decided: Vec<&'static [&'static str]>,
}

/// One refused row: the call, and what the handler must have been told.
struct Refused {
    case: &'static str,
    /// The service the client is built for, or `None` for the vector-store client the handler
    /// holds at module scope.
    service: Option<&'static str>,
    method: &'static str,
    parameters: Value,
    /// The error code the service refuses the call with, or `None` for a call the SDK has no
    /// model to make.
    code: Option<&'static str>,
    /// Text the detail of a call that was never made has to name.
    names: Vec<&'static str>,
}

/// The ARN of the vector bucket of this account in this Region, as the model's
/// `VectorBucketArn` pattern spells one.
fn bucket_arn() -> String {
    format!("arn:aws:s3vectors:{REGION}:{ACCOUNT}:bucket/{BUCKET}")
}

/// The ARN of an index inside it, as the model's `IndexArn` pattern spells one.
fn index_arn() -> String {
    format!("{}/index/{INDEX}", bucket_arn())
}

/// The ARN of the execution role, as IAM records it.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE_NAME}")
}

/// The identity policy that grants the role every vector-store action this suite's handler
/// calls, on every resource of the account.
fn grant_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3vectors:*",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The trust policy a Lambda execution role carries.
fn trust_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "lambda.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The `float32` data of one vector, as the model's `VectorData` union carries it.
fn vector_data(data: &[f64]) -> Value {
    json!({ "float32": data })
}

/// The vectors the write carries, each with its key and data, and metadata on the first.
fn written_vectors() -> Value {
    Value::Array(
        VECTORS
            .iter()
            .enumerate()
            .map(|(position, (key, data))| {
                let mut vector = json!({ "key": key, "data": vector_data(data) });
                if position == 0 {
                    vector["metadata"] = json!({ METADATA_KEY: METADATA_VALUE });
                }
                vector
            })
            .collect(),
    )
}

/// The served rows, in the order one invocation makes them: each later row stands on what the
/// earlier ones did, which is what puts the vectors the search reads into the index the create
/// made.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create carrying a required name and a map of tags",
            method: "create_vector_bucket",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "tags": { TAG_KEY: TAG_VALUE },
            }),
            answered: vec![(&["vectorBucketArn"], json!(bucket_arn()))],
            decided: Vec::new(),
        },
        Row {
            case: "a create carrying two enums and an integer",
            method: "create_index",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "dataType": DATA_TYPE,
                "dimension": DIMENSION,
                "distanceMetric": DISTANCE_METRIC,
            }),
            answered: vec![(&["indexArn"], json!(index_arn()))],
            decided: Vec::new(),
        },
        Row {
            case: "a write carrying a list of structures whose data is a union of floats",
            method: "put_vectors",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "vectors": written_vectors(),
            }),
            // `PutVectorsOutput` declares no member at all, so a served call answers the
            // empty document beside the SDK's own response metadata.
            answered: vec![(&[], json!({}))],
            decided: Vec::new(),
        },
        Row {
            case: "a read carrying a list of keys and two booleans",
            method: "get_vectors",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "keys": [VECTORS[0].0],
                "returnData": true,
                "returnMetadata": true,
            }),
            answered: vec![
                (&["vectors", "0", "key"], json!(VECTORS[0].0)),
                (
                    &["vectors", "0", "metadata", METADATA_KEY],
                    json!(METADATA_VALUE),
                ),
            ],
            decided: vec![&["vectors", "0", "data", "float32"]],
        },
        Row {
            case: "a nearest-neighbour search carrying a union and an integer",
            method: "query_vectors",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "topK": TOP_K,
                "queryVector": vector_data(&QUERY_VECTOR),
                "returnDistance": true,
            }),
            answered: vec![
                (&["vectors", "0", "key"], json!(VECTORS[NEAREST].0)),
                (&["distanceMetric"], json!(DISTANCE_METRIC)),
            ],
            decided: vec![&["vectors", "0", "distance"], &["vectors", "1", "key"]],
        },
        Row {
            case: "a listing of the index's own vector keys",
            method: "list_vectors",
            parameters: json!({ "vectorBucketName": BUCKET, "indexName": INDEX }),
            // The keys the listing answers are asserted as a SET below, because a listing
            // states no order of its own.
            answered: Vec::new(),
            decided: vec![&["vectors"]],
        },
        Row {
            case: "a listing answering a list of structures carrying a timestamp",
            method: "list_vector_buckets",
            parameters: json!({}),
            answered: vec![
                (&["vectorBuckets", "0", "vectorBucketName"], json!(BUCKET)),
                (&["vectorBuckets", "0", "vectorBucketArn"], json!(bucket_arn())),
            ],
            decided: vec![&["vectorBuckets", "0", "creationTime"]],
        },
    ]
}

/// The refused rows, which one further invocation makes.
fn refused_rows() -> Vec<Refused> {
    vec![
        Refused {
            case: "a read naming an index the bucket does not hold",
            service: None,
            method: "get_vectors",
            parameters: json!({
                "vectorBucketName": BUCKET,
                "indexName": ABSENT_INDEX,
                "keys": [VECTORS[0].0],
            }),
            code: Some(NOT_FOUND),
            names: Vec::new(),
        },
        Refused {
            case: "a search naming a bucket no bucket answers to",
            service: None,
            method: "query_vectors",
            parameters: json!({
                "vectorBucketName": "no-such-bucket",
                "indexName": INDEX,
                "topK": TOP_K,
                "queryVector": vector_data(&QUERY_VECTOR),
            }),
            code: Some(NOT_FOUND),
            names: Vec::new(),
        },
        Refused {
            case: "a client for a token the runtime's SDK carries no model for",
            service: Some(UNKNOWN_SERVICE),
            method: "list_vector_buckets",
            parameters: json!({}),
            code: None,
            // botocore's own wording: the token asked for, and the models the runtime has —
            // which has to name the service this suite's own calls reach.
            names: vec!["Unknown service", UNKNOWN_SERVICE, "s3vectors"],
        },
    ]
}

/// The `key` of each vector of an answered list, in the order the answer carried them.
fn keys_of(vectors: &Value) -> Vec<String> {
    vectors
        .as_array()
        .map(|carried| {
            carried
                .iter()
                .map(|vector| vector["key"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The value at `place` of `document`, where a numeric step indexes an array.
fn at<'a>(document: &'a Value, place: &[&str]) -> &'a Value {
    let mut read = document;
    for step in place {
        read = match step.parse::<usize>() {
            Ok(index) => &read[index],
            Err(_not_an_index) => &read[*step],
        };
    }
    read
}

/// Invoke the function with `event` and answer the payload, asserting the invocation itself
/// answered 200 with no function error — the handler reports a refusal in its own payload, so a
/// function error here is the crash the runtime must not produce.
async fn invoked(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
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
        "case {case:?}: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal, None,
        "case {case:?}: expected no FunctionError, got {signal:?} with {}",
        answered.text()
    );
    answered.json()
}

/// The event one invocation carries for `calls`.
fn event(calls: &[Value]) -> Value {
    json!({ "calls": calls })
}

/// RULE: a handler reaches the vector store through the SDK its runtime supplies, so what its
/// call answers is that service's own answer and what it wrote is what that service then
/// reports.
#[tokio::test]
async fn a_running_handler_writes_and_searches_vectors_through_its_runtimes_sdk() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    // The role the function runs as, written with IAM's own operations, so the session the
    // runtime mints speaks for the record `iam:CreateRole` wrote.
    iam(
        &router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust_document()),
        ],
    )
    .await;
    iam(
        &router,
        "AttachRolePolicy",
        &[("RoleName", ROLE_NAME), ("PolicyArn", BASIC_EXECUTION_POLICY)],
    )
    .await;
    iam(
        &router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", INLINE_POLICY),
            ("PolicyDocument", &grant_document()),
        ],
    )
    .await;

    // The function itself, running as that role.
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": role_arn(),
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 30,
            "MemorySize": MEMORY_SIZE_MB,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        created.text()
    );
    await_function_active(&router, "setup", FUNCTION).await;

    // ONE invocation makes every served call, in order.
    let rows = rows();
    let asked: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "method": row.method, "parameters": row.parameters }))
        .collect();
    let payload = invoked(&router, "the served calls", &event(&asked)).await;
    let results = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        rows.len(),
        "case \"the served calls\": expected one result per call, got {payload}"
    );

    for (row, result) in rows.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(true),
            "case {case:?}: expected the handler's call to be served, got {result}"
        );
        assert_eq!(
            result["status"],
            json!(200),
            "case {case:?}: expected the SDK to report HTTP 200, got {result}"
        );
        for (place, expected) in &row.answered {
            let read = at(&result["answered"], place);
            assert_eq!(
                read, expected,
                "case {case:?}: expected the output member {place:?} to be {expected}, got \
                 {result}"
            );
        }
        for place in &row.decided {
            let read = at(&result["answered"], place);
            assert!(
                !read.is_null(),
                "case {case:?}: expected the service to fill in the output member {place:?}, \
                 got {result}"
            );
        }
    }

    // The nearest-neighbour search answered as many vectors as it asked for, and the nearest
    // one first: the query leans along the LAST written vector's dimension, so a search
    // answering the order the vectors were written in would answer another key first, and the
    // second answer is a different vector rather than that same key again.
    let searched = &results[4]["answered"]["vectors"];
    let keys = keys_of(searched);
    assert_eq!(
        keys.len(),
        TOP_K as usize,
        "case \"a nearest-neighbour search carrying a union and an integer\": expected {TOP_K} \
         vectors, got {keys:?}"
    );
    assert_eq!(
        keys.first().map(String::as_str),
        Some(VECTORS[NEAREST].0),
        "case \"a nearest-neighbour search carrying a union and an integer\": expected the \
         vector nearest the query first, got {keys:?}"
    );
    assert_ne!(
        keys.first(),
        keys.last(),
        "case \"a nearest-neighbour search carrying a union and an integer\": expected two \
         distinct vectors, got {keys:?}"
    );

    // The listing answered every key the write put in the index, and no other: a listing
    // states no order, so the keys are compared as a set.
    let mut listed = keys_of(&results[5]["answered"]["vectors"]);
    listed.sort();
    let mut written: Vec<String> = VECTORS.iter().map(|(key, _)| key.to_string()).collect();
    written.sort();
    assert_eq!(
        listed, written,
        "case \"a listing of the index's own vector keys\": expected the keys the write put in \
         the index, got {listed:?}"
    );

    // The data the read answered is the data the write carried, read as the floats of the
    // union's `float32` member.
    let read_back = at(&results[3]["answered"], &["vectors", "0", "data", "float32"]);
    let floats: Vec<f64> = read_back
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    assert_eq!(
        floats.len(),
        VECTORS[0].1.len(),
        "case \"a read carrying a list of keys and two booleans\": expected one element per \
         dimension, got {read_back}"
    );
    for (position, expected) in VECTORS[0].1.iter().enumerate() {
        let actual = floats.get(position).copied().unwrap_or(f64::NAN);
        assert!(
            (actual - expected).abs() < 1e-6,
            "case \"a read carrying a list of keys and two booleans\": expected element \
             {position} to be {expected}, got {actual}"
        );
    }

    // The other half of each served row: what the vector store itself holds, read where a
    // client reads it.
    let index = vectors(
        &router,
        "GetIndex",
        &json!({ "vectorBucketName": BUCKET, "indexName": INDEX }),
    )
    .await;
    assert_eq!(
        index["index"]["indexArn"],
        json!(index_arn()),
        "case \"a create carrying two enums and an integer\": expected the service to report \
         the index the handler created, got {index}"
    );
    assert_eq!(
        index["index"]["dimension"],
        json!(DIMENSION),
        "case \"a create carrying two enums and an integer\": expected the dimension the \
         create carried, got {index}"
    );
    assert_eq!(
        index["index"]["distanceMetric"],
        json!(DISTANCE_METRIC),
        "case \"a create carrying two enums and an integer\": expected the distance metric \
         the create carried, got {index}"
    );
    let listed = vectors(&router, "ListVectorBuckets", &json!({})).await;
    assert_eq!(
        listed["vectorBuckets"][0]["vectorBucketName"],
        json!(BUCKET),
        "case \"a create carrying a required name and a map of tags\": expected the service to \
         report the bucket the handler created, got {listed}"
    );
    let stored = vectors(
        &router,
        "GetVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "keys": VECTORS.iter().map(|(key, _)| *key).collect::<Vec<&str>>(),
            "returnData": true,
            "returnMetadata": true,
        }),
    )
    .await;
    let held = keys_of(&stored["vectors"]);
    for (key, _data) in VECTORS {
        assert!(
            held.iter().any(|stored| stored == key),
            "case \"a write carrying a list of structures whose data is a union of floats\": \
             expected the service to hold the vector {key:?}, got {held:?}"
        );
    }

    // A second invocation makes the calls that are refused.
    let refused = refused_rows();
    let asked: Vec<Value> = refused
        .iter()
        .map(|row| {
            json!({
                "service": row.service,
                "method": row.method,
                "parameters": row.parameters,
            })
        })
        .collect();
    let payload = invoked(&router, "the refused calls", &event(&asked)).await;
    let results = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        refused.len(),
        "case \"the refused calls\": expected one result per call, got {payload}"
    );
    for (row, result) in refused.iter().zip(&results) {
        let case = row.case;
        assert_eq!(
            result["ok"],
            json!(false),
            "case {case:?}: expected the call to be refused, got {result}"
        );
        if let Some(code) = row.code {
            assert_eq!(
                result["code"],
                json!(code),
                "case {case:?}: expected the error code {code:?}, got {result}"
            );
            assert!(
                result["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected the service's own sentence beside the code, got \
                 {result}"
            );
        }
        for named in &row.names {
            let detail = result["unknown_service"].as_str().unwrap_or_default();
            assert!(
                detail.contains(named),
                "case {case:?}: expected the unknown-service detail to name {named:?}, got \
                 {result}"
            );
        }
    }

    // A third invocation writes with the grant WITHDRAWN from the role: the call a handler
    // makes is the role's, so the service refuses it in its own words.
    iam(
        &router,
        "DeleteRolePolicy",
        &[("RoleName", ROLE_NAME), ("PolicyName", INLINE_POLICY)],
    )
    .await;
    let case = "a write the execution role is no longer granted";
    let payload = invoked(
        &router,
        case,
        &event(&[json!({
            "method": "put_vectors",
            "parameters": {
                "vectorBucketName": BUCKET,
                "indexName": INDEX,
                "vectors": [{ "key": "v-d", "data": vector_data(&QUERY_VECTOR) }],
            },
        })]),
    )
    .await;
    let denied = &payload["results"][0];
    assert_eq!(
        denied["ok"],
        json!(false),
        "case {case:?}: expected the write to be refused, got {payload}"
    );
    assert_eq!(
        denied["code"],
        json!(ACCESS_DENIED),
        "case {case:?}: expected the service's own authorization refusal, got {payload}"
    );
    let message = denied["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("s3vectors:PutVectors") && message.contains(ROLE_NAME),
        "case {case:?}: expected the refusal to name the action and the role, got {payload}"
    );

    // And nothing was written by it: the index holds exactly the vectors the granted write
    // put there.
    let after = vectors(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": BUCKET, "indexName": INDEX }),
    )
    .await;
    let count = after["vectors"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        count,
        VECTORS.len(),
        "case {case:?}: expected the refused write to leave the index as it was, got {after}"
    );
}
