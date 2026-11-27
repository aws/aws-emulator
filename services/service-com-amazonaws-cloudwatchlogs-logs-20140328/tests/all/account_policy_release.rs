// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `PutAccountPolicy` judges against the policies the account
//! CURRENTLY holds, and for what a refused put leaves behind.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rules under test:
//!
//! * every judgement a put makes about the policies the account already holds — the per
//!   type quota ("Each account can have ... either one account-level ... policy that
//!   applies to all log groups, or up to N policies that are each scoped ...") and the
//!   rule that "no two of them" may select the same log groups — is computed from the
//!   policies held at the instant of the put, so the delete that removes a policy gives
//!   back both the quota slot it occupied and the log groups its `selectionCriteria`
//!   selected;
//! * a put the operation refuses stores nothing and changes nothing: a policy already
//!   stored under the `(policyType, policyName)` pair being put is reported afterwards
//!   exactly as it was, down to its `lastUpdatedTime`.
//!
//! The tables span what those rules generalise over — the policy type, whether the policy
//! is scoped, which criteria dimension two policies collide on, and which sentence the
//! refused put breaks. A policy an issue happens to cite is one row.
//!
//! The suites next door cover the other half of this loop: `tests/all/account_policies.rs`
//! the put / describe / delete round trip and the quotas while nothing is deleted, and
//! `tests/all/account_policy_selection_criteria.rs` the criteria grammar, the conflict
//! rules and the one thing a metric extraction document may say.

use serde_json::{json, Value};

use crate::account_policies::{
    delete_body, describe, described_names, member, put_body, put_policy, scoped_criteria,
    subscription_filter_document, POLICY_TYPES,
};
use crate::account_policy_selection_criteria::document_for;
use crate::harness::Logs;

/// How many metric extraction policies the account may hold when each is scoped. Source:
/// `PutAccountPolicy` — "Each account can have either one account-level metric extraction
/// policy that applies to all log groups, or up to 5 policies that are each scoped to a
/// subset of log groups with the selectionCriteria parameter."
const MAX_SCOPED_METRIC_EXTRACTION_POLICIES: usize = 5;

/// The policy types that admit a `selectionCriteria` and whose quota leaves room for two
/// scoped policies at once, so two of them can collide over the log groups they select.
/// Source: `PutAccountPolicy$selectionCriteria` (which types accept the parameter) and the
/// quota sentences on `PutAccountPolicy` (a subscription filter policy has no scoped half,
/// so the account never holds two of them to collide).
const COLLIDING_TYPES: [&str; 3] = [
    "FIELD_INDEX_POLICY",
    "TRANSFORMER_POLICY",
    "METRIC_EXTRACTION_POLICY",
];

/// One refused put, against the policy of its type the account already holds: the case
/// name, the type put, the `policyName` put (the stored policy's own name, or another for
/// a refusal that is about a policy the put would ADD), the document and criteria put, and
/// the error code the sentence it breaks is refused with.
struct RefusedPut {
    case: &'static str,
    policy_type: &'static str,
    policy_name: &'static str,
    document: String,
    criteria: Option<String>,
    expected: &'static str,
}

/// The names one type's policies are reported under, sorted, so a case asserts WHICH
/// policies the account holds rather than the order the reader walks them in (that order
/// is `tests/all/account_policies.rs`'s subject).
async fn held_names(logs: &Logs, policy_type: &str) -> Vec<String> {
    let mut names = described_names(logs, &json!({"policyType": policy_type})).await;
    names.sort();
    names
}

