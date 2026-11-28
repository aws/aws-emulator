// SPDX-License-Identifier: CC0-1.0

//! The DIALECT one database's engine speaks, as the service that owns that
//! database states it.
//!
//! [`super::session`] holds the session and [`super::sql_engine`] runs the SQL,
//! and neither of them knows which product's engine a resource publishes. But a
//! client talks to a PRODUCT: the statements it writes are that product's
//! dialect, the sentence `version()` answers names that product, and a refusal
//! carries the `SQLSTATE` that product reports it under. Those three are the
//! owning service's facts, so the service supplies them here
//! ([`super::ServedDatabase::dialect`]) and the session asks before it hands a
//! statement to the engine.
//!
//! A service that supplies none is served as it was before: the statement goes
//! to the engine as the client wrote it, `version()` answers the version the
//! resource publishes, and a failure is reported under the class the engine
//! itself named.
//!
//! What this module OWNS, because it is the WIRE's and not any one product's, is
//! the PostgreSQL spelling of the session functions ([`respell_session_keywords`]):
//! `current_user` and its siblings are reserved words a PostgreSQL client writes
//! without parentheses, and the engine behind this endpoint publishes them as
//! functions ([`super::sql_engine`]'s server functions), so the one spelling is
//! read as the other.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use super::session::Answer;
use super::sql_engine::{DatabaseEngine, EngineError, EngineView};
use super::LoginRefusal;

/// What the session does with one statement after the engine's own dialect has
/// read it.
#[derive(Debug)]
pub enum DialectReading {
    /// Run this SQL on the engine: the statement as the client wrote it, or the
    /// engine's own spelling of it where the dialect re-spelled it.
    Run(String),
    /// The dialect answered the statement itself, because what it asks for is
    /// not the engine's work (a catalogue of the server's own, a control the
    /// engine has no grammar for).
    Answered(Answer),
    /// The dialect refuses it, with the message the product reports for it.
    Refused(EngineError),
}

/// How a refused statement is reported to the client, as the PRODUCT behind the
/// engine reports it.
///
/// A client branches on the `SQLSTATE` and reads the message; some products say
/// more about a refusal in the detail field beside it, and what they say there
/// is part of what a client reads. Carrying the three together keeps one
/// refusal's wording in one place.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The `SQLSTATE` the product reports the failure under.
    pub sqlstate: &'static str,
    /// The message, as the product words it for this failure.
    pub message: String,
    /// What the product says in the detail field beside that message, where it
    /// says anything.
    pub detail: Option<String>,
}

/// The dialect of the engine ONE service's database resource publishes.
///
/// Implemented by the service that opened the plane, for the product its
/// resource runs. Every method has a default, so a service states only what its
/// engine does differently.
pub trait SqlDialect: Send + Sync + 'static {
    /// Read one statement as this engine's dialect, and answer what the session
    /// should do with it.
    ///
    /// `engine` is the database's own engine and `view` the calling session's,
    /// so a dialect that has to put something on the engine before the statement
    /// runs — a catalogue of this product's own, materialised out of the
    /// engine's catalog — does it here. Called on the BLOCKING pool, where the
    /// engine's statements run.
    fn read(&self, statement: &str, engine: &DatabaseEngine, view: &mut EngineView)
        -> DialectReading;

    /// The sentence `version()` answers on this engine, for a resource that
    /// publishes `server_version`, where this product words it its own way;
    /// `None` leaves the wire's own banner for that version.
    ///
    /// The wire's `server_version` parameter carries the version itself, which
    /// is what a driver reads; `version()` is a SENTENCE, and which product's
    /// sentence it is belongs to the service that owns the engine.
    fn version_sentence(&self, server_version: &str) -> Option<String> {
        let _ = server_version;
        None
    }

    /// The `SQLSTATE`, message and detail this product reports `error` under,
    /// where any of them differs from what the engine itself said; `None`
    /// leaves the wire's own mapping and the engine's own message.
    fn refusal(&self, error: &EngineError) -> Option<Refusal> {
        let _ = error;
        None
    }

    /// The settings a session on this engine STARTS with, beside the ones the
    /// wire itself publishes — the configuration parameters whose default is
    /// this product's rather than the engine's.
    ///
    /// A client reads one with `SHOW <name>` and `current_setting('<name>')`, and
    /// branches on it: the isolation level a transaction runs at unless it asks
    /// for another is the product's own answer, not a value this endpoint may
    /// guess. A service that states none leaves the wire's own set.
    fn session_settings(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// The DATABASE USER a session that logged on as `login` runs as.
    ///
    /// The name a credential logs on under is not always the name of the
    /// database user it stands for: a product whose control plane mints a
    /// credential may encode in that name how the user is to be resolved. What
    /// the engine's session functions then answer is the USER, which is what
    /// this maps `login` to.
    fn session_user(&self, login: &str) -> String {
        login.to_string()
    }

    /// The `SQLSTATE` and the sentence this product refuses the login of `user`
    /// with, where either differs from the wire's own; `None` leaves the wire's.
    ///
    /// A product whose control plane mints database credentials refuses one of
    /// those credentials in its own words, and a client branches on them.
    fn login_refusal(&self, refusal: &LoginRefusal, user: &str) -> Option<(&'static str, String)> {
        let _ = (refusal, user);
        None
    }
}

