// SPDX-License-Identifier: CC0-1.0

//! What a Lambda handler run as the WORK of a Step Functions `Task` can do with the AWS SDK
//! the managed Python runtime puts on its import path: build a client for a service the
//! runtime's call seam binds and change that service's own state.
//!
//! RULE: a managed runtime ships the AWS SDK with a client for every service its call seam
//! binds, so `boto3.client(<token>)` for one of them CONSTRUCTS — it is never refused as an
//! unknown service — and the operations the handler calls on it reach that service and leave
//! their effect in it, whatever protocol that service speaks; a `Task` state whose work is
//! such a handler therefore ends the task on the handler's own result, and a chain of them
//! ends the execution `SUCCEEDED`.
//!
//! Five crates have to agree for that to hold — Step Functions interprets the chain and makes
//! each invocation, IAM holds the role each call is judged against, Lambda runs the handler and
//! supplies it the SDK, and Amazon S3 and DynamoDB take the handler's own writes — so the suite
//! lives here: a crate-local suite can only ever see one side of it. The Lambda crate's
//! `handler_sdk_service_inventory` suite pins WHICH tokens the runtime's SDK answers to, and
//! `sfn_lambda_task_invocation` pins that the task's invocation happens at all; what only this
//! suite can reach is the EFFECT a task's handler left in another service, read back through
//! that service's own API once the execution has finished.
//!
//! The rows differ along what the rule generalises over: the form of the payload the handler
//! writes (a JSON document, text outside ASCII whose UTF-8 is longer than its characters, an
//! empty document, a multi-kilobyte document), the bytes it writes as a BLOB (bytes no text
//! encoding carries, no bytes at all, a kilobyte of them), and the shape of the key they land
//! under (a nested prefix, one carrying a dot and an underscore). Every row drives the same
//! three-task chain, so both protocol classes — the object store's REST protocol and the
//! key-value store's AWS JSON one — are exercised per row, and each row asserts both halves:
//! what the execution reports, and what each service holds afterwards.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{form_encode, opt_element};

/// The Region every request here is signed for, and so the Region the bucket, the table, the
/// function and the state machine belong to.
const REGION: &str = "us-east-1";

/// The bucket every row's handler writes its objects to.
const BUCKET: &str = "xsvc-task-tenant-assets";

/// The table every row's handler records the provisioned tenant in, and the name of its one
/// key attribute.
const TABLE: &str = "xsvc-task-tenants";
const TABLE_KEY: &str = "tenant";

/// The status the last task of the chain records for a tenant it provisioned.
const PROVISIONED: &str = "PROVISIONED";

/// The terminal status an execution whose every task ended reaches
/// (`ExecutionStatus`: `SUCCEEDED`), and the one it sits in while tasks are still running.
const SUCCEEDED: &str = "SUCCEEDED";
const RUNNING: &str = "RUNNING";

/// The `State` a function reports once it can be invoked (`FunctionConfiguration$State`:
/// "Active" once "the function is ready to be invoked").
const FUNCTION_ACTIVE: &str = "Active";

/// The stack the function is declared in, and the `StackStatus` it must reach.
const FUNCTION_STACK: &str = "xsvc-task-provisioner-stack";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host, because what is under test is the writes the handler makes and not how
/// fast an interpreter gets through it.
const FUNCTION_TIMEOUT: u64 = 120;

/// How often to look while waiting for work the clock owes — a created table reaching
/// `ACTIVE`, a created function reaching [`FUNCTION_ACTIVE`], an execution leaving
/// [`RUNNING`] — and how many looks to take. The spans are those services' own, so no fixed
/// sleep names the instant one ends, and the deadline is generous so a loaded host cannot
/// decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// The handler every task of the chain runs: it builds a client with the SDK the runtime
/// supplies and performs the one write the step it was handed names, reporting what the
/// service answered.
///
/// Each step reads its own result back through the SAME client, so a call that answered
/// without doing the work is visible in the task's result as well as in the service.
const PROVISIONING_HANDLER: &str = r#"
import base64

import boto3

PREFIX = "tenants/"


