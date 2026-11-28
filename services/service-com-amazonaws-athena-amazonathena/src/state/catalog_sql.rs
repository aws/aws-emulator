// SPDX-License-Identifier: CC0-1.0

//! Athena's STATEMENT layer: what a submitted statement means to the DATA CATALOG, and
//! how it is run.
//!
//! Athena is a query service over a data catalog, not a database of its own: a statement
//! either DEFINES or REPORTS a catalog object, or it READS data through relations the
//! catalog resolves. So a statement is first read as what it is
//! ([`statement::read`]), and then:
//!
//! * a catalog statement — `CREATE DATABASE`, `DROP DATABASE`, `SHOW DATABASES`,
//!   `CREATE EXTERNAL TABLE`, `DROP TABLE`, `CREATE VIEW`, `DROP VIEW`,
//!   `SHOW CREATE VIEW`, `SHOW TABLES`, `SHOW VIEWS` — is performed against the AWS Glue
//!   Data Catalog ([`ddl`]), which is the one catalog `AwsDataCatalog` names, so what this
//!   DDL writes is what Glue's own reads report and what Glue writes is what these
//!   statements see;
//! * a statement that reads relations has each one resolved through that same catalog and
//!   the OBJECTS at its location decoded into rows ([`data`]), which the SQL engine then
//!   computes over ([`super::query_engine`]);
//! * anything else — a projection of constants, a statement over a table an earlier
//!   statement defined in the engine itself — is the engine's own business and runs as
//!   submitted.
//!
//! A name the catalog cannot resolve is a FAILED execution carrying Athena's own error
//! classification for it ([`Failure`]): the category, the error type and the message of
//! that failure class, with the identifiers and the line and column taken from the
//! submitted statement.

/// The objects under a catalog table's location, as the rows they hold.
pub mod data;
/// The statements the Data Catalog performs.
pub mod ddl;
/// The TERMS a parenthesised declaration list of Hive's DDL holds.
mod declaration_list;
/// Why an unregistered relation fails on the relation: the database the catalog carries.
mod default_database;
/// The type one expression of a statement has, resolved against the catalog at plan time.
pub mod expression_types;
/// What Athena says about text its query grammar cannot read as a statement.
pub mod grammar;
/// The `ALTER TABLE` column changes Hive's DDL grammar takes beyond the general parser.
pub mod hive_alter_table;
/// The `CREATE DATABASE` clauses Hive's DDL grammar takes beyond the general parser.
pub mod hive_database;
/// The column types Hive's DDL grammar takes beyond the general parser.
pub mod hive_column_types;
/// Whether a catalog column's declared type names a type, and the name a query reports.
pub mod hive_types;
/// Reading INTO a column whose value is a structure rather than one scalar.
pub mod nested_values;
/// The partitions a table PROJECTS rather than registers, as its properties declare them.
pub mod partition_projection;
/// The objects a statement WRITES, and the format and layout they are written in.
pub mod query_output;
/// What one submitted statement is, and the plan for running it.
pub mod statement;

use ::std::collections::{HashMap, HashSet};
use ::std::time::{Duration, Instant, SystemTime};

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use ::service_com_amazonaws_s3_amazons3::state as s3_state;

use super::lake_formation::{DataLakeAccess, TableAccess};
use super::query_engine::{
    self, Completion, DatabaseHandle, FailureCause, LocalColumn, LocalObject, LocalTable, Outcome,
    StoredResultSet,
};
use super::query_results::ResultEncryption;
use data::{Budget, DataSource};
use query_output::{Destination, OutputFormat, Sheet, SheetColumn};
use statement::{
    CatalogStatement, DeclaredColumn, ExternalTable, Plan, Position, QueryContext, Reading,
    Relation, RelationalStatement, TableAsSelect, WriteDestination, WritingStatement,
};

/// The `AthenaError$ErrorCategory` of a failure the SERVICE is the cause of: the
/// documented category list is "1 - System, 2 - User, 3 - Other".
pub const ERROR_CATEGORY_SYSTEM: i32 = 1;

/// The `AthenaError$ErrorCategory` of a failure the CALLER's own input is the cause of;
/// see [`ERROR_CATEGORY_SYSTEM`].
pub const ERROR_CATEGORY_USER: i32 = 2;

/// The `AthenaError$ErrorType` Athena publishes for a failure the data catalog LAYER
/// raises rather than the query engine: a task the catalog's own metastore refused, and a
/// catalog name the statement's context addresses that the account has not registered.
///
/// RECORDED behaviour: a wire fixture submits a statement whose `QueryExecutionContext`
/// names a catalog nothing registered, and the execution settles FAILED with this type
/// beside [`ERROR_CATEGORY_USER`].
pub const ERROR_TYPE_CATALOG_LAYER: i32 = 1006;

/// The `AthenaError$ErrorType` Athena publishes for a DDL TASK the catalog's own
/// metastore refused: a statement that would define an object the catalog already holds,
/// and one that would drop a database that still holds tables.
pub const ERROR_TYPE_DDL_TASK: i32 = ERROR_TYPE_CATALOG_LAYER;

/// The `AthenaError$ErrorType` Athena publishes for a failure the query's EVALUATION raises
/// over the values it was given rather than one its analysis or the catalog raises: a cast
/// of a value that is not a value of the type it names.
///
/// RECORDED behaviour: such an execution settles FAILED with this type beside
/// [`ERROR_CATEGORY_USER`].
pub const ERROR_TYPE_EVALUATION: i32 = 1100;

/// The `AthenaError$ErrorType` Athena publishes for the NOT-FOUND class: a statement
/// naming a table or a database the data catalog does not hold.
pub const ERROR_TYPE_ENTITY_NOT_FOUND: i32 = 1301;

/// The `AthenaError$ErrorType` Athena publishes for a VIEW a statement addresses that is
/// not the view it needs: one that already exists where a plain `CREATE VIEW` would
/// define it, and one that is not there — or is not a view at all — where a `DROP VIEW` or
/// a `SHOW CREATE VIEW` addresses it.
pub const ERROR_TYPE_VIEW: i32 = 1502;

/// The `AthenaError$ErrorType` Athena publishes for the GENERIC user-error class: a
/// statement the caller has to change, of no narrower published class — which is the
/// class whose messages carry the `GENERIC_USER_ERROR` prefix, and the type every
/// user-caused failure this service reports no narrower class for carries
/// ([`Failure::user`]).
///
/// RECORDED behaviour: a wire fixture reads a Data Catalog entry a query cannot be planned
/// against and the execution settles FAILED with this type beside [`ERROR_CATEGORY_USER`].
pub const ERROR_TYPE_GENERIC_USER: i32 = 1000;

/// The `AthenaError$ErrorType` Athena publishes for a table DEFINITION the caller got
/// wrong: a `CREATE EXTERNAL TABLE` whose storage the catalog cannot register the table
/// on, such as a `LOCATION` that is not a folder, and a catalog entry a read cannot plan
/// against because what it declares is not a table definition Athena can read — the class
/// whose messages carry the `HIVE_INVALID_METADATA` prefix.
pub const ERROR_TYPE_TABLE_DEFINITION: i32 = 1100;

/// The `AthenaError$ErrorType` Athena publishes for a `LOCATION` the object store does not
/// hold at all: a `CREATE EXTERNAL TABLE` whose bucket does not exist, so there is nothing
/// for the table to be registered over.
///
/// RECORDED behaviour, like the category and the wording that go with it
/// ([`LOCATION_BUCKET_ABSENT`]): two wire fixtures submit such a statement — one naming the
/// table in a bucket nothing created, one under `IF NOT EXISTS` over the bucket root — and
/// each settles FAILED with this type beside [`ERROR_CATEGORY_USER`]. It is a type of its
/// own rather than the table-definition one ([`ERROR_TYPE_TABLE_DEFINITION`]) because what
/// the statement declares is a location that is NOT THERE, not a location of the wrong
/// shape.
///
/// The same class, and the same recorded type, for a READ whose table location cannot be
/// dereferenced ([`Failure::location_not_readable`]): the catalog never resolves a location
/// it records, so the location that is not there is reported when a query goes to read it.
pub const ERROR_TYPE_LOCATION_NOT_FOUND: i32 = 1306;

/// Athena's message for a `CREATE EXTERNAL TABLE` whose `LOCATION` denotes a file: fixed
/// text with no variable part — it names neither the statement's table nor its location.
const LOCATION_ON_FILE: &str = "Cannot create table on file, please specify a directory";

/// Athena's message for a `CREATE EXTERNAL TABLE` whose `LOCATION` names a bucket the
/// object store does not hold: fixed text with no variable part — it names neither the
/// bucket nor the statement's table. Recorded, with the classification beside it; see
/// [`ERROR_TYPE_LOCATION_NOT_FOUND`].
const LOCATION_BUCKET_ABSENT: &str = "The bucket specified in the ddl statement does not exist";

/// What Athena's message for a failure of its FILESYSTEM class opens with: the class a read
/// that cannot dereference a table's storage location belongs to. Fixed text; the detail
/// after it names the location that could not be read ([`Failure::location_not_readable`]).
const HIVE_FILESYSTEM_ERROR: &str = "HIVE_FILESYSTEM_ERROR";

/// What Athena's message for an unreadable table LOCATION says after that prefix: in Hive's
/// terms the data directory of a table is its partition — an unpartitioned table has exactly
/// one, its location — so a location a read cannot list is a partition location that is not
/// there. Fixed text in front of the location the catalog entry declares, which is the only
/// variable part.
const LOCATION_DOES_NOT_EXIST: &str = "Partition location does not exist";

/// What Athena's message for a failure of its INVALID-METADATA class opens with: the class a
/// catalog entry a read cannot be planned against belongs to. Fixed text; the detail after
/// it names what about the entry cannot be read ([`Failure::invalid_column_type`]).
const HIVE_INVALID_METADATA: &str = "HIVE_INVALID_METADATA";

/// Athena's message for a read of a table the catalog marks as an Iceberg table and holds
/// no metadata location for: fixed text with no variable part — it names neither the
/// database, the table nor the serialization library.
const ICEBERG_WITHOUT_METADATA_LOCATION: &str = "GENERIC_USER_ERROR: Detected Iceberg type \
     table without metadata location. Please make sure an Iceberg-enabled compute engine \
     such as Athena or EMR Spark is used to create the table, or the table is created by \
     using the Iceberg open source AWS library iceberg-aws. Setting table_type parameter \
     in Glue metastore to create an Iceberg table is not supported.";

/// How many levels of view a statement may read through: a view over a view over a table
/// is resolved, and a chain longer than this is reported rather than followed.
const MAX_VIEW_DEPTH: usize = 4;

/// What Athena's message for a write whose destination is ALREADY THERE opens with: the
/// class a statement that would write over data already at its destination belongs to.
/// Fixed text, and the error code a client reads off the message.
///
/// RECORDED behaviour: a wire fixture submits an `UNLOAD` into a prefix that already holds
/// an object and the execution settles FAILED with this code in front of
/// [`TARGET_DIRECTORY_EXISTS`] and the destination it named.
const HIVE_PATH_ALREADY_EXISTS: &str = "HIVE_PATH_ALREADY_EXISTS";

/// What that message says after the code for an `UNLOAD`: fixed text, in front of the
/// destination the statement named, which is the only variable part. Recorded beside the
/// code; see [`HIVE_PATH_ALREADY_EXISTS`].
const TARGET_DIRECTORY_EXISTS: &str = "Target directory already exists";

/// The `AthenaError$ErrorType` Athena publishes for that class: a write this statement
/// would perform over data that is already at its destination.
///
/// RECORDED behaviour, like the code and the wording in front of it: the same fixture's
/// execution reports this type beside [`ERROR_CATEGORY_USER`].
pub const ERROR_TYPE_PATH_ALREADY_EXISTS: i32 = 1109;

/// What Athena's message for a statement it will not perform at all opens with: the class a
/// form of a statement Athena does not support belongs to. Fixed text, and the error code a
/// client reads off the message.
const NOT_SUPPORTED: &str = "NOT_SUPPORTED";

/// What Athena's message for a TABLE PROPERTY a statement cannot be performed under opens
/// with: the class a `partitioned_by` that does not match the query's own columns, and a
/// required property a statement left out, belong to. Fixed text, and the error code a
/// client reads off the message.
const INVALID_TABLE_PROPERTY: &str = "INVALID_TABLE_PROPERTY";

/// Whether `database` — folded the way the Data Catalog folds a name — is a database the
/// catalog SERVES whether or not it holds a record of it.
///
/// Athena's own `default` database is one: it is the database an unqualified name resolves
/// against when a submission names none of its own
/// ([`crate::state::DEFAULT_QUERY_DATABASE`]), and every account's Data Catalog carries it,
/// so a statement addressing a relation in it addresses a database that IS there and holds
/// exactly the relations the catalog records in it — none at all, where the catalog holds no
/// record of the database. A statement naming a relation there that the catalog does not
/// hold is therefore the not-found refusal for the RELATION, never one for the database: a
/// database the catalog serves does not fail to resolve.
///
/// Only the resolution of a RELATION's database is decided here. A statement that acts on
/// the database RECORD itself — the entries `CREATE DATABASE`, `DROP DATABASE` and
/// `SHOW DATABASES` write and read, which are the same records the catalog's own API writes
/// and reads — acts on what the catalog HOLDS, and this says nothing about those.
pub fn served_without_record(database: &str) -> bool {
    database == glue_state::fold_name(crate::state::DEFAULT_QUERY_DATABASE)
}

