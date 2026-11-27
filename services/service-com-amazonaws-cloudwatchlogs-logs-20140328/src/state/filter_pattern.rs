// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs FILTER AND PATTERN syntax: what a `filterPattern` MEANS when it is
//! run over a log event message.
//!
//! `super::validate_filter_pattern` answers whether a pattern is well formed, which is all a
//! configuration write needs. This module goes one step further and RUNS a pattern over a
//! message, which is what `TestMetricFilter` ("Tests the filter pattern of a metric filter
//! against a sample of log event messages") reports, and what any operation that selects log
//! events by pattern needs.
//!
//! The syntax has three forms, told apart by the pattern's first character — the same
//! reading `super::FILTER_PATTERN_DELIMITERS` records:
//!
//! * `{ … }` — a JSON pattern: conditions over the fields of an event that IS a JSON
//!   document, each field selected with `$.path`. A path segment may index into an array
//!   member — `$.arr[0]` names one element and `$.arr[*]` names every element, so a
//!   condition on the latter holds when one element satisfies it. An event that is not JSON
//!   matches no JSON pattern.
//! * `[ … ]` — a space-delimited pattern: a comma-separated field list, optionally with
//!   conditions on the fields it names, matched against the event's fields. A field of the
//!   EVENT is a run of non-space characters, except that a `"…"` or `[…]` group is ONE field
//!   and the delimiters are not part of its value. A list of N fields matches an event of at
//!   least N fields, naming its leading fields in order; `...` stands for any number of
//!   fields, so a list using it names the fields before it from the event's start and the
//!   fields after it from its end.
//! * anything else — an unstructured pattern: the terms the message has to carry. A term is
//!   a bare word of the syntax's word characters, a `"…"` string (which may carry any
//!   character, including spaces) or a `%…%` regular expression; a term prefixed `-` must
//!   NOT appear, and a term prefixed `?` is one of the alternatives at least one of which
//!   must appear. A bare term carrying any other character is invalid rather than text —
//!   such a term is spelled as a string or an expression. The empty pattern matches every
//!   event.
//!
//! A matching event also yields the values the pattern EXTRACTED from it, keyed the way a
//! metric filter's dimension values select them (`super::is_log_event_field_reference`): a
//! space-delimited field under `$name`, or `$position` when the list left it unnamed, and
//! EVERY leaf of a JSON event under the `$.path` that selects it — the whole document's
//! leaves, not only the paths the condition happens to name. An unstructured pattern
//! extracts nothing — it names no field of the event.
//!
//! WHAT BOUNDS IT: no crates.io crate parses this syntax — it is CloudWatch Logs' own, not
//! a published data format — so the reading lives here, beside the structural check that
//! already reads it, and carries its own explicit bounds rather than relying on the caller:
//!
//! * the pattern is held to [`MAX_PATTERN_CHARS`], which is the modelled `FilterPattern`
//!   ceiling, so every scan here is linear in at most that many characters and the slot list
//!   and token list it builds are bounded by the same;
//! * the only caller-controlled NESTING is a condition's parenthesised groups, held to
//!   [`MAX_CONDITION_DEPTH`], so the recursive-descent parse cannot be driven deeper than
//!   that by a request;
//! * a `%…%` regular expression is compiled under [`MAX_COMPILED_REGEX_BYTES`] and run on
//!   the linear-time `regex` engine;//! * an event that is JSON is parsed by `serde_json` rather than by anything here, and the
//!   walk that extracts its leaves is bounded on every axis a request can grow: its depth
//!   by [`MAX_DOCUMENT_DEPTH`], the nodes it visits by [`MAX_DOCUMENT_NODES`], each key's
//!   length by [`MAX_SELECTOR_BYTES`], and the map it builds by [`MAX_EXTRACTED_VALUES`]
//!   entries and [`MAX_EXTRACTED_BYTES`] of key and value bytes — so the extraction, and
//!   the response it becomes, are capped rather than sized by the event;
//! * a sample event message itself is held, by the caller, to the largest log event
//!   CloudWatch Logs holds (`super::MAX_LOG_EVENT_BYTES`), so the length every scan here is
//!   linear in is capped server-side;
//! * the tokenizer, the field splitter and the event's field reader are iterative — nothing
//!   recurses on the length of caller input, and every path through the tokenizer consumes
//!   at least one character.

use std::collections::{BTreeMap, BTreeSet};

/// The longest pattern this module will read, which is the modelled `FilterPattern`
/// ceiling (`@length(0..1024)`). The request walk already refuses a longer one before a
/// handler runs; stating it here is what bounds every scan below even when a caller inside
/// the emulator reaches this module directly.
pub const MAX_PATTERN_CHARS: usize = 1024;

/// How deeply a condition may nest parenthesised groups. THE EMULATOR'S OWN BOUND: the
/// recursive-descent parse recurses once per group, so a request cannot drive it deeper
/// than this. A pattern nesting further is refused rather than parsed.
pub const MAX_CONDITION_DEPTH: usize = 32;

/// How deeply the extraction walks a JSON event. THE EMULATOR'S OWN BOUND: a leaf below it
/// is left out of the extracted values. Conditions are unaffected — a `$.path` selector is
/// resolved by iterating the path, never by recursing.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// How many nodes of a JSON event the extraction visits. THE EMULATOR'S OWN BOUND: an
/// interior node yields no value, so entry and byte caps alone would not bound a document
/// made of a million empty objects; this bounds the walk itself whatever shape the event
/// has.
pub const MAX_DOCUMENT_NODES: usize = 100_000;

/// How many values the extraction reports for one event. THE EMULATOR'S OWN BOUND: an
/// event's leaf count is the caller's to choose, and each leaf becomes an entry of the
/// response, so this is what keeps a small request from producing an arbitrarily larger
/// answer.
pub const MAX_EXTRACTED_VALUES: usize = 1_000;

/// How many bytes of keys and values together the extraction reports for one event. THE
/// EMULATOR'S OWN BOUND, beside [`MAX_EXTRACTED_VALUES`]: a few long values cost as much as
/// many short ones, so the count and the size are both capped.
pub const MAX_EXTRACTED_BYTES: usize = 256 * 1024;

/// How long one extracted key may be. THE EMULATOR'S OWN BOUND: a key carries the whole
/// path that selects its value, so without this a single long member name would be
/// re-allocated once per leaf below it.
pub const MAX_SELECTOR_BYTES: usize = 1_024;

/// How large a compiled expression this module will build from a `%…%` literal. THE
/// EMULATOR'S OWN BOUND: the pattern carrying it is already `@length(0..1024)`, and this caps
/// what compiling the expression inside it may allocate.
const MAX_COMPILED_REGEX_BYTES: usize = 1 << 20;

/// The wildcard a space-delimited field list uses to stand for any number of fields.
const ELLIPSIS: &str = "...";

/// The fixed text CloudWatch Logs refuses an unreadable `filterPattern` with: the whole
/// message for the class, with NO detail appended — a pattern whose structure or whose
/// conditions the service cannot read is refused with exactly this sentence, whatever about
/// it did not read, because that is the one text the service answers for the class. The two
/// defects that have a message of their OWN are the only exceptions: a bracketed form that
/// closes with the wrong delimiter (`super::validate_filter_pattern`) and a term carrying a
/// character the grammar admits nowhere ([`invalid_characters_in_term`]).
pub const INVALID_PATTERN: &str = "Invalid filter pattern";

/// [`INVALID_PATTERN`] as a refusal: the message every reader below hands back when it
/// cannot read the pattern it was given.
fn invalid_pattern() -> String {
    INVALID_PATTERN.to_string()
}

/// The fixed text the service refuses an unreadable pattern of a METRIC FILTER with: the
/// same message as [`INVALID_PATTERN`] for the same defect, naming the kind of pattern the
/// member carries. The members that take a metric filter's pattern are `PutMetricFilter`'s
/// and `TestMetricFilter`'s ("Tests the filter pattern of a metric filter against a sample
/// of log event messages"); every other `filterPattern` is a log-event filter pattern.
pub const INVALID_METRIC_PATTERN: &str = "Invalid metric filter pattern";

