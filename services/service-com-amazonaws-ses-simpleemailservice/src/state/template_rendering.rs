// SPDX-License-Identifier: CC0-1.0

//! How an email template of this account is RENDERED against a caller's replacement values,
//! and the MIME message the rendered parts compose.
//!
//! A template carries a subject line and up to two body parts, each written in the template
//! language this service's templates are documented with: `{{name}}` stands for a replacement
//! value, and a block (`{{#if}}`, `{{#each}}`, `{{#unless}}`, `{{#with}}`) takes a branch or
//! repeats its body over one. So rendering a template is a SUBSTITUTION over the replacement
//! data the request carried, and these are the rules it follows — one place for them, because
//! every operation of this service that renders a template follows the same ones:
//!
//! * the replacement data is a JSON OBJECT of replacement values ("This parameter is a JSON
//!   object, typically consisting of key-value pairs in which the keys correspond to
//!   replacement tags in the email template"), so text that is not JSON, and JSON that is not
//!   an object, carries no replacement value at all;
//! * a replacement tag the rendering REACHES and the data holds no value for is a missing
//!   rendering attribute, reported with the tag it names rather than substituted as nothing —
//!   and a tag inside a branch the data did not take is never reached, so it is never missing;
//! * a value substituted into the HTML body is escaped as markup and one substituted into the
//!   subject line or the text body is not ([`Escaping`]);
//! * what the rendering produces is ONE MIME message: the rendered subject line as the
//!   message's `Subject` header, the instant it was composed as its `Date` header, and the
//!   rendered body parts as its content — the two of them as the alternatives of a
//!   `multipart/alternative` message when the template carries both, since that is the one
//!   message a template with a text and an HTML body composes.
//!
//! The language is NOT re-implemented here, and no construct of it is recognised by scanning
//! the template's TEXT: the maintained Handlebars implementation this workspace already builds
//! with (see this crate's `Cargo.toml`) parses a template into its own tree and evaluates that
//! tree, and everything this module decides about what a template does it reads OFF that tree.
//! The two counts below are the single exception, and they are no reading of the language at
//! all: they bound the engine's own PARSE, which necessarily happens before there is a tree to
//! read.
//!
//! # What bounds a rendering
//!
//! The template is the account's and the data is the request's, so a rendering is work a
//! caller sizes. Every input is bounded before it gets here — `TemplateData` at 262144 bytes
//! in the model — and a rendering happens in three stages, each of them bounded:
//!
//! 1. The PARSE is the engine's, and two properties of the TEXT are counted before it, since a
//!    parse is what they bound: the tags a part writes, against [`MAX_TAGS`], and the
//!    parentheses it carries, against [`MAX_PARENTHESES`]. Both are counts of a character
//!    sequence, so text that merely looks like a construct is counted too, which can only
//!    refuse a template rather than admit one. The part's own length is bounded as well, at
//!    [`MAX_MESSAGE_BYTES`]: a part longer than the largest message this service sends can be
//!    part of no message at all.
//! 2. What the parsed template IS is then read off the engine's tree, ITERATIVELY over an
//!    explicit stack ([`RenderCost`]): how deeply its blocks nest, against
//!    [`MAX_TEMPLATE_NESTING`], and what evaluating it would COST — every element of every
//!    body, counted once per time that body can be evaluated — against [`MAX_RENDER_STEPS`].
//!    That is the PRODUCT of the two factors a caller chooses rather than either alone, and a
//!    body with no elements still counts, because a block whose body writes nothing costs time
//!    and nothing else. The walk is FAIL-CLOSED: a partial, a decorator or any construct it
//!    does not recognise is refused rather than rendered — a partial expands a template inside
//!    a template, so refusing it is what leaves the evaluation a finite walk.
//! 3. The RENDER writes into a sink that stops at the message ceiling ([`Capped`]), one budget
//!    across the parts, so a template that names the same value many times stops at the
//!    ceiling instead of building what it asked for in this process's heap.
//!
//! All three stages run on a thread of this module's own, with a stack of its own
//! ([`RENDER_STACK_BYTES`]), and the request waits [`RENDER_DEADLINE`] for them and no longer:
//! that wait is the bound that does not depend on anything here having foreseen what the
//! template does. A rendering still going when it passes is given up on and the caller
//! answered without it; such a rendering keeps its permit until its thread ends, and at most
//! [`MAX_RENDERS_IN_FLIGHT`] of them run at once, so abandoned renderings cannot accumulate —
//! past that, a rendering is refused rather than started.
//!
//! The engine runs with NO feature of its crate enabled (see this crate's `Cargo.toml`): no
//! scripting language, no directory template source and no embedded one, so a template reaches
//! no host file and no second interpreter. Nothing is registered on it either — a template
//! that calls a helper the language does not define is refused rather than served — so the
//! language a caller's template is rendered in is exactly the one the engine defines.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use ::serde_json::Value;

