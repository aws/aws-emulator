// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDestinationPolicy`: the access policy an account attaches to one
//! of its logical cross-account destinations.
//!
//! The rule under test: a destination holds at most one access policy, so
//! `PutDestinationPolicy` stores the IAM policy document the request carries VERBATIM on the
//! destination it names and REPLACES whatever that destination held, leaving every other
//! member of the destination as it was; `DescribeDestinations` then reports those stored
//! contents as the destination's `accessPolicy`, a destination that has had none attached
//! reporting none at all. The policy is "associated with an existing destination", so a
//! `destinationName` the account does not hold is refused as an invalid parameter and stores
//! nothing; an `accessPolicy` that is not an IAM policy document — a JSON object within the
//! documented size — is refused the same way; and `forceUpdate` is the affirmation required
//! only for the one update the parameter documents, from granting individual accounts to
//! granting an organization ID.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: policy documents of
//! several forms and sizes (one statement written as an object, one statement in a list,
//! several statements, a nested condition, non-ASCII contents, pretty-printed whitespace, and
//! a document on the documented maximum size and one past it), destinations of several targets
//! and names at both published `DestinationName` length bounds, puts that attach a first policy
//! and puts that replace one, every shape of value the member refuses, and each combination of
//! grant kind and `forceUpdate`.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::MAX_ACCESS_POLICY_BYTES;

use crate::harness::{Logs, REGION};

/// The operation under test, plus the destination operations a round trip goes through.
const PUT_POLICY: &str = "PutDestinationPolicy";
const PUT_DESTINATION: &str = "PutDestination";
const DESCRIBE: &str = "DescribeDestinations";

/// The refusal this operation models for a parameter specified incorrectly.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// Physical targets a destination delivers to, and the role it impersonates: one Kinesis
/// stream (the example `PutDestination$targetArn` gives), one Firehose delivery stream, and a
/// plain IAM role of the caller's OWN account — a role is passed to this service by the
/// account that holds it.
const KINESIS_ARN: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/log-events";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/log-events";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CWLtoKinesisRole";

/// The accounts and the organization an access policy grants to.
const SENDER_ACCOUNT: &str = "210987654321";
const OTHER_SENDER_ACCOUNT: &str = "309876543210";
const ORGANIZATION_ID: &str = "o-1234567890";
const OTHER_ORGANIZATION_ID: &str = "o-0987654321";

/// One destination to attach policies to: the name it is created under and the target it
/// delivers to.
#[derive(Debug)]
struct Destination {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `destinationName` (`DestinationName`, `@length` 1..512, `^[^:*]*$`).
    name: String,
    /// `targetArn` (`TargetArn`, `@length` 1..).
    target_arn: &'static str,
}

impl Destination {
    fn of(label: &'static str, name: &str, target_arn: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            target_arn,
        }
    }

    /// The `PutDestination` request that creates this destination.
    fn put_body(&self) -> Value {
        json!({
            "destinationName": self.name,
            "targetArn": self.target_arn,
            "roleArn": ROLE_ARN,
        })
    }
}

/// Destinations spanning the targets a destination can name and both published bounds of
/// `DestinationName` (`@length` 1..512).
fn destinations() -> Vec<Destination> {
    vec![
        Destination::of("a Kinesis stream destination", "kinesis-dest", KINESIS_ARN),
        Destination::of("a Firehose destination", "firehose/dest", FIREHOSE_ARN),
        Destination::of("a name at the minimum length", "d", KINESIS_ARN),
        Destination::of(
            "a name at the maximum length",
            &"z".repeat(512),
            FIREHOSE_ARN,
        ),
    ]
}

/// One access policy document, as a client sends it.
#[derive(Debug)]
struct PolicyCase {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The `accessPolicy` string, stored and reported verbatim.
    document: String,
}

impl PolicyCase {
    fn of(label: &'static str, document: &str) -> Self {
        Self {
            label,
            document: document.to_string(),
        }
    }
}

