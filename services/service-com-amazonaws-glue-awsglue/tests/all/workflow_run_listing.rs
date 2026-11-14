// SPDX-License-Identifier: CC0-1.0

//! `GetWorkflowRuns`: the runs of ONE workflow, newest started first, in pages.
//!
//! RULE: a paginated read of a resource's children reports every child the store holds of
//! the named parent — nothing of any other parent — in the order the listing serves,
//! projected exactly as the single-child read projects it, one `MaxResults`-sized page at a
//! time with a `NextToken` that resumes the walk and no token on the last page; a parent
//! the store does not hold is refused with the operation's modelled not-found error, and so
//! is a read with no child to report, because the list member's modelled minimum length of
//! one leaves the operation no 2xx shape that reports nothing. For a
//! run history the order is the chronological one, newest start first — the order this
//! service's sibling run listing documents — so it is over the instant each run STARTED and
//! not over the order the store happens to hold the runs in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many runs are stored, in what
//! order their start instants fall against the order they were recorded in, the page size
//! asked for (including both published bounds of `MaxResults` and one past each), whether
//! the reader asked for the workflow graph, and whether the workflow is one the store
//! holds.
//!
//! The start instants and the due instants the rows turn on are written THROUGH THE STORE
//! rather than slept for, so a row's expected order is written down rather than inferred
//! from how fast the clock ticked between two starts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::WORKFLOW_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, WORKFLOW_RUN_STATUS_COMPLETED, WORKFLOW_RUN_STATUS_RUNNING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::emulator_with_engine;
use crate::wire::{call, call_in_region, error_code, error_message, ok, REGION};

/// A second Region's endpoint, so a row can assert the key a workflow is addressed by.
/// A workflow is per-ACCOUNT — every operation the model declares over one names it by
/// `Name` alone, with no `CatalogId` and no Region member — while a TRIGGER is served only
/// through the endpoint that created it, which is what the graph is read by.
const OTHER_REGION: &str = "eu-west-1";

/// A role ARN `CreateJob` accepts, for the job the entry-point trigger's action names.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-workflow-run-listing-role";

/// The instant a row's start offsets are measured from, so the order a row expects is fixed
/// by the row itself rather than by the clock.
const START_EPOCH: Duration = Duration::from_secs(1_767_225_600);

/// How far out the transition a run is owed is held while a listing is read, so every run
/// stays in flight and two listings of it report the same run.
const HELD_OPEN: Duration = Duration::from_secs(3_600);

/// Create `name`, give it an on-demand entry point, and start one run of it per entry of
/// `start_offsets`; then give the runs those start instants — in the order the store
/// recorded them — and hold the transition each is owed far out. Answers the run ids, in
/// the order they were started.
///
/// The workflow names no `MaxConcurrentRuns`, so every run of a row may be in flight at
/// once: "If you leave this parameter blank, there is no limit to the number of concurrent
/// workflow runs" (`CreateWorkflowRequest$MaxConcurrentRuns`).
pub(crate) async fn given_workflow_with_runs(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    start_offsets: &[u64],
) -> Vec<String> {
    let job = format!("{name}-job");
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": job,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
    ok(router, "CreateWorkflow", &json!({ "Name": name })).await;
    // A workflow is runnable only through a trigger that can BEGIN a run of it, so the
    // entry point is what makes the starts below succeed.
    ok(
        router,
        "CreateTrigger",
        &json!({
            "Name": format!("{name}-trigger"),
            "Type": "ON_DEMAND",
            "WorkflowName": name,
            "Actions": [{ "JobName": job }],
        }),
    )
    .await;

    let mut started = Vec::new();
    for _ in start_offsets {
        let answer = ok(router, "StartWorkflowRun", &json!({ "Name": name })).await;
        started.push(
            answer
                .get("RunId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {answer}"))
                .to_string(),
        );
    }

    set_start_instants(engine, name, start_offsets);
    set_due_instant(engine, name, SystemTime::now() + HELD_OPEN);
    started
}

