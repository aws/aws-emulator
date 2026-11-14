// SPDX-License-Identifier: CC0-1.0

//! X-Ray filter expressions: EVALUATING the query language
//! `GetTraceSummaries$FilterExpression` and `Group$FilterExpression` carry.
//!
//! The language is PARSED in exactly one place — [`super::filter_expression`],
//! which owns the grammar, the catalog of fields X-Ray indexes, the bounds on
//! how long and how deeply nested an expression may be, and the wording of every
//! refusal that follows from them. A surface that only STORES an expression
//! (`CreateGroup`, `UpdateGroup`) asks it and nothing else.
//!
//! This module takes the tree that parser produces and makes it ANSWERABLE from
//! the stored segment documents, which needs three things the parser does not
//! decide:
//!
//! * which fields this emulator can derive a value for — a subset of the indexed
//!   catalog, and a field outside it is refused BY NAME rather than quietly
//!   evaluating to false;
//! * whether a comparison's operands can be compared at all — a string field
//!   against a number can never be true for any trace, which is a mistake in the
//!   request wherever it appears, so [`operand_check`] lends that one judgement
//!   back to the storing surfaces;
//! * what one QUERY may cost, which the parser cannot know because it sees one
//!   expression and not the store it will be run over ([`MAX_QUERY_STEPS`]).
//!
//! A filter expression "selects a subset of the traces in the requested time
//! frame" (`smithy_query(query="doc",shape="GetTraceSummaries")`), so it is
//! evaluated against each stored trace here.
//!
//! ## The keywords, and where each one's value comes from
//!
//! Every keyword reads a value this service already derives from the stored
//! segment documents, and the catalog is the `TraceSummary` the reader returns
//! (`smithy_query(query="shape",shape="TraceSummary")`) plus the root-cause
//! shapes it carries:
//!
//! | keyword | type | value |
//! |---|---|---|
//! | `ok`, `error`, `fault`, `throttle` | boolean | the entity classifications in scope ([`Classification`]) |
//! | `partial` | boolean | `TraceSummary$IsPartial` |
//! | `duration`, `responsetime` | number | `TraceSummary$Duration` / `$ResponseTime` |
//! | `http.status` | number | `Http$HttpStatus` |
//! | `http.url`, `http.method`, `http.useragent`, `http.clientip` | string | the other `Http` members |
//! | `user` | string | `TraceUser$UserName` |
//! | `resource.arn` | string | `ResourceARNDetail$ARN` |
//! | `annotation.<key>` | any | `TraceSummary$Annotations` |
//! | `rootcause.{fault,error,responsetime}.service.{name,type}` | string | `FaultRootCauseService$Name` / `$Type` |
//! | `rootcause.{fault,error,responsetime}.service.inferred` | boolean | `FaultRootCauseService$Inferred` |
//! | `rootcause.{fault,error,responsetime}.entity.name` | string | `FaultRootCauseEntity$Name` |
//! | `rootcause.{fault,error,responsetime}.entity.remote` | boolean | `FaultRootCauseEntity$Remote` |
//! | `rootcause.responsetime.entity.coverage` | number | `ResponseTimeRootCauseEntity$Coverage` |
//! | `rootcause.{fault,error,responsetime}.exception.{name,message}` | string | `RootCauseException$Name` / `$Message` |
//!
//! An annotation's key is written either after a dot or as a SUBSCRIPT
//! (`annotation[<key>]`), and both name the one field. A subscript with no
//! comparison after it is the unary predicate: it asks whether that annotation
//! is SET at all, whatever value it holds, so a trace that recorded the key
//! matches and one that never recorded it does not — and negated
//! (`!annotation[<key>]`, `NOT annotation[<key>]`) it asks the opposite.
//!
//! A root-cause keyword is written either as the whole path above or as the
//! class and part followed by a `{ … }` block whose keywords are that part's
//! own members — `rootcause.responsetime.entity { name = "…" }` is
//! `rootcause.responsetime.entity.name = "…"` — so the two forms compile to one
//! field and are evaluated by one rule. A member the part does not carry is
//! refused by name, and a comparison inside a block is type-checked like any
//! other: an entity name against a number can never hold, so it is refused.
//!
//! `service("name"[, "type"])` and `edge("source", "destination")` scope the
//! keywords inside their `{ … }` block to one service's segments, or to one
//! edge's destination segments; a scope keyword without a block asks only
//! whether the trace ran through it. Each names its entities in either of the two
//! spellings the same reference gives — the names alone, or one `id(…)` function
//! per entity (`service(id(name: "…", type: "…"))`) — and both spellings name the
//! same scope.
//!
//! A keyword whose value this emulator does not derive is REFUSED by name,
//! rather than answered with silence: a caller is not told "no traces match" for
//! a question that was never asked.
//!
//! A value a trace carries more than once (an annotation several services
//! recorded, the entities of a root cause) satisfies a comparison when ANY of
//! those values does.
//!
//! ## What one query may cost
//!
//! An expression's own size is bounded by the parser
//! ([`super::filter_expression::MAX_LENGTH`] characters,
//! [`super::filter_expression::MAX_DEPTH`] levels of nesting), which also bounds
//! this module's recursion: the tree holds at most one node per two characters
//! of source, and [`evaluate`] and the recursive `Drop` of the tree walk its
//! height.
//!
//! What those bounds do NOT cap is the PRODUCT one request drives — traces
//! scanned × nodes × segments per node — so [`MAX_QUERY_STEPS`] does, carried
//! through a whole query as a [`QueryBudget`]. Every trace a query reads spends
//! from the same budget, and a query that spends it is answered by the caller
//! with the modelled throttling error rather than from a selection that stopped
//! halfway. Per trace, evaluation visits each node at most once, and the values
//! the nodes read are derived ONCE per trace into [`Facts`] rather than per node.

use std::collections::BTreeMap;

use super::filter_expression::{self, Operand, Operator};
use super::{
    Classification, Outcome, RootCauseKind, RootCauseView, SegmentRecord, SubsegmentRecord,
    TraceRecord,
};

/// The most steps one filtered query may spend across EVERY trace it reads.
///
/// The parser caps one expression; this caps the PRODUCT a single request can
/// drive — the traces it scans, the nodes it evaluates per trace, and the
/// segments each node reads — because capping the factors alone still multiplies
/// them. A step is one node visit or one segment, subsegment or root cause read,
/// counted by [`QueryBudget`]; a query that would spend more is answered with the
/// modelled throttling error (the caller narrows the window or simplifies the
/// expression) rather than from a selection that stopped halfway.
///
/// Sized so that no request within the other bounds reaches it in normal use:
/// the whole trace store ([`super::MAX_TRACES`]) read by a 20-node expression
/// over 10-segment traces spends about 200_000 steps.
pub const MAX_QUERY_STEPS: usize = 2_000_000;