#[tokio::test]
async fn a_delete_gives_back_the_quota_slot_its_policy_held() {
    // The unscoped half of every type's quota: one policy "applies to all log groups", so
    // while it is held a second one of that type is over the quota — and once it is
    // deleted the very same put is admitted. The retention policies are left out: their
    // quota is "up to 20 account-level retention policies in each Region", one number
    // rather than an either/or, so a second unscoped one of that type is admitted
    // (`tests/all/account_retention_policies.rs` drives that quota and its release).
    for policy_type in POLICY_TYPES
        .into_iter()
        .filter(|policy_type| *policy_type != "RETENTION_POLICY")
    {
        let logs = Logs::new();
        let document = document_for(policy_type);
        let second = put_body("second", policy_type, &document, None);

        logs.ok(
            "PutAccountPolicy",
            &put_body("first", policy_type, &document, None),
        )
        .await;
        let code = logs.err("PutAccountPolicy", &second).await;
        assert_eq!(
            code, "LimitExceededException",
            "case {policy_type}: expected LimitExceededException while the slot is held, got {code}"
        );

        logs.ok("DeleteAccountPolicy", &delete_body("first", policy_type))
            .await;
        let put = put_policy(&logs.ok("PutAccountPolicy", &second).await);
        assert_eq!(
            member(&put, "policyName"),
            "second",
            "case {policy_type}: expected the released slot to take the second policy, got {put}"
        );
        let names = held_names(&logs, policy_type).await;
        assert_eq!(
            names,
            vec!["second".to_string()],
            "case {policy_type}: expected only the second policy to be held, got {names:?}"
        );
    }

    // The scoped half, on the type whose quota has one: five scoped policies are admitted,
    // the sixth is over the quota, and deleting one of the five lets that sixth in.
    let policy_type = "METRIC_EXTRACTION_POLICY";
    let logs = Logs::new();
    let document = document_for(policy_type);
    let scoped = |index: usize| {
        put_body(
            &format!("scoped-{index}"),
            policy_type,
            &document,
            Some(&scoped_criteria(policy_type, index)),
        )
    };
    for index in 0..MAX_SCOPED_METRIC_EXTRACTION_POLICIES {
        logs.ok("PutAccountPolicy", &scoped(index)).await;
    }
    let over = MAX_SCOPED_METRIC_EXTRACTION_POLICIES;
    let code = logs.err("PutAccountPolicy", &scoped(over)).await;
    assert_eq!(
        code, "LimitExceededException",
        "case scoped policy {over}: expected LimitExceededException while the quota is full, got {code}"
    );

    logs.ok("DeleteAccountPolicy", &delete_body("scoped-0", policy_type))
        .await;
    logs.ok("PutAccountPolicy", &scoped(over)).await;
    let names = held_names(&logs, policy_type).await;
    let expected: Vec<String> = (1..=over).map(|index| format!("scoped-{index}")).collect();
    assert_eq!(
        names, expected,
        "case scoped policy {over}: expected {expected:?} after the release, got {names:?}"
    );
}

