// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `PutAccountPolicy` holds a scoped account-level policy to:
//! the `selectionCriteria` filters its `policyType` supports, the conflict rules between
//! two policies of one type, and the one thing a metric extraction `policyDocument` can
//! say.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rules under test, each a sentence of the operation's own documentation:
//!
//! * a `selectionCriteria` string is a filter of the form `Field IN ["value", ...]` or
//!   `Field NOT IN ["value", ...]` (`DataSourceName` AND `DataSourceType` joined by `AND`),
//!   on the fields the member names for the policy type it is put with, and a string that
//!   is not one of those is refused;
//! * no two account-level policies of one type may select the same log groups: not by the
//!   same or overlapping log group name prefixes, not by the same data source name and
//!   type combination, and for a metric extraction policy not by criteria that overlap
//!   under the `IN` / `NOT IN` rules;
//! * a metric extraction policy document says only that the Embedded Metrics Format is
//!   `Disabled`, and any other document is refused.
//!
//! Each table below spans the class its rule generalises over (the filter's field, its
//! operator, the number of values, how two value sets sit relative to one another); the
//! case an issue happens to cite is one row.

use serde_json::{json, Value};

use crate::account_policies::{
    data_protection_document, described_names, field_index_document, member,
    metric_extraction_document, put_body, put_policy, retention_document,
    subscription_filter_document, transformer_document,
};
use crate::harness::Logs;

/// The one `EmbeddedMetricFormat.Status` a metric extraction policy document may carry.
/// Source: `PutAccountPolicy` — "By default, EMF metric creation is enabled for all log
/// groups. You can use metric extraction policies to disable EMF metric creation for your
/// entire account or specific log groups" — the real service accepting only this exact
/// string there.
const EMF_STATUS_DISABLED: &str = "Disabled";

/// How many values one `IN` or `NOT IN` list may carry. Source: `PutAccountPolicy` — "You
/// can specify up to 50 values in each IN or NOT IN list."
const MAX_CRITERIA_VALUES: usize = 50;

/// A `policyDocument` the type admits, so a case about `selectionCriteria` is refused for
/// its criteria and nothing else.
pub(crate) fn document_for(policy_type: &str) -> String {
    match policy_type {
        "SUBSCRIPTION_FILTER_POLICY" => subscription_filter_document(
            "arn:aws:lambda:eu-west-2:000000000000:function:criteria-sink",
            None,
        ),
        "FIELD_INDEX_POLICY" => field_index_document(&["requestId"]),
        "TRANSFORMER_POLICY" => transformer_document(),
        "METRIC_EXTRACTION_POLICY" => metric_extraction_document(),
        "RETENTION_POLICY" => retention_document(),
        _ => data_protection_document("EmailAddress"),
    }
}

/// One put of a scoped policy of `policy_type`.
fn scoped_put(name: &str, policy_type: &str, criteria: &str) -> Value {
    put_body(name, policy_type, &document_for(policy_type), Some(criteria))
}

/// A criteria string listing `count` values of one field, to drive the documented bound on
/// how many values a list may carry.
fn criteria_of_values(field: &str, count: usize) -> String {
    let values: Vec<String> = (0..count).map(|index| format!("/aws/value-{index}")).collect();
    format!(
        "{field} IN {}",
        serde_json::to_string(&values).expect("the value list serializes")
    )
}

