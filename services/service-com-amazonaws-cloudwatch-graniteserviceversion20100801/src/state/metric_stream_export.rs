// SPDX-License-Identifier: CC0-1.0

//! The EXPORT a running metric stream makes of the metric updates it streams.
//!
//! "Creates or updates a metric stream. Metric streams can automatically stream CloudWatch
//! metrics to Amazon Web Services destinations" … "When you use `PutMetricStream` to create a
//! new metric stream, the stream is created in the `running` state." (`PutMetricStream`) — so
//! the configuration a stream is written with is not the whole resource: a stream in that state
//! EXPORTS, without any further call, every metric update its filters select into the Amazon
//! Data Firehose delivery stream its `FirehoseArn` names. That export is this module.
//!
//! What one exported record is:
//!
//! * WHICH metrics — the stream's filters, as the operation states the three choices: "Stream
//!   metrics from all metric namespaces in the account", "… except for the namespaces that you
//!   list in `ExcludeFilters`", "… only the metric namespaces that you list in
//!   `IncludeFilters`", narrowed per namespace by "If you omit this parameter, all metrics in
//!   the namespace are included or excluded" (`MetricStreamFilter$MetricNames`)
//!   ([`streams_metric`]).
//! * WHICH statistics — "By default, a metric stream always sends the `MAX`, `MIN`, `SUM`, and
//!   `SAMPLECOUNT` statistics for each metric that is streamed. You can use the
//!   `StatisticsConfigurations` parameter to have the metric stream send additional statistics
//!   in the stream." (`PutMetricStream`), each additional statistic computed over the same
//!   distribution every other read of this store computes a statistic over
//!   ([`super::statistic_value`]).
//! * OVER WHICH SPAN — one record per metric per [`EXPORT_PERIOD_SECS`] period, the
//!   granularity a regular-resolution metric is aggregated at ("a period can be as short as one
//!   minute (60 seconds) and must be a multiple of 60", `MetricStat$Period`), stamped with the
//!   instant that period opens at.
//! * IN WHICH FORM — the `OutputFormat` the stream carries ([`JSON_FORMAT`]).
//!
//! The export happens where the update does, whichever path RECORDS the update: `PutMetricData`
//! stores a caller's datapoints, and the service's own API metering stores the call count it
//! publishes about itself ([`super::usage`]) — "Stream metrics from all metric namespaces in the
//! account" is every namespace the account holds metrics in, the reserved ones included — so the
//! records either write makes due are pushed from that call ([`export_published`]) and a consumer
//! downstream of the delivery stream sees them without anybody calling CloudWatch again. The
//! datapoints a record reports are only READ here — a record is a reading of what the store
//! already holds — and what the export WRITES is its own metering: the stream's own series
//! counting the updates it streamed ([`metering`]).
//!
//! The delivery stream is a resource of another service, so a record reaches it the way any
//! emulated service reaches another: an AWS SDK client over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), with `PutRecord` per record in
//! chronological order, so the records of one metric arrive in the order they were published.
//! The call is the ROLE's to make — "The ARN of an IAM role that this metric stream will use to
//! access Amazon Kinesis Data Firehose resources. This IAM role must already exist and must be
//! in the same account as the metric stream. This IAM role must include the following
//! permissions: firehose:PutRecord firehose:PutRecordBatch" (`PutMetricStreamInput$RoleArn`) —
//! so a role IAM answers withholds that write exports nothing.
//!
//! A stream configured for one of the OpenTelemetry output formats exports the same metric
//! updates, selected by the same filters and summarised over the same periods, in the records
//! that format asks for: [`open_telemetry`] renders those, and this module pushes them the same
//! way.

use ::serde_json::{Map, Value};

use crate::ops::types::{GetMetricStreamOutput, MetricStreamFilter};

/// What a stream publishes about its own export: the series counting the updates it streamed.
pub mod metering;

/// The records of a stream whose `OutputFormat` is one of the OpenTelemetry values.
pub mod open_telemetry;

/// The span one exported record covers, and so the bucket a published datapoint is exported
/// in: one minute, the granularity a regular-resolution metric is aggregated at ("For metrics
/// with regular resolution, a period can be as short as one minute (60 seconds) and must be a
/// multiple of 60", `MetricStat$Period`). A record is stamped with the instant its period opens
/// at, so two datapoints of one metric in one period are one record and two datapoints in
/// different periods are two.
const EXPORT_PERIOD_SECS: f64 = 60.0;

