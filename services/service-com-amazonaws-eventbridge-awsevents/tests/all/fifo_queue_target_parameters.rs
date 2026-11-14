// SPDX-License-Identifier: CC0-1.0

//! The parameters a target's own kind of resource can only be delivered to with, driven
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter` — so routing, deserialization, the
//! modelled-constraint validator, the handler and serialization are all exercised.
//!
//! The RULE asserted here: `PutTargets` refuses a request WHOLE when one of its targets
//! names a resource EventBridge could only deliver to with that resource's own target
//! parameters and the entry carries none — an Amazon SQS FIFO queue, whose every send
//! needs the message group `Target$SqsParameters` "contains the message group ID to use
//! when the target is an Amazon SQS fair or FIFO queue" carries, because "if you do not
//! provide a MessageGroupId when sending a message to a FIFO queue, the action fails"
//! (Amazon SQS `SendMessage$MessageGroupId`). The refusal is HTTP 400
//! `ValidationException`, it names the `Target$Id` of the entry it is about, and it leaves
//! the rule's stored target list exactly as it was.
//!
//! The rows span the class rather than the one case: they differ along whether the entry
//! carries `SqsParameters` at all, whether those parameters name a message group, the
//! VALUE of the group they name (a word, the empty value the member's `@length(min: 0)`
//! admits, and one of the 100 characters its maximum allows), the KIND of queue the ARN
//! names (standard, FIFO), the FORM of the queue name (the suffix at the end, the same
//! text in the middle, the same letters in another case, the suffix without its dot), the
//! Region and account the ARN names, the SERVICE the ARN names (a Kinesis data stream
//! whose name ends the same way needs no message group), and the `Id` the entry carries,
//! so the refusal's subject is read off the request rather than off a constant.
//!
//! The delivery a message group travels with once the target IS installed is
//! `tests/all/fifo_queue_target_delivery.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The event pattern every rule below carries.
const PATTERN: &str = r#"{"source":["com.demo.orders"]}"#;

/// The longest message group a target may name: `SqsParameters$MessageGroupId` is
/// modelled `@length(min: 0, max: 100)`, so a group of exactly that many characters is
/// still one the service takes.
const MAX_MESSAGE_GROUP_LENGTH: usize = 100;

/// The emulator: one router carrying only this service over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or_default()
}

/// The account every ARN below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of a queue of this account in the Region every request is signed for.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{name}", account())
}

/// The refusal a target whose resource needs parameters the request did not specify is
/// answered with, written out here rather than imported so a wrong sentence in the
/// implementation cannot make a row pass.
fn expected_refusal(target_id: &str) -> String {
    format!("Parameter(s) SqsParameters must be specified for target: {target_id}.")
}

