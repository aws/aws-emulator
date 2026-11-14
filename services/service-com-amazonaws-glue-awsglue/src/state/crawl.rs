// SPDX-License-Identifier: CC0-1.0

//! What a CRAWL is: the plan a crawler's definition states, what a walk of its data stores
//! discovered, and how that discovery is written into the Data Catalog.
//!
//! A crawl "populates the Glue Data Catalog with tables": it reads the data stores a
//! crawler's `Targets` name and, for each dataset it finds, writes one table whose columns
//! are the ones the data carries and whose partition keys are the folder levels below the
//! dataset's root, plus one partition per distinct value of those levels. WHERE the data is
//! read from and HOW a dataset's schema is inferred live beside this
//! ([`super::crawl_source`]); this module holds the records, the catalog write and the
//! lifecycle.
//!
//! The walk is performed where the crawl is STARTED, with no store guard held, and what it
//! discovered is written to the catalog under the guard that records the crawl — so a crawl
//! writes exactly the tables its own reading produced. The crawler's run state is what the
//! CLOCK owes: it leaves [`CRAWLER_STATE_READY`] when the crawl starts and is owed its way
//! back [`CRAWL_SETTLES_AFTER`] later, which is when the crawl's `LastCrawl` and its table
//! counts become readable.

use super::*;

/// How long a crawler stays in [`CRAWLER_STATE_RUNNING`] before the clock owes it its way
/// back to [`CRAWLER_STATE_READY`].
///
/// CHOSEN, not an AWS duration: a real crawl takes as long as the data takes to walk, and
/// Glue publishes no figure for it. Deliberately not zero, so a caller that reads the
/// crawler straight after starting it still observes the crawl in flight and the refusals
/// that go with one. The due instant is stored on the crawler and materialised by whichever
/// later call reads it; nothing sleeps.
pub const CRAWL_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// `LastCrawlInfo$Status` of a crawl that read its targets and wrote what it found: of the
/// modelled `LastCrawlStatus` (`SUCCEEDED | CANCELLED | FAILED`), the one that names a
/// finished crawl.
pub const LAST_CRAWL_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// `LastCrawlInfo$Status` of a crawl that could not be carried out, carrying the reason in
/// `LastCrawlInfo$ErrorMessage`. See [`LAST_CRAWL_STATUS_SUCCEEDED`].
pub const LAST_CRAWL_STATUS_FAILED: &str = "FAILED";

/// The third modelled `LastCrawlStatus` (`SUCCEEDED | CANCELLED | FAILED`): the status of a
/// crawl that was cut short rather than finishing or failing. Named here so the vocabulary a
/// crawl's status is published in ([`crawl_event_state`]) is the modelled one in full.
pub const LAST_CRAWL_STATUS_CANCELLED: &str = "CANCELLED";

/// `SchemaChangePolicy$UpdateBehavior` a crawl follows when the crawler states none: of the
/// modelled `UpdateBehavior` (`LOG | UPDATE_IN_DATABASE`), the one that writes the changed
/// schema into the catalog, which is the behaviour Glue applies to a crawler that states no
/// policy.
pub const UPDATE_BEHAVIOR_UPDATE_IN_DATABASE: &str = "UPDATE_IN_DATABASE";

/// See [`UPDATE_BEHAVIOR_UPDATE_IN_DATABASE`]: the behaviour that leaves the stored table
/// alone and only records that the schema changed.
pub const UPDATE_BEHAVIOR_LOG: &str = "LOG";

/// `SchemaChangePolicy$DeleteBehavior` a crawl follows when the crawler states none: of the
/// modelled `DeleteBehavior` (`LOG | DELETE_FROM_DATABASE | DEPRECATE_IN_DATABASE`), the one
/// that marks a table whose data is gone as deprecated and keeps it, which is the behaviour
/// Glue applies to a crawler that states no policy.
pub const DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE: &str = "DEPRECATE_IN_DATABASE";

/// See [`DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE`]: the behaviour that removes the table whose
/// data the crawl no longer found.
pub const DELETE_BEHAVIOR_DELETE_FROM_DATABASE: &str = "DELETE_FROM_DATABASE";

