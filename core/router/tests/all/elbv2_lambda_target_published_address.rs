// SPDX-License-Identifier: CC0-1.0

//! What a client that dials the address an Application Load Balancer PUBLISHES is answered
//! with when the listener forwards to a `lambda` target group, on the emulator as it is
//! ASSEMBLED.
//!
//! RULE: **a request a client sends to the address a load balancer publishes as its
//! `DNSName` is served by the listener on that address: a `forward` to a `lambda` target
//! group INVOKES the function the group registers, the client is answered from the object
//! that function returned while that object satisfies the Lambda-target response contract,
//! and a function that breaks the contract earns the load balancer's own HTTP 502 with its
//! payload never passed on — the function having RUN either way.**
//!
//! `LoadBalancer$DNSName` is "the public DNS name of the load balancer",
//! `TargetGroup$TargetType` gives `lambda` as "Register a single Lambda function as a
//! target", and `TargetDescription$Id` is "the ARN of the Lambda function" for such a group;
//! the 502 for a response the load balancer cannot interpret is recorded behaviour.
//!
//! Five crates have to agree for that to hold — CloudFormation archives the inline handler
//! into a deployment package this emulator can run, IAM holds the execution role and its
//! inline document, Amazon SQS holds the queue the handler reports to, Lambda runs the
//! handler as a session of that role, and ELBv2 opens the address, evaluates the listener
//! and makes the invocation — so the suite lives here, on the WHOLE assembled service set
//! and over a real connection to the published address. A crate-local suite mounts a handful
//! of services on a router of its own and so can never see the one thing this one asks: that
//! the address the assembled emulator publishes answers at all.
//!
//! That the function RAN is asserted where nothing of ELBv2 can fake it: the handler reports
//! every invocation to a queue through its own SDK before deciding what to return, so a row
//! answered without the handler having been reached leaves the queue empty and fails.
//!
//! The rows differ along what the rule generalises over: the CLASS of contract violation (no
//! status code, a payload that is no response object, a status outside the HTTP range, a run
//! that raised) and the shape of a well-formed answer (a body and a status of the function's
//! own, no body at all), with a well-formed row last so a refusal is seen to cost one request
//! and no more.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{element, form_encode, physical_ids_of};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The API versions the three query protocols carry.
const CFN_VERSION: &str = "2010-05-15";
const EC2_VERSION: &str = "2016-11-15";
const ELB_VERSION: &str = "2015-12-01";

/// The stack the queue, the execution role and the function are deployed as, and the names
/// it declares them under.
const STACK: &str = "alb-lambda-target-estate";
const QUEUE: &str = "alb-lambda-target-side-channel";
const ROLE: &str = "alb-lambda-target-exec";
const FUNCTION: &str = "alb-lambda-target";

/// The load balancer, the target group and the statement the grant is written under.
const LOAD_BALANCER: &str = "alb-lambda-target-lb";
const TARGET_GROUP: &str = "alb-lambda-tg";
const STATEMENT: &str = "elbv2-invoke";

/// The status the load balancer answers a function response its contract refuses.
const BAD_GATEWAY: u16 = 502;

/// The body the well-formed answer carries, which is the function's OWN and must never be
/// what a refused response is answered with.
const TARGET_BODY: &str = "TARGET-OK";

/// The network the load balancer is placed in.
const VPC_CIDR: &str = "10.80.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 2] = [("10.80.1.0/24", "us-east-1a"), ("10.80.2.0/24", "us-east-1b")];

/// How long a wait for work the clock owes may take before it is a failure, and how long
/// between two looks. Generous on purpose: everything waited for happens in this process, so
/// a loaded host does not decide the outcome.
const BUDGET: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How long one dial and one exchange on the published address may take.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The handler every row runs. It reports the invocation to the side channel FIRST, so a row
/// answered without the handler having run leaves the queue empty, and then decides what to
/// return from the path the event carries — one package serves every row, which is what the
/// rule generalises over.
const HANDLER_SOURCE: &str = r#"
import os

