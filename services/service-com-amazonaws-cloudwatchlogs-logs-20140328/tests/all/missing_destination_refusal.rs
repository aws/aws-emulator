// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal CloudWatch Logs answers when the `destinationName` a
//! request names is not a destination the account holds, on an operation that models no
//! `ResourceNotFoundException`.
//!
//! The rule under test: `InvalidParameterException` is documented only as "A parameter is
//! specified incorrectly", so that error class carries no sentence of its own — when it is the
//! error an operation reports a missing resource through, the message states WHICH parameter
//! value was rejected. For a destination the fixed text is "Destination with name " … " does
//! not exist", and its one variable part is the `destinationName` the request carried,
//! verbatim. So a caller managing several destinations can attribute the refusal to one
//! request, and the refusal never depends on the `accessPolicy` beside that name, on which
//! other destinations the account holds, or on how the name came to be absent (never created,
//! or created and then deleted).
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a message that is not built from the
//! request cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: names in every
//! character class the model's `DestinationName` (`@length` 1..512, `^[^:*]*$`) admits and at
//! both its published bounds, names that are a prefix of a held one and that differ from a held
//! one only in case, names never created and names whose destination was deleted, and — for
//! each of them — access policy documents of more than one shape.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operation under test, plus the destination operations a flow goes through.
const PUT_POLICY: &str = "PutDestinationPolicy";
const PUT_DESTINATION: &str = "PutDestination";
const DELETE_DESTINATION: &str = "DeleteDestination";
const DESCRIBE: &str = "DescribeDestinations";

/// The refusal `PutDestinationPolicy` models for a parameter specified incorrectly; it
/// declares no `ResourceNotFoundException`, so this is also what it answers a destination the
/// account does not hold with.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// AWS's missing-destination message: fixed text with the requested `destinationName` as its
/// one variable part.
fn missing_destination_message(destination_name: &str) -> String {
    format!("Destination with name {destination_name} does not exist")
}

/// Physical targets a destination delivers to, and the role CloudWatch Logs impersonates to
/// reach them — a role of the caller's OWN account, the only account a role can be passed
/// from.
const KINESIS_ARN: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/log-events";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/log-events";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CWLtoKinesisRole";

/// Access policy documents spanning what `accessPolicy` admits — one statement written as an
/// object and a full cross-account grant written as a list — so each row shows the refusal's
/// message depends on the name alone and not on the document beside it. Both are documents the
/// service's IAM policy parser TAKES: the parse comes first, so a document it rejects is
/// refused for the document rather than for the destination.
fn access_policies() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement written as an object rather than a list",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": "210987654321"},
                    "Action": "logs:PutSubscriptionFilter",
                },
            })
            .to_string(),
        ),
        (
            "a cross-account grant",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "cross-account",
                    "Effect": "Allow",
                    "Principal": {"AWS": "210987654321"},
                    "Action": "logs:PutSubscriptionFilter",
                    "Resource": "*",
                }],
            })
            .to_string(),
        ),
    ]
}

/// Create one destination under `name`.
async fn create(logs: &Logs, name: &str, target_arn: &str) {
    logs.ok(
        PUT_DESTINATION,
        &json!({"destinationName": name, "targetArn": target_arn, "roleArn": ROLE_ARN}),
    )
    .await;
}

