// SPDX-License-Identifier: CC0-1.0

//! Reading a submitted statement as AMAZON REDSHIFT's dialect, and handing the
//! engine what it has to run for it.
//!
//! The Data API runs whatever SQL its data warehouse accepts, and that warehouse
//! is PostgreSQL-derived: a caller writes `<expr>::<type>` for a cast, qualifies
//! a table with the schema it lives in, creates those schemas with `CREATE
//! SCHEMA`, and serialises writers with `LOCK TABLE`. This emulator's engine is
//! a different SQL engine, so a statement in that dialect is READ here — by the
//! dialect-aware parser this crate already depends on, never by a text scan —
//! and turned into the work the engine can really do:
//!
//! * a construct the engine spells differently is re-spelled ([`Translated::Run`]):
//!   a `::` cast becomes the standard `CAST(… AS …)`, and a schema-qualified name
//!   becomes the engine name that schema's tables are held under;
//! * a statement about the database's own CATALOGUE of schemas, which the engine
//!   has no notion of, is applied to that catalogue instead
//!   ([`Translated::CreateSchema`], [`Translated::DropSchema`]), and one about the
//!   WAREHOUSE's catalogue of databases to the catalogue that holds those
//!   ([`Translated::CreateDatabase`]);
//! * `LOCK` asks for a lock on tables in a mode ([`Translated::Lock`]): the
//!   relations it names are resolved — a `LOCK` on a table that does not exist
//!   is an error on the real service too — and a lock taken inside an open
//!   transaction is held by the CONNECTION that took it (see
//!   [`super::connections`]), so a statement on another connection that needs a
//!   conflicting lock on the same table waits for it;
//! * a TEMPORARY relation belongs to the connection that created it, so it is
//!   created under, and resolved against, that connection's own temporary names
//!   ([`super::TempRelations`], [`Translated::RunCreatingTemp`]): a statement on
//!   another connection cannot resolve it at all;
//! * a transaction control the warehouse answers with a WARNING where the engine
//!   answers an error — a `BEGIN` inside a transaction, a `COMMIT` outside one —
//!   is reported as the warehouse reports it ([`Translated::Transaction`]).
//!
//! Nothing else is rewritten. A statement the parser cannot read, and one no
//! rewrite applies to, is handed to the engine EXACTLY as the caller wrote it:
//! the engine is what runs a statement and what reports one it cannot run, so a
//! statement is never re-rendered unless a rewrite really changed it. A text
//! longer than the engine itself accepts is not read here at all, so every pass
//! this module makes is bounded by that length and costs one walk of the text.
//!
//! The Redshift functions the engine does not carry are a separate concern, and
//! live in [`super::functions`]: they are real implementations registered on the
//! connection rather than anything rewritten here.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use sqlparser::ast::{
    CastKind, Expr, Ident, ObjectName, ObjectNamePart, ObjectType, SchemaName, Statement, Visit,
    VisitMut, Visitor, VisitorMut,
};
use sqlparser::dialect::RedshiftSqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::{LockMode, LockRequest, TempRelations, TEMP_QUALIFIER};

/// The schemas one database holds, by name. Ordered, so a catalogue read of it
/// is in name order whatever order the schemas were created in.
pub type SchemaSet = BTreeSet<String>;