/// What is left of one query's [`MAX_QUERY_STEPS`].
///
/// One budget is created per `GetTraceSummaries` call and threaded through every
/// trace it evaluates, so the bound is on the REQUEST, not on one trace.
#[derive(Debug)]
pub struct QueryBudget {
    remaining: usize,
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryBudget {
    /// A budget of [`MAX_QUERY_STEPS`].
    pub fn new() -> Self {
        Self {
            remaining: MAX_QUERY_STEPS,
        }
    }

    /// Whether the query has spent its budget. The caller answers the request
    /// with the modelled throttling error when it has, because the selection it
    /// would otherwise report is incomplete.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0
    }

    /// Spend `steps`, saturating at nothing left.
    fn spend(&mut self, steps: usize) {
        self.remaining = self.remaining.saturating_sub(steps);
    }
}

/// Why an expression could not be evaluated. The caller renders it as the
/// modelled `InvalidRequestException` (HTTP 400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    /// What is wrong with the expression, naming the part at fault.
    pub message: String,
    /// Whether the fault is a COMPARISON whose operands cannot be compared at
    /// all — a string keyword against a number, say.
    ///
    /// Such an expression can never be true for any trace, so it is a mistake in
    /// the request wherever it appears, and the surfaces that only store an
    /// expression refuse it too ([`operand_check`]). Every other fault this
    /// module reports is about what this evaluator can answer, which is not
    /// those surfaces' business.
    pub operand_mismatch: bool,
}

impl FilterError {
    fn new(detail: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Invalid filter expression: {detail}"),
            operand_mismatch: false,
        }
    }

    /// A comparison whose operands cannot be compared.
    ///
    /// Worded as the SERVICE words it, so the sentence stands on its own rather
    /// than under this module's "Invalid filter expression" preamble.
    fn operands(message: impl std::fmt::Display) -> Self {
        Self {
            message: message.to_string(),
            operand_mismatch: true,
        }
    }
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// An expression compiled into what this module can evaluate.
#[derive(Debug, Clone)]
pub struct FilterExpression {
    root: Node,
    /// Which derived values the expression reads, so a trace derives only
    /// those, and derives each once.
    reads: Reads,
}

/// Which derived values an expression's keywords read. Noted once at compile
/// time, so evaluating a trace never derives a value no keyword asks for.
#[derive(Debug, Clone, Copy, Default)]
struct Reads {
    fault_root_causes: bool,
    error_root_causes: bool,
    response_time_root_causes: bool,
    annotations: bool,
    /// Whether any scope is an `edge(…)`, which resolves a parent entity id to
    /// the service that holds it.
    edges: bool,
}

/// Parse `source` with [`super::filter_expression`] and compile the tree it
/// answers into what this module evaluates.
pub fn parse(source: &str) -> Result<FilterExpression, FilterError> {
    let parsed = filter_expression::parse(source).map_err(|failure| FilterError {
        message: failure.message(),
        operand_mismatch: false,
    })?;
    let root = compile(&parsed, None)?;
    let mut reads = Reads::default();
    note_reads(&root, &mut reads);
    Ok(FilterExpression { root, reads })
}

/// Whether `source` holds a comparison whose operands cannot be compared at all.
///
/// The surfaces that STORE an expression ask this beside the parser: everything
/// else this module can refuse is about what the evaluator can answer rather
/// than about the request — but a comparison between a string keyword and a
/// number (or an ordering operator on a value that has no order) can never be
/// true for any trace, so it is a mistake in the request wherever it appears.
/// `Ok(())` for anything the parser itself refuses: that refusal is the
/// storing surface's own call.
pub fn operand_check(source: &str) -> Result<(), FilterError> {
    match parse(source) {
        Ok(_) => Ok(()),
        Err(error) if error.operand_mismatch => Err(error),
        Err(_) => Ok(()),
    }
}

impl FilterExpression {
    /// Whether `trace` satisfies this expression, spending `budget`.
    ///
    /// The values the expression reads are derived from the trace ONCE, into
    /// [`Facts`], before the tree is walked — so a node costs a lookup rather
    /// than a re-derivation, and the whole evaluation is one pass over the trace
    /// plus one pass over the tree.
    ///
    /// `budget` is the WHOLE query's, so the work a request does over every
    /// trace it reads is bounded, not just the work one trace costs. Once it is
    /// spent, evaluation stops and answers `false`; the caller checks
    /// [`QueryBudget::exhausted`] and answers the request with the modelled
    /// throttling error rather than reporting a selection that stopped halfway.
    pub fn matches(&self, trace: &TraceRecord, budget: &mut QueryBudget) -> bool {
        if budget.exhausted() {
            return false;
        }
        let facts = Facts::of(trace, self.reads, budget);
        evaluate(&self.root, &facts, &Scope::whole(trace.segments.len()), budget)
    }
}

// ---------------------------------------------------------------------------
// Compiling the parsed tree
// ---------------------------------------------------------------------------

/// What this module evaluates: the parsed tree with every predicate resolved to
/// a value it can read and a comparison it can make.
#[derive(Debug, Clone)]
enum Node {
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    /// A keyword compared to a literal, or a boolean keyword on its own (which
    /// compiles to `keyword = true`).
    Compare {
        field: Field,
        op: Operator,
        value: Literal,
    },
    /// `service(…)` / `edge(…)`, with the expression its block scopes.
    Scoped {
        selector: Selector,
        inner: Option<Box<Node>>,
    },
    /// Whether a field is SET at all, whatever value it holds — what the unary
    /// `annotation[<key>]` predicate asks. A key only some of a trace's entities
    /// recorded is set for the trace when any entity IN SCOPE recorded it, which
    /// is how every other keyword reads a value a trace carries more than once.
    Present {
        /// The annotation key, the one field whose presence this module reads.
        key: String,
    },
}

/// A comparison's right-hand value.
#[derive(Debug, Clone)]
enum Literal {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Literal {
    /// The data type this literal is of, as a refusal names it.
    fn kind(&self) -> &'static str {
        match self {
            Self::Str(_) => "String",
            Self::Num(_) => "Number",
            Self::Bool(_) => "Boolean",
        }
    }

    /// The literal a parsed operand carries, or `None` for one naming another
    /// field or an entity — neither is a value a comparison is evaluated against
    /// here.
    fn of(operand: &Operand) -> Option<Self> {
        match operand {
            Operand::Text(text) => Some(Self::Str(text.clone())),
            Operand::Number(number) => Some(Self::Num(*number)),
            Operand::Boolean(value) => Some(Self::Bool(*value)),
            Operand::Field(_) | Operand::Entity(_) => None,
        }
    }
}

/// Which entities a scope block's keywords read.
#[derive(Debug, Clone)]
enum Selector {
    /// `service("name")` / `service("name", "type")`.
    Service {
        name: String,
        r#type: Option<String>,
    },
    /// `edge("source", "destination")`.
    Edge { source: String, destination: String },
}

