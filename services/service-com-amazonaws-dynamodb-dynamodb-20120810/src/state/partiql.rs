// SPDX-License-Identifier: CC0-1.0

//! The PartiQL statements `ExecuteStatement` runs, parsed into the primitives the
//! classic item API already works in.
//!
//! DynamoDB serves "reads and singleton writes on data stored in DynamoDB, using
//! PartiQL" (the `ExecuteStatement` doc) over the SAME item store the classic API
//! reads and writes: a `SELECT` reports the stored items, and `INSERT`, `UPDATE` and
//! `DELETE` mutate them, so a statement's effect is visible to every later `GetItem`,
//! `Query` or statement and a classic write is visible to every later `SELECT`.
//!
//! This module is the dialect's front end only. A statement is lexed and parsed here
//! into
//!
//! * the table (and optional index) it addresses,
//! * the item an `INSERT` carries,
//! * the `WHERE` predicate, RENDERED back into the `ConditionExpression` grammar
//!   [`super::ParsedCondition`] already parses and evaluates, and
//! * the `SET`/`REMOVE` clauses, rendered into the `UpdateExpression` grammar
//!   [`super::ParsedUpdate`] already applies,
//!
//! so the VALUE semantics (which comparison holds between which two attribute values,
//! what an update action does to an item) have exactly one implementation in this
//! crate, shared with `UpdateItem`, `PutItem` and `DeleteItem`. Rendering goes through
//! `#name` aliases and `:value` placeholders, so no attribute name or value is ever
//! spliced into expression text.
//!
//! Why the dialect is lexed here rather than taken from a crate: `partiql-parser`
//! (the PartiQL reference implementation for Rust, 0.15) parses the query language but
//! NOT the parts DynamoDB's dialect is made of — it rejects `INSERT INTO t VALUE
//! {'a': 1}`, `UPDATE`/`DELETE` statements, the `?` parameter marker every
//! `Parameters` entry fills, and numeric literals past what `i64`/`Decimal` hold,
//! while DynamoDB's `N` carries 38 significant digits. What is left of the dialect
//! once those are gone is not the dialect, so this grammar — DynamoDB's own, like the
//! `ConditionExpression` grammar in [`super`] and the `KeyConditionExpression` grammar
//! in the `Query` handler — is parsed here, and everything a statement cannot say is
//! refused with a `ValidationException` rather than guessed at.
//!
//! A refusal states the DEFECT CLASS, not one catch-all sentence: a parse defect
//! carries the parser's phrase for the position the parse stopped at
//! ([`ParseDefect`]), a call names the function it called
//! ([`unrecognized_function`], [`requires_document_path`],
//! [`incorrect_operand_count`]), and a refusal about a run of the statement text
//! reports where the statement spells it ([`Position`]) — so a client can tell a
//! quoting mistake from an unknown function, and nothing but the request's own text
//! appears in the message.
//!
//! Every bound the walk uses is this module's own constant
//! ([`MAX_STATEMENT_TOKENS`], [`MAX_STATEMENT_DEPTH`]), so neither the token vector
//! nor the parser's stack is a function of what a caller sends.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

use super::*;

/// The most bytes one statement may hold: `PartiQLStatement` is modelled at most
/// 8192 bytes, named here so the bound the lexer enforces is this module's own — the
/// text is refused BEFORE it is collected, the way
/// [`MAX_EXPRESSION_BYTES`] bounds an expression member.
pub const MAX_STATEMENT_BYTES: usize = 8192;

/// The most tokens one statement may hold.
///
/// `PartiQLStatement` is modelled at most 8192 bytes, and the shortest token a
/// statement can spell takes a byte and a separator, so this is that ceiling named:
/// it bounds the token vector before any parsing starts, the way
/// [`MAX_EXPRESSION_TOKENS`] bounds an expression's.
pub const MAX_STATEMENT_TOKENS: usize = 4096;

/// How deeply one statement's GRAMMAR may nest — a parenthesised predicate, a `NOT`,
/// a `list_append`: the same ceiling an expression's parse tree is held to
/// ([`MAX_EXPRESSION_DEPTH`]), for the same reason — nesting is the caller's choice, so
/// the parser's stack use must not be. The VALUES a statement carries are held to the
/// DOCUMENTED [`MAX_ATTRIBUTE_DEPTH`] instead, the ceiling every other write path holds
/// an attribute value to, so the statement surface and the classic item API accept the
/// same items.
pub const MAX_STATEMENT_DEPTH: usize = 16;

/// Where in the statement text one token sits.
///
/// A refusal that reports a position reports it as `<line>:<column>:<length>` — the
/// line and column the statement spells the offending run at, counted in characters
/// from 1, and how many characters it takes — so every part of it is a fact of the
/// text the request sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    line: usize,
    column: usize,
    width: usize,
}

impl Position {
    /// This position as a refusal reports it.
    fn stated(&self) -> String {
        format!("{}:{}:{}", self.line, self.column, self.width)
    }
}

/// Which defect a statement that does not parse carries, as the phrase the parser
/// reports for the position it stopped at.
///
/// DynamoDB states a parse defect as ONE fixed sentence
/// ([`statement_parse_error`]) whose tail is the parser's phrase for what was wrong
/// there, so a client can tell a statement that is no data manipulation from a name
/// that is not a name from text that carries on past a complete statement. Each phrase
/// below is the PartiQL parser's own for that position.
///
/// The phrases this surface is RECORDED answering with are
/// [`Self::NotDataManipulation`], [`Self::Identifier`], [`Self::TrailingToken`],
/// [`Self::UnexpectedTerm`], [`Self::FromAfterSelectList`], [`Self::AfterSemicolon`] and
/// the EMPTY phrase of [`Self::Unterminated`]. [`Self::Keyword`] covers the keyword
/// positions no recorded answer names, with the reference parser's own phrase, so the
/// class a client sees is still the class that failed; a recording that names one of those
/// positions is what would settle its wording, the way the `FROM` of a `SELECT` list has
/// its own phrase above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseDefect {
    /// The text read as ONE COMPLETE expression — a query of the language — and states none
    /// of the verbs that read or write items, so it manipulates no data whatever else it
    /// spells: a bare name, a literal, a predicate. Text that opens with no such verb and is
    /// no complete expression either keeps this defect as well, while text with a token left
    /// over past the expression stopped at that token
    /// ([`Self::TrailingToken`]), and text whose leading keywords name one of the language's
    /// other operations is reported as that operation instead
    /// ([`unsupported_statement_operation`]).
    NotDataManipulation,
    /// A NAME was required there — a table, an index, an attribute or an alias — and
    /// what the statement spells is not one (a string literal, say).
    Identifier,
    /// A keyword the grammar requires at that position is not there.
    Keyword,
    /// The `FROM` a `SELECT` list must be followed by is not there — the one keyword
    /// position the parser phrases by naming the keyword and the clause it follows.
    FromAfterSelectList,
    /// Text carries on past the `;` that ended the one query a statement states.
    AfterSemicolon,
    /// One complete statement of the dialect — a data-manipulation statement, or the one
    /// expression a query is — and the text carries on past it with a token nothing in the
    /// dialect can follow an expression with.
    TrailingToken,
    /// The token at that position is not one the grammar admits there.
    UnexpectedTerm,
    /// A bare word at a position where only a NAME may stand is one of the dialect's
    /// keywords: a name that collides with a keyword is spelled `"quoted"`, so the
    /// unquoted word is read as the keyword it is and the grammar has no place for it
    /// there.
    UnexpectedKeyword,
    /// The text STOPS inside a quoted run — a string literal or a quoted name the
    /// statement never closes — so there is no token at the position to name and the
    /// parser reports no phrase for it at all.
    Unterminated,
    /// A LEXEME the scanner cannot read at all: a numeric literal whose exponent marker
    /// carries no exponent, a number run together with a word. The text never became
    /// tokens, so there is no grammar position to name and the parser reports no phrase
    /// — the same empty tail an unterminated run earns.
    Lexeme,
    /// The `}` that closes a tuple literal is not there.
    ExpectedRightCurly,
    /// The `]` that closes a list literal is not there.
    ExpectedRightBracket,
    /// The `>>` that closes a bag literal is not there.
    ExpectedRightDoubleAngleBracket,
}

impl ParseDefect {
    /// The parser's phrase for this defect, which is EMPTY where it reports none.
    fn phrase(self) -> &'static str {
        match self {
            Self::NotDataManipulation => "Expected data manipulation",
            Self::Identifier => "Expected identifier for simple path",
            Self::Keyword => "Expected keyword",
            Self::FromAfterSelectList => "Expected FROM after SELECT list",
            Self::AfterSemicolon => "Unexpected token after semicolon. (Only one query is allowed.)",
            Self::TrailingToken => "Unexpected token after expression",
            Self::UnexpectedTerm => "Unexpected term",
            Self::UnexpectedKeyword => "Unexpected keyword",
            Self::Unterminated | Self::Lexeme => "",
            Self::ExpectedRightCurly => "Expected RIGHT_CURLY",
            Self::ExpectedRightBracket => "Expected RIGHT_BRACKET",
            Self::ExpectedRightDoubleAngleBracket => "Expected RIGHT_DOUBLE_ANGLE_BRACKET",
        }
    }
}

/// How a statement this dialect cannot parse is refused: a `ValidationException`
/// carrying the fixed sentence DynamoDB states a parse defect with, followed by the
/// parser's own phrase for the position the parse stopped at. The statement itself is
/// not echoed back, and a defect the parser reports no phrase for leaves the sentence's
/// tail empty — the fixed text, its colon and the space that separates the two are the
/// whole of what such a refusal states.
fn statement_parse_error(defect: ParseDefect) -> RequestError {
    RequestError::Validation(format!(
        "Statement wasn't well formed, can't be processed: {}",
        defect.phrase()
    ))
}

/// The operations the statement language names that this surface does not run, each
/// paired with the keywords that name it.
///
/// This surface runs "reads and singleton writes on data stored in DynamoDB, using
/// PartiQL" (the `ExecuteStatement` doc) — the four data-manipulation verbs — while the
/// language also spells SCHEMA operations, each as a verb and the kind of object it acts
/// on. Text that opens with such a pair names an operation rather than failing to read as
/// a statement, so it is refused as the operation this surface does not run
/// ([`unsupported_statement_operation`]) and not as a parse defect.
///
/// Each entry is the verb, the kind of object, and the name of the operation the pair
/// names — the two keywords written as one operation name, the way this service names its
/// own operations.
const NON_DML_STATEMENT_OPERATIONS: [(&str, &str, &str); 4] = [
    ("create", "table", "CreateTable"),
    ("create", "index", "CreateIndex"),
    ("drop", "table", "DropTable"),
    ("drop", "index", "DropIndex"),
];

/// How text naming an operation this surface does not run is refused: the fixed sentence,
/// over the name of the operation the text's leading keywords name
/// ([`NON_DML_STATEMENT_OPERATIONS`]).
///
/// A client can tell this from a statement that did not read as one at all
/// ([`statement_parse_error`]): the text states an operation of the language and what it
/// is told is that this surface does not run that operation, so retry and report logic
/// reading the message is not handed a parse defect for a statement that has none.
fn unsupported_statement_operation(operation: &str) -> RequestError {
    RequestError::Validation(format!("Unsupported operation: {operation}"))
}

/// How a call to a function this dialect does not define is refused: the name the
/// statement called, and where it spells it.
fn unrecognized_function(name: &str, at: Position) -> RequestError {
    RequestError::Validation(format!("Unrecognized function: {name} at {}", at.stated()))
}

/// How an operand that is not a document path is refused, naming the operator or
/// function that takes one — the functions that read an attribute of the item
/// (`attribute_exists`, `size`, …) read a PATH, not a value.
fn requires_document_path(operator_or_function: &str) -> RequestError {
    RequestError::Validation(format!(
        "Operator or function requires a document path; operator or function: \
         {operator_or_function}"
    ))
}

/// How a call carrying the wrong number of operands is refused, naming what was
/// called and how many operands the call carried.
fn incorrect_operand_count(operator_or_function: &str, operands: usize) -> RequestError {
    RequestError::Validation(format!(
        "Incorrect number of operands for operator or function; operator or function: \
         {operator_or_function}, number of operands: {operands}"
    ))
}

/// How an `INSERT` whose `INTO` clause carries an index qualifier is refused: the
/// positioned sentence about that clause.
///
/// An index projects a table's items and is written through no statement ("attributes
/// that are copied (projected) from the table into the global secondary index",
/// `GlobalSecondaryIndex$Projection`), so the clause naming a statement's source names
/// one table and nothing else. The position reported is where the statement spells that
/// TABLE, so every part of the message is a fact of the request. An `UPDATE` and a
/// `DELETE` are refused with [`unsupported_on_an_index`] instead.
fn single_table_in_data_manipulation(at: Position) -> RequestError {
    RequestError::Validation(format!(
        "FROM clause may only contain a single table name in data manipulation statements at {}",
        at.stated()
    ))
}

/// How an `UPDATE` or a `DELETE` whose source carries an index qualifier is refused:
/// the fixed sentence DynamoDB states for an operation asked of an index, which is the
/// same one the classic surface states for one.
///
/// Both verbs CHANGE an item of the table, and an index holds a projection of one
/// rather than items of its own, so a qualified source asks for a write through an
/// index — which is what the refusal says. An `INSERT` names its target inside a clause
/// and earns that clause's own sentence
/// ([`single_table_in_data_manipulation`]) instead.
fn unsupported_on_an_index() -> RequestError {
    RequestError::Validation("This operation is not supported on an index".to_string())
}

