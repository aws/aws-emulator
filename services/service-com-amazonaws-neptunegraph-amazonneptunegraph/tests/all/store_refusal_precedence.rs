// SPDX-License-Identifier: CC0-1.0

//! The ORDER in which a minting operation of this service decides its refusals, driven
//! over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: an operation that mints a record decides the
//! refusals that depend only on what the store ALREADY HOLDS — an identity already taken
//! and a store with no room left — before any precondition on the status of the record
//! it reads from, so a request that breaks both at once is answered by the store-level
//! refusal. Nothing the source record goes on to do changes either store-level fact — no
//! status it reaches makes room in a full store or frees a taken name — while the two
//! refusals ask the caller for different things, so which one is reported is part of the
//! answer and not an accident of the order the checks happen to be written in.
//!
//! The rows span what the rule generalises over: every operation of this service that
//! mints a record into a bounded store and also gates on a source record's status
//! (`StartExportTask`, `StartImportTask`, `CreateGraphSnapshot`,
//! `CreatePrivateGraphEndpoint` and `RestoreGraphFromSnapshot` —
//! `smithy_query(query="digest")` lists the five), both kinds of store-level refusal (a
//! reached ceiling and a taken identity), both error classes a reached ceiling is
//! reported in (the 409 of an operation that models no quota error and the 402 of one
//! that does), and both kinds of source record (a graph, read in several statuses, and a
//! snapshot).
//!
//! Every row also asserts the CONTROL: the identical request, with the source record in
//! the identical state but room in the store (or a free identity), is refused by the
//! status gate. So each row really does break both conditions at once, and what it
//! measures is precedence rather than one refusal having swallowed the other.
//!
//! The filler records are written through the store's own writers, which is where an
//! accepted call writes them: driving any of these stores to its ceiling over the wire is
//! not possible against one graph, since each accepted call takes a resource through a
//! transient status, and every call whose ANSWER is asserted goes over the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, ExportTask, ExportTaskStatus, Graph, GraphCompute, GraphConfiguration,
    GraphData, GraphStatus, ImportLoad, ImportTask, ImportTaskStatus, PrivateGraphEndpoint,
    PrivateGraphEndpointStatus, Snapshot, SnapshotStatus, ARN_NAMESPACE, DEFAULT_REPLICA_COUNT,
    GRAPH_SOFTWARE_BUILD_NUMBER, MAX_EXPORT_TASKS, MAX_GRAPHS, MAX_IMPORT_TASKS,
    MAX_PRIVATE_GRAPH_ENDPOINTS, MAX_SNAPSHOTS, SOURCE_UNREADABLE_REASON,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::graph_state_gate_messages::graph_in_status;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the service documents ("Min = 16"), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i32 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneTaskRole";

/// A subnet id matching the modelled `SubnetId` pattern `^subnet-[a-z0-9]+$`.
const SUBNET_ID: &str = "subnet-0a1b2c3d4e5f67890";

/// An operation of this service that mints a record into a bounded store AND gates on
/// the status of a record it reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Minting {
    StartExportTask,
    StartImportTask,
    CreateGraphSnapshot,
    CreatePrivateGraphEndpoint,
    RestoreGraphFromSnapshot,
}

