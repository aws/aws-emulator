// SPDX-License-Identifier: CC0-1.0

//! Three rules this suite is about, each one a client reads off the wire:
//!
//! * a SPACE-DELIMITED filter pattern of N fields describes an event of AT LEAST N fields,
//!   naming its leading fields in order — a field the list does not name is one it says
//!   nothing about, not one that makes the event a different shape — while an event with
//!   FEWER fields than the list names is not described by it at all;
//! * a `policyDocument` that breaks a quota `PutIndexPolicy` states for "each index policy"
//!   is a parameter stated incorrectly (`InvalidParameterException`), never a count of the
//!   resources the account holds;
//! * a `dataSource` TYPE is spelled the way a data source type is spelled — lower-case
//!   letters, digits and underscores — so the same word in capitals, which is how a
//!   vended-log `logType` is spelled, names no data source type.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! Each table spans what its rule generalises over rather than one request: for the pattern,
//! how many fields the list names against how many the event carries (fewer, equal, more),
//! whether the list carries a condition, whether a field of the event is a `[…]` or `"…"`
//! GROUP, and which operation runs the pattern (`TestMetricFilter` and the metric filter a
//! put configures, read back through the extraction `FilterLogEvents` reports); for the index
//! policy, the field count at the quota, one past it and far past it, and both members a
//! document names fields through; for the data source type, both spellings of one word, each
//! character class the spelling admits, and the one-character ends.

use serde_json::{json, Value};

use crate::harness::{base_ms, Logs, REGION};

/// How many field indexes one index policy can name. Source: `PutIndexPolicy` — "Each index
/// policy has the following quotas and restrictions: As many as 20 fields can be included in
/// the policy."
const MAX_INDEX_POLICY_FIELDS: usize = 20;

/// The error class a parameter stated incorrectly travels under.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The class the S3-Table-Integration family refuses a parameter it will not accept with.
const VALIDATION: &str = "ValidationException";

/// The whole sentence a document naming more field indexes than the quota admits is refused
/// with. Stated here independently of the crate under test.
fn over_field_quota_message() -> String {
    format!("Policy document contains more than {MAX_INDEX_POLICY_FIELDS} fields.")
}

/// The whole sentence a data source TYPE that is not spelled the way one is spelled is
/// refused with — one sentence for either fault, a character outside the set and a letter in
/// the wrong case. Stated here independently of the crate under test.
const SOURCE_TYPE_MESSAGE: &str =
    "Source type must only contain letters, numbers, and underscores";

/// One row of the alignment table: a space-delimited pattern, the event it is run over, and
/// whether the pattern describes that event.
struct Alignment {
    case: &'static str,
    pattern: &'static str,
    message: &'static str,
    matches: bool,
}

/// The rows: the field count of the list against the field count of the event, with and
/// without conditions, with and without a grouped field.
fn alignment_rows() -> Vec<Alignment> {
    vec![
        // The recorded case: a list naming the first three fields of a space-delimited event
        // that carries a fourth, which is a `[…]` GROUP and so one field.
        Alignment {
            case: "three slots over an event of four fields, the fourth a bracket group",
            pattern: "[ip, identity, user_id]",
            message: "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700]",
            matches: true,
        },
        Alignment {
            case: "three slots over an event of exactly three fields",
            pattern: "[ip, identity, user_id]",
            message: "127.0.0.1 - frank",
            matches: true,
        },
        Alignment {
            case: "three slots over an event of two fields",
            pattern: "[ip, identity, user_id]",
            message: "127.0.0.1 -",
            matches: false,
        },
        Alignment {
            case: "one slot over an event of seven fields",
            pattern: "[ip]",
            message: "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700] \"GET / HTTP/1.0\" 200 2326",
            matches: true,
        },
        Alignment {
            case: "a condition on a leading field the event satisfies",
            pattern: "[ip, identity, user_id=frank]",
            message: "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700]",
            matches: true,
        },
        Alignment {
            case: "a condition on a leading field the event does not satisfy",
            pattern: "[ip, identity, user_id=anna]",
            message: "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700]",
            matches: false,
        },
        Alignment {
            case: "a quoted group counted as the one field it is",
            pattern: "[ip, request]",
            message: "127.0.0.1 \"GET / HTTP/1.0\" 200",
            matches: true,
        },
        Alignment {
            case: "the wildcard still aligns the slots after it to the event's end",
            pattern: "[..., status_code, bytes]",
            message: "127.0.0.1 - frank 200 2326",
            matches: true,
        },
        Alignment {
            case: "a numeric condition on a field the wildcard aligned from the end",
            pattern: "[..., status_code, bytes>1000]",
            message: "127.0.0.1 - frank 200 128",
            matches: false,
        },
    ]
}

