// SPDX-License-Identifier: CC0-1.0

//! Metric extraction from an INGESTED log event, the path that turns an Embedded Metric
//! Format log event into CloudWatch metric datapoints.
//!
//! A log event whose message is an Embedded Metric Format document declares the metrics it
//! carries in its own `_aws` member: a `CloudWatchMetrics` list of directives, each naming
//! the namespace to publish into, the metrics (by the name of the document member holding
//! each value) and the dimension sets to publish them under. Ingesting such an event
//! publishes those values as metric data, which is why a client sees a datapoint in that
//! namespace without ever calling `PutMetricData`.
//!
//! The datapoints are appended to the CloudWatch metric store — the SAME store
//! `PutMetricData` writes and `GetMetricStatistics` / `GetMetricData` read — reached through
//! the shared engine, so one metric has one home and the two writers cannot drift.
//!
//! What this module does NOT decide is whether extraction happens at all: an account-level
//! `METRIC_EXTRACTION_POLICY` can turn it off for the log groups its selection criteria
//! name, and that question belongs to the account policies in [`super`]. What this module
//! does do with that answer is account for it: a document whose metric creation was skipped
//! because extraction is off for its log group is COUNTED, as the service's own
//! [`EMF_DISABLED_ERRORS_METRIC`] metric in [`LOGS_NAMESPACE`], so a client can tell a
//! suppressing policy from a broken pipeline.

use std::collections::{BTreeMap, BTreeSet};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The member of a log event's message that makes it an Embedded Metric Format document,
/// and the members of it this extraction reads. The names are the ones the format defines,
/// which is also how the documents the format's own libraries emit spell them.
const AWS_MEMBER: &str = "_aws";
/// See [`AWS_MEMBER`]: the directives, each publishing into one namespace.
const CLOUDWATCH_METRICS_MEMBER: &str = "CloudWatchMetrics";
/// See [`AWS_MEMBER`]: the instant the document's values were observed, in milliseconds
/// since the epoch.
const TIMESTAMP_MEMBER: &str = "Timestamp";
/// See [`AWS_MEMBER`]: the namespace one directive publishes into.
const NAMESPACE_MEMBER: &str = "Namespace";
/// See [`AWS_MEMBER`]: the metrics one directive declares.
const METRICS_MEMBER: &str = "Metrics";
/// See [`AWS_MEMBER`]: the dimension sets one directive publishes its metrics under, each an
/// array of the document members whose values are the dimension values.
const DIMENSIONS_MEMBER: &str = "Dimensions";
/// See [`AWS_MEMBER`]: the name of the document member holding one metric's value, which is
/// also the metric's name.
const NAME_MEMBER: &str = "Name";
/// See [`AWS_MEMBER`]: the unit one metric's values are in.
const UNIT_MEMBER: &str = "Unit";
/// See [`AWS_MEMBER`]: the resolution one metric is stored at.
const STORAGE_RESOLUTION_MEMBER: &str = "StorageResolution";

/// The resolution a metric is stored at when nothing declares one: the default
/// `MetricDatum$StorageResolution` documents ("This field is optional, if you do not specify
/// it the default of 60 is used"), so an extracted metric is a regular-resolution metric
/// unless the document says otherwise. Shared with the metric-filter extraction in
/// [`super::metric_filters`], whose transformations declare no resolution at all.
pub const DEFAULT_STORAGE_RESOLUTION: i32 = 60;

/// The namespace CloudWatch Logs publishes its OWN operational metrics into, rather than a
/// client's: the service's metrics are "in the AWS/Logs namespace".
pub const LOGS_NAMESPACE: &str = "AWS/Logs";

/// The metric counting the Embedded Metric Format documents whose metric creation was
/// skipped because it is disabled for the log group they were ingested into. See
/// [`LOGS_NAMESPACE`].
pub const EMF_DISABLED_ERRORS_METRIC: &str = "EMFDisabledErrors";

/// The metric counting the ingested log events that were an Embedded Metric Format document
/// this service could not READ as one, so no metric could be created from them. See
/// [`LOGS_NAMESPACE`]; this is the signal that tells a client its EMF producer is emitting
/// documents the service rejects from one that is emitting none.
pub const EMF_PARSING_ERRORS_METRIC: &str = "EMFParsingErrors";

