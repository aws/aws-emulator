// SPDX-License-Identifier: CC0-1.0

//! What `PutDestination` and `PutDestinationPolicy` VALIDATE before they store anything, end
//! to end over the wire.
//!
//! Four rules, each asserted over a table that spans what it generalises over rather than one
//! request:
//!
//! * a `roleArn` is PASSED to this service by the account that holds it ("To perform a
//!   PutDestination operation, you must also have the iam:PassRole permission"), so a role of
//!   another account is refused with the service's fixed sentence for that class and NOTHING is
//!   stored — the read path then reports no such destination;
//! * a destination "encapsulates a physical resource", and the resources one can encapsulate
//!   are the streaming destinations this service documents ("We currently support Kinesis Data
//!   Streams and Firehose as logical destinations"), so a `targetArn` of any other vendor is
//!   refused naming that vendor;
//! * the put VALIDATES the wiring by delivering one test message to the target, so a target
//!   whose own service reports it holds nothing under it is refused — and an accepted put
//!   leaves that test message on the target, which is what makes the validation real rather
//!   than a shape check. That rule is driven with the Kinesis emulator mounted beside this
//!   service on one engine, so the target exists (or does not) where its own service says;
//! * `PutDestinationPolicy` PARSES the `accessPolicy` before it resolves the destination, and
//!   what its IAM policy parser takes is a document whose statements grant to ACCOUNTS — the
//!   cross-account users a destination's access policy authorizes.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};
use crate::subscription_fan_out::{stream_arn, Emulator};

/// The operations under test, plus the read a refused put is proved absent through.
const PUT: &str = "PutDestination";
const PUT_POLICY: &str = "PutDestinationPolicy";
const DESCRIBE: &str = "DescribeDestinations";

/// The refusal both operations model for a parameter specified incorrectly.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// AWS's fixed sentence for a request that hands this service a role of another account. It
/// names neither the member, the role nor the account.
const CROSS_ACCOUNT_ROLE: &str = "Cross-account role passing is not allowed.";

/// AWS's fixed sentence for a destination whose target would not take the service's test
/// message.
const UNDELIVERABLE_TARGET: &str =
    "Could not deliver test message to specified destination. Check if the destination is valid.";

/// AWS's fixed sentence for an `accessPolicy` its IAM policy parser will not take.
const POLICY_PARSE_FAILED: &str = "Error occurred while parsing accessPolicy. Please check if \
                                   the accessPolicy has been constructed correctly using IAM \
                                   grammar.";

/// The account the emulator serves, which is the only account a role can be passed from.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

/// A role ARN of `account`, under `path` (empty for a role at the root).
fn role_arn(account: &str, path: &str, name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{path}{name}")
}

/// A target of a vendor a destination CAN deliver to, in `account`.
fn kinesis_arn(account: &str, stream: &str) -> String {
    format!("arn:aws:kinesis:{REGION}:{account}:stream/{stream}")
}

/// The destinations the service reports under `prefix`.
async fn described(logs: &Logs, prefix: &str) -> Vec<Value> {
    let page = logs
        .ok(DESCRIBE, &json!({"DestinationNamePrefix": prefix}))
        .await;
    page.get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} must report destinations, got {page}"))
        .clone()
}

