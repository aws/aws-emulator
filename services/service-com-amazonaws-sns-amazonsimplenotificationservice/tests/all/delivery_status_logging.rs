// SPDX-License-Identifier: CC0-1.0

//! DELIVERY-STATUS LOGGING: what a topic carrying a `<Protocol>` feedback role writes about
//! each delivery it attempted, end to end over the wire through the real `MultiServiceRouter`,
//! with Amazon SQS, AWS Lambda, IAM and CloudWatch Logs mounted on the same engine as SNS — so
//! the record is read back where a client reads it, through `logs:DescribeLogStreams` and
//! `logs:GetLogEvents`.
//!
//! The RULE asserted here: "`<Protocol>`SuccessFeedbackRoleArn – Indicates successful message
//! delivery status for an Amazon SNS topic that is subscribed to an endpoint of that kind …
//! `<Protocol>`FailureFeedbackRoleArn – Indicates failed message delivery status …
//! `<Protocol>`SuccessFeedbackSampleRate – Indicates percentage of successful messages to
//! sample … The `<ENDPOINT>`SuccessFeedbackRoleArn and `<ENDPOINT>`FailureFeedbackRoleArn
//! attributes are used to give Amazon SNS write access to use CloudWatch Logs on your behalf"
//! (`SetTopicAttributesInput$AttributeName`) — a topic that names a feedback role for the
//! protocol a delivery went over has SNS ASSUME that role and write one record per delivery,
//! naming the endpoint delivered to, the `SUCCESS`/`FAILURE` of the attempt, the HTTP status
//! the endpoint's own service answered with and that service's own answer. A success goes into
//! `sns/<region>/<account-id>/<topic-name>` at the sampled percentage; a failure goes into that
//! group's `/Failure` child, always. A topic that names no role for the delivery's protocol
//! writes nothing, and neither does one whose role does not TRUST this service (there is then
//! nothing to assume) or whose role is not ALLOWED the CloudWatch Logs write (there is then no
//! write to make, and no group is created) — while the delivery itself happens either way.
//!
//! The rows differ along what the rule generalises over: which of the feedback attributes the
//! topic carries, the sample rate (a full sample, none, and none named at all), the protocol
//! the attributes were set for as against the one the delivery went over, whether the delivery
//! succeeded or was withheld by the endpoint's own policy, whether the named role's trust
//! policy still admits this service when the delivery is made, and which permissions that role
//! carries.
//!
//! Both delivery protocols SNS performs itself are exercised: the `sqs` one, whose endpoint is
//! a queue reached by `SendMessage`, and the `lambda` one, whose endpoint is a function reached
//! by an asynchronous `Invoke` — the two answer different success statuses and different
//! refusal codes, which is exactly what the record reports.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::lambda_subscription_delivery::{ROLE as LAMBDA_EXECUTION_ROLE, ZIP_FILE};
use crate::sqs_subscription_delivery::element;
use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for, and so the Region every ARN here names.
const REGION: &str = "us-east-1";

/// The service principal a subscribed endpoint's own policy has to grant for a notification to
/// reach it, and the one a feedback role has to trust for SNS to assume it.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// How long a row waits for a record that is owed. A record is written as soon as the delivery
/// it reports has been attempted, so this is many writes' worth of slack rather than a span the
/// behaviour depends on.
const RECORD_BUDGET: Duration = Duration::from_secs(10);

/// How long a row waits before concluding that NOTHING was written.
const QUIET_BUDGET: Duration = Duration::from_millis(1_500);

/// How often a wait re-reads the log group it is waiting on.
const LOOK_INTERVAL: Duration = Duration::from_millis(25);

/// SNS with the four services a logged delivery spans, on ONE engine: the queue and the
/// function the notification is delivered to, the IAM store the feedback role is resolved in,
/// and the CloudWatch Logs store the record lands in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router, engine }
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. Neither the key id nor the signature is a credential: this harness turns
/// request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One form-encoded (awsQuery) call that must SUCCEED, as its body text — the shape both SNS
/// and IAM take, differing only in the signing name and the API version.
async fn query(
    emulator: &Emulator,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        // The form encoding is the suite-shared one, not a second copy.
        body.push_str(&percent_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "{signing_name} {action}: expected HTTP 200, got {status} / {text}"
    );
    text
}

/// One SNS call that must succeed.
async fn sns(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    query(emulator, "sns", "2010-03-31", action, params).await
}

/// One IAM call that must succeed.
async fn iam(emulator: &Emulator, action: &str, params: &[(&str, &str)]) -> String {
    query(emulator, "iam", "2010-05-08", action, params).await
}

