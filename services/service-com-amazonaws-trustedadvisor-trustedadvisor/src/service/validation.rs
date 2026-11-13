// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's input-validation refusal: how the constraint
//! violations the framework's own boundary walk collected become the `message`
//! the modelled `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAM — which requests are read and validated, and how
//! the modelled shape is serialized — and this module owns the sentence, so the clause
//! rules live in one place instead of being spread through the handlers that never see
//! them. What is ENFORCED is always the model's: every [`Violation`] handed to
//! [`refusal_message`] comes from `aws_smithy_schema_server::constraints::validate`, the
//! same call the centralized request walk makes.
//!
//! The rules, each stated over every member of every operation rather than over the
//! members one report happened to cite:
//!
//! 1. the envelope is `<N> validation error(s) detected: ` over one clause per failing
//!    member, joined with `; ` ([`refusal_sentence`]) — the framework's own envelope,
//!    kept so the two refusal paths cannot disagree. `<N>` is the number of violations
//!    COLLECTED, which for an absent `@required` member is one: the codec stops at the
//!    first one it misses, so a request missing two is stated as missing one. That is the
//!    frozen crate's own short-circuit, recorded here because the sentence is now this
//!    service's;
//! 2. a clause states the value the member held, its dotted path and the requirement it
//!    failed — except on a member whose target shape the model marks `@sensitive`, whose
//!    value is never echoed ([`SENSITIVE_MEMBERS`], [`subject`]);
//! 3. a reported `@pattern` is the expression a value has to satisfy, WITHOUT the `^` /
//!    `$` anchors a model writes to demand a whole-string match ([`unanchored`]);
//! 4. an enum-constraint clause lists the member's legal values in the order the SERVICE
//!    reports that enum in — a property of the enum SHAPE, not of the member or the
//!    operation — rather than in the model's declaration order
//!    ([`REPORTED_ENUM_ORDERS`], [`reported_order`]);
//! 5. a clause about a COLLECTION member states the collection the caller sent, rendered
//!    the way the service renders a structure (`<Shape>(<member>=<value>, …)` inside
//!    `[…]`), not the framework's `<n> items` summary of it
//!    ([`rendered_collections`], [`subject`]).
//!
//! # What this module does NOT render
//!
//! A MAP is not rendered: no input member of this service's model is a map
//! (`smithy_query(query="operation", …)` over all 12 operations — every input member is a
//! scalar, an enum, or the one list `RecommendationResourceExclusionList`), so there is no
//! recorded map rendering to state and nothing here invents one. Meeting a map therefore
//! declines every rendering of that request, and each clause keeps the value the
//! framework stored.
//!
//! A condition the model's constraint traits CANNOT express is refused by the handler that
//! owns it, so the framework's walk never collects it and it reaches no [`Violation`]. Such
//! a refusal is still one of this service's input-validation refusals and is worded here
//! too: the handler states its own subject, member path and requirement and builds the
//! sentence through [`clause`], [`echoed_value`] and [`refusal_sentence`], which is why
//! those three are the crate's and not this module's alone. Nothing about the CONDITION
//! moves here — only the sentence. Such a refusal is also ORDERED where the modelled ones
//! are: ahead of the support-level gate, so a malformed request is this 400 whether the
//! calling account's support plan includes the API or not.
//!
//! A condition SEVERAL handlers raise off ONE shared mechanism states its subject, member
//! path and requirement here as well, in one function those handlers call: a `nextToken`
//! that is not a cursor the read it was sent to issued is refused by every paginated
//! operation of this service off the single cursor encoding in [`crate::state`], so the
//! sentence for it is [`page_token_refusal`] rather than one copy per handler, which is
//! what keeps those copies from drifting from each other as well as from this module.
//!
//! A `@sensitive` member reached INSIDE a collection declines the same way, so rule 3
//! holds wherever a value is stated and not only where the violation was raised. Declining
//! is ALL OR NOTHING per request ([`rendered_collections`]): otherwise the sentence for one
//! member would depend on the order the request's other members happen to be written in.
//!
//! # The bounds this module states
//!
//! Rendering a collection INTO a message re-echoes request-sized content, so the walk
//! that builds it is bounded, and beyond those bounds the framework's own summary stands
//! — a shortened echo is a divergence in one message, while an unbounded one is a
//! response sized by the request:
//!
//! * [`MAX_RENDERED_TOTAL`] (64 KiB) — the rendered bytes ONE request's walk may hold.
//!   Every value is charged at least one byte, so the NUMBER of values the walk holds is
//!   bounded by it as well as their size.
//! * [`MAX_RENDERED_COLLECTION`] (16 KiB) — the largest single collection rendering a
//!   clause may carry. 64x the largest `@length` maximum a list member of this service
//!   declares (25 elements), so the fidelity this exists for always lands.
//!
//! Both are the bounds `services/common/src/validation.rs` states for the same job on
//! scalar values, for the same reason.
//!
//! What this module does NOT do is lengthen the SCALAR echo the framework stores: past
//! 256 bytes the frozen crate keeps a prefix plus `... [<n> bytes]`, which is text AWS
//! never emits, and carrying the whole value belongs in that crate — filed as
//! `issues/things-that-do-not-work/20261022T133000Z-gap-smithy-schema-server-constraint-violation-stores-only-the-elided-rejected-value.md`.
//! Only `nextToken` (`@length(4, 10000)`) can hold a value that long at all, and only
//! past its own modelled maximum.
//!
//! # NOTE on where these rules belong
//!
//! Rules 2, 3 and 5 are AWS's wording rather than this service's, and sibling service
//! crates carry their own copies of parts of them (`grep -rln 'fn unanchored' services/*/src`).
//! `services/common/src/validation.rs` is the staged home for exactly those clause rules
//! — its header lists the requirement half, the subject half and the unanchored
//! `@pattern` as "still to migrate" — so, as that header asks, this is the NOTE rather
//! than a silent further copy: migrating them re-words services whose recorded behaviour
//! their own suites pin, which is one change of its own. What would stay here either way
//! is this service's own: [`SENSITIVE_MEMBERS`], [`REPORTED_ENUM_ORDERS`] and the
//! collection rendering, which no other service has recorded answers for yet.