/// A rule on the default bus carrying [`PATTERN`], asserted to be accepted.
async fn given_rule(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(
        router,
        "PutRule",
        &json!({ "Name": name, "EventPattern": PATTERN }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule {name} answered {status} {body}"
    );
}

/// The ids of the targets `rule` carries, in the order they are paged through.
async fn listed_target_ids(router: &MultiServiceRouter, case: &str, rule: &str) -> Vec<String> {
    let (status, body) = call(router, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTargetsByRule answered {status} {body}"
    );
    body["Targets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|target| target["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `SqsParameters` the stored target `id` of `rule` carries, as a read reports them.
async fn listed_sqs_parameters(
    router: &MultiServiceRouter,
    case: &str,
    rule: &str,
    id: &str,
) -> Value {
    let (status, body) = call(router, "ListTargetsByRule", &json!({ "Rule": rule })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTargetsByRule answered {status} {body}"
    );
    body["Targets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|target| target["Id"].as_str() == Some(id))
        .map(|target| target["SqsParameters"].clone())
        .unwrap_or(Value::Null)
}

/// One row of the table: the case, the `Id` and `Arn` the one entry carries, the
/// `SqsParameters` it carries (`None` for an entry carrying the member not at all), the
/// `RoleArn` it carries where its kind of target needs one, and whether the request is
/// refused.
struct Row {
    case: &'static str,
    id: &'static str,
    arn: String,
    sqs_parameters: Option<Value>,
    role_arn: Option<String>,
    refused: bool,
}

#[tokio::test]
async fn put_targets_requires_the_message_group_a_fifo_queue_target_is_delivered_with() {
    let router = emulator();
    let account = account();
    let longest_group = "g".repeat(MAX_MESSAGE_GROUP_LENGTH);
    let rows = [
        Row {
            case: "a FIFO queue target carrying no SqsParameters",
            id: "fifo-target",
            arn: queue_arn("orders.fifo"),
            sqs_parameters: None,
            role_arn: None,
            refused: true,
        },
        Row {
            // The member is there and names no group, so the parameters the delivery
            // needs are still unspecified.
            case: "a FIFO queue target whose SqsParameters name no message group",
            id: "ordered.target-2",
            arn: queue_arn("orders.fifo"),
            sqs_parameters: Some(json!({})),
            role_arn: None,
            refused: true,
        },
        Row {
            case: "a FIFO queue target naming a message group",
            id: "t3",
            arn: queue_arn("orders.fifo"),
            sqs_parameters: Some(json!({ "MessageGroupId": "orders-group" })),
            role_arn: None,
            refused: false,
        },
        Row {
            // `@length(min: 0)` admits a group of no characters, so a request that
            // specified one specified the parameter.
            case: "a FIFO queue target naming the shortest message group the model admits",
            id: "t4",
            arn: queue_arn("orders.fifo"),
            sqs_parameters: Some(json!({ "MessageGroupId": "" })),
            role_arn: None,
            refused: false,
        },
        Row {
            case: "a FIFO queue target naming the longest message group the model admits",
            id: "t5",
            arn: queue_arn("orders.fifo"),
            sqs_parameters: Some(json!({ "MessageGroupId": longest_group.clone() })),
            role_arn: None,
            refused: false,
        },
        Row {
            case: "a standard queue target carrying no SqsParameters",
            id: "t6",
            arn: queue_arn("orders"),
            sqs_parameters: None,
            role_arn: None,
            refused: false,
        },
        Row {
            // The suffix is what names a FIFO queue, so the same text inside the name is
            // a standard queue's name.
            case: "a queue whose name carries the suffix in the middle",
            id: "t7",
            arn: queue_arn("orders.fifo.backup"),
            sqs_parameters: None,
            role_arn: None,
            refused: false,
        },
        Row {
            // "Queue names are case-sensitive", so the suffix is the one Amazon SQS
            // requires as written and no other spelling of it.
            case: "a queue whose name ends in the suffix in another case",
            id: "t8",
            arn: queue_arn("orders.FIFO"),
            sqs_parameters: None,
            role_arn: None,
            refused: false,
        },
        Row {
            case: "a queue named the suffix without its dot",
            id: "t9",
            arn: queue_arn("fifo"),
            sqs_parameters: None,
            role_arn: None,
            refused: false,
        },
        Row {
            case: "a FIFO queue of another Region and account",
            id: "t10",
            arn: "arn:aws:sqs:us-west-2:111122223333:tenant-orders.fifo".to_string(),
            sqs_parameters: None,
            role_arn: None,
            refused: true,
        },
        Row {
            // The requirement is a property of the Amazon SQS queue class: a stream whose
            // name happens to end the same way is delivered to without a message group.
            case: "a Kinesis data stream whose name ends in the same suffix",
            id: "t11",
            arn: format!("arn:aws:kinesis:{REGION}:{account}:stream/telemetry.fifo"),
            sqs_parameters: None,
            role_arn: Some(format!("arn:aws:iam::{account}:role/events-invoke")),
            refused: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let rule = format!("fifo-parameters-{index}");
        given_rule(&router, case, &rule).await;
        let mut target = json!({ "Id": row.id, "Arn": row.arn });
        if let Some(parameters) = &row.sqs_parameters {
            target["SqsParameters"] = parameters.clone();
        }
        if let Some(role_arn) = &row.role_arn {
            target["RoleArn"] = json!(role_arn);
        }

        let (status, body) = call(
            &router,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": [target] }),
        )
        .await;

        if row.refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the request to be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual,
                expected_refusal(row.id),
                "case {case:?}: expected {:?}, got {actual:?}",
                expected_refusal(row.id)
            );
            let listed = listed_target_ids(&router, case, &rule).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused request installs no target, got {listed:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the target to be installed, got {status} {body}"
            );
            assert_eq!(
                body["FailedEntryCount"],
                json!(0),
                "case {case:?}: PutTargets reported a failed entry: {body}"
            );
            let listed = listed_target_ids(&router, case, &rule).await;
            assert_eq!(
                listed,
                vec![row.id.to_string()],
                "case {case:?}: expected the rule to carry only {:?}, got {listed:?}",
                row.id
            );
            // The parameters the entry carried are the ones the stored target reports:
            // what was accepted is what a reader gets back.
            let reported = listed_sqs_parameters(&router, case, &rule, row.id).await;
            let expected = row.sqs_parameters.clone().unwrap_or(Value::Null);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the stored SqsParameters {expected:?}, got {reported:?}"
            );
        }
    }
}

/// A request is accepted or refused as a WHOLE: one entry whose FIFO queue target names no
/// message group refuses every other entry of the same request, and leaves whatever the
/// rule already carried untouched.
#[tokio::test]
async fn one_fifo_entry_with_no_message_group_refuses_the_whole_request() {
    let router = emulator();
    let case = "a request carrying a sound entry and a FIFO entry naming no message group";
    let rule = "fifo-parameters-whole";
    given_rule(&router, case, rule).await;

    // What the rule already carries, so the refusal below is read against stored state
    // rather than against an empty rule.
    let (status, body) = call(
        &router,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [{ "Id": "already-there", "Arn": queue_arn("already-there") }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first PutTargets answered {status} {body}"
    );

    let (status, body) = call(
        &router,
        "PutTargets",
        &json!({
            "Rule": rule,
            "Targets": [
                { "Id": "sound", "Arn": queue_arn("sound") },
                { "Id": "first-offender", "Arn": queue_arn("one.fifo") },
                { "Id": "second-offender", "Arn": queue_arn("two.fifo") },
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the whole request to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    // Reported in the request's own order: the first entry that failed the requirement is
    // the one named.
    let actual = message(&body);
    assert_eq!(
        actual,
        expected_refusal("first-offender"),
        "case {case:?}: expected {:?}, got {actual:?}",
        expected_refusal("first-offender")
    );
    let listed = listed_target_ids(&router, case, rule).await;
    assert_eq!(
        listed,
        vec!["already-there".to_string()],
        "case {case:?}: a refused request changes no stored target, got {listed:?}"
    );
}

/// The same requirement on a CUSTOM event bus, where the rule is resolved in the bus the
/// request names: the refusal is the same sentence, and the rule of that bus reports no
/// target afterwards.
#[tokio::test]
async fn the_message_group_requirement_holds_on_a_custom_event_bus() {
    let router = emulator();
    let case = "a FIFO queue target on a rule of a custom event bus";
    let bus = "fifo-parameters-bus";
    let rule = "fifo-parameters-custom";
    let target_id = "fifo-target";

    let (status, body) = call(&router, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {body}"
    );
    let (status, body) = call(
        &router,
        "PutRule",
        &json!({ "Name": rule, "EventBusName": bus, "EventPattern": PATTERN }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status} {body}"
    );

    let (status, body) = call(
        &router,
        "PutTargets",
        &json!({
            "Rule": rule,
            "EventBusName": bus,
            "Targets": [{ "Id": target_id, "Arn": queue_arn("bus-orders.fifo") }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    let actual = message(&body);
    assert_eq!(
        actual,
        expected_refusal(target_id),
        "case {case:?}: expected {:?}, got {actual:?}",
        expected_refusal(target_id)
    );

    let (status, listed) = call(
        &router,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "EventBusName": bus }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTargetsByRule answered {status} {listed}"
    );
    let targets = listed["Targets"].as_array().cloned().unwrap_or_default();
    assert!(
        targets.is_empty(),
        "case {case:?}: a refused request installs no target, got {targets:?}"
    );

    // The same call naming the message group is taken, so the refusal above is about the
    // parameter it omitted and nothing else.
    let (status, body) = call(
        &router,
        "PutTargets",
        &json!({
            "Rule": rule,
            "EventBusName": bus,
            "Targets": [{
                "Id": target_id,
                "Arn": queue_arn("bus-orders.fifo"),
                "SqsParameters": { "MessageGroupId": "orders-group" },
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the target naming a group to be installed, got {status} {body}"
    );
    let (status, listed) = call(
        &router,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "EventBusName": bus }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTargetsByRule answered {status} {listed}"
    );
    let ids: Vec<String> = listed["Targets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|target| target["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![target_id.to_string()],
        "case {case:?}: expected the rule to carry {target_id:?}, got {ids:?}"
    );
}