/// `refusal` as the refusal of a METRIC filter's pattern.
///
/// The one fixed text for a pattern the service cannot read is the metric one for a member
/// that carries a metric filter's pattern; every other refusal the reading answers — the
/// delimiter rule, a term carrying a character the grammar admits nowhere — is one sentence
/// for either member and is handed back as it is.
#[must_use]
pub fn as_metric_pattern_refusal(refusal: String) -> String {
    if refusal == INVALID_PATTERN {
        return INVALID_METRIC_PATTERN.to_string();
    }
    refusal
}

/// Hold a `filterPattern` a request CONFIGURES to what a configuration write reads of it:
/// the bracket, quote and expression structure of any form (`super::validate_filter_pattern`),
/// and, for one of the syntax's STRUCTURED forms (`{ … }` and `[ … ]`), the whole condition —
/// a stored pattern whose conditions the service cannot read could never select a log event,
/// so it is refused where it is written rather than at every later read.
///
/// An UNSTRUCTURED pattern is held to its structure only: its terms are read by whatever
/// RUNS it, which is the side that reports a term the running grammar does not admit.
pub fn validate_configured_pattern(pattern: &str) -> Result<(), String> {
    super::validate_filter_pattern(pattern)?;
    if !opens_a_structured_form(pattern) {
        return Ok(());
    }
    FilterPattern::parse(pattern).map(|_| ())
}

/// Whether `pattern` opens one of the syntax's structured forms, which is what its first
/// character says (`super::FILTER_PATTERN_DELIMITERS`).
fn opens_a_structured_form(pattern: &str) -> bool {
    pattern.trim().starts_with(['{', '['])
}

/// How the service refuses a TERM of an unstructured pattern that carries a character the
/// pattern grammar admits nowhere in it: the fixed sentence, naming the term the request
/// spells. The `"` of a quoted string delimits one — it is not a character a term may
/// carry — so a quote that opens no quoted string makes the term it sits in unreadable.
fn invalid_characters_in_term(term: &str) -> String {
    format!("Invalid character(s) in term '{term}'")
}

/// The whitespace-delimited run of `characters` starting at `at`: the TERM a `"…"` string
/// that does not close spells, which is what a refusal names for it — the request's own
/// text, rather than whatever the reader got through.
fn term_text(characters: &[char], at: usize) -> String {
    characters
        .iter()
        .skip(at)
        .take_while(|character| !character.is_whitespace())
        .collect()
}

/// Whether `character` may appear in a BARE term of an unstructured pattern: the syntax's
/// own word characters — the alphanumerics, `_` and the `-` an ordinary word carries inside
/// it. Everything else — a delimiter, a punctuation mark, a path separator — is spelled
/// inside a `"…"` string or a `%…%` regular expression, which is what the syntax gives a
/// term carrying one: the filter and pattern syntax the `filterPattern` members point at
/// states that a term including characters other than these is enclosed in double quotes,
/// and the service refuses the ones that are not (recorded for a quote, a colon and a slash
/// alike). A `-` is admitted because a term that carries one is accepted as the term it
/// spells (recorded); only a `-` where a term STARTS is the exclusion prefix, which the
/// pattern's own walk reads before a term is read. A letter outside ASCII is still a letter
/// and so is admitted: what the refusals name is the punctuation.
fn is_term_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-')
}

/// A `filterPattern` parsed into the form it is RUN in.
#[derive(Debug)]
pub enum FilterPattern {
    /// The empty pattern, which matches every log event and extracts nothing.
    Everything,
    /// An unstructured pattern: the terms the message has to carry.
    Terms(TermPattern),
    /// A space-delimited pattern (`[ … ]`).
    SpaceDelimited(SpaceDelimitedPattern),
    /// A JSON pattern (`{ … }`).
    Json(JsonPattern),
}

impl FilterPattern {
    /// Parse a `filterPattern`. The `Err` carries the message to refuse the request with, as
    /// the service states the rule.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        // The module's own input bound, so nothing below scans more than this.
        if pattern.chars().count() > MAX_PATTERN_CHARS {
            return Err(invalid_pattern());
        }
        // The bracket, quote and regular-expression structure is one rule for every member
        // that takes a filter pattern, so it is checked by the one checker a configuration
        // write uses rather than restated here.
        super::validate_filter_pattern(pattern)?;
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            return Ok(Self::Everything);
        }
        if let Some(body) = trimmed
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        {
            return Ok(Self::Json(JsonPattern::parse(body)?));
        }
        if let Some(body) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            return Ok(Self::SpaceDelimited(SpaceDelimitedPattern::parse(body)?));
        }
        Ok(Self::Terms(TermPattern::parse(trimmed)?))
    }

    /// The values this pattern extracts from `message`, or `None` when the pattern does not
    /// match it. A pattern that matches but selects no field of the event yields an EMPTY
    /// map, which is a match all the same.
    pub fn match_message(&self, message: &str) -> Option<BTreeMap<String, String>> {
        match self {
            Self::Everything => Some(BTreeMap::new()),
            Self::Terms(terms) => terms.match_message(message),
            Self::SpaceDelimited(fields) => fields.match_message(message),
            Self::Json(json) => json.match_message(message),
        }
    }

    /// Whether this pattern NAMES fields of the log event — whether, for some event, the
    /// values it extracts are keyed by a field this pattern selects rather than by nothing at
    /// all.
    ///
    /// One of the syntax's STRUCTURED forms does: a JSON pattern selects the fields of a JSON
    /// event by their `$.path`, and a space-delimited pattern selects the fields its list
    /// NAMES. An unstructured pattern names no field of the event — it only states terms the
    /// message carries — and neither does the empty pattern, which matches everything and
    /// extracts nothing ([`Self::match_message`]).
    #[must_use]
    pub fn names_event_fields(&self) -> bool {
        match self {
            Self::Everything | Self::Terms(_) => false,
            Self::SpaceDelimited(fields) => fields.names_a_field(),
            Self::Json(_) => true,
        }
    }
}

/// An unstructured pattern: terms that must appear, terms that must not, and the
/// alternatives at least one of which must.
#[derive(Debug, Default)]
pub struct TermPattern {
    /// Terms every matching message carries.
    required: Vec<Term>,
    /// `?`-prefixed alternatives, at least one of which a matching message carries. Empty
    /// when the pattern states none, in which case it demands nothing.
    optional: Vec<Term>,
    /// `-`-prefixed terms no matching message carries.
    excluded: Vec<Term>,
}

impl TermPattern {
    /// Parse the whitespace-separated terms of an unstructured pattern.
    fn parse(pattern: &str) -> Result<Self, String> {
        let characters: Vec<char> = pattern.chars().collect();
        let mut parsed = Self::default();
        let mut at = 0usize;
        // `?` and `-` each PREFIX a term, which is what they are where one starts; inside
        // a term they are characters the syntax does not admit, so a term that ends before
        // the whitespace-delimited run it sits in does not make the next character a prefix.
        let mut starts_a_term = true;
        while let Some(character) = characters.get(at).copied() {
            if character.is_whitespace() {
                at += 1;
                starts_a_term = true;
                continue;
            }
            let optional = starts_a_term && character == '?';
            let excluded = starts_a_term && character == '-';
            if optional || excluded {
                at += 1;
            }
            let term = read_term(&characters, &mut at)?;
            starts_a_term = false;
            if optional {
                parsed.optional.push(term);
            } else if excluded {
                parsed.excluded.push(term);
            } else {
                parsed.required.push(term);
            }
        }
        if parsed.required.is_empty() && parsed.optional.is_empty() && parsed.excluded.is_empty()
        {
            return Err(invalid_pattern());
        }
        Ok(parsed)
    }

