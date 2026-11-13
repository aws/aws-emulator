// SPDX-License-Identifier: CC0-1.0

//! Running a PIPELINED query over the records of a scan: the Logs Insights QL pipeline
//! `StartQuery` compiles, and the OpenSearch PPL pipeline it also accepts.
//!
//! A pipeline is a sequence of stages, each reading the records the previous one produced:
//! `fields @timestamp, @message | filter @message like /ERROR/ | stats count(*) by level |
//! sort level | limit 20`. The two languages differ only in which words name those stages —
//! PPL writes `where` for a selection and `head` for a bound — so both run here, over the
//! same [`super::Record`]s, with one vocabulary each ([`PPL_COMMANDS`]).
//!
//! What a stage does is what the language documents it does, computed over the records in
//! hand:
//!
//! * `fields` / `display` choose the columns a result carries, and may define one with an
//!   `as` clause;
//! * `filter` / `where` keep the records their expression selects ([`super::expression`]);
//! * `parse` extracts ephemeral fields from a field's text, by regular expression or by the
//!   glob pattern whose `*`s the `as` clause names;
//! * `stats` aggregates the records into one row per group, and `pattern` groups them by the
//!   shape of their messages;
//! * `sort`, `limit`/`head` and `dedup` order, bound and deduplicate what is left;
//! * `lookup` joins the rows of one of this account's lookup tables onto each record;
//! * `unmask` reveals nothing further here, because this store holds every event's raw
//!   message and a query reads those messages as they were ingested.
//!
//! WHAT BOUNDS IT: the records come from a bounded scan ([`super::MAX_SCANNED_EVENTS`]), the
//! stages are a bounded list (the query string is `@length(0..10000)`), every expression is
//! parsed once per stage rather than once per record, and the rows the run answers are capped
//! at [`super::MAX_RESULT_ROWS`].

use std::collections::BTreeMap;

use super::expression::{self, Expr, Value};
use super::{
    Budget, Execution, ExecutionFailure, GroupSelector, Record, Scan, DEFAULT_COLUMNS,
    FIELD_MESSAGE, FIELD_PTR, FIELD_TIMESTAMP, MAX_RESULT_ROWS,
};
use crate::state::query_compile::{self, CompiledStage};
use crate::state::QueryRecord;

/// Plan a Logs Insights QL query: the pipeline `StartQuery` compiled.
///
/// The query string is re-read through the same compiler that accepted it, so what runs is
/// exactly the pipeline `StartQuery` judged; a string that no longer compiles cannot be run
/// at all.
pub(super) fn plan_logs_insights(query_string: &str) -> Result<super::Plan, ExecutionFailure> {
    let compiled = query_compile::compile(query_string)
        .map_err(|failure| ExecutionFailure::new(failure.message))?;
    Ok(super::Plan::Pipeline {
        dialect: Dialect::LogsInsights,
        stages: compiled.stages,
    })
}

/// Plan an OpenSearch PPL query: the same pipeline machinery over PPL's own stage vocabulary.
pub(super) fn plan_ppl(query_string: &str) -> Result<super::Plan, ExecutionFailure> {
    let stages: Vec<CompiledStage> = query_compile::split_stages(query_string, &PPL_COMMANDS)
        .into_iter()
        .map(normalize_ppl_stage)
        .collect();
    if stages.is_empty() {
        return Err(ExecutionFailure::new("the query string is empty"));
    }
    Ok(super::Plan::Pipeline {
        dialect: Dialect::Ppl,
        stages,
    })
}

/// The log groups a pipeline's own source stages select.
///
/// A `SOURCE` / `source=` stage selects the log groups itself, overriding whatever the start
/// was handed; a pipeline with no such stage selects none, so the query reads the groups the
/// start resolved.
pub(super) fn source_selectors(stages: &[CompiledStage]) -> Vec<GroupSelector> {
    stages
        .iter()
        .filter(|stage| stage.command == SOURCE)
        .flat_map(|stage| super::read_group_selectors(&stage.arguments))
        .collect()
}

