// SPDX-License-Identifier: CC0-1.0

//! Amazon Redshift SPECTRUM: an external schema bound to a database of the AWS
//! Glue Data Catalog, and the rows its tables serve straight out of Amazon S3.
//!
//! "CREATE EXTERNAL SCHEMA … FROM DATA CATALOG" binds a local schema name to an
//! external database — "the external database exists in … the Athena data
//! catalog or the AWS Glue Data Catalog" — and the tables of that database are
//! read "directly from Amazon S3 … You don't need to load the data into Amazon
//! Redshift tables". Both halves of that are this module's:
//!
//! * the BINDING is a real schema of the database the statement ran in
//!   ([`ExternalSchemas`]), so a later statement resolves `<schema>.<table>`
//!   through it, and the external database it names is a database of the Glue
//!   Data Catalog this emulator holds — created there when the statement asks
//!   for it, so Glue's own `GetDatabase` then reports it;
//! * a QUERY that references one of those tables has the table's definition read
//!   from the catalog (its columns, the Amazon S3 location its data lives in and
//!   the serialization its rows are written with) and its rows read from the
//!   objects at that location, immediately before the query runs
//!   ([`materialise`]) — so the rows a query answers with are the objects' rows
//!   as they stand at that moment, and nothing was loaded into the database to
//!   get them. The table is put on the engine's TEMPORARY schema, which is why
//!   the database's own catalog still holds only the caller's tables.

use std::collections::BTreeMap;

use super::data_plane::DataPlane;
use super::object_store::{self, S3Failure};
use super::warehouse_statements::{ExternalSchemaStatement, ExternalTableStatement};

/// How many external schemas ONE database may hold. A caller creates them one
/// statement at a time under names it chooses, so the map is bounded as every
/// other map this service grows on request is.
pub const MAX_EXTERNAL_SCHEMAS: usize = 32;

/// How many rows ONE external table serves a query. The data is a caller's own
/// objects, bounded by what one statement may move
/// ([`object_store::MAX_TRANSFER_BYTES`]); this bounds the rows those bytes are
/// read into.
pub const MAX_EXTERNAL_ROWS: usize = 100_000;

/// How many VALUES one query reads out of an external table. A row costs one
/// value per declared column, so what is bounded is the product of the rows and
/// the width — a table declared wide serves fewer rows per query than a narrow
/// one, and neither is sized by the objects alone.
pub const MAX_EXTERNAL_CELLS: usize = 2_000_000;

/// How many columns ONE external table is served with: the widest row the engine
/// behind this emulator produces, so a definition declaring more is refused when
/// it is created rather than read into rows nothing could hold.
pub const MAX_EXTERNAL_COLUMNS: usize = 1_000;

/// What an external schema is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSchema {
    /// The Data Catalog database its tables come from.
    pub catalog_database: String,
    /// The role the catalog and the data are read under, or `None` for
    /// `IAM_ROLE default`.
    pub role_arn: Option<String>,
}

/// The external schemas ONE database holds, by lower-cased schema name.
#[derive(Debug, Default)]
pub struct ExternalSchemas(BTreeMap<String, ExternalSchema>);

impl ExternalSchemas {
    /// The binding of this schema, if the database holds one.
    pub fn binding(&self, schema: &str) -> Option<&ExternalSchema> {
        self.0.get(&schema.to_ascii_lowercase())
    }

    /// Whether the database holds an external schema of this name.
    pub fn holds(&self, schema: &str) -> bool {
        self.0.contains_key(&schema.to_ascii_lowercase())
    }

    /// Bind `schema` to an external database.
    fn bind(&mut self, schema: &str, binding: ExternalSchema) -> Result<(), String> {
        if self.0.len() >= MAX_EXTERNAL_SCHEMAS {
            return Err(format!(
                "ERROR: a database of this emulator holds at most {MAX_EXTERNAL_SCHEMAS} external \
                 schemas"
            ));
        }
        self.0.insert(schema.to_ascii_lowercase(), binding);
        Ok(())
    }

