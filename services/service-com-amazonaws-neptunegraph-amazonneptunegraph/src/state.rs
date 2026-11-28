// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-neptunegraph-amazonneptunegraph service.
//!
//! The graph resource is the root of this service's control plane: snapshots,
//! import/export tasks, private endpoints and queries all hang off a graph, so the
//! store below is the one every later slice extends rather than duplicates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

use crate::ops::types::{
    AccessDeniedException, ComputeConfiguration, ConflictException, ExportFilter,
    GraphSessionConfiguration, ImportOptions, ProvisionedGraphConfiguration,
    ResourceNotFoundException, ServiceQuotaExceededException, ValidationException,
    VectorSearchConfiguration,
};

/// The ARN namespace (and SigV4 signing name) of this service, from the model's
/// `aws.api#service` trait:
/// `smithy_query(query="service_info")` -> `awsService.arnNamespace == "neptune-graph"`.
pub const ARN_NAMESPACE: &str = "neptune-graph";

/// How long after the call that starts it a graph's provisioning (or a
/// reconfiguration) is owed: the graph answers `CREATING`/`UPDATING` until this span
/// has passed, then the next read settles it to `AVAILABLE`.
///
/// The real service takes minutes; this emulator picks a span inside the
/// emulator-wide 250ms..750ms band for a transition the clock owes, so a caller that
/// acts immediately after `CreateGraph` still observes the transient status (and the
/// state errors that go with it) without any test having to wait minutes. No handler
/// ever sleeps for it — the due instant is stored and compared on read.
pub const PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// How long after the `DeleteGraph` that starts it a graph's deletion is owed: the
/// graph answers `DELETING` — and stays listed, with its name still reserved — until
/// this span has passed, after which the next read finds the record gone.
///
/// The real service takes minutes to tear a graph down; this emulator picks a span
/// inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the graph straight after `DeleteGraph` still observes the
/// transient status (and the state errors that go with it). No handler ever sleeps for
/// it — the due instant is stored and compared on read.
pub const DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after the `DeleteGraphSnapshot` that accepts it a snapshot's removal is
/// owed: the snapshot answers `DELETING` — and stays listed, with its name still
/// reserved — until this span has passed, after which the next read finds the record
/// gone.
///
/// The real service takes minutes to discard a snapshot's data; this emulator picks a
/// span inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the snapshot straight after `DeleteGraphSnapshot` still observes the
/// transient `DELETING` status (and the state errors that go with it). No handler ever
/// sleeps for it — the due instant is stored and compared on read.
pub const SNAPSHOT_DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after the call that accepts it a cancellation is owed: the cancelled
/// record answers `CANCELLING` until this span has passed, then the next read sees the
/// cancellation through — a query is terminated and no longer reported, and an export
/// task reports `CANCELLED`.
///
/// The real service stops the cancelled work as soon as its engine notices the cancel;
/// this emulator picks a span inside the emulator-wide 250ms..750ms band for a
/// transition the clock owes, so a caller that reads the record straight after
/// cancelling it still observes the transient `CANCELLING` state. No handler ever sleeps
/// for it — the due instant is stored and compared on read.
pub const CANCELLATION_SPAN: Duration = Duration::from_millis(500);

/// How long an admitted query may run before the service stops it: the query timeout.
///
/// `ExecuteQuery` takes the timeout per request — "Specifies the query timeout duration,
/// in milliseconds. (optional)"
/// (`smithy_query(query="doc",shape="ExecuteQueryInput",member="queryTimeoutMilliseconds")`)
/// — and this is the value a request that names none runs under: the 120 seconds Neptune
/// Analytics documents as a graph's query timeout. It is an upper bound on the life of a
/// query RECORD, reached only by a query nothing stopped: a query is over when its work
/// is, or when a cancel the clock has made due takes effect, so nothing here ends a query
/// a caller is still listing and cancelling. No handler sleeps for it — the due instant
/// is stored and compared on read.
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_millis(120_000);

/// How long an accepted query's run lasts in this emulator: the span after which the
/// clock owes the query record its end.
///
/// A query is a unit of work this service accepts and then reports on through its own
/// operations (`ListQueries`, `GetQuery`, `CancelQuery`), so — like the run of an
/// accepted import task ([`IMPORT_RUN_SPAN`]) — its record is in flight for the span the
/// clock owes it and is reclaimed once that span is up. The evaluation itself happens in
/// this process over data this process holds, in a fraction of the time the real service
/// spends on the same query, so the span rather than the microseconds an evaluator spent
/// is what makes an accepted query observable: a caller that submits one and then lists,
/// reads or cancels it finds it, exactly as it would on the real data plane. It is longer
/// than the span a single status transition of this service is owed (`PROVISIONING_SPAN`
/// and its siblings) because what has to fit inside it is a whole exchange rather than
/// one read: a client that lists the graph's queries, reads the one it submitted and then
/// cancels it.
///
/// The span is the RUN's, so it is owed whatever that run answered — exactly as an import
/// task whose load failed still runs through its phases before reporting the failure. A
/// run still asking when the span is up keeps its record until it stops
/// ([`Query::end_run`]), while a cancel the clock has made due and a timeout that has run
/// out each end the record earlier — so a cancel can only ever shorten a query's life. No
/// handler ever sleeps for it: the due instant is stored and compared on read.
pub const QUERY_RUN_SPAN: Duration = Duration::from_millis(3_000);

/// How long after the `DeletePrivateGraphEndpoint` that accepts it a private graph
/// endpoint's removal is owed: the endpoint answers `DELETING` until this span has
/// passed, then the next read finds it gone.
///
/// The real service tears a VPC endpoint down over minutes; this emulator picks a span
/// inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the endpoint straight after deleting it still observes the
/// transient `DELETING` status. No handler ever sleeps for it — the due instant is
/// stored and compared on read.
pub const ENDPOINT_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The number of replicas a graph gets when the request that mints it supplies no
/// `replicaCount`, from the member's own documentation: "The number of replicas in
/// other AZs. Min =0, Max = 2, Default = 1."
/// (`smithy_query(query="doc",shape="CreateGraphInput",member="replicaCount")`, and the
/// same sentence on `CreateGraphUsingImportTaskInput` and
/// `RestoreGraphFromSnapshotInput`).
///
/// Stated once here because it is the documented answer for EVERY operation that mints
/// a graph: a member whose model documentation spells out a default takes that default
/// when the request omits it, whatever value the call could otherwise have inherited.
pub const DEFAULT_REPLICA_COUNT: i32 = 1;

/// The member path a refusal of a vector-search dimension is reported at: the
/// configuration member, then the dimension inside it, spelled the way the walk in front
/// of a handler spells a path into a nested structure member (lowerCamel segments joined
/// by dots).
const VECTOR_SEARCH_DIMENSION_MEMBER: &str = "vectorSearchConfiguration.dimension";

/// The most dimensions a vector-search configuration may ask a graph for, from the
/// member's own documentation: "Specifies the number of dimensions for vector embeddings
/// that will be loaded into the graph. The value is specified as `dimension=`value. Max =
/// 65,535"
/// (`smithy_query(query="doc",shape="CreateGraphInput",member="vectorSearchConfiguration")`,
/// and the structure itself publishes the same number —
/// `smithy_query(query="shape",shape="VectorSearchConfiguration")` -> "Max = 65535").
///
/// The modelled `@range` on `VectorSearchDimension` is `min: 1, max: 65536` — one ABOVE
/// the number both of those sentences publish — so the boundary validator admits a
/// dimension the service documents as out of range, and the published maximum is the
/// bound enforced here ([`vector_search_dimension_clause`]). The minimum (1) and every
/// value past the modelled maximum are already refused at the boundary by the `@range`
/// trait, so this is the only part of the documented range nothing else applies.
pub const MAX_VECTOR_SEARCH_DIMENSION: i32 = 65_535;

/// The `ValidationException` clause a `vectorSearchConfiguration` fails, or `None` when
/// its dimension is within the documented bound ([`MAX_VECTOR_SEARCH_DIMENSION`]).
///
/// Stated ONCE here and reached by every operation whose input carries a
/// `vectorSearchConfiguration` (`CreateGraph`, `CreateGraphUsingImportTask`), so the two
/// cannot drift into different acceptance sets for one documented member and no graph
/// record can be minted holding a dimension the service's own documentation puts out of
/// range — which is also what keeps it out of what a later read, a snapshot's captured
/// configuration and a restore from that snapshot report.
///
/// The clause is the one the boundary validator renders for a `@range` maximum on the
/// same member — the value, its member path, and the bound as the requirement — so a
/// caller cannot tell the bound this handler applies from the bound the model carries.
pub fn vector_search_dimension_clause(
    configuration: Option<&VectorSearchConfiguration>,
) -> Option<String> {
    let dimension = configuration?.dimension;
    if dimension > MAX_VECTOR_SEARCH_DIMENSION {
        return Some(constraint_clause(
            VECTOR_SEARCH_DIMENSION_MEMBER,
            Some(&dimension.to_string()),
            &max_value_requirement(MAX_VECTOR_SEARCH_DIMENSION),
        ));
    }
    None
}

/// The build a graph record reports for the software serving it: "The build number of
/// the graph software"
/// (`smithy_query(query="doc",shape="CreateGraphOutput",member="buildNumber")`).
///
/// The software serving these graphs is this emulator, so the build reported is this
/// crate's own version — a real build identifier of the thing answering, rather than a
/// version string of an engine nothing here runs. It is a property of every graph this
/// service holds, so it is the same for all of them and stable across reads.
pub const GRAPH_SOFTWARE_BUILD_NUMBER: &str = env!("CARGO_PKG_VERSION");

/// The lifecycle statuses this slice of the emulator puts a graph into.
///
/// Every variant's wire form is one of the modelled `GraphStatus` values
/// (`crate::ops::types::GRAPH_STATUS_VALUES`). The status this slice never
/// produces (`FAILED`) belongs to a failure path nothing in this emulator takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStatus {
    /// `CreateGraph` has accepted the graph; it is not usable yet.
    Creating,
    /// The graph is provisioned and serving.
    Available,
    /// `UpdateGraph` has accepted a reconfiguration.
    Updating,
    /// `CreateGraphSnapshot` has accepted a snapshot of the graph; the graph is
    /// being snapshotted and is not reconfigurable until that finishes.
    Snapshotting,
    /// `StartImportTask` or `CreateGraphUsingImportTask` has accepted a bulk import
    /// into the graph; the graph is loading data and is not reconfigurable until the
    /// task stops.
    Importing,
    /// `StopGraph` has accepted the stop; the graph is winding down.
    Stopping,
    /// The graph is stopped: it exists and keeps its configuration and data, but it
    /// is not serving, so nothing reaches its endpoint until `StartGraph`.
    Stopped,
    /// `StartGraph` has accepted the start; the graph is coming back up.
    Starting,
    /// `DeleteGraph` has accepted the deletion.
    Deleting,
    /// `ResetGraph` has emptied the graph's data; the graph is coming back up on it.
    Resetting,
}

impl GraphStatus {
    /// This status as the modelled `GraphStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Updating => "UPDATING",
            Self::Snapshotting => "SNAPSHOTTING",
            Self::Importing => "IMPORTING",
            Self::Stopping => "STOPPING",
            Self::Stopped => "STOPPED",
            Self::Starting => "STARTING",
            Self::Deleting => "DELETING",
            Self::Resetting => "RESETTING",
        }
    }

    /// Whether this is a RESTING status: one the clock owes nothing out of.
    ///
    /// A graph rests either serving (`AVAILABLE`) or stopped (`STOPPED`), and a resting
    /// graph holds its data and its configuration settled; every other status this slice
    /// produces is a transition already in flight, during which a request that would
    /// start another transition, or capture the data moving under it, is a conflict.
    pub const fn is_at_rest(self) -> bool {
        matches!(self, Self::Available | Self::Stopped)
    }

    /// Whether a graph in this status is SERVING: whether the graph's OWN endpoint —
    /// the per-graph data plane every operation the model binds to the host prefix
    /// `{graphIdentifier}.` is reached through — answers at all.
    ///
    /// A graph serves only while it is `AVAILABLE`: one that is provisioning,
    /// reconfiguring, importing, snapshotting, starting, stopped or on its way out has
    /// no data plane behind that endpoint, so there is nothing there to read a summary,
    /// a query listing or one query's status from, and nothing to cancel a query on.
    /// Stated once here so every operation on that endpoint applies the same
    /// precondition.
    pub const fn is_serving(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// A status change the clock owes a graph: the status it lands in, and the instant
/// from which it is due.
///
/// The target is carried beside the instant because a graph does not always settle
/// into the same status: provisioning, reconfiguring and starting settle to
/// `AVAILABLE`, while stopping settles to `STOPPED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTransition {
    /// The status the graph takes once this transition falls due.
    pub settled_status: GraphStatus,
    /// The instant from which the transition is due.
    pub due: Instant,
}

impl PendingTransition {
    /// The transition a call starting at `now` owes: `settled_status`, due
    /// [`PROVISIONING_SPAN`] later.
    pub fn owed(settled_status: GraphStatus, now: Instant) -> Self {
        Self {
            settled_status,
            due: now + PROVISIONING_SPAN,
        }
    }
}

/// One node of a graph's data: the labels it carries and the properties set on it.
///
/// A node is addressed by its own id (openCypher's `~id`), which is the key it sits
/// under in [`GraphData::nodes`], so the id is not repeated here.
///
/// `PartialEq` compares what the node IS, which is what a write is judged against: an
/// evaluation that left a node exactly as it found it changed nothing, and nothing about
/// the graph may move for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphNode {
    /// The node's labels. openCypher lets one node carry several, and a summary of
    /// the data counts DISTINCT labels, so they are held as a set.
    pub labels: BTreeSet<String>,
    /// The properties set on the node, by key. A value is held as the JSON document
    /// the data plane exchanges it as, so a property keeps the type it was written
    /// with; a summary of the data counts keys and usages only.
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// One edge of a graph's data: its label, the nodes it joins, and its properties.
///
/// An edge is addressed by its own id, which is the key it sits under in
/// [`GraphData::edges`], so the id is not repeated here. `PartialEq` compares what the
/// edge IS, for the reason [`GraphNode`] carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    /// The edge's label. openCypher gives an edge exactly one.
    pub label: String,
    /// The id of the node this edge leaves.
    pub from_node: String,
    /// The id of the node this edge enters.
    pub to_node: String,
    /// The properties set on the edge, by key, held as [`GraphNode::properties`]
    /// holds a node's.
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// The DATA one graph holds: its nodes and its edges, plus the instant the
/// statistics over them were last computed.
///
/// It lives INSIDE [`Graph`] rather than in a map of its own, so a graph's data is
/// created, read and reclaimed with the graph that holds it, and no second copy of
/// a graph's contents can drift from this one. Both maps are `BTreeMap`s, so every
/// reading of the data — a count, a label list, a grouping — has one stable total
/// order to walk.
///
/// Writing it belongs to the data plane (an openCypher write, a reset, a bulk
/// load); the mutators below are the one way in, because each of them also moves
/// [`GraphData::statistics_computed_at`].
#[derive(Debug, Clone)]
pub struct GraphData {
    /// Every node of this graph, keyed by its node id.
    pub nodes: BTreeMap<String, GraphNode>,
    /// Every edge of this graph, keyed by its edge id.
    pub edges: BTreeMap<String, GraphEdge>,
    /// When the statistics over this data were last computed.
    ///
    /// Statistics are a function of the data, so they are recomputed when the data
    /// changes and not otherwise: every mutator below sets this to the instant its
    /// caller is writing at, and a graph nobody has written to since it was created
    /// still reports the statistics of its empty data, computed when the graph came
    /// up.
    pub statistics_computed_at: SystemTime,
}

impl GraphData {
    /// The empty data a graph starts out holding, its statistics computed `as_of` —
    /// the instant the graph itself was created.
    pub fn empty(as_of: SystemTime) -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            statistics_computed_at: as_of,
        }
    }

    /// Store one node under `id`, replacing the node that id already named. The
    /// statistics over the data are recomputed `as_of` the instant the caller is
    /// writing at — the store reads no clock of its own.
    ///
    /// `Err` carries the reason the store refuses the write, for the caller to
    /// report in the wording its own operation uses.
    pub fn insert_node(
        &mut self,
        id: &str,
        node: GraphNode,
        as_of: SystemTime,
    ) -> Result<(), String> {
        if node.properties.len() > MAX_PROPERTIES_PER_ELEMENT {
            return Err(format!(
                "this emulator holds at most {MAX_PROPERTIES_PER_ELEMENT} properties per node"
            ));
        }
        if !self.nodes.contains_key(id) && self.nodes.len() >= MAX_NODES_PER_GRAPH {
            return Err(format!(
                "this emulator holds at most {MAX_NODES_PER_GRAPH} nodes per graph"
            ));
        }
        self.nodes.insert(id.to_string(), node);
        self.statistics_computed_at = as_of;
        Ok(())
    }

    /// Store one edge under `id`, replacing the edge that id already named, with
    /// the statistics recomputed `as_of` the instant the caller is writing at.
    ///
    /// An edge joins two nodes OF THIS GRAPH, so an endpoint the graph does not
    /// hold is refused: an edge dangling off a node nobody can read would be
    /// counted by every later summary of the data.
    pub fn insert_edge(
        &mut self,
        id: &str,
        edge: GraphEdge,
        as_of: SystemTime,
    ) -> Result<(), String> {
        if edge.properties.len() > MAX_PROPERTIES_PER_ELEMENT {
            return Err(format!(
                "this emulator holds at most {MAX_PROPERTIES_PER_ELEMENT} properties per edge"
            ));
        }
        for endpoint in [&edge.from_node, &edge.to_node] {
            if !self.nodes.contains_key(endpoint) {
                return Err(format!(
                    "edge {id} joins node {endpoint}, which this graph does not hold"
                ));
            }
        }
        if !self.edges.contains_key(id) && self.edges.len() >= MAX_EDGES_PER_GRAPH {
            return Err(format!(
                "this emulator holds at most {MAX_EDGES_PER_GRAPH} edges per graph"
            ));
        }
        self.edges.insert(id.to_string(), edge);
        self.statistics_computed_at = as_of;
        Ok(())
    }

    /// Whether this graph holds NOTHING: no nodes and no edges.
    ///
    /// Derived from the two maps rather than tracked beside them, so it cannot
    /// disagree with the data a summary of the graph is computed from. What an
    /// operation whose documented precondition is an EMPTY target graph decides on
    /// (see [`graph_not_empty_conflict`]): a graph that was created empty and never
    /// written to, or whose data a reset carried away, satisfies it; one that came up
    /// holding a snapshot's capture, or has been written to since, does not.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Drop every node and edge: the data a graph holds after it is emptied, with
    /// the statistics recomputed `as_of` the instant the caller is emptying it at.
    pub fn clear(&mut self, as_of: SystemTime) {
        self.nodes.clear();
        self.edges.clear();
        self.statistics_computed_at = as_of;
    }

    /// This data as the data of a graph that is coming up holding it: the same
    /// nodes and edges, with the statistics over them computed `as_of` the instant
    /// that graph came up.
    ///
    /// What a restore reads a snapshot's captured data through: the new graph's data
    /// is the captured data, and the statistics over it are the ones the new graph
    /// came up with rather than the ones the graph it was captured from had. It
    /// CONSUMES the data, so the one copy taken off the snapshot becomes the new
    /// graph's own rather than being duplicated again.
    pub fn into_as_of(mut self, as_of: SystemTime) -> Self {
        self.statistics_computed_at = as_of;
        self
    }

    /// The distinct labels of the edges LEAVING `node_id`.
    ///
    /// Derived from the edges rather than stored on the node, so it cannot disagree
    /// with the edges the graph holds. It SCANS the edges, so it answers about ONE
    /// node: a reading that needs this for every node indexes the edges once
    /// instead (see `crate::ops::get_graph_summary`).
    pub fn outgoing_edge_labels(&self, node_id: &str) -> BTreeSet<String> {
        self.edges
            .values()
            .filter(|edge| edge.from_node == node_id)
            .map(|edge| edge.label.clone())
            .collect()
    }
}

/// One stored Neptune Analytics graph: every member `GetGraph` reports, plus the
/// status change the clock still owes it, if any.
#[derive(Debug, Clone)]
pub struct Graph {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region the request that created this graph was signed for.
    ///
    /// neptune-graph is a REGIONAL control plane: a graph exists in exactly one
    /// Region, so this is the lookup and listing predicate every operation applies
    /// beside the identifier (see [`Self::is_in`]) — not merely the Region that names
    /// the record's ARN and endpoint.
    pub region: String,
    pub status: GraphStatus,
    pub status_reason: Option<String>,
    pub create_time: SystemTime,
    /// The compute serving this graph: ONE configuration, reported through both of the
    /// members a graph record names it with (see [`GraphCompute`]).
    pub compute: GraphCompute,
    pub endpoint: String,
    pub public_connectivity: bool,
    pub replica_count: i32,
    /// The customer managed key this graph's data is encrypted with: the key the
    /// request that minted the graph named, and `None` when it named none.
    ///
    /// A graph the caller named no key for is encrypted with a key the service owns,
    /// and such a key has no ARN a caller may name, so there is no value to report:
    /// every member this is reported through is an optional one targeting `KmsKeyArn`,
    /// which the model constrains to a KMS key ARN
    /// (`smithy_query(query="shape",shape="KmsKeyArn")`), so the member is left out
    /// rather than filled with a stand-in no `KmsKeyArn` can hold.
    pub kms_key_identifier: Option<String>,
    pub vector_search_configuration: Option<VectorSearchConfiguration>,
    pub deletion_protection: bool,
    pub source_snapshot_id: Option<String>,
    /// The build of the graph software serving this graph
    /// ([`GRAPH_SOFTWARE_BUILD_NUMBER`]), which every graph record reports.
    pub build_number: String,
    pub tags: BTreeMap<String, String>,
    /// The status change the clock owes this graph, and when it falls due. `None`
    /// once there is nothing outstanding.
    pub pending: Option<PendingTransition>,
    /// When the accepted deletion of this graph completes, after which the record is
    /// no longer one this service holds. `None` for a graph nobody has deleted.
    ///
    /// A deletion is a transition like any other — the graph reports `DELETING` and
    /// stays observable while it runs — but what it settles into is the ABSENCE of
    /// the record, which no `GraphStatus` can express, so it is carried here rather
    /// than in [`Graph::pending`].
    pub deletes_at: Option<Instant>,
    /// The nodes and edges this graph holds: the data every reading of the graph's
    /// contents (a summary of it, a query over it, an export of it) is computed
    /// from. A freshly created graph holds none.
    pub data: GraphData,
}

impl Graph {
    /// Whether this record belongs to `region`: the predicate that makes a resource
    /// of a regional control plane invisible outside the Region that holds it.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// This is the one write a reporting operation (`GetGraph`, `ListGraphs`) is
    /// allowed to make: the graph would have reached the pending transition's
    /// `settled_status` at its `due` instant whether or not anybody called, so
    /// applying it on read is bookkeeping, not a state change caused by the read.
    pub fn settle(&mut self, now: Instant) {
        let Some(pending) = self.pending else {
            return;
        };
        if now >= pending.due {
            self.status = pending.settled_status;
            self.pending = None;
        }
    }

    /// Report `transient` now and settle into `settled` [`PROVISIONING_SPAN`] after
    /// `now`: the shape of every graph transition an operation accepts.
    pub fn begin_transition(&mut self, transient: GraphStatus, settled: GraphStatus, now: Instant) {
        self.status = transient;
        self.pending = Some(PendingTransition::owed(settled, now));
    }

    /// Report the modelled `IMPORTING` status for as long as the import task accepted
    /// at `now` runs, and come back `AVAILABLE` when that load lands.
    ///
    /// The span is the task's own ([`IMPORT_RUN_SPAN`]), not the provisioning one, so
    /// the graph is serving again exactly when it holds what the load wrote: a graph
    /// that reported `AVAILABLE` while its only load was still running would claim to
    /// be ready before it held the data.
    pub fn begin_import(&mut self, now: Instant) {
        self.status = GraphStatus::Importing;
        self.pending = Some(PendingTransition {
            settled_status: GraphStatus::Available,
            due: now + IMPORT_RUN_SPAN,
        });
    }

    /// Report `DELETING` now and complete the deletion [`DELETION_SPAN`] after `now`.
    ///
    /// Any status change that was still outstanding is dropped: the graph is on its way
    /// out, so nothing it was heading for can fall due any more.
    pub fn begin_deletion(&mut self, now: Instant) {
        self.status = GraphStatus::Deleting;
        self.pending = None;
        self.deletes_at = Some(now + DELETION_SPAN);
    }