/// Which root cause's members the keywords of a `{ … }` block address, when the
/// block is a root-cause one: `rootcause.<class>.<part> { … }` scopes its
/// keywords to the members of that part, so `name` inside it is the part's own
/// name and not a keyword of the trace.
#[derive(Debug, Clone, Copy)]
struct RootCauseBlock {
    /// Which of the three root-cause lists the block reads.
    kind: RootCauseKind,
    /// Which member of a cause its keywords name.
    part: RootCauseBlockPart,
}

/// The parts of a root cause a block can address — the three shapes a cause's
/// `Services` carry: the service itself, the entity frames inside it, and the
/// exceptions those frames named.
#[derive(Debug, Clone, Copy)]
enum RootCauseBlockPart {
    Service,
    Entity,
    Exception,
}

/// Compile one parsed node, inside `block` when it sits in a root-cause block.
///
/// The recursion follows the parsed tree's height, which
/// [`super::filter_expression`] bounds.
fn compile(
    node: &filter_expression::Node,
    block: Option<RootCauseBlock>,
) -> Result<Node, FilterError> {
    match node {
        filter_expression::Node::And(left, right) => Ok(Node::And(
            Box::new(compile(left, block)?),
            Box::new(compile(right, block)?),
        )),
        filter_expression::Node::Or(left, right) => Ok(Node::Or(
            Box::new(compile(left, block)?),
            Box::new(compile(right, block)?),
        )),
        filter_expression::Node::Not(inner) => Ok(Node::Not(Box::new(compile(inner, block)?))),
        filter_expression::Node::Exists { path } => compile_exists(path, block),
        filter_expression::Node::Predicate {
            path,
            arguments,
            block: inner,
            comparison,
        } => compile_predicate(path, arguments, inner.as_deref(), comparison.as_ref(), block),
    }
}

/// Compile one predicate: a scope, a comparison, or a bare boolean keyword.
fn compile_predicate(
    path: &str,
    arguments: &[filter_expression::Argument],
    block: Option<&filter_expression::Node>,
    comparison: Option<&(Operator, Operand)>,
    inside: Option<RootCauseBlock>,
) -> Result<Node, FilterError> {
    if inside.is_none() && (path == "service" || path == "edge") {
        let selector = selector_of(path, arguments)?;
        let inner = match block {
            None => None,
            Some(inner) => Some(Box::new(compile(inner, None)?)),
        };
        return Ok(Node::Scoped { selector, inner });
    }
    // `rootcause.<class>.<part> { … }`: the block's keywords are the members of
    // that part of a cause, so the block is compiled with them in scope.
    if let Some(inner) = block {
        if inside.is_none() {
            if let Some(rest) = path.strip_prefix("rootcause.") {
                return compile(inner, Some(root_cause_block(rest)?));
            }
        }
        return Err(unevaluated(format!(
            "`{path}` with a `{{ … }}` block is not evaluated by this emulator yet"
        )));
    }
    if !arguments.is_empty() {
        return Err(unevaluated(format!(
            "`{path}` with arguments is not evaluated by this emulator yet"
        )));
    }
    let field = match inside {
        None => field_of(path)?,
        Some(block) => root_cause_member(block, path)?,
    };
    match comparison {
        Some((op, operand)) => {
            let Some(value) = Literal::of(operand) else {
                return Err(unevaluated(
                    "a comparison between two keywords is not evaluated by this emulator yet",
                ));
            };
            check(path, &field, *op, &value)?;
            Ok(Node::Compare {
                field,
                op: *op,
                value,
            })
        }
        // A boolean keyword stands alone: `fault` is `fault = true`.
        None if field.value_type() == ValueType::Bool => Ok(Node::Compare {
            field,
            op: Operator::Equal,
            value: Literal::Bool(true),
        }),
        None => Err(FilterError::new(format!(
            "`{path}` is not a boolean, so it needs a comparison"
        ))),
    }
}

/// Compile one unary existence predicate: whether the field the subscript named
/// is set at all.
///
/// An annotation is the one field whose VALUE is the submitted document's rather
/// than derived, so "was it recorded" is a question this module can answer from
/// the annotation index; a field whose presence it cannot read is named rather
/// than answered with silence, exactly as an unevaluated comparison is.
fn compile_exists(path: &str, inside: Option<RootCauseBlock>) -> Result<Node, FilterError> {
    let field = match inside {
        None => field_of(path)?,
        Some(block) => root_cause_member(block, path)?,
    };
    match field {
        Field::Annotation(key) => Ok(Node::Present { key }),
        _ => Err(unevaluated(format!(
            "whether `{path}` is set is not evaluated by this emulator yet"
        ))),
    }
}

