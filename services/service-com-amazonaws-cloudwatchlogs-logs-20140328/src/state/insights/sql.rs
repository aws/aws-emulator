// SPDX-License-Identifier: CC0-1.0

//! Running an OpenSearch SQL query — the `SELECT` `StartQuery` accepts for the `SQL` query
//! language — over the log events this service stores.
//!
//! The statement is PARSED, never matched: the `sqlparser` crate reads it, so what runs is
//! the structure of the SQL the caller wrote — its projection, its `WHERE` condition, its
//! grouping, its ordering and its bound — and a statement that is not SQL at all makes the
//! query `Failed` rather than being read as something it is not.
//!
//! The log groups come from the statement's own `FROM` clause: for the SQL language "you
//! specify the log group names inside the querystring instead of here"
//! (`StartQuery$logGroupIdentifiers`), so the tables a `SELECT` names are the log groups it
//! reads and they override whatever log groups the request named.
//!
//! Each log event is a ROW whose columns are the fields of its record
//! ([`super::Record`]) — the generated `@`-prefixed fields and the keys discovered in a JSON
//! message — so `SELECT \`@message\` FROM \`/my/group\` WHERE level = 'ERROR'` selects over
//! exactly what the store holds.
//!
//! WHAT BOUNDS IT: the statement is `@length(0..10000)` (`QueryString`), the rows come from a
//! bounded scan ([`super::MAX_SCANNED_EVENTS`]), the expression tree is bounded in depth by
//! [`super::MAX_EXPRESSION_DEPTH`] on the way in, and every clause is evaluated by one pass
//! over the rows in hand.

use std::collections::BTreeMap;

use ::sqlparser::ast::{
    BinaryOperator, DuplicateTreatment, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    GroupByExpr, LimitClause, OrderByKind, Query, Select, SelectItem, SetExpr, Statement,
    TableFactor, UnaryOperator,
};
use ::sqlparser::dialect::GenericDialect;
use ::sqlparser::parser::Parser;

use super::expression::{self, AggregateFunction, Value};
use super::{
    Budget, Execution, ExecutionFailure, GroupSelector, Record, Scan, FIELD_PTR, MAX_RESULT_ROWS,
};
use crate::state::QueryRecord;

/// Plan a SQL query: parse the statement, which is what decides whether it can run at all.
pub(super) fn plan(query_string: &str) -> Result<super::Plan, ExecutionFailure> {
    let statements = Parser::parse_sql(&GenericDialect {}, query_string)
        .map_err(|error| ExecutionFailure::new(error.to_string()))?;
    // One `SELECT`, and nothing else: a query of log events reads them.
    let [Statement::Query(parsed)] = statements.as_slice() else {
        return Err(ExecutionFailure::new(
            "a SQL query of log events is one SELECT statement",
        ));
    };
    if parsed.with.is_some() {
        return Err(ExecutionFailure::new(
            "a SQL query of log events carries no WITH clause",
        ));
    }
    if !matches!(parsed.body.as_ref(), SetExpr::Select(_)) {
        return Err(ExecutionFailure::new(
            "a SQL query of log events is a SELECT rather than a set operation",
        ));
    }
    // Every clause and every expression the statement carries is checked HERE, once, against
    // what this evaluation reads: a statement naming something it does not read is reported
    // as the execution that produced nothing, never answered with rows computed without it.
    check_statement(parsed)?;
    Ok(super::Plan::Sql(parsed.clone()))
}

/// Check that every clause and expression of the statement is one this evaluation reads.
fn check_statement(parsed: &Query) -> Result<(), ExecutionFailure> {
    let SetExpr::Select(select) = parsed.body.as_ref() else {
        return Ok(());
    };
    if select.having.is_some() {
        // "HAVING condition" filters the groups the statement built, over the columns its
        // projection names — which is where an aggregate's value stands once the groups are
        // folded. A condition carrying an aggregate function expression of its OWN is not a
        // condition the language admits ([`aggregate_expression_in_having`]), and a statement
        // carrying one is refused where it is compiled.
        if let Some(predicate) = select
            .having
            .as_ref()
            .and_then(aggregate_expression_in_condition)
        {
            return Err(ExecutionFailure::new(format!(
                "{NO_AGGREGATE_IN_FILTER_PREFIX}{predicate}"
            )));
        }
    }
    if let Some(condition) = select.having.as_ref() {
        check_expression(condition, 0)?;
    }
    if select.distinct.is_some() {
        return Err(ExecutionFailure::new(
            "a DISTINCT selection has no execution in this service",
        ));
    }
    from_clause_selectors(select)?;
    for column in read_projection(select)? {
        match &column.source {
            Source::Everything => {}
            Source::Value(expression) => check_expression(expression, 0)?,
            Source::Aggregate { over, .. } => {
                if let Some(expression) = over {
                    check_expression(expression, 0)?;
                }
            }
        }
    }
    if let Some(condition) = select.selection.as_ref() {
        check_expression(condition, 0)?;
    }
    for expression in read_grouping(select)? {
        check_expression(&expression, 0)?;
    }
    if let Some(order_by) = parsed.order_by.as_ref() {
        let OrderByKind::Expressions(expressions) = &order_by.kind else {
            return Err(ExecutionFailure::new(
                "a SQL query of log events orders by columns",
            ));
        };
        for expression in expressions {
            check_expression(&expression.expr, 0)?;
        }
    }
    read_limit(parsed)?;
    Ok(())
}

