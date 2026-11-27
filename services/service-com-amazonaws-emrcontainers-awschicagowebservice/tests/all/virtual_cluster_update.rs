// SPDX-License-Identifier: CC0-1.0

//! `UpdateVirtualCluster`, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests
//! (`PATCH /virtualclusters/{id}`), so routing, deserialization, constraint validation,
//! the handler and serialization all run exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The submitted `schedulerConfiguration` FULLY REPLACES the stored one: "the new
//!    configuration fully replaces the existing one. If you omit a field, the
//!    corresponding limit is removed" — so a request that carries no configuration at all
//!    leaves the cluster with none, and a cluster with no limit configured reports no
//!    `schedulerConfiguration` member ("the service does not return this field when no
//!    scheduler limits are configured").
//! 2. The update is PERSISTED and nothing else about the cluster moves: a later
//!    `DescribeVirtualCluster` answers exactly what the update answered, and the
//!    cluster's identity and creation members are untouched.
//! 3. `clientToken` is an `@idempotencyToken`: "if this token matches a previous request,
//!    the service ignores the request, but does not return an error", so a replayed token
//!    leaves the cluster as the first call left it while a fresh one applies.
//! 4. The new `maxConcurrentJobRuns` is the limit the scheduler then enforces: raising it
//!    (or removing it) frees running slots and "as running slots free up, queued job runs
//!    start automatically"; lowering it does not evict the runs already running.
//! 5. A request naming no stored cluster, and one naming a cluster whose teardown has
//!    started, are refused with the modelled errors; each limit's published `@range` is
//!    honoured at both bounds and refused one past either.

use serde_json::{json, Value};

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::{
    cluster_id, run_id, run_state, scheduler_status, wait_for_the_clock,
};

/// `PATCH /virtualclusters/{id}`, carrying `scheduler` as the `schedulerConfiguration`
/// when the row has one. A row with none omits the member entirely, which is a different
/// request from an empty structure on the wire and (by RULE 1) the same statement.
///
/// `pub(crate)` because [`crate::virtual_cluster_queue_depth`] installs a limit through
/// this same endpoint: one definition of the request, so no two suites can disagree about
/// what an update looks like.
pub(crate) async fn update_cluster(
    emr: &Emr,
    id: &str,
    token: &str,
    scheduler: Option<Value>,
) -> Answer {
    let mut body = json!({"clientToken": token});
    if let (Some(target), Some(scheduler)) = (body.as_object_mut(), scheduler) {
        target.insert("schedulerConfiguration".to_string(), scheduler);
    }
    emr.send("PATCH", &format!("/virtualclusters/{id}"), Some(body))
        .await
}

/// The `virtualCluster` an update must have answered with.
fn updated(answer: &Answer, case: &str) -> Value {
    field(answer.ok(case), "/virtualCluster", case).clone()
}

/// The `virtualCluster` `DescribeVirtualCluster` reports for this cluster.
async fn described(emr: &Emr, case: &str, id: &str) -> Value {
    let answer = emr.send("GET", &format!("/virtualclusters/{id}"), None).await;
    field(answer.ok(case), "/virtualCluster", case).clone()
}

/// The `schedulerConfiguration` member of a reported cluster, absent when the cluster
/// configures no limit.
fn configuration_of(cluster: &Value) -> Option<Value> {
    cluster.get("schedulerConfiguration").cloned()
}

