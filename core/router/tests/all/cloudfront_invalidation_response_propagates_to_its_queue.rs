// SPDX-License-Identifier: CC0-1.0

//! The response of a `CreateInvalidation` a RUNNING handler submitted, republished onto the
//! event bus and read back off the Amazon SQS queue a second rule delivers it to: EventBridge,
//! Lambda, CloudFront, Amazon SQS, IAM, CloudFormation and Amazon S3 on ONE engine, driven end
//! to end over the assembled router.
//!
//! RULE: the AWS SDK a managed `python3.x` runtime bundles hands the caller a native `datetime`
//! for every member of an answer the called service's model types as a `timestamp` — including
//! one inside a response PAYLOAD, which for a `restXml` service is the whole document the answer
//! carries — so a handler can FORMAT and COMPARE the instant an answer reported: it publishes it
//! as ISO-8601 text, and `CreateInvalidation`'s own `Invalidation` (`Id`, `Status`, `CreateTime`
//! and the `InvalidationBatch` carrying the caller's own `CallerReference`, every one of them
//! `@required` — `smithy_query(query="operation", operation="CreateInvalidation")`) survives that
//! round trip verbatim, with `GetInvalidation` re-serving the same id, the same creation instant
//! and the same caller reference.
//!
//! Only a cross-service suite can state this: the two rules, the match and the handover are
//! EventBridge's, the function, its runtime and the SDK seam it calls through are Lambda's, the
//! invalidation is CloudFront's, the queue and the policy that admits the delivery are Amazon
//! SQS's, and the execution role is IAM's. The suite that pins what the SDK does with one such
//! member on its own is the Lambda crate's
//! (`services/service-com-amazonaws-lambda-awsgirapiservice/tests/all/
//! python_handler_sdk_answer_instants.rs`); what this one adds is the WHOLE chain a client builds
//! out of it — a handler that reacts to an event by invalidating a distribution and republishing
//! the result, whose downstream consumer is keyed on the four values.
//!
//! The rows differ along what the rule generalises over: the form of the batch the handler
//! submits (one exact object, a wildcard over a prefix, several paths at once) and the form of
//! the caller reference the client chose (a plain name, one carrying dots, dashes and digits, and
//! a long one — which has to come back untruncated and still as text).
//!
//! The harness is [`super::lambda_handler_invalidates_its_distribution`]'s, reused rather than
//! repeated: the router, the request helpers, the origin, the distribution and the stack
//! deployment are that suite's, so two readings of one wire cannot drift.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::lambda_handler_invalidates_its_distribution::{
    authorization, build_router, cloudfront, create_distribution, create_origin, deployed,
    wait_until_active, API_VERSION, LOOK_INTERVAL, REGION,
};

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Where Lambda's function routes are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The service principal EventBridge hands a target over as, and the actions a grant for it has
/// to name on each kind of target.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";

/// The pair the first rule matches, which is what a client publishes to ask for an
/// invalidation.
const TRIGGER_SOURCE: &str = "sim2real.dataformat-trigger";
const TRIGGER_DETAIL_TYPE: &str = "invalidation-request";

/// The pair the handler publishes its result under, which is what the second rule matches and
/// forwards to the queue.
const RESULT_SOURCE: &str = "sim2real.invalidator";
const RESULT_DETAIL_TYPE: &str = "invalidation-result";

/// The two rules, the function, the role and the queue of this suite.
const TRIGGER_RULE: &str = "dataformat-trigger-rule";
const FORWARD_RULE: &str = "dataformat-forward-rule";
const FUNCTION: &str = "dataformat-invalidator";
const ROLE: &str = "dataformat-invalidator-role";
const QUEUE: &str = "dataformat-invalidation-results";

/// The statuses CloudFront reports an invalidation under: "InProgress" until the batch is
/// finished, "Completed" once it is
/// (`smithy_query(query="doc", shape="Invalidation", member="Status")`).
const IN_PROGRESS: &str = "InProgress";
const COMPLETED: &str = "Completed";

/// How long the chain's one message is waited for, looked at every [`LOOK_INTERVAL`]. The loop
/// exits the instant the message is there, so this is a deadline and not a wait: generous, since
/// no row measures how long a delivery takes.
const DELIVERY: Duration = Duration::from_secs(120);

/// The handler the stack deploys: it invalidates the distribution the event names, reads the
/// invalidation back, and republishes the four values of the response onto the bus — the
/// creation instant as the ISO-8601 text a consumer parses, which is what the SDK's own
/// `datetime` for that member renders.
const HANDLER: &str = r#"
import json