/// The table parameter a crawl stamps every table it wrote with, holding the name of the
/// crawler that wrote it. It is how a later crawl of the same crawler tells the tables it
/// owns from the ones a client or another crawler created, which is what the delete
/// behaviour is applied to.
pub const UPDATED_BY_CRAWLER_PARAMETER: &str = "UPDATED_BY_CRAWLER";

/// The table parameter a crawl stamps a table it no longer found with, under
/// [`DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE`]: it holds the crawler's name, so a reader can
/// tell which crawler deprecated the table.
pub const DEPRECATED_BY_CRAWLER_PARAMETER: &str = "DEPRECATED_BY_CRAWLER";

/// The table parameter holding the format a crawl classified the dataset's objects as —
/// `csv` or `json` here ([`super::crawl_source`]). Every engine that reads a crawled table
/// keys its reader off this property.
pub const CLASSIFICATION_PARAMETER: &str = "classification";

/// The value [`CLASSIFICATION_PARAMETER`] carries for a dataset of delimited text.
pub const CLASSIFICATION_CSV: &str = "csv";

/// The value [`CLASSIFICATION_PARAMETER`] carries for a dataset whose rows are one JSON
/// document each.
pub const CLASSIFICATION_JSON: &str = "json";

/// The table parameter holding how many objects the crawl counted behind the table.
pub const OBJECT_COUNT_PARAMETER: &str = "objectCount";

/// The table parameter holding how many bytes those objects hold together.
pub const SIZE_KEY_PARAMETER: &str = "sizeKey";

/// The table parameter holding the compression of the dataset's objects.
pub const COMPRESSION_TYPE_PARAMETER: &str = "compressionType";

/// The value [`COMPRESSION_TYPE_PARAMETER`] carries for a dataset of plain objects: this
/// crawl classifies no compressed object, so the objects it wrote a table for are
/// uncompressed.
pub const UNCOMPRESSED: &str = "none";

/// The table parameter saying what kind of store the data sits in.
pub const TYPE_OF_DATA_PARAMETER: &str = "typeOfData";

/// The value [`TYPE_OF_DATA_PARAMETER`] carries for data that sits in objects of an object
/// store, which is what an Amazon S3 target holds.
pub const TYPE_OF_DATA_FILE: &str = "file";

/// The table parameter holding the field delimiter of a delimited-text dataset.
pub const DELIMITER_PARAMETER: &str = "delimiter";

/// The table parameter holding how many leading lines of each object are a header rather
/// than data — the same property a reader of the table consults
/// ([`super::column_statistics_source`] reads it to skip them).
pub const SKIP_HEADER_LINE_COUNT_PARAMETER: &str = "skip.header.line.count";

/// The two table parameters recording the version of the schema representation a crawl
/// wrote, which every Glue crawler stamps on the tables it writes.
pub const SCHEMA_DESERIALIZER_VERSION_PARAMETER: &str = "CrawlerSchemaDeserializerVersion";
/// See [`SCHEMA_DESERIALIZER_VERSION_PARAMETER`].
pub const SCHEMA_SERIALIZER_VERSION_PARAMETER: &str = "CrawlerSchemaSerializerVersion";
/// The value both schema-version parameters carry.
pub const CRAWLER_SCHEMA_VERSION: &str = "1.0";

/// `Table$TableType` of a table a crawl wrote: the data stays in the store the crawler read
/// it from and the catalog holds only its metadata, which is what an external table is.
pub const CRAWLED_TABLE_TYPE: &str = "EXTERNAL_TABLE";

/// `StorageDescriptor$InputFormat` of a dataset whose objects are LINES OF TEXT, which is
/// the only layout this crawl classifies — the Hadoop text input format, the one every
/// Hive-style delimited or JSON-per-line table is stored under.
pub const TEXT_INPUT_FORMAT: &str = "org.apache.hadoop.mapred.TextInputFormat";

/// `StorageDescriptor$OutputFormat` that goes with [`TEXT_INPUT_FORMAT`].
pub const TEXT_OUTPUT_FORMAT: &str = "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat";

/// `SerDeInfo$SerializationLibrary` of a delimited-text dataset.
pub const LAZY_SIMPLE_SERDE: &str = "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe";

/// `SerDeInfo$SerializationLibrary` of a dataset whose rows are one JSON document each.
pub const JSON_SERDE: &str = "org.openx.data.jsonserde.JsonSerDe";