    /// Whether `message` carries the terms this pattern demands. An unstructured pattern
    /// names no field of the event, so a match extracts nothing.
    fn match_message(&self, message: &str) -> Option<BTreeMap<String, String>> {
        let matched = self.required.iter().all(|term| term.matches(message))
            && !self.excluded.iter().any(|term| term.matches(message))
            && (self.optional.is_empty()
                || self.optional.iter().any(|term| term.matches(message)));
        matched.then(BTreeMap::new)
    }
}

/// One term of an unstructured pattern.
#[derive(Debug)]
enum Term {
    /// A literal the message has to carry — a bare word, or the inside of a `"…"` string.
    Text(String),
    /// A `%…%` regular expression the message has to match.
    Expression(::regex::Regex),
}

impl Term {
    /// Whether `message` carries this term.
    fn matches(&self, message: &str) -> bool {
        match self {
            Self::Text(text) => message.contains(text.as_str()),
            Self::Expression(expression) => expression.is_match(message),
        }
    }
}

/// Read one term's body, with `at` on its first character. A `?` or `-` whose body is
/// EMPTY — at the end of the pattern, or because whitespace follows it — prefixes no term,
/// and is refused rather than read as a term that every message carries.
///
/// A term is a BARE word of the syntax's word characters ([`is_term_character`]), a `"…"`
/// string or a `%…%` regular expression. A character the syntax admits in none of them — a
/// punctuation mark, or a quote that opens no string — makes a term of its own, which is
/// refused as [`invalid_characters_in_term`] naming the run of such characters, because the
/// syntax spells a term carrying one inside a string or an expression instead.
fn read_term(characters: &[char], at: &mut usize) -> Result<Term, String> {
    let dangling = invalid_pattern;
    match characters.get(*at).copied() {
        // Only reachable through a `?` or `-` that prefixes nothing: the pattern's own
        // walk skips whitespace before reading a term.
        None => Err(dangling()),
        Some(character) if character.is_whitespace() => Err(dangling()),
        Some('"') => {
            // A quoted string carries its content straight after the opening delimiter and
            // closes with another one, so a quote with whitespace or the end of the pattern
            // after it, and one that never closes, delimit no string: the term at fault is
            // then the run the pattern spells from that quote on.
            let opens = *at;
            if characters
                .get(*at + 1)
                .is_none_or(|next| next.is_whitespace())
            {
                return Err(invalid_characters_in_term(&term_text(characters, opens)));
            }
            read_delimited(characters, at, '"', Escapes::Unescape)
                .map(Term::Text)
                .map_err(|_| invalid_characters_in_term(&term_text(characters, opens)))
        }
        Some('%') => {
            let source = read_delimited(characters, at, '%', Escapes::Keep)?;
            Ok(Term::Expression(compile(&source)?))
        }
        Some(character) if !is_term_character(character) => {
            let mut offending = String::new();
            while let Some(found) = characters.get(*at).copied() {
                if found.is_whitespace()
                    || is_term_character(found)
                    || matches!(found, '"' | '%')
                {
                    break;
                }
                offending.push(found);
                *at += 1;
            }
            Err(invalid_characters_in_term(&offending))
        }
        Some(_) => {
            let mut text = String::new();
            while let Some(found) = characters.get(*at).copied() {
                if !is_term_character(found) {
                    break;
                }
                text.push(found);
                *at += 1;
            }
            Ok(Term::Text(text))
        }
    }
}

/// How a delimited literal reads a backslash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escapes {
    /// A quoted string: `\x` IS the character `x`, so `\"` carries a quote into the literal
    /// and the backslash itself is not part of the text.
    Unescape,
    /// A regular-expression literal: the expression's own escapes (`\d`, `\.`, `\s`) are
    /// part of the expression, so only `\%` is consumed as "a literal per-cent" and every
    /// other backslash is kept for the expression engine to read.
    Keep,
}

/// Read a `delimiter`-delimited literal, with `at` on its OPENING delimiter, leaving `at`
/// one past its closing one. `escapes` says what a backslash means inside it.
fn read_delimited(
    characters: &[char],
    at: &mut usize,
    delimiter: char,
    escapes: Escapes,
) -> Result<String, String> {
    *at += 1;
    let mut text = String::new();
    while let Some(character) = characters.get(*at) {
        *at += 1;
        match *character {
            '\\' => match (escapes, characters.get(*at)) {
                (Escapes::Unescape, Some(escaped)) => {
                    text.push(*escaped);
                    *at += 1;
                }
                (Escapes::Keep, Some(escaped)) => {
                    if *escaped != delimiter {
                        text.push('\\');
                    }
                    text.push(*escaped);
                    *at += 1;
                }
                (_, None) => text.push('\\'),
            },
            found if found == delimiter => return Ok(text),
            found => text.push(found),
        }
    }
    Err(invalid_pattern())
}

/// Compile a `%…%` literal under this module's bound, refusing a literal that is no regular
/// expression at all.
fn compile(source: &str) -> Result<::regex::Regex, String> {
    ::regex::RegexBuilder::new(source)
        .size_limit(MAX_COMPILED_REGEX_BYTES)
        .build()
        .map_err(|_| invalid_pattern())
}

/// A space-delimited pattern: the field list, and the conditions its fields carry.
#[derive(Debug)]
pub struct SpaceDelimitedPattern {
    /// The declared slots in order; `None` is the `...` wildcard.
    slots: Vec<Option<Slot>>,
    /// What every matching event's fields have to satisfy, when the list states conditions.
    condition: Option<Expression>,
}

/// One named position of a space-delimited field list.
#[derive(Debug)]
struct Slot {
    /// The key this slot's value is extracted under: `$name`, or `$position` when the list
    /// left the slot unnamed.
    key: String,
    /// The name a condition refers to this slot by, when the list named it.
    name: Option<String>,
}

impl SpaceDelimitedPattern {
    /// Parse the inside of a `[ … ]` pattern.
    fn parse(body: &str) -> Result<Self, String> {
        if body.trim().is_empty() {
            return Err(invalid_pattern());
        }
        // Two passes: the field NAMES are what tells a condition's field reference from a
        // literal, and a condition may refer to a field the list names after it.
        let mut slots: Vec<Option<Slot>> = Vec::new();
        let mut conditions: Vec<Vec<Token>> = Vec::new();
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut position = 0usize;
        for part in split_fields(body) {
            let trimmed = part.trim();
            if trimmed == ELLIPSIS {
                slots.push(None);
                continue;
            }
            position += 1;
            let tokens = lex(trimmed)?;
            let name = match tokens.first() {
                None => None,
                Some(Token::Word(word)) => Some(word.clone()),
                Some(Token::Selector(selector)) => Some(selector.clone()),
                Some(_) => return Err(invalid_pattern()),
            };
            if let Some(name) = &name {
                names.insert(name.clone());
            }
            slots.push(Some(Slot {
                key: name
                    .as_ref()
                    .map_or_else(|| format!("${position}"), |name| format!("${name}")),
                name,
            }));
            if tokens.len() > 1 {
                conditions.push(tokens);
            }
        }
        if slots.iter().filter(|slot| slot.is_none()).count() > 1 {
            return Err(invalid_pattern());
        }
        let declared = Declared {
            names,
            fields: position,
        };
        let context = Context::Fields(&declared);
        let mut parsed: Vec<Expression> = Vec::with_capacity(conditions.len());
        for tokens in &conditions {
            parsed.push(parse_expression(tokens, &context)?);
        }
        let condition = match parsed.len() {
            0 => None,
            1 => parsed.pop(),
            _ => Some(Expression::All(parsed)),
        };
        Ok(Self { slots, condition })
    }

