// SPDX-License-Identifier: CC0-1.0

//! A container definition's `secrets`, driven end to end over the wire with AWS Secrets
//! Manager, IAM and CloudWatch Logs mounted beside Amazon ECS on ONE shared engine — so
//! every secret, role, policy and log group is created through the OWNING service's own API
//! rather than written into a store by hand.
//!
//! RULE: Amazon ECS retrieves every `secrets` `valueFrom` of a task's containers as the task
//! definition's EXECUTION role, before any container of the task exists, and exposes what it
//! retrieved to the container's process as the environment variable the entry's `name`
//! spells. So:
//!
//! * a launch whose EXECUTION role may retrieve the secret runs: the container's program sees
//!   the value Secrets Manager holds — the whole value, or the one JSON key the reference
//!   selects — writes it to the `awslogs` stream its log configuration names, and the
//!   container reports the status it exited with;
//! * a launch whose execution role may NOT retrieve it launches nothing: the task never
//!   reports `RUNNING`, it reaches `STOPPED` with `Task$stopCode` `TaskFailedToStart` and a
//!   `stoppedReason` of the agent's `ResourceInitializationError` class naming the refusal,
//!   its container reports no `exitCode` — nothing ran to produce one — and nothing is
//!   delivered to the log group its configuration names.
//!
//! The grant on the TASK role is not the grant that decides it: that role is what the
//! container's own calls are signed with, and the secret is retrieved before the container
//! exists.
//!
//! The rows differ along what the rule generalises over: WHICH role carries the grant (the
//! execution role, the task role, both, neither), how its statement is scoped (the secret's
//! own ARN, a wildcard), the effect it names (`Allow`, `Allow` beside an explicit `Deny`),
//! which member references the secret (a container's `secrets`, a log configuration's
//! `secretOptions`), the form of the reference (the secret's ARN, a JSON key within it),
//! whether the account holds the secret at all, and whether IAM holds the execution role at
//! all — plus, for the running rows, the environment variable the value is exposed under.
//!
//! Nothing here reaches into any state struct: every value asserted is read back through
//! `DescribeTasks` and CloudWatch Logs' own `GetLogEvents`/`DescribeLogStreams`, and the
//! secret's value is the one Secrets Manager's own `CreateSecret` stored. The `stoppedReason`
//! class is spelled out here rather than imported, so a wrong wording in the implementation
//! cannot make a test pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_cluster, iam, list, ok, present_count,
    seed_awsvpc_subnets, string, ACCOUNT, SIGNED_REGION,
};

/// The log group every container below delivers to, created through CloudWatch Logs' own API
/// before any task is launched.
const LOG_GROUP: &str = "/emulator/ecs/container-secrets";

/// The `awslogs-stream-prefix` the task definitions below configure, and so the first segment
/// of every stream name they deliver to.
const STREAM_PREFIX: &str = "ecs";

/// The one container of every task definition below.
const CONTAINER: &str = "app";

/// The image the containers start from: a reference naming a registry this emulator answers
/// nothing for, so the image fetch decides nothing in this suite.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// `Task$lastStatus` of a task that is running, and of one that has stopped.
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task Amazon ECS could not launch the containers of at all
/// (`smithy_query(query="shape",shape="TaskStopCode")` -> EssentialContainerExited,
/// InfrastructureHealth, ServiceSchedulerInitiated, SpotInterruption, TaskFailedToStart,
/// TerminationNotice, UserInitiated).
const TASK_FAILED_TO_START: &str = "TaskFailedToStart";

/// The fixed text of the `stoppedReason` the agent words for an execution resource it could
/// not retrieve from Secrets Manager, spelled out here rather than imported.
const RESOURCE_INITIALIZATION_ERROR: &str =
    "ResourceInitializationError: unable to pull secrets or registry auth: execution resource \
     retrieval failed: unable to retrieve secret from asm: service call has been retried 1 \
     time(s): ";

/// The IAM action a retrieval of a secret's value needs.
const GET_SECRET_VALUE: &str = "secretsmanager:GetSecretValue";

/// How often a poll re-reads the emulator.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a poll keeps trying before it gives up: comfortably past every clock-owed
/// transition a task takes to come up, run its work and stop (`state::CONTAINER_WORK_SPAN` is
/// 10 s), with generous margin for a loaded test machine.
const POLL_BUDGET: Duration = Duration::from_secs(90);

