// SPDX-License-Identifier: CC0-1.0

//! A dead letter's `ERROR_CODE` and `ERROR_MESSAGE` come from ONE answer, end to end
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter` with Amazon SQS on the same engine — so the
//! pair is read where a client reads it: off the message attributes of the dead letter
//! the target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here: a dead letter's two attributes come from one failed
//! delivery — the `ERROR_MESSAGE` is the refusal the target's own service answered, in
//! full, and the `ERROR_CODE` is the class that answer's own error CODE belongs to.
//! For a queue target the send is authorized by the queue's own resource policy
//! alone ("for Lambda functions,
//! Amazon SNS topics and Amazon SQS queues, EventBridge relies on resource-based
//! policies", `PutTargets`), and Amazon SQS models no authorization failure on
//! `SendMessage` at all (`smithy_query(query="operation", operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! because it does not disclose a queue it will not let the caller write to: both a
//! queue whose documents withheld the send and a queue that is not THERE are answered
//! `QueueDoesNotExist`, HTTP 400 under `AWS.SimpleQueueService.NonExistentQueue`
//! (`smithy_query(query="shape", shape="QueueDoesNotExist",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`),
//! each under its own sentence. ONE code, so ONE class — the RESOURCE one that code
//! belongs to — while the SENTENCE inside the answer is what tells the two situations
//! apart, and neither attribute is read off the other.
//!
//! The table spans that rule rather than one case: its rows differ along the FORM a
//! refusal takes (no document at all, an explicit `Deny`, a grant scoped by
//! `aws:SourceArn` to another rule, one scoped by `aws:SourceAccount` to another
//! account, a grant of another action, a grant to another service principal) and
//! along the FORM an absence takes (an ARN no queue was created for, a queue deleted
//! after the target was written). Each row states the code, the sentence and the class
//! it expects, so a row whose attributes disagreed would fail whichever of them was
//! wrong.
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
    is_request_id, not_disclosed_answer_head, publish, sqs, Emulator, QUEUE_ABSENT_SENTENCE,
    QUEUE_NOT_DISCLOSED_CODE, QUEUE_NOT_DISCLOSED_SENTENCE, REGION,
};

/// The rule every row below publishes through.
const RULE: &str = "paired-refusal";

/// The id the one target of that rule carries.
const TARGET_ID: &str = "queue";

/// The IAM action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";

/// The error code a queue target's service answers when the key an ENCRYPTED queue's
/// messages are encrypted under withholds the data key — the one refusal of a send
/// that service really does answer as an authorization failure, which it relays under
/// its own code (`KmsAccessDenied`'s `awsQueryError` code,
/// `smithy_query(query="shape", shape="KmsAccessDenied",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
/// No row here encrypts its queue, so no row may read it.
const KEY_ACCESS_DENIED: &str = "KMS.AccessDeniedException";

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of `action` to the service principal `principal` on `resource`, scoped
/// by `condition` when the row states one.
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

/// A document that denies the send outright, beside a grant of it.
fn denying(queue_arn: &str) -> String {
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

/// A grant scoped by `aws:SourceArn` to the deliveries of a rule other than the one
/// publishing here.
fn granting_another_rule(queue_arn: &str) -> String {
    let other = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule");
    document(json!([grant(
        SERVICE_PRINCIPAL,
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"ArnEquals": {"aws:SourceArn": other}})),
    )]))
}

/// A grant scoped by `aws:SourceAccount` to another account's rules.
fn granting_another_account(queue_arn: &str) -> String {
    document(json!([grant(
        SERVICE_PRINCIPAL,
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"StringEquals": {"aws:SourceAccount": "111122223333"}})),
    )]))
}

/// A grant of another action of the same service, so no statement is about the call
/// the delivery makes.
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

/// How a row brings the queue its target names about.
enum Setup {
    /// A queue carrying the document the row's function builds from its own ARN, or
    /// none at all when the function is `None`.
    Queue(Option<fn(&str) -> String>),
    /// A queue ARN of this account and Region that no queue was ever created for.
    NeverCreated,
    /// A queue created, named by the target, and then deleted.
    Deleted,
}

