// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDestination`: removing one of the logical cross-account
//! destinations an account holds.
//!
//! The rule under test: a destination is identified by its `destinationName` alone, so
//! `DeleteDestination` removes the ONE destination that name identifies and nothing else —
//! afterwards `DescribeDestinations` reports no destination under that name, every other
//! destination the account holds is reported exactly as it was, and the state only that
//! destination named (the access policy `PutDestinationPolicy` attached to it and the tag set
//! assigned to the resource) is gone with it, so a later `PutDestination` under the same name
//! creates a FRESH destination rather than reviving the old one. A name the account holds no
//! destination under names nothing to delete, which is refused `ResourceNotFoundException`
//! carrying this service's missing-resource sentence for the destination kind. "This
//! operation does not delete the physical resource encapsulated by the destination", and it
//! "eventually disables" rather than deletes the subscription filters that publish to it, so
//! the filters of the log groups they are associated with survive the delete.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: destination names
//! in every character class the model's `DestinationName` admits and at both its published
//! length bounds, destinations carrying no tags, one tag and several, destinations with and
//! without an access policy, and — for the refusal — names never created, a name deleted
//! twice, a name that is a PREFIX of one the account holds, and one differing from a held
//! name only in case.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::destination_arn;

use crate::harness::{now_ms, Logs, REGION};

/// The operation that creates and updates a destination.
const PUT: &str = "PutDestination";

/// The operation under test.
const DELETE: &str = "DeleteDestination";

/// The operation that reports the destinations an account holds.
const DESCRIBE: &str = "DescribeDestinations";

/// The operation that attaches an access policy to one destination.
const PUT_POLICY: &str = "PutDestinationPolicy";

/// The operation that reports a destination's own tag set (`PutDestination$tags`: "An
/// optional list of key-value pairs to associate with the resource").
const LIST_TAGS: &str = "ListTagsForResource";

/// The operations that configure and report a log group's subscription filters, which
/// publish to a destination but belong to the group they are associated with.
const CREATE_GROUP: &str = "CreateLogGroup";
const PUT_FILTER: &str = "PutSubscriptionFilter";
const DESCRIBE_FILTERS: &str = "DescribeSubscriptionFilters";

/// The refusal `DeleteDestination` models for a destination the account does not hold.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The refusal `PutDestinationPolicy` models for one (it declares no
/// `ResourceNotFoundException`).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The sentence this service states when the resource an operation names is not there, for
/// the destination kind — what an operation modelling `ResourceNotFoundException` ("The
/// specified resource does not exist.") refuses a missing destination with.
const DESTINATION_NOT_FOUND_MESSAGE: &str = "The specified destination does not exist.";

/// What an operation that models NO `ResourceNotFoundException` refuses a destination the
/// account does not hold with: `InvalidParameterException` carries no sentence of its own
/// ("A parameter is specified incorrectly"), so the message names the rejected
/// `destinationName`. Asserted per name in `tests/all/missing_destination_refusal.rs`.
fn missing_destination_message(destination_name: &str) -> String {
    format!("Destination with name {destination_name} does not exist")
}

/// One physical target of each vendor a destination can deliver to, and the roles it can be
/// created with — a destination is "used only to create destinations for cross-account
/// subscriptions", so one target is in another account, while both roles are in the caller's
/// own: a role is passed to this service by the account that holds it.
const KINESIS_ARN: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/log-events";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/log-events";
const OTHER_FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/retired";
const CROSS_ACCOUNT_KINESIS_ARN: &str = "arn:aws:kinesis:us-east-1:210987654321:stream/other";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CWLtoKinesisRole";
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/CWLtoKinesis";

/// An access policy document — "An IAM policy document that governs which Amazon Web
/// Services accounts can create subscription filters against this destination."
const ACCESS_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"cross-account","Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"logs:PutSubscriptionFilter","Resource":"*"}]}"#;

