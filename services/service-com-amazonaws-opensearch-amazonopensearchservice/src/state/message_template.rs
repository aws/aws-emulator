// SPDX-License-Identifier: CC0-1.0

//! What a trigger action SENDS: the message and the subject its templates render over the
//! run's context.
//!
//! "The message subject and body are Mustache templates", so what a notification carries is
//! the caller's template with the run's own values substituted into it — the count a search
//! returned, the monitor and trigger it came from, the period it covered. Nothing here
//! composes a message of its own: a template is RENDERED, and a template this renderer cannot
//! resolve is reported as the action's own error rather than sent half-substituted. The same
//! renderer substitutes the two values a monitor's own query is written against
//! (`period_start` and `period_end`), so one template language serves both.
//!
//! The renderer is the maintained Handlebars implementation this workspace builds with,
//! driven over the run context as a JSON value: Handlebars is a superset of Mustache's
//! interpolation — `{{a.b.0.c}}` resolves the same path through objects and list indexes —
//! and it is configured for the Mustache reading an alerting template is written against:
//! nothing is HTML-escaped (a notification is text, not markup) and a path the context does
//! not carry renders as nothing rather than failing the template. Its scripting feature is
//! left out of the build (see this crate's `Cargo.toml`), so a template evaluates no
//! expression language: it interpolates the context and iterates over it.
//!
//! # What bounds a rendering
//!
//! A template is the CALLER's, and the context it reads carries as many hits as a search
//! answers with, so three bounds in host code the template cannot reach:
//!
//! * a SOURCE ceiling ([`MAX_TEMPLATE_BYTES`]), so a template longer than any a monitor
//!   carries is refused before the renderer is handed anything;
//! * an OUTPUT ceiling ([`MAX_RENDERED_BYTES`]), enforced as the rendering is WRITTEN
//!   ([`Capped`]) rather than after it finishes, so a template that would produce more stops
//!   at the ceiling instead of building it in this process's heap first;
//! * a BLOCK bound ([`MAX_BLOCKS`], [`MAX_BLOCK_DEPTH`]): a block repeats its body once per
//!   element of what it names, so NESTED blocks would multiply the work by a factor the
//!   template chooses. With no nesting and a bounded count, the work one template causes is
//!   linear in the context it reads, whatever the template says — and a template that would
//!   produce no output at all is bounded by this rather than by the output ceiling.
//!
//! The blocks are counted by COMPILING the template with the renderer's own parser and
//! walking what it parsed ([`blocks_within_bounds`]), not by looking for a marker in the
//! text: every form the renderer accepts for a block — including the whitespace-control
//! spellings — is a block in that tree, so there is no spelling of one the bound does not
//! see. The walk is FAIL-CLOSED: it names the elements it admits (text, a comment, an
//! interpolation, a block) and refuses every other, which is what keeps a template from
//! pulling ANOTHER template in as it renders — a partial or an inline decorator expands at
//! render time, so neither this walk nor the output ceiling could bound its expansion, and a
//! notification template needs neither.

use std::io::Write;

use ::serde_json::Value;

/// The longest template source this module renders, in bytes. The template arrived in a
/// monitor document, which the data plane's own request-body ceiling already bounds; this is
/// what bounds ONE template of it.
const MAX_TEMPLATE_BYTES: usize = 64 * 1024;

/// The largest rendering one template may produce, in bytes — the ceiling on what a
/// notification this emulator publishes carries, since a template may repeat its context.
const MAX_RENDERED_BYTES: usize = 256 * 1024;

/// How many BLOCKS one template may open: a block repeats its body once per element of what
/// it names, so this is what bounds how many such repetitions one template asks for.
const MAX_BLOCKS: usize = 8;

/// How deep those blocks may nest, which is NOT AT ALL: a block inside a block multiplies its
/// body by both of their lengths, and nothing about a notification needs it.
const MAX_BLOCK_DEPTH: usize = 1;

/// Render `source` over `data`, or say why it could not be rendered.
///
/// `data` is the whole data the template reads, with the run context under the `ctx` member
/// an alerting template addresses it by.
pub fn render(source: &str, data: &Value) -> Result<String, String> {
    if source.len() > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "the template is longer than the {MAX_TEMPLATE_BYTES} bytes this cluster renders"
        ));
    }
    blocks_within_bounds(source)?;
    let mut registry = ::handlebars::Handlebars::new();
    // A notification carries TEXT, so nothing in it is escaped as markup: an ampersand or a
    // quote in a document the query matched reaches the channel as the document holds it.
    registry.register_escape_fn(::handlebars::no_escape);
    // A path the context does not carry renders as nothing, which is how a Mustache template
    // reads an absent value — not as a failure of the whole template.
    registry.set_strict_mode(false);
    let mut written = Capped::new();
    registry
        .render_template_to_write(source, data, &mut written)
        .map_err(|failed| {
            if written.overflowed {
                format!(
                    "the rendered message is larger than the {MAX_RENDERED_BYTES} bytes this \
                     cluster publishes"
                )
            } else {
                format!("the template could not be rendered: {failed}")
            }
        })?;
    String::from_utf8(written.text).map_err(|_| "the rendered message is not text".to_string())
}

