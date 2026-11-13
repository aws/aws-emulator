// SPDX-License-Identifier: CC0-1.0

//! The Application Load Balancer attribute `routing.http.drop_invalid_header_fields.enabled`,
//! end to end: what the attribute's domain admits, what a read reports after a refused write,
//! and which of a client's request headers reach the target behind the load balancer.
//!
//! The rule under test: **a load balancer that carries
//! `routing.http.drop_invalid_header_fields.enabled` at `true` removes from the request every
//! header whose name is not written in the character set a header field name is written in —
//! the alphanumeric characters and the hyphen — before anything behind the load balancer sees
//! it, and forwards the request and every one of its remaining headers, values included,
//! unchanged; carried at `false`, which is the value the attribute's documentation gives it,
//! every header the client sent reaches the target. The attribute takes `true` or `false` and
//! nothing else: another value is refused and NOT stored, so a read keeps reporting the value
//! the attribute was carried at.**
//!
//! The rows span what the rule generalises over: the character classes a field name can be
//! written in (letters of either case, digits, the hyphen, and one row per class of character
//! HTTP's own token grammar admits that a header name is not written in — the underscore, the
//! dot and the other token punctuation), each of them sent in ONE request, under each state
//! the attribute can be in (never written, `true`, written back to `false`).
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router to
//! the published `DNSName`, carrying no AWS credential, exactly as a client sends it — and
//! what the target saw is reported by the TARGET: a Lambda function created through Lambda's
//! own API, run by the Lambda emulator mounted on this suite's engine, which echoes the
//! headers of the event it was handed. Nothing here is read out of a record a test invented.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, send, settle};
use crate::provisioning_loop::{action, attribute_sequence, call, first_text, p, REGION};
use crate::target_group_attributes_and_modification::unescaped;

/// The attribute under test.
const ATTRIBUTE: &str = "routing.http.drop_invalid_header_fields.enabled";

/// The function the load balancer forwards to, and the module and attribute its package
/// carries the handler as.
const FUNCTION: &str = "alb-header-echo";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the package is created for.
const RUNTIME: &str = "python3.12";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The handler every case runs: it answers with the headers of the event it was handed, so
/// what the TARGET saw is what the suite reads. A header the load balancer removed is absent
/// from this document, and one it passed on carries the value the client sent.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json"},
        "body": json.dumps(event.get("headers") or {}),
    }
"#;

/// One row of the table: the case, the header name the client sends, the value it sends it
/// with, and whether the name is written in the character set a header field name is written
/// in — which is what decides whether the attribute drops it.
struct Row {
    case: &'static str,
    name: &'static str,
    value: &'static str,
    valid: bool,
}

/// Every header one request carries, spanning the character classes a field name can be
/// written in and the classes of character HTTP's token grammar admits that it cannot.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a hyphenated name",
            name: "x-echo-good",
            value: "kept-hyphenated",
            valid: true,
        },
        Row {
            case: "mixed case and digits",
            name: "X-Echo-42",
            value: "kept-mixed-case",
            valid: true,
        },
        Row {
            case: "letters only, with no separator at all",
            name: "xecho",
            value: "kept-letters",
            valid: true,
        },
        Row {
            case: "an underscore, as an application framework spells a variable",
            name: "x_echo_bad",
            value: "dropped-underscore",
            valid: false,
        },
        Row {
            case: "a dot",
            name: "x.echo.bad",
            value: "dropped-dot",
            valid: false,
        },
        Row {
            case: "a dollar sign, which HTTP's token grammar admits",
            name: "x$echo",
            value: "dropped-dollar",
            valid: false,
        },
        Row {
            case: "a tilde, which HTTP's token grammar admits",
            name: "x~echo",
            value: "dropped-tilde",
            valid: false,
        },
        Row {
            case: "a plus sign, which HTTP's token grammar admits",
            name: "x+echo",
            value: "dropped-plus",
            valid: false,
        },
    ]
}

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

/// This service and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the function the load balancer forwards to is run by that Lambda.
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
    (router, engine)
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

/// Write one value of the attribute, and answer the status and body of the write.
async fn modify(router: &MultiServiceRouter, lb_arn: &str, value: &str) -> (u16, String) {
    let mut form = action("ModifyLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Attributes.member.1.Key", ATTRIBUTE));
    form.push(p("Attributes.member.1.Value", value));
    let reply = call(router, &form).await;
    (reply.status, reply.body)
}

/// What `DescribeLoadBalancerAttributes` reports the attribute at.
async fn described(router: &MultiServiceRouter, lb_arn: &str) -> String {
    let mut form = action("DescribeLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", lb_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "DescribeLoadBalancerAttributes: expected 200, got {} body {}",
        reply.status, reply.body
    );
    attribute_sequence(&reply.body)
        .into_iter()
        .find(|(key, _)| key == ATTRIBUTE)
        .map(|(_, value)| value)
        .unwrap_or_else(|| {
            panic!("DescribeLoadBalancerAttributes: no {ATTRIBUTE} in {}", reply.body)
        })
}