/// One `awsJson1_0` call against SQS that must succeed.
async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "SQS {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One `restJson1` call against Lambda, with whatever status it answered.
async fn lambda(emulator: &Emulator, method: &str, uri: &str, body: Value) -> (u16, Value) {
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
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `awsJson1_1` call against CloudWatch Logs, with whatever status it answered: a row that
/// expects NO record must be able to see the group simply not being there.
async fn logs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization("logs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The trust policy a delivery-status feedback role carries: `sts:AssumeRole` for `trusted`,
/// which is what SNS has to be granted to assume the role and write the record.
fn trusts(trusted: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": trusted},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// What the permissions in force on a feedback role allow — the half of the configuration that
/// decides whether the record can be WRITTEN once the role has been assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Permissions {
    /// The three CloudWatch Logs actions a record costs, which is what AWS documents a
    /// delivery-status feedback role has to grant.
    WritesLogs,
    /// A CloudWatch Logs READ and nothing else: a role that can be assumed but cannot write.
    ReadsLogs,
    /// No policy at all.
    None,
}

impl Permissions {
    /// The inline policy document this grants, or `None` for a role that carries no policy.
    ///
    /// The action names are CloudWatch Logs' own
    /// (`smithy_query(query="list_operations")` of the `logs` model: `CreateLogGroup`,
    /// `CreateLogStream`, `PutLogEvents`, `DescribeLogGroups`).
    fn document(self) -> Option<String> {
        let actions: &[&str] = match self {
            Self::WritesLogs => &[
                "logs:CreateLogGroup",
                "logs:CreateLogStream",
                "logs:PutLogEvents",
            ],
            Self::ReadsLogs => &["logs:DescribeLogGroups"],
            Self::None => return None,
        };
        Some(
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": actions,
                    "Resource": "*",
                }],
            })
            .to_string(),
        )
    }
}

/// Create the role `name` with a trust policy admitting `trusted` and the permissions
/// `permissions` describes, and answer its ARN.
async fn given_role(
    emulator: &Emulator,
    name: &str,
    trusted: &str,
    permissions: Permissions,
) -> String {
    let answered = iam(
        emulator,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trusts(trusted)),
        ],
    )
    .await;
    if let Some(document) = permissions.document() {
        iam(
            emulator,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "delivery-status"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    }
    element(&answered, "Arn")
}

/// Narrow the trust policy of the role `name` to admit `trusted` alone, after a topic has
/// already been given the role: a feedback role is only ACCEPTED while its trust policy admits
/// this service, so this is how a topic comes to name a role it can no longer assume.
async fn retrust(emulator: &Emulator, name: &str, trusted: &str) {
    iam(
        emulator,
        "UpdateAssumeRolePolicy",
        &[("RoleName", name), ("PolicyDocument", &trusts(trusted))],
    )
    .await;
}

/// The queue policy AWS documents for an SQS queue subscribed to an SNS topic, which is what
/// makes a delivery to it SUCCEED; a queue carrying none has granted this service nothing, so
/// its delivery FAILS.
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
async fn given_queue(emulator: &Emulator, name: &str, grants: bool) -> (String, String) {
    let created = sqs(emulator, "CreateQueue", &json!({"QueueName": name})).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string();
    if grants {
        sqs(
            emulator,
            "SetQueueAttributes",
            &json!({
                "QueueUrl": url,
                "Attributes": {"Policy": queue_grants_sns(&arn)},
            }),
        )
        .await;
    }
    (url, arn)
}

