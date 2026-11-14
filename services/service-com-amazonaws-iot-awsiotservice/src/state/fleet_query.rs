// SPDX-License-Identifier: CC0-1.0

//! Whether a FLEET INDEXING QUERY STRING is one this service can read at all.
//!
//! Several operations take a `queryString` over a fleet index — `CreateFleetMetric`
//! aggregates over the things one selects, `CreateDynamicThingGroup` makes a group's
//! membership the things one selects — and every one of them models
//! `InvalidQueryException` ("The query is invalid", HTTP 400) for a string that is not
//! a query. The reading of such a string is stated ONCE here so those operations
//! cannot drift apart on what they accept.
//!
//! The syntax is the fleet-indexing query syntax: whitespace-separated TERMS, each
//! either a bare value or a `field:value` pair (whose value may be a quoted phrase, a
//! `[low TO high]` / `{low TO high}` range or carry the `*` and `?` wildcards),
//! combined with the `AND` / `OR` / `NOT` operators and grouped with parentheses.
//! What is checked here is the FORM — that the delimiters close, that every operator
//! has the terms it joins, and that every term names something — and never which
//! fields an index happens to hold: a query naming a field nothing is indexed under
//! selects no things, which is a result and not a refusal.
//!
//! Nothing here is a hand-rolled copy of a published grammar's parser: the fleet
//! indexing query language is AWS IoT's own, so there is no crates.io parser for it,
//! and the walk below is a single bounded pass that answers one question (is this
//! readable as a query) rather than an expression evaluator.

/// How many BYTES of query string this service reads.
///
/// `QueryString` is modelled `length 1..` with no maximum
/// (`smithy_query(query="shape",shape="QueryString")`), and a record keeps the query
/// verbatim so a reader hands back what the writer sent — so without a ceiling one
/// request would choose how large a single record is. It is the emulator's own bound,
/// far above any fleet-indexing query written in practice.
pub const MAX_QUERY_STRING_BYTES: usize = 4 * 1024;

/// How deeply a query's parentheses may nest.
///
/// The walk below is iterative, so nesting costs no stack; this bounds what one
/// request may make the reader count, for the same reason
/// [`MAX_QUERY_STRING_BYTES`] bounds its length.
pub const MAX_QUERY_NESTING_DEPTH: usize = 32;

/// The operators that JOIN two terms of a fleet-indexing query, so one of them with
/// no term on either side is a query that says nothing.
const BINARY_OPERATORS: &[&str] = &["AND", "OR", "&&", "||"];

/// The operators that NEGATE the term after them, which is why one may open a query
/// but never close it.
const UNARY_OPERATORS: &[&str] = &["NOT", "!"];

/// Why a string is not a fleet-indexing query.
///
/// Each variant is a DIFFERENT refusal, so the caller is told which rule the string
/// broke rather than one fixed sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryStringRejection {
    /// It carries no term at all — it is empty or only whitespace.
    Empty,
    /// It is longer than [`MAX_QUERY_STRING_BYTES`]; carries its length in bytes.
    TooManyBytes(usize),
    /// A `(` is never closed, or a `)` closes a group that was never opened.
    UnbalancedParentheses,
    /// A range's `[` / `{` is never closed by its `]` / `}`.
    UnbalancedRange,
    /// A quoted phrase is opened and never closed.
    UnterminatedQuote,
    /// Its parentheses nest deeper than [`MAX_QUERY_NESTING_DEPTH`].
    TooDeeplyNested,
    /// An operator has no term to join or negate; carries the operator.
    DanglingOperator(String),
    /// A `field:value` term is missing one half; carries the term.
    IncompleteTerm(String),
}

/// How this service states that a `queryString` is not a query it can read.
///
/// FIXED text with no variable part: a query that does not parse is refused as a
/// syntax error, and nothing of the request is echoed back into the message.
pub const UNPARSABLE_QUERY_MESSAGE: &str = "Unable to parse query, invalid syntax";

impl QueryStringRejection {
    /// What to tell the caller.
    ///
    /// Every way a string fails to PARSE is one refusal — the syntax is either read or
    /// not, so the caller is told that and not which token the reader stopped at
    /// ([`UNPARSABLE_QUERY_MESSAGE`]). The two emulator ceilings on what one query may
    /// cost are not syntax, so each states its own bound.
    pub fn message(&self) -> String {
        match self {
            Self::TooManyBytes(bytes) => format!(
                "queryString must be at most {MAX_QUERY_STRING_BYTES} bytes long; this one is \
                 {bytes}"
            ),
            Self::TooDeeplyNested => format!(
                "queryString must not nest more than {MAX_QUERY_NESTING_DEPTH} levels of \
                 parentheses"
            ),
            Self::Empty
            | Self::UnbalancedParentheses
            | Self::UnbalancedRange
            | Self::UnterminatedQuote
            | Self::DanglingOperator(_)
            | Self::IncompleteTerm(_) => UNPARSABLE_QUERY_MESSAGE.to_string(),
        }
    }
}

