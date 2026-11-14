// SPDX-License-Identifier: CC0-1.0

//! The runs of a workflow: the stored run records and the statuses the clock owes them, the
//! graph a read projects beside a workflow or a run, and the data-quality rulesets and runs
//! this service's data-quality surface keeps.

use super::*;

/// The `WorkflowRunStatus` a run is in the instant `StartWorkflowRun` returns. One of the
/// modelled `WorkflowRunStatus` values (`RUNNING`, `COMPLETED`, `STOPPING`, `STOPPED`,
/// `ERROR`).
pub const WORKFLOW_RUN_STATUS_RUNNING: &str = "RUNNING";

/// The `WorkflowRunStatus` of a run that finished: "COMPLETED" names a run whose actions
/// have run their course, whatever each of them came to — what became of an individual
/// action is reported by the run's own statistics ([`WorkflowRunStatistics`]) and not by
/// this status. See [`WORKFLOW_RUN_STATUS_RUNNING`].
pub const WORKFLOW_RUN_STATUS_COMPLETED: &str = "COMPLETED";

/// The `WorkflowRunStatus` of a run that finished having errored. `WorkflowRun$ErrorMessage`
/// "describes any error that may have occurred in STARTING the workflow run", so this status
/// is what a run that could not be started comes to rest in — not one whose actions failed,
/// which is a completed run with failed actions. See [`WORKFLOW_RUN_STATUS_RUNNING`].
pub const WORKFLOW_RUN_STATUS_ERROR: &str = "ERROR";

/// The `WorkflowRunStatus` values a run never moves out of again: of the modelled enum
/// quoted on [`WORKFLOW_RUN_STATUS_RUNNING`], the ones that name a finished run. The other
/// two (`RUNNING`, `STOPPING`) each still owe a further status.
pub const TERMINAL_WORKFLOW_RUN_STATUSES: [&str; 3] = [
    WORKFLOW_RUN_STATUS_COMPLETED,
    "STOPPED",
    WORKFLOW_RUN_STATUS_ERROR,
];

/// How long a workflow run stays in the status the call that started it put it in before
/// the clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for a workflow run, so this is the
/// emulator's own span — deliberately not zero, so a caller that polls `GetWorkflowRun`
/// straight after `StartWorkflowRun` still observes the run in flight, and the concurrency
/// ceiling that goes with it. A run whose actions are still owed a state of their own waits
/// for them past this span
/// ([`ComAmazonawsGlueAwsglueState::settle_due_workflow_runs`]). The due instant is stored
/// on the run and materialised by whichever later call reads it; nothing sleeps.
pub const WORKFLOW_RUN_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The prefix a Glue workflow run id carries, which is what makes an `IdString`
/// recognisable as a workflow run id rather than any other id the service mints.
pub const WORKFLOW_RUN_ID_PREFIX: &str = "wr_";

/// How many runs one workflow keeps.
///
/// CHOSEN: Glue publishes no retention for workflow-run records, so this is the emulator's
/// own window, for the reason [`MAX_RUNS_PER_JOB`] gives. The OLDEST record is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_WORKFLOW: usize = 100;

/// Ceiling on the run properties ONE workflow run may hold.
///
/// This emulator's own capacity, NOT an AWS quota, for the reason
/// [`MAX_DATABASES_PER_CATALOG`] gives: `PutWorkflowRunProperties` adds to the map a run
/// holds and is reachable by any caller, so what one process will hold for a run is bounded
/// here. A put that would take the run past the ceiling is refused with that operation's
/// modelled `ResourceNumberLimitExceededException` rather than written.
pub const MAX_RUN_PROPERTIES_PER_RUN: usize = 100;

/// How many workflow run ids this process has minted. A nonce source for
/// [`new_workflow_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
pub(super) static MINTED_WORKFLOW_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a Glue workflow run id carries: four, i.e. 256 bits rendered as
/// 64 lowercase hexadecimal characters.
pub(super) const WORKFLOW_RUN_ID_WORDS: usize = 4;

/// A fresh `RunId`: the id the service assigns a workflow run when it starts.
///
/// [`WORKFLOW_RUN_ID_PREFIX`] followed by 64 lowercase hexadecimal characters, which is
/// inside `IdString`'s `length 1..255`. The bits come from [`mint_hex_id`], as a job run
/// id's do, so two runs started in the same nanosecond cannot be given the same id. It is
/// stored on the run, so every later read of that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_workflow_run_id() -> String {
    format!(
        "{WORKFLOW_RUN_ID_PREFIX}{}",
        mint_hex_id(&MINTED_WORKFLOW_RUN_IDS, WORKFLOW_RUN_ID_WORDS)
    )
}

/// One job run an action of a workflow run began: the job definition it is a run of and
/// the id that addresses it inside that definition ([`JobEntry::run`]).
///
/// A job run carries the trigger that started it (`JobRun$TriggerName`) but no workflow
/// run, so this pair is what ties the run an action began back to the workflow run that
/// initiated the action — which is what a `JOB` node of a run's graph reports and what the
/// run's statistics count.
#[derive(Debug, Clone)]
pub struct ActionJobRun {
    /// The name of the job definition the run belongs to.
    pub job_name: String,
    /// `JobRun$Id` of the run.
    pub run_id: String,
}

/// One run of a workflow, as `StartWorkflowRun` recorded it: "A workflow run is an
/// execution of a workflow providing all the runtime information" (`WorkflowRun`).
#[derive(Debug, Clone)]
pub struct WorkflowRunEntry {
    /// `WorkflowRun$WorkflowRunId`, minted by [`new_workflow_run_id`].
    pub id: String,
    /// `WorkflowRun$Name`: "Name of the workflow that was run."
    pub workflow_name: String,
    /// `WorkflowRun$WorkflowRunProperties`: "The workflow run properties which were set
    /// during the run" — the workflow's own `DefaultRunProperties` ("to be used as part of
    /// each execution of the workflow") with the start's `RunProperties` applied over them.
    pub run_properties: HashMap<String, String>,
    /// Server-set: the instant `StartWorkflowRun` recorded this run.
    pub started_on: SystemTime,
    /// Server-set once the run reaches a terminal status, and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `WorkflowRunStatus` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `WorkflowRunStatus` the clock owes this run at [`Self::settles_at`].
    pub settles_to: String,
    /// `WorkflowRun$ErrorMessage`, which a run carries only once something has gone wrong
    /// with it.
    pub error_message: Option<String>,
    /// `WorkflowRunStatistics$TotalActions`: "Total number of Actions in the workflow run",
    /// which are the actions the trigger that began the run initiates.
    pub total_actions: i32,
    /// The job runs this run's actions began, in the order they were initiated.
    ///
    /// Only the IDENTITY of each run is held here: its state, and everything else a read
    /// reports about it, is read out of the job definition the run belongs to, so a read of
    /// this workflow run reports what the clock has since made of each of its actions.
    pub action_job_runs: Vec<ActionJobRun>,
}

