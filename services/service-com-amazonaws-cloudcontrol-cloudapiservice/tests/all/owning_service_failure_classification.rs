// SPDX-License-Identifier: CC0-1.0

//! WHICH `HandlerErrorCode` a resource operation reports a refusal under.
//!
//! The rule every case here holds the service to: **a refusal the resource type's OWNING
//! service raised that the model's own values do not settle — a name it holds to a charset of
//! its own or keeps for itself, a name and a flag that contradict each other, an attribute no
//! queue of that kind has — is reported under the handler code for a downstream error,
//! `GeneralServiceException`, because that is the code for "the downstream service generated
//! an error that doesn't map to any other handler error code"
//! (`smithy_query(query="doc",shape="GeneralServiceException")`), and its `StatusMessage` is
//! what THAT type's handler states about such an error — for a type whose handler carries its
//! client's rendering, the owning service's own diagnosis of the refused call
//! (`relayed_downstream_diagnosis`), and for one that states a sentence of its own, that
//! sentence about the CALL it was making (`downstream_refusal_reporting`); `InvalidRequest`,
//! which is the code for "invalid input from the user has generated a generic exception"
//! (`smithy_query(query="doc",shape="InvalidRequestException")`), is the code for a fault in
//! the model against what the type's schema publishes — a property that schema declares which
//! this handler does not apply, and a value outside the domain it publishes for the property
//! carrying it (`property_value_domain_refusals`) — and is stated in the sentence naming what
//! is at fault.**
//!
//! A fault the SCHEMA itself states — a property it does not declare, one it declares
//! `required` that the model leaves out, a value outside a constraint or the JSON type it
//! declares — is not a case here at all: the front door refuses such a call before a handler
//! is asked (`front_door_model_validation`).
//!
//! The rows differ along what the rule generalises over: which owning service answers for the
//! type, which kind of rule the model breaks (a name's charset, a name and a flag that
//! contradict each other, an attribute that does not apply), and
//! whether the refusal arises on a create or on an update. The controls are the refusals that
//! DO map to a verdict of their own — a resource already there, one that is not there — which
//! keep those codes, and the consistent model of each refused pair, which is accepted.

use serde_json::{json, Value};

use super::emulator::{progress, text, Emulator, STATUS_FAILED, STATUS_SUCCESS};
use super::sqs_queue_read_through::{queue_url, QUEUE};

/// The handler code for an error the OWNING service raised that the type's handler maps to
/// no other verdict.
const DOWNSTREAM: &str = "GeneralServiceException";

/// The sentence such a refusal carries on the CREATE path of `AWS::SQS::Queue`: this type's
/// handler carries its Amazon SQS client's rendering of the refusal, so the diagnosis is that
/// service's own — here, the rule it holds a FIFO queue's name to
/// (`relayed_downstream_diagnosis` holds the whole rendering to its groups).
const DOWNSTREAM_FIFO_NAME: &str = "The name of a FIFO queue can only include";

/// The same for a name that is no queue name of either kind.
const DOWNSTREAM_QUEUE_NAME: &str = "Can only include alphanumeric characters";

/// The same on the UPDATE path, where that service answers for an attribute the queue the
/// write addresses does not have.
const DOWNSTREAM_UNKNOWN_ATTRIBUTE: &str = "Unknown Attribute ContentBasedDeduplication.";

/// The handler code for a fault the handler itself judges in the model.
const MODEL_FAULT: &str = "InvalidRequest";

/// A type whose resources live in the Amazon SNS service's own store.
const TOPIC: &str = "AWS::SNS::Topic";

/// A type whose resources live in the Amazon S3 service's own store.
const BUCKET: &str = "AWS::S3::Bucket";

/// A type whose resources live in the AWS Systems Manager service's own store.
const PARAMETER: &str = "AWS::SSM::Parameter";

/// Drive a create of `type_name` that must FAIL, and answer the handler code and the
/// diagnostic the terminal event reports.
async fn failed_create(
    emulator: &Emulator,
    type_name: &str,
    desired: &Value,
    case: &str,
) -> (String, String) {
    let accepted = emulator.create_ok(type_name, desired, None).await;
    let settled = emulator.terminal(&text(&accepted, "RequestToken")).await;
    assert_eq!(
        text(&settled, "OperationStatus"),
        STATUS_FAILED,
        "case {case:?}: a model the owning service will not take must not succeed, got \
         {settled:?}"
    );
    assert!(
        !text(&settled, "StatusMessage").is_empty(),
        "case {case:?}: a failed request must state a diagnostic, got {settled:?}"
    );
    (
        text(&settled, "ErrorCode"),
        text(&settled, "StatusMessage"),
    )
}

