// SPDX-License-Identifier: CC0-1.0

//! How this service CLASSIFIES a refused input constraint, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! # The rule
//!
//! A refused input constraint is reported through this service's own modelled
//! `ValidationException` — HTTP 400, the counting message envelope that names the member
//! and the requirement it failed, AND the `reason` member that classifies the refusal as
//! `CONSTRAINT_VIOLATION` (`smithy_query(query="shape",shape="ValidationException")` ->
//! `message` + `reason`; `smithy_query(query="shape",
//! shape="ValidationExceptionReason")` for the value set) — whichever side of the request
//! boundary caught it. A refusal of client input that is NOT a constraint on a member's
//! value keeps its own classification, so the member is a classification rather than a
//! stamp.
//!
//! # What the tables span
//!
//! The rows differ along what the rule generalises over: the constraint CLASS (an absent
//! `@required` member, `@length` at both ends, `@range` at both ends, `@pattern`, an enum
//! value set, a bound on a map's keys, a blank member, and a minimum a member's own
//! documentation publishes where the model declares none); WHERE the value the request
//! carried it in sits (the body, a URI label, a query string, a header); the number of
//! constraints one request breaks (1 and 2); WHICH SIDE of the boundary enforces the
//! constraint (the model-driven walk in front of a handler, and the handler itself); the
//! operation; and whether the operation is served behind the pass-role authorization
//! boundary, which answers ahead of the constraint walk.
//!
//! `tests/all/error_message_rendering.rs` and `tests/all/accumulated_constraint_refusals.rs`
//! pin what such a refusal SAYS and how it counts; this suite pins the shape it is
//! reported in and the `reason` that classifies it.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// A graph this store HOLDS and that is SERVING, so a request addressed to its own endpoint
// reaches the operation behind that endpoint instead of being refused at the door for an
// address that resolves to nothing: the sibling suite's helper, which creates the graph and
// polls `GetGraph` until it reports itself in service, reached rather than copied so the
// status and the poll are stated once for this crate's suites.
use super::query_cancellation::graph_in_service;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The modelled error a refused constraint is reported as (`@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `ValidationExceptionReason` value that classifies a member whose value does not
/// satisfy a rule the service states about it
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const CONSTRAINT_VIOLATION: &str = "CONSTRAINT_VIOLATION";

/// The `ValidationExceptionReason` value that classifies an argument the operation does
/// not accept — the classification the rows that are NOT constraint violations must keep.
const ILLEGAL_ARGUMENT: &str = "ILLEGAL_ARGUMENT";

/// Every member a `ValidationException` body may carry: the modelled `message` and
/// `reason` (`smithy_query(query="shape",shape="ValidationException")`), plus the
/// `__type` discriminator restJson1 writes into an error envelope.
const BODY_MEMBERS: [&str; 3] = ["__type", "message", "reason"];

/// The smallest `provisionedMemory` the service documents ("Min = 16").
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The largest `provisionedMemory` the model allows
/// (`smithy_query(query="shape",shape="ProvisionedMemory")` -> `@range max`).
const MAX_PROVISIONED_MEMORY: i64 = 24_576;

/// The longest `graphName` the model allows (`GraphName` is `@length(1..63)`).
const MAX_GRAPH_NAME_CHARS: usize = 63;

/// The longest tag key this service stores (`TagKey` is `@length(1..128)`), the bound a
/// map's KEYS are refused by in a handler: the walk in front of a handler treats a key as
/// the entry's name rather than as a constrained value.
const MAX_TAG_KEY_CHARS: usize = 128;

/// A well-formed `GraphIdentifier` (`^g-[a-z0-9]{10}$`) that names no stored graph: a
/// request is validated against the model before anything is looked up, so a call on it
/// is still refused for the modelled constraints it broke. A rule the HANDLER enforces is
/// decided later, so a row about one of those names a graph the store holds instead — an
/// identifier naming no graph of the Region addresses no graph endpoint at all, and the
/// operations reached at one answer that address before their own arguments.
const ABSENT_GRAPH_ID: &str = "g-0123456789";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// An IAM role ARN of the one account this emulator serves, which is the only kind of
/// role the pass-role boundary lets through — so a request carrying it is refused for the
/// constraints it broke rather than for the role it named.
fn passable_role_arn() -> String {
    engine::emulator_arn("iam", "", "role/NeptuneConstraintRole")
}

/// What one request was answered with: the status, the `x-amzn-errortype` header
/// restJson1 resolves an error shape from, and the parsed JSON body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

/// Dispatch one `restJson1` request and return what it was answered with.
///
/// `graph_identifier` is sent as the `graphIdentifier` HEADER, which is where the model
/// binds it for the query operations (`smithy_query(query="operation",
/// operation="ListQueries")`: `httpHeader: graphIdentifier`).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    graph_identifier: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        );
    if let Some(graph_identifier) = graph_identifier {
        request = request.header("graphIdentifier", graph_identifier);
    }
    let request = request
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The count a refusal's envelope opens with, read back out of it.
fn reported_count(text: &str) -> Option<usize> {
    text.split_whitespace().next()?.parse().ok()
}

/// A tag map whose single key is `key_chars` characters long.
fn oversized_tags(key_chars: usize) -> Value {
    json!({ "k".repeat(key_chars): "v" })
}

/// One refusal to check: the case, the request, how many constraints it breaks, and the
/// member path each broken constraint must be reported at.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    graph_identifier: Option<String>,
    body: Option<Value>,
    members: Vec<&'static str>,
}

/// Every modelled constraint class, enforced by the walk in front of a handler, plus
/// every constraint this service's handlers enforce themselves — each answered with the
/// modelled shape and classified the same way.
///
/// `stored_graph` is a graph this store HOLDS, which the rows about a handler-enforced
/// rule on an operation reached at a graph's own endpoint address: that endpoint resolves
/// the address before the operation behind it reads its arguments.
fn refusal_rows(stored_graph: &str) -> Vec<Row> {
    let role_arn = passable_role_arn();
    vec![
        // ---- Caught by the model-driven walk in front of the handler. ----
        Row {
            case: "an absent @required member, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({"provisionedMemory": MIN_PROVISIONED_MEMORY})),
            members: vec!["graphName"],
        },
        Row {
            case: "a @length minimum, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({"graphName": "", "provisionedMemory": MIN_PROVISIONED_MEMORY})),
            members: vec!["graphName"],
        },
        Row {
            case: "a @length maximum, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "a".repeat(MAX_GRAPH_NAME_CHARS + 1),
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
            })),
            members: vec!["graphName"],
        },
        Row {
            case: "a documented value floor, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({"graphName": "under-memory", "provisionedMemory": 15})),
            members: vec!["provisionedMemory"],
        },
        Row {
            case: "a @range maximum, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "over-replicas",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "replicaCount": 3,
            })),
            members: vec!["replicaCount"],
        },
        Row {
            case: "a @pattern, in the body",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "kms-graph",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "kmsKeyIdentifier": "not-a-kms-arn",
            })),
            members: vec!["kmsKeyIdentifier"],
        },
        Row {
            case: "a @pattern, in a URI label",
            method: "GET",
            path: "/graphs/not-a-valid-id".to_string(),
            graph_identifier: None,
            body: None,
            members: vec!["graphIdentifier"],
        },
        Row {
            case: "a @pattern, in a header",
            method: "GET",
            path: "/queries?maxResults=1".to_string(),
            graph_identifier: Some("not-a-valid-id".to_string()),
            body: None,
            members: vec!["graphIdentifier"],
        },
        Row {
            case: "a @range minimum, in a query string",
            method: "GET",
            path: "/graphs?maxResults=0".to_string(),
            graph_identifier: None,
            body: None,
            members: vec!["maxResults"],
        },
        Row {
            case: "a @range maximum, in a query string",
            method: "GET",
            path: "/graphs?maxResults=101".to_string(),
            graph_identifier: None,
            body: None,
            members: vec!["maxResults"],
        },
        Row {
            // The modelled walk runs ahead of the address this request is sent to, so an
            // identifier naming no graph is still refused for the enum value it carried.
            case: "an enum value set, in a query string",
            method: "GET",
            path: "/queries?maxResults=1&state=NOT_A_STATE".to_string(),
            graph_identifier: Some(ABSENT_GRAPH_ID.to_string()),
            body: None,
            members: vec!["state"],
        },
        Row {
            case: "two modelled constraints broken by one request",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "boundary-pair",
                "provisionedMemory": MAX_PROVISIONED_MEMORY + 1,
                "replicaCount": 3,
            })),
            members: vec!["provisionedMemory", "replicaCount"],
        },
        Row {
            // Behind the pass-role authorization boundary, which answers ahead of the
            // constraint walk: the role is passable, so the request is refused for the
            // constraint it broke.
            case: "a @pattern, in the body of a pass-role operation",
            method: "POST",
            path: "/exporttasks".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphIdentifier": ABSENT_GRAPH_ID,
                "roleArn": role_arn,
                "kmsKeyIdentifier": "not-a-kms-arn",
                "format": "PARQUET",
                "destination": "s3://bucket/prefix",
            })),
            members: vec!["kmsKeyIdentifier"],
        },
        // ---- Enforced by the handler, for a rule the model cannot express. ----
        Row {
            // A rule the HANDLER enforces, so this row addresses a graph the store holds:
            // an address resolving to no graph is refused ahead of the handler, and this
            // row is about the argument rather than about the address.
            case: "a published minimum the model declares no range for",
            method: "GET",
            path: "/queries?maxResults=0".to_string(),
            graph_identifier: Some(stored_graph.to_string()),
            body: None,
            members: vec!["maxResults"],
        },
        Row {
            case: "a @pattern whose lookahead the walk cannot compile",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "g-reserved",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
            })),
            members: vec!["graphName"],
        },
        Row {
            case: "a bound on a map's keys",
            method: "POST",
            path: "/graphs".to_string(),
            graph_identifier: None,
            body: Some(json!({
                "graphName": "oversized-tag-key",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": oversized_tags(MAX_TAG_KEY_CHARS + 1),
            })),
            members: vec!["tags"],
        },
        Row {
            case: "a blank member, in the body of a pass-role operation",
            method: "POST",
            path: format!("/graphs/{ABSENT_GRAPH_ID}/importtasks"),
            graph_identifier: None,
            body: Some(json!({
                "source": "   ",
                "format": "CSV",
                "roleArn": role_arn,
            })),
            members: vec!["source"],
        },
        Row {
            case: "a documented dependency between two members",
            method: "POST",
            path: format!("/graphs/{ABSENT_GRAPH_ID}/importtasks"),
            graph_identifier: None,
            body: Some(json!({
                "source": "s3://bucket/data",
                "format": "NTRIPLES",
                "roleArn": role_arn,
            })),
            members: vec!["blankNodeHandling"],
        },
    ]
}

