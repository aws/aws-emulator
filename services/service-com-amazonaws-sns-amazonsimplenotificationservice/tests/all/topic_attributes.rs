// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the topic ATTRIBUTE slice: the map `GetTopicAttributes`
//! projects, the write paths that fill it (`SetTopicAttributes`, `AddPermission`,
//! `RemovePermission`) and the two documents SNS computes into it (the access policy
//! and the effective delivery policy).
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use super::topics_subscriptions_publish::{attribute_entries, attribute_map, Answer, Sns};

/// The account id an ARN this emulator minted carries: the fifth field of
/// `arn:aws:sns:<region>:<account>:<name>`.
fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// The JSON document an attribute value carries, or a panic naming the case.
fn json_of(case: &str, name: &str, value: &str) -> serde_json::Value {
    serde_json::from_str(value)
        .unwrap_or_else(|e| panic!("case {case:?}: {name} must be a JSON document, got {value:?}: {e}"))
}

/// Set one topic attribute, asserting the call was accepted.
async fn set(sns: &Sns, case: &str, arn: &str, name: &str, value: &str) {
    let answer = sns
        .call(
            "SetTopicAttributes",
            &[
                ("TopicArn", arn),
                ("AttributeName", name),
                ("AttributeValue", value),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: SetTopicAttributes {name} expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// A read of one topic's attributes, asserting the call was answered.
async fn read(sns: &Sns, case: &str, arn: &str) -> Answer {
    let answer = sns.call("GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetTopicAttributes expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
}

/// One row of the projection table: the case, the topic's name, the attributes set on
/// it, and the attribute keys SNS reports for it, in order.
type ProjectionCase<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);

/// GetTopicAttributes reports ONE entry per attribute, each key paired with its own
/// value, in the order SNS reports them — and the same order on every call, whatever
/// the topic holds. The rows differ in which attributes the topic carries, which is
/// what the order generalises over.
#[tokio::test]
async fn topic_attributes_are_reported_in_one_stable_order() {
    let sns = Sns::new();
    let cases: &[ProjectionCase<'_>] = &[
        (
            "a topic with nothing set",
            "order-plain",
            &[],
            &[
                "Policy",
                "Owner",
                "SubscriptionsPending",
                "TopicArn",
                "EffectiveDeliveryPolicy",
                "SubscriptionsConfirmed",
                "DisplayName",
                "SubscriptionsDeleted",
            ],
        ),
        (
            "a topic with a display name",
            "order-display",
            &[("DisplayName", "probe-display-name")],
            &[
                "Policy",
                "Owner",
                "SubscriptionsPending",
                "TopicArn",
                "EffectiveDeliveryPolicy",
                "SubscriptionsConfirmed",
                "DisplayName",
                "SubscriptionsDeleted",
            ],
        ),
        (
            "an encrypted topic",
            "order-encrypted",
            &[("KmsMasterKeyId", "alias/aws/sns")],
            &[
                "Policy",
                "Owner",
                "SubscriptionsPending",
                "KmsMasterKeyId",
                "TopicArn",
                "EffectiveDeliveryPolicy",
                "SubscriptionsConfirmed",
                "DisplayName",
                "SubscriptionsDeleted",
            ],
        ),
        (
            "an encrypted topic with a delivery policy and a display name",
            "order-everything",
            &[
                ("KmsMasterKeyId", "alias/aws/sns"),
                ("DisplayName", "probe-display-name"),
                ("DeliveryPolicy", r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":7}}}"#),
                ("TracingConfig", "Active"),
            ],
            &[
                "Policy",
                "Owner",
                "SubscriptionsPending",
                "KmsMasterKeyId",
                "TopicArn",
                "EffectiveDeliveryPolicy",
                "SubscriptionsConfirmed",
                "DisplayName",
                "SubscriptionsDeleted",
                // Every other attribute the topic holds follows, ordered by name.
                "DeliveryPolicy",
                "TracingConfig",
            ],
        ),
    ];

    for (case, name, configured, expected_keys) in cases {
        let arn = sns.topic(name).await;
        for (attribute, value) in *configured {
            set(&sns, case, &arn, attribute, value).await;
        }

        let answer = read(&sns, case, &arn).await;
        let entries = attribute_entries(&answer);
        let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys, *expected_keys,
            "case {case:?}: expected the attribute keys {expected_keys:?}, got {keys:?}"
        );

        // Two reads of one topic answer the same document, entry for entry: the order
        // is SNS's, not a hash map's.
        let again = read(&sns, case, &arn).await;
        assert_eq!(
            attribute_entries(&again),
            entries,
            "case {case:?}: two reads of one topic must report the same entries in the same order"
        );

        // Each key carries its OWN value, so every one is the kind of value its
        // attribute holds.
        let held = attribute_map(&answer);
        assert_eq!(
            held.get("TopicArn").map(String::as_str),
            Some(arn.as_str()),
            "case {case:?}: TopicArn must be the topic's ARN, got {held:?}"
        );
        assert_eq!(
            held.get("Owner").map(String::as_str),
            Some(account_of(&arn).as_str()),
            "case {case:?}: Owner must be the account that owns the topic, got {held:?}"
        );
        for counter in ["SubscriptionsConfirmed", "SubscriptionsPending", "SubscriptionsDeleted"] {
            let value = held.get(counter).cloned().unwrap_or_default();
            assert!(
                value.parse::<u64>().is_ok(),
                "case {case:?}: {counter} must be a count, got {value:?}"
            );
        }
        let policy = json_of(case, "Policy", &held["Policy"]);
        assert!(
            policy.get("Statement").is_some_and(serde_json::Value::is_array),
            "case {case:?}: Policy must be an access policy document, got {policy}"
        );
        let effective = json_of(case, "EffectiveDeliveryPolicy", &held["EffectiveDeliveryPolicy"]);
        assert!(
            effective
                .pointer("/http/defaultHealthyRetryPolicy/numRetries")
                .is_some(),
            "case {case:?}: EffectiveDeliveryPolicy must be a delivery policy document, got {effective}"
        );
        for (attribute, value) in *configured {
            assert_eq!(
                held.get(*attribute).map(String::as_str),
                Some(*value),
                "case {case:?}: {attribute} must report the value set on the topic, got {held:?}"
            );
        }
    }
}

/// SetTopicAttributes persists the named attribute on the topic, and a later
/// GetTopicAttributes reports it: a set-then-get round trip over every attribute class
/// the operation documents — an id, a name, a JSON document, a byte count, a mode, a
/// role ARN and a sample rate.
#[tokio::test]
async fn set_topic_attributes_round_trips_every_documented_attribute() {
    let sns = Sns::new();
    let arn = sns.topic("set-and-get").await;
    let policy_document = serde_json::json!({
        "Version": "2008-10-17",
        "Id": "probe",
        "Statement": [{
            "Sid": "probe-statement",
            "Effect": "Allow",
            "Principal": { "AWS": "*" },
            "Action": "SNS:Publish",
            "Resource": arn,
        }]
    })
    .to_string();
    // The policy grammar accepts a single statement OBJECT as well as a list of them.
    let single_statement_policy = serde_json::json!({
        "Version": "2008-10-17",
        "Statement": {
            "Sid": "one-statement",
            "Effect": "Allow",
            "Principal": { "AWS": "*" },
            "Action": "SNS:Publish",
            "Resource": arn,
        }
    })
    .to_string();
    // The attribute catalog from the `SetTopicAttributesInput$AttributeName` doc; the
    // FIFO-only names are covered by the FIFO row of the refusal table below.
    let cases: &[(&str, &str, &str)] = &[
        ("a display name", "DisplayName", "probe-display-name"),
        ("an encryption key id", "KmsMasterKeyId", "alias/aws/sns"),
        (
            "a delivery policy document",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":5}}}"#,
        ),
        ("an access policy document", "Policy", &policy_document),
        (
            "an access policy whose Statement is one statement",
            "Policy",
            &single_statement_policy,
        ),
        ("a byte count at the documented floor", "MaximumMessageSize", "1024"),
        ("a byte count at the documented ceiling", "MaximumMessageSize", "1048576"),
        ("a tracing mode", "TracingConfig", "Active"),
        ("a signature version", "SignatureVersion", "2"),
        (
            "a delivery-status role ARN",
            "SQSSuccessFeedbackRoleArn",
            "arn:aws:iam::000000000000:role/sns-logs",
        ),
        ("a delivery-status sample rate", "SQSSuccessFeedbackSampleRate", "100"),
    ];

    for (case, name, value) in cases {
        set(&sns, case, &arn, name, value).await;
        let held = attribute_map(&read(&sns, case, &arn).await);
        assert_eq!(
            held.get(*name).map(String::as_str),
            Some(*value),
            "case {case:?}: {name} must read back as it was set, got {held:?}"
        );
    }

    // A request that carries no value clears the attribute: the topic reports what a
    // topic that was never given one reports.
    let answer = sns
        .call(
            "SetTopicAttributes",
            &[("TopicArn", &arn), ("AttributeName", "TracingConfig")],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "clearing an attribute: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let held = attribute_map(&read(&sns, "cleared", &arn).await);
    assert!(
        !held.contains_key("TracingConfig"),
        "a cleared attribute must not be reported, got {held:?}"
    );
}

/// SetTopicAttributes refuses a name SNS does not define for a topic, a name only
/// CreateTopic sets, a value outside a documented range and a document that is not one
/// — and a topic that does not exist.
#[tokio::test]
async fn set_topic_attributes_refuses_what_sns_refuses() {
    let sns = Sns::new();
    let arn = sns.topic("set-refusals").await;
    // (case, attribute name, value, expected status, expected awsQuery error code)
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        ("a name SNS does not define", "NotAnAttribute", "x", 400, "InvalidParameter"),
        ("a name only CreateTopic sets", "FifoTopic", "true", 400, "InvalidParameter"),
        ("an access policy that is not JSON", "Policy", "not-a-document", 400, "InvalidParameter"),
        (
            "an access policy with no statements",
            "Policy",
            r#"{"Version":"2008-10-17"}"#,
            400,
            "InvalidParameter",
        ),
        (
            "a delivery policy that is not JSON",
            "DeliveryPolicy",
            "not-a-document",
            400,
            "InvalidParameter",
        ),
        (
            "a delivery policy that is a bare scalar, not a document",
            "DeliveryPolicy",
            "5",
            400,
            "InvalidParameter",
        ),
        (
            "a delivery policy member of the wrong type",
            "DeliveryPolicy",
            r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":"five"}}}"#,
            400,
            "InvalidParameter",
        ),
        // The documented range is 1024 to 1048576; each bound is accepted above, and
        // one past each is refused here.
        ("one byte below the documented floor", "MaximumMessageSize", "1023", 400, "InvalidParameter"),
        (
            "one byte above the documented ceiling",
            "MaximumMessageSize",
            "1048577",
            400,
            "InvalidParameter",
        ),
        ("a byte count that is not a number", "MaximumMessageSize", "many", 400, "InvalidParameter"),
        // The closed value sets and the percentage bound SNS documents; each accepted
        // value is exercised by the round-trip table above.
        (
            "a tracing mode SNS does not define",
            "TracingConfig",
            "Sampled",
            400,
            "InvalidParameter",
        ),
        (
            "a signature version SNS does not define",
            "SignatureVersion",
            "3",
            400,
            "InvalidParameter",
        ),
        (
            "a sample rate at the documented floor",
            "SQSSuccessFeedbackSampleRate",
            "0",
            200,
            "",
        ),
        (
            "a sample rate one past the documented ceiling",
            "SQSSuccessFeedbackSampleRate",
            "101",
            400,
            "InvalidParameter",
        ),
        (
            "a sample rate that is not a percentage",
            "HTTPSuccessFeedbackSampleRate",
            "half",
            400,
            "InvalidParameter",
        ),
        ("a valid attribute", "DisplayName", "fine", 200, ""),
    ];

    for (case, name, value, status, code) in cases {
        let answer = sns
            .call(
                "SetTopicAttributes",
                &[
                    ("TopicArn", &arn),
                    ("AttributeName", name),
                    ("AttributeValue", value),
                ],
            )
            .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {}",
            answer.body
        );
    }

    let missing = format!("{arn}-does-not-exist");
    let answer = sns
        .call(
            "SetTopicAttributes",
            &[
                ("TopicArn", &missing),
                ("AttributeName", "DisplayName"),
                ("AttributeValue", "x"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 404,
        "a topic that does not exist: expected 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "NotFound",
        "a topic that does not exist: expected NotFound, got {}",
        answer.body
    );
}

/// One row of the delivery-policy table: the case, the `DeliveryPolicy` the topic is
/// given, and the document members that must then differ from the system default,
/// named by JSON pointer.
type DeliveryPolicyCase<'a> = (&'a str, Option<&'a str>, &'a [(&'a str, serde_json::Value)]);

/// EffectiveDeliveryPolicy is the system default delivery policy with the topic's own
/// DeliveryPolicy laid over it, member by member: a member the topic sets overrides the
/// default, and every member it leaves out keeps the default.
#[tokio::test]
async fn effective_delivery_policy_lays_the_topic_policy_over_the_system_default() {
    let sns = Sns::new();
    // Each row: the case, the DeliveryPolicy set on the topic, and the members that
    // must differ from the system default.
    let cases: &[DeliveryPolicyCase<'_>] = &[
        ("no delivery policy at all", None, &[]),
        (
            "one retry member",
            Some(r#"{"http":{"defaultHealthyRetryPolicy":{"numRetries":10}}}"#),
            &[("/http/defaultHealthyRetryPolicy/numRetries", serde_json::json!(10))],
        ),
        (
            "several retry members",
            Some(
                r#"{"http":{"defaultHealthyRetryPolicy":{"minDelayTarget":5,"maxDelayTarget":300,"backoffFunction":"exponential"}}}"#,
            ),
            &[
                ("/http/defaultHealthyRetryPolicy/minDelayTarget", serde_json::json!(5)),
                ("/http/defaultHealthyRetryPolicy/maxDelayTarget", serde_json::json!(300)),
                (
                    "/http/defaultHealthyRetryPolicy/backoffFunction",
                    serde_json::json!("exponential"),
                ),
            ],
        ),
        (
            "the subscription-override switch",
            Some(r#"{"http":{"disableSubscriptionOverrides":true}}"#),
            &[("/http/disableSubscriptionOverrides", serde_json::json!(true))],
        ),
        (
            "the default request policy",
            Some(r#"{"http":{"defaultRequestPolicy":{"headerContentType":"application/json"}}}"#),
            &[(
                "/http/defaultRequestPolicy/headerContentType",
                serde_json::json!("application/json"),
            )],
        ),
        (
            "members this emulator gives no meaning to",
            Some(
                r#"{"http":{"defaultThrottlePolicy":{"maxReceivesPerSecond":5},"defaultHealthyRetryPolicy":{"numRetries":2,"extraMember":"kept"}}}"#,
            ),
            &[
                (
                    "/http/defaultThrottlePolicy/maxReceivesPerSecond",
                    serde_json::json!(5),
                ),
                ("/http/defaultHealthyRetryPolicy/numRetries", serde_json::json!(2)),
                (
                    "/http/defaultHealthyRetryPolicy/extraMember",
                    serde_json::json!("kept"),
                ),
            ],
        ),
    ];

    // SNS's system default delivery policy: a linear retry of 3 attempts 20 seconds
    // apart, no subscription overrides, a plain-text request policy.
    let system_default: &[(&str, serde_json::Value)] = &[
        ("/http/defaultHealthyRetryPolicy/minDelayTarget", serde_json::json!(20)),
        ("/http/defaultHealthyRetryPolicy/maxDelayTarget", serde_json::json!(20)),
        ("/http/defaultHealthyRetryPolicy/numRetries", serde_json::json!(3)),
        ("/http/defaultHealthyRetryPolicy/numMaxDelayRetries", serde_json::json!(0)),
        ("/http/defaultHealthyRetryPolicy/numNoDelayRetries", serde_json::json!(0)),
        ("/http/defaultHealthyRetryPolicy/numMinDelayRetries", serde_json::json!(0)),
        (
            "/http/defaultHealthyRetryPolicy/backoffFunction",
            serde_json::json!("linear"),
        ),
        ("/http/disableSubscriptionOverrides", serde_json::json!(false)),
        (
            "/http/defaultRequestPolicy/headerContentType",
            serde_json::json!("text/plain; charset=UTF-8"),
        ),
    ];

    for (index, (case, configured, overrides)) in cases.iter().enumerate() {
        let arn = sns.topic(&format!("effective-policy-{index}")).await;
        if let Some(configured) = configured {
            set(&sns, case, &arn, "DeliveryPolicy", configured).await;
        }

        let held = attribute_map(&read(&sns, case, &arn).await);
        let effective = json_of(
            case,
            "EffectiveDeliveryPolicy",
            &held["EffectiveDeliveryPolicy"],
        );
        for (pointer, expected) in *overrides {
            assert_eq!(
                effective.pointer(pointer),
                Some(expected),
                "case {case:?}: {pointer} must carry what the topic set, got {effective}"
            );
        }
        for (pointer, expected) in system_default {
            if overrides.iter().any(|(overridden, _)| overridden == pointer) {
                continue;
            }
            assert_eq!(
                effective.pointer(pointer),
                Some(expected),
                "case {case:?}: {pointer} must keep the system default, got {effective}"
            );
        }
    }
}

/// One row of the grant table: the case, the label the grant carries, the accounts and
/// actions it names, and the `Principal.AWS` and `Action` the resulting statement must
/// carry — one value as a bare string, several as an array.
type GrantCase<'a> = (
    &'a str,
    &'a str,
    &'a [&'a str],
    &'a [&'a str],
    serde_json::Value,
    serde_json::Value,
);

/// AddPermission adds a labelled Allow statement for the named accounts and actions to
/// the topic's access policy, GetTopicAttributes reports the policy with it, and
/// RemovePermission takes that statement back out. The rows differ in how many
/// accounts and actions one grant names, which is what the statement's shape
/// generalises over.
#[tokio::test]
async fn add_permission_grants_and_remove_permission_revokes_a_labelled_statement() {
    let sns = Sns::new();
    let cases: &[GrantCase<'_>] = &[
        (
            "one account, one action",
            "CrossAccountPublish",
            &["111122223333"],
            &["Publish"],
            serde_json::json!("arn:aws:iam::111122223333:root"),
            serde_json::json!("SNS:Publish"),
        ),
        (
            "two accounts, one action",
            "TwoAccounts",
            &["111122223333", "444455556666"],
            &["Publish"],
            serde_json::json!([
                "arn:aws:iam::111122223333:root",
                "arn:aws:iam::444455556666:root"
            ]),
            serde_json::json!("SNS:Publish"),
        ),
        (
            "one account, three actions",
            "ThreeActions",
            &["111122223333"],
            &["Publish", "Subscribe", "GetTopicAttributes"],
            serde_json::json!("arn:aws:iam::111122223333:root"),
            serde_json::json!(["SNS:Publish", "SNS:Subscribe", "SNS:GetTopicAttributes"]),
        ),
    ];

    for (index, (case, label, accounts, actions, principal, action)) in cases.iter().enumerate() {
        let arn = sns.topic(&format!("permissions-{index}")).await;

        // The policy of a topic nobody has granted anything on: SNS's default document.
        let before = attribute_map(&read(&sns, case, &arn).await);
        let before = json_of(case, "Policy", &before["Policy"]);
        assert_eq!(
            before.pointer("/Statement/0/Sid"),
            Some(&serde_json::json!("__default_statement_ID")),
            "case {case:?}: a fresh topic must report the default policy, got {before}"
        );

        let mut params: Vec<(&str, &str)> = vec![("TopicArn", &arn), ("Label", label)];
        let account_params: Vec<(String, &str)> = accounts
            .iter()
            .enumerate()
            .map(|(at, account)| (format!("AWSAccountId.member.{}", at + 1), *account))
            .collect();
        let action_params: Vec<(String, &str)> = actions
            .iter()
            .enumerate()
            .map(|(at, action)| (format!("ActionName.member.{}", at + 1), *action))
            .collect();
        for (name, value) in account_params.iter().chain(action_params.iter()) {
            params.push((name.as_str(), value));
        }
        let answer = sns.call("AddPermission", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: AddPermission expected 200, got {} / {}",
            answer.status, answer.body
        );

        let granted = attribute_map(&read(&sns, case, &arn).await);
        let policy = json_of(case, "Policy", &granted["Policy"]);
        let statements = policy["Statement"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Policy must carry a Statement list: {policy}"));
        assert_eq!(
            statements.len(),
            2,
            "case {case:?}: the grant must be added beside the default statement, got {policy}"
        );
        let added = statements
            .iter()
            .find(|statement| statement["Sid"] == serde_json::json!(label))
            .unwrap_or_else(|| panic!("case {case:?}: no statement labelled {label}: {policy}"));
        assert_eq!(
            added.get("Effect"),
            Some(&serde_json::json!("Allow")),
            "case {case:?}: the added statement must allow, got {added}"
        );
        assert_eq!(
            added.pointer("/Principal/AWS"),
            Some(principal),
            "case {case:?}: expected the principal {principal}, got {added}"
        );
        assert_eq!(
            added.get("Action"),
            Some(action),
            "case {case:?}: expected the action {action}, got {added}"
        );
        assert_eq!(
            added.get("Resource"),
            Some(&serde_json::json!(arn)),
            "case {case:?}: the grant must be on this topic alone, got {added}"
        );

        // A label identifies ONE statement, so a label the policy already carries one for
        // names a grant that has already been made: a repeat of the SAME grant asks for the
        // state the topic is already in and is served, while a DIFFERENT grant under that
        // label would have to replace the statement and is refused. Either way the policy is
        // left exactly as it was.
        let changed: Vec<(&str, &str)> = vec![
            ("TopicArn", &arn),
            ("Label", label),
            ("AWSAccountId.member.1", "999988887777"),
            ("ActionName.member.1", "Subscribe"),
        ];
        for (repeat, request, expected_status) in [
            ("the same grant", &params, 200),
            ("a different grant", &changed, 400),
        ] {
            let again = sns.call("AddPermission", request).await;
            assert_eq!(
                again.status, expected_status,
                "case {case:?}: a repeated label ({repeat}) expected {expected_status}, got \
                 {} / {}",
                again.status, again.body
            );
            if expected_status == 400 {
                assert_eq!(
                    again.code(),
                    "InvalidParameter",
                    "case {case:?}: a repeated label ({repeat}) expected the InvalidParameter \
                     code, got {}",
                    again.body
                );
            }
            let repeated = attribute_map(&read(&sns, case, &arn).await);
            let repeated = json_of(case, "Policy", &repeated["Policy"]);
            assert_eq!(
                repeated, policy,
                "case {case:?}: a repeated grant ({repeat}) must leave the policy as it \
                 was, got {repeated}"
            );
        }

        // And RemovePermission takes exactly that statement back out.
        let removed = sns
            .call("RemovePermission", &[("TopicArn", &arn), ("Label", label)])
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: RemovePermission expected 200, got {} / {}",
            removed.status, removed.body
        );
        let after = attribute_map(&read(&sns, case, &arn).await);
        let after = json_of(case, "Policy", &after["Policy"]);
        assert_eq!(
            after.pointer("/Statement/0/Sid"),
            Some(&serde_json::json!("__default_statement_ID")),
            "case {case:?}: the default statement must survive the revoke, got {after}"
        );
        assert_eq!(
            after["Statement"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the labelled statement must be gone, got {after}"
        );

        // Revoking a label the policy does not carry asks for a state the topic is
        // already in.
        let twice = sns
            .call("RemovePermission", &[("TopicArn", &arn), ("Label", label)])
            .await;
        assert_eq!(
            twice.status, 200,
            "case {case:?}: a repeated revoke expected 200, got {} / {}",
            twice.status, twice.body
        );
    }
}

/// Editing a caller's access policy keeps the parts of the document this emulator
/// gives no meaning to: a top-level member and a statement member it does not model
/// both survive a grant, beside the statement the grant adds.
#[tokio::test]
async fn a_grant_keeps_the_parts_of_a_caller_policy_it_does_not_model() {
    let sns = Sns::new();
    let case = "a caller policy carrying unmodelled members";
    let arn = sns.topic("policy-passthrough").await;
    let policy = serde_json::json!({
        "Version": "2012-10-17",
        "Id": "caller-policy",
        "UnmodelledDocumentMember": { "kept": true },
        "Statement": [{
            "Sid": "caller-statement",
            "Effect": "Allow",
            "NotPrincipal": { "AWS": "arn:aws:iam::111122223333:root" },
            "Action": "SNS:Publish",
            "Resource": arn,
            "UnmodelledStatementMember": "kept",
        }]
    })
    .to_string();
    set(&sns, case, &arn, "Policy", &policy).await;

    let answer = sns
        .call(
            "AddPermission",
            &[
                ("TopicArn", &arn),
                ("Label", "Grant"),
                ("AWSAccountId.member.1", "111122223333"),
                ("ActionName.member.1", "Publish"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: AddPermission expected 200, got {} / {}",
        answer.status, answer.body
    );

    let held = attribute_map(&read(&sns, case, &arn).await);
    let edited = json_of(case, "Policy", &held["Policy"]);
    assert_eq!(
        edited.get("UnmodelledDocumentMember"),
        Some(&serde_json::json!({ "kept": true })),
        "case {case:?}: a top-level member must survive the grant, got {edited}"
    );
    assert_eq!(
        edited.pointer("/Statement/0/UnmodelledStatementMember"),
        Some(&serde_json::json!("kept")),
        "case {case:?}: a statement member must survive the grant, got {edited}"
    );
    assert_eq!(
        edited.pointer("/Statement/0/NotPrincipal/AWS"),
        Some(&serde_json::json!("arn:aws:iam::111122223333:root")),
        "case {case:?}: a statement's NotPrincipal must survive the grant, got {edited}"
    );
    assert_eq!(
        edited["Statement"].as_array().map(Vec::len),
        Some(2),
        "case {case:?}: the grant must be added beside the caller's statement, got {edited}"
    );
}

/// AddPermission and RemovePermission refuse a topic that does not exist, and
/// AddPermission refuses a principal that is not an account id.
#[tokio::test]
async fn permission_operations_refuse_what_sns_refuses() {
    let sns = Sns::new();
    let arn = sns.topic("permission-refusals").await;
    let missing = format!("{arn}-does-not-exist");

    // (case, account id, expected status, expected error code)
    let cases: &[(&str, &str, u16, &str)] = &[
        ("a 12 digit account id", "111122223333", 200, ""),
        ("an account id that is too short", "11112222333", 400, "InvalidParameter"),
        ("an account id that is too long", "1111222233334", 400, "InvalidParameter"),
        ("an account id that is not digits", "11112222333x", 400, "InvalidParameter"),
        ("an ARN rather than an account id", "arn:aws:iam::111122223333:root", 400, "InvalidParameter"),
    ];

    for (index, (case, account, status, code)) in cases.iter().enumerate() {
        let label = format!("label-{index}");
        let answer = sns
            .call(
                "AddPermission",
                &[
                    ("TopicArn", &arn),
                    ("Label", &label),
                    ("AWSAccountId.member.1", account),
                    ("ActionName.member.1", "Publish"),
                ],
            )
            .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {}",
            answer.body
        );
    }

    for action in ["AddPermission", "RemovePermission"] {
        let mut params: Vec<(&str, &str)> =
            vec![("TopicArn", &missing), ("Label", "on-a-missing-topic")];
        if action == "AddPermission" {
            params.push(("AWSAccountId.member.1", "111122223333"));
            params.push(("ActionName.member.1", "Publish"));
        }
        let answer = sns.call(action, &params).await;
        assert_eq!(
            answer.status, 404,
            "case {action:?}: a topic that does not exist expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "NotFound",
            "case {action:?}: expected NotFound, got {}",
            answer.body
        );
    }
}