/// Why one statement did not run: the message Athena reports as the execution's
/// `AthenaError$ErrorMessage`, the `StateChangeReason` it reports beside it where the two
/// differ, and the classification that failure belongs to.
#[derive(Debug, Clone)]
pub struct Failure {
    /// The message, as Athena words it for this failure class.
    pub message: String,
    /// The execution's `StateChangeReason`, where the engine that performed the statement
    /// reports something other than the message itself — the Hive DDL engine reports the
    /// TASK that failed and the metastore exception it wrapped. `None` where the two are
    /// the same text, which is what every other failure class reports.
    pub reason: Option<String>,
    /// Whose doing the failure was, and the error type Athena publishes for it.
    pub cause: FailureCause,
    /// The data usage a LIMIT in force on the query accounts for, where what stopped the
    /// run was such a limit rather than the statement: the run is then ABORTED rather than
    /// failed ([`query_engine::Outcome::Aborted`]), so the query is interrupted at that
    /// limit and reports it as the data it scanned. `None` for every failure.
    pub aborted_scan: Option<i64>,
}

impl Failure {
    /// A failure this EMULATOR is the cause of: a bound it holds a read to, or a
    /// capability it does not have. Neither the caller's statement being wrong nor the
    /// service failing, which is what the third category names.
    pub fn emulator(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            cause: FailureCause::Emulator,
            aborted_scan: None,
        }
    }

    /// A failure the submitted statement is the cause of, of no error class Athena
    /// publishes a NARROWER type for, so it carries the type published for the generic
    /// user-error class ([`ERROR_TYPE_GENERIC_USER`]).
    pub fn user(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            cause: FailureCause::Athena {
                category: ERROR_CATEGORY_USER,
                error_type: Some(ERROR_TYPE_GENERIC_USER),
            },
            aborted_scan: None,
        }
    }

    /// The run scanned more data than the cutoff its workgroup configured, so Athena
    /// aborted it at that cutoff.
    ///
    /// Not a failure of the statement: the query is INTERRUPTED (the cancelled state), its
    /// `StateChangeReason` is the wholly fixed sentence the limit publishes
    /// ([`BYTES_SCANNED_LIMIT_EXCEEDED`]) and the data it reports scanning is the cutoff it
    /// was stopped at.
    pub fn scan_cutoff_exceeded(cutoff: usize) -> Self {
        Self {
            message: BYTES_SCANNED_LIMIT_EXCEEDED.to_string(),
            reason: None,
            cause: FailureCause::Emulator,
            aborted_scan: Some(i64::try_from(cutoff).unwrap_or(i64::MAX)),
        }
    }

    /// A failure of one of Athena's published error classes.
    fn classified(message: String, category: i32, error_type: i32) -> Self {
        Self {
            message,
            reason: None,
            cause: FailureCause::Athena {
                category,
                error_type: Some(error_type),
            },
            aborted_scan: None,
        }
    }

    /// A failure the Hive DDL engine's own TASK reports: `message` is what
    /// `AthenaError$ErrorMessage` carries, and the task line — the task that failed,
    /// followed by what the task `wrote` about the refusal — is what the execution's
    /// `StateChangeReason` carries, because that is what the DDL engine wrote about it.
    ///
    /// The two are composed INDEPENDENTLY: the service words each field for the condition it
    /// reports, so neither is read off the other. For one condition the task wrote the
    /// metastore exception it wrapped, for another the bare sentence — so each caller states
    /// the text its own condition is reported in.
    fn ddl_task(message: String, wrote: String) -> Self {
        Self {
            message,
            reason: Some(format!("{DDL_TASK_FAILED} {wrote}")),
            cause: FailureCause::Athena {
                category: ERROR_CATEGORY_USER,
                error_type: Some(ERROR_TYPE_DDL_TASK),
            },
            aborted_scan: None,
        }
    }

    /// A `CREATE DATABASE` names a database the catalog already holds.
    ///
    /// RECORDED behaviour: the sentence `AthenaError$ErrorMessage` carries is the
    /// condition's own, closed with a full stop, and what the DDL task wrote after its task
    /// line is that same sentence WITHOUT the stop and in no exception rendering. The
    /// database is the only part either text takes from the statement.
    pub fn database_already_exists(database: &str) -> Self {
        Self::ddl_task(
            format!("{}.", database_already_exists_sentence(database)),
            database_already_exists_sentence(database),
        )
    }

    /// A `DROP DATABASE` without `CASCADE` names a database that still holds tables.
    pub fn database_not_empty(database: &str) -> Self {
        Self::ddl_task(
            format!("Cannot drop database {database} as its not empty."),
            format!(
                "MetaException(message:Unable to drop database: \
                 InvalidOperationException(message:Database {database} is not empty.))"
            ),
        )
    }

    /// The statement names a table the data catalog does not hold.
    pub fn table_not_found(relation: &Relation) -> Self {
        Self::classified(
            format!(
                "TABLE_NOT_FOUND: line {}:{}: Table '{}' does not exist",
                relation.at.line,
                relation.at.column,
                relation.qualified()
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_ENTITY_NOT_FOUND,
        )
    }

    /// The statement applies an operator to operands of types no such operator is defined
    /// between.
    ///
    /// Athena's own class for a statement its TYPE ANALYSIS refuses, worded the way its
    /// engine words an operator it has no signature for: the `TYPE_MISMATCH: ` prefix, the
    /// `line <l>:<c>: ` locator and the clause [`OPERATOR_NOT_APPLICABLE`] are fixed text,
    /// the operator is the one the statement wrote, QUOTED, and the operand types follow it
    /// in the order the operands were written, separated by a comma and a space — the two
    /// types being the ones the catalog and the statement's own literals fix, and the line
    /// and column where the statement wrote the operator.
    pub fn type_mismatch(at: Position, mismatch: &expression_types::Mismatch) -> Self {
        Self::user(format!(
            "{TYPE_MISMATCH}: line {}:{}: '{}' {OPERATOR_NOT_APPLICABLE} {}, {}",
            at.line, at.column, mismatch.operator, mismatch.left, mismatch.right
        ))
    }

    /// The statement asked for a value to be read as a type it is not a value of, which the
    /// engine raised while the statement RAN.
    ///
    /// `message` is the engine's own, in Athena's words for that class
    /// (`super::query_engine::strict_casts`), and what is added here is the classification:
    /// the caller's own statement is the cause, and the type Athena publishes for a failure
    /// its EVALUATION raises over the values the statement asked for
    /// ([`ERROR_TYPE_EVALUATION`]).
    pub fn invalid_cast_argument(message: String) -> Self {
        Self::classified(message, ERROR_CATEGORY_USER, ERROR_TYPE_EVALUATION)
    }

    /// The statement names a column no relation it reads carries.
    ///
    /// Athena's own class for an unresolvable name, worded like the not-found classes
    /// above: the `COLUMN_NOT_FOUND: ` prefix and "cannot be resolved" are fixed text, the
    /// column is the one the statement wrote, and the line and column are where it wrote
    /// it.
    pub fn column_not_found(at: Position, column: &str) -> Self {
        Self::user(format!(
            "{COLUMN_NOT_FOUND}: line {}:{}: Column '{column}' cannot be resolved",
            at.line, at.column
        ))
    }

    /// The statement names a column the relation it reads DECLARES and the caller may not
    /// see, because the Lake Formation grant it reads that relation through covers other
    /// columns.
    ///
    /// The same class as [`Self::column_not_found`] and the same prefix, with the clause
    /// Athena adds where a name is unresolvable BECAUSE of the reader's permissions — the
    /// one its not-authorized wording closes with, so the reader is not told which of the
    /// two it is. Fixed text either side of the column the statement wrote.
    pub fn column_not_authorized(column: &str) -> Self {
        Self::user(format!(
            "{COLUMN_NOT_FOUND}: Column '{column}' cannot be resolved or requester is not \
             authorized to access requested resources"
        ))
    }

    /// The statement names a database the data catalog does not hold.
    pub fn schema_not_found(relation: &Relation) -> Self {
        Self::schema_not_found_at(relation.at, &relation.database)
    }

    /// The statement names a database the data catalog does not hold, where the database
    /// is named on its own rather than as part of a relation.
    pub fn schema_not_found_at(at: Position, database: &str) -> Self {
        Self::classified(
            format!(
                "SCHEMA_NOT_FOUND: line {}:{}: Schema '{database}' does not exist",
                at.line, at.column
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_ENTITY_NOT_FOUND,
        )
    }

    /// The statement would define a table the data catalog already holds.
    ///
    /// The statement is one the HIVE DDL engine performs, so what it reports is that
    /// engine's own, in the two texts that engine writes (see [`Self::ddl_task`]): the
    /// service's sentence for the condition closed with a full stop as the error message,
    /// and the task line followed by the metastore EXCEPTION the task wrapped as the state
    /// change reason ([`table_already_exists_exception`]). RECORDED behaviour, and the half
    /// of it that differs from the already-held DATABASE: that condition's task wrote the
    /// bare sentence and this one's wrote the exception rendering
    /// ([`Self::database_already_exists`]), so each field carries what the task really wrote
    /// for its own condition.
    pub fn table_already_exists(target: &Relation) -> Self {
        Self::ddl_task(
            format!("Table {} already exists.", target.table),
            table_already_exists_exception(&target.table),
        )
    }

    /// An `ALTER TABLE … ADD COLUMNS` declares a column the table already declares, or a
    /// column list that would carry one name twice.
    ///
    /// The statement is the HIVE DDL engine's, so it reports that engine's own task failure
    /// (see [`Self::ddl_task`]): the metastore's sentence for a column list that would carry
    /// one name twice, with the column the statement declared as its only variable part. The
    /// task line carries that same sentence rather than an exception rendering, because no
    /// exception rendering is recorded for this condition and none is invented here.
    pub fn duplicate_column(column: &str) -> Self {
        Self::ddl_task(
            duplicate_column_sentence(column),
            duplicate_column_sentence(column),
        )
    }

    /// A statement the HIVE DDL engine performs names a database the catalog does not
    /// hold.
    ///
    /// That engine resolves the name while it COMPILES the statement, so what it reports is
    /// the semantic error of its own compilation rather than the query engine's planning
    /// error ([`Self::schema_not_found_at`]), which is what a statement the query engine
    /// plans reports instead — and the compiler's diagnostic is what the `StateChangeReason`
    /// carries. The ERROR MESSAGE beside it is the Data Catalog's own not-found sentence for
    /// the database, because what the compiler could not resolve the name against is that
    /// catalog, and the sentence it answered with is the one the failure reports: it is taken
    /// from the catalog's own wording ([`glue_state::messages::database_not_found`]) rather
    /// than spelled again here.
    pub fn hive_database_not_found(database: &str) -> Self {
        Self::hive_database_absent(
            glue_state::messages::database_not_found(database),
            database,
        )
    }

    /// A `DROP DATABASE` (or `DROP SCHEMA`) names a database the catalog does not hold.
    ///
    /// The compilation error is the one every statement that engine compiles reports for a
    /// database it cannot resolve, so the `StateChangeReason` is its semantic exception, the
    /// same text [`Self::hive_database_not_found`] states. What `AthenaError$ErrorMessage`
    /// carries is the service's own sentence for THIS condition — a drop of a database that
    /// is not there — because the two fields are worded independently of each other.
    /// RECORDED behaviour: fixed text either side of the database name the statement wrote,
    /// which is its only variable part.
    pub fn drop_database_not_found(database: &str) -> Self {
        Self::hive_database_absent(
            format!("Cannot drop database {database}, as it does not exist."),
            database,
        )
    }

    /// A statement the Hive DDL engine compiles names a database the catalog does not hold,
    /// reported with `message` as the error message: the `StateChangeReason` is that
    /// engine's own semantic exception for the name it could not resolve.
    fn hive_database_absent(message: String, database: &str) -> Self {
        Self {
            message,
            reason: Some(hive_database_absent_diagnostic(database)),
            cause: FailureCause::Athena {
                category: ERROR_CATEGORY_USER,
                error_type: Some(ERROR_TYPE_ENTITY_NOT_FOUND),
            },
            aborted_scan: None,
        }
    }

    /// A plain `CREATE VIEW` names a view that already exists.
    pub fn view_already_exists(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: View already exists: '{}.{}'",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// A statement addresses a view the catalog does not hold.
    pub fn view_not_found(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: View '{}.{}' does not exist",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// A statement that addresses a view names an entry that is a TABLE.
    pub fn not_a_view(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: Relation '{}.{}' is a table, not a view",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// The statement names a data catalog this account has not registered.
    ///
    /// A classified failure, not an unclassified user one: the catalog layer is what
    /// cannot resolve the name, so the execution carries the error type Athena publishes
    /// for that layer ([`ERROR_TYPE_CATALOG_LAYER`]) and a client can branch on the
    /// category and the type instead of reading the message. The `CATALOG_NOT_FOUND: `
    /// prefix and "Catalog '"/"' does not exist" are fixed text; the catalog is the one
    /// the request named, folded the way the catalog folds a name, and the line and column
    /// are where the statement addressed it.
    pub fn catalog_not_found(at: Position, catalog: &str) -> Self {
        Self::classified(
            format!(
                "CATALOG_NOT_FOUND: line {}:{}: Catalog '{}' does not exist",
                at.line,
                at.column,
                catalog.to_ascii_lowercase()
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_CATALOG_LAYER,
        )
    }

    /// A `CREATE EXTERNAL TABLE` declares a `LOCATION` that denotes a single OBJECT
    /// rather than the folder an external table's data lives under.
    pub fn location_on_file() -> Self {
        Self::classified(
            LOCATION_ON_FILE.to_string(),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_TABLE_DEFINITION,
        )
    }

    /// A `CREATE EXTERNAL TABLE` declares a `LOCATION` whose BUCKET the object store does
    /// not hold, so the statement names no storage the table could be registered over.
    pub fn location_bucket_absent() -> Self {
        Self::classified(
            LOCATION_BUCKET_ABSENT.to_string(),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_LOCATION_NOT_FOUND,
        )
    }

    /// A read cannot DEREFERENCE the location a table's catalog entry declares — the
    /// storage it names is not there — so no split of that table can be listed, let alone
    /// opened.
    ///
    /// Athena's FILESYSTEM class, which is what a location that cannot be read is: an
    /// access decision about storage the reader is not allowed to read is a different class
    /// and a different remediation. Fixed text, with `location` — the table's own, as the
    /// catalog recorded it — as the only variable part.
    pub fn location_not_readable(location: &str) -> Self {
        Self::classified(
            format!("{HIVE_FILESYSTEM_ERROR}: {LOCATION_DOES_NOT_EXIST}: {location}"),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_LOCATION_NOT_FOUND,
        )
    }

    /// A read cannot be PLANNED against a table's catalog entry because the type text one
    /// of its columns carries names no type ([`hive_types::names_a_type`]).
    ///
    /// Athena's INVALID-METADATA class: the catalog stores a column's type as text and
    /// never reads it, so an entry carrying a type nothing can read is found when a query
    /// plans a read of it. Fixed text, with the column and the type text it carries — both
    /// off the catalog entry — as the variable parts.
    pub fn invalid_column_type(column: &str, data_type: &str) -> Self {
        Self::classified(
            format!(
                "{HIVE_INVALID_METADATA}: Invalid column type for column {column}: \
                 {data_type}"
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_TABLE_DEFINITION,
        )
    }

    /// A statement reads a table the catalog marks as an Iceberg table and holds no
    /// Iceberg metadata location for, so there is no table metadata to read it through.
    pub fn iceberg_without_metadata_location() -> Self {
        Self::classified(
            ICEBERG_WITHOUT_METADATA_LOCATION.to_string(),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_GENERIC_USER,
        )
    }

    /// An `UNLOAD` names a destination that already holds objects.
    ///
    /// Athena's own class for a write whose target directory is already there: the code is
    /// the prefix of the message a client reads it off, and what follows it is fixed text in
    /// front of the destination the statement named — the one variable part. The category and
    /// the type are that class's own, so a client can branch on them instead of reading the
    /// message.
    pub fn target_directory_exists(location: &str) -> Self {
        Self::classified(
            format!("{HIVE_PATH_ALREADY_EXISTS}: {TARGET_DIRECTORY_EXISTS}: {location}"),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_PATH_ALREADY_EXISTS,
        )
    }

    /// A `CREATE TABLE … AS <query>` names a location that already holds objects.
    ///
    /// The same class as [`Self::target_directory_exists`], in the words that class is
    /// reported in for a statement that defines a TABLE over the directory: the table it
    /// defines is named as well as the directory it would write.
    pub fn table_target_directory_exists(target: &Relation, location: &str) -> Self {
        Self::classified(
            format!(
                "{HIVE_PATH_ALREADY_EXISTS}: Target directory for table '{}.{}' already \
                 exists: {location}",
                target.database, target.table
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_PATH_ALREADY_EXISTS,
        )
    }

    /// The object store would not take an object a statement wrote at `location`.
    ///
    /// Athena's FILESYSTEM class, the same one a read that cannot dereference a location
    /// reports ([`Self::location_not_readable`]): a destination that is not there is the
    /// caller's own, and what cannot be done with it is write rather than read.
    pub fn destination_not_writable(location: &str) -> Self {
        Self::classified(
            format!("{HIVE_FILESYSTEM_ERROR}: Failed to write to {location}"),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_LOCATION_NOT_FOUND,
        )
    }

    /// A `CREATE TABLE … AS <query>` whose `partitioned_by` names columns that are not the
    /// LAST columns the query projects, in that order.
    ///
    /// Athena's own class for a table property that cannot be honoured, in the words that
    /// class states the layout rule in: fixed text naming the property, with the columns
    /// the statement listed after it.
    pub fn partition_keys_not_last(partitioned_by: &[String]) -> Self {
        Self::user(format!(
            "{INVALID_TABLE_PROPERTY}: Partition keys must be the last columns in the \
             table and in the same order as the table property '{}': [{}]",
            statement::PARTITIONED_BY_PROPERTY,
            partitioned_by.join(", ")
        ))
    }

    /// An `UNLOAD` that declares no `format` property, which that statement requires.
    pub fn missing_format_property(at: Position) -> Self {
        Self::user(format!(
            "{INVALID_TABLE_PROPERTY}: line {}:{}: Required property '{}' is not set",
            at.line,
            at.column,
            statement::FORMAT_PROPERTY
        ))
    }

    /// A statement names a destination that is no Amazon S3 location at all, so there is
    /// nowhere its output could be written.
    pub fn invalid_destination(at: Position, location: &str) -> Self {
        Self::user(format!(
            "{INVALID_TABLE_PROPERTY}: line {}:{}: Invalid S3 URI: {location}",
            at.line, at.column
        ))
    }

    /// An `INSERT INTO` names a VIEW, which is not a relation rows can be appended to.
    ///
    /// Athena's class for a statement it will not perform at all, worded like its other
    /// analysis failures: the `NOT_SUPPORTED: ` prefix is the code a client reads off the
    /// message, the sentence after it is fixed text stating what is not supported, and the
    /// line and column are where the statement named the view.
    pub fn insert_into_a_view(target: &Relation) -> Self {
        Self::user(format!(
            "{NOT_SUPPORTED}: line {}:{}: Inserting into views is not supported",
            target.at.line, target.at.column
        ))
    }

    /// An `INSERT INTO` whose query projects a different number of values than the statement
    /// writes columns.
    ///
    /// Athena's analysis refuses it in the class it reports for a statement whose values do
    /// not fit the columns they are written into, worded like its other analysis failures: the
    /// class prefix, the line and column the statement named the table at, and then the
    /// sentence stating both counts — the columns the statement writes and the values its
    /// query projects.
    pub fn insert_column_count(at: Position, columns: usize, projected: usize) -> Self {
        Self::user(format!(
            "{TYPE_MISMATCH}: line {}:{}: Insert query has mismatched column count: Table \
             has {columns} columns but query has {projected} columns",
            at.line, at.column
        ))
    }

    /// An `INSERT INTO` whose column list names a column its target table does not declare.
    ///
    /// The same class an unresolvable column anywhere else in a statement is reported in
    /// ([`Self::column_not_found`]), in the words Athena's analysis states it for a column
    /// written into: fixed text either side of the column the statement named, and the line
    /// and column it named the table at.
    pub fn insert_column_not_found(at: Position, column: &str) -> Self {
        Self::user(format!(
            "{COLUMN_NOT_FOUND}: line {}:{}: Insert column name does not exist in target \
             table: {column}",
            at.line, at.column
        ))
    }

    /// This failure as the outcome of a run that produced nothing.
    pub fn outcome(self) -> Outcome {
        // A LIMIT in force on the query stopped the run, which interrupts the query rather
        // than failing it (see [`Self::aborted_scan`]).
        if let Some(data_scanned_bytes) = self.aborted_scan {
            return Outcome::Aborted {
                reason: self.message,
                data_scanned_bytes,
            };
        }
        Outcome::Failed {
            message: self.message,
            reason: self.reason,
            cause: self.cause,
        }
    }
}

/// What Athena publishes as the `StateChangeReason` of a query it aborted because the query
/// scanned more data than its workgroup's cutoff allows: wholly fixed text, naming neither
/// the cutoff, the workgroup nor the data the query read.
pub const BYTES_SCANNED_LIMIT_EXCEEDED: &str = "Bytes scanned limit was exceeded";

/// What the Hive DDL engine writes in front of the metastore exception a failed task
/// wrapped: the task that failed and the return code it failed with. The text is the
/// engine's own and carries nothing from the request; the exception after it is the one
/// the metastore raised.
const DDL_TASK_FAILED: &str =
    "FAILED: Execution Error, return code 1 from org.apache.hadoop.hive.ql.exec.DDLTask.";

/// What the Hive DDL engine says about a database it cannot resolve while compiling a
/// statement, and the error number its own catalogue gives that semantic error.
const DATABASE_DOES_NOT_EXIST: &str = "Database does not exist";

/// The error number of [`DATABASE_DOES_NOT_EXIST`] in the Hive DDL engine's catalogue.
const HIVE_DATABASE_NOT_EXISTS: i32 = 10_072;

/// What the Hive DDL engine WROTE about a statement whose database it could not resolve
/// while it compiled it: its own driver line for a failed compilation, with the database the
/// statement named as its only variable part.
fn hive_database_absent_diagnostic(database: &str) -> String {
    format!(
        "FAILED: SemanticException [Error {HIVE_DATABASE_NOT_EXISTS}]: \
         {DATABASE_DOES_NOT_EXIST}: {database}"
    )
}

/// The sentence the metastore states a database that is ALREADY THERE in, with the database
/// the statement named as its only variable part.
fn database_already_exists_sentence(database: &str) -> String {
    format!("Database {database} already exists")
}

/// The sentence the metastore states a column list that would carry ONE NAME TWICE in, with
/// the column the statement declared as its only variable part.
fn duplicate_column_sentence(column: &str) -> String {
    format!("Duplicate column name: {column}")
}

/// What a failed DDL task WROTE about a table that is ALREADY THERE: the metastore raises
/// its already-exists exception for a table, and what the task reports is that exception's
/// own rendering — the exception class, the `message:` label and the metastore's sentence
/// for the condition, worded as that exception words it rather than as the service's own
/// sentence for the same condition. The table the statement named is its only variable part.
fn table_already_exists_exception(table: &str) -> String {
    format!("AlreadyExistsException(message:Table {table} already exist)")
}

/// Everything one run of a statement needs beside the statement itself.
#[derive(Debug, Clone, Copy)]
pub struct Submission<'r> {
    /// The `QueryExecutionId` this statement runs under, which is what the objects a
    /// statement WRITES are named after — so an object says which query produced it.
    pub id: &'r str,
    /// The catalog and database an unqualified name in the statement resolves against.
    pub context: &'r QueryContext,
    /// The values bound to the statement's placeholders, in order.
    pub parameters: &'r [String],
    /// What Lake Formation said the caller may read in the Data Catalogs this statement
    /// addresses, resolved before the statement was planned: it decides whether each
    /// relation the statement reads is there for this caller at all, and which of its
    /// columns and rows are.
    pub access: &'r DataLakeAccess,
    /// When the submission was made, which is what a catalog write it performs records.
    pub now: SystemTime,
    /// How many bytes of data this ONE query may scan, where the workgroup it runs in sets a
    /// cutoff: a query that would scan past it is aborted at the cutoff rather than run to
    /// an end (`WorkGroupConfiguration$BytesScannedCutoffPerQuery`).
    pub scan_cutoff: Option<i64>,
    /// The results location this submission resolved, which is where a statement that
    /// writes a table and names no location of its own writes it.
    pub output_location: Option<&'r str>,
    /// The server-side encryption this submission's resolved result configuration settled
    /// on, which every object the execution stores is written under — the data a statement
    /// WRITES as much as the results beside it
    /// ([`crate::state::query_results::resolved_encryption`]).
    pub encryption: &'r ResultEncryption,
}

/// Run the statement `reading` planned, and report what it produced.
///
/// The catalog and the object store are reached here rather than in the handler because
/// this is the one place that knows what the statement needs from each: a catalog
/// statement takes the catalog's own guard and writes or reads it; a statement over
/// relations resolves them in the catalog, releases that guard, reads the objects their
/// locations name, and only then hands the SQL to the engine.
///
/// BLOCKING, deliberately, exactly as [`query_engine::run`] is: the engine is
/// synchronous, so a caller runs this on a blocking pool.
pub fn execute<Eng: Send + Sync + 'static>(
    engine: &Eng,
    handle: &DatabaseHandle,
    sql: &str,
    reading: Reading,
    submission: &Submission<'_>,
) -> Completion {
    let Submission {
        context,
        parameters,
        access,
        now,
        scan_cutoff,
        ..
    } = *submission;
    match reading.plan {
        Plan::Engine => query_engine::run(handle, sql, reading.counts_rows, parameters),
        // A `DROP TABLE` naming a database the catalog does not hold addresses no catalog
        // table, so it is the ENGINE's to perform — the same engine whose own database a
        // `CREATE TABLE` that declares no location defines its table in ([`Plan::Engine`]),
        // which is the only way a relation outside the catalog comes to exist here. Decided
        // before the catalog's guard is taken for the drop itself.
        Plan::Catalog(statement) if drops_a_table_of_the_engines_own(engine, &statement) => {
            query_engine::run(handle, sql, reading.counts_rows, parameters)
        }
        // A `CREATE TABLE … AS <query>` that declares NO location, in a database the data
        // catalog does not hold, defines a table of the ENGINE's own exactly as a
        // `CREATE TABLE` with no location does: there is no catalog database for it to be
        // registered in, and nowhere of its own for its data to go. Decided before the
        // destination is resolved, like the drop above.
        Plan::Written(writing) if defines_a_table_of_the_engines_own(engine, &writing) => {
            query_engine::run(handle, sql, reading.counts_rows, parameters)
        }
        // An `INSERT INTO` naming a table the DATA CATALOG holds nothing for, which an
        // earlier statement defined in this engine database, appends its rows there \u2014 the
        // same reading [`drops_a_table_of_the_engines_own`] takes of a drop.
        Plan::Written(writing)
            if inserts_into_a_table_of_the_engines_own(engine, handle, &writing, context) =>
        {
            query_engine::run(handle, sql, reading.counts_rows, parameters)
        }
        Plan::Catalog(statement) => {
            let started = Instant::now();
            let outcome = match perform(engine, &statement, now) {
                Ok(result) => Outcome::Succeeded {
                    result,
                    update_count: None,
                    data_scanned_bytes: 0,
                    // A catalog statement names no relation and materialises no object, so
                    // it read no rows of stored data.
                    input_rows: 0,
                    // It writes NO OUTPUT of its own either: what it changed is the
                    // catalog's own records, so it has no data manifest at all.
                    written_objects: None,
                },
                Err(failure) => failure.outcome(),
            };
            Completion {
                outcome,
                engine_time: started.elapsed(),
                // A catalog statement names no relation to resolve and reads no data: the
                // whole of its run is the task the catalog performs, so the planning this
                // run did is the READING of the statement, which the caller measured.
                planning_time: Duration::ZERO,
            }
        }
        Plan::Relations(relational) => {
            let started = Instant::now();
            match resolved_sql(engine, handle, *relational, context, access, scan_cutoff, sql) {
                Ok(prepared) => {
                    // Resolving the statement's relations against the catalog and reading
                    // the objects at their locations is what Athena measures as the query
                    // PLANNING phase — "the time spent retrieving table partitions from
                    // the data source" — and it is over before the engine is handed the
                    // SQL, so the two spans are measured separately rather than reported
                    // as one.
                    let planning_time = started.elapsed();
                    let mut completion = query_engine::run_over(
                        handle,
                        &prepared.objects,
                        &prepared.sql,
                        reading.counts_rows,
                        parameters,
                        prepared.scanned_bytes,
                    );
                    completion.planning_time = planning_time;
                    // A name the relations it read do not carry is a failure of one of
                    // Athena's own classes, reported in Athena's words about the SUBMITTED
                    // statement rather than in the engine's about the one it ran.
                    classify_unresolvable_name(&mut completion, sql, &prepared.hidden_columns);
                    completion
                }
                Err(failure) => Completion {
                    outcome: failure.outcome(),
                    engine_time: Duration::ZERO,
                    planning_time: started.elapsed(),
                },
            }
        }
        Plan::Written(writing) => {
            let started = Instant::now();
            let WritingStatement { query, destination } = *writing;
            // WHERE the output goes is resolved BEFORE the query runs for a statement that
            // DEFINES what it writes over: a destination that already holds objects is
            // refused rather than written over, and nothing about that refusal depends on
            // the rows the query would produce. A statement that APPENDS to a table that is
            // already there refuses no destination of its own, so its target is resolved
            // after the query, where the names the query reads are resolved first
            // ([`WriteDestination::is_settled_before_the_query`]).
            let settled = destination.is_settled_before_the_query();
            let mut output = None;
            if settled {
                output = match resolved_destination(engine, &destination, submission) {
                    Ok(output) => Some(output),
                    Err(failure) => return refused(failure, started),
                };
            }
            match resolved_sql(engine, handle, query, context, access, scan_cutoff, sql) {
                Ok(prepared) => {
                    // The query is run exactly as any other read is, and measured the same
                    // way: its relations are resolved first and the engine is handed SQL
                    // that names the rows they were materialised as.
                    let planning_time = started.elapsed();
                    let mut completion = query_engine::run_over(
                        handle,
                        &prepared.objects,
                        &prepared.sql,
                        reading.counts_rows,
                        parameters,
                        prepared.scanned_bytes,
                    );
                    completion.planning_time = planning_time;
                    classify_unresolvable_name(&mut completion, sql, &prepared.hidden_columns);
                    // A run that produced no rows to write is left exactly as the engine
                    // reported it, so the destination of such a statement is never resolved.
                    if !matches!(completion.outcome, Outcome::Succeeded { .. }) {
                        return completion;
                    }
                    let output = match output {
                        Some(output) => output,
                        None => match resolved_destination(engine, &destination, submission) {
                            Ok(output) => output,
                            Err(failure) => {
                                return Completion {
                                    outcome: failure.outcome(),
                                    ..completion
                                }
                            }
                        },
                    };
                    // What the query produced is now in hand: it is WRITTEN rather than
                    // answered with, and what the statement defines over it is registered.
                    written(engine, &destination, &output, completion, submission)
                }
                Err(failure) => refused(failure, started),
            }
        }
    }
}

/// One run that produced nothing because the statement was refused before the engine ran
/// it: the refusal is what the execution reports, and the span it took is the resolution
/// that refused it.
fn refused(failure: Failure, started: Instant) -> Completion {
    Completion {
        outcome: failure.outcome(),
        engine_time: Duration::ZERO,
        planning_time: started.elapsed(),
    }
}

/// Where a statement that writes its output sends it, or why it cannot be written there.
///
/// Resolved from what the statement DECLARES: the format its `format` property names, the
/// location it names (and, for a table that names none, the one Athena writes such a table
/// under), and whether that location is free — a destination already holding objects is
/// refused, which is the one rule both statements are held to.
fn resolved_destination<Eng: Send + Sync + 'static>(
    engine: &Eng,
    destination: &WriteDestination,
    submission: &Submission<'_>,
) -> Result<Destination, Failure> {
    match destination {
        WriteDestination::Table(table) => {
            let format = match table.format.as_deref() {
                Some(named) => OutputFormat::named(named)?,
                None => query_output::DEFAULT_TABLE_FORMAT,
            };
            let output = Destination::of(
                &table_location(table, submission)?,
                format,
                submission.id,
                table.target.at,
            )?;
            if output.bucket_is_absent(engine) {
                return Err(Failure::destination_not_writable(&output.written));
            }
            if output.is_occupied(engine) {
                return Err(Failure::table_target_directory_exists(
                    &table.target,
                    &output.written,
                ));
            }
            Ok(output)
        }
        WriteDestination::Unload(unload) => {
            // The format an `UNLOAD` writes is REQUIRED of it: the statement says what the
            // objects it writes are, and one that says nothing writes none.
            let Some(named) = unload.format.as_deref() else {
                return Err(Failure::missing_format_property(unload.at));
            };
            let output = Destination::of(
                &unload.location,
                OutputFormat::named(named)?,
                submission.id,
                unload.at,
            )?;
            if output.bucket_is_absent(engine) {
                return Err(Failure::destination_not_writable(&output.written));
            }
            if output.is_occupied(engine) {
                return Err(Failure::target_directory_exists(&output.written));
            }
            Ok(output)
        }
        WriteDestination::Insert(insert) => {
            // The target is the table the catalog ALREADY holds: where its data lives and
            // the format its objects are stored in are its own, not the statement's.
            let target = insert_target(engine, insert)?;
            let output = Destination::of(
                &target.location,
                OutputFormat::of_stored(target.format),
                submission.id,
                insert.target.at,
            )?;
            if output.bucket_is_absent(engine) {
                return Err(Failure::destination_not_writable(&output.written));
            }
            // A location already holding objects is NOT refused here: this statement
            // APPENDS one object to the table's data, which is what the objects already
            // there are. The object is named after the execution, so it replaces none of
            // them.
            Ok(output)
        }
    }
}

/// What the Data Catalog holds for the table an `INSERT INTO` appends to.
#[derive(Debug, Clone)]
struct InsertTarget {
    /// Where the table's data lives, as its storage descriptor records it.
    location: String,
    /// The format its objects hold their records in, which is the format the appended
    /// object is written in so the table's own reader reads it back.
    format: data::StoredFormat,
    /// The table's columns, in order: the storage descriptor's, and then the partition keys
    /// ([`ddl::relation_columns`]).
    columns: Vec<DeclaredColumn>,
    /// How many of the LAST [`Self::columns`] are partition keys, whose values decide which
    /// directory under the location a row's object goes in.
    partition_keys: usize,
    /// Whether the table's partitions are PROJECTED from its own properties rather than
    /// recorded in the catalog ([`partition_projection`]): a partition of such a table is
    /// not registered when its data is written, because the properties are what says which
    /// partitions the table has.
    projects_partitions: bool,
}

/// What the catalog holds for `insert`'s target, or why nothing can be appended to it.
///
/// A name the catalog resolves nothing for is the not-found refusal for the relation, exactly
/// as a READ of that name reports it; a VIEW is not a relation rows can be appended to; and a
/// table whose data this emulator could not write back in the format the table's own reader
/// reads is reported rather than written as something else.
fn insert_target<Eng: Send + Sync + 'static>(
    engine: &Eng,
    insert: &statement::InsertStatement,
) -> Result<InsertTarget, Failure> {
    let catalog_id = catalog_id_of(engine, &insert.target.catalog, insert.target.at)?;
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let store = state.lock();
    let database = match store.database(&catalog_id, &insert.target.database) {
        Some(database) => database,
        // A database the catalog SERVES without holding a record of it holds no relation at
        // all, so what the statement named is the relation that is not there.
        None if served_without_record(&insert.target.database) => {
            return Err(Failure::table_not_found(&insert.target))
        }
        None => return Err(Failure::schema_not_found(&insert.target)),
    };
    let Some(table) = database.tables.get(&insert.target.table) else {
        return Err(Failure::table_not_found(&insert.target));
    };
    if ddl::is_view(table) {
        return Err(Failure::insert_into_a_view(&insert.target));
    }
    let location = table
        .storage_descriptor
        .as_ref()
        .and_then(|storage| storage.location.clone())
        .map(|location| location.trim().to_string())
        .filter(|location| !location.is_empty())
        .ok_or_else(|| Failure::user(statement::NO_TABLE_LOCATION.to_string()))?;
    let format = data::stored_format(table).ok_or_else(|| {
        Failure::emulator(format!(
            "the data of {}.{} is stored in a format this emulator's query engine does not \
             write, so no statement can append rows to it",
            insert.target.database, insert.target.table
        ))
    })?;
    let keys: Vec<String> = ddl::partition_key_columns(table)
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let projects_partitions = partition_projection::declared(
        &table.parameters.clone().unwrap_or_default(),
        &keys,
    )
    .is_some();
    Ok(InsertTarget {
        location,
        format,
        columns: ddl::relation_columns(table),
        partition_keys: keys.len(),
        projects_partitions,
    })
}

/// Where the data of the table a `CREATE TABLE … AS <query>` defines is written: the
/// `external_location` it declared, and otherwise the folder Athena writes a table that
/// declares none into — "a subfolder of the query result location", named after the
/// execution, which is what makes the data of two such statements two different folders.
fn table_location(
    table: &TableAsSelect,
    submission: &Submission<'_>,
) -> Result<String, Failure> {
    if let Some(declared) = table
        .external_location
        .as_deref()
        .map(str::trim)
        .filter(|location| !location.is_empty())
    {
        return Ok(declared.to_string());
    }
    let results = submission
        .output_location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or_else(|| {
            Failure::emulator(
                "this submission resolved no results location, so a table that declares \
                 no external location of its own has nowhere for its data to go",
            )
        })?;
    Ok(format!(
        "{}/{TABLES_FOLDER}/{}",
        results.trim_end_matches('/'),
        submission.id
    ))
}

/// The folder of the query result location Athena writes a table that declared no
/// `external_location` under.
const TABLES_FOLDER: &str = "tables";

/// Write what the query produced, and perform what the statement DEFINES over it.
///
/// A run that produced no rows to write — one that failed, and one a limit interrupted — is
/// left exactly as the engine reported it. A run that produced rows reports them as WRITTEN:
/// the objects it stored, with no result set of its own, because neither statement answers
/// with rows.
fn written<Eng: Send + Sync + 'static>(
    engine: &Eng,
    destination: &WriteDestination,
    output: &Destination,
    completion: Completion,
    submission: &Submission<'_>,
) -> Completion {
    let Completion {
        outcome,
        engine_time,
        planning_time,
    } = completion;
    match outcome {
        Outcome::Succeeded {
            result,
            data_scanned_bytes,
            input_rows,
            ..
        } => {
            let outcome = match write_output(engine, destination, output, &result, submission) {
                Ok(written_objects) => Outcome::Succeeded {
                    // "The number of rows inserted with a CREATE TABLE AS SELECT, INSERT
                    // INTO, or UPDATE statement": a statement that APPENDS rows to a table
                    // counts the rows it appended, which are the rows its query produced.
                    update_count: match destination {
                        WriteDestination::Insert(..) => {
                            Some(i64::try_from(result.rows.len()).unwrap_or(i64::MAX))
                        }
                        WriteDestination::Table(..) | WriteDestination::Unload(..) => None,
                    },
                    // The rows were WRITTEN, not returned: a read of this execution's
                    // results reports the empty result set rather than the rows of the
                    // query that produced the objects.
                    result: StoredResultSet::default(),
                    data_scanned_bytes,
                    input_rows,
                    written_objects,
                },
                Err(failure) => failure.outcome(),
            };
            Completion {
                outcome,
                engine_time,
                planning_time,
            }
        }
        outcome => Completion {
            outcome,
            engine_time,
            planning_time,
        },
    }
}

/// Write the rows `result` holds where `destination` says, and answer the objects written.
///
/// The answer is `Some`, even where it is EMPTY: this statement wrote its output, and a
/// query that produced no rows wrote that output as no objects at all — which is what its
/// data manifest then lists.
fn write_output<Eng: Send + Sync + 'static>(
    engine: &Eng,
    destination: &WriteDestination,
    output: &Destination,
    result: &StoredResultSet,
    submission: &Submission<'_>,
) -> Result<Option<Vec<String>>, Failure> {
    match destination {
        // An `UNLOAD` writes the rows as they were projected, under the destination
        // itself: it defines no table, so it partitions its output by nothing.
        WriteDestination::Unload(_) => {
            let columns = sheet_columns(result);
            let values = sheet_values(result);
            if values.is_empty() {
                return Ok(Some(Vec::new()));
            }
            let sheet = Sheet {
                columns,
                rows: values,
            };
            let directory = query_output::partition_directory(output, &[], &[]);
            query_output::store(
                engine,
                vec![query_output::object_of(output, &directory, &sheet)?],
                submission.encryption,
            )
            .map(Some)
        }
        WriteDestination::Table(table) => {
            written_table(engine, table, output, result, submission).map(Some)
        }
        WriteDestination::Insert(insert) => {
            written_insert(engine, insert, output, result, submission).map(Some)
        }
    }
}

/// APPEND the rows `result` holds to the table an `INSERT INTO` names: one new object per
/// partition they fall into, under that table's own location.
///
/// The object carries the table's OWN columns, in the table's own order: a value the query
/// produced goes into the column the statement's column list puts it in, and a column the
/// statement writes nothing for carries no value — because that is how the table's reader
/// reads the fields back, positionally against the columns the catalog declares. The rows are
/// grouped by the values of the partition keys, which are the LAST columns of the table, so
/// each object lands in the Hive directory its partition's values name, exactly as a read of
/// that table resolves one.
///
/// The object is named after the EXECUTION, so it replaces no object already under the
/// location: a second `INSERT` adds a second object and a later read answers with the union
/// of the rows.
fn written_insert<Eng: Send + Sync + 'static>(
    engine: &Eng,
    insert: &statement::InsertStatement,
    output: &Destination,
    result: &StoredResultSet,
    submission: &Submission<'_>,
) -> Result<Vec<String>, Failure> {
    // The target is read again rather than carried from the destination resolution: what the
    // write needs of it is the COLUMNS the rows are laid out against, and the catalog's guard
    // is taken once for each read and released before the objects are written.
    let target = insert_target(engine, insert)?;
    let data_columns = target.columns.len().saturating_sub(target.partition_keys);
    let keys: Vec<String> = target
        .columns
        .iter()
        .skip(data_columns)
        .map(|column| column.name.clone())
        .collect();
    let writes = insert_columns(&target, insert, result.column_info.len())?;
    let columns: Vec<SheetColumn<'_>> = target
        .columns
        .iter()
        .map(|column| SheetColumn {
            name: column.name.as_str(),
            data_type: data::engine_type(&column.data_type),
        })
        .collect();
    let produced = sheet_values(result);
    let mut values: Vec<Vec<Option<&str>>> = Vec::with_capacity(produced.len());
    for row in &produced {
        let mut laid_out: Vec<Option<&str>> = vec![None; target.columns.len()];
        for (at, value) in row.iter().enumerate() {
            let Some(column) = writes.get(at).copied() else {
                continue;
            };
            if let Some(cell) = laid_out.get_mut(column) {
                *cell = *value;
            }
        }
        values.push(laid_out);
    }
    let partitions = query_output::partitioned(&values, keys.len())?;
    let mut objects = Vec::new();
    let mut directories: Vec<(Vec<String>, String)> = Vec::new();
    for partition in &partitions {
        // A partition with no row of its own is no partition this statement wrote: nothing
        // was written there, so there is no object and no record for it.
        if partition.rows.is_empty() {
            continue;
        }
        let sheet = Sheet {
            columns: columns[..data_columns].to_vec(),
            rows: partition
                .rows
                .iter()
                .filter_map(|at| values.get(*at))
                .map(|row| row.iter().take(data_columns).copied().collect())
                .collect(),
        };
        let directory = query_output::partition_directory(output, &keys, &partition.values);
        objects.push(query_output::object_of(output, &directory, &sheet)?);
        if !partition.values.is_empty() {
            directories.push((partition.values.clone(), directory));
        }
    }
    let written = query_output::store(engine, objects, submission.encryption)?;

    // The partitions the appended rows belong to are registered once their objects are
    // really there, exactly as a `CREATE TABLE … AS <query>` registers the ones it wrote —
    // unless the table's partitions are PROJECTED, in which case its own properties say
    // which partitions it has and the catalog holds none of them.
    if !directories.is_empty() && !target.projects_partitions {
        let catalog_id = catalog_id_of(engine, &insert.target.catalog, insert.target.at)?;
        let Some(state) = glue_state::service_state(engine) else {
            return Err(Failure::emulator(CATALOG_UNREACHABLE));
        };
        let mut store = state.lock();
        ddl::register_partitions(
            &mut store,
            &catalog_id,
            &insert.target,
            &directories,
            submission.now,
        )?;
    }
    Ok(written)
}