/// A `roleArn` of another account is refused with the fixed cross-account sentence, whatever
/// form the role takes and whatever the target is, and the refused put stores nothing; the same
/// put with a role of the caller's own account is accepted.
#[tokio::test]
async fn a_role_of_another_account_cannot_be_passed() {
    let logs = Logs::new();

    // The rows differ along what the rule generalises over: the account the role belongs to
    // (one that is not the caller's, in either direction from it), whether the role sits at
    // the root of the account or under a path, the partition the ARN names, and whether the
    // target is in the caller's account or elsewhere.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a role of another account, same-account target",
            role_arn("210987654321", "", "CWLtoKinesisRole"),
            kinesis_arn(&own_account(), "log-events"),
        ),
        (
            "a role under a path in another account",
            role_arn("210987654321", "service-role/", "CWLtoKinesis"),
            kinesis_arn(&own_account(), "log-events"),
        ),
        (
            "a role of another account, cross-account target",
            role_arn("123456789012", "", "CWLtoKinesisRole"),
            kinesis_arn("123456789012", "other"),
        ),
        (
            "a role in another partition's spelling of another account",
            format!("arn:aws-cn:iam::123456789012:role/{}", "CWLtoKinesisRole"),
            kinesis_arn(&own_account(), "log-events"),
        ),
    ];

    for (index, (case, role, target)) in refused.iter().enumerate() {
        let name = format!("cross-account-{index}");
        let (code, message) = logs
            .refusal(
                PUT,
                &json!({"destinationName": name, "targetArn": target, "roleArn": role}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, CROSS_ACCOUNT_ROLE),
            "case {case:?}: expected ({INVALID_PARAMETER}, {CROSS_ACCOUNT_ROLE:?}), got \
             ({code}, {message:?})"
        );
        assert_eq!(
            described(&logs, &name).await,
            Vec::<Value>::new(),
            "case {case:?}: a refused put stores no destination"
        );
    }

    // The same puts with a role of the caller's OWN account: accepted, and read back.
    let accepted: Vec<(&str, String)> = vec![
        (
            "a role at the root of this account",
            role_arn(&own_account(), "", "CWLtoKinesisRole"),
        ),
        (
            "a role under a path in this account",
            role_arn(&own_account(), "service-role/", "CWLtoKinesis"),
        ),
    ];
    for (index, (case, role)) in accepted.iter().enumerate() {
        let name = format!("own-account-{index}");
        let target = kinesis_arn("123456789012", "log-events");
        logs.ok(
            PUT,
            &json!({"destinationName": name, "targetArn": target, "roleArn": role}),
        )
        .await;
        let reported = described(&logs, &name).await;
        assert_eq!(
            reported
                .first()
                .and_then(|destination| destination.get("roleArn")),
            Some(&json!(role)),
            "case {case:?}: the destination carries the role it was created with, got {reported:?}"
        );
    }
}

