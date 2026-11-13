// SPDX-License-Identifier: CC0-1.0

//! The MAPPING TEMPLATE half of an integration: rendering one against the call it serves.
//!
//! An integration's `requestTemplates` and an integration response's `responseTemplates`
//! are each "a map of Velocity templates that are applied on the request payload"
//! (`Integration$requestTemplates`), keyed by content type. API Gateway RENDERS the
//! template that keys the call and uses the TEXT it produced — as the integration request
//! (which for a `MOCK` is also what the integration answers) and as the response body — so
//! a template is not a body until it has been rendered.
//!
//! # What is rendered, and what is not
//!
//! A template is made of literal text and REFERENCES. A reference names one of the
//! variables API Gateway documents a mapping template against — the method request
//! (`$input`), the stage's own map (`$stageVariables`) and the call's context
//! (`$context`) — optionally calling one of the methods that variable documents. Those
//! are a CLOSED set of values read off this call, so rendering one is a lookup, not an
//! evaluation: nothing here executes anything the template says.
//!
//! Everything else stays unrendered, and the caller of this module answers such a template
//! the way it answers an integration it cannot perform:
//!
//! * a reference naming a variable, a method or a name outside that set — a `$context`
//!   member this call has no value for, a stage variable the stage does not define, or one
//!   of the `$util` transformations whose exact output AWS's own documentation does not
//!   state (`parseJson`, `validateProperty`), so a rendering of one would be guesswork. The
//!   `$util` conversions whose output a published standard DOES state exactly are performed:
//!   the `application/x-www-form-urlencoded` encode and decode, by the maintained coder for
//!   that format, and the JavaScript string escape, by the rule that language's own grammar
//!   gives for a string literal;
//! * a `$input` reference to the payload where the call has no payload the template can
//!   read (see [`TemplateInputs::payload`]);
//! * a DIRECTIVE (`#set`, `#if`, `#foreach`, …), which is the half of the language that is
//!   statements rather than values. Deciding what a directive produces means executing it,
//!   which this emulator does not do; the caller checks for one before rendering.
//!
//! A `$` that opens no reference at all is literal text, exactly as the language treats it,
//! so a template carrying no reference renders to itself.

use std::collections::BTreeMap;

/// What a mapping template's references are rendered against: one call, as the method
/// request and the stage present it.
///
/// Built by whatever is serving the call — the `execute-api` data plane off the wire, and
/// `TestInvokeMethod` out of the members a caller simulates a request with — so a template
/// renders the same way for an invocation that reached a socket and one that never did.
#[derive(Debug, Default, Clone)]
pub struct TemplateInputs {
    /// The payload `$input.body`, `$input.json(…)` and `$input.path(…)` read, or `None`
    /// when this call has none they can read.
    ///
    /// `None` is not an empty payload: it says the payload a template would read is not
    /// known here (the response template of an integration whose own request template
    /// could not be rendered, say), so a reference to it resolves to nothing rather than
    /// to the empty string.
    pub payload: Option<Vec<u8>>,
    /// The path parameters the matched resource path binds, by name.
    pub path: BTreeMap<String, String>,
    /// The query-string parameters the request carries, by name — the first value under
    /// each name, which is the one a single-valued reference names.
    pub querystring: BTreeMap<String, String>,
    /// The headers the request carries, by name as sent; looked up case-insensitively,
    /// because a header name is.
    pub header: BTreeMap<String, String>,
    /// The stage's own `variables`, which `$stageVariables` names.
    pub stage_variables: BTreeMap<String, String>,
    /// The `$context` values of this call, keyed by the member path a template writes
    /// (`httpMethod`, `identity.sourceIp`, …).
    pub context: BTreeMap<String, String>,
}

/// A template this emulator does not render, and why in one phrase.
#[derive(Debug)]
pub struct Unrendered {
    /// What about the template is not rendered — the reference nothing resolves, or the
    /// bound the rendering would pass — for the reason the caller logs and reports.
    pub reason: String,
}

/// Emulator resource guard, NOT an AWS quota: the largest body one template renders to.
///
/// A template is at most [`crate::ops::put_integration::MAX_TEMPLATE_CHARS`] and each
/// reference in it stands for a value of the call, so the rendering is bounded by the
/// PRODUCT of how many references a developer stored and how large the payload one of them
/// names is — which is what this caps, rather than either factor alone. A small multiple of
/// the template cap: generous enough that no body a backend would answer is refused for its
/// size, and close enough to it that what the stage's response cache may come to hold stays
/// the same order as the configuration it was rendered from. Counted in BYTES, so the check
/// is the length of the text built so far rather than a walk over it.
const MAX_RENDERED_BYTES: usize = 8 * crate::ops::put_integration::MAX_TEMPLATE_CHARS;

/// Emulator resource guard, NOT an AWS quota: the work one rendering may do, counted in the
/// bytes it READS.
///
/// The one piece of work a reference can make large is a query of the PAYLOAD
/// (`$input.json`, `$input.path`): the payload is read as a JSON document once per
/// rendering ([`Render::document`]) and each query looks through it, so without a budget the
/// cost would be the product of two caller-chosen factors — how many such references the
/// stored template carries and how large the payload the request sent is. Charging each
/// query the payload's own size against one budget makes the whole rendering linear in what
/// the request carried, whatever the template asks for.
const MAX_RENDER_WORK: usize = 4 * 1024 * 1024;


