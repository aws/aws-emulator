// SPDX-License-Identifier: CC0-1.0

//! The METERING a running metric stream publishes about its own export.
//!
//! A metric stream is a metered pipeline: beside the records it pushes into its destination it
//! publishes, as metrics of its OWN, how much it streamed — the series a client watches a
//! stream's volume on, and the ones its cost is read from. They live in the reserved
//! [`METRIC_STREAMS_NAMESPACE`] under the [`STREAM_NAME_DIMENSION`] dimension naming the stream
//! they are about, so no caller can publish them (`PutMetricData` refuses the reserved
//! namespace prefix, [`crate::ops::put_metric_data`]) and the service itself is the only thing
//! that puts a series there — the same seam this service meters its own API through
//! ([`crate::state::usage`]).
//!
//! What one export is metered as:
//!
//! * [`METRIC_UPDATE`] counts the metric UPDATES the stream exported: one observation per record
//!   that reached the destination.
//! * [`TOTAL_METRIC_UPDATE`] counts the STATISTICS those updates carried, each as an update of
//!   its own: the four every streamed metric gets are the update itself ("a metric stream always
//!   sends the `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT` statistics for each metric that is
//!   streamed", `PutMetricStream`), and each additional statistic a statistics configuration
//!   adds for the metric a record reports counts once more ("You can use the
//!   `StatisticsConfigurations` parameter to have the metric stream send additional statistics
//!   in the stream", `PutMetricStream`). So the two series are equal for a stream that
//!   configures no additional statistic, and the total is the greater exactly when a
//!   configuration added statistics to what was streamed — by one per configured statistic per
//!   record.
//!
//! Both are stamped with the instant the export was MADE rather than with the period the records
//! report: the counters are a reading of the stream's own throughput, so a backdated datapoint
//! streamed now is metered now. A stream that exports nothing — one `StopMetricStreams` has
//! taken out of service, or one whose destination took no record — meters nothing: there is no
//! update to count, which is what tells a stopped stream from a running one by its own metrics.

use super::MeasuredPeriod;
use crate::state::{
    ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, DimensionSet, MetricKey,
};

/// The namespace a metric stream's own metrics live in, which no caller may publish into
/// ([`crate::ops::put_metric_data`] refuses the reserved prefix).
pub const METRIC_STREAMS_NAMESPACE: &str = "AWS/CloudWatch/MetricStreams";

/// The metric counting the metric updates a stream exported: a `Sum` over a window is how many
/// updates that window's exports carried.
pub const METRIC_UPDATE: &str = "MetricUpdate";

/// The metric counting the same exports with each statistic of an update counted separately.
pub const TOTAL_METRIC_UPDATE: &str = "TotalMetricUpdate";

/// The dimension naming WHICH stream a metering series is about.
pub const STREAM_NAME_DIMENSION: &str = "MetricStreamName";

/// What ONE record of an export counts as in [`TOTAL_METRIC_UPDATE`] before its additional
/// statistics are added: the update itself, which is the four statistics every streamed metric
/// carries.
const UPDATE_ITSELF: f64 = 1.0;

/// The series one of a stream's own metrics lives in: the metric name, under the dimension
/// naming the stream.
///
/// Derived from the stream's name alone, so the series the export metered and the one a reader
/// asks for by that stream name are the same series.
#[must_use]
pub fn metering_key(metric_name: &str, stream_name: &str) -> MetricKey {
    let mut dimensions = DimensionSet::new();
    dimensions.insert(STREAM_NAME_DIMENSION.to_string(), stream_name.to_string());
    MetricKey {
        namespace: METRIC_STREAMS_NAMESPACE.to_string(),
        metric_name: metric_name.to_string(),
        dimensions,
    }
}

/// What one export counts as: the updates it carried, and those updates' statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportedUpdates {
    /// The metric updates exported, one per record.
    pub updates: f64,
    /// The statistics those updates carried, each counted as an update of its own.
    pub statistic_updates: f64,
}

/// What the first `taken` of one export's measurements count as: one update per record, and per
/// record the update itself plus each additional statistic it carries.
///
/// The measurements and the records rendered from them are one for one and in the same order, so
/// `taken` is how many of them the destination took.
///
/// A record's additional statistics are the ones it CARRIES, which is what the stream streamed:
/// a configured statistic a period's distribution does not define is left out of the record
/// ([`super::measured_period`]) and so is not counted as an update either, which is why the
/// total is at most the updates times one plus the configured statistics rather than always
/// exactly that.
#[must_use]
pub(super) fn exported(measured: &[MeasuredPeriod], taken: usize) -> ExportedUpdates {
    let delivered = measured.iter().take(taken);
    ExportedUpdates {
        updates: delivered.clone().count() as f64,
        statistic_updates: delivered
            .map(|measured| UPDATE_ITSELF + measured.additional.len() as f64)
            .sum(),
    }
}