/// The serde parameter carrying the field delimiter of a delimited-text dataset, the name
/// the Hive serde itself reads it under.
pub const FIELD_DELIMITER_SERDE_PARAMETER: &str = "field.delim";

/// `Column$Type` of a partition key a crawl writes: a partition value is a path segment, so
/// it is text.
pub const PARTITION_KEY_TYPE: &str = "string";

/// How a partition key is named when the folder level it stands for is not a Hive-style
/// `key=value` segment: `partition_0`, `partition_1`, … by the level's depth, which is how
/// Glue names a partition key it cannot read a name off the path for.
pub const UNNAMED_PARTITION_KEY_PREFIX: &str = "partition_";

/// How many finished crawls one crawler keeps the duration of, for the median runtime a
/// metrics read reports.
///
/// CHOSEN: the record is resident memory and a crawl is reachable by any caller, so the
/// history one crawler holds is bounded here; the oldest duration is dropped first.
pub const MAX_CRAWL_DURATIONS: usize = 20;

/// `LastCrawlInfo$ErrorMessage` of a crawl of a crawler that names no target database.
///
/// A crawl WRITES the tables it discovers into a database, so a crawler with no
/// `DatabaseName` and no database on its targets has nowhere to put them: the crawl is
/// carried out and fails. Whole fixed text: it names no value of the crawler, because the
/// crawler it is about is the one the caller started.
pub const CRAWL_WITHOUT_TARGET_DATABASE: &str =
    "The crawler specifies no target database, so no table could be created for the data it \
     crawled.";

/// `LastCrawlInfo$ErrorMessage` of a crawl whose output the Data Catalog could not take: a
/// table or a partition the crawl FOUND is past one of the store's ceilings.
///
/// A crawl that could not write what it found is a crawl that failed, not one whose success
/// reports a catalog missing part of the data — the records it did write stay, the way a real
/// crawl's do.
pub const CRAWL_PAST_A_STORE_CEILING: &str =
    "The Data Catalog could not take every table and partition the crawl found, so the crawl \
     did not finish writing its output.";

/// What a crawl's walk of the crawler's data stores produced.
///
/// Recorded by the start, which performs the walk, and consumed by the catalog write: the
/// status a crawl reaches is decided by what the walk answered and not by the operation that
/// asked for it.
#[derive(Debug, Clone)]
pub enum CrawlOutcome {
    /// The targets were walked and these are the datasets found behind them, in the order
    /// the targets name them. An EMPTY list is a crawl of targets that hold no dataset,
    /// which succeeded and wrote nothing.
    Discovered(Vec<DiscoveredTable>),
    /// The targets could not be walked, for the reason the crawl reports in
    /// `LastCrawlInfo$ErrorMessage`.
    Failed(String),
}

/// What a crawl plans to do, read off the stored crawler under the store's guard so the walk
/// itself runs with no guard held.
#[derive(Debug, Clone)]
pub struct CrawlPlan {
    /// The crawler being crawled, which is what every table it writes is stamped
    /// [`UPDATED_BY_CRAWLER_PARAMETER`] with.
    pub crawler_name: String,
    /// The catalog the crawler writes its output to (`Crawler$CatalogId`).
    pub catalog_id: String,
    /// "The name of the database in which the crawler's output is stored"
    /// (`Crawler$DatabaseName`), or `None` when the crawler names none.
    pub database_name: Option<String>,
    /// "The prefix added to the names of tables that are created" (`Crawler$TablePrefix`).
    pub table_prefix: Option<String>,
    /// The Amazon S3 targets to walk: "specifies Amazon Simple Storage Service (Amazon S3)
    /// targets" (`CrawlerTargets$S3Targets`).
    pub s3_targets: Vec<S3Target>,
    /// The update behaviour in force, resolved from the crawler's `SchemaChangePolicy`, else
    /// [`UPDATE_BEHAVIOR_UPDATE_IN_DATABASE`].
    pub update_behavior: String,
    /// The delete behaviour in force, resolved the same way, else
    /// [`DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE`].
    pub delete_behavior: String,
}

