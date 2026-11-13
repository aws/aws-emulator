// SPDX-License-Identifier: CC0-1.0

//! The statements the DATA CATALOG performs: the Hive and Presto DDL Athena serves by
//! writing to, or reading from, the AWS Glue Data Catalog.
//!
//! Athena registers no catalog objects of its own — `AwsDataCatalog` IS the account's
//! Glue Data Catalog — so a statement here that defines, drops or reports a DATABASE or a
//! TABLE acts on the same catalog records a client writes and reads through `AWSGlue`, and
//! every read below answers out of that one catalog. Nothing is kept beside it: a database
//! or table a client creates through Glue is one these statements see, and one this DDL
//! defines is one Glue's own reads report and Glue's own deletes remove.

use ::std::collections::HashMap;
use ::std::time::SystemTime;

use ::service_com_amazonaws_glue_awsglue::ops::types::{
    Column as GlueColumn, DatabaseInput, SerDeInfo, StorageDescriptor, TableInput,
};
use ::service_com_amazonaws_glue_awsglue::state as glue_state;

use crate::ops::types::{Datum, Row};

use super::statement::{
    self, DeclaredColumn, ExternalTable, Projection, Relation, RelationFilter,
    Schema, StorageFormat, ViewStatement,
};
use super::{Failure, StoredResultSet};

/// The `TableType` of a table whose data lives outside the catalog, which is what a
/// `CREATE EXTERNAL TABLE` defines.
pub const TABLE_TYPE_EXTERNAL: &str = "EXTERNAL_TABLE";

/// The `TableType` of a view: a query the catalog holds rather than data.
pub const TABLE_TYPE_VIEW: &str = "VIRTUAL_VIEW";

/// The `Owner` of a table Athena's DDL engine registers: the Hadoop user that engine
/// runs its Hive metastore client as, which is the owner a client reads back for a table
/// authored through DDL rather than through the Glue API.
pub const DDL_TABLE_OWNER: &str = "hadoop";

/// The table property that marks a table as external, as Hive writes it.
pub const EXTERNAL_PROPERTY: &str = "EXTERNAL";

/// The value [`EXTERNAL_PROPERTY`] carries.
pub const EXTERNAL_PROPERTY_VALUE: &str = "TRUE";

/// The table property that marks a view as one the Presto-derived query engine defined.
pub const PRESTO_VIEW_PROPERTY: &str = "presto_view";

/// The value [`PRESTO_VIEW_PROPERTY`] carries.
pub const PRESTO_VIEW_PROPERTY_VALUE: &str = "true";

/// The `ViewExpandedText` of a view the Presto-derived engine defined: the marker that
/// engine writes in place of an expanded Hive query, because the view's own definition is
/// carried in [`ViewOriginalText`](view_original_text) instead.
pub const PRESTO_VIEW_EXPANDED_TEXT: &str = "/* Presto View */";

/// What a view's `ViewOriginalText` wraps its definition in.
const PRESTO_VIEW_PREFIX: &str = "/* Presto View: ";
/// The close of [`PRESTO_VIEW_PREFIX`].
const PRESTO_VIEW_SUFFIX: &str = " */";

/// How many buckets a table the DDL engine registers is stored in.
///
/// A `CREATE EXTERNAL TABLE` declares no bucketing, and Hive records an unbucketed table
/// with this count — which is how a DDL-authored table differs from one the Glue API
/// created with no count of its own.
const UNBUCKETED: i32 = -1;

/// The serialization library a table whose statement names none is read with: Hive's own
/// default SerDe for delimited text, which is what Athena records for a
/// `CREATE EXTERNAL TABLE` with no `ROW FORMAT SERDE` of its own.
pub const LAZY_SIMPLE_SERDE: &str = "org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe";

/// The `InputFormat` of a table stored as delimited text.
pub const TEXT_INPUT_FORMAT: &str = "org.apache.hadoop.mapred.TextInputFormat";

/// The `OutputFormat` of a table stored as delimited text.
pub const TEXT_OUTPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.HiveIgnoreKeyTextOutputFormat";

/// The serialization library a table of JSON documents is read with.
pub const JSON_SERDE: &str = "org.apache.hive.hcatalog.data.JsonSerDe";

/// The `InputFormat` of a table stored in the columnar format of that name.
pub const PARQUET_INPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.parquet.MapredParquetInputFormat";

/// The `OutputFormat` that goes with [`PARQUET_INPUT_FORMAT`].
pub const PARQUET_OUTPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.parquet.MapredParquetOutputFormat";

/// The serialization library that goes with [`PARQUET_INPUT_FORMAT`].
pub const PARQUET_SERDE: &str = "org.apache.hadoop.hive.ql.io.parquet.serde.ParquetHiveSerDe";

/// The `InputFormat` of a table stored in the optimised row-columnar format.
pub const ORC_INPUT_FORMAT: &str = "org.apache.hadoop.hive.ql.io.orc.OrcInputFormat";

/// The `OutputFormat` that goes with [`ORC_INPUT_FORMAT`].
pub const ORC_OUTPUT_FORMAT: &str = "org.apache.hadoop.hive.ql.io.orc.OrcOutputFormat";

/// The serialization library that goes with [`ORC_INPUT_FORMAT`].
pub const ORC_SERDE: &str = "org.apache.hadoop.hive.ql.io.orc.OrcSerde";

/// The `InputFormat` of a table stored in Avro containers.
pub const AVRO_INPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.avro.AvroContainerInputFormat";

