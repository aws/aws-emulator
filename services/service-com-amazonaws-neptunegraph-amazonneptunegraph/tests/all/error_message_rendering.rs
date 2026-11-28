// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a failure, driven over the wire (`restJson1`) through the same
//! router the emulator serves from.
//!
//! Two rules, each asserted over a set of inputs rather than one recorded request:
//!
//! 1. A `ValidationException` for a value outside a `@pattern` reports the expression
//!    the value had to satisfy WITHOUT the anchors the model spells it with; every other
//!    constraint class keeps the wording it already had. The rows below span three
//!    operations, four pattern-constrained members with four different expressions, and
//!    the length / range / absent-member classes as controls.
//! 2. A `ResourceNotFoundException` for a graph identifier that names no stored graph
//!    reports the account the lookup resolved in as well as the identifier requested.
//!    The rows span every operation that resolves a `graphIdentifier` and several
//!    well-formed identifiers.
//! 3. That account-scoped wording names the KIND of resource that was looked for, and
//!    then the identifier clause as the OPERATION words it: every lookup but a task
//!    CANCEL spells the resource kind in front of that clause, while `CancelImportTask`
//!    and `CancelExportTask` carry the bare `task id`. The rows span every resource kind,
//!    each of its implemented lookups, and several well-formed identifiers per kind.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The modelled `GraphIdentifier` / `GraphId` pattern, as the model spells it
/// (`smithy_query(query="shape",shape="GraphIdentifier")`).
const GRAPH_IDENTIFIER_PATTERN: &str = "^g-[a-z0-9]{10}$";

/// The modelled `GraphName` pattern
/// (`smithy_query(query="shape",shape="GraphName")`).
const GRAPH_NAME_PATTERN: &str = "^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$";

/// The modelled `KmsKeyArn` pattern
/// (`smithy_query(query="shape",shape="KmsKeyArn")`).
const KMS_KEY_ARN_PATTERN: &str =
    "^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$";

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
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

/// A `@pattern` expression as a refusal must report it: the model's expression minus its
/// outer anchors. Written out here rather than reusing the service's own helper, so the
/// test states the expected text independently of the code under test.
fn expected_expression(model_pattern: &str) -> String {
    model_pattern
        .trim_start_matches('^')
        .trim_end_matches('$')
        .to_string()
}