/// The `OutputFormat` whose records this module renders itself: "Valid values are `json`,
/// `opentelemetry1.0`, and `opentelemetry0.7`." (`PutMetricStreamInput$OutputFormat`) — the
/// other two are rendered by [`open_telemetry`].
const JSON_FORMAT: &str = "json";

/// The FORM a stream's records take, which is what its `OutputFormat` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordFormat {
    /// One JSON document per record ([`json_record`]).
    Json,
    /// One length-delimited OpenTelemetry message per record, of the release the format names
    /// ([`open_telemetry`]).
    OpenTelemetry(open_telemetry::OpenTelemetryVersion),
}

impl RecordFormat {
    /// The form an `OutputFormat` value asks for, or `None` for a stream carrying no format at
    /// all — the member is required, so a stored stream has one, and a value outside the set the
    /// model declares never reaches the store.
    fn named(format: Option<&str>) -> Option<Self> {
        match format? {
            JSON_FORMAT => Some(Self::Json),
            other => open_telemetry::OpenTelemetryVersion::named(other).map(Self::OpenTelemetry),
        }
    }
}

/// What ONE stream's records are rendered from: the stream's identity, where its metrics were
/// published, and the form its records take.
#[derive(Debug, Clone)]
struct StreamIdentity {
    /// The stream's name, which every record names as the stream it came from.
    name: String,
    /// The stream's own ARN, which an OpenTelemetry record names as the exporter.
    arn: String,
    /// The Region the metrics were published in.
    region: String,
    /// The form the records take.
    format: RecordFormat,
}

/// The ARN namespace of Amazon Data Firehose, whose delivery streams are what a metric stream
/// exports into.
const FIREHOSE_ARN_NAMESPACE: &str = "firehose";

/// The resource type a Firehose delivery stream's ARN carries
/// (`arn:aws:firehose:<region>:<account>:deliverystream/<name>`).
const DELIVERY_STREAM_RESOURCE_TYPE: &str = "deliverystream";

/// The write a metric stream's role has to be allowed to make on the delivery stream, as
/// `PutMetricStreamInput$RoleArn` names it: "This IAM role must include the following
/// permissions: firehose:PutRecord firehose:PutRecordBatch".
const PUT_RECORD_ACTION: &str = "firehose:PutRecord";

/// How many records ONE request exports.
///
/// The request's OWN bound, not a limit AWS publishes: a batch names at most 1000 metrics
/// (`PutMetricData`) and every factor below that is bounded too, but their PRODUCT with the
/// number of running streams is not, and each record is an in-process `PutRecord`. It sits far
/// above the records any realistic publish owes (a handful of streams over the metrics of one
/// batch); what it does not reach is not exported, exactly as a metric update AWS drops when a
/// stream cannot keep up is not exported.
pub const MAX_RECORDS_PER_REQUEST: usize = 500;

/// The members of the `value` object a json record carries, as the statistics a stream
/// always sends: "a metric stream always sends the `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT`
/// statistics for each metric that is streamed" (`PutMetricStream`), each under the name the
/// json record format spells it with.
const MAX_MEMBER: &str = "max";
const MIN_MEMBER: &str = "min";
const SUM_MEMBER: &str = "sum";
const COUNT_MEMBER: &str = "count";

/// One series a request published datapoints into, and the periods those datapoints fall in.
///
/// What a publish owes an export is the (series, period) pairs it touched: the record for such a
/// pair is computed from everything the store holds for that period, so this carries the pairs
/// and not the values.
#[derive(Debug, Clone)]
pub struct PublishedSeries {
    /// The series the datapoints were published into.
    pub key: super::MetricKey,
    /// The instants the periods they fall in open at, ascending and without repetition.
    pub period_starts: Vec<f64>,
}