/// Check one expression against what [`evaluate`] reads: the same forms, the same operators
/// and, for a call, the aggregates this service computes.
///
/// A form this evaluation does not read is REFUSED here rather than evaluated to nothing:
/// a condition that quietly decided nothing would select no rows and report success, which
/// is a plausible wrong answer for a statement this service did not run.
fn check_expression(expression: &Expr, depth: usize) -> Result<(), ExecutionFailure> {
    if depth > super::MAX_EXPRESSION_DEPTH {
        return Err(ExecutionFailure::new(
            "the expression nests deeper than this service evaluates",
        ));
    }
    let depth = depth + 1;
    let mut children: Vec<&Expr> = Vec::new();
    match expression {
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) | Expr::Value(_) => {}
        Expr::Nested(inner)
        | Expr::IsNull(inner)
        | Expr::IsNotNull(inner)
        | Expr::IsTrue(inner)
        | Expr::IsFalse(inner) => children.push(inner),
        Expr::UnaryOp { op, expr } => {
            if !matches!(
                op,
                UnaryOperator::Not | UnaryOperator::Minus | UnaryOperator::Plus
            ) {
                return Err(ExecutionFailure::new(format!(
                    "the operator {op} has no execution in this service"
                )));
            }
            children.push(expr);
        }
        Expr::Between {
            expr, low, high, ..
        } => children.extend([expr.as_ref(), low.as_ref(), high.as_ref()]),
        Expr::InList { expr, list, .. } => {
            children.push(expr);
            children.extend(list.iter());
        }
        Expr::Like {
            expr, pattern, ..
        }
        | Expr::ILike {
            expr, pattern, ..
        } => children.extend([expr.as_ref(), pattern.as_ref()]),
        Expr::BinaryOp { left, op, right } => {
            if !matches!(
                op,
                BinaryOperator::And
                    | BinaryOperator::Or
                    | BinaryOperator::Eq
                    | BinaryOperator::NotEq
                    | BinaryOperator::Lt
                    | BinaryOperator::LtEq
                    | BinaryOperator::Gt
                    | BinaryOperator::GtEq
                    | BinaryOperator::Plus
                    | BinaryOperator::Minus
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo
                    | BinaryOperator::StringConcat
            ) {
                return Err(ExecutionFailure::new(format!(
                    "the operator {op} has no execution in this service"
                )));
            }
            children.extend([left.as_ref(), right.as_ref()]);
        }
        // A call is an aggregate, which `read_aggregate` reads and the grouping computes;
        // anything else — a scalar function, a window, a subquery, a cast — is not a form
        // this evaluation reads.
        Expr::Function(_) => {
            let (_, over) = read_aggregate(expression)?.ok_or_else(|| {
                ExecutionFailure::new(format!(
                    "the function {expression} has no execution in this service"
                ))
            })?;
            if let Some(inner) = over {
                check_expression(&inner, depth)?;
            }
        }
        other => {
            return Err(ExecutionFailure::new(format!(
                "the expression {other} has no execution in this service"
            )))
        }
    }
    for child in children {
        check_expression(child, depth)?;
    }
    Ok(())
}

/// The log groups a planned statement's `FROM` clause names.
pub(super) fn from_clause_groups(parsed: &Query) -> Result<Vec<GroupSelector>, ExecutionFailure> {
    let SetExpr::Select(select) = parsed.body.as_ref() else {
        return Err(ExecutionFailure::new(
            "a SQL query of log events is a SELECT rather than a set operation",
        ));
    };
    from_clause_selectors(select)
}

/// What the SQL statement `query_string` says about the log groups it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementSources {
    /// The statement's own source clause, as the selectors it names.
    Named(Vec<GroupSelector>),
    /// The query string names no log groups this service can resolve — a `SELECT` with no
    /// `FROM` clause, and a query string that is no statement of this language at all.
    Unnamed,
    /// The query string IS a statement of this language — it opens with one of the keywords a
    /// statement of it opens with — and does NOT compile, with the compiler's own account of
    /// what it stopped on.
    Malformed(String),
}

/// The log groups the SQL statement `query_string` names, read from the statement itself.
///
/// This is what the SQL language names INSTEAD of the request's members ("you specify the log
/// group names inside the querystring instead of here", `StartQuery$logGroupIdentifiers`), so
/// the caller that ADMITS a request reads them here — through the same parser that runs the
/// statement, so the groups a request is admitted over are the groups its run will read.
///
/// The two ways a query string can name none are kept apart, because they are different
/// faults: a string that is no statement of this language names no log groups and nothing
/// else can be said about it, while a statement OF this language that does not compile is a
/// malformed query, which is what this operation models `MalformedQueryException` for.
pub fn statement_sources(query_string: &str) -> StatementSources {
    let parsed = match Parser::parse_sql(&GenericDialect {}, query_string) {
        Ok(statements) => statements,
        Err(error) => {
            return if opens_a_statement(query_string) {
                StatementSources::Malformed(compiler_detail(&error.to_string()))
            } else {
                StatementSources::Unnamed
            }
        }
    };
    let [Statement::Query(parsed)] = parsed.as_slice() else {
        return if opens_a_statement(query_string) {
            StatementSources::Malformed(
                "a SQL query of log events is one SELECT statement".to_string(),
            )
        } else {
            StatementSources::Unnamed
        };
    };
    match from_clause_groups(parsed) {
        Ok(selectors) => StatementSources::Named(selectors),
        // A statement that compiles and names no source clause names no log groups.
        Err(_) => StatementSources::Unnamed,
    }
}

/// Whether `query_string` opens a statement of this query language: its first word is one of
/// the keywords such a statement opens with, whatever case it is written in.
fn opens_a_statement(query_string: &str) -> bool {
    query_string
        .split_whitespace()
        .next()
        .is_some_and(|word| {
            STATEMENT_KEYWORDS
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword))
        })
}

/// The keywords a statement of this query language opens with.
const STATEMENT_KEYWORDS: [&str; 2] = ["select", "with"];

/// The compiler's account of a statement that does not compile, as this service states one:
/// the parser's own diagnostic, without the crate-level prefix it writes in front of it.
fn compiler_detail(error: &str) -> String {
    error
        .strip_prefix(PARSER_ERROR_PREFIX)
        .unwrap_or(error)
        .to_string()
}

/// The prefix `sqlparser` writes in front of its diagnostics, which names the parser rather
/// than the fault.
const PARSER_ERROR_PREFIX: &str = "sql parser error: ";

/// Where a statement's `HAVING` clause filters on something the language does not admit: the
/// predicate to state the fault with, and where in the submitted query string that condition
/// stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HavingFault {
    /// The condition as the pipeline the statement TRANSLATES to states it.
    pub predicate: String,
    /// The first character of the condition in the query string.
    pub start_char_offset: usize,
    /// One past its last character.
    pub end_char_offset: usize,
}

/// The aggregate function expression the SQL statement `query_string` filters its groups on,
/// if it filters them on one — the fault that makes such a statement one the language cannot
/// compile.
///
/// The predicate is answered as the pipeline the statement TRANSLATES to states it: the
/// `filter` stage the `HAVING` clause becomes, written as the command and the condition with
/// no space between them, and with the name of every function call lowercased
/// ([`translated_predicate`]). A query string that is no `SELECT`, or one whose `HAVING`
/// filters on the columns the projection names, carries no such fault.
pub fn aggregate_expression_in_having(query_string: &str) -> Option<HavingFault> {
    let statements = Parser::parse_sql(&GenericDialect {}, query_string).ok()?;
    let [Statement::Query(parsed)] = statements.as_slice() else {
        return None;
    };
    let SetExpr::Select(select) = parsed.body.as_ref() else {
        return None;
    };
    let condition = select.having.as_ref()?;
    let predicate = aggregate_expression_in_condition(condition)?;
    let (start_char_offset, end_char_offset) =
        having_condition_span(query_string).unwrap_or((0, query_string.chars().count()));
    Some(HavingFault {
        predicate,
        start_char_offset,
        end_char_offset,
    })
}