/// The `OutputFormat` that goes with [`AVRO_INPUT_FORMAT`].
pub const AVRO_OUTPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.avro.AvroContainerOutputFormat";

/// The serialization library that goes with [`AVRO_INPUT_FORMAT`].
pub const AVRO_SERDE: &str = "org.apache.hadoop.hive.serde2.avro.AvroSerDe";

/// The `InputFormat` of a table stored in Hadoop sequence files.
pub const SEQUENCE_INPUT_FORMAT: &str = "org.apache.hadoop.mapred.SequenceFileInputFormat";

/// The `OutputFormat` that goes with [`SEQUENCE_INPUT_FORMAT`].
pub const SEQUENCE_OUTPUT_FORMAT: &str =
    "org.apache.hadoop.hive.ql.io.HiveSequenceFileOutputFormat";

/// The `InputFormat` of a table stored in record-columnar files.
pub const RCFILE_INPUT_FORMAT: &str = "org.apache.hadoop.hive.ql.io.RCFileInputFormat";

/// The `OutputFormat` that goes with [`RCFILE_INPUT_FORMAT`].
pub const RCFILE_OUTPUT_FORMAT: &str = "org.apache.hadoop.hive.ql.io.RCFileOutputFormat";

/// The serialization library that goes with [`RCFILE_INPUT_FORMAT`].
pub const COLUMNAR_SERDE: &str =
    "org.apache.hadoop.hive.serde2.columnar.LazyBinaryColumnarSerDe";

/// The column a `SHOW TABLES` or `SHOW VIEWS` result set carries, as the Hive DDL engine
/// that answers those statements names it.
const RELATION_NAME_COLUMN: &str = "tab_name";

/// The column a `SHOW CREATE VIEW` result set carries.
const CREATE_VIEW_COLUMN: &str = "Create View";

/// The column a `SHOW DATABASES` or `SHOW SCHEMAS` result set carries, as the Hive DDL
/// engine that answers those statements names it.
const DATABASE_NAME_COLUMN: &str = "database_name";

/// Register the database a `CREATE DATABASE` (or `CREATE SCHEMA`) defines in the Data
/// Catalog.
///
/// The entry is the one the catalog holds for a database: the folded name, the statement's
/// `COMMENT` as its description, its `LOCATION` as the location URI and its properties as
/// the database parameters — the same entry `AWSGlue.CreateDatabase` writes, because
/// `AwsDataCatalog` is that one catalog.
///
/// `IF NOT EXISTS` over a database that is already there changes NOTHING and succeeds;
/// without it, an existing database is the metastore's own already-exists refusal and the
/// stored entry is left exactly as it was.
pub fn create_database(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    statement: &statement::DatabaseStatement,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let database = &statement.target.database;
    if store.database(catalog_id, database).is_some() {
        if statement.if_not_exists {
            return Ok(StoredResultSet::default());
        }
        return Err(Failure::database_already_exists(database));
    }
    let catalog_can_hold_another = store
        .catalogs
        .get(catalog_id)
        .is_none_or(glue_state::Catalog::can_store_database);
    if !store.can_store_database() || !catalog_can_hold_another {
        return Err(Failure::emulator(format!(
            "the Data Catalog already holds the maximum of {} databases, so {database} \
             cannot be registered",
            glue_state::MAX_DATABASES_PER_CATALOG
        )));
    }
    let input = DatabaseInput {
        name: database.clone(),
        description: statement.comment.clone(),
        location_uri: statement.location.as_deref().map(trimmed_location),
        parameters: (!statement.properties.is_empty()).then(|| {
            statement
                .properties
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        }),
        ..DatabaseInput::default()
    };
    let entry = glue_state::DatabaseEntry::from_input(&input, catalog_id, None, now);
    let Some(catalog) = store.catalog_mut_for_create(catalog_id) else {
        return Err(Failure::emulator(format!(
            "this emulator holds no Data Catalog {catalog_id} can be created in, so \
             {database} cannot be registered"
        )));
    };
    catalog
        .databases
        .insert(glue_state::fold_name(database), entry);
    Ok(StoredResultSet::default())
}

/// Drop the database a `DROP DATABASE` (or `DROP SCHEMA`) names.
///
/// A database that still holds tables is dropped only by a statement that asked for its
/// contents to go with it (`CASCADE`); without that, the metastore refuses the drop and
/// the database and every table in it are left exactly as they were. `IF EXISTS` tolerates
/// an absent database; without it, an absent database is the catalog's own not-found
/// refusal.
///
/// Answers the TABLES the drop removed with the database, whose own per-table records go
/// with them.
pub fn drop_database(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    target: &Schema,
    if_exists: bool,
    cascade: bool,
) -> Result<Vec<glue_state::TableEntry>, Failure> {
    let Some(database) = store.database(catalog_id, &target.database) else {
        if if_exists {
            return Ok(Vec::new());
        }
        // The drop is performed by the Hive DDL engine, which resolves the name while it
        // compiles the statement.
        return Err(Failure::hive_database_not_found(&target.database));
    };
    if !database.tables.is_empty() && !cascade {
        return Err(Failure::database_not_empty(&target.database));
    }
    // Removing the entry removes the tables it owns with it: they are stored inside it,
    // which is the cascade the catalog's own delete performs.
    let removed = store.catalogs.get_mut(catalog_id).and_then(|catalog| {
        catalog
            .databases
            .remove(&glue_state::fold_name(&target.database))
    });
    Ok(removed
        .into_iter()
        .flat_map(|database| database.tables.into_values())
        .collect())
}

