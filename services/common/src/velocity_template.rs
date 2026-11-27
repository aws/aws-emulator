// SPDX-License-Identifier: CC0-1.0

//! The VELOCITY TEMPLATE LANGUAGE, read once for every emulated service whose API takes a
//! template written in it.
//!
//! Two services in this tree hand a caller's own Velocity template to the emulator and
//! render it against a call: API Gateway's mapping templates — "a map of Velocity templates
//! that are applied on the request payload" (`Integration$requestTemplates`) — and AppSync's
//! resolver mapping templates — "Mapping templates are written in the Apache Velocity
//! Template Language (VTL)" (`EvaluateMappingTemplate`). The LANGUAGE is the same for both;
//! what differs is the VARIABLE SURFACE each one resolves a reference against (`$input` /
//! `$stageVariables` / `$context` / `$util` for one, `$ctx` / `$util` for the other) and what
//! each one does with a reference it cannot resolve.
//!
//! So this module holds the language and nothing of either service: the SYNTAX ([`parse`],
//! [`Parse`]) — text, references, the `#set` assignment, the `#if` / `#elseif` / `#else` /
//! `#end` conditional, the `#foreach` loop, `#break`, `#stop`, the `##` and `#* … *#`
//! comments, the `#[[ … ]]#` block that is written out unparsed, the `#return` a service may
//! publish for ending an evaluation early, the expression grammar — whose logical and
//! relational operators carry both of their spellings, the symbols and the words (`and`, `or`,
//! `not`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`) — and
//! the line break a directive's own line ends with — and the VALUE model its operators and
//! METHODS are defined over ([`truthy`], [`equal`], [`ordering`], [`arithmetic`], [`rendered`],
//! [`number`], [`method`]), which is JSON because a template's values are the ones its call
//! carries.
//!
//! Each service drives it from its own renderer: it walks the [`Node`]s this produces,
//! resolves a [`Reference`] against its own surface, decides WHICH of the value methods it
//! answers, and decides its own answer for a reference or a call it does not. Nothing here
//! reads a store, a request or the clock, and nothing here executes anything the template says
//! beyond the operators and methods above.
//!
//! BOUNDS. The text is caller-supplied, so the parse is bounded rather than as deep as that
//! text nests: every block, every expression and every operator of a chain is one level
//! charged against [`MAX_NESTING`], a reference walks at most [`MAX_ACCESSES`] members, and
//! both are released again as the parse leaves them. The parse recurses only through those
//! charged levels. What a RUN of the parsed template may spend is the caller's own bound,
//! because only the caller knows what it is rendering for.

use ::serde_json::{Map as JsonMap, Value as Json};

/// How deeply a template may nest — a block within a block, an expression within an
/// expression, a string that is itself a template. A parse guard, so a caller-supplied
/// template cannot drive the parser's own recursion past the host's stack.
pub const MAX_NESTING: usize = 64;

/// How far one reference may walk: how many members, indexes and calls it may chain. The
/// same guard, for the recursion a walk into a value takes.
pub const MAX_ACCESSES: usize = 64;

/// The word spelling of the prefix `!` ([`Parse::unary`]), beside the word spellings of the
/// binary operators ([`Parse::word_operator`]).
const WORDED_NOT: [char; 3] = ['n', 'o', 't'];

/// One piece of a parsed template.
#[derive(Debug, Clone)]
pub enum Node {
    /// Text written out as it stands.
    Text(String),
    /// A reference written out: the value, or — for one that resolves to nothing — whatever
    /// the renderer's own rule says, which is why the reference's own text travels with it.
    Write {
        reference: Reference,
        quiet: bool,
        literal: String,
    },
    /// `#set($target = value)`.
    Set { target: Reference, value: Expr },
    /// `#if` with its `#elseif` branches and its `#else` body.
    If {
        branches: Vec<(Expr, Vec<Node>)>,
        otherwise: Vec<Node>,
    },
    /// `#foreach($name in over)`.
    ForEach {
        name: String,
        over: Expr,
        body: Vec<Node>,
    },
    /// `#break`, which ends the innermost loop.
    Break,
    /// `#stop`, which ends the whole evaluation.
    Stop,
    /// `#return` / `#return(value)`, which ends the whole evaluation and, where it names one,
    /// makes that value the template's own result.
    ///
    /// Not one of Velocity's own directives: AppSync publishes it for a mapping template
    /// ("return early" from a request or response template), so a renderer whose service does
    /// not publish it answers it the way it answers any construct it does not perform.
    Return(Option<Expr>),
}

/// A reference: the variable it starts at, and how it walks into the value.
#[derive(Debug, Clone)]
pub struct Reference {
    pub root: String,
    pub accesses: Vec<Access>,
}

/// One step of a reference's walk.
#[derive(Debug, Clone)]
pub enum Access {
    /// `.name`
    Prop(String),
    /// `[expr]`
    Index(Expr),
    /// `.name(args)`
    Call(String, Vec<Expr>),
}

/// An expression, as a condition, a `#set` value or an argument.
#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Json),
    /// A double-quoted string carrying references, which renders as a template of its own.
    Interpolated(Vec<Node>),
    Reference(Reference),
    List(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Binary(Operator, Box<Expr>, Box<Expr>),
}

/// The operators an expression may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

/// What ended a block of nodes.
#[derive(Debug)]
pub enum Terminator {
    Eof,
    End,
    Else,
    ElseIf(Expr),
}

/// What the grammar would have ACCEPTED where a parse stopped.
///
/// A parser states a failure by naming the token it stopped at and the tokens it could have
/// gone on with, so a caller that words a parse failure the way the language's own parser
/// words one — one line per accepted token — needs the accepted set beside the position.
/// Each variant is one POSITION OF THE GRAMMAR rather than one construct: what may follow a
/// complete operand is the same set wherever that operand sits, bar the token the construct
/// around it closes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Expected {
    /// Nothing the grammar states: the parse stopped against one of this reader's own BOUNDS
    /// (how deeply a template may nest) or against text the language's own lexer, not its
    /// parser, refuses — neither of which has an accepted-token set.
    #[default]
    Unstated,
    /// Where an operand must BEGIN: a `#set` value, a condition, an argument, an element.
    Operand,
    /// Just past a complete operand, inside a construct that closes with `)`.
    OperandDoneCloseParen,
    /// Just past a complete argument of a call: another one follows a comma.
    OperandDoneArgument,
    /// Just past a complete element of a `[ … ]` list, or of an index.
    OperandDoneCloseBracket,
    /// Just past a complete value of a `{ … }` map.
    OperandDoneCloseCurly,
    /// Just past a map's key, which its value follows a colon.
    MapSeparator,
    /// The `(` a directive's arguments open with.
    DirectiveArguments,
    /// The variable a `#set` assigns to or a `#foreach` names.
    Variable,
    /// The `=` of a `#set`.
    Assignment,
    /// The `in` of a `#foreach`.
    LoopIn,
    /// The `#end` an open block is closed by.
    BlockEnd,
    /// The `#end`, `#else` or `#elseif` an open conditional is continued or closed by.
    ConditionalEnd,
    /// The end of the template: nothing may follow.
    EndOfTemplate,
}

