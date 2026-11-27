// SPDX-License-Identifier: CC0-1.0

//! What becomes of a job run nobody cancels: the outcome the clock owes it.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop`]), and driven with restJson1 HTTP requests, so
//! routing, deserialization, constraint validation, the handler and serialization all run
//! exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of submissions rather than one
//! request:
//!
//! 1. A job run the service hands back in a non-terminal state REACHES a terminal one on
//!    the clock, so a client that polls `DescribeJobRun` eventually reads a terminal
//!    `state` of the modelled `JobRunState` enum together with a `finishedAt` no earlier
//!    than the run's `createdAt`, and the state the run is reported in brings its
//!    `stateDetails` with it. The end an admitted run nobody cancels reaches, while the
//!    virtual cluster it was admitted to is available throughout, is the modelled state
//!    for a run that finished, and it carries no `failureReason`
//!    ([`crate::job_run_completion`] is this binary's suite for that rule and for the
//!    other end).
//! 2. `retryPolicyExecution.currentAttemptCount` is "the current number of attempts made
//!    on the driver of the job", counted out of the run's own lifecycle: none while the
//!    run is still waiting for a running slot, one from the admission that starts it, and
//!    unchanged by reading the run. A run submitted without a `retryPolicyConfiguration`
//!    reports no `retryPolicyExecution` at all.
//! 3. `ListJobRuns`' `states` filter selects a run by the state it has reached, and
//!    reports it exactly as `DescribeJobRun` does.

use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::JOB_RUN_STATE_VALUES;
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{field, instant_millis, text, Emr};
use crate::virtual_cluster_scheduler_status::{cancel_run, cluster_id, wait_for_the_clock};

/// The `IAMRoleArn` every submission below runs under.
const ROLE: &str = "arn:aws:iam::000000000000:role/EMRJobRole";

/// One submission: the members it carries beside its `clientToken`, and the retry policy
/// it configures when it configures one.
struct RunCase {
    label: &'static str,
    members: Value,
    /// The `retryPolicyConfiguration.maxAttempts` this row submits. `None` is a
    /// submission that configures no retry policy.
    max_attempts: Option<i64>,
}

/// The rows differ along everything the rule generalises over: which `JobDriver` member
/// the submission carries, whether it names the run, how many optional members it fills,
/// and whether it configures a retry policy — at `maxAttempts` values from one upwards,
/// since the attempts a run has MADE is a different number from the attempts it may make.
fn outcome_rows() -> Vec<RunCase> {
    vec![
        RunCase {
            label: "no retry policy, a spark-submit driver",
            members: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/plain.py"}},
            }),
            max_attempts: None,
        },
        RunCase {
            label: "a retry policy of a single attempt",
            members: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/single.py",
                    "entryPointArguments": ["--rows", "10"],
                }},
            }),
            max_attempts: Some(1),
        },
        RunCase {
            label: "a named run with a retry policy of three attempts and tags",
            members: json!({
                "name": "outcome-job",
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.15.0",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/named.py",
                    "sparkSubmitParameters": "--conf spark.executor.instances=2",
                }},
                "tags": {"owner": "analytics"},
            }),
            max_attempts: Some(3),
        },
        RunCase {
            label: "a spark-sql driver with a retry policy of ten attempts",
            members: json!({
                "name": "sql_outcome.1",
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-7.0.0-latest",
                "jobDriver": {"sparkSqlJobDriver": {
                    "entryPoint": "s3://bucket/query.sql",
                    "sparkSqlParameters": "-e select 1",
                }},
            }),
            max_attempts: Some(10),
        },
    ]
}

/// `GET /virtualclusters/{cluster}/jobruns/{run}`, as the `jobRun` it reports.
async fn described_run(emr: &Emr, case: &str, cluster: &str, run: &str) -> Value {
    let described = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{run}"),
            None,
        )
        .await;
    field(described.ok(case), "/jobRun", case).clone()
}

