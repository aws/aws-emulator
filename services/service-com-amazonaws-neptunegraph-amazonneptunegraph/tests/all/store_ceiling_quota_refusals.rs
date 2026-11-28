// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how this service reports a FULL store through the modelled
//! `ServiceQuotaExceededException`, driven over the wire (`restJson1`) through the same
//! router the emulator serves from.
//!
//! The rule every assertion here is about: an operation that mints a record and models
//! `ServiceQuotaExceededException` (402) reports a store with no room left with that
//! error, worded by the ONE clause this service states a reached ceiling in — how many
//! records of that kind the emulator holds at most — naming the kind of record that did
//! not fit and the service whose ceiling was met, and minting nothing. The clause is a
//! fact about the STORE, so it does not depend on which operation met the ceiling or on
//! what its request looked like: two operations that meet the same full store answer the
//! same sentence.
//!
//! `CreateGraph`, `CreateGraphSnapshot`, `RestoreGraphFromSnapshot`,
//! `CreateGraphUsingImportTask` and `CreatePrivateGraphEndpoint` are the operations of
//! this service that model the error
//! (`smithy_query(query="operation",operation="CreateGraph,CreateGraphSnapshot,\
//! RestoreGraphFromSnapshot,CreateGraphUsingImportTask,CreatePrivateGraphEndpoint")`),
//! so the table drives every one of them, and the three that grow the GRAPH store are
//! asserted to answer one full graph store word for word. The 409 half of the same rule
//! — an operation that models no quota error at all — is covered by
//! `full_task_store_refusals`.
//!
//! So the rows differ along what the rule generalises over: which store is full, which
//! operation met it, and the members of the refused request (the optional configuration
//! members of a created graph, the restore's overrides, the import's format and options,
//! the tags, the subnet count and the security groups) — rather than re-running one
//! request, because a full store is a fact about the STORE and the refusal may not
//! depend on the shape of the request that met it. Each ceiling is also tested at its
//! boundary: one record below it the same growth request is accepted and round-trips
//! through the read of its own namespace, and at it the next one is refused.
//!
//! The filler records are written through the store's own writers, which is where an
//! accepted call writes them: driving the snapshot, import-task or endpoint store to its
//! ceiling over the wire is not possible against one graph (each accepted call takes a
//! resource through a transient status), so no store is filled that way, and every call
//! that must be ACCEPTED goes over the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, Graph, GraphCompute, GraphConfiguration, GraphData, GraphStatus, ImportLoad,
    ImportTask, ImportTaskStatus, PrivateGraphEndpoint, PrivateGraphEndpointStatus, Snapshot,
    SnapshotStatus, ARN_NAMESPACE, DEFAULT_REPLICA_COUNT, GRAPH_SOFTWARE_BUILD_NUMBER, MAX_GRAPHS,
    MAX_IMPORT_TASKS, MAX_PRIVATE_GRAPH_ENDPOINTS, MAX_SNAPSHOTS, PROVISIONING_SPAN,
    SOURCE_UNREADABLE_REASON,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

/// The same pattern's `role/service-role` form, so the table spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneTask";

/// Which store a row is about: the four this service refuses a growth call against
/// through the modelled `ServiceQuotaExceededException`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullStore {
    Graphs,
    Snapshots,
    ImportTasks,
    PrivateEndpoints,
}

impl FullStore {
    /// The ceiling this store keeps.
    fn ceiling(self) -> usize {
        match self {
            Self::Graphs => MAX_GRAPHS,
            Self::Snapshots => MAX_SNAPSHOTS,
            Self::ImportTasks => MAX_IMPORT_TASKS,
            Self::PrivateEndpoints => MAX_PRIVATE_GRAPH_ENDPOINTS,
        }
    }