impl CrawlPlan {
    /// The plan the stored `crawler`'s definition states.
    pub fn of(crawler: &CrawlerEntry) -> Self {
        let policy = crawler.schema_change_policy.as_ref();
        Self {
            crawler_name: crawler.name.clone(),
            catalog_id: crawler.catalog_id.clone(),
            database_name: crawler
                .database_name
                .clone()
                .filter(|database| !database.trim().is_empty()),
            table_prefix: crawler
                .table_prefix
                .clone()
                .filter(|prefix| !prefix.is_empty()),
            s3_targets: crawler
                .targets
                .s3_targets
                .clone()
                .unwrap_or_default(),
            update_behavior: policy
                .and_then(|policy| policy.update_behavior.clone())
                .unwrap_or_else(|| UPDATE_BEHAVIOR_UPDATE_IN_DATABASE.to_string()),
            delete_behavior: policy
                .and_then(|policy| policy.delete_behavior.clone())
                .unwrap_or_else(|| DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE.to_string()),
        }
    }

    /// Whether this crawler names a data store this crawl reads, i.e. at least one Amazon S3
    /// target carrying a path.
    pub fn has_readable_target(&self) -> bool {
        self.s3_targets.iter().any(|target| {
            target
                .path
                .as_deref()
                .is_some_and(|path| !path.trim().is_empty())
        })
    }

    /// The table name a dataset discovered under `root_name` is written under: the
    /// crawler's `TablePrefix` followed by the dataset's own name.
    pub fn table_name(&self, root_name: &str) -> String {
        match self.table_prefix.as_deref() {
            Some(prefix) => format!("{prefix}{root_name}"),
            None => root_name.to_string(),
        }
    }
}

/// One PARTITION a crawl found behind a dataset: a folder path below the dataset's root, and
/// the values the partition keys take there.
#[derive(Debug, Clone)]
pub struct DiscoveredPartition {
    /// One value per partition key, in the order the keys are declared — the address of the
    /// partition ("ordered in the same order as the partition keys", `PartitionInput$Values`).
    pub values: Vec<String>,
    /// Where that slice of the data sits: the dataset's root followed by the folder path,
    /// which is the partition's own `StorageDescriptor$Location`.
    pub location: String,
}

/// One DATASET a crawl found behind a target: the table it is written as.
#[derive(Debug, Clone)]
pub struct DiscoveredTable {
    /// The dataset's own name, read off the last folder level of the target's path. The
    /// crawler's `TablePrefix` is applied where the table is written
    /// ([`CrawlPlan::table_name`]), so one name is stored in one place.
    pub name: String,
    /// The dataset's root: the location of the table.
    pub location: String,
    /// What the objects were classified as ([`CLASSIFICATION_PARAMETER`]).
    pub classification: String,
    /// The serialization library the classified layout is read with.
    pub serialization_library: String,
    /// The field delimiter of a delimited-text dataset, as the text it is written as, or
    /// `None` for a layout that has none.
    pub delimiter: Option<String>,
    /// How many leading lines of each object are a header rather than data.
    pub header_lines: usize,
    /// The columns the data carries, in the order it carries them.
    pub columns: Vec<Column>,
    /// The partition keys: one per folder level below the dataset's root.
    pub partition_keys: Vec<Column>,
    /// Every distinct folder path at that depth.
    pub partitions: Vec<DiscoveredPartition>,
    /// How many objects the crawl counted behind this dataset.
    pub object_count: u64,
    /// How many bytes those objects hold together.
    pub size_bytes: u64,
}

impl DiscoveredTable {
    /// This dataset as the table definition the catalog stores, under the `name` the plan
    /// resolved and stamped as written by `crawler`.
    pub fn to_table_input(&self, name: String, crawler: &str) -> TableInput {
        TableInput {
            name,
            table_type: Some(CRAWLED_TABLE_TYPE.to_string()),
            parameters: Some(self.table_parameters(crawler)),
            storage_descriptor: Some(self.storage_descriptor(self.location.clone())),
            partition_keys: Some(self.partition_keys.clone()),
            ..TableInput::default()
        }
    }

    /// One partition of this dataset as the input the catalog stores: the same layout as the
    /// table's, at the partition's own location.
    pub fn to_partition_input(&self, partition: &DiscoveredPartition) -> PartitionInput {
        PartitionInput {
            values: Some(partition.values.clone()),
            storage_descriptor: Some(self.storage_descriptor(partition.location.clone())),
            ..PartitionInput::default()
        }
    }

