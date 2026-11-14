// SPDX-License-Identifier: CC0-1.0

//! Restoring ONE TABLE of a captured warehouse into a live one.
//!
//! `RestoreTableFromClusterSnapshot` "creates a new table from a table in an
//! Amazon Redshift cluster snapshot" and "you must create the new table within
//! the Amazon Redshift cluster that the snapshot was taken from"
//! (`smithy_query(query="doc", operation="RestoreTableFromClusterSnapshot")`), so
//! the work is a copy from a [`WarehouseCapture`] into the databases of a live
//! warehouse — one table of one database, not the whole warehouse
//! [`restore_warehouse`] writes back.
//!
//! It lives HERE, beside the capture and the budget, for the same reason the
//! whole-warehouse pair does: the SQL engine the databases live in belongs to
//! this crate, so a control plane that restores a table asks for it through this
//! seam and holds an opaque capture. Nothing is reported as restored unless the
//! engine really wrote it — the answer says which of the four things the request
//! named was not there, and a control plane turns that into its own modelled
//! fault.

use super::*;

/// How many rows one table restore copies before it is given up on, unless the
/// control plane asking for the restore holds it to fewer.
///
/// The emulator's own bound, like [`MAX_DATABASE_PAGES`] around it: the copy
/// streams a row at a time, so this bounds the WORK one request can ask for
/// rather than the memory it holds, and it is far above any table an emulator
/// serves.
///
/// PUBLIC so the control plane that asks for a restore passes it in as the bound
/// the copy is held to (`redshift`'s `state/cluster_data.rs` does), which keeps
/// the bound stated at the call it governs rather than only here.
pub const MAX_RESTORED_TABLE_ROWS: i64 = 200_000;

/// How often the copy checks its deadline while streaming rows.
const ROWS_PER_DEADLINE_CHECK: i64 = 1_000;

/// Which table of which databases a restore moves, and what to call the copy.
///
/// The members are `RestoreTableFromClusterSnapshot`'s own: the source database,
/// schema and table the snapshot holds, and the database, schema and name the new
/// table is created under.
#[derive(Debug, Clone)]
pub struct TableRestoreRequest {
    /// "The name of the source database that contains the table to restore from."
    pub source_database: String,
    /// "The name of the source schema that contains the table to restore from",
    /// and `None` for a request that named none — which is the default schema.
    pub source_schema: Option<String>,
    /// "The name of the source table to restore from."
    pub source_table: String,
    /// "The name of the database to restore the table to", and `None` for a
    /// request that named none — which is the source database.
    pub target_database: Option<String>,
    /// "The name of the schema to restore the table to", and `None` for a request
    /// that named none — which is the source schema.
    pub target_schema: Option<String>,
    /// "The name of the table to create as a result of the current request."
    pub new_table: String,
}

/// What a table restore did, or which of the names the request carried resolved
/// to nothing.
///
/// The four negative answers are the four things a restore can fail on that are
/// about the REQUEST rather than about the engine, so the control plane that asked
/// for it can answer each with its own modelled fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableRestored {
    /// The new table is in the target database, holding this many rows.
    Restored {
        /// How many rows the copy wrote.
        rows: i64,
    },
    /// The capture holds no database of that name, so there is no table to copy.
    SourceDatabaseAbsent,
    /// The captured database holds no such table, under the schema the request
    /// named.
    SourceTableAbsent,
    /// The live warehouse holds no database of that name to restore into.
    TargetDatabaseAbsent,
    /// The target database holds no schema of that name, so the new table has
    /// nowhere to be created.
    TargetSchemaAbsent,
    /// The target database already holds a table under the new name, and a
    /// restore creates a NEW table rather than replacing one.
    NewTableExists,
    /// The source table holds more rows than this restore copies (the bound the
    /// asking control plane set, capped by [`MAX_RESTORED_TABLE_ROWS`]).
    SourceTableTooLarge {
        /// The bound the table is past.
        limit: i64,
    },
}