/// The lookup tables a pipeline's `lookup` stages read, each named by that command's first
/// argument (`lookup <lookupTableName> <keyField> …`).
pub(super) fn lookup_table_names(stages: &[CompiledStage]) -> Vec<String> {
    stages
        .iter()
        .filter(|stage| stage.command == LOOKUP)
        .filter_map(|stage| {
            stage
                .arguments
                .split_whitespace()
                .next()
                .map(|name| super::trim_identifier(name).to_string())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// The stage words PPL's pipelines are written with, each lowercased as [`split_stages`]
/// reads them.
///
/// [`split_stages`]: crate::state::query_compile::split_stages
pub const PPL_COMMANDS: [&str; 12] = [
    "source", "search", "where", "fields", "table", "stats", "sort", "head", "dedup", "eval",
    "rename", "parse",
];

/// A PPL stage as the pipeline runs it: a `source=<selector>` stage is given its selector
/// however the query string spaced the `=`, which the splitter leaves at the head of the
/// stage's arguments.
fn normalize_ppl_stage(stage: CompiledStage) -> CompiledStage {
    CompiledStage {
        command: stage.command,
        arguments: stage
            .arguments
            .trim()
            .trim_start_matches('=')
            .trim()
            .to_string(),
    }
}

/// Which language's stage vocabulary a pipeline is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    /// Logs Insights QL.
    LogsInsights,
    /// OpenSearch PPL.
    Ppl,
}

/// What a pipeline's stages have produced so far.
#[derive(Debug, Default)]
struct Rows {
    /// The records in hand.
    records: Vec<Record>,
    /// The columns the result carries, when a stage has chosen them; `None` while the result
    /// still carries the default columns.
    columns: Option<Vec<String>>,
    /// Whether the records are GROUPS rather than log events, which is what an aggregation
    /// leaves behind: a group is no single log record, so it carries no `@ptr`.
    aggregated: bool,
    /// How many log events matched the query's selection criteria — the count
    /// `QueryStatistics$recordsMatched` reports, which a bound on the rows returned does not
    /// change.
    matched: usize,
}

/// Run `stages` over the records the scan read.
///
/// Reached with this service's state released ([`super::Prepared`]), so a stage reaches
/// nothing but the records in hand and the tables the scan carries. The stages yield between
/// one another, so a cancelled or timed-out request has somewhere to land rather than running
/// the whole pipeline first.
pub(super) async fn run(
    dialect: Dialect,
    stages: &[CompiledStage],
    scan: Scan,
    query: &QueryRecord,
    budget: &Budget,
) -> Execution {
    let Scan {
        records,
        scanned,
        bytes,
        groups,
        lookup_tables,
    } = scan;
    let mut rows = Rows {
        matched: records.len(),
        records,
        ..Rows::default()
    };
    // The row bound the START named, which is authoritative over any the query string carries
    // ([`super::requested_row_limit`]).
    let requested = super::requested_row_limit(query);
    for stage in stages {
        ::tokio::task::yield_now().await;
        // A stage this service cannot run, and a run that has spent its budget, each leave
        // the query with no results. What the run READ is still reported, because the events
        // were read before the stage was reached.
        let outcome = apply(&lookup_tables, &mut rows, stage, dialect, requested, budget)
            .err()
            .map(|failure| failure.reason)
            .or_else(|| budget.reason());
        if let Some(reason) = outcome {
            return Execution {
                failed: Some(reason),
                records_scanned: scanned as f64,
                bytes_scanned: bytes as f64,
                log_groups_scanned: groups as f64,
                ..Execution::default()
            };
        }
    }

    // "The maximum number of log events to return from the query" (`StartQuery$limit`) bounds
    // the whole result, and the API hands back at most `MAX_RESULT_ROWS` of them however many
    // the query asked for.
    rows.records.truncate(requested.unwrap_or(MAX_RESULT_ROWS));

    Execution {
        rows: rendered(&rows, budget),
        // The result set holds the records its rows point at, so the `@ptr` a row carries
        // keeps addressing the record this run read ([`Execution::records`]).
        records: pointed_records(&rows),
        records_matched: rows.matched as f64,
        records_scanned: scanned as f64,
        bytes_scanned: bytes as f64,
        log_groups_scanned: groups as f64,
        failed: None,
    }
}

/// The whole record behind each row of LOG RECORDS, keyed by the `@ptr` that row carries.
///
/// An AGGREGATION's rows are no single log record and carry no pointer, so they contribute
/// nothing; neither does a record a stage left without one.
fn pointed_records(rows: &Rows) -> BTreeMap<String, Record> {
    if rows.aggregated {
        return BTreeMap::new();
    }
    rows.records
        .iter()
        .filter_map(|record| {
            record
                .get(FIELD_PTR)
                .map(|pointer| (pointer.to_string(), record.clone()))
        })
        .collect()
}

/// The rows a finished pipeline answers: each record as the field/value pairs of the columns
/// the result carries, in that order.
///
/// A field a record does not carry contributes no pair, so a row states what the record has
/// rather than filling a column with a value nothing computed. `@ptr` is appended to every
/// row of LOG RECORDS — "Only the fields requested in the query are returned, along with a
/// @ptr field, which is the identifier for the log record" — and to no row of an aggregation,
/// which is no single log record.
fn rendered(rows: &Rows, budget: &Budget) -> Vec<Vec<(String, String)>> {
    let mut columns: Vec<String> = rows.columns.clone().unwrap_or_else(|| {
        DEFAULT_COLUMNS
            .iter()
            .map(|column| (*column).to_string())
            .collect()
    });
    if !rows.aggregated && !columns.iter().any(|column| column == FIELD_PTR) {
        columns.push(FIELD_PTR.to_string());
    }
    rows.records
        .iter()
        // Rendering one record is work the run pays for, so the product of the records and
        // the columns cannot outlast the budget either.
        .take_while(|_| budget.tick())
        .map(|record| {
            columns
                .iter()
                .filter_map(|column| {
                    record
                        .get(column)
                        .map(|value| (column.clone(), value.to_string()))
                })
                .collect()
        })
        .collect()
}

/// The command that selects the log groups a query reads.
const SOURCE: &str = "source";

/// The command a PPL pipeline may open with, which carries that same selector.
const SEARCH: &str = "search";

/// What a stage's arguments carry BEYOND a source selector, with nothing left of the
/// selector itself: the empty string for arguments that are a selector and nothing else.
///
/// A selector is `source = <name>` or `source = logGroups(…)`, so what follows it is whatever
/// comes after the selector's closing bracket, or after the one word that names a log group.
fn source_selector_remainder(arguments: &str) -> String {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let after_keyword = match trimmed.get(..SOURCE.len()) {
        Some(head) if head.eq_ignore_ascii_case(SOURCE) => trimmed[SOURCE.len()..]
            .trim_start()
            .trim_start_matches('=')
            .trim_start(),
        // A `search` that opens with no selector at all carries only a condition.
        _ => trimmed,
    };
    match (after_keyword.find('('), after_keyword.find(')')) {
        (Some(open), Some(close)) if close > open => after_keyword[close + 1..].trim().to_string(),
        _ => after_keyword
            .split_whitespace()
            .skip(1)
            .collect::<Vec<&str>>()
            .join(" "),
    }
}

/// The command that enriches each record from a lookup table.
const LOOKUP: &str = "lookup";

/// Apply one stage to the records in hand.
///
/// `requested` is the row bound the start of the query named, when it named one: it is
/// authoritative over the one a row-bounding command of the pipeline carries
/// ([`super::requested_row_limit`]).
fn apply(
    lookup_tables: &BTreeMap<String, String>,
    rows: &mut Rows,
    stage: &CompiledStage,
    dialect: Dialect,
    requested: Option<usize>,
    budget: &Budget,
) -> Result<(), ExecutionFailure> {
    let arguments = stage.arguments.trim();
    match stage.command.as_str() {
        // The log groups it selects were resolved before the scan, so the stage selects no
        // records of its own.
        SOURCE => Ok(()),
        // `search` opens a PPL pipeline with the same selector — and may also carry a
        // condition over the records it opens with, which this service does not read there:
        // a `search` carrying anything beyond its selector is the execution that produced
        // nothing rather than a selection quietly dropped.
        SEARCH => {
            if source_selector_remainder(arguments).is_empty() {
                Ok(())
            } else {
                Err(ExecutionFailure::new(
                    "a search command's condition has no execution in this service",
                ))
            }
        }
        "fields" | "display" | "table" => fields(rows, arguments, dialect, budget),
        "filter" | "where" => filter(rows, arguments, budget),
        "parse" => parse_stage(rows, arguments, budget),
        "eval" => eval(rows, arguments, budget),
        "rename" => rename(rows, arguments),
        "stats" => stats(rows, arguments, budget),
        "pattern" | "patterns" => pattern(rows, arguments, budget),
        "sort" => sort(rows, arguments, dialect, budget),
        "limit" | "head" => {
            let count = arguments
                .split_whitespace()
                .next()
                .and_then(|count| count.parse::<usize>().ok())
                .ok_or_else(|| {
                    ExecutionFailure::new(format!("{} is not a number of log events", stage.command))
                })?;
            // The count the command states is still read — a command stating none is no
            // bound at all — but a start that named its own row bound REPLACES it: the
            // request parameter is authoritative over the query string's bound in both
            // directions ([`super::requested_row_limit`]).
            rows.records.truncate(requested.unwrap_or(count));
            Ok(())
        }
        "dedup" => dedup(rows, arguments, budget),
        LOOKUP => lookup(lookup_tables, rows, arguments, budget),
        // "unmask — Displays all the content of a log event that has masked data": the
        // records in hand carry every event's message exactly as it was ingested
        // (`insights::record_of`), so this stage has nothing further to reveal. A data
        // protection policy's masked rendering is deliberately not applied on this path:
        // the two event reads that report it are `GetLogEvents` and `FilterLogEvents`
        // (`crate::state::data_protection`).
        "unmask" => Ok(()),
        // "diff — Compares the log events in the time range with the events in the previous
        // time range of the same length": the comparison is over a SECOND range this run
        // does not read, so this service cannot run the stage — and a query carrying it is
        // reported as the execution that produced nothing rather than answered with the rows
        // of the one range it did read.
        other => Err(ExecutionFailure::new(format!(
            "the query command {other} has no execution in this service"
        ))),
    }
}

/// Store a value a stage COMPUTED on a record, spending the run's budget for it.
///
/// A computed value — a concatenation, a captured substring, an aggregate — is not bounded by
/// the log event it came from, so what one run computes is bounded by the budget
/// ([`Budget::store`]); a value the budget will not take is not stored, and the run is
/// reported as the failure the budget records.
fn store(record: &mut Record, field: &str, value: String, budget: &Budget) {
    if budget.store(value.len()) {
        record.set(field, value);
    }
}

/// `fields` / `display`: choose the columns the result carries, defining one for each item
/// that is an expression rather than a plain field name.
///
/// PPL writes a leading `+` to keep the listed fields and a leading `-` to drop them, which
/// is the one difference between the two languages' spelling of this stage.
fn fields(
    rows: &mut Rows,
    arguments: &str,
    dialect: Dialect,
    budget: &Budget,
) -> Result<(), ExecutionFailure> {
    let (dropping, listed) = match (dialect, arguments.strip_prefix('-')) {
        (Dialect::Ppl, Some(rest)) => (true, rest),
        _ => (false, arguments.trim_start_matches('+')),
    };
    let mut columns: Vec<String> = Vec::new();
    for item in split_items(listed) {
        let (text, alias) = split_alias(&item);
        let name = match (expression::parse(&text), alias) {
            // A plain field name selects that field.
            (Ok(Expr::Field(field)), None) => field,
            // An expression defines the field its `as` clause names, computed per record.
            (Ok(expression), Some(alias)) => {
                for record in &mut rows.records {
                    let value = expression::evaluate(&expression, record, budget);
                    if !matches!(value, Value::Null) {
                        store(record, &alias, value.rendered(), budget);
                    }
                }
                alias
            }
            // An expression with no name to put it under contributes a column named by the
            // text that computes it, which is how a result reports one.
            (Ok(expression), None) => {
                for record in &mut rows.records {
                    let value = expression::evaluate(&expression, record, budget);
                    if !matches!(value, Value::Null) {
                        store(record, &text, value.rendered(), budget);
                    }
                }
                text
            }
            (Err(reason), _) => return Err(ExecutionFailure::new(reason)),
        };
        if !columns.contains(&name) {
            columns.push(name);
        }
    }
    if dropping {
        // `fields - a, b` drops the named fields and leaves the rest.
        for record in &mut rows.records {
            for column in &columns {
                record.values.remove(column);
            }
        }
        if let Some(kept) = rows.columns.as_mut() {
            kept.retain(|column| !columns.contains(column));
        }
        return Ok(());
    }
    rows.columns = Some(columns);
    Ok(())
}

/// `filter` / `where`: keep the records the expression selects, and count them as the
/// records that matched the query's criteria.
fn filter(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    let expression = expression::parse(arguments).map_err(ExecutionFailure::new)?;
    rows.records
        .retain(|record| expression::evaluate(&expression, record, budget).truthy());
    rows.matched = rows.records.len();
    Ok(())
}

/// `parse`: extract ephemeral fields out of a field's text.
///
/// Two forms, both of which the language documents: a regular expression whose NAMED capture
/// groups name the fields, and a glob pattern whose `*`s are named by the `as` clause in the
/// order they appear.
fn parse_stage(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    let (text, aliases) = split_alias_list(arguments);
    let source = text
        .split_whitespace()
        .next()
        .filter(|token| token.starts_with('@') || !token.starts_with('/'))
        .map_or_else(|| FIELD_MESSAGE.to_string(), ToString::to_string);
    let rest = text
        .trim_start()
        .strip_prefix(source.as_str())
        .unwrap_or(&text)
        .trim();
    let pattern = if let Some(literal) = rest.strip_prefix('/') {
        expression::compile_pattern(literal.trim_end_matches('/'))
            .map_err(ExecutionFailure::new)?
    } else {
        let glob = super::trim_identifier(rest);
        expression::compile_pattern(&glob_to_pattern(glob)).map_err(ExecutionFailure::new)?
    };
    let names: Vec<String> = if aliases.is_empty() {
        pattern
            .capture_names()
            .flatten()
            .map(ToString::to_string)
            .collect()
    } else {
        aliases
    };
    for record in &mut rows.records {
        // Running the expression over one record is work the run pays for, whether or not it
        // captures anything.
        if !budget.tick() {
            break;
        }
        let Some(subject) = record.get(&source).map(ToString::to_string) else {
            continue;
        };
        let Some(captures) = pattern.captures(&subject) else {
            continue;
        };
        for (index, name) in names.iter().enumerate() {
            // A named group is read by its name and a glob's `*` by its position, which is
            // the group the pattern built for it.
            let captured = captures
                .name(name)
                .or_else(|| captures.get(index + 1))
                .map(|capture| capture.as_str().to_string());
            if let Some(captured) = captured {
                store(record, name, captured, budget);
            }
        }
    }
    Ok(())
}

/// The regular expression one glob pattern stands for: every `*` captures the text it
/// stands in for, and everything else matches itself.
///
/// A `*` captures as LITTLE as it can, so the text between two literal parts of the pattern
/// is the text between them; a trailing `*` has no literal part after it to stop at, so it
/// captures the whole of the rest of the field.
fn glob_to_pattern(glob: &str) -> String {
    let parts: Vec<&str> = glob.split('*').collect();
    let mut pattern = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            if index + 1 == parts.len() {
                pattern.push_str("(.*)");
            } else {
                pattern.push_str("(.*?)");
            }
        }
        pattern.push_str(&::regex::escape(part));
    }
    pattern
}

