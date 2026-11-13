// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A CALL THAT CHANGES AN EXISTING QUEUE ANSWERS WHEN IT SUPPLIES
//! AN ATTRIBUTE ONLY A CREATE MAY SUPPLY, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<Operation>`) through the shared harness in [`super::support`].
//!
//! The rule under test: **which refusal such an attribute draws follows whether the queue the
//! call addresses HAS it, exactly as it does for every attribute belonging to the FIFO kind
//! of queue.**
//!
//! * On a queue of the kind the attribute designates, the queue carries it and reports it, so
//!   what the call asked for is the CHANGE: `InvalidAttributeValue` at HTTP 400 (legacy query
//!   code `InvalidAttributeValue` with the `Sender` fault) and the sentence `Invalid value for
//!   the parameter <Attribute>. Reason: Modifying queue type is not supported.`
//! * On a queue of any other kind the attribute is not one that queue has at all — a standard
//!   queue reports no attribute saying it is FIFO — so the NAME is refused, in the one
//!   sentence every name outside the addressed queue's vocabulary draws:
//!   `InvalidAttributeName` at HTTP 400 (legacy query code `InvalidAttributeName` with the
//!   `Sender` fault) and `Unknown Attribute <Attribute>.`
//!
//! The attribute is named exactly as the request's attribute map spelled it, whatever value
//! the call supplied for it and whether or not that value matches what the queue already is,
//! and nothing about the queue changes either way.
//!
//! A client branching on the modelled class is therefore told what AWS tells it: drop an
//! attribute the queue does not have, and stop trying to change one it does. Mapping both to
//! one class loses one of those two instructions.
//!
//! The rows span what the rule generalises over rather than the one recorded request: the
//! KIND of queue the call addresses (standard and FIFO), the VALUE supplied (the designating
//! one — which asks a FIFO queue for no change at all — its opposite, other casings, numeric
//! spellings, a padded one, no text, and a word outside the attribute's vocabulary), and
//! whether the attribute arrives alone or beside an attribute the call would otherwise
//! accept. Each row is asserted over the answer AND over the queue it did not change, and
//! the control rows pin that the create that may supply the attribute still does, that a
//! value outside what an attribute the queue HAS publishes still draws the value class on its
//! own terms, and that the update path still works on the queue the rows refused.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

use super::support::{
    all_attributes, call, call_in_query_mode, create, create_ok, emulator, error_code, ok,
    QUERY_ERROR_HEADER,
};

/// The attribute only a create may supply: "You can provide this attribute only during queue
/// creation. You can't change it for an existing queue" (`FifoQueue`'s own documentation in
/// `smithy_query(query="operation",operation="CreateQueue",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
/// Spelled out here rather than imported, so a wrong constant in the service cannot make a
/// row pass.
const CREATE_ONLY_ATTRIBUTE: &str = "FifoQueue";

/// The one value of that attribute that designates a queue as FIFO.
const DESIGNATES: &str = "true";

/// The refusal class an attribute VALUE the call does not accept carries, and its status.
const INVALID_ATTRIBUTE_VALUE: (u16, &str) = (400, "InvalidAttributeValue");

/// The refusal class a name the queue the call addresses does not have carries, and its
/// status: the sibling of the class above, which this suite's rows hold apart by the kind of
/// queue the call was made against.
const INVALID_ATTRIBUTE_NAME: (u16, &str) = (400, "InvalidAttributeName");

/// An attribute every queue has, with a value outside the range its own documentation
/// publishes ("Valid values: An integer from 0 to 43,200 (12 hours)").
const OUT_OF_RANGE: (&str, &str) = ("VisibilityTimeout", "43201");

/// The fault side the legacy query report names for a refusal the caller caused.
const SENDER_FAULT: &str = "Sender";

/// The one sentence Amazon SQS refuses a change to the queue's type with, spelled out here
/// rather than imported: the fixed prefix, the fixed separator and the fixed `Reason:`
/// sentence, with the attribute name the request supplied as the only variable part.
fn not_modifiable_message(attribute: &str) -> String {
    format!("Invalid value for the parameter {attribute}. Reason: Modifying queue type is not supported.")
}

