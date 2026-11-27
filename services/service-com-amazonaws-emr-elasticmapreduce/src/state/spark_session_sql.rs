// SPDX-License-Identifier: CC0-1.0

//! The SQL a submitted Spark application's SESSION runs: the statements its `spark.sql` and
//! `spark.table` submit, planned and run against the cluster's Hive metastore.
//!
//! A Spark driver does not interpret a statement in the Python client it was submitted from: it
//! resolves the names the statement carries in the session's catalog, reads the data those
//! tables point at and runs the statement over it. On a cluster whose configuration selects the
//! AWS Glue Data Catalog as its Hive metastore ([`super::uses_glue_data_catalog`]) that catalog
//! is the metastore, so this is the engine the application's session is given: the one this
//! crate already runs a step's SQL CLIENT statements on ([`super::step_sql_engine`]), reached
//! here through the seam the Spark runtime registers for a program
//! ([`::service_common::spark_runtime::SessionSql`]).
//!
//! So one statement is answered exactly as the same statement submitted by `hive -e` is: every
//! relation it names is resolved in that catalog, the objects at the relation's location (and
//! at each of its partitions') are read out of this emulator's own Amazon S3 as the role the
//! step runs as, and the statement is run over them by the in-process SQL engine. What the
//! program reads back is what that engine answered — the columns and the values, never a value
//! read off the statement's own text — and a statement the catalog or the engine refused is
//! reported as the failure CLASS that refusal is, so the driver raises what a client branches
//! on: an analysis failure over a table the catalog holds none of apart from a failure of the
//! engine itself.
//!
//! A statement nothing here ran — one on a cluster whose metastore this emulator does not hold,
//! one whose effect is of a class this crate does not apply, one past a bound below — is
//! answered by the refusal that says so. It is never answered with an empty result, which
//! would tell the program its statement ran and found nothing.

use ::service_common::spark_runtime::{
    SessionSql, ANALYSIS_FAILURE_CLASS, ENGINE_FAILURE_CLASS,
};
use ::service_com_amazonaws_glue_awsglue::state as glue_state;

use super::step_sql_engine::{self, SqlFault, StatementResult};
use super::*;

/// How many bytes of statement text ONE submission may carry.
///
/// CHOSEN, in the units the step's own submission is bounded in: the model caps "the string
/// values passed into HadoopJarStep" at 10240 characters (`AddJobFlowSteps` doc), and a
/// statement a submitted APPLICATION builds is not bound by that, so the bound is stated here —
/// generous beside any statement a program writes out, and held where no program can reach it.
const MAX_STATEMENT_BYTES: usize = 64 * 1024;

/// How many rows of one answer are handed back to the program, beside the engine's own ceiling
/// on the rows and bytes one answer may hold at all (`sql_engine::MAX_RESULT_ROWS`,
/// `MAX_RESULT_BYTES`).
///
/// CHOSEN: the rows travel into the interpreter as one document, so what one statement hands a
/// program is bounded here as well as by the memory ceiling the run itself is given.
const MAX_ROWS_HANDED: usize = 10_000;

/// How many bytes that document may hold, for the same reason.
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;

/// The ENGINE one submitted Spark application's session runs its statements on.
///
/// Built per application run ([`super::spark_step_run`]), which is what gives each run its own
/// object-store budget and its own reading of the permissions in force on the role the step runs
/// as.
#[derive(Clone)]
pub struct SparkSessionSql {
    /// The Glue Data Catalog of this emulator, where this engine holds one at all.
    catalog: Option<::engine::ServiceState<glue_state::ComAmazonawsGlueAwsglueState>>,
    /// Whether that catalog IS the cluster's Hive metastore: a cluster whose metastore is its
    /// own master node's is one this emulator does not hold, so a statement of its session that
    /// names a relation, or that would change a metastore, is run by nothing.
    metastore_is_the_catalog: bool,
    /// What the relations' data is read out of, as the role the step runs as.
    objects: Option<StepObjectCli>,
    /// What the permissions in force on that role say about the metastore calls a statement
    /// makes ([`super::step_catalog_access`]). `None` where nothing is known about them, which
    /// withholds nothing.
    access: Option<StepCatalogAccess>,
}