#[tokio::test]
async fn a_metric_extraction_document_must_disable_the_embedded_metric_format() {
    let logs = Logs::new();

    // Each row says something other than "the Embedded Metrics Format is Disabled": a
    // different status, a differently-cased one, a missing or non-string status, a
    // different member, no member at all, and a document that is not JSON.
    let refused: Vec<(&str, String)> = vec![
        (
            "status-enabled",
            json!({"EmbeddedMetricFormat": {"Status": "Enabled"}}).to_string(),
        ),
        (
            "status-lower-case",
            json!({"EmbeddedMetricFormat": {"Status": "disabled"}}).to_string(),
        ),
        (
            "status-upper-case",
            json!({"EmbeddedMetricFormat": {"Status": "DISABLED"}}).to_string(),
        ),
        (
            "status-absent",
            json!({"EmbeddedMetricFormat": {}}).to_string(),
        ),
        (
            "status-not-a-string",
            json!({"EmbeddedMetricFormat": {"Status": false}}).to_string(),
        ),
        (
            "embedded-metric-format-not-an-object",
            json!({"EmbeddedMetricFormat": "Disabled"}).to_string(),
        ),
        ("another-member-entirely", json!({"EMFEnabled": false}).to_string()),
        ("empty-object", json!({}).to_string()),
        ("not-json", "Disabled".to_string()),
    ];
    for (case, document) in refused {
        let (code, message) = logs
            .refusal(
                "PutAccountPolicy",
                &put_body(case, "METRIC_EXTRACTION_POLICY", &document, None),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {code} ({message})"
        );
    }
    let names = described_names(&logs, &json!({"policyType": "METRIC_EXTRACTION_POLICY"})).await;
    assert!(
        names.is_empty(),
        "a refused put stores nothing, got {names:?}"
    );

    // The document it does admit, alone and beside another member. Both rows are put under
    // ONE name, because an account holds one unscoped policy of this type.
    let accepted: Vec<(&str, String)> = vec![
        (
            "status-disabled",
            json!({"EmbeddedMetricFormat": {"Status": EMF_STATUS_DISABLED}}).to_string(),
        ),
        (
            "status-disabled-beside-a-name",
            json!({
                "Name": "no-emf-metrics",
                "EmbeddedMetricFormat": {"Status": EMF_STATUS_DISABLED},
            })
            .to_string(),
        ),
    ];
    for (case, document) in accepted {
        let put = put_policy(
            &logs
                .ok(
                    "PutAccountPolicy",
                    &put_body("no-emf", "METRIC_EXTRACTION_POLICY", &document, None),
                )
                .await,
        );
        assert_eq!(
            member(&put, "policyDocument"),
            document,
            "case {case}: expected the document stored verbatim, got {put}"
        );
    }
}