/// A function called `name`, brought to `Active` — the state an invocation needs — by moving
/// the activation instant the store holds into the past, granting SNS the invoke when `grants`
/// asks for it. Its ARN, which is what a subscription names it by.
async fn given_function(emulator: &Emulator, name: &str, grants: bool) -> String {
    let (status, created) = lambda(
        emulator,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": LAMBDA_EXECUTION_ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction {name}: expected HTTP 201, got {status} with {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateFunction {name}: no FunctionArn in {created}"))
        .to_string();
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }
    if grants {
        let (status, added) = lambda(
            emulator,
            "POST",
            &format!("/2015-03-31/functions/{name}/policy"),
            json!({
                "StatementId": "AllowSNSInvoke",
                "Action": "lambda:InvokeFunction",
                "Principal": SNS_PRINCIPAL,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "AddPermission {name}: expected HTTP 201, got {status} with {added}"
        );
    }
    arn
}

/// The log group a topic's SUCCESSFUL delivery-status records go into, read off the topic's own
/// ARN: `sns/<region>/<account-id>/<topic-name>`.
fn feedback_log_group(topic_arn: &str) -> String {
    let fields: Vec<&str> = topic_arn.split(':').collect();
    let [_, _, _, region, account, name] = fields[..] else {
        panic!("a topic ARN has six colon-separated fields, got {topic_arn}");
    };
    format!("sns/{region}/{account}/{name}")
}

/// The group a FAILED delivery's record goes into: the `/Failure` child of the topic's own.
fn failure_log_group(topic_arn: &str) -> String {
    format!("{}/Failure", feedback_log_group(topic_arn))
}

/// The group a record of `status` goes into, which is the half of the rule that depends on the
/// outcome.
fn group_of(topic_arn: &str, status: &str) -> String {
    if status == "SUCCESS" {
        feedback_log_group(topic_arn)
    } else {
        failure_log_group(topic_arn)
    }
}

/// Every delivery-status record `log_group` holds, parsed, across every stream in it — and an
/// empty list for a group that is not there at all.
async fn records_in(emulator: &Emulator, log_group: &str) -> Vec<Value> {
    let (status, streams) = logs(
        emulator,
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
            emulator,
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

/// Wait until `log_group` holds a record, and answer the ones it held — or an empty list once
/// `budget` is spent.
async fn records_within(emulator: &Emulator, log_group: &str, budget: Duration) -> Vec<Value> {
    let deadline = Instant::now() + budget;
    loop {
        let records = records_in(emulator, log_group).await;
        if !records.is_empty() || Instant::now() >= deadline {
            return records;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// The document a delivery-status record carries under `providerResponse`, parsed.
fn provider_response(case: &str, record: &Value) -> Value {
    let text = record["delivery"]["providerResponse"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the record must carry a providerResponse, got {record}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: providerResponse is not JSON ({error}): {text}"))
}

/// Stands in for the feedback role's ARN in a row's attribute values, which is only known once
/// the role has been created.
const ROLE_PLACEHOLDER: &str = "<role>";

/// What one row configures, and what must come of it.
struct Row {
    case: &'static str,
    /// The feedback attributes the topic is given, as `(name, value)` pairs — the role ARN is
    /// substituted for [`ROLE_PLACEHOLDER`].
    attributes: &'static [(&'static str, &'static str)],
    /// The service principal the named role's trust policy admits once the topic has been
    /// configured: a role is only ACCEPTED as a feedback role while it admits this service, so
    /// a row naming another principal narrows the trust policy after the write.
    trusted: &'static str,
    /// What the permissions in force on that role allow.
    permissions: Permissions,
    /// Whether the subscribed queue grants this service the send, which is what decides whether
    /// the delivery succeeds.
    queue_grants: bool,
    /// The `status` the record must report, and `None` for a configuration that must write no
    /// record at all.
    status: Option<&'static str>,
}

/// The attribute names are this service's own closed set for the delivery-status feature
/// (`SetTopicAttributesInput$AttributeName`), whose per-protocol prefixes are `Application`,
/// `Firehose`, `HTTP`, `Lambda` and `SQS`; the rows below exercise the `SQS` pair against an
/// `sqs` delivery and the `Lambda` pair against it, which must log nothing.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a full sample of successes over the delivery's own protocol",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: Some("SUCCESS"),
        },
        Row {
            case: "a failure role, and a delivery the endpoint's own policy withheld",
            attributes: &[("SQSFailureFeedbackRoleArn", ROLE_PLACEHOLDER)],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: false,
            status: Some("FAILURE"),
        },
        Row {
            case: "a success role but no sample of successes",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "0"),
            ],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a success role with no sample rate named at all",
            attributes: &[("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER)],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a full sample, but set for ANOTHER protocol",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a role whose trust policy no longer admits this service",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: "events.amazonaws.com",
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a trusted role carrying no permissions at all",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::None,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a trusted role that may read CloudWatch Logs but not write it",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::ReadsLogs,
            queue_grants: true,
            status: None,
        },
        Row {
            case: "a failure role that may not write CloudWatch Logs, and a withheld delivery",
            attributes: &[("SQSFailureFeedbackRoleArn", ROLE_PLACEHOLDER)],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::ReadsLogs,
            queue_grants: false,
            status: None,
        },
        Row {
            case: "no feedback attribute at all",
            attributes: &[],
            trusted: SNS_PRINCIPAL,
            permissions: Permissions::WritesLogs,
            queue_grants: true,
            status: None,
        },
    ]
}

/// The refusal AWS states for a delivery no resource-based policy allows: the principal the
/// delivery is made as, the action it performs and the resource it performs it on.
///
/// Amazon SQS compares action names lower-cased and words its refusal with them as compared,
/// which is why the queue form carries `sqs:sendmessage`.
fn not_authorized(action: &str, resource: &str) -> String {
    format!(
        "User: {SNS_PRINCIPAL} is not authorized to perform: {action} on resource: {resource} \
         because no resource-based policy allows the {action} action"
    )
}

#[tokio::test]
async fn a_topic_with_a_feedback_role_logs_one_record_per_delivery() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "feedback")]).await,
            "TopicArn",
        );
        let role_name = format!("sns-feedback-{index}");
        let role = given_role(&emulator, &role_name, SNS_PRINCIPAL, row.permissions).await;
        let (_, endpoint) =
            given_queue(&emulator, &format!("feedback-{index}"), row.queue_grants).await;
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &endpoint),
            ],
        )
        .await;
        for (name, value) in row.attributes {
            let value = value.replace(ROLE_PLACEHOLDER, &role);
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", name),
                    ("AttributeValue", &value),
                ],
            )
            .await;
        }
        // A topic may only be GIVEN a feedback role whose trust policy admits this service, so
        // a row whose role trusts something ELSE narrows the trust policy once the attributes
        // are written: the role the topic names is then one this service cannot assume, and
        // the publish below has nothing to write the record as.
        if row.trusted != SNS_PRINCIPAL {
            retrust(&emulator, &role_name, row.trusted).await;
        }

        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a notification")],
        )
        .await;

        let Some(expected) = row.status else {
            // Neither group may come into existence: the role that would have created it
            // either cannot be assumed or may not write.
            for log_group in [feedback_log_group(&topic), failure_log_group(&topic)] {
                let records = records_within(&emulator, &log_group, QUIET_BUDGET).await;
                assert!(
                    records.is_empty(),
                    "case {case:?}: this topic asks for no delivery-status record, so \
                     {log_group} must stay empty, got {records:?}"
                );
            }
            continue;
        };
        let log_group = group_of(&topic, expected);
        let records = records_within(&emulator, &log_group, RECORD_BUDGET).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one delivery-status record in {log_group}, got {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {record}"
        );
        assert_eq!(
            record["delivery"]["destination"].as_str(),
            Some(endpoint.as_str()),
            "case {case:?}: the record must name the endpoint delivered to, got {record}"
        );
        assert_eq!(
            record["notification"]["topicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the record must name the topic published to, got {record}"
        );
        assert!(
            record["notification"]["messageId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record must name the published message, got {record}"
        );
        assert!(
            record["delivery"]["deliveryId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record must carry a delivery id, got {record}"
        );
        if expected == "SUCCESS" {
            // Amazon SQS answers a `SendMessage` it took with 200, and reports the id it
            // minted for the message.
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
            assert_eq!(
                response["ErrorMessage"].as_str(),
                Some(not_authorized("sqs:sendmessage", &endpoint).as_str()),
                "case {case:?}: expected the refusal AWS words, got {response}"
            );
        }
    }
}

/// One row of the lambda table: the case, whether the function grants SNS the invoke, the
/// status the record must report, the HTTP status the record must carry and the error code its
/// provider response must name (`None` for a delivery that was taken).
type FunctionCase<'a> = (&'a str, bool, &'a str, u64, Option<&'a str>);

