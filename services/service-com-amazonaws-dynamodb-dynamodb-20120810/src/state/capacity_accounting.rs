// SPDX-License-Identifier: CC0-1.0

//! What a request CONSUMED, as `ConsumedCapacity` reports it: the capacity units a
//! read and a write of a given size take, the per-index breakdown a write reports,
//! and the item-collection size estimate a write of a local secondary index's
//! partition reports beside it.

use super::*;

/// The capacity units a write of `size` bytes consumes: one unit per 1 KB, and never
/// less than one unit.
pub fn write_capacity_units(size: i64) -> f64 {
    units_of(size, WRITE_UNIT_BYTES)
}

/// The capacity units a read of `size` bytes consumes: one unit per 4 KB when the
/// read is strongly consistent, half that when it is eventually consistent.
pub fn read_capacity_units(size: i64, consistent_read: bool) -> f64 {
    let units = units_of(size, READ_UNIT_BYTES);
    if consistent_read {
        units
    } else {
        units / 2.0
    }
}

/// How many whole units of `unit_bytes` cover `size` bytes, never fewer than one.
fn units_of(size: i64, unit_bytes: i64) -> f64 {
    let size = size.max(0);
    let partial_unit = i64::from(size % unit_bytes != 0);
    let whole_units = (size / unit_bytes + partial_unit).max(1);
    f64::from(u32::try_from(whole_units).unwrap_or(u32::MAX))
}

/// What ONE write consumed, broken down the way `ConsumedCapacity` reports it: the
/// units the table itself took, and the units each secondary index the write affected
/// took.
///
/// The per-index maps are ordered, so a write reports its indexes in one stable order
/// whatever order the table declared them in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteCapacity {
    /// The units consumed on the table itself.
    pub table: f64,
    /// The units consumed on each local secondary index the write affected.
    pub local_secondary_indexes: BTreeMap<String, f64>,
    /// The units consumed on each global secondary index the write affected.
    pub global_secondary_indexes: BTreeMap<String, f64>,
    /// The `VectorWriteRequestBytes` metered on each VECTOR index the write affected: a
    /// vector index is metered in BYTES rather than in capacity units, so its figure sits
    /// beside the breakdown above and is no part of [`Self::total`] — what writing the
    /// vector costs in capacity is charged on the item itself
    /// ([`Table::metered_item_size`]).
    pub vector_indexes: BTreeMap<String, f64>,
}

impl WriteCapacity {
    /// The aggregate `CapacityUnits`: "The total number of capacity units consumed by
    /// the operation" (`ConsumedCapacity$CapacityUnits`) — the table's units plus those
    /// of every index the write fanned out to.
    pub fn total(&self) -> f64 {
        self.local_secondary_indexes
            .values()
            .chain(self.global_secondary_indexes.values())
            .fold(self.table, |total, units| total + units)
    }
}

/// WHICH KIND of capacity an operation consumed, which decides the arm every figure it
/// reports is carried in beside the aggregate.
///
/// `ConsumedCapacity` and `Capacity` each carry an aggregate (`CapacityUnits`, "the total
/// number of capacity units consumed") and one figure per kind: `ReadCapacityUnits` is
/// "the total number of read capacity units consumed" and `WriteCapacityUnits` the write
/// equivalent. A data-plane operation spends capacity of ONE kind — a read reads and a
/// write writes — so the figure of that kind is the aggregate, and the other kind's is
/// absent because the operation consumed none of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityKind {
    /// Read capacity: what `GetItem`, `Query`, `Scan` and the transactional and batch
    /// reads consume.
    Read,
    /// Write capacity: what `PutItem`, `UpdateItem`, `DeleteItem` and the transactional
    /// and batch writes consume.
    Write,
}

impl CapacityKind {
    /// One `Capacity` line — a table's or an index's — of `units` of this kind: the
    /// aggregate and this kind's own figure, which are the same number.
    fn line(self, units: f64) -> Capacity {
        Capacity {
            capacity_units: Some(units),
            read_capacity_units: (self == Self::Read).then_some(units),
            write_capacity_units: (self == Self::Write).then_some(units),
        }
    }

