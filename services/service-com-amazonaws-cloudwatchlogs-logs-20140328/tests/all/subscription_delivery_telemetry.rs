// SPDX-License-Identifier: CC0-1.0

//! The TELEMETRY of a subscription filter's delivery, end to end over the wire with the
//! Kinesis emulator (the destination) and the CloudWatch emulator (the metric store) mounted
//! beside this service on one engine — so every datapoint is read back through
//! `cloudwatch:ListMetrics` and `cloudwatch:GetMetricStatistics`, where a client reads it.
//!
//! RULE: a delivery through a log-group-level subscription filter publishes the service's own
//! `AWS/Logs` metric data under exactly the dimensions of that filter — the log group, the
//! kind of destination and the filter's name, and nothing else, `PolicyLevel` being a
//! dimension of an account-level subscription filter — with `ForwardedLogEvents` counting the
//! events the filter's `filterPattern` SELECTED out of the ingested batch (not the events the
//! batch wrote) and `DeliveryErrors` counting an envelope the destination would not take, both
//! with the `AWS/Logs` table's unit for a count of neither bytes nor time. A destination that
//! refused is not attempted again until a `PutSubscriptionFilter` re-validates it, so the
//! events written during the outage stay readable in the log group and are never re-delivered.
//!
//! The recorded flows this was derived from write three matching and two non-matching events
//! through one filter on a Kinesis destination, and then delete that destination and write one
//! more; those are rows of the tables below. The forwarding rows differ along what the rule
//! generalises over: the form of the filter pattern, how many of the batch's events it
//! selects, how many events the batch carries, and whether the destination is named directly
//! or through a logical destination.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{base_ms, now_ms};
use crate::subscription_fan_out::{
    past_activation, past_deletion, role_arn, stream_arn, Emulator,
};

/// The namespace CloudWatch Logs publishes its own metrics into: "the AWS/Logs namespace".
const LOGS_NAMESPACE: &str = "AWS/Logs";

/// The two metrics of the delivery path under test, from the `AWS/Logs` metric table.
pub(crate) const FORWARDED_LOG_EVENTS: &str = "ForwardedLogEvents";
pub(crate) const DELIVERY_ERRORS: &str = "DeliveryErrors";

/// The dimension names a LOG-GROUP-level subscription filter's delivery metrics carry, as the
/// `AWS/Logs` table names them.
pub(crate) const LOG_GROUP_NAME: &str = "LogGroupName";
pub(crate) const DESTINATION_TYPE: &str = "DestinationType";
pub(crate) const FILTER_NAME: &str = "FilterName";

/// The `StandardUnit` value for a datapoint in no unit, which is the unit the table gives both
/// metrics.
const UNIT_NONE: &str = "None";

/// The log stream every case writes to.
const LOG_STREAM: &str = "events";

/// One row of the forwarding table.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The log group the row writes to, one per row so its series is its own.
    group: &'static str,
    /// The filter's name, which is the `FilterName` dimension value its series must carry.
    filter: &'static str,
    /// The filter's `filterPattern`.
    pattern: &'static str,
    /// Whether the filter names the destination through a `PutDestination` logical
    /// destination rather than the stream itself.
    through_logical_destination: bool,
    /// The messages the batch carries, in chronological order.
    messages: &'static [&'static str],
    /// How many of them the pattern selects — what `ForwardedLogEvents` must sum to, and
    /// `None` where the pattern selects nothing, so no delivery is owed and no series exists.
    forwarded: Option<f64>,
}