/// How a component of a source path that is spelled as the EMPTY name is refused.
///
/// A `"quoted"` component names a table or an index, and the empty name names neither,
/// so the path is refused as the malformed path it is rather than resolved and reported
/// as a missing index.
fn empty_path_component() -> RequestError {
    RequestError::Validation("Path component cannot be an empty string".to_string())
}

/// How a call standing where only a document PATH may stand — a `SELECT`'s projection
/// list — is refused: the component the parser stopped at, and where the statement
/// spells it.
///
/// A projection names paths into the item, so the parse of a projection item is the
/// parse of a path: a `(` is no step of one, and the refusal is about that COMPONENT
/// rather than about the function it would have called, which is why it carries the
/// position and not the name.
fn unexpected_path_component(at: Position) -> RequestError {
    RequestError::Validation(format!("Unexpected path component at {}", at.stated()))
}

/// How a `DELETE` whose `RETURNING` clause asks for the item as it stands AFTER the
/// write is refused, echoing the clause the statement wrote.
///
/// A `DELETE` leaves no item to report, so the only projection it can return is the one
/// it replaced; the sentence names the clause the caller spelled and the one clause that
/// is allowed instead.
fn invalid_returning_clause(clause: &str) -> RequestError {
    RequestError::Validation(format!(
        "Invalid returning clause: RETURNING {clause}. Only RETURNING ALL OLD * is allowed in \
         DELETE statements."
    ))
}

/// How a statement that binds an ALIAS to the table it names is refused.
///
/// PartiQL's grammar lets a source bind a name — `FROM table AS t`, or the same binding
/// with the keyword left out, `FROM table t` — and DynamoDB's statement surface serves
/// no such binding: every path of a statement names an attribute of the item, so a name
/// standing for the table is something no clause can read. The sentence is fixed and
/// names neither the alias nor the table.
fn aliasing_unsupported() -> RequestError {
    RequestError::Validation("Aliasing is not supported".to_string())
}

/// How a source path naming more than a table and an index of it is refused: the two
/// components a `FROM` path may hold are the table and the index of it a read is served
/// through.
const FROM_PATH_ARITY_MESSAGE: &str = "A path may contain at most 2 components in the FROM clause";

/// How a request whose `Parameters` list and whose statement's `?` markers do not
/// correspond is refused: "The parameters for the PartiQL statement, if any"
/// (`ExecuteStatement$Parameters`) fill the markers in order, so one list and one
/// statement that do not go together is a defect of the REQUEST rather than of either
/// part.
pub const PARAMETER_COUNT_MESSAGE: &str =
    "Number of parameters in request and statement don't match.";

/// How a statement that addresses ONE item, and whose `WHERE` clause does not pin every
/// key attribute by equality, is refused.
///
/// "ExecuteStatement allows you to perform reads and SINGLETON writes" (the operation's
/// doc): an `UPDATE` or a `DELETE` names one item, so a clause that leaves a key
/// attribute free names no one item.
pub const MANDATORY_KEY_EQUALITY_MESSAGE: &str =
    "Where clause does not contain a mandatory equality on all key attributes";

/// How a token the grammar has no place for is refused: a bare word that spells one of
/// the dialect's keywords is reported as the KEYWORD it is, and anything else as the
/// term it is.
///
/// The two phrases are the parser's own for those two positions, and which one a
/// statement earns is a fact of its text: `{status: 1}` writes a name where a tuple
/// takes a quoted one, and `{timestamp: 1}` writes a keyword there.
fn unexpected_token(token: Option<&Token>) -> RequestError {
    match token {
        Some(Token::Word(word)) if is_statement_keyword(word) => {
            statement_parse_error(ParseDefect::UnexpectedKeyword)
        }
        _ => statement_parse_error(ParseDefect::UnexpectedTerm),
    }
}

/// How a statement that parses but asks for something this dialect does not serve is
/// refused, naming the rule it broke.
///
/// What is left here is this surface's OWN bounds (the token count and the nesting
/// depth the parser is held to) and the shapes of the dialect it does not spell: each
/// AWS refusal class carries AWS's own sentence above instead.
fn statement_refused(detail: &str) -> RequestError {
    RequestError::Validation(format!("Invalid Statement: {detail}"))
}

/// How a statement that reads as a COMPLETE statement and is then followed by more
/// tokens is refused.
///
/// A statement is one expression, so anything after the one the parser read is not part
/// of it and nothing in the dialect can be — which is a different fact from a statement
/// that never parsed, and AWS states it with its own fixed sentence
/// ([`ParseDefect::TrailingToken`]).
fn unexpected_token_after_statement() -> RequestError {
    statement_parse_error(ParseDefect::TrailingToken)
}

/// The type codes `attribute_type` may name, in the order the STATEMENT surface's own
/// refusal lists them.
///
/// The same closed set the expression surface holds the function to
/// ([`crate::state::ATTRIBUTE_TYPE_NAMES`] — the members of the `AttributeValue` union),
/// listed in the order AWS states it in for a statement, which is not the order its
/// expression refusals use.
const STATEMENT_ATTRIBUTE_TYPE_NAMES: [&str; 10] =
    ["N", "BS", "L", "B", "NULL", "M", "S", "SS", "NS", "BOOL"];

/// `attribute_type`'s second operand names a TYPE, so a statement that spells a literal
/// which is not one of the type codes is refused as it is read.
///
/// The statement surface states the clause on its own, the way it states every other
/// refusal about a statement, and over its own ordering of the code set.
fn check_statement_type_name(operand: &PredicateOperand) -> Result<(), RequestError> {
    let PredicateOperand::Value(value) = operand else {
        return Ok(());
    };
    if let AttributeValue::S(name) = value {
        if STATEMENT_ATTRIBUTE_TYPE_NAMES.contains(&name.as_str()) {
            return Ok(());
        }
        return Err(RequestError::Validation(format!(
            "Invalid attribute type name found; type: {name}, valid types: {{{}}}",
            STATEMENT_ATTRIBUTE_TYPE_NAMES.join(",")
        )));
    }
    Err(RequestError::Validation(format!(
        "Incorrect operand type for operator or function; operator or function: attribute_type, \
         operand type: {}",
        attribute_value_type(value)
    )))
}

/// The relational operators compare ONE value with another, so each operand is a scalar.
///
/// "<, <=, >, >=" and `BETWEEN` order their operands, and only the scalar types `S`,
/// `N` and `B` have an order — a list, a map, a set, a boolean or the null type has no
/// place on either side of one. The operand is a literal the statement already spells,
/// so the statement is refused as it is read, naming the operator and the type of the
/// offending operand. Equality is not included: two values of any type can be equal.
fn check_relational_operands(
    operator: CompareOp,
    left: &PredicateOperand,
    right: &PredicateOperand,
) -> Result<(), RequestError> {
    let operator = match operator {
        CompareOp::Equal | CompareOp::NotEqual => return Ok(()),
        CompareOp::Less => "<",
        CompareOp::LessOrEqual => "<=",
        CompareOp::Greater => ">",
        CompareOp::GreaterOrEqual => ">=",
    };
    check_scalar_operands(operator, [left, right])
}

/// Hold every literal operand of an ordering comparison to the scalar types, naming the
/// operator the way the statement spells it.
fn check_scalar_operands<'o>(
    operator: &str,
    operands: impl IntoIterator<Item = &'o PredicateOperand>,
) -> Result<(), RequestError> {
    for operand in operands {
        let PredicateOperand::Value(value) = operand else {
            continue;
        };
        if !matches!(
            value,
            AttributeValue::S(_) | AttributeValue::N(_) | AttributeValue::B(_)
        ) {
            return Err(RequestError::Validation(format!(
                "Incorrect operand type for operator or function; operator or function: \
                 {operator}, operand type: {}",
                attribute_value_type(value)
            )));
        }
    }
    Ok(())
}

/// One number a statement carries, canonicalized, or the refusal its text earns.
///
/// The clause is the one every write path states for that numeric rule
/// ([`number_rejection`]), stated on its OWN rather than inside the
/// `<N> validation error(s) detected: ` envelope ([`unenveloped`]): the envelope reports
/// a constraint on an input MEMBER the request sent, and a literal inside the statement
/// text is not one — the statement is the member, and what it says is this parser's to
/// report.
fn statement_number(lexeme: &str) -> Result<String, RequestError> {
    canonical_number_text(lexeme).map_err(unenveloped)
}

// ===========================================================================
// The lexer
// ===========================================================================

/// One lexical token of a PartiQL statement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word: a keyword (`SELECT`, `AND`, `NULL`, …), a function name, or a
    /// table or attribute name written plainly.
    Word(String),
    /// A `"double-quoted"` identifier — how a name outside the bare-word character
    /// set, or one that collides with a keyword, is spelled.
    Quoted(String),
    /// A `'single-quoted'` string literal, with `''` standing for one quote.
    Text(String),
    /// A numeric literal, exactly as the statement spelled it: the lexeme is kept so
    /// the stored `N` carries the number it denotes to the documented 38 significant
    /// digits rather than whatever a primitive could hold.
    Number(String),
    /// The `?` marker one `Parameters` entry fills, in order.
    Parameter,
    Star,
    Compare(CompareOp),
    Open,
    Close,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    /// `<<`, which opens a PartiQL bag — how a statement spells a DynamoDB set.
    OpenBag,
    CloseBag,
    Comma,
    Dot,
    Colon,
    /// `;`, which ends the one query a statement states.
    Semicolon,
    Minus,
    Plus,
}

/// Which characters make up a bare word.
fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// One statement's tokens and where the text spells each of them: the two vectors are
/// the same length, so a token's position is the entry at its own index.
#[derive(Debug, Default)]
struct Lexemes {
    tokens: Vec<Token>,
    positions: Vec<Position>,
}

/// Split one statement into tokens, refusing a token count past
/// [`MAX_STATEMENT_TOKENS`] before any parsing starts.
fn tokenize(statement: &str) -> Result<Lexemes, RequestError> {
    if statement.len() > MAX_STATEMENT_BYTES {
        return Err(statement_refused(&format!(
            "a statement may hold at most {MAX_STATEMENT_BYTES} bytes"
        )));
    }
    let characters: Vec<char> = statement.chars().collect();
    let mut tokens = Vec::new();
    let mut positions: Vec<Position> = Vec::new();
    let mut at = 0;
    // Where the scan stands in the TEXT: which line it is on, and the character the
    // line starts at, so a column is counted from the line rather than from the
    // statement.
    let mut line = 1;
    let mut line_start = 0;
    while at < characters.len() {
        if tokens.len() >= MAX_STATEMENT_TOKENS {
            return Err(statement_refused(&format!(
                "a statement may hold at most {MAX_STATEMENT_TOKENS} tokens"
            )));
        }
        let character = characters[at];
        let start = at;
        let (token_line, token_column) = (line, start + 1 - line_start);
        match character {
            c if c.is_whitespace() => at += 1,
            '\'' | '"' => {
                let (text, next) = quoted(&characters, at, character)?;
                tokens.push(if character == '\'' {
                    Token::Text(text)
                } else {
                    Token::Quoted(text)
                });
                at = next;
            }
            c if c.is_ascii_digit() => {
                let (lexeme, next) = number(&characters, at)?;
                tokens.push(Token::Number(lexeme));
                at = next;
            }
            c if is_word_char(c) => {
                let start = at;
                while at < characters.len() && is_word_char(characters[at]) {
                    at += 1;
                }
                tokens.push(Token::Word(characters[start..at].iter().collect()));
            }
            '?' => {
                tokens.push(Token::Parameter);
                at += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                at += 1;
            }
            '=' => {
                tokens.push(Token::Compare(CompareOp::Equal));
                at += 1;
            }
            '!' if characters.get(at + 1) == Some(&'=') => {
                tokens.push(Token::Compare(CompareOp::NotEqual));
                at += 2;
            }
            '<' => {
                let (token, width) = match characters.get(at + 1) {
                    Some('<') => (Token::OpenBag, 2),
                    Some('=') => (Token::Compare(CompareOp::LessOrEqual), 2),
                    Some('>') => (Token::Compare(CompareOp::NotEqual), 2),
                    _ => (Token::Compare(CompareOp::Less), 1),
                };
                tokens.push(token);
                at += width;
            }
            '>' => {
                let (token, width) = match characters.get(at + 1) {
                    Some('>') => (Token::CloseBag, 2),
                    Some('=') => (Token::Compare(CompareOp::GreaterOrEqual), 2),
                    _ => (Token::Compare(CompareOp::Greater), 1),
                };
                tokens.push(token);
                at += width;
            }
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            '[' => {
                tokens.push(Token::OpenBracket);
                at += 1;
            }
            ']' => {
                tokens.push(Token::CloseBracket);
                at += 1;
            }
            '{' => {
                tokens.push(Token::OpenBrace);
                at += 1;
            }
            '}' => {
                tokens.push(Token::CloseBrace);
                at += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                at += 1;
            }
            '.' => {
                tokens.push(Token::Dot);
                at += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                at += 1;
            }
            ';' => {
                tokens.push(Token::Semicolon);
                at += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                at += 1;
            }
            '+' => {
                tokens.push(Token::Plus);
                at += 1;
            }
            _ => return Err(statement_parse_error(ParseDefect::UnexpectedTerm)),
        }
        // Every arm above reads ONE run of text and pushes at most one token for it, so
        // the position of whatever it pushed is the run it read.
        while positions.len() < tokens.len() {
            positions.push(Position {
                line: token_line,
                column: token_column,
                width: at - start,
            });
        }
        // A run the scan has passed may itself hold line breaks — a quoted literal can
        // — so the line the NEXT token sits on is counted off the text that was read.
        for (offset, character) in characters[start..at].iter().enumerate() {
            if *character == '\n' {
                line += 1;
                line_start = start + offset + 1;
            }
        }
    }
    if tokens.is_empty() {
        return Err(statement_parse_error(ParseDefect::NotDataManipulation));
    }
    Ok(Lexemes { tokens, positions })
}