#[tokio::test]
async fn every_refused_input_constraint_is_answered_with_the_modelled_reason() {
    let router = emulator();
    let held = graph_in_service(&router, "constraint-rows-graph").await;

    for row in refusal_rows(&held) {
        let case = row.case;
        let reply = call(
            &router,
            row.method,
            &row.path,
            row.graph_identifier.as_deref(),
            row.body,
        )
        .await;
        let body = &reply.body;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {}: {body}",
            reply.status
        );
        assert_eq!(
            error_code(body),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {body}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(VALIDATION_EXCEPTION),
            "case {case:?}: expected the error shape in the x-amzn-errortype header, got {:?}",
            reply.error_type
        );

        // The member this whole suite is about: present, and carrying the value that
        // classifies a broken constraint. A body missing it is the synthetic framework
        // shape rather than this service's own.
        let reason = body["reason"].as_str();
        assert_eq!(
            reason,
            Some(CONSTRAINT_VIOLATION),
            "case {case:?}: expected reason {CONSTRAINT_VIOLATION:?}, got {reason:?} in {body}"
        );

        // Nothing but the modelled shape: a `ValidationException` carries `message` and
        // `reason`, and restJson1's envelope adds `__type`.
        let members: BTreeSet<&str> = body
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {body}"))
            .keys()
            .map(String::as_str)
            .collect();
        let expected_members: BTreeSet<&str> = BODY_MEMBERS.into_iter().collect();
        assert_eq!(
            members, expected_members,
            "case {case:?}: expected exactly the modelled members, got {members:?} in {body}"
        );

        // And the message is still the counting envelope, stating every member the
        // request broke a constraint on.
        let text = message(body);
        let count = reported_count(&text);
        assert_eq!(
            count,
            Some(row.members.len()),
            "case {case:?}: expected the envelope to count {} violations, got {text:?}",
            row.members.len()
        );
        for member in &row.members {
            let clause = format!("at '{member}' failed to satisfy constraint: ");
            assert!(
                text.contains(&clause),
                "case {case:?}: expected a clause {clause:?}, got {text:?}"
            );
        }
    }
}

