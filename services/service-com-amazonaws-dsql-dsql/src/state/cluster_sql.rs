// SPDX-License-Identifier: CC0-1.0

//! The SQL a session on a cluster's endpoint speaks: AURORA DSQL's dialect, and
//! the semantics a statement of it is answered under.
//!
//! Aurora DSQL is a PostgreSQL-COMPATIBLE engine rather than a PostgreSQL
//! server, and a client tells them apart by what the server answers. Three of
//! those answers are this dialect's, and each is a fact about the product and
//! not about the engine behind the endpoint
//! ([`::service_common::sql_wire_planes::sql_engine`]):
//!
//! * the sentence `version()` answers, which names the PostgreSQL version this
//!   engine is compatible with and the service behind it;
//! * the ISOLATION a transaction runs at unless it asks for another, which here
//!   is `repeatable read` — the level a snapshot-isolated, optimistically
//!   concurrent engine runs at, and not stock PostgreSQL's `read committed`.
//!   The concurrency control itself is the shared statement of an optimistic one
//!   ([`::service_common::sql_wire_planes::optimistic_writers`]), which this
//!   cluster's plane states it serves; what is here is the sentence a refused
//!   commit carries and the level a client reads back;
//! * the features this engine does NOT implement, each refused under the class
//!   its condition has rather than quietly accepted: a column of the `serial`
//!   family, which denotes a SEQUENCE and Aurora DSQL has none, and a TEMPORARY
//!   relation.
//!
//! Everything else goes to the engine exactly as the client wrote it, and what
//! the engine answers — its rows, its counts and its own error messages — is what
//! the client is told. A statement that reads the standard catalog is served the
//! one reading of the engine's own catalog
//! ([`::service_common::sql_wire_planes::catalog_views`]), materialised
//! immediately before the statement runs, so what `information_schema` reports is
//! the relations this cluster really holds.

use ::service_common::sql_wire_planes::catalog_views;
use ::service_common::sql_wire_planes::sql_engine::{
    DatabaseEngine, EngineError, EngineErrorKind, EngineView,
};
use ::service_common::sql_wire_planes::{DialectReading, Refusal, SqlDialect};
use ::sqlparser::ast::{AlterTableOperation, ColumnDef, Statement};
use ::sqlparser::dialect::PostgreSqlDialect;
use ::sqlparser::parser::Parser;
use ::sqlparser::tokenizer::{Token, Tokenizer};

/// The `SQLSTATE` a commit refused because another transaction wrote the same
/// row first carries: `serialization_failure`, which is what a client of an
/// optimistically concurrent engine retries on.
const SERIALIZATION_FAILURE: &str = "40001";

/// The sentence that refusal carries, as Aurora DSQL words it — fixed text, with
/// no variable part.
///
/// Recorded behaviour: the primary message of the losing commit's
/// `ErrorResponse`, reported at severity `ERROR` under
/// [`SERIALIZATION_FAILURE`].
const WRITE_CONFLICT_MESSAGE: &str = "change conflicts with another transaction (OC000)";

/// The `SQLSTATE` a type the engine does not have is refused under:
/// `undefined_object`, which is the class PostgreSQL's "Appendix A. Error Codes"
/// names for a declaration naming a type that does not exist.
const UNDEFINED_OBJECT: &str = "42704";

/// The `SQLSTATE` a construct this engine does not implement is refused under:
/// `feature_not_supported`.
const FEATURE_NOT_SUPPORTED: &str = "0A000";

/// The types that declare a SEQUENCE-backed column, which is what Aurora DSQL
/// has none of.
///
/// Source: PostgreSQL's "Numeric Types", whose serial table lists `smallserial`
/// / `serial2`, `serial` / `serial4` and `bigserial` / `serial8` as "not true
/// types, but merely a notational convenience for creating unique identifier
/// columns" — the notation that creates a sequence. An engine without sequences
/// therefore has no such type, and a declaration naming one names a type that
/// does not exist.
const SEQUENCE_TYPES: &[&str] = &[
    "smallserial",
    "serial2",
    "serial",
    "serial4",
    "bigserial",
    "serial8",
];