impl Expected {
    /// The accepted tokens, each as the TOKEN IMAGE the language's parser prints for it — a
    /// named token as `<NAME>`, a token the grammar spells inline as that text in quotes.
    ///
    /// The images and the names are the Apache Velocity parser grammar's own
    /// (`org.apache.velocity.runtime.parser.ParserConstants`: `RPAREN`, `LPAREN`,
    /// `WHITESPACE`, `IDENTIFIER`, `STRING_LITERAL`, `INTEGER_LITERAL`,
    /// `FLOATING_POINT_LITERAL`, `TRUE`, `FALSE`, `LOGICAL_NOT`, `LOGICAL_AND`, `LOGICAL_OR`,
    /// `LOGICAL_LT`, `LOGICAL_LE`, `LOGICAL_GT`, `LOGICAL_GE`, `LOGICAL_EQUALS`,
    /// `LOGICAL_NOT_EQUALS`, `IN`, `END`, `ELSE`, `ELSEIF`, `EOF`), which is what a diagnostic
    /// of that parser lists them under; the arithmetic operators, the index and the grouping
    /// characters are spelled inline by that grammar and so are listed quoted.
    #[must_use]
    pub fn tokens(self) -> Vec<&'static str> {
        // What may follow a COMPLETE operand: the expression may be indexed, the construct
        // around it may close, space may separate it from either, or a binary operator may
        // carry the expression on.
        let after_operand = |closers: &[&'static str]| {
            let mut tokens = vec![INDEX_TOKEN];
            tokens.extend_from_slice(closers);
            tokens.push(WHITESPACE_TOKEN);
            tokens.extend_from_slice(&ARITHMETIC_TOKENS);
            tokens.extend_from_slice(&LOGICAL_TOKENS);
            tokens
        };
        match self {
            Self::Unstated => Vec::new(),
            Self::Operand => OPERAND_TOKENS.to_vec(),
            Self::OperandDoneCloseParen => after_operand(&[CLOSE_PAREN_TOKEN]),
            Self::OperandDoneArgument => after_operand(&[CLOSE_PAREN_TOKEN, COMMA_TOKEN]),
            Self::OperandDoneCloseBracket => after_operand(&[CLOSE_BRACKET_TOKEN, COMMA_TOKEN]),
            Self::OperandDoneCloseCurly => after_operand(&[CLOSE_CURLY_TOKEN, COMMA_TOKEN]),
            Self::MapSeparator => vec![COLON_TOKEN, WHITESPACE_TOKEN],
            Self::DirectiveArguments => vec![OPEN_PAREN_TOKEN, WHITESPACE_TOKEN],
            Self::Variable => vec![IDENTIFIER_TOKEN, WHITESPACE_TOKEN],
            Self::Assignment => vec![ASSIGN_TOKEN, WHITESPACE_TOKEN],
            Self::LoopIn => vec![IN_TOKEN, WHITESPACE_TOKEN],
            Self::BlockEnd => vec![END_TOKEN],
            Self::ConditionalEnd => vec![END_TOKEN, ELSE_TOKEN, ELSEIF_TOKEN],
            Self::EndOfTemplate => vec![END_OF_INPUT_TOKEN],
        }
    }
}

/// A parse that did not finish: why it stopped, WHERE it stopped — the index, in CHARACTERS,
/// the cursor had reached — and what the grammar would have accepted there.
#[derive(Debug, Clone)]
pub struct ParseFailure {
    pub why: String,
    pub at: usize,
    pub expected: Expected,
}

/// Parse a whole template.
pub fn parse(template: &str) -> Result<Vec<Node>, String> {
    parse_with_position(template).map_err(|(why, _)| why)
}

/// Parse a whole template, reporting WHERE the parse stopped beside why it did: the index, in
/// CHARACTERS, the cursor had reached when the text stopped being a template.
///
/// A caller that reports a parse failure the way its own API words one — naming the line, the
/// column and the token the parse stopped at — needs the position as well as the reason, which
/// is why this is the parse [`parse`] itself is written in terms of.
pub fn parse_with_position(template: &str) -> Result<Vec<Node>, (String, usize)> {
    parse_with_diagnosis(template).map_err(|failure| (failure.why, failure.at))
}

/// Parse a whole template, reporting everything a DIAGNOSTIC of the failure states: why the
/// parse stopped, where it stopped and what the grammar would have accepted there
/// ([`ParseFailure`]).
///
/// This is the parse the two above are written in terms of, for a caller that words a parse
/// failure the way the language's own parser words one.
pub fn parse_with_diagnosis(template: &str) -> Result<Vec<Node>, ParseFailure> {
    let characters: Vec<char> = template.chars().collect();
    let mut parse = Parse::at(&characters, 0);
    let parsed = parse.block();
    let at = parse.at.min(characters.len());
    let (nodes, terminator) = parsed.map_err(|why| ParseFailure {
        why,
        at,
        expected: parse.expected,
    })?;
    match terminator {
        Terminator::Eof => Ok(nodes),
        // A block-closing directive at the top level closes a block nothing opened, and the
        // DIRECTIVE is what the parse stopped at, so the failure is stated where that
        // directive begins rather than wherever reading it left the cursor.
        _ => Err(ParseFailure {
            why: UNEXPECTED_DIRECTIVE.to_string(),
            at: parse.terminator_at.min(characters.len()),
            expected: Expected::EndOfTemplate,
        }),
    }
}

/// The parser's cursor over one template.
///
/// Public because a renderer that reads a template ONE construct at a time — API Gateway's,
/// whose references are resolved by its own reader — drives the same grammar from here
/// rather than keeping a second reading of it.
pub struct Parse<'t> {
    pub src: &'t [char],
    pub at: usize,
    /// How deeply the parse is nested, which [`MAX_NESTING`] bounds.
    pub nesting: usize,
    /// What the grammar would have accepted where this parse last stopped, which the failure
    /// that stopped it records as it is raised ([`Parse::expecting`]).
    pub expected: Expected,
    /// Where the block-closing directive that ended the last block begins, which is where a
    /// parse stopped when it is one that closed a block nothing had opened.
    pub terminator_at: usize,
}

impl<'t> Parse<'t> {
    /// A cursor over `src`, positioned at `at`.
    #[must_use]
    pub fn at(src: &'t [char], at: usize) -> Self {
        Self {
            src,
            at,
            nesting: 0,
            expected: Expected::Unstated,
            terminator_at: at,
        }
    }
}