/// `eval`: define a field from an expression, as PPL writes it (`eval name = expression`).
fn eval(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    for item in split_items(arguments) {
        let Some((name, text)) = item.split_once('=') else {
            return Err(ExecutionFailure::new(
                "an eval command assigns an expression to a field",
            ));
        };
        let name = super::trim_identifier(name).to_string();
        let expression = expression::parse(text).map_err(ExecutionFailure::new)?;
        for record in &mut rows.records {
            let value = expression::evaluate(&expression, record, budget);
            store(record, &name, value.rendered(), budget);
        }
        if let Some(columns) = rows.columns.as_mut() {
            if !columns.contains(&name) {
                columns.push(name);
            }
        }
    }
    Ok(())
}

/// `rename`: carry a field's value under a new name, as PPL writes it (`rename a as b`).
fn rename(rows: &mut Rows, arguments: &str) -> Result<(), ExecutionFailure> {
    for item in split_items(arguments) {
        let (from, Some(to)) = split_alias(&item) else {
            return Err(ExecutionFailure::new(
                "a rename command names the field to rename and the name to give it",
            ));
        };
        let from = super::trim_identifier(&from).to_string();
        for record in &mut rows.records {
            if let Some(value) = record.values.remove(&from) {
                record.set(&to, value);
            }
        }
        if let Some(columns) = rows.columns.as_mut() {
            for column in columns.iter_mut() {
                if *column == from {
                    *column = to.clone();
                }
            }
        }
    }
    Ok(())
}

