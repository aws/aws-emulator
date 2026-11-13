// SPDX-License-Identifier: CC0-1.0

//! The throughput a table has PROVISIONED, sampled into the CloudWatch series DynamoDB
//! publishes it under.
//!
//! # What the series are
//!
//! A provisioned table's capacity setting is not only a member of its description: Amazon
//! DynamoDB reports it as the `AWS/DynamoDB` metrics `ProvisionedReadCapacityUnits` and
//! `ProvisionedWriteCapacityUnits` — "the number of provisioned read/write capacity units for
//! a table or a global secondary index" — so a client graphs the capacity a table HOLDS
//! beside the capacity its requests CONSUME ([`crate::state::consumed_capacity_metrics`]).
//! That pair of families is what every provisioned-versus-consumed ratio, every dashboard of
//! a table's headroom and every target-tracking policy that reads a table's provisioned
//! capacity is computed from.
//!
//! The capacity a table holds is a property of the TABLE, so the series exist for a table
//! whose capacity mode provisions capacity at all and for no other: "On-demand mode ... you
//! don't need to specify how much read and write throughput you expect" (`BillingMode`), and
//! an on-demand table therefore provisions nothing and has no such series. A client reads the
//! documented distinction between the two billing modes off exactly that: the dimension set
//! is there for a provisioned table and absent for an on-demand one.
//!
//! WHICH series a figure belongs to follows the resource the capacity is provisioned FOR,
//! which is the same split the consumption families are published under:
//!
//! * the table's own capacity goes under `{TableName}` alone — "the `TableName` dimension
//!   returns the `ProvisionedWriteCapacityUnits` for the table, but not for any global
//!   secondary indexes" — so an index's capacity is never folded into its table's series;
//! * each global secondary index's own capacity goes under
//!   `{TableName, GlobalSecondaryIndexName}`: "to view `ProvisionedWriteCapacityUnits` for a
//!   global secondary index, you must specify both `TableName` and
//!   `GlobalSecondaryIndexName`";
//! * a local secondary index has no capacity of its own ("local secondary indexes consume
//!   provisioned throughput capacity as part of the table with which they are associated"),
//!   so it adds no series and no figure.
//!
//! The dimension sets are built by the one function that states that rule
//! ([`crate::state::consumed_capacity_metrics::Series::dimensions`]), so the two families of
//! a table cannot be published under different dimensions.
//!
//! # When a sample is taken
//!
//! A capacity setting is a LEVEL rather than a count, so what is published is a sample of the
//! figure in force, taken on a schedule rather than per request: one observation per series
//! every [`SAMPLE_INTERVAL`], which is the resolution a standard-resolution metric is held at.
//! A sample owed at an instant was owed whether or not anybody called, so it is materialised
//! by the seam every request of this service passes through ([`crate::service`]) — the same
//! place the consumption observations are published and the counters a metering interval has
//! made due are refreshed. Nothing is slept for, and a read of the METRICS never takes one:
//! CloudWatch's own readers see whatever the schedule has already made due.
//!
//! A sample is taken as soon as the figure DIFFERS from the one last published, so a capacity
//! change (an `UpdateTable`, a switch between the two billing modes, an index gaining or
//! losing its own capacity) is observable from the call that made it rather than only at the
//! end of the interval it fell in — the same compression this emulator applies to every span
//! it does not sleep through.
//!
//! Nothing here fails a request: a CloudWatch store this engine does not hold, or one at
//! capacity, drops the sample and the request is answered as it was.

use ::std::collections::HashMap;
use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::consumed_capacity_metrics::Series;

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that reports how many read capacity units a resource has provisioned.
const PROVISIONED_READ_CAPACITY_UNITS: &str = "ProvisionedReadCapacityUnits";

/// The metric that reports how many write capacity units it has provisioned.
const PROVISIONED_WRITE_CAPACITY_UNITS: &str = "ProvisionedWriteCapacityUnits";

/// The unit a capacity figure is published in: the `StandardUnit` value for a plain number of
/// things, which is what a number of capacity units is.
const COUNT_UNIT: &str = "Count";

/// The storage resolution a standard-resolution metric is published at, in seconds — which is
/// what `PutMetricData` stores for a publisher that names no `StorageResolution`.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How often a series' level is sampled while the figure it carries does not change: one
/// observation per standard-resolution period ([`STANDARD_STORAGE_RESOLUTION_SECONDS`]), so a
/// window of the series holds one sample of the level per period it covers rather than one per
/// request that happened to pass through.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

/// The capacity one series' resource has provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    /// The series the figures belong to: the table's own, or one global secondary index's.
    pub series: Series,
    /// Its `ReadCapacityUnits`.
    pub read_capacity_units: i64,
    /// Its `WriteCapacityUnits`.
    pub write_capacity_units: i64,
}

/// One sample of one series: the capacity in force on `table` at `at`.
#[derive(Debug, Clone)]
pub struct Sample {
    /// The table the series belongs to, which is its `TableName` dimension.
    pub table: String,
    /// The figures sampled, and the series they belong to.
    pub provisioned: Provisioned,
    /// The instant sampled, which is the instant the datapoints carry.
    pub at: SystemTime,
}

/// What this service has already published of each table's provisioned capacity.
///
/// One entry per table the store holds, keyed by its region and name — the scope a table name
/// resolves in — carrying the figures last sampled and when. That is the whole of what decides
/// whether a sample is due ([`Self::sample_due`]): a figure that has not changed is sampled
/// once per [`SAMPLE_INTERVAL`], and one that has changed is sampled at once.
#[derive(Debug, Default)]
pub struct Publisher {
    /// Keyed by `(region, table name)`.
    published: HashMap<(String, String), Published>,
}

