// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING container's own AWS calls may do, across the services the decision spans.
//!
//! RULE under test: a container of a running Amazon ECS task runs the program its launch names,
//! and an AWS call that program makes is signed with a session of the TASK ROLE the launch
//! resolved — `TaskDefinition$taskRoleArn` is "the IAM role that grants containers in the task
//! permission to call Amazon Web Services APIs on your behalf" — so the service that owns the
//! resource authorizes the call against the policies in force on that role: a container whose
//! task role lacks the action is refused by that service and its program reports the refusal,
//! attaching the grant to the role lifts the refusal on the next task, a role granted another
//! action is refused this one, and a task that resolves no task role at all locates no
//! credentials to call with. The container's own exit status is the status its program ended
//! with either way — so a program that ENDS WITH the status of its one call makes the grant
//! decide the `exitCode` a stopped task reports for that container.
//!
//! Why this suite lives beside the router: nothing about the decision belongs to one service.
//! The role and its policies are IAM's records, the session is the token service's, the
//! decision and the object are Amazon S3's, and what the container WROTE is CloudWatch Logs' —
//! so all of them have to agree for the rule to hold. Every resource below is created through
//! the real operation of the service that owns it on ONE shared engine, and everything asserted
//! is read back through a real read: the container's `exitCode` through `DescribeTasks`, its
//! lines through `GetLogEvents`, and the object through Amazon S3's own `GetObject`.
//!
//! Member and doc source: `smithy_query(query="shape", shape="TaskDefinition")` for
//! `taskRoleArn`, `smithy_query(query="shape", shape="TaskOverride")` for the `taskRoleArn` a
//! launch overrides it with ("the Amazon Resource Name (ARN) of the role that containers in
//! this task can assume"), and `smithy_query(query="shape", shape="Container")` for `exitCode`
//! ("the exit code returned from the container").

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The cluster every task below runs in.
const CLUSTER: &str = "taskrolecalls";

/// The one container of every task definition below.
const CONTAINER: &str = "writer";

/// The image the containers start from. Nothing is fetched from it: what runs is the `command`
/// the revision states, so the reference names a repository this emulator knows no program of
/// — an image whose own entry point IS a program would be exec'd with that command as its
/// arguments instead
/// (`batch_job_container_runs_its_images_entry_point.rs` covers that rule).
const IMAGE: &str = "registry.example.com/simparity/task-role-probe:1";

/// The log group every container delivers its output to, created through CloudWatch Logs' own
/// API before any task is launched.
const LOG_GROUP: &str = "/emulator/ecs/task-role-calls";

/// The `awslogs-stream-prefix` every task definition configures, and so the first segment of
/// every stream name they deliver to.
const STREAM_PREFIX: &str = "role";

/// The bucket every container's transfer addresses, created through Amazon S3's own API.
const BUCKET: &str = "ecs-task-role-objects";

/// The XML namespace an Amazon S3 request body carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The code Amazon S3 refuses a request no policy allows with. Written out rather than read off
/// the service's constants: it is fixed by the Amazon S3 API ("AccessDenied" in its published
/// list of error codes), so a literal here is what catches a wrong wire code.
const ACCESS_DENIED: &str = "AccessDenied";