/// `template` rendered against `inputs`, refusing outright a template that carries a
/// DIRECTIVE — the half of the language that is statements rather than values (see the
/// module docs).
///
/// The ONE entry point for a caller that renders a mapping template and reports both
/// refusals the same way. The REST data plane in this crate checks the two halves
/// separately, because it words them differently per integration type; another plane that
/// renders the same language — the WEBSOCKET data plane of API Gateway v2, whose `MOCK`
/// integration's request and response templates are the same `TemplateMap` values — wants
/// the whole rule in one call, and gets it here rather than keeping a second renderer.
pub fn rendered_without_directives(
    template: &str,
    inputs: &TemplateInputs,
) -> Result<String, Unrendered> {
    if super::invoke::carries_velocity_directive(template) {
        return Err(Unrendered {
            reason: "it carries a Velocity directive, which is a statement this emulator \
                     does not execute"
                .to_string(),
        });
    }
    rendered(template, inputs)
}

/// `template` rendered against `inputs`, or the first reason this emulator does not render
/// it.
///
/// The work is one pass over the template: every character is either copied or part of a
/// reference, and resolving a reference is a map lookup or one query of the payload — which
/// is read as a document once per rendering, not once per reference. So rendering costs a
/// constant per template character, plus one value per reference, plus the reads
/// [`MAX_RENDER_WORK`] bounds; the body it produces is bounded by [`MAX_RENDERED_BYTES`].
pub fn rendered(template: &str, inputs: &TemplateInputs) -> Result<String, Unrendered> {
    let glyphs: Vec<char> = template.chars().collect();
    let mut render = Render {
        inputs,
        document: None,
        work: MAX_RENDER_WORK,
    };
    let mut rendered = String::with_capacity(template.len());
    let mut at = 0usize;
    while at < glyphs.len() {
        if glyphs[at] != '$' {
            rendered.push(glyphs[at]);
            at += 1;
            continue;
        }
        // A `$` that does not open a reference is text: the language renders it as it
        // stands, which is what a price or a shell-looking literal in a body is.
        if !opens_reference(&glyphs, at) {
            rendered.push('$');
            at += 1;
            continue;
        }
        // From here it IS a reference, so a text this module cannot take apart leaves the
        // template unrendered rather than being copied through as if it were literal: a
        // half-read reference would put the template's own source in the body.
        let (reference, next) = reference_at(&glyphs, at, 0).ok_or_else(|| Unrendered {
            reason: format!(
                "the reference {} is not one this emulator reads",
                spelled(&glyphs, at)
            ),
        })?;
        rendered.push_str(&render.resolved(&reference)?);
        // The length of what has been built, not a walk over it: the check is O(1), so the
        // cap cannot itself become the quadratic cost it exists to prevent. One value may
        // carry the total past the cap, which is why it is checked after each one.
        if rendered.len() > MAX_RENDERED_BYTES {
            return Err(Unrendered {
                reason: format!(
                    "it renders to more than the {MAX_RENDERED_BYTES} bytes this emulator \
                     renders"
                ),
            });
        }
        at = next;
    }
    Ok(rendered)
}

/// One rendering in progress: the values it resolves against, the payload read as a
/// document at most ONCE, and what is left of the reads [`MAX_RENDER_WORK`] allows.
struct Render<'i> {
    inputs: &'i TemplateInputs,
    /// The payload as a JSON document: `None` until a reference needs it, `Some(None)` once
    /// it has been read and is not a JSON document (or there is no payload to read), so
    /// neither the parse nor the refusal happens twice.
    document: Option<Option<::serde_json::Value>>,
    /// How many more bytes this rendering may READ.
    work: usize,
}

impl Render<'_> {
    /// Charge `cost` bytes of reading to this rendering, or refuse it for having done all
    /// the work one request may cause.
    fn spend(&mut self, cost: usize) -> Result<(), Unrendered> {
        match self.work.checked_sub(cost) {
            Some(left) => {
                self.work = left;
                Ok(())
            }
            None => Err(Unrendered {
                reason: format!(
                    "rendering it reads more than the {MAX_RENDER_WORK} bytes one request \
                     may cause this emulator to read"
                ),
            }),
        }
    }

    /// The payload as the JSON document a `$input.json`/`$input.path` query is applied to,
    /// read on the first reference that needs it and kept for the rest of this rendering;
    /// `None` when there is no payload to read or it is not a JSON document.
    fn document(&mut self) -> Result<Option<&::serde_json::Value>, Unrendered> {
        if self.document.is_none() {
            let read = match self.inputs.payload.as_deref() {
                Some(payload) => {
                    // The parse looks through the whole payload, so it is charged for it
                    // once — and the queries below are charged again per query, which is
                    // what bounds the product this module could otherwise cost.
                    self.spend(payload.len())?;
                    ::serde_json::from_slice::<::serde_json::Value>(payload).ok()
                }
                None => None,
            };
            self.document = Some(read);
        }
        Ok(self.document.as_ref().and_then(Option::as_ref))
    }

    /// The size one query of the payload is charged: the payload's own length, which is
    /// what a query may look through.
    fn query_cost(&self) -> usize {
        self.inputs.payload.as_deref().map_or(0, <[u8]>::len)
    }
}