/// RULE: the record reports the status the ENDPOINT'S OWN SERVICE answered — so the same rule
/// over the `lambda` protocol reports Lambda's asynchronous-invoke `202` for an invocation it
/// accepted and a `403` naming Lambda's own refusal code for a function SNS may not invoke.
#[tokio::test]
async fn a_lambda_delivery_reports_the_status_lambda_answered() {
    let rows: [FunctionCase<'_>; 2] = [
        ("an invocation the function's policy grants", true, "SUCCESS", 202, None),
        (
            "a function SNS may not invoke",
            false,
            "FAILURE",
            403,
            Some("AccessDeniedException"),
        ),
    ];
    for (index, (case, grants, expected, code, error_code)) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", "fn-feedback")]).await,
            "TopicArn",
        );
        let role = given_role(
            &emulator,
            &format!("sns-fn-feedback-{index}"),
            SNS_PRINCIPAL,
            Permissions::WritesLogs,
        )
        .await;
        let endpoint = given_function(&emulator, &format!("notify-{index}"), grants).await;
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "lambda"),
                ("Endpoint", &endpoint),
            ],
        )
        .await;
        for (name, value) in [
            ("LambdaSuccessFeedbackRoleArn", role.as_str()),
            ("LambdaFailureFeedbackRoleArn", role.as_str()),
            ("LambdaSuccessFeedbackSampleRate", "100"),
        ] {
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", name),
                    ("AttributeValue", value),
                ],
            )
            .await;
        }

        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a notification")],
        )
        .await;

        let log_group = group_of(&topic, expected);
        let records = records_within(&emulator, &log_group, RECORD_BUDGET).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one delivery-status record in {log_group}, got {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {record}"
        );
        assert_eq!(
            record["delivery"]["destination"].as_str(),
            Some(endpoint.as_str()),
            "case {case:?}: the record must name the function delivered to, got {record}"
        );
        assert_eq!(
            record["delivery"]["statusCode"].as_u64(),
            Some(code),
            "case {case:?}: expected statusCode {code}, got {record}"
        );
        let Some(error_code) = error_code else {
            // Lambda's own answer about the invocation it accepted: the request it answered.
            let response = provider_response(case, record);
            assert!(
                response["lambdaRequestId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: the provider response must name Lambda's invocation, got \
                 {response}"
            );
            continue;
        };
        let response = provider_response(case, record);
        assert_eq!(
            response["ErrorCode"].as_str(),
            Some(error_code),
            "case {case:?}: expected the {error_code} code, got {response}"
        );
        assert_eq!(
            response["ErrorMessage"].as_str(),
            Some(not_authorized("lambda:InvokeFunction", &endpoint).as_str()),
            "case {case:?}: expected the refusal AWS words, got {response}"
        );
    }
}

