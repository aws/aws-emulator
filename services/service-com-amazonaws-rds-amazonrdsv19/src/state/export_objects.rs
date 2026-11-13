// SPDX-License-Identifier: CC0-1.0

//! The objects one snapshot or cluster export WRITES, and where it writes them.
//!
//! "You can export DB snapshot data to an Amazon S3 bucket ... The export process runs in
//! the background" and what it leaves behind is the exported data itself: one Parquet file
//! per table under a path built from the export task's identifier, plus the JSON file that
//! says what was exported (Exporting DB snapshot data to Amazon S3, Amazon RDS User Guide).
//! A caller then LISTS and READS those objects with Amazon S3's own operations, so they have
//! to be objects that service really holds — which is why they are recorded through its own
//! store on this same engine ([`::service_com_amazonaws_s3_amazons3::state`]) rather than
//! reported as written.
//!
//! The DATA is the source's own. A DB snapshot carries a copy of the source instance's
//! storage, taken by the engine when the snapshot was taken
//! ([`super::DbSnapshotRecord::storage`]), and a DB cluster that serves a database carries
//! the live one: the export reads the tables out of it with ordinary SQL and writes their
//! rows ([`super::export_parquet`]). Nothing is synthesised: a source this emulator holds no
//! database for exports no table, and the file that says what was exported says so.

use ::std::collections::BTreeMap;
use ::std::time::SystemTime;

use super::data_plane::sql_engine::{Cell, InstanceDatabase, ResultSet};
use super::data_plane::wire_session::MYSQL_SYSTEM_DATABASES;
use super::data_plane::EngineWire;
use super::export_parquet::table_parquet_bytes;

/// One table an export wrote: where it came from, and the object that holds it.
#[derive(Debug, Clone)]
pub struct ExportedTable {
    /// The database the table lives in — the first path segment of its objects.
    pub database: String,
    /// The schema the table lives in: a PostgreSQL schema, and the database's own name on
    /// MySQL and MariaDB, which have no schema layer of their own.
    pub schema: String,
    /// The table's name.
    pub table: String,
    /// How many rows were written.
    pub rows: usize,
    /// The key of the object its rows were written to.
    pub key: String,
    /// The bytes of that object.
    pub body: Vec<u8>,
    /// The source columns, as `(name, declared type)` — what the export's own JSON file
    /// reports as the mapping from the source type to the exported one.
    pub columns: Vec<(String, Option<String>)>,
}

/// One object an export writes: a key in the destination bucket and the bytes stored under
/// it.
#[derive(Debug, Clone)]
pub struct ExportedObject {
    /// The object's key, under the export's own prefix.
    pub key: String,
    /// The object's bytes.
    pub body: Vec<u8>,
}

/// The PostgreSQL schema the tables of a database created without one live in: `public`,
/// the schema in every database's default `search_path`.
const POSTGRES_DEFAULT_SCHEMA: &str = "public";

/// The path segment Amazon RDS writes the one partition of a table under: an export writes
/// each table "in one or more files" under a numbered part name, and a table small enough to
/// be written whole is one part, the first.
const FIRST_PART: &str = "part-00000";

/// How many tables one export may write, so an export of a source holding an unbounded
/// number of tables cannot write an unbounded number of objects into the store. The object
/// store has its own ceilings and refuses a write past them; this is the bound on what one
/// call attempts.
pub const MAX_EXPORTED_TABLES: usize = 256;

/// Whether `entry` — one `ExportOnly` value — is well formed: "database",
/// "database.table", "database.schema" or "database.schema.table", which is what the member
/// publishes as its valid values. A value with no segments, with an empty segment or with
/// more than three names nothing exportable.
#[must_use]
pub fn export_only_entry_is_well_formed(entry: &str) -> bool {
    let segments: Vec<&str> = entry.split('.').collect();
    (1..=3).contains(&segments.len()) && segments.iter().all(|segment| !segment.is_empty())
}

/// Whether an export limited to `export_only` includes the table `(database, schema, table)`.
///
/// An empty list exports everything ("If this parameter isn't provided, all of the data is
/// exported"). A one-segment entry names a database, a three-segment entry names
/// `database.schema.table`, and a two-segment entry names `database.table` on the MySQL
/// engines and `database.schema` on the PostgreSQL ones — which is exactly how the member
/// documents the forms each engine family accepts.
#[must_use]
pub fn export_includes(
    export_only: &[String],
    wire: EngineWire,
    database: &str,
    schema: &str,
    table: &str,
) -> bool {
    if export_only.is_empty() {
        return true;
    }
    export_only.iter().any(|entry| {
        let segments: Vec<&str> = entry.split('.').collect();
        match segments.as_slice() {
            [named_database] => *named_database == database,
            [named_database, second] => {
                *named_database == database
                    && match wire {
                        EngineWire::MySql => *second == table,
                        EngineWire::Postgres => *second == schema,
                    }
            }
            [named_database, named_schema, named_table] => {
                *named_database == database && *named_schema == schema && *named_table == table
            }
            _ => false,
        }
    })
}