impl Minting {
    /// The operation's name, named in every assertion about it.
    fn name(self) -> &'static str {
        match self {
            Self::StartExportTask => "StartExportTask",
            Self::StartImportTask => "StartImportTask",
            Self::CreateGraphSnapshot => "CreateGraphSnapshot",
            Self::CreatePrivateGraphEndpoint => "CreatePrivateGraphEndpoint",
            Self::RestoreGraphFromSnapshot => "RestoreGraphFromSnapshot",
        }
    }

    /// The modelled `POST` URI this operation is bound to, with its labels filled in.
    ///
    /// Source: the operations' HTTP bindings (`smithy_query(query="operation",
    /// operation="StartExportTask,StartImportTask,CreateGraphSnapshot,\
    /// CreatePrivateGraphEndpoint,RestoreGraphFromSnapshot")`).
    fn path(self, graph_id: &str, snapshot_id: &str) -> String {
        match self {
            Self::StartExportTask => "/exporttasks".to_string(),
            Self::StartImportTask => format!("/graphs/{graph_id}/importtasks"),
            Self::CreateGraphSnapshot => "/snapshots".to_string(),
            Self::CreatePrivateGraphEndpoint => format!("/graphs/{graph_id}/endpoints/"),
            Self::RestoreGraphFromSnapshot => format!("/snapshots/{snapshot_id}/restore"),
        }
    }

    /// A well-formed request for this operation, where `identity` is the value the store
    /// keys the minted record's identity on (a graph name, a snapshot name, a VPC id).
    /// The task operations mint an identity of their own, so they ignore it.
    fn request(self, graph_id: &str, identity: &str) -> Value {
        match self {
            Self::StartExportTask => json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": "s3://export-bucket/precedence/",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
            Self::StartImportTask => json!({
                "source": "s3://import-bucket/precedence/",
                "roleArn": ROLE_ARN,
                "format": "CSV",
            }),
            Self::CreateGraphSnapshot => json!({
                "graphIdentifier": graph_id,
                "snapshotName": identity,
            }),
            Self::CreatePrivateGraphEndpoint => json!({
                "vpcId": identity,
                "subnetIds": [SUBNET_ID],
            }),
            Self::RestoreGraphFromSnapshot => json!({"graphName": identity}),
        }
    }

    /// A free, well-formed identity for the record this operation mints: the modelled
    /// `VpcId` pattern is `^vpc-[a-z0-9]+$`, while `GraphName`
    /// (`^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`) and `SnapshotName`
    /// (`^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`) take a hyphenated word
    /// (`smithy_query(query="shape",shape="VpcId,GraphName,SnapshotName")`), so the two
    /// namespaces take different spellings of one free identity.
    fn free_identity(self) -> &'static str {
        match self {
            Self::CreatePrivateGraphEndpoint => "vpc-precedenceminted",
            _ => "precedence-minted",
        }
    }

    /// What the refusal of this operation says it could not do — the action half of the
    /// service's one precondition grammar.
    fn action(self) -> &'static str {
        match self {
            Self::StartExportTask => "start the export task",
            Self::StartImportTask => "start the import task",
            Self::CreateGraphSnapshot => "create the graph snapshot",
            Self::CreatePrivateGraphEndpoint => "create the private graph endpoint",
            Self::RestoreGraphFromSnapshot => "restore the graph snapshot",
        }
    }

    /// The HTTP status and modelled error a full store is reported with: the operations
    /// that model `ServiceQuotaExceededException` report it there, and the ones that
    /// model no quota error report it as the conflict it is
    /// (`smithy_query(query="operation",operation="StartExportTask,StartImportTask,\
    /// CreateGraphSnapshot,CreatePrivateGraphEndpoint,RestoreGraphFromSnapshot")` ->
    /// errors).
    fn ceiling_refusal(self) -> (u16, &'static str) {
        match self {
            Self::StartExportTask | Self::StartImportTask => (409, "ConflictException"),
            Self::CreateGraphSnapshot
            | Self::CreatePrivateGraphEndpoint
            | Self::RestoreGraphFromSnapshot => (402, "ServiceQuotaExceededException"),
        }
    }
}

/// A store of this service that a minting call grows by one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    Graphs,
    Snapshots,
    ImportTasks,
    ExportTasks,
    PrivateEndpoints,
}

impl Store {
    /// The ceiling this store keeps.
    fn ceiling(self) -> usize {
        match self {
            Self::Graphs => MAX_GRAPHS,
            Self::Snapshots => MAX_SNAPSHOTS,
            Self::ImportTasks => MAX_IMPORT_TASKS,
            Self::ExportTasks => MAX_EXPORT_TASKS,
            Self::PrivateEndpoints => MAX_PRIVATE_GRAPH_ENDPOINTS,
        }
    }

    /// The plural the refusal names this store's records with.
    fn records(self) -> &'static str {
        match self {
            Self::Graphs => "graphs",
            Self::Snapshots => "graph snapshots",
            Self::ImportTasks => "import tasks",
            Self::ExportTasks => "export tasks",
            Self::PrivateEndpoints => "private graph endpoints",
        }
    }

    /// The ONE sentence this service refuses a full store with.
    fn refusal(self) -> String {
        format!(
            "this emulator holds at most {} {}",
            self.ceiling(),
            self.records()
        )
    }
}

