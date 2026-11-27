// SPDX-License-Identifier: CC0-1.0

//! The CONCURRENCY a consumable resource bounds for jobs that run as Amazon ECS TASKS, driven
//! end to end across the services that own the facts: AWS Batch, Amazon ECS and Amazon VPC on
//! one engine.
//!
//! The RULE under test: **a consumable resource's `totalQuantity` bounds how many claimants run
//! at once and refuses none of them** — `ConsumableResourceRequirement$quantity` is "the
//! quantity of the consumable resource that is needed" and `totalQuantity` "the total amount of
//! the consumable resource that is available", so a job is admitted past its queue only while
//! every resource it names has what it claims AVAILABLE, holds that quantity while it runs
//! (`inUseQuantity` is "the amount of the consumable resource that is currently in use" and
//! `availableQuantity` what is left of the total once that is taken off), and gives it back on
//! reaching a terminal `JobStatus` — a `REPLENISHABLE` resource then offers its whole total
//! again. The claimants are therefore SERIALISED, not rejected: every one of them runs and
//! reaches `SUCCEEDED`, and the turns of two claimants of the one quantity never overlap.
//!
//! Sources: `smithy_query(query="shape",shape="ConsumableResourceRequirement")` and
//! `smithy_query(query="operation",operation="CreateConsumableResource,DescribeConsumableResource")`
//! for the three quantities and `resourceType`;
//! `smithy_query(query="operation",operation="DescribeJobs")` for `JobDetail`'s `status`,
//! `startedAt` and `stoppedAt`; `smithy_query(query="shape",shape="JobStatus")` for the ladder.
//!
//! WHY this suite is in the router's own tree rather than in the Batch crate's: a job of a
//! FARGATE compute environment runs as a task of that environment's Amazon ECS cluster, so the
//! statuses the admission control is observed through — and the terminal status that RELEASES
//! the quantity — are reached through Amazon ECS. The Batch crate's own suite
//! (`services/service-com-amazonaws-batch-awsbatchv20160810/tests/all/consumable_resource_capacity.rs`)
//! drives the same rule with no Amazon ECS mounted; this one drives it over the path a Fargate
//! estate takes, which is the path a client provisions.
//!
//! The rows differ along what the rule generalises over: the resource's `totalQuantity`, the
//! `quantity` one job claims of it, and therefore how much of the total one claimant holds —
//! each row's total admitting exactly one claimant at a time, which is the bound the turns
//! below are judged against.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account this emulator mints its ARNs under, which the execution role below is named in.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `CEStatus`/`JQStatus` a compute environment and a job queue come to rest in. Source:
/// `smithy_query(query="shape",shape="CEStatus,JQStatus")`.
const VALID: &str = "VALID";

/// The terminal `JobStatus` members, and the two live statuses a job holds its claim in, spelled
/// out here rather than imported from a service crate so a wrong constant in the implementation
/// cannot make this suite pass. Source: `smithy_query(query="shape",shape="JobStatus")`, whose
/// order is SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED.
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";
const STARTING: &str = "STARTING";
const RUNNING: &str = "RUNNING";

/// `ConsumableResource$resourceType` REPLENISHABLE: the resource "available to be re-used after
/// a job completes" (`smithy_query(query="operation",operation="CreateConsumableResource")`).
const REPLENISHABLE: &str = "REPLENISHABLE";

/// The image every job definition below runs: a registry this emulator holds none of, reached
/// through the subnet the placement is given a route off the VPC in.
const IMAGE: &str = "registry.example.com/simparity/consumable-worker:1";

/// The program every claimant runs: a wait and nothing else, the shape of a job that DWELLS so
/// its hold on the quantity is a window a reader can look into. The seconds it names are the
/// program's own account of how long it took, not a stretch of this suite's clock.
const PROGRAM: [&str; 3] = ["sh", "-c", "sleep 45"];

