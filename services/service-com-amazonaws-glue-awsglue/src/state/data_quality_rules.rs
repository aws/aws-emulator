// SPDX-License-Identifier: CC0-1.0

//! What a DQDL rule MEANS, and how one is checked against the rows of the table a data
//! quality run evaluates.
//!
//! A ruleset's text is split into its rules where the rule list is walked
//! ([`dqdl_rules`]); this is where one of those rules is read as a check and carried out
//! over the data that was read ([`TableRows`]). DQDL is proprietary to this service and no
//! published crate parses it, so the reading is here rather than through a dependency.
//!
//! The rule types below are the ones whose truth this emulator can decide from the table's
//! rows: a count of rows or columns, whether a column is there, how complete or unique a
//! column's values are, how many distinct values it holds, and whether every row's value
//! satisfies a condition. A rule of any other type is one this evaluation cannot decide, and
//! answering it `None` is what makes the RUN report that it evaluated nothing rather than
//! claim a pass or a fail nothing computed.

use super::*;

/// How one rule's check came out: whether the rows satisfied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DqdlRuleOutcome {
    /// Whether the rows satisfied the rule.
    pub passed: bool,
}

/// Check the DQDL rule `rule` against `rows`, or `None` when it is not a rule this
/// evaluation can decide from them.
pub fn evaluate_dqdl_rule(rule: &str, rows: &TableRows) -> Option<DqdlRuleOutcome> {
    let tokens = tokenize_dqdl_rule(rule)?;
    let (kind, rest) = tokens.split_first()?;
    let DqdlToken::Word(kind) = kind else {
        return None;
    };
    let passed = match kind.to_ascii_lowercase().as_str() {
        // A count of the rows read, and of the columns the table declares.
        "rowcount" => {
            let count = numeric(rows.rows.len());
            condition_of(rest)?.admits(count)
        }
        "columncount" => {
            let count = numeric(rows.columns.len());
            condition_of(rest)?.admits(count)
        }
        // Whether the table declares the column at all, which is decided off its schema
        // rather than off any row. A rule of this type states nothing beyond the column, so
        // text carrying more than that — a clause this evaluation does not read — is a rule
        // it cannot decide rather than one that failed.
        "columnexists" => {
            let (column, rest) = column_of(rest)?;
            nothing_further(rest)?;
            rows.column_position(&column).is_some()
        }
        // The two rule types that are a fixed condition on a fraction: a complete column
        // has a value in every row, and a unique one has no value twice. Each states nothing
        // beyond its column either, so [`nothing_further`] holds of them too.
        "iscomplete" => {
            let (column, rest) = column_of(rest)?;
            nothing_further(rest)?;
            let cells = rows.column_cells(&column)?;
            completeness(&cells) >= 1.0
        }
        "isunique" => {
            let (column, rest) = column_of(rest)?;
            nothing_further(rest)?;
            let cells = rows.column_cells(&column)?;
            uniqueness(&cells) >= 1.0
        }
        // The same two fractions, and the distinct-value count, under a condition the rule
        // states.
        "completeness" => {
            let (column, rest) = column_of(rest)?;
            let cells = rows.column_cells(&column)?;
            condition_of(rest)?.admits(completeness(&cells))
        }
        "uniqueness" => {
            let (column, rest) = column_of(rest)?;
            let cells = rows.column_cells(&column)?;
            condition_of(rest)?.admits(uniqueness(&cells))
        }
        "distinctvaluescount" => {
            let (column, rest) = column_of(rest)?;
            let cells = rows.column_cells(&column)?;
            condition_of(rest)?.admits(numeric(distinct_values(&cells)))
        }
        // A condition on the VALUES: every row's cell has to satisfy it, so one row that
        // does not is what fails the rule.
        "columnvalues" => {
            let (column, rest) = column_of(rest)?;
            let cells = rows.column_cells(&column)?;
            let condition = value_condition_of(rest)?;
            cells.iter().all(|cell| condition.admits(*cell))
        }
        _ => return None,
    };
    Some(DqdlRuleOutcome { passed })
}