/// The state the record a row's request READS is left in — the state that operation's own
/// precondition gate refuses.
#[derive(Clone, Copy, Debug)]
enum Source {
    /// The graph the request names, driven into this status over the wire by the
    /// operation that owns that transition, with the transition still in flight.
    GraphIn(&'static str),
    /// The graph the request names, PARKED in this status in the store with nothing owed
    /// out of it — so the row keeps measuring precedence however long it takes, where a
    /// status the clock settles on its own would stop being the state under test.
    ParkedGraph(GraphStatus, &'static str),
    /// The snapshot the request restores from, parked with its creation outstanding and
    /// nothing owed out of it.
    ParkedSnapshot,
}

/// One row of the precedence table.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// What this row is about, named in every assertion of it.
    case: &'static str,
    op: Minting,
    source: Source,
    /// The store this row fills to its ceiling.
    store: Store,
}

/// An emulator holding only this service, with the engine it was built on, so a test can
/// fill a store the way an accepted call fills it and count what it holds.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
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

/// The whole refusal the source record's status gate answers — its HTTP status, its
/// modelled error and its message — written out here rather than reached from the
/// service's own builders so the test states the expectation independently of the code
/// under test.
///
/// Every operation here but one answers the service's precondition conflict: what the
/// call could not do, and the condition that held it back. `CreateGraphSnapshot`'s is
/// the refusal AWS words itself, a 400 naming the graph the snapshot would have been
/// taken from.
fn gate_refusal(op: Minting, source: Source, graph_id: &str) -> (u16, &'static str, String) {
    if op == Minting::CreateGraphSnapshot {
        return (
            400,
            "ValidationException",
            format!(
                "Graph [{graph_id}] is not available for snapshot. You can only create a \
                 snapshot for an AVAILABLE Graph."
            ),
        );
    }
    let message = match source {
        // A call on another resource whose precondition is the graph's status names the
        // graph as the subject of the condition.
        Source::GraphIn(status) => format!(
            "Unable to {} because the graph is in the {status} state.",
            op.action()
        ),
        Source::ParkedGraph(_, status) => format!(
            "Unable to {} because the graph is in the {status} state.",
            op.action()
        ),
        // A call on the snapshot itself says "it": the action already named that record.
        Source::ParkedSnapshot => {
            format!("Unable to {} because it is in the CREATING state.", op.action())
        }
    };
    (409, "ConflictException", message)
}

/// How many records each of the five stores holds.
fn counts(engine: &Arc<engine::InMemoryEngine>) -> [usize; 5] {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let store = state.lock();
    [
        store.graphs.len(),
        store.snapshots.len(),
        store.import_tasks.len(),
        store.export_tasks.len(),
        store.private_endpoint_count(),
    ]
}

/// How many records the store `which` holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>, which: Store) -> usize {
    let held = counts(engine);
    match which {
        Store::Graphs => held[0],
        Store::Snapshots => held[1],
        Store::ImportTasks => held[2],
        Store::ExportTasks => held[3],
        Store::PrivateEndpoints => held[4],
    }
}

