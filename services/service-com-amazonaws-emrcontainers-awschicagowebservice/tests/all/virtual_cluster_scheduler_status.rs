// SPDX-License-Identifier: CC0-1.0

//! `VirtualCluster.schedulerStatus`, and the `schedulerConfiguration` limits its counts
//! are the use of.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `schedulerStatus` is COUNTED out of the job runs the service holds for the cluster:
//!    `currentInQueueJobRuns` is its runs in `PENDING` or `SUBMITTED`,
//!    `currentConcurrentJobRuns` its runs in `RUNNING`. Both members default to `0`, so a
//!    cluster holding no run reports zeros rather than nothing, and a run that has left
//!    both sets (a cancelled one) counts in neither.
//! 2. `schedulerConfiguration.maxConcurrentJobRuns` is the number of runs that may be
//!    `RUNNING` at once: a run the clock has made due waits in the queue while the limit
//!    is taken, and "as running slots free up, queued job runs start automatically" — in
//!    submission order.
//! 3. `schedulerConfiguration.maxInQueueJobRuns` is the number of runs that may wait at
//!    once: "when the queue is full, the service rejects `StartJobRun` requests with a
//!    `ValidationException`", and the submission that was refused is not in the queue.
//! 4. Each limit's published `@range` is honoured at both bounds, and a value one past
//!    either bound is refused.
//!
//! The helpers below are `pub(crate)` because the suites beside this one
//! ([`crate::virtual_cluster_scheduler`] and [`crate::virtual_cluster_update`]) drive the
//! same endpoints: one definition of each call, so no two suites can disagree about what
//! a request looks like.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};

/// Wait until a status transition the service owes has come due, with margin, so the next
/// call materialises it. The service stores the due instant and never sleeps; this is the
/// test's own clock.
pub(crate) fn wait_for_the_clock() {
    std::thread::sleep(state::TRANSITION_SPAN + Duration::from_millis(250));
}

/// A `KubernetesNamespace` of this suite's container cluster for a fixture named `name`.
///
/// A virtual cluster maps 1:1 onto a namespace, so every fixture cluster needs its OWN:
/// the characters the shape admits are `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$` with length
/// 1..63 (smithy_query: shape `EksInfo`), so anything else in the name maps to `-` and
/// the value is framed so that it starts and ends on an accepted character.
pub(crate) fn namespace_for(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    let core: String = mapped.trim_matches('-').chars().take(56).collect();
    let core = core.trim_matches('-');
    if core.is_empty() {
        "ns".to_string()
    } else {
        format!("{core}-ns")
    }
}

/// `POST /virtualclusters`, carrying `scheduler` as the cluster's
/// `schedulerConfiguration` when the row has one.
pub(crate) async fn create_cluster(
    emr: &Emr,
    name: &str,
    token: &str,
    scheduler: Option<Value>,
) -> Answer {
    let mut body = json!({
        "name": name,
        "clientToken": token,
        "containerProvider": {
            "type": "EKS",
            "id": "eks-scheduler",
            "info": {"eksInfo": {"namespace": namespace_for(name)}},
        },
    });
    if let (Some(target), Some(scheduler)) = (body.as_object_mut(), scheduler) {
        target.insert("schedulerConfiguration".to_string(), scheduler);
    }
    emr.send("POST", "/virtualclusters", Some(body)).await
}

/// The id of a cluster the create must have accepted.
pub(crate) async fn cluster_id(
    emr: &Emr,
    case: &str,
    name: &str,
    token: &str,
    scheduler: Option<Value>,
) -> String {
    let created = create_cluster(emr, name, token, scheduler).await;
    text(created.ok(case), "/id", case)
}

/// `POST /virtualclusters/{cluster}/jobruns` with a Spark driver.
pub(crate) async fn start_run(emr: &Emr, cluster: &str, token: &str) -> Answer {
    emr.send(
        "POST",
        &format!("/virtualclusters/{cluster}/jobruns"),
        Some(json!({
            "clientToken": token,
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.10.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/main.py"}},
        })),
    )
    .await
}

/// The id of a job run the submission must have accepted.
pub(crate) async fn run_id(emr: &Emr, case: &str, cluster: &str, token: &str) -> String {
    let started = start_run(emr, cluster, token).await;
    text(started.ok(case), "/id", case)
}

/// `DELETE /virtualclusters/{cluster}/jobruns/{run}`.
pub(crate) async fn cancel_run(emr: &Emr, cluster: &str, run: &str) -> Answer {
    emr.send(
        "DELETE",
        &format!("/virtualclusters/{cluster}/jobruns/{run}"),
        None,
    )
    .await
}