/// The headers the TARGET was handed, as the function echoed them: one request carrying every
/// row's header.
async fn seen_by_the_target(
    router: &MultiServiceRouter,
    case: &str,
    dns_name: &str,
    rows: &[Row],
) -> serde_json::Map<String, Value> {
    let headers: Vec<(&str, &str)> = rows.iter().map(|row| (row.name, row.value)).collect();
    let answered = send(router, "GET", dns_name, "/", &headers).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected the target's HTTP 200, got {} with body {:?}",
        answered.status, answered.body
    );
    let echoed: Value = serde_json::from_str(&answered.body).unwrap_or_else(|error| {
        panic!("case {case:?}: the target's echo is not JSON ({error}) in {:?}", answered.body)
    });
    match echoed {
        Value::Object(headers) => headers,
        other => panic!("case {case:?}: the target's echo is no object, got {other}"),
    }
}

#[tokio::test]
async fn a_load_balancer_drops_invalid_header_fields_only_while_the_attribute_is_enabled() {
    let (router, engine) = emulator();

    // The target: a function created through Lambda's own API, which echoes the headers of
    // the event it is handed.
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

    // The load balancer, the `lambda` target group holding the function, and the HTTP:80
    // listener that forwards to it: each through this service's own API.
    let (lb_arn, dns_name) = create_load_balancer(&router, "drop-headers-alb").await;

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "drop-headers-tg"));
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

    // The grant that admits the load balancer's invocation, written through Lambda's own
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

    let rows = rows();

    // The attribute as its documentation carries it, with nothing written: every header the
    // client sent reaches the target.
    let carried = described(&router, &lb_arn).await;
    assert_eq!(
        carried, "false",
        "case the attribute as it is carried: expected {ATTRIBUTE} at \"false\", got \
         {carried:?}"
    );
    let seen = seen_by_the_target(&router, "the attribute never written", &dns_name, &rows).await;
    for row in &rows {
        let lowered = row.name.to_ascii_lowercase();
        let delivered = seen.get(&lowered).and_then(Value::as_str);
        assert_eq!(
            delivered,
            Some(row.value),
            "case {:?} with the attribute never written: every header reaches the target, \
             expected {lowered:?} at {:?}, got {delivered:?} in {seen:?}",
            row.case,
            row.value
        );
    }

    // The attribute enabled: the headers whose names are no field name are gone, and every
    // other header of the SAME request is delivered with its value untouched.
    let (status, body) = modify(&router, &lb_arn, "true").await;
    assert_eq!(
        status, 200,
        "case the attribute enabled: expected 200, got {status} body {body}"
    );
    let carried = described(&router, &lb_arn).await;
    assert_eq!(
        carried, "true",
        "case the attribute enabled: expected the write to be reported, got {carried:?}"
    );
    let seen = seen_by_the_target(&router, "the attribute enabled", &dns_name, &rows).await;
    for row in &rows {
        let lowered = row.name.to_ascii_lowercase();
        let delivered = seen.get(&lowered).and_then(Value::as_str);
        let expected = row.valid.then_some(row.value);
        assert_eq!(
            delivered, expected,
            "case {:?} with the attribute enabled: expected {lowered:?} at {expected:?}, got \
             {delivered:?} in {seen:?}",
            row.case
        );
    }

    // A value outside the attribute's domain is refused and NOT stored: the read keeps
    // reporting the value the attribute was carried at, and the target keeps seeing what
    // that value has it see.
    for refused in ["yes-please", "TRUE", "1", ""] {
        let (status, body) = modify(&router, &lb_arn, refused).await;
        assert_eq!(
            status, 400,
            "case the refused value {refused:?}: expected 400, got {status} body {body}"
        );
        let code = first_text(&body, "Code").unwrap_or_default();
        assert_eq!(
            code, "ValidationError",
            "case the refused value {refused:?}: expected the ValidationError code, got \
             {code:?} in {body}"
        );
        let message = unescaped(&first_text(&body, "Message").unwrap_or_default());
        let expected =
            format!("The value of '{ATTRIBUTE}' must be 'true' or 'false', but was '{refused}'");
        assert_eq!(
            message, expected,
            "case the refused value {refused:?}: expected {expected:?}, got {message:?}"
        );
        let carried = described(&router, &lb_arn).await;
        assert_eq!(
            carried, "true",
            "case the refused value {refused:?}: the refused value is not stored, expected the \
             prior \"true\", got {carried:?}"
        );
    }
    let seen = seen_by_the_target(&router, "after the refused writes", &dns_name, &rows).await;
    for row in &rows {
        let lowered = row.name.to_ascii_lowercase();
        let delivered = seen.get(&lowered).and_then(Value::as_str);
        let expected = row.valid.then_some(row.value);
        assert_eq!(
            delivered, expected,
            "case {:?} after the refused writes: expected {lowered:?} at {expected:?}, got \
             {delivered:?} in {seen:?}",
            row.case
        );
    }

    // The attribute written back to the value its documentation carries it at: every header
    // of the same request reaches the target again.
    let (status, body) = modify(&router, &lb_arn, "false").await;
    assert_eq!(
        status, 200,
        "case the attribute disabled again: expected 200, got {status} body {body}"
    );
    let seen = seen_by_the_target(&router, "the attribute disabled again", &dns_name, &rows).await;
    for row in &rows {
        let lowered = row.name.to_ascii_lowercase();
        let delivered = seen.get(&lowered).and_then(Value::as_str);
        assert_eq!(
            delivered,
            Some(row.value),
            "case {:?} with the attribute disabled again: every header reaches the target, \
             expected {lowered:?} at {:?}, got {delivered:?} in {seen:?}",
            row.case,
            row.value
        );
    }
}