/// Read one quoted run starting at `at`, where `delimiter` is the quote character: a
/// doubled delimiter stands for one of itself, which is how both PartiQL string
/// literals and quoted identifiers escape their quote.
fn quoted(
    characters: &[char],
    at: usize,
    delimiter: char,
) -> Result<(String, usize), RequestError> {
    let mut text = String::new();
    let mut index = at + 1;
    while index < characters.len() {
        let character = characters[index];
        if character == delimiter {
            if characters.get(index + 1) == Some(&delimiter) {
                text.push(delimiter);
                index += 2;
                continue;
            }
            return Ok((text, index + 1));
        }
        text.push(character);
        index += 1;
    }
    // The text stops inside the run: there is no token at the position the parse
    // stopped at, so the refusal names no phrase for it.
    Err(statement_parse_error(ParseDefect::Unterminated))
}

/// Read one numeric lexeme starting at `at`: digits, an optional fraction and an
/// optional exponent. The TEXT is returned; what number it denotes — and whether the
/// store can hold it — is [`canonical_number_text`]'s business.
///
/// A literal the scanner cannot finish reading is no lexeme at all: an exponent marker
/// with no exponent after it (`1e`), or a number run together with a word (`1abc`),
/// stops the scan where it stands rather than splitting into a number and a word the
/// grammar would then refuse — a LEXICAL defect, which carries no phrase
/// ([`ParseDefect::Lexeme`]).
fn number(characters: &[char], at: usize) -> Result<(String, usize), RequestError> {
    let mut index = at;
    while index < characters.len() && characters[index].is_ascii_digit() {
        index += 1;
    }
    let mut fractional = false;
    if characters.get(index) == Some(&'.') {
        fractional = true;
        index += 1;
        while index < characters.len() && characters[index].is_ascii_digit() {
            index += 1;
        }
    }
    // A second decimal point: the run of text is one lexeme the scanner cannot read as a
    // number, rather than a number and a path step after it.
    if fractional && characters.get(index) == Some(&'.') {
        return Err(statement_parse_error(ParseDefect::Lexeme));
    }
    if matches!(characters.get(index), Some('e' | 'E')) {
        let mut lookahead = index + 1;
        if matches!(characters.get(lookahead), Some('+' | '-')) {
            lookahead += 1;
        }
        if !characters.get(lookahead).is_some_and(char::is_ascii_digit) {
            return Err(statement_parse_error(ParseDefect::Lexeme));
        }
        index = lookahead;
        while index < characters.len() && characters[index].is_ascii_digit() {
            index += 1;
        }
    }
    // A word character where the number ends means the two ran together, which is one
    // lexeme the scanner cannot read rather than two tokens.
    if characters.get(index).copied().is_some_and(is_word_char) {
        return Err(statement_parse_error(ParseDefect::Lexeme));
    }
    Ok((characters[at..index].iter().collect(), index))
}

// ===========================================================================
// What one statement says
// ===========================================================================

/// One step of a document path a statement addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PathSegment {
    Name(String),
    Index(usize),
}

/// A document path, as a statement spells it: a top-level attribute and the steps
/// into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementPath {
    /// Never empty, and its first segment is always a name: the parser builds a path
    /// from an attribute name and only then reads the steps into it.
    segments: Vec<PathSegment>,
}

impl StatementPath {
    /// The top-level attribute this path starts at.
    fn attribute(&self) -> &str {
        match self.segments.first() {
            Some(PathSegment::Name(name)) => name,
            _ => "",
        }
    }

    /// Whether this path addresses one whole top-level attribute, which is what a key
    /// equality has to.
    fn is_attribute(&self) -> bool {
        self.segments.len() == 1
    }

    /// The value this path addresses inside one item, or `None` when it addresses
    /// nothing: a step into a value of the wrong type, a name no map holds or an index
    /// past the end of a list all address nothing.
    fn read<'i>(&self, item: &'i Item) -> Option<&'i AttributeValue> {
        let mut segments = self.segments.iter();
        let PathSegment::Name(first) = segments.next()? else {
            return None;
        };
        let mut value = item.get(first)?;
        for segment in segments {
            value = match (segment, value) {
                (PathSegment::Name(name), AttributeValue::M(entries)) => entries.get(name)?,
                (PathSegment::Index(index), AttributeValue::L(members)) => members.get(*index)?,
                _ => return None,
            };
        }
        Some(value)
    }

    /// What a projection of this path is reported under when the statement states no
    /// alias: the PATH as the clause spells it from its last named step on — the name,
    /// and the `[index]` steps into it.
    ///
    /// PartiQL reports a projected path under the path itself, so an element of a list
    /// is reported under `l[1]` rather than under the attribute `l` that holds it: two
    /// elements of one list are two entries of the answer, which one shared key could
    /// not carry.
    fn label(&self) -> String {
        let last_name = self
            .segments
            .iter()
            .rposition(|segment| matches!(segment, PathSegment::Name(_)))
            .unwrap_or(0);
        let mut label = String::new();
        for segment in self.segments.iter().skip(last_name) {
            match segment {
                PathSegment::Name(name) => label.push_str(name),
                PathSegment::Index(index) => label.push_str(&format!("[{index}]")),
            }
        }
        label
    }
}

/// One item of a `SELECT`'s projection: the path it reports and the name it reports
/// it under.
#[derive(Debug, Clone)]
pub struct ProjectionItem {
    path: StatementPath,
    alias: Option<String>,
}

impl ProjectionItem {
    /// The identifier this item is reported under: the alias the statement gave it, and
    /// otherwise the path's own label ([`StatementPath::label`]). The ONE place that is
    /// decided, so what a served statement reports an item under and what a refusal calls
    /// it cannot drift ([`check_distinct_identifiers`], [`project_item`]).
    fn identifier(&self) -> String {
        match self.alias.as_deref() {
            Some(alias) => alias.to_string(),
            None => self.path.label(),
        }
    }
}

/// How a `SELECT` whose projection list reports two of its paths under ONE identifier is
/// refused, naming the identifier(s) the list repeats.
///
/// A projection reports one entry per path it names, keyed by the identifier that path is
/// reported under ([`ProjectionItem::identifier`]) — so a list that states one identifier
/// twice asks for a result in which one key carries two values, which no item can hold:
/// serving it would silently report ONE of them and hand back a tuple narrower than the
/// clause asked for. The statement is refused instead, before anything is read.
///
/// The repeated identifiers are each named once, in the order the clause first repeats
/// them, so a client reading the message sees which names to disambiguate (with `AS`) or
/// drop.
fn check_distinct_identifiers(projection: &[ProjectionItem]) -> Result<(), RequestError> {
    let mut stated: Vec<String> = Vec::new();
    let mut repeated: Vec<String> = Vec::new();
    for item in projection {
        let identifier = item.identifier();
        if stated.contains(&identifier) {
            if !repeated.contains(&identifier) {
                repeated.push(identifier);
            }
        } else {
            stated.push(identifier);
        }
    }
    if repeated.is_empty() {
        return Ok(());
    }
    Err(RequestError::Validation(format!(
        "Duplicate identifiers in select clause: {}",
        repeated.join(", ")
    )))
}

/// One operand of a predicate: a path into the item, a value the statement carries,
/// or the `size` of a path.
#[derive(Debug, Clone)]
pub enum PredicateOperand {
    Path(StatementPath),
    Value(AttributeValue),
    Size(StatementPath),
}

/// One `WHERE` predicate.
///
/// `And` and `Or` hold their operands FLAT, so a long chain widens the tree rather
/// than deepening it — which is also what keeps the rendered expression inside the
/// depth the native parser allows.
#[derive(Debug, Clone)]
pub enum Predicate {
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
    Not(Box<Predicate>),
    Compare(PredicateOperand, CompareOp, PredicateOperand),
    Between(PredicateOperand, PredicateOperand, PredicateOperand),
    In(PredicateOperand, Vec<PredicateOperand>),
    /// `path IS NULL` when true, `path IS NOT NULL` when false. `IS NULL` asks about an
    /// attribute's TYPE, so it holds only for a stored value of the null type: an
    /// attribute the item does not carry is MISSING, which is what [`Self::IsMissing`]
    /// asks about.
    IsNull(StatementPath, bool),
    /// `path IS MISSING` when true, `path IS NOT MISSING` when false.
    IsMissing(StatementPath, bool),
    BeginsWith(StatementPath, PredicateOperand),
    Contains(StatementPath, PredicateOperand),
    AttributeType(StatementPath, PredicateOperand),
    /// `attribute_exists` when true, `attribute_not_exists` when false.
    Exists(StatementPath, bool),
}

/// One clause of an `UPDATE`.
#[derive(Debug, Clone)]
pub enum UpdateClause {
    /// `SET path = value`, where the value may be an operand, a sum or difference of
    /// two, or `list_append`.
    Set(StatementPath, SetExpression),
    /// `REMOVE path`.
    Remove(StatementPath),
    /// `SET path = set_add(path, value)`, the statement form of the `ADD` action.
    Add(StatementPath, AttributeValue),
    /// `SET path = set_delete(path, value)`, the statement form of the `DELETE`
    /// action.
    Delete(StatementPath, AttributeValue),
}

/// The right-hand side of one `SET`.
#[derive(Debug, Clone)]
pub enum SetExpression {
    Single(AssignmentOperand),
    Sum(AssignmentOperand, AssignmentOperand),
    Difference(AssignmentOperand, AssignmentOperand),
}

/// One operand of a `SET`.
#[derive(Debug, Clone)]
pub enum AssignmentOperand {
    Path(StatementPath),
    Value(AttributeValue),
    ListAppend(Box<AssignmentOperand>, Box<AssignmentOperand>),
}

/// What a write reports back, as `RETURNING` names it, in the spelling
/// [`projected_return_values`] reads: the two `MODIFIED` forms are the `UPDATED_*`
/// projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnedValues {
    AllOld,
    ModifiedOld,
    AllNew,
    ModifiedNew,
}

impl ReturnedValues {
    /// The `ReturnValue` spelling of this form.
    pub fn as_return_value(self) -> &'static str {
        match self {
            Self::AllOld => "ALL_OLD",
            Self::ModifiedOld => "UPDATED_OLD",
            Self::AllNew => "ALL_NEW",
            Self::ModifiedNew => "UPDATED_NEW",
        }
    }

    /// Whether this form reports the item as it stands AFTER the write, which a
    /// `DELETE` leaves nothing of.
    pub fn reports_new(self) -> bool {
        matches!(self, Self::AllNew | Self::ModifiedNew)
    }

    /// The `RETURNING` clause that asks for this form, as a refusal about the clause
    /// echoes it.
    fn as_clause(self) -> &'static str {
        match self {
            Self::AllOld => "ALL OLD *",
            Self::ModifiedOld => "MODIFIED OLD *",
            Self::AllNew => "ALL NEW *",
            Self::ModifiedNew => "MODIFIED NEW *",
        }
    }
}

/// The source a statement's `FROM` clause names: the table and the index of it a read
/// is served through.
#[derive(Debug, Clone)]
struct StatementSource {
    table: String,
    index: Option<String>,
}

/// A `SELECT`: which stored items to report, and in which order.
#[derive(Debug, Clone)]
pub struct SelectStatement {
    pub table: String,
    pub index: Option<String>,
    /// The attributes to report, or `None` for `SELECT *`.
    pub projection: Option<Vec<ProjectionItem>>,
    pub filter: Option<Predicate>,
    /// The key attribute `ORDER BY` names and whether it sorts descending.
    pub order_by: Option<(String, bool)>,
}

/// An `INSERT`: the item to add, which must not already be there.
#[derive(Debug, Clone)]
pub struct InsertStatement {
    pub table: String,
    pub item: Item,
}

/// An `UPDATE`: which single item to change, how, and what to report.
#[derive(Debug, Clone)]
pub struct UpdateStatement {
    pub table: String,
    pub clauses: Vec<UpdateClause>,
    pub filter: Predicate,
    pub returning: Option<ReturnedValues>,
}

/// A `DELETE`: which single item to remove, and what to report.
#[derive(Debug, Clone)]
pub struct DeleteStatement {
    pub table: String,
    pub filter: Predicate,
    pub returning: Option<ReturnedValues>,
}

/// One parsed statement.
#[derive(Debug, Clone)]
pub enum PartiqlStatement {
    Select(SelectStatement),
    Insert(InsertStatement),
    Update(UpdateStatement),
    Delete(DeleteStatement),
}

impl PartiqlStatement {
    /// The table this statement addresses.
    pub fn table(&self) -> &str {
        match self {
            Self::Select(select) => &select.table,
            Self::Insert(insert) => &insert.table,
            Self::Update(update) => &update.table,
            Self::Delete(delete) => &delete.table,
        }
    }