/// Which of the target table's columns each value the query produced is written into, in
/// projection order.
///
/// The statement's own column list says it where it has one, and otherwise the values go into
/// the table's columns in the table's own order — which is what an `INSERT` with no column
/// list writes. A list naming a column the table does not declare, and a projection of a
/// different number of values than the statement writes columns, are refused: either would
/// write a row into columns the statement did not name.
fn insert_columns(
    target: &InsertTarget,
    insert: &statement::InsertStatement,
    projected: usize,
) -> Result<Vec<usize>, Failure> {
    if insert.columns.is_empty() {
        if projected != target.columns.len() {
            return Err(Failure::insert_column_count(
                insert.target.at,
                target.columns.len(),
                projected,
            ));
        }
        return Ok((0..target.columns.len()).collect());
    }
    if projected != insert.columns.len() {
        return Err(Failure::insert_column_count(
            insert.target.at,
            insert.columns.len(),
            projected,
        ));
    }
    insert
        .columns
        .iter()
        .map(|named| {
            target
                .columns
                .iter()
                .position(|column| glue_state::fold_name(&column.name) == *named)
                .ok_or_else(|| {
                    Failure::insert_column_not_found(insert.target.at, named)
                })
        })
        .collect()
}

/// Write the data of the table a `CREATE TABLE … AS <query>` defines, and register the
/// table and the partitions that data belongs to.
///
/// The PARTITION columns are the last columns the query projects — Athena's own layout rule
/// for the property, which is what makes a partition's values readable off the directory its
/// objects live in — so a statement whose `partitioned_by` names anything else is refused
/// rather than written as if it had named the last ones.
///
/// The order is: everything that REFUSES the statement first (the layout rule, and an entry
/// the catalog already holds), then the objects, then the catalog entry and the partitions of
/// the data — so a refused statement writes nothing, and the catalog never names a location
/// the objects were not written to.
fn written_table<Eng: Send + Sync + 'static>(
    engine: &Eng,
    table: &TableAsSelect,
    output: &Destination,
    result: &StoredResultSet,
    submission: &Submission<'_>,
) -> Result<Vec<String>, Failure> {
    let now = submission.now;
    let columns = sheet_columns(result);
    let keys = table.partitioned_by.clone();
    if !partitions_are_last(&columns, &keys) {
        return Err(Failure::partition_keys_not_last(&keys));
    }
    let data_columns = columns.len().saturating_sub(keys.len());
    let values = sheet_values(result);
    let partitions = query_output::partitioned(&values, keys.len())?;
    let catalog_id = catalog_id_of(engine, &table.target.catalog, table.target.at)?;

    // An entry the catalog ALREADY holds is what decides whether this statement writes at
    // all: one that tolerates it writes nothing over the table that is there, and one that
    // does not is refused before a single object is written.
    match held_table(engine, &catalog_id, &table.target)? {
        true if table.if_not_exists => return Ok(Vec::new()),
        true => return Err(Failure::table_already_exists(&table.target)),
        false => {}
    }

    let mut objects = Vec::new();
    let mut directories: Vec<(Vec<String>, String)> = Vec::new();
    for partition in &partitions {
        // A partition with no row of its own is no partition of this table: nothing was
        // written there, so there is no object and no record for it.
        if partition.rows.is_empty() {
            continue;
        }
        let sheet = Sheet {
            columns: columns[..data_columns].to_vec(),
            rows: partition
                .rows
                .iter()
                .filter_map(|at| values.get(*at))
                .map(|row| row.iter().take(data_columns).copied().collect())
                .collect(),
        };
        let directory = query_output::partition_directory(output, &keys, &partition.values);
        objects.push(query_output::object_of(output, &directory, &sheet)?);
        if !partition.values.is_empty() {
            directories.push((partition.values.clone(), directory));
        }
    }
    let written = query_output::store(engine, objects, submission.encryption)?;

    // The table and the partitions its data belongs to are registered once the objects they
    // address are really there, under ONE guard of the catalog's store.
    let definition = table_definition(table, output, &columns[..data_columns], &keys);
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let mut store = state.lock();
    ddl::create_external_table(&mut store, &catalog_id, &definition, now)?;
    ddl::register_partitions(&mut store, &catalog_id, &table.target, &directories, now)?;
    Ok(written)
}