impl Parse<'_> {
    /// State a failure: `why` the parse stopped, recording what the grammar would have
    /// accepted at the cursor so the diagnostic of it can list those tokens.
    pub fn expecting(&mut self, expected: Expected, why: &str) -> String {
        self.expected = expected;
        why.to_string()
    }

    /// Enter one level of nesting, or refuse a template that nests past the bound.
    pub fn deeper(&mut self) -> Result<(), String> {
        self.nesting = self.nesting.saturating_add(1);
        if self.nesting > MAX_NESTING {
            let why = format!("{TOO_DEEP} {MAX_NESTING}.");
            return Err(self.expecting(Expected::Unstated, &why));
        }
        Ok(())
    }

    /// Leave it again.
    pub fn shallower(&mut self) {
        self.nesting = self.nesting.saturating_sub(1);
    }

    /// Parse nodes until the template ends or a directive that closes a block is reached.
    pub fn block(&mut self) -> Result<(Vec<Node>, Terminator), String> {
        self.deeper()?;
        let parsed = self.block_nodes();
        self.shallower();
        parsed
    }

    /// The body of [`Self::block`], which that one bounds the nesting of.
    pub fn block_nodes(&mut self) -> Result<(Vec<Node>, Terminator), String> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, nodes: &mut Vec<Node>| {
            if !text.is_empty() {
                nodes.push(Node::Text(::std::mem::take(text)));
            }
        };
        while let Some(character) = self.peek(0) {
            match character {
                // An escaped `$` or `#` is the character itself.
                '\\' if matches!(self.peek(1), Some('$' | '#')) => {
                    text.push(self.peek(1).unwrap_or('\\'));
                    self.at += 2;
                }
                '#' => {
                    // A comment, an unparsed block, a directive — or, for anything else, a
                    // `#` of the text.
                    let hash = self.at;
                    if self.matches(&['#', '#']) {
                        self.skip_line();
                        continue;
                    }
                    if self.matches(&['#', '*']) {
                        self.at += 2;
                        self.skip_until(&['*', '#']);
                        continue;
                    }
                    if self.matches(&['#', '[', '[']) {
                        self.at += 3;
                        let start = self.at;
                        // Text written out UNPARSED, to its closing marker or to the end of
                        // the template when it carries none.
                        let end = if self.skip_until(&[']', ']', '#']) {
                            self.at.saturating_sub(3)
                        } else {
                            self.at
                        };
                        text.extend(self.src[start..end.max(start)].iter());
                        continue;
                    }
                    let Some(name) = self.directive_name() else {
                        text.push('#');
                        self.at += 1;
                        continue;
                    };
                    match name.as_str() {
                        "set" => {
                            let (target, value) = self.assignment()?;
                            flush(&mut text, &mut nodes);
                            nodes.push(Node::Set { target, value });
                            self.skip_trailing_newline();
                        }
                        "if" => {
                            let node = self.conditional()?;
                            flush(&mut text, &mut nodes);
                            nodes.push(node);
                        }
                        "foreach" => {
                            let node = self.loop_over()?;
                            flush(&mut text, &mut nodes);
                            nodes.push(node);
                        }
                        "return" => {
                            let value = self.returned()?;
                            flush(&mut text, &mut nodes);
                            nodes.push(Node::Return(value));
                            self.skip_trailing_newline();
                        }
                        "break" | "stop" => {
                            flush(&mut text, &mut nodes);
                            nodes.push(if name == "break" {
                                Node::Break
                            } else {
                                Node::Stop
                            });
                            self.skip_trailing_newline();
                        }
                        "end" => {
                            self.terminator_at = hash;
                            self.skip_trailing_newline();
                            flush(&mut text, &mut nodes);
                            return Ok((nodes, Terminator::End));
                        }
                        "else" => {
                            self.terminator_at = hash;
                            self.skip_trailing_newline();
                            flush(&mut text, &mut nodes);
                            return Ok((nodes, Terminator::Else));
                        }
                        "elseif" => {
                            self.terminator_at = hash;
                            let condition = self.parenthesised()?;
                            self.skip_trailing_newline();
                            flush(&mut text, &mut nodes);
                            return Ok((nodes, Terminator::ElseIf(condition)));
                        }
                        // A directive this does not know is text: a `#` followed by a word
                        // is only a directive when it is one of the above. Reading the name
                        // left the cursor past it, so the whole of it is read again AS TEXT —
                        // the `#` and the word both belong to the template's output, as they
                        // do in the `#name` an expression attribute name is written as.
                        _ => {
                            text.push('#');
                            self.at = hash.saturating_add(1);
                        }
                    }
                }
                '$' => match self.reference() {
                    Some((reference, quiet, literal)) => {
                        flush(&mut text, &mut nodes);
                        nodes.push(Node::Write {
                            reference,
                            quiet,
                            literal,
                        });
                    }
                    None => {
                        text.push('$');
                        self.at += 1;
                    }
                },
                other => {
                    text.push(other);
                    self.at += 1;
                }
            }
        }
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }
        Ok((nodes, Terminator::Eof))
    }

    /// The character `ahead` positions on, when the template has one.
    #[must_use]
    pub fn peek(&self, ahead: usize) -> Option<char> {
        self.src.get(self.at.saturating_add(ahead)).copied()
    }

    /// Whether the cursor is on `expected`.
    #[must_use]
    pub fn matches(&self, expected: &[char]) -> bool {
        expected
            .iter()
            .enumerate()
            .all(|(offset, character)| self.peek(offset) == Some(*character))
    }

    /// Skip to just past the end of this line.
    pub fn skip_line(&mut self) {
        while let Some(character) = self.peek(0) {
            self.at += 1;
            if character == '\n' {
                return;
            }
        }
    }

    /// Skip to just past the next `marker`, or to the end of the template; `true` when the
    /// marker was the reason it stopped.
    pub fn skip_until(&mut self, marker: &[char]) -> bool {
        while self.at < self.src.len() {
            if self.matches(marker) {
                self.at = self.at.saturating_add(marker.len());
                return true;
            }
            self.at += 1;
        }
        false
    }

    /// Velocity eats the line break that follows a directive, so a template written one
    /// directive per line renders without the blank lines between them.
    pub fn skip_trailing_newline(&mut self) {
        let mut at = self.at;
        while matches!(self.src.get(at), Some(' ' | '\t')) {
            at += 1;
        }
        if self.src.get(at) == Some(&'\r') {
            at += 1;
        }
        if self.src.get(at) == Some(&'\n') {
            self.at = at.saturating_add(1);
        }
    }

    /// Skip the whitespace an expression may be written around.
    pub fn skip_spaces(&mut self) {
        while matches!(self.peek(0), Some(character) if character.is_whitespace()) {
            self.at += 1;
        }
    }

    /// The name of the directive the cursor is on, with the cursor left just past it —
    /// `None` for a `#` that starts no directive, with the cursor left where it was.
    pub fn directive_name(&mut self) -> Option<String> {
        let start = self.at;
        self.at += 1;
        let braced = self.peek(0) == Some('{');
        if braced {
            self.at += 1;
        }
        let name = self.identifier();
        match name {
            Some(name) if !name.is_empty() => {
                if braced && self.peek(0) == Some('}') {
                    self.at += 1;
                }
                Some(name)
            }
            _ => {
                self.at = start;
                None
            }
        }
    }

    /// The identifier the cursor is on, with the cursor left just past it.
    pub fn identifier(&mut self) -> Option<String> {
        let first = self.peek(0)?;
        if !(first.is_alphabetic() || first == '_') {
            return None;
        }
        let mut name = String::new();
        while let Some(character) = self.peek(0) {
            if character.is_alphanumeric() || character == '_' || character == '-' {
                name.push(character);
                self.at += 1;
            } else {
                break;
            }
        }
        Some(name)
    }

    /// The reference the cursor is on, its quiet flag and its own text — `None` for a `$`
    /// that starts no reference, with the cursor left where it was.
    pub fn reference(&mut self) -> Option<(Reference, bool, String)> {
        let start = self.at;
        self.at += 1;
        let quiet = self.peek(0) == Some('!');
        if quiet {
            self.at += 1;
        }
        let braced = self.peek(0) == Some('{');
        if braced {
            self.at += 1;
        }
        let Some(root) = self.identifier() else {
            self.at = start;
            return None;
        };
        let mut accesses = Vec::new();
        loop {
            if self.peek(0) == Some('.')
                && matches!(self.peek(1), Some(character) if character.is_alphabetic() || character == '_')
            {
                self.at += 1;
                let Some(name) = self.identifier() else { break };
                if self.peek(0) == Some('(') {
                    match self.arguments() {
                        Ok(args) => accesses.push(Access::Call(name, args)),
                        Err(_) => {
                            self.at = start;
                            return None;
                        }
                    }
                } else {
                    accesses.push(Access::Prop(name));
                }
                continue;
            }
            if self.peek(0) == Some('[') {
                self.at += 1;
                let Ok(index) = self.expression() else {
                    self.at = start;
                    return None;
                };
                self.skip_spaces();
                if self.peek(0) != Some(']') {
                    self.at = start;
                    return None;
                }
                self.at += 1;
                accesses.push(Access::Index(index));
                continue;
            }
            break;
        }
        if braced {
            if self.peek(0) != Some('}') {
                self.at = start;
                return None;
            }
            self.at += 1;
        }
        if accesses.len() > MAX_ACCESSES {
            self.at = start;
            return None;
        }
        let literal: String = self.src[start..self.at].iter().collect();
        Some((Reference { root, accesses }, quiet, literal))
    }

    /// The argument list the cursor is on: `(` expressions `)`.
    pub fn arguments(&mut self) -> Result<Vec<Expr>, String> {
        if self.peek(0) != Some('(') {
            return Err(self.expecting(Expected::DirectiveArguments, NOT_AN_ARGUMENT_LIST));
        }
        self.at += 1;
        let mut args = Vec::new();
        self.skip_spaces();
        if self.peek(0) == Some(')') {
            self.at += 1;
            return Ok(args);
        }
        loop {
            args.push(self.expression()?);
            self.skip_spaces();
            match self.peek(0) {
                Some(',') => {
                    self.at += 1;
                    self.skip_spaces();
                }
                Some(')') => {
                    self.at += 1;
                    return Ok(args);
                }
                _ => {
                    return Err(self.expecting(Expected::OperandDoneArgument, NOT_AN_ARGUMENT_LIST))
                }
            }
        }
    }

    /// The value a `#return` names, which is `None` for one written with no value at all and
    /// for one written as `#return()`: the directive ends the evaluation either way, and only
    /// a value it names becomes the result.
    pub fn returned(&mut self) -> Result<Option<Expr>, String> {
        let opening = self.at;
        self.skip_spaces();
        if self.peek(0) != Some('(') {
            // The spaces belong to the template's own text where the directive names no value
            // at all, so the cursor is left where the directive ended.
            self.at = opening;
            return Ok(None);
        }
        self.at += 1;
        self.skip_spaces();
        if self.peek(0) == Some(')') {
            self.at += 1;
            return Ok(None);
        }
        let value = self.expression()?;
        self.skip_spaces();
        if self.peek(0) != Some(')') {
            return Err(self.expecting(Expected::OperandDoneCloseParen, NOT_A_DIRECTIVE));
        }
        self.at += 1;
        Ok(Some(value))
    }

    /// The `( expression )` a directive takes.
    pub fn parenthesised(&mut self) -> Result<Expr, String> {
        self.skip_spaces();
        if self.peek(0) != Some('(') {
            return Err(self.expecting(Expected::DirectiveArguments, NOT_A_DIRECTIVE));
        }
        self.at += 1;
        let expression = self.expression()?;
        self.skip_spaces();
        if self.peek(0) != Some(')') {
            return Err(self.expecting(Expected::OperandDoneCloseParen, NOT_A_DIRECTIVE));
        }
        self.at += 1;
        Ok(expression)
    }

    /// The `( $target = value )` a `#set` takes.
    pub fn assignment(&mut self) -> Result<(Reference, Expr), String> {
        self.skip_spaces();
        if self.peek(0) != Some('(') {
            return Err(self.expecting(Expected::DirectiveArguments, NOT_A_DIRECTIVE));
        }
        self.at += 1;
        self.skip_spaces();
        let Some((target, _, _)) = self.reference() else {
            return Err(self.expecting(Expected::Variable, NOT_AN_ASSIGNMENT));
        };
        self.skip_spaces();
        if self.peek(0) != Some('=') {
            return Err(self.expecting(Expected::Assignment, NOT_AN_ASSIGNMENT));
        }
        self.at += 1;
        let value = self.expression()?;
        self.skip_spaces();
        if self.peek(0) != Some(')') {
            return Err(self.expecting(Expected::OperandDoneCloseParen, NOT_AN_ASSIGNMENT));
        }
        self.at += 1;
        Ok((target, value))
    }

    /// `#if` through its `#end`.
    pub fn conditional(&mut self) -> Result<Node, String> {
        let mut condition = self.parenthesised()?;
        self.skip_trailing_newline();
        let mut branches = Vec::new();
        loop {
            let (body, terminator) = self.block()?;
            match terminator {
                Terminator::End => {
                    branches.push((condition, body));
                    return Ok(Node::If {
                        branches,
                        otherwise: Vec::new(),
                    });
                }
                Terminator::ElseIf(next) => {
                    branches.push((condition, body));
                    condition = next;
                }
                Terminator::Else => {
                    branches.push((condition, body));
                    let (otherwise, terminator) = self.block()?;
                    return match terminator {
                        Terminator::End => Ok(Node::If {
                            branches,
                            otherwise,
                        }),
                        _ => Err(self.expecting(Expected::BlockEnd, NO_END)),
                    };
                }
                Terminator::Eof => return Err(self.expecting(Expected::ConditionalEnd, NO_END)),
            }
        }
    }

    /// `#foreach` through its `#end`.
    pub fn loop_over(&mut self) -> Result<Node, String> {
        self.skip_spaces();
        if self.peek(0) != Some('(') {
            return Err(self.expecting(Expected::DirectiveArguments, NOT_A_DIRECTIVE));
        }
        self.at += 1;
        self.skip_spaces();
        if self.peek(0) != Some('$') {
            return Err(self.expecting(Expected::Variable, NOT_A_LOOP));
        }
        self.at += 1;
        let braced = self.peek(0) == Some('{');
        if braced {
            self.at += 1;
        }
        let Some(name) = self.identifier() else {
            return Err(self.expecting(Expected::Variable, NOT_A_LOOP));
        };
        if braced && self.peek(0) == Some('}') {
            self.at += 1;
        }
        self.skip_spaces();
        if !self.matches(&['i', 'n']) {
            return Err(self.expecting(Expected::LoopIn, NOT_A_LOOP));
        }
        self.at += 2;
        let over = self.expression()?;
        self.skip_spaces();
        if self.peek(0) != Some(')') {
            return Err(self.expecting(Expected::OperandDoneCloseParen, NOT_A_LOOP));
        }
        self.at += 1;
        self.skip_trailing_newline();
        let (body, terminator) = self.block()?;
        match terminator {
            Terminator::End => Ok(Node::ForEach { name, over, body }),
            _ => Err(self.expecting(Expected::BlockEnd, NO_END)),
        }
    }

    /// One expression, lowest precedence first.
    pub fn expression(&mut self) -> Result<Expr, String> {
        self.deeper()?;
        let parsed = self.either();
        self.shallower();
        parsed
    }

    /// A left-associative operator CHAIN: `operand` parses each side and `operator` reads the
    /// operator at the cursor, answering it and how many characters it takes.
    ///
    /// Every operator is one more level of the tree this builds, charged against the nesting
    /// bound like every other level — evaluation recurses over that tree, so its depth is
    /// bounded however the text spells it — and the levels are released when the chain ends.
    pub fn chain(
        &mut self,
        operator: impl Fn(&mut Self) -> Option<(Operator, usize)>,
        operand: impl Fn(&mut Self) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let mut left = operand(self)?;
        let mut charged = 0usize;
        let parsed = loop {
            self.skip_spaces();
            let Some((named, width)) = operator(self) else {
                break Ok(left);
            };
            self.at = self.at.saturating_add(width);
            if let Err(why) = self.deeper() {
                break Err(why);
            }
            charged = charged.saturating_add(1);
            match operand(self) {
                Ok(right) => left = Expr::Binary(named, Box::new(left), Box::new(right)),
                Err(why) => break Err(why),
            }
        };
        for _ in 0..charged {
            self.shallower();
        }
        parsed
    }

    pub fn either(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| {
                parse
                    .matches(&['|', '|'])
                    .then_some((Operator::Or, 2))
                    .or_else(|| parse.word_operator("or", Operator::Or))
            },
            Self::both,
        )
    }

    pub fn both(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| {
                parse
                    .matches(&['&', '&'])
                    .then_some((Operator::And, 2))
                    .or_else(|| parse.word_operator("and", Operator::And))
            },
            Self::equality,
        )
    }

    pub fn equality(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| {
                if parse.matches(&['=', '=']) {
                    Some((Operator::Equal, 2))
                } else if parse.matches(&['!', '=']) {
                    Some((Operator::NotEqual, 2))
                } else {
                    parse
                        .word_operator("eq", Operator::Equal)
                        .or_else(|| parse.word_operator("ne", Operator::NotEqual))
                }
            },
            Self::relational,
        )
    }

    pub fn relational(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| {
                if parse.matches(&['<', '=']) {
                    Some((Operator::LessOrEqual, 2))
                } else if parse.matches(&['>', '=']) {
                    Some((Operator::GreaterOrEqual, 2))
                } else if parse.matches(&['<']) {
                    Some((Operator::Less, 1))
                } else if parse.matches(&['>']) {
                    Some((Operator::Greater, 1))
                } else {
                    parse
                        .word_operator("le", Operator::LessOrEqual)
                        .or_else(|| parse.word_operator("ge", Operator::GreaterOrEqual))
                        .or_else(|| parse.word_operator("lt", Operator::Less))
                        .or_else(|| parse.word_operator("gt", Operator::Greater))
                }
            },
            Self::additive,
        )
    }

    /// The operator `named` where the cursor is on that WORD spelling of it, and how wide it
    /// is — `None` where it is not.
    ///
    /// The language spells each of its logical and relational operators two ways: `&&` and
    /// `and`, `||` and `or`, `!` and `not`, `==` and `eq`, `!=` and `ne`, `<` and `lt`, `<=`
    /// and `le`, `>` and `gt`, `>=` and `ge`. A word is the operator only where it stands on
    /// its own: one that runs straight into a name (`ltx`) is part of that name, not an
    /// operator and an operand.
    #[must_use]
    pub fn word_operator(&self, named: &str, operator: Operator) -> Option<(Operator, usize)> {
        let spelling: Vec<char> = named.chars().collect();
        if !self.matches(&spelling) {
            return None;
        }
        let after = self.peek(spelling.len());
        let runs_on = after.is_some_and(|character| {
            character.is_alphanumeric() || character == '_' || character == '-'
        });
        (!runs_on).then_some((operator, spelling.len()))
    }

    pub fn additive(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| match parse.peek(0) {
                Some('+') => Some((Operator::Add, 1)),
                // A `-` that starts a number is part of it, not an operator; the cursor is
                // past an operand here, so this one is the operator.
                Some('-') => Some((Operator::Subtract, 1)),
                _ => None,
            },
            Self::multiplicative,
        )
    }

    pub fn multiplicative(&mut self) -> Result<Expr, String> {
        self.chain(
            |parse| match parse.peek(0) {
                Some('*') => Some((Operator::Multiply, 1)),
                Some('/') => Some((Operator::Divide, 1)),
                Some('%') => Some((Operator::Remainder, 1)),
                _ => None,
            },
            Self::unary,
        )
    }

    pub fn unary(&mut self) -> Result<Expr, String> {
        self.skip_spaces();
        // `not` is the word spelling of `!`: it is the prefix operator where it stands on its
        // own before an operand, and part of a name where it runs into one.
        if self.matches(&WORDED_NOT)
            && matches!(self.peek(WORDED_NOT.len()), Some(' ' | '\t' | '(' | '$' | '!'))
        {
            self.at = self.at.saturating_add(WORDED_NOT.len());
            self.deeper()?;
            let inner = self.unary();
            self.shallower();
            return Ok(Expr::Not(Box::new(inner?)));
        }
        match self.peek(0) {
            // A prefix operator is one more level of nesting, charged like every other, so a
            // chain of them cannot drive this recursion past the bound.
            Some('!') if self.peek(1) != Some('=') => {
                self.at += 1;
                self.deeper()?;
                let inner = self.unary();
                self.shallower();
                Ok(Expr::Not(Box::new(inner?)))
            }
            Some('-') if !matches!(self.peek(1), Some(digit) if digit.is_ascii_digit()) => {
                self.at += 1;
                self.deeper()?;
                let inner = self.unary();
                self.shallower();
                Ok(Expr::Negate(Box::new(inner?)))
            }
            _ => self.primary(),
        }
    }

    pub fn primary(&mut self) -> Result<Expr, String> {
        self.skip_spaces();
        match self.peek(0) {
            Some('(') => {
                self.at += 1;
                let inner = self.expression()?;
                self.skip_spaces();
                if self.peek(0) != Some(')') {
                    return Err(self.expecting(Expected::OperandDoneCloseParen, NOT_AN_EXPRESSION));
                }
                self.at += 1;
                Ok(inner)
            }
            Some('$') => match self.reference() {
                Some((reference, _, _)) => Ok(Expr::Reference(reference)),
                None => Err(self.expecting(Expected::Operand, NOT_AN_EXPRESSION)),
            },
            Some('\'') => {
                let text = self.quoted('\'')?;
                Ok(Expr::Literal(Json::String(text)))
            }
            Some('"') => {
                let text = self.quoted('"')?;
                // A double-quoted string carries references, so it renders as a template.
                if text.contains('$') || text.contains('#') {
                    let nested: Vec<char> = text.chars().collect();
                    let mut parse = Parse::at(&nested, 0);
                    parse.nesting = self.nesting;
                    let parsed = parse.block();
                    // The string is a template of its own, parsed by its own cursor, so what
                    // the grammar accepted where THAT parse stopped is carried back out to be
                    // stated: only the position stays this one's, because the inner cursor
                    // counts from the start of the string rather than of the template.
                    self.expected = parse.expected;
                    let (nodes, terminator) = parsed?;
                    return match terminator {
                        Terminator::Eof => Ok(Expr::Interpolated(nodes)),
                        _ => Err(self.expecting(Expected::EndOfTemplate, UNEXPECTED_DIRECTIVE)),
                    };
                }
                Ok(Expr::Literal(Json::String(text)))
            }
            Some('[') => {
                self.at += 1;
                let mut elements = Vec::new();
                self.skip_spaces();
                if self.peek(0) == Some(']') {
                    self.at += 1;
                    return Ok(Expr::List(elements));
                }
                loop {
                    elements.push(self.expression()?);
                    self.skip_spaces();
                    match self.peek(0) {
                        Some(',') => self.at += 1,
                        Some(']') => {
                            self.at += 1;
                            return Ok(Expr::List(elements));
                        }
                        _ => {
                            return Err(self
                                .expecting(Expected::OperandDoneCloseBracket, NOT_AN_EXPRESSION))
                        }
                    }
                }
            }
            Some('{') => {
                self.at += 1;
                let mut entries = Vec::new();
                self.skip_spaces();
                if self.peek(0) == Some('}') {
                    self.at += 1;
                    return Ok(Expr::Map(entries));
                }
                loop {
                    let key = self.expression()?;
                    self.skip_spaces();
                    if self.peek(0) != Some(':') {
                        return Err(self.expecting(Expected::MapSeparator, NOT_AN_EXPRESSION));
                    }
                    self.at += 1;
                    let value = self.expression()?;
                    entries.push((key, value));
                    self.skip_spaces();
                    match self.peek(0) {
                        Some(',') => self.at += 1,
                        Some('}') => {
                            self.at += 1;
                            return Ok(Expr::Map(entries));
                        }
                        _ => {
                            return Err(
                                self.expecting(Expected::OperandDoneCloseCurly, NOT_AN_EXPRESSION)
                            )
                        }
                    }
                }
            }
            Some(character) if character.is_ascii_digit() || character == '-' => self.number(),
            Some(character) if character.is_alphabetic() => {
                let Some(word) = self.identifier() else {
                    return Err(self.expecting(Expected::Operand, NOT_AN_EXPRESSION));
                };
                match word.as_str() {
                    "true" => Ok(Expr::Literal(Json::Bool(true))),
                    "false" => Ok(Expr::Literal(Json::Bool(false))),
                    "null" => Ok(Expr::Literal(Json::Null)),
                    _ => Err(self.expecting(Expected::Operand, NOT_AN_EXPRESSION)),
                }
            }
            _ => Err(self.expecting(Expected::Operand, NOT_AN_EXPRESSION)),
        }
    }

    /// The number the cursor is on.
    pub fn number(&mut self) -> Result<Expr, String> {
        let start = self.at;
        if self.peek(0) == Some('-') {
            self.at += 1;
        }
        let mut seen_point = false;
        while let Some(character) = self.peek(0) {
            if character.is_ascii_digit() {
                self.at += 1;
            } else if character == '.'
                && !seen_point
                && matches!(self.peek(1), Some(digit) if digit.is_ascii_digit())
            {
                seen_point = true;
                self.at += 1;
            } else {
                break;
            }
        }
        let text: String = self.src[start..self.at].iter().collect();
        match text.parse::<f64>() {
            Ok(parsed) => Ok(Expr::Literal(number(parsed))),
            Err(_) => Err(self.expecting(Expected::Operand, NOT_AN_EXPRESSION)),
        }
    }

    /// The quoted string the cursor is on, with its escapes resolved.
    pub fn quoted(&mut self, quote: char) -> Result<String, String> {
        self.at += 1;
        let mut text = String::new();
        while let Some(character) = self.peek(0) {
            self.at += 1;
            if character == '\\' {
                if let Some(escaped) = self.peek(0) {
                    self.at += 1;
                    text.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                }
                continue;
            }
            if character == quote {
                return Ok(text);
            }
            text.push(character);
        }
        // A string the text never closes is what the LEXER refuses rather than the parser, so
        // there is no accepted-token set to state for it.
        Err(self.expecting(Expected::Unstated, NOT_AN_EXPRESSION))
    }
}

