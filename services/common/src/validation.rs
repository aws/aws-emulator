// SPDX-License-Identifier: CC0-1.0

//! The value a constraint-violation message echoes: the one the caller submitted.
//!
//! An AWS constraint-validation message is BUILT FROM THE REQUEST —
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>` — so the value between the quotes is part of the rule, not
//! prose: the message is the only place the server reports the offending input back, and
//! a caller that compares the echo against what it sent must see its own bytes.
//!
//! The framework's constraint walk
//! ([`aws_smithy_schema_server::constraints::validate`]) stores an ELIDED rendering on
//! [`Violation::value`] — a byte-bounded prefix followed by `... [<n> bytes]` — because
//! the clause interpolates whatever is stored and a caller controls the length. That
//! marker is wire text AWS never emits, so a service that words its own refusal from
//! those violations echoes a value the caller never submitted.
//!
//! [`with_submitted_values`] closes that gap WITHOUT re-implementing any constraint
//! check: it re-walks the SAME built input the walk validated, collects every
//! string-rendered leaf under the path spelling
//! [`aws_smithy_schema_server::constraints::path`] defines, and replaces a violation's
//! stored echo with the leaf it was elided from. A stored echo is replaced only when it
//! IS that leaf's elision (identical, or a proper prefix plus the marker naming that
//! leaf's byte count), so a path whose leaf is unrelated keeps the framework's own text.
//! [`refusing_violations`] is the step before it: the violations of one request, collected
//! from the framework's own deserializer and constraint walk and handed back with their
//! echoes already restored, for a service that words its own pre-handler refusal.
//!
//! Only string-rendered leaves are collected, because they are the only ones whose echo
//! is the caller's own bytes: the walk renders a blob as `<n> bytes`, a list as
//! `<n> items`, a map as `<n> entries` and a document as `<document>`, each a bounded
//! summary of the value rather than the value, and a numeric or timestamp leaf is already
//! far shorter than any elision cap. A blob still has a CLAUSE RENDERING — the buffer it
//! was read into ([`rendered_blob`]), which states its length and nothing of its bytes — so
//! a structure carrying one, and the list that structure belongs to, render.
//!
//! # A clause on the collection itself
//!
//! `@length` on a LIST is checked on the collection member, before the walk descends into
//! its elements, and the only rendering the walk has for a collection is the count —
//! `<n> items`. AWS echoes the member's VALUE there too, rendering the list the way a list
//! renders: `[<element>, <element>, …]`. So a clause whose stored echo is that summary is
//! answered from the element leaves already collected one step deeper
//! (`<list>.<1-based position>.member`), and only when ALL of them were collected — the
//! count the summary states equals the number recorded — so a list whose elements are not
//! string-rendered (a list of structures), or one the leaf caps truncated, keeps the
//! framework's own summary rather than an echo stating fewer elements than were sent.
//!
//! A MAP member is the same story with the other summary — `<n> entries` — and the value
//! AWS echoes there is the map, rendered the way a map renders: `{<key>=<value>, …}`
//! ([`rendered_map`]). It is rendered only for a caller that asks
//! ([`refusing_violations_with_mapped_echoes`]), so a service whose recordings show the
//! valueless clause keeps it.
//!
//! # The bounds this module states
//!
//! The framework's cap is a RESOURCE bound as well as a rendering choice: a request whose
//! members are large strings would otherwise be re-echoed into an arbitrarily large
//! response, once per clause that fired, and the violation COUNT cap
//! (`MAX_VIOLATIONS` = 100) bounds only one factor of that product. Restoring the value
//! therefore comes with its own finite bounds, chosen here and not derived from the
//! request:
//!
//! * [`MAX_RESTORED_ECHO`] (16 KiB) — the largest single value restored into a clause by
//!   default. 64x the largest `@length` maximum most emulated members declare (256), so the
//!   fidelity this exists for — echoing a 300-character id, an over-long ARN, a rejected
//!   enum value — always lands, while one clause can never carry request-sized content. A
//!   service whose model declares a member LONGER than this (a policy document, a stylesheet)
//!   states its own ceiling from that member's declared maximum
//!   ([`refusing_violations_quoting_up_to`], or
//!   [`refusing_violations_with_declared_echoes_quoting_up_to`] for a service that reads the
//!   echo rather than the restored violation), which is a model constant rather than a
//!   request-sized one. A stated ceiling raises the per-message total with it, and bounds one
//!   LEAF value only: a COMPOSITE's rendering keeps this ceiling, since it is built from many
//!   of a caller's values rather than being one of them. The same seams serve the OTHER
//!   direction: a service whose model bounds the member a refusal is CARRIED in — the prose
//!   member of its error shapes — states its ceiling from THAT maximum, so a clause fits the
//!   member that has to hold it instead of overrunning it sixteenfold. Either way the number
//!   is read off a model and the default is what a service that states nothing is held to. A
//!   service whose recordings quote a whole COLLECTION whose rendering is larger than this
//!   states that ceiling separately
//!   ([`refusing_violations_with_declared_echoes_rendering_up_to`]), from the declared
//!   maximum of the collection in question, for the same reason and under the same rule:
//!   the number is a model constant, and past it the framework's own count summary stands.
//! * [`MAX_RESTORED_TOTAL`] (64 KiB) — the restored bytes ALL clauses of one message may
//!   carry together, so `violations x value size` is bounded as a product. For scale, the
//!   framework's own ceiling on the same text is 100 clauses x 256 bytes ~= 25 KiB, so
//!   this is the same order of magnitude rather than a new amplification class.
//! * [`MAX_RECORDED_LEAVES`] (256) and [`MAX_RECORDED_BYTES`] (64 KiB) — the leaf count
//!   and the owned bytes (paths plus values) this walk may collect from one request. The
//!   leaf count is over every leaf VISITED, kept or dropped, so the traversal stops at the
//!   cap even for a request whose every value is too large to keep; the walk skips any
//!   leaf that does not fit the byte budget, so the collection is never a second full copy
//!   of the request's strings. A rendered composite is bounded the same way, by child
//!   count and by the bytes its children hold ([`MAX_RENDERED_CHILDREN`]). A service whose
//!   model declares a COLLECTION longer than that count states its own position ceiling
//!   from that collection's declared maximum ([`refusing_violations_with_echoes_over`]),
//!   which is a model constant rather than a request-sized one: a composite renders only
//!   from children the walk reached, so a collection past the count could not be echoed at
//!   all.
//!
//! * [`MAX_RECORDED_DEPTH`] (64) — how deep into nested composites the walk descends. The
//!   leaf count bounds LEAF positions, which a value made only of composites never spends,
//!   and a model may declare a recursive shape; so the depth is bounded by its own stated
//!   constant, and a value nested past it is left to the framework's own text exactly as a
//!   spent leaf budget leaves it.
//!
//! Beyond any of these the framework's elided text stands: a shortened echo is a
//! divergence in one message, while an unbounded one is a response sized by the request.
//!
//! # Why this lives here at all
//!
//! The structurally right place for this is the frozen crate: carry the unelided value
//! beside the elided one on `Violation`, or elide in `Violation::message` rather than
//! inside `LengthTrait::check` / `PatternTrait::check`, so no re-walk is needed. `vendor/`
//! is read-only here, so that change is filed as
//! `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`.
//! Until it lands, the walk below MIRRORS that crate's private path bookkeeping
//! (`ValidatingSerializer`), and the two can drift: the drift is fail-safe — a path that
//! no longer matches yields no leaf, so the framework's own text stands — but this module
//! is to be deleted, not copied, once the core seam exists.
//!
//! # What of a validation message this module now owns, and what is still to come
//!
//! Four parts of a validation-exception message are the same for EVERY service, and three
//! of them live here:
//!
//! * which value a clause echoes — the one the caller submitted ([`with_submitted_values`]),
//!   and, for a clause stated at a collection member, the collection itself
//!   ([`submitted_lists`]), rendered over the members a structure is printed with: the ones
//!   the request SET, every one the shape DECLARES, or the ones the SERVICE states for a
//!   layer that prints members no model declares
//!   ([`refusing_violations_with_printed_echoes_over`]);
//! * a `@pattern` printed as the expression a value has to satisfy, without the `^`/`$`
//!   anchors a model writes to demand a whole-string match ([`unanchored`],
//!   [`unanchored_grouping_alternation`] for the services whose recordings show the pair
//!   dropped from a top-level alternation, where the alternation is kept grouped so the
//!   expression still matches what it matched anchored, and
//!   [`unanchored_keeping_inner_anchors`] for the services whose refusals quote an
//!   expression that anchors its own BRANCHES as the model declares it,
//!   [`unanchored_at_the_close`] for the services whose refusals drop only the anchor that
//!   CLOSES the expression, and
//!   [`whole_value_expression`] for the services whose refusals quote the expression the
//!   WHOLE value must match, deciding the two ends independently) — LANDED here,
//!   while ~26 service crates still carry their own inlined copy
//!   (`grep -rln 'fn unanchored' services/*/src`), each pinned by that service's own
//!   recordings and tests; deleting those copies in favour of this one is the migration's
//!   own change, and nothing new should inline the rule again. The copies that are not
//!   wired to this one still decline to drop the pair around a top-level alternation, which
//!   the recordings show AWS dropping ([`unanchored`] explains why); that difference goes
//!   away as each is deleted;
//! * the subject half's one fact a violation cannot carry: WHETHER the member was absent
//!   or was present with no text rendering of its own, so a service that states
//!   `Value at '<path>' …` for a present-but-unquotable value can still state
//!   `Value null at '<path>' …` for an absent one ([`SubmittedEcho`],
//!   [`with_submitted_echoes`]) — which value a clause then quotes, and how long a value it
//!   is willing to quote, stays with the service.
//!
//! One part still belongs here and is not here yet:
//!
//! * the requirement half, read off the framework's own wording from a violation with NO
//!   value attached and split at the LAST clause marker, so neither the rejected value
//!   nor a caller-chosen map key in the member path can reach it.
//!
//! How the clauses are then counted, ordered and joined into an envelope is a service
//! DIALECT (`N validation error(s) detected: …,…` for Amazon EVS,
//! `N validation errors detected: …; …` elsewhere, and `artifact` prints a `@pattern`
//! WITH its anchors), so that half stays with the service — as does which values an enum's
//! value set lists and in which order.

use std::collections::{HashMap, HashSet};

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, path, Violation, ViolationKind};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use smithy_server_runtime::{DeserializableStruct, Operation, ServerHttpProtocol};

/// The greatest number of violations one refusal states.
///
/// The framework's constraint walk stops collecting at this many
/// (`MAX_VIOLATIONS` in [`aws_smithy_schema_server::constraints`], which is private), so a
/// service rendering its own refusal from [`refusing_violations`] can never be handed
/// more. It is restated here, once, because a refusal body renders each violation at least
/// twice — as a clause of the message and, where the shape models one, as a field-list
/// entry — so a service that bounds its own rendering needs the same number the walk used,
/// rather than a literal of its own that can drift from it
/// (`grep -rn 'MAX_VIOLATIONS' services/*/src` lists the copies left to migrate here).
pub const MAX_VIOLATIONS: usize = 100;

/// As [`refusing_violations`], with the largest single value one clause may quote stated
/// by the SERVICE rather than taken from this module's own [`MAX_RESTORED_ECHO`].
///
/// What it is for: the flat bound is sized for the members most models declare, and a
/// model that declares a `@length` maximum ABOVE it has refusals whose value could never
/// be quoted in full — the one thing an AWS constraint message always does. A service
/// whose model declares such a member states its own ceiling here, from that member's
/// declared maximum, so the ceiling is a MODEL constant and no response is sized by the
/// request. Everything else is [`refusing_violations`]'s: the same collecting walk, the
/// same violations, the same per-message total ([`MAX_RESTORED_TOTAL`]), which still
/// bounds `violations x value size` as a product.
#[must_use]
pub fn refusing_violations_quoting_up_to<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_echo: usize,
) -> Vec<Violation> {
    violations_in::<O>(protocol, req, Echoes::SubmittedUpTo(max_echo)).violations
}

/// Every violation that refuses `req` before `O`'s handler runs — a `@required` member
/// absent, or a value outside a declared constraint — with each value echo restored to the
/// bytes the caller sent ([`with_submitted_values`]).
///
/// Empty when the request satisfies the model, and empty for a failure that is NOT a
/// refusal (a body the codec cannot read at all, a member of the wrong type), which the
/// framework's own walk answers in its own envelope.
///
/// This is the collecting half every service that words its own pre-handler refusal needs:
/// both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so a caller cannot find a different set
/// of violations from the walk that follows it, and the ONE place a request has to be
/// re-read to restore an echo is here. What is then DONE with them — which envelope, which
/// error code, which prose — is the service's own, so it stays with the service.
#[must_use]
pub fn refusing_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    violations_in::<O>(protocol, req, Echoes::Submitted).violations
}

/// As [`refusing_violations`], and with it every list of SCALAR elements the request
/// carried — keyed by the member path the walk reports that list at, rendered as the value
/// AWS states AT that member ([`submitted_lists`]).
///
/// For a service whose refusal states a constraint broken by a list's ELEMENTS at the LIST
/// member: the framework reports such a violation at the element, while AWS reports it at
/// the list and echoes the value that member holds — the whole list, which no violation
/// carries. Collected together with the violations because both read the SAME built input,
/// which is alive only inside this walk, and collected ONLY when a violation names a list
/// element, so a request that satisfies the model pays for no second walk.
///
/// The map is EMPTY when the walk could not collect the request's leaves in full, so a
/// clause can never state a list the caller did not send; see [`submitted_lists`].
#[must_use]
pub fn refusing_violations_with_lists<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> (Vec<Violation>, HashMap<String, String>) {
    let refusing = violations_in::<O>(protocol, req, Echoes::SubmittedWithLists);
    (refusing.violations, refusing.lists)
}

/// [`refusing_violations_with_lists`], quoting one value up to the ceiling the SERVICE
/// states ([`refusing_violations_quoting_up_to`]) instead of [`MAX_RESTORED_ECHO`].
///
/// The two choices a service makes about its clauses' value halves are independent — how
/// much of ONE value a clause quotes, and whether the clause stated at a LIST member can
/// quote the whole list — so a service that makes both reads them out of the SAME walk here
/// rather than reading the request twice. Everything else is
/// [`refusing_violations_with_lists`]'s: the same violations, the same lists, collected only
/// when a violation names a list element.
#[must_use]
pub fn refusing_violations_with_lists_quoting_up_to<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_echo: usize,
) -> (Vec<Violation>, HashMap<String, String>) {
    let refusing = violations_in::<O>(protocol, req, Echoes::SubmittedWithListsUpTo(max_echo));
    (refusing.violations, refusing.lists)
}

/// Every violation that refuses `req` before `O`'s handler runs, with the framework's own
/// rendering of each value kept — except where the validating walk WITHHELD it, which is
/// put back as that walk itself would have stored it ([`with_withheld_values`]).
///
/// For a service whose clause quotes the value the framework stored, a collection's count
/// summary included: the redaction of a `@sensitive` member's value is what has to be
/// undone, and nothing else about the clause.
#[must_use]
pub fn refusing_violations_restoring_withheld<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    violations_in::<O>(protocol, req, Echoes::Withheld).violations
}

/// What one collecting walk found: the violations, and the request's lists when the caller
/// asked for them ([`Echoes::SubmittedWithLists`]; otherwise empty).
#[derive(Debug, Default)]
pub(crate) struct Refusing {
    pub(crate) violations: Vec<Violation>,
    pub(crate) lists: HashMap<String, String>,
    /// How a clause can state each violation's value, in the same order as `violations`,
    /// when the caller asked for it ([`Echoes::Renderings`]; otherwise empty).
    pub(crate) echoes: Vec<SubmittedEcho>,
}

/// How a clause can state the value one violation is about.
///
/// The distinction [`Violation::value`] cannot carry: `None` there means the member was
/// ABSENT, so a value that was PRESENT but has no text rendering (a blob's bytes, a map, a
/// document) reaches a service as the framework's summary of it — `<n> bytes`,
/// `<n> entries`, `<document>` — which is wire text AWS never emits, and a service that
/// omits the value for exactly those members cannot tell them apart. Here such a value is
/// [`SubmittedEcho::Unrendered`], an ABSENT member is [`SubmittedEcho::Absent`], and
/// everything this walk can render — including a LIST as `[<element>, …]` and a STRUCTURE
/// as `<ShapeName>(<member>=<value>, …)`, the form the AWS SDKs print a modelled structure
/// in, or over every member the shape declares where the caller states that form
/// ([`refusing_violations_with_declared_echoes_over`]) — is [`SubmittedEcho::Rendered`]. Which of them a clause quotes, and how long a
/// value it is willing to quote, stays with the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmittedEcho {
    /// The member was absent from the request: there is no submitted value at all.
    Absent,
    /// The value the caller submitted, rendered the way a value renders inside a clause:
    /// a string as its own text, a number as its digits, a list as `[<element>, …]`, a
    /// structure as `<ShapeName>(<member>=<value>, …)`.
    Rendered(String),
    /// A value that WAS submitted and has no text rendering this walk produces, or one
    /// past the bounds this module states. What a clause says in place of it — omit the
    /// value, name its class — is the service's own choice.
    Unrendered,
}

/// Every violation that refuses `req` before `O`'s handler runs, each paired with how a
/// clause can state the value it is about ([`SubmittedEcho`]).
///
/// The same collecting walk as [`refusing_violations`] — [`violations_in`], over the
/// framework's own deserializer and `constraints::validate` — so the two cannot find a
/// different set of violations. Only the echo differs: this one renders a COMPOSITE value
/// (a list, a structure) rather than leaving the framework's count summary in place, and
/// reports a value it cannot render as such instead of as a summary.
#[must_use]
pub fn refusing_violations_with_echoes<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(protocol, req, Echoes::Renderings);
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// As [`refusing_violations_with_echoes`], with the member POSITIONS the walk may visit
/// stated by the SERVICE rather than taken from this module's own [`MAX_RECORDED_LEAVES`].
///
/// What it is for, and why it is the same kind of knob as
/// [`refusing_violations_quoting_up_to`]: the default count is sized for the members most
/// models declare, and a COMPOSITE renders only when every one of its children was walked
/// — so a model whose list admits `n` elements of a `m`-member structure needs about
/// `n * (m + 1)` positions before that list can be echoed at all, which is past the
/// default for a list of a couple of hundred elements. A service whose model declares such
/// a list states its own ceiling here, computed from that list's declared maximum, so the
/// ceiling is a MODEL constant and no response is sized by the request. Everything else is
/// [`refusing_violations_with_echoes`]'s: the same collecting walk, the same violations,
/// the same per-value and per-message echo bounds ([`MAX_RESTORED_ECHO`],
/// [`MAX_RESTORED_TOTAL`]), which still bound what one message carries.
#[must_use]
pub fn refusing_violations_with_echoes_over<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_positions: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(protocol, req, Echoes::RenderingsOver(max_positions));
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// As [`refusing_violations_with_echoes`], with a MAP rendered as the entries it holds
/// ([`rendered_map`]) instead of reported [`SubmittedEcho::Unrendered`] — and with every
/// LIST of scalars the request carried, keyed by the member path the walk reports that list
/// at ([`submitted_lists`]).
///
/// For a service whose recordings show a clause on a COLLECTION member quoting that
/// collection. Its two halves are the two ways such a clause arises: a `@length` on a map or
/// a list is checked on the member, so the only rendering the framework has for it is the
/// size, and a clause quoting the size states a value no caller sent (the ECHO answers
/// that); while a constraint broken by a list's ELEMENTS is reported by the framework at the
/// element and by AWS at the list, where the value is the whole list, which no violation
/// carries at all (the LISTS answer that).
///
/// The map entries are stated in KEY order, which is the one order one request's map can be
/// stated in twice: the map reaches this walk as the built input's own unordered map, so the
/// order its entries were written in is not the order the request carried them in. Which
/// order the service being reproduced prints them in is a fact about THAT service's own
/// renderer and is not recoverable here. Everything else is
/// [`refusing_violations_with_echoes`]'s: the same collecting walk, the same violations, the
/// same per-value and per-message echo bounds.
#[must_use]
pub fn refusing_violations_with_mapped_echoes<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> (Vec<(Violation, SubmittedEcho)>, HashMap<String, String>) {
    let refusing = violations_in::<O>(protocol, req, Echoes::MappedRenderings);
    let lists = refusing.lists;
    (
        refusing
            .violations
            .into_iter()
            .zip(refusing.echoes)
            .collect(),
        lists,
    )
}

/// As [`refusing_violations_with_echoes_over`], with a STRUCTURE rendered over EVERY member
/// its shape declares — one the request left unset as `null` — and with a structure reached
/// as a struct MEMBER rendered too, under the shape name its own members carry.
///
/// The two forms a value of structure type is printed in differ in exactly that: the
/// server-side validation layer in front of a service prints a modelled structure with its
/// unset members named (`Thing(name=n, size=null)`), while the client SDKs print only the
/// members that are set (`Thing(name=n)`) — so which one a clause quotes is a fact about
/// the service whose refusal is being reproduced, and is chosen here rather than guessed.
/// Everything else is [`refusing_violations_with_echoes_over`]'s: the same collecting walk,
/// the same violations, the same per-value and per-message echo bounds.
///
/// ONE LEVEL DEEP, and this is a limit of the frozen schema crate rather than a choice: a
/// structure reached as a struct MEMBER is written against the MEMBER's schema, which
/// carries the member's own name and nothing of its target shape — no shape name (recovered
/// here from the members the structure wrote, [`declaring_shape`]) and no member list. Such
/// a structure therefore renders over the members that were SET, which is the same text
/// whenever every member its shape declares was sent (the single-member destination shapes
/// are that case) and states one member too few when it was not. Rendering it over its
/// declared members needs the target shape's members on the member schema, which only
/// `aws-smithy-schema` can carry; the fail-safe directions are taken meanwhile — a
/// structure the walk cannot name at all renders as nothing, and a composite with an
/// unrenderable child renders as nothing, so a clause states the framework's own summary
/// rather than a value missing a part of itself.
#[must_use]
pub fn refusing_violations_with_declared_echoes_over<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_positions: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(protocol, req, Echoes::DeclaredRenderingsOver(max_positions));
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// As [`refusing_violations_with_declared_echoes_over`], with the largest single value one
/// clause may quote stated by the SERVICE rather than taken from [`MAX_RESTORED_ECHO`].
///
/// The echo counterpart of [`refusing_violations_quoting_up_to`], and it exists for the same
/// reason: the flat bound is sized for the members most models declare, so a model that
/// declares a `@length` maximum ABOVE it has refusals whose value could never be quoted in
/// full — the one thing an AWS constraint message always does. A service whose model declares
/// such a member states its own ceiling here, from that member's declared maximum, so the
/// ceiling is a MODEL constant and no response is sized by the request.
///
/// The ceiling is the one a LEAF value is quoted up to, and with it the bytes one message may
/// restore. A COMPOSITE's rendering keeps this module's own ceiling
/// ([`MAX_RESTORED_ECHO`]): a composite is built from many of a caller's values rather than
/// being one of them, so quoting it is not the fidelity this knob exists for. Everything else
/// is [`refusing_violations_with_declared_echoes_over`]'s: the same collecting walk, the same
/// violations, the same position count.
#[must_use]
pub fn refusing_violations_with_declared_echoes_quoting_up_to<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_positions: usize,
    max_echo: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(
        protocol,
        req,
        Echoes::DeclaredRenderingsQuotingUpTo(max_positions, max_echo),
    );
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// As [`refusing_violations_with_declared_echoes_over`], with the ceiling ONE COMPOSITE's
/// rendering is kept up to stated by the SERVICE rather than taken from
/// [`MAX_RESTORED_ECHO`].
///
/// The sibling of [`refusing_violations_with_declared_echoes_quoting_up_to`], for the other
/// half of the same fidelity: that one raises the ceiling a single LEAF value is quoted up
/// to, this one the ceiling the rendering of a COLLECTION (or any other composite) is kept
/// up to. A service needs it when its recordings show a clause quoting the whole collection
/// a request carried and that collection's own declared maximum puts its rendering above the
/// flat bound — a batch member, whose documented request maximum is the ceiling to state it
/// from, so the ceiling is a MODEL constant and no response is sized by the request. Past it
/// the framework's own count summary stands, which is the fail-safe direction.
///
/// `max_render` is the ceiling for a composite's rendering AND for one value a clause
/// quotes, because a rendering no clause could carry is not worth holding. Everything else
/// is [`refusing_violations_with_declared_echoes_over`]'s: the same collecting walk, the
/// same violations, the same position count, the same structure form.
#[must_use]
pub fn refusing_violations_with_declared_echoes_rendering_up_to<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_positions: usize,
    max_render: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(
        protocol,
        req,
        Echoes::DeclaredRenderingsRenderingUpTo(max_positions, max_render),
    );
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// As [`refusing_violations_with_declared_echoes_over`], with the members a structure is
/// printed over stated by the CALLER for the shapes `printed` names, instead of read off
/// the model.
///
/// What it is for: the object the validation layer in front of a service prints is that
/// layer's own, and for some services it carries members the public model does not declare
/// — a member of an internal request type, which no caller can set and which therefore
/// always prints unset. The member list and its ORDER are fixed text of that message rather
/// than anything derived from one request, so they are stated by the service whose refusal
/// is being reproduced ([`StructureForm::MembersPrinted`]): one entry per shape, naming the
/// members in the order the layer prints them, each spelled as a clause spells a member.
///
/// A shape `printed` does not name renders exactly as
/// [`refusing_violations_with_declared_echoes_over`] renders it, so a service states only
/// the shapes whose printed form it knows. Everything else is that function's: the same
/// collecting walk, the same violations, the same per-value and per-message echo bounds.
#[must_use]
pub fn refusing_violations_with_printed_echoes_over<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    max_positions: usize,
    printed: PrintedMembers,
) -> Vec<(Violation, SubmittedEcho)> {
    let refusing = violations_in::<O>(
        protocol,
        req,
        Echoes::PrintedRenderingsOver(max_positions, printed),
    );
    refusing
        .violations
        .into_iter()
        .zip(refusing.echoes)
        .collect()
}

/// The members one or more SHAPES are printed with inside a clause, in the order they are
/// printed: one entry per shape, naming the shape and its printed member list.
///
/// Stated by the service whose refusal is being reproduced — see
/// [`refusing_violations_with_printed_echoes_over`] — and `'static` because it is a
/// property of that service's recorded wording rather than of any request.
pub type PrintedMembers = &'static [(&'static str, &'static [&'static str])];