use super::MAX_MESSAGE_BYTES;

/// How deeply a template may nest its blocks.
///
/// An EMULATOR ceiling, not an AWS quota: the evaluation of a block's body is a nested
/// evaluation, so nesting is the one property of a caller's template that becomes depth inside
/// the engine's render. Sixty-four is far past what a message's own structure needs, and the
/// nesting counted against it is the nesting of the engine's own parse tree, so no spelling of
/// a block tag can hide a level from it — a chained `else if` nests one level per link, which
/// is how that tree holds it.
pub const MAX_TEMPLATE_NESTING: usize = 64;

/// How many tags one part may write.
///
/// An EMULATOR ceiling, and what keeps the engine's PARSE short: the parse costs more than
/// linearly in the number of tags it reads, and a stored template is only size-bounded, so a
/// part that writes more tags than this is refused before the parse begins. It bounds the
/// nesting of every construct as well, since a level of nesting needs a tag of its own.
/// Counted as the occurrences of a tag's opening braces in the text, so text that merely looks
/// like a tag is counted too; a thousand is far past the number of values any one message
/// substitutes.
pub const MAX_TAGS: usize = 1024;

/// How many parentheses one part may carry.
///
/// An EMULATOR ceiling, and the one bound on the engine's PARSE this module has to supply
/// itself: the engine stops a template that nests BLOCKS too deeply, but a subexpression — the
/// parenthesised form of a helper call — nests its parse with nothing stopping it, and every
/// level of that nesting opens one parenthesis. They are counted over the whole text, so a
/// parenthesis in the message's own words is counted too, which can only refuse a template
/// rather than admit one; two hundred and fifty-six is far past the nesting a message's own
/// structure needs and far below what [`RENDER_STACK_BYTES`] holds.
pub const MAX_PARENTHESES: usize = 256;

/// How many elements of a template one part may evaluate.
///
/// The ceiling that bounds the WORK a rendering costs rather than the message it produces, and
/// it bounds the PRODUCT of the two things that make a rendering expensive rather than either
/// alone: how many times a body is evaluated (once per element of whatever the blocks around
/// it repeat over) and how many elements that body holds each time. The output ceiling cannot
/// stand in for it, because an element that resolves to nothing writes nothing; a body with no
/// elements at all still counts as one evaluation, because evaluating it costs something too.
/// A million element evaluations is far past any message a template of the permitted size
/// describes, and takes a fraction of the deadline below.
pub const MAX_RENDER_STEPS: u64 = 1_000_000;

/// How long the request waits for a rendering.
///
/// The bound that does not depend on anything here having foreseen what the template does:
/// whatever it turns out to be, the caller is answered within this of asking. CHOSEN, like
/// every other ceiling in this module — far longer than the bounded work above can take, so it
/// is reached only by something the bounds did not foresee.
pub const RENDER_DEADLINE: Duration = Duration::from_secs(2);

/// How many renderings may be under way at once.
///
/// A rendering that outlives [`RENDER_DEADLINE`] is given up on but cannot be stopped — the
/// engine has no interrupt — so it keeps its permit until its own thread ends. This is
/// therefore what stops abandoned renderings accumulating: past it, a rendering is refused
/// rather than started.
pub const MAX_RENDERS_IN_FLIGHT: usize = 32;