/// Written by hand rather than derived: the seam requires it and neither the metastore handle
/// nor the credential the object program holds carries one. It names no field, because neither
/// belongs in a log.
impl ::std::fmt::Debug for SparkSessionSql {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.debug_struct("SparkSessionSql").finish()
    }
}

/// The engine the session of ONE submitted application gets.
///
/// `metastore_is_the_catalog` is whether the cluster's configuration selects the AWS Glue Data
/// Catalog as its Hive metastore ([`uses_glue_data_catalog`]), `objects` the program the
/// relations' data is read through and `access` what the role the step runs as may do in that
/// metastore — the same three the SQL client of a step's command line is built with
/// ([`step_sql_cli`]), because it is the same metastore and the same role.
#[must_use]
pub fn spark_session_sql<Eng: Send + Sync + 'static>(
    engine: &Eng,
    metastore_is_the_catalog: bool,
    objects: Option<StepObjectCli>,
    access: Option<StepCatalogAccess>,
) -> SparkSessionSql {
    SparkSessionSql {
        catalog: glue_state::service_state(engine),
        metastore_is_the_catalog,
        objects,
        access,
    }
}

impl SessionSql for SparkSessionSql {
    fn answered(&self, document: &str) -> String {
        let Some(asked) = Submitted::of(document) else {
            // The seam carries a document; one this engine cannot read is no statement, so
            // nothing ran.
            return refused(ENGINE_FAILURE_CLASS, &not_run_message());
        };
        match self.answer(&asked.query, &asked.database) {
            Ok(StatementResult::Applied) => applied_answer(),
            Ok(StatementResult::Answered(answered)) => {
                rendered_answer(&answered).unwrap_or_else(|| refused(
                    ENGINE_FAILURE_CLASS,
                    &not_run_message(),
                ))
            }
            Err(SqlFault::Driver(failure)) => refused(failure_class(&failure), &failure.message),
            Err(SqlFault::NotPerformed) => refused(ENGINE_FAILURE_CLASS, &not_run_message()),
        }
    }
}

impl SparkSessionSql {
    /// What the engine answered for ONE statement, with `database` the session's current one.
    ///
    /// The statement is read by the same grammar a step's SQL client reads one by
    /// ([`parse_statements`]) and run by the same engine ([`step_sql_engine::run_statement_for`]),
    /// so the one statement text has one reading and one answer however it was submitted.
    fn answer(&self, text: &str, database: &str) -> Result<StatementResult, SqlFault> {
        if text.len() > MAX_STATEMENT_BYTES {
            return Err(SqlFault::NotPerformed);
        }
        let statements = parse_statements(text).ok_or(SqlFault::NotPerformed)?;
        // `sql` submits ONE statement, which is what the engine plans and runs; a text carrying
        // more than one is not a statement this engine was asked for.
        let [statement] = statements.as_slice() else {
            return Err(SqlFault::NotPerformed);
        };
        let catalog = self.catalog.as_ref().ok_or(SqlFault::NotPerformed)?;
        // A cluster whose Hive metastore is NOT the catalog this emulator holds resolves no
        // relation and applies no change in it: such a statement is run by nothing. One that
        // names no relation and changes no metastore asks for no metastore at all, so it runs
        // on any cluster.
        if !self.metastore_is_the_catalog
            && (!statement_tables(statement, database).is_empty()
                || metastore_change(statement, database).is_some())
        {
            return Err(SqlFault::NotPerformed);
        }
        // Each statement gets the budget one piece of this step's work gets: the reads a
        // statement makes are its own, so a program that runs several is not held to the budget
        // of the first.
        step_sql_engine::run_statement_for(
            catalog,
            self.objects.as_ref().map(StepObjectCli::renewed).as_ref(),
            self.access
                .as_ref()
                .map(StepCatalogAccess::renewed)
                .as_ref(),
            statement,
            database,
            SystemTime::now(),
        )
    }
}

/// One submission of the seam: the statement, and the database an unqualified name in it
/// resolves in.
struct Submitted {
    query: String,
    database: String,
}

impl Submitted {
    /// The submission `document` carries, or `None` for a document that is no submission.
    fn of(document: &str) -> Option<Self> {
        let held: ::serde_json::Value = ::serde_json::from_str(document).ok()?;
        let query = held.get("query")?.as_str()?.to_string();
        let database = held
            .get("database")
            .and_then(::serde_json::Value::as_str)
            .filter(|database| !database.is_empty())
            .unwrap_or(DEFAULT_DATABASE)
            .to_string();
        Some(Self { query, database })
    }
}

