// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of WHERE a JSON document a caller submitted stopped parsing.
//!
//! Several AWS services take a JSON document as an opaque string member — an IAM policy
//! document, a message filter policy — and refuse one they cannot parse with their JSON
//! parser's own diagnostic rather than with a restatement of the member's type. Those
//! diagnostics all say the same things: what the parse stopped ON, what a value could have
//! been there instead, the line and column it stopped at, the container whose entries a
//! document that stopped on an entry separator ran out between, and — for a document that
//! ran out of input inside a collection — where that collection opened. The SENTENCE each service
//! frames them in is its own (the services differ in what they prefix and in which fault
//! classes they report), but the readings are the same question about the same document, so
//! they are stated once here: two services handed the same document cannot disagree about
//! where it went wrong.
//!
//! Everything here derives from [`serde_json::Error`], which is the parse that actually ran:
//! `line()` and `column()` are where it stopped, and its rendered cause is why. The one
//! reading the parse does not carry — the mark of the container it was still inside — is
//! read off the document itself ([`unclosed_container_start`]), for the fault class the
//! parse has already named; nothing re-scans a document looking for a likely fault.
//!
//! The reported position is ONE-based, and a column counts BYTES from the start of its line,
//! which is the convention the parser reports in.

/// The token classes a JSON value may begin with, as a parser's diagnostic lists them.
///
/// A JSON value is a string, a number, an array, an object or one of the three literal
/// tokens, and a diagnostic about a value position lists exactly those. The list is stated
/// ONCE here because the services that quote it frame it in sentences of their own — one
/// says `was expecting a valid value (<classes>)` where the parse stopped on a character and
/// another `Unrecognized token '<word>': was expecting (<classes>)` where it stopped on a
/// word — so the sentence is the caller's and the classes are not.
pub const VALUE_CLASSES: &str = "JSON String, Number, Array, Object or token 'null', 'true' or 'false'";

/// The clause a diagnostic carries where the parse stopped at a VALUE position: the classes
/// [`VALUE_CLASSES`] lists, in the sentence this module frames them in.
///
/// Spelled out because the lookup below answers with one `&'static str`; the test at the foot
/// of this module checks the clause carries [`VALUE_CLASSES`], so the two cannot drift apart.
const VALUE_POSITION: &str =
    "was expecting a valid value (JSON String, Number, Array, Object or token 'null', 'true' \
     or 'false')";

/// The cause of `error` without the position it renders after it.
///
/// `serde_json` renders a failure as `<cause> at line <l> column <c>`; the position is
/// reported on its own, so only the cause is wanted from the text.
#[must_use]
pub fn cause(error: &serde_json::Error) -> String {
    let rendered = error.to_string();
    rendered
        .split(" at line ")
        .next()
        .unwrap_or(&rendered)
        .trim()
        .to_string()
}

/// The byte offset in `document` of the position `line`:`column`, or `None` when the
/// document holds no such position — which includes the position one past its end, where a
/// document that stopped mid-value fails.
#[must_use]
pub fn byte_offset(document: &str, line: usize, column: usize) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut start = 0;
    for _ in 1..line {
        start += document[start..].find('\n')? + 1;
    }
    let offset = start + column - 1;
    (offset < document.len()).then_some(offset)
}

/// The character the parse stopped on: the `column`th byte of the `line`th line of
/// `document`, both counted from one, and `None` when the position is past its end.
///
/// A column counts bytes, so a position inside a multi-byte character names that character.
#[must_use]
pub fn character_at(document: &str, line: usize, column: usize) -> Option<char> {
    let line_start = byte_offset(document, line, 1)?;
    let mut offset = byte_offset(document, line, column)?;
    while offset > line_start && !document.is_char_boundary(offset) {
        offset -= 1;
    }
    document[offset..].chars().next()
}