    /// The storage of this dataset at `location`: the layout its classification states, which
    /// a table and each of its partitions describe the same way — only the location differs.
    fn storage_descriptor(&self, location: String) -> StorageDescriptor {
        let mut serde_parameters: HashMap<String, String> = HashMap::new();
        if let Some(delimiter) = self.delimiter.as_ref() {
            serde_parameters.insert(
                FIELD_DELIMITER_SERDE_PARAMETER.to_string(),
                delimiter.clone(),
            );
        }
        StorageDescriptor {
            columns: Some(self.columns.clone()),
            location: Some(location),
            input_format: Some(TEXT_INPUT_FORMAT.to_string()),
            output_format: Some(TEXT_OUTPUT_FORMAT.to_string()),
            serde_info: Some(SerDeInfo {
                serialization_library: Some(self.serialization_library.clone()),
                parameters: Some(serde_parameters),
                ..SerDeInfo::default()
            }),
            ..StorageDescriptor::default()
        }
    }

    /// The table properties a crawl records: what it classified the data as, how much of it
    /// there is, how a reader has to read it, and which crawler wrote the table.
    ///
    /// Every value is computed from THIS crawl's own reading, so a property is there only
    /// when the crawl produced it: a dataset with no field delimiter and no header line
    /// records neither.
    fn table_parameters(&self, crawler: &str) -> HashMap<String, String> {
        let mut parameters: HashMap<String, String> = HashMap::new();
        parameters.insert(
            CLASSIFICATION_PARAMETER.to_string(),
            self.classification.clone(),
        );
        parameters.insert(
            UPDATED_BY_CRAWLER_PARAMETER.to_string(),
            crawler.to_string(),
        );
        parameters.insert(
            COMPRESSION_TYPE_PARAMETER.to_string(),
            UNCOMPRESSED.to_string(),
        );
        parameters.insert(
            TYPE_OF_DATA_PARAMETER.to_string(),
            TYPE_OF_DATA_FILE.to_string(),
        );
        parameters.insert(
            OBJECT_COUNT_PARAMETER.to_string(),
            self.object_count.to_string(),
        );
        parameters.insert(SIZE_KEY_PARAMETER.to_string(), self.size_bytes.to_string());
        parameters.insert(
            SCHEMA_DESERIALIZER_VERSION_PARAMETER.to_string(),
            CRAWLER_SCHEMA_VERSION.to_string(),
        );
        parameters.insert(
            SCHEMA_SERIALIZER_VERSION_PARAMETER.to_string(),
            CRAWLER_SCHEMA_VERSION.to_string(),
        );
        if let Some(delimiter) = self.delimiter.as_ref() {
            parameters.insert(DELIMITER_PARAMETER.to_string(), delimiter.clone());
        }
        if self.header_lines > 0 {
            parameters.insert(
                SKIP_HEADER_LINE_COUNT_PARAMETER.to_string(),
                self.header_lines.to_string(),
            );
        }
        parameters
    }
}

/// What ONE crawl did, recorded on the crawler: the status it reached, the reason it failed
/// if it did, and the tables it wrote.
///
/// Written by the start, which carried the crawl out, and PUBLISHED when the clock settles
/// the crawler ([`CrawlerEntry::settle_crawl_if_due`]) — so the `LastCrawl` and the table
/// counts a read reports are those of the crawl that has finished, and a crawl still in
/// flight leaves the previous crawl's report in place.
#[derive(Debug, Clone)]
pub struct CrawlRecord {
    /// One of the modelled `LastCrawlStatus` values.
    pub status: String,
    /// Why the crawl failed, absent for one that did not.
    pub error_message: Option<String>,
    /// "The time at which the crawl started" (`LastCrawlInfo$StartTime`).
    pub started_at: SystemTime,
    /// When the crawl finished: the DUE instant the clock settled it at, so two reads of one
    /// finished crawl report the same duration. `None` while it is still in flight.
    pub finished_at: Option<SystemTime>,
    /// How many tables this crawl created in the catalog.
    pub tables_created: i32,
    /// How many it updated.
    pub tables_updated: i32,
    /// How many it deleted.
    pub tables_deleted: i32,
}