/// The `/16` this suite's VPC is carved out of, and the `/24` its subnet takes.
const VPC_CIDR: &str = "10.71.0.0/16";
const SUBNET_CIDR: &str = "10.71.1.0/24";

/// The destination of a default route.
const DEFAULT_ROUTE: &str = "0.0.0.0/0";

/// The vCPUs the compute environment maintains: comfortably above the combined vCPU of the
/// claimants of any row, so the CAPACITY behind the queue is never the bound and what serialises
/// the jobs is the consumable resource alone.
const MAX_VCPUS: i64 = 4;

/// How long a wait below gives the clock: far above the spans AWS Batch, Amazon ECS and Amazon
/// VPC give the moves a placed job's task makes, and every wait looks again for the CONDITION
/// rather than sleeping a fixed stretch, so a loaded machine delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(240);

/// How long one look waits before the next.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// AWS Batch, Amazon ECS, Amazon EC2 and CloudWatch Logs over ONE shared engine, the way
/// `core/router/src/lib.rs` mounts every service: the task a placed job runs as is Amazon ECS's
/// resource, the subnet it is placed in is Amazon VPC's, and the group its container's output is
/// delivered into is CloudWatch Logs'. Each service claims only requests scoped to its own
/// signing name.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `service`, which is what routes the request. The key id and
/// signature are not credentials: the router above runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One AWS Batch call: a `restJson1` service, so `POST <uri>` with a JSON body, which must
/// answer 200.
async fn batch(router: &MultiServiceRouter, case: &str, uri: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://batch.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("batch.{REGION}.amazonaws.com"))
        .header("authorization", authorization("batch"))
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).into_owned()))
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Batch {uri}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon EC2 call: an `ec2Query` service, so a form body naming the action, answered as XML
/// — returned as text, which [`element`] pulls the minted ids out of.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={}&Version=2016-11-15", form_encode(action));
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("authorization", authorization("ec2"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with {text}"
    );
    text
}

/// Where this suite's tasks are placed: one VPC with an attached internet gateway, a subnet whose
/// route table sends the default route to that gateway (so a task given a public address reaches
/// the registry its image names), and the security group every placement names.
struct Network {
    subnet: String,
    security_group: String,
}

/// Build that network through Amazon EC2's own API, so every id below is one that service minted
/// and holds.
async fn network_of(router: &MultiServiceRouter) -> Network {
    let vpc = element(&ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await, "vpcId");
    let gateway = element(
        &ec2(router, "CreateInternetGateway", &[]).await,
        "internetGatewayId",
    );
    ec2(
        router,
        "AttachInternetGateway",
        &[
            ("InternetGatewayId", gateway.as_str()),
            ("VpcId", vpc.as_str()),
        ],
    )
    .await;
    let subnet = element(
        &ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", SUBNET_CIDR),
                ("AvailabilityZone", &format!("{REGION}a")),
            ],
        )
        .await,
        "subnetId",
    );
    let table = element(
        &ec2(router, "CreateRouteTable", &[("VpcId", vpc.as_str())]).await,
        "routeTableId",
    );
    ec2(
        router,
        "AssociateRouteTable",
        &[("RouteTableId", table.as_str()), ("SubnetId", subnet.as_str())],
    )
    .await;
    ec2(
        router,
        "CreateRoute",
        &[
            ("RouteTableId", table.as_str()),
            ("DestinationCidrBlock", DEFAULT_ROUTE),
            ("GatewayId", gateway.as_str()),
        ],
    )
    .await;
    let security_group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", "batch-consumable-claimants"),
                ("GroupDescription", "the group the tasks of this suite run under"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    Network {
        subnet,
        security_group,
    }
}

/// One row of the table: how much the resource holds and how much one claimant takes of it.
struct Row {
    case: &'static str,
    /// `CreateConsumableResource$totalQuantity`.
    total: i64,
    /// The `quantity` one job's requirement claims.
    claimed: i64,
}

