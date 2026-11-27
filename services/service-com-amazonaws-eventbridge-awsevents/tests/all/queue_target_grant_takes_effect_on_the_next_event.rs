// SPDX-License-Identifier: CC0-1.0

//! An Amazon SQS target handover is decided against the QUEUE's own resource policy AS IT
//! STANDS at that delivery, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter` with Amazon
//! SQS on the same engine — so every observation is made where a client makes one: on the
//! target queue, and on the message attributes of the dead letter the target's
//! `DeadLetterConfig` queue received.
//!
//! The RULE pinned here: "for Lambda functions, Amazon SNS topics and Amazon SQS queues,
//! EventBridge relies on resource-based policies" (`PutTargets`), so a queue target is reached
//! with `sqs:SendMessage` as the service principal and the document in force is the one the
//! queue carries WHEN the event is routed, not one read when the target was written. A send no
//! document in force over the queue admits is refused by that service without disclosing the
//! queue — `SendMessage` models no authorization failure at all
//! (`smithy_query(query="operation", operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! and such a send is answered `QueueDoesNotExist`, HTTP 400 under the `awsQueryError` code
//! `AWS.SimpleQueueService.NonExistentQueue` (`smithy_query(query="shape",
//! shape="QueueDoesNotExist", model_path=...)`), carrying the fixed sentence that states the
//! absence and the want of access without saying which holds — so the dead letter carries that
//! answer verbatim in `ERROR_MESSAGE`, with only the id of the ONE refused call varying, and
//! beside it the class that answer's own code belongs to, the resource one (`NO_RESOURCE`).
//! Nothing of the refused send appears in that answer: not the queue, not the principal, not
//! the action, and no authorization status or code that service does not answer on this call.
//! A grant written onto the SAME queue afterwards then makes the NEXT matching event deliver
//! to it with the rule and the target untouched — which is what proves the queue was there all
//! along and only the authorization had failed, and what tells a client "refused for want of a
//! grant" from "this target is never delivered to".
//!
//! The rows differ along what the rule generalises over and nothing else: the FORM the
//! withholding takes (no document at all, an explicit `Deny` of the send, a grant scoped by
//! `aws:SourceArn` to another rule of the bus, a grant of the send to another service
//! principal) and the FORM the grant that restores delivery takes (unconditioned, scoped by
//! `aws:SourceArn` to the publishing rule, spelled in the console's casing of the action,
//! naming every resource with the wildcard). Each row is a FRESH emulator, so one row's
//! documents never decide another row's delivery.
//!
//! The condition operators and keys the rows use are IAM's own, because a queue's `Policy` is
//! an IAM policy document
//! (`smithy_query(query="doc",shape="com.amazonaws.sqs#QueueAttributeName")`).
//!
//! `tests/all/queue_target_policy_authorization.rs` is the suite for WHICH document shapes
//! admit the send and `tests/all/queue_target_withheld_send_answer.rs` the suite for what one
//! refused delivery reports; what this one adds is that the decision is taken per delivery —
//! the queue analogue of `tests/all/function_target_grant_takes_effect_on_the_next_event.rs`.
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and CloudWatch on
//! one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, NO_RESOURCE, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, given_queue, given_queue_with_policy, given_rule,
    granting_events, is_request_id, not_disclosed_answer_head, publish, sqs, Emulator,
    ANSWER_TAIL, DETAIL_TYPE, QUEUE_ABSENT_SENTENCE, QUEUE_NOT_DISCLOSED_CODE,
    QUEUE_NOT_DISCLOSED_SENTENCE, REGION, SOURCE,
};

/// The rule every delivery below is made by, and the id of its one queue target.
const RULE: &str = "grant-timed";
const TARGET_ID: &str = "queue";

/// The IAM action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The casing the console writes that same action in — IAM compares an action name
/// case-insensitively, so a grant spelled this way grants the same send.
const CONSOLE_CASED_SEND_MESSAGE: &str = "SQS:SendMessage";

/// The casing this service's own IAM-style refusals name an action in, which no answer of the
/// target's service may read.
const CASE_FOLDED_SEND_MESSAGE: &str = "sqs:sendmessage";

/// The wildcard an IAM `Resource` element may name instead of an ARN.
const EVERY_RESOURCE: &str = "*";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";

