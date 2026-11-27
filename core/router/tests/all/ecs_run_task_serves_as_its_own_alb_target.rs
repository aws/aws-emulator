// SPDX-License-Identifier: CC0-1.0

//! A task Amazon ECS placed with `RunTask` — no service scheduler behind it — whose essential
//! container runs a web server, and the three answers an Application Load Balancer gives for the
//! address that task holds.
//!
//! RULE: **`Task$stopCode` `EssentialContainerExited` states that an essential container EXITED,
//! and `Container$exitCode` is "the exit code returned from the container" — the record of one.
//! So a standalone task whose essential container is still PERFORMING its program has had no
//! container exit: it is reported `RUNNING`, with no `stopCode` and no `stoppedReason`, until
//! something stops it. A command that runs a server in the FOREGROUND is such a program — it does
//! not return — so the task goes on running and the server it started answers at the task's own
//! address, which is what an `ip` registration naming that address is checked and forwarded to.
//! Only a program that really ended with a status stops the task, and that stop is the one whose
//! code names an essential container's exit.**
//!
//! `Container$exitCode` ("the exit code returned from the container"), `Task$stopCode`
//! (`smithy_query(query="shape",shape="TaskStopCode")` -> `EssentialContainerExited`,
//! `TaskFailedToStart`, `UserInitiated`, ...) and `Task$lastStatus` ("the last known status for
//! the task") are the members read back; `ContainerDefinition$command` is "the command that's
//! passed to the container", and `ContainerOverride$command` names what a revision that writes
//! none leaves in place: "the default command from the Docker image".
//!
//! The rows span what the rule generalises over — WHICH foreground server the container's command
//! runs (the server its own image ships, invoked by the revision and left to the image's own
//! declared command, the interpreter's own HTTP server module, and the small multi-call binary's
//! server applet) — and the contrast is a program that really ends, which stops the task with the
//! code that names an essential container's exit and the sentence Amazon ECS words for it. Each
//! row's server is read through the load balancer at its own task's address, and one of them is
//! then left registered for the whole chain of answers a client gets as its health moves.
//!
//! It lives beside the router because four services have to agree: Amazon ECS runs the container
//! and records the server at the task's address, EC2 owns the network and the rule that lets a
//! check arrive, CloudWatch Logs owns the group the container's `awslogs` driver delivers into,
//! IAM holds the execution role that delivery is performed as, and Elastic Load Balancing v2
//! checks the registration and answers the client. Every fact is created through the owning
//! service's own API and every value asserted is read back through one.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element readers this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{
    element, elements, form_encode, members_of, opt_element, target_health_of,
};
// The ONE network fixture for "an Application Load Balancer in front of an `awsvpc` Amazon ECS
// workload", and the ONE in-process request and raw HTTP/1.1 exchange on a load balancer's own
// published address, from the suite that owns them.
use crate::ecs_alb_serves_its_awsvpc_tasks_container::{dialled, get, seed_network, Network};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The API versions an awsQuery request to each model carries.
const ELB_VERSION: &str = "2015-12-01";
const IAM_VERSION: &str = "2010-05-08";

/// The status every control-plane call of the setup must answer.
const OK: u16 = 200;

/// The cluster every task below is placed in, and the one container of every revision.
const CLUSTER: &str = "standalone";
const CONTAINER: &str = "app";

/// The port every server below listens on, which is the `containerPort` its revision declares
/// and the port the registration receives traffic on.
const CONTAINER_PORT: i64 = 80;

/// The log group every container's `awslogs` driver delivers into, and the stream prefix it
/// names.
const LOG_GROUP: &str = "/emulator/ecs/standalone";
const STREAM_PREFIX: &str = "ecs";

/// The service principal Amazon ECS assumes a task's roles as, the action that takes, and the
/// actions the `awslogs` driver performs under the execution role.
const ECS_TASKS_PRINCIPAL: &str = "ecs-tasks.amazonaws.com";
const ASSUME_ROLE: &str = "sts:AssumeRole";
const CREATE_LOG_STREAM: &str = "logs:CreateLogStream";
const PUT_LOG_EVENTS: &str = "logs:PutLogEvents";