/// A count as the number a condition is checked against.
fn numeric(count: usize) -> f64 {
    count as f64
}

/// The fraction of rows in which the column holds a value: "completeness" as the rule types
/// above name it. A column of no rows at all is complete — nothing is missing.
fn completeness(cells: &[Option<&str>]) -> f64 {
    if cells.is_empty() {
        return 1.0;
    }
    let held = cells.iter().filter(|cell| cell.is_some()).count();
    numeric(held) / numeric(cells.len())
}

/// The fraction of the column's values that occur exactly once: "uniqueness" as the rule
/// types above name it, over the values the column actually holds — a cell holding no value
/// is no value of the column. A column holding no value at all is unique.
fn uniqueness(cells: &[Option<&str>]) -> f64 {
    let held: Vec<&str> = cells.iter().filter_map(|cell| *cell).collect();
    if held.is_empty() {
        return 1.0;
    }
    let mut occurrences: HashMap<&str, usize> = HashMap::new();
    for value in &held {
        *occurrences.entry(value).or_insert(0) += 1;
    }
    let once = occurrences
        .values()
        .filter(|count| **count == 1)
        .count();
    numeric(once) / numeric(held.len())
}

/// How many distinct values the column holds, a cell holding no value counting as none.
fn distinct_values(cells: &[Option<&str>]) -> usize {
    cells
        .iter()
        .filter_map(|cell| *cell)
        .collect::<HashSet<&str>>()
        .len()
}

/// The column a rule names, and the rest of its tokens: a rule addresses a column by a
/// quoted name.
fn column_of(tokens: &[DqdlToken]) -> Option<(String, &[DqdlToken])> {
    let (first, rest) = tokens.split_first()?;
    match first {
        DqdlToken::Text(column) => Some((column.clone(), rest)),
        _ => None,
    }
}

/// `Some` where a rule states nothing beyond what has been read, and `None` where it carries
/// tokens this evaluation does not read.
///
/// A rule type whose whole statement is its column — the three fixed ones — says nothing
/// about text that follows it, and DQDL does put clauses there (a `where` filter, for one),
/// so anything left over is a rule this evaluation cannot decide. Answering `None` is what
/// makes the RUN report that it evaluated nothing, rather than claiming a pass or a fail over
/// a statement it only partly read.
fn nothing_further(rest: &[DqdlToken]) -> Option<()> {
    rest.is_empty().then_some(())
}

/// A condition on a NUMBER, as a rule states it after its metric.
#[derive(Debug, Clone, PartialEq)]
enum DqdlNumericCondition {
    /// A comparison against one number.
    Compare(DqdlComparison, f64),
    /// `between <lower> and <upper>`, which admits the numbers strictly inside the pair:
    /// the bounds themselves are what `>=` and `<=` are for.
    Between(f64, f64),
    /// `in [ … ]`, which admits a number the list holds.
    In(Vec<f64>),
}

impl DqdlNumericCondition {
    /// Whether this condition admits `value`.
    fn admits(&self, value: f64) -> bool {
        match self {
            Self::Compare(comparison, against) => comparison.admits(value, *against),
            Self::Between(lower, upper) => value > *lower && value < *upper,
            Self::In(values) => values.contains(&value),
        }
    }
}

/// The comparisons a condition can state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DqdlComparison {
    Equal,
    NotEqual,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
}

impl DqdlComparison {
    /// The comparison `operator` spells, or `None` for an operator no condition states.
    fn of(operator: &str) -> Option<Self> {
        match operator {
            "=" | "==" => Some(Self::Equal),
            "!=" | "<>" => Some(Self::NotEqual),
            ">" => Some(Self::Greater),
            ">=" => Some(Self::GreaterOrEqual),
            "<" => Some(Self::Less),
            "<=" => Some(Self::LessOrEqual),
            _ => None,
        }
    }