    /// Whether the deletion this graph is undergoing has fallen due, so the record is
    /// one the service no longer holds.
    pub fn deletion_complete(&self, now: Instant) -> bool {
        self.deletes_at.is_some_and(|due| now >= due)
    }
}

/// The configuration of one graph: the members that describe how it is provisioned
/// and reachable, as opposed to its identity, its lifecycle or its data.
///
/// A snapshot carries the configuration of the graph it was taken from
/// ([`Snapshot::source_configuration`]), because a graph restored from a snapshot
/// reports these members whether or not the restore request carries a value for them:
/// `RestoreGraphFromSnapshot` accepts no `vectorSearchConfiguration` member at all yet
/// reports one (`smithy_query(query="operation",
/// operation="RestoreGraphFromSnapshot")`), so the value can only come from the
/// snapshot, and the members it does accept override the captured value rather than
/// replacing a graph's configuration with defaults. The exception is a member whose own
/// documentation states a default for an omitted value — [`DEFAULT_REPLICA_COUNT`] —
/// which the documented default answers for, so the capture of it is a record of the
/// snapshotted graph rather than the value a restore reports.
#[derive(Debug, Clone)]
pub struct GraphConfiguration {
    pub compute: GraphCompute,
    pub public_connectivity: bool,
    pub replica_count: i32,
    pub deletion_protection: bool,
    pub vector_search_configuration: Option<VectorSearchConfiguration>,
}

impl From<&Graph> for GraphConfiguration {
    fn from(graph: &Graph) -> Self {
        Self {
            compute: graph.compute,
            public_connectivity: graph.public_connectivity,
            replica_count: graph.replica_count,
            deletion_protection: graph.deletion_protection,
            vector_search_configuration: graph.vector_search_configuration.clone(),
        }
    }
}

/// The lifecycle statuses this slice of the emulator puts a graph snapshot into.
///
/// Every variant's wire form is one of the modelled `SnapshotStatus` values
/// (`crate::ops::types::SNAPSHOT_STATUS_VALUES`). `FAILED` is deliberately never
/// produced: nothing here can fail to capture a stored graph's data, so no snapshot may
/// report a failure that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// `CreateGraphSnapshot` has accepted the snapshot; it is not restorable yet.
    Creating,
    /// The snapshot is complete and restorable.
    Available,
    /// `DeleteGraphSnapshot` has accepted the deletion; the snapshot is on its way out
    /// and is reclaimed once the clock makes its removal due.
    Deleting,
}

impl SnapshotStatus {
    /// This status as the modelled `SnapshotStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored graph snapshot: every member `GetGraphSnapshot` reports, plus the
/// instant at which the pending `CREATING` -> `AVAILABLE` transition becomes due.
///
/// A snapshot is a resource in its own right — it outlives the graph it was taken
/// from — so it is stored beside the graphs rather than inside the graph record.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// The Region the request that created this snapshot was signed for — the Region
    /// of the graph it was taken from, and the only Region it is visible in. The same
    /// predicate [`Graph::region`] carries, for the same reason.
    pub region: String,
    /// The graph the snapshot was taken from. Kept as the graph's id (not a
    /// borrow), so deleting that graph leaves the snapshot's provenance intact.
    pub source_graph_id: String,
    pub status: SnapshotStatus,
    pub snapshot_create_time: SystemTime,
    /// The customer managed key the source graph's data is encrypted with: a snapshot
    /// of a graph encrypted with a caller-named key is encrypted with the same key, and
    /// is `None` for a snapshot of a graph that named none, for the reason
    /// [`Graph::kms_key_identifier`] is.
    pub kms_key_identifier: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When the transition the clock owes this snapshot falls due: the move from
    /// `CREATING` to `AVAILABLE`, or the removal of a `DELETING` snapshot. `None` once
    /// the snapshot rests `AVAILABLE`, which is the only status nothing is owed out of.
    pub settles_at: Option<Instant>,
    /// The configuration of the source graph AS IT WAS when the snapshot was taken:
    /// what a graph restored from this snapshot is provisioned with, member by
    /// member, unless the restore request overrides it.
    ///
    /// Captured here rather than looked up on the source graph, because a snapshot
    /// outlives the graph it was taken from and that graph can be reconfigured
    /// afterwards: a restore reproduces the snapshotted graph, not the current one.
    pub source_configuration: GraphConfiguration,
    /// The nodes and edges the source graph held when the snapshot was taken: the
    /// data a graph restored from this snapshot comes up holding.
    ///
    /// Captured for the same reason [`Self::source_configuration`] is — a snapshot is
    /// a copy of the data as of the instant it was taken, which later writes to the
    /// source graph (and the deletion of that graph) leave untouched.
    pub data: GraphData,
}

impl Snapshot {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// snapshot would have become `AVAILABLE` at `settles_at` whether or not anybody
    /// called, so applying it on read is bookkeeping rather than a state change
    /// caused by the read. A `DELETING` snapshot is not settled here — it is RECLAIMED,
    /// by [`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_snapshots`],
    /// since the record itself goes away.
    pub fn settle(&mut self, now: Instant) {
        if self.status == SnapshotStatus::Creating && self.settles_at.is_some_and(|due| now >= due)
        {
            self.status = SnapshotStatus::Available;
            self.settles_at = None;
        }
    }

    /// Whether the accepted deletion of this snapshot has fallen due, so the record is
    /// gone and no read may still report it.
    pub fn is_reclaimed(&self, now: Instant) -> bool {
        self.status == SnapshotStatus::Deleting && self.settles_at.is_some_and(|due| now >= due)
    }

    /// Accept this snapshot's deletion: it answers `DELETING` until
    /// [`SNAPSHOT_DELETION_SPAN`] has passed, after which it is reclaimed.
    ///
    /// Any transition that was still outstanding is dropped: the snapshot is on its way
    /// out, so nothing it was heading for can fall due any more — the same rule
    /// [`Graph::begin_deletion`] follows.
    pub fn begin_deletion(&mut self, now: Instant) {
        self.status = SnapshotStatus::Deleting;
        self.settles_at = Some(now + SNAPSHOT_DELETION_SPAN);
    }
}

/// How long each PHASE of an import task's run lasts: the span after which the clock
/// owes the task the next status of its progression.
///
/// The real service loads for as long as the data takes; this emulator picks a span
/// inside the emulator-wide 250ms..750ms band for a transition the clock owes, so a
/// caller that reads the task straight after starting it still observes the transient
/// `INITIALIZING` status — and can cancel it — while one that polls the task to
/// completion sees the stages of the run before its terminal status. No handler ever
/// sleeps for it: the due instant is stored and compared on read.
pub const IMPORT_TASK_SPAN: Duration = Duration::from_millis(500);

/// How long the whole run of an accepted import task takes: its three phases
/// (`INITIALIZING`, `ANALYZING_DATA`, `IMPORTING`) at [`IMPORT_TASK_SPAN`] each.
///
/// It is the span the load's WRITE into the graph falls due after, so the graph it
/// loads reports `IMPORTING` for exactly as long as the task's run lasts and comes
/// back `AVAILABLE` holding what the load wrote.
pub const IMPORT_RUN_SPAN: Duration = IMPORT_TASK_SPAN.saturating_mul(3);

/// The lifecycle statuses this slice of the emulator puts an import task into.
///
/// Every variant's wire form is one of the modelled `ImportTaskStatus` values
/// (`crate::ops::types::IMPORT_TASK_STATUS_VALUES`). Three of those values are
/// deliberately never produced, each because the condition that would produce it does
/// not arise here: `EXPORTING` (an import reads a source and writes a graph rather than
/// exporting anything), `REPROVISIONING` (the capacity a graph of this emulator is
/// provisioned with is never outgrown by a load, which is bounded far below it by
/// [`MAX_NODES_PER_GRAPH`] and [`MAX_EDGES_PER_GRAPH`]), and `DELETED` (no operation of
/// this service deletes an import task).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTaskStatus {
    /// `StartImportTask` has accepted the task; the run has not begun.
    Initializing,
    /// The data the source held is being analysed — "The data is being analyzed to
    /// determine the optimal infrastructure configuration for the new graph"
    /// (`smithy_query(query="doc",shape="GetImportTaskOutput",member="status")`).
    AnalyzingData,
    /// The data is being loaded into the graph.
    Importing,
    /// The load met an error it could not skip, so what it had read is being released
    /// rather than left half-loaded.
    RollingBack,
    /// The run is over: every element the source held was loaded.
    Succeeded,
    /// The run stopped without loading the data.
    Failed,
    /// `CancelImportTask` has accepted the cancellation; the task is winding down.
    Cancelling,
    /// The task stopped because it was cancelled.
    Cancelled,
}

impl ImportTaskStatus {
    /// This status as the modelled `ImportTaskStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Initializing => "INITIALIZING",
            Self::AnalyzingData => "ANALYZING_DATA",
            Self::Importing => "IMPORTING",
            Self::RollingBack => "ROLLING_BACK",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Cancelling => "CANCELLING",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Whether this is a TERMINAL status: the task has stopped, so the clock owes it
    /// nothing more and its outcome can no longer change.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    /// Whether a task in this status can still be cancelled: only one whose load has
    /// not finished — a task waiting to start, analysing its data, or loading it. A
    /// task that has stopped has no work left to stop, one that is rolling back has
    /// already stopped loading, and one already `CANCELLING` is stopping already —
    /// each is a conflict rather than a second cancellation.
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Initializing | Self::AnalyzingData | Self::Importing)
    }

    /// How long a task stays in this status before the status it settles into falls
    /// due. `None` for a terminal status, which settles into nothing.
    ///
    /// Holding the span beside the status is what keeps one timeline for the whole
    /// run: each phase's due instant is the previous one's plus this span, so a task
    /// read long after it started lands on the status it would have reached.
    pub const fn settles_in(self) -> Option<Duration> {
        match self {
            Self::Initializing | Self::AnalyzingData | Self::Importing | Self::RollingBack => {
                Some(IMPORT_TASK_SPAN)
            }
            Self::Cancelling => Some(CANCELLATION_SPAN),
            Self::Succeeded | Self::Failed | Self::Cancelled => None,
        }
    }
}

/// One stored import task: every member `GetImportTask` reports, what reading its
/// source established, and the instant at which the next status of its run is due.
///
/// A task is a resource in its own right — it is read by its own `t-` id and outlives
/// the load it describes — so it is stored beside the graphs rather than inside the
/// graph record.
#[derive(Debug, Clone)]
pub struct ImportTask {
    pub id: String,
    /// The Region the request that started this task was signed for — the same
    /// predicate [`Graph::region`] carries, for the same reason: this is a regional
    /// control plane, so a task of one Region is not visible from another.
    pub region: String,
    /// The graph the data is imported INTO, kept as the graph's id (not a borrow), so
    /// deleting that graph leaves the task's provenance intact.
    pub graph_id: String,
    pub source: String,
    pub format: Option<String>,
    pub parquet_type: Option<String>,
    pub blank_node_handling: Option<String>,
    pub fail_on_error: bool,
    pub role_arn: String,
    pub import_options: Option<ImportOptions>,
    pub status: ImportTaskStatus,
    pub status_reason: Option<String>,
    /// How many times this task has been run. One per accepted `StartImportTask`:
    /// nothing in this emulator retries a task, so the stored count is what
    /// `GetImportTask` reports rather than a number invented per read.
    pub attempt_number: i32,
    /// What reading this task's source established: the elements the load writes and
    /// the counts over the run ([`read_import_source`]). Measured ONCE, when the task
    /// was accepted, so nothing a later caller does changes what this task reports.
    pub load: ImportLoad,
    /// Whether the load's elements have been written into the graph. The write falls
    /// due [`IMPORT_RUN_SPAN`] after the task was accepted and happens exactly once
    /// ([`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_import_loads`]).
    pub delivered: bool,
    /// When the task was accepted: what `ImportTaskDetails$startTime` reports.
    pub start_time: SystemTime,
    /// The monotonic counterpart of [`Self::start_time`], which
    /// `ImportTaskDetails$timeElapsedSeconds` and the run's progress are measured
    /// from.
    pub started_at: Instant,
    /// The `progressPercentage` the run had reached when it stopped short.
    ///
    /// `None` for a run that completed: only a run that was stopped — by a
    /// cancellation, or by an error it could not skip — has a progress that stays
    /// where it was, and the share of a run still in flight is derived from the clock
    /// instead ([`Self::progress_percentage`]).
    pub stopped_progress: Option<i32>,
    /// When the next status of this task's run falls due. `None` once there is nothing
    /// outstanding.
    pub settles_at: Option<Instant>,
}

impl ImportTask {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// The status the clock owes this task next, if any.
    ///
    /// The STAGES are the same for every accepted task — a load is analysed and then
    /// imported — and what differs is where it ends: a task whose source could not be
    /// read has no data to analyse, so it stops where it is, and one whose load met an
    /// error it could not skip releases what it read before it stops. Both answers are
    /// properties of the run that was measured when the task was accepted
    /// ([`Self::load`]), never of the instant a caller reads it.
    fn next_status(&self) -> Option<ImportTaskStatus> {
        match self.status {
            ImportTaskStatus::Initializing => Some(if self.load.source_read {
                ImportTaskStatus::AnalyzingData
            } else {
                ImportTaskStatus::Failed
            }),
            ImportTaskStatus::AnalyzingData => Some(ImportTaskStatus::Importing),
            ImportTaskStatus::Importing => Some(if self.load.failure_reason.is_some() {
                ImportTaskStatus::RollingBack
            } else {
                ImportTaskStatus::Succeeded
            }),
            ImportTaskStatus::RollingBack => Some(ImportTaskStatus::Failed),
            ImportTaskStatus::Cancelling => Some(ImportTaskStatus::Cancelled),
            ImportTaskStatus::Succeeded
            | ImportTaskStatus::Failed
            | ImportTaskStatus::Cancelled => None,
        }
    }

    /// The `statusReason` `status` carries on this task — why the run stopped. A run
    /// that is still going, and one that loaded its data, carries none: there is
    /// nothing to explain.
    fn reason_for(&self, status: ImportTaskStatus) -> Option<String> {
        match status {
            ImportTaskStatus::Failed => self.load.failure_reason.clone(),
            ImportTaskStatus::Cancelled => Some(IMPORT_CANCELLED_REASON.to_string()),
            _ => None,
        }
    }

    /// Materialise the statuses the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// task would have reached these statuses at these instants whether or not anybody
    /// read it, so applying them on read is bookkeeping rather than a state change
    /// caused by the read. Every status the clock owed by `now` is applied, not just
    /// the next one: a task first read long after it started reports the status it
    /// would have reached, never a phase it had already left.
    pub fn settle(&mut self, now: Instant) {
        while let Some(due) = self.settles_at {
            if now < due {
                return;
            }
            match self.next_status() {
                Some(settled) => {
                    // A run that stops short keeps the progress it had reached at the
                    // instant it stopped, which is this one.
                    if settled == ImportTaskStatus::Failed {
                        self.stopped_progress = Some(self.progress_percentage(due));
                    }
                    self.status = settled;
                    self.status_reason = self.reason_for(settled);
                    self.settles_at = settled.settles_in().map(|span| due + span);
                }
                None => self.settles_at = None,
            }
        }
    }

    /// Start this task's run at `now`: it reports `INITIALIZING` and the clock owes it
    /// the next phase [`IMPORT_TASK_SPAN`] later, so a caller acting straight after
    /// the call that accepted it still sees the run in flight.
    pub fn begin_run(&mut self, now: Instant) {
        self.status = ImportTaskStatus::Initializing;
        self.status_reason = None;
        self.started_at = now;
        self.settles_at = ImportTaskStatus::Initializing
            .settles_in()
            .map(|span| now + span);
    }

    /// Accept a cancellation of this task: it reports the modelled `CANCELLING` status
    /// now ("Because you cancelled the import task, cancellation is in progress") and
    /// the clock owes the move to `CANCELLED` [`CANCELLATION_SPAN`] later, so a caller
    /// that reads the task straight after cancelling it still sees it winding down.
    ///
    /// The progress the run had reached is kept as it was at this instant: a stopped
    /// run does no more work, so its share of the work done stops rising here. The
    /// elements it had read are dropped — a cancelled load writes nothing, so the
    /// graph is left as empty as the task found it.
    pub fn begin_cancellation(&mut self, now: Instant) {
        self.stopped_progress = Some(self.progress_percentage(now));
        self.load.data = ImportedData::default();
        self.status = ImportTaskStatus::Cancelling;
        self.status_reason = None;
        self.settles_at = ImportTaskStatus::Cancelling
            .settles_in()
            .map(|span| now + span);
    }

    /// When the load's write into the graph falls due, on the wall clock the graph's
    /// statistics are stamped with: the instant the task was accepted plus the span
    /// its run takes. Derived from the stored record rather than read off a clock, so
    /// the data and the status land at one instant.
    pub fn load_landed_at(&self) -> SystemTime {
        self.start_time
            .checked_add(IMPORT_RUN_SPAN)
            .unwrap_or(self.start_time)
    }

    /// How long the task has been running, in whole seconds: what
    /// `ImportTaskDetails$timeElapsedSeconds` reports.
    pub fn elapsed_seconds(&self, now: Instant) -> i64 {
        i64::try_from(now.saturating_duration_since(self.started_at).as_secs())
            .unwrap_or(i64::MAX)
    }

    /// The share of this task's work that is done, as of `now`: what
    /// `ImportTaskDetails$progressPercentage` reports.
    ///
    /// The work is the RUN, so a task that has not begun reports none of it done, a
    /// run in flight reports the share of [`IMPORT_RUN_SPAN`] that has elapsed — short
    /// of the whole ([`MAX_IN_FLIGHT_PROGRESS_PERCENTAGE`]), because it has not
    /// finished — and a completed run reports the whole of it. A run that stopped
    /// short reports where it had got to.
    pub fn progress_percentage(&self, now: Instant) -> i32 {
        match self.status {
            ImportTaskStatus::Initializing => 0,
            ImportTaskStatus::AnalyzingData
            | ImportTaskStatus::Importing
            | ImportTaskStatus::RollingBack => {
                in_flight_progress(now.saturating_duration_since(self.started_at), IMPORT_RUN_SPAN)
            }
            ImportTaskStatus::Succeeded => COMPLETE_PROGRESS_PERCENTAGE,
            ImportTaskStatus::Failed
            | ImportTaskStatus::Cancelling
            | ImportTaskStatus::Cancelled => self.stopped_progress.unwrap_or(0),
        }
    }

    /// The task's `ImportTaskDetails`, as of `now`.
    ///
    /// The statement and dictionary-entry counts are of the data the load has WRITTEN,
    /// so they are reported once its write has landed and are zero before that: a run
    /// still in flight, and one that stopped without loading anything, has written no
    /// statement. The error count is of what the run MET, so it is reported once the
    /// run has stopped on its own account — which is also when `errorDetails` says
    /// what the first of those errors was.
    pub fn details(&self, now: Instant) -> crate::ops::types::ImportTaskDetails {
        let wrote = self.delivered;
        let stopped_on_its_own = matches!(
            self.status,
            ImportTaskStatus::Succeeded | ImportTaskStatus::Failed
        );
        crate::ops::types::ImportTaskDetails {
            status: self.status.as_wire().to_string(),
            start_time: ::aws_smithy_types::DateTime::from(self.start_time),
            time_elapsed_seconds: self.elapsed_seconds(now),
            progress_percentage: self.progress_percentage(now),
            error_count: if stopped_on_its_own {
                self.load.error_count
            } else {
                0
            },
            error_details: stopped_on_its_own
                .then(|| self.load.error_details.clone())
                .flatten(),
            statement_count: if wrote { self.load.statement_count } else { 0 },
            dictionary_entry_count: if wrote {
                self.load.dictionary_entry_count
            } else {
                0
            },
        }
    }

    /// This task as `GetImportTask`'s output, as of `now`: the stored record plus the
    /// details derived from the clock, so that read cannot drift from what
    /// `StartImportTask` reported.
    pub fn get_output(&self, now: Instant) -> crate::ops::types::GetImportTaskOutput {
        crate::ops::types::GetImportTaskOutput {
            graph_id: Some(self.graph_id.clone()),
            task_id: self.id.clone(),
            source: self.source.clone(),
            format: self.format.clone(),
            parquet_type: self.parquet_type.clone(),
            role_arn: self.role_arn.clone(),
            status: self.status.as_wire().to_string(),
            import_options: self.import_options.clone(),
            import_task_details: Some(self.details(now)),
            attempt_number: Some(self.attempt_number),
            status_reason: self.status_reason.clone(),
        }
    }
}

/// How long each PHASE of an export task lasts: the span after which the clock owes a
/// task the next status of its progression — `INITIALIZING` to `EXPORTING`, and
/// `EXPORTING` to the terminal `SUCCEEDED`.
///
/// The real service exports for as long as the graph's data takes; this emulator picks
/// a span inside the emulator-wide 250ms..750ms band for a transition the clock owes,
/// so a caller that reads the task straight after starting it still observes the
/// transient `INITIALIZING` status, and one that polls the task to completion sees the
/// run in flight before the terminal status. No handler ever sleeps for it — the due
/// instant is stored and compared on read.
pub const EXPORT_TASK_SPAN: Duration = Duration::from_millis(500);

/// The `progressPercentage` of an export task whose run is over: all of the work the
/// task had to do is done.
pub const COMPLETE_PROGRESS_PERCENTAGE: i32 = 100;

/// The most `progressPercentage` a task whose run is still in flight reports: a run
/// that has not finished has not done all of its work, so it stays below
/// [`COMPLETE_PROGRESS_PERCENTAGE`] however late into its span a read falls.
pub const MAX_IN_FLIGHT_PROGRESS_PERCENTAGE: i32 = 99;

/// The `progressPercentage` a run in flight reports: the share of `over` that `elapsed`
/// covers, short of the whole ([`MAX_IN_FLIGHT_PROGRESS_PERCENTAGE`]) because a run that
/// has not finished has not done all of its work.
///
/// ONE function because an import and an export report the same thing about their own
/// span, so neither can drift from the other: each hands over the span its run takes and
/// how much of it has elapsed.
pub fn in_flight_progress(elapsed: Duration, over: Duration) -> i32 {
    // Scaled by 100, the whole of the work ([`COMPLETE_PROGRESS_PERCENTAGE`]).
    let share = elapsed.as_millis().saturating_mul(100) / over.as_millis().max(1);
    i32::try_from(share)
        .unwrap_or(MAX_IN_FLIGHT_PROGRESS_PERCENTAGE)
        .clamp(0, MAX_IN_FLIGHT_PROGRESS_PERCENTAGE)
}

/// The `statusReason` of an export task that stopped because it was cancelled.
pub const EXPORT_CANCELLED_REASON: &str =
    "The export task was cancelled by a CancelExportTask request.";

/// The statuses this slice of the emulator puts an export task into.
///
/// Every variant's wire form is a modelled `ExportTaskStatus` value
/// (`crate::ops::types::EXPORT_TASK_STATUS_VALUES`). The modelled `FAILED` is not among
/// them, and nor is `DELETED`: each joins this enum with the condition that would
/// produce it, and this service holds neither — the elements an accepted export writes
/// are the source graph's own stored nodes and edges, which nothing can make unreadable
/// once the request has been accepted, and no operation deletes an export task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTaskStatus {
    /// `StartExportTask` has accepted the task; the run has not begun, so no element
    /// has been written yet.
    Initializing,
    /// The run is in flight: the elements the export selected are being written to the
    /// destination the request named.
    Exporting,
    /// The run is over: every element the export selected was written.
    Succeeded,
    /// `CancelExportTask` has accepted the cancellation; the task is winding down.
    Cancelling,
    /// The task stopped because it was cancelled ([`EXPORT_CANCELLED_REASON`]).
    Cancelled,
}