/// `Task$lastStatus` of a task whose containers are running, and of one that has stopped.
const RUNNING: &str = "RUNNING";
const STOPPED: &str = "STOPPED";

/// `Task$stopCode` of a task an essential container's exit stopped, and the `Task$stoppedReason`
/// Amazon ECS words for it — both spelled out here rather than imported, so a wrong constant in
/// the implementation cannot make a test pass.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";
const ESSENTIAL_CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// The `TargetHealth$State` values this suite reads back, and the reason an answer the group's
/// matcher rejects carries.
const HEALTHY: &str = "healthy";
const UNHEALTHY: &str = "unhealthy";
const RESPONSE_CODE_MISMATCH: &str = "Target.ResponseCodeMismatch";

/// The path every server below holds a document at, and one it holds nothing at — which the
/// group's health check is pointed at first.
const SERVED_PATH: &str = "/";
const MISSING_PATH: &str = "/not-deployed";

/// The statuses the client is answered with: the target's own for the document it holds, and the
/// load balancer's own for a group it has no target to send the request to.
const SERVED_STATUS: u16 = 200;
const NO_TARGET_STATUS: u16 = 503;

/// The status a static server answers a path its document root holds nothing at, which is what
/// the health check's first destination earns.
const NOT_FOUND_STATUS: u16 = 404;

/// How long a step waits for a clock-owed move — the task's start, the verdict the group's
/// cadence reaches — and how often it looks. Generous on purpose: the deadline is only here so a
/// failure is a failure rather than a hang, and this suite runs beside every other test of the
/// workspace.
const DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(50);

/// How long a window in which an answer must HOLD is looked at: the tasks' run, past the span
/// after which a standalone task whose essential container HAS exited is stopped — so a task that
/// keeps running is observed keeping it rather than read once before anything could have stopped
/// it — and the load balancer's answer, over a window the health checks of its group fall due
/// several times inside, so an answer that only held between two checks is not read as the
/// answer. Each window is looked at by LOOKING AGAIN, never by waiting a fixed time and reading
/// once.
const RUN_WINDOW: Duration = Duration::from_secs(5);
const ANSWER_WINDOW: Duration = Duration::from_secs(2);

/// One row: a foreground server a container's command starts, and the document it serves.
struct Row {
    case: &'static str,
    /// The family the revision is registered under, so no row's revision is another's.
    family: &'static str,
    /// The image the container runs.
    image: &'static str,
    /// The command the revision declares, and `None` for a revision that declares none — which
    /// leaves the command the IMAGE declares to run.
    command: Option<&'static [&'static str]>,
    /// The marker the document the server answers `/` with carries.
    marker: &'static str,
}

/// The programs this suite drives, each a server that does not return.
///
/// The document each row serves is written by the container's own program, except the row that
/// declares no command at all: that one runs the command its image declares and answers with the
/// page the image ships at its document root.
const ROWS: &[Row] = &[
    Row {
        case: "the image's own server, invoked in the foreground by the revision's command",
        family: "standaloneNginx",
        image: "public.ecr.aws/docker/library/nginx:alpine",
        command: Some(&[
            "/bin/sh",
            "-c",
            "echo STANDALONE-NGINX > /usr/share/nginx/html/index.html && nginx -g 'daemon off;'",
        ]),
        marker: "STANDALONE-NGINX",
    },
    Row {
        case: "the other image's foreground wrapper, which its own documentation runs",
        family: "standaloneHttpd",
        image: "public.ecr.aws/docker/library/httpd:alpine",
        command: Some(&[
            "/bin/sh",
            "-c",
            "echo STANDALONE-HTTPD > /usr/local/apache2/htdocs/index.html && httpd-foreground",
        ]),
        marker: "STANDALONE-HTTPD",
    },
    Row {
        case: "no command at all, which leaves the one the image declares",
        family: "standaloneDeclared",
        image: "public.ecr.aws/docker/library/nginx:alpine",
        command: None,
        // The page nginx's own distribution ships at its document root, which a container that
        // ran that image's server and wrote nothing of its own answers with.
        marker: "Welcome to nginx!",
    },
    Row {
        case: "the interpreter's own HTTP server module, over a directory the program wrote",
        family: "standaloneModule",
        image: "public.ecr.aws/docker/library/python:alpine",
        command: Some(&[
            "/bin/sh",
            "-c",
            "mkdir -p /srv/pages && echo STANDALONE-MODULE > /srv/pages/index.html && \
             python3 -m http.server 80 --directory /srv/pages",
        ]),
        marker: "STANDALONE-MODULE",
    },
    Row {
        case: "the multi-call binary's server applet, serving the directory it was given",
        family: "standaloneApplet",
        image: "public.ecr.aws/docker/library/busybox:latest",
        command: Some(&[
            "/bin/sh",
            "-c",
            "mkdir -p /www && echo STANDALONE-APPLET > /www/index.html && \
             busybox httpd -f -p 80 -h /www",
        ]),
        marker: "STANDALONE-APPLET",
    },
];

