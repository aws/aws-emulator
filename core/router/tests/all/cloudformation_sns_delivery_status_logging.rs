// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::SNS::Topic` with `DeliveryStatusLogging`
//! beside the `AWS::IAM::Role` it names: the deployed role is ASSUMED by Amazon SNS, which
//! leaves one delivery-status record per delivery in CloudWatch Logs.
//!
//! RULE: a topic that names a feedback role for the protocol a delivery went over has Amazon
//! SNS assume that role and write one record about the attempt into the log group
//! `sns/<region>/<account-id>/<topic-name>` — a success through
//! `<Protocol>SuccessFeedbackRoleArn` at the percentage
//! `<Protocol>SuccessFeedbackSampleRate` names, a failure through
//! `<Protocol>FailureFeedbackRoleArn` into that group's `/Failure` child
//! ("`<Protocol>`SuccessFeedbackRoleArn – Indicates successful message delivery status for an
//! Amazon SNS topic that is subscribed to an endpoint of that kind …
//! `<Protocol>`FailureFeedbackRoleArn – Indicates failed message delivery status …
//! `<Protocol>`SuccessFeedbackSampleRate – Indicates percentage of successful messages to
//! sample", `SetTopicAttributesInput$AttributeName`) — and a `DeliveryStatusLogging` entry a
//! template declares IS those attributes, under the prefix that entry's `Protocol` is spelled
//! with. A topic naming no role for the delivery's own protocol writes nothing.
//!
//! The rows differ along what the rule generalises over: which of the entry's members the
//! template declares (the success role, the failure role), the sample rate it declares beside
//! the success role (a full sample, none, and none named at all — written as a template writes
//! one, as a string and as a number), the `Protocol` the entry names against the protocol the
//! delivery went over (`sqs` beside `lambda` and `http/s`, whose prefixes differ from the
//! value), whether the subscribed queue's own policy grants this service the send (which is
//! what decides SUCCESS against FAILURE), and a topic that declares no logging at all.
//!
//! Five crates have to agree for this to hold — CloudFormation deploys the role and the topic,
//! IAM holds the role and the documents that decide whether SNS may act as it, SNS attempts
//! the delivery and writes the record, Amazon SQS is the endpoint that takes or withholds the
//! message, and CloudWatch Logs is where the record is read back from — so the suite lives
//! here: no single service crate's test target can deploy the stack and then read the log.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{decoded_entries, element, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for, and so the Region the stack's topic, its ARN
/// and its delivery-status log group all name. Deliberately NOT the emulator's default
/// Region: the group's name carries the Region the topic is in, so a default would hide a
/// group built from the wrong one.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries, and so the account the log group's
/// name carries.
const ACCOUNT_ID: &str = "000000000000";

/// The service principal a delivery-status feedback role has to trust for SNS to assume it,
/// and the one a subscribed queue's policy grants the send to.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The CloudWatch Logs actions a delivery-status record costs the assumed role: the group and
/// the stream are created on first write and then the record is put. The names are CloudWatch
/// Logs' own operations (`smithy_query(query="list_operations")` of the `logs` model:
/// `CreateLogGroup`, `CreateLogStream`, `PutLogEvents`).
const LOG_WRITE_ACTIONS: [&str; 3] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:PutLogEvents",
];

/// The body every row publishes.
const NOTIFICATION: &str = "a notification whose delivery is logged";

/// How often a wait looks again, and how many looks it takes: the spans waited on are the
/// owning services' own, so nothing here sleeps for a fixed span expecting work to be done and
/// the deadline is generous enough that a loaded host cannot decide an outcome.
const LOOK: Duration = Duration::from_millis(50);
const LOOKS: usize = 1_200;

/// How long a row waits for a record that is owed. The record is written before the publish is
/// answered, so this is many writes' worth of slack rather than a span the behaviour depends
/// on.
const RECORD_BUDGET: Duration = Duration::from_secs(30);

/// Stands in for the ARN of the role the row's own stack deployed, which is known only once the
/// stack has published it.
const ROLE_PLACEHOLDER: &str = "<role>";

/// The emulator as it is ASSEMBLED, on one engine: SNS's own calls to IAM, Amazon SQS and
/// CloudWatch Logs are dispatched back into this same router, so the record lands in the very
/// store this suite reads it from.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. Neither the key id nor the signature is a credential: this harness turns
/// request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
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