/// How deeply a reference's method arguments may nest before this module stops reading it.
///
/// An argument of a documented method is a quoted literal or one other reference
/// (`$util.urlEncode($input.params('x'))`), so two levels is already past every documented
/// form. The bound is what keeps the reading of a stored template — caller-sized data —
/// from recursing as deeply as that text nests.
const MAX_REFERENCE_DEPTH: usize = 8;

/// How much of a reference is quoted in the reason when nothing resolves it, so one
/// unreadable reference cannot copy a whole template into a log.
const SPELLED_GLYPHS: usize = 64;

/// Whether the `$` at `at` opens a REFERENCE: `$`, optionally `!`, optionally `{`, then a
/// name the language admits.
fn opens_reference(glyphs: &[char], at: usize) -> bool {
    let mut index = at + 1;
    if glyphs.get(index) == Some(&'!') {
        index += 1;
    }
    if glyphs.get(index) == Some(&'{') {
        index += 1;
    }
    glyphs
        .get(index)
        .is_some_and(|first| first.is_ascii_alphabetic() || *first == '_')
}

/// The reference at `at` as the template spells it, clipped to [`SPELLED_GLYPHS`].
fn spelled(glyphs: &[char], at: usize) -> String {
    glyphs
        .iter()
        .skip(at)
        .take(SPELLED_GLYPHS)
        .collect::<String>()
}

/// One reference of a template: the variable it names, the members and methods it reaches
/// through it, and the text the template wrote it as.
#[derive(Debug)]
struct Reference {
    /// The variable the reference opens with (`input`, `context`, `stageVariables`, …).
    variable: String,
    /// What it reaches through that variable, in order.
    steps: Vec<Step>,
    /// The reference exactly as the template wrote it, so a reference nothing resolves is
    /// reported as the developer spelled it.
    source: String,
}

/// One step of a reference: a member of what came before it, or a method called on it.
#[derive(Debug)]
enum Step {
    /// `.name`
    Member(String),
    /// `.name(argument, …)`
    Call(String, Vec<Argument>),
}

/// One argument of a called method: a quoted literal, or another reference.
#[derive(Debug)]
enum Argument {
    Literal(String),
    Reference(Reference),
}

/// The reference the template writes at `at` (the index of its `$`) and the index just
/// past it, or `None` when the text there is not a reference this module reads — which its
/// caller answers as a template it does not render.
///
/// Both spellings of the language are read: the bare `$name.member` and the braced
/// `${name.member}`, each optionally QUIET (`$!name`) — a quiet reference renders the empty
/// string where a plain one renders its own text, and since this module resolves a value or
/// refuses the whole template, the two forms resolve identically here.
///
/// `depth` is how many method arguments this reading is already inside
/// ([`MAX_REFERENCE_DEPTH`]).
fn reference_at(glyphs: &[char], at: usize, depth: usize) -> Option<(Reference, usize)> {
    if depth > MAX_REFERENCE_DEPTH {
        return None;
    }
    let mut index = at + 1;
    if glyphs.get(index) == Some(&'!') {
        index += 1;
    }
    let braced = glyphs.get(index) == Some(&'{');
    if braced {
        index += 1;
    }
    let (variable, next) = identifier_at(glyphs, index)?;
    index = next;
    let mut steps = Vec::new();
    loop {
        if glyphs.get(index) == Some(&'.') {
            let Some((name, after_name)) = identifier_at(glyphs, index + 1) else {
                break;
            };
            if glyphs.get(after_name) == Some(&'(') {
                let (arguments, after_call) = arguments_at(glyphs, after_name + 1, depth + 1)?;
                steps.push(Step::Call(name, arguments));
                index = after_call;
            } else {
                steps.push(Step::Member(name));
                index = after_name;
            }
            continue;
        }
        // The BRACKETED spelling of the same member, which is how a key that is not a bare
        // name is reached: `$context.authorizer.claims['cognito:username']`. A Cognito
        // authorizer publishes namespaced claims, whose `:` no identifier admits, so the
        // bracketed form is the only way a template can read one.
        if glyphs.get(index) == Some(&'[') {
            let Some((key, after_key)) = quoted_key_at(glyphs, index + 1) else {
                break;
            };
            if glyphs.get(after_key) != Some(&']') {
                break;
            }
            steps.push(Step::Member(key));
            index = after_key + 1;
            continue;
        }
        break;
    }
    if braced {
        if glyphs.get(index) != Some(&'}') {
            // An unterminated `${…` is a reference this module cannot read.
            return None;
        }
        index += 1;
    }
    let source: String = glyphs.get(at..index)?.iter().collect();
    Some((
        Reference {
            variable,
            steps,
            source,
        },
        index,
    ))
}

