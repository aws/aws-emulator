// SPDX-License-Identifier: CC0-1.0

//! The PARTITION INDEXES of a Data Catalog table: the records a create registers on a table
//! and `GetPartitionIndexes` reports.
//!
//! The single home of this resource's records, reached by every handler that reads or writes
//! one, so no two handlers keep their own copy of a table's indexes. It is reached exactly as
//! [`crate::state::service_state`] reaches the catalog itself — through the SHARED engine, by
//! type ([`partition_index_store`]) — so the records one request writes are the records the
//! next request reads.
//!
//! An index is registered either WITH its table (`CreateTable$PartitionIndexes`, resolved by
//! [`resolve_indexes`]) or on a table the catalog already holds (`CreatePartitionIndex`,
//! resolved by [`resolve_index`] and added by [`PartitionIndexStore::add`]). The second case
//! is the one with a BACKFILL: the table already holds partitions, and the ones the index
//! cannot cover are what its `BackfillErrors` report ([`backfill_errors`]), which is also
//! what decides whether its build succeeds.
//!
//! The file sits with the rest of this service's state, beside
//! [`crate::state::catalog_encryption`]; it is DECLARED from `src/service.rs` (hence its
//! module path), which is the file this service owns and which is never regenerated. See the
//! declaration there for why.
//!
//! An index is a resource OF one table: "the partition indexes associated with a table"
//! (`GetPartitionIndexes`), addressed by catalog, database and table and then by its own
//! `IndexName`. A record set is therefore bound to the stored table it belongs to, including
//! the instant that table was created ([`TableIndexKey`]), and it goes when that table goes:
//! every operation that removes a table removes its records with it
//! ([`PartitionIndexStore::forget_tables`]), exactly as a table's partitions are dropped with
//! the table that holds them. So this store holds records for no more tables than the catalog
//! holds, which is what bounds it, and the table a later create puts under a recycled name is
//! a different table, with no indexes until something registers one on it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BackfillError, Column, KeySchemaElement, PartitionIndex, PartitionIndexDescriptor,
    PartitionValueList,
};

/// The `PartitionIndexStatus` an index is in while the service is still building it:
/// "CREATING: The index is being created" (`PartitionIndexDescriptor$IndexStatus`). One of
/// the modelled values carried by [`crate::ops::types::PARTITION_INDEX_STATUS_VALUES`].
pub const INDEX_STATUS_CREATING: &str = "CREATING";

/// The `PartitionIndexStatus` the clock owes an index it has finished building: "ACTIVE: The
/// index creation succeeds" (`PartitionIndexDescriptor$IndexStatus`).
pub const INDEX_STATUS_ACTIVE: &str = "ACTIVE";

/// The `PartitionIndexStatus` of an index whose build did not succeed: "FAILED: The index
/// creation fails" (`PartitionIndexDescriptor$IndexStatus`). An index registered on a table
/// whose stored partitions the index cannot cover reaches this status rather than
/// [`INDEX_STATUS_ACTIVE`], and reports WHY in its `BackfillErrors`.
pub const INDEX_STATUS_FAILED: &str = "FAILED";

/// The `BackfillErrorCode` of a stored partition whose value for an indexed key is not a
/// value of the type that partition key declares: of the modelled `BackfillErrorCode`
/// values, the one for invalid partition-type data.
pub const BACKFILL_INVALID_PARTITION_TYPE_DATA: &str = "INVALID_PARTITION_TYPE_DATA_ERROR";

/// The `BackfillErrorCode` of a stored partition that carries no value at all for an indexed
/// key: of the modelled `BackfillErrorCode` values, the one for a missing partition value.
pub const BACKFILL_MISSING_PARTITION_VALUE: &str = "MISSING_PARTITION_VALUE_ERROR";

/// How many partitions ONE `BackfillError` names.
///
/// CHOSEN, not documented: a backfill error "provides a limited number of partitions in the
/// response, so that you can fix the partitions at fault and try registering the index
/// again" (`BackfillError`), and Glue publishes no figure for that limit — so the error names
/// the first partitions at fault in the order the table holds them and stops there. The code
/// itself is reported whether or not the list was cut off, because it is the code that says
/// why the build failed.
pub const MAX_BACKFILL_ERROR_PARTITIONS: usize = 10;