    /// Forget the binding of `schema`, for a `DROP SCHEMA` that removed it.
    pub fn unbind(&mut self, schema: &str) {
        self.0.remove(&schema.to_ascii_lowercase());
    }
}

/// Carry out one `CREATE EXTERNAL SCHEMA`.
///
/// `already_held` is whether the database's own catalogue of schemas already
/// holds a schema of that name, which is what decides the duplicate refusal.
pub fn create(
    schemas: &mut ExternalSchemas,
    plane: &DataPlane,
    statement: &ExternalSchemaStatement,
    already_held: bool,
) -> Result<(), String> {
    if already_held || schemas.holds(&statement.schema) {
        // "IF NOT EXISTS … a clause that indicates that if the specified schema
        // already exists, the command should make no changes".
        if statement.if_not_exists {
            return Ok(());
        }
        return Err(format!(
            "ERROR: Schema \"{}\" already exists",
            statement.schema
        ));
    }
    // The external database has to BE there: the schema's tables are its
    // tables. "CREATE EXTERNAL DATABASE IF NOT EXISTS" is what creates it when
    // it is not, and it is created where it lives — in the Data Catalog — so
    // Glue's own read of it reports it afterwards.
    match external_database(plane, &statement.catalog_database) {
        CatalogAnswer::Holds => {}
        CatalogAnswer::Absent if statement.create_database => {
            create_external_database(plane, &statement.catalog_database)?;
        }
        CatalogAnswer::Absent => {
            return Err(format!(
                "ERROR: External database \"{}\" does not exist in the data catalog",
                statement.catalog_database
            ))
        }
        // No Data Catalog is mounted beside this service, so whether the
        // external database exists is unknown: the schema is bound, and a query
        // over one of its tables is what then reports that the table cannot be
        // read.
        CatalogAnswer::Unresolvable => {}
    }
    schemas.bind(
        &statement.schema,
        ExternalSchema {
            catalog_database: statement.catalog_database.clone(),
            role_arn: statement.role_arn.clone(),
        },
    )
}

/// What asking the Data Catalog about one external database found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogAnswer {
    /// The catalog holds it.
    Holds,
    /// The catalog was asked and holds no such database.
    Absent,
    /// There is no Data Catalog on this engine to ask.
    Unresolvable,
}

/// Whether the Data Catalog holds the external database `database`.
fn external_database(plane: &DataPlane, database: &str) -> CatalogAnswer {
    let Some(catalog) = plane.catalog.as_ref() else {
        return CatalogAnswer::Unresolvable;
    };
    let store = catalog.lock();
    match store.database(&catalog_id(), database) {
        Some(_) => CatalogAnswer::Holds,
        None => CatalogAnswer::Absent,
    }
}

/// The Data Catalog a Redshift cluster's external schema reads: the account's
/// own, which is the catalog Glue's operations address when a request names
/// none.
fn catalog_id() -> String {
    ::service_com_amazonaws_glue_awsglue::state::resolve_catalog_id(None)
}

/// Create the external database in the Data Catalog, as `CREATE EXTERNAL
/// DATABASE IF NOT EXISTS` asks.
///
/// Written through the catalog's own store on this engine, so what this creates
/// is what Glue's `GetDatabase` and `GetTables` then report.
fn create_external_database(plane: &DataPlane, database: &str) -> Result<(), String> {
    let Some(catalog) = plane.catalog.as_ref() else {
        return Ok(());
    };
    let mut store = catalog.lock();
    if !store.can_store_database() {
        return Err(format!(
            "ERROR: the AWS Glue Data Catalog would not take the external database \"{database}\""
        ));
    }
    let catalog_id = catalog_id();
    let input = ::service_com_amazonaws_glue_awsglue::ops::types::DatabaseInput {
        name: database.to_string(),
        ..Default::default()
    };
    let entry = ::service_com_amazonaws_glue_awsglue::state::DatabaseEntry::from_input(
        &input,
        &catalog_id,
        None,
        ::std::time::SystemTime::now(),
    );
    let folded = ::service_com_amazonaws_glue_awsglue::state::fold_name(database);
    let Some(held) = store.catalog_mut_for_create(&catalog_id) else {
        return Err(format!(
            "ERROR: the AWS Glue Data Catalog would not take the external database \"{database}\""
        ));
    };
    held.databases.insert(folded, entry);
    Ok(())
}