/// An access policy granting the named accounts the claim this operation's documentation
/// describes — authorizing them "to register a subscription filter against a given
/// destination" — as one IAM policy document.
fn account_grant_policy(accounts: &[&str], destination_name: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowCrossAccountSubscription",
            "Effect": "Allow",
            "Principal": {
                "AWS": accounts
                    .iter()
                    .map(|account| format!("arn:aws:iam::{account}:root"))
                    .collect::<Vec<String>>(),
            },
            "Action": "logs:PutSubscriptionFilter",
            "Resource": format!(
                "arn:aws:logs:{REGION}:123456789012:destination:{destination_name}"
            ),
        }],
    })
    .to_string()
}

/// The same grant made to an ORGANIZATION instead of to individual accounts: the policy form
/// `PutDestinationPolicy$forceUpdate` is about ("grant permission to an organization ID
/// instead of granting permission to individual Amazon Web Services accounts"), which names
/// the organization through the `aws:PrincipalOrgID` condition key.
fn organization_grant_policy(organization_id: &str, destination_name: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowOrgSubscription",
            "Effect": "Allow",
            "Principal": {"AWS": "*"},
            "Action": "logs:PutSubscriptionFilter",
            "Resource": format!(
                "arn:aws:logs:{REGION}:123456789012:destination:{destination_name}"
            ),
            "Condition": {"StringEquals": {"aws:PrincipalOrgID": organization_id}},
        }],
    })
    .to_string()
}

/// One real grant whose `Sid` carries `sid`: the rows that vary a document's SIZE and
/// character set pad that one element, so each of them is still a document the service's IAM
/// policy parser takes.
fn sid_policy(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{SENDER_ACCOUNT}:root")},
            "Action": "logs:PutSubscriptionFilter",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A policy document of exactly `length` bytes: [`sid_policy`] padded through its `Sid`
/// value, so a case can sit on a documented size bound.
fn policy_of_length(length: usize) -> String {
    let skeleton = sid_policy("");
    sid_policy(&"p".repeat(length.saturating_sub(skeleton.len())))
}

/// Access policy documents spanning form, size, nesting, character set and whitespace. Every
/// row is an IAM policy document, which is what the member carries ("An access policy is an
/// IAM policy document"); JSON that is not one is a refused row instead.
fn policies() -> Vec<PolicyCase> {
    vec![
        PolicyCase::of(
            "one statement written as an object rather than a list",
            &json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{SENDER_ACCOUNT}:root")},
                    "Action": "logs:PutSubscriptionFilter",
                },
            })
            .to_string(),
        ),
        PolicyCase::of(
            "a one-account grant",
            &account_grant_policy(&[SENDER_ACCOUNT], "kinesis-dest"),
        ),
        PolicyCase::of(
            "a grant to several accounts",
            &account_grant_policy(&[SENDER_ACCOUNT, OTHER_SENDER_ACCOUNT], "kinesis-dest"),
        ),
        PolicyCase::of(
            "a grant with nested conditions",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowConditionalSubscription",
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{SENDER_ACCOUNT}:root")},
                    "Action": ["logs:PutSubscriptionFilter"],
                    "Resource": "*",
                    "Condition": {
                        "StringEquals": {
                            "aws:SourceAccount": [SENDER_ACCOUNT, OTHER_SENDER_ACCOUNT],
                        },
                        "ArnLike": {"aws:SourceArn": format!("arn:aws:logs:{REGION}:*:*")},
                    },
                }],
            })
            .to_string(),
        ),
        PolicyCase::of(
            "a policy carrying non-ASCII contents",
            &sid_policy("zugriff-\u{00fc}ber-gr\u{00f6}\u{00dfe}-\u{65e5}\u{672c}"),
        ),
        PolicyCase::of(
            "a pretty-printed policy, whitespace and all",
            "{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ {\n    \"Effect\" : \"Allow\",\n    \"Principal\" : {\n      \"AWS\" : \"arn:aws:iam::123456789012:root\"\n    },\n    \"Action\" : \"logs:PutSubscriptionFilter\"\n  } ]\n}",
        ),
        // On the documented size of `accessPolicy` ("This can be up to 5120 bytes"). Source:
        // `state::MAX_ACCESS_POLICY_BYTES`.
        PolicyCase::of(
            "a policy on the documented maximum size",
            &policy_of_length(MAX_ACCESS_POLICY_BYTES),
        ),
    ]
}