/// The (series, period) pairs `ingest` publishes into, as [`export_published`] takes them.
///
/// Built from the batch a `PutMetricData` call is about to store, so an export is owed for
/// exactly the periods that call wrote into — a datum carrying no observation at all creates the
/// series without publishing an update, and owes no record.
#[must_use]
pub fn published_series(
    ingest: &[(super::MetricKey, Vec<super::StoredDatum>)],
) -> Vec<PublishedSeries> {
    let mut published: Vec<PublishedSeries> = Vec::new();
    for (key, observations) in ingest {
        if observations.is_empty() {
            continue;
        }
        let starts = observations
            .iter()
            .map(|datum| period_start(datum.timestamp_secs));
        match published.iter_mut().find(|series| series.key == *key) {
            Some(series) => series.period_starts.extend(starts),
            None => published.push(PublishedSeries {
                key: key.clone(),
                period_starts: starts.collect(),
            }),
        }
    }
    for series in &mut published {
        series.period_starts.sort_unstable_by(f64::total_cmp);
        series.period_starts.dedup();
    }
    published
}

/// The (series, period) pair ONE datapoint stored at `timestamp_secs` publishes into, as
/// [`export_published`] takes them.
///
/// The same derivation [`published_series`] reads a batch with, for a caller that stored a
/// single observation and holds no batch to hand over — the service's own API metering
/// ([`super::usage`]) — so one observation owes exactly the record a publish of the same
/// datapoint would.
#[must_use]
pub fn published_at(key: super::MetricKey, timestamp_secs: f64) -> Vec<PublishedSeries> {
    vec![PublishedSeries {
        key,
        period_starts: vec![period_start(timestamp_secs)],
    }]
}

/// The instant the [`EXPORT_PERIOD_SECS`] period holding `timestamp_secs` opens at.
fn period_start(timestamp_secs: f64) -> f64 {
    (timestamp_secs / EXPORT_PERIOD_SECS).floor() * EXPORT_PERIOD_SECS
}

/// Export the records the datapoints a request just stored owe, to every running stream of
/// `region` whose filters select them.
///
/// The store is locked to MEASURE what the records report and released before any of them is
/// rendered or delivered, so no document formatting happens in the critical section and one
/// publisher's fan-out cannot block another's read. The datapoints a record reports are only
/// read: a record is a reading of what the store already holds. What a delivery WRITES is the
/// stream's own metering, counting the updates that reached the destination
/// ([`metering::exported`]) — a stream that delivered nothing writes nothing.
pub async fn export_published<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    published: &[PublishedSeries],
) {
    if published.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        // No store to read. The handler that called this answers the store's absence with its
        // own modelled server fault; there is nothing to report from here.
        return;
    };
    let exports = {
        let store = state.lock();
        planned(&store, region, published)
    };
    for export in exports {
        let records = rendered(&export);
        if records.is_empty() {
            continue;
        }
        let delivered = deliver(ctx, &export, &records).await;
        // The stream's own counters report what it STREAMED, so they count the records the
        // destination took and are stamped with the instant it took them.
        let exported = metering::exported(&export.measured, delivered);
        let now = super::now_secs();
        state
            .lock()
            .meter_exported_updates(&export.stream.name, exported, now);
    }
}

/// What ONE stream is owed: where it delivers, under which role, how its records are rendered,
/// and what each record reports, in the order they are to arrive.
#[derive(Debug, Clone)]
struct StreamExport {
    /// The delivery stream the records are pushed into.
    firehose_arn: String,
    /// The role the push is made as, when the stream carries one.
    role_arn: Option<String>,
    /// The stream the records come from, and the form they take.
    stream: StreamIdentity,
    /// What each record reports, oldest period first.
    measured: Vec<MeasuredPeriod>,
}

/// ONE record's measurements, taken while the store is held: a record is rendered from these and
/// from nothing else, so the rendering needs no access to the store.
#[derive(Debug, Clone)]
struct MeasuredPeriod {
    /// The metric the record reports.
    key: super::MetricKey,
    /// The instant the period opens at, as epoch seconds.
    start: f64,
    /// The unit its datapoints were published under.
    unit: String,
    /// The statistics every streamed metric carries.
    statistics: super::BasicStatistics,
    /// The additional statistics the stream asks for, each as `(name, value)`, in the order the
    /// stream asks for them and without the ones this period does not define.
    additional: Vec<(String, f64)>,
}

/// One (period, unit) group of a published series: the datapoints one record reports.
struct PeriodGroup<'a> {
    /// The instant the period opens at.
    start: f64,
    /// The unit these datapoints were published under.
    unit: &'a str,
    /// The datapoints themselves.
    data: Vec<&'a super::StoredDatum>,
}