/// The stack the parse and the rendering of one template run on.
///
/// Its own, rather than the serving task's: the parse recurses once per level of subexpression
/// nesting, which [`MAX_PARENTHESES`] bounds, and the render recurses once per level of block
/// nesting, which [`MAX_TEMPLATE_NESTING`] bounds. This holds many times the deepest either
/// ceiling admits, and only the pages a rendering touches are ever committed.
const RENDER_STACK_BYTES: usize = 32 * 1024 * 1024;

/// What a tag of the language opens with, whatever the tag turns out to be: counting these is
/// what [`MAX_TAGS`] bounds.
const TAG_OPEN: &str = "{{";

/// The name the one template of a rendering is registered under. A registry is built for one
/// part and dropped with it, so there is only ever one.
const RENDERED: &str = "rendered";

/// How many renderings are under way, which [`MAX_RENDERS_IN_FLIGHT`] bounds.
static RENDERS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Why a template could not be rendered over a request's replacement values.
///
/// Two outcomes, because the service models two: a replacement tag the data holds no value
/// for, and replacement data this service cannot render a template over at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderingFailure {
    /// A replacement tag the rendering reached, which the data holds no value for. Carries
    /// the tag as the template spells it, which is what the refusal names.
    MissingAttribute(Option<String>),
    /// The replacement values cannot render this template: they are not a JSON object of
    /// replacement values, or the rendering they ask for is past a bound this module states.
    /// Carries the sentence the refusal reports.
    InvalidRenderingData(String),
}

/// How this service states a replacement tag the rendering data holds no value for.
///
/// `MissingRenderingAttributeException` is "Indicates that one or more of the replacement
/// values for the specified template was not specified", and the sentence names the attribute
/// that was missing — the one variable part — quoted. One wording, here rather than in one
/// operation, because every operation that renders a template reports the same absence.
pub fn missing_rendering_attribute_message(attribute: Option<&str>) -> String {
    match attribute {
        Some(attribute) => {
            format!("Attribute '{attribute}' is not present in the rendering data.")
        }
        // Unreachable while the renderer names the tag it could not resolve; the sentence
        // still reports the same absence rather than an empty quotation.
        None => "An attribute referenced by the template is not present in the rendering \
                 data."
            .to_string(),
    }
}