/// Carry out one `CREATE EXTERNAL TABLE`: DEFINE the table in the external
/// database its schema is bound to.
///
/// "CREATE EXTERNAL TABLE … registers [the table] as a new external table. The
/// metadata is stored in your external data catalog": the definition is written
/// into the Data Catalog where that metadata lives, so Glue's own `GetTable`
/// reports it and a query over the table reads the objects at its `LOCATION`.
/// Nothing is written into the database itself — an external table's data stays
/// in Amazon S3.
pub fn create_table(
    schemas: &ExternalSchemas,
    plane: &DataPlane,
    statement: &ExternalTableStatement,
) -> Result<(), String> {
    let Some(binding) = schemas.binding(&statement.schema) else {
        return Err(format!(
            "ERROR: Schema \"{}\" is not an external schema of this database",
            statement.schema
        ));
    };
    let Some(catalog) = plane.catalog.as_ref() else {
        return Err("ERROR: the data catalog an external table's metadata is stored in is not \
                    reachable from this emulator"
            .to_string());
    };
    let catalog_id = catalog_id();
    let mut store = catalog.lock();
    let Some(database) = store.database_mut(&catalog_id, &binding.catalog_database) else {
        return Err(format!(
            "ERROR: External database \"{}\" does not exist in the data catalog",
            binding.catalog_database
        ));
    };
    let folded = ::service_com_amazonaws_glue_awsglue::state::fold_name(&statement.table);
    if database.tables.contains_key(&folded) {
        // "IF NOT EXISTS … if the table already exists, the command makes no
        // changes".
        if statement.if_not_exists {
            return Ok(());
        }
        return Err(format!(
            "ERROR: Table \"{}\" already exists",
            statement.table
        ));
    }
    if !database.can_store_table() {
        return Err(format!(
            "ERROR: the data catalog would not take another table in the external database \
             \"{}\"",
            binding.catalog_database
        ));
    }
    if statement.columns.len().saturating_add(statement.partitions.len()) > MAX_EXTERNAL_COLUMNS {
        return Err(format!(
            "ERROR: a table of this emulator declares at most {MAX_EXTERNAL_COLUMNS} columns"
        ));
    }
    let input = table_input(statement);
    let entry = ::service_com_amazonaws_glue_awsglue::state::TableEntry::from_input(
        &input,
        &binding.catalog_database,
        &catalog_id,
        None,
        ::std::time::SystemTime::now(),
    );
    database.tables.insert(folded, entry);
    Ok(())
}

