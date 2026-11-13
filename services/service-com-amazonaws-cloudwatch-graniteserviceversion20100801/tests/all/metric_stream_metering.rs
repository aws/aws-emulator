// SPDX-License-Identifier: CC0-1.0

//! What a running metric stream publishes ABOUT ITSELF while it exports, read back where a
//! client reads it: the stream's own `MetricUpdate` / `TotalMetricUpdate` series in the
//! `AWS/CloudWatch/MetricStreams` namespace, under the dimension naming the stream.
//!
//! The rule under test: a running metric stream meters its own export — for every metric update
//! it delivers it counts one `MetricUpdate`, and in `TotalMetricUpdate` it counts that update
//! once for itself (the four statistics "a metric stream always sends … `MAX`, `MIN`, `SUM`, and
//! `SAMPLECOUNT`", `PutMetricStream`) plus once more for each additional statistic a
//! `StatisticsConfigurations` entry adds for the metric the update is about ("You can use the
//! `StatisticsConfigurations` parameter to have the metric stream send additional statistics in
//! the stream", `PutMetricStream`) — both in the `AWS/CloudWatch/MetricStreams` namespace under
//! the `MetricStreamName` dimension, so a stream that configures no additional statistic reports
//! the two series equal, one that configures `n` of them reports a total of `1 + n` per update,
//! and a stream that exports nothing meters nothing.
//!
//! The pipeline (CloudWatch, Amazon Data Firehose and Amazon S3 on one engine) and the clock-free
//! way a case reaches a past period are the ones `metric_stream_delivery` stands up; what this
//! suite adds is the counters, read through CloudWatch's own `GetMetricStatistics`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::metric_stream_delivery::{
    period_of, stamp, Emulator, ACCOUNT, PERIOD_SECS, POLL_INTERVAL, REGION, ROLE_ARN,
};

/// The namespace a metric stream's own metrics live in, and the two series this suite reads.
const METRIC_STREAMS_NAMESPACE: &str = "AWS/CloudWatch/MetricStreams";
const METRIC_UPDATE: &str = "MetricUpdate";
const TOTAL_METRIC_UPDATE: &str = "TotalMetricUpdate";

/// The dimension naming which stream a metering series is about.
const STREAM_NAME_DIMENSION: &str = "MetricStreamName";

/// The status a delivery stream takes records in (`DeliveryStreamStatus`).
const ACTIVE: &str = "ACTIVE";

/// How long a created delivery stream is waited for before a case fails: far longer than that
/// service's own clock-owed `CREATING` -> `ACTIVE` span, so a loaded machine does not fail a case
/// the clock would have served. What is waited for is the STATUS, polled.
const ACTIVATION_DEADLINE: Duration = Duration::from_secs(120);

/// How long the counters are waited for before a case fails — far longer than an export needs,
/// for the same reason: what is waited for is the ARRIVAL of the datapoints, polled.
const METERING_DEADLINE: Duration = Duration::from_secs(120);

/// How far back and ahead a metering read looks, in periods: enough to hold every count a case
/// makes whichever minute the case straddles, and far inside `GetMetricStatistics`' own
/// datapoint cap.
const READ_PERIODS_BACK: f64 = 10.0;
const READ_PERIODS_AHEAD: f64 = 2.0;

