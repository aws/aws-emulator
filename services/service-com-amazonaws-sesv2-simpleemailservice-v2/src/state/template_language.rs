// SPDX-License-Identifier: CC0-1.0

//! The template language a SAVED email template is written in, and how a template
//! written in it is evaluated against one set of rendering data.
//!
//! The RULE this module implements is the one the model draws between the two kinds of
//! template an email may be built from. `Template$TemplateContent` says Amazon SES
//! "supports only simple substitions when you send email using the `SendEmail` or
//! `SendBulkEmail` operations and you provide the full template content in the request" —
//! so the restriction to plain substitution is a property of content supplied INLINE in a
//! send ([`crate::state::template_rendering`] is that substitution), and a template SAVED
//! in the account carries the full language: expressions, conditional blocks, iteration
//! blocks and comments. This module is the evaluation of that language, which is what
//! `TestRenderEmailTemplate` previews ("creates a preview of the MIME content of an email
//! when provided with a template and a set of replacement data").
//!
//! The language is NOT re-implemented here, and what a template MEANS is not read here
//! either: the `handlebars` crate parses a template into its own tree and evaluates that
//! tree, and everything this module decides about what a template does it reads OFF that
//! tree. No construct is recognised by scanning the template text, so no spelling of one —
//! a tag written to omit whitespace, a comment carrying what looks like a tag, a string
//! literal carrying what looks like a tag's close — can mean one thing to this module and
//! another to the engine. The single exception is the two counts below, which are not a
//! reading of the language at all: they bound the engine's own parse, which necessarily
//! happens before there is a tree to read.
//!
//! Two further rules the evaluation follows, both of them the engine's own settings
//! rather than anything computed here:
//!
//! * Every attribute a template's expressions name has to be stated by the rendering
//!   data: the engine runs in its STRICT mode, so a reference to an attribute the data
//!   states no value for produces [`EvaluationFailure::MissingAttribute`] rather than
//!   rendering as nothing. That is the one condition this surface is recorded refusing a
//!   render for, and it is why a preview is worth asking for. A CONDITIONAL block's test
//!   is the exception the engine makes, and it is the right one: `{{#if x}}` and
//!   `{{#unless x}}` ask whether the data states a value for `x` at all, so an attribute
//!   it omits selects the other branch instead of failing the render — which is what a
//!   conditional over optional data is for. Every other reference, an iteration's
//!   included, is a value the data has to state.
//! * The HTML part's substituted values are HTML-escaped and the subject line's and the
//!   text part's are not: the subject line and the text body are not HTML, so a value
//!   carrying an HTML metacharacter belongs in them as the data wrote it. A part that
//!   wants a value in HTML as written says so with the language's unescaped spelling
//!   (`{{{name}}}` or `{{&name}}`). This is the same rule the inline-substitution module
//!   follows, so one template renders the same way whichever of the two read it.
//!
//! # What bounds it
//!
//! Both the template and the data are a caller's, so evaluating one is work a caller
//! sizes, and the engine offers no step hook of its own. Every input is bounded before it
//! gets here — a saved template is at most [`crate::state::MAX_EMAIL_TEMPLATE_BYTES`] and
//! `EmailTemplateData` at most 262144 bytes in the model — and a render happens in three
//! stages, each of them bounded:
//!
//! 1. The PARSE is the engine's, and two properties of the TEXT are counted before it,
//!    because a parse is what they bound and there is no tree to read until it has run.
//!    Both are counts of a character sequence, so text that merely looks like a construct
//!    is counted too, which can only refuse a template rather than admit one:
//!    * the tags it writes, against [`MAX_TAGS`]. The engine's parse costs more than
//!      linearly in the number of tags it reads, so this is what keeps a parse short
//!      rather than minutes long, and it bounds the nesting of every construct too, since
//!      each level of one needs a tag of its own.
//!    * the parentheses it carries, against [`MAX_PARENTHESES`]. Nesting blocks is bounded
//!      by the engine itself — a template nesting them past what it will parse is answered
//!      with a parse error ([`EvaluationFailure::NotAValidTemplate`]) rather than by
//!      exhausting a stack — but a subexpression, the parenthesised form of a helper call,
//!      nests its parse with nothing stopping it, and each level of that opens one
//!      parenthesis.
//! 2. What the parsed template IS is then read off the engine's tree, ITERATIVELY over an
//!    explicit stack: how deeply its blocks nest, how deeply its ITERATING blocks nest,
//!    how many blocks it has, and whether it carries a construct this module does not
//!    evaluate. Those are exact rather than estimated, and three ceilings are applied
//!    before the tree is evaluated at all:
//!    * off the parse tree, before anything is evaluated: how deeply blocks nest (the
//!      evaluation recurses once per level), whether the template carries a construct this
//!      module does not evaluate, and what the evaluation would COST — the elements of
//!      every body, each counted once per time that body can be evaluated, against
//!      [`MAX_EVALUATIONS`]. That is the product of the two factors rather than either
//!      alone, and a body with no elements still counts, because a block whose body writes
//!      nothing costs time and nothing else.
//!    * a partial, a decorator, or any construct the walk does not recognise is refused
//!      ([`EvaluationFailure::ConstructNotEvaluated`]) — a partial is the one construct
//!      that expands a template inside a template, so two partials of one template can
//!      name each other and never finish, and refusing them is what leaves the evaluation
//!      a finite walk.
//! 3. The EVALUATION writes into a writer that stops at
//!    [`crate::state::template_rendering::MAX_RENDERED_PART_BYTES`], which bounds both what
//!    one part may produce — the one way a template amplifies its input, by naming the
//!    same attribute many times — and the cost of every block body that writes anything,
//!    since a body's own text is written through the same writer.
//!
//! All three stages run on a thread of this module's own, with a stack of its own, and the
//! request waits [`RENDER_DEADLINE`] for them and no longer: that wait is the bound that
//! does not depend on anything here having foreseen what the template does. A render still
//! going when it passes is given up on and the caller answered without it; such a render
//! keeps its permit until its thread ends, and at most [`MAX_RENDERS_IN_FLIGHT`] renders
//! run at once, so abandoned renders cannot accumulate — past that, a render is refused
//! rather than started.
//!
//! The engine runs with NO feature of its crate enabled beyond the default: no scripting
//! language (`script_helper`, `rhai`), no directory template source (`dir_source`,
//! `walkdir`) and no embedded one (`rust-embed`), so a template reaches no host file and
//! no second interpreter. Nothing is registered on it either — a template that calls a
//! helper the language does not define is refused rather than served — so the language a
//! caller's template is evaluated in is exactly the one the engine defines.