import boto3

LIMIT = 1024 * 1024


def handler(event, context):
    whole = event.get("path") or "/"
    path = whole.rsplit("/", 1)[-1]
    boto3.client("sqs").send_message(
        QueueUrl=os.environ["QUEUE_URL"], MessageBody=whole
    )
    if path == "nostatus":
        return {"statusDescription": "200 OK", "body": "TARGET-OK"}
    if path == "nonjson":
        return "not an alb response object at all"
    if path == "badstatus":
        return {"statusCode": 7000, "body": "TARGET-OK"}
    if path == "toolarge":
        return {"statusCode": 200, "body": "x" * (LIMIT + 1)}
    if path == "raise":
        raise RuntimeError("the handler refused to answer")
    if path == "nobody":
        return {"statusCode": 204}
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "text/plain"},
        "body": "TARGET-OK",
    }
"#;

/// The emulator as it is ASSEMBLED, on one engine: the function the load balancer forwards to
/// is run by the Lambda of this same router, and the handler's own SDK call is dispatched
/// back into it, so the queue it reports to is the queue this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    // The address the emulator publishes its data port at, which is what every URL a service
    // hands a caller is built from — the queue URL this suite's handler is given among them.
    // Recorded here as the core records it on boot, so those URLs take the form they take in
    // service.
    ::emulator_server::EmulatorService::serving_at(&router, "https://127.0.0.1:9297");
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded (query-protocol) call of `service`, as its status and its XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One EC2 call that must succeed.
async fn ec2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "ec2", EC2_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must answer 200, got {status} with body {body}"
    );
    body
}

/// One ELBv2 call that must succeed. The credential scope names THIS model's own routing key,
/// which is how an ELBv2 request is addressed at the model rather than at the Classic one it
/// shares an endpoint with.
async fn elbv2(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(
        router,
        "elasticloadbalancingv2",
        ELB_VERSION,
        action,
        members,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ELBv2 {action} must answer 200, got {status} with body {body}"
    );
    body
}

/// One Lambda `restJson1` call, as its status and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Amazon SQS operation over its `awsJson1_0` wire, made as the account's own credential.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("authorization", authorization("sqs"))
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
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

/// The template the suite deploys: the side-channel queue, the execution role carrying the one
/// grant the handler's own call needs, and the function itself. The inline source is archived
/// into the deployment package by CloudFormation, which is how a function whose handler is
/// written here comes to exist at all.
fn stack_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "SideChannel": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": QUEUE },
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "report-invocations",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:SendMessage",
                                "Resource": { "Fn::GetAtt": ["SideChannel", "Arn"] },
                            }],
                        },
                    }],
                },
            },
            "Target": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Timeout": 30,
                    "Environment": {
                        "Variables": { "QUEUE_URL": { "Ref": "SideChannel" } },
                    },
                    "Code": { "ZipFile": HANDLER_SOURCE },
                },
            },
        },
    })
    .to_string()
}

/// Deploy that stack, wait for `CREATE_COMPLETE` and answer the physical id of each resource.
async fn deploy(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let template = stack_template();
    let (status, answered) = query(
        router,
        "cloudformation",
        CFN_VERSION,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateStack: expected 200, got {status} with {answered}"
    );
    let reported = wait_for_stack_status(router, STACK, "CREATE_COMPLETE").await;
    assert_eq!(
        reported.0, "CREATE_COMPLETE",
        "setup: the stack must reach CREATE_COMPLETE, got {} with {}",
        reported.0, reported.1
    );
    let (status, resources) = query(
        router,
        "cloudformation",
        CFN_VERSION,
        "DescribeStackResources",
        &[("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup DescribeStackResources: expected 200, got {status} with {resources}"
    );
    physical_ids_of(&resources)
}

/// Every message the side channel holds, deleted as it is read, so the next row starts from an
/// empty queue. One wait for the FIRST message, then one more look, so a row that reported two
/// invocations is seen to have reported two.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    budget: Duration,
) -> Vec<String> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + budget;
    loop {
        let (status, received) = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ReceiveMessage to answer 200, got {status} {received}"
        );
        let batch = received["Messages"].as_array().cloned().unwrap_or_default();
        for message in &batch {
            seen.push(message["Body"].as_str().unwrap_or_default().to_string());
            let handle = message["ReceiptHandle"].as_str().unwrap_or_default();
            let (status, deleted) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteMessage to answer 200, got {status} {deleted}"
            );
        }
        if !seen.is_empty() && batch.is_empty() {
            return seen;
        }
        if Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Look at `DescribeLoadBalancers` until the load balancer reports the state the clock owes