/// The dialect of the engine behind a cluster's endpoint.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClusterSql;

impl SqlDialect for ClusterSql {
    /// Read one statement as Aurora DSQL reads it: refuse what this engine does
    /// not implement, put the standard catalog on the engine for a statement
    /// that reads it, and hand everything else to the engine as written.
    fn read(
        &self,
        statement: &str,
        engine: &DatabaseEngine,
        view: &mut EngineView,
    ) -> DialectReading {
        if let Some(refused) = unsupported_construct(statement) {
            return DialectReading::Refused(refused);
        }
        if reads_the_standard_catalog(statement) {
            let views = catalog_views::CATALOG_VIEWS.to_vec();
            let catalog = view.database.clone();
            if let Err(error) = catalog_views::materialise(engine, view, &catalog, &views) {
                return DialectReading::Refused(error);
            }
            // The engine holds one database and no schemas, so a relation of the
            // standard catalog is held under its qualified name as one
            // identifier: the client's own spelling is read as that.
            if let Some(respelled) = ::service_common::sql_wire_planes::dialect::
                respell_schema_relations(statement, catalog_views::INFORMATION_SCHEMA)
            {
                return DialectReading::Run(respelled);
            }
        }
        DialectReading::Run(statement.to_string())
    }

    /// The sentence `version()` answers: the PostgreSQL version this engine is
    /// compatible with — the one the endpoint publishes as `server_version` — and
    /// the service that serves it.
    ///
    /// Source: the Amazon Aurora DSQL User Guide's "PostgreSQL compatibility and
    /// feature differences", which states the PostgreSQL version a cluster is
    /// compatible with. A client reads this sentence to tell what it is talking
    /// to, so it names Aurora DSQL after the version, exactly as a managed
    /// PostgreSQL-compatible engine's own `version()` does.
    fn version_sentence(&self, server_version: &str) -> Option<String> {
        Some(format!(
            "PostgreSQL {server_version} on aarch64-unknown-linux-gnu, Aurora DSQL"
        ))
    }

    /// How Aurora DSQL reports a refused statement, where the `SQLSTATE` or the
    /// sentence is this service's own rather than the wire's mapping of the
    /// engine's class:
    ///
    /// * a commit refused because another transaction wrote the same row first
    ///   carries this service's own sentence under [`SERIALIZATION_FAILURE`];
    /// * a type this engine does not have is [`UNDEFINED_OBJECT`], the class
    ///   PostgreSQL reports a declaration naming a type that does not exist
    ///   under, and a construct it does not implement is
    ///   [`FEATURE_NOT_SUPPORTED`] — each with the engine's own sentence, which
    ///   is the one this dialect wrote when it refused the statement.
    fn refusal(&self, error: &EngineError) -> Option<Refusal> {
        let sqlstate = match error.kind {
            EngineErrorKind::SerializationFailure => {
                return Some(Refusal {
                    sqlstate: SERIALIZATION_FAILURE,
                    message: WRITE_CONFLICT_MESSAGE.to_string(),
                    detail: None,
                })
            }
            EngineErrorKind::UndefinedType => UNDEFINED_OBJECT,
            EngineErrorKind::FeatureNotSupported => FEATURE_NOT_SUPPORTED,
            _ => return None,
        };
        Some(Refusal {
            sqlstate,
            message: error.message.clone(),
            detail: None,
        })
    }

    /// The settings a session on a cluster starts with: the ISOLATION a
    /// transaction runs at unless it asks for another.
    ///
    /// Aurora DSQL runs a transaction on the snapshot taken when it began and
    /// settles two writers of one row at commit, which is `repeatable read` —
    /// the level a client reads back from `default_transaction_isolation`, and
    /// not stock PostgreSQL's `read committed`. `transaction_isolation` is the
    /// same value for the transaction a session is in, because nothing on this
    /// endpoint runs at another level.
    fn session_settings(&self) -> Vec<(String, String)> {
        ["default_transaction_isolation", "transaction_isolation"]
            .iter()
            .map(|name| ((*name).to_string(), REPEATABLE_READ.to_string()))
            .collect()
    }
}

/// The isolation level every transaction on a cluster runs at.
pub const REPEATABLE_READ: &str = "repeatable read";

