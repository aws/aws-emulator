// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SHAPE of what the completion of an asynchronous invocation
//! delivers: which function the invocation record names, and the bookkeeping a dead letter
//! carries beside the event whichever kind of target the configuration points at.
//!
//! Two rules are under test, each over a set of inputs rather than one case:
//!
//! * the `requestContext.functionArn` of an invocation record names the function QUALIFIED by
//!   the qualifier the event was addressed to — the configuration a record is written for is
//!   held per qualifier (`PutFunctionEventInvokeConfig$Qualifier`), so the record says which
//!   one it belongs to; an event addressed to no qualifier is the unpublished version's;
//! * a dead letter records the invocation's own request id and the failure the last attempt
//!   reported as MESSAGE ATTRIBUTES beside the event payload, each carrying the `DataType`
//!   its value is (`MessageAttributeValue$DataType`: the status of the failed invocation is a
//!   `Number`, the request id and the message are `String`), and it does so whether
//!   `DeadLetterConfig$TargetArn` names an Amazon SQS queue or an Amazon SNS topic — a topic
//!   relays them to its subscribers as their own attributes.
//!
//! Every row drives real HTTP requests through one router carrying Lambda, Amazon SQS and
//! Amazon SNS on ONE engine, so what is asserted is what a client reads: the message on the
//! queue the configuration names, or on the queue subscribed to the topic it names. The
//! function's package RUNS, so the failure a dead letter reports is the handler's own.
//!
//! Nothing here waits a span of its own: every wait looks again until what it waits for is
//! there or a generous deadline passes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::synchronous_invoke_run::package;

/// Where `CreateFunction`, `PublishVersion` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `PutFunctionEventInvokeConfig` is bound.
const EVENT_INVOKE_CONFIG_URI: &str = "/2019-09-25/functions";

/// The Region every call below is made in.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The service principal Amazon SNS delivers to a subscribed queue as, which that queue's own
/// policy has to grant `sqs:SendMessage` to.
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The `MaximumRetryAttempts` every configuration below states: none, the floor of the
/// member's own `@range(min: 0, max: 2)`. So each event has exactly ONE delivery attempt,
/// which is what makes `approximateInvokeCount` a known number.
const NO_RETRIES: i64 = 0;

/// The request header `Invoke$InvocationType` is bound to, and the value that queues the event
/// instead of waiting for the run.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";
/// See [`INVOCATION_TYPE_HEADER`].
const EVENT_TYPE: &str = "Event";

/// The unpublished version, as the model spells it: the qualifier an event addressed to none
/// belongs to.
const LATEST: &str = "$LATEST";

/// The `requestContext.condition` of a record for a handler that returned, and for an event
/// whose attempts were spent — the two conditions the rows reach.
const CONDITION_SUCCESS: &str = "Success";
/// See [`CONDITION_SUCCESS`].
const CONDITION_RETRIES_EXHAUSTED: &str = "RetriesExhausted";

/// The message attributes a dead letter carries, and the `DataType` of each.
const REQUEST_ID_ATTRIBUTE: &str = "RequestID";
/// See [`REQUEST_ID_ATTRIBUTE`].
const ERROR_CODE_ATTRIBUTE: &str = "ErrorCode";
/// See [`REQUEST_ID_ATTRIBUTE`].
const ERROR_MESSAGE_ATTRIBUTE: &str = "ErrorMessage";
/// See [`REQUEST_ID_ATTRIBUTE`].
const STRING_TYPE: &str = "String";
/// See [`REQUEST_ID_ATTRIBUTE`].
const NUMBER_TYPE: &str = "Number";

/// The status an invocation that executed answers, which is what a dead letter's `ErrorCode`
/// records.
const INVOKED_STATUS: &str = "200";

/// How long a row waits for work the CLOCK owes — the delivery attempt of an accepted event,
/// and the record or dead letter that follows it.
///
/// A ceiling on PATIENCE, never a span anything counts on: every wait looks again until what
/// it waits for is there or this deadline passes.
const BUDGET: Duration = Duration::from_secs(120);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The handler every function below runs: it RAISES when its event asks it to and returns
/// otherwise, so which outcome a completion reached is the handler's own doing, and the
/// message of the failure is its own too.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    if isinstance(event, dict) and event.get("fail"):
        raise RuntimeError("the handler refused " + str(event.get("mark")))
    return {"handled": event}
"#;