    /// Whether this list NAMES any of the fields it declares: a slot the list named is
    /// selected by that name (`$name`), which is what a dimension value selects a
    /// space-delimited field by. A list of nothing but the `...` wildcard names none.
    fn names_a_field(&self) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|slot| slot.name.is_some())
    }

    /// Align the event's fields to this list and run its conditions over them.
    fn match_message(&self, message: &str) -> Option<BTreeMap<String, String>> {
        let fields = space_delimited_fields(message);
        let (extracted, values) = self.align(&fields)?;
        if let Some(condition) = &self.condition {
            if !condition.satisfied(&values) {
                return None;
            }
        }
        Some(extracted)
    }

    /// Map each declared slot onto one of the event's fields, or `None` when the event has a
    /// field count this list does not describe.
    fn align(&self, fields: &[String]) -> Option<(BTreeMap<String, String>, FieldValues)> {
        let declared = self.slots.len();
        let wildcard = self.slots.iter().position(Option::is_none);
        let named = declared - usize::from(wildcard.is_some());
        let mut mapping: Vec<(usize, usize)> = Vec::with_capacity(named);
        match wildcard {
            // A list of N fields describes an event of AT LEAST N fields: the slots name the
            // event's leading fields, in order, and a field past the last slot is one the list
            // says nothing about rather than one that makes the event a different shape
            // (recorded: a list naming the first three fields of a space-delimited event
            // matches an event carrying a fourth). An event with FEWER fields than the list
            // names leaves a named slot with no field to align to, so the list does not
            // describe it.
            None => {
                if fields.len() < declared {
                    return None;
                }
                for index in 0..declared {
                    mapping.push((index, index));
                }
            }
            // `...` stands for any number of fields, so the slots before it count from the
            // start of the event and the slots after it from its end.
            Some(at) => {
                if fields.len() < named {
                    return None;
                }
                for index in 0..at {
                    mapping.push((index, index));
                }
                for index in (at + 1)..declared {
                    mapping.push((index, fields.len() - (declared - index)));
                }
            }
        }

        let mut extracted = BTreeMap::new();
        let mut by_name = BTreeMap::new();
        let mut by_position = Vec::with_capacity(named);
        for (slot_index, field_index) in mapping {
            let Some(slot) = self.slots.get(slot_index).and_then(Option::as_ref) else {
                continue;
            };
            let value = fields.get(field_index).cloned().unwrap_or_default();
            extracted.insert(slot.key.clone(), value.clone());
            if let Some(name) = &slot.name {
                by_name.insert(name.clone(), value.clone());
            }
            by_position.push(value);
        }
        Some((
            extracted,
            FieldValues {
                by_name,
                by_position,
            },
        ))
    }
}

/// The fields of ONE space-delimited event, reachable the way a condition names them: by the
/// name the list gave the slot, or by the slot's 1-based position in the list.
#[derive(Debug)]
struct FieldValues {
    /// Field name to the value the event carries there.
    by_name: BTreeMap<String, String>,
    /// The values in slot order, so `$1` is the first.
    by_position: Vec<String>,
}

impl Lookup for FieldValues {
    fn value(&self, key: &str) -> FieldValue {
        if let Some(found) = self.by_name.get(key) {
            return FieldValue::Text(found.clone());
        }
        if let Ok(position) = key.parse::<usize>() {
            if let Some(found) = position.checked_sub(1).and_then(|at| self.by_position.get(at)) {
                return FieldValue::Text(found.clone());
            }
        }
        FieldValue::Absent
    }
}

/// The fields of a space-delimited EVENT: a run of non-space characters, except that a
/// `"…"` or `[…]` group is ONE field whose value excludes the delimiters.
pub fn space_delimited_fields(message: &str) -> Vec<String> {
    let characters: Vec<char> = message.chars().collect();
    let mut fields = Vec::new();
    let mut at = 0usize;
    while let Some(character) = characters.get(at).copied() {
        if character.is_whitespace() {
            at += 1;
            continue;
        }
        let field = match character {
            '"' => read_group(&characters, &mut at, '"', '"'),
            '[' => read_group(&characters, &mut at, '[', ']'),
            _ => {
                let mut text = String::new();
                while let Some(character) = characters.get(at) {
                    if character.is_whitespace() {
                        break;
                    }
                    text.push(*character);
                    at += 1;
                }
                text
            }
        };
        fields.push(field);
    }
    fields
}

/// Read one grouped field of an event, with `at` on its opening delimiter. Nesting of the
/// SAME pair is tracked, so a bracketed group carrying brackets stays one field; a group the
/// message never closes runs to the end of the message.
fn read_group(characters: &[char], at: &mut usize, open: char, close: char) -> String {
    *at += 1;
    let mut text = String::new();
    let mut depth = 1usize;
    while let Some(character) = characters.get(*at) {
        if *character == close {
            depth -= 1;
            if depth == 0 {
                *at += 1;
                return text;
            }
        } else if *character == open && open != close {
            depth += 1;
        }
        text.push(*character);
        *at += 1;
    }
    text
}

/// A JSON pattern: the condition the event document has to satisfy.
#[derive(Debug)]
pub struct JsonPattern {
    /// What a matching document satisfies.
    condition: Expression,
}

impl JsonPattern {
    /// Parse the inside of a `{ … }` pattern.
    fn parse(body: &str) -> Result<Self, String> {
        let tokens = lex(body)?;
        if tokens.is_empty() {
            return Err(invalid_pattern());
        }
        let condition = parse_expression(&tokens, &Context::Json)?;
        // A JSON pattern is a condition over the event DOCUMENT's fields, so one that
        // selects no field describes no event: `{ foo = 1 }` relates two literals and is
        // not a pattern at all.
        if !condition.references_a_field() {
            return Err(invalid_pattern());
        }
        Ok(Self { condition })
    }

    /// Run this pattern over `message`. A message that is not a JSON document has no fields
    /// to select, so it matches no JSON pattern.
    fn match_message(&self, message: &str) -> Option<BTreeMap<String, String>> {
        let document: ::serde_json::Value = ::serde_json::from_str(message).ok()?;
        let lookup = JsonDocument {
            document: &document,
        };
        if !self.condition.satisfied(&lookup) {
            return None;
        }
        let mut extracted = BTreeMap::new();
        flatten("$", &document, 0, &mut Budget::new(), &mut extracted);
        Some(extracted)
    }
}

/// One JSON event document, reachable by the `$.path` selectors a condition names.
#[derive(Debug)]
struct JsonDocument<'a> {
    /// The parsed event.
    document: &'a ::serde_json::Value,
}

impl Lookup for JsonDocument<'_> {
    fn value(&self, key: &str) -> FieldValue {
        resolve(self.document, key).map_or(FieldValue::Absent, field_value)
    }

    fn values(&self, key: &str) -> Vec<FieldValue> {
        let selected = resolve_all(self.document, key);
        if selected.is_empty() {
            // A selector the document satisfies nowhere reads as the ABSENT field it is,
            // exactly as one naming a member the document does not carry does.
            return vec![FieldValue::Absent];
        }
        selected.into_iter().map(field_value).collect()
    }
}

/// The node a `$.path` selector names, or `None` when the document carries no such field.
/// A selector naming every element of an array (`[*]`) resolves to the FIRST element it
/// selects, which is what a single-valued reader of a selector can report;
/// [`resolve_all`] is what a condition is run over.
fn resolve<'a>(document: &'a ::serde_json::Value, selector: &str) -> Option<&'a ::serde_json::Value> {
    resolve_all(document, selector).into_iter().next()
}