/// One destination to stand up and then delete.
#[derive(Debug, Clone)]
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `destinationName` (`DestinationName`, `@length` 1..512, `^[^:*]*$`).
    name: String,
    /// `targetArn` (`TargetArn`, `@length` 1..).
    target_arn: &'static str,
    /// `roleArn` (`RoleArn`, `@length` 1..).
    role_arn: &'static str,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
    /// Whether `PutDestinationPolicy` attaches an access policy to this destination.
    access_policy: bool,
}

impl Case {
    fn of(label: &'static str, name: &str, target_arn: &'static str, role_arn: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            target_arn,
            role_arn,
            tags: Vec::new(),
            access_policy: false,
        }
    }

    fn with_tags(mut self, tags: &[(&'static str, &'static str)]) -> Self {
        self.tags = tags.to_vec();
        self
    }

    fn with_access_policy(mut self) -> Self {
        self.access_policy = true;
        self
    }

    /// The request body that creates this destination.
    fn put_body(&self) -> Value {
        let mut body = json!({
            "destinationName": self.name,
            "targetArn": self.target_arn,
            "roleArn": self.role_arn,
        });
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect::<Map<String, Value>>(),
            );
        }
        body
    }

    /// Whether this destination's ARN can be presented to a tagging operation at all: the
    /// `resourceArn` those take is the model's `AmazonResourceName` (`@length` 1..1011,
    /// `^[\w+=/:,.@-]*$`), so the ARN of a destination whose name carries a character outside
    /// that set — a space, say — is refused at the boundary before any resource is looked up.
    /// `DestinationName`'s own pattern (`^[^:*]*$`) is far wider, so the two do not agree and
    /// the tag set of such a destination is simply not addressable.
    fn arn_addressable(&self) -> bool {
        self.name
            .chars()
            .all(|c| c.is_alphanumeric() || "_+=/:,.@-".contains(c))
    }

    /// The tag set the service must report for this destination while it exists.
    fn expected_tags(&self) -> Value {
        Value::Object(
            self.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect::<Map<String, Value>>(),
        )
    }
}

/// The corpus the cases below delete: names in every class the model admits and at both
/// published length bounds, tag sets of every size, and destinations with and without an
/// access policy.
fn cases() -> Vec<Case> {
    vec![
        Case::of("a plain name, no tags and no policy", "kinesis-dest", KINESIS_ARN, ROLE_ARN),
        Case::of(
            "a name carrying separators and punctuation, one tag",
            "team/one.dest-2_final+x",
            FIREHOSE_ARN,
            ROLE_ARN,
        )
        .with_tags(&[("team", "platform")]),
        Case::of(
            "a name outside ASCII, several tags and a policy",
            "ログ-送信先",
            OTHER_FIREHOSE_ARN,
            ROLE_ARN,
        )
        .with_tags(&[("env", "prod"), ("owner", "obs"), ("tier", "gold")])
        .with_access_policy(),
        Case::of(
            "a name carrying a space, a cross-account target, with a policy",
            "cross account dest",
            CROSS_ACCOUNT_KINESIS_ARN,
            OTHER_ROLE_ARN,
        )
        .with_access_policy(),
        // Both published bounds of `DestinationName` (`@length` 1..512).
        Case::of("the shortest name the model admits", "a", KINESIS_ARN, ROLE_ARN),
        Case::of(
            "the longest name the model admits",
            &"z".repeat(512),
            FIREHOSE_ARN,
            ROLE_ARN,
        )
        .with_tags(&[("bound", "max")]),
    ]
}

/// Stand up one case's destination, with its tags and its access policy.
async fn create(logs: &Logs, case: &Case) {
    logs.ok(PUT, &case.put_body()).await;
    if case.access_policy {
        logs.ok(
            PUT_POLICY,
            &json!({"destinationName": case.name, "accessPolicy": ACCESS_POLICY}),
        )
        .await;
    }
}