/// The unquoted word the parse stopped on where a VALUE belongs, with the line and column
/// that word begins at — or `None` when the parse stopped on something else.
///
/// A JSON value is a string, a number, an array, an object or one of the three literals, so
/// a bare word where a value belongs is a word the parser does not recognise, and a parser
/// that names it names the WHOLE word and the position it STARTS at. The word is read at the
/// position the parse stopped: a parse that stopped while matching a literal is already a
/// character or more into the word, so the word is extended backwards to where it starts as
/// well as forwards to where it ends.
///
/// `None` covers every other fault — a document that ends early, a stray punctuation mark, a
/// bare word where a member NAME belongs (which is a fault about the missing quote, not about
/// the word) — each of which a caller reports in its own words.
#[must_use]
pub fn unrecognized_token<'a>(
    document: &'a str,
    error: &serde_json::Error,
) -> Option<(&'a str, usize, usize)> {
    if !expected_a_value(&cause(error)) {
        return None;
    }
    let stopped_at = byte_offset(document, error.line(), error.column())?;
    if !document.is_char_boundary(stopped_at) {
        return None;
    }
    let start = document[..stopped_at]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word_character(*character))
        .map(|(offset, _)| offset)
        .last()
        .unwrap_or(stopped_at);
    let word = &document[start..];
    if !word.chars().next().is_some_and(is_word_start) {
        return None;
    }
    let end = word
        .find(|character: char| !is_word_character(character))
        .unwrap_or(word.len());
    let (line, column) = position_of(document, start);
    Some((&word[..end], line, column))
}

/// Whether `cause` is a parse failure where a VALUE was expected: nothing that can start one
/// was there, or what was there began a literal and then did not spell it.
fn expected_a_value(cause: &str) -> bool {
    cause == "expected value" || cause == "expected ident"
}

/// The words a JSON parser reads as a number only where it is configured to admit the
/// non-numeric values IEEE-754 has and JSON does not.
///
/// JSON's grammar has no spelling for an infinity or for a not-a-number, so a parser that
/// admits them reads these words — and one that does not names the word it found rather than
/// reporting arbitrary unquoted text, because a document carrying one was written by a
/// serializer that had the extension enabled.
const NON_NUMERIC_NUMBER_WORDS: [&str; 3] = ["NaN", "Infinity", "INF"];

/// The sign a non-numeric numeric word may be written with.
const NEGATIVE: char = '-';

/// The non-numeric numeric token the parse stopped on, with the line and the column that
/// token begins at — or `None` when the parse stopped on something else.
///
/// A parser that refuses these words refuses them as a CLASS of their own rather than as
/// unrecognised text ([`unrecognized_token`]): the word is one a JSON serializer writes for
/// a value JSON cannot spell, so the token names the sign as well when one was written and a
/// caller can say which value the document meant.
///
/// The word is read at the position the parse stopped, which is the sign, the word's first
/// character, or — for a parse already matching the word — a character inside it; `None`
/// covers every other fault, including an unquoted word that is not one of these.
#[must_use]
pub fn non_numeric_number_token<'a>(
    document: &'a str,
    error: &serde_json::Error,
) -> Option<(&'a str, usize, usize)> {
    let cause = cause(error);
    // The fault a bare word gets where a value belongs, and the one a word gets where a
    // NUMBER had already begun, which is what a sign in front of it makes it.
    if !expected_a_value(&cause) && cause != "invalid number" {
        return None;
    }
    let stopped_at = byte_offset(document, error.line(), error.column())?;
    if !document.is_char_boundary(stopped_at) {
        return None;
    }
    // A parse that stopped on the sign has not reached the word yet.
    let reached = if document[stopped_at..].starts_with(NEGATIVE) {
        stopped_at + NEGATIVE.len_utf8()
    } else {
        stopped_at
    };
    let word_start = document[..reached]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word_character(*character))
        .map(|(offset, _)| offset)
        .last()
        .unwrap_or(reached);
    let rest = document.get(word_start..)?;
    let end = rest
        .find(|character: char| !is_word_character(character))
        .unwrap_or(rest.len());
    if !NON_NUMERIC_NUMBER_WORDS.contains(&&rest[..end]) {
        return None;
    }
    // The sign belongs to the token: it is what says which infinity was meant.
    let start = if document[..word_start].ends_with(NEGATIVE) {
        word_start - NEGATIVE.len_utf8()
    } else {
        word_start
    };
    let (line, column) = position_of(document, start);
    Some((&document[start..word_start + end], line, column))
}

