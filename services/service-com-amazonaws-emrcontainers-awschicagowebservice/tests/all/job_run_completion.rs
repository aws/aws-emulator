// SPDX-License-Identifier: CC0-1.0

//! Where an admitted job run's driver ENDS: the state the compute it was admitted to
//! accounts for.
//!
//! Every call here goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//! a job run this service admitted to a virtual cluster reaches, when the end of its
//! driver falls due, the terminal state that cluster's compute accounts for —
//!
//! 1. while the cluster the run was admitted to is available (held at the run's own
//!    region and still `RUNNING`), the run's driver reached its end, so the run settles in
//!    the modelled state for a job run that finished, with a `finishedAt` strictly after
//!    its `createdAt` and NO `failureReason`; and
//! 2. once that cluster is no longer available, there is no compute left to finish the
//!    run on, so it settles in the modelled failure state with the `FailureReason` that
//!    names unavailable compute.
//!
//! Rule 1 holds for every submission the service admits — whichever `JobDriver` member it
//! carries, and whether the run was admitted straight away or only once a concurrency slot
//! freed — because none of that is what the end of the driver is decided by.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::{
    FAILURE_REASON_VALUES, JOB_RUN_STATE_VALUES,
};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::job_run_outcome::poll_until_terminal;
use crate::virtual_cluster_job_run_loop::{field, instant_millis, text, Emr};
use crate::virtual_cluster_scheduler_status::{cluster_id, wait_for_the_clock};

/// The `IAMRoleArn` every submission below runs under.
const ROLE: &str = "arn:aws:iam::000000000000:role/EMRJobRole";

/// One submission: the label of the row and the members it carries beside its
/// `clientToken`.
struct Submission {
    label: &'static str,
    members: Value,
}

/// The rows differ along everything rule 1 generalises over: which of the two modelled
/// `JobDriver` members the submission names, what the driver's `entryPoint` points at (a
/// path inside the image, an object in a bucket, a SQL file), which optional members the
/// submission fills, and the `releaseLabel` it asks for.
fn submissions() -> Vec<Submission> {
    vec![
        Submission {
            label: "a spark-submit of a jar on a path inside the image",
            members: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "local:///usr/lib/spark/examples/jars/spark-examples.jar",
                    "sparkSubmitParameters":
                        "--class org.apache.spark.examples.SparkPi --conf spark.executor.instances=1",
                }},
            }),
        },
        Submission {
            label: "a spark-submit of an object in a bucket, with arguments",
            members: json!({
                "name": "entry-point-arguments",
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/jobs/aggregate.py",
                    "entryPointArguments": ["--rows", "1000"],
                }},
                "tags": {"owner": "analytics"},
            }),
        },
        Submission {
            label: "a spark-sql driver with a retry policy",
            members: json!({
                "name": "sql_completion.1",
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-7.0.0-latest",
                "jobDriver": {"sparkSqlJobDriver": {
                    "entryPoint": "s3://bucket/queries/rollup.sql",
                    "sparkSqlParameters": "-e select 1",
                }},
                "retryPolicyConfiguration": {"maxAttempts": 2},
            }),
        },
    ]
}

/// `POST /virtualclusters/{cluster}/jobruns` for one row, with the token this run owns.
async fn submit(emr: &Emr, cluster: &str, row: &Submission, token: &str) -> String {
    let mut body = row.members.clone();
    body["clientToken"] = json!(token);
    let started = emr.start_job(cluster, body).await;
    text(started.ok(row.label), "/id", row.label)
}

/// `GET /virtualclusters/{cluster}/jobruns/{run}`, as the `jobRun` it reports.
async fn described(emr: &Emr, case: &str, cluster: &str, run: &str) -> Value {
    let answer = emr.describe_job(cluster, run).await;
    field(answer.ok(case), "/jobRun", case).clone()
}

/// Assert the run REPORTS a finished job run of AWS's own vocabulary: a terminal
/// `JobRunState`, a `finishedAt` strictly after `createdAt`, and a `stateDetails` within
/// `String256` that is not the one it carried while it was running.
fn assert_finished_run(case: &str, settled: &Value, created_at: i64, running_detail: &str) {
    let reported = text(settled, "/state", case);
    assert!(
        JOB_RUN_STATE_VALUES.contains(&reported.as_str()),
        "case {case:?}: {reported:?} is not a modelled JobRunState"
    );
    let finished_at = instant_millis(settled, "/finishedAt", case);
    assert!(
        finished_at > created_at,
        "case {case:?}: a run that finished reports a finishedAt strictly after its \
         createdAt, got finishedAt {finished_at} against createdAt {created_at}"
    );
    let detail = text(settled, "/stateDetails", case);
    assert!(
        (1..=256).contains(&detail.chars().count()) && detail.chars().any(|c| !c.is_whitespace()),
        "case {case:?}: stateDetails must satisfy String256, got {detail:?}"
    );
    assert_ne!(
        detail, running_detail,
        "case {case:?}: the detail must describe the state the run is reported in, not the \
         one it was in before"
    );
}