/// One row: the case, how its target queue is brought about, and the answer that
/// state earns — the error code the target's service gives, the sentence it words that
/// answer with, and the class this service puts that code in.
struct Row {
    case: &'static str,
    setup: Setup,
    code: &'static str,
    sentence: &'static str,
    class: &'static str,
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The ARN the row's target names, and the url of its queue when the row has one.
async fn given_target(
    emulator: &Emulator,
    case: &str,
    index: usize,
    setup: &Setup,
) -> (String, Option<String>) {
    let name = format!("paired-target-{index}");
    match setup {
        Setup::Queue(policy) => {
            let policy: Option<&dyn Fn(&str) -> String> = policy
                .as_ref()
                .map(|written| written as &dyn Fn(&str) -> String);
            let (url, arn) = given_queue_with_policy(emulator, &name, policy).await;
            (arn, Some(url))
        }
        Setup::NeverCreated => (
            format!("arn:aws:sqs:{REGION}:{}:never-created-{index}", account()),
            None,
        ),
        Setup::Deleted => {
            let (url, arn) = given_queue(emulator, &name).await;
            let (status, body) = sqs(emulator, "DeleteQueue", &json!({ "QueueUrl": url })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteQueue answered {status} {body}"
            );
            (arn, None)
        }
    }
}

/// Every form a failed delivery to a queue target takes reports the answer that
/// service gave, with the class of that answer's own code beside it.
#[tokio::test]
async fn a_dead_letters_class_and_message_come_from_one_answer() {
    let rows = [
        Row {
            case: "a queue carrying no policy at all",
            setup: Setup::Queue(None),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue whose policy denies the send beside granting it",
            setup: Setup::Queue(Some(denying)),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue whose grant is scoped by aws:SourceArn to another rule",
            setup: Setup::Queue(Some(granting_another_rule)),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue whose grant is scoped by aws:SourceAccount to another account",
            setup: Setup::Queue(Some(granting_another_account)),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue granting this service another action",
            setup: Setup::Queue(Some(granting_another_action)),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue granting the send to another service principal",
            setup: Setup::Queue(Some(granting_another_principal)),
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_NOT_DISCLOSED_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue ARN no queue was created for",
            setup: Setup::NeverCreated,
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_ABSENT_SENTENCE,
            class: NO_RESOURCE,
        },
        Row {
            case: "a queue deleted after the target was written",
            setup: Setup::Deleted,
            code: QUEUE_NOT_DISCLOSED_CODE,
            sentence: QUEUE_ABSENT_SENTENCE,
            class: NO_RESOURCE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("paired-dlq-{index}")).await;
        let (target_arn, target_url) = given_target(&emulator, case, index, &row.setup).await;
        given_rule(
            &emulator,
            RULE,
            json!([{
                "Id": TARGET_ID,
                "Arn": target_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
                "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
            }]),
        )
        .await;

        publish(&emulator).await;

        if let Some(url) = &target_url {
            let delivered = drain(&emulator, url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a failed delivery leaves the target queue empty, got {delivered:?}"
            );
        }
        let parked = drain(&emulator, &dead_letter_url).await;
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {parked:?}"
        );
        let message = &parked[0];
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        // The answer is the target service's own: the code that service answered
        // under and the sentence it worded for what it answered, as the delivery read
        // them.
        for stated in [row.code, row.sentence] {
            assert!(
                reported.contains(stated),
                "case {case:?}: expected the answer that service gave to state {stated:?}, got \
                 {reported:?}"
            );
        }
        // The class is the one this service's single classifier puts that answer's own
        // code in, so a withheld send and a queue that is not there \u2014 one code, two
        // sentences \u2014 come back under the same class and are told apart by the sentence.
        let class = attribute(message, ERROR_CODE_ATTRIBUTE);
        assert!(
            !reported.contains(KEY_ACCESS_DENIED),
            "case {case:?}: no row here encrypts its queue, so {KEY_ACCESS_DENIED:?} must not \
             appear in {reported:?}"
        );
        assert_eq!(
            class,
            row.class,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {:?}, got {class:?} \
             in {message:?}",
            row.class
        );
        // The class says nothing about which delivery failed, so the letter names the
        // rule and the target beside it.
        assert_eq!(
            attribute(message, RULE_ARN_ATTRIBUTE),
            rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE),
            "case {case:?}: expected the dead letter to name the rule, got {message:?}"
        );
        assert_eq!(
            attribute(message, TARGET_ARN_ATTRIBUTE),
            target_arn,
            "case {case:?}: expected the dead letter to name the target, got {message:?}"
        );
    }
}

/// The answer a withheld send earns states the ONE call it refused: a second delivery
/// to the same queue reports the same refusal with a fresh id for its own refused
/// call, and the rest of it is the fixed text that service words for a queue it will
/// not disclose — so it names neither the queue, nor the principal, nor the action
/// that call asked for, and it is not the sentence that service keeps for a queue
/// which is simply gone.
#[tokio::test]
async fn the_withheld_send_answer_states_one_call_and_discloses_nothing_of_it() {
    let case = "a queue granting this service nothing";
    let emulator = emulator();
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "named-dlq").await;
    let (target_url, target_arn) =
        given_queue_with_policy(&emulator, "named-target", None).await;
    given_rule(
        &emulator,
        RULE,
        json!([{
            "Id": TARGET_ID,
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        }]),
    )
    .await;

    publish(&emulator).await;
    publish(&emulator).await;

    let parked = drain(&emulator, &dead_letter_url).await;
    assert_eq!(
        parked.len(),
        2,
        "case {case:?}: expected one dead letter per refused delivery, got {parked:?}"
    );
    let head = not_disclosed_answer_head();
    let mut ids = Vec::new();
    for message in &parked {
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        let id = answered_request_id(&reported, &head).unwrap_or_else(|| {
            panic!("case {case:?}: expected {head:?} then the id of the refused call, got {reported:?}")
        });
        assert!(
            is_request_id(&id),
            "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {id:?}"
        );
        // That answer discloses nothing of the call it was asked about, and is not
        // the sentence a queue that is simply gone earns.
        for withheld in [target_arn.as_str(), SERVICE_PRINCIPAL, SEND_MESSAGE] {
            assert!(
                !reported.contains(withheld),
                "case {case:?}: expected the refused send's {withheld:?} NOT to be disclosed, got \
                 {reported:?}"
            );
        }
        assert!(
            !reported.contains(QUEUE_ABSENT_SENTENCE),
            "case {case:?}: a queue that is there does not read the absence sentence, got \
             {reported:?}"
        );
        ids.push(id);
    }
    assert_ne!(
        ids[0], ids[1],
        "case {case:?}: expected each refused call its own id, got {:?} twice",
        ids[0]
    );
    let delivered = drain(&emulator, &target_url).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: the queue that refused the send receives nothing, got {delivered:?}"
    );
}
