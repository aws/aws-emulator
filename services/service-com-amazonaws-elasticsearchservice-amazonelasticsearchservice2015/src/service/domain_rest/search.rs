// SPDX-License-Identifier: CC0-1.0

//! The QUERY a search of a domain's indexes SELECTS its documents with: the
//! vocabulary this cluster reads, and what each clause of it holds of one stored
//! document.
//!
//! A search body's `query` names a tree of clauses, and every hit a search
//! answers with is a document that tree HOLDS — computed from the document's own
//! stored source, never from the request. The vocabulary served is the one this
//! emulator can answer out of a stored document: the match-all, the term and
//! terms a field's value is compared against, the ids a document is addressed
//! by, the analyzed `match` and `match_phrase` over a text field's own terms, a
//! prefix, whether a field exists at all, a range, and the boolean composition
//! of all of them. A clause outside it is REFUSED ([`Unreadable`]) rather than
//! answered as if it selected everything, which would report hits no query asked
//! for.
//!
//! Both bounds on the tree are the caller's to drive, so both are enforced while
//! it is read: how deep it nests ([`MAX_QUERY_DEPTH`]) and how many clauses it
//! names in all ([`MAX_QUERY_CLAUSES`]).

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// The score every hit of this cluster carries.
///
/// A relevance score is a function of the index's own term statistics, which
/// this emulator does not keep; what it can say about a document is that the
/// query holds of it, so every hit carries the same score rather than a figure
/// invented per document.
pub(super) const MATCH_SCORE: f64 = 1.0;

/// How deep a query tree may nest before it is refused.
const MAX_QUERY_DEPTH: usize = 20;

/// How many clauses one query may name in all — the engine's own
/// `indices.query.bool.max_clause_count` default, which its refusal quotes.
const MAX_QUERY_CLAUSES: usize = 1_024;

/// The engine's error type for a query body it cannot read.
pub(super) const PARSING_EXCEPTION: &str = "parsing_exception";

/// The engine's error type for a query naming more clauses than it evaluates.
pub(super) const TOO_MANY_CLAUSES: &str = "too_many_clauses";

/// What a query could not be read as: the engine's error type for it, and the
/// reason it states.
#[derive(Debug)]
pub(super) struct Unreadable {
    pub(super) error_type: &'static str,
    pub(super) reason: String,
}

/// One clause of a query tree.
#[derive(Debug)]
pub(super) enum Selector {
    /// `match_all`, and the query a body names none of: every document.
    All,
    /// `match_none`: no document.
    None,
    /// `term` / `terms`: a field carrying one of these values exactly.
    Values { field: String, values: Vec<Value> },
    /// `ids`: a document whose `_id` the query names.
    Ids(BTreeSet<String>),
    /// `match`: a field whose own terms carry EVERY term the query names.
    Match { field: String, terms: Vec<String> },
    /// `match_phrase`: a field whose terms carry the query's terms CONSECUTIVELY.
    Phrase { field: String, terms: Vec<String> },
    /// `prefix`: a field one of whose terms begins with this one.
    Prefix { field: String, prefix: String },
    /// `exists`: a document carrying any value at this field.
    Exists { field: String },
    /// `range`: a field whose value falls inside these bounds.
    Range { field: String, bounds: Bounds },
    /// `bool`: the composition of the clauses below.
    Bool(Box<BoolClauses>),
}

/// The four clause lists a `bool` query composes, and how many of its `should`
/// clauses a document has to hold.
#[derive(Debug, Default)]
pub(super) struct BoolClauses {
    must: Vec<Selector>,
    filter: Vec<Selector>,
    should: Vec<Selector>,
    must_not: Vec<Selector>,
    /// How many `should` clauses must hold. The engine's own default: one when
    /// the query names no other clause type, none when it does.
    minimum_should_match: usize,
}

/// The bounds a `range` clause states, each inclusive or exclusive.
#[derive(Debug, Default)]
pub(super) struct Bounds {
    lower: Option<(Value, bool)>,
    upper: Option<(Value, bool)>,
}

