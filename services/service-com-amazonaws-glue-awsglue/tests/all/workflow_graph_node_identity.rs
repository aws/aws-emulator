// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two things a workflow READ reports about the components and the
//! executions of a workflow: the identity of the nodes of its graph, and the status a run of
//! it comes to rest in.
//!
//! The rules under test:
//!
//! 1. `Node$UniqueId` is an OPAQUE id: it is "the unique Id assigned to the node within the
//!    workflow", so it reveals neither the KIND of the component the node stands for nor its
//!    NAME, no two nodes of a graph share one, the same node reports the same id on every
//!    read of an unchanged store, the same component in two workflows is two nodes with two
//!    ids, and every `Edge` of the graph references nodes by exactly those ids.
//! 2. A workflow run is a COMPOSITE of the work it initiated: it comes to rest only once the
//!    runs its actions began have come to rest, and then in the status of a run whose actions
//!    have run their course — what each action came to is reported by the run's own
//!    statistics, not by its status, and a run that was started reports no `ErrorMessage`.
//!
//! The rows differ along what the rule generalises over: the form and length of the
//! component names (including a name that spells the node's own kind, and one at the
//! published bound), how many components the workflow is made of, and how many of a run's
//! actions begin a job run at all.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, and the due
//! instants the lifecycle turns on are moved THROUGH THE STORE rather than slept for, so no
//! outcome depends on how fast the host runs.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, NODE_TYPE_JOB, NODE_TYPE_TRIGGER, WORKFLOW_RUN_STATUS_COMPLETED,
    WORKFLOW_RUN_STATUS_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, far_future};
use crate::wire::{ok, REGION};

/// The role every job below is registered with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The longest `Name` a node carries: `NameString`, "len 1..255"
/// (`smithy_query shape=Node`).
const MAX_NAME_LENGTH: usize = 255;

/// Register the job `name`.
async fn given_job(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
}

/// Register the workflow `name` with an on-demand trigger of its own initiating one action
/// per entry of `jobs`, and answer the trigger's name.
async fn given_workflow(router: &MultiServiceRouter, name: &str, jobs: &[String]) -> String {
    ok(router, "CreateWorkflow", &json!({ "Name": name })).await;
    let trigger = format!("{name}-trigger");
    let actions: Vec<Value> = jobs.iter().map(|job| json!({ "JobName": job })).collect();
    ok(
        router,
        "CreateTrigger",
        &json!({
            "Name": trigger,
            "Type": "ON_DEMAND",
            "WorkflowName": name,
            "Actions": actions,
        }),
    )
    .await;
    trigger
}

/// The `Graph` a `GetWorkflow` with `IncludeGraph` reports for `workflow`.
async fn graph_of(router: &MultiServiceRouter, workflow: &str) -> Value {
    let body = ok(
        router,
        "GetWorkflow",
        &json!({ "Name": workflow, "IncludeGraph": true }),
    )
    .await;
    body["Workflow"]
        .get("Graph")
        .cloned()
        .unwrap_or_else(|| panic!("a read that asks for the graph reports one, got {body}"))
}

/// The nodes of `graph`, as the list the answer carries.
fn nodes_of(case: &str, graph: &Value) -> Vec<Value> {
    graph
        .get("Nodes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: Graph.Nodes must be a list, got {graph}"))
        .clone()
}

/// The `UniqueId` of `node`.
fn unique_id(case: &str, node: &Value) -> String {
    node.get("UniqueId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: every node reports a UniqueId, got {node}"))
        .to_string()
}

/// The `LastRun` of a `GetWorkflow` answer for `workflow`.
async fn last_run_of(router: &MultiServiceRouter, workflow: &str) -> Value {
    let body = ok(router, "GetWorkflow", &json!({ "Name": workflow })).await;
    body["Workflow"]
        .get("LastRun")
        .cloned()
        .unwrap_or_else(|| panic!("a workflow that was run reports its last execution, got {body}"))
}