/// The forwarding table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: three of five events carry the term.
            case: "an unstructured term selecting three of five events",
            group: "/aws/emulator/forwarded-term",
            filter: "term-filter",
            pattern: "SIM2REALMATCH",
            through_logical_destination: false,
            messages: &[
                "SIM2REALMATCH one",
                "quiet two",
                "SIM2REALMATCH three",
                "quiet four",
                "SIM2REALMATCH five",
            ],
            forwarded: Some(3.0),
        },
        Row {
            case: "a match-everything pattern over a two-event batch",
            group: "/aws/emulator/forwarded-everything",
            filter: "everything-filter",
            pattern: "",
            through_logical_destination: false,
            messages: &["alpha", "beta"],
            forwarded: Some(2.0),
        },
        Row {
            case: "a JSON condition selecting one event",
            group: "/aws/emulator/forwarded-json",
            filter: "json-filter",
            pattern: "{$.level = \"WARN\"}",
            through_logical_destination: false,
            messages: &[
                "{\"level\":\"INFO\",\"msg\":\"up\"}",
                "{\"level\":\"WARN\",\"msg\":\"slow\"}",
            ],
            forwarded: Some(1.0),
        },
        Row {
            case: "a space-delimited pattern through a logical destination",
            group: "/aws/emulator/forwarded-logical",
            filter: "logical-filter",
            pattern: "[level = ERROR, ...]",
            through_logical_destination: true,
            messages: &["ERROR disk full", "INFO disk fine"],
            forwarded: Some(1.0),
        },
        Row {
            case: "a pattern selecting nothing",
            group: "/aws/emulator/forwarded-none",
            filter: "quiet-filter",
            pattern: "NOTHING_MATCHES_THIS",
            through_logical_destination: false,
            messages: &["alpha", "beta"],
            forwarded: None,
        },
    ]
}

/// The dimension set of one filter's delivery series, as CloudWatch takes it.
pub(crate) fn dimensions(group: &str, filter: &str, destination_type: &str) -> Value {
    json!([
        {"Name": LOG_GROUP_NAME, "Value": group},
        {"Name": DESTINATION_TYPE, "Value": destination_type},
        {"Name": FILTER_NAME, "Value": filter},
    ])
}

/// The dimension NAMES and the `FilterName` value of every `AWS/Logs` series of `metric` that
/// `ListMetrics` reports for the log group `group`.
pub(crate) async fn listed_series(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    group: &str,
) -> Vec<(BTreeSet<String>, String)> {
    let listed = emulator
        .cloudwatch(
            case,
            "ListMetrics",
            &json!({"Namespace": LOGS_NAMESPACE, "MetricName": metric}),
        )
        .await;
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|series| {
            let reported = series["Dimensions"].as_array()?;
            let named: Vec<(String, String)> = reported
                .iter()
                .filter_map(|dimension| {
                    Some((
                        dimension["Name"].as_str()?.to_string(),
                        dimension["Value"].as_str()?.to_string(),
                    ))
                })
                .collect();
            let of_group = named
                .iter()
                .any(|(name, value)| name == LOG_GROUP_NAME && value == group);
            if !of_group {
                return None;
            }
            let filter = named
                .iter()
                .find(|(name, _)| name == FILTER_NAME)
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            Some((
                named.iter().map(|(name, _)| name.clone()).collect(),
                filter,
            ))
        })
        .collect()
}

/// The datapoints CloudWatch reports for one series over a window that holds every delivery a
/// case makes: from before the batch's own events to past the instant this read is made.
///
/// The two ends come from two different clocks, because the values they bound do. The batch's
/// events are stamped from [`base_ms`], an hour behind the ingestion clock, while a delivery's
/// datapoint carries the instant the delivery was PUBLISHED — the service's own clock, now —
/// and CloudWatch takes no datapoint whose stored span begins at or after the read's
/// `EndTime`. An `EndTime` of `at_ms` plus an hour is exactly the moment the case STARTED, so
/// whether a later delivery's datapoint is in range would depend on which side of a span
/// boundary the wall clock had reached by the time the case got to it — a case that passes on
/// an idle machine and fails on a loaded one. The end is taken from the clock the datapoints
/// are stamped by instead, with an hour of room past it, so nothing here turns on how fast the
/// host runs.
async fn datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Vec<Value> {
    let at_secs = at_ms as f64 / 1_000.0;
    let until_secs = now_ms() as f64 / 1_000.0 + 3_600.0;
    let stats = emulator
        .cloudwatch(
            case,
            "GetMetricStatistics",
            &json!({
                "Namespace": LOGS_NAMESPACE,
                "MetricName": metric,
                "Dimensions": dimensions,
                "StartTime": at_secs - 3_600.0,
                "EndTime": until_secs,
                "Period": 7_200,
                "Statistics": ["Sum"],
            }),
        )
        .await;
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The `Sum` of one series over that window, `None` when it holds no datapoint, asserting
/// that every datapoint it does hold carries [`UNIT_NONE`].
pub(crate) async fn sum(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
    at_ms: i64,
) -> Option<f64> {
    let points = datapoints(emulator, case, metric, dimensions, at_ms).await;
    if points.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for point in &points {
        assert_eq!(
            point["Unit"].as_str(),
            Some(UNIT_NONE),
            "case {case}: every {metric} datapoint carries Unit {UNIT_NONE}, got {point}"
        );
        total += point["Sum"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: a {metric} datapoint carries a numeric Sum, got {point}")
        });
    }
    Some(total)
}