/// How this service states a template the RENDERING of one could not resolve: the preview
/// reports the lookup it made over the account's templates — the fixed text `Unable to find `,
/// then the template the request named and the account it was looked for in, each in angle
/// brackets, with no trailing period.
///
/// A different sentence from the one the template store's own operations report
/// ([`super::missing_template_message`]) because it reports a different lookup: this one is
/// the rendering resolving a template of an account, which is also why it names the account.
pub fn missing_rendering_template_message(template_name: &str) -> String {
    format!(
        "Unable to find <{template_name}> for <{}>",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// How this service states replacement values it cannot render a template over.
///
/// `InvalidRenderingParameterException` "may occur when the TemplateData object contains
/// invalid JSON", and the sentence the service answers for it states that of the rendering
/// DATA, with no variable part at all: it quotes neither the data the request sent nor the
/// place the reading of it stopped.
pub const INVALID_RENDERING_DATA: &str = "Template rendering data is invalid";

/// How this service states a rendering whose work is past the bounds this module states: the
/// message it would produce is larger than the largest message this service sends, the
/// template and the data together ask for more evaluation than one rendering performs, or
/// there was no rendering to be had within the budget.
pub const RENDERING_TOO_LARGE: &str =
    "The replacement values ask for a message larger than this service sends.";

/// How this service states a template its renderer cannot read, or one whose shape or
/// constructs this module does not render.
pub const UNRENDERABLE_TEMPLATE: &str =
    "The template could not be rendered with the replacement values provided.";

/// The MIME message the template's parts render to over `template_data`, rendered on a thread
/// of this module's own and waited for no longer than [`RENDER_DEADLINE`].
///
/// What an operation of this service calls. The engine is synchronous and has no interrupt of
/// its own, so the rendering happens off the serving runtime: a rendering that does not come
/// back cannot hold the task that asked for it, and one given up on keeps its place among the
/// [`MAX_RENDERS_IN_FLIGHT`] allowed at once until its thread really ends.
pub async fn rendered_template_within_budget(
    subject_part: Option<String>,
    text_part: Option<String>,
    html_part: Option<String>,
    template_data: String,
    composed_at: SystemTime,
) -> Result<String, RenderingFailure> {
    let too_large =
        || RenderingFailure::InvalidRenderingData(RENDERING_TOO_LARGE.to_string());
    let Some(permit) = Permit::take() else {
        return Err(too_large());
    };
    let (answer, wait) = ::tokio::sync::oneshot::channel();
    let started = ::std::thread::Builder::new()
        .name("ses-template-render".to_string())
        .stack_size(RENDER_STACK_BYTES)
        .spawn(move || {
            let outcome = rendered_template(
                subject_part.as_deref(),
                text_part.as_deref(),
                html_part.as_deref(),
                &template_data,
                composed_at,
            );
            // The permit goes back when this thread ENDS, whether or not anybody is still
            // waiting for what it produced.
            drop(permit);
            // A caller that stopped waiting dropped the other end; the answer is dropped
            // with it.
            let _ = answer.send(outcome);
        });
    if started.is_err() {
        // The host would not start another thread, so this rendering cannot run now. The
        // permit was moved into the closure that was never spawned and is released with it.
        return Err(too_large());
    }
    match ::tokio::time::timeout(RENDER_DEADLINE, wait).await {
        Ok(Ok(outcome)) => outcome,
        // The thread ended without sending an answer, or the wait ran out: either way this
        // request has no rendering to report.
        Ok(Err(_)) | Err(_) => Err(too_large()),
    }
}

/// One rendering's place among the [`MAX_RENDERS_IN_FLIGHT`] allowed at once.
///
/// Held by the thread that renders, and released when that thread ends — not when the request
/// stops waiting for it — so a rendering given up on still counts for as long as it really
/// lasts.
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

/// The MIME message the template's parts render to over `template_data`.
///
/// The ONE rendering every operation of this service that renders a template reaches (through
/// [`rendered_template_within_budget`], which is what runs it off the serving runtime): the
/// parts as the account holds them, the replacement values as the request sent them, and the
/// whole message as the caller reads it.
pub fn rendered_template(
    subject_part: Option<&str>,
    text_part: Option<&str>,
    html_part: Option<&str>,
    template_data: &str,
    composed_at: SystemTime,
) -> Result<String, RenderingFailure> {
    let data = rendering_data(template_data)?;
    let widest = widest_collection(&data);
    // One output budget across the parts, so the message as a whole is bounded rather than
    // each part being bounded on its own, and ONE deadline across them: the whole rendering is
    // what a request waits [`RENDER_DEADLINE`] for, so that is the span the sink the engine
    // writes into accepts writes within ([`Capped`]) — which is what ends a rendering whose
    // measured cost turned out to be an under-estimate instead of leaving it running.
    let mut budget = MAX_MESSAGE_BYTES;
    let deadline = ::std::time::Instant::now() + RENDER_DEADLINE;
    let subject = rendered_part(
        subject_part,
        &data,
        widest,
        Escaping::None,
        &mut budget,
        deadline,
    )?;
    let text = rendered_part(text_part, &data, widest, Escaping::None, &mut budget, deadline)?;
    let html = rendered_part(html_part, &data, widest, Escaping::Html, &mut budget, deadline)?;
    let message = super::composed_message::compose(&super::composed_message::Composition::of_parts(
        subject.as_deref(),
        text.as_deref(),
        html.as_deref(),
        composed_at,
    ));
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(RenderingFailure::InvalidRenderingData(
            RENDERING_TOO_LARGE.to_string(),
        ));
    }
    Ok(message)
}

/// The replacement values `template_data` carries, as the data a rendering reads.
///
/// `TemplateData` "is a JSON object, typically consisting of key-value pairs in which the keys
/// correspond to replacement tags in the email template", so text that is not JSON — and JSON
/// that is not an object — is replacement data no template can be rendered over.
fn rendering_data(template_data: &str) -> Result<Value, RenderingFailure> {
    let invalid =
        || RenderingFailure::InvalidRenderingData(INVALID_RENDERING_DATA.to_string());
    let data: Value = ::serde_json::from_str(template_data).map_err(|_| invalid())?;
    if !data.is_object() {
        return Err(invalid());
    }
    Ok(data)
}

/// Whether the values a part substitutes are escaped as markup.
///
/// The HTML body IS markup, so a replacement value substituted into it is escaped: a value
/// carrying `&` or `<` reaches the reader as that character rather than as the start of an
/// entity or a tag. The subject line and the text body are not markup, so a value belongs in
/// them as the data wrote it. A part that wants a value in HTML as written says so with the
/// template language's own unescaped spelling (`{{{name}}}`). The same rule the account's
/// other email API renders a saved template by, so one template renders the same way whichever
/// of the two reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escaping {
    /// The value is substituted as the data holds it.
    None,
    /// The value is escaped as HTML.
    Html,
}