impl Selector {
    /// The clause tree `query` names — the `query` member of a search body, or
    /// `None` for a body naming none, which selects every document.
    pub(super) fn of(query: Option<&Value>) -> Result<Self, Unreadable> {
        let mut clauses = 0_usize;
        match query {
            None | Some(Value::Null) => Ok(Self::All),
            Some(query) => read(query, 0, &mut clauses),
        }
    }

    /// Whether this clause holds of the document `source` stored under `id`.
    pub(super) fn selects(&self, id: &str, source: &Value) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Values { field, values } => held_values(source, field)
                .iter()
                .any(|held| values.iter().any(|wanted| same_value(held, wanted))),
            Self::Ids(ids) => ids.contains(id),
            Self::Match { field, terms } => {
                let held = field_terms(source, field);
                !terms.is_empty() && terms.iter().all(|term| held.contains(term))
            }
            Self::Phrase { field, terms } => {
                !terms.is_empty() && carries_phrase(&field_terms(source, field), terms)
            }
            Self::Prefix { field, prefix } => field_terms(source, field)
                .iter()
                .any(|term| term.starts_with(prefix)),
            Self::Exists { field } => !held_values(source, field).is_empty(),
            Self::Range { field, bounds } => held_values(source, field)
                .iter()
                .any(|held| bounds.hold(held)),
            Self::Bool(clauses) => clauses.selects(id, source),
        }
    }
}

impl BoolClauses {
    /// Whether this composition holds: every `must` and `filter` clause, no
    /// `must_not` clause, and at least [`Self::minimum_should_match`] of the
    /// `should` clauses.
    fn selects(&self, id: &str, source: &Value) -> bool {
        if !self
            .must
            .iter()
            .chain(self.filter.iter())
            .all(|clause| clause.selects(id, source))
        {
            return false;
        }
        if self
            .must_not
            .iter()
            .any(|clause| clause.selects(id, source))
        {
            return false;
        }
        let matched = self
            .should
            .iter()
            .filter(|clause| clause.selects(id, source))
            .count();
        matched >= self.minimum_should_match
    }
}

impl Bounds {
    /// Whether `held` falls inside these bounds.
    fn hold(&self, held: &Value) -> bool {
        let lower = match &self.lower {
            None => true,
            Some((bound, inclusive)) => match compare(held, bound) {
                None => false,
                Some(::std::cmp::Ordering::Greater) => true,
                Some(::std::cmp::Ordering::Equal) => *inclusive,
                Some(::std::cmp::Ordering::Less) => false,
            },
        };
        let upper = match &self.upper {
            None => true,
            Some((bound, inclusive)) => match compare(held, bound) {
                None => false,
                Some(::std::cmp::Ordering::Less) => true,
                Some(::std::cmp::Ordering::Equal) => *inclusive,
                Some(::std::cmp::Ordering::Greater) => false,
            },
        };
        lower && upper
    }
}

/// Read one clause of a query tree, counting it against the clause budget and
/// refusing a tree deeper than this cluster evaluates.
fn read(query: &Value, depth: usize, clauses: &mut usize) -> Result<Selector, Unreadable> {
    if depth > MAX_QUERY_DEPTH {
        return Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!(
                "query nests deeper than the maximum of {MAX_QUERY_DEPTH} this cluster evaluates"
            ),
        });
    }
    *clauses = clauses.saturating_add(1);
    if *clauses > MAX_QUERY_CLAUSES {
        return Err(Unreadable {
            error_type: TOO_MANY_CLAUSES,
            reason: format!("maxClauseCount is set to {MAX_QUERY_CLAUSES}"),
        });
    }
    let Value::Object(members) = query else {
        return Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: "query malformed, expected a query object".to_string(),
        });
    };
    let (name, body) = single_member(members, "query")?;
    match name.as_str() {
        "match_all" => Ok(Selector::All),
        "match_none" => Ok(Selector::None),
        "ids" => read_ids(body),
        "exists" => {
            let field = text_member(body, "exists", "field")?;
            Ok(Selector::Exists { field })
        }
        "term" => {
            let (field, value) = single_field(body, "term")?;
            Ok(Selector::Values {
                field,
                values: vec![term_value(value)],
            })
        }
        "terms" => read_terms(body),
        "match" | "match_phrase" => {
            let (field, value) = single_field(body, &name)?;
            let text = match_text(&field, &name, value)?;
            let terms = analyzed_terms(&text);
            if name == "match_phrase" {
                Ok(Selector::Phrase { field, terms })
            } else {
                Ok(Selector::Match { field, terms })
            }
        }
        "prefix" => {
            let (field, value) = single_field(body, "prefix")?;
            let prefix = match_text(&field, "prefix", value)?.to_lowercase();
            Ok(Selector::Prefix { field, prefix })
        }
        "range" => read_range(body),
        "bool" => read_bool(body, depth, clauses),
        other => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("no [query] registered for [{other}]"),
        }),
    }
}