/// Attach one policy, asserting the call succeeds and reports nothing (the operation's output
/// is the modelled unit).
async fn put_policy(logs: &Logs, name: &str, document: &str) {
    let body = json!({"destinationName": name, "accessPolicy": document});
    let response = logs.ok(PUT_POLICY, &body).await;
    assert!(
        response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
        "case {name:?}: {PUT_POLICY} reports no members, got {response}"
    );
}

/// The one destination the service reports under `name`, through a wire
/// `DescribeDestinations`.
async fn described(logs: &Logs, name: &str) -> Option<Value> {
    let body = json!({"DestinationNamePrefix": name});
    let response = logs.ok(DESCRIBE, &body).await;
    response
        .get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} {body}: must report destinations, got {response}"))
        .iter()
        .find(|destination| destination.get("destinationName") == Some(&json!(name)))
        .cloned()
}

/// The `accessPolicy` the service reports for `name`, or `None` when the destination carries
/// none.
async fn reported_policy(logs: &Logs, name: &str) -> Option<String> {
    let destination = described(logs, name)
        .await
        .unwrap_or_else(|| panic!("case {name:?}: the destination must be reported"));
    destination
        .get("accessPolicy")
        .map(|policy| {
            policy
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: accessPolicy is a string, got {destination}")
                })
                .to_string()
        })
}

/// Every policy document, attached to every destination, is stored verbatim and reported back
/// as that destination's `accessPolicy` — the round trip through stored state — and a read
/// changes nothing.
#[tokio::test]
async fn an_attached_policy_is_described_back() {
    let logs = Logs::new();
    let destinations = destinations();
    let policies = policies();

    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        for policy in &policies {
            let case = (destination.label, policy.label);
            put_policy(&logs, &destination.name, &policy.document).await;

            let read_back = reported_policy(&logs, &destination.name).await;
            assert_eq!(
                read_back.as_deref(),
                Some(policy.document.as_str()),
                "case {case:?}: {DESCRIBE} reports the attached policy, got {read_back:?}"
            );

            let again = reported_policy(&logs, &destination.name).await;
            assert_eq!(
                again, read_back,
                "case {case:?}: a second read reports the same policy, got {again:?}"
            );
        }
    }
}

/// A destination that exists but has had no policy attached reports NO `accessPolicy`, and
/// attaching one to a sibling destination leaves it reporting none.
#[tokio::test]
async fn a_destination_with_no_policy_attached_reports_none() {
    let logs = Logs::new();
    let destinations = destinations();
    for destination in &destinations {
        logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    }

    for destination in &destinations {
        let reported = reported_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a destination with no policy attached reports none, got {reported:?}",
            destination.label
        );
    }

    put_policy(
        &logs,
        &destinations[0].name,
        &account_grant_policy(&[SENDER_ACCOUNT], &destinations[0].name),
    )
    .await;
    for destination in destinations.iter().skip(1) {
        let reported = reported_policy(&logs, &destination.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: a policy on one destination is not reported for another, got {reported:?}",
            destination.label
        );
    }
}