// ---------------------------------------------------------------------------------------
// The VALUES the language's operators are defined over.
// ---------------------------------------------------------------------------------------

/// How a value is written into the rendered text.
///
/// A string is written as itself and every other value as the JSON text of it, which is the
/// form a template's own output is read back as.
#[must_use]
pub fn rendered(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Whether a value is TRUE to a condition, by Velocity's own rule: a `boolean` decides by
/// its own value, and every other value decides by being there at all — which is why an
/// empty string is true here.
#[must_use]
pub fn truthy(value: Option<&Json>) -> bool {
    match value {
        None | Some(Json::Null) => false,
        Some(Json::Bool(flag)) => *flag,
        Some(_) => true,
    }
}

/// Whether two values are EQUAL: numbers by their numeric value, strings, booleans, lists
/// and objects by structure, and an absent value only to another absent one.
#[must_use]
pub fn equal(left: Option<&Json>, right: Option<&Json>) -> bool {
    match (left, right) {
        (None, None) => true,
        (None, Some(Json::Null)) | (Some(Json::Null), None) => true,
        (Some(left), Some(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => left == right,
            _ => left == right,
        },
        _ => false,
    }
}

/// How two values ORDER, for `<`, `<=`, `>` and `>=`: numbers numerically and strings
/// lexicographically. Anything else cannot be ordered, which is a fault.
pub fn ordering(left: Option<&Json>, right: Option<&Json>) -> Result<::std::cmp::Ordering, String> {
    match (left, right) {
        (Some(left), Some(right)) => match (left.as_f64(), right.as_f64()) {
            (Some(left), Some(right)) => left
                .partial_cmp(&right)
                .ok_or_else(|| NOT_COMPARABLE.to_string()),
            _ => match (left.as_str(), right.as_str()) {
                (Some(left), Some(right)) => Ok(left.cmp(right)),
                _ => Err(NOT_COMPARABLE.to_string()),
            },
        },
        _ => Err(NOT_COMPARABLE.to_string()),
    }
}

/// What `+`, `-`, `*`, `/` and `%` come to.
///
/// Two numbers are arithmetic, and `+` with a string on either side concatenates — the
/// Java operator a template reaches for when it builds a key out of parts. Anything else is
/// a fault.
pub fn arithmetic(operator: char, left: &Json, right: &Json) -> Result<Json, String> {
    if operator == '+' && (left.is_string() || right.is_string()) {
        return Ok(Json::String(format!(
            "{}{}",
            rendered(left),
            rendered(right)
        )));
    }
    let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
        return Err(format!("{NOT_ARITHMETIC} {operator}"));
    };
    let produced = match operator {
        '+' => left + right,
        '-' => left - right,
        '*' => left * right,
        '/' => {
            if right == 0.0 {
                return Err(DIVIDE_BY_ZERO.to_string());
            }
            left / right
        }
        '%' => {
            if right == 0.0 {
                return Err(DIVIDE_BY_ZERO.to_string());
            }
            left % right
        }
        other => return Err(format!("{NOT_ARITHMETIC} {other}")),
    };
    Ok(number(produced))
}

/// A computed number as the value a template carries: an integral result stays an integer,
/// so a count or an index renders as `3` rather than as `3.0`.
#[must_use]
pub fn number(produced: f64) -> Json {
    if produced.fract() == 0.0 && produced.abs() < 9.007_199_254_740_992e15 {
        return Json::Number((produced as i64).into());
    }
    ::serde_json::Number::from_f64(produced).map_or(Json::Null, Json::Number)
}

/// An empty object, as the value a template that builds one starts from.
#[must_use]
pub fn empty_object() -> Json {
    Json::Object(JsonMap::new())
}

/// Call `name` on `target` with `args`, and answer what it returned — `None` for a method
/// that returns nothing.
///
/// The METHODS of the language's value model, which is the Java one: an object is a `Map`, an
/// array a `List`, and a string its own scalar, so what a reference may call on a value it
/// reached are the `Map`, `List` and `String` members. They are defined over the values a call
/// carries and nothing else, so they live here beside the operators rather than in each
/// service that renders a template: a service decides WHICH of them it answers (its own
/// policy) and what it does with a call this does not answer, not what one of them comes to.
///
/// `target` is borrowed MUTABLY because the collection methods of the language mutate the
/// value they are called on: `put` and `add` are how a template builds up a map or a list it
/// has in hand. A caller that resolves values immutably answers none of those.
pub fn method(target: &mut Json, name: &str, args: &[Json]) -> Result<Option<Json>, String> {
    let arg = |index: usize| args.get(index).cloned().unwrap_or(Json::Null);
    let text = |index: usize| match args.get(index) {
        Some(Json::String(text)) => text.clone(),
        Some(other) => rendered(other),
        None => String::new(),
    };
    match (&mut *target, name) {
        // The members of a `Map`.
        (Json::Object(map), "get") => Ok(Some(map.get(&text(0)).cloned().unwrap_or(Json::Null))),
        (Json::Object(map), "put") => Ok(map.insert(text(0), arg(1))),
        (Json::Object(map), "putAll") => {
            if let Json::Object(entries) = arg(0) {
                for (key, value) in entries {
                    map.insert(key, value);
                }
            }
            Ok(None)
        }
        (Json::Object(map), "remove") => Ok(map.remove(&text(0))),
        (Json::Object(map), "containsKey") => Ok(Some(Json::Bool(map.contains_key(&text(0))))),
        (Json::Object(map), "keySet") => Ok(Some(Json::Array(
            map.keys().map(|key| Json::String(key.clone())).collect(),
        ))),
        (Json::Object(map), "values") => Ok(Some(Json::Array(map.values().cloned().collect()))),
        // An entry of an `entrySet` is read as `$entry.key` / `$entry.value`, which is what
        // the two members of each object below are.
        (Json::Object(map), "entrySet") => Ok(Some(Json::Array(
            map.iter()
                .map(|(key, value)| {
                    let mut entry = JsonMap::new();
                    entry.insert("key".to_string(), Json::String(key.clone()));
                    entry.insert("value".to_string(), value.clone());
                    Json::Object(entry)
                })
                .collect(),
        ))),
        (Json::Object(map), "size") => Ok(Some(count(map.len()))),
        (Json::Object(map), "isEmpty") => Ok(Some(Json::Bool(map.is_empty()))),
        (Json::Object(map), "clear") => {
            map.clear();
            Ok(None)
        }
        // The members of a `List`.
        (Json::Array(list), "add") => {
            match args.len() {
                // `add(index, element)` inserts, `add(element)` appends.
                2 => {
                    let at = index_of(&arg(0), list.len())?;
                    list.insert(at, arg(1));
                    Ok(None)
                }
                _ => {
                    list.push(arg(0));
                    Ok(Some(Json::Bool(true)))
                }
            }
        }
        (Json::Array(list), "addAll") => {
            if let Json::Array(appended) = arg(0) {
                list.extend(appended);
            }
            Ok(Some(Json::Bool(true)))
        }
        (Json::Array(list), "get") => {
            let at = index_of(&arg(0), list.len().saturating_sub(1))?;
            Ok(Some(list.get(at).cloned().unwrap_or(Json::Null)))
        }
        (Json::Array(list), "set") => {
            let at = index_of(&arg(0), list.len().saturating_sub(1))?;
            let replaced = list.get(at).cloned();
            if let Some(slot) = list.get_mut(at) {
                *slot = arg(1);
            }
            Ok(replaced)
        }
        (Json::Array(list), "remove") => {
            let at = index_of(&arg(0), list.len().saturating_sub(1))?;
            Ok((at < list.len()).then(|| list.remove(at)))
        }
        (Json::Array(list), "contains") => Ok(Some(Json::Bool(
            list.iter().any(|element| equal(Some(element), Some(&arg(0)))),
        ))),
        (Json::Array(list), "indexOf") => Ok(Some(count_or_absent(
            list.iter()
                .position(|element| equal(Some(element), Some(&arg(0)))),
        ))),
        (Json::Array(list), "size") => Ok(Some(count(list.len()))),
        (Json::Array(list), "isEmpty") => Ok(Some(Json::Bool(list.is_empty()))),
        (Json::Array(list), "clear") => {
            list.clear();
            Ok(None)
        }
        // The members of a `String`.
        (Json::String(subject), "length" | "size") => Ok(Some(count(subject.chars().count()))),
        (Json::String(subject), "isEmpty") => Ok(Some(Json::Bool(subject.is_empty()))),
        (Json::String(subject), "toUpperCase") => Ok(Some(Json::String(subject.to_uppercase()))),
        (Json::String(subject), "toLowerCase") => Ok(Some(Json::String(subject.to_lowercase()))),
        (Json::String(subject), "trim") => Ok(Some(Json::String(subject.trim().to_string()))),
        (Json::String(subject), "substring") => {
            let characters: Vec<char> = subject.chars().collect();
            let from = index_of(&arg(0), characters.len())?;
            let to = match args.len() {
                0 | 1 => characters.len(),
                _ => index_of(&arg(1), characters.len())?,
            };
            let Some(selected) = characters.get(from..to) else {
                return Err(format!("{OUTSIDE_THE_STRING} {from}..{to}"));
            };
            Ok(Some(Json::String(selected.iter().collect())))
        }
        (Json::String(subject), "replace" | "replaceAll") => {
            Ok(Some(Json::String(subject.replace(&text(0), &text(1)))))
        }
        (Json::String(subject), "split") => Ok(Some(Json::Array(
            subject
                .split(text(0).as_str())
                .map(|part| Json::String(part.to_string()))
                .collect(),
        ))),
        (Json::String(subject), "contains") => Ok(Some(Json::Bool(subject.contains(&text(0))))),
        (Json::String(subject), "startsWith") => {
            Ok(Some(Json::Bool(subject.starts_with(&text(0)))))
        }
        (Json::String(subject), "endsWith") => Ok(Some(Json::Bool(subject.ends_with(&text(0))))),
        (Json::String(subject), "indexOf") => {
            Ok(Some(count_or_absent(subject.find(&text(0)).map(|at| {
                subject.get(..at).map_or(at, |head| head.chars().count())
            }))))
        }
        (Json::String(subject), "concat") => Ok(Some(Json::String(format!("{subject}{}", text(0))))),
        (Json::String(subject), "equals") => Ok(Some(Json::Bool(
            Some(subject.as_str()) == args.first().and_then(Json::as_str),
        ))),
        (Json::String(subject), "equalsIgnoreCase") => Ok(Some(Json::Bool(
            args.first()
                .and_then(Json::as_str)
                .is_some_and(|other| subject.eq_ignore_ascii_case(other)),
        ))),
        (Json::String(subject), "charAt") => {
            let at = index_of(&arg(0), subject.chars().count())?;
            Ok(Some(
                subject
                    .chars()
                    .nth(at)
                    .map_or(Json::Null, |character| Json::String(character.to_string())),
            ))
        }
        // Every value renders as text, which is what `toString` answers.
        (value, "toString") => Ok(Some(Json::String(rendered(value)))),
        (value, _) => Err(format!("{NO_SUCH_METHOD}{name} on {}", kind_of(value))),
    }
}

/// Whether `name` is a method [`method`] can call on `target` with no arguments, which is what
/// a property reference falls back to: `$list.size` is the `size()` of a list, as Velocity's
/// own property lookup reads it.
#[must_use]
pub fn is_zero_argument_method(target: &Json, name: &str) -> bool {
    matches!(
        (target, name),
        (
            Json::Object(_) | Json::Array(_) | Json::String(_),
            "size" | "isEmpty" | "keySet" | "values" | "entrySet" | "length"
        ) | (Json::String(_), "toUpperCase" | "toLowerCase" | "trim")
    )
}

/// A collection's size as the value a template carries.
fn count(size: usize) -> Json {
    Json::Number(i64::try_from(size).unwrap_or(i64::MAX).into())
}

/// An index a search answers with, or `-1` for the search that found nothing — which is
/// what `indexOf` answers in the language this emulates.
fn count_or_absent(found: Option<usize>) -> Json {
    found.map_or(Json::Number((-1).into()), count)
}

/// One index argument as an offset into a collection of `len` elements.
fn index_of(value: &Json, len: usize) -> Result<usize, String> {
    let Some(index) = value.as_i64() else {
        return Err(format!("{NOT_AN_INDEX} {}", rendered(value)));
    };
    usize::try_from(index)
        .ok()
        .filter(|at| *at <= len)
        .ok_or_else(|| format!("{NOT_AN_INDEX} {index}"))
}

/// What a value IS, as a fault names it.
fn kind_of(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(_) => "a number",
        Json::String(_) => "a string",
        Json::Array(_) => "a list",
        Json::Object(_) => "a map",
    }
}