impl ExportTaskStatus {
    /// This status as the modelled `ExportTaskStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Initializing => "INITIALIZING",
            Self::Exporting => "EXPORTING",
            Self::Succeeded => "SUCCEEDED",
            Self::Cancelling => "CANCELLING",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Whether this is a TERMINAL status: the task has stopped, so the clock owes it
    /// nothing more and its outcome can no longer change.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Cancelled)
    }

    /// Whether a task in this status can still be cancelled: only one whose export has
    /// not finished — a task waiting to start its run, or running it. A task that has
    /// stopped has no work left to stop, and one already `CANCELLING` is stopping
    /// already — each is a conflict rather than a second cancellation.
    pub const fn is_cancellable(self) -> bool {
        matches!(self, Self::Initializing | Self::Exporting)
    }

    /// The status the clock owes a task currently reported in this status, if any: an
    /// accepted task starts its run, a run in flight finishes, and an accepted
    /// cancellation completes. A terminal status is owed nothing.
    pub const fn settles_into(self) -> Option<Self> {
        match self {
            Self::Initializing => Some(Self::Exporting),
            Self::Exporting => Some(Self::Succeeded),
            Self::Cancelling => Some(Self::Cancelled),
            Self::Succeeded | Self::Cancelled => None,
        }
    }

    /// How long a task stays in this status before the status it settles into
    /// ([`Self::settles_into`]) falls due. `None` for a terminal status, which settles
    /// into nothing.
    ///
    /// Holding the span beside the transition is what keeps one timeline for the whole
    /// progression: each phase's due instant is the previous one's plus this span, so a
    /// task read long after it started lands on the status it would have reached.
    pub const fn settles_in(self) -> Option<Duration> {
        match self {
            Self::Initializing | Self::Exporting => Some(EXPORT_TASK_SPAN),
            Self::Cancelling => Some(CANCELLATION_SPAN),
            Self::Succeeded | Self::Cancelled => None,
        }
    }

    /// The `statusReason` this status carries — why the task stopped early. It is a
    /// property of the status itself, so the two can never be read apart, and a task
    /// that ran to completion carries none: there is nothing to explain.
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Cancelled => Some(EXPORT_CANCELLED_REASON),
            Self::Initializing | Self::Exporting | Self::Succeeded | Self::Cancelling => None,
        }
    }
}

/// One stored export task: every member `GetExportTask` reports, plus the instant at
/// which the next status of its progression becomes due.
///
/// An export task is a resource in its own right — it is addressed by its own
/// `^t-[a-z0-9]{10}$` id and outlives the graph it reads — so it is stored beside the
/// graphs rather than inside the graph record, the way a snapshot is.
#[derive(Debug, Clone)]
pub struct ExportTask {
    pub id: String,
    /// The ARN the tagging surface reads this task's tags back through. Spelled with
    /// the same hyphenated resource segment convention as the other resources this
    /// service mints (`graph/`, `graph-snapshot/`).
    pub arn: String,
    /// The Region the request that started this task was signed for — the same
    /// predicate [`Graph::region`] carries, for the same reason: this is a regional
    /// control plane, so a task of one Region is not visible from another.
    pub region: String,
    /// The graph the export reads, kept as that graph's id (not a borrow) so deleting
    /// the graph leaves the task's provenance intact.
    pub source_graph_id: String,
    pub role_arn: String,
    pub format: String,
    pub parquet_type: Option<String>,
    pub destination: String,
    pub kms_key_identifier: String,
    pub export_filter: Option<ExportFilter>,
    pub status: ExportTaskStatus,
    pub status_reason: Option<String>,
    /// How many of the source graph's vertices this export writes: the vertices the
    /// request's `exportFilter` selects out of the graph's stored nodes, counted when
    /// the task was accepted ([`exported_element_counts`]). An export reads ONE view of
    /// the graph, so the number cannot drift with writes made to the graph after the
    /// export was accepted.
    pub exported_vertices: i64,
    /// How many of the source graph's edges this export writes, counted the same way
    /// and for the same reason as [`Self::exported_vertices`].
    pub exported_edges: i64,
    /// The `progressPercentage` the run had reached when a cancellation stopped it.
    ///
    /// `None` for a task nobody cancelled: only a run that was stopped short has a
    /// progress that stays where it was, and the share of a run still in flight is
    /// derived from the clock instead ([`Self::progress_percentage`]).
    pub cancelled_progress: Option<i32>,
    pub tags: BTreeMap<String, String>,
    /// When the task was accepted: what `ExportTaskDetails$startTime` reports.
    pub start_time: SystemTime,
    /// The monotonic counterpart of [`Self::start_time`], which
    /// `ExportTaskDetails$timeElapsedSeconds` is measured from.
    pub started_at: Instant,
    /// When the next status of this task's progression falls due. `None` once there is
    /// nothing outstanding.
    pub settles_at: Option<Instant>,
}

impl ExportTask {
    /// Whether this record belongs to `region`: the same predicate [`Graph::is_in`]
    /// applies to a graph.
    pub fn is_in(&self, region: &str) -> bool {
        self.region == region
    }

    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the task
    /// would have reached these statuses at these instants whether or not anybody
    /// called, so applying them on read is bookkeeping rather than a state change caused
    /// by the read.
    ///
    /// Which status falls due, and how long after the one before it, are properties of
    /// the status the task is in ([`ExportTaskStatus::settles_into`],
    /// [`ExportTaskStatus::settles_in`]), so an accepted task runs and then completes
    /// while an accepted cancellation settles into `CANCELLED`. Every status the clock
    /// owed by `now` is applied, not just the next one: a task first read long after it
    /// started reports the status it would have reached, never a phase it had already
    /// left.
    pub fn settle(&mut self, now: Instant) {
        while let Some(due) = self.settles_at {
            if now < due {
                return;
            }
            match self.status.settles_into() {
                Some(settled) => {
                    self.status = settled;
                    self.status_reason = settled.reason().map(str::to_string);
                    self.settles_at = settled.settles_in().map(|span| due + span);
                }
                None => self.settles_at = None,
            }
        }
    }

    /// Accept a cancellation of this task: report the modelled `CANCELLING` status now
    /// ("The status is CANCELLING when the export task is cancelled") and let the clock
    /// owe the move to `CANCELLED` [`CANCELLATION_SPAN`] later, so a caller that reads
    /// the task straight after cancelling it still sees it winding down.
    ///
    /// The progress the run had reached is kept as it was at this instant: a stopped run
    /// does no more work, so its share of the work done stops rising here.
    pub fn begin_cancellation(&mut self, now: Instant) {
        self.cancelled_progress = Some(self.progress_percentage(now));
        self.status = ExportTaskStatus::Cancelling;
        self.status_reason = None;
        self.settles_at = ExportTaskStatus::Cancelling
            .settles_in()
            .map(|span| now + span);
    }

    /// How long the task has been running, in whole seconds: what
    /// `ExportTaskDetails$timeElapsedSeconds` reports.
    pub fn elapsed_seconds(&self, now: Instant) -> i64 {
        i64::try_from(now.saturating_duration_since(self.started_at).as_secs())
            .unwrap_or(i64::MAX)
    }

    /// The share of this task's work that is done, as of `now`: what
    /// `ExportTaskDetails$progressPercentage` reports.
    ///
    /// The work is the RUN, so a task that has not begun its run reports none of it
    /// done, a run in flight reports the share of its span that has elapsed — short of
    /// the whole ([`MAX_IN_FLIGHT_PROGRESS_PERCENTAGE`]), because it has not finished —
    /// and a completed run reports the whole of it. A run a cancellation stopped reports
    /// where it had got to ([`Self::begin_cancellation`]).
    pub fn progress_percentage(&self, now: Instant) -> i32 {
        match self.status {
            ExportTaskStatus::Initializing => 0,
            ExportTaskStatus::Exporting => {
                // The run begins one span after the task was accepted, which is when
                // `INITIALIZING` falls due for `EXPORTING`.
                in_flight_progress(
                    now.saturating_duration_since(self.started_at + EXPORT_TASK_SPAN),
                    EXPORT_TASK_SPAN,
                )
            }
            ExportTaskStatus::Succeeded => COMPLETE_PROGRESS_PERCENTAGE,
            ExportTaskStatus::Cancelling | ExportTaskStatus::Cancelled => {
                self.cancelled_progress.unwrap_or(0)
            }
        }
    }

    /// The vertices and edges this task has WRITTEN, if any count can be reported yet.
    ///
    /// `numVerticesWritten` and `numEdgesWritten` are OPTIONAL members
    /// (`smithy_query(query="shape",shape="ExportTaskDetails")`), so a task whose run
    /// has not written the graph's elements — one still initializing or exporting, or
    /// one a cancellation stopped — reports neither, rather than a zero that would claim
    /// a completed run which wrote nothing.
    fn written_elements(&self) -> Option<(i64, i64)> {
        matches!(self.status, ExportTaskStatus::Succeeded)
            .then_some((self.exported_vertices, self.exported_edges))
    }

    /// The task's `ExportTaskDetails`, as of `now`: the elements the export selected off
    /// the stored graph, and the progress the clock has carried the run to.
    pub fn details(&self, now: Instant) -> crate::ops::types::ExportTaskDetails {
        let written = self.written_elements();
        crate::ops::types::ExportTaskDetails {
            start_time: ::aws_smithy_types::DateTime::from(self.start_time),
            time_elapsed_seconds: self.elapsed_seconds(now),
            progress_percentage: self.progress_percentage(now),
            num_vertices_written: written.map(|(vertices, _)| vertices),
            num_edges_written: written.map(|(_, edges)| edges),
        }
    }

    /// This task as `GetExportTask`'s output, as of `now`: the stored record plus the
    /// details derived from the clock, so that read cannot drift from what
    /// `StartExportTask` reported.
    pub fn get_output(&self, now: Instant) -> crate::ops::types::GetExportTaskOutput {
        crate::ops::types::GetExportTaskOutput {
            graph_id: self.source_graph_id.clone(),
            role_arn: self.role_arn.clone(),
            task_id: self.id.clone(),
            status: self.status.as_wire().to_string(),
            format: self.format.clone(),
            destination: self.destination.clone(),
            kms_key_identifier: self.kms_key_identifier.clone(),
            parquet_type: self.parquet_type.clone(),
            status_reason: self.status_reason.clone(),
            export_task_details: Some(self.details(now)),
            export_filter: self.export_filter.clone(),
        }
    }
}

/// The vertices and the edges an export of `data` writes under `filter`.
///
/// "If the ExportFilter is not provided, then all properties for all labels will be
/// exported. If the ExportFilter is provided but is an empty object, then no data will
/// be exported" (`smithy_query(query="shape",shape="ExportFilter")`), and each of that
/// shape's two members is a map keyed by LABEL, "used to specify filters on a per-label
/// basis" for the vertices and for the edges respectively. So: an absent filter selects
/// every element the graph holds; a supplied one selects exactly the elements carrying a
/// label it names, which is why a filter that leaves a map out selects nothing on that
/// side and an empty filter object exports nothing at all. A node carries several labels
/// (openCypher allows it), so a node is selected when ANY of its labels is named.
///
/// The counts are what the export WRITES, so they are taken off the graph's stored data
/// rather than guessed: an export of a graph holding nothing writes nothing, and one of
/// a graph holding elements writes exactly those the filter admits.
pub fn exported_element_counts(data: &GraphData, filter: Option<&ExportFilter>) -> (i64, i64) {
    let Some(filter) = filter else {
        return (
            element_count(data.nodes.len()),
            element_count(data.edges.len()),
        );
    };
    let vertices = filter.vertex_filter.as_ref().map_or(0, |labels| {
        data.nodes
            .values()
            .filter(|node| node.labels.iter().any(|label| labels.contains_key(label)))
            .count()
    });
    let edges = filter.edge_filter.as_ref().map_or(0, |labels| {
        data.edges
            .values()
            .filter(|edge| labels.contains_key(&edge.label))
            .count()
    });
    (element_count(vertices), element_count(edges))
}

/// A stored element count as the modelled `Long` it is reported in. A graph's data is
/// bounded well below `i64::MAX` ([`MAX_NODES_PER_GRAPH`], [`MAX_EDGES_PER_GRAPH`]), so
/// the saturating fallback is unreachable rather than a choice about what to report.
fn element_count(stored: usize) -> i64 {
    i64::try_from(stored).unwrap_or(i64::MAX)
}

/// The `minProvisionedMemory` a request that names none gets, from the member's own
/// documentation: "The minimum provisioned memory-optimized Neptune Capacity Units
/// (m-NCUs) to use for the graph. Default: 16"
/// (`smithy_query(query="doc",shape="CreateGraphUsingImportTaskInput",
/// member="minProvisionedMemory")`).
pub const DEFAULT_MIN_PROVISIONED_MEMORY: i32 = 16;

/// The provisioned memory a graph gets when the request bounds it NEITHER way, from
/// `maxProvisionedMemory`'s own documentation: "If neither value is specified, 128
/// m-NCUs are used" (`smithy_query(query="doc",
/// shape="CreateGraphUsingImportTaskInput",member="maxProvisionedMemory")`).
pub const UNBOUNDED_IMPORT_PROVISIONED_MEMORY: i32 = 128;

/// The provisioned memory a graph created together with an import task gets, from the
/// bounds the request put on it.
///
/// `minProvisionedMemory` and `maxProvisionedMemory` are bounds on a value the service
/// CHOOSES, not the value itself: "If both the minimum and maximum values are
/// specified, the final provisioned-memory will be chosen per the actual size of your
/// imported data. If neither value is specified, 128 m-NCUs are used"
/// (`smithy_query(query="doc",shape="CreateGraphUsingImportTaskInput",
/// member="maxProvisionedMemory")`). So the chosen value is the FLOOR of the range the
/// request allows — the supplied minimum, or [`DEFAULT_MIN_PROVISIONED_MEMORY`] when
/// only a maximum is given — because what would push it above that floor is the size of
/// the imported data, and the data one load of this emulator reads is bounded far below
/// what that floor serves ([`MAX_NODES_PER_GRAPH`], [`MAX_EDGES_PER_GRAPH`]). With
/// neither bound supplied the documented [`UNBOUNDED_IMPORT_PROVISIONED_MEMORY`]
/// stands.
pub fn import_provisioned_memory(min: Option<i32>, max: Option<i32>) -> i32 {
    match (min, max) {
        (Some(min), _) => min,
        (None, Some(_)) => DEFAULT_MIN_PROVISIONED_MEMORY,
        (None, None) => UNBOUNDED_IMPORT_PROVISIONED_MEMORY,
    }
}

/// The states this slice of the emulator puts an in-flight query into.
///
/// Every variant's wire form is one of the modelled `QueryState` values
/// (`crate::ops::types::QUERY_STATE_VALUES`), which is the whole set: the model names
/// no terminal state, because a query that has finished — on its own or through
/// `CancelQuery` — stops being a reportable query at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    /// The query has been accepted but has not started executing yet.
    Waiting,
    /// The query is executing.
    Running,
    /// `CancelQuery` has accepted a cancel; the query is stopping.
    Cancelling,
}

impl QueryState {
    /// This state as the modelled `QueryState` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Waiting => "WAITING",
            Self::Running => "RUNNING",
            Self::Cancelling => "CANCELLING",
        }
    }
}

/// One in-flight query on a graph: what `GetQuery` reports about it, plus the
/// termination the clock owes it once a cancel has been accepted.
///
/// A query is not a resource that outlives its graph: it exists only while it is
/// waiting, running or stopping, so the record carries the graph it runs on — which is
/// also where the store files it — and is dropped both when that graph goes and when
/// the query terminates.
#[derive(Debug, Clone)]
pub struct Query {
    /// The id the query is addressed by. The model puts no pattern on `queryId`
    /// (`smithy_query(query="operation",operation="GetQuery")`: `String`), so whatever
    /// the operation that admitted the query minted is what identifies it here.
    pub id: String,
    /// The graph the query runs on, kept as that graph's id so a query is only ever
    /// found through the graph the request named.
    pub graph_id: String,
    /// The query text, as `GetQuery`'s `queryString` reports it.
    pub query_string: String,
    pub state: QueryState,
    /// When the query was accepted: the start of the span `waited` measures.
    pub submitted_at: Instant,
    /// When the query started executing: the end of the span `waited` measures and the
    /// start of the span `elapsed` measures. `None` while the query is still waiting.
    pub started_at: Option<Instant>,
    /// When the accepted cancellation falls due, after which the query has terminated
    /// and is no longer reported. `None` while nothing has been cancelled.
    pub terminates_at: Option<Instant>,
    /// When the query's own timeout runs out, after which the service has stopped it and
    /// it is no longer reported: [`DEFAULT_QUERY_TIMEOUT`] after its admission, or the
    /// timeout the request that admitted it asked for ([`Self::with_timeout`]). A query
    /// terminates at the EARLIER of this instant and [`Self::terminates_at`], so a cancel
    /// can only shorten its life, never extend it, and a query nobody cancels is reported
    /// for as long as it may run.
    pub times_out_at: Instant,
    /// When the run this record reports is over, after which the record is reclaimed:
    /// [`QUERY_RUN_SPAN`] after the query's admission, or the instant its evaluation
    /// stopped if that is later ([`Self::end_run`]). `None` while the operation that
    /// admitted the query has not reported its run as over.
    pub ends_at: Option<Instant>,
    /// The handle the evaluation of this query is stopped through, shared with the run
    /// itself: accepting a cancel signals it, so a `CancelQuery` ends the work and not
    /// just the record ([`Self::begin_cancellation`]). A record admitted by something
    /// that is not running a query carries a handle nothing listens on.
    pub cancellation: QueryCancellation,
}

impl Query {
    /// A query accepted at `now` and executing from `now`: the state a query admitted
    /// on a graph with capacity for it is in.
    pub fn running(
        id: impl Into<String>,
        graph_id: impl Into<String>,
        query_string: impl Into<String>,
        now: Instant,
    ) -> Self {
        Self {
            id: id.into(),
            graph_id: graph_id.into(),
            query_string: query_string.into(),
            state: QueryState::Running,
            submitted_at: now,
            started_at: Some(now),
            terminates_at: None,
            times_out_at: now + DEFAULT_QUERY_TIMEOUT,
            ends_at: None,
            cancellation: QueryCancellation::new(),
        }
    }

    /// A query accepted at `now` that has not started executing yet.
    pub fn waiting(
        id: impl Into<String>,
        graph_id: impl Into<String>,
        query_string: impl Into<String>,
        now: Instant,
    ) -> Self {
        Self {
            id: id.into(),
            graph_id: graph_id.into(),
            query_string: query_string.into(),
            state: QueryState::Waiting,
            submitted_at: now,
            started_at: None,
            terminates_at: None,
            times_out_at: now + DEFAULT_QUERY_TIMEOUT,
            ends_at: None,
            cancellation: QueryCancellation::new(),
        }
    }

    /// The same query under the timeout `timeout` rather than [`DEFAULT_QUERY_TIMEOUT`]:
    /// what the request's own `queryTimeoutMilliseconds` sets
    /// (`smithy_query(query="operation",operation="ExecuteQuery")`).
    ///
    /// Measured from the query's ADMISSION, which is where its timeout starts running.
    /// The request chooses the span, so the instant is computed with checked arithmetic
    /// and a timeout no clock can reach keeps the documented one: a value a caller sends
    /// may not panic the handler that reads it.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.times_out_at = self
            .submitted_at
            .checked_add(timeout)
            .unwrap_or(self.times_out_at);
        self
    }

    /// Accept a cancel at `now`: the query reports `CANCELLING` and terminates
    /// [`CANCELLATION_SPAN`] later.
    ///
    /// IDEMPOTENT, because `CancelQuery` is (`smithy_query(query="operation",
    /// operation="CancelQuery")`: `@idempotent`): a cancel of a query already stopping
    /// leaves the termination it is already owed where it is, so repeating the request
    /// cannot keep the query alive.
    pub fn begin_cancellation(&mut self, now: Instant) {
        if self.state == QueryState::Cancelling {
            return;
        }
        self.state = QueryState::Cancelling;
        self.terminates_at = Some(now + CANCELLATION_SPAN);
        // The run itself is stopped, not just the record: the evaluation reads this
        // handle at every question it asks the graph and stops asking.
        self.cancellation.signal();
    }

    /// The run this record reports has stopped at `now`: the record is reclaimed once
    /// the span the clock owes an accepted query's run is up ([`QUERY_RUN_SPAN`]), and
    /// once this instant has passed when the span already has.
    ///
    /// Called by the operation that admitted the query, so a record never reports a run
    /// the service has not accepted and never outlives one it is still performing.
    pub fn end_run(&mut self, now: Instant) {
        let owed = self.submitted_at + QUERY_RUN_SPAN;
        self.ends_at = Some(if now > owed { now } else { owed });
    }

    /// Whether this query is over: the end of its run has fallen due ([`Self::ends_at`]),
    /// the cancellation the clock owes it has fallen due, or its own timeout has run out
    /// ([`Self::times_out_at`]). Once it is, the query has stopped and is no longer a
    /// query the service reports.
    pub fn has_terminated(&self, now: Instant) -> bool {
        now >= self.times_out_at
            || self.terminates_at.is_some_and(|due| now >= due)
            || self.ends_at.is_some_and(|due| now >= due)
    }

    /// How long the query waited before executing, in milliseconds: the span from its
    /// acceptance to its start, or to `now` while it is still waiting.
    pub fn waited_millis(&self, now: Instant) -> i32 {
        millis_between(self.submitted_at, self.started_at.unwrap_or(now))
    }

    /// How long the query has been executing, in milliseconds. Zero while it is still
    /// waiting: nothing has run yet.
    pub fn elapsed_millis(&self, now: Instant) -> i32 {
        self.started_at
            .map_or(0, |started| millis_between(started, now))
    }
}

/// The milliseconds from `from` to `to`, saturating into the `Integer` the model
/// reports `waited` and `elapsed` as. Zero when `to` precedes `from`, which
/// [`Instant`] arithmetic makes unreachable here but which no caller has to reason
/// about.
fn millis_between(from: Instant, to: Instant) -> i32 {
    i32::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(i32::MAX)
}

/// The lifecycle statuses this slice of the emulator puts a private graph endpoint
/// into.
///
/// Every variant's wire form is one of the modelled `PrivateGraphEndpointStatus`
/// values (`crate::ops::types::PRIVATE_GRAPH_ENDPOINT_STATUS_VALUES`). `FAILED` is
/// deliberately never produced: nothing here can fail to attach an endpoint, so no
/// endpoint may report a failure that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateGraphEndpointStatus {
    /// `CreatePrivateGraphEndpoint` has accepted the endpoint; it is not reachable
    /// yet.
    Creating,
    /// The endpoint is provisioned: the graph is reachable through it from the VPC.
    Available,
    /// `DeletePrivateGraphEndpoint` has accepted the deletion; the endpoint is on its
    /// way out and is reclaimed once the clock makes its removal due.
    Deleting,
}

impl PrivateGraphEndpointStatus {
    /// This status as the modelled `PrivateGraphEndpointStatus` enum value.
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
        }
    }
}

/// One stored private graph endpoint: every member `GetPrivateGraphEndpoint` reports,
/// the graph and VPC the pair of identifiers it is addressed by names, the security
/// groups it was created with, and the instant at which the transition the clock owes
/// it falls due.
///
/// An endpoint belongs to ONE graph in ONE VPC — `GetPrivateGraphEndpoint` and
/// `DeletePrivateGraphEndpoint` address it by `(graphIdentifier, vpcId)` and nothing
/// else — so that pair is its identity and the minted `vpcEndpointId` is a property of
/// the record rather than a second key.
#[derive(Debug, Clone)]
pub struct PrivateGraphEndpoint {
    /// The graph the endpoint reaches. Kept as the graph's id, so the endpoint is
    /// reclaimed with the graph it belongs to.
    pub graph_id: String,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    /// The security groups the request attached to the endpoint. Stored because they
    /// are part of the record the service accepted, even though no modelled output
    /// member reports them back.
    pub security_group_ids: Vec<String>,
    /// The minted `^vpce-[0-9a-f]{17}$` id of the VPC endpoint behind this record.
    pub vpc_endpoint_id: String,
    pub status: PrivateGraphEndpointStatus,
    /// When the transition the clock owes this endpoint falls due: the move from
    /// `CREATING` to `AVAILABLE`, or the removal of a `DELETING` endpoint. `None` once
    /// the endpoint rests `AVAILABLE`, which is the only status nothing is owed out of.
    pub due: Option<Instant>,
}

impl PrivateGraphEndpoint {
    /// Materialise the transition the clock has already made due.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// endpoint would have become `AVAILABLE` at `due` whether or not anybody called,
    /// so applying it on read is bookkeeping rather than a state change caused by the
    /// read. A `DELETING` endpoint is not settled here — it is RECLAIMED, by
    /// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_private_endpoints`],
    /// since the record itself goes away.
    pub fn settle(&mut self, now: Instant) {
        if self.status == PrivateGraphEndpointStatus::Creating
            && self.due.is_some_and(|due| now >= due)
        {
            self.status = PrivateGraphEndpointStatus::Available;
            self.due = None;
        }
    }