/// Every node a `$.path` selector names in `document`.
///
/// A path of member names and decimal indices names at most ONE node, so the answer is
/// empty or a single node. The `[*]` element selector names every element of the array it
/// is applied to, so a path carrying one names as many nodes as those arrays hold between
/// them — which is what makes `{$.arr[*] = "red"}` a condition over the array's elements.
/// The walk is held to [`MAX_SELECTED_NODES`], so a document cannot make one selector
/// resolve to more nodes than that.
fn resolve_all<'a>(
    document: &'a ::serde_json::Value,
    selector: &str,
) -> Vec<&'a ::serde_json::Value> {
    let Some(path) = selector.strip_prefix("$.") else {
        return Vec::new();
    };
    let mut current: Vec<&::serde_json::Value> = vec![document];
    for segment in path.split('.') {
        let (name, indices) = split_indices(segment);
        if !name.is_empty() {
            current = current.iter().filter_map(|node| node.get(name)).collect();
        }
        for index in indices {
            current = match index {
                Index::At(at) => current.iter().filter_map(|node| node.get(at)).collect(),
                Index::Every => current
                    .iter()
                    .filter_map(|node| node.as_array())
                    .flatten()
                    .take(MAX_SELECTED_NODES)
                    .collect(),
            };
            current.truncate(MAX_SELECTED_NODES);
        }
        if current.is_empty() {
            return Vec::new();
        }
    }
    current
}

/// The text a `$.path` selector names in one JSON log event, or `None` when the event
/// carries no such field.
///
/// A selector may only name a VALUE of the event: a container node is a subtree rather than
/// a value, so a selector that lands on one names nothing. The rendering is the one a
/// condition reads a field as, so a number reads the way the event wrote it.
///
/// This is how a dimension value or a metric value resolves a selector against an event
/// whose leaves a pattern did not extract — a pattern that selects no field of the event at
/// all, or one whose extraction stopped at this module's bounds.
pub fn json_selector_text(document: &::serde_json::Value, selector: &str) -> Option<String> {
    match resolve(document, selector)? {
        ::serde_json::Value::Object(_) | ::serde_json::Value::Array(_) => None,
        leaf => field_value(leaf).as_text(),
    }
}

/// Whether one segment of a `$.path` selects anything: a member name, followed by the array
/// element selectors applied to it. An element selector is a decimal index (`[0]`) or the
/// `[*]` that names every element ([`Index`]); a bracket group that is neither selects
/// nothing, so a selector carrying one is refused rather than read as if the brackets were
/// not there.
fn is_selector_segment(segment: &str) -> bool {
    let (name, mut remaining) = match segment.find('[') {
        Some(at) => segment.split_at(at),
        None => (segment, ""),
    };
    if name.is_empty() {
        return false;
    }
    while !remaining.is_empty() {
        let Some(opened) = remaining.strip_prefix('[') else {
            return false;
        };
        let Some(closed) = opened.find(']') else {
            return false;
        };
        if opened.get(..closed).and_then(Index::read).is_none() {
            return false;
        }
        remaining = opened.get(closed + 1..).unwrap_or_default();
    }
    true
}

/// One array element selector of a `$.path` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Index {
    /// `[N]`: the element at this position.
    At(usize),
    /// `[*]`: every element of the array.
    Every,
}

impl Index {
    /// The element selector `text` spells, between the brackets it was read from, or `None`
    /// when it spells neither form.
    fn read(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == EVERY_ELEMENT {
            return Some(Self::Every);
        }
        text.parse::<usize>().ok().map(Self::At)
    }
}

/// The element selector that names EVERY element of an array, as the filter-pattern syntax
/// spells it: `$.arr[*]`.
const EVERY_ELEMENT: &str = "*";

/// How many nodes ONE selector may resolve to. THE EMULATOR'S OWN BOUND: `[*]` selects as
/// many nodes as the event's array holds, and an event is caller-supplied, so the frontier
/// the walk carries is capped here rather than left to grow with the event.
const MAX_SELECTED_NODES: usize = 1_024;

/// One path segment split into its member name and the array element selectors applied to
/// it, so `items[2]` reads the third element of `items` and `items[*]` reads every element.
/// The segment has already been held to [`is_selector_segment`] when the pattern was
/// parsed, so every bracket group here is an element selector.
fn split_indices(segment: &str) -> (&str, Vec<Index>) {
    let Some(at) = segment.find('[') else {
        return (segment, Vec::new());
    };
    let indices = segment[at..]
        .split('[')
        .filter_map(|part| part.strip_suffix(']'))
        .filter_map(Index::read)
        .collect();
    (&segment[..at], indices)
}

/// One JSON node as a condition compares it. A container node is no value: a selector that
/// lands on one names a subtree ([`FieldValue::Container`]).
fn field_value(value: &::serde_json::Value) -> FieldValue {
    match value {
        ::serde_json::Value::Null => FieldValue::Null,
        ::serde_json::Value::Bool(flag) => FieldValue::Boolean(*flag),
        ::serde_json::Value::Number(number) => number
            .as_f64()
            .map_or_else(|| FieldValue::Text(number.to_string()), FieldValue::Number),
        ::serde_json::Value::String(text) => FieldValue::Text(text.clone()),
        ::serde_json::Value::Object(_) | ::serde_json::Value::Array(_) => FieldValue::Container,
    }
}

/// What the extraction of ONE event still has to spend. Every bound below is this module's
/// own: the event is caller-supplied, so the walk over it and the map it builds are capped
/// here rather than left to grow with the request.
#[derive(Debug)]
struct Budget {
    /// Nodes of the document still to be visited, so the walk is bounded whatever shape
    /// the document has — an interior node costs a visit even when it yields no value.
    nodes: usize,
    /// Entries the extracted map may still take.
    entries: usize,
    /// Key and value bytes the extracted map may still hold between them.
    bytes: usize,
}

impl Budget {
    /// A full budget, for one event.
    fn new() -> Self {
        Self {
            nodes: MAX_DOCUMENT_NODES,
            entries: MAX_EXTRACTED_VALUES,
            bytes: MAX_EXTRACTED_BYTES,
        }
    }
}

/// Every LEAF of a JSON event, under the `$.path` that selects it — the values a JSON
/// pattern extracts from a matching event. An object contributes its members and an array
/// its elements, so only scalars land in the map, and ALL of them do: the extraction is
/// about the event, not about which paths the condition happened to name.
///
/// The walk is bounded on every axis a request could grow: `depth` against
/// [`MAX_DOCUMENT_DEPTH`], the nodes visited and the map's entries and bytes against
/// `budget`, and each key's length against [`MAX_SELECTOR_BYTES`]. Past any of them the
/// walk STOPS — a value below the bound is simply not extracted, which changes nothing
/// about whether the event matched.
fn flatten(
    prefix: &str,
    value: &::serde_json::Value,
    depth: usize,
    budget: &mut Budget,
    extracted: &mut BTreeMap<String, String>,
) {
    if depth > MAX_DOCUMENT_DEPTH || prefix.len() > MAX_SELECTOR_BYTES {
        return;
    }
    let Some(nodes) = budget.nodes.checked_sub(1) else {
        return;
    };
    budget.nodes = nodes;
    if budget.entries == 0 || budget.bytes == 0 {
        return;
    }
    match value {
        ::serde_json::Value::Object(members) => {
            for (name, member) in members {
                flatten(
                    &format!("{prefix}.{name}"),
                    member,
                    depth + 1,
                    budget,
                    extracted,
                );
            }
        }
        ::serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(
                    &format!("{prefix}[{index}]"),
                    item,
                    depth + 1,
                    budget,
                    extracted,
                );
            }
        }
        leaf => {
            if let Some(text) = field_value(leaf).as_text() {
                let cost = prefix.len().saturating_add(text.len());
                let Some(bytes) = budget.bytes.checked_sub(cost) else {
                    budget.bytes = 0;
                    return;
                };
                budget.bytes = bytes;
                budget.entries = budget.entries.saturating_sub(1);
                extracted.insert(prefix.to_string(), text);
            }
        }
    }
}

/// What a condition reads a field as.
#[derive(Debug, Clone, PartialEq)]
enum FieldValue {
    /// The event carries no such field.
    Absent,
    /// The field is there and is JSON `null`.
    Null,
    /// A JSON boolean.
    Boolean(bool),
    /// A JSON number.
    Number(f64),
    /// A string, which is also how a space-delimited field reads before any comparison
    /// decides to read it as a number.
    Text(String),
    /// The field is there and is a CONTAINER — a JSON object or array. A selector that lands
    /// on one names a subtree rather than a value, so the field EXISTS and yet stands in no
    /// comparison: an element of it is selected with a `[…]` element selector or a further
    /// member name.
    Container,
}