/// The scope a `service(…)` / `edge(…)` predicate selects.
///
/// Each of the two keywords names its entities in either of the two spellings
/// AWS's filter-expression reference gives: the NAMES alone
/// (`service("name", "type")`, `edge("source", "destination")`) or an `id(…)`
/// function per entity (`service(id(name: "name", type: "type"))`,
/// `edge(id(name: "source"), id(name: "destination"))`). Both spellings name the
/// same scope, so both compile to the one selector this module evaluates.
fn selector_of(
    path: &str,
    arguments: &[filter_expression::Argument],
) -> Result<Selector, FilterError> {
    if arguments
        .iter()
        .any(|argument| matches!(argument.value, Operand::Entity(_)))
    {
        return entity_selector_of(path, arguments);
    }
    let names: Vec<&str> = arguments
        .iter()
        .filter_map(|argument| match &argument.value {
            Operand::Text(text) => Some(text.as_str()),
            Operand::Field(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if names.len() != arguments.len() {
        return Err(unevaluated(format!(
            "`{path}` is evaluated for named services only"
        )));
    }
    match (path, names.as_slice()) {
        ("service", [name]) => Ok(Selector::Service {
            name: (*name).to_string(),
            r#type: None,
        }),
        ("service", [name, r#type]) => Ok(Selector::Service {
            name: (*name).to_string(),
            r#type: Some((*r#type).to_string()),
        }),
        ("edge", [source, destination]) => Ok(Selector::Edge {
            source: (*source).to_string(),
            destination: (*destination).to_string(),
        }),
        ("service", found) => Err(FilterError::new(format!(
            "`service` takes a name and optionally a type, not {} arguments",
            found.len()
        ))),
        (_, found) => Err(FilterError::new(format!(
            "`edge` takes a source and a destination, not {} arguments",
            found.len()
        ))),
    }
}

/// The same scope, where the keyword named its entities with `id(…)` functions.
///
/// One function per entity: a `service(…)` names one and an `edge(…)` names its
/// source and its destination, so an argument list of any other shape is refused
/// in the same words the names spelling is. The entity's NAME is what this module
/// evaluates a scope by, so an `id(…)` that labels no name — or one that narrows
/// by an account, which this module does not derive — is named rather than
/// answered with silence.
fn entity_selector_of(
    path: &str,
    arguments: &[filter_expression::Argument],
) -> Result<Selector, FilterError> {
    let entities: Vec<&filter_expression::EntityId> = arguments
        .iter()
        .filter_map(|argument| match &argument.value {
            Operand::Entity(entity) => Some(entity),
            _ => None,
        })
        .collect();
    if entities.len() != arguments.len() {
        return Err(FilterError::new(format!(
            "`{path}` names its entities either by name or with `id(…)` functions, not both"
        )));
    }
    for entity in &entities {
        if entity.account_id.is_some() {
            return Err(unevaluated(format!(
                "`{path}` narrowed by an entity's account is not evaluated by this emulator yet"
            )));
        }
    }
    let named = |entity: &filter_expression::EntityId| {
        entity.name.clone().ok_or_else(|| {
            unevaluated(format!("`{path}` is evaluated for named services only"))
        })
    };
    match (path, entities.as_slice()) {
        ("service", [entity]) => Ok(Selector::Service {
            name: named(entity)?,
            r#type: entity.r#type.clone(),
        }),
        ("edge", [source, destination]) => Ok(Selector::Edge {
            source: named(source)?,
            destination: named(destination)?,
        }),
        ("service", found) => Err(FilterError::new(format!(
            "`service` takes a name and optionally a type, not {} arguments",
            found.len()
        ))),
        ("edge", found) => Err(FilterError::new(format!(
            "`edge` takes a source and a destination, not {} arguments",
            found.len()
        ))),
        // No other keyword is scoped, so none reaches here; a keyword that one day
        // is reports its own name rather than inheriting another's wording.
        (_, found) => Err(FilterError::new(format!(
            "`{path}` is not scoped by {} entities",
            found.len()
        ))),
    }
}

/// A part of the language this emulator reads but does not evaluate. Named
/// rather than answered with silence, so a caller is not told "no traces match"
/// for a question that was never asked.
fn unevaluated(detail: impl std::fmt::Display) -> FilterError {
    FilterError::new(detail)
}

/// Which derived values `node`'s keywords read. Walked once at compile time, so
/// a trace derives only what the expression actually asks for.
fn note_reads(node: &Node, reads: &mut Reads) {
    match node {
        Node::And(left, right) | Node::Or(left, right) => {
            note_reads(left, reads);
            note_reads(right, reads);
        }
        Node::Not(inner) => note_reads(inner, reads),
        Node::Present { .. } => reads.annotations = true,
        Node::Scoped { selector, inner } => {
            if matches!(selector, Selector::Edge { .. }) {
                reads.edges = true;
            }
            if let Some(inner) = inner {
                note_reads(inner, reads);
            }
        }
        Node::Compare { field, .. } => match field {
            Field::RootCause {
                kind: RootCauseKind::Fault,
                ..
            } => reads.fault_root_causes = true,
            Field::RootCause {
                kind: RootCauseKind::Error,
                ..
            } => reads.error_root_causes = true,
            Field::RootCause {
                kind: RootCauseKind::ResponseTime,
                ..
            } => reads.response_time_root_causes = true,
            Field::Annotation(_) => reads.annotations = true,
            _ => {}
        },
    }
}

// ---------------------------------------------------------------------------
// The fields this module can answer from, and their types
// ---------------------------------------------------------------------------

/// What one keyword reads.
#[derive(Debug, Clone)]
enum Field {
    Flag(Flag),
    Duration,
    ResponseTime,
    HttpStatus,
    Http(HttpField),
    User,
    ResourceArn,
    Annotation(String),
    RootCause {
        kind: RootCauseKind,
        part: RootCausePart,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Ok,
    Error,
    Fault,
    Throttle,
    Partial,
}

#[derive(Debug, Clone, Copy)]
enum HttpField {
    Url,
    Method,
    UserAgent,
    ClientIp,
}

#[derive(Debug, Clone, Copy)]
enum RootCausePart {
    ServiceName,
    ServiceType,
    ServiceInferred,
    EntityName,
    EntityRemote,
    ExceptionName,
    ExceptionMessage,
    /// `ResponseTimeRootCauseEntity$Coverage`: the part of the trace's response
    /// time the frame ran for.
    EntityCoverage,
}

/// What type a keyword's value has, which is what a comparison is checked
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueType {
    Bool,
    Num,
    Str,
    /// An annotation: the submitted value decides, so any literal compares.
    Any,
}

impl Field {
    fn value_type(&self) -> ValueType {
        match self {
            Self::Flag(_) => ValueType::Bool,
            Self::Duration | Self::ResponseTime | Self::HttpStatus => ValueType::Num,
            Self::Http(_) | Self::User | Self::ResourceArn => ValueType::Str,
            Self::Annotation(_) => ValueType::Any,
            Self::RootCause { part, .. } => match part {
                RootCausePart::ServiceInferred | RootCausePart::EntityRemote => ValueType::Bool,
                RootCausePart::EntityCoverage => ValueType::Num,
                _ => ValueType::Str,
            },
        }
    }
}

/// The subset of the indexed catalog this module can answer from.
///
/// The catalog of fields X-Ray INDEXES is [`super::filter_expression`]'s, and it
/// is wider than this one: it is what decides whether an expression is of the
/// language at all. A field the parser accepts and this does not is refused as
/// unevaluated rather than as unknown, so the refusal says which of the two
/// judged it.
fn field_of(keyword: &str) -> Result<Field, FilterError> {
    if let Some(key) = keyword.strip_prefix("annotation.") {
        if key.is_empty() {
            return Err(FilterError::new(
                "`annotation.` needs the name of an annotation",
            ));
        }
        return Ok(Field::Annotation(key.to_string()));
    }
    if let Some(rest) = keyword.strip_prefix("rootcause.") {
        return root_cause_field(rest);
    }
    match keyword {
        "ok" => Ok(Field::Flag(Flag::Ok)),
        "error" => Ok(Field::Flag(Flag::Error)),
        "fault" => Ok(Field::Flag(Flag::Fault)),
        "throttle" => Ok(Field::Flag(Flag::Throttle)),
        "partial" => Ok(Field::Flag(Flag::Partial)),
        "duration" => Ok(Field::Duration),
        "responsetime" => Ok(Field::ResponseTime),
        "http.status" => Ok(Field::HttpStatus),
        "http.url" => Ok(Field::Http(HttpField::Url)),
        "http.method" => Ok(Field::Http(HttpField::Method)),
        "http.useragent" => Ok(Field::Http(HttpField::UserAgent)),
        "http.clientip" => Ok(Field::Http(HttpField::ClientIp)),
        "user" => Ok(Field::User),
        "resource.arn" => Ok(Field::ResourceArn),
        other => Err(unevaluated(format!(
            "`{other}` is an indexed field this emulator does not evaluate yet"
        ))),
    }
}

/// `rootcause.<fault|error|responsetime>.<service|entity|exception>.<member>` —
/// the whole path written out, as a keyword that stands on its own.
fn root_cause_field(rest: &str) -> Result<Field, FilterError> {
    let (class, selector) = rest.split_once('.').ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{rest}` is not a root cause member this emulator evaluates"
        ))
    })?;
    let kind = root_cause_kind(class)?;
    let (part_name, member) = selector.split_once('.').ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{class}.{selector}` is not a root cause member this emulator evaluates"
        ))
    })?;
    let part = root_cause_part(part_name).ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{class}.{part_name}` is not a part of a root cause"
        ))
    })?;
    root_cause_member(RootCauseBlock { kind, part }, member)
}

/// Which of the three root-cause lists a class names.
fn root_cause_kind(class: &str) -> Result<RootCauseKind, FilterError> {
    match class {
        "fault" => Ok(RootCauseKind::Fault),
        "error" => Ok(RootCauseKind::Error),
        "responsetime" => Ok(RootCauseKind::ResponseTime),
        other => Err(unevaluated(format!(
            "`rootcause.{other}` is not a root cause class"
        ))),
    }
}

/// Which part of a cause a path element names.
fn root_cause_part(part: &str) -> Option<RootCauseBlockPart> {
    match part {
        "service" => Some(RootCauseBlockPart::Service),
        "entity" => Some(RootCauseBlockPart::Entity),
        "exception" => Some(RootCauseBlockPart::Exception),
        _ => None,
    }
}

/// The root cause class and part a `rootcause.<class>.<part>` keyword carrying a
/// `{ … }` block addresses.
fn root_cause_block(rest: &str) -> Result<RootCauseBlock, FilterError> {
    let (class, part_name) = rest.split_once('.').ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{rest}` names no part of a root cause to scope a `{{ … }}` block to"
        ))
    })?;
    let kind = root_cause_kind(class)?;
    let part = root_cause_part(part_name).ok_or_else(|| {
        unevaluated(format!(
            "`rootcause.{class}.{part_name}` is not a part of a root cause"
        ))
    })?;
    Ok(RootCauseBlock { kind, part })
}