impl WorkflowRunEntry {
    /// Record a run of `workflow` whatever began it — a caller's `StartWorkflowRun`, or a
    /// trigger of the workflow that fired — in [`WORKFLOW_RUN_STATUS_RUNNING`] with its
    /// next status due [`WORKFLOW_RUN_SETTLES_AFTER`] out.
    ///
    /// `requested_properties` are the run properties asked for this run, and `None` when
    /// nothing asked for any. `total_actions` is how many actions the entry point that
    /// began this run initiates, which is the run's total action count.
    pub fn begin(
        workflow: &WorkflowEntry,
        requested_properties: Option<&HashMap<String, String>>,
        id: String,
        now: SystemTime,
        total_actions: i32,
    ) -> Self {
        // "A collection of properties to be used as part of each execution of the
        // workflow" is the base every run starts from, and "the workflow run properties
        // for the new workflow run" (`StartWorkflowRunRequest$RunProperties`) are this
        // run's own, so they win where both name a key.
        let mut run_properties = workflow.default_run_properties.clone().unwrap_or_default();
        if let Some(requested) = requested_properties {
            run_properties.extend(
                requested
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }

        Self {
            id,
            workflow_name: workflow.name.clone(),
            run_properties,
            started_on: now,
            completed_on: None,
            status: WORKFLOW_RUN_STATUS_RUNNING.to_string(),
            settles_at: Some(
                now.checked_add(WORKFLOW_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            // The actions a run initiates are runs of jobs this emulator really executes,
            // so the status the clock owes a run nobody stops is the one a run whose actions
            // have run their course comes to rest in; what each action came to is reported
            // by the run's statistics rather than by its status.
            //
            // An action naming a CRAWLER initiates no execution here — this crate records no
            // crawl — and nothing claims one ran: no counter of the run's statistics moves
            // for it ([`ComAmazonawsGlueAwsglueState::workflow_run_statistics`]) and the
            // crawl node of the graph reports no crawls, so a reader sees the action counted
            // among the run's total and no outcome asserted for it.
            settles_to: WORKFLOW_RUN_STATUS_COMPLETED.to_string(),
            // Nothing has gone wrong yet: a run carries an error message only once it has.
            error_message: None,
            total_actions,
            // The actions are initiated by the call that began this run, which records what
            // they began ([`ComAmazonawsGlueAwsglueState::record_workflow_run_job_runs`]).
            action_job_runs: Vec::new(),
        }
    }

    /// Whether this run has finished, by [`TERMINAL_WORKFLOW_RUN_STATUSES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_WORKFLOW_RUN_STATUSES.contains(&self.status.as_str())
    }

    /// How many properties this run would hold once `incoming` had been written over what
    /// it holds: the keys it already has, plus the ones the put brings that are new to it.
    pub fn properties_after(&self, incoming: &HashMap<String, String>) -> usize {
        self.run_properties.len()
            + incoming
                .keys()
                .filter(|property| !self.run_properties.contains_key(*property))
                .count()
    }

    /// Move this run into the status the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the status a call put it in at [`Self::settles_at`] whether
    /// or not anybody called, and the instants recorded are that DUE instant, not the
    /// instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = self.settles_to.clone();
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
    }

    /// Rebuild the modelled `WorkflowRun` shape from what is stored.
    ///
    /// `graph` is the graph the reader asked for with its own `IncludeGraph` ("Specifies
    /// whether to include the workflow graph in response or not"), handled as a workflow
    /// read handles it ([`WorkflowEntry::to_workflow`]). `statistics` are this run's
    /// statistics, which count the runs its actions began and so are computed by the STORE
    /// that holds those runs
    /// ([`ComAmazonawsGlueAwsglueState::workflow_run_statistics`]) rather than here.
    pub fn to_workflow_run(
        &self,
        graph: Option<WorkflowGraph>,
        statistics: WorkflowRunStatistics,
    ) -> WorkflowRun {
        WorkflowRun {
            name: Some(self.workflow_name.clone()),
            workflow_run_id: Some(self.id.clone()),
            // "The ID of the previous workflow run": a run a start began follows no run,
            // so there is none to report.
            previous_run_id: None,
            workflow_run_properties: Some(self.run_properties.clone()),
            started_on: Some(to_date_time(self.started_on)),
            completed_on: self.completed_on.map(to_date_time),
            status: Some(self.status.clone()),
            error_message: self.error_message.clone(),
            statistics: Some(statistics),
            graph,
            // "The batch condition that started the workflow run": only an EVENT trigger's
            // batch condition starts a run, and a start a caller made carries none.
            starting_event_batch_condition: None,
        }
    }
}

/// A workflow's graph under construction: the nodes and the directed connections
/// [`ComAmazonawsGlueAwsglueState::workflow_graph`] has placed so far.
///
/// Each component gets ONE node and each connection ONE edge however many of a workflow's
/// triggers name it, which is what the two sets of placed ids are for; the lists keep the
/// order the walk placed them in, so the graph is the same on every read of an unchanged
/// store.
#[derive(Debug, Default)]
pub(super) struct WorkflowGraphBuilder {
    /// The workflow whose graph is being built, which is part of what identifies each of
    /// its nodes ([`workflow_node_id`]).
    pub(super) workflow_name: String,
    /// "A list of the the Glue components belong to the workflow represented as nodes."
    pub(super) nodes: Vec<Node>,
    /// "A list of all the directed connections between the nodes belonging to the
    /// workflow."
    pub(super) edges: Vec<Edge>,
    /// The `UniqueId` of every node already placed.
    pub(super) placed_nodes: HashSet<String>,
    /// The `(SourceId, DestinationId)` of every edge already placed.
    pub(super) placed_edges: HashSet<(String, String)>,
}

impl WorkflowGraphBuilder {
    /// A graph of the workflow `workflow_name`, with nothing placed yet.
    pub(super) fn for_workflow(workflow_name: &str) -> Self {
        Self {
            workflow_name: workflow_name.to_string(),
            ..Self::default()
        }
    }
}

impl WorkflowGraphBuilder {
    /// Place the node for the component `name` of type `node_type` unless it is already
    /// placed, and answer its `UniqueId` either way.
    ///
    /// `trigger` is the stored trigger a TRIGGER node stands for, and `None` for the job and
    /// crawler nodes: "details of the Trigger when the node represents a Trigger"
    /// (`Node$TriggerDetails`). `job_runs` are "the information for the job runs represented
    /// by the job node" (`JobNodeDetails$JobRuns`), which only a JOB node has and only for a
    /// graph reported beside a workflow run.
    ///
    /// Every node carries the details member of its OWN kind and no other kind's: the
    /// details structure is what the node IS, so a job node reports `JobDetails` even when
    /// no run of this workflow has begun of it, and a crawl node reports `CrawlerDetails`
    /// even though no crawl has run. What is empty inside them is the list of runs or
    /// crawls, which is what distinguishes "this node has had no run" from "this is not a
    /// job node".
    pub(super) fn node(
        &mut self,
        node_type: &str,
        name: &str,
        trigger: Option<&TriggerEntry>,
        job_runs: &[JobRun],
    ) -> String {
        let unique_id = workflow_node_id(&self.workflow_name, node_type, name);
        if self.placed_nodes.insert(unique_id.clone()) {
            self.nodes.push(Node {
                r#type: Some(node_type.to_string()),
                name: Some(name.to_string()),
                unique_id: Some(unique_id.clone()),
                trigger_details: trigger.map(|trigger| TriggerNodeDetails {
                    trigger: Some(trigger.to_trigger()),
                }),
                // "The information for the job runs represented by the job node"
                // (`JobNodeDetails$JobRuns`): the runs the workflow run this graph is
                // reported beside began of this job — and no runs at all for a job no run
                // of this workflow has begun, or for a graph reported beside no run.
                job_details: (node_type == NODE_TYPE_JOB).then(|| JobNodeDetails {
                    job_runs: (!job_runs.is_empty()).then(|| job_runs.to_vec()),
                }),
                // "A list of crawls represented by the crawl node"
                // (`CrawlerNodeDetails$Crawls`): nothing here records a crawl of a crawler
                // ([`CrawlerEntry`] holds none), so there is no crawl of this workflow to
                // report inside a crawl node's details.
                crawler_details: (node_type == NODE_TYPE_CRAWLER)
                    .then_some(CrawlerNodeDetails { crawls: None }),
            });
        }
        unique_id
    }

    /// Place the connection from the node `source_id` to the node `destination_id` unless
    /// it is already placed.
    pub(super) fn edge(&mut self, source_id: &str, destination_id: &str) {
        if self
            .placed_edges
            .insert((source_id.to_string(), destination_id.to_string()))
        {
            self.edges.push(Edge {
                source_id: Some(source_id.to_string()),
                destination_id: Some(destination_id.to_string()),
            });
        }
    }

    /// The modelled graph the walk built. A workflow nothing is attached to yields the
    /// graph it is — present, with no components and no connections — rather than no graph
    /// at all, which is what an unasked-for graph is.
    pub(super) fn build(self) -> WorkflowGraph {
        WorkflowGraph {
            nodes: Some(self.nodes),
            edges: Some(self.edges),
        }
    }
}

/// The components one `Action` or `Condition` of a trigger names, as the node type and name
/// of each: both shapes carry a `JobName` and a `CrawlerName`, and which of them is set
/// says whether the component is a job or a crawler.
pub(super) fn named_components<'a>(
    job_name: Option<&'a str>,
    crawler_name: Option<&'a str>,
) -> impl Iterator<Item = (&'static str, &'a str)> {
    job_name
        .map(|name| (NODE_TYPE_JOB, name))
        .into_iter()
        .chain(crawler_name.map(|name| (NODE_TYPE_CRAWLER, name)))
}