impl FieldValue {
    /// This value as a number, when it reads as one: a space-delimited field arrives as
    /// text, so `200` compares as a number against a numeric literal.
    fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            Self::Text(text) => text.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    /// This value as text, or `None` for a value that has no text form: a field the event
    /// does not carry, and a container node, which is a subtree rather than a value.
    fn as_text(&self) -> Option<String> {
        match self {
            Self::Absent | Self::Container => None,
            Self::Null => Some("null".to_string()),
            Self::Boolean(flag) => Some(flag.to_string()),
            Self::Number(number) => Some(render_number(*number)),
            Self::Text(text) => Some(text.clone()),
        }
    }

    /// Whether this value can stand in a comparison at all. A field the event does not carry
    /// and one that lands on a container node have no value to compare, so a comparison
    /// naming one is unsatisfied whichever relation it states — `!=` included, because only
    /// `NOT EXISTS` speaks about a field's PRESENCE.
    fn comparable(&self) -> bool {
        !matches!(self, Self::Absent | Self::Container)
    }
}

/// A JSON number as text, with a whole number rendered without a fractional part so a
/// count or a status code reads the way the event wrote it.
fn render_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{:.0}", number)
    } else {
        format!("{number}")
    }
}

/// Where a condition reads its fields from.
trait Lookup {
    /// The value under `key` — a field name or 1-based position for a space-delimited
    /// event, a `$.path` selector for a JSON one.
    fn value(&self, key: &str) -> FieldValue;

    /// EVERY value under `key`. A key names one value unless it selects every element of an
    /// array (`$.arr[*]`), which is why a condition is run over this rather than over
    /// [`Self::value`]: a key that selects several values satisfies a predicate when ONE of
    /// them does. Never empty — a key the event satisfies nowhere reads as one absent value,
    /// so a selector that selects nothing behaves exactly like one naming a missing member.
    fn values(&self, key: &str) -> Vec<FieldValue> {
        vec![self.value(key)]
    }
}

/// One condition of a pattern.
#[derive(Debug)]
enum Expression {
    /// At least one branch holds (`||`).
    Any(Vec<Expression>),
    /// Every branch holds (`&&`).
    All(Vec<Expression>),
    /// Two operands in the stated relation.
    Compare {
        /// The left-hand operand.
        left: Operand,
        /// The relation the two have to be in.
        comparison: Comparison,
        /// The right-hand operand.
        right: Operand,
    },
    /// The event carries this field.
    Present(String),
    /// The event carries no such field (`NOT EXISTS`).
    Missing(String),
    /// The field is there and is `null` (`IS NULL`).
    IsNull(String),
}

impl Expression {
    /// Whether the event these fields come from satisfies this condition.
    fn satisfied<L: Lookup>(&self, lookup: &L) -> bool {
        match self {
            Self::Any(branches) => branches.iter().any(|branch| branch.satisfied(lookup)),
            Self::All(branches) => branches.iter().all(|branch| branch.satisfied(lookup)),
            Self::Compare {
                left,
                comparison,
                right,
            } => compare(left, *comparison, right, lookup),
            Self::Present(field) => lookup
                .values(field)
                .iter()
                .any(|value| *value != FieldValue::Absent),
            Self::Missing(field) => lookup
                .values(field)
                .iter()
                .all(|value| *value == FieldValue::Absent),
            Self::IsNull(field) => lookup.values(field).contains(&FieldValue::Null),
        }
    }

    /// Whether this condition selects any field of the event at all, which is what makes it
    /// a condition ABOUT an event rather than a relation between two literals.
    fn references_a_field(&self) -> bool {
        match self {
            Self::Any(branches) | Self::All(branches) => {
                branches.iter().any(Self::references_a_field)
            }
            Self::Compare { left, right, .. } => {
                matches!(left, Operand::Field(_)) || matches!(right, Operand::Field(_))
            }
            Self::Present(_) | Self::Missing(_) | Self::IsNull(_) => true,
        }
    }
}

/// The relation a comparison states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    /// `=` / `==`.
    Equal,
    /// `!=`.
    NotEqual,
    /// `<`.
    Less,
    /// `<=`.
    LessOrEqual,
    /// `>`.
    Greater,
    /// `>=`.
    GreaterOrEqual,
}

/// One side of a comparison.
#[derive(Debug)]
enum Operand {
    /// A field of the event, by the key its lookup names it under.
    Field(String),
    /// A literal the pattern wrote.
    Literal(FieldValue),
    /// A literal carrying `*`, which stands for any run of characters.
    Glob(String),
    /// A `%…%` regular expression.
    Expression(::regex::Regex),
}

impl Operand {
    /// This operand against one event: every value it resolves to, which is one value unless
    /// it selects every element of an array ([`Lookup::values`]).
    fn resolve<L: Lookup>(&self, lookup: &L) -> Vec<Resolved<'_>> {
        match self {
            Self::Field(key) => lookup
                .values(key)
                .into_iter()
                .map(Resolved::Value)
                .collect(),
            Self::Literal(value) => vec![Resolved::Value(value.clone())],
            Self::Glob(glob) => vec![Resolved::Glob(glob)],
            Self::Expression(expression) => vec![Resolved::Expression(expression)],
        }
    }
}