/// The PostgreSQL session functions a client writes as RESERVED WORDS, without
/// parentheses, and the engine behind this endpoint publishes as functions of no
/// arguments.
///
/// Source: PostgreSQL's "System Information Functions and Operators", which
/// notes of `current_catalog`, `current_role`, `current_schema`, `current_user`,
/// `session_user` and `user` that they "do not follow the function calling
/// conventions" and "must be called without trailing parentheses". The engine
/// has no such notion, so a bare one of these reads as a column it cannot find;
/// re-spelled as the call, it answers from the calling session's own view.
const SESSION_KEYWORDS: &[&str] = &[
    "current_catalog",
    "current_database",
    "current_role",
    "current_schema",
    "current_user",
    "session_user",
];

/// Re-spell every bare [`SESSION_KEYWORDS`] word of `sql` as the call the engine
/// publishes it under, or `None` when the statement carries none — a statement
/// nothing applied to is run exactly as the client wrote it.
///
/// Read off the TOKEN stream the wire's own dialect produces (the maintained
/// parser every service in this workspace reads SQL with), which is what keeps a
/// keyword inside a string literal — of any form that dialect admits — a quoted
/// identifier, a dollar-quoted body or a comment part of that literal,
/// identifier, body or comment. Only a word that stands on its own is
/// re-spelled: one already followed by `(` is a call, and one preceded by `.` is
/// a column of the table named before it.
///
/// What is answered is the CLIENT's own text with `()` SPLICED IN after each
/// such word, byte for byte everywhere else: the tokenizer un-escapes what it
/// reads (a doubled quote inside a quoted identifier, the `\'` of an escape
/// string), so a statement rebuilt from the tokens would not be the statement
/// the client sent. Nothing is re-rendered.
#[must_use]
pub fn respell_session_keywords(sql: &str) -> Option<String> {
    let dialect = PostgreSqlDialect {};
    let tokens = Tokenizer::new(&dialect, sql).tokenize_with_location().ok()?;
    // The ENDS of the words to re-spell, in the order they appear, read off each
    // token's own span.
    let mut spliced: Vec<usize> = Vec::new();
    let mut cursor = Cursor::new(sql);
    let mut qualified = false;
    for (index, token) in tokens.iter().enumerate() {
        let names_a_session_function = match &token.token {
            Token::Word(word) => {
                word.quote_style.is_none()
                    && SESSION_KEYWORDS
                        .iter()
                        .any(|keyword| word.value.eq_ignore_ascii_case(keyword))
            }
            _ => false,
        };
        // The look-ahead runs only for a word that could be re-spelled at all:
        // a statement of nothing but white space would otherwise be walked once
        // per token, and the tokenizer emits one token per space.
        if names_a_session_function
            && !qualified
            && !next_significant(&tokens, index + 1)
                .is_some_and(|next| matches!(tokens[next].token, Token::LParen))
        {
            let end = cursor.offset_of(token.span.end.line, token.span.end.column)?;
            spliced.push(end);
        }
        if !matches!(token.token, Token::Whitespace(_)) {
            qualified = matches!(token.token, Token::Period);
        }
    }
    if spliced.is_empty() {
        return None;
    }
    let mut respelled = String::with_capacity(sql.len() + 2 * spliced.len());
    let mut from = 0usize;
    for at in spliced {
        respelled.push_str(sql.get(from..at)?);
        respelled.push_str("()");
        from = at;
    }
    respelled.push_str(sql.get(from..)?);
    Some(respelled)
}

/// The index of the first token at or after `from` that is not white space.
fn next_significant(tokens: &[TokenWithSpan], from: usize) -> Option<usize> {
    (from..tokens.len()).find(|index| !matches!(tokens[*index].token, Token::Whitespace(_)))
}