/// The one destination the service reports under `name`, or `None` once it holds none —
/// read through the enumeration a client reads it through, following every page.
async fn described(logs: &Logs, name: &str) -> Option<Value> {
    let mut token: Option<String> = None;
    loop {
        let mut body = Map::new();
        body.insert("DestinationNamePrefix".to_string(), json!(name));
        if let Some(token) = token.as_ref() {
            body.insert("nextToken".to_string(), json!(token));
        }
        let body = Value::Object(body);
        let response = logs.ok(DESCRIBE, &body).await;
        let page = response
            .get("destinations")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{DESCRIBE} {body}: must report destinations, got {response}"))
            .clone();
        if let Some(found) = page
            .into_iter()
            .find(|destination| destination.get("destinationName") == Some(&json!(name)))
        {
            return Some(found);
        }
        match response.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return None,
        }
    }
}

/// A delete that must succeed.
async fn delete(logs: &Logs, name: &str) {
    logs.ok(DELETE, &json!({"destinationName": name})).await;
}

/// A delete that removes the destination its name names, leaves every other destination
/// exactly as it was, and takes the destination's access policy and tag set with it.
#[tokio::test]
async fn a_delete_removes_only_the_named_destination() {
    let logs = Logs::new();
    let cases = cases();

    for case in &cases {
        create(&logs, case).await;
    }

    // What the service reports for each destination before any of them is deleted: the
    // record every surviving destination must still be reported as.
    let mut held: Vec<(Case, Value)> = Vec::new();
    for case in &cases {
        let reported = described(&logs, &case.name).await.unwrap_or_else(|| {
            panic!("case {:?}: the destination must be reported once created", case.label)
        });
        held.push((case.clone(), reported));
    }

    while let Some((case, _)) = held.first().cloned() {
        held.remove(0);
        delete(&logs, &case.name).await;

        // The destination the delete named is gone, so nothing reports it any more.
        let after = described(&logs, &case.name).await;
        assert!(
            after.is_none(),
            "case {:?}: expected no destination reported under {:?} after the delete, got {after:?}",
            case.label,
            case.name
        );

        // A second delete of the same destination names nothing, so it is refused.
        let (code, message) = logs
            .refusal(DELETE, &json!({"destinationName": case.name}))
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (NOT_FOUND, DESTINATION_NOT_FOUND_MESSAGE),
            "case {:?}: expected deleting the same destination twice to be refused \
             ({NOT_FOUND}, {DESTINATION_NOT_FOUND_MESSAGE:?}), got ({code}, {message:?})",
            case.label
        );

        // The tag set assigned to the resource went with the resource, so the ARN it was
        // known by names no destination to report tags for.
        if case.arn_addressable() {
            let arn = destination_arn(REGION, &case.name);
            let (code, message) = logs.refusal(LIST_TAGS, &json!({"resourceArn": arn})).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (NOT_FOUND, DESTINATION_NOT_FOUND_MESSAGE),
                "case {:?}: expected the deleted destination's tags to be unreadable \
                 ({NOT_FOUND}, {DESTINATION_NOT_FOUND_MESSAGE:?}), got ({code}, {message:?})",
                case.label
            );
        }

        // An access policy is "associated with an existing destination", and this one no
        // longer exists, so there is nothing to attach one to — and the refusal names the
        // destination it was asked for.
        let expected = missing_destination_message(&case.name);
        let (code, message) = logs
            .refusal(
                PUT_POLICY,
                &json!({"destinationName": case.name, "accessPolicy": ACCESS_POLICY}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, expected.as_str()),
            "case {:?}: expected a policy put against the deleted destination to be refused \
             ({INVALID_PARAMETER}, {expected:?}), got ({code}, {message:?})",
            case.label
        );

        // Every destination not named by the delete is reported exactly as it was, tag set
        // and access policy included.
        for (survivor, expected) in &held {
            let read = described(&logs, &survivor.name).await.unwrap_or_else(|| {
                panic!(
                    "case {:?}: deleting {:?} must leave {:?} reported",
                    survivor.label, case.name, survivor.name
                )
            });
            assert_eq!(
                &read, expected,
                "case {:?}: after deleting {:?}, expected {expected}, got {read}",
                survivor.label, case.name
            );
            if survivor.arn_addressable() {
                let tags = logs
                    .ok(
                        LIST_TAGS,
                        &json!({"resourceArn": destination_arn(REGION, &survivor.name)}),
                    )
                    .await;
                let expected_tags = survivor.expected_tags();
                assert_eq!(
                    tags.get("tags"),
                    Some(&expected_tags),
                    "case {:?}: after deleting {:?}, expected tags {expected_tags}, got {tags}",
                    survivor.label,
                    case.name
                );
            }
        }
    }

    // Once every destination has been deleted the account holds none of them.
    let remaining = logs.ok(DESCRIBE, &json!({})).await;
    let reported = remaining
        .get("destinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE}: must report destinations, got {remaining}"));
    assert!(
        reported.is_empty(),
        "case {:?}: expected no destination left after deleting every one, got {reported:?}",
        "every destination deleted"
    );
}