/// An operand resolved against one event: a value, or the pattern a value is held to.
#[derive(Debug)]
enum Resolved<'a> {
    /// A concrete value.
    Value(FieldValue),
    /// A `*` wildcard literal.
    Glob(&'a str),
    /// A regular expression.
    Expression(&'a ::regex::Regex),
}

/// Whether the two operands stand in `comparison`. Ordering is NUMERIC: `<`, `<=`, `>` and
/// `>=` relate numbers, so a side that reads as no number stands in no ordering at all.
///
/// A side that selects SEVERAL values (`$.arr[*]`) stands in the relation when one of the
/// values it selects does, which is what makes such a selector a condition over the array's
/// elements.
fn compare<L: Lookup>(
    left: &Operand,
    comparison: Comparison,
    right: &Operand,
    lookup: &L,
) -> bool {
    let left = left.resolve(lookup);
    let right = right.resolve(lookup);
    left.iter()
        .flat_map(|left| right.iter().map(move |right| (left, right)))
        // THE EMULATOR'S OWN BOUND on one comparison: each side is already held to
        // [`MAX_SELECTED_NODES`], and this caps their PRODUCT, so a condition relating two
        // element-wildcard selectors over a caller-supplied event cannot multiply out.
        .take(MAX_COMPARED_PAIRS)
        .any(|(left, right)| related(left, comparison, right))
}

/// How many (left, right) value pairs ONE comparison considers. THE EMULATOR'S OWN BOUND:
/// see [`compare`].
const MAX_COMPARED_PAIRS: usize = MAX_SELECTED_NODES;

/// Whether ONE value on each side stands in `comparison`.
///
/// A side with no value to compare ([`FieldValue::comparable`]) stands in NO relation, so
/// every comparison on it — `!=` as much as `=` — is unsatisfied: a condition on a field the
/// event does not carry, or on one that lands on a container node, says nothing about the
/// event, and the syntax's own way of asking about PRESENCE is `EXISTS` / `NOT EXISTS`.
fn related(left: &Resolved<'_>, comparison: Comparison, right: &Resolved<'_>) -> bool {
    if !comparable(left) || !comparable(right) {
        return false;
    }
    match comparison {
        Comparison::Equal => equal(left, right),
        Comparison::NotEqual => !equal(left, right),
        Comparison::Less | Comparison::LessOrEqual | Comparison::Greater
        | Comparison::GreaterOrEqual => {
            let (Resolved::Value(left), Resolved::Value(right)) = (left, right) else {
                return false;
            };
            let (Some(left), Some(right)) = (left.as_number(), right.as_number()) else {
                return false;
            };
            match comparison {
                Comparison::Less => left < right,
                Comparison::LessOrEqual => left <= right,
                Comparison::Greater => left > right,
                _ => left >= right,
            }
        }
    }
}

/// Whether one side of a comparison carries something to compare. A pattern's own literal
/// always does; a field of the event may not ([`FieldValue::comparable`]).
fn comparable(side: &Resolved<'_>) -> bool {
    match side {
        Resolved::Value(value) => value.comparable(),
        Resolved::Glob(_) | Resolved::Expression(_) => true,
    }
}

/// Whether the two operands are equal: a `*` literal or a regular expression is matched
/// against the other side's text, two sides that both read as numbers are compared as
/// numbers, and anything else is compared as text.
///
/// A BOOLEAN and a NULL are states rather than text: the syntax's literals are numbers and
/// strings, and what asks about either state is `IS TRUE` / `IS FALSE` / `IS NULL`, so a
/// boolean or null field equals only the same state and never the text that renders it. That
/// is what tells `{$.flag IS TRUE}` (the boolean) from `{$.flag = true}` (the string).
fn equal(left: &Resolved<'_>, right: &Resolved<'_>) -> bool {
    match (left, right) {
        (Resolved::Value(value), Resolved::Glob(glob))
        | (Resolved::Glob(glob), Resolved::Value(value)) => value
            .as_text()
            // The `*` wildcard is read the one way this service reads it, wherever a member
            // carries one; the filter-pattern syntax is case sensitive, so neither side is
            // folded here.
            .is_some_and(|text| super::wildcard_matches(&text, glob)),
        (Resolved::Value(value), Resolved::Expression(expression))
        | (Resolved::Expression(expression), Resolved::Value(value)) => {
            value.as_text().is_some_and(|text| expression.is_match(&text))
        }
        (Resolved::Value(left), Resolved::Value(right)) => match (left, right) {
            (FieldValue::Boolean(left), FieldValue::Boolean(right)) => left == right,
            (FieldValue::Null, FieldValue::Null) => true,
            (FieldValue::Boolean(_) | FieldValue::Null, _)
            | (_, FieldValue::Boolean(_) | FieldValue::Null) => false,
            (left, right) => match (left.as_number(), right.as_number()) {
                (Some(left), Some(right)) => left == right,
                _ => left.as_text() == right.as_text(),
            },
        },
        _ => false,
    }
}

/// The names and the field count a space-delimited list declared, which is what tells a
/// condition's field reference from a literal.
#[derive(Debug)]
struct Declared {
    /// Every name the list gave a slot.
    names: BTreeSet<String>,
    /// How many slots it declared, so `$N` names one of them.
    fields: usize,
}

/// Which pattern form a condition belongs to, which decides what a selector may be.
#[derive(Debug)]
enum Context<'a> {
    /// A `{ … }` pattern: a selector is a `$.path` into the event document.
    Json,
    /// A `[ … ]` pattern: a selector names one of the declared slots.
    Fields(&'a Declared),
}

/// Parse one whole condition, which has to consume every token it was handed.
fn parse_expression(tokens: &[Token], context: &Context<'_>) -> Result<Expression, String> {
    let mut parser = Parser {
        tokens,
        at: 0,
        depth: 0,
        context,
    };
    let expression = parser.any()?;
    if parser.at < tokens.len() {
        return Err(invalid_pattern());
    }
    Ok(expression)
}

/// A recursive-descent parse over one condition's tokens.
struct Parser<'a> {
    /// The tokens of the condition.
    tokens: &'a [Token],
    /// How far the parse has got.
    at: usize,
    /// How many parenthesised groups the parse is inside, held to
    /// [`MAX_CONDITION_DEPTH`] so a request cannot drive the recursion deeper.
    depth: usize,
    /// Which pattern form this condition belongs to.
    context: &'a Context<'a>,
}

impl Parser<'_> {
    /// `||`-separated alternatives.
    fn any(&mut self) -> Result<Expression, String> {
        let mut branches = vec![self.all()?];
        while matches!(self.tokens.get(self.at), Some(Token::Or)) {
            self.at += 1;
            branches.push(self.all()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Expression::Any(branches)
        })
    }

    /// `&&`-separated conjuncts.
    fn all(&mut self) -> Result<Expression, String> {
        let mut branches = vec![self.predicate()?];
        while matches!(self.tokens.get(self.at), Some(Token::And)) {
            self.at += 1;
            branches.push(self.predicate()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Expression::All(branches)
        })
    }

    /// One parenthesised group, or one comparison.
    fn predicate(&mut self) -> Result<Expression, String> {
        if matches!(self.tokens.get(self.at), Some(Token::Open)) {
            self.at += 1;
            self.depth += 1;
            if self.depth > MAX_CONDITION_DEPTH {
                return Err(invalid_pattern());
            }
            let inner = self.any()?;
            self.depth -= 1;
            if !matches!(self.tokens.get(self.at), Some(Token::Close)) {
                return Err(invalid_pattern());
            }
            self.at += 1;
            return Ok(inner);
        }
        let left = self.operand()?;
        match self.tokens.get(self.at) {
            Some(Token::Compare(comparison)) => {
                let comparison = *comparison;
                self.at += 1;
                let right = self.operand()?;
                Ok(Expression::Compare {
                    left,
                    comparison,
                    right,
                })
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("is") => {
                self.at += 1;
                let field = Self::field(&left)?;
                let Some(Token::Word(state)) = self.tokens.get(self.at) else {
                    return Err(invalid_pattern());
                };
                self.at += 1;
                if state.eq_ignore_ascii_case("null") {
                    return Ok(Expression::IsNull(field));
                }
                let expected = if state.eq_ignore_ascii_case("true") {
                    true
                } else if state.eq_ignore_ascii_case("false") {
                    false
                } else {
                    return Err(invalid_pattern());
                };
                Ok(Expression::Compare {
                    left,
                    comparison: Comparison::Equal,
                    right: Operand::Literal(FieldValue::Boolean(expected)),
                })
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("not") => {
                self.at += 1;
                let field = Self::field(&left)?;
                let Some(Token::Word(exists)) = self.tokens.get(self.at) else {
                    return Err(invalid_pattern());
                };
                if !exists.eq_ignore_ascii_case("exists") {
                    return Err(invalid_pattern());
                }
                self.at += 1;
                Ok(Expression::Missing(field))
            }
            _ => Ok(Expression::Present(Self::field(&left)?)),
        }
    }

    /// One side of a comparison.
    fn operand(&mut self) -> Result<Operand, String> {
        let Some(token) = self.tokens.get(self.at).cloned() else {
            return Err(invalid_pattern());
        };
        self.at += 1;
        match token {
            Token::Selector(selector) => Ok(Operand::Field(self.selector(&selector)?)),
            Token::Quoted(text) => Ok(if text.contains('*') {
                Operand::Glob(text)
            } else {
                Operand::Literal(FieldValue::Text(text))
            }),
            Token::Expression(source) => Ok(Operand::Expression(compile(&source)?)),
            Token::Word(word) => Ok(self.word(&word)),
            Token::Compare(_) | Token::And | Token::Or | Token::Open | Token::Close => {
                Err(invalid_pattern())
            }
        }
    }

    /// The lookup key a `$…` selector names, refusing a selector that names no field.
    fn selector(&self, selector: &str) -> Result<String, String> {
        match self.context {
            Context::Json => {
                let Some(path) = selector.strip_prefix('.') else {
                    return Err(invalid_pattern());
                };
                if path.is_empty() || !path.split('.').all(is_selector_segment) {
                    return Err(invalid_pattern());
                }
                Ok(format!("${selector}"))
            }
            Context::Fields(declared) => {
                if declared.names.contains(selector) {
                    return Ok(selector.to_string());
                }
                if selector
                    .parse::<usize>()
                    .is_ok_and(|position| (1..=declared.fields).contains(&position))
                {
                    return Ok(selector.to_string());
                }
                Err(invalid_pattern())
            }
        }
    }

    /// A bare word: a field of a space-delimited list when the list declares that name,
    /// otherwise the literal it spells.
    ///
    /// A literal of a condition is a NUMBER or a STRING — the two kinds of value the syntax
    /// compares with `=`, `!=` and the orderings — so `true`, `false` and `null` spell the
    /// text they are written as rather than a boolean or null state. The state of a boolean
    /// or null field is asked about with `IS TRUE`, `IS FALSE` and `IS NULL`, which build
    /// those values themselves.
    fn word(&self, word: &str) -> Operand {
        if let Context::Fields(declared) = self.context {
            if declared.names.contains(word) {
                return Operand::Field(word.to_string());
            }
        }
        if let Ok(number) = word.parse::<f64>() {
            return Operand::Literal(FieldValue::Number(number));
        }
        if word.contains('*') {
            return Operand::Glob(word.to_string());
        }
        Operand::Literal(FieldValue::Text(word.to_string()))
    }

    /// The field an operand selects, for a predicate that is about the field's PRESENCE
    /// rather than about its value.
    fn field(operand: &Operand) -> Result<String, String> {
        match operand {
            Operand::Field(key) => Ok(key.clone()),
            Operand::Literal(_) | Operand::Glob(_) | Operand::Expression(_) => {
                Err(invalid_pattern())
            }
        }
    }
}

/// One lexical item of a condition.
#[derive(Debug, Clone)]
enum Token {
    /// A `$…` selector, without its `$`.
    Selector(String),
    /// A `"…"` literal, without its quotes.
    Quoted(String),
    /// A `%…%` regular expression, without its delimiters.
    Expression(String),
    /// A bare word: a field name, a keyword or an unquoted literal.
    Word(String),
    /// A comparison operator.
    Compare(Comparison),
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `(`.
    Open,
    /// `)`.
    Close,
}

/// The characters that end a bare word, each of which opens a token of its own.
const WORD_BREAKS: [char; 12] = [
    '(', ')', '&', '|', '=', '!', '<', '>', '"', '%', ',', '$',
];

/// Whether `character` may appear in a bare WORD of a condition: the alphanumerics and `_`
/// a name or a keyword is spelled with, the `-`, `+` and `.` a number carries, and the `*`
/// of a wildcard literal — which is also what spells the `...` a space-delimited field list
/// stands any number of fields for with. A value carrying any other character is spelled as
/// a `"…"` string or a `%…%` regular expression.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '+' | '.' | '*')
}

