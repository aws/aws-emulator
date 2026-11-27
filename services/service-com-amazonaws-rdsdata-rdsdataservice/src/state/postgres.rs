// SPDX-License-Identifier: CC0-1.0

//! The PostgreSQL front end of this service's data plane.
//!
//! The Data API runs its statements against Aurora, and the engine this emulator
//! answers for is Aurora PostgreSQL, so a statement is PARSED AS POSTGRESQL
//! (`sqlparser`'s PostgreSQL dialect) before the SQL engine behind it is asked to
//! run anything. The parse decides four things:
//!
//! * the statement the engine runs — the same statement, rewritten into the form
//!   the engine's own grammar accepts. `value::type` is PostgreSQL's spelling of
//!   `CAST(value AS type)`, which is the spelling the engine takes;
//! * the PostgreSQL type of every result column the statement's own text
//!   determines (a cast, a literal), which is what `columnMetadata` reports for a
//!   computed column and what decides which `Field` member carries its values;
//! * which columns a `CREATE TABLE` declares with a sequence (PostgreSQL's
//!   `serial` family), so the engine assigns their values when an INSERT leaves
//!   them out, the way the sequence default does;
//! * whether the statement changes rows, which is what `numberOfRecordsUpdated`
//!   counts.
//!
//! Nothing here inspects SQL text: the only input to every decision is the parsed
//! statement, and the statement the engine runs is this module's rendering of that
//! same parse.
//!
//! ## The dialect a statement is written in
//!
//! The Data API serves Aurora MySQL DB clusters as well as Aurora PostgreSQL ones, so a
//! statement reaching it may be written in MySQL's dialect. So a statement is READ as
//! PostgreSQL first and as MySQL second ([`read`]), which is what lets a form only MySQL
//! spells (an `AUTO_INCREMENT` column definition) run as the statement it is; and where
//! the two dialects spell ONE statement differently, the rewrite below renders it as the
//! statement the engine runs: MySQL's upsert (`INSERT … ON DUPLICATE KEY UPDATE`) is the
//! engine's own `INSERT … ON CONFLICT (<key>) DO UPDATE SET` ([`plan_upsert`]), with the
//! key the conflict is on named from the table's own declaration — which only the
//! database holds, so the caller of [`plan`] supplies it.
//!
//! ## The databases a cluster holds
//!
//! `CREATE DATABASE` and `DROP DATABASE` are statements about the cluster's DATABASES
//! rather than about the rows of one of them, and each database here is its own engine:
//! [`namespace_statement`] is what recognises them, and the cluster applies them.
//!
//! ## The diagnostics here are PostgreSQL's
//!
//! An error the DATABASE raised reaches the caller in the words of the engine the addressed
//! cluster RUNS, so the sentences below are the ones an Aurora PostgreSQL cluster answers
//! with; `crate::state::mysql` holds the same conditions as an Aurora MySQL cluster words
//! them, and `crate::state::Database::in_the_engines_words` is what chooses between them.
//!
//! ## The type catalog
//!
//! [`pg_type`] maps a type as a declaration or a cast spells it to the type
//! PostgreSQL's catalog holds, and to what the Data API reports for it. The type
//! codes are `java.sql.Types`' constants, which is what `ColumnMetadata.type`
//! carries; the names are PostgreSQL's own catalog names, and the accepted
//! spellings are the aliases PostgreSQL's "Data Types" chapter documents for each
//! one (`integer`, `int` and `int4` are one type). A precision is the number of
//! decimal digits the type's documented range holds (`smallint` spans ±32767, so
//! 5; `integer` ±2147483647, so 10; `bigint` 19; `double precision` carries 17
//! significant digits; `boolean` 1), a length-parameterised type reports the
//! length its declaration gave, and a type with no length limit reports the
//! largest value the member can carry.

use std::collections::HashMap;
use std::ops::ControlFlow;

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, CastKind, ColumnDef, ColumnOption, ColumnOptionDef,
    ConflictTarget, DataType, DoUpdate, ExactNumberInfo, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments, GroupByExpr, Ident, ObjectName, ObjectNamePart,
    ObjectType, OnConflict, OnConflictAction, OnInsert, OrderByKind, Query, SelectItem, SetExpr,
    Statement, TableConstraint, TableFactor, TableObject, Value, visit_expressions,
    visit_expressions_mut, CreateTableOptions, SqlOption,
};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use super::condition::{Condition, condition_of, covers_the_same_columns};
use super::{MAX_DECLARED_ARGUMENT, SqlError};

/// How a name the caller wrote is rendered for the engine.
mod names;

/// The schema a name is resolved in, and the catalog relations that describe the database.
mod catalog;
pub(crate) use catalog::catalog_name;
pub(crate) use catalog::PUBLIC_SCHEMA;

/// The precision a type with no length limit reports: the largest length the
/// member can carry, which is what a client reads as "unlimited".
const UNLIMITED_LENGTH: i32 = i32::MAX;

/// The scale a date-and-time type that carries a TIME reports: PostgreSQL's "Date/Time
/// Types" documents its time types as resolving to one microsecond, which is six
/// fractional digits.
const MICROSECOND_SCALE: i32 = 6;

/// How deeply one statement may nest an expression before the PARSE refuses it.
///
/// A bound on the parser itself, and so on the work one request can ask of it:
/// what it costs to parse a nested call grows with the nesting, so the nesting is
/// capped — well above what a client writes by hand (a statement a person writes
/// nests a handful of levels) and far below where the parse becomes expensive.
/// The engine carries its own, separate expression-depth limit for what it is
/// then asked to run.
const MAX_PARSE_DEPTH: usize = 16;

/// How many queries one statement may combine with `UNION`, `INTERSECT` or
/// `EXCEPT`.
///
/// The ENGINE's own compound-select limit, read from where it is set on the
/// connection, so this refuses nothing the engine would have run — and it bounds
/// a chain the PARSE does not: a combined query is parsed iteratively, so its
/// depth grows with the request text, while the tree it builds is walked and
/// dropped by recursion.
const MAX_SET_OPERATIONS: usize = super::MAX_COMPOUND_SELECT as usize;

/// How many of a statement's tokens may each make the statement tree one level
/// deeper (`a AND b AND …`, `a || b || …`, `a::text::text::…`).
///
/// Bounded for the same reason as the set operations above, and well above what
/// a client writes: a statement a person writes carries a few dozen such tokens,
/// and a generated one lists its values — which are not counted — rather than
/// chaining operators over them.
const MAX_CHAINED_LEVELS: usize = 1_024;

/// How many levels a statement's text could ask the tree to carry, as its tokens
/// show: the queries it combines, and the tokens that could each deepen it.
///
/// Counted on the TOKENS rather than on the tree, so the answer is reached
/// without building the tree it is about.
fn chained_levels(tokens: &[TokenWithSpan]) -> (usize, usize) {
    let mut set_operations = 0;
    let mut levels = 0;
    for token in tokens.iter().map(|token| &token.token) {
        if let Token::Word(word) = token {
            if matches!(
                word.keyword,
                Keyword::UNION | Keyword::INTERSECT | Keyword::EXCEPT | Keyword::MINUS
            ) {
                set_operations += 1;
            }
        }
        if deepens_the_tree(token) {
            levels += 1;
        }
    }
    (set_operations, levels)
}

/// Whether a token could make the tree one level deeper without the parse
/// recursing — an operator, a keyword that joins two operands, a cast, a
/// subscript.
///
/// CLOSED, and so an over-count rather than an exact one: what this names is
/// what CANNOT — the separators, the parentheses whose nesting the parser's own
/// recursion limit already counts, and the literals a chain is built OUT of.
/// EVERY other token counts, so an operator a later release of the parser or of
/// this dialect accepts is bounded by default rather than missed by a list
/// nobody updated.
fn deepens_the_tree(token: &Token) -> bool {
    !matches!(
        token,
        Token::Whitespace(_)
            | Token::Comma
            | Token::SemiColon
            | Token::LParen
            | Token::RParen
            | Token::Number(..)
            | Token::Char(_)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::NationalStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::Placeholder(_)
            | Token::EOF
    )
}

/// `java.sql.Types`' constants — the codes `ColumnMetadata.type` carries.
mod jdbc {
    pub const BIT: i32 = -7;
    pub const BIGINT: i32 = -5;
    pub const BINARY: i32 = -2;
    pub const CHAR: i32 = 1;
    pub const NUMERIC: i32 = 2;
    pub const INTEGER: i32 = 4;
    pub const SMALLINT: i32 = 5;
    pub const REAL: i32 = 7;
    pub const DOUBLE: i32 = 8;
    pub const VARCHAR: i32 = 12;
    pub const DATE: i32 = 91;
    pub const TIME: i32 = 92;
    pub const TIMESTAMP: i32 = 93;
    pub const ARRAY: i32 = 2003;
    /// A type with no `java.sql.Types` constant of its own.
    pub const OTHER: i32 = 1111;
}

/// What the Data API tells a caller whose `sql` carries more than one statement: a call
/// runs one statement, and this is the sentence that says so.
pub(super) const MULTISTATEMENT_REFUSAL: &str = "Multistatements aren't supported.";

/// What the modelled `UnsupportedResultException` says about a result whose type
/// it cannot carry. The model's own wording for the condition: "It contained a
/// multidimensional array."
pub(super) const MULTIDIMENSIONAL_ARRAY: &str = "The result contains a multidimensional array.";

/// What the Data API says about a floating value that is not a number at all: a `Field`
/// has no member that carries one — `doubleValue` is a number and `isNull` would say the
/// value is missing — so a result set carrying one in `records` is refused, and the
/// modelled `UnsupportedResultException` says so in AWS's own words.
pub(super) const NON_FINITE_FLOAT: &str = "NaN and Infinity values are not supported";

/// What the database DISPLAYS a floating value that is not a number as: the three spellings
/// PostgreSQL's "Data Types" chapter documents for the special values of its floating
/// types, which is the text such a value is written and read back as.
const NOT_A_NUMBER: &str = "NaN";
const POSITIVE_INFINITY: &str = "Infinity";
const NEGATIVE_INFINITY: &str = "-Infinity";

/// The non-finite value a float type's INPUT names, as the database displays it, and `None`
/// for text that names a number or nothing at all.
///
/// PostgreSQL's floating types accept the special values `NaN`, `Infinity` and `-Infinity`
/// — and the abbreviations `inf` and `-inf` — "in a case-insensitive manner", which is the
/// input this reads. The engine behind this data plane has no form of its own for such a
/// value, so a column holds it as this text; reading it back is the same question, which is
/// why one function answers both.
pub(super) fn non_finite_float(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    let negative = trimmed.starts_with('-');
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if unsigned.eq_ignore_ascii_case(NOT_A_NUMBER) {
        return Some(NOT_A_NUMBER);
    }
    if unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case(POSITIVE_INFINITY) {
        return Some(if negative {
            NEGATIVE_INFINITY
        } else {
            POSITIVE_INFINITY
        });
    }
    None
}

/// The same for a float the ENGINE holds as one: the text the database displays a
/// non-finite double as, and `None` for a number.
pub(super) fn non_finite_display(number: f64) -> Option<&'static str> {
    if number.is_nan() {
        Some(NOT_A_NUMBER)
    } else if number == f64::INFINITY {
        Some(POSITIVE_INFINITY)
    } else if number == f64::NEG_INFINITY {
        Some(NEGATIVE_INFINITY)
    } else {
        None
    }
}

/// How the Data API renders an error the DATABASE raised: the engine's own message,
/// introduced the way PostgreSQL introduces an error, and closed with the SQLSTATE of the
/// condition — which is what a client classifies the failure by.
fn postgresql_error(message: &str, sqlstate: &str) -> String {
    format!("ERROR: {message}; SQLState: {sqlstate}")
}

/// How PostgreSQL states that a statement reached a transaction a previous statement of
/// it already failed in: "current transaction is aborted, commands ignored until end of
/// transaction block", under the SQLSTATE of that condition (class 25, in-failed-SQL-
/// transaction).
///
/// An error the DATABASE raises for the statement, which is why it is reported as the
/// database's error and not as the transaction being gone: the transaction is still there,
/// and a commit or a rollback still ends it.
pub(super) fn transaction_aborted() -> String {
    postgresql_error(
        "current transaction is aborted, commands ignored until end of transaction block",
        SQLSTATE_IN_FAILED_TRANSACTION,
    )
}

/// How PostgreSQL states that a statement could not have a row of `table` that another
/// transaction holds: its own sentence for the condition, under the SQLSTATE of
/// `lock_not_available`.
pub(super) fn could_not_obtain_lock(table: &str) -> String {
    postgresql_error(
        &format!("could not obtain lock on row in relation \"{table}\""),
        SQLSTATE_LOCK_NOT_AVAILABLE,
    )
}

/// And how it states the one transaction of a waiting CYCLE the server breaks:
/// "deadlock detected", under the SQLSTATE of `deadlock_detected`.
pub(super) fn deadlock_detected() -> String {
    postgresql_error("deadlock detected", SQLSTATE_DEADLOCK_DETECTED)
}

/// And how it states a statement whose WAIT for such a row ran out: PostgreSQL's own
/// sentence for the cancellation, under the same `lock_not_available` SQLSTATE.
pub(super) fn lock_timeout() -> String {
    postgresql_error(
        "canceling statement due to lock timeout",
        SQLSTATE_LOCK_NOT_AVAILABLE,
    )
}

/// How PostgreSQL states a statement it cannot parse: "syntax error at or near" the token
/// it stopped on, the POSITION of that token in the statement, and the SQLSTATE of a
/// syntax error.
///
/// The token and the place come from the PARSER's own answer — which names the token it
/// found and the line and column it found it at — so nothing here reads the caller's SQL
/// to decide what is wrong with it; the statement is read only to count the characters
/// before that line and column, which is the POSITION PostgreSQL reports.
///
/// A parser answer that names no such token keeps the parser's own text: there is nothing
/// to state PostgreSQL's sentence from.
pub(super) fn syntax_error(sql: &str, parser_message: &str) -> String {
    let Some((token, place)) = parser_message
        .split_once(PARSER_FOUND)
        .and_then(|(_, rest)| rest.split_once(PARSER_AT_LINE))
    else {
        return parser_message.to_string();
    };
    let Some((line, column)) = place.split_once(PARSER_COLUMN) else {
        return parser_message.to_string();
    };
    let (Ok(line), Ok(column)) = (
        line.trim().trim_end_matches(',').parse::<usize>(),
        column.trim().parse::<usize>(),
    ) else {
        return parser_message.to_string();
    };
    postgresql_error(
        &format!(
            "syntax error at or near \"{}\"; Position: {}",
            token.trim(),
            character_position(sql, line, column)
        ),
        SQLSTATE_SYNTAX_ERROR,
    )
}