/// `stats`: aggregate the records into one row per group.
///
/// The aggregates come first and the grouping follows a `by` clause (`stats count(*) as
/// total by level, bin(5m)`). The result's columns are the grouped fields and then the
/// aggregates, each named by its `as` clause or by the text that computes it.
fn stats(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    let (aggregate_text, group_text) = split_on_keyword(arguments, BY);
    let mut aggregates: Vec<(String, Aggregate)> = Vec::new();
    for item in split_items(&aggregate_text) {
        let (text, alias) = split_alias(&item);
        let expression = expression::parse(&text).map_err(ExecutionFailure::new)?;
        let aggregate = Aggregate::read(&expression).ok_or_else(|| {
            ExecutionFailure::new(format!("{text} is not an aggregate of the query language"))
        })?;
        aggregates.push((
            alias.unwrap_or_else(|| reported_name(&expression, &text)),
            aggregate,
        ));
    }
    if aggregates.is_empty() {
        return Err(ExecutionFailure::new(
            "a stats command names at least one aggregate",
        ));
    }
    // "stats" takes a list of aggregates, and the records of the scan are folded once per
    // aggregate per group: the list's length is the caller's, so it is bounded here.
    if aggregates.len() > MAX_AGGREGATES {
        return Err(ExecutionFailure::new(format!(
            "a stats command names at most {MAX_AGGREGATES} aggregates"
        )));
    }

    let mut groupings: Vec<(String, Grouping)> = Vec::new();
    for item in group_text.iter().flat_map(|text| split_items(text)) {
        let (text, alias) = split_alias(&item);
        let expression = expression::parse(&text).map_err(ExecutionFailure::new)?;
        let name = alias.unwrap_or_else(|| reported_name(&expression, &text));
        groupings.push((name, Grouping::read(&expression)?));
    }

    // One accumulator per aggregate per group, FOLDED record by record: an aggregate keeps a
    // fixed amount of state, so a group costs the same whatever it holds, and the groups
    // themselves are bounded ([`MAX_GROUPS`]). Kept in the order of the group's key values,
    // so one store answers one query the same way every time.
    let mut groups: BTreeMap<Vec<String>, Vec<expression::Accumulator>> = BTreeMap::new();
    for record in &rows.records {
        if !budget.tick() {
            break;
        }
        let key: Vec<String> = groupings
            .iter()
            .map(|(_, grouping)| grouping.key(record, budget))
            .collect();
        if !groups.contains_key(&key) && groups.len() >= MAX_GROUPS {
            return Err(ExecutionFailure::new(format!(
                "a stats command groups the records into at most {MAX_GROUPS} groups"
            )));
        }
        let collected = groups.entry(key).or_insert_with(|| {
            aggregates
                .iter()
                .map(|_| expression::Accumulator::default())
                .collect()
        });
        for ((_, aggregate), accumulator) in aggregates.iter().zip(collected.iter_mut()) {
            let value = aggregate.value_of(record, budget);
            if !accumulator.add(aggregate.function, &value, budget) {
                break;
            }
        }
    }

    let mut columns: Vec<String> = groupings.iter().map(|(name, _)| name.clone()).collect();
    columns.extend(aggregates.iter().map(|(name, _)| name.clone()));
    rows.records = groups
        .into_iter()
        .map(|(key, collected)| {
            let mut record = Record::default();
            for ((name, _), value) in groupings.iter().zip(key) {
                store(&mut record, name, value, budget);
            }
            for ((name, aggregate), accumulator) in aggregates.iter().zip(collected.iter()) {
                store(
                    &mut record,
                    name,
                    accumulator.answer(aggregate.function).rendered(),
                    budget,
                );
            }
            record
        })
        .collect();
    rows.columns = Some(columns);
    rows.aggregated = true;
    Ok(())
}

