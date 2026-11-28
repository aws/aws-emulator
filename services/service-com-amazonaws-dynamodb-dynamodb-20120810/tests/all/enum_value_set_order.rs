// SPDX-License-Identifier: CC0-1.0

//! What a constraint clause states when a value falls outside an enumerated member's
//! admissible set — and that a refused request changed nothing.
//!
//! The rule pinned here: a refusal of a value outside an enumerated member's set states
//! that set in the order THIS SERVICE states it in, which is a property of the enum and
//! not of the model's declaration order, with the rest of the clause unchanged — the
//! `<N> validation error(s) detected:` envelope, the member's lowerCamel path and the
//! rejected value echoed from the request.
//!
//! Each suite is a TABLE spanning what the rule generalises over rather than one recorded
//! request: which enumerated member is broken (and so which set is stated), which
//! operation carries it, how deep in the request the member sits (top level, a member of a
//! nested structure, a member of a structure inside a list), and the FORM of the rejected
//! value (a misspelling, the right value in the wrong case, an empty string, a value
//! belonging to another enum). Everything is driven over the wire, so nothing is asserted
//! that a client could not observe.

use serde_json::{json, Value};

use crate::emulator::{
    call, call_streams, describe_when_active, emulator_with_streams, error_code, message, ok,
};

/// The order DynamoDB states `StreamSpecification$StreamViewType`'s admissible set in.
///
/// It is the SERVICE's order, not the model's: the model declares the same four values as
/// `NEW_IMAGE OLD_IMAGE NEW_AND_OLD_IMAGES KEYS_ONLY`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#StreamViewType")`), while
/// the service's refusals state them in this one.
const STREAM_VIEW_TYPE_SET: &[&str] =
    &["OLD_IMAGE", "KEYS_ONLY", "NEW_AND_OLD_IMAGES", "NEW_IMAGE"];

/// Every value the stream view type accepts, in the model's declaration order
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#StreamViewType")`) — the
/// values a corrected request may name, each of which must enable a stream capturing it.
const VIEW_TYPES: &[&str] = &["NEW_IMAGE", "OLD_IMAGE", "NEW_AND_OLD_IMAGES", "KEYS_ONLY"];

/// The whole message a refusal of ONE value outside ONE enumerated member's set carries:
/// the clause, inside the counting envelope for every member of the input itself, and
/// stated BARE for the members whose refusal DynamoDB states without it.
fn expected_clause(value: &str, path: &str, set: &[&str], envelope: &str) -> String {
    format!(
        "{envelope}Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        set.join(", ")
    )
}

/// The envelope a refusal of a member of the input itself carries.
const ENVELOPE: &str = "1 validation error detected: ";

/// The envelope a refusal of one of the BARE members carries, which is none: the comparator
/// of a legacy condition entry, the projection type of an index declaration and a vector
/// index's distance function are each stated as the clause alone.
const BARE: &str = "";

/// The members whose enum refusal DynamoDB states bare, each named by the member the clause
/// names it with (the last segment of the path), so the same member is matched wherever the
/// input nests it.
const BARE_MEMBERS: &[&str] = &["comparisonOperator", "distanceFunction", "projectionType"];

/// The envelope the refusal of the member at `path` carries.
///
/// `pub(crate)` so the suite that reads the same sets back over other carriers
/// (`crate::scratch_enum_order`) states the split from here rather than from a second copy.
pub(crate) fn envelope_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some(member) if BARE_MEMBERS.contains(&member) => BARE,
        _ => ENVELOPE,
    }
}

/// A `CreateTable` body for a table keyed on `Pk` alone, with `extra` merged in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Pk", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    body
}

/// One row of the refusal table.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: fn() -> Value,
    /// The rejected value, as the clause must echo it.
    value: &'static str,
    /// The member path the clause must name.
    path: &'static str,
    /// The admissible set the clause must state, in the order it must state it.
    set: &'static [&'static str],
}

/// The sets stated for the members below, each named where it comes from.
/// `ReturnConsumedCapacity` (`INDEXES TOTAL NONE`), `ProjectionType` (`ALL KEYS_ONLY
/// INCLUDE`) and `TableClass` (`STANDARD STANDARD_INFREQUENT_ACCESS`) are stated in their
/// model-declared order (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#…")`) —
/// the BOUNDARY for the rule, since an enum the service is not observed to reorder keeps the
/// order the model declares; `StreamViewType` is stated in [`STREAM_VIEW_TYPE_SET`] and
/// `Select` in [`SELECT_SET`].
const CONSUMED_CAPACITY_SET: &[&str] = &["INDEXES", "TOTAL", "NONE"];

