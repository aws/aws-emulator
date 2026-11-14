// SPDX-License-Identifier: CC0-1.0

//! An API Gateway stage whose method integrates an AWS SERVICE ACTION on an SNS TOPIC, driven
//! through the ASSEMBLED emulator: the whole path a client takes from the stage invoke URL to
//! the message on the topic's SQS subscriber.
//!
//! RULE: a deployed method whose integration is of type `AWS` is served by MAKING the call its
//! `uri` and its integration request name against the service that `uri`'s subdomain names —
//! `Integration$uri` gives the form
//! `arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}` — and the method
//! response is the answer that service gave. Amazon SNS's api is a QUERY api, so the action
//! and its parameters travel in the integration request as that wire form carries them, and
//! every field reaches the service as that request spelled it: the MESSAGE ATTRIBUTES an
//! integration sets on a publish (`MessageAttributes.entry.{n}.…`) are the ones the topic's
//! subscribers are delivered.
//!
//! This is the cross-service twin of the owning crate's own
//! `tests/all/sns_service_integration.rs`, which varies the `uri` forms, the places the
//! parameters come from, the bodies and the roles: here the same rule is asked of the router
//! that mounts every service at once, which is the emulator a client actually reaches, so
//! what is pinned is that the hop between the stage and the topic is served by the assembled
//! emulator and not only by a suite's own pair of services.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the Region the integration ARN names.
const REGION: &str = "us-east-1";

/// The api version every `awsQuery` request to Amazon SNS states: that service's model
/// version.
const SNS_VERSION: &str = "2010-03-31";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The media type the method request declares, which is the key its integration request
/// template is stored under.
const JSON: &str = "application/json";

/// The topic the integration publishes to.
const TOPIC: &str = "assembled-orders";

/// The queue subscribed to it, which is where the delivered message is read.
const QUEUE: &str = "assembled-subscriber";

/// The four services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
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
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire, answering its status and its body.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> (u16, String) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(&response.into_body()).into_owned())
}

/// One API Gateway control-plane call that has to answer `expected`, as its parsed body.
async fn apigw_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Value,
    expected: u16,
) -> Value {
    let (status, text) = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", JSON),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {status} with \
         {text:?}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One Amazon SNS `awsQuery` call that has to succeed, answering its body. Every value named
/// here is an ARN or a name, which carries none of the characters a form body treats
/// specially, so nothing is encoded.
async fn sns_ok(router: &MultiServiceRouter, action: &str, fields: &str) -> String {
    let (status, text) = dispatch(
        router,
        "POST",
        &format!("sns.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("sns")),
        ],
        Bytes::from(format!("Action={action}&Version={SNS_VERSION}&{fields}")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {action}: expected HTTP 200 from Amazon SNS, got {status} with {text:?}"
    );
    text
}

/// One Amazon SQS call that has to succeed, as its parsed body.
async fn sqs_ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, text) = dispatch(
        router,
        "POST",
        &format!("sqs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
            ("authorization", &authorization("sqs")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {operation}: expected HTTP 200 from Amazon SQS, got {status} with {text:?}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

#[tokio::test]
async fn a_stage_publish_reaches_the_topics_queue_subscriber_through_the_assembled_emulator() {
    let router = emulator();
    let topic_arn = xml_element(&sns_ok(&router, "CreateTopic", &format!("Name={TOPIC}")).await, "TopicArn")
        .expect("CreateTopic answers the topic's ARN");
    let queue_url = member(
        "CreateQueue",
        &sqs_ok(&router, "CreateQueue", json!({ "QueueName": QUEUE })).await,
        "QueueUrl",
    );
    let attributes = sqs_ok(
        &router,
        "GetQueueAttributes",
        json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = member(
        "GetQueueAttributes",
        attributes.get("Attributes").unwrap_or(&Value::Null),
        "QueueArn",
    );
    // The queue policy AWS documents for a queue subscribed to a topic, and the RAW delivery
    // that makes the published attributes the queue's own message attributes.
    sqs_ok(
        &router,
        "SetQueueAttributes",
        json!({
            "QueueUrl": queue_url,
            "Attributes": {
                "Policy": json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Sid": "AllowSNSDelivery",
                        "Effect": "Allow",
                        "Principal": { "Service": "sns.amazonaws.com" },
                        "Action": "sqs:SendMessage",
                        "Resource": queue_arn,
                    }],
                })
                .to_string(),
            },
        }),
    )
    .await;
    sns_ok(
        &router,
        "Subscribe",
        &format!(
            "TopicArn={topic_arn}&Protocol=sqs&Endpoint={queue_arn}\
             &Attributes.entry.1.key=RawMessageDelivery&Attributes.entry.1.value=true"
        ),
    )
    .await;

    let created = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        json!({ "name": "assembled-topic-integration" }),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let resource = member(
        "CreateResource",
        &apigw_ok(
            &router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            json!({ "pathPart": "publish" }),
            201,
        )
        .await,
        "id",
    );
    apigw_ok(
        &router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/POST"),
        json!({ "authorizationType": "NONE" }),
        201,
    )
    .await;
    apigw_ok(
        &router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/POST/integration"),
        json!({
            "type": "AWS",
            "httpMethod": "POST",
            "uri": format!("arn:aws:apigateway:{REGION}:sns:action/Publish"),
            "requestTemplates": {
                JSON: format!(
                    "TopicArn={topic_arn}&Message=$util.urlEncode($input.body)\
                     &MessageAttributes.entry.1.Name=kind\
                     &MessageAttributes.entry.1.Value.DataType=String\
                     &MessageAttributes.entry.1.Value.StringValue=created"
                ),
            },
        }),
        201,
    )
    .await;
    apigw_ok(
        &router,
        "PutMethodResponse",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/POST/responses/200"),
        json!({}),
        201,
    )
    .await;
    apigw_ok(
        &router,
        "PutIntegrationResponse",
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/POST/integration/responses/200"),
        json!({ "statusCode": "200" }),
        201,
    )
    .await;
    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        json!({ "stageName": STAGE }),
        201,
    )
    .await;

    let sent = "the assembled order";
    let (status, answered) = dispatch(
        &router,
        "POST",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}/publish"),
        &[("content-type", JSON)],
        Bytes::from(sent),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the topic's own 200, got {status} with {answered:?}",
        "a publish through the assembled emulator"
    );
    assert!(
        xml_element(&answered, "MessageId").is_some_and(|id| !id.is_empty()),
        "case {:?}: expected a MessageId the topic minted, got {answered:?}",
        "a publish through the assembled emulator"
    );

    let received = sqs_ok(
        &router,
        "ReceiveMessage",
        json!({
            "QueueUrl": queue_url,
            "MaxNumberOfMessages": 10,
            "MessageAttributeNames": ["All"],
        }),
    )
    .await;
    let messages = received
        .get("Messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        messages.len(),
        1,
        "case {:?}: expected the one message the stage published, got {received}",
        "a publish through the assembled emulator"
    );
    assert_eq!(
        messages[0].get("Body").and_then(Value::as_str),
        Some(sent),
        "case {:?}: expected the body the client sent, got {}",
        "a publish through the assembled emulator",
        messages[0]
    );
    assert_eq!(
        messages[0]
            .get("MessageAttributes")
            .and_then(|attributes| attributes.get("kind"))
            .and_then(|attribute| attribute.get("StringValue"))
            .and_then(Value::as_str),
        Some("created"),
        "case {:?}: expected the attribute the integration set to reach the subscriber, got {}",
        "a publish through the assembled emulator",
        messages[0]
    );
}