/// The `state` `DescribeJobRun` reports for one run.
pub(crate) async fn run_state(emr: &Emr, case: &str, cluster: &str, run: &str) -> String {
    let described = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{run}"),
            None,
        )
        .await;
    text(described.ok(case), "/jobRun/state", case)
}

/// The `(currentInQueueJobRuns, currentConcurrentJobRuns)` `DescribeVirtualCluster`
/// reports for this cluster.
pub(crate) async fn scheduler_status(emr: &Emr, case: &str, cluster: &str) -> (i64, i64) {
    let described = emr
        .send("GET", &format!("/virtualclusters/{cluster}"), None)
        .await;
    let body = described.ok(case).clone();
    let status = field(&body, "/virtualCluster/schedulerStatus", case).clone();
    let count = |member: &str| -> i64 {
        field(&status, &format!("/{member}"), case)
            .as_i64()
            .unwrap_or_else(|| {
                panic!("case {case:?}: schedulerStatus/{member} is not a number: {status}")
            })
    };
    (
        count("currentInQueueJobRuns"),
        count("currentConcurrentJobRuns"),
    )
}

/// One row of RULE 1: a cluster, the scheduler configuration it was created with (if
/// any), and how many job runs are submitted to it.
struct StatusCase {
    label: &'static str,
    scheduler: Option<Value>,
    runs: i64,
}

/// RULE 1: `schedulerStatus` counts the cluster's stored job runs, in both phases of the
/// transition the clock owes them.
///
/// The rows differ in whether the cluster configures a scheduler at all, which limits it
/// configures, and how many runs it holds (none, one, several) — the counts are a fact
/// about the stored runs, so none of that may change whether they are reported.
#[tokio::test]
async fn scheduler_status_counts_the_clusters_stored_job_runs() {
    let emr = Emr::new();
    let cases = [
        StatusCase {
            label: "no scheduler configuration and no job run",
            scheduler: None,
            runs: 0,
        },
        StatusCase {
            label: "no scheduler configuration and one job run",
            scheduler: None,
            runs: 1,
        },
        StatusCase {
            label: "no scheduler configuration and three job runs",
            scheduler: None,
            runs: 3,
        },
        StatusCase {
            label: "a queue-depth limit only",
            scheduler: Some(json!({"maxInQueueJobRuns": 100})),
            runs: 2,
        },
        StatusCase {
            label: "both limits, well above the runs submitted",
            scheduler: Some(json!({"maxInQueueJobRuns": 1000, "maxConcurrentJobRuns": 10})),
            runs: 5,
        },
        StatusCase {
            // `JobLimitInteger`'s published maximum, exercised with runs under it: the
            // size of the limit may not change that every run starts.
            label: "the published maximum concurrency, with fewer runs than slots",
            scheduler: Some(json!({"maxConcurrentJobRuns": 1000})),
            runs: 2,
        },
    ];


    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let cluster = cluster_id(
            &emr,
            label,
            &format!("status-{index}"),
            &format!("tok-status-{index}"),
            case.scheduler.clone(),
        )
        .await;

        // A cluster holding no run reports zeros: both members carry the model's default
        // 0, so there is nothing to withhold.
        assert_eq!(
            scheduler_status(&emr, label, &cluster).await,
            (0, 0),
            "case {label:?}: a cluster with no job run must report zero in-queue and \
             zero concurrent runs"
        );

        let mut runs = Vec::new();
        for run in 0..case.runs {
            runs.push(run_id(&emr, label, &cluster, &format!("tok-status-{index}-{run}")).await);
        }

        // Phase one: a submitted run is accepted PENDING, so every run is in the queue
        // and none is running yet.
        let queued = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            queued,
            (case.runs, 0),
            "case {label:?}: {} freshly submitted runs must all be in queue, got {queued:?}",
            case.runs
        );

        // Phase two: the clock has made each start due, and no row's concurrency limit is
        // in the way, so the queue is empty and every run is running.
        wait_for_the_clock();
        let running = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            running,
            (0, case.runs),
            "case {label:?}: once the clock has started them, {} runs must be concurrent, \
             got {running:?}",
            case.runs
        );

        // A read must not change what the next reader sees.
        let again = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            again, running,
            "case {label:?}: reading the cluster twice reported two different scheduler \
             statuses, {running:?} then {again:?}"
        );

        // A run that has left both sets counts in neither.
        if let Some(first) = runs.first() {
            cancel_run(&emr, &cluster, first).await.ok(label);
            wait_for_the_clock();
            let cancelled_state = run_state(&emr, label, &cluster, first).await;
            assert_eq!(
                cancelled_state, "CANCELLED",
                "case {label:?}: the cancelled run must have finished, got {cancelled_state:?}"
            );
            let after_cancel = scheduler_status(&emr, label, &cluster).await;
            assert_eq!(
                after_cancel,
                (0, case.runs - 1),
                "case {label:?}: a cancelled run counts neither in queue nor concurrent, \
                 got {after_cancel:?}"
            );
        }
    }
}

