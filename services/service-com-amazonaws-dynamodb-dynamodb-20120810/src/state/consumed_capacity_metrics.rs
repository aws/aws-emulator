// SPDX-License-Identifier: CC0-1.0

//! The throughput each request consumes, counted into the CloudWatch series DynamoDB
//! publishes it under.
//!
//! A table's consumed capacity is not only a member of the response that asked for it: Amazon
//! DynamoDB publishes it as the `AWS/DynamoDB` metrics `ConsumedReadCapacityUnits` and
//! `ConsumedWriteCapacityUnits`, dimensioned by the table it was consumed on. Those series are what a client
//! graphs, and what every alarm on a table's utilisation is evaluated against — including the
//! ones Application Auto Scaling mints for a target-tracking policy on
//! `dynamodb:table:ReadCapacityUnits` and its three siblings, which is how a provisioned table
//! scales itself under load. A table whose consumption is never published has no utilisation
//! to breach on, so no such alarm can ever fire.
//!
//! The units published are the SAME ones the operation reports as its `ConsumedCapacity`
//! (`crate::state::read_capacity_units` / `crate::state::write_capacity_units`), whether or
//! not the caller asked for them: what a request consumed is a property of the request, not of
//! the member it chose to read back.
//!
//! WHICH series a charge lands in follows the line of `ConsumedCapacity` that itemised it, so
//! a client reading the metrics can split the consumption the same way the response does:
//!
//! * the table's own line goes under `{TableName}` alone — a global secondary index is
//!   provisioned for throughput of its own ("a global secondary index … has its own
//!   provisioned throughput settings for read and write activity that are separate from those
//!   of its table"), so what an index took is NOT part of what its table took and must not be
//!   folded into the table's series;
//! * each global secondary index's line goes under `{TableName, GlobalSecondaryIndexName}`,
//!   the one dimension pair DynamoDB reports a per-index figure on;
//! * a local secondary index has no dimension of its own, and it draws on the TABLE's
//!   provisioned throughput ("local secondary indexes consume provisioned throughput capacity
//!   as part of the table with which they are associated"), so its line is counted into the
//!   table's own series.
//!
//! Nothing is published under an index name ALONE: the index dimension identifies an index
//! WITHIN a table, so a series keyed by `{GlobalSecondaryIndexName}` with no `TableName` is
//! not one DynamoDB has, and a read of it finds no datapoints.
//!
//! The datum is one observation of that request's consumption, timestamped when it happened,
//! so a `Sum` over a period is the units the table consumed in that period — which is how the
//! metric is defined and how an alarm on it is evaluated. Nothing here fails a request: a
//! CloudWatch store this engine does not hold, or one at capacity, drops the observation and
//! the request is answered as it was.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts the read capacity units a table consumed.
const CONSUMED_READ_CAPACITY_UNITS: &str = "ConsumedReadCapacityUnits";

/// The metric that counts the write capacity units it consumed.
const CONSUMED_WRITE_CAPACITY_UNITS: &str = "ConsumedWriteCapacityUnits";

/// The dimension that names the table a series belongs to.
const TABLE_NAME: &str = "TableName";

/// The dimension that names the GLOBAL SECONDARY INDEX a series belongs to, beside the table
/// the index is declared on — the one per-index dimension DynamoDB reports consumption under.
const GLOBAL_SECONDARY_INDEX_NAME: &str = "GlobalSecondaryIndexName";

/// The storage resolution a standard-resolution metric is published at, in seconds — which is
/// what `PutMetricData` stores for a publisher that names no `StorageResolution`.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Which half of a table's provisioned throughput a request consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumed {
    /// Read capacity units.
    Read,
    /// Write capacity units.
    Write,
}

impl Consumed {
    /// The metric name this half of the throughput is counted under.
    fn metric_name(self) -> &'static str {
        match self {
            Self::Read => CONSUMED_READ_CAPACITY_UNITS,
            Self::Write => CONSUMED_WRITE_CAPACITY_UNITS,
        }
    }
}

/// How many observations one table holds before the oldest is dropped.
///
/// The EMULATOR's own bound, so a store nobody publishes from cannot grow without end: the
/// observations are published by the service seam every request of this service passes
/// through ([`crate::service`]), so a table only holds what one request charged it.
const MAX_PENDING_OBSERVATIONS: usize = 4_096;