/// The log group and the log stream one row writes into.
async fn given_log_stream(emulator: &Emulator, case: &str, group: &str) {
    emulator
        .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
}

/// One batch, every event stamped inside the ingestion window.
async fn write(emulator: &Emulator, case: &str, group: &str, at: i64, messages: &[&str]) {
    let events: Vec<Value> = messages
        .iter()
        .enumerate()
        .map(|(nth, message)| json!({"timestamp": at + nth as i64, "message": message}))
        .collect();
    emulator
        .logs(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": LOG_STREAM,
                "logEvents": events,
            }),
        )
        .await;
}

/// `ForwardedLogEvents` is discoverable under exactly the filter's three dimensions and sums
/// to the events the PATTERN selected, never to the events the batch wrote.
#[tokio::test]
async fn forwarded_log_events_counts_only_the_events_the_pattern_selected() {
    let emulator = Emulator::new();
    let stream = "forwarded-destination";
    emulator.create_stream("the destination stream", stream).await;
    tokio::time::sleep(past_activation()).await;
    let at = base_ms();

    let rows = rows();
    for row in &rows {
        let case = row.case;
        given_log_stream(&emulator, case, row.group).await;
        let destination = if row.through_logical_destination {
            emulator
                .logical_destination(case, "forwarded-logical-destination", stream)
                .await
        } else {
            stream_arn(stream)
        };
        let mut filter = json!({
            "logGroupName": row.group,
            "filterName": row.filter,
            "filterPattern": row.pattern,
            "destinationArn": destination,
        });
        if !row.through_logical_destination {
            // A logical destination carries the role it delivers through; a stream named
            // directly needs the filter to name one.
            filter["roleArn"] = json!(role_arn());
        }
        emulator
            .logs(case, "PutSubscriptionFilter", &filter)
            .await;
        write(&emulator, case, row.group, at, row.messages).await;
    }

    for row in &rows {
        let case = row.case;
        let series = listed_series(&emulator, case, FORWARDED_LOG_EVENTS, row.group).await;
        let Some(forwarded) = row.forwarded else {
            assert!(
                series.is_empty(),
                "case {case}: a filter that forwarded nothing publishes no series, got \
                 {series:?}"
            );
            continue;
        };
        let expected_names: BTreeSet<String> = [LOG_GROUP_NAME, DESTINATION_TYPE, FILTER_NAME]
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        assert_eq!(
            series,
            vec![(expected_names, row.filter.to_string())],
            "case {case}: {FORWARDED_LOG_EVENTS} is discoverable under exactly the log group, \
             destination type and filter name, got {series:?}"
        );
        // The series is read on its OWN dimension values, which is how a client reads it off
        // the triple ListMetrics reported.
        let destination_type = destination_type_of(&emulator, case, row.group).await;
        let reported = sum(
            &emulator,
            case,
            FORWARDED_LOG_EVENTS,
            &dimensions(row.group, row.filter, &destination_type),
            at,
        )
        .await;
        assert_eq!(
            reported,
            Some(forwarded),
            "case {case}: the forwarded count is the events the pattern selected out of {} \
             written, got {reported:?}",
            row.messages.len()
        );
    }
}

/// The `DestinationType` dimension value of the one series the log group `group` published,
/// read off `ListMetrics` rather than assumed: the test asserts the dimension NAMES and the
/// filter name, and takes the kind from what the service reported.
pub(crate) async fn destination_type_of(emulator: &Emulator, case: &str, group: &str) -> String {
    let listed = emulator
        .cloudwatch(
            case,
            "ListMetrics",
            &json!({"Namespace": LOGS_NAMESPACE, "MetricName": FORWARDED_LOG_EVENTS}),
        )
        .await;
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|series| {
            series["Dimensions"]
                .as_array()
                .is_some_and(|reported| {
                    reported.iter().any(|dimension| {
                        dimension["Name"].as_str() == Some(LOG_GROUP_NAME)
                            && dimension["Value"].as_str() == Some(group)
                    })
                })
        })
        .filter_map(|series| {
            series["Dimensions"].as_array()?.iter().find_map(|dimension| {
                (dimension["Name"].as_str() == Some(DESTINATION_TYPE))
                    .then(|| dimension["Value"].as_str().map(str::to_string))?
            })
        })
        .next()
        .unwrap_or_else(|| panic!("case {case}: {group} published a destination type"))
}