/// Fill the store `which` to `upto` records, through the store's own writers — where an
/// accepted call writes them.
fn fill_to(engine: &Arc<engine::InMemoryEngine>, which: Store, graph_id: &str, upto: usize) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut held = state.lock();
    let now = Instant::now();
    let mut filler = 0usize;
    while match which {
        Store::Graphs => held.graphs.len(),
        Store::Snapshots => held.snapshots.len(),
        Store::ImportTasks => held.import_tasks.len(),
        Store::ExportTasks => held.export_tasks.len(),
        Store::PrivateEndpoints => held.private_endpoint_count(),
    } < upto
    {
        filler += 1;
        match which {
            Store::Graphs => {
                let id = held.next_graph_id();
                held.insert_graph(Graph {
                    arn: engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("graph/{id}")),
                    endpoint: format!("{id}.{REGION}.{ARN_NAMESPACE}.amazonaws.com"),
                    region: REGION.to_string(),
                    id,
                    name: format!("filler-graph-{filler}"),
                    status: GraphStatus::Available,
                    pending: None,
                    status_reason: None,
                    create_time: SystemTime::now(),
                    compute: GraphCompute::provisioned(MIN_PROVISIONED_MEMORY),
                    public_connectivity: false,
                    deletion_protection: false,
                    replica_count: DEFAULT_REPLICA_COUNT,
                    kms_key_identifier: None,
                    vector_search_configuration: None,
                    source_snapshot_id: None,
                    build_number: GRAPH_SOFTWARE_BUILD_NUMBER.to_string(),
                    tags: BTreeMap::new(),
                    deletes_at: None,
                    data: GraphData::empty(SystemTime::now()),
                });
            }
            Store::Snapshots => {
                let id = held.next_snapshot_id();
                let record = snapshot_record(
                    id,
                    graph_id,
                    format!("filler-snapshot-{filler}"),
                    SnapshotStatus::Available,
                );
                held.insert_snapshot(record);
            }
            Store::ImportTasks => {
                let id = held.next_import_task_id();
                held.insert_import_task(ImportTask {
                    id,
                    region: REGION.to_string(),
                    graph_id: graph_id.to_string(),
                    source: "s3://import-bucket/filler/".to_string(),
                    format: None,
                    parquet_type: None,
                    blank_node_handling: None,
                    fail_on_error: true,
                    role_arn: ROLE_ARN.to_string(),
                    import_options: None,
                    status: ImportTaskStatus::Initializing,
                    status_reason: None,
                    attempt_number: 1,
                    // A filler record: nothing read a source for it, so it loads
                    // nothing and stops where it is.
                    load: ImportLoad::unread(SOURCE_UNREADABLE_REASON),
                    delivered: false,
                    start_time: SystemTime::now(),
                    started_at: Instant::now(),
                    stopped_progress: None,
                    settles_at: None,
                });
            }
            Store::ExportTasks => {
                let id = held.next_export_task_id();
                held.insert_export_task(ExportTask {
                    arn: engine::emulator_arn(
                        ARN_NAMESPACE,
                        REGION,
                        &format!("export-task/{id}"),
                    ),
                    region: REGION.to_string(),
                    id,
                    source_graph_id: graph_id.to_string(),
                    role_arn: ROLE_ARN.to_string(),
                    format: "CSV".to_string(),
                    parquet_type: None,
                    destination: "s3://export-bucket/filler/".to_string(),
                    kms_key_identifier: KMS_KEY_ARN.to_string(),
                    export_filter: None,
                    status: ExportTaskStatus::Initializing,
                    status_reason: None,
                    exported_vertices: 0,
                    exported_edges: 0,
                    cancelled_progress: None,
                    tags: BTreeMap::new(),
                    start_time: SystemTime::now(),
                    started_at: now,
                    settles_at: None,
                });
            }
            Store::PrivateEndpoints => {
                let vpc_endpoint_id = held.next_vpc_endpoint_id();
                held.insert_private_endpoint(PrivateGraphEndpoint {
                    graph_id: graph_id.to_string(),
                    // The identity of an endpoint is `(graphIdentifier, vpcId)`, so one
                    // graph takes as many endpoints as there are VPCs.
                    vpc_id: format!("vpc-filler{filler}"),
                    subnet_ids: vec![SUBNET_ID.to_string()],
                    security_group_ids: Vec::new(),
                    vpc_endpoint_id,
                    status: PrivateGraphEndpointStatus::Available,
                    due: None,
                });
            }
        }
    }
}

/// One snapshot record of `graph_id`, as the store holds it — the literal both the
/// fillers and the parked source snapshot are built from, so the two cannot drift.
fn snapshot_record(
    id: String,
    graph_id: &str,
    name: String,
    status: SnapshotStatus,
) -> Snapshot {
    Snapshot {
        arn: engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("graph-snapshot/{id}")),
        region: REGION.to_string(),
        id,
        name,
        source_graph_id: graph_id.to_string(),
        status,
        snapshot_create_time: SystemTime::now(),
        kms_key_identifier: None,
        tags: BTreeMap::new(),
        // Nothing is owed out of this record: a parked status stays put, and a filler
        // that is already AVAILABLE has nowhere to settle to.
        settles_at: None,
        source_configuration: GraphConfiguration {
            compute: GraphCompute::provisioned(MIN_PROVISIONED_MEMORY),
            public_connectivity: false,
            replica_count: DEFAULT_REPLICA_COUNT,
            deletion_protection: false,
            vector_search_configuration: None,
        },
        data: GraphData::empty(SystemTime::now()),
    }
}