/// One part rendered over `data`, with `budget` bytes left for the message, or `None` for a
/// part the template does not carry. Takes what it wrote out of the budget.
///
/// The bounds of the three stages this module states are applied here, in that order: the two
/// counts over the text, then the cost read off the parsed tree, then the rendering itself.
fn rendered_part(
    source: Option<&str>,
    data: &Value,
    widest: u64,
    escaping: Escaping,
    budget: &mut usize,
    deadline: ::std::time::Instant,
) -> Result<Option<String>, RenderingFailure> {
    let Some(source) = source else {
        return Ok(None);
    };
    let too_large =
        || RenderingFailure::InvalidRenderingData(RENDERING_TOO_LARGE.to_string());
    let unrenderable =
        || RenderingFailure::InvalidRenderingData(UNRENDERABLE_TEMPLATE.to_string());
    if source.len() > MAX_MESSAGE_BYTES {
        return Err(too_large());
    }
    // Before the parse, because the parse is what these bound: see [`MAX_TAGS`] and
    // [`MAX_PARENTHESES`].
    if source.matches(TAG_OPEN).count() > MAX_TAGS {
        return Err(unrenderable());
    }
    if source.bytes().filter(|byte| *byte == b'(').count() > MAX_PARENTHESES {
        return Err(unrenderable());
    }
    let parsed = ::handlebars::Template::compile(source).map_err(|_| unrenderable())?;
    let cost = RenderCost::of(&parsed, widest)?;
    if cost.nesting > MAX_TEMPLATE_NESTING {
        return Err(unrenderable());
    }
    if cost.evaluations > MAX_RENDER_STEPS {
        return Err(too_large());
    }
    let rendered = rendered_text(parsed, data, escaping, *budget, deadline)?;
    *budget = budget.saturating_sub(rendered.len());
    Ok(Some(rendered))
}

/// `parsed` rendered over `data`, escaping its substituted values as `escaping` says, and
/// writing at most `budget` bytes.
///
/// The engine is configured for the reading an email template is written against: a
/// replacement tag the data holds no value for FAILS the rendering rather than resolving to
/// nothing, which is what makes a missing rendering attribute reportable instead of silently
/// dropping part of the message; and a value is escaped only where the part it is substituted
/// into is markup ([`Escaping`]).
fn rendered_text(
    parsed: ::handlebars::Template,
    data: &Value,
    escaping: Escaping,
    budget: usize,
    deadline: ::std::time::Instant,
) -> Result<String, RenderingFailure> {
    let mut engine = ::handlebars::Handlebars::new();
    if escaping == Escaping::None {
        engine.register_escape_fn(::handlebars::no_escape);
    }
    engine.set_strict_mode(true);
    engine.register_template(RENDERED, parsed);
    let mut written = Capped::new(budget, deadline);
    engine
        .render_to_write(RENDERED, data, &mut written)
        .map_err(|failed| {
            if written.overflowed {
                return RenderingFailure::InvalidRenderingData(RENDERING_TOO_LARGE.to_string());
            }
            match failed.reason() {
                ::handlebars::RenderErrorReason::MissingVariable(attribute) => {
                    RenderingFailure::MissingAttribute(attribute.clone())
                }
                _ => RenderingFailure::InvalidRenderingData(UNRENDERABLE_TEMPLATE.to_string()),
            }
        })?;
    String::from_utf8(written.text).map_err(|_| {
        RenderingFailure::InvalidRenderingData(UNRENDERABLE_TEMPLATE.to_string())
    })
}