/// Move the due instant of every unsettled run of `workflow` to `at`.
fn hold_runs_due(engine: &Arc<engine::InMemoryEngine>, workflow: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .workflow_mut(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"));
    for run in &mut entry.runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

/// Move every due instant the job runs the runs of `workflow` began still owe to `at`, so a
/// row can say where the WORK the run initiated stands independently of the run itself.
///
/// A job run owes up to TWO states at once — the state it is in while its script runs, and
/// the state its own execution came to — so both of its instants move, as
/// `job_runs::set_due_instant` does for a job's own runs: moving only the first would leave
/// the run half way, in flight for ever.
fn hold_actions_due(engine: &Arc<engine::InMemoryEngine>, workflow: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let actions: Vec<(String, String)> = store
        .workflow(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"))
        .runs
        .iter()
        .flat_map(|run| run.action_job_runs.iter())
        .map(|begun| (begun.job_name.clone(), begun.run_id.clone()))
        .collect();
    assert!(
        !actions.is_empty(),
        "the runs of {workflow:?} must have begun a job run for this to say anything"
    );
    for (job_name, run_id) in actions {
        if let Some(job_run) = store
            .job_mut(REGION, &job_name)
            .and_then(|job| job.run_mut(&run_id))
        {
            if job_run.settles_at.is_some() {
                job_run.settles_at = Some(at);
            }
            if job_run.rests_at.is_some() {
                job_run.rests_at = Some(at);
            }
        }
    }
}

/// One row of the graph table: the component names a workflow is made of.
struct GraphCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The job names the workflow's trigger initiates an action of each.
    jobs: Vec<String>,
}

/// The rows: names of different forms and lengths, including one that spells the node's own
/// kind and one at the published bound, and workflows made of one and of several components.
fn graph_cases() -> Vec<GraphCase> {
    vec![
        GraphCase {
            case: "one plainly named job",
            jobs: vec!["etl-job".to_string()],
        },
        GraphCase {
            case: "a job whose name spells the kind of its own node",
            jobs: vec!["job".to_string(), "trigger".to_string()],
        },
        GraphCase {
            case: "two jobs whose names agree on all but their last character",
            jobs: vec!["payments-a".to_string(), "payments-b".to_string()],
        },
        GraphCase {
            case: "a job named at the published bound",
            jobs: vec!["j".repeat(MAX_NAME_LENGTH)],
        },
    ]
}

/// A node's `UniqueId` is opaque, unique within the graph, stable across reads, and what the
/// graph's edges reference.
#[tokio::test]
async fn a_graph_node_is_addressed_by_an_opaque_stable_id() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in graph_cases().iter().enumerate() {
        let case = row.case;
        let workflow = format!("graph-identity-{index}");
        for job in &row.jobs {
            given_job(&router, job).await;
        }
        let trigger = given_workflow(&router, &workflow, &row.jobs).await;

        let graph = graph_of(&router, &workflow).await;
        let nodes = nodes_of(case, &graph);
        assert_eq!(
            nodes.len(),
            row.jobs.len() + 1,
            "case {case}: the graph is the trigger plus the components it initiates, got \
             {graph}"
        );

        let mut ids = BTreeSet::new();
        for node in &nodes {
            let id = unique_id(case, node);
            let name = node
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: every node reports a Name, got {node}"));
            let node_type = node
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: every node reports a Type, got {node}"));
            assert!(
                !id.is_empty() && id.chars().count() <= MAX_NAME_LENGTH,
                "case {case}: a UniqueId is a NameString of 1..{MAX_NAME_LENGTH} characters, \
                 got {} in {node}",
                id.chars().count()
            );
            // Opaque: a client reads nothing out of the id, so neither the component's name
            // nor the kind of node it is appears in it.
            assert!(
                !id.contains(name),
                "case {case}: a UniqueId must not carry the component's name, got {id:?} for \
                 {name:?}"
            );
            assert!(
                !id.to_ascii_lowercase()
                    .contains(&node_type.to_ascii_lowercase()),
                "case {case}: a UniqueId must not carry the kind of its node, got {id:?} for \
                 a {node_type} node"
            );
            assert!(
                ids.insert(id.clone()),
                "case {case}: two nodes must not share a UniqueId, got {id:?} again in {graph}"
            );
        }

        // Every edge names nodes of this graph by their ids, and the trigger reaches each
        // component it initiates.
        let edges = graph
            .get("Edges")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: Graph.Edges must be a list, got {graph}"))
            .clone();
        assert_eq!(
            edges.len(),
            row.jobs.len(),
            "case {case}: the trigger reaches each component it initiates, got {graph}"
        );
        for edge in &edges {
            for member in ["SourceId", "DestinationId"] {
                let end = edge
                    .get(member)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: an edge reports its {member}, got {edge}"));
                assert!(
                    ids.contains(end),
                    "case {case}: {member} {end:?} must name a node of the graph, got {graph}"
                );
            }
        }

        // A read does not change the ids a later caller sees.
        let again = graph_of(&router, &workflow).await;
        assert_eq!(
            again, graph,
            "case {case}: two reads of an unchanged graph must agree, got {again}"
        );

        // The same component in ANOTHER workflow is another node, so its id is another one.
        let other = format!("graph-identity-{index}-again");
        given_workflow(&router, &other, &row.jobs).await;
        let elsewhere = graph_of(&router, &other).await;
        for node in nodes_of(case, &elsewhere) {
            let id = unique_id(case, &node);
            assert!(
                !ids.contains(&id),
                "case {case}: a component's node in another workflow is another node, got \
                 {id:?} in both"
            );
        }
        // The trigger of the first workflow is still its own node, so the two graphs are
        // not one.
        assert!(
            nodes_of(case, &graph).iter().any(|node| node
                .get("Name")
                .and_then(Value::as_str)
                == Some(trigger.as_str())),
            "case {case}: the workflow's own trigger is one of its nodes, got {graph}"
        );
    }
}

/// A run comes to rest only once the runs its actions began have, and then in the status of
/// a run whose actions have run their course — carrying no error of its own, and reporting
/// what each action came to in its statistics.
#[tokio::test]
async fn a_run_comes_to_rest_once_the_work_it_began_has() {
    // (case, the job actions the workflow's entry point initiates)
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one action", vec!["rest-job-a".to_string()]),
        (
            "two actions",
            vec!["rest-job-b".to_string(), "rest-job-c".to_string()],
        ),
    ];

    for (index, (case, jobs)) in rows.into_iter().enumerate() {
        let (router, engine) = emulator_with_engine();
        let workflow = format!("run-at-rest-{index}");
        for job in &jobs {
            given_job(&router, job).await;
        }
        given_workflow(&router, &workflow, &jobs).await;
        ok(&router, "StartWorkflowRun", &json!({ "Name": workflow })).await;

        // Nothing is due yet: the run is in flight.
        hold_runs_due(&engine, &workflow, far_future());
        hold_actions_due(&engine, &workflow, far_future());
        let in_flight = last_run_of(&router, &workflow).await;
        assert_eq!(
            in_flight.get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_RUNNING),
            "case {case}: a run whose work has not finished is \
             {WORKFLOW_RUN_STATUS_RUNNING}, got {in_flight}"
        );

        // The run's own span has passed, but the work it began has not finished: it is still
        // in flight, however often it is read.
        hold_runs_due(&engine, &workflow, already_past());
        for read in 0..2 {
            let waiting = last_run_of(&router, &workflow).await;
            assert_eq!(
                waiting.get("Status").and_then(Value::as_str),
                Some(WORKFLOW_RUN_STATUS_RUNNING),
                "case {case}: read {read} of a run whose actions are still running must \
                 report {WORKFLOW_RUN_STATUS_RUNNING}, got {waiting}"
            );
            assert!(
                waiting.get("CompletedOn").is_none_or(Value::is_null),
                "case {case}: read {read} of a run that has not finished reports no \
                 CompletedOn, got {waiting}"
            );
        }

        // Once the work has finished, the run comes to rest: its statistics report what each
        // action came to, and the run itself carries no error.
        hold_actions_due(&engine, &workflow, already_past());
        hold_runs_due(&engine, &workflow, already_past());
        let settled = last_run_of(&router, &workflow).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(WORKFLOW_RUN_STATUS_COMPLETED),
            "case {case}: a run whose actions have run their course is \
             {WORKFLOW_RUN_STATUS_COMPLETED}, got {settled}"
        );
        assert!(
            settled.get("ErrorMessage").is_none_or(Value::is_null),
            "case {case}: a run that was started and ran its course reports no ErrorMessage, \
             got {settled}"
        );
        assert!(
            settled.get("CompletedOn").and_then(Value::as_f64).is_some(),
            "case {case}: a finished run reports when it finished, got {settled}"
        );

        let statistics = settled
            .get("Statistics")
            .unwrap_or_else(|| panic!("case {case}: a run reports Statistics, got {settled}"));
        let total = i64::try_from(jobs.len()).expect("a row names few actions");
        assert_eq!(
            statistics.get("TotalActions").and_then(Value::as_i64),
            Some(total),
            "case {case}: the run counts the actions its entry point initiated, got \
             {statistics}"
        );
        assert_eq!(
            statistics
                .get("RunningActions")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            0,
            "case {case}: no action of a run that has come to rest is still running, got \
             {statistics}"
        );
        // Glue is mounted on its own here, so no job script could be read and every action
        // came to rest in the modelled failure state — which is what the statistics report,
        // while the RUN itself completed.
        assert_eq!(
            statistics.get("FailedActions").and_then(Value::as_i64),
            Some(total),
            "case {case}: the statistics report what each action came to, got {statistics}"
        );

        // A settled run is settled: the next read reports the same run.
        let again = last_run_of(&router, &workflow).await;
        assert_eq!(
            again, settled,
            "case {case}: a settled run must read the same twice; first {settled}, then \
             {again}"
        );
    }
}