/// Whether `character` may begin an unquoted word: a letter, or one of the two marks an
/// identifier starts with. A digit begins a NUMBER, whose own fault is not a word.
fn is_word_start(character: char) -> bool {
    character.is_alphabetic() || character == '_' || character == '$'
}

/// Whether `character` may be part of an unquoted word, once one has begun.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// The one-based line, and the one-based column in bytes, of byte `offset` of `document`.
fn position_of(document: &str, offset: usize) -> (usize, usize) {
    let before = &document[..offset];
    let line = 1 + before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |at| at + '\n'.len_utf8());
    (line, 1 + offset - line_start)
}

/// What the parse stopped on, as a diagnostic names it: the offending character and its
/// character code, or the end of the input, and what was expected in its place.
///
/// The caller frames it in its own sentence and appends [`source_location`].
#[must_use]
pub fn unexpected_input(document: &str, error: &serde_json::Error) -> String {
    let cause = cause(error);
    // A document that ran out BETWEEN a container's entries is read by the state the parse
    // was in rather than by what could have stood there: nothing could, the entry the
    // separator promised was simply never written.
    if let Some(container) = exhausted_between_entries(document, error) {
        return format!("Unexpected end-of-input within/between {container} entries");
    }
    // A document that ends mid-value has no offending character to name: the position is
    // the end of what was submitted.
    let offender = if cause.starts_with("EOF while parsing") {
        None
    } else {
        character_at(document, error.line(), error.column())
    };
    let expected = expected_where_parsing_stopped(&cause, offender);
    match offender {
        Some(offender) => format!(
            "Unexpected character ('{}' (code {})): {expected}",
            render_character(offender),
            u32::from(offender)
        ),
        None => format!("Unexpected end-of-input: {expected}"),
    }
}

/// What the parser expected where it stopped, from its own diagnosis of the failure.
///
/// A cause with no phrasing of its own is reported as the parser states it, so a failure this
/// list does not name still describes itself.
#[must_use]
pub fn expected_where_parsing_stopped(cause: &str, offender: Option<char>) -> &str {
    // A comma is trailing because what follows it closes the collection, so what was
    // expected depends on which collection that is: an object wants its next field name, an
    // array its next value.
    let cause = match (cause, offender) {
        ("trailing comma", Some(']')) => "expected value",
        ("trailing comma", _) => "key must be a string",
        (cause, _) => cause,
    };
    match cause {
        "key must be a string" => "was expecting double-quote to start field name",
        "expected value" => VALUE_POSITION,
        "expected ident" => "was expecting a valid token ('null', 'true' or 'false')",
        "expected `:`" => "was expecting a colon to separate field name and value",
        "expected `,` or `}`" => "was expecting comma to separate Object entries",
        "expected `,` or `]`" => "was expecting comma to separate Array entries",
        "trailing characters" => "was expecting end of the document",
        "EOF while parsing an object" => "expected close marker for Object",
        "EOF while parsing a list" => "expected close marker for Array",
        "EOF while parsing a string" => "expected close marker for String",
        "EOF while parsing a value" => "expected a value",
        other => other,
    }
}

/// A character as a diagnostic prints it: itself when it is printable, an escape when it is a
/// control character that would otherwise be invisible in the message.
#[must_use]
pub fn render_character(character: char) -> String {
    if character.is_control() {
        character.escape_default().to_string()
    } else {
        character.to_string()
    }
}

/// The SOURCE a location clause names where the document is not echoed into it.
///
/// A submitted document can be sensitive, so a service that keeps it out of its refusals
/// reports the source as redacted and names the feature that would have put it there —
/// which is what tells a caller the position is real and the document was withheld on
/// purpose.
pub const REDACTED_SOURCE: &str = "REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` \
                                   disabled)";

/// One location clause: the `source` the parse was reading, and the position in it.
///
/// Stated once because a diagnostic carries the SAME clause in more than one place — where
/// the parse stopped, and where the container it was inside opened — and because the source
/// is the caller's choice: [`REDACTED_SOURCE`] where the document is withheld, and the
/// document itself where a service is recorded echoing it.
#[must_use]
pub fn location_clause(source: &str, line: usize, column: usize) -> String {
    format!("[Source: {source}; line: {line}, column: {column}]")
}