/// A destination holds at most one access policy, so each put REPLACES the one before it: the
/// document attached last is the one every later reader sees, whichever order the documents are
/// attached in.
#[tokio::test]
async fn the_policy_attached_last_is_the_one_reported() {
    let logs = Logs::new();
    let destination = Destination::of("a Kinesis stream destination", "replaced", KINESIS_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;

    let mut documents: Vec<String> = policies().into_iter().map(|case| case.document).collect();
    // The same set attached in the reverse order too, so what is reported follows the LAST put
    // rather than the first, the longest or the shortest.
    let mut reversed = documents.clone();
    reversed.reverse();
    documents.extend(reversed);

    for document in &documents {
        // No document in this set grants an organization, so no row here is the one update
        // `forceUpdate` affirms; that rule has its own case below.
        put_policy(&logs, &destination.name, document).await;
        let reported = reported_policy(&logs, &destination.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(document.as_str()),
            "case {:?}: the policy attached last is the one reported, got {reported:?}",
            document.len()
        );
    }
}

/// Attaching a policy changes nothing else about the destination — its ARN, target, role,
/// creation instant and tags all survive — and a later `PutDestination` under the same name,
/// which "creates or updates a destination", leaves the attached policy in place.
#[tokio::test]
async fn a_policy_and_the_destination_parameters_do_not_overwrite_each_other() {
    let logs = Logs::new();
    let name = "paired-dest";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "destinationName": name,
            "targetArn": KINESIS_ARN,
            "roleArn": ROLE_ARN,
            "tags": {"team": "platform"},
        }),
    )
    .await;
    let created = described(&logs, name)
        .await
        .expect("case \"before attaching a policy\": the destination is reported");

    let document = account_grant_policy(&[SENDER_ACCOUNT], name);
    put_policy(&logs, name, &document).await;

    let after = described(&logs, name)
        .await
        .expect("case \"after attaching a policy\": the destination is reported");
    for member in ["arn", "destinationName", "targetArn", "roleArn", "creationTime"] {
        assert_eq!(
            after.get(member),
            created.get(member),
            "case \"after attaching a policy\": {member} is unchanged, got {after}"
        );
    }

    // Updating the destination's own parameters leaves the attached policy where it is.
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "destinationName": name,
            "targetArn": FIREHOSE_ARN,
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    let reported = reported_policy(&logs, name).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"after updating the destination parameters\": the attached policy stays, \
         got {reported:?}"
    );
    let updated = described(&logs, name)
        .await
        .expect("case \"after updating the destination\": the destination is reported");
    assert_eq!(
        updated.get("targetArn"),
        Some(&json!(FIREHOSE_ARN)),
        "case \"after updating the destination\": expected the new target, got {updated}"
    );
}

/// The policy is "associated with an existing destination", so a `destinationName` the account
/// does not hold is refused as an invalid parameter — and the refusal stores nothing, so the
/// name still holds no policy once a destination is created under it.
#[tokio::test]
async fn a_destination_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    let held = Destination::of("a Kinesis stream destination", "held-dest", KINESIS_ARN);
    logs.ok(PUT_DESTINATION, &held.put_body()).await;

    // Names that satisfy `DestinationName` (`@length` 1..512, `^[^:*]*$`) and that the account
    // does not hold: one never used, one differing from a held name only in case, one at each
    // published length bound.
    let absent = [
        ("a name never used", "absent-dest".to_string()),
        ("a held name in another case", "HELD-DEST".to_string()),
        ("a one-character name", "x".to_string()),
        ("a name at the maximum length", "q".repeat(512)),
    ];

    for (case, name) in &absent {
        let expected = format!("Destination with name {name} does not exist");
        let (code, message) = logs
            .refusal(
                PUT_POLICY,
                &json!({
                    "destinationName": name,
                    // A document the parser TAKES, so the refusal is the one about the
                    // destination: the parse comes FIRST, and a document it rejects is refused
                    // for the document whether or not the destination exists.
                    "accessPolicy": account_grant_policy(&[SENDER_ACCOUNT], name),
                }),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {case:?}: attaching a policy to a destination the account does not hold is \
             refused with ({INVALID_PARAMETER}, {expected:?}), got ({code}, {message:?})"
        );
    }

    // The refused puts wrote nothing: a destination created under one of those names afterwards
    // holds no policy, and the destination the account does hold holds none either.
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "destinationName": "absent-dest",
            "targetArn": KINESIS_ARN,
            "roleArn": ROLE_ARN,
        }),
    )
    .await;
    for name in ["absent-dest", &held.name] {
        let reported = reported_policy(&logs, name).await;
        assert_eq!(
            reported, None,
            "case {name:?}: a refused put persists nothing, got {reported:?}"
        );
    }
}

