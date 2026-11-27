// SPDX-License-Identifier: CC0-1.0

//! A CONSUMABLE RESOURCE caps how many jobs requiring it run at one instant, driven end to
//! end across the services that own the facts: Batch, Amazon ECS, Amazon VPC and CloudWatch
//! Logs on one engine.
//!
//! The RULE under test: **a job is admitted past `RUNNABLE` only while every consumable
//! resource its `consumableResourceProperties.consumableResourceList` names has the
//! `quantity` it asks for available, it HOLDS that quantity until it reaches a terminal
//! `JobStatus`, and the resource's reported triple reconciles at every instant** —
//! `CreateConsumableResource$totalQuantity` is "the total amount of the consumable resource
//! that is available", `DescribeConsumableResourceResponse$inUseQuantity` "the amount of the
//! consumable resource that is currently in use" and `availableQuantity` "the amount of the
//! consumable resource that is currently available to use", and `ConsumableResourceType`
//! `REPLENISHABLE` "indicates whether the resource is available to be re-used after a job
//! completes". So the quantity is a CONCURRENCY bound and never a refusal: a claimant that
//! does not fit waits where a job waits to be placed ("the job is ready to be scheduled to a
//! host", `JobStatus$RUNNABLE`), and it is admitted once a holder goes terminal and gives its
//! units back — so every claimant eventually reaches `SUCCEEDED`, the later one's `startedAt`
//! is at or after the earlier one's `stoppedAt`, and `inUseQuantity` is back to zero with
//! `totalQuantity` untouched by the jobs that consumed it.
//!
//! Sources: `smithy_query(query="operation",operation="DescribeConsumableResource")` for the
//! quantity triple; `smithy_query(query="shape",shape="ConsumableResourceRequirement")` for
//! `consumableResource`/`quantity`; `smithy_query(query="shape",shape="JobStatus")` for the
//! ladder.
//!
//! The rows differ along what the rule generalises over: the `totalQuantity` the resource
//! holds, the `quantity` each job claims of it (so one row admits one job at a time and
//! another admits two of three), how many claimants contend, whether the requirement names the
//! resource by NAME or by its ARN, and the shape of the program each claimant's container runs
//! — a wait that is the container's whole program, a wait the program ends on after a step of
//! its own, and a wait stated as the whole argument vector — because what a holder's run COSTS
//! must not decide whether the jobs behind it are ever admitted. The compute environment's
//! `maxvCpus` is deliberately far above the claimants' combined vCPU in every row, so the
//! consumable resource is the only bound in play.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing an assembled-emulator Batch suite needs is this test tree's, declared once
// beside the sibling suite that first needed it: the router that mounts Batch, Amazon ECS,
// Amazon EC2 and CloudWatch Logs over one engine, the signed Batch call, the network built
// through Amazon EC2's own API (so every id a placement names is one that service minted, and
// the subnet it places tasks in has a route to the registry their image comes from), and the
// public gallery image those tasks run.
use crate::batch_job_dependency_gate::{
    batch, network_of, router, IMAGE, PENDING, POLL_INTERVAL, REGION, SUCCEEDED,
};

/// The account this emulator mints its ARNs under.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `CEStatus`/`JQStatus` a compute environment and a job queue come to rest in.
const VALID: &str = "VALID";

/// The `JobStatus` members this suite judges by beside the two it imports, spelled out rather
/// than imported from the service crate so a wrong constant in the implementation cannot make
/// the suite pass. Source: `smithy_query(query="shape",shape="JobStatus")`.
const SUBMITTED: &str = "SUBMITTED";
const RUNNABLE: &str = "RUNNABLE";
const STARTING: &str = "STARTING";
const RUNNING: &str = "RUNNING";

/// The statuses of a job that is LIVE PAST ITS QUEUE: admitted onto capacity and not finished
/// with it, so holding whatever quantity it claimed.
const LIVE_PAST_THE_QUEUE: [&str; 2] = [STARTING, RUNNING];