    /// The plural the refusal names this store's records with.
    fn records(self) -> &'static str {
        match self {
            Self::Graphs => "graphs",
            Self::Snapshots => "graph snapshots",
            Self::ImportTasks => "import tasks",
            Self::PrivateEndpoints => "private graph endpoints",
        }
    }

    /// The kind of record that did not fit, which the modelled `resourceType` member
    /// asks for ("The type of the resource that exceeded quota. Ex: Graph, Snapshot" —
    /// `smithy_query(query="shape",shape="ServiceQuotaExceededException")`).
    fn resource_type(self) -> &'static str {
        match self {
            Self::Graphs => "Graph",
            Self::Snapshots => "GraphSnapshot",
            Self::ImportTasks => "ImportTask",
            Self::PrivateEndpoints => "PrivateGraphEndpoint",
        }
    }

    /// The ONE clause a reached ceiling of this store reads in: what the emulator holds
    /// at most, and of what.
    fn refusal(self) -> String {
        format!(
            "this emulator holds at most {} {}",
            self.ceiling(),
            self.records()
        )
    }
}

/// An operation that mints a record and models `ServiceQuotaExceededException`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    CreateGraph,
    RestoreGraphFromSnapshot,
    CreateGraphUsingImportTask,
    CreateGraphSnapshot,
    CreatePrivateGraphEndpoint,
}

impl Op {
    /// The operation's name, named in every assertion about it.
    fn name(self) -> &'static str {
        match self {
            Self::CreateGraph => "CreateGraph",
            Self::RestoreGraphFromSnapshot => "RestoreGraphFromSnapshot",
            Self::CreateGraphUsingImportTask => "CreateGraphUsingImportTask",
            Self::CreateGraphSnapshot => "CreateGraphSnapshot",
            Self::CreatePrivateGraphEndpoint => "CreatePrivateGraphEndpoint",
        }
    }

    /// The modelled `POST` URI this operation is bound to, with its labels filled in.
    fn path(self, graph_id: &str, snapshot_id: &str) -> String {
        match self {
            Self::CreateGraph => "/graphs".to_string(),
            Self::RestoreGraphFromSnapshot => format!("/snapshots/{snapshot_id}/restore"),
            Self::CreateGraphUsingImportTask => "/importtasks".to_string(),
            Self::CreateGraphSnapshot => "/snapshots".to_string(),
            Self::CreatePrivateGraphEndpoint => format!("/graphs/{graph_id}/endpoints/"),
        }
    }
}

/// One row of the full-store table: which operation meets the ceiling, and the
/// well-formed request it meets it with.
struct CeilingCase {
    /// What this row is about, named in every assertion of it.
    case: &'static str,
    op: Op,
    /// The request body, built from the id of the graph the call may name.
    body: fn(&str) -> Value,
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

/// `n` distinct subnet ids matching the modelled `SubnetId` pattern
/// `^subnet-[a-z0-9]+$`.
fn subnets(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("subnet-0a1b2c3d4e5f{index:05}"))
        .collect()
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path:?}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case {path:?}: never became AVAILABLE");
}

/// Create a graph over the wire and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

/// Take a snapshot of `graph_id` over the wire and wait it into service, returning its
/// id — what `RestoreGraphFromSnapshot` needs before it can reach any ceiling.
async fn snapshot_in_service(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    name: &str,
) -> String {
    let body = json!({"graphIdentifier": graph_id, "snapshotName": name});
    let (status, created) = call(router, "POST", "/snapshots", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no snapshot id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/snapshots/{id}")).await;
    // Taking the snapshot took the source graph through its own transient status, so the
    // graph is waited back into service too: what every row below meets is a FULL STORE
    // and not a graph that is busy.
    wait_until_available(router, &format!("/graphs/{graph_id}")).await;
    id
}

/// How many records each of the four stores holds.
fn counts(engine: &Arc<engine::InMemoryEngine>) -> [usize; 4] {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let store = state.lock();
    [
        store.graphs.len(),
        store.snapshots.len(),
        store.import_tasks.len(),
        store.private_endpoint_count(),
    ]
}

/// How many records the store of `full` holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>, full: FullStore) -> usize {
    let held = counts(engine);
    match full {
        FullStore::Graphs => held[0],
        FullStore::Snapshots => held[1],
        FullStore::ImportTasks => held[2],
        FullStore::PrivateEndpoints => held[3],
    }
}