/// `Node$UniqueId`: "the unique Id assigned to the node within the workflow."
///
/// An OPAQUE id: a client correlates nodes with the graph's edges by it and reads nothing
/// out of it, so it carries neither the kind of the component nor its name. It is DERIVED
/// from what identifies the node — the workflow it belongs to and the type and name of the
/// component it stands for — rather than minted per call, which is what makes every read of
/// an unchanged graph report the same id for a node while two nodes never share one: a job
/// and a trigger of the same name are two nodes, and the same component in two workflows is
/// two nodes as well.
///
/// Spelled as 32 lowercase hexadecimal characters — 128 bits, the width every opaque id this
/// crate hands out has ([`mint_hex_id`]) — which is inside `NameString`'s "len 1..255"
/// however long the component's name is. The bits are the leading half of a SHA-256 of the
/// three parts, each length-prefixed so no pair of triples can be spelled the same way. Not a
/// credential and not a digest anything is authenticated by.
pub(super) fn workflow_node_id(workflow_name: &str, node_type: &str, name: &str) -> String {
    use ::sha2::Digest;

    let keyed = format!(
        "{}:{workflow_name}:{}:{node_type}:{}:{name}",
        workflow_name.len(),
        node_type.len(),
        name.len()
    );
    let digest = ::sha2::Sha256::digest(keyed.as_bytes());
    ::hex::encode(&digest[..NODE_ID_BYTES])
}

/// How many bytes of the digest a node id carries: sixteen, i.e. 128 bits rendered as 32
/// lowercase hexadecimal characters.
const NODE_ID_BYTES: usize = 16;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlueAwsglueState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlueAwsglueState>)
}

/// The `TaskStatusType` a data quality run is recorded in by a start: of the modelled enum
/// (`STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED | TIMEOUT`), the one that
/// says the run has been accepted and nothing has begun analysing the data source yet.
pub const DATA_QUALITY_RUN_STATE_STARTING: &str = "STARTING";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run that finished its
/// analysis.
pub const DATA_QUALITY_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run that could not finish.
pub const DATA_QUALITY_RUN_STATE_FAILED: &str = "FAILED";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run a cancel has asked to
/// stop, which still owes the clock the status it stops in.
pub const DATA_QUALITY_RUN_STATE_STOPPING: &str = "STOPPING";
/// See [`DATA_QUALITY_RUN_STATE_STARTING`]: the status of a run a cancel stopped.
pub const DATA_QUALITY_RUN_STATE_STOPPED: &str = "STOPPED";

/// The `TaskStatusType` values a data quality run never moves out of again: of the
/// modelled enum quoted on [`DATA_QUALITY_RUN_STATE_STARTING`], the ones that name a
/// finished run. The other three (`STARTING`, `RUNNING`, `STOPPING`) each still owe a
/// further status.
pub const TERMINAL_DATA_QUALITY_RUN_STATES: [&str; 4] =
    ["STOPPED", "SUCCEEDED", "FAILED", "TIMEOUT"];

/// How long a data quality rule recommendation run stays in
/// [`DATA_QUALITY_RUN_STATE_STARTING`] before the clock owes it its terminal status.
///
/// CHOSEN, not documented: Glue publishes no duration for a recommendation run, so this is
/// the emulator's own span — deliberately not zero, so a caller that reads
/// `GetDataQualityRuleRecommendationRun` straight after starting one still observes the run
/// in flight, and the refusal that goes with a run in flight. The due instant is stored on
/// the run and materialised by whichever later call reads it; nothing sleeps.
pub const DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER: Duration =
    Duration::from_millis(500);

/// The `NumberOfWorkers` a recommendation run uses when the start names none: "The number
/// of G.1X workers to be used in the run. The default is 5."
/// (`StartDataQualityRuleRecommendationRunRequest$NumberOfWorkers`).
pub const DEFAULT_DATA_QUALITY_RUN_WORKERS: i32 = 5;

/// The `Timeout` in minutes a recommendation run uses when the start names none: "The
/// default is 2,880 minutes (48 hours)."
/// (`StartDataQualityRuleRecommendationRunRequest$Timeout`).
pub const DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES: i32 = 2_880;

/// The `RecommendationMode` a recommendation run uses when the start names none: "The mode
/// that Glue Data Quality uses to recommend rules. The default is `BASIC`."
/// (`StartDataQualityRuleRecommendationRunRequest$RecommendationMode`).
pub const DEFAULT_RECOMMENDATION_MODE: &str = "BASIC";

/// How many data quality rule recommendation runs this account keeps.
///
/// CHOSEN: the emulator's own window, for the same reason as
/// [`MAX_COLUMN_STATISTICS_TASK_RUNS`] — it bounds
/// [`ComAmazonawsGlueAwsglueState::data_quality_rule_recommendation_runs`] and every walk
/// over it. The OLDEST record is dropped first. (Glue documents that "recommendation runs
/// are automatically deleted after 90 days", which is a retention this process never
/// reaches.)
pub const MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS: usize = 100;

/// How many data quality rule recommendation run ids this process has minted. A nonce
/// source for [`new_data_quality_rule_recommendation_run_id`], exactly as
/// [`MINTED_COLUMN_STATISTICS_TASK_RUN_IDS`] is for a statistics run id.
pub(super) static MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `RunId`: the id the service assigns a data quality rule recommendation run when
/// it starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`. The bits come from [`mint_hex_id`], so two runs started in the same
/// nanosecond cannot be given the same id. It is stored on the run, so every later read of
/// that run reports the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_data_quality_rule_recommendation_run_id() -> String {
    mint_hex_id(
        &MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS,
        DATA_QUALITY_RUN_ID_WORDS,
    )
}

/// How many 64-bit words a data quality run id carries: two, i.e. 128 bits rendered as 32
/// lowercase hexadecimal characters.
pub(super) const DATA_QUALITY_RUN_ID_WORDS: usize = 2;

/// What the data source of a due recommendation run offers it: either the columns the
/// covered table declares, which is what the run recommends rules over, or the reason the
/// catalog offers it nothing to analyse.
#[derive(Debug, Clone)]
pub enum RecommendationRunOutcome {
    /// The covered table is stored and declares these columns, in the order it declares
    /// them.
    Recommended(Vec<String>),
    /// The catalog holds no such database or table, so the run has nothing to analyse and
    /// finishes carrying this reason.
    NothingToAnalyse(String),
}

/// The data source (a catalog table) a data quality run covers, as the request's
/// `DataSource` named it.
///
/// `DataSource` carries either a `GlueTable` ("An Glue table") or a
/// `DataQualityGlueTable` ("An Glue table for Data Quality Operations"), both of which
/// address a table by `DatabaseName` and `TableName` (`@required` on each) inside an
/// optional `CatalogId` — so the table a run covers is read out of whichever of the two the
/// request carried.
#[derive(Debug, Clone)]
pub struct DataSourceTable {
    /// `GlueTable$CatalogId`/`DataQualityGlueTable$CatalogId`, as named.
    pub catalog_id: Option<String>,
    /// The database name as the request spelled it.
    pub database_name: String,
    /// The table name as the request spelled it.
    pub table_name: String,
}

/// The table `data_source` addresses, or `None` when it addresses neither kind of table.
///
/// A `DataSource` that carries both is read as the plain `GlueTable` it names: that member
/// is the one every data quality operation documents as "an Glue table", and the other adds
/// only data-quality processing options on top of the same address.
pub fn data_source_table(data_source: &DataSource) -> Option<DataSourceTable> {
    if let Some(table) = data_source.glue_table.as_ref() {
        return Some(DataSourceTable {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        });
    }
    data_source
        .data_quality_glue_table
        .as_ref()
        .map(|table| DataSourceTable {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        })
}

