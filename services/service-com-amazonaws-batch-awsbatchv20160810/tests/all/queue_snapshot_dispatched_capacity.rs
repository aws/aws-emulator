// SPDX-License-Identifier: CC0-1.0

//! The CAPACITY `GetJobQueueSnapshot` reports a queue's ALREADY DISPATCHED jobs as using,
//! end to end over the wire.
//!
//! RULE under test: **`queueUtilization.totalCapacityUsage` is "capacity utilization for
//! already dispatched jobs", so it carries one entry per capacity unit the queue's dispatched
//! jobs reserve — for a container job, one per `ResourceType` of its container's
//! `resourceRequirements` ("the type of resource to assign to a container") — and each
//! entry's `quantity` is the SUM over those jobs of the quantity each one reserves ("the
//! quantity of the specified resource to reserve for the container"), reported as the
//! container states it. A job still waiting in the queue has been given nothing and a job
//! that has finished has given back what it held, so neither is counted.**
//!
//! Each row submits MORE jobs than the compute environment's `maxvCpus` can hold at once
//! (`ComputeResource$maxvCpus`, "the maximum number of vCPUs that a compute environment can
//! support"), so the queue always holds jobs that are NOT dispatched while the ones that are
//! hold their reservation. Whenever the snapshot reports capacity in use, the quantity of
//! every unit must therefore be a whole multiple — between one and the number of jobs that
//! fit at once — of what ONE of those jobs reserves.
//!
//! The rows differ along what the rule generalises over: the quantities one container
//! reserves (a whole vCPU, two of them, and the fraction a Fargate job may ask for, each with
//! its own `MEMORY` reservation), how many jobs hold a reservation at one instant (one and
//! two, so the SUM over several dispatched jobs is observed and not just one job's figure),
//! and how many jobs wait behind them.
//!
//! Sources: `smithy_query(query="operation", operation="GetJobQueueSnapshot")` for the
//! operation's own doc, `smithy_query(query="shape", shape="QueueSnapshotCapacityUsage")` for
//! `capacityUnit`/`quantity`, and `smithy_query(query="shape", shape="ResourceRequirement")`
//! for the reservation a container states.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{describe_compute_environment, describe_job, job_definition, Batch};

/// How long a loop here is given to reach the condition it polls for.
///
/// Every move of a job falls due [`state::JOB_PHASE_MILLIS`] after the one before it and the
/// jobs of a row are serialised by the capacity they share, so this is many spans of headroom
/// for a host running every other suite of this workspace beside this one. Each loop ends on
/// its CONDITION; this only bounds how long it waits for one.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);

/// How long one look waits before the next: far below one span of the clock, so a quantity
/// that is only held for one span is still seen several times.
const POLL_GAP: Duration = Duration::from_millis(25);

/// How close two reservation quantities must be to count as the same figure: both sides are
/// read from the same decimal strings, so the comparison only has to survive the sum.
const TOLERANCE: f64 = 1e-9;

/// The terminal `JobStatus` pair — a job no later call can move again, holding nothing.
/// Source: the `JobStatus` enum (`smithy_query(query="shape", shape="JobStatus")`).
const TERMINAL: [&str; 2] = [state::JOB_SUCCEEDED, state::JOB_FAILED];

/// One row: what the environment publishes, what one container reserves, and how many of
/// those reservations fit at once.
struct Row {
    case: &'static str,
    /// `ComputeResource$maxvCpus`.
    max_vcpus: i32,
    /// The `VCPU` quantity the container reserves, as `ResourceRequirement$value` carries it.
    vcpus: &'static str,
    /// The `MEMORY` quantity it reserves, in MiB, as the same member carries it.
    memory: &'static str,
    /// The MOST jobs whose reservation the queue may ever report at one instant.
    ///
    /// For a reservation of WHOLE vCPUs that is the published maximum divided by it: those
    /// are the vCPUs the environment "can support" (`ComputeResource$maxvCpus`) over the
    /// vCPUs one container reserves. For a reservation SMALLER than a vCPU the model fixes
    /// no such figure — how a fraction is counted against a whole-vCPU ceiling is the
    /// scheduler's accounting, not this rule — so that row bounds it by the jobs submitted
    /// instead and pins only the quantity reported per holder.
    holders_at_most: u32,
    /// The peak this row must actually OBSERVE, so a row that claims several jobs hold at
    /// once is not passed by a queue that only ever dispatches one.
    holders_peak: u32,
    /// How many jobs are submitted onto the one queue — always more than fit at once.
    jobs: usize,
}