/// Give the runs of `workflow` the start instants `offsets` names, in the order the store
/// recorded them, so the order a row expects is fixed by the row rather than by how fast
/// the clock ticked between two starts.
fn set_start_instants(engine: &Arc<engine::InMemoryEngine>, workflow: &str, offsets: &[u64]) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .workflow_mut(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"));
    assert_eq!(
        entry.runs.len(),
        offsets.len(),
        "the store must hold one run of {workflow:?} per start instant the row names"
    );
    for (run, offset) in entry.runs.iter_mut().zip(offsets) {
        run.started_on = SystemTime::UNIX_EPOCH + START_EPOCH + Duration::from_secs(*offset);
    }
}

/// Move EVERY due instant every run of `workflow` still owes, and every one the job runs
/// those runs' actions began still owe, to `at` — so a row can hold the transient window open
/// or make the owed transition due without sleeping.
///
/// The actions move WITH the run because a run is a composite of them: it comes to rest only
/// once the runs it began have. A job run owes up to TWO states at once — the state it is in
/// while its script runs, and the state its own execution came to — so both of its instants
/// move, as `job_runs::set_due_instant` does for a job's own runs.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, workflow: &str, at: SystemTime) {
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
    let entry = store
        .workflow_mut(workflow)
        .unwrap_or_else(|| panic!("the store must hold workflow {workflow:?}"));
    for run in &mut entry.runs {
        run.settles_at = Some(at);
    }
}

/// The `Runs` of one answered page. `Runs` is `length 1..1000`, so this operation has no 2xx
/// shape that reports nothing: an answered page always carries a non-empty `Runs`, and a
/// read with nothing to report is a refusal instead (asserted in
/// [`crate::workflow_run_history_absence`]).
pub(crate) fn listed_runs(page: &Value) -> Vec<Value> {
    let runs = page
        .get("Runs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetWorkflowRuns must answer Runs as a list, got {page}"))
        .clone();
    assert!(
        !runs.is_empty(),
        "an answered page carries a non-empty Runs, got {page}"
    );
    runs
}

/// The `WorkflowRunId` of every run one answer reports, in the order it reported them.
fn listed_ids(page: &Value) -> Vec<String> {
    listed_runs(page)
        .iter()
        .map(|run| {
            run["WorkflowRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("a WorkflowRun carries a WorkflowRunId, got {run}"))
                .to_string()
        })
        .collect()
}

/// The run ids of `started` in the order the rule puts them: newest start first.
fn newest_first(started: &[String], start_offsets: &[u64]) -> Vec<String> {
    let mut paired: Vec<(u64, String)> = start_offsets
        .iter()
        .copied()
        .zip(started.iter().cloned())
        .collect();
    paired.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    paired.into_iter().map(|(_, id)| id).collect()
}

/// The `WorkflowRun` the single-run read reports for one run, with `IncludeGraph` as the
/// caller asks.
async fn get_run(
    router: &MultiServiceRouter,
    workflow: &str,
    run_id: &str,
    include_graph: bool,
) -> Value {
    let body = ok(
        router,
        "GetWorkflowRun",
        &json!({ "Name": workflow, "RunId": run_id, "IncludeGraph": include_graph }),
    )
    .await;
    body.get("Run")
        .cloned()
        .unwrap_or_else(|| panic!("GetWorkflowRun must report a Run, got {body}"))
}

/// Walk every page `GetWorkflowRuns` hands out for `workflow`, following its own token, and
/// answer the ids in the order they were served plus how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    workflow: &str,
    page_size: Option<i32>,
    stored: usize,
) -> (Vec<String>, usize) {
    let mut served: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "Name": workflow });
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetWorkflowRuns", &request).await;
        pages += 1;
        let ids = listed_ids(&page);
        if let Some(size) = page_size {
            assert!(
                ids.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {ids:?}"
            );
        }
        served.extend(ids);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            served.len() <= stored,
            "case {case:?}: the walk must terminate, got {served:?} for {stored} runs"
        );
    }
    (served, pages)
}