/// Read every table the export covers out of `database` and write each one's rows as the
/// Parquet object it is exported to.
///
/// `part_id` is the id this export's part files carry, so the objects of two exports of the
/// same source do not collide. `prefix` is the export's own path — see [`export_prefix`].
///
/// A table the engine will not read is SKIPPED rather than failing the whole export: the
/// export reports the tables it wrote, and the file that says what was exported lists
/// exactly those.
pub fn exported_tables(
    database: &InstanceDatabase,
    wire: EngineWire,
    export_only: &[String],
    prefix: &str,
    part_id: &str,
) -> Vec<ExportedTable> {
    let mut exported = Vec::new();
    for database_name in database.database_names() {
        // The engine's own system schemas are not the customer's data and are not exported.
        if MYSQL_SYSTEM_DATABASES.contains(&database_name.as_str()) {
            continue;
        }
        let schema = match wire {
            EngineWire::MySql => database_name.clone(),
            EngineWire::Postgres => POSTGRES_DEFAULT_SCHEMA.to_string(),
        };
        for table in table_names(database, &database_name) {
            if exported.len() >= MAX_EXPORTED_TABLES {
                return exported;
            }
            if !export_includes(export_only, wire, &database_name, &schema, &table) {
                continue;
            }
            let Ok(rows) = database.execute(&database_name, &format!("SELECT * FROM \"{table}\""), &[])
            else {
                continue;
            };
            let key = format!("{prefix}{database_name}/{schema}.{table}/{part_id}");
            match table_parquet_bytes(&table, &rows) {
                Ok(body) => exported.push(ExportedTable {
                    database: database_name.clone(),
                    schema: schema.clone(),
                    table: table.clone(),
                    rows: rows.rows.len(),
                    key,
                    body,
                    columns: columns_of(&rows),
                }),
                Err(error) => {
                    ::tracing::warn!(
                        table = ?table,
                        error = %error,
                        "a table could not be written as Parquet, so the export left it out"
                    );
                }
            }
        }
    }
    exported
}