/// RULE: one record per DELIVERY — so a topic with two subscribed endpoints logs one record
/// naming each of them, and a second publish logs one more per endpoint.
#[tokio::test]
async fn one_record_is_written_per_delivery_of_the_topic() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "fan-out")]).await,
        "TopicArn",
    );
    let role = given_role(
        &emulator,
        "sns-fan-out-feedback",
        SNS_PRINCIPAL,
        Permissions::WritesLogs,
    )
    .await;
    let (_, first) = given_queue(&emulator, "fan-out-one", true).await;
    let (_, second) = given_queue(&emulator, "fan-out-two", true).await;
    for endpoint in [&first, &second] {
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }
    for (name, value) in [
        ("SQSSuccessFeedbackRoleArn", role.as_str()),
        ("SQSSuccessFeedbackSampleRate", "100"),
    ] {
        sns(
            &emulator,
            "SetTopicAttributes",
            &[
                ("TopicArn", &topic),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    }

    let log_group = feedback_log_group(&topic);
    // The rows differ in how many publishes precede the read: each one is delivered to both
    // endpoints, so the group holds two records per publish.
    for (case, publishes, expected) in [("one publish", 1, 2), ("a second publish", 2, 4)] {
        if case == "a second publish" {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", "again")],
            )
            .await;
        } else {
            sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", "first")],
            )
            .await;
        }
        let deadline = Instant::now() + RECORD_BUDGET;
        let mut records = records_in(&emulator, &log_group).await;
        while records.len() < expected && Instant::now() < deadline {
            tokio::time::sleep(LOOK_INTERVAL).await;
            records = records_in(&emulator, &log_group).await;
        }
        assert_eq!(
            records.len(),
            expected,
            "case {case:?}: expected {expected} record(s) after {publishes} publish(es), got \
             {records:?}"
        );
        let destinations: Vec<&str> = records
            .iter()
            .filter_map(|record| record["delivery"]["destination"].as_str())
            .collect();
        for endpoint in [&first, &second] {
            assert!(
                destinations.contains(&endpoint.as_str()),
                "case {case:?}: expected a record naming {endpoint}, got {destinations:?}"
            );
        }
    }
}

/// The MD5 digest of `body`, in lowercase hexadecimal — the digest function itself, so a row
/// with no published test value of its own still states what the record must carry.
fn md5_hex(body: &str) -> String {
    use md5::{Digest, Md5};

    hex::encode(Md5::digest(body.as_bytes()))
}

/// One row of the digest table: the case, the body published, and the digest the record must
/// report for it (`None` for a body with no published test value, whose digest is taken with
/// [`md5_hex`]).
type DigestCase = (&'static str, String, Option<&'static str>);

/// The bodies whose digest the table asserts.
///
/// The first four are RFC 1321's own published MD5 test values (its "MD5 test suite"), which
/// pin the digest to that function rather than to whatever this emulator computes; the rest
/// differ along what the rule generalises over — the body's length, its character set and its
/// shape.
fn digest_rows() -> Vec<DigestCase> {
    vec![
        (
            "a one-byte body",
            "a".to_string(),
            Some("0cc175b9c0f1b6a831c399e269772661"),
        ),
        (
            "a three-byte body",
            "abc".to_string(),
            Some("900150983cd24fb0d6963f7d28e17f72"),
        ),
        (
            "a body with a space in it",
            "message digest".to_string(),
            Some("f96b697d7cb7938d525a2f31aaf161d0"),
        ),
        (
            "the whole lower-case alphabet",
            "abcdefghijklmnopqrstuvwxyz".to_string(),
            Some("c3fcd3d76192e4007dfb496cca67e13b"),
        ),
        (
            "a multi-byte body",
            "notificación — ✉".to_string(),
            None,
        ),
        (
            "a JSON body",
            r#"{"alarm":"CPUHigh","value":97.5}"#.to_string(),
            None,
        ),
        // Far longer than one MD5 block, so the digest spans many of them.
        (
            "a body of eight thousand bytes",
            "payload-".repeat(1_000),
            None,
        ),
    ]
}