/// The metric counting the log events a log group took in, published for every ingestion
/// whatever the events carry: "the number of log events uploaded to CloudWatch Logs". See
/// [`LOGS_NAMESPACE`].
pub const INCOMING_LOG_EVENTS_METRIC: &str = "IncomingLogEvents";

/// The metric measuring what those events WEIGHED: "the volume of log events in uncompressed
/// bytes uploaded to CloudWatch Logs". See [`LOGS_NAMESPACE`].
pub const INCOMING_BYTES_METRIC: &str = "IncomingBytes";

/// The dimension [`EMF_DISABLED_ERRORS_METRIC`] is published under: the log group whose
/// documents were skipped, which is the granularity a metric extraction policy selects at.
pub const LOG_GROUP_NAME_DIMENSION: &str = "LogGroupName";

/// The unit of a count of documents, from the `StandardUnit` enumeration CloudWatch metric
/// data is published with.
const COUNT_UNIT: &str = "Count";

/// The unit of a VOLUME of log data, from the same enumeration.
const BYTES_UNIT: &str = "Bytes";

/// The `StandardUnit` value for a measurement in no unit, which is what a count of log events
/// uploaded carries in the `AWS/Logs` metric table.
const NO_UNIT: &str =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::UNIT_NONE;

/// How many datapoints one ingested event may publish, whatever its document or the log
/// group's metric filters declare: the ceiling that keeps one `PutLogEvents` batch from
/// writing an unbounded number of series. THE EMULATOR'S OWN CEILING, not a documented
/// limit.
pub const MAX_DATAPOINTS_PER_EVENT: usize = 100;

/// How many DISTINCT dimension projections one directive resolves, whatever number of
/// dimension sets it declares: the ceiling that keeps the work one message drives linear in
/// its own size rather than a product of the counts it declares. THE EMULATOR'S OWN CEILING,
/// not a documented limit.
const MAX_PROJECTIONS_PER_DIRECTIVE: usize = 30;

/// One datapoint an ingested Embedded Metric Format document publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedDatapoint {
    /// The namespace the directive publishes into.
    pub namespace: String,
    /// The metric's name, which is the document member the value was read from.
    pub metric_name: String,
    /// The dimension set the value is published under, name to value.
    pub dimensions: BTreeMap<String, String>,
    /// The instant the value was observed, in milliseconds since the epoch.
    pub timestamp_ms: i64,
    /// The value itself.
    pub value: f64,
    /// The unit the directive declared, when it declared one.
    pub unit: Option<String>,
    /// The resolution the metric is stored at.
    pub storage_resolution: i32,
}

/// The datapoints the log event `message` publishes, which is empty for every message that
/// is not an Embedded Metric Format document.
///
/// `event_timestamp_ms` is the log event's own timestamp, used for a document that declares
/// no `Timestamp` of its own: the value was observed when the event it travelled in was.
pub fn extracted_datapoints(message: &str, event_timestamp_ms: i64) -> Vec<ExtractedDatapoint> {
    extracted_datapoints_bounded(message, event_timestamp_ms, MAX_DATAPOINTS_PER_EVENT)
}