/// RULE 1: whatever the submission carried, a run admitted to a cluster that stays
/// available settles in the state of a run that finished — with no `failureReason` — and
/// a later read answers the very same run.
#[tokio::test]
async fn a_run_whose_cluster_stays_available_finishes_whatever_it_submitted() {
    let emr = Emr::new();
    let cluster = cluster_id(
        &emr,
        "the cluster the rows run on",
        "job-completion",
        "tok-job-completion",
        None,
    )
    .await;
    let rows = submissions();

    // Every row is submitted first, so one span of the clock covers them all.
    let mut submitted = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let id = submit(&emr, &cluster, row, &format!("tok-completion-{index}")).await;
        let accepted = described(&emr, row.label, &cluster, &id).await;
        let created_at = instant_millis(&accepted, "/createdAt", row.label);
        submitted.push((row, id, created_at));
    }

    // The admission, and the detail the run reports while its driver runs.
    wait_for_the_clock();
    let mut running_details = Vec::new();
    for (row, id, _) in &submitted {
        let running = described(&emr, row.label, &cluster, id).await;
        assert_eq!(
            text(&running, "/state", row.label),
            state::JOB_RUNNING,
            "case {:?}: the clock owed this run RUNNING, got {running}",
            row.label
        );
        running_details.push(text(&running, "/stateDetails", row.label));
    }

    for ((row, id, created_at), running_detail) in submitted.iter().zip(&running_details) {
        let case = row.label;
        let settled = poll_until_terminal(&emr, case, &cluster, id).await;
        assert_eq!(
            text(&settled, "/state", case),
            state::JOB_COMPLETED,
            "case {case:?}: the cluster was available throughout, so the run's driver \
             reached its end: {settled}"
        );
        assert!(
            settled.get("failureReason").is_none(),
            "case {case:?}: a run that finished did not fail, so it reports no \
             failureReason: {settled}"
        );
        assert_finished_run(case, &settled, *created_at, running_detail);

        // Reading a finished run neither moves it nor rewrites what it settled as.
        let again = described(&emr, case, &cluster, id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: describing a finished run twice answered two different runs"
        );
    }
}