/// A `targetArn` of a vendor a logical destination cannot deliver to is refused naming that
/// vendor, and stores nothing; the two vendors the service documents are accepted.
#[tokio::test]
async fn a_target_of_an_unsupported_vendor_is_refused_naming_the_vendor() {
    let logs = Logs::new();
    let role = role_arn(&own_account(), "", "CWLtoKinesisRole");

    // One row per vendor outside the supported pair, including resources of THIS service —
    // a log group and a logical destination are not targets a destination delivers to.
    // Source: `PutSubscriptionFilter`'s own list of logical destinations ("We currently
    // support Kinesis Data Streams and Firehose as logical destinations").
    let refused: Vec<(&str, String)> = vec![
        (
            "lambda",
            format!("arn:aws:lambda:{REGION}:123456789012:function:log-consumer"),
        ),
        ("s3", "arn:aws:s3:::log-archive".to_string()),
        ("sns", format!("arn:aws:sns:{REGION}:123456789012:log-topic")),
        ("sqs", format!("arn:aws:sqs:{REGION}:123456789012:log-queue")),
        (
            "logs",
            format!("arn:aws:logs:{REGION}:123456789012:log-group:/vended/one"),
        ),
        (
            "logs",
            format!("arn:aws:logs:{REGION}:123456789012:destination:other-dest"),
        ),
        (
            "kinesisanalytics",
            format!("arn:aws:kinesisanalytics:{REGION}:123456789012:application/app"),
        ),
    ];

    for (index, (vendor, target)) in refused.iter().enumerate() {
        let name = format!("unsupported-{index}");
        let expected = format!("Invalid destination. Vendor {vendor} not supported.");
        let (code, message) = logs
            .refusal(
                PUT,
                &json!({"destinationName": name, "targetArn": target, "roleArn": role}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {target:?}: expected ({INVALID_PARAMETER}, {expected:?}), got \
             ({code}, {message:?})"
        );
        assert_eq!(
            described(&logs, &name).await,
            Vec::<Value>::new(),
            "case {target:?}: a refused put stores no destination"
        );
    }

    // The supported pair, in either account: accepted and read back.
    let accepted = [
        kinesis_arn(&own_account(), "log-events"),
        kinesis_arn("123456789012", "log-events"),
        format!("arn:aws:firehose:{REGION}:123456789012:deliverystream/log-events"),
    ];
    for (index, target) in accepted.iter().enumerate() {
        let name = format!("supported-{index}");
        logs.ok(
            PUT,
            &json!({"destinationName": name, "targetArn": target, "roleArn": role}),
        )
        .await;
        let reported = described(&logs, &name).await;
        assert_eq!(
            reported
                .first()
                .and_then(|destination| destination.get("targetArn")),
            Some(&json!(target)),
            "case {target:?}: the destination carries the target it was created with, got \
             {reported:?}"
        );
    }
}

/// The put DELIVERS a test message to the target: a target its own service holds takes that
/// one record and the destination is stored, while a target that service reports absent is
/// refused and nothing is stored.
///
/// Driven with the Kinesis emulator mounted beside this service, so whether the target exists
/// is answered by the service that owns it.
#[tokio::test]
async fn the_put_validates_the_target_by_delivering_a_test_message() {
    let emulator = Emulator::new();
    let case = "a stream the target's own service holds";
    let stream = "wiring-target";
    emulator.usable_stream(case, stream).await;

    let role = role_arn(&own_account(), "", "CWLtoKinesisRole");
    let name = "validated-dest";
    emulator
        .logs(
            case,
            PUT,
            &json!({
                "destinationName": name,
                "targetArn": stream_arn(stream),
                "roleArn": role,
            }),
        )
        .await;

    // The accepted put wrote its one test message to the target, which is what validating the
    // wiring means — the record is read back from the stream's own shard.
    let checks = emulator.health_checks(case, stream).await;
    assert_eq!(
        checks.len(),
        1,
        "case {case}: the accepted put delivers one test message to the target, got {checks:?}"
    );

    // Targets of the same vendor that its own service holds nothing under: one stream never
    // created, and one whose name differs from the held stream only in case.
    let refused: Vec<(&str, String)> = vec![
        ("a stream never created", stream_arn("absent-target")),
        ("the held stream's name in another case", stream_arn("WIRING-TARGET")),
    ];
    for (index, (refused_case, target)) in refused.iter().enumerate() {
        let refused_name = format!("undeliverable-{index}");
        let (status, body) = emulator
            .call(
                "logs",
                "Logs_20140328",
                PUT,
                &json!({
                    "destinationName": refused_name,
                    "targetArn": target,
                    "roleArn": role,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {refused_case:?}: a target its service holds nothing under is refused, got \
             {status} {body}"
        );
        assert_eq!(
            (
                crate::harness::error_code(&body).as_str(),
                body.get("message").and_then(Value::as_str).unwrap_or_default()
            ),
            (INVALID_PARAMETER, UNDELIVERABLE_TARGET),
            "case {refused_case:?}: expected ({INVALID_PARAMETER}, {UNDELIVERABLE_TARGET:?}), \
             got {body}"
        );

        let page = emulator
            .logs(
                refused_case,
                DESCRIBE,
                &json!({"DestinationNamePrefix": refused_name}),
            )
            .await;
        assert_eq!(
            page.get("destinations"),
            Some(&json!([])),
            "case {refused_case:?}: a refused put stores no destination, got {page}"
        );
    }

    // A target whose OWNING SERVICE this emulator does not serve at all establishes nothing
    // about the target — no answer about it was read — so the put is not refused for it: the
    // refusal above is grounded in what the target's own service answered, and nothing else.
    let unasked = "unasked-vendor-dest";
    emulator
        .logs(
            case,
            PUT,
            &json!({
                "destinationName": unasked,
                "targetArn": format!(
                    "arn:aws:firehose:{REGION}:{}:deliverystream/never-created",
                    own_account()
                ),
                "roleArn": role,
            }),
        )
        .await;
}

/// `PutDestinationPolicy` parses the document BEFORE it resolves the destination, and what the
/// parser takes is a policy whose statements grant to accounts: so a document it rejects is
/// refused for the document whether or not the destination exists, and a document it takes is
/// refused for the missing destination.
#[tokio::test]
async fn the_access_policy_is_parsed_before_the_destination_is_resolved() {
    let logs = Logs::new();
    let held = "parse-order-dest";
    logs.ok(
        PUT,
        &json!({
            "destinationName": held,
            "targetArn": kinesis_arn("123456789012", "log-events"),
            "roleArn": role_arn(&own_account(), "", "CWLtoKinesisRole"),
        }),
    )
    .await;

    /// One grant, as a whole access policy document.
    fn policy(principal: Value) -> String {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowCrossAccountSubscription",
                "Effect": "Allow",
                "Principal": principal,
                "Action": "logs:PutSubscriptionFilter",
                "Resource": "*",
            }],
        })
        .to_string()
    }

    // Documents the parser will NOT take, spanning the forms it rejects: a document that is
    // not JSON, one that is JSON but no policy, one declaring no statement, and grants whose
    // principal is not an account — a service, a federated provider, and no principal at all.
    let rejected: Vec<(&str, String)> = vec![
        ("a document that is not JSON", "not a policy".to_string()),
        ("a JSON array", json!(["Statement"]).to_string()),
        ("the empty JSON object", "{}".to_string()),
        (
            "a policy declaring no version",
            json!({"Statement": [{"Effect": "Allow", "Principal": {"AWS": "210987654321"}, "Action": "logs:PutSubscriptionFilter"}]})
                .to_string(),
        ),
        (
            "a grant to a service principal",
            policy(json!({"Service": "logs.amazonaws.com"})),
        ),
        (
            "a grant to a federated provider",
            policy(json!({"Federated": "accounts.google.com"})),
        ),
        (
            "a grant to a canonical user",
            policy(json!({"CanonicalUser": "a".repeat(64)})),
        ),
        ("a grant to no principal at all", policy(Value::Null)),
    ];

    for (case, document) in &rejected {
        // The same document is refused for the DOCUMENT under both names: the destination the
        // account holds, and one it does not.
        for (where_case, name) in [("a held destination", held), ("an absent destination", "absent-dest")] {
            let (code, message) = logs
                .refusal(
                    PUT_POLICY,
                    &json!({"destinationName": name, "accessPolicy": document}),
                )
                .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_PARAMETER, POLICY_PARSE_FAILED),
                "case {case:?} on {where_case}: expected ({INVALID_PARAMETER}, \
                 {POLICY_PARSE_FAILED:?}), got ({code}, {message:?})"
            );
        }
    }

    // Documents the parser DOES take: every spelling of an ACCOUNT principal, and the
    // wildcard narrowed by a condition. Each is refused for the missing destination when the
    // name is one the account does not hold, and accepted under the name it holds.
    let taken: Vec<(&str, String)> = vec![
        ("an account by its id", policy(json!({"AWS": "210987654321"}))),
        (
            "an account by its root ARN",
            policy(json!({"AWS": "arn:aws:iam::210987654321:root"})),
        ),
        (
            "a role of another account",
            policy(json!({"AWS": "arn:aws:iam::210987654321:role/LogSender"})),
        ),
        (
            "two accounts",
            policy(json!({"AWS": ["210987654321", "arn:aws:iam::123456789012:root"]})),
        ),
    ];
    for (case, document) in &taken {
        let absent = format!("absent-{}", case.replace(' ', "-"));
        let expected = format!("Destination with name {absent} does not exist");
        let (code, message) = logs
            .refusal(
                PUT_POLICY,
                &json!({"destinationName": absent, "accessPolicy": document}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {case:?}: a document the parser takes is refused for the missing \
             destination, got ({code}, {message:?})"
        );

        logs.ok(
            PUT_POLICY,
            &json!({"destinationName": held, "accessPolicy": document}),
        )
        .await;
        let reported = described(&logs, held).await;
        assert_eq!(
            reported
                .first()
                .and_then(|destination| destination.get("accessPolicy")),
            Some(&json!(document)),
            "case {case:?}: the attached policy round-trips verbatim, got {reported:?}"
        );
    }
}