/// One piece of a query, as the single pass below reads it.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    /// A `(` or a `)`.
    GroupOpen,
    /// The closing half of the above.
    GroupClose,
    /// An operator joining two terms (`AND`, `OR`).
    Binary(String),
    /// An operator negating the term after it (`NOT`).
    Unary(String),
    /// Everything else: a term, as written.
    Term(String),
}

/// Check `query` against the fleet-indexing query rules, naming the first rule it
/// breaks.
///
/// Shared by every operation that takes a `queryString`, so `CreateFleetMetric` and
/// `CreateDynamicThingGroup` cannot drift apart on which queries they accept.
pub fn validate_query_string(query: &str) -> Result<(), QueryStringRejection> {
    if query.len() > MAX_QUERY_STRING_BYTES {
        return Err(QueryStringRejection::TooManyBytes(query.len()));
    }
    let tokens = tokenize(query)?;
    if tokens.is_empty() {
        return Err(QueryStringRejection::Empty);
    }
    check_token_order(&tokens)
}

/// Split `query` into [`Token`]s, refusing the delimiter faults a single pass sees:
/// an unterminated quote, an unclosed range and parentheses that do not balance.
fn tokenize(query: &str) -> Result<Vec<Token>, QueryStringRejection> {
    let mut tokens = Vec::new();
    let mut pending = String::new();
    let mut depth = 0usize;
    let mut range_depth = 0usize;
    let mut quoted = false;

    for character in query.chars() {
        if quoted {
            pending.push(character);
            if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => {
                quoted = true;
                pending.push(character);
            }
            '[' | '{' => {
                range_depth += 1;
                pending.push(character);
            }
            ']' | '}' => {
                if range_depth == 0 {
                    return Err(QueryStringRejection::UnbalancedRange);
                }
                range_depth -= 1;
                pending.push(character);
            }
            // Inside a range the parentheses and the whitespace are the range's own.
            _ if range_depth > 0 => pending.push(character),
            '(' => {
                push_term(&mut tokens, &mut pending)?;
                depth += 1;
                if depth > MAX_QUERY_NESTING_DEPTH {
                    return Err(QueryStringRejection::TooDeeplyNested);
                }
                tokens.push(Token::GroupOpen);
            }
            ')' => {
                push_term(&mut tokens, &mut pending)?;
                if depth == 0 {
                    return Err(QueryStringRejection::UnbalancedParentheses);
                }
                depth -= 1;
                tokens.push(Token::GroupClose);
            }
            _ if character.is_whitespace() => push_term(&mut tokens, &mut pending)?,
            _ => pending.push(character),
        }
    }
    if quoted {
        return Err(QueryStringRejection::UnterminatedQuote);
    }
    if range_depth > 0 {
        return Err(QueryStringRejection::UnbalancedRange);
    }
    push_term(&mut tokens, &mut pending)?;
    if depth > 0 {
        return Err(QueryStringRejection::UnbalancedParentheses);
    }
    Ok(tokens)
}

/// Close off the term the pass has been reading, if any, classifying it as an
/// operator or a search term.
///
/// A `field:value` term has to name BOTH halves: a trailing `:` asks for every value
/// of a field without saying which, and a leading one names no field, so neither is a
/// term this service can read.
fn push_term(tokens: &mut Vec<Token>, pending: &mut String) -> Result<(), QueryStringRejection> {
    if pending.is_empty() {
        return Ok(());
    }
    let term = std::mem::take(pending);
    if BINARY_OPERATORS.contains(&term.as_str()) {
        tokens.push(Token::Binary(term));
        return Ok(());
    }
    if UNARY_OPERATORS.contains(&term.as_str()) {
        tokens.push(Token::Unary(term));
        return Ok(());
    }
    if let Some((field, value)) = term.split_once(':') {
        if field.is_empty() || value.is_empty() {
            return Err(QueryStringRejection::IncompleteTerm(term));
        }
    }
    tokens.push(Token::Term(term));
    Ok(())
}

/// Check that every operator has the terms it applies to: a query neither opens nor
/// closes with an operator that joins two terms, no two such operators follow one
/// another, and a negation is followed by something to negate.
fn check_token_order(tokens: &[Token]) -> Result<(), QueryStringRejection> {
    let mut previous: Option<&Token> = None;
    for token in tokens {
        match token {
            Token::Binary(operator) => {
                let joins_a_term = matches!(previous, Some(Token::Term(_) | Token::GroupClose));
                if !joins_a_term {
                    return Err(QueryStringRejection::DanglingOperator(operator.clone()));
                }
            }
            Token::Term(_) | Token::GroupOpen | Token::Unary(_) | Token::GroupClose => {}
        }
        previous = Some(token);
    }
    match previous {
        Some(Token::Binary(operator) | Token::Unary(operator)) => {
            Err(QueryStringRejection::DanglingOperator(operator.clone()))
        }
        _ => Ok(()),
    }
}