/// What the engine has to do for one submitted statement.
#[derive(Debug, PartialEq, Eq)]
pub enum Translated {
    /// Run this SQL on the engine: the statement as the caller wrote it, or the
    /// engine's own spelling of it where a rewrite applied.
    Run(String),
    /// Run this SQL, which CREATES a temporary relation of the connection the
    /// statement runs on: the engine holds it under that connection's own name,
    /// and the connection is what resolves it afterwards.
    RunCreatingTemp {
        /// The statement, with the created relation named as the engine holds
        /// it.
        sql: String,
        /// The relation's name as the caller spelled it, which is what later
        /// statements of that connection resolve.
        relation: String,
    },
    /// Add a schema to the database's catalogue.
    CreateSchema {
        /// The schema to create.
        name: String,
        /// `IF NOT EXISTS`: creating one that is already there is not an error.
        if_not_exists: bool,
    },
    /// Remove schemas from the database's catalogue.
    DropSchema {
        /// The schemas to drop, in the order the statement names them.
        names: Vec<String>,
        /// `IF EXISTS`: dropping one that is not there is not an error.
        if_exists: bool,
        /// `CASCADE`: drop the tables the schema holds with it. Without it the
        /// drop is `RESTRICT`, which refuses a schema that still holds objects.
        cascade: bool,
    },
    /// Lock tables: resolve the relations the statement names and hold them for
    /// the connection's transaction.
    Lock {
        /// The ENGINE names of the tables the statement names, in its order.
        tables: Vec<String>,
        /// The mode the statement asks for — `ACCESS EXCLUSIVE` when it names
        /// none, which is the mode Amazon Redshift documents `LOCK` as taking.
        mode: LockMode,
    },
    /// Add a database to the WAREHOUSE's catalogue. The engine behind this
    /// service holds one database per connection, so this is not its statement
    /// to run: it is applied where that catalogue lives.
    CreateDatabase {
        /// The database to create.
        name: String,
    },
    /// A transaction-control statement, which the data warehouse answers with a
    /// WARNING rather than an error when there is nothing for it to do — a
    /// `BEGIN` inside a transaction, a `COMMIT` or `ROLLBACK` outside one.
    Transaction(TransactionControl),
}

/// What a transaction-control statement does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionControl {
    /// `BEGIN`/`START TRANSACTION`: open a transaction. "BEGIN … There is no
    /// effect on any transaction that is already in progress", so one issued
    /// inside a transaction changes nothing.
    Begin,
    /// `COMMIT`/`END`: commit the transaction in progress. One issued outside a
    /// transaction changes nothing.
    Commit,
    /// `ROLLBACK`/`ABORT`: abort the transaction in progress. One issued outside
    /// a transaction changes nothing.
    Rollback,
}

impl TransactionControl {
    /// The statement the engine runs for this control, in its own spelling: the
    /// warehouse's optional clauses (`TRANSACTION`, `WORK`, an isolation level,
    /// a read/write mode) name nothing the engine's one-writer transactions do
    /// differently, so what is run is the control itself.
    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Begin => "BEGIN",
            Self::Commit => "COMMIT",
            Self::Rollback => "ROLLBACK",
        }
    }

    /// Whether this control OPENS a transaction, as opposed to closing one.
    pub fn opens(self) -> bool {
        self == Self::Begin
    }
}

/// Read `sql` as Redshift SQL and report what the engine has to do for it.
///
/// `schemas` is the database's own catalogue of schemas, which is what decides
/// whether a qualified name names a schema's table; `temp` is the temporary
/// relations of the CONNECTION the statement runs on, which is what decides
/// whether an unqualified name is one of those instead.
pub fn translate(sql: &str, schemas: &SchemaSet, temp: &TempRelations<'_>) -> Translated {
    if !within_readable_length(sql) {
        return Translated::Run(sql.to_string());
    }
    // Two statements of this dialect the PARSER does not model are read from the
    // token stream, because there is no parsed form of them to read; every other
    // statement below is read from the parsed statement itself, so the grammar is
    // the parser's and not a second copy of it.
    if let Some(tokens) = significant_tokens(sql) {
        if let Some(unparsed) = unparsed_statement(&tokens, schemas, temp) {
            return unparsed;
        }
    }
    let dialect = RedshiftSqlDialect {};
    let Ok(mut statements) = Parser::parse_sql(&dialect, sql) else {
        return Translated::Run(sql.to_string());
    };
    // One statement per call ([`split_statements`] has already cut the
    // submission's text), so anything else is left to the engine as written.
    let [statement] = statements.as_mut_slice() else {
        return Translated::Run(sql.to_string());
    };
    if let Some(catalogue) = catalogue_statement(statement) {
        return catalogue;
    }
    // A statement that CREATES a temporary relation is renamed first, so the
    // walk below does not read the name it creates as a reference to a relation
    // the connection already holds.
    let created = create_temp_relation(statement, temp);
    let mut rewriter = Rewriter {
        schemas,
        temp: *temp,
        changed: created.is_some(),
    };
    let _walked = statement.visit(&mut rewriter);
    // A statement is re-rendered only when a rewrite CHANGED it: the parser's
    // own rendering is not the caller's text, so a statement nothing applied to
    // is run exactly as it was submitted.
    let text = if rewriter.changed {
        statement.to_string()
    } else {
        sql.to_string()
    };
    match created {
        Some(relation) => Translated::RunCreatingTemp {
            sql: text,
            relation,
        },
        None => Translated::Run(text),
    }
}