/// How the parser's own answer introduces the token it stopped on and the place it found
/// it at.
const PARSER_FOUND: &str = " found: ";
const PARSER_AT_LINE: &str = " at Line: ";
const PARSER_COLUMN: &str = " Column: ";

/// The 1-based CHARACTER position of (`line`, `column`) in `sql`, which is what
/// PostgreSQL's `Position` reports — where the parser answers a line and a column.
fn character_position(sql: &str, line: usize, column: usize) -> usize {
    character_position_in(&line_start_offsets(sql), line, column)
}

/// The CHARACTER offset each line of a statement starts at, in ONE pass over it: the
/// position of anything in the statement is then its line's offset plus its column, so
/// reporting N positions costs one pass and not N.
fn line_start_offsets(sql: &str) -> Vec<usize> {
    let mut starts = vec![0_usize];
    for (offset, character) in sql.chars().enumerate() {
        if character == '\n' {
            // The newline that ends a line counts as one character of it.
            starts.push(offset + 1);
        }
    }
    starts
}

/// The 1-based CHARACTER position of (`line`, `column`), read off those offsets.
fn character_position_in(line_starts: &[usize], line: usize, column: usize) -> usize {
    line_starts
        .get(line.saturating_sub(1))
        .copied()
        .unwrap_or_default()
        + column.max(1)
}

/// The SQLSTATEs of the conditions this front end words, from PostgreSQL's "Error Codes"
/// appendix: a unique violation, a not-null violation, a statement in a transaction that
/// has already failed, a statement that cannot be parsed, and the two catalog lookups
/// that find nothing.
const SQLSTATE_UNIQUE_VIOLATION: &str = "23505";
const SQLSTATE_NOT_NULL_VIOLATION: &str = "23502";
const SQLSTATE_IN_FAILED_TRANSACTION: &str = "25P02";
const SQLSTATE_SYNTAX_ERROR: &str = "42601";
const SQLSTATE_UNDEFINED_TABLE: &str = "42P01";
const SQLSTATE_UNDEFINED_COLUMN: &str = "42703";
/// And the one a function nothing defines is refused under (class 42, undefined function).
const SQLSTATE_UNDEFINED_FUNCTION: &str = "42883";
/// And the two the DATABASE namespace has: a database that is already there, and a
/// name no database of the cluster answers to.
const SQLSTATE_DUPLICATE_DATABASE: &str = "42P04";
const SQLSTATE_INVALID_CATALOG_NAME: &str = "3D000";
/// And the one a value of the wrong type for its column is refused under (class 42,
/// datatype mismatch).
const SQLSTATE_DATATYPE_MISMATCH: &str = "42804";
/// And the two a row another transaction HOLDS carries: `lock_not_available` for a row a
/// statement could not have, and `deadlock_detected` for the one transaction of a waiting
/// cycle the server breaks.
const SQLSTATE_LOCK_NOT_AVAILABLE: &str = "55P03";
const SQLSTATE_DEADLOCK_DETECTED: &str = "40P01";
/// And the one text that is no value of the type it was written as is refused under
/// (class 22, invalid textual representation).
const SQLSTATE_INVALID_TEXT_REPRESENTATION: &str = "22P02";

// How the SQL engine behind this front end introduces each condition in its own
// diagnostics is read ONCE, beside the other wording of the same conditions
// (`super::condition`); nothing here is matched on the statement's text.

/// The diagnostic the DATABASE THIS API ADVERTISES words an engine error with, or `None`
/// for a condition this front end does not word — whose own text the caller sees.
///
/// An error the database raised reaches the caller in PostgreSQL's words and with
/// PostgreSQL's SQLSTATE on an Aurora PostgreSQL cluster, whatever engine actually ran the
/// statement: that text and that code are what a client branches on, so an
/// engine-specific sentence in their place is a sentence no client can classify. A cluster
/// running the API's OTHER engine is answered in that engine's words instead
/// ([`super::mysql::mysql_diagnostic`]).
///
/// `primary_key_of` answers the columns a table's PRIMARY KEY covers, which is what
/// decides the NAME of the violated constraint: PostgreSQL names a table's primary-key
/// constraint `<table>_pkey` and a unique constraint over other columns
/// `<table>_<column…>_key`.
pub(super) fn postgresql_diagnostic(
    engine_message: &str,
    primary_key_of: &dyn Fn(&str) -> Vec<String>,
) -> Option<String> {
    Some(match condition_of(engine_message)? {
        Condition::UniqueViolation { table, columns } => {
            let constraint = if covers_the_same_columns(&primary_key_of(&table), &columns) {
                format!("{table}_pkey")
            } else {
                format!("{table}_{}_key", columns.join("_"))
            };
            postgresql_error(
                &format!("duplicate key value violates unique constraint \"{constraint}\""),
                SQLSTATE_UNIQUE_VIOLATION,
            )
        }
        Condition::NotNullViolation { table, column } => postgresql_error(
            &format!(
                "null value in column \"{column}\" of relation \"{table}\" violates not-null \
                 constraint"
            ),
            SQLSTATE_NOT_NULL_VIOLATION,
        ),
        Condition::UndefinedTable { table } => postgresql_error(
            &format!("relation \"{table}\" does not exist"),
            SQLSTATE_UNDEFINED_TABLE,
        ),
        Condition::UndefinedColumn {
            table: None,
            column,
        } => postgresql_error(
            &format!("column \"{column}\" does not exist"),
            SQLSTATE_UNDEFINED_COLUMN,
        ),
        Condition::UndefinedColumn {
            table: Some(table),
            column,
        } => postgresql_error(
            &format!("column \"{column}\" of relation \"{table}\" does not exist"),
            SQLSTATE_UNDEFINED_COLUMN,
        ),
        // "function <name>() does not exist": PostgreSQL names the function the statement
        // called. The name is folded to lower case, because an undelimited identifier is
        // folded there — "key words and unquoted identifiers are case insensitive … folded
        // to lower case" — and the engine's own answer carries it as the statement wrote it.
        Condition::UndefinedFunction { function } => postgresql_error(
            &format!(
                "function {}() does not exist",
                function.to_ascii_lowercase()
            ),
            SQLSTATE_UNDEFINED_FUNCTION,
        ),
    })
}

/// What a value of a type is, as far as the `Field` union is concerned: the
/// member a value comes back in, and whether a number is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeClass {
    /// `booleanValue`.
    Boolean,
    /// `longValue`, subject to `longReturnType`.
    Integer,
    /// `doubleValue`.
    Float,
    /// An exact number, whose representation `decimalReturnType` controls.
    Decimal,
    /// `stringValue`.
    Text,
    /// A date, a time or a timestamp: `stringValue`, and not a case-sensitive
    /// value for comparison.
    Temporal,
    /// `blobValue`.
    Binary,
    /// `arrayValue`: one value that is a list of values of the element type.
    Array,
    /// A type the catalog does not name, whose values are reported as the engine
    /// holds them.
    Other,
}

/// A PostgreSQL type, as the catalog holds it and as `columnMetadata` reports it.
#[derive(Debug, Clone)]
pub struct PgType {
    /// The catalog name, lower case (`int4`, `varchar`, `serial`).
    pub name: String,
    pub class: TypeClass,
    /// The `java.sql.Types` code.
    pub jdbc_type: i32,
    pub precision: i32,
    pub scale: i32,
    /// The scale the declaration itself gave (`numeric(10,2)`), when it gave one.
    /// A `numeric` with no declared scale carries each value's own scale, so its
    /// values are rendered as they are and not padded to a scale nobody declared.
    pub declared_scale: Option<i32>,
    pub signed: bool,
    pub case_sensitive: bool,
    /// The declaration carries a sequence, so the engine assigns the column's
    /// value when an INSERT leaves it out.
    pub auto_increment: bool,
    /// Whether this type holds a DOCUMENT rather than the text of one (`json`,
    /// `jsonb`): such a column stores the document a value NAMES, so what a read
    /// returns is that document written out and not the text a statement happened to
    /// spell it with.
    pub document: bool,
    /// Whether a value written as TEXT may be assigned to a column of this type
    /// without a cast.
    ///
    /// True for PostgreSQL's CHARACTER types (`text`, `varchar`, `bpchar`, `name`),
    /// which hold character values, and for a type the catalog does not name, which
    /// this front end cannot type and therefore refuses no value for. False for every
    /// type the catalog DOES name and that is not a character type: PostgreSQL has no
    /// implicit cast from a character type to another type, which is what
    /// `SqlParameter$typeHint` is for.
    pub takes_text: bool,
    /// The element type, for an array type.
    pub element: Option<Box<PgType>>,
    /// Why a result column of this type cannot be returned, when it cannot: the
    /// message the modelled `UnsupportedResultException` carries. `None` for a
    /// type whose values have a `Field` member to come back in.
    pub refusal: Option<String>,
}

impl PgType {
    /// The catalog's entry for a type, with the defaults its class implies: only
    /// a number is signed, and a value compared as text is case-sensitive.
    fn of(name: &str, class: TypeClass, jdbc_type: i32, precision: i32, scale: i32) -> Self {
        Self {
            name: name.to_string(),
            class,
            jdbc_type,
            precision,
            scale,
            declared_scale: None,
            signed: matches!(
                class,
                TypeClass::Integer | TypeClass::Float | TypeClass::Decimal
            ),
            case_sensitive: matches!(class, TypeClass::Text | TypeClass::Binary | TypeClass::Other),
            auto_increment: false,
            document: false,
            takes_text: false,
            element: None,
            refusal: None,
        }
    }

    /// The same type, as one a value written as TEXT may be assigned to (see
    /// [`PgType::takes_text`]).
    fn takes_text(mut self) -> Self {
        self.takes_text = true;
        self
    }

    /// The same type, as one that holds a DOCUMENT.
    fn document(mut self) -> Self {
        self.document = true;
        self
    }

    /// The same type, under the name a `serial` declaration gives it, with the
    /// sequence default that makes the engine assign its values.
    fn sequence(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self.auto_increment = true;
        self
    }

    /// A type whose values the Data API has no member to carry them in.
    fn unreturnable(mut self) -> Self {
        self.refusal = Some(format!(
            "The result contains the unsupported data type {}.",
            self.name.to_ascii_uppercase()
        ));
        self
    }

    /// The element type of an array type.
    pub fn element(&self) -> Option<&PgType> {
        self.element.as_deref()
    }

    /// The name the DATABASE words a diagnostic about this type with, which is the SQL
    /// name and not the catalog's own (`integer`, not `int4`; `timestamp without time
    /// zone`, not `timestamptz`'s sibling `timestamp`), as PostgreSQL's own messages
    /// spell them.
    pub fn worded_name(&self) -> &str {
        match self.name.as_str() {
            "bool" => "boolean",
            "int2" => "smallint",
            "int4" => "integer",
            "int8" => "bigint",
            "float4" => "real",
            "float8" => "double precision",
            "varchar" => "character varying",
            "bpchar" => "character",
            "time" => "time without time zone",
            "timetz" => "time with time zone",
            "timestamp" => "timestamp without time zone",
            "timestamptz" => "timestamp with time zone",
            named => named,
        }
    }
}

/// The array type whose elements are `element`.
///
/// PostgreSQL's catalog names an array type after its element (`text` ->
/// `_text`), and `java.sql.Types` has one code for every array. The digits, the
/// sign and the comparison are the ELEMENT's, because that is what an element of
/// the array carries.
///
/// An array OF arrays is the multidimensional array the model names as a result
/// the service cannot return, and an array whose element type has no member to
/// come back in cannot be returned either.
fn array_of(element: PgType) -> PgType {
    let refusal = if element.class == TypeClass::Array {
        Some(MULTIDIMENSIONAL_ARRAY.to_string())
    } else {
        element.refusal.clone()
    };
    PgType {
        name: format!("_{}", element.name),
        class: TypeClass::Array,
        jdbc_type: jdbc::ARRAY,
        precision: element.precision,
        scale: element.scale,
        declared_scale: element.declared_scale,
        signed: element.signed,
        case_sensitive: element.case_sensitive,
        auto_increment: false,
        document: false,
        takes_text: false,
        refusal,
        element: Some(Box::new(element)),
    }
}

/// What one statement is, as the data plane needs to know it.
#[derive(Debug)]
pub struct Plan {
    /// The statement to run, in the engine's own grammar.
    pub sql: String,
    /// Whether the statement changes rows, so its row count is what
    /// `numberOfRecordsUpdated` reports. Only a data-manipulation statement has
    /// one; DDL and a read report none, as PostgreSQL's own command tags do.
    pub changes_rows: bool,
    /// The PostgreSQL type of each result column the statement's own text determines.
    /// Empty when the statement's own text
    /// decides nothing (a wildcard, a statement with no result set).
    pub result_types: Vec<Option<PgType>>,
    /// The column each result column READS, in the same order, where the statement
    /// reads one directly — which is what says that an ALIAS renamed a column of a
    /// table rather than computing a new value.
    pub result_origins: Vec<Option<ColumnOrigin>>,
    /// Every column name the statement writes, with the CLAUSE it writes it in: what a
    /// diagnostic about one of those names reports, where the engine's own answer names
    /// the column alone (`crate::state::mysql`).
    pub named_columns: Vec<(String, NamedIn)>,
    /// The ONE table this statement reads or writes, when it names exactly one: the
    /// table a wildcard's columns come from.
    pub sole_table: Option<String>,
    /// The sequence-carrying columns this statement declares, which the store
    /// remembers so a later read of them reports the type they were declared
    /// with.
    pub sequence_columns: Vec<SequenceColumn>,
    /// The table whose counter this statement's own definition starts at a value of its
    /// own, and the MARK that start sets — the value below it, because a counter's mark is
    /// the last value it handed out. `None` for every statement whose definition says
    /// nothing about where a counter starts.
    pub starting_sequence: Option<(String, i64)>,
    /// Statements that complete this one, run after it: the sequence a
    /// declaration needs and the engine does not keep itself, and the further
    /// tables one statement empties.
    pub follow_on: Vec<String>,
    /// The tables this statement drops, whose remembered sequences go with them.
    pub dropped_tables: Vec<String>,
    /// The table an INSERT stored its rows in, which is where the engine may have
    /// generated a key for them (`generatedFields`). `None` for every statement that
    /// is not an insert.
    pub inserted_table: Option<String>,
    /// The table this statement ASSIGNS values to — an insert's or an update's — whose
    /// declarations say what type each of those values has to be.
    pub assigned_table: Option<String>,
    /// The column each NAMED placeholder of the statement assigns its value to, where
    /// the statement assigns one directly.
    pub assigned_values: Vec<AssignedValue>,
    /// The value the statement assigns to each column it names, where it assigns exactly
    /// one: what the duplicate-key diagnostic of the engine the cluster runs names as the
    /// colliding value (`crate::state::mysql`).
    pub assigned_cells: Vec<(String, AssignedCell)>,
    /// The type a CAST the statement writes names for the value a named placeholder
    /// carries: `(placeholder, type)`, which is the type that value has to be a value
    /// OF.
    pub cast_values: Vec<(String, PgType)>,
}