/// The one member `members` carries, or the refusal for an object naming no
/// clause or several where one belongs.
fn single_member<'m>(
    members: &'m Map<String, Value>,
    at: &str,
) -> Result<(String, &'m Value), Unreadable> {
    let mut entries = members.iter();
    match (entries.next(), entries.next()) {
        (Some((name, body)), None) => Ok((name.clone(), body)),
        (Some((first, _)), Some((second, _))) => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!(
                "[{at}] query doesn't support multiple fields, found [{first}] and [{second}]"
            ),
        }),
        _ => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[{at}] query malformed, expected a clause but found none"),
        }),
    }
}

/// The one field a field-level clause is about, and the value it states for it.
fn single_field<'b>(body: &'b Value, at: &str) -> Result<(String, &'b Value), Unreadable> {
    let Value::Object(members) = body else {
        return Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[{at}] query malformed, expected an object naming one field"),
        });
    };
    single_member(members, at)
}

/// The text a `match`-family clause states for its field: the value itself, or
/// the `query` member of the object form.
fn match_text(field: &str, at: &str, value: &Value) -> Result<String, Unreadable> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Object(members) => match members.get("query") {
            Some(Value::String(text)) => Ok(text.clone()),
            Some(Value::Number(number)) => Ok(number.to_string()),
            Some(Value::Bool(flag)) => Ok(flag.to_string()),
            _ => Err(Unreadable {
                error_type: PARSING_EXCEPTION,
                reason: format!("[{at}] query does not state a [query] for field [{field}]"),
            }),
        },
        _ => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[{at}] query states no text for field [{field}]"),
        }),
    }
}

/// The value a `term` clause compares against: the value itself, or the `value`
/// member of the object form the engine also takes.
fn term_value(value: &Value) -> Value {
    match value {
        Value::Object(members) => members.get("value").cloned().unwrap_or(Value::Null),
        other => other.clone(),
    }
}

/// A `terms` clause: one field, and the list of values it names.
fn read_terms(body: &Value) -> Result<Selector, Unreadable> {
    let (field, value) = single_field(body, "terms")?;
    match value {
        Value::Array(values) => Ok(Selector::Values {
            field,
            values: values.clone(),
        }),
        _ => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[terms] query states no list of values for field [{field}]"),
        }),
    }
}

/// An `ids` clause: the document ids it names.
fn read_ids(body: &Value) -> Result<Selector, Unreadable> {
    let values = match body {
        Value::Object(members) => members.get("values"),
        _ => None,
    };
    match values {
        Some(Value::Array(values)) => Ok(Selector::Ids(
            values
                .iter()
                .map(|value| match value {
                    Value::String(id) => id.clone(),
                    other => other.to_string(),
                })
                .collect(),
        )),
        _ => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: "[ids] query states no [values]".to_string(),
        }),
    }
}