/// it, which is what a client waits for before sending traffic.
async fn wait_until_active(router: &MultiServiceRouter, arn: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let body = elbv2(router, "DescribeLoadBalancers", &[("LoadBalancerArns.member.1", arn)])
            .await;
        if body.contains("<Code>active</Code>") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the load balancer never became active, last answer {body}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What one answer on the published address carried: its status line and the rest of it.
struct Answered {
    status: u16,
    raw: String,
}

/// Dial the published address and exchange one request on it, as a client of that address
/// does: a connection in the clear, HTTP/1.1, keep-alive.
///
/// Only the PORT is taken from the published authority: the core binds its listeners on
/// loopback (`EMULATOR_BIND_ADDRESS`, `127.0.0.1` by default), so the dial is to this machine.
async fn exchange(case: &str, published: &str, target: &str) -> Answered {
    let port = published
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the published address {published:?} carries no port to dial")
        });
    let mut stream = tokio::time::timeout(
        EXCHANGE_TIMEOUT,
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("case {case:?}: {published} did not accept a connection"))
    .unwrap_or_else(|error| panic!("case {case:?}: {published} refused a connection: {error}"));
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: {published}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the request was not sent: {error}"));
    let mut answer = Vec::new();
    tokio::time::timeout(EXCHANGE_TIMEOUT, stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("case {case:?}: the published address did not answer in time"))
        .unwrap_or_else(|error| panic!("case {case:?}: the answer was not read: {error}"));
    let raw = String::from_utf8_lossy(&answer).to_string();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no status line in {raw:?}"));
    Answered { status, raw }
}

/// One row: the path the client asks for, what the load balancer must answer, and whether the
/// function's own body may appear in that answer.
struct Row {
    case: &'static str,
    target: &'static str,
    status: u16,
    /// Whether the answer carries the function's own body, which a refused response never
    /// does.
    serves_target_body: bool,
}

/// Every row's answer must come from the records, and every row must have invoked the
/// function exactly once. The path carries a PREFIX, as a client's own path does: what the
/// listener answers is decided by the load balancer the address names, not by the shape of
/// the path.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a response naming no statusCode",
            target: "/behind/nostatus",
            status: BAD_GATEWAY,
            serves_target_body: false,
        },
        Row {
            case: "a payload that is no response object at all",
            target: "/behind/nonjson",
            status: BAD_GATEWAY,
            serves_target_body: false,
        },
        Row {
            case: "a statusCode outside the HTTP range",
            target: "/behind/badstatus",
            status: BAD_GATEWAY,
            serves_target_body: false,
        },
        Row {
            case: "a response past the size a Lambda target may answer with",
            target: "/behind/toolarge",
            status: BAD_GATEWAY,
            serves_target_body: false,
        },
        Row {
            case: "a run that raised",
            target: "/behind/raise",
            status: BAD_GATEWAY,
            serves_target_body: false,
        },
        Row {
            case: "a well-formed response naming no body, which the contract allows",
            target: "/behind/nobody",
            status: 204,
            serves_target_body: false,
        },
        Row {
            case: "a well-formed response after every refused one: the target still serves",
            target: "/behind/ok",
            status: 200,
            serves_target_body: true,
        },
    ]
}