/// The `(currentInQueueJobRuns, currentConcurrentJobRuns)` a reported cluster carries.
fn status_of(cluster: &Value, case: &str) -> (i64, i64) {
    let status = field(cluster, "/schedulerStatus", case).clone();
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

/// One row of RULES 1 and 2: what the cluster was created with, what the update submits,
/// and the configuration the cluster must carry afterwards.
struct ReplacementCase {
    label: &'static str,
    created_with: Option<Value>,
    submitted: Option<Value>,
    expected: Option<Value>,
}

/// RULES 1 and 2: the submitted configuration replaces the stored one wholesale, the
/// result is persisted, and nothing else about the cluster moves.
///
/// The rows differ along what "fully replaces" generalises over: which limits the cluster
/// started with, which the update submits, whether the update keeps both, swaps one for
/// the other, drops one, drops both (with an empty structure and by omitting the member),
/// or changes nothing. Limit VALUES come from the two published ranges
/// (`smithy_query(query="shape", shape="SchedulerConfiguration")`: `maxInQueueJobRuns`
/// targets `InQueueJobLimitInteger` @range 1..1000, `maxConcurrentJobRuns` targets
/// `JobLimitInteger` @range 1..1000).
#[tokio::test]
async fn the_submitted_scheduler_configuration_replaces_the_stored_one() {
    let emr = Emr::new();
    let cases = [
        ReplacementCase {
            label: "no limits, then both",
            created_with: None,
            submitted: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
            expected: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
        },
        ReplacementCase {
            label: "both limits, then only the queue depth",
            created_with: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
            submitted: Some(json!({"maxInQueueJobRuns": 250})),
            expected: Some(json!({"maxInQueueJobRuns": 250})),
        },
        ReplacementCase {
            label: "both limits, then only the concurrency",
            created_with: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
            submitted: Some(json!({"maxConcurrentJobRuns": 7})),
            expected: Some(json!({"maxConcurrentJobRuns": 7})),
        },
        ReplacementCase {
            label: "only the concurrency, then only the queue depth",
            created_with: Some(json!({"maxConcurrentJobRuns": 3})),
            submitted: Some(json!({"maxInQueueJobRuns": 1000})),
            expected: Some(json!({"maxInQueueJobRuns": 1000})),
        },
        ReplacementCase {
            label: "both limits, then both at their published maxima",
            created_with: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 1})),
            submitted: Some(json!({"maxInQueueJobRuns": 1000, "maxConcurrentJobRuns": 1000})),
            expected: Some(json!({"maxInQueueJobRuns": 1000, "maxConcurrentJobRuns": 1000})),
        },
        ReplacementCase {
            label: "both limits, then an empty configuration removes both",
            created_with: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
            submitted: Some(json!({})),
            expected: None,
        },
        ReplacementCase {
            label: "both limits, then no configuration at all removes both",
            created_with: Some(json!({"maxInQueueJobRuns": 100, "maxConcurrentJobRuns": 5})),
            submitted: None,
            expected: None,
        },
        ReplacementCase {
            label: "no limits, then still no limits",
            created_with: None,
            submitted: None,
            expected: None,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let id = cluster_id(
            &emr,
            label,
            &format!("update-{index}"),
            &format!("tok-update-{index}"),
            case.created_with.clone(),
        )
        .await;
        let before = described(&emr, label, &id).await;

        let answer = update_cluster(&emr, &id, &format!("tok-patch-{index}"), case.submitted.clone())
            .await;
        let after = updated(&answer, label);

        // RULE 1: exactly the submitted limits, and no member at all when none is left.
        assert_eq!(
            configuration_of(&after),
            case.expected,
            "case {label:?}: the update must leave exactly the submitted limits, got {after}"
        );

        // RULE 2: persisted — a later read answers precisely what the update answered.
        let read_back = described(&emr, label, &id).await;
        assert_eq!(
            read_back, after,
            "case {label:?}: DescribeVirtualCluster must answer what the update answered, \
             got {read_back} after {after}"
        );

        // RULE 2: the update touches the configuration and nothing else.
        for member in [
            "id",
            "name",
            "arn",
            "createdAt",
            "containerProvider",
            "state",
            "tags",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {label:?}: the update must not change {member}, it went from {:?} to {:?}",
                before.get(member),
                after.get(member)
            );
        }
        assert_eq!(
            text(&after, "/state", label),
            "RUNNING",
            "case {label:?}: an updated cluster stays RUNNING, got {after}"
        );
        assert_eq!(
            status_of(&after, label),
            (0, 0),
            "case {label:?}: a cluster holding no job run reports zero counts whatever its \
             limits, got {after}"
        );
    }
}

