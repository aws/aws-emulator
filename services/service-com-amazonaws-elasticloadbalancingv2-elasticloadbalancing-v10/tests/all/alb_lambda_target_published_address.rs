// SPDX-License-Identifier: CC0-1.0

//! A `forward` to a `lambda` target group over the ADDRESS THE LOAD BALANCER PUBLISHES: what
//! a client that dials `LoadBalancer$DNSName` on a real connection is answered with.
//!
//! The rule under test: **the address a load balancer publishes answers a client that dials
//! it, and a request its listener forwards to a `lambda` target group is served by INVOKING
//! the function the group registers — the client is answered from the object that function
//! returned only while that object satisfies the Lambda-target response contract, and a
//! function that breaks it earns the load balancer's own HTTP 502 with its payload never
//! passed on. The function RAN either way: the 502 is produced AFTER the invocation.**
//!
//! `LoadBalancer$DNSName` is "the public DNS name of the load balancer" — the address a
//! client then dials — `TargetGroup$TargetType` gives `lambda` as "Register a single Lambda
//! function as a target", and `TargetDescription$Id` is "the ARN of the Lambda function" for
//! such a group. The 502 for a payload the load balancer cannot interpret is recorded
//! behaviour.
//!
//! What this suite drives that its sibling [`crate::alb_lambda_target`] does not: that
//! sibling dispatches the client's request into the router by the published name, so it never
//! touches the SOCKET the published address is served on
//! (`src/service/listener_socket.rs`) — the connection, its framing and the leg that socket
//! hands the data plane. Here every row is a real TCP connection to the published authority,
//! speaking HTTP/1.1 as a client does, and the function is a real deployment package created
//! through Lambda's OWN API, run by the Lambda emulator mounted on this suite's engine, as a
//! session of an execution role IAM holds.
//!
//! That the function ran is asserted where nothing of this service can fake it: the handler
//! reports every invocation to an Amazon SQS queue through its own SDK — authorized by the
//! one grant its execution role carries — before it decides what to return, so a row answered
//! without the handler having been reached leaves the queue empty and fails.
//!
//! The rows differ along what the rule generalises over: the CLASS of contract violation (no
//! status code, a payload that is no response object at all, a status outside the HTTP range,
//! a response past the size a Lambda target may answer with, a run that raised) and the shape
//! of a well-formed answer (a body and media type of the function's own, no body at all),
//! with a well-formed row last so a refusal is seen to cost that one request and no more.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::alb_data_plane::{create_load_balancer, settle};
use crate::provisioning_loop::{action, call, p, REGION};

/// The function every row runs, and the module and attribute its package carries the handler
/// as.
const FUNCTION: &str = "alb-lambda-published";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the package is created for.
const RUNTIME: &str = "python3.12";

/// The execution role the function runs as, and the queue the handler reports to.
const ROLE_NAME: &str = "alb-lambda-published-exec";
const SIDE_CHANNEL: &str = "alb-lambda-published-side-channel";

/// The body a well-formed answer carries, which is the function's OWN and must never be what
/// a refused response is answered with.
const TARGET_BODY: &str = "TARGET-OK";

/// The status a load balancer answers for a Lambda-target response its contract refuses.
const BAD_GATEWAY: u16 = 502;

/// The most bytes a Lambda target's response may carry, which the oversized row is built
/// from so it exercises the bound rather than a number guessed at.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// How long a row waits for work the clock owes before it is a failure, how long between two
/// looks while waiting, and how long one dial and one exchange may take. Generous on purpose:
/// everything waited for happens in this process, so a loaded host does not decide a verdict.
const BUDGET: Duration = Duration::from_secs(30);
const LOOK_AGAIN: Duration = Duration::from_millis(25);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The handler every row runs. It reports the invocation to the side channel FIRST — so a row
/// answered without the handler having run leaves the queue empty — and then decides what to
/// return from the LAST SEGMENT of the path the event carries, so one package serves every
/// row.
const HANDLER_SOURCE: &str = r#"
import os

import boto3

LIMIT = 1024 * 1024


def lambda_handler(event, context):
    path = (event.get("path") or "/").rsplit("/", 1)[-1]
    boto3.client("sqs").send_message(
        QueueUrl=os.environ["QUEUE_URL"], MessageBody=path or "/"
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

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `lambda_function.py`, written in
/// memory the way a caller uploads one.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// This service, Lambda, Amazon SQS, IAM and the token service over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the function the load balancer forwards to is run by
/// that Lambda as a session of the role IAM holds, and the handler's own SDK call is
/// dispatched back into this same router, so the queue it reports to is the queue this suite
/// reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
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
    (router, engine)
}