use std::collections::HashMap;

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The lowerCamel paths of every INPUT member this service's model targets at a
/// `@sensitive` shape, so a clause about one of them states the constraint it broke
/// without echoing the value back.
///
/// Source (`smithy_query(query="operation", …)` over all 12 operations, and
/// `smithy_query(query="shape", shape="RecommendationUpdateReason")`):
/// `com.amazonaws.trustedadvisor#RecommendationUpdateReason` is the only `@sensitive`
/// shape any input member targets (`sensitive: true`, `@length(10, 4096)`), and the
/// members that target it are `UpdateRecommendationLifecycle$updateReason` and
/// `UpdateOrganizationRecommendationLifecycle$updateReason` — one name, `updateReason`.
///
/// Matched on the deepest NAMED segment of a violation's path, because the index and
/// collection steps a path carries (`1`, `member`) name a position rather than a member.
const SENSITIVE_MEMBERS: &[&str] = &["updateReason"];

/// The order this service reports an enum's value set in, one row per enum.
///
/// The value set itself is the model's; the ORDER is the service's own and is not derived
/// from the model, so each row is recorded rather than computed. Source: the AWS answers
/// the issue
/// `20261022T174730Z-trustedadvisor-trusted-advisor-validationexception-message-rendering-diverges-4-causes`
/// records, one per enum — `RecommendationPillar`, `RecommendationSource`,
/// `RecommendationType` and `RecommendationStatus`. A row is matched by its value SET
/// (see [`reported_order`]), which is a property of the enum shape, so EVERY member
/// targeting that enum — in any operation, named anything — is reported the same way.
///
/// `ResourceStatus` declares the same values in the same model order as
/// `RecommendationStatus` (`smithy_query(query="shape", shape="ResourceStatus")`:
/// `[ok, warning, error]`), so it matches that row too; no recorded answer distinguishes
/// the two. An enum with no row here — `RecommendationLanguage`, `ExclusionStatus`,
/// `UpdateRecommendationLifecycleStage`,
/// `UpdateRecommendationLifecycleStageReasonCode` — keeps the model's declaration order,
/// which is the only order there is evidence for.
const REPORTED_ENUM_ORDERS: &[&[&str]] = &[
    // com.amazonaws.trustedadvisor#RecommendationPillar
    &[
        "security",
        "service_limits",
        "cost_optimizing",
        "operational_excellence",
        "fault_tolerance",
        "performance",
    ],
    // com.amazonaws.trustedadvisor#RecommendationSource
    &[
        "rds",
        "pse",
        "well_architected",
        "aws_config",
        "manual",
        "resilience_hub",
        "stir",
        "compute_optimizer",
        "resilience",
        "cost_explorer",
        "cost_optimization_hub",
        "ta_check",
        "security_hub",
        "lse",
    ],
    // com.amazonaws.trustedadvisor#RecommendationType
    &["priority", "standard"],
    // com.amazonaws.trustedadvisor#RecommendationStatus, and
    // com.amazonaws.trustedadvisor#ResourceStatus, whose value set is identical
    &["error", "warning", "ok"],
];

