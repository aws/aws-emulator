// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares — or whose SAM transform generates — an
//! `AWS::Lambda::EventSourceMapping` really creates that mapping in AWS Lambda, and a
//! mapping Lambda refuses fails the stack.
//!
//! The rules under test:
//!
//! 1. **The `AWS::Serverless-2016-10-31` transform expands a function's pull event into an
//!    `AWS::Lambda::EventSourceMapping` that names the event's own source under
//!    `EventSourceArn` and names the function by the reference resolving to the identifier
//!    the mapping's property takes** — "Ref returns the function name" for an
//!    `AWS::Lambda::Function`, and `EventSourceMapping$FunctionName` is "the name or ARN of
//!    the Lambda function" — so the generated resource carries `{"Ref": <the function's
//!    logical id>}` rather than the function's ARN.
//! 2. **A resource of a type this emulator provisions is created in the service that OWNS
//!    it, through that service's own public API**, so the mapping the stack reports is one
//!    `lambda get-event-source-mapping` resolves under the physical id the stack published
//!    ("Ref returns the event source mapping's ID"), reading the source the template named
//!    and invoking the function the stack created, and a stack deletion takes it away again.
//! 3. **Whatever the owning service answers short of success fails the resource and the
//!    stack**: Lambda refuses a mapping whose function execution role may not make the
//!    poller's own reads of the named source with `InvalidParameterValueException`, so such a
//!    template is rolled back instead of reporting a trigger that could never fire.
//! 4. **An inline policy a template declares is embedded in every entity it names, in IAM
//!    itself** — "adds or updates an inline policy document that is embedded in the specified
//!    IAM role" — whether the declaration states it inside the role's own `Policies` or as an
//!    `AWS::IAM::Policy` resource of its own, so a role a stack granted the queue's reads to
//!    really holds that grant when Lambda asks IAM what it may do.
//!
//! Four crates have to agree for that to hold (CloudFormation expands the macro and makes
//! the call, Lambda serves it and judges the role, IAM answers for the role's policies, and
//! Amazon SQS and Amazon Kinesis hold the sources), so the suite lives here rather than in
//! any one crate.
//!
//! The rows of the first table differ along what rule 1 generalises over: the FORM the
//! event's source is stated in (a literal ARN, a `Ref` to a stack parameter, an `Fn::GetAtt`
//! of a resource the same template declares) and the KIND of pull event (an Amazon SQS queue
//! and an Amazon Kinesis data stream, which also carries a starting position). The rows of
//! the second table differ along WHICH of the poller's reads the role is granted on the
//! queue, one row per call the decision is made on plus the row that grants them all. The
//! rows of the third table differ along WHERE the template puts that grant, which is what
//! rule 4 generalises over.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the UUID AWS Lambda assigns the mapping, which is that
//! service's to mint: what is checked about it is its form, that Lambda resolves it, and that
//! the stack published the same one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, opt_element};

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls in
/// — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rules are stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// Where `CreateEventSourceMapping` and `ListEventSourceMappings` are bound
/// (`/2015-03-31/event-source-mappings`).
///
/// This constant and the three `State` values below, with [`is_mapping_uuid`],
/// [`mapping_until_settled`] and [`mappings_of_source`], are `pub(crate)` because more than
/// one suite of this crate reads a mapping a stack deployed: how a mapping is addressed, what
/// form its id takes and when it has settled are written once, here, so two suites cannot
/// disagree about them.
pub(crate) const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The `State` a mapping that is still being stood up reports, and the terminal ones it
/// settles into, from `EventSourceMappingConfiguration$State`: "It can be one of the
/// following: Creating, Enabling, Enabled, Disabling, Disabled, Updating, or Deleting."
pub(crate) const MAPPING_STATE_CREATING: &str = "Creating";
/// See [`MAPPING_STATE_CREATING`].
pub(crate) const MAPPING_STATE_ENABLED: &str = "Enabled";
/// See [`MAPPING_STATE_CREATING`]. The state a mapping whose declaration asked it not to poll
/// is created in: it has no poller to stand up, so it is terminal at once.
pub(crate) const MAPPING_STATE_DISABLED: &str = "Disabled";

/// How long a wait for a documented asynchronous transition goes on reading before this suite
/// calls it a hang, and how long it hands the runtime back between two reads.
///
/// The bound is a WALL-CLOCK deadline rather than a count of reads, because a count of reads
/// bounds the wait by how fast the host serves ONE read: the spans the services here choose,
/// and the function run behind a batch a mapping consumed, take the time they take whatever
/// the host is doing, so a fixed number of cheap reads can run out before the condition can
/// hold on a loaded machine while taking no longer on an idle one. What is waited for below is
/// the CONDITION; the deadline is generous slack over those spans, and all it is for is that
/// the wait TERMINATES.
const WAIT_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The emulator with the five services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
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
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation (CloudFormation, IAM) over its own wire.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
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
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "cloudformation",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "2010-05-15",
        action,
        members,
    )
    .await
}