    /// Whether the accepted deletion of this endpoint has fallen due, so the record is
    /// gone and no read may still report it.
    pub fn is_reclaimed(&self, now: Instant) -> bool {
        self.status == PrivateGraphEndpointStatus::Deleting
            && self.due.is_some_and(|due| now >= due)
    }

    /// Accept this endpoint's deletion: it answers `DELETING` until
    /// [`ENDPOINT_DELETION_SPAN`] has passed, after which it is reclaimed.
    pub fn begin_deletion(&mut self, now: Instant) {
        self.status = PrivateGraphEndpointStatus::Deleting;
        self.due = Some(now + ENDPOINT_DELETION_SPAN);
    }
}

/// Per-service state for com-amazonaws-neptunegraph-amazonneptunegraph. A handler reaches it through [`service_state`].
///
/// One store serves every Region, and every record carries the Region it belongs to
/// ([`Graph::region`], [`Snapshot::region`]), so each lookup, listing and
/// name-uniqueness check below takes the Region of the current request beside the
/// identifier: this is a regional control plane, and a resource of one Region is not
/// visible from another.
#[derive(Debug, Default)]
pub struct ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Every stored graph, keyed by its minted `g-xxxxxxxxxx` id. A `BTreeMap` so
    /// `ListGraphs` has one stable total order to page over. Ids are minted unique
    /// across the whole store, so the key needs no Region in it — the Region is the
    /// predicate a lookup applies to the record it finds.
    pub graphs: BTreeMap<String, Graph>,
    /// Every stored snapshot, keyed by its minted `gs-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListGraphSnapshots` has one stable total order to page over.
    pub snapshots: BTreeMap<String, Snapshot>,
    /// Every stored import task, keyed by its minted `t-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListImportTasks` has one stable total order to page over.
    pub import_tasks: BTreeMap<String, ImportTask>,
    /// Every in-flight query, keyed by the graph it runs on and then by its own id. A
    /// `BTreeMap` of `BTreeMap`s so `ListQueries` — which lists the queries of ONE
    /// graph — has one stable total order to report, and so a query id is resolved
    /// within the graph that admitted it: the query namespace is per graph, and the
    /// same id on another graph names nothing. A query is admitted by the operation
    /// that runs it and leaves this map when it terminates, so the map holds exactly
    /// the queries `GetQuery`, `ListQueries` and `CancelQuery` can still name. The
    /// graph the queries hang off is itself resolved per Region, so a query is reached
    /// only from the Region that holds its graph.
    pub queries: BTreeMap<String, BTreeMap<String, Query>>,
    /// Every stored export task, keyed by its minted `t-xxxxxxxxxx` id. A `BTreeMap`
    /// so `ListExportTasks` has one stable total order to page over.
    pub export_tasks: BTreeMap<String, ExportTask>,
    /// Every stored private graph endpoint, keyed by the graph it reaches and then by
    /// the VPC it lives in. A `BTreeMap` of `BTreeMap`s so `ListPrivateGraphEndpoints`
    /// — which lists the endpoints of ONE graph — has one stable total order to page
    /// over, and so an endpoint is resolved by the `(graphIdentifier, vpcId)` pair it
    /// is addressed by: the same VPC id against another graph names another endpoint.
    pub private_endpoints: BTreeMap<String, BTreeMap<String, PrivateGraphEndpoint>>,
    /// Feeds the graph-id generator, so ids are unique within a store.
    pub id_counter: u64,
    /// Feeds the snapshot-id generator, so ids are unique within a store.
    pub snapshot_id_counter: u64,
    /// Feeds the import-task-id generator, so ids are unique within a store.
    pub import_task_id_counter: u64,
    /// Feeds the export-task-id generator, so ids are unique within a store.
    pub export_task_id_counter: u64,
    /// Feeds the VPC-endpoint-id generator, so ids are unique within a store.
    pub vpc_endpoint_id_counter: u64,
    /// Feeds the query-id generator, so ids are unique within a store.
    pub query_id_counter: u64,
}

impl ComAmazonawsNeptunegraphAmazonneptunegraphState {
    /// Mint an id no graph in this store holds, matching the modelled `GraphId`
    /// pattern `^g-[a-z0-9]{10}$`.
    pub fn next_graph_id(&mut self) -> String {
        loop {
            self.id_counter = self.id_counter.wrapping_add(1);
            let id = encode_resource_id("g-", scramble(self.id_counter) % BASE36_POW10);
            if !self.graphs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an id no snapshot in this store holds, matching the modelled
    /// `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
    pub fn next_snapshot_id(&mut self) -> String {
        loop {
            self.snapshot_id_counter = self.snapshot_id_counter.wrapping_add(1);
            let id = encode_resource_id("gs-", scramble(self.snapshot_id_counter) % BASE36_POW10);
            if !self.snapshots.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an id no task in this store holds, matching the modelled `TaskId`
    /// pattern `^t-[a-z0-9]{10}$`.
    pub fn next_import_task_id(&mut self) -> String {
        loop {
            self.import_task_id_counter = self.import_task_id_counter.wrapping_add(1);
            let id = encode_resource_id(
                TASK_ID_PREFIX,
                scramble(self.import_task_id_counter) % BASE36_POW10,
            );
            if !self.holds_task_id(&id) {
                return id;
            }
        }
    }

    /// Mint an id no query this store holds in flight is addressed by.
    ///
    /// The model puts no pattern on `queryId` (`smithy_query(query="operation",
    /// operation="GetQuery")`: a bare `String`), so the id is spelled the way the service
    /// spells a query's: the 8-4-4-4-12 hexadecimal form, scrambled off the counter the
    /// same way every other id of this store is, so ids are not guessable neighbours
    /// while still being a pure function of the counter. Queries are keyed per graph, so
    /// the uniqueness checked here is across every graph: one id names at most one query
    /// in this store, whichever graph a request addresses it on.
    pub fn next_query_id(&mut self) -> String {
        loop {
            self.query_id_counter = self.query_id_counter.wrapping_add(1);
            let id = encode_query_id(self.query_id_counter);
            if !self.queries.values().any(|queries| queries.contains_key(&id)) {
                return id;
            }
        }
    }

    /// Whether this store already holds a task under `id`, in EITHER task map.
    ///
    /// Import and export tasks share ONE modelled id space (`TaskId`'s
    /// `^t-[a-z0-9]{10}$`), so an id minted for one must not be an id the other
    /// already holds: two resources would otherwise answer to the same identifier,
    /// each through its own read.
    pub fn holds_task_id(&self, id: &str) -> bool {
        self.import_tasks.contains_key(id) || self.export_tasks.contains_key(id)
    }

    /// Store a new graph.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graphs.insert(graph.id.clone(), graph);
    }

    /// Mint an id no task in this store holds, matching the modelled
    /// `ExportTaskId` pattern `^t-[a-z0-9]{10}$`.
    ///
    /// The task-id space is shared with the import tasks, so an id either map already
    /// holds is skipped (see [`Self::holds_task_id`]).
    pub fn next_export_task_id(&mut self) -> String {
        loop {
            self.export_task_id_counter = self.export_task_id_counter.wrapping_add(1);
            let id = encode_resource_id(
                EXPORT_TASK_ID_PREFIX,
                scramble(self.export_task_id_counter) % BASE36_POW10,
            );
            if !self.holds_task_id(&id) {
                return id;
            }
        }
    }

    /// Store a new export task.
    pub fn insert_export_task(&mut self, task: ExportTask) {
        self.export_tasks.insert(task.id.clone(), task);
    }

    /// Apply the transitions the clock has made due, then borrow the export
    /// task `id` names IN `region` — `None` when this Region holds no such task.
    pub fn settled_export_task(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&ExportTask> {
        self.settled_export_task_mut(id, region, now).map(|t| &*t)
    }

    /// [`Self::settled_export_task`], mutably.
    pub fn settled_export_task_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut ExportTask> {
        let task = self.export_tasks.get_mut(id)?;
        if !task.is_in(region) {
            return None;
        }
        task.settle(now);
        Some(task)
    }

    /// The export tasks `region` holds, in task-id order, resuming after `after_id`
    /// when a pagination token named a position, and restricted to the tasks that read
    /// one graph when `of_graph` names one: the listing `ListExportTasks` pages over.
    ///
    /// The same shape as [`Self::snapshots_in`], for the same reason: `export_tasks` is
    /// a `BTreeMap`, so one stable total order underlies every page and a token can name
    /// a position in it. `of_graph` is matched against the task's stored
    /// `source_graph_id`, so a task stays listed under the graph it read even once that
    /// graph is gone.
    pub fn export_tasks_in<'store>(
        &'store self,
        region: &'store str,
        of_graph: Option<&'store str>,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store ExportTask> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.export_tasks
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, task)| task)
            .filter(move |task| {
                task.is_in(region)
                    && of_graph.is_none_or(|graph_id| task.source_graph_id == graph_id)
            })
    }

    /// A free `snapshotName` for the FINAL snapshot of the graph named `graph_name`
    /// IN `region`, satisfying the modelled `SnapshotName` constraints
    /// (`@length(1..63)`, pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`).
    ///
    /// The request that asks for a final snapshot carries no name, so the service names
    /// it: after the graph it is taken of, which is how a caller finds it again. A name
    /// the modelled `SnapshotName` constraints would not accept, or that the account
    /// already holds IN THAT REGION, falls back to the snapshot's own freshly minted
    /// id, which no stored snapshot shares. Names are per Region, so a name another
    /// Region holds is free here.
    ///
    /// A name this service MINTS is held to the same constraints it enforces on a name a
    /// caller sends, so the candidate goes through [`resource_name_is_well_formed`] —
    /// the one place that pattern is enforced — and not through its length clause alone.
    /// A `graphName` satisfies `GraphName`, which reserves a DIFFERENT prefix (`g-`), so
    /// a graph may legitimately be named `gs…`; the name built from such a graph opens
    /// with the prefix `SnapshotName` reserves and is therefore not a snapshot name, and
    /// the fallback is taken instead. The fallback opens with its own fixed stem, so it
    /// satisfies the pattern whatever the graph is called.
    pub fn free_final_snapshot_name(
        &self,
        graph_name: &str,
        snapshot_id: &str,
        region: &str,
    ) -> String {
        let after_the_graph = format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}");
        if after_the_graph.len() <= MAX_RESOURCE_NAME_CHARS
            && resource_name_is_well_formed(&after_the_graph, SNAPSHOT_NAME_RESERVED_PREFIX)
            && self.snapshot_id_named(&after_the_graph, region).is_none()
        {
            return after_the_graph;
        }
        let mut from_the_id = format!("{FINAL_SNAPSHOT_STEM}-{snapshot_id}");
        let mut attempt = 1u32;
        while self.snapshot_id_named(&from_the_id, region).is_some() {
            attempt += 1;
            from_the_id = format!("{FINAL_SNAPSHOT_STEM}-{snapshot_id}-{attempt}");
        }
        from_the_id
    }