/// RULE 1 again, where the run reached `RUNNING` the OTHER way: a cluster that admits one
/// run at a time holds the second in the queue, and the end of the first is what admits
/// it. The route into `RUNNING` is not what the end of a driver is decided by, so the
/// queued run finishes exactly as the admitted one did.
#[tokio::test]
async fn a_run_admitted_from_the_queue_finishes_like_one_admitted_at_once() {
    let emr = Emr::new();
    // `JobLimitInteger`'s @range is 1..1000, so 1 is the published minimum.
    let cluster = cluster_id(
        &emr,
        "a cluster that admits one run at a time",
        "queued-completion",
        "tok-queued-completion",
        Some(json!({"maxConcurrentJobRuns": 1})),
    )
    .await;
    let row = &submissions()[0];

    let first = submit(&emr, &cluster, row, "tok-queued-first").await;
    let second = submit(&emr, &cluster, row, "tok-queued-second").await;
    let created_at = instant_millis(
        &described(&emr, row.label, &cluster, &second).await,
        "/createdAt",
        row.label,
    );

    wait_for_the_clock();
    let case = "the run the limit held in the queue";
    assert_eq!(
        text(&described(&emr, case, &cluster, &second).await, "/state", case),
        state::JOB_PENDING,
        "case {case:?}: the concurrency limit admits one run at a time"
    );

    // The first run's end frees the slot the second one was waiting for.
    let settled_first =
        poll_until_terminal(&emr, "the run the limit admitted", &cluster, &first).await;
    assert_eq!(
        text(&settled_first, "/state", case),
        state::JOB_COMPLETED,
        "case {case:?}: the run that held the slot finished: {settled_first}"
    );

    // The admission the freed slot gives the queued run, and the detail it reports while
    // its own driver runs. Bounded, so a run that never reaches RUNNING — or one already
    // past it — fails the case instead of spinning.
    let deadline = Instant::now() + state::JOB_OUTCOME_SPAN + Duration::from_secs(20);
    let running_detail = loop {
        let promoted = described(&emr, case, &cluster, &second).await;
        let reported = text(&promoted, "/state", case);
        if reported == state::JOB_RUNNING {
            break text(&promoted, "/stateDetails", case);
        }
        assert_eq!(
            reported,
            state::JOB_PENDING,
            "case {case:?}: the freed slot must admit the run that was waiting, and this \
             case has to observe it RUNNING: {promoted}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run the limit held was never admitted: {promoted}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    let settled = poll_until_terminal(&emr, case, &cluster, &second).await;
    assert_eq!(
        text(&settled, "/state", case),
        state::JOB_COMPLETED,
        "case {case:?}: a run admitted from the queue finishes like any other: {settled}"
    );
    assert!(
        settled.get("failureReason").is_none(),
        "case {case:?}: a run that finished reports no failureReason: {settled}"
    );
    assert_finished_run(case, &settled, created_at, &running_detail);
}

/// How a row of the rule below leaves the compute the run was admitted to, by the time the
/// end of the run's driver falls due.
#[derive(Debug, Clone, Copy)]
enum Compute {
    /// The cluster is still held and still `RUNNING`.
    Available,
    /// The cluster is still held, in a `VirtualClusterState` it no longer hosts work in.
    InState(&'static str),
    /// The cluster is no longer held at this endpoint at all.
    Gone,
}

/// One row: what becomes of the compute, and the terminal state and `failureReason` the
/// run then settles in.
struct ComputeCase {
    label: &'static str,
    compute: Compute,
    expected_state: &'static str,
    expected_reason: Option<&'static str>,
}

/// RULE 1 + RULE 2: the end of an admitted run's driver is decided by the compute the run
/// was admitted to, read when that end falls due.
///
/// The rows differ along exactly that: a cluster left alone, a cluster in each of the
/// `VirtualClusterState` values a torn-down cluster passes through (source: the generated
/// `VIRTUAL_CLUSTER_STATE_VALUES` catalog in `ops::types::shared_types`, of which
/// `RUNNING` is the only state a cluster hosts work in), and a cluster this endpoint no
/// longer holds.
///
/// Every row's run is submitted and admitted through the API. What no REQUEST can stage is
/// the compute going away BEHIND a run nobody cancelled — `DeleteVirtualCluster` carries
/// the runs of the cluster it tears down into `CANCEL_PENDING` with it, which is a
/// different end and is covered by [`crate::virtual_cluster_teardown_cascade`] — so those
/// rows stage it in the store the handlers themselves read.
#[tokio::test]
async fn the_end_of_a_driver_is_decided_by_the_compute_the_run_was_admitted_to() {
    let rows = [
        ComputeCase {
            label: "the cluster is still running",
            compute: Compute::Available,
            expected_state: state::JOB_COMPLETED,
            expected_reason: None,
        },
        ComputeCase {
            label: "the cluster's teardown has started",
            compute: Compute::InState(state::CLUSTER_TERMINATING),
            expected_state: state::JOB_FAILED,
            expected_reason: Some(state::JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON),
        },
        ComputeCase {
            label: "the cluster has been torn down",
            compute: Compute::InState(state::CLUSTER_TERMINATED),
            expected_state: state::JOB_FAILED,
            expected_reason: Some(state::JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON),
        },
        ComputeCase {
            label: "the cluster is no longer held at this endpoint",
            compute: Compute::Gone,
            expected_state: state::JOB_FAILED,
            expected_reason: Some(state::JOB_CLUSTER_UNAVAILABLE_FAILURE_REASON),
        },
    ];

    let emr = Emr::new();
    let row_submission = &submissions()[0];

    // One cluster and one admitted run per row, so a row's compute is staged without
    // touching any other row's.
    let mut staged = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let cluster = cluster_id(
            &emr,
            row.label,
            &format!("compute-outcome-{index}"),
            &format!("tok-compute-outcome-{index}"),
            None,
        )
        .await;
        let run = submit(
            &emr,
            &cluster,
            row_submission,
            &format!("tok-compute-run-{index}"),
        )
        .await;
        let created_at = instant_millis(
            &described(&emr, row.label, &cluster, &run).await,
            "/createdAt",
            row.label,
        );
        staged.push((row, cluster, run, created_at));
    }

    wait_for_the_clock();
    let mut running_details = Vec::new();
    for (row, cluster, run, _) in &staged {
        let running = described(&emr, row.label, cluster, run).await;
        assert_eq!(
            text(&running, "/state", row.label),
            state::JOB_RUNNING,
            "case {:?}: every row's run must be admitted before its compute is staged, got \
             {running}",
            row.label
        );
        running_details.push(text(&running, "/stateDetails", row.label));
    }

    // The compute each row's run is left with, while its driver is still running.
    {
        let store = emr.store();
        let mut state = store.lock();
        for (row, cluster, _, _) in &staged {
            match row.compute {
                Compute::Available => {}
                Compute::InState(cluster_state) => {
                    let record = state
                        .virtual_clusters
                        .get_mut(cluster)
                        .unwrap_or_else(|| panic!("case {:?}: the cluster is stored", row.label));
                    record.state = cluster_state.to_string();
                    record.pending = None;
                }
                Compute::Gone => {
                    state.virtual_clusters.remove(cluster);
                }
            }
        }
    }

    for ((row, cluster, run, created_at), running_detail) in staged.iter().zip(&running_details) {
        let case = row.label;
        let settled = poll_until_terminal(&emr, case, cluster, run).await;
        assert_eq!(
            text(&settled, "/state", case),
            row.expected_state,
            "case {case:?}: the compute the run was admitted to decides the end of its \
             driver, expected {:?}, got {settled}",
            row.expected_state
        );
        assert_eq!(
            settled.get("failureReason").and_then(Value::as_str),
            row.expected_reason,
            "case {case:?}: expected failureReason {:?}, got {settled}",
            row.expected_reason
        );
        if let Some(reason) = row.expected_reason {
            assert!(
                FAILURE_REASON_VALUES.contains(&reason),
                "case {case:?}: {reason:?} is not a modelled FailureReason"
            );
        }
        assert_finished_run(case, &settled, *created_at, running_detail);
    }
}