import boto3


def lambda_handler(event, context):
    asked = event["detail"]
    cloudfront = boto3.client("cloudfront")
    submitted = cloudfront.create_invalidation(
        DistributionId=asked["distribution"],
        InvalidationBatch={
            "Paths": {"Items": asked["paths"]},
            "CallerReference": asked["reference"],
        },
    )["Invalidation"]
    detail = {
        "invalidation_id": submitted["Id"],
        "status": submitted["Status"],
        "create_time": submitted["CreateTime"].isoformat(),
        "caller_reference": submitted["InvalidationBatch"]["CallerReference"],
    }
    read = cloudfront.get_invalidation(
        DistributionId=asked["distribution"], Id=submitted["Id"]
    )["Invalidation"]
    detail["read_create_time"] = read["CreateTime"].isoformat()
    # Two readings of ONE instant, compared as instants — which is what a handler acting on a
    # reported date does, and what a value handed over as a bare number could not be.
    detail["same_instant"] = read["CreateTime"] == submitted["CreateTime"]
    published = boto3.client("events").put_events(
        Entries=[
            {
                "Source": asked["source"],
                "DetailType": asked["detail_type"],
                "Detail": json.dumps(detail),
            }
        ]
    )
    return {"failed": published["FailedEntryCount"], "detail": detail}
"#;

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "events", "AWSEvents", "1.1", operation, body).await
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    rpc(router, case, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// One AWS JSON RPC call that must answer 200, as its parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).into_owned();
    assert_eq!(
        status, 200,
        "case {case:?}: {target_prefix}.{operation} must answer 200, got {status} with {text}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
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

/// The stack that declares the execution role and the function: the role trusts Lambda and is
/// granted the invalidation calls on the distribution and the publication the handler makes,
/// which is what each of its calls is judged against.
fn handler_stack(distribution: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "InvalidatorRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "invalidate-and-republish",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [
                                {
                                    "Effect": "Allow",
                                    "Action": [
                                        "cloudfront:CreateInvalidation",
                                        "cloudfront:GetInvalidation",
                                    ],
                                    "Resource": format!(
                                        "arn:aws:cloudfront::{ACCOUNT}:distribution/{distribution}"
                                    ),
                                },
                                {
                                    "Effect": "Allow",
                                    "Action": ["events:PutEvents", "logs:*"],
                                    "Resource": "*",
                                },
                            ],
                        },
                    }],
                },
            },
            "Invalidator": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["InvalidatorRole", "Arn"]},
                    "Timeout": 60,
                    "Code": {"ZipFile": HANDLER},
                },
            },
        },
        "Outputs": {
            "FunctionName": {"Value": {"Ref": "Invalidator"}},
        },
    })
    .to_string()
}

/// The function's own ARN, which is what a rule targets.
async fn function_arn(router: &MultiServiceRouter, name: &str) -> String {
    let (status, read) = lambda(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: GetFunctionConfiguration({name:?}) must answer 200, got {status} with {read}"
    );
    read["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the function reports no FunctionArn: {read}"))
        .to_string()
}