/// The table. Every row's total admits exactly ONE claimant at a time — which is the bound the
/// turns are judged against — and the rows differ in the total, in what one claimant takes of it
/// and in whether the total divides by that claim.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one unit claimed whole",
            total: 1,
            claimed: 1,
        },
        Row {
            case: "two units claimed together",
            total: 2,
            claimed: 2,
        },
        Row {
            case: "two of three units, so one claimant and a remainder",
            total: 3,
            claimed: 2,
        },
    ]
}

/// Create the consumable resource of one row.
async fn create_resource(router: &MultiServiceRouter, row: &Row, name: &str) {
    batch(
        router,
        row.case,
        "/v1/createconsumableresource",
        &json!({
            "consumableResourceName": name,
            "resourceType": REPLENISHABLE,
            "totalQuantity": row.total,
        }),
    )
    .await;
}

/// Register one row's job definition — a FARGATE container job claiming `row.claimed` of the
/// resource `resource` — and answer its name.
async fn register(router: &MultiServiceRouter, row: &Row, name: &str, resource: &str) {
    batch(
        router,
        row.case,
        "/v1/registerjobdefinition",
        &json!({
            "jobDefinitionName": name,
            "type": "container",
            "platformCapabilities": ["FARGATE"],
            "containerProperties": {
                "image": IMAGE,
                "command": PROGRAM,
                // "For jobs that run on Fargate resources, you must provide an execution role"
                // (`ContainerProperties$executionRoleArn`).
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batchExecutionRole"),
                "resourceRequirements": [
                    {"type": "VCPU", "value": "0.25"},
                    {"type": "MEMORY", "value": "512"},
                ],
                "networkConfiguration": {"assignPublicIp": "ENABLED"},
            },
            "consumableResourceProperties": {
                "consumableResourceList": [
                    {"consumableResource": resource, "quantity": row.claimed},
                ],
            },
        }),
    )
    .await;
}

/// Create one row's compute environment and the queue in front of it, both named after `suffix`,
/// and answer the queue's name.
async fn capacity(
    router: &MultiServiceRouter,
    row: &Row,
    network: &Network,
    suffix: usize,
) -> String {
    let environment = format!("cr-fargate-ce-{suffix}");
    let queue = format!("cr-fargate-queue-{suffix}");
    batch(
        router,
        row.case,
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": &environment,
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                "maxvCpus": MAX_VCPUS,
                "subnets": [&network.subnet],
                "securityGroupIds": [&network.security_group],
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
        "computeEnvironments",
        &environment,
    )
    .await;
    until_valid(
        router,
        row.case,
        "/v1/describejobqueues",
        "jobQueues",
        &queue,
    )
    .await;
    queue
}

/// Look again until the resource `name` the read `uri` reports under `collection` says `VALID`.
async fn until_valid(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    collection: &str,
    name: &str,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(router, case, uri, &json!({collection: [name]})).await;
        if body[collection][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {name} never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Submit one job through `definition` onto `queue` and answer its id.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    queue: &str,
    definition: &str,
    job_name: &str,
) -> String {
    let answered = batch(
        router,
        case,
        "/v1/submitjob",
        &json!({
            "jobName": job_name,
            "jobQueue": queue,
            "jobDefinition": definition,
        }),
    )
    .await;
    answered["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {answered}"))
        .to_string()
}

/// The `JobDetail` of each of `jobs`, in order, read in ONE `DescribeJobs` call so the statuses
/// describe one state of the store rather than as many as there were calls.
async fn details(router: &MultiServiceRouter, case: &str, jobs: &[String]) -> Vec<Value> {
    let body = batch(router, case, "/v1/describejobs", &json!({"jobs": jobs})).await;
    let described = body["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        described.len(),
        jobs.len(),
        "case {case:?}: expected one described job per id, got {body}"
    );
    described
}

/// `DescribeConsumableResource` for `name`.
async fn resource_of(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    batch(
        router,
        case,
        "/v1/describeconsumableresource",
        &json!({"consumableResource": name}),
    )
    .await
}

