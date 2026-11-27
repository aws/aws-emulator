// SPDX-License-Identifier: CC0-1.0

//! The concurrency a virtual cluster runs under when it configures none.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and the endpoints are driven through the
//! helpers of [`crate::virtual_cluster_scheduler_status`], so there is one definition of
//! each request rather than a second copy here.
//!
//! The rule under test, asserted over a TABLE of clusters rather than one request: the
//! number of a cluster's job runs that may be `RUNNING` at once is the
//! `schedulerConfiguration.maxConcurrentJobRuns` it configures, or — when it configures
//! none — the default the service applies to a cluster that omits it
//! (`SchedulerConfiguration$maxConcurrentJobRuns`: "if you omit this field, the service
//! applies a default limit of 1,000 concurrent job runs to the virtual cluster"; the shape
//! itself: "if you omit this configuration, the service applies a default limit of 1,000
//! concurrent job runs and does not limit queue depth" — smithy_query(query="shape",
//! shape="SchedulerConfiguration")). A submission over that ceiling waits in the queue, and
//! the ceiling is the same whether the cluster reached it by configuring the published
//! maximum or by configuring nothing at all.
//!
//! What no REQUEST can stage is a cluster sitting AT its ceiling with nothing draining it:
//! a run this service admits owes its own outcome ([`state::JOB_OUTCOME_SPAN`]), so filling
//! a ceiling of a thousand through the API would have the first runs finishing — and
//! freeing slots — while the last ones were still being submitted. So each row's ceiling is
//! held steady in the store the handlers themselves read: one run is admitted through the
//! API, and the rest of the ceiling is that very record, held `RUNNING` with no outcome
//! owed. Everything else is a request.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::Emr;
use crate::virtual_cluster_scheduler_status::{
    cancel_run, cluster_id, run_id, run_state, scheduler_status, start_run,
};

/// One row: the `schedulerConfiguration` the cluster is created with, and how many of its
/// job runs may be `RUNNING` at once as a result.
struct CeilingCase {
    label: &'static str,
    scheduler: Option<Value>,
    ceiling: i64,
}

/// The documented default concurrency of a cluster that configures none, from
/// `SchedulerConfiguration$maxConcurrentJobRuns`'s own documentation (quoted in this
/// module's docs). It is also the top of the member's published `@range` (1..1000), which
/// is why the last row below — a cluster that configures that maximum — must behave exactly
/// like the rows that configure nothing.
const DOCUMENTED_DEFAULT: i64 = 1_000;

/// The rows differ along everything the rule generalises over: whether the cluster carries
/// a `schedulerConfiguration` at all, whether the one it carries mentions concurrency,
/// which limit values it configures (the published minimum, a value inside the range, the
/// published maximum), and so whether its ceiling comes from the request or from the
/// default.
fn rows() -> Vec<CeilingCase> {
    vec![
        CeilingCase {
            label: "no schedulerConfiguration at all",
            scheduler: None,
            ceiling: DOCUMENTED_DEFAULT,
        },
        CeilingCase {
            label: "a configuration that mentions only the queue depth",
            scheduler: Some(json!({"maxInQueueJobRuns": 5})),
            ceiling: DOCUMENTED_DEFAULT,
        },
        CeilingCase {
            label: "a configuration with no member at all",
            scheduler: Some(json!({})),
            ceiling: DOCUMENTED_DEFAULT,
        },
        CeilingCase {
            label: "the published minimum concurrency",
            scheduler: Some(json!({"maxConcurrentJobRuns": 1})),
            ceiling: 1,
        },
        CeilingCase {
            label: "a concurrency inside the range",
            scheduler: Some(json!({"maxConcurrentJobRuns": 4})),
            ceiling: 4,
        },
        CeilingCase {
            label: "the published maximum concurrency",
            scheduler: Some(json!({"maxConcurrentJobRuns": 1000})),
            ceiling: DOCUMENTED_DEFAULT,
        },
    ]
}