/// RULE 2: `maxConcurrentJobRuns` caps the runs that may be `RUNNING` at once, the runs
/// over the cap wait in the queue, and a slot that frees is taken by the oldest waiting
/// run.
///
/// The rows differ in the limit (its published minimum, and values above it) and so in how
/// many of the submitted runs have to wait.
#[tokio::test]
async fn the_concurrency_limit_caps_the_running_runs_and_queues_the_rest() {
    let emr = Emr::new();
    // `JobLimitInteger`'s @range is 1..1000, so 1 is the published minimum.
    for limit in [1_i64, 2, 3] {
        let label = format!("maxConcurrentJobRuns {limit}");
        let submitted = limit + 2;
        let cluster = cluster_id(
            &emr,
            &label,
            &format!("concurrency-{limit}"),
            &format!("tok-concurrency-{limit}"),
            Some(json!({"maxConcurrentJobRuns": limit})),
        )
        .await;

        let mut runs = Vec::new();
        for run in 0..submitted {
            runs.push(
                run_id(
                    &emr,
                    &label,
                    &cluster,
                    &format!("tok-concurrency-{limit}-{run}"),
                )
                .await,
            );
        }

        wait_for_the_clock();
        let (in_queue, concurrent) = scheduler_status(&emr, &label, &cluster).await;
        assert_eq!(
            concurrent, limit,
            "case {label:?}: {submitted} submitted runs may not take more than {limit} \
             running slots, got {concurrent}"
        );
        assert_eq!(
            in_queue,
            submitted - limit,
            "case {label:?}: the runs over the limit must wait in queue, got {in_queue}"
        );

        // The slots went to the runs submitted first: the queue is served in order.
        for (position, run) in runs.iter().enumerate() {
            let expected = if i64::try_from(position).unwrap_or(i64::MAX) < limit {
                "RUNNING"
            } else {
                "PENDING"
            };
            let observed = run_state(&emr, &label, &cluster, run).await;
            assert_eq!(
                observed, expected,
                "case {label:?}: run {position} of {submitted} must be {expected}, got \
                 {observed:?}"
            );
        }

        // A freed slot is taken by the oldest run still waiting.
        cancel_run(&emr, &cluster, &runs[0]).await.ok(&label);
        wait_for_the_clock();
        let (in_queue, concurrent) = scheduler_status(&emr, &label, &cluster).await;
        assert_eq!(
            concurrent, limit,
            "case {label:?}: a freed running slot must be refilled from the queue, got \
             {concurrent}"
        );
        assert_eq!(
            in_queue,
            submitted - limit - 1,
            "case {label:?}: one waiting run must have left the queue, got {in_queue}"
        );
        let promoted = usize::try_from(limit).expect("a small limit fits a usize");
        let observed = run_state(&emr, &label, &cluster, &runs[promoted]).await;
        assert_eq!(
            observed, "RUNNING",
            "case {label:?}: the oldest waiting run must take the freed slot, got \
             {observed:?}"
        );
    }
}