/// Whether `source` stays within the block bounds this module states, or the refusal for a
/// template that does not.
///
/// The blocks are the ones the RENDERER parses, read off the tree its own parser produced:
/// the template is compiled here and the tree walked, so every spelling of a block the
/// renderer accepts is counted — a lexical scan for a marker would miss the spellings it does
/// not happen to look for. A template the parser cannot read is reported as the template
/// failure it is, before anything is rendered.
fn blocks_within_bounds(source: &str) -> Result<(), String> {
    let parsed = ::handlebars::Template::compile(source)
        .map_err(|failed| format!("the template could not be read: {failed}"))?;
    let mut opened = 0usize;
    counted_blocks(&parsed, 1, &mut opened)
}

/// Walk one parsed template, counting the blocks it opens, refusing a depth past
/// [`MAX_BLOCK_DEPTH`], and refusing every element whose work this module cannot bound.
///
/// `depth` is the nesting level this template sits at, so the body of a block is one level
/// deeper than the block itself. The walk is FAIL-CLOSED: the elements it admits are named
/// one by one — literal text, a comment, an interpolation, and a helper block whose body the
/// bounds above cover — and anything else is refused. That is what keeps a template from
/// pulling ANOTHER template in while it renders: a partial or a decorator expands at render
/// time rather than in the parse tree, so its expansion is bounded by nothing here (and by
/// nothing in the output ceiling either, since an expansion that writes no bytes is never
/// refused a write) — a notification template needs neither, so neither is rendered.
fn counted_blocks(
    template: &::handlebars::Template,
    depth: usize,
    opened: &mut usize,
) -> Result<(), String> {
    use ::handlebars::template::TemplateElement;

    for element in &template.elements {
        let nested: Vec<&::handlebars::Template> = match element {
            // Literal text, a comment and an interpolation each write once, so the output
            // ceiling is what bounds them.
            TemplateElement::RawString(_)
            | TemplateElement::Comment(_)
            | TemplateElement::Expression(_)
            | TemplateElement::HtmlExpression(_) => Vec::new(),
            // A block repeats its body over what it names, which is what the count and the
            // depth below bound.
            TemplateElement::HelperBlock(helper) => helper
                .template
                .iter()
                .chain(helper.inverse.iter())
                .collect(),
            // A partial, a decorator, or any element a later release of the renderer adds:
            // each can pull a template in as it renders, and this module bounds the parse
            // tree rather than the expansion, so such a template is refused rather than
            // rendered.
            _ => return Err(UNBOUNDED_ELEMENT.to_string()),
        };
        if nested.is_empty() {
            continue;
        }
        *opened = opened.saturating_add(1);
        if *opened > MAX_BLOCKS {
            return Err(format!(
                "the template opens more than the {MAX_BLOCKS} blocks this cluster renders"
            ));
        }
        if depth > MAX_BLOCK_DEPTH {
            return Err(format!(
                "the template nests a block inside a block, and this cluster renders at most \
                 {MAX_BLOCK_DEPTH} level of them"
            ));
        }
        for body in nested {
            counted_blocks(body, depth.saturating_add(1), opened)?;
        }
    }
    Ok(())
}

/// What a template carrying an element this module cannot bound is refused with: the
/// partials and decorators a notification template has no use for.
const UNBOUNDED_ELEMENT: &str = "this cluster renders the text, the interpolations and the \
                                 blocks of a template, and this template names something \
                                 else (a partial or a decorator)";

/// Where a rendering is written: the bytes it has produced so far, up to
/// [`MAX_RENDERED_BYTES`].
///
/// The ceiling is enforced HERE, as the renderer writes, so a template that would produce
/// more is stopped at the ceiling rather than after this process has held all of it.
#[derive(Debug)]
struct Capped {
    text: Vec<u8>,
    /// Set once a write was refused for passing the ceiling, so the refusal names the
    /// ceiling rather than whatever the renderer made of the write failure.
    overflowed: bool,
}

impl Capped {
    fn new() -> Self {
        Self {
            text: Vec::new(),
            overflowed: false,
        }
    }
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> ::std::io::Result<usize> {
        if self.text.len().saturating_add(buf.len()) > MAX_RENDERED_BYTES {
            self.overflowed = true;
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::WriteZero,
                "the rendering passed the ceiling this cluster publishes",
            ));
        }
        self.text.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> ::std::io::Result<()> {
        Ok(())
    }
}
