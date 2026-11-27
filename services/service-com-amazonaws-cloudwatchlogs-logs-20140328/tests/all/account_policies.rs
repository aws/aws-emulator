// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account-level policy loop:
//! `PutAccountPolicy` / `DescribeAccountPolicies` / `DeleteAccountPolicy`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: `DescribeAccountPolicies` reports, out of the account's persisted
//! account-level policies, exactly those of the `policyType` it is asked for — narrowed by
//! `policyName` and `accountIdentifiers` when the request carries them — each rendering
//! the stored policy's name, document, scope, selection criteria, update time and account
//! id; a policy is there from the put that stores it until the delete that removes it.
//!
//! The tables below span the classes that rule generalises over (policy type, document
//! shape, selection criteria, name ordering, filter, quota boundary, document length
//! bound); any single policy an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// Every value of the model's `PolicyType` enum, which is what `policyType` targets.
/// Source: `smithy_query(query="shape", shape="PolicyType")` — `DATA_PROTECTION_POLICY`,
/// `SUBSCRIPTION_FILTER_POLICY`, `FIELD_INDEX_POLICY`, `TRANSFORMER_POLICY`,
/// `METRIC_EXTRACTION_POLICY`, `RETENTION_POLICY`.
pub(crate) const POLICY_TYPES: [&str; 6] = [
    "DATA_PROTECTION_POLICY",
    "SUBSCRIPTION_FILTER_POLICY",
    "FIELD_INDEX_POLICY",
    "TRANSFORMER_POLICY",
    "METRIC_EXTRACTION_POLICY",
    "RETENTION_POLICY",
];

/// The one type whose quota is a single number rather than the either/or every other
/// type's is: "Each account can have up to 20 account-level retention policies in each
/// Region" (`PutAccountPolicy`). What that type's policies DO to a log group's retention,
/// and the quota itself, are the subject of `tests/all/account_retention_policies.rs`.
pub(crate) const RETENTION_POLICY_TYPE: &str = "RETENTION_POLICY";

/// The only value of the model's `Scope` enum, and the scope a put that names none
/// reports. Source: `PutAccountPolicy$scope` — "Currently the only valid value for this
/// parameter is ALL ... If you omit this parameter, the default of ALL is used."
const SCOPE_ALL: &str = "ALL";

/// The types a `FieldsV2` entry may carry. Source: `PutAccountPolicy$policyDocument` —
/// "Supported types are FIELD_INDEX and FACET" (the model's `IndexType` enum).
const INDEX_TYPES: [&str; 2] = ["FIELD_INDEX", "FACET"];

/// The longest `policyDocument` the operation admits. Source:
/// `PutAccountPolicy$policyDocument` — "The JSON specified in policyDocument can be up to
/// 30,720 characters long."
const MAX_POLICY_DOCUMENT_CHARS: usize = 30_720;

/// The longest `selectionCriteria` the operation admits. Source:
/// `PutAccountPolicy$selectionCriteria` — "The selectionCriteria string can be up to 25KB
/// in length. The length is determined by using its UTF-8 bytes."
const MAX_SELECTION_CRITERIA_BYTES: usize = 25 * 1024;

/// How many account-level policies of one type apply to all log groups. Source:
/// `PutAccountPolicy` — "Each account can have only one account-level data protection
/// policy", "Each account can have one account-level subscription filter policy per
/// Region", and the "one account-level transformer policy that applies to all log groups
/// in the account" half of the remaining quota sentences.
const MAX_UNSCOPED_POLICIES: usize = 1;

/// How many metric extraction policies the account may hold when each is scoped. Source:
/// `PutAccountPolicy` — "Each account can have either one account-level metric extraction
/// policy that applies to all log groups, or up to 5 policies that are each scoped to a
/// subset of log groups with the selectionCriteria parameter."
const MAX_SCOPED_METRIC_EXTRACTION_POLICIES: usize = 5;

/// A data protection policy document: the two blocks `PutAccountPolicy$policyDocument`
/// documents, each listing the same `DataIdentifier` array, with the Audit action's
/// `FindingsDestination` left empty (the "optionally" case).
pub(crate) fn data_protection_document(identifier: &str) -> String {
    let identifiers = json!([format!(
        "arn:aws:dataprotection::aws:data-identifier/{identifier}"
    )]);
    json!({
        "Name": "emulator-account-data-protection",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}},
            },
            {
                "Sid": "mask",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}},
            },
        ],
    })
    .to_string()
}

/// A subscription filter policy document: the attributes
/// `PutAccountPolicy$policyDocument` lists for one (`DestinationArn`, `RoleArn`,
/// `FilterPattern`, `Distribution`).
pub(crate) fn subscription_filter_document(
    destination_arn: &str,
    role_arn: Option<&str>,
) -> String {
    let mut document = json!({
        "DestinationArn": destination_arn,
        "FilterPattern": "",
        "Distribution": "ByLogStream",
    });
    if let Some(role_arn) = role_arn {
        document["RoleArn"] = json!(role_arn);
    }
    document.to_string()
}