#[tokio::test]
async fn runs_are_listed_newest_first_with_the_single_run_projection() {
    // (case, the start instant of each run, in the order the runs were started)
    let rows: &[(&str, &[u64])] = &[
        ("one run", &[10]),
        ("two runs started in the order they were recorded", &[10, 20]),
        ("three runs, the newest recorded first", &[30, 10, 20]),
        (
            "five runs, start instants against the recorded order",
            &[50, 10, 40, 20, 30],
        ),
    ];

    for (case, start_offsets) in rows {
        let (router, engine) = emulator_with_engine();
        let name = "listed-workflow";
        let started = given_workflow_with_runs(&router, &engine, name, start_offsets).await;

        let page = ok(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
        let expected = newest_first(&started, start_offsets);
        assert_eq!(
            listed_ids(&page),
            expected,
            "case {case:?}: expected the runs newest start first, got {page}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: a listing that reached the end carries no token, got {page}"
        );

        // Element for element the SAME run the single-run read reports: one record, two
        // reads.
        for (index, listed) in listed_runs(&page).iter().enumerate() {
            let id = &expected[index];
            let read = get_run(&router, name, id, false).await;
            assert_eq!(
                listed, &read,
                "case {case:?}: the listing's element {index} must be the run GetWorkflowRun \
                 reports for {id}, got {listed} and {read}"
            );
            assert_eq!(
                listed["Name"].as_str(),
                Some(name),
                "case {case:?}: every listed run names the workflow it belongs to, got {listed}"
            );
            assert_eq!(
                listed["Status"].as_str(),
                Some(WORKFLOW_RUN_STATUS_RUNNING),
                "case {case:?}: a run whose transition is not yet due is still in flight, got \
                 {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_workflow_no_run_was_started_of_is_refused() {
    let (router, _engine) = emulator_with_engine();
    let name = "unrun-workflow";
    ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;

    // `Runs` is `length 1..1000`, so there is no 2xx answer that reports no run: the read is
    // refused instead. The whole class of reads with nothing to report is tabled in
    // [`crate::workflow_run_history_absence`].
    let (status, body) = call(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
    assert_eq!(
        status, 400,
        "case \"a workflow with no runs\": expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case \"a workflow with no runs\": expected an EntityNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn every_page_size_walks_every_run_exactly_once() {
    let (router, engine) = emulator_with_engine();
    let name = "paged-workflow";
    // Seven runs whose start instants deliberately disagree with the order they were
    // recorded in, so paging is asserted over the served order and not over insertion.
    let start_offsets: &[u64] = &[70, 10, 60, 20, 50, 30, 40];
    let started = given_workflow_with_runs(&router, &engine, name, start_offsets).await;
    let expected = newest_first(&started, start_offsets);

    // (case, the page size asked for, how many pages the walk must take)
    let rows: &[(&str, Option<i32>, usize)] = &[
        ("no page size", None, 1),
        // The published lower bound of `MaxResults` (`PageSize`, range 1..1000): one run
        // per page.
        ("the smallest page the model accepts", Some(1), 7),
        ("a page smaller than the stored count", Some(3), 3),
        ("a page one short of the stored count", Some(6), 2),
        ("a page exactly the stored count", Some(7), 1),
        ("a page larger than the stored count", Some(50), 1),
        // The published upper bound.
        ("the largest page the model accepts", Some(1000), 1),
    ];

    for (case, page_size, pages_expected) in rows {
        let (served, pages) = walk(&router, case, name, *page_size, start_offsets.len()).await;
        assert_eq!(
            served, expected,
            "case {case:?}: expected every stored run once, newest start first, got {served:?}"
        );
        assert_eq!(
            pages, *pages_expected,
            "case {case:?}: expected {pages_expected} page(s), got {pages}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, engine) = emulator_with_engine();
    let name = "bounded-workflow";
    given_workflow_with_runs(&router, &engine, name, &[10]).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1001)] {
        let (status, body) = call(
            &router,
            "GetWorkflowRuns",
            &json!({ "Name": name, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_only_the_runs_of_the_workflow_it_names() {
    let (router, engine) = emulator_with_engine();
    let left = given_workflow_with_runs(&router, &engine, "workflow-left", &[10, 30]).await;
    let right = given_workflow_with_runs(&router, &engine, "workflow-right", &[20]).await;

    // (case, the workflow asked about, the runs it holds, their start instants)
    let rows: &[(&str, &str, &[String], &[u64])] = &[
        ("the workflow with two runs", "workflow-left", &left, &[10, 30]),
        ("the workflow with one run", "workflow-right", &right, &[20]),
    ];

    for (case, workflow, held, start_offsets) in rows {
        let page = ok(&router, "GetWorkflowRuns", &json!({ "Name": workflow })).await;
        assert_eq!(
            listed_ids(&page),
            newest_first(held, start_offsets),
            "case {case:?}: expected only this workflow's runs, got {page}"
        );
        for run in listed_runs(&page) {
            assert_eq!(
                run["Name"].as_str(),
                Some(*workflow),
                "case {case:?}: every listed run names the workflow it belongs to, got {run}"
            );
        }
    }
}

#[tokio::test]
async fn a_workflow_the_store_does_not_hold_is_not_found() {
    let (router, engine) = emulator_with_engine();
    given_workflow_with_runs(&router, &engine, "held-workflow", &[10]).await;

    // A workflow name is NOT folded, so a name differing only in case names a different
    // workflow — and a page size or a token on the request does not change the refusal.
    let rows: &[(&str, &str, Value)] = &[
        ("a name no workflow has", "absent-workflow", json!({})),
        (
            "the held workflow's name in another case",
            "HELD-WORKFLOW",
            json!({}),
        ),
        (
            "a name no workflow has, with a page size",
            "absent-workflow",
            json!({ "MaxResults": 5 }),
        ),
        (
            "a name no workflow has, with a token",
            "absent-workflow",
            json!({ "NextToken": "00000000000000000000:wr_0" }),
        ),
        (
            "a name no workflow has, with a graph asked for",
            "absent-workflow",
            json!({ "IncludeGraph": true }),
        ),
    ];

    for (case, workflow, extra) in rows {
        let mut request = json!({ "Name": workflow });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let (status, body) = call(&router, "GetWorkflowRuns", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected an EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            WORKFLOW_NOT_FOUND,
            "case {case:?}: expected the wording a workflow miss carries, got {body}"
        );
    }

    // The workflow the store DOES hold is served, so the refusal is about the name and not
    // about the operation.
    let page = ok(
        &router,
        "GetWorkflowRuns",
        &json!({ "Name": "held-workflow" }),
    )
    .await;
    assert_eq!(
        listed_runs(&page).len(),
        1,
        "case \"the held workflow\": expected its one run, got {page}"
    );
}

#[tokio::test]
async fn the_graph_is_reported_only_when_the_reader_asks_for_it() {
    let (router, engine) = emulator_with_engine();
    let name = "graphed-workflow";
    let started = given_workflow_with_runs(&router, &engine, name, &[10, 20]).await;

    // (case, what `IncludeGraph` the request carries, whether a graph must come back)
    let rows: &[(&str, Option<bool>, bool)] = &[
        ("no IncludeGraph member", None, false),
        ("IncludeGraph false", Some(false), false),
        ("IncludeGraph true", Some(true), true),
    ];

    for (case, include_graph, graphed) in rows {
        let mut request = json!({ "Name": name });
        if let Some(asked) = include_graph {
            request["IncludeGraph"] = json!(asked);
        }
        let page = ok(&router, "GetWorkflowRuns", &request).await;
        let runs = listed_runs(&page);
        assert_eq!(
            runs.len(),
            started.len(),
            "case {case:?}: expected every stored run, got {page}"
        );
        for run in &runs {
            let graph = run.get("Graph");
            assert_eq!(
                graph.is_some_and(|graph| !graph.is_null()),
                *graphed,
                "case {case:?}: expected a graph to be reported: {graphed}, got {run}"
            );
            // The graph a listed run reports is the one the single-run read reports for it:
            // the graph of the workflow these are runs of.
            let id = run["WorkflowRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a run carries an id, got {run}"));
            let read = get_run(&router, name, id, *graphed).await;
            assert_eq!(
                run.get("Graph"),
                read.get("Graph"),
                "case {case:?}: expected the graph GetWorkflowRun reports for {id}, got {run} \
                 and {read}"
            );
        }
        if *graphed {
            let graph = &runs[0]["Graph"];
            // The workflow holds one trigger running one job, so its graph holds the node
            // of each and the connection between them.
            assert_eq!(
                graph["Nodes"].as_array().map(Vec::len),
                Some(2),
                "case {case:?}: expected the trigger and job nodes, got {graph}"
            );
            assert_eq!(
                graph["Edges"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the connection from the trigger to the job, got {graph}"
            );
        }
    }
}

#[tokio::test]
async fn listing_runs_reports_the_state_the_clock_owes_and_changes_nothing_else() {
    let (router, engine) = emulator_with_engine();
    let name = "settling-workflow";
    let start_offsets: &[u64] = &[10, 20, 30];
    given_workflow_with_runs(&router, &engine, name, start_offsets).await;

    // Two listings of runs whose transition is not yet due report the SAME runs: reading a
    // listing moves nothing.
    let first = ok(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
    let second = ok(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
    assert_eq!(
        first, second,
        "case \"nothing due\": two listings must report the same runs, got {first} and {second}"
    );
    for run in listed_runs(&first) {
        assert_eq!(
            run["Status"].as_str(),
            Some(WORKFLOW_RUN_STATUS_RUNNING),
            "case \"nothing due\": a run whose transition is not due is in flight, got {run}"
        );
        assert!(
            run.get("CompletedOn").is_none_or(Value::is_null),
            "case \"nothing due\": a run that has not finished reports no CompletedOn, got {run}"
        );
    }

    // Once the clock owes the transition, the listing reports it — and keeps reporting the
    // same thing, so the listing materialises what was due rather than moving a run because
    // it was read.
    set_due_instant(&engine, name, SystemTime::UNIX_EPOCH);
    let settled = ok(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
    let again = ok(&router, "GetWorkflowRuns", &json!({ "Name": name })).await;
    assert_eq!(
        settled, again,
        "case \"the transition fell due\": two listings of settled runs must agree, got \
         {settled} and {again}"
    );
    for run in listed_runs(&settled) {
        assert_eq!(
            run["Status"].as_str(),
            Some(WORKFLOW_RUN_STATUS_COMPLETED),
            "case \"the transition fell due\": expected the status the clock owed, got {run}"
        );
        assert!(
            run.get("ErrorMessage").is_none_or(Value::is_null),
            "case \"the transition fell due\": a run that ran its course reports no error, \
             got {run}"
        );
    }
}

#[tokio::test]
async fn the_listing_resolves_a_workflow_by_the_same_key_every_workflow_reader_does() {
    let (router, engine) = emulator_with_engine();
    let name = "cross-endpoint-workflow";
    let started = given_workflow_with_runs(&router, &engine, name, &[10, 20]).await;
    let expected = newest_first(&started, &[10, 20]);

    // A workflow is addressed by `Name` alone — no `CatalogId` and no Region member on any
    // operation the model declares over one — so the listing resolves it by the same key
    // `GetWorkflowRun` does, at whichever endpoint the read is served. The rows are the two
    // endpoints: the one the workflow was created through, and another.
    for (case, region) in [
        ("the endpoint the workflow was created through", REGION),
        ("another Region's endpoint", OTHER_REGION),
    ] {
        let (status, page) =
            call_in_region(&router, region, "GetWorkflowRuns", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {page}"
        );
        assert_eq!(
            listed_ids(&page),
            expected,
            "case {case:?}: expected the workflow's runs newest start first, got {page}"
        );

        // The graph, in contrast, is read by the Region: a trigger is served only through
        // the endpoint that created it, so each endpoint reports the components attached
        // through it — and the listing reports exactly what the single-run read reports at
        // that same endpoint.
        let (graphed_status, graphed) = call_in_region(
            &router,
            region,
            "GetWorkflowRuns",
            &json!({ "Name": name, "IncludeGraph": true }),
        )
        .await;
        assert_eq!(
            graphed_status, 200,
            "case {case:?}: expected 200 with a graph asked for, got {graphed_status} with \
             {graphed}"
        );
        let (read_status, read) = call_in_region(
            &router,
            region,
            "GetWorkflowRun",
            &json!({ "Name": name, "RunId": &expected[0], "IncludeGraph": true }),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from the single-run read, got {read_status} with \
             {read}"
        );
        assert_eq!(
            listed_runs(&graphed).first().and_then(|run| run.get("Graph")),
            read["Run"].get("Graph"),
            "case {case:?}: expected the graph the single-run read reports at this endpoint, \
             got {graphed} and {read}"
        );
    }
}
