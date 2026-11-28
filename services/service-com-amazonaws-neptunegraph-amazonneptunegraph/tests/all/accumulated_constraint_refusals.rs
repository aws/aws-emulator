// SPDX-License-Identifier: CC0-1.0

//! How this service COUNTS the constraints one request broke, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a request is walked to the END before it is
//! refused, so one `ValidationException` reports EVERY constraint it broke — the count in
//! the envelope is the number of violations, and each violation contributes exactly one
//! clause — whether the constraint was caught by the model-driven walk in front of the
//! handler or enforced by the handler itself.
//!
//! The tables below span what that rule generalises over: the operation (six of them),
//! the number of constraints one request breaks (1, 2 and 5), the constraint class (a
//! pattern with a lookahead the boundary walk cannot compile, a stored tag bound, a URI
//! form, a blank member, a documented dependency between two members, a bound between two
//! members, and the length / range classes the model itself carries), and which side of
//! the boundary caught it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the service documents ("Min = 16"), which is all a
/// graph these tests create needs. Below the modelled `@range` minimum (1), so the floor
/// is enforced by the handler rather than by the model-driven walk.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The largest `provisionedMemory` the model allows
/// (`smithy_query(query="shape",shape="ProvisionedMemory")` -> `@range max`), which the
/// model-driven walk refuses a value past.
const MAX_PROVISIONED_MEMORY: i64 = 24_576;

/// The longest tag key this service stores: `TagKey` is `@length(1..128)`
/// (`smithy_query(query="shape",shape="TagKey")`). The boundary walk checks a map's entry
/// count and its VALUES, so a key past this bound is the one tag violation that reaches a
/// handler.
const MAX_TAG_KEY_CHARS: usize = 128;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneAccumulationRole";

/// A well-formed `SnapshotIdentifier` (`^gs-[a-z0-9]{10}$`) that names no snapshot: a
/// request is validated before anything is looked up, so a restore of it is still refused
/// for the constraints it broke.
const ABSENT_SNAPSHOT_ID: &str = "gs-0123456789";

/// The modelled `GraphName` pattern (`smithy_query(query="shape",shape="GraphName")`).
const GRAPH_NAME_PATTERN: &str = "^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$";

/// The modelled `SnapshotName` pattern
/// (`smithy_query(query="shape",shape="SnapshotName")`).
const SNAPSHOT_NAME_PATTERN: &str = "^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$";

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode one path segment, so an ARN fits the non-greedy `{resourceArn}` label
/// of `/tags/{resourceArn}` as a single segment.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

/// The envelope a refusal over `clauses` must carry, restated here independently of the
/// service's own builder: the count of violated constraints, then one clause each.
fn expected_envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The clauses a refusal reports, read back out of its envelope: everything after the
/// count, split on the one separator the envelope joins clauses with.
fn reported_clauses(text: &str) -> Vec<String> {
    let body = text.split_once(": ").map_or("", |(_, tail)| tail);
    if body.is_empty() {
        return Vec::new();
    }
    body.split("; ").map(str::to_string).collect()
}

/// The count a refusal's envelope opens with, read back out of it.
fn reported_count(text: &str) -> Option<usize> {
    text.split_whitespace().next()?.parse().ok()
}

/// A `@pattern` clause, as a refusal must word it: the expression WITHOUT the outer
/// anchors the model spells it with.
fn pattern_clause(member: &str, value: &str, model_pattern: &str) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {}",
        model_pattern.trim_start_matches('^').trim_end_matches('$')
    )
}

/// The clause a tag key longer than the stored bound fails, restated here independently
/// of the service's own builders: a bound on a map's KEYS is reported against the member
/// the map sits at, listing everything a key of that map has to satisfy — the key shape is
/// `@length(1..128)` and `@pattern("^[a-zA-Z+-=._:/]+$")`
/// (`smithy_query(query="shape",shape="TagKey")`), the pattern reported without its
/// anchors as this service reports every pattern — and, being a length bound, states those
/// requirements without echoing the key that broke one.
fn tag_key_clause() -> String {
    format!(
        "Value at 'tags' failed to satisfy constraint: Map keys must satisfy constraint: \
         [Member must have length less than or equal to {MAX_TAG_KEY_CHARS}, Member must \
         have length greater than or equal to 1, Member must satisfy regular expression \
         pattern: [a-zA-Z+-=._:/]+]"
    )
}

