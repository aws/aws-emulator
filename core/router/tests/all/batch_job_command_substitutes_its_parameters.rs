// SPDX-License-Identifier: CC0-1.0

//! The PARAMETER SUBSTITUTION a Batch job's container properties carry, driven end to end
//! across the services that own the facts: Batch, Amazon ECS, Amazon VPC and CloudWatch Logs
//! on one engine.
//!
//! The RULE under test: **the container a placed job's task runs is the job's declared
//! container with the job's EFFECTIVE parameters substituted into every `Ref::<name>`
//! placeholder it carries — the words of its `command` and the values of its `environment`
//! variables — so the status that program exits with is the job's terminal status and its
//! `container.exitCode`.** `RegisterJobDefinition$parameters` is "default parameter
//! substitution placeholders to set in the job definition" and `SubmitJob$parameters` is
//! "additional parameters passed to the job that replace parameter substitution placeholders
//! that are set in the job definition … Parameters in a `SubmitJob` request override any
//! corresponding parameter defaults from the job definition". The job REPORTS that resolved
//! container: `DescribeJobs` answers the `command` the container was given — no `Ref::`
//! placeholder stands in it — beside the `parameters` whose values fill it.
//!
//! Sources: `smithy_query(query="doc",shape="RegisterJobDefinitionRequest",member="parameters")`
//! and `smithy_query(query="doc",shape="SubmitJobRequest",member="parameters")` for the rule;
//! `smithy_query(query="shape",shape="JobStatus")` for the terminal statuses;
//! `smithy_query(query="shape",shape="ContainerDetail")` for `command`, `environment` and
//! `exitCode`.
//!
//! Only the assembled estate reaches this end of it: a Fargate job's container is a container
//! of an Amazon ECS TASK, so the words that container is given travel to that service and the
//! exit status the job reports is the one the program ended with there. The batch crate's own
//! `request_contracts_and_parameters.rs` pins the same rule on the path a job placed on
//! capacity that can take no task takes.
//!
//! The rows differ along what the rule generalises over: WHERE the placeholder stands (a
//! command word, an OPERAND the program reads as a positional parameter, an environment
//! variable's value, both), WHERE its value comes from (the job definition's default, the
//! submit's override), and the status the substituted program exits with.

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

/// The terminal `JobStatus` members, spelled out here rather than imported from a service
/// crate so a wrong constant in the implementation cannot make this suite pass. Source:
/// `smithy_query(query="shape",shape="JobStatus")`.
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";

/// The `/16` this suite's VPC is carved out of, and the `/24` its subnet takes.
const VPC_CIDR: &str = "10.71.0.0/16";
const SUBNET_CIDR: &str = "10.71.1.0/24";

/// The destination of a default route.
const DEFAULT_ROUTE: &str = "0.0.0.0/0";

/// How long a wait below gives the clock: far above the spans Batch and Amazon ECS give the
/// moves a placed job's task makes. Every wait looks again for the CONDITION rather than
/// sleeping a fixed stretch, so a loaded machine delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(240);

/// How long one look waits before the next.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// AWS Batch, Amazon ECS, Amazon EC2 and CloudWatch Logs over ONE shared engine, the way
/// `core/router/src/lib.rs` mounts every service.
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
    let parsed: Value = serde_json::from_slice(&payload)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).into_owned()));
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Batch {uri}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon EC2 call: an `ec2Query` service, so a form body naming the action, answered as
/// XML — returned as text, which [`element`] pulls the minted ids out of.
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

/// The subnet and security group the compute environment below places its tasks in, built
/// through Amazon EC2's own API so every id is one that service minted and holds.
struct Network {
    subnet: String,
    security_group: String,
}

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
                ("GroupName", "batch-parameter-jobs"),
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