/// The widest collection `data` carries — the most elements a block of a template could repeat
/// over — and never fewer than one, since a block over nothing still renders once.
///
/// Walked with an explicit worklist rather than by recursion, so the depth of the data decides
/// nothing about this process's stack; the data itself is already bounded by the model's
/// 262144 bytes and by the depth its own parse accepts.
fn widest_collection(data: &Value) -> u64 {
    let mut widest = 1_u64;
    let mut worklist = vec![data];
    while let Some(value) = worklist.pop() {
        match value {
            Value::Array(elements) => {
                widest = widest.max(elements.len() as u64);
                worklist.extend(elements.iter());
            }
            Value::Object(members) => {
                widest = widest.max(members.len() as u64);
                worklist.extend(members.values());
            }
            _ => {}
        }
    }
    widest
}

/// What rendering a parsed template COSTS, as the ceilings see it.
///
/// Read off the engine's own parse tree, so each of these is what the engine will do rather
/// than what the template's text looks like it says.
#[derive(Debug, Default, PartialEq, Eq)]
struct RenderCost {
    /// The deepest the template nests blocks, which is the depth its rendering recurses to.
    nesting: usize,
    /// An upper bound on how many ELEMENTS the rendering evaluates: every element of every
    /// body, counted once per time that body can be evaluated.
    evaluations: u64,
}

/// The block helpers of the language that evaluate their body at most once, so nesting them
/// multiplies nothing: the two conditionals and the one that only moves the context. Every
/// other block — `each`, and any name the language does not define, which the rendering
/// refuses anyway — is counted as one that repeats.
const BLOCKS_THAT_DO_NOT_REPEAT: [&str; 3] = ["if", "unless", "with"];

impl RenderCost {
    /// What rendering `template` costs, where `widest` is the most elements anything in the
    /// replacement data holds.
    ///
    /// The count is the PRODUCT of the two factors that make a rendering expensive, per body
    /// rather than per template: a body is evaluated once per element of whatever the block
    /// around it repeats over — at most `widest` elements per repeating block it sits inside —
    /// and each of those evaluations RESOLVES every value the body's elements name
    /// ([`resolutions_of`]), which is the element itself and every parameter and hash argument
    /// it carries. A body with no elements still costs one, because evaluating it has a cost of
    /// its own even when it writes nothing, which is the case neither the output ceiling nor a
    /// count of elements alone would catch. Saturating throughout: the bound only has to be an
    /// over-estimate that cannot wrap.
    ///
    /// The walk is ITERATIVE over an explicit stack of the bodies still to visit, so a
    /// template the engine parsed costs no stack to measure, and it stops as soon as either
    /// ceiling is passed: nothing further in can make either answer smaller.
    fn of(
        template: &::handlebars::Template,
        widest: u64,
    ) -> Result<Self, RenderingFailure> {
        use ::handlebars::template::TemplateElement;

        let mut cost = Self::default();
        // Each entry is a body to visit, with the nesting and the repeating nesting it sits
        // at.
        let mut pending: Vec<(&::handlebars::Template, usize, u32)> = vec![(template, 0, 0)];
        while let Some((body, nesting, repeating)) = pending.pop() {
            let resolutions = body
                .elements
                .iter()
                .map(resolutions_of)
                .fold(0_u64, u64::saturating_add)
                .max(1);
            let evaluations = resolutions.saturating_mul(widest.saturating_pow(repeating));
            cost.evaluations = cost.evaluations.saturating_add(evaluations);
            if cost.evaluations > MAX_RENDER_STEPS {
                return Ok(cost);
            }
            for element in &body.elements {
                match element {
                    // Literal text, a comment the engine has already dropped and an
                    // interpolation: none of them is a body to evaluate again.
                    TemplateElement::RawString(_)
                    | TemplateElement::Comment(_)
                    | TemplateElement::Expression(_)
                    | TemplateElement::HtmlExpression(_) => {}
                    TemplateElement::HelperBlock(block) => {
                        let nesting = nesting.saturating_add(1);
                        let repeating = match repeats(block) {
                            true => repeating.saturating_add(1),
                            false => repeating,
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
                            pending.push((inner, nesting, repeating));
                        }
                    }
                    // A partial expands a template inside a template and a decorator changes
                    // what the engine does with one; both are refused rather than rendered.
                    // The last arm is what makes this walk fail closed: a construct it does
                    // not recognise is one whose cost it has not measured, so it is not
                    // rendered either.
                    _ => {
                        return Err(RenderingFailure::InvalidRenderingData(
                            UNRENDERABLE_TEMPLATE.to_string(),
                        ))
                    }
                }
            }
        }
        Ok(cost)
    }
}