/// The rendered bytes one request's walk may hold (see the module docs).
const MAX_RENDERED_TOTAL: usize = 64 * 1024;

/// The largest single collection rendering a clause may carry (see the module docs).
const MAX_RENDERED_COLLECTION: usize = 16 * 1024;

/// Every collection member of one request, keyed by the dotted member path the
/// validating walk reports a violation on it at.
pub(super) type RenderedCollections = HashMap<String, RenderedCollection>;

/// One collection member as both sides of the substitution a clause makes: the framework's
/// own summary of it, and the collection itself.
///
/// The summary is kept so the substitution is EXACT — a clause states the collection only
/// where the value it would otherwise echo is the framework's rendering of that very
/// collection, never where the stored value is something else (a duplicate element a
/// `@uniqueItems` clause echoes, say).
#[derive(Debug)]
pub(super) struct RenderedCollection {
    /// What the framework's constraint walk stored for this collection: `<n> items`.
    summary: String,
    /// The collection as the service states it: `[<element>, <element>, …]`.
    value: String,
}

/// The `message` for a set of constraint violations, as AWS builds it from the request:
/// the count, then one clause per failing member.
///
/// `collections` carries the already-rendered value of every collection member of the
/// request ([`rendered_collections`]); it may be empty, in which case every clause states
/// the value the framework stored on the violation.
///
/// `None` declines the whole refusal — [`ViolationKind`] is `#[non_exhaustive]`, so a
/// violation class this service has no wording for keeps the framework's own, rather than
/// being dropped from a message that then under-reports what was wrong.
pub(super) fn refusal_message(
    violations: &[Violation],
    collections: &RenderedCollections,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::with_capacity(violations.len());
    for violation in violations {
        let requirement = requirement_of(&violation.kind)?;
        clauses.push(clause(
            &subject(violation, collections),
            &violation.path,
            &requirement,
        ));
    }
    refusal_sentence(&clauses)
}