/// How long a case waits for a transition the service owes before it fails. Deliberately
/// far longer than the spans involved: this bounds a hang, it does not time anything.
fn deadline() -> Instant {
    Instant::now() + state::JOB_OUTCOME_SPAN + Duration::from_secs(60)
}

/// Poll `DescribeJobRun` until this run has left `PENDING`, and answer the state it left
/// for. Fails the case if it never does.
///
/// A condition, not a duration: the service stores the instant a transition is due and
/// materialises it on the next call, so what a case waits for is the state it reads back.
async fn admitted_state(emr: &Emr, case: &str, cluster: &str, run: &str) -> String {
    let deadline = deadline();
    loop {
        let observed = run_state(emr, case, cluster, run).await;
        if observed != state::JOB_PENDING {
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run was never admitted, it is still {:?}",
            state::JOB_PENDING
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Hold `cluster` at exactly `ceiling` `RUNNING` job runs, and answer the ids holding it.
///
/// The first slot is taken by a run submitted and admitted through the API; the rest are
/// that record, held `RUNNING` with no outcome owed, so the ceiling does not drain while
/// the case is asserting against it.
async fn hold_at_ceiling(emr: &Emr, case: &str, cluster: &str, ceiling: i64) -> Vec<String> {
    let first = run_id(emr, case, cluster, "tok-ceiling-first").await;
    let observed = admitted_state(emr, case, cluster, &first).await;
    assert_eq!(
        observed,
        state::JOB_RUNNING,
        "case {case:?}: the first submission must be admitted to the cluster, got \
         {observed:?}"
    );

    let store = emr.store();
    let mut held = vec![first.clone()];
    let mut state_guard = store.lock();
    let template = state_guard
        .job_runs
        .get(&first)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the admitted run is in the store"));
    // Nothing is owed on the runs that hold the ceiling, including the real one: an
    // outcome falling due would free a slot in the middle of the assertions below.
    state_guard.pending_job_runs.remove(&first);
    if let Some(record) = state_guard.job_runs.get_mut(&first) {
        record.pending = None;
    }
    for index in 1..ceiling {
        let mut record = template.clone();
        record.id = format!("held{index}");
        record.arn = template.arn.replace(&template.id, &record.id);
        record.client_token = format!("tok-held-{index}");
        record.state = state::JOB_RUNNING.to_string();
        record.pending = None;
        record.sequence = template.sequence + u64::try_from(index).unwrap_or_default();
        state_guard
            .job_runs_by_cluster
            .entry(cluster.to_string())
            .or_default()
            .push(record.id.clone());
        held.push(record.id.clone());
        state_guard.job_runs.insert(record.id.clone(), record);
    }
    drop(state_guard);
    held
}

/// The ceiling a cluster's job runs are admitted up to is the concurrency it configures,
/// or the documented default when it configures none — and a submission over the ceiling
/// waits until a slot frees.
#[tokio::test]
async fn a_cluster_admits_runs_up_to_the_concurrency_it_runs_under() {
    for (index, case) in rows().into_iter().enumerate() {
        // One emulator per row, so a row's thousand held runs are not in another row's
        // store.
        let emr = Emr::new();
        let label = case.label;
        let cluster = cluster_id(
            &emr,
            label,
            &format!("ceiling-{index}"),
            &format!("tok-ceiling-{index}"),
            case.scheduler.clone(),
        )
        .await;

        let held = hold_at_ceiling(&emr, label, &cluster, case.ceiling).await;
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (0, case.ceiling),
            "case {label:?}: the cluster must be at its ceiling of {} running runs, got \
             {observed:?}",
            case.ceiling
        );

        // One more submission: accepted, and it cannot start — every slot the cluster runs
        // under is taken.
        let over = run_id(&emr, label, &cluster, "tok-over-the-ceiling").await;

        // The clock has made that start due once a run submitted AFTER it, on a cluster
        // with room, has been admitted: a condition the service answers for, so nothing
        // here waits on a duration.
        let control_cluster = cluster_id(
            &emr,
            label,
            &format!("ceiling-control-{index}"),
            &format!("tok-ceiling-control-{index}"),
            Some(json!({"maxConcurrentJobRuns": 1})),
        )
        .await;
        let control = run_id(&emr, label, &control_cluster, "tok-control").await;
        let observed = admitted_state(&emr, label, &control_cluster, &control).await;
        assert_eq!(
            observed,
            state::JOB_RUNNING,
            "case {label:?}: the control run must be admitted to the cluster that had room, \
             got {observed:?}"
        );

        let observed = run_state(&emr, label, &cluster, &over).await;
        assert_eq!(
            observed,
            state::JOB_PENDING,
            "case {label:?}: a submission over the ceiling of {} must wait, got {observed:?}",
            case.ceiling
        );
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (1, case.ceiling),
            "case {label:?}: the waiting run must be in the queue and the ceiling \
             unchanged, got {observed:?}"
        );

        // A freed slot is taken: cancelling one of the runs holding the ceiling admits the
        // one that was waiting ("as running slots free up, queued job runs start
        // automatically").
        cancel_run(&emr, &cluster, &held[0]).await.ok(label);
        let observed = admitted_state(&emr, label, &cluster, &over).await;
        assert!(
            observed == state::JOB_RUNNING
                || state::JOB_TERMINAL_STATES.contains(&observed.as_str()),
            "case {label:?}: the freed slot must admit the waiting run, got {observed:?}"
        );
    }
}

/// A cluster that configures no concurrency is not unbounded: its ceiling is the default,
/// and the queue is what holds everything over it — including a submission that arrives
/// while the queue already has one waiting.
#[tokio::test]
async fn a_cluster_that_configures_no_concurrency_is_not_unbounded() {
    let emr = Emr::new();
    let label = "a cluster at the default ceiling";
    let cluster = cluster_id(&emr, label, "default-ceiling", "tok-default-ceiling", None).await;
    let held = hold_at_ceiling(&emr, label, &cluster, DOCUMENTED_DEFAULT).await;
    assert_eq!(
        held.len(),
        usize::try_from(DOCUMENTED_DEFAULT).unwrap_or_default(),
        "case {label:?}: the default ceiling must be held by {DOCUMENTED_DEFAULT} runs, got \
         {} of them",
        held.len()
    );

    // Three more submissions, all accepted and all waiting: a cluster that configures no
    // limit still admits no more than its ceiling.
    let mut waiting = Vec::new();
    for position in 0..3 {
        waiting.push(
            run_id(
                &emr,
                label,
                &cluster,
                &format!("tok-default-over-{position}"),
            )
            .await,
        );
    }
    let control_cluster = cluster_id(
        &emr,
        label,
        "default-ceiling-control",
        "tok-default-control",
        Some(json!({"maxConcurrentJobRuns": 1})),
    )
    .await;
    let control = run_id(&emr, label, &control_cluster, "tok-default-control-run").await;
    let observed = admitted_state(&emr, label, &control_cluster, &control).await;
    assert_eq!(
        observed,
        state::JOB_RUNNING,
        "case {label:?}: the control run must be admitted to the cluster that had room, got \
         {observed:?}"
    );

    for (position, run) in waiting.iter().enumerate() {
        let observed = run_state(&emr, label, &cluster, run).await;
        assert_eq!(
            observed,
            state::JOB_PENDING,
            "case {label:?}: submission {position} over the default ceiling must wait, got \
             {observed:?}"
        );
    }
    let observed = scheduler_status(&emr, label, &cluster).await;
    assert_eq!(
        observed,
        (3, DOCUMENTED_DEFAULT),
        "case {label:?}: the three runs over the ceiling must be queued behind it, got \
         {observed:?}"
    );

    // The queue is what they wait in, so a refusal is not what a cluster at its ceiling
    // answers a submission with.
    let answer = start_run(&emr, &cluster, "tok-default-one-more").await;
    assert_eq!(
        answer.status,
        200,
        "case {label:?}: a cluster at its ceiling queues the submission rather than \
         refusing it, got {} with body {}",
        answer.status,
        answer.body
    );
}