/// One IAM call. IAM is global, so the request is signed for this suite's Region only
/// because that is what routes it.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    query(
        router,
        "iam",
        "iam.amazonaws.com",
        "2010-05-08",
        action,
        members,
    )
    .await
}

/// One awsJson1_0 RPC call to Amazon SQS.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", operation, body).await
}

/// One awsJson RPC call: `POST /` carrying `X-Amz-Target: <prefix>.<operation>`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One AWS Lambda call over its restJson1 wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// A trust policy that lets AWS Lambda assume the role, which is what an execution role has
/// to carry for CloudFormation to hand it to Lambda.
fn lambda_trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// The ARN a queue of `name` carries in this suite's Region.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}")
}

/// The ARN a data stream of `name` carries.
fn stream_arn(name: &str) -> String {
    format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/{name}")
}

/// The ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Whether `value` is the id AWS Lambda assigns an event source mapping: a UUID, which the
/// model pins to exactly 36 characters (`UUIDString`, `@length(min: 36, max: 36)`) in the
/// `8-4-4-4-12` hexadecimal grouping `EventSourceMappingArn`'s own pattern spells.
pub(crate) fn is_mapping_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12]) 
        && value
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit())
}

/// The `Resources` entry of a template body, read as the document it is.
fn declared_resource(case: &str, template: &str, logical_id: &str) -> Value {
    let document: Value = serde_json::from_str(template)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON template, got {error}"));
    document
        .get("Resources")
        .and_then(|resources| resources.get(logical_id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the template to declare {logical_id}, got {document}")
        })
}

/// The PROCESSED template body — the macro's output, which is what CloudFormation plans and
/// deploys.
async fn processed_template(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) = cloudformation(router, "GetTemplate", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetTemplate, got {status} with body {body}"
    );
    // Read through the decoding reader: the serializer escapes the body's own quotes, and
    // what the row asserts is the template document rather than its escaping.
    elements(&body, "TemplateBody")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no <TemplateBody> in {body}"))
}

/// The physical id the stack published for one of its logical resources.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", logical_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResource for {logical_id}, got \
         {status} with body {body}"
    );
    opt_element(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: no <PhysicalResourceId> for {logical_id} in {body}")
    })
}