/// What the run and the graph of a workflow whose trigger initiates a CRAWL report: this
/// crate records no crawl, so nothing claims one ran — the crawl node reports no crawls and
/// the run's statistics move no counter for it, while the action is still counted among the
/// actions the entry point initiated.
#[tokio::test]
async fn nothing_claims_a_crawl_ran_for_a_crawler_action() {
    let (router, engine) = emulator_with_engine();
    let case = "a trigger whose only action names a crawler";
    let workflow = "crawler-action-workflow";
    let crawler = "crawler-action-crawler";
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": crawler,
            "Role": ROLE_ARN,
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;
    ok(&router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": format!("{workflow}-trigger"),
            "Type": "ON_DEMAND",
            "WorkflowName": workflow,
            "Actions": [{ "CrawlerName": crawler }],
        }),
    )
    .await;
    ok(&router, "StartWorkflowRun", &json!({ "Name": workflow })).await;

    // The crawl node of the graph says what it is and that nothing has crawled through it.
    let graph = graph_of(&router, workflow).await;
    let crawl = nodes_of(case, &graph)
        .into_iter()
        .find(|node| node["Name"].as_str() == Some(crawler))
        .unwrap_or_else(|| panic!("case {case}: the crawler is a node of the graph, got {graph}"));
    assert_eq!(
        crawl["Type"].as_str(),
        Some("CRAWLER"),
        "case {case}: the node reports the kind of component it stands for, got {crawl}"
    );
    assert!(
        crawl["CrawlerDetails"]
            .get("Crawls")
            .is_none_or(Value::is_null),
        "case {case}: nothing crawled, so the node reports no crawl, got {crawl}"
    );

    // The run counts the action its entry point initiated and claims nothing about what
    // became of it: no counter of the statistics moves for a crawl this crate does not run.
    hold_runs_due(&engine, workflow, already_past());
    let settled = last_run_of(&router, workflow).await;
    let statistics = settled
        .get("Statistics")
        .unwrap_or_else(|| panic!("case {case}: a run reports Statistics, got {settled}"));
    assert_eq!(
        statistics.get("TotalActions").and_then(Value::as_i64),
        Some(1),
        "case {case}: the run counts the action its entry point initiated, got {statistics}"
    );
    for counted in [
        "RunningActions",
        "SucceededActions",
        "FailedActions",
        "TimeoutActions",
        "StoppedActions",
        "ErroredActions",
        "WaitingActions",
    ] {
        assert_eq!(
            statistics.get(counted).and_then(Value::as_i64).unwrap_or(0),
            0,
            "case {case}: nothing crawled, so no action is counted in {counted}, got \
             {statistics}"
        );
    }
}