#[tokio::test]
async fn selection_criteria_must_be_a_filter_its_policy_type_supports() {
    // Each accepted row is a filter the member documents for that type, and its criteria
    // must come back out of the store through the reader. A put persists a policy, so each
    // row stands up its own account.
    let accepted: &[(&str, &str, &str)] = &[
        (
            "subscription-filter-excluding-nothing",
            "SUBSCRIPTION_FILTER_POLICY",
            r#"LogGroupName NOT IN []"#,
        ),
        (
            "subscription-filter-excluding-two-groups",
            "SUBSCRIPTION_FILTER_POLICY",
            r#"LogGroupName NOT IN ["sink-one", "sink-two"]"#,
        ),
        (
            "transformer-by-prefix",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/transform"]"#,
        ),
        (
            "transformer-excluding-a-prefix",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix NOT IN ["/aws/transform"]"#,
        ),
        (
            "field-index-by-prefix",
            "FIELD_INDEX_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/index"]"#,
        ),
        (
            "field-index-by-data-source-name-and-type",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] AND DataSourceType IN ["flow"]"#,
        ),
        (
            "metric-extraction-by-exact-names",
            "METRIC_EXTRACTION_POLICY",
            r#"LogGroupName IN ["group-one", "group-two"]"#,
        ),
        (
            "metric-extraction-excluding-prefixes",
            "METRIC_EXTRACTION_POLICY",
            r#"LogGroupNamePrefix NOT IN ["/aws/containerinsights", "/aws/application-signals/data"]"#,
        ),
    ];
    for (case, policy_type, criteria) in accepted {
        let logs = Logs::new();
        let put = put_policy(
            &logs
                .ok("PutAccountPolicy", &scoped_put(case, policy_type, criteria))
                .await,
        );
        assert_eq!(
            member(&put, "selectionCriteria"),
            *criteria,
            "case {case}: expected the criteria stored verbatim, got {put}"
        );
        let described = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            described,
            vec![(*case).to_string()],
            "case {case}: expected the reader to report the scoped policy, got {described:?}"
        );
    }

    // Each refused row is either a field or operator its type does not support, or a
    // string that is not a filter at all.
    let logs = Logs::new();
    let refused: &[(&str, &str, &str)] = &[
        (
            "subscription-filter-by-prefix",
            "SUBSCRIPTION_FILTER_POLICY",
            r#"LogGroupNamePrefix NOT IN ["/aws/sink"]"#,
        ),
        (
            "subscription-filter-selecting-rather-than-excluding",
            "SUBSCRIPTION_FILTER_POLICY",
            r#"LogGroupName IN ["sink-one"]"#,
        ),
        (
            "transformer-by-exact-name",
            "TRANSFORMER_POLICY",
            r#"LogGroupName IN ["group-one"]"#,
        ),
        (
            "transformer-by-data-source",
            "TRANSFORMER_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] AND DataSourceType IN ["flow"]"#,
        ),
        (
            "field-index-by-a-data-source-name-alone",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["amazon_vpc"]"#,
        ),
        (
            "field-index-by-a-data-source-type-alone",
            "FIELD_INDEX_POLICY",
            r#"DataSourceType IN ["flow"]"#,
        ),
        (
            "field-index-by-exact-name",
            "FIELD_INDEX_POLICY",
            r#"LogGroupName IN ["group-one"]"#,
        ),
        (
            "metric-extraction-by-data-source",
            "METRIC_EXTRACTION_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] AND DataSourceType IN ["flow"]"#,
        ),
        (
            "prose-rather-than-a-filter",
            "TRANSFORMER_POLICY",
            "all log groups that start with my-log",
        ),
        (
            "no-operator",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix ["/aws/transform"]"#,
        ),
        (
            "an-unknown-operator",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix CONTAINS ["/aws/transform"]"#,
        ),
        (
            "no-list",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix IN "/aws/transform""#,
        ),
        (
            "unquoted-values",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix IN [/aws/transform]"#,
        ),
        (
            "a-list-that-is-never-closed",
            "TRANSFORMER_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/transform""#,
        ),
        (
            "a-dangling-join",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] AND"#,
        ),
        (
            "two-filters-joined-by-something-else",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] OR DataSourceType IN ["flow"]"#,
        ),
        (
            "values-that-are-not-strings",
            "METRIC_EXTRACTION_POLICY",
            r#"LogGroupName IN [1, 2]"#,
        ),
    ];
    for (case, policy_type, criteria) in refused {
        let (code, message) = logs
            .refusal("PutAccountPolicy", &scoped_put(case, policy_type, criteria))
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {code} ({message})"
        );
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert!(
            names.is_empty(),
            "case {case}: a refused put stores nothing, got {names:?}"
        );
    }

    // "You can specify up to 50 values in each IN or NOT IN list": at the bound and one
    // past it, for both fields a metric extraction policy may filter on.
    for field in ["LogGroupName", "LogGroupNamePrefix"] {
        for (count, accepted) in [
            (MAX_CRITERIA_VALUES, true),
            (MAX_CRITERIA_VALUES + 1, false),
        ] {
            let logs = Logs::new();
            let criteria = criteria_of_values(field, count);
            let body = scoped_put("value-count", "METRIC_EXTRACTION_POLICY", &criteria);
            let case = format!("{field} with {count} values");
            if accepted {
                let put = put_policy(&logs.ok("PutAccountPolicy", &body).await);
                assert_eq!(
                    member(&put, "selectionCriteria"),
                    criteria,
                    "case {case}: expected the criteria stored verbatim, got {put}"
                );
            } else {
                let code = logs.err("PutAccountPolicy", &body).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case}: expected InvalidParameterException, got {code}"
                );
            }
        }
    }
}