/// Whether the Data Catalog already holds an entry of the name `target` addresses.
fn held_table<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    target: &Relation,
) -> Result<bool, Failure> {
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let store = state.lock();
    Ok(store
        .database(catalog_id, &target.database)
        .is_some_and(|database| database.tables.contains_key(&target.table)))
}

/// The catalog entry a `CREATE TABLE … AS <query>` defines: the table declared over the rows
/// its query produced, at the location they were written to and in the storage format they
/// were written in.
fn table_definition(
    table: &TableAsSelect,
    output: &Destination,
    columns: &[SheetColumn<'_>],
    keys: &[String],
) -> ExternalTable {
    ExternalTable {
        target: table.target.clone(),
        if_not_exists: table.if_not_exists,
        // The columns the OBJECTS carry, and then the partition keys: each column with the
        // Hive type its own Athena type is recorded as.
        columns: columns.iter().map(declared_column).collect(),
        partition_columns: keys
            .iter()
            .map(|key| DeclaredColumn {
                name: key.clone(),
                data_type: HIVE_PARTITION_KEY_TYPE.to_string(),
                comment: None,
            })
            .collect(),
        location: Some(output.location.clone()),
        serde_library: None,
        serde_parameters: Vec::new(),
        table_properties: Vec::new(),
        storage: Some(output.format.storage()),
        comment: None,
    }
}

/// The Hive type a PARTITION KEY of a table written by one of these statements is recorded
/// with.
///
/// A partition key's values are the ones the directory its objects live under names, which
/// is text — the key is read back off that path, so what the catalog records for it is the
/// unbounded character type whatever the query's own column was.
const HIVE_PARTITION_KEY_TYPE: &str = "string";

/// One column of the rows a statement writes as the catalog declares it: the name it is
/// written under, and the Hive DDL name of the type the result reports for it.
fn declared_column(column: &SheetColumn<'_>) -> DeclaredColumn {
    DeclaredColumn {
        name: glue_state::fold_name(column.name),
        data_type: hive_types::declared_type(column.data_type),
        comment: None,
    }
}

/// Whether `keys` are the LAST columns of `columns`, in that order — the layout a
/// partitioned table's columns have to be in.
///
/// A statement that names no partition key satisfies it trivially. The comparison is the one
/// the catalog makes between two column names, which is case-insensitive.
fn partitions_are_last(columns: &[SheetColumn<'_>], keys: &[String]) -> bool {
    if keys.len() > columns.len() {
        return false;
    }
    columns
        .iter()
        .skip(columns.len() - keys.len())
        .zip(keys)
        .all(|(column, key)| glue_state::fold_name(column.name) == *key)
}

/// The columns of the rows a statement writes, in projection order.
fn sheet_columns(result: &StoredResultSet) -> Vec<SheetColumn<'_>> {
    result
        .column_info
        .iter()
        .map(|column| SheetColumn {
            name: column.name.as_str(),
            data_type: column.r#type.as_str(),
        })
        .collect()
}

/// The rows a statement writes: one value per projected column, `None` for a NULL.
fn sheet_values(result: &StoredResultSet) -> Vec<Vec<Option<&str>>> {
    result
        .rows
        .iter()
        .map(|row| {
            row.data
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|datum| datum.var_char_value.as_deref())
                .collect()
        })
        .collect()
}

