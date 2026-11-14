// SPDX-License-Identifier: CC0-1.0

//! A rule's Lambda target is invoked ASYNCHRONOUSLY, so what the handler itself comes to is no
//! part of the DELIVERY — end to end over the wire, with EventBridge, Lambda, Amazon SQS, AWS
//! Systems Manager and CloudWatch Logs on ONE engine and a real `python3.x` handler that
//! writes an effect and then raises.
//!
//! The RULE pinned here: EventBridge hands a matched event to a Lambda target by invoking the
//! function asynchronously ("Event: invoke the function asynchronously"), so the delivery is
//! complete once Lambda has accepted the invocation and the handler's OWN outcome cannot make
//! it fail — a target whose handler raises is dead-lettered NOWHERE at the rule, the effects
//! the handler managed before raising stand, and the exception surfaces only in the function's
//! own log group. What IS a delivery failure is a handover EventBridge is not permitted to
//! make: for a function target "EventBridge relies on resource-based policies"
//! (`PutTargets`), so a function whose policy does not admit `events.amazonaws.com`
//! `lambda:InvokeFunction` for the invoking rule is never invoked at all, the attempt is
//! retried within the target's `RetryPolicy` and the original envelope is then written to the
//! queue the target's `DeadLetterConfig` names, carrying `ERROR_CODE` `NO_PERMISSIONS` beside
//! the `RULE_ARN` and `TARGET_ARN` of the delivery that failed.
//!
//! That is the distinction a client's remediation branches on: a dead letter means EventBridge
//! never got the event to the function, never that the function ran and failed. An emulator
//! that dead-lettered a raising handler would let code which reads a DLQ record as "the target
//! never ran" pass, and one that dead-letters nothing at all hides a withheld permission
//! entirely.
//!
//! The rows span the class rather than one case. They differ along the FORM of the grant in
//! force (none, unscoped, scoped by `aws:SourceArn` to the invoking rule), along what the
//! handler DOES (returns normally, raises after its own effect has been written, raises before
//! writing anything), and along the `RetryPolicy` the target carries — `MaximumRetryAttempts`
//! at the bottom of its published range and at its published top (`@range(min: 0, max: 185)`),
//! and `MaximumEventAgeInSeconds` at both of its bounds (`@range(min: 60, max: 86400)`), per
//! `smithy_query(query="shape", shape="RetryPolicy")`.
//!
//! Every absence this suite asserts is PACED by something positive rather than by a wait: each
//! row also wires a second rule of the same bus to a second function that is granted nothing,
//! whose dead-letter record is therefore owed. Once that record is on the queue the dispatcher
//! has already decided and dead-lettered this publication, so a record owed to the subject
//! rule would be on the same queue by then.
//!
//! `lambda_target_handler_records_the_invoking_rule` is the suite where a permitted target's
//! handler runs and the invoking rule's identity reaches it; what this one adds is that the
//! handler's own FAILURE is not the rule's.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The `source` and `detail-type` every event this suite publishes carries.
const SOURCE: &str = "com.demo.async-handover";
const DETAIL_TYPE: &str = "Async Handover Probe";

/// The principal EventBridge invokes a resource-policy-governed target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The IAM action an invocation of a function performs, which is what a grant for this service
/// has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The id the one statement a row's grant is written under carries.
const STATEMENT_ID: &str = "events-invoke";

/// The `ERROR_CODE` EventBridge stamps on an event it was not permitted to deliver, as the
/// LITERAL it writes rather than through the service's own constant.
const NO_PERMISSIONS: &str = "NO_PERMISSIONS";

/// The module and attribute the deployment package carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The id of the one target each rule of a row carries.
const TARGET_ID: &str = "target-1";

/// How long this suite waits for work the clock owes — a function leaving `Pending`, an
/// asynchronous invocation, a handler's own write, a dead letter — before giving up on it.
const WORK_WAIT: Duration = Duration::from_secs(60);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// The exception type the handler raises, and the sentence it raises it with — the marker of
/// the event being processed is the variable part, so the line a row looks for in the log is
/// that row's own.
const ERROR_TYPE: &str = "RuntimeError";