/// How many values the engine RESOLVES to evaluate ONE element of a body.
///
/// The element itself, and every parameter, hash argument and subexpression it names: the
/// engine resolves each of them on every evaluation of the body holding the element, whether or
/// not the helper it hands them to reads them — a built-in conditional reads its first
/// parameter and resolves the rest all the same — so a count of elements alone would measure an
/// element carrying a long parameter list as costing what a bare one costs.
///
/// Walked ITERATIVELY over the parameters in hand, so measuring costs no stack however deeply a
/// template nests its subexpressions, and saturating, because the count only has to be an
/// over-estimate that cannot wrap.
fn resolutions_of(element: &::handlebars::template::TemplateElement) -> u64 {
    use ::handlebars::template::{Parameter, TemplateElement};

    let mut resolutions = 1_u64;
    let mut pending: Vec<&Parameter> = Vec::new();
    match element {
        TemplateElement::Expression(helper)
        | TemplateElement::HtmlExpression(helper)
        | TemplateElement::HelperBlock(helper) => {
            pending.push(&helper.name);
            pending.extend(helper.params.iter());
            pending.extend(helper.hash.values());
        }
        _ => {}
    }
    while let Some(parameter) = pending.pop() {
        resolutions = resolutions.saturating_add(1);
        if let Parameter::Subexpression(subexpression) = parameter {
            if let Some(params) = subexpression.params() {
                pending.extend(params.iter());
            }
            if let Some(hash) = subexpression.hash() {
                pending.extend(hash.values());
            }
        }
    }
    resolutions
}

/// Does `block` evaluate its body once per element of something?
///
/// Read from the name the engine parsed, so it is the helper the engine will call. A block
/// whose name is not a plain one — a subexpression or a path — is counted as repeating, which
/// is the conservative answer.
fn repeats(block: &::handlebars::template::HelperTemplate) -> bool {
    match &block.name {
        ::handlebars::template::Parameter::Name(name) => {
            !BLOCKS_THAT_DO_NOT_REPEAT.contains(&name.as_str())
        }
        _ => true,
    }
}

/// Where a rendering is written: the bytes it has produced so far, up to the budget it was
/// given and up to the instant it was given.
///
/// Both ceilings are enforced HERE, as the renderer writes, which is the only place a running
/// rendering can be stopped from: the engine has no interrupt, so a rendering whose measured
/// cost ([`RenderCost`]) turned out to be an under-estimate would otherwise keep a thread and
/// its permit ([`MAX_RENDERS_IN_FLIGHT`]) for as long as it kept going. A write past the budget
/// is refused so the message is stopped at the ceiling rather than after this process has held
/// all of it, and a write past the deadline is refused so the rendering ends when the request
/// that waited for it already has.
#[derive(Debug)]
struct Capped {
    /// What has been written so far.
    text: Vec<u8>,
    /// The most bytes this sink accepts.
    budget: usize,
    /// The instant past which this sink accepts nothing more.
    deadline: ::std::time::Instant,
    /// Set once a write was refused for passing the budget, so the refusal names the ceiling
    /// rather than whatever the renderer made of the write failure.
    overflowed: bool,
}

impl Capped {
    fn new(budget: usize, deadline: ::std::time::Instant) -> Self {
        Self {
            text: Vec::new(),
            budget,
            deadline,
            overflowed: false,
        }
    }
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        if self.text.len().saturating_add(buf.len()) > self.budget {
            self.overflowed = true;
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::WriteZero,
                "the rendering passed the ceiling this service composes a message within",
            ));
        }
        if ::std::time::Instant::now() > self.deadline {
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::TimedOut,
                "the rendering passed the deadline this service renders a template within",
            ));
        }
        self.text.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}