/// One data quality rule recommendation run, as `StartDataQualityRuleRecommendationRun`
/// recorded it.
///
/// A run carries the data source it covers and the configuration the request named — with
/// each member the request left unset filled in with the default the model documents for it
/// — plus the members only a run has: its id, its status, the instants it moved between
/// statuses and the ruleset it recommended.
#[derive(Debug, Clone)]
pub struct DataQualityRuleRecommendationRunEntry {
    /// `RunId`, minted by [`new_data_quality_rule_recommendation_run_id`].
    pub id: String,
    /// The `DataSource` the start named, reported back as it arrived.
    pub data_source: DataSource,
    /// The catalog the covered table was resolved in, as [`resolve_catalog_id`] resolved
    /// it.
    pub catalog_id: String,
    /// The covered database's name, folded the way the catalog stores it.
    pub database_name: String,
    /// The covered table's name, folded the way the catalog stores it.
    pub table_name: String,
    /// `Role`: "The IAM role that Glue assumes to access resources for the run."
    pub role: String,
    /// `NumberOfWorkers`, defaulting to [`DEFAULT_DATA_QUALITY_RUN_WORKERS`].
    pub number_of_workers: i32,
    /// `Timeout` in minutes, defaulting to
    /// [`DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// `CreatedRulesetName`: "A name for the ruleset" the run creates, absent when the
    /// start named none.
    pub created_ruleset_name: Option<String>,
    /// `DataQualitySecurityConfiguration`, as the start named it.
    pub data_quality_security_configuration: Option<String>,
    /// The `ClientToken` the start carried, which is what makes a repeat of that start
    /// answer this run instead of starting another.
    pub client_token: Option<String>,
    /// `AdditionalRunOptions`, as the start named them.
    pub additional_run_options: Option<DataQualityRuleRecommendationRunAdditionalRunOptions>,
    /// `RecommendationMode`, defaulting to [`DEFAULT_RECOMMENDATION_MODE`].
    pub recommendation_mode: String,
    /// One of the modelled `TaskStatusType` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `StartedOn`: "The date and time when this run started."
    pub started_on: SystemTime,
    /// `LastModifiedOn`: "The last point in time when this data quality rule
    /// recommendation run was modified."
    pub last_modified_on: SystemTime,
    /// `CompletedOn`: "The date and time when this run was completed", absent until the run
    /// has finished.
    pub completed_on: Option<SystemTime>,
    /// `RecommendedRuleset`: the rules the finished run recommends, in DQDL, absent until
    /// it has finished and when it recommended none.
    pub recommended_ruleset: Option<String>,
    /// `ErrorString`: "The error strings that are associated with the run", absent unless
    /// the run failed.
    pub error_string: Option<String>,
}

impl DataQualityRuleRecommendationRunEntry {
    /// Record a run of the data source the `StartDataQualityRuleRecommendationRun` request
    /// names, in [`DATA_QUALITY_RUN_STATE_STARTING`] with its next status due
    /// [`DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER`] out.
    ///
    /// `catalog_id` is the catalog the data source resolved in, and `table` the address it
    /// carried.
    pub fn start(
        input: &StartDataQualityRuleRecommendationRunInput,
        table: &DataSourceTable,
        catalog_id: &str,
        id: String,
        now: SystemTime,
    ) -> Self {
        Self {
            id,
            data_source: input.data_source.clone(),
            catalog_id: catalog_id.to_string(),
            database_name: fold_name(&table.database_name),
            table_name: fold_name(&table.table_name),
            role: input.role.clone(),
            number_of_workers: input
                .number_of_workers
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            created_ruleset_name: input.created_ruleset_name.clone(),
            data_quality_security_configuration: input.data_quality_security_configuration.clone(),
            client_token: input.client_token.clone(),
            additional_run_options: input.additional_run_options.clone(),
            recommendation_mode: input
                .recommendation_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_RECOMMENDATION_MODE.to_string()),
            status: DATA_QUALITY_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has been analysed yet, and nothing has failed.
            recommended_ruleset: None,
            error_string: None,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_DATA_QUALITY_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_DATA_QUALITY_RUN_STATES.contains(&self.status.as_str())
    }

    /// Whether the clock owed this run its next status at `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.settles_at.is_some_and(|due| due <= now)
    }

    /// Whether a cancel has asked this run to stop, so what it settles into is that stop
    /// rather than anything its data source holds.
    pub fn is_stopping(&self) -> bool {
        self.status == DATA_QUALITY_RUN_STATE_STOPPING
    }

    /// How long the run consumed resources, in whole seconds: `ExecutionTime`, "the amount
    /// of time (in seconds) that the run consumed resources".
    pub fn execution_time_seconds(&self) -> i32 {
        data_quality_run_execution_time_seconds(self.started_on, self.completed_on)
    }

    /// Close this run at the DUE instant the clock set for it, and answer that instant;
    /// `None` when the clock owed it nothing, in which case nothing is written.
    ///
    /// The instants recorded are that DUE instant rather than the instant somebody read it,
    /// so two reads of a settled run report the same run.
    pub(super) fn close_at_due_instant(&mut self) -> Option<SystemTime> {
        let due = self.settles_at?;
        self.settles_at = None;
        self.completed_on = Some(due);
        self.last_modified_on = due;
        Some(due)
    }

    /// Move this run into the status the clock owed it, given what its data source offers.
    ///
    /// The run was due to leave [`DATA_QUALITY_RUN_STATE_STARTING`] at
    /// [`Self::settles_at`] whether or not anybody called. A run a cancel asked to stop
    /// settles through [`Self::settle_stop_if_due`] instead, which consults no data source.
    pub fn settle(&mut self, outcome: RecommendationRunOutcome) {
        if self.close_at_due_instant().is_none() {
            return;
        }
        match outcome {
            RecommendationRunOutcome::Recommended(columns) => {
                self.status = DATA_QUALITY_RUN_STATE_SUCCEEDED.to_string();
                self.recommended_ruleset = recommended_ruleset(&columns);
            }
            RecommendationRunOutcome::NothingToAnalyse(reason) => {
                self.status = DATA_QUALITY_RUN_STATE_FAILED.to_string();
                self.error_string = Some(reason);
            }
        }
    }

    /// Settle a run a cancel asked to stop, once the stop the clock owed it is due: it
    /// finishes [`DATA_QUALITY_RUN_STATE_STOPPED`], having analysed nothing further, so its
    /// data source is never consulted and it recommends no ruleset.
    pub fn settle_stop_if_due(&mut self, now: SystemTime) {
        if !self.is_due(now) || self.close_at_due_instant().is_none() {
            return;
        }
        self.status = DATA_QUALITY_RUN_STATE_STOPPED.to_string();
    }

    /// Ask this run to stop, as `CancelDataQualityRuleRecommendationRun` does, and answer
    /// whether it was still running.
    ///
    /// A run in flight moves into [`DATA_QUALITY_RUN_STATE_STOPPING`] and the clock owes it
    /// [`DATA_QUALITY_RUN_STATE_STOPPED`] one span later, so the stop is observable while it
    /// drains. A run that has already finished is left exactly as it is.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() {
            return false;
        }
        self.status = DATA_QUALITY_RUN_STATE_STOPPING.to_string();
        self.last_modified_on = now;
        self.settles_at = Some(
            now.checked_add(DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER)
                .unwrap_or(now),
        );
        true
    }
}

/// How long a data quality run that started at `started_on` consumed resources, in whole
/// seconds, given the instant it completed: `ExecutionTime`, "the amount of time (in
/// seconds) that the run consumed resources", which is the span between the instant it
/// started and the instant it completed. A run still in flight has consumed no completed
/// time, so it reports the member's own default of 0.
///
/// Shared by every kind of data quality run, which all model that member the same way.
pub(super) fn data_quality_run_execution_time_seconds(
    started_on: SystemTime,
    completed_on: Option<SystemTime>,
) -> i32 {
    completed_on
        .and_then(|completed| completed.duration_since(started_on).ok())
        .map_or(0, |consumed| {
            i32::try_from(consumed.as_secs()).unwrap_or(i32::MAX)
        })
}

/// How long a data quality ruleset evaluation run stays in
/// [`DATA_QUALITY_RUN_STATE_STARTING`] before the clock owes it its terminal status.
///
/// The span a rule recommendation run is given
/// ([`DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER`]): both are data quality runs this
/// emulator accepts and finishes on its own clock, and Glue publishes a duration for
/// neither, so one chosen span serves both — deliberately not zero, so a caller that reads
/// `GetDataQualityRulesetEvaluationRun` straight after starting a run still observes it in
/// flight, and the refusal that goes with a run in flight.
pub const DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER: Duration =
    DATA_QUALITY_RULE_RECOMMENDATION_RUN_SETTLES_AFTER;