/// The row left registered once every row's server has been read through the load balancer: the
/// small multi-call binary's server applet, serving the directory the container's own program made
/// and wrote, whose registration the whole front-door chain below is then driven over.
const FRONTED: usize = 4;

/// The marker the document that row's server answers with carries — the one an answer the LOAD
/// BALANCER produced itself must not carry.
const FRONTED_MARKER: &str = ROWS[FRONTED].marker;

/// One emulator carrying EC2, Elastic Load Balancing v2, Amazon ECS, IAM — which holds the
/// execution role the log delivery is performed as — and CloudWatch Logs, over ONE shared engine,
/// exactly as the production router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
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

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is what
/// routes the request; the signature is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Drive one form-encoded action at the service `signing_name` addresses, which must succeed, and
/// answer its body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    action: &str,
    version: &str,
    params: &[(&str, &str)],
) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(form(action, version, params)))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, OK,
        "setup: {signing_name} {action} must succeed, got {status} with body {body}"
    );
    body
}

/// Drive one awsQuery action against the ELBv2 model that must succeed, and answer its body.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    query(
        router,
        "elasticloadbalancingv2",
        action,
        ELB_VERSION,
        params,
    )
    .await
}

/// Drive one awsQuery action against IAM that must succeed, and answer its body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    query(router, "iam", action, IAM_VERSION, params).await
}

/// One `awsJson1_1` call, answered with its HTTP status and parsed body.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", target)
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One Amazon ECS call that must succeed, answered with its parsed body.
async fn ecs(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) = json_call(
        router,
        "ecs",
        &format!("AmazonEC2ContainerServiceV20141113.{operation}"),
        body,
    )
    .await;
    assert_eq!(
        status, OK,
        "case {case:?}: expected ecs {operation} to answer {OK}, got {status} with {parsed}"
    );
    parsed
}

/// One CloudWatch Logs call that must succeed, answered with its parsed body.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, parsed) =
        json_call(router, "logs", &format!("Logs_20140328.{operation}"), body).await;
    assert_eq!(
        status, OK,
        "case {case:?}: expected logs {operation} to answer {OK}, got {status} with {parsed}"
    );
    parsed
}

/// Create the execution role the `awslogs` driver's calls are made as, with a trust policy that
/// lets Amazon ECS assume it and the delivery actions it performs, and answer the ARN IAM minted
/// for it — read out of IAM's own answer rather than built here.
async fn execution_role(router: &MultiServiceRouter) -> String {
    let name = "standaloneExecutionRole";
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": [ECS_TASKS_PRINCIPAL] },
            "Action": [ASSUME_ROLE],
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    let arn = element(&created, "Arn");
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [CREATE_LOG_STREAM, PUT_LOG_EVENTS],
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "delivery"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    arn
}

