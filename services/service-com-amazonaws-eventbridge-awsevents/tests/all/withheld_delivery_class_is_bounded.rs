// SPDX-License-Identifier: CC0-1.0

//! WHERE the class on a dead letter comes from, end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter` with Amazon SQS and Amazon SNS on the same engine — so every row is
//! read where a consumer of a dead-letter queue reads it: off the message attributes of the
//! dead letter the target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here: for a delivery the target's own service ANSWERED, the two
//! attributes a dead letter carries come from that ONE answer — `ERROR_MESSAGE` states it in
//! full, metadata block included, and `ERROR_CODE` names the class the error CODE inside that
//! block belongs to. So the pair can never name two different failures, and the class does not
//! depend on what this service guessed the answering service meant.
//!
//! Each row therefore asserts the LINK rather than a remembered pair: the code is read back out
//! of the reported answer and classified by this suite's own table of what each target service
//! publishes for an absent resource and for a refused identity, and the class beside it has to
//! be the one that table gives. The rows span the services a target lives in (Amazon SQS,
//! Amazon SNS), the FORM the refusal takes (no document at all, an explicit `Deny` beside a
//! grant, a grant scoped by `aws:SourceArn` to another rule, the document Amazon SNS generates
//! for a topic's owner) and whether the target is there at all — and one control row whose
//! queue grants the send receives the event and parks nothing, so no row can pass because
//! every delivery failed.
//!
//! The harness (the router with EventBridge, Amazon SQS, Amazon SNS, Step Functions, IAM and
//! CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    NO_PERMISSIONS, NO_RESOURCE, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_queue_with_policy, given_rule, granting_events, publish,
    sns, xml_text, Emulator, REGION,
};

/// The rule every row below publishes through.
const RULE: &str = "classified-by-its-answer";

/// The id the one failing target of that rule carries.
const TARGET_ID: &str = "answered";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The IAM actions the two target kinds' deliveries perform.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// Every error code a target's service answers for a RESOURCE THAT IS NOT THERE, as each of
/// those services publishes one. Written here rather than read from any implementation, so a
/// wrong classification cannot make this suite pass.
///
/// Source: each target service's own model — `smithy_query(query="operation",
/// operation="SendMessage")` of
/// `services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json`
/// names `QueueDoesNotExist`, whose `awsQueryError` code is the prefixed one
/// (`smithy_query(query="shape", shape="QueueDoesNotExist")`), and
/// `smithy_query(query="operation", operation="Publish")` of
/// `services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json`
/// names `NotFoundException`.
const RESOURCE_ABSENT_CODES: [&str; 3] = [
    "AWS.SimpleQueueService.NonExistentQueue",
    "NotFoundException",
    "ResourceNotFoundException",
];

/// Every error code a target's service answers when it refuses the IDENTITY the invocation was
/// made as, from the same models: `AuthorizationErrorException`'s `awsQueryError` code for
/// Amazon SNS, `AccessDeniedException` for the services that model it under its shape name, and
/// the code Amazon SQS relays a refusal of the key its messages are encrypted under under
/// (`smithy_query(query="shape", shape="KmsAccessDenied")`).
const ACCESS_DENIED_CODES: [&str; 3] = [
    "AuthorizationError",
    "AccessDeniedException",
    "KMS.AccessDeniedException",
];

/// The class a dead letter reports for an answer given under `code`, by this suite's own
/// reading of what each target service publishes — the expectation the implementation is
/// judged against, never read from it.
fn class_of(code: &str) -> &'static str {
    if RESOURCE_ABSENT_CODES.contains(&code) {
        return NO_RESOURCE;
    }
    assert!(
        ACCESS_DENIED_CODES.contains(&code),
        "this suite's rows answer only codes it classifies, got {code:?}"
    );
    NO_PERMISSIONS
}

/// The error code stated inside a reported answer: the `Error Code:` field of the metadata
/// block an AWS error carries, or the `code` of the error metadata where the answer is reported
/// as the SDK's own rendering of the caught error — the two forms a dead letter's
/// `ERROR_MESSAGE` states an answered code in. `None` for a message stating no code at all.
fn answered_code(reported: &str) -> Option<&str> {
    let in_block = reported
        .split_once("Error Code: ")
        .and_then(|(_head, rest)| rest.split_once(';'))
        .map(|(code, _tail)| code.trim());
    in_block.or_else(|| {
        reported
            .split_once("code: Some(\"")
            .and_then(|(_head, rest)| rest.split_once('"'))
            .map(|(code, _tail)| code.trim())
    })
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A grant of the send to this service on `queue_arn` with an explicit `Deny` of the same call
/// beside it, which refuses the delivery whatever is granted.
fn granting_then_denying(queue_arn: &str) -> String {
    document(json!([
        {
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
        {
            "Effect": "Deny",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": SEND_MESSAGE,
            "Resource": queue_arn,
        },
    ]))
}

/// A grant of the send scoped by `aws:SourceArn` to a rule that is not the one publishing, so
/// it authorizes no delivery of this rule.
fn granting_another_rule(queue_arn: &str) -> String {
    document(json!([{
        "Effect": "Allow",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
        "Condition": {"ArnEquals": {"aws:SourceArn": format!(
            "arn:aws:events:{REGION}:{}:rule/some-other-rule",
            ::engine::emulator_account_id()
        )}},
    }]))
}

/// How a row brings the target its rule names about.
enum Setup {
    /// A queue carrying the document the row's function builds from its own ARN, or none at
    /// all when the function is `None`.
    Queue(Option<fn(&str) -> String>),
    /// An Amazon SNS topic on the document that service generates for its owner, which grants
    /// this service nothing.
    TopicGrantingNothing,
    /// A queue ARN of this account and Region that no queue was ever created for.
    NeverCreated,
}

/// One row: the case, how its target is brought about, and the class the dead letter must
/// carry — `None` for the control row, whose delivery goes through.
struct Row {
    case: &'static str,
    setup: Setup,
    class: Option<&'static str>,
}

/// A topic called `name` on the document Amazon SNS generates for its owner; answers its ARN.
/// Nothing is subscribed to it: what this suite reads is the answer the PUBLISH earned, which
/// that topic's own document decides whether or not anybody is listening.
async fn given_topic(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, created) = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status}: {created}"
    );
    xml_text(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"))
}