use ::std::sync::atomic::{AtomicUsize, Ordering};
use ::std::time::Duration;

use ::serde_json::Value;

use crate::state::template_rendering::{RenderedMessage, MAX_RENDERED_PART_BYTES};

/// How deeply a template may nest its blocks.
///
/// An EMULATOR ceiling, not an AWS quota: the evaluation of a block's body is a nested
/// evaluation, so nesting is the one property of a caller's template that becomes depth
/// inside the engine's render. Sixty-four is far past what a message's own structure
/// needs, and the nesting counted against it is the nesting of the engine's own parse
/// tree, so no spelling of a block tag can hide a level from it. A chained `else if`
/// nests one level per link, which is how the engine's tree holds it.
pub const MAX_TEMPLATE_NESTING: usize = 64;

/// How many tags one part may write.
///
/// An EMULATOR ceiling, and what keeps the engine's PARSE short: the parse costs more
/// than linearly in the number of tags it reads — a thousand of them parse in a fraction
/// of a second, tens of thousands would take minutes — and a saved template is only
/// size-bounded ([`crate::state::MAX_EMAIL_TEMPLATE_BYTES`]), so a part that writes more
/// tags than this is refused before the parse begins. It bounds the nesting of every
/// construct as well, since a level of nesting needs a tag of its own. Counted as the
/// occurrences of a tag's opening braces in the text, so text that merely looks like a
/// tag is counted too; a thousand is far past the number of values any one message
/// substitutes.
pub const MAX_TAGS: usize = 1024;