/// A delete naming a destination the account does not hold is refused, and refusing it
/// changes nothing: the destination whose name the refused one resembles is still there.
#[tokio::test]
async fn deleting_a_destination_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();

    // One destination the account DOES hold, so the names below are refused for naming no
    // destination rather than because the account holds none at all.
    let held = Case::of("the one destination held", "kinesis-dest", KINESIS_ARN, ROLE_ARN)
        .with_tags(&[("team", "platform")]);
    create(&logs, &held).await;
    let before = described(&logs, &held.name).await.unwrap_or_else(|| {
        panic!("case {:?}: the destination must be reported once created", held.label)
    });

    // A destination created and then deleted, so the second delete of it names nothing.
    let gone = Case::of("a destination already deleted", "retired-dest", OTHER_FIREHOSE_ARN, ROLE_ARN);
    create(&logs, &gone).await;
    delete(&logs, &gone.name).await;

    let missing: Vec<(&str, String)> = vec![
        ("a plain name never created", "no-such-dest".to_string()),
        ("a name carrying punctuation never created", "team/none.dest-9".to_string()),
        ("a name outside ASCII never created", "存在しない-送信先".to_string()),
        ("a name carrying a space never created", "no such dest".to_string()),
        ("the shortest name the model admits", "q".to_string()),
        ("the longest name the model admits", "y".repeat(512)),
        ("a name already deleted", gone.name.clone()),
        // A PREFIX of the held destination's name names no destination of its own: the
        // delete identifies a destination by its whole name.
        ("a prefix of a held name", "kinesis".to_string()),
        // So does a name differing from a held one only in case.
        ("a held name in another case", "Kinesis-Dest".to_string()),
    ];

    for (label, name) in &missing {
        let (code, message) = logs
            .refusal(DELETE, &json!({"destinationName": name}))
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (NOT_FOUND, DESTINATION_NOT_FOUND_MESSAGE),
            "case {label:?}: expected deleting {name:?} to be refused \
             ({NOT_FOUND}, {DESTINATION_NOT_FOUND_MESSAGE:?}), got ({code}, {message:?})"
        );

        // The refused delete removed nothing.
        let read = described(&logs, &held.name)
            .await
            .unwrap_or_else(|| panic!("case {label:?}: the held destination must still be reported"));
        assert_eq!(
            read, before,
            "case {label:?}: a refused delete must leave the held destination at {before}, got {read}"
        );
    }
}

