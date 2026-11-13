// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW A QUEUE ATTRIBUTE NAME A CALL CANNOT ACCEPT IS REFUSED, driven
//! over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared
//! harness in [`super::support`].
//!
//! The rule under test: **an attribute name a call cannot accept is refused as an UNKNOWN
//! attribute — error code `InvalidAttributeName` at HTTP 400 with the message `Unknown
//! Attribute <name>.`, the name exactly as the request spelled it — whatever puts the name
//! out of that call's reach: a name the model declares nowhere, the selector that stands for
//! every attribute, an attribute the service only ever reports, or one that applies only to
//! the other kind of queue. Nothing about the queue changes.**
//!
//! The set of names a call accepts depends on the call and on the queue it addresses, so a
//! name outside that set is simply not an attribute THAT call has — which is why one sentence
//! covers every way of being outside it, and why a client classifying a refusal by its
//! message is not told that an attribute exists-but-is-read-only.
//!
//! The attribute only a create may supply is inside this rule for the queue that does not
//! HAVE it and outside it for the queue that does: a standard queue carries no attribute
//! saying it is FIFO, so a name refusal covers it there, while a FIFO queue carries and
//! reports it, so a later call naming it is refused for the change it asks for —
//! [`super::queue_type_immutability`] owns that half of the rule, and the rows here assert
//! which of the two each kind of queue draws.
//!
//! The table spans the class the rule generalises over rather than one example: every name
//! the model's `QueueAttributeName` enum declares is driven against a standard queue and a
//! FIFO queue, through `SetQueueAttributes` and `CreateQueue`, with a name the enum declares
//! nowhere alongside — each row asserting the refusal for a name that call cannot accept and
//! acceptance for one it can, so a rule that refused everything would fail the table too.

use serde_json::{json, Value};

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, expected_arn, fifo_attributes,
};

/// Every name the `QueueAttributeName` enum declares, in the order the model lists them.
///
/// Source: `smithy_query(query="shape",shape="com.amazonaws.sqs#QueueAttributeName",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`.
/// Spelled out here rather than imported, so a name the service forgot cannot make a row
/// pass.
const QUEUE_ATTRIBUTE_NAMES: &[&str] = &[
    "All",
    "Policy",
    "VisibilityTimeout",
    "MaximumMessageSize",
    "MessageRetentionPeriod",
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
    "CreatedTimestamp",
    "LastModifiedTimestamp",
    "QueueArn",
    "ApproximateNumberOfMessagesDelayed",
    "DelaySeconds",
    "ReceiveMessageWaitTimeSeconds",
    "RedrivePolicy",
    "FifoQueue",
    "ContentBasedDeduplication",
    "KmsMasterKeyId",
    "KmsDataKeyReusePeriodSeconds",
    "DeduplicationScope",
    "FifoThroughputLimit",
    "RedriveAllowPolicy",
    "SqsManagedSseEnabled",
];

/// A name the enum declares nowhere, so no call accepts it under any circumstances.
const UNDECLARED_NAME: &str = "NotAnAttribute";

/// The attributes that apply only to FIFO queues, so a standard queue has none of them:
/// "The following attributes apply only to FIFO (first-in-first-out) queues" and "The
/// following attributes apply only to high throughput for FIFO queues" (the `Attributes`
/// member's own documentation).
const FIFO_ONLY_NAMES: &[&str] = &[
    "ContentBasedDeduplication",
    "DeduplicationScope",
    "FifoThroughputLimit",
];

/// The attribute only a create may supply: "You can provide this attribute only during queue
/// creation. You can't change it for an existing queue" (`FifoQueue`'s own documentation).
const CREATE_ONLY_NAME: &str = "FifoQueue";