/// How many parentheses one part may carry.
///
/// An EMULATOR ceiling, and the one bound on the engine's PARSE this module has to supply
/// itself: the engine stops a template that nests BLOCKS too deeply, but a subexpression —
/// the parenthesised form of a helper call — nests its parse with nothing stopping it, and
/// a template is only size-bounded. Every level of that nesting opens one parenthesis, so
/// the number of parentheses in a part bounds the depth its parse can reach; they are
/// counted over the whole text, so a parenthesis in the message's own words is counted
/// too, which can only refuse a template rather than admit one. Two hundred and fifty-six
/// is far past the nesting a message's own structure needs and far below what
/// [`RENDER_STACK_BYTES`] holds.
pub const MAX_PARENTHESES: usize = 256;
/// How many elements of a template one part may evaluate.
///
/// The ceiling that bounds the WORK a render costs rather than the message it produces,
/// and it bounds the PRODUCT of the two things that make a render expensive rather than
/// either alone: how many times a body is evaluated (once per value of whatever the
/// blocks around it iterate over) and how many elements that body holds each time. The
/// output ceiling cannot stand in for it, because an element that resolves to nothing
/// writes nothing; a body with no elements at all still counts as one evaluation, because
/// evaluating it costs something too. A million element evaluations is far past any
/// message a template of the permitted size describes, and takes a fraction of the
/// deadline below; a part that would need more produces nothing at all.
pub const MAX_EVALUATIONS: u64 = 1_000_000;

/// How long the request waits for a render.
///
/// The bound that does not depend on anything here having foreseen what the template
/// does: whatever it turns out to be, the caller is answered within this of asking.
/// CHOSEN, like every other ceiling in this module — far longer than the bounded work
/// above can take, so it is reached only by something the bounds did not foresee.
pub const RENDER_DEADLINE: Duration = Duration::from_secs(2);

/// How many renders may be under way at once.
///
/// A render that outlives [`RENDER_DEADLINE`] is given up on but cannot be stopped — the
/// engine has no interrupt — so it keeps its permit until its own thread ends. This is
/// therefore what stops abandoned renders accumulating: past it, a render is refused
/// rather than started, which is the modelled too-many-requests answer on this surface.
pub const MAX_RENDERS_IN_FLIGHT: usize = 32;

/// The stack the parse and the evaluation of one template run on.
///
/// Its own, rather than the serving task's: the parse recurses once per level of
/// subexpression nesting, which [`MAX_PARENTHESES`] bounds, and the evaluation recurses
/// once per level of block nesting, which [`MAX_TEMPLATE_NESTING`] bounds. This holds many
/// times the deepest either ceiling admits, and only the pages a render touches are ever
/// committed.
const RENDER_STACK_BYTES: usize = 32 * 1024 * 1024;

/// The name the one template of a render is registered under.
///
/// A registry is built for one part and dropped with it, so there is only ever one.
const RENDERED: &str = "rendered";

/// What a tag of the language opens with, whatever the tag turns out to be: counting
/// these is what [`MAX_TAGS`] bounds.
const TAG_OPEN: &str = "{{";

/// How many renders are under way, which [`MAX_RENDERS_IN_FLIGHT`] bounds.
static RENDERS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Why evaluating a saved template produced no message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationFailure {
    /// An expression names an attribute the rendering data states no value for, and this
    /// is its name as the template wrote it.
    MissingAttribute(String),
    /// The template is not written in the language: a block that never closes, an
    /// expression that names nothing, a helper the language does not define, or nesting
    /// past what the engine's own parse accepts. The text is what the engine said about
    /// it, so the refusal names the construct rather than only the template.
    NotAValidTemplate(String),
    /// The template carries a construct this emulator does not evaluate: a partial, which
    /// expands a template inside a template, a decorator, or anything the walk over the
    /// engine's tree does not recognise.
    ConstructNotEvaluated,
    /// A part would evaluate to more than [`MAX_RENDERED_PART_BYTES`].
    TooLarge,
    /// A part nests blocks deeper than [`MAX_TEMPLATE_NESTING`], or carries more
    /// parentheses than [`MAX_PARENTHESES`].
    NestedTooDeeply,
    /// A part writes more tags than [`MAX_TAGS`].
    TooManyTags,
    /// A part's blocks, over the data it is evaluated against, could evaluate more than
    /// [`MAX_EVALUATIONS`] elements.
    TooMuchWork,
    /// The render was still going [`RENDER_DEADLINE`] after it started.
    OutOfTime,
    /// [`MAX_RENDERS_IN_FLIGHT`] renders are already under way.
    TooManyInFlight,
}