/// A router carrying Amazon ECS, AWS Secrets Manager — which holds the secret and the
/// policies on it — IAM, which holds the roles the retrieval is decided against, and
/// CloudWatch Logs, which the container's output is delivered to, over ONE shared engine,
/// mounted the way the production router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // A Fargate task is allocated its own elastic network interface in a subnet of the
    // account, so the placement below resolves against Amazon VPC's records.
    seed_awsvpc_subnets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call to another service of the router, the way a client makes it: the
/// target prefix and the credential scope are what pick the service.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{SIGNED_REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECSTEST/20260101/{SIGNED_REGION}\
                 /{signing_name}/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One CloudWatch Logs call.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    json_call(router, "logs", &format!("Logs_20140328.{operation}"), body).await
}

/// One Secrets Manager call that must succeed, returning its body.
async fn secrets(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) =
        json_call(router, "secretsmanager", &format!("secretsmanager.{operation}"), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Secrets Manager {operation} to answer 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// Create the log group every container of this suite delivers to.
async fn create_log_group(router: &MultiServiceRouter, case: &str) {
    let (status, body) = logs(router, "CreateLogGroup", json!({ "logGroupName": LOG_GROUP })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLogGroup to answer 200, got {status} with {body}"
    );
}

/// Create the secret `name` holding `value` through Secrets Manager's OWN API, and answer the
/// ARN it allocated — the ARN a policy statement and a `valueFrom` name it by.
async fn create_secret(router: &MultiServiceRouter, name: &str, value: &str, case: &str) -> String {
    // A write that stores a value creates a version the caller names, and the modelled
    // `ClientRequestToken` is at least 32 characters long.
    let mut token = format!("{name}-version-");
    while token.len() < 32 {
        token.push('0');
    }
    let created = secrets(
        router,
        "CreateSecret",
        json!({
            "Name": name,
            "SecretString": value,
            "ClientRequestToken": token,
        }),
        case,
    )
    .await;
    string(&created, "ARN", case)
}

/// The messages CloudWatch Logs holds in the stream one task's container delivers to, oldest
/// first. An unknown stream reads as no messages, which is what a stream nothing was
/// delivered to holds.
async fn delivered(router: &MultiServiceRouter, task_arn: &str) -> Vec<String> {
    let (status, body) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": LOG_GROUP,
            "logStreamName": stream_of(task_arn),
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The streams the log group holds for one task's container — none for a task whose container
/// never started.
async fn streams_of(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Vec<String> {
    let (status, body) = logs(
        router,
        "DescribeLogStreams",
        json!({
            "logGroupName": LOG_GROUP,
            "logStreamNamePrefix": stream_of(task_arn),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeLogStreams to answer 200, got {status} with {body}"
    );
    body["logStreams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| stream["logStreamName"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The stream the `awslogs` driver delivers one container of one task to:
/// `<prefix>/<container name>/<task id>`, the naming the driver's own
/// `awslogs-stream-prefix` documentation states. Spelled out here rather than imported.
fn stream_of(task_arn: &str) -> String {
    format!("{STREAM_PREFIX}/{CONTAINER}/{}", task_id_of(task_arn))
}

/// The task id: the last segment of a task's ARN.
fn task_id_of(task_arn: &str) -> String {
    task_arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A trust policy that lets this service's own service principal assume the role, which is
/// what Amazon ECS has to do before it can act under it.
fn ecs_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": ["ecs-tasks.amazonaws.com"] },
            "Action": ["sts:AssumeRole"],
        }],
    })
    .to_string()
}

/// An identity-based policy with one statement per `(effect, resource)` pair, each about the
/// action a retrieval of a secret's value needs.
fn secret_policy(statements: &[(&str, &str)]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": statements
            .iter()
            .map(|(effect, resource)| json!({
                "Effect": effect,
                "Action": [GET_SECRET_VALUE],
                "Resource": [resource],
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// Create the IAM role `name` trusting this service, carrying `document` as its inline policy
/// when the row gives one — each step through IAM's OWN API — and answer the role's ARN.
async fn role(
    router: &MultiServiceRouter,
    name: &str,
    document: Option<String>,
    case: &str,
) -> String {
    iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", ecs_trust()),
        ],
        case,
    )
    .await;
    if let Some(document) = document {
        iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "secret-access".to_string()),
                ("PolicyDocument", document),
            ],
            case,
        )
        .await;
    }
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Which member of the revision references the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// `ContainerDefinition$secrets`: the value is exposed to the container's process as an
    /// environment variable.
    ContainerSecrets,
    /// `LogConfiguration$secretOptions`: "the secrets to pass to the log configuration",
    /// retrieved under the same role before the container starts.
    LogSecretOptions,
}

/// Register one revision of `family` whose single container references `value_from` through
/// the row's own member, exposing it under the row's environment variable, and runs a program
/// that writes that variable to its `awslogs` stream.
///
/// `roles` is the revision's `(executionRoleArn, taskRoleArn)` — the two roles the rule is
/// about.
async fn register(
    router: &MultiServiceRouter,
    family: &str,
    roles: (&str, &str),
    case: &Case,
    value_from: &str,
) {
    let (execution_role_arn, task_role_arn) = roles;
    let variable = case.variable;
    let entry = json!({ "name": variable, "valueFrom": value_from });
    let mut logging = json!({
        "logDriver": "awslogs",
        "options": {
            "awslogs-group": LOG_GROUP,
            "awslogs-region": SIGNED_REGION,
            "awslogs-stream-prefix": STREAM_PREFIX,
        },
    });
    let mut container = json!({
        "name": CONTAINER,
        "image": IMAGE,
        "command": ["sh", "-c", format!("echo \"injected=${variable}\"")],
    });
    match case.reference {
        Reference::ContainerSecrets => container["secrets"] = json!([entry]),
        Reference::LogSecretOptions => logging["secretOptions"] = json!([entry]),
    }
    container["logConfiguration"] = logging;
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            // A documented Fargate task size: 256 CPU units, 512 MiB.
            "cpu": "256",
            "memory": "512",
            "executionRoleArn": execution_role_arn,
            "taskRoleArn": task_role_arn,
            "containerDefinitions": [container],
        }),
        case.name,
    )
    .await;
}