/// The `$context` member whose value is a MAP an authorizer returned rather than a fixed set
/// of members: `$context.authorizer`, whose `claims` a user-pool authorizer fills and whose
/// other keys a custom authorizer chooses. A key it does not carry is an absent value, so a
/// reference to one renders as nothing.
const AUTHORIZER_MEMBER: &str = "authorizer";

/// The identifier at `at` and the index just past it: a letter or `_`, then letters,
/// digits, `_` and `-`, which is what the language admits in a name.
fn identifier_at(glyphs: &[char], at: usize) -> Option<(String, usize)> {
    let first = glyphs.get(at)?;
    if !first.is_ascii_alphabetic() && *first != '_' {
        return None;
    }
    let mut index = at;
    let mut name = String::new();
    while let Some(glyph) = glyphs.get(index) {
        if glyph.is_ascii_alphanumeric() || *glyph == '_' || *glyph == '-' {
            name.push(*glyph);
            index += 1;
        } else {
            break;
        }
    }
    Some((name, index))
}

/// The quoted key at `at` and the index just past its closing quote: the subscript of a
/// bracketed member access (`['cognito:username']`, `["cognito:username"]`).
///
/// Either quote, as the language admits both, and the key is taken verbatim — a member name is
/// whatever the value it names is keyed by, which for a token's claim is the claim's own name.
/// `None` for a subscript that is not a quoted string, which leaves the text unread rather
/// than guessed at.
fn quoted_key_at(glyphs: &[char], at: usize) -> Option<(String, usize)> {
    let quote = *glyphs.get(at)?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut index = at + 1;
    let mut key = String::new();
    loop {
        let glyph = *glyphs.get(index)?;
        index += 1;
        if glyph == quote {
            break;
        }
        key.push(glyph);
    }
    (!key.is_empty()).then_some((key, index))
}

/// The arguments of a method call opened at `at` (just past its `(`), and the index just
/// past the closing `)`.
///
/// Each argument is a quoted literal or another reference, which are the two forms a
/// mapping template's documented methods take. Anything else leaves the whole text
/// unread, so a template this module cannot take apart is never rendered from a partial
/// reading of it. `depth` bounds how far a nested argument is followed
/// ([`MAX_REFERENCE_DEPTH`]).
fn arguments_at(glyphs: &[char], at: usize, depth: usize) -> Option<(Vec<Argument>, usize)> {
    if depth > MAX_REFERENCE_DEPTH {
        return None;
    }
    let mut index = at;
    let mut arguments = Vec::new();
    loop {
        while glyphs.get(index).is_some_and(|glyph| glyph.is_whitespace()) {
            index += 1;
        }
        match glyphs.get(index)? {
            ')' => return Some((arguments, index + 1)),
            ',' => index += 1,
            '\'' | '"' => {
                let quote = *glyphs.get(index)?;
                index += 1;
                let mut literal = String::new();
                loop {
                    let glyph = *glyphs.get(index)?;
                    index += 1;
                    if glyph == quote {
                        break;
                    }
                    literal.push(glyph);
                }
                arguments.push(Argument::Literal(literal));
            }
            '$' => {
                let (reference, next) = reference_at(glyphs, index, depth + 1)?;
                arguments.push(Argument::Reference(reference));
                index = next;
            }
            _ => return None,
        }
    }
}

