// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service lists an enum's admissible values in, over the enums the model
//! declares and the members that target them.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! RULE: a refusal that states an enum's admissible set lists the values in the order AWS
//! publishes for that enum — the order the enum's own documentation and its model
//! declaration state them in — and, for an enum with no order of its own published, in the
//! ascending order of the values. The order belongs to the ENUM, so one enum reads the same
//! way under every member and operation carrying it, and in both sentences this service
//! states a set in: the constraint clause of a modelled-constraint refusal, and the
//! `Valid values are: [...]` catalogue a policy-document attribute is held to.
//!
//! The tables below span what the rule generalises over: which enum, whether an order is
//! published for it, how many values it declares, how those values are spelled
//! (SCREAMING_SNAKE or mixed case), which operation and member carries it, which
//! out-of-set value the request sends, and which of the two refusal sentences states the
//! set.

use serde_json::{json, Value};

use crate::account_policies::put_body;
use crate::harness::{error_code, Logs, REGION};

/// `cloudwatchlogs#Distribution` as this service states it: the published order, the
/// random grouping first. Source: `smithy_query(query="shape", shape="Distribution")` —
/// declared `Random`, `ByLogStream`, and documented as "The method used to distribute log
/// data to the destination, which can be either random or grouped by log stream".
const DISTRIBUTION_SET: &str = "Random, ByLogStream";

/// `cloudwatchlogs#StorageTier`, which publishes no order of its own, so it is stated in
/// the ascending order of its values. Source:
/// `smithy_query(query="shape", shape="StorageTier")` — declared `STANDARD`,
/// `INTELLIGENT_TIERING`.
const STORAGE_TIER_SET: &str = "INTELLIGENT_TIERING, STANDARD";

/// `cloudwatchlogs#LogGroupClass`, four values and no published order of its own. Source:
/// `smithy_query(query="shape", shape="LogGroupClass")` — declared `STANDARD`,
/// `INFREQUENT_ACCESS`, `DELIVERY`, `ESSENTIALS`.
const LOG_GROUP_CLASS_SET: &str = "DELIVERY, ESSENTIALS, INFREQUENT_ACCESS, STANDARD";

/// `cloudwatchlogs#ListAggregateLogGroupSummariesGroupBy`, no published order of its own.
/// Source: `smithy_query(query="shape", shape="ListAggregateLogGroupSummariesGroupBy")` —
/// declared `DATA_SOURCE_NAME_TYPE_AND_FORMAT`, `DATA_SOURCE_NAME_AND_TYPE`.
const GROUP_BY_SET: &str = "DATA_SOURCE_NAME_AND_TYPE, DATA_SOURCE_NAME_TYPE_AND_FORMAT";

/// `cloudwatchlogs#OrderBy` as this service states it: the published order, the stream-name
/// value first. Source: `smithy_query(query="shape", shape="OrderBy")` — declared
/// `LogStreamName`, `LastEventTime` — and the documentation of
/// `DescribeLogStreams$orderBy`, which states them in the same order ("If the value is
/// LogStreamName, the results are ordered by log stream name. If the value is
/// LastEventTime, the results are ordered by the event time").
const ORDER_BY_SET: &str = "LogStreamName, LastEventTime";

/// The log group every row that needs one names.
const GROUP: &str = "enum-value-set-order";

/// A destination `PutSubscriptionFilter` documents, in another account's namespace: a
/// Lambda function, which needs no `roleArn`.
fn destination_arn() -> String {
    format!("arn:aws:lambda:{REGION}:123456789012:function:log-consumer")
}

/// A `PutSubscriptionFilter` body whose every other member is within its modelled
/// constraints, so the `distribution` it names is the only thing that can be refused.
fn put_filter_body(distribution: &str) -> Value {
    json!({
        "logGroupName": GROUP,
        "filterName": "ordered-set",
        "filterPattern": "",
        "destinationArn": destination_arn(),
        "distribution": distribution,
    })
}

/// One refusal row: the request, the member whose value was outside its enum, and the
/// admissible set the clause must state.
struct Case {
    name: &'static str,
    operation: &'static str,
    body: Value,
    member: &'static str,
    value_set: &'static str,
}

