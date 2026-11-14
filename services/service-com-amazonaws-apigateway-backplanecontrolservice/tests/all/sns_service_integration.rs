// SPDX-License-Identifier: CC0-1.0

//! What a deployed method whose `AWS` integration names an AMAZON SNS topic action answers,
//! end to end over the wire — and what the topic then delivers to the queue subscribed to it.
//!
//! RULE: an `AWS` integration "integrat[es] the API method request with an AWS service
//! action" (`IntegrationType`), named by its `uri`
//! (`arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}`, `Integration$uri`),
//! so a method whose `uri` names a topic action is served by MAKING that call against the
//! service that owns the topic, under the role `Integration$credentials` names, and the
//! method response carries what that service answered, mapped through the integration
//! response whose `selectionPattern` matches the status it answered with. Amazon SNS's api is
//! a QUERY api, so the action and its parameters travel in the integration request as that
//! wire form carries them — the `Action` and the other fields of an
//! `application/x-www-form-urlencoded` body, which the request template renders, the query
//! string the integration maps, and the `action/{Action}&{name}={value}` form of the `uri`
//! itself — and every field reaches the service as that request spelled it, so the MESSAGE
//! ATTRIBUTES an integration sets on a publish (`MessageAttributes.entry.{n}.…`) are the ones
//! the topic's subscribers are delivered. The role decides whether the call is made at all: a
//! role whose trust policy does not let `apigateway.amazonaws.com` assume it is an
//! integration that could not be performed (`API_CONFIGURATION_ERROR`, HTTP 500 and
//! `{"message":"Internal server error"}`), and a call the policies in force on the role do
//! not allow is refused with that service's own authorization error (HTTP 403, the
//! `AuthorizationError` code). Every request to the deployed stage is therefore answered, and
//! what it is answered with is computed from the topic the call reached.
//!
//! Nothing of the answer is invented: the document a client reads is the one that api answers
//! a query request with, the message a subscriber holds is the one the client sent, and a
//! refusal is worded as the service that refused it words one.
//!
//! The rows vary what the rule generalises over: the FORM the `uri` names the call in (the
//! `path` form, which addresses this api's root, and the `action` form, which names the
//! action), WHERE the action and its parameters come from (the rendered request template, the
//! mapped query string, the `uri`'s own pairs), the ACTION (a publish and a read of the
//! topic's attributes), the message BODY (plain text, text carrying every character that is
//! significant in a form body, text outside ASCII, a long body), the message ATTRIBUTES the
//! integration sets, and WHICH role the integration names (one that may publish, one whose
//! grant allows no publish, one this service cannot assume, and no role at all).

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{
    create_api, create_resource, gateway_with_sns_sqs_and_iam, string_member, Gateway,
};

/// The region every request of this suite is addressed in.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The api version every `awsQuery` request to Amazon SNS states: that service's model
/// version (`smithy_query(query="service_info")` of that service's model).
const SNS_VERSION: &str = "2010-03-31";

/// The topic the integrations publish to.
const TOPIC: &str = "stage-integration-orders";

/// The queue subscribed to that topic, which is where a delivered message is read.
const QUEUE: &str = "stage-integration-subscriber";

/// The media type the method requests below declare, which is the key their integration
/// request template is stored under.
const JSON: &str = "application/json";

/// The integration responses every method here declares: the default (no pattern) for a
/// success, and one per class of refusal, so what a client sees is the status the service
/// answered rather than one status for every outcome.
const MAPPED_STATUSES: &[(&str, Option<&str>)] = &[
    ("200", None),
    ("400", Some("400")),
    ("403", Some("403")),
    ("500", Some(r"5\d{2}")),
];

/// One answered invoke of a deployed stage: its status and its body verbatim.
struct Invoked {
    status: u16,
    body: String,
}

impl Invoked {
    /// The `message` member of a JSON body, which is what an API Gateway gateway response
    /// carries; `None` for a body that is not such an object.
    fn message(&self) -> Option<String> {
        serde_json::from_str::<Value>(&self.body)
            .ok()?
            .get("message")?
            .as_str()
            .map(str::to_owned)
    }
}