/// A tag map whose single key is `key_chars` characters long.
fn oversized_tags(key_chars: usize) -> Value {
    json!({ "k".repeat(key_chars): "v" })
}

/// Create a graph and return its id and ARN, for the operations that address one.
async fn fixture_graph(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture graph {name:?} must be created, got {status}: {body}"
    );
    (
        body["id"].as_str().unwrap_or_default().to_string(),
        body["arn"].as_str().unwrap_or_default().to_string(),
    )
}

/// One refusal to check: the case, the request, and every clause it must report.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    clauses: Vec<String>,
}

#[tokio::test]
async fn a_refusal_reports_every_constraint_the_request_broke() {
    let router = emulator();
    let (graph_id, graph_arn) = fixture_graph(&router, "counting-graph").await;

    // A key one character past the stored bound, and one far past it: a refusal states
    // the bound rather than the length it was given, so both rows must earn the one
    // clause.
    let one_past = MAX_TAG_KEY_CHARS + 1;
    let far_past = MAX_TAG_KEY_CHARS * 2;

    let rows: Vec<Row> = vec![
        // ONE handler-enforced violation: the count is 1 and the clause stands alone.
        Row {
            case: "CreateGraph: a graphName inside the reserved id space",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "g-reserved",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
            })),
            clauses: vec![pattern_clause(
                "graphName",
                "g-reserved",
                GRAPH_NAME_PATTERN,
            )],
        },
        Row {
            case: "CreateGraph: a tag key one character past the stored bound",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "one-past-tag-key",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": oversized_tags(one_past),
            })),
            clauses: vec![tag_key_clause()],
        },
        // TWO handler-enforced violations, in four different operations: the same two
        // broken members are reported as two clauses by each of them.
        Row {
            case: "CreateGraph: a reserved graphName AND an oversized tag key",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "g-reserved",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": oversized_tags(far_past),
            })),
            clauses: vec![
                pattern_clause("graphName", "g-reserved", GRAPH_NAME_PATTERN),
                tag_key_clause(),
            ],
        },
        Row {
            case: "RestoreGraphFromSnapshot: a reserved graphName AND an oversized tag \
                   key",
            method: "POST",
            path: format!("/snapshots/{ABSENT_SNAPSHOT_ID}/restore"),
            body: Some(json!({
                "graphName": "g-reserved",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": oversized_tags(far_past),
            })),
            clauses: vec![
                pattern_clause("graphName", "g-reserved", GRAPH_NAME_PATTERN),
                tag_key_clause(),
            ],
        },
        Row {
            case: "CreateGraphSnapshot: a reserved snapshotName AND an oversized tag key",
            method: "POST",
            path: "/snapshots".to_string(),
            body: Some(json!({
                "graphIdentifier": graph_id,
                "snapshotName": "gs-reserved",
                "tags": oversized_tags(one_past),
            })),
            clauses: vec![
                pattern_clause("snapshotName", "gs-reserved", SNAPSHOT_NAME_PATTERN),
                tag_key_clause(),
            ],
        },
        Row {
            case: "StartExportTask: a destination that is no S3 URI AND an oversized tag \
                   key",
            method: "POST",
            path: "/exporttasks".to_string(),
            body: Some(json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "kmsKeyIdentifier": KMS_KEY_ARN,
                "format": "PARQUET",
                "destination": "not-a-bucket-uri",
                "tags": oversized_tags(far_past),
            })),
            clauses: vec![
                "Value 'not-a-bucket-uri' at 'destination' failed to satisfy constraint: \
                 Member must be an Amazon S3 URI of the form s3://bucket[/key-prefix]"
                    .to_string(),
                tag_key_clause(),
            ],
        },
        // TWO violations of a different pair of classes: a blank member and a documented
        // dependency between two members.
        Row {
            case: "StartImportTask: a blank source AND an N-Triples load with no \
                   blankNodeHandling",
            method: "POST",
            path: format!("/graphs/{graph_id}/importtasks"),
            body: Some(json!({
                "source": "   ",
                "format": "NTRIPLES",
                "roleArn": ROLE_ARN,
            })),
            clauses: vec![
                "Value at 'source' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1"
                    .to_string(),
                "Value null at 'blankNodeHandling' failed to satisfy constraint: Member \
                 must be provided when format is NTRIPLES"
                    .to_string(),
            ],
        },
        Row {
            case: "StartImportTask: a blank source alone",
            method: "POST",
            path: format!("/graphs/{graph_id}/importtasks"),
            body: Some(json!({
                "source": "",
                "format": "CSV",
                "roleArn": ROLE_ARN,
            })),
            clauses: vec![
                "Value at 'source' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1"
                    .to_string(),
            ],
        },
        // FIVE violations at once, every one of them handler-enforced.
        Row {
            case: "CreateGraphUsingImportTask: five broken constraints in one request",
            method: "POST",
            path: "/importtasks".to_string(),
            body: Some(json!({
                "graphName": "g-reserved",
                "source": "",
                "format": "NTRIPLES",
                "roleArn": ROLE_ARN,
                "minProvisionedMemory": 1024,
                "maxProvisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": oversized_tags(one_past),
            })),
            clauses: vec![
                pattern_clause("graphName", "g-reserved", GRAPH_NAME_PATTERN),
                "Value at 'source' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1"
                    .to_string(),
                "Value null at 'blankNodeHandling' failed to satisfy constraint: Member \
                 must be provided when format is NTRIPLES"
                    .to_string(),
                format!(
                    "Value '1024' at 'minProvisionedMemory' failed to satisfy constraint: \
                     Member must be less than or equal to maxProvisionedMemory \
                     ({MIN_PROVISIONED_MEMORY})"
                ),
                tag_key_clause(),
            ],
        },
        // TagResource: one handler-enforced violation, reported through the same counting
        // envelope as every other.
        Row {
            case: "TagResource: a tag key past the stored bound",
            method: "POST",
            path: format!("/tags/{}", encode(&graph_arn)),
            body: Some(json!({"tags": oversized_tags(far_past)})),
            clauses: vec![tag_key_clause()],
        },
        // Controls on the OTHER side of the boundary: the model-driven walk counts the
        // same way, so a refusal cannot be told apart by its count or its form.
        Row {
            case: "CreateGraph: two model constraints broken at once",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "boundary-pair",
                "provisionedMemory": MAX_PROVISIONED_MEMORY + 1,
                "replicaCount": 3,
            })),
            clauses: vec![
                format!(
                    "Value '{}' at 'provisionedMemory' failed to satisfy constraint: \
                     Member must have value less than or equal to {MAX_PROVISIONED_MEMORY}",
                    MAX_PROVISIONED_MEMORY + 1
                ),
                "Value '3' at 'replicaCount' failed to satisfy constraint: Member must \
                 have value less than or equal to 2"
                    .to_string(),
            ],
        },
        Row {
            case: "CreateGraph: one model constraint broken",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "boundary-single",
                "provisionedMemory": MAX_PROVISIONED_MEMORY + 1,
            })),
            clauses: vec![format!(
                "Value '{}' at 'provisionedMemory' failed to satisfy constraint: Member \
                 must have value less than or equal to {MAX_PROVISIONED_MEMORY}",
                MAX_PROVISIONED_MEMORY + 1
            )],
        },
        // The documented memory floor sits BELOW the modelled range's own minimum, so it
        // is handler-enforced and counts beside a handler-enforced name violation in the
        // one envelope.
        Row {
            case: "CreateGraph: a reserved graphName AND memory below the documented floor",
            method: "POST",
            path: "/graphs".to_string(),
            body: Some(json!({
                "graphName": "g-reserved",
                "provisionedMemory": MIN_PROVISIONED_MEMORY - 1,
            })),
            clauses: vec![
                pattern_clause("graphName", "g-reserved", GRAPH_NAME_PATTERN),
                format!(
                    "Value '{}' at 'provisionedMemory' failed to satisfy constraint: \
                     Member must have value greater than or equal to \
                     {MIN_PROVISIONED_MEMORY}",
                    MIN_PROVISIONED_MEMORY - 1
                ),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answer) = call(&router, row.method, &row.path, row.body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        let text = message(&answer);
        let expected_count = row.clauses.len();
        let actual_count = reported_count(&text);
        assert_eq!(
            actual_count,
            Some(expected_count),
            "case {case:?}: expected the envelope to count {expected_count} violations, \
             got {text:?}"
        );
        // Every clause the request earned, and nothing else. The walk order differs
        // between the two sides of the boundary, so the clauses are compared as a set.
        let mut actual_clauses = reported_clauses(&text);
        let mut expected_clauses = row.clauses.clone();
        actual_clauses.sort();
        expected_clauses.sort();
        assert_eq!(
            actual_clauses, expected_clauses,
            "case {case:?}: expected clauses {expected_clauses:?}, got {actual_clauses:?} \
             in {text:?}"
        );
    }
}

