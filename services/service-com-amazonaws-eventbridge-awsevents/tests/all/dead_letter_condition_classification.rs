// SPDX-License-Identifier: CC0-1.0

//! The CONDITION a failed delivery is reported under, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter`, with Amazon SQS, Step Functions, IAM and CloudWatch
//! on the same engine — so each row's condition is read where a client reads one:
//! off the dead letter the target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here: the `ERROR_CODE` EventBridge stamps on a dead-lettered
//! event is a CLASS of failure rather than the target service's own error code — and
//! for a delivery that service ANSWERED it is the class that answer's code belongs to.
//! So `NO_PERMISSIONS` is the class of a refusal for want of a permission this service
//! decided itself (a role whose policies withhold the call, the class
//! `tests/all/dead_letter_error_code.rs` spans over the two steps of a role-governed
//! handover) and of an answer given under an authorization code; `NO_RESOURCE` is the
//! class of every answer given under a target service's NOT-FOUND code — a target the
//! delivery could not address at all, and a queue whose own document withheld the send,
//! which Amazon SQS answers under that same not-found code because it does not disclose
//! a queue it will not let the caller write to; a failure of any other kind is
//! `UNKNOWN`. The
//! target service's own answer is what the `ERROR_MESSAGE` beside it carries, in
//! full, so nothing of the refusal is lost by classifying it.
//!
//! That is what makes the attribute usable: a consumer of a dead-letter queue keys
//! its alarms and its replay tooling off the same few conditions whichever service
//! the target lives in, which it cannot do if the code varies with the target's
//! vendor.
//!
//! The table spans the class rather than one case: the rows differ along WHICH
//! classification the failure falls in, along the target's SERVICE (a queue, a
//! state machine), and along the FORM the failure takes within one class — a queue
//! granting nothing, one refused by a policy scoped to another rule, one that was never
//! created and one deleted after the target was written all being answered under that
//! service's not-found code, while a queue refusing one parameter
//! of the send is a third class on the same service and a state machine whose role
//! withholds the call is the permission one.
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and
//! CloudWatch on one engine, and the queue, role and rule fixtures) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, InvocationOutcome, DEFAULT_EVENT_BUS_NAME, NO_PERMISSIONS, NO_RESOURCE,
    SERVICE_PRINCIPAL, UNKNOWN_CONDITION,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, given_queue, given_queue_with_policy, given_role,
    given_rule, given_state_machine, granting_events, invocations, is_request_id,
    not_disclosed_answer_head, publish, sqs, Emulator, QUEUE_ABSENT_SENTENCE, REGION,
};

/// The rule every row publishes through.
const RULE: &str = "classified-delivery";

/// The id the one target of that rule carries.
const TARGET_ID: &str = "t1";

/// The action a state-machine target's invocation performs, which its role has to
/// be allowed to make so the role gate is not what the row's delivery fails at.
const START_EXECUTION: &str = "states:StartExecution";

/// The message group a FIFO queue target names, which EventBridge requires of one
/// before it will install it at all.
const MESSAGE_GROUP_ID: &str = "classified";

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// A rule of this account and Region that is NOT the one publishing below, which a
/// target queue's policy can be scoped to so its grant does not reach this
/// delivery.
const OTHER_RULE: &str = "somebody-elses-rule";

/// A queue policy granting this service `sqs:SendMessage` on `queue_arn`, scoped by
/// `aws:SourceArn` to the deliveries of the rule `rule` and no other — IAM's own
/// condition key and operator, as a queue's `Policy` is an IAM policy document.
fn granting_one_rule(queue_arn: &str, rule: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": {
                "ArnEquals": {"aws:SourceArn": rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, rule)},
            },
        }],
    })
    .to_string()
}

/// What a row's target is, and how it is brought into the state the row needs.
enum Target {
    /// A queue carrying the policy a delivery needs.
    Queue,
    /// A queue carrying no policy at all, so it has granted this service nothing.
    QueueGrantingNothing,
    /// A queue whose policy grants this service the send, but only for the
    /// deliveries of a rule other than the one publishing — so the grant does not
    /// reach this delivery, on a queue that is plainly there.
    QueueGrantingAnotherRule,
    /// A queue ARN of this account that no queue was ever created for.
    QueueNeverCreated,
    /// A queue created, named by the target, and then deleted.
    DeletedQueue,
    /// A FIFO queue with no content-based deduplication, whose send needs a
    /// deduplication id no EventBridge delivery carries.
    FifoQueueWithoutContentDeduplication,
    /// A state-machine ARN of this account that no state machine exists for.
    StateMachineNeverCreated,
    /// A state machine that IS there, named by a target whose role can be assumed
    /// but whose policies allow the invocation's own API call nowhere — a target
    /// service that really does answer an authorization refusal.
    StateMachineWhoseRoleWithholdsTheCall,
}