// The TOKEN IMAGES an accepted-token set is listed under ([`Expected::tokens`]). A named token
// of the Apache Velocity parser grammar is written `<NAME>` under that grammar's own name for
// it; a token that grammar spells inline is written as that text, quoted.
/// `[`, which indexes the expression to its left.
const INDEX_TOKEN: &str = "\"[\"";
/// The `)` and `(` a construct's arguments close and open with.
const CLOSE_PAREN_TOKEN: &str = "<RPAREN>";
/// See [`CLOSE_PAREN_TOKEN`].
const OPEN_PAREN_TOKEN: &str = "<LPAREN>";
/// The space a construct's parts may be written around.
const WHITESPACE_TOKEN: &str = "<WHITESPACE>";
/// The `,` another argument, element or entry follows.
const COMMA_TOKEN: &str = "\",\"";
/// The `]` a list or an index closes with.
const CLOSE_BRACKET_TOKEN: &str = "\"]\"";
/// The `}` a map closes with, and the `:` its key is followed by.
const CLOSE_CURLY_TOKEN: &str = "\"}\"";
/// See [`CLOSE_CURLY_TOKEN`].
const COLON_TOKEN: &str = "\":\"";
/// The `=` a `#set` assigns with.
const ASSIGN_TOKEN: &str = "\"=\"";
/// The name a variable is written under.
const IDENTIFIER_TOKEN: &str = "<IDENTIFIER>";
/// The `in` of a `#foreach`.
const IN_TOKEN: &str = "<IN>";
/// The directives a block is continued or closed by.
const END_TOKEN: &str = "<END>";
/// See [`END_TOKEN`].
const ELSE_TOKEN: &str = "<ELSE>";
/// See [`END_TOKEN`].
const ELSEIF_TOKEN: &str = "<ELSEIF>";
/// The end of the template, which is the token a parse that ran out of text stopped at.
const END_OF_INPUT_TOKEN: &str = "<EOF>";
/// The arithmetic operators an expression may carry on with, which the grammar spells inline.
const ARITHMETIC_TOKENS: [&str; 5] = ["\"-\"", "\"+\"", "\"*\"", "\"/\"", "\"%\""];
/// The logical and relational operators it may carry on with, each a named token.
const LOGICAL_TOKENS: [&str; 8] = [
    "<LOGICAL_AND>",
    "<LOGICAL_OR>",
    "<LOGICAL_LT>",
    "<LOGICAL_LE>",
    "<LOGICAL_GT>",
    "<LOGICAL_GE>",
    "<LOGICAL_EQUALS>",
    "<LOGICAL_NOT_EQUALS>",
];
/// What an OPERAND may begin with: a reference or a word, a literal of each kind the language
/// has, a prefix operator, a grouped expression, a list or a map.
const OPERAND_TOKENS: [&str; 12] = [
    WHITESPACE_TOKEN,
    OPEN_PAREN_TOKEN,
    IDENTIFIER_TOKEN,
    "<STRING_LITERAL>",
    "<INTEGER_LITERAL>",
    "<FLOATING_POINT_LITERAL>",
    "<TRUE>",
    "<FALSE>",
    "<LOGICAL_NOT>",
    "\"-\"",
    INDEX_TOKEN,
    "\"{\"",
];