/// The order DynamoDB states `Select`'s admissible set in — the SERVICE's order again: the
/// model declares the same four values as `ALL_ATTRIBUTES ALL_PROJECTED_ATTRIBUTES
/// SPECIFIC_ATTRIBUTES COUNT`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#Select")`), while the
/// service's refusals state them in this one.
///
/// `pub(crate)` so the suite that reads the same set back over other surfaces
/// (`crate::scratch_enum_order`) states it from here rather than from a second copy.
pub(crate) const SELECT_SET: &[&str] = &[
    "SPECIFIC_ATTRIBUTES",
    "COUNT",
    "ALL_ATTRIBUTES",
    "ALL_PROJECTED_ATTRIBUTES",
];
const PROJECTION_TYPE_SET: &[&str] = &["ALL", "KEYS_ONLY", "INCLUDE"];
const TABLE_CLASS_SET: &[&str] = &["STANDARD", "STANDARD_INFREQUENT_ACCESS"];

/// The order DynamoDB states `IncrementalExportSpecification$ExportViewType`'s admissible
/// set in — the SERVICE's order again: the model declares the two values as
/// `NEW_IMAGE NEW_AND_OLD_IMAGES`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ExportViewType")`), while
/// the service's refusals state them in this one.
const EXPORT_VIEW_TYPE_SET: &[&str] = &["NEW_AND_OLD_IMAGES", "NEW_IMAGE"];

/// The order DynamoDB states `ExportTableToPointInTime$ExportFormat`'s admissible set in —
/// the SERVICE's order again: the model declares the two values as `DYNAMODB_JSON ION`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ExportFormat")`), while the
/// service's refusals state them in this one.
const EXPORT_FORMAT_SET: &[&str] = &["ION", "DYNAMODB_JSON"];

/// The order DynamoDB states `AttributeDefinition$AttributeType`'s admissible set in — the
/// SERVICE's order again: the model declares the three scalar types as `S N B`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ScalarAttributeType")`), while
/// the service's refusals state them in this one.
const SCALAR_ATTRIBUTE_TYPE_SET: &[&str] = &["B", "N", "S"];

/// The order DynamoDB states `Condition$ComparisonOperator`'s admissible set in — the
/// SERVICE's order again: the model declares the same thirteen values as `EQ NE IN LE LT GE
/// GT BETWEEN NOT_NULL NULL CONTAINS NOT_CONTAINS BEGINS_WITH`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ComparisonOperator")`), while
/// the service's refusals state them in this one.
///
/// `pub(crate)` for the same reason as [`SELECT_SET`].
pub(crate) const COMPARISON_OPERATOR_SET: &[&str] = &[
    "IN",
    "NULL",
    "BETWEEN",
    "LT",
    "NOT_CONTAINS",
    "EQ",
    "GT",
    "NOT_NULL",
    "NE",
    "LE",
    "BEGINS_WITH",
    "GE",
    "CONTAINS",
];

/// An `ExportTableToPointInTime` body carrying both members the model makes required, with
/// `extra` merged in: a constraint on an enumerated member is reported before the export's
/// own handler runs, so no row needs a recoverable table behind it.
fn export_body(extra: Value) -> Value {
    let mut body = json!({"TableArn": "Music", "S3Bucket": "exports"});
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    body
}