    /// The IAM action this statement performs, which is what a table's
    /// resource-based policy is read for: PartiQL has its own four actions.
    pub fn action(&self) -> &'static str {
        match self {
            Self::Select(_) => "dynamodb:PartiQLSelect",
            Self::Insert(_) => "dynamodb:PartiQLInsert",
            Self::Update(_) => "dynamodb:PartiQLUpdate",
            Self::Delete(_) => "dynamodb:PartiQLDelete",
        }
    }
}

// ===========================================================================
// The parser
// ===========================================================================

/// The boolean functions of this dialect: each is a predicate in its own right, read
/// where the grammar admits a predicate.
const BOOLEAN_FUNCTIONS: [&str; 5] = [
    "begins_with",
    "contains",
    "attribute_type",
    "attribute_exists",
    "attribute_not_exists",
];

/// Whether `name` is a function this dialect defines at all: the boolean predicates
/// above, `size` (an operand), and the three that belong to a `SET` clause.
///
/// What it decides is whether a call the grammar does not admit where it stands is a
/// call to something UNDEFINED — which AWS names — or one of the dialect's own standing
/// where only a name may, which is a name the statement does not spell.
fn is_defined_function(name: &str) -> bool {
    BOOLEAN_FUNCTIONS
        .iter()
        .chain(["size", "list_append", "set_add", "set_delete"].iter())
        .any(|defined| name.eq_ignore_ascii_case(defined))
}

/// The KEYWORDS of the statement language: the words a bare identifier may not be.
///
/// PartiQL's keywords are the clause words its grammar is made of and the type names
/// its literals can be cast to; DynamoDB's dialect is that grammar, so a statement that
/// writes one of them out plainly where a name belongs has written the keyword, not the
/// name — "you must use `"double quotes"` around a reserved word" is how a name that
/// collides with one is spelled. Compared without regard to case, the way every other
/// keyword of this dialect is ([`StatementParser::peek_word`]).
const STATEMENT_KEYWORDS: [&str; 104] = [
    "all", "and", "any", "as", "asc", "at", "bag", "between", "bigint", "blob", "bool", "boolean",
    "both", "by", "case", "cast", "char", "character", "clob", "cross", "date", "decimal",
    "delete", "desc", "distinct", "double", "else", "end", "escape", "except", "exclude", "exists",
    "extract", "false", "first", "float", "for", "from", "full", "group", "having", "in", "inner",
    "insert", "int", "int2", "int4", "int8", "integer", "intersect", "into", "is", "join", "last",
    "lateral", "left", "like", "limit", "list", "missing", "modified", "natural", "new", "not",
    "null", "nulls", "numeric", "offset", "old", "on", "or", "order", "outer", "pivot", "real",
    "remove", "replace", "returning", "right", "select", "set", "sexp", "smallint", "string",
    "struct", "symbol", "table", "then", "time", "timestamp", "tinyint", "to", "true", "union",
    "unpivot", "update", "upsert", "using", "value", "values", "varchar", "when", "where", "with",
];

/// Whether a bare word spells one of the dialect's keywords.
fn is_statement_keyword(word: &str) -> bool {
    STATEMENT_KEYWORDS
        .iter()
        .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

/// Parse one statement, resolving each `?` against `parameters` in order.
///
/// A `Parameters` entry is a value the statement carries, so each is held to the same
/// documented value rules a written literal is ([`check_statement_value`]): a number
/// outside the stored numeric domain, or a set with an empty or repeated member, is
/// refused here rather than bound into a predicate that would then select nothing.
pub fn parse_statement(
    statement: &str,
    parameters: &[AttributeValue],
) -> Result<PartiqlStatement, RequestError> {
    let mut bound = parameters.to_vec();
    for (position, value) in bound.iter_mut().enumerate() {
        // The rules, stated the way the statement surface states a refusal about a value
        // it carries ([`check_statement_value`]) — one choke point for every surface
        // that runs a statement, so `BatchExecuteStatement` and `ExecuteTransaction`
        // hold a parameter to exactly what `ExecuteStatement` holds it to.
        check_statement_value(&format!("Parameters[{position}]"), value)?;
        // The canonical text is what the markers bind, so one number has one spelling
        // however the request wrote it. Nothing here can fail: the rules above have
        // already refused every value outside the stored domain.
        canonicalize_attribute_value(value).map_err(unenveloped)?;
    }
    let lexemes = tokenize(statement)?;
    let mut parser = StatementParser {
        tokens: &lexemes.tokens,
        positions: &lexemes.positions,
        at: 0,
        parameters: &bound,
        used_parameters: 0,
        depth: 0,
        aliased: false,
    };
    let parsed = parser.statement()?;
    // "Only one query is allowed": a `;` ends the query, so text after it is a second
    // one — or a token belonging to nothing — and is refused as such rather than as a
    // token after the expression.
    if parser.accept(&Token::Semicolon) && !parser.at_end() {
        return Err(statement_parse_error(ParseDefect::AfterSemicolon));
    }
    if !parser.at_end() {
        return Err(unexpected_token_after_statement());
    }
    // A statement that READ as a whole statement and bound a name to its table is one
    // this surface does not serve: the binding is refused after the text has parsed, so
    // text that never parsed still reports its own parse defect.
    if parser.aliased {
        return Err(aliasing_unsupported());
    }
    // "The parameters for the PartiQL statement, if any" (`Parameters`): a request
    // that sends more values than the statement has markers for has sent a statement
    // and a parameter list that do not go together.
    if parser.used_parameters != bound.len() {
        return Err(RequestError::Validation(
            PARAMETER_COUNT_MESSAGE.to_string(),
        ));
    }
    Ok(parsed)
}

/// The recursive-descent parser over one statement's tokens.
#[derive(Debug)]
struct StatementParser<'t> {
    tokens: &'t [Token],
    /// Where the statement spells each token, at the same index as the token itself.
    positions: &'t [Position],
    at: usize,
    parameters: &'t [AttributeValue],
    used_parameters: usize,
    depth: usize,
    /// Whether the statement bound an alias to the table it names. The binding is part
    /// of the grammar, so it is READ here and refused once the whole statement has
    /// parsed ([`aliasing_unsupported`]).
    aliased: bool,
}

