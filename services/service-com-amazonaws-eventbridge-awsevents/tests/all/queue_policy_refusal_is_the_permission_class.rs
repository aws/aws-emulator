// SPDX-License-Identifier: CC0-1.0

//! WHICH refusals of a delivery are the PERMISSION class (`NO_PERMISSIONS`) and which
//! are not, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter` with
//! Amazon SQS, Amazon SNS, Step Functions and IAM on the same engine — so each row is
//! read where a consumer of a dead-letter queue reads it: off the message attributes of
//! the dead letter the target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here bounds that class: a delivery the target's own service ANSWERED
//! is classified by the class that answer's error CODE belongs to, and a delivery THIS
//! service refused without a call being made carries the class of the step that refused
//! it. So the permission class covers an answer given under an authorization code — Amazon
//! SNS refusing a publish its topic's document does not permit
//! (`AuthorizationErrorException`, `awsQueryError` code `AuthorizationError`,
//! `smithy_query(query="shape", shape="AuthorizationErrorException",
//! model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`)
//! — and the identity policies in force on a role EventBridge invokes a target AS, which
//! is this service's own verdict for want of a permission.
//!
//! What the class does NOT cover is either of the two other things a refused delivery can
//! be. A role this service cannot assume at all is `FAILED_TO_ASSUME_ROLE`, the step that
//! failed, asserted here as the contrast that keeps the permission class from swallowing
//! every failed delivery. And a send a queue's own `Policy` withheld is the RESOURCE class,
//! because that is the class of the answer Amazon SQS gave it: that service models no
//! authorization failure on `SendMessage` at all
//! (`smithy_query(query="operation", operation="SendMessage",
//! model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`)
//! because it does not disclose a queue it will not let the caller write to, so the answer
//! the dead letter's `ERROR_MESSAGE` carries comes back under that service's own not-found
//! code and the class beside it is read off that same code.
//!
//! The rows therefore differ along the SERVICE that answered and along WHAT it was
//! asked (a publish its topic's document withholds, an execution the invoking role's
//! policies withhold, a trust policy that does not let this service assume the role at
//! all, and the queue-policy send), so a classifier that answered one class for every
//! failed delivery fails here whichever way it leant. The key an ENCRYPTED queue's
//! messages are encrypted under is a second refusal Amazon SQS relays as an authorization
//! failure (`KMS.AccessDeniedException`); that is
//! `tests/all/encrypted_queue_target_key_policy.rs`'s rule and is not re-asserted here.
//!
//! The harness (the router with EventBridge, Amazon SQS, Amazon SNS, Step Functions,
//! IAM and CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    FAILED_TO_ASSUME_ROLE, NO_PERMISSIONS, NO_RESOURCE, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_queue_with_policy, given_role, given_rule,
    given_state_machine, publish, sns, xml_text, Emulator, QUEUE_NOT_DISCLOSED_CODE,
};

/// The rule every row below publishes through.
const RULE: &str = "class-bounded";

/// The id the one failing target of that rule carries.
const TARGET_ID: &str = "refused";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The action a state-machine target's invocation performs, which a role that may make
/// it is allowed.
const START_EXECUTION: &str = "states:StartExecution";

/// The error code Amazon SNS answers a publish its topic's document does not permit
/// (`AuthorizationErrorException`'s `awsQueryError` code) — stated here rather than read
/// from any implementation, so a wrong answer cannot make this suite pass.
const AUTHORIZATION_ERROR: &str = "AuthorizationError";

/// Which target refuses the row's handover, and by what.
enum Refused {
    /// An Amazon SNS topic on the document that service generates for its owner, which
    /// grants this service nothing: a publish that service answers as an authorization
    /// refusal.
    TopicGrantingNothing,
    /// A state machine named by a target whose role this service may assume but whose
    /// policies allow the invocation's own call nowhere.
    RoleWithholdingTheCall,
    /// A state machine named by a target whose role does not trust this service at all,
    /// so the delivery failed before any call was made.
    RoleThisServiceCannotAssume,
    /// An Amazon SQS queue carrying no policy, which that service answers without
    /// disclosing the queue — an answer given under its not-found code.
    QueueGrantingNothing,
}