/// The status and code pair a consumer parses out of that answer, as it is written in it.
/// Stated here rather than read from any implementation, so a wrong answer cannot make this
/// suite pass.
const REPORTED_STATUS: &str = "Status Code: 400";
const REPORTED_CODE: &str = "Error Code: AWS.SimpleQueueService.NonExistentQueue";

/// The authorization pair that service does NOT answer on this call, in the forms an
/// IAM-authored sentence would state them.
const AUTHORIZATION_STATUS: &str = "Status Code: 403";
const AUTHORIZATION_CODE: &str = "AccessDenied";
const AUTHORIZATION_SENTENCE_HEAD: &str = "is not authorized to perform";

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of `action` to the service principal on `resource`, scoped by `condition` where
/// the row states one.
fn grant(action: &str, resource: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A document stating an explicit `Deny` of the send beside a grant of it: the deny refuses the
/// delivery whatever is granted beside it.
fn denying_the_send(queue_arn: &str) -> String {
    document(json!([
        grant(SEND_MESSAGE, queue_arn, None),
        {
            "Effect": "Deny",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
    ]))
}

/// A grant of the send scoped by `aws:SourceArn` to the deliveries of ANOTHER rule of the bus,
/// so no statement is about the rule publishing here.
fn granting_another_rule(queue_arn: &str) -> String {
    let other = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule");
    document(json!([grant(
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"ArnEquals": {"aws:SourceArn": other}})),
    )]))
}

/// A grant of the send to a DIFFERENT service principal, so nothing is granted to the one a
/// queue target is reached as.
fn granting_another_principal(queue_arn: &str) -> String {
    document(json!([{
        "Effect": "Allow",
        "Principal": {"Service": "s3.amazonaws.com"},
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
    }]))
}

/// A grant of the send scoped by `aws:SourceArn` to the rule publishing here, which authorizes
/// that rule's deliveries.
fn granting_this_rule(queue_arn: &str) -> String {
    let publishing = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE);
    document(json!([grant(
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"ArnEquals": {"aws:SourceArn": publishing}})),
    )]))
}

/// A grant of the send spelled in the console's casing of the action.
fn granting_console_cased(queue_arn: &str) -> String {
    document(json!([grant(
        CONSOLE_CASED_SEND_MESSAGE,
        queue_arn,
        None
    )]))
}

/// A grant of the send naming every resource with the wildcard rather than the queue's ARN.
fn granting_every_resource(_queue_arn: &str) -> String {
    document(json!([grant(SEND_MESSAGE, EVERY_RESOURCE, None)]))
}

/// One row: how the queue withholds the first delivery, and the document written onto it
/// afterwards that admits the next one.
struct Row {
    case: &'static str,
    /// The document the queue carries for the first publication, or none at all.
    withholding: Option<fn(&str) -> String>,
    /// The document written onto the same queue before the second publication.
    restoring: fn(&str) -> String,
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Write `policy` onto the queue at `url` as a client does, through that service's own
/// `SetQueueAttributes`.
async fn write_policy(emulator: &Emulator, case: &str, url: &str, policy: &str) {
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
}

/// Assert the dead letter of a withheld delivery reports the answer that service gave: its
/// fixed sentence inside the metadata block naming that service, the status and the code, with
/// only the id of the one refused call varying — and nothing of the refused send in it.
fn assert_withheld_answer(case: &str, reported: &str, target_arn: &str) {
    let head = not_disclosed_answer_head();
    let request_id = answered_request_id(reported, &head).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {head:?} then the id of the refused call and \
             {ANSWER_TAIL:?}, got {reported:?}"
        )
    });
    assert!(
        is_request_id(&request_id),
        "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {request_id:?}"
    );
    // The pair a consumer parses out of the message, as that answer states it.
    for stated in [
        QUEUE_NOT_DISCLOSED_SENTENCE,
        REPORTED_STATUS,
        REPORTED_CODE,
        QUEUE_NOT_DISCLOSED_CODE,
    ] {
        assert!(
            reported.contains(stated),
            "case {case:?}: expected the reported answer to state {stated:?}, got {reported:?}"
        );
    }
    // That answer discloses nothing of the refused send, states no authorization status or
    // code this call cannot answer, and is not the sentence kept for a queue that is gone.
    for withheld in [
        target_arn,
        SERVICE_PRINCIPAL,
        SEND_MESSAGE,
        CASE_FOLDED_SEND_MESSAGE,
        AUTHORIZATION_STATUS,
        AUTHORIZATION_CODE,
        AUTHORIZATION_SENTENCE_HEAD,
        QUEUE_ABSENT_SENTENCE,
    ] {
        assert!(
            !reported.contains(withheld),
            "case {case:?}: that service discloses nothing of the refused send, so {withheld:?} \
             must not appear in {reported:?}"
        );
    }
}