/// Issue one request to `api`'s own invoke endpoint carrying `body`, as a client of a public
/// method sends it: no credential at all, on the api's own host.
async fn invoke(gateway: &Gateway, api: &str, verb: &str, path: &str, body: &str) -> Invoked {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", JSON)
        .body(Bytes::from(body.to_owned()))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// One Amazon SNS `awsQuery` call on the same router that has to succeed, answering its body.
///
/// `fields` is the form body past the action and the version. Every value this suite names
/// there is an ARN or a name, which carry none of the characters a form body treats
/// specially, so nothing is encoded here.
async fn sns_ok(gateway: &Gateway, case: &str, action: &str, fields: &str) -> String {
    let host = format!("sns.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", &host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sns\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(format!(
            "Action={action}&Version={SNS_VERSION}&{fields}"
        )))
        .expect("a well-formed request");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    assert_eq!(
        status, 200,
        "setup {case:?}: expected HTTP 200 from Amazon SNS {action}, got {status} with \
         {body:?}"
    );
    body
}

/// One Amazon SQS call on the same router that has to succeed, as its parsed body.
async fn sqs_ok(gateway: &Gateway, case: &str, operation: &str, body: Value) -> Value {
    let host = format!("sqs.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", &host)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sqs\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    let parsed: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One IAM role API Gateway may assume (unless `trusted` is false, in which case another
/// service's principal is the one trusted), carrying `document` as its inline policy.
async fn given_role(gateway: &Gateway, name: &str, trusted: bool, document: Value) -> String {
    let principal = match trusted {
        true => "apigateway.amazonaws.com",
        false => "lambda.amazonaws.com",
    };
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    });
    let host = format!("iam.{REGION}.amazonaws.com");
    let send = |body: String| async {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", &host)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam\
                     /aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = gateway.send(request).await;
        response.status().as_u16()
    };
    let created = send(format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={name}&AssumeRolePolicyDocument={}",
        urlencoded(&trust.to_string())
    ))
    .await;
    assert_eq!(created, 200, "setup CreateRole({name}): got {created}");
    let attached = send(format!(
        "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}&PolicyName=calls\
         &PolicyDocument={}",
        urlencoded(&document.to_string())
    ))
    .await;
    assert_eq!(attached, 200, "setup PutRolePolicy({name}): got {attached}");
    format!(
        "arn:aws:iam::{}:role/{name}",
        ::engine::emulator_account_id()
    )
}