/// The tables `database_name` holds, in name order, read out of the engine's own catalog —
/// so a table a client created is exported and one it dropped is not. It is also what the
/// size of a stored snapshot is summed over
/// ([`crate::state::snapshot_data_bytes`]), so the two walk the same tables.
pub fn table_names(database: &InstanceDatabase, database_name: &str) -> Vec<String> {
    let Ok(answer) = database.execute(
        database_name,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
        &[],
    ) else {
        return Vec::new();
    };
    answer
        .rows
        .into_iter()
        .filter_map(|row| match row.first() {
            Some(Cell::Text(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// The `(name, declared type)` of each column of a read.
fn columns_of(rows: &ResultSet) -> Vec<(String, Option<String>)> {
    rows.columns
        .iter()
        .map(|column| (column.name.clone(), column.declared_type.clone()))
        .collect()
}

/// The path every object of one export is written under:
/// `<S3Prefix>/<ExportTaskIdentifier>/`, and `<ExportTaskIdentifier>/` for an export that
/// named no prefix — "the Amazon S3 bucket prefix to use as the file name and path of the
/// exported data" (`S3Prefix`), with the export task's own identifier below it, which is
/// what keeps two exports into one bucket apart.
#[must_use]
pub fn export_prefix(s3_prefix: Option<&str>, export_task_identifier: &str) -> String {
    match s3_prefix.map(|prefix| prefix.trim_matches('/')) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{export_task_identifier}/"),
        _ => format!("{export_task_identifier}/"),
    }
}

/// The name of the part file one table's rows are written to:
/// `part-00000-<id>-c000.gz.parquet` — the part name Amazon RDS writes a table's single
/// partition under, carrying the id of the export that wrote it and the `.gz.parquet`
/// suffix of a GZIP-compressed Parquet file.
#[must_use]
pub fn part_file_name(part_id: &str) -> String {
    format!("{FIRST_PART}-{part_id}-c000.gz.parquet")
}

/// The key of the file that says what an export wrote:
/// `<prefix>export_tables_info_<ExportTaskIdentifier>_from_1_to_<n>.json`, where `n` is how
/// many tables were exported — the file the user guide names as the one holding "information
/// about the exported data".
#[must_use]
pub fn tables_info_key(prefix: &str, export_task_identifier: &str, tables: usize) -> String {
    format!(
        "{prefix}export_tables_info_{export_task_identifier}_from_1_to_{}.json",
        tables.max(1)
    )
}

/// The key of the export's own SUMMARY file: `<prefix>export_info_<ExportTaskIdentifier>.json`
/// — the object a completed export writes beside the per-table files, which is what tooling
/// reads to discover what one export covered.
#[must_use]
pub fn export_info_key(prefix: &str, export_task_identifier: &str) -> String {
    format!("{prefix}export_info_{export_task_identifier}.json")
}

/// What the export's summary file says about the export ITSELF, beside the tables it wrote:
/// the export's own identity and destination, each taken from the task that ran.
#[derive(Debug, Clone, Copy)]
pub struct ExportSummary<'a> {
    /// `ExportTaskIdentifier`, which is also the folder every object of the export sits in.
    pub export_task_identifier: &'a str,
    /// `SourceArn`: the ARN of the snapshot or cluster the data was read out of.
    pub source_arn: &'a str,
    /// `SourceType`, one of the values `ExportSourceType` publishes.
    pub source_type: &'a str,
    /// `S3Bucket`: the destination the objects were written into.
    pub s3_bucket: &'a str,
    /// `S3Prefix`, absent for an export written at the root of the bucket.
    pub s3_prefix: Option<&'a str>,
    /// `KmsKeyId`: the key the exported data is encrypted under.
    pub kms_key_id: &'a str,
    /// `ExportOnly`, empty where the whole source was exported.
    pub export_only: &'a [String],
}

/// The SUMMARY file a completed export writes: what was exported, out of what, where to,
/// how much of it there was, and the per-table row and byte counts — the description of the
/// export as a whole, which the per-table file beside it does not carry.
///
/// Everything in it is read off the export that RAN: the task's own members, the tables the
/// source really held and the sizes of the objects really produced. `started` and `ended`
/// are the instants the export ran between.
#[must_use]
pub fn export_info_document(
    summary: &ExportSummary<'_>,
    tables: &[ExportedTable],
    started: SystemTime,
    ended: SystemTime,
) -> String {
    let bytes: u64 = tables.iter().map(|table| table.body.len() as u64).sum();
    let rows: usize = tables.iter().map(|table| table.rows).sum();
    let exported: Vec<::serde_json::Value> = tables
        .iter()
        .map(|table| {
            ::serde_json::json!({
                "target": format!("{}.{}", table.schema, table.table),
                "database": table.database,
                "schema": table.schema,
                "table": table.table,
                "rowsExported": table.rows,
                "sizeBytes": table.body.len(),
                "files": [table.key.clone()],
            })
        })
        .collect();
    ::serde_json::json!({
        "exportTaskIdentifier": summary.export_task_identifier,
        "sourceArn": summary.source_arn,
        "sourceType": summary.source_type,
        "exportOnly": summary.export_only,
        "s3Bucket": summary.s3_bucket,
        "s3Prefix": summary.s3_prefix.unwrap_or_default(),
        "kmsKeyId": summary.kms_key_id,
        "status": super::EXPORT_TASK_STATUS_COMPLETE,
        "percentProgress": super::COMPLETE_PERCENT_PROGRESS,
        "startTime": epoch_millis(started),
        "endTime": epoch_millis(ended),
        "totalExtractedDataInGB": gigabytes(bytes),
        "totalExtractedDataInBytes": bytes,
        "tablesExported": tables.len(),
        "rowsExported": rows,
        "tables": exported,
    })
    .to_string()
}

/// The JSON file that says what the export wrote: one entry per exported table, naming the
/// table it came from, the status it ended in, the size it wrote, the part files it was
/// written to and the source-to-exported type mapping of each of its columns — the contents
/// the user guide describes for it.
///
/// `started` and `ended` are the instants the export ran between, which the per-table
/// statistics report.
#[must_use]
pub fn tables_info_document(
    tables: &[ExportedTable],
    started: SystemTime,
    ended: SystemTime,
) -> String {
    let entries: Vec<::serde_json::Value> = tables
        .iter()
        .map(|table| {
            let bytes: u64 = table.body.len() as u64;
            ::serde_json::json!({
                "target": format!("{}.{}", table.schema, table.table),
                "database": table.database,
                "schema": table.schema,
                "table": table.table,
                "status": super::EXPORT_TASK_STATUS_COMPLETE,
                "sizeGB": gigabytes(bytes),
                "files": [table.key.clone()],
                "tableStatistics": {
                    "extractionStartTime": epoch_millis(started),
                    "extractionEndTime": epoch_millis(ended),
                    "rowsExported": table.rows,
                    "partitioningInfo": {
                        "numberOfPartitions": 1,
                        "isOverflow": false,
                    },
                },
                "schemaMetadata": {
                    "originalTypeMappings": table
                        .columns
                        .iter()
                        .map(|(name, declared)| {
                            ::serde_json::json!({
                                "columnName": name,
                                "originalType": declared.clone().unwrap_or_default(),
                            })
                        })
                        .collect::<Vec<_>>(),
                },
            })
        })
        .collect();
    ::serde_json::Value::Array(entries).to_string()
}

/// `SizeGB`-style whole gigabytes of `bytes`, which is the unit both the export's own JSON
/// file and `ExportTask$TotalExtractedDataInGB` report in: an integer, so an export smaller
/// than a gigabyte reports zero, exactly as the real service does for a small snapshot.
#[must_use]
pub fn gigabytes(bytes: u64) -> i32 {
    i32::try_from(bytes / (1024 * 1024 * 1024)).unwrap_or(i32::MAX)
}

/// `instant` as epoch milliseconds, the form the per-table statistics carry a time in.
fn epoch_millis(instant: SystemTime) -> u64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Record `objects` in the destination bucket through Amazon S3's own store, answering how
/// many BYTES of them that store took.
///
/// The object store is reached on the SAME engine this service is mounted on, so what this
/// writes is what that service's `ListObjectsV2` and `GetObject` then serve. A store that
/// refuses a write — its own object or byte ceiling — leaves that object unwritten and its
/// bytes out of the answer: the export then reports what was really stored rather than
/// claiming the whole of it, and says so in its `WarningMessage`.
pub fn write_objects<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    objects: &[ExportedObject],
) -> ExportWrite {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        ::tracing::warn!(
            "no object store is reachable from this engine, so an export wrote no object"
        );
        return ExportWrite::default();
    };
    let mut store = state.lock();
    let mut written = ExportWrite::default();
    for object in objects {
        let encryption = store
            .bucket(bucket)
            .map(::service_com_amazonaws_s3_amazons3::state::Bucket::default_object_encryption)
            .unwrap_or_default();
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object {
            // The entity tag is computed over the bytes stored, exactly as it is for an
            // object written through `PutObject`.
            etag: ::service_com_amazonaws_s3_amazons3::state::object_etag(&object.body),
            body: object.body.clone(),
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: ::service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS
                .to_string(),
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            encryption,
            // An exported object carries no Object Lock retention, legal hold, restore or
            // annotation: each of those is placed on a stored version afterwards, by Amazon
            // S3's own operation for it.
            retention: None,
            legal_hold: None,
            restore: None,
            annotations: BTreeMap::new(),
            // And no ACL of its own: it carries the one every object starts with, the
            // owner's `FULL_CONTROL`, which a reader derives.
            acl: None,
            // And no checksum: the export names no checksum algorithm, so a read of the
            // object reports none.
            checksum: None,
            // It is written whole rather than assembled from parts.
            parts: Vec::new(),
            // And nothing replicated it: S3's own store settles a replication record when
            // it records a write through its own data plane.
            replication: None,
        };
        if matches!(
            store.put_object(bucket, &object.key, stored),
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                    | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced
            )
        ) {
            written.objects += 1;
            written.bytes += object.body.len() as u64;
        }
    }
    written
}

/// What one export's write into the object store really stored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportWrite {
    /// How many of the objects offered the store took.
    pub objects: usize,
    /// How many bytes of them it took — what the export reports as the data it extracted.
    pub bytes: u64,
}

/// Whether the object store on this engine holds no bucket named `bucket`.
///
/// `None` means nothing was established — there is no object store on this engine to ask —
/// so a refusal grounded in this answer is only given where the store really answered.
#[must_use]
pub fn bucket_is_absent<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    Some(store.bucket(bucket).is_none())
}

/// The Region the object store holds `bucket` in, for a bucket it holds.
#[must_use]
pub fn bucket_region<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> Option<String> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    store.bucket(bucket).map(|held| held.region.clone())
}

/// The destination bucket's own POLICY, for a bucket the object store holds; `None` for a
/// bucket it does not hold or one carrying no policy.
#[must_use]
pub fn bucket_policy<Eng: Send + Sync + 'static>(engine: &Eng, bucket: &str) -> Option<String> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    store.bucket(bucket).and_then(|held| held.policy.clone())
}
