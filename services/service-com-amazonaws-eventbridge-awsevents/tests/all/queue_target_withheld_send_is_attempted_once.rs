// SPDX-License-Identifier: CC0-1.0

//! A delivery the target queue's own documents withheld is attempted ONCE, end to end
//! over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter` with Amazon SQS and CloudWatch on the same
//! engine — so what the delivery came to is read where a client reads it: off the
//! message attributes of the dead letter the target's `DeadLetterConfig` queue
//! received, and off the rule's own `AWS/Events` counters.
//!
//! The RULE asserted here: EventBridge assumes no role for an SQS target, so the
//! destination queue's own resource policy alone authorizes the send (`PutTargets`:
//! "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies
//! on resource-based policies"); what the delivery reads back from a queue whose
//! documents do not permit the send is that service's own answer for a queue it will
//! not disclose to the caller — its not-found code at HTTP 400, which is why
//! `SendMessage` models no authorization failure at all
//! (`smithy_query(query="operation",
//! operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! — and EventBridge reports it beside the class that answer's own code belongs to, the
//! resource one (`NO_RESOURCE`), the answer's own fixed text stating the absence
//! and the want of access without saying which holds. Such an
//! answer is the
//! same however often it is asked for, so nothing retries the handover: ONE attempt
//! is counted, ONE dead letter is parked, and the letter accounts for no retry
//! whatever the target's `RetryPolicy` allows.
//!
//! The table spans that rule rather than one case: its rows differ along the
//! `RetryPolicy` the target carries — none at all (the documented defaults apply),
//! both bounds at their published minima, both at their published maxima, one step
//! inside the maxima, each member alone with the other defaulted
//! (`RetryPolicy$MaximumRetryAttempts` `@range(min: 0, max: 185)`,
//! `RetryPolicy$MaximumEventAgeInSeconds` `@range(min: 60, max: 86400)`,
//! `smithy_query(query="shape", shape="RetryPolicy")`) — and along the FORM the
//! withholding takes (no document at all, an explicit `Deny` beside a grant, a grant
//! scoped by `aws:SourceArn` to another rule, a grant of another action of the same
//! service). The last row is the same generous policy over a queue that DOES grant the
//! send, so the table cannot pass by failing every delivery.
//!
//! The condition operators and keys the rows use are IAM's own, because a queue's
//! `Policy` is an IAM policy document
//! (`smithy_query(query="doc",shape="com.amazonaws.sqs#QueueAttributeName")`).
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and
//! CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::retry_bounds::{
    EXHAUSTED_RETRY_CONDITION, RETRY_ATTEMPTS,
};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, FAILED_INVOCATIONS_METRIC, INVOCATIONS_SENT_TO_DLQ_METRIC,
    INVOCATION_ATTEMPTS_METRIC, NO_RESOURCE, SERVICE_PRINCIPAL,
    SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, given_queue, given_queue_with_policy, given_rule,
    granting_events, is_request_id, metric_sum, not_disclosed_answer_head, publish, Emulator,
    REGION,
};

/// The rule every row below publishes through.
const RULE: &str = "attempted-once";

/// The id the one target of that rule carries.
const TARGET_ID: &str = "queue";

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

/// A document granting the send and DENYING it beside the grant, which refuses the
/// delivery whatever is granted.
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

/// A grant of the send scoped by `aws:SourceArn` to the deliveries of a rule other
/// than the one publishing here, so the grant does not reach this delivery.
fn granting_another_rule(queue_arn: &str) -> String {
    let other = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule");
    document(json!([grant(
        SERVICE_PRINCIPAL,
        SEND_MESSAGE,
        queue_arn,
        Some(json!({"ArnEquals": {"aws:SourceArn": other}})),
    )]))
}

/// A grant of a DIFFERENT action of the same service, so no statement is about the
/// call the delivery makes.
fn granting_another_action(queue_arn: &str) -> String {
    document(json!([grant(
        SERVICE_PRINCIPAL,
        "sqs:ReceiveMessage",
        queue_arn,
        None
    )]))
}