/// The matched event numbers `TestMetricFilter` reports for `pattern` over one message.
async fn matched(logs: &Logs, pattern: &str, message: &str) -> Vec<Value> {
    let reported = logs
        .ok(
            "TestMetricFilter",
            &json!({"filterPattern": pattern, "logEventMessages": [message]}),
        )
        .await;
    reported
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {pattern:?}: a verdict carries matches, got {reported}"))
}

/// RULE 1, through the operation that exists to report a pattern's verdict.
#[tokio::test]
async fn a_space_delimited_list_describes_an_event_of_at_least_as_many_fields() {
    let logs = Logs::new();

    for row in alignment_rows() {
        let reported = matched(&logs, row.pattern, row.message).await;
        assert_eq!(
            !reported.is_empty(),
            row.matches,
            "case {}: {:?} over {:?}: expected matches={}, got {reported:?}",
            row.case,
            row.pattern,
            row.message,
            row.matches
        );
        if row.matches {
            assert_eq!(
                reported[0].get("eventMessage").and_then(Value::as_str),
                Some(row.message),
                "case {}: a match echoes the message it matched, got {}",
                row.case,
                reported[0]
            );
        }
    }
}

/// RULE 1 again, through the OTHER side that runs a filter pattern: a metric filter the
/// service stores, whose extraction `FilterLogEvents` reports for the events it selected. The
/// same list, over the same event, selects it there too.
#[tokio::test]
async fn the_same_alignment_decides_which_ingested_events_a_metric_filter_selects() {
    let logs = Logs::new();
    let group = "/alignment/ingested";
    let stream = "events";

    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": group, "logStreamName": stream}),
    )
    .await;
    logs.ok(
        "PutMetricFilter",
        &json!({
            "logGroupName": group,
            "filterName": "leading-fields",
            "filterPattern": "[ip, identity, user_id]",
            "metricTransformations": [{
                "metricName": "Requests",
                "metricNamespace": "Alignment",
                "metricValue": "1",
            }],
        }),
    )
    .await;

    // One event of four fields, one of exactly three and one of two: the pattern names three,
    // so the first two are selected and the third is not.
    let messages = [
        "127.0.0.1 - frank [10/Oct/2000:13:25:15 -0700]",
        "10.0.0.7 - anna",
        "10.0.0.8 -",
    ];
    let events: Vec<Value> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            json!({"timestamp": base_ms() + index as i64, "message": message})
        })
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({"logGroupName": group, "logStreamName": stream, "logEvents": events}),
    )
    .await;

    let page = logs
        .ok(
            "FilterLogEvents",
            &json!({"logGroupName": group, "filterPattern": "[ip, identity, user_id]"}),
        )
        .await;
    let selected: Vec<&str> = page
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a search reports events, got {page}"))
        .iter()
        .filter_map(|event| event.get("message").and_then(Value::as_str))
        .collect();
    assert_eq!(
        selected,
        vec![messages[0], messages[1]],
        "case \"three slots over events of four, three and two fields\": expected the first \
         two events, got {selected:?}"
    );
}

/// A field index policy document naming `count` distinct fields through `Fields`.
fn fields_document(count: usize) -> String {
    let fields: Vec<String> = (0..count).map(|index| format!("field{index}")).collect();
    json!({"Fields": fields}).to_string()
}

/// The same document written through `FieldsV2`, where each field carries its index type.
fn fields_v2_document(count: usize) -> String {
    let mut entries = serde_json::Map::new();
    for index in 0..count {
        entries.insert(format!("field{index}"), json!({"type": "FIELD_INDEX"}));
    }
    json!({"FieldsV2": entries}).to_string()
}

