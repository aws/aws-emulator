// SPDX-License-Identifier: CC0-1.0

//! One rule for the whole tagging family, over the wire: which answer a `ResourceARN` that
//! resolves to no registered scalable target earns.
//!
//! RULE under test: `ResourceARN` "identifies the Application Auto Scaling scalable target"
//! the call addresses, so an ARN that identifies no REGISTERED target is the resource reported
//! ABSENT — the modelled `ResourceNotFoundException`, which
//! `smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource")`
//! declares on all three operations, carrying the fixed sentence "Scalable Target not found"
//! and the ARN asked about in the shape's own `ResourceName` member — and the answer is the
//! same whichever of the three operations asked and however the ARN fails to resolve. It is
//! served at the client-error status this service answers that condition on, not at the 404
//! the shape's `@httpError` declares.
//!
//! The rows span what the rule generalises over: the OPERATION (the read and both mutations,
//! each with its own payload) crossed with how the ARN fails to resolve (an id this service
//! never minted, an id of another form, the shortest id the modelled pattern admits, another
//! account, another Region, another partition, and the ARN of a target that WAS registered and
//! has since been deregistered). A registered target stands beside them and round-trips a tag
//! through the same three operations, so no row passes because the family refuses everything.

use serde_json::{json, Value};

use crate::emulator::{error_code, error_message, Emulator, ACCOUNT, REGION};

/// The sentence the refusal carries, written out here rather than read off the code under
/// test: the WORDING is part of what this suite holds the family to.
const TARGET_NOT_FOUND: &str = "Scalable Target not found";

/// The code the refusal carries.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The status it is served on.
const CLIENT_ERROR: u16 = 400;

/// One request per operation of the family, all three addressed to `resource_arn` with a
/// payload of its own: the answer is about the resource the ARN names, so neither the tags
/// written nor the keys removed change it.
fn family(resource_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({"ResourceARN": resource_arn, "Tags": {"env": "sim2real"}}),
        ),
        (
            "TagResource",
            json!({
                "ResourceARN": resource_arn,
                "Tags": {"env": "sim2real", "owner": "orders-team"},
            }),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env"]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": resource_arn, "TagKeys": ["env", "owner"]}),
        ),
        ("ListTagsForResource", json!({"ResourceARN": resource_arn})),
    ]
}

/// ARNs that satisfy the modelled `@pattern` on `ResourceARN`
/// (`^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$`), so each one
/// reaches the handler rather than the boundary validator, and none of them names a registered
/// target. `deregistered` is the ARN this service minted for a target that has since been
/// deregistered, so the table carries an ARN that WAS real beside ones that never were.
fn unresolvable(deregistered: &str) -> Vec<(String, String)> {
    vec![
        (
            "the ARN of a target that has been deregistered".to_string(),
            deregistered.to_string(),
        ),
        (
            "an id this service never minted".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an id of another form".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/nothing-here"
            ),
        ),
        (
            "the shortest id the pattern admits".to_string(),
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/a"),
        ),
        (
            "an ARN of another account".to_string(),
            format!(
                "arn:aws:application-autoscaling:{REGION}:111122223333:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an ARN of another Region".to_string(),
            format!(
                "arn:aws:application-autoscaling:eu-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
        (
            "an ARN of another partition".to_string(),
            format!(
                "arn:aws-us-gov:application-autoscaling:us-gov-west-1:{ACCOUNT}:scalable-target/\
                 0123456789abcdef0123456789abcdef0123"
            ),
        ),
    ]
}

#[tokio::test]
async fn every_tag_operation_reports_an_unresolvable_arn_as_the_absent_resource() {
    let emulator = Emulator::empty();

    // One target that stays registered, so the rows below are about the ARN they name, and one
    // that is deregistered to give the table the ARN of a target that was real.
    let resident = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resident_arn = emulator.target_arn(&resident).await;
    let withdrawn = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;
    let withdrawn_arn = emulator.target_arn(&withdrawn).await;
    let (status, body) = emulator
        .call("DeregisterScalableTarget", withdrawn.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"the deregistration\": expected HTTP 200, got {status} with {body:?}"
    );

    for (reason, arn) in unresolvable(&withdrawn_arn) {
        for (operation, request) in family(&arn) {
            let case = format!("{reason} on {operation}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, CLIENT_ERROR,
                "case {case:?}: expected HTTP {CLIENT_ERROR}, got {status} with {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, RESOURCE_NOT_FOUND,
                "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {code:?} in {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, TARGET_NOT_FOUND,
                "case {case:?}: expected {TARGET_NOT_FOUND:?}, got {message:?}"
            );
            assert_eq!(
                body["ResourceName"],
                Value::String(arn.clone()),
                "case {case:?}: expected the ARN asked about in ResourceName, got {body:?}"
            );
            assert!(
                !message.contains(arn.as_str()),
                "case {case:?}: expected the sentence to echo nothing of the ARN, got {message:?}"
            );
        }
    }

    // None of those refusals touched the store, and none of them was a blanket refusal: the
    // target that IS registered takes a tag, reports it back, and gives it up again — the whole
    // family round-tripping through the same three operations.
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": resident_arn, "Tags": {"env": "sim2real"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"tag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"read the resident target's tags\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({"env": "sim2real"}),
        "case \"read the resident target's tags\": expected the tag just written, got {body:?}"
    );
    let (status, body) = emulator
        .call(
            "UntagResource",
            json!({"ResourceARN": resident_arn, "TagKeys": ["env"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"untag the resident target\": expected HTTP 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": resident_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"read the emptied tag set\": expected HTTP 200, got {status} with {body:?}"
    );
    assert_eq!(
        body["Tags"],
        json!({}),
        "case \"read the emptied tag set\": expected an empty tag set, got {body:?}"
    );
}