/// `value` percent-encoded for a form field, which is how a query-protocol request carries a
/// policy document — by the maintained coder for that encoding, as this target's sibling
/// suites do.
fn urlencoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The topic this suite publishes to, with a queue subscribed to it for RAW delivery —
/// "enables raw message delivery to Amazon SQS endpoints"
/// (`SubscribeInput$Attributes`/`RawMessageDelivery`), which is what makes the attributes a
/// publish carried the queue's OWN message attributes. Answers the topic's ARN and the
/// subscriber's url, both as the services that own them reported them.
async fn topic_with_subscriber(gateway: &Gateway) -> (String, String) {
    let created = sns_ok(gateway, "CreateTopic", "CreateTopic", &format!("Name={TOPIC}")).await;
    let topic_arn = xml_element(&created, "TopicArn")
        .unwrap_or_else(|| panic!("setup CreateTopic: no <TopicArn> in {created}"));
    let queue_url = string_member(
        &sqs_ok(
            gateway,
            "CreateQueue",
            "CreateQueue",
            json!({ "QueueName": QUEUE }),
        )
        .await,
        "QueueUrl",
        "CreateQueue",
    );
    let attributes = sqs_ok(
        gateway,
        "GetQueueAttributes",
        "GetQueueAttributes",
        json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = string_member(
        attributes.get("Attributes").unwrap_or(&Value::Null),
        "QueueArn",
        "GetQueueAttributes",
    );
    // The queue policy AWS documents for a queue subscribed to a topic: the service principal
    // `sns.amazonaws.com`, allowed `sqs:SendMessage` on that queue.
    sqs_ok(
        gateway,
        "SetQueueAttributes",
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
        gateway,
        "Subscribe",
        "Subscribe",
        &format!(
            "TopicArn={topic_arn}&Protocol=sqs&Endpoint={queue_arn}\
             &Attributes.entry.1.key=RawMessageDelivery&Attributes.entry.1.value=true"
        ),
    )
    .await;
    (topic_arn, queue_url)
}

/// Every message waiting on the queue at `url`, as its body and its message attributes, with
/// each one removed as it is read.
async fn drained(gateway: &Gateway, case: &str, url: &str) -> Vec<(String, Value)> {
    let received = sqs_ok(
        gateway,
        case,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
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
    let mut read = Vec::new();
    for message in &messages {
        read.push((
            message
                .get("Body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            message
                .get("MessageAttributes")
                .cloned()
                .unwrap_or(Value::Null),
        ));
        sqs_ok(
            gateway,
            case,
            "DeleteMessage",
            json!({
                "QueueUrl": url,
                "ReceiptHandle": string_member(message, "ReceiptHandle", case),
            }),
        )
        .await;
    }
    read
}

/// One method of the api whose integration is the `AWS` topic call `uri` names, under
/// `credentials` (none when it is `None`), with `template` as its integration request
/// template (none when it is `None`) and `parameters` as the values it maps onto the
/// integration request.
#[allow(clippy::too_many_arguments)]
async fn topic_method(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    uri: &str,
    credentials: Option<&str>,
    template: Option<&str>,
    parameters: &[(&str, String)],
) {
    put_method(
        gateway,
        api,
        resource,
        verb,
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    let mut integration = json!({
        "type": "AWS",
        // `integrationHttpMethod` travels under its `jsonName`, which is what
        // `smithy_query(query="operation",operation="PutIntegration")` reports for it. Every
        // call of a query api is a POST, whatever verb the METHOD carries.
        "httpMethod": "POST",
        "uri": uri,
    });
    if let Some(credentials) = credentials {
        integration["credentials"] = json!(credentials);
    }
    if let Some(template) = template {
        integration["requestTemplates"] = json!({ JSON: template });
    }
    if !parameters.is_empty() {
        integration["requestParameters"] = Value::Object(
            parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    put_integration(gateway, api, resource, verb, integration).await;
    for (status, pattern) in MAPPED_STATUSES {
        method_response(gateway, api, resource, verb, status, json!({})).await;
        let mut response = json!({ "statusCode": status });
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        integration_response(gateway, api, resource, verb, status, response).await;
    }
}

/// One row of the publish table: the route it is sent through, the message body it carries,
/// the body the subscriber must receive, and the message attribute the integration set.
struct Sent {
    case: &'static str,
    route: &'static str,
    body: String,
    delivered: String,
    attribute: Option<(&'static str, &'static str)>,
}

#[tokio::test]
async fn a_deployed_method_integrating_a_publish_makes_that_call() {
    let gateway = gateway_with_sns_sqs_and_iam();
    let (topic_arn, queue_url) = topic_with_subscriber(&gateway).await;
    // One role this api can assume that may act on this topic, written by IAM's own
    // `CreateRole`/`PutRolePolicy`.
    let allowed = given_role(
        &gateway,
        "apigateway-sns",
        true,
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["SNS:Publish", "SNS:GetTopicAttributes"],
                "Resource": topic_arn,
            }],
        }),
    )
    .await;
    let (api, root) = create_api(&gateway, "sns-integration").await;

    // The two forms of the `uri`: the ACTION form, which names the action, and the PATH form,
    // which addresses this api's root — the one path it publishes, every action of it being
    // reached there.
    let action_form = format!("arn:aws:apigateway:{REGION}:sns:action/Publish");
    let root_form = format!("arn:aws:apigateway:{REGION}:sns:path//");

    // A publish whose topic, message and message attribute all come from the RENDERED
    // request template, which is the form the integration request carries.
    let publish = string_member(
        &create_resource(&gateway, &api, &root, "publish").await,
        "id",
        "CreateResource(publish)",
    );
    topic_method(
        &gateway,
        &api,
        &publish,
        "POST",
        &action_form,
        Some(&allowed),
        Some(&format!(
            "TopicArn={topic_arn}&Message=$util.urlEncode($input.body)\
             &MessageAttributes.entry.1.Name=kind\
             &MessageAttributes.entry.1.Value.DataType=String\
             &MessageAttributes.entry.1.Value.StringValue=created"
        )),
        &[],
    )
    .await;

    // The same publish through the PATH form, whose request template names the action.
    let rooted = string_member(
        &create_resource(&gateway, &api, &root, "rooted").await,
        "id",
        "CreateResource(rooted)",
    );
    topic_method(
        &gateway,
        &api,
        &rooted,
        "POST",
        &root_form,
        Some(&allowed),
        Some(&format!(
            "Action=Publish&TopicArn={topic_arn}&Message=$util.urlEncode($input.body)\
             &MessageAttributes.entry.1.Name=origin\
             &MessageAttributes.entry.1.Value.DataType=String\
             &MessageAttributes.entry.1.Value.StringValue=rooted"
        )),
        &[],
    )
    .await;

    // A publish whose topic the `uri`'s OWN pairs fix, so the template names only the
    // message and its attribute.
    let fixed = string_member(
        &create_resource(&gateway, &api, &root, "fixed").await,
        "id",
        "CreateResource(fixed)",
    );
    topic_method(
        &gateway,
        &api,
        &fixed,
        "POST",
        &format!("{action_form}&TopicArn={topic_arn}"),
        Some(&allowed),
        Some(
            "Message=$util.urlEncode($input.body)\
             &MessageAttributes.entry.1.Name=kind\
             &MessageAttributes.entry.1.Value.DataType=String\
             &MessageAttributes.entry.1.Value.StringValue=fixed",
        ),
        &[],
    )
    .await;

    // A publish whose every parameter is MAPPED rather than rendered: no request template at
    // all, so the action, the topic and the message come from the query string the
    // integration maps.
    let mapped = string_member(
        &create_resource(&gateway, &api, &root, "mapped").await,
        "id",
        "CreateResource(mapped)",
    );
    topic_method(
        &gateway,
        &api,
        &mapped,
        "POST",
        &root_form,
        Some(&allowed),
        None,
        &[
            (
                "integration.request.querystring.Action",
                "'Publish'".to_string(),
            ),
            (
                "integration.request.querystring.TopicArn",
                format!("'{topic_arn}'"),
            ),
            (
                "integration.request.querystring.Message",
                "method.request.querystring.text".to_string(),
            ),
        ],
    )
    .await;

    // Another action of the same topic, to show the rule is about the call the `uri` names
    // rather than about one action of it: a READ of the topic's attributes.
    let attributes = string_member(
        &create_resource(&gateway, &api, &root, "attributes").await,
        "id",
        "CreateResource(attributes)",
    );
    topic_method(
        &gateway,
        &api,
        &attributes,
        "GET",
        &format!("arn:aws:apigateway:{REGION}:sns:action/GetTopicAttributes&TopicArn={topic_arn}"),
        Some(&allowed),
        None,
        &[],
    )
    .await;

    deploy(&gateway, &api, STAGE).await;

    // The publish rows: each is sent through the stage, then READ BACK on the topic's own
    // subscriber, so what is asserted is the message the queue holds rather than what the
    // stage said about it.
    let rows = vec![
        Sent {
            case: "plain text through the action form",
            route: "/publish",
            body: "the first order".to_string(),
            delivered: "the first order".to_string(),
            attribute: Some(("kind", "created")),
        },
        Sent {
            case: "text carrying every character significant in a form body",
            route: "/publish",
            body: "a&b=c+d%e".to_string(),
            delivered: "a&b=c+d%e".to_string(),
            attribute: Some(("kind", "created")),
        },
        Sent {
            case: "text outside ASCII",
            route: "/publish",
            body: "café — naïve".to_string(),
            delivered: "café — naïve".to_string(),
            attribute: Some(("kind", "created")),
        },
        Sent {
            case: "a long body, which one form field carries whole",
            route: "/publish",
            body: "x".repeat(4096),
            delivered: "x".repeat(4096),
            attribute: Some(("kind", "created")),
        },
        Sent {
            case: "the path form, which addresses this api's root",
            route: "/rooted",
            body: "sent through the root form".to_string(),
            delivered: "sent through the root form".to_string(),
            attribute: Some(("origin", "rooted")),
        },
        Sent {
            case: "the topic fixed by the uri's own pairs",
            route: "/fixed",
            body: "sent with the topic fixed in the uri".to_string(),
            delivered: "sent with the topic fixed in the uri".to_string(),
            attribute: Some(("kind", "fixed")),
        },
        Sent {
            case: "a publish whose parameters are mapped onto the query string",
            route: "/mapped?text=mapped+from+the+query+string",
            body: String::new(),
            delivered: "mapped from the query string".to_string(),
            attribute: None,
        },
    ];
    for row in &rows {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}{}", row.route),
            &row.body,
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected the topic's own 200, got {} with {:?}",
            row.case, answered.status, answered.body
        );
        assert!(
            answered.body.contains("<PublishResponse"),
            "case {:?}: expected the document this api answers a publish with, got {:?}",
            row.case,
            answered.body
        );
        assert!(
            xml_element(&answered.body, "MessageId").is_some_and(|id| !id.is_empty()),
            "case {:?}: expected a MessageId the topic minted, got {:?}",
            row.case,
            answered.body
        );
        let delivered = drained(&gateway, row.case, &queue_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {:?}: expected exactly the one message this row published, got {delivered:?}",
            row.case
        );
        let (body, attributes) = &delivered[0];
        assert_eq!(
            body, &row.delivered,
            "case {:?}: expected the body the client sent, got {body:?}",
            row.case
        );
        match row.attribute {
            Some((name, value)) => assert_eq!(
                attributes
                    .get(name)
                    .and_then(|attribute| attribute.get("StringValue"))
                    .and_then(Value::as_str),
                Some(value),
                "case {:?}: expected the attribute the integration set to reach the \
                 subscriber, got {attributes}",
                row.case
            ),
            None => assert!(
                attributes.as_object().is_none_or(serde_json::Map::is_empty),
                "case {:?}: expected no attribute, since the integration set none, got \
                 {attributes}",
                row.case
            ),
        }
    }

    // The ATTRIBUTE read through the stage, whose answer is the topic's own records.
    let read = invoke(&gateway, &api, "GET", &format!("/{STAGE}/attributes"), "").await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected the topic's own 200, got {} with {:?}",
        "the attribute read", read.status, read.body
    );
    assert!(
        read.body.contains("<GetTopicAttributesResponse") && read.body.contains(&topic_arn),
        "case {:?}: expected the topic's own attributes, got {:?}",
        "the attribute read",
        read.body
    );
}