    /// Store a new snapshot.
    pub fn insert_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshots.insert(snapshot.id.clone(), snapshot);
    }

    /// Take the FINAL snapshot of `graph` that a request whose required `skipSnapshot`
    /// is `false` asks for, and return the id it was minted under — or the reason the
    /// store had no room for it.
    ///
    /// ONE rule, stated once: a final snapshot captures the graph AS THE CALL FOUND IT
    /// — the data it holds and the configuration it is provisioned with, under the
    /// graph's own encryption key — and is not restorable until the clock makes its
    /// move to `AVAILABLE` due, exactly as an explicitly named snapshot is not. Every
    /// operation that empties or removes a graph's data takes the same snapshot, so
    /// neither what it captures, nor the span it settles over, nor the wording of the
    /// no-room refusal can differ between them. The Err is the refusal's MESSAGE: each
    /// operation reports it through its own modelled error channel, which is all that
    /// is left to the caller of this helper.
    ///
    /// The graph is passed in rather than looked up here, because the caller has
    /// already resolved it (in its own Region, with the transitions the clock owes it
    /// settled) and gated the call on the status it was found in.
    pub fn mint_final_snapshot(
        &mut self,
        graph: &Graph,
        region: &str,
        now: Instant,
    ) -> Result<String, String> {
        // The name a snapshot reserves is released, and the room it takes given back,
        // when its deletion completes, so the removals the clock has already made due
        // are materialised before the ceiling and the name are checked against what the
        // store holds.
        self.materialise_snapshots(now);
        // The snapshot store grows by one entry per final snapshot, so it has the same
        // ceiling `CreateGraphSnapshot` enforces. Neither operation that takes a final
        // snapshot models a quota error, so a store with no room for the snapshot the
        // caller asked for is reported as the conflict it is: the request cannot be
        // carried out against what the store already holds.
        if self.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(format!(
                "a final snapshot of graph {} cannot be taken because {}; delete a \
                 snapshot or pass skipSnapshot=true",
                graph.id,
                store_ceiling_clause("graph snapshots", MAX_SNAPSHOTS)
            ));
        }
        // The request carries no name for the snapshot, so the service names it after
        // the graph it is taken of, the way it mints the id.
        let id = self.next_snapshot_id();
        let name = self.free_final_snapshot_name(&graph.name, &id, region);
        self.insert_snapshot(Snapshot {
            arn: ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("graph-snapshot/{id}")),
            id: id.clone(),
            name,
            // The final snapshot lives in the Region of the graph it was taken of,
            // which is the Region the asking request was signed for.
            region: region.to_string(),
            source_graph_id: graph.id.clone(),
            // Not restorable when the call returns; the clock owes the move to
            // `AVAILABLE` a fixed span later.
            status: SnapshotStatus::Creating,
            settles_at: Some(now + PROVISIONING_SPAN),
            snapshot_create_time: SystemTime::now(),
            // A snapshot of an encrypted graph is encrypted with the graph's own key:
            // the key belongs to the data being snapshotted, not to a request that
            // carries no key member at all.
            kms_key_identifier: graph.kms_key_identifier.clone(),
            // No request that asks for a final snapshot carries tags, and a snapshot's
            // tags are its own: the graph's tag set is not copied onto it.
            tags: BTreeMap::new(),
            source_configuration: GraphConfiguration::from(graph),
            data: graph.data.clone(),
        });
        Ok(id)
    }

    /// Store a new import task. Tasks are keyed by their minted id alone: the model
    /// gives a task no name, so there is no second index to keep in step.
    pub fn insert_import_task(&mut self, task: ImportTask) {
        self.import_tasks.insert(task.id.clone(), task);
    }

    /// Drop the graph `id` names IN `region`, leaving another Region's graph of the
    /// same id (and any graph of the same NAME in another Region) in place.
    ///
    /// Every query that was running on it goes too, and every private endpoint that
    /// reached it: a query exists only while the graph it runs on does, so a deleted
    /// graph takes its in-flight queries with it rather than leaving entries a later
    /// `GetQuery` on a re-minted id could find. A private graph endpoint is the same:
    /// it is the private route INTO one graph, addressed as an endpoint of that graph,
    /// so it cannot outlive it. An import task is NOT dropped with its graph: it is
    /// read by its own id and keeps the provenance of the load it describes.
    pub fn remove_graph(&mut self, id: &str, region: &str) -> Option<Graph> {
        if !self.graphs.get(id).is_some_and(|graph| graph.is_in(region)) {
            return None;
        }
        let graph = self.graphs.remove(id)?;
        self.queries.remove(id);
        self.private_endpoints.remove(id);
        Some(graph)
    }

    /// The id of the graph named `name` in `region`, or `None` when that Region holds
    /// no graph by that name: the uniqueness check behind `CreateGraph`'s
    /// `ConflictException`.
    ///
    /// Graph names are unique per account PER REGION, so the same name in two Regions
    /// is two graphs. The record itself carries its Region and its name, so this one
    /// scan over a bounded map ([`MAX_GRAPHS`]) is the whole index — a second map
    /// keyed by name could disagree with it.
    pub fn graph_id_named(&self, name: &str, region: &str) -> Option<&str> {
        self.graphs
            .values()
            .find(|graph| graph.is_in(region) && graph.name == name)
            .map(|graph| graph.id.as_str())
    }

    /// Mint a VPC endpoint id no endpoint in this store holds, matching the modelled
    /// `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`.
    pub fn next_vpc_endpoint_id(&mut self) -> String {
        loop {
            self.vpc_endpoint_id_counter = self.vpc_endpoint_id_counter.wrapping_add(1);
            let id = encode_vpc_endpoint_id(self.vpc_endpoint_id_counter);
            if !self.holds_vpc_endpoint_id(&id) {
                return id;
            }
        }
    }

    /// Whether this store already holds a private endpoint under `vpc_endpoint_id`, on
    /// any graph: the id names one VPC endpoint across the whole store.
    pub fn holds_vpc_endpoint_id(&self, vpc_endpoint_id: &str) -> bool {
        self.private_endpoints
            .values()
            .flat_map(BTreeMap::values)
            .any(|endpoint| endpoint.vpc_endpoint_id == vpc_endpoint_id)
    }

    /// Store a newly accepted private graph endpoint, under the graph it reaches.
    ///
    /// Replaces nothing: the growth site refuses a second endpoint in the same VPC
    /// before reaching here, because `(graphIdentifier, vpcId)` is the identity the
    /// read and the delete address an endpoint by.
    pub fn insert_private_endpoint(&mut self, endpoint: PrivateGraphEndpoint) {
        self.private_endpoints
            .entry(endpoint.graph_id.clone())
            .or_default()
            .insert(endpoint.vpc_id.clone(), endpoint);
    }

    /// How many private graph endpoints this store holds, across every graph.
    pub fn private_endpoint_count(&self) -> usize {
        self.private_endpoints.values().map(BTreeMap::len).sum()
    }

    /// Materialise every private-endpoint transition the clock has already made due:
    /// each accepted deletion whose span has passed is reclaimed, and each endpoint
    /// whose provisioning has come due settles to `AVAILABLE`.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// these was owed at that instant whether or not anybody called.
    pub fn materialise_private_endpoints(&mut self, now: Instant) {
        for endpoints in self.private_endpoints.values_mut() {
            endpoints.retain(|_, endpoint| !endpoint.is_reclaimed(now));
            for endpoint in endpoints.values_mut() {
                endpoint.settle(now);
            }
        }
        // A graph with no endpoints left holds no entry, so the outer map does not grow
        // one bucket per graph that ever had one.
        self.private_endpoints.retain(|_, endpoints| !endpoints.is_empty());
    }

    /// Materialise what the clock owes, then borrow the one private endpoint the
    /// `(graph_id, vpc_id)` pair names.
    pub fn settled_private_endpoint(
        &mut self,
        graph_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> Option<&PrivateGraphEndpoint> {
        self.materialise_private_endpoints(now);
        self.private_endpoints.get(graph_id)?.get(vpc_id)
    }

    /// [`Self::settled_private_endpoint`], mutably: what `DeletePrivateGraphEndpoint`
    /// writes the accepted deletion through.
    pub fn settled_private_endpoint_mut(
        &mut self,
        graph_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> Option<&mut PrivateGraphEndpoint> {
        self.materialise_private_endpoints(now);
        self.private_endpoints.get_mut(graph_id)?.get_mut(vpc_id)
    }

    /// The private endpoints of ONE graph, in VPC-id order, resuming after
    /// `after_vpc_id` when a pagination token named a position: the listing
    /// `ListPrivateGraphEndpoints` pages over.
    ///
    /// The same shape as [`Self::graphs_in`] and [`Self::snapshots_in`], for the same
    /// reason: the endpoints of a graph are held in a `BTreeMap` keyed by the VPC id an
    /// endpoint is addressed by, so one stable total order underlies every page and a
    /// token can name a position in it. A graph holding no endpoint at all yields
    /// nothing rather than being distinguished from one whose endpoints are all gone.
    pub fn private_endpoints_of<'store>(
        &'store self,
        graph_id: &str,
        after_vpc_id: Option<&str>,
    ) -> impl Iterator<Item = &'store PrivateGraphEndpoint> {
        let lower = match after_vpc_id {
            Some(vpc_id) => ::std::ops::Bound::Excluded(vpc_id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.private_endpoints
            .get(graph_id)
            .map(|endpoints| endpoints.range((lower, ::std::ops::Bound::Unbounded)))
            .into_iter()
            .flatten()
            .map(|(_, endpoint)| endpoint)
    }

    /// Store a newly accepted query, under the graph it runs on.
    ///
    /// Fallible at the GROWTH SITE, the way the graph and snapshot caps are enforced by
    /// the operations that mint those: the query map grows by one entry per admitted
    /// query in a store shared process-wide, so an admitting operation has to be told
    /// when the store is full ([`MAX_QUERIES`]) or the text is longer than one record
    /// may carry ([`MAX_QUERY_STRING_CHARS`]) rather than growing the emulator's memory
    /// without bound. `Err` carries the message, naming the bound it broke.
    ///
    /// Replacing a query already filed under the same id on the same graph is not
    /// growth, so it is not refused by the count bound.
    pub fn insert_query(&mut self, query: Query) -> Result<(), String> {
        if query.query_string.chars().count() > MAX_QUERY_STRING_CHARS {
            return Err(format!(
                "queryString may carry at most {MAX_QUERY_STRING_CHARS} characters, got {}",
                query.query_string.chars().count()
            ));
        }
        let replaces_an_entry = self
            .queries
            .get(&query.graph_id)
            .is_some_and(|queries| queries.contains_key(&query.id));
        if !replaces_an_entry && self.query_count() >= MAX_QUERIES {
            return Err(store_ceiling_clause("queries in flight", MAX_QUERIES));
        }
        self.queries
            .entry(query.graph_id.clone())
            .or_default()
            .insert(query.id.clone(), query);
        Ok(())
    }

    /// How many queries this store holds in flight, across every graph.
    pub fn query_count(&self) -> usize {
        self.queries.values().map(BTreeMap::len).sum()
    }

    /// Drop every query the clock has made due: each one whose accepted cancellation
    /// has fallen due, and each one whose own timeout has run out ([`Query::times_out_at`]).
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// those queries was over at that instant whether or not anybody called, so
    /// reclaiming it on read is bookkeeping rather than a state change caused by the
    /// read.
    pub fn reap_terminated_queries(&mut self, now: Instant) {
        for queries in self.queries.values_mut() {
            queries.retain(|_, query| !query.has_terminated(now));
        }
        // A graph with no queries left holds no entry, so the outer map does not grow
        // one bucket per graph that ever ran a query.
        self.queries.retain(|_, queries| !queries.is_empty());
    }

    /// Apply every query termination the clock has made due, then borrow the one query
    /// `query_id` names ON the graph `graph_id` names.
    ///
    /// The lookup is scoped to the graph because a query is addressed as a query of one
    /// graph: the same id against another graph names nothing. The graph itself is
    /// resolved per Region by the caller ([`Self::settled_graph`]), which is what keeps
    /// a query of one Region's graph unreachable from another.
    pub fn live_query(&mut self, graph_id: &str, query_id: &str, now: Instant) -> Option<&Query> {
        self.reap_terminated_queries(now);
        self.queries.get(graph_id)?.get(query_id)
    }

    /// [`Self::live_query`], mutably: what `CancelQuery` writes the accepted
    /// cancellation through.
    pub fn live_query_mut(
        &mut self,
        graph_id: &str,
        query_id: &str,
        now: Instant,
    ) -> Option<&mut Query> {
        self.reap_terminated_queries(now);
        self.queries.get_mut(graph_id)?.get_mut(query_id)
    }

    /// The queries in flight on ONE graph, oldest admission first, after applying every
    /// termination the clock has made due: the listing `ListQueries` reports.
    ///
    /// Admission order is the order the listing reports, because it is the order the work
    /// arrived in: the query that has been in flight longest comes first, so a caller
    /// that asks for fewer results than there are queries is told about the oldest rather
    /// than an arbitrary slice of them. `queries` is keyed by query id, which is what
    /// breaks a tie between two queries admitted at the same instant and keeps the order
    /// total. A graph holding no query at all yields nothing rather than being
    /// distinguished from one whose queries have all terminated.
    pub fn live_queries_on(&mut self, graph_id: &str, now: Instant) -> Vec<&Query> {
        self.reap_terminated_queries(now);
        let mut in_flight: Vec<&Query> = self
            .queries
            .get(graph_id)
            .into_iter()
            .flat_map(BTreeMap::values)
            .collect();
        in_flight.sort_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        in_flight
    }

    /// The id of the snapshot named `name` in `region`, or `None` when that Region
    /// holds no snapshot by that name. Snapshot names are unique per account per
    /// Region, exactly as graph names are ([`Self::graph_id_named`]).
    pub fn snapshot_id_named(&self, name: &str, region: &str) -> Option<&str> {
        self.snapshots
            .values()
            .find(|snapshot| snapshot.is_in(region) && snapshot.name == name)
            .map(|snapshot| snapshot.id.as_str())
    }

    /// Drop every graph whose accepted deletion the clock has made due, along with
    /// everything [`Self::remove_graph`] reclaims with a graph.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// those graphs was gone at its `deletes_at` instant whether or not anybody called,
    /// so reclaiming it on read is bookkeeping rather than a state change caused by the
    /// read. Each graph is reclaimed in its OWN Region — the Region the record carries
    /// — so this is the one lookup that is not scoped to the caller's Region: the clock
    /// owes it in every Region at once.
    pub fn reap_deleted_graphs(&mut self, now: Instant) {
        let due: Vec<(String, String)> = self
            .graphs
            .iter()
            .filter(|(_, graph)| graph.deletion_complete(now))
            .map(|(id, graph)| (id.clone(), graph.region.clone()))
            .collect();
        for (id, region) in due {
            self.remove_graph(&id, &region);
        }
    }

    /// Apply every provisioning transition the clock has made due, then borrow the
    /// graph `id` names IN `region` — `None` when this Region holds no such graph,
    /// whatever another Region holds.
    pub fn settled_graph(&mut self, id: &str, region: &str, now: Instant) -> Option<&Graph> {
        self.settled_graph_mut(id, region, now).map(|graph| &*graph)
    }

    /// Apply every provisioning transition the clock has made due, then borrow the
    /// graph `id` names in `region` mutably.
    pub fn settled_graph_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut Graph> {
        self.reap_deleted_graphs(now);
        // A graph whose import has come due holds what that load wrote, so the write
        // is materialised before the graph is read: the data landed at that instant
        // whether or not anybody read the graph.
        self.materialise_import_loads(now);
        let graph = self.graphs.get_mut(id)?;
        if !graph.is_in(region) {
            return None;
        }
        graph.settle(now);
        Some(graph)
    }

    /// The graph id a PER-GRAPH LISTING of records that outlive their graph resolves
    /// `identifier` to IN `region`, or `None` when this Region holds nothing under it.
    ///
    /// A listing of the records TAKEN FROM a graph is not a read OF that graph. A
    /// snapshot, an import task and an export task each store a COPY of their graph's
    /// id ([`Snapshot::source_graph_id`], [`ImportTask::graph_id`],
    /// [`ExportTask::source_graph_id`]) precisely so the record — and the provenance it
    /// reports — survives the graph, and [`Self::remove_graph`] reclaims none of them.
    /// So the identifier is resolved against everything this Region still holds under
    /// it: the graph record while it exists, and once that record is reclaimed the
    /// retained records whose provenance names it. Only an identifier that resolves to
    /// neither names no resource at all, which is the failed lookup the modelled
    /// `ResourceNotFoundException` reports; a graph that exists and holds no such record
    /// resolves fine, to an empty page.
    ///
    /// Whether this Region knows an id is ONE fact, not one per record kind, so all
    /// three listings resolve it here and cannot disagree about it. A query and a
    /// private graph endpoint are NOT retained records — [`Self::remove_graph`] takes
    /// them with the graph — so the listings that page over those resolve their graph
    /// through [`Self::settled_graph`] instead.
    pub fn listable_graph_id(
        &mut self,
        identifier: &str,
        region: &str,
        now: Instant,
    ) -> Option<String> {
        if let Some(graph) = self.settled_graph(identifier, region, now) {
            return Some(graph.id.clone());
        }
        let retained = self
            .snapshots_in(region, Some(identifier), None)
            .next()
            .is_some()
            || self
                .import_tasks_in(region, Some(identifier), None)
                .next()
                .is_some()
            || self
                .export_tasks_in(region, Some(identifier), None)
                .next()
                .is_some();
        retained.then(|| identifier.to_string())
    }

    /// Apply every provisioning transition the clock has made due across the store.
    pub fn settle_all(&mut self, now: Instant) {
        self.reap_deleted_graphs(now);
        for graph in self.graphs.values_mut() {
            graph.settle(now);
        }
        self.materialise_snapshots(now);
        self.materialise_import_loads(now);
        for task in self.export_tasks.values_mut() {
            task.settle(now);
        }
        self.materialise_private_endpoints(now);
    }

    /// Materialise every import-task status the clock has already made due, and write
    /// the data of each load whose write has come with it.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: the
    /// task reached that status, and its load landed in the graph, at those instants
    /// whether or not anybody called — so applying them on read is bookkeeping rather
    /// than a state change caused by the read. The load is delivered exactly once
    /// ([`ImportTask::delivered`]) and in the Region of the task that read it, so a
    /// caller of another Region neither sees nor delays it.
    ///
    /// The elements are stored through the graph's OWN writers, which is where an
    /// accepted openCypher write puts a graph's data, so an imported element and a
    /// written one are the same kind of record. The load was bounded against the
    /// graph's ceilings when it was read and the graph it loads is empty and
    /// `IMPORTING` until the write lands, so a refusal here is the backstop for a
    /// graph something else wrote to meanwhile: it is counted as an error of the run
    /// rather than dropped silently.
    pub fn materialise_import_loads(&mut self, now: Instant) {
        for task in self.import_tasks.values_mut() {
            task.settle(now);
        }
        let due: Vec<(String, String, ImportedData, SystemTime)> = self
            .import_tasks
            .values_mut()
            .filter(|task| task.status == ImportTaskStatus::Succeeded && !task.delivered)
            .map(|task| {
                task.delivered = true;
                (
                    task.id.clone(),
                    task.graph_id.clone(),
                    ::std::mem::take(&mut task.load.data),
                    task.load_landed_at(),
                )
            })
            .collect();
        for (task_id, graph_id, data, as_of) in due {
            let mut refused: Vec<String> = Vec::new();
            if let Some(graph) = self.graphs.get_mut(&graph_id) {
                for (id, node) in data.nodes {
                    if let Err(reason) = graph.data.insert_node(&id, node, as_of) {
                        refused.push(reason);
                    }
                }
                for (id, edge) in data.edges {
                    if let Err(reason) = graph.data.insert_edge(&id, edge, as_of) {
                        refused.push(reason);
                    }
                }
            } else {
                // The graph was deleted while its load ran, so there is nothing left
                // to load into: the elements are released with it.
                refused.push("The graph the import task loads was deleted.".to_string());
            }
            if refused.is_empty() {
                continue;
            }
            if let Some(task) = self.import_tasks.get_mut(&task_id) {
                task.load.error_count = task
                    .load
                    .error_count
                    .saturating_add(i32::try_from(refused.len()).unwrap_or(i32::MAX));
                if task.load.error_details.is_none() {
                    task.load.error_details = refused.into_iter().next();
                }
            }
        }
    }

    /// The graphs `region` holds, in graph-id order, resuming after `after_id` when a
    /// pagination token named a position: the one listing `ListGraphs` pages over.
    pub fn graphs_in<'store>(
        &'store self,
        region: &'store str,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store Graph> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.graphs
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, graph)| graph)
            .filter(move |graph| graph.is_in(region))
    }

    /// The snapshots `region` holds, in snapshot-id order, resuming after `after_id`
    /// when a pagination token named a position, and restricted to the snapshots taken
    /// from one graph when `of_graph` names one: the listing `ListGraphSnapshots` pages
    /// over.
    ///
    /// The same shape as [`Self::graphs_in`], for the same reason: `snapshots` is a
    /// `BTreeMap`, so one stable total order underlies every page and a token can name
    /// a position in it. `of_graph` is matched against the snapshot's stored
    /// `source_graph_id`, so a snapshot stays listed under the graph it was taken from
    /// even once that graph is gone.
    pub fn snapshots_in<'store>(
        &'store self,
        region: &'store str,
        of_graph: Option<&'store str>,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store Snapshot> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.snapshots
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, snapshot)| snapshot)
            .filter(move |snapshot| {
                snapshot.is_in(region)
                    && of_graph.is_none_or(|graph_id| snapshot.source_graph_id == graph_id)
            })
    }

    /// Materialise every snapshot transition the clock has already made due: each
    /// accepted deletion whose span has passed is reclaimed, and each snapshot whose
    /// creation has come due settles to `AVAILABLE`.
    ///
    /// The same one write a reporting operation may make as [`Graph::settle`]: each of
    /// these was owed at that instant whether or not anybody called. Each snapshot is
    /// reclaimed whatever Region it belongs to — the clock owes it in every Region at
    /// once, exactly as [`Self::reap_deleted_graphs`] reclaims a graph.
    pub fn materialise_snapshots(&mut self, now: Instant) {
        self.snapshots.retain(|_, snapshot| !snapshot.is_reclaimed(now));
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
    }

    /// Apply the snapshot transition the clock has made due, then borrow the snapshot
    /// `id` names IN `region` — `None` when this Region holds no such snapshot.
    pub fn settled_snapshot(&mut self, id: &str, region: &str, now: Instant) -> Option<&Snapshot> {
        self.settled_snapshot_mut(id, region, now)
            .map(|snapshot| &*snapshot)
    }

    /// [`Self::settled_snapshot`], mutably: what `DeleteGraphSnapshot` writes the
    /// accepted deletion through.
    pub fn settled_snapshot_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut Snapshot> {
        self.materialise_snapshots(now);
        let snapshot = self.snapshots.get_mut(id)?;
        if !snapshot.is_in(region) {
            return None;
        }
        Some(snapshot)
    }

    /// Apply the import-task transition the clock has made due, then borrow the task
    /// `id` names IN `region` — `None` when this Region holds no such task.
    pub fn settled_import_task(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&ImportTask> {
        self.settled_import_task_mut(id, region, now).map(|t| &*t)
    }

    /// [`Self::settled_import_task`], mutably.
    pub fn settled_import_task_mut(
        &mut self,
        id: &str,
        region: &str,
        now: Instant,
    ) -> Option<&mut ImportTask> {
        // The statuses the clock owes every task, and the loads whose write it has
        // made due, are materialised together: a task that reports a completed load
        // and a graph that does not hold it would be one record contradicting the
        // other.
        self.materialise_import_loads(now);
        let task = self.import_tasks.get_mut(id)?;
        if !task.is_in(region) {
            return None;
        }
        Some(task)
    }

    /// The import tasks `region` holds, in task-id order, resuming after `after_id`
    /// when a pagination token named a position, and restricted to the tasks that load
    /// into one graph when `of_graph` names one: the listing `ListImportTasks` pages
    /// over.
    ///
    /// The same shape as [`Self::export_tasks_in`], for the same reason: `import_tasks`
    /// is a `BTreeMap`, so one stable total order underlies every page and a token can
    /// name a position in it. `of_graph` is matched against the task's stored `graph_id`,
    /// so a task stays listed under the graph it loaded into even once that graph is
    /// gone.
    pub fn import_tasks_in<'store>(
        &'store self,
        region: &'store str,
        of_graph: Option<&'store str>,
        after_id: Option<&str>,
    ) -> impl Iterator<Item = &'store ImportTask> {
        let lower = match after_id {
            Some(id) => ::std::ops::Bound::Excluded(id.to_string()),
            None => ::std::ops::Bound::Unbounded,
        };
        self.import_tasks
            .range((lower, ::std::ops::Bound::Unbounded))
            .map(|(_, task)| task)
            .filter(move |task| {
                task.is_in(region) && of_graph.is_none_or(|graph_id| task.graph_id == graph_id)
            })
    }

    /// The tag map of the resource `arn` names IN `region`, or `None` when this
    /// Region holds no resource by that ARN.
    ///
    /// The lookup is on the ARN the service MINTED for the resource, so a request
    /// naming the right resource id in the wrong region, account or partition names
    /// no resource here — an ARN identifies one resource in one region of one
    /// account. `region` is the Region the REQUEST was signed for, which the regional
    /// endpoint makes part of the lookup as well: a resource of another Region is not
    /// reachable through this one even by its own full ARN.
    ///
    /// Tags live on the resource record itself, which is what makes a tag supplied at
    /// create time and one written later by `TagResource` the same bytes, read back by
    /// this one lookup. Graphs, snapshots and export tasks are the taggable resources
    /// this slice stores; an import task holds no tags of its own (the `tags` of
    /// `CreateGraphUsingImportTask` are documented as metadata "to the new graph", so
    /// they live on the graph that call creates). A private graph endpoint never joins
    /// this lookup either: the model gives it neither an ARN nor a `tags` member, so the
    /// tagging surface has nothing to address it by.
    pub fn tags_of(&self, arn: &str, region: &str) -> Option<&BTreeMap<String, String>> {
        self.graphs
            .values()
            .find(|graph| graph.is_in(region) && graph.arn == arn)
            .map(|graph| &graph.tags)
            .or_else(|| {
                self.snapshots
                    .values()
                    .find(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
                    .map(|snapshot| &snapshot.tags)
            })
            .or_else(|| {
                self.export_tasks
                    .values()
                    .find(|task| task.is_in(region) && task.arn == arn)
                    .map(|task| &task.tags)
            })
    }

    /// [`Self::tags_of`], mutably: the one place `TagResource`/`UntagResource` write.
    pub fn tags_of_mut(
        &mut self,
        arn: &str,
        region: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        // Two disjoint maps and one `&mut self`, so which map holds the ARN is
        // decided by an immutable scan before either is borrowed mutably.
        if self
            .graphs
            .values()
            .any(|graph| graph.is_in(region) && graph.arn == arn)
        {
            return self
                .graphs
                .values_mut()
                .find(|graph| graph.is_in(region) && graph.arn == arn)
                .map(|graph| &mut graph.tags);
        }
        if self
            .snapshots
            .values()
            .any(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
        {
            return self
                .snapshots
                .values_mut()
                .find(|snapshot| snapshot.is_in(region) && snapshot.arn == arn)
                .map(|snapshot| &mut snapshot.tags);
        }
        self.export_tasks
            .values_mut()
            .find(|task| task.is_in(region) && task.arn == arn)
            .map(|task| &mut task.tags)
    }
}

// ---------------------------------------------------------------------------
// Store bounds
//
// A graph is created by a caller and reclaimed only by an explicit `DeleteGraph`, so
// the growth site carries a cap and nothing caller-sized reaches the store unchecked.
// The product is finite: at most `MAX_GRAPHS` graphs, each holding a `graphName` of at
// most 63 characters, a `kmsKeyIdentifier` of at most 1024, and at most `MAX_TAGS` tags
// of at most `MAX_TAG_KEY_CHARS` / `MAX_TAG_VALUE_CHARS` characters. A query is the one
// record no caller has to reclaim: it is capped at `MAX_QUERIES` entries of at most
// `MAX_QUERY_STRING_CHARS` characters AND every entry carries a due end instant, so the
// query map shrinks on its own as well.
// ---------------------------------------------------------------------------

/// The most graphs one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes: the store is shared
/// process-wide and grows by one entry per `CreateGraph`, so without a ceiling
/// repeated calls grow the emulator's memory without bound. `CreateGraph` reports
/// reaching it with the `ServiceQuotaExceededException` (402) the model declares for
/// exactly this condition. Set well above any realistic use of a local emulator.
pub const MAX_GRAPHS: usize = 100;

/// The most snapshots one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows
/// by one entry per `CreateGraphSnapshot`, so without a ceiling repeated calls grow
/// the emulator's memory without bound. `CreateGraphSnapshot` reports reaching it
/// with the `ServiceQuotaExceededException` (402) the model declares for exactly
/// this condition. Set well above any realistic use of a local emulator.
pub const MAX_SNAPSHOTS: usize = 100;

/// The most characters a `graphName` or a `snapshotName` may carry, from the modelled
/// `@length(1..63)` on both
/// (`smithy_query(query="operation",operation="CreateGraphSnapshot")`).
pub const MAX_RESOURCE_NAME_CHARS: usize = 63;

/// What the name of a graph's FINAL snapshot — the one a `skipSnapshot=false` delete
/// asks for — adds to the graph's own name.
pub const FINAL_SNAPSHOT_SUFFIX: &str = "-final-snapshot";

/// What a final snapshot is named after when the graph's own name yields no name the
/// modelled `SnapshotName` accepts: this stem, then the snapshot's freshly minted id.
///
/// It opens with a lowercase letter and carries nothing but lowercase letters and
/// single interior hyphens, so prefixing a `gs-…` id with it also lifts the value clear
/// of the prefix `SnapshotName` reserves.
pub const FINAL_SNAPSHOT_STEM: &str = "final-snapshot";

/// The prefix the modelled `SnapshotName` pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`
/// forbids a snapshot name from opening with — the prefix the service reserves for
/// snapshot IDS (`SnapshotId`: `^gs-[a-z0-9]{10}$`), so that a name can never be read as
/// an id.
///
/// Passed to [`resource_name_is_well_formed`] by every site that CHECKS a caller's
/// `snapshotName` and by the site that MINTS one, so neither can enforce a different
/// reserved prefix than the other.
pub const SNAPSHOT_NAME_RESERVED_PREFIX: &str = "gs-";

/// The most import tasks one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows
/// by one entry per `StartImportTask` or `CreateGraphUsingImportTask`, and a task is
/// never reclaimed, so without a ceiling repeated calls grow the emulator's memory
/// without bound. `StartImportTask`
/// models no quota error, so reaching the ceiling is reported with the
/// `ConflictException` (409) it does model — the request conflicts with what the store
/// already holds; `CreateGraphUsingImportTask` reports it with the
/// `ServiceQuotaExceededException` (402) its own model declares. Set well above any
/// realistic use of a local emulator.
pub const MAX_IMPORT_TASKS: usize = 100;

/// The most characters an import task's `source` may carry.
///
/// The model puts no `@length` on `source`
/// (`smithy_query(query="operation",operation="StartImportTask")`), so this is the
/// store's own bound on a caller-sized string, set to the 1024 characters the
/// service's other S3-location members are modelled with (`NeptuneImportOptions`'s
/// `s3ExportPath`, `s3ExportKmsKeyId`: `@length(1..1024)`).
pub const MAX_IMPORT_SOURCE_CHARS: usize = 1024;

/// The modelled `Format` value for RDF N-Triples data
/// (`smithy_query(query="shape",shape="Format")`), the one format whose load needs
/// `blankNodeHandling`.
pub const NTRIPLES_FORMAT: &str = "NTRIPLES";

/// The `ValidationException` clause a request that omits `blankNodeHandling` beside an
/// `NTRIPLES` format fails, or `None` when the pair holds.
///
/// Documented member dependency: `blankNodeHandling` "must be provided when format is
/// ntriples" (`smithy_query(query="doc",shape="StartImportTaskInput",
/// member="blankNodeHandling")`), because an N-Triples dataset is the one whose load has
/// to be told what to do with its blank nodes. No modelled trait can express a
/// dependency between two members, so it is refused by hand — stated ONCE here and
/// reached by every operation that accepts an import `format` (`StartImportTask`,
/// `CreateGraphUsingImportTask`), so the two cannot drift into different acceptance
/// sets or different wording for one documented rule.
pub fn blank_node_handling_clause(
    format: Option<&str>,
    blank_node_handling: Option<&str>,
) -> Option<String> {
    if format == Some(NTRIPLES_FORMAT) && blank_node_handling.is_none() {
        return Some(constraint_clause(
            "blankNodeHandling",
            None,
            &format!("Member must be provided when format is {NTRIPLES_FORMAT}"),
        ));
    }
    None
}

/// The requirement an import task's `source` fails, or `None` when it satisfies both
/// bounds the store puts on that member.
///
/// Stated ONCE, and reached by every operation that accepts an import `source`
/// (`StartImportTask`, `CreateGraphUsingImportTask`), so no two of them can drift into
/// different acceptance sets for one documented member:
///
/// * A required LOCATION cannot be blank. The member is modelled as a bare `String` —
///   no `@length`, no `@pattern` — so neither emptiness nor a value of nothing but
///   whitespace is refused at the boundary, and neither locates any data.
/// * The upper bound ([`MAX_IMPORT_SOURCE_CHARS`]) is measured on the value that would
///   be STORED, so no stored record ever holds more than the bound a refusal
///   advertises.
///
/// The SCHEME is deliberately not constrained: `source` is documented as "an Amazon S3
/// path, or ... a Neptune database endpoint or snapshot", so no one scheme is required.
/// Both bounds are worded by the one builder every length refusal of this service is
/// worded by, so a bound this handler enforces reads exactly as the boundary states the
/// same class — and the requirement never measures the value back, so the value that
/// broke the upper bound is not echoed.
pub fn import_source_requirement(source: &str) -> Option<String> {
    if source.trim().is_empty() {
        return Some(min_length_requirement(1));
    }
    let stored_chars = source.chars().count();
    if stored_chars > MAX_IMPORT_SOURCE_CHARS {
        return Some(max_length_requirement(MAX_IMPORT_SOURCE_CHARS));
    }
    None
}

/// The sentence an import that locates its data in Amazon S3 and says nothing about how
/// that data is written is refused with.
///
/// Fixed text that interpolates nothing from the request: the rule is about a PAIR of
/// members rather than about one value, so there is no value to name in it.
const S3_SOURCE_WITHOUT_FORMAT: &str = "Source format should be specified when Source type is S3.";

/// The modelled `ValidationException` an import request that locates its data in Amazon
/// S3 without naming a `format` earns, or `None` when the pair holds.
///
/// Documented member dependency: `format` "Specifies the format of Amazon S3 data to be
/// imported" (`smithy_query(query="doc",shape="StartImportTaskInput",member="format")`),
/// so it is what tells the load how the objects at an S3 location are WRITTEN — a
/// location in an object store names bytes and nothing about their layout, while the
/// other thing `source` may name, "a Neptune database endpoint or snapshot"
/// (`smithy_query(query="doc",shape="StartImportTaskInput",member="source")`), carries
/// its own structure and needs no format. So the member is required exactly when the
/// source is an S3 location ([`is_s3_uri`]) and optional otherwise.
///
/// The member is NOT `@required` (`smithy_query(query="operation",
/// operation="StartImportTask")`) — no modelled trait can express a dependency between
/// two members — so the rule is refused by hand, stated ONCE here and reached by every
/// operation that accepts an import `source` and `format` (`StartImportTask`,
/// `CreateGraphUsingImportTask`), so the two cannot drift into different acceptance sets
/// or different wording for one rule.
///
/// It is answered as a whole `ValidationException` rather than as a clause of the
/// constraint envelope [`constraint_violation`] builds, because the service words this
/// rule as one sentence about the request and not as a measurement of a member's value;
/// the modelled `reason` beside it is still the one a member that does not satisfy a rule
/// stated about it carries.
pub fn import_format_requirement(source: &str, format: Option<&str>) -> Option<ValidationException> {
    if format.is_some() || !is_s3_uri(source) {
        return None;
    }
    Some(ValidationException {
        message: S3_SOURCE_WITHOUT_FORMAT.to_string(),
        reason: Some(CONSTRAINT_VIOLATION.to_string()),
    })
}

/// The prefix the service reserves for import- and export-task ids, from the modelled
/// `TaskId` pattern `^t-[a-z0-9]{10}$`.
pub const TASK_ID_PREFIX: &str = "t-";

/// The `statusReason` of an import task that stopped because it was cancelled.
pub const IMPORT_CANCELLED_REASON: &str =
    "The import task was cancelled by a CancelImportTask request.";

/// The sentence the caller of a query that was stopped by a `CancelQuery` request is
/// answered with: the run it was waiting on did not finish, and why.
///
/// This service's own wording, worded like the `statusReason` of cancelled work
/// elsewhere in it ([`IMPORT_CANCELLED_REASON`]) so a caller reads one grammar for "the
/// request that stopped this": no recording of a cancelled query's own answer is
/// available to take a sentence from, and the condition is reported under the modelled
/// `reason` the model keeps for a cancelled query.
pub const QUERY_CANCELLED_MESSAGE: &str = "The query was cancelled by a CancelQuery request.";

/// The most export tasks one store holds.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per `StartExportTask`, and an export task is never reclaimed (a finished
/// task stays readable for the life of the process), so without a ceiling repeated
/// calls grow the emulator's memory without bound. `StartExportTask` models no quota
/// error (`smithy_query(query="operation",operation="StartExportTask")` declares
/// `ConflictException`, `ValidationException`, `ThrottlingException`,
/// `ResourceNotFoundException` and `InternalServerException` only), so reaching it is
/// reported with the `ConflictException` (409) it does model, through
/// [`store_ceiling_conflict`] — the request conflicts with what the store already holds
/// — and not as throttling, which would promise that a retry can succeed. Set well
/// above any realistic use of a local emulator.
pub const MAX_EXPORT_TASKS: usize = 100;

/// The most queries one store holds in flight, across every graph.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same kind of bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per admitted query, so without a ceiling repeated query requests grow the
/// emulator's memory without bound. Reaching it is reported by the operation that
/// admits a query, from the message [`ComAmazonawsNeptunegraphAmazonneptunegraphState::insert_query`]
/// hands back. Set higher than the other stores' ceilings because a query record is in
/// flight for the span the clock owes its run ([`QUERY_RUN_SPAN`]) rather than until a
/// caller reclaims it: what this bounds is how many queries one caller may have in
/// flight at once, and it is set well above the queries a client submits within one such
/// span. Each record carries at most [`MAX_QUERY_STRING_CHARS`] of text, so the two
/// bounds together cap what the map can hold, and records leave it on their own as the
/// clock makes their end due.
pub const MAX_QUERIES: usize = 250;

/// The most nodes one graph's data may hold.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same kind of bound
/// [`MAX_GRAPHS`] puts on the graph map: a graph's data grows by what the data plane
/// writes into it and is reclaimed only when the graph is emptied or deleted, so
/// without a ceiling repeated writes grow the emulator's memory without bound.
/// [`GraphData::insert_node`] hands the reason back for the operation that was
/// writing to report.
///
/// It also bounds the work ONE reading of the data costs: a report over the data
/// walks the nodes, the edges and the properties on them once each, so the cost of a
/// request is bounded by this ceiling, [`MAX_EDGES_PER_GRAPH`] and
/// [`MAX_PROPERTIES_PER_ELEMENT`] TOGETHER rather than by any of them alone. Set
/// above any realistic use of a local emulator and low enough that the whole product
/// stays small.
pub const MAX_NODES_PER_GRAPH: usize = 10_000;

/// The most edges one graph's data may hold.
///
/// The same guard [`MAX_NODES_PER_GRAPH`] puts on the nodes, set higher because a
/// graph holds more relationships than things: a node can be an endpoint of many
/// edges. [`GraphData::insert_edge`] hands the reason back for the operation that
/// was writing to report.
pub const MAX_EDGES_PER_GRAPH: usize = 20_000;

/// The most properties one node or one edge may carry.
///
/// Emulator resource guard, NOT a quota AWS publishes: without it one element's
/// property map is unbounded, so the number of properties a graph holds — and the
/// work a report over them costs — would not be bounded by the two ceilings above.
/// [`GraphData::insert_node`] and [`GraphData::insert_edge`] hand the reason back for
/// the operation that was writing to report.
pub const MAX_PROPERTIES_PER_ELEMENT: usize = 100;

/// The most characters one stored query's text may carry.
///
/// Emulator resource guard, NOT a quota AWS publishes: `queryString` is modelled as an
/// unconstrained `String` (`smithy_query(query="operation",operation="ExecuteQuery")`),
/// so without a ceiling one request could store an arbitrarily large text that
/// `GetQuery` then keeps reporting. Set well above any query a local emulator is asked
/// to run.
pub const MAX_QUERY_STRING_CHARS: usize = 65_536;

/// AWS's tagging limit of 50 tags per resource, which this service models as
/// `TagMap`'s `@length(0..50)`.
pub const MAX_TAGS: usize = 50;

/// The most private graph endpoints one store holds, across every graph.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the same bound
/// [`MAX_GRAPHS`] puts on the graph map: the store is shared process-wide and grows by
/// one entry per `CreatePrivateGraphEndpoint`, so without a ceiling repeated calls grow
/// the emulator's memory without bound. `CreatePrivateGraphEndpoint` reports reaching
/// it with the `ServiceQuotaExceededException` (402) the model declares for exactly
/// this condition. Set well above any realistic use of a local emulator.
pub const MAX_PRIVATE_GRAPH_ENDPOINTS: usize = 100;

/// The prefix AWS keeps for the tags it applies itself, which a caller-supplied tag may
/// carry on neither half.
///
/// The service documents the rule on the member that carries caller tags, once for each
/// half of an entry (`smithy_query(query="doc",shape="TagResourceInput",member="tags")`:
/// a key "can't be prefixed with `aws:`", and a value "can't be prefixed with `aws:`"),
/// and it is AWS's tagging rule for every taggable resource. The modelled traits cannot
/// express it — `TagKey`'s `@pattern` admits `:` anywhere and `TagValue` carries no
/// pattern at all (`smithy_query(query="shape",shape="TagKey,TagValue")`) — so it is a
/// rule the boundary walk leaves to the service, enforced for every operation that
/// accepts tags by [`bound_tags`].
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Whether `text` — one half of a tag entry, a key or a value — lies in the namespace
/// [`RESERVED_TAG_PREFIX`] reserves.
///
/// Compared WITHOUT regard to case: a differently cased spelling of the prefix would
/// otherwise store a tag that still reads as one AWS applied itself. `get(..)` rather
/// than slicing, so a value whose prefix-length byte boundary falls inside a multi-byte
/// character is simply not reserved instead of panicking.
pub fn is_reserved_tag_text(text: &str) -> bool {
    text.get(..RESERVED_TAG_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_PREFIX))
}