/// WHICH of a table's series one charge is counted into.
///
/// The two are the two dimension sets DynamoDB reports a table's consumption on, and they are
/// disjoint: the capacity an index was provisioned for is not the capacity its table was, so
/// an index's units belong to the index's series and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Series {
    /// The table's own series, dimensioned `{TableName}`: what the table itself took, plus
    /// what a local secondary index took out of the table's throughput.
    Table,
    /// One global secondary index's series, dimensioned
    /// `{TableName, GlobalSecondaryIndexName}`.
    GlobalSecondaryIndex(String),
}

impl Series {
    /// The dimension set DynamoDB publishes this series of `table` under.
    ///
    /// The table's own series is dimensioned by the table alone; an index's carries the index
    /// BESIDE it, because the index dimension identifies an index WITHIN a table and a series
    /// keyed by an index name alone is not one DynamoDB has. Shared with every other family a
    /// table publishes, so the two cannot be published under different dimensions
    /// ([`crate::state::provisioned_capacity_metrics`]).
    pub fn dimensions(&self, table: &str) -> cloudwatch::state::DimensionSet {
        let mut dimensions = cloudwatch::state::DimensionSet::new();
        dimensions.insert(TABLE_NAME.to_string(), table.to_string());
        if let Self::GlobalSecondaryIndex(index) = self {
            dimensions.insert(GLOBAL_SECONDARY_INDEX_NAME.to_string(), index.clone());
        }
        dimensions
    }
}

/// One charge against a table's throughput: how much, of which half, into which series, and
/// when.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The instant the request consumed it at, which is the instant the datapoint carries.
    pub at: ::std::time::SystemTime,
    /// The capacity units it consumed.
    pub units: f64,
    /// Which half of the throughput they came out of.
    pub consumed: Consumed,
    /// The series the units are counted into.
    pub series: Series,
}

/// What a table has been charged and this service has not published yet.
///
/// The charge happens under this store's lock, where the request's figure is settled — inside
/// the admission every read and every item write of a table goes through, or against the
/// ledger a transactional or statement write reports
/// ([`charge_reported_capacity`]) — and the publish happens with that lock released, in the
/// CloudWatch store. So the observations wait here in between, exactly as a capacity movement
/// waits to be delivered to the service that owns the scaled resource.
#[derive(Debug, Default)]
pub struct Pending(Vec<Observation>);

impl Pending {
    /// Record that this table itself consumed `units` of `consumed` throughput at `at`.
    pub fn charge(&mut self, consumed: Consumed, units: f64, at: ::std::time::SystemTime) {
        self.charge_series(consumed, Series::Table, units, at);
    }

    /// Record that the GLOBAL secondary index `index` of this table consumed `units` of
    /// `consumed` throughput at `at`, which is the index's own series rather than the
    /// table's.
    pub fn charge_global_index(
        &mut self,
        consumed: Consumed,
        index: &str,
        units: f64,
        at: ::std::time::SystemTime,
    ) {
        self.charge_series(
            consumed,
            Series::GlobalSecondaryIndex(index.to_string()),
            units,
            at,
        );
    }

    /// Record what ONE write consumed, itemised the way `ConsumedCapacity` itemises it: the
    /// table's own units into the table's series, and each global secondary index's units into
    /// that index's series.
    ///
    /// A local secondary index draws on the table's throughput, so its units go into the
    /// table's series beside the table's own; a vector index is metered in BYTES rather than
    /// in capacity units ([`crate::state::WriteCapacity::vector_indexes`]), so it is no part
    /// of either capacity series.
    pub fn charge_write(
        &mut self,
        capacity: &crate::state::WriteCapacity,
        at: ::std::time::SystemTime,
    ) {
        self.charge_itemised(Consumed::Write, capacity, at);
    }

    /// [`Self::charge_write`] for either half of the throughput: `consumed` is the half the
    /// request spent, which is the half it reports its figures in.
    ///
    /// The two halves split the same itemisation, because `ConsumedCapacity` itemises a read
    /// and a write the same way — a figure for the table and one per index — so a request
    /// billed as a read counts its figures into the read series of exactly those resources.
    pub fn charge_itemised(
        &mut self,
        consumed: Consumed,
        capacity: &crate::state::WriteCapacity,
        at: ::std::time::SystemTime,
    ) {
        let table_units = capacity
            .local_secondary_indexes
            .values()
            .fold(capacity.table, |units, local| units + local);
        self.charge(consumed, table_units, at);
        for (index, units) in &capacity.global_secondary_indexes {
            self.charge_global_index(consumed, index, *units, at);
        }
    }