/// The member `keyword` names inside a root-cause block: the members of the
/// shapes a cause's `Services` carry — `FaultRootCauseService` /
/// `ErrorRootCauseService` / `ResponseTimeRootCauseService`, their entity frames
/// and those frames' exceptions — so a keyword addressing anything else is
/// refused by name rather than evaluated to nothing.
fn root_cause_member(block: RootCauseBlock, keyword: &str) -> Result<Field, FilterError> {
    let part = match (block.part, keyword) {
        (RootCauseBlockPart::Service, "name") => RootCausePart::ServiceName,
        (RootCauseBlockPart::Service, "type") => RootCausePart::ServiceType,
        (RootCauseBlockPart::Service, "inferred") => RootCausePart::ServiceInferred,
        (RootCauseBlockPart::Entity, "name") => RootCausePart::EntityName,
        (RootCauseBlockPart::Entity, "remote") => RootCausePart::EntityRemote,
        // `Coverage` is a member of the response-time entity shape alone, so it
        // reads nothing on a fault or an error cause.
        (RootCauseBlockPart::Entity, "coverage")
            if block.kind == RootCauseKind::ResponseTime =>
        {
            RootCausePart::EntityCoverage
        }
        (RootCauseBlockPart::Exception, "name") => RootCausePart::ExceptionName,
        (RootCauseBlockPart::Exception, "message") => RootCausePart::ExceptionMessage,
        (part, other) => {
            return Err(unevaluated(format!(
                "`{other}` is not a member of a root cause {} this emulator evaluates",
                match part {
                    RootCauseBlockPart::Service => "service",
                    RootCauseBlockPart::Entity => "entity",
                    RootCauseBlockPart::Exception => "exception",
                }
            )))
        }
    };
    Ok(Field::RootCause {
        kind: block.kind,
        part,
    })
}

/// Refuse a comparison whose operands cannot be compared at all.
///
/// X-Ray words this refusal "You can't compare property [<property>] against
/// the <Type> data type.", naming the keyword as the expression wrote it and
/// the data type of the value it was compared to, so that is the sentence
/// [`FilterError::operands`] carries — the one wording for every comparison this
/// language cannot make, whether the types differ outright, the operator orders
/// a value that has no order, or a string operator was given something that is
/// not a string.
fn check(keyword: &str, field: &Field, op: Operator, value: &Literal) -> Result<(), FilterError> {
    let expected = field.value_type();
    let compatible = matches!(
        (expected, value),
        (ValueType::Any, _)
            | (ValueType::Bool, Literal::Bool(_))
            | (ValueType::Num, Literal::Num(_))
            | (ValueType::Str, Literal::Str(_))
    );
    // An ordering operator takes only a value that HAS an order, and a string
    // operator only a string, so each operator class narrows what it can be
    // given beyond the field's own type.
    let ordered_well = !orders(op) || matches!(value, Literal::Num(_));
    let textual_well = !textual(op) || matches!(value, Literal::Str(_));
    if !(compatible && ordered_well && textual_well) {
        return Err(FilterError::operands(format!(
            "You can't compare property [{keyword}] against the {} data type.",
            value.kind()
        )));
    }
    Ok(())
}

/// Whether `op` orders its operands, which only numbers do.
fn orders(op: Operator) -> bool {
    matches!(
        op,
        Operator::Less | Operator::LessOrEqual | Operator::Greater | Operator::GreaterOrEqual
    )
}

/// Whether `op` is one of the string operators, which only strings take.
fn textual(op: Operator) -> bool {
    matches!(
        op,
        Operator::Contains | Operator::BeginsWith | Operator::EndsWith
    )
}