/// The `accessPolicy` the service reports for the destination under `name`, read back through
/// the enumeration a client reads it through; `None` when it reports no destination at all.
async fn reported_policy(logs: &Logs, name: &str) -> Option<String> {
    let page = logs
        .ok(DESCRIBE, &json!({"DestinationNamePrefix": name}))
        .await;
    page.get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE}: must report destinations, got {page}"))
        .iter()
        .find(|destination| destination.get("destinationName") == Some(&json!(name)))
        .and_then(|destination| destination.get("accessPolicy"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Names satisfying `DestinationName` that the account holds no destination under, spanning
/// the character classes the model's pattern admits and both published length bounds.
fn absent_names(held: &str) -> Vec<(&'static str, String)> {
    vec![
        ("a plain name never created", "absent-dest".to_string()),
        (
            "a name carrying separators and punctuation",
            "team/one.dest-2_final+x".to_string(),
        ),
        ("a name outside ASCII", "存在しない-送信先".to_string()),
        ("a name carrying a space", "no such dest".to_string()),
        ("a digits-only name", "20140328".to_string()),
        // Both published bounds of `DestinationName` (`@length` 1..512).
        ("the shortest name the model admits", "x".to_string()),
        ("the longest name the model admits", "q".repeat(512)),
        // A destination is identified by its WHOLE name, so neither a prefix of a held name
        // nor a held name in another case names one.
        ("a prefix of a held name", held[..held.len() / 2].to_string()),
        ("a held name in another case", held.to_uppercase()),
    ]
}

/// A put against a destination the account does not hold is refused naming that destination,
/// whatever the policy document beside it and whichever destinations the account does hold.
#[tokio::test]
async fn the_refusal_names_the_destination_the_request_asked_for() {
    let logs = Logs::new();

    // Destinations the account DOES hold, so each row below is refused for naming no
    // destination rather than because the account holds none at all.
    let held = "held-dest";
    create(&logs, held, KINESIS_ARN).await;
    create(&logs, "another/held.dest", FIREHOSE_ARN).await;

    for (case, name) in absent_names(held) {
        for (policy_case, policy) in access_policies() {
            let expected = missing_destination_message(&name);
            let (code, message) = logs
                .refusal(
                    PUT_POLICY,
                    &json!({"destinationName": name, "accessPolicy": policy}),
                )
                .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_PARAMETER, expected.as_str()),
                "case {case:?} with {policy_case:?}: expected ({INVALID_PARAMETER}, \
                 {expected:?}), got ({code}, {message:?})"
            );
        }
    }

    // Refusing wrote nothing: the destinations the account holds still hold no policy.
    for name in [held, "another/held.dest"] {
        let reported = reported_policy(&logs, name).await;
        assert_eq!(
            reported, None,
            "case {name:?}: a refused put persists nothing, got {reported:?}"
        );
    }
}

/// The same refusal, reached the way a client reaches it in a flow that manages a
/// destination's whole life: the policy a held destination accepts is reported back, the
/// destination is deleted, the next put of the same policy is refused NAMING that destination,
/// and a destination created again under the name accepts a policy again.
#[tokio::test]
async fn a_deleted_destination_is_named_by_the_refusal_that_follows() {
    let logs = Logs::new();
    let (_, policy) = access_policies()
        .into_iter()
        .next_back()
        .expect("the policy table is not empty");

    // Several destinations, differing in target and in name class, each taken through the
    // same flow — the rule holds per destination, not for one name.
    let destinations = [
        ("a plain name", "kinesis-dest".to_string(), KINESIS_ARN),
        ("a name carrying a space", "cross account dest".to_string(), FIREHOSE_ARN),
        ("a name outside ASCII", "ログ-送信先".to_string(), KINESIS_ARN),
        ("the longest name the model admits", "z".repeat(512), FIREHOSE_ARN),
    ];

    for (case, name, target_arn) in &destinations {
        create(&logs, name, target_arn).await;
        logs.ok(
            PUT_POLICY,
            &json!({"destinationName": name, "accessPolicy": policy}),
        )
        .await;
        // The accepted policy is stored on the destination, so a read of it reports the
        // document verbatim: the refusal below is about the destination being gone.
        let reported = reported_policy(&logs, name).await;
        assert_eq!(
            reported.as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected the stored policy {policy:?} to be reported, got {reported:?}"
        );

        logs.ok(DELETE_DESTINATION, &json!({"destinationName": name}))
            .await;

        let expected = missing_destination_message(name);
        let (code, message) = logs
            .refusal(
                PUT_POLICY,
                &json!({"destinationName": name, "accessPolicy": policy}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {case:?}: expected ({INVALID_PARAMETER}, {expected:?}) once the destination \
             is deleted, got ({code}, {message:?})"
        );

        // A destination created again under the same name is a destination that exists, so
        // the very same put is accepted and reported back.
        create(&logs, name, target_arn).await;
        logs.ok(
            PUT_POLICY,
            &json!({"destinationName": name, "accessPolicy": policy}),
        )
        .await;
        let reported = reported_policy(&logs, name).await;
        assert_eq!(
            reported.as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected the re-created destination to report {policy:?} in \
             {REGION}, got {reported:?}"
        );
    }
}