    /// A `ConsumedCapacity` reporting `units` of this kind for `table_name`, with no
    /// per-table or per-index breakdown yet: the operation's aggregate and this kind's
    /// own figure, filled from [`Self::line`] so one rule decides every arm.
    fn consumed(self, table_name: &str, units: f64) -> ConsumedCapacity {
        let aggregate = self.line(units);
        ConsumedCapacity {
            table_name: Some(table_name.to_string()),
            capacity_units: aggregate.capacity_units,
            read_capacity_units: aggregate.read_capacity_units,
            write_capacity_units: aggregate.write_capacity_units,
            ..ConsumedCapacity::default()
        }
    }
}

/// The `ConsumedCapacity` a WRITE that asked for it gets back, or `None` when it asked
/// for `NONE` (or did not ask at all).
///
/// `TOTAL` reports "only the aggregate ConsumedCapacity for the operation"; `INDEXES`
/// reports "the aggregate ConsumedCapacity for the operation, together with
/// ConsumedCapacity for each table and secondary index that was accessed" (the
/// `ReturnConsumedCapacity` doc), so the `Table` and per-index breakdown is returned
/// only then — and only for the indexes the write actually affected, since an index it
/// did not reach consumed nothing on it.
///
/// Every figure is reported in `kind`'s own arm as well as in the aggregate
/// ([`CapacityKind`]). The breakdown is a WRITE's — the table's units and those of each
/// index the write fanned out to — while the arm is the kind the operation BILLED it as,
/// which is `Write` for a write that was applied and `Read` where the service bills the
/// same figures as a read (a `ClientRequestToken` replay, which reads the transaction's
/// items rather than writing them).
pub fn consumed_write_capacity(
    requested: Option<&str>,
    table_name: &str,
    capacity: &WriteCapacity,
    kind: CapacityKind,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    Some(ConsumedCapacity {
        table: detail.then(|| kind.line(capacity.table)),
        local_secondary_indexes: detail
            .then(|| capacity_breakdown(kind, &capacity.local_secondary_indexes))
            .flatten(),
        global_secondary_indexes: detail
            .then(|| capacity_breakdown(kind, &capacity.global_secondary_indexes))
            .flatten(),
        // A vector index's figure is not part of the capacity breakdown `INDEXES` asks
        // for — it is a different meter, in bytes — so it is reported whenever the
        // request asked for its consumption at all, for each index the write reached.
        vector_indexes: vector_breakdown(&capacity.vector_indexes),
        ..kind.consumed(table_name, capacity.total())
    })
}

/// The `VectorIndexes` map a write reports, or `None` when it reached no vector index —
/// an index a write did not touch metered nothing on it, so it is left out rather than
/// reported as zero.
fn vector_breakdown(
    bytes: &BTreeMap<String, f64>,
) -> Option<HashMap<String, VectorCapacity>> {
    (!bytes.is_empty()).then(|| {
        bytes
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    VectorCapacity {
                        vector_write_request_bytes: Some(*bytes),
                        // Nothing was searched, so the read figure is left out.
                        vector_search_request_bytes: None,
                    },
                )
            })
            .collect()
    })
}

/// One per-index `ConsumedCapacity` map, or `None` when no index of that kind was
/// reached at all — an index an operation did not touch consumed nothing on it, so it is
/// left out rather than reported as zero.
fn capacity_breakdown(
    kind: CapacityKind,
    units: &BTreeMap<String, f64>,
) -> Option<HashMap<String, Capacity>> {
    (!units.is_empty()).then(|| {
        units
            .iter()
            .map(|(name, units)| (name.clone(), kind.line(*units)))
            .collect()
    })
}

/// Whether a write reports item collection metrics: "If set to SIZE, the response
/// includes statistics about item collections, if any, that were modified during the
/// operation ... If set to NONE (the default), no statistics are returned"
/// (`PutItem$ReturnItemCollectionMetrics`). A value the model's
/// `ReturnItemCollectionMetrics` enum does not name is a bad request rather than a
/// silent `NONE`.
pub fn returns_item_collection_metrics(requested: Option<&str>) -> Result<bool, RequestError> {
    match requested {
        None | Some("NONE") => Ok(false),
        Some("SIZE") => Ok(true),
        Some(other) => Err(RequestError::Validation(format!(
            "ReturnItemCollectionMetrics can only be SIZE or NONE, got {other}"
        ))),
    }
}