/// Whether a collected violation's value echo is restored to the bytes the caller sent.
///
/// A service whose refusal QUOTES the value needs it restored; one whose refusal reports
/// only the class of failure (see [`crate::front_door`]) does not, and the walk that
/// restores it is work that would never be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Echoes {
    /// Restore each echo through [`with_submitted_values`].
    Submitted,
    /// Restore each echo, quoting one value up to the bound the SERVICE states
    /// ([`with_submitted_values_up_to`]) instead of [`MAX_RESTORED_ECHO`].
    SubmittedUpTo(usize),
    /// Leave the framework's own rendering in place.
    Rendered,
    /// Leave the framework's own rendering in place, EXCEPT where the validating walk
    /// withheld the value as it recorded the violation: that one is put back as that walk
    /// itself would have stored it ([`with_withheld_values`]). For a service whose clause
    /// quotes the value the framework stored — a collection's count summary included — and
    /// so needs the redaction undone and nothing else.
    Withheld,
    /// Restore each echo, and collect the request's lists too ([`submitted_lists`]).
    SubmittedWithLists,
    /// As [`Self::SubmittedWithLists`], quoting one value up to the bound the SERVICE
    /// states ([`with_submitted_values_up_to`]) instead of [`MAX_RESTORED_ECHO`].
    SubmittedWithListsUpTo(usize),
    /// Pair each violation with how a clause can state its value
    /// ([`with_submitted_echoes`]), which renders a composite value instead of leaving the
    /// framework's count summary in place.
    Renderings,
    /// As [`Self::Renderings`], over as many member positions as the SERVICE states
    /// ([`with_submitted_echoes_over`]) instead of [`MAX_RECORDED_LEAVES`].
    RenderingsOver(usize),
    /// As [`Self::Renderings`], with a MAP rendered as the entries it holds
    /// ([`refusing_violations_with_mapped_echoes`]).
    MappedRenderings,
    /// As [`Self::RenderingsOver`], with a structure rendered over every member its shape
    /// declares ([`refusing_violations_with_declared_echoes_over`]).
    DeclaredRenderingsOver(usize),
    /// As [`Self::DeclaredRenderingsOver`], quoting one LEAF value up to the ceiling the
    /// SERVICE states ([`refusing_violations_with_declared_echoes_quoting_up_to`]) instead of
    /// [`MAX_RESTORED_ECHO`].
    DeclaredRenderingsQuotingUpTo(usize, usize),
    /// As [`Self::DeclaredRenderingsOver`], with the members a structure is printed over
    /// stated by the caller for the shapes it names
    /// ([`refusing_violations_with_printed_echoes_over`]).
    PrintedRenderingsOver(usize, PrintedMembers),
    /// As [`Self::DeclaredRenderingsOver`], with the ceiling ONE COMPOSITE's rendering is
    /// kept up to stated by the SERVICE
    /// ([`refusing_violations_with_declared_echoes_rendering_up_to`]) instead of
    /// [`MAX_RESTORED_ECHO`].
    DeclaredRenderingsRenderingUpTo(usize, usize),
}

/// Which members a STRUCTURE is rendered over inside a clause.
///
/// Both forms are printed by AWS, by different layers (see
/// [`refusing_violations_with_declared_echoes_over`]), so the caller states the one its
/// recordings show rather than this walk picking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StructureForm {
    /// The members the request SET, in the order the value wrote them, and only for a
    /// structure whose own shape schema is at hand to name it.
    #[default]
    MembersSet,
    /// Every member the shape DECLARES, in declaration order, with one the request left
    /// unset rendered as `null`.
    EveryMemberDeclared,
    /// Every member the shapes this names are PRINTED with, in the order it states them,
    /// with one the request left unset rendered as `null` — the form for a layer whose
    /// printed object carries members the model does not declare
    /// ([`refusing_violations_with_printed_echoes_over`]). A shape it does not name renders
    /// as [`Self::EveryMemberDeclared`] renders it.
    MembersPrinted(PrintedMembers),
}

/// Whether a MAP is RENDERED inside a clause, or reported as a value this walk states
/// nothing for.
///
/// The two are different claims about the service being reproduced — a clause quoting the
/// map it refused, and one naming the member alone — so the caller states the form its
/// recordings show rather than this walk picking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MapForm {
    /// A map has no rendering: a clause states [`SubmittedEcho::Unrendered`] for one.
    #[default]
    Unrendered,
    /// A map renders as the entries it holds, in key order ([`rendered_map`]).
    Entries,
}