/// Register one row's `awsvpc`, Fargate-compatible revision — one essential container delivering
/// through the `awslogs` driver under `role` — and answer its ARN.
async fn register_revision(router: &MultiServiceRouter, row: &Row, role: &str) -> String {
    let mut container = json!({
        "name": CONTAINER,
        "image": row.image,
        "memory": 128,
        "essential": true,
        "portMappings": [{ "containerPort": CONTAINER_PORT }],
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": LOG_GROUP,
                "awslogs-region": REGION,
                "awslogs-stream-prefix": STREAM_PREFIX,
            },
        },
    });
    if let Some(command) = row.command {
        container["command"] = json!(command);
    }
    let registered = ecs(
        router,
        row.case,
        "RegisterTaskDefinition",
        json!({
            "family": row.family,
            "networkMode": "awsvpc",
            "requiresCompatibilities": ["FARGATE"],
            "cpu": "256",
            "memory": "512",
            "executionRoleArn": role,
            "containerDefinitions": [container],
        }),
    )
    .await;
    registered["taskDefinition"]["taskDefinitionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the registration must answer an ARN, got {registered}",
                row.case
            )
        })
        .to_string()
}

/// `RunTask` one standalone Fargate task of `revision` into the network's first subnet, and
/// answer the ARN of the task it placed.
async fn run_task(
    router: &MultiServiceRouter,
    case: &str,
    network: &Network,
    revision: &str,
) -> String {
    let placed = ecs(
        router,
        case,
        "RunTask",
        json!({
            "cluster": CLUSTER,
            "taskDefinition": revision,
            "launchType": "FARGATE",
            "count": 1,
            "networkConfiguration": {
                "awsvpcConfiguration": {
                    "subnets": [network.subnets[0]],
                    "securityGroups": [network.task_group],
                    "assignPublicIp": "ENABLED",
                }
            },
        }),
    )
    .await;
    placed["tasks"][0]["taskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected RunTask to place one task, got {placed}"))
        .to_string()
}

/// One task as `DescribeTasks` reports it: the status it last reached, the stop it carries and
/// the address of the interface it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTask {
    last_status: String,
    stop_code: Option<String>,
    stopped_reason: Option<String>,
    address: Option<String>,
}