/// The exports every running stream of `region` is owed for `published`.
///
/// The record budget is applied BEFORE any measuring: a stream collects at most as many
/// (period, unit) groups as the budget still allows, so what the budget does not reach is never
/// measured and never rendered.
///
/// The datapoints are grouped ONCE per published series, before any stream is looked at: which
/// (period, unit) groups a series owes records for is a property of the series and of the
/// periods this request published into, and of no stream — so the scan over a series' stored
/// datapoints costs the request once however many streams select it, rather than once per
/// stream. That is what bounds the work of a call made while the store holds many running
/// streams: at most one pass per published series, each pass over at most that series' own
/// datapoint cap ([`super::MAX_DATAPOINTS_PER_SERIES`]), and at most
/// [`MAX_RECORDS_PER_REQUEST`] groups kept from it.
fn planned(
    store: &super::ComAmazonawsCloudwatchGraniteserviceversion20100801State,
    region: &str,
    published: &[PublishedSeries],
) -> Vec<StreamExport> {
    let mut exports: Vec<StreamExport> = Vec::new();
    let mut budget = MAX_RECORDS_PER_REQUEST;
    // One pass per published series, shared by every stream below.
    let grouped_series: Vec<(&PublishedSeries, Vec<PeriodGroup<'_>>)> = published
        .iter()
        .filter_map(|series| {
            let data = store.metrics.get(&series.key)?;
            Some((series, grouped(series, data, MAX_RECORDS_PER_REQUEST)))
        })
        .collect();
    for (key, stored) in &store.metric_streams {
        if key.region != region {
            continue;
        }
        if budget == 0 {
            break;
        }
        let stream = &stored.stream;
        // "The state of the metric stream. The possible values are `running` and `stopped`."
        // (`GetMetricStreamOutput$State`) — a stopped stream is one `StopMetricStreams` has
        // taken out of service, so it exports nothing until it is started again.
        if stream.state.as_deref() != Some(super::METRIC_STREAM_RUNNING) {
            continue;
        }
        let Some(format) = RecordFormat::named(stream.output_format.as_deref()) else {
            continue;
        };
        let Some(firehose_arn) = stream.firehose_arn.clone() else {
            continue;
        };
        let name = stream.name.clone().unwrap_or_else(|| key.name.clone());
        let arn = stream
            .arn
            .clone()
            .unwrap_or_else(|| super::metric_stream_arn(region, &name));
        // The groups this stream owes a record for, measured as they are collected; at most
        // `budget` of them, so the work is bounded by the budget rather than truncated after it.
        let mut measured: Vec<MeasuredPeriod> = Vec::new();
        for (series, groups) in &grouped_series {
            if measured.len() >= budget {
                break;
            }
            if !streams_metric(stream, &series.key) {
                continue;
            }
            let additional = additional_statistics(stream, &series.key);
            for group in groups.iter().take(budget - measured.len()) {
                measured.push(measured_period(&series.key, &additional, group));
            }
        }
        // Oldest period first across every metric of the batch, so the records of one metric
        // arrive in the order its datapoints were published.
        measured.sort_by(|left, right| left.start.total_cmp(&right.start));
        if measured.is_empty() {
            continue;
        }
        budget -= measured.len();
        exports.push(StreamExport {
            firehose_arn,
            role_arn: stream.role_arn.clone(),
            stream: StreamIdentity {
                name,
                arn,
                region: region.to_string(),
                format,
            },
            measured,
        });
    }
    exports
}

/// The (period, unit) groups one published series owes records for, at most `limit` of them.
///
/// ONE pass over the series' stored datapoints: each datum is placed in the period it falls in
/// and under the unit it was published under, and a datum of a period this request did not
/// publish into is skipped — so the scan costs the series' datapoints once, whatever the number
/// of periods the request spans. A record reports the measurement's unit, and two datapoints
/// published under different units are not one measurement, so the unit is part of the group.
fn grouped<'a>(
    series: &PublishedSeries,
    data: &'a [super::StoredDatum],
    limit: usize,
) -> Vec<PeriodGroup<'a>> {
    let mut groups: ::std::collections::BTreeMap<(i64, &'a str), Vec<&'a super::StoredDatum>> =
        ::std::collections::BTreeMap::new();
    for datum in data {
        let start = period_start(datum.timestamp_secs);
        if !start.is_finite() || !publishes(series, start) {
            continue;
        }
        // A period opens at a whole second, so the instant keys the group as the integer it is.
        let bucket = (start as i64, datum.unit());
        if let Some(group) = groups.get_mut(&bucket) {
            group.push(datum);
        } else if groups.len() < limit {
            groups.insert(bucket, vec![datum]);
        }
    }
    groups
        .into_iter()
        .map(|((bucket, unit), data)| PeriodGroup {
            start: bucket as f64,
            unit,
            data,
        })
        .collect()
}