/// `content` evaluated against `variables`, part by part.
///
/// A part the template does not carry is absent from the result too — a template with no
/// HTML part renders no HTML part — because the parts of the message are the parts of the
/// template.
///
/// The parse and the evaluation happen on a thread of their own
/// ([`RENDER_STACK_BYTES`]), and this waits [`RENDER_DEADLINE`] for their answer: see this
/// module's bounds. The serving task is never blocked on the engine, so a render that does
/// not come back cannot hold it.
pub async fn evaluate(
    content: &crate::ops::types::EmailTemplateContent,
    variables: &Value,
) -> Result<RenderedMessage, EvaluationFailure> {
    let Some(permit) = Permit::take() else {
        return Err(EvaluationFailure::TooManyInFlight);
    };
    // The thread evaluates copies, so nothing it holds outlives it if it is abandoned.
    let content = content.clone();
    let variables = variables.clone();
    let (answer, wait) = ::tokio::sync::oneshot::channel();
    let started = ::std::thread::Builder::new()
        .name("sesv2-template-render".to_string())
        .stack_size(RENDER_STACK_BYTES)
        .spawn(move || {
            let outcome = evaluate_parts(&content, &variables);
            // The permit goes back when this thread ENDS, whether or not anybody is still
            // waiting for what it produced.
            drop(permit);
            // A caller that stopped waiting dropped the other end; the answer is simply
            // dropped with it.
            let _ = answer.send(outcome);
        });
    if started.is_err() {
        // The host would not start another thread, so this render cannot run now. The
        // permit was moved into the closure that was never spawned and is released with
        // it.
        return Err(EvaluationFailure::TooManyInFlight);
    }
    match ::tokio::time::timeout(RENDER_DEADLINE, wait).await {
        Ok(Ok(outcome)) => outcome,
        // The thread ended without sending an answer, or the wait ran out: either way
        // this request has no render to report.
        Ok(Err(_)) | Err(_) => Err(EvaluationFailure::OutOfTime),
    }
}

/// One render's place among the [`MAX_RENDERS_IN_FLIGHT`] allowed at once.
///
/// Held by the thread that evaluates, and released when that thread ends — not when the
/// request stops waiting for it — so a render given up on still counts for as long as it
/// really lasts.
#[derive(Debug)]
struct Permit;

