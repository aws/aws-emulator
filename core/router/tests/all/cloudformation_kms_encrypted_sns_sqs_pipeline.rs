// SPDX-License-Identifier: CC0-1.0

//! The compliance-estate pipeline a CloudFormation stack deploys: one customer managed KMS
//! key, an Amazon SNS topic and an Amazon SQS queue both encrypted at rest with it, the queue
//! policy that lets the topic write into the queue, and an SNS subscription binding the two —
//! and a published message that the subscriber reads in PLAINTEXT.
//!
//! The rule under test: **encryption at rest is transparent to the subscriber.** A topic and
//! a queue carrying a `KmsMasterKeyId` deliver and receive exactly what an unencrypted pair
//! does, provided the documents the delivery passes through permit it — the queue's own policy
//! (`sqs:SendMessage` for the service principal SNS delivers as) and the KEY's policy (the
//! data-key operations SNS performs to seal the message into the queue). So the envelope the
//! subscriber reads carries the published body byte for byte, the topic's ARN, and the
//! published message attributes, whichever of the two resources is on a customer managed key.
//!
//! Four crates have to agree for that to hold (CloudFormation provisions the four resources
//! and resolves the references between them, KMS mints the key and decides the data-key grant,
//! SNS performs the delivery as a service principal, Amazon SQS stores the message under the
//! queue's key), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: WHICH of the two resources the
//! stack's key encrypts (both, the queue alone, the topic alone, neither), WHICH identifier
//! form the resources are keyed by (the key id a `Ref` reads, the key ARN a `Fn::GetAtt`
//! reads), and what the published message carries (a body with message attributes, a body
//! with none, a body of multi-byte text). The stack's own convergence is waited for by
//! POLLING its status, and the delivery by polling the queue, so neither outcome depends on
//! how fast the host runs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources and
/// their ARNs belong to.
const REGION: &str = "us-east-1";

/// How long the stack is given to converge, and the delivery to arrive, before the suite
/// gives up: far longer than the clock-owed transitions of four resources, so a loaded host
/// cannot decide the outcome.
const DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one operation over an awsQuery wire (CloudFormation and SNS both speak it).
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

/// Call one CloudFormation operation.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one SNS operation.
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_call(router, "sqs", "AmazonSQS", operation, request).await
}

/// Call one KMS operation over its awsJson1_1 wire.
async fn kms(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_call(router, "kms", "TrentService", operation, request).await
}

/// One awsJson call, over the target prefix the service's protocol names.
async fn json_call(
    router: &MultiServiceRouter,
    service: &str,
    target_prefix: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let version = if service == "kms" { "1.1" } else { "1.0" };
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(service))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = kms(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One output of a settled stack.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {outputs:?}"))
        .clone()
}

/// Create the stack and POLL its status until it reaches `CREATE_COMPLETE`, answering the
/// `DescribeStacks` body it settled with.
///
/// The wait is for the CONDITION rather than for a time: a stack of four resources converges
/// over as many clock-owed transitions as it has dependency levels, and what this suite is
/// about is the pipeline it leaves behind.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == "CREATE_COMPLETE" {
            return body;
        }
        assert!(
            reported == "CREATE_IN_PROGRESS",
            "case {case:?}: expected the stack to be creating or complete, got {reported:?} in \
             {body}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack never reached CREATE_COMPLETE, last {reported:?} in {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The key policy the compliance template writes: the account root administers the key, and
/// the service principal SNS delivers as is allowed the data-key operations a write into an
/// encrypted queue performs.
fn key_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AccountAdministers",
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
                "Action": "kms:*",
                "Resource": "*",
            },
            {
                "Sid": "SnsSealsIntoTheQueue",
                "Effect": "Allow",
                "Principal": {"Service": "sns.amazonaws.com"},
                "Action": ["kms:GenerateDataKey*", "kms:Decrypt"],
                "Resource": "*",
            },
        ],
    })
}

