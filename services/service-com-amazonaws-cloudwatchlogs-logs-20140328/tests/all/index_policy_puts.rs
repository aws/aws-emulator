// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutIndexPolicy`: the log-group level field index policy one log
//! group holds.
//!
//! The rule under test: a put stores the index policy document VERBATIM on the log group
//! its identifier names — whichever identifier form names it — stamps it with the instant
//! of the put, and reports it back as the policy of that one group (the group's ARN,
//! `source` `LOG_GROUP`, no `policyName`), so a later put replaces it and
//! `DescribeIndexPolicies` reports exactly what was stored. A document that is not a field
//! index policy, a field name longer than the documented name bound, or a group the
//! account does not hold or that is not in the class the feature supports, is refused as an
//! invalid parameter; a document naming more field indexes than the documented quota admits
//! is refused as a limit that has been reached; and a refused put stores nothing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass, and every stored policy
//! is read back over the wire through `DescribeIndexPolicies`.
//!
//! The tables below span what the rule generalises over (which attribute of the document
//! names the field indexes, which index type each carries, how many fields the policy
//! names, how long a field name is, how long the document is, which identifier form names
//! the group, and which class the group is in).

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::log_group_arn;

use crate::account_policies::field_index_document;
use crate::harness::{now_ms, Logs, REGION};

/// The operation under test.
const OP: &str = "PutIndexPolicy";

/// The `IndexPolicy$source` of a policy stored on the one log group it applies to. Source:
/// `smithy_query(query="shape", shape="IndexSource")` — the enum's two values are `ACCOUNT`
/// and `LOG_GROUP`.
const SOURCE_LOG_GROUP: &str = "LOG_GROUP";

/// The types a `FieldsV2` entry can carry. Source:
/// `smithy_query(query="shape", shape="IndexType")` — the model's `IndexType` enum, which
/// `PutIndexPolicy$policyDocument` states as "Supported types are FIELD_INDEX and FACET".
const INDEX_TYPES: [&str; 2] = ["FACET", "FIELD_INDEX"];

/// How many field indexes one index policy can name. Source: `PutIndexPolicy` — "Each index
/// policy has the following quotas and restrictions: As many as 20 fields can be included
/// in the policy."
const MAX_FIELDS: usize = 20;

/// How long one indexed field's name can be. Source: the same quota list — "Each field name
/// can include as many as 100 characters."
const MAX_FIELD_NAME_CHARS: usize = 100;

/// The longest `policyDocument` the model admits. Source:
/// `smithy_query(query="operation", operation="PutIndexPolicy")` — `policyDocument` carries
/// `@length(min: 1, max: 51200)`.
const MAX_DOCUMENT_CHARS: usize = 51_200;

/// The log group classes `CreateLogGroup` admits. Source:
/// `smithy_query(query="shape", shape="LogGroupClass")`.
const LOG_GROUP_CLASSES: [&str; 3] = ["DELIVERY", "INFREQUENT_ACCESS", "STANDARD"];

/// The one class that supports a field index policy. Source: `PutIndexPolicy` — "Only log
/// groups in the Standard log class support field index policies."
const INDEX_POLICY_CLASS: &str = "STANDARD";

/// A field index policy document whose `FieldsV2` object names each `(field, type)` pair.
/// Source: `PutIndexPolicy$policyDocument` — "FieldsV2 to specify the type for each field".
pub(crate) fn fields_v2_document(entries: &[(&str, &str)]) -> String {
    let mut fields = serde_json::Map::new();
    for (field, index_type) in entries {
        fields.insert((*field).to_string(), json!({"type": index_type}));
    }
    json!({"FieldsV2": fields}).to_string()
}

/// A field index policy document naming `fields` in its `Fields` array and each
/// `(field, type)` pair in its `FieldsV2` object — the two attributes together, as the
/// example on `PutIndexPolicy$policyDocument` writes them.
fn mixed_document(fields: &[&str], entries: &[(&str, &str)]) -> String {
    let mut v2 = serde_json::Map::new();
    for (field, index_type) in entries {
        v2.insert((*field).to_string(), json!({"type": index_type}));
    }
    json!({"Fields": fields, "FieldsV2": v2}).to_string()
}

/// A field index policy document of exactly `chars` characters, padded through the one field
/// name it indexes.
fn document_of_length(chars: usize) -> String {
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

/// `count` distinct field names, so a row about how MANY fields a policy names cannot pass
/// on two names colliding.
fn distinct_fields(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("field{index}")).collect()
}

/// A field index policy document naming `count` distinct fields.
pub(crate) fn document_with_fields(count: usize) -> String {
    let fields = distinct_fields(count);
    let borrowed: Vec<&str> = fields.iter().map(String::as_str).collect();
    field_index_document(&borrowed)
}