/// A field index policy document carrying a `Fields` array. Source:
/// `PutAccountPolicy$policyDocument` — "Fields The array of field indexes to create".
pub(crate) fn field_index_document(fields: &[&str]) -> String {
    json!({"Fields": fields}).to_string()
}

/// A field index policy document whose `FieldsV2` object names one field per index type.
/// Source: the example on `PutAccountPolicy$policyDocument`.
fn field_index_v2_document() -> String {
    let mut fields = serde_json::Map::new();
    for (index, index_type) in INDEX_TYPES.iter().enumerate() {
        fields.insert(format!("field{index}"), json!({"type": index_type}));
    }
    json!({"FieldsV2": fields}).to_string()
}

/// A transformer policy document: "one JSON block with the array of processors and their
/// configurations".
pub(crate) fn transformer_document() -> String {
    json!([{"parseJSON": {}}, {"trimString": {"withKeys": ["message"]}}]).to_string()
}

/// A metric extraction policy document: the one thing such a policy can say. Source:
/// `PutAccountPolicy` — "You can use metric extraction policies to disable EMF metric
/// creation for your entire account or specific log groups", which the document says
/// through `EmbeddedMetricFormat.Status`, where the service accepts only the exact string
/// `Disabled` (the subject of `tests/all/account_policy_selection_criteria.rs`).
pub(crate) fn metric_extraction_document() -> String {
    json!({"EmbeddedMetricFormat": {"Status": "Disabled"}}).to_string()
}

/// A retention policy document: the two attributes `PutAccountPolicy$policyDocument` lists
/// for one, in the shape of the example it gives — `"{\"retentionInDays\": 90,
/// \"enforcementMode\": \"RETROACTIVE\"}"`.
pub(crate) fn retention_document() -> String {
    json!({"retentionInDays": 90, "enforcementMode": "RETROACTIVE"}).to_string()
}