/// The whole compliance template: the key, the topic and the queue (each encrypted with that
/// key where the row asks for it), the queue policy the subscribed queue needs, and the
/// subscription that binds the topic to the queue.
fn pipeline_template(topic_name: &str, queue_name: &str, encrypt: Encrypt) -> String {
    let keyed = encrypt.keyed_by.intrinsic();
    let mut topic = json!({"TopicName": topic_name});
    if encrypt.topic {
        topic["KmsMasterKeyId"] = keyed.clone();
    }
    let mut queue = json!({"QueueName": queue_name, "SqsManagedSseEnabled": false});
    if encrypt.queue {
        queue["KmsMasterKeyId"] = keyed.clone();
    }
    json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {"KeyPolicy": key_policy()}},
            "Topic": {"Type": "AWS::SNS::Topic", "Properties": topic},
            "Queue": {"Type": "AWS::SQS::Queue", "Properties": queue},
            "QueueAccess": {
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{"Ref": "Queue"}],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Sid": "TopicSends",
                            "Effect": "Allow",
                            "Principal": {"Service": "sns.amazonaws.com"},
                            "Action": "sqs:SendMessage",
                            "Resource": {"Fn::GetAtt": ["Queue", "Arn"]},
                            "Condition": {
                                "ArnEquals": {"aws:SourceArn": {"Ref": "Topic"}},
                            },
                        }],
                    },
                },
            },
            "Subscription": {
                "Type": "AWS::SNS::Subscription",
                "Properties": {
                    "TopicArn": {"Ref": "Topic"},
                    "Protocol": "sqs",
                    "Endpoint": {"Fn::GetAtt": ["Queue", "Arn"]},
                },
            },
        },
        "Outputs": {
            "KeyId": {"Value": {"Ref": "Key"}},
            "KeyArn": {"Value": {"Fn::GetAtt": ["Key", "Arn"]}},
            "TopicArn": {"Value": {"Ref": "Topic"}},
            "QueueUrl": {"Value": {"Ref": "Queue"}},
            "QueueArn": {"Value": {"Fn::GetAtt": ["Queue", "Arn"]}},
        },
    })
    .to_string()
}

/// Which identifier form a resource is keyed by: the two the stack can read the key's identity
/// with, which are the two forms AWS documents for `KmsMasterKeyId` — "the ID of an Amazon Web
/// Services managed customer master key" or its ARN.
#[derive(Debug, Clone, Copy)]
enum KeyedBy {
    /// `Ref` on an `AWS::KMS::Key`, which reads the key's ID.
    KeyId,
    /// `Fn::GetAtt … Arn`, which reads the key's ARN.
    KeyArn,
}

impl KeyedBy {
    /// The template intrinsic that reads this form of the key's identity.
    fn intrinsic(self) -> Value {
        match self {
            Self::KeyId => json!({"Ref": "Key"}),
            Self::KeyArn => json!({"Fn::GetAtt": ["Key", "Arn"]}),
        }
    }

    /// The stack output that carries this form of the key's identity.
    fn output_key(self) -> &'static str {
        match self {
            Self::KeyId => "KeyId",
            Self::KeyArn => "KeyArn",
        }
    }
}

/// Which of the pipeline's two resources the stack's own key encrypts, and which form of the
/// key's identity they are keyed by.
#[derive(Debug, Clone, Copy)]
struct Encrypt {
    topic: bool,
    queue: bool,
    keyed_by: KeyedBy,
}

/// One published message, and the attributes it carries.
struct Published<'a> {
    case: &'a str,
    body: &'a str,
    /// `(name, data type, value)` for the one message attribute the publish carries, when it
    /// carries any.
    attribute: Option<(&'a str, &'a str, &'a str)>,
}