/// The keyword that opens the grouping of a `stats` command.
const BY: &str = "by";

/// THE EMULATOR'S OWN CEILING on how many aggregates ONE `stats` command may name: the
/// records of the scan are folded once per aggregate per group, so the list's length is a
/// factor of the work and the caller writes it.
const MAX_AGGREGATES: usize = 64;

/// THE EMULATOR'S OWN CEILING on how many groups ONE aggregation may build: a group's key
/// comes from the records, so a grouping by a field whose values are all different would
/// otherwise hold one group per record.
const MAX_GROUPS: usize = 10_000;

/// The name a list item with no `as` clause is reported under: the FIELD's own name when the
/// item is a plain field reference — whatever quoting the query string wrote it with — and
/// otherwise the text that computes it, which is how a result reports an aggregate like
/// `count(*)`.
fn reported_name(expression: &Expr, text: &str) -> String {
    match expression {
        Expr::Field(field) => field.clone(),
        _ => text.trim().to_string(),
    }
}

/// `pattern`: group the records by the SHAPE of a field's text, which is what that command
/// finds ("automatically cluster your log data into patterns"). Each group reports the
/// pattern, how many records carry it and what share of the records that is.
fn pattern(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    let field = split_items(arguments)
        .into_iter()
        .next()
        .map(|item| super::trim_identifier(&item).to_string())
        .filter(|field| !field.is_empty())
        .unwrap_or_else(|| FIELD_MESSAGE.to_string());
    let total = rows.records.len();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for record in &rows.records {
        if !budget.tick() {
            break;
        }
        let text = record.get(&field).unwrap_or_default();
        *counts.entry(message_pattern(text)).or_default() += 1;
        if counts.len() > MAX_GROUPS {
            return Err(ExecutionFailure::new(format!(
                "a pattern command clusters the records into at most {MAX_GROUPS} patterns"
            )));
        }
    }
    rows.records = counts
        .into_iter()
        .map(|(pattern, count)| {
            let mut record = Record::default();
            store(&mut record, FIELD_PATTERN, pattern, budget);
            store(
                &mut record,
                FIELD_SAMPLE_COUNT,
                expression::render_number(count as f64),
                budget,
            );
            store(
                &mut record,
                FIELD_RATIO,
                expression::render_number(if total == 0 {
                    0.0
                } else {
                    count as f64 / total as f64
                }),
                budget,
            );
            record
        })
        .collect();
    rows.columns = Some(
        [FIELD_PATTERN, FIELD_SAMPLE_COUNT, FIELD_RATIO]
            .iter()
            .map(|column| (*column).to_string())
            .collect(),
    );
    rows.aggregated = true;
    Ok(())
}