/// From the modelled `TagKey`: `@length(1..128)`.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// From the modelled `TagKey`: `@length(1..128)`'s lower bound.
pub const MIN_TAG_KEY_CHARS: usize = 1;

/// From the modelled `TagKey`: `@pattern("^[a-zA-Z+-=._:/]+$")`, as the model spells it
/// (`smithy_query(query="shape",shape="TagKey")`). Reported through
/// [`pattern_requirement`], which states it the way this service states every pattern.
pub const TAG_KEY_PATTERN: &str = "^[a-zA-Z+-=._:/]+$";

/// From the modelled `TagValue`: `@length(0..256)`.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// One paginated listing of this service, the listing half of the scope a pagination
/// token belongs to ([`PageScope`]).
///
/// A token names a position in ONE listing's order — the order of the listing that
/// handed it out — so the listing is part of what a token carries and is checked when
/// the token comes back. The id space the position lies in does not identify the
/// listing on its own: the model spells one `^t-[a-z0-9]{10}$` pattern for an import
/// task and an export task (`smithy_query(query="shape",shape="TaskId,ExportTaskId")`),
/// so the two task listings order two independently keyed collections by positions drawn
/// from the SAME space, and a position in that space names a different cut point in each
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagedListing {
    /// `ListGraphs`, ordered by graph id.
    Graphs,
    /// `ListGraphSnapshots`, ordered by snapshot id.
    GraphSnapshots,
    /// `ListImportTasks`, ordered by import-task id.
    ImportTasks,
    /// `ListExportTasks`, ordered by export-task id.
    ExportTasks,
    /// `ListPrivateGraphEndpoints`, ordered by VPC id inside the collection held under
    /// one graph, so a token of it is scoped to that graph as well
    /// ([`PageScope::listing_in`]).
    PrivateGraphEndpoints,
}

impl PagedListing {
    /// The name a token carries the listing that minted it under: the operation's own
    /// name, which no two listings of this service share.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Graphs => "ListGraphs",
            Self::GraphSnapshots => "ListGraphSnapshots",
            Self::ImportTasks => "ListImportTasks",
            Self::ExportTasks => "ListExportTasks",
            Self::PrivateGraphEndpoints => "ListPrivateGraphEndpoints",
        }
    }

    /// Whether `key` names a position in the order this listing pages: the id space its
    /// entries are keyed by, one of the `is_*_id` predicates below.
    fn holds_position(self, key: &str) -> bool {
        match self {
            Self::Graphs => is_graph_id(key),
            Self::GraphSnapshots => is_snapshot_id(key),
            // One predicate for both, because the model gives the two task listings one
            // id pattern; what separates their tokens is the listing, not the pattern.
            Self::ImportTasks | Self::ExportTasks => is_task_id(key),
            Self::PrivateGraphEndpoints => is_vpc_id(key),
        }
    }
}

/// What separates the minting listing — and, where it has one, the parent its entries
/// are ordered inside — from the position inside a token's decoded payload.
///
/// Neither a listing's name nor any key this service orders a listing by carries this
/// character, so a payload splits back into exactly the parts it was built from.
const PAGE_TOKEN_SEPARATOR: char = ':';

/// The scope one pagination token belongs to: the listing that minted it, and — when
/// that listing pages a collection held per parent resource — the parent whose order
/// the position lies in.
///
/// A position names a cut point in ONE order. Four listings of this service range over a
/// single collection and apply their `graphIdentifier` restriction as a filter after the
/// range, so one total order underlies every page of them and the listing alone scopes
/// their tokens. `ListPrivateGraphEndpoints` instead pages the endpoints held under one
/// graph ([`ComAmazonawsNeptunegraphAmazonneptunegraphState::private_endpoints`]), a
/// separate order per graph, so the
/// parent is part of the scope: the same VPC id is a legal key under every graph and
/// names a different cut point in each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageScope<'parent> {
    /// Which listing minted — and will take back — the token.
    listing: PagedListing,
    /// The parent resource whose own collection the position lies in, for a listing
    /// that keeps one order per parent, and `None` for a listing with a single order.
    parent: Option<&'parent str>,
}

impl<'parent> PageScope<'parent> {
    /// The scope of a listing that pages ONE order: a token of it names a position in
    /// that order wherever the listing is asked from.
    pub const fn whole_listing(listing: PagedListing) -> Self {
        Self {
            listing,
            parent: None,
        }
    }

    /// The scope of a listing that pages the collection held under `parent`: a token of
    /// it names a position in THAT parent's order and in no other's.
    pub const fn listing_in(listing: PagedListing, parent: &'parent str) -> Self {
        Self {
            listing,
            parent: Some(parent),
        }
    }

    /// How a token spells the scope it belongs to: the listing's own name, followed by
    /// the parent whose order the position lies in when the listing keeps one order per
    /// parent.
    fn label(self) -> String {
        match self.parent {
            Some(parent) => format!("{}{PAGE_TOKEN_SEPARATOR}{parent}", self.listing.label()),
            None => self.listing.label().to_string(),
        }
    }
}

/// The pagination token `scope` resumes after `last_returned_key` — the id, or other
/// stable key, of the last entry the page just returned.
///
/// ONE token format for every listing of this service, so a client that pages one
/// listing and then another sees one token grammar: the scope the token belongs to and
/// the position it reached, hex-encoded, which keeps the token opaque to a client and
/// within `PaginationToken`'s 1..8192 length.
pub fn encode_page_token(scope: PageScope<'_>, last_returned_key: &str) -> String {
    hex::encode(format!(
        "{}{PAGE_TOKEN_SEPARATOR}{last_returned_key}",
        scope.label()
    ))
}

/// The position a token resumes `scope` after, or `None` when it is not a token
/// [`encode_page_token`] minted for that scope.
///
/// A token is a token OF ONE SCOPE: it is accepted only when it names `scope` itself —
/// the listing, and the parent that listing was paging when the token was handed out —
/// and a position in the id space that listing orders its entries by. A token minted
/// under any other scope, even one whose position lies in the same modelled id space (an
/// import-task token sent to the export-task listing, or one graph's endpoint token sent
/// to another graph's endpoints), names no position in the order being paged, so it is
/// refused rather than used as a cut point in a collection it was never taken from.
///
/// Membership in the store is deliberately NOT required: a resource deleted between two
/// pages must still leave its successors reachable, which is what resuming from a
/// position — rather than from a row — gives.
///
/// The accepted set is exactly the MINTED set: a value is a token of this scope only
/// when it is, byte for byte, the token [`encode_page_token`] produces for the position
/// it decodes to. Decoding alone is weaker than that — several spellings of the token
/// grammar can carry the same bytes, and only one of them is ever handed out — so the
/// token is compared against its own canonical minting before the id space is consulted.
pub fn decode_page_token(token: &str, scope: PageScope<'_>) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let position = decoded
        .strip_prefix(&scope.label())?
        .strip_prefix(PAGE_TOKEN_SEPARATOR)?;
    if encode_page_token(scope, position) != token {
        return None;
    }
    scope
        .listing
        .holds_position(position)
        .then(|| position.to_string())
}

/// The modelled `ValidationException` for a `nextToken` [`decode_page_token`] refuses:
/// a value this service never issued for the page it was sent to.
///
/// The service models its own `ValidationException`, which carries a `reason` beside
/// `message` (`smithy_query(query="shape",shape="ValidationException")`), so every
/// refusal of client input is reported through that shape and fills the member a client
/// reads: a token the caller sent that this service cannot resolve to a position is an
/// argument the operation does not accept, which is what `ILLEGAL_ARGUMENT` names among
/// the modelled reasons (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`). The
/// message names the member and the value, the way every other refusal of a sent value
/// does. Written once here and reached by every paginated listing, so no two of them
/// word — or classify — the same refusal differently.
pub fn unissued_page_token(token: &str) -> ValidationException {
    ValidationException {
        message: format!("nextToken '{token}' is not a pagination token this service issued"),
        reason: Some("ILLEGAL_ARGUMENT".to_string()),
    }
}

/// The page size a listing serves when `maxResults` is absent: the largest the member
/// allows (`smithy_query(query="operation",operation="ListGraphs,ListGraphSnapshots,ListImportTasks,ListExportTasks,ListPrivateGraphEndpoints")`
/// -> `maxResults` range 1..100), so an unpaginated caller sees as much as one page can
/// carry.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How many entries one page of a listing reports: the `maxResults` the request carries
/// when it carries one that a count can hold, else [`DEFAULT_PAGE_SIZE`].
///
/// ONE resolution for every paginated listing of this service, hosted beside the token
/// codec the same listings call, so the page-size rule cannot differ between two of
/// them. The bound is what keeps the invariant the paging relies on — a page of at least
/// one entry — so a page can never come back empty while entries remain; the modelled
/// `maxResults` range 1..100 is refused by the boundary validator before any handler
/// runs, so no response can differ for it.
pub fn resolve_page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, DEFAULT_PAGE_SIZE)
}

/// One page of at most `page_size` entries taken from `entries`, and the `nextToken`
/// that resumes the listing after it.
///
/// ONE paging decision for every paginated listing of this service: a token is minted
/// only when an entry remains after the page, and it names the LAST entry the page
/// reports — `key_of` is the stable key that entry is ordered by — so a resumed page
/// returns the remainder without repeats or omissions. A listing whose entries are
/// exhausted by the page reports no token, which is what tells a client it has seen
/// everything.
///
/// `entries` is consumed past the page and past the one entry the remainder probe
/// takes, so a caller that needs the rest of the listing re-builds the iterator from the
/// token rather than reusing this one.
pub fn take_page<T, I: Iterator<Item = T>>(
    entries: &mut I,
    page_size: usize,
    scope: PageScope<'_>,
    key_of: impl Fn(&T) -> &str,
) -> (Vec<T>, Option<String>) {
    let page: Vec<T> = entries.by_ref().take(page_size).collect();
    let next_token = if entries.next().is_some() {
        page.last()
            .map(|entry| encode_page_token(scope, key_of(entry)))
    } else {
        None
    };
    (page, next_token)
}

/// Whether `value` matches the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
///
/// Shared by the id generator's own test and by [`PagedListing::holds_position`], which
/// refuses a pagination token that does not name a position in the graph-id space.
pub fn is_graph_id(value: &str) -> bool {
    is_resource_id(value, "g-")
}

/// Whether `value` matches the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
pub fn is_snapshot_id(value: &str) -> bool {
    is_resource_id(value, "gs-")
}

/// Whether `value` matches the modelled `VpcId` pattern `^vpc-[a-z0-9]+$`
/// (`smithy_query(query="shape",shape="VpcId")`).
///
/// Unlike this service's own resource ids, a VPC id is minted by another service, so
/// the pattern bounds only its prefix and alphabet and not its length: anything from
/// one character after the prefix upwards is a position in that id space.
pub fn is_vpc_id(value: &str) -> bool {
    value.strip_prefix("vpc-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
///
/// One predicate for the whole task-id space: the model spells the same pattern for
/// an import task and an export task, and this service mints both from it.
pub fn is_task_id(value: &str) -> bool {
    is_resource_id(value, TASK_ID_PREFIX)
}

/// The prefix the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$` requires: the
/// same task-id space [`TASK_ID_PREFIX`] names, since the model gives an import task
/// and an export task one id pattern.
pub const EXPORT_TASK_ID_PREFIX: &str = TASK_ID_PREFIX;

/// The scheme an Amazon S3 URI is spelled with, which an export `destination` has to
/// carry: the member is documented as "The Amazon S3 URI where data will be exported
/// to" (`smithy_query(query="doc",shape="StartExportTaskInput",member="destination")`)
/// and is modelled as a plain `String` of 1..1024 characters, so the scheme is the one
/// part of it no boundary constraint checks.
pub const S3_URI_SCHEME: &str = "s3://";

/// Whether `value` is an Amazon S3 URI: the `s3://` scheme followed by a bucket name,
/// optionally followed by `/` and a key prefix.
///
/// Only the two facts that make a URI addressable are required — the scheme and a
/// non-empty bucket segment — because S3's own bucket-name rules are enforced by S3,
/// not by the service that is handed the URI.
pub fn is_s3_uri(value: &str) -> bool {
    value
        .strip_prefix(S3_URI_SCHEME)
        .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// The requirement clause a `destination` that is not an Amazon S3 URI fails, in the
/// wording [`constraint_clause`] frames.
pub fn s3_uri_requirement() -> String {
    format!("Member must be an Amazon S3 URI of the form {S3_URI_SCHEME}bucket[/key-prefix]")
}

/// Whether `value` is `prefix` followed by exactly ten `[a-z0-9]` characters — the
/// shape every resource id of this service has, differing only in its prefix.
fn is_resource_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    suffix.len() == 10
        && suffix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `name` satisfies this service's resource-name pattern
/// `^(?!<forbidden_prefix>)[a-z][a-z0-9]*(-[a-z0-9]+)*$`, read literally: not the
/// prefix the service reserves for its own ids, a lowercase letter first, then
/// lowercase letters, digits and single interior hyphens, and no trailing hyphen.
///
/// `GraphName` and `SnapshotName` spell the same expression with a different
/// reserved prefix (`g-`, `gs-`), and both open with a negative lookahead the
/// boundary validator's regex engine cannot compile — an uncompilable `@pattern` is
/// treated as matching — so this is where that constraint is enforced for every
/// operation that accepts a name.
pub fn resource_name_is_well_formed(name: &str, forbidden_prefix: &str) -> bool {
    if name.starts_with(forbidden_prefix) {
        return false;
    }
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let mut after_hyphen = false;
    for c in chars {
        if c == '-' {
            // `(-[a-z0-9]+)*` requires at least one alphanumeric between hyphens.
            if after_hyphen {
                return false;
            }
            after_hyphen = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            after_hyphen = false;
        } else {
            return false;
        }
    }
    // A trailing hyphen would leave the last group empty.
    !after_hyphen
}

/// The region this request was signed for — the region that names the ARN (and, for
/// a graph, the endpoint) of every resource this service mints. Falls back to the
/// emulator-wide default region when the request carries no SigV4 credential scope
/// (request authentication may be off).
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| "us-east-1".to_string(), |scope| scope.region)
}

// ---------------------------------------------------------------------------
// How this service words a failure
//
// Both error messages below are built from the REQUEST — the identity it named, the
// value it sent and the constraint that value broke — and each is written in exactly
// one place, reached by every operation and by the framework-level refusal renderer in
// `crate::service`, so no two call sites can drift into different wording for the same
// failure.
// ---------------------------------------------------------------------------

/// The account every request this emulator serves is made by: the one canonical
/// emulator account, which is also the account segment of every ARN this service mints
/// (`::engine::emulator_arn`). Never a literal — the account contract has one source of
/// truth.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The one wording of "this account holds no such resource" that every account-scoped
/// lookup in this service reports.
///
/// The service words such a failure as the resource kind it looked for, the account the
/// lookup was resolved in, and then the clause that names what the request addressed the
/// resource by — so a caller reading only the message can tell which namespace was
/// searched and by which identifier. `resource` is the kind as the service names it
/// ("graph", "export task"); `identifier_clause` is how the OPERATION spells the
/// identifier it resolved ("graph id", "import task id", the bare "task id"), because
/// that clause is per-operation wording rather than a property of the namespace — which
/// is why it is passed in rather than derived from `resource` here. Written once so no
/// refusal of a missing resource can drift in anything else.
fn resource_not_found_for_caller(
    resource: &str,
    identifier_clause: &str,
    identifier: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "No {resource} found for customer [{}] with {identifier_clause} [{identifier}]",
            caller_account_id()
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `graphIdentifier` that names no
/// stored graph.
///
/// The graph namespace is per-account, so "there is no such graph" is a statement about
/// one account: the failure names the account the lookup was resolved in as well as the
/// identifier the request asked for, which is what lets a caller tell a graph that was
/// deleted apart from a call that reached the wrong account or partition. The modelled
/// shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go.
pub fn graph_not_found(graph_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller("graph", "graph id", graph_identifier)
}

/// The modelled `ResourceNotFoundException` for a `snapshotIdentifier` that names no
/// stored snapshot.
///
/// The snapshot namespace is per-account like the graph namespace, so the failure
/// names the account the lookup was resolved in beside the identifier the request
/// asked for — the same two facts [`graph_not_found`] reports, and for the same
/// reason.
pub fn snapshot_not_found(snapshot_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller("snapshot", "snapshot id", snapshot_identifier)
}

/// The identifier clause the CANCEL operations of both task namespaces word their
/// not-found refusal with: the bare kind of identifier `taskIdentifier` is, the modelled
/// `^t-[a-z0-9]{10}$` `TaskId` (`smithy_query(query="shape",shape="TaskId")`).
///
/// The READ operations instead qualify that clause with the kind of task they searched
/// for ([`read_task_id_clause`]): the two task namespaces share this identifier form, so
/// on a read that qualifier is the only thing in the message that says which namespace
/// was searched. Which clause a refusal carries is per-operation wording, which is why
/// each operation's builder states it.
const TASK_ID_CLAUSE: &str = "task id";

/// The identifier clause the operations that READ a task word their not-found refusal
/// with: the kind of task that was searched for, then `id`.
fn read_task_id_clause(resource: &str) -> String {
    format!("{resource} id")
}

/// The modelled `ResourceNotFoundException` `GetImportTask` refuses a `taskIdentifier`
/// that names no stored import task with.
///
/// The task namespace is per-account like the graph and snapshot namespaces, so the
/// failure names the account the lookup was resolved in beside the identifier the
/// request asked for — the same two facts [`graph_not_found`] reports, and for the
/// same reason — and the READ spells that identifier out as the import task's own.
pub fn import_task_not_found(task_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller(
        IMPORT_TASK_RESOURCE,
        &read_task_id_clause(IMPORT_TASK_RESOURCE),
        task_identifier,
    )
}

/// The modelled `ResourceNotFoundException` `CancelImportTask` refuses a
/// `taskIdentifier` that names no stored import task with.
///
/// The same lookup [`import_task_not_found`] reports, worded as a CANCEL operation words
/// it: the bare [`TASK_ID_CLAUSE`].
pub fn import_task_not_found_on_cancel(task_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller(IMPORT_TASK_RESOURCE, TASK_ID_CLAUSE, task_identifier)
}

/// How this service names an import task in a refusal.
const IMPORT_TASK_RESOURCE: &str = "import task";

/// How this service names an export task in a refusal.
const EXPORT_TASK_RESOURCE: &str = "export task";

/// The modelled `ValidationException` (HTTP 400) a `queryId` that names no query in
/// flight on the graph the request named is refused with.
///
/// A query id is not a resource identifier a caller can hold on to: a query exists only
/// while it is waiting, running or stopping, so once it is over the id it was addressed
/// by names nothing the service can resolve at all — which is why the operations
/// addressed by `queryId` (`GetQuery`, `CancelQuery`) report such a value as an invalid
/// ARGUMENT rather than as a resource that is missing. The message states the value the
/// request sent, which is its one variable part, and the `reason` member the modelled
/// shape carries beside it (`smithy_query(query="shape",shape="ValidationException")`)
/// is filled with the modelled reason for an argument the operation does not accept,
/// `ILLEGAL_ARGUMENT` (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`) — the
/// same classification [`unissued_page_token`] gives a token the caller sent that this
/// service cannot resolve.
///
/// Written once here and reached by both of those operations, so a caller polling an id
/// through one of them and then the other cannot be told two different things about it.
pub fn invalid_query_id(query_id: &str) -> ValidationException {
    ValidationException {
        message: format!("Supplied queryId {query_id} is invalid"),
        reason: Some("ILLEGAL_ARGUMENT".to_string()),
    }
}

/// The modelled `ResourceNotFoundException` `GetExportTask` refuses a `taskIdentifier`
/// that names no stored export task with.
///
/// The export-task namespace is per-account like the graph and snapshot namespaces, so
/// the failure names the account the lookup was resolved in beside the identifier the
/// request asked for — the same two facts [`graph_not_found`] reports, and for the same
/// reason — and the READ spells that identifier out as the export task's own.
pub fn export_task_not_found(task_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller(
        EXPORT_TASK_RESOURCE,
        &read_task_id_clause(EXPORT_TASK_RESOURCE),
        task_identifier,
    )
}

/// The modelled `ResourceNotFoundException` `CancelExportTask` refuses a
/// `taskIdentifier` that names no stored export task with.
///
/// The same lookup [`export_task_not_found`] reports, worded as a CANCEL operation words
/// it: the bare [`TASK_ID_CLAUSE`].
pub fn export_task_not_found_on_cancel(task_identifier: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller(EXPORT_TASK_RESOURCE, TASK_ID_CLAUSE, task_identifier)
}

/// The modelled `ResourceNotFoundException` for a `(graphIdentifier, vpcId)` pair that
/// names no stored private graph endpoint.
///
/// A private graph endpoint is addressed as the endpoint of one graph in one VPC, so
/// the failure names both halves of that pair, in the order the request carries them —
/// which is what lets a caller tell an endpoint that has already been deleted apart
/// from one created in another VPC. The modelled shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go, and it is rendered the way the service renders
/// it: the graph the endpoint would reach, then the VPC it would be reached from.
pub fn private_endpoint_not_found(graph_identifier: &str, vpc_id: &str) -> ResourceNotFoundException
{
    ResourceNotFoundException {
        message: format!(
            "Private Graph Endpoint to Graph: {graph_identifier} from VPC: {vpc_id} does not exist"
        ),
    }
}

/// The modelled `ResourceNotFoundException` for a `resourceArn` that names no stored
/// resource.
///
/// The tagging operations identify their subject by ARN rather than by graph id, so the
/// failure names the ARN the request asked for, beside the account the lookup was
/// resolved in — the same two facts [`graph_not_found`] reports, which is what lets a
/// caller tell a deleted resource apart from a call that reached the wrong account or
/// partition. The modelled shape carries only `message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), so the message is
/// the only place either fact can go.
pub fn resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    resource_not_found_for_caller("resource", "resource arn", resource_arn)
}

// ---------------------------------------------------------------------------
// Passing an IAM role
//
// Three operations take a `roleArn` this service has to ASSUME to reach the caller's
// data (`StartImportTask`, `CreateGraphUsingImportTask`, `StartExportTask`). Whether
// the caller may hand that role over is an authorization decision about the caller and
// that one member, so it is stated once here and reached by every one of them — and by
// the gate in `crate::service::pass_role`, which settles it ahead of the boundary's
// model-constraint walk.
// ---------------------------------------------------------------------------

/// The IAM principal this request was signed by, as an authorization refusal names it.
///
/// Resolved from the request's own access key id through the SAME identity resolution
/// `sts:GetCallerIdentity` answers from — the session the key was minted for, or the IAM
/// user of this account that holds a long-term key
/// (`sts_store::resolve_request_caller` over `sts_store::long_term_identity`) — so the
/// principal a denial blames is the one the caller reads back from STS for the
/// credentials it signed with, rather than an identity derived from the credential's own
/// text. The resolution first materialises the session expiries the clock has already
/// made due, a write owed at the same instant whether or not anybody called.
///
/// An access key id that resolves to no session and to no user of this account is the
/// account's own published credential (`sts_store::ambient_identity`), and an engine
/// carrying neither store can hold no session and no user at all: both are that same
/// ambient principal. A credential whose session has expired resolves to it too, because
/// the resolution materialises the expiries the clock has made due before it looks the key
/// up; one the registry still holds as expired speaks for nobody, so the denial names the
/// account root — the authentication layer, not this decision, is what refuses such a
/// credential.
pub fn caller_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    // The IAM store answers which user holds a long-term key, and is read and released
    // before the session registry is locked, so no two of this emulator's stores are
    // held at once.
    let long_term = match sts_store::iam_state(engine) {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::long_term_identity(None, &access_key_id),
    };
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            ::std::time::SystemTime::now(),
            ctx.credential_store(),
            long_term,
        )
        .map(|caller| caller.identity.arn),
        // No session registry on this engine: the credential names no session, so it
        // speaks for the long-term identity it was resolved to.
        None => Ok(long_term.arn),
    };
    resolved.unwrap_or_else(|_expired| ::engine::emulator_arn("iam", "", "root"))
}

