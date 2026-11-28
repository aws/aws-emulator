// SPDX-License-Identifier: CC0-1.0

//! Which queue gets a compute environment's ONE slot when the job holding it FINISHES, end to
//! end over the wire.
//!
//! RULE under test: **`CreateJobQueue$priority` — "job queues with a higher priority (or a
//! higher integer value for the priority parameter) are evaluated first when associated with
//! the same compute environment. Priority is determined in descending order" — decides every
//! time the environment has capacity to give, including the capacity a DISPATCHED job gives
//! back by reaching a terminal status: the job at the front of the highest-priority queue
//! waiting on that environment is admitted next, and the job on a lower-priority queue is
//! admitted after it. Between queues of EQUAL priority nothing separates the queues, so
//! submission order decides.**
//!
//! Each row fills the single vCPU the environment publishes
//! (`ComputeResource$maxvCpus`, "the maximum number of vCPUs that a compute environment can
//! support") with a run of filler jobs on one queue, and puts one waiting job on each of the
//! two queues that order that environment. Both waiting jobs name the LAST filler in their
//! `dependsOn` ("a list of dependencies for the job"), so neither can be considered until
//! that filler has reached a terminal status and given its reservation back — and both are
//! then released at the same instant, one span after it stopped, so what decides which of
//! them is admitted onto the one freed slot is their queues' `priority` and nothing about
//! when anybody happened to call.
//!
//! What is asserted is the instant each one's work STARTED (`JobDetail$startedAt`, "when the
//! job transitioned from the `STARTING` state to the `RUNNING` state") and the instant the
//! filler STOPPED (`JobDetail$stoppedAt`) — stored instants, not artefacts of when a poll
//! happened to look — and that every job then settles.
//!
//! The rows differ along what the rule generalises over: which of the two queues carries the
//! higher `priority` (the one the fillers are on and the other, so submission order cannot be
//! what decides), and the case where the two priorities are EQUAL.
//!
//! Sources: `smithy_query(query="operation", operation="CreateJobQueue")` for `priority`,
//! `smithy_query(query="shape", shape="ComputeResource")` for `maxvCpus`,
//! `smithy_query(query="doc", shape="SubmitJobRequest", member="dependsOn")` for the hold,
//! and `smithy_query(query="shape", shape="JobStatus")` for the statuses.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{describe_compute_environment, describe_job, job_definition, Batch};

/// How long a loop here is given to reach the condition it polls for.
///
/// Each move of a job falls due [`state::JOB_PHASE_MILLIS`] after the one before it and the
/// three jobs of a row are serialised by the one vCPU they share, so this is many spans of
/// headroom for a loaded host. Every loop ends on its CONDITION; this only bounds the wait.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);

/// How long one look waits before the next: far below one span of the clock.
const POLL_GAP: Duration = Duration::from_millis(25);

/// The terminal `JobStatus` pair. Source: the `JobStatus` enum
/// (`smithy_query(query="shape", shape="JobStatus")`).
const TERMINAL: [&str; 2] = [state::JOB_SUCCEEDED, state::JOB_FAILED];

/// How many filler jobs hold the one slot in turn before the waiting jobs are released.
///
/// The jobs of one queue are dispatched in submission order, and the environment publishes
/// one vCPU, so these run one after another and the LAST of them — the one the waiting jobs
/// depend on — ends several spans after the submits. That is what makes the hold independent
/// of how fast the host got through the submits: the waiting jobs are PENDING on a job that
/// is still queued, not racing a job that is about to finish.
const FILLERS: usize = 4;

/// One row: the two queues' priorities, and which of the two waiting jobs the freed slot must
/// go to first.
struct Row {
    case: &'static str,
    /// `CreateJobQueue$priority` of the queue the FILLERS and the first waiting job are
    /// submitted to, and of the queue the second waiting job is submitted to.
    priorities: (i32, i32),
    /// Which of the two waiting jobs starts first: 0 is the one on the fillers' own queue.
    starts_first: usize,
}