impl CrawlRecord {
    /// A crawl that started at `started_at` and failed for `reason`, having written nothing.
    pub fn failed(started_at: SystemTime, reason: String) -> Self {
        Self {
            status: LAST_CRAWL_STATUS_FAILED.to_string(),
            error_message: Some(reason),
            started_at,
            finished_at: None,
            tables_created: 0,
            tables_updated: 0,
            tables_deleted: 0,
        }
    }

    /// A crawl that started at `started_at` and wrote `counts` into the catalog.
    pub fn succeeded(started_at: SystemTime, counts: CrawlCounts) -> Self {
        Self {
            status: LAST_CRAWL_STATUS_SUCCEEDED.to_string(),
            // A crawl that wrote what it found failed at nothing, so it carries no error
            // message at all.
            error_message: None,
            started_at,
            finished_at: None,
            tables_created: counts.created,
            tables_updated: counts.updated,
            tables_deleted: counts.deleted,
        }
    }

    /// How long this crawl took, or `None` while it has not finished.
    pub fn duration(&self) -> Option<Duration> {
        self.finished_at?
            .duration_since(self.started_at)
            .ok()
    }

    /// This crawl as the modelled `LastCrawlInfo`: "status and error information about the
    /// most recent crawl".
    ///
    /// `LogGroup`, `LogStream` and `MessagePrefix` are absent: they address the CloudWatch
    /// Logs stream a crawl writes its log to, and nothing here writes one — so naming a
    /// stream would name a record no log read can serve. A job run reports its own log group
    /// the same way, and for the same reason.
    pub fn to_last_crawl_info(&self) -> LastCrawlInfo {
        LastCrawlInfo {
            status: Some(self.status.clone()),
            error_message: self.error_message.clone(),
            log_group: None,
            log_stream: None,
            message_prefix: None,
            start_time: Some(to_date_time(self.started_at)),
        }
    }
}

/// How many tables one crawl created, updated and deleted in the catalog: the figures a
/// metrics read reports for the crawl that finished.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrawlCounts {
    /// Tables the crawl wrote that the database did not hold.
    pub created: i32,
    /// Tables the crawl rewrote because the database already held them.
    pub updated: i32,
    /// Tables the crawl removed because their data was gone and the delete behaviour in
    /// force removes them.
    pub deleted: i32,
}

impl ComAmazonawsGlueAwsglueState {
    /// Give every crawler the run state the CLOCK already owed it at `now`.
    ///
    /// The one write a reporting operation performs on a crawler, exactly as
    /// [`Self::settle_due_column_statistics_task_runs`] is for a column statistics task run:
    /// a crawl was due to finish at [`CrawlerEntry::crawl_settles_at`] whether or not anybody
    /// called, so materialising it on read reports the state the clock had already reached —
    /// the crawler back in [`CRAWLER_STATE_READY`], carrying the finished crawl's
    /// `LastCrawl` and its table counts.
    ///
    /// A crawl that comes to rest here is also the EVENT this service publishes for it
    /// ([`crawler_events`]): the settle is the one place a crawl's record moves from in-flight
    /// to finished, so queueing the change here is what makes it published exactly once —
    /// whichever door materialised the finish, the clock pump or a reader.
    pub fn settle_due_crawls(&mut self, now: SystemTime) {
        let mut changes: Vec<CrawlerStateChange> = Vec::new();
        for crawler in self.crawlers.values_mut() {
            changes.extend(crawler.settle_crawl_if_due(now));
        }
        for change in changes {
            self.queue_crawler_state_change(change);
        }
    }

