// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the COMPUTE configuration of a graph, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A graph is served either by a provisioned graph (dedicated compute, sized in
//!   m-NCUs) or by a graph session (ephemeral compute, floored in s-NCUs)
//!   (`smithy_query(query="doc",shape="CreateGraph")`). A graph holds ONE such
//!   configuration, and every graph record reports it through both of the members that
//!   name it: the current `computeConfiguration`, and the DEPRECATED `provisionedMemory`,
//!   which "returns `memory` for a `provisionedGraph` and `minMemory` for a
//!   `graphSession`" (`smithy_query(query="doc",shape="GraphSummary",
//!   member="provisionedMemory")`).
//! * The two input members name the SAME quantity, so a request may carry either and
//!   not both, and a provisioned graph's memory satisfies the documented floor ("Min =
//!   16", `smithy_query(query="doc",shape="CreateGraphInput",member="provisionedMemory")`)
//!   whichever member names it — the modelled `@range` on `ProvisionedMemory` floors it
//!   at 1, so that part of the documented range is refused by the service itself.
//! * A create that names neither is refused: neither member's documentation publishes a
//!   default. An update that names neither leaves the stored compute alone, and a
//!   restore that names neither comes up with the compute the snapshot captured.
//! * An update REPLACES the compute configuration, in either direction: a graph session
//!   scales up to a provisioned graph and a provisioned graph scales down to a graph
//!   session (`smithy_query(query="doc",shape="UpdateGraphInput",
//!   member="computeConfiguration")`).
//!
//! So each table below spans what its rule generalises over: the compute type, which of
//! the two members names it, the memory value (each published bound and one past it),
//! and the operation that reads or writes it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest memory a provisioned graph may be asked for, from the member's own
/// documentation: "Min = 16"
/// (`smithy_query(query="doc",shape="CreateGraphInput",member="provisionedMemory")`).
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The largest memory a provisioned graph may be asked for: the modelled `@range`
/// maximum on `ProvisionedMemory`
/// (`smithy_query(query="shape",shape="ProvisionedMemory")`).
const MAX_PROVISIONED_MEMORY: i64 = 24_576;

/// The smallest floor a graph session may be given: the modelled `@range` minimum on
/// `GraphSessionMemory` (`smithy_query(query="shape",shape="GraphSessionConfiguration")`
/// -> `minMemory`, `range 4..8`).
const MIN_SESSION_MEMORY: i64 = 4;

/// The largest floor a graph session may be given, from the same `@range` trait.
const MAX_SESSION_MEMORY: i64 = 8;

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

/// The `message` of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Poll one resource until it reports `AVAILABLE`, which the clock makes due on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// `POST /graphs` with `body`, which carries every member but the name.
async fn create(router: &MultiServiceRouter, name: &str, body: &Value) -> (u16, Value) {
    let mut request = body.clone();
    request["graphName"] = json!(name);
    call(router, "POST", "/graphs", Some(request)).await
}