/// Fill the store of `full` to `upto` records, through the store's own writers — where
/// an accepted call writes them.
fn fill_to(
    engine: &Arc<engine::InMemoryEngine>,
    full: FullStore,
    graph_id: &str,
    upto: usize,
) {
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut held = state.lock();
    let mut filler = 0usize;
    loop {
        let stored = match full {
            FullStore::Graphs => held.graphs.len(),
            FullStore::Snapshots => held.snapshots.len(),
            FullStore::ImportTasks => held.import_tasks.len(),
            FullStore::PrivateEndpoints => held.private_endpoint_count(),
        };
        if stored >= upto {
            return;
        }
        filler += 1;
        match full {
            FullStore::Graphs => {
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
            FullStore::Snapshots => {
                let id = held.next_snapshot_id();
                held.insert_snapshot(Snapshot {
                    arn: engine::emulator_arn(
                        ARN_NAMESPACE,
                        REGION,
                        &format!("graph-snapshot/{id}"),
                    ),
                    region: REGION.to_string(),
                    id,
                    name: format!("filler-snapshot-{filler}"),
                    source_graph_id: graph_id.to_string(),
                    status: SnapshotStatus::Available,
                    snapshot_create_time: SystemTime::now(),
                    kms_key_identifier: None,
                    tags: BTreeMap::new(),
                    settles_at: None,
                    source_configuration: GraphConfiguration {
                        compute: GraphCompute::provisioned(MIN_PROVISIONED_MEMORY),
                        public_connectivity: false,
                        replica_count: DEFAULT_REPLICA_COUNT,
                        deletion_protection: false,
                        vector_search_configuration: None,
                    },
                    data: GraphData::empty(SystemTime::now()),
                });
            }
            FullStore::ImportTasks => {
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
            FullStore::PrivateEndpoints => {
                let vpc_endpoint_id = held.next_vpc_endpoint_id();
                held.insert_private_endpoint(PrivateGraphEndpoint {
                    graph_id: graph_id.to_string(),
                    // The identity of an endpoint is `(graphIdentifier, vpcId)`, so one
                    // graph takes as many endpoints as there are VPCs.
                    vpc_id: format!("vpc-filler{filler}"),
                    subnet_ids: subnets(1),
                    security_group_ids: Vec::new(),
                    vpc_endpoint_id,
                    status: PrivateGraphEndpointStatus::Available,
                    due: None,
                });
            }
        }
    }
}

/// The rows that meet a full store of `full`: every operation that grows it, each with
/// requests that differ in the members they carry.
fn rows(full: FullStore) -> Vec<CeilingCase> {
    match full {
        FullStore::Graphs => vec![
            CeilingCase {
                case: "a full graph store, CreateGraph with the plainest request",
                op: Op::CreateGraph,
                body: |_| json!({"graphName": "one-too-many", "provisionedMemory": 16}),
            },
            CeilingCase {
                case: "a full graph store, CreateGraph with every optional member",
                op: Op::CreateGraph,
                body: |_| {
                    json!({
                        "graphName": "one-too-many-configured",
                        "provisionedMemory": 1024,
                        "replicaCount": 2,
                        "publicConnectivity": true,
                        "deletionProtection": true,
                        "kmsKeyIdentifier": KMS_KEY_ARN,
                        "vectorSearchConfiguration": {"dimension": 128},
                        "tags": {"team": "graph", "env": "test"},
                    })
                },
            },
            CeilingCase {
                case: "a full graph store, RestoreGraphFromSnapshot with the plainest \
                       request",
                op: Op::RestoreGraphFromSnapshot,
                body: |_| json!({"graphName": "restored-one-too-many"}),
            },
            CeilingCase {
                case: "a full graph store, RestoreGraphFromSnapshot overriding the \
                       captured configuration",
                op: Op::RestoreGraphFromSnapshot,
                body: |_| {
                    json!({
                        "graphName": "restored-one-too-many-configured",
                        "provisionedMemory": 2048,
                        "replicaCount": 0,
                        "deletionProtection": true,
                        "publicConnectivity": true,
                        "tags": {"team": "graph"},
                    })
                },
            },
            CeilingCase {
                case: "a full graph store, CreateGraphUsingImportTask with the plainest \
                       request",
                op: Op::CreateGraphUsingImportTask,
                body: |_| {
                    json!({
                        "graphName": "imported-one-too-many",
                        "source": "s3://import-bucket/plain/",
                        "roleArn": ROLE_ARN,
                        "format": "CSV",
                    })
                },
            },
            CeilingCase {
                case: "a full graph store, CreateGraphUsingImportTask with a format and \
                       import options",
                op: Op::CreateGraphUsingImportTask,
                body: |_| {
                    json!({
                        "graphName": "imported-one-too-many-configured",
                        "source": "s3://another-import-bucket/deep/prefix/",
                        "roleArn": SERVICE_ROLE_ARN,
                        "format": "PARQUET",
                        "parquetType": "COLUMNAR",
                        "failOnError": false,
                        "maxProvisionedMemory": 2048,
                        "minProvisionedMemory": 16,
                        "tags": {"env": "test"},
                    })
                },
            },
        ],
        FullStore::Snapshots => vec![
            CeilingCase {
                case: "a full snapshot store, CreateGraphSnapshot with the plainest \
                       request",
                op: Op::CreateGraphSnapshot,
                body: |graph_id| {
                    json!({"graphIdentifier": graph_id, "snapshotName": "one-too-many"})
                },
            },
            CeilingCase {
                case: "a full snapshot store, CreateGraphSnapshot with tags",
                op: Op::CreateGraphSnapshot,
                body: |graph_id| {
                    json!({
                        "graphIdentifier": graph_id,
                        "snapshotName": "one-too-many-tagged",
                        "tags": {"team": "graph", "env": "test"},
                    })
                },
            },
        ],
        FullStore::ImportTasks => vec![
            CeilingCase {
                case: "a full import-task store, CreateGraphUsingImportTask with the \
                       plainest request",
                op: Op::CreateGraphUsingImportTask,
                body: |_| {
                    json!({
                        "graphName": "import-task-one-too-many",
                        "source": "s3://import-bucket/plain/",
                        "roleArn": ROLE_ARN,
                        "format": "CSV",
                    })
                },
            },
            CeilingCase {
                case: "a full import-task store, CreateGraphUsingImportTask converting \
                       blank nodes",
                op: Op::CreateGraphUsingImportTask,
                body: |_| {
                    json!({
                        "graphName": "import-task-one-too-many-rdf",
                        "source": "s3://import-bucket/rdf/",
                        "roleArn": SERVICE_ROLE_ARN,
                        "format": "NTRIPLES",
                        "blankNodeHandling": "convertToIri",
                    })
                },
            },
        ],
        FullStore::PrivateEndpoints => vec![
            CeilingCase {
                case: "a full endpoint store, CreatePrivateGraphEndpoint with one subnet",
                op: Op::CreatePrivateGraphEndpoint,
                body: |_| json!({"vpcId": "vpc-onetoomany", "subnetIds": subnets(1)}),
            },
            CeilingCase {
                case: "a full endpoint store, CreatePrivateGraphEndpoint with subnets at \
                       the published bound and security groups",
                op: Op::CreatePrivateGraphEndpoint,
                body: |_| {
                    json!({
                        "vpcId": "vpc-0a1b2c3d4e5f67890",
                        "subnetIds": subnets(6),
                        "vpcSecurityGroupIds": ["sg-0a1b2c3d4e5f67890", "sg-1234567890abcdef0"],
                    })
                },
            },
        ],
    }
}

#[tokio::test]
async fn a_full_store_is_one_quota_refusal_whichever_operation_met_it() {
    for full in [
        FullStore::Graphs,
        FullStore::Snapshots,
        FullStore::ImportTasks,
        FullStore::PrivateEndpoints,
    ] {
        let case = full.records();
        let (router, engine) = emulator();
        let graph_id = graph_in_service(&router, case, "ceiling-source").await;
        let snapshot_id =
            snapshot_in_service(&router, case, &graph_id, "ceiling-source-snapshot").await;

        // The store this group is about is filled right up to its ceiling, so what every
        // row below meets is a store with no room left rather than anything about its
        // own request. Every OTHER store keeps room, so a row's refusal can only be
        // about the store it is filling.
        fill_to(&engine, full, &graph_id, full.ceiling());
        assert_eq!(
            stored_count(&engine, full),
            full.ceiling(),
            "case {case:?}: expected the store filled to its ceiling, got {}",
            stored_count(&engine, full)
        );

        let mut answers: Vec<(&str, String)> = Vec::new();
        for row in rows(full) {
            let CeilingCase { case, op, body } = row;
            let operation = op.name();
            let before = counts(&engine);
            let (status, refused) = call(
                &router,
                "POST",
                &op.path(&graph_id, &snapshot_id),
                Some(body(&graph_id)),
            )
            .await;
            assert_eq!(
                status, 402,
                "case {case:?}: expected 402 from {operation} against a full store, got \
                 {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ServiceQuotaExceededException",
                "case {case:?}: expected a ServiceQuotaExceededException from \
                 {operation}, got {refused}"
            );
            let refusal = message(&refused);
            assert_eq!(
                refusal,
                full.refusal(),
                "case {case:?}: unexpected refusal wording from {operation}, got \
                 {refusal:?}"
            );
            assert_eq!(
                refused["resourceType"],
                json!(full.resource_type()),
                "case {case:?}: expected the refusal to name the kind of record that did \
                 not fit, got {refused}"
            );
            assert_eq!(
                refused["serviceCode"],
                json!(ARN_NAMESPACE),
                "case {case:?}: expected the refusal to name this service, got {refused}"
            );
            // The store as a whole is what has no room left, and these ceilings are the
            // emulator's own resource guards rather than quotas AWS publishes, so no
            // stored record and no Service Quotas code is named.
            for absent in ["resourceId", "quotaCode"] {
                assert_eq!(
                    refused[absent],
                    Value::Null,
                    "case {case:?}: expected no {absent} member from {operation}, got \
                     {refused}"
                );
            }
            assert_eq!(
                counts(&engine),
                before,
                "case {case:?}: expected {operation} to mint nothing against a full \
                 store, got {:?} where it held {before:?}",
                counts(&engine)
            );
            answers.push((case, refusal));
        }

        // The heart of the rule: the operations that met THIS full store are not
        // distinguishable by the wording of their refusal.
        let (first_case, first) = answers
            .first()
            .cloned()
            .expect("every store has at least one operation that grows it");
        for (case, refusal) in &answers {
            assert_eq!(
                refusal, &first,
                "case {case:?}: expected the same sentence {first_case:?} answered for \
                 the same full store ({first:?}), got {refusal:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_record_below_a_ceiling_the_same_growth_request_is_accepted() {
    for full in [
        FullStore::Graphs,
        FullStore::Snapshots,
        FullStore::ImportTasks,
        FullStore::PrivateEndpoints,
    ] {
        let case = full.records();
        let (router, engine) = emulator();
        let graph_id = graph_in_service(&router, case, "boundary-source").await;

        // One record below the ceiling the store still has room, so the request the full
        // store refuses is accepted here: the refusal is the BOUND being met, not a
        // blanket refusal of the request.
        fill_to(&engine, full, &graph_id, full.ceiling() - 1);
        assert_eq!(
            stored_count(&engine, full),
            full.ceiling() - 1,
            "case {case:?}: expected the store one record below its ceiling, got {}",
            stored_count(&engine, full)
        );

        let (status, accepted) = call(
            &router,
            "POST",
            &growth_path(full, &graph_id),
            Some(growth_request(full, &graph_id, 1)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 one record below the ceiling, got {status}: \
             {accepted}"
        );
        assert_eq!(
            stored_count(&engine, full),
            full.ceiling(),
            "case {case:?}: expected the accepted call to fill the store to its ceiling, \
             got {}",
            stored_count(&engine, full)
        );

        // The accepted record round-trips through the read of its own namespace, so what
        // filled the last slot is a stored record and not a reported one.
        let (member, read_path) = read_back(full, &graph_id, &accepted);
        let (read_status, read) = call(&router, "GET", &read_path, None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 reading back {read_path:?}, got {read_status}: \
             {read}"
        );
        assert_eq!(
            read[member], accepted[member],
            "case {case:?}: expected the read to answer the record the call minted, got \
             {read}"
        );

        // And the next growth request, now that the ceiling is met, is the refusal.
        let (refused_status, refused) = call(
            &router,
            "POST",
            &growth_path(full, &graph_id),
            Some(growth_request(full, &graph_id, 2)),
        )
        .await;
        assert_eq!(
            refused_status, 402,
            "case {case:?}: expected 402 from the call past the ceiling, got \
             {refused_status}: {refused}"
        );
        assert_eq!(
            message(&refused),
            full.refusal(),
            "case {case:?}: unexpected refusal wording, got {refused}"
        );
        assert_eq!(
            refused["resourceType"],
            json!(full.resource_type()),
            "case {case:?}: expected the refusal to name the kind of record that did not \
             fit, got {refused}"
        );
    }
}

/// Where a request that grows the store of `full` is sent.
fn growth_path(full: FullStore, graph_id: &str) -> String {
    match full {
        FullStore::Graphs => Op::CreateGraph.path(graph_id, ""),
        FullStore::Snapshots => Op::CreateGraphSnapshot.path(graph_id, ""),
        FullStore::ImportTasks => Op::CreateGraphUsingImportTask.path(graph_id, ""),
        FullStore::PrivateEndpoints => Op::CreatePrivateGraphEndpoint.path(graph_id, ""),
    }
}

/// The `nth` well-formed request that grows the store of `full` by one record.
fn growth_request(full: FullStore, graph_id: &str, nth: u32) -> Value {
    match full {
        FullStore::Graphs => json!({
            "graphName": format!("boundary-graph-{nth}"),
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        }),
        FullStore::Snapshots => json!({
            "graphIdentifier": graph_id,
            "snapshotName": format!("boundary-snapshot-{nth}"),
        }),
        FullStore::ImportTasks => json!({
            "graphName": format!("boundary-import-{nth}"),
            "source": "s3://import-bucket/boundary/",
            "roleArn": ROLE_ARN,
            "format": "CSV",
        }),
        FullStore::PrivateEndpoints => json!({
            "vpcId": format!("vpc-boundary{nth}"),
            "subnetIds": subnets(1),
        }),
    }
}

/// The member that identifies the accepted record, and the path the read of its own
/// namespace answers it under.
fn read_back(full: FullStore, graph_id: &str, accepted: &Value) -> (&'static str, String) {
    let member = match full {
        FullStore::Graphs | FullStore::Snapshots => "id",
        FullStore::ImportTasks => "taskId",
        FullStore::PrivateEndpoints => "vpcId",
    };
    let value = accepted[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no {member} in {accepted}", full.records()))
        .to_string();
    let path = match full {
        FullStore::Graphs => format!("/graphs/{value}"),
        FullStore::Snapshots => format!("/snapshots/{value}"),
        FullStore::ImportTasks => format!("/importtasks/{value}"),
        FullStore::PrivateEndpoints => format!("/graphs/{graph_id}/endpoints/{value}"),
    };
    (member, path)
}