/// How long an index stays [`INDEX_STATUS_CREATING`] before the clock owes it
/// [`INDEX_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: Glue publishes no duration for building a partition index, so this
/// is the emulator's own span — deliberately not zero, so a caller that reads
/// `GetPartitionIndexes` straight after registering an index still observes the index being
/// built. The due instant is stored on the record and compared with the instant of the call
/// that reads it; nothing sleeps.
pub const PARTITION_INDEX_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// The most partition indexes one table holds.
///
/// Source: the Glue documentation on partition indexes, which states that a maximum of three
/// partition indexes can be created on a table. A create that would take a table past it is
/// refused with the modelled `ResourceNumberLimitExceededException` rather than stored, so
/// this store holds no table the real service would not have.
pub const MAX_INDEXES_PER_TABLE: usize = 3;

/// Why a table cannot carry the partition indexes a request submitted, as the two different
/// failures the catalog answers them with.
///
/// The variants are the DISTINCTION the caller sees: indexes the table's own definition
/// cannot support are a bad request, while one index too many is a limit on the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRefusal {
    /// The submitted indexes do not describe something this table can index — answered with
    /// `InvalidInputException`.
    Invalid(String),
    /// The table would hold more than [`MAX_INDEXES_PER_TABLE`] indexes — answered with
    /// `ResourceNumberLimitExceededException`.
    TooMany(String),
}

impl IndexRefusal {
    /// The refusal's message, as the error it is answered with carries it.
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message) | Self::TooMany(message) => message,
        }
    }
}

/// Why a table that is already stored cannot take ONE MORE partition index.
///
/// The variants are the DISTINCTION the caller sees: a name the table already holds an index
/// under is a duplicate resource, while one index too many is a limit on the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRegisterRefusal {
    /// The table already holds an index of the submitted `IndexName` — answered with
    /// `AlreadyExistsException`.
    AlreadyExists,
    /// The table already holds [`MAX_INDEXES_PER_TABLE`] indexes — answered with
    /// `ResourceNumberLimitExceededException`.
    TooMany {
        /// How many indexes the table holds, which is what the refusal reports.
        held: usize,
    },
}

/// The refusal wordings this module answers with, each built from the REQUEST and the table's
/// own definition rather than from any one caller's values.
pub mod messages {
    use super::MAX_INDEXES_PER_TABLE;

    /// A key an index names that the table does not partition by: an index indexes "the name
    /// of a partition key" (`KeySchemaElement$Name`), so a column that is not one of the
    /// table's partition keys names nothing to index.
    pub fn key_is_not_a_partition_key(table_name: &str, index_name: &str, key: &str) -> String {
        format!(
            "Index key {key} of partition index {index_name} is not a partition key of table \
             {table_name}"
        )
    }

    /// One `IndexName` submitted twice in one request: an index is addressed by its name
    /// within its table, so two of one name name one index.
    pub fn index_named_twice(table_name: &str, index_name: &str) -> String {
        format!("Partition index {index_name} is named more than once for table {table_name}")
    }

    /// The same key named twice inside one index's `Keys`.
    pub fn key_named_twice(table_name: &str, index_name: &str, key: &str) -> String {
        format!(
            "Index key {key} is named more than once in partition index {index_name} of table \
             {table_name}"
        )
    }

    /// More indexes than a table holds.
    pub fn too_many_indexes(table_name: &str, would_hold: usize) -> String {
        format!(
            "Table {table_name} cannot have more than {MAX_INDEXES_PER_TABLE} partition indexes, \
             and {would_hold} were submitted"
        )
    }

    /// One more index on a table that already holds as many as it can.
    pub fn index_limit_reached(table_name: &str, held: usize) -> String {
        format!(
            "Table {table_name} cannot have more than {MAX_INDEXES_PER_TABLE} partition indexes, \
             and it already holds {held}"
        )
    }