/// One IAM call (`awsQuery`: a form body, an XML answer).
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&crate::provisioning_loop::encode(name));
        body.push('=');
        body.push_str(&crate::provisioning_loop::encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<tag>` element of an XML body.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"))
        + open.len();
    let end = body[start..]
        .find(&format!("</{tag}>"))
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"))
        + start;
    body[start..end].to_string()
}

/// The execution role the function runs as: a role Lambda may assume, carrying the one grant
/// the handler's own call needs. Answers the ARN IAM persisted.
async fn execution_role(router: &MultiServiceRouter, queue_arn: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateRole: expected 200, got {status} body {created}"
    );
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string();
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME),
            ("PolicyName", "report-invocations"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup PutRolePolicy: expected 200, got {status} body {written}"
    );
    element(&created, "Arn")
}

/// One Lambda `restJson1` call, as its status and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://lambda.{REGION}.amazonaws.com{path}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`), the way a client
/// makes it.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Every message the side channel holds, deleted as it is read, so the next row starts from
/// an empty queue. One wait for the FIRST message, then one more look, so a row that reported
/// two invocations is seen to have reported two.
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

/// What one exchange on the published address observed.
struct Answered {
    status: u16,
    /// The whole answer as it came off the connection, status line and headers included.
    raw: String,
}

/// Dial the published address and exchange one request on it, as a client of that address
/// does: a connection in the clear, HTTP/1.1, with the authority it dialled as `Host`.
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
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
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

/// One row: the path the client asks for, the status the load balancer must answer, and
/// whether the answer carries the function's own body — which a refused response never does.
struct Row {
    case: &'static str,
    target: &'static str,
    status: u16,
    serves_target_body: bool,
}

/// Every row's answer must come from the records, and every row must have invoked the function
/// exactly once. The paths carry a PREFIX, as a client's own path does.
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

/// RULE: the address a load balancer publishes answers a client that dials it, and a
/// `forward` to a `lambda` target group is served by invoking the registered function — the
/// load balancer's own 502 for a response its contract refuses, the function's own answer
/// otherwise, and the invocation having happened either way.
#[tokio::test]
async fn the_published_address_answers_a_lambda_target_and_generates_its_own_502() {
    let (router, engine) = emulator();

    // The side channel the handler reports every invocation to, and the role whose one grant
    // admits that report.
    let (status, queue) = sqs(&router, "CreateQueue", &json!({ "QueueName": SIDE_CHANNEL })).await;
    assert_eq!(
        status, 200,
        "setup CreateQueue: expected 200, got {status} {queue}"
    );
    let queue_url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateQueue: no QueueUrl in {queue}"))
        .to_string();
    let (status, attributes) = sqs(
        &router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup GetQueueAttributes: expected 200, got {status} {attributes}"
    );
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup GetQueueAttributes: no QueueArn in {attributes}"))
        .to_string();
    let role = execution_role(&router, &queue_arn).await;

    // The function the load balancer will forward to, created through Lambda's own API.
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": role,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Environment": { "Variables": { "QUEUE_URL": queue_url } },
            "Code": { "ZipFile": aws_smithy_types::base64::encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created}"
    );
    let function_arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateFunction: no FunctionArn in {created}"))
        .to_string();
    // The activation span Lambda owes the function, placed rather than slept for — the same
    // reading every suite that invokes a freshly created function uses.
    {
        let state = engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    // The load balancer and the address it publishes, the `lambda` target group, the
    // registration of the function, the grant that admits the invocation, and the HTTP:80
    // listener that forwards to the group — every one of them through the owning API.
    let (lb_arn, published) = create_load_balancer(&router, "lambda-published-alb").await;

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "lambda-published-tg"));
    form.push(p("TargetType", "lambda"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateTargetGroup: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("setup CreateTargetGroup: no TargetGroupArn in {}", reply.body));

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", &function_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup RegisterTargets: expected 200, got {} body {}",
        reply.status, reply.body
    );

    let (status, granted) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": "elbv2-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": tg_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission: expected 201, got {status} {granted}"
    );

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

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
        // A refused response is the LOAD BALANCER's own answer: the oversized payload the
        // function returned is not proxied either.
        if row.status == BAD_GATEWAY {
            assert!(
                answered.raw.len() < MAX_RESPONSE_BYTES,
                "case {:?}: the refused payload must not be proxied, got {} byte(s)",
                row.case,
                answered.raw.len()
            );
        }
        // The function RAN, exactly once, whatever the load balancer answered.
        let mode = row
            .target
            .rsplit('/')
            .next()
            .expect("a target names a last segment");
        let seen = reported(&router, row.case, &queue_url, BUDGET).await;
        assert_eq!(
            seen,
            vec![mode.to_string()],
            "case {:?}: expected exactly one invocation reported as {mode:?}, got {seen:?}",
            row.case
        );
    }
}