impl State {
    /// Meter `exported` against the stream named `stream_name`, stamped `now_secs`.
    ///
    /// Each series takes as many observations of 1 as the count it reports, through the seam
    /// every count this service publishes about itself goes through
    /// ([`State::append_self_metered_count`]): a `Sum` over a window is how much that window
    /// streamed, a `SampleCount` is the same number and an `Average` is 1 — the aggregate of
    /// that many single updates.
    pub fn meter_exported_updates(
        &mut self,
        stream_name: &str,
        exported: ExportedUpdates,
        now_secs: f64,
    ) {
        if exported.updates <= 0.0 {
            return;
        }
        for (metric_name, count) in [
            (METRIC_UPDATE, exported.updates),
            (TOTAL_METRIC_UPDATE, exported.statistic_updates),
        ] {
            self.append_self_metered_count(metering_key(metric_name, stream_name), count, now_secs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MetricKey, UNIT_NONE};

    /// A measured period carrying `additional` additional statistics, for the rows below.
    fn measured(additional: usize) -> MeasuredPeriod {
        MeasuredPeriod {
            key: MetricKey {
                namespace: "Custom/One".to_string(),
                metric_name: "Latency".to_string(),
                dimensions: DimensionSet::new(),
            },
            start: 60.0,
            unit: UNIT_NONE.to_string(),
            statistics: crate::state::BasicStatistics::default(),
            additional: (0..additional)
                .map(|index| (format!("p{index}0"), index as f64))
                .collect(),
        }
    }

    /// One counting case: the records one export carried, with how many additional statistics
    /// each of them did, and what the counts are for every record and for one record fewer.
    struct CountingRow {
        case: &'static str,
        /// The additional statistics each record carries, one entry per record.
        records: Vec<usize>,
        /// The counts when the destination took every record.
        all: ExportedUpdates,
        /// The counts when it took one fewer, which is an export whose last put it refused.
        short_of_one: ExportedUpdates,
    }

    /// The counts as `(updates, statistic updates)`, for the rows below.
    fn counts(updates: f64, statistic_updates: f64) -> ExportedUpdates {
        ExportedUpdates {
            updates,
            statistic_updates,
        }
    }

    /// RULE: one record is one metric update, and counts in the total as itself plus each
    /// additional statistic it carries — over the records the destination TOOK.
    #[test]
    fn the_total_counts_each_additional_statistic_as_one_more_update() {
        // The rows differ along how many records one export carried and how many additional
        // statistics each of them did, and each is counted twice: for every record, and for
        // one record fewer.
        let cases: Vec<CountingRow> = vec![
            CountingRow {
                case: "nothing exported",
                records: vec![],
                all: counts(0.0, 0.0),
                short_of_one: counts(0.0, 0.0),
            },
            CountingRow {
                case: "one record with no additional statistic",
                records: vec![0],
                all: counts(1.0, 1.0),
                short_of_one: counts(0.0, 0.0),
            },
            CountingRow {
                case: "one record with three of them",
                records: vec![3],
                all: counts(1.0, 4.0),
                short_of_one: counts(0.0, 0.0),
            },
            CountingRow {
                case: "two records with six of them each",
                records: vec![6, 6],
                all: counts(2.0, 14.0),
                short_of_one: counts(1.0, 7.0),
            },
            CountingRow {
                case: "three records, two of them configured",
                records: vec![0, 2, 6],
                all: counts(3.0, 11.0),
                short_of_one: counts(2.0, 4.0),
            },
        ];
        for row in cases {
            let case = row.case;
            let delivered: Vec<MeasuredPeriod> = row.records.into_iter().map(measured).collect();
            for (taken, expected) in [
                (delivered.len(), row.all),
                (delivered.len().saturating_sub(1), row.short_of_one),
            ] {
                let counted = exported(&delivered, taken);
                assert_eq!(
                    counted, expected,
                    "case {case:?} with {taken} record(s) taken: expected {expected:?}, got \
                     {counted:?}"
                );
            }
        }
    }
}