/// The rows of the table.
const ROWS: &[Row] = &[
    Row {
        // The shape the scenario takes when a backlog is pre-empted: the slot is held by
        // jobs on the LOW-priority queue, and the job waiting on the high-priority one is
        // admitted before the one that has been waiting beside them on the low-priority
        // queue.
        case: "the other queue has the higher priority",
        priorities: (1, 100),
        starts_first: 1,
    },
    Row {
        // The same two priorities swapped, so submission order cannot be what decides.
        case: "the fillers' own queue has the higher priority",
        priorities: (100, 1),
        starts_first: 0,
    },
    Row {
        case: "equal priorities: the job submitted first",
        priorities: (5, 5),
        starts_first: 0,
    },
];

/// A `MANAGED` Fargate compute environment called `name` publishing ONE vCPU, polled until it
/// reports `VALID`; answers its ARN.
async fn single_slot_environment(batch: &Batch, name: &str) -> String {
    let body = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": name,
                "type": "MANAGED",
                "state": state::STATE_ENABLED,
                "computeResources": {
                    "type": "FARGATE",
                    "maxvCpus": 1,
                    "subnets": ["subnet-1111"],
                    "securityGroupIds": ["sg-1111"],
                },
            }),
        )
        .await;
    let arn = body["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: expected an ARN, got {body}"))
        .to_string();
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let described = describe_compute_environment(batch, name).await;
        if described["status"].as_str() == Some(state::STATUS_VALID) {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case create {name}: the compute environment never reached {}, last {described}",
            state::STATUS_VALID
        );
        tokio::time::sleep(POLL_GAP).await;
    }
}

/// A queue called `name` at `priority` over `environment`; answers its ARN.
async fn job_queue(batch: &Batch, name: &str, priority: i32, environment: &str) -> String {
    let created = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": name,
                "priority": priority,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
            }),
        )
        .await;
    created["jobQueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: expected a jobQueueArn, got {created}"))
        .to_string()
}

/// A `container` job definition called `name` whose container reserves the whole vCPU the
/// environment publishes, and whose program names a wait.
async fn definition_holding_the_slot(batch: &Batch, name: &str) -> String {
    let (arn, _) = job_definition(
        batch,
        name,
        json!({
            "image": "busybox",
            "command": ["/bin/sh", "-c", "sleep 30"],
            "resourceRequirements": [
                {"type": state::RESOURCE_TYPE_VCPU, "value": "1"},
                {"type": state::RESOURCE_TYPE_MEMORY, "value": "2048"},
            ],
        }),
    )
    .await;
    arn
}

/// Submit one job called `name` into `queue_arn`, held until every job in `after` is
/// terminal; answers its id.
async fn submit(
    batch: &Batch,
    queue_arn: &str,
    definition: &str,
    name: &str,
    after: &[&str],
) -> String {
    let mut body = json!({
        "jobName": name,
        "jobQueue": queue_arn,
        "jobDefinition": definition,
    });
    if !after.is_empty() {
        body["dependsOn"] = Value::Array(
            after
                .iter()
                .map(|job_id| json!({"jobId": job_id}))
                .collect(),
        );
    }
    let answered = batch.ok("/v1/submitjob", body).await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {answered}"))
        .to_string()
}

/// Look again until `job_id` reports a terminal `JobStatus`, and answer its detail.
async fn until_terminal(batch: &Batch, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let detail = describe_job(batch, job_id).await;
        if TERMINAL.contains(&detail["status"].as_str().unwrap_or_default()) {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the job to settle, last {detail}"
        );
        tokio::time::sleep(POLL_GAP).await;
    }
}

/// The instant `detail`'s work started, required to be there.
fn started_at(case: &str, label: &str, detail: &Value) -> i64 {
    detail["startedAt"]
        .as_i64()
        .filter(|instant| *instant > 0)
        .unwrap_or_else(|| {
            panic!("case {case}: {label} ran, so it reports when it started, got {detail}")
        })
}

/// RULE: the slot a finished job gives back goes to the job at the front of the
/// highest-priority queue waiting on that compute environment — the rows where the two
/// queues' priorities differ.
///
/// The rows are driven by two tests rather than one, so the cases — whose jobs the one vCPU
/// SERIALISES, and which therefore take a span each — run beside each other rather than one
/// after the other.
#[tokio::test]
async fn the_freed_slot_goes_to_the_highest_priority_queue_waiting_on_it() {
    assert_the_freed_slot_follows_the_queues_priority(&ROWS[..2], "a").await;
}