fn failure_sentence(marker: &str) -> String {
    format!("the handler could not process {marker}")
}

/// The line the python runtime reports an unhandled exception on the invocation's stderr as,
/// which is what reaches the function's log group.
fn error_line(marker: &str) -> String {
    format!("[ERROR] {ERROR_TYPE}: {}", failure_sentence(marker))
}

/// The handler every row deploys. What it does is taken from the EVENT: it writes the event's
/// marker into Parameter Store when the event asks it to, and raises when the event asks it to
/// — the write coming first, so a row can tell an effect that was made before the exception
/// from one that was never made at all.
const PROBE_SOURCE: &str = r#"
import boto3


def lambda_handler(event, context):
    detail = event["detail"]
    marker = detail["marker"]
    if detail["record"]:
        boto3.client("ssm").put_parameter(
            Name=detail["parameter"], Value=marker, Type="String", Overwrite=True
        )
    if detail["outcome"] == "raise":
        raise RuntimeError("the handler could not process %s" % marker)
    return {"marker": marker}
"#;

/// A .zip deployment package holding the handler above as `lambda_function.py`, written in
/// memory the way a caller uploads one and base64-encoded as `FunctionCode$ZipFile` carries it.
fn package() -> String {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(PROBE_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
}

/// EventBridge, Lambda, Amazon SQS, AWS Systems Manager and CloudWatch Logs over ONE shared
/// engine, as `core/router/src/lib.rs` mounts them: the handler's own SDK call is dispatched
/// back into this same router, and the log its run produces is read through that service's own
/// `FilterLogEvents`.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Systems Manager call (`awsJson1_1`, `AmazonSSM.<Operation>`), as its status and parsed
/// body: a read of a name no parameter answers to is that service's refusal, so the status is
/// answered rather than asserted.
async fn ssm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "ssm", "AmazonSSM", "1.1", operation, body).await
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), as its status and
/// parsed body: a group no function has written to yet is that service's refusal.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "logs", "Logs_20140328", "1.1", operation, body).await
}

/// A Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The account every emulated resource of this suite belongs to, as the engine holds it.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The log group a function's invocations are logged to: "By default, Lambda functions send
/// logs to a default log group named `/aws/lambda/<function name>`"
/// (`LoggingConfig$LogGroup`).
fn log_group(function: &str) -> String {
    format!("/aws/lambda/{function}")
}

/// Create one function running the probe handler, and answer the ARN Lambda assigned it.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": format!("arn:aws:iam::{}:role/service-role/lambda-exec", account()),
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 20,
            "Code": { "ZipFile": package() },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FunctionArn, got {created}"))
        .to_string()
}

/// What Lambda reports as the function's `State`, or `None` while the read does not answer.
async fn function_state(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, read) = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        Value::Null,
    )
    .await;
    if status != 200 {
        return None;
    }
    read["State"].as_str().map(ToString::to_string)
}

/// Wait until the function is `Active`. "During this time, you can't invoke or modify the
/// function" (`CreateFunction`): the created function leaves `Pending` by the clock, so this
/// LOOKS for the state rather than waiting a span out, and a handover withheld afterwards is
/// the policy's refusal and not the lifecycle's.
async fn until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        if function_state(router, name).await.as_deref() == Some("Active") {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let state = function_state(router, name).await;
    panic!("case {case:?}: expected the function {name:?} to become Active, got {state:?}");
}

/// Add one statement to the function's resource-based policy through `AddPermission`, which is
/// how a client grants this service the invocation.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    source_arn: Option<&str>,
) {
    let mut body = json!({
        "StatementId": STATEMENT_ID,
        "Action": INVOKE_ACTION,
        "Principal": EVENTS_PRINCIPAL,
    });
    if let Some(source_arn) = source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    let (status, answered) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/policy"),
        body,
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} ({answered})"
    );
}

/// The queue both targets of a row dead-letter to, carrying the policy this service's own send
/// needs; answers its URL and its ARN.
async fn dead_letter_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        router,
        case,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    (url, arn)
}