#[tokio::test]
async fn a_create_the_owning_service_refuses_carries_the_downstream_handler_code() {
    let emulator = Emulator::with_sqs();
    // The rows differ along the kind of rule the model breaks. The first three are rules the
    // model's own values do not settle — the FIFO suffix a name must or must not carry, the
    // character set a queue name admits — and the last two are properties the type's schema
    // does not declare, which this type's handler judges itself: Amazon SQS has no row in the
    // model-validation catalogue, so its schema is read where its model is
    // ([`crate::state::native`]). The diagnostic is the sentence whichever of the two worded
    // it, which every row pins. A value outside the domain the schema publishes for its
    // property is the third kind, and has its own suite
    // (`property_value_domain_refusals`).
    let rows: [(&str, Value, &str, &str); 5] = [
        (
            "a name ending in the FIFO suffix on a queue that is not one",
            json!({ "QueueName": "classified-standard.fifo", "FifoQueue": false }),
            DOWNSTREAM,
            // The diagnosis the owning service worded, which this type's handler relays.
            DOWNSTREAM_QUEUE_NAME,
        ),
        (
            "a FIFO queue whose name lacks the suffix",
            json!({ "QueueName": "classified-fifo", "FifoQueue": true }),
            DOWNSTREAM,
            DOWNSTREAM_FIFO_NAME,
        ),
        (
            "a name carrying a character the owning service's charset excludes",
            json!({ "QueueName": "classified queue!" }),
            DOWNSTREAM,
            DOWNSTREAM_QUEUE_NAME,
        ),
        (
            "a property the type's schema does not declare",
            json!({ "QueueName": "classified-stranger", "Flavour": "vanilla" }),
            MODEL_FAULT,
            "Flavour is not a property of AWS::SQS::Queue",
        ),
        (
            "an attribute of the queue the type's schema declares no property for",
            // A queue's access policy is carried by resource types of its own
            // (`AWS::SQS::QueuePolicy`, `AWS::SQS::QueueInlinePolicy`), so this type's
            // schema declares no `Policy`: the model names a property the schema does not
            // declare, which is the handler's own judgement rather than anything Amazon SQS
            // said about the queue.
            json!({ "QueueName": "classified-policy", "Policy": {} }),
            MODEL_FAULT,
            "Policy is not a property of AWS::SQS::Queue",
        ),
    ];

    for (case, desired, expected_code, expected_detail) in rows {
        let (code, detail) = failed_create(&emulator, QUEUE, &desired, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code} handler code, got {code:?} with \
             {detail:?}"
        );
        // A refusal the handler judged states the handler's own diagnostic; one the owning
        // service raised states that service's own diagnosis of the call it refused.
        assert!(
            detail.contains(expected_detail),
            "case {case:?}: expected a diagnostic carrying {expected_detail:?}, got {detail:?}"
        );
        // Nothing was created, in either store: a refused create leaves no resource behind.
        let name = text(&desired, "QueueName");
        let (status, body) = emulator.get(QUEUE, &queue_url(&name)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must leave no resource, got {status} with body \
             {body:?}"
        );
    }

    // The controls. A refusal that maps to a verdict of its OWN keeps that verdict rather
    // than the downstream catch-all: a resource the owning service already holds, and one it
    // does not hold at all.
    let held = "classified-held";
    emulator
        .provision(QUEUE, &json!({ "QueueName": held }))
        .await;
    let (code, detail) = failed_create(
        &emulator,
        QUEUE,
        &json!({ "QueueName": held }),
        "a queue the owning service already holds",
    )
    .await;
    assert_eq!(
        code, "AlreadyExists",
        "a queue the owning service already holds: expected AlreadyExists, got {code:?} with \
         {detail:?}"
    );
    let (status, accepted) = emulator
        .delete(QUEUE, &queue_url("classified-absent"))
        .await;
    assert_eq!(
        status, 200,
        "a delete of a queue the owning service does not hold must be accepted, got {status} \
         with body {accepted:?}"
    );
    let gone = emulator
        .terminal(&text(&progress(&accepted), "RequestToken"))
        .await;
    assert_eq!(
        text(&gone, "ErrorCode"),
        "NotFound",
        "a queue the owning service does not hold: expected NotFound, got {gone:?}"
    );

    // And the consistent model of the first pair of rows is accepted, so what each row
    // asserts is the classification of a refusal rather than a type that cannot be created.
    for (case, desired) in [
        (
            "a FIFO queue whose name carries the suffix",
            json!({ "QueueName": "classified-good.fifo", "FifoQueue": true }),
        ),
        (
            "a standard queue whose name carries no suffix",
            json!({ "QueueName": "classified-good" }),
        ),
    ] {
        let accepted = emulator.create_ok(QUEUE, &desired, None).await;
        let settled = emulator.settled(&text(&accepted, "RequestToken")).await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_SUCCESS,
            "case {case:?}: a consistent model must be created, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn the_classification_is_the_same_whichever_service_owns_the_type() {
    // The rule is about WHO judged the call, not about one owning service, so the same two
    // codes are reported for the same two kinds of fault over three more owning services.
    let topics = Emulator::with_sns();
    let rows: [(&str, Value, &str); 2] = [
        (
            "a FIFO topic whose name lacks the suffix the owning service requires",
            json!({ "TopicName": "classified-topic", "FifoTopic": true }),
            DOWNSTREAM,
        ),
        (
            "a property the schema declares that this handler does not apply",
            json!({
                "TopicName": "classified-subscribed",
                "Subscription": [{ "Protocol": "sqs", "Endpoint": "arn:aws:sqs:us-east-1:000000000000:q" }],
            }),
            MODEL_FAULT,
        ),
    ];
    for (case, desired, expected_code) in rows {
        let (code, detail) = failed_create(&topics, TOPIC, &desired, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code} handler code, got {code:?} with \
             {detail:?}"
        );
    }

    let buckets = Emulator::with_s3();
    let rows: [(&str, Value, &str); 2] = [
        (
            "a bucket name outside the published general purpose naming rules",
            json!({ "BucketName": "Classified_Bucket" }),
            DOWNSTREAM,
        ),
        (
            "a property the type's schema does not declare",
            json!({ "BucketName": "classified-bucket", "Flavour": "vanilla" }),
            MODEL_FAULT,
        ),
    ];
    for (case, desired, expected_code) in rows {
        let (code, detail) = failed_create(&buckets, BUCKET, &desired, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code} handler code, got {code:?} with \
             {detail:?}"
        );
    }

    let parameters = Emulator::with_ssm();
    let rows: [(&str, Value, &str); 2] = [
        (
            "a name in the hierarchy the owning service keeps for itself",
            json!({ "Name": "/aws/classified", "Type": "String", "Value": "v" }),
            DOWNSTREAM,
        ),
        (
            "a property the type's schema declares that this handler does not apply",
            json!({
                "Name": "/classified/policed",
                "Type": "String",
                "Value": "v",
                "Policies": "[{\"Type\":\"Expiration\",\"Version\":\"1.0\"}]",
            }),
            MODEL_FAULT,
        ),
    ];
    for (case, desired, expected_code) in rows {
        let (code, detail) = failed_create(&parameters, PARAMETER, &desired, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the {expected_code} handler code, got {code:?} with \
             {detail:?}"
        );
    }
}