/// Read `task` back through `DescribeTasks`.
async fn describe(router: &MultiServiceRouter, case: &str, task: &str) -> ReportedTask {
    let described = ecs(
        router,
        case,
        "DescribeTasks",
        json!({ "cluster": CLUSTER, "tasks": [task] }),
    )
    .await;
    let reported = &described["tasks"][0];
    let text = |member: &str| {
        reported[member]
            .as_str()
            .map(::std::string::ToString::to_string)
    };
    // "tasks that use the `awsvpc` network mode are associated with an elastic network
    // interface": the address the task is reached at is a detail of that attachment.
    let address = reported["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| attachment["details"].as_array())
        .flatten()
        .find(|detail| detail["name"] == json!("privateIPv4Address"))
        .and_then(|detail| detail["value"].as_str())
        .map(::std::string::ToString::to_string);
    ReportedTask {
        last_status: text("lastStatus").unwrap_or_default(),
        stop_code: text("stopCode"),
        stopped_reason: text("stoppedReason"),
        address,
    }
}

/// Look at `task` until it reports `RUNNING` with an address of its own, and answer it.
async fn running(router: &MultiServiceRouter, case: &str, task: &str) -> ReportedTask {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = describe(router, case, task).await;
        if reported.last_status == RUNNING && reported.address.is_some() {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to reach {RUNNING} with an address of its own, got \
             {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at `task` until it reports `STOPPED`, and answer it.
async fn stopped(router: &MultiServiceRouter, case: &str, task: &str) -> ReportedTask {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = describe(router, case, task).await;
        if reported.last_status == STOPPED {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the task to reach {STOPPED}, got {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// One registration as `DescribeTargetHealth` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    state: String,
    reason: String,
    description: String,
}

/// Every registration `group` holds, as it is reported.
async fn health(router: &MultiServiceRouter, group: &str) -> Vec<Reported> {
    let body = elbv2(router, "DescribeTargetHealth", &[("TargetGroupArn", group)]).await;
    members_of(&body)
        .into_iter()
        .map(|description| {
            // The health's own block, because the override beside it carries members of the
            // same names ([`target_health_of`]).
            let health = target_health_of(description);
            let text = |body: &str, tag: &str| {
                elements(body, tag)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            };
            Reported {
                id: text(description, "Id"),
                state: text(health, "State"),
                reason: text(health, "Reason"),
                description: text(health, "Description"),
            }
        })
        .collect()
}

/// Look at `group` until its one registration reports `state` (and, where `reason` is given, that
/// reason), and answer that registration.
///
/// Polls the CONDITION — the verdict the group's own cadence reaches is clock-owed — rather than
/// waiting a fixed time.
async fn reaching(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    state: &str,
    reason: Option<&str>,
) -> Reported {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, group).await;
        let settled = reported.len() == 1
            && reported[0].state == state
            && reason.is_none_or(|reason| reported[0].reason == reason);
        if settled {
            return reported[0].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected one registration reported {state} {reason:?}, got {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at the load balancer named `name` until it reports `State.Code` `active`, which is when
/// its listener serves traffic on the address it published.
async fn active(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let body = elbv2(router, "DescribeLoadBalancers", &[("Names.member.1", name)]).await;
        let code = elements(&body, "Code").into_iter().next().unwrap_or_default();
        if code == "active" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: expected the load balancer {name} to reach active, got {code:?} in {body}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at every task of `placed` again and again across `RUN_WINDOW`, and answer once the whole
/// window has been looked at — failing on the first look at which any of them is not still
/// `RUNNING` with no stop at the address it holds.
///
/// The window is LOOKED AT rather than waited out: each look is an assertion, so a task stopped
/// anywhere inside it fails the case where it happened rather than being missed by a single read
/// at the end.
async fn hold_their_run(router: &MultiServiceRouter, placed: &[(&Row, String, String)]) {
    let until = Instant::now() + RUN_WINDOW;
    loop {
        for (row, task, address) in placed {
            let held = describe(router, row.case, task).await;
            assert_eq!(
                held,
                ReportedTask {
                    last_status: RUNNING.to_string(),
                    stop_code: None,
                    stopped_reason: None,
                    address: Some(address.clone()),
                },
                "case {:?}: the foreground server holds the container's program, so the task goes \
                 on running with no stop",
                row.case
            );
        }
        if Instant::now() >= until {
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at what the load balancer answers for `path` again and again across `ANSWER_WINDOW`, and
/// answer once the whole window has been looked at — failing on the first answer that is not
/// `status`, or that does not carry `marker` where one is given and does carry it where it is
/// `None`.
///
/// The window is LOOKED AT for the reason [`hold_their_run`]'s is: the group's checks fall due
/// several times inside it, so an answer that stopped holding part-way through fails here rather
/// than being missed.
async fn stays_answering(
    router: &MultiServiceRouter,
    case: &str,
    dns_name: &str,
    path: &str,
    status: u16,
    marker: Option<&str>,
) {
    let until = Instant::now() + ANSWER_WINDOW;
    loop {
        let answered = get(router, dns_name, path).await;
        assert_eq!(
            answered.status, status,
            "case {case:?}: expected {status} for every look across the window, got {} with body \
             {:?}",
            answered.status, answered.body
        );
        match marker {
            Some(marker) => assert!(
                answered.body.contains(marker),
                "case {case:?}: expected the target's own document on every look, got {:?}",
                answered.body
            ),
            None => assert!(
                !answered.body.contains(FRONTED_MARKER),
                "case {case:?}: expected no document of the target on any look, got {:?}",
                answered.body
            ),
        }
        if Instant::now() >= until {
            return;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at what the load balancer answers for the served path until it is the target's own
/// document — the `200` carrying `marker`.
///
/// Polls the CONDITION: the registration just made has no verdict yet, and the one the group's
/// checks reach is clock-owed, so the request reaches the target as soon as that verdict is in.
async fn answers_with(router: &MultiServiceRouter, case: &str, dns_name: &str, marker: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let answered = get(router, dns_name, SERVED_PATH).await;
        if answered.status == SERVED_STATUS && answered.body.contains(marker) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the load balancer to answer {SERVED_STATUS} with the \
             container's own document carrying {marker:?}, got {} with body {:?}",
            answered.status,
            answered.body
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Look at `group` until it reports no registration at all, which is when a deregistered target
/// has finished draining out of it.
async fn holds_no_registration(router: &MultiServiceRouter, case: &str, group: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let reported = health(router, group).await;
        if reported.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the group to hold no registration, got {reported:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// Create the `ip` target group whose health check requests `path`, the internet-facing load
/// balancer and the listener that forwards to it — and answer the group's ARN with the address the
/// load balancer published, once that load balancer reports itself serving.
///
/// The group names no cadence and no threshold of its own: those are durations and counts
/// `CreateTargetGroup` settles from its own documented defaults, which is what a caller that asks
/// for neither gets — the cadence a request DOES ask for is another suite's rule
/// (`alb_health_check_cadence_the_request_asked_for.rs`), not this one's.
async fn front_door(router: &MultiServiceRouter, network: &Network, path: &str) -> (String, String) {
    let created = elbv2(
        router,
        "CreateTargetGroup",
        &[
            ("Name", "standalone-group"),
            ("Protocol", "HTTP"),
            ("Port", &CONTAINER_PORT.to_string()),
            ("TargetType", "ip"),
            ("VpcId", network.vpc.as_str()),
            ("HealthCheckProtocol", "HTTP"),
            ("HealthCheckPath", path),
        ],
    )
    .await;
    let group = opt_element(&created, "TargetGroupArn")
        .unwrap_or_else(|| panic!("the create must answer a TargetGroupArn, got {created}"));
    // The drain a deregistration of this group takes, asked for as the shortest its own
    // documented range admits (`deregistration_delay.timeout_seconds` - "The amount of time, in
    // seconds, for Elastic Load Balancing to wait before changing the state of a deregistering
    // target from `draining` to `unused`", range 0-3600): what this suite is about is the answer a
    // group with NO target left gives, so a deregistration it makes has to take effect rather
    // than spend the five documented minutes the attribute defaults to.
    elbv2(
        router,
        "ModifyTargetGroupAttributes",
        &[
            ("TargetGroupArn", group.as_str()),
            (
                "Attributes.member.1.Key",
                "deregistration_delay.timeout_seconds",
            ),
            ("Attributes.member.1.Value", "0"),
        ],
    )
    .await;
    let balanced = elbv2(
        router,
        "CreateLoadBalancer",
        &[
            ("Name", "standalone-lb"),
            ("Type", "application"),
            ("Scheme", "internet-facing"),
            ("Subnets.member.1", network.subnets[0].as_str()),
            ("Subnets.member.2", network.subnets[1].as_str()),
            (
                "SecurityGroups.member.1",
                network.load_balancer_group.as_str(),
            ),
        ],
    )
    .await;
    let load_balancer = element(&balanced, "LoadBalancerArn");
    let dns_name = element(&balanced, "DNSName");
    elbv2(
        router,
        "CreateListener",
        &[
            ("LoadBalancerArn", load_balancer.as_str()),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", group.as_str()),
        ],
    )
    .await;
    active(router, "standalone-lb").await;
    (group, dns_name)
}

/// RULE: a standalone task whose essential container runs a server in the foreground keeps
/// running, with no stop of any kind, and the server answers at the address the task holds — so
/// an `ip` registration naming that address is checked and forwarded to; only a program that
/// really ends stops the task, with the code that names an essential container's exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_standalone_tasks_foreground_server_keeps_it_running_and_answers_its_load_balancer() {
    let router = emulator();
    let network = seed_network(&router).await;
    let role = execution_role(&router).await;
    logs(
        &router,
        "the group every container delivers into",
        "CreateLogGroup",
        json!({ "logGroupName": LOG_GROUP }),
    )
    .await;
    ecs(
        &router,
        "the cluster every task is placed in",
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
    )
    .await;

    // The front door is stood up before the tasks, so the registration the fronted row makes is
    // checked from the instant it is made.
    let (group, dns_name) = front_door(&router, &network, MISSING_PATH).await;

    // Every row's task is PLACED first and only then waited for, so the placements come up
    // together: the window in which each one must be seen HOLDING its run is one window rather
    // than one per row.
    let mut running_tasks: Vec<(&Row, String)> = Vec::new();
    for row in ROWS {
        let revision = register_revision(&router, row, &role).await;
        let task = run_task(&router, row.case, &network, &revision).await;
        running_tasks.push((row, task));
    }
    let mut placed: Vec<(&Row, String, String)> = Vec::new();
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    for (row, task) in running_tasks {
        let reported = running(&router, row.case, &task).await;
        assert_eq!(
            (
                reported.stop_code.as_deref(),
                reported.stopped_reason.as_deref()
            ),
            (None, None),
            "case {:?}: a task whose essential container is still running its program carries no \
             stop at all, got {reported:?}",
            row.case
        );
        let address = reported
            .address
            .clone()
            .expect("a settled poll answers a task holding an address");
        assert!(
            addresses.insert(address.clone()),
            "case {:?}: every task holds an address of its own, got {address} twice",
            row.case
        );
        placed.push((row, task, address));
    }

    // And they KEEP running: across a window past the span after which a standalone task whose
    // essential container exited is stopped, every look finds every task still running with no
    // stop.
    hold_their_run(&router, &placed).await;

    // The server EACH of them started answers at its own task's address, which is what a load
    // balancer forwarding to an `ip` registration naming that address reaches: every row is
    // registered as the group's only target in turn and must be answered with that row's own
    // document — the group's checks are pointed at a path no container holds anything at, so the
    // request reaches the registration through the load balancer's fail-open. The row driven
    // through the whole front door below is left registered.
    for (index, (row, _, address)) in placed.iter().enumerate() {
        elbv2(
            &router,
            "RegisterTargets",
            &[
                ("TargetGroupArn", group.as_str()),
                ("Targets.member.1.Id", address.as_str()),
                ("Targets.member.1.Port", &CONTAINER_PORT.to_string()),
            ],
        )
        .await;
        answers_with(&router, row.case, &dns_name, row.marker).await;
        if index == FRONTED {
            continue;
        }
        elbv2(
            &router,
            "DeregisterTargets",
            &[
                ("TargetGroupArn", group.as_str()),
                ("Targets.member.1.Id", address.as_str()),
                ("Targets.member.1.Port", &CONTAINER_PORT.to_string()),
            ],
        )
        .await;
        holds_no_registration(&router, row.case, &group).await;
    }

    // The checks were ANSWERED, and the answer is not one the group's matcher counts as a
    // success: the registration reaches `unhealthy` with the model's own reason for that, named
    // by the status the checks got back.
    let case = "the only target answering the checked path outside the matcher";
    let unhealthy = reaching(&router, case, &group, UNHEALTHY, Some(RESPONSE_CODE_MISMATCH)).await;
    assert_eq!(
        unhealthy.description,
        format!("Health checks failed with these codes: [{NOT_FOUND_STATUS}]"),
        "case {case:?}: the reason must be described by the status the checks got back, got \
         {unhealthy:?}"
    );

    // FAIL OPEN: the group holds no healthy target, so the request goes to the unhealthy one
    // anyway and the client is answered with the CONTAINER's own response — on every look across
    // a window the group's checks fall due several times inside, so the fail-open answer is the
    // answer rather than one that happened to land between two checks.
    let row = &ROWS[FRONTED];
    stays_answering(
        &router,
        case,
        &dns_name,
        SERVED_PATH,
        SERVED_STATUS,
        Some(row.marker),
    )
    .await;
    // And over a REAL connection to the address the load balancer published, which is what a
    // client handed that name dials: the same fail-open answer, off the socket.
    let off_the_socket = dialled(case, &dns_name, SERVED_PATH).await;
    assert_eq!(
        off_the_socket.status, SERVED_STATUS,
        "case {case:?}: expected the unhealthy target's own {SERVED_STATUS} on the published \
         address, got {} with body {:?}",
        off_the_socket.status, off_the_socket.body
    );
    assert!(
        off_the_socket.body.contains(row.marker),
        "case {case:?}: expected the container's own document on the published address, got {:?}",
        off_the_socket.body
    );

    // RESTORED: the check is pointed at the path the container serves, so its checks pass and the
    // registration returns to `healthy` with no reason at all.
    let restored = "the health check pointed back at the path the container serves";
    elbv2(
        &router,
        "ModifyTargetGroup",
        &[
            ("TargetGroupArn", group.as_str()),
            ("HealthCheckPath", SERVED_PATH),
        ],
    )
    .await;
    let healthy = reaching(&router, restored, &group, HEALTHY, None).await;
    assert_eq!(
        (healthy.reason.as_str(), healthy.description.as_str()),
        ("", ""),
        "case {restored:?}: a healthy registration carries no reason and no description, got \
         {healthy:?}"
    );
    stays_answering(
        &router,
        restored,
        &dns_name,
        SERVED_PATH,
        SERVED_STATUS,
        Some(row.marker),
    )
    .await;

    // NO TARGET TO SEND IT TO: the one registration is deregistered and leaves the group, so the
    // load balancer has no target for this request and answers its own status, carrying no
    // document of the target — on every look across a window the group holds nothing at all.
    let drained = "the only target deregistered";
    elbv2(
        &router,
        "DeregisterTargets",
        &[
            ("TargetGroupArn", group.as_str()),
            ("Targets.member.1.Id", healthy.id.as_str()),
            ("Targets.member.1.Port", &CONTAINER_PORT.to_string()),
        ],
    )
    .await;
    holds_no_registration(&router, drained, &group).await;
    stays_answering(
        &router,
        drained,
        &dns_name,
        SERVED_PATH,
        NO_TARGET_STATUS,
        None,
    )
    .await;
    let off_the_socket = dialled(drained, &dns_name, SERVED_PATH).await;
    assert_eq!(
        off_the_socket.status, NO_TARGET_STATUS,
        "case {drained:?}: expected the load balancer's own {NO_TARGET_STATUS} on the published \
         address, got {} with body {:?}",
        off_the_socket.status, off_the_socket.body
    );
}

/// RULE (the contrast): a standalone task whose essential container's program really ENDS is
/// stopped for that exit — `EssentialContainerExited` with the sentence Amazon ECS words for it —
/// which is the stop a task whose container is still running its program never takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_standalone_task_whose_program_ends_is_stopped_for_its_containers_exit() {
    let router = emulator();
    let network = seed_network(&router).await;
    let role = execution_role(&router).await;
    let case = "a program that writes its document and then returns";
    logs(&router, case, "CreateLogGroup", json!({ "logGroupName": LOG_GROUP })).await;
    ecs(&router, case, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let row = Row {
        case,
        family: "standaloneEnds",
        image: "public.ecr.aws/docker/library/nginx:alpine",
        command: Some(&[
            "/bin/sh",
            "-c",
            "echo STANDALONE-ENDS > /usr/share/nginx/html/index.html",
        ]),
        marker: "STANDALONE-ENDS",
    };
    let revision = register_revision(&router, &row, &role).await;
    let task = run_task(&router, case, &network, &revision).await;
    let reported = stopped(&router, case, &task).await;
    assert_eq!(
        (
            reported.stop_code.as_deref(),
            reported.stopped_reason.as_deref()
        ),
        (
            Some(ESSENTIAL_CONTAINER_EXITED),
            Some(ESSENTIAL_CONTAINER_EXITED_REASON)
        ),
        "case {case:?}: a task whose essential container exited carries that stop, got {reported:?}"
    );
}