/// An ENABLED rule on the default bus matching one marker, answered with its ARN.
async fn create_rule(router: &MultiServiceRouter, case: &str, name: &str, marker: &str) -> String {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": { "marker": [marker] },
    })
    .to_string();
    let rule = events(
        router,
        case,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    rule["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn, got {rule}"))
        .to_string()
}

/// Wire one function as the rule's only target, dead-lettering to `dead_letter_arn` under the
/// retry policy the row carries.
async fn wire_target(
    router: &MultiServiceRouter,
    case: &str,
    rule: &str,
    function_arn: &str,
    dead_letter_arn: &str,
    retry: &Value,
) {
    let wired = events(
        router,
        case,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [{
                "Id": TARGET_ID,
                "Arn": function_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
                "RetryPolicy": retry,
            }],
        }),
    )
    .await;
    assert_eq!(
        wired["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the target to be accepted, got {wired}"
    );
}

/// One entry of a publication: the detail the handler reads its instructions out of.
fn entry(marker: &str, parameter: &str, record: bool, raises: bool) -> Value {
    json!({
        "Source": SOURCE,
        "DetailType": DETAIL_TYPE,
        "Detail": json!({
            "marker": marker,
            "parameter": parameter,
            "record": record,
            "outcome": if raises { "raise" } else { "return" },
        })
        .to_string(),
    })
}

/// What Systems Manager reports for `name`, or `None` while no parameter answers to it.
async fn recorded(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, answered) = ssm(router, "GetParameter", &json!({ "Name": name })).await;
    if status != 200 {
        return None;
    }
    answered["Parameter"]["Value"]
        .as_str()
        .map(ToString::to_string)
}

/// Wait until the parameter `name` is there, which is the handler having run and written it.
async fn until_recorded(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        if let Some(value) = recorded(router, name).await {
            return Some(value);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    recorded(router, name).await
}

/// Every line the function's log group holds, as `FilterLogEvents` reports them — empty for a
/// group nothing has written to, which is what a function that never ran leaves behind.
async fn log_lines(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, answered) = logs(router, "FilterLogEvents", &json!({ "logGroupName": group })).await;
    if status != 200 {
        return Vec::new();
    }
    answered["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until the function's log group holds a line carrying `needle`, and answer every line
/// it then holds.
async fn until_logged(router: &MultiServiceRouter, group: &str, needle: &str) -> Vec<String> {
    until_log_line(router, group, |line| line.contains(needle)).await
}

/// Wait until the function's log group holds any line at all, which is the invocation's own
/// log having been delivered, and answer every line it then holds.
async fn until_any_logged(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    until_log_line(router, group, |_| true).await
}

/// Wait until the group holds a line `wanted` accepts, and answer every line it then holds.
async fn until_log_line(
    router: &MultiServiceRouter,
    group: &str,
    wanted: impl Fn(&str) -> bool,
) -> Vec<String> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let held = log_lines(router, group).await;
        if held.iter().any(|line| wanted(line)) {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    log_lines(router, group).await
}

/// One pass of the dead-letter queue, taking everything waiting on it off.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answered = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    answered["Messages"].as_array().cloned().unwrap_or_default()
}

/// The marker the envelope one dead-letter message carries names, which is the event the
/// delivery failed for.
fn dead_letter_marker(message: &Value) -> Option<String> {
    let body = message["Body"].as_str()?;
    let envelope: Value = serde_json::from_str(body).ok()?;
    envelope["detail"]["marker"]
        .as_str()
        .map(ToString::to_string)
}

/// Keep taking messages off the dead-letter queue until one carrying `marker` has been seen,
/// accumulating everything taken off it into `held`.
async fn until_dead_lettered(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    marker: &str,
    held: &mut Vec<Value>,
) {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        held.extend(drain(router, case, url).await);
        if held
            .iter()
            .any(|message| dead_letter_marker(message).as_deref() == Some(marker))
        {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let seen: Vec<Option<String>> = held.iter().map(dead_letter_marker).collect();
    panic!(
        "case {case:?}: expected the undeliverable event {marker:?} on the dead-letter queue, \
         got the markers {seen:?}"
    );
}

/// The one message of `held` whose envelope carries `marker`.
fn record_for<'a>(held: &'a [Value], marker: &str) -> Option<&'a Value> {
    held.iter()
        .find(|message| dead_letter_marker(message).as_deref() == Some(marker))
}