/// The message the handler above raises for the event `mark` marks, which is what the dead
/// letter's `ErrorMessage` must carry.
fn refusal_of(mark: &str) -> String {
    format!("the handler refused {mark}")
}

/// Lambda, Amazon SQS and Amazon SNS on ONE engine: a record is delivered through the seam
/// the invocation carries, so the queue and the topic it reaches are this emulator's own.
fn router_with_targets(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One Lambda control-plane or data-plane call, the way a client makes it.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the request body serializes")
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(
            builder
                .body(bytes::Bytes::from(payload))
                .expect("a well-formed request"),
        )
        .await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().clone(),
    }
}

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`).
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
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/sqs/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
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

/// One Amazon SNS call, in the `awsQuery` form that service's protocol binds.
async fn sns(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) -> (u16, String) {
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", "2010-03-31");
    for (name, value) in parameters {
        form.append_pair(name, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/sns/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text one XML element of an Amazon SNS answer carries.
fn element(document: &str, name: &str) -> Option<String> {
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let from = document.find(&opening)? + opening.len();
    let to = document[from..].find(&closing)? + from;
    Some(document[from..to].to_string())
}

/// A queue of this account: its url and its ARN, both as the queue itself reports them.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let (status, created) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue {name:?} answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let (status, attributes) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {attributes}"
    );
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {attributes}"))
        .to_string();
    (url, arn)
}

/// A topic of this account with `queue_arn` subscribed to it for RAW delivery — "this
/// eliminates the need for the endpoints to process JSON formatting"
/// (`RawMessageDelivery`), so the queue holds the published message and its attributes
/// themselves — and the queue's own `Policy` written to admit that topic's deliveries.
/// Answers the topic's ARN.
async fn given_topic_fanning_out(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_url: &str,
    queue_arn: &str,
) -> String {
    let (status, created) = sns(router, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic {name:?} answered {status} {created}"
    );
    let topic_arn = element(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"));
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowDeliveryFromTheTopic",
            "Effect": "Allow",
            "Principal": { "Service": SNS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string();
    let (status, written) = sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {written}"
    );
    let (status, subscribed) = sns(
        router,
        "Subscribe",
        &[
            ("TopicArn", topic_arn.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", queue_arn),
            ("ReturnSubscriptionArn", "true"),
            ("Attributes.entry.1.key", "RawMessageDelivery"),
            ("Attributes.entry.1.value", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: Subscribe answered {status} {subscribed}"
    );
    topic_arn
}

/// A function whose package RUNS [`HANDLER_SOURCE`], with the dead-letter target `dead_letter`
/// names when the row asks for one, brought to `Active`. Answers its ARN.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    dead_letter: Option<&str>,
) -> String {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": RUNTIME,
        "Handler": HANDLER,
        "Code": {
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HANDLER_SOURCE)),
        },
    });
    if let Some(target) = dead_letter {
        body["DeadLetterConfig"] = json!({ "TargetArn": target });
    }
    let created = lambda(router, "POST", FUNCTIONS_URI, &body, &[]).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateFunction {name:?} answered {} {}",
        created.status,
        created.text()
    );
    let arn = created.json()["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {}", created.text()))
        .to_string();
    await_active(router, case, name).await;
    arn
}

/// Wait until `name` reports `State` `Active`, which is the move the clock owes after a
/// create and what every invocation of it waits for.
async fn await_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
            &[],
        )
        .await;
        if read.json()["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} must become Active within {BUDGET:?}, got {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `PublishVersion` on `function`, answering the version it published.
async fn published_version(router: &MultiServiceRouter, case: &str, function: &str) -> String {
    let answer = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/versions"),
        &json!({}),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: PublishVersion answered {} {}",
        answer.status,
        answer.text()
    );
    answer.json()["Version"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Version in {}", answer.text()))
        .to_string()
}

/// `PutFunctionEventInvokeConfig` on the qualifier `qualifier` names (the unpublished version
/// when it names none), stating no retries and the destination slots the row asks for.
async fn given_event_invoke_config(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    qualifier: Option<&str>,
    on_success: Option<&str>,
    on_failure: Option<&str>,
) {
    let mut destinations = json!({});
    if let Some(destination) = on_success {
        destinations["OnSuccess"] = json!({ "Destination": destination });
    }
    if let Some(destination) = on_failure {
        destinations["OnFailure"] = json!({ "Destination": destination });
    }
    let uri = match qualifier {
        Some(qualifier) => format!(
            "{EVENT_INVOKE_CONFIG_URI}/{function}/event-invoke-config?Qualifier={qualifier}"
        ),
        None => format!("{EVENT_INVOKE_CONFIG_URI}/{function}/event-invoke-config"),
    };
    let answer = lambda(
        router,
        "PUT",
        &uri,
        &json!({
            "MaximumRetryAttempts": NO_RETRIES,
            "DestinationConfig": destinations,
        }),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: PutFunctionEventInvokeConfig answered {} {}",
        answer.status,
        answer.text()
    );
}

/// One asynchronous invocation of `function` at `qualifier`, which the accepting call answers
/// `202` for.
async fn invoke_async(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    qualifier: Option<&str>,
    event: &Value,
) {
    let uri = match qualifier {
        Some(qualifier) => {
            format!("{FUNCTIONS_URI}/{function}/invocations?Qualifier={qualifier}")
        }
        None => format!("{FUNCTIONS_URI}/{function}/invocations"),
    };
    let answer = lambda(
        router,
        "POST",
        &uri,
        event,
        &[(INVOCATION_TYPE_HEADER, EVENT_TYPE)],
    )
    .await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: an asynchronous Invoke answered {} {}",
        answer.status,
        answer.text()
    );
}

/// One message of the queue `url` addresses, with its own message attributes, once one has
/// arrived — or a failure saying nothing did.
async fn awaited_message(router: &MultiServiceRouter, case: &str, what: &str, url: &str) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, received) = sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "MessageAttributeNames": ["All"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReceiveMessage answered {status} {received}"
        );
        if let Some(message) = received["Messages"].as_array().and_then(|held| held.first()) {
            return message.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the {what} on {url} within {BUDGET:?}, got nothing"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Where a row's record or dead letter is delivered: straight to a queue, or to a topic that
/// relays it to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// An Amazon SQS queue, named by its own ARN.
    Queue,
    /// An Amazon SNS topic whose subscriber is that queue.
    Topic,
}

/// One row of the invocation-record table: what the event is addressed to, what the handler
/// does with it, and which kind of target receives the record.
struct RecordRow {
    case: &'static str,
    /// The resource the row drives, which carries targets of its own.
    name: &'static str,
    /// Whether the event is addressed to a PUBLISHED version rather than to no qualifier.
    published: bool,
    /// Whether the handler fails, which decides the slot the record is written to and the
    /// condition it reports.
    fails: bool,
    /// Where the record is delivered.
    target: Target,
}

/// The catalog: the qualifier the event is addressed to (none, a published version), the
/// outcome the event reached (a handler that returned, one that raised), and the kind of
/// target the record is delivered to (a queue, a topic that relays to one).
fn record_rows() -> Vec<RecordRow> {
    vec![
        RecordRow {
            case: "a success record on a queue, addressed to no qualifier",
            name: "record-latest-queue",
            published: false,
            fails: false,
            target: Target::Queue,
        },
        RecordRow {
            case: "a success record relayed by a topic, addressed to no qualifier",
            name: "record-latest-topic",
            published: false,
            fails: false,
            target: Target::Topic,
        },
        RecordRow {
            case: "a success record on a queue, addressed to a published version",
            name: "record-version-queue",
            published: true,
            fails: false,
            target: Target::Queue,
        },
        RecordRow {
            case: "a failure record on a queue, addressed to no qualifier",
            name: "record-latest-failure",
            published: false,
            fails: true,
            target: Target::Queue,
        },
    ]
}

/// RULE: the invocation record a destination receives names the function QUALIFIED by the
/// qualifier the event was addressed to, and reports the condition that ended the event.
#[tokio::test]
async fn an_invocation_record_names_the_qualifier_the_event_was_addressed_to() {
    for row in record_rows() {
        let case = row.case;
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_with_targets(&engine);

        let (url, queue_arn) = given_queue(&router, case, &format!("{}-sink", row.name)).await;
        let destination = match row.target {
            Target::Queue => queue_arn.clone(),
            Target::Topic => {
                given_topic_fanning_out(
                    &router,
                    case,
                    &format!("{}-topic", row.name),
                    &url,
                    &queue_arn,
                )
                .await
            }
        };
        let arn = given_function(&router, case, row.name, None).await;
        let (qualifier, expected_arn) = if row.published {
            let version = published_version(&router, case, row.name).await;
            (Some(version.clone()), format!("{arn}:{version}"))
        } else {
            (None, format!("{arn}:{LATEST}"))
        };
        let (on_success, on_failure) = if row.fails {
            (None, Some(destination.as_str()))
        } else {
            (Some(destination.as_str()), None)
        };
        given_event_invoke_config(
            &router,
            case,
            row.name,
            qualifier.as_deref(),
            on_success,
            on_failure,
        )
        .await;

        let event = json!({ "fail": row.fails, "mark": row.name });
        invoke_async(&router, case, row.name, qualifier.as_deref(), &event).await;

        let message = awaited_message(&router, case, "invocation record", &url).await;
        let body = message["Body"].as_str().unwrap_or_default();
        let record: Value = serde_json::from_str(body)
            .unwrap_or_else(|_| panic!("case {case:?}: the record is no JSON document: {body}"));
        let answered = record["requestContext"]["functionArn"].as_str();
        assert_eq!(
            answered,
            Some(expected_arn.as_str()),
            "case {case:?}: expected the record to name {expected_arn}, got {answered:?} in \
             {record}"
        );
        let expected_condition = if row.fails {
            CONDITION_RETRIES_EXHAUSTED
        } else {
            CONDITION_SUCCESS
        };
        let answered = record["requestContext"]["condition"].as_str();
        assert_eq!(
            answered,
            Some(expected_condition),
            "case {case:?}: expected the condition {expected_condition}, got {answered:?} in \
             {record}"
        );
        let answered = record["requestContext"]["approximateInvokeCount"].as_i64();
        assert_eq!(
            answered,
            Some(1),
            "case {case:?}: expected the one attempt the configuration allows to be reported, \
             got {answered:?} in {record}"
        );
    }
}

/// RULE: a dead letter carries the invocation's own request id and the failure the last
/// attempt reported as message attributes beside the event payload, each with the `DataType`
/// its value is — whichever kind of target `DeadLetterConfig$TargetArn` names.
#[tokio::test]
async fn a_dead_letter_records_its_bookkeeping_on_either_kind_of_target() {
    // (case, the resource names, where the dead letter goes)
    let rows: &[(&str, &str, Target)] = &[
        ("a dead-letter queue", "dead-letter-queue", Target::Queue),
        ("a dead-letter topic", "dead-letter-topic", Target::Topic),
    ];
    for (case, name, target) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_with_targets(&engine);

        // The dead letter's own target, and the failure destination beside it: the record the
        // destination receives is what says which request the dead letter belongs to.
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&router, case, &format!("{name}-dlq")).await;
        let dead_letter_target = match target {
            Target::Queue => dead_letter_arn.clone(),
            Target::Topic => {
                given_topic_fanning_out(
                    &router,
                    case,
                    &format!("{name}-topic"),
                    &dead_letter_url,
                    &dead_letter_arn,
                )
                .await
            }
        };
        let (record_url, record_arn) =
            given_queue(&router, case, &format!("{name}-records")).await;
        given_function(&router, case, name, Some(&dead_letter_target)).await;
        given_event_invoke_config(&router, case, name, None, None, Some(&record_arn)).await;

        let payload = json!({ "fail": true, "mark": name });
        invoke_async(&router, case, name, None, &payload).await;

        let record = awaited_message(&router, case, "invocation record", &record_url).await;
        let document: Value = serde_json::from_str(record["Body"].as_str().unwrap_or_default())
            .unwrap_or_else(|_| panic!("case {case:?}: the record is no JSON document"));
        let request_id = document["requestContext"]["requestId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !request_id.is_empty(),
            "case {case:?}: the record must name the request the event was accepted as, got \
             {document}"
        );

        let letter = awaited_message(&router, case, "dead-lettered event", &dead_letter_url).await;
        // The body is the event payload EXACTLY as the caller supplied it, which is what makes
        // the dropped event replayable.
        let answered = letter["Body"].as_str().unwrap_or_default();
        assert_eq!(
            answered,
            payload.to_string(),
            "case {case:?}: expected the dead letter to carry the event itself, got {answered:?}"
        );
        let expected = json!({
            REQUEST_ID_ATTRIBUTE: { "DataType": STRING_TYPE, "StringValue": request_id },
            ERROR_CODE_ATTRIBUTE: { "DataType": NUMBER_TYPE, "StringValue": INVOKED_STATUS },
            ERROR_MESSAGE_ATTRIBUTE: {
                "DataType": STRING_TYPE,
                "StringValue": refusal_of(name),
            },
        });
        let answered = &letter["MessageAttributes"];
        assert_eq!(
            answered, &expected,
            "case {case:?}: expected the dead letter to record {expected}, got {answered}"
        );
    }
}