/// The rows: both enums an order is published for and enums where none is, two and three
/// value sets, SCREAMING_SNAKE and mixed-case values, five operations, and out-of-set
/// values of different forms (an unknown word, a declared value in the wrong case, the
/// empty string).
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "PutSubscriptionFilter distribution naming an unknown grouping",
            operation: "PutSubscriptionFilter",
            body: put_filter_body("ByShard"),
            member: "distribution",
            value_set: DISTRIBUTION_SET,
        },
        Case {
            name: "PutSubscriptionFilter distribution naming a declared value in lower case",
            operation: "PutSubscriptionFilter",
            body: put_filter_body("random"),
            member: "distribution",
            value_set: DISTRIBUTION_SET,
        },
        Case {
            name: "PutSubscriptionFilter distribution naming the empty string",
            operation: "PutSubscriptionFilter",
            body: put_filter_body(""),
            member: "distribution",
            value_set: DISTRIBUTION_SET,
        },
        Case {
            name: "PutStorageTierPolicy storageTier outside its enum",
            operation: "PutStorageTierPolicy",
            body: json!({"storageTier": "GLACIER"}),
            member: "storageTier",
            value_set: STORAGE_TIER_SET,
        },
        Case {
            name: "CreateLogGroup logGroupClass outside its enum",
            operation: "CreateLogGroup",
            body: json!({"logGroupName": "a-class-of-its-own", "logGroupClass": "NOT_A_CLASS"}),
            member: "logGroupClass",
            value_set: LOG_GROUP_CLASS_SET,
        },
        Case {
            name: "ListAggregateLogGroupSummaries groupBy outside its enum",
            operation: "ListAggregateLogGroupSummaries",
            body: json!({"groupBy": "DATA_SOURCE"}),
            member: "groupBy",
            value_set: GROUP_BY_SET,
        },
        Case {
            name: "DescribeLogStreams orderBy outside its enum",
            operation: "DescribeLogStreams",
            body: json!({"logGroupName": GROUP, "orderBy": "CreationTime"}),
            member: "orderBy",
            value_set: ORDER_BY_SET,
        },
    ]
}

/// The clause a value outside an enum is refused with states the admissible set in the one
/// order this service lists that enum's values in, for every enum and every member
/// carrying one.
#[tokio::test]
async fn an_enum_clause_states_the_set_in_the_order_the_service_publishes() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    for case in cases() {
        let Case {
            name,
            operation,
            body,
            member,
            value_set,
        } = case;
        let expected = format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{value_set}]"
        );
        let (code, message) = logs.refusal(operation, &body).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {name:?}: expected InvalidParameterException, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The set a clause states is the set the operation ACCEPTS: every value the refusal lists
/// for `distribution` is admitted by a put and read back on the stored filter, and the
/// listing order is the published one rather than the order a client happens to ask in.
#[tokio::test]
async fn every_value_an_enum_clause_lists_is_accepted_and_stored() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let (_, message) = logs
        .refusal("PutSubscriptionFilter", &put_filter_body("ByShard"))
        .await;
    let listed: Vec<&str> = message
        .split_once("enum value set: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("case distribution: the clause states a set, got {message:?}"))
        .split(", ")
        .collect();
    assert_eq!(
        listed,
        DISTRIBUTION_SET.split(", ").collect::<Vec<&str>>(),
        "case distribution: expected the published order, got {listed:?}"
    );
    // Asking in the reverse of the published order changes nothing about how the set is
    // stated: the order is the enum's, not the request's.
    for distribution in listed.iter().rev() {
        logs.ok("PutSubscriptionFilter", &put_filter_body(distribution))
            .await;
        let filters = logs
            .ok(
                "DescribeSubscriptionFilters",
                &json!({"logGroupName": GROUP, "filterNamePrefix": "ordered-set"}),
            )
            .await;
        let stored = filters
            .get("subscriptionFilters")
            .and_then(Value::as_array)
            .and_then(|filters| filters.first().cloned())
            .unwrap_or_else(|| {
                panic!("case distribution {distribution}: the filter is reported, got {filters}")
            });
        let actual = stored.get("distribution").and_then(Value::as_str);
        assert_eq!(
            actual,
            Some(*distribution),
            "case distribution {distribution}: expected it stored, got {stored}"
        );
    }
}

/// The other sentence a set is stated in reads the same way: the catalogue a
/// policy-document attribute outside its enum is refused with lists the same enum's values
/// in the same published order.
#[tokio::test]
async fn a_policy_document_catalogue_lists_the_same_published_order() {
    let logs = Logs::new();
    let expected = format!("Invalid Distribution value. Valid values are: [{DISTRIBUTION_SET}]");
    // (case, the value the document's `Distribution` attribute carries)
    let rows = [
        ("an unknown grouping", "ByShard"),
        ("a declared value in lower case", "random"),
        ("the empty string", ""),
    ];
    for (case, distribution) in rows {
        let document = json!({
            "DestinationArn": destination_arn(),
            "FilterPattern": "",
            "Distribution": distribution,
        })
        .to_string();
        let (status, parsed) = logs
            .call(
                "PutAccountPolicy",
                &put_body(
                    "ordered-set-policy",
                    "SUBSCRIPTION_FILTER_POLICY",
                    &document,
                    None,
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a document outside the enum is refused, got {status} {parsed}"
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {code}"
        );
        let message = parsed.get("message").and_then(Value::as_str);
        assert_eq!(
            message,
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}