#[tokio::test]
async fn the_integration_request_this_stage_makes_is_signed_for_the_emulators_own_verifier() {
    // The call the stage makes to the topic is an in-process request like any other, so the
    // emulator's own verifier checks its signature whenever request authentication is on —
    // which is how the assembled emulator runs. The rows above drive the router with that
    // check off, as every suite in this target does, so this one turns it ON for the invoke
    // itself: the setup calls are made first (they carry a credential scope rather than a
    // real signature), and only then is the verifier armed, so what the invoke proves is that
    // the signature this data plane puts on its integration request is the one that verifier
    // computes.
    let mut gateway = gateway_with_sns_sqs_and_iam();
    let (topic_arn, queue_url) = topic_with_subscriber(&gateway).await;
    let (api, root) = create_api(&gateway, "sns-signed-integration").await;
    let resource = string_member(
        &create_resource(&gateway, &api, &root, "publish").await,
        "id",
        "CreateResource(publish)",
    );
    topic_method(
        &gateway,
        &api,
        &resource,
        "POST",
        &format!("arn:aws:apigateway:{REGION}:sns:action/Publish"),
        None,
        Some(&format!(
            "TopicArn={topic_arn}&Message=$util.urlEncode($input.body)"
        )),
        &[],
    )
    .await;
    deploy(&gateway, &api, STAGE).await;

    gateway.router.set_request_authentication(true);
    let sent = "published under a verified signature";
    let answered = invoke(&gateway, &api, "POST", &format!("/{STAGE}/publish"), sent).await;
    assert_eq!(
        answered.status, 200,
        "case {:?}: expected the topic's own 200, got {} with {:?}",
        "an invoke served while the verifier is armed", answered.status, answered.body
    );
    gateway.router.set_request_authentication(false);
    let delivered = drained(&gateway, "an invoke served while the verifier is armed", &queue_url)
        .await;
    assert_eq!(
        delivered.iter().map(|(body, _)| body.as_str()).collect::<Vec<_>>(),
        vec![sent],
        "case {:?}: expected the published message on the topic's subscriber",
        "an invoke served while the verifier is armed"
    );
}

