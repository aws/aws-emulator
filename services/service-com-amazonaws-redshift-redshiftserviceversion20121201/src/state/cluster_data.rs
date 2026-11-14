// SPDX-License-Identifier: CC0-1.0

//! A cluster's DATA, which lives in the SQL engine the Redshift Data API runs
//! statements on.
//!
//! A snapshot of a provisioned cluster is not a copy of its configuration: "a
//! snapshot contains a copy of the cluster data as of this exact time"
//! (`smithy_query(query="doc", shape="Snapshot", member="SnapshotCreateTime")`),
//! and `RestoreFromClusterSnapshot` "creates a new cluster from a snapshot" — so
//! the cluster it creates serves the tables and rows the snapshot holds and none
//! of the writes the source took afterwards.
//!
//! Those tables and rows live in the in-process SQL engine the Redshift Data API
//! owns, keyed by the warehouse a statement named. So the copy is taken, held and
//! written back through that service's own seam — its bounded `capture_warehouse`
//! / `restore_warehouse` pair, which runs the copy on the blocking pool under its
//! own budget — and no SQL type crosses into this crate: the capture is opaque
//! here, exactly as it is to the Redshift Serverless control plane that snapshots
//! a namespace through the same seam.

use std::time::Duration;

/// A copy of a cluster's databases, as [`capture_cluster_data`] took it.
///
/// Opaque: nothing in this crate can read or alter a captured database.
pub type ClusterData = ::service_com_amazonaws_redshiftdata_redshiftdata::state::WarehouseCapture;

/// Why a copy of a cluster's data did not happen.
///
/// The three outcomes a bounded copy has, as the crate that owns the seam and the
/// budget states them: there is ONE of these, so this control plane and the
/// serverless one cannot drift apart on what a failed copy was.
pub type DataPlaneFailure = ::service_com_amazonaws_redshiftdata_redshiftdata::state::CopyFailure;

/// How long a request waits for a copy of a cluster's data: the Data API's own
/// budget for one copy, which is longer than the deadline the copy holds itself
/// to, so what ends a copy is its own deadline and not this request's patience.
pub const DATA_COPY_BUDGET: Duration =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::COPY_REQUEST_BUDGET;

/// How many engine pages the snapshots this store retains may hold between them.
///
/// The emulator's own bound, like every other quota in this module: a snapshot
/// keeps a copy of the cluster's databases in memory, so the number of records is
/// not on its own a bound on what they occupy.
pub const MAX_SNAPSHOT_DATA_PAGES: i64 = 16_384;

/// The warehouse a provisioned cluster's databases are addressed by in the Data
/// API's engine: the cluster itself, by `ClusterIdentifier` — the same selector a
/// statement submitted against that cluster carries.
fn cluster_target(
    cluster_identifier: String,
) -> ::service_com_amazonaws_redshiftdata_redshiftdata::state::ConnectionTarget {
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::ConnectionTarget::Cluster(
        cluster_identifier,
    )
}

/// Copy the databases of the cluster `cluster_identifier` as they stand NOW.
///
/// This is what makes a cluster snapshot a snapshot of DATA rather than of
/// configuration. A cluster no statement has ever run against captures empty,
/// which is correct: it holds no table to restore.
///
/// The copy is blocking engine work, so it runs on the blocking pool under
/// [`DATA_COPY_BUDGET`] — the bounded form the crate that owns the engine provides
/// (`capture_warehouse_bounded`), rather than a second copy of that rule here.
/// Call it with NO state guard held, so one caller's copy serialises nothing else.
pub async fn capture_cluster_data<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    cluster_identifier: String,
) -> Result<ClusterData, DataPlaneFailure> {
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::capture_warehouse_bounded(
        engine,
        vec![cluster_target(cluster_identifier)],
    )
    .await
}

/// Write `data` into the databases of the cluster `cluster_identifier`, and
/// report how many databases it materialised.
///
/// The cluster then holds exactly the databases the snapshot holds, with exactly
/// their contents, so a statement submitted against it afterwards reads the
/// restored tables. Bounded and taken off the async worker exactly as
/// [`capture_cluster_data`] is, and for the same reasons.
pub async fn restore_cluster_data<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    data: ::std::sync::Arc<ClusterData>,
    cluster_identifier: String,
) -> Result<usize, DataPlaneFailure> {
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::restore_warehouse_bounded(
        engine,
        data,
        cluster_target(cluster_identifier),
    )
    .await
}