/// The prefix Athena's message for an unresolvable name opens with, which is also the error
/// code a client reads off it.
const COLUMN_NOT_FOUND: &str = "COLUMN_NOT_FOUND";

/// What Athena's message for a statement its TYPE ANALYSIS refuses opens with: the class of
/// an operator applied to operands no such operator is defined between.
const TYPE_MISMATCH: &str = "TYPE_MISMATCH";

/// What Athena's message for an operator it has no signature for says between the operator
/// and the operand types: the clause its engine words that condition in. Fixed text, with
/// the quoted operator in front of it and the operand types after it
/// ([`Failure::type_mismatch`]).
const OPERATOR_NOT_APPLICABLE: &str = "cannot be applied to";

/// What Athena's message for a CAST it could not perform opens with: the class of a value
/// that is not a value of the type the cast names. The engine itself raises the message, in
/// those words (`super::query_engine::strict_casts`); this is where the CLASS it belongs to
/// is put back on it.
const INVALID_CAST_ARGUMENT: &str = "INVALID_CAST_ARGUMENT";

/// Report a run the SQL engine stopped over a NAME the SUBMITTED statement wrote and it
/// could not resolve as the failure of Athena's own class for it.
///
/// The engine resolves the names of the statement it was handed, which is the submitted one
/// with each relation rewritten to the rows it was materialised as — so what it says about
/// an unresolvable column names that column but neither the class Athena publishes for it
/// nor where the submitted statement wrote it. Both are added here: the class from the
/// engine's own diagnostic ([`ENGINE_UNRESOLVABLE_COLUMN`], its fixed wording for that one
/// failure), and the position from the submitted text.
///
/// A name the submitted statement does NOT carry is not the caller's to be told about in
/// those words — the engine also resolves names this emulator wrote for it, among them the
/// columns a Lake Formation row filter's own expression refers to — so such a failure is
/// left exactly as the engine reported it, as is every other engine failure.
fn classify_unresolvable_name(
    completion: &mut Completion,
    sql: &str,
    hidden_columns: &HashSet<String>,
) {
    let Outcome::Failed {
        message,
        cause: FailureCause::Sql,
        ..
    } = &completion.outcome
    else {
        return;
    };
    // A failure the engine reports in ATHENA's own words is one of Athena's published
    // classes already — the checked reading of a cast raises exactly that
    // (`super::query_engine::strict_casts`) — so it carries that class's category and type
    // rather than being reported as a plain engine error.
    if message.starts_with(INVALID_CAST_ARGUMENT) {
        completion.outcome = Failure::invalid_cast_argument(message.clone()).outcome();
        return;
    }
    let Some(column) = unresolvable_column(message) else {
        return;
    };
    // A column the relation DECLARES and this reader may not see is unresolvable because of
    // what the reader holds, which Athena says in a sentence of its own.
    let failure = if hidden_columns.contains(&glue_state::fold_name(&column)) {
        Failure::column_not_authorized(&column)
    } else {
        match statement::identifier_position(sql, &column) {
            Some(at) => Failure::column_not_found(at, &column),
            None => return,
        }
    };
    completion.outcome = failure.outcome();
}