/// The statuses of a job that is still BEFORE the queue's gate: accepted, and waiting to be
/// placed, so holding nothing.
const BEFORE_THE_QUEUE: [&str; 3] = [SUBMITTED, PENDING, RUNNABLE];

/// How long a wait below gives the clock: far above the spans Batch, Amazon ECS and Amazon
/// VPC give the moves a placed job's task makes, and every wait looks again for the CONDITION
/// rather than sleeping a fixed stretch, so a loaded machine delays a row instead of breaking
/// it.
const DEADLINE: Duration = Duration::from_secs(120);

/// One row of the table: a consumable resource, how much each claimant takes of it, and how
/// many claimants contend for it.
struct Row {
    case: &'static str,
    /// `CreateConsumableResource$totalQuantity`.
    total: i64,
    /// What each job's requirement claims of it.
    claim: i64,
    /// How many jobs are submitted at once.
    claimants: usize,
    /// Whether the requirement names the resource by its ARN rather than by its name.
    by_arn: bool,
    /// The `command` each claimant's container runs: a program that DWELLS, in one of the
    /// shapes a batch workload is written in.
    command: &'static [&'static str],
}

/// The rows. They differ along the total the resource holds, the quantity each claimant takes
/// (so the cap admits one at a time, and two of three at a time), how many claimants contend
/// and the form the requirement names the resource in.
const ROWS: [Row; 3] = [
    Row {
        // The recorded shape: the wait is the container's WHOLE program.
        case: "one unit, two claimants of one each, named by name",
        total: 1,
        claim: 1,
        claimants: 2,
        by_arn: false,
        command: &["sh", "-c", "sleep 45"],
    },
    Row {
        // A wait the program ENDS on, after a step of its own.
        case: "one unit, two claimants of one each, named by ARN",
        total: 1,
        claim: 1,
        claimants: 2,
        by_arn: true,
        command: &["sh", "-c", "echo working; sleep 40"],
    },
    Row {
        // A wait stated as the whole argument vector, with two of the three claimants
        // admitted at once.
        case: "four units, three claimants of two each",
        total: 4,
        claim: 2,
        claimants: 3,
        by_arn: false,
        command: &["sleep", "2"],
    },
];

/// How many claimants of `claim` units fit inside `total` at one instant.
fn admitted_at_once(row: &Row) -> usize {
    usize::try_from(row.total / row.claim).unwrap_or(0).min(row.claimants)
}

/// The ARN AWS Batch gives the consumable resource `name`: `consumable-resource/<name>`
/// under this emulator's account and the Region of the request that created it.
fn resource_arn(name: &str) -> String {
    format!("arn:aws:batch:{REGION}:{ACCOUNT}:consumable-resource/{name}")
}

/// What one row left in the store.
struct Built {
    /// The consumable resource the claimants require.
    resource: String,
    /// The queue they were submitted to.
    queue: String,
    /// The job definition carrying the row's requirement.
    definition: String,
}

