// SPDX-License-Identifier: CC0-1.0

//! WHICH class a dead letter carries when the destination queue's own resource policy
//! withheld the delivery, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter` with
//! Amazon SQS on the same engine — so each row is read where a consumer of a
//! dead-letter queue reads it: off the message attributes of the dead letter the
//! target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here: EventBridge reports a failed delivery under the class of the
//! ANSWER the target's own service gave it, and that answer beside the class. A queue
//! target is reached with `sqs:SendMessage` as the service principal and no
//! role ("for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
//! relies on resource-based policies", `PutTargets`), and the answer that service gives a
//! sender it has not authorized discloses nothing: `SendMessage` models no authorization
//! failure at all
//! (`smithy_query(query="operation",
//! operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! and such a send is answered `QueueDoesNotExist` — HTTP 400 under the `awsQueryError`
//! code `AWS.SimpleQueueService.NonExistentQueue` — carrying the fixed sentence that
//! states the absence and the want of access without saying which holds. That is a
//! NOT-FOUND code, so the class read off it is the RESOURCE one (`NO_RESOURCE`), the same
//! class a queue ARN nothing was created for earns; what tells the two apart is the
//! SENTENCE, because that service words an absence it is sure of differently.
//!
//! The table spans the class rather than the one case the defect was found on: its rows
//! differ along the FORM the withholding takes (no document at all, a document with no
//! statement, an explicit `Deny` beside the grant, a grant scoped by `aws:SourceArn` to
//! another rule, a grant of another action, a grant to another service principal) and
//! along whether the queue was there at all. Every row installs a SECOND queue target
//! on the same rule that DOES grant the send, and asserts it received the event — so a
//! row cannot pass by failing every delivery, and the two kinds of answer are told
//! apart on one publication.
//!
//! The condition operators and keys the rows use are IAM's own, because a queue's
//! `Policy` is an IAM policy document
//! (`smithy_query(query="doc",shape="com.amazonaws.sqs#QueueAttributeName")`).
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and
//! CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, NO_RESOURCE, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, given_queue, given_queue_with_policy, given_rule,
    granting_events, is_request_id, not_disclosed_answer_head, publish, Emulator,
    QUEUE_ABSENT_SENTENCE, QUEUE_NOT_DISCLOSED_CODE, QUEUE_NOT_DISCLOSED_SENTENCE, REGION,
};

/// The rule every row below publishes through.
const RULE: &str = "classified-by-answer";

/// The ids the two queue targets of that rule carry.
const WITHHELD_TARGET_ID: &str = "withheld";
const GRANTED_TARGET_ID: &str = "granted";

/// The IAM action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of `action` to the service principal `principal` on `resource`, scoped by
/// `condition` when the row states one.
fn grant(principal: &str, action: &str, resource: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": {"Service": principal},
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A document carrying no statement at all, which grants nothing.
fn no_statements(_queue_arn: &str) -> String {
    document(json!([]))
}

/// A grant of the send with an explicit `Deny` of it beside the grant.
fn granting_then_denying(queue_arn: &str) -> String {
    document(json!([
        grant(SERVICE_PRINCIPAL, SEND_MESSAGE, queue_arn, None),
        {
            "Effect": "Deny",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
    ]))
}

/// A grant of the send scoped by `aws:SourceArn` to the deliveries of a rule other than
/// the one publishing here.
fn granting_another_rule(queue_arn: &str) -> String {
    let other = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule");
    document(json!([grant(
        SERVICE_PRINCIPAL,
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"ArnEquals": {"aws:SourceArn": other}})),
    )]))
}

/// A grant of another action of the same service on the queue.
fn granting_another_action(queue_arn: &str) -> String {
    document(json!([grant(
        SERVICE_PRINCIPAL,
        "sqs:ReceiveMessage",
        queue_arn,
        None
    )]))
}