/// Assert `message` is the envelope of the event this suite publishes.
fn assert_delivered_envelope(case: &str, message: &Value) {
    let body = message["Body"].as_str().unwrap_or_default();
    let envelope: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the delivered body is no JSON: {error}"));
    assert_eq!(
        envelope["source"].as_str(),
        Some(SOURCE),
        "case {case:?}: expected the delivered envelope's source to be {SOURCE:?}, got \
         {envelope}"
    );
    assert_eq!(
        envelope["detail-type"].as_str(),
        Some(DETAIL_TYPE),
        "case {case:?}: expected the delivered envelope's detail-type to be {DETAIL_TYPE:?}, \
         got {envelope}"
    );
}

/// The queue's own document decides each delivery as it stands at that delivery: a send it
/// withholds is dead-lettered under the answer that service gave, and a grant written
/// afterwards delivers the next matching event to the same queue.
#[tokio::test]
async fn a_queue_grant_written_after_a_withheld_delivery_admits_the_next_event() {
    let rows = [
        Row {
            case: "no document at all, then an unconditioned grant",
            withholding: None,
            restoring: granting_events,
        },
        Row {
            case: "an explicit deny of the send, then a grant scoped to the publishing rule",
            withholding: Some(denying_the_send),
            restoring: granting_this_rule,
        },
        Row {
            case: "a grant scoped to another rule, then one spelled in the console's casing",
            withholding: Some(granting_another_rule),
            restoring: granting_console_cased,
        },
        Row {
            case: "a grant to another principal, then one naming every resource",
            withholding: Some(granting_another_principal),
            restoring: granting_every_resource,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("grant-timed-dlq-{index}")).await;
        let withholding: Option<&dyn Fn(&str) -> String> = row
            .withholding
            .as_ref()
            .map(|written| written as &dyn Fn(&str) -> String);
        let (target_url, target_arn) = given_queue_with_policy(
            &emulator,
            &format!("grant-timed-target-{index}"),
            withholding,
        )
        .await;
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

        // The leg the queue's document withholds: nothing is enqueued on the target and the
        // envelope is parked on the dead-letter queue instead, carrying that service's answer.
        publish(&emulator).await;
        let withheld = drain(&emulator, &target_url).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: a withheld delivery leaves the target queue empty, got {withheld:?}"
        );
        let parked = drain(&emulator, &dead_letter_url).await;
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {parked:?}"
        );
        let class = attribute(&parked[0], ERROR_CODE_ATTRIBUTE);
        assert_eq!(
            class, NO_RESOURCE,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {NO_RESOURCE:?}, \
             got {class:?} in {:?}",
            parked[0]
        );
        let named_rule = attribute(&parked[0], RULE_ARN_ATTRIBUTE);
        assert_eq!(
            named_rule,
            rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE),
            "case {case:?}: expected the dead letter's {RULE_ARN_ATTRIBUTE} to name the failed \
             delivery's rule, got {named_rule:?}"
        );
        let named_target = attribute(&parked[0], TARGET_ARN_ATTRIBUTE);
        assert_eq!(
            named_target, target_arn,
            "case {case:?}: expected the dead letter's {TARGET_ARN_ATTRIBUTE} to name the \
             failed delivery's target, got {named_target:?}"
        );
        assert_withheld_answer(
            case,
            &attribute(&parked[0], ERROR_MESSAGE_ATTRIBUTE),
            &target_arn,
        );

        // The grant is written onto the SAME queue, with the rule and its target untouched.
        write_policy(
            &emulator,
            case,
            &target_url,
            &(row.restoring)(&target_arn),
        )
        .await;

        publish(&emulator).await;
        let delivered = drain(&emulator, &target_url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected the grant to admit the next event, got {delivered:?}"
        );
        assert_delivered_envelope(case, &delivered[0]);
        let parked_again = drain(&emulator, &dead_letter_url).await;
        assert!(
            parked_again.is_empty(),
            "case {case:?}: a delivered event is dead-lettered nowhere, got {parked_again:?}"
        );
    }
}