/// ONE clause of the sentence: what the member held, the dotted path it is at, and the
/// requirement it failed.
///
/// The only place this service spells that skeleton, so a clause is worded identically
/// whether the violation came from the framework's constraint walk or from a handler
/// refusing a condition the model cannot express.
pub(crate) fn clause(subject: &str, member_path: &str, requirement: &str) -> String {
    format!("{subject} at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// The subject of a clause about a member that HELD a value: the value echoed between
/// quotes. The other two subjects a clause can state — an absent member, and a member
/// whose value is never echoed — are [`subject`]'s own.
pub(crate) fn echoed_value(value: &str) -> String {
    format!("Value '{value}'")
}

/// Rule 1's envelope over the clauses of ONE refusal: the clause count, then the clauses
/// joined with `; `, in the singular for a single clause.
///
/// `None` for no clauses at all — there is nothing refused, so there is no sentence to
/// state.
pub(crate) fn refusal_sentence(clauses: &[String]) -> Option<String> {
    match clauses {
        [] => None,
        [only] => Some(format!("1 validation error detected: {only}")),
        many => Some(format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        )),
    }
}

/// The member path a page-token clause names: the paginated member every
/// `@paginated(inputToken: nextToken)` operation of this service carries the cursor on,
/// which is the member a caller has to change.
const PAGE_TOKEN_MEMBER_PATH: &str = "nextToken";

/// What that member had to satisfy: a cursor is only meaningful to the read that issued
/// it (`crate::state::list_scope`), so what is required of the value is that it be one of
/// those.
///
/// Stated in the `Member must …` form every other requirement of this service's clauses
/// is in ([`requirement_of`]), which is where the wording comes from: unlike those, this
/// condition is in no recorded answer of this service, so there is no recorded sentence to
/// state and nothing here claims one. What IS recorded about this service's refusals — the
/// envelope and the clause skeleton, pinned against recorded answers by
/// `tests/all/validation_message_rendering.rs` — is reused rather than re-worded, so the
/// half that is a choice is the requirement half alone, and it is the same choice for all
/// seven paginated reads.
const PAGE_TOKEN_REQUIREMENT: &str =
    "Member must be a pagination token this service issued for this list";

/// The `message` of the refusal a `nextToken` this service did not issue for THIS read
/// earns, for every paginated operation of this service.
///
/// Only the CONDITION is the handler's — the modelled constraint traits say how long a
/// `nextToken` may be, not which values are cursors, so the framework's walk never
/// collects it. The SENTENCE is the service's and is built here, through the same
/// [`clause`], [`echoed_value`] and [`refusal_sentence`] every other refusal of this
/// service goes through, and in ONE place for all of them: the paginated operations share
/// one cursor encoding, so they state one sentence about a token that is not one of its
/// cursors rather than each wording its own.
///
/// The clause echoes the value the member held, as rule 2 states, and nothing more: the
/// echo is the request's own `nextToken`, whose size the model's `@length(4, 10000)`
/// bounds before any handler runs, so the sentence is bounded whatever the request holds.
pub(crate) fn page_token_refusal(token: &str) -> String {
    let clauses = [clause(
        &echoed_value(token),
        PAGE_TOKEN_MEMBER_PATH,
        PAGE_TOKEN_REQUIREMENT,
    )];
    // [`refusal_sentence`] declines an EMPTY clause list only, and this array always holds
    // the one clause above. The fallback is that clause itself rather than a panic or an
    // empty `message`: this runs on a request path, where a panic would poison the store
    // every other caller shares, and a caller handed the clause alone is still told what
    // to change.
    refusal_sentence(&clauses).unwrap_or_else(|| clauses.join("; "))
}

/// The rule half of a clause: what the member had to satisfy.
///
/// Every requirement but two is worded exactly as the framework words it, so the two
/// refusal paths state the same sentence. The two that differ are the ones AWS states
/// differently: a `@pattern` is reported unanchored, and an enum's value set in the order
/// the service reports it.
///
/// `ViolationKind::UniqueItems` is unreachable for this service and so is pinned by no
/// test row: the only list member it could fire on
/// (`RecommendationResourceExclusionList`, `@uniqueItems`) holds STRUCTURES, and the
/// framework's duplicate check records only the scalar leaves of a list (a structure
/// element pushes a shield frame, so nothing reaches the list's `seen`). The wording is
/// the framework's verbatim, so a request that ever did reach it is stated identically on
/// either refusal path.
///
/// `None` for a violation class this service has no wording for.
fn requirement_of(kind: &ViolationKind) -> Option<String> {
    let requirement = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => format!(
            "Member must satisfy enum value set: [{}]",
            reported_order(allowed).join(", ")
        ),
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(requirement)
}

/// The value half of a clause: what the member held.
///
/// * an ABSENT member held no value at all and is reported as the null it was;
/// * a `@sensitive` member names no value — the constraint it broke is the whole of what
///   a caller can act on, and the value belongs in nobody's logs, which is what
///   `@sensitive` exists to say, and the validating walk withholds such a value from the
///   violation itself;
/// * a COLLECTION member is reported as the collection the caller sent, in place of the
///   framework's `<n> items` summary of it;
/// * everything else echoes the value the framework stored.
///
/// A value the validating walk itself withheld (`Violation::sensitive`) is a withheld
/// value and not an absent one, so it is answered before the null above: the walk drops
/// a `@sensitive` member's value as it records the violation.
fn subject(violation: &Violation, collections: &RenderedCollections) -> String {
    if violation.sensitive {
        return "Value".to_string();
    }
    let Some(stored) = violation.value.as_deref() else {
        return "Value null".to_string();
    };
    // The same answer for a member this service's own list names, which is what the walk
    // withholds the value of: a value that reached here with the member still named keeps
    // no more of itself in the clause than one the walk had already dropped.
    if is_sensitive(&violation.path) {
        return "Value".to_string();
    }
    if let Some(collection) = collections.get(&violation.path) {
        if collection.summary == stored {
            return echoed_value(&collection.value);
        }
    }
    echoed_value(stored)
}

/// A `@pattern` expression without the anchors a model may spell it with: the service
/// reports the pattern a value must satisfy, not the anchoring Smithy adds to make the
/// match total, so a client that re-validates locally with the expression it read out of
/// the message uses the one AWS reports.
///
/// A `$` escaped by a backslash is a literal dollar sign inside the expression rather
/// than the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// `allowed` in the order this service reports that enum in, or in the order it arrived
/// (the model's) when there is no recorded order for it.
///
/// Matched by value SET, not by position or by the member the clause names: the order is a
/// property of the enum shape, and the set is what the walk reports, so every member
/// targeting that enum is reported alike and an enum with no row keeps the model's order.
fn reported_order(allowed: &[String]) -> Vec<&str> {
    match REPORTED_ENUM_ORDERS
        .iter()
        .find(|reported| is_same_set(reported, allowed))
    {
        Some(reported) => reported.to_vec(),
        None => allowed.iter().map(String::as_str).collect(),
    }
}

/// Whether two value sets hold the same values. An enum's values are unique by
/// definition, so equal lengths plus containment is set equality.
fn is_same_set(reported: &[&str], allowed: &[String]) -> bool {
    reported.len() == allowed.len()
        && reported
            .iter()
            .all(|value| allowed.iter().any(|candidate| candidate == value))
}

/// Does `path` name a member this service's model targets at a `@sensitive` shape?
///
/// Read off the deepest NAMED segment: the steps a path carries for a position inside a
/// collection (`1`, `member`) name no member.
///
/// Read BESIDE [`Violation::sensitive`], not instead of it: the flag answers for a value
/// the validating walk itself withheld, and this answers for one this service states a
/// clause about with the value still in hand (a handler's own violation).
fn is_sensitive(path: &str) -> bool {
    path.split('.')
        .rfind(|segment| {
            *segment != "member" && !segment.bytes().all(|byte| byte.is_ascii_digit())
        })
        .is_some_and(|segment| SENSITIVE_MEMBERS.contains(&segment))
}

/// Every collection member of `input`, rendered as a clause states it and keyed by the
/// path the validating walk reports it under.
///
/// A collection-level `@length` is checked on the collection member itself, and the
/// framework stores `<n> items` as the value it held — a count, not the value. AWS states
/// the collection the caller sent, which is not on the violation, so it is read off the
/// request here, through the same `SerializableStruct` walk the validator uses.
///
/// `input` and `schema` must be the ones the violations were collected from, so a
/// violation's path names a member of this walk.
///
/// ALL OR NOTHING per request: a walk that declined anything — met a value it does not
/// render, or ran out of budget — yields NO rendering at all, even for a collection it had
/// already rendered before declining. Otherwise the sentence would depend on the order the
/// request's members happen to be written in, and the clause for one member would be
/// worded by whether an unrelated member was large.
pub(super) fn rendered_collections(
    input: &dyn SerializableStruct,
    schema: &Schema,
) -> RenderedCollections {
    let mut walk = Rendering::new();
    // Rendering only: nothing here checks anything, so a walk that cannot be completed
    // simply leaves every clause the framework's own summary.
    let _ = walk.write_struct(schema, input);
    if walk.declined {
        return RenderedCollections::new();
    }
    walk.collections
}

/// Where in the value tree [`Rendering`] currently is, and what that frame has collected.
#[derive(Debug)]
enum Frame {
    /// A list, as its elements are collected.
    List { elements: Vec<String> },
    /// A structure, as its `<member>=<value>` fields are collected. `name` is the shape's
    /// own name, which the rendering opens with.
    Struct { name: String, fields: Vec<String> },
    /// A map: this module renders none (see the module docs), so nothing is collected
    /// inside one and the whole walk is declined.
    Map,
}

/// A [`ShapeSerializer`] that renders each collection member of a built value instead of
/// producing output — the read half of [`rendered_collections`].
///
/// It walks the same `serialize_members` tree the validating walk walks and spells a
/// member path the same way (`constraints::path::member`), so the keys it produces are
/// the paths a [`Violation`] carries. It checks nothing: the constraints have already
/// been checked, and this walk exists only to state the value one of them held.
#[derive(Debug)]
struct Rendering {
    /// Member-name segments from the root to the value being written.
    path: Vec<String>,
    /// Open frames, innermost last.
    frames: Vec<Frame>,
    /// The rendering of every collection member reached so far.
    collections: RenderedCollections,
    /// The rendered bytes this walk may still hold ([`MAX_RENDERED_TOTAL`]).
    budget: usize,
    /// How many collection ELEMENTS the walk is currently inside. A collection reached
    /// through one is not recorded (see [`Rendering::is_element`]).
    element_depth: usize,
    /// Set when the walk met something it does not render, or ran out of budget. Every
    /// rendering is then declined and each clause keeps the framework's own value.
    declined: bool,
}

impl Rendering {
    fn new() -> Self {
        Self {
            path: Vec::new(),
            frames: Vec::new(),
            collections: HashMap::new(),
            budget: MAX_RENDERED_TOTAL,
            element_depth: 0,
            declined: false,
        }
    }

    /// Whether the value about to be written is an ELEMENT of the collection enclosing
    /// it, rather than a member reached by name.
    ///
    /// The validating walk spells an element's path with its 1-based position
    /// (`<list>.<n>.member.…`), which this walk does not track, so a collection reached
    /// through an element is not recorded under a path a violation would carry — and is
    /// therefore not recorded at all, leaving the framework's own summary in the clause.
    /// This service's model nests no collection inside another, so the case does not
    /// arise today.
    fn is_element(&self) -> bool {
        matches!(
            self.frames.last(),
            Some(Frame::List { .. }) | Some(Frame::Map)
        )
    }

    /// Push `schema`'s member-name segment, if it has one. The root input struct and a
    /// collection element carry no member name, so they contribute no segment.
    fn push_segment(&mut self, schema: &Schema) -> bool {
        match schema.member_name() {
            Some(name) => {
                self.path.push(constraints::path::member(name));
                true
            }
            None => false,
        }
    }

    fn pop_segment(&mut self, pushed: bool) {
        if pushed {
            self.path.pop();
        }
    }

    /// Record one rendered value in the frame that encloses it.
    ///
    /// Charged against [`MAX_RENDERED_TOTAL`] — at least one byte per value, so the walk
    /// cannot be made to hold an unbounded NUMBER of empty renderings either.
    fn emit(&mut self, schema: &Schema, rendered: &str) {
        // Rule 2 holds wherever a value is stated, not only where the violation was
        // raised: a `@sensitive` member reached INSIDE a collection would otherwise be
        // echoed through that collection's rendering, which the clause for the collection
        // states. Nothing of this service's model puts one there today, so declining is
        // the whole answer — the clause keeps the framework's own summary.
        if schema
            .member_name()
            .is_some_and(|member| SENSITIVE_MEMBERS.contains(&member))
        {
            self.declined = true;
            return;
        }
        let entry = match self.frames.last() {
            Some(Frame::List { .. }) => rendered.to_string(),
            Some(Frame::Struct { .. }) => {
                // Every member the generated serializer writes carries its own member
                // schema. A value written into a structure without one cannot be named,
                // so the rendering is declined rather than guessed.
                let Some(member) = schema.member_name() else {
                    self.declined = true;
                    return;
                };
                format!("{member}={rendered}")
            }
            // Nothing inside a map is rendered, and a value written outside every frame
            // is the root input's own rendering, which no clause states.
            Some(Frame::Map) | None => return,
        };
        let Some(remaining) = self.budget.checked_sub(entry.len().max(1)) else {
            self.declined = true;
            return;
        };
        self.budget = remaining;
        match self.frames.last_mut() {
            Some(Frame::List { elements }) => elements.push(entry),
            Some(Frame::Struct { fields, .. }) => fields.push(entry),
            Some(Frame::Map) | None => {}
        }
    }
}

impl ShapeSerializer for Rendering {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        let pushed = self.push_segment(schema);
        let element = self.is_element();
        if element {
            self.element_depth += 1;
        }
        // A structure is stated as `<Shape>(<member>=<value>, …)`, so the rendering needs
        // the SHAPE's name. A collection element is written against the element shape's
        // own schema, which carries it; a structure MEMBER is written against the member's
        // schema, which names the member rather than its target, so that rendering is
        // declined. No input member of this service's model is a structure, so only the
        // element case arises here.
        let name = match schema.member_name() {
            None => schema.shape_id().shape_name().to_string(),
            Some(_) => {
                self.declined = true;
                String::new()
            }
        };
        self.frames.push(Frame::Struct {
            name,
            fields: Vec::new(),
        });
        let out = value.serialize_members(self);
        if let Some(Frame::Struct { name, fields }) = self.frames.pop() {
            self.emit(schema, &format!("{name}({})", fields.join(", ")));
        }
        if element {
            self.element_depth -= 1;
        }
        self.pop_segment(pushed);
        out
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let pushed = self.push_segment(schema);
        let element = self.is_element();
        if element {
            self.element_depth += 1;
        }
        self.frames.push(Frame::List {
            elements: Vec::new(),
        });
        let out = write_elements(self);
        if let Some(Frame::List { elements }) = self.frames.pop() {
            let rendered = format!("[{}]", elements.join(", "));
            // Recorded only when the whole list is in hand, and only under the path a
            // violation on it would carry: a walk that declined anything, a rendering past
            // the stated bound, or a list reached through a collection element leaves the
            // framework's own summary in the clause rather than stating a list the caller
            // did not send.
            if !self.declined && self.element_depth == 0 && rendered.len() <= MAX_RENDERED_COLLECTION
            {
                self.collections.insert(
                    constraints::path::join(&self.path),
                    RenderedCollection {
                        summary: format!("{} items", elements.len()),
                        value: rendered.clone(),
                    },
                );
            }
            self.emit(schema, &rendered);
        }
        if element {
            self.element_depth -= 1;
        }
        self.pop_segment(pushed);
        out
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let pushed = self.push_segment(schema);
        // This service's model binds no map input member, so there is no recorded map
        // rendering to state: meeting one declines every rendering of this request.
        self.declined = true;
        self.frames.push(Frame::Map);
        let out = write_entries(self);
        self.frames.pop();
        self.pop_segment(pushed);
        out
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        let rendered: &str = value.as_ref();
        self.emit(schema, rendered);
        Ok(())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        let rendered: &str = value.as_ref();
        self.emit(schema, rendered);
        Ok(())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.emit(schema, value);
        Ok(())
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        // The bytes themselves are not a rendering a message can carry, and this
        // service's model binds no blob member; the framework's own validator states a
        // blob leaf the same way.
        self.emit(schema, &format!("{} bytes", value.len()));
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.emit(schema, &value.to_string());
        Ok(())
    }

    fn write_document(&mut self, schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        // A document has no rendering of its own either; the framework's validator states
        // it the same way.
        self.emit(schema, "<document>");
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // An absent member holds no value: the generated serializer skips it, and a
        // structure rendering omits it, exactly as AWS omits an absent field.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 3, over the forms a model writes a `@pattern` in: both anchors, one anchor,
    /// neither, and a trailing `$` that is a literal dollar sign rather than the anchor.
    #[test]
    fn a_reported_pattern_drops_the_anchors_and_keeps_a_literal_dollar() {
        let cases = [
            (r"^arn:[\w-]+:trustedadvisor::\d{12}$", r"arn:[\w-]+:trustedadvisor::\d{12}"),
            (r"^[\s\S]*$", r"[\s\S]*"),
            ("^[a-z]+", "[a-z]+"),
            ("[a-z]+$", "[a-z]+"),
            ("[a-z]+", "[a-z]+"),
            // An escaped `$` is part of the expression: only the unescaped trailing
            // anchor is dropped.
            (r"^costs\$$", r"costs\$"),
            (r"^costs\$", r"costs\$"),
            // Two backslashes escape the backslash, so the `$` after them IS the anchor.
            (r"^back\\$", r"back\\"),
        ];
        for (pattern, expected) in cases {
            let actual = unanchored(pattern);
            assert_eq!(
                actual, expected,
                "case {pattern:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Rule 4 is keyed by the enum's value SET: a set with a recorded order is reported in
    /// it whatever order the walk hands it over in, and a set with none keeps the order it
    /// arrived in.
    #[test]
    fn an_enum_value_set_is_reported_in_its_recorded_order() {
        let pillar_in_model_order = owned(&[
            "cost_optimizing",
            "performance",
            "security",
            "service_limits",
            "fault_tolerance",
            "operational_excellence",
        ]);
        assert_eq!(
            reported_order(&pillar_in_model_order),
            vec![
                "security",
                "service_limits",
                "cost_optimizing",
                "operational_excellence",
                "fault_tolerance",
                "performance",
            ],
            "case RecommendationPillar: the recorded order is reported"
        );

        // The same set in another order is still that enum: the row is matched by set.
        let pillar_sorted = owned(&[
            "cost_optimizing",
            "fault_tolerance",
            "operational_excellence",
            "performance",
            "security",
            "service_limits",
        ]);
        assert_eq!(
            reported_order(&pillar_sorted),
            reported_order(&pillar_in_model_order),
            "case RecommendationPillar re-ordered: the same set reports the same order"
        );

        // An enum with no recorded order keeps the model's.
        let language = owned(&["en", "ja", "zh", "fr", "de"]);
        assert_eq!(
            reported_order(&language),
            vec!["en", "ja", "zh", "fr", "de"],
            "case a value set with no recorded order: the model's order is kept"
        );
    }

    /// Every recorded order is still an order for a value set the MODEL declares, read
    /// off the generated value-set constants. A model update that changes one of those
    /// sets then fails here, rather than silently falling back to the model's order with
    /// the recorded row left matching nothing.
    #[test]
    fn every_recorded_enum_order_is_a_modelled_value_set() {
        // The generated constants for the enums the rows are recorded for.
        let modelled: &[(&str, &[&str])] = &[
            (
                "RecommendationPillar",
                crate::ops::types::RECOMMENDATION_PILLAR_VALUES,
            ),
            (
                "RecommendationSource",
                crate::ops::types::RECOMMENDATION_SOURCE_VALUES,
            ),
            (
                "RecommendationType",
                crate::ops::types::RECOMMENDATION_TYPE_VALUES,
            ),
            (
                "RecommendationStatus",
                crate::ops::types::RECOMMENDATION_STATUS_VALUES,
            ),
            ("ResourceStatus", crate::ops::types::RESOURCE_STATUS_VALUES),
        ];
        for row in REPORTED_ENUM_ORDERS {
            let named = modelled
                .iter()
                .find(|(_, values)| is_same_set(row, &owned(values)))
                .map(|(name, _)| *name);
            assert!(
                named.is_some(),
                "case {row:?}: no modelled enum declares this value set any more"
            );
        }
        // And every one of those enums is covered by a row, so a reported order cannot be
        // lost by an edit to the catalog.
        for (name, values) in modelled {
            let covered = REPORTED_ENUM_ORDERS
                .iter()
                .any(|row| is_same_set(row, &owned(values)));
            assert!(
                covered,
                "case {name}: its value set has no recorded order in the catalog"
            );
        }
    }

    /// Rule 2 reads the deepest NAMED segment, so a member inside a collection is
    /// classified by its own name and a member whose name merely ENDS in a sensitive
    /// one is not mistaken for it.
    #[test]
    fn only_a_sensitive_member_suppresses_the_value() {
        let cases = [
            ("updateReason", true),
            ("exclusions.1.member.updateReason", true),
            ("updateReasonCode", false),
            ("notTheUpdateReason", false),
            ("recommendationIdentifier", false),
            ("recommendationResourceExclusions.1.member.arn", false),
        ];
        for (path, expected) in cases {
            let actual = is_sensitive(path);
            assert_eq!(
                actual, expected,
                "case {path:?}: expected is_sensitive {expected}, got {actual}"
            );
        }
    }

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }
}