/// Drop the table a `DROP TABLE` names, and answer the entry it removed: that entry is the
/// one the catalog API's own delete removes, so neither reader is left holding a table the
/// other dropped.
///
/// Only a TABLE is dropped: the statement names a table, so an entry of that name that is
/// a VIEW is not what it addresses, and is answered with the same table-does-not-exist
/// refusal a name the database holds nothing for gets — the mirror of [`drop_view`], which
/// answers a table of the name it addresses with its own view-does-not-exist refusal.
///
/// The caller decides whether the statement addresses a catalog table at all
/// (`super::drops_a_table_of_the_engines_own`): a name in a database the catalog does not
/// hold is no catalog table. Inside a database it does hold, `IF EXISTS` tolerates an
/// absent table and without it that absence is the engine's own table-does-not-exist
/// refusal.
pub fn drop_table(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    target: &Relation,
    if_exists: bool,
) -> Result<Vec<glue_state::TableEntry>, Failure> {
    let Some(database) = store.database_mut(catalog_id, &target.database) else {
        if if_exists {
            return Ok(Vec::new());
        }
        return Err(Failure::schema_not_found(target));
    };
    match database.tables.get(&target.table) {
        Some(entry) if !is_view(entry) => Ok(database
            .tables
            .remove(&target.table)
            .into_iter()
            .collect()),
        // An entry that is a view, and a name nothing is stored under, are the same answer:
        // the statement addresses no table.
        Some(_) | None if if_exists => Ok(Vec::new()),
        Some(_) | None => Err(Failure::table_not_found(target)),
    }
}

/// List the databases of one data catalog, as `SHOW DATABASES` and `SHOW SCHEMAS` answer
/// it.
///
/// Every database the catalog holds, in the catalog's own name order — and a statement that
/// gives a PATTERN lists only the names that match it as a whole, exactly as a filtered
/// `SHOW TABLES` does.
pub fn show_databases(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    filter: Option<&RelationFilter>,
) -> Result<StoredResultSet, Failure> {
    let matcher = match filter {
        None => None,
        Some(filter) => Some(name_matcher(filter)?),
    };
    let names = store
        .catalogs
        .get(catalog_id)
        .map(glue_state::Catalog::databases_ordered)
        .unwrap_or_default()
        .into_iter()
        .filter(|database| {
            matcher
                .as_ref()
                .is_none_or(|matcher| matcher.is_match(&database.name))
        })
        .map(|database| database.name.clone())
        .collect();
    Ok(single_column_result(DATABASE_NAME_COLUMN, names))
}

/// Register the table a `CREATE EXTERNAL TABLE` defines in the Data Catalog.
///
/// The entry is the one the catalog holds for a DDL-authored table: its type is
/// [`TABLE_TYPE_EXTERNAL`], its owner the DDL engine's own
/// ([`DDL_TABLE_OWNER`]), its parameters carry [`EXTERNAL_PROPERTY`] beside every
/// declared table property, and its storage descriptor carries the declared columns, the
/// location with no trailing separator — a location names a PREFIX, and the catalog
/// records the prefix itself rather than the separator that followed it — and the SerDe
/// the statement named or the default one for delimited text.
///
/// Everything a Hive registration materialises beyond what the statement declared is
/// stored with it ([`glue_state::materialise_hive_registration`]), so the entry a catalog
/// read reports carries the empty collections and the DDL time of a Hive table object
/// rather than leaving them unknown.
///
/// `IF NOT EXISTS` over an entry that is already there changes NOTHING and succeeds;
/// without it, an existing entry is the catalog's own already-exists refusal and the
/// stored entry is left exactly as it was.
pub fn create_external_table(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    table: &ExternalTable,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let store_can_hold_another = store.can_store_table();
    let Some(database) = store.database_mut(catalog_id, &table.target.database) else {
        // The statement is one the Hive DDL engine performs, so a database it cannot
        // resolve is that engine's own semantic error.
        return Err(Failure::hive_database_not_found(&table.target.database));
    };
    if database.tables.contains_key(&table.target.table) {
        if table.if_not_exists {
            return Ok(StoredResultSet::default());
        }
        return Err(Failure::table_already_exists(&table.target));
    }
    if !store_can_hold_another || !database.can_store_table() {
        return Err(Failure::emulator(format!(
            "the Data Catalog already holds the maximum of {} tables, so {} cannot be \
             registered",
            glue_state::MAX_TABLES_PER_DATABASE, table.target.table
        )));
    }
    let mut input = external_table_input(table)?;
    // The definition a Hive metastore client writes is a WHOLE Hive table object, so the
    // members the statement declared nothing for are stored as that client stores them.
    glue_state::materialise_hive_registration(&mut input, now);
    let database_name = database.name.clone();
    let entry = glue_state::TableEntry::from_input(
        &input,
        &database_name,
        catalog_id,
        Some(glue_state::request_principal()),
        now,
    );
    database.tables.insert(table.target.table.clone(), entry);
    Ok(StoredResultSet::default())
}