def handler(event, context):
    tenant = event["tenant"]
    bucket = event["bucket"]
    step = event["step"]
    objects = boto3.client("s3")

    if step == "profile":
        key = PREFIX + tenant + "/profile.json"
        body = event["profile"].encode("utf-8")
        written = objects.put_object(
            Bucket=bucket, Key=key, Body=body, ContentType="application/json"
        )
        return {
            "key": key,
            "url": "s3://%s/%s" % (bucket, key),
            "etagged": bool(written.get("ETag")),
            "length": len(body),
        }

    if step == "manifest":
        key = PREFIX + tenant + "/manifest.bin"
        body = base64.b64decode(event["manifest"])
        objects.put_object(Bucket=bucket, Key=key, Body=body)
        read = objects.get_object(Bucket=bucket, Key=key)
        return {
            "key": key,
            "read_back": base64.b64encode(read["Body"].read()).decode("ascii"),
        }

    items = boto3.client("dynamodb")
    items.put_item(
        TableName=event["table"],
        Item={
            "tenant": {"S": tenant},
            "status": {"S": "PROVISIONED"},
            "profile_url": {"S": event["profile_url"]},
            "manifest_key": {"S": event["manifest_key"]},
        },
    )
    recorded = items.get_item(
        TableName=event["table"],
        Key={"tenant": {"S": tenant}},
        ConsistentRead=True,
    )["Item"]
    return {
        "status": recorded["status"]["S"],
        "url": recorded["profile_url"]["S"],
        "manifest_key": recorded["manifest_key"]["S"],
    }
"#;

/// One row: the tenant provisioned, and the payloads its tasks write.
struct Row {
    case: &'static str,
    /// The tenant id, which is the segment the keys are nested under.
    tenant: &'static str,
    /// The document the first task writes as the tenant's profile.
    profile: String,
    /// The bytes the second task writes as the tenant's manifest.
    manifest: Vec<u8>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a JSON document, and bytes no text encoding carries",
            tenant: "basalt-tenant",
            profile: json!({ "tier": "gold", "seats": 12 }).to_string(),
            manifest: vec![0x00, 0x01, 0x7f, 0xfe, 0xff],
        },
        Row {
            case: "text outside ASCII, whose UTF-8 is longer than its characters, and no \
                   bytes at all",
            tenant: "granite-tenant",
            profile: "tier: café ☕".to_string(),
            manifest: Vec::new(),
        },
        Row {
            case: "an empty document, and a kilobyte of bytes, under a key carrying a dot \
                   and an underscore",
            tenant: "quartz.tenant_7",
            profile: String::new(),
            manifest: (0..1024).map(|byte| (byte % 251) as u8).collect(),
        },
        Row {
            case: "a multi-kilobyte document, and one byte",
            tenant: "slate-tenant",
            profile: "x".repeat(8 * 1024),
            manifest: vec![0x42],
        },
    ]
}

/// The emulator as it is ASSEMBLED, on one engine: a handler's own SDK call is dispatched back
/// into this same router, so the bucket and the table it addresses are the ones this suite
/// reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One response of a REST surface: its status and its body's own bytes.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The body as text, for a refusal or a JSON document.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as the JSON document a `restJson1` answer carries.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or_else(|_| Value::String(self.text()))
        }
    }
}

/// One request against Amazon S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/octet-stream")
        .header("authorization", authorization("s3"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().to_vec(),
    }
}

/// One Lambda `restJson1` request.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().to_vec(),
    }
}

/// One call to an AWS JSON service, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Step Functions call, requiring a 200.
async fn sfn(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "states", "AWSStepFunctions", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One DynamoDB call, requiring a 200.
async fn dynamodb(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) =
        rpc(router, "dynamodb", "DynamoDB_20120810", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One awsQuery call to IAM, requiring a 200, answered with its XML body.
async fn iam(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must answer 200 from IAM, got {status} with {text}"
    );
    text
}

/// One awsQuery call to CloudFormation, answered with its status and its XML body.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("cloudformation.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// A role `service` may assume, carrying `statements` as its inline policy; answers its ARN.
///
/// A state machine's execution role is trusted by Step Functions and a function's own
/// execution role by Lambda — which `CreateFunction` checks — so the trusted service is the
/// caller's.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    service: &str,
    statements: Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        case,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    let arn = opt_element(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole({name:?}) must answer an ARN"));
    let document = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
    iam(
        router,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "inline"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    arn
}

/// Create the table every row's last task records its tenant in, and wait until DynamoDB
/// reports it `ACTIVE` — "you can perform read and write operations only on an ACTIVE table".
async fn given_table(router: &MultiServiceRouter, case: &str) {
    dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{ "AttributeName": TABLE_KEY, "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": TABLE_KEY, "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let mut described = Value::Null;
    for _ in 0..LOOKS {
        described = dynamodb(router, case, "DescribeTable", &json!({ "TableName": TABLE })).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: DynamoDB must report {TABLE:?} ACTIVE, last saw {described}");
}

/// Create the bucket every row's handler writes to.
async fn given_bucket(router: &MultiServiceRouter, case: &str) {
    let answer = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: CreateBucket({BUCKET:?}) must answer 200, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// Deploy the function the chain's tasks invoke, wait until Lambda reports it
/// [`FUNCTION_ACTIVE`], and answer its ARN.
///
/// The handler is declared as a stack's inline source, which CloudFormation archives into the
/// deployment package — the way a function whose handler is written out in a suite comes to
/// exist at all.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
) -> String {
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Provisioner": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": name,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": role_arn,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Code": { "ZipFile": PROVISIONING_HANDLER },
                },
            },
        },
    })
    .to_string();
    let (status, answered) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", FUNCTION_STACK), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({FUNCTION_STACK:?}) must be accepted, got {status} with \
         {answered}"
    );
    // The stack's completion is owed from the instant Lambda reports the function created,
    // which no fixed sleep names: this looks until it reports it.
    let mut reported = stack_status(router, FUNCTION_STACK).await;
    for _ in 0..LOOKS {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK).await;
        reported = stack_status(router, FUNCTION_STACK).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "case {case:?}: the function's stack must reach {CREATE_COMPLETE}, last saw {}",
        reported.1
    );

    let mut arn = String::new();
    let mut configuration = Value::Null;
    for _ in 0..LOOKS {
        let answer = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            &Value::Null,
        )
        .await;
        configuration = answer.json();
        if let Some(reported) = configuration["FunctionArn"].as_str() {
            arn = reported.to_string();
        }
        if configuration["State"].as_str() == Some(FUNCTION_ACTIVE) {
            assert!(
                !arn.is_empty(),
                "case {case:?}: the function must report its ARN, got {configuration}"
            );
            return arn;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: the function must reach {FUNCTION_ACTIVE}, last saw {configuration}");
}