/// Assert that the dead-letter record for `marker` names the delivery it failed for: the
/// condition, the rule and the target.
fn assert_no_permissions_record(
    held: &[Value],
    case: &str,
    marker: &str,
    rule_arn: &str,
    target_arn: &str,
) {
    let record = record_for(held, marker)
        .unwrap_or_else(|| panic!("case {case:?}: no dead letter for {marker:?} in {held:?}"));
    let attributes = &record["MessageAttributes"];
    assert_eq!(
        attributes["ERROR_CODE"]["StringValue"],
        json!(NO_PERMISSIONS),
        "case {case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS:?}, got {record}"
    );
    assert_eq!(
        attributes["RULE_ARN"]["StringValue"],
        json!(rule_arn),
        "case {case:?}: expected the dead-letter RULE_ARN {rule_arn:?}, got {record}"
    );
    assert_eq!(
        attributes["TARGET_ARN"]["StringValue"],
        json!(target_arn),
        "case {case:?}: expected the dead-letter TARGET_ARN {target_arn:?}, got {record}"
    );
    // The refusal travels with it, so a client reads WHY the delivery was not made rather than
    // only that it was not.
    let reported = attributes["ERROR_MESSAGE"]["StringValue"]
        .as_str()
        .unwrap_or_default();
    let expected_opening = format!(
        "User: {EVENTS_PRINCIPAL} is not authorized to perform: {INVOKE_ACTION} on resource: \
         {target_arn}"
    );
    assert!(
        reported.starts_with(&expected_opening),
        "case {case:?}: expected the dead letter to carry the refusal opening \
         {expected_opening:?}, got {reported:?}"
    );
}

/// What a row's grant on the subject function says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No `AddPermission` at all, so the function carries no policy.
    None,
    /// The service principal allowed the invocation, unscoped.
    Unscoped,
    /// The same grant, scoped by `aws:SourceArn` to the rule that invokes the function.
    ThisRule,
}

/// One row: the grant in force on the subject function, what its handler does, and the retry
/// policy its target carries.
struct Row {
    case: &'static str,
    grant: Grant,
    /// Whether the handler raises instead of returning.
    raises: bool,
    /// Whether the handler writes its parameter before that.
    record: bool,
    /// The target's `RetryPolicy`.
    retry: Value,
}

/// Everything one row's fixtures amount to, so the two tests below set a row up the same way.
struct Wiring {
    router: MultiServiceRouter,
    /// The queue both targets dead-letter to.
    dead_letter_url: String,
    /// The rule under test and the function it targets.
    subject_rule_arn: String,
    subject_function: String,
    subject_function_arn: String,
    subject_marker: String,
    /// The parameter the subject's handler writes when the event asks it to.
    parameter: String,
    /// The rule whose delivery is owed a dead letter, which paces every absence asserted here,
    /// and the function it targets.
    pacer_rule_arn: String,
    pacer_function_arn: String,
    pacer_marker: String,
}