/// Create the row's consumable resource, its Fargate job definition, its compute environment
/// and the queue in front of it, and observe both come to rest in `VALID`.
async fn fixture(
    router: &MultiServiceRouter,
    row: &Row,
    index: usize,
    network: &(String, String),
) -> Built {
    let resource = format!("batch-consumable-{index}");
    batch(
        router,
        row.case,
        "/v1/createconsumableresource",
        &json!({
            "consumableResourceName": &resource,
            "resourceType": "REPLENISHABLE",
            "totalQuantity": row.total,
        }),
    )
    .await;
    let reference = if row.by_arn {
        resource_arn(&resource)
    } else {
        resource.clone()
    };
    let definition = format!("batch-consumable-definition-{index}");
    batch(
        router,
        row.case,
        "/v1/registerjobdefinition",
        &json!({
            "jobDefinitionName": &definition,
            "type": "container",
            "platformCapabilities": ["FARGATE"],
            "containerProperties": {
                "image": IMAGE,
                "command": row.command,
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batchExecutionRole"),
                "resourceRequirements": [
                    {"type": "VCPU", "value": "0.25"},
                    {"type": "MEMORY", "value": "512"},
                ],
                "networkConfiguration": {"assignPublicIp": "ENABLED"},
            },
            "consumableResourceProperties": {
                "consumableResourceList": [
                    {"consumableResource": &reference, "quantity": row.claim},
                ],
            },
        }),
    )
    .await;
    let environment = format!("batch-consumable-ce-{index}");
    let queue = format!("batch-consumable-queue-{index}");
    batch(
        router,
        row.case,
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": &environment,
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                // Far above the claimants' combined vCPU, so the consumable resource is the
                // only bound in play.
                "maxvCpus": 16,
                "subnets": [&network.0],
                "securityGroupIds": [&network.1],
            },
        }),
    )
    .await;
    batch(
        router,
        row.case,
        "/v1/createjobqueue",
        &json!({
            "jobQueueName": &queue,
            "priority": 1,
            "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment}],
        }),
    )
    .await;
    until_valid(
        router,
        row.case,
        "/v1/describecomputeenvironments",
        &json!({"computeEnvironments": [&environment]}),
        "computeEnvironments",
    )
    .await;
    until_valid(
        router,
        row.case,
        "/v1/describejobqueues",
        &json!({"jobQueues": [&queue]}),
        "jobQueues",
    )
    .await;
    Built {
        resource,
        queue,
        definition,
    }
}

