// SPDX-License-Identifier: CC0-1.0

//! RUNNING the statements a step's SQL client submits, against the cluster's Hive metastore
//! and the data that metastore's tables point at.
//!
//! "command-runner.jar … lets you run commands on your cluster" (Amazon EMR documentation),
//! and one of the commands an Amazon EMR cluster provides is its SQL client
//! ([`super::step_sql_cli`], the program this module answers for). What such a client DOES is
//! run statements, so this is where they are run:
//!
//! * a statement whose whole effect is a change to the metastore is APPLIED to it — the AWS
//!   Glue Data Catalog, for a cluster whose configuration selects it
//!   ([`super::uses_glue_data_catalog`]) — through the one place this crate applies one
//!   ([`super::step_metastore_ddl`]);
//! * a statement whose only effect is READING is COMPUTED: every relation it names is
//!   resolved in that catalog, the objects the relation's location (and the location of each
//!   of its partitions) holds are read out of this emulator's own Amazon S3, decoded as the
//!   table's own SerDe says they are laid out, loaded into this workspace's in-process SQL
//!   engine and the statement is run over them. What the client prints is what that engine
//!   answered — never a value synthesised from the statement.
//!
//! The two readings a query over a Data Catalog table needs — what the objects at a location
//! hold, and how a relation is handed to the engine — are the workspace's ONE reading of
//! each ([`::service_common::catalog_table_rows`]); nothing about either is decided here.
//!
//! Everything else is a statement nothing here performs, and the client answers for nothing
//! of it ([`SqlFault::NotPerformed`]), so its step reports that its work was not run rather
//! than a success for an effect nobody applied.

use ::std::collections::HashMap;
use ::std::time::SystemTime;

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use ::service_common::catalog_table_rows as catalog_rows;
use ::service_common::sql_wire_planes::sql_engine::{Cell, DatabaseEngine, EngineView, ResultSet};
use sqlparser::ast::{visit_relations_mut, ObjectName, ObjectNamePart, Statement};

use super::*;

/// How many relations ONE statement may read. CHOSEN: a bound on the catalog lookups and the
/// object reads one statement can cost.
const MAX_RELATIONS_PER_STATEMENT: usize = 8;

/// How many partitions of one relation are read. CHOSEN: a bound on how many locations one
/// relation's data is gathered from.
const MAX_PARTITIONS_READ: usize = 64;

/// How many objects are read from ONE location. CHOSEN: a bound on the objects one relation's
/// data is gathered from, beside the bytes each object carries (which is the transfer's own
/// bound, [`MAX_TRANSFERRED_BYTES`]).
const MAX_OBJECTS_PER_LOCATION: usize = 32;

/// How many rows of one relation one statement reads. CHOSEN: a bound on what this process
/// holds for one statement.
const MAX_ROWS_PER_RELATION: usize = 20_000;

/// How many rows of an ANSWER one client invocation prints, beside the engine's own ceiling on
/// the rows and the bytes one answer may hold at all
/// (`sql_engine::MAX_RESULT_ROWS`/`MAX_RESULT_BYTES`). CHOSEN: what a step's command writes
/// reaches its own log files ([`super::step_logs`], which keep a bounded amount of it), so what
/// is RENDERED here is bounded too.
const MAX_ROWS_ANSWERED: usize = 10_000;

/// How many bytes of printed answer one client invocation writes, for the same reason. CHOSEN,
/// in the units the step's log files are bounded in.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// How a row of an answer is written: Hive's own column separator for the records a client
/// prints.
const COLUMN_SEPARATOR: char = '\t';

/// What a printed cell carrying no value holds, as a Hive client writes one.
const NULL_CELL: &str = "NULL";

/// What one statement of a step's SQL client ANSWERED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StatementAnswer {
    /// A change the metastore applied. Such a statement answers no rows.
    Applied,
    /// The rows a query answered, each already rendered as the client prints it.
    Rows(Vec<String>),
}

/// The same, as the ENGINE answered it: the rows with their columns and their values, before
/// any one caller renders them.
///
/// A SQL client prints them ([`printed_rows`]); a Spark session hands them to the program that
/// submitted the statement as the columns and values they are
/// ([`super::spark_session_sql`]), which is why the engine's own answer is what this carries.
pub(super) enum StatementResult {
    /// A change the metastore applied.
    Applied,
    /// What the engine answered: its columns, and its rows in the order it answered them.
    Answered(ResultSet),
}

/// Why a statement answered nothing.
#[derive(Debug, Clone)]
pub(super) enum SqlFault {
    /// The statement RAN and was refused: the driver's own error, which is what the step
    /// reports when the command line's client exits on it.
    Driver(StepFailure),
    /// Nothing performed the statement at all — there is no store to read the relation's data
    /// out of, or the data is past a bound of this emulator — so there is no answer about it.
    /// The client answers for nothing of the invocation, and its step reports that its work
    /// was not run.
    NotPerformed,
}