/// How many data quality ruleset evaluation runs this account keeps, for the same reason
/// and with the same window as [`MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS`]. The OLDEST
/// record is dropped first.
pub const MAX_DATA_QUALITY_RULESET_EVALUATION_RUNS: usize =
    MAX_DATA_QUALITY_RULE_RECOMMENDATION_RUNS;

/// How many data quality ruleset evaluation run ids this process has minted. A nonce source
/// for [`new_data_quality_ruleset_evaluation_run_id`], exactly as
/// [`MINTED_DATA_QUALITY_RULE_RECOMMENDATION_RUN_IDS`] is for a recommendation run id.
pub(super) static MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh `RunId`: the id the service assigns a data quality ruleset evaluation run when it
/// starts.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255`, exactly as [`new_data_quality_rule_recommendation_run_id`] mints a
/// recommendation run's. It is stored on the run, so every later read of that run reports
/// the same id.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_data_quality_ruleset_evaluation_run_id() -> String {
    mint_hex_id(
        &MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS,
        DATA_QUALITY_RUN_ID_WORDS,
    )
}

/// One catalog table a data quality ruleset evaluation run evaluates: the run reads every
/// one of them, so each is kept as an address the catalog can be asked about rather than as
/// the `DataSource` it arrived in.
///
/// The two names are kept AS THE REQUEST SPELLED THEM, because they are what a refusal and a
/// failed run's `ErrorString` name the table by; every lookup goes through
/// [`ComAmazonawsGlueAwsglueState::catalog_table_miss`], which folds them the way the catalog
/// stores a name, so another casing of a stored table still resolves to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedTable {
    /// The catalog the table was resolved in, as [`resolve_catalog_id`] resolved it.
    pub catalog_id: String,
    /// The database's name, as the request spelled it.
    pub database_name: String,
    /// The table's name, as the request spelled it.
    pub table_name: String,
}

impl EvaluatedTable {
    /// The address `table` names inside `catalog_id`.
    pub fn of(table: &DataSourceTable, catalog_id: &str) -> Self {
        Self {
            catalog_id: catalog_id.to_string(),
            database_name: table.database_name.clone(),
            table_name: table.table_name.clone(),
        }
    }
}

/// One data quality ruleset evaluation run, as `StartDataQualityRulesetEvaluationRun`
/// recorded it.
///
/// A run carries the data sources it evaluates and the rulesets it evaluates over them, the
/// configuration the request named — with each member the request left unset filled in with
/// the default the model documents for it — plus the members only a run has: its id, its
/// status and the instants it moved between statuses.
#[derive(Debug, Clone)]
pub struct DataQualityRulesetEvaluationRunEntry {
    /// `RunId`, minted by [`new_data_quality_ruleset_evaluation_run_id`].
    pub id: String,
    /// The `DataSource` the start named, reported back as it arrived.
    pub data_source: DataSource,
    /// The table that `DataSource` addresses, as the catalog stores it.
    pub primary_table: EvaluatedTable,
    /// `AdditionalDataSources`: "a map of reference strings to additional data sources you
    /// can specify for an evaluation run", reported back as they arrived.
    pub additional_data_sources: Option<HashMap<String, DataSource>>,
    /// The tables those additional data sources address, in the order of the reference
    /// strings they arrived under, so the run reads them in one deterministic order.
    pub additional_tables: Vec<EvaluatedTable>,
    /// `Role`: "an IAM role supplied to encrypt the results of the run."
    pub role: String,
    /// `NumberOfWorkers`, defaulting to [`DEFAULT_DATA_QUALITY_RUN_WORKERS`].
    pub number_of_workers: i32,
    /// `Timeout` in minutes, defaulting to [`DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES`].
    pub timeout: i32,
    /// The `ClientToken` the start carried, which is what makes a repeat of that start the
    /// duplicate the service refuses instead of a second run.
    pub client_token: Option<String>,
    /// `AdditionalRunOptions`, as the start named them.
    pub additional_run_options: Option<DataQualityEvaluationRunAdditionalRunOptions>,
    /// `RulesetNames`: the rulesets this run evaluates, as the start listed them.
    pub ruleset_names: Vec<String>,
    /// One of the modelled `TaskStatusType` values.
    pub status: String,
    /// The instant the clock owes this run its next status; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// `StartedOn`: "the date and time when this run started."
    pub started_on: SystemTime,
    /// `LastModifiedOn`: "the date and time when this run was modified."
    pub last_modified_on: SystemTime,
    /// `CompletedOn`: "the date and time when this run was completed", absent until the run
    /// has finished.
    pub completed_on: Option<SystemTime>,
    /// `ErrorString`: "the error strings that are associated with the run", absent unless
    /// the run failed.
    pub error_string: Option<String>,
    /// `ResultIds`: "a list of result IDs for the data quality results for the run" — one id
    /// per ruleset the run evaluated, minted when the run finishes evaluating them, so a run
    /// still in flight and a run that could not evaluate anything carry none.
    pub result_ids: Vec<String>,
    /// What this run's evaluation of its rulesets over its data source produced.
    ///
    /// The work is done where the run is STARTED (`StartDataQualityRulesetEvaluationRun`
    /// performs it with no store guard held, see
    /// [`evaluate_data_quality_rulesets`]), and this is what it answered — so the terminal
    /// status the clock owes this run is decided by the data, and the results a successful
    /// run publishes are the ones its own reading produced.
    pub evaluation: DataQualityEvaluationOutcome,
}

impl DataQualityRulesetEvaluationRunEntry {
    /// Record a run of the rulesets a `StartDataQualityRulesetEvaluationRun` request lists
    /// over the data sources it names, in [`DATA_QUALITY_RUN_STATE_STARTING`] with its
    /// terminal status due [`DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER`] out.
    ///
    /// `primary_table` is the table the request's `DataSource` addresses and
    /// `additional_tables` the ones its `AdditionalDataSources` address, each already
    /// resolved into the catalog that serves it.
    pub fn start(
        input: &StartDataQualityRulesetEvaluationRunInput,
        primary_table: EvaluatedTable,
        additional_tables: Vec<EvaluatedTable>,
        id: String,
        now: SystemTime,
        evaluation: DataQualityEvaluationOutcome,
    ) -> Self {
        Self {
            id,
            data_source: input.data_source.clone(),
            primary_table,
            additional_data_sources: input.additional_data_sources.clone(),
            additional_tables,
            role: input.role.clone(),
            number_of_workers: input
                .number_of_workers
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_WORKERS),
            timeout: input
                .timeout
                .unwrap_or(DEFAULT_DATA_QUALITY_RUN_TIMEOUT_MINUTES),
            client_token: input.client_token.clone(),
            additional_run_options: input.additional_run_options.clone(),
            ruleset_names: input.ruleset_names.clone(),
            status: DATA_QUALITY_RUN_STATE_STARTING.to_string(),
            settles_at: Some(
                now.checked_add(DATA_QUALITY_RULESET_EVALUATION_RUN_SETTLES_AFTER)
                    .unwrap_or(now),
            ),
            started_on: now,
            last_modified_on: now,
            completed_on: None,
            // Nothing has been evaluated yet, and nothing has failed.
            error_string: None,
            result_ids: Vec::new(),
            evaluation,
        }
    }

    /// Every table this run evaluates: the one its `DataSource` addresses, then the ones its
    /// `AdditionalDataSources` do.
    pub fn evaluated_tables(&self) -> impl Iterator<Item = &EvaluatedTable> {
        ::std::iter::once(&self.primary_table).chain(self.additional_tables.iter())
    }

    /// Whether the clock owed this run its next status at `now`.
    pub fn is_due(&self, now: SystemTime) -> bool {
        self.settles_at.is_some_and(|due| due <= now)
    }

    /// How long the run consumed resources, in whole seconds: `ExecutionTime`, "the amount
    /// of time (in seconds) that the run consumed resources".
    pub fn execution_time_seconds(&self) -> i32 {
        data_quality_run_execution_time_seconds(self.started_on, self.completed_on)
    }

    /// Move this run into the status the clock owed it: it was due to leave
    /// [`DATA_QUALITY_RUN_STATE_STARTING`] at [`Self::settles_at`] whether or not anybody
    /// called.
    ///
    /// A run that evaluated its rulesets over a data source the catalog still holds finishes
    /// [`DATA_QUALITY_RUN_STATE_SUCCEEDED`], carrying one freshly minted result id per
    /// ruleset it evaluated: "the evaluation computes results which you can retrieve with the
    /// GetDataQualityResult API", and a result is what one ruleset's evaluation produced —
    /// which is what the returned entries hold, for the store to keep
    /// ([`ComAmazonawsGlueAwsglueState::record_data_quality_results`] is the one caller,
    /// because only the store can keep them). One whose table the catalog holds no longer
    /// cannot have evaluated anything over it, so it finishes
    /// [`DATA_QUALITY_RUN_STATE_FAILED`] carrying `miss`, the catalog's own reason, as its
    /// `ErrorString` and no result at all; so does one whose own evaluation answered that it
    /// evaluated nothing ([`DataQualityEvaluationOutcome::Failed`]), carrying that reason.
    ///
    /// The instants recorded are the DUE instant rather than the instant somebody read the
    /// run, so two reads of a settled run report the same run.
    #[must_use]
    pub fn settle(&mut self, miss: Option<String>) -> Vec<DataQualityResultEntry> {
        let Some(due) = self.settles_at else {
            return Vec::new();
        };
        self.settles_at = None;
        self.completed_on = Some(due);
        self.last_modified_on = due;
        // A data source the catalog no longer holds is reported ahead of what the evaluation
        // answered: the run covers a table that is not there any more.
        let failure = miss.or_else(|| match &self.evaluation {
            DataQualityEvaluationOutcome::Failed(reason) => Some(reason.clone()),
            DataQualityEvaluationOutcome::Evaluated(_) => None,
        });
        if let Some(reason) = failure {
            self.status = DATA_QUALITY_RUN_STATE_FAILED.to_string();
            self.error_string = Some(reason);
            return Vec::new();
        }
        self.status = DATA_QUALITY_RUN_STATE_SUCCEEDED.to_string();
        let DataQualityEvaluationOutcome::Evaluated(evaluated) = &self.evaluation else {
            return Vec::new();
        };
        let results: Vec<DataQualityResultEntry> = evaluated
            .iter()
            .map(|ruleset| {
                DataQualityResultEntry::of(
                    ruleset,
                    &self.id,
                    self.data_source.clone(),
                    self.started_on,
                    due,
                )
            })
            .collect();
        self.result_ids = results.iter().map(|result| result.id.clone()).collect();
        results
    }
}