impl StatementParser<'_> {
    fn at_end(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// Where the statement spells the token the parser stands at, or — past the last
    /// token — the empty position just after the text, so a refusal about the end of a
    /// statement still reports a position of the request's own text.
    fn position(&self) -> Position {
        if let Some(position) = self.positions.get(self.at) {
            return *position;
        }
        self.positions.last().map_or(
            Position {
                line: 1,
                column: 1,
                width: 0,
            },
            |last| Position {
                line: last.line,
                column: last.column + last.width,
                width: 0,
            },
        )
    }

    fn bump(&mut self) -> Result<&Token, RequestError> {
        let token = self
            .tokens
            .get(self.at)
            .ok_or_else(|| statement_parse_error(ParseDefect::UnexpectedTerm))?;
        self.at += 1;
        Ok(token)
    }

    fn accept(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, token: &Token) -> Result<(), RequestError> {
        if self.accept(token) {
            return Ok(());
        }
        Err(unexpected_token(self.peek()))
    }

    /// Whether the next token is this keyword, compared case-insensitively — "SELECT"
    /// and "select" are one keyword — and never a `"quoted"` identifier, which is how
    /// a name that collides with a keyword is spelled.
    fn peek_word(&self, keyword: &str) -> bool {
        self.peek_word_at(0, keyword)
    }

    /// The same question asked of the token `offset` tokens on, so a decision the grammar
    /// takes on a PAIR of keywords — the verb and the kind of object a schema statement
    /// names ([`NON_DML_STATEMENT_OPERATIONS`]) — reads both without consuming either.
    fn peek_word_at(&self, offset: usize, keyword: &str) -> bool {
        matches!(
            self.tokens.get(self.at + offset),
            Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword)
        )
    }

    fn accept_word(&mut self, keyword: &str) -> bool {
        if self.peek_word(keyword) {
            self.at += 1;
            return true;
        }
        false
    }

    fn expect_word(&mut self, keyword: &str) -> Result<(), RequestError> {
        if self.accept_word(keyword) {
            return Ok(());
        }
        Err(statement_parse_error(ParseDefect::Keyword))
    }

    /// One keyword of the statement's own CLAUSE STRUCTURE — an `INSERT`'s `INTO` and
    /// `VALUE`, a `DELETE`'s `FROM`, the `BY` of an `ORDER BY`.
    ///
    /// Text that opens with a data-manipulation verb and then does not read as that
    /// verb's statement manipulates no data: the clause the grammar required is not
    /// there, so there is no statement, and the defect reported is the one text that
    /// states no manipulation of data earns
    /// ([`ParseDefect::NotDataManipulation`]) rather than a phrase about the keyword
    /// position inside a clause that parsed.
    fn expect_clause_word(&mut self, keyword: &str) -> Result<(), RequestError> {
        if self.accept_word(keyword) {
            return Ok(());
        }
        Err(statement_parse_error(ParseDefect::NotDataManipulation))
    }

    /// One identifier: a bare word or a quoted name.
    ///
    /// A `"double-quoted"` name is a NAME whatever it spells; a bare word is read as the
    /// text it is, so one that spells a keyword of the dialect
    /// ([`is_statement_keyword`]) is the keyword rather than the name a clause needs
    /// there, and the statement is refused as one ([`ParseDefect::UnexpectedKeyword`]) —
    /// which is what makes quoting the way a name that collides with a keyword is
    /// spelled.
    fn identifier(&mut self) -> Result<String, RequestError> {
        match self.bump()? {
            Token::Word(word) if is_statement_keyword(word) => {
                Err(statement_parse_error(ParseDefect::UnexpectedKeyword))
            }
            Token::Word(word) => Ok(word.clone()),
            Token::Quoted(name) => Ok(name.clone()),
            _ => Err(statement_parse_error(ParseDefect::Identifier)),
        }
    }

    /// One call operand that has to be a document path: the functions that read an
    /// attribute of the item take a PATH, and AWS names the function when it is given
    /// anything else.
    ///
    /// A call that states NO operand at all is a call with the wrong NUMBER of them, not
    /// one given something that is no path: the count is read off the call before its
    /// operands are, so a client is told what the call carried.
    fn function_path(&mut self, function: &str) -> Result<StatementPath, RequestError> {
        if self.peek() == Some(&Token::Close) {
            return Err(incorrect_operand_count(function, 0));
        }
        if !self.starts_path() {
            return Err(requires_document_path(function));
        }
        self.path()
    }

    /// The `,` between two operands of one call: a call that states no further operand
    /// has the wrong number of them, counted off the call's own text.
    fn expect_operand(&mut self, function: &str, read: usize) -> Result<(), RequestError> {
        if self.accept(&Token::Comma) {
            return Ok(());
        }
        Err(incorrect_operand_count(function, self.operands_in_call(read)))
    }

    /// The `)` that closes one call: text where the close belongs is another operand,
    /// so the call carries more of them than the function takes.
    fn expect_call_close(&mut self, function: &str, read: usize) -> Result<(), RequestError> {
        if self.accept(&Token::Close) {
            return Ok(());
        }
        Err(incorrect_operand_count(function, self.operands_in_call(read)))
    }

    /// How many operands the call being read carries: the `read` already parsed plus one
    /// per `,` left in it at the call's own bracket depth.
    ///
    /// Counted off the TOKENS rather than by parsing them: the count belongs in the
    /// refusal, and a call this parser has already rejected is not read any further.
    fn operands_in_call(&self, read: usize) -> usize {
        let mut count = read;
        let mut depth = 0usize;
        for token in self.tokens.iter().skip(self.at) {
            match token {
                Token::Open | Token::OpenBracket | Token::OpenBrace | Token::OpenBag => {
                    depth += 1;
                }
                Token::Close if depth == 0 => break,
                Token::Close | Token::CloseBracket | Token::CloseBrace | Token::CloseBag => {
                    depth = depth.saturating_sub(1);
                }
                Token::Comma if depth == 0 => count += 1,
                _ => {}
            }
        }
        count
    }

    /// Run `body` one level deeper, refusing a statement whose GRAMMAR nests past
    /// [`MAX_STATEMENT_DEPTH`] — a parenthesised predicate, a `NOT`, a `list_append`.
    fn nested<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, RequestError>,
    ) -> Result<T, RequestError> {
        self.nested_to(MAX_STATEMENT_DEPTH, body)
    }

    /// Run `body` one level deeper, against the ceiling that governs what is being
    /// read: the grammar's own ([`MAX_STATEMENT_DEPTH`]) or, for the VALUES a
    /// statement carries, the documented [`MAX_ATTRIBUTE_DEPTH`] every other write
    /// path holds an attribute value to — so an item the classic API stores is an
    /// item a statement can write, and the two surfaces never disagree about one.
    fn nested_to<T>(
        &mut self,
        limit: usize,
        body: impl FnOnce(&mut Self) -> Result<T, RequestError>,
    ) -> Result<T, RequestError> {
        if self.depth >= limit {
            return Err(statement_refused(&format!(
                "a statement may nest at most {limit} levels deep"
            )));
        }
        self.depth += 1;
        let outcome = body(self);
        self.depth -= 1;
        outcome
    }

    /// The statement, by the verb it opens with.
    fn statement(&mut self) -> Result<PartiqlStatement, RequestError> {
        if self.accept_word("select") {
            return self.select().map(PartiqlStatement::Select);
        }
        if self.accept_word("insert") {
            return self.insert().map(PartiqlStatement::Insert);
        }
        if self.accept_word("update") {
            return self.update().map(PartiqlStatement::Update);
        }
        if self.accept_word("delete") {
            return self.delete().map(PartiqlStatement::Delete);
        }
        // Text whose leading keywords name one of the language's OTHER operations names
        // an operation this surface does not run, which is what it is told — before the
        // parse carries on into the clauses of a statement that would never run.
        if let Some(operation) = self.non_dml_operation() {
            return Err(unsupported_statement_operation(operation));
        }
        // Text that opens with none of the four verbs this surface serves, and names no
        // operation of the language either, is read as the language's OTHER statement form:
        // one query, which is one expression. What the refusal states is the position the
        // parse stopped at ([`Self::query_defect`]) — the token that cannot follow the
        // complete expression the text opens with, or, where the whole text reads as one
        // expression, that it manipulates no data whatever it spells.
        let defect = self.query_defect();
        Err(statement_parse_error(defect))
    }

    /// The defect text that states none of the data-manipulation verbs carries, read off
    /// WHERE the parse of it stopped.
    ///
    /// The language's other statement form is a QUERY — one expression — so text that opens
    /// with no verb this surface serves is read as one, with the expression grammar this
    /// dialect already has: the predicate form ([`Self::disjunction`] — a comparison, a
    /// membership, a boolean call) and, for text that states no operator at all, the bare
    /// operand form ([`Self::operand`] — a path, a literal, a collection), each of which is
    /// a complete expression. Then:
    ///
    /// * text that READ as one expression with a token left over stopped at that token,
    ///   which nothing in the dialect can follow an expression with
    ///   ([`ParseDefect::TrailingToken`]) — a misspelled verb (`INSRT INTO …`) is this: the
    ///   misspelling is a name, a complete expression, and the clause word after it belongs
    ///   to no expression;
    /// * text that IS one expression, with nothing after it, parsed as a query and
    ///   manipulates no data whatever it spells
    ///   ([`ParseDefect::NotDataManipulation`]) — a bare name, a literal, a predicate;
    /// * text that reads as no expression at all states no manipulation of data either,
    ///   which is the defect it keeps.
    ///
    /// The walk is a LOOK: the statement is refused whatever it finds, so where the cursor
    /// is left does not matter, and no further parse reads from it.
    fn query_defect(&mut self) -> ParseDefect {
        let opened_at = self.at;
        let mut read = self.disjunction().is_ok();
        if !read {
            self.at = opened_at;
            read = self.operand().is_ok();
        }
        if !read {
            return ParseDefect::NotDataManipulation;
        }
        // The `;` that ends a query is part of no expression and is not text carrying on
        // past one either, so a query terminated by one is still one complete expression.
        if self.at_end() || self.peek() == Some(&Token::Semicolon) {
            ParseDefect::NotDataManipulation
        } else {
            ParseDefect::TrailingToken
        }
    }

    /// The operation the text's leading keywords name, when they name one of the
    /// language's operations this surface does not run
    /// ([`NON_DML_STATEMENT_OPERATIONS`]).
    fn non_dml_operation(&self) -> Option<&'static str> {
        NON_DML_STATEMENT_OPERATIONS
            .iter()
            .find(|(verb, object, _)| self.peek_word(verb) && self.peek_word_at(1, object))
            .map(|(_, _, operation)| *operation)
    }

    /// `SELECT (* | attribute, …) FROM source [WHERE predicate] [ORDER BY attribute
    /// [ASC | DESC]]`.
    fn select(&mut self) -> Result<SelectStatement, RequestError> {
        let projection = if self.accept(&Token::Star) {
            None
        } else {
            let mut items = Vec::new();
            loop {
                let path = self.projection_path()?;
                let alias = if self.accept_word("as") {
                    Some(self.identifier()?)
                } else {
                    None
                };
                items.push(ProjectionItem { path, alias });
                if !self.accept(&Token::Comma) {
                    break;
                }
            }
            check_distinct_identifiers(&items)?;
            Some(items)
        };
        // The `FROM` that follows a projection list is the one keyword position the
        // parser names in its phrase, rather than reporting a keyword generically.
        if !self.accept_word("from") {
            return Err(statement_parse_error(ParseDefect::FromAfterSelectList));
        }
        // A `SELECT` carries on with its own `WHERE` or `ORDER BY`; any other name here
        // binds the source ([`Self::read_table_alias`]).
        let source = self.read_source(&["where", "order"])?;
        let (table, index) = (source.table, source.index);
        let filter = self.where_clause()?;
        let order_by = if self.accept_word("order") {
            self.expect_clause_word("by")?;
            let attribute = self.identifier()?;
            let descending = if self.accept_word("desc") {
                true
            } else {
                self.accept_word("asc");
                false
            };
            if self.peek() == Some(&Token::Comma) {
                return Err(statement_refused(
                    "ORDER BY may name one key attribute of the source",
                ));
            }
            Some((attribute, descending))
        } else {
            None
        };
        Ok(SelectStatement {
            table,
            index,
            projection,
            filter,
            order_by,
        })
    }

    /// `INSERT INTO source VALUE tuple`.
    fn insert(&mut self) -> Result<InsertStatement, RequestError> {
        self.expect_clause_word("into")?;
        let table = self.write_target(&["value", "values"], single_table_in_data_manipulation)?;
        if !self.accept_word("value") && !self.accept_word("values") {
            // What follows an `INSERT`'s target is the VALUE it inserts, so the position
            // admits a TERM: text that states none — the statement stopping there, or a
            // token the grammar has no place for — is refused as that term rather than as
            // text stating no manipulation of data, which is what an `UPDATE` with
            // neither action earns ([`ParseDefect::NotDataManipulation`]).
            return Err(unexpected_token(self.peek()));
        }
        let value = self.value()?;
        let AttributeValue::M(item) = value else {
            return Err(statement_refused(
                "the VALUE an INSERT carries must be a tuple of attribute names and values",
            ));
        };
        Ok(InsertStatement { table, item })
    }

    /// `UPDATE source (SET assignment | REMOVE path)+ WHERE predicate
    /// [RETURNING values]`.
    ///
    /// Each clause states ONE action, and several actions are stated by repeating the
    /// keyword — "UPDATE "Music" SET AwardsWon=1 SET AwardDetail={…} WHERE …" is how the
    /// dialect spells two of them. A comma after a complete assignment or path is
    /// therefore a token the grammar has no place for, and the statement is refused as
    /// one token past its expression rather than read as a list.
    fn update(&mut self) -> Result<UpdateStatement, RequestError> {
        let table = self.write_target(&["set", "remove"], |_| unsupported_on_an_index())?;
        let mut clauses = Vec::new();
        loop {
            if self.accept_word("set") {
                clauses.push(self.assignment()?);
                self.refuse_clause_list()?;
                continue;
            }
            if self.accept_word("remove") {
                clauses.push(UpdateClause::Remove(self.path()?));
                self.refuse_clause_list()?;
                continue;
            }
            break;
        }
        if clauses.is_empty() {
            // An `UPDATE` that states no action states no manipulation of data: the
            // verb opens the statement but nothing in it changes an item, so the text
            // is refused as the parse defect it is rather than with a rule of its own.
            return Err(statement_parse_error(ParseDefect::NotDataManipulation));
        }
        let filter = self.required_where_clause()?;
        let returning = self.returning()?;
        Ok(UpdateStatement {
            table,
            clauses,
            filter,
            returning,
        })
    }

    /// `DELETE FROM source WHERE predicate [RETURNING values]`.
    fn delete(&mut self) -> Result<DeleteStatement, RequestError> {
        self.expect_clause_word("from")?;
        let source = self.read_source(&["where", "returning"])?;
        let table = source.table;
        if source.index.is_some() {
            // A `DELETE` removes an item of the TABLE, and an index holds a projection
            // of one rather than items of its own, so a qualified source asks for a
            // delete through an index: the fixed index sentence, the same one the
            // classic surface states for an operation asked of an index.
            return Err(unsupported_on_an_index());
        }
        let filter = self.required_where_clause()?;
        let returning = self.returning()?;
        if let Some(values) = returning.filter(|values| values.reports_new()) {
            return Err(invalid_returning_clause(values.as_clause()));
        }
        Ok(DeleteStatement {
            table,
            filter,
            returning,
        })
    }

    /// Refuse a comma where one clause of an `UPDATE` has just ended: the clause states
    /// one action, so the comma is a token past a complete expression.
    fn refuse_clause_list(&mut self) -> Result<(), RequestError> {
        if self.peek() == Some(&Token::Comma) {
            return Err(unexpected_token_after_statement());
        }
        Ok(())
    }

    /// The source a `FROM` clause names — a `SELECT`'s and a `DELETE`'s: `table` or
    /// `table.index`, either part quoted or bare.
    fn read_source(&mut self, continues: &[&str]) -> Result<StatementSource, RequestError> {
        let table = self.path_component()?;
        let index = if self.accept(&Token::Dot) {
            Some(self.path_component()?)
        } else {
            None
        };
        // A source path names a table and, for a read, one index of it: a further
        // component names nothing this store holds.
        if self.peek() == Some(&Token::Dot) {
            return Err(RequestError::Validation(
                FROM_PATH_ARITY_MESSAGE.to_string(),
            ));
        }
        let source = StatementSource { table, index };
        self.read_table_alias(continues);
        Ok(source)
    }

    /// One component of a source path: a name, never the empty one
    /// ([`empty_path_component`]).
    fn path_component(&mut self) -> Result<String, RequestError> {
        let component = self.identifier()?;
        if component.is_empty() {
            return Err(empty_path_component());
        }
        Ok(component)
    }

    /// The target a write names DIRECTLY — an `INSERT`'s `INTO` and an `UPDATE`'s own
    /// source: one table, since a data-manipulation statement addresses the table rather
    /// than an index of it.
    ///
    /// `qualified` is how a qualifier on that target is refused, which depends on how the
    /// verb spells the target: inside a CLAUSE (an `INSERT`'s `INTO`) the refusal is
    /// about the clause and reports where the path sits
    /// ([`single_table_in_data_manipulation`]); with no clause around it (an `UPDATE`)
    /// it is the fixed index sentence ([`unsupported_on_an_index`]).
    fn write_target(
        &mut self,
        continues: &[&str],
        qualified: impl FnOnce(Position) -> RequestError,
    ) -> Result<String, RequestError> {
        let opens_at = self.position();
        let table = self.path_component()?;
        if self.peek() == Some(&Token::Dot) {
            // The whole path is read, qualifier included, so a statement whose target
            // carries one is refused for the target rather than for the token after it;
            // the position reported is where the statement spells the TABLE.
            self.at += 1;
            let _qualifier = self.path_component()?;
            while self.accept(&Token::Dot) {
                let _further = self.path_component()?;
            }
            return Err(qualified(opens_at));
        }
        self.read_table_alias(continues);
        Ok(table)
    }

    /// Read the ALIAS a statement binds to the table it has just named, when it states
    /// one, and remember that it did.
    ///
    /// A binding is `AS name`, or the same name with the keyword left out; `continues`
    /// names the keywords the statement itself carries on with at this position, so a
    /// word that is none of them — and a `"quoted"` name, which is never a keyword —
    /// binds the table rather than continuing the statement. The binding is part of the
    /// grammar and is therefore READ rather than refused here: a statement that does not
    /// parse past it earns its own parse defect, and one that parses whole is refused for
    /// the binding ([`aliasing_unsupported`], applied once the statement is read).
    fn read_table_alias(&mut self, continues: &[&str]) {
        if self.accept_word("as") {
            self.aliased = true;
            // The name the binding states, when it states one: text that is no name
            // leaves the statement to fail on its own defect.
            if matches!(self.peek(), Some(Token::Word(_) | Token::Quoted(_))) {
                self.at += 1;
            }
            return;
        }
        let binds = match self.peek() {
            Some(Token::Word(word)) => !continues
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword)),
            Some(Token::Quoted(_)) => true,
            _ => false,
        };
        if binds {
            self.aliased = true;
            self.at += 1;
        }
    }

    /// `RETURNING (ALL | MODIFIED) (OLD | NEW) *`.
    fn returning(&mut self) -> Result<Option<ReturnedValues>, RequestError> {
        if !self.accept_word("returning") {
            return Ok(None);
        }
        let modified = if self.accept_word("all") {
            false
        } else {
            self.expect_word("modified")?;
            true
        };
        let new = if self.accept_word("old") {
            false
        } else {
            self.expect_word("new")?;
            true
        };
        self.expect(&Token::Star)?;
        Ok(Some(match (modified, new) {
            (false, false) => ReturnedValues::AllOld,
            (true, false) => ReturnedValues::ModifiedOld,
            (false, true) => ReturnedValues::AllNew,
            (true, true) => ReturnedValues::ModifiedNew,
        }))
    }

    /// `SET path = expression`, where the expression is one operand, two joined by
    /// `+` or `-`, or one of the documented functions.
    fn assignment(&mut self) -> Result<UpdateClause, RequestError> {
        let path = self.path()?;
        self.expect(&Token::Compare(CompareOp::Equal))?;
        // `set_add` and `set_delete` are the statement forms of the ADD and DELETE
        // update actions, so they are whole clauses rather than values.
        for (function, is_add) in [("set_add", true), ("set_delete", false)] {
            if self.peek_word(function) && self.tokens.get(self.at + 1) == Some(&Token::Open) {
                self.at += 2;
                let target = self.function_path(function)?;
                if target != path {
                    return Err(statement_refused(&format!(
                        "{function} adds to the attribute it is assigned to"
                    )));
                }
                self.expect_operand(function, 1)?;
                let value = self.value()?;
                self.expect_call_close(function, 2)?;
                return Ok(if is_add {
                    UpdateClause::Add(path, value)
                } else {
                    UpdateClause::Delete(path, value)
                });
            }
        }
        let left = self.set_operand()?;
        if self.accept(&Token::Plus) {
            let right = self.set_operand()?;
            return Ok(UpdateClause::Set(path, SetExpression::Sum(left, right)));
        }
        if self.accept(&Token::Minus) {
            let right = self.set_operand()?;
            return Ok(UpdateClause::Set(
                path,
                SetExpression::Difference(left, right),
            ));
        }
        Ok(UpdateClause::Set(path, SetExpression::Single(left)))
    }

    /// One operand of a `SET`: `list_append`, a path, or a value.
    fn set_operand(&mut self) -> Result<AssignmentOperand, RequestError> {
        if self.peek_word("list_append") && self.tokens.get(self.at + 1) == Some(&Token::Open) {
            self.at += 2;
            let (left, right) = self.nested(|parser| {
                let left = parser.set_operand()?;
                parser.expect_operand("list_append", 1)?;
                let right = parser.set_operand()?;
                Ok((left, right))
            })?;
            self.expect_call_close("list_append", 2)?;
            return Ok(AssignmentOperand::ListAppend(Box::new(left), Box::new(right)));
        }
        if self.starts_path() {
            return Ok(AssignmentOperand::Path(self.path()?));
        }
        Ok(AssignmentOperand::Value(self.value()?))
    }

    /// `WHERE predicate`, when the statement states one.
    fn where_clause(&mut self) -> Result<Option<Predicate>, RequestError> {
        if !self.accept_word("where") {
            return Ok(None);
        }
        self.disjunction().map(Some)
    }

    /// The `WHERE` an `UPDATE` or a `DELETE` must state: both are singleton writes,
    /// addressed by the primary key the predicate pins, so a statement that states no
    /// clause at all pins no key either.
    fn required_where_clause(&mut self) -> Result<Predicate, RequestError> {
        self.where_clause()?
            .ok_or_else(|| RequestError::Validation(MANDATORY_KEY_EQUALITY_MESSAGE.to_string()))
    }

    fn disjunction(&mut self) -> Result<Predicate, RequestError> {
        let mut branches = vec![self.conjunction()?];
        while self.accept_word("or") {
            branches.push(self.conjunction()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Predicate::Any(branches)
        })
    }

    fn conjunction(&mut self) -> Result<Predicate, RequestError> {
        let mut branches = vec![self.negation()?];
        while self.accept_word("and") {
            branches.push(self.negation()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Predicate::All(branches)
        })
    }

    fn negation(&mut self) -> Result<Predicate, RequestError> {
        if self.accept_word("not") {
            let inner = self.nested(Self::negation)?;
            return Ok(Predicate::Not(Box::new(inner)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Predicate, RequestError> {
        if self.accept(&Token::Open) {
            let inner = self.nested(Self::disjunction)?;
            self.expect(&Token::Close)?;
            return Ok(inner);
        }
        // The documented boolean functions are predicates in their own right.
        if self.tokens.get(self.at + 1) == Some(&Token::Open) {
            for function in BOOLEAN_FUNCTIONS {
                if self.peek_word(function) {
                    self.at += 2;
                    let path = self.function_path(function)?;
                    // How many operands the function takes, which is what the count in a
                    // refusal is read against.
                    let arity = if matches!(
                        function,
                        "attribute_exists" | "attribute_not_exists"
                    ) {
                        1
                    } else {
                        2
                    };
                    let predicate = match function {
                        "attribute_exists" => Predicate::Exists(path, true),
                        "attribute_not_exists" => Predicate::Exists(path, false),
                        _ => {
                            self.expect_operand(function, 1)?;
                            let operand = self.operand()?;
                            match function {
                                "begins_with" => Predicate::BeginsWith(path, operand),
                                "contains" => Predicate::Contains(path, operand),
                                _ => {
                                    check_statement_type_name(&operand)?;
                                    Predicate::AttributeType(path, operand)
                                }
                            }
                        }
                    };
                    self.expect_call_close(function, arity)?;
                    return Ok(predicate);
                }
            }
        }
        let left = self.operand()?;
        if self.accept_word("is") {
            let negated = self.accept_word("not");
            let PredicateOperand::Path(path) = left else {
                return Err(statement_refused(
                    "IS NULL and IS MISSING read an attribute of the item",
                ));
            };
            if self.accept_word("null") {
                return Ok(Predicate::IsNull(path, !negated));
            }
            self.expect_word("missing")?;
            return Ok(Predicate::IsMissing(path, !negated));
        }
        let negated = self.accept_word("not");
        if self.accept_word("between") {
            let low = self.operand()?;
            self.expect_word("and")?;
            let high = self.operand()?;
            check_scalar_operands("BETWEEN", [&left, &low, &high])?;
            let between = Predicate::Between(left, low, high);
            return Ok(if negated {
                Predicate::Not(Box::new(between))
            } else {
                between
            });
        }
        if self.accept_word("in") {
            let closing = if self.accept(&Token::Open) {
                Token::Close
            } else {
                self.expect(&Token::OpenBracket)?;
                Token::CloseBracket
            };
            let mut candidates = Vec::new();
            loop {
                candidates.push(self.operand()?);
                if !self.accept(&Token::Comma) {
                    break;
                }
            }
            self.expect(&closing)?;
            let membership = Predicate::In(left, candidates);
            return Ok(if negated {
                Predicate::Not(Box::new(membership))
            } else {
                membership
            });
        }
        if negated {
            return Err(unexpected_token(self.peek()));
        }
        let Some(Token::Compare(operator)) = self.peek().cloned() else {
            return Err(unexpected_token(self.peek()));
        };
        self.at += 1;
        let right = self.operand()?;
        check_relational_operands(operator, &left, &right)?;
        Ok(Predicate::Compare(left, operator, right))
    }

    /// One comparison operand: `size(path)`, a path, or a value.
    fn operand(&mut self) -> Result<PredicateOperand, RequestError> {
        if self.peek_word("size") && self.tokens.get(self.at + 1) == Some(&Token::Open) {
            self.at += 2;
            let path = self.function_path("size")?;
            self.expect_call_close("size", 1)?;
            return Ok(PredicateOperand::Size(path));
        }
        if self.starts_path() {
            return Ok(PredicateOperand::Path(self.path()?));
        }
        Ok(PredicateOperand::Value(self.value()?))
    }

    /// Whether the next token opens a document path rather than a value: a quoted
    /// identifier always does, and a bare word does unless it spells one of the
    /// keyword literals.
    fn starts_path(&self) -> bool {
        match self.peek() {
            Some(Token::Quoted(_)) => true,
            Some(Token::Word(word)) => !["null", "missing", "true", "false"]
                .iter()
                .any(|literal| word.eq_ignore_ascii_case(literal)),
            _ => false,
        }
    }

    /// One path of a `SELECT`'s projection list.
    ///
    /// A projection names paths into the item and nothing else, so a CALL standing there
    /// is refused as the path component it is not
    /// ([`unexpected_path_component`]) — whether or not the dialect defines the function
    /// called, because what the parse stopped on is the component rather than the call.
    fn projection_path(&mut self) -> Result<StatementPath, RequestError> {
        if self.tokens.get(self.at + 1) == Some(&Token::Open)
            && matches!(self.peek(), Some(Token::Word(_)))
        {
            return Err(unexpected_path_component(self.position()));
        }
        self.path()
    }

    /// One document path: an attribute name, then the `.name` and `[index]` steps
    /// into it.
    fn path(&mut self) -> Result<StatementPath, RequestError> {
        // A name the statement CALLS is a function, not a path. Every function this
        // dialect defines is read where the grammar admits it — before a path is read —
        // so a call that reaches here is either a function the dialect does not define,
        // or one of its own standing where only a name may stand, which is no name.
        if self.tokens.get(self.at + 1) == Some(&Token::Open) {
            if let Some(Token::Word(name)) = self.peek() {
                if is_defined_function(name) {
                    return Err(statement_parse_error(ParseDefect::Identifier));
                }
                let name = name.clone();
                return Err(unrecognized_function(&name, self.position()));
            }
        }
        let mut segments = vec![PathSegment::Name(self.identifier()?)];
        loop {
            if segments.len() > MAX_STATEMENT_DEPTH {
                return Err(statement_refused(&format!(
                    "a document path may hold at most {MAX_STATEMENT_DEPTH} steps"
                )));
            }
            if self.accept(&Token::Dot) {
                segments.push(PathSegment::Name(self.identifier()?));
                continue;
            }
            if self.accept(&Token::OpenBracket) {
                let Token::Number(lexeme) = self.bump()?.clone() else {
                    return Err(statement_parse_error(ParseDefect::UnexpectedTerm));
                };
                let index = lexeme
                    .parse::<usize>()
                    .map_err(|_| statement_parse_error(ParseDefect::UnexpectedTerm))?;
                self.expect(&Token::CloseBracket)?;
                segments.push(PathSegment::Index(index));
                continue;
            }
            break;
        }
        Ok(StatementPath { segments })
    }

    /// One value the statement carries: a literal, a collection of literals, or a
    /// `?` marker standing for one `Parameters` entry.
    fn value(&mut self) -> Result<AttributeValue, RequestError> {
        let value = self.raw_value()?;
        // A value a statement carries is held to the same rules a stored one is: the
        // documented nesting depth and this emulator's ceiling on a collection's
        // members.
        check_statement_value("Statement", &value)?;
        Ok(value)
    }

    fn raw_value(&mut self) -> Result<AttributeValue, RequestError> {
        match self.peek().cloned() {
            Some(Token::Text(text)) => {
                self.at += 1;
                Ok(AttributeValue::S(text))
            }
            Some(Token::Number(lexeme)) => {
                self.at += 1;
                Ok(AttributeValue::N(statement_number(&lexeme)?))
            }
            Some(Token::Minus | Token::Plus) => {
                let negative = self.peek() == Some(&Token::Minus);
                self.at += 1;
                let Some(Token::Number(lexeme)) = self.peek().cloned() else {
                    return Err(statement_parse_error(ParseDefect::UnexpectedTerm));
                };
                self.at += 1;
                let signed = if negative {
                    format!("-{lexeme}")
                } else {
                    lexeme
                };
                Ok(AttributeValue::N(statement_number(&signed)?))
            }
            Some(Token::Parameter) => {
                self.at += 1;
                let value = self
                    .parameters
                    .get(self.used_parameters)
                    .cloned()
                    .ok_or_else(|| {
                        RequestError::Validation(PARAMETER_COUNT_MESSAGE.to_string())
                    })?;
                self.used_parameters += 1;
                Ok(value)
            }
            Some(Token::OpenBracket) => {
                self.at += 1;
                let members = self.nested_to(MAX_ATTRIBUTE_DEPTH, |parser| {
                    parser.value_list(&Token::CloseBracket)
                })?;
                Ok(AttributeValue::L(members))
            }
            Some(Token::OpenBag) => {
                self.at += 1;
                let members = self
                    .nested_to(MAX_ATTRIBUTE_DEPTH, |parser| {
                        parser.value_list(&Token::CloseBag)
                    })?;
                set_of(members)
            }
            Some(Token::OpenBrace) => {
                self.at += 1;
                let entries = self.nested_to(MAX_ATTRIBUTE_DEPTH, Self::tuple_entries)?;
                Ok(AttributeValue::M(entries))
            }
            Some(Token::Word(word)) => {
                if word.eq_ignore_ascii_case("null") {
                    self.at += 1;
                    return Ok(AttributeValue::Null(true));
                }
                if word.eq_ignore_ascii_case("true") {
                    self.at += 1;
                    return Ok(AttributeValue::Bool(true));
                }
                if word.eq_ignore_ascii_case("false") {
                    self.at += 1;
                    return Ok(AttributeValue::Bool(false));
                }
                Err(unexpected_token(self.peek()))
            }
            _ => Err(unexpected_token(self.peek())),
        }
    }

    /// The members of a list or bag literal, up to its closing delimiter.
    fn value_list(&mut self, closing: &Token) -> Result<Vec<AttributeValue>, RequestError> {
        let mut members = Vec::new();
        if self.accept(closing) {
            return Ok(members);
        }
        loop {
            members.push(self.raw_value()?);
            if !self.accept(&Token::Comma) {
                break;
            }
        }
        self.expect_close(closing)?;
        Ok(members)
    }

    /// The delimiter that closes one collection literal: a literal the statement never
    /// closes is refused by NAMING the token the grammar required there, which is the
    /// phrase DynamoDB's parser reports for an unclosed tuple, list or bag.
    fn expect_close(&mut self, closing: &Token) -> Result<(), RequestError> {
        if self.accept(closing) {
            return Ok(());
        }
        Err(statement_parse_error(match closing {
            Token::CloseBrace => ParseDefect::ExpectedRightCurly,
            Token::CloseBracket => ParseDefect::ExpectedRightBracket,
            Token::CloseBag => ParseDefect::ExpectedRightDoubleAngleBracket,
            _ => ParseDefect::UnexpectedTerm,
        }))
    }

    /// The `'name': value` entries of a tuple literal, up to its closing brace.
    fn tuple_entries(&mut self) -> Result<Item, RequestError> {
        let mut entries = Item::new();
        if self.accept(&Token::CloseBrace) {
            return Ok(entries);
        }
        loop {
            let name = match self.peek().cloned() {
                Some(Token::Text(text)) => {
                    self.at += 1;
                    text
                }
                Some(Token::Quoted(name)) => {
                    self.at += 1;
                    name
                }
                // A tuple names its attributes with quoted names, so a bare word here is
                // the word it spells rather than a name — and a word that spells one of
                // the dialect's keywords is that keyword.
                _ => return Err(unexpected_token(self.peek())),
            };
            self.expect(&Token::Colon)?;
            let value = self.raw_value()?;
            if entries.insert(name, value).is_some() {
                return Err(statement_refused(
                    "a tuple names each of its attributes once",
                ));
            }
            if !self.accept(&Token::Comma) {
                break;
            }
        }
        self.expect_close(&Token::CloseBrace)?;
        Ok(entries)
    }
}

/// The DynamoDB set a `<<…>>` bag denotes: its members decide which of the three set
/// types it is, and a bag whose members are not all of one scalar type is no set.
fn set_of(members: Vec<AttributeValue>) -> Result<AttributeValue, RequestError> {
    if members.is_empty() {
        return Err(statement_refused("a set must hold at least one member"));
    }
    if members
        .iter()
        .all(|member| matches!(member, AttributeValue::S(_)))
    {
        return Ok(AttributeValue::Ss(
            members
                .into_iter()
                .filter_map(|member| match member {
                    AttributeValue::S(text) => Some(text),
                    _ => None,
                })
                .collect(),
        ));
    }
    if members
        .iter()
        .all(|member| matches!(member, AttributeValue::N(_)))
    {
        return Ok(AttributeValue::Ns(
            members
                .into_iter()
                .filter_map(|member| match member {
                    AttributeValue::N(text) => Some(text),
                    _ => None,
                })
                .collect(),
        ));
    }
    if members
        .iter()
        .all(|member| matches!(member, AttributeValue::B(_)))
    {
        return Ok(AttributeValue::Bs(
            members
                .into_iter()
                .filter_map(|member| match member {
                    AttributeValue::B(bytes) => Some(bytes),
                    _ => None,
                })
                .collect(),
        ));
    }
    Err(statement_refused(
        "the members of a set must all be strings, all numbers or all binary values",
    ))
}

// ===========================================================================
// Rendering back into the expression grammars the item API already evaluates
// ===========================================================================

/// The substitution tables one rendered expression's `#name` aliases and `:value`
/// placeholders resolve through, and how deep the render has walked.
#[derive(Debug, Default)]
struct Rendered {
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
    /// How many levels of the predicate or operand tree are being rendered right now,
    /// held to [`MAX_STATEMENT_DEPTH`] by [`Rendered::deeper`].
    depth: usize,
}

impl Rendered {
    /// Render `body` one level deeper, refusing a tree that nests past
    /// [`MAX_STATEMENT_DEPTH`].
    ///
    /// The render walks a tree whose shape is the CALLER's — `NOT (NOT (…))`,
    /// `list_append(list_append(…))` — so the budget travels with the walk rather than
    /// being inherited from the parse that built the tree: the recursion is bounded here,
    /// at the site that recurses, by the same ceiling [`Parser::nested`] holds the grammar
    /// to, and a tree past it is refused with this surface's own sentence instead of
    /// growing the stack.
    fn deeper<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, RequestError>,
    ) -> Result<T, RequestError> {
        if self.depth >= MAX_STATEMENT_DEPTH {
            return Err(statement_refused(&format!(
                "a statement may nest at most {MAX_STATEMENT_DEPTH} levels deep"
            )));
        }
        self.depth += 1;
        let outcome = body(self);
        self.depth -= 1;
        outcome
    }

    /// The alias for one attribute name, minted once per distinct name so the same
    /// name renders the same way wherever it appears.
    fn alias(&mut self, name: &str) -> String {
        if let Some((alias, _)) = self
            .names
            .iter()
            .find(|(_, stored)| stored.as_str() == name)
        {
            return alias.clone();
        }
        let alias = format!("#n{}", self.names.len());
        self.names.insert(alias.clone(), name.to_string());
        alias
    }

    /// The placeholder for one value.
    fn placeholder(&mut self, value: AttributeValue) -> String {
        let placeholder = format!(":v{}", self.values.len());
        self.values.insert(placeholder.clone(), value);
        placeholder
    }

    /// One document path, as the expression grammar spells it.
    fn path(&mut self, path: &StatementPath) -> String {
        let mut text = String::new();
        for segment in &path.segments {
            match segment {
                PathSegment::Name(name) => {
                    let alias = self.alias(name);
                    if !text.is_empty() {
                        text.push('.');
                    }
                    text.push_str(&alias);
                }
                PathSegment::Index(index) => {
                    text.push('[');
                    text.push_str(&index.to_string());
                    text.push(']');
                }
            }
        }
        text
    }

    fn operand(&mut self, operand: &PredicateOperand) -> String {
        match operand {
            PredicateOperand::Path(path) => self.path(path),
            PredicateOperand::Value(value) => self.placeholder(value.clone()),
            PredicateOperand::Size(path) => {
                let path = self.path(path);
                format!("size({path})")
            }
        }
    }

    /// One predicate, as a `ConditionExpression`.
    ///
    /// `AND` and `OR` chains are joined FLAT, and only a branch of lower precedence
    /// is parenthesised, so the rendered text nests no deeper than the statement did.
    /// Every descent into a branch goes through [`Self::deeper`], which bounds the walk.
    fn predicate(&mut self, predicate: &Predicate) -> Result<String, RequestError> {
        Ok(match predicate {
            Predicate::All(branches) => {
                let mut rendered = Vec::with_capacity(branches.len());
                for branch in branches {
                    let text = self.deeper(|this| this.predicate(branch))?;
                    rendered.push(if matches!(branch, Predicate::Any(_)) {
                        format!("({text})")
                    } else {
                        text
                    });
                }
                rendered.join(" AND ")
            }
            Predicate::Any(branches) => {
                let mut rendered = Vec::with_capacity(branches.len());
                for branch in branches {
                    rendered.push(self.deeper(|this| this.predicate(branch))?);
                }
                rendered.join(" OR ")
            }
            Predicate::Not(inner) => {
                let text = self.deeper(|this| this.predicate(inner))?;
                format!("NOT ({text})")
            }
            Predicate::Compare(left, operator, right) => {
                let left = self.operand(left);
                let right = self.operand(right);
                let operator = match operator {
                    CompareOp::Equal => "=",
                    CompareOp::NotEqual => "<>",
                    CompareOp::Less => "<",
                    CompareOp::LessOrEqual => "<=",
                    CompareOp::Greater => ">",
                    CompareOp::GreaterOrEqual => ">=",
                };
                format!("{left} {operator} {right}")
            }
            Predicate::Between(value, low, high) => {
                let value = self.operand(value);
                let low = self.operand(low);
                let high = self.operand(high);
                format!("{value} BETWEEN {low} AND {high}")
            }
            Predicate::In(value, candidates) => {
                let value = self.operand(value);
                let candidates: Vec<String> = candidates
                    .iter()
                    .map(|candidate| self.operand(candidate))
                    .collect();
                format!("{value} IN ({})", candidates.join(", "))
            }
            // `IS NULL` asks about the TYPE of a stored attribute: only a value of the
            // null type is null, and an attribute the item does not carry is MISSING
            // rather than null (`IS MISSING` is the clause that asks about that). So
            // `IS NULL` holds for a stored null and for nothing else, and its negation
            // holds for every item whose attribute is not a stored null — those that
            // carry another value and those that carry none.
            Predicate::IsNull(path, expected) => {
                let rendered = self.path(path);
                let null = self.placeholder(AttributeValue::Null(true));
                if *expected {
                    format!("{rendered} = {null}")
                } else {
                    format!("NOT {rendered} = {null}")
                }
            }
            Predicate::IsMissing(path, expected) => {
                let rendered = self.path(path);
                if *expected {
                    format!("attribute_not_exists({rendered})")
                } else {
                    format!("attribute_exists({rendered})")
                }
            }
            Predicate::BeginsWith(path, prefix) => {
                let path = self.path(path);
                let prefix = self.operand(prefix);
                format!("begins_with({path}, {prefix})")
            }
            Predicate::Contains(path, needle) => {
                let path = self.path(path);
                let needle = self.operand(needle);
                format!("contains({path}, {needle})")
            }
            Predicate::AttributeType(path, wanted) => {
                let path = self.path(path);
                let wanted = self.operand(wanted);
                format!("attribute_type({path}, {wanted})")
            }
            Predicate::Exists(path, expected) => {
                let path = self.path(path);
                if *expected {
                    format!("attribute_exists({path})")
                } else {
                    format!("attribute_not_exists({path})")
                }
            }
        })
    }

    /// One assignment operand, whose `list_append` nests as deeply as the statement
    /// spelled it — so each descent goes through [`Self::deeper`].
    fn set_operand(&mut self, operand: &AssignmentOperand) -> Result<String, RequestError> {
        Ok(match operand {
            AssignmentOperand::Path(path) => self.path(path),
            AssignmentOperand::Value(value) => self.placeholder(value.clone()),
            AssignmentOperand::ListAppend(left, right) => {
                let left = self.deeper(|this| this.set_operand(left))?;
                let right = self.deeper(|this| this.set_operand(right))?;
                format!("list_append({left}, {right})")
            }
        })
    }

    /// The `UPDATE`'s clauses, as an `UpdateExpression`: one section per action kind,
    /// in the order the grammar names them.
    fn clauses(&mut self, clauses: &[UpdateClause]) -> Result<String, RequestError> {
        let mut sets = Vec::new();
        let mut removes = Vec::new();
        let mut adds = Vec::new();
        let mut deletes = Vec::new();
        for clause in clauses {
            match clause {
                UpdateClause::Set(path, expression) => {
                    let path = self.path(path);
                    let value = match expression {
                        SetExpression::Single(operand) => self.set_operand(operand)?,
                        SetExpression::Sum(left, right) => {
                            let left = self.set_operand(left)?;
                            let right = self.set_operand(right)?;
                            format!("{left} + {right}")
                        }
                        SetExpression::Difference(left, right) => {
                            let left = self.set_operand(left)?;
                            let right = self.set_operand(right)?;
                            format!("{left} - {right}")
                        }
                    };
                    sets.push(format!("{path} = {value}"));
                }
                UpdateClause::Remove(path) => {
                    let path = self.path(path);
                    removes.push(path);
                }
                UpdateClause::Add(path, value) => {
                    let path = self.path(path);
                    let value = self.placeholder(value.clone());
                    adds.push(format!("{path} {value}"));
                }
                UpdateClause::Delete(path, value) => {
                    let path = self.path(path);
                    let value = self.placeholder(value.clone());
                    deletes.push(format!("{path} {value}"));
                }
            }
        }
        let mut sections = Vec::new();
        for (keyword, actions) in [
            ("SET", sets),
            ("REMOVE", removes),
            ("ADD", adds),
            ("DELETE", deletes),
        ] {
            if !actions.is_empty() {
                sections.push(format!("{keyword} {}", actions.join(", ")));
            }
        }
        Ok(sections.join(" "))
    }
}

/// The statement's `WHERE` predicate as the parsed `ConditionExpression` the item API
/// evaluates, so one implementation decides which comparison holds between which two
/// attribute values.
pub fn parsed_condition(predicate: &Predicate) -> Result<ParsedCondition, RequestError> {
    let mut rendered = Rendered::default();
    let text = rendered.predicate(predicate)?;
    ParsedCondition::parse(
        "Statement",
        &text,
        Substitutions::new(Some(&rendered.names), Some(&rendered.values)),
    )
}

/// The statement's `SET`/`REMOVE` clauses as the parsed `UpdateExpression` the item
/// API applies, so one implementation decides what each action does to an item.
pub fn parsed_update(clauses: &[UpdateClause]) -> Result<ParsedUpdate, RequestError> {
    let mut rendered = Rendered::default();
    let text = rendered.clauses(clauses)?;
    ParsedUpdate::parse(
        "Statement",
        &text,
        Substitutions::new(Some(&rendered.names), Some(&rendered.values)),
    )
}

// ===========================================================================
// What the predicate pins, and where a page resumes
// ===========================================================================

/// Every `attribute = value` the top level of this predicate requires, which is how a
/// statement addresses one item: "WHERE" equality on each key attribute.
///
/// Only the conjunction at the top level pins anything — an `OR` branch requires none
/// of its terms — and only a whole top-level attribute compared with a value.
pub fn pinned_equalities(predicate: &Predicate) -> HashMap<String, AttributeValue> {
    let mut pinned = HashMap::new();
    let mut pending = vec![predicate];
    while let Some(predicate) = pending.pop() {
        match predicate {
            Predicate::All(branches) => pending.extend(branches.iter()),
            Predicate::Compare(left, CompareOp::Equal, right) => {
                if let Some((name, value)) = equality(left, right) {
                    pinned.insert(name, value);
                }
            }
            _ => {}
        }
    }
    pinned
}

/// The attribute and value one `=` comparison pins, whichever side the path is on.
fn equality(
    left: &PredicateOperand,
    right: &PredicateOperand,
) -> Option<(String, AttributeValue)> {
    match (left, right) {
        (PredicateOperand::Path(path), PredicateOperand::Value(value))
        | (PredicateOperand::Value(value), PredicateOperand::Path(path))
            if path.is_attribute() =>
        {
            Some((path.attribute().to_string(), value.clone()))
        }
        _ => None,
    }
}

/// The `NextToken` a truncated page reports: the statement the page belongs to and the
/// primary key it stopped on, encoded so the next request resumes strictly after it.
///
/// Each key component is its type tag and its bytes in hex — the key's own stored form
/// (a number's canonical text, a string's UTF-8, a blob's bytes) — so the token carries
/// nothing but the key and decodes back to exactly it. It is prefixed by a digest of
/// the STATEMENT, because a `NextToken` continues one statement's result set and means
/// nothing to another: a token presented to a different statement would otherwise
/// resume a result set that was never computed for it.
pub fn encode_cursor(statement: &str, key: &ItemKey) -> String {
    let mut token = statement_fingerprint(statement);
    token.push(':');
    token.push_str(&encode_key_component(&key.partition));
    if let Some(sort) = key.sort.as_ref() {
        token.push('.');
        token.push_str(&encode_key_component(sort));
    }
    token
}

/// How much of the statement's digest the token carries. A prefix of a SHA-256 digest:
/// long enough that two different statements do not share one, short enough that the
/// token stays a token.
const FINGERPRINT_HEX_CHARS: usize = 16;

/// The digest that binds a token to its statement.
fn statement_fingerprint(statement: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(statement.as_bytes());
    hex::encode(digest)[..FINGERPRINT_HEX_CHARS].to_string()
}

fn encode_key_component(component: &AttributeValueKey) -> String {
    match component {
        AttributeValueKey::S(text) => format!("S{}", hex::encode(text.as_bytes())),
        AttributeValueKey::N(number) => {
            format!("N{}", hex::encode(number.canonical_text().as_bytes()))
        }
        AttributeValueKey::B(bytes) => format!("B{}", hex::encode(bytes)),
    }
}

/// The key a `NextToken` stands for, or the refusal for a token this service did not
/// mint FOR THIS STATEMENT — a token of another statement names a position in a result
/// set this statement never computed, so it is as invalid here as one nothing minted.
pub fn decode_cursor(statement: &str, token: &str) -> Result<ItemKey, RequestError> {
    let (fingerprint, key) = token.split_once(':').ok_or_else(invalid_next_token)?;
    if fingerprint != statement_fingerprint(statement) {
        return Err(invalid_next_token());
    }
    let mut components = key.split('.');
    let partition = components
        .next()
        .ok_or_else(invalid_next_token)
        .and_then(decode_key_component)?;
    let sort = match components.next() {
        Some(component) => Some(decode_key_component(component)?),
        None => None,
    };
    if components.next().is_some() {
        return Err(invalid_next_token());
    }
    Ok(ItemKey { partition, sort })
}

fn decode_key_component(component: &str) -> Result<AttributeValueKey, RequestError> {
    let mut characters = component.chars();
    let tag = characters.next().ok_or_else(invalid_next_token)?;
    let bytes = hex::decode(characters.as_str()).map_err(|_| invalid_next_token())?;
    match tag {
        'S' => String::from_utf8(bytes)
            .map(AttributeValueKey::S)
            .map_err(|_| invalid_next_token()),
        'N' => String::from_utf8(bytes)
            .ok()
            .and_then(|text| NumberKey::parse(&text))
            .map(AttributeValueKey::N)
            .ok_or_else(invalid_next_token),
        'B' => Ok(AttributeValueKey::B(bytes)),
        _ => Err(invalid_next_token()),
    }
}

/// How a `NextToken` that does not continue THIS request is refused — one this service
/// never minted, and one it minted for another statement, are the same mistake: the
/// token names a position in a result set this request did not ask for.
///
/// The sentence is the one the service states for that refusal, which says what is
/// wrong with the token rather than that it is unreadable: a client resuming a page
/// reads from it that the token belongs to another request.
fn invalid_next_token() -> RequestError {
    RequestError::Validation("NextToken does not match request".to_string())
}

// ===========================================================================
// Reading the stored items a SELECT reports
// ===========================================================================

/// The TOP-LEVEL attributes one predicate speaks about, in name order.
///
/// A predicate is judged against the item a source reports, so these are the attributes
/// that source has to carry for the predicate to mean anything — which is what
/// [`unprojected_filter_attributes`] holds an index's projection to. A path that reaches
/// inside an attribute needs that attribute, so only its first step counts.
///
/// The walk is an explicit WORKLIST rather than a recursion, the way
/// [`pinned_equalities`] walks the same tree: how deeply a predicate nests is the
/// client's choice, so the branches still to visit live in a vector on the heap and the
/// walk's own memory is never a function of the statement's shape.
pub fn predicate_attributes(predicate: &Predicate) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let operand = |operand: &PredicateOperand, named: &mut BTreeSet<String>| match operand {
        PredicateOperand::Path(path) | PredicateOperand::Size(path) => {
            named.insert(path.attribute().to_string());
        }
        PredicateOperand::Value(_) => {}
    };
    let mut pending = vec![predicate];
    while let Some(predicate) = pending.pop() {
        match predicate {
            Predicate::All(operands) | Predicate::Any(operands) => {
                pending.extend(operands.iter());
            }
            Predicate::Not(nested) => pending.push(nested.as_ref()),
            Predicate::Compare(left, _, right) => {
                operand(left, &mut named);
                operand(right, &mut named);
            }
            Predicate::Between(value, low, high) => {
                operand(value, &mut named);
                operand(low, &mut named);
                operand(high, &mut named);
            }
            Predicate::In(value, candidates) => {
                operand(value, &mut named);
                for candidate in candidates {
                    operand(candidate, &mut named);
                }
            }
            Predicate::IsNull(path, _)
            | Predicate::IsMissing(path, _)
            | Predicate::Exists(path, _) => {
                named.insert(path.attribute().to_string());
            }
            Predicate::BeginsWith(path, operand_value)
            | Predicate::Contains(path, operand_value)
            | Predicate::AttributeType(path, operand_value) => {
                named.insert(path.attribute().to_string());
                operand(operand_value, &mut named);
            }
        }
    }
    named
}

/// The attributes a statement's `WHERE` clause speaks about that the SOURCE it names
/// does not report, when that source is an index.
///
/// An index holds "attributes that are copied (projected) from the table into the
/// index" (`GlobalSecondaryIndex$Projection`), and a predicate is judged against what
/// the source reports — so a filter on an attribute the index does not project could
/// never be decided from the index. The statement surface refuses that rather than
/// answering from the parent table, and the refusal names the index and the attributes.
pub fn unprojected_filter_attributes(
    table: &Table,
    index: Option<&str>,
    predicate: Option<&Predicate>,
) -> Vec<String> {
    let (Some(index), Some(predicate)) = (index, predicate) else {
        return Vec::new();
    };
    let Some(projected) = projected_attributes(table, Some(index)) else {
        return Vec::new();
    };
    predicate_attributes(predicate)
        .into_iter()
        .filter(|attribute| !attribute.is_empty() && !projected.contains(attribute))
        .collect()
}

/// AWS's sentence for that refusal: the index the statement named and the filter
/// attributes it does not project, in name order.
pub fn unprojected_filter_refusal(index: &str, attributes: &[String]) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Secondary index {index} does not project one \
         or more filter attributes: [{}]",
        attributes.join(", ")
    ))
}