/// Apply an equality or ordering operator to an `Ordering`.
fn ordered(op: Operator, ordering: std::cmp::Ordering) -> bool {
    match op {
        Operator::Equal => ordering.is_eq(),
        Operator::NotEqual => ordering.is_ne(),
        Operator::Less => ordering.is_lt(),
        Operator::LessOrEqual => ordering.is_le(),
        Operator::Greater => ordering.is_gt(),
        Operator::GreaterOrEqual => ordering.is_ge(),
        // The string operators are applied in `matches_str`, which is the only
        // value kind that takes them.
        Operator::Contains | Operator::BeginsWith | Operator::EndsWith => false,
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Everything one trace contributes to an expression, derived ONCE per trace
/// before the tree is walked.
///
/// Without this, a node that reads an entity classification, an annotation or a
/// root cause would re-derive it from the whole segment tree every time it is
/// visited; with it, the per-trace cost is one pass over the trace plus, per
/// node, a lookup over the segments in scope.
struct Facts<'a> {
    trace: &'a TraceRecord,
    /// Per segment, the recorded outcomes of its own entity tree.
    outcomes: Vec<Vec<Outcome>>,
    /// Every annotation the trace carries, by key, with the segment that
    /// recorded it. Empty unless the expression reads an annotation.
    annotations: BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
    /// Every entity id in the trace mapped to the segment that holds it, so an
    /// `edge(…)` scope resolves a `parent_id` by lookup instead of re-scanning
    /// the trace per segment. Empty unless the expression carries an edge.
    entity_owners: BTreeMap<&'a str, usize>,
    /// The trace's own outcome flags, each in the scope the model documents for
    /// it, so a keyword at trace scope answers exactly what `TraceSummary`
    /// reports.
    trace_flags: TraceFlags,
    /// The fault root causes, when the expression reads them.
    fault_causes: Vec<RootCauseView>,
    /// The error root causes, when the expression reads them.
    error_causes: Vec<RootCauseView>,
    /// The response-time root cause, when the expression reads it.
    response_time_causes: Vec<RootCauseView>,
}

impl<'a> Facts<'a> {
    fn of(trace: &'a TraceRecord, reads: Reads, budget: &mut QueryBudget) -> Self {
        let outcomes: Vec<Vec<Outcome>> = trace
            .segments
            .iter()
            .map(SegmentRecord::outcomes)
            .collect();
        // One step per entity read: this is the pass over the trace, so it is
        // what a trace costs before a single node is visited.
        budget.spend(outcomes.iter().map(Vec::len).sum::<usize>().max(1));
        let mut annotations: BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>> =
            BTreeMap::new();
        if reads.annotations {
            for (at, segment) in trace.segments.iter().enumerate() {
                collect_annotations(at, &segment.annotations, &mut annotations);
                walk_annotations(at, &segment.subsegments, &mut annotations);
            }
            budget.spend(annotations.values().map(Vec::len).sum());
        }
        let entity_owners = if reads.edges {
            let owners = trace.entity_owners();
            budget.spend(owners.len());
            owners
        } else {
            BTreeMap::new()
        };
        let fault_causes = if reads.fault_root_causes {
            let causes = trace.root_causes(RootCauseKind::Fault);
            budget.spend(trace.segments.len() + causes.len());
            causes
        } else {
            Vec::new()
        };
        let error_causes = if reads.error_root_causes {
            let causes = trace.root_causes(RootCauseKind::Error);
            budget.spend(trace.segments.len() + causes.len());
            causes
        } else {
            Vec::new()
        };
        let response_time_causes = if reads.response_time_root_causes {
            let causes = trace.root_causes(RootCauseKind::ResponseTime);
            budget.spend(trace.segments.len() + causes.len());
            causes
        } else {
            Vec::new()
        };
        // `HasFault` and `HasError` are the ROOT segment document's and
        // `HasThrottle` is any document's, as `TraceSummary` documents each
        // (see `TraceRecord::root_document_has`), read here off the
        // classifications already derived above.
        let root_at = trace
            .segments
            .iter()
            .position(|segment| segment.parent_id.is_none());
        let root_outcomes: &[Outcome] = root_at.map_or(&[], |at| outcomes[at].as_slice());
        let trace_flags = TraceFlags {
            fault: root_outcomes
                .iter()
                .any(|outcome| outcome.class == Classification::Fault),
            // A 400-series error the root document declared — exactly what
            // `TraceRecord::has_error` reports, so the keyword and the summary
            // cannot disagree.
            error: root_outcomes.iter().any(|outcome| outcome.error_400),
            throttle: outcomes
                .iter()
                .flatten()
                .any(|outcome| outcome.class == Classification::Throttle),
        };
        Self {
            trace,
            outcomes,
            annotations,
            entity_owners,
            trace_flags,
            fault_causes,
            error_causes,
            response_time_causes,
        }
    }

    /// The root causes of `kind`.
    fn root_causes(&self, kind: RootCauseKind) -> &[RootCauseView] {
        match kind {
            RootCauseKind::Fault => &self.fault_causes,
            RootCauseKind::Error => &self.error_causes,
            RootCauseKind::ResponseTime => &self.response_time_causes,
        }
    }
}

/// A trace's own `HasFault` / `HasError` / `HasThrottle`.
#[derive(Debug, Clone, Copy)]
struct TraceFlags {
    fault: bool,
    error: bool,
    throttle: bool,
}

/// One entity's annotations, indexed by key.
fn collect_annotations<'a>(
    at: usize,
    source: &'a BTreeMap<String, serde_json::Value>,
    out: &mut BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
) {
    for (key, value) in source {
        out.entry(key.as_str()).or_default().push((at, value));
    }
}

/// The annotations of a subsegment tree, indexed by key.
///
/// Walked with an explicit worklist rather than by recursion, so the nesting a
/// submitted document carries costs heap and not call stack (the ingest also
/// refuses a document nested deeper than [`super::MAX_SUBSEGMENT_DEPTH`]). The
/// order is the recursion's: a node, then its children left to right, which is
/// the order a key's values are indexed in.
fn walk_annotations<'a>(
    at: usize,
    subsegments: &'a [SubsegmentRecord],
    out: &mut BTreeMap<&'a str, Vec<(usize, &'a serde_json::Value)>>,
) {
    let mut pending: Vec<&'a SubsegmentRecord> = subsegments.iter().rev().collect();
    while let Some(subsegment) = pending.pop() {
        collect_annotations(at, &subsegment.annotations, out);
        pending.extend(subsegment.subsegments.iter().rev());
    }
}

/// Which of a trace's segments the keywords in hand read.
///
/// Carries the selected indices for iteration AND a membership mask, so a check
/// of the form "is this segment in scope" is a lookup rather than a scan of the
/// selection.
struct Scope {
    /// Whether this is the whole trace: `duration`, `responsetime` and the
    /// `http` block are then the trace's own, as `TraceSummary` reports them.
    whole_trace: bool,
    /// The segment indices in scope.
    indices: Vec<usize>,
    /// Membership by segment index.
    member: Vec<bool>,
}

impl Scope {
    /// Every segment of a trace of `segments` segments.
    fn whole(segments: usize) -> Self {
        Self {
            whole_trace: true,
            indices: (0..segments).collect(),
            member: vec![true; segments],
        }
    }

    /// The segments one scope block selected, out of `segments` in the trace.
    fn selected(segments: usize, indices: Vec<usize>) -> Self {
        let mut member = vec![false; segments];
        for at in &indices {
            if let Some(slot) = member.get_mut(*at) {
                *slot = true;
            }
        }
        Self {
            whole_trace: false,
            indices,
            member,
        }
    }

    fn indices(&self) -> &[usize] {
        &self.indices
    }

    fn contains(&self, at: usize) -> bool {
        self.member.get(at).copied().unwrap_or(false)
    }

    fn is_whole_trace(&self) -> bool {
        self.whole_trace
    }
}