/// One row: the case, what refused the handover, the class the dead letter is to carry
/// and what the answer beside it must state.
struct Row {
    case: &'static str,
    refused: Refused,
    class: &'static str,
    states: Option<&'static str>,
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A topic called `name` on the document Amazon SNS generates for its owner; answers its
/// ARN. Nothing is subscribed to it: what the row reads is the answer the PUBLISH earned,
/// which that topic's own document decides whether or not anybody is listening.
async fn given_topic(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, created) = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status}: {created}"
    );
    xml_text(&created, "TopicArn").unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"))
}

/// The ARN the row's failing target names, the role it names where the row's refusal is
/// a role's, and the url of its queue where the row's target is one.
async fn given_refused_target(
    emulator: &Emulator,
    case: &str,
    index: usize,
    refused: &Refused,
) -> (String, Option<String>, Option<String>) {
    match refused {
        Refused::TopicGrantingNothing => (
            given_topic(emulator, case, &format!("bounded-topic-{index}")).await,
            None,
            None,
        ),
        Refused::RoleWithholdingTheCall => {
            let role = given_role(
                emulator,
                &format!("bounded-role-{index}"),
                SERVICE_PRINCIPAL,
                None,
            )
            .await;
            (
                given_state_machine(emulator, &format!("bounded-machine-{index}")).await,
                Some(role),
                None,
            )
        }
        Refused::RoleThisServiceCannotAssume => {
            // The role allows the call but trusts another service, so this service
            // cannot act as it at all.
            let role = given_role(
                emulator,
                &format!("bounded-untrusting-role-{index}"),
                "lambda.amazonaws.com",
                Some(START_EXECUTION),
            )
            .await;
            (
                given_state_machine(emulator, &format!("bounded-untrusted-machine-{index}")).await,
                Some(role),
                None,
            )
        }
        Refused::QueueGrantingNothing => {
            let (url, arn) =
                given_queue_with_policy(emulator, &format!("bounded-queue-{index}"), None).await;
            (arn, None, Some(url))
        }
    }
}

/// The permission class is the class of an answer given under an AUTHORIZATION code and of
/// this service's own verdict that a role's policies withhold the call — while a role this
/// service cannot assume at all is its own class, and a send answered under Amazon SQS's
/// not-found code is the resource class.
#[tokio::test]
async fn the_permission_class_is_the_class_of_a_withheld_delivery() {
    let rows = [
        Row {
            case: "a topic on the document Amazon SNS generates for its owner",
            refused: Refused::TopicGrantingNothing,
            class: NO_PERMISSIONS,
            states: Some(AUTHORIZATION_ERROR),
        },
        Row {
            case: "a target whose role may be assumed but allows the call nowhere",
            refused: Refused::RoleWithholdingTheCall,
            class: NO_PERMISSIONS,
            states: Some(START_EXECUTION),
        },
        Row {
            case: "a target whose role does not trust this service",
            refused: Refused::RoleThisServiceCannotAssume,
            class: FAILED_TO_ASSUME_ROLE,
            states: None,
        },
        Row {
            case: "a queue carrying no policy at all",
            refused: Refused::QueueGrantingNothing,
            class: NO_RESOURCE,
            states: Some(QUEUE_NOT_DISCLOSED_CODE),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("bounded-dlq-{index}")).await;
        let (target_arn, role_arn, target_url) =
            given_refused_target(&emulator, case, index, &row.refused).await;
        let mut target = json!({
            "Id": TARGET_ID,
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
        });
        if let Some(role_arn) = &role_arn {
            target["RoleArn"] = json!(role_arn);
        }
        given_rule(&emulator, RULE, json!([target])).await;

        publish(&emulator).await;

        if let Some(url) = &target_url {
            let delivered = drain(&emulator, url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a refused delivery leaves the target empty, got {delivered:?}"
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
        // The answer beside the class is the one the refusing service gave, so a reader
        // of the dead letter is told both why the event was not delivered and what the
        // delivery was told.
        if let Some(states) = row.states {
            let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
            assert!(
                reported.contains(states),
                "case {case:?}: expected the reported answer to state {states:?}, got {reported:?}"
            );
        }
    }
}