/// The ONE collecting walk this crate performs: every violation that refuses `req` before
/// `O`'s handler runs, with `echoes` deciding whether each value echo is restored.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so no caller of this can find a different
/// set of violations from the walk that follows it. What is then DONE with them — which
/// members they are narrowed to, which envelope, which error code, which prose — belongs to
/// the caller.
pub(crate) fn violations_in<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    echoes: Echoes,
) -> Refusing {
    let mut deserializer = match protocol.deserialize_input(req, O::INPUT_SCHEMA) {
        Ok(deserializer) => deserializer,
        // A refusal the PROTOCOL itself made while reading the request states the same
        // thing the walk below states — one or more values the request carried are not
        // ones the shape admits, carried structurally as violations — so it is the same
        // refusal to a caller and the same one here. A service whose codec refuses a
        // document before any member is built (a character the document's own grammar
        // cannot hold, a union carrying other than the one member it chooses between) would
        // otherwise find NO violation here and leave the request to the layers behind the
        // door, where the framework's own renderer answers it in an envelope the service
        // never chose.
        //
        // No value is echoed for such a violation: there is no built input to render one
        // from, so each is reported [`SubmittedEcho::Unrendered`] — "submitted, with no
        // text rendering this walk produces" — which leaves every clause exactly the words
        // the violation itself carries.
        Err(::aws_smithy_schema_server::ProtocolError::Invalid(violations)) => {
            let echoes = vec![SubmittedEcho::Unrendered; violations.len()];
            return Refusing {
                violations,
                lists: HashMap::new(),
                echoes,
            };
        }
        Err(_) => return Refusing::default(),
    };
    let read = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => {
            let violations = constraints::validate(&input, O::INPUT_SCHEMA);
            match echoes {
                Echoes::Submitted => Refusing {
                    violations: with_submitted_values(violations, &input, O::INPUT_SCHEMA),
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::SubmittedUpTo(max_echo) => Refusing {
                    violations: with_submitted_values_up_to(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_echo,
                    ),
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::Rendered => Refusing {
                    violations,
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::Withheld => Refusing {
                    violations: with_withheld_values(violations, &input, O::INPUT_SCHEMA),
                    lists: HashMap::new(),
                    echoes: Vec::new(),
                },
                Echoes::Renderings => {
                    let (violations, echoes) =
                        with_submitted_echoes(violations, &input, O::INPUT_SCHEMA)
                            .into_iter()
                            .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::RenderingsOver(max_positions) => {
                    let (violations, echoes) = with_submitted_echoes_over(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_positions,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::MappedRenderings => {
                    // The lists are only ever READ for a violation found on a list ELEMENT,
                    // so a request that broke no such constraint pays nothing for them.
                    let wanted = violations
                        .iter()
                        .any(|violation| list_of_element(&violation.path).is_some());
                    let lists = if wanted {
                        submitted_lists(&input, O::INPUT_SCHEMA)
                    } else {
                        HashMap::new()
                    };
                    let (violations, echoes) = echoed_over(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        MAX_RECORDED_LEAVES,
                        StructureForm::MembersSet,
                        MapForm::Entries,
                        MAX_RESTORED_ECHO,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists,
                        echoes,
                    }
                }
                Echoes::DeclaredRenderingsOver(max_positions) => {
                    let (violations, echoes) = echoed_over(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_positions,
                        StructureForm::EveryMemberDeclared,
                        MapForm::Unrendered,
                        MAX_RESTORED_ECHO,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::DeclaredRenderingsQuotingUpTo(max_positions, max_echo) => {
                    let (violations, echoes) = echoed_over(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_positions,
                        StructureForm::EveryMemberDeclared,
                        MapForm::Unrendered,
                        max_echo,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::DeclaredRenderingsRenderingUpTo(max_positions, max_render) => {
                    let (violations, echoes) = echoed_over_rendering_up_to(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_positions,
                        StructureForm::EveryMemberDeclared,
                        MapForm::Unrendered,
                        max_render,
                        max_render,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::PrintedRenderingsOver(max_positions, printed) => {
                    let (violations, echoes) = echoed_over(
                        violations,
                        &input,
                        O::INPUT_SCHEMA,
                        max_positions,
                        StructureForm::MembersPrinted(printed),
                        MapForm::Unrendered,
                        MAX_RESTORED_ECHO,
                    )
                    .into_iter()
                    .unzip();
                    Refusing {
                        violations,
                        lists: HashMap::new(),
                        echoes,
                    }
                }
                Echoes::SubmittedWithLists => {
                    // The lists are only ever READ for a violation found on a list
                    // ELEMENT, so a request that broke no such constraint — which is every
                    // request that satisfies the model — pays nothing for them.
                    let wanted = violations
                        .iter()
                        .any(|violation| list_of_element(&violation.path).is_some());
                    Refusing {
                        violations: with_submitted_values(violations, &input, O::INPUT_SCHEMA),
                        lists: if wanted {
                            submitted_lists(&input, O::INPUT_SCHEMA)
                        } else {
                            HashMap::new()
                        },
                        echoes: Vec::new(),
                    }
                }
                Echoes::SubmittedWithListsUpTo(max_echo) => {
                    // The same one read as `SubmittedWithLists`, under the service's own
                    // quoting ceiling: the lists are collected on the same condition and
                    // from the same built input.
                    let wanted = violations
                        .iter()
                        .any(|violation| list_of_element(&violation.path).is_some());
                    Refusing {
                        violations: with_submitted_values_up_to(
                            violations,
                            &input,
                            O::INPUT_SCHEMA,
                            max_echo,
                        ),
                        lists: if wanted {
                            submitted_lists(&input, O::INPUT_SCHEMA)
                        } else {
                            HashMap::new()
                        },
                        echoes: Vec::new(),
                    }
                }
            }
        }
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here. `member_name`
        // is already a violation path in the spelling the constraint walk uses
        // (`aws_smithy_schema_server::missing_member` applies `path::member`, and each
        // enclosing member prepends its own segment), so it places identically.
        Err(SerdeError::MissingMember { member_name }) => Refusing {
            violations: vec![Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                // An absent member has no value to withhold, so the clause names the
                // path alone whether or not the model marks the member `@sensitive`.
                sensitive: false,
            }],
            lists: HashMap::new(),
            // An absent member has no submitted value, whichever echo the caller asked
            // for, so this stays in step with `violations` for every mode.
            echoes: vec![SubmittedEcho::Absent],
        },
        Err(_) => Refusing::default(),
    }
}

/// `value` as a CLAUSE of a refusal quotes it: the value itself when it fits the ceiling
/// one clause may carry ([`MAX_RESTORED_ECHO`]), and past that a character-boundary-safe
/// prefix of it followed by the marker naming the whole value's byte count — the same shape
/// the framework's own constraint walk stores for a value too long to interpolate, and the
/// shape [`is_elision_of`] recognises.
///
/// For a service that words a clause over a value the framework reported NO violation on: a
/// constraint the model states only in PROSE, which the handler therefore checks itself.
/// Asking here is what gives that clause the SAME ceiling as every clause restored from the
/// walk ([`with_submitted_values`]), so the two refusal paths of one service cannot carry
/// different ceilings and no message is sized by the request.
///
/// The CHECK must still be made against the value the caller sent, never against this
/// rendering: eliding before the check compares text nobody submitted.
///
/// A service whose model states a ceiling of its own asks [`quoted_echo_up_to`] instead.
#[must_use]
pub fn quoted_echo(value: &str) -> ::std::borrow::Cow<'_, str> {
    quoted_echo_up_to(value, MAX_RESTORED_ECHO)
}

/// [`quoted_echo`], with the largest value one clause may quote stated by the SERVICE
/// rather than taken from [`MAX_RESTORED_ECHO`].
///
/// `max_echo` is the only difference: the rendering, the character-boundary-safe prefix and
/// the marker naming the whole value's byte count are the same, so a clause a HANDLER words
/// is quoted exactly as one restored from the framework's walk. A service that states its
/// own ceiling for the walk ([`refusing_violations_quoting_up_to`]) states the same one
/// here, so its two refusal paths cannot carry different ceilings — which is what a model
/// that declares a bound of its own on the member a refusal is CARRIED in needs, in either
/// direction: a member declared longer than [`MAX_RESTORED_ECHO`] so a value can be quoted
/// in full, and one declared shorter so a clause fits the member that has to hold it.
#[must_use]
pub fn quoted_echo_up_to(value: &str, max_echo: usize) -> ::std::borrow::Cow<'_, str> {
    if value.len() <= max_echo {
        return ::std::borrow::Cow::Borrowed(value);
    }
    ::std::borrow::Cow::Owned(format!(
        "{}... [{} bytes]",
        char_prefix(value, max_echo),
        value.len()
    ))
}

/// One clause's ECHO, elided at `max_echo` bytes with `marker` standing for the rest —
/// `<prefix><marker>` — for a service whose recorded clauses mark an elision WITHOUT naming
/// the submitted byte count.
///
/// [`echo_held_to`] is the rendering to reach for first, and the one most services are
/// recorded stating: it keeps the `... [<n> bytes]` marker the framework's own walk writes,
/// so a caller still reads how much it sent. A service whose recordings show a different
/// marker states it HERE rather than open-coding the prefix walk, so there is one
/// character-boundary-safe elision in the tree ([`char_prefix`]) however a service marks it.
///
/// The prefix is `max_echo` bytes of the ECHO as handed in, on a character boundary. An echo
/// the framework already elided therefore keeps that elision's own marker inside the prefix
/// when it falls there, which is the honest reading: the bytes past `max_echo` are gone
/// either way, and only the marker a service states is this function's business.
#[must_use]
pub fn echo_elided_with<'e>(
    echo: &'e str,
    max_echo: usize,
    marker: &str,
) -> ::std::borrow::Cow<'e, str> {
    if echo.len() <= max_echo {
        return ::std::borrow::Cow::Borrowed(echo);
    }
    ::std::borrow::Cow::Owned(format!("{}{marker}", char_prefix(echo, max_echo)))
}

/// One clause's ECHO — the value itself, or an elision of it ([`quoted_echo_up_to`], or the
/// framework's own rendering, which has the same shape) — stated in at most `max_echo`
/// bytes.
///
/// For a service whose MESSAGE carries a bound of its own: where the member a refusal is
/// carried in declares an `@length` maximum, what all the clauses of one sentence may quote
/// TOGETHER is bounded, so a per-value ceiling is not enough and each echo has to be held to
/// its share of that budget — which is what this does, after the fact, to an echo a walk
/// already rendered.
///
/// An echo that already NAMES a byte count is a prefix of a value the caller of this
/// function does not hold: shortening it keeps that count, because the count is the
/// submitted value's size rather than the rendering's, and only the prefix gets shorter. So
/// a clause held twice still reports how much was sent, and what it quotes is still an
/// elision of the same value ([`is_elision_of`]). An echo that names none is the value
/// itself, and is elided as [`quoted_echo_up_to`] elides one.
///
/// A `max_echo` smaller than the marker leaves the marker: how much was sent is the one
/// thing a caller can read nowhere else.
#[must_use]
pub fn echo_held_to(echo: &str, max_echo: usize) -> ::std::borrow::Cow<'_, str> {
    if echo.len() <= max_echo {
        return ::std::borrow::Cow::Borrowed(echo);
    }
    let (value_prefix, marker) = match named_byte_count(echo) {
        Some(at) => (&echo[..at], echo[at..].to_string()),
        None => (echo, format!("... [{} bytes]", echo.len())),
    };
    let room = max_echo.saturating_sub(marker.len());
    ::std::borrow::Cow::Owned(format!("{}{marker}", char_prefix(value_prefix, room)))
}

/// Where the marker naming a submitted byte count starts in `echo`, when `echo` is an
/// elision of a value rather than the value itself.
///
/// The marker is the one [`quoted_echo_up_to`] and the framework's own walk append —
/// `... [<n> bytes]` at the very end — and it is read as one only when the count it names is
/// larger than the text before it, which is what makes it an elision of something longer
/// rather than a value that happens to end in those characters.
fn named_byte_count(echo: &str) -> Option<usize> {
    let digits = echo.strip_suffix(" bytes]")?;
    let at = digits.rfind("... [")?;
    let named: usize = digits[at + "... [".len()..].parse().ok()?;
    (named > at).then_some(at)
}

/// The longest prefix of `value` no longer than `max` bytes that ends on a character
/// boundary, so a rendering never splits a UTF-8 character.
fn char_prefix(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// The framework's own ceiling on the value one clause echoes
/// (`MAX_ECHOED_VALUE` in `aws_smithy_schema_server::constraints`, which is private): a
/// longer value is stored on the violation ALREADY elided to `<prefix>... [<n> bytes]`.
///
/// Read only by [`crate::validation_message::quoting_the_whole_submitted_value`], to put
/// back exactly what the framework's own walk would have stored for a value it WITHHELD —
/// so undoing a redaction never also undoes that elision.
pub(crate) const FRAMEWORK_ECHO_CEILING: usize = 256;

/// The largest single submitted value restored into a clause by default, and the ceiling a
/// service that states none is held to (see the module docs).
const MAX_RESTORED_ECHO: usize = 16 * 1024;

/// The restored bytes all clauses of one message may carry together.
const MAX_RESTORED_TOTAL: usize = 64 * 1024;

/// The most leaves this walk collects from one request.
const MAX_RECORDED_LEAVES: usize = 256;

/// The owned bytes — joined paths plus values, both caller-sized — this walk may collect
/// from one request. Equal to [`MAX_RESTORED_TOTAL`]: a leaf that could never be restored
/// within that budget is not worth copying.
const MAX_RECORDED_BYTES: usize = MAX_RESTORED_TOTAL;

/// The most member positions [`submitted_renderings`] records from one request. The same
/// count bound the leaf collection states, for the same reason: the collection is never
/// sized by the request.
const MAX_RECORDED_RENDERINGS: usize = MAX_RECORDED_LEAVES;

/// The owned bytes — paths plus renderings — [`submitted_renderings`] may hold.
const MAX_RECORDED_RENDERED_BYTES: usize = MAX_RECORDED_BYTES;

/// The most composites deep this walk descends into one request.
///
/// The leaf budget counts LEAF positions, so a value built only of composites — a model
/// may declare a recursive shape, so the nesting is the caller's to choose — would reach
/// the recursion below without ever spending it. This is the depth half of the same
/// bound: past it the walk stops descending, and the composite it stopped inside renders
/// as nothing exactly as a spent leaf budget makes it. Deeper than any input shape a
/// model nests, and shallow enough that the walk's own frames cannot run the stack out.
const MAX_RECORDED_DEPTH: usize = 64;

/// The most children ONE composite renders from. A composite with more keeps no
/// rendering at all, rather than one stating fewer elements than were sent.
///
/// The other half of a composite's bound is its SIZE: its children may hold at most
/// [`MAX_RESTORED_ECHO`] rendering bytes together, and a child rendering past that bound
/// is never handed to a composite at all, so the text a nested value retains cannot be
/// sized by the request at any depth.
const MAX_RENDERED_CHILDREN: usize = 256;

/// `violations`, with every elided value echo replaced by the value as submitted, within
/// the bounds this module states.
///
/// `input` and `schema` must be the ones the violations were collected from — the built
/// input and its operation's input schema — so a violation's path names a leaf of this
/// walk, or the collection one step above one. A violation this walk finds neither for (an
/// absent `@required` member, a value the framework rendered as a summary it cannot rebuild
/// — a blob, a map, a document) is returned untouched, and so is one whose value is larger
/// than the stated bounds allow.
#[must_use]
pub fn with_submitted_values(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<Violation> {
    with_submitted_values_up_to(violations, input, schema, MAX_RESTORED_ECHO)
}

/// [`with_submitted_values`], with the largest single value one clause may quote stated by
/// the caller rather than taken from [`MAX_RESTORED_ECHO`].
///
/// `max_echo` is the only difference: the walk, the matching of a stored echo against the
/// leaf it was elided from, and the per-message total ([`MAX_RESTORED_TOTAL`]) are the
/// same. See [`refusing_violations_quoting_up_to`] for when a service states its own.
#[must_use]
pub fn with_submitted_values_up_to(
    mut violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_echo: usize,
) -> Vec<Violation> {
    // Nothing to re-echo: a walk over the whole input is not worth doing for a refusal
    // that states no value at all (an absent `@required` member is the common case). A
    // violation whose value the validating walk WITHHELD is not one of those: the member
    // was there, so this walk still holds what it sent.
    if violations
        .iter()
        .all(|violation| violation.value.is_none() && !violation.sensitive)
    {
        return violations;
    }
    let leaves = submitted_leaves_up_to(input, schema, max_echo);
    // ONE pass over the leaves builds every lookup the loop below needs, so the cost is
    // O(leaves) rather than O(violations x leaves).
    let index = Submitted::of(&leaves);
    let mut budget = MAX_RESTORED_TOTAL;
    for violation in &mut violations {
        // A value the walk WITHHELD is put back as that walk itself would have stored it —
        // the submitted value under the framework's own echo ceiling — so only the
        // redaction is undone, never the elision, and a service that withholds such a
        // value does so in its own clause builder rather than by its absence here.
        if violation.sensitive {
            let withheld = index
                .candidate(violation)
                .map(|submitted| quoted_echo_up_to(submitted, FRAMEWORK_ECHO_CEILING).into_owned());
            if let Some(withheld) = withheld.filter(|value| value.len() <= budget) {
                budget -= withheld.len();
                violation.value = Some(withheld);
                violation.sensitive = false;
            }
            continue;
        }
        let Some(stored) = violation.value.as_deref() else {
            // Nothing to read back: the member held no value, or the constraint walk
            // WITHHELD it because the member is `@sensitive`, and a value it kept out of
            // the violation is not this layer's to put back.
            continue;
        };
        // Only what the framework elided FROM this leaf, or — for a clause on a
        // collection, which has no leaf of its own — the collection the summary it stored
        // counts; anything else keeps the framework's own text. A stored echo that
        // already IS the submitted value yields nothing to restore, so it spends no
        // budget.
        let restored = match index.candidate(violation) {
            Some(submitted) if is_elision_of(stored, submitted) => {
                (stored != submitted).then(|| submitted.to_string())
            }
            _ => index.collection_candidate(violation, stored),
        };
        let Some(restored) = restored else {
            continue;
        };
        // And only within the stated bounds.
        if restored.len() > max_echo || restored.len() > budget {
            continue;
        }
        budget -= restored.len();
        violation.value = Some(restored);
    }
    violations
}

/// `violations` with the value of every one the validating walk WITHHELD — a `@sensitive`
/// member's, dropped as the violation was recorded — put back as that walk itself would
/// have stored it: the submitted value under the framework's own echo ceiling
/// ([`FRAMEWORK_ECHO_CEILING`]). Every other violation is untouched, the framework's own
/// rendering (a collection's count summary included) and all.
///
/// For a service whose published clause QUOTES the value the framework stored: the
/// redaction is what has to be undone, and nothing else about the clause. A service whose
/// clause withholds such a value reads [`Violation::sensitive`] (or its own reading of the
/// model) in its clause builder instead of relying on the value's absence.
///
/// `input` and `schema` must be the ones the violations were collected from, exactly as for
/// [`with_submitted_values`].
#[must_use]
pub fn with_withheld_values(
    mut violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<Violation> {
    // Nothing was withheld: a walk over the whole input would be read by nobody.
    if violations.iter().all(|violation| !violation.sensitive) {
        return violations;
    }
    let leaves = submitted_leaves_up_to(input, schema, FRAMEWORK_ECHO_CEILING);
    let index = Submitted::of(&leaves);
    let mut budget = MAX_RESTORED_TOTAL;
    for violation in &mut violations {
        if !violation.sensitive {
            continue;
        }
        let withheld = index
            .candidate(violation)
            .map(|submitted| quoted_echo_up_to(submitted, FRAMEWORK_ECHO_CEILING).into_owned());
        if let Some(withheld) = withheld.filter(|value| value.len() <= budget) {
            budget -= withheld.len();
            violation.value = Some(withheld);
            violation.sensitive = false;
        }
    }
    violations
}

/// `violations`, each paired with how a clause can state the value it is about.
///
/// `input` and `schema` must be the ones the violations were collected from, exactly as
/// for [`with_submitted_values`]. A violation whose path this walk recorded no rendering
/// for — a value with no text rendering, a path past the stated bounds — is
/// [`SubmittedEcho::Unrendered`]; nothing is guessed from the framework's summary of it.
#[must_use]
pub fn with_submitted_echoes(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(Violation, SubmittedEcho)> {
    with_submitted_echoes_over(violations, input, schema, MAX_RECORDED_LEAVES)
}

/// [`with_submitted_echoes`], over as many member positions as the CALLER states rather
/// than [`MAX_RECORDED_LEAVES`].
///
/// `max_positions` is the only difference: the walk, the path spelling and the echo bounds
/// are the same. See [`refusing_violations_with_echoes_over`] for when a service states its
/// own.
#[must_use]
pub fn with_submitted_echoes_over(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_positions: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    echoed_over(
        violations,
        input,
        schema,
        max_positions,
        StructureForm::MembersSet,
        MapForm::Unrendered,
        MAX_RESTORED_ECHO,
    )
}

/// [`with_submitted_echoes_over`], with the STRUCTURE and MAP forms the caller states, and
/// the ceiling one LEAF value is quoted up to. A composite's rendering keeps this module's
/// own ceiling; [`echoed_over_rendering_up_to`] is the form that states that one too.
///
/// The one body every echo mode runs: the walk, the path spelling and the echo bounds are
/// the same, so the forms cannot drift into several walks.
#[allow(clippy::too_many_arguments)]
fn echoed_over(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_positions: usize,
    structures: StructureForm,
    maps: MapForm,
    max_echo: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    echoed_over_rendering_up_to(
        violations,
        input,
        schema,
        max_positions,
        structures,
        maps,
        max_echo,
        MAX_RESTORED_ECHO,
    )
}

/// [`echoed_over`], with the ceiling one COMPOSITE's rendering is kept up to stated too
/// ([`refusing_violations_with_declared_echoes_rendering_up_to`]).
///
/// The ONE body every echo mode runs — [`echoed_over`] is this with the module's own
/// composite ceiling — so a service that raises that ceiling runs exactly the same walk,
/// the same path spelling and the same per-message budget as one that does not.
#[allow(clippy::too_many_arguments)]
fn echoed_over_rendering_up_to(
    violations: Vec<Violation>,
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_positions: usize,
    structures: StructureForm,
    maps: MapForm,
    max_echo: usize,
    max_render: usize,
) -> Vec<(Violation, SubmittedEcho)> {
    // A refusal that states no value at all (an absent `@required` member is the common
    // case) is not worth a walk over the whole input. A violation whose value the
    // validating walk WITHHELD is not one of those: the member was there, so this walk
    // still has a rendering to collect for a service whose refusal quotes it.
    if violations
        .iter()
        .all(|violation| violation.value.is_none() && !violation.sensitive)
    {
        return violations
            .into_iter()
            .map(|violation| (violation, SubmittedEcho::Absent))
            .collect();
    }
    let renderings = renderings_in(
        input,
        schema,
        max_positions,
        structures,
        maps,
        max_echo,
        max_render,
    );
    // ONE pass over the renderings builds the lookup, so the cost is O(positions) rather
    // than O(violations x positions).
    let mut by_path: HashMap<&str, Option<&str>> = HashMap::new();
    for (position, rendering) in &renderings {
        by_path
            .entry(position.as_str())
            .or_insert(rendering.as_deref());
    }
    // One clause may quote up to the stated ceiling, so the bytes all clauses of one
    // message restore is at least that much.
    let mut budget = MAX_RESTORED_TOTAL.max(max_echo);
    violations
        .into_iter()
        .map(|violation| {
            let echo = echo_of(&violation, &by_path, &mut budget, max_echo);
            (violation, echo)
        })
        .collect()
}

/// How a clause can state `violation`'s value, within the bounds this module states and the
/// ceiling the caller stated for one value.
///
/// A violation whose value the validating walk WITHHELD — the value of a `@sensitive`
/// member, which the walk drops as it records the violation and marks
/// [`Violation::sensitive`] for — is not an absent member: this walk re-reads the request
/// itself, so it can still state what the caller sent, for a service whose published
/// refusal quotes that value. A service that withholds it instead reads the flag in its own
/// clause builder.
fn echo_of(
    violation: &Violation,
    by_path: &HashMap<&str, Option<&str>>,
    budget: &mut usize,
    max_echo: usize,
) -> SubmittedEcho {
    if violation.value.is_none() && !violation.sensitive {
        return SubmittedEcho::Absent;
    }
    let rendered = match violation.kind {
        // A `@uniqueItems` clause is reported at the LIST's path but states the ELEMENT
        // that repeated, where the rendering recorded is the whole list. The framework's
        // own echo IS that element, already bounded, so it stands — unless it was
        // withheld, where the list's rendering at this path is all there is.
        ViolationKind::UniqueItems if !violation.sensitive => violation.value.as_deref(),
        _ => match by_path.get(violation.path.as_str()) {
            Some(Some(rendering)) => Some(*rendering),
            // A value with no text rendering, or a position this walk recorded nothing
            // for (its bounds, or a path the frozen crate spells differently).
            _ => None,
        },
    };
    match rendered {
        Some(rendering) if rendering.len() <= max_echo && rendering.len() <= *budget => {
            *budget -= rendering.len();
            SubmittedEcho::Rendered(rendering.to_string())
        }
        _ => SubmittedEcho::Unrendered,
    }
}

/// The collected leaves of one request, indexed for the lookups a violation needs.
///
/// All three maps borrow the collected leaves and are built in a single pass over them.
#[derive(Debug)]
struct Submitted<'l> {
    /// The first leaf recorded at each dotted path: the member a clause names.
    by_path: HashMap<&'l str, &'l str>,
    /// For each list whose scalar elements were recorded, the first element value that
    /// repeated — what a `@uniqueItems` clause echoes, reported at the LIST's path.
    duplicate_in: HashMap<&'l str, &'l str>,
    /// For each list whose scalar elements were recorded, those elements in walk order —
    /// what a clause on the LIST itself echoes, reported at the list's own path.
    elements_in: HashMap<&'l str, Vec<&'l str>>,
}

impl<'l> Submitted<'l> {
    fn of(leaves: &'l [(String, String)]) -> Self {
        // `leaves` is already bounded by `MAX_RECORDED_LEAVES` / `MAX_RECORDED_BYTES`, so
        // every map is too.
        let mut by_path: HashMap<&str, &str> = HashMap::new();
        let mut seen: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut duplicate_in: HashMap<&str, &str> = HashMap::new();
        let mut elements_in: HashMap<&str, Vec<&str>> = HashMap::new();
        for (leaf_path, value) in leaves {
            by_path.entry(leaf_path.as_str()).or_insert(value.as_str());
            if let Some(list) = list_of_element(leaf_path) {
                if !seen.entry(list).or_default().insert(value.as_str()) {
                    duplicate_in.entry(list).or_insert(value.as_str());
                }
                elements_in.entry(list).or_default().push(value.as_str());
            }
        }
        Self {
            by_path,
            duplicate_in,
            elements_in,
        }
    }

    /// The leaf `violation`'s stored echo could have been elided from.
    fn candidate(&self, violation: &Violation) -> Option<&'l str> {
        if matches!(violation.kind, ViolationKind::UniqueItems) {
            // A duplicate is reported at the LIST's path, and the value it echoes is one
            // of that list's elements — recorded one step deeper, never at the list.
            return self.duplicate_in.get(violation.path.as_str()).copied();
        }
        self.by_path.get(violation.path.as_str()).copied()
    }

    /// The COLLECTION `violation`'s stored echo summarises, rendered as the value the
    /// caller submitted, when the two describe the same collection.
    ///
    /// A `@length` clause on a list is reported at the LIST's path, where the walk records
    /// no leaf of its own — the elements are recorded one step deeper — and the echo it
    /// stored is the framework's count rendering rather than the value. The count is what
    /// ties the two together: a summary of `<n> items` is rebuilt only from exactly `n`
    /// recorded elements, so a partially collected list (the leaf caps) and a list whose
    /// elements have no string rendering yield nothing and keep the framework's summary.
    fn collection_candidate(&self, violation: &Violation, stored: &str) -> Option<String> {
        let count = collection_count(stored)?;
        let elements = self.elements_in.get(violation.path.as_str())?;
        (elements.len() == count).then(|| rendered_list(elements))
    }
}

/// How many elements the framework's rendering of a collection states, when `stored` is
/// one: the walk renders a list as `<n> items`
/// ([`aws_smithy_schema_server::constraints`]'s collection-length check).
fn collection_count(stored: &str) -> Option<usize> {
    stored.strip_suffix(" items")?.parse::<usize>().ok()
}

/// `elements` as a list renders: the elements in order, comma-separated, in brackets —
/// the form AWS echoes a list-valued member back in.
///
/// `pub` for the same reason [`rendered_map`] is: where the cap a list is held to is the
/// SERVICE's rather than the model's, no walk ahead of the handler reports it, so the
/// HANDLER that applies that cap words its own clause and has to quote the list the same
/// way a clause restored from the walk does ([`submitted_lists`]).
#[must_use]
pub fn rendered_list(elements: &[&str]) -> String {
    format!("[{}]", elements.join(", "))
}

/// `entries` as a map renders: `{<key>=<value>, …}` over the entries in the order given —
/// the form AWS echoes a map-valued member back in.
///
/// `pub` because the one constraint class no walk ahead of a handler reports is a map's KEY
/// (the walk treats it as a path segment), so the HANDLER that applies the key shape's
/// constraints words its own clause and has to quote the map the same way a clause restored
/// from the walk does ([`refusing_violations_with_mapped_echoes`]).
#[must_use]
pub fn rendered_map(entries: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{{{}}}", rendered.join(", "))
}

/// `regex` without the whole-value anchors a model adds to express the match AWS's own
/// pattern makes implicitly: the requirement a refusal states is the expression a value has
/// to satisfy, and the `^` / `$` are how Smithy turns a find into a whole-string match
/// rather than part of the character grammar — so a client that lifts the expression out of
/// the message and re-validates locally gets the one the service advertises.
///
/// THE STAGED HOME for this rendering, which the module docs above list as one of the parts
/// of a validation message that are AWS's rather than any one service's. ~26 service crates
/// carry their own inlined copy (`grep -rln 'fn unanchored' services/*/src`), each pinned by
/// that service's own recordings; those copies are to be DELETED in favour of this one by the
/// change that owns that migration, and nothing new should inline the rule again.
///
/// The pair is dropped POSITIONALLY: the leading `^` and the trailing `$`, and only when
/// BOTH ends carry one. A `^` alone, or a `$` alone, is an expression anchored at one end —
/// a different expression, not an anchored spelling of this one — so it is emitted
/// verbatim, as is an expression that carries no `^` at its start or no `$` at its end
/// because its own branches carry them (`(^a$)|(^b$)`). A `$` that is ESCAPED is a literal
/// dollar, not an anchor, so it stays and nothing is dropped.
///
/// WHY POSITIONALLY, and not only when the pair demonstrably wraps the whole expression:
/// the published `@pattern` is the service's own full-match expression with that pair
/// written around it by the model, so recovering the expression the service advertises is
/// the textual inverse of that wrapping. The recordings show it applied to a pattern whose
/// top-level alternation makes the pair's placement semantically significant —
/// `com.amazonaws.lambdacore#NetworkConnectorName` is modelled
/// `^(arn:…:network-connector:…)|[a-zA-Z0-9_-]{1,64}$` and a refusal of
/// `CreateNetworkConnector$Name` quotes `(arn:…:network-connector:…)|[a-zA-Z0-9_-]{1,64}` —
/// so a renderer that declined the strip there would state an expression AWS never
/// publishes. The CHECK always runs the model's pattern as written; only this rendering
/// drops the pair.
///
/// THE CONSEQUENCE, stated plainly: an expression whose own branches carry the pair at the
/// ends (`^a$|^b$`, `^a|b$`) is quoted as `a$|^b` / `a|b`, which does NOT describe the same
/// language as the model's text. That is the rule's faithful output, not an oversight: such
/// a model text is itself the wrapping of an expression that already anchored a branch, and
/// what AWS publishes is that inner expression. A model that means an alternation of
/// whole-value branches parenthesises them (`(^a$)|(^b$)`), which carries no outer pair and
/// is quoted verbatim. Every one of these forms is a row in the module's own table test.
///
/// A SECOND RENDERING this does not implement, recorded here so the migration above carries
/// both: a service whose refusals quote the expression the WHOLE value must match decides
/// the two ends INDEPENDENTLY — an anchored end loses its anchor, and an end the model
/// leaves open is spelled with the unconstrained run (`.*`) that end admits, while an
/// expression anchored only inside its own branches is quoted verbatim. The bedrock control
/// plane is recorded stating that form (`services/service-com-amazonaws-bedrock-\
/// amazonbedrockcontrolplaneservice/src/service/validation.rs`), so its rendering stays
/// beside its own recordings until the migration can take both; this function keeps the
/// pair-positional rule the services that call it are recorded stating.
#[must_use]
pub fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => inner.to_string(),
        _ => regex.to_string(),
    }
}

/// [`unanchored`], with a top-level ALTERNATION left GROUPED so the expression quoted still
/// matches exactly what the model's anchored text matched.
///
/// Dropping the pair from `^a|b$` turns "starts with `a`, or ends with `b`" into "contains
/// `a`, or contains `b`"; wrapping what is left in one group — `(a|b)` — restores it, since
/// that expression matches, anywhere in a value, exactly what the anchored one matched at
/// the ends. A service whose recordings show the quoted expression grouped states it this
/// way; one whose recordings show it bare (lambdacore's `NetworkConnectorName`, quoted in
/// [`unanchored`]) states the plain rendering. Which of the two a service uses is part of
/// its validation-message dialect (the module docs above), so the choice stays with the
/// service and both rules live here.
///
/// The DROP is [`unanchored`]'s, called rather than restated, so the two cannot disagree on
/// it: this adds exactly the group, and only when the pair was dropped AND what is left
/// alternates at the top level. An alternation already inside a group (`^(a|b)$`) is not
/// wrapped again, and neither is an expression that keeps its pair.
#[must_use]
pub fn unanchored_grouping_alternation(regex: &str) -> String {
    let stated = unanchored(regex);
    if stated == regex || !alternates_at_top_level(&stated) {
        return stated;
    }
    format!("({stated})")
}

/// [`unanchored`], declining the drop where what the pair encloses carries an anchor of
/// its OWN — an alternation whose branches are each anchored (`^$|^arn:…$`, `^(^a$)|(^b$)$`).
///
/// Same reading of the pair as [`unanchored`]: the published `@pattern` is the expression a
/// service advertises with the whole-value pair written around it by the model, and this
/// rendering is the inverse of that wrapping. What this adds is a discriminator on what the
/// pair ENCLOSES. An expression carrying no anchor of its own is one the model wrapped,
/// whatever its top level does — so a top-level alternation alone does not stop the drop,
/// and the lambdacore recording quoted in [`unanchored`]
/// (`^(arn:…)|[a-zA-Z0-9_-]{1,64}$`, quoted as `(arn:…)|[a-zA-Z0-9_-]{1,64}`) is stated
/// identically by both renderings. An expression that anchors its own BRANCHES is already a
/// whole-value expression before any wrapping: the two ends of its text are the first and
/// last branch's anchors, so dropping them states a requirement of neither branch — what is
/// left begins with an alternative that is a bare end-of-input anchor, which every value
/// satisfies, and the branch that anchor belonged to (the empty-string branch of `^$|…`
/// included) is no longer stated at all. Such an expression is quoted as the model declares
/// it.
///
/// Which of the renderings a service states is part of its validation-message dialect (the
/// module docs above), so this is a third dialect beside the two above rather than a change
/// to either, and it shares their one reading of the pair: the DROP is [`unanchored`]'s,
/// called rather than restated.
#[must_use]
pub fn unanchored_keeping_inner_anchors(regex: &str) -> String {
    let stated = unanchored(regex);
    if stated == regex || !regex_syntax(&stated).anchor {
        return stated;
    }
    regex.to_string()
}

/// [`unanchored_keeping_inner_anchors`], declining the drop for an expression the service
/// PUBLISHES exactly as the model writes it.
///
/// A service's `@pattern` clause quotes the expression that service publishes for the
/// member's SHAPE — the `Pattern` its API reference documents — and one API can publish both
/// forms: the expression inside the whole-value pair for one shape, and the model's own text,
/// anchors included, for another. Which of the two a shape is published with cannot be read
/// off the text, because both are spelled `^…$` in the model, so it is reference data about
/// the shape: `published_as_declared` is the service's list of the expressions it publishes
/// as declared, keyed by the EXPRESSION, since what is published belongs to the shape and so
/// holds for every member targeting it — in any operation, in whatever binding carried the
/// value, and inside the constraint set stated at a collection of them.
///
/// THE HOME for that rule, so the services recorded stating both forms share one reading of
/// it and only their own rows live in their own crates. Everything else is
/// [`unanchored_keeping_inner_anchors`]'s, called rather than restated: an expression the
/// list does not name is quoted exactly as that rendering quotes it, and the CHECK is always
/// the model's pattern as written.
#[must_use]
pub fn unanchored_unless_published_as_declared(regex: &str, published_as_declared: &[&str]) -> String {
    if published_as_declared.contains(&regex) {
        return regex.to_string();
    }
    unanchored_keeping_inner_anchors(regex)
}

/// The path of `element_member` on element `position` (0-BASED) of the list member
/// `list_member`, as the framework's own constraint walk spells one: the list member, the
/// element's 1-based position and the collection step, then the element's member —
/// `assetModelHierarchies.2.member.childAssetModelId`.
///
/// Pure [`path`] plumbing, and the shared home for it: a service that words its own clause
/// about a member of a list element has to name that member exactly as the walk names it,
/// and each crate restating the 1-based rule is how two of them come to spell one position
/// differently. Several service crates still carry their own copy
/// (`grep -rn 'fn element_member_path' services/*/src`); they are to be migrated onto this
/// one rather than copied again.
#[must_use]
pub fn element_member_path(list_member: &str, position: usize, element_member: &str) -> String {
    path::join(&[
        path::member(list_member),
        path::element(position + 1),
        path::member(element_member),
    ])
}

/// The unconstrained run an expression spells for an end a `@pattern` leaves OPEN.
const ANY_RUN: &str = ".*";

/// [`unanchored`] at ONE end: the expression as the model declares it, less the `$` that
/// CLOSES it — a further dialect beside the three above, for the services whose refusals
/// quote an expression that keeps its opening `^`.
///
/// Same reading of an anchor as its siblings, and the same single purpose: a Smithy
/// `@pattern` is a FIND, so the `$` a model writes at the very end of the expression is how
/// it makes the match total THERE rather than part of the grammar a caller has to satisfy,
/// while everything else the model wrote — its leading `^`, and every anchor it writes
/// around its own branches — is quoted as declared. An expression that closes with no
/// anchor is quoted verbatim, and an escaped `\$` is a literal dollar rather than an
/// anchor, the same reading `unanchored` applies to the pair.
///
/// Which of the renderings a service states is part of its validation-message dialect (the
/// module docs above), so this is one more dialect rather than a change to any of them.
#[must_use]
pub fn unanchored_at_the_close(regex: &str) -> String {
    match regex.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => inner.to_string(),
        _ => regex.to_string(),
    }
}

/// The expression the WHOLE value must match, for the services whose refusals quote a
/// `@pattern` that way — one more dialect beside the renderings above, sharing their
/// reading of an anchor and differing in what it does with an end the model leaves open.
///
/// A Smithy `@pattern` is a FIND rather than a total match, so the expression a member
/// declares and the expression its whole value must satisfy are two different things, and a
/// refusal in this dialect states the second. The two ends are decided INDEPENDENTLY,
/// because a model may anchor one and leave the other open:
///
/// * an end the model ANCHORS carries its `^` / `$` away with it — the anchor is how the
///   model makes the match total at that end, not part of the character grammar a caller
///   has to satisfy;
/// * an end the model leaves OPEN admits any run of characters before (or after) the match,
///   which this spells as [`ANY_RUN`] unless the expression already leaves that end open
///   with one;
/// * an expression that anchors NEITHER end while carrying an anchor inside itself — an
///   alternation whose branches are each anchored (`(^a$)|(^b$)`) — already describes the
///   whole value, so it is quoted exactly as the model declares it, which is the same
///   reading [`unanchored_keeping_inner_anchors`] states;
/// * a top-level ALTERNATION left over once BOTH anchors were dropped is kept GROUPED, and
///   only when its own branches carry no anchor: the quoted expression then still matches
///   what the model's anchored text matched, which is [`unanchored_grouping_alternation`]'s
///   rule. Branches that anchor themselves pin each alternative already, so nothing is
///   added around them.
///
/// Recorded for the Bedrock family, whose refusals quote all four forms: `^[0-9a-zA-Z._:-]+$`
/// as `[0-9a-zA-Z._:-]+`, `^(arn:…flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$` as
/// `((arn:…flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))`, `^[a-zA-Z0-9-]{1,36}$|^(arn:…)$` as
/// `[a-zA-Z0-9-]{1,36}$|^(arn:…)` and the model-identifier alternation `(^…)$|(^…)$` as
/// `.*(^…)$|(^…)`. The CHECK always runs the model's pattern as written; only this
/// rendering decides the ends.
#[must_use]
pub fn whole_value_expression(regex: &str) -> String {
    let (body, anchored_at_start, anchored_at_end) = split_outer_anchors(regex);
    match (anchored_at_start, anchored_at_end) {
        (true, true) => grouping_an_unanchored_alternation(body),
        // Anchored inside its own branches rather than at either end.
        (false, false) if regex_syntax(regex).anchor => regex.to_string(),
        (start, end) => opened(body, !start, !end),
    }
}

/// `regex` without the anchors at its two ends, and whether each end carried one. The two
/// are read independently, and an escaped `$` is a literal dollar rather than an anchor —
/// the same reading [`unanchored`] applies to the pair as a whole.
fn split_outer_anchors(regex: &str) -> (&str, bool, bool) {
    let (body, anchored_at_start) = match regex.strip_prefix('^') {
        Some(rest) => (rest, true),
        None => (regex, false),
    };
    match body.strip_suffix('$') {
        Some(inner) if !ends_with_escape(inner) => (inner, anchored_at_start, true),
        _ => (body, anchored_at_start, false),
    }
}

/// `body` wrapped in ONE group when it alternates at its top level and carries no anchor of
/// its own, else `body` itself.
fn grouping_an_unanchored_alternation(body: &str) -> String {
    let syntax = regex_syntax(body);
    if syntax.top_level_alternation && !syntax.anchor {
        return format!("({body})");
    }
    body.to_string()
}

/// `body` with [`ANY_RUN`] spelled at each open end, unless the expression already leaves
/// that end open with one.
fn opened(body: &str, at_start: bool, at_end: bool) -> String {
    let mut expression = String::with_capacity(body.len() + 2 * ANY_RUN.len());
    if at_start && !body.starts_with(ANY_RUN) {
        expression.push_str(ANY_RUN);
    }
    expression.push_str(body);
    if at_end && !body.ends_with(ANY_RUN) {
        expression.push_str(ANY_RUN);
    }
    expression
}

/// Whether `regex` ANCHORS ITS OWN ENDS: every alternative it offers at its top level —
/// each read through a group that wraps it, since a model wraps an alternation, or a branch
/// of one, to keep it together — begins with `^` and ends with an unescaped `$`.
///
/// The one fact every rendering above decides an expression's ends on, published here
/// because a service's own dialect needs it too: an expression that already pins both ends
/// of each of its branches (`(^a$|^b$)`) describes the whole value on its own, so the
/// `^`/`$` a model writes AROUND it is that model's wrapping rather than part of what a
/// refusal advertises. An expression carrying an anchor that pins neither end (`a$b`) is not
/// one of those, which is why this asks about the ends rather than about the mere presence
/// of an anchor.
#[must_use]
pub fn anchors_its_own_ends(regex: &str) -> bool {
    top_level_alternatives(inside_a_wrapping_group(regex))
        .iter()
        .all(|branch| pins_both_ends(inside_a_wrapping_group(branch)))
}

/// Whether `branch` opens with a `^` and closes with an unescaped `$`, with something
/// between them or nothing at all (`^$`, the empty-string branch, pins both ends too).
fn pins_both_ends(branch: &str) -> bool {
    branch.starts_with('^')
        && branch.len() > 1
        && branch
            .strip_suffix('$')
            .is_some_and(|without| !ends_with_escape(without))
}

/// `regex` read through every group that wraps the WHOLE expression — `(^a$|^b$)` is the
/// alternation `^a$|^b$`, and a non-capturing group's `?:` is read through with it.
fn inside_a_wrapping_group(regex: &str) -> &str {
    let mut inner = regex;
    while let Some(unwrapped) = inside_one_wrapping_group(inner) {
        inner = unwrapped;
    }
    inner
}

/// `regex` without the ONE group that wraps the whole expression, or `None` when its first
/// character is not a group's `(` that closes at its last.
fn inside_one_wrapping_group(regex: &str) -> Option<&str> {
    let body = regex.strip_prefix('(')?;
    let inner = body.strip_suffix(')')?;
    // A `\)` is a literal parenthesis, and a `)` that closes a group OPENED inside the body
    // is not this one's: `(a)(b)` is two groups, not one wrapping `a)(b`.
    if ends_with_escape(inner) || !balanced(inner) {
        return None;
    }
    Some(inner.strip_prefix("?:").unwrap_or(inner))
}

/// Whether `text`'s own groups are balanced — no `)` without a `(` before it, and none left
/// open at the end — read the way [`regex_syntax`] reads an expression.
fn balanced(text: &str) -> bool {
    let mut depth = 0_i32;
    let mut escaped = false;
    let mut in_class: Option<usize> = None;
    for character in text.chars() {
        if escaped {
            escaped = false;
            in_class = in_class.map(|seen| seen + 1);
            continue;
        }
        match (character, in_class) {
            ('\\', _) => escaped = true,
            ('^', Some(0)) => {}
            (']', Some(0)) => in_class = Some(1),
            (']', Some(_)) => in_class = None,
            (_, Some(seen)) => in_class = Some(seen + 1),
            ('[', None) => in_class = Some(0),
            ('(', None) => depth += 1,
            (')', None) => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// `text` split at the `|`s that separate the branches of the WHOLE expression: those
/// neither inside a group nor inside a character class, where a `|` is the literal
/// character — the same reading [`regex_syntax`] applies.
fn top_level_alternatives(text: &str) -> Vec<&str> {
    let mut branches = Vec::new();
    let mut start = 0_usize;
    let mut escaped = false;
    let mut in_class: Option<usize> = None;
    let mut depth = 0_usize;
    for (index, character) in text.char_indices() {
        if escaped {
            escaped = false;
            in_class = in_class.map(|seen| seen + 1);
            continue;
        }
        match (character, in_class) {
            ('\\', _) => escaped = true,
            ('^', Some(0)) => {}
            (']', Some(0)) => in_class = Some(1),
            (']', Some(_)) => in_class = None,
            (_, Some(seen)) => in_class = Some(seen + 1),
            ('[', None) => in_class = Some(0),
            ('(', None) => depth += 1,
            (')', None) => depth = depth.saturating_sub(1),
            ('|', None) if depth == 0 => {
                branches.push(&text[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    branches.push(&text[start..]);
    branches
}

/// Whether `text` alternates at its TOP level — a `|` that is not inside a group and not
/// inside a character class, where it is the literal character.
fn alternates_at_top_level(text: &str) -> bool {
    regex_syntax(text).top_level_alternation
}

/// What a regular expression spells in its own syntax, as the renderings above decide on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RegexSyntax {
    /// A `|` that separates branches of the whole expression: one that is neither inside a
    /// group nor inside a character class, where it is the literal character.
    top_level_alternation: bool,
    /// A `^` or `$` the expression ANCHORS with: one that neither negates a character
    /// class, nor sits inside one, nor is escaped into a literal.
    anchor: bool,
}

/// [`RegexSyntax`] of `text`, in ONE scan. Escapes are honoured throughout, so an escaped
/// `|`, `[`, `]`, `(`, `)`, `^` or `$` is a literal.
///
/// A `]` in the FIRST position of a class is the literal bracket rather than the class's
/// end (`[]|]` is the two-character class `]|`, not an empty class followed by `|]`), and a
/// leading `^` does not count as a position, so `[^]|]` is read the same way. Tracking the
/// position inside the class is what keeps a `|` in such a class from being read as an
/// alternation, and a `^` that negates one from being read as an anchor.
fn regex_syntax(text: &str) -> RegexSyntax {
    let mut syntax = RegexSyntax::default();
    let mut escaped = false;
    // `Some(n)`: inside a class, with `n` characters seen in it (a leading `^` excluded).
    let mut in_class: Option<usize> = None;
    let mut depth = 0_usize;
    for character in text.chars() {
        if escaped {
            escaped = false;
            // An escaped character still occupies a position in the class it is in.
            in_class = in_class.map(|seen| seen + 1);
            continue;
        }
        match (character, in_class) {
            ('\\', _) => escaped = true,
            // The negation marker, which is not a position of its own.
            ('^', Some(0)) => {}
            // A `]` where the class has no members yet is the literal bracket.
            (']', Some(0)) => in_class = Some(1),
            (']', Some(_)) => in_class = None,
            (_, Some(seen)) => in_class = Some(seen + 1),
            ('[', None) => in_class = Some(0),
            ('(', None) => depth += 1,
            (')', None) => depth = depth.saturating_sub(1),
            ('|', None) if depth == 0 => syntax.top_level_alternation = true,
            ('^' | '$', None) => syntax.anchor = true,
            _ => {}
        }
    }
    syntax
}

/// Whether `text` ends in an escape — an odd number of trailing backslashes, so the
/// character that followed them was escaped.
fn ends_with_escape(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The list path a leaf is a SCALAR ELEMENT of, if it is one.
///
/// A list element's path is `<list>.<1-based position>.member`
/// ([`aws_smithy_schema_server::constraints::path::element`]), so the list is that path
/// minus those two segments. Anything else — a struct member, a map entry value — belongs
/// to no list's uniqueness check.
///
/// Public because a service that states a clause at the LIST member when one of its
/// ELEMENTS failed has to recognise an element path to do it, and that spelling belongs
/// with the walk that mirrors it rather than being re-derived per service.
#[must_use]
pub fn list_of_element(leaf_path: &str) -> Option<&str> {
    let head = leaf_path.strip_suffix(".member")?;
    let (list, position) = head.rsplit_once('.')?;
    let is_position = !position.is_empty() && position.bytes().all(|byte| byte.is_ascii_digit());
    (is_position && !list.is_empty()).then_some(list)
}

/// Whether `stored` is what the framework's renderer makes of `submitted`: the value
/// itself, or a proper prefix of it followed by the marker naming its byte count.
///
/// Stated as a relation rather than by re-deriving the elision, so this does not depend
/// on the cap the frozen crate elides at — only on the shape of what it writes.
///
/// `pub(crate)` because the other half of this crate's clause rendering
/// ([`crate::validation_message::quoting_the_whole_submitted_value`]) restores a clause the
/// framework elided and has to recognise the same relation, which belongs with the walk that
/// mirrors that renderer rather than being re-derived beside it.
pub(crate) fn is_elision_of(stored: &str, submitted: &str) -> bool {
    if stored == submitted {
        return true;
    }
    let marker = format!("... [{} bytes]", submitted.len());
    stored
        .strip_suffix(&marker)
        .is_some_and(|prefix| prefix.len() < submitted.len() && submitted.starts_with(prefix))
}

/// Every list of SCALAR elements `input` carried, keyed by the member path the validating
/// walk reports that list at, and rendered as the value AWS states AT that member: the
/// elements in position order, comma-separated, in brackets ([`rendered_list`]).
///
/// What this is for: a constraint on a list's element shape is violated by ONE element and
/// the framework reports it at that element (`eventPublishers.1.member`), while AWS reports
/// it at the list member and echoes the value that member holds — the WHOLE list, which no
/// violation carries. A service wording such a clause reads the list from here rather than
/// rebuilding the walk, and never from a violation.
///
/// A list this walk could not collect in FULL is NOT offered, so a clause can never state
/// a list the caller did not send. The leaf collection is bounded ([`MAX_RECORDED_LEAVES`],
/// [`MAX_RESTORED_ECHO`], [`MAX_RECORDED_BYTES`]) and a leaf those bounds drop leaves a hole
/// the leaves alone cannot reveal — a short list reads exactly like a short list the caller
/// sent — so once ANY leaf has been dropped ([`SubmittedValues::skipped`]) nothing is
/// offered at all and the framework's own echo stands. Within the bounds every rendering is
/// covered by [`MAX_RECORDED_BYTES`], the budget the leaves themselves were collected
/// under.
///
/// A list whose elements have no string rendering (a list of structures, a list of blobs)
/// contributes no entry either, for the same reason: its elements are not recorded.
#[must_use]
pub fn submitted_lists(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> HashMap<String, String> {
    let (leaves, complete) = collected_leaves(input, schema, MAX_RESTORED_ECHO);
    if !complete {
        return HashMap::new();
    }
    let mut elements: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for (leaf_path, value) in &leaves {
        let Some(list) = list_of_element(leaf_path) else {
            continue;
        };
        let held = elements.entry(list).or_default();
        if held.is_empty() {
            order.push(list);
        }
        held.push(value.as_str());
    }
    order
        .into_iter()
        .map(|list| (list.to_string(), rendered_list(&elements[list])))
        .collect()
}

/// Every string-rendered leaf of `input` this walk is allowed to collect, in walk order,
/// paired with the dotted member path the validating walk reports a violation on it at.
///
/// Public because restoring a violation's echo is not the only thing a service needs the
/// values a request SUBMITTED for: a service that runs its own check over a caller-
/// supplied value — an ARN it parses itself, ahead of the constraints the model declares
/// — needs the same (path, value) pairs for a value the framework never reports a
/// violation on. Reading them here keeps ONE walk in the tree: a second copy would mirror
/// the frozen crate's path bookkeeping twice and the two could drift apart.
///
/// The bounds the module docs state apply: at most [`MAX_RECORDED_LEAVES`] leaves and
/// [`MAX_RECORDED_BYTES`] of collected bytes, so this is never a second full copy of a
/// request's strings.
#[must_use]
pub fn submitted_leaves(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(String, String)> {
    collected_leaves(input, schema, MAX_RESTORED_ECHO).0
}

/// [`submitted_leaves`], collecting a leaf up to `max_value` bytes rather than up to
/// [`MAX_RESTORED_ECHO`].
///
/// A leaf no clause could quote is not copied at all, so the collection bound and the
/// quoting bound are the same number: a service that states its own ceiling
/// ([`refusing_violations_quoting_up_to`]) collects to that ceiling. The COUNT and TOTAL
/// bounds ([`MAX_RECORDED_LEAVES`], [`MAX_RECORDED_BYTES`]) are unchanged, so the walk is
/// still never a second full copy of a request's strings.
#[must_use]
pub fn submitted_leaves_up_to(
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_value: usize,
) -> Vec<(String, String)> {
    collected_leaves(input, schema, max_value).0
}

/// The collected leaves of `input`, and whether the collection is COMPLETE — false once any
/// string-rendered leaf was dropped by one of the stated bounds (see
/// [`SubmittedValues::skipped`]).
///
/// A caller that reads ONE named leaf does not care: a dropped leaf yields no candidate and
/// the framework's own text stands. A caller that reads a whole COLLECTION does, because a
/// hole in the leaves is invisible in the collection it renders.
fn collected_leaves(
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_value: usize,
) -> (Vec<(String, String)>, bool) {
    let mut walk = SubmittedValues {
        max_value: Some(max_value),
        ..SubmittedValues::default()
    };
    // The root input is a struct; drive it the way a codec — and the validating walk —
    // does.
    let _ = walk.write_struct(schema, input);
    (walk.leaves, !walk.skipped)
}

/// Every member position of `input` this walk reaches, in walk order, paired with the
/// rendering a clause states the value held there with — `None` for a value this walk
/// produces no text rendering for (a blob, a map, a document) or one past its bounds.
///
/// The SIBLING of [`submitted_leaves`] and [`submitted_lists`], over the SAME walk and so
/// the same path spelling: the positions are the ones the validating walk reports a
/// violation at, including a COMPOSITE member (a list, a structure), which that walk
/// reports a `@length` clause on and has only a count rendering for.
///
/// A composite renders from its children: a list as `[<element>, …]`, a structure as
/// `<ShapeName>(<member>=<value>, …)` over the members that were present. A composite
/// with any unrenderable child renders as `None` rather than as one missing a part of the
/// value, and so does a map (no rule for how AWS states one in a clause is known here)
/// and a structure reached as a struct MEMBER, whose schema names the member rather than
/// the shape. The other structure form — every member the shape declares, an unset one as
/// `null` — is [`refusing_violations_with_declared_echoes_over`]'s.
///
/// Bounded by [`MAX_RECORDED_RENDERINGS`], [`MAX_RECORDED_RENDERED_BYTES`],
/// [`MAX_RENDERED_CHILDREN`] and [`MAX_RESTORED_ECHO`], so neither the memory nor the
/// text this produces is sized by the request.
#[must_use]
pub fn submitted_renderings(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> Vec<(String, Option<String>)> {
    submitted_renderings_over(input, schema, MAX_RECORDED_LEAVES)
}

/// [`submitted_renderings`], over as many member positions as the CALLER states rather
/// than [`MAX_RECORDED_LEAVES`].
///
/// The count bounds the positions VISITED and the positions RECORDED alike, because a
/// composite renders only from children the walk reached: a ceiling that stopped the walk
/// short, or that dropped the composite's own position, would answer no rendering at all
/// for the very value the caller asked about. The BYTES this may hold are unchanged
/// ([`MAX_RECORDED_RENDERED_BYTES`]), so a larger count cannot make the collection
/// request-sized.
#[must_use]
pub fn submitted_renderings_over(
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_positions: usize,
) -> Vec<(String, Option<String>)> {
    renderings_in(
        input,
        schema,
        max_positions,
        StructureForm::MembersSet,
        MapForm::Unrendered,
        MAX_RESTORED_ECHO,
        MAX_RESTORED_ECHO,
    )
}

/// [`submitted_renderings_over`], with the STRUCTURE and MAP forms the caller states, the
/// ceiling one LEAF rendering is kept up to and the ceiling one COMPOSITE rendering is kept
/// up to: the one walk every form runs.
#[allow(clippy::too_many_arguments)]
fn renderings_in(
    input: &dyn SerializableStruct,
    schema: &Schema,
    max_positions: usize,
    structures: StructureForm,
    maps: MapForm,
    max_echo: usize,
    max_render: usize,
) -> Vec<(String, Option<String>)> {
    // The ONLY walk that composes renderings: a caller that reads one named leaf
    // ([`submitted_leaves`], [`submitted_lists`]) pays nothing for them.
    let mut walk = SubmittedValues {
        renders: true,
        max_positions: Some(max_positions),
        max_echo: Some(max_echo),
        max_render: Some(max_render),
        structures,
        maps,
        ..SubmittedValues::default()
    };
    let _ = walk.write_struct(schema, input);
    walk.renderings
}

/// One child of an open composite: the member name it reported under (absent for a
/// collection element), the shape that DECLARES that member, the map KEY it was held
/// under, and how its value renders.
#[derive(Debug)]
struct RenderedChild {
    name: Option<String>,
    /// The name of the shape this child is a member of, for a child written against a
    /// MEMBER schema; `None` for a collection element. It is how a structure reached as a
    /// struct member learns its own shape's name: that member schema names the member,
    /// while the members the structure itself wrote name the structure (see
    /// [`declaring_shape`]).
    declared_by: Option<String>,
    /// The key of the MAP entry this child is the value of, for a child written inside a
    /// map frame; `None` everywhere else. A map value is written against the map MEMBER's
    /// schema, which names the member and not the entry, so the key is the one thing about
    /// such a child the schema cannot carry.
    key: Option<String>,
    rendering: Option<String>,
}

/// The name of the shape that DECLARES `schema`, when `schema` is a member schema.
///
/// A member's shape id is `<namespace>#<Shape>$<member>`, so the shape it belongs to is
/// the part between the two separators. `None` for anything that is not a member schema.
fn declaring_shape(schema: &Schema) -> Option<String> {
    schema.member_name()?;
    let id = schema.shape_id().to_string();
    let (_, qualified) = id.split_once('#')?;
    let (shape, _) = qualified.split_once('$')?;
    Some(shape.to_string())
}

/// How a member the request left UNSET is stated inside a rendered structure.
///
/// `pub` for the same reason [`rendered_record`] is: a HANDLER that words its own clause
/// about a value no walk ahead of it reported has to spell an unset member the same way a
/// clause restored from the walk does.
pub const UNSET_MEMBER: &str = "null";

/// How a BLOB member is stated inside a rendered structure, and wherever a clause renders
/// one: the buffer the validating front end read it into, which states the value's LENGTH
/// three ways and nothing of its bytes.
///
/// A blob's content is never echoed — it is not text, and a clause that quoted it would
/// return the caller's payload — so this is the only rendering a blob has; it is what lets a
/// structure carrying one render at all, and with it the list that structure belongs to.
///
/// `pub` for the same reason [`UNSET_MEMBER`] is.
#[must_use]
pub fn rendered_blob(bytes: usize) -> String {
    format!("java.nio.HeapByteBuffer[pos=0 lim={bytes} cap={bytes}]")
}

/// `entries` as a map renders, with the entries put in KEY order first — the form a clause
/// quotes a map-valued member in, stated the one way the same map is stated twice.
///
/// Key order because a map reaches a handler (and this walk) as the built input's own
/// unordered map, so the order its entries were written in is not the order the request
/// carried them in. `pub` for the same reason [`rendered_map`] is.
///
/// `None` when the rendering would be longer than the largest value a clause quotes
/// ([`MAX_RESTORED_ECHO`]) — measured BEFORE anything is joined, so an oversized rendering
/// is never allocated, and the clause then states no value at all, the valueless form
/// [`crate::validation_message::map_key_requirements_refusal`] takes. That is what keeps one
/// request from sizing the message it is refused with: the map a caller sends is bounded by
/// nothing this module can see, and each entry of it renders to more text than it occupied.
#[must_use]
pub fn rendered_map_in_key_order(entries: &[(&str, String)]) -> Option<String> {
    // The braces, plus `<key>=<value>` and `, ` per entry — the same measurement
    // [`rendered_entries`] makes for the walk's own map rendering.
    let mut length = 2;
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            length += 2; // ", "
        }
        length += key.len() + 1 + value.len();
        if length > MAX_RESTORED_ECHO {
            return None;
        }
    }
    let mut ordered: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    ordered.sort_unstable_by_key(|(key, _)| *key);
    Some(rendered_map(&ordered))
}

/// `members` as a STRUCTURE of shape `name` renders: the shape's name over the members
/// given, each `<member>=<value>`, in the order given — the form the AWS SDKs and the
/// validation layer in front of a service print a modelled structure in.
///
/// THE ONE renderer of that form, called by this module's own walk ([`rendered_structure`],
/// [`rendered_declared_structure`]) and `pub` for the same reason [`rendered_map`] is: the
/// constraint classes no walk ahead of a handler reports (a map's KEY, a cap the SERVICE
/// rather than the model states) are stated by the HANDLER, which has to quote a structure
/// the same way a clause restored from the walk does. What a member the request left unset
/// renders as is the caller's ([`UNSET_MEMBER`] is how this module's own declared-member
/// form states one), and so is the member ORDER, which a clause states as the model
/// declares it.
#[must_use]
pub fn rendered_record(name: &str, members: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = members
        .iter()
        .map(|(member, value)| format!("{member}={value}"))
        .collect();
    format!("{name}({})", rendered.join(", "))
}

/// The children of one composite being walked, and whether more arrived than the walk's
/// child-count ceiling allows ([`SubmittedValues::child_bound`]), one of them was larger
/// than a composite may hold, or one of them was never written.
#[derive(Debug, Default)]
struct Composite {
    children: Vec<RenderedChild>,
    /// Rendering bytes the children hold together, bounded by [`MAX_RESTORED_ECHO`]: a
    /// composite that could never be quoted in a clause is not worth holding, and holding
    /// it is what would let one request's text be retained at every nesting level.
    bytes: usize,
    truncated: bool,
}

impl Composite {
    /// Take one child, or mark this composite truncated once it is full — by child COUNT,
    /// up to `max_children`, or by the bytes its children already hold, up to `max_bytes`
    /// (the walk's own ceilings for a composite's rendering). A truncated composite renders
    /// as nothing ([`SubmittedValues::close_children`]), so dropping a child never states a
    /// value missing a part of itself.
    fn take(&mut self, child: RenderedChild, max_children: usize, max_bytes: usize) {
        let bytes = child.name.as_ref().map_or(0, String::len)
            + child.key.as_ref().map_or(0, String::len)
            + child.rendering.as_ref().map_or(0, String::len);
        if self.children.len() >= max_children || self.bytes + bytes > max_bytes {
            self.truncated = true;
            return;
        }
        self.bytes += bytes;
        self.children.push(child);
    }
}

/// The text `children` would render to, including the separators and brackets a list or a
/// structure puts between them, or `None` when any child has no text rendering.
///
/// Measured BEFORE anything is joined, so a rendering past [`MAX_RESTORED_ECHO`] is never
/// allocated only to be discarded.
fn rendered_length(children: &[RenderedChild], named: bool) -> Option<usize> {
    let mut length = 2; // the brackets, or the parentheses after a shape's name
    for (index, child) in children.iter().enumerate() {
        if index > 0 {
            length += 2; // ", "
        }
        if named {
            // `<member>=`
            length += child.name.as_ref()?.len() + 1;
        }
        length += child.rendering.as_ref()?.len();
    }
    Some(length)
}

/// `children` as a list renders: the elements in order, comma-separated, in brackets,
/// through the one renderer of that form ([`rendered_list`]). `None` when any element has
/// no text rendering, so a list of maps states no value rather than one missing an
/// element, and `None` when the rendering would be longer than [`MAX_RESTORED_ECHO`] —
/// measured before it is built, so an oversized rendering is never allocated.
fn rendered_sequence(children: &[RenderedChild], max_render: usize) -> Option<String> {
    if rendered_length(children, false)? > max_render {
        return None;
    }
    let elements: Vec<&str> = children
        .iter()
        .map(|child| child.rendering.as_deref())
        .collect::<Option<Vec<_>>>()?;
    Some(rendered_list(&elements))
}

/// `children` as a MAP renders: its entries in KEY order, each `<key>=<value>`, in braces,
/// through the one renderer of that form ([`rendered_map`]).
///
/// `None` when an entry's value has no text rendering, so a map of blobs states no value
/// rather than one missing an entry, and `None` when the rendering would be longer than
/// [`MAX_RESTORED_ECHO`] — measured before it is built, so an oversized rendering is never
/// allocated. A child the walk recorded no key for is not an entry of a map at all, which
/// is the same `None`.
///
/// Why KEY order: see [`refusing_violations_with_mapped_echoes`]. The built input's map is
/// unordered, so the order the entries were written in is not the request's.
fn rendered_entries(children: &[RenderedChild], max_render: usize) -> Option<String> {
    // The braces, plus `<key>=<value>` and `, ` per entry — the same measurement
    // `rendered_length` makes for a named composite, with the key in place of the member.
    let mut length = 2;
    let mut entries: Vec<(&str, &str)> = Vec::with_capacity(children.len());
    for child in children {
        if !entries.is_empty() {
            length += 2; // ", "
        }
        let key = child.key.as_deref()?;
        let value = child.rendering.as_deref()?;
        length += key.len() + 1 + value.len();
        if length > max_render {
            return None;
        }
        entries.push((key, value));
    }
    entries.sort_unstable_by_key(|(key, _)| *key);
    Some(rendered_map(&entries))
}

/// `children` as a structure renders: the shape's name over the members that were
/// present, each `<member>=<value>`, in the order the value wrote them — the form the AWS
/// SDKs print a modelled structure in. `None` when a member that WAS written has no text
/// rendering, so a structure carrying a map states no value at all rather than
/// one missing a part of itself. (An ABSENT member is never written, so it is not a child
/// here and does not suppress the rendering; a BLOB renders as the buffer it was read into,
/// [`rendered_blob`].) `None` too when the rendering would be
/// longer than [`MAX_RESTORED_ECHO`], measured before it is built.
fn rendered_structure(name: &str, children: &[RenderedChild], max_render: usize) -> Option<String> {
    if name.len() + rendered_length(children, true)? > max_render {
        return None;
    }
    let members: Vec<(&str, &str)> = children
        .iter()
        .map(|child| {
            let member = child.name.as_deref()?;
            let rendering = child.rendering.as_deref()?;
            Some((member, rendering))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(rendered_record(name, &members))
}

/// `children` as a structure renders over the members `declared` names: the shape's name
/// over EVERY member its shape declares, in declaration order, each `<member>=<value>`,
/// with one the request left unset stated as [`UNSET_MEMBER`] — the form the validation
/// layer in front of an AWS service prints a modelled structure in
/// ([`refusing_violations_with_declared_echoes_over`]).
///
/// `None` on the same terms as [`rendered_structure`]: a member that WAS written and has
/// no text rendering suppresses the whole structure, so one carrying a map
/// states no value at all rather than one missing a part of itself, and so does a
/// rendering longer than [`MAX_RESTORED_ECHO`], measured before it is built.
fn rendered_declared_structure(
    name: &str,
    declared: &[String],
    children: &[RenderedChild],
    max_render: usize,
) -> Option<String> {
    let mut length = name.len() + 2;
    let mut members: Vec<(&str, &str)> = Vec::with_capacity(declared.len());
    for member in declared {
        let rendering = match children
            .iter()
            .find(|child| child.name.as_deref() == Some(member.as_str()))
        {
            Some(child) => child.rendering.as_deref()?,
            None => UNSET_MEMBER,
        };
        if !members.is_empty() {
            length += 2; // ", "
        }
        length += member.len() + 1 + rendering.len();
        if length > max_render {
            return None;
        }
        members.push((member.as_str(), rendering));
    }
    Some(rendered_record(name, &members))
}

/// The members the shape `name` is PRINTED with, as the caller stated them
/// ([`PrintedMembers`]), or `None` when it named no such shape — which renders that shape
/// over the members its own schema declares instead.
fn printed_members(printed: PrintedMembers, name: &str) -> Option<Vec<String>> {
    printed
        .iter()
        .find(|(shape, _)| *shape == name)
        .map(|(_, members)| members.iter().map(|member| (*member).to_string()).collect())
}

/// The members `schema` DECLARES, in declaration order, spelled as a clause spells a
/// member — empty for a schema that declares none, and for a member schema, which carries
/// the member's own name rather than its target shape's members.
fn declared_members(schema: &Schema) -> Vec<String> {
    schema
        .members()
        .iter()
        .filter_map(|member| member.member_name())
        .map(path::member)
        .collect()
}

/// Whether a value's rendering is also the one a clause ABOUT that value may quote, or only
/// the one the composite holding it prints it with.
///
/// A BLOB is the second: the structure it sits in prints it as the buffer it was read into
/// ([`rendered_blob`]), while a clause about the blob member itself states no value — AWS
/// echoes no blob content, and the length alone is not a value any caller sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    /// The rendering answers a clause about this very position, and the composite above it.
    Own,
    /// The rendering is the enclosing composite's only.
    ParentOnly,
}

/// A frame of the collection stack, mirroring the one the validating walk keeps so a
/// leaf's path is spelled identically.
///
/// The generated `serialize_members` drives a list as a sequence of element writes and a
/// map as alternating key/value writes, both against the collection member's schema; a
/// struct pushes a shield, because its members report under their own names rather than
/// as elements of the collection enclosing the struct.
#[derive(Debug)]
enum Frame {
    /// A list: each element contributes `<1-based index>.member`.
    List { index: usize },
    /// A map: entries alternate key / value, and the captured key is the value's segment.
    Map {
        expecting_key: bool,
        pending_key: Option<String>,
    },
    /// Inside a struct's `serialize_members`: the enclosing collection is invisible.
    Struct,
}

/// A [`ShapeSerializer`] that records each string-rendered leaf of a built value with
/// the path the validating walk names it by, instead of producing output.
///
/// It checks nothing: the constraints have already been checked, and this walk exists
/// only to recover the values those checks elided. What it collects is bounded by
/// [`MAX_RECORDED_LEAVES`] and [`MAX_RECORDED_BYTES`], so neither its memory nor its
/// traversal is sized by the request.
#[derive(Debug, Default)]
struct SubmittedValues {
    /// Recorded leaves in walk order (a `@uniqueItems` duplicate is the first repeat in
    /// that order).
    leaves: Vec<(String, String)>,
    /// Owned bytes recorded so far: path plus value, both caller-sized.
    recorded: usize,
    /// Every member position reached, paired with its clause rendering — see
    /// [`submitted_renderings`]. Empty unless `renders`.
    renderings: Vec<(String, Option<String>)>,
    /// Owned bytes `renderings` holds: positions plus renderings.
    rendered: usize,
    /// Leaf positions VISITED, recorded or not: the walk's own stop condition
    /// ([`Self::done`]), which must not depend on whether a leaf was kept.
    visited: usize,
    /// Whether this walk composes renderings at all. Off by default, so a caller that
    /// only reads leaves pays neither the frames nor the owned text for them.
    renders: bool,
    /// Which members a STRUCTURE renders over, as the caller stated it
    /// ([`StructureForm`]).
    structures: StructureForm,
    /// Whether a MAP renders at all, as the caller stated it ([`MapForm`]).
    maps: MapForm,
    /// The member positions this walk may visit and record, or `None` for
    /// [`MAX_RECORDED_LEAVES`], the count this module states. A caller whose model
    /// declares a collection larger than that count states its own ceiling
    /// ([`submitted_renderings_over`]), because a composite renders only from children the
    /// walk reached.
    max_positions: Option<usize>,
    /// The largest single leaf this walk copies, or `None` for [`MAX_RESTORED_ECHO`], the
    /// bound this module states. A caller that quotes a value up to a ceiling of its own
    /// ([`submitted_leaves_up_to`]) collects to that same ceiling, because a leaf no clause
    /// could quote is not worth copying.
    max_value: Option<usize>,
    /// The largest single LEAF rendering this walk keeps, or `None` for
    /// [`MAX_RESTORED_ECHO`], the bound this module states. A caller that quotes one value
    /// up to a ceiling of its own
    /// ([`refusing_violations_with_declared_echoes_quoting_up_to`]) keeps a rendering up to
    /// that same ceiling, because a rendering no clause could quote is not worth holding. A
    /// COMPOSITE's rendering is bounded by [`Self::max_render`], which a caller states
    /// separately: a composite is built from many of a caller's values rather than being
    /// one of them.
    max_echo: Option<usize>,
    /// The largest single COMPOSITE rendering this walk keeps, or `None` for
    /// [`MAX_RESTORED_ECHO`], the bound this module states. A caller whose recordings show
    /// a clause quoting a whole collection larger than that states its own ceiling
    /// ([`refusing_violations_with_declared_echoes_rendering_up_to`]), from the declared
    /// maximum of the collection in question, so the ceiling is a model constant and the
    /// text this walk holds is not sized by the request.
    max_render: Option<usize>,
    /// The composites currently open, innermost last, collecting the children each
    /// renders from. Pushed and popped with `collections`.
    open: Vec<Composite>,
    /// Dotted path segments from the root to the value being written.
    path: Vec<String>,
    /// Active collection frames, innermost last.
    collections: Vec<Frame>,
    /// Set once any string-rendered leaf was DROPPED rather than recorded — because the
    /// leaf cap stopped the walk, or because one value was larger than
    /// [`MAX_RESTORED_ECHO`], or because the byte budget was spent. The collection is then
    /// no longer a faithful picture of the request, which matters to a caller that reads a
    /// WHOLE collection out of it ([`submitted_lists`]) rather than one named leaf: with a
    /// leaf missing, a list would render short, stating a value the caller never sent.
    /// Restoring a single named echo is unaffected, since an absent leaf simply yields no
    /// candidate.
    skipped: bool,
}

impl SubmittedValues {
    /// The largest single leaf this walk copies; see [`Self::max_value`].
    fn value_bound(&self) -> usize {
        self.max_value.unwrap_or(MAX_RESTORED_ECHO)
    }

    /// The largest single leaf rendering this walk keeps; see [`Self::max_echo`].
    fn echo_bound(&self) -> usize {
        self.max_echo.unwrap_or(MAX_RESTORED_ECHO)
    }

    /// The largest single composite rendering this walk keeps; see [`Self::max_render`].
    fn render_bound(&self) -> usize {
        self.max_render.unwrap_or(MAX_RESTORED_ECHO)
    }

    /// True once no further leaf can be recorded, so the rest of the walk is skipped.
    ///
    /// Counted over every leaf VISITED rather than over the leaves kept: a leaf dropped
    /// for its size is not recorded, so a request made of oversized values would
    /// otherwise keep this false and let the walk run over every member of it.
    ///
    /// Reaching the cap also means a leaf the walk has not got to will be dropped, so it
    /// records that the collection is incomplete.
    fn done(&mut self) -> bool {
        let done = self.visited >= self.position_bound();
        self.skipped |= done;
        done
    }

    /// The member positions this walk may visit and record; see [`Self::max_positions`].
    fn position_bound(&self) -> usize {
        self.max_positions.unwrap_or(MAX_RECORDED_LEAVES)
    }

    /// The positions this walk may RECORD a rendering for — leaves and the composites they
    /// are children of alike. The same stated number as [`Self::position_bound`], counted
    /// separately from it: a composite's own position is recorded after its children's, so
    /// one request fills this count faster than the leaf count.
    fn recorded_renderings_bound(&self) -> usize {
        self.max_positions.unwrap_or(MAX_RECORDED_RENDERINGS)
    }

    /// The children ONE composite may render from: the same stated ceiling the walk VISITS
    /// positions under ([`Self::position_bound`]), because a composite renders only from
    /// children the walk reached — so a service that states its own position ceiling, from
    /// the declared maximum of a collection longer than [`MAX_RENDERED_CHILDREN`], gets that
    /// collection rendered rather than a count it could never have echoed
    /// ([`refusing_violations_with_echoes_over`]). A caller that states none keeps this
    /// module's own count, which is the same number as the default leaf budget.
    fn child_bound(&self) -> usize {
        self.max_positions.unwrap_or(MAX_RENDERED_CHILDREN)
    }

    /// The lowerCamel path segment for `schema`, if it is a member schema.
    fn segment_for(schema: &Schema) -> Option<String> {
        schema.member_name().map(path::member)
    }

    /// Push `schema`'s member-name segment, run `body`, then pop. A schema that is not a
    /// member schema (the root input) leaves the path unchanged.
    fn with_segment<R>(&mut self, schema: &Schema, body: impl FnOnce(&mut Self) -> R) -> R {
        match Self::segment_for(schema) {
            Some(segment) => {
                self.path.push(segment);
                let out = body(self);
                self.path.pop();
                out
            }
            None => body(self),
        }
    }

    /// The path segment for a child written directly inside the innermost collection
    /// frame, advancing list state; `None` when this write is not a collection child, so
    /// the caller falls back to the member-name segment.
    ///
    /// `terminal` is true when the child itself is the value: a list element names its
    /// position either way, while a map entry names just its key for the value itself and
    /// `<key>.member` when the walk is descending into it.
    fn begin_child(&mut self, terminal: bool) -> Option<String> {
        match self.collections.last_mut()? {
            Frame::List { index } => {
                *index += 1;
                Some(path::element(*index))
            }
            Frame::Map { pending_key, .. } => {
                let key = pending_key.clone().unwrap_or_default();
                Some(if terminal { key } else { path::entry(&key) })
            }
            Frame::Struct => None,
        }
    }

    /// After a child completes inside a map frame, expect the next entry's key.
    fn end_child(&mut self) {
        if let Some(Frame::Map { expecting_key, .. }) = self.collections.last_mut() {
            *expecting_key = true;
        }
    }

    /// The walk stopped before the value now being written: the composite that value
    /// belongs to is missing a child, so it must render as nothing rather than as a value
    /// with a part of itself left out.
    fn abandoned(&mut self) {
        if let Some(open) = self.open.last_mut() {
            open.truncated = true;
        }
    }

    /// Record the leaf now being written, when it has a string rendering and fits the
    /// bounds this walk states.
    fn record(&mut self, value: Option<&str>) {
        let Some(value) = value else {
            return;
        };
        // A value no clause could carry anyway (see `MAX_RESTORED_ECHO`), or one that
        // does not fit what is left of the byte budget, is not copied at all. A later,
        // smaller leaf can still be recorded, so this is not terminal — but the collection
        // now has a hole in it, which `skipped` records.
        let leaf_path = path::join(&self.path);
        let bytes = leaf_path.len() + value.len();
        if value.len() > self.value_bound() || self.recorded + bytes > MAX_RECORDED_BYTES {
            self.skipped = true;
            return;
        }
        self.recorded += bytes;
        self.leaves.push((leaf_path, value.to_string()));
    }

    /// Record the position now being written with how a clause renders the value it held,
    /// and hand that rendering to the composite this value is a child of, if any.
    ///
    /// Called with the path still naming this value, and AFTER a composite's own frame is
    /// closed, so the child lands on its PARENT.
    fn produced(&mut self, schema: &Schema, rendering: Option<String>) {
        self.produced_as(schema, rendering, Clause::Own);
    }

    /// [`Self::produced`], with `clause` saying whether the rendering is also the one a
    /// clause ABOUT this position may quote: [`Clause::ParentOnly`] hands it to the composite
    /// this value is a child of and records nothing for the position itself, which is what a
    /// BLOB needs — its enclosing structure prints it as the buffer it was read into, while a
    /// clause about the blob member itself states no value at all.
    fn produced_as(&mut self, schema: &Schema, rendering: Option<String>, clause: Clause) {
        if !self.renders {
            return;
        }
        let position = path::join(&self.path);
        // A rendering no clause could carry (see `MAX_RESTORED_ECHO`) is neither kept nor
        // handed to the composite this value is a child of: holding it would retain
        // request-sized text once per nesting level. The composite then renders as
        // nothing, rather than as a value missing a part of itself.
        let rendering = rendering.filter(|rendered| rendered.len() <= self.echo_bound());
        let bytes = position.len() + rendering.as_ref().map_or(0, String::len);
        if clause == Clause::Own
            && self.renderings.len() < self.recorded_renderings_bound()
            && self.rendered + bytes <= MAX_RECORDED_RENDERED_BYTES.max(self.echo_bound())
        {
            self.rendered += bytes;
            self.renderings.push((position, rendering.clone()));
        }
        // Computed before the composite is borrowed: a map entry's key lives on the frame
        // stack, not on the child. Read only by the form that renders a map, as the
        // declaring shape is read only by the forms that name one.
        let key = match (self.maps, self.collections.last()) {
            // The entry whose VALUE is being written: the key was captured when the codec
            // wrote it, just before this value, and is still pending.
            (
                MapForm::Entries,
                Some(Frame::Map {
                    expecting_key: false,
                    pending_key,
                }),
            ) => pending_key.clone(),
            _ => None,
        };
        let ceiling = self.render_bound();
        let children = self.child_bound();
        if let Some(open) = self.open.last_mut() {
            open.take(
                RenderedChild {
                    name: Self::segment_for(schema),
                    // Read only by the forms that need it, so a form that states no
                    // shape name pays no allocation for one.
                    declared_by: match self.structures {
                        StructureForm::EveryMemberDeclared | StructureForm::MembersPrinted(_) => {
                            declaring_shape(schema)
                        }
                        StructureForm::MembersSet => None,
                    },
                    key,
                    rendering,
                },
                children,
                ceiling,
            );
        }
    }

    /// One leaf write. `value` carries the leaf's text when its echo is the caller's own
    /// bytes (a string, or a big-number rendering) and is `None` for every other leaf
    /// kind — which is still walked, so a later leaf's path stays correct. `rendering` is
    /// how a clause states that value, which a number or a boolean has even though its
    /// text is not the caller's own bytes, and a map or a document has not at all.
    fn leaf(&mut self, schema: &Schema, value: Option<&str>, rendering: Option<&str>) {
        self.leaf_rendered(schema, value, rendering, Clause::Own);
    }

    /// One leaf write whose rendering is for the composite this value is a CHILD of and not
    /// for a clause about the value itself ([`Clause::ParentOnly`]) — a blob, whose enclosing
    /// structure prints it as the buffer it was read into while its own clause states no
    /// value.
    fn leaf_rendered_for_parent(
        &mut self,
        schema: &Schema,
        value: Option<&str>,
        rendering: Option<&str>,
    ) {
        self.leaf_rendered(schema, value, rendering, Clause::ParentOnly);
    }

    /// The body both leaf writes share: `clause` says whether the rendering is also the one a
    /// clause ABOUT this position may quote.
    fn leaf_rendered(
        &mut self,
        schema: &Schema,
        value: Option<&str>,
        rendering: Option<&str>,
        clause: Clause,
    ) {
        if self.done() {
            self.abandoned();
            return;
        }
        // A map ENTRY KEY is written as a string against the map member's schema before
        // the entry's value. It is the value's path segment, not a leaf of its own.
        if let Some(Frame::Map {
            expecting_key,
            pending_key,
        }) = self.collections.last_mut()
        {
            if *expecting_key {
                *pending_key = value.map(str::to_string);
                *expecting_key = false;
                return;
            }
        }
        let rendering = rendering.map(str::to_string);
        // Every leaf POSITION counts against the walk's budget, whether or not its value
        // is kept, so the traversal is bounded by the cap even when nothing is recorded.
        self.visited += 1;
        match self.begin_child(true) {
            Some(segment) => {
                self.path.push(segment);
                self.record(value);
                self.produced_as(schema, rendering, clause);
                self.path.pop();
                self.end_child();
            }
            None => self.with_segment(schema, |this| {
                this.record(value);
                this.produced_as(schema, rendering, clause);
            }),
        }
    }

    /// Walk a collection (list or map): a collection that is itself a collection element
    /// shares the enclosing member's schema, whose name the enclosing level already
    /// pushed, so it contributes no segment of its own.
    fn collection(
        &mut self,
        schema: &Schema,
        frame: Frame,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.done() {
            self.abandoned();
            return Ok(());
        }
        let element = self.begin_child(false);
        let is_element = element.is_some();
        if let Some(segment) = element {
            self.path.push(segment);
        }
        let out = if is_element {
            self.walk_children(schema, frame, write_children)
        } else {
            self.with_segment(schema, |this| this.walk_children(schema, frame, write_children))
        };
        if is_element {
            self.path.pop();
            self.end_child();
        }
        out
    }

    /// Open `frame`, write the composite's children into it, then close it and record how
    /// the composite itself renders.
    fn walk_children(
        &mut self,
        schema: &Schema,
        frame: Frame,
        write_children: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        // The one place this walk recurses, so the one place its DEPTH is bounded: the
        // leaf budget counts leaf positions only, which a value made of composites never
        // spends. Past the ceiling nothing is descended into, and the composite the walk
        // stopped inside renders as nothing rather than as a value with a part left out.
        if self.collections.len() >= MAX_RECORDED_DEPTH {
            self.skipped = true;
            self.abandoned();
            return Ok(());
        }
        self.collections.push(frame);
        if self.renders {
            self.open.push(Composite::default());
        }
        let out = write_children(self);
        let rendering = self.close_children(schema);
        self.produced(schema, rendering);
        out
    }

    /// Close the innermost composite and render it from the children it collected.
    fn close_children(&mut self, schema: &Schema) -> Option<String> {
        let frame = self.collections.pop();
        if !self.renders {
            return None;
        }
        let composite = self.open.pop().unwrap_or_default();
        if composite.truncated {
            return None;
        }
        let rendering = match frame {
            Some(Frame::List { .. }) => rendered_sequence(&composite.children, self.render_bound()),
            Some(Frame::Struct) => self.rendered_struct(schema, &composite.children),
            // A map renders only for a caller that states the form ([`MapForm`]), so a
            // service whose recordings show the valueless clause keeps it rather than
            // gaining a form it never emits.
            Some(Frame::Map { .. }) => match self.maps {
                MapForm::Entries => rendered_entries(&composite.children, self.render_bound()),
                MapForm::Unrendered => None,
            },
            None => None,
        };
        rendering.filter(|rendered| rendered.len() <= self.render_bound())
    }

    /// One closed STRUCT frame, rendered in the form the caller stated
    /// ([`StructureForm`]).
    ///
    /// A structure renders under its SHAPE's name, which only the shape's own schema
    /// carries: a list element is written against the element struct's own schema
    /// (`&Inner::SCHEMA`, from the generated `serialize_members`), while a struct reached
    /// as a MEMBER carries the member schema, whose name is the member's. Rendering that
    /// one would state `<member>(…)`, a form AWS never emits — so under
    /// [`StructureForm::MembersSet`] such a structure renders as nothing, and under the two
    /// forms that name every member it is named by the shape its own members declare
    /// ([`declaring_shape`]), which is the one fact about it the member schema does not
    /// carry. The members that shape DECLARES are not reachable that way, so a structure
    /// reached as a member renders over the members it wrote — unless the caller STATED
    /// that shape's printed members ([`StructureForm::MembersPrinted`]), which is a fact
    /// about the service's wording rather than about the schema and so is available at
    /// either position.
    fn rendered_struct(&self, schema: &Schema, children: &[RenderedChild]) -> Option<String> {
        let printed = |name: &str| match self.structures {
            StructureForm::MembersPrinted(printed) => printed_members(printed, name),
            _ => None,
        };
        match schema.member_name() {
            // Written against the structure's OWN schema: that schema names the shape and
            // carries the members it declares.
            None => {
                let name = schema.shape_id().shape_name();
                match self.structures {
                    StructureForm::MembersSet => {
                        rendered_structure(name, children, self.render_bound())
                    }
                    _ => rendered_declared_structure(
                        name,
                        &printed(name).unwrap_or_else(|| declared_members(schema)),
                        children,
                        self.render_bound(),
                    ),
                }
            }
            // Reached as a struct MEMBER, so the shape is recovered from the members the
            // structure wrote and its declared members are out of reach.
            Some(_) => match self.structures {
                StructureForm::MembersSet => None,
                _ => {
                    let name = children.iter().find_map(|child| child.declared_by.as_deref())?;
                    match printed(name) {
                        Some(members) => rendered_declared_structure(
                            name,
                            &members,
                            children,
                            self.render_bound(),
                        ),
                        None => rendered_structure(name, children, self.render_bound()),
                    }
                }
            },
        }
    }
}

impl ShapeSerializer for SubmittedValues {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.done() {
            self.abandoned();
            return Ok(());
        }
        // A struct is the root input (no member name, no segment), a nested struct member
        // (member name -> segment), a list element or a map value (the frame supplies the
        // segment).
        let write_members = |s: &mut dyn ShapeSerializer| value.serialize_members(s);
        match self.begin_child(false) {
            Some(segment) => {
                self.path.push(segment);
                let out = self.walk_children(schema, Frame::Struct, &write_members);
                self.path.pop();
                self.end_child();
                out
            }
            None => self.with_segment(schema, |this| {
                this.walk_children(schema, Frame::Struct, &write_members)
            }),
        }
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collection(schema, Frame::List { index: 0 }, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.collection(
            schema,
            Frame::Map {
                expecting_key: true,
                pending_key: None,
            },
            write_entries,
        )
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        // A number or a boolean is not the caller's own BYTES (so no leaf is recorded for
        // re-echoing one), but a clause does state it, in the same text the validating
        // walk renders it with.
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()), Some(value.as_ref()));
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value.as_ref()), Some(value.as_ref()));
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.leaf(schema, Some(value), Some(value));
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        // The walk renders a blob as its byte count, never its bytes, so there is no
        // caller-submitted text to re-echo here. The clause rendering is the BUFFER the
        // value was read into ([`rendered_blob`]), which states its length and nothing of
        // its content — and it is the ENCLOSING structure's to print: a clause about the
        // blob member itself states no value at all, which is what `ParentOnly` keeps.
        self.leaf_rendered_for_parent(schema, None, Some(&rendered_blob(value.len())));
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.leaf(schema, None, Some(&value.to_string()));
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        // The walk renders a document as `<document>`, which is a summary rather than the
        // value, so this position has no text rendering.
        self.leaf(schema, None, None);
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // Deliberately nothing, bug for bug with the walk this one mirrors: the
        // validating serializer's `write_null` also returns without touching the path or
        // the collection frames. A null therefore has the SAME effect on both walks'
        // bookkeeping (including leaving a map frame mid-entry), which is what keeps a
        // leaf's path here identical to the path the violation was reported at. Handling
        // it "better" here than there would spell some paths differently from the
        // violations this module has to match. Either way the outcome is fail-safe: a
        // path that does not match yields no leaf, so the framework's own text stands.
        // Generated `serialize_members` skips an absent member rather than writing null,
        // so this is reached only by a codec that writes one explicitly.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        echo_of, is_elision_of, list_of_element, named_byte_count, quoted_echo, quoted_echo_up_to,
        echo_held_to, rendered_blob, rendered_declared_structure, rendered_entries, rendered_map,
        rendered_map_in_key_order, rendered_record, rendered_sequence, rendered_structure,
        unanchored, unanchored_grouping_alternation,
        unanchored_keeping_inner_anchors, unanchored_unless_published_as_declared, Composite,
        RenderedChild, Submitted, SubmittedEcho,
        SubmittedValues, MAX_RECORDED_BYTES, MAX_RECORDED_DEPTH, MAX_RECORDED_LEAVES,
        MAX_RECORDED_RENDERED_BYTES, MAX_RECORDED_RENDERINGS, MAX_RENDERED_CHILDREN,
        MAX_RESTORED_ECHO, MAX_RESTORED_TOTAL, UNSET_MEMBER,
    };
    use super::whole_value_expression;
    use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
    use std::collections::HashMap;

    /// RULE: a `@pattern` is quoted as the model's expression minus the leading `^` and the
    /// trailing `$` the model writes around the service's own full-match expression — the
    /// pair dropped positionally, and only when both ends carry one.
    #[test]
    fn a_pattern_loses_the_anchor_pair_the_model_wraps_it_in() {
        // (case, the modelled expression, the expression a clause quotes)
        let rows: [(&str, &str, &str); 11] = [
            ("a wrapped character class", r"^[a-z]+$", r"[a-z]+"),
            ("a wrapped group", r"^([\p{L}\p{N}_.:/=+\-@]*)$", r"([\p{L}\p{N}_.:/=+\-@]*)"),
            // The recorded case: the pair is dropped even though the expression
            // alternates at the top level (`NetworkConnectorName`, lambdacore).
            (
                "a wrapped top-level alternation",
                r"^(arn:aws:lambda:[a-z-]+-\d:\d{12}:network-connector:[a-z0-9-]+)|[a-zA-Z0-9_-]{1,64}$",
                r"(arn:aws:lambda:[a-z-]+-\d:\d{12}:network-connector:[a-z0-9-]+)|[a-zA-Z0-9_-]{1,64}",
            ),
            ("anchored at the start only", r"^arn:aws.*", r"^arn:aws.*"),
            ("anchored at the end only", r"[0-9]{12}$", r"[0-9]{12}$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            // Anchored inside its branches rather than at its ends: no outer pair.
            ("anchors inside the branches", r"(^a$)|(^b$)", r"(^a$)|(^b$)"),
            // The same two branches WITHOUT the parentheses carry the pair at the ends,
            // so the pair is what is dropped — the rule's faithful output even though the
            // result describes another language. See this function's doc comment.
            ("branches sharing the outer pair", r"^a$|^b$", r"a$|^b"),
            ("one branch anchored at each end", r"^a|b$", r"a|b"),
            // An escaped trailing `$` is a literal dollar, not the end anchor.
            ("an escaped trailing dollar", r"^costs\$", r"^costs\$"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            ("an escaped backslash then the anchor", r"^[a-z]\\$", r"[a-z]\\"),
        ];
        for (case, modelled, expected) in rows {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {modelled:?} to be quoted as {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE: the collection is COMPLETE only while every string-rendered leaf was
    /// recorded; any leaf one of the stated bounds drops marks it incomplete, which is what
    /// stops [`super::submitted_lists`] from offering a collection with a hole in it.
    #[test]
    fn a_leaf_a_bound_dropped_marks_the_collection_incomplete() {
        // (case, the leaf values written in order, whether the walk stayed complete)
        let rows: [(&str, Vec<String>, bool); 5] = [
            ("nothing written at all", Vec::new(), true),
            (
                "two small leaves",
                vec!["first".to_string(), "second".to_string()],
                true,
            ),
            (
                "a leaf at the per-value ceiling",
                vec!["x".repeat(MAX_RESTORED_ECHO)],
                true,
            ),
            (
                "a leaf one byte past the per-value ceiling",
                vec!["x".repeat(MAX_RESTORED_ECHO + 1)],
                false,
            ),
            (
                "leaves that exhaust the byte budget",
                (0..=MAX_RECORDED_BYTES / MAX_RESTORED_ECHO)
                    .map(|_| "y".repeat(MAX_RESTORED_ECHO))
                    .collect(),
                false,
            ),
        ];

        for (case, values, expected) in rows {
            let mut walk = SubmittedValues::default();
            walk.path.push("member".to_string());
            for value in &values {
                walk.record(Some(value));
            }
            assert_eq!(
                !walk.skipped, expected,
                "case {case:?}: expected complete == {expected}, got {} (recorded {} leaves)",
                !walk.skipped,
                walk.leaves.len()
            );
        }
    }

    /// RULE: reaching the leaf cap also marks the collection incomplete — the walk stops
    /// there, so a leaf beyond it is a leaf that was dropped — and the cap is counted over
    /// every leaf VISITED, so a request whose values are all dropped for their size still
    /// stops the walk.
    #[test]
    fn the_leaf_cap_marks_the_collection_incomplete() {
        let mut walk = SubmittedValues::default();
        walk.path.push("member".to_string());
        assert!(
            !walk.done() && !walk.skipped,
            "case an empty walk: expected it to be complete and not done"
        );
        for index in 0..MAX_RECORDED_LEAVES {
            walk.visited += 1;
            walk.record(Some(&index.to_string()));
        }
        assert_eq!(
            walk.leaves.len(),
            MAX_RECORDED_LEAVES,
            "case a full walk: expected the cap to be reached, got {} leaves",
            walk.leaves.len()
        );
        assert!(
            walk.done() && walk.skipped,
            "case a full walk: expected it to be done and incomplete"
        );

        // Nothing is recorded for a value past the echo bound, so a walk whose stop
        // condition counted RECORDED leaves would never stop on a request made of them.
        let mut dropping = SubmittedValues::default();
        dropping.path.push("member".to_string());
        let oversized = "x".repeat(MAX_RESTORED_ECHO + 1);
        for _ in 0..MAX_RECORDED_LEAVES {
            dropping.visited += 1;
            dropping.record(Some(&oversized));
        }
        assert!(
            dropping.leaves.is_empty(),
            "case a walk of oversized values: expected nothing recorded, got {} leaves",
            dropping.leaves.len()
        );
        assert!(
            dropping.done() && dropping.skipped,
            "case a walk of oversized values: expected it to be done and incomplete"
        );
    }

    /// RULE: the traversal's DEPTH is bounded by a stated constant just as its leaf count
    /// is — a value nested past the ceiling is not descended into and marks the collection
    /// incomplete — so a value built only of composites, which spends no leaf budget at
    /// all, cannot set how deep the walk goes.
    #[test]
    fn the_depth_ceiling_bounds_a_value_built_only_of_composites() {
        use aws_smithy_schema::serde::{SerdeError, ShapeSerializer};
        use aws_smithy_schema::{Schema, ShapeId, ShapeType};

        static ELEMENT: Schema = Schema::new_member(
            ShapeId::from_static("test#Nested$member", "test", "member"),
            ShapeType::String,
            "member",
            0,
        );
        static LIST: Schema = Schema::new_member(
            ShapeId::from_static("test#Nested$values", "test", "values"),
            ShapeType::List,
            "values",
            0,
        );

        /// `depth` lists nested one inside the other, holding one string at the bottom.
        fn nest(
            serializer: &mut dyn ShapeSerializer,
            depth: usize,
        ) -> Result<(), SerdeError> {
            if depth == 0 {
                return serializer.write_string(&ELEMENT, "innermost");
            }
            let deeper = move |inner: &mut dyn ShapeSerializer| nest(inner, depth - 1);
            serializer.write_list(&LIST, &deeper)
        }

        // (case, how deep the value nests, whether the walk reaches the bottom of it)
        let rows: [(&str, usize, bool); 5] = [
            ("one list", 1, true),
            ("a handful of lists", 8, true),
            ("nested exactly to the ceiling", MAX_RECORDED_DEPTH, true),
            ("one list past the ceiling", MAX_RECORDED_DEPTH + 1, false),
            ("nested far past the ceiling", 16 * MAX_RECORDED_DEPTH, false),
        ];
        for (case, depth, reached) in rows {
            let mut walk = SubmittedValues::default();
            nest(&mut walk, depth).unwrap_or_else(|e| panic!("case {case:?}: the walk failed: {e}"));
            assert!(
                walk.collections.is_empty(),
                "case {case:?}: expected every frame closed, got {} open",
                walk.collections.len()
            );
            assert_eq!(
                !walk.leaves.is_empty(),
                reached,
                "case {case:?}: expected the innermost value recorded == {reached}, got {} leaves",
                walk.leaves.len()
            );
            assert_eq!(
                walk.skipped, !reached,
                "case {case:?}: expected incomplete == {}, got {}",
                !reached, walk.skipped
            );
        }
    }

    /// RULE: a clause quotes the value itself up to the ceiling one clause may carry, and
    /// past that a character-boundary-safe prefix plus the marker naming the whole value's
    /// byte count — so the quoting is bounded by a constant rather than by the request, and
    /// what it renders is an elision of the value the walk itself would recognise.
    #[test]
    fn a_quoted_echo_is_the_value_itself_up_to_one_ceiling() {
        // One row: the case, the value, and whether the ceiling leaves it whole.
        let rows: [(&str, String, bool); 5] = [
            ("a short value", "bob".to_string(), true),
            (
                "a value at the ceiling",
                "x".repeat(MAX_RESTORED_ECHO),
                true,
            ),
            (
                "a value one byte over the ceiling",
                "x".repeat(MAX_RESTORED_ECHO + 1),
                false,
            ),
            (
                "a value far over the ceiling",
                "x".repeat(64 * MAX_RESTORED_ECHO),
                false,
            ),
            (
                // Two bytes per character: the prefix must not split one.
                "a multi-byte value over the ceiling",
                "é".repeat(MAX_RESTORED_ECHO),
                false,
            ),
        ];
        for (case, value, whole) in rows {
            let quoted = quoted_echo(&value);
            if whole {
                assert_eq!(
                    quoted, value,
                    "case {case:?}: expected the value itself, got {} bytes",
                    quoted.len()
                );
            } else {
                assert!(
                    quoted.len() <= MAX_RESTORED_ECHO + 32,
                    "case {case:?}: expected a clause bounded by the ceiling, got {} bytes",
                    quoted.len()
                );
                assert!(
                    quoted.ends_with(&format!("... [{} bytes]", value.len())),
                    "case {case:?}: expected the marker naming {} bytes, got the tail {:?}",
                    value.len(),
                    &quoted[quoted.len().saturating_sub(32)..]
                );
                assert!(
                    is_elision_of(&quoted, &value),
                    "case {case:?}: the walk must recognise the rendering as this value's \
                     elision"
                );
            }
        }
    }

    /// RULE: the same rendering holds at a ceiling the SERVICE states, in either direction:
    /// a value within it is quoted whole and one past it keeps a character-boundary-safe
    /// prefix plus the marker naming the whole value's byte count.
    #[test]
    fn a_stated_ceiling_quotes_the_value_itself_up_to_that_ceiling() {
        // Rows differ along the ceiling (far below the default, the default itself, and far
        // above it), the value's size relative to it, and its encoding.
        let rows: [(&str, usize, String, bool); 6] = [
            ("a short value under a small ceiling", 768, "bob".to_string(), true),
            ("a value at a small ceiling", 768, "x".repeat(768), true),
            (
                "a value one byte over a small ceiling",
                768,
                "x".repeat(769),
                false,
            ),
            (
                "a value the default would have quoted whole",
                768,
                "x".repeat(MAX_RESTORED_ECHO),
                false,
            ),
            (
                // Three bytes per character, and 768 is a multiple of three only by
                // accident of the repeat count: the prefix must not split a character.
                "a multi-byte value over a small ceiling",
                1000,
                "€".repeat(500),
                false,
            ),
            (
                "a value under a ceiling above the default",
                64 * 1024,
                "x".repeat(MAX_RESTORED_ECHO + 1),
                true,
            ),
        ];
        for (case, ceiling, value, whole) in rows {
            let quoted = quoted_echo_up_to(&value, ceiling);
            if whole {
                assert_eq!(
                    quoted, value,
                    "case {case:?}: expected the value itself, got {} bytes",
                    quoted.len()
                );
                continue;
            }
            let marker = format!("... [{} bytes]", value.len());
            let Some(prefix) = quoted.strip_suffix(marker.as_str()) else {
                panic!(
                    "case {case:?}: expected the marker naming {} bytes, got {} quoted bytes",
                    value.len(),
                    quoted.len()
                );
            };
            assert!(
                value.starts_with(prefix) && prefix.len() <= ceiling,
                "case {case:?}: expected a prefix of the value no longer than {ceiling} \
                 bytes, got {} bytes",
                prefix.len()
            );
            assert!(
                is_elision_of(&quoted, &value),
                "case {case:?}: the walk must recognise the rendering as this value's elision"
            );
        }
    }

    /// RULE: an echo held to a smaller bound keeps the byte count it names — the submitted
    /// value's size, not the rendering's — and shortens only the prefix, so a clause held
    /// twice still reports how much was sent and still quotes an elision of the same value.
    #[test]
    fn an_echo_held_again_keeps_the_count_it_names() {
        let submitted = "v".repeat(5000);
        let once = quoted_echo_up_to(&submitted, 768).into_owned();
        // Rows differ along what the echo IS (the value itself, an elision of a longer one),
        // the bound it is held to, and the encoding.
        let rows: [(&str, String, usize); 5] = [
            ("the value itself, within the bound", "bob".to_string(), 64),
            ("the value itself, over the bound", "v".repeat(700), 200),
            ("an elision, within the bound", once.clone(), 2048),
            ("an elision, over the bound", once.clone(), 200),
            (
                "an elision of multibyte characters, over the bound",
                quoted_echo_up_to(&"€".repeat(2000), 768).into_owned(),
                200,
            ),
        ];
        for (case, echo, bound) in rows {
            let held = echo_held_to(&echo, bound);
            assert!(
                held.len() <= bound,
                "case {case:?}: expected at most {bound} bytes, got {}",
                held.len()
            );
            if echo.len() <= bound {
                assert_eq!(
                    held, echo,
                    "case {case:?}: expected the echo unchanged, got {} bytes",
                    held.len()
                );
                continue;
            }
            let named = named_byte_count(&echo)
                .map_or(echo.len(), |at| echo[at + "... [".len()..].trim_end_matches(" bytes]").parse().expect("a byte count"));
            assert!(
                held.ends_with(&format!("... [{named} bytes]")),
                "case {case:?}: expected the count {named} kept, got the tail {:?}",
                &held[held.len().saturating_sub(32)..]
            );
            assert!(
                echo.starts_with(held.split("... [").next().unwrap_or_default()),
                "case {case:?}: expected the kept prefix to be a prefix of the echo"
            );
        }

        // Held to the submitted value's own elision bound, the count still names the value
        // the caller sent rather than the text the first rendering produced.
        let twice = echo_held_to(&once, 200);
        assert!(
            twice.ends_with(&format!("... [{} bytes]", submitted.len())),
            "case \"a doubly held echo\": expected the submitted {} bytes named, got {twice:?}",
            submitted.len()
        );
        assert!(
            is_elision_of(&twice, &submitted),
            "case \"a doubly held echo\": the walk must still recognise it as this value's \
             elision"
        );
    }

    /// RULE: an echo elided with a marker of the service's own keeps at most the bound in
    /// bytes, ends on a character boundary, and ends with that marker — and a value inside
    /// the bound is borrowed back unchanged, whatever the marker.
    #[test]
    fn an_echo_elided_with_a_marker_is_bounded_and_character_safe() {
        const MARKER: &str = " ...";
        // Rows differ along the value's length relative to the bound, its encoding, and the
        // bound itself.
        let rows: [(&str, String, usize); 5] = [
            ("a value well inside the bound", "bob".to_string(), 64),
            ("a value exactly at the bound", "v".repeat(64), 64),
            ("a value one byte past the bound", "v".repeat(65), 64),
            ("a value many times the bound", "v".repeat(5000), 64),
            (
                "a multibyte value whose boundary is inside the bound",
                "€".repeat(100),
                64,
            ),
        ];
        for (case, value, bound) in rows {
            let shown = super::echo_elided_with(&value, bound, MARKER);
            if value.len() <= bound {
                assert_eq!(
                    shown, value,
                    "case {case:?}: expected the value unchanged, got {shown:?}"
                );
                continue;
            }
            let prefix = shown
                .strip_suffix(MARKER)
                .unwrap_or_else(|| panic!("case {case:?}: expected the marker, got {shown:?}"));
            assert!(
                prefix.len() <= bound && value.starts_with(prefix),
                "case {case:?}: expected a bounded prefix of the value, got {prefix:?}"
            );
            assert!(
                value.is_char_boundary(prefix.len()),
                "case {case:?}: expected the prefix to end on a character boundary, got {} \
                 bytes",
                prefix.len()
            );
        }
    }

    /// RULE: a stored echo is the submitted value's elision when it is that value, or a
    /// proper prefix of it followed by the marker naming the value's byte count.
    #[test]
    fn an_elision_is_recognised_by_its_shape_not_by_a_cap() {
        let long = "t".repeat(300);
        let prefix: String = long.chars().take(256).collect();
        let rows: [(&str, String, bool); 6] = [
            ("the value itself", long.clone(), true),
            (
                "prefix plus the marker",
                format!("{prefix}... [300 bytes]"),
                true,
            ),
            (
                "prefix plus the marker, a shorter cap",
                format!("{}... [300 bytes]", "t".repeat(16)),
                true,
            ),
            (
                "the marker naming another byte count",
                format!("{prefix}... [301 bytes]"),
                false,
            ),
            (
                "a prefix of another value",
                format!("{}... [300 bytes]", "x".repeat(16)),
                false,
            ),
            ("an unrelated rendering", "300 items".to_string(), false),
        ];
        for (case, stored, expected) in rows {
            let actual = is_elision_of(&stored, &long);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected is_elision_of == {expected}, got {actual}"
            );
        }
    }

    /// RULE: a leaf is a scalar list element exactly when its path ends in a 1-based
    /// position followed by the collection step, and the list it belongs to is that path
    /// without those two segments.
    #[test]
    fn a_list_element_path_names_its_list() {
        let rows: [(&str, &str, Option<&str>); 7] = [
            ("a top-level list element", "tags.1.member", Some("tags")),
            (
                "a nested list element",
                "config.rules.12.member",
                Some("config.rules"),
            ),
            (
                "an element of a list inside a list",
                "outer.2.member.inner.3.member",
                Some("outer.2.member.inner"),
            ),
            ("a struct member", "trajectoryId", None),
            ("a nested struct member", "tags.1.member.value", None),
            ("a map entry value", "attributes.someKey", None),
            ("a map key spelled like the step", "attributes.member", None),
        ];
        for (case, leaf_path, expected) in rows {
            let actual = list_of_element(leaf_path);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the single index pass answers a clause's path with the leaf recorded there,
    /// and a `@uniqueItems` clause — reported at the LIST's path — with the first element
    /// value that repeated in walk order.
    #[test]
    fn the_index_answers_each_violation_class() {
        let leaves: Vec<(String, String)> = [
            ("trajectoryId", "the-id"),
            ("tags.1.member", "a"),
            ("tags.2.member", "b"),
            ("tags.3.member", "a"),
            ("attributes.someKey", "entry-value"),
        ]
        .into_iter()
        .map(|(leaf_path, value)| (leaf_path.to_string(), value.to_string()))
        .collect();
        let index = Submitted::of(&leaves);

        let rows: [(&str, &str, ViolationKind, Option<&str>); 5] = [
            (
                "a member clause",
                "trajectoryId",
                ViolationKind::MaxLength(256),
                Some("the-id"),
            ),
            (
                "a list-element clause",
                "tags.2.member",
                ViolationKind::MaxLength(1),
                Some("b"),
            ),
            (
                "a map-entry clause",
                "attributes.someKey",
                ViolationKind::Pattern("^x$".to_string()),
                Some("entry-value"),
            ),
            (
                "a uniqueItems clause at the list",
                "tags",
                ViolationKind::UniqueItems,
                Some("a"),
            ),
            (
                "a clause on a member no leaf was recorded for",
                "clientToken",
                ViolationKind::MinLength(1),
                None,
            ),
        ];
        for (case, leaf_path, kind, expected) in rows {
            let violation = Violation {
                path: leaf_path.to_string(),
                kind,
                value: Some("whatever".to_string()),
                sensitive: false,
            };
            let actual = index.candidate(&violation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a clause on a LIST — whose stored echo is the framework's `<n> items`
    /// summary — is answered with the list rendered from exactly the `n` elements
    /// recorded under it, and with nothing when the two do not agree.
    #[test]
    fn a_collection_clause_is_answered_with_the_list_as_submitted() {
        let leaves: Vec<(String, String)> = [
            ("resourceIds.1.member", "table/a"),
            ("resourceIds.2.member", "table/b"),
            ("resourceIds.3.member", "table/c"),
            ("policyNames.1.member", "only"),
            ("serviceNamespace", "dynamodb"),
        ]
        .into_iter()
        .map(|(leaf_path, value)| (leaf_path.to_string(), value.to_string()))
        .collect();
        let index = Submitted::of(&leaves);

        // (case, the clause's path, the echo the framework stored, the restored echo)
        let rows: [(&str, &str, &str, Option<&str>); 6] = [
            (
                "a list of three",
                "resourceIds",
                "3 items",
                Some("[table/a, table/b, table/c]"),
            ),
            (
                "a list of one, which the framework still renders in the plural",
                "policyNames",
                "1 items",
                Some("[only]"),
            ),
            (
                "a summary counting more elements than were recorded",
                "resourceIds",
                "51 items",
                None,
            ),
            ("a list no element was recorded for", "tagKeys", "2 items", None),
            (
                "a clause on the list whose echo is not a collection summary",
                "resourceIds",
                "table/a",
                None,
            ),
            (
                "a scalar member, whose echo is never a collection summary",
                "serviceNamespace",
                "dynamodb",
                None,
            ),
        ];
        for (case, path, stored, expected) in rows {
            let violation = Violation {
                path: path.to_string(),
                kind: ViolationKind::MaxLength(50),
                value: Some(stored.to_string()),
                sensitive: false,
            };
            let actual = index.collection_candidate(&violation, stored);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the stated bounds are finite and ordered — no single echo may exceed the
    /// per-message budget, and nothing is collected that could never be restored.
    #[test]
    fn the_stated_bounds_are_coherent() {
        const COHERENT: () = {
            assert!(MAX_RESTORED_ECHO <= MAX_RESTORED_TOTAL);
            assert!(MAX_RECORDED_BYTES <= MAX_RESTORED_TOTAL);
            assert!(MAX_RECORDED_LEAVES > 0 && MAX_RECORDED_BYTES > 0);
            assert!(MAX_RECORDED_RENDERINGS > 0 && MAX_RECORDED_RENDERED_BYTES > 0);
            assert!(MAX_RENDERED_CHILDREN > 0);
        };
        let () = COHERENT;
    }

    /// RULE: a composite renders from its children — a list as `[<element>, …]`, a
    /// structure as `<ShapeName>(<member>=<value>, …)` over the members that were
    /// present — and renders as nothing at all when a child has no text rendering.
    #[test]
    fn a_composite_renders_from_the_children_it_was_given() {
        let child = |name: Option<&str>, rendering: Option<&str>| RenderedChild {
            name: name.map(str::to_string),
            declared_by: name.map(|_| "Thing".to_string()),
            key: None,
            rendering: rendering.map(str::to_string),
        };
        let sequences: [(&str, Vec<RenderedChild>, Option<&str>); 4] = [
            ("no elements", vec![], Some("[]")),
            ("one element", vec![child(None, Some("a"))], Some("[a]")),
            (
                "three elements keep their order",
                vec![
                    child(None, Some("a")),
                    child(None, Some("b")),
                    child(None, Some("c")),
                ],
                Some("[a, b, c]"),
            ),
            (
                "an element with no text rendering",
                vec![child(None, Some("a")), child(None, None)],
                None,
            ),
        ];
        for (case, children, expected) in sequences {
            let actual = rendered_sequence(&children, MAX_RESTORED_ECHO);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        let structures: [(&str, Vec<RenderedChild>, Option<&str>); 5] = [
            ("no members present", vec![], Some("Thing()")),
            (
                "one member present",
                vec![child(Some("name"), Some("n"))],
                Some("Thing(name=n)"),
            ),
            (
                "two members in the order they were written",
                vec![child(Some("name"), Some("n")), child(Some("size"), Some("2"))],
                Some("Thing(name=n, size=2)"),
            ),
            (
                "a member with no text rendering suppresses the whole structure",
                vec![child(Some("name"), Some("n")), child(Some("body"), None)],
                None,
            ),
            (
                "a child with no member name at all",
                vec![child(None, Some("n"))],
                None,
            ),
        ];
        for (case, children, expected) in structures {
            let actual = rendered_structure("Thing", &children, MAX_RESTORED_ECHO);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a BLOB renders as the buffer the validating front end read it into — its length
    /// three ways and nothing of its bytes — so a structure carrying one, and the list that
    /// structure belongs to, render rather than stating no value at all.
    #[test]
    fn a_blob_renders_as_the_buffer_it_was_read_into() {
        // (case, the blob's length, the rendering a clause states it with). The rows differ
        // along the length: none, one byte, and a mebibyte.
        let rows: [(&str, usize, &str); 3] = [
            ("an empty blob", 0, "java.nio.HeapByteBuffer[pos=0 lim=0 cap=0]"),
            ("one byte", 1, "java.nio.HeapByteBuffer[pos=0 lim=1 cap=1]"),
            (
                "a mebibyte",
                1024 * 1024,
                "java.nio.HeapByteBuffer[pos=0 lim=1048576 cap=1048576]",
            ),
        ];
        for (case, bytes, expected) in rows {
            let actual = rendered_blob(bytes);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // A structure carrying a blob renders over every member its shape declares, the
        // unset ones as `null` — and so does the list of such structures.
        let entry = RenderedChild {
            name: Some("data".to_string()),
            declared_by: Some("Entry".to_string()),
            key: None,
            rendering: Some(rendered_blob(1)),
        };
        let declared = ["data".to_string(), "key".to_string()];
        let rendered = rendered_declared_structure("Entry", &declared, &[entry], MAX_RESTORED_ECHO);
        assert_eq!(
            rendered.as_deref(),
            Some("Entry(data=java.nio.HeapByteBuffer[pos=0 lim=1 cap=1], key=null)"),
            "case a structure carrying a blob: expected it rendered, got {rendered:?}"
        );
        let element = RenderedChild {
            name: None,
            declared_by: None,
            key: None,
            rendering: rendered,
        };
        let list = rendered_sequence(&[element], MAX_RESTORED_ECHO);
        assert_eq!(
            list.as_deref(),
            Some("[Entry(data=java.nio.HeapByteBuffer[pos=0 lim=1 cap=1], key=null)]"),
            "case a list of structures carrying blobs: expected it rendered, got {list:?}"
        );
    }

    /// RULE: the renderers a HANDLER states its own clause with produce the same forms the
    /// walk's own composites do — a structure as `<ShapeName>(<member>=<value>, …)` over
    /// the members given, a map as `{<key>=<value>, …}` over its entries in KEY order — and
    /// a map larger than a clause quotes renders as nothing, so one request cannot size the
    /// message it is refused with.
    #[test]
    fn the_handler_renderers_state_the_walk_s_own_forms() {
        /// One structure row: the members given and the rendering they state.
        struct Record {
            case: &'static str,
            members: Vec<(&'static str, &'static str)>,
            expected: &'static str,
        }

        // The rows differ along how many members there are and whether one of them is the
        // unset spelling.
        let records = [
            Record {
                case: "no members",
                members: vec![],
                expected: "Thing()",
            },
            Record {
                case: "one member",
                members: vec![("name", "n")],
                expected: "Thing(name=n)",
            },
            Record {
                case: "two members in the order given",
                members: vec![("name", "n"), ("size", "2")],
                expected: "Thing(name=n, size=2)",
            },
            Record {
                case: "a member the request left unset",
                members: vec![("name", UNSET_MEMBER), ("size", "2")],
                expected: "Thing(name=null, size=2)",
            },
        ];
        for Record {
            case,
            members,
            expected,
        } in records
        {
            let actual = rendered_record("Thing", &members);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        /// One map row: the entries given, in the order they arrive, and the rendering.
        struct Map {
            case: &'static str,
            entries: Vec<(&'static str, String)>,
            expected: Option<&'static str>,
        }

        // The rows differ along the entry count and the order the entries arrive in, which
        // the rendering is independent of, and the last is past what a clause quotes.
        let entry = |key: &'static str, value: &str| (key, value.to_string());
        let maps = [
            Map {
                case: "no entries",
                entries: vec![],
                expected: Some("{}"),
            },
            Map {
                case: "one entry",
                entries: vec![entry("b", "2")],
                expected: Some("{b=2}"),
            },
            Map {
                case: "two entries already in key order",
                entries: vec![entry("a", "1"), entry("b", "2")],
                expected: Some("{a=1, b=2}"),
            },
            Map {
                case: "the same two entries the other way round",
                entries: vec![entry("b", "2"), entry("a", "1")],
                expected: Some("{a=1, b=2}"),
            },
            Map {
                case: "a map larger than a clause quotes",
                entries: vec![entry("a", &"x".repeat(MAX_RESTORED_ECHO))],
                expected: None,
            },
        ];
        for Map {
            case,
            entries,
            expected,
        } in maps
        {
            let actual = rendered_map_in_key_order(&entries);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: in the DECLARED form a structure renders over every member its shape
    /// declares, in declaration order, with one the request left unset stated as `null`;
    /// a member that WAS written and has no text rendering still suppresses the whole
    /// structure.
    #[test]
    fn a_declared_structure_names_every_member_its_shape_declares() {
        let child = |name: &str, rendering: Option<&str>| RenderedChild {
            name: Some(name.to_string()),
            declared_by: Some("Thing".to_string()),
            key: None,
            rendering: rendering.map(str::to_string),
        };
        let declared = |names: &[&str]| -> Vec<String> {
            names.iter().map(|name| (*name).to_string()).collect()
        };

        // (case, the members the shape declares, the children the request wrote, the
        // rendering). The rows differ along which declared members were set, how many
        // were not, and where the unset ones sit in the declaration order.
        struct Row {
            case: &'static str,
            declared: Vec<String>,
            children: Vec<RenderedChild>,
            expected: Option<&'static str>,
        }

        let rows = [
            Row {
                case: "a shape declaring nothing",
                declared: declared(&[]),
                children: vec![],
                expected: Some("Thing()"),
            },
            Row {
                case: "every declared member set",
                declared: declared(&["name", "size"]),
                children: vec![child("name", Some("n")), child("size", Some("2"))],
                expected: Some("Thing(name=n, size=2)"),
            },
            Row {
                case: "a trailing member left unset",
                declared: declared(&["name", "size"]),
                children: vec![child("name", Some("n"))],
                expected: Some("Thing(name=n, size=null)"),
            },
            Row {
                case: "a leading member left unset",
                declared: declared(&["name", "size"]),
                children: vec![child("size", Some("2"))],
                expected: Some("Thing(name=null, size=2)"),
            },
            Row {
                case: "every declared member left unset",
                declared: declared(&["name", "size", "body"]),
                children: vec![],
                expected: Some("Thing(name=null, size=null, body=null)"),
            },
            Row {
                case: "a member written with no text rendering",
                declared: declared(&["name", "body"]),
                children: vec![child("name", Some("n")), child("body", None)],
                expected: None,
            },
        ];
        for row in rows {
            let actual = rendered_declared_structure(
                "Thing",
                &row.declared,
                &row.children,
                MAX_RESTORED_ECHO,
            );
            assert_eq!(
                actual.as_deref(),
                row.expected,
                "case {:?}: expected {:?}, got {actual:?}",
                row.case,
                row.expected
            );
        }
    }

    /// RULE: what a composite may hold is bounded by BYTES as well as by child count, and
    /// a rendering past [`MAX_RESTORED_ECHO`] is refused before it is built — so neither
    /// the text this walk retains nor the text it allocates is sized by the request.
    #[test]
    fn a_composite_is_bounded_by_the_bytes_its_children_hold() {
        let child = |rendering: &str| RenderedChild {
            name: Some("name".to_string()),
            declared_by: Some("Thing".to_string()),
            key: None,
            rendering: Some(rendering.to_string()),
        };

        // One child at the bound is held; the next one tips the composite over it, and a
        // truncated composite renders as nothing rather than as a part of the value.
        let mut composite = Composite::default();
        composite.take(
            child(&"a".repeat(MAX_RESTORED_ECHO - "name".len())),
            MAX_RENDERED_CHILDREN,
            MAX_RESTORED_ECHO,
        );
        assert!(
            !composite.truncated && composite.children.len() == 1,
            "case a child at the byte bound: expected it held, got {composite:?}"
        );
        composite.take(child("b"), MAX_RENDERED_CHILDREN, MAX_RESTORED_ECHO);
        assert!(
            composite.truncated && composite.children.len() == 1,
            "case a child past the byte bound: expected the composite truncated with the \
             child dropped, got {composite:?}"
        );

        // A child count past the cap is the other bound, unchanged.
        let mut counted = Composite::default();
        for _ in 0..=MAX_RENDERED_CHILDREN {
            counted.take(child("a"), MAX_RENDERED_CHILDREN, MAX_RESTORED_ECHO);
        }
        assert!(
            counted.truncated && counted.children.len() == MAX_RENDERED_CHILDREN,
            "case more children than the cap: expected the composite truncated at the cap, \
             got {} children",
            counted.children.len()
        );

        // A caller that states a LARGER child ceiling — a service whose model declares a
        // collection longer than the default count — gets those children held.
        let stated_children = 2 * MAX_RENDERED_CHILDREN;
        let mut stated = Composite::default();
        for _ in 0..stated_children {
            stated.take(child("a"), stated_children, MAX_RESTORED_ECHO * 64);
        }
        assert!(
            !stated.truncated && stated.children.len() == stated_children,
            "case a stated child ceiling: expected every child held, got {} of \
             {stated_children}",
            stated.children.len()
        );

        // And a rendering that would exceed the bound is refused by both renderers, which
        // measure it before joining anything.
        let oversized = vec![
            child(&"a".repeat(MAX_RESTORED_ECHO)),
            child(&"b".repeat(MAX_RESTORED_ECHO)),
        ];
        let sequence = rendered_sequence(&oversized, MAX_RESTORED_ECHO);
        assert_eq!(
            sequence, None,
            "case an oversized list: expected no rendering, got {sequence:?}"
        );
        let structure = rendered_structure("Thing", &oversized, MAX_RESTORED_ECHO);
        assert_eq!(
            structure, None,
            "case an oversized structure: expected no rendering, got {structure:?}"
        );
        let declared = rendered_declared_structure(
            "Thing",
            &["name".to_string()],
            &[child(&"a".repeat(MAX_RESTORED_ECHO))],
            MAX_RESTORED_ECHO,
        );
        assert_eq!(
            declared, None,
            "case an oversized declared structure: expected no rendering, got {declared:?}"
        );

        // A caller that states its OWN composite ceiling is held to THAT one: the same
        // children the module's own bound refuses render whole under a ceiling above them,
        // and the bound still bites one byte past whatever was stated.
        let stated = 4 * MAX_RESTORED_ECHO;
        let under_stated = rendered_sequence(&oversized, stated);
        assert!(
            under_stated.is_some_and(|rendered| rendered.len() > MAX_RESTORED_ECHO),
            "case a list past the default bound under a stated ceiling: expected the whole \
             list rendered"
        );
        let mut held = Composite::default();
        held.take(
            child(&"a".repeat(MAX_RESTORED_ECHO)),
            MAX_RENDERED_CHILDREN,
            stated,
        );
        held.take(
            child(&"b".repeat(MAX_RESTORED_ECHO)),
            MAX_RENDERED_CHILDREN,
            stated,
        );
        assert!(
            !held.truncated && held.children.len() == 2,
            "case two children inside a stated ceiling: expected both held, got {held:?}"
        );
        let tight = rendered_sequence(&oversized, 1);
        assert_eq!(
            tight, None,
            "case a stated ceiling below the rendering: expected no rendering, got {tight:?}"
        );
    }

    /// RULE: a clause's echo is `Absent` for a member the request never sent, `Rendered`
    /// for a position this walk rendered, and `Unrendered` for a value that was present
    /// with no text rendering, or past the stated bounds.
    #[test]
    fn an_echo_distinguishes_absent_from_present_but_unrenderable() {
        let too_long = "x".repeat(MAX_RESTORED_ECHO + 1);
        let renderings: Vec<(String, Option<String>)> = [
            ("name", Some("a-name")),
            ("body", None),
            ("tags", Some("[a, b]")),
        ]
        .into_iter()
        .map(|(position, rendering)| {
            (
                position.to_string(),
                rendering.map(|text: &str| text.to_string()),
            )
        })
        .chain(std::iter::once((
            "huge".to_string(),
            Some(too_long.clone()),
        )))
        .collect();
        let mut by_path: HashMap<&str, Option<&str>> = HashMap::new();
        for (position, rendering) in &renderings {
            by_path.insert(position.as_str(), rendering.as_deref());
        }

        let rows: [(&str, &str, ViolationKind, Option<&str>, SubmittedEcho); 6] = [
            (
                "an absent member",
                "name",
                ViolationKind::MissingRequiredMember,
                None,
                SubmittedEcho::Absent,
            ),
            (
                "a rendered string",
                "name",
                ViolationKind::MaxLength(1),
                Some("a-name"),
                SubmittedEcho::Rendered("a-name".to_string()),
            ),
            (
                "a list rendered from its elements",
                "tags",
                ViolationKind::MaxLength(1),
                Some("2 items"),
                SubmittedEcho::Rendered("[a, b]".to_string()),
            ),
            (
                "a value with no text rendering",
                "body",
                ViolationKind::MaxLength(1),
                Some("4000 bytes"),
                SubmittedEcho::Unrendered,
            ),
            (
                "a position this walk recorded nothing for",
                "elsewhere",
                ViolationKind::MaxLength(1),
                Some("whatever"),
                SubmittedEcho::Unrendered,
            ),
            (
                "a rendering past the stated bound",
                "huge",
                ViolationKind::MaxLength(1),
                Some("whatever"),
                SubmittedEcho::Unrendered,
            ),
        ];
        for (case, path, kind, value, expected) in rows {
            let violation = Violation {
                path: path.to_string(),
                kind,
                value: value.map(str::to_string),
                sensitive: false,
            };
            let mut budget = MAX_RESTORED_TOTAL;
            let actual = echo_of(&violation, &by_path, &mut budget, MAX_RESTORED_ECHO);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a MAP renders as the entries it holds — `{<key>=<value>, …}` in KEY order —
    /// for the caller that states that form, and as nothing for the caller that does not;
    /// an entry whose value has no text rendering, and a child that is no map entry at
    /// all, suppress the whole rendering rather than state a map missing an entry.
    ///
    /// The rows span what the rule generalises over: how many entries the map holds, which
    /// order they arrived in, and whether every entry can be stated.
    #[test]
    fn a_map_renders_as_its_entries_in_key_order() {
        let entry = |key: Option<&str>, rendering: Option<&str>| RenderedChild {
            name: Some("tags".to_string()),
            declared_by: None,
            key: key.map(str::to_string),
            rendering: rendering.map(str::to_string),
        };

        let rows: [(&str, Vec<RenderedChild>, Option<&str>); 6] = [
            ("no entries", vec![], Some("{}")),
            (
                "one entry",
                vec![entry(Some("a"), Some("1"))],
                Some("{a=1}"),
            ),
            (
                "entries that arrived in key order",
                vec![
                    entry(Some("a"), Some("1")),
                    entry(Some("b"), Some("2")),
                    entry(Some("c"), Some("3")),
                ],
                Some("{a=1, b=2, c=3}"),
            ),
            (
                "entries that arrived in another order",
                vec![
                    entry(Some("c"), Some("3")),
                    entry(Some("a"), Some("1")),
                    entry(Some("b"), Some("2")),
                ],
                Some("{a=1, b=2, c=3}"),
            ),
            (
                "an entry whose value has no text rendering",
                vec![entry(Some("a"), Some("1")), entry(Some("b"), None)],
                None,
            ),
            (
                "a child that is no map entry",
                vec![entry(None, Some("1"))],
                None,
            ),
        ];

        for (case, children, expected) in rows {
            let actual = rendered_entries(&children, MAX_RESTORED_ECHO);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The entries a caller renders by hand go through the same renderer, so a clause a
        // handler words and one restored from the walk state one map the same way.
        let stated = rendered_map(&[("a", "1"), ("b", "2")]);
        assert_eq!(
            stated, "{a=1, b=2}",
            "case \"stated by a caller\": expected the same form, got {stated}"
        );
    }

    /// RULE: the two renderings drop the SAME pair ([`super::unanchored`]'s positional
    /// rule, which the test above pins) and differ in exactly one thing —
    /// [`super::unanchored_grouping_alternation`] leaves a top-level ALTERNATION grouped,
    /// so the expression it quotes still matches what the model's anchored text matched,
    /// while [`super::unanchored`] quotes it bare. Nothing else is grouped: an alternation
    /// already inside a group is not wrapped again, and an expression whose pair was not
    /// dropped is stated verbatim by both.
    ///
    /// The rows are the model patterns this rule meets in the tree plus the regex
    /// constructs that can hide an anchor or an alternation — an escape, a character class,
    /// a group — and the half-anchored forms neither rendering may touch.
    #[test]
    fn only_the_grouping_rendering_wraps_a_top_level_alternation() {
        // (case, the model's pattern, what `unanchored` states, what
        // `unanchored_grouping_alternation` states)
        let rows: [(&str, &str, &str, &str); 15] = [
            ("a plain whole-value pattern", "^[0-9]{12}$", "[0-9]{12}", "[0-9]{12}"),
            (
                "an alternation already inside a group",
                "^(a|b)$",
                "(a|b)",
                "(a|b)",
            ),
            // The shape of this service family's `SessionIdentifier`: an ARN branch or a
            // bare uuid branch, sharing one outer pair.
            (
                "a top-level alternation of two groups",
                "^(arn:[^:]+:x)|([a-f0-9]{8})$",
                "(arn:[^:]+:x)|([a-f0-9]{8})",
                "((arn:[^:]+:x)|([a-f0-9]{8}))",
            ),
            (
                "a bare top-level alternation",
                "^a|b$",
                "a|b",
                "(a|b)",
            ),
            // Branches that carry an anchor of their own while SHARING the outer pair: the
            // pair is dropped positionally (see the test above), and the group this
            // rendering adds around what is left changes nothing about what it matches.
            (
                "branches anchored individually, sharing the outer pair",
                "^a$|^b$",
                "a$|^b",
                "(a$|^b)",
            ),
            // Branches parenthesised, so there is no outer pair to drop and nothing to
            // group.
            (
                "branches anchored individually inside their own groups",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
            ),
            ("anchored at the start only", "^abc", "^abc", "^abc"),
            ("anchored at the end only", "abc$", "abc$", "abc$"),
            ("no anchors at all", "[0-9]+", "[0-9]+", "[0-9]+"),
            (
                "a trailing escaped dollar, which is a literal",
                r"^cost\$$",
                r"cost\$",
                r"cost\$",
            ),
            (
                "an expression that IS a literal dollar",
                r"^\$$",
                r"\$",
                r"\$",
            ),
            (
                "a pipe inside a character class, which is a literal",
                "^[a|b]{2}$",
                "[a|b]{2}",
                "[a|b]{2}",
            ),
            (
                "a caret inside a negated class, which is not an anchor",
                "^[^:]+$",
                "[^:]+",
                "[^:]+",
            ),
            // A `]` in the first position of a class is the literal bracket, so the `|`
            // after it is still inside the class and alternates nothing.
            (
                "a literal bracket opening a class, with a pipe inside it",
                "^[]|]a$",
                "[]|]a",
                "[]|]a",
            ),
            (
                "an escaped bracket inside a class, with a pipe inside it",
                r"^[\]|]a$",
                r"[\]|]a",
                r"[\]|]a",
            ),
        ];

        for (case, pattern, plain, grouped) in rows {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, plain,
                "case {case:?}: expected unanchored {plain:?}, got {actual:?}"
            );
            let actual = unanchored_grouping_alternation(pattern);
            assert_eq!(
                actual, grouped,
                "case {case:?}: expected unanchored_grouping_alternation {grouped:?}, \
                 got {actual:?}"
            );
        }
    }

    /// RULE: [`super::unanchored_keeping_inner_anchors`] drops the same pair the two
    /// renderings above drop — whatever the expression's top level does, which is what the
    /// recorded lambdacore refusal quoted in [`super::unanchored`] states — and declines the
    /// drop for one case: an expression carrying an anchor of its OWN, which is a whole-value
    /// expression before any wrapping and so is quoted as the model declares it.
    ///
    /// The rows are the forms a published `@pattern` takes in the tree: a plain whole-value
    /// expression, the recorded top-level alternation that carries no anchor of its own, the
    /// alternations whose branches anchor themselves (sharing one pair, parenthesised, and
    /// with an empty first branch), the half-anchored forms no rendering may touch, and the
    /// regex constructs that can hide an anchor — an escape, a negated class, a class whose
    /// first member is a literal bracket.
    #[test]
    fn the_inner_anchor_rendering_declines_the_drop_for_a_self_anchored_expression() {
        // (case, the model's pattern, what `unanchored_keeping_inner_anchors` states)
        let rows: [(&str, &str, &str); 14] = [
            ("a plain whole-value pattern", "^[0-9]{12}$", "[0-9]{12}"),
            // The recorded form: a top-level alternation carrying no anchor of its own, so
            // the pair is the model's wrapping and is dropped, exactly as `unanchored`
            // states it (`com.amazonaws.lambdacore#NetworkConnectorName`).
            (
                "a recorded top-level alternation of a group and a bare branch",
                "^(arn:[^:]+:network-connector:[^:]+)|[a-zA-Z0-9_-]{1,64}$",
                "(arn:[^:]+:network-connector:[^:]+)|[a-zA-Z0-9_-]{1,64}",
            ),
            ("a bare top-level alternation", "^a|b$", "a|b"),
            ("an alternation already inside a group", "^(a|b)$", "(a|b)"),
            // Branches that anchor themselves while sharing the outer pair: dropping it
            // would state `a$|^b`, which requires nothing of either branch.
            (
                "branches anchored individually, sharing the outer pair",
                "^a$|^b$",
                "^a$|^b$",
            ),
            // The shape of an "empty or an ARN" member: the first branch IS the anchored
            // empty string, the only value it accepts.
            (
                "an anchored empty branch beside an anchored ARN branch",
                r"^$|^arn:aws:logs:r:1:log-group:g:\*$",
                r"^$|^arn:aws:logs:r:1:log-group:g:\*$",
            ),
            (
                "branches anchored inside their own groups, wrapped in a further pair",
                "^(^a$)|(^b$)$",
                "^(^a$)|(^b$)$",
            ),
            (
                "branches anchored inside their own groups, with no outer pair",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
            ),
            ("anchored at the start only", "^abc", "^abc"),
            ("anchored at the end only", "abc$", "abc$"),
            ("no anchors at all", "[0-9]+", "[0-9]+"),
            (
                "a trailing escaped dollar, which is a literal and no anchor",
                r"^cost\$$",
                r"cost\$",
            ),
            (
                "an escaped caret inside the expression, which is a literal",
                r"^a\^b$",
                r"a\^b",
            ),
            (
                "a caret negating a class, and a literal bracket opening another",
                "^[^:]+[]|]$",
                "[^:]+[]|]",
            ),
        ];

        for (case, pattern, stated) in rows {
            let actual = unanchored_keeping_inner_anchors(pattern);
            assert_eq!(
                actual, stated,
                "case {case:?}: expected unanchored_keeping_inner_anchors {stated:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE: [`super::unanchored_unless_published_as_declared`] quotes an expression the
    /// service's own list names exactly as the model writes it, and defers to
    /// [`super::unanchored_keeping_inner_anchors`] for every other expression — so the list
    /// decides nothing but its own rows.
    ///
    /// The rows span what the rule generalises over: a listed expression and an unlisted one
    /// over the same shape of text, a listed expression that rendering would have left alone
    /// anyway, an expression that only LOOKS like a listed one (the same text without its
    /// pair), and the empty list, which is every service that publishes no expression as
    /// declared.
    #[test]
    fn a_published_expression_is_quoted_as_the_model_declares_it() {
        let published: &[&str] = &[r"^aos-[a-zA-Z0-9]*$", "^a$|^b$"];
        // (case, the list, the model's pattern, what the rendering states)
        let rows: [(&str, &[&str], &str, &str); 6] = [
            (
                "an expression the list names",
                published,
                r"^aos-[a-zA-Z0-9]*$",
                r"^aos-[a-zA-Z0-9]*$",
            ),
            (
                "an expression of the same shape the list does not name",
                published,
                r"^vpce-[a-zA-Z0-9]*$",
                r"vpce-[a-zA-Z0-9]*",
            ),
            (
                "a listed expression the other rendering would keep anyway",
                published,
                "^a$|^b$",
                "^a$|^b$",
            ),
            (
                "the listed expression without its pair, which is a different expression",
                published,
                r"aos-[a-zA-Z0-9]*",
                r"aos-[a-zA-Z0-9]*",
            ),
            (
                "no expression published as declared",
                &[],
                r"^aos-[a-zA-Z0-9]*$",
                r"aos-[a-zA-Z0-9]*",
            ),
            (
                "no expression published as declared, over a self-anchored alternation",
                &[],
                "^a$|^b$",
                "^a$|^b$",
            ),
        ];

        for (case, list, pattern, stated) in rows {
            let actual = unanchored_unless_published_as_declared(pattern, list);
            assert_eq!(
                actual, stated,
                "case {case:?}: expected {stated:?}, got {actual:?}"
            );
        }
    }

    /// RULE: [`super::unanchored_at_the_close`] states the expression as the model declares
    /// it, less the `$` that closes it — so the opening `^` and every anchor the expression
    /// writes around its own branches stay, an expression that closes with no anchor is
    /// quoted verbatim, and a trailing escaped dollar is a literal rather than an anchor.
    ///
    /// The rows span the forms the two ends take (both anchored, start only, end only,
    /// neither), the alternations whose branches carry anchors of their own, the lookahead
    /// form the `CustomID` family of `iotsitewise` publishes, and the escape that hides an
    /// anchor.
    #[test]
    fn the_closing_anchor_rendering_drops_only_the_expressions_last_anchor() {
        // (case, the model's pattern, what `unanchored_at_the_close` states)
        let rows: [(&str, &str, &str); 8] = [
            ("a plain whole-value pattern", "^[0-9]{12}$", "^[0-9]{12}"),
            ("anchored at the start only", "^abc", "^abc"),
            ("anchored at the end only", "abc$", "abc"),
            ("no anchors at all", "[0-9]+", "[0-9]+"),
            ("branches anchored individually", "^a$|^b$", "^a$|^b"),
            (
                "branches anchored inside their own groups",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
            ),
            // The published form of a reference that is a UUID or an `externalId:` alias:
            // a lookahead, two anchored branches, and the closing anchor this drops.
            (
                "a two-branch reference pattern opening with a negative lookahead",
                "^(?!0{4})[0-9a-f]{4}$|^externalId:[a-zA-Z0-9_]+$",
                "^(?!0{4})[0-9a-f]{4}$|^externalId:[a-zA-Z0-9_]+",
            ),
            (
                "a trailing escaped dollar, which is a literal and no anchor",
                r"^cost\$",
                r"^cost\$",
            ),
        ];

        for (case, pattern, stated) in rows {
            let actual = super::unanchored_at_the_close(pattern);
            assert_eq!(
                actual, stated,
                "case {case:?}: expected unanchored_at_the_close {stated:?}, got {actual:?}"
            );
        }
    }

    /// RULE: [`super::anchors_its_own_ends`] reports whether EVERY alternative an expression
    /// offers at its top level — read through a group that wraps the whole expression —
    /// pins both of its own ends with a `^` and an unescaped `$`.
    ///
    /// The rows span the forms such an expression takes (branches anchored inside one
    /// wrapping group, anchored without one, a single anchored branch, an anchored empty
    /// branch), the forms that pin only one end or neither, an anchor that pins no end at
    /// all, and the constructs that can hide a `|` or an anchor: a group, a character class,
    /// a negated class, a class whose first member is a literal bracket, and an escape.
    #[test]
    fn an_expression_that_pins_both_of_its_own_ends_is_reported_as_such() {
        // (case, the expression, whether it anchors its own ends)
        let rows: [(&str, &str, bool); 14] = [
            // The `DeploymentStrategyId` shape: an alternation of whole-value branches,
            // kept together by one wrapping group.
            (
                "branches anchored inside one wrapping group",
                r"(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)",
                true,
            ),
            ("branches anchored with no wrapping group", r"^a$|^b$", true),
            ("branches anchored inside their own groups", r"(^a$)|(^b$)", true),
            ("one branch, anchored at both ends", r"^[a-z]+$", true),
            ("an anchored empty branch beside another", r"^$|^arn:aws.*$", true),
            ("a branch anchored at the start only", r"^a$|^b", false),
            ("anchored at the start only", r"^arn:aws.*", false),
            ("anchored at the end only", r"[0-9]{12}$", false),
            ("an interior dollar that pins neither end", r"a$b", false),
            ("no anchor at all", r"[a-zA-Z0-9_-]{1,64}", false),
            ("a wrapping group around an unanchored branch", r"((arn):[a-z]+)", false),
            ("two groups, neither wrapping the whole", r"(^a$)(^b$)", false),
            ("a caret that negates a class", r"[^0-9]", false),
            ("a trailing escaped dollar", r"^cost\$", false),
        ];

        for (case, expression, anchors) in rows {
            let actual = super::anchors_its_own_ends(expression);
            assert_eq!(
                actual, anchors,
                "case {case:?}: expected anchors_its_own_ends {anchors}, got {actual}"
            );
        }
    }

    /// RULE: [`super::whole_value_expression`] states the expression the WHOLE value must
    /// match, deciding the two ends independently — an anchored end loses its anchor, an
    /// open end is spelled with the unconstrained run it admits — while an expression that
    /// anchors only its own branches is quoted as declared, and a top-level alternation
    /// left bare by dropping BOTH anchors is kept grouped.
    ///
    /// The rows span every combination of the two ends (both anchored, neither, start only,
    /// end only), the alternation forms the Bedrock family's published patterns take
    /// (branches sharing one pair, branches anchored individually, branches anchored inside
    /// their own groups), an expression that already opens an end with the run, and the
    /// regex constructs that can hide an anchor or an alternation — an escape, a negated
    /// class, a class whose first member is a literal bracket.
    #[test]
    fn the_whole_value_rendering_decides_the_two_ends_independently() {
        // (case, the model's pattern, what `whole_value_expression` states)
        let rows: [(&str, &str, &str); 16] = [
            ("a plain whole-value pattern", "^[0-9]{12}$", "[0-9]{12}"),
            // The recorded `SessionId` form: one branch, both ends anchored.
            (
                "a recorded single-branch label pattern",
                "^[0-9a-zA-Z._:-]+$",
                "[0-9a-zA-Z._:-]+",
            ),
            // The recorded `FlowIdentifier` form: two groups sharing one pair, neither
            // branch anchored, so the leftover alternation is grouped.
            (
                "a top-level alternation of two unanchored groups",
                "^(arn:[^:]+:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$",
                "((arn:[^:]+:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))",
            ),
            ("a bare top-level alternation", "^a|b$", "(a|b)"),
            // The recorded `FlowExecutionIdentifier` form: the two ends are the first and
            // last branch's own anchors, and the interior ones stay where they are.
            (
                "branches anchored individually, sharing the outer pair",
                "^[a-zA-Z0-9-]{1,36}$|^(arn:[^:]+:execution/x)$",
                "[a-zA-Z0-9-]{1,36}$|^(arn:[^:]+:execution/x)",
            ),
            // The recorded `ModelIdentifier` form: the start is open, the end anchored.
            (
                "branches in their own groups with the anchors outside them",
                "(^a)$|(^b)$",
                ".*(^a)$|(^b)",
            ),
            (
                "branches anchored inside their own groups",
                "(^a$)|(^b$)",
                "(^a$)|(^b$)",
            ),
            (
                "one branch anchored inside its own group",
                "(^arn:[^:]+:session/x$)",
                "(^arn:[^:]+:session/x$)",
            ),
            ("anchored at the start only", "^abc", "abc.*"),
            ("anchored at the end only", "abc$", ".*abc"),
            ("no anchors at all", "[0-9]+", ".*[0-9]+.*"),
            (
                "an expression that already opens its tail with the run",
                "^abc.*",
                "abc.*",
            ),
            (
                "a trailing escaped dollar, which is a literal and no anchor",
                r"^cost\$",
                r"cost\$.*",
            ),
            (
                "a caret inside a negated class, which is not an anchor",
                "^[^:]+$",
                "[^:]+",
            ),
            (
                "a pipe inside a character class, which is a literal",
                "^[a|b]{2}$",
                "[a|b]{2}",
            ),
            (
                "a literal bracket opening a class, with a pipe inside it",
                "^[]|]a$",
                "[]|]a",
            ),
        ];

        for (case, pattern, stated) in rows {
            let actual = whole_value_expression(pattern);
            assert_eq!(
                actual, stated,
                "case {case:?}: expected whole_value_expression {stated:?}, got {actual:?}"
            );
        }
    }
}