/// Where the condition of `query_string`'s `HAVING` clause stands in it, as CHARACTER offsets
/// — what `QueryCompileErrorLocation` carries, so a client can point at the text at fault.
///
/// The clause is found by its keyword and runs to the clause that follows it, which in a
/// `SELECT` of log events is an ordering or a bound; `None` when the query string carries no
/// such keyword, which a statement whose parser found a `HAVING` clause does not happen to be.
fn having_condition_span(query_string: &str) -> Option<(usize, usize)> {
    let characters: Vec<char> = query_string.chars().collect();
    let mut words: Vec<(String, usize, usize)> = Vec::new();
    let mut index = 0usize;
    while index < characters.len() {
        if !characters[index].is_alphanumeric() && characters[index] != '_' {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len()
            && (characters[index].is_alphanumeric() || characters[index] == '_')
        {
            index += 1;
        }
        words.push((
            characters[start..index].iter().collect::<String>().to_lowercase(),
            start,
            index,
        ));
    }
    let at = words.iter().position(|(word, ..)| word == HAVING_KEYWORD)?;
    let start = words.get(at + 1).map_or(words[at].2, |(_, from, _)| *from);
    let end = words
        .iter()
        .skip(at + 1)
        .find(|(word, ..)| CLAUSES_AFTER_HAVING.contains(&word.as_str()))
        .map_or(characters.len(), |(_, from, _)| *from);
    Some((start, end.max(start)))
}

/// The keyword that opens the clause filtering a statement's groups.
const HAVING_KEYWORD: &str = "having";

/// The keywords that can open the clause AFTER it, which is where its condition ends.
const CLAUSES_AFTER_HAVING: [&str; 2] = ["order", "limit"];

/// The fixed text a statement filtering its groups on an aggregate function expression is
/// refused with, the translated predicate following it.
pub const NO_AGGREGATE_IN_FILTER_PREFIX: &str = "No aggregation function expression allowed: ";

/// Whether `condition` calls an aggregate anywhere in it, and the translated predicate to
/// state it with.
fn aggregate_expression_in_condition(condition: &Expr) -> Option<String> {
    if !calls_an_aggregate(condition, 0) {
        return None;
    }
    Some(translated_predicate(condition))
}

/// Whether `expression` calls an aggregate function anywhere inside it.
///
/// Bounded by the same depth every other walk of a caller-written expression is bounded by
/// ([`super::MAX_EXPRESSION_DEPTH`]): an expression nesting deeper than that is not read, and
/// the compile check that follows refuses it for exactly that.
fn calls_an_aggregate(expression: &Expr, depth: usize) -> bool {
    if depth > super::MAX_EXPRESSION_DEPTH {
        return false;
    }
    let depth = depth + 1;
    if matches!(read_aggregate(expression), Ok(Some(_))) {
        return true;
    }
    match expression {
        Expr::Nested(inner)
        | Expr::IsNull(inner)
        | Expr::IsNotNull(inner)
        | Expr::IsTrue(inner)
        | Expr::IsFalse(inner)
        | Expr::UnaryOp { expr: inner, .. } => calls_an_aggregate(inner, depth),
        Expr::BinaryOp { left, right, .. } => {
            calls_an_aggregate(left, depth) || calls_an_aggregate(right, depth)
        }
        Expr::Between {
            expr, low, high, ..
        } => [expr.as_ref(), low.as_ref(), high.as_ref()]
            .into_iter()
            .any(|inner| calls_an_aggregate(inner, depth)),
        Expr::InList { expr, list, .. } => {
            calls_an_aggregate(expr, depth)
                || list.iter().any(|inner| calls_an_aggregate(inner, depth))
        }
        Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
            calls_an_aggregate(expr, depth) || calls_an_aggregate(pattern, depth)
        }
        Expr::Function(function) => match &function.args {
            FunctionArguments::List(arguments) => arguments.args.iter().any(|argument| {
                matches!(argument, FunctionArg::Unnamed(FunctionArgExpr::Expr(inner))
                    if calls_an_aggregate(inner, depth))
            }),
            _ => false,
        },
        _ => false,
    }
}

/// `condition` as the `filter` stage of the pipeline a SQL statement translates to: the
/// command word, then the condition with its spacing removed and the name of every function
/// call lowercased, which is how that pipeline writes one.
fn translated_predicate(condition: &Expr) -> String {
    let written = condition.to_string();
    let characters: Vec<char> = written.chars().filter(|c| !c.is_whitespace()).collect();
    let mut predicate = String::with_capacity(characters.len() + TRANSLATED_FILTER.len());
    predicate.push_str(TRANSLATED_FILTER);
    let mut index = 0usize;
    while index < characters.len() {
        let start = index;
        while index < characters.len()
            && (characters[index].is_alphanumeric() || characters[index] == '_')
        {
            index += 1;
        }
        if index > start {
            let word: String = characters[start..index].iter().collect();
            // A word the arguments open right after is a call, and a call's name is written
            // lowercased; every other word is the caller's own text.
            if characters.get(index) == Some(&'(') {
                predicate.push_str(&word.to_lowercase());
            } else {
                predicate.push_str(&word);
            }
            continue;
        }
        predicate.push(characters[index]);
        index += 1;
    }
    predicate
}

/// The command the translated pipeline filters with.
const TRANSLATED_FILTER: &str = "filter";