    /// The keys a REGISTRATION on an existing table named that the table does not partition
    /// by, as the catalog words that refusal: the fixed sentence, then the keys at fault in
    /// the order the request named them.
    ///
    /// An index indexes "the name of a partition key" (`KeySchemaElement$Name`), so a key
    /// that is not one of the table's partition columns names nothing to index — a column of
    /// the table's `StorageDescriptor` included.
    pub fn index_keys_not_partition_columns(keys: &[&str]) -> String {
        format!(
            "IndexKeys not a part of PartitionColumns. Verify the indexKeys : [{}]",
            keys.join(", ")
        )
    }

    /// A registration naming an index the table already holds. Like the catalog's other
    /// duplicate-create refusals (`Table already exists.`), the name is not disclosed: the
    /// request carries it.
    pub const PARTITION_INDEX_ALREADY_EXISTS: &str = "Partition index already exists.";
}

/// The table one set of index records belongs to.
///
/// The CREATE INSTANT is part of the identity on purpose: a table is addressed by catalog,
/// database and name, but those name a table only while it is stored, so binding the records
/// to the incarnation that registered them is what keeps a table created under a recycled
/// name from reading another table's indexes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableIndexKey {
    /// The catalog holding the table, as the table itself records it.
    pub catalog_id: String,
    /// The table's database, as the table itself records it.
    pub database_name: String,
    /// The table's own stored name.
    pub table_name: String,
    /// `Table$CreateTime`: the instant the catalog stored this table, which a later table of
    /// the same name does not share.
    pub table_created_at: SystemTime,
}

impl TableIndexKey {
    /// The key of the records belonging to one STORED table, taken from the table itself so
    /// that a writer and a reader of the same table cannot disagree about its key.
    pub fn of(table: &crate::state::TableEntry) -> Self {
        Self {
            catalog_id: table.catalog_id.clone(),
            database_name: table.database_name.clone(),
            table_name: table.name.clone(),
            table_created_at: table.create_time,
        }
    }

    /// Whether this key addresses the same catalog, database and table name as `other`,
    /// whichever incarnation of that table each one belongs to.
    fn names_same_table(&self, other: &Self) -> bool {
        self.catalog_id == other.catalog_id
            && self.database_name == other.database_name
            && self.table_name == other.table_name
    }
}

/// ONE stored partition index: the index a create registered, with the keys it indexes and
/// the instant its build is due to be finished.
#[derive(Debug, Clone)]
pub struct PartitionIndexEntry {
    /// `PartitionIndex$IndexName` as the create sent it, which is also this entry's key
    /// within its table's records.
    pub index_name: String,
    /// The indexed keys, each resolved against the table's partition keys at create time, in
    /// the order the create named them: "a list of one or more keys, as `KeySchemaElement`
    /// structures, for the partition index" (`PartitionIndexDescriptor$Keys`).
    pub keys: Vec<KeySchemaElement>,
    /// The instant from which the clock owes this index its built status; before it, the
    /// index is still [`INDEX_STATUS_CREATING`].
    pub active_at: SystemTime,
    /// What the BACKFILL of this index over the partitions the table already held found at
    /// fault, computed when the index was registered ([`backfill_errors`]) — empty when every
    /// stored partition carries a value of the declared type for each indexed key, which is
    /// the only way the build succeeds.
    pub backfill_errors: Vec<BackfillError>,
}

impl PartitionIndexEntry {
    /// The status this index is in at `at`: the status its create put it in while the due
    /// instant is still ahead, and from the instant it is reached the status its build
    /// earned — [`INDEX_STATUS_ACTIVE`] when the backfill found nothing at fault,
    /// [`INDEX_STATUS_FAILED`] when it did.
    pub fn status(&self, at: SystemTime) -> &'static str {
        if at < self.active_at {
            INDEX_STATUS_CREATING
        } else if self.backfill_errors.is_empty() {
            INDEX_STATUS_ACTIVE
        } else {
            INDEX_STATUS_FAILED
        }
    }

    /// This index as `GetPartitionIndexes` reports it.
    ///
    /// `BackfillErrors` are "errors that can occur when registering partition indexes for an
    /// existing table" (`PartitionIndexDescriptor$BackfillErrors`) — the partitions already
    /// stored that the index cannot cover. They are what the BUILD found, so they are
    /// reported from the instant that build is due and not before: while the index is still
    /// being built there is no result to report, and an index registered over no stored
    /// partition at all has none to report ever.
    pub fn to_descriptor(&self, at: SystemTime) -> PartitionIndexDescriptor {
        PartitionIndexDescriptor {
            index_name: self.index_name.clone(),
            keys: self.keys.clone(),
            index_status: self.status(at).to_string(),
            backfill_errors: (at >= self.active_at && !self.backfill_errors.is_empty())
                .then(|| self.backfill_errors.clone()),
        }
    }
}