/// The column the SQL engine could not resolve, read off its own diagnostic for that
/// failure — `None` for every other engine failure.
///
/// The engine words it as [`ENGINE_UNRESOLVABLE_COLUMN`] followed by the name as the
/// statement wrote it, and then by what it was resolving the name in. A name the statement
/// QUOTED is quoted there too, and carries the engine's own hint about the quoting after it;
/// the name itself is what is taken, unquoted, because that is what Athena's own message
/// names.
fn unresolvable_column(message: &str) -> Option<String> {
    let named = message.strip_prefix(ENGINE_UNRESOLVABLE_COLUMN)?.trim_start();
    let name = match named.strip_prefix(ENGINE_QUOTE) {
        // A quoted name runs to its closing quote, so a name carrying a space is one name.
        Some(quoted) => quoted.split(ENGINE_QUOTE).next().unwrap_or(quoted),
        None => named.split_whitespace().next().unwrap_or(named),
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// How the SQL engine opens its diagnostic for a column it cannot resolve.
const ENGINE_UNRESOLVABLE_COLUMN: &str = "no such column: ";

/// How that diagnostic quotes a name the statement quoted.
const ENGINE_QUOTE: char = '"';

/// Perform one catalog statement against the Data Catalog.
fn perform<Eng: Send + Sync + 'static>(
    engine: &Eng,
    statement: &CatalogStatement,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let (catalog, at) = match statement {
        CatalogStatement::CreateDatabase(database) => {
            (database.target.catalog.clone(), database.target.at)
        }
        CatalogStatement::DropDatabase { target, .. } => (target.catalog.clone(), target.at),
        CatalogStatement::ShowDatabases { catalog, .. } => (catalog.clone(), Position::START),
        CatalogStatement::CreateExternalTable(table) => {
            (table.target.catalog.clone(), table.target.at)
        }
        CatalogStatement::CreateView(view) => (view.target.catalog.clone(), view.target.at),
        CatalogStatement::DropView { target, .. }
        | CatalogStatement::DropTable { target, .. }
        | CatalogStatement::ShowCreateView { target } => (target.catalog.clone(), target.at),
        CatalogStatement::ShowRelations { schema, .. } => (schema.catalog.clone(), schema.at),
        CatalogStatement::ShowPartitions { target }
        | CatalogStatement::RepairTable { target } => (target.catalog.clone(), target.at),
        CatalogStatement::ChangeTableColumns(change) => {
            (change.target.catalog.clone(), change.target.at)
        }
        CatalogStatement::ChangeTableProperties { target, .. } => {
            (target.catalog.clone(), target.at)
        }
    };
    let catalog_id = catalog_id_of(engine, &catalog, at)?;
    // The partitions a `MSCK REPAIR TABLE` registers are the ones the OBJECTS under the
    // table's location are laid out under, so this statement reads the object store between
    // two reads of the catalog rather than inside one: the guard of each store is taken and
    // released on its own, so no two are ever held at once.
    if let CatalogStatement::RepairTable { target } = statement {
        return repair_table(engine, &catalog_id, target, now);
    }
    // An external table's LOCATION has to name storage the table can be registered over:
    // a bucket the object store holds, and a FOLDER inside it. Both halves are validated
    // here, before the catalog entry the statement would define is resolved, because what
    // it declares is wrong whatever the catalog holds; the bucket first, since what sits at
    // a key of a bucket that is not there is not a question. The object store's guard is
    // taken and released inside, so no two stores are ever held at once.
    if let CatalogStatement::CreateExternalTable(table) = statement {
        if location_bucket_is_absent(engine, table.location.as_deref()) {
            return Err(Failure::location_bucket_absent());
        }
        if location_denotes_an_object(engine, table.location.as_deref()) {
            return Err(Failure::location_on_file());
        }
    }
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let mut store = state.lock();
    match statement {
        CatalogStatement::CreateDatabase(database) => {
            ddl::create_database(&mut store, &catalog_id, database, now)
        }
        CatalogStatement::DropDatabase {
            target,
            if_exists,
            cascade,
        } => {
            let dropped =
                ddl::drop_database(&mut store, &catalog_id, target, *if_exists, *cascade)?;
            forget_partition_indexes(engine, &dropped);
            Ok(StoredResultSet::default())
        }
        CatalogStatement::ShowDatabases { filter, .. } => {
            ddl::show_databases(&store, &catalog_id, filter.as_ref())
        }
        CatalogStatement::CreateExternalTable(table) => {
            ddl::create_external_table(&mut store, &catalog_id, table, now)
        }
        CatalogStatement::CreateView(view) => {
            ddl::create_view(&mut store, &catalog_id, view, now)
        }
        CatalogStatement::DropTable { target, if_exists } => {
            let dropped = ddl::drop_table(&mut store, &catalog_id, target, *if_exists)?;
            forget_partition_indexes(engine, &dropped);
            Ok(StoredResultSet::default())
        }
        CatalogStatement::DropView { target, if_exists } => {
            ddl::drop_view(&mut store, &catalog_id, target, *if_exists)
        }
        CatalogStatement::ShowCreateView { target } => {
            ddl::show_create_view(&store, &catalog_id, target)
        }
        CatalogStatement::ShowRelations {
            schema,
            views_only,
            filter,
        } => ddl::show_relations(&store, &catalog_id, schema, *views_only, filter.as_ref()),
        CatalogStatement::ShowPartitions { target } => {
            ddl::show_partitions(&store, &catalog_id, target)
        }
        CatalogStatement::ChangeTableColumns(change) => {
            ddl::change_table_columns(&mut store, &catalog_id, change, now)
        }
        CatalogStatement::ChangeTableProperties { target, properties } => {
            ddl::change_table_properties(&mut store, &catalog_id, target, properties, now)
        }
        // Performed before the catalog's guard was taken, because it reads the object store
        // as well.
        CatalogStatement::RepairTable { .. } => Ok(StoredResultSet::default()),
    }
}

/// Register the partitions the OBJECTS under one table's location are laid out under, which
/// is what `MSCK REPAIR TABLE` does.
///
/// The statement is the documented way to make Hive-style partition directories readable
/// after data was loaded into them: every directory under the table's location whose path
/// names a value for each of the table's partition keys is a partition of it, and the ones
/// the catalog has no record of are registered — so a later read of the table scans them and
/// `SHOW PARTITIONS` reports them. A partition the catalog already holds is left exactly as
/// it is, and a table that declares no partition key, or whose entry records no location, has
/// nothing to discover.
///
/// Each store is read under its OWN guard, in turn: the catalog says where to look and what
/// the keys are, the object store says which directories are there, and the catalog then
/// takes the records.
fn repair_table<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    target: &Relation,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let partitioned = {
        let store = state.lock();
        ddl::partitioned_location(&store, catalog_id, target)?
    };
    let Some((location, keys)) = partitioned else {
        return Ok(StoredResultSet::default());
    };
    let discovered = discovered_partitions(engine, &location, &keys)?;
    let mut store = state.lock();
    ddl::register_partitions(&mut store, catalog_id, target, &discovered, now)?;
    Ok(StoredResultSet::default())
}

/// The partitions the objects under `location` are laid out under: for every object there,
/// the values its own path names for the partition keys `keys`, and the directory those
/// values name.
///
/// The layout is Hive's own — one `<key>=<value>` directory per key, in key order — which is
/// the layout a read of a registered partition looks under and the one a statement that
/// writes a partitioned table lays its objects out in ([`data::hive_partition_location`]), so
/// discovery and reading share one rule. An object whose path names no value for one of the
/// keys belongs to no partition of the table and is passed over, exactly as a read of the
/// table passes over it.
///
/// BOUNDED by what the catalog could take anyway: how many objects the caller put at the
/// location is the caller's choice, so the walk stops once it has found as many distinct
/// partitions as one table may hold
/// ([`glue_state::MAX_PARTITIONS_PER_TABLE`]) — past that the catalog would
/// take no further record, so nothing is held for one.
fn discovered_partitions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    location: &str,
    keys: &[String],
) -> Result<Vec<(Vec<String>, String)>, Failure> {
    let Some((bucket_name, prefix)) = data::parse_s3_uri(location) else {
        return Err(Failure::location_not_readable(location));
    };
    let Some(state) = s3_state::service_state(engine) else {
        return Err(Failure::emulator(OBJECT_STORE_UNREACHABLE));
    };
    let store = state.lock();
    let Some(bucket) = store.bucket(&bucket_name) else {
        return Err(Failure::location_not_readable(location));
    };
    let mut discovered: Vec<(Vec<String>, String)> = Vec::new();
    let mut held: HashSet<Vec<String>> = HashSet::new();
    for (key, _) in bucket.objects() {
        if discovered.len() >= glue_state::MAX_PARTITIONS_PER_TABLE {
            break;
        }
        if !data::under_prefix(key, &prefix) {
            continue;
        }
        let Some(values) = data::hive_path_values(key, keys) else {
            continue;
        };
        // Many objects live under ONE partition directory, so each partition is discovered
        // once however many of them it holds.
        if !held.insert(values.clone()) {
            continue;
        }
        let at = data::hive_partition_location(location, keys, &values);
        discovered.push((values, at));
    }
    Ok(discovered)
}

/// Whether `statement` is a `DROP TABLE` whose name the DATA CATALOG holds no database
/// for, and which therefore addresses a table of the ENGINE's own.
///
/// Every table Athena's DDL registers lives in the catalog, and so does every table a
/// client registers through the catalog API — but a `CREATE TABLE` that declares no
/// location is not a catalog statement here ([`Plan::Engine`]) and defines its table in the
/// engine database the query's context addresses. A drop naming a database the catalog does
/// not hold can only be addressing one of those, so it is performed where its create was,
/// rather than refused by a catalog that was never asked to hold it. A database the catalog
/// DOES hold is a catalog database: a drop inside it is the catalog's, including its own
/// table-does-not-exist refusal.
fn drops_a_table_of_the_engines_own<Eng: Send + Sync + 'static>(
    engine: &Eng,
    statement: &CatalogStatement,
) -> bool {
    let CatalogStatement::DropTable { target, .. } = statement else {
        return false;
    };
    let Ok(catalog_id) = catalog_id_of(engine, &target.catalog, target.at) else {
        return false;
    };
    let Some(state) = glue_state::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    store.database(&catalog_id, &target.database).is_none()
}

/// Whether `writing` is a `CREATE TABLE … AS <query>` that declares no location in a
/// database the DATA CATALOG does not hold, and which therefore defines a table of the
/// ENGINE's own.
///
/// The same reading [`drops_a_table_of_the_engines_own`] takes of a drop, for the create it
/// is the counterpart of: a definition with no location of its own, in a database the catalog
/// never held, addresses no catalog table at all — there is no catalog database to register
/// it in and nowhere of its own for its data to go — so it defines its table where a
/// `CREATE TABLE` with no query defines one, in the engine database the query's context
/// addresses. A statement that DOES declare where its data lives, and one inside a database
/// the catalog holds, defines a catalog table; an `UNLOAD` defines nothing, so it is never
/// one of these.
fn defines_a_table_of_the_engines_own<Eng: Send + Sync + 'static>(
    engine: &Eng,
    writing: &WritingStatement,
) -> bool {
    let WriteDestination::Table(table) = &writing.destination else {
        return false;
    };
    if table
        .external_location
        .as_deref()
        .map(str::trim)
        .is_some_and(|location| !location.is_empty())
    {
        return false;
    }
    let Ok(catalog_id) = catalog_id_of(engine, &table.target.catalog, table.target.at) else {
        return false;
    };
    let Some(state) = glue_state::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    store
        .database(&catalog_id, &table.target.database)
        .is_none()
}

/// Whether `writing` is an `INSERT INTO` whose target the DATA CATALOG holds no table for,
/// and which therefore appends its rows to a table of the ENGINE's own.
///
/// The same reading [`drops_a_table_of_the_engines_own`] takes of a drop: a `CREATE TABLE`
/// that declares no location defines its table in the engine database the query's context
/// addresses, so a write into a name the catalog resolves no table for goes where that
/// create put it — either because the catalog holds no database of that name at all, or
/// because the database it holds has no such table while THIS engine database does. A name
/// the catalog does resolve is a catalog table: the rows are appended to its own data, which
/// includes the catalog's own table-does-not-exist refusal for one it no longer holds.
fn inserts_into_a_table_of_the_engines_own<Eng: Send + Sync + 'static>(
    engine: &Eng,
    handle: &DatabaseHandle,
    writing: &WritingStatement,
    context: &QueryContext,
) -> bool {
    let WriteDestination::Insert(insert) = &writing.destination else {
        return false;
    };
    let Ok(catalog_id) = catalog_id_of(engine, &insert.target.catalog, insert.target.at) else {
        return false;
    };
    let Some(state) = glue_state::service_state(engine) else {
        return false;
    };
    let recorded = {
        let store = state.lock();
        store
            .database(&catalog_id, &insert.target.database)
            .map(|database| database.tables.contains_key(&insert.target.table))
    };
    // The catalog holding the table settles it: the rows go into that table's own data.
    // Otherwise the name may be a table an earlier statement defined in THIS engine
    // database, which is where a read of that name resolves it too (see `resolved_sql`) —
    // and a name NEITHER holds is the catalog's own not-found refusal, reported in Athena's
    // words rather than the engine's.
    recorded != Some(true)
        && insert.target.database == glue_state::fold_name(&context.database)
        && query_engine::holds_relation(handle, &insert.target.table)
}

