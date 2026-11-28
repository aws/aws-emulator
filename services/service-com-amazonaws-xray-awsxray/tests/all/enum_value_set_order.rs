// SPDX-License-Identifier: CC0-1.0

//! The ORDER a refusal lists an enumerated type's admitted values in.
//!
//! The rule asserted here: a value outside a modelled enum is refused in AWS's
//! validation envelope whose requirement clause states the admitted set in the
//! order THIS SERVICE states that set in, which is a property of the enumerated
//! TYPE rather than of the member, the operation or the request that carried the
//! value — so every member targeting one enum reports the same bracketed list,
//! in the same order, whatever request carried it, and whatever order the model
//! happens to declare its members in. The service states most of its types
//! sorted, and the ones its own refusals are recorded stating otherwise in that
//! recorded order. The envelope, the echoed value, the lowerCamel member path
//! and the requirement's wording are the framework's; only the order inside the
//! brackets is this service's.
//!
//! The rows differ along what the rule generalises over: which enumerated type
//! the value belongs to (five of this model's, each declared in an order that is
//! NOT its sorted one for four of them), which operation and member carried it
//! (a top-level member, a member nested inside a structure), the value refused (a
//! name no enum carries, an admitted value in the wrong case, the empty string),
//! and the order the model declares the type in relative to the one the refusal
//! states — including the types this service states in neither their declared
//! nor their sorted order. A second test holds ONE type's set to the order the
//! service states it in over every spelling of a refused value a caller can
//! send, with the set read off the service's own message rather than compared
//! against a literal.
//!
//! Each value set below names where its order comes from — the `smithy_query`
//! that gives the model's declaration order, and for a type stated in
//! neither that nor the sorted order, the order this service's own refusals
//! state it in — so the test pins the order rather than restating whatever the
//! service produced.

use serde_json::{json, Value};

use super::xray_wire::{Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

/// `com.amazonaws.xray#EncryptionType`, declared `NONE, KMS`
/// (`smithy_query(query="shape",shape="EncryptionType")`).
const ENCRYPTION_TYPE_SET: &str = "[KMS, NONE]";

/// `com.amazonaws.xray#TraceSegmentDestination`, whose admitted values are
/// `XRay`, `CloudWatchLogs` and `Composite` (the third declared in this crate's
/// overlay model, see `smithy-model/LOCAL-ADDITIONS.md`), and which this service
/// states with the composite destination first — neither the sorted order nor
/// the order the type is declared in.
const TRACE_SEGMENT_DESTINATION_SET: &str = "[Composite, CloudWatchLogs, XRay]";

/// `com.amazonaws.xray#TimeRangeType`, declared `TraceId, Event, Service`
/// (`smithy_query(query="shape",shape="TimeRangeType")`) and stated by this
/// service's own refusals in neither that order nor the sorted one.
const TIME_RANGE_TYPE_SET: &str = "[TraceId, Service, Event]";

/// `com.amazonaws.xray#SamplingStrategyName`, declared `PartialScan, FixedRate`.
const SAMPLING_STRATEGY_NAME_SET: &str = "[FixedRate, PartialScan]";

/// `com.amazonaws.xray#TraceFormatType`, declared `XRAY, OTEL`.
const TRACE_FORMAT_TYPE_SET: &str = "[OTEL, XRAY]";

/// One refused request: the operation's URI, the body it sends, and the three
/// things the clause must state.
struct Case {
    label: &'static str,
    uri: &'static str,
    body: Value,
    /// The lowerCamel member path the clause names.
    member_path: &'static str,
    /// The value the request sent, which the clause echoes.
    value: &'static str,
    /// The bracketed admitted set the clause must list.
    value_set: &'static str,
}

/// AWS's one-error validation envelope over an enum-constraint clause: the value
/// as submitted, the member path, and the admitted set.
fn enum_clause(value: &str, member_path: &str, value_set: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must satisfy enum value set: {value_set}"
    )
}