/// Every table's partition-index records, keyed by the table they belong to.
///
/// BOUNDED by the catalog itself: a record set is written by the create of one table and
/// removed by whatever removes that table ([`Self::forget_tables`], called from every
/// operation that drops a table, a database or a catalog), so the store holds records for no
/// more tables than the catalog holds tables — which the catalog's own store-wide ceiling
/// bounds ([`crate::state::MAX_STORED_TABLES`]).
///
/// Ordered by index name within a table, so a reader walks one table's indexes in one
/// deterministic order and a continuation token resumes against that same order.
#[derive(Debug, Default)]
pub struct PartitionIndexStore {
    tables: BTreeMap<TableIndexKey, BTreeMap<String, PartitionIndexEntry>>,
}

impl PartitionIndexStore {
    /// The indexes of one table as `GetPartitionIndexes` reports them at `at`, in `IndexName`
    /// order. A table nothing has registered an index on has none.
    pub fn descriptors(
        &self,
        table: &TableIndexKey,
        at: SystemTime,
    ) -> Vec<PartitionIndexDescriptor> {
        self.tables
            .get(table)
            .into_iter()
            .flat_map(BTreeMap::values)
            .map(|index| index.to_descriptor(at))
            .collect()
    }

    /// Record the indexes a create registered on ONE table.
    ///
    /// The records of any EARLIER table of the same name go with it. Those records are
    /// already removed by the delete that dropped that table ([`Self::forget_tables`]); this
    /// is the guard that keeps the invariant — at most one record set per catalog, database
    /// and table name — true of this store itself, whatever reached it before.
    pub fn record(&mut self, table: TableIndexKey, indexes: Vec<PartitionIndexEntry>) {
        self.tables.retain(|held, _| !held.names_same_table(&table));
        if indexes.is_empty() {
            return;
        }
        self.tables.insert(
            table,
            indexes
                .into_iter()
                .map(|index| (index.index_name.clone(), index))
                .collect(),
        );
    }

    /// Register ONE MORE index on a table that is already stored, or say why the table
    /// cannot take it.
    ///
    /// An index is addressed by its `IndexName` within its table, so a name the table
    /// already holds an index under is a DUPLICATE rather than an overwrite — whatever status
    /// that index reached — and a table takes at most [`MAX_INDEXES_PER_TABLE`] of them,
    /// counting every index it already holds. The records of any earlier table of the same
    /// name go, exactly as [`Self::record`] drops them.
    pub fn add(
        &mut self,
        table: TableIndexKey,
        index: PartitionIndexEntry,
    ) -> Result<(), IndexRegisterRefusal> {
        // Decided BEFORE anything is written, so a registration the table cannot take
        // leaves this store exactly as it was.
        if let Some(refusal) = self.refusal_for(&table, &index.index_name) {
            return Err(refusal);
        }
        self.tables
            .retain(|held, _| !held.names_same_table(&table) || *held == table);
        self.tables
            .entry(table)
            .or_default()
            .insert(index.index_name.clone(), index);
        Ok(())
    }

    /// Why `table` cannot take one more index named `index_name`, or `None` when it can.
    ///
    /// The decision [`Self::add`] makes, answered WITHOUT the record to store, so a caller
    /// can learn a registration is refused before it does the work of building that record —
    /// and one place decides it either way.
    pub fn refusal_for(
        &self,
        table: &TableIndexKey,
        index_name: &str,
    ) -> Option<IndexRegisterRefusal> {
        let held = self.tables.get(table);
        if held.is_some_and(|held| held.contains_key(index_name)) {
            return Some(IndexRegisterRefusal::AlreadyExists);
        }
        let count = held.map_or(0, BTreeMap::len);
        (count >= MAX_INDEXES_PER_TABLE).then_some(IndexRegisterRefusal::TooMany { held: count })
    }