/// Poll `GetEventSourceMapping` until Lambda reports the mapping out of `Creating`, and
/// answer the body it last reported.
///
/// The wait is on that CONDITION, looking again until it holds or [`WAIT_DEADLINE`] passes:
/// nothing here is bounded by how many reads were taken or by how long one took.
pub(crate) async fn mapping_until_settled(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
) -> Value {
    let uri = format!("{MAPPINGS_URI}/{uuid}");
    let giving_up_at = Instant::now() + WAIT_DEADLINE;
    loop {
        let (status, body) = lambda(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetEventSourceMapping to resolve the id the stack \
             published, got {status} with body {body}"
        );
        if body.get("State").and_then(Value::as_str) != Some(MAPPING_STATE_CREATING) {
            return body;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: the mapping never settled out of {MAPPING_STATE_CREATING} within \
             {WAIT_DEADLINE:?}, last saw {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `GetEventSourceMapping` until Lambda no longer holds the mapping, and answer the last
/// status it reported — the condition the caller asserts on, looked for again until it holds
/// or [`WAIT_DEADLINE`] passes.
async fn mapping_until_gone(router: &MultiServiceRouter, uuid: &str) -> u16 {
    let uri = format!("{MAPPINGS_URI}/{uuid}");
    let giving_up_at = Instant::now() + WAIT_DEADLINE;
    loop {
        let (status, _) = lambda(router, "GET", &uri, None).await;
        if status == 404 || Instant::now() >= giving_up_at {
            return status;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every mapping Lambda holds for one event source, through `ListEventSourceMappings`.
pub(crate) async fn mappings_of_source(
    router: &MultiServiceRouter,
    case: &str,
    source: &str,
) -> Vec<Value> {
    let uri = format!("{MAPPINGS_URI}?EventSourceArn={}", form_encode(source));
    let (status, body) = lambda(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListEventSourceMappings, got {status} with body \
         {body}"
    );
    body.get("EventSourceMappings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Send one message to a queue and poll until the queue holds none — which is what a mapping
/// that really CONSUMES does: "Lambda reads items from the event source and invokes the
/// function", and a batch the function processed is deleted from the queue.
///
/// Answers how many messages the queue reported last, so the caller names the case. The queue
/// is read again until it holds nothing or [`WAIT_DEADLINE`] passes — a span far longer than
/// the poll interval and the function run a consumed batch goes through, so a row fails only
/// when the mapping did not consume at all.
async fn consumed_from(router: &MultiServiceRouter, case: &str, queue_url: &str) -> String {
    let (status, sent) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": queue_url, "MessageBody": "one batch" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the message to be sent, got {status} with body {sent}"
    );
    let giving_up_at = Instant::now() + WAIT_DEADLINE;
    loop {
        let (status, attributes) = sqs(
            router,
            "GetQueueAttributes",
            &json!({
                "QueueUrl": queue_url,
                "AttributeNames": [
                    "ApproximateNumberOfMessages",
                    "ApproximateNumberOfMessagesNotVisible",
                ],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetQueueAttributes, got {status} with body \
             {attributes}"
        );
        let visible = attributes["Attributes"]["ApproximateNumberOfMessages"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let hidden = attributes["Attributes"]["ApproximateNumberOfMessagesNotVisible"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let last = format!("{visible} visible, {hidden} in flight");
        if (visible == "0" && hidden == "0") || Instant::now() >= giving_up_at {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// The expansion, and the mapping the stack really creates
// ---------------------------------------------------------------------------

/// One row of the expansion table.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The logical id of the serverless function, and the key of its `Events` entry: the
    /// generated mapping's logical id is the two of them joined.
    function: &'static str,
    event: &'static str,
    /// The template body.
    template: String,
    /// The stack parameters the row submits, as `(key, value)`.
    parameters: Vec<(String, String)>,
    /// A queue to create in Amazon SQS BEFORE the stack, for a row whose source was
    /// provisioned outside the template.
    queue_before: Option<&'static str>,
    /// The `Properties` the processed template must carry for the generated mapping.
    expected_properties: Value,
    /// The source ARN the mapping must really poll, once CloudFormation has resolved what the
    /// event stated.
    expected_source: String,
    /// The batch size the mapping must carry, as the event declared it.
    expected_batch_size: i64,
    /// The URL of the queue the mapping polls, for a row whose source is a queue: the row
    /// sends a message to it and the mapping has to consume it.
    consumes: Option<&'static str>,
}

/// A SAM template declaring one function with one pull event, plus whatever resources the row
/// needs beside it.
fn sam_template(resources: Value) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": resources,
    })
    .to_string()
}

/// The same, with a `Parameters` section.
fn parameterised_sam_template(parameters: Value, resources: Value) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Parameters": parameters,
        "Resources": resources,
    })
    .to_string()
}

/// One `AWS::Serverless::Function` with one pull event.
fn serverless_function(event: &str, event_type: &str, event_properties: Value) -> Value {
    json!({
        "Type": "AWS::Serverless::Function",
        "Properties": {
            "Handler": "index.handler",
            "Runtime": "python3.12",
            "InlineCode": "def handler(event, context):\n    return len(event['Records'])\n",
            "Events": {
                event: { "Type": event_type, "Properties": event_properties }
            }
        }
    })
}

fn rows() -> Vec<Row> {
    let literal = queue_arn("orders-intake");
    let parameterised = queue_arn("orders-param");
    vec![
        Row {
            case: "an SQS event naming a queue by a literal ARN",
            stack: "esm-literal-arn",
            function: "Pump",
            event: "Jobs",
            template: sam_template(json!({
                "Pump": serverless_function(
                    "Jobs",
                    "SQS",
                    json!({ "Queue": literal.clone(), "BatchSize": 10 }),
                )
            })),
            parameters: Vec::new(),
            queue_before: Some("orders-intake"),
            expected_properties: json!({
                "BatchSize": 10,
                "EventSourceArn": literal.clone(),
                "FunctionName": { "Ref": "Pump" },
            }),
            expected_source: literal,
            expected_batch_size: 10,
            consumes: Some("orders-intake"),
        },
        Row {
            case: "an SQS event naming a queue through a Ref to a stack parameter",
            stack: "esm-parameter-ref",
            function: "Fn",
            event: "Work",
            template: parameterised_sam_template(
                json!({ "QueueArn": { "Type": "String" } }),
                json!({
                    "Fn": serverless_function(
                        "Work",
                        "SQS",
                        json!({ "Queue": { "Ref": "QueueArn" }, "BatchSize": 1 }),
                    )
                }),
            ),
            parameters: vec![("QueueArn".to_string(), parameterised.clone())],
            queue_before: Some("orders-param"),
            expected_properties: json!({
                "BatchSize": 1,
                "EventSourceArn": { "Ref": "QueueArn" },
                "FunctionName": { "Ref": "Fn" },
            }),
            expected_source: parameterised,
            expected_batch_size: 1,
            consumes: Some("orders-param"),
        },
        Row {
            case: "an SQS event naming a queue the same template declares, by Fn::GetAtt",
            stack: "esm-getatt-queue",
            function: "Consume",
            event: "Inbox",
            template: sam_template(json!({
                "Inbound": {
                    "Type": "AWS::SQS::Queue",
                    "Properties": { "QueueName": "orders-declared" }
                },
                "Consume": serverless_function(
                    "Inbox",
                    "SQS",
                    json!({
                        "Queue": { "Fn::GetAtt": ["Inbound", "Arn"] },
                        "BatchSize": 5,
                    }),
                )
            })),
            parameters: Vec::new(),
            queue_before: None,
            expected_properties: json!({
                "BatchSize": 5,
                "EventSourceArn": { "Fn::GetAtt": ["Inbound", "Arn"] },
                "FunctionName": { "Ref": "Consume" },
            }),
            expected_source: queue_arn("orders-declared"),
            expected_batch_size: 5,
            consumes: Some("orders-declared"),
        },
        Row {
            case: "a Kinesis event over a stream the same template declares, with a starting \
                   position",
            stack: "esm-kinesis-stream",
            function: "Reader",
            event: "Records",
            template: sam_template(json!({
                "Events": {
                    "Type": "AWS::Kinesis::Stream",
                    "Properties": { "Name": "orders-events", "ShardCount": 1 }
                },
                "Reader": serverless_function(
                    "Records",
                    "Kinesis",
                    json!({
                        "Stream": { "Fn::GetAtt": ["Events", "Arn"] },
                        "BatchSize": 100,
                        "StartingPosition": "LATEST",
                    }),
                )
            })),
            parameters: Vec::new(),
            queue_before: None,
            expected_properties: json!({
                "BatchSize": 100,
                "EventSourceArn": { "Fn::GetAtt": ["Events", "Arn"] },
                "FunctionName": { "Ref": "Reader" },
                "StartingPosition": "LATEST",
            }),
            expected_source: stream_arn("orders-events"),
            expected_batch_size: 100,
            consumes: None,
        },
    ]
}

#[tokio::test]
async fn a_generated_event_source_mapping_is_created_in_lambda_and_names_its_function_by_ref() {
    for row in rows() {
        let router = build_router();
        let case = row.case;

        if let Some(queue) = row.queue_before {
            let (status, created) = sqs(&router, "CreateQueue", &json!({ "QueueName": queue })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the queue provisioned outside the template to be \
                 created, got {status} with body {created}"
            );
        }

        let mut members: Vec<(&str, &str)> = vec![
            ("StackName", row.stack),
            ("TemplateBody", &row.template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ];
        let parameter_members: Vec<(String, String)> = row
            .parameters
            .iter()
            .enumerate()
            .flat_map(|(index, (key, value))| {
                let position = index + 1;
                vec![
                    (
                        format!("Parameters.member.{position}.ParameterKey"),
                        key.clone(),
                    ),
                    (
                        format!("Parameters.member.{position}.ParameterValue"),
                        value.clone(),
                    ),
                ]
            })
            .collect();
        for (key, value) in &parameter_members {
            members.push((key.as_str(), value.as_str()));
        }
        let (status, body) = cloudformation(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (status, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to deploy the expanded template, got {described}"
        );

        // Rule 1: the generated mapping of the processed template.
        let mapping_id = format!("{}{}", row.function, row.event);
        let processed = processed_template(&router, case, row.stack).await;
        let generated = declared_resource(case, &processed, &mapping_id);
        assert_eq!(
            generated.get("Type").and_then(Value::as_str),
            Some("AWS::Lambda::EventSourceMapping"),
            "case {case:?}: expected the event to expand into a mapping, got {generated}"
        );
        assert_eq!(
            generated.get("Properties"),
            Some(&row.expected_properties),
            "case {case:?}: expected the generated mapping's properties to be {}, got {generated}",
            row.expected_properties
        );

        // Rule 2: the mapping AWS Lambda really holds, under the id the stack published.
        let uuid = physical_id(&router, case, row.stack, &mapping_id).await;
        assert!(
            is_mapping_uuid(&uuid),
            "case {case:?}: expected the physical id to be the UUID Lambda assigns a mapping, \
             got {uuid:?}"
        );
        let function_name = physical_id(&router, case, row.stack, row.function).await;
        let settled = mapping_until_settled(&router, case, &uuid).await;
        assert_eq!(
            settled.get("State").and_then(Value::as_str),
            Some(MAPPING_STATE_ENABLED),
            "case {case:?}: expected the mapping to settle into {MAPPING_STATE_ENABLED}, got \
             {settled}"
        );
        assert_eq!(
            settled.get("EventSourceArn").and_then(Value::as_str),
            Some(row.expected_source.as_str()),
            "case {case:?}: expected the mapping to poll {}, got {settled}",
            row.expected_source
        );
        assert_eq!(
            settled.get("FunctionArn").and_then(Value::as_str),
            Some(function_arn(&function_name).as_str()),
            "case {case:?}: expected the mapping to invoke the function the stack created, got \
             {settled}"
        );
        assert_eq!(
            settled.get("BatchSize").and_then(Value::as_i64),
            Some(row.expected_batch_size),
            "case {case:?}: expected the batch size the event declared, got {settled}"
        );
        let listed = mappings_of_source(&router, case, &row.expected_source).await;
        let ids: Vec<&str> = listed
            .iter()
            .filter_map(|mapping| mapping.get("UUID").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![uuid.as_str()],
            "case {case:?}: expected the source to be read by exactly the mapping the stack \
             created, got {listed:?}"
        );

        // A mapping on a QUEUE does not only exist: it CONSUMES. "Lambda reads items from the
        // event source and invokes the function", so a message put on the queue is taken off
        // it by the mapping the stack created, with nothing asking it to.
        if let Some(queue) = row.consumes {
            let (status, resolved) =
                sqs(&router, "GetQueueUrl", &json!({ "QueueName": queue })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the queue's URL, got {status} with body {resolved}"
            );
            let queue_url = resolved["QueueUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {resolved}"))
                .to_string();
            let held = consumed_from(&router, case, &queue_url).await;
            assert_eq!(
                held, "0 visible, 0 in flight",
                "case {case:?}: expected the mapping to consume the message, got {held}"
            );
        }

        // And deleting the stack takes the mapping away again.
        let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let gone = mapping_until_gone(&router, &uuid).await;
        assert_eq!(
            gone, 404,
            "case {case:?}: expected Lambda to hold no mapping once the stack deleted it, got \
             {gone}"
        );
    }
}

// ---------------------------------------------------------------------------
// The role gate: a mapping whose execution role cannot read the queue
// ---------------------------------------------------------------------------

/// One row of the role table: the reads the function's execution role is granted on the
/// queue, and whether the rule says the stack can then deploy the mapping.
struct RoleRow {
    case: &'static str,
    /// The actions the role's inline policy grants on the queue — the poller's own reads are
    /// `sqs:ReceiveMessage`, `sqs:DeleteMessage` and `sqs:GetQueueAttributes`, so a row short
    /// of one of them is a role the mapping cannot be created for.
    granted: &'static [&'static str],
    /// Whether the stack reaches `CREATE_COMPLETE` (so the mapping exists) or is rolled back.
    deployed: bool,
    /// The call Lambda's refusal must name for a row that is rolled back.
    withheld: Option<&'static str>,
}

/// The rows differ along WHICH of the poller's reads the role is granted: none of them, each
/// prefix of the three in the order the decision asks about them, and all three.
const ROLE_ROWS: &[RoleRow] = &[
    RoleRow {
        case: "a role granted nothing on the queue",
        granted: &[],
        deployed: false,
        withheld: Some("ReceiveMessage"),
    },
    RoleRow {
        case: "a role granted only the receive",
        granted: &["sqs:ReceiveMessage"],
        deployed: false,
        withheld: Some("DeleteMessage"),
    },
    RoleRow {
        case: "a role granted the receive and the delete but not the attribute read",
        granted: &["sqs:ReceiveMessage", "sqs:DeleteMessage"],
        deployed: false,
        withheld: Some("GetQueueAttributes"),
    },
    RoleRow {
        case: "a role granted every read the poller makes on the queue",
        granted: &[
            "sqs:ReceiveMessage",
            "sqs:DeleteMessage",
            "sqs:GetQueueAttributes",
        ],
        deployed: true,
        withheld: None,
    },
];

/// The sentence AWS Lambda refuses a mapping whose function execution role may not make one
/// of the poller's calls on the queue with — the wording that service answers this error
/// class with, naming the call and the service it would have been made to.
fn refusal_sentence(withheld: &str) -> String {
    format!("The function execution role does not have permissions to call {withheld} on SQS")
}

#[tokio::test]
async fn a_mapping_whose_execution_role_cannot_read_the_queue_rolls_the_stack_back() {
    for (index, row) in ROLE_ROWS.iter().enumerate() {
        let router = build_router();
        let case = row.case;
        let stack = format!("esm-role-{index}");
        let queue = format!("role-case-{index}");
        let role = format!("role-case-{index}-exec");
        let source = queue_arn(&queue);

        let (status, created) = sqs(&router, "CreateQueue", &json!({ "QueueName": queue })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the queue to be created, got {status} with body {created}"
        );

        let (status, body) = iam(
            &router,
            "CreateRole",
            &[
                ("RoleName", &role),
                ("AssumeRolePolicyDocument", &lambda_trust_policy()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the execution role to be created in IAM, got {status} with \
             body {body}"
        );
        if !row.granted.is_empty() {
            let document = json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": row.granted,
                    "Resource": source,
                }]
            })
            .to_string();
            let (status, body) = iam(
                &router,
                "PutRolePolicy",
                &[
                    ("RoleName", &role),
                    ("PolicyName", "queue-reads"),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the inline policy to be written, got {status} with body \
                 {body}"
            );
        }

        let template = sam_template(json!({
            "Pump": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Handler": "index.handler",
                    "Runtime": "python3.12",
                    "InlineCode": "def handler(event, context):\n    return 1\n",
                    "Role": format!("arn:aws:iam::{ACCOUNT}:role/{role}"),
                    "Events": {
                        "Jobs": {
                            "Type": "SQS",
                            "Properties": { "Queue": source, "BatchSize": 1 }
                        }
                    }
                }
            }
        }));
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let wanted = if row.deployed {
            CREATE_COMPLETE
        } else {
            ROLLBACK_COMPLETE
        };
        let (status, described) = wait_for_stack_status(&router, &stack, wanted).await;
        assert_eq!(
            status, wanted,
            "case {case:?}: expected the stack to reach {wanted}, got {described}"
        );

        let held = mappings_of_source(&router, case, &source).await;
        if row.deployed {
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the queue to be read by the mapping the stack created, \
                 got {held:?}"
            );
        } else {
            assert!(
                held.is_empty(),
                "case {case:?}: expected no mapping to be created for a role that cannot read \
                 the queue, got {held:?}"
            );
            let (status, events) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with \
                 body {events}"
            );
            let sentence = refusal_sentence(row.withheld.unwrap_or_default());
            let reasons = elements(&events, "ResourceStatusReason");
            assert!(
                reasons.iter().any(|reason| reason.contains(&sentence)),
                "case {case:?}: expected a resource failure carrying Lambda's refusal \
                 {sentence:?}, got {reasons:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Where the template puts the grant: the role's own Policies, or a policy resource
// ---------------------------------------------------------------------------

/// Where a template can put the inline policy that grants the queue's reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantPlace {
    /// Inside the role's own `Policies`.
    RolePolicies,
    /// As an `AWS::IAM::Policy` resource naming the role under `Roles`.
    PolicyResource,
    /// As an `AWS::IAM::RolePolicy` resource naming the role under `RoleName`.
    RolePolicyResource,
    /// As an AWS-managed policy of IAM's own catalog, attached to the role.
    AwsManagedPolicy,
    /// As an `AWS::IAM::ManagedPolicy` resource the template declares, attached to the role.
    ManagedPolicyResource,
}

/// One row of the declared-grant table: where the template states the inline policy that
/// grants the queue's reads, and whether the mapping can then be created.
struct GrantRow {
    case: &'static str,
    /// The name the inline policy is embedded under, which is what `GetRolePolicy` reads it
    /// back by — `None` for a row whose grant is a MANAGED policy, which is attached rather
    /// than embedded.
    policy_name: Option<&'static str>,
    /// Where the template puts it.
    declared_as: GrantPlace,
    /// The actions the grant carries.
    granted: &'static [&'static str],
    /// What the grant is granted ON: the queue's ARN where this is `None`, and the value
    /// itself where the row states one (`*`, every resource).
    on: Option<&'static str>,
    /// Whether the stack reaches `CREATE_COMPLETE` (so the mapping exists) or is rolled back.
    deployed: bool,
}

/// The poller's own reads of a queue, which a role polling one has to be granted. Source:
/// the actions AWS's own `AWSLambdaSQSQueueExecutionRole` grants for an SQS event source.
const QUEUE_READS: &[&str] = &[
    "sqs:ReceiveMessage",
    "sqs:DeleteMessage",
    "sqs:GetQueueAttributes",
];

/// The rows differ along WHERE the grant is declared — the five places a template can put one
/// — and along how widely it is written (the queue's own ARN and every resource, the named
/// reads and every action of the source service). The last row shows the gate still bites when
/// what is declared grants something else.
const GRANT_ROWS: &[GrantRow] = &[
    GrantRow {
        case: "a grant inside the role's own Policies",
        policy_name: Some("queue-reads-inline"),
        declared_as: GrantPlace::RolePolicies,
        granted: QUEUE_READS,
        on: None,
        deployed: true,
    },
    GrantRow {
        case: "a grant declared as an AWS::IAM::Policy resource of its own",
        policy_name: Some("queue-reads-resource"),
        declared_as: GrantPlace::PolicyResource,
        granted: QUEUE_READS,
        on: None,
        deployed: true,
    },
    GrantRow {
        case: "a grant declared as an AWS::IAM::RolePolicy resource of its own",
        policy_name: Some("queue-reads-role-policy"),
        declared_as: GrantPlace::RolePolicyResource,
        granted: QUEUE_READS,
        on: None,
        deployed: true,
    },
    GrantRow {
        case: "every action of the source service, on every resource",
        policy_name: Some("queue-wildcards"),
        declared_as: GrantPlace::RolePolicies,
        granted: &["sqs:*"],
        on: Some("*"),
        deployed: true,
    },
    GrantRow {
        case: "an AWS-managed policy of IAM's own catalog, attached to the role",
        policy_name: None,
        declared_as: GrantPlace::AwsManagedPolicy,
        granted: &[],
        on: None,
        deployed: true,
    },
    GrantRow {
        case: "a customer managed policy the template declares, attached to the role",
        policy_name: None,
        declared_as: GrantPlace::ManagedPolicyResource,
        granted: QUEUE_READS,
        on: None,
        deployed: true,
    },
    GrantRow {
        case: "a policy resource granting something else entirely",
        policy_name: Some("queue-sends"),
        declared_as: GrantPlace::PolicyResource,
        granted: &["sqs:SendMessage"],
        on: None,
        deployed: false,
    },
];

/// The AWS-managed policy one row attaches, as the ARN IAM holds it under. Source: the
/// catalog entry's own path and name (`AmazonSQSFullAccess`, which grants `sqs:*`).
const SQS_FULL_ACCESS_ARN: &str = "arn:aws:iam::aws:policy/AmazonSQSFullAccess";

/// The document one row grants, as the template states it.
fn grant_document(granted: &[&str], source: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": granted,
            "Resource": source,
        }]
    })
}

/// The template one grant row submits: the queue's reads granted where the row puts them, the
/// role the function runs as, and the function whose SQS event the transform expands.
fn grant_template(row: &GrantRow, source: &str) -> String {
    let document = grant_document(row.granted, row.on.unwrap_or(source));
    let policy_name = row.policy_name.unwrap_or_default();
    let mut role = json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            }
        }
    });
    let mut resources = json!({
        "Pump": {
            "Type": "AWS::Serverless::Function",
            "Properties": {
                "Handler": "index.handler",
                "Runtime": "python3.12",
                "InlineCode": "def handler(event, context):\n    return 1\n",
                "Role": { "Fn::GetAtt": ["PumpRole", "Arn"] },
                "Events": {
                    "Jobs": {
                        "Type": "SQS",
                        "Properties": { "Queue": source, "BatchSize": 1 }
                    }
                }
            }
        }
    });
    match row.declared_as {
        GrantPlace::PolicyResource => {
            resources["QueueAccess"] = json!({
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyName": policy_name,
                    "PolicyDocument": document,
                    "Roles": [{ "Ref": "PumpRole" }],
                }
            });
        }
        GrantPlace::RolePolicyResource => {
            resources["QueueAccess"] = json!({
                "Type": "AWS::IAM::RolePolicy",
                "Properties": {
                    "PolicyName": policy_name,
                    "PolicyDocument": document,
                    "RoleName": { "Ref": "PumpRole" },
                }
            });
        }
        GrantPlace::AwsManagedPolicy => {
            role["Properties"]["ManagedPolicyArns"] = json!([SQS_FULL_ACCESS_ARN]);
        }
        GrantPlace::ManagedPolicyResource => {
            resources["QueueAccessPolicy"] = json!({
                "Type": "AWS::IAM::ManagedPolicy",
                "Properties": { "PolicyDocument": document }
            });
            role["Properties"]["ManagedPolicyArns"] = json!([{ "Ref": "QueueAccessPolicy" }]);
        }
        GrantPlace::RolePolicies => {
            role["Properties"]["Policies"] = json!([{
                "PolicyName": policy_name,
                "PolicyDocument": document,
            }]);
        }
    }
    resources["PumpRole"] = role;
    sam_template(resources)
}