/// [`extracted_datapoints`], stopping once `limit` of them are extracted.
///
/// `limit` bounds the WORK as well as the answer: every loop over a part of the document the
/// message chose the size of — the directives, one directive's metrics, one metric's values
/// and the dimension sets they are published under — is left as soon as the limit is
/// reached, so one message cannot drive iteration proportional to the product of the counts
/// it declares. A caller that only needs to know WHETHER the message declares a metric asks
/// for one.
pub fn extracted_datapoints_bounded(
    message: &str,
    event_timestamp_ms: i64,
    limit: usize,
) -> Vec<ExtractedDatapoint> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(message) else {
        return Vec::new();
    };
    let Some(metadata) = document.get(AWS_MEMBER).and_then(|aws| aws.as_object()) else {
        return Vec::new();
    };
    let timestamp_ms = metadata
        .get(TIMESTAMP_MEMBER)
        .and_then(::serde_json::Value::as_i64)
        .unwrap_or(event_timestamp_ms);
    let Some(directives) = metadata
        .get(CLOUDWATCH_METRICS_MEMBER)
        .and_then(::serde_json::Value::as_array)
    else {
        return Vec::new();
    };

    let mut datapoints: Vec<ExtractedDatapoint> = Vec::new();
    // The numbers one document member holds, read at most once however many of the
    // document's metrics name that same member.
    let mut values_by_member: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for directive in directives {
        if datapoints.len() >= limit {
            return datapoints;
        }
        let Some(namespace) = directive
            .get(NAMESPACE_MEMBER)
            .and_then(::serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(metrics) = directive
            .get(METRICS_MEMBER)
            .and_then(::serde_json::Value::as_array)
        else {
            continue;
        };
        // A directive that declares no metric publishes nothing, so its dimension sets are
        // never resolved: the work one message drives is paid only where a datapoint can come
        // of it.
        if metrics.is_empty() {
            continue;
        }
        // "Dimensions" is a list of dimension SETS: each set publishes the directive's
        // metrics under its own dimensions, so one metric of a directive declaring two sets
        // is two series. A directive declaring no set at all publishes the metrics with no
        // dimensions. Resolving them is bounded by what this event may still publish, so no
        // directive drives more of that work than its datapoints could ever amount to.
        let dimension_sets =
            dimension_sets(directive, &document, limit.saturating_sub(datapoints.len()));
        // A directive that declares no PROJECTION at all — every element of its `Dimensions`
        // being something other than a list of names — resolves no series for its metrics to
        // be read for, so it publishes nothing.
        if dimension_sets.is_empty() {
            continue;
        }
        for metric in metrics {
            if datapoints.len() >= limit {
                return datapoints;
            }
            let Some(metric_name) = metric.get(NAME_MEMBER).and_then(::serde_json::Value::as_str)
            else {
                continue;
            };
            let unit = metric
                .get(UNIT_MEMBER)
                .and_then(::serde_json::Value::as_str)
                .map(ToString::to_string);
            let storage_resolution = metric
                .get(STORAGE_RESOLUTION_MEMBER)
                .and_then(::serde_json::Value::as_i64)
                .and_then(|resolution| i32::try_from(resolution).ok())
                .unwrap_or(DEFAULT_STORAGE_RESOLUTION);
            // The metric's values live in the document member the directive names, as one
            // number or as an array of numbers: a member the document does not carry, or
            // that is not numeric, declares a metric with nothing observed, so it publishes
            // no datapoint rather than a zero nothing measured.
            let values: &[f64] = values_by_member
                .entry(metric_name)
                .or_insert_with(|| values_of(document.get(metric_name)));
            for value in values {
                for dimensions in &dimension_sets {
                    if datapoints.len() >= limit {
                        return datapoints;
                    }
                    datapoints.push(ExtractedDatapoint {
                        namespace: namespace.to_string(),
                        metric_name: metric_name.to_string(),
                        dimensions: dimensions.clone(),
                        timestamp_ms,
                        value: *value,
                        unit: unit.clone(),
                        storage_resolution,
                    });
                }
            }
        }
    }
    datapoints
}

/// The datapoints an ingested log event publishes when metric extraction is OFF for the log
/// group `log_group_name` it landed in: one count of [`EMF_DISABLED_ERRORS_METRIC`] for a
/// document whose metrics were skipped, and nothing at all for a message that declared no
/// metric to skip.
///
/// A suppressed document publishes NOTHING into its own namespace — that is what the policy
/// does — so the count is the only trace it leaves, and one document counts one whatever
/// number of metrics it declared. The count carries the document's own instant, so it falls
/// in the same period as the datapoints it stands in for.
pub fn suppressed_datapoints(
    message: &str,
    event_timestamp_ms: i64,
    log_group_name: &str,
) -> Vec<ExtractedDatapoint> {
    // One document counts once, so only the FIRST datapoint it would have published is
    // needed: the extraction stops there rather than paying for the whole document to
    // answer whether it declared anything.
    let skipped = extracted_datapoints_bounded(message, event_timestamp_ms, 1);
    let Some(first) = skipped.first() else {
        return Vec::new();
    };
    vec![ExtractedDatapoint {
        namespace: LOGS_NAMESPACE.to_string(),
        metric_name: EMF_DISABLED_ERRORS_METRIC.to_string(),
        dimensions: BTreeMap::from([(
            LOG_GROUP_NAME_DIMENSION.to_string(),
            log_group_name.to_string(),
        )]),
        timestamp_ms: first.timestamp_ms,
        value: 1.0,
        unit: Some(COUNT_UNIT.to_string()),
        storage_resolution: DEFAULT_STORAGE_RESOLUTION,
    }]
}

