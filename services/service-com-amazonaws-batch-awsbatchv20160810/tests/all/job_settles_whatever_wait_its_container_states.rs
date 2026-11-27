// SPDX-License-Identifier: CC0-1.0

//! What a placed Fargate job REPORTS when its container's program states a wait, driven end to
//! end against the services that own the facts.
//!
//! The RULE under test: **the seconds in a container's `sleep` are seconds its program is
//! still there, so a job whose container is waiting is reported RUNNING and a job whose
//! container's program ENDED settles on the status that program exited with. How much wall
//! clock one such wait may spend is this emulator's own bound and never the request's — the
//! seconds are the caller's words, and `JobStatus` is what every client of AWS Batch waits on
//! — so a program that names a wait of minutes is read past it and settles well inside this
//! suite's budget, wherever that wait stands in it. Only a wait nothing in the program will
//! end keeps the container up with no bound at all.**
//!
//! Why the bound has to be the emulator's: `JobStatus` is what every client of AWS Batch waits
//! on — `aws batch wait jobs-complete`, a Step Functions `.sync` integration, any step that
//! blocks on `DescribeJobs` — so a job whose container's program named a long wait would hold
//! every one of them for as long as that request's own words, and the terminal status, the exit
//! code and the `statusReason` would be unreadable until it had passed. The spans a job's
//! transitions take are this service's ([`state::JOB_PHASE_MILLIS`] and the Amazon ECS task's
//! own), never a number taken off the request.
//!
//! The rows differ along what the rule generalises over: WHERE the wait stands in the program
//! (the whole program, last after a line it wrote, last before a step that exits non-zero, the
//! whole argument vector, inside a bounded loop), HOW it is spelled (whole seconds and seconds
//! with a fraction), HOW LONG it states (from ninety seconds to ten minutes, every one of them
//! far beyond the bound above) and WHETHER anything in the program will end it at all — the
//! contrast row, whose container goes on running because nothing in its program will.
//!
//! Sources: `smithy_query(query="shape",shape="JobStatus")` for the statuses and their terminal
//! pair; `smithy_query(query="shape",shape="AttemptContainerDetail")` for the attempt's
//! `exitCode` ("the exit code returned from the container … a non-zero exit code is considered
//! failed") and `taskArn`; `smithy_query(query="doc",shape="JobDetail",member="startedAt")` for
//! the instants a run reports. POSIX states what `sleep` means for the PROGRAM ("suspend
//! execution for at least the integral number of seconds specified by the time operand"), which
//! is the duration it states; nothing in the model or the guide makes a service's own reporting
//! wait for it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::fargate_job_execution_role_and_task_tags::{
    batch, ec2, element, execution_role, router, rpc, DEFAULT_ROUTE, FAILED, IMAGE, POLL_INTERVAL,
    PUBLISHED_EXECUTION_POLICY, REGION, SUBNET_CIDR, SUCCEEDED, VALID, VPC_CIDR,
};

/// Every name this suite creates begins with this, so nothing of it collides with another
/// suite's resources in the one test binary.
const PREFIX: &str = "statedwait";

/// How long a job of a settling row is given to reach a terminal status, polled for the
/// CONDITION rather than waited out.
///
/// Above the bound this emulator puts on one wait of a container's program, and far above the
/// spans the job plane and the Amazon ECS task really take, so a loaded host delays a row
/// instead of breaking it — while being far UNDER the shortest wait any settling row's program
/// names (ninety seconds), which is what makes those rows about the RULE rather than about this
/// machine's speed.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);

/// How long the compute environment is given to reach `VALID`.
const SETUP_WITHIN: Duration = Duration::from_secs(60);

/// How long a contrast row is watched once its job is RUNNING, re-read inside the loop that
/// spends it: nothing in its program will end the wait inside this window, so a job whose
/// container is still there is still running at the end of it however slowly the host got
/// there.
const WATCH_WINDOW: Duration = Duration::from_secs(2);

/// `JobStatus` members this suite names beside the terminal pair it imports.
const RUNNING: &str = "RUNNING";

/// What one row's job must come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ends {
    /// The program is read past its wait, so the job settles on this status with this exit code
    /// — inside [`SETTLES_WITHIN`], far short of the minutes its program named.
    Settles(&'static str, i64),
    /// Nothing in the program will end the wait at all — it names no end of its own — so the
    /// container stays up and the job goes on being reported `RUNNING` through
    /// [`WATCH_WINDOW`].
    RunsOn,
}

/// One row: the program the job definition's container declares, and what the job must report.
struct Row {
    case: &'static str,
    command: &'static [&'static str],
    ends: Ends,
}