/// The document IAM holds for one inline policy of one role, read through `GetRolePolicy` —
/// the call a client reads an embedded policy back with. The STATUS comes back with it, so a
/// caller asserts what its own case expects of a policy IAM does or does not hold.
///
/// IAM reports a policy document URL-encoded compliant with RFC 3986, so the member is
/// percent-decoded back to plain JSON text here — what the documented contract tells a
/// caller to do before it reads the document.
async fn embedded_policy(
    router: &MultiServiceRouter,
    role: &str,
    policy_name: &str,
) -> (u16, String) {
    let (status, body) = iam(
        router,
        "GetRolePolicy",
        &[("RoleName", role), ("PolicyName", policy_name)],
    )
    .await;
    let document = elements(&body, "PolicyDocument")
        .into_iter()
        .next()
        .unwrap_or_default();
    (status, ::smithy_server_runtime::percent_decode(&document))
}

#[tokio::test]
async fn a_grant_the_template_declares_reaches_iam_wherever_it_is_stated() {
    for (index, row) in GRANT_ROWS.iter().enumerate() {
        let router = build_router();
        let case = row.case;
        let stack = format!("esm-grant-{index}");
        let queue = format!("grant-case-{index}");
        let source = queue_arn(&queue);

        let (status, created) = sqs(&router, "CreateQueue", &json!({ "QueueName": queue })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the queue to be created, got {status} with body {created}"
        );

        let template = grant_template(row, &source);
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let wanted = if row.deployed {
            CREATE_COMPLETE
        } else {
            ROLLBACK_COMPLETE
        };
        let (status, described) = wait_for_stack_status(&router, &stack, wanted).await;
        assert_eq!(
            status, wanted,
            "case {case:?}: expected the stack to reach {wanted}, got {described}"
        );

        let held = mappings_of_source(&router, case, &source).await;
        if !row.deployed {
            assert!(
                held.is_empty(),
                "case {case:?}: expected no mapping for a grant that does not cover the \
                 poller's reads, got {held:?}"
            );
            continue;
        }

        // The grant really reached IAM: the role the stack created carries the document the
        // template declared, under the name it stated, wherever it stated it. A grant the
        // template attached as a MANAGED policy is attached rather than embedded, so what is
        // read back for it is the attachment.
        let role = physical_id(&router, case, &stack, "PumpRole").await;
        match row.policy_name {
            Some(policy_name) => {
                let (status, document) = embedded_policy(&router, &role, policy_name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected IAM to hold the inline policy the template \
                     declared, got {status} with {document:?}"
                );
                for action in row.granted {
                    assert!(
                        document.contains(action),
                        "case {case:?}: expected the embedded document to grant {action:?}, got \
                         {document:?}"
                    );
                }
                let granted_on = row.on.unwrap_or(&source);
                assert!(
                    document.contains(granted_on),
                    "case {case:?}: expected the embedded document to be granted on \
                     {granted_on:?}, got {document:?}"
                );
            }
            None => {
                let (status, body) =
                    iam(&router, "ListAttachedRolePolicies", &[("RoleName", &role)]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from ListAttachedRolePolicies, got \
                     {status} with body {body}"
                );
                let attached = elements(&body, "PolicyArn");
                assert_eq!(
                    attached.len(),
                    1,
                    "case {case:?}: expected the one managed policy the template attached, got \
                     {attached:?}"
                );
            }
        }

        // And the mapping the grant made possible is the one Lambda holds for the queue.
        let uuid = physical_id(&router, case, &stack, "PumpJobs").await;
        let ids: Vec<&str> = held
            .iter()
            .filter_map(|mapping| mapping.get("UUID").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![uuid.as_str()],
            "case {case:?}: expected the queue to be read by the mapping the stack created, got \
             {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The role the transform GENERATES: what the function's own Policies put on it
// ---------------------------------------------------------------------------

/// One row of the generated-role table: what the function's `Policies` declares, and the
/// statements the role the transform generates for it must then carry in IAM.
struct GeneratedRoleRow {
    case: &'static str,
    /// The `Policies` the `AWS::Serverless::Function` declares, or `Value::Null` for a
    /// function that declares none — whose role carries only the managed policies the
    /// transform attaches.
    policies: fn(&str) -> Value,
    /// Text every embedded document of the role must carry between them, so a declaration
    /// whose grant never reached IAM fails here rather than at the mapping.
    expected_statements: &'static [&'static str],
}

/// The rows differ along the three things `AWS::Serverless::Function$Policies` accepts — "a
/// list of AWS SAM policy templates, IAM managed policy ARNs, or IAM policy documents" — plus
/// the function that declares none at all.
const GENERATED_ROLE_ROWS: &[GeneratedRoleRow] = &[
    GeneratedRoleRow {
        case: "a function that declares no policies of its own",
        policies: |_| Value::Null,
        expected_statements: &[],
    },
    GeneratedRoleRow {
        case: "a policy TEMPLATE naming the queue it polls",
        policies: |queue| json!([{ "SQSPollerPolicy": { "QueueName": queue } }]),
        expected_statements: &["sqs:ReceiveMessage", "sqs:DeleteMessage"],
    },
    GeneratedRoleRow {
        case: "a policy DOCUMENT stated inline",
        policies: |queue| {
            json!([{
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                    "Resource": queue_arn(queue),
                }]
            }])
        },
        expected_statements: &["sqs:GetQueueAttributes"],
    },
];