/// The one sentence it refuses an attribute name the addressed queue does not have with,
/// spelled out the same way.
fn unknown_attribute_message(attribute: &str) -> String {
    format!("Unknown Attribute {attribute}.")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Every value a call supplies the create-only attribute with, spanning the forms a client
/// writes one in: the value that designates, its opposite, other casings and spellings, and
/// text the attribute has no meaning for.
const SUPPLIED_VALUES: &[(&str, &str)] = &[
    ("the designating value", DESIGNATES),
    ("the opposite boolean", "false"),
    ("the designating word upper-cased", "TRUE"),
    ("the designating word capitalised", "True"),
    ("a numeric spelling", "1"),
    ("the opposite numeric spelling", "0"),
    ("a padded designating word", " true"),
    ("no text at all", ""),
    ("a word outside the attribute's vocabulary", "yes"),
];

/// The attribute maps an update supplies the create-only attribute in: alone, and beside an
/// attribute the call would accept on its own — sorted before and sorted after it, so the row
/// pins the refusal whichever order the names are visited in.
fn maps_for(value: &str) -> Vec<(String, Value)> {
    vec![
        (
            "alone".to_string(),
            json!({ CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string() }),
        ),
        (
            "beside an attribute sorted before it".to_string(),
            json!({
                CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string(),
                "DelaySeconds": "5",
            }),
        ),
        (
            "beside an attribute sorted after it".to_string(),
            json!({
                CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string(),
                "VisibilityTimeout": "77",
            }),
        ),
    ]
}

/// What a queue of each kind answers a call naming the create-only attribute with: the class,
/// its status and its sentence, which is what the kind of queue decides.
fn expected_for(fifo: bool) -> (u16, &'static str, String) {
    if fifo {
        // The queue HAS the attribute: the change its value asks for is what is refused.
        (
            INVALID_ATTRIBUTE_VALUE.0,
            INVALID_ATTRIBUTE_VALUE.1,
            not_modifiable_message(CREATE_ONLY_ATTRIBUTE),
        )
    } else {
        // The queue does not have the attribute at all: the name is refused.
        (
            INVALID_ATTRIBUTE_NAME.0,
            INVALID_ATTRIBUTE_NAME.1,
            unknown_attribute_message(CREATE_ONLY_ATTRIBUTE),
        )
    }
}

/// The queue each row addresses: a standard one and a FIFO one, with what the queue reports
/// before any row ran.
async fn queues(
    router: &MultiServiceRouter,
) -> Vec<(&'static str, bool, String, BTreeMap<String, String>)> {
    let standard = create_ok(router, "immutable-standard", &json!({})).await;
    let fifo = create_ok(
        router,
        "immutable-queue.fifo",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): DESIGNATES }),
    )
    .await;
    let before_standard = all_attributes(router, &standard).await;
    let before_fifo = all_attributes(router, &fifo).await;
    vec![
        ("a standard queue", false, standard, before_standard),
        ("a FIFO queue", true, fifo, before_fifo),
    ]
}

/// The rule: whatever the value, the update is refused in the class the kind of queue it
/// addresses decides, and the queue is left exactly as it was.
#[tokio::test]
async fn an_update_naming_the_create_only_attribute_is_refused_in_the_class_its_queue_decides() {
    let router = emulator();
    for (kind, fifo, url, before) in queues(&router).await {
        let expected = expected_for(fifo);
        for (value_case, value) in SUPPLIED_VALUES {
            for (map_case, attributes) in maps_for(value) {
                let case = format!("{kind} with {value_case} ({value:?}) {map_case}");
                let (status, parsed) = call(
                    &router,
                    "SetQueueAttributes",
                    &json!({"QueueUrl": url, "Attributes": attributes}),
                )
                .await;
                assert_eq!(
                    (status, error_code(&parsed).as_str(), message_of(&parsed)),
                    (expected.0, expected.1, expected.2.clone()),
                    "case {case}: expected HTTP {} {} stating {:?}, got {status} with {parsed}",
                    expected.0,
                    expected.1,
                    expected.2
                );

                // The refusal is taken before anything is written: the attribute beside it is
                // not stored, the queue's type is what its create made it, and the record's
                // own timestamps did not move.
                let after = all_attributes(&router, &url).await;
                assert_eq!(
                    after, before,
                    "case {case}: a refused update must leave the queue exactly as it was, \
                     got {after:?}"
                );
            }
        }
    }
}