/// The resource prefix of an IAM role ARN, which is what tells a role apart from every
/// other IAM identity an ARN can name (a user, a group, an instance profile).
const IAM_ROLE_RESOURCE_PREFIX: &str = "role/";

/// The account an IAM role ARN names, or `None` when the value names no IAM role at all.
///
/// The acceptance set is the one the model fixes for a pass-role member — `RoleArn`'s
/// `@pattern` `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`
/// (`smithy_query(query="shape",shape="RoleArn")`): an `arn` of some `aws` partition, the
/// `iam` service with no Region (IAM is global), a twelve-digit account, and a `role/`
/// resource carrying at least one path segment. Read here rather than left to the
/// boundary validator because the authorization decision below is reached BEFORE that
/// walk, so it cannot assume the value has any particular form.
fn iam_role_account(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some("iam"),
        Some(""),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if !partition.starts_with("aws") {
        return None;
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let name = resource.strip_prefix(IAM_ROLE_RESOURCE_PREFIX)?;
    let names_a_role = !name.is_empty()
        && name.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c))
        });
    names_a_role.then_some(account)
}

/// The denial for a role in an account other than the caller's: the service would have
/// to assume a role it holds no trust relationship with, and nothing about the request
/// can establish one.
const CROSS_ACCOUNT_PASS_ROLE_DENIED: &str = "Cross-account pass role is not allowed.";

/// The authorization decision on a role the request asks this service to pass: the
/// `AccessDeniedException` (HTTP 403) that refuses it, or `None` when the caller may
/// pass it.
///
/// A role is passable only when it belongs to the account the call was made by — there
/// is one such account in this emulator ([`caller_account_id`]) — so the decision is the
/// account the value names. The two refusals differ in what the caller can do about
/// them: a role of another account is a role this service cannot assume however the
/// request is spelled, while a value that names no role at all is simply an
/// authorization the caller does not hold, which is the principal's own failure and
/// names it.
///
/// Taken before anything else an operation does, so neither reaches the store: an
/// authorization failure leaves no record behind.
pub fn pass_role_denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> Option<AccessDeniedException> {
    match iam_role_account(role_arn) {
        Some(account) if account == caller_account_id() => None,
        Some(_) => Some(AccessDeniedException {
            message: CROSS_ACCOUNT_PASS_ROLE_DENIED.to_string(),
        }),
        None => Some(AccessDeniedException {
            message: format!(
                "User: {} is not authorized to perform: null",
                caller_principal(engine, ctx)
            ),
        }),
    }
}

/// The modelled `ConflictException` for a create call whose requested name is already
/// held by a stored resource of the same kind.
///
/// `resource` is the kind of resource whose namespace the name belongs to, capitalised
/// as the refusal names it ("Graph", "Snapshot"). The message is derived from the
/// REQUESTED name and nothing else: the conflict is a statement about the name the
/// caller sent, and the record that already holds it is a resource this request never
/// addressed, so its identifier is not reported. Written once here and reached by every
/// name-uniqueness check in the service, so no two of them word one refusal differently
/// or disclose different amounts about the record they collided with.
fn duplicate_name_conflict(resource: &str, name: &str) -> ConflictException {
    ConflictException {
        message: format!("{resource} with name {name} already exists!"),
        // The modelled `reason` enum carries one value, `CONCURRENT_MODIFICATION`
        // (`smithy_query(query="shape",shape="ConflictExceptionReason")`), and a name
        // that is simply already taken is not a concurrent modification of anything, so
        // the member is left absent.
        reason: None,
    }
}

/// The modelled `ConflictException` for a `graphName` the account already holds in the
/// Region — raised by every operation that mints a graph (`CreateGraph`,
/// `CreateGraphUsingImportTask`, `RestoreGraphFromSnapshot`), which all key uniqueness
/// on the same index
/// ([`ComAmazonawsNeptunegraphAmazonneptunegraphState::graph_id_named`]).
pub fn graph_name_conflict(graph_name: &str) -> ConflictException {
    duplicate_name_conflict("Graph", graph_name)
}

/// The modelled `ConflictException` for a `snapshotName` the account already holds in
/// the Region
/// ([`ComAmazonawsNeptunegraphAmazonneptunegraphState::snapshot_id_named`]) — the
/// snapshot namespace's half of the same rule [`graph_name_conflict`] states for graphs.
pub fn snapshot_name_conflict(snapshot_name: &str) -> ConflictException {
    duplicate_name_conflict("Snapshot", snapshot_name)
}

/// The ONE clause this service words a reached store ceiling with: how many records of
/// that kind this emulator keeps at most.
///
/// `records` names what the ceiling counts, in the plural the refusal reads in ("import
/// tasks", "export tasks", "graph snapshots"), and `ceiling` is the bound that was met.
/// Written once here so a caller cannot tell one full store from another by the wording
/// of its refusal.
fn store_ceiling_clause(records: &str, ceiling: usize) -> String {
    format!("this emulator holds at most {ceiling} {records}")
}

/// The modelled `ConflictException` for a growth call that met a FULL store, raised by
/// every operation that mints a record and models no quota error.
///
/// Which modelled error reports a reached ceiling follows from the operation's own error
/// set: an operation that models `ServiceQuotaExceededException` (402) reports it there
/// (`CreateGraph`, `CreateGraphSnapshot`, `RestoreGraphFromSnapshot`,
/// `CreateGraphUsingImportTask`, `CreatePrivateGraphEndpoint`), through
/// [`store_ceiling_quota_exceeded`], and an operation that
/// models none reports it as the conflict it is — the request cannot be carried out
/// against what the store already holds — through the `ConflictException` (409) it does
/// model. A full store is never a `ValidationException`: every member of such a request
/// is well formed and accepted, so nothing about the INPUT was invalid, and a reason
/// value that names a bad input would tell a caller to change a request that has nothing
/// wrong with it.
///
/// Worded by [`store_ceiling_clause`], so the ceiling reads the same whichever store was
/// full.
pub fn store_ceiling_conflict(records: &str, ceiling: usize) -> ConflictException {
    ConflictException {
        message: store_ceiling_clause(records, ceiling),
        // The modelled `reason` enum carries one value, `CONCURRENT_MODIFICATION`
        // (`smithy_query(query="shape",shape="ConflictExceptionReason")`), and a store
        // that is simply full is not a concurrent modification of anything, so the
        // member is left absent.
        reason: None,
    }
}

/// The modelled `ServiceQuotaExceededException` for a growth call that met a FULL store,
/// raised by every operation that mints a record and DOES model a quota error — the 402
/// half of the rule [`store_ceiling_conflict`] states for the operations that model none.
///
/// `records` names what the ceiling counts and `ceiling` is the bound the caller's guard
/// read, so the clause always reports the bound that was actually met; `resource_type`
/// names the kind of record that did not fit, which the modelled member asks for ("The
/// type of the resource that exceeded quota. Ex: Graph, Snapshot" —
/// `smithy_query(query="shape",shape="ServiceQuotaExceededException")`).
///
/// Worded by [`store_ceiling_clause`], so the ceiling reads the same whichever store was
/// full and whichever operation met it.
pub fn store_ceiling_quota_exceeded(
    records: &str,
    ceiling: usize,
    resource_type: &str,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: store_ceiling_clause(records, ceiling),
        // The store as a whole is what has no room left; no stored record is the one
        // that exceeded anything, and the record that did not fit was never minted, so
        // there is no identifier to name.
        resource_id: None,
        resource_type: Some(resource_type.to_string()),
        // The service whose ceiling was met is this one.
        service_code: Some(ARN_NAMESPACE.to_string()),
        // These ceilings are this emulator's own resource guards rather than quotas AWS
        // publishes (see [`MAX_GRAPHS`]), so there is no Service Quotas code to name.
        quota_code: None,
    }
}

/// The ONE grammar this service words a refused precondition with: what the call could
/// not do, and the condition on a stored record that held it back.
///
/// `action` is what the service could not do, `subject` is what the condition speaks
/// about — "it" where `action` already named that record, and the record's kind where
/// the call was asking for something else — and `condition` is the condition that held
/// the request back. No half of it names a resource identifier: every one of these calls
/// addressed its record by an identifier the caller sent, so the facts the caller does
/// not already hold are the action and the condition. The modelled shape carries only
/// `message` and `reason`
/// (`smithy_query(query="shape",shape="ConflictException")`), so the message is the only
/// place any of them can go. Written once here and reached by every precondition gate in
/// this service, so no two of them word the same refusal differently.
fn precondition_conflict(action: &str, subject: &str, condition: &str) -> ConflictException {
    ConflictException {
        message: format!("Unable to {action} because {subject} {condition}."),
        // The modelled `reason` enum carries one value, `CONCURRENT_MODIFICATION`
        // (`smithy_query(query="shape",shape="ConflictExceptionReason")`), and a record
        // that simply does not meet a precondition is not a concurrent modification of
        // anything, so the member is left absent.
        reason: None,
    }
}

/// The [`precondition_conflict`] condition of the commonest precondition of all: the
/// status the stored record was in when the call reached it.
fn status_condition(status: &str) -> String {
    format!("is in the {status} state")
}

/// The modelled `ConflictException` for a call that would transition THE GRAPH ITSELF
/// (`StartGraph`, `StopGraph`, `UpdateGraph`, `DeleteGraph`, `ResetGraph`) while the
/// graph's current status does not permit that transition.
///
/// `action` is the verb of the operation asking for the transition ("start", "stop",
/// "update", "delete", "reset"), and `status` is the status that held the request back.
/// The service words this refusal as what it could not do and the state that stopped it
/// — and nothing else: the graph was addressed by an identifier the caller sent, so the
/// refusal does not front it, and the current status is the one fact the caller does
/// not already hold. Worded by the one template
/// [`precondition_conflict`] every state gate here reaches, so no two of them word the
/// same refusal differently.
pub fn graph_state_conflict(action: &str, status: GraphStatus) -> ConflictException {
    precondition_conflict(
        &format!("{action} the graph"),
        "it",
        &status_condition(status.as_wire()),
    )
}

/// The modelled `ConflictException` for a call on ANOTHER resource whose precondition is
/// the status of the graph it names (`StartImportTask`, `StartExportTask`,
/// `CreateGraphSnapshot`, `CreatePrivateGraphEndpoint`), made while that graph's status
/// does not meet it.
///
/// `action` is the whole phrase of what the call could not do ("start the import task",
/// "create the graph snapshot"), because the refusal is not about a transition of the
/// graph but about something else the graph had to be serving for, and `status` is the
/// status that held the request back. Worded by the same template
/// [`graph_state_conflict`] words the graph's own transitions with, with the graph as
/// the subject of the condition rather than of the action, and for the same reasons: the
/// graph was addressed by an identifier the caller sent, so the refusal does not front
/// it. Written once here and reached by every one of those gates, so no two of them word
/// the same refusal differently.
pub fn graph_status_precondition_conflict(action: &str, status: GraphStatus) -> ConflictException {
    precondition_conflict(action, "the graph", &status_condition(status.as_wire()))
}

/// The modelled `ValidationException` for a call on the graph's OWN data plane — an
/// operation the model binds to the host prefix `{graphIdentifier}.`
/// (`GetGraphSummary`, `ListQueries`, `GetQuery`, `CancelQuery`) — made while that
/// graph is not serving ([`GraphStatus::is_serving`]).
///
/// `action` is what the endpoint had nothing to do it with, worded from the operation
/// asking ("summarise", "list queries on"), and `status` is the status that was read.
/// None of those operations declares a `ConflictException`
/// (`smithy_query(query="operation",operation="GetGraphSummary,ListQueries,GetQuery,CancelQuery")`
/// -> errors), so the one client error they all do model carries the refusal, and it
/// names the graph as well as the status: the request addressed the graph as the HOST
/// it was sent to, so which graph was not serving is a fact the caller needs back.
/// Written once here and reached by every one of those gates, so no two of them word
/// the same refusal differently.
pub fn graph_data_plane_unavailable(
    graph_id: &str,
    status: GraphStatus,
    action: &str,
) -> ValidationException {
    ValidationException {
        message: format!(
            "graph {graph_id} is in status {} and has no serving data plane to {action}",
            status.as_wire()
        ),
        // The other modelled `reason` values all name something wrong with the INPUT —
        // malformed, illegal or oversized — or one cancelled query
        // (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`), and a well-formed
        // request the graph's state leaves nothing to answer is none of those, so it is
        // reported as the bad request it is.
        reason: Some("BAD_REQUEST".to_string()),
    }
}

/// The modelled `ConflictException` for an import into a graph that still HOLDS DATA.
///
/// `StartImportTask` states two preconditions on the graph it loads into — "The graph
/// needs to be empty and in the AVAILABLE state"
/// (`smithy_query(query="doc",shape="StartImportTask")`) — and this is the refusal of the
/// emptiness half, the sibling of the status half [`graph_status_precondition_conflict`]
/// words: a bulk load FILLS a graph rather than merging into the data it already holds,
/// so a graph holding any node or edge ([`GraphData::is_empty`]) cannot take one. Worded
/// by the one template [`precondition_conflict`] this service refuses a precondition
/// with — what it could not do and the condition that held it back, and no resource
/// identifier, since the graph was addressed by an identifier the caller sent — so the
/// two halves of that one documented sentence read as the one sentence they are. Written
/// once here so every operation that needs an empty target graph words this refusal the
/// same way.
pub fn graph_not_empty_conflict() -> ConflictException {
    precondition_conflict("start the import task", "the graph", "is not empty")
}

/// The modelled `reason` a well-formed request the stored state leaves nothing to do is
/// reported under: none of the other modelled values names something wrong with the
/// INPUT's form (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES` — malformed,
/// illegal, oversized, a cancelled query), so a request this service can read and still
/// cannot serve is the bad request it is.
const BAD_REQUEST_REASON: &str = "BAD_REQUEST";

/// The modelled `ValidationException` (HTTP 400) for a `DeleteGraph` call on a graph
/// whose stored `deletionProtection` flag is set — "Graphs cannot be deleted if
/// delete-protection is enabled." (`smithy_query(query="doc",shape="DeleteGraph")`).
///
/// Deletion protection is a SETTING of the addressed graph that the caller clears with
/// `UpdateGraph`, not a transition in flight, so the refusal is the permanent
/// input/configuration failure the service answers it as rather than the retryable
/// `ConflictException` (409) a state still settling gets ([`graph_state_conflict`]) —
/// which is what lets a client tell "fix the configuration and retry" from "wait and
/// retry". The sentence names the graph that was not deleted, so a caller deleting
/// several graphs learns which one is protected.
pub fn graph_deletion_protected_validation(graph_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Graph [{graph_id}] cannot be deleted because deletion protection is enabled."
        ),
        reason: Some(BAD_REQUEST_REASON.to_string()),
    }
}

/// The modelled `ValidationException` (HTTP 400) for a `CreateGraphSnapshot` call whose
/// source graph is not in a status a snapshot can be taken from.
///
/// A snapshot is asked for on ANOTHER resource than the graph — the snapshot being
/// minted — so the graph's status is a precondition on the request rather than a
/// transition of the graph the call competes with, and the service reports it as the
/// permanent input failure it is: the caller has to wait for the graph and send the
/// request again. The sentence names the graph the snapshot would have been taken from,
/// which is the fact a caller snapshotting several graphs does not already hold, and
/// states the status a snapshot IS served from.
pub fn graph_not_available_for_snapshot(graph_id: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Graph [{graph_id}] is not available for snapshot. You can only create a \
             snapshot for an AVAILABLE Graph."
        ),
        reason: Some(BAD_REQUEST_REASON.to_string()),
    }
}

/// The modelled `ConflictException` for a call on a GRAPH SNAPSHOT (`DeleteGraphSnapshot`,
/// `RestoreGraphFromSnapshot`) whose current status does not permit what was asked.
///
/// `action` is the verb of the operation ("delete", "restore"), and `status` is the
/// status that held the request back. Worded by the same template
/// [`graph_state_conflict`] words the graph's half of this rule with, and for the same
/// reasons: the snapshot was addressed by an identifier the caller sent, so the refusal
/// does not front it, and the current status is the one fact the caller does not already
/// hold. Written once here and reached by every snapshot state gate, so no two of them
/// word the same refusal differently.
pub fn snapshot_state_conflict(action: &str, status: SnapshotStatus) -> ConflictException {
    precondition_conflict(
        &format!("{action} the graph snapshot"),
        "it",
        &status_condition(status.as_wire()),
    )
}

/// The modelled `ConflictException` for a call on an IMPORT TASK (`CancelImportTask`)
/// whose current status does not permit what was asked.
///
/// `action` is the verb of the operation ("cancel"), and `status` is the status that
/// held the request back — the status the task had settled into by the time the call
/// reached it. Worded by the same template [`graph_state_conflict`] words the graph's
/// half of this rule with, and for the same reasons: the task was addressed by an
/// identifier the caller sent, so the refusal does not front it.
pub fn import_task_state_conflict(action: &str, status: ImportTaskStatus) -> ConflictException {
    precondition_conflict(
        &format!("{action} the import task"),
        "it",
        &status_condition(status.as_wire()),
    )
}

/// The modelled `ConflictException` for a call on an EXPORT TASK (`CancelExportTask`)
/// whose current status does not permit what was asked — the export namespace's half of
/// the rule [`import_task_state_conflict`] states for imports, worded by the same
/// template for the same reasons.
pub fn export_task_state_conflict(action: &str, status: ExportTaskStatus) -> ConflictException {
    precondition_conflict(
        &format!("{action} the export task"),
        "it",
        &status_condition(status.as_wire()),
    )
}

/// The modelled `ConflictException` for a call on a PRIVATE GRAPH ENDPOINT
/// (`DeletePrivateGraphEndpoint`) whose current status does not permit what was asked.
///
/// `action` is the verb of the operation ("delete"), and `status` is the status that
/// held the request back. Worded by the same template [`graph_state_conflict`] words the
/// graph's half of this rule with, and for the same reasons: the endpoint was addressed
/// by the graph identifier and VPC id the caller sent, so the refusal does not front
/// either of them.
pub fn private_graph_endpoint_state_conflict(
    action: &str,
    status: PrivateGraphEndpointStatus,
) -> ConflictException {
    precondition_conflict(
        &format!("{action} the private graph endpoint"),
        "it",
        &status_condition(status.as_wire()),
    )
}

/// The modelled `ConflictException` for a `CreatePrivateGraphEndpoint` whose graph and
/// VPC already hold an endpoint.
///
/// `(graphIdentifier, vpcId)` is the identity `GetPrivateGraphEndpoint` and
/// `DeletePrivateGraphEndpoint` address an endpoint by, so a second create of that pair
/// asks for a record the store already holds. Worded by the one template
/// [`precondition_conflict`] this service refuses a precondition with, and for the same
/// reasons: the endpoint was addressed by the graph identifier and VPC id the caller
/// sent, so the refusal fronts neither of them, and the `vpcEndpointId` this service
/// minted for the stored endpoint is a value the request never named, so it is not
/// reported either.
pub fn private_graph_endpoint_exists_conflict() -> ConflictException {
    precondition_conflict("create the private graph endpoint", "it", "already exists")
}

/// The member every operation of this service binds a `TagMap` to, which is the member
/// path every refusal of a tag bound is reported at.
const TAGS_MEMBER: &str = "tags";