/// `GET /virtualclusters/{cluster}/jobruns{query}`, as the `jobRuns` it reports.
async fn listed_runs(emr: &Emr, case: &str, cluster: &str, query: &str) -> Vec<Value> {
    let listed = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns{query}"), None)
        .await;
    field(listed.ok(case), "/jobRuns", case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: jobRuns is not a list"))
        .clone()
}

/// `POST /virtualclusters/{cluster}/jobruns` for one row, with the token this run owns.
async fn submit(emr: &Emr, cluster: &str, case: &RunCase, token: &str) -> String {
    let mut body = case.members.clone();
    body["clientToken"] = json!(token);
    if let Some(max_attempts) = case.max_attempts {
        body["retryPolicyConfiguration"] = json!({"maxAttempts": max_attempts});
    }
    let started = emr.start_job(cluster, body).await;
    text(started.ok(case.label), "/id", case.label)
}

/// `String256`'s constraints on `JobRun$stateDetails`: length 1..256 and the pattern `\S`
/// — at least one non-whitespace character (smithy_query: shape `JobRun`).
fn is_state_details(value: &Value) -> bool {
    value.as_str().is_some_and(|rendered| {
        (1..=256).contains(&rendered.chars().count())
            && rendered.chars().any(|c| !c.is_whitespace())
    })
}

/// The `retryPolicyExecution.currentAttemptCount` a reported run carries, if it carries a
/// `retryPolicyExecution` at all. The member is `@required` inside that structure, so a
/// reported execution without it is a defect rather than an absent count.
fn attempts_made(job_run: &Value, case: &str) -> Option<i64> {
    let execution = job_run.get("retryPolicyExecution")?;
    Some(
        field(execution, "/currentAttemptCount", case)
            .as_i64()
            .unwrap_or_else(|| {
                panic!("case {case:?}: currentAttemptCount is not a number: {execution}")
            }),
    )
}

/// Poll `DescribeJobRun` until the run reports a terminal state, and answer the run as it
/// was then.
///
/// The deadline is the span the service owes a run's outcome plus margin for a loaded
/// machine: a run that is still non-terminal past it is the defect this suite is about —
/// a poll-until-terminal loop that never ends.
///
/// `pub(crate)` because [`crate::state_transition_refusal_wording`] waits for the same
/// outcome before it asks what the refusal of a finished run says, so the test binary
/// carries ONE definition of "wait until the run has settled".
pub(crate) async fn poll_until_terminal(
    emr: &Emr,
    case: &str,
    cluster: &str,
    run: &str,
) -> Value {
    let deadline = Instant::now() + state::JOB_OUTCOME_SPAN + Duration::from_secs(20);
    loop {
        let job_run = described_run(emr, case, cluster, run).await;
        let reported = text(&job_run, "/state", case);
        if state::job_state_is_terminal(&reported) {
            return job_run;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run never left {reported:?}: a submitted job run must reach \
             a terminal state, got {job_run}"
        );
        sleep(Duration::from_millis(100));
    }
}

/// RULES 1 + 2 + 3: every submitted run walks PENDING -> RUNNING -> a terminal state on
/// the clock, reporting its `stateDetails` in each, its `finishedAt` and `failureReason`
/// at the end, and the attempts made on its driver while it carries a retry policy.
#[tokio::test]
async fn every_admitted_job_run_reaches_a_terminal_state_on_the_clock() {
    let emr = Emr::new();
    let cluster = cluster_id(
        &emr,
        "the cluster the rows run on",
        "job-outcome",
        "tok-job-outcome",
        None,
    )
    .await;
    let rows = outcome_rows();

    // Every row is submitted first, so one span of the clock covers them all.
    let mut submitted = Vec::new();
    for (index, case) in rows.iter().enumerate() {
        let label = case.label;
        let id = submit(&emr, &cluster, case, &format!("tok-outcome-{index}")).await;
        let job_run = described_run(&emr, label, &cluster, &id).await;
        assert_eq!(
            text(&job_run, "/state", label),
            state::JOB_PENDING,
            "case {label:?}: a submitted run is accepted PENDING, got {job_run}"
        );
        assert!(
            job_run.get("finishedAt").is_none(),
            "case {label:?}: a run that has not finished reports no finishedAt: {job_run}"
        );
        assert!(
            job_run.get("failureReason").is_none(),
            "case {label:?}: a run that has not failed reports no failureReason: {job_run}"
        );
        assert_eq!(
            attempts_made(&job_run, label),
            case.max_attempts.map(|_| 0),
            "case {label:?}: a run still waiting has made no attempt on its driver, and one \
             with no retry policy reports no execution: {job_run}"
        );
        let created_at = instant_millis(&job_run, "/createdAt", label);
        submitted.push((case, id, created_at));
    }

    // The admission: the run is running, says so, and has had its driver attempted once.
    wait_for_the_clock();
    let mut running_details = Vec::new();
    for (case, id, _) in &submitted {
        let label = case.label;
        let running = described_run(&emr, label, &cluster, id).await;
        assert_eq!(
            text(&running, "/state", label),
            state::JOB_RUNNING,
            "case {label:?}: the clock owed this run RUNNING, got {running}"
        );
        let detail = field(&running, "/stateDetails", label).clone();
        assert!(
            is_state_details(&detail),
            "case {label:?}: an admitted run reports the detail of the state it is in, and it \
             must satisfy String256, got {detail}"
        );
        running_details.push(text(&running, "/stateDetails", label));
        assert!(
            running.get("finishedAt").is_none(),
            "case {label:?}: a RUNNING run has not finished, so it reports no finishedAt: \
             {running}"
        );
        assert_eq!(
            attempts_made(&running, label),
            case.max_attempts.map(|_| 1),
            "case {label:?}: the admission makes one attempt on the driver: {running}"
        );
    }

    // The outcome: a terminal state, with everything the model says a finished run reports.
    for ((case, id, created_at), running_detail) in submitted.iter().zip(&running_details) {
        let label = case.label;
        let settled = poll_until_terminal(&emr, label, &cluster, id).await;
        let reported = text(&settled, "/state", label);
        assert!(
            JOB_RUN_STATE_VALUES.contains(&reported.as_str()),
            "case {label:?}: {reported:?} is not a modelled JobRunState"
        );
        assert_eq!(
            reported,
            state::JOB_COMPLETED,
            "case {label:?}: a run admitted to an available cluster and left alone reaches \
             the state of a run that finished, got {settled}"
        );
        let finished_at = instant_millis(&settled, "/finishedAt", label);
        assert!(
            finished_at >= *created_at,
            "case {label:?}: finishedAt {finished_at} precedes createdAt {created_at}"
        );
        let settled_detail = text(&settled, "/stateDetails", label);
        assert!(
            is_state_details(field(&settled, "/stateDetails", label)),
            "case {label:?}: stateDetails must satisfy String256, got {settled_detail:?}"
        );
        assert_ne!(
            &settled_detail, running_detail,
            "case {label:?}: the detail must describe the state the run is reported in, not \
             the one it was in before"
        );
        assert!(
            settled.get("failureReason").is_none(),
            "case {label:?}: a run that finished did not fail, so it reports no \
             failureReason: {settled}"
        );
        assert_eq!(
            attempts_made(&settled, label),
            case.max_attempts.map(|_| 1),
            "case {label:?}: the attempts made on the driver are the ones the run made, \
             whatever maxAttempts allowed: {settled}"
        );
        assert_eq!(
            settled.get("retryPolicyConfiguration").cloned(),
            case.max_attempts
                .map(|max_attempts| json!({"maxAttempts": max_attempts})),
            "case {label:?}: the submitted retry policy must still round-trip: {settled}"
        );

        // A read neither moves a finished run nor consumes it.
        let again = described_run(&emr, label, &cluster, id).await;
        assert_eq!(
            again, settled,
            "case {label:?}: describing a finished run twice answered two different runs"
        );

        // A run that has already finished cannot be cancelled.
        cancel_run(&emr, &cluster, id)
            .await
            .refused(label, 400, "ValidationException");
    }

    // RULE 3: the states filter selects the runs by the state they reached, and reports
    // each exactly as the describe does.
    let case = "the states filter over the finished runs";
    let ids: Vec<&String> = submitted.iter().map(|(_, id, _)| id).collect();
    for (query, expected) in [
        (format!("?states={}", state::JOB_COMPLETED), ids.len()),
        (
            format!(
                "?states={}&states={}",
                state::JOB_COMPLETED,
                state::JOB_RUNNING
            ),
            ids.len(),
        ),
        (format!("?states={}", state::JOB_CANCELLED), 0),
        (format!("?states={}", state::JOB_FAILED), 0),
    ] {
        let listed = listed_runs(&emr, case, &cluster, &query).await;
        assert_eq!(
            listed.len(),
            expected,
            "case {case:?}: {query} must report {expected} runs, got {listed:?}"
        );
    }
    let listed =
        listed_runs(&emr, case, &cluster, &format!("?states={}", state::JOB_COMPLETED)).await;
    for (_, id, _) in &submitted {
        let same = listed
            .iter()
            .find(|run| run.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the listing does not report run {id}"));
        let described = described_run(&emr, case, &cluster, id).await;
        assert_eq!(
            same, &described,
            "case {case:?}: ListJobRuns and DescribeJobRun must report the same run"
        );
    }
}

/// RULE 2 again, where the count has to come out of the run's own lifecycle: a run held in
/// the queue by its cluster's `maxConcurrentJobRuns` has had NO attempt made on its
/// driver, and the admission a freed slot gives it is what makes one.
///
/// The rows differ in the limit, so in how many of the submitted runs are admitted at once
/// and how many wait. A cancellation is what frees a slot, so each row also walks the
/// cancelled run through the detail of each state it is moved into.
#[tokio::test]
async fn a_run_waiting_for_a_slot_has_made_no_attempt_on_its_driver() {
    let emr = Emr::new();
    // `JobLimitInteger`'s @range is 1..1000, so 1 is the published minimum.
    for limit in [1_i64, 2] {
        let label = format!("maxConcurrentJobRuns {limit}");
        let cluster = cluster_id(
            &emr,
            &label,
            &format!("attempt-count-{limit}"),
            &format!("tok-attempt-count-{limit}"),
            Some(json!({"maxConcurrentJobRuns": limit})),
        )
        .await;
        // One run more than the limit admits, each carrying a retry policy so that every
        // one of them reports the attempts made on it.
        let case = RunCase {
            label: "a run submitted against a concurrency limit",
            members: json!({
                "executionRoleArn": ROLE,
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/queued.py"}},
            }),
            max_attempts: Some(2),
        };
        let mut runs = Vec::new();
        for position in 0..=limit {
            runs.push(
                submit(
                    &emr,
                    &cluster,
                    &case,
                    &format!("tok-attempt-{limit}-{position}"),
                )
                .await,
            );
        }

        wait_for_the_clock();
        let mut admitted_detail = None;
        for (position, run) in runs.iter().enumerate() {
            let admitted = i64::try_from(position).unwrap_or(i64::MAX) < limit;
            let job_run = described_run(&emr, &label, &cluster, run).await;
            assert_eq!(
                text(&job_run, "/state", &label),
                if admitted {
                    state::JOB_RUNNING
                } else {
                    state::JOB_PENDING
                },
                "case {label:?}: run {position} of {} must be the one the limit admits, got \
                 {job_run}",
                runs.len()
            );
            assert_eq!(
                attempts_made(&job_run, &label),
                Some(i64::from(admitted)),
                "case {label:?}: run {position} has made an attempt only if it was admitted: \
                 {job_run}"
            );
            if admitted {
                admitted_detail = Some(text(&job_run, "/stateDetails", &label));
            } else {
                assert!(
                    job_run.get("stateDetails").is_none(),
                    "case {label:?}: a run that is only waiting has no detail beyond its \
                     state: {job_run}"
                );
            }
        }
        let admitted_detail = admitted_detail.expect("the row admits at least one run");

        // A cancellation frees the slot — and reports the detail of the state it moves the
        // cancelled run into, in each of its two steps, each distinct from the one the run
        // reported while it was running.
        cancel_run(&emr, &cluster, &runs[0]).await.ok(&label);
        let cancelling = described_run(&emr, &label, &cluster, &runs[0]).await;
        assert_eq!(
            text(&cancelling, "/state", &label),
            state::JOB_CANCEL_PENDING,
            "case {label:?}: the transient state right after CancelJobRun must be observable"
        );
        let cancelling_detail = text(&cancelling, "/stateDetails", &label);
        assert!(
            is_state_details(field(&cancelling, "/stateDetails", &label))
                && cancelling_detail != admitted_detail,
            "case {label:?}: a cancellation in flight reports its own String256 detail, got \
             {cancelling_detail:?} against the running run's {admitted_detail:?}"
        );

        wait_for_the_clock();
        let cancelled = described_run(&emr, &label, &cluster, &runs[0]).await;
        assert_eq!(
            text(&cancelled, "/state", &label),
            state::JOB_CANCELLED,
            "case {label:?}: the clock owed the cancelled run CANCELLED, got {cancelled}"
        );
        let cancelled_detail = text(&cancelled, "/stateDetails", &label);
        assert!(
            is_state_details(field(&cancelled, "/stateDetails", &label))
                && cancelled_detail != cancelling_detail,
            "case {label:?}: a cancelled run reports the detail of THAT state, got \
             {cancelled_detail:?} against the in-flight {cancelling_detail:?}"
        );
        assert!(
            cancelled.get("finishedAt").is_some(),
            "case {label:?}: a run in a terminal state reports a finishedAt: {cancelled}"
        );
        assert!(
            cancelled.get("failureReason").is_none(),
            "case {label:?}: a cancelled run did not fail, so it reports no failureReason: \
             {cancelled}"
        );

        // The freed slot admitted the run that was waiting, and THAT is what made its
        // first attempt.
        let promoted = runs.last().expect("the row submits at least one run");
        let waiting = described_run(&emr, &label, &cluster, promoted).await;
        assert_eq!(
            text(&waiting, "/state", &label),
            state::JOB_RUNNING,
            "case {label:?}: the freed slot must admit the run that was waiting, got {waiting}"
        );
        assert_eq!(
            attempts_made(&waiting, &label),
            Some(1),
            "case {label:?}: the admission is what makes the attempt: {waiting}"
        );
    }
}