    /// Write what a crawl DISCOVERED into the Data Catalog, answering how many tables it
    /// created, updated and deleted — or why the discovery could not be written at all.
    ///
    /// The crawl's output goes into the one database the crawler names, so a crawler that
    /// names none, or names one the catalog does not hold, has nowhere to write its tables
    /// and the crawl FAILED with that reason. Every other outcome is a crawl that succeeded:
    /// a target holding no dataset writes no table, which is not a failure.
    ///
    /// What happens to a table the database ALREADY holds is the crawler's
    /// `SchemaChangePolicy$UpdateBehavior`: the default
    /// ([`UPDATE_BEHAVIOR_UPDATE_IN_DATABASE`]) writes the schema the crawl inferred over it,
    /// and [`UPDATE_BEHAVIOR_LOG`] leaves the stored table exactly as it was. What happens to
    /// a table THIS crawler wrote whose data the crawl no longer found is its
    /// `DeleteBehavior`: the default ([`DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE`]) marks it
    /// [`DEPRECATED_BY_CRAWLER_PARAMETER`] and keeps it,
    /// [`DELETE_BEHAVIOR_DELETE_FROM_DATABASE`] removes it, and the logging behaviour leaves
    /// it alone. A table no crawl of this crawler wrote is never touched by either: it is not
    /// this crawler's.
    ///
    /// `encryption` is the key material this write is encrypted under, resolved from the
    /// catalog's own settings by the caller before it took the store's guard, exactly as a
    /// `CreateTable` resolves it (see [`catalog_encryption`]).
    pub fn apply_crawl(
        &mut self,
        plan: &CrawlPlan,
        discovered: &[DiscoveredTable],
        now: SystemTime,
        encryption: Option<ObjectEncryption>,
    ) -> Result<CrawlCounts, String> {
        let Some(database_name) = plan.database_name.as_deref() else {
            return Err(CRAWL_WITHOUT_TARGET_DATABASE.to_string());
        };
        let Some(database) = self.database(&plan.catalog_id, database_name) else {
            return Err(messages::database_not_found(database_name));
        };
        let stored_database_name = database.name.clone();

        let mut counts = CrawlCounts::default();
        // The folded names of the tables this crawl found, which is what the delete
        // behaviour below compares the stored tables against.
        let mut written: HashSet<String> = HashSet::new();
        // Whether a record the crawl found could not be stored because a ceiling was
        // reached. The crawl says so rather than reporting a success whose catalog is
        // missing what the walk found.
        let mut past_a_ceiling = false;
        for dataset in discovered {
            let definition = dataset.to_table_input(plan.table_name(&dataset.name), &plan.crawler_name);
            let folded = fold_name(&definition.name);
            written.insert(folded.clone());
            // The store-wide ceilings are read BEFORE the database is borrowed, and a crawl
            // that would pass one writes no further record rather than growing the store
            // past it.
            let store_has_table_room = self.can_store_table();
            let store_has_partition_room = self.can_store_partition();
            let Some(database) = self.database_mut(&plan.catalog_id, database_name) else {
                // The database was dropped while the data was being walked: there is
                // nothing to write the discovery into.
                return Err(messages::database_not_found(database_name));
            };
            match database.tables.get_mut(&folded) {
                None => {
                    if !store_has_table_room || !database.can_store_table() {
                        past_a_ceiling = true;
                        continue;
                    }
                    let mut entry = TableEntry::from_input(
                        &definition,
                        &stored_database_name,
                        &plan.catalog_id,
                        // `CreatedBy` is "the person or entity who created the table": a
                        // crawl is carried out by the service on the crawler's behalf, and
                        // no principal of the caller's created this table.
                        None,
                        now,
                    );
                    entry.encryption = encryption.clone();
                    database.tables.insert(folded.clone(), entry);
                    counts.created = counts.created.saturating_add(1);
                }
                Some(existing) => {
                    if plan.update_behavior == UPDATE_BEHAVIOR_LOG {
                        // The schema change is recorded and the stored table left alone,
                        // which is what the logging behaviour asks for — so this table is
                        // neither created nor updated.
                        ::tracing::info!(
                            crawler = %plan.crawler_name,
                            table = %folded,
                            "a Glue crawl found a changed schema and left the stored table \
                             alone, as its SchemaChangePolicy asks"
                        );
                        continue;
                    }
                    // The definition the crawl replaced is kept as the version it was
                    // current under, the way every other write to a table keeps it.
                    let archive = existing.can_archive_version();
                    existing.apply_input(&definition, now, archive);
                    existing.encryption = encryption.clone();
                    counts.updated = counts.updated.saturating_add(1);
                }
            }
            if let Some(entry) = database.tables.get_mut(&folded) {
                past_a_ceiling |= !write_partitions(
                    entry,
                    dataset,
                    plan,
                    now,
                    store_has_partition_room,
                    encryption.clone(),
                );
            }
        }
        if past_a_ceiling {
            return Err(CRAWL_PAST_A_STORE_CEILING.to_string());
        }

        // What happens to the tables THIS crawler wrote whose data the crawl no longer
        // found, which is the one decision the delete behaviour makes.
        let Some(database) = self.database_mut(&plan.catalog_id, database_name) else {
            return Err(messages::database_not_found(database_name));
        };
        let abandoned: Vec<String> = database
            .tables
            .iter()
            .filter(|(folded, table)| {
                !written.contains(*folded) && written_by_crawler(table, &plan.crawler_name)
            })
            .map(|(folded, _)| folded.clone())
            .collect();
        for folded in abandoned {
            match plan.delete_behavior.as_str() {
                DELETE_BEHAVIOR_DELETE_FROM_DATABASE => {
                    database.tables.remove(&folded);
                    counts.deleted = counts.deleted.saturating_add(1);
                }
                DELETE_BEHAVIOR_DEPRECATE_IN_DATABASE => {
                    if let Some(table) = database.tables.get_mut(&folded) {
                        table
                            .parameters
                            .get_or_insert_with(HashMap::new)
                            .insert(
                                DEPRECATED_BY_CRAWLER_PARAMETER.to_string(),
                                plan.crawler_name.clone(),
                            );
                        table.update_time = now;
                    }
                }
                _ => ::tracing::info!(
                    crawler = %plan.crawler_name,
                    table = %folded,
                    "a Glue crawl found the data of a stored table gone and left the table \
                     alone, as its SchemaChangePolicy asks"
                ),
            }
        }

        Ok(counts)
    }
}