#[tokio::test]
async fn the_integration_role_decides_whether_the_publish_is_made() {
    let gateway = gateway_with_sns_sqs_and_iam();
    let (topic_arn, queue_url) = topic_with_subscriber(&gateway).await;
    let grant = |actions: Value| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": actions,
                "Resource": topic_arn,
            }],
        })
    };
    // Three roles: one that may publish, one this api can assume whose grant allows other
    // topic actions but NOT the publish, and one this api cannot assume at all.
    let allowed = given_role(&gateway, "role-allowed", true, grant(json!(["SNS:Publish"]))).await;
    let without_publish = given_role(
        &gateway,
        "role-without-publish",
        true,
        grant(json!(["SNS:GetTopicAttributes", "SNS:ListSubscriptionsByTopic"])),
    )
    .await;
    let untrusted =
        given_role(&gateway, "role-untrusted", false, grant(json!(["SNS:Publish"]))).await;

    let (api, root) = create_api(&gateway, "sns-integration-roles").await;
    let root_form = format!("arn:aws:apigateway:{REGION}:sns:path//");
    let template =
        format!("Action=Publish&TopicArn={topic_arn}&Message=$util.urlEncode($input.body)");
    for (part, credentials) in [
        ("allowed", Some(allowed.as_str())),
        ("withheld", Some(without_publish.as_str())),
        ("unassumable", Some(untrusted.as_str())),
        ("open", None),
    ] {
        let resource = string_member(
            &create_resource(&gateway, &api, &root, part).await,
            "id",
            part,
        );
        topic_method(
            &gateway,
            &api,
            &resource,
            "POST",
            &root_form,
            credentials,
            Some(&template),
            &[],
        )
        .await;
    }
    deploy(&gateway, &api, STAGE).await;

    // Rows: the route, the status the stage must answer, and whether the message reached the
    // topic's subscriber — which happens only where the call was made.
    let rows: &[(&str, &str, u16, bool)] = &[
        ("a role that may publish", "allowed", 200, true),
        ("a role whose grant allows no publish", "withheld", 403, false),
        ("a role this api cannot assume", "unassumable", 500, false),
        ("no role at all", "open", 200, true),
    ];
    for (case, part, expected, delivered) in rows {
        let answered = invoke(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/{part}"),
            case,
        )
        .await;
        assert_eq!(
            answered.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {} with {:?}",
            answered.status, answered.body
        );
        match *expected {
            200 => assert!(
                xml_element(&answered.body, "MessageId").is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected the topic's own MessageId, got {:?}",
                answered.body
            ),
            403 => {
                assert_eq!(
                    xml_element(&answered.body, "Code").as_deref(),
                    Some("AuthorizationError"),
                    "case {case:?}: expected this api's own authorization error, got {:?}",
                    answered.body
                );
                assert_eq!(
                    xml_element(&answered.body, "Type").as_deref(),
                    Some("Sender"),
                    "case {case:?}: expected the fault to be the caller's, got {:?}",
                    answered.body
                );
                let sentence = xml_element(&answered.body, "Message").unwrap_or_default();
                assert!(
                    sentence.contains(&format!(
                        "User: {without_publish} is not authorized to perform: SNS:Publish on \
                         resource: {topic_arn}"
                    )) && sentence
                        .contains("because no identity-based policy allows the SNS:Publish action"),
                    "case {case:?}: expected the sentence naming the role, the action and the \
                     topic, got {sentence:?}"
                );
            }
            _ => assert_eq!(
                answered.message().as_deref(),
                Some("Internal server error"),
                "case {case:?}: expected the api's own integration-failure message, got {:?}",
                answered.body
            ),
        }
        let received = drained(&gateway, case, &queue_url).await;
        let bodies: Vec<&str> = received.iter().map(|(body, _)| body.as_str()).collect();
        assert_eq!(
            bodies.contains(case),
            *delivered,
            "case {case:?}: expected delivered={delivered}, got {bodies:?}"
        );
    }
}