/// The rows of the table.
const ROWS: &[Row] = &[
    Row {
        case: "one vCPU published, three jobs reserving one each",
        max_vcpus: 1,
        vcpus: "1",
        memory: "2048",
        holders_at_most: 1,
        holders_peak: 1,
        jobs: 3,
    },
    Row {
        case: "two vCPUs published, three jobs reserving one each",
        max_vcpus: 2,
        vcpus: "1",
        memory: "2048",
        holders_at_most: 2,
        holders_peak: 2,
        jobs: 3,
    },
    Row {
        // A reservation smaller than one vCPU: what it costs the published maximum is the
        // scheduler's accounting, so this row bounds the holders by the jobs submitted and
        // asserts only that the quantity reported is a whole number of the 0.25 the
        // container states — the declared value, not a rounded one.
        case: "two vCPUs published, three jobs reserving a quarter of one each",
        max_vcpus: 2,
        vcpus: "0.25",
        memory: "512",
        holders_at_most: 3,
        holders_peak: 1,
        jobs: 3,
    },
    Row {
        case: "four vCPUs published, three jobs reserving two each",
        max_vcpus: 4,
        vcpus: "2",
        memory: "4096",
        holders_at_most: 2,
        holders_peak: 2,
        jobs: 3,
    },
];

/// A `MANAGED` Fargate compute environment called `name` publishing `max_vcpus`, polled until
/// it reports `VALID`; answers its ARN.
///
/// The two required members of `ComputeResource` (`type`, `maxvCpus`) plus the networking a
/// Fargate environment needs. The maximum is what every row varies, so the create is made
/// here rather than through the crate's fixed-maximum helper.
async fn compute_environment(batch: &Batch, name: &str, max_vcpus: i32) -> String {
    let body = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": name,
                "type": "MANAGED",
                "state": state::STATE_ENABLED,
                "computeResources": {
                    "type": "FARGATE",
                    "maxvCpus": max_vcpus,
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

/// A queue called `name` over `environment`; answers its ARN.
async fn job_queue(batch: &Batch, name: &str, environment: &str) -> String {
    let created = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": name,
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
            }),
        )
        .await;
    created["jobQueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: expected a jobQueueArn, got {created}"))
        .to_string()
}

/// A `container` job definition called `name` whose container reserves `vcpus` vCPUs and
/// `memory` MiB.
async fn definition_reserving(batch: &Batch, name: &str, vcpus: &str, memory: &str) -> String {
    let (arn, _) = job_definition(
        batch,
        name,
        json!({
            "image": "busybox",
            "command": ["/bin/sh", "-c", "sleep 90"],
            "resourceRequirements": [
                {"type": state::RESOURCE_TYPE_VCPU, "value": vcpus},
                {"type": state::RESOURCE_TYPE_MEMORY, "value": memory},
            ],
        }),
    )
    .await;
    arn
}

/// Submit one job called `name` into `queue_arn`; answers its id.
async fn submit(batch: &Batch, queue_arn: &str, definition: &str, name: &str) -> String {
    let answered = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": name,
                "jobQueue": queue_arn,
                "jobDefinition": definition,
            }),
        )
        .await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {answered}"))
        .to_string()
}