/// Run a planned SQL statement over the records the scan read.
///
/// Reached with this service's state released ([`super::Prepared`]), so every clause is
/// evaluated over the records in hand and nothing else. The run spends the [`Budget`] and
/// yields between clauses and every [`RECORDS_BETWEEN_YIELDS`] records, so a cancelled or
/// timed-out request has somewhere to land and a statement that asks for more than the
/// budget is abandoned rather than run to the end.
pub(super) async fn run(
    parsed: &Query,
    scan: Scan,
    query: &QueryRecord,
    budget: &Budget,
) -> Execution {
    let evaluation = Evaluation {
        budget,
        patterns: std::sync::Mutex::new(BTreeMap::new()),
    };
    let scanned = scan.scanned;
    let bytes = scan.bytes;
    let groups = scan.groups;
    let outcome = evaluate_statement(parsed, scan, query, &evaluation).await;
    let failed = outcome
        .as_ref()
        .err()
        .map(|failure| failure.reason.clone())
        .or_else(|| budget.reason());
    match failed {
        Some(reason) => Execution {
            failed: Some(reason),
            records_scanned: scanned as f64,
            bytes_scanned: bytes as f64,
            log_groups_scanned: groups as f64,
            ..Execution::default()
        },
        None => outcome.unwrap_or_default(),
    }
}

/// How many records one clause works through between yields, so a long filter or projection
/// leaves the runtime a place to land a cancellation.
const RECORDS_BETWEEN_YIELDS: usize = 512;

/// What one statement is evaluated with: the run's budget, and the regular expressions its
/// `LIKE` patterns compiled to.
///
/// A pattern is compiled ONCE per distinct pattern — a `LIKE` in a `WHERE` clause is
/// evaluated once per record, and compiling its pattern per record would make the compile
/// cost a multiple of the records rather than of the statement.
struct Evaluation<'a> {
    /// What the run may still spend.
    budget: &'a Budget,
    /// Each `LIKE` pattern this statement carries, keyed by its text and whether it is
    /// matched without regard to case, as the expression it compiled to (`None` for a pattern
    /// that is not a pattern at all). Behind a lock a panicking run cannot poison for the
    /// next one, because a run yields as it goes and may resume on another thread.
    patterns: std::sync::Mutex<BTreeMap<(String, bool), Option<::regex::Regex>>>,
}

impl Evaluation<'_> {
    /// Whether `subject` matches the SQL `LIKE` pattern `pattern`, compiling that pattern
    /// once however many records are matched against it.
    ///
    /// A pattern operand need not be a literal: `a LIKE b` reads `b` out of each RECORD, so
    /// the patterns one statement matches against are the caller's DATA and there can be one
    /// per scanned event. What is RETAINED is therefore bounded two ways — at most
    /// [`MAX_CACHED_PATTERNS`] expressions are kept, each compiled under
    /// [`MAX_COMPILED_PATTERN_BYTES`], so the whole cache is bounded by a constant rather than
    /// by the data; and the pattern text each entry keeps is charged to the run
    /// ([`Budget::store`]) like every other value a run holds on to. Past either bound a
    /// pattern is compiled for the one match it is needed for and dropped.
    fn like_matches(&self, subject: &str, pattern: &str, insensitive: bool) -> bool {
        let key = (pattern.to_string(), insensitive);
        let mut patterns = self
            .patterns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(compiled) = patterns.get(&key) {
            return compiled
                .as_ref()
                .is_some_and(|compiled| compiled.is_match(subject));
        }
        let compiled = compile_like(pattern, insensitive);
        let matched = compiled
            .as_ref()
            .is_some_and(|compiled| compiled.is_match(subject));
        if patterns.len() < MAX_CACHED_PATTERNS && self.budget.store(pattern.len()) {
            patterns.insert(key, compiled);
        }
        matched
    }
}

/// THE EMULATOR'S OWN CEILING on how many compiled `LIKE` patterns one run keeps: a statement
/// matching against literals carries a handful of them, and a statement matching against a
/// FIELD has as many patterns as the scan has distinct values — which is data rather than
/// statement, so it is not what a cache may grow with.
const MAX_CACHED_PATTERNS: usize = 16;