/// Walk the tree. The recursion follows the tree's height, which
/// [`super::filter_expression`]'s bounds cap — at `MAX_DEPTH` for nesting and at
/// one node per two characters of `MAX_LENGTH` for a chained `a OR b OR c …` —
/// so this cannot run off the stack. Every node visit spends from `budget`, so
/// the work one query does over the traces it reads is bounded too.
fn evaluate(node: &Node, facts: &Facts<'_>, scope: &Scope, budget: &mut QueryBudget) -> bool {
    if budget.exhausted() {
        return false;
    }
    budget.spend(1);
    match node {
        Node::And(left, right) => {
            evaluate(left, facts, scope, budget) && evaluate(right, facts, scope, budget)
        }
        Node::Or(left, right) => {
            evaluate(left, facts, scope, budget) || evaluate(right, facts, scope, budget)
        }
        Node::Not(inner) => !evaluate(inner, facts, scope, budget),
        Node::Compare { field, op, value } => {
            budget.spend(scope.indices().len());
            compare(field, *op, value, facts, scope)
        }
        Node::Present { key } => {
            budget.spend(scope.indices().len());
            facts
                .annotations
                .get(key.as_str())
                .is_some_and(|recorded| {
                    recorded.iter().any(|(at, _)| scope.contains(*at))
                })
        }
        Node::Scoped { selector, inner } => {
            budget.spend(facts.trace.segments.len());
            let selected = select(selector, facts);
            if selected.is_empty() {
                return false;
            }
            match inner {
                None => true,
                Some(inner) => evaluate(
                    inner,
                    facts,
                    &Scope::selected(facts.trace.segments.len(), selected),
                    budget,
                ),
            }
        }
    }
}

/// The segments a scope selector names.
///
/// An `edge(…)` resolves each candidate's `parent_id` through the entity index
/// [`Facts`] built once for the trace, so the selection is one pass over the
/// segments rather than a scan of the trace per segment.
fn select(selector: &Selector, facts: &Facts<'_>) -> Vec<usize> {
    let trace = facts.trace;
    match selector {
        Selector::Service { name, r#type } => trace
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| {
                // The type a `service(name, type)` selector names is the type
                // the service is REPORTED with, which for a service this trace
                // only called is the one the call inferred
                // (`SegmentRecord::service_type`) rather than an `origin` it
                // never carried.
                &segment.name == name
                    && r#type.as_deref().is_none_or(|wanted| {
                        segment.service_type().as_deref() == Some(wanted)
                    })
            })
            .map(|(at, _)| at)
            .collect(),
        Selector::Edge {
            source,
            destination,
        } => trace
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| {
                &segment.name == destination
                    && segment.parent_id.as_deref().is_some_and(|parent| {
                        facts
                            .entity_owners
                            .get(parent)
                            .is_some_and(|owner| &trace.segments[*owner].name == source)
                    })
            })
            .map(|(at, _)| at)
            .collect(),
    }
}

/// Whether any value this keyword reads in `scope` satisfies the comparison.
fn compare(
    field: &Field,
    op: Operator,
    value: &Literal,
    facts: &Facts<'_>,
    scope: &Scope,
) -> bool {
    let trace = facts.trace;
    let segments = || scope.indices().iter().map(|at| &trace.segments[*at]);
    match field {
        Field::Flag(Flag::Partial) => {
            let partial = segments().any(|segment| segment.in_progress);
            matches_bool(partial, op, value)
        }
        Field::Flag(flag) => {
            // At trace scope a flag is exactly what `TraceSummary` reports, so
            // the keyword and the summary cannot disagree; inside a scope block
            // it is the outcome the scoped entities recorded.
            if scope.is_whole_trace() {
                let held = match flag {
                    Flag::Fault => facts.trace_flags.fault,
                    Flag::Error => facts.trace_flags.error,
                    Flag::Throttle => facts.trace_flags.throttle,
                    // `ok` is the absence of all three anywhere in the trace,
                    // and `partial` is answered by the arm above.
                    Flag::Ok => {
                        let mut outcomes = facts.outcomes.iter().flatten().peekable();
                        outcomes.peek().is_some()
                            && outcomes.all(|outcome| outcome.class == Classification::Ok)
                    }
                    Flag::Partial => false,
                };
                return matches_bool(held, op, value);
            }
            let mut outcomes = scope
                .indices()
                .iter()
                .flat_map(|at| facts.outcomes[*at].iter().copied())
                .peekable();
            let held = match flag {
                Flag::Ok => {
                    outcomes.peek().is_some()
                        && outcomes.all(|outcome| outcome.class == Classification::Ok)
                }
                // As at trace scope: a 400-series error, the 429 of a throttled
                // request included.
                Flag::Error => outcomes.any(|outcome| outcome.error_400),
                Flag::Fault => outcomes.any(|outcome| outcome.class == Classification::Fault),
                Flag::Throttle => {
                    outcomes.any(|outcome| outcome.class == Classification::Throttle)
                }
                Flag::Partial => false,
            };
            matches_bool(held, op, value)
        }
        Field::Duration => {
            let duration = if scope.is_whole_trace() {
                trace.duration()
            } else {
                scoped_duration(trace, scope)
            };
            duration.is_some_and(|duration| matches_num(duration, op, value))
        }
        Field::ResponseTime => {
            let response_time = if scope.is_whole_trace() {
                trace.response_time()
            } else {
                segments().find_map(|segment| Some(segment.end_time? - segment.start_time))
            };
            response_time.is_some_and(|value_seconds| matches_num(value_seconds, op, value))
        }
        Field::HttpStatus => http_segments(trace, scope)
            .iter()
            .filter_map(|segment| segment.http_status())
            .any(|status| matches_num(f64::from(status), op, value)),
        Field::Http(member) => http_segments(trace, scope)
            .iter()
            .filter_map(|segment| {
                let http = segment.http.as_ref()?;
                match member {
                    HttpField::Url => http.url.as_deref(),
                    HttpField::Method => http.http_method.as_deref(),
                    HttpField::UserAgent => http.user_agent.as_deref(),
                    HttpField::ClientIp => http.client_ip.as_deref(),
                }
            })
            .any(|found| matches_str(found, op, value)),
        Field::User => segments()
            .filter_map(|segment| segment.user.as_deref())
            .any(|found| matches_str(found, op, value)),
        Field::ResourceArn => segments()
            .filter_map(|segment| segment.resource_arn.as_deref())
            .any(|found| matches_str(found, op, value)),
        Field::Annotation(key) => facts
            .annotations
            .get(key.as_str())
            .is_some_and(|recorded| {
                recorded
                    .iter()
                    .filter(|(at, _)| scope.contains(*at))
                    .any(|(_, found)| matches_json(found, op, value))
            }),
        Field::RootCause { kind, part } => {
            root_cause_matches(facts.root_causes(*kind), *part, op, value)
        }
    }
}