/// Where the parse stopped, as a diagnostic's location clause reports it.
///
/// The clause names the line and the column but NOT the document: a service does not echo a
/// submitted document back in its refusal, so the source is reported as redacted.
#[must_use]
pub fn source_location(line: usize, column: usize) -> String {
    format!("\n at {}", location_clause(REDACTED_SOURCE, line, column))
}

/// The position the parse stopped at, as a location clause reports it.
///
/// A parse that ran out of input stopped one past the document's last character — there is
/// no character there, so the position named is where the next one would have gone, which
/// is what the parser's own clause reports and what a caller appending to its payload needs.
/// Every other fault stopped ON a character, which is the position the parse itself reports.
#[must_use]
pub fn stopped_at(document: &str, error: &serde_json::Error) -> (usize, usize) {
    if ran_out_of_input(&cause(error)) {
        return position_of(document, document.len());
    }
    (error.line(), error.column())
}

/// Whether `cause` is a parse that ended because the document did.
fn ran_out_of_input(cause: &str) -> bool {
    cause.starts_with("EOF while parsing")
}

/// The mark that separates one entry of a container from the next.
const ENTRY_SEPARATOR: u8 = b',';

/// The two containers a diagnostic names, as it names them.
const OBJECT: &str = "Object";
const ARRAY: &str = "Array";

/// The container the parse stood BETWEEN the entries of when `document` ran out of input —
/// `"Object"` or `"Array"`, as a diagnostic names it — and `None` for every other fault.
///
/// A document whose last mark outside a string is an entry separator has promised its
/// container a further entry and then stopped, so the parse did not run out where something could have
/// stood and was named: it ran out while reading ON between the container's entries, which is
/// the state a diagnostic reports instead. Every other way of running out — mid-value,
/// inside a string, or where the container could have closed — is still read by what the
/// parse was waiting for, so this answers `None` for all of them.
///
/// Read by [`unexpected_input`], which is how every service states this state and so
/// never reports it differently; the classification itself is not exported, so a service
/// cannot word this state in one of its own.
fn exhausted_between_entries(
    document: &str,
    error: &serde_json::Error,
) -> Option<&'static str> {
    if !ran_out_of_input(&cause(error)) {
        return None;
    }
    let tail = structural_tail(document)?;
    // A mark inside a string is text: a document ending inside one separates nothing.
    if tail.inside_string || tail.last_non_space != Some(ENTRY_SEPARATOR) {
        return None;
    }
    match tail.open.last()?.0 {
        b'{' => Some(OBJECT),
        b'[' => Some(ARRAY),
        _ => None,
    }
}

/// The most still-open containers the reading below tracks.
///
/// A parse nesting deeper than the parser's own recursion limit (128 for `serde_json`) fails
/// for THAT and never for the end of its input, so a document with more open marks than this
/// carries a different fault than the one this reading answers.
const MAX_UNCLOSED_CONTAINERS: usize = 128;

/// Where the container the parse was STILL INSIDE when the document ended was OPENED — the
/// line and column of its `{` or `[` — and `None` for every other fault.
///
/// A parse that runs out of input inside a collection reports where that collection opened as
/// well as where the input ended, because the opening mark is what the caller has left
/// unmatched: the two readings together say WHICH object or array was not closed, which a
/// document built by templating or concatenation cannot be read off the end position alone.
///
/// The mark is found by walking the document once and keeping the marks that are still open —
/// a string carries no structure, so what it contains, escapes included, is skipped — and
/// taking the innermost one left at the end, which is the context the parse was in.
///
/// `None` covers every fault that is not a container left open, and a document carrying more
/// open marks than [`MAX_UNCLOSED_CONTAINERS`].
#[must_use]
pub fn unclosed_container_start(
    document: &str,
    error: &serde_json::Error,
) -> Option<(usize, usize)> {
    let opening = match cause(error).as_str() {
        "EOF while parsing an object" => b'{',
        "EOF while parsing a list" => b'[',
        _ => return None,
    };
    let (mark, line, column) = *structural_tail(document)?.open.last()?;
    (mark == opening).then_some((line, column))
}

