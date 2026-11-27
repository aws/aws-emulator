// SPDX-License-Identifier: CC0-1.0

//! The queue-depth limit a virtual cluster enforces is the one it CURRENTLY carries.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and the endpoints are driven through the
//! helpers of [`crate::virtual_cluster_scheduler_status`] and
//! [`crate::virtual_cluster_update`], so there is one definition of each request rather
//! than a second copy here.
//!
//! The rule under test — the one its sibling suites leave to this one, because each of
//! them drives a single queue-depth limit, installed by the create, at its published
//! minimum:
//!
//! `schedulerConfiguration.maxInQueueJobRuns` is "the maximum number of job runs that can
//! be in the `PENDING` or `SUBMITTED` state at any time for the virtual cluster. When the
//! queue is full, the service rejects `StartJobRun` requests with a
//! `ValidationException`" — enforced at the depth the cluster carries WHEN the submission
//! arrives, whichever call installed it and whatever its value inside the published
//! range, and taking work again as soon as the queue has room.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{field, Emr};
use crate::virtual_cluster_scheduler_status::{
    cancel_run, cluster_id, run_id, run_state, scheduler_status, start_run, wait_for_the_clock,
};
use crate::virtual_cluster_update::update_cluster;

/// One row: how this cluster's queue-depth limit got there, and the depth it then
/// enforces.
///
/// Every row configures `maxConcurrentJobRuns: 1` and spends that single running slot
/// before filling the queue, so no run can leave the queue while it fills and the depth
/// the refusal happens at is exact.
struct QueueDepthCase {
    label: &'static str,
    /// The `schedulerConfiguration` the cluster is created with.
    created_with: Value,
    /// The `schedulerConfiguration` a later `UpdateVirtualCluster` installs, if any. It
    /// fully replaces the created one.
    updated_to: Option<Value>,
    /// How many runs are queued BEFORE that update, i.e. under the created limit.
    queued_before_update: i64,
    /// The depth the cluster enforces once the row's calls have all been made.
    limit: i64,
}

/// How many runs this row's cluster has queued so far, as the row counts them.
fn depth(queued: &[String]) -> i64 {
    i64::try_from(queued.len()).unwrap_or(i64::MAX)
}