/// The catalog definition one `CREATE EXTERNAL TABLE` describes.
///
/// Its columns, the location of its data and the serialization its rows are
/// written with go where the catalog holds them: on the table's storage
/// descriptor, which is what a reader of the table — Glue's own `GetTable`, or a
/// query over the table here — reads them back from.
fn table_input(
    statement: &ExternalTableStatement,
) -> ::service_com_amazonaws_glue_awsglue::ops::types::TableInput {
    use ::service_com_amazonaws_glue_awsglue::ops::types as glue;
    let column = |(name, declared): &(String, String)| glue::Column {
        name: name.clone(),
        r#type: Some(declared.to_ascii_lowercase()),
        ..Default::default()
    };
    let mut parameters: ::std::collections::HashMap<String, String> = statement
        .properties
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    // "EXTERNAL_TABLE" is the table type the catalog records for a table whose
    // data lives outside it, and the `external` parameter is the one a Hive
    // metastore carries for the same fact.
    parameters.insert("EXTERNAL".to_string(), "TRUE".to_string());
    let mut serde_parameters: ::std::collections::HashMap<String, String> =
        ::std::collections::HashMap::new();
    if let Some(delimiter) = statement.delimiter {
        // `field.delim` is the parameter Hive's delimited serialization holds a
        // field delimiter under, which is what a reader of the table reads.
        serde_parameters.insert("field.delim".to_string(), delimiter.to_string());
    }
    for (key, value) in &statement.properties {
        serde_parameters.insert(key.clone(), value.clone());
    }
    glue::TableInput {
        name: statement.table.clone(),
        table_type: Some("EXTERNAL_TABLE".to_string()),
        storage_descriptor: Some(glue::StorageDescriptor {
            columns: Some(statement.columns.iter().map(column).collect()),
            location: Some(statement.location.clone()),
            serde_info: Some(glue::SerDeInfo {
                name: None,
                serialization_library: statement.serde_library.clone(),
                parameters: Some(serde_parameters),
            }),
            parameters: Some(parameters.clone()),
            ..Default::default()
        }),
        partition_keys: Some(statement.partitions.iter().map(column).collect()),
        parameters: Some(parameters),
        ..Default::default()
    }
}

/// One external table, as the Data Catalog defines it.
#[derive(Debug, Clone)]
struct ExternalTable {
    /// Its columns, in catalog order: each name with the type the catalog
    /// declares for it.
    columns: Vec<(String, String)>,
    /// The Amazon S3 location its data lives in.
    location: String,
    /// The field delimiter its rows are written with.
    delimiter: char,
    /// The quote character a field may be wrapped in, for a serialization that
    /// quotes.
    quote: Option<char>,
    /// Whether its rows are JSON documents rather than delimited text.
    json: bool,
    /// How many leading lines of each object are a header
    /// (`skip.header.line.count`).
    skip_header: usize,
}

/// The CATALOG VIEWS a Spectrum client reads an external schema's own
/// definitions through, by the name a statement references them under.
///
/// Source: the Amazon Redshift *System tables and views reference* — these are
/// the views that report "the external schemas", "the tables in external
/// schemas" and "the columns of the tables in external schemas". Each one is a
/// read of the external catalog, so each is served from the catalog itself.
pub const CATALOG_VIEWS: &[&str] = &[
    "svv_external_schemas",
    "svv_external_tables",
    "svv_external_columns",
];

/// The catalog views `relations` names.
pub fn referenced_views(relations: &[String]) -> Vec<&'static str> {
    CATALOG_VIEWS
        .iter()
        .copied()
        .filter(|view| {
            relations.iter().any(|relation| {
                relation.eq_ignore_ascii_case(view)
                    || relation
                        .rsplit('.')
                        .next()
                        .is_some_and(|name| name.eq_ignore_ascii_case(view))
            })
        })
        .collect()
}