/// "An access policy is an IAM policy document" of up to the documented size, so an
/// `accessPolicy` that is not a JSON object, or is larger than that, is refused as an invalid
/// parameter — and the refusal leaves the policy the destination already held in place.
#[tokio::test]
async fn a_policy_that_is_not_an_iam_policy_document_is_refused() {
    let logs = Logs::new();
    let destination = Destination::of("a Kinesis stream destination", "guarded", KINESIS_ARN);
    logs.ok(PUT_DESTINATION, &destination.put_body()).await;
    let kept = account_grant_policy(&[SENDER_ACCOUNT], &destination.name);
    put_policy(&logs, &destination.name, &kept).await;

    let rejected = [
        ("plain text", "not a policy at all".to_string()),
        ("a one-character document", "p".to_string()),
        // JSON that parses and is still not an IAM policy document: the empty object, an
        // object declaring no statement, and one whose statement list is empty.
        ("the empty JSON object", "{}".to_string()),
        (
            "an object that declares no statement",
            json!({"Sid": "a"}).to_string(),
        ),
        (
            "a policy whose statement list is empty",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        (
            "a JSON array of statements",
            r#"[{"Effect":"Allow"}]"#.to_string(),
        ),
        ("a JSON string", r#""{\"Effect\":\"Allow\"}""#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON boolean", "true".to_string()),
        ("a JSON null", "null".to_string()),
        ("a truncated object", r#"{"Statement":"#.to_string()),
        ("an object with a trailing comma", r#"{"Sid":"a",}"#.to_string()),
        // One byte past the documented size ("This can be up to 5120 bytes"), whose in-bound
        // sibling is a passing row in `policies()`.
        (
            "one byte past the documented maximum size",
            policy_of_length(MAX_ACCESS_POLICY_BYTES + 1),
        ),
        // One byte short of `AccessPolicy`'s published minimum (`@length` min 1), refused by
        // the same code this operation models for a parameter it will not accept.
        ("an empty document", String::new()),
    ];

    for (case, document) in &rejected {
        let code = logs
            .err(
                PUT_POLICY,
                &json!({"destinationName": destination.name, "accessPolicy": document}),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: a policy that is not an IAM policy document is refused with \
             {INVALID_PARAMETER}, got {code}"
        );

        let reported = reported_policy(&logs, &destination.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(kept.as_str()),
            "case {case:?}: a refused put leaves the attached policy in place, got {reported:?}"
        );
    }
}

/// `forceUpdate` is the affirmation for ONE class of update — "updating an existing destination
/// policy to grant permission to an organization ID instead of granting permission to
/// individual Amazon Web Services accounts" — so exactly that update needs it and every other
/// put is unaffected by the parameter ("If you omit this parameter, the default of false is
/// used").
#[tokio::test]
async fn only_an_update_to_an_organization_grant_needs_the_affirmation() {
    let logs = Logs::new();

    for (index, case) in force_update_cases().iter().enumerate() {
        // One destination per row, so each row starts from the state it describes.
        let name = format!("force-{index}");
        logs.ok(
            PUT_DESTINATION,
            &json!({
                "destinationName": name,
                "targetArn": KINESIS_ARN,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
        let label = case.label;
        let held_document = case.held.map(|grant| grant.document(&name));
        if let Some(document) = held_document.as_deref() {
            put_policy(&logs, &name, document).await;
        }

        let next_document = case.next.document(&name);
        let mut body = json!({"destinationName": name, "accessPolicy": next_document});
        if let Some(force_update) = case.force_update {
            body["forceUpdate"] = json!(force_update);
        }

        if case.refused {
            let (code, message) = logs.refusal(PUT_POLICY, &body).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {label:?}: expected {INVALID_PARAMETER}, got {code} {message}"
            );
            assert!(
                message.contains("forceUpdate"),
                "case {label:?}: the refusal names the parameter to set, got {message:?}"
            );
            let reported = reported_policy(&logs, &name).await;
            assert_eq!(
                reported.as_deref(),
                held_document.as_deref(),
                "case {label:?}: a refused put leaves the attached policy in place, \
                 got {reported:?}"
            );
        } else {
            logs.ok(PUT_POLICY, &body).await;
            let reported = reported_policy(&logs, &name).await;
            assert_eq!(
                reported.as_deref(),
                Some(next_document.as_str()),
                "case {label:?}: expected the policy just attached, got {reported:?}"
            );
        }
    }
}

/// One grant an access policy can make, as the `forceUpdate` cases move between them.
#[derive(Debug, Clone, Copy)]
enum Grant {
    /// A grant to one individual account.
    OneAccount,
    /// A grant to two individual accounts.
    TwoAccounts,
    /// A grant to one organization ID.
    Organization,
    /// A grant to a different organization ID.
    OtherOrganization,
    /// A policy that names no organization and grants nobody.
    Nobody,
}

impl Grant {
    /// This grant as an access policy document for one destination.
    fn document(self, destination_name: &str) -> String {
        match self {
            Self::OneAccount => account_grant_policy(&[SENDER_ACCOUNT], destination_name),
            Self::TwoAccounts => {
                account_grant_policy(&[SENDER_ACCOUNT, OTHER_SENDER_ACCOUNT], destination_name)
            }
            Self::Organization => organization_grant_policy(ORGANIZATION_ID, destination_name),
            Self::OtherOrganization => {
                organization_grant_policy(OTHER_ORGANIZATION_ID, destination_name)
            }
            Self::Nobody => json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "DenyEverySender",
                    "Effect": "Deny",
                    "Principal": {"AWS": format!("arn:aws:iam::{SENDER_ACCOUNT}:root")},
                    "Action": "logs:PutSubscriptionFilter",
                    "Resource": "*",
                }],
            })
            .to_string(),
        }
    }
}

/// One `forceUpdate` row: the policy the destination already holds, the policy put next, the
/// `forceUpdate` value the put carries, and whether that put is refused.
#[derive(Debug)]
struct ForceUpdateCase {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The grant the destination already holds, or `None` for a destination holding no policy.
    held: Option<Grant>,
    /// The grant the put under test makes.
    next: Grant,
    /// `forceUpdate`, or `None` for a put that omits it ("the default of false is used").
    force_update: Option<bool>,
    /// Whether the put is refused for the missing affirmation.
    refused: bool,
}

/// Each combination of the grant held, the grant put and the affirmation: only the documented
/// one — replacing a policy that names no organization with one that grants an organization
/// ID, unaffirmed — is refused.
fn force_update_cases() -> Vec<ForceUpdateCase> {
    vec![
        ForceUpdateCase {
            label: "a first policy granting an organization, no affirmation",
            held: None,
            next: Grant::Organization,
            force_update: None,
            refused: false,
        },
        ForceUpdateCase {
            label: "a first policy granting one account, no affirmation",
            held: None,
            next: Grant::OneAccount,
            force_update: None,
            refused: false,
        },
        ForceUpdateCase {
            label: "replacing an account grant with another account grant",
            held: Some(Grant::OneAccount),
            next: Grant::TwoAccounts,
            force_update: None,
            refused: false,
        },
        ForceUpdateCase {
            label: "replacing an account grant with an organization grant, no affirmation",
            held: Some(Grant::OneAccount),
            next: Grant::Organization,
            force_update: None,
            refused: true,
        },
        ForceUpdateCase {
            label: "replacing an account grant with an organization grant, affirmation declined",
            held: Some(Grant::TwoAccounts),
            next: Grant::Organization,
            force_update: Some(false),
            refused: true,
        },
        ForceUpdateCase {
            label: "replacing an account grant with an organization grant, affirmed",
            held: Some(Grant::OneAccount),
            next: Grant::Organization,
            force_update: Some(true),
            refused: false,
        },
        ForceUpdateCase {
            label: "replacing an organization grant with another organization grant",
            held: Some(Grant::Organization),
            next: Grant::OtherOrganization,
            force_update: None,
            refused: false,
        },
        ForceUpdateCase {
            label: "replacing an organization grant with an account grant",
            held: Some(Grant::Organization),
            next: Grant::OneAccount,
            force_update: None,
            refused: false,
        },
        ForceUpdateCase {
            label: "replacing a policy that names no organization with an organization grant",
            held: Some(Grant::Nobody),
            next: Grant::Organization,
            force_update: None,
            refused: true,
        },
        ForceUpdateCase {
            label: "an affirmed put that makes no organization grant",
            held: Some(Grant::OneAccount),
            next: Grant::TwoAccounts,
            force_update: Some(true),
            refused: false,
        },
    ]
}