    /// Forget the records of every table in `tables`: the indexes of a table that is no
    /// longer stored.
    ///
    /// An index is a resource OF its table, so a delete that drops a table — on its own, in a
    /// batch, or as the cascade of dropping its database or its catalog — drops the table's
    /// indexes too, and this store keeps nothing for a table the catalog no longer holds.
    pub fn forget_tables<'a>(
        &mut self,
        tables: impl IntoIterator<Item = &'a crate::state::TableEntry>,
    ) {
        for table in tables {
            self.tables.remove(&TableIndexKey::of(table));
        }
    }

    /// Move the records of a renamed database's tables to the name those tables now record.
    ///
    /// A renamed database takes its tables with it, and each table records the database it
    /// belongs to ([`TableIndexKey`] is read off the stored table), so the records have to
    /// move with them: an index belongs to its table, and renaming the database around it
    /// changes neither the table nor the indexes registered on it.
    pub fn rename_database(&mut self, catalog_id: &str, from: &str, to: &str) {
        if from == to {
            return;
        }
        let moved: Vec<TableIndexKey> = self
            .tables
            .keys()
            .filter(|held| held.catalog_id == catalog_id && held.database_name == from)
            .cloned()
            .collect();
        for key in moved {
            if let Some(records) = self.tables.remove(&key) {
                self.tables.insert(
                    TableIndexKey {
                        database_name: to.to_string(),
                        ..key
                    },
                    records,
                );
            }
        }
    }
}

/// This service's partition-index records on the SHARED engine, or `None` when the engine
/// mounted under this service keeps no state at all.
///
/// Resolved by type, exactly as [`crate::state::service_state`] resolves the Data Catalog
/// itself, so every handler that touches an index reaches the one set of records.
pub fn partition_index_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<PartitionIndexStore>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<PartitionIndexStore>)
}