/// Rename the TEMPORARY relation a statement creates to the name the engine
/// holds this connection's temporary relations under, and report the name the
/// caller gave it.
///
/// `None` for every other statement, and for a temporary relation the caller
/// qualified with a schema of its own, which names something this mapping does
/// not reach.
fn create_temp_relation(statement: &mut Statement, temp: &TempRelations<'_>) -> Option<String> {
    let name = match statement {
        Statement::CreateTable(create) if create.temporary => &mut create.name,
        Statement::CreateView {
            temporary: true,
            name,
            ..
        } => name,
        _ => return None,
    };
    let [part] = name.0.as_slice() else {
        return None;
    };
    let relation = part.as_ident()?.value.clone();
    // The engine name carries a dot, so it is a quoted identifier: unquoted it
    // would address a schema the engine does not have.
    name.0 = vec![ObjectNamePart::Identifier(Ident::with_quote(
        '"',
        temp.engine_name(&relation),
    ))];
    Some(relation)
}

/// Whether a text is one this module READS at all.
///
/// A text longer than the engine itself accepts (`super::MAX_SQL_BYTES`, its
/// `SQLITE_LIMIT_SQL_LENGTH`) is handed to the engine unread: the engine is what
/// refuses it, and every pass this module makes is therefore bounded by the
/// length of text the engine accepts rather than by whatever a request carries.
fn within_readable_length(sql: &str) -> bool {
    usize::try_from(super::MAX_SQL_BYTES).is_ok_and(|max| sql.len() <= max)
}

/// The ENGINE name for the table a statement's qualified name parts address.
///
/// Redshift resolves an unqualified name through the search path, whose default
/// holds [`super::DEFAULT_SCHEMA`], so a name in that schema and an unqualified
/// one are the same table; a name in any OTHER schema the database holds is a
/// table of its own, held under the qualified name. A name whose schema the
/// database does not hold is left as written, so the engine reports it as the
/// relation it cannot find.
pub fn engine_table_name(parts: &[String], schemas: &SchemaSet) -> String {
    let Some((table, qualifier)) = parts.split_last() else {
        return String::new();
    };
    match qualifier.last() {
        None => table.clone(),
        Some(schema) if schema.eq_ignore_ascii_case(super::DEFAULT_SCHEMA) => table.clone(),
        // A schema the database holds: its tables are held under the qualified
        // name. One it does not hold is left as the caller wrote it, so the
        // engine reports the relation it cannot find.
        Some(schema) if holds_schema(schemas, schema) => format!("{schema}.{table}"),
        Some(_) => parts.join("."),
    }
}

/// The ENGINE name a statement's name parts address on the connection they are
/// read for: one of that connection's temporary relations when the name is a
/// bare one it holds or one it qualified with its own temporary schema, and
/// otherwise the schema's table ([`engine_table_name`]).
pub fn relation_name(parts: &[String], schemas: &SchemaSet, temp: &TempRelations<'_>) -> String {
    match parts {
        [name] if temp.holds(name) => temp.engine_name(name),
        [qualifier, name] if qualifier.eq_ignore_ascii_case(TEMP_QUALIFIER) => {
            temp.engine_name(name)
        }
        _ => engine_table_name(parts, schemas),
    }
}

/// Whether the database holds a schema of this name.
pub fn holds_schema(schemas: &SchemaSet, name: &str) -> bool {
    schemas
        .iter()
        .any(|schema| schema.eq_ignore_ascii_case(name))
}