/// The refusal `statement` meets because it asks for a construct this engine
/// does not implement, and `None` for every other statement.
///
/// Read off the PARSED statement — the maintained parser of the wire's own SQL
/// dialect — so a `serial` inside a string literal or a column named `temporary`
/// is not read as the construct, and a statement the parser cannot read at all
/// is left for the engine to report in its own words.
fn unsupported_construct(statement: &str) -> Option<EngineError> {
    let parsed = Parser::parse_sql(&PostgreSqlDialect {}, statement).ok()?;
    parsed.iter().find_map(refused_statement)
}

/// The refusal one parsed statement meets, where it asks for a construct this
/// engine does not implement.
fn refused_statement(statement: &Statement) -> Option<EngineError> {
    match statement {
        Statement::CreateTable(created) => {
            if created.temporary {
                return Some(temporary_relation("TABLE"));
            }
            created.columns.iter().find_map(sequence_column)
        }
        Statement::CreateView { temporary, .. } if *temporary => Some(temporary_relation("VIEW")),
        Statement::CreateSequence { .. } => Some(no_sequences()),
        Statement::AlterTable { operations, .. } => {
            operations.iter().find_map(|operation| match operation {
                AlterTableOperation::AddColumn { column_def, .. } => sequence_column(column_def),
                _ => None,
            })
        }
        _ => None,
    }
}

/// The refusal a column declared with a type of the `serial` family meets: the
/// type does not exist on an engine that has no sequences, which is the sentence
/// PostgreSQL's `undefined_object` carries for a declaration naming a type it
/// does not have.
fn sequence_column(column: &ColumnDef) -> Option<EngineError> {
    let declared = column.data_type.to_string().to_ascii_lowercase();
    let named = declared
        .split_once('(')
        .map_or(declared.as_str(), |(base, _)| base)
        .trim();
    SEQUENCE_TYPES.contains(&named).then(|| {
        EngineError::new(
            EngineErrorKind::UndefinedType,
            format!("type \"{named}\" does not exist"),
        )
    })
}

/// The refusal a `CREATE SEQUENCE` meets, which is the same absence the `serial`
/// family's types are: this engine has no sequences at all.
fn no_sequences() -> EngineError {
    EngineError::new(
        EngineErrorKind::FeatureNotSupported,
        "CREATE SEQUENCE is not supported".to_string(),
    )
}

/// The refusal a TEMPORARY relation of kind `relation` meets.
///
/// Source: the Amazon Aurora DSQL User Guide's "Unsupported PostgreSQL
/// features", which lists temporary tables among the features a cluster does not
/// serve. A construct a server does not implement is PostgreSQL's
/// `feature_not_supported`, whose sentence names the construct that was asked
/// for.
fn temporary_relation(relation: &str) -> EngineError {
    EngineError::new(
        EngineErrorKind::FeatureNotSupported,
        format!("CREATE TEMPORARY {relation} is not supported"),
    )
}