/// The characters a `$…` selector may carry after its `$`: a path of member names, the
/// array element selectors applied to them, and the name of a space-delimited slot. The `*`
/// of the `[*]` element selector is admitted only INSIDE a bracket group
/// ([`selector_characters`]), so it keeps its wildcard meaning everywhere else a condition
/// spells a value.
fn is_selector_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '.' | '[' | ']' | '@' | '-')
}

/// The run of selector characters at `at`, with `at` left one past it: the body of a `$…`
/// token. A `*` is read only between `[` and `]`, where it is the element selector that
/// names every element of an array rather than a wildcard literal.
fn selector_characters(characters: &[char], at: &mut usize) -> String {
    let mut selector = String::new();
    let mut in_brackets = false;
    while let Some(found) = characters.get(*at).copied() {
        if found == EVERY_ELEMENT_CHARACTER && in_brackets {
            selector.push(found);
            *at += 1;
            continue;
        }
        if !is_selector_character(found) {
            break;
        }
        if found == '[' {
            in_brackets = true;
        } else if found == ']' {
            in_brackets = false;
        }
        selector.push(found);
        *at += 1;
    }
    selector
}

/// [`EVERY_ELEMENT`] as the one character it is.
const EVERY_ELEMENT_CHARACTER: char = '*';

/// Split a condition into its tokens.
fn lex(source: &str) -> Result<Vec<Token>, String> {
    let characters: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0usize;
    while let Some(character) = characters.get(at).copied() {
        if character.is_whitespace() {
            at += 1;
            continue;
        }
        match character {
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            // The syntax states its boolean operators as `&&` and `||`, so a lone `&` or
            // `|` is no operator — and giving it an arm of its own is also what keeps every
            // path through this loop consuming at least one character.
            '&' | '|' => {
                if characters.get(at + 1) != Some(&character) {
                    return Err(invalid_pattern());
                }
                tokens.push(if character == '&' { Token::And } else { Token::Or });
                at += 2;
            }
            '=' => {
                tokens.push(Token::Compare(Comparison::Equal));
                at += if characters.get(at + 1) == Some(&'=') { 2 } else { 1 };
            }
            '!' => {
                if characters.get(at + 1) != Some(&'=') {
                    return Err(invalid_pattern());
                }
                tokens.push(Token::Compare(Comparison::NotEqual));
                at += 2;
            }
            '<' | '>' => {
                let orientation = character == '<';
                let inclusive = characters.get(at + 1) == Some(&'=');
                tokens.push(Token::Compare(match (orientation, inclusive) {
                    (true, false) => Comparison::Less,
                    (true, true) => Comparison::LessOrEqual,
                    (false, false) => Comparison::Greater,
                    (false, true) => Comparison::GreaterOrEqual,
                }));
                at += if inclusive { 2 } else { 1 };
            }
            '"' => tokens.push(Token::Quoted(read_delimited(
                &characters,
                &mut at,
                '"',
                Escapes::Unescape,
            )?)),
            '%' => tokens.push(Token::Expression(read_delimited(
                &characters,
                &mut at,
                '%',
                Escapes::Keep,
            )?)),
            // A condition states its own structure with the operators above; a comma is no
            // part of it. The field list of a space-delimited pattern is split on its
            // top-level commas BEFORE a slot's condition reaches here
            // ([`split_fields`]), so a comma that arrives here is in no condition the
            // syntax describes.
            ',' => return Err(invalid_pattern()),
            '$' => {
                at += 1;
                tokens.push(Token::Selector(selector_characters(&characters, &mut at)));
            }
            _ => {
                // A bare word of a condition is a run of the characters a word carries
                // ([`is_word_character`]): a name, a keyword, a number or a `*` wildcard.
                // A character the syntax admits nowhere makes a term of its own, which is
                // refused rather than read into a word — the value carrying it is spelled
                // as a `"…"` string or a `%…%` expression instead.
                if !is_word_character(character) {
                    let mut offending = String::new();
                    while let Some(found) = characters.get(at).copied() {
                        if found.is_whitespace()
                            || WORD_BREAKS.contains(&found)
                            || is_word_character(found)
                        {
                            break;
                        }
                        offending.push(found);
                        at += 1;
                    }
                    return Err(invalid_characters_in_term(&offending));
                }
                let mut word = String::new();
                while let Some(found) = characters.get(at).copied() {
                    if !is_word_character(found) {
                        break;
                    }
                    word.push(found);
                    at += 1;
                }
                // The arm above consumed every character a word does not admit, so this one
                // reads at least one. Refusing an empty word is what keeps that true — and
                // keeps this loop making progress — if that ever stops holding.
                if word.is_empty() {
                    return Err(invalid_pattern());
                }
                tokens.push(Token::Word(word));
            }
        }
    }
    Ok(tokens)
}

/// Split a space-delimited field list on its TOP-LEVEL commas: a comma inside a quoted
/// string, a regular expression or a nested bracket belongs to the slot it is in.
fn split_fields(body: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut expression = false;
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                current.push(character);
                if let Some(escaped) = characters.next() {
                    current.push(escaped);
                }
            }
            '"' if !expression => {
                quoted = !quoted;
                current.push(character);
            }
            '%' if !quoted => {
                expression = !expression;
                current.push(character);
            }
            _ if quoted || expression => current.push(character),
            '(' | '[' | '{' => {
                depth += 1;
                current.push(character);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    parts.push(current);
    parts
}