/// A put under a name whose destination was deleted creates a NEW destination: it carries
/// the target and role of the new put, no access policy and no tags, and its creation time
/// is the instant it was created rather than the deleted one's.
#[tokio::test]
async fn a_put_after_a_delete_creates_a_fresh_destination() {
    let logs = Logs::new();

    let original = Case::of("the destination first created", "kinesis-dest", KINESIS_ARN, ROLE_ARN)
        .with_tags(&[("env", "prod"), ("team", "platform")])
        .with_access_policy();
    create(&logs, &original).await;
    let first = described(&logs, &original.name).await.unwrap_or_else(|| {
        panic!("case {:?}: the destination must be reported once created", original.label)
    });
    let first_created = first
        .get("creationTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("a destination carries creationTime, got {first}"));

    delete(&logs, &original.name).await;

    // A different target and role, so a record the delete failed to remove would show.
    let recreated = Case::of(
        "the destination put again after the delete",
        &original.name,
        FIREHOSE_ARN,
        OTHER_ROLE_ARN,
    );
    let before = now_ms();
    logs.ok(PUT, &recreated.put_body()).await;
    let after = now_ms();

    let read = described(&logs, &recreated.name).await.unwrap_or_else(|| {
        panic!("case {:?}: the destination must be reported again", recreated.label)
    });
    assert_eq!(
        read.get("targetArn"),
        Some(&json!(FIREHOSE_ARN)),
        "case {:?}: expected the new put's targetArn, got {read}",
        recreated.label
    );
    assert_eq!(
        read.get("roleArn"),
        Some(&json!(OTHER_ROLE_ARN)),
        "case {:?}: expected the new put's roleArn, got {read}",
        recreated.label
    );
    // The ARN is derived from the Region and the name, so it is the same ARN as before.
    assert_eq!(
        read.get("arn"),
        Some(&json!(destination_arn(REGION, &recreated.name))),
        "case {:?}: expected the destination's derived ARN, got {read}",
        recreated.label
    );
    // "By default, PutDestination does not set any access policy with the destination" — the
    // policy the deleted destination carried went with it.
    assert_eq!(
        read.get("accessPolicy"),
        None,
        "case {:?}: the recreated destination carries no access policy, got {read}",
        recreated.label
    );
    let created = read
        .get("creationTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("a destination carries creationTime, got {read}"));
    assert!(
        (before..=after).contains(&created),
        "case {:?}: expected a creationTime in [{before}, {after}] (the deleted one's was \
         {first_created}), got {created}",
        recreated.label
    );
    // The tag set went with the resource too, so the fresh destination carries none.
    let tags = logs
        .ok(
            LIST_TAGS,
            &json!({"resourceArn": destination_arn(REGION, &recreated.name)}),
        )
        .await;
    assert_eq!(
        tags.get("tags"),
        Some(&json!({})),
        "case {:?}: expected the recreated destination to carry no tags, got {tags}",
        recreated.label
    );
}

/// Deleting a destination leaves the subscription filters that publish to it where they
/// are: a filter belongs to the log group it is associated with, and the delete "eventually
/// disables" rather than deletes it.
#[tokio::test]
async fn deleting_a_destination_leaves_the_subscription_filters_that_publish_to_it() {
    let logs = Logs::new();

    // The destination carries an access policy, because a filter can only be registered
    // against a destination whose access policy authorizes the registration: "the
    // destination owner must call PutDestinationPolicy after PutDestination".
    let case = Case::of(
        "the destination a filter publishes to",
        "kinesis-dest",
        KINESIS_ARN,
        ROLE_ARN,
    )
    .with_access_policy();
    create(&logs, &case).await;
    let arn = destination_arn(REGION, &case.name);

    // Two log groups, each with a filter publishing to the destination, so the delete is
    // asserted over more than one owner.
    let groups = ["/aws/app/one", "/aws/app/two"];
    for group in groups {
        logs.ok(CREATE_GROUP, &json!({"logGroupName": group})).await;
        logs.ok(
            PUT_FILTER,
            &json!({
                "logGroupName": group,
                "filterName": "to-destination",
                "filterPattern": "ERROR",
                "destinationArn": arn,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    }

    let reported = |group: &'static str| {
        let logs = &logs;
        async move {
            logs.ok(DESCRIBE_FILTERS, &json!({"logGroupName": group}))
                .await
                .get("subscriptionFilters")
                .cloned()
                .unwrap_or_else(|| panic!("case {group:?}: must report subscriptionFilters"))
        }
    };

    let mut before = Vec::new();
    for group in groups {
        before.push(reported(group).await);
    }

    delete(&logs, &case.name).await;

    for (group, expected) in groups.into_iter().zip(before) {
        let after = reported(group).await;
        assert_eq!(
            after, expected,
            "case {group:?}: deleting the destination must leave the group's subscription \
             filters at {expected}, got {after}"
        );
    }
}