impl Permit {
    /// A permit, or `None` when every one of them is taken.
    fn take() -> Option<Self> {
        let mut in_flight = RENDERS_IN_FLIGHT.load(Ordering::SeqCst);
        loop {
            if in_flight >= MAX_RENDERS_IN_FLIGHT {
                return None;
            }
            match RENDERS_IN_FLIGHT.compare_exchange_weak(
                in_flight,
                in_flight + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Self),
                Err(seen) => in_flight = seen,
            }
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        RENDERS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The three parts of `content`, evaluated against `variables`.
fn evaluate_parts(
    content: &crate::ops::types::EmailTemplateContent,
    variables: &Value,
) -> Result<RenderedMessage, EvaluationFailure> {
    Ok(RenderedMessage {
        subject: evaluate_part(content.subject.as_deref(), variables, Escape::None)?,
        text: evaluate_part(content.text.as_deref(), variables, Escape::None)?,
        html: evaluate_part(content.html.as_deref(), variables, Escape::Html)?,
    })
}

/// How a substituted value is written into the part it goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// As the data wrote it.
    None,
    /// With the HTML metacharacters replaced by their entities.
    Html,
}

/// One optional part, evaluated.
fn evaluate_part(
    part: Option<&str>,
    variables: &Value,
    escape: Escape,
) -> Result<Option<String>, EvaluationFailure> {
    match part {
        None => Ok(None),
        Some(source) => evaluate_one(source, variables, escape).map(Some),
    }
}

/// `source` parsed, measured against this module's ceilings, and then evaluated against
/// `variables` into a part of at most [`MAX_RENDERED_PART_BYTES`].
///
/// The template the ceilings are read from is the very one that is evaluated: it is parsed
/// ONCE here and handed to the registry, so nothing can be true of the text the ceilings
/// saw and false of the tree the engine evaluates.
fn evaluate_one(
    source: &str,
    variables: &Value,
    escape: Escape,
) -> Result<String, EvaluationFailure> {
    // Before the parse, because the parse is what these bound: see [`MAX_TAGS`] and
    // [`MAX_PARENTHESES`].
    if source.matches(TAG_OPEN).count() > MAX_TAGS {
        return Err(EvaluationFailure::TooManyTags);
    }
    if source.bytes().filter(|byte| *byte == b'(').count() > MAX_PARENTHESES {
        return Err(EvaluationFailure::NestedTooDeeply);
    }
    let template = ::handlebars::Template::compile(source).map_err(|error| {
        // The engine's own rendering of a parse error quotes the offending line and
        // underlines it over several lines, which is a developer's diagnostic rather than
        // an error message: the REASON alone is the one line of it that says what is
        // wrong with the template. Nesting past what the engine parses arrives here too.
        EvaluationFailure::NotAValidTemplate(error.reason().to_string())
    })?;

    let cost = EvaluationCost::of(&template, widest_container(variables))?;
    if cost.nesting > MAX_TEMPLATE_NESTING {
        return Err(EvaluationFailure::NestedTooDeeply);
    }
    if cost.evaluations > MAX_EVALUATIONS {
        return Err(EvaluationFailure::TooMuchWork);
    }

    let mut engine = ::handlebars::Handlebars::new();
    // STRICT: an expression naming an attribute the data does not state is a failure of
    // the render, not the empty string.
    engine.set_strict_mode(true);
    if escape == Escape::None {
        engine.register_escape_fn(::handlebars::no_escape);
    }
    engine.register_template(RENDERED, template);
    let mut part = BoundedPart::default();
    match engine.render_to_write(RENDERED, variables, &mut part) {
        Ok(()) => part.text(),
        Err(error) => Err(classify(&error)),
    }
}

/// What the engine's refusal to evaluate a part means on this surface.
fn classify(error: &::handlebars::RenderError) -> EvaluationFailure {
    match error.reason() {
        // The engine reports the path it could not resolve, which is the attribute's name
        // as the template wrote it.
        ::handlebars::RenderErrorReason::MissingVariable(Some(path)) => {
            EvaluationFailure::MissingAttribute(path.clone())
        }
        // The only writer a part is evaluated into is this module's own, and it fails for
        // exactly one reason: the part reached the ceiling.
        ::handlebars::RenderErrorReason::IOError(_) => EvaluationFailure::TooLarge,
        ::handlebars::RenderErrorReason::TemplateError(parse) => {
            EvaluationFailure::NotAValidTemplate(parse.reason().to_string())
        }
        other => EvaluationFailure::NotAValidTemplate(other.to_string()),
    }
}

/// What evaluating a parsed template COSTS, as the ceilings see it.
///
/// Read off the engine's own parse tree, so each of these is what the engine will do
/// rather than what the template's text looks like it says.
#[derive(Debug, Default, PartialEq, Eq)]
struct EvaluationCost {
    /// The deepest the template nests blocks, which is the depth its evaluation recurses
    /// to.
    nesting: usize,
    /// An upper bound on how many ELEMENTS the evaluation evaluates: every element of
    /// every body, counted once per time that body can be evaluated.
    evaluations: u64,
}

/// The block helpers of the language that evaluate their body at most once, so nesting
/// them multiplies nothing: the two conditionals and the one that only moves the context.
/// Every other block — `each`, and any name the language does not define, which the
/// evaluation refuses anyway — is counted as one that iterates.
const BLOCKS_THAT_DO_NOT_ITERATE: [&str; 3] = ["if", "unless", "with"];

impl EvaluationCost {
    /// What evaluating `template` costs, where `widest` is the most values anything in the
    /// rendering data holds.
    ///
    /// The count is the PRODUCT of the two factors that make a render expensive, per body
    /// rather than per template: a body is evaluated once per value of whatever the block
    /// around it iterates over — at most `widest` values per iterating block it sits
    /// inside — and each of those evaluations evaluates every element the body holds. A
    /// body with no elements still costs one, because evaluating it has a cost of its own
    /// even when it writes nothing, which is the case neither the output ceiling nor a
    /// count of elements alone would catch. Saturating throughout: the bound only has to
    /// be an over-estimate that cannot wrap.
    ///
    /// The walk is ITERATIVE over an explicit stack of the bodies still to visit, so a
    /// template the engine parsed costs no stack to measure, and it stops as soon as
    /// either ceiling is passed: nothing further in can make either answer smaller.
    fn of(
        template: &::handlebars::Template,
        widest: u64,
    ) -> Result<Self, EvaluationFailure> {
        let mut cost = Self::default();
        // Each entry is a body to visit, with the nesting and the iterating nesting it
        // sits at.
        let mut pending: Vec<(&::handlebars::Template, usize, u32)> = vec![(template, 0, 0)];
        while let Some((body, nesting, iterating)) = pending.pop() {
            let elements = (body.elements.len() as u64).max(1);
            let evaluations = elements.saturating_mul(widest.saturating_pow(iterating));
            cost.evaluations = cost.evaluations.saturating_add(evaluations);
            if cost.evaluations > MAX_EVALUATIONS {
                return Ok(cost);
            }
            for element in &body.elements {
                match element {
                    // Text, a comment the engine has already dropped, and an expression:
                    // none of them is a body to evaluate again.
                    ::handlebars::template::TemplateElement::RawString(_)
                    | ::handlebars::template::TemplateElement::Comment(_)
                    | ::handlebars::template::TemplateElement::Expression(_)
                    | ::handlebars::template::TemplateElement::HtmlExpression(_) => {}
                    ::handlebars::template::TemplateElement::HelperBlock(block) => {
                        let nesting = nesting.saturating_add(1);
                        let iterating = match iterates(block) {
                            true => iterating.saturating_add(1),
                            false => iterating,
                        };
                        cost.nesting = cost.nesting.max(nesting);
                        if cost.nesting > MAX_TEMPLATE_NESTING {
                            return Ok(cost);
                        }
                        // The block's body and the body its test selects instead are both
                        // bodies of this block, at this nesting.
                        for inner in [block.template.as_ref(), block.inverse.as_ref()]
                            .into_iter()
                            .flatten()
                        {
                            pending.push((inner, nesting, iterating));
                        }
                    }
                    // A partial expands a template inside a template and a decorator
                    // changes what the engine does with one; both are refused rather than
                    // evaluated. The last arm is what makes this walk fail closed: a
                    // construct it does not recognise is one whose cost it has not
                    // measured, so it is not evaluated either.
                    _ => return Err(EvaluationFailure::ConstructNotEvaluated),
                }
            }
        }
        Ok(cost)
    }
}

/// Does `block` evaluate its body once per value of something?
///
/// Read from the name the engine parsed, so it is the helper the engine will call. A block
/// whose name is not a plain one — a subexpression or a path — is counted as iterating,
/// which is the conservative answer.
fn iterates(block: &::handlebars::template::HelperTemplate) -> bool {
    match &block.name {
        ::handlebars::template::Parameter::Name(name) => {
            !BLOCKS_THAT_DO_NOT_ITERATE.contains(&name.as_str())
        }
        _ => true,
    }
}

/// How many values the widest container in `variables` holds, and never fewer than one.
///
/// The walk is ITERATIVE over an explicit stack, so data a caller nested deeply costs
/// stack space here for nothing: the data itself is already bounded by the model's 262144
/// bytes and by the depth its own parse accepts.
fn widest_container(variables: &Value) -> u64 {
    let mut widest = 1u64;
    let mut pending = vec![variables];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => {
                widest = widest.max(items.len() as u64);
                pending.extend(items);
            }
            Value::Object(members) => {
                widest = widest.max(members.len() as u64);
                pending.extend(members.values());
            }
            _ => {}
        }
    }
    widest
}