/// Which attributes of an item a source reports: every one for the table and for an
/// `ALL` index, and the projected ones for any other index.
///
/// Returns `None` when every attribute is reported.
pub fn projected_attributes(table: &Table, index: Option<&str>) -> Option<BTreeSet<String>> {
    let index = index?;
    let (key_schema, projection) = index_shape(table, index)?;
    let projection_type = projection.projection_type.as_deref().unwrap_or("ALL");
    if projection_type == "ALL" {
        return None;
    }
    let mut attributes: BTreeSet<String> = table
        .key_schema
        .iter()
        .chain(key_schema)
        .map(|element| element.attribute_name.clone())
        .collect();
    if projection_type == "INCLUDE" {
        attributes.extend(projection.non_key_attributes.iter().flatten().cloned());
    }
    Some(attributes)
}

/// One index's key schema and projection, whichever kind of index it is.
fn index_shape<'t>(
    table: &'t Table,
    index: &str,
) -> Option<(&'t [KeySchemaElement], &'t Projection)> {
    if let Some(found) = table
        .global_secondary_indexes
        .iter()
        .find(|candidate| candidate.name == index)
    {
        return Some((found.key_schema.as_slice(), &found.projection));
    }
    table
        .local_secondary_indexes
        .iter()
        .find(|candidate| candidate.name == index)
        .map(|found| (found.key_schema.as_slice(), &found.projection))
}