/// Copy one table of `capture` into the live databases of `target`, writing at
/// most `row_limit` rows.
///
/// BLOCKING, deliberately, exactly as [`restore_warehouse`] is: it drives the SQL
/// engine, so the CALLER must put it on a blocking pool
/// (`tokio::task::spawn_blocking`) and bound the wait for it, which is what the
/// control plane's own seam does. Its own deadline is [`COPY_TIMEOUT`], checked
/// while waiting for a database and between batches of rows, so it always gives
/// up before a request that waits [`COPY_REQUEST_BUDGET`] for it does.
///
/// `row_limit` is the row bound the asking control plane holds this copy to,
/// capped by [`MAX_RESTORED_TABLE_ROWS`] so no caller can ask for an unbounded
/// one.
///
/// The copy is made with SQL on the two connections, because the two ends are
/// different databases and the engine's page-level copy moves a whole database:
/// the new table is created with the source table's COLUMNS and the source rows
/// are streamed into it, so the values keep the storage class the engine held them
/// under. Nothing else of the target database is touched, and a copy that cannot
/// be completed leaves no half-written table behind — the whole write is one
/// engine transaction.
pub fn restore_table<Eng: Send + Sync + 'static>(
    engine: &Eng,
    capture: &WarehouseCapture,
    target: &ConnectionTarget,
    request: &TableRestoreRequest,
    row_limit: i64,
) -> Result<TableRestored, WarehouseUnavailable> {
    let Some(state) = service_state(engine) else {
        return Err(WarehouseUnavailable::StoreUnreachable);
    };
    let deadline = Instant::now() + COPY_TIMEOUT;

    let Some(source) = capture.database(&request.source_database) else {
        return Ok(TableRestored::SourceDatabaseAbsent);
    };
    // "If you omit the TargetDatabaseName, the table is restored to the source
    // database": the target database defaults to the source's.
    let target_database = request
        .target_database
        .clone()
        .unwrap_or_else(|| request.source_database.clone());
    let handles = state.lock().warehouse_handles(target);
    let Some((_, destination)) = handles
        .into_iter()
        .find(|(database, _)| database == &target_database)
    else {
        return Ok(TableRestored::TargetDatabaseAbsent);
    };

    let source = lock_database_until(&source, deadline)
        .ok_or(WarehouseUnavailable::TimedOut(COPY_TIMEOUT))?;
    let mut destination = lock_database_until(&destination, deadline)
        .ok_or(WarehouseUnavailable::TimedOut(COPY_TIMEOUT))?;

    // Which ENGINE relation each name addresses, under its own database's
    // catalogue of schemas — the one rule a statement's table name is resolved by
    // ([`redshift_sql::engine_table_name`]), so a restore reaches exactly the
    // table a `SELECT` of the same name would.
    let Some(source_relation) = relation_in(&source, request.source_schema.as_deref(), &request.source_table)
    else {
        return Ok(TableRestored::SourceTableAbsent);
    };
    // "If you omit the TargetSchemaName, the table is restored to the source
    // schema."
    let target_schema = request
        .target_schema
        .clone()
        .or_else(|| request.source_schema.clone());
    let Some(new_relation) = relation_in(&destination, target_schema.as_deref(), &request.new_table)
    else {
        return Ok(TableRestored::TargetSchemaAbsent);
    };

    let columns = match table_columns(&source, &source_relation) {
        Ok(Some(columns)) => columns,
        Ok(None) => return Ok(TableRestored::SourceTableAbsent),
        Err(error) => return Err(engine_failure(error)),
    };
    match table_columns(&destination, &new_relation) {
        // A restore creates a NEW table, so a name the target database already
        // holds is not one it can create.
        Ok(Some(_)) => return Ok(TableRestored::NewTableExists),
        Ok(None) => {}
        Err(error) => return Err(engine_failure(error)),
    }

    copy_rows(
        &source,
        &mut destination,
        &source_relation,
        &new_relation,
        &columns,
        deadline,
        row_limit.clamp(0, MAX_RESTORED_TABLE_ROWS),
    )
}

/// The engine relation `schema`.`table` addresses in `database`, or `None` when
/// that database holds no such schema — in which case the name reaches no table of
/// it at all.
///
/// A request that names no schema addresses the database's default schema, which
/// is where an unqualified name lives.
fn relation_in(database: &Database, schema: Option<&str>, table: &str) -> Option<String> {
    let Some(schema) = schema else {
        return Some(redshift_sql::engine_table_name(
            &[table.to_string()],
            &database.schemas,
        ));
    };
    if !schema.eq_ignore_ascii_case(DEFAULT_SCHEMA)
        && !redshift_sql::holds_schema(&database.schemas, schema)
    {
        return None;
    }
    Some(redshift_sql::engine_table_name(
        &[schema.to_string(), table.to_string()],
        &database.schemas,
    ))
}