/// Whether `statement` reads the standard catalog, which is what has to be on
/// the engine before it runs.
///
/// Read off the TOKEN stream of the wire's own dialect, so the schema's name
/// inside a string literal, a quoted identifier or a comment is not read as a
/// reference to it.
fn reads_the_standard_catalog(statement: &str) -> bool {
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, statement).tokenize() else {
        return false;
    };
    tokens.iter().any(|token| match token {
        Token::Word(word) => word
            .value
            .eq_ignore_ascii_case(catalog_views::INFORMATION_SCHEMA),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal `statement` meets, as its `SQLSTATE` and its sentence.
    fn refusal_of(statement: &str) -> Option<(&'static str, String)> {
        let error = unsupported_construct(statement)?;
        let sqlstate = match error.kind {
            EngineErrorKind::UndefinedType => UNDEFINED_OBJECT,
            EngineErrorKind::FeatureNotSupported => FEATURE_NOT_SUPPORTED,
            other => panic!("unexpected class {other:?} for {statement:?}"),
        };
        Some((sqlstate, error.message))
    }

    /// One refusal case: the case, the statement, and the `SQLSTATE` and
    /// sentence it meets (`None` for a statement this dialect leaves for the
    /// engine).
    type RefusalCase<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);

    /// RULE: a construct this engine does not implement is refused under the
    /// class its condition has — a `serial` column names a type that does not
    /// exist, a temporary relation is a feature that is not supported — and
    /// every other statement is left for the engine.
    ///
    /// The rows span the whole `serial` family and both spellings of each, the
    /// statements that can declare such a column (a create and an alter), the
    /// temporary relations, and the statements that merely MENTION the words.
    #[test]
    fn a_construct_this_engine_does_not_implement_is_refused_under_its_own_class() {
        // (case, the statement, the refusal it meets)
        let rows: &[RefusalCase<'_>] = &[
            (
                "a serial key",
                "CREATE TABLE t (id SERIAL PRIMARY KEY, note TEXT)",
                Some((UNDEFINED_OBJECT, "type \"serial\" does not exist")),
            ),
            (
                "a bigserial column",
                "CREATE TABLE t (id BIGSERIAL, note TEXT)",
                Some((UNDEFINED_OBJECT, "type \"bigserial\" does not exist")),
            ),
            (
                "a smallserial column",
                "CREATE TABLE t (id SMALLSERIAL)",
                Some((UNDEFINED_OBJECT, "type \"smallserial\" does not exist")),
            ),
            (
                "the serial4 spelling",
                "CREATE TABLE t (id SERIAL4)",
                Some((UNDEFINED_OBJECT, "type \"serial4\" does not exist")),
            ),
            (
                "the serial8 spelling",
                "CREATE TABLE t (id SERIAL8)",
                Some((UNDEFINED_OBJECT, "type \"serial8\" does not exist")),
            ),
            (
                "the serial2 spelling",
                "CREATE TABLE t (id SERIAL2)",
                Some((UNDEFINED_OBJECT, "type \"serial2\" does not exist")),
            ),
            (
                "a serial column added later",
                "ALTER TABLE t ADD COLUMN counter SERIAL",
                Some((UNDEFINED_OBJECT, "type \"serial\" does not exist")),
            ),
            (
                "a temporary table",
                "CREATE TEMP TABLE t (id INTEGER)",
                Some((
                    FEATURE_NOT_SUPPORTED,
                    "CREATE TEMPORARY TABLE is not supported",
                )),
            ),
            (
                "a temporary table, spelled in full",
                "CREATE TEMPORARY TABLE t (id INTEGER)",
                Some((
                    FEATURE_NOT_SUPPORTED,
                    "CREATE TEMPORARY TABLE is not supported",
                )),
            ),
            (
                "a sequence of its own",
                "CREATE SEQUENCE orders_id_seq",
                Some((FEATURE_NOT_SUPPORTED, "CREATE SEQUENCE is not supported")),
            ),
            ("an ordinary table", "CREATE TABLE t (id INTEGER PRIMARY KEY, note TEXT)", None),
            (
                "an integer column of every other type",
                "CREATE TABLE t (a SMALLINT, b INTEGER, c BIGINT, d NUMERIC(9,2))",
                None,
            ),
            (
                "a column whose NAME is the word",
                "CREATE TABLE t (serial INTEGER)",
                None,
            ),
            (
                "the word inside a value",
                "INSERT INTO t (note) VALUES ('serial')",
                None,
            ),
            (
                "a table whose name is the word",
                "CREATE TABLE temporary (id INTEGER)",
                None,
            ),
            ("an ordinary read", "SELECT * FROM t", None),
        ];
        for (case, statement, want) in rows {
            let refused = refusal_of(statement);
            let want = want.map(|(state, message)| (state, (*message).to_string()));
            assert_eq!(
                refused, want,
                "case {case:?}: expected {want:?} for {statement:?}, got {refused:?}"
            );
        }
    }

    /// RULE: the standard catalog is put on the engine for a statement that
    /// REFERENCES it, and not for one that merely carries the words.
    #[test]
    fn the_standard_catalog_is_materialised_for_a_statement_that_reads_it() {
        // (case, the statement, whether it reads the catalog)
        let rows: &[(&str, &str, bool)] = &[
            (
                "a read of the tables view",
                "SELECT table_name FROM information_schema.tables",
                true,
            ),
            (
                "a read of the columns view, qualified in another case",
                "SELECT * FROM INFORMATION_SCHEMA.COLUMNS WHERE table_name = 't'",
                true,
            ),
            (
                "a count over it",
                "SELECT count(*) FROM information_schema.tables WHERE table_name = 'serial_t'",
                true,
            ),
            ("an ordinary read", "SELECT * FROM t", false),
            (
                "the words inside a value",
                "INSERT INTO t (note) VALUES ('information_schema.tables')",
                false,
            ),
            (
                "the words inside a comment",
                "SELECT 1 -- information_schema.tables\n",
                false,
            ),
        ];
        for (case, statement, want) in rows {
            let reads = reads_the_standard_catalog(statement);
            assert_eq!(
                reads, *want,
                "case {case:?}: expected {want} for {statement:?}, got {reads}"
            );
        }
    }

    /// RULE: a session on a cluster reports the isolation level this engine runs
    /// at, and `version()` names the PostgreSQL version it is compatible with.
    #[test]
    fn a_session_reports_this_engines_isolation_and_version() {
        let dialect = ClusterSql;
        let settings = dialect.session_settings();
        for name in ["default_transaction_isolation", "transaction_isolation"] {
            let held = settings
                .iter()
                .find(|(setting, _)| setting == name)
                .map(|(_, value)| value.clone());
            assert_eq!(
                held.as_deref(),
                Some(REPEATABLE_READ),
                "case {name:?}: expected {REPEATABLE_READ:?}, got {held:?}"
            );
        }
        for version in ["16.9", "16.4"] {
            let sentence = dialect.version_sentence(version);
            let sentence = sentence.unwrap_or_default();
            assert!(
                sentence.starts_with(&format!("PostgreSQL {version} ")),
                "case {version:?}: the sentence must name the PostgreSQL version first, got \
                 {sentence:?}"
            );
            assert!(
                sentence.contains("Aurora DSQL"),
                "case {version:?}: the sentence must name the service, got {sentence:?}"
            );
        }
    }

    /// RULE: the refusals this dialect owns carry the `SQLSTATE` Aurora DSQL
    /// reports them under — its own sentence for a write conflict, and the
    /// engine's own sentence for the constructs it refuses — and no other class
    /// is re-worded.
    #[test]
    fn a_write_conflict_is_reported_as_this_services_own_refusal() {
        let dialect = ClusterSql;
        let conflict = dialect
            .refusal(&EngineError::new(
                EngineErrorKind::SerializationFailure,
                "could not serialize access due to concurrent update".to_string(),
            ))
            .expect("a write conflict is re-worded");
        assert_eq!(
            (conflict.sqlstate, conflict.message.as_str()),
            (SERIALIZATION_FAILURE, WRITE_CONFLICT_MESSAGE),
            "case the conflict: expected the service's own refusal, got {conflict:?}"
        );
        // (case, the class the dialect refused the statement under, its SQLSTATE)
        for (case, kind, sqlstate) in [
            ("a type this engine does not have", EngineErrorKind::UndefinedType, UNDEFINED_OBJECT),
            (
                "a construct it does not implement",
                EngineErrorKind::FeatureNotSupported,
                FEATURE_NOT_SUPPORTED,
            ),
        ] {
            let refusal = dialect
                .refusal(&EngineError::new(kind, "the engine's own".to_string()))
                .unwrap_or_else(|| panic!("case {case:?}: the class must carry its SQLSTATE"));
            assert_eq!(
                (refusal.sqlstate, refusal.message.as_str()),
                (sqlstate, "the engine's own"),
                "case {case:?}: expected {sqlstate} with the engine's own sentence, got \
                 {refusal:?}"
            );
        }
        for kind in [
            EngineErrorKind::UniqueViolation,
            EngineErrorKind::UndefinedObject,
            EngineErrorKind::Syntax,
        ] {
            let left = dialect.refusal(&EngineError::new(kind, "the engine's own".to_string()));
            assert!(
                left.is_none(),
                "case {kind:?}: the engine's own class and message must be left alone, got \
                 {left:?}"
            );
        }
    }
}