/// The attributes the service derives and reports but no call sets — the queue's identity and
/// its timestamps, and the three message counters.
const REPORTED_ONLY_NAMES: &[&str] = &[
    "QueueArn",
    "CreatedTimestamp",
    "LastModifiedTimestamp",
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesDelayed",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The refusal class every unacceptable attribute NAME carries.
const INVALID_ATTRIBUTE_NAME: &str = "InvalidAttributeName";

/// The refusal class an attribute the queue the call addresses HAS but no later call may
/// CHANGE carries instead — the sibling class, held apart here by the kind of queue and
/// asserted in full by [`super::queue_type_immutability`].
const INVALID_ATTRIBUTE_VALUE: &str = "InvalidAttributeValue";

/// The one sentence Amazon SQS refuses an unacceptable attribute name with, spelled out here
/// rather than imported.
fn unknown_attribute_message(name: &str) -> String {
    format!("Unknown Attribute {name}.")
}

/// The one sentence it refuses a change to the queue's type with, spelled out the same way.
fn not_modifiable_message(name: &str) -> String {
    format!("Invalid value for the parameter {name}. Reason: Modifying queue type is not supported.")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The dead-letter queue a row's `RedrivePolicy` names, of the kind the row is about: "the
/// dead-letter queue of a FIFO queue must also be a FIFO queue".
fn dead_letter_name(fifo: bool) -> String {
    format!("names-dlq{}", if fifo { ".fifo" } else { "" })
}

/// A value of the right FORM for `name`, so a row is refused for the name it supplied and
/// never for the value beside it.
fn value_for(name: &str, fifo: bool) -> String {
    match name {
        "Policy" => "{}".to_string(),
        "RedrivePolicy" => json!({
            "deadLetterTargetArn": expected_arn(&dead_letter_name(fifo)),
            "maxReceiveCount": 5,
        })
        .to_string(),
        "RedriveAllowPolicy" => r#"{"redrivePermission":"denyAll"}"#.to_string(),
        "KmsMasterKeyId" => "alias/aws/sqs".to_string(),
        "KmsDataKeyReusePeriodSeconds" => "300".to_string(),
        "DeduplicationScope" => "messageGroup".to_string(),
        "FifoThroughputLimit" => "perQueue".to_string(),
        "ContentBasedDeduplication" | "FifoQueue" | "SqsManagedSseEnabled" => "true".to_string(),
        "MaximumMessageSize" => "2048".to_string(),
        "MessageRetentionPeriod" => "120".to_string(),
        // Every remaining name is either an integer attribute whose range holds 1, or one a
        // call does not accept at all — where the value is never reached.
        _ => "1".to_string(),
    }
}

/// Whether `SetQueueAttributes` on a queue of this kind accepts `name` at all: the settable
/// attributes, minus the one only a create may supply and minus the ones that belong to the
/// other kind of queue.
fn settable_on(name: &str, fifo: bool) -> bool {
    if name == "All" || name == UNDECLARED_NAME || name == CREATE_ONLY_NAME {
        return false;
    }
    if REPORTED_ONLY_NAMES.contains(&name) {
        return false;
    }
    if FIFO_ONLY_NAMES.contains(&name) {
        return fifo;
    }
    true
}

/// `SetQueueAttributes` answers every name the queue it addresses does not have with the one
/// unknown-attribute sentence, on a standard queue and on a FIFO queue, and leaves the queue
/// untouched — the create-only attribute being refused as a name on the queue that does not
/// carry it and for the change it asks for on the queue that does.
#[tokio::test]
async fn set_queue_attributes_refuses_every_name_it_cannot_accept_as_an_unknown_attribute() {
    for fifo in [false, true] {
        let router = emulator();
        let suffix = if fifo { ".fifo" } else { "" };
        let attributes = if fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        let url = create_ok(&router, &format!("names{suffix}"), &attributes).await;
        // The dead-letter queue the `RedrivePolicy` row names, of this row's own kind.
        create_ok(&router, &dead_letter_name(fifo), &attributes).await;

        for name in QUEUE_ATTRIBUTE_NAMES.iter().chain([&UNDECLARED_NAME]) {
            let value = value_for(name, fifo);
            // The queue as it stands going INTO this call, which is what a refused call has to
            // leave untouched: the rows before this one set the names they could, and an
            // accepted set is a change to the queue, so the comparison below is against the
            // state this call found rather than against the queue as it was created.
            let before = all_attributes(&router, &url).await;
            let (status, parsed) = call(
                &router,
                "SetQueueAttributes",
                &json!({"QueueUrl": url, "Attributes": {name.to_string(): value}}),
            )
            .await;
            let kind = if fifo { "a FIFO queue" } else { "a standard queue" };
            if settable_on(name, fifo) {
                assert_eq!(
                    status, 200,
                    "case {name} on {kind}: expected the attribute accepted, got {status} with \
                     {parsed}"
                );
                continue;
            }
            // The create-only attribute is one a FIFO queue HAS, so there a call that may
            // not change it is refused for the change its value asks for rather than as a
            // name; a standard queue does not carry it at all, so there it is a name like
            // any other the queue does not have. The two classes are held apart here and the
            // rule behind the second is asserted in full by the sibling suite.
            let expected = if *name == CREATE_ONLY_NAME && fifo {
                (400, INVALID_ATTRIBUTE_VALUE, not_modifiable_message(name))
            } else {
                (400, INVALID_ATTRIBUTE_NAME, unknown_attribute_message(name))
            };
            assert_eq!(
                (status, error_code(&parsed).as_str(), message_of(&parsed)),
                expected,
                "case {name} on {kind}: expected {expected:?}, got {status} with {parsed}"
            );
            let after = all_attributes(&router, &url).await;
            assert_eq!(
                after.get(*name),
                before.get(*name),
                "case {name} on {kind}: a refused name must change nothing about the queue"
            );
        }
    }
}

/// `CreateQueue` judges a supplied name the same way, for the queue the request asks for: the
/// create-only attribute is accepted there and the FIFO-only ones only for a FIFO queue, and
/// every other unacceptable name draws the same sentence and leaves no queue behind.
#[tokio::test]
async fn create_queue_refuses_every_name_it_cannot_accept_as_an_unknown_attribute() {
    for fifo in [false, true] {
        let router = emulator();
        let kind = if fifo { "a FIFO queue" } else { "a standard queue" };
        let dead_letter_attributes = if fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        create_ok(&router, &dead_letter_name(fifo), &dead_letter_attributes).await;
        for (index, name) in QUEUE_ATTRIBUTE_NAMES
            .iter()
            .chain([&UNDECLARED_NAME])
            .enumerate()
        {
            // The create under test supplies the row's name beside the one attribute that
            // makes the queue the kind the row is about, so a FIFO row is judged as a FIFO
            // queue.
            let mut attributes = if fifo {
                fifo_attributes(&[])
            } else {
                json!({})
            };
            attributes[name.to_string()] = json!(value_for(name, fifo));
            // `FifoQueue` is the one attribute only a create accepts, and a queue it
            // designates as FIFO carries the suffix a FIFO queue's name must end with — so
            // the row that supplies it is named accordingly whichever kind it started from,
            // and is judged on the attribute rather than on the name.
            let designates_fifo = fifo || *name == CREATE_ONLY_NAME;
            let suffix = if designates_fifo { ".fifo" } else { "" };
            let queue = format!("created-{}-{index}{suffix}", if fifo { "f" } else { "s" });
            let (status, parsed) = create(&router, &queue, &attributes).await;
            // A create accepts everything `SetQueueAttributes` does, plus the one attribute
            // that may only be supplied at creation.
            let accepted = settable_on(name, fifo) || *name == CREATE_ONLY_NAME;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {name} on {kind}: expected the attribute accepted, got {status} with \
                     {parsed}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&parsed).as_str(), message_of(&parsed)),
                (
                    400,
                    INVALID_ATTRIBUTE_NAME,
                    unknown_attribute_message(name)
                ),
                "case {name} on {kind}: expected the unknown-attribute sentence, got {status} \
                 with {parsed}"
            );
            let (status, parsed) = call(&router, "GetQueueUrl", &json!({"QueueName": queue})).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueDoesNotExist"),
                "case {name} on {kind}: a refused create must leave no queue behind, got \
                 {status} with {parsed}"
            );
        }
    }
}