/// RULE 3: a replayed `clientToken` is ignored, a fresh one applies.
///
/// The rows are the submissions a retry can differ in: the same configuration again (the
/// plain retry), a different one under the SAME token (which must change nothing), and a
/// different one under a NEW token (which must apply).
#[tokio::test]
async fn a_replayed_client_token_leaves_the_cluster_as_the_first_update_left_it() {
    let emr = Emr::new();
    let label = "an update retried under one token";
    let id = cluster_id(
        &emr,
        label,
        "idempotent-update",
        "tok-idempotent-create",
        Some(json!({"maxInQueueJobRuns": 100})),
    )
    .await;

    let first = json!({"maxConcurrentJobRuns": 9});
    let applied = updated(
        &update_cluster(&emr, &id, "tok-first", Some(first.clone())).await,
        label,
    );
    assert_eq!(
        configuration_of(&applied),
        Some(first.clone()),
        "case {label:?}: the first update must apply, got {applied}"
    );

    // The same token, whatever the body carries: ignored, and no error.
    for (case, submitted) in [
        ("the same token and the same configuration", Some(first.clone())),
        (
            "the same token and a different configuration",
            Some(json!({"maxInQueueJobRuns": 1000})),
        ),
        ("the same token and no configuration", None),
    ] {
        let replayed = updated(&update_cluster(&emr, &id, "tok-first", submitted).await, case);
        assert_eq!(
            configuration_of(&replayed),
            Some(first.clone()),
            "case {case:?}: a replayed token must leave the first update's configuration \
             in place, got {replayed}"
        );
        let read_back = described(&emr, case, &id).await;
        assert_eq!(
            configuration_of(&read_back),
            Some(first.clone()),
            "case {case:?}: a replayed token must persist nothing, got {read_back}"
        );
    }

    // A fresh token is a new request, so it applies.
    let second = json!({"maxInQueueJobRuns": 1000, "maxConcurrentJobRuns": 2});
    let case = "a fresh token";
    let applied = updated(
        &update_cluster(&emr, &id, "tok-second", Some(second.clone())).await,
        case,
    );
    assert_eq!(
        configuration_of(&applied),
        Some(second.clone()),
        "case {case:?}: a token never seen before must apply, got {applied}"
    );

    // And the first token stays spent: replaying it does not undo the second update.
    let case = "the first token replayed after a later update";
    let replayed = updated(
        &update_cluster(&emr, &id, "tok-first", Some(json!({"maxConcurrentJobRuns": 1}))).await,
        case,
    );
    assert_eq!(
        configuration_of(&replayed),
        Some(second),
        "case {case:?}: a spent token must change nothing, got {replayed}"
    );
}

/// RULE 4: the concurrency the update installs is the limit the scheduler enforces from
/// then on.
///
/// The steps differ in the direction the limit moves — up, down, and away entirely — and
/// the cluster's job runs are read back through `DescribeJobRun` as well as through the
/// cluster's `schedulerStatus`, so the counts are checked against the runs they count.
#[tokio::test]
async fn the_updated_concurrency_limit_governs_the_clusters_job_runs() {
    let emr = Emr::new();
    let label = "a cluster whose concurrency limit is updated";
    let id = cluster_id(
        &emr,
        label,
        "concurrency-update",
        "tok-concurrency-update",
        Some(json!({"maxConcurrentJobRuns": 1})),
    )
    .await;

    let mut runs = Vec::new();
    for position in 0..3 {
        runs.push(run_id(&emr, label, &id, &format!("tok-cu-run-{position}")).await);
    }
    wait_for_the_clock();
    let case = "one running slot for three runs";
    assert_eq!(
        scheduler_status(&emr, case, &id).await,
        (2, 1),
        "case {case:?}: only one of three runs may hold the single slot"
    );

    // Raising the limit frees slots, and the queued runs take them — the update's own
    // answer already reports the counts it leaves behind.
    let case = "the limit raised to three";
    let after = updated(
        &update_cluster(&emr, &id, "tok-cu-up", Some(json!({"maxConcurrentJobRuns": 3}))).await,
        case,
    );
    assert_eq!(
        status_of(&after, case),
        (0, 3),
        "case {case:?}: the queued runs must take the slots the new limit frees, got {after}"
    );
    assert_eq!(
        scheduler_status(&emr, case, &id).await,
        (0, 3),
        "case {case:?}: the counts the update reported must be the ones a later read sees"
    );
    for (position, run) in runs.iter().enumerate() {
        let observed = run_state(&emr, case, &id, run).await;
        assert_eq!(
            observed, "RUNNING",
            "case {case:?}: run {position} must be RUNNING under the raised limit, got \
             {observed:?}"
        );
    }

    // Lowering it does not evict a run that is already RUNNING; it only governs what may
    // start next.
    let case = "the limit lowered back to one";
    let after = updated(
        &update_cluster(&emr, &id, "tok-cu-down", Some(json!({"maxConcurrentJobRuns": 1}))).await,
        case,
    );
    assert_eq!(
        status_of(&after, case),
        (0, 3),
        "case {case:?}: a lowered limit must not evict the runs already running, got {after}"
    );
    let queued = run_id(&emr, case, &id, "tok-cu-run-queued").await;
    wait_for_the_clock();
    assert_eq!(
        scheduler_status(&emr, case, &id).await,
        (1, 3),
        "case {case:?}: a run submitted under the lowered limit must wait"
    );
    let observed = run_state(&emr, case, &id, &queued).await;
    assert_eq!(
        observed, "PENDING",
        "case {case:?}: the waiting run must still be PENDING, got {observed:?}"
    );

    // Removing the configuration removes the limit, so the waiting run starts.
    let case = "the limit removed entirely";
    let after = updated(&update_cluster(&emr, &id, "tok-cu-none", None).await, case);
    assert_eq!(
        configuration_of(&after),
        None,
        "case {case:?}: the cluster must report no schedulerConfiguration, got {after}"
    );
    assert_eq!(
        status_of(&after, case),
        (0, 4),
        "case {case:?}: with no concurrency limit every run must run, got {after}"
    );
    let observed = run_state(&emr, case, &id, &queued).await;
    assert_eq!(
        observed, "RUNNING",
        "case {case:?}: the run that was waiting must have started, got {observed:?}"
    );
}