/// Launch one standalone Fargate task of `family` in `cluster` and answer its ARN.
async fn run_task(
    router: &MultiServiceRouter,
    cluster: &str,
    family: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RunTask",
        json!({
            "cluster": cluster,
            "taskDefinition": family,
            "launchType": "FARGATE",
            "networkConfiguration": awsvpc_network_configuration(),
        }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected RunTask to place one task, got {body}"
    );
    string(&tasks[0], "taskArn", case)
}

/// The task `task_arn` as `DescribeTasks` reports it now.
async fn describe(router: &MultiServiceRouter, cluster: &str, task_arn: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeTasks",
        json!({ "cluster": cluster, "tasks": [task_arn] }),
        case,
    )
    .await;
    let tasks = list(&body, "tasks", case);
    assert_eq!(
        tasks.len(),
        1,
        "case {case:?}: expected DescribeTasks to describe {task_arn}, got {body}"
    );
    tasks[0].clone()
}

/// Poll one task until its `lastStatus` is `STOPPED`, answering that record and every status
/// the poll observed on the way — which is how a row says a task never reported `RUNNING`.
async fn poll_to_stopped(
    router: &MultiServiceRouter,
    cluster: &str,
    task_arn: &str,
    case: &str,
) -> (Value, Vec<String>) {
    let deadline = Instant::now() + POLL_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let task = describe(router, cluster, task_arn, case).await;
        let status = string(&task, "lastStatus", case);
        if !seen.contains(&status) {
            seen.push(status.clone());
        }
        if status == STOPPED {
            return (task, seen);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to reach {STOPPED}, it was still {status:?} after \
             {seen:?}: {task}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll the stream this task's container delivers to until it holds a line, and answer the
/// lines it holds.
async fn poll_for_output(router: &MultiServiceRouter, task_arn: &str, case: &str) -> Vec<String> {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let events = delivered(router, task_arn).await;
        if !events.is_empty() {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a line in {}, the stream held none",
            stream_of(task_arn)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Where the grant for the secret sits, which is the whole of what the rule turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// On the EXECUTION role, scoped to the secret's own ARN.
    Execution,
    /// On the EXECUTION role, scoped to every resource — how AWS's own task-execution policy
    /// states a grant that is not narrowed to one secret.
    ExecutionWildcard,
    /// On BOTH roles: the execution role's is the one that decides, so the task runs.
    BothRoles,
    /// On the EXECUTION role, with an explicit `Deny` about the same secret beside it.
    ExecutionWithDeny,
    /// On the TASK role alone — the misconfiguration the rule is about.
    TaskRoleOnly,
    /// On neither role.
    Neither,
    /// On the task role, with an `executionRoleArn` naming a role IAM holds no record for:
    /// nothing is shown about that role, so nothing is refused on it.
    UnheldExecutionRole,
}

impl Grant {
    /// The inline policy each role carries under this grant, scoped to `secret_arn`.
    fn documents(self, secret_arn: &str) -> (Option<String>, Option<String>) {
        let allow = || Some(secret_policy(&[("Allow", secret_arn)]));
        match self {
            Self::Execution => (allow(), None),
            Self::ExecutionWildcard => (Some(secret_policy(&[("Allow", "*")])), None),
            Self::BothRoles => (allow(), allow()),
            Self::ExecutionWithDeny => (
                Some(secret_policy(&[("Allow", secret_arn), ("Deny", secret_arn)])),
                None,
            ),
            Self::TaskRoleOnly | Self::UnheldExecutionRole => (None, allow()),
            Self::Neither => (None, None),
        }
    }
}

/// One row of either table.
struct Case {
    name: &'static str,
    /// Where the grant for the secret sits.
    grant: Grant,
    /// Which member of the revision references the secret.
    reference: Reference,
    /// The environment variable the entry exposes the value under.
    variable: &'static str,
    /// The value the secret holds, as `CreateSecret` stored it.
    secret_value: &'static str,
    /// What is written after the secret's ARN in the `valueFrom`: the JSON key and the
    /// version selectors, empty for a reference to the whole value.
    selector: &'static str,
    /// Whether the account holds the secret at all.
    held: bool,
}

/// The rows whose execution role MAY retrieve the secret, so the task runs and the value
/// reaches the container.
const RUNS: &[(Case, &str)] = &[
    (
        Case {
            name: "the grant on the execution role",
            grant: Grant::Execution,
            reference: Reference::ContainerSecrets,
            variable: "DATABASE_PASSWORD",
            secret_value: "s3cr3t-value",
            selector: "",
            held: true,
        },
        "s3cr3t-value",
    ),
    (
        Case {
            name: "a wildcard resource on the execution role",
            grant: Grant::ExecutionWildcard,
            reference: Reference::ContainerSecrets,
            variable: "API_TOKEN",
            secret_value: "{\"not\":\"selected\"}",
            selector: "",
            held: true,
        },
        "{\"not\":\"selected\"}",
    ),
    (
        Case {
            name: "the grant on both roles",
            grant: Grant::BothRoles,
            reference: Reference::ContainerSecrets,
            variable: "SHARED_VALUE",
            secret_value: "value-for-both",
            selector: "",
            held: true,
        },
        "value-for-both",
    ),
    (
        Case {
            name: "a JSON key within the secret",
            grant: Grant::Execution,
            reference: Reference::ContainerSecrets,
            variable: "ONLY_THIS_KEY",
            secret_value: "{\"username\":\"admin\",\"password\":\"only-this\"}",
            selector: ":password",
            held: true,
        },
        "only-this",
    ),
    (
        Case {
            name: "an execution role IAM holds no record for",
            grant: Grant::UnheldExecutionRole,
            reference: Reference::ContainerSecrets,
            variable: "UNDECIDED",
            secret_value: "nothing-was-shown",
            selector: "",
            held: true,
        },
        "nothing-was-shown",
    ),
];

/// RULE, the running half: a `secrets` entry the EXECUTION role may retrieve is exposed to
/// the container's process under the entry's own name, with the value Secrets Manager holds —
/// the whole value, or the one JSON key the reference selects.
#[tokio::test]
async fn a_secret_the_execution_role_may_retrieve_reaches_the_container() {
    for (index, (case, expected)) in RUNS.iter().enumerate() {
        let name = case.name;
        // A fresh emulator per row, so one row's secrets and policies cannot decide another's.
        let router = emulator();
        let cluster = "secrets";
        create_cluster(&router, cluster, name).await;
        create_log_group(&router, name).await;

        let secret_arn =
            create_secret(&router, &format!("injected-{index}"), case.secret_value, name).await;
        let (execution_document, task_document) = case.grant.documents(&secret_arn);
        let execution_role_arn = match case.grant {
            // A role IAM holds nothing for is named without being created.
            Grant::UnheldExecutionRole => format!("arn:aws:iam::{ACCOUNT}:role/unheld-{index}"),
            _ => {
                role(
                    &router,
                    &format!("exec-{index}"),
                    execution_document,
                    name,
                )
                .await
            }
        };
        let task_role_arn = role(&router, &format!("task-{index}"), task_document, name).await;

        let family = format!("secretruns{index}");
        register(
            &router,
            &family,
            (&execution_role_arn, &task_role_arn),
            case,
            &format!("{secret_arn}{}", case.selector),
        )
        .await;
        let task_arn = run_task(&router, cluster, &family, name).await;

        // The container ran, and what it wrote is the value the secret holds.
        let events = poll_for_output(&router, &task_arn, name).await;
        assert_eq!(
            events,
            vec![format!("injected={expected}")],
            "case {name:?}: expected the container to write injected={expected} to {}, got \
             {events:?}",
            stream_of(&task_arn)
        );

        // The task reaches a terminal status, and the container reports the status its
        // program ended with.
        let (task, seen) = poll_to_stopped(&router, cluster, &task_arn, name).await;
        assert!(
            seen.contains(&RUNNING.to_string()),
            "case {name:?}: expected the task to report {RUNNING} on its way, saw {seen:?}"
        );
        let containers = list(&task, "containers", name);
        let exit_code = present_count(&containers[0], "exitCode", name);
        assert_eq!(
            exit_code,
            Some(0),
            "case {name:?}: expected exitCode 0, got {exit_code:?} in {task}"
        );
        assert_eq!(
            task["stopCode"], json!("EssentialContainerExited"),
            "case {name:?}: expected the stop of a task that RAN, got {task}"
        );
    }
}

/// The rows whose execution role may NOT retrieve the secret, so the task fails to start.
///
/// Each row's `class` is the error class Secrets Manager answers the retrieval with, and
/// `clause` which of IAM's rules withheld the grant — absent for the row that is refused
/// because nothing holds the secret.
struct Refused {
    case: Case,
    /// The error class the `stoppedReason` carries.
    class: &'static str,
    /// The clause naming which rule withheld the grant, when a policy is what withheld it.
    clause: Option<&'static str>,
}

const REFUSALS: &[Refused] = &[
    Refused {
        case: Case {
            name: "the grant on the task role instead of the execution role",
            grant: Grant::TaskRoleOnly,
            reference: Reference::ContainerSecrets,
            variable: "DATABASE_PASSWORD",
            secret_value: "never-injected",
            selector: "",
            held: true,
        },
        class: "AccessDeniedException",
        clause: Some("because no identity-based policy allows the secretsmanager:GetSecretValue \
                      action"),
    },
    Refused {
        case: Case {
            name: "no grant on either role",
            grant: Grant::Neither,
            reference: Reference::ContainerSecrets,
            variable: "API_TOKEN",
            secret_value: "never-injected",
            selector: "",
            held: true,
        },
        class: "AccessDeniedException",
        clause: Some("because no identity-based policy allows the secretsmanager:GetSecretValue \
                      action"),
    },
    Refused {
        case: Case {
            name: "an explicit deny beside the allow on the execution role",
            grant: Grant::ExecutionWithDeny,
            reference: Reference::ContainerSecrets,
            variable: "DENIED_VALUE",
            secret_value: "never-injected",
            selector: "",
            held: true,
        },
        class: "AccessDeniedException",
        clause: Some("with an explicit deny in an identity-based policy"),
    },
    Refused {
        case: Case {
            name: "a JSON key of a secret the execution role may not retrieve",
            grant: Grant::TaskRoleOnly,
            reference: Reference::ContainerSecrets,
            variable: "ONLY_THIS_KEY",
            secret_value: "{\"password\":\"never-injected\"}",
            selector: ":password",
            held: true,
        },
        class: "AccessDeniedException",
        clause: Some("because no identity-based policy allows the secretsmanager:GetSecretValue \
                      action"),
    },
    Refused {
        case: Case {
            name: "a log configuration's secretOptions the execution role may not retrieve",
            grant: Grant::TaskRoleOnly,
            reference: Reference::LogSecretOptions,
            variable: "log-token",
            secret_value: "never-injected",
            selector: "",
            held: true,
        },
        class: "AccessDeniedException",
        clause: Some("because no identity-based policy allows the secretsmanager:GetSecretValue \
                      action"),
    },
    Refused {
        case: Case {
            name: "a secret the account does not hold",
            grant: Grant::Execution,
            reference: Reference::ContainerSecrets,
            variable: "ABSENT_SECRET",
            secret_value: "never-created",
            selector: "",
            held: false,
        },
        class: "ResourceNotFoundException",
        clause: None,
    },
];

/// RULE, the refusing half: a `secrets` entry the EXECUTION role may not retrieve launches no
/// container — the task never reports `RUNNING`, it stops under `TaskFailedToStart` with the
/// agent's `ResourceInitializationError`, its container reports no `exitCode`, and the log
/// group its configuration names receives no stream.
#[tokio::test]
async fn a_secret_the_execution_role_may_not_retrieve_fails_the_task_at_start() {
    for (index, row) in REFUSALS.iter().enumerate() {
        let case = &row.case;
        let name = case.name;
        let router = emulator();
        let cluster = "secrets";
        create_cluster(&router, cluster, name).await;
        create_log_group(&router, name).await;

        let secret_name = format!("refused-{index}");
        let secret_arn = if case.held {
            create_secret(&router, &secret_name, case.secret_value, name).await
        } else {
            // The ARN a secret of that name would carry, which the account holds nothing
            // under: it is what the reference names and what the refusal reports.
            format!("arn:aws:secretsmanager:{SIGNED_REGION}:{ACCOUNT}:secret:{secret_name}-AbCdEf")
        };
        let (execution_document, task_document) = case.grant.documents(&secret_arn);
        let execution_role_name = format!("exec-{index}");
        let execution_role_arn =
            role(&router, &execution_role_name, execution_document, name).await;
        let task_role_arn = role(&router, &format!("task-{index}"), task_document, name).await;

        let family = format!("secretrefusals{index}");
        register(
            &router,
            &family,
            (&execution_role_arn, &task_role_arn),
            case,
            &format!("{secret_arn}{}", case.selector),
        )
        .await;
        let task_arn = run_task(&router, cluster, &family, name).await;

        let (task, seen) = poll_to_stopped(&router, cluster, &task_arn, name).await;
        assert!(
            !seen.contains(&RUNNING.to_string()),
            "case {name:?}: a task whose secrets could not be retrieved never runs, saw {seen:?}"
        );
        assert_eq!(
            task["stopCode"],
            json!(TASK_FAILED_TO_START),
            "case {name:?}: expected stopCode {TASK_FAILED_TO_START}, got {task}"
        );
        let reason = string(&task, "stoppedReason", name);
        assert!(
            reason.starts_with(RESOURCE_INITIALIZATION_ERROR),
            "case {name:?}: expected a reason beginning {RESOURCE_INITIALIZATION_ERROR:?}, got \
             {reason:?}"
        );
        assert!(
            reason.contains(row.class),
            "case {name:?}: expected the reason to name {}, got {reason:?}",
            row.class
        );
        if let Some(clause) = row.clause {
            // The refusal names the principal the retrieval was made by — a session of the
            // EXECUTION role, named after the task it was made for — the action, the secret
            // and the rule that withheld the grant.
            let session_arn = format!(
                "arn:aws:sts::{ACCOUNT}:assumed-role/{execution_role_name}/{}",
                task_id_of(&task_arn)
            );
            let refusal = format!(
                "User: {session_arn} is not authorized to perform: {GET_SECRET_VALUE} on \
                 resource: {secret_arn} {clause}"
            );
            assert!(
                reason.contains(&refusal),
                "case {name:?}: expected the reason to carry {refusal:?}, got {reason:?}"
            );
        }
        // No container started, so none reports a status it exited with.
        let containers = list(&task, "containers", name);
        let exit_code = present_count(&containers[0], "exitCode", name);
        assert_eq!(
            exit_code, None,
            "case {name:?}: a container that never ran reports no exitCode, got \
             {exit_code:?} in {task}"
        );
        // And nothing was delivered: the driver creates the stream when the container
        // starts, and this container never did.
        let streams = streams_of(&router, &task_arn, name).await;
        assert!(
            streams.is_empty(),
            "case {name:?}: expected no stream in {LOG_GROUP} for this task, got {streams:?}"
        );
    }
}