/// Re-spell every reference to a relation of the SCHEMA `schema` as the ONE
/// quoted identifier the engine holds that relation under
/// (`information_schema.tables` -> `"information_schema.tables"`), or `None`
/// where the statement names none.
///
/// The engine behind an endpoint holds ONE database and no schemas
/// ([`super::sql_engine`]), so a relation of a schema is held under its
/// qualified name as a single identifier — which is how the catalog views are
/// materialised ([`super::catalog_views::rebuild`]). A client writes the
/// qualified form, and unquoted that would address a schema the engine does not
/// have, so the one spelling is read as the other.
///
/// Read off the TOKEN stream the wire's own dialect produces, exactly as
/// [`respell_session_keywords`] is, so the schema's name inside a string
/// literal, a quoted identifier, a dollar-quoted body or a comment stays part of
/// that literal, identifier, body or comment — and a name already written as one
/// quoted identifier is left alone. What is answered is the CLIENT's own text
/// with those references replaced and every other byte as it was.
#[must_use]
pub fn respell_schema_relations(sql: &str, schema: &str) -> Option<String> {
    let dialect = PostgreSqlDialect {};
    let tokens = Tokenizer::new(&dialect, sql).tokenize_with_location().ok()?;
    // The byte ranges to replace, in the order they appear, and the quoted
    // identifier each becomes.
    let mut replaced: Vec<(usize, usize, String)> = Vec::new();
    let mut cursor = Cursor::new(sql);
    let mut qualified = false;
    for (index, token) in tokens.iter().enumerate() {
        let names_the_schema = match &token.token {
            Token::Word(word) => word
                .quote_style
                .is_none()
                .then(|| word.value.clone())
                .filter(|written| written.eq_ignore_ascii_case(schema)),
            _ => None,
        };
        if let Some(written) = names_the_schema.filter(|_| !qualified) {
            // `<schema> . <relation>`, which is the only form that addresses a
            // relation of it; the word on its own is left as the client wrote it.
            let dot = next_significant(&tokens, index + 1)
                .filter(|next| matches!(tokens[*next].token, Token::Period));
            let relation = dot
                .and_then(|dot| next_significant(&tokens, dot + 1))
                .and_then(|next| match &tokens[next].token {
                    Token::Word(word) => Some((next, word.value.clone())),
                    _ => None,
                });
            if let Some((end_index, relation)) = relation {
                let from = cursor.offset_of(token.span.start.line, token.span.start.column)?;
                let end = cursor.offset_of(
                    tokens[end_index].span.end.line,
                    tokens[end_index].span.end.column,
                )?;
                let held = format!("{written}.{relation}").replace('"', "\"\"");
                replaced.push((from, end, format!("\"{held}\"")));
            }
        }
        if !matches!(token.token, Token::Whitespace(_)) {
            qualified = matches!(token.token, Token::Period);
        }
    }
    if replaced.is_empty() {
        return None;
    }
    let mut respelled = String::with_capacity(sql.len());
    let mut from = 0usize;
    for (start, end, held) in replaced {
        respelled.push_str(sql.get(from..start)?);
        respelled.push_str(&held);
        from = end;
    }
    respelled.push_str(sql.get(from..)?);
    Some(respelled)
}