/// A `range` clause: one field, and the bounds it states for it.
fn read_range(body: &Value) -> Result<Selector, Unreadable> {
    let (field, value) = single_field(body, "range")?;
    let Value::Object(stated) = value else {
        return Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[range] query states no bounds for field [{field}]"),
        });
    };
    let mut bounds = Bounds::default();
    for (bound, value) in stated {
        match bound.as_str() {
            "gte" => bounds.lower = Some((value.clone(), true)),
            "gt" => bounds.lower = Some((value.clone(), false)),
            "lte" => bounds.upper = Some((value.clone(), true)),
            "lt" => bounds.upper = Some((value.clone(), false)),
            // `boost` and `format` do not narrow which documents the clause
            // holds of, so a clause naming one is read without it.
            "boost" | "format" => {}
            other => {
                return Err(Unreadable {
                    error_type: PARSING_EXCEPTION,
                    reason: format!("[range] query does not support [{other}]"),
                })
            }
        }
    }
    Ok(Selector::Range { field, bounds })
}

/// A `bool` clause: the four clause lists it composes, and how many of its
/// `should` clauses a document has to hold.
fn read_bool(body: &Value, depth: usize, clauses: &mut usize) -> Result<Selector, Unreadable> {
    let Value::Object(members) = body else {
        return Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: "[bool] query malformed, expected an object of clause lists".to_string(),
        });
    };
    let mut composed = BoolClauses::default();
    let mut stated_minimum: Option<usize> = None;
    for (name, value) in members {
        let list = match name.as_str() {
            "must" => &mut composed.must,
            "filter" => &mut composed.filter,
            "should" => &mut composed.should,
            "must_not" => &mut composed.must_not,
            "minimum_should_match" => {
                stated_minimum = Some(minimum_should_match(value)?);
                continue;
            }
            // A boost changes a hit's score, which this cluster states per hit
            // rather than per clause, so it narrows nothing here.
            "boost" => continue,
            other => {
                return Err(Unreadable {
                    error_type: PARSING_EXCEPTION,
                    reason: format!("[bool] query does not support [{other}]"),
                })
            }
        };
        match value {
            Value::Array(stated) => {
                for clause in stated {
                    list.push(read(clause, depth.saturating_add(1), clauses)?);
                }
            }
            single => list.push(read(single, depth.saturating_add(1), clauses)?),
        }
    }
    // The engine's own default: one `should` clause has to hold when the query
    // names nothing else, and none when it does.
    let names_others = !composed.must.is_empty()
        || !composed.filter.is_empty()
        || !composed.must_not.is_empty();
    composed.minimum_should_match = match stated_minimum {
        Some(stated) => stated.min(composed.should.len()),
        None if names_others || composed.should.is_empty() => 0,
        None => 1,
    };
    Ok(Selector::Bool(Box::new(composed)))
}

/// The `minimum_should_match` a `bool` query states: a whole number, or the
/// string form of one.
fn minimum_should_match(value: &Value) -> Result<usize, Unreadable> {
    let stated = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse::<i64>().ok(),
        _ => None,
    };
    match stated {
        Some(stated) => Ok(usize::try_from(stated.max(0)).unwrap_or(0)),
        None => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: "[bool] query states a [minimum_should_match] that is not a number"
                .to_string(),
        }),
    }
}

/// The text member a clause states at `member`.
fn text_member(body: &Value, at: &str, member: &str) -> Result<String, Unreadable> {
    match body.get(member) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(Unreadable {
            error_type: PARSING_EXCEPTION,
            reason: format!("[{at}] query states no [{member}]"),
        }),
    }
}

/// The values a document holds at `field`, which is a DOTTED path into its
/// source: one value for a scalar member, every element for a list, and none at
/// all for a member the document does not carry.
fn held_values(source: &Value, field: &str) -> Vec<Value> {
    let mut reached: Vec<&Value> = vec![source];
    for step in field.split('.') {
        let mut next: Vec<&Value> = Vec::new();
        for value in reached {
            match value {
                Value::Object(members) => next.extend(members.get(step)),
                Value::Array(elements) => {
                    for element in elements {
                        if let Value::Object(members) = element {
                            next.extend(members.get(step));
                        }
                    }
                }
                _ => {}
            }
        }
        reached = next;
    }
    let mut held: Vec<Value> = Vec::new();
    for value in reached {
        match value {
            Value::Array(elements) => held.extend(elements.iter().cloned()),
            Value::Null => {}
            scalar => held.push(scalar.clone()),
        }
    }
    held
}