#[tokio::test]
async fn a_delete_gives_back_the_log_groups_its_criteria_selected() {
    // Each row is a criteria dimension two policies of one type can collide on: the same
    // log group name prefix, the same data source name and type combination, and — for a
    // metric extraction policy — overlapping `IN` lists and a second `NOT IN` list.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "transformer-same-prefix",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/release"]"#.to_string(),
        ),
        (
            "field-index-same-prefix",
            "FIELD_INDEX_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/release"]"#.to_string(),
        ),
        (
            "field-index-same-data-source",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["release-source"] AND DataSourceType IN ["AWS_LAMBDA"]"#
                .to_string(),
        ),
        (
            "metric-extraction-overlapping-in",
            "METRIC_EXTRACTION_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/release"]"#.to_string(),
        ),
        (
            "metric-extraction-second-not-in",
            "METRIC_EXTRACTION_POLICY",
            r#"LogGroupName NOT IN ["/aws/release"]"#.to_string(),
        ),
    ];
    let covered: Vec<&str> = COLLIDING_TYPES
        .iter()
        .copied()
        .filter(|policy_type| rows.iter().any(|(_, row_type, _)| row_type == policy_type))
        .collect();
    assert_eq!(
        covered,
        COLLIDING_TYPES.to_vec(),
        "the table must carry a row for every type two scoped policies can collide under, got {covered:?}"
    );

    for (case, policy_type, criteria) in rows {
        let logs = Logs::new();
        let document = document_for(policy_type);
        let contender = put_body("contender", policy_type, &document, Some(&criteria));

        logs.ok(
            "PutAccountPolicy",
            &put_body("holder", policy_type, &document, Some(&criteria)),
        )
        .await;
        let code = logs.err("PutAccountPolicy", &contender).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException while the holder selects them, got {code}"
        );

        logs.ok("DeleteAccountPolicy", &delete_body("holder", policy_type))
            .await;
        let put = put_policy(&logs.ok("PutAccountPolicy", &contender).await);
        assert_eq!(
            put.get("selectionCriteria").and_then(Value::as_str),
            Some(criteria.as_str()),
            "case {case}: expected the contender to keep the criteria it put, got {put}"
        );
        let names = held_names(&logs, policy_type).await;
        assert_eq!(
            names,
            vec!["contender".to_string()],
            "case {case}: expected only the contender to be held, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_put_leaves_the_policy_already_stored_as_it_was() {
    // The name the accepted put stores its policy under. Every row but the quota one puts
    // under this same name, so a refusal that wrote anything would be visible as a changed
    // document, criteria or `lastUpdatedTime`.
    const STORED: &str = "stored";

    // Each row is a put the operation refuses — one row per sentence it breaks — against
    // the policy of that type the account already holds.
    let rows: Vec<RefusedPut> = vec![
        RefusedPut {
            case: "metric-extraction-status-enabled",
            policy_type: "METRIC_EXTRACTION_POLICY",
            policy_name: STORED,
            document: json!({"EmbeddedMetricFormat": {"Status": "Enabled"}}).to_string(),
            criteria: None,
            expected: "InvalidParameterException",
        },
        RefusedPut {
            case: "field-index-without-any-field",
            policy_type: "FIELD_INDEX_POLICY",
            policy_name: STORED,
            document: json!({"Fields": []}).to_string(),
            criteria: None,
            expected: "InvalidParameterException",
        },
        RefusedPut {
            case: "subscription-filter-to-an-unsupported-destination",
            policy_type: "SUBSCRIPTION_FILTER_POLICY",
            policy_name: STORED,
            document: subscription_filter_document(
                "arn:aws:sqs:eu-west-2:000000000000:release-queue",
                Some("arn:aws:iam::000000000000:role/release-role"),
            ),
            criteria: None,
            expected: "InvalidParameterException",
        },
        RefusedPut {
            case: "transformer-with-a-criteria-filter-it-does-not-support",
            policy_type: "TRANSFORMER_POLICY",
            policy_name: STORED,
            document: document_for("TRANSFORMER_POLICY"),
            criteria: Some(r#"LogGroupName IN ["/aws/release"]"#.to_string()),
            expected: "InvalidParameterException",
        },
        RefusedPut {
            case: "data-protection-with-a-criteria-at-all",
            policy_type: "DATA_PROTECTION_POLICY",
            policy_name: STORED,
            document: document_for("DATA_PROTECTION_POLICY"),
            criteria: Some(r#"LogGroupNamePrefix IN ["/aws/release"]"#.to_string()),
            expected: "InvalidParameterException",
        },
        RefusedPut {
            case: "second-unscoped-policy-over-the-quota",
            policy_type: "FIELD_INDEX_POLICY",
            policy_name: "another-name",
            document: document_for("FIELD_INDEX_POLICY"),
            criteria: None,
            expected: "LimitExceededException",
        },
    ];

    for row in rows {
        let case = row.case;
        let logs = Logs::new();
        let stored = put_policy(
            &logs
                .ok(
                    "PutAccountPolicy",
                    &put_body(STORED, row.policy_type, &document_for(row.policy_type), None),
                )
                .await,
        );

        let code = logs
            .err(
                "PutAccountPolicy",
                &put_body(
                    row.policy_name,
                    row.policy_type,
                    &row.document,
                    row.criteria.as_deref(),
                ),
            )
            .await;
        assert_eq!(
            code, row.expected,
            "case {case}: expected {}, got {code}",
            row.expected
        );

        let described = describe(&logs, &json!({"policyType": row.policy_type})).await;
        assert_eq!(
            described,
            vec![stored.clone()],
            "case {case}: expected the refused put to leave {stored} untouched, got {described:?}"
        );
    }
}