/// The class the driver raises `failure` as: a name the metastore holds nothing under is an
/// analysis failure, which is the class a Spark driver raises an unresolved relation as, and
/// every other refusal is a failure of the engine carrying its own message.
fn failure_class(failure: &StepFailure) -> &'static str {
    if failure.reason == TABLE_NOT_FOUND_REASON {
        ANALYSIS_FAILURE_CLASS
    } else {
        ENGINE_FAILURE_CLASS
    }
}

/// What a statement NOTHING RAN is reported with: the classes of statement this emulator runs,
/// so a program is told what was not run rather than handed an empty answer.
fn not_run_message() -> String {
    format!(
        "This emulator runs {RESOLVABLE_WORK}, resolved in a metastore it holds; the statement \
         submitted is not one of those, so it was not run."
    )
}

/// The answer of a statement whose whole effect was a CHANGE the metastore applied: no columns
/// and no rows, which is the empty dataset Spark answers such a statement with.
fn applied_answer() -> String {
    ::serde_json::json!({ "columns": [], "rows": [] }).to_string()
}

/// The answer the engine gave, as the seam carries it — `None` for one past a bound of this
/// emulator, which is an answer no program is handed part of.
fn rendered_answer(answered: &::service_common::sql_wire_planes::sql_engine::ResultSet) -> Option<String> {
    if answered.rows.len() > MAX_ROWS_HANDED {
        return None;
    }
    let columns: Vec<::serde_json::Value> = answered
        .columns
        .iter()
        .map(|column| {
            ::serde_json::json!({
                "name": column.name,
                "type": spark_type(column.declared_type.as_deref()),
            })
        })
        .collect();
    let rows: Vec<::serde_json::Value> = answered
        .rows
        .iter()
        .map(|row| {
            ::serde_json::Value::Array(row.iter().map(cell_value).collect::<Vec<_>>())
        })
        .collect();
    let rendered = ::serde_json::json!({ "columns": columns, "rows": rows }).to_string();
    (rendered.len() <= MAX_ANSWER_BYTES).then_some(rendered)
}

/// The Spark type a column the engine declared a storage class for holds, and `None` for a
/// COMPUTED column, which the engine declares no class for — its type is then what the values
/// it answered are, as a dataset built from values reads it.
///
/// The classes are the ones a catalog table's columns are loaded under
/// (`::service_common::catalog_table_rows::storage_class`): a numeric column is held as a
/// number and everything else as text.
fn spark_type(declared: Option<&str>) -> Option<&'static str> {
    let declared = declared?;
    let base = declared
        .split_once('(')
        .map_or(declared, |(base, _)| base)
        .trim()
        .to_ascii_uppercase();
    match base.as_str() {
        "INTEGER" | "INT" | "BIGINT" => Some("bigint"),
        "REAL" | "DOUBLE" | "FLOAT" | "NUMERIC" | "DECIMAL" => Some("double"),
        "TEXT" | "CHAR" | "VARCHAR" | "STRING" => Some("string"),
        _ => None,
    }
}

/// One value of an answer, as the seam carries it: the value the engine holds, in the JSON type
/// that value is. Bytes that are no text carry nothing, the way a client reading them prints
/// nothing.
fn cell_value(cell: &::service_common::sql_wire_planes::sql_engine::Cell) -> ::serde_json::Value {
    use ::service_common::sql_wire_planes::sql_engine::Cell;
    match cell {
        Cell::Null => ::serde_json::Value::Null,
        Cell::Integer(value) => ::serde_json::json!(value),
        Cell::Real(value) => ::serde_json::json!(value),
        Cell::Text(value) => ::serde_json::Value::String(value.clone()),
        Cell::Blob(value) => ::serde_json::Value::String(
            String::from_utf8(value.clone()).unwrap_or_default(),
        ),
    }
}

/// The refusal the seam carries: the class the driver raises it as, and the message that
/// refusal's own.
fn refused(class: &str, message: &str) -> String {
    ::serde_json::json!({ "error": { "class": class, "message": message } }).to_string()
}