/// Resolve the partition indexes a create submitted against the partition keys of the table
/// it is registering them on, as the records to store — or the reason the table cannot carry
/// them.
///
/// RULE: an index indexes the table's PARTITION keys ("the name of a partition key",
/// `KeySchemaElement$Name`), so every key an index names has to be one of the table's own
/// partition keys, each index of a table is addressed by a name of its own, and a table takes
/// at most [`MAX_INDEXES_PER_TABLE`] of them. The TYPE the stored record carries for a key is
/// the type that partition key declares, so a read reports the table's own column type and
/// never one this code picked.
///
/// `registered_at` is the instant the create is served; the records it returns are due
/// [`INDEX_STATUS_ACTIVE`] one [`PARTITION_INDEX_SETTLES_AFTER`] after it.
///
/// BOUNDED by the table's own definition, whatever lists the request carries. The partition
/// keys are indexed by name ONCE, so resolving a key is a lookup and not a walk; and because
/// every key has to be a partition key this index has not named yet, the walk over one
/// index's keys stops at the first key that is not — at most one step past the number of
/// partition keys the table declares. With at most [`MAX_INDEXES_PER_TABLE`] indexes to
/// resolve, the work is linear in that definition and never the product of the two submitted
/// lists.
pub fn resolve_indexes(
    table_name: &str,
    indexes: &[PartitionIndex],
    partition_keys: &[Column],
    registered_at: SystemTime,
) -> Result<Vec<PartitionIndexEntry>, IndexRefusal> {
    if indexes.len() > MAX_INDEXES_PER_TABLE {
        return Err(IndexRefusal::TooMany(messages::too_many_indexes(
            table_name,
            indexes.len(),
        )));
    }

    // The table's partition keys by name: built once, from the definition about to be stored,
    // so no key resolution walks them. The FIRST column of a name wins, as a walk looking for
    // that name would have found.
    let partition_keys_by_name = partition_keys_by_name(partition_keys);

    let mut resolved: Vec<PartitionIndexEntry> = Vec::with_capacity(indexes.len());
    for index in indexes {
        if resolved
            .iter()
            .any(|held| held.index_name == index.index_name)
        {
            return Err(IndexRefusal::Invalid(messages::index_named_twice(
                table_name,
                &index.index_name,
            )));
        }
        // Grown by what this index actually indexes, which is bounded by the table's own
        // partition keys — never reserved from the length of the submitted list.
        let mut keys: Vec<KeySchemaElement> = Vec::new();
        let mut named: HashSet<&str> = HashSet::new();
        for key in &index.keys {
            if !named.insert(key.as_str()) {
                return Err(IndexRefusal::Invalid(messages::key_named_twice(
                    table_name,
                    &index.index_name,
                    key,
                )));
            }
            let Some(partition_key) = partition_keys_by_name.get(key.as_str()) else {
                return Err(IndexRefusal::Invalid(
                    messages::key_is_not_a_partition_key(table_name, &index.index_name, key),
                ));
            };
            keys.push(KeySchemaElement {
                name: partition_key.name.clone(),
                // `Column$Type` is optional and `KeySchemaElement$Type` is a
                // `ColumnTypeString`, whose modelled length starts at zero: a partition key
                // that declares no type is indexed under none, rather than under a type
                // nothing in the table says it has.
                r#type: partition_key.r#type.clone().unwrap_or_default(),
            });
        }
        resolved.push(PartitionIndexEntry {
            index_name: index.index_name.clone(),
            keys,
            // Checked, as every other due instant in this crate is: an instant the clock
            // cannot represent leaves the index due its ACTIVE status straight away rather
            // than taking the process down.
            active_at: registered_at
                .checked_add(PARTITION_INDEX_SETTLES_AFTER)
                .unwrap_or(registered_at),
            // An index registered WITH its table is registered over a table that holds no
            // partition yet, so its build has nothing to backfill and nothing to report.
            backfill_errors: Vec::new(),
        });
    }
    Ok(resolved)
}

/// The table's partition keys by name, so resolving a key is a lookup and not a walk.
///
/// The FIRST column of a name wins, as a walk looking for that name would have found.
fn partition_keys_by_name(partition_keys: &[Column]) -> HashMap<&str, &Column> {
    let mut by_name: HashMap<&str, &Column> = HashMap::new();
    for column in partition_keys {
        by_name.entry(column.name.as_str()).or_insert(column);
    }
    by_name
}

/// WHERE each partition key's value sits in a partition's value list, by key name: a
/// partition states one value per partition key, "ordered in the same order as the partition
/// keys" (`PartitionInput$Values`).
///
/// Built once per backfill, so finding a key's value is a lookup and not a walk. The FIRST
/// column of a name wins, as [`partition_keys_by_name`] resolves the same name.
fn partition_key_positions(partition_keys: &[Column]) -> HashMap<&str, usize> {
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for (position, column) in partition_keys.iter().enumerate() {
        by_name.entry(column.name.as_str()).or_insert(position);
    }
    by_name
}