/// A grant of the send to a different service principal.
fn granting_another_principal(queue_arn: &str) -> String {
    document(json!([grant(
        "s3.amazonaws.com",
        SEND_MESSAGE,
        queue_arn,
        None
    )]))
}

/// How a row brings the queue its withheld target names about.
enum Setup {
    /// A queue carrying the document the row's function builds from its own ARN, or none
    /// at all when the function is `None`.
    Queue(Option<fn(&str) -> String>),
    /// A queue ARN of this account and Region that no queue was ever created for.
    NeverCreated,
}

/// One row: the case, how its withheld target is brought about, the class the dead
/// letter is to carry and the sentence the answer beside it must state.
struct Row {
    case: &'static str,
    setup: Setup,
    class: &'static str,
    sentence: &'static str,
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The ARN the row's withheld target names, and the url of its queue when the row has
/// one.
async fn given_withheld_target(
    emulator: &Emulator,
    index: usize,
    setup: &Setup,
) -> (String, Option<String>) {
    match setup {
        Setup::Queue(policy) => {
            let policy: Option<&dyn Fn(&str) -> String> = policy
                .as_ref()
                .map(|written| written as &dyn Fn(&str) -> String);
            let (url, arn) =
                given_queue_with_policy(emulator, &format!("undisclosed-target-{index}"), policy)
                    .await;
            (arn, Some(url))
        }
        Setup::NeverCreated => (
            format!(
                "arn:aws:sqs:{REGION}:{}:never-created-{index}",
                engine::emulator_account_id()
            ),
            None,
        ),
    }
}

/// A delivery the destination queue's own documents withheld is dead-lettered under the
/// class of the answer that service gave — which it gives without disclosing the queue, so
/// the class is its not-found one — over every form the withholding takes; a queue that was
/// never there earns the same class under that service's own sentence for an absence.
#[tokio::test]
async fn a_queue_policy_refusal_is_dead_lettered_under_the_answer_that_service_gave() {
    let rows = [
        Row {
            case: "a queue carrying no policy at all",
            setup: Setup::Queue(None),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue whose document states no statement",
            setup: Setup::Queue(Some(no_statements)),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue whose policy denies the send beside granting it",
            setup: Setup::Queue(Some(granting_then_denying)),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue whose grant is scoped by aws:SourceArn to another rule",
            setup: Setup::Queue(Some(granting_another_rule)),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue granting this service another action",
            setup: Setup::Queue(Some(granting_another_action)),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue granting the send to another service principal",
            setup: Setup::Queue(Some(granting_another_principal)),
            class: NO_RESOURCE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
        },
        Row {
            case: "a queue ARN no queue was created for",
            setup: Setup::NeverCreated,
            class: NO_RESOURCE,
            sentence: QUEUE_ABSENT_SENTENCE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("undisclosed-dlq-{index}")).await;
        let (withheld_arn, withheld_url) =
            given_withheld_target(&emulator, index, &row.setup).await;
        // The sibling target of the same rule, whose own policy DOES grant the send:
        // one publication, two queues, and only the one that granted it receives the
        // event.
        let (granted_url, granted_arn) = given_queue_with_policy(
            &emulator,
            &format!("undisclosed-sibling-{index}"),
            Some(&granting_events),
        )
        .await;
        given_rule(
            &emulator,
            RULE,
            json!([
                {
                    "Id": WITHHELD_TARGET_ID,
                    "Arn": withheld_arn,
                    "DeadLetterConfig": { "Arn": dead_letter_arn },
                    "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
                },
                {
                    "Id": GRANTED_TARGET_ID,
                    "Arn": granted_arn,
                },
            ]),
        )
        .await;

        publish(&emulator).await;

        // The sibling that granted the send received the whole envelope: the rule
        // matched and the delivery path ran.
        let delivered = drain(&emulator, &granted_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the granting queue to receive the event, got {delivered:?}"
        );
        if let Some(url) = &withheld_url {
            let leaked = drain(&emulator, url).await;
            assert!(
                leaked.is_empty(),
                "case {case:?}: a queue that did not authorize the send receives nothing, got \
                 {leaked:?}"
            );
        }

        let parked = drain(&emulator, &dead_letter_url).await;
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {parked:?}"
        );
        let message = &parked[0];
        let class = attribute(message, ERROR_CODE_ATTRIBUTE);
        assert_eq!(
            class, row.class,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {:?}, got {class:?} \
             in {message:?}",
            row.class
        );
        // The answer beside the class is the one the refused send read, so a reader of
        // the dead letter is told both why the event was not delivered and what the
        // delivery was told.
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        for stated in [QUEUE_NOT_DISCLOSED_CODE, row.sentence] {
            assert!(
                reported.contains(stated),
                "case {case:?}: expected the answering service's own answer to state {stated:?}, \
                 got {reported:?}"
            );
        }
        if row.sentence == QUEUE_NOT_DISCLOSED_SENTENCE {
            // That answer discloses nothing of the queue it was asked about: not the
            // principal the call was made as, not the action, not the queue.
            let head = not_disclosed_answer_head();
            let request_id = answered_request_id(&reported, &head).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected {head:?} then the id of the refused call, got \
                     {reported:?}"
                )
            });
            assert!(
                is_request_id(&request_id),
                "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {request_id:?}"
            );
            for withheld in [withheld_arn.as_str(), SERVICE_PRINCIPAL, SEND_MESSAGE] {
                assert!(
                    !reported.contains(withheld),
                    "case {case:?}: that service discloses nothing of the refused send, so \
                     {withheld:?} must not appear in {reported:?}"
                );
            }
        }
        // The class names neither the rule nor the target, so the letter names both.
        assert_eq!(
            attribute(message, RULE_ARN_ATTRIBUTE),
            rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE),
            "case {case:?}: expected the dead letter to name the rule, got {message:?}"
        );
        assert_eq!(
            attribute(message, TARGET_ARN_ATTRIBUTE),
            withheld_arn,
            "case {case:?}: expected the dead letter to name the target, got {message:?}"
        );
    }
}