/// The catalog definition a `CREATE EXTERNAL TABLE` DECLARES, or why the statement names a
/// storage format the catalog cannot record.
///
/// What the statement itself says, and nothing more: the members a Hive registration
/// materialises beside them are filled in by the caller, so there is one place that decides
/// each.
fn external_table_input(table: &ExternalTable) -> Result<TableInput, Failure> {
    let mut parameters: HashMap<String, String> = table
        .table_properties
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    parameters.insert(
        EXTERNAL_PROPERTY.to_string(),
        EXTERNAL_PROPERTY_VALUE.to_string(),
    );
    let serde_parameters: HashMap<String, String> = table
        .serde_parameters
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let storage = hive_storage(table.storage.as_ref()).ok_or_else(|| {
        Failure::user(format!(
            "line {}:{}: Unsupported storage format: {}",
            table.target.at.line,
            table.target.at.column,
            match table.storage.as_ref() {
                Some(StorageFormat::Named(named)) => named.clone(),
                _ => String::new(),
            }
        ))
    })?;
    Ok(TableInput {
        name: table.target.table.clone(),
        description: table.comment.clone(),
        owner: Some(DDL_TABLE_OWNER.to_string()),
        table_type: Some(TABLE_TYPE_EXTERNAL.to_string()),
        parameters: Some(parameters),
        storage_descriptor: Some(StorageDescriptor {
            columns: Some(table.columns.iter().map(glue_column).collect()),
            location: table.location.as_deref().map(trimmed_location),
            input_format: Some(storage.input.clone()),
            output_format: Some(storage.output.clone()),
            number_of_buckets: UNBUCKETED,
            serde_info: Some(SerDeInfo {
                name: None,
                // The serialization library is the one the statement named, and with none
                // the one the DECLARED storage format is read with — never a library the
                // statement did not ask for.
                serialization_library: Some(
                    table
                        .serde_library
                        .clone()
                        .unwrap_or_else(|| storage.serde.to_string()),
                ),
                parameters: Some(serde_parameters),
            }),
            ..StorageDescriptor::default()
        }),
        partition_keys: (!table.partition_columns.is_empty())
            .then(|| table.partition_columns.iter().map(glue_column).collect()),
        ..TableInput::default()
    })
}

/// How Hive stores a table of one storage format: the format its data is read with, the
/// one it is written with, and the serialization library a table that names none of its
/// own is read with.
struct HiveStorage {
    /// `StorageDescriptor$InputFormat`.
    input: String,
    /// `StorageDescriptor$OutputFormat`.
    output: String,
    /// The default `SerDeInfo$SerializationLibrary` of that format.
    serde: &'static str,
}

/// The input format, output format and default serialization library the declared storage
/// format is recorded with, or `None` for a `STORED AS` naming a format Hive has no such
/// triple for.
///
/// A statement that declares NO storage format stores delimited text, which is the format
/// Hive stores a table of no declared format in. Each named format carries Hive's own
/// classes for it, so the entry the catalog holds says how the data is really stored and a
/// reader of that entry — Glue's own reads, and this crate's own data reader — is told the
/// truth about it.
fn hive_storage(storage: Option<&StorageFormat>) -> Option<HiveStorage> {
    let named = |input: &str, output: &str, serde: &'static str| HiveStorage {
        input: input.to_string(),
        output: output.to_string(),
        serde,
    };
    let Some(storage) = storage else {
        return Some(named(TEXT_INPUT_FORMAT, TEXT_OUTPUT_FORMAT, LAZY_SIMPLE_SERDE));
    };
    match storage {
        StorageFormat::Formats { input, output } => Some(HiveStorage {
            input: input.clone(),
            output: output.clone(),
            // A statement that names the two formats itself and no serde stores Hive's
            // own default serialization library, exactly as one that names no format does.
            serde: LAZY_SIMPLE_SERDE,
        }),
        StorageFormat::Named(format) => match format.to_ascii_uppercase().as_str() {
            "TEXTFILE" => Some(named(
                TEXT_INPUT_FORMAT,
                TEXT_OUTPUT_FORMAT,
                LAZY_SIMPLE_SERDE,
            )),
            "JSONFILE" => Some(named(TEXT_INPUT_FORMAT, TEXT_OUTPUT_FORMAT, JSON_SERDE)),
            "PARQUET" => Some(named(
                PARQUET_INPUT_FORMAT,
                PARQUET_OUTPUT_FORMAT,
                PARQUET_SERDE,
            )),
            "ORC" => Some(named(ORC_INPUT_FORMAT, ORC_OUTPUT_FORMAT, ORC_SERDE)),
            "AVRO" => Some(named(AVRO_INPUT_FORMAT, AVRO_OUTPUT_FORMAT, AVRO_SERDE)),
            "SEQUENCEFILE" => Some(named(
                SEQUENCE_INPUT_FORMAT,
                SEQUENCE_OUTPUT_FORMAT,
                LAZY_SIMPLE_SERDE,
            )),
            "RCFILE" => Some(named(
                RCFILE_INPUT_FORMAT,
                RCFILE_OUTPUT_FORMAT,
                COLUMNAR_SERDE,
            )),
            _ => None,
        },
    }
}

/// A declared column as the catalog's own column.
fn glue_column(column: &DeclaredColumn) -> GlueColumn {
    GlueColumn {
        name: column.name.clone(),
        r#type: Some(column.data_type.clone()),
        comment: column.comment.clone(),
        parameters: None,
    }
}

/// A `LOCATION` as the catalog records it: the prefix itself, without the separator that
/// closed it.
fn trimmed_location(location: &str) -> String {
    location.trim_end_matches('/').to_string()
}