/// Look again until the compute environment `name` reports `VALID`.
async fn until_environment_valid(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(
            router,
            case,
            "/v1/describecomputeenvironments",
            &json!({"computeEnvironments": [name]}),
        )
        .await;
        if body["computeEnvironments"][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the compute environment never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the job queue `name` reports `VALID`.
async fn until_queue_valid(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(router, case, "/v1/describejobqueues", &json!({"jobQueues": [name]})).await;
        if body["jobQueues"][0]["status"].as_str() == Some(VALID) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job queue never reached {VALID}, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until `job_id` reports a terminal `JobStatus`, and answer the detail it reports
/// then. Nothing here sleeps for a span: it waits for the CONDITION.
async fn until_terminal(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = batch(router, case, "/v1/describejobs", &json!({"jobs": [job_id]})).await;
        let detail = body["jobs"][0].clone();
        let status = detail["status"].as_str().unwrap_or_default().to_string();
        if status == SUCCEEDED || status == FAILED {
            return detail;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status, last {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One row of the table: the container properties the definition declares, the parameter
/// defaults it carries, the parameters the submit names, and the status the substituted
/// program must exit with.
struct Row {
    case: &'static str,
    /// The container's `command`, the argument vector its image is handed.
    command: Vec<&'static str>,
    /// The container's `environment`, as (name, value) pairs — values may carry placeholders.
    environment: Vec<(&'static str, &'static str)>,
    /// The job definition's `parameters`.
    defaults: Vec<(&'static str, &'static str)>,
    /// The submit's own `parameters`, which override those defaults.
    overrides: Vec<(&'static str, &'static str)>,
    /// The command the job must then report, which is the one its container was given.
    resolved: Vec<&'static str>,
    /// The status the program must exit with.
    exit_code: i64,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a placeholder in a command word, filled by the definition's default",
            command: vec!["sh", "-c", "exit Ref::code"],
            environment: Vec::new(),
            defaults: vec![("code", "0")],
            overrides: Vec::new(),
            resolved: vec!["sh", "-c", "exit 0"],
            exit_code: 0,
        },
        Row {
            case: "the same placeholder, overridden by the submit",
            command: vec!["sh", "-c", "exit Ref::code"],
            environment: Vec::new(),
            defaults: vec![("code", "0")],
            overrides: vec![("code", "5")],
            resolved: vec!["sh", "-c", "exit 5"],
            exit_code: 5,
        },
        Row {
            case: "a placeholder in an OPERAND the program reads as a positional parameter",
            command: vec!["sh", "-c", "exit \"$0\"", "Ref::code"],
            environment: Vec::new(),
            defaults: vec![("code", "0")],
            overrides: Vec::new(),
            resolved: vec!["sh", "-c", "exit \"$0\"", "0"],
            exit_code: 0,
        },
        Row {
            case: "that same operand, decided by the submit",
            command: vec!["sh", "-c", "exit \"$0\"", "Ref::code"],
            environment: Vec::new(),
            defaults: vec![("code", "0")],
            overrides: vec![("code", "7")],
            resolved: vec!["sh", "-c", "exit \"$0\"", "7"],
            exit_code: 7,
        },
        Row {
            case: "a placeholder in an environment value, filled by the default",
            command: vec!["sh", "-c", "exit $EXIT_CODE"],
            environment: vec![("EXIT_CODE", "Ref::code")],
            defaults: vec![("code", "0")],
            overrides: Vec::new(),
            resolved: vec!["sh", "-c", "exit $EXIT_CODE"],
            exit_code: 0,
        },
        Row {
            case: "a placeholder in an environment value, overridden by the submit",
            command: vec!["sh", "-c", "exit $EXIT_CODE"],
            environment: vec![("EXIT_CODE", "Ref::code")],
            defaults: vec![("code", "0")],
            overrides: vec![("code", "3")],
            resolved: vec!["sh", "-c", "exit $EXIT_CODE"],
            exit_code: 3,
        },
        Row {
            case: "placeholders in both, and a parameter only the submit names",
            command: vec!["sh", "-c", "echo Ref::label; exit $EXIT_CODE"],
            environment: vec![("EXIT_CODE", "Ref::code")],
            defaults: vec![("code", "9")],
            overrides: vec![("code", "0"), ("label", "shard-7")],
            resolved: vec!["sh", "-c", "echo shard-7; exit $EXIT_CODE"],
            exit_code: 0,
        },
    ]
}

/// `pairs` as a `ParametersMap` document.
fn parameters(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect(),
    )
}

/// RULE: the container a placed job's task runs is the declared one with the job's effective
/// parameters substituted into its command words and its environment values.
#[tokio::test]
async fn a_jobs_container_runs_with_its_parameters_substituted() {
    let rows = rows();
    let router = router();
    let network = network_of(&router).await;

    // One MANAGED Fargate compute environment and one queue over it: every row's job is
    // placed on the same capacity, so the table costs one estate.
    let case = "the estate";
    batch(
        &router,
        case,
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": "ce-parameters",
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                "maxvCpus": 16,
                "subnets": [&network.subnet],
                "securityGroupIds": [&network.security_group],
            },
        }),
    )
    .await;
    until_environment_valid(&router, case, "ce-parameters").await;
    batch(
        &router,
        case,
        "/v1/createjobqueue",
        &json!({
            "jobQueueName": "jq-parameters",
            "priority": 1,
            "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "ce-parameters"}],
        }),
    )
    .await;
    until_queue_valid(&router, case, "jq-parameters").await;

    // Every job is submitted first and read back afterwards: each settles on its own clock.
    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut container = json!({
            "image": "busybox",
            "command": row.command,
            // "For jobs that run on Fargate resources, you must provide an execution role"
            // (`ContainerProperties$executionRoleArn`).
            "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batchExecutionRole"),
            "resourceRequirements": [
                {"type": "VCPU", "value": "0.25"},
                {"type": "MEMORY", "value": "512"},
            ],
            "networkConfiguration": {"assignPublicIp": "ENABLED"},
        });
        if !row.environment.is_empty() {
            container["environment"] = Value::Array(
                row.environment
                    .iter()
                    .map(|(name, value)| json!({"name": name, "value": value}))
                    .collect(),
            );
        }
        let registered = batch(
            &router,
            row.case,
            "/v1/registerjobdefinition",
            &json!({
                "jobDefinitionName": format!("jd-parameters-{index}"),
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "parameters": parameters(&row.defaults),
                "containerProperties": container,
            }),
        )
        .await;
        let definition = registered["jobDefinitionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected an ARN, got {registered}", row.case))
            .to_string();
        let mut request = json!({
            "jobName": format!("parameterised-{index}"),
            "jobQueue": "jq-parameters",
            "jobDefinition": definition,
        });
        if !row.overrides.is_empty() {
            request["parameters"] = parameters(&row.overrides);
        }
        let answered = batch(&router, row.case, "/v1/submitjob", &request).await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a jobId, got {answered}", row.case))
            .to_string();
        submitted.push((row, job_id));
    }

    for (row, job_id) in submitted {
        let case = row.case;
        let detail = until_terminal(&router, case, &job_id).await;
        let expected_status = if row.exit_code == 0 { SUCCEEDED } else { FAILED };
        assert_eq!(
            detail["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the substituted program to end the job {expected_status}, \
             got {detail}"
        );
        assert_eq!(
            detail["container"]["exitCode"].as_i64(),
            Some(row.exit_code),
            "case {case:?}: expected the program to exit {}, got {detail}",
            row.exit_code
        );
        // The RECORD is the container the job RUNS: the values stand where the placeholders
        // were, beside the parameters they came from, so a client reads what ran.
        assert_eq!(
            detail["container"]["command"],
            json!(row.resolved),
            "case {case:?}: expected the resolved command to be reported, got {detail}"
        );
        assert!(
            !detail["container"]["command"]
                .to_string()
                .contains("Ref::"),
            "case {case:?}: no placeholder may stand in the command the job ran, got {detail}"
        );
        for (name, value) in &row.overrides {
            assert_eq!(
                detail["parameters"][*name].as_str(),
                Some(*value),
                "case {case:?}: the job reports the parameter {name} it was submitted with, \
                 got {detail}"
            );
        }
    }
}