/// The two listings of this service that refuse the SAME constraint class on the same
/// member name through the two DIFFERENT paths: `ListQueries$maxResults` targets the
/// unconstrained prelude `Integer`, so its published minimum is enforced in the handler,
/// while `ListGraphs$maxResults` targets `MaxResults` (`@range(1..100)`), so the identical
/// violation is caught by the walk in front of the handler. One service answers one
/// constraint class one way, so the two answers must be indistinguishable.
#[tokio::test]
async fn one_constraint_class_is_answered_the_same_whichever_side_catches_it() {
    let router = emulator();
    let held = graph_in_service(&router, "one-class-graph").await;

    let handler_side = call(
        &router,
        "GET",
        "/queries?maxResults=0",
        Some(&held),
        None,
    )
    .await;
    let boundary_side = call(&router, "GET", "/graphs?maxResults=0", None, None).await;

    assert_eq!(
        (handler_side.status, boundary_side.status),
        (400, 400),
        "case maxResults below its minimum: expected 400 from both listings, got {} and {}",
        handler_side.status,
        boundary_side.status
    );
    assert_eq!(
        handler_side.error_type, boundary_side.error_type,
        "case maxResults below its minimum: expected one error shape header, got {:?} and {:?}",
        handler_side.error_type, boundary_side.error_type
    );
    assert_eq!(
        handler_side.body, boundary_side.body,
        "case maxResults below its minimum: expected one body for one constraint class, \
         got {} from the handler-enforced listing and {} from the boundary-enforced one",
        handler_side.body, boundary_side.body
    );
}