/// Resolve ONE partition index a registration submitted on a table the catalog already
/// holds, as the record to store — or the reason the table cannot carry it.
///
/// The same RULE [`resolve_indexes`] applies to the indexes a table is created with, for the
/// one index a later registration adds: every key it names has to be one of the table's own
/// partition keys, named once, and the type the record carries for a key is the type that
/// partition key declares. What differs is the REFUSAL the caller sees, because the catalog
/// words a registration against an existing table's partition columns as such
/// ([`messages::index_keys_not_partition_columns`]), naming every key at fault rather than
/// the first one; whether the table can hold another index at all is the store's answer
/// ([`PartitionIndexStore::add`]).
///
/// `registered_at` is the instant the registration is served; the record it returns is due
/// its built status one [`PARTITION_INDEX_SETTLES_AFTER`] after it.
///
/// BOUNDED by the request and the table's own definition: the partition keys are indexed by
/// name once, so every key is one lookup, and the keys the refusal names are the submitted
/// keys themselves — linear in the request, never a product of the two lists.
pub fn resolve_index(
    table_name: &str,
    index: &PartitionIndex,
    partition_keys: &[Column],
    registered_at: SystemTime,
) -> Result<PartitionIndexEntry, IndexRefusal> {
    let partition_keys_by_name = partition_keys_by_name(partition_keys);

    let mut named: HashSet<&str> = HashSet::new();
    for key in &index.keys {
        if !named.insert(key.as_str()) {
            return Err(IndexRefusal::Invalid(messages::key_named_twice(
                table_name,
                &index.index_name,
                key,
            )));
        }
    }
    // Every key at fault, in the order the request named them: the refusal names the keys to
    // verify, so it names all of them and not just the first.
    let not_partition_columns: Vec<&str> = index
        .keys
        .iter()
        .map(String::as_str)
        .filter(|key| !partition_keys_by_name.contains_key(key))
        .collect();
    if !not_partition_columns.is_empty() {
        return Err(IndexRefusal::Invalid(
            messages::index_keys_not_partition_columns(&not_partition_columns),
        ));
    }

    Ok(PartitionIndexEntry {
        index_name: index.index_name.clone(),
        keys: index
            .keys
            .iter()
            .filter_map(|key| partition_keys_by_name.get(key.as_str()))
            .map(|partition_key| KeySchemaElement {
                name: partition_key.name.clone(),
                // See [`resolve_indexes`]: a partition key that declares no type is indexed
                // under none.
                r#type: partition_key.r#type.clone().unwrap_or_default(),
            })
            .collect(),
        // Checked, as every other due instant in this crate is.
        active_at: registered_at
            .checked_add(PARTITION_INDEX_SETTLES_AFTER)
            .unwrap_or(registered_at),
        // Filled by the caller from the partitions the table already holds
        // ([`backfill_errors`]), which is the only thing a registration on an existing table
        // has to backfill over.
        backfill_errors: Vec::new(),
    })
}

/// What the BACKFILL of one index over the partitions a table already holds finds at fault.
///
/// RULE: registering an index on an existing table indexes the partitions already stored, so
/// a stored partition whose value for an indexed key is not a value of the type that
/// partition key declares, or which carries no value for it at all, is a partition the index
/// cannot cover — and the registration reports it, under the `BackfillErrorCode` that says
/// which of the two it is, with the partitions at fault named by their `Values`
/// (`BackfillError`). A table with no such partition has nothing at fault, which is what
/// lets the build succeed.
///
/// `keys` are the index's resolved keys (each carrying the type its partition key declares),
/// `partition_keys` the table's partition keys in the order a partition's values are given in
/// ("ordered in the same order as the partition keys", `PartitionInput$Values`), and
/// `partitions` the value lists of the partitions the table holds, in the order it holds
/// them. One partition is reported ONCE, under the first fault found walking the index's keys
/// in order, so a single partition cannot be counted twice.
///
/// BOUNDED by STORED state, not by this request: the partitions walked are the table's own,
/// capped per table by [`crate::state::MAX_PARTITIONS_PER_TABLE`] and store-wide by
/// [`crate::state::MAX_STORED_PARTITIONS`], and `keys` are DISTINCT partition keys of that
/// same table ([`resolve_index`] refuses anything else), so there are no more of them than the
/// stored definition declares. Each key's value is found by an O(1) lookup into a position
/// map built once from that definition — never a walk per key — and at most
/// [`MAX_BACKFILL_ERROR_PARTITIONS`] partitions are kept per code however many are at fault.
pub fn backfill_errors<'a>(
    keys: &[KeySchemaElement],
    partition_keys: &[Column],
    partitions: impl IntoIterator<Item = &'a [String]>,
) -> Vec<BackfillError> {
    // Which of a partition's values carries each indexed key: a partition states one value
    // per partition key, in the partition-key order, so the key's POSITION among the table's
    // partition keys is where its value is. Indexed by name ONCE, so resolving a key is a
    // lookup and not a walk.
    let positions_by_name = partition_key_positions(partition_keys);
    let positions: Vec<Option<usize>> = keys
        .iter()
        .map(|key| positions_by_name.get(key.name.as_str()).copied())
        .collect();

    // Ordered by code, so a registration over the same table reports the same errors in the
    // same order however the partitions were stored.
    let mut at_fault: BTreeMap<&'static str, Vec<PartitionValueList>> = BTreeMap::new();
    for values in partitions {
        if let Some(code) = first_backfill_fault(keys, &positions, values) {
            let reported = at_fault.entry(code).or_default();
            if reported.len() < MAX_BACKFILL_ERROR_PARTITIONS {
                reported.push(PartitionValueList {
                    values: values.to_vec(),
                });
            }
        }
    }

    at_fault
        .into_iter()
        .map(|(code, partitions)| BackfillError {
            code: Some(code.to_string()),
            partitions: Some(partitions),
        })
        .collect()
}