/// Register the view a `CREATE [OR REPLACE] VIEW` defines in the Data Catalog.
///
/// A view is a QUERY the catalog holds: its entry is of type [`TABLE_TYPE_VIEW`], it
/// carries the submitted query ([`view_original_text`]) and the marker the
/// Presto-derived engine writes for an expanded text, its parameters say it is such a
/// view, and its storage descriptor names the view's own columns over an empty location —
/// there is no data under a view.
///
/// The view's sources must be relations the catalog holds, so a view over a table that
/// does not exist is refused rather than stored; an existing view is REPLACED only when
/// the statement said so, and an existing TABLE of that name is never replaced by a view.
pub fn create_view(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    view: &ViewStatement,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let columns = view_columns(store, catalog_id, view)?;
    let store_can_hold_another = store.can_store_table();
    let Some(database) = store.database_mut(catalog_id, &view.target.database) else {
        return Err(Failure::schema_not_found(&view.target));
    };
    match database.tables.get(&view.target.table) {
        Some(existing) if !is_view(existing) => {
            return Err(Failure::table_already_exists(&view.target));
        }
        Some(_) if !view.or_replace => {
            return Err(Failure::view_already_exists(&view.target));
        }
        Some(_) => {}
        None if !store_can_hold_another || !database.can_store_table() => {
            return Err(Failure::emulator(format!(
                "the Data Catalog already holds the maximum of {} tables, so the view {} \
                 cannot be registered",
                glue_state::MAX_TABLES_PER_DATABASE, view.target.table
            )));
        }
        None => {}
    }
    let input = view_input(view, &columns);
    let database_name = database.name.clone();
    let entry = glue_state::TableEntry::from_input(
        &input,
        &database_name,
        catalog_id,
        Some(glue_state::request_principal()),
        now,
    );
    database.tables.insert(view.target.table.clone(), entry);
    Ok(StoredResultSet::default())
}

/// The columns a view projects, read off its query's projection and the catalog entries
/// that query reads.
///
/// A view's columns are the ones its query PROJECTS: a projection that reads a source
/// column takes that column's type, a `*` takes every column its sources hold, and an
/// expression carries the name it is projected under. A column list the statement declared
/// names them instead, position by position. Every source must exist: a view over a
/// relation the catalog does not hold is refused.
fn view_columns(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    view: &ViewStatement,
) -> Result<Vec<DeclaredColumn>, Failure> {
    let mut source_columns: Vec<DeclaredColumn> = Vec::new();
    for source in &view.sources {
        let Some(database) = store.database(catalog_id, &source.database) else {
            return Err(Failure::schema_not_found(source));
        };
        let Some(table) = database.tables.get(&source.table) else {
            return Err(Failure::table_not_found(source));
        };
        source_columns.extend(table_columns(table));
    }
    let typed = |name: &str| {
        source_columns
            .iter()
            .find(|column| column.name == name)
            .map_or_else(|| UNKNOWN_COLUMN_TYPE.to_string(), |column| column.data_type.clone())
    };
    let mut columns: Vec<DeclaredColumn> = Vec::new();
    for projected in &view.projection {
        match projected {
            Projection::All => columns.extend(source_columns.iter().cloned()),
            Projection::Column { name, source } => columns.push(DeclaredColumn {
                name: name.clone(),
                data_type: typed(source.as_deref().unwrap_or(name)),
                comment: None,
            }),
        }
    }
    if view.columns.is_empty() {
        return Ok(columns);
    }
    // A declared column list names the view's columns; the type of each is the one the
    // projected column at that position carries.
    Ok(view
        .columns
        .iter()
        .enumerate()
        .map(|(position, name)| DeclaredColumn {
            name: name.clone(),
            data_type: columns.get(position).map_or_else(
                || UNKNOWN_COLUMN_TYPE.to_string(),
                |column| column.data_type.clone(),
            ),
            comment: None,
        })
        .collect())
}

/// The type recorded for a view column whose type no source names.
const UNKNOWN_COLUMN_TYPE: &str = "string";