/// The resource-based statement EventBridge's handover of a matched event to a FUNCTION needs
/// ("for Lambda functions … EventBridge relies on resource-based policies", `PutTargets`).
async fn grant_events_invoke(router: &MultiServiceRouter, name: &str, rule_arn: &str) {
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/policy"),
        &json!({
            "StatementId": "events-invoke",
            "Action": INVOKE_ACTION,
            "Principal": EVENTS_PRINCIPAL,
            "SourceArn": rule_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: AddPermission({name:?}) must answer 201, got {status} with {granted}"
    );
}

/// A queue whose own policy admits EventBridge's delivery from `rule_arn` and nothing else: its
/// url and the ARN a rule targets it by.
async fn given_queue(router: &MultiServiceRouter, rule_arn: &str) -> (String, String) {
    let created = sqs(router, "setup", "CreateQueue", &json!({ "QueueName": QUEUE })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateQueue reports no QueueUrl: {created}"))
        .to_string();
    let reported = sqs(
        router,
        "setup",
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = reported["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the queue reports no QueueArn: {reported}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "events-forward",
            "Effect": "Allow",
            "Principal": {"Service": EVENTS_PRINCIPAL},
            "Action": SEND_MESSAGE_ACTION,
            "Resource": arn,
            "Condition": {"ArnEquals": {"aws:SourceArn": rule_arn}},
        }],
    })
    .to_string();
    sqs(
        router,
        "setup",
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    (url, arn)
}

/// A rule on the default bus matching `source` and `detail_type`, answering the ARN it was
/// recorded under.
async fn given_rule(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    detail_type: &str,
) -> String {
    let put = events(
        router,
        "setup",
        "PutRule",
        &json!({
            "Name": name,
            "EventPattern": json!({
                "source": [source],
                "detail-type": [detail_type],
            })
            .to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    put["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: PutRule({name:?}) reports no RuleArn: {put}"))
        .to_string()
}

/// Point `rule` at the one target `arn` names.
async fn given_target(router: &MultiServiceRouter, rule: &str, id: &str, arn: &str) {
    let targeted = events(
        router,
        "setup",
        "PutTargets",
        &json!({ "Rule": rule, "Targets": [{ "Id": id, "Arn": arn }] }),
    )
    .await;
    assert_eq!(
        targeted["FailedEntryCount"].as_i64(),
        Some(0),
        "setup: the target {id:?} of {rule:?} must be accepted, got {targeted}"
    );
}

/// The one message the chain is to leave on the queue at `url`, waited for until [`DELIVERY`]
/// has passed, and DELETED as it is read so the next row sees only its own.
///
/// Answers every message the queue held, so a row that produced more than one fails on the
/// count rather than on whichever one came first.
async fn drained(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY;
    loop {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let messages = received["Messages"].as_array().cloned().unwrap_or_default();
        if !messages.is_empty() {
            let mut bodies = Vec::with_capacity(messages.len());
            for message in &messages {
                let handle = message["ReceiptHandle"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: the message reports no ReceiptHandle: {message}")
                });
                sqs(
                    router,
                    case,
                    "DeleteMessage",
                    &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                )
                .await;
                let body = message["Body"].as_str().unwrap_or_default();
                bodies.push(serde_json::from_str(body).unwrap_or(Value::Null));
            }
            return bodies;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the chain left no message on the downstream queue"
        );
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// One ISO-8601 instant in UTC as the text of its whole seconds and its microsecond fraction.
///
/// Two readings of ONE instant may state a different number of fractional digits — the text a
/// `restXml` answer carries states as many as the value holds, while a Python `datetime` holds
/// microseconds — so what is compared is the instant and not the resolution each reading of it
/// states. A reading in neither of UTC's two designators (`Z`, `+00:00`) is not an instant in
/// UTC at all and is reported as none.
fn instant_parts(text: &str) -> Option<(String, u32)> {
    let body = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (seconds, fraction) = match body.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (body, ""),
    };
    if !fraction.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let micros: String = fraction.chars().chain("000000".chars()).take(6).collect();
    Some((seconds.to_string(), micros.parse::<u32>().ok()?))
}

/// One row: the batch the handler submits, and the caller reference the client chose for it.
struct Row {
    case: &'static str,
    /// The paths the batch carries, which the invalidation has to hold afterwards.
    paths: &'static [&'static str],
    /// The reference that has to come back through the chain verbatim.
    reference: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "one exact object path under a plain reference",
        paths: &["/data-format/app.js"],
        reference: "dataformat-plain",
    },
    Row {
        case: "a wildcard over a prefix under a reference carrying dots, dashes and digits",
        paths: &["/data-format/*"],
        reference: "dataformat-7f3a.b2-1c4d_0123456789",
    },
    Row {
        case: "several paths at once under a long reference, which must not be truncated",
        paths: &["/data-format/app.js", "/index.html", "/data-format/nested/a.b-c.json"],
        reference: "dataformat-0123456789-0123456789-0123456789-0123456789-0123456789-\
                    0123456789-0123456789-0123456789-0123456789-0123456789",
    },
];

/// RULE: the `Invalidation` a `CreateInvalidation` answers reaches the handler whole — the
/// creation instant as the `datetime` an SDK materialises for a timestamp member, so the handler
/// can format and compare it — and every one of its four required values survives the republish
/// onto the bus and the delivery to the queue, agreeing with what `GetInvalidation` serves for
/// the same id.
#[tokio::test]
async fn a_create_invalidation_response_propagates_through_the_bus_to_its_queue() {
    let router = build_router();
    create_origin(&router).await;
    let distribution = create_distribution(&router, "dataformat-propagation-reference").await;

    // The second hop first: the rule that forwards the handler's result, and the queue whose own
    // policy admits exactly that rule's delivery.
    let forward_arn = given_rule(&router, FORWARD_RULE, RESULT_SOURCE, RESULT_DETAIL_TYPE).await;
    let (queue_url, queue_arn) = given_queue(&router, &forward_arn).await;
    given_target(&router, FORWARD_RULE, "results", &queue_arn).await;

    // The first hop: the function, and the rule that hands it a matched request.
    let outputs = deployed(
        &router,
        "xsvc-dataformat-propagation-stack",
        &handler_stack(&distribution),
    )
    .await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| FUNCTION.to_string());
    wait_until_active(&router, &function).await;
    let target_arn = function_arn(&router, &function).await;
    let trigger_arn = given_rule(&router, TRIGGER_RULE, TRIGGER_SOURCE, TRIGGER_DETAIL_TYPE).await;
    grant_events_invoke(&router, &function, &trigger_arn).await;
    given_target(&router, TRIGGER_RULE, "invalidator", &target_arn).await;

    for row in ROWS {
        let case = row.case;
        let published = events(
            &router,
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": TRIGGER_SOURCE,
                    "DetailType": TRIGGER_DETAIL_TYPE,
                    "Detail": json!({
                        "distribution": distribution,
                        "paths": row.paths,
                        "reference": row.reference,
                        "source": RESULT_SOURCE,
                        "detail_type": RESULT_DETAIL_TYPE,
                    })
                    .to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            published["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: the request event must be accepted, got {published}"
        );

        let messages = drained(&router, case, &queue_url).await;
        assert_eq!(
            messages.len(),
            1,
            "case {case:?}: expected exactly one forwarded message, got {messages:?}"
        );
        let envelope = &messages[0];
        assert_eq!(
            envelope["source"],
            json!(RESULT_SOURCE),
            "case {case:?}: expected the handler's own publication, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"],
            json!(RESULT_DETAIL_TYPE),
            "case {case:?}: expected the result's own detail type, got {envelope}"
        );
        let detail = &envelope["detail"];

        let id = detail["invalidation_id"].as_str().unwrap_or_default();
        assert!(
            !id.is_empty(),
            "case {case:?}: the propagated detail carries no invalidation id, got {detail}"
        );
        let status = detail["status"].as_str().unwrap_or_default();
        assert!(
            status == IN_PROGRESS || status == COMPLETED,
            "case {case:?}: expected {IN_PROGRESS:?} or {COMPLETED:?}, got {detail}"
        );
        assert_eq!(
            detail["caller_reference"],
            json!(row.reference),
            "case {case:?}: expected the caller reference the client chose, verbatim, got \
             {detail}"
        );
        // The instant was handed to the handler as a value it could FORMAT, and both of its
        // readings of it are the same instant.
        let propagated = detail["create_time"].as_str().unwrap_or_default();
        let parts = instant_parts(propagated).unwrap_or_else(|| {
            panic!("case {case:?}: expected an ISO-8601 instant in UTC, got {detail}")
        });
        assert_eq!(
            detail["read_create_time"],
            json!(propagated),
            "case {case:?}: expected the read to report the same instant text, got {detail}"
        );
        assert_eq!(
            detail["same_instant"],
            json!(true),
            "case {case:?}: expected the two readings to compare as one instant, got {detail}"
        );

        // BACKED: the invalidation is CloudFront's own record, read through CloudFront's own API
        // rather than out of what the handler said.
        let read = cloudfront(
            &router,
            "GET",
            &format!("/{API_VERSION}/distribution/{distribution}/invalidation/{id}"),
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetInvalidation({id}) must answer 200, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.element("Id"),
            id,
            "case {case:?}: expected the invalidation the detail names, got {}",
            read.body
        );
        assert_eq!(
            read.element("CallerReference"),
            row.reference,
            "case {case:?}: expected the batch's own caller reference, got {}",
            read.body
        );
        let served = instant_parts(&read.element("CreateTime")).unwrap_or_else(|| {
            panic!(
                "case {case:?}: CloudFront must state the creation instant in UTC, got {}",
                read.body
            )
        });
        assert_eq!(
            parts, served,
            "case {case:?}: expected the propagated instant {propagated:?} to be the one \
             CloudFront serves, got {}",
            read.body
        );
        for path in row.paths {
            assert!(
                read.body.contains(&format!("<Path>{path}</Path>")),
                "case {case:?}: CloudFront must hold the path {path:?} for this invalidation, \
                 got {}",
                read.body
            );
        }
    }
}