/// Whether `index` is one this table declares.
pub fn declares_index(table: &Table, index: &str) -> bool {
    index_shape(table, index).is_some()
}

/// Whether `index` is one of the table's GLOBAL secondary indexes.
///
/// The two kinds differ in what a read of them can ask for: a global index has its own
/// partitions and is replicated to, so it serves eventually consistent reads only,
/// while a local one shares the table's partitions and can be read consistently.
pub fn is_global_index(table: &Table, index: &str) -> bool {
    table
        .global_secondary_indexes
        .iter()
        .any(|candidate| candidate.name == index)
}

/// Whether an item is projected into `index`: an index holds only the items that
/// carry every attribute its key schema names.
pub fn index_holds(table: &Table, index: &str, item: &Item) -> bool {
    index_shape(table, index)
        .is_some_and(|(key_schema, _)| super::index_covers(key_schema, item))
}

/// The key attributes a source orders by: the table's own, or the index's plus the
/// table's.
pub fn orderable_attributes(table: &Table, index: Option<&str>) -> BTreeSet<String> {
    let mut attributes: BTreeSet<String> = table
        .key_schema
        .iter()
        .map(|element| element.attribute_name.clone())
        .collect();
    if let Some((key_schema, _)) = index.and_then(|index| index_shape(table, index)) {
        attributes.extend(
            key_schema
                .iter()
                .map(|element| element.attribute_name.clone()),
        );
    }
    attributes
}