/// A destination that would not take the envelope counts a `DeliveryErrors` datapoint under
/// the same dimension values, leaves the event readable in the log group, and is not
/// attempted again until the identical `PutSubscriptionFilter` re-validates it — and the
/// event written during the outage is never re-delivered.
#[tokio::test]
async fn a_refusing_destination_counts_a_delivery_error_until_the_filter_is_re_put() {
    let case = "the destination deleted under a working filter";
    let emulator = Emulator::new();
    let stream = "failing-destination";
    let group = "/aws/emulator/delivery-errors";
    let filter = "error-filter";
    emulator.create_stream(case, stream).await;
    tokio::time::sleep(past_activation()).await;
    given_log_stream(&emulator, case, group).await;

    let put = json!({
        "logGroupName": group,
        "filterName": filter,
        "filterPattern": "SIM2REALMATCH",
        "destinationArn": stream_arn(stream),
        "roleArn": role_arn(),
    });
    emulator.logs(case, "PutSubscriptionFilter", &put).await;

    // The positive control: one matching event is delivered, and that delivery is what the
    // dimension values are read off.
    let at = base_ms();
    write(&emulator, case, group, at, &["SIM2REALMATCH control"]).await;
    let destination_type = destination_type_of(&emulator, case, group).await;
    let series = dimensions(group, filter, &destination_type);
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &series, at).await,
        Some(1.0),
        "case {case}: the control event is forwarded"
    );
    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &series, at).await,
        None,
        "case {case}: a delivery that succeeded counts no delivery error"
    );
    let control = emulator.delivered(case, stream).await;
    assert_eq!(
        control.len(),
        1,
        "case {case}: the control event reached the destination, got {control:?}"
    );

    // The destination goes away, and one more matching event is written.
    emulator
        .kinesis(
            case,
            "DeleteStream",
            &json!({"StreamName": stream, "EnforceConsumerDeletion": true}),
        )
        .await;
    let outage = "SIM2REALMATCH during the outage";
    write(&emulator, case, group, at + 10, &[outage]).await;

    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &series, at).await,
        Some(1.0),
        "case {case}: the destination refusing the envelope counts one {DELIVERY_ERRORS}"
    );
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &series, at).await,
        Some(1.0),
        "case {case}: an envelope the destination refused forwarded nothing, so the count \
         stands at the control's one event"
    );

    // The event is not lost: the log group still holds it.
    let read = emulator
        .logs(
            case,
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
    let stored: Vec<String> = read["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["message"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        stored.contains(&outage.to_string()),
        "case {case}: the event written during the outage stays readable, got {stored:?}"
    );

    // The destination comes back under the same name, so it presents the identical ARN. The
    // filter does not deliver again on its own: a further event counts no new delivery error
    // either, because nothing is attempted.
    tokio::time::sleep(past_deletion()).await;
    emulator.create_stream(case, stream).await;
    tokio::time::sleep(past_activation()).await;
    let while_unhealthy = "SIM2REALMATCH before the re-put";
    write(&emulator, case, group, at + 20, &[while_unhealthy]).await;
    assert!(
        emulator.delivered(case, stream).await.is_empty(),
        "case {case}: a destination marked unhealthy is not attempted until the next put"
    );
    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &series, at).await,
        Some(1.0),
        "case {case}: a delivery that is not attempted counts no further {DELIVERY_ERRORS}"
    );

    // The identical put re-validates the destination: delivery resumes from the NEXT batch,
    // and what was written while the destination was gone is never re-delivered.
    emulator.logs(case, "PutSubscriptionFilter", &put).await;
    let after = "SIM2REALMATCH after the re-put";
    write(&emulator, case, group, at + 30, &[after]).await;
    let resumed: Vec<String> = emulator
        .delivered(case, stream)
        .await
        .iter()
        .flat_map(|(_key, envelope)| {
            envelope["logEvents"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|event| event["message"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect();
    assert_eq!(
        resumed,
        vec![after.to_string()],
        "case {case}: the re-validated filter delivers what was written after the put and \
         replays nothing, got {resumed:?}"
    );
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &series, at).await,
        Some(2.0),
        "case {case}: the resumed delivery forwards one more event"
    );
}