/// The table of refusals. The `Music` table exists and is active before any row runs, so
/// no row is answered by a missing table instead of by the constraint.
const ROWS: &[Row] = &[
    Row {
        case: "a misspelled stream view type, on an update",
        operation: "UpdateTable",
        request: || {
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true,
                                        "StreamViewType": "NEW_AND_OLD_IMAGE"},
            })
        },
        value: "NEW_AND_OLD_IMAGE",
        path: "streamSpecification.streamViewType",
        set: STREAM_VIEW_TYPE_SET,
    },
    Row {
        case: "a declared stream view type in the wrong case",
        operation: "UpdateTable",
        request: || {
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "keys_only"},
            })
        },
        value: "keys_only",
        path: "streamSpecification.streamViewType",
        set: STREAM_VIEW_TYPE_SET,
    },
    Row {
        case: "an empty stream view type",
        operation: "UpdateTable",
        request: || {
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": ""},
            })
        },
        value: "",
        path: "streamSpecification.streamViewType",
        set: STREAM_VIEW_TYPE_SET,
    },
    Row {
        case: "a stream view type taken from another enum",
        operation: "UpdateTable",
        request: || {
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "TOTAL"},
            })
        },
        value: "TOTAL",
        path: "streamSpecification.streamViewType",
        set: STREAM_VIEW_TYPE_SET,
    },
    Row {
        case: "the same member on a create, where the stream is turned off anyway",
        operation: "CreateTable",
        request: || {
            create_body(
                "another",
                json!({"StreamSpecification": {"StreamEnabled": false,
                                               "StreamViewType": "NEW_IMAGES"}}),
            )
        },
        value: "NEW_IMAGES",
        path: "streamSpecification.streamViewType",
        set: STREAM_VIEW_TYPE_SET,
    },
    Row {
        case: "a top-level enumerated member on a read",
        operation: "GetItem",
        request: || {
            json!({"TableName": "Music", "Key": {"Pk": {"S": "a"}},
                   "ReturnConsumedCapacity": "EVERYTHING"})
        },
        value: "EVERYTHING",
        path: "returnConsumedCapacity",
        set: CONSUMED_CAPACITY_SET,
    },
    Row {
        case: "another top-level enumerated member, on a query",
        operation: "Query",
        request: || {
            json!({"TableName": "Music", "Select": "ALL",
                   "KeyConditionExpression": "Pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "a"}}})
        },
        value: "ALL",
        path: "select",
        set: SELECT_SET,
    },
    Row {
        case: "an enumerated member of a table class",
        operation: "UpdateTable",
        request: || json!({"TableName": "Music", "TableClass": "STANDARD_IA"}),
        value: "STANDARD_IA",
        path: "tableClass",
        set: TABLE_CLASS_SET,
    },
    Row {
        case: "an enumerated member inside a structure inside a list",
        operation: "CreateTable",
        request: || {
            create_body(
                "indexed",
                json!({
                    "GlobalSecondaryIndexes": [{
                        "IndexName": "by-payload",
                        "KeySchema": [{"AttributeName": "Pk", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "EVERY_ATTRIBUTE"},
                    }],
                }),
            )
        },
        value: "EVERY_ATTRIBUTE",
        path: "globalSecondaryIndexes.1.member.projection.projectionType",
        set: PROJECTION_TYPE_SET,
    },
    Row {
        case: "an enumerated member of a nested structure, misspelled",
        operation: "ExportTableToPointInTime",
        request: || {
            export_body(json!({
                "ExportType": "INCREMENTAL_EXPORT",
                "IncrementalExportSpecification": {"ExportViewType": "NEW_AND_OLD_IMAGE"},
            }))
        },
        value: "NEW_AND_OLD_IMAGE",
        path: "incrementalExportSpecification.exportViewType",
        set: EXPORT_VIEW_TYPE_SET,
    },
    Row {
        case: "the same member with a declared value in the wrong case",
        operation: "ExportTableToPointInTime",
        request: || {
            export_body(json!({
                "IncrementalExportSpecification": {"ExportViewType": "new_image"},
            }))
        },
        value: "new_image",
        path: "incrementalExportSpecification.exportViewType",
        set: EXPORT_VIEW_TYPE_SET,
    },
    Row {
        case: "the same member carrying a value that belongs to the stream view enum",
        operation: "ExportTableToPointInTime",
        request: || {
            export_body(json!({
                "IncrementalExportSpecification": {"ExportViewType": "KEYS_ONLY"},
            }))
        },
        value: "KEYS_ONLY",
        path: "incrementalExportSpecification.exportViewType",
        set: EXPORT_VIEW_TYPE_SET,
    },
    Row {
        case: "an enumerated member the service states in an order of its own",
        operation: "ExportTableToPointInTime",
        request: || export_body(json!({"ExportFormat": "PARQUET"})),
        value: "PARQUET",
        path: "exportFormat",
        set: EXPORT_FORMAT_SET,
    },
    Row {
        case: "the scalar type of a key attribute, misspelled, on a create",
        operation: "CreateTable",
        request: || {
            create_body(
                "typed",
                json!({"AttributeDefinitions": [{"AttributeName": "Pk",
                                                "AttributeType": "STRING"}]}),
            )
        },
        value: "STRING",
        path: "attributeDefinitions.1.member.attributeType",
        set: SCALAR_ATTRIBUTE_TYPE_SET,
    },
    Row {
        case: "the same scalar type in the wrong case, on a second definition",
        operation: "CreateTable",
        request: || {
            create_body(
                "typed-again",
                json!({"AttributeDefinitions": [{"AttributeName": "Pk",
                                                "AttributeType": "S"},
                                               {"AttributeName": "Sk",
                                                "AttributeType": "n"}]}),
            )
        },
        value: "n",
        path: "attributeDefinitions.2.member.attributeType",
        set: SCALAR_ATTRIBUTE_TYPE_SET,
    },
    Row {
        case: "the comparator of a legacy filter entry, in the wrong case",
        operation: "Scan",
        request: || {
            json!({"TableName": "Music",
                   "ScanFilter": {"tag": {"ComparisonOperator": "eq",
                                          "AttributeValueList": [{"S": "a"}]}}})
        },
        value: "eq",
        path: "scanFilter.tag.member.comparisonOperator",
        set: COMPARISON_OPERATOR_SET,
    },
    Row {
        case: "the same comparator on a legacy key condition, misspelled",
        operation: "Query",
        request: || {
            json!({"TableName": "Music",
                   "KeyConditions": {"Pk": {"ComparisonOperator": "EQUALS",
                                            "AttributeValueList": [{"S": "a"}]}}})
        },
        value: "EQUALS",
        path: "keyConditions.Pk.member.comparisonOperator",
        set: COMPARISON_OPERATOR_SET,
    },
];

#[tokio::test]
async fn an_enum_refusal_states_the_admissible_set_in_the_order_the_service_states_it() {
    let router = emulator_with_streams();
    ok(&router, "CreateTable", create_body("Music", json!({}))).await;
    describe_when_active(&router, "the subject table", "Music").await;

    for row in ROWS {
        let case = row.case;
        let (status, refused) = call(&router, row.operation, (row.request)()).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({}): expected HTTP 400, got {status} with {refused}",
            row.operation
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?} ({}): expected ValidationException, got {refused}",
            row.operation
        );
        let expected = expected_clause(row.value, row.path, row.set, envelope_for(row.path));
        let stated = message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?} ({}): expected the message {expected:?}, got {stated:?}",
            row.operation
        );
    }
}