/// The `reason` is a CLASSIFICATION, not a stamp: a refusal of client input that is not a
/// constraint on a member's value keeps the value that names what it is.
#[tokio::test]
async fn a_refusal_that_is_not_a_constraint_violation_keeps_its_own_reason() {
    let router = emulator();
    // A graph that is serving, so the query-id row below is answered by the operation
    // rather than refused for an address or a plane that is not there.
    let serving = graph_in_service(&router, "illegal-argument-graph").await;

    // (case, method, path, graphIdentifier header) — each a refusal of client input the
    // service classifies as an argument it does not accept: a pagination token it never
    // issued, and a query id that names no query in flight.
    let rows: [(&str, &str, String, Option<&str>); 3] = [
        (
            "a nextToken this service never issued",
            "GET",
            "/graphs?nextToken=deadbeef".to_string(),
            None,
        ),
        (
            "a nextToken that is not even a token spelling",
            "GET",
            "/graphs?nextToken=not-a-token".to_string(),
            None,
        ),
        (
            "a queryId that names no query in flight",
            "GET",
            "/queries/6f3a1b2c-4d5e-4f60-8a91-b2c3d4e5f607".to_string(),
            Some(&serving),
        ),
    ];

    for (case, method, path, graph_identifier) in rows {
        let reply = call(&router, method, &path, graph_identifier, None).await;
        let body = &reply.body;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {}: {body}",
            reply.status
        );
        assert_eq!(
            error_code(body),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {body}"
        );
        let reason = body["reason"].as_str();
        assert_eq!(
            reason,
            Some(ILLEGAL_ARGUMENT),
            "case {case:?}: expected reason {ILLEGAL_ARGUMENT:?}, got {reason:?} in {body}"
        );
    }
}

/// A request that breaks no constraint is served and stored: the refusals above come from
/// the constraints the request broke, not from a walk that refuses everything.
#[tokio::test]
async fn a_request_inside_every_constraint_is_served_and_stored() {
    let router = emulator();

    // The published bounds themselves: the longest name the model accepts, the smallest
    // memory, the largest replica count, and a tag key of exactly the longest length the
    // service stores.
    let name = format!("{}{}", "n", "a".repeat(MAX_GRAPH_NAME_CHARS - 1));
    let key = "k".repeat(MAX_TAG_KEY_CHARS);
    let created = call(
        &router,
        "POST",
        "/graphs",
        None,
        Some(json!({
            "graphName": name,
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
            "replicaCount": 2,
            "kmsKeyIdentifier": KMS_KEY_ARN,
            "tags": { key.clone(): "stored" },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a request at every bound: expected 201, got {}: {}",
        created.status, created.body
    );
    let id = created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case a request at every bound: no id in {}", created.body))
        .to_string();

    // Read the record back through the getter: acceptance was not an empty success.
    let read = call(&router, "GET", &format!("/graphs/{id}"), None, None).await;
    assert_eq!(
        (read.status, read.body["name"].as_str()),
        (200, Some(name.as_str())),
        "case reading the created graph back: expected the stored graph, got {}: {}",
        read.status,
        read.body
    );

    // And the listings at the other bound of the same member the refusals above used.
    let listed = call(&router, "GET", "/graphs?maxResults=100", None, None).await;
    assert_eq!(
        (listed.status, listed.body["graphs"].as_array().map(Vec::len)),
        (200, Some(1)),
        "case a listing at the page-size maximum: expected the stored graph, got {}: {}",
        listed.status,
        listed.body
    );
    let one = call(&router, "GET", "/graphs?maxResults=1", None, None).await;
    assert_eq!(
        (one.status, one.body["graphs"].as_array().map(Vec::len)),
        (200, Some(1)),
        "case a listing at the page-size minimum: expected the stored graph, got {}: {}",
        one.status,
        one.body
    );
}