/// The answer follows the DOCUMENT in force at the instant of the delivery: the very
/// queue a withheld delivery was dead-lettered for receives the next event once its
/// policy grants the send, with nothing else changed — which is what that service's
/// answer declines to disclose, and what an operator reading the letter has to repair.
#[tokio::test]
async fn the_refusal_clears_when_the_queues_policy_grants_the_send() {
    let case = "a target queue granted the send after a withheld delivery";
    let emulator = emulator();
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "regranted-dlq").await;
    let (target_url, target_arn) =
        given_queue_with_policy(&emulator, "regranted-target", None).await;
    given_rule(
        &emulator,
        RULE,
        json!([{
            "Id": WITHHELD_TARGET_ID,
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        }]),
    )
    .await;

    publish(&emulator).await;

    let parked = drain(&emulator, &dead_letter_url).await;
    assert_eq!(
        parked.len(),
        1,
        "case {case:?}: expected one dead letter before the grant, got {parked:?}"
    );
    let class = attribute(&parked[0], ERROR_CODE_ATTRIBUTE);
    assert_eq!(
        class, NO_RESOURCE,
        "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {NO_RESOURCE:?}, got \
         {class:?} in {:?}",
        parked[0]
    );

    // Nothing but the queue's own document changes.
    let (status, body) = crate::target_dispatch::sqs(
        &emulator,
        "SetQueueAttributes",
        &json!({ "QueueUrl": target_url, "Attributes": { "Policy": granting_events(&target_arn) } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );

    publish(&emulator).await;

    let delivered = drain(&emulator, &target_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected the regranted queue to receive the next event, got {delivered:?}"
    );
    let parked = drain(&emulator, &dead_letter_url).await;
    assert!(
        parked.is_empty(),
        "case {case:?}: a delivery the policy now grants parks nothing, got {parked:?}"
    );
}