/// RULE 3: a full queue refuses the next submission with `ValidationException`, reports
/// the depth it refused at, and takes work again as soon as the queue has room.
///
/// The depth driven to here is a value inside the published range
/// (`InQueueJobLimitInteger` @range 1..1000), and the cluster's single running slot is
/// spent on a run that is already `RUNNING`, so the queue cannot drain while it fills and
/// the depth the refusal happens at is exact.
#[tokio::test]
async fn a_full_queue_refuses_the_next_job_run() {
    let emr = Emr::new();
    let label = "a queue of 100 with its one running slot taken";
    let limit = 100_i64;
    let cluster = cluster_id(
        &emr,
        label,
        "queue-limit",
        "tok-queue-limit",
        Some(json!({"maxInQueueJobRuns": limit, "maxConcurrentJobRuns": 1})),
    )
    .await;

    let running = run_id(&emr, label, &cluster, "tok-queue-running").await;
    wait_for_the_clock();
    assert_eq!(
        scheduler_status(&emr, label, &cluster).await,
        (0, 1),
        "case {label:?}: the first run must hold the single running slot"
    );

    let mut queued = Vec::new();
    for position in 0..limit {
        queued.push(run_id(&emr, label, &cluster, &format!("tok-queue-{position}")).await);
    }
    assert_eq!(
        scheduler_status(&emr, label, &cluster).await,
        (limit, 1),
        "case {label:?}: the queue must hold every run that could not start"
    );

    start_run(&emr, &cluster, "tok-queue-overflow")
        .await
        .refused(label, 400, "ValidationException");
    assert_eq!(
        scheduler_status(&emr, label, &cluster).await,
        (limit, 1),
        "case {label:?}: a refused submission must not enter the queue"
    );

    // Room in the queue is room for work again.
    cancel_run(&emr, &cluster, &queued[0]).await.ok(label);
    wait_for_the_clock();
    assert_eq!(
        scheduler_status(&emr, label, &cluster).await,
        (limit - 1, 1),
        "case {label:?}: cancelling a waiting run must free queue room"
    );
    let accepted = run_id(&emr, label, &cluster, "tok-queue-after-room").await;
    assert_eq!(
        scheduler_status(&emr, label, &cluster).await,
        (limit, 1),
        "case {label:?}: the accepted submission must be the queue's new tail"
    );
    let observed = run_state(&emr, label, &cluster, &accepted).await;
    assert_eq!(
        observed, "PENDING",
        "case {label:?}: the accepted submission must wait, the slot is taken, got \
         {observed:?}"
    );
    let observed = run_state(&emr, label, &cluster, &running).await;
    assert_eq!(
        observed, "RUNNING",
        "case {label:?}: the run holding the slot must keep it, got {observed:?}"
    );
}

/// RULE 4: each limit is accepted at both published bounds and refused one past either.
///
/// Source: `smithy_query(query="shape", shape="SchedulerConfiguration")` —
/// `maxInQueueJobRuns` targets `InQueueJobLimitInteger` (@range 1..1000) and
/// `maxConcurrentJobRuns` targets `JobLimitInteger` (@range 1..1000).
#[tokio::test]
async fn each_scheduler_limit_is_honoured_at_its_published_bounds() {
    let emr = Emr::new();
    let cases: [(&str, Value, bool); 8] = [
        (
            "the queue-depth minimum",
            json!({"maxInQueueJobRuns": 1}),
            true,
        ),
        (
            "the queue-depth maximum",
            json!({"maxInQueueJobRuns": 1000}),
            true,
        ),
        (
            "one below the queue-depth minimum",
            json!({"maxInQueueJobRuns": 0}),
            false,
        ),
        (
            "one above the queue-depth maximum",
            json!({"maxInQueueJobRuns": 1001}),
            false,
        ),
        (
            "the concurrency minimum",
            json!({"maxConcurrentJobRuns": 1}),
            true,
        ),
        (
            "the concurrency maximum",
            json!({"maxConcurrentJobRuns": 1000}),
            true,
        ),
        (
            "one below the concurrency minimum",
            json!({"maxConcurrentJobRuns": 0}),
            false,
        ),
        (
            "one above the concurrency maximum",
            json!({"maxConcurrentJobRuns": 1001}),
            false,
        ),
    ];

    for (index, (label, scheduler, accepted)) in cases.iter().enumerate() {
        let answer = create_cluster(
            &emr,
            &format!("bounds-{index}"),
            &format!("tok-bounds-{index}"),
            Some(scheduler.clone()),
        )
        .await;
        if !accepted {
            assert_eq!(
                answer.status, 400,
                "case {label:?}: a value past the bound must be refused with HTTP 400, \
                 got {} with body {}",
                answer.status, answer.body
            );
            continue;
        }
        let id = text(answer.ok(label), "/id", label);
        let described = emr
            .send("GET", &format!("/virtualclusters/{id}"), None)
            .await;
        let body = described.ok(label).clone();
        let cluster = field(&body, "/virtualCluster", label);
        assert_eq!(
            cluster.get("schedulerConfiguration").cloned(),
            Some(scheduler.clone()),
            "case {label:?}: the accepted limit must round-trip through state, got \
             {cluster}"
        );
        assert_eq!(
            scheduler_status(&emr, label, &id).await,
            (0, 0),
            "case {label:?}: a cluster holding no run reports zeros whatever its limits"
        );
    }
}