/// Whether this request published into the period opening at `start`: the periods it touched are
/// held ascending, so the question is answered without scanning them.
fn publishes(series: &PublishedSeries, start: f64) -> bool {
    series
        .period_starts
        .binary_search_by(|held| held.total_cmp(&start))
        .is_ok()
}

/// What one group's record reports: the statistics a stream always sends, folded over the
/// group's datapoints, and the additional statistics it asks for.
///
/// An additional statistic the period does not define is left out: a percentile of a datapoint
/// whose individual values were never published, say, is a value this store cannot compute
/// rather than one it may invent.
fn measured_period(
    key: &super::MetricKey,
    additional: &[String],
    group: &PeriodGroup<'_>,
) -> MeasuredPeriod {
    MeasuredPeriod {
        key: key.clone(),
        start: group.start,
        unit: group.unit.to_string(),
        statistics: super::BasicStatistics::of(group.data.iter().copied()),
        additional: additional
            .iter()
            .filter_map(|statistic| {
                Some((
                    statistic.clone(),
                    super::statistic_value(&group.data, statistic)?,
                ))
            })
            .collect(),
    }
}

/// Whether `stream` streams the metric `key` names.
///
/// The three choices `PutMetricStream` documents: "Stream metrics from all metric namespaces in
/// the account", "Stream metrics from all metric namespaces in the account, except for the
/// namespaces that you list in `ExcludeFilters`", "Stream metrics from only the metric
/// namespaces that you list in `IncludeFilters`" — a stream carries at most one of the two
/// lists ("You cannot include ExcludeFilters and IncludeFilters in the same operation.",
/// `PutMetricStreamInput$ExcludeFilters`), so the lists are read in that order.
fn streams_metric(stream: &GetMetricStreamOutput, key: &super::MetricKey) -> bool {
    if let Some(include) = stream.include_filters.as_deref() {
        return include.iter().any(|filter| selects(filter, key));
    }
    if let Some(exclude) = stream.exclude_filters.as_deref() {
        return !exclude.iter().any(|filter| selects(filter, key));
    }
    true
}

/// Whether one filter names the metric `key` names: its namespace, and either the whole
/// namespace or a metric name it lists — "If you omit this parameter, all metrics in the
/// namespace are included or excluded" (`MetricStreamFilter$MetricNames`).
fn selects(filter: &MetricStreamFilter, key: &super::MetricKey) -> bool {
    filter.namespace.as_deref() == Some(key.namespace.as_str())
        && filter
            .metric_names
            .as_deref()
            .is_none_or(|names| names.contains(&key.metric_name))
}

/// The additional statistics `stream` sends for the metric `key` names: "For each entry in this
/// array, you specify one or more metrics and the list of additional statistics to stream for
/// those metrics." (`PutMetricStreamInput$StatisticsConfigurations`) — so the configurations
/// naming this metric contribute their statistics, and a metric no configuration names gets the
/// four every streamed metric gets and nothing else.
fn additional_statistics(stream: &GetMetricStreamOutput, key: &super::MetricKey) -> Vec<String> {
    let mut statistics: Vec<String> = Vec::new();
    for configuration in stream.statistics_configurations.iter().flatten() {
        let names_metric = configuration.include_metrics.iter().any(|metric| {
            metric.namespace == key.namespace && metric.metric_name == key.metric_name
        });
        if !names_metric {
            continue;
        }
        for statistic in &configuration.additional_statistics {
            if !statistics.iter().any(|held| held == statistic) {
                statistics.push(statistic.clone());
            }
        }
    }
    statistics
}

/// The records one planned export delivers, rendered once the store's lock is released: the
/// measurements are already taken, so no part of the formatting reads the store.
fn rendered(export: &StreamExport) -> Vec<Vec<u8>> {
    export
        .measured
        .iter()
        .map(|measured| record(&export.stream, measured))
        .collect()
}