/// The rows walk the shapes a stated wait takes, and the one shape that is not a stated
/// duration at all.
const ROWS: &[Row] = &[
    Row {
        // The shape a batch workload takes: one wait, and nothing else.
        case: "a wait that is the program's whole word",
        command: &["/bin/sh", "-c", "sleep 150"],
        ends: Ends::Settles(SUCCEEDED, 0),
    },
    Row {
        case: "a wait the program ends on after writing a line",
        command: &["/bin/sh", "-c", "echo working; sleep 600"],
        ends: Ends::Settles(SUCCEEDED, 0),
    },
    Row {
        // The container's WHOLE program is the wait, as the vector the runtime exec's: one
        // reading of a named wait, so this settles exactly as its shell spelling above does.
        case: "a wait the program states as its whole argument vector",
        command: &["sleep", "300"],
        ends: Ends::Settles(SUCCEEDED, 0),
    },
    Row {
        // Seconds with a fraction, which the utility also reads, so the shape the seconds are
        // written in is not what the rule turns on.
        case: "a wait the program ends on, stated with a fraction",
        command: &["/bin/sh", "-c", "sleep 240.5"],
        ends: Ends::Settles(SUCCEEDED, 0),
    },
    Row {
        // A wait between two of the program's own steps: the step after it is what the job
        // ends on, so a non-zero status is what it reports.
        case: "a wait before a step that exits non-zero",
        command: &["/bin/sh", "-c", "sleep 90; exit 7"],
        ends: Ends::Settles(FAILED, 7),
    },
    Row {
        // A wait inside a LOOP is honoured — that is the container waiting for something else —
        // but a loop with an end of its own still reaches it, so the job settles there.
        case: "a wait the program goes round a bounded loop on",
        command: &["/bin/sh", "-c", "for trip in 1 2; do sleep 1; done"],
        ends: Ends::Settles(SUCCEEDED, 0),
    },
    Row {
        // The CONTRAST: a wait with no end of its own is how a container stays up, so nothing
        // of the program will end this job's container and the job is reported running.
        case: "a wait with no end of its own",
        command: &["/bin/sh", "-c", "echo up; sleep infinity"],
        ends: Ends::RunsOn,
    },
];