/// The destination bucket and the delivery stream writing into it, waited past the `CREATING`
/// status, which is when it takes records.
async fn given_destination(emulator: &Emulator, bucket: &str, delivery_stream: &str) {
    emulator.create_bucket(bucket).await;
    emulator
        .firehose(
            "CreateDeliveryStream",
            &json!({
                "DeliveryStreamName": delivery_stream,
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": format!("arn:aws:s3:::{bucket}"),
                    // The shortest wait `IntervalInSeconds` admits, so a case does not wait out a
                    // buffering interval it is not about.
                    "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                },
            }),
        )
        .await;
    let deadline = Instant::now() + ACTIVATION_DEADLINE;
    loop {
        let described = emulator
            .firehose(
                "DescribeDeliveryStream",
                &json!({"DeliveryStreamName": delivery_stream}),
            )
            .await;
        let status = described
            .get("DeliveryStreamDescription")
            .and_then(|description| description.get("DeliveryStreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status == ACTIVE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "delivery stream {delivery_stream} must reach {ACTIVE} before it takes records, last \
             status {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A `PutMetricStream` request for `name`, streaming only `namespace` and asking for
/// `statistics` on `metric_name` where a row names any.
fn metric_stream_request(
    name: &str,
    delivery_stream: &str,
    namespace: &str,
    metric_name: &str,
    statistics: Option<&[&str]>,
) -> Value {
    let mut request = json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/{delivery_stream}"),
        "RoleArn": ROLE_ARN,
        "OutputFormat": "json",
        "IncludeFilters": [{"Namespace": namespace}],
    });
    if let Some(statistics) = statistics {
        request["StatisticsConfigurations"] = json!([{
            "IncludeMetrics": [{"Namespace": namespace, "MetricName": metric_name}],
            "AdditionalStatistics": statistics,
        }]);
    }
    request
}

/// Publish one datapoint of `metric_name` in each of `periods_back`, in ONE call — so the batch
/// owes one exported record per period, and the stream is metered for all of them.
async fn publish_periods(
    emulator: &Emulator,
    namespace: &str,
    metric_name: &str,
    periods_back: &[i64],
    as_values: bool,
) {
    let data: Vec<Value> = periods_back
        .iter()
        .map(|back| {
            let mut datum = json!({
                "MetricName": metric_name,
                "Unit": "Count",
                // The MIDDLE of the period, so a row's placement does not depend on where in a
                // period the suite happens to run.
                "Timestamp": stamp(*back),
            });
            if as_values {
                // Several published values, so a percentile is computed over a distribution
                // rather than over one observation.
                datum["Values"] = json!([1.0, 2.0, 3.0, 4.0]);
            } else {
                datum["Value"] = json!(7.0);
            }
            datum
        })
        .collect();
    emulator
        .cloudwatch(
            "PutMetricData",
            &json!({"Namespace": namespace, "MetricData": data}),
        )
        .await;
}

/// The `Sum` of every datapoint one metering series holds in the window around now, read through
/// `GetMetricStatistics` exactly as a client reads it, and how many datapoints carried it.
async fn metered(emulator: &Emulator, metric_name: &str, dimensions: Value) -> (f64, usize) {
    let answer = emulator
        .cloudwatch(
            "GetMetricStatistics",
            &json!({
                "Namespace": METRIC_STREAMS_NAMESPACE,
                "MetricName": metric_name,
                "Dimensions": dimensions,
                "StartTime": period_of(0) - READ_PERIODS_BACK * PERIOD_SECS,
                "EndTime": period_of(0) + READ_PERIODS_AHEAD * PERIOD_SECS,
                "Period": PERIOD_SECS as i64,
                "Statistics": ["Sum"],
            }),
        )
        .await;
    let datapoints = answer
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let sum = datapoints
        .iter()
        .filter_map(|datapoint| datapoint.get("Sum").and_then(Value::as_f64))
        .sum();
    (sum, datapoints.len())
}

/// The dimension list identifying the metering series of `stream`.
fn stream_dimension(stream: &str) -> Value {
    json!([{"Name": STREAM_NAME_DIMENSION, "Value": stream}])
}

/// Wait until `stream` has been metered for `updates` metric updates, then answer what both
/// counters hold — or fail naming `case` once [`METERING_DEADLINE`] has passed.
async fn metering_of(emulator: &Emulator, case: &str, stream: &str, updates: f64) -> (f64, f64) {
    let deadline = Instant::now() + METERING_DEADLINE;
    loop {
        let (counted, _) = metered(emulator, METRIC_UPDATE, stream_dimension(stream)).await;
        let (total, _) = metered(emulator, TOTAL_METRIC_UPDATE, stream_dimension(stream)).await;
        if counted >= updates {
            return (counted, total);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stream} to be metered for {updates} metric update(s), got \
             {METRIC_UPDATE}={counted} and {TOTAL_METRIC_UPDATE}={total}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One metering case: how the stream was configured and what was published into it.
struct MeteringRow {
    case: &'static str,
    /// The additional statistics the stream's configuration asks for, where it has one.
    statistics: Option<&'static [&'static str]>,
    /// The periods a datapoint is published into, each owing one exported record.
    periods_back: &'static [i64],
    /// Whether the datapoints carry a distribution (`Values`) or one observation (`Value`).
    as_values: bool,
    /// What the two counters must hold once the export has settled.
    updates: f64,
    statistic_updates: f64,
}

/// The metering cases.
///
/// The rows differ along what the rule generalises over: how many additional statistics the
/// configuration asks for (none, one, the six a client enabling percentiles and trimmed means
/// asks for), how many updates one publish owes (one period, three periods), and the form the
/// datapoints were published in (one observation, a distribution) — so the total is asserted as
/// `updates * (1 + configured statistics)` across the class rather than for one shape of input.
fn metering_rows() -> Vec<MeteringRow> {
    vec![
        MeteringRow {
            case: "no statistics configuration at all",
            statistics: None,
            periods_back: &[1],
            as_values: false,
            updates: 1.0,
            statistic_updates: 1.0,
        },
        MeteringRow {
            case: "one additional statistic over three updates",
            statistics: Some(&["p99"]),
            periods_back: &[1, 2, 3],
            as_values: true,
            updates: 3.0,
            statistic_updates: 6.0,
        },
        MeteringRow {
            // The statistics a client enabling percentiles and the bounded families asks for:
            // percentiles (`MetricStreamStatisticsConfiguration$AdditionalStatistics`: "p??
            // percentile statistics such as p90, p99") beside the abbreviations the json format
            // also admits ("this includes tm98, wm90").
            case: "six additional statistics over two updates",
            statistics: Some(&["p10", "p50", "p90", "p99", "tm90", "wm90"]),
            periods_back: &[1, 2],
            as_values: true,
            updates: 2.0,
            statistic_updates: 14.0,
        },
        MeteringRow {
            case: "six additional statistics over one update published as one observation",
            statistics: Some(&["p10", "p50", "p90", "p99", "tm90", "wm90"]),
            periods_back: &[2],
            as_values: false,
            updates: 1.0,
            statistic_updates: 7.0,
        },
        MeteringRow {
            case: "two additional statistics over four updates",
            statistics: Some(&["p50", "IQM"]),
            periods_back: &[1, 2, 3, 4],
            as_values: true,
            updates: 4.0,
            statistic_updates: 12.0,
        },
    ]
}

/// RULE: a running stream counts one `MetricUpdate` per exported update and, in
/// `TotalMetricUpdate`, that update plus each additional statistic its configuration adds.
#[tokio::test]
async fn the_total_counts_each_configured_statistic_as_one_more_update() {
    for row in metering_rows() {
        let emulator = Emulator::new();
        given_destination(&emulator, "metering-sink", "metering-stream").await;
        emulator
            .cloudwatch(
                "PutMetricStream",
                &metric_stream_request(
                    "metered",
                    "metering-stream",
                    "Custom/Metering",
                    "Streamed",
                    row.statistics,
                ),
            )
            .await;
        publish_periods(
            &emulator,
            "Custom/Metering",
            "Streamed",
            row.periods_back,
            row.as_values,
        )
        .await;

        let (counted, total) = metering_of(&emulator, row.case, "metered", row.updates).await;
        assert_eq!(
            (counted, total),
            (row.updates, row.statistic_updates),
            "case {:?}: expected {METRIC_UPDATE}={} and {TOTAL_METRIC_UPDATE}={}, got {counted} \
             and {total}",
            row.case,
            row.updates,
            row.statistic_updates
        );
        // The lift is exactly what the configuration asks for: equal counters without one, and
        // a strictly greater total with one.
        match row.statistics {
            None => assert!(
                (total - counted).abs() < f64::EPSILON,
                "case {:?}: a stream configuring no additional statistic must report the two \
                 counters equal, got {counted} and {total}",
                row.case
            ),
            Some(statistics) => assert!(
                total > counted && total <= counted * (1.0 + statistics.len() as f64),
                "case {:?}: expected {TOTAL_METRIC_UPDATE} above {METRIC_UPDATE} and at most \
                 {METRIC_UPDATE} * {}, got {counted} and {total}",
                row.case,
                1 + statistics.len(),
            ),
        }
    }
}

/// RULE: the counters are a stream's OWN series, identified by the dimension naming it — so one
/// stream's counts are not another's, and the series is not readable without that dimension.
#[tokio::test]
async fn the_counters_are_identified_by_the_stream_they_are_about() {
    let emulator = Emulator::new();
    given_destination(&emulator, "named-sink", "named-stream").await;
    for (stream, statistics) in [
        ("named-plain", None),
        ("named-configured", Some(&["p90", "p99"][..])),
    ] {
        emulator
            .cloudwatch(
                "PutMetricStream",
                &metric_stream_request(
                    stream,
                    "named-stream",
                    "Custom/Named",
                    "Streamed",
                    statistics,
                ),
            )
            .await;
    }
    publish_periods(&emulator, "Custom/Named", "Streamed", &[1, 2], true).await;

    for (stream, statistic_updates) in [("named-plain", 2.0), ("named-configured", 6.0)] {
        let (counted, total) = metering_of(&emulator, stream, stream, 2.0).await;
        assert_eq!(
            (counted, total),
            (2.0, statistic_updates),
            "case {stream:?}: each stream must be metered for its own export, got \
             {METRIC_UPDATE}={counted} and {TOTAL_METRIC_UPDATE}={total}"
        );
    }

    // The dimension is part of the series' identity: "If a specific combination of dimensions
    // was not published, you can't retrieve statistics for it"
    // (`GetMetricStatisticsInput$Dimensions`), so the same metric name with no dimension at all
    // names a series nothing published into.
    for metric_name in [METRIC_UPDATE, TOTAL_METRIC_UPDATE] {
        let (sum, datapoints) = metered(&emulator, metric_name, json!([])).await;
        assert_eq!(
            (sum, datapoints),
            (0.0, 0),
            "case {metric_name:?}: the counters must be readable only under the dimension naming \
             the stream, got a sum of {sum} over {datapoints} datapoint(s)"
        );
    }
}

/// RULE: a stopped stream exports nothing, so it meters nothing — while its running twin on the
/// same destination keeps counting.
#[tokio::test]
async fn a_stopped_stream_meters_nothing() {
    let emulator = Emulator::new();
    given_destination(&emulator, "twin-sink", "twin-stream").await;
    for stream in ["twin-running", "twin-stopped"] {
        emulator
            .cloudwatch(
                "PutMetricStream",
                &metric_stream_request(
                    stream,
                    "twin-stream",
                    "Custom/Twin",
                    "Streamed",
                    Some(&["p99"]),
                ),
            )
            .await;
    }
    emulator
        .cloudwatch("StopMetricStreams", &json!({"Names": ["twin-stopped"]}))
        .await;
    publish_periods(&emulator, "Custom/Twin", "Streamed", &[1, 2], true).await;

    // The running twin's counters are what says the publish's exports have settled: both streams
    // are driven by the same publish, so once the running one is metered the stopped one has had
    // its chance too.
    let (counted, total) = metering_of(&emulator, "the running twin", "twin-running", 2.0).await;
    assert_eq!(
        (counted, total),
        (2.0, 4.0),
        "the running twin must be metered for both updates, got {METRIC_UPDATE}={counted} and \
         {TOTAL_METRIC_UPDATE}={total}"
    );
    for metric_name in [METRIC_UPDATE, TOTAL_METRIC_UPDATE] {
        let (sum, datapoints) =
            metered(&emulator, metric_name, stream_dimension("twin-stopped")).await;
        assert_eq!(
            (sum, datapoints),
            (0.0, 0),
            "case {metric_name:?}: a stopped stream must meter nothing, got a sum of {sum} over \
             {datapoints} datapoint(s)"
        );
    }

    // Started again, it meters what it exports from then on — so the silence above is the state,
    // not a stream that can never be metered.
    emulator
        .cloudwatch("StartMetricStreams", &json!({"Names": ["twin-stopped"]}))
        .await;
    publish_periods(&emulator, "Custom/Twin", "Streamed", &[3], true).await;
    let (restarted, restarted_total) =
        metering_of(&emulator, "the started twin", "twin-stopped", 1.0).await;
    assert_eq!(
        (restarted, restarted_total),
        (1.0, 2.0),
        "a started stream must meter the updates it exports from then on, got \
         {METRIC_UPDATE}={restarted} and {TOTAL_METRIC_UPDATE}={restarted_total}"
    );
}