/// Order two items by one attribute, as `ORDER BY` asked: the values compare the way
/// stored attribute values do, and an item the attribute is absent from sorts before
/// one that carries it.
pub fn compare_by_attribute(
    left: &Item,
    right: &Item,
    attribute: &str,
    descending: bool,
) -> Ordering {
    let ordering = match (left.get(attribute), right.get(attribute)) {
        (Some(left), Some(right)) => {
            super::compare_values(left, right).unwrap_or(Ordering::Equal)
        }
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    };
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

/// The one item a statement addresses: the primary key its `WHERE` clause pins by
/// equality.
///
/// "ExecuteStatement allows you to perform reads and SINGLETON writes" (the
/// operation's doc): a write statement names one item, so a `WHERE` clause that does
/// not pin every key attribute by equality names no item and is refused rather than
/// applied to whatever it matches. A read inside a transaction addresses one item the
/// same way, which is why both reach this.
pub fn singleton_key(table: &Table, predicate: &Predicate) -> Result<ItemKey, RequestError> {
    check_pinned_key_types(table, Some(predicate))?;
    let pinned = pinned_equalities(predicate);
    let mut key = Item::new();
    for element in &table.key_schema {
        let Some(value) = pinned.get(&element.attribute_name) else {
            // AWS's own sentence for this fact, and the whole of it is fixed: the rule
            // is about the clause as a whole — every key attribute pinned by equality —
            // so the refusal names no one attribute.
            return Err(RequestError::Validation(
                MANDATORY_KEY_EQUALITY_MESSAGE.to_string(),
            ));
        };
        key.insert(element.attribute_name.clone(), value.clone());
    }
    table.key_of_key_map(&key)
}

/// Hold every attribute a statement's `WHERE` clause pins BY EQUALITY to the type the
/// table declared for it, naming the attribute whose value does not match.
///
/// An equality on a key attribute is how a statement addresses items — it is what a
/// plan reads a partition by — so a value of another type addresses nothing the table
/// could hold: the statement is refused rather than served from a scan that answers no
/// rows. Only the attributes the table DECLARES a type for are held to one (those are
/// its key attributes, the table's own and its indexes'); every other attribute of an
/// item is untyped and compares as whatever it holds.
pub fn check_pinned_key_types(
    table: &Table,
    predicate: Option<&Predicate>,
) -> Result<(), RequestError> {
    let Some(predicate) = predicate else {
        return Ok(());
    };
    for (name, value) in pinned_equalities(predicate) {
        let Some(declared) = table.declared_type(&name) else {
            continue;
        };
        if declared != attribute_value_type(&value) {
            return Err(RequestError::Validation(format!(
                "Key attribute's data type should match its data type in table's schema: Key \
                 {name}"
            )));
        }
    }
    Ok(())
}

/// Just what a statement's projection names, in the item that holds it.
///
/// A projection reports one entry per path it names, under the path as the clause spells
/// it (or the alias the statement gave it), and reports nothing at all for a path the
/// item does not hold — so `SELECT a, b.c` over an item carrying neither reports an
/// empty tuple rather than nulls.
pub fn project_item(item: &Item, projection: Option<&Vec<ProjectionItem>>) -> Item {
    let Some(projection) = projection else {
        return item.clone();
    };
    projection
        .iter()
        .filter_map(|projected| {
            projected
                .path
                .read(item)
                .map(|value| (projected.identifier(), value.clone()))
        })
        .collect()
}