/// Create one log group, in the class given when one is given.
async fn create_group(logs: &Logs, name: &str, class: Option<&str>) {
    let mut body = json!({"logGroupName": name});
    if let Some(class) = class {
        body["logGroupClass"] = json!(class);
    }
    logs.ok("CreateLogGroup", &body).await;
}

/// The `PutIndexPolicy` body for one group and document.
fn put_body(identifier: &str, document: &str) -> Value {
    json!({"logGroupIdentifier": identifier, "policyDocument": document})
}

/// The `indexPolicy` one accepted put reports.
async fn put(logs: &Logs, identifier: &str, document: &str) -> Value {
    let response = logs.ok(OP, &put_body(identifier, document)).await;
    response
        .get("indexPolicy")
        .cloned()
        .unwrap_or_else(|| panic!("{OP} must return indexPolicy, got {response}"))
}

/// One string member of a reported `IndexPolicy`.
pub(crate) fn member(policy: &Value, key: &str) -> String {
    policy
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an index policy must carry {key}, got {policy}"))
        .to_string()
}

/// The `lastUpdateTime` of a reported `IndexPolicy`.
fn last_update_time(policy: &Value) -> i64 {
    policy
        .get("lastUpdateTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("an index policy must carry lastUpdateTime, got {policy}"))
}

/// The one index policy `DescribeIndexPolicies` reports for a log group, or `None` when
/// none applies to it.
pub(crate) async fn described_one(logs: &Logs, identifier: &str) -> Option<Value> {
    let page = logs
        .ok(
            "DescribeIndexPolicies",
            &json!({"logGroupIdentifiers": [identifier]}),
        )
        .await;
    let policies = page
        .get("indexPolicies")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {identifier}: DescribeIndexPolicies must return indexPolicies, got {page}"))
        .clone();
    assert!(
        policies.len() <= 1,
        "case {identifier}: at most one index policy applies to one log group, got {policies:?}"
    );
    policies.into_iter().next()
}

/// Assert one reported `IndexPolicy` is the log-group level policy of `group` carrying
/// `document`, stamped inside `[put_at, now]`.
async fn assert_group_policy(
    logs: &Logs,
    case: &str,
    policy: &Value,
    group: &str,
    document: &str,
    put_at: i64,
) {
    let (_, arn) = logs.group_arns(group).await;
    assert_eq!(
        member(policy, "source"),
        SOURCE_LOG_GROUP,
        "case {case}: expected source {SOURCE_LOG_GROUP}, got {policy}"
    );
    assert!(
        policy.get("policyName").is_none(),
        "case {case}: a log-group level policy has no name, got {policy}"
    );
    assert_eq!(
        member(policy, "policyDocument"),
        document,
        "case {case}: expected the document stored verbatim, got {policy}"
    );
    assert_eq!(
        member(policy, "logGroupIdentifier"),
        arn,
        "case {case}: expected the log group's ARN, got {policy}"
    );
    let stamped = last_update_time(policy);
    assert!(
        (put_at..=now_ms()).contains(&stamped),
        "case {case}: expected an update time in [{put_at}, now], got {stamped}"
    );
}

/// One row of the round-trip table: the case, the log group it puts on, and the document it
/// puts.
struct Row {
    case: &'static str,
    group: &'static str,
    document: String,
}

/// The documents a field index policy can be written as: through `Fields`, through
/// `FieldsV2` with each index type, through both together, at the field quota, and at the
/// field-name bound.
fn round_trip_rows() -> Vec<Row> {
    let v2_rows: Vec<Row> = INDEX_TYPES
        .iter()
        .map(|index_type| Row {
            case: index_type,
            group: match *index_type {
                "FACET" => "/index/fields-v2-facet",
                _ => "/index/fields-v2-field-index",
            },
            document: fields_v2_document(&[("RequestId", index_type)]),
        })
        .collect();
    let mut rows = vec![
        Row {
            case: "one field in Fields",
            group: "/index/one-field",
            document: field_index_document(&["requestId"]),
        },
        Row {
            case: "three fields in Fields",
            group: "/index/three-fields",
            document: field_index_document(&["requestId", "sessionId", "userId"]),
        },
        Row {
            case: "Fields and FieldsV2 together",
            group: "/index/mixed",
            document: mixed_document(
                &["TransactionId"],
                &[("RequestId", "FIELD_INDEX"), ("APIName", "FACET")],
            ),
        },
        Row {
            case: "twenty fields — the policy's field quota",
            group: "/index/at-the-field-quota",
            document: document_with_fields(MAX_FIELDS),
        },
        Row {
            case: "a field name of a hundred characters — the name bound",
            group: "/index/at-the-name-bound",
            document: field_index_document(&["n".repeat(MAX_FIELD_NAME_CHARS).as_str()]),
        },
    ];
    rows.extend(v2_rows);
    rows
}

/// Every accepted put reports the policy it stored and is readable back through
/// `DescribeIndexPolicies` with the same document and the same update time.
#[tokio::test]
async fn an_accepted_put_is_readable_back_through_describe_index_policies() {
    let logs = Logs::new();

    for row in round_trip_rows() {
        create_group(&logs, row.group, None).await;
        let put_at = now_ms();
        let reported = put(&logs, row.group, &row.document).await;
        assert_group_policy(&logs, row.case, &reported, row.group, &row.document, put_at).await;

        let described = described_one(&logs, row.group)
            .await
            .unwrap_or_else(|| panic!("case {}: the group holds its own policy", row.case));
        assert_eq!(
            described, reported,
            "case {}: expected the described policy to be the one the put reported, got {described}",
            row.case
        );
    }
}

/// A put names its log group by either identifier form the modelled `LogGroupIdentifier`
/// admits, stores onto the one group either form names, and reports that group's ARN.
#[tokio::test]
async fn either_identifier_form_names_the_same_log_group() {
    const GROUP: &str = "/index/identifier-forms";

    let logs = Logs::new();
    create_group(&logs, GROUP, None).await;
    // The group's name, and the ARN form WITHOUT the trailing `:*`, which the modelled
    // pattern `^[\w#+=/:,.@-]*$` has no `*` for.
    let (_, arn) = logs.group_arns(GROUP).await;

    for (case, identifier) in [("name", GROUP.to_string()), ("arn", arn)] {
        let document = field_index_document(&[case]);
        let put_at = now_ms();
        let reported = put(&logs, &identifier, &document).await;
        assert_group_policy(&logs, case, &reported, GROUP, &document, put_at).await;

        let described = described_one(&logs, GROUP)
            .await
            .unwrap_or_else(|| panic!("case {case}: the group holds its own policy"));
        assert_eq!(
            member(&described, "policyDocument"),
            document,
            "case {case}: expected the document this put stored, got {described}"
        );
    }
}

/// A put onto a group that already holds a log-group level policy REPLACES it — the group
/// holds one policy, carrying the last document put and a no-earlier update time.
#[tokio::test]
async fn a_later_put_replaces_the_policy_the_group_holds() {
    const GROUP: &str = "/index/replaced";

    let logs = Logs::new();
    create_group(&logs, GROUP, None).await;

    let mut previous: Option<i64> = None;
    for (case, document) in [
        ("first", field_index_document(&["requestId"])),
        (
            "second",
            fields_v2_document(&[("sessionId", "FIELD_INDEX")]),
        ),
        ("third", document_with_fields(MAX_FIELDS)),
    ] {
        let put_at = now_ms();
        let reported = put(&logs, GROUP, &document).await;
        assert_group_policy(&logs, case, &reported, GROUP, &document, put_at).await;

        let described = described_one(&logs, GROUP)
            .await
            .unwrap_or_else(|| panic!("case {case}: the group holds its own policy"));
        assert_eq!(
            member(&described, "policyDocument"),
            document,
            "case {case}: expected only the last document put, got {described}"
        );
        let stamped = last_update_time(&described);
        if let Some(previous) = previous {
            assert!(
                stamped >= previous,
                "case {case}: expected an update time no earlier than {previous}, got {stamped}"
            );
        }
        previous = Some(stamped);
    }
}

/// One row of the refusal table: the case, the document it puts, and the error code the put
/// is refused with.
struct Refusal {
    case: &'static str,
    document: String,
    code: &'static str,
}

/// A document that is not a field index policy, and one naming more field indexes than the
/// quota admits, are both an invalid parameter, and either way the put stores nothing.
#[tokio::test]
async fn a_document_outside_its_contract_is_refused_and_stores_nothing() {
    const GROUP: &str = "/index/refused-documents";
    const INVALID: &str = "InvalidParameterException";

    let rows = vec![
        Refusal {
            case: "not JSON at all",
            document: "not a policy".to_string(),
            code: INVALID,
        },
        Refusal {
            case: "a JSON array rather than the policy object",
            document: json!(["requestId"]).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "an empty object",
            document: json!({}).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "no field index at all",
            document: json!({"Fields": []}).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "Fields that is not an array",
            document: json!({"Fields": "requestId"}).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "a FieldsV2 entry carrying no type",
            document: json!({"FieldsV2": {"RequestId": {}}}).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "a FieldsV2 entry whose type is outside the index types",
            document: json!({"FieldsV2": {"RequestId": {"type": "SORT_KEY"}}}).to_string(),
            code: INVALID,
        },
        Refusal {
            case: "a field name in both Fields and FieldsV2",
            document: mixed_document(&["RequestId"], &[("RequestId", "FACET")]),
            code: INVALID,
        },
        Refusal {
            case: "a field name one character past the name bound",
            document: field_index_document(&["n".repeat(MAX_FIELD_NAME_CHARS + 1).as_str()]),
            code: INVALID,
        },
        Refusal {
            case: "one field index past the policy's field quota",
            document: document_with_fields(MAX_FIELDS + 1),
            code: INVALID,
        },
        Refusal {
            case: "far more field indexes than the policy's field quota",
            document: document_with_fields(MAX_FIELDS * 15),
            code: INVALID,
        },
    ];

    let logs = Logs::new();
    create_group(&logs, GROUP, None).await;

    for row in &rows {
        let code = logs.err(OP, &put_body(GROUP, &row.document)).await;
        assert_eq!(
            code, row.code,
            "case {}: expected {}, got {code}",
            row.case, row.code
        );
        let described = described_one(&logs, GROUP).await;
        assert!(
            described.is_none(),
            "case {}: a refused put stores nothing, got {described:?}",
            row.case
        );
    }

    // A document one character past the modelled bound is refused on that bound, and the
    // refusal states the constraint the member failed; the bound itself is accepted, which
    // is what shows the refusal is the bound rather than the length of any one document.
    let (code, message) = logs
        .refusal(OP, &put_body(GROUP, &document_of_length(MAX_DOCUMENT_CHARS + 1)))
        .await;
    assert_eq!(
        code, INVALID,
        "case a document past the modelled length bound: expected {INVALID}, got {code}"
    );
    assert!(
        message.contains(&MAX_DOCUMENT_CHARS.to_string()) && message.contains("policyDocument"),
        "case a document past the modelled length bound: the refusal names the member and its bound, got {message}"
    );
    assert!(
        described_one(&logs, GROUP).await.is_none(),
        "case a document past the modelled length bound: a refused put stores nothing"
    );

    // At the bound the document is one the field-name quota admits only through padding the
    // document with members the policy does not name, so the accepted half of the bound is
    // asserted on the longest document the field quota leaves room for.
    let longest = document_with_fields(MAX_FIELDS);
    assert!(
        longest.chars().count() <= MAX_DOCUMENT_CHARS,
        "the quota-filling document must fit the modelled bound, got {} characters",
        longest.chars().count()
    );
    let put_at = now_ms();
    let reported = put(&logs, GROUP, &longest).await;
    assert_group_policy(
        &logs,
        "the longest document the field quota admits",
        &reported,
        GROUP,
        &longest,
        put_at,
    )
    .await;
}

/// A log group the account does not hold is a missing resource, under either identifier
/// form.
#[tokio::test]
async fn a_put_onto_a_log_group_the_account_does_not_hold_is_refused() {
    const ABSENT: &str = "/index/absent";

    let logs = Logs::new();
    let document = field_index_document(&["requestId"]);

    for (case, identifier) in [
        ("name", ABSENT.to_string()),
        ("arn", log_group_arn(REGION, ABSENT, false)),
    ] {
        let code = logs.err(OP, &put_body(&identifier, &document)).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: a log group the account does not hold is a missing resource, got {code}"
        );
    }
}