/// The `BackfillErrorCode` ONE stored partition is at fault under, walking the index's keys
/// in order — or `None` when the index covers it.
fn first_backfill_fault(
    keys: &[KeySchemaElement],
    positions: &[Option<usize>],
    values: &[String],
) -> Option<&'static str> {
    for (key, position) in keys.iter().zip(positions) {
        // A partition states one value per partition key of its table, so a key with no
        // position, or a position past this partition's values, is a key this partition
        // states no value for.
        let value = position.and_then(|position| values.get(position));
        match value {
            None => return Some(BACKFILL_MISSING_PARTITION_VALUE),
            Some(value) if value.trim().is_empty() => {
                return Some(BACKFILL_MISSING_PARTITION_VALUE)
            }
            Some(value) if !is_value_of_type(value, &key.r#type) => {
                return Some(BACKFILL_INVALID_PARTITION_TYPE_DATA)
            }
            Some(_) => {}
        }
    }
    None
}

/// Whether `value` — a partition value, which is text — is a value OF the type a column
/// declares.
///
/// `declared_type` is `Column$Type`, "the data type of the column", which a Data Catalog
/// table states in Hive type syntax (`int`, `bigint`, `string`, `varchar(64)`,
/// `decimal(10,2)`, `timestamp`). The parameter list of a parameterised name does not change
/// which values the type holds here, so it is cut off before the name is matched, and the
/// match ignores case because a declared type is client-written text.
///
/// A TEXT type holds every value, and so does a type nothing here defines — a complex type
/// (`array`, `map`, `struct`, `uniontype`) or a name the catalog does not define — because
/// nothing then contradicts the value. Every other type holds exactly the values its own
/// spelling admits, the width of an integer type included.
pub fn is_value_of_type(value: &str, declared_type: &str) -> bool {
    let value = value.trim();
    match crate::state::base_hive_type(declared_type).as_str() {
        "boolean" => value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false"),
        "tinyint" | "byte" => value.parse::<i8>().is_ok(),
        "smallint" | "short" => value.parse::<i16>().is_ok(),
        "int" | "integer" => value.parse::<i32>().is_ok(),
        "bigint" | "long" => value.parse::<i64>().is_ok(),
        "float" | "real" => value.parse::<f32>().is_ok_and(f32::is_finite),
        "double" => value.parse::<f64>().is_ok_and(f64::is_finite),
        "decimal" | "numeric" => is_decimal(value),
        "date" => is_date(value),
        "timestamp" => crate::state::hive_instant_seconds(value).is_some(),
        _ => true,
    }
}

/// Whether `value` spells a fixed-point number: an optional sign, then digits with at most
/// one point among them and at least one digit.
fn is_decimal(value: &str) -> bool {
    let digits = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    !(whole.is_empty() && fraction.is_empty())
        && whole
            .chars()
            .chain(fraction.chars())
            .all(|character| character.is_ascii_digit())
}

/// Whether `value` spells a Hive DATE: `YYYY-MM-DD`, and a day the calendar holds.
///
/// The day itself is read by [`crate::state::hive_instant_seconds`], this crate's one reading
/// of a Hive date or timestamp; a date states no time of day.
fn is_date(value: &str) -> bool {
    !value.contains(['T', ' ', ':']) && crate::state::hive_instant_seconds(value).is_some()
}