/// The shared shape a `pattern` group reports.
const FIELD_PATTERN: &str = "@pattern";

/// How many records of the group carry that shape.
const FIELD_SAMPLE_COUNT: &str = "@sampleCount";

/// What share of the records the group is.
const FIELD_RATIO: &str = "@ratio";

/// The placeholder a pattern writes where its records carry different text.
const PATTERN_PLACEHOLDER: &str = "<*>";

/// The shape of one message: the text it shares with the messages of its cluster, with every
/// token that VARIES — a number, or a token carrying one — replaced by a placeholder.
fn message_pattern(text: &str) -> String {
    let mut pattern = String::new();
    for token in text.split_whitespace() {
        if !pattern.is_empty() {
            pattern.push(' ');
        }
        if token.chars().any(char::is_numeric) {
            pattern.push_str(PATTERN_PLACEHOLDER);
        } else {
            pattern.push_str(token);
        }
    }
    pattern
}

/// `sort`: order the records by the keys the stage names.
///
/// A key is descending when it says so (`sort level desc`) or when PPL writes it with a
/// leading `-`; ascending is the default both languages state. The order itself is the one
/// total order both query surfaces sort by ([`expression::total_ordering`]) — a rendered
/// timestamp is fixed-width, so ordering one as text orders it by the instant it stands for.
fn sort(
    rows: &mut Rows,
    arguments: &str,
    dialect: Dialect,
    budget: &Budget,
) -> Result<(), ExecutionFailure> {
    let mut keys: Vec<(Expr, bool)> = Vec::new();
    for item in split_items(arguments) {
        let mut text = item.trim().to_string();
        let mut descending = false;
        if dialect == Dialect::Ppl {
            if let Some(rest) = text.strip_prefix('-') {
                descending = true;
                text = rest.to_string();
            } else if let Some(rest) = text.strip_prefix('+') {
                text = rest.to_string();
            }
        }
        for (suffix, flag) in [(" desc", true), (" asc", false)] {
            if text.to_lowercase().ends_with(suffix) {
                descending = flag;
                text.truncate(text.len() - suffix.len());
            }
        }
        keys.push((
            expression::parse(&text).map_err(ExecutionFailure::new)?,
            descending,
        ));
    }
    // The keys are computed ONCE PER RECORD and the records are then ordered by the computed
    // keys: a comparator that evaluated them would evaluate each record's keys once per
    // comparison, which is a multiple of the records rather than one pass over them.
    let mut keyed: Vec<(Vec<Value>, Record)> = rows
        .records
        .drain(..)
        .map(|record| {
            let computed = keys
                .iter()
                .map(|(key, _)| expression::evaluate(key, &record, budget))
                .collect();
            (computed, record)
        })
        .collect();
    // `sort_by` is STABLE, so records sharing every key keep the order they arrived in.
    keyed.sort_by(|(left, _), (right, _)| {
        // Ordering two records is work the run pays for, so a sort of the whole scan cannot
        // outlast the budget however many comparisons it takes.
        let _ = budget.tick();
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
    rows.records = keyed.into_iter().map(|(_, record)| record).collect();
    Ok(())
}

/// `dedup`: keep the FIRST record carrying each distinct combination of the named fields,
/// which is what deduplicating by them means over records already in order.
fn dedup(rows: &mut Rows, arguments: &str, budget: &Budget) -> Result<(), ExecutionFailure> {
    let fields: Vec<String> = split_items(arguments)
        .iter()
        .map(|item| super::trim_identifier(item).to_string())
        .filter(|field| !field.is_empty())
        .collect();
    if fields.is_empty() {
        return Err(ExecutionFailure::new(
            "a dedup command names at least one field",
        ));
    }
    let mut seen: std::collections::BTreeSet<Vec<String>> = std::collections::BTreeSet::new();
    rows.records.retain(|record| {
        if !budget.tick() {
            return false;
        }
        let key: Vec<String> = fields
            .iter()
            .map(|field| record.get(field).unwrap_or_default().to_string())
            .collect();
        seen.insert(key)
    });
    Ok(())
}

/// `lookup`: join the rows of one of this account's lookup tables onto each record.
///
/// `lookup <table> <keyField>[ AS <tableKeyField>] [OUTPUT <tableField>[ AS <newField>], …]`:
/// the record's key field is matched against the table's key column, and the table's columns
/// — the ones the `OUTPUT` list names, or every column when it names none — are set on the
/// record. A record whose key matches no row of the table is left as it is.
///
/// The table's content comes from the scan, which read it while the store was locked
/// ([`Scan::lookup_tables`]), so the join itself reaches no state.
fn lookup(
    lookup_tables: &BTreeMap<String, String>,
    rows: &mut Rows,
    arguments: &str,
    budget: &Budget,
) -> Result<(), ExecutionFailure> {
    let (before_output, output) = split_on_keyword(arguments, OUTPUT);
    let mut tokens = before_output.split_whitespace();
    let table_name = tokens
        .next()
        .map(|name| super::trim_identifier(name).to_string())
        .ok_or_else(|| ExecutionFailure::new("a lookup command names a lookup table"))?;
    let record_key = tokens
        .next()
        .map(|field| super::trim_identifier(field).to_string())
        .ok_or_else(|| ExecutionFailure::new("a lookup command names the field to join on"))?;
    // `keyField AS tableKeyField`: the record's field is matched against THAT column of the
    // table, and against the column of the same name when no `as` clause renames it.
    let table_key = tokens
        .next()
        .filter(|word| word.eq_ignore_ascii_case(AS))
        .and_then(|_| tokens.next())
        .map_or_else(|| record_key.clone(), |field| {
            super::trim_identifier(field).to_string()
        });

    let table_body = lookup_tables.get(&table_name).ok_or_else(|| {
        ExecutionFailure::new(format!("this account holds no lookup table {table_name}"))
    })?;
    let (columns, table_rows) = read_table(table_body);
    let Some(key_column) = columns.iter().position(|column| *column == table_key) else {
        return Err(ExecutionFailure::new(format!(
            "the lookup table {table_name} has no column {table_key}"
        )));
    };

    // The columns joined onto each record: the `OUTPUT` list's, each under the name its own
    // `as` clause gives it, or every column of the table other than the key.
    let mut joined: Vec<(String, String)> = Vec::new();
    match output {
        Some(output) => {
            for item in split_items(&output) {
                let (column, alias) = split_alias(&item);
                let column = super::trim_identifier(&column).to_string();
                joined.push((column.clone(), alias.unwrap_or(column)));
            }
        }
        None => {
            joined.extend(
                columns
                    .iter()
                    .filter(|column| **column != table_key)
                    .map(|column| (column.clone(), column.clone())),
            );
        }
    }

    for record in &mut rows.records {
        if !budget.tick() {
            break;
        }
        let Some(key) = record.get(&record_key).map(ToString::to_string) else {
            continue;
        };
        let Some(row) = table_rows
            .iter()
            .find(|row| row.get(key_column).is_some_and(|value| *value == key))
        else {
            continue;
        };
        for (column, name) in &joined {
            let value = columns
                .iter()
                .position(|candidate| candidate == column)
                .and_then(|index| row.get(index));
            if let Some(value) = value {
                store(record, name, value.clone(), budget);
            }
        }
    }
    Ok(())
}

/// The keyword that opens the column list a `lookup` command joins.
const OUTPUT: &str = "output";

/// The keyword that names what a command's expression is assigned to.
const AS: &str = "as";

/// THE EMULATOR'S OWN CEILING on how many rows of a lookup table one `lookup` stage reads.
const MAX_LOOKUP_ROWS: usize = 10_000;

/// Read a lookup table's CSV body as its column names and its rows, with the `csv` crate that
/// wrote it — so a quoted cell carrying a comma is one cell, as CSV defines it.
fn read_table(body: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(body.as_bytes());
    let columns: Vec<String> = reader
        .headers()
        .map(|headers| headers.iter().map(str::to_string).collect())
        .unwrap_or_default();
    let rows: Vec<Vec<String>> = reader
        .records()
        .take(MAX_LOOKUP_ROWS)
        .filter_map(Result::ok)
        .map(|row| row.iter().map(str::to_string).collect())
        .collect();
    (columns, rows)
}

/// One aggregate of a `stats` command: the function and what it is computed over (`None` for
/// `count(*)`, which counts the records themselves).
#[derive(Debug, Clone)]
struct Aggregate {
    function: expression::AggregateFunction,
    over: Option<Expr>,
}

impl Aggregate {
    /// Read one parsed expression as an aggregate, or `None` when it is not a call to one.
    fn read(expression: &Expr) -> Option<Self> {
        let Expr::Call {
            name,
            args,
            wildcard,
        } = expression
        else {
            return None;
        };
        let function = expression::AggregateFunction::parse(name)?;
        let over = args.first().cloned();
        if over.is_none()
            && !(*wildcard || matches!(function, expression::AggregateFunction::Count))
        {
            return None;
        }
        Some(Self { function, over })
    }

    /// What one record contributes to this aggregate: the value of what it aggregates, or the
    /// record's own presence for an aggregate over the records themselves.
    fn value_of(&self, record: &Record, budget: &Budget) -> Value {
        match &self.over {
            None => Value::Bool(true),
            Some(expression) => expression::evaluate(expression, record, budget),
        }
    }
}

/// What one grouping key of a `stats` command groups by.
#[derive(Debug, Clone)]
enum Grouping {
    /// An expression evaluated per record, whose value is the key.
    Value(Expr),
    /// `bin(<width>)` / `span(<field>, <width>)`: the record's timestamp truncated to a whole
    /// multiple of the width, which is what puts the records of one span of time into one
    /// group.
    Bin {
        /// The timestamp field the span is taken over.
        field: String,
        /// The span's width, in milliseconds.
        width: i64,
    },
}

impl Grouping {
    /// Read one parsed expression as a grouping key.
    ///
    /// A span is written with its width alone (`bin(5m)`) or with the field it is taken over
    /// and then the width (`span(@timestamp, 1m)`), so the WIDTH is whichever argument is
    /// written as a span of time and the field is whichever one names a field.
    fn read(expression: &Expr) -> Result<Self, ExecutionFailure> {
        if let Expr::Call { name, args, .. } = expression {
            if name == BIN || name == SPAN {
                let width = args
                    .iter()
                    .filter_map(|argument| match argument {
                        Expr::Literal(value) => duration_ms(&value.rendered()),
                        Expr::Field(field) => duration_ms(field),
                        _ => None,
                    })
                    .next_back()
                    .ok_or_else(|| {
                        ExecutionFailure::new("a bin groups by a span of time, such as bin(5m)")
                    })?;
                let field = args
                    .iter()
                    .find_map(|argument| match argument {
                        Expr::Field(field) if duration_ms(field).is_none() => Some(field.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| FIELD_TIMESTAMP.to_string());
                return Ok(Self::Bin { field, width });
            }
        }
        Ok(Self::Value(expression.clone()))
    }

    /// This key's value for one record.
    fn key(&self, record: &Record, budget: &Budget) -> String {
        match self {
            Self::Value(expression) => {
                expression::evaluate(expression, record, budget).rendered()
            }
            Self::Bin { field, width } => {
                let timestamp = record
                    .get(field)
                    .and_then(super::parse_timestamp)
                    .unwrap_or_default();
                let floored = timestamp - timestamp.rem_euclid(*width);
                super::format_timestamp(floored)
            }
        }
    }
}

/// The function that groups records into spans of time, as Logs Insights QL writes it.
const BIN: &str = "bin";

/// The same grouping, as PPL writes it.
const SPAN: &str = "span";

/// The span of time a duration is written as: a number and a unit, as a `bin` command writes
/// one (`bin(30s)`, `bin(5m)`, `bin(1h)`).
fn duration_ms(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    let digits: String = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let unit = trimmed[digits.len()..].trim().to_lowercase();
    let count = digits.parse::<f64>().ok()?;
    let millis = match unit.as_str() {
        "ms" => 1.0,
        "s" | "sec" | "secs" | "second" | "seconds" => 1_000.0,
        // No unit at all is a span in MINUTES, the unit a bin is written in by default.
        "" | "m" | "min" | "mins" | "minute" | "minutes" => 60_000.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000.0,
        "d" | "day" | "days" => 86_400_000.0,
        _ => return None,
    };
    let span = count * millis;
    (span >= 1.0).then_some(span as i64)
}

/// Split a command's argument text on its TOP-LEVEL commas: the separator of a list of
/// fields, aggregates or sort keys. A comma inside brackets, a quoted string or a
/// regular-expression literal belongs to what it sits in.
fn split_items(text: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (index, character) in text.chars().enumerate() {
        if let Some(delimiter) = quote {
            current.push(character);
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => {
                quote = Some(character);
                current.push(character);
            }
            // A `/` opens a regular-expression literal only when another `/` closes it; a
            // lone one is the division operator and belongs to the item it sits in.
            '/' if text.chars().skip(index + 1).any(|rest| rest == '/') => {
                quote = Some(character);
                current.push(character);
            }
            '(' | '[' => {
                depth = depth.saturating_add(1);
                current.push(character);
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                items.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items.retain(|item| !item.is_empty());
    items
}

/// Split one list item into the text that computes it and the name its `as` clause gives it.
fn split_alias(item: &str) -> (String, Option<String>) {
    let (text, mut names) = split_alias_list(item);
    (text, names.pop())
}

/// Split one item into the text that computes it and ALL the names its `as` clause gives it:
/// a glob `parse` names one field per `*` (`parse @message "* *" as user, level`).
fn split_alias_list(item: &str) -> (String, Vec<String>) {
    let (before, after) = split_on_keyword(item, AS);
    let names = after
        .map(|names| {
            split_items(&names)
                .iter()
                .map(|name| super::trim_identifier(name).to_string())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    (before, names)
}

/// Split `text` at its first TOP-LEVEL occurrence of the whole word `keyword`, answering what
/// came before it and what follows it (`None` when the word is not there).
///
/// Top-level means outside brackets, quoted strings and regular-expression literals, so the
/// `as` of `strlen(message) as length` splits and a field called `last` does not.
fn split_on_keyword(text: &str, keyword: &str) -> (String, Option<String>) {
    let characters: Vec<char> = text.chars().collect();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut index = 0usize;
    while index < characters.len() {
        let character = characters[index];
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        match character {
            '"' | '\'' | '`' => quote = Some(character),
            // See [`split_items`]: a `/` nothing closes is the division operator.
            '/' if characters[index + 1..].contains(&'/') => quote = Some(character),
            '(' | '[' => depth = depth.saturating_add(1),
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        let word_boundary = index == 0 || characters[index - 1].is_whitespace();
        if depth == 0 && quote.is_none() && word_boundary {
            let word: String = characters[index..]
                .iter()
                .take(keyword.chars().count())
                .collect();
            let ends = characters
                .get(index + keyword.chars().count())
                .is_none_or(|character| character.is_whitespace());
            if ends && word.eq_ignore_ascii_case(keyword) {
                let before: String = characters[..index].iter().collect();
                let after: String = characters[index + keyword.chars().count()..].iter().collect();
                return (before.trim().to_string(), Some(after.trim().to_string()));
            }
        }
        index += 1;
    }
    (text.trim().to_string(), None)
}
