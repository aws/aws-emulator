// SPDX-License-Identifier: CC0-1.0

//! WHICH class a dead letter carries when the TARGET's own resource policy withheld the
//! delivery, across the target services EventBridge reaches that way — end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter` with Amazon SQS and Amazon SNS on the same engine, so
//! each row is read where a client reads one: off the message attributes of the dead
//! letter the target's `DeadLetterConfig` queue received.
//!
//! The RULE asserted here: "for Lambda functions, Amazon SNS topics and Amazon SQS
//! queues, EventBridge relies on resource-based policies" (`PutTargets`), and the class a
//! delivery those documents withheld is dead-lettered under is the one the ANSWER that
//! target's own service gave belongs to — the class of that answer's error code, never a
//! verdict this service reaches about why the call was refused. The two services do not
//! word the refusal the same way, and so do not land in the same class:
//!
//! * Amazon SQS models no authorization failure on `SendMessage` at all
//!   (`smithy_query(query="operation", operation="SendMessage",
//!   model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`):
//!   it does not disclose a queue it will not let the caller write to, so a send the
//!   queue's own document does not permit is answered `QueueDoesNotExist` (HTTP 400,
//!   `awsQueryError` code `AWS.SimpleQueueService.NonExistentQueue`) — the same NOT-FOUND
//!   code that service answers for a queue that is not THERE, under its own sentence for
//!   each of the two. So both are the RESOURCE class, and the SENTENCE is what tells them
//!   apart;
//! * Amazon SNS answers a publish its topic's policy does not permit with
//!   `AuthorizationErrorException` (HTTP 403, `awsQueryError` code
//!   `AuthorizationError`,
//!   `smithy_query(query="shape", shape="AuthorizationErrorException",
//!   model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`)
//!   — an authorization refusal, and so the PERMISSION class.
//!
//! The table's rows therefore differ along the target's SERVICE, along the FORM the
//! withholding takes (no document at all, an explicit `Deny` beside a grant, the
//! document that service generates for the resource's owner) and along whether the
//! target is there at all, each row stating both the class and what the message must
//! name of that service's answer — so a class collapsed for every failed delivery, and
//! one read off this service's own guess at the reason rather than off the answer, both
//! fail here. A control row whose queue DOES grant the
//! send receives the event and parks no dead letter, so no row passes because nothing
//! was delivered anywhere.
//!
//! The harness (the router with EventBridge, Amazon SQS, Amazon SNS, Step Functions,
//! IAM and CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    NO_PERMISSIONS, NO_RESOURCE, SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_queue_with_policy, given_rule, granting_events, publish,
    sns, xml_text, Emulator, QUEUE_ABSENT_SENTENCE, QUEUE_NOT_DISCLOSED_SENTENCE, REGION,
};

/// The rule every row below publishes through.
const RULE: &str = "answer-classified";

/// The id the one withheld target of that rule carries.
const TARGET_ID: &str = "withheld";

/// The dead-letter message attributes this suite reads.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";

/// The IAM action each target kind's delivery performs on its target.
const SEND_MESSAGE: &str = "sqs:SendMessage";
const PUBLISH: &str = "sns:Publish";

/// The error code Amazon SNS reports its refusal of a publish under
/// (`AuthorizationErrorException`'s `awsQueryError` code).
const AUTHORIZATION_ERROR: &str = "AuthorizationError";

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A grant of `action` to this service on `resource`, with an explicit `Deny` of the
/// same call beside it — which refuses the delivery whatever is granted.
fn granting_then_denying(action: &str, resource: &str) -> String {
    document(json!([
        {
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": action,
            "Resource": resource,
        },
        {
            "Effect": "Deny",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": action,
            "Resource": resource,
        },
    ]))
}

/// How a row brings the target its rule names about.
enum Setup {
    /// A queue carrying the document the row's function builds from its own ARN, or
    /// none at all when the function is `None`.
    Queue(Option<fn(&str) -> String>),
    /// A topic carrying `Some` document beside the one Amazon SNS generates for its
    /// owner, or only that generated one when `None`.
    Topic(Option<fn(&str) -> String>),
    /// A queue ARN of this account and Region that no queue was ever created for — the
    /// target that is not there, which earns the other class.
    NeverCreated,
}

/// One row: the case, how its target is brought about, and the answer the dead letter
/// is to carry — what the answering service's own answer states, and the class this
/// service puts that answer in. What it states is that service's error code, or, where
/// one code covers two classes of answer, the sentence that tells them apart. `None`
/// for a row whose delivery goes through.
struct Row {
    case: &'static str,
    setup: Setup,
    expected: Option<(&'static str, &'static str)>,
}

/// A queue whose document denies the send beside granting it.
fn queue_denying(queue_arn: &str) -> String {
    granting_then_denying(SEND_MESSAGE, queue_arn)
}

/// A topic whose document denies the publish beside granting it.
fn topic_denying(topic_arn: &str) -> String {
    granting_then_denying(PUBLISH, topic_arn)
}