/// One CONSISTENT look at a row: the claimants' statuses and the resource's quantities, taken
/// with the statuses read either side of the quantities, so a look the clock moved through is
/// discarded instead of judged.
struct Snapshot {
    claimants: Vec<Value>,
    resource: Value,
}

impl Snapshot {
    /// Take that look, or `None` when a claimant moved between the reads.
    async fn of(
        router: &MultiServiceRouter,
        case: &str,
        resource: &str,
        claimants: &[String],
    ) -> Option<Self> {
        let before = details(router, case, claimants).await;
        let resource = resource_of(router, case, resource).await;
        let after = details(router, case, claimants).await;
        let statuses = |jobs: &[Value]| -> Vec<String> {
            jobs.iter()
                .map(|job| job["status"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        (statuses(&before) == statuses(&after)).then_some(Self {
            claimants: after,
            resource,
        })
    }

    /// How many claimants are LIVE on the quantity: in a status past the queue and not terminal,
    /// which is the window a job holds what it claimed in.
    fn live_past_the_queue(&self) -> usize {
        self.claimants
            .iter()
            .filter(|job| {
                matches!(job["status"].as_str(), Some(status) if status == STARTING || status == RUNNING)
            })
            .count()
    }

    /// Whether every claimant has finished with the quantity.
    fn all_terminal(&self) -> bool {
        self.claimants.iter().all(|job| {
            matches!(job["status"].as_str(), Some(status) if status == SUCCEEDED || status == FAILED)
        })
    }
}

/// Look again until `job` reports a terminal `JobStatus`, and answer the detail it reports then.
async fn until_terminal(router: &MultiServiceRouter, case: &str, job: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    let jobs = [job.to_string()];
    loop {
        let detail = details(router, case, &jobs).await.remove(0);
        let status = detail["status"].as_str().unwrap_or_default().to_string();
        if status == SUCCEEDED || status == FAILED {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status, last {detail}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Drive ONE row: its own resource, job definition, compute environment and queue, two claimants
/// of a total that admits one of them, and every assertion the rule makes about them.
///
/// Each row's estate is its own, so the rows are driven BESIDE each other
/// ([`a_consumable_resource_serialises_the_fargate_jobs_that_claim_it`]) and the spans their
/// clocks owe run together rather than one row after another.
async fn drive_row(router: &MultiServiceRouter, row: &Row, index: usize, network: &Network) {
    let case = row.case;
    let resource = format!("cr-fargate-{index}");
    let definition = format!("cr-fargate-jd-{index}");
    create_resource(router, row, &resource).await;
    register(router, row, &definition, &resource).await;
    let queue = capacity(router, row, network, index).await;

    // Two claimants of a total that admits one of them, submitted back to back: the second
    // asks for a quantity the first is about to hold.
    let claimants = vec![
        submit(router, case, &queue, &definition, &format!("claimant-{index}-one")).await,
        submit(router, case, &queue, &definition, &format!("claimant-{index}-two")).await,
    ];

    // The hold is WITNESSED while the claimants are outstanding: the quantity one of them
    // took is reported in use, that much less is available, the total never moves, and no
    // more claimants than the total admits are live on it. Every look is one CONSISTENT
    // snapshot ([`Snapshot::of`]) — the statuses and the quantities are separate calls and
    // the clock moves between them — so a look the clock moved through is taken again
    // rather than judged, and the loop ends on the CONDITION.
    let admitted_at_once = usize::try_from(row.total / row.claimed).unwrap_or(1);
    let mut witnessed = false;
    let mut settled = false;
    let deadline = Instant::now() + DEADLINE;
    while !(witnessed && settled) {
        if let Some(snapshot) = Snapshot::of(router, case, &resource, &claimants).await {
            let live = snapshot.live_past_the_queue();
            assert!(
                live <= admitted_at_once,
                "case {case:?}: a total of {} admits {admitted_at_once} claimant(s) of {} at \
                 once, got {live} live on it: {:?}",
                row.total,
                row.claimed,
                snapshot.claimants
            );
            let in_use = snapshot.resource["inUseQuantity"].as_i64();
            assert_eq!(
                snapshot.resource["totalQuantity"].as_i64(),
                Some(row.total),
                "case {case:?}: the jobs that consume a resource never move its total, got {}",
                snapshot.resource
            );
            assert_eq!(
                snapshot.resource["availableQuantity"].as_i64(),
                Some(row.total - in_use.unwrap_or_default()),
                "case {case:?}: what is available is the total less what is in use, got {}",
                snapshot.resource
            );
            assert!(
                in_use.is_some_and(|in_use| in_use >= 0 && in_use <= row.total),
                "case {case:?}: what is in use never exceeds the total, got {}",
                snapshot.resource
            );
            // One claimant holding exactly what it claimed is the hold this rule is about:
            // the quantity a job takes is published while the job is in flight, and (every
            // row's total admitting ONE claimant) no second claimant holds it beside it.
            witnessed |= in_use == Some(row.claimed);
            settled = snapshot.all_terminal();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the quantity {} was never reported in use while its claimants \
             were outstanding",
            row.claimed
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Every claimant gets its turn: it RUNS rather than being refused, reaches SUCCEEDED, and
    // reports the instants its work ran between.
    let mut turns: Vec<(String, i64, i64)> = Vec::with_capacity(claimants.len());
    for (position, job) in claimants.iter().enumerate() {
        let claimant = format!("{case} / claimant {position}");
        let detail = until_terminal(router, &claimant, job).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(SUCCEEDED),
            "case {claimant:?}: a claimant the total serialises still RUNS and succeeds, got \
             {detail}"
        );
        let started = detail["startedAt"].as_i64().unwrap_or_default();
        let stopped = detail["stoppedAt"].as_i64().unwrap_or_default();
        assert!(
            started > 0 && stopped >= started,
            "case {claimant:?}: a job that ran reports when it started and stopped, got {detail}"
        );
        turns.push((claimant, started, stopped));
    }

    // The turns do not OVERLAP: the quantity is a concurrency bound this total admits one
    // claimant inside, so no two of them ever ran at the same instant. The same rule the
    // snapshot witnessed, stated over what the FINISHED jobs report, which no window of the
    // clock can hide.
    turns.sort_by_key(|(_, started, _)| *started);
    for consecutive in turns.windows(2) {
        let (earlier, _, earlier_stopped) = &consecutive[0];
        let (later, later_started, _) = &consecutive[1];
        assert!(
            later_started >= earlier_stopped,
            "case {later:?}: a claimant runs only once {earlier:?} has given the quantity \
             back, got a start at {later_started} against a stop at {earlier_stopped}"
        );
    }

    // A REPLENISHABLE resource is whole again once every claimant is done with it, and its
    // total never moved.
    let after = resource_of(router, case, &resource).await;
    assert_eq!(
        (
            after["totalQuantity"].as_i64(),
            after["inUseQuantity"].as_i64(),
            after["availableQuantity"].as_i64(),
        ),
        (Some(row.total), Some(0), Some(row.total)),
        "case {case:?}: a replenishable total is offered again once its claimants are done, \
         got {after}"
    );
}

/// RULE: a consumable resource's total bounds how many of its claimants run at once and refuses
/// none of them — the quantity is held while a claimant runs, given back when it goes terminal,
/// and every claimant runs in a turn of its own and reaches `SUCCEEDED`.
#[tokio::test]
async fn a_consumable_resource_serialises_the_fargate_jobs_that_claim_it() {
    let router = router();
    let network = network_of(&router).await;
    let rows = rows();

    // The rows are driven BESIDE each other, each over its own estate: a row's claimants wait for
    // one another, so running the rows one after the other would add their waits up instead of
    // overlapping them.
    tokio::join!(
        drive_row(&router, &rows[0], 0, &network),
        drive_row(&router, &rows[1], 1, &network),
        drive_row(&router, &rows[2], 2, &network),
    );
}