impl Render<'_> {
    /// The value `reference` names, or the reference itself when this emulator resolves
    /// none.
    ///
    /// Every variable below is one API Gateway documents a mapping template against, and
    /// every method is one that variable documents; a reference outside that set is
    /// answered as unresolved rather than rendered as something close to it.
    fn resolved(&mut self, reference: &Reference) -> Result<String, Unrendered> {
        let unresolved = || Unrendered {
            reason: format!(
                "it names {}, which this emulator resolves no value of this call for",
                reference.source
            ),
        };
        match reference.variable.as_str() {
            // "$input" — "represents the method request payload and parameters to be
            // processed by the mapping template".
            "input" => match reference.steps.as_slice() {
                // `$input.body` — "returns the raw request payload as a string".
                [Step::Member(member)] if member == "body" => {
                    let bytes = self
                        .inputs
                        .payload
                        .as_deref()
                        .map(<[u8]>::len)
                        .ok_or_else(unresolved)?;
                    self.spend(bytes)?;
                    let payload = self.inputs.payload.as_deref().unwrap_or_default();
                    // As TEXT, which is what the reference stands for: a payload that is
                    // not text is left unresolved rather than written out with its
                    // undecodable bytes replaced, which would put a body in the response
                    // that the caller never sent.
                    ::std::str::from_utf8(payload)
                        .map(str::to_string)
                        .map_err(|_| Unrendered {
                            reason: format!(
                                "it names {}, and this call's payload is not text",
                                reference.source
                            ),
                        })
                }
                // `$input.json(x)` — "evaluates a JSONPath expression and returns the
                // results as a JSON string": the selected node written as JSON.
                [Step::Call(method, arguments)] if method == "json" => {
                    let path = self.one_argument(arguments)?.ok_or_else(unresolved)?;
                    match self.queried(&path)?.ok_or_else(unresolved)? {
                        Some(node) => Ok(node.to_string()),
                        // A path that selects nothing writes nothing.
                        None => Ok(String::new()),
                    }
                }
                // `$input.path(x)` — "takes a JSONPath expression string and returns a
                // JSON object representation of the result". A selected SCALAR is that
                // value; an object or an array is a Java object whose own rendering AWS
                // does not state, so it is left unresolved rather than written as
                // something else.
                [Step::Call(method, arguments)] if method == "path" => {
                    let path = self.one_argument(arguments)?.ok_or_else(unresolved)?;
                    match self.queried(&path)?.ok_or_else(unresolved)? {
                        Some(::serde_json::Value::String(text)) => Ok(text),
                        Some(
                            node @ (::serde_json::Value::Number(_) | ::serde_json::Value::Bool(_)),
                        ) => Ok(node.to_string()),
                        Some(::serde_json::Value::Null) | None => Ok(String::new()),
                        Some(_) => Err(unresolved()),
                    }
                }
                // `$input.params(x)` — "returns the method request parameter value from the
                // path, query string, or header (in that order) given a parameter name".
                [Step::Call(method, arguments)] if method == "params" => {
                    let name = self.one_argument(arguments)?.ok_or_else(unresolved)?;
                    Ok(parameter(self.inputs, &name).unwrap_or_default())
                }
                _ => Err(unresolved()),
            },
            // "$stageVariables" — the stage's own map; `.<name>` and `.get('<name>')` name
            // the same entry. A name the stage defines no value for resolves to nothing at
            // all, which is how the rest of this data plane reads a reference to one.
            "stageVariables" => {
                let name = match reference.steps.as_slice() {
                    [Step::Member(name)] => name.clone(),
                    [Step::Call(method, arguments)] if method == "get" => {
                        self.one_argument(arguments)?.ok_or_else(unresolved)?
                    }
                    _ => return Err(unresolved()),
                };
                self.inputs
                    .stage_variables
                    .get(&name)
                    .cloned()
                    .ok_or_else(unresolved)
            }
            // "$context" — "contains the contextual information of your request". Only the
            // members this call has a value for resolve, with the one exception the
            // `authorizer` map below states.
            "context" => {
                let mut named = Vec::with_capacity(reference.steps.len());
                for step in &reference.steps {
                    match step {
                        Step::Member(name) => named.push(name.clone()),
                        // `get('name')` is the method a MAP of the context is read by, which
                        // is the other spelling of the bracketed member access above — the
                        // same member, named by its key.
                        Step::Call(method, arguments) if method == "get" => {
                            match self.one_argument(arguments)? {
                                Some(key) => named.push(key),
                                None => return Err(unresolved()),
                            }
                        }
                        Step::Call(_, _) => return Err(unresolved()),
                    }
                }
                self.inputs
                    .context
                    .get(&named.join("."))
                    .cloned()
                    .or_else(|| {
                        // `$context.authorizer` is not a fixed set of members: it is the MAP
                        // an authorizer returned as the context of its decision — its
                        // `claims` for a user-pool authorizer, whatever a custom one put
                        // there — so a key it does not carry is an ABSENT VALUE rather than a
                        // reference naming something outside the language. It renders as
                        // nothing, the way a lookup in every other map of the call does
                        // (`$input.params` of a parameter the request did not send).
                        named
                            .first()
                            .is_some_and(|first| first == AUTHORIZER_MEMBER)
                            .then(String::new)
                    })
                    .ok_or_else(unresolved)
            }
            // "$util" — "contains utility functions", each documented with the conversion
            // it performs. The three whose output a published standard STATES exactly are
            // performed — the two form-encoding conversions by the maintained coder for that
            // format, and the JavaScript string escape by the rule that language's own
            // grammar gives ([`escaped_as_javascript`]); every other `$util` method
            // (`parseJson`, `validateProperty`) stays unresolved, for the reason the module
            // docs give.
            "util" => match reference.steps.as_slice() {
                [Step::Call(method, arguments)] => {
                    let text = self.one_argument(arguments)?.ok_or_else(unresolved)?;
                    self.spend(text.len())?;
                    match method.as_str() {
                        // "$util.urlEncode(String) — Converts a string into
                        // 'application/x-www-form-urlencoded' format."
                        "urlEncode" => Ok(::form_urlencoded::byte_serialize(text.as_bytes())
                            .collect::<String>()),
                        // "$util.urlDecode(String) — Decodes an
                        // 'application/x-www-form-urlencoded' string back into its
                        // original form." The decoder is the framework's one QUERY decoder,
                        // which resolves every escape and folds `+` to a space as that
                        // format's rule is.
                        "urlDecode" => Ok(::smithy_server_runtime::percent_decode(&text)),
                        // "$util.escapeJavaScript() — Escapes the characters in a string
                        // using JavaScript string rules", which is what lets a template put
                        // a caller's own text inside a quoted string of the document it
                        // renders.
                        "escapeJavaScript" => Ok(escaped_as_javascript(&text)),
                        _ => Err(unresolved()),
                    }
                }
                _ => Err(unresolved()),
            },
            _ => Err(unresolved()),
        }
    }

    /// The single argument a one-argument method was called with, resolved to its text, or
    /// `None` when it was called with no argument or with several.
    fn one_argument(&mut self, arguments: &[Argument]) -> Result<Option<String>, Unrendered> {
        match arguments {
            [Argument::Literal(literal)] => Ok(Some(literal.clone())),
            // An argument that is itself a reference is resolved first, so
            // `$input.params($stageVariables.name)` names the parameter that variable
            // names.
            [Argument::Reference(reference)] => self.resolved(reference).map(Some),
            _ => Ok(None),
        }
    }

    /// The node `path` selects in the payload, or `None` for a payload or a path this
    /// emulator cannot query — and `Some(None)` for a path that selects nothing.
    ///
    /// The path is read and evaluated by `service_common`'s ONE bounded single-value JSON
    /// path reader, which is where this emulator's rule for such a path lives: a path is
    /// accepted only as a REFERENCE path (`$`, `.name`, `['name']`, `[index]`, a bounded
    /// number of them), its segments are what the maintained query engine is handed, and a
    /// path naming more than one place is refused rather than evaluated. That is also what
    /// makes one query's cost the document it looks through — the cost each query is
    /// charged ([`Render::query_cost`]) — because each accepted segment selects at most one
    /// node from at most one node.
    ///
    /// Three answers, told apart: a path that reader does not evaluate leaves the template
    /// unrendered, a path that selects no place writes nothing, and a path that names one
    /// value writes it.
    fn queried(&mut self, path: &str) -> Result<Option<Option<::serde_json::Value>>, Unrendered> {
        let Some(segments) = ::service_common::json_reference_path::segments(path) else {
            return Ok(None);
        };
        self.spend(self.query_cost())?;
        let Some(document) = self.document()? else {
            return Ok(None);
        };
        Ok(Some(
            ::service_common::json_reference_path::select_segments(&segments, document).cloned(),
        ))
    }
}