/// The same refusals as a query-compatible SDK sees them: the legacy code of the class the
/// queue's kind decides, beside the caller's fault, and the same class in the body's
/// discriminator.
#[tokio::test]
async fn the_refusal_reports_its_class_to_a_query_compatible_client() {
    let router = emulator();
    for (kind, fifo, url, _) in queues(&router).await {
        let expected = expected_for(fifo);
        for (value_case, value) in [("the designating value", DESIGNATES), ("its opposite", "false")]
        {
            let case = format!("{kind} with {value_case} ({value:?})");
            let (status, headers, parsed) = call_in_query_mode(
                &router,
                "SetQueueAttributes",
                &json!({
                    "QueueUrl": url,
                    "Attributes": { CREATE_ONLY_ATTRIBUTE.to_string(): value.to_string() },
                }),
            )
            .await;
            let report = headers
                .get(QUERY_ERROR_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string);
            let report_expected = format!("{};{SENDER_FAULT}", expected.1);
            assert_eq!(
                (status, error_code(&parsed).as_str(), report.clone()),
                (expected.0, expected.1, Some(report_expected.clone())),
                "case {case}: expected HTTP {} with the {} class and {QUERY_ERROR_HEADER} \
                 {report_expected:?}, got {status} with {parsed} and {report:?}",
                expected.0,
                expected.1
            );
        }
    }
}

/// The controls, which is what keeps the rule from being "refuse everything": the create that
/// MAY supply the attribute still does, the queue it designates still reports itself as FIFO,
/// a standard queue a refused update addressed still reports no such attribute, another
/// attribute of the same update path is still accepted on the queue the rows refused, and a
/// value outside what an attribute every queue HAS publishes still draws the value class on
/// its own terms.
#[tokio::test]
async fn the_attribute_is_still_accepted_at_create_and_the_sibling_classes_are_unmoved() {
    let router = emulator();

    // The create may supply it, and the queue reads back as the type it designated.
    let url = create_ok(
        &router,
        "immutable-created.fifo",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): DESIGNATES }),
    )
    .await;
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get(CREATE_ONLY_ATTRIBUTE).map(String::as_str),
        Some(DESIGNATES),
        "case the create that may supply the attribute: expected the queue to report itself as \
         FIFO, got {reported:?}"
    );

    // A create naming a value that designates nothing asks for a standard queue, which has no
    // such attribute — so the name class covers it, at the create as at the update.
    let (status, parsed) = create(
        &router,
        "immutable-not-designated",
        &json!({ CREATE_ONLY_ATTRIBUTE.to_string(): "false" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str(), message_of(&parsed)),
        (
            INVALID_ATTRIBUTE_NAME.0,
            INVALID_ATTRIBUTE_NAME.1,
            unknown_attribute_message(CREATE_ONLY_ATTRIBUTE)
        ),
        "case a create supplying a value that designates nothing: expected the name class, got \
         {status} with {parsed}"
    );

    // A standard queue a refused update addressed carries no such attribute afterwards, and
    // the update path itself still works on it.
    let standard = create_ok(&router, "immutable-control", &json!({})).await;
    let (status, parsed) = call(
        &router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": standard,
            "Attributes": { CREATE_ONLY_ATTRIBUTE.to_string(): DESIGNATES },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (INVALID_ATTRIBUTE_NAME.0, INVALID_ATTRIBUTE_NAME.1),
        "case a standard queue the update addressed: expected the name class, got {status} with \
         {parsed}"
    );
    let reported = all_attributes(&router, &standard).await;
    assert_eq!(
        reported.get(CREATE_ONLY_ATTRIBUTE),
        None,
        "case a standard queue the update addressed: the refused attribute must not be reported, \
         got {reported:?}"
    );
    let _ = ok(
        &router,
        "SetQueueAttributes",
        json!({"QueueUrl": standard, "Attributes": {"VisibilityTimeout": "45"}}),
    )
    .await;
    let reported = all_attributes(&router, &standard).await;
    assert_eq!(
        reported.get("VisibilityTimeout").map(String::as_str),
        Some("45"),
        "case an attribute the update accepts: expected the set value to be reported, got \
         {reported:?}"
    );

    // And a value outside what an attribute every queue HAS publishes is refused as a value
    // naming that attribute alone — the class the queue's own attribute draws, without the
    // type rule's `Reason:` clause.
    for (kind, url) in [("a FIFO queue", &url), ("a standard queue", &standard)] {
        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({
                "QueueUrl": url,
                "Attributes": { OUT_OF_RANGE.0.to_string(): OUT_OF_RANGE.1.to_string() },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message_of(&parsed)),
            (
                INVALID_ATTRIBUTE_VALUE.0,
                INVALID_ATTRIBUTE_VALUE.1,
                format!("Invalid value for the parameter {}.", OUT_OF_RANGE.0)
            ),
            "case {kind} with a value outside the attribute's published range: expected the \
             value class, got {status} with {parsed}"
        );
    }
}