/// Forget the PARTITION INDEX records of the tables a drop removed.
///
/// An index is a resource OF its table, so a table the catalog no longer holds leaves none
/// behind — the same cascade the catalog's own deletes perform, taken in the same order:
/// the catalog's guard is still held while these records are written.
fn forget_partition_indexes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    tables: &[glue_state::TableEntry],
) {
    if tables.is_empty() {
        return;
    }
    if let Some(records) =
        ::service_com_amazonaws_glue_awsglue::service::partition_indexes::partition_index_store(
            engine,
        )
    {
        records.lock().forget_tables(tables.iter());
    }
}

/// The message for a store this engine does not carry.
const CATALOG_UNREACHABLE: &str =
    "the Data Catalog is not reachable on this engine, so no statement can resolve a table";

/// The message for an object store this engine does not carry.
const OBJECT_STORE_UNREACHABLE: &str =
    "the object store is not reachable on this engine, so no table's data can be read";

/// Whether `location` names a BUCKET the object store does not hold, so there is no
/// storage for a table declared over it.
///
/// A location is resolved against the object store before the statement is committed: the
/// bucket it names has to be one the account holds, because an external table is a
/// definition OVER data that already exists. A location that is no object-store path at all
/// names no bucket here, and so is not judged — the same reading
/// [`location_denotes_an_object`] takes of one.
fn location_bucket_is_absent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    location: Option<&str>,
) -> bool {
    let Some(location) = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
    else {
        return false;
    };
    let Some((bucket_name, _)) = data::parse_s3_uri(location) else {
        return false;
    };
    let Some(state) = s3_state::service_state(engine) else {
        // Without an object store there is nothing to resolve the location against, so the
        // statement is not refused over what cannot be read.
        return false;
    };
    let store = state.lock();
    store.bucket(&bucket_name).is_none()
}

/// Whether `location` denotes a single OBJECT rather than the folder an external table's
/// data lives under.
///
/// Athena's own rule for the clause is that a location names a FOLDER: its documentation
/// tells a caller to close a folder or bucket location with a trailing separator and never
/// to write a file name in it, and a statement that does name a file is refused rather
/// than registered. So both halves of what is known decide it:
///
/// * what the OBJECT STORE holds there — an object stored at exactly the key the location
///   names is a file, and a key other objects live UNDER is a folder, whatever either is
///   named;
/// * where the object store holds nothing at that key at all, the NAME itself: a location
///   closed with a separator names a folder, and one whose last segment carries a file
///   extension names a file.
///
/// A location that is no object-store path at all names no object here.
fn location_denotes_an_object<Eng: Send + Sync + 'static>(
    engine: &Eng,
    location: Option<&str>,
) -> bool {
    let Some(location) = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
    else {
        return false;
    };
    let Some((bucket_name, key)) =
        data::parse_s3_uri(location).filter(|(_, key)| !key.is_empty())
    else {
        return false;
    };
    match held_at(engine, &bucket_name, &key) {
        HeldAt::Object => true,
        HeldAt::Folder => false,
        HeldAt::Nothing => !location.ends_with('/') && names_a_file(&key),
    }
}

/// What the object store holds at one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldAt {
    /// An object stored at exactly that key.
    Object,
    /// No object at the key itself, but objects under it: a folder.
    Folder,
    /// Nothing at the key and nothing under it.
    Nothing,
}

/// What the object store holds at `key` of `bucket_name`, under ONE guard of its store.
fn held_at<Eng: Send + Sync + 'static>(engine: &Eng, bucket_name: &str, key: &str) -> HeldAt {
    let Some(state) = s3_state::service_state(engine) else {
        return HeldAt::Nothing;
    };
    let store = state.lock();
    let Some(bucket) = store.bucket(bucket_name) else {
        return HeldAt::Nothing;
    };
    if bucket.object(key).is_some() {
        return HeldAt::Object;
    }
    let under = format!("{key}/");
    if bucket.objects().any(|(held, _)| held.starts_with(&under)) {
        return HeldAt::Folder;
    }
    HeldAt::Nothing
}

/// Whether the last segment of `key` is a FILE name: a name closed by an extension, which
/// is the form Athena's documentation of the clause tells a caller not to write in it.
///
/// An extension is letters: a segment closed by digits — a dated or versioned folder, say
/// — is a folder name, and so is one carrying no extension at all.
fn names_a_file(key: &str) -> bool {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| {
            !stem.is_empty()
                && !extension.is_empty()
                && extension.len() <= MAX_FILE_EXTENSION
                && extension.chars().all(|letter| letter.is_ascii_alphabetic())
        })
}

/// How long the extension of a file name may be for [`names_a_file`] to read it as one.
const MAX_FILE_EXTENSION: usize = 10;

/// The `CatalogId` the data catalog `catalog` names, or `None` when this emulator holds no
/// metadata for it.
///
/// The same resolution a statement's own relations take ([`catalog_id_of`]), for a caller
/// that needs the id rather than the failure to report: the Lake Formation decision a
/// submission resolves before it is planned
/// ([`crate::state::lake_formation::resolved`]) is keyed by the id, and a catalog whose
/// metadata this emulator does not serve is a catalog nothing is known about.
pub fn catalog_id<Eng: Send + Sync + 'static>(engine: &Eng, catalog: &str) -> Option<String> {
    catalog_id_of(engine, catalog, Position::START).ok()
}

/// The Data Catalog id the data catalog `catalog` names, whichever ACCOUNT holds that
/// catalog — this one, or another whose catalog a registered `GLUE` catalog points at — or
/// `None` for a name that names no Glue Data Catalog at all.
///
/// A Glue `CatalogId` IS an account id ("If none is provided, the Amazon Web Services account
/// ID is used by default"), so this is also WHOSE catalog a statement over that name reaches:
/// what the caller needs permission on ([`crate::state::catalog_authorization`]) is the
/// catalog, database and table of that account. Whether this emulator HOLDS the catalog is
/// [`catalog_id`], which answers only for the one it serves.
pub fn glue_catalog_id<Eng: Send + Sync + 'static>(engine: &Eng, catalog: &str) -> Option<String> {
    let state = crate::state::service_state(engine)?;
    // The registry's guard is taken and released here, like every other reading of it.
    let source = {
        let store = state.lock();
        crate::state::metadata_source(&store.data_catalogs, catalog)
    };
    match source {
        crate::state::MetadataSource::GlueDataCatalog(catalog_id)
        | crate::state::MetadataSource::ForeignGlueCatalog(catalog_id) => Some(catalog_id),
        crate::state::MetadataSource::ExternalMetastore(_)
        | crate::state::MetadataSource::Unregistered => None,
    }
}

/// The `CatalogId` the data catalog `catalog` names, or why its metadata cannot be read.
///
/// The SAME resolution Athena's metadata reads use: `AwsDataCatalog` is the account's own
/// Glue Data Catalog, a registered `GLUE` catalog naming that catalog id is that catalog
/// under a second name, and one whose metadata an external metastore or another account's
/// catalog serves is not one this emulator holds.
fn catalog_id_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog: &str,
    at: Position,
) -> Result<String, Failure> {
    let Some(state) = crate::state::service_state(engine) else {
        return Err(Failure::emulator(
            "Athena state is not reachable on this engine",
        ));
    };
    // The registry's guard is taken and released here, before the catalog's own store is
    // reached, so no two stores are ever held at once.
    let source = {
        let store = state.lock();
        crate::state::metadata_source(&store.data_catalogs, catalog)
    };
    match source {
        crate::state::MetadataSource::GlueDataCatalog(catalog_id) => Ok(catalog_id),
        crate::state::MetadataSource::ExternalMetastore(catalog_type) => Err(Failure::emulator(
            crate::state::metastore_unreachable(catalog, &catalog_type),
        )),
        crate::state::MetadataSource::ForeignGlueCatalog(catalog_id) => Err(Failure::emulator(
            crate::state::foreign_glue_catalog(catalog, &catalog_id),
        )),
        crate::state::MetadataSource::Unregistered => Err(Failure::catalog_not_found(at, catalog)),
    }
}

/// A statement whose relations are resolved: the objects to materialise in the engine
/// database, the SQL that reads them, and the bytes the resolution read.
struct Prepared {
    /// The tables and views the statement reads, base tables first.
    objects: Vec<LocalObject>,
    /// The statement, with every resolved relation naming its materialised rows.
    sql: String,
    /// The bytes of object data this statement scanned.
    scanned_bytes: i64,
    /// The columns the relations this statement reads declare and the reader may not see,
    /// as the catalog folds their names — what makes a statement that names one of them a
    /// failure of authorization rather than a failure to resolve a name at all.
    hidden_columns: HashSet<String>,
}

/// Resolve every relation one statement reads, and render the SQL the engine runs.
///
/// `sql` is the text the statement was SUBMITTED as, which is where a refusal about the
/// statement reports a line and a column from.
fn resolved_sql<Eng: Send + Sync + 'static>(
    engine: &Eng,
    handle: &DatabaseHandle,
    relational: RelationalStatement,
    context: &QueryContext,
    access: &DataLakeAccess,
    scan_cutoff: Option<i64>,
    sql: &str,
) -> Result<Prepared, Failure> {
    let RelationalStatement {
        mut statement,
        relations,
    } = relational;
    // ONE budget for the whole statement: how many relations may be resolved at all
    // (which is what bounds the PRODUCT of the relations a statement names and the levels
    // of view they are defined over), how many rows and bytes may be materialised, how much
    // data the workgroup's own cutoff lets the query scan, and when the resolution must stop
    // whatever it has reached.
    let mut budget = Budget::for_scan_cutoff(scan_cutoff);
    let mut resolution = Resolution::default();
    let resolved = resolve(engine, &relations, 0, &mut resolution, &mut budget, access)?;

    // The catalog's guard is released before the object store's is taken, so no two
    // stores are ever held at once.
    let mut objects: Vec<LocalObject> = resolution
        .schema_tables
        .into_iter()
        .map(LocalObject::Table)
        .collect();
    let mut scanned_bytes = 0usize;
    let mut hidden_columns: HashSet<String> = HashSet::new();
    if !resolution.sources.is_empty() {
        // WHICH PARTITIONS each read scans: the statement's own predicate decides it, so a
        // read whose predicate fixes a partition key reads the data of the partitions that
        // carry it and not the data of the whole table.
        //
        // Only where the statement reads exactly ONE relation and nothing else: a column a
        // predicate names is resolved against the relations in scope, which this layer does
        // not do — it tracks no alias, and a relation a VIEW reads is materialised once for
        // the view and the statement both — so with more than one relation in play a
        // constraint cannot be attributed to the source it belongs to, and narrowing by it
        // could drop rows the statement asked for. Nothing is narrowed then, which costs a
        // read nothing but the bytes it scans.
        let constraints = statement::pruning_constraints(&statement);
        let one_relation = resolution.sources.len() == 1
            && resolution.views.is_empty()
            // `objects` holds the catalog's own reporting tables at this point, and
            // nothing else: a statement that reads one of those reads more than the
            // one relation below.
            && objects.is_empty();
        if !constraints.is_empty() && one_relation {
            for source in &mut resolution.sources {
                source.pruned(&constraints);
            }
        }
        let Some(state) = s3_state::service_state(engine) else {
            return Err(Failure::emulator(OBJECT_STORE_UNREACHABLE));
        };
        let store = state.lock();
        // The hidden metadata columns the statement NAMES: each one says something about the
        // OBJECT a row was read from rather than carrying a field of it, and a statement
        // sees one only where it addresses it — so a read carries exactly the ones this
        // statement reads, and a column list of `*` expands to none of them.
        let metadata: Vec<data::MetadataColumn> = data::MetadataColumn::ALL
            .iter()
            .copied()
            .filter(|column| statement::names_column(&statement, column.name()))
            .collect();
        // Which COLUMNS the statement reads, for the same reason: a columnar object holds
        // each column apart, so a read of one goes through the columns the statement names
        // and no others — while a row-format object is read whole whatever it projects.
        let read_columns = statement::read_columns(&statement);
        for source in &resolution.sources {
            hidden_columns.extend(source.hidden_columns());
            let (object, scanned) =
                source.read(&store, &mut budget, &metadata, read_columns.as_ref())?;
            scanned_bytes = scanned_bytes.saturating_add(scanned);
            objects.push(object);
        }
    }
    objects.extend(
        resolution
            .views
            .into_iter()
            .map(|(name, sql)| LocalObject::View { name, sql }),
    );

    // A relation the catalog holds nothing for may still be a table an earlier statement
    // defined in THIS engine database, which is where a query that defined its own table
    // reads it from; one that is neither is the catalog's own not-found refusal.
    let mut rewrites = resolved.rewrites;
    for (relation, miss) in resolved.unresolved {
        let local = relation.table.clone();
        if relation.database == glue_state::fold_name(&context.database)
            && query_engine::holds_relation(handle, &local)
        {
            rewrites.insert(relation.written.clone(), local);
            continue;
        }
        return Err(miss);
    }

    // What TYPE each column the statement can read has is settled here, while the catalog's
    // answer about those columns is in hand: an expression whose type the catalog fixes is
    // resolved in the parse before the engine is handed anything, because the engine holds
    // only the storage class of the values it was given (see [`expression_types`]). The
    // columns a Lake Formation grant hides are not among them — a statement cannot read
    // their type any more than their cells.
    let column_types = expression_types::ColumnTypes::of(
        resolution
            .sources
            .iter()
            .flat_map(data::DataSource::visible_columns),
    );
    // What those types make IMPOSSIBLE is judged here, before the engine is handed anything:
    // an operator applied across two type families it is not defined between is a failure of
    // Athena's analysis, and this engine would otherwise read one operand as the other and
    // answer a value.
    if let Some(found) = expression_types::mismatch(&statement, &column_types) {
        let at = statement::operator_position(sql, found.left_column.as_deref(), &found.operator)
            .unwrap_or(Position::START);
        return Err(Failure::type_mismatch(at, &found));
    }
    expression_types::resolve(&mut statement, &column_types);
    // A reading INTO a column the catalog declares as a STRUCTURE is resolved here for the
    // same reason: the engine holds such a value as the document the object carried it as,
    // and its own dialect has no production for a field, an element or an entry of one (see
    // [`nested_values`]).
    nested_values::resolve(
        &mut statement,
        &nested_values::structures(
            resolution
                .sources
                .iter()
                .flat_map(data::DataSource::visible_columns),
        ),
    );

    Ok(Prepared {
        sql: statement::rewritten(&mut statement, &rewrites),
        objects,
        scanned_bytes: i64::try_from(scanned_bytes).unwrap_or(i64::MAX),
        hidden_columns,
    })
}