    /// Whether `value` compares to `against` the way this comparison asks.
    fn admits(self, value: f64, against: f64) -> bool {
        match self {
            Self::Equal => value == against,
            Self::NotEqual => value != against,
            Self::Greater => value > against,
            Self::GreaterOrEqual => value >= against,
            Self::Less => value < against,
            Self::LessOrEqual => value <= against,
        }
    }
}

/// The condition `tokens` state on a number, or `None` when they state none this evaluation
/// reads.
fn condition_of(tokens: &[DqdlToken]) -> Option<DqdlNumericCondition> {
    match tokens.split_first()? {
        (DqdlToken::Operator(operator), rest) => {
            let comparison = DqdlComparison::of(operator)?;
            let [DqdlToken::Word(number)] = rest else {
                return None;
            };
            Some(DqdlNumericCondition::Compare(
                comparison,
                number.parse::<f64>().ok()?,
            ))
        }
        (DqdlToken::Word(keyword), rest) => match keyword.to_ascii_lowercase().as_str() {
            "between" => {
                let [DqdlToken::Word(lower), DqdlToken::Word(and), DqdlToken::Word(upper)] = rest
                else {
                    return None;
                };
                if !and.eq_ignore_ascii_case("and") {
                    return None;
                }
                Some(DqdlNumericCondition::Between(
                    lower.parse::<f64>().ok()?,
                    upper.parse::<f64>().ok()?,
                ))
            }
            "in" => {
                let listed = list_of(rest)?;
                let mut numbers = Vec::with_capacity(listed.len());
                for entry in listed {
                    numbers.push(entry.parse::<f64>().ok()?);
                }
                Some(DqdlNumericCondition::In(numbers))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A condition on a column's VALUES, which a rule can state over numbers or over text.
#[derive(Debug, Clone, PartialEq)]
enum DqdlValueCondition {
    /// A condition on the value read as a number: a cell that is not a number satisfies no
    /// such condition.
    Numeric(DqdlNumericCondition),
    /// `in [ … ]` over a set of values, or its negation.
    InSet { values: Vec<String>, negated: bool },
    /// A comparison against one text value, which only equality and inequality are.
    Matches { value: String, negated: bool },
}

impl DqdlValueCondition {
    /// Whether this condition admits one cell. A cell holding NO value satisfies no
    /// condition on the values: there is no value to satisfy it.
    fn admits(&self, cell: Option<&str>) -> bool {
        let Some(value) = cell else {
            return false;
        };
        match self {
            Self::Numeric(condition) => value
                .trim()
                .parse::<f64>()
                .is_ok_and(|number| condition.admits(number)),
            Self::InSet { values, negated } => {
                values.iter().any(|admitted| admitted == value) != *negated
            }
            Self::Matches { value: against, negated } => (against == value) != *negated,
        }
    }
}

/// The condition `tokens` state on a column's values, or `None` when they state none this
/// evaluation reads.
fn value_condition_of(tokens: &[DqdlToken]) -> Option<DqdlValueCondition> {
    match tokens.split_first()? {
        // `in [ … ]` and `not in [ … ]` over the values the list holds, which a rule writes
        // as quoted text or as bare numbers.
        (DqdlToken::Word(keyword), rest) if keyword.eq_ignore_ascii_case("in") => {
            Some(DqdlValueCondition::InSet {
                values: list_of(rest)?,
                negated: false,
            })
        }
        (DqdlToken::Word(keyword), rest) if keyword.eq_ignore_ascii_case("not") => {
            let (next, listed) = rest.split_first()?;
            match next {
                DqdlToken::Word(word) if word.eq_ignore_ascii_case("in") => {
                    Some(DqdlValueCondition::InSet {
                        values: list_of(listed)?,
                        negated: true,
                    })
                }
                _ => None,
            }
        }
        // A comparison against one value: text compares only for equality, while a number
        // takes the whole numeric condition.
        (DqdlToken::Operator(operator), [DqdlToken::Text(value)]) => {
            match DqdlComparison::of(operator)? {
                DqdlComparison::Equal => Some(DqdlValueCondition::Matches {
                    value: value.clone(),
                    negated: false,
                }),
                DqdlComparison::NotEqual => Some(DqdlValueCondition::Matches {
                    value: value.clone(),
                    negated: true,
                }),
                _ => None,
            }
        }
        _ => condition_of(tokens).map(DqdlValueCondition::Numeric),
    }
}

/// The values of a bracketed list, each as the text it was written as — a quoted literal's
/// content, or a bare word. `None` when `tokens` do not open and close one list.
fn list_of(tokens: &[DqdlToken]) -> Option<Vec<String>> {
    let (open, rest) = tokens.split_first()?;
    if *open != DqdlToken::OpenBracket {
        return None;
    }
    let mut values = Vec::new();
    let mut expect_value = true;
    for (position, token) in rest.iter().enumerate() {
        match token {
            DqdlToken::CloseBracket => {
                // The list's close must end the rule: a rule carries nothing after the
                // values its condition admits.
                return (position + 1 == rest.len()).then_some(values);
            }
            DqdlToken::Comma if !expect_value => expect_value = true,
            DqdlToken::Text(value) if expect_value => {
                values.push(value.clone());
                expect_value = false;
            }
            DqdlToken::Word(value) if expect_value => {
                values.push(value.clone());
                expect_value = false;
            }
            _ => return None,
        }
    }
    None
}

/// One token of a DQDL rule's text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DqdlToken {
    /// An unquoted word: a rule type, a keyword, or a value written without quotes.
    Word(String),
    /// A quoted string literal, as the content between its quotes.
    Text(String),
    /// A comparison operator.
    Operator(String),
    OpenBracket,
    CloseBracket,
    Comma,
}

/// How many tokens one rule may hold: a rule states a type, a column, a condition and the
/// values it admits, so a text carrying more than this is not one rule's.
///
/// CHOSEN, to bound what is read out of text a caller wrote: a rule past it is one this
/// evaluation does not read.
const MAX_DQDL_RULE_TOKENS: usize = 1_000;

/// Split one rule's text into its tokens, or `None` for text that is not tokens of a rule:
/// an unterminated string literal, or more tokens than one rule holds.
fn tokenize_dqdl_rule(rule: &str) -> Option<Vec<DqdlToken>> {
    let mut tokens: Vec<DqdlToken> = Vec::new();
    let mut characters = rule.chars().peekable();
    while let Some(character) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        let token = match character {
            '"' | '\'' => {
                let mut literal = String::new();
                let mut terminated = false;
                let mut escaped = false;
                for inside in characters.by_ref() {
                    if escaped {
                        literal.push(inside);
                        escaped = false;
                    } else if inside == '\\' {
                        escaped = true;
                    } else if inside == character {
                        terminated = true;
                        break;
                    } else {
                        literal.push(inside);
                    }
                }
                if !terminated {
                    return None;
                }
                DqdlToken::Text(literal)
            }
            '[' => DqdlToken::OpenBracket,
            ']' => DqdlToken::CloseBracket,
            ',' => DqdlToken::Comma,
            '=' | '!' | '<' | '>' => {
                let mut operator = String::from(character);
                if characters.peek().is_some_and(|next| *next == '=') {
                    operator.push('=');
                    characters.next();
                } else if character == '<' && characters.peek().is_some_and(|next| *next == '>') {
                    operator.push('>');
                    characters.next();
                }
                DqdlToken::Operator(operator)
            }
            _ => {
                let mut word = String::from(character);
                while let Some(next) = characters.peek() {
                    if next.is_whitespace() || matches!(next, '"' | '\'' | '[' | ']' | ',' | '=' | '!' | '<' | '>') {
                        break;
                    }
                    word.push(*next);
                    characters.next();
                }
                DqdlToken::Word(word)
            }
        };
        tokens.push(token);
        if tokens.len() > MAX_DQDL_RULE_TOKENS {
            return None;
        }
    }
    (!tokens.is_empty()).then_some(tokens)
}