/// The last sample taken of one table's series.
#[derive(Debug)]
struct Published {
    /// When it was taken.
    at: SystemTime,
    /// Every series of that table and the figures it carried, in the order
    /// [`provisioned_series`] states them.
    provisioned: Vec<Provisioned>,
}

impl Publisher {
    /// Take the samples the schedule has already made due at `now`, over the tables `regions`
    /// holds, and record them as published.
    ///
    /// A table is sampled when nothing has been sampled of it yet, when its figures differ
    /// from the ones last sampled, or when [`SAMPLE_INTERVAL`] has elapsed since its last
    /// sample. An on-demand table provisions no capacity, so it yields no sample at all —
    /// including one that has just left the provisioned mode, whose series simply stops
    /// gaining datapoints.
    ///
    /// The bookkeeping of a table this store no longer holds is dropped, so a deleted table
    /// leaves nothing behind.
    pub fn sample_due(
        &mut self,
        regions: &HashMap<String, HashMap<String, super::Table>>,
        now: SystemTime,
    ) -> Vec<Sample> {
        self.published.retain(|(region, name), _| {
            regions
                .get(region)
                .is_some_and(|tables| tables.contains_key(name))
        });
        let mut samples: Vec<Sample> = Vec::new();
        for (region, tables) in regions {
            for (name, table) in tables {
                let provisioned = provisioned_series(table);
                let key = (region.clone(), name.clone());
                let due = self.published.get(&key).is_none_or(|last| {
                    last.provisioned != provisioned
                        || now
                            .duration_since(last.at)
                            .is_ok_and(|age| age >= SAMPLE_INTERVAL)
                });
                if !due {
                    continue;
                }
                samples.extend(provisioned.iter().cloned().map(|figures| Sample {
                    table: name.clone(),
                    provisioned: figures,
                    at: now,
                }));
                self.published.insert(key, Published { at: now, provisioned });
            }
        }
        samples
    }
}

/// Every series one table publishes its provisioned capacity under, with the figures in
/// force: the table's own when it provisions capacity, and one per global secondary index that
/// provisions its own.
///
/// Empty for an on-demand table, whose capacity mode provisions nothing — which is what makes
/// its dimension sets absent from `ListMetrics` rather than present and zero.
fn provisioned_series(table: &super::Table) -> Vec<Provisioned> {
    let mut series: Vec<Provisioned> = Vec::new();
    if let Some(throughput) = table.provisioned_throughput.as_ref() {
        series.push(Provisioned {
            series: Series::Table,
            read_capacity_units: throughput.read_capacity_units,
            write_capacity_units: throughput.write_capacity_units,
        });
    }
    for index in &table.global_secondary_indexes {
        if let Some(throughput) = index.provisioned_throughput.as_ref() {
            series.push(Provisioned {
                series: Series::GlobalSecondaryIndex(index.name.clone()),
                read_capacity_units: throughput.read_capacity_units,
                write_capacity_units: throughput.write_capacity_units,
            });
        }
    }
    series
}

impl super::ComAmazonawsDynamodbDynamodb20120810State {
    /// [`Publisher::sample_due`] over the tables this store holds.
    ///
    /// The store owns both halves — the tables the figures are read from and the record of
    /// what has been published — so the walk happens here, under the one lock, and the
    /// samples are published with it released ([`publish_due_provisioned_capacity`]).
    pub fn sample_due_provisioned_capacity(&mut self, now: SystemTime) -> Vec<Sample> {
        let Self {
            regions,
            provisioned_metrics,
            ..
        } = self;
        provisioned_metrics.sample_due(regions, now)
    }
}

/// Publish the provisioned-capacity samples the schedule has already made due, into the
/// CloudWatch series each resource's capacity is reported under.
///
/// Call it with NO store lock held: this takes this service's lock to take the samples and
/// CloudWatch's to append them, one after the other. It is called from the one seam every
/// request of this service passes through ([`crate::service`]), so a capacity a call
/// established or changed is published as part of serving that call.
pub fn publish_due_provisioned_capacity<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let samples = {
        let mut store = state.lock();
        store.sample_due_provisioned_capacity(SystemTime::now())
    };
    if samples.is_empty() {
        return;
    }
    let Some(metrics) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = metrics.lock();
    for sample in samples {
        let dimensions = sample.provisioned.series.dimensions(&sample.table);
        let timestamp_secs = sample
            .at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        for (metric_name, units) in [
            (
                PROVISIONED_READ_CAPACITY_UNITS,
                sample.provisioned.read_capacity_units,
            ),
            (
                PROVISIONED_WRITE_CAPACITY_UNITS,
                sample.provisioned.write_capacity_units,
            ),
        ] {
            // One observation of a level: the figure in force at this instant.
            let units = units as f64;
            let key = cloudwatch::state::MetricKey {
                namespace: NAMESPACE.to_string(),
                metric_name: metric_name.to_string(),
                dimensions: dimensions.clone(),
            };
            let datum = cloudwatch::state::StoredDatum {
                timestamp_secs,
                sample_count: 1.0,
                sum: units,
                minimum: units,
                maximum: units,
                unit: Some(COUNT_UNIT.to_string()),
                storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
            };
            if store.append_datapoints(key, vec![datum]).is_err() {
                // The metric store holds no more series. The request itself was already
                // served, so nothing is failed for it; the dropped sample is reported where a
                // developer can see it.
                ::tracing::warn!(
                    table = %sample.table,
                    metric = %metric_name,
                    "the CloudWatch metric store is at capacity, so the capacity this table \
                     has provisioned was not published"
                );
            }
        }
    }
}