/// A position in a text, moved forward to the tokenizer's locations — a 1-based
/// line and a 1-based COLUMN counted in characters — to read their byte offsets.
///
/// Forward only, because the locations a token stream reports are
/// non-decreasing: the whole walk costs ONE pass over the text however many
/// locations are asked for.
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
    /// which the caller leaves the statement exactly as the client wrote it.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bare session keywords are re-spelled as the calls the engine
    /// publishes, and nothing else is touched.
    #[test]
    fn only_a_bare_session_keyword_is_respelled() {
        for (case, sql, expected) in [
            (
                "a bare keyword",
                "SELECT current_user",
                Some("SELECT current_user()".to_string()),
            ),
            (
                "two of them, in either case",
                "SELECT CURRENT_USER, session_user",
                Some("SELECT CURRENT_USER(), session_user()".to_string()),
            ),
            (
                "one already called",
                "SELECT current_user()",
                None,
            ),
            (
                "one called with space before the parenthesis",
                "SELECT current_user ()",
                None,
            ),
            (
                "a column of a table named the same",
                "SELECT t.current_user FROM t",
                None,
            ),
            (
                "inside a string literal",
                "SELECT 'current_user'",
                None,
            ),
            (
                "inside a quoted identifier",
                "SELECT \"current_user\" FROM t",
                None,
            ),
            (
                "inside a line comment",
                "SELECT 1 -- current_user\n",
                None,
            ),
            (
                "inside a block comment",
                "SELECT /* current_user */ 1",
                None,
            ),
            (
                "inside a dollar-quoted body",
                "SELECT $tag$current_user$tag$",
                None,
            ),
            (
                "inside an escape string whose quote is escaped",
                "SELECT E'it\\'s current_user'",
                None,
            ),
            // A keyword to re-spell BESIDE a token the tokenizer un-escapes: the
            // client's own text is answered with the call spliced in and every
            // other byte exactly as it was, so neither the escape string's `\'`
            // nor the doubled quote of the identifier is rewritten.
            (
                "a keyword beside an escape string",
                "UPDATE t SET note = E'it\\'s current_user' WHERE u = current_user",
                Some(
                    "UPDATE t SET note = E'it\\'s current_user' WHERE u = current_user()"
                        .to_string(),
                ),
            ),
            (
                "a keyword beside a quoted identifier carrying a doubled quote",
                "SELECT \"a\"\"b\" FROM t WHERE u = current_user",
                Some("SELECT \"a\"\"b\" FROM t WHERE u = current_user()".to_string()),
            ),
            (
                "a keyword beside a dollar-quoted body naming one",
                "SELECT $tag$current_user$tag$, current_user",
                Some("SELECT $tag$current_user$tag$, current_user()".to_string()),
            ),
            (
                "a keyword on a later line, after a comment",
                "SELECT 1 -- current_user\nUNION SELECT current_user",
                Some("SELECT 1 -- current_user\nUNION SELECT current_user()".to_string()),
            ),
            (
                "a longer word that merely starts with one",
                "SELECT current_users FROM t",
                None,
            ),
            (
                "one in a predicate, with the rest of the statement kept",
                "SELECT * FROM t WHERE owner = current_user ORDER BY 1",
                Some("SELECT * FROM t WHERE owner = current_user() ORDER BY 1".to_string()),
            ),
        ] {
            assert_eq!(
                respell_session_keywords(sql),
                expected,
                "case {case:?}: expected {expected:?} for {sql:?}, got {:?}",
                respell_session_keywords(sql)
            );
            // And what is answered is the CLIENT's own text with the call
            // spliced in: taking the spliced `()` back out gives the statement
            // byte for byte, so nothing the tokenizer un-escaped was
            // re-rendered. (No case above writes `()` itself.)
            if let Some(respelled) = respell_session_keywords(sql) {
                assert_eq!(
                    respelled.replace("()", ""),
                    sql,
                    "case {case:?}: every byte outside the spliced call must be the client's \
                     own, got {respelled:?}"
                );
            }
        }
    }

    /// RULE: a relation of a SCHEMA is re-spelled as the one quoted identifier
    /// the engine holds it under, and nothing else about the statement is
    /// touched.
    ///
    /// The rows span what the rule generalises over: the views of the schema, the
    /// case a client writes them in, several references in one statement, a name
    /// of the schema that addresses no relation of it, and the schema's name
    /// inside a literal, an identifier, a comment or another name's qualifier.
    #[test]
    fn only_a_relation_of_the_schema_is_respelled() {
        let schema = "information_schema";
        for (case, sql, expected) in [
            (
                "a read of one view",
                "SELECT table_name FROM information_schema.tables",
                Some("SELECT table_name FROM \"information_schema.tables\"".to_string()),
            ),
            (
                "the same, in upper case",
                "SELECT * FROM INFORMATION_SCHEMA.COLUMNS",
                Some("SELECT * FROM \"INFORMATION_SCHEMA.COLUMNS\"".to_string()),
            ),
            (
                "two references in one statement",
                "SELECT (SELECT count(*) FROM information_schema.tables), \
                 (SELECT count(*) FROM information_schema.columns)",
                Some(
                    "SELECT (SELECT count(*) FROM \"information_schema.tables\"), \
                     (SELECT count(*) FROM \"information_schema.columns\")"
                        .to_string(),
                ),
            ),
            (
                "a predicate after it",
                "SELECT count(*) FROM information_schema.tables WHERE table_name = 'orders'",
                Some(
                    "SELECT count(*) FROM \"information_schema.tables\" WHERE table_name = \
                     'orders'"
                        .to_string(),
                ),
            ),
            (
                "the schema named on its own",
                "SELECT information_schema FROM t",
                None,
            ),
            (
                "the name inside a string literal",
                "INSERT INTO t (note) VALUES ('information_schema.tables')",
                None,
            ),
            (
                "the name inside a quoted identifier",
                "SELECT * FROM \"information_schema.tables\"",
                None,
            ),
            (
                "the name inside a comment",
                "SELECT 1 -- information_schema.tables\n",
                None,
            ),
            (
                "a column of a table named the same",
                "SELECT t.information_schema.tables FROM t",
                None,
            ),
            ("a statement naming no schema at all", "SELECT * FROM t", None),
        ] {
            let respelled = respell_schema_relations(sql, schema);
            assert_eq!(
                respelled, expected,
                "case {case:?}: expected {expected:?} for {sql:?}, got {respelled:?}"
            );
        }
    }
}