/// One CloudFormation call (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Amazon SNS call (API version 2010-03-31) that must be answered 200.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = query(router, "sns", "2010-03-31", action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SNS {action} must be answered 200, got {status} with {body}"
    );
    body
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`), as its status and parsed body.
async fn sqs_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
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

/// One Amazon SQS call that must be answered 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = sqs_call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SQS {operation} must be answered 200, got {status} with {parsed}"
    );
    parsed
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), with whatever status
/// it answered: a row that expects NO record has to be able to see the group simply not being
/// there.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization("logs"))
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

/// Whether a stack status is one a deployment STOPS in, so polling it again tells a client
/// nothing new.
fn is_terminal(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack settles in a terminal status, and answer that status
/// with the body it was last described with.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack:?}) must be answered, got {status} with {body}"
        );
        described = body;
        if is_terminal(&opt_element(&described, "StackStatus").unwrap_or_default()) {
            return (
                opt_element(&described, "StackStatus").unwrap_or_default(),
                described,
            );
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!(
        "case {case:?}: the deployment must settle in a terminal status rather than leaving a \
         client polling, last described {described}"
    );
}

/// The template one row deploys: the feedback role, with the trust policy that lets SNS act as
/// it and the CloudWatch Logs permissions a record costs, and the topic whose
/// `DeliveryStatusLogging` names it.
fn template(row: &Row) -> String {
    let mut topic_properties = json!({"TopicName": row.topic_name});
    if let Some(declared) = row.logging {
        let logging: Value = serde_json::from_str(declared)
            .unwrap_or_else(|error| panic!("case {:?}: {declared} is JSON ({error})", row.case));
        topic_properties["DeliveryStatusLogging"] = logging;
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "FeedbackRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": SNS_PRINCIPAL},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "write-the-delivery-status-records",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": LOG_WRITE_ACTIONS,
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Topic": {"Type": "AWS::SNS::Topic", "Properties": topic_properties},
        },
        "Outputs": {
            "TopicArn": {"Value": {"Ref": "Topic"}},
            "RoleArn": {"Value": {"Fn::GetAtt": ["FeedbackRole", "Arn"]}},
        },
    })
    .to_string()
}

/// The queue policy AWS documents for a queue subscribed to an SNS topic, which is what makes a
/// delivery to it SUCCEED; a queue carrying none has granted this service nothing, so its
/// delivery FAILS.
fn queue_grants_sns(queue_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowSNSDelivery",
            "Effect": "Allow",
            "Principal": {"Service": SNS_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
        }],
    })
    .to_string()
}

/// A queue called `name`, granting SNS the send when `grants` asks for it: its url and the ARN
/// a subscription names it by.
async fn given_queue(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    grants: bool,
) -> (String, String) {
    let created = sqs(router, case, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered no QueueUrl: {created}"))
        .to_string();
    let described = sqs(
        router,
        case,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = described["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {described}"))
        .to_string();
    if grants {
        sqs(
            router,
            case,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"Policy": queue_grants_sns(&arn)}}),
        )
        .await;
    }
    (url, arn)
}

/// Whether `name` is one of the delivery-status feedback attributes — the closed set SNS
/// documents for the feature, whose names all end in one of these two suffixes
/// (`SetTopicAttributesInput$AttributeName`).
fn is_feedback_attribute(name: &str) -> bool {
    name.ends_with("FeedbackRoleArn") || name.ends_with("FeedbackSampleRate")
}

/// Every delivery-status record `log_group` holds, parsed, across every stream in it — and an
/// empty list for a group that is not there at all.
async fn records_in(router: &MultiServiceRouter, log_group: &str) -> Vec<Value> {
    let (status, streams) = logs(
        router,
        "DescribeLogStreams",
        &json!({"logGroupName": log_group}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    let mut records: Vec<Value> = Vec::new();
    for stream in streams["logStreams"].as_array().cloned().unwrap_or_default() {
        let Some(name) = stream["logStreamName"].as_str() else {
            continue;
        };
        let (status, events) = logs(
            router,
            "GetLogEvents",
            &json!({
                "logGroupName": log_group,
                "logStreamName": name,
                "startFromHead": true,
            }),
        )
        .await;
        if status != 200 {
            continue;
        }
        records.extend(
            events["events"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|event| event["message"].as_str())
                .filter_map(|message| serde_json::from_str::<Value>(message).ok()),
        );
    }
    records
}

/// Wait until `log_group` holds a record and answer the ones it held — or an empty list once
/// `budget` is spent.
async fn records_within(
    router: &MultiServiceRouter,
    log_group: &str,
    budget: Duration,
) -> Vec<Value> {
    let deadline = Instant::now() + budget;
    loop {
        let records = records_in(router, log_group).await;
        if !records.is_empty() || Instant::now() >= deadline {
            return records;
        }
        tokio::time::sleep(LOOK).await;
    }
}

/// Whether the notification reached the queue, which is what says the DELIVERY itself happened
/// whatever was logged about it.
async fn notification_reached(router: &MultiServiceRouter, case: &str, queue_url: &str) -> bool {
    for _ in 0..LOOKS {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 1}),
        )
        .await;
        let bodies = received["Messages"].as_array().cloned().unwrap_or_default();
        if bodies
            .iter()
            .filter_map(|message| message["Body"].as_str())
            .any(|body| body.contains(NOTIFICATION))
        {
            return true;
        }
        tokio::time::sleep(LOOK).await;
    }
    false
}

/// The document a delivery-status record carries under `providerResponse`, parsed.
fn provider_response(case: &str, record: &Value) -> Value {
    let text = record["delivery"]["providerResponse"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the record must carry a providerResponse, got {record}")
        });
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: providerResponse is not JSON ({error}): {text}")
    })
}

/// One row: what the stack declares, what the subscribed queue grants, and what must come of
/// the publish.
struct Row {
    case: &'static str,
    stack: &'static str,
    topic_name: &'static str,
    queue_name: &'static str,
    /// The `DeliveryStatusLogging` list the topic declares, as the JSON a template carries, and
    /// `None` for a topic that declares none.
    logging: Option<&'static str>,
    /// The feedback attributes the deployed topic must carry, with [`ROLE_PLACEHOLDER`] standing
    /// for the ARN of the role the same stack deployed.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// Whether the subscribed queue's policy grants this service the send.
    queue_grants: bool,
    /// The `status` the one record must report, and `None` for a configuration that must write
    /// no record at all.
    status: Option<&'static str>,
}

/// The `Protocol` values an `AWS::SNS::Topic`'s `DeliveryStatusLogging` entry takes are
/// `http/s`, `sqs`, `lambda`, `firehose` and `application`, each standing for the prefix SNS
/// spells its own attributes with (`HTTP`, `SQS`, `Lambda`, `Firehose`, `Application`) —
/// source: that type's `LoggingConfig` property set against the attribute names
/// `SetTopicAttributesInput$AttributeName` documents. The rows below declare `sqs` against an
/// `sqs` delivery, and `lambda` and `http/s` against it, which must log nothing.
const ROWS: &[Row] = &[
    Row {
        case: "a full sample of successes over the delivery's own protocol",
        stack: "xsvc-sns-feedback-sampled",
        topic_name: "xsvc-sns-feedback-sampled-topic",
        queue_name: "xsvc-sns-feedback-sampled-queue",
        logging: Some(
            r#"[{"Protocol":"sqs",
                 "SuccessFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]},
                 "SuccessFeedbackSampleRate":"100"}]"#,
        ),
        expected_attributes: &[
            ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
            ("SQSSuccessFeedbackSampleRate", "100"),
        ],
        queue_grants: true,
        status: Some("SUCCESS"),
    },
    Row {
        case: "a failure role, and a delivery the queue's own policy withheld",
        stack: "xsvc-sns-feedback-withheld",
        topic_name: "xsvc-sns-feedback-withheld-topic",
        queue_name: "xsvc-sns-feedback-withheld-queue",
        logging: Some(
            r#"[{"Protocol":"sqs",
                 "FailureFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]}}]"#,
        ),
        expected_attributes: &[("SQSFailureFeedbackRoleArn", ROLE_PLACEHOLDER)],
        queue_grants: false,
        status: Some("FAILURE"),
    },
    Row {
        // The rate written as a NUMBER, which is the other form a template declares one in;
        // what the topic carries is the text of that number either way.
        case: "a success role but no sample of successes",
        stack: "xsvc-sns-feedback-unsampled",
        topic_name: "xsvc-sns-feedback-unsampled-topic",
        queue_name: "xsvc-sns-feedback-unsampled-queue",
        logging: Some(
            r#"[{"Protocol":"sqs",
                 "SuccessFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]},
                 "SuccessFeedbackSampleRate":0}]"#,
        ),
        expected_attributes: &[
            ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
            ("SQSSuccessFeedbackSampleRate", "0"),
        ],
        queue_grants: true,
        status: None,
    },
    Row {
        case: "a success role named for a success role, with no sample rate declared at all",
        stack: "xsvc-sns-feedback-rateless",
        topic_name: "xsvc-sns-feedback-rateless-topic",
        queue_name: "xsvc-sns-feedback-rateless-queue",
        logging: Some(
            r#"[{"Protocol":"sqs",
                 "SuccessFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]}}]"#,
        ),
        expected_attributes: &[("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER)],
        queue_grants: true,
        status: None,
    },
    Row {
        case: "a lambda-protocol entry against a delivery that went over sqs",
        stack: "xsvc-sns-feedback-lambda-entry",
        topic_name: "xsvc-sns-feedback-lambda-entry-topic",
        queue_name: "xsvc-sns-feedback-lambda-entry-queue",
        logging: Some(
            r#"[{"Protocol":"lambda",
                 "SuccessFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]},
                 "SuccessFeedbackSampleRate":"100"}]"#,
        ),
        expected_attributes: &[
            ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
            ("LambdaSuccessFeedbackSampleRate", "100"),
        ],
        queue_grants: true,
        status: None,
    },
    Row {
        // `http/s` is the one entry whose `Protocol` is not the attribute prefix at all: the
        // topic carries `HTTP…` attributes for it.
        case: "an http/s-protocol entry against a delivery that went over sqs",
        stack: "xsvc-sns-feedback-https-entry",
        topic_name: "xsvc-sns-feedback-https-entry-topic",
        queue_name: "xsvc-sns-feedback-https-entry-queue",
        logging: Some(
            r#"[{"Protocol":"http/s",
                 "SuccessFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]},
                 "SuccessFeedbackSampleRate":"100",
                 "FailureFeedbackRoleArn":{"Fn::GetAtt":["FeedbackRole","Arn"]}}]"#,
        ),
        expected_attributes: &[
            ("HTTPFailureFeedbackRoleArn", ROLE_PLACEHOLDER),
            ("HTTPSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
            ("HTTPSuccessFeedbackSampleRate", "100"),
        ],
        queue_grants: true,
        status: None,
    },
    Row {
        case: "no delivery-status logging declared at all",
        stack: "xsvc-sns-feedback-none",
        topic_name: "xsvc-sns-feedback-none-topic",
        queue_name: "xsvc-sns-feedback-none-queue",
        logging: None,
        expected_attributes: &[],
        queue_grants: true,
        status: None,
    },
];

/// Every row: the stack converges, the `DeliveryStatusLogging` it declared IS the topic's
/// per-protocol feedback attributes, and the record the delivery over `sqs` leaves in
/// CloudWatch Logs is the one the rule owes — in the group named for the topic's Region,
/// account and name, or in nothing at all where the topic asks for no record.
#[tokio::test]
async fn a_stack_declared_feedback_role_is_assumed_for_each_delivery() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must be accepted, got {status} with {body}"
        );
        let (settled, described) = settled_stack(&router, case, row.stack).await;
        assert_eq!(
            settled, "CREATE_COMPLETE",
            "case {case:?}: the stack must deploy the role and the topic, got {described}"
        );

        let outputs = outputs_of(&described);
        let topic_arn = outputs
            .get("TopicArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack must publish the topic's ARN"));
        let role_arn = outputs
            .get("RoleArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack must publish the role's ARN"));
        // The handle the stack published is the one SNS keys the topic by, and the one the log
        // group's name is read off: computed from what the row declared, never read back out of
        // the answer being checked.
        let expected_arn = format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{}", row.topic_name);
        assert_eq!(
            topic_arn, expected_arn,
            "case {case:?}: expected the topic ARN {expected_arn}, got {topic_arn}"
        );

        // The declaration half of the rule: the entry IS the per-protocol attributes.
        let attributes = decoded_entries(
            &sns(
                &router,
                case,
                "GetTopicAttributes",
                &[("TopicArn", &topic_arn)],
            )
            .await,
        );
        let carried: BTreeMap<String, String> = attributes
            .into_iter()
            .filter(|(name, _)| is_feedback_attribute(name))
            .collect();
        let expected: BTreeMap<String, String> = row
            .expected_attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.replace(ROLE_PLACEHOLDER, &role_arn)))
            .collect();
        assert_eq!(
            carried, expected,
            "case {case:?}: expected the feedback attributes {expected:?}, got {carried:?}"
        );

        let (queue_url, queue_arn) =
            given_queue(&router, case, row.queue_name, row.queue_grants).await;
        sns(
            &router,
            case,
            "Subscribe",
            &[
                ("TopicArn", &topic_arn),
                ("Protocol", "sqs"),
                ("Endpoint", &queue_arn),
            ],
        )
        .await;

        let published = sns(
            &router,
            case,
            "Publish",
            &[("TopicArn", &topic_arn), ("Message", NOTIFICATION)],
        )
        .await;
        let message_id = element(&published, "MessageId");
        assert!(
            !message_id.is_empty(),
            "case {case:?}: Publish must report the message it saved, got {published}"
        );

        // The group's name is the template the rule states, built from the Region the topic is
        // in, the account that owns it and its own name.
        let success_group = format!("sns/{REGION}/{ACCOUNT_ID}/{}", row.topic_name);
        let failure_group = format!("{success_group}/Failure");

        if row.queue_grants {
            // The delivery itself happened, whatever the topic asked to have logged about it.
            assert!(
                notification_reached(&router, case, &queue_url).await,
                "case {case:?}: the notification must reach the subscribed queue"
            );
        }

        let Some(expected_status) = row.status else {
            for group in [&success_group, &failure_group] {
                let records = records_in(&router, group).await;
                assert!(
                    records.is_empty(),
                    "case {case:?}: this topic asks for no delivery-status record, so {group} \
                     must stay empty, got {records:?}"
                );
            }
            continue;
        };
        let (group, quiet) = if expected_status == "SUCCESS" {
            (&success_group, &failure_group)
        } else {
            (&failure_group, &success_group)
        };
        let records = records_within(&router, group, RECORD_BUDGET).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one delivery-status record in {group}, got {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected status {expected_status:?}, got {record}"
        );
        assert_eq!(
            record["notification"]["topicArn"].as_str(),
            Some(topic_arn.as_str()),
            "case {case:?}: the record must name the topic published to, got {record}"
        );
        assert_eq!(
            record["notification"]["messageId"].as_str(),
            Some(message_id.as_str()),
            "case {case:?}: the record must name the published message, got {record}"
        );
        assert_eq!(
            record["delivery"]["destination"].as_str(),
            Some(queue_arn.as_str()),
            "case {case:?}: the record must name the endpoint delivered to, got {record}"
        );
        assert!(
            record["delivery"]["deliveryId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record must carry a delivery id, got {record}"
        );
        if expected_status == "SUCCESS" {
            // Amazon SQS answers a `SendMessage` it took with 200, and reports the id it minted.
            assert_eq!(
                record["delivery"]["statusCode"].as_u64(),
                Some(200),
                "case {case:?}: expected the queue's own success status, got {record}"
            );
            let response = provider_response(case, record);
            assert!(
                response["sqsMessageId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: the provider response must name the queue's message, got \
                 {response}"
            );
        } else {
            assert_eq!(
                record["delivery"]["statusCode"].as_u64(),
                Some(403),
                "case {case:?}: a delivery no policy allows is a 403, got {record}"
            );
            let response = provider_response(case, record);
            assert_eq!(
                response["ErrorCode"].as_str(),
                Some("AccessDenied"),
                "case {case:?}: expected the AccessDenied code, got {response}"
            );
        }
        let others = records_in(&router, quiet).await;
        assert!(
            others.is_empty(),
            "case {case:?}: only the group of this outcome is written, so {quiet} must stay \
             empty, got {others:?}"
        );
    }
}