/// How many data quality result ids this process has minted. A nonce source for
/// [`new_data_quality_result_id`], exactly as
/// [`MINTED_DATA_QUALITY_RULESET_EVALUATION_RUN_IDS`] is for an evaluation run id.
pub(super) static MINTED_DATA_QUALITY_RESULT_IDS: AtomicU64 = AtomicU64::new(0);

/// A fresh data quality result id: what a finished evaluation run reports in its
/// `ResultIds`, and the id the data quality result surface addresses one result by.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside the
/// `DataQualityResultIdList` member type's `length 1..255`. It is stored on the run that
/// minted it, so every later read of that run reports the same ids.
pub fn new_data_quality_result_id() -> String {
    mint_hex_id(&MINTED_DATA_QUALITY_RESULT_IDS, DATA_QUALITY_RUN_ID_WORDS)
}

/// The most bytes a recommended ruleset may carry: the model's own `length 1..65536` on
/// `DataQualityRulesetString`, the shape
/// `GetDataQualityRuleRecommendationRunResponse$RecommendedRuleset` targets. A table wide
/// enough to overrun it is recommended as many rules as fit and no more, so the member
/// stays inside the constraint it is modelled with.
pub const MAX_DATA_QUALITY_RULESET_BYTES: usize = 65_536;

/// How a DQDL rule list opens, and how it closes.
pub(super) const DQDL_RULES_PREFIX: &str = "Rules = [\n";
/// See [`DQDL_RULES_PREFIX`].
pub(super) const DQDL_RULES_SUFFIX: &str = "\n]";

/// The ruleset a finished recommendation run recommends over a table that declares
/// `columns`, in DQDL — "when a start rule recommendation run completes, it creates a
/// recommended ruleset (a set of rules). This member has those rules in Data Quality
/// Definition Language (DQDL) format"
/// (`GetDataQualityRuleRecommendationRunResponse$RecommendedRuleset`).
///
/// The rules are computed from the SCHEMA the catalog holds for the covered table: one
/// `ColumnExists` rule per column the table declares, in the order it declares them. This
/// emulator holds the Glue CONTROL plane and reads none of the objects behind the table, so
/// it profiles no data and a rule whose truth depends on the DATA (a row count, a
/// completeness, a uniqueness or a value range — which is most of what a real recommendation
/// run proposes) is deliberately not recommended: what the run recommends is what the stored
/// schema actually supports. A table that declares no columns yields no rules at all, and so
/// no ruleset.
///
/// A quote inside a column name is escaped, so one name can never end the string its rule
/// sits in, and rules stop being added once the next one would carry the ruleset past
/// [`MAX_DATA_QUALITY_RULESET_BYTES`].
pub fn recommended_ruleset(columns: &[String]) -> Option<String> {
    let mut rules = String::new();
    for column in columns {
        let rule = format!("    ColumnExists \"{}\"", column.replace('"', "\\\""));
        let separator = if rules.is_empty() { "" } else { ",\n" };
        let would_carry = DQDL_RULES_PREFIX.len()
            + rules.len()
            + separator.len()
            + rule.len()
            + DQDL_RULES_SUFFIX.len();
        if would_carry > MAX_DATA_QUALITY_RULESET_BYTES {
            break;
        }
        rules.push_str(separator);
        rules.push_str(&rule);
    }
    if rules.is_empty() {
        return None;
    }
    Some(format!("{DQDL_RULES_PREFIX}{rules}{DQDL_RULES_SUFFIX}"))
}

/// The keyword that opens the rule list of a DQDL ruleset.
pub(super) const DQDL_RULES_KEYWORD: &str = "Rules";

/// What every character of a DQDL string literal reads as while the rule list is walked:
/// a word character, so the literal is still a rule's text but punctuates nothing.
pub(super) const DQDL_LITERAL_PLACEHOLDER: char = 'x';

/// The most characters of one ruleset [`dqdl_rule_count`] reads. A ruleset longer than
/// this cannot have been stored through the modelled operations, whose `Ruleset` member is
/// `DataQualityRulesetString` (`length 1..65536`, a bound on the string's characters), so
/// the bound is that member's own.
///
/// It reuses the constant the recommended-ruleset builder states that same limit in, which
/// is spelled in bytes because that builder measures the text it BUILDS in bytes. The
/// number is the member's own in either unit, and bounding the character count bounds the
/// walk, so the reuse is deliberate rather than a unit confusion.
pub(super) const MAX_DQDL_CHARS_READ: usize = MAX_DATA_QUALITY_RULESET_BYTES;

/// The deepest bracket nesting [`dqdl_rule_count`] walks. Nesting is what a rule's own
/// operands are written with, so a handful of levels is all a rule uses; past this the walk
/// stops rather than following text a caller chose to any depth.
pub(super) const MAX_DQDL_NESTING: usize = 32;

/// How many rules the DQDL text `ruleset` declares — "the number of rules in the ruleset"
/// (`DataQualityRulesetListDetails$RuleCount`).
///
/// The entries of the rule list, counted ([`dqdl_rules`] is the one walk over that list), so
/// the count and the rules an evaluation run evaluates can never disagree.
pub fn dqdl_rule_count(ruleset: &str) -> i32 {
    i32::try_from(dqdl_rules(ruleset).len()).unwrap_or(i32::MAX)
}