/// The terms a field's values produce for an analyzed clause: each value's
/// alphanumeric runs, case-folded — the tokenization an ordinary text field's
/// standard analysis produces, which is what a `match` clause is compared
/// against.
fn field_terms(source: &Value, field: &str) -> Vec<String> {
    held_values(source, field)
        .iter()
        .flat_map(|value| analyzed_terms(&scalar_text(value)))
        .collect()
}

/// The terms `text` produces: its alphanumeric runs, case-folded, in order.
pub(super) fn analyzed_terms(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|run| !run.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// One stored value as the text an analyzed clause reads it as.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

/// Whether `held` carries `phrase` as consecutive terms.
fn carries_phrase(held: &[String], phrase: &[String]) -> bool {
    if phrase.len() > held.len() {
        return false;
    }
    held.windows(phrase.len()).any(|window| window == phrase)
}

/// Whether a stored value and a queried one are the SAME value: equal as JSON,
/// or equal as the text a scalar is written with — so a number stored as a
/// number is found by a query naming it as a string, which is how a client that
/// read the document back states it.
fn same_value(held: &Value, wanted: &Value) -> bool {
    if held == wanted {
        return true;
    }
    match (held, wanted) {
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => false,
        _ => scalar_text(held) == scalar_text(wanted),
    }
}

/// How a stored value orders against a range bound: as numbers when both are
/// numbers, else as the text each is written with. `None` for values that do not
/// order against each other at all, which no bound holds of.
fn compare(held: &Value, bound: &Value) -> Option<::std::cmp::Ordering> {
    if let (Some(held), Some(bound)) = (number_of(held), number_of(bound)) {
        return held.partial_cmp(&bound);
    }
    match (held, bound) {
        (Value::Object(_) | Value::Array(_) | Value::Null, _)
        | (_, Value::Object(_) | Value::Array(_) | Value::Null) => None,
        _ => Some(scalar_text(held).cmp(&scalar_text(bound))),
    }
}

/// The number a value states, as a number or as the text of one.
fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// RULE: a query selects exactly the documents the clause tree holds of, over
    /// the whole vocabulary this cluster reads.
    #[test]
    fn each_clause_selects_the_documents_it_holds_of() {
        let source = json!({
            "title": "Sim2Real Behaviour",
            "count": 7,
            "tags": ["alpha", "beta"],
            "nested": { "field": "inner value" }
        });
        // (the case, the query, whether it selects the document above)
        let cases: &[(&str, Value, bool)] = &[
            ("match_all", json!({"match_all": {}}), true),
            ("match_none", json!({"match_none": {}}), false),
            ("match on one term", json!({"match": {"title": "sim2real"}}), true),
            (
                "match on every term",
                json!({"match": {"title": "behaviour sim2real"}}),
                true,
            ),
            ("match on a missing term", json!({"match": {"title": "absent"}}), false),
            (
                "match_phrase in order",
                json!({"match_phrase": {"title": "sim2real behaviour"}}),
                true,
            ),
            (
                "match_phrase out of order",
                json!({"match_phrase": {"title": "behaviour sim2real"}}),
                false,
            ),
            ("term on the whole value", json!({"term": {"title": "Sim2Real Behaviour"}}), true),
            ("term on a number", json!({"term": {"count": 7}}), true),
            ("term on a list element", json!({"term": {"tags": "beta"}}), true),
            ("term on another value", json!({"term": {"count": 8}}), false),
            ("terms over a list", json!({"terms": {"tags": ["beta", "gamma"]}}), true),
            ("prefix of a term", json!({"prefix": {"title": "sim"}}), true),
            ("exists on a nested field", json!({"exists": {"field": "nested.field"}}), true),
            ("exists on a missing field", json!({"exists": {"field": "absent"}}), false),
            ("range holding the value", json!({"range": {"count": {"gte": 7, "lt": 9}}}), true),
            ("range excluding the value", json!({"range": {"count": {"gt": 7}}}), false),
            (
                "bool must and must_not",
                json!({"bool": {
                    "must": [{"match": {"title": "sim2real"}}],
                    "must_not": [{"term": {"count": 8}}]
                }}),
                true,
            ),
            (
                "bool should with none holding",
                json!({"bool": {"should": [{"term": {"count": 8}}]}}),
                false,
            ),
            (
                "bool should with one holding",
                json!({"bool": {"should": [{"term": {"count": 7}}, {"term": {"count": 8}}]}}),
                true,
            ),
            (
                "bool minimum_should_match of two",
                json!({"bool": {
                    "minimum_should_match": 2,
                    "should": [{"term": {"count": 7}}, {"term": {"count": 8}}]
                }}),
                false,
            ),
            ("ids naming the document", json!({"ids": {"values": ["1"]}}), true),
            ("ids naming another", json!({"ids": {"values": ["2"]}}), false),
        ];
        for (case, query, expected) in cases {
            let selector = Selector::of(Some(query))
                .unwrap_or_else(|failed| panic!("case {case:?}: expected a readable query, got {failed:?}"));
            let selected = selector.selects("1", &source);
            assert_eq!(
                selected, *expected,
                "case {case:?}: expected selects {expected}, got {selected}"
            );
        }
    }

    /// RULE: a clause this cluster does not read is refused, never answered as if
    /// it selected everything.
    #[test]
    fn a_clause_outside_the_vocabulary_is_refused() {
        // (the case, the query, the error type its refusal carries)
        let cases: &[(&str, Value, &str)] = &[
            ("an unregistered query", json!({"fuzzy": {"title": "sim"}}), PARSING_EXCEPTION),
            ("two queries where one belongs", json!({"match_all": {}, "match_none": {}}), PARSING_EXCEPTION),
            ("no query at all", json!({}), PARSING_EXCEPTION),
            ("a query that is not an object", json!("match_all"), PARSING_EXCEPTION),
            (
                "an unsupported bool section",
                json!({"bool": {"nope": []}}),
                PARSING_EXCEPTION,
            ),
        ];
        for (case, query, error_type) in cases {
            let read = Selector::of(Some(query));
            match read {
                Ok(selector) => panic!("case {case:?}: expected a refusal, got {selector:?}"),
                Err(refusal) => assert_eq!(
                    refusal.error_type, *error_type,
                    "case {case:?}: expected the type {error_type:?}, got {refusal:?}"
                ),
            }
        }
    }

    /// RULE: a query tree past either bound this cluster evaluates is refused —
    /// the depth it nests and the clauses it names.
    #[test]
    fn a_query_past_a_bound_this_cluster_evaluates_is_refused() {
        // Nested one level deeper than the maximum, and a flat query naming one
        // clause more than the maximum.
        let mut deep = json!({"match_all": {}});
        for _ in 0..=MAX_QUERY_DEPTH {
            deep = json!({"bool": {"must": [deep]}});
        }
        let wide = json!({"bool": {
            "should": (0..=MAX_QUERY_CLAUSES).map(|_| json!({"match_all": {}})).collect::<Vec<Value>>()
        }});
        let cases: &[(&str, Value, &str)] = &[
            ("past the nesting bound", deep, PARSING_EXCEPTION),
            ("past the clause bound", wide, TOO_MANY_CLAUSES),
        ];
        for (case, query, error_type) in cases {
            match Selector::of(Some(query)) {
                Ok(selector) => panic!("case {case:?}: expected a refusal, got {selector:?}"),
                Err(refusal) => assert_eq!(
                    refusal.error_type, *error_type,
                    "case {case:?}: expected the type {error_type:?}, got {refusal:?}"
                ),
            }
        }
    }
}