/// Stand a row up and publish its two entries, answering what the assertions read.
async fn given(row: &Row, index: usize) -> Wiring {
    let case = row.case;
    let router = router();
    let subject_function = format!("async-handover-subject-{index}");
    let pacer_function = format!("async-handover-pacer-{index}");
    let subject_rule = format!("async-handover-rule-{index}");
    let pacer_rule = format!("async-handover-pacer-rule-{index}");
    let subject_marker = format!("marker-subject-{index}");
    let pacer_marker = format!("marker-pacer-{index}");
    let parameter = format!("/emulator/async-handover/{index}");

    let (dead_letter_url, dead_letter_arn) =
        dead_letter_queue(&router, case, &format!("async-handover-dlq-{index}")).await;
    let subject_function_arn = create_function(&router, case, &subject_function).await;
    let pacer_function_arn = create_function(&router, case, &pacer_function).await;

    let subject_rule_arn = create_rule(&router, case, &subject_rule, &subject_marker).await;
    let pacer_rule_arn = create_rule(&router, case, &pacer_rule, &pacer_marker).await;

    match row.grant {
        Grant::None => {}
        Grant::Unscoped => add_permission(&router, case, &subject_function, None).await,
        Grant::ThisRule => {
            add_permission(&router, case, &subject_function, Some(&subject_rule_arn)).await;
        }
    }

    wire_target(
        &router,
        case,
        &subject_rule,
        &subject_function_arn,
        &dead_letter_arn,
        &row.retry,
    )
    .await;
    // The pacer's target is granted nothing, so its delivery is owed a dead letter whatever
    // the subject's comes to.
    wire_target(
        &router,
        case,
        &pacer_rule,
        &pacer_function_arn,
        &dead_letter_arn,
        &json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 }),
    )
    .await;

    until_active(&router, case, &subject_function).await;

    let published = events(
        &router,
        case,
        "PutEvents",
        &json!({
            "Entries": [
                entry(&subject_marker, &parameter, row.record, row.raises),
                entry(&pacer_marker, &parameter, false, false),
            ],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected both entries to be accepted, got {published}"
    );

    Wiring {
        router,
        dead_letter_url,
        subject_rule_arn,
        subject_function,
        subject_function_arn,
        subject_marker,
        parameter,
        pacer_rule_arn,
        pacer_function_arn,
        pacer_marker,
    }
}

/// The rows whose subject target IS permitted: the handler runs, and whatever it comes to the
/// rule dead-letters nothing.
fn permitted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "an unscoped grant, and a handler that returns normally",
            grant: Grant::Unscoped,
            raises: false,
            record: true,
            // The bottom of the published range: one attempt and no retry.
            retry: json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 }),
        },
        Row {
            case: "a grant scoped to the invoking rule, and a handler that raises after \
                   writing its own effect",
            grant: Grant::ThisRule,
            raises: true,
            record: true,
            retry: json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 }),
        },
        Row {
            case: "a grant scoped to the invoking rule, and a handler that raises before \
                   writing anything",
            grant: Grant::ThisRule,
            raises: true,
            record: false,
            retry: json!({ "MaximumRetryAttempts": 2 }),
        },
        Row {
            case: "an unscoped grant, a handler that raises, and the published maximum retry \
                   allowance",
            grant: Grant::Unscoped,
            raises: true,
            record: false,
            // Both members at the top of their published ranges.
            retry: json!({ "MaximumRetryAttempts": 185, "MaximumEventAgeInSeconds": 86400 }),
        },
    ]
}

/// The rows whose subject target is NOT permitted: nothing is invoked, so what the handler
/// would have done makes no difference to the delivery.
fn refused_rows() -> Vec<Row> {
    vec![
        Row {
            case: "no grant at all, and a handler that would have returned",
            grant: Grant::None,
            raises: false,
            record: true,
            retry: json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 }),
        },
        Row {
            case: "no grant at all, and a handler that would have raised",
            grant: Grant::None,
            raises: true,
            record: true,
            retry: json!({ "MaximumRetryAttempts": 185, "MaximumEventAgeInSeconds": 86400 }),
        },
    ]
}