/// The rules the DQDL text `ruleset` declares, each as the text the author wrote for it,
/// trimmed, in the order the list declares them.
///
/// A DQDL ruleset declares its rules as one comma-separated list introduced by
/// `Rules = [ … ]`, so the rules are the entries of THAT list: the commas that
/// separate a rule from the next one sit at the list's own nesting level, while a comma
/// inside a rule's own brackets, parentheses or quoted string belongs to that rule. Text
/// declaring no rule list at all declares no rule; a trailing separator adds none.
///
/// A comment runs from `#` to the end of its line and declares nothing, so neither the
/// rules it describes nor a separator inside it is read as part of one.
///
/// DQDL is proprietary to this service and no published crate parses it, so the list is
/// walked here rather than through a dependency. Splitting it needs none of the
/// language beyond its nesting and its string literals, so no rule GRAMMAR is assumed
/// here: whatever a rule's own text is, it is one entry, and what that text MEANS is read
/// where a rule is evaluated ([`super::data_quality_rules`]). The walk reads at most
/// [`MAX_DQDL_CHARS_READ`] characters in ONE pass, nests no
/// deeper than [`MAX_DQDL_NESTING`] and never recurses, so text a caller chose bounds
/// neither the memory nor the stack this takes. Text no rule list can be read out of
/// declares no rules at all: no list at all, a list left unterminated by a truncated
/// text, an unterminated string or comment, or one nested past the limit.
pub fn dqdl_rules(ruleset: &str) -> Vec<String> {
    let keyword: Vec<char> = DQDL_RULES_KEYWORD.chars().collect();
    let mut phase = DqdlPhase::SeekKeyword(0);
    // Whether the character before this one could be part of a word, which is what decides
    // whether the keyword stands on its own or is the tail of a longer one.
    let mut preceded_by_word = false;
    let mut in_string = false;
    let mut in_comment = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut rules: Vec<String> = Vec::new();
    // The text of the entry being walked, as its author wrote it.
    let mut entry = String::new();
    let mut has_content = false;
    let mut closed = false;

    for raw in ruleset.chars().take(MAX_DQDL_CHARS_READ) {
        // A quoted string is where a rule names a column and a comment declares nothing, so
        // neither carries structure: a literal's characters read as one word character each,
        // which still counts as a rule's text while punctuating nothing, and a comment's
        // read as blanks up to the line break that ends it. A backslash escapes the
        // character after it, so an escaped quote does not end the string it sits in.
        let ch = if in_comment {
            in_comment = raw != '\n';
            if in_comment {
                ' '
            } else {
                raw
            }
        } else if in_string {
            if escaped {
                escaped = false;
            } else if raw == '\\' {
                escaped = true;
            } else if raw == '"' {
                in_string = false;
            }
            DQDL_LITERAL_PLACEHOLDER
        } else if raw == '#' {
            in_comment = true;
            ' '
        } else if raw == '"' {
            in_string = true;
            DQDL_LITERAL_PLACEHOLDER
        } else {
            raw
        };

        match phase {
            // The keyword opens a list only where it stands on its own, so a match that a
            // word character precedes is the tail of a longer word and no opener at all.
            DqdlPhase::SeekKeyword(matched) => {
                phase = if keyword.get(matched) == Some(&ch)
                    && (matched > 0 || !preceded_by_word)
                {
                    if matched + 1 == keyword.len() {
                        DqdlPhase::SeekEquals
                    } else {
                        DqdlPhase::SeekKeyword(matched + 1)
                    }
                } else {
                    // A failed match restarts the search WITHOUT re-testing the character
                    // that failed it, which loses no opener: that character is either not
                    // the keyword's first, or it is and the match it broke means a word
                    // character precedes it — and the keyword that a word character
                    // precedes is part of a longer word, which opens no list.
                    DqdlPhase::SeekKeyword(0)
                };
            }
            DqdlPhase::SeekEquals => {
                phase = match ch {
                    '=' => DqdlPhase::SeekOpen,
                    _ if ch.is_whitespace() => DqdlPhase::SeekEquals,
                    _ => DqdlPhase::SeekKeyword(0),
                };
            }
            DqdlPhase::SeekOpen => {
                phase = match ch {
                    '[' => DqdlPhase::Counting,
                    _ if ch.is_whitespace() => DqdlPhase::SeekOpen,
                    _ => DqdlPhase::SeekKeyword(0),
                };
            }
            DqdlPhase::Counting => match ch {
                // The list's own closing bracket: the entry before it is the last rule, and
                // whatever follows the list declares no rule of this list's.
                ']' if depth == 0 => {
                    if has_content {
                        rules.push(entry.trim().to_string());
                    }
                    closed = true;
                    break;
                }
                '[' | '(' | '{' => {
                    has_content = true;
                    entry.push(raw);
                    depth += 1;
                    if depth > MAX_DQDL_NESTING {
                        break;
                    }
                }
                ']' | ')' | '}' => {
                    has_content = true;
                    entry.push(raw);
                    depth = depth.saturating_sub(1);
                }
                ',' if depth == 0 => {
                    if has_content {
                        rules.push(entry.trim().to_string());
                    }
                    entry.clear();
                    has_content = false;
                }
                _ => {
                    has_content |= !ch.is_whitespace();
                    // A comment's characters declare nothing, so they are no part of the
                    // rule's own text; everything else is kept exactly as written, the
                    // characters of a string literal included.
                    if !in_comment {
                        entry.push(raw);
                    }
                }
            },
        }
        preceded_by_word = is_dqdl_word_char(ch);
    }

    if !closed {
        return Vec::new();
    }
    rules
}

/// Where [`dqdl_rules`]' single pass over a ruleset has got to: the three steps that
/// find the rule list, then the walk over its entries.
#[derive(Debug, Clone, Copy)]
pub(super) enum DqdlPhase {
    /// Matching the rule-list keyword, with this many of its characters matched so far.
    SeekKeyword(usize),
    /// The keyword is spelled; awaiting the `=` that assigns the list.
    SeekEquals,
    /// The `=` is seen; awaiting the `[` that opens the list.
    SeekOpen,
    /// Inside the list, counting its entries.
    Counting,
}

/// Whether `ch` can be part of a DQDL word, and so whether the rule-list keyword touching
/// it is a word of its own.
pub(super) fn is_dqdl_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// How many 64-bit words a data quality ruleset's identifier carries: two, i.e. 128 bits
/// rendered as 32 lowercase hexadecimal characters, the width every opaque id this crate
/// mints has ([`mint_hex_id`]).
pub(super) const DATA_QUALITY_RULESET_IDENTIFIER_WORDS: usize = 2;

/// How many data quality ruleset identifiers this process has minted. A nonce source for
/// [`new_data_quality_ruleset_identifier`], one counter of its own so another kind of id
/// running ahead cannot skew it.
pub(super) static MINTED_DATA_QUALITY_RULESET_IDENTIFIERS: AtomicU64 = AtomicU64::new(0);

/// A fresh ruleset identifier: the id the service assigns a data quality ruleset when it is
/// created, and reports beside the ruleset afterwards.
///
/// 32 lowercase hexadecimal characters, i.e. 128 bits, which is inside `HashString`'s
/// `length 1..255` — the string every id this service hands a data quality resource is
/// spelled as. It is minted once, at create, and stored on the [`DataQualityRulesetEntry`],
/// so every later answer about that ruleset reports the same identifier and two rulesets
/// never share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_data_quality_ruleset_identifier() -> String {
    mint_hex_id(
        &MINTED_DATA_QUALITY_RULESET_IDENTIFIERS,
        DATA_QUALITY_RULESET_IDENTIFIER_WORDS,
    )
}