/// What resolving a statement's relations accumulated, across every level of view it
/// followed.
#[derive(Debug, Default)]
struct Resolution {
    /// The catalog tables to read, in the order they were resolved.
    sources: Vec<DataSource>,
    /// The catalog's own reporting tables the statement reads, already built.
    schema_tables: Vec<LocalTable>,
    /// The views to define over them, base-first.
    views: Vec<(String, String)>,
    /// The local names already resolved, ACROSS the whole statement, so a relation two
    /// branches of it share is resolved and read exactly once.
    resolved: HashSet<String>,
}

/// What resolving ONE list of relations established for the statement that names them.
#[derive(Debug, Default)]
struct Resolved {
    /// Which local name each written relation is rewritten to.
    rewrites: HashMap<String, String>,
    /// The relations the catalog holds nothing for, each with the refusal to report when
    /// nothing else resolves them either.
    unresolved: Vec<(Relation, Failure)>,
}

/// Resolve `relations` against the Data Catalog, following a view to the relations it
/// reads, and charging every relation against `budget`.
///
/// `access` is Lake Formation's answer about the caller: a relation it covers no `SELECT`
/// of is resolved to nothing — the table is not there for this caller — and one it covers
/// a NARROWED `SELECT` of is read through that narrowing.
fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    relations: &[Relation],
    depth: usize,
    resolution: &mut Resolution,
    budget: &mut Budget,
    access: &DataLakeAccess,
) -> Result<Resolved, Failure> {
    if depth > MAX_VIEW_DEPTH {
        return Err(Failure::emulator(format!(
            "a view this statement reads is defined over more than {MAX_VIEW_DEPTH} levels \
             of view, which this emulator does not expand"
        )));
    }
    let mut resolved = Resolved::default();
    for relation in relations {
        let local = relation.local_name();
        if resolution.resolved.contains(&local) {
            resolved
                .rewrites
                .insert(relation.written.clone(), local.clone());
            continue;
        }
        budget.charge_relation()?;
        let catalog_id = catalog_id_of(engine, &relation.catalog, relation.at)?;
        // What LAKE FORMATION lets this caller read of the relation, decided before the
        // catalog is asked what it holds: a relation no `SELECT` of the caller's reaches is
        // hidden from it, and its cells are never read.
        let permitted =
            access.table_access(&catalog_id, &relation.database, &relation.table);
        // What the catalog holds for this relation is read under its guard, and the guard
        // is released before anything else is reached.
        let held = {
            let Some(state) = glue_state::service_state(engine) else {
                return Err(Failure::emulator(CATALOG_UNREACHABLE));
            };
            let store = state.lock();
            if relation.is_information_schema() {
                Held::ReportingTable(reporting_table(&store, &catalog_id, relation, budget)?)
            } else {
                match store.database(&catalog_id, &relation.database) {
                    // A database the catalog serves without holding a record of it holds no
                    // relation at all, so what the statement named is the relation that is
                    // not there ([`served_without_record`]).
                    None if served_without_record(&relation.database) => Held::NoTable,
                    None => Held::NoDatabase,
                    Some(database) => match database.tables.get(&relation.table) {
                        None => Held::NoTable,
                        // A relation Lake Formation hides is not there for this caller,
                        // whether it is a table or a view: the governance decision is taken
                        // on the relation the statement NAMES, and the relations a view
                        // reads are each governed in their own right when it is expanded.
                        //
                        // What a NARROWED grant on a view resource would show is not
                        // applied here: a view's rows are the rows its own query computes
                        // from the relations under it, and each of those is read through the
                        // grant the caller holds on IT — so the narrowing that reaches the
                        // caller is the one on the data, which is where Lake Formation holds
                        // a column scope and a row filter (`TableWithColumns` and
                        // `DataCellsFilter` both name a table).
                        Some(_) if matches!(permitted, TableAccess::Hidden) => Held::Hidden,
                        Some(table) if ddl::is_view(table) => Held::View(
                            table
                                .view_original_text
                                .as_deref()
                                .map(ddl::view_definition_sql)
                                .unwrap_or_default(),
                        ),
                        Some(table) => {
                            let source = DataSource::of(relation, table)?;
                            match &permitted {
                                TableAccess::Readable(scope) if !scope.is_unrestricted() => {
                                    let narrowed = source.narrowed(scope);
                                    // A grant that covers no column of the table lets its
                                    // holder read no cell of it, which is the same answer
                                    // as holding no grant at all.
                                    if narrowed.visible_columns().is_empty() {
                                        Held::Hidden
                                    } else {
                                        Held::Table(Box::new(narrowed))
                                    }
                                }
                                _ => Held::Table(Box::new(source)),
                            }
                        }
                    },
                }
            }
        };
        match held {
            Held::ReportingTable(table) => {
                resolution.schema_tables.push(table);
                resolution.resolved.insert(local.clone());
                resolved.rewrites.insert(relation.written.clone(), local);
            }
            Held::Table(source) => {
                resolution.sources.push(*source);
                resolution.resolved.insert(local.clone());
                resolved.rewrites.insert(relation.written.clone(), local);
            }
            Held::View(definition) => {
                resolution.resolved.insert(local.clone());
                resolved
                    .rewrites
                    .insert(relation.written.clone(), local.clone());
                let view_context = QueryContext {
                    catalog: relation.catalog.clone(),
                    database: relation.database.clone(),
                };
                let sql = view_sql(
                    engine,
                    &definition,
                    &view_context,
                    depth,
                    resolution,
                    budget,
                    access,
                )?;
                resolution.views.push((local, sql));
            }
            Held::NoDatabase => resolved
                .unresolved
                .push((relation.clone(), Failure::schema_not_found(relation))),
            Held::NoTable => resolved
                .unresolved
                .push((relation.clone(), Failure::table_not_found(relation))),
            // A relation the caller may not read is reported as one that is not there, and
            // nothing else of this engine database may answer for it: an earlier statement's
            // own table of that name is not what the statement addressed.
            Held::Hidden => return Err(Failure::table_not_found(relation)),
        }
    }
    Ok(resolved)
}

/// What the catalog holds for one relation.
enum Held {
    /// A table whose data lives at a location. BOXED: a data source carries the table's
    /// whole definition — its columns, its partitions and how its objects decode — and is
    /// much the largest thing this enum can hold.
    Table(Box<DataSource>),
    /// One of the catalog's own reporting tables, already built.
    ReportingTable(LocalTable),
    /// A view, carrying the query it is defined by.
    View(String),
    /// No database of that name.
    NoDatabase,
    /// No relation of that name in a database that is there.
    NoTable,
    /// A relation that IS there, which Lake Formation does not let this caller read.
    Hidden,
}

/// The SQL one view's rows come from: its own definition, with every relation IT reads
/// resolved and rewritten the same way the submitted statement's are.
fn view_sql<Eng: Send + Sync + 'static>(
    engine: &Eng,
    definition: &str,
    context: &QueryContext,
    depth: usize,
    resolution: &mut Resolution,
    budget: &mut Budget,
    access: &DataLakeAccess,
) -> Result<String, Failure> {
    let parsed = statement::parse_statements(definition)
        .and_then(|mut statements| (statements.len() == 1).then(|| statements.remove(0)));
    let Some(mut parsed) = parsed else {
        return Err(Failure::emulator(
            "a view this statement reads carries a definition that is not one SQL \
             statement, so its rows cannot be read",
        ));
    };
    let relations = statement::relations_of(&parsed, context);
    if relations.is_empty() {
        return Ok(statement::rewritten(&mut parsed, &HashMap::new()));
    }
    let resolved = resolve(engine, &relations, depth + 1, resolution, budget, access)?;
    // A view whose own source is missing cannot be read at all: the view is there, and
    // the relation under it is not.
    if let Some((_, miss)) = resolved.unresolved.into_iter().next() {
        return Err(miss);
    }
    Ok(statement::rewritten(&mut parsed, &resolved.rewrites))
}

/// One of the catalog's own reporting tables, as the rows it holds.
///
/// `information_schema` is how a SQL client asks the engine what the catalog holds, and
/// every row below is read off the catalog itself: the databases it holds, the relations
/// in each, what kind each relation is, and the columns each one declares.
fn reporting_table(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    relation: &Relation,
    budget: &mut Budget,
) -> Result<LocalTable, Failure> {
    let catalog = relation.catalog.to_ascii_lowercase();
    let held = store.catalogs.get(catalog_id);
    let databases = held.map(glue_state::Catalog::databases_ordered).unwrap_or_default();
    let mut rows: Vec<Vec<::rusqlite::types::Value>> = Vec::new();
    let columns: Vec<&str> = match relation.table.as_str() {
        "schemata" => {
            for database in &databases {
                budget.charge_row(relation, SCHEMATA_COLUMNS)?;
                rows.push(vec![text(&catalog), text(&database.name)]);
            }
            vec!["catalog_name", "schema_name"]
        }
        "tables" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    budget.charge_row(relation, TABLES_COLUMNS)?;
                    rows.push(vec![
                        text(&catalog),
                        text(&database.name),
                        text(&table.name),
                        text(if ddl::is_view(table) {
                            RELATION_TYPE_VIEW
                        } else {
                            RELATION_TYPE_BASE_TABLE
                        }),
                    ]);
                }
            }
            vec!["table_catalog", "table_schema", "table_name", "table_type"]
        }
        "views" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    if !ddl::is_view(table) {
                        continue;
                    }
                    budget.charge_row(relation, VIEWS_COLUMNS)?;
                    rows.push(vec![
                        text(&catalog),
                        text(&database.name),
                        text(&table.name),
                        text(
                            &table
                                .view_original_text
                                .as_deref()
                                .map(ddl::view_definition_sql)
                                .unwrap_or_default(),
                        ),
                    ]);
                }
            }
            vec![
                "table_catalog",
                "table_schema",
                "table_name",
                "view_definition",
            ]
        }
        "columns" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    for (position, column) in ddl::relation_columns(table).iter().enumerate() {
                        budget.charge_row(relation, COLUMNS_COLUMNS)?;
                        rows.push(vec![
                            text(&catalog),
                            text(&database.name),
                            text(&table.name),
                            text(&column.name),
                            ::rusqlite::types::Value::Integer(
                                i64::try_from(position.saturating_add(1)).unwrap_or(i64::MAX),
                            ),
                            text(&column.data_type),
                            text(COLUMN_IS_NULLABLE),
                            column
                                .comment
                                .as_deref()
                                .map_or(::rusqlite::types::Value::Null, text),
                        ]);
                    }
                }
            }
            vec![
                "table_catalog",
                "table_schema",
                "table_name",
                "column_name",
                "ordinal_position",
                "data_type",
                "is_nullable",
                "comment",
            ]
        }
        _ => return Err(Failure::table_not_found(relation)),
    };
    Ok(LocalTable {
        name: relation.local_name(),
        columns: columns
            .into_iter()
            .map(|name| LocalColumn {
                name: name.to_string(),
                declared: data::engine_type(if name == "ordinal_position" {
                    "int"
                } else {
                    "string"
                }),
            })
            .collect(),
        rows,
    })
}

/// How many columns `information_schema.schemata` reports.
const SCHEMATA_COLUMNS: usize = 2;

/// How many columns `information_schema.tables` reports.
const TABLES_COLUMNS: usize = 4;

/// How many columns `information_schema.views` reports.
const VIEWS_COLUMNS: usize = 4;

/// How many columns `information_schema.columns` reports.
const COLUMNS_COLUMNS: usize = 8;

/// How `information_schema.tables` classifies a view.
const RELATION_TYPE_VIEW: &str = "VIEW";

/// How `information_schema.tables` classifies a table that holds data.
const RELATION_TYPE_BASE_TABLE: &str = "BASE TABLE";

/// What `information_schema.columns` reports for a column's nullability: the catalog
/// records none for a Hive column, and every such column admits a missing value.
const COLUMN_IS_NULLABLE: &str = "YES";

/// One text value, as the engine holds it.
fn text(value: &str) -> ::rusqlite::types::Value {
    ::rusqlite::types::Value::Text(value.to_string())
}