/// Look again until the one resource `request` describes reports `VALID`.
async fn until_valid(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    request: &Value,
    member: &str,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(router, case, uri, request).await;
        if body[member][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {uri} never reported {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `DescribeConsumableResource` for `reference`.
async fn describe_resource(router: &MultiServiceRouter, case: &str, reference: &str) -> Value {
    batch(
        router,
        case,
        "/v1/describeconsumableresource",
        &json!({"consumableResource": reference}),
    )
    .await
}

/// The `JobDetail`s `DescribeJobs` reports for `ids`, in the order asked for.
async fn describe_jobs(router: &MultiServiceRouter, case: &str, ids: &[String]) -> Vec<Value> {
    let body = batch(router, case, "/v1/describejobs", &json!({"jobs": ids})).await;
    let jobs = body["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        jobs.len(),
        ids.len(),
        "case {case:?}: expected one described job per id, got {body}"
    );
    jobs
}

/// Submit one job through `definition` on `queue` and answer its id.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    queue: &str,
    definition: &str,
    name: &str,
) -> String {
    let answered = batch(
        router,
        case,
        "/v1/submitjob",
        &json!({"jobName": name, "jobQueue": queue, "jobDefinition": definition}),
    )
    .await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {answered}"))
        .to_string()
}

/// RULE: the quantity of a consumable resource caps how many of its claimants are live past
/// the queue at one instant, every claimant is eventually admitted and succeeds, and the
/// reported quantity triple reconciles at every read — ending back at nothing in use with the
/// total untouched.
#[tokio::test]
async fn a_consumable_resources_quantity_serialises_the_jobs_that_require_it() {
    let router = router();
    let network = network_of(&router).await;

    // Every row's resource, capacity and claimants are built and SUBMITTED up front, so the
    // rows contend beside one another in the one emulator and the suite waits for the longest
    // of them rather than for their sum. Each row has its own resource, queue and compute
    // environment, so no row is a claimant on another's quantity.
    let mut rows: Vec<Watched> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let built = fixture(&router, row, index, &network).await;
        let before = describe_resource(&router, case, &built.resource).await;
        assert_eq!(
            (
                before["totalQuantity"].as_i64(),
                before["inUseQuantity"].as_i64(),
                before["availableQuantity"].as_i64(),
            ),
            (Some(row.total), Some(0), Some(row.total)),
            "case {case:?}: a resource nothing claims holds its whole total, got {before}"
        );
        // The claimants are submitted one after another with nothing waited on in between, so
        // which of them the allocator admits is the allocator's decision and not the suite's
        // ordering.
        let mut ids = Vec::new();
        for claimant in 0..row.claimants {
            ids.push(
                submit(
                    &router,
                    case,
                    &built.queue,
                    &built.definition,
                    &format!("batch-consumable-job-{index}-{claimant}"),
                )
                .await,
            );
        }
        rows.push(Watched {
            row,
            built,
            ids,
            seen_live: BTreeSet::new(),
            max_in_use: 0,
            finals: None,
        });
    }

    // Watch every row until all its claimants are terminal, judging each observation as it is
    // made: no instant may show more claimants live past the queue than the quantity fits, and
    // the triple must reconcile on every read.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut all_settled = true;
        for watched in &mut rows {
            if watched.finals.is_some() {
                continue;
            }
            let row = watched.row;
            let case = row.case;
            let fits = admitted_at_once(row);
            let details = describe_jobs(&router, case, &watched.ids).await;
            let live: Vec<&Value> = details
                .iter()
                .filter(|detail| {
                    LIVE_PAST_THE_QUEUE.contains(&detail["status"].as_str().unwrap_or_default())
                })
                .collect();
            assert!(
                live.len() <= fits,
                "case {case:?}: at most {fits} claimant(s) of {} units fit inside {} at one \
                 instant, got {} live: {details:?}",
                row.claim,
                row.total,
                live.len()
            );
            for detail in &live {
                if let Some(id) = detail["jobId"].as_str() {
                    watched.seen_live.insert(id.to_string());
                }
            }
            let resource = describe_resource(&router, case, &watched.built.resource).await;
            let total = resource["totalQuantity"].as_i64().unwrap_or_default();
            let in_use = resource["inUseQuantity"].as_i64().unwrap_or_default();
            let available = resource["availableQuantity"].as_i64().unwrap_or_default();
            assert_eq!(
                total, row.total,
                "case {case:?}: the jobs that consume a resource never change its total, got \
                 {resource}"
            );
            assert!(
                (0..=total).contains(&in_use),
                "case {case:?}: the quantity in use stays inside the total, got {resource}"
            );
            assert_eq!(
                available,
                total - in_use,
                "case {case:?}: the available quantity is the total less what is in use, got \
                 {resource}"
            );
            watched.max_in_use = watched.max_in_use.max(in_use);

            if details
                .iter()
                .all(|detail| detail["status"].as_str() == Some(SUCCEEDED))
            {
                watched.finals = Some(details);
            } else {
                all_settled = false;
            }
        }
        if all_settled {
            break;
        }
        let unsettled: Vec<&str> = rows
            .iter()
            .filter(|watched| watched.finals.is_none())
            .map(|watched| watched.row.case)
            .collect();
        assert!(
            Instant::now() < deadline,
            "the claimants of {unsettled:?} never all reached {SUCCEEDED}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    for watched in &rows {
        let row = watched.row;
        let case = row.case;
        let fits = admitted_at_once(row);
        let finals = watched
            .finals
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: every row settled before this point"));

        // Every claimant was admitted — the cap serialises the jobs, it never refuses them —
        // and each reports the window it ran in.
        assert_eq!(
            watched.seen_live.len(),
            row.claimants,
            "case {case:?}: every claimant must be observed live past the queue, saw {:?} of \
             {:?}",
            watched.seen_live,
            watched.ids
        );
        assert!(
            watched.max_in_use >= row.claim,
            "case {case:?}: a holder's units must be reported in use, the highest read was {}",
            watched.max_in_use
        );
        let mut windows: Vec<(i64, i64)> = Vec::new();
        for detail in finals {
            let started = detail["startedAt"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: a job that ran reports an integral startedAt, got {detail}")
            });
            let stopped = detail["stoppedAt"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: a job that ran reports an integral stoppedAt, got {detail}")
            });
            assert!(
                stopped >= started,
                "case {case:?}: a run stops at or after it started, got {detail}"
            );
            windows.push((started, stopped));
        }
        // The windows the claimants ran in never overlap more than the cap allows: no instant
        // has more than `fits` of them in flight, which for a resource that fits ONE claimant
        // is the later job's `startedAt` being at or after the earlier one's `stoppedAt`. The
        // instants are swept rather than compared pairwise, because which claimant releases
        // first is the allocator's business and not this suite's.
        let mut events: Vec<(i64, i8)> = Vec::with_capacity(windows.len() * 2);
        for (started, stopped) in &windows {
            events.push((*started, 1));
            events.push((*stopped, -1));
        }
        // A run that ends where another begins leaves no instant holding both, so the END of a
        // window is swept before the START of one at the same instant.
        events.sort_by_key(|(instant, delta)| (*instant, *delta));
        let mut in_flight: i64 = 0;
        let mut most_in_flight: i64 = 0;
        for (_, delta) in &events {
            in_flight += i64::from(*delta);
            most_in_flight = most_in_flight.max(in_flight);
        }
        assert!(
            most_in_flight <= i64::try_from(fits).unwrap_or(i64::MAX),
            "case {case:?}: at most {fits} claimant(s) of {} units can be in flight inside {} \
             at one instant, the windows {windows:?} overlap {most_in_flight} deep",
            row.claim,
            row.total
        );

        // The release: a REPLENISHABLE resource offers its whole total again.
        let after = describe_resource(&router, case, &watched.built.resource).await;
        assert_eq!(
            (
                after["totalQuantity"].as_i64(),
                after["inUseQuantity"].as_i64(),
                after["availableQuantity"].as_i64(),
            ),
            (Some(row.total), Some(0), Some(row.total)),
            "case {case:?}: once every claimant is terminal the whole total is offered again, \
             got {after}"
        );
    }
}