/// Put `views` on the connection's temporary schema, holding the external
/// schemas this database is bound to and the tables and columns the Data
/// Catalog defines for them NOW.
///
/// Rebuilt per statement, because the catalog has moved on since the last
/// statement read it: a table another client created in the external database is
/// there the next time the view is read, exactly as it is on the real service.
pub fn materialise_views(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    schemas: &ExternalSchemas,
    views: &[&'static str],
) -> rusqlite::Result<()> {
    for view in views {
        match *view {
            "svv_external_schemas" => write_schema_view(connection, schemas)?,
            "svv_external_tables" => write_table_view(connection, plane, schemas)?,
            "svv_external_columns" => write_column_view(connection, plane, schemas)?,
            _ => {}
        }
    }
    Ok(())
}

/// `SVV_EXTERNAL_SCHEMAS`: "all external schemas and their underlying external
/// databases".
fn write_schema_view(
    connection: &rusqlite::Connection,
    schemas: &ExternalSchemas,
) -> rusqlite::Result<()> {
    create_view(
        connection,
        "svv_external_schemas",
        "esoid INTEGER, eskind INTEGER, schemaname VARCHAR(128), esowner INTEGER, \
         databasename VARCHAR(128), esoptions VARCHAR(256)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.svv_external_schemas (esoid, eskind, schemaname, esowner, \
         databasename, esoptions) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (index, (schema, binding)) in schemas.0.iter().enumerate() {
        insert.execute(rusqlite::params![
            schema_oid(index),
            // "eskind: the type of the external catalog": 1 for a Data Catalog,
            // which is the catalog these schemas are bound to.
            DATA_CATALOG_KIND,
            schema,
            SCHEMA_OWNER_ID,
            binding.catalog_database,
            // "esoptions: external schema options", which for a Data Catalog
            // schema carries the role the catalog is read under.
            ::serde_json::json!({"IAM_ROLE": binding.role_arn.clone().unwrap_or_default()})
                .to_string(),
        ])?;
    }
    Ok(())
}

/// `SVV_EXTERNAL_TABLES`: "all tables in external schemas".
fn write_table_view(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    schemas: &ExternalSchemas,
) -> rusqlite::Result<()> {
    create_view(
        connection,
        "svv_external_tables",
        "esoid INTEGER, schemaname VARCHAR(128), tablename VARCHAR(128), \
         location VARCHAR(1024), input_format VARCHAR(256), output_format VARCHAR(256), \
         serialization_lib VARCHAR(256), serde_parameters VARCHAR(1024), compressed INTEGER, \
         parameters VARCHAR(1024)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.svv_external_tables (esoid, schemaname, tablename, location, \
         input_format, output_format, serialization_lib, serde_parameters, compressed, \
         parameters) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for (index, (schema, binding)) in schemas.0.iter().enumerate() {
        for table in catalog_tables(plane, binding) {
            let descriptor = table.storage_descriptor.clone().unwrap_or_default();
            insert.execute(rusqlite::params![
                schema_oid(index),
                schema,
                table.name,
                descriptor.location.clone().unwrap_or_default(),
                descriptor.input_format.clone().unwrap_or_default(),
                descriptor.output_format.clone().unwrap_or_default(),
                descriptor
                    .serde_info
                    .as_ref()
                    .and_then(|serde| serde.serialization_library.clone())
                    .unwrap_or_default(),
                properties(
                    descriptor
                        .serde_info
                        .as_ref()
                        .and_then(|serde| serde.parameters.clone())
                ),
                i64::from(descriptor.compressed),
                properties(table.parameters.clone()),
            ])?;
        }
    }
    Ok(())
}

/// `SVV_EXTERNAL_COLUMNS`: "all columns in external tables".
fn write_column_view(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    schemas: &ExternalSchemas,
) -> rusqlite::Result<()> {
    create_view(
        connection,
        "svv_external_columns",
        "schemaname VARCHAR(128), tablename VARCHAR(128), columnname VARCHAR(128), \
         external_type VARCHAR(128), columnnum INTEGER, part_key INTEGER, \
         is_nullable VARCHAR(8)",
    )?;
    let mut insert = connection.prepare(
        "INSERT INTO temp.svv_external_columns (schemaname, tablename, columnname, \
         external_type, columnnum, part_key, is_nullable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (schema, binding) in &schemas.0 {
        for table in catalog_tables(plane, binding) {
            let descriptor = table.storage_descriptor.clone().unwrap_or_default();
            let columns = descriptor.columns.clone().unwrap_or_default();
            let partitions = table.partition_keys.clone().unwrap_or_default();
            for (number, column) in columns.iter().chain(partitions.iter()).enumerate() {
                insert.execute(rusqlite::params![
                    schema,
                    table.name,
                    column.name,
                    column.r#type.clone().unwrap_or_default(),
                    // "columnnum: the position of the column in the table", from
                    // one.
                    i64::try_from(number + 1).unwrap_or(i64::MAX),
                    // "part_key: whether the column is a partition key", which
                    // the keys following the table's own columns are.
                    i64::from(number >= columns.len()),
                    // Every external column is nullable: the catalog declares no
                    // NOT NULL for one.
                    "true",
                ])?;
            }
        }
    }
    Ok(())
}

/// The tables the Data Catalog holds in one external schema's database, in name
/// order.
fn catalog_tables(
    plane: &DataPlane,
    binding: &ExternalSchema,
) -> Vec<::service_com_amazonaws_glue_awsglue::state::TableEntry> {
    let Some(catalog) = plane.catalog.as_ref() else {
        return Vec::new();
    };
    let store = catalog.lock();
    store
        .database(&catalog_id(), &binding.catalog_database)
        .map(|database| {
            database
                .tables_ordered()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// A property map as a catalog view reports one: the JSON document of its
/// entries, so a reader sees what the catalog holds.
fn properties(
    parameters: Option<::std::collections::HashMap<String, String>>,
) -> String {
    let held: ::std::collections::BTreeMap<String, String> =
        parameters.unwrap_or_default().into_iter().collect();
    ::serde_json::json!(held).to_string()
}

/// The object id a catalog view reports for one external schema. The ids are
/// this database's own, handed out in the order it holds its schemas, and a
/// schema keeps the same id for as long as the database holds it.
fn schema_oid(index: usize) -> i64 {
    FIRST_SCHEMA_OID.saturating_add(i64::try_from(index).unwrap_or(0))
}

/// The id the first external schema of a database is given, above the system
/// objects' own ids.
const FIRST_SCHEMA_OID: i64 = 200_000;

/// `eskind` for a schema bound to a Data Catalog database.
const DATA_CATALOG_KIND: i64 = 1;

/// `esowner` for a schema a database user created: the user id this emulator's
/// database user runs under ([`super::system_tables::DATABASE_USER_ID`]).
const SCHEMA_OWNER_ID: i64 = super::system_tables::DATABASE_USER_ID;

/// Create one catalog view EMPTY on the connection's temporary schema,
/// replacing what a previous statement left there.
fn create_view(
    connection: &rusqlite::Connection,
    view: &str,
    columns: &str,
) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "DROP TABLE IF EXISTS temp.{view}; CREATE TEMP TABLE {view} ({columns});"
    ))
}

/// Put every external table `relations` names on the connection's temporary
/// schema, holding the rows the objects at its location carry now.
///
/// `relations` are the names a statement references, as the caller wrote them; a
/// name this database has no external schema for is not an external table and is
/// left alone. The error is the one the warehouse states for a Spectrum table it
/// cannot read.
pub fn materialise(
    connection: &rusqlite::Connection,
    plane: &DataPlane,
    schemas: &ExternalSchemas,
    relations: &[String],
) -> Result<(), String> {
    for relation in relations {
        let Some((schema, table)) = relation.split_once('.') else {
            continue;
        };
        let Some(binding) = schemas.binding(schema) else {
            continue;
        };
        let Some(definition) = catalog_table(plane, binding, table) else {
            // The catalog holds no such table, so the engine is left to report
            // the relation it cannot find — the same answer a query over a
            // table nothing defines gets.
            continue;
        };
        let rows = external_rows(plane, binding, &definition)
            .map_err(|failure| failure.message())?;
        write_relation(connection, schema, table, &definition, &rows)
            .map_err(|error| format!("ERROR: {error}"))?;
    }
    Ok(())
}

/// The definition the Data Catalog holds for one table of an external
/// database.
fn catalog_table(
    plane: &DataPlane,
    binding: &ExternalSchema,
    table: &str,
) -> Option<ExternalTable> {
    let catalog = plane.catalog.as_ref()?;
    let store = catalog.lock();
    let held = store
        .database(&catalog_id(), &binding.catalog_database)?
        .tables
        .values()
        .find(|held| held.name.eq_ignore_ascii_case(table))?;
    let descriptor = held.storage_descriptor.as_ref()?;
    let columns: Vec<(String, String)> = descriptor
        .columns
        .as_ref()?
        .iter()
        // Bounded, because the definition may have been written through the
        // catalog's own API rather than by a statement this service read.
        .take(MAX_EXTERNAL_COLUMNS)
        .map(|column| {
            (
                column.name.clone(),
                column.r#type.clone().unwrap_or_default(),
            )
        })
        .collect();
    let parameters = descriptor
        .serde_info
        .as_ref()
        .and_then(|serde| serde.parameters.clone())
        .unwrap_or_default();
    let library = descriptor
        .serde_info
        .as_ref()
        .and_then(|serde| serde.serialization_library.clone())
        .unwrap_or_default();
    // The parameters each serialization library names its delimiter under:
    // `field.delim` for Hive's own delimited reader and `separatorChar` for the
    // CSV one, with `quoteChar` for the quoting that reader does.
    let delimiter = parameters
        .get("field.delim")
        .or_else(|| parameters.get("separatorChar"))
        .and_then(|value| value.chars().next())
        .unwrap_or(DEFAULT_HIVE_DELIMITER);
    let quote = parameters
        .get("quoteChar")
        .and_then(|value| value.chars().next())
        .or_else(|| library.contains("OpenCSVSerde").then_some('"'));
    let skip_header = descriptor
        .parameters
        .as_ref()
        .and_then(|table_parameters| table_parameters.get("skip.header.line.count").cloned())
        .or_else(|| parameters.get("skip.header.line.count").cloned())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Some(ExternalTable {
        columns,
        location: descriptor.location.clone().unwrap_or_default(),
        delimiter,
        quote,
        json: library.to_ascii_lowercase().contains("json"),
        skip_header,
    })
}

/// The field delimiter Hive's delimited serialization uses when the table names
/// none: the `^A` control character, which is the default `field.delim` of
/// `LazySimpleSerDe`.
const DEFAULT_HIVE_DELIMITER: char = '\u{1}';

/// The rows the objects at an external table's location carry.
fn external_rows(
    plane: &DataPlane,
    binding: &ExternalSchema,
    table: &ExternalTable,
) -> Result<Vec<Vec<Option<String>>>, S3Failure> {
    let address =
        object_store::split_uri(&table.location).ok_or(S3Failure::NoSuchBucket)?;
    let objects =
        object_store::read_objects_under(plane, binding.role_arn.as_deref(), &address)?;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let width = table.columns.len().max(1);
    // The budget bounds the WORK and not only the answer: each object is read
    // for at most the rows the query may still be served, counted in the VALUES
    // a row of this table costs, so a reader never builds more cells than it
    // keeps.
    let served = MAX_EXTERNAL_ROWS.min(MAX_EXTERNAL_CELLS / width);
    for (_, body) in objects {
        let remaining = served.saturating_sub(rows.len());
        if remaining == 0 {
            return Err(S3Failure::TooMuch(format!(
                "an external table serves at most {served} rows per query in this emulator, of \
                 its {MAX_EXTERNAL_CELLS} values"
            )));
        }
        rows.extend(object_rows(table, &body, remaining));
    }
    Ok(rows)
}

/// The rows one object carries, read with the serialization the catalog
/// declares — at most `limit` of them, which is what is left of the query's own
/// budget of rows and values.
fn object_rows(
    table: &ExternalTable,
    body: &[u8],
    limit: usize,
) -> Vec<Vec<Option<String>>> {
    if table.json {
        return String::from_utf8_lossy(body)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(limit)
            .map(|line| json_row(table, line))
            .collect();
    }
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(u8::try_from(table.delimiter as u32).unwrap_or(b'\x01'))
        .quote(
            table
                .quote
                .and_then(|quote| u8::try_from(quote as u32).ok())
                .unwrap_or(b'"'),
        )
        .quoting(table.quote.is_some())
        .has_headers(false)
        .flexible(true)
        .from_reader(body);
    reader
        .records()
        .skip(table.skip_header)
        .filter_map(Result::ok)
        .take(limit)
        .map(|record| {
            table
                .columns
                .iter()
                .enumerate()
                .map(|(index, _)| record.get(index).map(str::to_string))
                .collect()
        })
        .collect()
}

/// One JSON document as a row of the table's columns, each column taking the
/// member of its own name.
fn json_row(table: &ExternalTable, line: &str) -> Vec<Option<String>> {
    let document: ::serde_json::Value = ::serde_json::from_str(line).unwrap_or_default();
    table
        .columns
        .iter()
        .map(|(name, _)| {
            document
                .get(name)
                .map(|value| match value {
                    ::serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
        })
        .collect()
}

/// Write one external table and its rows onto the connection's temporary
/// schema, under the engine name a qualified reference to it resolves to.
fn write_relation(
    connection: &rusqlite::Connection,
    schema: &str,
    table: &str,
    definition: &ExternalTable,
    rows: &[Vec<Option<String>>],
) -> rusqlite::Result<()> {
    let engine_name = format!(
        "{}.{}",
        schema.to_ascii_lowercase(),
        table.to_ascii_lowercase()
    );
    let quoted = engine_name.replace('"', "\"\"");
    let columns: Vec<String> = definition
        .columns
        .iter()
        .map(|(name, declared)| {
            format!(
                "\"{}\" {}",
                name.replace('"', "\"\""),
                engine_type(declared)
            )
        })
        .collect();
    connection.execute_batch(&format!(
        "DROP TABLE IF EXISTS temp.\"{quoted}\"; CREATE TEMP TABLE \"{quoted}\" ({});",
        columns.join(", ")
    ))?;
    if rows.is_empty() {
        return Ok(());
    }
    let placeholders: Vec<String> = (1..=definition.columns.len())
        .map(|index| format!("?{index}"))
        .collect();
    let mut insert = connection.prepare(&format!(
        "INSERT INTO temp.\"{quoted}\" VALUES ({})",
        placeholders.join(", ")
    ))?;
    for row in rows {
        let bound: Vec<&dyn rusqlite::ToSql> = row
            .iter()
            .map(|value| value as &dyn rusqlite::ToSql)
            .collect();
        insert.execute(bound.as_slice())?;
    }
    Ok(())
}

/// The type the engine declares a column of this CATALOG type with.
///
/// Source: the Amazon Redshift *Data types* reference and the Hive types a Data
/// Catalog column carries — the external table's `string` is a character
/// column, its `int` an integer one, and so on. The declared type is what makes
/// a value read out of a text object compare and aggregate as a number rather
/// than as text.
fn engine_type(declared: &str) -> &'static str {
    let name = declared
        .split(['(', '<', ' '])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match name.as_str() {
        "tinyint" | "smallint" | "int2" => "SMALLINT",
        "int" | "integer" | "int4" => "INTEGER",
        "bigint" | "int8" => "BIGINT",
        "float" | "real" | "float4" => "REAL",
        "double" | "double precision" | "float8" => "DOUBLE PRECISION",
        "decimal" | "numeric" => "DECIMAL",
        "boolean" | "bool" => "BOOLEAN",
        "date" => "DATE",
        "timestamp" => "TIMESTAMP",
        // Every other catalog type — a string, a character type, a nested
        // `struct`, `array` or `map`, a binary — is served as the text the
        // object carries for it.
        _ => "VARCHAR",
    }
}