impl SqlFault {
    /// The driver error `message` is, for a refusal the statement's own run answered.
    fn driver(reason: &str, message: String) -> Self {
        Self::Driver(StepFailure {
            reason: reason.to_string(),
            message,
        })
    }
}

/// Run ONE statement and answer what it came to.
///
/// `database` is the session's current database, which an unqualified name resolves in;
/// `objects` is the store the relations' data is read from, and a statement that needs it
/// where there is none is performed by nothing. `access` is what the role the step runs as may
/// do in the metastore ([`super::step_catalog_access`]): reaching the catalog is a call of
/// that role, so a table whose resolution its permissions deny — and a change they deny — is
/// refused with the catalog's own error BEFORE anything is read or written, exactly as the
/// metastore client is refused.
pub(super) fn run_statement(
    catalog: &::engine::ServiceState<glue_state::ComAmazonawsGlueAwsglueState>,
    objects: Option<&StepObjectCli>,
    access: Option<&StepCatalogAccess>,
    statement: &Statement,
    database: &str,
    now: SystemTime,
) -> Result<StatementAnswer, SqlFault> {
    match run_statement_for(catalog, objects, access, statement, database, now)? {
        StatementResult::Applied => Ok(StatementAnswer::Applied),
        StatementResult::Answered(answered) => {
            printed_rows(&answered.rows).map(StatementAnswer::Rows)
        }
    }
}

/// Run ONE statement and answer what the ENGINE answered, for a caller that reports a
/// statement's answer as the columns and values it holds rather than as printed lines
/// ([`StatementResult`]). Every decision above is this function's; [`run_statement`] is this
/// one with the answer rendered the way a SQL client prints it.
pub(super) fn run_statement_for(
    catalog: &::engine::ServiceState<glue_state::ComAmazonawsGlueAwsglueState>,
    objects: Option<&StepObjectCli>,
    access: Option<&StepCatalogAccess>,
    statement: &Statement,
    database: &str,
    now: SystemTime,
) -> Result<StatementResult, SqlFault> {
    let catalog_id = glue_state::resolve_catalog_id(None);
    if let Some(change) = metastore_change(statement, database) {
        if let Some(refused) = access.and_then(|access| access.change_refusal(&change)) {
            return Err(SqlFault::Driver(refused));
        }
        return match apply_metastore_change(&mut catalog.lock(), &catalog_id, &change, now) {
            Ok(()) => Ok(StatementResult::Applied),
            Err(failure) => Err(SqlFault::Driver(failure)),
        };
    }
    if !reads_only(statement) {
        return Err(SqlFault::NotPerformed);
    }
    if let Some(access) = access {
        // Every table the read resolves, in the order the client resolves them: the first one
        // the role may not read is where the client stops.
        for reference in statement_tables(statement, database) {
            if let Some(refused) = access.table_refusal(&reference) {
                return Err(SqlFault::Driver(refused));
            }
        }
    }
    answer_query(catalog, &catalog_id, objects, statement, database)
        .map(StatementResult::Answered)
}

/// One relation of a statement, as the catalog describes it: the table the engine holds it
/// in, its columns, and every location its rows live under.
#[derive(Debug, Clone)]
struct RelationPlan {
    /// The name the engine holds this relation under, which the statement is rewritten to
    /// name.
    engine_name: String,
    /// Every column the relation has, in order: the table's own columns and then its
    /// partition columns, each with the type the catalog records for it.
    columns: Vec<(String, String)>,
    /// Which fields of a record the table's own columns are, which is their position in it.
    picked: Vec<usize>,
    /// How the objects at the relation's locations decode.
    format: catalog_rows::TextFormat,
    /// One entry per location the relation's rows live under: the location, and the partition
    /// values every row read there carries.
    sources: Vec<(String, Vec<Option<String>>)>,
}

