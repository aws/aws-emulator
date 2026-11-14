// SPDX-License-Identifier: CC0-1.0

//! `GetJobRuns`: the runs of ONE job definition, newest first, in pages.
//!
//! RULE: a paginated read of a resource's children reports every child the store holds of
//! the named parent — nothing of any other parent — in the order the operation documents,
//! projected exactly as the single-child read projects it, one `MaxResults`-sized page at a
//! time with a `NextToken` that resumes the walk and no token on the last page; a parent
//! the store does not hold is refused with the operation's modelled not-found error. For
//! this operation the documented order is the chronological one, "with the newest jobs
//! returned first", so it is over the instant each run STARTED and not over the order the
//! store happens to hold the runs in.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many runs are stored, in what
//! order their start instants fall against the order they were recorded in, the page size
//! asked for (including both published bounds of `MaxResults` and one past each), and
//! whether the job is one the store holds.
//!
//! The start instants and the due instants the rows turn on are written THROUGH THE STORE
//! rather than slept for, so a row's expected order is written down rather than inferred
//! from how fast the clock ticked between two starts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::JOB_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_STARTING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_on, error_code, error_message, ok};

/// A role ARN `CreateJob` accepts.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-job-run-listing-role";

/// The instant a row's start offsets are measured from, so the order a row expects is
/// fixed by the row itself rather than by the clock.
const START_EPOCH: Duration = Duration::from_secs(1_767_225_600);

/// How far out the transition a run is owed is held while a listing is read, so every run
/// stays in flight and two listings of it report the same run.
const HELD_OPEN: Duration = Duration::from_secs(3_600);

/// An emulator plus the engine it runs on, so a test can write the instants the run
/// lifecycle turns on instead of waiting for them.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Create `name` and start one run of it per entry of `start_offsets`, then give the runs
/// those start instants — in the order the store recorded them — and hold the transition
/// each is owed far out. Answers the run ids, in the order they were started.
async fn given_job_with_runs(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    start_offsets: &[u64],
) -> Vec<String> {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            // Every run of a row is started before any of them finishes, so the job must
            // allow them all to be in flight at once.
            "ExecutionProperty": { "MaxConcurrentRuns": 50 },
        }),
    )
    .await;

    let mut started = Vec::new();
    for _ in start_offsets {
        let answer = ok(router, "StartJobRun", &json!({ "JobName": name })).await;
        started.push(
            answer
                .get("JobRunId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {answer}"))
                .to_string(),
        );
    }

    set_start_instants(engine, name, start_offsets);
    set_due_instant(engine, name, SystemTime::now() + HELD_OPEN);
    started
}

/// Give the runs of `job_name` the start instants `offsets` names, in the order the store
/// recorded them, so the order a row expects is fixed by the row rather than by how fast
/// the clock ticked between two starts.
fn set_start_instants(engine: &Arc<engine::InMemoryEngine>, job_name: &str, offsets: &[u64]) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let job = store
        .job_mut(crate::wire::REGION, job_name)
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"));
    assert_eq!(
        job.runs.len(),
        offsets.len(),
        "the store must hold one run of {job_name:?} per start instant the row names"
    );
    for (run, offset) in job.runs.iter_mut().zip(offsets) {
        run.started_on = SystemTime::UNIX_EPOCH + START_EPOCH + Duration::from_secs(*offset);
    }
}

/// Move every due instant of every run of `job_name` to `at`, so a row can make what the
/// clock owes due without sleeping.
///
/// A run owes up to TWO states at once — the one it is in while its script runs, and the one
/// its own execution came to — so both instants move, or a run would stop half way.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, job_name: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let job = store
        .job_mut(crate::wire::REGION, job_name)
        .unwrap_or_else(|| panic!("the store must hold job {job_name:?}"));
    for run in &mut job.runs {
        run.settles_at = Some(at);
        if run.rests_at.is_some() {
            run.rests_at = Some(at);
        }
    }
}