/// The chain every row runs: three `Task` states, each invoking `function_arn` with the step
/// it performs, the tenant read off the execution's own input, and the previous steps' results
/// read off the document the chain has built so far.
fn definition(function_arn: &str) -> String {
    json!({
        "StartAt": "WriteProfile",
        "States": {
            "WriteProfile": {
                "Type": "Task",
                "Resource": function_arn,
                "Parameters": {
                    "step": "profile",
                    "bucket": BUCKET,
                    "tenant.$": "$.tenant",
                    "profile.$": "$.profile",
                },
                "ResultPath": "$.written",
                "Next": "WriteManifest",
            },
            "WriteManifest": {
                "Type": "Task",
                "Resource": function_arn,
                "Parameters": {
                    "step": "manifest",
                    "bucket": BUCKET,
                    "tenant.$": "$.tenant",
                    "manifest.$": "$.manifest",
                },
                "ResultPath": "$.archived",
                "Next": "RecordStatus",
            },
            "RecordStatus": {
                "Type": "Task",
                "Resource": function_arn,
                "Parameters": {
                    "step": "status",
                    "bucket": BUCKET,
                    "table": TABLE,
                    "tenant.$": "$.tenant",
                    "profile_url.$": "$.written.url",
                    "manifest_key.$": "$.archived.key",
                },
                "ResultPath": "$.recorded",
                "End": true,
            },
        },
    })
    .to_string()
}