/// Compute what a read-only statement answers over the metastore's own data.
fn answer_query(
    catalog: &::engine::ServiceState<glue_state::ComAmazonawsGlueAwsglueState>,
    catalog_id: &str,
    objects: Option<&StepObjectCli>,
    statement: &Statement,
    database: &str,
) -> Result<ResultSet, SqlFault> {
    // The relations the statement names, each ONCE: the walk that collects them keeps a name
    // it has already seen out ([`statement_tables`]), so a self-join or a name used in two
    // subqueries is one relation here — read, loaded and counted against the bound once.
    let references = statement_tables(statement, database);
    if references.len() > MAX_RELATIONS_PER_STATEMENT {
        return Err(SqlFault::NotPerformed);
    }
    // The relations are described under the catalog's guard and their DATA is read with it
    // released: the read reaches the object store, which is another service of this emulator.
    let plans: Vec<(TableReference, RelationPlan)> = {
        let held = catalog.lock();
        references
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                planned_relation(&held, catalog_id, reference, index)
                    .map(|plan| (reference.clone(), plan))
            })
            .collect::<Result<Vec<(TableReference, RelationPlan)>, SqlFault>>()?
    };
    if !plans.is_empty() && objects.is_none() {
        // There is nothing to read the relations' data out of, so nothing computed this
        // statement.
        return Err(SqlFault::NotPerformed);
    }
    let engine = DatabaseEngine::open().map_err(|_| SqlFault::NotPerformed)?;
    let mut view = EngineView::default();
    for (reference, plan) in &plans {
        let rows = relation_rows(objects, plan)?;
        catalog_rows::load_relation(&engine, &mut view, &plan.engine_name, &plan.columns, &rows)
            .map_err(|error| {
                SqlFault::driver(
                    QUERY_FAILED_REASON,
                    format!(
                        "Failed to read table {}.{}: {}",
                        reference.database, reference.name, error.message
                    ),
                )
            })?;
    }
    let rewritten = rewritten_statement(statement, database, &plans);
    // WHAT THE RUN MAY COST is the engine's own, and none of it is reachable from the
    // statement: it interrupts a statement that outlives its deadline
    // (`sql_engine::STATEMENT_DEADLINE`, checked by the engine's progress handler), and refuses
    // one whose answer would hold more rows than `sql_engine::MAX_RESULT_ROWS` or more bytes
    // than `sql_engine::MAX_RESULT_BYTES`, charged as each value is read rather than after it
    // was allocated. A statement past any of them is an error here, not an answer.
    let answered = engine
        .execute_as(&mut view, &rewritten)
        .map_err(|error| SqlFault::driver(QUERY_FAILED_REASON, error.message))?;
    Ok(answered)
}

/// The rows an answer PRINTS, bounded by what one client invocation writes.
///
/// The engine already refused an answer past its own row and byte ceilings, and this is what
/// the CLIENT may print of the answer it did give: a larger one is an answer this emulator did
/// not write whole, so nothing is reported for the statement rather than part of its rows
/// ([`SqlFault::NotPerformed`]). The text is built row by row and charged as it is built, so a
/// bound is reached before the rest is rendered.
fn printed_rows(rows: &[Vec<Cell>]) -> Result<Vec<String>, SqlFault> {
    if rows.len() > MAX_ROWS_ANSWERED {
        return Err(SqlFault::NotPerformed);
    }
    let mut printed: Vec<String> = Vec::with_capacity(rows.len());
    let mut bytes = 0usize;
    for row in rows {
        let line = printed_row(row.as_slice());
        bytes = bytes.saturating_add(line.len());
        if bytes > MAX_ANSWER_BYTES {
            return Err(SqlFault::NotPerformed);
        }
        printed.push(line);
    }
    Ok(printed)
}

/// `FailureDetails$Reason` for a statement the engine that ran it refused, beside the
/// engine's own message for it.
pub const QUERY_FAILED_REASON: &str = "The statement the step submitted failed.";