/// Evaluate one planned statement over the scanned records.
async fn evaluate_statement(
    parsed: &Query,
    scan: Scan,
    query: &QueryRecord,
    evaluation: &Evaluation<'_>,
) -> Result<Execution, ExecutionFailure> {
    let SetExpr::Select(select) = parsed.body.as_ref() else {
        return Err(ExecutionFailure::new(
            "a SQL query of log events is a SELECT rather than a set operation",
        ));
    };

    // "WHERE condition": the rows the condition selects, evaluated over each record.
    let mut rows: Vec<Record> = match select.selection.as_ref() {
        None => scan.records,
        Some(condition) => {
            let mut selected: Vec<Record> = Vec::new();
            for (index, record) in scan.records.into_iter().enumerate() {
                if index > 0 && index.is_multiple_of(RECORDS_BETWEEN_YIELDS) {
                    ::tokio::task::yield_now().await;
                }
                if evaluate(evaluation, condition, &record, 0).truthy() {
                    selected.push(record);
                }
            }
            selected
        }
    };
    let matched = rows.len();

    let projection = read_projection(select)?;
    let grouping = read_grouping(select)?;
    let aggregating = !grouping.is_empty()
        || projection
            .iter()
            .any(|column| matches!(column.source, Source::Aggregate { .. }));
    let mut columns: Vec<String> = Vec::new();
    ::tokio::task::yield_now().await;
    if aggregating {
        let (grouped, names) = aggregate_rows(evaluation, &rows, &projection, &grouping)?;
        rows = grouped;
        columns = names;
    } else {
        // The columns the result carries, kept as a set beside the ordered list so a column
        // already named costs one lookup rather than one comparison per column named so far.
        let mut named: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for column in &projection {
            match &column.source {
                // `SELECT *` carries every field the selected rows name between them.
                Source::Everything => {
                    for (index, record) in rows.iter().enumerate() {
                        if index > 0 && index.is_multiple_of(RECORDS_BETWEEN_YIELDS) {
                            ::tokio::task::yield_now().await;
                        }
                        if !evaluation.budget.tick() {
                            break;
                        }
                        for field in record.values.keys() {
                            if named.contains(field) {
                                continue;
                            }
                            // The fields of the scanned events are the caller's, so how many
                            // columns one result may carry is bounded here.
                            if columns.len() >= MAX_RESULT_COLUMNS {
                                return Err(ExecutionFailure::new(format!(
                                    "a SQL query of log events reports at most \
                                     {MAX_RESULT_COLUMNS} columns"
                                )));
                            }
                            named.insert(field.clone());
                            columns.push(field.clone());
                        }
                    }
                }
                Source::Value(expression) => {
                    for (index, record) in rows.iter_mut().enumerate() {
                        if index > 0 && index.is_multiple_of(RECORDS_BETWEEN_YIELDS) {
                            ::tokio::task::yield_now().await;
                        }
                        let value = evaluate(evaluation, expression, record, 0);
                        if !matches!(value, Value::Null) {
                            let rendered = value.rendered();
                            if evaluation.budget.store(rendered.len()) {
                                record.set(&column.name, rendered);
                            }
                        }
                    }
                    if named.insert(column.name.clone()) {
                        columns.push(column.name.clone());
                    }
                }
                Source::Aggregate { .. } => {}
            }
        }
    }

    ::tokio::task::yield_now().await;
    // "HAVING condition": the groups the condition selects, read over each group's own row —
    // which carries the projection's columns, so an aggregate's value is there under the name
    // the projection gave it.
    if let Some(condition) = select.having.as_ref() {
        rows.retain(|row| evaluate(evaluation, condition, row, 0).truthy());
    }

    ::tokio::task::yield_now().await;
    order_rows(evaluation, parsed, &mut rows)?;

    // "LIMIT count" bounds the rows the statement answers, and the API's ceiling bounds it
    // further — but a start that named its own row bound REPLACES the statement's clause, the
    // way it replaces a pipeline's row-bounding command ([`super::requested_row_limit`]). The
    // clause is read either way, so a `LIMIT` this service cannot run is still refused.
    let clause = read_limit(parsed)?;
    let limit = super::requested_row_limit(query)
        .or(clause)
        .map_or(MAX_RESULT_ROWS, |rows| rows.min(MAX_RESULT_ROWS));
    rows.truncate(limit);

    // A row of log events carries the pointer to its record, as every other query language's
    // result does; a row of GROUPS is no single log record and carries none.
    if !aggregating && !columns.iter().any(|column| column == FIELD_PTR) {
        columns.push(FIELD_PTR.to_string());
    }

    // The rows as the field/value pairs of the columns the result carries: one row at a
    // time, so the product of the rows and the columns spends the run's budget and yields
    // like every other pass over the records.
    let mut rendered: Vec<Vec<(String, String)>> = Vec::with_capacity(rows.len());
    for (index, record) in rows.iter().enumerate() {
        if index > 0 && index.is_multiple_of(RECORDS_BETWEEN_YIELDS) {
            ::tokio::task::yield_now().await;
        }
        if !evaluation.budget.tick() {
            break;
        }
        rendered.push(
            columns
                .iter()
                .filter_map(|column| {
                    record
                        .get(column)
                        .map(|value| (column.clone(), value.to_string()))
                })
                .collect(),
        );
    }

    Ok(Execution {
        rows: rendered,
        // The result set holds the records its rows point at, so the `@ptr` a row carries
        // keeps addressing the record this run read (`Execution::records`). A row of GROUPS
        // points at no single log record.
        records: if aggregating {
            ::std::collections::BTreeMap::new()
        } else {
            rows.iter()
                .filter_map(|record| {
                    record
                        .get(FIELD_PTR)
                        .map(|pointer| (pointer.to_string(), record.clone()))
                })
                .collect()
        },
        records_matched: matched as f64,
        records_scanned: scan.scanned as f64,
        bytes_scanned: scan.bytes as f64,
        log_groups_scanned: scan.groups as f64,
        failed: None,
    })
}

/// THE EMULATOR'S OWN CEILING on how many columns one result may carry: a `SELECT *` reports
/// the fields the scanned events name, and those are the caller's.
const MAX_RESULT_COLUMNS: usize = 1_000;

/// The log groups the statement's `FROM` clause names.
fn from_clause_selectors(select: &Select) -> Result<Vec<GroupSelector>, ExecutionFailure> {
    let mut selectors: Vec<GroupSelector> = Vec::new();
    for table in &select.from {
        if !table.joins.is_empty() {
            return Err(ExecutionFailure::new(
                "a SQL query of log events joins no tables",
            ));
        }
        let TableFactor::Table { name, .. } = &table.relation else {
            return Err(ExecutionFailure::new(
                "a SQL query of log events reads log groups named in its FROM clause",
            ));
        };
        // A log group's name carries `/`, `-` and `.`, so a statement writes it as a quoted
        // or back-quoted identifier; the quoting is not part of the name.
        let named = name
            .0
            .iter()
            .map(|part| super::trim_identifier(&part.to_string()).to_string())
            .collect::<Vec<String>>()
            .join(".");
        if !named.is_empty() {
            // A table may name the log groups to read through the `logGroups(…)` selector the
            // source command of the pipelined languages takes, which selects by name prefix or
            // by identifier; a table that is a plain name names that one log group.
            selectors.extend(super::read_group_selectors(&named));
        }
    }
    if selectors.is_empty() {
        return Err(ExecutionFailure::new(
            "a SQL query of log events names the log groups to read in its FROM clause",
        ));
    }
    Ok(selectors)
}

/// One column a `SELECT` projects.
#[derive(Debug, Clone)]
struct Column {
    /// The name the result reports it under: its alias, or the text that computes it.
    name: String,
    /// What computes it.
    source: Source,
}

/// What one projected column is computed from.
#[derive(Debug, Clone)]
enum Source {
    /// `*`: every field the selected rows carry.
    Everything,
    /// An expression evaluated per row.
    Value(Expr),
    /// An aggregate over the rows of one group.
    Aggregate {
        /// Which aggregate.
        function: AggregateFunction,
        /// What it aggregates, or `None` for `count(*)`.
        over: Option<Expr>,
    },
}

/// Read the statement's projection as the columns it reports.
fn read_projection(select: &Select) -> Result<Vec<Column>, ExecutionFailure> {
    let mut columns: Vec<Column> = Vec::new();
    for item in &select.projection {
        let (expression, alias) = match item {
            SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(..) => {
                columns.push(Column {
                    name: WILDCARD.to_string(),
                    source: Source::Everything,
                });
                continue;
            }
            SelectItem::UnnamedExpr(expression) => (expression, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.clone())),
        };
        let source = read_source(expression)?;
        columns.push(Column {
            // A column with no alias is reported under the text that computes it, which for a
            // plain column is that column's own name.
            name: alias.unwrap_or_else(|| column_name(expression)),
            source,
        });
    }
    if columns.is_empty() {
        return Err(ExecutionFailure::new(
            "a SQL query of log events selects at least one column",
        ));
    }
    Ok(columns)
}

/// How a `SELECT *` is reported when its columns are named one by one.
const WILDCARD: &str = "*";

/// What one projected expression is computed from: an aggregate call, or the expression
/// itself.
fn read_source(expression: &Expr) -> Result<Source, ExecutionFailure> {
    if let Some((function, over)) = read_aggregate(expression)? {
        return Ok(Source::Aggregate { function, over });
    }
    Ok(Source::Value(expression.clone()))
}