/// The `totalCapacityUsage` entries of `queue`'s snapshot, beside the whole answer.
async fn capacity_in_use(batch: &Batch, queue: &str) -> (Vec<Value>, Value) {
    let answered = batch
        .ok("/v1/getjobqueuesnapshot", json!({"jobQueue": queue}))
        .await;
    let usage = answered["queueUtilization"]["totalCapacityUsage"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    (usage, answered)
}

/// The `quantity` reported for `unit`, or `None` when no entry carries that unit.
fn quantity_of(usage: &[Value], unit: &str) -> Option<f64> {
    usage
        .iter()
        .find(|entry| entry["capacityUnit"].as_str() == Some(unit))
        .and_then(|entry| entry["quantity"].as_f64())
}

/// RULE: the capacity a queue reports in use is the sum, per unit, of what its DISPATCHED
/// jobs reserve — the first half of the table.
#[tokio::test]
async fn the_snapshot_sums_what_the_queues_dispatched_jobs_reserve() {
    assert_the_usage_follows_the_dispatched_jobs(&ROWS[..2], "a").await;
}

/// The other half of the same table: a reservation smaller than one vCPU, and one of more
/// than one.
#[tokio::test]
async fn the_snapshot_reports_the_reservation_the_container_states() {
    assert_the_usage_follows_the_dispatched_jobs(&ROWS[2..], "b").await;
}

/// Drive `rows` through the rule, naming this run's resources after `group` so two tests of
/// the same table cannot collide.
async fn assert_the_usage_follows_the_dispatched_jobs(rows: &[Row], group: &str) {
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let tag = format!("{group}{index}");
        let batch = Batch::new();
        let environment =
            compute_environment(&batch, &format!("ce-usage{tag}"), row.max_vcpus).await;
        let queue_arn = job_queue(&batch, &format!("q-usage{tag}"), &environment).await;
        let definition =
            definition_reserving(&batch, &format!("jd-usage{tag}"), row.vcpus, row.memory).await;

        let mut submitted: Vec<String> = Vec::with_capacity(row.jobs);
        for position in 0..row.jobs {
            submitted.push(
                submit(
                    &batch,
                    &queue_arn,
                    &definition,
                    &format!("usage{tag}-{position}"),
                )
                .await,
            );
        }

        let one_vcpu: f64 = row
            .vcpus
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the row's VCPU value is a number"));
        let one_memory: f64 = row
            .memory
            .parse()
            .unwrap_or_else(|_| panic!("case {case}: the row's MEMORY value is a number"));
        // The most jobs ever seen holding their reservation at one instant, read off the
        // quantity the snapshot reported rather than off a separate look at the jobs.
        let mut most_held = 0_u32;
        let mut statuses: Vec<String> = Vec::new();
        let deadline = Instant::now() + SETTLES_WITHIN;
        loop {
            let (usage, answered) = capacity_in_use(&batch, &queue_arn).await;
            if !usage.is_empty() {
                // One entry per `ResourceType` the container reserves, and no other unit.
                let mut units: Vec<String> = usage
                    .iter()
                    .map(|entry| {
                        entry["capacityUnit"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                units.sort();
                assert_eq!(
                    units,
                    vec![
                        state::RESOURCE_TYPE_MEMORY.to_string(),
                        state::RESOURCE_TYPE_VCPU.to_string(),
                    ],
                    "case {case}: expected one entry per reserved resource type, got {answered}"
                );
                let held_vcpus = quantity_of(&usage, state::RESOURCE_TYPE_VCPU)
                    .unwrap_or_else(|| panic!("case {case}: expected a VCPU quantity, got {answered}"));
                let held_memory = quantity_of(&usage, state::RESOURCE_TYPE_MEMORY)
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a MEMORY quantity, got {answered}")
                    });
                // The quantity is a whole number of those reservations, and never more than
                // this row's bound however many jobs are on the queue.
                let holders = (1..=row.holders_at_most)
                    .find(|count| {
                        (held_vcpus - f64::from(*count) * one_vcpu).abs() < TOLERANCE
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: expected the vCPUs held to be between one and {} \
                             times {one_vcpu}, got {held_vcpus} from {answered}",
                            row.holders_at_most
                        )
                    });
                assert!(
                    (held_memory - f64::from(holders) * one_memory).abs() < TOLERANCE,
                    "case {case}: expected {} MiB held beside the {held_vcpus} vCPU of \
                     {holders} dispatched jobs, got {held_memory} from {answered}",
                    f64::from(holders) * one_memory
                );
                most_held = most_held.max(holders);
            }

            statuses.clear();
            for id in &submitted {
                let detail = describe_job(&batch, id).await;
                statuses.push(detail["status"].as_str().unwrap_or_default().to_string());
            }
            if statuses
                .iter()
                .all(|status| TERMINAL.contains(&status.as_str()))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected every job to settle, last {statuses:?}"
            );
            tokio::time::sleep(POLL_GAP).await;
        }

        assert!(
            most_held >= row.holders_peak,
            "case {case}: expected the queue to report the reservation of at least {} \
             dispatched jobs at once out of {}, the most it reported was {most_held}",
            row.holders_peak,
            row.jobs
        );
        // Every job has finished, so every one of them has given back what it held.
        let (settled, answered) = capacity_in_use(&batch, &queue_arn).await;
        assert!(
            settled.is_empty(),
            "case {case}: a finished job holds nothing, so the queue reports no capacity in \
             use, got {answered}"
        );
    }
}