#[tokio::test]
async fn no_two_policies_of_one_type_may_select_the_same_log_groups() {
    // "If you have multiple account-level transformer policies with selection criteria, no
    // two of them can use the same or overlapping log group name prefixes. For example, if
    // you have one policy filtered to log groups that start with my-log, you can't have
    // another transformer policy filtered to my-logpprod or my-logging." The same sentence
    // is stated for a field index policy's prefix criteria, so both types take the table.
    let prefix_rows: &[(&str, &str, &str, bool)] = &[
        ("the same prefix", "my-log", "my-log", true),
        ("a longer prefix inside it", "my-log", "my-logging", true),
        ("a longer prefix inside it, the other way round", "my-logging", "my-log", true),
        ("a prefix of a prefix", "my-log", "my-logpprod", true),
        ("disjoint prefixes", "my-log", "other-log", false),
        ("sibling prefixes", "my-log/a", "my-log/b", false),
        ("a prefix differing in case", "my-log", "My-Log", false),
    ];
    for policy_type in ["TRANSFORMER_POLICY", "FIELD_INDEX_POLICY"] {
        for (case, held, putting, conflicts) in prefix_rows {
            let logs = Logs::new();
            let criteria = |prefix: &str| format!(r#"LogGroupNamePrefix IN ["{prefix}"]"#);
            logs.ok(
                "PutAccountPolicy",
                &scoped_put("held", policy_type, &criteria(held)),
            )
            .await;
            let body = scoped_put("putting", policy_type, &criteria(putting));
            let case = format!("{policy_type} {case}");
            if *conflicts {
                let (code, message) = logs.refusal("PutAccountPolicy", &body).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case}: expected InvalidParameterException, got {code} ({message})"
                );
                let names = described_names(&logs, &json!({"policyType": policy_type})).await;
                assert_eq!(
                    names,
                    vec!["held".to_string()],
                    "case {case}: a refused put leaves the held policy alone and stores nothing, got {names:?}"
                );
            } else {
                logs.ok("PutAccountPolicy", &body).await;
                let names = described_names(&logs, &json!({"policyType": policy_type})).await;
                assert_eq!(
                    names,
                    vec!["held".to_string(), "putting".to_string()],
                    "case {case}: expected both policies stored, got {names:?}"
                );
            }
        }
    }

    // "Similarly, if you have multiple account-level index policies with DataSourceName and
    // DataSourceType selection criteria, no two of them can use the same data source name
    // and type combination."
    let data_source_rows: &[(&str, &str, &str, &str, &str, bool)] = &[
        (
            "the same name and type",
            "amazon_vpc",
            "flow",
            "amazon_vpc",
            "flow",
            true,
        ),
        (
            "the same name, another type",
            "aws_cloudtrail",
            "data",
            "aws_cloudtrail",
            "management",
            false,
        ),
        (
            "another name, the same type",
            "amazon_vpc",
            "flow",
            "amazon_route53",
            "flow",
            false,
        ),
        (
            "a name in common and a type in common",
            "amazon_vpc",
            "flow",
            "amazon_vpc",
            "flow",
            true,
        ),
    ];
    for (case, held_name, held_type, name, source_type, conflicts) in data_source_rows {
        let logs = Logs::new();
        let criteria = |name: &str, source_type: &str| {
            format!(r#"DataSourceName IN ["{name}"] AND DataSourceType IN ["{source_type}"]"#)
        };
        logs.ok(
            "PutAccountPolicy",
            &scoped_put(
                "held",
                "FIELD_INDEX_POLICY",
                &criteria(held_name, held_type),
            ),
        )
        .await;
        let body = scoped_put(
            "putting",
            "FIELD_INDEX_POLICY",
            &criteria(name, source_type),
        );
        if *conflicts {
            let code = logs.err("PutAccountPolicy", &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case}: expected InvalidParameterException, got {code}"
            );
        } else {
            logs.ok("PutAccountPolicy", &body).await;
        }
    }

    // A data source combination and a log group prefix select along different dimensions —
    // "You can have another 20 account-level field index policies using DataSourceName and
    // DataSourceType" — so neither constrains the other.
    let logs = Logs::new();
    logs.ok(
        "PutAccountPolicy",
        &scoped_put(
            "by-prefix",
            "FIELD_INDEX_POLICY",
            r#"LogGroupNamePrefix IN ["/aws/vpc"]"#,
        ),
    )
    .await;
    logs.ok(
        "PutAccountPolicy",
        &scoped_put(
            "by-data-source",
            "FIELD_INDEX_POLICY",
            r#"DataSourceName IN ["amazon_vpc"] AND DataSourceType IN ["flow"]"#,
        ),
    )
    .await;
    let names = described_names(&logs, &json!({"policyType": "FIELD_INDEX_POLICY"})).await;
    assert_eq!(
        names,
        vec!["by-data-source".to_string(), "by-prefix".to_string()],
        "expected a prefix policy and a data source policy to coexist, got {names:?}"
    );

    // A put under a policy's OWN name is an update of that policy, so its criteria never
    // conflicts with itself.
    let logs = Logs::new();
    for criteria in [
        r#"LogGroupNamePrefix IN ["my-log"]"#,
        r#"LogGroupNamePrefix IN ["my-log"]"#,
        r#"LogGroupNamePrefix IN ["my-logging"]"#,
    ] {
        let put = put_policy(
            &logs
                .ok(
                    "PutAccountPolicy",
                    &scoped_put("updated-in-place", "TRANSFORMER_POLICY", criteria),
                )
                .await,
        );
        assert_eq!(
            member(&put, "selectionCriteria"),
            criteria,
            "case update to {criteria}: expected the replacing criteria, got {put}"
        );
    }
}