/// The graph a workflow read reports is the one the components attached to it make it of:
/// a trigger naming no workflow is no node of it.
#[tokio::test]
async fn a_graph_holds_only_the_components_attached_to_its_workflow() {
    let (router, _engine) = emulator_with_engine();
    let case = "a trigger bound to another workflow";
    let workflow = "graph-scoped";
    let elsewhere = "graph-scoped-other";
    given_job(&router, "graph-scoped-job").await;
    given_job(&router, "graph-scoped-other-job").await;
    given_workflow(&router, workflow, &["graph-scoped-job".to_string()]).await;
    given_workflow(
        &router,
        elsewhere,
        &["graph-scoped-other-job".to_string()],
    )
    .await;

    let graph = graph_of(&router, workflow).await;
    let reported: BTreeSet<(String, String)> = nodes_of(case, &graph)
        .iter()
        .map(|node| {
            (
                node["Type"].as_str().unwrap_or_default().to_string(),
                node["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        BTreeSet::from([
            (
                NODE_TYPE_TRIGGER.to_string(),
                format!("{workflow}-trigger")
            ),
            (NODE_TYPE_JOB.to_string(), "graph-scoped-job".to_string()),
        ]),
        "case {case}: the graph holds this workflow's components alone, got {graph}"
    );
}