/// A column whose PostgreSQL declaration carries a sequence.
///
/// The column is declared to the engine as an integer, so that is the type the
/// engine reports for it; this is what remembers the name the declaration used
/// (`serial`, `bigserial`, `smallserial`), which is the name `columnMetadata`
/// reports and the one its precision and type code come from.
#[derive(Debug, Clone)]
pub struct SequenceColumn {
    /// The table, lower-cased: an unquoted SQL identifier is case-insensitive.
    pub table: String,
    /// The column, lower-cased.
    pub column: String,
    /// The catalog name of the declared type.
    pub type_name: String,
    /// Whether the value the engine assigns this column is one the API reports in
    /// `generatedFields`: MySQL's `AUTO_INCREMENT` key is, and PostgreSQL's `serial`
    /// is not, because `generatedFields`'s own documentation says the data "isn't
    /// supported by Aurora PostgreSQL. To get the values of generated fields, use the
    /// RETURNING clause".
    pub reports_generated_key: bool,
}

/// One value a statement assigns DIRECTLY to a column of the table it writes.
#[derive(Debug, Clone)]
pub struct AssignedValue {
    /// The placeholder the value comes from, WITHOUT the prefix that introduces it in
    /// the statement — which is the name a parameter carries.
    pub placeholder: String,
    /// The column it is assigned to, as the statement wrote it.
    pub column: String,
    /// The 1-based CHARACTER position of the placeholder in the statement, which is
    /// where the database reports an error about that value (`Position`).
    pub position: Option<usize>,
}

/// One value a statement assigns to a column, as the STATEMENT wrote it.
///
/// What this is for: an engine's duplicate-key diagnostic names the VALUE that collided,
/// and the only places that value exists are the statement's own text and the parameters
/// the request carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignedCell {
    /// The name of the parameter that carries it, WITHOUT the prefix that introduces it in
    /// the statement.
    Parameter(String),
    /// The literal the statement wrote, as the value it names.
    Literal(String),
}

impl Plan {
    /// The type the statement's text gives result column `index`, when it gives
    /// one. The list is used only when it covers the result set exactly: a
    /// wildcard expands to columns the text does not list, so a mismatched count
    /// means nothing can be read off a position.
    pub fn result_type(&self, index: usize, columns: usize) -> Option<&PgType> {
        if self.result_types.len() != columns {
            return None;
        }
        self.result_types.get(index).and_then(Option::as_ref)
    }

    /// The column result column `index` READS, when the statement reads one directly.
    /// Bounded by the same count check [`Plan::result_type`] applies, and for the same
    /// reason.
    pub fn result_origin(&self, index: usize, columns: usize) -> Option<&ColumnOrigin> {
        if self.result_origins.len() != columns {
            return None;
        }
        self.result_origins.get(index).and_then(Option::as_ref)
    }

    /// The CLAUSE this statement writes `column` in, and `None` for a name it does not
    /// write — one the engine resolved through a wildcard, or a name of its own.
    ///
    /// The FIRST clause that writes it, in the order a statement's clauses are resolved,
    /// because that is the one a diagnostic about the name reports.
    pub fn named_in(&self, column: &str) -> Option<NamedIn> {
        self.named_columns
            .iter()
            .find(|(named, _)| named.eq_ignore_ascii_case(column))
            .map(|(_, clause)| *clause)
    }
}

/// Where in a statement a NAME is written, which is what a diagnostic about that name
/// reports: the engine's own answer names the column alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedIn {
    /// The values the statement selects, or the columns it assigns.
    Projection,
    /// The condition it chooses rows by.
    Where,
    /// The expressions it groups those rows by.
    GroupBy,
    /// The condition over the groups.
    Having,
    /// The order it returns rows in.
    OrderBy,
    /// The condition a join matches rows on.
    JoinOn,
}

/// Every column name the statement writes, with the clause it writes it in — in the order
/// a statement's clauses are RESOLVED, so the first entry for a name is the clause a
/// diagnostic about it reports.
///
/// Read from the parse, like every other decision here. A statement that is not a query
/// names its columns where it selects or assigns them, which is the clause a diagnostic
/// about one of them reports.
fn named_columns(statement: &Statement) -> Vec<(String, NamedIn)> {
    let mut named = Vec::new();
    let Statement::Query(query) = statement else {
        collect_names(statement, NamedIn::Projection, &mut named);
        return named;
    };
    if let Some(SetExpr::Select(select)) = leftmost_select(&query.body) {
        for item in &select.projection {
            match item {
                SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                    collect_names(expr, NamedIn::Projection, &mut named);
                }
                _ => {}
            }
        }
        if let Some(selection) = &select.selection {
            collect_names(selection, NamedIn::Where, &mut named);
        }
        if let GroupByExpr::Expressions(expressions, _) = &select.group_by {
            for expression in expressions {
                collect_names(expression, NamedIn::GroupBy, &mut named);
            }
        }
        if let Some(having) = &select.having {
            collect_names(having, NamedIn::Having, &mut named);
        }
        // Everything the FROM clause writes, which is where a join's own condition is.
        for item in &select.from {
            collect_names(item, NamedIn::JoinOn, &mut named);
        }
    }
    if let Some(OrderByKind::Expressions(expressions)) =
        query.order_by.as_ref().map(|order_by| &order_by.kind)
    {
        for expression in expressions {
            collect_names(&expression.expr, NamedIn::OrderBy, &mut named);
        }
    }
    named
}