#[tokio::test]
async fn a_refused_stream_specification_enables_nothing_and_the_corrected_one_enables_one() {
    // One row per view type the model declares: each is first asked for by a misspelling
    // of itself, which must be refused and must enable nothing, and then by the value
    // itself, which must enable exactly one stream capturing it.
    for view_type in VIEW_TYPES {
        let case = format!("asking for {view_type}");
        let router = emulator_with_streams();
        ok(&router, "CreateTable", create_body("Music", json!({}))).await;
        describe_when_active(&router, &case, "Music").await;

        // A misspelling: the published value with its last character dropped, which is
        // outside the set whatever the value was.
        let misspelled: String = view_type[..view_type.len() - 1].to_string();
        let (status, refused) = call(
            &router,
            "UpdateTable",
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": misspelled},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a misspelled view type must be refused, got {status} with {refused}"
        );
        let expected = expected_clause(&misspelled, "streamSpecification.streamViewType",
                                       STREAM_VIEW_TYPE_SET, ENVELOPE);
        let stated = message(&refused);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );

        // Nothing was enabled: the table reports no stream at all, and the streams
        // service reports none for it.
        let described = ok(&router, "DescribeTable", json!({"TableName": "Music"})).await;
        assert!(
            described["Table"].get("StreamSpecification").is_none()
                && described["Table"].get("LatestStreamArn").is_none(),
            "case {case:?}: a refused update enables no stream, got {described}"
        );
        let listed = streams_of(&router, &case, "Music").await;
        assert_eq!(
            listed,
            Vec::<Value>::new(),
            "case {case:?}: a refused update leaves the table with no stream, got {listed:?}"
        );

        // The corrected request enables exactly one stream, capturing what it asked for.
        ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": "Music",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
            }),
        )
        .await;
        let described = describe_when_active(&router, &case, "Music").await;
        assert_eq!(
            described["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: the table must report the stream it was asked for, got {described}"
        );
        let arn = described["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: an enabled stream must be reported by ARN, got {described}"
        );

        let listed = streams_of(&router, &case, "Music").await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: exactly one stream must exist for the table, got {listed:?}"
        );
        assert_eq!(
            listed[0]["StreamArn"],
            json!(arn),
            "case {case:?}: the listed stream must be the one the table reports, got {listed:?}"
        );
        let (status, described_stream) =
            call_streams(&router, "DescribeStream", json!({"StreamArn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStream answered {status} with {described_stream}"
        );
        assert_eq!(
            described_stream["StreamDescription"]["StreamViewType"],
            json!(view_type),
            "case {case:?}: the stream must capture what the update asked for, got \
             {described_stream}"
        );
    }
}

/// Every stream the STREAMS service reports for `table`.
async fn streams_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
) -> Vec<Value> {
    let (status, listed) =
        call_streams(router, "ListStreams", json!({"TableName": table})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListStreams answered {status} with {listed}"
    );
    listed["Streams"].as_array().cloned().unwrap_or_default()
}