/// One datapoint of this service's own metric for the log group `log_group_name`, in
/// [`LOGS_NAMESPACE`] and under [`LOG_GROUP_NAME_DIMENSION`] — the dimension the `AWS/Logs`
/// metrics of an ingestion are published under, since the ingestion belongs to a log group
/// rather than to any one message.
fn service_datapoint(
    metric_name: &str,
    log_group_name: &str,
    timestamp_ms: i64,
    value: f64,
    unit: &str,
) -> ExtractedDatapoint {
    ExtractedDatapoint {
        namespace: LOGS_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions: BTreeMap::from([(
            LOG_GROUP_NAME_DIMENSION.to_string(),
            log_group_name.to_string(),
        )]),
        timestamp_ms,
        value,
        unit: Some(unit.to_string()),
        storage_resolution: DEFAULT_STORAGE_RESOLUTION,
    }
}

/// The `AWS/Logs` datapoints an ingestion into `log_group_name` publishes for the log events it
/// STORED: how many they were ([`INCOMING_LOG_EVENTS_METRIC`]) and what they weighed
/// ([`INCOMING_BYTES_METRIC`], the messages' own uncompressed bytes).
///
/// Both carry the instant the batch was ingested, which is when the volume was observed, and
/// an ingestion that stored nothing publishes neither: there was no incoming log event to
/// count. This is a property of every ingestion rather than of what a message carries, so it
/// is independent of the Embedded Metric Format path above.
#[must_use]
pub fn ingestion_datapoints(
    log_group_name: &str,
    stored_messages: &[(i64, String)],
    ingestion_time_ms: i64,
) -> Vec<ExtractedDatapoint> {
    if stored_messages.is_empty() {
        return Vec::new();
    }
    let events = stored_messages.len() as f64;
    let bytes: usize = stored_messages
        .iter()
        .map(|(_timestamp, message)| message.len())
        .sum();
    vec![
        service_datapoint(
            INCOMING_LOG_EVENTS_METRIC,
            log_group_name,
            ingestion_time_ms,
            events,
            NO_UNIT,
        ),
        service_datapoint(
            INCOMING_BYTES_METRIC,
            log_group_name,
            ingestion_time_ms,
            bytes as f64,
            BYTES_UNIT,
        ),
    ]
}

/// The datapoint an ingested log event publishes when it WAS an Embedded Metric Format
/// document and this service could not read it as one: one count of
/// [`EMF_PARSING_ERRORS_METRIC`] for the log group it landed in, and nothing for a message that
/// is not such a document ([`unreadable_emf_document`]).
#[must_use]
pub fn parsing_error_datapoints(
    message: &str,
    event_timestamp_ms: i64,
    log_group_name: &str,
) -> Vec<ExtractedDatapoint> {
    if !unreadable_emf_document(message) {
        return Vec::new();
    }
    vec![service_datapoint(
        EMF_PARSING_ERRORS_METRIC,
        log_group_name,
        event_timestamp_ms,
        1.0,
        NO_UNIT,
    )]
}

/// Whether `message` declares itself an Embedded Metric Format document — it carries the
/// format's own [`AWS_MEMBER`] — and the metadata that member holds cannot be READ as the
/// format defines it: a metadata member that is not an object, one that declares no
/// `CloudWatchMetrics` list, an empty list, or a directive that names no namespace or no
/// metrics list.
///
/// A message that is not JSON at all, and a JSON document carrying no such member, is an
/// ordinary log event rather than a failed document, so neither counts. A document whose
/// directives READ but whose declared values the document does not supply is a different thing
/// again: it was understood, and it simply published nothing.
fn unreadable_emf_document(message: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(message) else {
        return false;
    };
    let Some(declared) = document.get(AWS_MEMBER) else {
        return false;
    };
    let Some(metadata) = declared.as_object() else {
        return true;
    };
    let Some(directives) = metadata
        .get(CLOUDWATCH_METRICS_MEMBER)
        .and_then(::serde_json::Value::as_array)
    else {
        return true;
    };
    directives.is_empty() || !directives.iter().all(readable_directive)
}