/// Start one execution with `input` and report it once it has left [`RUNNING`].
async fn ran(
    router: &MultiServiceRouter,
    case: &str,
    machine_arn: &str,
    name: &str,
    input: &Value,
) -> Value {
    let started = sfn(
        router,
        case,
        "StartExecution",
        &json!({
            "stateMachineArn": machine_arn,
            "name": name,
            "input": input.to_string(),
        }),
    )
    .await;
    let execution_arn = started["executionArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: StartExecution must answer an ARN, got {started}")
        })
        .to_string();
    let mut described = Value::Null;
    for _ in 0..LOOKS {
        described = sfn(
            router,
            case,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
        if described["status"].as_str() != Some(RUNNING) {
            return described;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: the execution must leave {RUNNING}, last saw {described}");
}

/// The base64 the SDK carries a blob member as, which is how a row's manifest bytes travel in
/// and out of the handler.
fn base64_of(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

#[tokio::test]
async fn a_task_handler_writes_objects_and_items_with_the_runtimes_own_sdk() {
    let case = "provisioning a tenant from a chain of Lambda tasks";
    let router = build_router();

    given_bucket(&router, case).await;
    given_table(&router, case).await;

    // The function runs as a role allowed the writes its handler makes; the state machine runs
    // as a role allowed the invocation it makes.
    let function_role = given_role(
        &router,
        case,
        "xsvc-task-handler-role",
        "lambda.amazonaws.com",
        json!([
            {
                "Effect": "Allow",
                "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
                "Resource": "*",
            },
            { "Effect": "Allow", "Action": ["s3:*", "dynamodb:*"], "Resource": "*" },
        ]),
    )
    .await;
    let machine_role = given_role(
        &router,
        case,
        "xsvc-task-machine-role",
        "states.amazonaws.com",
        json!([{ "Effect": "Allow", "Action": "lambda:InvokeFunction", "Resource": "*" }]),
    )
    .await;

    let function_arn = given_function(
        &router,
        case,
        "xsvc-task-tenant-provisioner",
        &function_role,
    )
    .await;
    let created = sfn(
        &router,
        case,
        "CreateStateMachine",
        &json!({
            "name": "xsvc-task-tenant-provisioning",
            "definition": definition(&function_arn),
            "roleArn": machine_role,
        }),
    )
    .await;
    let machine_arn = created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateStateMachine must answer an ARN"))
        .to_string();

    for row in rows() {
        let input = json!({
            "tenant": row.tenant,
            "profile": row.profile,
            "manifest": base64_of(&row.manifest),
        });
        let described = ran(
            &router,
            row.case,
            &machine_arn,
            &format!("provision-{}", row.tenant.replace('.', "-")),
            &input,
        )
        .await;

        // The execution itself: a task whose handler could not build its client fails the
        // whole chain, so the status is where that shows first.
        assert_eq!(
            described["status"].as_str(),
            Some(SUCCEEDED),
            "case {:?}: expected the execution to end {SUCCEEDED}, got {} {} {}",
            row.case,
            described["status"],
            described["error"],
            described["cause"]
        );

        let output: Value = serde_json::from_str(described["output"].as_str().unwrap_or("null"))
            .unwrap_or(Value::Null);
        let profile_key = format!("tenants/{}/profile.json", row.tenant);
        let manifest_key = format!("tenants/{}/manifest.bin", row.tenant);

        // What each task reported: the object store answered the write with an entity tag over
        // the bytes the handler encoded, and the handler read its own blob back inside the run.
        assert_eq!(
            output["written"]["length"].as_u64(),
            Some(row.profile.len() as u64),
            "case {:?}: expected the write to carry the encoded document's own length, got {}",
            row.case,
            output["written"]
        );
        assert_eq!(
            output["written"]["etagged"],
            json!(true),
            "case {:?}: expected the object store to answer the write with its ETag, got {}",
            row.case,
            output["written"]
        );
        assert_eq!(
            output["written"]["url"].as_str(),
            Some(format!("s3://{BUCKET}/{profile_key}").as_str()),
            "case {:?}: expected the task to report the object it wrote, got {}",
            row.case,
            output["written"]
        );
        assert_eq!(
            output["archived"]["read_back"].as_str(),
            Some(base64_of(&row.manifest).as_str()),
            "case {:?}: expected the handler to read its own blob back, got {}",
            row.case,
            output["archived"]
        );
        assert_eq!(
            output["recorded"]["status"].as_str(),
            Some(PROVISIONED),
            "case {:?}: expected the last task to report the recorded status, got {}",
            row.case,
            output["recorded"]
        );

        // And what each service HOLDS afterwards, read through its own API: a call that
        // answered without doing the work fails here.
        let stored = s3(&router, "GET", &format!("/{BUCKET}/{profile_key}"), b"").await;
        assert_eq!(
            stored.status,
            200,
            "case {:?}: expected GetObject({profile_key:?}) to answer 200, got {} with {}",
            row.case,
            stored.status,
            stored.text()
        );
        assert_eq!(
            stored.body,
            row.profile.as_bytes(),
            "case {:?}: expected the object to hold the document the handler encoded, got {:?}",
            row.case,
            stored.text()
        );
        let archived = s3(&router, "GET", &format!("/{BUCKET}/{manifest_key}"), b"").await;
        assert_eq!(
            archived.status,
            200,
            "case {:?}: expected GetObject({manifest_key:?}) to answer 200, got {} with {}",
            row.case,
            archived.status,
            archived.text()
        );
        assert_eq!(
            archived.body,
            row.manifest,
            "case {:?}: expected the object to hold the handler's own bytes, got {} byte(s)",
            row.case,
            archived.body.len()
        );

        let item = dynamodb(
            &router,
            row.case,
            "GetItem",
            &json!({
                "TableName": TABLE,
                "Key": { TABLE_KEY: { "S": row.tenant } },
                "ConsistentRead": true,
            }),
        )
        .await;
        assert_eq!(
            item["Item"]["status"]["S"].as_str(),
            Some(PROVISIONED),
            "case {:?}: expected the table to hold the recorded tenant, got {item}",
            row.case
        );
        assert_eq!(
            item["Item"]["profile_url"]["S"].as_str(),
            Some(format!("s3://{BUCKET}/{profile_key}").as_str()),
            "case {:?}: expected the record to name the object written for it, got {item}",
            row.case
        );
        assert_eq!(
            item["Item"]["manifest_key"]["S"].as_str(),
            Some(manifest_key.as_str()),
            "case {:?}: expected the record to name the archived blob, got {item}",
            row.case
        );
    }
}