/// Which table of a snapshot's databases a table restore moves, and what to call
/// the copy: the request `RestoreTableFromClusterSnapshot` carries, as the crate
/// that owns the engine states it.
pub type TableRestoreRequest =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::TableRestoreRequest;

/// What a table restore did, or which of the names the request carried resolved
/// to nothing — as the crate that owns the engine answers it.
pub type TableRestoreOutcome =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::TableRestored;

/// How many rows ONE table restore copies before the engine gives it up.
///
/// The bound the Data API's engine publishes for a table copy, passed to it by
/// the call below so the bound a `RestoreTableFromClusterSnapshot` is held to is
/// stated at the call it governs; a source table past it is refused with nothing
/// written (see [`crate::state::table_restore_row_limit_message`]).
pub const MAX_RESTORED_TABLE_ROWS: i64 =
    ::service_com_amazonaws_redshiftdata_redshiftdata::state::MAX_RESTORED_TABLE_ROWS;

/// Copy ONE table of `data` into the databases of the cluster
/// `cluster_identifier`.
///
/// This is what makes `RestoreTableFromClusterSnapshot` a restore of a TABLE:
/// "creates a new table from a table in an Amazon Redshift cluster snapshot", so
/// the new table holds the rows the snapshot's table held and the cluster serves
/// it from then on.
///
/// The three bounds one call is held to, all applied HERE so none of them is a
/// claim about another crate:
///
/// * the copy is blocking engine work, so it runs on the BLOCKING POOL
///   (`tokio::task::spawn_blocking`) and never on the async worker this request is
///   served on;
/// * this request waits at most [`DATA_COPY_BUDGET`] for it and then abandons it,
///   which is longer than the deadline the copy holds ITSELF to, so what ends a
///   copy is its own deadline rather than this request's patience;
/// * the copy writes at most [`MAX_RESTORED_TABLE_ROWS`] rows, which is passed in
///   as the bound rather than assumed, and a source table past it is reported as
///   too large with nothing written.
///
/// Call it with NO state guard held: it awaits.
pub async fn restore_cluster_table<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    data: ::std::sync::Arc<ClusterData>,
    cluster_identifier: String,
    request: TableRestoreRequest,
) -> Result<TableRestoreOutcome, DataPlaneFailure> {
    let engine = ::std::sync::Arc::clone(engine);
    let target = cluster_target(cluster_identifier);
    let copied = ::tokio::time::timeout(
        DATA_COPY_BUDGET,
        ::tokio::task::spawn_blocking(move || {
            ::service_com_amazonaws_redshiftdata_redshiftdata::state::restore_table(
                &*engine,
                &data,
                &target,
                &request,
                MAX_RESTORED_TABLE_ROWS,
            )
        }),
    )
    .await;
    match copied {
        Ok(Ok(Ok(outcome))) => Ok(outcome),
        // The copy itself could not be made, the task carrying it did not return,
        // or this request gave up waiting: the same three outcomes every other
        // bounded copy of this engine reports (see [`DataPlaneFailure`]).
        Ok(Ok(Err(unavailable))) => Err(DataPlaneFailure::Unavailable(unavailable)),
        Ok(Err(_panicked)) => Err(DataPlaneFailure::Lost),
        Err(_elapsed) => Err(DataPlaneFailure::GaveUp(DATA_COPY_BUDGET)),
    }
}

/// The engine pages the stored snapshots' data occupies between them, the
/// quantity [`MAX_SNAPSHOT_DATA_PAGES`] bounds.
///
/// Each CAPTURE is counted once however many snapshot records share it: a
/// `CopyClusterSnapshot` copy is another name for the same copy of the data (the
/// records hold one `Arc`), so it occupies nothing more.
pub fn retained_snapshot_data_pages(
    store: &super::ComAmazonawsRedshiftRedshiftserviceversion20121201State,
) -> i64 {
    let mut counted: ::std::collections::BTreeSet<usize> = ::std::collections::BTreeSet::new();
    store
        .snapshots
        .iter()
        .filter(|stored| counted.insert(::std::sync::Arc::as_ptr(&stored.data) as usize))
        .map(|stored| stored.data.pages())
        .sum()
}