/// Wait until `log_group` holds `expected` record(s) of `message_id`, and answer them.
async fn records_of(
    emulator: &Emulator,
    log_group: &str,
    message_id: &str,
    expected: usize,
    case: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + RECORD_BUDGET;
    loop {
        let records: Vec<Value> = records_in(emulator, log_group)
            .await
            .into_iter()
            .filter(|record| record["notification"]["messageId"].as_str() == Some(message_id))
            .collect();
        if records.len() >= expected {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {expected} delivery-status record(s) for {message_id} in \
             {log_group}, got {records:?}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// RULE: a delivery-status record's `notification` block reports the digest of the PUBLISHED
/// MESSAGE'S BODY as `messageMD5Sum`, beside the message's id, its topic and its publish time —
/// for every delivery of that message, whatever the outcome of the attempt.
///
/// The rows differ along the body the digest is taken of; every row is published to ONE topic
/// with TWO subscribed queues, so each row also asserts that the two records of one message
/// report the same digest (it is taken of the content, not of the delivery), and that bodies
/// that differ report digests that differ.
#[tokio::test]
async fn a_record_reports_the_digest_of_the_published_body() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "digest")]).await,
        "TopicArn",
    );
    let role = given_role(
        &emulator,
        "sns-digest-feedback",
        SNS_PRINCIPAL,
        Permissions::WritesLogs,
    )
    .await;
    let (_, first) = given_queue(&emulator, "digest-one", true).await;
    let (_, second) = given_queue(&emulator, "digest-two", true).await;
    for endpoint in [&first, &second] {
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", endpoint),
            ],
        )
        .await;
    }
    for (name, value) in [
        ("SQSSuccessFeedbackRoleArn", role.as_str()),
        ("SQSSuccessFeedbackSampleRate", "100"),
    ] {
        sns(
            &emulator,
            "SetTopicAttributes",
            &[
                ("TopicArn", &topic),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    }

    let log_group = feedback_log_group(&topic);
    let mut seen: Vec<(String, String)> = Vec::new();
    for (case, body, published_digest) in digest_rows() {
        let expected = published_digest
            .map(str::to_string)
            .unwrap_or_else(|| md5_hex(&body));
        let message_id = element(
            &sns(
                &emulator,
                "Publish",
                &[("TopicArn", &topic), ("Message", &body)],
            )
            .await,
            "MessageId",
        );
        let records = records_of(&emulator, &log_group, &message_id, 2, case).await;
        assert_eq!(
            records.len(),
            2,
            "case {case:?}: expected one record per subscribed queue, got {records:?}"
        );
        for record in &records {
            assert_eq!(
                record["notification"]["messageMD5Sum"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected messageMD5Sum {expected:?}, got {record}"
            );
        }
        // The two records are of two different deliveries of ONE message: the digest is of the
        // body, so neither the endpoint nor the delivery changes it.
        let destinations: Vec<&str> = records
            .iter()
            .filter_map(|record| record["delivery"]["destination"].as_str())
            .collect();
        for endpoint in [&first, &second] {
            assert!(
                destinations.contains(&endpoint.as_str()),
                "case {case:?}: expected a record naming {endpoint}, got {destinations:?}"
            );
        }
        for (earlier, digest) in &seen {
            assert!(
                (earlier == &body) == (digest == &expected),
                "case {case:?}: a body differing from {earlier:?} must digest differently, got \
                 {digest:?} and {expected:?}"
            );
        }
        seen.push((body, expected));
    }
}

/// RULE (same rule, the other outcome): a FAILED delivery's record carries the same
/// `notification` block, so the digest of the published body is reported in the `/Failure`
/// group too.
#[tokio::test]
async fn a_failed_deliverys_record_reports_the_digest_too() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "digest-failure")]).await,
        "TopicArn",
    );
    let role = given_role(
        &emulator,
        "sns-digest-failure-feedback",
        SNS_PRINCIPAL,
        Permissions::WritesLogs,
    )
    .await;
    // A queue granting this service nothing: the delivery is withheld, so the record is a
    // FAILURE.
    let (_, endpoint) = given_queue(&emulator, "digest-failure", false).await;
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &endpoint),
        ],
    )
    .await;
    sns(
        &emulator,
        "SetTopicAttributes",
        &[
            ("TopicArn", &topic),
            ("AttributeName", "SQSFailureFeedbackRoleArn"),
            ("AttributeValue", &role),
        ],
    )
    .await;

    let case = "a delivery the queue's own policy withheld";
    let body = "abc";
    let message_id = element(
        &sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", body)],
        )
        .await,
        "MessageId",
    );
    let records = records_of(&emulator, &failure_log_group(&topic), &message_id, 1, case).await;
    let record = &records[0];
    assert_eq!(
        record["status"].as_str(),
        Some("FAILURE"),
        "case {case:?}: expected a FAILURE record, got {record}"
    );
    assert_eq!(
        record["notification"]["messageMD5Sum"].as_str(),
        Some(md5_hex(body).as_str()),
        "case {case:?}: expected the body's digest, got {record}"
    );
}