/// The `Id` of every run one `GetJobRuns` answer reports, in the order it reported them.
fn listed_ids(page: &Value) -> Vec<String> {
    listed_runs(page)
        .iter()
        .map(|run| {
            run["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("a JobRun carries an Id, got {run}"))
                .to_string()
        })
        .collect()
}

/// The `JobRuns` of one answer.
fn listed_runs(page: &Value) -> Vec<Value> {
    page["JobRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("GetJobRuns must answer a JobRuns list, got {page}"))
        .clone()
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

/// Walk every page `GetJobRuns` hands out for `job_name`, following its own token, and
/// answer the ids in the order they were served plus how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    job_name: &str,
    page_size: Option<i32>,
    stored: usize,
) -> (Vec<String>, usize) {
    let mut served: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "JobName": job_name });
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetJobRuns", &request).await;
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
        ("five runs, start instants against the recorded order", &[50, 10, 40, 20, 30]),
    ];

    for (case, start_offsets) in rows {
        let (router, engine) = emulator_with_engine();
        let name = "listed-job";
        let started = given_job_with_runs(&router, &engine, name, start_offsets).await;

        let page = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
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
            let read = ok(
                &router,
                "GetJobRun",
                &json!({ "JobName": name, "RunId": id }),
            )
            .await;
            assert_eq!(
                listed, &read["JobRun"],
                "case {case:?}: the listing's element {index} must be the run GetJobRun \
                 reports for {id}, got {listed} and {read}"
            );
            assert_eq!(
                listed["JobRunState"].as_str(),
                Some(JOB_RUN_STATE_STARTING),
                "case {case:?}: a run whose transition is not yet due is still in flight, \
                 got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_whose_runs_were_never_started_lists_none() {
    let (router, _engine) = emulator_with_engine();
    let name = "unrun-job";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;

    let page = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    assert_eq!(
        listed_runs(&page),
        Vec::<Value>::new(),
        "case \"a job with no runs\": expected an empty listing, got {page}"
    );
    assert!(
        page.get("NextToken").is_none(),
        "case \"a job with no runs\": an empty listing carries no token, got {page}"
    );
}

#[tokio::test]
async fn every_page_size_walks_every_run_exactly_once() {
    let (router, engine) = emulator_with_engine();
    let name = "paged-job";
    // Seven runs whose start instants deliberately disagree with the order they were
    // recorded in, so paging is asserted over the documented order and not over insertion.
    let start_offsets: &[u64] = &[70, 10, 60, 20, 50, 30, 40];
    let started = given_job_with_runs(&router, &engine, name, start_offsets).await;
    let expected = newest_first(&started, start_offsets);

    // (case, the page size asked for, how many pages the walk must take)
    let rows: &[(&str, Option<i32>, usize)] = &[
        ("no page size", None, 1),
        // The published lower bound of `MaxResults` (`OrchestrationPageSize200`, range
        // 1..200): one run per page.
        ("the smallest page the model accepts", Some(1), 7),
        ("a page smaller than the stored count", Some(3), 3),
        ("a page one short of the stored count", Some(6), 2),
        ("a page exactly the stored count", Some(7), 1),
        ("a page larger than the stored count", Some(50), 1),
        // The published upper bound.
        ("the largest page the model accepts", Some(200), 1),
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
    let name = "bounded-job";
    given_job_with_runs(&router, &engine, name, &[10]).await;

    // `MaxResults` is `OrchestrationPageSize200`, range 1..200: one past each published
    // bound is refused by the modelled constraint rather than served as some other page
    // size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 201)] {
        let (status, body) = call(
            &router,
            "GetJobRuns",
            &json!({ "JobName": name, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn the_listing_reports_only_the_runs_of_the_job_it_names() {
    let (router, engine) = emulator_with_engine();
    let left = given_job_with_runs(&router, &engine, "job-left", &[10, 30]).await;
    let right = given_job_with_runs(&router, &engine, "job-right", &[20]).await;

    // (case, the job asked about, the runs it holds, their start instants)
    let rows: &[(&str, &str, &[String], &[u64])] = &[
        ("the job with two runs", "job-left", &left, &[10, 30]),
        ("the job with one run", "job-right", &right, &[20]),
    ];

    for (case, job_name, held, start_offsets) in rows {
        let page = ok(&router, "GetJobRuns", &json!({ "JobName": job_name })).await;
        assert_eq!(
            listed_ids(&page),
            newest_first(held, start_offsets),
            "case {case:?}: expected only this job's runs, got {page}"
        );
        for run in listed_runs(&page) {
            assert_eq!(
                run["JobName"].as_str(),
                Some(*job_name),
                "case {case:?}: every listed run names the job it belongs to, got {run}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_the_store_does_not_hold_is_not_found() {
    let (router, engine) = emulator_with_engine();
    given_job_with_runs(&router, &engine, "held-job", &[10]).await;

    // A job name is NOT folded, so a name differing only in case names a different job —
    // and a page size or a token on the request does not change the refusal.
    let rows: &[(&str, &str, Value)] = &[
        ("a name no job has", "absent-job", json!({})),
        ("the held job's name in another case", "HELD-JOB", json!({})),
        (
            "a name no job has, with a page size",
            "absent-job",
            json!({ "MaxResults": 5 }),
        ),
        (
            "a name no job has, with a token",
            "absent-job",
            json!({ "NextToken": "00000000000000000000:jr_0" }),
        ),
    ];

    for (case, job_name, extra) in rows {
        let mut request = json!({ "JobName": job_name });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let (status, body) = call(&router, "GetJobRuns", &request).await;
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
            JOB_NOT_FOUND,
            "case {case:?}: expected the wording the corpus records for this listing's job \
             miss, got {body}"
        );
    }

    // The job the store DOES hold is served, so the refusal is about the name and not
    // about the operation.
    let page = ok(&router, "GetJobRuns", &json!({ "JobName": "held-job" })).await;
    assert_eq!(
        listed_runs(&page).len(),
        1,
        "case \"the held job\": expected its one run, got {page}"
    );
}

#[tokio::test]
async fn listing_runs_reports_the_state_the_clock_owes_and_changes_nothing_else() {
    let (router, engine) = emulator_with_engine();
    let name = "settling-job";
    let start_offsets: &[u64] = &[10, 20, 30];
    given_job_with_runs(&router, &engine, name, start_offsets).await;

    // Two listings of runs whose transition is not yet due report the SAME runs: reading
    // a listing moves nothing.
    let first = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    let second = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    assert_eq!(
        first, second,
        "case \"nothing due\": two listings must report the same runs, got {first} and \
         {second}"
    );
    for run in listed_runs(&first) {
        assert_eq!(
            run["JobRunState"].as_str(),
            Some(JOB_RUN_STATE_STARTING),
            "case \"nothing due\": a run whose transition is not due is in flight, got {run}"
        );
    }

    // Once the clock owes the transition, the listing reports it — and keeps reporting the
    // same thing, so the listing materialises what was due rather than moving a run
    // because it was read.
    set_due_instant(&engine, name, SystemTime::UNIX_EPOCH);
    let settled = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    let again = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    assert_eq!(
        settled, again,
        "case \"the transition fell due\": two listings of settled runs must agree, got \
         {settled} and {again}"
    );
    for run in listed_runs(&settled) {
        assert_eq!(
            run["JobRunState"].as_str(),
            Some(JOB_RUN_STATE_FAILED),
            "case \"the transition fell due\": expected the state the clock owed, got {run}"
        );
        crate::job_runs::assert_script_was_unreadable("the transition fell due", &run);
    }
}

#[tokio::test]
async fn a_retry_chain_is_listed_newest_first_with_each_link_intact() {
    let (router, engine) = emulator_with_engine();
    let name = "chained-job";
    ok(
        &router,
        "CreateJob",
        &json!({
            "Name": name,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            // No retry budget: the chain under test is the one these rows build with
            // explicit `StartJobRun$JobRunId` starts, so the service owes the job no
            // further attempts of its own (`Job$MaxRetries`, covered by
            // `crate::job_run_retries`).
        }),
    )
    .await;

    // Each start retries the run before it, which must have finished first — so the chain
    // is as long as the rows ask and every link is a run of its own.
    let mut chain: Vec<String> = Vec::new();
    for attempt in 0..3usize {
        let mut request = json!({ "JobName": name });
        if let Some(previous) = chain.last() {
            request["JobRunId"] = json!(previous);
        }
        let answer = ok(&router, "StartJobRun", &request).await;
        let id = answer
            .get("JobRunId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {answer}"))
            .to_string();
        chain.push(id);
        // The run just started must finish before the next attempt retries it.
        set_due_instant(&engine, name, SystemTime::UNIX_EPOCH);
        assert_eq!(
            chain.len(),
            attempt + 1,
            "case attempt {attempt}: every start is a run of its own, got {chain:?}"
        );
    }

    // The chain's attempts were started microseconds apart, so the row writes their start
    // instants rather than leaning on the clock's granularity to tell them apart.
    set_start_instants(&engine, name, &[10, 20, 30]);

    let page = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
    let listed = listed_runs(&page);    assert_eq!(
        listed.len(),
        chain.len(),
        "case \"a chain of three attempts\": expected every attempt, got {page}"
    );
    // Newest first: the listing walks the chain backwards, and each link still names the
    // run it retries and its own attempt number.
    for (index, run) in listed.iter().enumerate() {
        let position = chain.len() - 1 - index;
        assert_eq!(
            run["Id"].as_str(),
            Some(chain[position].as_str()),
            "case \"a chain of three attempts\": expected attempt {position} at position \
             {index}, got {run}"
        );
        assert_eq!(
            run["Attempt"].as_i64(),
            i64::try_from(position).ok(),
            "case \"a chain of three attempts\": expected attempt number {position}, got {run}"
        );
        assert_eq!(
            run["PreviousRunId"].as_str(),
            position
                .checked_sub(1)
                .map(|earlier| chain[earlier].as_str()),
            "case \"a chain of three attempts\": expected the run attempt {position} \
             retries, got {run}"
        );
    }
}