/// RULE: the address a load balancer publishes answers a client, and a `forward` to a
/// `lambda` target group is served by invoking the registered function — the load balancer's
/// own 502 for a response its contract refuses, the function's own answer otherwise, and the
/// invocation having happened either way.
#[tokio::test]
async fn the_published_address_serves_a_lambda_target_and_generates_the_502_itself() {
    let router = build_router();
    let resources = deploy(&router).await;
    let queue_url = resources
        .get("SideChannel")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack reports no queue, got {resources:?}"));
    let function_arn = {
        let (status, configuration) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "setup GetFunctionConfiguration: expected 200, got {status} {configuration}"
        );
        configuration["FunctionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: no FunctionArn in {configuration}"))
            .to_string()
    };

    // The network the load balancer is placed in, through EC2 itself.
    let vpc = element(&ec2(&router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await, "vpcId");
    let mut subnets: Vec<String> = Vec::new();
    for (cidr, zone) in SUBNET_CIDRS {
        let created = ec2(
            &router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", cidr),
                ("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push(element(&created, "subnetId"));
    }
    let security_group = element(
        &ec2(
            &router,
            "CreateSecurityGroup",
            &[
                ("GroupName", "alb-lambda-target"),
                ("GroupDescription", "the group of the load balancer"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );

    // The load balancer, and the address it publishes for a client to dial.
    let created = elbv2(
        &router,
        "CreateLoadBalancer",
        &[
            ("Name", LOAD_BALANCER),
            ("Scheme", "internet-facing"),
            ("Subnets.member.1", subnets[0].as_str()),
            ("Subnets.member.2", subnets[1].as_str()),
            ("SecurityGroups.member.1", security_group.as_str()),
        ],
    )
    .await;
    let load_balancer_arn = element(&created, "LoadBalancerArn");
    let published = element(&created, "DNSName");

    // The `lambda` target group, the registration of the function, the grant that admits the
    // invocation, and the listener that forwards to the group.
    let target_group = element(
        &elbv2(
            &router,
            "CreateTargetGroup",
            &[("Name", TARGET_GROUP), ("TargetType", "lambda")],
        )
        .await,
        "TargetGroupArn",
    );
    let (status, granted) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        &json!({
            "StatementId": STATEMENT,
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": target_group,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission: expected 201, got {status} {granted}"
    );
    elbv2(
        &router,
        "RegisterTargets",
        &[
            ("TargetGroupArn", target_group.as_str()),
            ("Targets.member.1.Id", function_arn.as_str()),
        ],
    )
    .await;
    elbv2(
        &router,
        "CreateListener",
        &[
            ("LoadBalancerArn", load_balancer_arn.as_str()),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("DefaultActions.member.1.Type", "forward"),
            ("DefaultActions.member.1.TargetGroupArn", target_group.as_str()),
        ],
    )
    .await;
    wait_until_active(&router, &load_balancer_arn).await;

    for row in rows() {
        let answered = exchange(row.case, &published, row.target).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} in {:?}",
            row.case, row.status, answered.status, answered.raw
        );
        assert_eq!(
            answered.raw.contains(TARGET_BODY),
            row.serves_target_body,
            "case {:?}: expected the function's own body to be served={}, got {:?}",
            row.case,
            row.serves_target_body,
            answered.raw
        );
        // The function RAN, exactly once, whatever the load balancer answered — and the event
        // it was handed named the path THE CLIENT asked for.
        let seen = reported(&router, row.case, &queue_url, BUDGET).await;
        assert_eq!(
            seen,
            vec![row.target.to_string()],
            "case {:?}: expected exactly one invocation reported for the client's own path {:?}, \
             got {seen:?}",
            row.case,
            row.target
        );
    }
}