/// Whether `table` was written by the crawl of `crawler`, i.e. carries that crawler's name
/// in [`UPDATED_BY_CRAWLER_PARAMETER`].
fn written_by_crawler(table: &TableEntry, crawler: &str) -> bool {
    table.parameters.as_ref().is_some_and(|parameters| {
        parameters
            .get(UPDATED_BY_CRAWLER_PARAMETER)
            .is_some_and(|written_by| written_by == crawler)
    })
}

/// Write the partitions one dataset was found with onto the stored `table`, answering whether
/// every one of them could be stored.
///
/// A partition the table already holds is left where it is: its address is the value list,
/// and a crawl that found the same slice again found no new partition. A partition the crawl
/// did NOT find is the delete behaviour's decision, the same one it makes about a table whose
/// data is gone — [`DELETE_BEHAVIOR_DELETE_FROM_DATABASE`] removes it, and the other two keep
/// it (a partition carries no table property to deprecate it with).
///
/// `false` says a partition the crawl FOUND could not be stored because a ceiling was
/// reached, which is what makes the crawl report that it could not write its output
/// ([`CRAWL_PAST_A_STORE_CEILING`]). Every other refusal is a partition that is not this
/// table's to hold: a value list that does not line up with the stored table's partition keys
/// addresses no slice of it, which is what a crawl under [`UPDATE_BEHAVIOR_LOG`] finds when
/// the stored schema it left alone is partitioned differently.
fn write_partitions(
    table: &mut TableEntry,
    dataset: &DiscoveredTable,
    plan: &CrawlPlan,
    now: SystemTime,
    store_has_room: bool,
    encryption: Option<ObjectEncryption>,
) -> bool {
    let found: HashSet<Vec<String>> = dataset
        .partitions
        .iter()
        .map(|partition| partition.values.clone())
        .collect();
    if plan.delete_behavior == DELETE_BEHAVIOR_DELETE_FROM_DATABASE {
        let gone: Vec<Vec<String>> = table
            .partitions
            .keys()
            .filter(|values| !found.contains(*values))
            .cloned()
            .collect();
        for values in gone {
            table.partitions.remove(&values);
        }
    }
    let mut stored_every_one = true;
    for partition in &dataset.partitions {
        if table.partitions.contains_key(&partition.values) {
            continue;
        }
        if let Err(refusal) = table.create_partition(
            &dataset.to_partition_input(partition),
            now,
            store_has_room,
            encryption.clone(),
        ) {
            if matches!(refusal, PartitionCreateRefusal::LimitExceeded { .. }) {
                stored_every_one = false;
            }
        }
    }
    stored_every_one
}