/// The value of the method request parameter `name`: the path parameter, then the query
/// string, then the header, which is the order the method documents. A header is matched
/// case-insensitively, because a header name is.
fn parameter(inputs: &TemplateInputs, name: &str) -> Option<String> {
    if let Some(bound) = inputs.path.get(name) {
        return Some(bound.clone());
    }
    if let Some(sent) = inputs.querystring.get(name) {
        return Some(sent.clone());
    }
    inputs
        .header
        .iter()
        .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// `text` with the characters a JavaScript string literal cannot carry as they stand
/// replaced by their escapes, which is what `$util.escapeJavaScript` performs.
///
/// The rule is ECMAScript's own string-literal grammar, which states exactly which
/// characters need an escape and what each escape is: the quote characters and the escape
/// character itself, the five escapes the grammar names (`\b`, `\t`, `\n`, `\f`, `\r`), and
/// every other control character as the `\uXXXX` the grammar spells it with. The forward
/// slash is escaped too, because a rendering of this is embedded in a document a script may
/// carry and `</` has to stay inert there. Nothing else is touched: a template that escapes
/// text and one that writes it put the same characters on the wire.
fn escaped_as_javascript(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\'' => escaped.push_str("\\'"),
            '\\' => escaped.push_str("\\\\"),
            '/' => escaped.push_str("\\/"),
            '\u{8}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{c}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            other if other.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", u32::from(other)));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inputs the cases below render against: one of each kind of value a template can
    /// name.
    fn inputs() -> TemplateInputs {
        TemplateInputs {
            payload: Some(br#"{"id":"a1","qty":2,"nested":{"deep":"value"}}"#.to_vec()),
            path: [("pet".to_string(), "fido".to_string())].into_iter().collect(),
            querystring: [("tenant".to_string(), "acme".to_string())]
                .into_iter()
                .collect(),
            header: [("X-Trace".to_string(), "t-1".to_string())]
                .into_iter()
                .collect(),
            stage_variables: [("backend".to_string(), "beta".to_string())]
                .into_iter()
                .collect(),
            context: [
                ("httpMethod".to_string(), "POST".to_string()),
                ("stage".to_string(), "test".to_string()),
                (
                    "requestId".to_string(),
                    "6c3e2a77-0000-4000-8000-9f1c2d3e4f50".to_string(),
                ),
                (
                    "authorizer.claims.sub".to_string(),
                    "6b1d0f0e-user".to_string(),
                ),
                (
                    "authorizer.claims.cognito:username".to_string(),
                    "alice".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn every_documented_variable_renders_the_value_the_call_carries() {
        let cases: &[(&str, &str)] = &[
            // Literal text, including a `$` that opens no reference.
            ("{\"literal\":true}", "{\"literal\":true}"),
            ("cost is $12", "cost is $12"),
            // The method request.
            (r#"{"tenant":"$input.params('tenant')"}"#, r#"{"tenant":"acme"}"#),
            (r#"$input.params("X-TRACE")"#, "t-1"),
            ("$input.params('pet')", "fido"),
            ("$input.params('absent')", ""),
            ("${input.params('tenant')}", "acme"),
            ("$!input.params('tenant')", "acme"),
            ("$input.body", r#"{"id":"a1","qty":2,"nested":{"deep":"value"}}"#),
            ("$input.json('$.id')", "\"a1\""),
            ("$input.json('$.qty')", "2"),
            ("$input.path('$.id')", "a1"),
            ("$input.path('$.qty')", "2"),
            ("$input.path('$.nested.deep')", "value"),
            ("$input.json('$.missing')", ""),
            // The two form-encoding conversions and the JavaScript string escape, which is
            // what puts a caller's own text inside a quoted string of a rendered document.
            ("$util.urlEncode('a b&c')", "a+b%26c"),
            ("$util.urlDecode('a+b%26c')", "a b&c"),
            (
                r#"{"lastBody":"$util.escapeJavaScript($input.body)"}"#,
                r#"{"lastBody":"{\"id\":\"a1\",\"qty\":2,\"nested\":{\"deep\":\"value\"}}"}"#,
            ),
            // The stage and the context.
            ("$stageVariables.backend", "beta"),
            ("$stageVariables.get('backend')", "beta"),
            ("$context.httpMethod/$context.stage", "POST/test"),
            // The id the call is answered under, which the data plane carries for every
            // request it serves.
            ("$context.requestId", "6c3e2a77-0000-4000-8000-9f1c2d3e4f50"),
            // An authorizer's claims, by the bare member path and by the two spellings a
            // claim whose name is no bare identifier is reached by.
            ("$context.authorizer.claims.sub", "6b1d0f0e-user"),
            ("$context.authorizer.claims['cognito:username']", "alice"),
            ("$context.authorizer.claims[\"cognito:username\"]", "alice"),
            ("$context.authorizer.claims.get('cognito:username')", "alice"),
            ("${context.authorizer.claims['cognito:username']}", "alice"),
            // A key the authorizer's context map does not carry is an absent VALUE, so it
            // renders as nothing — the same way a parameter the request did not send does.
            ("$context.authorizer.claims.email", ""),
            ("$context.authorizer.principalId", ""),
        ];
        let inputs = inputs();
        for (template, expected) in cases {
            let rendered = rendered(template, &inputs);
            assert_eq!(
                rendered.as_deref().ok(),
                Some(*expected),
                "case {template:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// RULE: `$util.escapeJavaScript` replaces exactly the characters a JavaScript string
    /// literal cannot carry as they stand, each with the escape that grammar names.
    #[test]
    fn the_javascript_escape_replaces_what_a_string_literal_cannot_carry() {
        /// (case, the text, its escaping)
        type Row<'a> = (&'a str, &'a str, &'a str);
        let rows: &[Row<'_>] = &[
            ("text needing no escape at all", "a-b_c 1", "a-b_c 1"),
            ("a quoted document", r#"{"a":1}"#, r#"{\"a\":1}"#),
            ("an apostrophe", "it's", r"it\'s"),
            ("a backslash", r"a\b", r"a\\b"),
            ("a forward slash", "a/b", r"a\/b"),
            ("the named control escapes", "a\tb\nc\rd", r"a\tb\nc\rd"),
            ("a control character the grammar spells", "a\u{1}b", r"a\u0001b"),
            ("text past ASCII, which needs none", "café", "café"),
            ("nothing at all", "", ""),
        ];
        for (case, text, expected) in rows {
            let actual = escaped_as_javascript(text);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_reference_nothing_resolves_leaves_the_template_unrendered() {
        let cases: &[&str] = &[
            // A variable outside the documented set, and a transformation this emulator
            // states rather than performs.
            "$util.parseJson($input.body)",
            "$whatever.body",
            // A member of a documented variable that this call has no value for.
            "$context.status",
            "$stageVariables.undefined",
            // A method the variable does not document, and one called wrongly.
            "$input.query('$.id')",
            "$input.params()",
            // A path selecting a structure, and one selecting several nodes.
            "$input.path('$.nested')",
            "$input.json('$.*')",
            // A path carrying a construct whose work is per node rather than per document
            // — a filter, a descendant segment, a wildcard, a selector list, a slice — one
            // that is not rooted at the payload, and one that is not a path at all: the
            // single-value reader evaluates none of them.
            "$input.json('$[?@.id == \"a1\"]')",
            "$input.json('$..deep')",
            "$input.json('$..nested..deep')",
            "$input.json('$[0,0]')",
            "$input.json('$[0:2]')",
            "$input.json('$[*]')",
            "$input.json('$.*')",
            "$input.json('id')",
            "$input.json('@.id')",
            // A reference this module cannot take apart at all: an unterminated brace, an
            // unterminated argument, an argument in neither documented form, and
            // arguments nested past the bound this reading is done under.
            "${input.params('tenant')",
            "$input.params('tenant",
            "$input.params(tenant)",
            "$a.b($a.b($a.b($a.b($a.b($a.b($a.b($a.b($a.b($a.b('x')))))))))",
        ];
        let inputs = inputs();
        for template in cases {
            let rendered = rendered(template, &inputs);
            assert!(
                rendered.is_err(),
                "case {template:?}: expected the template to be left unrendered, got \
                 {rendered:?}"
            );
        }
        // And a path the single-value reader does not evaluate because of its SIZE: more
        // segments than it reads, built rather than written out.
        let deep = format!(
            "$input.json('${}')",
            ".a".repeat(::service_common::json_reference_path::MAX_SEGMENTS + 1)
        );
        let stepped = rendered(&deep, &inputs);
        assert!(
            stepped.is_err(),
            "case a path past the segments the reader evaluates: expected the template to \
             be left unrendered, got {stepped:?}"
        );
    }

    #[test]
    fn only_the_reference_path_forms_are_applied_to_the_payload() {
        // The forms a mapping template's documented path takes are the REFERENCE paths
        // `service_common`'s single-value reader evaluates, and the constructs it refuses
        // are the ones whose work is per node rather than per document. Asserted through
        // the rendering, which is what this module's cost argument rests on: the rows
        // differ along the step kind (member, index, quoted member), how many steps are
        // chained, and which construct puts a path outside the set.
        let inputs = inputs();
        let applied: &[(&str, &str)] = &[
            ("$input.json('$.id')", "\"a1\""),
            ("$input.json('$.nested.deep')", "\"value\""),
            (r#"$input.json("$['id']")"#, "\"a1\""),
            ("$input.path('$.nested.deep')", "value"),
            // A place the payload does not carry: evaluated, and selected nothing.
            ("$input.json('$.absent')", ""),
            ("$input.json('$.nested.absent')", ""),
        ];
        for (template, expected) in applied {
            let rendered = rendered(template, &inputs);
            assert_eq!(
                rendered.as_deref().ok(),
                Some(*expected),
                "case {template:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    #[test]
    fn a_rendering_past_the_emulators_own_bounds_leaves_the_template_unrendered() {
        // Two bounds, each on the PRODUCT of the references a template carries and the
        // payload one of them names: the BODY it produces, and the bytes it READS. A small
        // template must not be able to amplify one request into an unbounded one either
        // way.
        let inputs = inputs();
        let payload = inputs.payload.as_deref().unwrap_or_default().len();
        // The body: every reference writes the payload out again.
        let references = MAX_RENDERED_BYTES / payload + 2;
        let template = "$input.body".repeat(references);
        let refused = rendered(&template, &inputs);
        assert!(
            refused.is_err(),
            "case {references} payload references: expected the rendering to be refused \
             past {MAX_RENDERED_BYTES} bytes, got {} bytes",
            refused.map(|body| body.len()).unwrap_or_default()
        );
        // The reads: every reference queries the payload, and writes almost nothing.
        let queries = MAX_RENDER_WORK / payload + 2;
        let template = "$input.json('$.qty')".repeat(queries);
        let refused = rendered(&template, &inputs);
        assert!(
            refused.is_err(),
            "case {queries} payload queries: expected the rendering to be refused past \
             {MAX_RENDER_WORK} bytes read, got {} bytes",
            refused.map(|body| body.len()).unwrap_or_default()
        );
        // And one that stays inside both renders.
        let template = "$input.body".repeat(2);
        let twice = rendered(&template, &inputs);
        assert_eq!(
            twice.as_deref().ok().map(str::len),
            Some(payload * 2),
            "case two payload references: expected {} characters, got {twice:?}",
            payload * 2
        );
    }

    #[test]
    fn a_payload_that_is_not_text_leaves_a_payload_reference_unrendered() {
        // `$input.body` stands for the payload AS TEXT, so a payload that is not text is
        // left unrendered rather than written out with its undecodable bytes replaced.
        let inputs = TemplateInputs {
            payload: Some(vec![0x7b, 0x22, 0xff, 0xfe, 0x22, 0x7d]),
            ..inputs()
        };
        let refused = rendered("$input.body", &inputs);
        assert!(
            refused.is_err(),
            "case a non-text payload: expected the template to be left unrendered, got \
             {refused:?}"
        );
        // The parameters of the same call are still text to read.
        let parameter = rendered("$input.params('tenant')", &inputs);
        assert_eq!(
            parameter.as_deref().ok(),
            Some("acme"),
            "case params beside a non-text payload: expected acme, got {parameter:?}"
        );
    }

    #[test]
    fn a_payload_reference_is_unresolved_where_the_call_carries_no_payload_to_read() {
        let inputs = TemplateInputs {
            payload: None,
            ..inputs()
        };
        for template in ["$input.body", "$input.json('$.id')", "$input.path('$.id')"] {
            let rendered = rendered(template, &inputs);
            assert!(
                rendered.is_err(),
                "case {template:?}: expected no payload to leave the template unrendered, \
                 got {rendered:?}"
            );
        }
        // The parameters of the same call are still there to read.
        let rendered = rendered("$input.params('tenant')", &inputs);
        assert_eq!(
            rendered.as_deref().ok(),
            Some("acme"),
            "case params without a payload: expected acme, got {rendered:?}"
        );
    }
}