/// The `containerProperties` of a Fargate job definition running `command` as `role`.
fn container(command: &[&'static str], role: &str) -> Value {
    json!({
        "image": IMAGE,
        "command": command,
        // "For jobs that run on Fargate resources, you must provide an execution role"
        // (`ContainerProperties$executionRoleArn`).
        "executionRoleArn": role,
        "networkConfiguration": {"assignPublicIp": "ENABLED"},
        "resourceRequirements": [
            {"type": "VCPU", "value": "0.25"},
            {"type": "MEMORY", "value": "512"},
        ],
    })
}

/// Stand the network, the execution role, the compute environment and the queue up, and answer
/// the router, the queue name and the role ARN.
///
/// The network is built through Amazon EC2's own API — with a way off the VPC, because the
/// fetch of the image the containers run has to reach the registry that serves it — so every id
/// the placement names is one that service minted and holds.
async fn fixture() -> (MultiServiceRouter, String, String) {
    let router = router();
    let vpc = element(
        &ec2(&router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
        "vpcId",
    );
    let subnet = element(
        &ec2(
            &router,
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
    let security_group = element(
        &ec2(
            &router,
            "CreateSecurityGroup",
            &[
                ("GroupName", &format!("{PREFIX}-group")),
                ("GroupDescription", "the-group-this-suites-tasks-run-under"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    let gateway = element(
        &ec2(&router, "CreateInternetGateway", &[]).await,
        "internetGatewayId",
    );
    ec2(
        &router,
        "AttachInternetGateway",
        &[
            ("InternetGatewayId", gateway.as_str()),
            ("VpcId", vpc.as_str()),
        ],
    )
    .await;
    let table = element(
        &ec2(&router, "CreateRouteTable", &[("VpcId", vpc.as_str())]).await,
        "routeTableId",
    );
    ec2(
        &router,
        "AssociateRouteTable",
        &[
            ("RouteTableId", table.as_str()),
            ("SubnetId", subnet.as_str()),
        ],
    )
    .await;
    ec2(
        &router,
        "CreateRoute",
        &[
            ("RouteTableId", table.as_str()),
            ("DestinationCidrBlock", DEFAULT_ROUTE),
            ("GatewayId", gateway.as_str()),
        ],
    )
    .await;
    let role = execution_role(
        &router,
        &format!("{PREFIX}Exec"),
        &[PUBLISHED_EXECUTION_POLICY],
    )
    .await;

    let environment = format!("{PREFIX}-ce");
    let queue = format!("{PREFIX}-queue");
    batch(
        &router,
        "/v1/createcomputeenvironment",
        json!({
            "computeEnvironmentName": &environment,
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                "maxvCpus": 16,
                "subnets": [&subnet],
                "securityGroupIds": [&security_group],
            },
        }),
    )
    .await;
    batch(
        &router,
        "/v1/createjobqueue",
        json!({
            "jobQueueName": &queue,
            "priority": 1,
            "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": &environment}],
        }),
    )
    .await;
    let deadline = Instant::now() + SETUP_WITHIN;
    loop {
        let body = batch(
            &router,
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": [&environment]}),
        )
        .await;
        if body["computeEnvironments"][0]["status"].as_str() == Some(VALID) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the compute environment never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    (router, queue, role)
}

/// The detail `DescribeJobs` reports for `job_id`.
async fn described(router: &MultiServiceRouter, job_id: &str) -> Value {
    let body = batch(router, "/v1/describejobs", json!({"jobs": [job_id]})).await;
    body["jobs"][0].clone()
}

/// Look again until `job_id` reports a terminal `JobStatus`, and answer its detail.
async fn until_terminal(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let detail = described(router, job_id).await;
        let status = detail["status"].as_str().unwrap_or_default();
        if status == SUCCEEDED || status == FAILED {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a terminal status inside {SETTLES_WITHIN:?} — how long a \
             wait of the program may spend is this emulator's bound, not the request's — last \
             {detail}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until `job_id` reports `RUNNING`, and answer its detail.
async fn until_running(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let detail = described(router, job_id).await;
        if detail["status"].as_str() == Some(RUNNING) {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the job to reach {RUNNING}, last {detail}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE: a placed job is read past the wait its container's program names and settles on the
/// status that program exited with, and only a wait with no end of its own keeps it running.
#[tokio::test]
async fn a_job_settles_whatever_wait_its_container_states() {
    let (router, queue, role) = fixture().await;

    // One job definition per row, and every job submitted BEFORE any is waited on, so the spans
    // the clocks owe them run together rather than one after another.
    let mut submitted: Vec<(&Row, String)> = Vec::with_capacity(ROWS.len());
    for (index, row) in ROWS.iter().enumerate() {
        let definition = format!("{PREFIX}-jd-{index}");
        batch(
            &router,
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": &definition,
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": container(row.command, &role),
            }),
        )
        .await;
        let answered = batch(
            &router,
            "/v1/submitjob",
            json!({
                "jobName": format!("{PREFIX}-job-{index}"),
                "jobQueue": &queue,
                "jobDefinition": &definition,
            }),
        )
        .await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected a jobId in {answered}", row.case))
            .to_string();
        submitted.push((row, job_id));
    }

    for (row, job_id) in &submitted {
        let case = row.case;
        let Ends::Settles(status, exit) = row.ends else {
            continue;
        };
        let detail = until_terminal(&router, case, job_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(status),
            "case {case:?}: expected the job to settle {status}, got {detail}"
        );
        for (where_, found) in [
            ("the job's container", &detail["container"]["exitCode"]),
            (
                "the attempt's container",
                &detail["attempts"][0]["container"]["exitCode"],
            ),
        ] {
            assert_eq!(
                found.as_i64(),
                Some(exit),
                "case {case:?}: expected the status its program exited with on {where_}, got \
                 {detail}"
            );
        }
        // A job that RAN reports the instants of that run, and the Amazon ECS task it ran as.
        for member in ["startedAt", "stoppedAt"] {
            assert!(
                detail[member].as_i64().is_some(),
                "case {case:?}: a job whose container ran reports {member}, got {detail}"
            );
        }
        let task_arn = detail["attempts"][0]["container"]["taskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected the task the job ran as, got {detail}"))
            .to_string();
        let cluster = task_arn
            .split('/')
            .nth(1)
            .unwrap_or_else(|| panic!("case {case:?}: expected a task ARN, got {task_arn}"))
            .to_string();
        let answered = rpc(
            &router,
            "ecs",
            "AmazonEC2ContainerServiceV20141113",
            "DescribeTasks",
            json!({"cluster": &cluster, "tasks": [&task_arn]}),
        )
        .await;
        let task = answered["tasks"][0].clone();
        assert_eq!(
            task["lastStatus"].as_str(),
            Some("STOPPED"),
            "case {case:?}: the task a settled job ran as has stopped, got {answered}"
        );
        assert_eq!(
            task["containers"][0]["exitCode"].as_i64(),
            Some(exit),
            "case {case:?}: the job's end is the status its task's container exited with, got \
             {answered}"
        );
    }

    // The CONTRAST rows: nothing in their programs will end the wait, so their containers stay
    // up and the jobs go on being reported running, read after read.
    let contrasts: Vec<(&str, &String)> = ROWS
        .iter()
        .zip(submitted.iter())
        .filter(|(row, _)| row.ends == Ends::RunsOn)
        .map(|(row, (_, job_id))| (row.case, job_id))
        .collect();
    assert!(
        !contrasts.is_empty(),
        "the table carries at least one contrast row"
    );
    for (case, job_id) in &contrasts {
        until_running(&router, case, job_id).await;
    }
    let until = Instant::now() + WATCH_WINDOW;
    while Instant::now() < until {
        tokio::time::sleep(POLL_INTERVAL).await;
        for (case, job_id) in &contrasts {
            let detail = described(&router, job_id).await;
            assert_eq!(
                detail["status"].as_str(),
                Some(RUNNING),
                "case {case:?}: a container nothing in its program will stop stays up, so its \
                 job goes on running, got {detail}"
            );
            assert!(
                detail["container"]["exitCode"].as_i64().is_none(),
                "case {case:?}: no exit status is reported for a program that has not ended, \
                 got {detail}"
            );
        }
    }
}