/// A gigabyte, as `SizeEstimateRangeGB` denominates its bounds. DynamoDB's size quotas
/// are decimal ("The maximum size of any item collection is 10 GB" — "Service, Account,
/// and Table Quotas"), so a gigabyte here is 10^9 bytes.
const BYTES_PER_GB: i64 = 1_000_000_000;

/// The two-element `[lower, upper]` bound, in GB, that brackets `bytes`.
///
/// "This value is a two-element array containing a lower bound and an upper bound for
/// the estimate ... The estimate is subject to change over time; therefore, do not rely
/// on the precision or accuracy of the estimate"
/// (`ItemCollectionMetrics$SizeEstimateRangeGB`). The bracket reported here is the whole
/// gigabyte at or below the measured size and the next one above it, so the measured
/// size always lies inside the reported range and a collection under a gigabyte reports
/// `[0.0, 1.0]`.
pub fn size_estimate_range_gb(bytes: i64) -> Vec<f64> {
    let whole_gb = bytes.max(0) / BYTES_PER_GB;
    let lower = f64::from(u32::try_from(whole_gb).unwrap_or(u32::MAX));
    vec![lower, lower + 1.0]
}

/// The `ConsumedCapacity` a read SERVED FROM A SECONDARY INDEX reports, or `None` when
/// the request asked for `NONE` (or did not ask at all).
///
/// `INDEXES` asks for "the aggregate ConsumedCapacity for the operation, together with
/// ConsumedCapacity for each table and secondary index that was accessed" (the
/// `ReturnConsumedCapacity` doc): a read of an index spends its units ON THAT INDEX, so
/// the index carries the aggregate and the table's own line is zero — the read did not go
/// to the table (`Scan$Select`: a read served from the index "reads only the index and not
/// the table"). Which of the two per-index maps the index appears under is its kind.
pub fn consumed_index_capacity(
    requested: Option<&str>,
    table_name: &str,
    index: &SecondaryIndex<'_>,
    units: f64,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    let kind = CapacityKind::Read;
    let mut per_index: BTreeMap<String, f64> = BTreeMap::new();
    per_index.insert(index.name.to_string(), units);
    let breakdown = detail.then(|| capacity_breakdown(kind, &per_index)).flatten();
    Some(ConsumedCapacity {
        // The read did not go to the table, so its own line is zero rather than absent:
        // `INDEXES` asks for "each table and secondary index that was accessed", and the
        // table that holds the index is named by every line of the breakdown.
        table: detail.then(|| kind.line(0.0)),
        local_secondary_indexes: (!index.global).then(|| breakdown.clone()).flatten(),
        global_secondary_indexes: index.global.then_some(breakdown).flatten(),
        ..kind.consumed(table_name, units)
    })
}

/// The `ConsumedCapacity` a READ that asked for it gets back, or `None` when it
/// asked for `NONE` (or did not ask at all).
///
/// `INDEXES` asks for the per-table breakdown as well as the aggregate; on a base
/// table there is no index to break out, so the table's own capacity is the whole of
/// it (the `ReturnConsumedCapacity` doc: "specifying INDEXES will only return
/// ConsumedCapacity information for table(s)").
///
/// Every figure is reported in the read arm as well as the aggregate
/// ([`CapacityKind`]).
pub fn consumed_capacity(
    requested: Option<&str>,
    table_name: &str,
    units: f64,
) -> Option<ConsumedCapacity> {
    let detail = match requested {
        Some("TOTAL") => false,
        Some("INDEXES") => true,
        _ => return None,
    };
    let kind = CapacityKind::Read;
    Some(ConsumedCapacity {
        table: detail.then(|| kind.line(units)),
        ..kind.consumed(table_name, units)
    })
}