/// The rest of the same table: two queues of EQUAL priority, where nothing separates the
/// queues and the job submitted first is admitted first.
#[tokio::test]
async fn equal_priority_queues_are_separated_by_submission_order() {
    assert_the_freed_slot_follows_the_queues_priority(&ROWS[2..], "b").await;
}

/// Drive `rows` through the rule, naming this run's resources after `group` so two tests of
/// the same table cannot collide.
async fn assert_the_freed_slot_follows_the_queues_priority(rows: &[Row], group: &str) {
    for (position, row) in rows.iter().enumerate() {
        let index = format!("{group}{position}");
        let case = row.case;
        let batch = Batch::new();
        let environment = single_slot_environment(&batch, &format!("ce-freed{index}")).await;
        let queues = [
            job_queue(
                &batch,
                &format!("q-freed{index}-a"),
                row.priorities.0,
                &environment,
            )
            .await,
            job_queue(
                &batch,
                &format!("q-freed{index}-b"),
                row.priorities.1,
                &environment,
            )
            .await,
        ];
        let definition =
            definition_holding_the_slot(&batch, &format!("jd-freed{index}")).await;

        // The FILLERS take the one vCPU the environment publishes in turn, so the slot is
        // held from the first of them until the last has finished with it.
        let mut fillers: Vec<String> = Vec::with_capacity(FILLERS);
        for position in 0..FILLERS {
            fillers.push(
                submit(
                    &batch,
                    &queues[0],
                    &definition,
                    &format!("freed{index}-filler-{position}"),
                    &[],
                )
                .await,
            );
        }
        let last_filler = fillers
            .last()
            .cloned()
            .expect("the table holds at least one filler");

        // One job on each queue, each held until that last filler is done — so both are
        // released at the ONE instant its reservation comes back, and nothing but their
        // queues' priority separates them.
        let waiting = [
            submit(
                &batch,
                &queues[0],
                &definition,
                &format!("freed{index}-probe-a"),
                &[&last_filler],
            )
            .await,
            submit(
                &batch,
                &queues[1],
                &definition,
                &format!("freed{index}-probe-b"),
                &[&last_filler],
            )
            .await,
        ];

        for (position, filler) in fillers.iter().enumerate() {
            let detail = until_terminal(&batch, case, filler).await;
            assert_eq!(
                detail["status"].as_str(),
                Some(state::JOB_SUCCEEDED),
                "case {case}: filler {position} runs and settles, got {detail}"
            );
        }
        let filler_detail = describe_job(&batch, &last_filler).await;
        let first = &waiting[row.starts_first];
        let second = &waiting[1 - row.starts_first];
        let first_detail = until_terminal(&batch, case, first).await;
        let second_detail = until_terminal(&batch, case, second).await;
        for (label, detail) in [
            ("the job admitted first", &first_detail),
            ("the job admitted second", &second_detail),
        ] {
            assert_eq!(
                detail["status"].as_str(),
                Some(state::JOB_SUCCEEDED),
                "case {case}: {label} runs once it is admitted, got {detail}"
            );
        }

        let first_started = started_at(case, "the job admitted first", &first_detail);
        let second_started = started_at(case, "the job admitted second", &second_detail);
        assert!(
            first_started < second_started,
            "case {case}: expected the job on the queue evaluated first to start before the \
             other, got {first_detail} then {second_detail}"
        );
        // The slot really was the last filler's: neither waiting job started before it gave
        // its reservation back.
        let filler_stopped = filler_detail["stoppedAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: the last filler ran, got {filler_detail}"));
        assert!(
            filler_stopped <= first_started,
            "case {case}: expected the first admitted job to start only once the filler had \
             given the slot back, got {filler_detail} then {first_detail}"
        );
        // And the one slot serialised the two of them: the second started only once the
        // first had stopped.
        let first_stopped = first_detail["stoppedAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: the first admitted job ran, got {first_detail}"));
        assert!(
            first_stopped <= second_started,
            "case {case}: one vCPU holds one of them at a time, so the second starts after \
             the first stops, got {first_detail} then {second_detail}"
        );
    }
}