/// One stored data quality ruleset: the DQDL rules `CreateDataQualityRuleset` registers
/// under a name, plus the members only the server sets.
///
/// Every member of the modelled `GetDataQualityRulesetResponse` is held here, so a read
/// serves what the write stored and invents nothing, plus the identifier the create assigned
/// the ruleset and the create's `Tags` and `ClientToken`, which that response does not carry.
#[derive(Debug, Clone)]
pub struct DataQualityRulesetEntry {
    /// "A unique name for the data quality ruleset", which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::data_quality_rulesets`].
    pub name: String,
    /// The identifier the create assigned this ruleset
    /// ([`new_data_quality_ruleset_identifier`]), which is what an answer reporting the
    /// ruleset's own identity carries. Set once and never moved, so it addresses this record
    /// for as long as the record exists.
    pub identifier: String,
    /// "A description of the data quality ruleset", absent when the write named none.
    pub description: Option<String>,
    /// "A Data Quality Definition Language (DQDL) ruleset", stored exactly as sent: this
    /// emulator holds the Glue control plane, so the rules are kept as the text the client
    /// authored rather than re-rendered.
    pub ruleset: String,
    /// "A target table associated with the data quality ruleset", as the write named it.
    pub target_table: Option<DataQualityTargetTable>,
    /// "The name of the security configuration created with the data quality encryption
    /// option", as the write named it.
    pub data_quality_security_configuration: Option<String>,
    /// "The date and time the data quality ruleset was created", set by the create that
    /// stored it and never moved afterwards.
    pub created_on: SystemTime,
    /// "The date and time the data quality ruleset was last modified": the instant of the
    /// most recent write to this record, which for a ruleset nothing has updated is the
    /// instant it was created.
    pub last_modified_on: SystemTime,
    /// "When a ruleset was created from a recommendation run, this run ID is generated to
    /// link the two together" (`DataQualityRulesetListDetails$RecommendationRunId`) — so a
    /// ruleset a client authored directly, which is what `CreateDataQualityRuleset` stores,
    /// carries NO run id: there is no run to link it to.
    pub recommendation_run_id: Option<String>,
    /// The `ClientToken` the create carried, which is what makes a RETRY of that create
    /// answer the stored ruleset instead of the duplicate-name refusal.
    pub client_token: Option<String>,
    /// The create's `Tags`, which no reader of a ruleset models.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, exactly as a custom entity type's are
    /// ([`CustomEntityTypeEntry::tags`]): [`TaggedResourceKind`] carries no data quality
    /// ruleset variant, because the model states no ARN resource-type segment for this kind.
    pub tags: HashMap<String, String>,
}

impl DataQualityRulesetEntry {
    /// Build a stored ruleset from a `CreateDataQualityRuleset` request served at `now`.
    ///
    /// Every member but the identifier and the two timestamps comes from the request: the
    /// client authors the whole ruleset, and the model gives a created ruleset no status of
    /// its own.
    pub fn from_create_input(input: &CreateDataQualityRulesetInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            identifier: new_data_quality_ruleset_identifier(),
            description: input.description.clone(),
            ruleset: input.ruleset.clone(),
            target_table: input.target_table.clone(),
            data_quality_security_configuration: input.data_quality_security_configuration.clone(),
            created_on: now,
            last_modified_on: now,
            recommendation_run_id: None,
            client_token: input.client_token.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Whether this stored ruleset is the one `input` asks to create, member for member.
    ///
    /// What makes a RETRY of a create idempotent is that it asks for the same resource, so
    /// a request that reuses a stored ruleset's `ClientToken` while asking for a different
    /// definition is not that retry: it earns the duplicate-name refusal rather than having
    /// the definition it sent silently dropped behind a 200.
    pub fn matches_create_input(&self, input: &CreateDataQualityRulesetInput) -> bool {
        // The name is compared too, so the comparison is member for member and holds for
        // any pair rather than only for a lookup already keyed by the requested name.
        self.name == input.name
            && self.description == input.description
            && self.ruleset == input.ruleset
            && self.data_quality_security_configuration == input.data_quality_security_configuration
            && same_data_quality_target_table(self.target_table.as_ref(), input.target_table.as_ref())
            && input
                .tags
                .as_ref()
                .map_or(self.tags.is_empty(), |tags| &self.tags == tags)
    }

    /// Apply an `UpdateDataQualityRuleset` request to this stored ruleset, member by
    /// member, as served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: "updates the specified data quality ruleset"
    /// (`UpdateDataQualityRuleset`) names no required member beyond the `Name` that
    /// addresses the ruleset, so the request is a patch over the definition the create
    /// registered rather than a replacement for it.
    ///
    /// The members `UpdateDataQualityRulesetRequest` has NO member for are not this
    /// request's to change: the target table, the security configuration, the creation
    /// instant, the recommendation run id, the create's client token and its tags all stay
    /// as they are. `LastModifiedOn` becomes `now`, since it is "the last point in time
    /// when this data quality ruleset was modified"
    /// (`GetDataQualityRulesetResponse$LastModifiedOn`).
    pub fn apply_update(&mut self, input: &UpdateDataQualityRulesetInput, now: SystemTime) {
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(ruleset) = input.ruleset.as_ref() {
            self.ruleset = ruleset.clone();
        }
        self.last_modified_on = now;
    }

    /// This stored ruleset as one entry of a listing — "describes a data quality ruleset
    /// returned by `GetDataQualityRuleset`" (`DataQualityRulesetListDetails`).
    ///
    /// Every member but `RuleCount` is the one the single-ruleset read serves, so the two
    /// can never disagree about a ruleset; `RuleCount`, which that read does not carry, is
    /// counted from the DQDL text this record holds ([`dqdl_rule_count`]). The DQDL text
    /// itself is not a member of this shape, so a listing reports how many rules a ruleset
    /// holds without serving them.
    pub fn to_list_details(&self) -> DataQualityRulesetListDetails {
        DataQualityRulesetListDetails {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            target_table: self.target_table.clone(),
            recommendation_run_id: self.recommendation_run_id.clone(),
            rule_count: Some(dqdl_rule_count(&self.ruleset)),
        }
    }

    /// Whether this ruleset satisfies every criterion `filter` names.
    ///
    /// A `DataQualityRulesetFilterCriteria` is "the criteria used to filter data quality
    /// rulesets": a conjunction in which each member it carries narrows the listing and a
    /// member it leaves out narrows nothing. `Name` and the description are matched whole —
    /// the model documents no matching mode for either, so they are compared the way this
    /// service's other filtered listing compares the name it is given — the target table is
    /// matched on the table it addresses, and the two instant pairs bound the window the
    /// criteria describe ([`instant_window_admits`], the one reading this service's filtered
    /// listings share).
    pub fn matches(&self, filter: &DataQualityRulesetFilterCriteria) -> bool {
        if filter
            .name
            .as_deref()
            .is_some_and(|name| name != self.name.as_str())
        {
            return false;
        }
        if filter.description.as_deref().is_some_and(|description| {
            self.description.as_deref() != Some(description)
        }) {
            return false;
        }
        // "The name and database name of the target table": a ruleset qualifies when the
        // table it targets is the one named, so a ruleset targeting no table at all is
        // never admitted by criteria that name one. The catalog is compared only where the
        // criteria name one, since the member documents the table and its database as what
        // it addresses.
        if let Some(wanted) = filter.target_table.as_ref() {
            let Some(targeted) = self.target_table.as_ref() else {
                return false;
            };
            if targeted.table_name != wanted.table_name
                || targeted.database_name != wanted.database_name
                || wanted
                    .catalog_id
                    .as_ref()
                    .is_some_and(|catalog| Some(catalog) != targeted.catalog_id.as_ref())
            {
                return false;
            }
        }
        instant_window_admits(
            self.created_on,
            filter.created_before.as_ref(),
            filter.created_after.as_ref(),
        ) && instant_window_admits(
            self.last_modified_on,
            filter.last_modified_before.as_ref(),
            filter.last_modified_after.as_ref(),
        )
    }
}

/// Whether an `UpdateDataQualityRuleset` request names a member to change at all.
///
/// `Description` and `Ruleset` are the only members of that request other than the `Name`
/// addressing the ruleset, and both are optional, so a request carrying neither asks for no
/// change: nothing is written, so the ruleset's `LastModifiedOn` stays where the last write
/// that did change something put it — the same way an `UpdateMLTransform` naming no member
/// leaves its transform's ([`ml_transform_update_names_a_change`]).
pub fn data_quality_ruleset_update_names_a_change(input: &UpdateDataQualityRulesetInput) -> bool {
    input.description.is_some() || input.ruleset.is_some()
}

/// Whether two optional `DataQualityTargetTable` values address the same Glue table. The
/// modelled structure derives no equality, so its three members are compared here.
pub(super) fn same_data_quality_target_table(
    stored: Option<&DataQualityTargetTable>,
    requested: Option<&DataQualityTargetTable>,
) -> bool {
    match (stored, requested) {
        (None, None) => true,
        (Some(stored), Some(requested)) => {
            stored.table_name == requested.table_name
                && stored.database_name == requested.database_name
                && stored.catalog_id == requested.catalog_id
        }
        _ => false,
    }
}