#[tokio::test]
async fn a_constraint_violation_reports_the_pattern_without_its_anchors() {
    let router = emulator();

    // (case, method, path, body, the whole message the refusal must carry)
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "GetGraph graphIdentifier that is not a graph id at all",
            "GET",
            "/graphs/not-a-valid-id".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "GetGraph graphIdentifier with an uppercase suffix",
            "GET",
            "/graphs/g-ABCDEFGHIJ".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'g-ABCDEFGHIJ' at 'graphIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "GetGraph graphIdentifier with a short suffix",
            "GET",
            "/graphs/g-0123".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'g-0123' at 'graphIdentifier' failed \
                 to satisfy constraint: Member must satisfy regular expression pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "GetGraph graphIdentifier carrying a snapshot id",
            "GET",
            "/graphs/gs-0123456789".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'gs-0123456789' at 'graphIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "UpdateGraph graphIdentifier outside the pattern",
            "PATCH",
            "/graphs/not-a-valid-id".to_string(),
            Some(json!({"provisionedMemory": 32})),
            format!(
                "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "DeleteGraph graphIdentifier outside the pattern",
            "DELETE",
            "/graphs/not-a-valid-id?skipSnapshot=true".to_string(),
            None,
            format!(
                "1 validation error detected: Value 'not-a-valid-id' at 'graphIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(GRAPH_IDENTIFIER_PATTERN)
            ),
        ),
        (
            "CreateGraph kmsKeyIdentifier outside the pattern",
            "POST",
            "/graphs".to_string(),
            Some(json!({
                "graphName": "kms-graph",
                "provisionedMemory": 16,
                "kmsKeyIdentifier": "not-a-kms-arn",
            })),
            format!(
                "1 validation error detected: Value 'not-a-kms-arn' at 'kmsKeyIdentifier' \
                 failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {}",
                expected_expression(KMS_KEY_ARN_PATTERN)
            ),
        ),
        (
            // `GraphName`'s expression opens with a negative lookahead the boundary
            // validator cannot compile, so the handler enforces it — and words it with
            // the same builder, which is what this row pins.
            "CreateGraph graphName reserved for graph ids",
            "POST",
            "/graphs".to_string(),
            Some(json!({"graphName": "g-reserved", "provisionedMemory": 16})),
            format!(
                "1 validation error detected: Value 'g-reserved' at 'graphName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: {}",
                expected_expression(GRAPH_NAME_PATTERN)
            ),
        ),
        (
            "CreateGraph graphName with two consecutive hyphens",
            "POST",
            "/graphs".to_string(),
            Some(json!({"graphName": "my--graph", "provisionedMemory": 16})),
            format!(
                "1 validation error detected: Value 'my--graph' at 'graphName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: {}",
                expected_expression(GRAPH_NAME_PATTERN)
            ),
        ),
        // Controls: a constraint class that is NOT a pattern keeps the wording it had,
        // so the fix cannot have reworded the rest of the envelope.
        (
            "CreateGraph graphName below its length minimum",
            "POST",
            "/graphs".to_string(),
            Some(json!({"graphName": "", "provisionedMemory": 16})),
            "1 validation error detected: Value '' at 'graphName' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "CreateGraph provisionedMemory below the documented floor",
            "POST",
            "/graphs".to_string(),
            Some(json!({"graphName": "under-memory", "provisionedMemory": 15})),
            "1 validation error detected: Value '15' at 'provisionedMemory' failed to \
             satisfy constraint: Member must have value greater than or equal to 16"
                .to_string(),
        ),
        (
            "CreateGraph replicaCount above its range maximum",
            "POST",
            "/graphs".to_string(),
            Some(json!({
                "graphName": "over-replicas",
                "provisionedMemory": 16,
                "replicaCount": 3,
            })),
            "1 validation error detected: Value '3' at 'replicaCount' failed to satisfy \
             constraint: Member must have value less than or equal to 2"
                .to_string(),
        ),
        (
            "CreateGraph without the required graphName",
            "POST",
            "/graphs".to_string(),
            Some(json!({"provisionedMemory": 16})),
            "1 validation error detected: Value null at 'graphName' failed to \
             satisfy constraint: Member must not be null"
                .to_string(),
        ),
    ];

    for (case, method, path, body, expected) in rows {
        let (status, answer) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The expression a pattern refusal reports is usable as-is: it carries neither the
/// leading `^` nor the trailing `$` of the model's own spelling.
#[tokio::test]
async fn a_reported_pattern_carries_no_outer_anchors() {
    let router = emulator();

    // (case, method, path, body) — one row per pattern-constrained member this service
    // can be made to refuse.
    let rows: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "graphIdentifier",
            "GET",
            "/graphs/not-a-valid-id",
            None,
        ),
        (
            "kmsKeyIdentifier",
            "POST",
            "/graphs",
            Some(json!({
                "graphName": "anchor-kms",
                "provisionedMemory": 16,
                "kmsKeyIdentifier": "not-a-kms-arn",
            })),
        ),
        (
            "graphName",
            "POST",
            "/graphs",
            Some(json!({"graphName": "g-reserved", "provisionedMemory": 16})),
        ),
    ];

    const CLAUSE: &str = "Member must satisfy regular expression pattern: ";
    for (case, method, path, body) in rows {
        let (status, answer) = call(&router, method, path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {answer}"
        );
        let text = message(&answer);
        let expression = text
            .split_once(CLAUSE)
            .map(|(_, tail)| tail.to_string())
            .unwrap_or_else(|| panic!("case {case:?}: no pattern clause in {text:?}"));
        assert!(
            !expression.starts_with('^'),
            "case {case:?}: the reported expression must carry no leading anchor, got \
             {expression:?}"
        );
        assert!(
            !expression.ends_with('$'),
            "case {case:?}: the reported expression must carry no trailing anchor, got \
             {expression:?}"
        );
    }
}