/// RULE (the same one, over ONE definition): two jobs submitted from the SAME job definition
/// each run with their OWN effective parameters, so the placeholder the definition declares is
/// filled per job and neither job runs the other's words.
#[tokio::test]
async fn two_jobs_of_one_definition_run_their_own_parameters() {
    let case = "one definition, several jobs";
    let router = router();
    let network = network_of(&router).await;
    batch(
        &router,
        case,
        "/v1/createcomputeenvironment",
        &json!({
            "computeEnvironmentName": "ce-shared-definition",
            "type": "MANAGED",
            "computeResources": {
                "type": "FARGATE",
                "maxvCpus": 16,
                "subnets": [&network.subnet],
                "securityGroupIds": [&network.security_group],
            },
        }),
    )
    .await;
    until_environment_valid(&router, case, "ce-shared-definition").await;
    batch(
        &router,
        case,
        "/v1/createjobqueue",
        &json!({
            "jobQueueName": "jq-shared-definition",
            "priority": 1,
            "computeEnvironmentOrder": [
                {"order": 1, "computeEnvironment": "ce-shared-definition"},
            ],
        }),
    )
    .await;
    until_queue_valid(&router, case, "jq-shared-definition").await;

    let registered = batch(
        &router,
        case,
        "/v1/registerjobdefinition",
        &json!({
            "jobDefinitionName": "jd-shared",
            "type": "container",
            "platformCapabilities": ["FARGATE"],
            "parameters": {"code": "0"},
            "containerProperties": {
                "image": "busybox",
                "command": ["sh", "-c", "exit Ref::code"],
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batchExecutionRole"),
                "resourceRequirements": [
                    {"type": "VCPU", "value": "0.25"},
                    {"type": "MEMORY", "value": "512"},
                ],
                "networkConfiguration": {"assignPublicIp": "ENABLED"},
            },
        }),
    )
    .await;
    let definition = registered["jobDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {registered}"))
        .to_string();

    // (the `code` the submit overrides with, the status the program must exit with) — the job
    // that takes the definition's default, and two that override it.
    let rows: &[(Option<i64>, i64)] = &[(None, 0), (Some(4), 4), (Some(0), 0)];
    let mut submitted: Vec<(i64, String)> = Vec::new();
    for (index, (override_code, exit_code)) in rows.iter().enumerate() {
        let mut request = json!({
            "jobName": format!("shared-{index}"),
            "jobQueue": "jq-shared-definition",
            "jobDefinition": &definition,
        });
        if let Some(code) = override_code {
            request["parameters"] = json!({"code": code.to_string()});
        }
        let answered = batch(&router, case, "/v1/submitjob", &request).await;
        let job_id = answered["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {answered}"))
            .to_string();
        submitted.push((*exit_code, job_id));
    }

    for (exit_code, job_id) in submitted {
        let detail = until_terminal(&router, case, &job_id).await;
        let expected_status = if exit_code == 0 { SUCCEEDED } else { FAILED };
        assert_eq!(
            detail["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected {expected_status} for the job whose parameter is \
             {exit_code}, got {detail}"
        );
        assert_eq!(
            detail["container"]["exitCode"].as_i64(),
            Some(exit_code),
            "case {case:?}: expected the program to exit {exit_code}, got {detail}"
        );
    }
}