/// The rewrites that re-spell one statement for the engine.
struct Rewriter<'a> {
    /// The database's catalogue of schemas.
    schemas: &'a SchemaSet,
    /// The temporary relations of the connection the statement runs on.
    temp: TempRelations<'a>,
    /// Whether anything was really rewritten, which is what decides whether the
    /// statement is re-rendered at all.
    changed: bool,
}

impl Rewriter<'_> {
    /// Merge a qualified name's leading schema into the ENGINE name the
    /// schema's tables are held under, or answer `None` for a name this
    /// mapping leaves alone.
    fn merged(&self, schema: &Ident, table: &Ident) -> Option<Ident> {
        // The connection's own temporary schema: its relations are held under
        // the connection's names, whether a statement qualifies them or not.
        if schema.value.eq_ignore_ascii_case(TEMP_QUALIFIER) {
            return Some(Ident::with_quote('"', self.temp.engine_name(&table.value)));
        }
        if schema.value.eq_ignore_ascii_case(super::DEFAULT_SCHEMA) {
            return Some(table.clone());
        }
        if !holds_schema(self.schemas, &schema.value) {
            return None;
        }
        // The engine name carries a dot, so it is a quoted identifier: unquoted
        // it would address a schema the engine does not have.
        Some(Ident::with_quote(
            '"',
            format!("{}.{}", schema.value, table.value),
        ))
    }

    /// The ENGINE name an UNQUALIFIED relation name addresses, when it is one
    /// of this connection's temporary relations: the search path resolves a
    /// temporary relation ahead of a permanent one of the same name, so a name
    /// the connection holds temporarily is that relation.
    fn temporary(&self, name: &Ident) -> Option<Ident> {
        self.temp
            .holds(&name.value)
            .then(|| Ident::with_quote('"', self.temp.engine_name(&name.value)))
    }
}