    /// Record one charge of `units` against `series`.
    ///
    /// A charge of nothing is no observation; past [`MAX_PENDING_OBSERVATIONS`] the oldest
    /// goes, which at worst loses an observation nothing has published in that long.
    fn charge_series(
        &mut self,
        consumed: Consumed,
        series: Series,
        units: f64,
        at: ::std::time::SystemTime,
    ) {
        if !units.is_finite() || units <= 0.0 {
            return;
        }
        self.0.push(Observation {
            at,
            units,
            consumed,
            series,
        });
        let overflow = self.0.len().saturating_sub(MAX_PENDING_OBSERVATIONS);
        if overflow > 0 {
            self.0.drain(..overflow);
        }
    }

    /// Take everything recorded so far, leaving nothing behind: publishing is at-most-once.
    fn take(&mut self) -> Vec<Observation> {
        ::std::mem::take(&mut self.0)
    }
}

/// Count what ONE request reported consuming on ONE table into that table's series.
///
/// The surfaces that take their capacity from the table's budgets count where the capacity is
/// TAKEN ([`crate::state::Table::admit_item_write`]), because one write's figure is settled
/// right there. The surfaces whose figure is read back OUT of the stored table after the write
/// has landed — the statement surface's three write verbs, and the transactional ledgers
/// below — count it here instead, so the series carries exactly the units the response named
/// whichever surface the request came in through.
///
/// `consumed` is the half of the throughput the request is BILLED as: the kind its own
/// `ConsumedCapacity` reports the figure in.
pub fn charge_reported_capacity(
    store: &mut crate::state::ComAmazonawsDynamodbDynamodb20120810State,
    region: &str,
    name: &str,
    capacity: &crate::state::WriteCapacity,
    consumed: Consumed,
    at: ::std::time::SystemTime,
) {
    // The request has already reached this table, so the name is one this region resolves; a
    // table that is no longer there has no series to count into and is passed over rather
    // than guessed at.
    if let Ok(table) = store.table_mut(region, name) {
        table
            .pending_consumed
            .charge_itemised(consumed, capacity, at);
    }
}

/// Count what one MULTI-ITEM request reported consuming into the series of each table it
/// addressed, as the ledger it answers with itemises it.
///
/// [`charge_reported_capacity`] per table of the ledger. A TRANSACTIONAL request's figure is
/// settled only once every element has landed and the transactional factor has been applied to
/// the whole ledger ([`crate::state::TRANSACTIONAL_CAPACITY_FACTOR`]) — "transactional write
/// requests require two write capacity units" — so the ledger the response reports is what is
/// counted.
///
/// `consumed` is the half of the throughput the surface bills the request as: a transaction
/// that applies its elements spends write capacity, while a repeat under a client request token
/// applies nothing and "returns the number of read capacity units consumed in reading the
/// item", so it spends read capacity.
pub fn charge_reported_ledger(
    store: &mut crate::state::ComAmazonawsDynamodbDynamodb20120810State,
    region: &str,
    ledger: &crate::state::WriteLedger,
    consumed: Consumed,
    at: ::std::time::SystemTime,
) {
    for (name, capacity) in ledger.itemised() {
        charge_reported_capacity(store, region, name, capacity, consumed, at);
    }
}

/// Publish what the tables of this store have been charged since the last publish, into the
/// CloudWatch series each table's throughput is reported under.
///
/// Call it with NO store lock held: this takes this service's lock to drain the observations
/// and CloudWatch's to append them, one after the other. It is called from the one seam every
/// request of this service passes through, so what a request charged is published as part of
/// serving it — which is what makes an alarm on a table's utilisation, and the auto scaling
/// built on one, see the load that happened.
pub fn publish_pending_consumed<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let charged: Vec<(String, Observation)> = {
        let mut store = state.lock();
        store
            .regions
            .values_mut()
            .flat_map(|tables| tables.values_mut())
            .flat_map(|table| {
                let name = table.name.clone();
                table
                    .pending_consumed
                    .take()
                    .into_iter()
                    .map(move |observation| (name.clone(), observation))
            })
            .collect()
    };
    if charged.is_empty() {
        return;
    }
    let Some(metrics) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = metrics.lock();
    for (table_name, observation) in charged {
        let dimensions = observation.series.dimensions(&table_name);
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: observation.consumed.metric_name().to_string(),
            dimensions,
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs: observation
                .at
                .duration_since(::std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            sample_count: 1.0,
            sum: observation.units,
            minimum: observation.units,
            maximum: observation.units,
            unit: None,
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already served,
            // so nothing is failed for it; the dropped observation is reported where a
            // developer can see it.
            ::tracing::warn!(
                table = %table_name,
                metric = %observation.consumed.metric_name(),
                "the CloudWatch metric store is at capacity, so the throughput this request \
                 consumed was not counted"
            );
        }
    }
}