/// Every row of the table, built fresh so each carries its own timestamps.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the encryption type of PutEncryptionConfig",
            uri: "/PutEncryptionConfig",
            body: json!({"Type": "NOT_A_TYPE"}),
            member_path: "type",
            value: "NOT_A_TYPE",
            value_set: ENCRYPTION_TYPE_SET,
        },
        Case {
            label: "an admitted encryption type in the wrong case",
            uri: "/PutEncryptionConfig",
            body: json!({"Type": "kms"}),
            member_path: "type",
            value: "kms",
            value_set: ENCRYPTION_TYPE_SET,
        },
        Case {
            label: "the empty string where an encryption type is admitted",
            uri: "/PutEncryptionConfig",
            body: json!({"Type": ""}),
            member_path: "type",
            value: "",
            value_set: ENCRYPTION_TYPE_SET,
        },
        Case {
            label: "the destination of UpdateTraceSegmentDestination",
            uri: "/UpdateTraceSegmentDestination",
            body: json!({"Destination": "S3"}),
            member_path: "destination",
            value: "S3",
            value_set: TRACE_SEGMENT_DESTINATION_SET,
        },
        Case {
            label: "the time-range type of GetTraceSummaries",
            uri: "/TraceSummaries",
            body: json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "TimeRangeType": "Trace",
            }),
            member_path: "timeRangeType",
            value: "Trace",
            value_set: TIME_RANGE_TYPE_SET,
        },
        Case {
            label: "a strategy name NESTED in GetTraceSummaries' sampling strategy",
            uri: "/TraceSummaries",
            body: json!({
                "StartTime": now() - 3_600.0,
                "EndTime": now(),
                "SamplingStrategy": {"Name": "FullScan", "Value": 0.5},
            }),
            member_path: "samplingStrategy.name",
            value: "FullScan",
            value_set: SAMPLING_STRATEGY_NAME_SET,
        },
        Case {
            label: "the trace format of ListRetrievedTraces",
            uri: "/ListRetrievedTraces",
            body: json!({"RetrievalToken": "a-token", "TraceFormat": "Otel"}),
            member_path: "traceFormat",
            value: "Otel",
            value_set: TRACE_FORMAT_TYPE_SET,
        },
    ]
}

#[tokio::test]
async fn a_refused_enum_value_lists_the_admitted_set_in_the_order_the_service_states_it() {
    let xray = Xray::new();

    for case in cases() {
        let refused = xray.call(case.uri, &case.body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected 400 ValidationException, got {refused:?}",
            case.label
        );
        let expected = enum_clause(case.value, case.member_path, case.value_set);
        assert_eq!(
            said(&refused),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.label,
            said(&refused)
        );
    }
}

#[tokio::test]
async fn the_stated_order_is_the_enums_own_whatever_request_carries_it() {
    let xray = Xray::new();

    // One enumerated type, refused through every spelling of a value outside it a
    // caller can send, states ONE set in ONE order: the order belongs to the type,
    // so nothing about the request moves it. The set each answer states is read off
    // the SERVICE's own message here, and then held to the order the type is listed
    // in, rather than compared against a literal.
    let spellings = [
        ("a name no value carries", json!({"Type": "NOT_A_TYPE"})),
        ("an admitted value miscased", json!({"Type": "none"})),
        ("an admitted value padded", json!({"Type": " KMS"})),
        ("the empty string", json!({"Type": ""})),
        ("a value carrying the set's own text", json!({"Type": "KMS, NONE"})),
    ];
    for (label, body) in spellings {
        let refused = xray.call("/PutEncryptionConfig", &body).await;
        let stated = stated_set(said(&refused)).unwrap_or_else(|| {
            panic!("case {label:?}: the message states no enum value set: {:?}", said(&refused))
        });
        assert_eq!(
            stated, ENCRYPTION_TYPE_SET,
            "case {label:?}: the clause must state {ENCRYPTION_TYPE_SET} however the value was \
             spelled, got {stated:?}"
        );
        // And the values the service listed are in sorted order — which for this
        // type is NOT the order the model declares them in (`NONE, KMS`), and is
        // the order this service's own refusals state it in, so a client reading
        // the admitted values off the message sees one stable order whatever a
        // model revision happens to list first.
        let listed = values_of(&stated);
        let mut sorted = listed.clone();
        sorted.sort_unstable();
        assert_eq!(
            listed, sorted,
            "case {label:?}: the service must list the admitted values sorted, got {listed:?}"
        );
        assert_eq!(
            listed,
            vec!["KMS".to_string(), "NONE".to_string()],
            "case {label:?}: the service must list every value the type admits, got {listed:?}"
        );
    }
}

/// The bracketed value set a clause states, or `None` for a message stating none.
fn stated_set(message: &str) -> Option<String> {
    let (_, set) = message.split_once("enum value set: ")?;
    Some(set.trim().to_string())
}

/// The values a bracketed set lists, in the order it lists them.
fn values_of(value_set: &str) -> Vec<String> {
    value_set
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(", ")
        .map(str::to_string)
        .collect()
}