/// Park the stored graph in `status` with nothing owed out of it, and prove over the
/// wire that `GetGraph` reports it — so the state a row is about is the state the service
/// reads, and the clock cannot move it between the row's two calls.
async fn park_graph(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    graph_id: &str,
    status: (GraphStatus, &str),
) {
    {
        let state = service_state(&**engine).expect("the neptune-graph store is mounted");
        let mut held = state.lock();
        let graph = held
            .graphs
            .get_mut(graph_id)
            .unwrap_or_else(|| panic!("case {case:?}: the store holds no graph {graph_id}"));
        graph.status = status.0;
        graph.pending = None;
        graph.deletes_at = None;
    }
    let (code, graph) = call(router, "GET", &format!("/graphs/{graph_id}"), None).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected 200 from GetGraph, got {code}: {graph}"
    );
    assert_eq!(
        graph["status"],
        json!(status.1),
        "case {case:?}: expected the parked graph to report {}, got {graph}",
        status.1
    );
}

/// Park a snapshot of `graph_id` with its creation OUTSTANDING and nothing owed out of
/// it, and prove over the wire that `GetGraphSnapshot` reports it — a source record a
/// restore refuses for as long as the row needs it to.
async fn parked_snapshot(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    graph_id: &str,
    name: &str,
) -> String {
    let snapshot_id = {
        let state = service_state(&**engine).expect("the neptune-graph store is mounted");
        let mut held = state.lock();
        let id = held.next_snapshot_id();
        let record = snapshot_record(
            id.clone(),
            graph_id,
            name.to_string(),
            SnapshotStatus::Creating,
        );
        held.insert_snapshot(record);
        id
    };
    let (code, snapshot) = call(router, "GET", &format!("/snapshots/{snapshot_id}"), None).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected 200 from GetGraphSnapshot, got {code}: {snapshot}"
    );
    assert_eq!(
        snapshot["status"],
        json!("CREATING"),
        "case {case:?}: expected the parked snapshot to report CREATING, got {snapshot}"
    );
    snapshot_id
}

/// Put the record a row's request reads into the state its gate refuses, and answer the
/// graph id and snapshot id the request is built from.
async fn source_record(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    source: Source,
) -> (String, String) {
    match source {
        Source::GraphIn(status) => (
            graph_in_status(router, case, status, "precedence-source").await,
            String::new(),
        ),
        Source::ParkedGraph(status, wire) => {
            let graph_id = graph_in_status(router, case, "AVAILABLE", "precedence-source").await;
            park_graph(router, engine, case, &graph_id, (status, wire)).await;
            (graph_id, String::new())
        }
        Source::ParkedSnapshot => {
            let graph_id = graph_in_status(router, case, "AVAILABLE", "precedence-source").await;
            let snapshot_id =
                parked_snapshot(router, engine, case, &graph_id, "precedence-snapshot").await;
            (graph_id, snapshot_id)
        }
    }
}

/// Assert the CONTROL answer: the source record's status gate, which is the refusal this
/// request meets while the store still has room.
fn assert_gate(case: &str, operation: &str, code: u16, body: &Value, expected: (u16, &str, &str)) {
    let (status, error, refusal) = expected;
    assert_eq!(
        code, status,
        "case {case:?}: expected {status} from {operation} against the source record's \
         state, got {code}: {body}"
    );
    assert_eq!(
        error_code(body),
        error,
        "case {case:?}: expected a {error} from {operation}, got {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, refusal,
        "case {case:?}: expected {operation} to answer {refusal:?}, got {actual:?}"
    );
}