/// RULE: a permitted Lambda target is invoked asynchronously, so the delivery is already made
/// when the handler runs — a handler that raises is dead-lettered nowhere at the rule, its
/// exception reaches only the function's own log group, and an effect it made before raising
/// stands.
#[tokio::test]
async fn a_permitted_lambda_targets_handler_failure_is_not_dead_lettered_at_the_rule() {
    for (index, row) in permitted_rows().iter().enumerate() {
        let case = row.case;
        let wiring = given(row, index).await;
        let router = &wiring.router;
        let group = log_group(&wiring.subject_function);

        // The handler RAN: a raising one is reported on the invocation's own log, and a
        // returning one is waited for at the effect it writes.
        if row.raises {
            let needle = error_line(&wiring.subject_marker);
            let lines = until_logged(router, &group, &needle).await;
            assert!(
                lines.iter().any(|line| line.contains(&needle)),
                "case {case:?}: expected the handler's exception {needle:?} in {group}, got \
                 {lines:?}"
            );
        }
        if row.record {
            let value = until_recorded(router, &wiring.parameter).await;
            assert_eq!(
                value.as_deref(),
                Some(wiring.subject_marker.as_str()),
                "case {case:?}: expected the handler's own effect to stand, so {:?} holds {:?}, \
                 got {value:?}",
                wiring.parameter,
                wiring.subject_marker
            );
        } else {
            // A handler that raised before writing anything left nothing behind, which is what
            // tells this row's exception from the row above's.
            let value = recorded(router, &wiring.parameter).await;
            assert!(
                value.is_none(),
                "case {case:?}: expected the raising handler to have written nothing, got \
                 {value:?}"
            );
        }
        // Whatever the handler came to, its run is on the function's own log.
        let lines = until_any_logged(router, &group).await;
        assert!(
            !lines.is_empty(),
            "case {case:?}: expected the invocation's log in {group}, got nothing"
        );

        // The pacer's delivery is the one that WAS refused, so its dead letter is owed: once it
        // is on the queue the dispatcher has decided this publication, and a record owed to the
        // subject would be on the same queue.
        let mut held = Vec::new();
        until_dead_lettered(
            router,
            case,
            &wiring.dead_letter_url,
            &wiring.pacer_marker,
            &mut held,
        )
        .await;
        assert_no_permissions_record(
            &held,
            case,
            &wiring.pacer_marker,
            &wiring.pacer_rule_arn,
            &wiring.pacer_function_arn,
        );
        held.extend(drain(router, case, &wiring.dead_letter_url).await);

        // The assertion this suite exists for: the handler's own outcome is not the rule's, so
        // nothing of the subject's event is on the dead-letter queue.
        let subject = record_for(&held, &wiring.subject_marker);
        assert!(
            subject.is_none(),
            "case {case:?}: an asynchronous invocation completes the delivery, so the subject \
             event {:?} is never dead-lettered at the rule, got {subject:?}",
            wiring.subject_marker
        );
    }
}

/// RULE: a handover the function's resource-based policy does not admit is a DELIVERY failure —
/// nothing is invoked, and the envelope goes to the target's dead-letter queue under
/// `NO_PERMISSIONS` with the rule and target it failed for — whatever the handler would have
/// done had it run.
#[tokio::test]
async fn an_unpermitted_lambda_target_is_dead_lettered_under_no_permissions() {
    for (index, row) in refused_rows().iter().enumerate() {
        let case = row.case;
        // A fresh offset per test, so two suites' resource names never collide.
        let wiring = given(row, index + permitted_rows().len()).await;
        let router = &wiring.router;

        let mut held = Vec::new();
        until_dead_lettered(
            router,
            case,
            &wiring.dead_letter_url,
            &wiring.subject_marker,
            &mut held,
        )
        .await;
        assert_no_permissions_record(
            &held,
            case,
            &wiring.subject_marker,
            &wiring.subject_rule_arn,
            &wiring.subject_function_arn,
        );
        // The pacer's record is owed too, and seeing it is what shows the queue has had
        // everything this publication owes it.
        until_dead_lettered(
            router,
            case,
            &wiring.dead_letter_url,
            &wiring.pacer_marker,
            &mut held,
        )
        .await;

        // Nothing ran: no log line and no effect, so the dead letter reports a delivery that
        // was never made rather than one that was made and failed.
        let group = log_group(&wiring.subject_function);
        let lines = log_lines(router, &group).await;
        assert!(
            lines.is_empty(),
            "case {case:?}: expected no invocation at all, so {group} holds no line, got \
             {lines:?}"
        );
        let value = recorded(router, &wiring.parameter).await;
        assert!(
            value.is_none(),
            "case {case:?}: expected no invocation at all, so no parameter was written, got \
             {value:?}"
        );
        // And exactly the two refused deliveries were dead-lettered, once each.
        held.extend(drain(router, case, &wiring.dead_letter_url).await);
        let markers: Vec<Option<String>> = held.iter().map(dead_letter_marker).collect();
        assert_eq!(
            markers.len(),
            2,
            "case {case:?}: expected one dead letter per refused delivery, got {markers:?}"
        );
    }
}
