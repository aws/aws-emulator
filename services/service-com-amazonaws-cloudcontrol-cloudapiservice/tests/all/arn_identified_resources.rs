// SPDX-License-Identifier: CC0-1.0

//! A resource type whose primary identifier is an ARN its OWNING service assigns.
//!
//! The rule every case here holds the service to: **`ProgressEvent$Identifier` is "The
//! primary identifier for the resource" — the property the type's own registry schema
//! points `primaryIdentifier` at — so for a type whose schema points it at an ARN (for
//! `AWS::SNS::Topic`, `/properties/TopicArn`, which that schema also declares read-only
//! because Amazon SNS assigns it), every answer reports the ARN the owning service holds
//! for the resource, the resource model carries it under that property, and that ARN is
//! what `GetResource`, `ListResources` and `DeleteResource` accept.**
//!
//! The rows differ along the VALUE the caller chose — the only part of the ARN it chooses —
//! so what is asserted is the ARN form rather than one recorded name: a plain name, one
//! carrying the punctuation a topic name may carry, a long one, and a FIFO topic, whose
//! name carries the suffix its owning service requires.
//!
//! `resource_identity` holds the same rule across the other kinds of primary identifier,
//! so what the two suites assert together is where a resource's identity COMES FROM rather
//! than anything about one type.

use serde_json::json;

use super::emulator::{text, Emulator, SIGNING_REGION};

/// The type whose primary identifier is the ARN Amazon SNS assigns.
const TOPIC: &str = "AWS::SNS::Topic";

/// The property the type's schema points `primaryIdentifier` at.
const TOPIC_ARN: &str = "TopicArn";

#[tokio::test]
async fn the_identifier_is_the_topic_arn_whatever_name_the_create_chose() {
    let emulator = Emulator::new();
    // The name is the only part of the ARN the caller chooses; the partition, the Region
    // and the account come from the call and this emulator's own configuration.
    let names = [
        "orders",
        "orders-2026_eu-west",
        &"n".repeat(80),
        "orders-events.fifo",
    ];

    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for name in names {
        let mut desired = json!({ "TopicName": name });
        // "FIFO topics ... must have the .fifo suffix": the flag and the suffix go
        // together, so the FIFO row carries both.
        if name.ends_with(".fifo") {
            desired["FifoTopic"] = json!(true);
        }
        let event = emulator.create_ok(TOPIC, &desired, None).await;
        accepted.push(text(&event, "RequestToken"));
    }

    for (name, request_token) in names.iter().zip(&accepted) {
        let expected = engine::emulator_arn("sns", SIGNING_REGION, name);
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        assert_eq!(
            identifier, expected,
            "name {name:?}: expected the topic ARN {expected:?}, got {identifier:?}"
        );

        // The model reports it under the property the schema names, so the handle a caller
        // was handed is findable as a property of the resource it names.
        let stored = emulator.properties(TOPIC, &identifier).await;
        assert_eq!(
            stored.get(TOPIC_ARN),
            Some(&json!(expected)),
            "name {name:?}: the model must carry the ARN under {TOPIC_ARN}, got {stored:?}"
        );
        assert_eq!(
            stored.get("TopicName"),
            Some(&json!(name)),
            "name {name:?}: the model must carry the name the create chose, got {stored:?}"
        );

        // And the name the ARN is built from addresses the same resource, because the
        // schema declares it as a further identifier of it.
        let by_name = emulator.properties(TOPIC, name).await;
        assert_eq!(
            by_name, stored,
            "name {name:?}: the name must address the same topic, got {by_name:?}"
        );
    }

    // Every topic is listed under its ARN, and nothing else is.
    let mut listed = emulator.walk(TOPIC, None).await;
    listed.sort();
    let mut expected: Vec<String> = names
        .iter()
        .map(|name| engine::emulator_arn("sns", SIGNING_REGION, name))
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing must report every topic under its ARN, expected {expected:?}, got \
         {listed:?}"
    );

    // The documented flow is create, read the Identifier off the ProgressEvent, delete by
    // it — so each ARN is one the delete accepts.
    for name in names {
        let arn = engine::emulator_arn("sns", SIGNING_REGION, name);
        let deleted = emulator.delete_and_settle(TOPIC, &arn).await;
        assert_eq!(
            text(&deleted, "Identifier"),
            arn,
            "name {name:?}: the delete must name the topic ARN, got {deleted:?}"
        );
        let (status, body) = emulator.get(TOPIC, &arn).await;
        assert_eq!(
            status, 404,
            "name {name:?}: GetResource after the delete: expected HTTP 404, got {status} \
             with body {body:?}"
        );
    }
}