/// One row being watched: what it left in the store, and what the reads of it have shown so
/// far.
struct Watched<'row> {
    row: &'row Row,
    built: Built,
    /// The claimants, in the order they were submitted.
    ids: Vec<String>,
    /// The claimants that have been OBSERVED live past the queue.
    seen_live: BTreeSet<String>,
    /// The highest `inUseQuantity` any read of the row's resource reported.
    max_in_use: i64,
    /// The `JobDetail`s of the read that found every claimant terminal, and `None` while the
    /// row is still running.
    finals: Option<Vec<Value>>,
}

/// RULE: a claimant that does not fit WAITS where a job waits to be placed rather than being
/// refused — it is reported in a status before the queue while the holder runs, and nothing it
/// does charges the resource twice.
#[tokio::test]
async fn a_claimant_that_does_not_fit_waits_in_the_queue() {
    let router = router();
    let network = network_of(&router).await;
    let row = &ROWS[0];
    let case = "a claimant waiting for the single unit";
    let built = fixture(&router, row, 90, &network).await;

    let mut ids = Vec::new();
    for claimant in 0..row.claimants {
        ids.push(
            submit(
                &router,
                case,
                &built.queue,
                &built.definition,
                &format!("batch-consumable-waiter-{claimant}"),
            )
            .await,
        );
    }

    // While one claimant holds the unit, the other is in a status before the queue and the
    // resource reports exactly the one unit in use.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let details = describe_jobs(&router, case, &ids).await;
        let live: Vec<&Value> = details
            .iter()
            .filter(|detail| {
                LIVE_PAST_THE_QUEUE.contains(&detail["status"].as_str().unwrap_or_default())
            })
            .collect();
        if live.len() == 1 {
            let waiting: Vec<&str> = details
                .iter()
                .map(|detail| detail["status"].as_str().unwrap_or_default())
                .filter(|status| !LIVE_PAST_THE_QUEUE.contains(status))
                .collect();
            assert!(
                waiting
                    .iter()
                    .all(|status| BEFORE_THE_QUEUE.contains(status)),
                "case {case:?}: a claimant that does not fit waits where a job waits to be \
                 placed, got {details:?}"
            );
            let resource = describe_resource(&router, case, &built.resource).await;
            assert_eq!(
                (
                    resource["inUseQuantity"].as_i64(),
                    resource["availableQuantity"].as_i64(),
                ),
                (Some(row.claim), Some(row.total - row.claim)),
                "case {case:?}: the holder's units are the ones reported in use, got {resource}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no claimant was ever admitted while another waited, last \
             {details:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