/// One row: the case, the target the rule names and the condition the dead letter
/// is to report — `None` for a row whose delivery must go through.
struct Row {
    case: &'static str,
    target: Target,
    condition: Option<&'static str>,
}

/// A queue called `name` whose `FifoQueue` attribute is set and whose
/// `ContentBasedDeduplication` is NOT — so Amazon SQS refuses a send that carries
/// no deduplication id, which is every send EventBridge makes ("if you specify a
/// fair or FIFO queue as a target, the queue must have content-based
/// deduplication enabled", `Target$SqsParameters`); answers its ARN.
async fn given_fifo_queue(emulator: &Emulator, case: &str, name: &str) -> String {
    let fifo = format!("{name}.fifo");
    let (status, body) = sqs(
        emulator,
        "CreateQueue",
        &json!({
            "QueueName": fifo,
            "Attributes": { "FifoQueue": "true" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": granting_events(&arn) } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
    arn
}

/// The ARN of the row's target, with whatever the row's state needs done to it
/// already done — and the `RoleArn` the target names, for the kinds AWS invokes
/// under one.
async fn given_target(
    emulator: &Emulator,
    case: &str,
    index: usize,
    target: &Target,
) -> (String, Option<String>) {
    match target {
        Target::Queue => {
            let (_url, arn) = given_queue(emulator, &format!("classified-{index}")).await;
            (arn, None)
        }
        Target::QueueGrantingNothing => {
            let (_url, arn) =
                given_queue_with_policy(emulator, &format!("classified-{index}"), None).await;
            (arn, None)
        }
        Target::QueueGrantingAnotherRule => {
            let scoped = |queue_arn: &str| granting_one_rule(queue_arn, OTHER_RULE);
            let (_url, arn) =
                given_queue_with_policy(emulator, &format!("classified-{index}"), Some(&scoped))
                    .await;
            (arn, None)
        }
        Target::QueueNeverCreated => (
            format!("arn:aws:sqs:{REGION}:{}:never-created-{index}", account()),
            None,
        ),
        Target::DeletedQueue => {
            let (url, arn) = given_queue(emulator, &format!("classified-{index}")).await;
            let (status, body) = sqs(emulator, "DeleteQueue", &json!({ "QueueUrl": url })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteQueue answered {status} {body}"
            );
            (arn, None)
        }
        Target::FifoQueueWithoutContentDeduplication => (
            given_fifo_queue(emulator, case, &format!("classified-{index}")).await,
            None,
        ),
        Target::StateMachineNeverCreated => {
            // The target's role admits the call, so what the row's delivery fails
            // at is the state machine itself and not the role gate.
            let role = given_role(
                emulator,
                &format!("classified-role-{index}"),
                SERVICE_PRINCIPAL,
                Some(START_EXECUTION),
            )
            .await;
            (
                format!(
                    "arn:aws:states:{REGION}:{}:stateMachine:never-created-{index}",
                    account()
                ),
                Some(role),
            )
        }
        Target::StateMachineWhoseRoleWithholdsTheCall => {
            // The role can be assumed and allows nothing, so what the row's
            // delivery fails at is the permission for the call it would make.
            let role = given_role(
                emulator,
                &format!("classified-role-{index}"),
                SERVICE_PRINCIPAL,
                None,
            )
            .await;
            (
                given_state_machine(emulator, &format!("classified-{index}")).await,
                Some(role),
            )
        }
    }
}

/// The dead-letter `ERROR_CODE` names the class of failure in EventBridge's own
/// terms — for an answered refusal, the class that answer's own code belongs to — over a
/// table of failures spanning three target services and every class.
#[tokio::test]
async fn the_dead_letter_condition_classifies_why_the_delivery_failed() {
    let rows = [
        Row {
            case: "a queue that admits the delivery",
            target: Target::Queue,
            condition: None,
        },
        Row {
            case: "a queue whose policy grants this service nothing",
            target: Target::QueueGrantingNothing,
            condition: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue whose policy is scoped by aws:SourceArn to another rule",
            target: Target::QueueGrantingAnotherRule,
            condition: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue ARN no queue was created for",
            target: Target::QueueNeverCreated,
            condition: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue deleted after the target was written",
            target: Target::DeletedQueue,
            condition: Some(NO_RESOURCE),
        },
        Row {
            case: "a state-machine ARN no state machine was created for",
            target: Target::StateMachineNeverCreated,
            condition: Some(NO_RESOURCE),
        },
        Row {
            case: "a state machine whose target role withholds the invocation's call",
            target: Target::StateMachineWhoseRoleWithholdsTheCall,
            condition: Some(NO_PERMISSIONS),
        },
        Row {
            case: "a FIFO queue whose send has no deduplication id and derives none",
            target: Target::FifoQueueWithoutContentDeduplication,
            condition: Some(UNKNOWN_CONDITION),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("classified-dlq-{index}")).await;
        let (target_arn, role_arn) = given_target(&emulator, case, index, &row.target).await;
        let mut target = json!({
            "Id": TARGET_ID,
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        });
        if let Some(role_arn) = &role_arn {
            target["RoleArn"] = json!(role_arn);
        }
        // A FIFO queue target is only installable with the message group its send
        // carries, so the row whose queue is a FIFO one names one: what that row's
        // delivery then fails at is the deduplication id, not the group.
        if matches!(row.target, Target::FifoQueueWithoutContentDeduplication) {
            target["SqsParameters"] = json!({ "MessageGroupId": MESSAGE_GROUP_ID });
        }
        given_rule(&emulator, RULE, json!([target])).await;

        publish(&emulator).await;

        let recorded = invocations(&emulator);
        let outcome = recorded
            .first()
            .map(|(_, _, outcome)| outcome.clone())
            .unwrap_or_else(|| panic!("case {case:?}: nothing was recorded"));
        let dead_lettered = drain(&emulator, &dead_letter_url).await;
        let Some(condition) = row.condition else {
            assert_eq!(
                outcome,
                InvocationOutcome::Invoked,
                "case {case:?}: expected the delivery to go through, got {outcome:?}"
            );
            assert!(
                dead_lettered.is_empty(),
                "case {case:?}: a delivery that went through is not dead-lettered, got \
                 {dead_lettered:?}"
            );
            continue;
        };
        let InvocationOutcome::Failed {
            code: actual,
            message,
        } = &outcome
        else {
            panic!("case {case:?}: expected a failed invocation, got {outcome:?}");
        };
        assert_eq!(
            actual, condition,
            "case {case:?}: expected the condition {condition}, got {outcome:?}"
        );
        assert_eq!(
            dead_lettered.len(),
            1,
            "case {case:?}: an undeliverable event goes to the dead-letter queue, got \
             {dead_lettered:?}"
        );
        let attributes = &dead_lettered[0]["MessageAttributes"];
        assert_eq!(
            attributes["ERROR_CODE"]["StringValue"],
            json!(condition),
            "case {case:?}: expected the dead-letter ERROR_CODE {condition}, got {:?}",
            dead_lettered[0]
        );
        // Nothing of the target's own answer is lost by classifying it: the
        // message beside the condition is that answer as the delivery read it.
        assert_eq!(
            attributes["ERROR_MESSAGE"]["StringValue"],
            json!(message),
            "case {case:?}: expected the target's own answer as the ERROR_MESSAGE, got {:?}",
            dead_lettered[0]
        );
        assert_ne!(
            attributes["ERROR_MESSAGE"]["StringValue"],
            Value::Null,
            "case {case:?}: the dead letter reports the answer the target gave, got {:?}",
            dead_lettered[0]
        );
    }
}

/// A queue target refused by its own policy reports the answer Amazon SQS gives a
/// caller whose send the documents in force over the queue do not permit: that
/// service does not disclose the queue, so the answer is its not-found code at HTTP
/// 400 under the fixed sentence that states the absence and the want of access
/// without saying which holds — which names neither the principal, nor the action,
/// nor the queue, and is why the condition beside it is the resource one. The
/// sentence a queue that is simply not there earns is that service's OTHER sentence
/// for the same code, so the two are told apart by the answer and not by the class.
#[tokio::test]
async fn a_queue_refusing_the_send_reports_the_answer_that_service_gives() {
    let case = "a queue granting this service nothing";
    let emulator = emulator();
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "worded-dlq").await;
    let (_target_url, target_arn) =
        given_queue_with_policy(&emulator, "worded-target", None).await;
    given_rule(
        &emulator,
        RULE,
        json!([{
            "Id": TARGET_ID,
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
        }]),
    )
    .await;

    publish(&emulator).await;

    let dead_lettered = drain(&emulator, &dead_letter_url).await;
    assert_eq!(
        dead_lettered.len(),
        1,
        "case {case:?}: expected one dead letter, got {dead_lettered:?}"
    );
    let reported = dead_lettered[0]["MessageAttributes"]["ERROR_MESSAGE"]["StringValue"]
        .as_str()
        .unwrap_or_default();
    let expected = not_disclosed_answer_head();
    let request_id = answered_request_id(reported, &expected).unwrap_or_else(|| {
        panic!("case {case:?}: expected {expected:?} and the id of the refused request, got {reported:?}")
    });
    assert!(
        is_request_id(&request_id),
        "case {case:?}: expected a request id in the 8-4-4-4-12 form, got {request_id:?}"
    );
    // That answer discloses nothing of the queue it was asked about: neither the
    // principal the send was made as, nor the queue, nor the sentence that service
    // keeps for a queue that is simply not there.
    assert!(
        !reported.contains(target_arn.as_str())
            && !reported.contains(SERVICE_PRINCIPAL)
            && !reported.contains(QUEUE_ABSENT_SENTENCE),
        "case {case:?}: expected an answer naming neither the queue nor the principal nor the \
         absence, got {reported:?}"
    );
}