/// The string value of the dead letter's message attribute `name`.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A topic called `name` carrying the document `policy` builds from its own ARN beside
/// the one Amazon SNS generates for its owner; answers its ARN. Nothing is subscribed
/// to it: what this suite reads is the answer the PUBLISH earned, which the topic's own
/// document decides whether or not anybody is listening.
async fn given_topic(
    emulator: &Emulator,
    case: &str,
    name: &str,
    policy: Option<fn(&str) -> String>,
) -> String {
    let (status, created) = sns(emulator, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTopic answered {status}: {created}"
    );
    let arn = xml_text(&created, "TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: no TopicArn in {created}"));
    if let Some(policy) = policy {
        let written = policy(&arn);
        let (status, answered) = sns(
            emulator,
            "SetTopicAttributes",
            &[
                ("TopicArn", &arn),
                ("AttributeName", "Policy"),
                ("AttributeValue", &written),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SetTopicAttributes answered {status}: {answered}"
        );
    }
    arn
}

/// The ARN the row's target names, and the url of its queue when the row's target is
/// one — so a row that expected a delivery can read it where it landed.
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
                given_queue_with_policy(emulator, &format!("classified-target-{index}"), policy)
                    .await;
            (arn, Some(url))
        }
        Setup::Topic(policy) => (
            given_topic(emulator, case, &format!("classified-topic-{index}"), *policy).await,
            None,
        ),
        Setup::NeverCreated => (
            format!(
                "arn:aws:sqs:{REGION}:{}:classified-never-created-{index}",
                ::engine::emulator_account_id()
            ),
            None,
        ),
    }
}

/// The class a withheld delivery is dead-lettered under is the class of the CODE the
/// TARGET's own service answered with: the resource one for the not-found code Amazon SQS
/// answers a send it will not disclose the queue for, the permission one for the
/// authorization code Amazon SNS answers a publish its topic withheld.
#[tokio::test]
async fn a_withheld_deliverys_class_is_the_one_the_answering_services_code_belongs_to() {
    let rows = [
        Row {
            case: "a queue carrying no policy at all",
            setup: Setup::Queue(None),
            expected: Some((QUEUE_NOT_DISCLOSED_SENTENCE, NO_RESOURCE)),
        },
        Row {
            case: "a queue whose policy denies the send beside granting it",
            setup: Setup::Queue(Some(queue_denying)),
            expected: Some((QUEUE_NOT_DISCLOSED_SENTENCE, NO_RESOURCE)),
        },
        Row {
            case: "a queue ARN no queue was created for",
            setup: Setup::NeverCreated,
            expected: Some((QUEUE_ABSENT_SENTENCE, NO_RESOURCE)),
        },
        Row {
            case: "a topic on the document Amazon SNS generates for its owner",
            setup: Setup::Topic(None),
            expected: Some((AUTHORIZATION_ERROR, NO_PERMISSIONS)),
        },
        Row {
            case: "a topic whose policy denies the publish beside granting it",
            setup: Setup::Topic(Some(topic_denying)),
            expected: Some((AUTHORIZATION_ERROR, NO_PERMISSIONS)),
        },
        Row {
            case: "a queue whose policy grants this service the send",
            setup: Setup::Queue(Some(granting_events)),
            expected: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let (dead_letter_url, dead_letter_arn) =
            given_queue(&emulator, &format!("classified-dlq-{index}")).await;
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

        let dead_lettered = drain(&emulator, &dead_letter_url).await;
        let Some((states, class)) = row.expected else {
            assert!(
                dead_lettered.is_empty(),
                "case {case:?}: a delivery that went through is not dead-lettered, got \
                 {dead_lettered:?}"
            );
            let url = target_url
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: expected a queue target"));
            let delivered = drain(&emulator, &url).await;
            assert_eq!(
                delivered.len(),
                1,
                "case {case:?}: expected the granted queue to receive the event, got \
                 {delivered:?}"
            );
            continue;
        };

        if let Some(url) = &target_url {
            let delivered = drain(&emulator, url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a withheld delivery leaves the target empty, got {delivered:?}"
            );
        }
        assert_eq!(
            dead_lettered.len(),
            1,
            "case {case:?}: expected exactly one dead letter, got {dead_lettered:?}"
        );
        let message = &dead_lettered[0];
        let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
        assert!(
            reported.contains(states),
            "case {case:?}: expected the answering service's own answer to state {states:?}, got \
             {reported:?}"
        );
        let reported_class = attribute(message, ERROR_CODE_ATTRIBUTE);
        assert_eq!(
            reported_class, class,
            "case {case:?}: expected the dead-letter {ERROR_CODE_ATTRIBUTE} {class:?} for the \
             answer {reported:?}, got {reported_class:?}"
        );
        // The class names no target, so the letter names the one whose delivery failed.
        assert_eq!(
            attribute(message, TARGET_ARN_ATTRIBUTE),
            target_arn,
            "case {case:?}: expected the dead letter to name the target, got {message:?}"
        );
    }
}