/// Create a graph and wait it into service, returning its id and the record
/// `CreateGraph` reported.
async fn graph_in_service(router: &MultiServiceRouter, name: &str, body: &Value) -> (String, Value) {
    let (status, created) = create(router, name, body).await;
    assert_eq!(
        status, 201,
        "case {name:?} ({body}): expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    (id, created)
}

/// The compute configuration a request asks for through the DEPRECATED member.
fn deprecated_memory(memory: i64) -> Value {
    json!({"provisionedMemory": memory})
}

/// The compute configuration a request asks for through the CURRENT member, as a
/// provisioned graph.
fn provisioned_graph(memory: i64) -> Value {
    json!({"computeConfiguration": {"provisionedGraph": {"memory": memory}}})
}

/// The compute configuration a request asks for through the CURRENT member, as a graph
/// session with the floor `min_memory` when it names one.
fn graph_session(min_memory: Option<i64>) -> Value {
    match min_memory {
        Some(floor) => json!({"computeConfiguration": {"graphSession": {"minMemory": floor}}}),
        None => json!({"computeConfiguration": {"graphSession": {}}}),
    }
}

/// One row of the compute table: the case, the compute members of the request, the
/// `computeConfiguration` every record must report, and the value the deprecated
/// `provisionedMemory` member must carry (`None` for a session nobody floored, which
/// has no number to report).
struct ComputeCase {
    case: &'static str,
    name: &'static str,
    request: Value,
    configuration: Value,
    reported_memory: Option<i64>,
}

/// Every well-formed way a request names a graph's compute, spanning the compute type,
/// the member that names it, and each published bound of the memory it carries.
fn compute_cases() -> Vec<ComputeCase> {
    vec![
        ComputeCase {
            case: "the deprecated member at the documented floor",
            name: "deprecated-floor",
            request: deprecated_memory(MIN_PROVISIONED_MEMORY),
            configuration: json!({"provisionedGraph": {"memory": MIN_PROVISIONED_MEMORY}}),
            reported_memory: Some(MIN_PROVISIONED_MEMORY),
        },
        ComputeCase {
            case: "the deprecated member at the modelled maximum",
            name: "deprecated-maximum",
            request: deprecated_memory(MAX_PROVISIONED_MEMORY),
            configuration: json!({"provisionedGraph": {"memory": MAX_PROVISIONED_MEMORY}}),
            reported_memory: Some(MAX_PROVISIONED_MEMORY),
        },
        ComputeCase {
            case: "a provisioned graph through the current member",
            name: "provisioned-graph",
            request: provisioned_graph(128),
            configuration: json!({"provisionedGraph": {"memory": 128}}),
            reported_memory: Some(128),
        },
        ComputeCase {
            case: "a graph session floored at the modelled minimum",
            name: "session-floor",
            request: graph_session(Some(MIN_SESSION_MEMORY)),
            configuration: json!({"graphSession": {"minMemory": MIN_SESSION_MEMORY}}),
            reported_memory: Some(MIN_SESSION_MEMORY),
        },
        ComputeCase {
            case: "a graph session floored at the modelled maximum",
            name: "session-ceiling",
            request: graph_session(Some(MAX_SESSION_MEMORY)),
            configuration: json!({"graphSession": {"minMemory": MAX_SESSION_MEMORY}}),
            reported_memory: Some(MAX_SESSION_MEMORY),
        },
        ComputeCase {
            case: "a graph session nobody floored",
            name: "session-unfloored",
            request: graph_session(None),
            configuration: json!({"graphSession": {}}),
            reported_memory: None,
        },
    ]
}

/// Assert that `record` reports `configuration` through the current member and
/// `reported_memory` through the deprecated one.
fn assert_compute(case: &str, where_from: &str, record: &Value, expected: &ComputeCase) {
    assert_eq!(
        &record["computeConfiguration"], &expected.configuration,
        "case {case:?}: {where_from} computeConfiguration: expected {}, got {}",
        expected.configuration, record["computeConfiguration"]
    );
    let actual = record["provisionedMemory"].as_i64();
    assert_eq!(
        actual, expected.reported_memory,
        "case {case:?}: {where_from} provisionedMemory: expected {:?}, got {actual:?} in \
         {record}",
        expected.reported_memory
    );
}

#[tokio::test]
async fn every_graph_record_reports_the_compute_it_was_created_with() {
    let router = emulator();

    for expected in compute_cases() {
        let case = expected.case;
        let (id, created) = graph_in_service(&router, expected.name, &expected.request).await;
        assert_compute(case, "CreateGraph", &created, &expected);

        // ROUND TRIP: the compute is persisted, not echoed.
        let (status, fetched) = call(&router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {fetched}"
        );
        assert_compute(case, "GetGraph", &fetched, &expected);

        // The listing projection of the same record reports the same two members.
        let (status, listed) = call(&router, "GET", "/graphs", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListGraphs, got {status}: {listed}"
        );
        let summary = listed["graphs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListGraphs reports an array in {listed}"))
            .iter()
            .find(|graph| graph["id"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for {id} in {listed}"))
            .clone();
        assert_compute(case, "ListGraphs", &summary, &expected);
    }
}

/// One row of the update and restore tables: the case, the graph's name, the compute
/// the graph is CREATED with, the compute members the second call names, and the
/// configuration and deprecated memory the graph must report afterwards.
type ComputeChange = (
    &'static str,
    &'static str,
    Value,
    Value,
    Value,
    Option<i64>,
);

#[tokio::test]
async fn an_update_replaces_the_compute_configuration_in_either_direction() {
    let router = emulator();

    let rows: Vec<ComputeChange> = vec![
        (
            "a provisioned graph scaled down to a graph session",
            "scaled-down",
            deprecated_memory(MIN_PROVISIONED_MEMORY),
            graph_session(Some(MIN_SESSION_MEMORY)),
            json!({"graphSession": {"minMemory": MIN_SESSION_MEMORY}}),
            Some(MIN_SESSION_MEMORY),
        ),
        (
            "a graph session scaled up to a provisioned graph",
            "scaled-up",
            graph_session(Some(MAX_SESSION_MEMORY)),
            provisioned_graph(MAX_PROVISIONED_MEMORY),
            json!({"provisionedGraph": {"memory": MAX_PROVISIONED_MEMORY}}),
            Some(MAX_PROVISIONED_MEMORY),
        ),
        (
            "a graph session scaled up through the deprecated member",
            "scaled-up-deprecated",
            graph_session(None),
            deprecated_memory(MIN_PROVISIONED_MEMORY),
            json!({"provisionedGraph": {"memory": MIN_PROVISIONED_MEMORY}}),
            Some(MIN_PROVISIONED_MEMORY),
        ),
        (
            "a provisioned graph resized through the current member",
            "resized",
            deprecated_memory(MIN_PROVISIONED_MEMORY),
            provisioned_graph(512),
            json!({"provisionedGraph": {"memory": 512}}),
            Some(512),
        ),
        (
            "an update naming neither member keeps the stored compute",
            "untouched-compute",
            graph_session(Some(MIN_SESSION_MEMORY)),
            json!({"deletionProtection": true}),
            json!({"graphSession": {"minMemory": MIN_SESSION_MEMORY}}),
            Some(MIN_SESSION_MEMORY),
        ),
    ];

    for (case, name, created_with, update, configuration, reported_memory) in rows {
        let (id, _) = graph_in_service(&router, name, &created_with).await;
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/graphs/{id}"),
            Some(update.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateGraph, got {status}: {updated}"
        );
        let expected = ComputeCase {
            case,
            name,
            request: update,
            configuration,
            reported_memory,
        };
        assert_compute(case, "UpdateGraph", &updated, &expected);

        // ROUND TRIP: the replacement is persisted, and survives the UPDATING span the
        // clock settles on its own.
        let settled = wait_until_available(&router, &format!("/graphs/{id}")).await;
        assert_compute(case, "GetGraph", &settled, &expected);
    }
}

#[tokio::test]
async fn a_restore_takes_the_snapshots_compute_unless_the_request_names_one() {
    let router = emulator();

    // The SOURCE graph's compute, then the compute members the restore names.
    let rows: Vec<ComputeChange> = vec![
        (
            "a provisioned graph's snapshot restored with no compute named",
            "source-provisioned",
            provisioned_graph(128),
            json!({}),
            json!({"provisionedGraph": {"memory": 128}}),
            Some(128),
        ),
        (
            "a graph session's snapshot restored with no compute named",
            "source-session",
            graph_session(Some(MAX_SESSION_MEMORY)),
            json!({}),
            json!({"graphSession": {"minMemory": MAX_SESSION_MEMORY}}),
            Some(MAX_SESSION_MEMORY),
        ),
        (
            "a provisioned graph's snapshot restored into a graph session",
            "provisioned-into-session",
            deprecated_memory(MIN_PROVISIONED_MEMORY),
            graph_session(Some(MIN_SESSION_MEMORY)),
            json!({"graphSession": {"minMemory": MIN_SESSION_MEMORY}}),
            Some(MIN_SESSION_MEMORY),
        ),
        (
            "a graph session's snapshot restored into a provisioned graph",
            "session-into-provisioned",
            graph_session(Some(MIN_SESSION_MEMORY)),
            deprecated_memory(MAX_PROVISIONED_MEMORY),
            json!({"provisionedGraph": {"memory": MAX_PROVISIONED_MEMORY}}),
            Some(MAX_PROVISIONED_MEMORY),
        ),
    ];

    for (case, name, created_with, restore_compute, configuration, reported_memory) in rows {
        let (graph_id, _) = graph_in_service(&router, name, &created_with).await;
        let (status, snapshot) = call(
            &router,
            "POST",
            "/snapshots",
            Some(json!({"graphIdentifier": graph_id, "snapshotName": format!("{name}-snap")})),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {snapshot}"
        );
        let snapshot_id = snapshot["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {snapshot}"))
            .to_string();
        wait_until_available(&router, &format!("/snapshots/{snapshot_id}")).await;

        let mut body = restore_compute.clone();
        body["graphName"] = json!(format!("{name}-restored"));
        let (status, restored) = call(
            &router,
            "POST",
            &format!("/snapshots/{snapshot_id}/restore"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from RestoreGraphFromSnapshot, got {status}: \
             {restored}"
        );
        let expected = ComputeCase {
            case,
            name,
            request: restore_compute,
            configuration,
            reported_memory,
        };
        assert_compute(case, "RestoreGraphFromSnapshot", &restored, &expected);

        // ROUND TRIP: the restored graph's compute is persisted.
        let restored_id = restored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {restored}"))
            .to_string();
        let settled = wait_until_available(&router, &format!("/graphs/{restored_id}")).await;
        assert_compute(case, "GetGraph", &settled, &expected);
    }
}

/// One refusal to check: the case, the request, and the clause its message must carry.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    clause: String,
}

#[tokio::test]
async fn a_compute_configuration_outside_the_rules_is_refused() {
    let router = emulator();

    // A graph and a snapshot of it, so the refusals of `UpdateGraph` and
    // `RestoreGraphFromSnapshot` are refusals of a request that would otherwise be
    // served.
    let (graph_id, _) = graph_in_service(
        &router,
        "refusal-fixture",
        &deprecated_memory(MIN_PROVISIONED_MEMORY),
    )
    .await;
    let (status, snapshot) = call(
        &router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": "refusal-snap"})),
    )
    .await;
    assert_eq!(
        status, 201,
        "the fixture snapshot must be created, got {status}: {snapshot}"
    );
    let snapshot_id = snapshot["id"].as_str().unwrap_or_default().to_string();
    wait_until_available(&router, &format!("/snapshots/{snapshot_id}")).await;
    wait_until_available(&router, &format!("/graphs/{graph_id}")).await;

    let below_floor = MIN_PROVISIONED_MEMORY - 1;
    let exclusive = format!(
        "Value '{MIN_PROVISIONED_MEMORY}' at 'provisionedMemory' failed to satisfy \
         constraint: Member must not be provided together with computeConfiguration"
    );
    let deprecated_floor = format!(
        "Value '{below_floor}' at 'provisionedMemory' failed to satisfy constraint: Member \
         must have value greater than or equal to {MIN_PROVISIONED_MEMORY}"
    );
    let current_floor = format!(
        "Value '{below_floor}' at 'computeConfiguration.provisionedGraph.memory' failed to \
         satisfy constraint: Member must have value greater than or equal to \
         {MIN_PROVISIONED_MEMORY}"
    );

    let rows: Vec<RefusalCase> = vec![
        // Neither member: a create has nothing to size the graph from.
        RefusalCase {
            case: "CreateGraph naming neither compute member",
            method: "POST",
            path: "/graphs".to_string(),
            body: json!({"graphName": "no-compute"}),
            clause: "Value null at 'computeConfiguration' failed to satisfy constraint: \
                     Member must be provided when provisionedMemory is not provided"
                .to_string(),
        },
        // BOTH members, on each of the three operations that accept the pair.
        RefusalCase {
            case: "CreateGraph naming both compute members",
            method: "POST",
            path: "/graphs".to_string(),
            body: json!({
                "graphName": "both-members",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "computeConfiguration": {"graphSession": {"minMemory": MIN_SESSION_MEMORY}},
            }),
            clause: exclusive.clone(),
        },
        RefusalCase {
            case: "UpdateGraph naming both compute members",
            method: "PATCH",
            path: format!("/graphs/{graph_id}"),
            body: json!({
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "computeConfiguration": {"provisionedGraph": {"memory": 128}},
            }),
            clause: exclusive.clone(),
        },
        RefusalCase {
            case: "RestoreGraphFromSnapshot naming both compute members",
            method: "POST",
            path: format!("/snapshots/{snapshot_id}/restore"),
            body: json!({
                "graphName": "both-members-restored",
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "computeConfiguration": {"graphSession": {"minMemory": MIN_SESSION_MEMORY}},
            }),
            clause: exclusive.clone(),
        },
        // The documented floor, through EACH spelling, on each of the three.
        RefusalCase {
            case: "CreateGraph below the documented floor, deprecated spelling",
            method: "POST",
            path: "/graphs".to_string(),
            body: json!({"graphName": "under-floor", "provisionedMemory": below_floor}),
            clause: deprecated_floor.clone(),
        },
        RefusalCase {
            case: "CreateGraph below the documented floor, current spelling",
            method: "POST",
            path: "/graphs".to_string(),
            body: json!({
                "graphName": "under-floor-current",
                "computeConfiguration": {"provisionedGraph": {"memory": below_floor}},
            }),
            clause: current_floor.clone(),
        },
        RefusalCase {
            case: "UpdateGraph below the documented floor, current spelling",
            method: "PATCH",
            path: format!("/graphs/{graph_id}"),
            body: json!({
                "computeConfiguration": {"provisionedGraph": {"memory": below_floor}},
            }),
            clause: current_floor.clone(),
        },
        RefusalCase {
            case: "RestoreGraphFromSnapshot below the documented floor, deprecated spelling",
            method: "POST",
            path: format!("/snapshots/{snapshot_id}/restore"),
            body: json!({
                "graphName": "under-floor-restored",
                "provisionedMemory": below_floor,
            }),
            clause: deprecated_floor.clone(),
        },
        // One value PAST the modelled maximum, which the model-driven walk refuses in
        // the same envelope through either spelling.
        RefusalCase {
            case: "CreateGraph past the modelled maximum, deprecated spelling",
            method: "POST",
            path: "/graphs".to_string(),
            body: json!({
                "graphName": "over-maximum",
                "provisionedMemory": MAX_PROVISIONED_MEMORY + 1,
            }),
            clause: format!(
                "Value '{}' at 'provisionedMemory' failed to satisfy constraint: Member \
                 must have value less than or equal to {MAX_PROVISIONED_MEMORY}",
                MAX_PROVISIONED_MEMORY + 1
            ),
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, answer) = call(&router, row.method, &row.path, Some(row.body)).await;
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
        assert!(
            text.contains(&row.clause),
            "case {case:?}: expected the message to carry {:?}, got {text:?}",
            row.clause
        );
    }

    // A refused request persisted nothing: the fixture graph still reports the compute
    // it was created with, and no refusal took a name.
    let (status, fetched) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
    assert_eq!(
        status, 200,
        "case the fixture after the refusals: expected 200, got {status}: {fetched}"
    );
    assert_eq!(
        fetched["computeConfiguration"],
        json!({"provisionedGraph": {"memory": MIN_PROVISIONED_MEMORY}}),
        "case the fixture after the refusals: a refused update must change nothing, got {fetched}"
    );
    let (status, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        status, 200,
        "case the graphs after the refusals: expected 200, got {status}: {listed}"
    );
    let names: Vec<String> = listed["graphs"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|graph| graph["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["refusal-fixture".to_string()],
        "case the graphs after the refusals: a refused create must mint no graph, got {names:?}"
    );
}