/// One row per policy type: a document that type admits, and the `selectionCriteria` it
/// may be put with — `None` for a data protection policy, the one type
/// `PutAccountPolicy$selectionCriteria` does not list. The criteria strings are in the
/// forms that member documents per type.
fn policy_rows() -> Vec<(&'static str, String, Option<&'static str>)> {
    vec![
        (
            "DATA_PROTECTION_POLICY",
            data_protection_document("EmailAddress"),
            None,
        ),
        (
            "SUBSCRIPTION_FILTER_POLICY",
            subscription_filter_document(
                "arn:aws:lambda:eu-west-2:000000000000:function:account-policy-sink",
                None,
            ),
            Some(r#"LogGroupName NOT IN ["account-policy-sink-logs"]"#),
        ),
        (
            "FIELD_INDEX_POLICY",
            field_index_document(&["requestId", "sessionId"]),
            Some(r#"LogGroupNamePrefix IN ["/aws/index"]"#),
        ),
        (
            "TRANSFORMER_POLICY",
            transformer_document(),
            Some(r#"LogGroupNamePrefix IN ["/aws/transform"]"#),
        ),
        (
            "METRIC_EXTRACTION_POLICY",
            metric_extraction_document(),
            Some(r#"LogGroupNamePrefix NOT IN ["/aws/containerinsights"]"#),
        ),
        (
            RETENTION_POLICY_TYPE,
            retention_document(),
            // "Retention policies support log group name prefixes only."
            Some(r#"LogGroupNamePrefix IN ["/aws/retain"]"#),
        ),
    ]
}

/// A request body for one delete, naming the `(policyType, policyName)` pair that
/// identifies one account-level policy.
pub(crate) fn delete_body(name: &str, policy_type: &str) -> Value {
    json!({"policyName": name, "policyType": policy_type})
}

/// A request body for one put.
pub(crate) fn put_body(
    name: &str,
    policy_type: &str,
    document: &str,
    criteria: Option<&str>,
) -> Value {
    let mut body = json!({
        "policyName": name,
        "policyType": policy_type,
        "policyDocument": document,
    });
    if let Some(criteria) = criteria {
        body["selectionCriteria"] = json!(criteria);
    }
    body
}

/// One string member of a returned `AccountPolicy`.
pub(crate) fn member(policy: &Value, key: &str) -> String {
    policy
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an account policy must carry {key}, got {policy}"))
        .to_string()
}

/// The `accountPolicy` of a `PutAccountPolicy` response.
pub(crate) fn put_policy(response: &Value) -> Value {
    response
        .get("accountPolicy")
        .cloned()
        .unwrap_or_else(|| panic!("PutAccountPolicy must return an accountPolicy, got {response}"))
}

/// The policies one `DescribeAccountPolicies` request reports, asserting it reports them
/// in one page (this operation mints no `nextToken`).
pub(crate) async fn describe(logs: &Logs, body: &Value) -> Vec<Value> {
    let page = logs.ok("DescribeAccountPolicies", body).await;
    assert!(
        page.get("nextToken").is_none(),
        "case {body}: DescribeAccountPolicies reports one page, got a nextToken in {page}"
    );
    page.get("accountPolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {body}: DescribeAccountPolicies must return accountPolicies, got {page}")
        })
        .clone()
}

/// The `policyName`s one `DescribeAccountPolicies` request reports, in the order returned.
pub(crate) async fn described_names(logs: &Logs, body: &Value) -> Vec<String> {
    describe(logs, body)
        .await
        .iter()
        .map(|policy| member(policy, "policyName"))
        .collect()
}

#[tokio::test]
async fn every_policy_type_round_trips_through_put_describe_and_delete() {
    let logs = Logs::new();
    let account = engine::emulator_account_id();

    let covered: Vec<&str> = policy_rows()
        .iter()
        .map(|(policy_type, _, _)| *policy_type)
        .collect();
    assert_eq!(
        covered,
        POLICY_TYPES.to_vec(),
        "the table must span every modelled policy type, got {covered:?}"
    );

    for (policy_type, document, criteria) in policy_rows() {
        let case = policy_type;
        let name = format!("{}-under-test", policy_type.to_ascii_lowercase());
        let body = put_body(&name, policy_type, &document, criteria);

        let before = now_ms();
        let put = put_policy(&logs.ok("PutAccountPolicy", &body).await);
        let after = now_ms();

        assert_eq!(
            member(&put, "policyName"),
            name,
            "case {case}: expected the stored policy name, got {put}"
        );
        assert_eq!(
            member(&put, "policyDocument"),
            document,
            "case {case}: expected the document verbatim, got {put}"
        );
        assert_eq!(
            member(&put, "policyType"),
            policy_type,
            "case {case}: expected the policy type put, got {put}"
        );
        assert_eq!(
            member(&put, "scope"),
            SCOPE_ALL,
            "case {case}: expected the documented default scope, got {put}"
        );
        assert_eq!(
            put.get("selectionCriteria").and_then(Value::as_str),
            criteria,
            "case {case}: expected the selection criteria put, got {put}"
        );
        assert_eq!(
            member(&put, "accountId"),
            account,
            "case {case}: expected this account's id, got {put}"
        );
        let updated = put
            .get("lastUpdatedTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: a policy carries lastUpdatedTime, got {put}"));
        assert!(
            (before..=after).contains(&updated),
            "case {case}: expected lastUpdatedTime in [{before}, {after}], got {updated}"
        );

        // The reader reports the policy the put stored, member for member.
        let described = describe(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            described,
            vec![put.clone()],
            "case {case}: expected DescribeAccountPolicies to report the stored policy, got {described:?}"
        );

        // Reading it again changes nothing a later caller sees.
        let reread = describe(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            reread, described,
            "case {case}: expected a repeated read to report the same policy, got {reread:?}"
        );

        // ... and the delete takes it away, so the reader reports it is gone.
        let delete = json!({"policyName": name, "policyType": policy_type});
        logs.ok("DeleteAccountPolicy", &delete).await;
        let after_delete = describe(&logs, &json!({"policyType": policy_type})).await;
        assert!(
            after_delete.is_empty(),
            "case {case}: expected no policies after the delete, got {after_delete:?}"
        );
        let code = logs.err("DeleteAccountPolicy", &delete).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException deleting it twice, got {code}"
        );
    }
}

#[tokio::test]
async fn describe_filters_by_type_name_and_account() {
    let logs = Logs::new();
    let account = engine::emulator_account_id();

    // Two policies of one type, stored out of name order, plus one of another type: the
    // reader must split them by type and report one type's policies by policy name.
    let index_document = field_index_document(&["requestId"]);
    for name in ["zz-index", "aa-index"] {
        // A prefix per policy: no two policies of one type may select log groups by the
        // same or overlapping prefixes, which
        // `tests/all/account_policy_selection_criteria.rs` is about.
        let criteria = format!(r#"LogGroupNamePrefix IN ["/aws/index/{name}"]"#);
        logs.ok(
            "PutAccountPolicy",
            &put_body(name, "FIELD_INDEX_POLICY", &index_document, Some(&criteria)),
        )
        .await;
    }
    logs.ok(
        "PutAccountPolicy",
        &put_body(
            "zz-index",
            "TRANSFORMER_POLICY",
            &transformer_document(),
            Some(r#"LogGroupNamePrefix IN ["/aws/transform"]"#),
        ),
    )
    .await;

    let by_type: &[(&str, &[&str])] = &[
        ("FIELD_INDEX_POLICY", &["aa-index", "zz-index"]),
        ("TRANSFORMER_POLICY", &["zz-index"]),
        ("DATA_PROTECTION_POLICY", &[]),
        ("SUBSCRIPTION_FILTER_POLICY", &[]),
        ("METRIC_EXTRACTION_POLICY", &[]),
        (RETENTION_POLICY_TYPE, &[]),
    ];
    for (policy_type, expected) in by_type {
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            names, *expected,
            "case {policy_type}: expected {expected:?}, got {names:?}"
        );
    }

    // `policyName` narrows one type's policies to the one it names, and names nothing
    // when no policy of that type carries it.
    let by_name: &[(&str, &str, &[&str])] = &[
        ("FIELD_INDEX_POLICY", "aa-index", &["aa-index"]),
        ("FIELD_INDEX_POLICY", "zz-index", &["zz-index"]),
        ("FIELD_INDEX_POLICY", "no-such-policy", &[]),
        ("TRANSFORMER_POLICY", "aa-index", &[]),
    ];
    for (policy_type, policy_name, expected) in by_name {
        let names = described_names(
            &logs,
            &json!({"policyType": policy_type, "policyName": policy_name}),
        )
        .await;
        assert_eq!(
            names, *expected,
            "case {policy_type}/{policy_name}: expected {expected:?}, got {names:?}"
        );
    }

    // "If you omit this parameter, only the policy in the current account is returned":
    // naming this account is the same request, and naming another account selects none of
    // this account's policies.
    let by_account: &[(&str, &[&str])] = &[
        (account, &["aa-index", "zz-index"]),
        ("210987654321", &[]),
        ("000000000001", &[]),
    ];
    for (identifier, expected) in by_account {
        let names = described_names(
            &logs,
            &json!({"policyType": "FIELD_INDEX_POLICY", "accountIdentifiers": [identifier]}),
        )
        .await;
        assert_eq!(
            names, *expected,
            "case account {identifier}: expected {expected:?}, got {names:?}"
        );
    }

    // "Currently, you can specify only one account ID in this parameter."
    let code = logs
        .err(
            "DescribeAccountPolicies",
            &json!({
                "policyType": "FIELD_INDEX_POLICY",
                "accountIdentifiers": [account, "210987654321"],
            }),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case two account identifiers: expected InvalidParameterException, got {code}"
    );
}

#[tokio::test]
async fn describe_refuses_a_next_token_it_did_not_issue() {
    let logs = Logs::new();
    logs.ok(
        "PutAccountPolicy",
        &put_body(
            "token-case",
            "FIELD_INDEX_POLICY",
            &field_index_document(&["requestId"]),
            None,
        ),
    )
    .await;

    for token in ["deadbeef", "not-a-token", "0"] {
        let code = logs
            .err(
                "DescribeAccountPolicies",
                &json!({"policyType": "FIELD_INDEX_POLICY", "nextToken": token}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case nextToken {token}: expected InvalidParameterException, got {code}"
        );
    }
}

#[tokio::test]
async fn a_put_under_a_stored_name_updates_that_policy_and_leaves_the_others() {
    let logs = Logs::new();

    // One name per type: the pair `(policyType, policyName)` identifies a policy, so the
    // same name under two types is two policies and a delete of one leaves the other.
    for (policy_type, document, _) in policy_rows() {
        logs.ok(
            "PutAccountPolicy",
            &put_body("shared-name", policy_type, &document, None),
        )
        .await;
    }
    for (policy_type, _, _) in policy_rows() {
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            names,
            vec!["shared-name".to_string()],
            "case {policy_type}: expected one policy under the shared name, got {names:?}"
        );
    }
    logs.ok(
        "DeleteAccountPolicy",
        &json!({"policyName": "shared-name", "policyType": "TRANSFORMER_POLICY"}),
    )
    .await;
    for (policy_type, _, _) in policy_rows() {
        let expected = usize::from(policy_type != "TRANSFORMER_POLICY");
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            names.len(),
            expected,
            "case {policy_type}: expected {expected} policies after deleting the transformer one, got {names:?}"
        );
    }

    // A put under a name a type already holds replaces that policy's members rather than
    // adding a second one — including dropping a criteria string the new put omits.
    let replacements: &[(&str, Option<&str>)] = &[
        ("requestId", Some(r#"LogGroupNamePrefix IN ["/aws/one"]"#)),
        ("sessionId", Some(r#"LogGroupNamePrefix IN ["/aws/two"]"#)),
        ("traceId", None),
    ];
    let mut previous_updated = 0;
    for (field, criteria) in replacements {
        let document = field_index_document(&[*field]);
        let put = put_policy(
            &logs
                .ok(
                    "PutAccountPolicy",
                    &put_body("shared-name", "FIELD_INDEX_POLICY", &document, *criteria),
                )
                .await,
        );
        let described = describe(&logs, &json!({"policyType": "FIELD_INDEX_POLICY"})).await;
        assert_eq!(
            described,
            vec![put.clone()],
            "case {field}: expected the update to replace the one stored policy, got {described:?}"
        );
        assert_eq!(
            member(&put, "policyDocument"),
            document,
            "case {field}: expected the replacing document, got {put}"
        );
        assert_eq!(
            put.get("selectionCriteria").and_then(Value::as_str),
            *criteria,
            "case {field}: expected the replacing selection criteria, got {put}"
        );
        let updated = put
            .get("lastUpdatedTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {field}: a policy carries lastUpdatedTime, got {put}"));
        assert!(
            updated >= previous_updated,
            "case {field}: expected lastUpdatedTime to be restamped to at least {previous_updated}, got {updated}"
        );
        previous_updated = updated;
    }
}

#[tokio::test]
async fn put_enforces_the_documented_per_type_quotas() {
    // One policy of each type applies to all log groups, so a SECOND unscoped policy of
    // that type under another name is over the quota. The retention policies are the one
    // type this does not hold for — their quota is "up to 20 account-level retention
    // policies in each Region" whether or not they are scoped — so that type's quota is
    // driven by `tests/all/account_retention_policies.rs` instead.
    for (policy_type, document, _) in policy_rows()
        .into_iter()
        .filter(|(policy_type, _, _)| *policy_type != RETENTION_POLICY_TYPE)
    {
        let logs = Logs::new();
        for index in 0..=MAX_UNSCOPED_POLICIES {
            let name = format!("unscoped-{index}");
            let body = put_body(&name, policy_type, &document, None);
            if index < MAX_UNSCOPED_POLICIES {
                logs.ok("PutAccountPolicy", &body).await;
            } else {
                let code = logs.err("PutAccountPolicy", &body).await;
                assert_eq!(
                    code, "LimitExceededException",
                    "case {policy_type} policy {index}: expected LimitExceededException, got {code}"
                );
            }
        }
    }

    // "up to 5 policies that are each scoped to a subset of log groups with the
    // selectionCriteria parameter": the fifth is admitted, the sixth is not.
    let logs = Logs::new();
    for index in 0..=MAX_SCOPED_METRIC_EXTRACTION_POLICIES {
        let name = format!("scoped-{index}");
        let criteria = format!(r#"LogGroupNamePrefix IN ["/aws/metrics-{index}"]"#);
        let body = put_body(
            &name,
            "METRIC_EXTRACTION_POLICY",
            &metric_extraction_document(),
            Some(&criteria),
        );
        if index < MAX_SCOPED_METRIC_EXTRACTION_POLICIES {
            logs.ok("PutAccountPolicy", &body).await;
        } else {
            let code = logs.err("PutAccountPolicy", &body).await;
            assert_eq!(
                code, "LimitExceededException",
                "case scoped metric extraction policy {index}: expected LimitExceededException, got {code}"
            );
        }
    }
    let names = described_names(&logs, &json!({"policyType": "METRIC_EXTRACTION_POLICY"})).await;
    assert_eq!(
        names.len(),
        MAX_SCOPED_METRIC_EXTRACTION_POLICIES,
        "expected the account to hold {MAX_SCOPED_METRIC_EXTRACTION_POLICIES} scoped policies, got {names:?}"
    );

    // Each quota sentence is an EITHER/OR — "either one account-level metric extraction
    // policy that applies to all log groups, or up to 5 policies that are each scoped to a
    // subset of log groups" — so a scoped policy and an unscoped one of the same type
    // count against the same quota rather than one each.
    let mixed: &[(&str, usize)] = &[
        ("METRIC_EXTRACTION_POLICY", MAX_SCOPED_METRIC_EXTRACTION_POLICIES),
        ("SUBSCRIPTION_FILTER_POLICY", MAX_UNSCOPED_POLICIES),
    ];
    for (policy_type, limit) in mixed {
        let logs = Logs::new();
        let document = policy_rows()
            .into_iter()
            .find(|(row_type, _, _)| row_type == policy_type)
            .map(|(_, document, _)| document)
            .unwrap_or_else(|| panic!("case {policy_type}: the table must carry a document"));
        // The unscoped policy first: it is the "applies to all log groups" half.
        logs.ok(
            "PutAccountPolicy",
            &put_body("mixed-unscoped", policy_type, &document, None),
        )
        .await;
        for index in 1..=*limit {
            let criteria = scoped_criteria(policy_type, index);
            let body = put_body(
                &format!("mixed-scoped-{index}"),
                policy_type,
                &document,
                Some(&criteria),
            );
            if index < *limit {
                logs.ok("PutAccountPolicy", &body).await;
            } else {
                let code = logs.err("PutAccountPolicy", &body).await;
                assert_eq!(
                    code, "LimitExceededException",
                    "case {policy_type} mixed policy {index} of {limit}: expected LimitExceededException, got {code}"
                );
            }
        }
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert_eq!(
            names.len(),
            *limit,
            "case {policy_type}: expected the type's quota of {limit} policies in total, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_name_in_the_reserved_namespace_is_refused() {
    let logs = Logs::new();
    // "This must be unique within the account and cannot start with aws/." The rows differ
    // in what follows the reserved prefix; the accepted rows carry it elsewhere in the
    // name, which the sentence says nothing against.
    let refused = ["aws/", "aws/reserved", "aws/nested/name"];
    for name in refused {
        let code = logs
            .err(
                "PutAccountPolicy",
                &put_body(
                    name,
                    "FIELD_INDEX_POLICY",
                    &field_index_document(&["requestId"]),
                    None,
                ),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case policyName {name}: expected InvalidParameterException, got {code}"
        );
    }
    let accepted = ["my-aws/policy", "AWS/policy", "awspolicy"];
    for name in accepted {
        let put = put_policy(
            &logs
                .ok(
                    "PutAccountPolicy",
                    &put_body(
                        name,
                        "FIELD_INDEX_POLICY",
                        &field_index_document(&["requestId"]),
                        None,
                    ),
                )
                .await,
        );
        assert_eq!(
            member(&put, "policyName"),
            name,
            "case policyName {name}: expected it stored, got {put}"
        );
        logs.ok(
            "DeleteAccountPolicy",
            &json!({"policyName": name, "policyType": "FIELD_INDEX_POLICY"}),
        )
        .await;
    }
}

#[tokio::test]
async fn a_document_its_type_does_not_admit_is_refused() {
    let logs = Logs::new();

    // Each row is a document that breaks one sentence of
    // `PutAccountPolicy$policyDocument` for the type it is put with.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "not-json",
            "FIELD_INDEX_POLICY",
            "this is not a policy".to_string(),
        ),
        (
            "empty-object",
            "METRIC_EXTRACTION_POLICY",
            json!({}).to_string(),
        ),
        (
            "field-index-without-any-field",
            "FIELD_INDEX_POLICY",
            json!({"Fields": []}).to_string(),
        ),
        (
            "field-index-with-an-unsupported-type",
            "FIELD_INDEX_POLICY",
            json!({"FieldsV2": {"requestId": {"type": "NOT_AN_INDEX_TYPE"}}}).to_string(),
        ),
        (
            "field-index-with-a-field-in-both-lists",
            "FIELD_INDEX_POLICY",
            json!({"Fields": ["requestId"], "FieldsV2": {"requestId": {"type": "FACET"}}})
                .to_string(),
        ),
        (
            "subscription-filter-without-a-destination",
            "SUBSCRIPTION_FILTER_POLICY",
            json!({"FilterPattern": ""}).to_string(),
        ),
        (
            "subscription-filter-to-an-unsupported-destination",
            "SUBSCRIPTION_FILTER_POLICY",
            subscription_filter_document(
                "arn:aws:sqs:eu-west-2:000000000000:account-policy-queue",
                Some("arn:aws:iam::000000000000:role/account-policy-role"),
            ),
        ),
        (
            "subscription-filter-without-the-role-its-destination-needs",
            "SUBSCRIPTION_FILTER_POLICY",
            subscription_filter_document(
                "arn:aws:kinesis:eu-west-2:000000000000:stream/account-policy-stream",
                None,
            ),
        ),
        (
            "subscription-filter-with-an-unsupported-distribution",
            "SUBSCRIPTION_FILTER_POLICY",
            json!({
                "DestinationArn": "arn:aws:lambda:eu-west-2:000000000000:function:sink",
                "Distribution": "RoundRobin",
            })
            .to_string(),
        ),
        (
            "transformer-without-processors",
            "TRANSFORMER_POLICY",
            json!([]).to_string(),
        ),
        (
            // An account-level transformer is a transformer: its pipeline must begin with a
            // parse-type processor, the same rule `PutTransformer` holds a log-group level
            // one to (the subject of `tests/all/transformers.rs`).
            "transformer-whose-first-processor-is-a-mutation",
            "TRANSFORMER_POLICY",
            json!([{"trimString": {"withKeys": ["message"]}}, {"parseJSON": {}}]).to_string(),
        ),
        (
            "data-protection-without-the-deidentify-block",
            "DATA_PROTECTION_POLICY",
            json!({
                "Statement": [{
                    "DataIdentifier": ["arn:aws:dataprotection::aws:data-identifier/Name"],
                    "Operation": {"Audit": {"FindingsDestination": {}}},
                }],
            })
            .to_string(),
        ),
        (
            "data-protection-with-a-non-empty-mask-config",
            "DATA_PROTECTION_POLICY",
            data_protection_document("EmailAddress").replace(
                r#""MaskConfig":{}"#,
                r#""MaskConfig":{"Mask":"***"}"#,
            ),
        ),
    ];

    for (case, policy_type, document) in rows {
        let code = logs
            .err(
                "PutAccountPolicy",
                &put_body(case, policy_type, &document, None),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {code}"
        );
        let names = described_names(&logs, &json!({"policyType": policy_type})).await;
        assert!(
            names.is_empty(),
            "case {case}: a refused put stores nothing, got {names:?}"
        );
    }

    // The documents the same sentences DO admit, including one `FieldsV2` entry per
    // documented index type.
    let accepted: Vec<(&str, &str, String)> = vec![
        (
            "field-index-by-fields-v2",
            "FIELD_INDEX_POLICY",
            field_index_v2_document(),
        ),
        (
            "transformer-processors-in-a-block",
            "TRANSFORMER_POLICY",
            json!({"processors": [{"parseJSON": {}}]}).to_string(),
        ),
        (
            "subscription-filter-with-a-role",
            "SUBSCRIPTION_FILTER_POLICY",
            subscription_filter_document(
                "arn:aws:kinesis:eu-west-2:000000000000:stream/account-policy-stream",
                Some("arn:aws:iam::000000000000:role/account-policy-role"),
            ),
        ),
    ];
    for (case, policy_type, document) in accepted {
        let put = put_policy(&logs.ok("PutAccountPolicy", &put_body(case, policy_type, &document, None)).await);
        assert_eq!(
            member(&put, "policyDocument"),
            document,
            "case {case}: expected the document to be stored verbatim, got {put}"
        );
    }
}

#[tokio::test]
async fn documents_and_criteria_are_held_to_their_documented_bounds() {
    let logs = Logs::new();

    // The document length bound, at it and one character past it.
    let cases: &[(usize, bool)] = &[
        (MAX_POLICY_DOCUMENT_CHARS - 1, true),
        (MAX_POLICY_DOCUMENT_CHARS, true),
        (MAX_POLICY_DOCUMENT_CHARS + 1, false),
    ];
    for (length, accepted) in cases {
        let document = field_index_document_of_length(*length);
        let body = put_body(
            &format!("document-{length}"),
            "FIELD_INDEX_POLICY",
            &document,
            None,
        );
        if *accepted {
            let put = put_policy(&logs.ok("PutAccountPolicy", &body).await);
            assert_eq!(
                member(&put, "policyDocument").chars().count(),
                *length,
                "case document of {length} characters: expected it stored whole, got {put}"
            );
            logs.ok(
                "DeleteAccountPolicy",
                &json!({"policyName": format!("document-{length}"), "policyType": "FIELD_INDEX_POLICY"}),
            )
            .await;
        } else {
            let code = logs.err("PutAccountPolicy", &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case document of {length} characters: expected InvalidParameterException, got {code}"
            );
        }
    }

    // The `selectionCriteria` byte bound, measured in UTF-8 bytes on the whole criteria
    // string: a multi-byte character counts as its bytes, so a string of half as many
    // characters is still at the bound. The padding goes inside the prefix value, so each
    // case is a criteria string the member's grammar admits.
    let criteria_cases: &[(usize, char, bool)] = &[
        (MAX_SELECTION_CRITERIA_BYTES, 'a', true),
        (MAX_SELECTION_CRITERIA_BYTES + 1, 'a', false),
        (MAX_SELECTION_CRITERIA_BYTES, 'é', true),
        (MAX_SELECTION_CRITERIA_BYTES + 2, 'é', false),
    ];
    for (bytes, filler, accepted) in criteria_cases {
        let criteria = prefix_criteria_of_bytes(*bytes, *filler);
        let case = format!("{bytes} bytes of {filler}");
        assert_eq!(
            criteria.len(),
            *bytes,
            "case {case}: the case must build a criteria of {bytes} bytes, got {}",
            criteria.len()
        );
        let body = put_body(
            "criteria-bound",
            "FIELD_INDEX_POLICY",
            &field_index_document(&["requestId"]),
            Some(&criteria),
        );
        if *accepted {
            let put = put_policy(&logs.ok("PutAccountPolicy", &body).await);
            assert_eq!(
                member(&put, "selectionCriteria").len(),
                criteria.len(),
                "case {case}: expected the criteria stored whole, got {put}"
            );
        } else {
            let code = logs.err("PutAccountPolicy", &body).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {case}: expected InvalidParameterException, got {code}"
            );
        }
    }

    // "Specifying selectionCriteria is valid only when you specify
    // SUBSCRIPTION_FILTER_POLICY, FIELD_INDEX_POLICY or TRANSFORMER_POLICY for
    // policyType" — plus the metric extraction policies the operation scopes with it. A
    // data protection policy applies to all log groups, so a criteria string with one is
    // refused.
    let code = logs
        .err(
            "PutAccountPolicy",
            &put_body(
                "criteria-with-data-protection",
                "DATA_PROTECTION_POLICY",
                &data_protection_document("Name"),
                Some(r#"LogGroupNamePrefix IN ["/aws/anything"]"#),
            ),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case selectionCriteria with a data protection policy: expected InvalidParameterException, got {code}"
    );
}

#[tokio::test]
async fn a_data_protection_findings_destination_log_group_must_exist() {
    let logs = Logs::new();
    let document = |group: &str| {
        let identifiers = json!(["arn:aws:dataprotection::aws:data-identifier/EmailAddress"]);
        json!({
            "Statement": [
                {
                    "DataIdentifier": identifiers,
                    "Operation": {"Audit": {
                        "FindingsDestination": {"CloudWatchLogs": {"LogGroup": group}},
                    }},
                },
                {
                    "DataIdentifier": identifiers,
                    "Operation": {"Deidentify": {"MaskConfig": {}}},
                },
            ],
        })
        .to_string()
    };

    // "If you specify destinations such as log groups ... they must already exist."
    let code = logs
        .err(
            "PutAccountPolicy",
            &put_body(
                "findings-destination",
                "DATA_PROTECTION_POLICY",
                &document("/aws/findings/absent"),
                None,
            ),
        )
        .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case an absent findings destination: expected InvalidParameterException, got {code}"
    );

    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": "/aws/findings/present"}),
    )
    .await;
    let put = put_policy(
        &logs
            .ok(
                "PutAccountPolicy",
                &put_body(
                    "findings-destination",
                    "DATA_PROTECTION_POLICY",
                    &document("/aws/findings/present"),
                    None,
                ),
            )
            .await,
    );
    assert_eq!(
        member(&put, "policyDocument"),
        document("/aws/findings/present"),
        "case a findings destination that exists: expected the document stored verbatim, got {put}"
    );
}

#[tokio::test]
async fn deleting_a_policy_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    logs.ok(
        "PutAccountPolicy",
        &put_body(
            "held",
            "FIELD_INDEX_POLICY",
            &field_index_document(&["requestId"]),
            None,
        ),
    )
    .await;
    logs.ok(
        "PutAccountPolicy",
        &put_body(
            "deleted",
            "TRANSFORMER_POLICY",
            &transformer_document(),
            None,
        ),
    )
    .await;
    logs.ok(
        "DeleteAccountPolicy",
        &json!({"policyName": "deleted", "policyType": "TRANSFORMER_POLICY"}),
    )
    .await;

    // A policy is identified by BOTH members, so each row names a pair this account does
    // not hold: one never put, one held under a different type, one already deleted.
    let rows: &[(&str, &str, &str)] = &[
        ("never-put", "never-put", "FIELD_INDEX_POLICY"),
        ("wrong-type", "held", "METRIC_EXTRACTION_POLICY"),
        ("already-deleted", "deleted", "TRANSFORMER_POLICY"),
    ];
    for (case, policy_name, policy_type) in rows {
        let (code, message) = logs
            .refusal(
                "DeleteAccountPolicy",
                &json!({"policyName": policy_name, "policyType": policy_type}),
            )
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {code} ({message})"
        );
        assert!(
            message.contains("does not exist"),
            "case {case}: the refusal says the resource is not there, got {message}"
        );
    }

    // ... and the policy the account does hold is still there.
    let names = described_names(&logs, &json!({"policyType": "FIELD_INDEX_POLICY"})).await;
    assert_eq!(
        names,
        vec!["held".to_string()],
        "expected the refused deletes to leave the held policy alone, got {names:?}"
    );
}

/// A field index policy document of exactly `chars` characters, padded through the field
/// name it indexes.
fn field_index_document_of_length(chars: usize) -> String {
    let shell = field_index_document(&[""]).chars().count();
    let field = "f".repeat(chars.saturating_sub(shell));
    let document = field_index_document(&[field.as_str()]);
    assert_eq!(
        document.chars().count(),
        chars,
        "a padded document must be exactly {chars} characters, got {}",
        document.chars().count()
    );
    document
}

/// A `LogGroupNamePrefix IN [...]` criteria of exactly `bytes` UTF-8 bytes, padded with
/// `filler` inside the prefix value so the string the bound is measured on is one the
/// member's grammar admits.
fn prefix_criteria_of_bytes(bytes: usize, filler: char) -> String {
    let frame = r#"LogGroupNamePrefix IN [""]"#;
    let padding = bytes.saturating_sub(frame.len()) / filler.len_utf8();
    format!(
        r#"LogGroupNamePrefix IN ["{}"]"#,
        filler.to_string().repeat(padding)
    )
}

/// A `selectionCriteria` string scoping one policy of `policy_type` to its own subset of
/// log groups, in the filter form `PutAccountPolicy$selectionCriteria` documents for that
/// type: `LogGroupName NOT IN [...]` for a subscription filter policy, a
/// `LogGroupNamePrefix` filter for the others. Each `index` names a disjoint subset, so no
/// two of them select the same log groups.
pub(crate) fn scoped_criteria(policy_type: &str, index: usize) -> String {
    if policy_type == "SUBSCRIPTION_FILTER_POLICY" {
        format!(r#"LogGroupName NOT IN ["/aws/mixed-{index}"]"#)
    } else {
        format!(r#"LogGroupNamePrefix IN ["/aws/mixed-{index}"]"#)
    }
}