/// The part being evaluated, bounded by [`MAX_RENDERED_PART_BYTES`].
///
/// The engine writes a part incrementally, so the bound is applied as it is written rather
/// than measured afterwards: a template that names a large attribute many times stops at
/// the ceiling instead of allocating what it asked for. The bytes are kept as bytes and
/// read as text once, at the end, so no single write has to land on a character boundary
/// for the part to be the text the engine produced.
#[derive(Debug, Default)]
struct BoundedPart {
    /// What has been written so far.
    written: Vec<u8>,
}

impl BoundedPart {
    /// What was written, as the text it is.
    fn text(self) -> Result<String, EvaluationFailure> {
        String::from_utf8(self.written).map_err(|invalid| {
            // The engine writes the template's own text and the data's own values, both
            // of which reached here as `str`, so this says the engine wrote something
            // that is no text at all rather than anything about the template.
            EvaluationFailure::NotAValidTemplate(invalid.to_string())
        })
    }
}

impl ::std::io::Write for BoundedPart {
    fn write(&mut self, buffer: &[u8]) -> ::std::io::Result<usize> {
        if self.written.len().saturating_add(buffer.len()) > MAX_RENDERED_PART_BYTES {
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::WriteZero,
                "the rendered part reached this emulator's ceiling",
            ));
        }
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}