/// The queue-depth limit in force is the cluster's current one.
///
/// The rows differ along what the rule generalises over: which call installed the limit
/// (the create, an update that lowers it onto a queue that is already that deep, an update
/// that raises it above the depth it was refusing at, an update that adds one where the
/// cluster had none) and its value (its published minimum, and a value inside the range
/// that no other suite drives) — so a limit read once at creation time, or a refusal
/// keyed to one depth, cannot satisfy any of them.
///
/// Limit values come from the published range
/// (`smithy_query(query="shape", shape="SchedulerConfiguration")`: `maxInQueueJobRuns`
/// targets `InQueueJobLimitInteger` @range 1..1000); the bounds themselves, and the
/// values past them, are [`crate::virtual_cluster_scheduler_status`]'s rows.
#[tokio::test]
async fn the_enforced_queue_depth_is_the_clusters_current_limit() {
    let emr = Emr::new();
    let cases = [
        QueueDepthCase {
            label: "the limit the create installed",
            created_with: json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 1}),
            updated_to: None,
            queued_before_update: 100,
            limit: 100,
        },
        QueueDepthCase {
            label: "a limit lowered onto a queue that is already that deep",
            created_with: json!({"maxInQueueJobRuns": 1000, "maxConcurrentJobRuns": 1}),
            updated_to: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 1})),
            queued_before_update: 100,
            limit: 100,
        },
        QueueDepthCase {
            label: "a limit raised above the depth it was refusing at",
            created_with: json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 1}),
            updated_to: Some(json!({"maxInQueueJobRuns": 150, "maxConcurrentJobRuns": 1})),
            queued_before_update: 100,
            limit: 150,
        },
        QueueDepthCase {
            label: "a limit added to a cluster created with none",
            created_with: json!({"maxConcurrentJobRuns": 1}),
            updated_to: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 1})),
            queued_before_update: 100,
            limit: 100,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let cluster = cluster_id(
            &emr,
            label,
            &format!("queue-depth-{index}"),
            &format!("tok-qd-{index}"),
            Some(case.created_with.clone()),
        )
        .await;

        // Spend the single running slot first, so nothing drains the queue below.
        let running = run_id(&emr, label, &cluster, &format!("tok-qd-{index}-slot")).await;
        wait_for_the_clock();
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (0, 1),
            "case {label:?}: the first run must hold the single running slot, got \
             {observed:?}"
        );

        // Fill the queue under the limit the create installed. Every submission below the
        // limit is accepted: `run_id` fails the case on anything but a 200.
        let mut queued: Vec<String> = Vec::new();
        while depth(&queued) < case.queued_before_update {
            let token = format!("tok-qd-{index}-pre-{}", depth(&queued));
            queued.push(run_id(&emr, label, &cluster, &token).await);
        }
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (case.queued_before_update, 1),
            "case {label:?}: the queue must hold every run that could not start, got \
             {observed:?}"
        );

        // A row whose CREATED limit is already taken is refused at that depth, before any
        // update moves it: the limit in force is the one the cluster carries now.
        let created_limit = case
            .created_with
            .get("maxInQueueJobRuns")
            .and_then(Value::as_i64);
        if created_limit.is_some_and(|limit| case.queued_before_update >= limit) {
            let before = format!("{label}, before the update");
            start_run(&emr, &cluster, &format!("tok-qd-{index}-pre-overflow"))
                .await
                .refused(&before, 400, "ValidationException");
        }

        if let Some(updated_to) = &case.updated_to {
            let answer =
                update_cluster(&emr, &cluster, &format!("tok-qd-{index}-patch"), Some(updated_to.clone()))
                    .await;
            let after = field(answer.ok(label), "/virtualCluster", label).clone();
            assert_eq!(
                after.get("schedulerConfiguration").cloned(),
                Some(updated_to.clone()),
                "case {label:?}: the update must install the row's limit, got {after}"
            );
            // Replacing the limit evicts nothing: the queue is as deep as it was.
            let observed = scheduler_status(&emr, label, &cluster).await;
            assert_eq!(
                observed,
                (case.queued_before_update, 1),
                "case {label:?}: a replaced limit must not evict the runs already queued, \
                 got {observed:?}"
            );
        }

        // Fill the rest of the way to the limit now in force. On the row that raised it,
        // these are the submissions the previous limit was refusing.
        while depth(&queued) < case.limit {
            let token = format!("tok-qd-{index}-post-{}", depth(&queued));
            queued.push(run_id(&emr, label, &cluster, &token).await);
        }
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (case.limit, 1),
            "case {label:?}: the queue must fill to the limit now in force, got {observed:?}"
        );

        // The queue is full at the current limit: the next submission is refused, and the
        // refusal leaves the queue as it was.
        start_run(&emr, &cluster, &format!("tok-qd-{index}-overflow"))
            .await
            .refused(label, 400, "ValidationException");
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (case.limit, 1),
            "case {label:?}: a refused submission must not enter the queue, got {observed:?}"
        );

        // Room in the queue is room for work again.
        cancel_run(&emr, &cluster, &queued[0]).await.ok(label);
        wait_for_the_clock();
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (case.limit - 1, 1),
            "case {label:?}: cancelling a waiting run must free queue room, got {observed:?}"
        );
        let accepted = run_id(&emr, label, &cluster, &format!("tok-qd-{index}-after-room")).await;
        let observed = scheduler_status(&emr, label, &cluster).await;
        assert_eq!(
            observed,
            (case.limit, 1),
            "case {label:?}: the accepted submission must be the queue's new tail, got \
             {observed:?}"
        );
        let observed = run_state(&emr, label, &cluster, &accepted).await;
        assert_eq!(
            observed, "PENDING",
            "case {label:?}: the accepted submission must wait, the running slot is taken, \
             got {observed:?}"
        );
        let observed = run_state(&emr, label, &cluster, &running).await;
        assert_eq!(
            observed, "RUNNING",
            "case {label:?}: the run holding the slot must keep it, got {observed:?}"
        );
    }
}