/// How long a poll keeps looking before it calls a clock-owed move stalled, and how long it
/// leaves between looks. Generous on purpose: the wait ends on the CONDITION, and the deadline
/// is only there so a move that never happens fails the test instead of hanging it.
const DEADLINE: Duration = Duration::from_secs(120);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(150);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`, which is what
/// routes the request; the signature is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
         /aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// The services this decision spans over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them, with the engine handle the network records are written through.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// One awsJson1_1 call to the service `signing_name`, answered with the HTTP status and the
/// parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// One Amazon ECS call that has to succeed.
async fn ecs_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let (status, answer) = rpc(
        router,
        "ecs",
        "AmazonEC2ContainerServiceV20141113",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ecs {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

/// One CloudWatch Logs call, answered with the HTTP status and the parsed body.
async fn logs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    rpc(router, "logs", "Logs_20140328", operation, body).await
}

/// One raw request to the emulator's Amazon S3 front door: path-style, so the bucket and the
/// key live in the URI and the body is the payload.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .header("content-type", "application/xml")
        .body(Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.into_body().as_ref()).to_string(),
    )
}

/// One awsQuery call to IAM that has to succeed, answered with its XML body.
async fn iam(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(name));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: iam {action} must answer 200, got {status} with {text}"
    );
    text
}

/// One ec2Query action that has to succeed, answered with the first `element` of its XML body —
/// the identifier Amazon EC2 minted for the resource the action created.
async fn ec2_element(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
    element: &str,
) -> String {
    let mut body = format!("Action={action}&Version=2016-11-15");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(name));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("ec2"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: ec2 {action} must answer 200, got {status} with {text}"
    );
    crate::query_wire::opt_element(&text, element)
        .unwrap_or_else(|| panic!("case {case:?}: no <{element}> in {text}"))
}

/// The trust policy a role Amazon ECS assumes for a task carries: the service principal it
/// assumes a task's roles as.
fn ecs_tasks_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ecs-tasks.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create the role `name`, trusted by this service's principal, through IAM's own API; answer
/// its ARN.
async fn role(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = iam(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &ecs_tasks_trust()),
        ],
    )
    .await;
    crate::query_wire::opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: no <Arn> in {created}"))
}

/// Attach an inline policy to `role_name` allowing `actions` on `resource`, through IAM's own
/// `PutRolePolicy` — the call a client makes to repair a missing grant.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    policy_name: &str,
    actions: &[&str],
    resource: &str,
) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": resource,
        }],
    })
    .to_string();
    iam(
        router,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", &document),
        ],
    )
    .await;
}

/// Register one revision whose single container runs `program` under `task_role_arn`, and
/// answer nothing: the family is what the launch below names.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    execution_role_arn: &str,
    task_role_arn: Option<&str>,
    program: &str,
) {
    let mut body = json!({
        "family": family,
        "networkMode": "awsvpc",
        "cpu": "256",
        "memory": "512",
        "executionRoleArn": execution_role_arn,
        "requiresCompatibilities": ["FARGATE"],
        "containerDefinitions": [{
            "name": CONTAINER,
            "image": IMAGE,
            "essential": true,
            "command": ["sh", "-c", program],
            "logConfiguration": {
                "logDriver": "awslogs",
                "options": {
                    "awslogs-group": LOG_GROUP,
                    "awslogs-region": REGION,
                    "awslogs-stream-prefix": STREAM_PREFIX,
                },
            },
        }],
    });
    if let Some(task_role_arn) = task_role_arn {
        body["taskRoleArn"] = json!(task_role_arn);
    }
    ecs_ok(router, case, "RegisterTaskDefinition", body).await;
}

/// `RunTask` one Fargate task of `family` in `subnet`, with `overrides` as sent, and answer its
/// ARN.
async fn run_task(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    subnet: &str,
    overrides: Option<Value>,
) -> String {
    let mut body = json!({
        "cluster": CLUSTER,
        "taskDefinition": family,
        "launchType": "FARGATE",
        "networkConfiguration": {
            "awsvpcConfiguration": { "subnets": [subnet], "assignPublicIp": "ENABLED" }
        },
    });
    if let Some(overrides) = overrides {
        body["overrides"] = overrides;
    }
    let answer = ecs_ok(router, case, "RunTask", body).await;
    let tasks = answer["tasks"].as_array().cloned().unwrap_or_default();
    let task = tasks
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected RunTask to place one task, got {answer}"));
    task["taskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskArn, got {task}"))
        .to_string()
}

/// The stream the `awslogs` driver delivers one container of one task to:
/// `<prefix>/<container name>/<task id>`, the naming that driver's own documentation states.
fn stream_of(task_arn: &str) -> String {
    let task_id = task_arn.rsplit('/').next().unwrap_or_default();
    format!("{STREAM_PREFIX}/{CONTAINER}/{task_id}")
}

/// Look again until the task `task_arn` reports `STOPPED`, then answer the lines its container
/// delivered to its own `awslogs` stream and the task as `DescribeTasks` reports it.
///
/// The CONDITION is polled rather than a span slept for: the task's own end is what makes its
/// container's run certainly over, so the lines read afterwards are the whole of what the
/// program wrote.
async fn stopped_task(
    router: &MultiServiceRouter,
    case: &str,
    task_arn: &str,
) -> (Vec<String>, Value) {
    let deadline = Instant::now() + DEADLINE;
    let task = loop {
        let body = ecs_ok(
            router,
            case,
            "DescribeTasks",
            json!({ "cluster": CLUSTER, "tasks": [task_arn] }),
        )
        .await;
        let task = body["tasks"][0].clone();
        if task["lastStatus"].as_str() == Some("STOPPED") {
            break task;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the task never stopped, last read {task}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    };
    let (status, read) = logs(
        router,
        "GetLogEvents",
        json!({
            "logGroupName": LOG_GROUP,
            "logStreamName": stream_of(task_arn),
            "startFromHead": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetLogEvents must answer 200, got {status} with {read}"
    );
    let lines = read["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .collect();
    (lines, task)
}

/// The value a line `NAME=<value>` carries, and `None` when no line names it.
fn reported(lines: &[String], name: &str) -> Option<String> {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .map(str::to_string)
}

/// The program an UPLOAD arm runs: it writes a file in the container's own layer and carries it
/// into the bucket, reporting which way the call went either way — the shape a client's probe
/// of a task role's grant takes, which ends successfully whatever the call answered.
fn upload_program(key: &str, payload: &str) -> String {
    format!(
        "echo {payload} > /tmp/out.txt; \
         if aws s3 cp /tmp/out.txt s3://{BUCKET}/{key}; then echo \"RESULT=ALLOWED\"; \
         else echo \"RESULT=DENIED\"; fi"
    )
}

/// The program a DOWNLOAD arm runs: the same probe in the other direction, reporting the bytes
/// it read back.
fn download_program(key: &str) -> String {
    format!(
        "if aws s3 cp s3://{BUCKET}/{key} /tmp/in.txt; then echo \"RESULT=ALLOWED\"; \
         echo \"READ=$(cat /tmp/in.txt)\"; else echo \"RESULT=DENIED\"; fi"
    )
}

/// What the object store holds at `key`: `Some(bytes)` for an object it holds, and `None` when
/// it answers that there is none — read through Amazon S3's own `GetObject`.
async fn held_object(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let (status, body) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    (status == 200).then_some(body)
}

/// Everything a launch below needs, each created through the API of the service that owns it:
/// the network the Fargate tasks are placed in (with the route off the VPC a task that fetches
/// an image needs), the bucket the transfers address, the log group the containers deliver to,
/// the cluster they run in, and the EXECUTION role the agent acts as — which a Fargate task
/// with the `awslogs` driver needs. Answers the subnet and that role's ARN.
async fn given_the_account(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    execution_role_name: &str,
) -> (String, String) {
    let vpc = ec2_element(
        router,
        case,
        "CreateVpc",
        &[("CidrBlock", "10.0.0.0/16")],
        "vpcId",
    )
    .await;
    let subnet = ec2_element(
        router,
        case,
        "CreateSubnet",
        &[
            ("VpcId", vpc.as_str()),
            ("CidrBlock", "10.0.0.0/24"),
            ("AvailabilityZone", &format!("{REGION}a")),
        ],
        "subnetId",
    )
    .await;
    crate::ec2_network::register_internet_route(engine, &vpc);
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (bucket_status, bucket_body) =
        s3(router, "PUT", &format!("/{BUCKET}"), configuration.as_bytes()).await;
    assert_eq!(
        bucket_status, 200,
        "case {case:?}: the bucket must be created, got {bucket_status} with {bucket_body}"
    );
    let (log_status, log_body) =
        logs(router, "CreateLogGroup", json!({ "logGroupName": LOG_GROUP })).await;
    assert_eq!(
        log_status, 200,
        "case {case:?}: CreateLogGroup must answer 200, got {log_status} with {log_body}"
    );
    ecs_ok(router, case, "CreateCluster", json!({ "clusterName": CLUSTER })).await;
    let execution = role(router, case, execution_role_name).await;
    grant(
        router,
        case,
        execution_role_name,
        "deliver-container-logs",
        &["logs:CreateLogStream", "logs:PutLogEvents"],
        "*",
    )
    .await;
    (subnet, execution)
}

/// RULE: an AWS call a container's program makes is signed with a session of the task role the
/// launch resolved, so the owning service decides it on the policies in force on THAT role.
///
/// The rows differ along what the rule generalises over: whether the role carries the grant the
/// call needs (none at all, the grant attached to the same role between two launches, a grant
/// for ANOTHER action), which direction the transfer goes (a write and a read), and where the
/// task role comes from (the revision's own member, and the launch's `overrides`).
#[tokio::test]
async fn the_task_role_decides_what_a_containers_own_aws_call_may_do() {
    let case = "the task role's grant decides the container's call";
    let (router, engine) = emulator();
    let (subnet, execution) = given_the_account(&router, &engine, case, "taskrolecallsexec").await;

    // The two TASK roles the containers call as: one that carries no grant at all, and one
    // granted the read alone.
    let writer_role = role(&router, case, "taskrolecallswriter").await;
    let reader_role = role(&router, case, "taskrolecallsreader").await;
    grant(
        &router,
        case,
        "taskrolecallsreader",
        "read-objects",
        &["s3:GetObject"],
        &format!("arn:aws:s3:::{BUCKET}/*"),
    )
    .await;

    // One revision per arm, each registered before any of them runs.
    let arms: [(&str, Option<&str>, String); 4] = [
        (
            "ungrantedwriter",
            Some(writer_role.as_str()),
            upload_program("ungranted.txt", "payload-ungranted"),
        ),
        (
            "grantedwriter",
            Some(writer_role.as_str()),
            upload_program("granted.txt", "payload-granted"),
        ),
        (
            "readonlywriter",
            Some(reader_role.as_str()),
            upload_program("readonly.txt", "payload-readonly"),
        ),
        (
            "readonlyreader",
            Some(reader_role.as_str()),
            download_program("granted.txt"),
        ),
    ];
    for (family, task_role, program) in &arms {
        register(&router, case, family, &execution, *task_role, program).await;
    }

    // ARM 1 — the role carries no grant: the object store refuses the write, the program reports
    // the refusal it saw, and the container still ends with the status its program chose.
    let ungranted = run_task(&router, case, "ungrantedwriter", &subnet, None).await;
    let (lines, task) = stopped_task(&router, case, &ungranted).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("DENIED"),
        "case {case:?} arm \"no grant\": expected the program to report DENIED, got {lines:?}"
    );
    let refusal = lines
        .iter()
        .find(|line| line.contains("upload failed:"))
        .cloned()
        .unwrap_or_default();
    for expected in [
        ACCESS_DENIED,
        "calling the PutObject operation",
        &format!("s3://{BUCKET}/ungranted.txt"),
    ] {
        assert!(
            refusal.contains(expected),
            "case {case:?} arm \"no grant\": expected the CLI's failure line to carry \
             {expected:?}, got {refusal:?}"
        );
    }
    assert_eq!(
        task["containers"][0]["exitCode"].as_i64(),
        Some(0),
        "case {case:?} arm \"no grant\": expected the container's own exit status, got {task}"
    );
    let held = held_object(&router, "ungranted.txt").await;
    assert!(
        held.is_none(),
        "case {case:?} arm \"no grant\": the refused write must have stored nothing, got {held:?}"
    );

    // ARM 2 — the grant is attached to the SAME role and another task runs: the write is made,
    // and the object the store holds carries the bytes the program wrote.
    grant(
        &router,
        case,
        "taskrolecallswriter",
        "write-objects",
        &["s3:PutObject"],
        &format!("arn:aws:s3:::{BUCKET}/*"),
    )
    .await;
    let granted = run_task(&router, case, "grantedwriter", &subnet, None).await;
    let (lines, task) = stopped_task(&router, case, &granted).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("ALLOWED"),
        "case {case:?} arm \"the grant attached\": expected ALLOWED, got {lines:?}"
    );
    assert_eq!(
        task["containers"][0]["exitCode"].as_i64(),
        Some(0),
        "case {case:?} arm \"the grant attached\": expected exit status 0, got {task}"
    );
    let held = held_object(&router, "granted.txt").await;
    assert_eq!(
        held.as_deref().map(str::trim),
        Some("payload-granted"),
        "case {case:?} arm \"the grant attached\": expected the object to hold what the \
         container wrote, got {held:?}"
    );

    // ARM 3 — a role granted the READ is refused the write: the decision is the role's own
    // policies per action, not the task's.
    let read_only = run_task(&router, case, "readonlywriter", &subnet, None).await;
    let (lines, _) = stopped_task(&router, case, &read_only).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("DENIED"),
        "case {case:?} arm \"granted another action\": expected DENIED, got {lines:?}"
    );
    let held = held_object(&router, "readonly.txt").await;
    assert!(
        held.is_none(),
        "case {case:?} arm \"granted another action\": nothing must have been stored, got \
         {held:?}"
    );

    // ARM 4 — the same role IS allowed the read, in the other direction: the container reads
    // the object arm 2 wrote and reports its bytes.
    let reading = run_task(&router, case, "readonlyreader", &subnet, None).await;
    let (lines, task) = stopped_task(&router, case, &reading).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("ALLOWED"),
        "case {case:?} arm \"the read it is granted\": expected ALLOWED, got {lines:?}"
    );
    assert_eq!(
        reported(&lines, "READ").as_deref(),
        Some("payload-granted"),
        "case {case:?} arm \"the read it is granted\": expected the object's bytes, got {lines:?}"
    );
    assert_eq!(
        task["containers"][0]["exitCode"].as_i64(),
        Some(0),
        "case {case:?} arm \"the read it is granted\": expected exit status 0, got {task}"
    );

    // ARM 5 — the launch's `overrides` name the GRANTED role for a revision whose own member
    // names the read-only one: the role the launch RESOLVED is the one the call is signed as.
    let overridden = run_task(
        &router,
        case,
        "readonlywriter",
        &subnet,
        Some(json!({ "taskRoleArn": writer_role })),
    )
    .await;
    let (lines, _) = stopped_task(&router, case, &overridden).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("ALLOWED"),
        "case {case:?} arm \"an overridden task role\": expected ALLOWED, got {lines:?}"
    );
    let held = held_object(&router, "readonly.txt").await;
    assert_eq!(
        held.as_deref().map(str::trim),
        Some("payload-readonly"),
        "case {case:?} arm \"an overridden task role\": expected the object the override's role \
         was allowed to write, got {held:?}"
    );
}

/// RULE, at the other end of the same rule: a task that resolves NO task role hands its
/// containers no credential, so a program of theirs that makes an AWS call locates none — it
/// reports the CLI's own account of that and writes nothing into the bucket.
#[tokio::test]
async fn a_container_of_a_task_with_no_task_role_locates_no_credentials() {
    let case = "no task role at all";
    let (router, engine) = emulator();
    let (subnet, execution) = given_the_account(&router, &engine, case, "norolecallsexec").await;
    register(
        &router,
        case,
        "noroletask",
        &execution,
        None,
        &upload_program("norole.txt", "payload-norole"),
    )
    .await;

    let task_arn = run_task(&router, case, "noroletask", &subnet, None).await;
    let (lines, task) = stopped_task(&router, case, &task_arn).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("DENIED"),
        "case {case:?}: expected the program to report the call it could not make, got {lines:?}"
    );
    let refusal = lines
        .iter()
        .find(|line| line.contains("upload failed:"))
        .cloned()
        .unwrap_or_default();
    assert!(
        refusal.contains("Unable to locate credentials"),
        "case {case:?}: expected the CLI's own account of a call it has no credential for, got \
         {refusal:?}"
    );
    assert_eq!(
        task["containers"][0]["exitCode"].as_i64(),
        Some(0),
        "case {case:?}: expected the container's own exit status, got {task}"
    );
    let held = held_object(&router, "norole.txt").await;
    assert!(
        held.is_none(),
        "case {case:?}: a call nothing was signed with must have stored nothing, got {held:?}"
    );
}

/// RULE, over the CLI's LOW-LEVEL object commands: an `aws s3api` call a container's program
/// makes is the same signed request as any other, so the task role decides it — and what the
/// command answers is what the program reads, the response document on success and the
/// service's own refusal when the role may not perform the action.
///
/// The arms differ along whether the role carries the grant (before and after the one `Allow`
/// statement is attached to the SAME role) and along the command (`put-object`, then
/// `head-object` on what it wrote), with the program written the way a client writes it: the
/// call's own output captured, and the outcome line printed from it.
#[tokio::test]
async fn the_low_level_object_commands_are_decided_by_the_task_role() {
    let case = "the low-level object commands";
    let (router, engine) = emulator();
    let (subnet, execution) = given_the_account(&router, &engine, case, "apicallsexec").await;
    let task_role = role(&router, case, "apicallstaskrole").await;

    // The program every arm runs: it writes a file in the container's own layer, calls the
    // low-level write with it, and reports which way the call went — capturing the command's
    // own output, which is where the service's refusal is reported.
    let program = |key: &str| {
        format!(
            "echo payload-api > /tmp/payload; \
             if out=$(aws s3api put-object --bucket {BUCKET} --key {key} \
             --body /tmp/payload 2>&1); then echo \"RESULT=OK\"; echo \"DOCUMENT=$out\"; \
             else echo \"RESULT=DENIED\"; echo \"ERROR=$out\"; fi"
        )
    };
    register(
        &router,
        case,
        "apiwriterdenied",
        &execution,
        Some(task_role.as_str()),
        &program("api-denied.txt"),
    )
    .await;
    register(
        &router,
        case,
        "apiwritergranted",
        &execution,
        Some(task_role.as_str()),
        &program("api-granted.txt"),
    )
    .await;
    register(
        &router,
        case,
        "apireader",
        &execution,
        Some(task_role.as_str()),
        &format!(
            "if out=$(aws s3api head-object --bucket {BUCKET} --key api-granted.txt 2>&1); \
             then echo \"RESULT=OK\"; else echo \"RESULT=DENIED\"; echo \"ERROR=$out\"; fi"
        ),
    )
    .await;

    // ARM 1 — no grant on the role: the service refuses the call, the program reports the
    // refusal it captured, and nothing is stored.
    let denied = run_task(&router, case, "apiwriterdenied", &subnet, None).await;
    let (lines, task) = stopped_task(&router, case, &denied).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("DENIED"),
        "case {case:?} arm \"no grant\": expected the program to report DENIED, got {lines:?}"
    );
    let error = reported(&lines, "ERROR").unwrap_or_default();
    for expected in [ACCESS_DENIED, "calling the PutObject operation"] {
        assert!(
            error.contains(expected),
            "case {case:?} arm \"no grant\": expected the captured error to carry {expected:?}, \
             got {error:?}"
        );
    }
    assert_eq!(
        task["containers"][0]["exitCode"].as_i64(),
        Some(0),
        "case {case:?} arm \"no grant\": expected the status the program ended with, got {task}"
    );
    let held = held_object(&router, "api-denied.txt").await;
    assert!(
        held.is_none(),
        "case {case:?} arm \"no grant\": the refused call must have stored nothing, got {held:?}"
    );

    // ARM 2 — the one `Allow` statement is attached to the same role: the next task's call is
    // made, the object holds the bytes the command carried, and the program reads back the
    // response document the call answered.
    grant(
        &router,
        case,
        "apicallstaskrole",
        "write-and-read-objects",
        &["s3:PutObject", "s3:GetObject"],
        &format!("arn:aws:s3:::{BUCKET}/*"),
    )
    .await;
    let granted = run_task(&router, case, "apiwritergranted", &subnet, None).await;
    let (lines, _) = stopped_task(&router, case, &granted).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("OK"),
        "case {case:?} arm \"the grant attached\": expected OK, got {lines:?}"
    );
    let held = held_object(&router, "api-granted.txt").await;
    assert_eq!(
        held.as_deref().map(str::trim),
        Some("payload-api"),
        "case {case:?} arm \"the grant attached\": expected the object to hold the body the \
         command carried, got {held:?}"
    );
    // The response document the call answered is JSON, which the CLI prints over several
    // lines, so the line the program's own prefix opened is followed by the members.
    assert!(
        lines.iter().any(|line| line.contains("ETag")),
        "case {case:?} arm \"the grant attached\": expected the response document the call \
         answered, got {lines:?}"
    );

    // ARM 3 — the same role reading the object's metadata, which is the other command.
    let reading = run_task(&router, case, "apireader", &subnet, None).await;
    let (lines, _) = stopped_task(&router, case, &reading).await;
    assert_eq!(
        reported(&lines, "RESULT").as_deref(),
        Some("OK"),
        "case {case:?} arm \"the metadata read\": expected OK, got {lines:?}"
    );
}

/// RULE: the container runs its program as the shell's own grammar reads it, so a program that
/// RETRIES its call — the shape a client writes to wait for a grant it has just attached —
/// takes each attempt and then ENDS, which is what lets the task reach `STOPPED` with the
/// status its program chose.
///
/// The rows differ along the FORM the retry is written in, each a form the grammar parses: a
/// `for` loop over the attempts, and a `while` loop whose counter is an arithmetic expansion.
/// Both run against a task role carrying no grant, so every attempt is refused and only the
/// loop's own end can end the program.
#[tokio::test]
async fn a_retrying_program_takes_each_attempt_and_ends() {
    let case = "a retrying program";
    let (router, engine) = emulator();
    let (subnet, execution) = given_the_account(&router, &engine, case, "retryexec").await;
    let task_role = role(&router, case, "retrytaskrole").await;

    // (the arm, the family, the program, the attempts it must report)
    let arms: [(&str, &str, String, &str); 2] = [
        (
            "a for loop over the attempts",
            "retryforloop",
            format!(
                "echo payload-for > /tmp/out.txt; \
                 for attempt in 1 2 3; do \
                 if aws s3 cp /tmp/out.txt s3://{BUCKET}/retry-for.txt; \
                 then echo \"RESULT=ALLOWED\"; exit 0; fi; done; \
                 echo \"RESULT=DENIED\"; echo \"ATTEMPTS=$attempt\""
            ),
            "3",
        ),
        (
            "a while loop counting with an arithmetic expansion",
            "retrywhileloop",
            format!(
                "echo payload-while > /tmp/out.txt; attempt=0; \
                 while [ \"$attempt\" -lt 3 ]; do \
                 if aws s3 cp /tmp/out.txt s3://{BUCKET}/retry-while.txt; \
                 then echo \"RESULT=ALLOWED\"; exit 0; fi; \
                 attempt=$((attempt + 1)); done; \
                 echo \"RESULT=DENIED\"; echo \"ATTEMPTS=$attempt\""
            ),
            "3",
        ),
    ];
    for (_, family, program, _) in &arms {
        register(
            &router,
            case,
            family,
            &execution,
            Some(task_role.as_str()),
            program,
        )
        .await;
    }
    for (arm, family, _, attempts) in &arms {
        let task_arn = run_task(&router, case, family, &subnet, None).await;
        let (lines, task) = stopped_task(&router, case, &task_arn).await;
        assert_eq!(
            reported(&lines, "RESULT").as_deref(),
            Some("DENIED"),
            "case {case:?} arm {arm:?}: expected the program past its loop, got {lines:?}"
        );
        assert_eq!(
            reported(&lines, "ATTEMPTS").as_deref(),
            Some(*attempts),
            "case {case:?} arm {arm:?}: expected {attempts} attempts, got {lines:?}"
        );
        assert_eq!(
            task["containers"][0]["exitCode"].as_i64(),
            Some(0),
            "case {case:?} arm {arm:?}: expected the status the program ended with, got {task}"
        );
    }
    for key in ["retry-for.txt", "retry-while.txt"] {
        let held = held_object(&router, key).await;
        assert!(
            held.is_none(),
            "case {case:?}: every refused attempt must have stored nothing at {key:?}, got \
             {held:?}"
        );
    }
}

/// The status the AWS CLI exits with for a call it MADE: "0 -- Command was successful. There
/// were no errors thrown by either the CLI or by the service the request was made to." SOURCE:
/// the AWS CLI's own return codes, written out here rather than read off the service's
/// constants so a wrong status is caught instead of mirrored.
const CALL_MADE: i64 = 0;

/// The status a HIGH-LEVEL transfer command exits with when its call was refused: "1 -- Limited
/// errors … For `s3` commands, this means at least one or more s3 transfers failed." SOURCE: the
/// same list of return codes.
const TRANSFER_REFUSED: i64 = 1;

/// The status a LOW-LEVEL `s3api` command exits with when its call was refused: "254 -- The
/// command was successfully parsed and a request was made to the specified service but the
/// service returned an error." SOURCE: the same list of return codes.
const API_CALL_REFUSED: i64 = 254;

/// The bytes every program below writes into its file and carries into the bucket.
const EXIT_STATUS_PAYLOAD: &str = "payload-exit-status";

/// `Task$stopCode` of a task Amazon ECS stopped because an essential container of it exited,
/// from that member's own list of codes.
const ESSENTIAL_CONTAINER_EXITED: &str = "EssentialContainerExited";

/// The sentence Amazon ECS words that code with in `Task$stoppedReason`.
const ESSENTIAL_CONTAINER_EXITED_REASON: &str = "Essential container in task exited";

/// One row of the table below: a program whose own exit status is the status of the ONE AWS
/// call it makes, and the status that call's refusal exits with.
struct ExitStatusRow {
    /// What the row is called in an assertion.
    arm: &'static str,
    /// The family its revision is registered under. The SAME revision is launched twice — once
    /// before the grant is attached and once after — which is the shape of a client's repair.
    family: &'static str,
    /// The object its call addresses.
    key: &'static str,
    /// The program its container runs, given that key.
    program: fn(&str) -> String,
    /// Whether that program PRINTS the status it read before ending with it, which is the form
    /// dimension the rows differ along.
    prints_status: bool,
    /// The CLI's own account of a call the service refused, as that command group words it: the
    /// transfer commands report the transfer that failed, and the low-level commands report the
    /// operation the error came back from.
    refusal_line: &'static str,
    /// The status the CLI exits with while the task role carries no grant for the call.
    refused_status: i64,
}

/// How the high-level transfer commands report a transfer that was not made.
const TRANSFER_REFUSAL_LINE: &str = "upload failed:";

/// How the low-level commands report an error the service answered: "An error occurred
/// (<code>) when calling the <operation> operation", the CLI's own wording.
const API_REFUSAL_LINE: &str = "when calling the PutObject operation";

/// What `row` must have printed for a container that ended with `status`: the status itself for
/// a program that names it, and nothing at all for one that leaves it to its last command.
fn printed_status(row: &ExitStatusRow, status: i64) -> Option<String> {
    row.prints_status.then(|| status.to_string())
}

/// A high-level transfer whose status the program NAMES: it reads the status of the command it
/// just ran and ends with it.
fn transfer_naming_its_status(key: &str) -> String {
    format!(
        "echo {EXIT_STATUS_PAYLOAD} > /tmp/out.txt; \
         aws s3 cp /tmp/out.txt s3://{BUCKET}/{key}; status=$?; \
         echo \"STATUS=$status\"; exit $status"
    )
}

/// The same transfer as the LAST command the program runs, which is the other way its status
/// reaches the container: "the exit status of a shell shall be the exit status of the last
/// command it executed", with no `exit` written at all.
fn transfer_as_the_last_command(key: &str) -> String {
    format!("echo {EXIT_STATUS_PAYLOAD} > /tmp/out.txt; aws s3 cp /tmp/out.txt s3://{BUCKET}/{key}")
}

/// A low-level object write whose status the program names.
fn api_write_naming_its_status(key: &str) -> String {
    format!(
        "echo {EXIT_STATUS_PAYLOAD} > /tmp/payload; \
         aws s3api put-object --bucket {BUCKET} --key {key} --body /tmp/payload; status=$?; \
         echo \"STATUS=$status\"; exit $status"
    )
}

/// The same low-level write as the last command the program runs.
fn api_write_as_the_last_command(key: &str) -> String {
    format!(
        "echo {EXIT_STATUS_PAYLOAD} > /tmp/payload; \
         aws s3api put-object --bucket {BUCKET} --key {key} --body /tmp/payload"
    )
}

/// RULE: a task Amazon ECS stopped because an essential container of it exited reports THAT
/// container's own exit status — `Container$exitCode`, "the exit code returned from the
/// container" — so a program whose status is the status of its one AWS call has the task role's
/// grant decide the `exitCode` `DescribeTasks` reports: the status the CLI documents for a call
/// the owning service refused while the role carries no grant, and the status of a call made
/// once the one statement is attached to that role.
///
/// The rows differ along what the rule generalises over: the command group the call is made
/// through (the high-level transfer and the low-level object write, whose documented refusal
/// statuses differ) and the form the program propagates it in (named with `exit`, and left as
/// the status of the last command it ran). Every row is launched twice — before the grant and
/// after it — against the SAME role, so the only difference between the two reads is the
/// policy in force.
///
/// Both phases launch every task before polling any of them: what each task waits on is the
/// moves its own clock owes it, and those are owed concurrently.
#[tokio::test]
async fn the_task_roles_grant_decides_the_exit_status_a_stopped_task_reports() {
    let case = "the exit status of the call";
    let (router, engine) = emulator();
    let (subnet, execution) = given_the_account(&router, &engine, case, "exitstatusexec").await;
    let role_name = "exitstatustaskrole";
    let task_role = role(&router, case, role_name).await;

    let rows: [ExitStatusRow; 4] = [
        ExitStatusRow {
            arm: "a transfer whose status the program names",
            family: "exitstatuscpnamed",
            key: "exit-status-cp-named.txt",
            program: transfer_naming_its_status,
            prints_status: true,
            refusal_line: TRANSFER_REFUSAL_LINE,
            refused_status: TRANSFER_REFUSED,
        },
        ExitStatusRow {
            arm: "a transfer as the program's last command",
            family: "exitstatuscplast",
            key: "exit-status-cp-last.txt",
            program: transfer_as_the_last_command,
            prints_status: false,
            refusal_line: TRANSFER_REFUSAL_LINE,
            refused_status: TRANSFER_REFUSED,
        },
        ExitStatusRow {
            arm: "an object write whose status the program names",
            family: "exitstatusapinamed",
            key: "exit-status-api-named.txt",
            program: api_write_naming_its_status,
            prints_status: true,
            refusal_line: API_REFUSAL_LINE,
            refused_status: API_CALL_REFUSED,
        },
        ExitStatusRow {
            arm: "an object write as the program's last command",
            family: "exitstatusapilast",
            key: "exit-status-api-last.txt",
            program: api_write_as_the_last_command,
            prints_status: false,
            refusal_line: API_REFUSAL_LINE,
            refused_status: API_CALL_REFUSED,
        },
    ];
    for row in &rows {
        register(
            &router,
            case,
            row.family,
            &execution,
            Some(task_role.as_str()),
            &(row.program)(row.key),
        )
        .await;
    }

    // PHASE 1 — the role carries no grant for the write: the owning service refuses the call,
    // and the status that refusal exits with is the status the stopped task reports.
    let mut launched = Vec::new();
    for row in &rows {
        launched.push((row, run_task(&router, case, row.family, &subnet, None).await));
    }
    for (row, task_arn) in &launched {
        let arm = row.arm;
        let (lines, task) = stopped_task(&router, case, task_arn).await;
        let container = task["containers"][0].clone();
        assert_eq!(
            container["exitCode"].as_i64(),
            Some(row.refused_status),
            "case {case:?} arm {arm:?}: expected exitCode {:?} on the refused call, got \
             {container} with {lines:?}",
            row.refused_status
        );
        assert_eq!(
            task["stopCode"].as_str(),
            Some(ESSENTIAL_CONTAINER_EXITED),
            "case {case:?} arm {arm:?}: expected the stop code of an essential container that \
             exited, got {task}"
        );
        assert_eq!(
            task["stoppedReason"].as_str(),
            Some(ESSENTIAL_CONTAINER_EXITED_REASON),
            "case {case:?} arm {arm:?}: expected the reason Amazon ECS words that code with, \
             got {task}"
        );
        assert!(
            lines.iter().any(|line| line.contains(ACCESS_DENIED)),
            "case {case:?} arm {arm:?}: expected the owning service's own refusal in what the \
             container wrote, got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains(row.refusal_line)),
            "case {case:?} arm {arm:?}: expected the CLI's own account of the refused call \
             ({:?}) in what the container wrote, got {lines:?}",
            row.refusal_line
        );
        assert_eq!(
            reported(&lines, "STATUS"),
            printed_status(row, row.refused_status),
            "case {case:?} arm {arm:?}: expected the program to read the status of the call it \
             ran, got {lines:?}"
        );
        let held = held_object(&router, row.key).await;
        assert!(
            held.is_none(),
            "case {case:?} arm {arm:?}: the refused call must have stored nothing, got {held:?}"
        );
    }

    // The repair: the ONE statement the calls need, attached to the same role through IAM's own
    // API — nothing about the task definitions changes.
    grant(
        &router,
        case,
        role_name,
        "write-the-objects",
        &["s3:PutObject"],
        &format!("arn:aws:s3:::{BUCKET}/*"),
    )
    .await;

    // PHASE 2 — the same revisions launched again: every call is made, so every container ends
    // with the status of a call that was made and every object holds the bytes it carried.
    let mut repaired = Vec::new();
    for row in &rows {
        repaired.push((row, run_task(&router, case, row.family, &subnet, None).await));
    }
    for (row, task_arn) in &repaired {
        let arm = row.arm;
        let (lines, task) = stopped_task(&router, case, task_arn).await;
        let container = task["containers"][0].clone();
        assert_eq!(
            container["exitCode"].as_i64(),
            Some(CALL_MADE),
            "case {case:?} arm {arm:?}: expected exitCode {CALL_MADE:?} once the grant is in \
             force, got {container} with {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains(ACCESS_DENIED)),
            "case {case:?} arm {arm:?}: expected no refusal once the grant is in force, got \
             {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains(row.refusal_line)),
            "case {case:?} arm {arm:?}: expected no failed call once the grant is in force, \
             got {lines:?}"
        );
        assert_eq!(
            reported(&lines, "STATUS"),
            printed_status(row, CALL_MADE),
            "case {case:?} arm {arm:?}: expected the program to read the status of the call it \
             ran, got {lines:?}"
        );
        let held = held_object(&router, row.key).await;
        assert_eq!(
            held.as_deref().map(str::trim),
            Some(EXIT_STATUS_PAYLOAD),
            "case {case:?} arm {arm:?}: expected the object to hold what the container wrote, \
             got {held:?}"
        );
    }
}