/// Whether one `CloudWatchMetrics` directive carries what the format requires of it: the
/// namespace it publishes into and the list of metrics it declares.
fn readable_directive(directive: &::serde_json::Value) -> bool {
    directive
        .get(NAMESPACE_MEMBER)
        .and_then(::serde_json::Value::as_str)
        .is_some_and(|namespace| !namespace.is_empty())
        && directive
            .get(METRICS_MEMBER)
            .and_then(::serde_json::Value::as_array)
            .is_some()
}

/// The dimension sets one directive publishes under, each resolved from the document's own
/// members: a dimension's value is the value of the document member named by it.
///
/// A declared name the document supplies no string value for is not a dimension of the
/// published series: it is LEFT OUT of the projection it was declared in, so a projection
/// whose names the document supplies none of is the projection with no dimensions — the
/// series a metric declared without dimensions is read back under. Declaring one name twice,
/// or two projections the document resolves to the same dimensions, is one series and not
/// two, so the resolved projections are a SET: a value observed once is one datapoint of its
/// series.
///
/// `limit` is how many datapoints the event this document travelled in may still publish, and
/// it bounds this resolution as well as the answer: a projection past it could publish no
/// datapoint anyway, and no directive resolves more than [`MAX_PROJECTIONS_PER_DIRECTIVE`] of
/// them whatever its `Dimensions` declares, so one message cannot drive work proportional to a
/// product of the counts it declares.
fn dimension_sets(
    directive: &::serde_json::Value,
    document: &::serde_json::Value,
    limit: usize,
) -> Vec<BTreeMap<String, String>> {
    let Some(sets) = directive
        .get(DIMENSIONS_MEMBER)
        .and_then(::serde_json::Value::as_array)
    else {
        return vec![BTreeMap::new()];
    };
    if sets.is_empty() {
        return vec![BTreeMap::new()];
    }
    let ceiling = limit.min(MAX_PROJECTIONS_PER_DIRECTIVE);
    // A SET of the projections resolved so far, so one more of them costs a lookup rather
    // than a scan of every projection the message has declared before it.
    let mut resolved: BTreeSet<BTreeMap<String, String>> = BTreeSet::new();
    for set in sets {
        if resolved.len() >= ceiling {
            break;
        }
        // An element that is not a list of names declares no projection at all, which is not
        // the same as declaring an empty one.
        let Some(names) = set.as_array() else {
            continue;
        };
        let mut dimensions: BTreeMap<String, String> = BTreeMap::new();
        for name in names {
            let Some(name) = name.as_str() else {
                continue;
            };
            let Some(value) = document.get(name).and_then(::serde_json::Value::as_str) else {
                continue;
            };
            dimensions.insert(name.to_string(), value.to_string());
        }
        resolved.insert(dimensions);
    }
    resolved.into_iter().collect()
}

/// The numeric observations a document member holds: one value, every value of an array of
/// them, or none at all for a member that is absent or not numeric.
fn values_of(member: Option<&::serde_json::Value>) -> Vec<f64> {
    match member {
        Some(::serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(::serde_json::Value::as_f64)
            .collect(),
        Some(value) => value.as_f64().map(|value| vec![value]).unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Publish `datapoints` into the CloudWatch metric store the shared engine holds.
///
/// Nothing is published when that store is not mounted: a datapoint has one home, and this
/// path never makes a second one. The call is made with no log-store guard held, so the two
/// services' stores are never locked at once.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, datapoints: &[ExtractedDatapoint]) {
    if datapoints.is_empty() {
        return;
    }
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for datapoint in datapoints {
        let key = cloudwatch::state::MetricKey {
            namespace: datapoint.namespace.clone(),
            metric_name: datapoint.metric_name.clone(),
            dimensions: datapoint.dimensions.clone(),
        };
        let stored = cloudwatch::state::StoredDatum {
            // CloudWatch holds a datapoint's instant in seconds, which is the unit its own
            // readers window by; the document carries milliseconds.
            timestamp_secs: datapoint.timestamp_ms as f64 / 1_000.0,
            sample_count: 1.0,
            sum: datapoint.value,
            minimum: datapoint.value,
            maximum: datapoint.value,
            unit: datapoint.unit.clone(),
            storage_resolution: datapoint.storage_resolution,
        };
        // A metric store at capacity takes no new series; the ingest it was extracted from
        // is not failed for it, because the log event itself was accepted.
        let _ = store.append_datapoints(key, vec![stored]);
    }
}
