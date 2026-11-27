// SPDX-License-Identifier: CC0-1.0

//! The value class a replication rule's repository filter carries: which values ECR
//! ACCEPTS for it, and how one outside the class is refused.
//!
//! What a caller is held to is the expression ECR's own validator PUBLISHES for the class
//! — the one it quotes back in the constraint clause of its refusal — and that expression
//! admits the `*` wildcard in every character class and states the length the value may
//! run to as a leading look-ahead ([`published_filter_expression`]).
//! `smithy_query(query="shape", shape="RepositoryFilterValue")` spells the same class
//! without the wildcard and states its length as a separate `@length(2..256)`, so the
//! model and the service disagree about this one class and the published expression is the
//! rule.
//!
//! That is why the class is judged HERE rather than at the request boundary. The boundary's
//! walk is driven by the model's own constraints, so it would enforce the narrower spelling
//! — `RepositoryFilter$filter` therefore carries `skip_validation` where that shape is
//! declared (`src/ops/types/repository_filter_types.rs`), keeping that spelling off the
//! walk — and the walk's
//! regular-expression engine compiles no look-around, so the published expression cannot be
//! installed there verbatim either. The two halves the look-ahead states are checked
//! separately instead ([`LENGTH`] beside [`BODY`]), and the expression a refusal quotes is
//! built back from those same two halves, so the values accepted and the expression quoted
//! are one rule.
//!
//! The filter's `filterType` is untouched by any of this: its enumerated type is a
//! constraint the model and the service agree on, so the boundary walk still states it.

use ::std::ops::RangeInclusive;
use ::std::sync::LazyLock;

use ::aws_smithy_schema_server::constraints::path;

use crate::ops::types::ReplicationConfiguration;

/// The length a value of this class may run to, as the published expression's leading
/// look-ahead states it (`(?=.{1,256}$)`): at least one character and at most 256. The
/// look-ahead counts CHARACTERS, which is what it is compared against below.
const LENGTH: RangeInclusive<usize> = 1..=256;

/// The BODY of the published expression: what a value has to match once its length is
/// known, which is that expression with the length look-ahead taken out of it — a run of
/// `[a-z0-9*]`, single `.`, `_` or `-` separators between runs, and `/`-separated
/// namespace segments. The `*` is admitted wherever a character of a repository name is.
const BODY: &str = "^(?:[a-z0-9*]+(?:[._-][a-z0-9*]*)*/)*[a-z0-9*]*(?:[._-][a-z0-9*]*)*$";

/// [`BODY`] compiled once for the process, never per request: the expression is a constant
/// of the class.
///
/// `None` for an expression that does not compile, which then admits every value — exactly
/// how the framework's own constraint walk reads a pattern it cannot compile, so no value
/// is refused because of an unreadable expression.
static COMPILED_BODY: LazyLock<Option<::regex::Regex>> =
    LazyLock::new(|| ::regex::Regex::new(BODY).ok());

/// Whether `value` is one the class admits: inside the length the look-ahead states, and
/// matching the body beside it.
fn admits(value: &str) -> bool {
    LENGTH.contains(&value.chars().count())
        && COMPILED_BODY
            .as_ref()
            .is_none_or(|body| body.is_match(value))
}

/// The expression ECR publishes for the class, which is what a refusal quotes: [`BODY`]
/// with the length bound stated as the leading look-ahead ECR writes it as.
///
/// Built from the two halves that are ENFORCED rather than written out beside them, so a
/// refusal cannot quote a rule other than the one the value was held to.
fn published_filter_expression() -> String {
    format!(
        "^(?=.{{{min},{max}}}$){body}",
        min = LENGTH.start(),
        max = LENGTH.end(),
        body = BODY.strip_prefix('^').unwrap_or(BODY),
    )
}

/// The requirement a value outside the class fails, in the `Member must …` wording a
/// modelled constraint is stated in, over the expression ECR publishes.
///
/// The sentence itself is [`::service_common::validation_message::requirement`], the one
/// derivation of it in the tree, so this cannot word a `@pattern` differently from the
/// boundary walk's own refusal.
fn filter_requirement() -> String {
    ::service_common::validation_message::requirement(
        ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            published_filter_expression(),
        ),
    )
}

/// The members the walk descends through to one repository filter of the configuration a
/// put carries, as the model names them
/// (`smithy_query(query="operation", operation="PutReplicationConfiguration")` for the
/// first, then `ReplicationConfiguration$rules`, `ReplicationRule$repositoryFilters` and
/// `RepositoryFilter$filter`).
const PATH_MEMBERS: [&str; 4] = [
    "replicationConfiguration",
    "rules",
    "repositoryFilters",
    "filter",
];

/// How many clauses one refusal states, so a configuration whose every filter is outside
/// the class is answered a bounded message rather than one sized by the request: the model
/// admits 25 rules of 100 filters each, and a caller cannot act on thousands of clauses.
const MAX_CLAUSES: usize = 100;

/// The path the clause for the `filter`-th filter of the `rule`-th rule names, with the
/// 1-based element steps the validating walk spells.
///
/// Spelled through the framework's own path builder
/// ([`::aws_smithy_schema_server::constraints::path`]), the one place a member path is
/// written, so a handler's clause cannot name a position differently from the boundary
/// walk's.
fn filter_path(rule: usize, filter: usize) -> String {
    let [configuration, rules, filters, value] = PATH_MEMBERS;
    path::join(&[
        path::member(configuration),
        path::member(rules),
        path::element(rule),
        path::member(filters),
        path::element(filter),
        path::member(value),
    ])
}

/// The refusal of `configuration` for the repository filters it carries that are outside
/// the class: one clause per filter at fault, each naming the path to that filter and the
/// expression ECR publishes, joined the way this service joins the clauses of one refusal.
///
/// `None` when every filter it carries is one the class admits — a configuration carrying
/// no filters at all included — so the caller goes on to the rules of the configuration
/// this cannot see.
pub fn refused_repository_filters(configuration: &ReplicationConfiguration) -> Option<String> {
    let clauses: Vec<String> = configuration
        .rules
        .iter()
        .enumerate()
        .flat_map(|(rule, replication_rule)| {
            replication_rule
                .repository_filters
                .iter()
                .flatten()
                .enumerate()
                .map(move |(filter, repository_filter)| (rule, filter, repository_filter))
        })
        .filter(|(_, _, repository_filter)| !admits(&repository_filter.filter))
        .take(MAX_CLAUSES)
        .map(|(rule, filter, _)| {
            super::parameter_constraint(&filter_path(rule + 1, filter + 1), &filter_requirement())
        })
        .collect();
    (!clauses.is_empty()).then(|| clauses.join(super::CLAUSE_SEPARATOR))
}