/// What a walk of a document's structure holds where the document ends.
struct StructuralTail {
    /// Every container mark still open there, outermost first, each with the one-based line
    /// and the one-based column in BYTES it sits at.
    open: Vec<(u8, usize, usize)>,
    /// The last byte read outside a string that is not whitespace — what the document ends
    /// ON, the entry separator this reading consults among them — and `None` for a document
    /// that carries nothing but whitespace.
    last_non_space: Option<u8>,
    /// Whether the document ends INSIDE a string, whose contents carry no structure.
    inside_string: bool,
}

/// The structure of `document` where it ends, or `None` when it leaves more containers open
/// than [`MAX_UNCLOSED_CONTAINERS`].
///
/// The document is walked once: a string carries no structure, so what it contains, escapes
/// included, is skipped, and every container mark that is closed again is matched off — so
/// what is left is the context the parse was in where the document stopped.
fn structural_tail(document: &str) -> Option<StructuralTail> {
    let mut open: Vec<(u8, usize, usize)> = Vec::new();
    let mut last_non_space = None;
    let mut line = 1;
    let mut column = 1;
    let mut inside_string = false;
    let mut escaped = false;
    for &byte in document.as_bytes() {
        if inside_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                inside_string = false;
                last_non_space = Some(byte);
            }
        } else {
            match byte {
                b'"' => inside_string = true,
                b'{' | b'[' => {
                    if open.len() == MAX_UNCLOSED_CONTAINERS {
                        return None;
                    }
                    open.push((byte, line, column));
                }
                b'}' | b']' => {
                    open.pop();
                }
                _ => {}
            }
            if !byte.is_ascii_whitespace() {
                last_non_space = Some(byte);
            }
        }
        if byte == b'\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Some(StructuralTail {
        open,
        last_non_space,
        inside_string,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parse failure of `document`, or a panic when it parses.
    fn failure(document: &str) -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>(document)
            .expect_err("the document must not parse")
    }

    /// One row: what a failing assertion calls it, the document, and the word the parse
    /// stopped on with the position it starts at (`None` when the fault names no word).
    type WordRow = (
        &'static str,
        &'static str,
        Option<(&'static str, usize, usize)>,
    );

    /// The rows differ along what the reading generalises over: whether the fault is a word
    /// at all, how far into the word the parse got, where the word sits (the first value, a
    /// member's value, a later line), and which other fault class the document carries.
    #[test]
    fn the_word_a_parse_stopped_on_is_read_at_the_position_the_parser_reports() {
        let rows: [WordRow; 11] = [
            // A word that begins a literal and then does not spell it: the parse is already
            // inside the word when it stops.
            ("a bare word of its own", "this-is-not-json", Some(("this", 1, 1))),
            ("a bare word spelling no literal", "nope", Some(("nope", 1, 1))),
            (
                "a bare word where a member's value belongs",
                r#"{"event": created}"#,
                Some(("created", 1, 11)),
            ),
            (
                "a bare word on the second line",
                "{\n  \"event\": oops\n}",
                Some(("oops", 2, 12)),
            ),
            (
                "a word a template left unsubstituted",
                r#"{"event": $event}"#,
                Some(("$event", 1, 11)),
            ),
            (
                "a literal spelled short",
                r#"{"event": tru}"#,
                Some(("tru", 1, 11)),
            ),
            // Faults that are not a word where a value belongs.
            ("a stray comma", r#"{"a": [1,, 2], "b": 3}"#, None),
            ("a missing comma between members", r#"{"a": 1 "b": 2}"#, None),
            ("an unquoted member name", r#"{event: 1}"#, None),
            ("a document that ends early", "{", None),
            ("an empty document", "", None),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = unrecognized_token(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} (the parser stopped at \
                 line {}, column {}: {})",
                error.line(),
                error.column(),
                cause(&error)
            );
        }
    }

    /// One row: what a failing assertion calls it, the document, and the non-numeric numeric
    /// token the parse stopped on with the position it starts at (`None` when the fault names
    /// no such token).
    type NonNumericRow = (
        &'static str,
        &'static str,
        Option<(&'static str, usize, usize)>,
    );

    /// The words JSON cannot spell are read as the class of their own they are, sign
    /// included, and every other fault — an unquoted word among them — names none.
    ///
    /// The rows differ along which word was written, whether it carries a sign, where it
    /// sits (the first value, a member's value, a later line), and which other fault class
    /// the document carries.
    #[test]
    fn a_non_numeric_number_word_is_read_with_its_sign_at_the_position_it_starts() {
        let rows: [NonNumericRow; 10] = [
            ("an infinity", r#"{"n": Infinity}"#, Some(("Infinity", 1, 7))),
            (
                "a negative infinity",
                r#"{"n": -Infinity}"#,
                Some(("-Infinity", 1, 7)),
            ),
            ("a not-a-number", r#"{"n": NaN}"#, Some(("NaN", 1, 7))),
            ("the short infinity", "[INF]", Some(("INF", 1, 2))),
            ("the short negative infinity", "[-INF]", Some(("-INF", 1, 2))),
            (
                "an infinity on the second line",
                "{\n  \"n\": Infinity\n}",
                Some(("Infinity", 2, 8)),
            ),
            // Faults that name no such token.
            ("an unquoted word that is not one", r#"{"n": nul}"#, None),
            ("a word that merely begins with one", r#"{"n": NaNish}"#, None),
            ("a missing comma between members", r#"{"a": 1 "b": 2}"#, None),
            ("a document that ends early", "{", None),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = non_numeric_number_token(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} (the parser stopped at \
                 line {}, column {}: {})",
                error.line(),
                error.column(),
                cause(&error)
            );
        }
    }

    /// A fault that names no word still describes itself: the character it stopped on, its
    /// code, and what was expected there — or, for a document that ran out of input on an
    /// entry separator, the container whose entries it ran out between.
    ///
    /// The rows differ along the fault class, which container the parse was inside, how
    /// deeply it nests, whether whitespace follows the separator, and whether a separator
    /// that could be mistaken for one sits inside a string.
    #[test]
    fn a_fault_that_names_no_word_names_the_character_it_stopped_on() {
        // (case, the document, what the diagnostic must say)
        let rows: [(&str, &str, &str); 10] = [
            (
                "a missing comma between members",
                r#"{"a": 1 "b": 2}"#,
                "Unexpected character ('\"' (code 34)): was expecting comma to separate Object \
                 entries",
            ),
            (
                "an unquoted member name",
                r#"{event: 1}"#,
                "Unexpected character ('e' (code 101)): was expecting double-quote to start \
                 field name",
            ),
            (
                "a document that ends early",
                r#"{"a": 1"#,
                "Unexpected end-of-input: expected close marker for Object",
            ),
            // A document that stops on an entry separator stopped BETWEEN its container's
            // entries, whichever container that is and however deep it sits.
            (
                "an object that ends on its member separator",
                r#"{"a": 1,"#,
                "Unexpected end-of-input within/between Object entries",
            ),
            (
                "an array that ends on its element separator",
                "[1, 2,",
                "Unexpected end-of-input within/between Array entries",
            ),
            (
                "an inner array that ends on its separator",
                r#"{"a": [1,"#,
                "Unexpected end-of-input within/between Array entries",
            ),
            (
                "an outer object that ends on its separator past a closed inner one",
                r#"{"a": {"b": 1},"#,
                "Unexpected end-of-input within/between Object entries",
            ),
            (
                "a separator followed by whitespace and a newline",
                "{\n  \"a\": 1,\n  ",
                "Unexpected end-of-input within/between Object entries",
            ),
            // A separator INSIDE a string separates nothing: that document ran out of input
            // while still reading the string.
            (
                "a string left open on a comma",
                r#"{"a": "one,"#,
                "Unexpected end-of-input: expected close marker for String",
            ),
            (
                "a member whose value never came",
                r#"{"a":"#,
                "Unexpected end-of-input: expected a value",
            ),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = unexpected_input(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The location clause names the position and never the document.
    #[test]
    fn the_location_clause_names_the_position_and_not_the_document() {
        let clause = source_location(2, 7);
        assert!(
            clause.starts_with("\n at [Source: REDACTED "),
            "the source must be reported as redacted, got {clause:?}"
        );
        assert!(
            clause.ends_with("line: 2, column: 7]"),
            "the clause must name the position it was given, got {clause:?}"
        );
    }

    /// The clause for a value position lists the classes this module states once, so the
    /// sentence and the list cannot drift apart.
    #[test]
    fn the_value_position_clause_lists_the_classes_a_value_may_begin_with() {
        assert!(
            VALUE_POSITION.contains(VALUE_CLASSES),
            "the clause {VALUE_POSITION:?} must list {VALUE_CLASSES:?}"
        );
    }

    /// One row: what a failing assertion calls it, the document, and the position the
    /// container it ran out of input inside opens at (`None` when the fault names none).
    type ContainerRow = (&'static str, &'static str, Option<(usize, usize)>);

    /// The container left open is read for the fault class that names one, and for no other.
    ///
    /// The rows differ along which collection was left open, how deeply it nests, whether a
    /// mark inside a string could be mistaken for one, which line it opens on, and which
    /// other fault class the document carries.
    #[test]
    fn the_container_a_parse_ran_out_of_input_inside_is_read_at_its_opening_mark() {
        let rows: [ContainerRow; 10] = [
            ("an object left open", r#"{"a": 1"#, Some((1, 1))),
            ("an array left open", "[1, 2", Some((1, 1))),
            // The innermost context is the one the parse was in.
            ("an inner object left open", r#"{"a": {"b": 1"#, Some((1, 7))),
            // Every closed collection is matched off, so the mark left over is the outer
            // one even though inner ones opened after it.
            (
                "an outer object left open past closed inner ones",
                r#"{"a": {"b": 1}, "c": [2]"#,
                Some((1, 1)),
            ),
            (
                "an array left open inside an object",
                r#"{"a": [1, 2"#,
                Some((1, 7)),
            ),
            // A brace inside a string is text, not structure.
            (
                "a brace inside a string value",
                r#"{"a": "{{{""#,
                Some((1, 1)),
            ),
            (
                "an escaped quote inside a string value",
                r#"{"a": "x\"y""#,
                Some((1, 1)),
            ),
            ("an object left open on a later line", "{\n  \"a\": {\n", Some((2, 8))),
            // Faults that are not a container left open.
            ("a string left open", r#"{"a": "text"#, None),
            ("a bare word where a value belongs", "nope", None),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = unclosed_container_start(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} (the parse stopped at \
                 line {}, column {}: {})",
                error.line(),
                error.column(),
                cause(&error)
            );
        }
    }

    /// The position reported is one past the end for a parse that ran out of input, and the
    /// parse's own otherwise.
    ///
    /// The rows differ along whether the document ended early at all, how long it is, and
    /// whether it carries more than one line.
    #[test]
    fn the_position_a_parse_stopped_at_is_one_past_the_end_when_the_input_ran_out() {
        // (case, the document, the position a location clause names)
        let rows: [(&str, &str, (usize, usize)); 5] = [
            ("an object left open", r#"{"a": 1"#, (1, 8)),
            ("the empty document", "", (1, 1)),
            ("a document that ends on its second line", "{\n  \"a\": 1", (2, 9)),
            // A fault that stopped ON a character keeps the parse's own position.
            ("an unquoted field name", r#"{event: 1}"#, (1, 2)),
            ("a missing comma between members", r#"{"a": 1 "b": 2}"#, (1, 9)),
        ];

        for (case, document, expected) in rows {
            let error = failure(document);
            let actual = stopped_at(document, &error);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} ({})",
                cause(&error)
            );
        }
    }

    /// A location clause names the source it was given and the position in it, and the
    /// redacted form is that clause with the source withheld.
    #[test]
    fn a_location_clause_names_its_source_and_position() {
        let clause = location_clause("(String)\"{}\"", 3, 4);
        assert_eq!(
            clause, "[Source: (String)\"{}\"; line: 3, column: 4]",
            "the clause must name the source it was given, got {clause:?}"
        );
        assert_eq!(
            source_location(2, 7),
            format!("\n at {}", location_clause(REDACTED_SOURCE, 2, 7)),
            "the redacted clause must be the same clause with the source withheld"
        );
    }
}