/// Every log group CloudWatch Logs reports whose name starts with `prefix`, as a client
/// DISCOVERS the groups a feedback role created (`logs:DescribeLogGroups`, whose
/// `logGroupNamePrefix` "matches the beginning of a log group name") — the empty list for a
/// prefix no group is under.
async fn described_groups(emulator: &Emulator, prefix: &str) -> Vec<String> {
    let (status, described) = logs(
        emulator,
        "DescribeLogGroups",
        &json!({"logGroupNamePrefix": prefix}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeLogGroups {prefix}: expected HTTP 200, got {status} with {described}"
    );
    described["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
        .collect()
}

/// Wait until CloudWatch Logs reports a group under `prefix`, and answer the ones it reported —
/// or the empty list once `budget` is spent, which is what says nothing was created.
async fn described_groups_within(
    emulator: &Emulator,
    prefix: &str,
    budget: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + budget;
    loop {
        let groups = described_groups(emulator, prefix).await;
        if !groups.is_empty() || Instant::now() >= deadline {
            return groups;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Wait until Lambda holds the asynchronous invocation a delivery to `function` makes, and
/// answer how many it holds: the evidence that a delivery was attempted at all, which is what a
/// row expecting NO record needs before it can read an absent log group as a decision rather
/// than as an attempt that has not been made yet.
async fn invocations_within(emulator: &Emulator, function: &str) -> usize {
    let deadline = Instant::now() + RECORD_BUDGET;
    loop {
        let held = {
            let state = emulator
                .engine
                .state::<ComAmazonawsLambdaAwsgirapiserviceState>();
            let store = state.lock();
            store
                .functions
                .get(function)
                .map(|record| record.async_invocations.len())
                .unwrap_or_default()
        };
        if held > 0 || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// What one row of the `lambda` feedback table configures, and what must come of it.
struct FunctionRow {
    case: &'static str,
    /// The topic's own name, which is the last field of the log group's name: the rows differ
    /// along it, because the group's name is built from the topic rather than fixed.
    topic_name: &'static str,
    /// The feedback attributes the topic is given, with the role ARN substituted for
    /// [`ROLE_PLACEHOLDER`].
    attributes: &'static [(&'static str, &'static str)],
    /// Whether the subscribed function's own policy grants this service the invoke, which is
    /// what decides whether the delivery succeeds.
    grants: bool,
    /// The `status` the record must report, and `None` for a configuration under which no
    /// group may be created at all.
    status: Option<&'static str>,
}

/// The attribute names below are this service's own closed set for the delivery-status feature
/// (`SetTopicAttributesInput$AttributeName`), whose per-protocol prefixes are `Application`,
/// `Firehose`, `HTTP`, `Lambda` and `SQS` — the rows exercise the `Lambda` pair against a
/// `lambda` delivery, and the `SQS` and `HTTP` pairs against it, which must log nothing.
fn function_rows() -> Vec<FunctionRow> {
    vec![
        FunctionRow {
            case: "a full sample of successes over the delivery's own protocol",
            topic_name: "lambda-feedback",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            grants: true,
            status: Some("SUCCESS"),
        },
        FunctionRow {
            case: "the same, for a topic whose name carries digits and hyphens",
            topic_name: "ops-alerts-2",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            grants: true,
            status: Some("SUCCESS"),
        },
        FunctionRow {
            case: "a failure role, and a function this service may not invoke",
            topic_name: "WithheldDeliveries",
            attributes: &[
                ("LambdaFailureFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            grants: false,
            status: Some("FAILURE"),
        },
        FunctionRow {
            case: "both roles of the protocol, and a delivery the function took",
            topic_name: "both-roles",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaFailureFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "100"),
            ],
            grants: true,
            status: Some("SUCCESS"),
        },
        FunctionRow {
            case: "a full sample, but set for the sqs protocol",
            topic_name: "sqs-prefixed",
            attributes: &[
                ("SQSSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("SQSSuccessFeedbackSampleRate", "100"),
            ],
            grants: true,
            status: None,
        },
        FunctionRow {
            case: "a full sample, but set for the http protocol",
            topic_name: "http-prefixed",
            attributes: &[
                ("HTTPSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("HTTPSuccessFeedbackSampleRate", "100"),
            ],
            grants: true,
            status: None,
        },
        FunctionRow {
            case: "the protocol's own success role, sampling none of its successes",
            topic_name: "unsampled",
            attributes: &[
                ("LambdaSuccessFeedbackRoleArn", ROLE_PLACEHOLDER),
                ("LambdaSuccessFeedbackSampleRate", "0"),
            ],
            grants: true,
            status: None,
        },
        FunctionRow {
            case: "a failure role only, and a delivery the function took",
            topic_name: "failures-only",
            attributes: &[("LambdaFailureFeedbackRoleArn", ROLE_PLACEHOLDER)],
            grants: true,
            status: None,
        },
    ]
}

/// RULE: a topic that names a feedback role for the protocol a delivery went over has this
/// service assume that role and write the delivery's record into the CloudWatch Logs group
/// named `sns/<region>/<account-id>/<topic-name>` from the topic's own ARN — a SUCCESS into
/// that group, at the sampled percentage, and a FAILURE into its `/Failure` child — so a client
/// DISCOVERS the group under that name through `logs:DescribeLogGroups` and reads a record there
/// naming the endpoint delivered to. A topic that names a role only for ANOTHER protocol, or
/// that samples none of its successes, leaves no group of either kind.
///
/// The rows differ along what the rule generalises over: the topic's name (which the group's
/// name is built from), which of the protocol's feedback attributes the topic carries, the
/// protocol those attributes were set for as against the one the delivery went over, the sample
/// rate, and whether the subscribed function's own policy admits the delivery.
#[tokio::test]
async fn a_lambda_delivery_is_logged_in_the_topics_own_log_group() {
    for (index, row) in function_rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let topic = element(
            &sns(&emulator, "CreateTopic", &[("Name", row.topic_name)]).await,
            "TopicArn",
        );
        let role = given_role(
            &emulator,
            &format!("sns-lambda-feedback-{index}"),
            SNS_PRINCIPAL,
            Permissions::WritesLogs,
        )
        .await;
        let function = format!("notified-{index}");
        let endpoint = given_function(&emulator, &function, row.grants).await;
        sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "lambda"),
                ("Endpoint", &endpoint),
            ],
        )
        .await;
        for (name, value) in row.attributes {
            let value = value.replace(ROLE_PLACEHOLDER, &role);
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", name),
                    ("AttributeValue", &value),
                ],
            )
            .await;
        }

        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", "a notification")],
        )
        .await;

        // The group every record of this topic goes into, and the prefix both it and its
        // `/Failure` child are under — spelled out from the template rather than through the
        // helpers above, so the assertions below bind the name the SERVICE created to the
        // Region every call here is signed for, the account the topic belongs to and this
        // row's own topic name.
        let account = topic
            .split(':')
            .nth(4)
            .expect("a topic ARN names the account it belongs to");
        let base = format!("sns/{REGION}/{account}/{}", row.topic_name);
        let Some(expected) = row.status else {
            // The DELIVERY itself happened — the subscribed function holds the asynchronous
            // invocation it was made with — so a group that is not there is one this
            // configuration never asked for, rather than an attempt still outstanding.
            let invocations = invocations_within(&emulator, &function).await;
            assert_eq!(
                invocations, 1,
                "case {case:?}: expected the delivery to have invoked {function} once, got \
                 {invocations}"
            );
            let groups = described_groups_within(&emulator, &base, QUIET_BUDGET).await;
            assert!(
                groups.is_empty(),
                "case {case:?}: this topic asks for no record of a lambda delivery, so no \
                 group under {base} may be created, got {groups:?}"
            );
            continue;
        };
        let log_group = if expected == "SUCCESS" {
            base.clone()
        } else {
            format!("{base}/Failure")
        };
        let groups = described_groups_within(&emulator, &base, RECORD_BUDGET).await;
        assert_eq!(
            groups,
            vec![log_group.clone()],
            "case {case:?}: expected CloudWatch Logs to report exactly the group \
             {log_group}, got {groups:?}"
        );
        let records = records_within(&emulator, &log_group, RECORD_BUDGET).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: expected one delivery-status record in {log_group}, got {records:?}"
        );
        let record = &records[0];
        assert_eq!(
            record["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {record}"
        );
        assert_eq!(
            record["delivery"]["destination"].as_str(),
            Some(endpoint.as_str()),
            "case {case:?}: the record must name the function delivered to, got {record}"
        );
        assert_eq!(
            record["notification"]["topicArn"].as_str(),
            Some(topic.as_str()),
            "case {case:?}: the record must name the topic published to, got {record}"
        );
    }
}