/// ONE exported record, in the form the stream's `OutputFormat` asks for.
fn record(stream: &StreamIdentity, measured: &MeasuredPeriod) -> Vec<u8> {
    match stream.format {
        RecordFormat::Json => json_record(stream, measured).into_bytes(),
        RecordFormat::OpenTelemetry(version) => {
            open_telemetry::record(&open_telemetry::ExportedPeriod {
                version,
                exporter_arn: &stream.arn,
                region: &stream.region,
                key: &measured.key,
                start_secs: measured.start,
                end_secs: measured.start + EXPORT_PERIOD_SECS,
                unit: &measured.unit,
                statistics: measured.statistics,
                percentiles: &percentiles(&measured.additional),
            })
        }
    }
}

/// The additional statistics as the quantiles an OpenTelemetry record reports them at: the
/// percentile each `pNN` statistic names, in percent, and its value over the period's
/// distribution.
///
/// These formats carry percentiles only ("If the `OutputFormat` for the stream is
/// `opentelemetry1.0` or `opentelemetry0.7`, the only valid values are `p??` percentile
/// statistics such as `p90`, `p99` and so on.",
/// `MetricStreamStatisticsConfiguration$AdditionalStatistics`), so a statistic that names no
/// percentile has no quantile to be reported at.
fn percentiles(additional: &[(String, f64)]) -> Vec<(f64, f64)> {
    additional
        .iter()
        .filter_map(|(statistic, value)| Some((super::parse_percentile(statistic)?, *value)))
        .collect()
}

/// ONE exported record, in the json output format: which stream and account it came from, the
/// metric it reports, the period it covers and the statistics of that period.
fn json_record(stream: &StreamIdentity, measured: &MeasuredPeriod) -> String {
    let folded = measured.statistics;
    let mut value = Map::new();
    // The additional statistics first, so the four a stream always sends are the ones a record
    // reports under their own members whatever a statistics configuration asks for.
    for (statistic, computed) in &measured.additional {
        insert_number(&mut value, statistic, *computed);
    }
    for (member, statistic) in [
        (MAX_MEMBER, folded.maximum),
        (MIN_MEMBER, folded.minimum),
        (SUM_MEMBER, folded.sum),
        (COUNT_MEMBER, folded.sample_count),
    ] {
        insert_number(&mut value, member, statistic);
    }
    let dimensions: Map<String, Value> = measured
        .key
        .dimensions
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect();
    let mut document = Map::new();
    document.insert(
        "metric_stream_name".to_string(),
        Value::String(stream.name.clone()),
    );
    document.insert(
        "account_id".to_string(),
        Value::String(::emulator_server::emulator_account_id().to_string()),
    );
    document.insert("region".to_string(), Value::String(stream.region.clone()));
    document.insert(
        "namespace".to_string(),
        Value::String(measured.key.namespace.clone()),
    );
    document.insert(
        "metric_name".to_string(),
        Value::String(measured.key.metric_name.clone()),
    );
    document.insert("dimensions".to_string(), Value::Object(dimensions));
    // The instant the period opens at, in the milliseconds the json record format stamps a
    // record with.
    document.insert(
        "timestamp".to_string(),
        Value::from(milliseconds(measured.start)),
    );
    document.insert("value".to_string(), Value::Object(value));
    document.insert("unit".to_string(), Value::String(measured.unit.clone()));
    // One record per line, so a consumer reading the delivered bytes reads one JSON document
    // per record however many of them a buffer was flushed with.
    format!("{}\n", Value::Object(document))
}

/// An epoch instant in the milliseconds the json record format stamps a record with.
///
/// A period opens at a whole second, so the milliseconds are taken from those seconds: an
/// instant no millisecond count can hold is reported as the nearest one it can, rather than
/// through a conversion whose result bears no relation to it.
fn milliseconds(seconds: f64) -> i64 {
    const MILLIS_PER_SECOND: i64 = 1_000;
    if !seconds.is_finite() {
        return 0;
    }
    (seconds.trunc() as i64).saturating_mul(MILLIS_PER_SECOND)
}

/// Report one statistic in the `value` object, where it is a number JSON can carry: a
/// non-finite fold is no measurement at all, and is left out rather than reported as null.
fn insert_number(value: &mut Map<String, Value>, member: &str, statistic: f64) {
    if let Some(number) = ::serde_json::Number::from_f64(statistic) {
        value.insert(member.to_string(), Value::Number(number));
    }
}