/// The sentences a template fault states.
pub const UNEXPECTED_DIRECTIVE: &str =
    "This template closes a block it never opened: #end, #else or #elseif with no #if or \
     #foreach.";
pub const NO_END: &str = "This template opens a block it never closes with #end.";
pub const NOT_A_DIRECTIVE: &str = "This directive is not written as ( … ).";
pub const NOT_AN_ASSIGNMENT: &str = "This #set is not written as ( $target = value ).";
pub const NOT_A_LOOP: &str = "This #foreach is not written as ( $element in value ).";
pub const NOT_AN_ARGUMENT_LIST: &str = "This call's arguments are not written as ( … ).";
pub const NOT_AN_EXPRESSION: &str = "This is not an expression.";
pub const TOO_DEEP: &str = "This template nests deeper than this endpoint parses:";
pub const NOT_COMPARABLE: &str = "These two values cannot be compared.";
pub const NOT_ARITHMETIC: &str = "These two values cannot be combined with";
pub const DIVIDE_BY_ZERO: &str = "This expression divides by zero.";
pub const OUTSIDE_THE_STRING: &str = "This substring is outside the string:";
pub const NOT_AN_INDEX: &str = "This is not an index into the value:";
pub const NO_SUCH_METHOD: &str = "No such method: ";

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the language's two constructs are told apart and read as what they are — text,
    /// a reference, and each directive this grammar states — whatever the template spells
    /// around them.
    #[test]
    fn every_construct_of_the_language_is_read_as_what_it_is() {
        /// (case, the template, how many nodes it parses into, the first node's shape)
        type Row<'a> = (&'a str, &'a str, usize, &'a str);
        let rows: &[Row<'_>] = &[
            ("literal text", "{\"a\": 1}", 1, "Text"),
            ("a `$` that opens no reference", "cost is $12", 1, "Text"),
            ("a `#` that opens no directive", "#fff0aa", 1, "Text"),
            ("a reference", "$input.body", 1, "Write"),
            ("a braced reference", "${input.body}", 1, "Write"),
            ("a quiet reference", "$!input.body", 1, "Write"),
            ("a reference with a quoted member", "$a['k:1']", 1, "Write"),
            ("a binding", "#set($a = 1)", 1, "Set"),
            ("the formal spelling of one", "#{set}($a = 1)", 1, "Set"),
            ("a conditional", "#if($a)x#end", 1, "If"),
            ("a conditional with branches", "#if($a)x#elseif($b)y#else z#end", 1, "If"),
            ("a loop", "#foreach($x in $a)y#end", 1, "ForEach"),
            ("an early return", "#return($a)", 1, "Return"),
            ("an early return naming no value", "#return", 1, "Return"),
            ("a comment, which renders nothing", "## a note", 0, ""),
            ("a block comment", "#* a note *#", 0, ""),
            ("an unparsed block", "#[[ $a #if ]]#", 1, "Text"),
            ("a binding then the body it builds", "#set($a = 1)\n{\"a\": $a}", 4, "Set"),
        ];
        for (case, template, count, first) in rows {
            let parsed = parse(template);
            let nodes = parsed
                .as_ref()
                .unwrap_or_else(|why| panic!("case {case:?}: expected a parse, got {why}"));
            assert_eq!(
                nodes.len(),
                *count,
                "case {case:?}: expected {count} node(s), got {nodes:?}"
            );
            let shape = match nodes.first() {
                None => "",
                Some(Node::Text(_)) => "Text",
                Some(Node::Write { .. }) => "Write",
                Some(Node::Set { .. }) => "Set",
                Some(Node::If { .. }) => "If",
                Some(Node::ForEach { .. }) => "ForEach",
                Some(Node::Break) => "Break",
                Some(Node::Stop) => "Stop",
                Some(Node::Return(_)) => "Return",
            };
            assert_eq!(
                shape, *first,
                "case {case:?}: expected a {first} node, got {shape}"
            );
        }
    }

    /// RULE: a `#` that opens no directive of this grammar is TEXT — the whole of it, the word
    /// after it included — because that is what the template renders. A body stating an
    /// expression attribute name (`#title`) depends on it: every character of the name belongs
    /// to the output.
    #[test]
    fn a_hash_that_opens_no_directive_renders_as_the_text_it_is() {
        // The rows span what FOLLOWS the `#`: a word that is no directive of this grammar, a
        // word that merely starts like one, the formal braced spelling of such a word, a
        // character no name can start with, and the name written twice in one body.
        let rows: &[(&str, &str)] = &[
            ("{\"#title\": \"title\"}", "{\"#title\": \"title\"}"),
            ("#sets", "#sets"),
            ("#{title}", "#{title}"),
            ("#fff0aa", "#fff0aa"),
            ("#-tag", "#-tag"),
            (
                "{\"#a\": \"a\", \"#b\": \"b\"}",
                "{\"#a\": \"a\", \"#b\": \"b\"}",
            ),
        ];
        for (case, expected) in rows {
            let parsed = parse(case);
            let nodes = parsed
                .as_ref()
                .unwrap_or_else(|why| panic!("case {case:?}: expected a parse, got {why}"));
            let rendered: String = nodes
                .iter()
                .map(|node| match node {
                    Node::Text(text) => text.clone(),
                    other => panic!("case {case:?}: expected text alone, got {other:?}"),
                })
                .collect();
            assert_eq!(
                rendered, *expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// RULE: a template this grammar cannot read is reported as one, rather than read in
    /// part — including one that nests past the bound this parse is done under.
    #[test]
    fn a_template_this_grammar_cannot_read_is_reported() {
        let rows: &[(&str, &str)] = &[
            ("a block nothing closes", "#if($a)x"),
            ("a close with no block", "x#end"),
            ("a binding that is no assignment", "#set($a)"),
            ("a binding of nothing", "#set($a = )"),
            ("a condition that is no expression", "#if(,)"),
            ("a loop that names no element", "#foreach(x in $a)y#end"),
            ("an unterminated argument", "#if($a.b('x)"),
        ];
        for (case, template) in rows {
            let parsed = parse(template);
            assert!(
                parsed.is_err(),
                "case {case:?}: expected the template to be reported unread, got {parsed:?}"
            );
        }
        // Nested past the bound, built rather than written out: the parse refuses instead of
        // recursing as deeply as the text does.
        let deep = format!(
            "{}x{}",
            "#if(true)".repeat(MAX_NESTING + 2),
            "#end".repeat(MAX_NESTING + 2)
        );
        assert!(
            parse(&deep).is_err(),
            "case \"blocks nested past {MAX_NESTING}\": expected the template to be reported \
             unread"
        );
        let chained = format!("#if({}$a)x#end", "!".repeat(MAX_NESTING + 2));
        assert!(
            parse(&chained).is_err(),
            "case \"a condition nested past {MAX_NESTING}\": expected the template to be \
             reported unread"
        );
    }

    /// RULE: the language's value model decides truth, equality and order as Velocity does:
    /// a boolean by itself, anything else by being there, numbers numerically whatever JSON
    /// shape they arrive in, and strings lexicographically.
    #[test]
    fn the_value_model_decides_truth_equality_and_order() {
        let yes = Json::Bool(true);
        let no = Json::Bool(false);
        let zero = Json::Number(0.into());
        let empty = Json::String(String::new());
        let rows: &[(&str, Option<&Json>, bool)] = &[
            ("a true boolean", Some(&yes), true),
            ("a false boolean", Some(&no), false),
            ("a zero, which is a value", Some(&zero), true),
            ("an empty string, which is a value", Some(&empty), true),
            ("null", Some(&Json::Null), false),
            ("nothing at all", None, false),
        ];
        for (case, value, expected) in rows {
            let actual = truthy(*value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
        let two = Json::Number(2.into());
        let two_point = ::serde_json::json!(2.0);
        assert!(
            equal(Some(&two), Some(&two_point)),
            "case \"a number spelt two ways\": expected them to be equal"
        );
        assert!(
            !equal(Some(&two), None),
            "case \"a number against nothing\": expected them not to be equal"
        );
        let one = Json::Number(1.into());
        assert_eq!(
            ordering(Some(&one), Some(&two)).ok(),
            Some(::std::cmp::Ordering::Less),
            "case \"one against two\": expected Less"
        );
        let alpha = Json::String("alpha".to_string());
        let beta = Json::String("beta".to_string());
        assert_eq!(
            ordering(Some(&beta), Some(&alpha)).ok(),
            Some(::std::cmp::Ordering::Greater),
            "case \"two strings\": expected Greater"
        );
        assert!(
            ordering(Some(&yes), Some(&no)).is_err(),
            "case \"two booleans\": expected them not to be orderable"
        );
        // An integral result stays an integer, so a count renders as `3`.
        assert_eq!(
            rendered(&number(3.0)),
            "3",
            "case \"an integral number\": expected 3"
        );
    }
}