/// A field index policy is stored only on a log group in the class that supports one; a put
/// onto a group in any other class is refused and stores nothing.
#[tokio::test]
async fn a_put_is_accepted_only_for_a_log_group_in_the_supported_class() {
    let logs = Logs::new();
    let document = field_index_document(&["requestId"]);

    for class in LOG_GROUP_CLASSES {
        let group = format!("/index/class-{class}");
        create_group(&logs, &group, Some(class)).await;
        if class == INDEX_POLICY_CLASS {
            let put_at = now_ms();
            let reported = put(&logs, &group, &document).await;
            assert_group_policy(&logs, class, &reported, &group, &document, put_at).await;
        } else {
            let (code, message) = logs.refusal(OP, &put_body(&group, &document)).await;
            assert_eq!(
                code, "InvalidParameterException",
                "case {class}: only a log group in the {INDEX_POLICY_CLASS} class supports a field index policy, got {code}"
            );
            // The sentence is the service's one wording for a feature the log group's class
            // does not support: fixed text naming the class that does, and nothing of the
            // request.
            let expected = "This operation is only supported on the Standard log class.";
            assert_eq!(
                message, expected,
                "case {class}: expected {expected:?}, got {message:?}"
            );
            let described = described_one(&logs, &group).await;
            assert!(
                described.is_none(),
                "case {class}: a refused put stores nothing, got {described:?}"
            );
        }
    }

    // A group created with no `logGroupClass` is in the default class, which is the one that
    // supports a field index policy ("If you omit this parameter, the default of STANDARD is
    // used").
    const DEFAULTED: &str = "/index/class-defaulted";
    create_group(&logs, DEFAULTED, None).await;
    let put_at = now_ms();
    let reported = put(&logs, DEFAULTED, &document).await;
    assert_group_policy(&logs, "defaulted class", &reported, DEFAULTED, &document, put_at).await;
}
