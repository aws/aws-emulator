// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 web ACL in front of an Application Load Balancer whose listener forwards to a
//! `lambda` TARGET GROUP: who answers the request, and whether the registered function runs
//! at all.
//!
//! The rule under test: **a request to the name a load balancer publishes is decided by the
//! web ACL associated with that load balancer's ARN BEFORE any listener action is performed —
//! a terminating action other than `Allow` is answered at the edge, under that action's own
//! status and `CustomResponse` where it configures one, and the listener's action never runs,
//! so nothing behind it is reached; a request the web ACL lets through is served by the
//! listener's action, and where that action is a `forward` to a `lambda` target group it is
//! served by INVOKING the function the group registers and answering from the object that
//! function returned, carrying whatever a `CustomRequestHandling` inserted into the request.**
//!
//! Nothing behind the listener is a record a test invented: the function is a real deployment
//! package created through Lambda's OWN API and run by the Lambda emulator mounted on this
//! suite's engine, and whether it RAN is read where nothing of this service can fake it — the
//! handler reports every invocation to an Amazon SQS queue through its own SDK before it
//! decides what to return, so the queue's contents after a row are a consequence of the
//! handler having been reached.
//!
//! Every row ends the same way, which is what makes a row's report list conclusive: the
//! binding is taken away and the same client sends one PROBE request, which the function must
//! serve. The handler reports an invocation before the load balancer can answer anything, so
//! by the time the probe's own report is on the queue every report the row earned is there
//! too — a row whose refusal invoked the function would show its path beside the probe's and
//! fail, with no wait on the clock to decide it.
//!
//! The rows differ along what the rule generalises over: the request component the rule
//! inspects (a single header, the URI path, the query string, the method, the body), a
//! matching and a non-matching request for each, the terminating action (`Block`, `Captcha`,
//! `Challenge`), a block carrying its own custom response, a rule whose action is not
//! terminating, the web ACL's `DefaultAction` deciding instead of a rule, an `Allow` that
//! inserts a header the FUNCTION then sees in its event, and no web ACL at all — the
//! unprotected baseline a client establishes before it associates anything.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, settle, Answered};
use crate::provisioning_loop::{action, call, p, REGION};
use crate::web_acl_enforcement::{
    byte_match, create_web_acl, rule, waf_ok, WebAcl, BLOCK_HEADER, BLOCK_STATUS, CAPTCHA_STATUS,
    CHALLENGE_STATUS, INSERTED_HEADER, INSERTED_HEADER_SENT, INSERTED_VALUE,
};

/// The function the one `lambda` target group registers, and the module and attribute its
/// package carries the handler as.
const FUNCTION: &str = "waf-lambda-target";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the package is created for.
const RUNTIME: &str = "python3.12";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The queue the handler reports each invocation to.
const SIDE_CHANNEL: &str = "waf-lambda-side-channel";

/// The body the function answers with, which is the TARGET's own: no status the edge answers
/// may carry it.
const TARGET_BODY: &str = "TARGET-OK";

/// The path of the probe every row ends with, sent with the binding taken away.
const PROBE_PATH: &str = "/probe";

/// How long a row waits for work the clock owes before it is a failure, and how long between
/// two looks while waiting. Generous: the row is waiting for a condition, not for a duration.
const BUDGET: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The handler the one function runs. It reports the path of the event it was handed to the
/// side channel FIRST, so a request answered without the handler having run leaves that
/// report absent, and then answers with the header a web ACL's `CustomRequestHandling`
/// inserted where the event carries one — which is how a row sees that an allowed request
/// reached the TARGET carrying it.
///
/// The header name it reads is the placeholder [`handler_source`] fills with the spelling WAF
/// puts that header on the request under, so the two cannot drift apart.
const HANDLER_SOURCE: &str = r#"
import os

import boto3

QUEUE_URL = os.environ["QUEUE_URL"]


def lambda_handler(event, context):
    path = event.get("path") or "/"
    boto3.client("sqs").send_message(QueueUrl=QUEUE_URL, MessageBody=path)
    inserted = (event.get("headers") or {}).get("<INSERTED-HEADER>")
    return {
        "statusCode": 200,
        "statusDescription": "200 OK",
        "headers": {"content-type": "text/plain"},
        "body": inserted if inserted is not None else "TARGET-OK",
        "isBase64Encoded": False,
    }
"#;

/// [`HANDLER_SOURCE`] with the inserted header's own name in it.
fn handler_source() -> String {
    HANDLER_SOURCE.replace("<INSERTED-HEADER>", INSERTED_HEADER_SENT)
}