/// The segments whose `http` block a keyword reads: the trace's own `Http` is
/// its root segment's, as `TraceSummary$Http` reports it, and a scope block
/// reads the scoped segments'.
fn http_segments<'a>(trace: &'a TraceRecord, scope: &Scope) -> Vec<&'a SegmentRecord> {
    if scope.is_whole_trace() {
        return trace.root().into_iter().collect();
    }
    scope
        .indices()
        .iter()
        .map(|at| &trace.segments[*at])
        .collect()
}

/// The span the scoped segments cover.
fn scoped_duration(trace: &TraceRecord, scope: &Scope) -> Option<f64> {
    let mut first = f64::INFINITY;
    let mut last = f64::NEG_INFINITY;
    for at in scope.indices() {
        let segment = &trace.segments[*at];
        first = first.min(segment.start_time);
        last = last.max(segment.end_time.unwrap_or(segment.start_time));
    }
    (first.is_finite() && last.is_finite()).then_some(last - first)
}

/// Whether any service, entity or exception of `causes` satisfies the
/// comparison.
fn root_cause_matches(
    causes: &[RootCauseView],
    part: RootCausePart,
    op: Operator,
    value: &Literal,
) -> bool {
    causes.iter().any(|cause| {
        cause.services.iter().any(|service| match part {
            RootCausePart::ServiceName => matches_str(&service.name, op, value),
            RootCausePart::ServiceType => service
                .r#type
                .as_ref()
                .is_some_and(|found| matches_str(found, op, value)),
            RootCausePart::ServiceInferred => matches_bool(service.inferred, op, value),
            RootCausePart::EntityName => service
                .entity_path
                .iter()
                .any(|entity| matches_str(&entity.name, op, value)),
            RootCausePart::EntityRemote => service
                .entity_path
                .iter()
                .any(|entity| matches_bool(entity.remote, op, value)),
            RootCausePart::EntityCoverage => service.entity_path.iter().any(|entity| {
                entity
                    .coverage
                    .is_some_and(|coverage| matches_num(coverage, op, value))
            }),
            RootCausePart::ExceptionName => service.entity_path.iter().any(|entity| {
                entity.exceptions.iter().any(|exception| {
                    exception
                        .name
                        .as_ref()
                        .is_some_and(|found| matches_str(found, op, value))
                })
            }),
            RootCausePart::ExceptionMessage => service.entity_path.iter().any(|entity| {
                entity.exceptions.iter().any(|exception| {
                    exception
                        .message
                        .as_ref()
                        .is_some_and(|found| matches_str(found, op, value))
                })
            }),
        })
    })
}

fn matches_bool(found: bool, op: Operator, value: &Literal) -> bool {
    match value {
        Literal::Bool(wanted) => ordered(op, found.cmp(wanted)),
        _ => false,
    }
}

fn matches_num(found: f64, op: Operator, value: &Literal) -> bool {
    match value {
        Literal::Num(wanted) => found
            .partial_cmp(wanted)
            .is_some_and(|ordering| ordered(op, ordering)),
        _ => false,
    }
}

/// A string comparison, including the string operators the language carries
/// beside the six orderings.
fn matches_str(found: &str, op: Operator, value: &Literal) -> bool {
    let Literal::Str(wanted) = value else {
        return false;
    };
    match op {
        Operator::Contains => found.contains(wanted.as_str()),
        Operator::BeginsWith => found.starts_with(wanted.as_str()),
        Operator::EndsWith => found.ends_with(wanted.as_str()),
        other => ordered(other, found.cmp(wanted.as_str())),
    }
}

/// An annotation compares by the type the document stored it as.
fn matches_json(found: &serde_json::Value, op: Operator, value: &Literal) -> bool {
    match found {
        serde_json::Value::String(text) => matches_str(text, op, value),
        serde_json::Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| matches_num(number, op, value)),
        serde_json::Value::Bool(flag) => matches_bool(*flag, op, value),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace of one complete root segment named `svc`, built through the same
    /// parser `PutTraceSegments` ingests with.
    fn trace() -> TraceRecord {
        let document = r#"{"trace_id":"1-58406520-a006649127e371903a2de979","id":"0000000000000001","name":"svc","start_time":1.0,"end_time":2.0}"#;
        let (trace_id, segment) =
            super::super::parse_segment_document(document).expect("the document is well formed");
        let mut record = TraceRecord {
            id: trace_id,
            first_start: segment.start_time,
            last_end: segment.end_time.unwrap_or(segment.start_time),
            ..TraceRecord::default()
        };
        record.segments.push(segment);
        record
    }

    /// The budget is the WHOLE query's, so a query that has spent it stops
    /// evaluating instead of reading further traces — which is what lets the
    /// handler refuse the request rather than report half a selection.
    #[test]
    fn a_query_that_has_spent_its_budget_stops_evaluating() {
        let expression = parse("service(\"svc\")").expect("the expression is readable");
        let trace = trace();

        let mut budget = QueryBudget::new();
        assert!(
            expression.matches(&trace, &mut budget),
            "the expression selects the trace it matches"
        );
        assert!(
            !budget.exhausted(),
            "one small trace must not spend a whole query's budget"
        );

        budget.spend(MAX_QUERY_STEPS);
        assert!(
            budget.exhausted(),
            "spending the whole budget leaves it exhausted"
        );
        assert!(
            !expression.matches(&trace, &mut budget),
            "a spent budget stops evaluation rather than reading the trace again"
        );
    }

    /// Every trace a query reads spends from the same budget, so the bound is on
    /// the request and not on one trace.
    #[test]
    fn each_trace_a_query_reads_spends_from_the_one_budget() {
        let expression =
            parse("annotation.stage = \"beta\" OR fault OR service(\"svc\")").expect("readable");
        let trace = trace();
        let mut budget = QueryBudget::new();
        expression.matches(&trace, &mut budget);
        let after_one = budget.remaining;
        expression.matches(&trace, &mut budget);
        assert!(
            budget.remaining < after_one,
            "the second trace spends further from the same budget: {} then {}",
            after_one,
            budget.remaining
        );
    }

    /// `AND` binds tighter than `OR`, which the one parser's fold decides and
    /// this module inherits: `fault OR ok AND partial` is
    /// `fault OR (ok AND partial)`, so a trace that is merely `ok` does not
    /// match.
    #[test]
    fn and_binds_tighter_than_or() {
        let trace = trace();
        let mut budget = QueryBudget::new();
        let expression = parse("fault OR ok AND partial").expect("the expression is readable");
        assert!(
            !expression.matches(&trace, &mut budget),
            "the trace is ok but not partial, so the conjunction fails and nothing else matches"
        );
        let grouped = parse("(fault OR ok) AND partial").expect("the expression is readable");
        assert!(
            !grouped.matches(&trace, &mut budget),
            "the grouped form also fails, for the same `partial`"
        );
        let satisfied = parse("fault OR ok AND NOT partial").expect("the expression is readable");
        assert!(
            satisfied.matches(&trace, &mut budget),
            "the trace is ok and not partial, so the conjunction holds"
        );
    }
}