/// Poll the queue until a message arrives, answering its body. The wait is for the CONDITION
/// rather than for a time.
async fn received(router: &MultiServiceRouter, case: &str, queue_url: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let answer = sqs_ok(
            router,
            case,
            "ReceiveMessage",
            json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
        )
        .await;
        if let Some(message) = answer["Messages"].as_array().and_then(|held| held.first()) {
            return message.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: no notification ever reached the subscribed queue"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A stack-deployed, key-encrypted topic and queue deliver the published message to the
/// subscriber in plaintext — whichever of the two the key encrypts, and whatever the message
/// carries.
#[tokio::test]
async fn a_stack_deployed_kms_encrypted_topic_and_queue_deliver_plaintext() {
    let router = build_router();
    // (case, what the key encrypts and how the resources name it)
    let rows: &[(&str, Encrypt)] = &[
        (
            "a topic and a queue both keyed by the key's ARN",
            Encrypt {
                topic: true,
                queue: true,
                keyed_by: KeyedBy::KeyArn,
            },
        ),
        (
            "a topic and a queue both keyed by the key's ID",
            Encrypt {
                topic: true,
                queue: true,
                keyed_by: KeyedBy::KeyId,
            },
        ),
        (
            "only the queue on the stack's own key",
            Encrypt {
                topic: false,
                queue: true,
                keyed_by: KeyedBy::KeyArn,
            },
        ),
        (
            "only the topic on the stack's own key, keyed by its ID",
            Encrypt {
                topic: true,
                queue: false,
                keyed_by: KeyedBy::KeyId,
            },
        ),
        (
            "neither resource encrypted at rest",
            Encrypt {
                topic: false,
                queue: false,
                keyed_by: KeyedBy::KeyArn,
            },
        ),
    ];

    for (row, (case, encrypt)) in rows.iter().enumerate() {
        let stack_name = format!("kms-pipeline-{row}");
        let topic_name = format!("kms-pipeline-topic-{row}");
        let queue_name = format!("kms-pipeline-queue-{row}");
        let body = create_and_settle(
            &router,
            case,
            &stack_name,
            &pipeline_template(&topic_name, &queue_name, *encrypt),
        )
        .await;
        let outputs = outputs_of(&body);
        let key_arn = output(&outputs, case, "KeyArn");
        // The identity the resources were keyed by, as the stack published it.
        let keyed = output(&outputs, case, encrypt.keyed_by.output_key());
        let topic_arn = output(&outputs, case, "TopicArn");
        let queue_url = output(&outputs, case, "QueueUrl");

        // Whichever form the resources name it by, it resolves in KMS to the key the stack
        // created — so what they report is an identifier of that key and not a value
        // CloudFormation computed.
        let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": keyed})).await;
        assert_eq!(
            described["KeyMetadata"]["Arn"].as_str(),
            Some(key_arn.as_str()),
            "case {case:?}: the identity the resources are keyed by must resolve to the stack's \
             key, got {described}"
        );

        // Both resources report the key the stack created, exactly where a client reads it.
        let (status, reported) = sns(&router, "GetTopicAttributes", &[("TopicArn", &topic_arn)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTopicAttributes, got {status} / {reported}"
        );
        let topic_key = super::query_wire::decoded_entries(&reported)
            .get("KmsMasterKeyId")
            .cloned();
        assert_eq!(
            topic_key.as_deref(),
            encrypt.topic.then_some(keyed.as_str()),
            "case {case:?}: expected the topic's KmsMasterKeyId to be the stack's key, got \
             {topic_key:?} in {reported}"
        );
        let attributes = sqs_ok(
            &router,
            case,
            "GetQueueAttributes",
            json!({"QueueUrl": queue_url, "AttributeNames": ["All"]}),
        )
        .await;
        assert_eq!(
            attributes["Attributes"]["KmsMasterKeyId"].as_str(),
            encrypt.queue.then_some(keyed.as_str()),
            "case {case:?}: expected the queue's KmsMasterKeyId to be the stack's key, got \
             {attributes}"
        );

        // The messages the rows publish differ in what they carry: attributes or none, and
        // single-byte or multi-byte text.
        let published = [
            Published {
                case: "a body with one string message attribute",
                body: "order 4711 placed",
                attribute: Some(("kind", "String", "sim2real-order")),
            },
            Published {
                case: "a body with no message attribute at all",
                body: "order 4712 placed",
                attribute: None,
            },
            Published {
                case: "a body of multi-byte text",
                body: "pedido aceptado — 4713 ✅",
                attribute: Some(("kind", "String", "sim2real-order")),
            },
        ];

        for message in &published {
            let case = format!("{case} / {}", message.case);
            let mut members: Vec<(&str, &str)> =
                vec![("TopicArn", &topic_arn), ("Message", message.body)];
            if let Some((name, data_type, value)) = message.attribute {
                members.push(("MessageAttributes.entry.1.Name", name));
                members.push(("MessageAttributes.entry.1.Value.DataType", data_type));
                members.push(("MessageAttributes.entry.1.Value.StringValue", value));
            }
            let (status, answer) = sns(&router, "Publish", &members).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from Publish, got {status} / {answer}"
            );
            let message_id = element(&answer, "MessageId");

            let held = received(&router, &case, &queue_url).await;
            let text = held["Body"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Body in {held}"));
            let envelope: Value = serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("case {case:?}: body is not JSON ({error}): {text}"));
            assert_eq!(
                envelope["Message"].as_str(),
                Some(message.body),
                "case {case:?}: the subscriber must read the published body byte for byte, got \
                 {envelope}"
            );
            assert_eq!(
                envelope["TopicArn"].as_str(),
                Some(topic_arn.as_str()),
                "case {case:?}: the envelope must name the stack's topic, got {envelope}"
            );
            assert_eq!(
                envelope["MessageId"].as_str(),
                Some(message_id.as_str()),
                "case {case:?}: the envelope must carry the id the publish answered with, got \
                 {envelope}"
            );
            match message.attribute {
                Some((name, data_type, value)) => assert_eq!(
                    envelope["MessageAttributes"][name],
                    json!({"Type": data_type, "Value": value}),
                    "case {case:?}: the envelope must carry the published attribute, got \
                     {envelope}"
                ),
                None => assert_eq!(
                    envelope["MessageAttributes"],
                    Value::Null,
                    "case {case:?}: a publish carrying no attribute delivers none, got \
                     {envelope}"
                ),
            }

            // Reading it back out of the queue took the message: nothing else is waiting.
            sqs_ok(
                &router,
                &case,
                "DeleteMessage",
                json!({
                    "QueueUrl": queue_url,
                    "ReceiptHandle": held["ReceiptHandle"].clone(),
                }),
            )
            .await;
        }
    }
}