/// A .zip deployment package holding [`handler_source`] as `lambda_function.py`, written in
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
            .write_all(handler_source().as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// This service, WAFv2, Lambda and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the binding is WAFv2's record, the function is run by
/// that Lambda, and the handler's own SDK call is dispatched back into this same router, so
/// the queue it reports to is the queue this suite reads.
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
        service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One request to the name the load balancer published, carrying no credential, as a client
/// of that name sends it.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{target}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("the data-plane request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Answered {
        status,
        headers,
        body,
    }
}

/// One Lambda `restJson1` call.
async fn lambda(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
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

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`), the way a
/// client makes it.
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

/// Every invocation the side channel holds, deleted as it is read so the next row starts from
/// an empty queue, sorted so a row compares a SET of reports.
///
/// The queue is looked at again until the probe's own report — the one every row is certain to
/// earn — has been read and a further look finds nothing, or the generous budget above runs
/// out; the assertion is the caller's.
async fn reported(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let deadline = Instant::now() + BUDGET;
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
        let probe_read = seen.iter().any(|path| path == PROBE_PATH);
        if (probe_read && batch.is_empty()) || Instant::now() >= deadline {
            seen.sort();
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the web ACL in front of the load balancer (none at all for `web_acl: None`), the
/// request the client sends, what must come back, and whether the function behind the
/// listener was reached by it.
struct Row {
    case: &'static str,
    web_acl: Option<WebAcl>,
    method: &'static str,
    /// The path, query string included, within the load balancer's own endpoint.
    target: &'static str,
    headers: &'static [(&'static str, &'static str)],
    /// The request body the client sends.
    request_body: &'static str,
    status: u16,
    /// The body the answer must carry, verbatim.
    body: &'static str,
    /// The media type the answer must declare, when the row pins one.
    content_type: Option<&'static str>,
    /// Response headers the answer must carry, for a row whose refusal configures some.
    response_headers: &'static [(&'static str, &'static str)],
    /// Whether the function must have been invoked for THIS row's own request.
    invoked: bool,
}

/// A row whose request is a plain `GET /served` with no body, answered by the function.
fn served(case: &'static str, web_acl: Option<WebAcl>) -> Row {
    Row {
        case,
        web_acl,
        method: "GET",
        target: "/served",
        headers: &[],
        request_body: "",
        status: 200,
        body: TARGET_BODY,
        content_type: Some("text/plain"),
        response_headers: &[],
        invoked: true,
    }
}

/// A row whose request the edge refuses under `status`, with no body and no media type of its
/// own, so the function is never reached.
fn refused(case: &'static str, web_acl: Option<WebAcl>, status: u16) -> Row {
    Row {
        case,
        web_acl,
        method: "GET",
        target: "/served",
        headers: &[(BLOCK_HEADER, "1")],
        request_body: "",
        status,
        body: "",
        content_type: None,
        response_headers: &[],
        invoked: false,
    }
}

/// A rule carrying `rule_action` over the single header the recorded blocking rule inspects.
fn on_header(name: &'static str, rule_action: Value) -> Value {
    rule(
        name,
        byte_match(
            json!({ "SingleHeader": { "Name": BLOCK_HEADER } }),
            "1",
            "EXACTLY",
        ),
        rule_action,
    )
}

/// Every row of the table.
fn rows() -> Vec<Row> {
    vec![
        // The unprotected baseline: nothing is in front of the load balancer, and the
        // listener's `forward` to the `lambda` group is served by the function.
        served("no web ACL in front of the load balancer", None),
        // A web ACL with nothing to match: its `DefaultAction` allows, so the target serves.
        served(
            "a web ACL with no rules and a default action that allows",
            Some(WebAcl::allowing(json!([]))),
        ),
        // The recorded case: a `Block` matching the request, and the two requests the same
        // web ACL must NOT refuse.
        refused(
            "a Block rule on a single header, the header matching",
            Some(WebAcl::allowing(json!([on_header(
                "block-header",
                json!({ "Block": {} }),
            )]))),
            BLOCK_STATUS,
        ),
        served(
            "a Block rule on a single header, the header absent",
            Some(WebAcl::allowing(json!([on_header(
                "block-header",
                json!({ "Block": {} }),
            )]))),
        ),
        Row {
            headers: &[(BLOCK_HEADER, "2")],
            ..served(
                "a Block rule on a single header, the header carrying another value",
                Some(WebAcl::allowing(json!([on_header(
                    "block-header",
                    json!({ "Block": {} }),
                )]))),
            )
        },
        // The same decision over every other request component a `FieldToMatch` names, each
        // with a matching and a non-matching request.
        Row {
            headers: &[],
            ..refused(
                "a Block rule on the URI path, the path matching",
                Some(WebAcl::allowing(json!([rule(
                    "block-path",
                    byte_match(json!({ "UriPath": {} }), "/served", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
            )
        },
        Row {
            target: "/elsewhere",
            ..served(
                "a Block rule on the URI path, another path",
                Some(WebAcl::allowing(json!([rule(
                    "block-path",
                    byte_match(json!({ "UriPath": {} }), "/served", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
            )
        },
        Row {
            headers: &[],
            target: "/served?drop=all",
            ..refused(
                "a Block rule on the query string, a query string containing the value",
                Some(WebAcl::allowing(json!([rule(
                    "block-query",
                    byte_match(json!({ "QueryString": {} }), "drop", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
            )
        },
        Row {
            target: "/served?keep=all",
            ..served(
                "a Block rule on the query string, another query string",
                Some(WebAcl::allowing(json!([rule(
                    "block-query",
                    byte_match(json!({ "QueryString": {} }), "drop", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
            )
        },
        Row {
            headers: &[],
            method: "POST",
            request_body: "a body the client sent",
            ..refused(
                "a Block rule on the method, the method matching",
                Some(WebAcl::allowing(json!([rule(
                    "block-method",
                    byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
            )
        },
        served(
            "a Block rule on the method, another method",
            Some(WebAcl::allowing(json!([rule(
                "block-method",
                byte_match(json!({ "Method": {} }), "POST", "EXACTLY"),
                json!({ "Block": {} }),
            )]))),
        ),
        Row {
            headers: &[],
            method: "POST",
            request_body: "carrying a secret the edge looks for",
            ..refused(
                "a Block rule on the body, the body containing the value",
                Some(WebAcl::allowing(json!([rule(
                    "block-body",
                    byte_match(json!({ "Body": {} }), "secret", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
                BLOCK_STATUS,
            )
        },
        Row {
            method: "POST",
            request_body: "carrying nothing the edge looks for",
            ..served(
                "a Block rule on the body, another body",
                Some(WebAcl::allowing(json!([rule(
                    "block-body",
                    byte_match(json!({ "Body": {} }), "secret", "CONTAINS"),
                    json!({ "Block": {} }),
                )]))),
            )
        },
        // The other two terminating actions, each under the status WAF serves it with: the
        // function is reached by neither.
        refused(
            "a Captcha rule that matches, with no token on the request",
            Some(WebAcl::allowing(json!([on_header(
                "captcha-header",
                json!({ "Captcha": {} }),
            )]))),
            CAPTCHA_STATUS,
        ),
        refused(
            "a Challenge rule that matches, with no token on the request",
            Some(WebAcl::allowing(json!([on_header(
                "challenge-header",
                json!({ "Challenge": {} }),
            )]))),
            CHALLENGE_STATUS,
        ),
        // A block that configures its OWN response is served by WAF, with that response's
        // status, body, media type and headers — and still nothing is invoked.
        Row {
            body: "refused at the edge",
            content_type: Some("text/plain"),
            response_headers: &[("x-refused-by", "waf"), ("content-type", "text/plain")],
            ..refused(
                "a Block rule carrying its own custom response",
                Some(WebAcl {
                    default_action: json!({ "Allow": {} }),
                    rules: json!([on_header(
                        "block-header",
                        json!({
                            "Block": {
                                "CustomResponse": {
                                    "ResponseCode": 503,
                                    "CustomResponseBodyKey": "refused",
                                    "ResponseHeaders": [
                                        { "Name": "x-refused-by", "Value": "waf" }
                                    ],
                                }
                            }
                        }),
                    )]),
                    custom_response_bodies: Some(json!({
                        "refused": {
                            "ContentType": "TEXT_PLAIN",
                            "Content": "refused at the edge",
                        }
                    })),
                }),
                503,
            )
        },
        // The web ACL's own default action decides a request no rule matched: nothing behind
        // the listener is reached by it either.
        refused(
            "a web ACL whose default action blocks",
            Some(WebAcl {
                default_action: json!({ "Block": {} }),
                rules: json!([]),
                custom_response_bodies: None,
            }),
            BLOCK_STATUS,
        ),
        // A `Count` action is not terminating: the request goes on to the default action,
        // which allows it, so the function serves it.
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            ..served(
                "a Count rule that matches, under a default action that allows",
                Some(WebAcl::allowing(json!([on_header(
                    "count-header",
                    json!({ "Count": {} }),
                )]))),
            )
        },
        // An `Allow` that inserts a header: the request reaches the TARGET carrying it, and
        // the function answers with the value it was handed.
        Row {
            body: INSERTED_VALUE,
            ..served(
                "an Allow rule inserting a header the function's own event carries",
                Some(WebAcl::allowing(json!([rule(
                    "tag-everything",
                    byte_match(json!({ "UriPath": {} }), "/served", "EXACTLY"),
                    json!({
                        "Allow": {
                            "CustomRequestHandling": {
                                "InsertHeaders": [
                                    { "Name": INSERTED_HEADER, "Value": INSERTED_VALUE }
                                ]
                            }
                        }
                    }),
                )]))),
            )
        },
        // The same request with no web ACL in front: nothing inserts that header, so the
        // function answers its own body — the inserted header above was the edge's.
        served("no web ACL, so nothing inserts that header", None),
        // And a request carrying the header the blocked rows set, with nothing in front of
        // the load balancer at all: the refusals were the web ACL's, not this listener's.
        Row {
            headers: &[(BLOCK_HEADER, "1")],
            ..served("no web ACL, the header the blocked rows set", None)
        },
    ]
}

/// The path within the load balancer's endpoint, without the query string, which is what the
/// `path` of the event the function is handed carries.
fn event_path(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

#[tokio::test]
async fn a_web_acl_decides_a_request_before_the_listener_forwards_it_to_its_lambda_target() {
    let (router, engine) = emulator();

    // The side channel the handler reports every invocation to.
    let (status, queue) = sqs(&router, "CreateQueue", &json!({ "QueueName": SIDE_CHANNEL })).await;
    assert_eq!(
        status, 200,
        "setup CreateQueue: expected 200, got {status} {queue}"
    );
    let queue_url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateQueue: no QueueUrl in {queue}"))
        .to_string();

    // The function the load balancer forwards to, created through Lambda's own API.
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
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
    // The activation span Lambda owes the function, placed rather than slept for.
    {
        let state = engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    // The load balancer, the `lambda` target group, the registration of the function and the
    // HTTP:80 listener that forwards to the group: every one of them through this service's
    // own API.
    let (lb_arn, dns_name) = create_load_balancer(&router, "waf-lambda-alb").await;

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "waf-lambda-tg"));
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

    // The grant that admits the load balancer's invocation: the function's own
    // resource-based policy, narrowed to this target group, written through Lambda's own
    // `AddPermission` as a client grants it.
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

    for (index, row) in rows().into_iter().enumerate() {
        // The binding this row is decided under, written through WAFv2's own
        // `AssociateWebACL` over the load balancer's ARN. An association REPLACES the one
        // before it, so a row with a web ACL needs no teardown of the last one.
        let bound = match &row.web_acl {
            Some(spec) => {
                let web_acl_arn =
                    create_web_acl(&router, &format!("waf-lambda-acl-{index}"), spec).await;
                waf_ok(
                    &router,
                    "AssociateWebACL",
                    json!({ "ResourceArn": lb_arn, "WebACLArn": web_acl_arn }),
                )
                .await;
                true
            }
            None => false,
        };

        let answered = send(
            &router,
            row.method,
            &dns_name,
            row.target,
            row.headers,
            row.request_body,
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            row.case, row.status, answered.status, answered.body
        );
        assert_eq!(
            answered.body, row.body,
            "case {:?}: expected body {:?}, got {:?}",
            row.case, row.body, answered.body
        );
        let declared = answered.header("content-type").map(str::to_string);
        assert_eq!(
            declared.as_deref(),
            row.content_type,
            "case {:?}: expected content-type {:?}, got {declared:?}",
            row.case,
            row.content_type
        );
        for (name, value) in row.response_headers {
            assert_eq!(
                answered.header(name),
                Some(*value),
                "case {:?}: expected header {name}: {value:?}, got {:?}",
                row.case,
                answered.header(name)
            );
        }
        if !row.invoked {
            assert!(
                !answered.body.contains(TARGET_BODY),
                "case {:?}: the edge's own answer must carry nothing of the target, got {:?}",
                row.case,
                answered.body
            );
        }

        // The binding taken away, and one PROBE the function must serve: the baseline a
        // client establishes before it associates anything, re-established after every row.
        if bound {
            waf_ok(
                &router,
                "DisassociateWebACL",
                json!({ "ResourceArn": lb_arn }),
            )
            .await;
        }
        let probed = send(&router, "GET", &dns_name, PROBE_PATH, &[], "").await;
        assert_eq!(
            (probed.status, probed.body.as_str()),
            (200, TARGET_BODY),
            "case {:?}: with the binding gone the target must serve the probe, got {} {:?}",
            row.case,
            probed.status,
            probed.body
        );

        // And what RAN: the probe always, this row's own request only where the web ACL let
        // it through to the listener's `forward`.
        let mut expected = vec![PROBE_PATH.to_string()];
        if row.invoked {
            expected.push(event_path(row.target).to_string());
        }
        expected.sort();
        let reported = reported(&router, row.case, &queue_url).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected exactly the invocations {expected:?}, got {reported:?}",
            row.case
        );
    }
}