#[tokio::test]
async fn metric_extraction_criteria_may_not_overlap_under_the_in_and_not_in_rules() {
    // Each row is the criteria of the policy the account holds, the criteria of the policy
    // being put, and whether `PutAccountPolicy`'s overlap rules let the second one in:
    //
    // * two `IN` policies conflict when the log groups they select meet, by exact name or
    //   by prefix ("no two of them can have overlapping criteria");
    // * "When using NOT IN, only one policy with this operator is allowed per account";
    // * an `IN` policy and a `NOT IN` policy "do not overlap if and only if every value in
    //   the IN policy is completely contained within some value in the NOT IN policy".
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "two prefixes, one inside the other",
            r#"LogGroupNamePrefix IN ["my-log"]"#,
            r#"LogGroupNamePrefix IN ["my-log-prod"]"#,
            true,
        ),
        (
            "two disjoint prefixes",
            r#"LogGroupNamePrefix IN ["my-log"]"#,
            r#"LogGroupNamePrefix IN ["other-log"]"#,
            false,
        ),
        (
            "a name under a selected prefix",
            r#"LogGroupNamePrefix IN ["my-log"]"#,
            r#"LogGroupName IN ["my-log/one"]"#,
            true,
        ),
        (
            "a name outside every selected prefix",
            r#"LogGroupNamePrefix IN ["my-log"]"#,
            r#"LogGroupName IN ["other-log/one"]"#,
            false,
        ),
        (
            "the same exact name",
            r#"LogGroupName IN ["group-one", "group-two"]"#,
            r#"LogGroupName IN ["group-two"]"#,
            true,
        ),
        (
            "different exact names",
            r#"LogGroupName IN ["group-one"]"#,
            r#"LogGroupName IN ["group-two"]"#,
            false,
        ),
        (
            "an exact name inside the excluded prefix",
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            r#"LogGroupName IN ["/aws/lambda/function1"]"#,
            false,
        ),
        (
            "a prefix wider than the excluded one",
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            r#"LogGroupNamePrefix IN ["/aws"]"#,
            true,
        ),
        (
            "a prefix inside the excluded one",
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            r#"LogGroupNamePrefix IN ["/aws/lambda/api"]"#,
            false,
        ),
        (
            "one selected value outside the excluded prefix",
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            r#"LogGroupName IN ["/aws/lambda/one", "/aws/ecs/two"]"#,
            true,
        ),
        (
            "an excluded prefix put after the selecting policy",
            r#"LogGroupName IN ["/aws/lambda/function1"]"#,
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            false,
        ),
        (
            "a second excluding policy",
            r#"LogGroupNamePrefix NOT IN ["/aws/lambda"]"#,
            r#"LogGroupNamePrefix NOT IN ["/aws/ecs"]"#,
            true,
        ),
    ];

    for (case, held, putting, conflicts) in rows {
        let logs = Logs::new();
        logs.ok(
            "PutAccountPolicy",
            &scoped_put("held", "METRIC_EXTRACTION_POLICY", held),
        )
        .await;
        let body = scoped_put("putting", "METRIC_EXTRACTION_POLICY", putting);
        if *conflicts {
            let (code, message) = logs.refusal("PutAccountPolicy", &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case}: expected InvalidParameterException, got {code} ({message})"
            );
            let names =
                described_names(&logs, &json!({"policyType": "METRIC_EXTRACTION_POLICY"})).await;
            assert_eq!(
                names,
                vec!["held".to_string()],
                "case {case}: a refused put stores nothing, got {names:?}"
            );
        } else {
            logs.ok("PutAccountPolicy", &body).await;
            let names =
                described_names(&logs, &json!({"policyType": "METRIC_EXTRACTION_POLICY"})).await;
            assert_eq!(
                names,
                vec!["held".to_string(), "putting".to_string()],
                "case {case}: expected both policies stored, got {names:?}"
            );
        }
    }
}