impl VisitorMut for Rewriter<'_> {
    type Break = ();

    /// A table name: `<schema>.<table>` is the schema's table, and a bare name
    /// the connection holds temporarily is that temporary relation.
    fn pre_visit_relation(&mut self, relation: &mut ObjectName) -> ControlFlow<Self::Break> {
        if relation.0.len() == 1 {
            if let Some(name) = relation.0[0].as_ident().cloned() {
                if let Some(temporary) = self.temporary(&name) {
                    relation.0 = vec![ObjectNamePart::Identifier(temporary)];
                    self.changed = true;
                }
            }
        }
        if relation.0.len() == 2 {
            let schema = relation.0[0].as_ident().cloned();
            let table = relation.0[1].as_ident().cloned();
            if let (Some(schema), Some(table)) = (schema, table) {
                if let Some(merged) = self.merged(&schema, &table) {
                    relation.0 = vec![ObjectNamePart::Identifier(merged)];
                    self.changed = true;
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        match expr {
            // `<expr>::<type>` is the same cast the standard spells
            // `CAST(<expr> AS <type>)`, which is what the engine reads.
            Expr::Cast { kind, .. } if *kind == CastKind::DoubleColon => {
                *kind = CastKind::Cast;
                self.changed = true;
            }
            // `<relation>.<column>`, where the relation is one this connection
            // holds temporarily: the column belongs to the relation's engine
            // name. A query that aliases another relation to that same name is
            // the one case this reads as the temporary relation, which is the
            // same choice the search path makes for the unqualified name.
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                if let Some(temporary) = self.temporary(&parts[0]) {
                    parts[0] = temporary;
                    self.changed = true;
                }
            }
            // `<schema>.<table>.<column>`: the leading schema belongs to the
            // table's engine name, and the column stays the column.
            Expr::CompoundIdentifier(parts) if parts.len() == 3 => {
                if let Some(merged) = self.merged(&parts[0], &parts[1]) {
                    let column = parts[2].clone();
                    *parts = vec![merged, column];
                    self.changed = true;
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// The statement's tokens, with the whitespace, the comments and the trailing
/// statement terminators dropped — what the recognisers below read.
///
/// Tokenised by the DIALECT's own tokenizer, so a keyword inside a string
/// literal or a comment is part of that literal or comment and starts no
/// statement. `None` for a statement the tokenizer cannot read at all, which is
/// one the engine reports rather than this module.
fn significant_tokens(sql: &str) -> Option<Vec<Token>> {
    let dialect = RedshiftSqlDialect {};
    let tokens = Tokenizer::new(&dialect, sql).tokenize().ok()?;
    let mut kept: Vec<Token> = tokens
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect();
    while matches!(kept.last(), Some(Token::SemiColon)) {
        kept.pop();
    }
    Some(kept)
}

/// The statements the text of ONE submission carries, in order and as the
/// caller wrote each of them.
///
/// A data warehouse's wire protocol runs a text that holds SEVERAL statements
/// separated by `;` as that sequence of statements — which is how a caller opens
/// a transaction and takes a lock in one submission (`BEGIN; LOCK TABLE …;`) —
/// so the text is cut into those statements here and each one is read and run on
/// its own.
///
/// The cuts are the SEMICOLON TOKENS the dialect's tokenizer reports, so a
/// semicolon inside a string literal, a quoted identifier or a comment is part
/// of that literal, identifier or comment and cuts nothing. Each piece is a
/// slice of the submitted text, byte for byte: nothing is re-rendered. A text
/// the tokenizer cannot read at all is ONE statement, which is what the engine
/// then reports on.
///
/// ONE forward pass over the text, whatever it carries: the tokenizer's
/// locations are non-decreasing, so [`Cursor`] walks the text once for all of
/// them together rather than once per separator — a text of nothing but
/// separators costs the length of that text and not its square. Cutting also
/// stops once the submission is past the statements one may carry
/// ([`super::MAX_STATEMENTS_PER_SUBMISSION`], which `run_sql` then refuses it
/// for), so neither the work nor the result grows past that bound, and a text
/// longer than the engine accepts is not read here at all
/// ([`within_readable_length`]).
pub fn split_statements(sql: &str) -> Vec<&str> {
    if !within_readable_length(sql) {
        return vec![sql];
    }
    let dialect = RedshiftSqlDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, sql).tokenize_with_location() else {
        return vec![sql];
    };
    let mut cursor = Cursor::new(sql);
    let mut statements: Vec<&str> = Vec::new();
    let mut start = 0_usize;
    for token in &tokens {
        if !matches!(token.token, Token::SemiColon) {
            continue;
        }
        // One statement past the bound is already past it, so nothing further
        // is cut: the submission is refused for its count either way.
        if statements.len() > super::MAX_STATEMENTS_PER_SUBMISSION {
            return statements;
        }
        let Some(cut) = cursor.offset_of(token.span.start.line, token.span.start.column) else {
            return vec![sql];
        };
        if cut < start || cut >= sql.len() || !sql.is_char_boundary(cut) {
            return vec![sql];
        }
        let piece = &sql[start..cut];
        if !piece.trim().is_empty() {
            statements.push(piece);
        }
        start = cut + 1;
    }
    let tail = &sql[start..];
    if !tail.trim().is_empty() {
        statements.push(tail);
    }
    if statements.is_empty() {
        // Nothing but separators and whitespace: there is one statement here,
        // the empty one, and the engine is what reports on it.
        return vec![sql];
    }
    statements
}

/// A position in a text, moved forward to the tokenizer's locations — a 1-based
/// line and a 1-based COLUMN counted in characters — to read their byte offsets.
///
/// Forward only, because the locations a token stream reports are
/// non-decreasing: the whole walk costs ONE pass over the text however many
/// locations are asked for, which is what keeps cutting a submission's text
/// linear in that text.
struct Cursor<'a> {
    /// The text being walked.
    text: &'a str,
    /// The byte offset the cursor is at.
    offset: usize,
    /// The 1-based line that offset is on.
    line: u64,
    /// The 1-based column, in characters, that offset is at.
    column: u64,
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of `text`, which is line 1, column 1 — where the
    /// tokenizer starts counting.
    fn new(text: &'a str) -> Self {
        Self {
            text,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    /// The byte offset of `line`:`column`, leaving the cursor there.
    ///
    /// `None` for a position BEHIND the cursor or past the end of the text,
    /// neither of which a tokenizer reports for a text it has read — and for
    /// which the caller falls back to treating the text as one statement.
    fn offset_of(&mut self, line: u64, column: u64) -> Option<usize> {
        if (line, column) < (self.line, self.column) {
            return None;
        }
        while (self.line, self.column) < (line, column) {
            let character = self.text.get(self.offset..)?.chars().next()?;
            self.offset += character.len_utf8();
            if character == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
        Some(self.offset)
    }
}

/// The statements of this dialect the PARSER does not model: `LOCK`, and
/// `ABORT`, which Amazon Redshift documents as a synonym of `ROLLBACK` ("ABORT
/// [ WORK | TRANSACTION ]"). They are read from the token stream because there
/// is no parsed form of them to read; everything else is read from the parse.
fn unparsed_statement(
    tokens: &[Token],
    schemas: &SchemaSet,
    temp: &TempRelations<'_>,
) -> Option<Translated> {
    if is_keyword(tokens.first()?, "lock") {
        return lock(tokens, schemas, temp);
    }
    if is_keyword(tokens.first()?, "abort") {
        let clauses_only = tokens[1..]
            .iter()
            .all(|token| is_keyword(token, "work") || is_keyword(token, "transaction"));
        return clauses_only.then_some(Translated::Transaction(TransactionControl::Rollback));
    }
    None
}

/// The statements the engine itself has no notion of, read from the PARSED
/// statement: the ones about a CATALOGUE (the schemas of a database, the
/// databases of a warehouse) and the transaction controls a data warehouse
/// answers with a warning rather than an error. `None` for everything else,
/// which is the engine's to run.
fn catalogue_statement(statement: &Statement) -> Option<Translated> {
    match statement {
        Statement::CreateSchema {
            schema_name,
            if_not_exists,
            ..
        } => Some(Translated::CreateSchema {
            // The forms that name an AUTHORIZATION instead of a schema name
            // name a schema after a database user this emulator does not hold,
            // so they are left to the engine.
            name: match schema_name {
                SchemaName::Simple(name) => object_name(name)?,
                _ => return None,
            },
            if_not_exists: *if_not_exists,
        }),
        Statement::Drop {
            object_type: ObjectType::Schema,
            if_exists,
            names,
            cascade,
            ..
        } => Some(Translated::DropSchema {
            names: names.iter().filter_map(object_name).collect(),
            if_exists: *if_exists,
            cascade: *cascade,
        }),
        Statement::CreateDatabase { db_name, .. } => Some(Translated::CreateDatabase {
            name: object_name(db_name)?,
        }),
        // `BEGIN`/`START TRANSACTION`, with whatever isolation level or
        // read/write mode it names: the engine's transactions are
        // one-writer-serializable, so none of those clauses names anything it
        // does differently.
        Statement::StartTransaction { .. } => {
            Some(Translated::Transaction(TransactionControl::Begin))
        }
        Statement::Commit { .. } => Some(Translated::Transaction(TransactionControl::Commit)),
        // A `ROLLBACK TO SAVEPOINT` is not a transaction-wide control: the
        // engine has savepoints, so it runs it.
        Statement::Rollback { savepoint: None, .. } => {
            Some(Translated::Transaction(TransactionControl::Rollback))
        }
        _ => None,
    }
}

/// The name a parsed object name carries, as one string: the LAST part, since a
/// schema or a database is named by itself.
fn object_name(name: &ObjectName) -> Option<String> {
    name.0
        .last()
        .and_then(ObjectNamePart::as_ident)
        .map(|ident| ident.value.clone())
}

/// `LOCK [TABLE] <name> [, …] [IN <mode> MODE] [NOWAIT]`.
///
/// Amazon Redshift documents the statement as `LOCK [TABLE] table_name [, …]`
/// and takes an `ACCESS EXCLUSIVE` lock; the PostgreSQL form it is derived from
/// names the mode, so a named mode is read and has to be one of the modes that
/// grammar declares ([`LOCK_MODES`]).
fn lock(tokens: &[Token], schemas: &SchemaSet, temp: &TempRelations<'_>) -> Option<Translated> {
    let mut rest = &tokens[1..];
    if is_keyword(rest.first()?, "table") {
        rest = &rest[1..];
    }
    let (names, mut rest) = name_list(rest)?;
    // "LOCK … takes an ACCESS EXCLUSIVE lock": the mode a statement that names
    // none asks for.
    let mut mode = LockMode::AccessExclusive;
    if rest.first().is_some_and(|token| is_keyword(token, "in")) {
        let (named, tail) = lock_mode(&rest[1..])?;
        mode = named;
        rest = tail;
    }
    if rest.first().is_some_and(|token| is_keyword(token, "nowait")) {
        rest = &rest[1..];
    }
    if !rest.is_empty() {
        return None;
    }
    Some(Translated::Lock {
        tables: names
            .iter()
            .map(|parts| relation_name(parts, schemas, temp))
            .collect(),
        mode,
    })
}

/// The lock modes the `LOCK` grammar declares, each followed by the `MODE`
/// keyword. Source: the PostgreSQL *LOCK* statement's explicit locking modes,
/// which Amazon Redshift's `LOCK` is derived from.
const LOCK_MODES: &[(&[&str], LockMode)] = &[
    (&["access", "share"], LockMode::AccessShare),
    (&["row", "share"], LockMode::RowShare),
    (&["row", "exclusive"], LockMode::RowExclusive),
    (
        &["share", "update", "exclusive"],
        LockMode::ShareUpdateExclusive,
    ),
    (&["share"], LockMode::Share),
    (
        &["share", "row", "exclusive"],
        LockMode::ShareRowExclusive,
    ),
    (&["exclusive"], LockMode::Exclusive),
    (&["access", "exclusive"], LockMode::AccessExclusive),
];

/// Read one `<mode> MODE` clause, and report the mode and what follows it.
/// `None` for a mode the grammar does not declare.
fn lock_mode(tokens: &[Token]) -> Option<(LockMode, &[Token])> {
    // Longest first, so `SHARE ROW EXCLUSIVE MODE` is not read as `SHARE MODE`
    // with tokens left over.
    let mut modes: Vec<&(&[&str], LockMode)> = LOCK_MODES.iter().collect();
    modes.sort_by_key(|(words, _)| std::cmp::Reverse(words.len()));
    for (words, mode) in modes {
        if tokens.len() <= words.len() {
            continue;
        }
        let matched = words
            .iter()
            .zip(tokens)
            .all(|(word, token)| is_keyword(token, word));
        if matched && is_keyword(&tokens[words.len()], "mode") {
            return Some((*mode, &tokens[words.len() + 1..]));
        }
    }
    None
}

/// Read a comma-separated list of (possibly qualified) names, and report what
/// follows it.
fn name_list(tokens: &[Token]) -> Option<(Vec<Vec<String>>, &[Token])> {
    let mut names: Vec<Vec<String>> = Vec::new();
    let mut rest = tokens;
    loop {
        let (parts, tail) = qualified_name(rest)?;
        names.push(parts);
        rest = tail;
        match rest.first() {
            Some(Token::Comma) => rest = &rest[1..],
            _ => return Some((names, rest)),
        }
    }
}

/// Read one name — `<part>[.<part>…]` — and report what follows it.
fn qualified_name(tokens: &[Token]) -> Option<(Vec<String>, &[Token])> {
    let mut parts = vec![identifier(tokens.first()?)?];
    let mut rest = &tokens[1..];
    while matches!(rest.first(), Some(Token::Period)) {
        parts.push(identifier(rest.get(1)?)?);
        rest = &rest[2..];
    }
    Some((parts, rest))
}

/// Whether `token` is the unquoted keyword `word`. A quoted word is an
/// identifier that happens to be spelled like a keyword, never the keyword.
fn is_keyword(token: &Token, word: &str) -> bool {
    matches!(token, Token::Word(found)
        if found.quote_style.is_none() && found.value.eq_ignore_ascii_case(word))
}

/// The name `token` carries, quoted or not. `None` for a token that is not a
/// name at all.
fn identifier(token: &Token) -> Option<String> {
    match token {
        Token::Word(word) => Some(word.value.clone()),
        _ => None,
    }
}

/// The lock ONE statement needs before it can run: the tables it references and
/// the mode running it takes on them.
///
/// `None` for a statement that locks nothing — a transaction control, a
/// statement about a catalogue, or a text this module does not read. The names
/// are resolved for the connection the statement runs on, so a reference to one
/// of its own temporary relations never meets another connection's lock. A
/// database's schema catalogue does not change a resolved name — a schema the
/// database holds and one it does not both resolve to `<schema>.<table>` — so
/// this reads a statement without it.
pub fn lock_request(sql: &str, temp: &TempRelations<'_>) -> Option<LockRequest> {
    if !within_readable_length(sql) {
        return None;
    }
    let schemas = SchemaSet::new();
    if let Some(tokens) = significant_tokens(sql) {
        match unparsed_statement(&tokens, &schemas, temp) {
            Some(Translated::Lock { tables, mode }) => return Some(LockRequest { tables, mode }),
            Some(_) => return None,
            None => {}
        }
    }
    let dialect = RedshiftSqlDialect {};
    let statements = Parser::parse_sql(&dialect, sql).ok()?;
    let [statement] = statements.as_slice() else {
        return None;
    };
    if catalogue_statement(statement).is_some() {
        return None;
    }
    let mode = implied_mode(statement);
    let mut relations = Relations {
        schemas: &schemas,
        temp: *temp,
        tables: Vec::new(),
    };
    let _walked = statement.visit(&mut relations);
    Some(LockRequest {
        tables: relations.tables,
        mode,
    })
}

/// The lock mode running a statement takes on the tables it references.
///
/// Source: the "Explicit Locking" chapter of the PostgreSQL documentation,
/// which Amazon Redshift's locking is derived from — it states which mode each
/// command acquires: a read takes `ACCESS SHARE`, `INSERT`/`UPDATE`/`DELETE`
/// take `ROW EXCLUSIVE`, `CREATE INDEX` takes `SHARE`, `ANALYZE` takes `SHARE
/// UPDATE EXCLUSIVE`, and `DROP`, `TRUNCATE` and most `ALTER TABLE` forms take
/// `ACCESS EXCLUSIVE`. A statement this list does not name is read as the
/// weakest mode, which is what a statement that only reads takes.
fn implied_mode(statement: &Statement) -> LockMode {
    match statement {
        Statement::Insert { .. }
        | Statement::Update { .. }
        | Statement::Delete { .. }
        | Statement::Merge { .. } => LockMode::RowExclusive,
        Statement::CreateIndex { .. } => LockMode::Share,
        Statement::Analyze { .. } => LockMode::ShareUpdateExclusive,
        Statement::CreateTable { .. }
        | Statement::CreateView { .. }
        | Statement::Drop { .. }
        | Statement::Truncate { .. }
        | Statement::AlterTable { .. } => LockMode::AccessExclusive,
        _ => LockMode::AccessShare,
    }
}

/// The ENGINE names of the relations one statement references, read off the
/// parsed statement rather than from its text.
struct Relations<'a> {
    schemas: &'a SchemaSet,
    temp: TempRelations<'a>,
    tables: Vec<String>,
}

impl Visitor for Relations<'_> {
    type Break = ();

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        let parts: Vec<String> = relation
            .0
            .iter()
            .filter_map(ObjectNamePart::as_ident)
            .map(|ident| ident.value.clone())
            .collect();
        if !parts.is_empty() {
            let name = relation_name(&parts, self.schemas, &self.temp);
            if !self.tables.contains(&name) {
                self.tables.push(name);
            }
        }
        ControlFlow::Continue(())
    }
}