#[tokio::test]
async fn a_missing_graph_is_reported_against_the_account_and_the_requested_id() {
    let router = emulator();

    // A stored graph, so the store is not simply empty: the failures below are about the
    // identifier that was asked for, not about an empty service.
    let (created_status, created) = call(
        &router,
        "POST",
        "/graphs",
        Some(json!({"graphName": "present-graph", "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(
        created_status, 201,
        "the fixture graph must be created, got {created_status}: {created}"
    );

    // The one canonical emulator account every request is served for — read from the
    // framework rather than written out, so the assertion cannot drift from the account
    // the service mints ARNs with.
    let account = engine::emulator_account_id();

    // Well-formed identifiers (the modelled `^g-[a-z0-9]{10}$`) that name no stored
    // graph: the extremes of the id space, a mixed one, and the id the fixture graph
    // does NOT hold.
    let absent_ids = [
        "g-0000000000",
        "g-zzzzzzzzzz",
        "g-a1b2c3d4e5",
        "g-9999999999",
    ];
    // (operation, method, the path template, body) — every operation that resolves a
    // `graphIdentifier`.
    let operations: [(&str, &str, &str, Option<Value>); 3] = [
        ("GetGraph", "GET", "/graphs/{id}", None),
        (
            "UpdateGraph",
            "PATCH",
            "/graphs/{id}",
            Some(json!({"provisionedMemory": 32})),
        ),
        (
            "DeleteGraph",
            "DELETE",
            "/graphs/{id}?skipSnapshot=true",
            None,
        ),
    ];

    for (operation, method, template, body) in operations {
        for id in absent_ids {
            let case = format!("{operation} for {id}");
            let path = template.replace("{id}", id);
            let (status, answer) = call(&router, method, &path, body.clone()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer}"
            );
            let actual = message(&answer);
            let expected =
                format!("No graph found for customer [{account}] with graph id [{id}]");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The graph that IS stored still reads back, so the not-found wording did not come
    // from a lookup that stopped working.
    let (listed_status, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        (listed_status, listed["graphs"].as_array().map(Vec::len)),
        (200, Some(1)),
        "the stored graph must still be listed, got {listed_status}: {listed}"
    );
}

/// One implemented lookup of a resource kind: the operation, its HTTP method, its path
/// template (with `{id}` where the identifier goes), the body it needs, and the
/// identifier clause this operation words its refusal with.
type Lookup = (
    &'static str,
    &'static str,
    &'static str,
    Option<Value>,
    &'static str,
);

/// One account-scoped resource kind: the kind as the service names it, the lookups that
/// resolve it, and identifiers that name nothing.
type ResourceClass = (&'static str, Vec<Lookup>, Vec<String>);

/// Every account-scoped lookup words "this account holds no such resource" the same way:
/// the KIND of resource it searched for, the account it searched in, and the identifier
/// clause its own operation words.
///
/// The table spans what the rule generalises over: the resource kind (graph, snapshot,
/// import task, export task, and a resource addressed by ARN), the operation that
/// resolves it (read and delete/cancel, GET, DELETE and POST) and so the clause each
/// words, and the identifier value (the extremes of each id space, plus a mixed one, and
/// for an ARN a changed resource type, resource id and partition).
#[tokio::test]
async fn every_missing_resource_is_reported_with_its_own_resource_kind() {
    let router = emulator();

    // A stored graph, so no lookup below is answered out of an empty store.
    let (created_status, created) = call(
        &router,
        "POST",
        "/graphs",
        Some(json!({"graphName": "kinds-graph", "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(
        created_status, 201,
        "the fixture graph must be created, got {created_status}: {created}"
    );

    // The one canonical emulator account every request is served for — read from the
    // framework rather than written out.
    let account = engine::emulator_account_id();

    // Well-formed identifiers that name nothing: the extremes of each id space and a
    // mixed value, per the modelled patterns `^g-[a-z0-9]{10}$` (`GraphIdentifier`),
    // `^gs-[a-z0-9]{10}$` (`SnapshotIdentifier`), `^t-[a-z0-9]{10}$` (`TaskId` /
    // `ExportTaskId`) and `^arn:.+$` (`Arn`).
    let graph_ids = ["g-0000000000", "g-a1b2c3d4e5", "g-zzzzzzzzzz"];
    let snapshot_ids = ["gs-0000000000", "gs-a1b2c3d4e5", "gs-zzzzzzzzzz"];
    let task_ids = ["t-0000000000", "t-a1b2c3d4e5", "t-zzzzzzzzzz"];
    // `Arn` constrains only the `arn:` prefix, so these are well formed. Each is written
    // with a colon in front of the resource id rather than a slash, so the whole ARN fits
    // the one NON-greedy `{resourceArn}` label (`/tags/{resourceArn}`) as a single path
    // segment. The rows differ in the resource type, the resource id and the partition.
    let absent_arns = [
        format!("arn:aws:neptune-graph:{REGION}:{account}:graph:g-0000000000"),
        format!("arn:aws:neptune-graph:{REGION}:{account}:graph-snapshot:gs-0123456789"),
        format!("arn:aws-cn:neptune-graph:cn-north-1:{account}:graph:g-a1b2c3d4e5"),
    ];

    // (resource kind as the service names it, the implemented lookups that resolve it
    // with the identifier clause each words, the identifiers that name nothing)
    let classes: Vec<ResourceClass> = vec![
        (
            "graph",
            vec![
                ("GetGraph", "GET", "/graphs/{id}", None, "graph id"),
                (
                    "UpdateGraph",
                    "PATCH",
                    "/graphs/{id}",
                    Some(json!({"provisionedMemory": 32})),
                    "graph id",
                ),
                (
                    "DeleteGraph",
                    "DELETE",
                    "/graphs/{id}?skipSnapshot=true",
                    None,
                    "graph id",
                ),
            ],
            graph_ids.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "snapshot",
            vec![(
                "GetGraphSnapshot",
                "GET",
                "/snapshots/{id}",
                None,
                "snapshot id",
            )],
            snapshot_ids.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "import task",
            vec![
                (
                    "GetImportTask",
                    "GET",
                    "/importtasks/{id}",
                    None,
                    "import task id",
                ),
                (
                    "CancelImportTask",
                    "DELETE",
                    "/importtasks/{id}",
                    None,
                    "task id",
                ),
            ],
            task_ids.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "export task",
            vec![
                (
                    "GetExportTask",
                    "GET",
                    "/exporttasks/{id}",
                    None,
                    "export task id",
                ),
                (
                    "CancelExportTask",
                    "DELETE",
                    "/exporttasks/{id}",
                    None,
                    "task id",
                ),
            ],
            task_ids.iter().map(|id| (*id).to_string()).collect(),
        ),
        (
            "resource",
            vec![
                (
                    "ListTagsForResource",
                    "GET",
                    "/tags/{id}",
                    None,
                    "resource arn",
                ),
                (
                    "TagResource",
                    "POST",
                    "/tags/{id}",
                    Some(json!({"tags": {"owner": "tests"}})),
                    "resource arn",
                ),
            ],
            absent_arns.to_vec(),
        ),
    ];

    for (resource, operations, identifiers) in classes {
        for (operation, method, template, body, clause) in operations {
            for identifier in &identifiers {
                let case = format!("{operation} for {identifier}");
                let path = template.replace("{id}", identifier);
                let (status, answer) = call(&router, method, &path, body.clone()).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404, got {status}: {answer}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {answer}"
                );
                let actual = message(&answer);
                let expected = format!(
                    "No {resource} found for customer [{account}] with {clause} [{identifier}]"
                );
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }

    // The resource that IS stored still reads back, so the wording above did not come
    // from a lookup that stopped finding anything.
    let (read_status, read) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        (read_status, read["graphs"].as_array().map(Vec::len)),
        (200, Some(1)),
        "the stored graph must still be listed, got {read_status}: {read}"
    );
}