/// Refuse more tag entries on one resource than it may hold ([`MAX_TAGS`]).
///
/// `total` is the number of entries the record would hold AFTER the call, so
/// `TagResource` checks the merge of what is stored with what it was sent rather than
/// the request alone. `Err` carries the CLAUSE that states the broken bound, for the
/// caller's operation to report inside the one envelope every constraint refusal of this
/// service is counted in.
///
/// That clause is the one the boundary validator renders for the same `@length` on the
/// same member — the entry count as the collection's display form, and the bound as the
/// requirement — so a caller cannot tell the bound measured on a merge from the bound
/// measured on a request.
pub fn bound_tag_count(total: usize) -> Result<(), String> {
    if total > MAX_TAGS {
        return Err(constraint_clause(
            TAGS_MEMBER,
            Some(&format!("{total} entries")),
            &max_length_requirement(MAX_TAGS),
        ));
    }
    Ok(())
}

/// Refuse a `tags` map a request may not store: more entries than one record may hold
/// ([`MAX_TAGS`]), a key or value longer than its modelled bound ([`MAX_TAG_KEY_CHARS`] /
/// [`MAX_TAG_VALUE_CHARS`] characters), or a key or value in the namespace AWS reserves
/// for its own tags ([`RESERVED_TAG_PREFIX`]).
///
/// Reached by every operation that accepts a `tags` map (`CreateGraph`,
/// `CreateGraphSnapshot`, `CreateGraphUsingImportTask`, `RestoreGraphFromSnapshot`,
/// `StartExportTask`, `TagResource`), so what a caller's tags must satisfy is stated
/// once. The model's own `@length` traits on `TagMap`/`TagKey`/`TagValue` refuse the
/// entry count and the values at the boundary too; the check is repeated at the growth
/// site so the bound lives beside the write that stores it, and a key is bounded HERE
/// alone, because the boundary's walk treats a map key as the entry's name rather than
/// as a constrained value. The reserved namespace carries no modelled trait on either
/// half, so this is the only place it is refused at all.
///
/// Every `Err` is a CLAUSE in the one form this service states a constraint violation
/// in: the entry count through [`bound_tag_count`]; a KEY against the member the map sits
/// at, listing everything a key of that map has to satisfy ([`map_key_requirement`]),
/// because no boundary clause for a key exists to read like; and a VALUE at the entry's
/// own path, which is where the boundary states a constraint an entry's value broke. A
/// key or value that broke a length bound is exactly what must not be echoed, so those
/// clauses state the member and the requirement and nothing else ([`length_clause`]); a
/// half refused for the reserved prefix is short and is what the caller has to change,
/// so that clause carries it.
///
/// The keys are checked before the values, and the offending entry is the first in key
/// order, so one map that breaks either bound twice is refused identically every time
/// rather than by whichever entry the map happened to yield first.
pub fn bound_tags(tags: &HashMap<String, String>) -> Result<(), String> {
    bound_tag_count(tags.len())?;
    if tags.keys().any(|key| key.chars().count() > MAX_TAG_KEY_CHARS) {
        return Err(length_clause(
            TAGS_MEMBER,
            &map_key_requirement(&[
                max_length_requirement(MAX_TAG_KEY_CHARS),
                min_length_requirement(MIN_TAG_KEY_CHARS),
                pattern_requirement(TAG_KEY_PATTERN),
            ]),
        ));
    }
    let oversized_value = tags
        .iter()
        .filter(|(_, value)| value.chars().count() > MAX_TAG_VALUE_CHARS)
        .map(|(key, _)| key)
        .min();
    if let Some(key) = oversized_value {
        return Err(length_clause(
            &format!("{TAGS_MEMBER}.{key}"),
            &max_length_requirement(MAX_TAG_VALUE_CHARS),
        ));
    }
    // A tag in the reserved namespace is one AWS applies itself, so neither half of an
    // entry a caller sends may carry the prefix. Reported like the bounds above: a key
    // against the member the map sits at, a value at its own entry's path, and the
    // first offender in key order either way.
    let reserved_key = tags.keys().filter(|key| is_reserved_tag_text(key)).min();
    if let Some(key) = reserved_key {
        return Err(constraint_clause(
            TAGS_MEMBER,
            Some(key),
            &map_key_requirement(&[reserved_prefix_requirement()]),
        ));
    }
    let reserved_value = tags
        .iter()
        .filter(|(_, value)| is_reserved_tag_text(value))
        .min_by_key(|(key, _)| *key);
    if let Some((key, value)) = reserved_value {
        return Err(constraint_clause(
            &format!("{TAGS_MEMBER}.{key}"),
            Some(value),
            &reserved_prefix_requirement(),
        ));
    }
    Ok(())
}

/// One clause of a `ValidationException` message: the value that was rejected, the
/// member path it sat at, and the requirement it failed.
///
/// `value` is `None` for a member that was ABSENT, which is reported as the null it was
/// — a fact of the request's shape rather than a rendering of something the caller sent.
pub fn constraint_clause(member_path: &str, value: Option<&str>, requirement: &str) -> String {
    let subject = match value {
        Some(shown) => format!("Value '{shown}'"),
        None => "Value null".to_string(),
    };
    format!("{subject} at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// One clause of a `ValidationException` message for a LENGTH violation: the member
/// path and the requirement it failed, WITHOUT the value.
///
/// A value that broke a length bound is exactly the value that must not be echoed — a
/// refusal would otherwise carry the whole oversized string back — so this clause
/// states the member and the bound and nothing else.
pub fn length_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The requirement clause for a `@pattern` violation: the expression the value had to
/// satisfy, rendered WITHOUT the anchors the model spells it with (see [`unanchored`]).
pub fn pattern_requirement(pattern: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    )
}

/// The requirement clause for a MINIMUM-VALUE violation: the smallest value the member
/// accepts.
///
/// Worded here rather than at each call site so a bound the boundary validator refuses
/// from the model and one an operation enforces itself read identically. `min` is taken
/// as anything printable because the bound a model declares and the bound an operation
/// states are not always the same numeric type.
pub fn min_value_requirement(min: impl ::std::fmt::Display) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement clause for a MAXIMUM-VALUE violation: the largest value the member
/// accepts.
///
/// The upper-bound sibling of [`min_value_requirement`], and here for the same reason: a
/// bound the boundary validator refuses from the model and one an operation enforces
/// because the member's documentation publishes it read identically, so a caller cannot
/// tell which side of the boundary measured the value.
pub fn max_value_requirement(max: impl ::std::fmt::Display) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement clause for a MINIMUM-LENGTH violation: the shortest the member
/// accepts.
///
/// The sibling of [`min_value_requirement`], and here for the same reason: a bound the
/// boundary validator refuses from the model and one an operation enforces itself are
/// worded by the one builder, so neither side of the boundary can drift from the other.
pub fn min_length_requirement(min: impl ::std::fmt::Display) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement clause for a MAXIMUM-LENGTH violation: the longest the member
/// accepts.
///
/// The upper-bound sibling of [`min_length_requirement`]. The clause states the bound
/// the value had to satisfy and NOT the length it had: a requirement is what the member
/// accepts, and measuring the rejected value back to the caller is what the value half
/// of a clause is for.
pub fn max_length_requirement(max: impl ::std::fmt::Display) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement clause for a constraint on the KEYS of a map member: everything a key
/// of that map has to satisfy, listed inside the clause that says it is the map's keys
/// that carry those requirements.
///
/// A map's keys reach no boundary refusal at all — the walk in front of a handler treats
/// a key as the entry's name rather than as a constrained value — so the handler is where
/// a key's own constraints are applied, and the clause lists the key shape's WHOLE
/// declared set rather than only the bound that happened to break: the set is what a key
/// must satisfy, and a caller fixing one key wants all of it.
pub fn map_key_requirement(requirements: &[String]) -> String {
    format!("Map keys must satisfy constraint: [{}]", requirements.join(", "))
}

/// The requirement clause for a half of a tag entry drawn from the namespace AWS keeps
/// for its own tags: the prefix it may not open with.
///
/// Worded by a builder like every other requirement this service states, so the rule
/// reads the same whichever half of an entry broke it and whichever operation carried
/// the tags. See [`RESERVED_TAG_PREFIX`] for where the rule comes from.
pub fn reserved_prefix_requirement() -> String {
    format!("Member must not be prefixed with {RESERVED_TAG_PREFIX}")
}

/// The `ValidationException` message over one or more clauses: how many constraints were
/// violated, then the clauses themselves.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The modelled `reason` a refused CONSTRAINT is classified by: the value of
/// `ValidationExceptionReason` (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`)
/// that names a member whose value does not satisfy a rule stated about it, as opposed to
/// an argument the operation does not accept (`ILLEGAL_ARGUMENT`) or a well-formed request
/// the service's state leaves nothing to answer (`BAD_REQUEST`).
const CONSTRAINT_VIOLATION: &str = "CONSTRAINT_VIOLATION";

/// The modelled `ValidationException` for a request that broke one or more constraints on
/// its input, over the clauses that say which.
///
/// The ONE builder a refused input constraint of this service is reported through, reached
/// by both sides of the request boundary: by the walk in front of a handler, for a
/// constraint the model declares (`crate::service::constraints`), and by a handler, for one
/// the model cannot express. So a caller cannot tell which side caught the violation — the
/// message is the one envelope over the one set of clause builders, and the classification
/// is the same modelled `reason` either way.
pub fn constraint_violation(clauses: &[String]) -> ValidationException {
    ValidationException {
        message: validation_envelope(clauses),
        reason: Some(CONSTRAINT_VIOLATION.to_string()),
    }
}

/// A `@pattern` expression without the anchors a model may spell it with: the
/// requirement reported is the expression a value must satisfy, not the anchoring the
/// model uses to make the match total.
///
/// Only the expression's OUTER anchors go: an anchor inside the expression (one per
/// branch of an alternation, say) is part of its own structure, and dropping it would
/// change which strings the reported expression describes. A `$` escaped by a backslash
/// is a literal dollar sign rather than the end anchor, so it stays.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// `36^10` — the number of distinct values ten `[a-z0-9]` characters can spell.
const BASE36_POW10: u64 = 36u64.pow(10);

/// Spread consecutive counter values over the id space (SplitMix64's finalizer), so
/// ids are not guessable neighbours while still being a pure function of the
/// counter — the same sequence of calls against a fresh store mints the same ids.
fn scramble(seed: u64) -> u64 {
    let mut z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x6A09_E667_F3BC_C909);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Spell the VPC endpoint id of the `value`th endpoint this store minted, matching the
/// modelled `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`: seventeen lowercase hex
/// digits, scrambled off the counter the same way [`encode_resource_id`]'s input is, so
/// ids are not guessable neighbours while still being a pure function of the counter.
///
/// Seventeen hex digits are 68 bits, one more than a `u64` carries, so the leading digit
/// comes from a second, independently scrambled word.
fn encode_vpc_endpoint_id(counter: u64) -> String {
    let low = scramble(counter);
    let high = scramble(counter ^ 0x5DEE_CE66_D1B5_A1C3);
    format!("vpce-{:x}{low:016x}", high & 0xf)
}

/// Spell the id of the `counter`th query this store admitted in the 8-4-4-4-12
/// hexadecimal form a query id takes, from three independently scrambled words of the
/// counter: the model constrains the member not at all, so what identifies a query is
/// whatever the operation that admitted it minted, and this spelling keeps one id per
/// counter value without making neighbours guessable. The version and variant nibbles
/// the form fixes are set, so a minted id is indistinguishable in shape from the ids the
/// service answers with.
fn encode_query_id(counter: u64) -> String {
    let first = scramble(counter);
    let second = scramble(counter ^ 0x2545_F491_4F6C_DD1D);
    let third = scramble(counter ^ 0x9E37_79B9_7F4A_7C15);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        first >> 32,
        (first >> 16) & 0xffff,
        first & 0x0fff,
        0x8000 | (second & 0x3fff),
        third & 0xffff_ffff_ffff
    )
}

/// Spell `value` (already reduced below [`BASE36_POW10`]) as `prefix` plus exactly
/// ten base-36 digits, matching this service's resource-id patterns (`GraphId`'s
/// `^g-[a-z0-9]{10}$`, `SnapshotId`'s `^gs-[a-z0-9]{10}$`).
fn encode_resource_id(prefix: &str, mut value: u64) -> String {
    let mut digits = ['0'; 10];
    for slot in digits.iter_mut().rev() {
        // `from_digit` is total for radix 36 and a remainder below 36, and yields
        // lowercase letters above 9; the fallback keeps a handler panic-free.
        *slot = char::from_digit((value % 36) as u32, 36).unwrap_or('0');
        value /= 36;
    }
    let mut id = String::with_capacity(prefix.len() + digits.len());
    id.push_str(prefix);
    id.extend(digits);
    id
}

/// The `*Output` shapes that carry a whole graph record are structurally
/// identical, so one macro renders the stored graph into each rather than one
/// hand-copied field list per operation, which could drift apart.
macro_rules! graph_record_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&Graph> for crate::ops::types::$ty {
            fn from(graph: &Graph) -> Self {
                Self {
                    id: graph.id.clone(),
                    name: graph.name.clone(),
                    arn: graph.arn.clone(),
                    status: Some(graph.status.as_wire().to_string()),
                    status_reason: graph.status_reason.clone(),
                    create_time: Some(::aws_smithy_types::DateTime::from(graph.create_time)),
                    // ONE stored compute configuration, reported through both of the
                    // members that name it: the current one, and the deprecated member
                    // that carries a provisioned graph's `memory` and a graph session's
                    // `minMemory` (see `GraphCompute::reported_memory`).
                    provisioned_memory: graph.compute.reported_memory(),
                    compute_configuration: Some(graph.compute.as_wire()),
                    endpoint: Some(graph.endpoint.clone()),
                    public_connectivity: Some(graph.public_connectivity),
                    vector_search_configuration: graph.vector_search_configuration.clone(),
                    replica_count: Some(graph.replica_count),
                    kms_key_identifier: graph.kms_key_identifier.clone(),
                    source_snapshot_id: graph.source_snapshot_id.clone(),
                    deletion_protection: Some(graph.deletion_protection),
                    build_number: Some(graph.build_number.clone()),
                }
            }
        }
    )+};
}

graph_record_output!(
    CreateGraphOutput,
    GetGraphOutput,
    UpdateGraphOutput,
    DeleteGraphOutput,
    ResetGraphOutput,
    StopGraphOutput,
    StartGraphOutput,
    RestoreGraphFromSnapshotOutput,
);

/// `GraphSummary` is the listing projection of the same record: it carries no
/// `createTime`, `statusReason`, `sourceSnapshotId`, `buildNumber` or
/// `vectorSearchConfiguration`.
impl From<&Graph> for crate::ops::types::GraphSummary {
    fn from(graph: &Graph) -> Self {
        Self {
            id: graph.id.clone(),
            name: graph.name.clone(),
            arn: graph.arn.clone(),
            status: Some(graph.status.as_wire().to_string()),
            provisioned_memory: graph.compute.reported_memory(),
            compute_configuration: Some(graph.compute.as_wire()),
            public_connectivity: Some(graph.public_connectivity),
            endpoint: Some(graph.endpoint.clone()),
            replica_count: Some(graph.replica_count),
            kms_key_identifier: graph.kms_key_identifier.clone(),
            deletion_protection: Some(graph.deletion_protection),
        }
    }
}

/// The shapes that carry a whole snapshot record are structurally identical, so one
/// macro renders the stored snapshot into each rather than one hand-copied field list
/// per operation, which could drift apart. `GraphSnapshotSummary` — the member
/// `ListGraphSnapshots` reports — carries exactly those members too, so it is rendered
/// here rather than by a listing projection of its own.
macro_rules! snapshot_record_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&Snapshot> for crate::ops::types::$ty {
            fn from(snapshot: &Snapshot) -> Self {
                Self {
                    id: snapshot.id.clone(),
                    name: snapshot.name.clone(),
                    arn: snapshot.arn.clone(),
                    source_graph_id: Some(snapshot.source_graph_id.clone()),
                    snapshot_create_time: Some(::aws_smithy_types::DateTime::from(
                        snapshot.snapshot_create_time,
                    )),
                    status: Some(snapshot.status.as_wire().to_string()),
                    kms_key_identifier: snapshot.kms_key_identifier.clone(),
                }
            }
        }
    )+};
}

snapshot_record_output!(
    CreateGraphSnapshotOutput,
    GetGraphSnapshotOutput,
    DeleteGraphSnapshotOutput,
    GraphSnapshotSummary,
);

/// The `*Output` shapes that carry a whole import-task record differ only in the
/// members they add, so the members they SHARE are rendered by one macro rather than
/// one hand-copied field list per operation, which could drift apart. The listing
/// projection (`ImportTaskSummary`) is rendered here too, so a listed task cannot drift
/// from what a single-task read answers; `GetImportTaskOutput` is built by
/// [`ImportTask::get_output`] instead, because it also carries the `importTaskDetails`
/// that are derived from the clock.
///
/// Each entry names the binding the stored record is reached through, because macro
/// hygiene puts a binding the macro itself introduces out of reach of the extra
/// members an entry spells.
macro_rules! import_task_record_output {
    ($($ty:ident ($task:ident) { $($extra:tt)* }),+ $(,)?) => {$(
        impl From<&ImportTask> for crate::ops::types::$ty {
            fn from($task: &ImportTask) -> Self {
                Self {
                    task_id: $task.id.clone(),
                    graph_id: Some($task.graph_id.clone()),
                    source: $task.source.clone(),
                    format: $task.format.clone(),
                    parquet_type: $task.parquet_type.clone(),
                    role_arn: $task.role_arn.clone(),
                    status: $task.status.as_wire().to_string(),
                    $($extra)*
                }
            }
        }
    )+};
}

import_task_record_output!(
    // `CancelImportTaskOutput` carries only the shared members.
    CancelImportTaskOutput(task) {},
    // `ImportTaskSummary`, the entry `ListImportTasks` reports, is exactly the shared
    // member set (`smithy_query(query="shape",shape="ImportTaskSummary")`), so a listed
    // task cannot differ from what a read of it answers.
    ImportTaskSummary(task) {},
    StartImportTaskOutput(task) {
        import_options: task.import_options.clone(),
    },
    // `CreateGraphUsingImportTask` reports the task it stored, with the graph it
    // created named by the shared `graphId` member.
    CreateGraphUsingImportTaskOutput(task) {
        import_options: task.import_options.clone(),
    },
);

/// The `*Output` shapes that carry a whole export-task record differ only in the
/// members they add, so the members they SHARE are rendered by one macro rather than
/// one hand-copied field list per operation, which could drift apart. The listing
/// projection (`ExportTaskSummary`) is rendered here too, so a listed task cannot drift
/// from what a single-task read answers;
/// `GetExportTaskOutput` is built by [`ExportTask::get_output`] instead, because it also
/// carries the `exportTaskDetails` that are derived from the clock.
///
/// Each entry names the binding the stored record is reached through, because macro
/// hygiene puts a binding the macro itself introduces out of reach of the extra
/// members an entry spells.
macro_rules! export_task_record_output {
    ($($ty:ident ($task:ident) { $($extra:tt)* }),+ $(,)?) => {$(
        impl From<&ExportTask> for crate::ops::types::$ty {
            fn from($task: &ExportTask) -> Self {
                Self {
                    graph_id: $task.source_graph_id.clone(),
                    role_arn: $task.role_arn.clone(),
                    task_id: $task.id.clone(),
                    status: $task.status.as_wire().to_string(),
                    format: $task.format.clone(),
                    destination: $task.destination.clone(),
                    kms_key_identifier: $task.kms_key_identifier.clone(),
                    parquet_type: $task.parquet_type.clone(),
                    status_reason: $task.status_reason.clone(),
                    $($extra)*
                }
            }
        }
    )+};
}

export_task_record_output!(
    StartExportTaskOutput(task) {
        export_filter: task.export_filter.clone(),
    },
    // `CancelExportTaskOutput` carries only the shared members: the model gives it no
    // `exportFilter`.
    CancelExportTaskOutput(task) {},
    // `ExportTaskSummary`, the entry `ListExportTasks` reports, is exactly the shared
    // member set (`smithy_query(query="shape",shape="ExportTaskSummary")`), so a listed
    // task cannot differ from what a read of it answers.
    ExportTaskSummary(task) {},
);

/// The shapes that carry a whole private-graph-endpoint record are structurally
/// identical, so one macro renders the stored endpoint into each rather than one
/// hand-copied field list per operation, which could drift apart. The three `*Output`
/// shapes of the single-endpoint operations are joined here by
/// `PrivateGraphEndpointSummary`, the entry `ListPrivateGraphEndpoints` reports, so a
/// listed endpoint cannot differ from what a read of it answers.
macro_rules! private_graph_endpoint_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl From<&PrivateGraphEndpoint> for crate::ops::types::$ty {
            fn from(endpoint: &PrivateGraphEndpoint) -> Self {
                Self {
                    vpc_id: endpoint.vpc_id.clone(),
                    subnet_ids: endpoint.subnet_ids.clone(),
                    status: endpoint.status.as_wire().to_string(),
                    vpc_endpoint_id: Some(endpoint.vpc_endpoint_id.clone()),
                }
            }
        }
    )+};
}

private_graph_endpoint_output!(
    CreatePrivateGraphEndpointOutput,
    GetPrivateGraphEndpointOutput,
    DeletePrivateGraphEndpointOutput,
    PrivateGraphEndpointSummary,
);

/// One stored [`Query`] rendered into the shape an operation reports it with, as of
/// `now`.
///
/// A query's projection carries two members measured off the CLOCK
/// ([`Query::waited_millis`], [`Query::elapsed_millis`]), so it takes `now` rather than
/// being a plain `From<&Query>` — the same reason [`ExportTask::get_output`] takes it.
pub trait QueryRecordOutput {
    /// This stored query as this shape reports it, measured against `now`.
    fn from_query(query: &Query, now: Instant) -> Self;
}

/// The shapes that carry a whole query record are structurally identical
/// (`smithy_query(query="shape",shape="GetQueryOutput,QuerySummary")`: both declare
/// exactly `id`, `queryString`, `waited`, `elapsed`, `state`), so one macro renders the
/// stored query into each rather than one hand-copied field list per operation, which
/// could drift apart. The listing projection (`QuerySummary`) is rendered here too, so a
/// listed query cannot differ from what a single-query read answers.
macro_rules! query_record_output {
    ($($ty:ident),+ $(,)?) => {$(
        impl QueryRecordOutput for crate::ops::types::$ty {
            fn from_query(query: &Query, now: Instant) -> Self {
                Self {
                    id: Some(query.id.clone()),
                    query_string: Some(query.query_string.clone()),
                    state: Some(query.state.as_wire().to_string()),
                    waited: Some(query.waited_millis(now)),
                    elapsed: Some(query.elapsed_millis(now)),
                }
            }
        }
    )+};
}

query_record_output!(GetQueryOutput, QuerySummary);

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNeptunegraphAmazonneptunegraphState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNeptunegraphAmazonneptunegraphState>)
}

/// Take the store, then read the clock — in that order — and answer both, so every
/// handler decides with the instant it HOLDS the store at.
///
/// Every lifecycle decision in this service compares an instant the handler supplies
/// against a due instant the record holds ([`Graph::settle`],
/// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::reap_deleted_graphs`],
/// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::reap_terminated_queries`],
/// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_snapshots`],
/// [`ComAmazonawsNeptunegraphAmazonneptunegraphState::materialise_private_endpoints`],
/// [`ExportTask::settle`], [`ImportTask::settle`]). A handler that read the clock
/// BEFORE it contended for the store would settle and reap the world as it stood when
/// it started waiting: a transition the clock made due during the wait would stay
/// invisible to it, and it would then answer — and write — from that stale view, so a
/// stop that has come due would still refuse a start, a freed name would still read as
/// taken, and a query whose timeout is out would still be reported as in flight.
/// Sampling under the guard makes a handler's instant the instant of the store it is
/// about to read, however long it waited to get there.
pub fn locked_store_now(
    state: &::engine::ServiceState<ComAmazonawsNeptunegraphAmazonneptunegraphState>,
) -> (
    ::std::sync::MutexGuard<'_, ComAmazonawsNeptunegraphAmazonneptunegraphState>,
    Instant,
) {
    let store = state.lock();
    let now = Instant::now();
    (store, now)
}

#[cfg(test)]
mod tests;

mod endpoint_network;
mod graph_compute;
mod import_formats;
mod import_load;
mod open_cypher;
mod open_cypher_updates;
mod query_authorization;

pub use endpoint_network::*;
pub use graph_compute::*;
pub use import_formats::*;
pub use import_load::*;
pub use open_cypher::*;
pub use open_cypher_updates::*;
pub use query_authorization::*;