#[tokio::test]
async fn an_update_the_owning_service_refuses_carries_the_downstream_handler_code() {
    let emulator = Emulator::with_sqs();
    let identifier = emulator
        .provision(QUEUE, &json!({ "QueueName": "classified-patched" }))
        .await;
    let before = emulator.properties(QUEUE, &identifier).await;

    // The same classification on the UPDATE path: an attribute no queue of this kind has,
    // which is the owning service's own answer about that queue, against a property the
    // type's schema does not declare.
    let rows: [(&str, Value, &str, &str); 2] = [
        (
            "an attribute that applies to no queue of this kind",
            // `ContentBasedDeduplication` "applies only to FIFO (first-in-first-out)
            // queues", and this queue is a standard one.
            json!([{ "op": "add", "path": "/ContentBasedDeduplication", "value": true }]),
            DOWNSTREAM,
            DOWNSTREAM_UNKNOWN_ATTRIBUTE,
        ),
        (
            "a property the type's schema does not declare",
            json!([{ "op": "add", "path": "/Policy", "value": {} }]),
            MODEL_FAULT,
            "Policy is not a property of AWS::SQS::Queue",
        ),
    ];

    for (case, patch, expected_code, expected_detail) in rows {
        let (status, accepted) = emulator.update(QUEUE, &identifier, &patch, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the request to be accepted, got {status} with body \
             {accepted:?}"
        );
        let settled = emulator
            .terminal(&text(&progress(&accepted), "RequestToken"))
            .await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: expected the request to fail, got {settled:?}"
        );
        assert_eq!(
            text(&settled, "ErrorCode"),
            expected_code,
            "case {case:?}: expected the {expected_code} handler code, got {settled:?}"
        );
        assert!(
            text(&settled, "StatusMessage").contains(expected_detail),
            "case {case:?}: expected a diagnostic carrying {expected_detail:?}, got {settled:?}"
        );
        let after = emulator.properties(QUEUE, &identifier).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused patch must leave the queue as it was, got {after:?}"
        );
    }
}