/// The aggregate `expression` calls, if it calls one.
///
/// Every part of the call is read: a `DISTINCT` in its arguments makes a `count` the count of
/// DISTINCT values, and a clause this service does not compute over a group — a window
/// (`OVER`), a `FILTER`, a `WITHIN GROUP`, a null treatment, a parameter list — is refused
/// rather than ignored, because ignoring it would answer a plausible wrong number.
#[allow(clippy::type_complexity)]
fn read_aggregate(
    expression: &Expr,
) -> Result<Option<(AggregateFunction, Option<Expr>)>, ExecutionFailure> {
    let Expr::Function(function) = expression else {
        return Ok(None);
    };
    let name = function
        .name
        .0
        .iter()
        .map(|part| part.to_string())
        .collect::<Vec<String>>()
        .join(".");
    let Some(aggregate) = AggregateFunction::parse(&name) else {
        return Ok(None);
    };
    if function.over.is_some()
        || function.filter.is_some()
        || !function.within_group.is_empty()
        || function.null_treatment.is_some()
        || !matches!(function.parameters, FunctionArguments::None)
    {
        return Err(ExecutionFailure::new(format!(
            "the aggregate {name} carries a clause that has no execution in this service"
        )));
    }
    let FunctionArguments::List(arguments) = &function.args else {
        return Ok(Some((aggregate, None)));
    };
    if !arguments.clauses.is_empty() {
        return Err(ExecutionFailure::new(format!(
            "the aggregate {name} carries a clause that has no execution in this service"
        )));
    }
    let over = match arguments.args.as_slice() {
        // `count(*)` counts the rows of the group rather than the values of a column.
        [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] => None,
        [FunctionArg::Unnamed(FunctionArgExpr::Expr(inner))] => Some(inner.clone()),
        [] => None,
        _ => {
            return Err(ExecutionFailure::new(format!(
                "the aggregate {name} takes one argument"
            )))
        }
    };
    // "COUNT(DISTINCT column)" counts the column's distinct values, which is the aggregate
    // the pipeline languages spell `count_distinct`; `DISTINCT` under any other aggregate is
    // a computation this service does not do over a group.
    let aggregate = match arguments.duplicate_treatment {
        Some(DuplicateTreatment::Distinct) => match (aggregate, over.as_ref()) {
            (AggregateFunction::Count, Some(_)) => AggregateFunction::CountDistinct,
            _ => {
                return Err(ExecutionFailure::new(format!(
                    "a DISTINCT {name} has no execution in this service"
                )))
            }
        },
        _ => aggregate,
    };
    if over.is_none() && !matches!(aggregate, AggregateFunction::Count) {
        return Err(ExecutionFailure::new(format!(
            "the aggregate {name} aggregates a column"
        )));
    }
    Ok(Some((aggregate, over)))
}

/// The name a column with no alias is reported under: the column's own name for a plain
/// column reference, and otherwise the SQL that computes it.
fn column_name(expression: &Expr) -> String {
    match expression {
        Expr::Identifier(identifier) => identifier.value.clone(),
        Expr::CompoundIdentifier(parts) => parts
            .iter()
            .map(|part| part.value.clone())
            .collect::<Vec<String>>()
            .join("."),
        other => other.to_string(),
    }
}

/// Read the statement's `GROUP BY` clause as the expressions it groups by.
fn read_grouping(select: &Select) -> Result<Vec<Expr>, ExecutionFailure> {
    match &select.group_by {
        GroupByExpr::Expressions(expressions, modifiers) if modifiers.is_empty() => {
            Ok(expressions.clone())
        }
        GroupByExpr::Expressions(..) | GroupByExpr::All(..) => Err(ExecutionFailure::new(
            "a SQL query of log events groups by columns",
        )),
    }
}

/// Aggregate the rows into one row per group, answering them and the columns they carry.
///
/// The groups are folded record by record: an aggregate keeps a fixed amount of state
/// ([`expression::Accumulator`]), so a group costs the same whether it holds one row or every
/// row of the scan, and the number of groups is bounded ([`MAX_GROUPS`]).
fn aggregate_rows(
    evaluation: &Evaluation<'_>,
    rows: &[Record],
    projection: &[Column],
    grouping: &[Expr],
) -> Result<(Vec<Record>, Vec<String>), ExecutionFailure> {
    /// What one group has taken in: an accumulator per aggregated column, and the first
    /// row's value of each grouped column, which is the same for every row of the group.
    #[derive(Default)]
    struct Group {
        accumulators: Vec<expression::Accumulator>,
        grouped: Vec<Option<Value>>,
    }

    // One bucket per group, in the order of the group's key values, so one store answers one
    // statement the same way every time.
    let mut groups: BTreeMap<Vec<String>, Group> = BTreeMap::new();
    for record in rows {
        if !evaluation.budget.tick() {
            break;
        }
        let key: Vec<String> = grouping
            .iter()
            .map(|expression| evaluate(evaluation, expression, record, 0).rendered())
            .collect();
        if !groups.contains_key(&key) && groups.len() >= MAX_GROUPS {
            return Err(ExecutionFailure::new(format!(
                "a SQL query of log events groups its rows into at most {MAX_GROUPS} groups"
            )));
        }
        let group = groups.entry(key).or_insert_with(|| Group {
            accumulators: projection
                .iter()
                .map(|_| expression::Accumulator::default())
                .collect(),
            grouped: projection.iter().map(|_| None).collect(),
        });
        for (index, column) in projection.iter().enumerate() {
            match &column.source {
                Source::Aggregate { function, over } => {
                    let value = match over {
                        None => Value::Bool(true),
                        Some(expression) => evaluate(evaluation, expression, record, 0),
                    };
                    if let Some(accumulator) = group.accumulators.get_mut(index) {
                        if !accumulator.add(*function, &value, evaluation.budget) {
                            break;
                        }
                    }
                }
                // A grouped column's value is the same for every row of the group, so the
                // first row's is the group's.
                Source::Value(expression) => {
                    if group.grouped.get(index).is_some_and(Option::is_none) {
                        let value = evaluate(evaluation, expression, record, 0);
                        if let Some(held) = group.grouped.get_mut(index) {
                            *held = Some(value);
                        }
                    }
                }
                Source::Everything => {}
            }
        }
    }

    let columns: Vec<String> = projection
        .iter()
        .filter(|column| !matches!(column.source, Source::Everything))
        .map(|column| column.name.clone())
        .collect();
    let grouped = groups
        .into_values()
        .map(|group| {
            let mut record = Record::default();
            for (index, column) in projection.iter().enumerate() {
                let value = match &column.source {
                    Source::Aggregate { function, .. } => group
                        .accumulators
                        .get(index)
                        .map_or(Value::Null, |accumulator| accumulator.answer(*function)),
                    Source::Value(_) => group
                        .grouped
                        .get(index)
                        .cloned()
                        .flatten()
                        .unwrap_or(Value::Null),
                    Source::Everything => Value::Null,
                };
                if !matches!(value, Value::Null) {
                    let rendered = value.rendered();
                    if evaluation.budget.store(rendered.len()) {
                        record.set(&column.name, rendered);
                    }
                }
            }
            record
        })
        .collect();
    Ok((grouped, columns))
}