/// The column names of `relation`, or `None` when the database holds no such
/// table.
///
/// Read off the engine's own prepared statement rather than from a catalogue
/// query: preparing `SELECT * FROM <relation>` is what decides whether the table
/// is there, and the statement it compiles carries the columns it would report.
fn table_columns(database: &Database, relation: &str) -> rusqlite::Result<Option<Vec<String>>> {
    match database
        .connection
        .prepare(&format!("SELECT * FROM {} LIMIT 0", quoted(relation)))
    {
        Ok(statement) => Ok(Some(statement.column_names().map(str::to_string).collect())),
        // The engine reports a name it cannot resolve as an error on PREPARE,
        // which is this question's negative answer rather than a failure.
        Err(rusqlite::Error::SqliteFailure(_, Some(ref message)))
            if message.contains("no such table") =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Create `new_relation` with `columns` in `destination` and stream the rows of
/// `source_relation` into it.
///
/// One engine transaction, so a copy that cannot be completed leaves the target
/// database exactly as it found it. The deadline is checked every
/// [`ROWS_PER_DEADLINE_CHECK`] rows, which is the cancellation point a bounded
/// request needs, and the row count is bounded by `row_limit` before any further
/// row is written.
fn copy_rows(
    source: &Database,
    destination: &mut Database,
    source_relation: &str,
    new_relation: &str,
    columns: &[String],
    deadline: Instant,
    row_limit: i64,
) -> Result<TableRestored, WarehouseUnavailable> {
    // The columns are declared without a type, which is what the engine stores a
    // value's own storage class under: the copy moves the source's values, so it
    // declares no type of its own for them.
    let declared = columns
        .iter()
        .map(|column| quoted(column))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (0..columns.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");

    let written = {
        let transaction = destination
            .connection
            .unchecked_transaction()
            .map_err(engine_failure)?;
        transaction
            .execute(
                &format!("CREATE TABLE {} ({declared})", quoted(new_relation)),
                [],
            )
            .map_err(engine_failure)?;

        let mut read = source
            .connection
            .prepare(&format!("SELECT * FROM {}", quoted(source_relation)))
            .map_err(engine_failure)?;
        let mut rows = read.query([]).map_err(engine_failure)?;
        let mut insert = transaction
            .prepare(&format!(
                "INSERT INTO {} ({declared}) VALUES ({placeholders})",
                quoted(new_relation)
            ))
            .map_err(engine_failure)?;

        let mut written: i64 = 0;
        while let Some(row) = rows.next().map_err(engine_failure)? {
            if written % ROWS_PER_DEADLINE_CHECK == 0 && Instant::now() >= deadline {
                return Err(WarehouseUnavailable::TimedOut(COPY_TIMEOUT));
            }
            if written >= row_limit {
                // The transaction is dropped unfinished, so the table this copy
                // created is not in the target database: a restore the emulator
                // will not make leaves nothing behind.
                return Ok(TableRestored::SourceTableTooLarge { limit: row_limit });
            }
            let values: Vec<rusqlite::types::Value> = (0..columns.len())
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<_>>()
                .map_err(engine_failure)?;
            insert
                .execute(rusqlite::params_from_iter(values))
                .map_err(engine_failure)?;
            written += 1;
        }
        drop(insert);
        transaction.commit().map_err(engine_failure)?;
        written
    };

    Ok(TableRestored::Restored { rows: written })
}

/// One identifier as the engine reads it: double-quoted, with an embedded quote
/// doubled, so a name carrying a quote or a dot cannot change the statement it
/// travels in.
fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The engine failure a copy could not be made through, with any host path
/// scrubbed out of it exactly as every other engine error of this crate is.
fn engine_failure(error: rusqlite::Error) -> WarehouseUnavailable {
    WarehouseUnavailable::Engine(scrub_paths(&error.to_string()))
}