#[tokio::test]
async fn the_same_broken_members_are_counted_the_same_by_every_operation_that_checks_them() {
    let router = emulator();

    // (case, the graphName that breaks the name rule, the length of the tag key that
    // breaks the stored bound) — the rows differ in the clause of the name pattern they
    // break and in how far past the bound the key is.
    let inputs: [(&str, &str, usize); 3] = [
        ("the reserved id prefix", "g-reserved", MAX_TAG_KEY_CHARS + 1),
        ("two consecutive hyphens", "my--graph", MAX_TAG_KEY_CHARS + 7),
        ("a trailing hyphen", "my-graph-", MAX_TAG_KEY_CHARS * 3),
    ];

    for (case, graph_name, key_chars) in inputs {
        let expected = expected_envelope(&[
            pattern_clause("graphName", graph_name, GRAPH_NAME_PATTERN),
            tag_key_clause(),
        ]);

        // Every operation that enforces exactly these two constraints, each sent the same
        // two broken members.
        let requests: [(&str, String, Value); 3] = [
            (
                "CreateGraph",
                "/graphs".to_string(),
                json!({
                    "graphName": graph_name,
                    "provisionedMemory": MIN_PROVISIONED_MEMORY,
                    "tags": oversized_tags(key_chars),
                }),
            ),
            (
                "RestoreGraphFromSnapshot",
                format!("/snapshots/{ABSENT_SNAPSHOT_ID}/restore"),
                json!({
                    "graphName": graph_name,
                    "provisionedMemory": MIN_PROVISIONED_MEMORY,
                    "tags": oversized_tags(key_chars),
                }),
            ),
            (
                "CreateGraphUsingImportTask",
                "/importtasks".to_string(),
                json!({
                    "graphName": graph_name,
                    "source": "s3://bucket/data",
                    "roleArn": ROLE_ARN,
                    "tags": oversized_tags(key_chars),
                }),
            ),
        ];

        for (operation, path, body) in requests {
            let label = format!("{operation} with {case}");
            let (status, answer) = call(&router, "POST", &path, Some(body)).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status}: {answer}"
            );
            let actual = message(&answer);
            assert_eq!(
                actual, expected,
                "case {label:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_at_every_bound_is_still_accepted_and_stored() {
    let router = emulator();

    // The published bounds themselves: a name the pattern accepts and a tag key of
    // exactly the longest length the service stores. Nothing here breaks a constraint, so
    // the accumulating walk must refuse nothing.
    let key = "k".repeat(MAX_TAG_KEY_CHARS);
    let (status, created) = call(
        &router,
        "POST",
        "/graphs",
        Some(json!({
            "graphName": "at-the-bounds",
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
            "tags": { key.clone(): "stored" },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case a request at every bound: expected 201, got {status}: {created}"
    );
    let arn = created["arn"].as_str().unwrap_or_default().to_string();

    // Read the record back: the accepted tag is what the store holds, so acceptance was
    // not an empty success.
    let (read_status, read) = call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
    assert_eq!(
        read_status, 200,
        "case reading the stored tags back: expected 200, got {read_status}: {read}"
    );
    let actual = read["tags"][&key].as_str();
    assert_eq!(
        actual,
        Some("stored"),
        "case reading the stored tags back: expected the tag at the key bound to be \
         stored, got {read}"
    );

    // And the graph itself reads back through its own getter.
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{id}"), None).await;
    assert_eq!(
        (graph_status, graph["name"].as_str()),
        (200, Some("at-the-bounds")),
        "case reading the created graph back: expected the stored graph, got \
         {graph_status}: {graph}"
    );
}