/// Every column name one part of a statement writes, as the NAME a diagnostic reports: the
/// last part of a qualified reference, which is the column's own name.
fn collect_names<V: sqlparser::ast::Visit>(
    part: &V,
    clause: NamedIn,
    named: &mut Vec<(String, NamedIn)>,
) {
    let _ = visit_expressions(part, |expr| {
        match expr {
            Expr::Identifier(name) => named.push((name.value.clone(), clause)),
            Expr::CompoundIdentifier(parts) => {
                if let Some(column) = parts.last() {
                    named.push((column.value.clone(), clause));
                }
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The column of the table a result column READS: which column of which table a value
/// comes from, which an ALIAS renames without detaching it from.
#[derive(Debug, Clone)]
pub struct ColumnOrigin {
    /// The table that stores it, when the statement names exactly one it could be.
    pub table: Option<String>,
    /// The name that table gives the column, which is not the result column's label
    /// when the statement renamed it.
    pub column: String,
}

/// Parse one PostgreSQL statement and plan it.
///
/// A call runs ONE statement — the model's `sql` member is "The SQL statement" —
/// so a request carrying several is refused rather than having all but the first
/// silently dropped.
///
/// The statement is CHECKED BEFORE IT IS PARSED, by [`chained_levels`], for how
/// many levels it could ask the statement tree to carry: the parse itself is
/// bounded by its recursion limit, but an operator chain is parsed ITERATIVELY,
/// so such a chain's depth grows with the request and no parser bound sees it —
/// while the tree it builds is walked and dropped by recursion. The check runs on
/// the tokens, which is a pass over the text that allocates nothing per level, so
/// a statement past the bound is refused before a tree of it exists at all — which is
/// why it lives in [`read_as`], between the tokenize and the parse, where EVERY path
/// that parses caller SQL inherits it.
pub fn plan(sql: &str, catalog: &Catalog<'_>) -> Result<Plan, SqlError> {
    let (tokens, mut statements) = read(sql)?;
    if statements.len() != 1 {
        // "Multistatements aren't supported." — a call runs ONE statement (`sql` is "The
        // SQL statement"), and this is the sentence the service refuses a string carrying
        // several with, whatever it carries.
        return Err(SqlError::InvalidRequest(
            MULTISTATEMENT_REFUSAL.to_string(),
        ));
    }
    let mut statement = statements.remove(0);
    // The SCHEMA every name in the statement is qualified by is resolved FIRST, against the
    // one schema of stored objects this data plane has: a `public.` qualification selects
    // what the bare name does, and a relation of a catalog schema becomes the query over
    // the engine's own schema that answers it, so everything below reads the statement the
    // engine will run (`catalog`).
    let catalog_relations =
        catalog::resolve_schemas(&mut statement, catalog.database, catalog.user);
    // The types the statement's own text gives its result columns are read from
    // the parse BEFORE it is rewritten, because the rewrite replaces a cast's
    // PostgreSQL target type with the engine type that holds its values.
    let mut result_types = result_types(&statement, catalog);
    // And a result column read out of a catalog relation has the type that CATALOG declares
    // it with, which is the one thing about such a column the engine cannot report: the
    // relation it is read from is a derived table of this statement.
    catalog::catalog_column_types(&statement, &catalog_relations, &mut result_types);
    let result_origins = result_origins(&statement);
    // And the clause the statement writes each of its column names in, which is what a
    // diagnostic about one of those names reports.
    let named_columns = named_columns(&statement);
    let sole_table = sole_table(&statement);
    // A value the statement WRITES OUT as text has to be a value of the type it is
    // written as, which the database decides before it computes anything with it.
    require_valid_literals(&statement)?;
    // And the columns its placeholders assign to, for the same reason: a rewrite may
    // wrap a placeholder in an expression, and what a value is assigned to is what the
    // statement itself wrote.
    let (assigned_table, assigned_values) = assigned_values(&statement, sql, &tokens);
    // And the VALUE it assigns to each of them, for the same reason: the diagnostic a
    // duplicate key earns names the value that collided, which only the statement and the
    // request's parameters hold.
    let assigned_cells = assigned_cells(&statement);
    let cast_values = cast_values(&statement);
    // A value of an ARRAY type written out AS TEXT is written in the array output format
    // the database documents, which only the statement's text and the schema can say is
    // what is happening — so it is rewritten here, while the catalog is at hand and
    // before the rewrite below replaces the cast.
    plan_array_output(&mut statement, catalog);
    // The SESSION VALUES the statement names, substituted for what this session carries: the
    // value of `current_user` comes from the call that was admitted, never from the statement.
    plan_session_values(&mut statement, catalog);
    normalise(&mut statement);
    // An upsert the caller wrote in MySQL's spelling is the same upsert the engine
    // spells with the conflicting key named, which only the table's own declaration
    // knows; done before the names are delimited, so the columns this builds are
    // rendered the way the caller's own are.
    plan_upsert(&mut statement, catalog.key_columns);
    // The names the statement uses are rendered as delimited identifiers, so a
    // word PostgreSQL admits as a name is one the engine's grammar reads as that
    // name too. Done AFTER the rewrites above, so the expressions they build are
    // rendered the same way as the ones the caller wrote.
    names::quote_identifiers(&mut statement);
    let (sequence_columns, mut follow_on) = plan_sequences(&mut statement);
    let starting_sequence = plan_table_options(&mut statement);
    let (sql, emptied) = rendered(&statement);
    follow_on.extend(emptied);
    Ok(Plan {
        changes_rows: changes_rows(&statement),
        dropped_tables: dropped_tables(&statement),
        inserted_table: inserted_table(&statement),
        assigned_table,
        assigned_values,
        assigned_cells,
        cast_values,
        sql,
        result_types,
        result_origins,
        named_columns,
        sole_table,
        sequence_columns,
        starting_sequence,
        follow_on,
    })
}

/// Take the TABLE OPTIONS off a definition, and answer the mark its counter starts from
/// where one of them said so.
///
/// A table option names a property of how the engine STORES the table — which storage engine
/// holds it, the character set and collation its text columns default to, its row format, how
/// its pages are packed, its comment — and not what the table holds: every column, constraint
/// and default the definition declares is unchanged by them, and the engine behind this data
/// plane has one storage engine and one encoding, so it declares none of them. A definition
/// carrying them therefore defines exactly the table the same definition without them
/// defines, which is why they are taken off here rather than handed to an engine whose
/// grammar has no clause for them.
///
/// `AUTO_INCREMENT = <value>` is the one option in that list a client can OBSERVE — "the
/// initial `AUTO_INCREMENT` value for the table", so the first key the table's counter hands
/// out is that value — and it is answered rather than dropped: the engine keeps the counter as
/// the mark of the last value handed out, so the mark a definition starting at `<value>` sets
/// is the one below it ([`crate::state::Database::advance_sequences`], which carries it in
/// once the table exists). A value of one or less is the counter's own start, so it sets
/// nothing.
fn plan_table_options(statement: &mut Statement) -> Option<(String, i64)> {
    let Statement::CreateTable(create) = statement else {
        return None;
    };
    // The table as the ENGINE will hold it, which is the name the counter is kept under:
    // every name this front end renders is delimited and nothing folds its case
    // ([`names::quote_identifiers`]), so a definition that wrote `Orders` creates `Orders`
    // and a mark keyed by anything else would reach no counter at all.
    let table = create
        .name
        .0
        .last()
        .map(|part| part.to_string())
        .unwrap_or_default()
        .trim_matches('"')
        .to_string();
    let starting_at = starting_auto_increment(&create.table_options);
    create.table_options = CreateTableOptions::None;
    starting_at
        .filter(|value| *value > 1)
        .map(|value| (table, value - 1))
}

/// The value a definition's `AUTO_INCREMENT = <value>` table option names, read off the parse.
fn starting_auto_increment(options: &CreateTableOptions) -> Option<i64> {
    let named = match options {
        CreateTableOptions::With(options)
        | CreateTableOptions::Options(options)
        | CreateTableOptions::Plain(options) => options,
        _ => return None,
    };
    named.iter().find_map(|option| match option {
        SqlOption::KeyValue { key, value }
            if key.value.eq_ignore_ascii_case(AUTO_INCREMENT_OPTION) =>
        {
            value.to_string().trim().parse().ok()
        }
        _ => None,
    })
}

/// MySQL's own name for the table option that says which value a table's counter starts at.
const AUTO_INCREMENT_OPTION: &str = "AUTO_INCREMENT";

/// The column each result column of a statement READS, where the statement reads one
/// directly: a column reference, bare or qualified, whether or not an alias renames it.
///
/// What this is for: a result column that READS a stored column is that column — its
/// table declares it, and the declaration decides what `columnMetadata` reports — and
/// an alias changes only the LABEL the result set carries. A computed column reads no
/// column, which is what `None` says.
fn result_origins(statement: &Statement) -> Vec<Option<ColumnOrigin>> {
    let Statement::Query(query) = statement else {
        return Vec::new();
    };
    let Some(SetExpr::Select(select)) = leftmost_select(&query.body) else {
        return Vec::new();
    };
    // Where a name without a qualifier could come from: the one table the statement
    // reads, when it reads exactly one.
    let read_from = sole_read_table(select);
    select
        .projection
        .iter()
        .map(|item| {
            let expr = match item {
                SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => expr,
                _ => return None,
            };
            match expr {
                Expr::Identifier(name) => Some(ColumnOrigin {
                    table: read_from.clone(),
                    column: name.value.clone(),
                }),
                Expr::CompoundIdentifier(parts) => {
                    let [.., qualifier, column] = parts.as_slice() else {
                        return None;
                    };
                    Some(ColumnOrigin {
                        table: table_of_qualifier(select, &qualifier.value),
                        column: column.value.clone(),
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// The ONE table a statement reads or writes, when it names exactly one.
fn sole_table(statement: &Statement) -> Option<String> {
    match statement {
        Statement::Query(query) => match leftmost_select(&query.body) {
            Some(SetExpr::Select(select)) => sole_read_table(select),
            _ => None,
        },
        Statement::Insert(insert) => match &insert.table {
            TableObject::TableName(name) => bare_name(name),
            _ => None,
        },
        Statement::Update { table, .. } => match &table.relation {
            TableFactor::Table { name, .. } => bare_name(name),
            _ => None,
        },
        _ => None,
    }
}

/// The one table a SELECT's `FROM` names, when it names exactly one and joins nothing
/// to it — so a name the projection does not qualify can only be that table's.
fn sole_read_table(select: &sqlparser::ast::Select) -> Option<String> {
    let [only] = select.from.as_slice() else {
        return None;
    };
    if !only.joins.is_empty() {
        return None;
    }
    match &only.relation {
        TableFactor::Table { name, .. } => bare_name(name),
        _ => None,
    }
}

/// The table a qualifier in a projection names: a `FROM` item's alias, or the table's
/// own name.
fn table_of_qualifier(select: &sqlparser::ast::Select, qualifier: &str) -> Option<String> {
    for item in &select.from {
        for factor in std::iter::once(&item.relation).chain(item.joins.iter().map(|j| &j.relation))
        {
            let TableFactor::Table { name, alias, .. } = factor else {
                continue;
            };
            let named = bare_name(name)?;
            let known_as = match alias {
                Some(alias) => alias.name.value.clone(),
                None => named.clone(),
            };
            if known_as.eq_ignore_ascii_case(qualifier) {
                return Some(named);
            }
        }
    }
    None
}

/// The leftmost branch of a set operation, which is the query PostgreSQL names the
/// result after. Walked ITERATIVELY, for the reason [`leftmost_projection`] gives.
fn leftmost_select(body: &SetExpr) -> Option<&SetExpr> {
    let mut body = body;
    loop {
        match body {
            SetExpr::Select(_) => return Some(body),
            SetExpr::Query(query) => body = &query.body,
            SetExpr::SetOperation { left, .. } => body = left,
            _ => return None,
        }
    }
}

/// The table one statement assigns values to, and the column each of its NAMED
/// placeholders assigns a value to, where the statement assigns one DIRECTLY: an
/// insert's own column list against the values of each of its rows, and the target of
/// each of an update's assignments.
///
/// What this is for: the type of the value a placeholder carries has to be a type the
/// column takes, and only the statement says WHICH column each placeholder is for. A
/// placeholder inside an expression (a cast, an arithmetic) is not assigned directly —
/// the expression's own type is what the column sees — so it is not reported here.
///
/// Where each placeholder is WRITTEN comes from the tokens
/// ([`placeholder_positions`]), because that is the `Position` the database reports an
/// error about one of these values at.
fn assigned_values(
    statement: &Statement,
    sql: &str,
    tokens: &[TokenWithSpan],
) -> (Option<String>, Vec<AssignedValue>) {
    let written_at = placeholder_positions(sql, tokens);
    let at = |placeholder: String, column: String| AssignedValue {
        position: written_at.get(&placeholder).copied(),
        placeholder,
        column,
    };
    match statement {
        Statement::Insert(insert) => {
            let TableObject::TableName(name) = &insert.table else {
                return (None, Vec::new());
            };
            let Some(SetExpr::Values(values)) = insert
                .source
                .as_deref()
                .map(|source| source.body.as_ref())
            else {
                return (bare_name(name), Vec::new());
            };
            let assigned = values
                .rows
                .iter()
                .flat_map(|row| {
                    row.iter().enumerate().filter_map(|(position, value)| {
                        Some(at(
                            placeholder_name(value)?,
                            insert.columns.get(position)?.value.clone(),
                        ))
                    })
                })
                .collect();
            (bare_name(name), assigned)
        }
        Statement::Update {
            table,
            assignments,
            ..
        } => {
            let TableFactor::Table { name, .. } = &table.relation else {
                return (None, Vec::new());
            };
            let assigned = assignments
                .iter()
                .filter_map(|assignment| {
                    let AssignmentTarget::ColumnName(column) = &assignment.target else {
                        return None;
                    };
                    Some(at(
                        placeholder_name(&assignment.value)?,
                        bare_name(column)?,
                    ))
                })
                .collect();
            (bare_name(name), assigned)
        }
        _ => (None, Vec::new()),
    }
}

/// The value one statement assigns to each column it names, where it assigns EXACTLY ONE:
/// an insert of one row of values, and each of an update's assignments.
///
/// What this is for: the duplicate-key diagnostic of either engine names the VALUE that
/// collided, and only the statement and the request's parameters hold it.
///
/// A statement that assigns a column MORE than one value — an insert of several rows, one
/// whose rows come from a query — settles no single value for it, so it reports none and
/// the diagnostic says so in the engine's own other words for a duplicate key.
fn assigned_cells(statement: &Statement) -> Vec<(String, AssignedCell)> {
    match statement {
        Statement::Insert(insert) => {
            let Some(SetExpr::Values(values)) = insert
                .source
                .as_deref()
                .map(|source| source.body.as_ref())
            else {
                return Vec::new();
            };
            let [row] = values.rows.as_slice() else {
                return Vec::new();
            };
            insert
                .columns
                .iter()
                .zip(row)
                .filter_map(|(column, value)| Some((column.value.clone(), assigned_cell(value)?)))
                .collect()
        }
        Statement::Update { assignments, .. } => assignments
            .iter()
            .filter_map(|assignment| {
                let AssignmentTarget::ColumnName(column) = &assignment.target else {
                    return None;
                };
                Some((bare_name(column)?, assigned_cell(&assignment.value)?))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// One assigned value as the statement wrote it — a parameter's name, or the literal
/// itself — and `None` for an expression that is neither, whose value only running it
/// would answer.
fn assigned_cell(value: &Expr) -> Option<AssignedCell> {
    if let Some(placeholder) = placeholder_name(value) {
        return Some(AssignedCell::Parameter(placeholder));
    }
    let Expr::Value(literal) = value else {
        return None;
    };
    match &literal.value {
        Value::Number(digits, _) => Some(AssignedCell::Literal(digits.clone())),
        Value::SingleQuotedString(text) => Some(AssignedCell::Literal(text.clone())),
        _ => None,
    }
}

/// Where each NAMED placeholder of the statement is written: its name, without the
/// prefix that introduces it, and the 1-based CHARACTER position of the token.
///
/// Read from the TOKENS, which is where the position of anything in a statement comes
/// from here ([`syntax_error`] reads the parser's own answer the same way); nothing
/// searches the statement's text for it. A named placeholder reaches the tokens in
/// either of two shapes — as one token, and as the PREFIX that introduces it followed
/// by the name — so both are read, and the position is the prefix's, which is where the
/// value starts.
fn placeholder_positions(sql: &str, tokens: &[TokenWithSpan]) -> HashMap<String, usize> {
    // One pass over the statement for the LINE offsets, then one pass over the tokens:
    // the work is linear in the statement, however many placeholders it carries.
    let line_starts = line_start_offsets(sql);
    let mut written_at = HashMap::new();
    for (index, token) in tokens.iter().enumerate() {
        let name = match &token.token {
            Token::Placeholder(placeholder) => {
                placeholder.trim_start_matches([':', '@', '$']).to_string()
            }
            Token::Colon | Token::AtSign => match tokens.get(index + 1).map(|next| &next.token) {
                Some(Token::Word(word)) => word.value.clone(),
                _ => continue,
            },
            _ => continue,
        };
        if name.is_empty() {
            continue;
        }
        // A name the statement writes more than once is written FIRST where the
        // database reports an error about its value.
        written_at.entry(name).or_insert_with(|| {
            character_position_in(
                &line_starts,
                token.span.start.line as usize,
                token.span.start.column as usize,
            )
        });
    }
    written_at
}

/// The name a placeholder carries, WITHOUT the prefix that introduces it in the
/// statement, and `None` for every other expression — including the anonymous
/// placeholder, which carries no name to match a parameter by.
fn placeholder_name(value: &Expr) -> Option<String> {
    let Expr::Value(literal) = value else {
        return None;
    };
    let Value::Placeholder(placeholder) = &literal.value else {
        return None;
    };
    let name = placeholder.trim_start_matches([':', '@', '$']);
    (!name.is_empty() && name != placeholder).then(|| name.to_string())
}

/// The last part of a possibly-qualified name, as the statement wrote it.
fn bare_name(name: &ObjectName) -> Option<String> {
    Some(name.0.last()?.to_string().trim_matches('"').to_string())
}

/// How PostgreSQL states that a value of one type cannot be assigned to a column of
/// another, under the SQLSTATE of a datatype mismatch: the column, the type it was
/// declared with, the type the value arrived as, the HINT PostgreSQL closes that
/// sentence with, and where in the statement the value was written.
pub(super) fn wrong_type_for_column(
    column: &str,
    declared: &PgType,
    arrived: &str,
    position: Option<usize>,
) -> String {
    let written_at = match position {
        Some(position) => format!("; Position: {position}"),
        None => String::new(),
    };
    postgresql_error(
        &format!(
            "column \"{column}\" is of type {} but expression is of type {arrived}; \
             Hint: {REWRITE_OR_CAST_HINT}{written_at}",
            declared.worded_name()
        ),
        SQLSTATE_DATATYPE_MISMATCH,
    )
}

/// What PostgreSQL tells a caller whose value is of the wrong type for its column.
const REWRITE_OR_CAST_HINT: &str = "You will need to rewrite or cast the expression.";

/// Whether a value SENT as one type may be assigned to a column of another with no cast
/// written anywhere.
///
/// PostgreSQL's implicit and assignment casts run WITHIN a family of related types and
/// between no two of them: a number reaches any of the number types, a character value any
/// of the character types, a document either spelling of a document, an instant a date or a
/// timestamp — and a type with no family of its own takes only its own values. Everything
/// else is the datatype mismatch the database refuses the statement with
/// ([`wrong_type_for_column`]), which is what a client is told to rewrite or cast.
pub(super) fn assignable_to(sent: &PgType, column: &PgType) -> bool {
    match (assignment_of(sent), assignment_of(column)) {
        // A type this front end cannot type settles nothing, so it refuses no value.
        (Assignment::Untyped, _) | (_, Assignment::Untyped) => true,
        (Assignment::Family(sent), Assignment::Family(column)) => sent == column,
        (Assignment::OnlyItself(sent), Assignment::OnlyItself(column)) => sent == column,
        _ => false,
    }
}

/// What a type takes, for an assignment with no cast in it.
enum Assignment<'a> {
    /// A type the catalog does not name, which decides nothing about a value.
    Untyped,
    /// One of the families an assignment runs inside.
    Family(AssignmentFamily),
    /// A type the catalog names that has no family: it takes values of its own type and
    /// nothing else, under the catalog name that is its own.
    OnlyItself(&'a str),
}

/// The families PostgreSQL's implicit and assignment casts run inside.
#[derive(PartialEq, Eq)]
enum AssignmentFamily {
    /// Every number type: the integers, the floating types and the exact numbers.
    Number,
    /// The character types.
    Character,
    Boolean,
    /// A date and a timestamp, with or without a zone: a point on the calendar.
    Instant,
    /// A time of day, with or without a zone.
    TimeOfDay,
    /// Either spelling of a document (`json`, `jsonb`).
    Document,
    Binary,
    /// An array of any element type: the element decides what the array holds, and an
    /// array is assignable to no type that is not one.
    Array,
}

/// Which of those a type belongs to, read off the catalog's own entry for it.
fn assignment_of(pg: &PgType) -> Assignment<'_> {
    match pg.class {
        TypeClass::Integer | TypeClass::Float | TypeClass::Decimal => {
            Assignment::Family(AssignmentFamily::Number)
        }
        TypeClass::Boolean => Assignment::Family(AssignmentFamily::Boolean),
        TypeClass::Binary => Assignment::Family(AssignmentFamily::Binary),
        TypeClass::Array => Assignment::Family(AssignmentFamily::Array),
        TypeClass::Temporal => Assignment::Family(match pg.name.as_str() {
            "time" | "timetz" => AssignmentFamily::TimeOfDay,
            _ => AssignmentFamily::Instant,
        }),
        TypeClass::Text if pg.document => Assignment::Family(AssignmentFamily::Document),
        // A type that takes a value written as TEXT is a character type, and the catalog
        // marks the types the front end cannot type the same way ([`PgType::takes_text`]):
        // the character types are the ones it NAMES.
        TypeClass::Text if pg.takes_text => Assignment::Family(AssignmentFamily::Character),
        _ if pg.takes_text => Assignment::Untyped,
        _ => Assignment::OnlyItself(&pg.name),
    }
}

/// What the DATABASE itself holds about the statement's own names, which its text
/// does not carry and only a schema settles: the key an upsert conflicts on, and the
/// type a column was declared with.
///
/// Supplied by the caller, because a database is what holds a schema and this module
/// holds none.
pub struct Catalog<'a> {
    /// The DATABASE the statement runs against, which is the catalog name the relations
    /// describing it report ([`catalog`]).
    pub database: &'a str,
    /// The ENGINE the addressed cluster RUNS, which is what decides the dialect a session
    /// value is written in and which family defines it at all.
    pub engine: super::EngineFamily,
    /// The database USER the call authenticated as, which is the identity the engine reports
    /// for the session ([`plan_session_values`]). Empty where no call has resolved one.
    pub user: &'a str,
    /// The columns of the key a table's own declaration says a duplicate-key
    /// condition is raised against (see [`plan_upsert`]).
    pub key_columns: &'a dyn Fn(&str) -> Vec<String>,
    /// The type the schema declares for a column of this name, where it settles one —
    /// which is what gives an aggregate over that column its own type
    /// ([`function_type`]).
    pub column_type: &'a dyn Fn(&str) -> Option<String>,
}

/// Tokenize and parse one statement as the dialect it is WRITTEN IN.
///
/// The Data API serves Aurora PostgreSQL and Aurora MySQL DB clusters, and its `sql`
/// member is "The SQL statement" of whichever engine the caller addresses, so a
/// statement is read as PostgreSQL first — the engine this front end describes its
/// results as — and as MySQL second, which is what lets a statement only MySQL spells
/// (an `AUTO_INCREMENT` column definition, a backtick-delimited name) run as that
/// statement instead of being refused for a syntax error neither engine has.
///
/// A statement NEITHER dialect reads is reported with PostgreSQL's own diagnostic for
/// the token it stopped on, because that is the engine whose words every other error
/// here is in. A statement past the LEVEL BOUND is refused as that, without the second
/// dialect being tried: the bound is about how much tree one call may ask for, which is
/// the same question whichever grammar reads it.
fn read(sql: &str) -> Result<(Vec<TokenWithSpan>, Vec<Statement>), SqlError> {
    let refusal = match read_as(&PostgreSqlDialect {}, sql) {
        Ok(read) => return Ok(read),
        Err(Refused::Bound(bound)) => return Err(bound),
        Err(Refused::Unreadable(message)) => message,
    };
    match read_as(&MySqlDialect {}, sql) {
        Ok(read) => Ok(read),
        Err(Refused::Bound(bound)) => Err(bound),
        Err(Refused::Unreadable(_)) => Err(SqlError::Engine(syntax_error(sql, &refusal))),
    }
}

/// Why one dialect did not read a statement: the statement asks for more tree than one
/// call may ([`require_bounded_levels`]), or that dialect cannot read it.
enum Refused {
    Bound(SqlError),
    Unreadable(String),
}

/// The tokens — each with WHERE it is written, which is where a diagnostic about it
/// reports — and the statements one dialect reads `sql` as.
///
/// The ORDER is the bound: tokenize, CHECK THE TOKENS, and only then parse. A chain of
/// operators or of combined queries is parsed iteratively, so no parser limit sees its
/// depth, while the tree it builds is walked and dropped by recursion — which is why no
/// path here reaches the parser before the token check has passed.
fn read_as(
    dialect: &dyn Dialect,
    sql: &str,
) -> Result<(Vec<TokenWithSpan>, Vec<Statement>), Refused> {
    let tokens = Tokenizer::new(dialect, sql)
        .tokenize_with_location()
        .map_err(|error| Refused::Unreadable(error.to_string()))?;
    require_bounded_levels(&tokens).map_err(Refused::Bound)?;
    let statements = Parser::new(dialect)
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .map_err(|error| Refused::Unreadable(error.to_string()))?;
    Ok((tokens, statements))
}

/// Refuse a statement whose TOKENS ask for more levels of statement tree than one call
/// may: more combined queries than [`MAX_SET_OPERATIONS`], or more depth-adding tokens
/// than [`MAX_CHAINED_LEVELS`].
fn require_bounded_levels(tokens: &[TokenWithSpan]) -> Result<(), SqlError> {
    let (set_operations, levels) = chained_levels(tokens);
    if set_operations > MAX_SET_OPERATIONS {
        return Err(SqlError::InvalidRequest(format!(
            "this statement combines {set_operations} queries; a statement combines at most \
             {MAX_SET_OPERATIONS}"
        )));
    }
    if levels > MAX_CHAINED_LEVELS {
        return Err(SqlError::InvalidRequest(format!(
            "this statement is larger than one call runs: {levels} of its tokens could each make \
             it one level deeper, and at most {MAX_CHAINED_LEVELS} may"
        )));
    }
    Ok(())
}

/// A statement about the DATABASES a cluster holds rather than about the rows of one
/// of them.
///
/// Each database of a cluster is its own engine here, so no engine behind one of them
/// can create or drop another: these statements are applied by the cluster that owns
/// the namespace (`crate::state::Cluster::apply_namespace`).
#[derive(Debug, Clone)]
pub enum NamespaceStatement {
    /// `CREATE DATABASE <name>`.
    Create { name: String, if_not_exists: bool },
    /// `DROP DATABASE <name>`.
    Drop { name: String, if_exists: bool },
}

/// The database-namespace statement `sql` is, and `None` for every other statement —
/// which runs against ONE database.
///
/// Read from the PARSE, like every other decision in this module, and through the same
/// bounded reader [`plan`] uses — so this path reaches the parser only for a statement
/// whose tokens are inside the level bound.
///
/// `Err` is what that reader answers for a statement NEITHER dialect reads and for one
/// past that bound. Such a statement is not one of these either, so a caller may read
/// the error as "not a namespace statement" and hand the statement to [`plan`], which
/// refuses it in exactly these words — on the path where a refusal also ends the
/// transaction the statement ran in.
pub fn namespace_statement(sql: &str) -> Result<Option<NamespaceStatement>, SqlError> {
    let (_, statements) = read(sql)?;
    let [statement] = statements.as_slice() else {
        return Ok(None);
    };
    Ok(match statement {
        Statement::CreateDatabase {
            db_name,
            if_not_exists,
            ..
        } => Some(NamespaceStatement::Create {
            name: database_name(db_name),
            if_not_exists: *if_not_exists,
        }),
        Statement::Drop {
            object_type: ObjectType::Database,
            names,
            if_exists,
            ..
        } => names.first().map(|name| NamespaceStatement::Drop {
            name: database_name(name),
            if_exists: *if_exists,
        }),
        _ => None,
    })
}

/// Whether `sql` DEFINES OR MODIFIES a stored object of the database rather than reading
/// or changing rows: the data-definition class — a table, index, view, trigger, sequence,
/// schema or database that a statement creates, alters, renames, empties or drops.
///
/// What this is for: the engines behind this API differ on whether such a statement is
/// part of the transaction it is issued in, and the MySQL family's own documentation lists
/// the data-definition statements as the ones that COMMIT the transaction implicitly
/// (`crate::state::Cluster::commits_the_transaction`). A statement about a TEMPORARY
/// object is the exception that documentation names, and is not one of these: it defines
/// an object private to its own session.
///
/// Read from the PARSE, like every other decision in this module, and through the same
/// bounded reader [`plan`] uses. A statement NEITHER dialect reads, and one past that
/// bound, defines nothing — it never reaches the engine at all, because [`plan`] refuses
/// it in the words that reader answers with.
pub fn defines_data(sql: &str) -> bool {
    let Ok((_, statements)) = read(sql) else {
        return false;
    };
    let [statement] = statements.as_slice() else {
        return false;
    };
    match statement {
        Statement::CreateTable(create) => !create.temporary,
        Statement::Drop { temporary, .. } => !temporary,
        Statement::AlterTable { .. }
        | Statement::CreateIndex(_)
        | Statement::CreateView { .. }
        | Statement::CreateTrigger { .. }
        | Statement::CreateSchema { .. }
        | Statement::CreateSequence { .. }
        | Statement::CreateDatabase { .. }
        | Statement::Truncate { .. }
        | Statement::RenameTable(_) => true,
        // An ALLOW-LIST, pinned to the statements this data plane runs at all: every
        // definition class the authorizer admits ([`crate::state`]'s
        // `confine_to_data_plane`) is named above, and a statement that only reads or
        // changes rows is not one of these. A definition class this data plane starts
        // admitting has to be added here with it — which is why the two lists are stated
        // against each other rather than this one defaulting to "a definition".
        _ => false,
    }
}

/// The name a database statement names, as the catalog holds it: an UNQUOTED
/// identifier is folded to lower case, which is what PostgreSQL does to every name a
/// statement writes without quotes, and a delimited one keeps the case it was written
/// with.
fn database_name(name: &ObjectName) -> String {
    match name.0.last() {
        Some(ObjectNamePart::Identifier(identifier)) if identifier.quote_style.is_none() => {
            identifier.value.to_ascii_lowercase()
        }
        Some(ObjectNamePart::Identifier(identifier)) => identifier.value.clone(),
        _ => String::new(),
    }
}

/// How PostgreSQL states that a database a statement would create is already there,
/// under the SQLSTATE of a duplicate database.
pub(super) fn database_already_exists(name: &str) -> String {
    postgresql_error(
        &format!("database \"{name}\" already exists"),
        SQLSTATE_DUPLICATE_DATABASE,
    )
}

/// How PostgreSQL states that a database a call names is not there, under the SQLSTATE
/// of an invalid catalog name. The sentence a statement about a database it does not
/// have is refused with, and the one the precondition chain refuses a `database` member
/// with, worded once.
pub(crate) fn database_does_not_exist(name: &str) -> String {
    postgresql_error(
        &format!("database \"{name}\" does not exist"),
        SQLSTATE_INVALID_CATALOG_NAME,
    )
}

/// The table an INSERT stores its rows in, lower-cased, and `None` for every other
/// statement: which table it was is what decides whether the engine generated a key
/// for the row it stored (see `Plan::inserted_table`).
fn inserted_table(statement: &Statement) -> Option<String> {
    let Statement::Insert(insert) = statement else {
        return None;
    };
    let TableObject::TableName(name) = &insert.table else {
        return None;
    };
    Some(
        name.0
            .last()?
            .to_string()
            .trim_matches('"')
            .to_ascii_lowercase(),
    )
}

/// The statement as the engine's grammar spells it, and the statements that
/// follow it.
///
/// `TRUNCATE` empties a table, which the engine spells as the delete of every
/// row in it — one delete per table it names, because the engine runs one
/// statement at a time. The row count the call reports is still none, because
/// emptying a table is not a statement that reports the rows it changed.
fn rendered(statement: &Statement) -> (String, Vec<String>) {
    match statement {
        Statement::Truncate { table_names, .. } if !table_names.is_empty() => {
            let mut deletes = table_names
                .iter()
                .map(|target| format!("DELETE FROM {}", target.name));
            let first = deletes.next().unwrap_or_else(|| statement.to_string());
            (first, deletes.collect())
        }
        _ => (statement.to_string(), Vec::new()),
    }
}

/// Substitute the SESSION VALUES a statement names for what this session carries.
///
/// A statement may ask the server who and where it is — "which user am I", "which catalog am
/// I in" — and the answer comes from the CONNECTION rather than from the statement: the
/// database user the call authenticated as, and the database it addressed. Both engines spell
/// most of them as KEYWORDS of the SQL standard (`CURRENT_USER`, `SESSION_USER`), which the
/// parse reads as the identifier they are, so they are replaced here with the value this
/// session holds — written as a string literal, so the engine returns it as the text the
/// server would.
///
/// Only a word the SESSION answers is replaced: a column, an alias or a table of the caller's
/// own that happens to share one of those names is the identifier it is, which is why a name
/// the caller DELIMITED is left exactly as written and why the `USER` of `CREATE USER` is not
/// reached (a statement's keywords are not expressions). The functions the ENGINE itself
/// answers — the clock and `current_database()` — are deliberately absent from this:
/// [`crate::state::dialect_builtins`] defines them on the connection, so a statement that
/// calls one is planned and run like any other.
///
/// A value only the OTHER family's engine answers is not substituted either, so the statement
/// keeps the name it wrote and the engine refuses it as the undefined thing it is on this
/// cluster's engine.
///
/// DEPTH: bounded before the tree exists, exactly as [`cast_values`] states.
fn plan_session_values(statement: &mut Statement, catalog: &Catalog<'_>) {
    let _ = visit_expressions_mut(statement, |expr| {
        let substituted = match &*expr {
            // The keyword forms, which the parse reads as a bare identifier.
            Expr::Identifier(name) if name.quote_style.is_none() => {
                session_value(catalog, &name.value.to_ascii_lowercase(), false)
            }
            // ... and the forms written as a call, which the parse reads as a function of
            // that name. A call carrying ARGUMENTS is not one of these: they take none.
            Expr::Function(function) => match &function.args {
                FunctionArguments::None => session_value(
                    catalog,
                    &function.name.to_string().to_ascii_lowercase(),
                    false,
                ),
                FunctionArguments::List(list) if list.args.is_empty() => session_value(
                    catalog,
                    &function.name.to_string().to_ascii_lowercase(),
                    true,
                ),
                _ => None,
            },
            _ => None,
        };
        if let Some(value) = substituted {
            if let Some(built) = parse_expression(&format!("'{}'", value.replace('\'', "''"))) {
                *expr = built;
            }
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The value the session answers for `named`, or `None` for a name the session does not
/// answer — on the engine this cluster runs.
///
/// `called` says whether the name was written with an argument list, which is what separates
/// the two spellings: `USER` is a function of the MySQL grammar and an identifier otherwise,
/// so a bare `USER` is only the session's identity on the family whose grammar has it as a
/// keyword.
///
/// Sources: the PostgreSQL documentation's "System Information Functions and Operators" —
/// `current_user` ("user name of current execution context"), `session_user` ("session user
/// name"), `user` ("equivalent to current_user"), `current_catalog` ("name of current
/// database") and `current_schema` — and the MySQL Reference Manual's "Information
/// Functions": `CURRENT_USER`, which "returns the user name and host name combination for the
/// MySQL account that the server used to authenticate", and `USER()`/`SESSION_USER()`, which
/// report the account the client authenticated as.
fn session_value(catalog: &Catalog<'_>, named: &str, called: bool) -> Option<String> {
    if catalog.user.is_empty() {
        return None;
    }
    let mysql = catalog.engine == super::EngineFamily::MySql;
    match named {
        // A MySQL privilege is granted to an ACCOUNT, `user@host-pattern`, which is what that
        // family reports for the session's identity; the account the master user holds is
        // granted to any host.
        "current_user" | "current_role" if mysql => {
            Some(format!("{}@{MYSQL_ANY_HOST}", catalog.user))
        }
        "user" | "session_user" if mysql => {
            // `USER` and `SESSION_USER` are FUNCTIONS of the MySQL grammar rather than
            // keywords of it, so a bare `USER` is the identifier it is there.
            called.then(|| format!("{}@{MYSQL_ANY_HOST}", catalog.user))
        }
        "current_user" | "session_user" | "user" | "current_role" => {
            Some(catalog.user.to_string())
        }
        "current_catalog" if !mysql => Some(catalog::catalog_name(catalog.database).to_string()),
        "current_schema" if !mysql && !called => Some(PUBLIC_SCHEMA.to_string()),
        _ => None,
    }
}

/// The host pattern of the account Amazon RDS grants the master user: `%`, "any host".
const MYSQL_ANY_HOST: &str = "%";

/// Rewrite the statement into the form the engine's grammar accepts: the same
/// statement, spelled the way the engine spells it.
///
/// * PostgreSQL's `value::type` and standard `CAST(value AS type)` are one and
///   the same cast, and the engine spells it only the second way; the cast's
///   TARGET becomes the engine type that holds a PostgreSQL value of that type —
///   text for a character, document, date or time type (so a date keeps its
///   digits and separators instead of being read as the number it starts with),
///   an integer, a real or an exact number for the numeric types, and a blob for
///   binary. The PostgreSQL type the cast named is still what the result column
///   reports: it is read off the parse before this runs.
/// * a typed literal, an interval, an array constructor and a clock reading are
///   each the same value written in the engine's own spelling (see the arms
///   below).
/// * a cast to a BINARY type is the reading of that type's own input format
///   ([`read_binary_input`]), because the bytes such a value spells are not the bytes
///   of its text.
/// * `ILIKE` is a case-insensitive `LIKE`, which is what the engine's own `LIKE`
///   is for the ASCII letters it compares.
fn normalise(statement: &mut Statement) {
    // A locking clause asks the engine to hold the rows it read for the
    // transaction; this data plane runs one statement at a time against one
    // database, so the read it names is that same read, with nothing left for
    // the clause to say.
    if let Statement::Query(query) = statement {
        query.locks.clear();
    }
    let _ = visit_expressions_mut(statement, |expr| {
        match expr {
            Expr::Cast {
                kind,
                expr: value,
                data_type,
                ..
            } => match read_typed_input(value, data_type) {
                Some(built) => *expr = built,
                None => {
                    if matches!(kind, CastKind::DoubleColon) {
                        *kind = CastKind::Cast;
                    }
                    *data_type = engine_type(&pg_type(&data_type.to_string()));
                }
            },
            // A typed literal (`DATE '2024-01-01'`, `TIMESTAMP '...'`) is the
            // same value as the cast of that string to that type, which is the
            // form the engine takes.
            Expr::TypedString(typed) => {
                let pg = pg_type(&typed.data_type.to_string());
                let value = &typed.value;
                let built = match pg.class {
                    TypeClass::Binary => parse_expression(&format!("{BINARY_INPUT}({value})")),
                    TypeClass::Float => parse_expression(&format!("{FLOAT_INPUT}({value})")),
                    _ => parse_expression(&format!("CAST({value} AS {})", engine_type(&pg))),
                };
                if let Some(built) = built {
                    *expr = built;
                }
            }
            // A span of time is held as the text it names, the way every other
            // type the engine has no storage class of its own for is.
            Expr::Interval(interval) => {
                if let Some(built) =
                    parse_expression(&format!("CAST({} AS TEXT)", interval.value))
                {
                    *expr = built;
                }
            }
            // An array constructor builds one value out of its elements, which
            // this data plane holds as the JSON document of those elements —
            // the form the engine itself builds and reads.
            Expr::Array(array) => {
                let elements = array
                    .elem
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(built) = parse_expression(&format!("json_array({elements})")) {
                    *expr = built;
                }
            }
            // A case-insensitive match, in the engine's spelling of it.
            Expr::ILike {
                negated,
                expr: value,
                pattern,
                escape_char,
                ..
            } => {
                let not = if *negated { " NOT" } else { "" };
                let escape = match escape_char {
                    Some(character) => format!(" ESCAPE '{character}'"),
                    None => String::new(),
                };
                if let Some(built) =
                    parse_expression(&format!("{value}{not} LIKE {pattern}{escape}"))
                {
                    *expr = built;
                }
            }
            _ => {}
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The data plane's own reader of the INPUT FORMAT a binary value is written in,
/// which `crate::state::Database::open` defines on every connection.
///
/// A value of a binary type can be written as TEXT — PostgreSQL's documented `bytea`
/// hex input format, `\x` followed by two hexadecimal digits per byte — and what the
/// column then holds is the BYTES that text spells, not the text itself. The engine's
/// own cast to its binary type would store the text's own bytes instead, so a cast to
/// a binary type is rendered as this reader, which decodes the format whatever the
/// value turns out to be at run time: a literal the statement carries, or a value a
/// parameter binds.
const BINARY_INPUT: &str = "rds_data_binary_input";

/// A cast to a type whose INPUT FORMAT this data plane reads itself, rendered as that
/// reader, and `None` for a cast the engine's own performs.
///
/// Two of PostgreSQL's types hold values the engine's own cast would not: a BINARY value
/// written as text spells BYTES rather than the text's own bytes ([`BINARY_INPUT`]), and a
/// FLOATING value has three special spellings the engine has no number for
/// ([`FLOAT_INPUT`]).
fn read_typed_input(value: &Expr, data_type: &DataType) -> Option<Expr> {
    let reader = match pg_type(&data_type.to_string()).class {
        TypeClass::Binary => BINARY_INPUT,
        TypeClass::Float => FLOAT_INPUT,
        _ => return None,
    };
    parse_expression(&format!("{reader}({value})"))
}

/// The name of that reader, for the connection that defines it and the authorizer that
/// admits it.
#[must_use]
pub fn binary_input_reader() -> &'static str {
    BINARY_INPUT
}

/// The data plane's own reader of the INPUT a FLOATING type takes, defined on the same
/// connection and for the same reason.
///
/// A floating type's input has three values that are not numbers — `NaN`, `Infinity` and
/// `-Infinity`, which PostgreSQL's "Data Types" chapter documents as the special values
/// every one of its floating types accepts — and the engine behind this data plane has no
/// number for any of them: its own cast would read `'NaN'` as zero and store that, which
/// is a value the caller never wrote. So every cast to a floating type is rendered as this
/// reader, which answers the number for input that names one and the text the database
/// displays the value as for input that names a special value.
const FLOAT_INPUT: &str = "rds_data_float_input";

/// The name of THAT reader, for the connection that defines it and the authorizer that
/// admits it.
#[must_use]
pub fn float_input_reader() -> &'static str {
    FLOAT_INPUT
}

/// The data plane's own writer of the OUTPUT FORMAT a value of an ARRAY type is written
/// in, defined on the same connection as the readers above.
///
/// This data plane holds an array as the JSON document of its elements, which is the form
/// the engine itself builds and reads; the DATABASE writes such a value out in its own
/// documented array output format instead — "curly braces around the array value plus
/// delimiter characters between adjacent items", nested once per dimension — so a
/// statement that writes an array out AS TEXT is rendered as this writer.
const ARRAY_OUTPUT: &str = "rds_data_array_output";

/// The name of that writer, for the connection that defines it and the authorizer that
/// admits it.
#[must_use]
pub fn array_output_writer() -> &'static str {
    ARRAY_OUTPUT
}

/// Rewrite every cast that writes a value of an ARRAY type out AS TEXT into the writer of
/// the array output format.
///
/// A value of an array type has a text representation of its own — the braced format
/// PostgreSQL's "Arrays" chapter documents — and that is what a caller casting one to
/// `text` is given, not the form the engine happens to hold it in. Which expressions are
/// of an array type is settled by the statement's own text and the SCHEMA
/// ([`operand_type`]), so this runs where the catalog is at hand and before [`normalise`]
/// rewrites the cast itself.
fn plan_array_output(statement: &mut Statement, catalog: &Catalog<'_>) {
    let _ = visit_expressions_mut(statement, |expr| {
        if let Expr::Cast {
            expr: value,
            data_type,
            ..
        } = expr
        {
            let writes_text = pg_type(&data_type.to_string()).class == TypeClass::Text;
            let of_an_array =
                operand_type(value, catalog).is_some_and(|pg| pg.class == TypeClass::Array);
            if writes_text && of_an_array {
                if let Some(built) = parse_expression(&format!("{ARRAY_OUTPUT}({value})")) {
                    *expr = built;
                }
            }
        }
        ControlFlow::<()>::Continue(())
    });
}

/// MySQL's own name for the value an upsert's insert half would have stored, which
/// the engine spells as a column of the `excluded` row.
const MYSQL_INSERTED_VALUE_FUNCTION: &str = "VALUES";

/// The row an upsert's update half reads the values it would have inserted from, as
/// the engine names it.
const EXCLUDED_ROW: &str = "excluded";

/// Rewrite an upsert the caller wrote in MySQL's spelling into the one the engine
/// spells.
///
/// The Data API serves Aurora MySQL DB clusters as well as Aurora PostgreSQL ones, so
/// a statement reaching it may be written in MySQL's dialect, and
/// `INSERT … ON DUPLICATE KEY UPDATE <assignments>` is MySQL's UPSERT: the row is
/// inserted, and where it would duplicate a key the named columns of the row already
/// there are updated instead. That is the same statement as the engine's
/// `INSERT … ON CONFLICT (<key>) DO UPDATE SET <assignments>` — and PostgreSQL's own
/// spelling of it — with two differences this closes:
///
/// * the engine NAMES the key the conflict is on, which MySQL leaves implicit ("any
///   unique key"). The key comes from the table's own declaration, which only the
///   database holds, so the caller supplies it (`key_columns`). A table whose
///   declaration carries no key at all can raise no duplicate-key condition, so the
///   clause can never fire: it is DROPPED, and the insert runs as the insert MySQL runs
///   for such a table;
/// * MySQL's `VALUES(<column>)` inside an assignment is "the value that would have
///   been inserted", which the engine reads off the `excluded` row.
///
/// What this does NOT carry over is MySQL's affected-row COUNT: MySQL reports two rows
/// affected for a row its clause UPDATED and one for a row it inserted, while
/// `numberOfRecordsUpdated` reports what the engine behind this data plane counted, which
/// is one either way.
///
/// Nothing here inspects SQL text: the parse is rewritten and re-rendered, like every
/// other rewrite in this module.
fn plan_upsert(statement: &mut Statement, key_columns: &dyn Fn(&str) -> Vec<String>) {
    let Statement::Insert(insert) = statement else {
        return;
    };
    if !matches!(insert.on, Some(OnInsert::DuplicateKeyUpdate(_))) {
        return;
    }
    let TableObject::TableName(name) = &insert.table else {
        return;
    };
    let Some(table) = name.0.last().and_then(|part| match part {
        ObjectNamePart::Identifier(identifier) => Some(identifier.value.clone()),
        _ => None,
    }) else {
        return;
    };
    let keys = key_columns(&table);
    let Some(OnInsert::DuplicateKeyUpdate(mut assignments)) = insert.on.take() else {
        return;
    };
    if keys.is_empty() {
        // No key, so nothing the insert stores can duplicate one and the update half is
        // unreachable: the statement is the insert alone, which is what the engine is
        // handed.
        return;
    }
    for assignment in &mut assignments {
        read_inserted_values_off_the_excluded_row(&mut assignment.value);
    }
    insert.on = Some(OnInsert::OnConflict(OnConflict {
        conflict_target: Some(ConflictTarget::Columns(
            keys.into_iter().map(Ident::new).collect(),
        )),
        action: OnConflictAction::DoUpdate(DoUpdate {
            assignments,
            selection: None,
        }),
    }));
}

/// The columns of the FIRST unique key one table's own `CREATE TABLE` declares, and empty
/// for a declaration that carries none.
///
/// MySQL's upsert conflicts on any unique key, and a UNIQUE declaration is the half of
/// that the engine's permitted schema introspection does not report (`pragma_table_info`
/// reports the primary key alone), so it is read out of the declaration the engine stored
/// — PARSED, like every other statement this module reads, never inspected as text. A
/// single-column `UNIQUE` and a table-level `UNIQUE (a, b)` are both keys; the primary key
/// is not reported here, because its own reading already covers it. The FIRST such key is
/// the answer, for the reason [`crate::state::conflict_key_columns`] gives: the engine
/// spells an upsert with one conflict target.
///
/// Read through the same bounded reader every other statement here goes through, so
/// this path reaches the parser only for a declaration inside the level bound.
#[must_use]
pub fn unique_key_columns(declaration: &str) -> Vec<String> {
    let Ok((_, statements)) = read(declaration) else {
        return Vec::new();
    };
    let [Statement::CreateTable(create)] = statements.as_slice() else {
        return Vec::new();
    };
    for column in &create.columns {
        if column.options.iter().any(|option| {
            matches!(
                option.option,
                ColumnOption::Unique {
                    is_primary: false,
                    ..
                }
            )
        }) {
            return vec![column.name.value.clone()];
        }
    }
    for constraint in &create.constraints {
        if let TableConstraint::Unique { columns, .. } = constraint {
            let named: Vec<String> = columns
                .iter()
                .filter_map(|column| match &column.column.expr {
                    Expr::Identifier(name) => Some(name.value.clone()),
                    _ => None,
                })
                .collect();
            if !named.is_empty() {
                return named;
            }
        }
    }
    Vec::new()
}

/// Rewrite every `VALUES(<column>)` in one assignment into the engine's reading of
/// that same value: the column of the `excluded` row.
fn read_inserted_values_off_the_excluded_row(value: &mut Expr) {
    let _ = visit_expressions_mut(value, |expr| {
        if let Expr::Function(function) = expr {
            if function
                .name
                .to_string()
                .eq_ignore_ascii_case(MYSQL_INSERTED_VALUE_FUNCTION)
            {
                if let Some(column) = sole_column_argument(&function.args) {
                    *expr = Expr::CompoundIdentifier(vec![Ident::new(EXCLUDED_ROW), column]);
                }
            }
        }
        ControlFlow::<()>::Continue(())
    });
}

/// The one column a `VALUES(<column>)` names, or `None` for any other argument list —
/// which is left as the caller wrote it, for the engine to answer.
fn sole_column_argument(args: &FunctionArguments) -> Option<Ident> {
    let FunctionArguments::List(list) = args else {
        return None;
    };
    match list.args.as_slice() {
        [FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Identifier(column)))] => {
            Some(column.clone())
        }
        _ => None,
    }
}

/// Parse one expression this module built, back into the tree the statement
/// carries. The input is this module's own rendering, so a failure to parse it is
/// this module's bug and leaves the expression as the caller wrote it, for the
/// engine to accept or refuse.
fn parse_expression(sql: &str) -> Option<Expr> {
    let mut parser = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(sql)
        .ok()?;
    parser.parse_expr().ok()
}

/// Parse one QUERY this module built, back into the tree the statement carries — the same
/// reading [`parse_expression`] does of an expression, and under the same bound, for a
/// rewrite that replaces a relation with the query that answers it ([`catalog`]).
fn parse_query(sql: &str) -> Option<Box<Query>> {
    let mut parser = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(MAX_PARSE_DEPTH)
        .try_with_sql(sql)
        .ok()?;
    parser.parse_query().ok()
}

/// The engine type that holds a PostgreSQL value of this type.
fn engine_type(pg: &PgType) -> DataType {
    match pg.class {
        TypeClass::Integer => DataType::Integer(None),
        TypeClass::Float => DataType::Real,
        // A boolean is held as the 1 or 0 the engine compares and stores.
        TypeClass::Boolean | TypeClass::Decimal => DataType::Numeric(ExactNumberInfo::None),
        TypeClass::Binary => DataType::Blob(None),
        // An array is held as the document of its elements, which is text.
        TypeClass::Text | TypeClass::Temporal | TypeClass::Array | TypeClass::Other => {
            DataType::Text
        }
    }
}

/// Whether the statement manipulates rows, and so has a row count to report.
fn changes_rows(statement: &Statement) -> bool {
    matches!(
        statement,
        Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_)
    )
}

/// The PostgreSQL type of each column the statement's own text determines.
fn result_types(statement: &Statement, catalog: &Catalog<'_>) -> Vec<Option<PgType>> {
    let projection = match statement {
        Statement::Query(query) => leftmost_projection(&query.body),
        Statement::Insert(insert) => insert.returning.as_deref(),
        Statement::Delete(delete) => delete.returning.as_deref(),
        Statement::Update { returning, .. } => returning.as_deref(),
        _ => None,
    };
    projection
        .unwrap_or_default()
        .iter()
        .map(|item| selected_type(item, catalog))
        .collect()
}

/// The projection whose columns a query's result set takes its names and types
/// from: the leftmost branch of a set operation, which is the one PostgreSQL
/// names the result after.
///
/// Walked ITERATIVELY, because a combined query's depth grows with the request
/// text (see [`chained_levels`]) and a recursive walk of it would grow the stack
/// with it.
fn leftmost_projection(body: &SetExpr) -> Option<&[SelectItem]> {
    let mut body = body;
    loop {
        match body {
            SetExpr::Select(select) => return Some(&select.projection),
            SetExpr::Query(query) => body = &query.body,
            SetExpr::SetOperation { left, .. } => body = left,
            _ => return None,
        }
    }
}

/// The type a call of one of the engines' own DATE AND TIME built-ins answers in, or `None`
/// for a function that is not one of them.
///
/// The functions and what each one READS are [`::service_common::sql_dialect_time`], which is
/// also where the engine behind this data plane is given them: `columnMetadata` therefore
/// reports the type of the value the engine really computed.
fn clock_reading_type(name: &str) -> Option<PgType> {
    use ::service_common::sql_dialect_time::Reading;
    Some(match ::service_common::sql_dialect_time::reading_of(name)? {
        Reading::Timestamp => pg_type("timestamptz"),
        Reading::Date => pg_type("date"),
        Reading::Time => pg_type("time"),
        Reading::EpochSeconds => pg_type("int8"),
    })
}

/// The type one selected expression has, when its own form says.
fn selected_type(item: &SelectItem, catalog: &Catalog<'_>) -> Option<PgType> {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            expression_type(expr, catalog)
        }
        // A wildcard stands for columns the statement does not list; the engine
        // reports their declared types.
        _ => None,
    }
}

/// The type an expression has when its own form determines it: a cast names its
/// target type, a literal has the type PostgreSQL gives that literal, and a
/// function's result has the type that function's definition gives it over the
/// arguments it was given. Any other expression is left to the engine, which reports
/// the declared type of a stored column and the storage class of a computed one.
fn expression_type(expr: &Expr, catalog: &Catalog<'_>) -> Option<PgType> {
    match expr {
        Expr::Cast { data_type, .. } => Some(pg_type(&data_type.to_string())),
        // A typed literal names its own type, and a span of time is an interval.
        Expr::TypedString(typed) => Some(pg_type(&typed.data_type.to_string())),
        Expr::Interval(_) => Some(pg_type("interval")),
        Expr::Nested(inner) => expression_type(inner, catalog),
        Expr::UnaryOp { expr, .. } => expression_type(expr, catalog),
        Expr::Value(value) => literal_type(&value.value),
        // A clock built-in answers in the type ITS OWN reading has: a whole timestamp with
        // the time zone it was read at, a date, a time of day, or the seconds since the epoch
        // ([`::service_common::sql_dialect_time`], the one table of those built-ins).
        Expr::Function(function) if clock_reading_type(&function.name.to_string()).is_some() => {
            clock_reading_type(&function.name.to_string())
        }
        Expr::Function(function) => function_type(function, catalog),
        // An arithmetic over values of two types answers in the type that holds the
        // result: PostgreSQL's own rule for its numeric types, where an INEXACT operand
        // makes the result inexact and an exact one keeps it exact — in an
        // UNCONSTRAINED exact type, because the digits of a sum or a product are not the
        // digits either operand was declared with.
        Expr::BinaryOp { left, op, right } if is_arithmetic(op) => {
            let operands = [operand_type(left, catalog), operand_type(right, catalog)];
            let classes: Vec<TypeClass> =
                operands.iter().flatten().map(|pg| pg.class).collect();
            if classes.contains(&TypeClass::Float) {
                Some(pg_type("float8"))
            } else if classes.contains(&TypeClass::Decimal) {
                Some(pg_type("numeric"))
            } else {
                // Every other combination is left to the engine, which reports the
                // storage class its values arrive in.
                None
            }
        }
        // An array constructor is an array of whatever its elements are, and an
        // array of arrays is the multidimensional array the model names.
        Expr::Array(array) => Some(array_of(
            array
                .elem
                .iter()
                .find_map(|element| expression_type(element, catalog))
                .unwrap_or_else(|| pg_type("text")),
        )),
        _ => None,
    }
}

/// The type a function's result has, for the functions whose own definition gives it
/// one: the aggregates and the window functions, whose results the statement's text
/// types and no table declares.
///
/// PostgreSQL's "Aggregate Functions" and "Window Functions" chapters give each one:
/// a count, and a row's position in its window, are `bigint`; a SUM is exact where its
/// argument is exact, over the width that holds the total ([`sum_type`]); an average is
/// `numeric` over an exact argument; and the functions that answer ONE of the values
/// they were given (`min`, `max`, `lag`, `lead`, the first, last and nth of a window)
/// have their argument's own type. Every other function is left to the engine.
fn function_type(function: &Function, catalog: &Catalog<'_>) -> Option<PgType> {
    let name = function
        .name
        .0
        .last()?
        .to_string()
        .trim_matches('"')
        .to_ascii_lowercase();
    match name.as_str() {
        "count" | "row_number" | "rank" | "dense_rank" | "ntile" => Some(pg_type("int8")),
        "sum" => Some(sum_type(first_argument_type(function, catalog)?)),
        "avg" => Some(average_type(&first_argument_type(function, catalog)?)),
        "min" | "max" | "lag" | "lead" | "first_value" | "last_value" | "nth_value" => {
            first_argument_type(function, catalog)
        }
        _ => None,
    }
}

/// PostgreSQL's SUM: EXACT over an exact argument, in the width that holds the total —
/// `bigint` over the narrower integers, and `numeric` over `bigint`, whose total a
/// `bigint` could not hold. The sum of values of one scale has that same scale, so an
/// exact argument's own digits and scale are the sum's.
fn sum_type(argument: PgType) -> PgType {
    match (argument.class, argument.name.as_str()) {
        (TypeClass::Integer, "int8" | "oid") => pg_type("numeric"),
        (TypeClass::Integer, _) => pg_type("int8"),
        _ => argument,
    }
}

/// PostgreSQL's AVG: `numeric` over an exact argument (an integer's average is not an
/// integer), and `double precision` over an inexact one.
fn average_type(argument: &PgType) -> PgType {
    match argument.class {
        TypeClass::Integer | TypeClass::Decimal => pg_type("numeric"),
        _ => pg_type("float8"),
    }
}

/// The type of the first value a function was given: the expression's own, or — for a
/// bare column reference, whose type only the schema holds — the type the schema
/// declares for that column.
fn first_argument_type(function: &Function, catalog: &Catalog<'_>) -> Option<PgType> {
    let FunctionArguments::List(list) = &function.args else {
        return None;
    };
    let FunctionArg::Unnamed(FunctionArgExpr::Expr(argument)) = list.args.first()? else {
        return None;
    };
    operand_type(argument, catalog)
}

/// The type one operand of an expression has: the expression's own, or — for a bare
/// column reference, whose type only the schema holds — the type the schema declares
/// for that column.
fn operand_type(expr: &Expr, catalog: &Catalog<'_>) -> Option<PgType> {
    if let Some(known) = expression_type(expr, catalog) {
        return Some(known);
    }
    let column = match expr {
        Expr::Identifier(name) => name.value.clone(),
        Expr::CompoundIdentifier(parts) => parts.last()?.value.clone(),
        _ => return None,
    };
    Some(pg_type(&(catalog.column_type)(&column)?))
}

/// Whether an operator computes a VALUE out of its two operands, as the arithmetic
/// operators do — as against comparing them or combining two conditions, which answer
/// a boolean the engine reports itself.
fn is_arithmetic(op: &BinaryOperator) -> bool {
    matches!(
        op,
        BinaryOperator::Plus
            | BinaryOperator::Minus
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo
    )
}

/// The type a CAST the statement writes names for the value a named placeholder
/// carries.
///
/// A cast is how a statement says which type a value written as text IS, so the value
/// the placeholder carries has to be a value of that type — which only the request's
/// `parameters` hold, so the check itself is the caller's
/// (`crate::state::Database::require_readable_values`). Read off the parse BEFORE the
/// rewrite, which replaces a cast's PostgreSQL type with the engine type that holds
/// its values.
///
/// DEPTH: the walk descends the statement tree, which [`read_as`] has already bounded
/// BEFORE a tree exists at all ([`require_bounded_levels`]): a chain the parse builds
/// iteratively — combined queries, and the operators and casts that each add a level —
/// is counted on the TOKENS and refused past [`MAX_SET_OPERATIONS`] /
/// [`MAX_CHAINED_LEVELS`], and the nesting the parse DOES recurse through is capped by
/// [`MAX_PARSE_DEPTH`]. So no statement that reaches this walk, or any other walk in
/// this module, carries a depth a request chose.
fn cast_values(statement: &Statement) -> Vec<(String, PgType)> {
    let mut cast = Vec::new();
    let _ = visit_expressions(statement, |expr| {
        if let Expr::Cast {
            expr: value,
            data_type,
            ..
        } = expr
        {
            if let Some(placeholder) = placeholder_name(value) {
                cast.push((placeholder, pg_type(&data_type.to_string())));
            }
        }
        ControlFlow::<()>::Continue(())
    });
    cast
}

/// Refuse a statement that writes a value out as TEXT and names a NUMBER type for it,
/// where the text is not a number of that type.
///
/// A value written as text and cast to a number is read as that type's own input, and
/// the database refuses text that is not one of its values — "invalid input syntax for
/// type <type>" — rather than reading as much of it as it can and answering with a
/// value the caller never wrote. Read off the PARSE, before anything runs, because the
/// statement's own text is what names both the value and the type.
///
/// DEPTH: bounded before the tree exists, exactly as [`cast_values`] above states.
fn require_valid_literals(statement: &Statement) -> Result<(), SqlError> {
    let mut refusal = None;
    let _ = visit_expressions(statement, |expr| {
        if let Some(message) = invalid_literal(expr) {
            refusal = Some(message);
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    match refusal {
        Some(message) => Err(SqlError::Engine(message)),
        None => Ok(()),
    }
}

/// What ONE expression earns a refusal for: it casts a value written as text to a
/// number type the text is no value of.
fn invalid_literal(expr: &Expr) -> Option<String> {
    let Expr::Cast {
        expr: value,
        data_type,
        ..
    } = expr
    else {
        return None;
    };
    let text = quoted_text(value)?;
    let pg = pg_type(&data_type.to_string());
    (!reads_text(&pg, &text)).then(|| invalid_input_syntax(pg.worded_name(), &text))
}

/// Whether a type READS this text as one of its own values.
///
/// A number type reads the digits of a number of its own kind and nothing else — a
/// fraction is no integer — and every other type reads its own input, which the engine
/// or this front end answers for.
pub(super) fn reads_text(pg: &PgType, text: &str) -> bool {
    match pg.class {
        TypeClass::Integer => text.trim().parse::<i64>().is_ok(),
        TypeClass::Float | TypeClass::Decimal => text.trim().parse::<f64>().is_ok(),
        _ => true,
    }
}

/// The text a quoted string literal carries, and `None` for every other expression.
fn quoted_text(expr: &Expr) -> Option<String> {
    let Expr::Value(literal) = expr else {
        return None;
    };
    match &literal.value {
        Value::SingleQuotedString(text) => Some(text.clone()),
        _ => None,
    }
}

/// How PostgreSQL states that text is not a value of the type it was written as, under
/// the SQLSTATE of an invalid textual representation.
pub(super) fn invalid_input_syntax(declared: &str, value: &str) -> String {
    postgresql_error(
        &format!("invalid input syntax for type {declared}: \"{value}\""),
        SQLSTATE_INVALID_TEXT_REPRESENTATION,
    )
}

/// The type PostgreSQL gives a literal: an integer literal is the narrowest
/// integer type that holds it, a fractional literal is exact (`numeric`), a
/// quoted string is `text` and `true`/`false` is `bool`.
fn literal_type(value: &Value) -> Option<PgType> {
    match value {
        Value::Boolean(_) => Some(pg_type("bool")),
        Value::Number(digits, _) => Some(match digits.parse::<i32>() {
            Ok(_) => pg_type("int4"),
            Err(_) => match digits.parse::<i64>() {
                Ok(_) => pg_type("int8"),
                Err(_) => pg_type("numeric"),
            },
        }),
        Value::SingleQuotedString(_) | Value::DollarQuotedString(_) => Some(pg_type("text")),
        _ => None,
    }
}

/// The sequence of every identity column a `CREATE TABLE` declares: the columns
/// the store remembers, and the statements that install a sequence the engine
/// does not keep itself.
///
/// An identity column is declared in either of the two spellings the Data API's own
/// engines have: PostgreSQL's `serial` family ("an autoincrementing integer column")
/// and MySQL's `AUTO_INCREMENT` column attribute. Either way the sequence supplies the
/// column's value when an INSERT leaves it out. The engine assigns
/// an integer PRIMARY KEY's value itself, during the insert, so such a
/// primary key is declared to it as that — which is also what makes a `RETURNING`
/// clause report the assigned value, as the `generatedFields` documentation says
/// to read it. The key is declared AUTOINCREMENT as well, so the engine keeps the
/// sequence's own high-water mark rather than re-deriving it from the rows that
/// are still there: a KEY's sequence never hands out a value twice, so a value it
/// supplied for a row since deleted is not supplied again.
///
/// An identity column that is NOT its table's key is given its value by a trigger
/// on the table — the next value above the largest the column holds — because
/// only the key is assigned by the engine itself, and only the key's counter is
/// one the engine keeps: a non-key sequence therefore does re-use the value of a
/// row that has since been deleted.
fn plan_sequences(statement: &mut Statement) -> (Vec<SequenceColumn>, Vec<String>) {
    let Statement::CreateTable(create) = statement else {
        return (Vec::new(), Vec::new());
    };
    let table = create.name.to_string();
    let table_key = create
        .name
        .0
        .last()
        .map(|part| part.to_string())
        .unwrap_or_default()
        .trim_matches('"')
        .to_ascii_lowercase();
    let scope = if create.temporary { "TEMP " } else { "" };
    let mut columns = Vec::new();
    let mut sequences = Vec::new();
    for column in &mut create.columns {
        let declared = pg_type(&column.data_type.to_string());
        // Taken off the declaration before the key is located, because the engine
        // spells this attribute differently and in another place.
        let generated_key = take_generated_key_attribute(column);
        if !declared.auto_increment && !generated_key {
            continue;
        }
        let key = column
            .options
            .iter()
            .position(|option| matches!(option.option, ColumnOption::Unique { is_primary: true, .. }));
        column.data_type = DataType::Integer(None);
        match key {
            // The engine's own key counter, which it keeps where the key is
            // declared: directly after it, because that is where its grammar
            // takes it.
            Some(position) => column.options.insert(
                position + 1,
                ColumnOptionDef {
                    name: None,
                    option: ColumnOption::DialectSpecific(vec![Token::make_keyword(
                        "AUTOINCREMENT",
                    )]),
                },
            ),
            None => {
                let name = column.name.to_string();
                let trigger = quoted(&format!("{table_key}_{}_nextval", column.name.value));
                sequences.push(format!(
                    "CREATE {scope}TRIGGER IF NOT EXISTS {trigger} AFTER INSERT ON {table} \
                     WHEN NEW.{name} IS NULL \
                     BEGIN UPDATE {table} SET {name} = \
                     (SELECT IFNULL(MAX({name}), 0) + 1 FROM {table}) WHERE rowid = NEW.rowid; END"
                ));
            }
        }
        columns.push(SequenceColumn {
            table: table_key.clone(),
            column: column.name.value.to_ascii_lowercase(),
            type_name: declared.name.clone(),
            // A key the ENGINE assigns and whose value the API reports back: MySQL's
            // `AUTO_INCREMENT` key is the one identity column `generatedFields`
            // carries, and the engine assigns it as the table's key, so its value is
            // the key of the row the insert stored.
            reports_generated_key: generated_key && key.is_some(),
        });
    }
    (columns, sequences)
}

/// MySQL's `AUTO_INCREMENT` column attribute, TAKEN OFF the declaration: the engine
/// behind this data plane spells its own identity key differently, and directly after
/// the key rather than anywhere among the column's attributes, so the attribute is
/// removed here and the engine's own is put back by [`plan_sequences`].
fn take_generated_key_attribute(column: &mut ColumnDef) -> bool {
    let Some(position) = column
        .options
        .iter()
        .position(|option| is_generated_key_attribute(&option.option))
    else {
        return false;
    };
    column.options.remove(position);
    true
}

/// Whether one column attribute is the identity attribute MySQL spells
/// `AUTO_INCREMENT` — the dialect-specific word the parse keeps as it was written.
fn is_generated_key_attribute(option: &ColumnOption) -> bool {
    let ColumnOption::DialectSpecific(tokens) = option else {
        return false;
    };
    tokens.iter().any(|token| match token {
        Token::Word(word) => word.value.eq_ignore_ascii_case(MYSQL_GENERATED_KEY),
        _ => false,
    })
}

/// MySQL's own name for the attribute that makes a column's values the engine's to
/// assign.
const MYSQL_GENERATED_KEY: &str = "AUTO_INCREMENT";

/// The tables a `DROP TABLE` drops, lower-cased.
fn dropped_tables(statement: &Statement) -> Vec<String> {
    match statement {
        Statement::Drop {
            object_type: ObjectType::Table,
            names,
            ..
        } => names
            .iter()
            .map(|name| {
                name.0
                    .last()
                    .map(|part| part.to_string())
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_ascii_lowercase()
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An identifier, quoted so that whatever it holds is one identifier.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The PostgreSQL type a declaration or a cast spells `declared`.
///
/// The name is matched against the catalog's own names and the aliases
/// PostgreSQL documents for them, so `integer`, `int` and `int4` are one type and
/// all three report `int4`. A type the catalog does not name is reported under the
/// name it was written with, with no `java.sql.Types` code of its own.
pub fn pg_type(declared: &str) -> PgType {
    let (name, arguments) = split_declared_type(declared);
    // An array type is named after its element, in either spelling the parser
    // renders (`text[]`, `ARRAY<TEXT>`).
    if let Some(element) = array_element_name(&name) {
        let element = match array_element_name(element) {
            // An array OF arrays is already a result this service cannot
            // return, so the nesting is not followed further than it takes to
            // see it: however deeply a caller nests it, this looks twice.
            Some(_) => PgType::of(element, TypeClass::Array, jdbc::ARRAY, UNLIMITED_LENGTH, 0),
            None => pg_type(&with_arguments(element, &arguments)),
        };
        return array_of(element);
    }
    let length = arguments.first().copied();
    let scale = arguments.get(1).copied();
    match name.as_str() {
        "bool" | "boolean" => PgType::of("bool", TypeClass::Boolean, jdbc::BIT, 1, 0),
        "int2" | "smallint" => PgType::of("int2", TypeClass::Integer, jdbc::SMALLINT, 5, 0),
        "int4" | "int" | "integer" => PgType::of("int4", TypeClass::Integer, jdbc::INTEGER, 10, 0),
        "int8" | "bigint" => PgType::of("int8", TypeClass::Integer, jdbc::BIGINT, 19, 0),
        "oid" => PgType::of("oid", TypeClass::Integer, jdbc::BIGINT, 10, 0),
        "float4" | "real" => PgType::of("float4", TypeClass::Float, jdbc::REAL, 8, 8),
        "float8" | "double precision" => {
            PgType::of("float8", TypeClass::Float, jdbc::DOUBLE, 17, 17)
        }
        "numeric" | "decimal" | "dec" => {
            let mut numeric = PgType::of(
                "numeric",
                TypeClass::Decimal,
                jdbc::NUMERIC,
                length.unwrap_or(0),
                scale.unwrap_or(0),
            );
            numeric.declared_scale = scale;
            numeric
        }
        "text" => {
            PgType::of("text", TypeClass::Text, jdbc::VARCHAR, UNLIMITED_LENGTH, 0).takes_text()
        }
        "varchar" | "character varying" => PgType::of(
            "varchar",
            TypeClass::Text,
            jdbc::VARCHAR,
            length.unwrap_or(UNLIMITED_LENGTH),
            0,
        )
        .takes_text(),
        "bpchar" | "char" | "character" => {
            PgType::of("bpchar", TypeClass::Text, jdbc::CHAR, length.unwrap_or(1), 0).takes_text()
        }
        // The type PostgreSQL's catalog uses for an identifier, whose documented
        // length is 63 bytes.
        "name" => PgType::of("name", TypeClass::Text, jdbc::VARCHAR, 63, 0).takes_text(),
        "uuid" => PgType::of("uuid", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0),
        "json" => {
            PgType::of("json", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0).document()
        }
        "jsonb" => {
            PgType::of("jsonb", TypeClass::Text, jdbc::OTHER, UNLIMITED_LENGTH, 0).document()
        }
        "bytea" => PgType::of(
            "bytea",
            TypeClass::Binary,
            jdbc::BINARY,
            UNLIMITED_LENGTH,
            0,
        ),
        // A date and time type reports the width of the value it displays:
        // `YYYY-MM-DD` is 13 with room for an era, a time adds the clock and a
        // fraction, a timestamp both, and a zone offset six more. Every one of them
        // but the date carries a time, which PostgreSQL's "Date/Time Types" documents
        // as resolving to one microsecond: six fractional digits, which is the SCALE
        // such a column reports.
        "date" => PgType::of("date", TypeClass::Temporal, jdbc::DATE, 13, 0),
        "time" | "time without time zone" => {
            PgType::of("time", TypeClass::Temporal, jdbc::TIME, 15, MICROSECOND_SCALE)
        }
        "timetz" | "time with time zone" => PgType::of(
            "timetz",
            TypeClass::Temporal,
            // `java.sql.Types` names no time carrying a zone either.
            jdbc::OTHER,
            21,
            MICROSECOND_SCALE,
        ),
        "timestamp" | "timestamp without time zone" => PgType::of(
            "timestamp",
            TypeClass::Temporal,
            jdbc::TIMESTAMP,
            29,
            MICROSECOND_SCALE,
        ),
        "timestamptz" | "timestamp with time zone" => PgType::of(
            "timestamptz",
            TypeClass::Temporal,
            // `java.sql.Types` has no constant for a timestamp that carries a time
            // zone, so the one for a type it does not name is what the Data API
            // reports for it.
            jdbc::OTHER,
            35,
            MICROSECOND_SCALE,
        ),
        "serial" | "serial4" => {
            PgType::of("int4", TypeClass::Integer, jdbc::INTEGER, 10, 0).sequence("serial")
        }
        "bigserial" | "serial8" => {
            PgType::of("int8", TypeClass::Integer, jdbc::BIGINT, 19, 0).sequence("bigserial")
        }
        "smallserial" | "serial2" => {
            PgType::of("int2", TypeClass::Integer, jdbc::SMALLINT, 5, 0).sequence("smallserial")
        }
        // A span of time is neither a number nor a string: the `Field` union has
        // no member for one, so a result column of that type is refused rather
        // than reported as something it is not.
        "interval" => PgType::of("interval", TypeClass::Other, jdbc::OTHER, 49, 0).unreturnable(),
        // And neither is a currency amount: `money` holds a fixed-point amount whose
        // fractional digits and symbol come from the database's monetary locale, which no
        // member of the union carries either — a `doubleValue` would be an inexact number
        // with no currency — so that result is refused too. It is a type the catalog NAMES,
        // so a value written as text is no value of it, any more than it is of a date.
        "money" => {
            PgType::of("money", TypeClass::Other, jdbc::OTHER, UNLIMITED_LENGTH, 0).unreturnable()
        }
        // A type the catalog does not name is one this front end cannot type, so a
        // value written as text is not refused for it.
        _ => PgType::of(&name, TypeClass::Other, jdbc::OTHER, UNLIMITED_LENGTH, 0).takes_text(),
    }
}

/// The element a type name names, when the name is an array type's.
fn array_element_name(name: &str) -> Option<&str> {
    name.strip_suffix("[]").or_else(|| {
        name.strip_prefix("array<")
            .and_then(|rest| rest.strip_suffix('>'))
    })
}

/// A type name with the arguments the declaration gave it put back, so an
/// element type carries the length its array's declaration named
/// (`varchar(8)[]`).
fn with_arguments(name: &str, arguments: &[i32]) -> String {
    if arguments.is_empty() {
        return name.to_string();
    }
    let arguments = arguments
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}({arguments})")
}

/// The type a result column has when neither its declaration nor the statement's
/// text names one: the PostgreSQL type of the storage class the engine returned
/// its values in.
pub fn type_of_storage_class(class: Option<&str>) -> PgType {
    match class {
        Some("INTEGER") => pg_type("int8"),
        Some("REAL") => pg_type("float8"),
        Some("BLOB") => pg_type("bytea"),
        _ => pg_type("text"),
    }
}

/// Split a type as it was written into its name and its arguments:
/// `VARCHAR(32)` -> (`varchar`, [32]), `NUMERIC(10,2)` -> (`numeric`, [10, 2]),
/// `TIMESTAMP(3) WITH TIME ZONE` -> (`timestamp with time zone`, [3]). The
/// arguments are clamped, so a declaration cannot put an arbitrary number into
/// `columnMetadata` or into arithmetic over it.
fn split_declared_type(declared: &str) -> (String, Vec<i32>) {
    let lower = declared.to_ascii_lowercase();
    let (head, arguments, tail) = match lower.split_once('(') {
        // Whatever follows the arguments is part of the name
        // (`WITH TIME ZONE`), so the three parts are kept apart.
        Some((head, rest)) => match rest.split_once(')') {
            Some((arguments, tail)) => (head, arguments, tail),
            None => (head, rest, ""),
        },
        None => (lower.as_str(), "", ""),
    };
    let numbers = arguments
        .split(',')
        .filter_map(|argument| argument.trim().parse::<i32>().ok())
        .map(|number| number.clamp(0, MAX_DECLARED_ARGUMENT))
        .collect();
    let name = collapse_whitespace(&format!("{head} {tail}"));
    (name, numbers)
}

/// A type name with its words separated by exactly one space, so the multi-word
/// spellings PostgreSQL documents (`double precision`) match the catalog however
/// they were laid out.
fn collapse_whitespace(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}