/// The ARN the row's target names, and the url of its queue when the row's target is one.
async fn given_target(
    emulator: &Emulator,
    case: &str,
    index: usize,
    setup: &Setup,
) -> (String, Option<String>) {
    match setup {
        Setup::Queue(policy) => {
            let policy: Option<&dyn Fn(&str) -> String> = policy
                .as_ref()
                .map(|written| written as &dyn Fn(&str) -> String);
            let (url, arn) =
                given_queue_with_policy(emulator, &format!("answered-target-{index}"), policy).await;
            (arn, Some(url))
        }
        Setup::TopicGrantingNothing => (
            given_topic(emulator, case, &format!("answered-topic-{index}")).await,
            None,
        ),
        Setup::NeverCreated => (
            format!(
                "arn:aws:sqs:{REGION}:{}:answered-never-created-{index}",
                ::engine::emulator_account_id()
            ),
            None,
        ),
    }
}

/// RULE: the `ERROR_CODE` on a dead letter is the class of the error code inside the answer its
/// `ERROR_MESSAGE` reports, whichever service answered and whatever that service answered.
#[tokio::test]
async fn the_dead_letter_class_is_the_class_of_the_answers_own_code() {
    let rows = [
        Row {
            case: "a queue carrying no policy at all",
            setup: Setup::Queue(None),
            class: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue whose policy denies the send beside granting it",
            setup: Setup::Queue(Some(granting_then_denying)),
            class: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue whose grant is scoped by aws:SourceArn to another rule",
            setup: Setup::Queue(Some(granting_another_rule)),
            class: Some(NO_RESOURCE),
        },
        Row {
            case: "a queue ARN no queue was created for",
            setup: Setup::NeverCreated,
            class: Some(NO_RESOURCE),
        },
        Row {
            case: "a topic on the document Amazon SNS generates for its owner",
            setup: Setup::TopicGrantingNothing,
            class: Some(NO_PERMISSIONS),
        },
        Row {
            case: "a queue whose policy grants this service the send",
            setup: Setup::Queue(Some(granting_events)),
            class: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("answered-dlq-{index}")).await;
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

        let parked = drain(&emulator, &dead_letter_url).await;
        let Some(expected) = row.class else {
            assert!(
                parked.is_empty(),
                "case {case:?}: a delivery the policy grants parks nothing, got {parked:?}"
            );
            let url = target_url
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: expected a queue target"));
            let delivered = drain(&emulator, &url).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: expected the granting queue to receive the event, got \
                 {delivered:?}"
            );
            continue;
        };

        if let Some(url) = &target_url {
            let leaked = drain(&emulator, url).await;
            assert!(
                leaked.is_empty(),
                "case {case:?}: a failed delivery leaves the target empty, got {leaked:?}"
            );
        }
        assert_eq!(
            parked.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {parked:?}"
        );
        let message = &parked[0];
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        let code = answered_code(&reported).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the answering service's metadata block to state its \
                 error code, got {reported:?}"
            )
        });
        let class = attribute(message, ERROR_CODE_ATTRIBUTE);
        // The class is the one THAT code belongs to: the pair comes from one answer, so the
        // class cannot be read off anything the reported answer does not state.
        assert_eq!(
            class,
            class_of(code),
            "case {case:?}: expected the class of the answered code {code:?}, got {class:?} in \
             {message:?}"
        );
        // And it is the class this row's target service answers that refusal under, so a row
        // cannot pass on an answer from the wrong service either.
        assert_eq!(
            class, expected,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {expected:?}, got \
             {class:?} in {message:?}"
        );
        // The target that refused the delivery is named on the letter beside the class, which
        // names no target of its own.
        assert_eq!(
            attribute(message, "TARGET_ARN"),
            target_arn,
            "case {case:?}: expected the dead letter to name the target, got {message:?}"
        );
    }
}