/// The columns a stored catalog entry holds.
pub fn table_columns(table: &glue_state::TableEntry) -> Vec<DeclaredColumn> {
    table
        .storage_descriptor
        .as_ref()
        .and_then(|storage| storage.columns.as_ref())
        .map(|columns| {
            columns
                .iter()
                .map(|column| DeclaredColumn {
                    name: column.name.clone(),
                    data_type: column
                        .r#type
                        .clone()
                        .unwrap_or_else(|| UNKNOWN_COLUMN_TYPE.to_string()),
                    comment: column.comment.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The catalog definition a `CREATE VIEW` writes.
fn view_input(view: &ViewStatement, columns: &[DeclaredColumn]) -> TableInput {
    let mut parameters = HashMap::new();
    parameters.insert(
        PRESTO_VIEW_PROPERTY.to_string(),
        PRESTO_VIEW_PROPERTY_VALUE.to_string(),
    );
    TableInput {
        name: view.target.table.clone(),
        owner: Some(DDL_TABLE_OWNER.to_string()),
        table_type: Some(TABLE_TYPE_VIEW.to_string()),
        parameters: Some(parameters),
        view_original_text: Some(view_original_text(view, columns)),
        view_expanded_text: Some(PRESTO_VIEW_EXPANDED_TEXT.to_string()),
        storage_descriptor: Some(StorageDescriptor {
            columns: Some(columns.iter().map(glue_column).collect()),
            // A view has no data, so its location is empty rather than absent: the
            // catalog holds a storage descriptor for it with nothing under it.
            location: Some(String::new()),
            ..StorageDescriptor::default()
        }),
        ..TableInput::default()
    }
}

/// The `ViewOriginalText` of a view the Presto-derived engine defined.
///
/// That engine does not store the submitted SQL as plain text: it stores its own
/// definition of the view — the query as the ENGINE renders it, the catalog and schema the
/// view was created in, and the columns it projects — encoded, inside the comment marker a
/// Hive metastore carries it in. A reader that understands the marker gets the definition
/// back; one that does not sees only that the entry is such a view.
///
/// The query is the engine's own rendering of the statement's parse
/// ([`statement::formatted_query`]) rather than the text the client typed, so a predicate
/// is held as the expression it is.
fn view_original_text(view: &ViewStatement, columns: &[DeclaredColumn]) -> String {
    let definition = ::serde_json::json!({
        "originalSql": statement::formatted_query(&view.query),
        "catalog": view.target.catalog.to_ascii_lowercase(),
        "schema": view.target.database,
        "columns": columns
            .iter()
            .map(|column| ::serde_json::json!({
                "name": column.name,
                "type": engine_type_name(&column.data_type),
            }))
            .collect::<Vec<_>>(),
        "owner": DDL_TABLE_OWNER,
        "runAsInvoker": false,
    });
    let encoded = ::aws_smithy_types::base64::encode(definition.to_string());
    format!("{PRESTO_VIEW_PREFIX}{encoded}{PRESTO_VIEW_SUFFIX}")
}

/// Whether a stored catalog entry is a VIEW: the entry's own `TableType`, which is what
/// every reader of the catalog tells a view from a table by.
pub fn is_view(table: &glue_state::TableEntry) -> bool {
    table
        .table_type
        .as_deref()
        .is_some_and(|table_type| table_type.eq_ignore_ascii_case(TABLE_TYPE_VIEW))
}

/// Drop the view a `DROP VIEW` names.
///
/// Only a VIEW is dropped: the statement names a view, so an entry of that name that is a
/// table is not what it addresses, and is refused with the engine's own
/// view-does-not-exist refusal rather than deleted. `IF EXISTS` tolerates an absent view;
/// without it, an absent view is that same refusal.
pub fn drop_view(
    store: &mut glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    target: &Relation,
    if_exists: bool,
) -> Result<StoredResultSet, Failure> {
    let Some(database) = store.database_mut(catalog_id, &target.database) else {
        if if_exists {
            return Ok(StoredResultSet::default());
        }
        return Err(Failure::schema_not_found(target));
    };
    match database.tables.get(&target.table) {
        Some(entry) if is_view(entry) => {
            database.tables.remove(&target.table);
            Ok(StoredResultSet::default())
        }
        Some(_) => Err(Failure::view_not_found(target)),
        None if if_exists => Ok(StoredResultSet::default()),
        None => Err(Failure::view_not_found(target)),
    }
}

/// Report the statement a view was defined by, as `SHOW CREATE VIEW` answers it.
///
/// Only a VIEW has one: an entry of that name that is a table is not a view, which is the
/// same refusal `DROP VIEW` answers for it.
pub fn show_create_view(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    target: &Relation,
) -> Result<StoredResultSet, Failure> {
    let Some(database) = store.database(catalog_id, &target.database) else {
        return Err(Failure::schema_not_found(target));
    };
    let entry = database.tables.get(&target.table);
    match entry {
        Some(entry) if is_view(entry) => {}
        Some(_) => return Err(Failure::not_a_view(target)),
        None => return Err(Failure::view_not_found(target)),
    }
    let original = database
        .tables
        .get(&target.table)
        .and_then(|entry| entry.view_original_text.as_deref())
        .map(view_definition_sql)
        .unwrap_or_default();
    Ok(single_column_result(
        CREATE_VIEW_COLUMN,
        vec![format!(
            "CREATE VIEW {}.{} AS\n{}",
            target.database,
            target.table,
            statement::formatted_query(&original)
        )],
    ))
}

/// The name Athena's QUERY ENGINE has for the type a catalog column is declared with.
///
/// The catalog records a column in Hive's type names and the engine computes in its own,
/// and the two differ where each names the same type differently: Hive's unbounded
/// character type is the engine's `varchar`, its single-precision number the engine's
/// `real`, its binary type the engine's `varbinary` and its `int` the engine's `integer`.
/// A type the two name alike keeps its name, and the precision or length that qualifies a
/// type is kept as written, because it is part of the type either engine names.
pub fn engine_type_name(data_type: &str) -> String {
    let declared = data_type.trim().to_ascii_lowercase();
    let (named, qualifier) = match declared.split_once('(') {
        Some((named, rest)) => (named.trim().to_string(), Some(format!("({rest}"))),
        None => (declared.clone(), None),
    };
    let engine = match named.as_str() {
        "string" => "varchar",
        "float" => "real",
        "binary" => "varbinary",
        "int" => "integer",
        named => named,
    };
    match qualifier {
        Some(qualifier) => format!("{engine}{qualifier}"),
        None => engine.to_string(),
    }
}

/// The SQL a view's stored `ViewOriginalText` carries: the original statement out of the
/// definition the engine encoded ([`view_original_text`]), or the stored text itself when
/// it carries no such definition.
pub fn view_definition_sql(original_text: &str) -> String {
    let encoded = original_text
        .trim()
        .strip_prefix(PRESTO_VIEW_PREFIX)
        .and_then(|rest| rest.strip_suffix(PRESTO_VIEW_SUFFIX));
    let Some(encoded) = encoded else {
        return original_text.to_string();
    };
    let decoded = ::aws_smithy_types::base64::decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some(decoded) = decoded else {
        return original_text.to_string();
    };
    ::serde_json::from_str::<::serde_json::Value>(&decoded)
        .ok()
        .as_ref()
        .and_then(|definition| definition.get("originalSql"))
        .and_then(::serde_json::Value::as_str)
        .map_or_else(|| original_text.to_string(), ToString::to_string)
}

/// List the relations of one database, as `SHOW TABLES` and `SHOW VIEWS` answer it.
///
/// `SHOW TABLES` lists every relation the database holds and `SHOW VIEWS` only the views,
/// each in the catalog's own name order — and a statement that gives a PATTERN lists only
/// the names that match it as a whole, because a filtered statement narrows its answer.
pub fn show_relations(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    schema: &Schema,
    views_only: bool,
    filter: Option<&RelationFilter>,
) -> Result<StoredResultSet, Failure> {
    let matcher = match filter {
        None => None,
        Some(filter) => Some(name_matcher(filter)?),
    };
    let Some(database) = store.database(catalog_id, &schema.database) else {
        return Err(Failure::schema_not_found_at(schema.at, &schema.database));
    };
    let names = database
        .tables_ordered()
        .into_iter()
        .filter(|table| !views_only || is_view(table))
        .filter(|table| {
            matcher
                .as_ref()
                .is_none_or(|matcher| matcher.is_match(&table.name))
        })
        .map(|table| table.name.clone())
        .collect();
    Ok(single_column_result(RELATION_NAME_COLUMN, names))
}

/// The matcher one `SHOW` filter selects names with, or why the statement cannot be
/// narrowed by it.
///
/// A BARE pattern is the one the Data Catalog's own listings match a name by, so it is
/// compiled by the catalog's own matcher and the two listings of one set of relations can
/// never disagree. A `LIKE` pattern is the SQL one — `%` for any run of characters, `_`
/// for one, every other character itself — and `ILIKE` matches it without regard to case.
/// A filter that narrows by something other than a pattern is one no `SHOW` statement of
/// this service takes, and is reported rather than ignored.
fn name_matcher(filter: &RelationFilter) -> Result<::regex::Regex, Failure> {
    match filter {
        RelationFilter::Pattern(pattern) => glue_state::compile_name_pattern(pattern)
            .map_err(|reason| Failure::user(crate::state::malformed_pattern(pattern, &reason))),
        RelationFilter::Like {
            pattern,
            case_insensitive,
        } => {
            let mut translated = String::from("^(?:");
            for character in pattern.chars() {
                match character {
                    '%' => translated.push_str(".*"),
                    '_' => translated.push('.'),
                    other => translated.push_str(&::regex::escape(&other.to_string())),
                }
            }
            translated.push_str(")$");
            ::regex::RegexBuilder::new(&translated)
                .case_insensitive(*case_insensitive)
                .size_limit(glue_state::MAX_PATTERN_PROGRAM_BYTES)
                .build()
                .map_err(|reason| {
                    Failure::user(crate::state::malformed_pattern(pattern, &reason))
                })
        }
        RelationFilter::Other(form) => Err(Failure::user(format!(
            "Unsupported filter in a SHOW statement: {form}"
        ))),
    }
}

/// A result set of one character column, named `column`, carrying `values` in order.
pub fn single_column_result(column: &str, values: Vec<String>) -> StoredResultSet {
    StoredResultSet {
        column_info: vec![super::super::query_engine::varchar_column(column)],
        rows: values
            .into_iter()
            .map(|value| Row {
                data: Some(vec![Datum {
                    var_char_value: Some(value),
                }]),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the storage-format table: a declared format and the triple the catalog
    /// records for it.
    struct StorageCase {
        /// What this row is about.
        case: &'static str,
        /// The format the statement declared, if it declared one.
        declared: Option<StorageFormat>,
        /// The input format, output format and serialization library the catalog records,
        /// or `None` where Hive has no such triple for the declared format.
        recorded: Option<(&'static str, &'static str, &'static str)>,
    }

    /// A declared storage format decides what the catalog records for a table: the
    /// formats its data is read and written with, and the serialization library a table
    /// that names none of its own is read with. The rows differ along the format declared,
    /// including the two Hive formats named directly and a format no triple exists for.
    #[test]
    fn a_declared_storage_format_decides_what_the_catalog_records() {
        let cases: Vec<StorageCase> = vec![
            StorageCase {
                case: "no declared format at all",
                declared: None,
                recorded: Some((TEXT_INPUT_FORMAT, TEXT_OUTPUT_FORMAT, LAZY_SIMPLE_SERDE)),
            },
            StorageCase {
                case: "delimited text",
                declared: Some(StorageFormat::Named("TEXTFILE".to_string())),
                recorded: Some((TEXT_INPUT_FORMAT, TEXT_OUTPUT_FORMAT, LAZY_SIMPLE_SERDE)),
            },
            StorageCase {
                case: "JSON documents",
                declared: Some(StorageFormat::Named("JSONFILE".to_string())),
                recorded: Some((TEXT_INPUT_FORMAT, TEXT_OUTPUT_FORMAT, JSON_SERDE)),
            },
            StorageCase {
                case: "a columnar format",
                declared: Some(StorageFormat::Named("PARQUET".to_string())),
                recorded: Some((PARQUET_INPUT_FORMAT, PARQUET_OUTPUT_FORMAT, PARQUET_SERDE)),
            },
            StorageCase {
                case: "the optimised row-columnar format",
                declared: Some(StorageFormat::Named("orc".to_string())),
                recorded: Some((ORC_INPUT_FORMAT, ORC_OUTPUT_FORMAT, ORC_SERDE)),
            },
            StorageCase {
                case: "Avro containers",
                declared: Some(StorageFormat::Named("AVRO".to_string())),
                recorded: Some((AVRO_INPUT_FORMAT, AVRO_OUTPUT_FORMAT, AVRO_SERDE)),
            },
            StorageCase {
                case: "sequence files",
                declared: Some(StorageFormat::Named("SEQUENCEFILE".to_string())),
                recorded: Some((
                    SEQUENCE_INPUT_FORMAT,
                    SEQUENCE_OUTPUT_FORMAT,
                    LAZY_SIMPLE_SERDE,
                )),
            },
            StorageCase {
                case: "record-columnar files",
                declared: Some(StorageFormat::Named("RCFILE".to_string())),
                recorded: Some((RCFILE_INPUT_FORMAT, RCFILE_OUTPUT_FORMAT, COLUMNAR_SERDE)),
            },
            StorageCase {
                case: "the two Hive formats named directly",
                declared: Some(StorageFormat::Formats {
                    input: "com.example.InFormat".to_string(),
                    output: "com.example.OutFormat".to_string(),
                }),
                recorded: Some((
                    "com.example.InFormat",
                    "com.example.OutFormat",
                    LAZY_SIMPLE_SERDE,
                )),
            },
            StorageCase {
                case: "a format Hive has no such triple for",
                declared: Some(StorageFormat::Named("ICEBERG".to_string())),
                recorded: None,
            },
        ];

        for case in cases {
            let actual = hive_storage(case.declared.as_ref()).map(|storage| {
                (
                    storage.input.clone(),
                    storage.output.clone(),
                    storage.serde.to_string(),
                )
            });
            let expected = case.recorded.map(|(input, output, serde)| {
                (input.to_string(), output.to_string(), serde.to_string())
            });
            assert_eq!(
                actual, expected,
                "case {:?}: expected {expected:?}, got {actual:?}",
                case.case
            );
        }
    }

    /// A `SHOW` statement's filter selects the names it narrows to: the rows differ along
    /// the pattern language the statement wrote it in, and one row is a form that narrows
    /// by something other than a pattern.
    #[test]
    fn a_show_filter_selects_the_names_it_narrows_to() {
        // (case, the filter, the names it keeps out of the three below)
        let names = ["events", "v_recent", "v_old"];
        let cases: Vec<(&str, RelationFilter, Option<Vec<&str>>)> = vec![
            (
                "the catalog's own pattern language",
                RelationFilter::Pattern("v_*".to_string()),
                Some(vec!["v_recent", "v_old"]),
            ),
            (
                "a pattern with alternatives",
                RelationFilter::Pattern("events|v_old".to_string()),
                Some(vec!["events", "v_old"]),
            ),
            (
                "a SQL pattern with a run wildcard",
                RelationFilter::Like {
                    pattern: "v%".to_string(),
                    case_insensitive: false,
                },
                Some(vec!["v_recent", "v_old"]),
            ),
            (
                "a SQL pattern with a single-character wildcard",
                RelationFilter::Like {
                    pattern: "v_o__".to_string(),
                    case_insensitive: false,
                },
                Some(vec!["v_old"]),
            ),
            (
                "a SQL pattern in another case, matched without regard to it",
                RelationFilter::Like {
                    pattern: "V_RECENT".to_string(),
                    case_insensitive: true,
                },
                Some(vec!["v_recent"]),
            ),
            (
                "a SQL pattern in another case, matched with regard to it",
                RelationFilter::Like {
                    pattern: "V_RECENT".to_string(),
                    case_insensitive: false,
                },
                Some(vec![]),
            ),
            (
                "a pattern carrying a character of the matcher's own language",
                RelationFilter::Like {
                    pattern: "v_recent.".to_string(),
                    case_insensitive: false,
                },
                Some(vec![]),
            ),
            (
                "a form that narrows by something other than a pattern",
                RelationFilter::Other("table_name = 'events'".to_string()),
                None,
            ),
        ];

        for (case, filter, expected) in cases {
            let actual = name_matcher(&filter).ok().map(|matcher| {
                names
                    .iter()
                    .filter(|name| matcher.is_match(name))
                    .copied()
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A column's type has one name in the catalog and one in the query engine, and a
    /// view's own definition records the ENGINE's: the rows differ along whether the two
    /// names agree, and whether a precision or a length qualifies the type.
    #[test]
    fn a_catalog_column_type_has_the_engines_own_name() {
        // (the Hive type the catalog records, the name the engine has for it)
        let cases: Vec<(&str, &str)> = vec![
            ("string", "varchar"),
            ("int", "integer"),
            ("integer", "integer"),
            ("bigint", "bigint"),
            ("smallint", "smallint"),
            ("tinyint", "tinyint"),
            ("float", "real"),
            ("double", "double"),
            ("boolean", "boolean"),
            ("binary", "varbinary"),
            ("date", "date"),
            ("timestamp", "timestamp"),
            ("varchar(20)", "varchar(20)"),
            ("char(3)", "char(3)"),
            ("decimal(10,2)", "decimal(10,2)"),
            ("STRING", "varchar"),
        ];

        for (declared, expected) in cases {
            let actual = engine_type_name(declared);
            assert_eq!(
                actual, expected,
                "case {declared:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