/// One row: the case, the `RetryPolicy` the target carries (`None` for a target
/// configuring none), the document its queue carries (`None` for a queue carrying
/// none), and whether that document admits the send.
struct Row {
    case: &'static str,
    retry: Option<Value>,
    policy: Option<fn(&str) -> String>,
    admits: bool,
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `Sum` of `metric` for the rule every row publishes through.
async fn counted(emulator: &Emulator, metric: &str) -> f64 {
    metric_sum(emulator, metric, DEFAULT_EVENT_BUS_NAME, RULE).await
}

/// A rule whose one queue target names `target_arn`, dead-letters onto
/// `dead_letter_arn` and carries the row's `RetryPolicy`.
async fn given_rule_with_retry(
    emulator: &Emulator,
    target_arn: &str,
    dead_letter_arn: &str,
    retry: Option<&Value>,
) {
    let mut target = json!({
        "Id": TARGET_ID,
        "Arn": target_arn,
        "DeadLetterConfig": { "Arn": dead_letter_arn },
    });
    if let Some(retry) = retry {
        target["RetryPolicy"] = retry.clone();
    }
    given_rule(emulator, RULE, json!([target])).await;
}

/// A withheld send is answered by the target's own service, classified as the class of
/// that answer, and attempted once — whatever the target's `RetryPolicy` allows.
#[tokio::test]
async fn a_withheld_queue_delivery_is_attempted_once_under_every_retry_policy() {
    let rows = [
        Row {
            case: "a target configuring no RetryPolicy, on a queue carrying no document",
            retry: None,
            policy: None,
            admits: false,
        },
        Row {
            case: "both retry bounds at their published minima",
            retry: Some(json!({ "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 })),
            policy: None,
            admits: false,
        },
        Row {
            case: "both retry bounds at their published maxima, over an explicit Deny",
            retry: Some(json!({
                "MaximumRetryAttempts": 185,
                "MaximumEventAgeInSeconds": 86_400,
            })),
            policy: Some(granting_then_denying),
            admits: false,
        },
        Row {
            case: "one step inside both maxima, over an explicit Deny",
            retry: Some(json!({
                "MaximumRetryAttempts": 184,
                "MaximumEventAgeInSeconds": 86_399,
            })),
            policy: Some(granting_then_denying),
            admits: false,
        },
        Row {
            case: "an allowance alone, the age bound defaulted, over a grant scoped to \
                   another rule",
            retry: Some(json!({ "MaximumRetryAttempts": 1 })),
            policy: Some(granting_another_rule),
            admits: false,
        },
        Row {
            case: "an age bound alone, the allowance defaulted, over a grant of another action",
            retry: Some(json!({ "MaximumEventAgeInSeconds": 3_600 })),
            policy: Some(granting_another_action),
            admits: false,
        },
        Row {
            case: "both retry bounds at their published maxima, over a queue granting the send",
            retry: Some(json!({
                "MaximumRetryAttempts": 185,
                "MaximumEventAgeInSeconds": 86_400,
            })),
            policy: Some(granting_events),
            admits: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("attempted-dlq-{index}")).await;
        let policy: Option<&dyn Fn(&str) -> String> = row
            .policy
            .as_ref()
            .map(|written| written as &dyn Fn(&str) -> String);
        let (target_url, target_arn) =
            given_queue_with_policy(&emulator, &format!("attempted-target-{index}"), policy).await;
        given_rule_with_retry(&emulator, &target_arn, &dead_letter_arn, row.retry.as_ref()).await;

        publish(&emulator).await;

        let delivered = drain(&emulator, &target_url).await;
        let parked = drain(&emulator, &dead_letter_url).await;
        let attempts = counted(&emulator, INVOCATION_ATTEMPTS_METRIC).await;
        let accepted = counted(&emulator, SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC).await;
        let failed = counted(&emulator, FAILED_INVOCATIONS_METRIC).await;
        let parked_count = counted(&emulator, INVOCATIONS_SENT_TO_DLQ_METRIC).await;
        // One handover, so one attempt — a retried delivery would count one per
        // attempt it made ("InvocationAttempts: the number of times EventBridge
        // attempted invoking a target").
        assert_eq!(
            attempts, 1.0,
            "case {case:?}: expected one {INVOCATION_ATTEMPTS_METRIC} datapoint for the one \
             handover, got {attempts}"
        );

        if row.admits {
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: the granted queue receives the event, got {delivered:?}"
            );
            assert!(
                parked.is_empty(),
                "case {case:?}: a delivery that went through is not dead-lettered, got {parked:?}"
            );
            assert_eq!(
                (accepted, failed, parked_count),
                (1.0, 0.0, 0.0),
                "case {case:?}: expected the accepted attempt counted and neither failure \
                 counter written, got accepted {accepted}, failed {failed}, parked \
                 {parked_count}"
            );
            continue;
        }

        assert!(
            delivered.is_empty(),
            "case {case:?}: a withheld delivery leaves the target queue empty, got {delivered:?}"
        );
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {parked:?}"
        );
        let message = &parked[0];
        // The class is the one that answer's own code belongs to: that service answered
        // its not-found code, so the delivery is reported as the resource class.
        let class = attribute(message, ERROR_CODE_ATTRIBUTE);
        assert_eq!(
            class, NO_RESOURCE,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {NO_RESOURCE:?}, \
             got {class:?} in {message:?}"
        );
        // The sentence beside it is that service's own answer, in full: its fixed text
        // inside the metadata block of the one refused call, whose id is the only part
        // that varies.
        let head = not_disclosed_answer_head();
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        let id = answered_request_id(&reported, &head).unwrap_or_else(|| {
            panic!("case {case:?}: expected {head:?} then the id of the refused call, got {reported:?}")
        });
        assert!(
            is_request_id(&id),
            "case {case:?}: expected the refused call's own id in that answer, got {reported:?}"
        );
        // Nothing retried the handover, so the letter accounts for no retry and names
        // no exhausted bound, however generous the target's policy was.
        for retried in [RETRY_ATTEMPTS, EXHAUSTED_RETRY_CONDITION] {
            assert_eq!(
                message["MessageAttributes"][retried],
                Value::Null,
                "case {case:?}: a refusal nothing retried carries no {retried}, got {message:?}"
            );
        }
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
        // The counters agree with the letter: the one attempt failed, the event was
        // parked, and nothing was accepted.
        assert_eq!(
            (accepted, failed, parked_count),
            (0.0, 1.0, 1.0),
            "case {case:?}: expected the one attempt counted as failed and parked once, got \
             accepted {accepted}, failed {failed}, parked {parked_count}"
        );
    }
}