/// RULE 5: a request naming no usable cluster is refused with the modelled error, and
/// nothing is persisted for it.
#[tokio::test]
async fn an_update_naming_no_usable_cluster_is_refused() {
    let emr = Emr::new();

    // `ResourceIdString` is `^[0-9a-z]+$`, so this id is well-formed but names nothing:
    // the operation models `ResourceNotFoundException` (HTTP 400) for exactly that.
    update_cluster(
        &emr,
        "doesnotexist1",
        "tok-missing",
        Some(json!({"maxConcurrentJobRuns": 4})),
    )
    .await
    .refused("an id that names no cluster", 400, "ResourceNotFoundException");

    // A cluster whose teardown has started is still describable, so it is not missing —
    // it is the request that is invalid, in both phases of the teardown.
    let label = "a cluster being torn down";
    let id = cluster_id(&emr, label, "terminating", "tok-terminating", None).await;
    emr.send("DELETE", &format!("/virtualclusters/{id}"), None)
        .await
        .ok(label);
    for (case, expected_state) in [
        ("while the teardown is in flight", "TERMINATING"),
        ("once the teardown has settled", "TERMINATED"),
    ] {
        if expected_state == "TERMINATED" {
            wait_for_the_clock();
        }
        let cluster = described(&emr, case, &id).await;
        assert_eq!(
            text(&cluster, "/state", case),
            expected_state,
            "case {case:?}: the cluster must be {expected_state}, got {cluster}"
        );
        update_cluster(
            &emr,
            &id,
            &format!("tok-torn-down-{expected_state}"),
            Some(json!({"maxConcurrentJobRuns": 4})),
        )
        .await
        .refused(case, 400, "ValidationException");
        let unchanged = described(&emr, case, &id).await;
        assert_eq!(
            configuration_of(&unchanged),
            None,
            "case {case:?}: a refused update must persist nothing, got {unchanged}"
        );
    }
}

/// RULE 5: each limit is accepted at both published bounds and refused one past either,
/// through the update as it is through the create.
///
/// Source: `smithy_query(query="shape", shape="SchedulerConfiguration")` —
/// `maxInQueueJobRuns` targets `InQueueJobLimitInteger` (@range 1..1000) and
/// `maxConcurrentJobRuns` targets `JobLimitInteger` (@range 1..1000).
#[tokio::test]
async fn an_update_honours_each_scheduler_limits_published_bounds() {
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

    for (index, (label, submitted, accepted)) in cases.iter().enumerate() {
        // Each row updates its own cluster, created with a limit of its own, so a refused
        // row is checked against a configuration it would have had to overwrite.
        let created_with = json!({"maxInQueueJobRuns": 200});
        let id = cluster_id(
            &emr,
            label,
            &format!("bounds-update-{index}"),
            &format!("tok-bounds-update-{index}"),
            Some(created_with.clone()),
        )
        .await;

        let answer = update_cluster(
            &emr,
            &id,
            &format!("tok-bounds-patch-{index}"),
            Some(submitted.clone()),
        )
        .await;
        if !accepted {
            assert_eq!(
                answer.status, 400,
                "case {label:?}: a value past the bound must be refused with HTTP 400, \
                 got {} with body {}",
                answer.status, answer.body
            );
            let unchanged = described(&emr, label, &id).await;
            assert_eq!(
                configuration_of(&unchanged),
                Some(created_with),
                "case {label:?}: a refused update must leave the stored configuration \
                 alone, got {unchanged}"
            );
            continue;
        }
        let after = updated(&answer, label);
        assert_eq!(
            configuration_of(&after),
            Some(submitted.clone()),
            "case {label:?}: the accepted limit must replace the stored one, got {after}"
        );
        let read_back = described(&emr, label, &id).await;
        assert_eq!(
            configuration_of(&read_back),
            Some(submitted.clone()),
            "case {label:?}: the accepted limit must round-trip through state, got \
             {read_back}"
        );
    }
}