/// Assert a store-level refusal: the status, the modelled error and the whole message.
fn assert_store_refusal(
    case: &str,
    operation: &str,
    code: u16,
    body: &Value,
    expected: (u16, &str, &str),
) {
    let (status, error, refusal) = expected;
    assert_eq!(
        code, status,
        "case {case:?}: expected {status} from {operation}, got {code}: {body}"
    );
    assert_eq!(
        error_code(body),
        error,
        "case {case:?}: expected a {error} from {operation}, got {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, refusal,
        "case {case:?}: expected {operation} to answer {refusal:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_full_store_outranks_the_state_of_the_record_a_minting_call_reads() {
    // Every operation that mints into a bounded store and also gates on a source
    // record's status, crossed with the states that gate refuses: a graph at rest but not
    // serving, a graph whose provisioning is still outstanding, a graph whose update is
    // in flight, and a snapshot whose creation is outstanding. Each operation has at
    // least one row whose source state the clock cannot move — a stopped graph, or a
    // record parked with nothing owed out of it — so no row can stop measuring the order
    // of the two checks.
    let rows = [
        Row {
            case: "a full export-task store and a stopped graph",
            op: Minting::StartExportTask,
            source: Source::GraphIn("STOPPED"),
            store: Store::ExportTasks,
        },
        Row {
            case: "a full export-task store and a graph still being created",
            op: Minting::StartExportTask,
            source: Source::GraphIn("CREATING"),
            store: Store::ExportTasks,
        },
        Row {
            case: "a full import-task store and a stopped graph",
            op: Minting::StartImportTask,
            source: Source::GraphIn("STOPPED"),
            store: Store::ImportTasks,
        },
        Row {
            case: "a full snapshot store and a graph being updated",
            op: Minting::CreateGraphSnapshot,
            source: Source::ParkedGraph(GraphStatus::Updating, "UPDATING"),
            store: Store::Snapshots,
        },
        Row {
            case: "a full endpoint store and a stopped graph",
            op: Minting::CreatePrivateGraphEndpoint,
            source: Source::GraphIn("STOPPED"),
            store: Store::PrivateEndpoints,
        },
        Row {
            case: "a full graph store and a snapshot still being created",
            op: Minting::RestoreGraphFromSnapshot,
            source: Source::ParkedSnapshot,
            store: Store::Graphs,
        },
    ];

    for row in rows {
        let Row {
            case,
            op,
            source,
            store,
        } = row;
        let operation = op.name();
        // One emulator per row, so the only facts a row's answer can rest on are the
        // ones it set up.
        let (router, engine) = emulator();
        let (graph_id, snapshot_id) = source_record(&router, &engine, case, source).await;
        let path = op.path(&graph_id, &snapshot_id);
        let request = op.request(&graph_id, op.free_identity());

        // CONTROL: the store still has room, so the one condition this request breaks is
        // the source record's state — and that gate is what answers it.
        let (gate_code, gated) = call(&router, "POST", &path, Some(request.clone())).await;
        let (gate_status, gate_error, gate_message) = gate_refusal(op, source, &graph_id);
        assert_gate(
            case,
            operation,
            gate_code,
            &gated,
            (gate_status, gate_error, &gate_message),
        );

        // Now the store this call grows is filled right up to its ceiling, so the
        // IDENTICAL request breaks both conditions at once.
        fill_to(&engine, store, &graph_id, store.ceiling());
        assert_eq!(
            stored_count(&engine, store),
            store.ceiling(),
            "case {case:?}: expected the store filled to its ceiling, got {}",
            stored_count(&engine, store)
        );
        let before = counts(&engine);
        let (code, refused) = call(&router, "POST", &path, Some(request)).await;
        let (status, error) = op.ceiling_refusal();
        assert_store_refusal(
            case,
            operation,
            code,
            &refused,
            (status, error, &store.refusal()),
        );
        assert_eq!(
            counts(&engine),
            before,
            "case {case:?}: expected {operation} to mint nothing, got {:?} where it held \
             {before:?}",
            counts(&engine)
        );
    }
}

#[tokio::test]
async fn a_taken_identity_outranks_the_state_of_the_record_a_minting_call_reads() {
    // The identity half of the same rule, for the three of those operations whose minted
    // record has a caller-chosen identity: a snapshot name, a graph name and the
    // (graph, VPC) pair an endpoint is addressed by. Each scenario reserves the identity
    // first, then leaves the source record in a state its own gate refuses, then sends
    // the SAME request twice — once with a free identity, once with the taken one.
    let scenarios: [(&str, Minting, Source, &str, &str, &str); 3] = [
        (
            "a taken snapshot name and a graph being updated",
            Minting::CreateGraphSnapshot,
            Source::ParkedGraph(GraphStatus::Updating, "UPDATING"),
            "precedence-taken",
            "precedence-free",
            // The one sentence this service refuses an already-held name with, stated
            // from the REQUESTED name alone.
            "Snapshot with name precedence-taken already exists!",
        ),
        (
            "a taken graph name and a snapshot still being created",
            Minting::RestoreGraphFromSnapshot,
            Source::ParkedSnapshot,
            "precedence-source",
            "precedence-free",
            "Graph with name precedence-source already exists!",
        ),
        (
            "an endpoint the graph and VPC already hold, and a stopped graph",
            Minting::CreatePrivateGraphEndpoint,
            Source::ParkedGraph(GraphStatus::Stopped, "STOPPED"),
            "vpc-precedencetaken",
            "vpc-precedencefree",
            "Unable to create the private graph endpoint because it already exists.",
        ),
    ];

    for (case, op, source, taken, free, expected) in scenarios {
        let operation = op.name();
        let (router, engine) = emulator();

        // The identity has to be reserved while the source record still accepts the
        // call, so each scenario mints it first and only then puts the source record
        // into the state its gate refuses.
        let (graph_id, snapshot_id) = match op {
            Minting::CreateGraphSnapshot => {
                let graph_id =
                    graph_in_status(&router, case, "AVAILABLE", "precedence-source").await;
                let (minted, snapshot) = call(
                    &router,
                    "POST",
                    &op.path(&graph_id, ""),
                    Some(op.request(&graph_id, taken)),
                )
                .await;
                assert_eq!(
                    minted, 201,
                    "case {case:?}: expected 201 reserving the identity, got {minted}: \
                     {snapshot}"
                );
                // The snapshot took the graph through its own transient status; the
                // parked update below is what this row's gate is about, and the clock
                // owes nothing out of it.
                park_graph(
                    &router,
                    &engine,
                    case,
                    &graph_id,
                    (GraphStatus::Updating, "UPDATING"),
                )
                .await;
                (graph_id, String::new())
            }
            Minting::RestoreGraphFromSnapshot => {
                // The taken graph name is the source graph's own, so what the restore
                // collides with is a stored graph.
                let (graph_id, snapshot_id) = source_record(&router, &engine, case, source).await;
                (graph_id, snapshot_id)
            }
            Minting::CreatePrivateGraphEndpoint => {
                let graph_id =
                    graph_in_status(&router, case, "AVAILABLE", "precedence-source").await;
                let (minted, endpoint) = call(
                    &router,
                    "POST",
                    &op.path(&graph_id, ""),
                    Some(op.request(&graph_id, taken)),
                )
                .await;
                assert_eq!(
                    minted, 201,
                    "case {case:?}: expected 201 reserving the identity, got {minted}: \
                     {endpoint}"
                );
                // The graph is parked stopped rather than stopped over the wire: the
                // row's gate is then the same state however long the two calls below
                // take.
                park_graph(
                    &router,
                    &engine,
                    case,
                    &graph_id,
                    (GraphStatus::Stopped, "STOPPED"),
                )
                .await;
                (graph_id, String::new())
            }
            other => panic!("case {case:?}: {} mints no caller-chosen identity", other.name()),
        };
        let path = op.path(&graph_id, &snapshot_id);

        // CONTROL: a FREE identity, so the one condition this request breaks is the
        // source record's state.
        let (gate_code, gated) = call(
            &router,
            "POST",
            &path,
            Some(op.request(&graph_id, free)),
        )
        .await;
        let (gate_status, gate_error, gate_message) = gate_refusal(op, source, &graph_id);
        assert_gate(
            case,
            operation,
            gate_code,
            &gated,
            (gate_status, gate_error, &gate_message),
        );

        // The same request with the TAKEN identity breaks both conditions at once, and
        // the store-level refusal is what reports it.
        let before = counts(&engine);
        let (code, refused) = call(
            &router,
            "POST",
            &path,
            Some(op.request(&graph_id, taken)),
        )
        .await;
        assert_store_refusal(case, operation, code, &refused, (409, "ConflictException", expected));
        assert_eq!(
            counts(&engine),
            before,
            "case {case:?}: expected {operation} to mint nothing, got {:?} where it held \
             {before:?}",
            counts(&engine)
        );
    }
}