/// Every inline policy IAM holds for one role, as `(policy name, document)` — read through
/// `ListRolePolicies` and `GetRolePolicy`, the pair a client reads an entity's embedded
/// policies back with.
async fn embedded_policies(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
) -> Vec<(String, String)> {
    let (status, body) = iam(router, "ListRolePolicies", &[("RoleName", role)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListRolePolicies, got {status} with body {body}"
    );
    let mut held = Vec::new();
    for name in elements(&body, "member") {
        let (status, document) = embedded_policy(router, role, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRolePolicy for {name:?}, got {status}"
        );
        held.push((name, document));
    }
    held
}

#[tokio::test]
async fn the_role_the_transform_generates_carries_the_policies_the_function_declares() {
    for (index, row) in GENERATED_ROLE_ROWS.iter().enumerate() {
        let router = build_router();
        let case = row.case;
        let stack = format!("esm-generated-role-{index}");
        let queue = format!("generated-case-{index}");
        let source = queue_arn(&queue);

        let (status, created) = sqs(&router, "CreateQueue", &json!({ "QueueName": queue })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the queue to be created, got {status} with body {created}"
        );

        let mut properties = json!({
            "Handler": "index.handler",
            "Runtime": "python3.12",
            "InlineCode": "def handler(event, context):\n    return 1\n",
            "Events": {
                "Jobs": {
                    "Type": "SQS",
                    "Properties": { "Queue": source, "BatchSize": 1 }
                }
            }
        });
        let declared = (row.policies)(&queue);
        if !declared.is_null() {
            properties["Policies"] = declared;
        }
        let template = sam_template(json!({
            "Pump": { "Type": "AWS::Serverless::Function", "Properties": properties }
        }));
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        let (status, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to deploy, got {described}"
        );

        // The statements the declaration asked for are the ones IAM holds for the generated
        // role, so what the function may do is what the template declared.
        let role = physical_id(&router, case, &stack, "PumpRole").await;
        let held = embedded_policies(&router, case, &role).await;
        let documents = held
            .iter()
            .map(|(name, document)| format!("{name}: {document}"))
            .collect::<Vec<String>>()
            .join("\n");
        for statement in row.expected_statements {
            assert!(
                documents.contains(statement),
                "case {case:?}: expected the generated role to carry {statement:?}, got \
                 {documents:?}"
            );
        }

        // And the mapping the transform generated is the one Lambda holds for the queue.
        let uuid = physical_id(&router, case, &stack, "PumpJobs").await;
        let mappings = mappings_of_source(&router, case, &source).await;
        let ids: Vec<&str> = mappings
            .iter()
            .filter_map(|mapping| mapping.get("UUID").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![uuid.as_str()],
            "case {case:?}: expected the queue to be read by the mapping the stack created, got \
             {mappings:?}"
        );
    }
}