/// Describe one relation off the stored table the catalog holds for it.
fn planned_relation(
    catalog: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    reference: &TableReference,
    index: usize,
) -> Result<RelationPlan, SqlFault> {
    let not_found =
        || SqlFault::driver(TABLE_NOT_FOUND_REASON, table_not_found_message(reference));
    let table = catalog
        .database(catalog_id, &reference.database)
        .ok_or_else(not_found)?
        .tables
        .get(&glue_state::fold_name(&reference.name))
        .ok_or_else(not_found)?;
    let storage = table.storage_descriptor.as_ref();
    let data_columns: Vec<(String, String)> = storage
        .and_then(|descriptor| descriptor.columns.as_ref())
        .map(|columns| {
            columns
                .iter()
                .map(|column| {
                    (
                        column.name.clone(),
                        column.r#type.clone().unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let partition_columns: Vec<(String, String)> = table
        .partition_keys
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|column| {
            (
                column.name.clone(),
                column.r#type.clone().unwrap_or_default(),
            )
        })
        .collect();
    let serde_parameters: HashMap<String, String> = storage
        .and_then(|descriptor| descriptor.serde_info.as_ref())
        .and_then(|serde| serde.parameters.clone())
        .unwrap_or_default();
    let format = catalog_rows::text_format(
        &serde_parameters,
        &table.parameters.clone().unwrap_or_default(),
    );
    let table_location = storage
        .and_then(|descriptor| descriptor.location.clone())
        .unwrap_or_default();
    // WHERE the rows live: a partitioned table keeps them under each partition's own
    // location, and every row read there carries that partition's values; an unpartitioned
    // one keeps them under the table's location.
    let sources: Vec<(String, Vec<Option<String>>)> = if partition_columns.is_empty() {
        vec![(table_location, Vec::new())]
    } else {
        // A table with more partitions than one statement reads is a table this query could
        // only read PART of, and part of a table is no answer about it.
        if table.partitions.len() > MAX_PARTITIONS_READ {
            return Err(SqlFault::NotPerformed);
        }
        table
            .partitions
            .values()
            .map(|partition| {
                let location = partition
                    .storage_descriptor
                    .as_ref()
                    .and_then(|descriptor| descriptor.location.clone())
                    .unwrap_or_default();
                (
                    location,
                    partition
                        .values
                        .iter()
                        .map(|value| Some(value.clone()))
                        .collect(),
                )
            })
            .collect()
    };
    let picked: Vec<usize> = (0..data_columns.len()).collect();
    let mut columns = data_columns;
    columns.extend(partition_columns);
    Ok(RelationPlan {
        engine_name: format!("relation_{index}"),
        columns,
        picked,
        format,
        sources,
    })
}

/// Read every row of one relation out of this emulator's own Amazon S3.
fn relation_rows(
    objects: Option<&StepObjectCli>,
    plan: &RelationPlan,
) -> Result<Vec<Vec<Option<String>>>, SqlFault> {
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for (location, partition_values) in &plan.sources {
        let Some((bucket, prefix)) = glue_state::split_s3_location(location) else {
            // A location this emulator cannot read data out of — one naming no bucket of its
            // own object store (another filesystem's, say), and one the catalog holds none at
            // all — is data nothing here read: the statement is answered by nothing rather
            // than over the rows it could reach, which would be part of the relation reported
            // as the whole of it.
            return Err(SqlFault::NotPerformed);
        };
        let Some(objects) = objects else {
            return Err(SqlFault::NotPerformed);
        };
        let texts = objects
            .text_objects_under(&bucket, &prefix, MAX_OBJECTS_PER_LOCATION + 1)
            .map_err(|_unread| SqlFault::NotPerformed)?;
        // A location holding more objects than one statement reads is one this query could
        // only read PART of: the bound is asked for with one object to spare, so a location
        // past it is known to be past it rather than read short.
        if texts.len() > MAX_OBJECTS_PER_LOCATION {
            return Err(SqlFault::NotPerformed);
        }
        for (key, text) in texts {
            // The markers a writer leaves beside the data are none of a table's splits.
            if catalog_rows::is_hidden_key(&key) {
                continue;
            }
            let room = MAX_ROWS_PER_RELATION.saturating_sub(rows.len());
            let read = catalog_rows::object_rows(text.as_bytes(), &plan.format, &plan.picked, room)
                .map_err(|_refused| SqlFault::NotPerformed)?;
            for mut row in read {
                row.extend(partition_values.iter().cloned());
                rows.push(row);
            }
        }
    }
    Ok(rows)
}

/// `statement` with every relation it names rewritten to the table the engine holds that
/// relation in, rendered as the SQL the engine is handed.
fn rewritten_statement(
    statement: &Statement,
    database: &str,
    plans: &[(TableReference, RelationPlan)],
) -> String {
    let mut rewritten = statement.clone();
    let defined = cte_names(statement);
    let _: ::std::ops::ControlFlow<()> = visit_relations_mut(&mut rewritten, |name| {
        if let Some(reference) = table_reference(name, database, &defined) {
            if let Some((_, plan)) = plans.iter().find(|(planned, _)| *planned == reference) {
                *name = ObjectName(vec![ObjectNamePart::Identifier(
                    sqlparser::ast::Ident::with_quote('"', plan.engine_name.clone()),
                )]);
            }
        }
        ::std::ops::ControlFlow::Continue(())
    });
    rewritten.to_string()
}

/// One row as a SQL client prints it: the cells separated by [`COLUMN_SEPARATOR`], with a
/// cell carrying no value written as [`NULL_CELL`].
fn printed_row(row: &[Cell]) -> String {
    row.iter()
        .map(printed_cell)
        .collect::<Vec<String>>()
        .join(&COLUMN_SEPARATOR.to_string())
}

/// One cell as that client prints it.
fn printed_cell(cell: &Cell) -> String {
    match cell {
        Cell::Null => NULL_CELL.to_string(),
        Cell::Integer(value) => value.to_string(),
        Cell::Real(value) => value.to_string(),
        Cell::Text(value) => value.clone(),
        // A value held as bytes is printed as the text those bytes are, which is what a
        // client reading a binary column writes; bytes that are no text print as nothing.
        Cell::Blob(value) => String::from_utf8(value.clone()).unwrap_or_default(),
    }
}