/// THE EMULATOR'S OWN CEILING on how many groups ONE statement may build: a group's key
/// comes from the rows, so a `GROUP BY` over a column whose values are all different would
/// otherwise hold one group per row.
const MAX_GROUPS: usize = 10_000;

/// Order the rows by the statement's `ORDER BY` clause.
///
/// The keys are computed ONCE PER ROW and the rows are then ordered by the computed keys: a
/// comparator that evaluated them would evaluate each row's keys once per comparison, which
/// is a multiple of the rows rather than one pass over them.
fn order_rows(
    evaluation: &Evaluation<'_>,
    query: &Query,
    rows: &mut Vec<Record>,
) -> Result<(), ExecutionFailure> {
    let Some(order_by) = query.order_by.as_ref() else {
        return Ok(());
    };
    let OrderByKind::Expressions(expressions) = &order_by.kind else {
        return Err(ExecutionFailure::new(
            "a SQL query of log events orders by columns",
        ));
    };
    let keys: Vec<(Expr, bool)> = expressions
        .iter()
        .map(|expression| {
            (
                expression.expr.clone(),
                // "ORDER BY column [ASC | DESC]": ascending unless the statement says
                // otherwise, which is SQL's own default.
                expression.options.asc == Some(false),
            )
        })
        .collect();
    let mut keyed: Vec<(Vec<Value>, Record)> = rows
        .drain(..)
        .map(|record| {
            let computed = keys
                .iter()
                .map(|(key, _)| evaluate(evaluation, key, &record, 0))
                .collect();
            (computed, record)
        })
        .collect();
    keyed.sort_by(|(left, _), (right, _)| {
        // Ordering two rows is work the run pays for, so a sort of the whole scan cannot
        // outlast the budget however many comparisons it takes.
        let _ = evaluation.budget.tick();
        for (index, (_, descending)) in keys.iter().enumerate() {
            let ordering = match (left.get(index), right.get(index)) {
                (Some(left), Some(right)) => expression::total_ordering(left, right),
                _ => std::cmp::Ordering::Equal,
            };
            let ordering = if *descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
    *rows = keyed.into_iter().map(|(_, record)| record).collect();
    Ok(())
}

/// Read the statement's `LIMIT` clause as the number of rows it bounds the answer to.
fn read_limit(query: &Query) -> Result<Option<usize>, ExecutionFailure> {
    let Some(clause) = query.limit_clause.as_ref() else {
        return Ok(None);
    };
    let LimitClause::LimitOffset {
        limit,
        offset,
        limit_by,
    } = clause
    else {
        return Err(ExecutionFailure::new(
            "a SQL query of log events carries a plain LIMIT",
        ));
    };
    if offset.is_some() || !limit_by.is_empty() {
        return Err(ExecutionFailure::new(
            "a SQL query of log events carries a plain LIMIT",
        ));
    }
    let Some(limit) = limit else {
        return Ok(None);
    };
    let rows = literal(limit)
        .and_then(|value| value.number())
        .ok_or_else(|| ExecutionFailure::new("a LIMIT bounds the answer to a number of rows"))?;
    Ok(Some(rows.max(0.0) as usize))
}

/// Evaluate one parsed SQL expression over one row.
///
/// `depth` bounds the recursion of this evaluation, which a caller-written expression
/// otherwise chooses the depth of, and each evaluation spends a step of the run's budget, so
/// the rows, the clauses and the nodes of the statement cannot multiply out past it.
fn evaluate(
    evaluation: &Evaluation<'_>,
    expression: &Expr,
    record: &Record,
    depth: usize,
) -> Value {
    if depth > super::MAX_EXPRESSION_DEPTH || !evaluation.budget.step() {
        return Value::Null;
    }
    let depth = depth + 1;
    match expression {
        Expr::Identifier(identifier) => field(&identifier.value, record),
        Expr::CompoundIdentifier(parts) => {
            // `group.column` and `column` name the same field of a row, so the qualified form
            // is read as the field its last part names, and the whole dotted path is read as
            // one field name too — which is how a flattened JSON path is written.
            let path = parts
                .iter()
                .map(|part| part.value.clone())
                .collect::<Vec<String>>()
                .join(".");
            match record.get(&path) {
                Some(value) => Value::Text(value.to_string()),
                None => parts
                    .last()
                    .map_or(Value::Null, |part| field(&part.value, record)),
            }
        }
        Expr::Nested(inner) => evaluate(evaluation, inner, record, depth),
        Expr::Value(value) => literal(expression).unwrap_or_else(|| {
            let _ = value;
            Value::Null
        }),
        Expr::UnaryOp { op, expr } => {
            let value = evaluate(evaluation, expr, record, depth);
            match op {
                UnaryOperator::Not => Value::Bool(!value.truthy()),
                UnaryOperator::Minus => value
                    .number()
                    .map_or(Value::Null, |number| Value::Number(-number)),
                UnaryOperator::Plus => value,
                _ => Value::Null,
            }
        }
        Expr::IsNull(inner) => Value::Bool(matches!(
            evaluate(evaluation, inner, record, depth),
            Value::Null
        )),
        Expr::IsNotNull(inner) => Value::Bool(!matches!(
            evaluate(evaluation, inner, record, depth),
            Value::Null
        )),
        Expr::IsTrue(inner) => Value::Bool(evaluate(evaluation, inner, record, depth).truthy()),
        Expr::IsFalse(inner) => Value::Bool(!evaluate(evaluation, inner, record, depth).truthy()),
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            let value = evaluate(evaluation, expr, record, depth);
            let inside = expression::compare(&value, &evaluate(evaluation, low, record, depth))
                .is_some_and(|ordering| ordering != std::cmp::Ordering::Less)
                && expression::compare(&value, &evaluate(evaluation, high, record, depth))
                    .is_some_and(|ordering| ordering != std::cmp::Ordering::Greater);
            Value::Bool(inside != *negated)
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let value = evaluate(evaluation, expr, record, depth);
            let found = list.iter().any(|item| {
                expression::compare(&value, &evaluate(evaluation, item, record, depth))
                    == Some(std::cmp::Ordering::Equal)
            });
            Value::Bool(found != *negated)
        }
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        }
        | Expr::ILike {
            negated,
            expr,
            pattern,
            ..
        } => {
            let subject = evaluate(evaluation, expr, record, depth).rendered();
            let pattern = evaluate(evaluation, pattern, record, depth).rendered();
            let insensitive = matches!(expression, Expr::ILike { .. });
            Value::Bool(evaluation.like_matches(&subject, &pattern, insensitive) != *negated)
        }
        Expr::BinaryOp { left, op, right } => binary(evaluation, op, left, right, record, depth),
        // An aggregate call is computed over a whole group rather than over one row, so it
        // has no value here; anything else is not an expression this evaluation reads.
        _ => Value::Null,
    }
}

/// The value of one field of the row.
fn field(name: &str, record: &Record) -> Value {
    record
        .get(name)
        .map_or(Value::Null, |value| Value::Text(value.to_string()))
}

/// Evaluate one SQL infix operator. `AND` and `OR` are evaluated lazily, so the right operand
/// of a decided boolean is never evaluated.
fn binary(
    evaluation: &Evaluation<'_>,
    op: &BinaryOperator,
    left: &Expr,
    right: &Expr,
    record: &Record,
    depth: usize,
) -> Value {
    match op {
        BinaryOperator::And => {
            if !evaluate(evaluation, left, record, depth).truthy() {
                return Value::Bool(false);
            }
            Value::Bool(evaluate(evaluation, right, record, depth).truthy())
        }
        BinaryOperator::Or => {
            if evaluate(evaluation, left, record, depth).truthy() {
                return Value::Bool(true);
            }
            Value::Bool(evaluate(evaluation, right, record, depth).truthy())
        }
        BinaryOperator::Eq
        | BinaryOperator::NotEq
        | BinaryOperator::Lt
        | BinaryOperator::LtEq
        | BinaryOperator::Gt
        | BinaryOperator::GtEq => {
            let ordering = expression::compare(
                &evaluate(evaluation, left, record, depth),
                &evaluate(evaluation, right, record, depth),
            );
            let Some(ordering) = ordering else {
                return Value::Bool(matches!(op, BinaryOperator::NotEq));
            };
            use std::cmp::Ordering;
            Value::Bool(match op {
                BinaryOperator::Eq => ordering == Ordering::Equal,
                BinaryOperator::NotEq => ordering != Ordering::Equal,
                BinaryOperator::Lt => ordering == Ordering::Less,
                BinaryOperator::LtEq => ordering != Ordering::Greater,
                BinaryOperator::Gt => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            })
        }
        BinaryOperator::Plus
        | BinaryOperator::Minus
        | BinaryOperator::Multiply
        | BinaryOperator::Divide
        | BinaryOperator::Modulo => {
            let (Some(left), Some(right)) = (
                evaluate(evaluation, left, record, depth).number(),
                evaluate(evaluation, right, record, depth).number(),
            ) else {
                return Value::Null;
            };
            let result = match op {
                BinaryOperator::Plus => left + right,
                BinaryOperator::Minus => left - right,
                BinaryOperator::Multiply => left * right,
                BinaryOperator::Divide if right != 0.0 => left / right,
                BinaryOperator::Modulo if right != 0.0 => left % right,
                _ => return Value::Null,
            };
            if result.is_finite() {
                Value::Number(result)
            } else {
                Value::Null
            }
        }
        BinaryOperator::StringConcat => {
            let left = evaluate(evaluation, left, record, depth).rendered();
            let right = evaluate(evaluation, right, record, depth).rendered();
            // The concatenation is COMPUTED, so its length is the operands' sum rather than
            // anything the log event bounds: the budget is asked for it BEFORE it is built,
            // so a statement that concatenates its way past the bound is abandoned instead
            // of allocating what it asked for.
            if !evaluation.budget.store(left.len().saturating_add(right.len())) {
                return Value::Null;
            }
            Value::Text(format!("{left}{right}"))
        }
        // An operator a query of log events does not use decides nothing.
        _ => Value::Null,
    }
}

/// The expression a SQL `LIKE` pattern stands for: `%` matches any run of characters and `_`
/// one, which is what SQL defines the pattern's two wildcards as. `None` for a pattern that
/// is not an expression at all, which matches nothing.
///
/// Compiled once per distinct pattern of a statement ([`Evaluation::like_matches`]).
fn compile_like(pattern: &str, insensitive: bool) -> Option<::regex::Regex> {
    let mut expression = String::from("^");
    for character in pattern.chars() {
        match character {
            '%' => expression.push_str(".*"),
            '_' => expression.push('.'),
            other => expression.push_str(&::regex::escape(&other.to_string())),
        }
    }
    expression.push('$');
    ::regex::RegexBuilder::new(&expression)
        .case_insensitive(insensitive)
        .size_limit(MAX_COMPILED_PATTERN_BYTES)
        .build()
        .ok()
}

/// How large a compiled pattern a `LIKE` may build. THE EMULATOR'S OWN BOUND, the same one
/// every other caller-supplied expression of this service is compiled under.
const MAX_COMPILED_PATTERN_BYTES: usize = 1 << 20;

/// The value one parsed SQL literal stands for.
fn literal(expression: &Expr) -> Option<Value> {
    use ::sqlparser::ast::Value as SqlValue;
    match expression {
        Expr::Nested(inner) => literal(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => literal(expr)
            .and_then(|value| value.number())
            .map(|number| Value::Number(-number)),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => literal(expr),
        Expr::Value(value) => match &value.value {
            SqlValue::Number(digits, _) => digits.parse::<f64>().ok().map(Value::Number),
            SqlValue::SingleQuotedString(text)
            | SqlValue::DoubleQuotedString(text)
            | SqlValue::EscapedStringLiteral(text)
            | SqlValue::UnicodeStringLiteral(text) => Some(Value::Text(text.clone())),
            SqlValue::Boolean(flag) => Some(Value::Bool(*flag)),
            SqlValue::Null => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}