/// Push one stream's records into the delivery stream its `FirehoseArn` names, oldest first, and
/// answer how many of them the destination took.
///
/// Every failure is LOGGED and changes nothing a caller is told: an export is asynchronous and
/// is not part of any operation's answer, exactly as an alarm action's delivery is not
/// ([`super::alarm_actions`]). What it does change is what the stream is metered for: a record
/// the destination never took is not an update the stream streamed.
async fn deliver(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    export: &StreamExport,
    records: &[Vec<u8>],
) -> usize {
    let Some(stream_name) = ::service_common::arn::resource_id_of_type(
        &export.firehose_arn,
        DELIVERY_STREAM_RESOURCE_TYPE,
    ) else {
        ::tracing::warn!(
            firehose_arn = %export.firehose_arn,
            "this metric stream's destination names no Firehose delivery stream, so nothing \
             was exported"
        );
        return 0;
    };
    // The Region the destination is addressed in comes from its own ARN, which a request
    // carried, so it is held to the grammar a Region is written in before it reaches an
    // endpoint.
    let Some(region) = ::service_common::arn::region_of(&export.firehose_arn, FIREHOSE_ARN_NAMESPACE)
        .and_then(::service_common::arn::region_label)
    else {
        ::tracing::warn!(
            firehose_arn = %export.firehose_arn,
            "this metric stream's destination names no Region this emulator can reach, so \
             nothing was exported"
        );
        return 0;
    };
    // The export is the ROLE's write to make, so a role IAM answers does not allow it exports
    // nothing: a record the destination's service would have refused the call to is never
    // pushed. A stream with no role at all makes no such write either — the member is required
    // ("The ARN of an IAM role that this metric stream will use to access Amazon Kinesis Data
    // Firehose resources", `PutMetricStreamInput$RoleArn`), so there is no role to put records
    // under.
    let Some(role_arn) = export.role_arn.as_deref() else {
        ::tracing::warn!(
            firehose_arn = %export.firehose_arn,
            "this metric stream names no role to put records into its destination under, so \
             nothing was exported"
        );
        return 0;
    };
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        PUT_RECORD_ACTION,
        &export.firehose_arn,
    )
    .await
    {
        ::tracing::warn!(
            role_arn = %role_arn,
            firehose_arn = %export.firehose_arn,
            "this metric stream's role is not allowed to put records into its destination, \
             so nothing was exported"
        );
        return 0;
    }
    let client = firehose_client(ctx.callback(), region);
    // One `PutRecord` per record, awaited in turn: the destination takes the records in the
    // order they are put, which is what makes the records of one metric arrive in the order its
    // datapoints were published.
    let mut taken = 0;
    for document in records {
        let built = ::aws_sdk_firehose::types::Record::builder()
            .data(::aws_sdk_firehose::primitives::Blob::new(document.clone()))
            .build();
        let failure = match built {
            Ok(record) => client
                .put_record()
                .delivery_stream_name(stream_name)
                .record(record)
                .send()
                .await
                .err()
                .map(|error| why(&error)),
            Err(error) => Some(why(&error)),
        };
        if let Some(failure) = failure {
            ::tracing::warn!(
                firehose_arn = %export.firehose_arn,
                error = %failure,
                "a metric stream record could not be exported"
            );
            // The destination answered this put a failure, so the ones after it would be
            // answered the same way: the rest of this stream's records are left for the next
            // update rather than re-asked for here.
            return taken;
        }
        taken += 1;
    }
    taken
}

/// How a failed export is worded in the warning: the SDK's own rendering of the answer the
/// destination service gave.
fn why(error: &impl ::std::error::Error) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
}