/// RULE 2: the per-policy field quota is a fault of the DOCUMENT, so it is refused as the
/// parameter it is and the group keeps whatever policy it held.
#[tokio::test]
async fn a_document_over_the_per_policy_field_quota_is_an_invalid_parameter() {
    let logs = Logs::new();
    let group = "/index/field-quota";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    // At the quota, through either member, the document is accepted — which is what shows the
    // refusals below are the quota rather than the size of any one document.
    for (case, document) in [
        ("Fields at the quota", fields_document(MAX_INDEX_POLICY_FIELDS)),
        (
            "FieldsV2 at the quota",
            fields_v2_document(MAX_INDEX_POLICY_FIELDS),
        ),
    ] {
        logs.ok(
            "PutIndexPolicy",
            &json!({"logGroupIdentifier": group, "policyDocument": document}),
        )
        .await;
        let described = logs
            .ok("DescribeIndexPolicies", &json!({"logGroupIdentifiers": [group]}))
            .await;
        let stored = described
            .get("indexPolicies")
            .and_then(Value::as_array)
            .and_then(|policies| policies.first())
            .and_then(|policy| policy.get("policyDocument"))
            .and_then(Value::as_str);
        assert_eq!(
            stored,
            Some(document.as_str()),
            "case {case}: the group holds the document the put named, got {described}"
        );
    }

    let held = logs
        .ok("DescribeIndexPolicies", &json!({"logGroupIdentifiers": [group]}))
        .await;

    // One past the quota, far past it, and either member: one class, one sentence.
    for (case, document) in [
        (
            "one field past the quota",
            fields_document(MAX_INDEX_POLICY_FIELDS + 1),
        ),
        (
            "fifteen times the quota",
            fields_document(MAX_INDEX_POLICY_FIELDS * 15),
        ),
        (
            "one field past the quota through FieldsV2",
            fields_v2_document(MAX_INDEX_POLICY_FIELDS + 1),
        ),
    ] {
        let (code, message) = logs
            .refusal(
                "PutIndexPolicy",
                &json!({"logGroupIdentifier": group, "policyDocument": document}),
            )
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (INVALID_PARAMETER, over_field_quota_message().as_str()),
            "case {case}: expected ({INVALID_PARAMETER}, {:?}), got ({code}, {message:?})",
            over_field_quota_message()
        );
    }

    // Every refusal wrote nothing: the group still holds the last accepted document.
    let after = logs
        .ok("DescribeIndexPolicies", &json!({"logGroupIdentifiers": [group]}))
        .await;
    assert_eq!(
        after, held,
        "case \"after every refusal\": expected the stored policy {held}, got {after}"
    );
}

/// RULE 3: the spelling of a data source TYPE, over one S3 Table Integration of the account.
#[tokio::test]
async fn a_data_source_type_is_spelled_the_way_a_data_source_type_is() {
    let logs = Logs::new();
    let integration_arn = crate::harness::seed_s3_table_integration(&logs, REGION, "type-lake");

    let associate = |source_type: &'static str| {
        json!({
            "integrationArn": integration_arn,
            "dataSource": {"name": "amazon_vpc", "type": source_type},
        })
    };

    // Every character class the spelling admits, at the one-character end and beyond it.
    for source_type in ["application_logs", "flow", "a", "7", "_", "ocsf_vpc_flow_logs_2"] {
        let answered = logs
            .ok("AssociateSourceToS3TableIntegration", &associate(source_type))
            .await;
        assert!(
            answered
                .get("identifier")
                .and_then(Value::as_str)
                .is_some_and(|identifier| !identifier.is_empty()),
            "case {source_type:?}: an accepted associate carries an identifier, got {answered}"
        );
    }

    // The same words with a capital in them are no data source type, however the capital
    // falls: that spelling belongs to the vended-log `logType` catalogue.
    for source_type in [
        "APPLICATION_LOGS",
        "Application_logs",
        "application_Logs",
        "FLOW",
        "A",
    ] {
        let (code, message) = logs
            .refusal("AssociateSourceToS3TableIntegration", &associate(source_type))
            .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (VALIDATION, SOURCE_TYPE_MESSAGE),
            "case {source_type:?}: expected ({VALIDATION}, {SOURCE_TYPE_MESSAGE:?}), got \
             ({code}, {message:?})"
        );
    }
}
