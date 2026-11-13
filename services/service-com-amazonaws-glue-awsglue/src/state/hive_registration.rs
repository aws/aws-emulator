// SPDX-License-Identifier: CC0-1.0

//! Registering a table in the Data Catalog the way a HIVE METASTORE CLIENT registers one:
//! the members the catalog materialises for such a table rather than leaving unknown.
//!
//! A table registered through a Hive metastore client — which is how a Hive
//! `CREATE EXTERNAL TABLE` reaches this catalog — is defined by a WHOLE Hive table object,
//! so every collection of that definition the statement declares nothing for is stored
//! EMPTY rather than absent, and the member that records when the definition was last
//! written by DDL is stored with it. `Table$PartitionKeys` says so for the partition keys:
//! "When you create a table used by Amazon Athena, and you do not specify any
//! `partitionKeys`, you must at least set the value of `partitionKeys` to an empty list",
//! and the same holds for the collections of its storage descriptor
//! (`StorageDescriptor$BucketColumns` "a list of reducer grouping columns, clustering
//! columns, and bucketing columns in the table", `StorageDescriptor$Parameters` "the
//! user-supplied properties in key-value form", `StorageDescriptor$SkewedInfo` "the
//! information about values that appear frequently in a column") and for
//! `Table$LastAccessTime`, "the last time that the table was accessed. This is usually
//! taken from HDFS" — a table nothing has read yet has been accessed at the epoch.
//!
//! So a client that round-trips such a table (a read followed by a create or an update)
//! reads an empty list or map where there is nothing, never a null it has to tell apart
//! from "unknown": iterating the partition keys of an unpartitioned table loops zero times.
//!
//! This is what a HIVE registration stores, not what every catalog write stores: a
//! `CreateTable` through the catalog's own API stores the definition its caller submitted
//! and nothing else, so the two are kept apart here rather than in the projection every
//! read goes through ([`super::to_storage_descriptor`]).
//!
//! The two collections that projection already reports definitely for EVERY table
//! (`StorageDescriptor$Columns` and `StorageDescriptor$SortColumns`) are stored here as
//! well, because what a Hive registration holds is a whole table object: a reader of the
//! STORE — this crate's own column statistics and partition writers, and the query engine
//! that resolves a relation through it — then sees the same definite lists a catalog read
//! reports, rather than depending on the projection to supply them.

use ::std::collections::HashMap;
use ::std::time::SystemTime;

use crate::ops::types::{SkewedInfo, StorageDescriptor, TableInput};

/// The table property Hive stamps on a table every time DDL writes its definition: the
/// instant of that write, in seconds from the epoch.
pub const HIVE_LAST_DDL_TIME_PARAMETER: &str = "transient_lastDdlTime";

/// The `LastAccessTime` of a table nothing has read yet, in seconds from the epoch.
///
/// The member is "usually taken from HDFS" (`Table$LastAccessTime`), where a freshly
/// created table carries no access of its own, so the registration records the epoch
/// itself rather than the instant of the write — which is what `CreateTime` records.
pub const HIVE_UNACCESSED_SECONDS: i64 = 0;

/// Fill in everything a HIVE REGISTRATION of `input` materialises, for a definition read
/// off a DDL statement performed at `now`.
///
/// Only the members the statement declared NOTHING for are filled: a declared partition
/// key list, a declared descriptor parameter and a declared DDL time are left exactly as
/// the statement wrote them.
pub fn materialise_hive_registration(input: &mut TableInput, now: SystemTime) {
    // An unpartitioned table's key list is EMPTY, which is the one the member's own
    // documentation tells an Athena client to store.
    if input.partition_keys.is_none() {
        input.partition_keys = Some(Vec::new());
    }
    if input.last_access_time.is_none() {
        input.last_access_time = Some(::aws_smithy_types::DateTime::from_secs(
            HIVE_UNACCESSED_SECONDS,
        ));
    }
    input
        .parameters
        .get_or_insert_with(HashMap::new)
        .entry(HIVE_LAST_DDL_TIME_PARAMETER.to_string())
        .or_insert_with(|| hive_ddl_time(now));
    materialise_hive_storage(
        input
            .storage_descriptor
            .get_or_insert_with(StorageDescriptor::default),
    );
}

/// Fill in the collections a Hive registration materialises on a table's STORAGE: the
/// bucketing and sort of an unbucketed table, the descriptor's own properties, and the
/// skew of a table no value is recorded as skewed in.
fn materialise_hive_storage(storage: &mut StorageDescriptor) {
    if storage.columns.is_none() {
        storage.columns = Some(Vec::new());
    }
    if storage.bucket_columns.is_none() {
        storage.bucket_columns = Some(Vec::new());
    }
    if storage.sort_columns.is_none() {
        storage.sort_columns = Some(Vec::new());
    }
    if storage.parameters.is_none() {
        storage.parameters = Some(HashMap::new());
    }
    let skewed = storage.skewed_info.get_or_insert_with(SkewedInfo::default);
    if skewed.skewed_column_names.is_none() {
        skewed.skewed_column_names = Some(Vec::new());
    }
    if skewed.skewed_column_values.is_none() {
        skewed.skewed_column_values = Some(Vec::new());
    }
    if skewed.skewed_column_value_location_maps.is_none() {
        skewed.skewed_column_value_location_maps = Some(HashMap::new());
    }
}

/// The value [`HIVE_LAST_DDL_TIME_PARAMETER`] carries for a definition written at `now`:
/// the instant in seconds from the epoch, as Hive writes it.
fn hive_ddl_time(now: SystemTime) -> String {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