/// A Firehose client over the in-process callback into this same emulator.
fn firehose_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_firehose::Client {
    let config = ::aws_sdk_firehose::Config::builder()
        .behavior_version(::aws_sdk_firehose::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_firehose::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://firehose.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_firehose::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_firehose::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_firehose::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_firehose::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_firehose::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{MetricStreamStatisticsConfiguration, MetricStreamStatisticsMetric};

    /// A series key, for the filter rows below.
    fn key(namespace: &str, metric_name: &str) -> super::super::MetricKey {
        super::super::MetricKey {
            namespace: namespace.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: super::super::DimensionSet::new(),
        }
    }

    /// A filter naming `namespace`, and the metric names when the row lists any.
    fn filter(namespace: &str, metric_names: Option<&[&str]>) -> MetricStreamFilter {
        MetricStreamFilter {
            namespace: Some(namespace.to_string()),
            metric_names: metric_names
                .map(|names| names.iter().map(|name| (*name).to_string()).collect()),
        }
    }

    /// RULE: a stream with no filter streams every namespace; one with `IncludeFilters` streams
    /// only what they name; one with `ExcludeFilters` streams everything else.
    #[test]
    fn the_filters_decide_which_metrics_are_streamed() {
        let cases: [(&str, GetMetricStreamOutput, &str, &str, bool); 6] = [
            (
                "no filter at all",
                GetMetricStreamOutput::default(),
                "Custom/One",
                "Latency",
                true,
            ),
            (
                "an include filter naming the namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Latency",
                true,
            ),
            (
                "an include filter naming another namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/Two",
                "Latency",
                false,
            ),
            (
                "an include filter naming one metric of the namespace",
                GetMetricStreamOutput {
                    include_filters: Some(vec![filter("Custom/One", Some(&["Latency"]))]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Errors",
                false,
            ),
            (
                "an exclude filter naming the namespace",
                GetMetricStreamOutput {
                    exclude_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/One",
                "Latency",
                false,
            ),
            (
                "an exclude filter naming another namespace",
                GetMetricStreamOutput {
                    exclude_filters: Some(vec![filter("Custom/One", None)]),
                    ..GetMetricStreamOutput::default()
                },
                "Custom/Two",
                "Latency",
                true,
            ),
        ];
        for (case, stream, namespace, metric_name, expected) in cases {
            let streamed = streams_metric(&stream, &key(namespace, metric_name));
            assert_eq!(
                streamed, expected,
                "case {case:?}: expected streamed={expected}, got {streamed}"
            );
        }
    }

    /// RULE: the additional statistics of a metric are the ones every configuration naming THAT
    /// metric asks for, each once.
    #[test]
    fn additional_statistics_are_collected_per_metric() {
        let stream = GetMetricStreamOutput {
            statistics_configurations: Some(vec![
                MetricStreamStatisticsConfiguration {
                    include_metrics: vec![MetricStreamStatisticsMetric {
                        namespace: "Custom/One".to_string(),
                        metric_name: "Latency".to_string(),
                    }],
                    additional_statistics: vec!["p99".to_string(), "p90".to_string()],
                },
                MetricStreamStatisticsConfiguration {
                    include_metrics: vec![MetricStreamStatisticsMetric {
                        namespace: "Custom/One".to_string(),
                        metric_name: "Latency".to_string(),
                    }],
                    additional_statistics: vec!["p99".to_string(), "IQM".to_string()],
                },
            ]),
            ..GetMetricStreamOutput::default()
        };
        let asked = additional_statistics(&stream, &key("Custom/One", "Latency"));
        assert_eq!(
            asked,
            vec!["p99".to_string(), "p90".to_string(), "IQM".to_string()],
            "the statistics of the named metric must be each configuration's, each once, got \
             {asked:?}"
        );
        let other = additional_statistics(&stream, &key("Custom/One", "Errors"));
        assert!(
            other.is_empty(),
            "a metric no configuration names must get no additional statistic, got {other:?}"
        );
    }

    /// RULE: a datapoint is exported in the one-minute period it falls in, and two datapoints of
    /// one period owe one record.
    #[test]
    fn published_periods_are_the_minute_buckets_the_datapoints_fall_in() {
        let datum = |timestamp_secs: f64| super::super::StoredDatum {
            timestamp_secs,
            sample_count: 1.0,
            sum: 1.0,
            minimum: 1.0,
            maximum: 1.0,
            unit: None,
            storage_resolution: 60,
        };
        let series = key("Custom/One", "Latency");
        let published = published_series(&[(
            series.clone(),
            vec![datum(120.0), datum(179.0), datum(60.5), datum(0.0)],
        )]);
        assert_eq!(
            published.len(),
            1,
            "one series must owe one entry, got {published:?}"
        );
        assert_eq!(
            published[0].period_starts,
            vec![0.0, 60.0, 120.0],
            "the periods must be the minute buckets, ascending and without repetition, got {:?}",
            published[0].period_starts
        );
        let none = published_series(&[(series, Vec::new())]);
        assert!(
            none.is_empty(),
            "a datum carrying no observation owes no record, got {none:?}"
        );
    }
}
