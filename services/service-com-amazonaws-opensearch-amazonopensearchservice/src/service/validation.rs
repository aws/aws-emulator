// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the boundary refusal of a request whose input breaks a
//! constraint the model declares.
//!
//! [`crate::service`] owns the seam (`Service::override_refusal`) and this module owns
//! the sentence. WHAT is enforced is always the model's: every [`Violation`] handed to
//! [`envelope`] comes from `aws_smithy_schema_server::constraints`, which reads the
//! `@length`, `@range`, `@pattern`, `@enum` and `@uniqueItems` traits codegen captured
//! off this service's model. Only how the violations are GROUPED into clauses is
//! decided here, and only for one class of them:
//!
//! * a violation on an ELEMENT of a collection member is stated against the COLLECTION
//!   MEMBER — no element position and no `member` step in the path — with the ELEMENT
//!   SHAPE'S WHOLE constraint set nested inside `Member must satisfy constraint: [ … ]`,
//!   and the collection rendered as the value. The set is the element shape's, not just
//!   the constraint that happened to fail: the clause tells a caller what every element
//!   of that member has to satisfy, so one request that breaks two of an element's
//!   constraints is ONE validation error about one member rather than one error per
//!   constraint at an indexed path. A violation on the collection member ITSELF (its own
//!   `@length`, its `@uniqueItems`) is not nested, and neither is one on a member INSIDE
//!   a structure element, which keeps its own full path;
//! * every OTHER violation keeps the framework's own clause, verbatim — the envelope,
//!   the member paths, the echoed value and every requirement sentence are the
//!   framework's, so nothing about them can drift from the one place that writes them.
//!
//! The element constraint set is read off the MODEL, not off the refusal: the structured
//! refusal carries only the constraints that FAILED, so the ones a value satisfied would
//! otherwise be missing from the clause. Codegen attaches a collection member's element
//! constraints to that member's schema as an
//! [`ElementConstraintsTrait`](aws_smithy_schema_server::constraints::ElementConstraintsTrait),
//! and [`ELEMENT_CONSTRAINTS`] indexes them for every input shape of this service.
//!
//! What the refusal does NOT carry is the value a collection member held — only the
//! elements that offended, each already elided to the framework's echo cap — so the
//! value a collection clause states is the one formed from those elements. That limit,
//! and why closing it does not belong in this seam, is recorded on [`Clause::render`].
//!
//! The SIZE of the message is bounded by the framework's own two ceilings on the walk
//! it comes from — how many violations one request accumulates, and how many bytes of a
//! value one of them echoes — and grouping only ever folds violations together, so a
//! refusal worded here is no larger than the one the framework would have worded. The one
//! statement that can ADD a clause ([`STATED`], whose recorded surface bounds a member's
//! requirements at two) adds at most one per violation, so the ceiling still holds as a
//! constant of this module rather than something one request chooses.
//!
//! # What a clause STATES, where that is not what the model declares
//!
//! A constraint clause reports the REQUIREMENT this service validates with, and four
//! renderings of it are the service's rather than the model's text:
//!
//! * a `@pattern` requirement quotes the expression the service PUBLISHES for the member's
//!   shape ([`quoted_pattern`]): the model's text verbatim for a shape whose published
//!   `Pattern` carries the whole-value pair ([`PUBLISHED_AS_DECLARED`]), and otherwise the
//!   model's text without that pair — dropped only where what the pair encloses carries no
//!   anchor of its own, since an expression whose branches are each anchored was a
//!   whole-value expression before any wrapping
//!   ([`::service_common::validation::unanchored_keeping_inner_anchors`], this tree's home
//!   for that rendering). The CHECK is unchanged — the boundary runs the model's pattern as
//!   written — so only the quoting differs;
//! * an enumerated type's value set the walk reported is stated in the order this
//!   service's front door walks its own hash table over the admissible values
//!   ([`::service_common::validation_message::in_hashed_enum_order`], this tree's home for
//!   that order), which is neither the order the MODEL declares them in nor lexicographic.
//!   The order belongs to the enumerated TYPE and is COMPUTED from the values themselves,
//!   so a type the model gains is stated in its own place and no table names it. The same
//!   order is what `crate::state`'s own refusal of a map KEY outside its key shape's set
//!   states, so the one set a caller may be refused on two paths is listed one way. A set
//!   an OPERATION declares itself, for a member the model types as a plain string, is
//!   stated in the order that operation states it: that order is part of what it states,
//!   and there is no model order to normalise;
//! * the MEMBERSHIP of one such set is the service's too, for the instance-type set: it is
//!   the catalog the service publishes rather than the older snapshot of it this model
//!   carries ([`in_published_instance_types`]);
//! * a member whose live validation surface the model has not caught up with states THAT
//!   surface ([`STATED`]): the expression the service quotes, and a bound it states a
//!   clause of its own for. Which members those are, and why each is there, is recorded
//!   on that table.
//!
//! One thing about a clause's VALUE half is the service's as well: a member the request
//! broke SEVERAL constraints of is stated without the value it submitted
//! ([`members_stated_without_their_value`]) — this front door echoes a member's value while
//! it has ONE thing to say about that member, and names the member and its requirements
//! alone once it has several.
//!
//! Two things about the message as a WHOLE are the service's too: the order the clauses
//! are stated in ([`STATED_FIRST`]), and one refusal that replaces the report entirely —
//! a request whose IAM role ARN cannot be read as one is answered with
//! [`invalid_role_arn`] and no constraint report at all, because that check runs before
//! the input's constraints are reported on.
//!
//! Every refusal of this service is worded by ONE function, [`worded`] — the boundary's
//! refusal, a constraint a HANDLER checks ([`constraint_report`]) and the input an
//! OPERATION validates for itself (`crate::service`'s seam over
//! `crate::ops::start_migration`, through [`stated_report`]) all go through it — so none of
//! them can word one constraint two ways. What differs between them is only where a
//! requirement CAME FROM: the two renderings above recover what the service states from
//! what the MODEL declares, so they are applied to the requirements the model declared and
//! not to the ones the stating code already stated.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use aws_smithy_schema::{Schema, ShapeId, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use aws_smithy_schema_server::{Violation, ViolationKind};
use service_common::validation_message::{in_hashed_enum_order, valueless_clause};
use smithy_server_runtime::{Refusal, Refused};

/// The error code AWS answers a constraint refusal with on the JSON protocols, and the
/// code the framework's own default picks there — named so a chosen message always
/// travels with its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The segment the framework's validation path carries for the step from a collection
/// into one of its elements: `constraints::path::element` spells an element as its
/// 1-based position followed by this segment (`packageUserList.1.member`).
const COLLECTION_STEP: &str = ".member";

/// The clause a collection member states around its element shape's constraint set.
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// The text that separates a clause's subject from the requirement it states, in the
/// framework's own sentence — the one place that sentence is written.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// The deepest collection nesting one clause states.
///
/// A step out of a collection is spelled `<position-or-key>.member`, and a MAP entry's
/// step carries the key the CALLER chose, so a key spelling `.<digits>.member` is
/// indistinguishable from a list position by the path alone: without a ceiling, the
/// nesting a request could ask for would grow with the length of that key. The inputs of
/// this model nest one collection deep (`packageUserList`, `vpcEndpointIds`, the tag and
/// id lists), so the ceiling is one past that — enough to state a list of lists, were
/// the model to grow one, and no more.
const MAX_DEPTH: usize = 2;

// =============================================================================
// Requirement wording
// =============================================================================

/// The requirement sentence of one violation class, as the FRAMEWORK words it.
///
/// Read back out of [`Violation::message`] — the ONE place that sentence is written —
/// rather than restated here, so a requirement this module nests into a collection
/// clause can never be worded differently from the clause a non-collection violation
/// keeps verbatim. `None` for a class whose sentence does not follow that shape, which
/// leaves that violation with the framework's own clause.
///
/// Stated through [`restated`], so a requirement read off the MODEL here and one read off
/// a violation carry the same renderings; both are idempotent, so a kind that already
/// went through it is unchanged by going through it again.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let probe = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
        sensitive: false,
    };
    restated(&probe)
        .message()
        .split_once(CONSTRAINT_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// A `@range` bound as it displays: an integral value without a trailing `.0`, a
/// fractional value with its decimals — the rendering the framework's own clause uses,
/// so a bound read off the model here and one read off a violation agree.
///
/// `{bound:.0}` rather than a cast through `i64`: a narrowing cast SATURATES, so a bound
/// whose magnitude is past `i64`'s would advertise `i64::MAX` instead of what the model
/// declares.
fn render_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.is_finite() {
        format!("{bound:.0}")
    } else {
        format!("{bound}")
    }
}

/// The syntax a VPC endpoint id has to satisfy, as this model declares it
/// (`smithy_query(query="shape",shape="VpcEndpointId")`).
///
/// Listed in [`PUBLISHED_AS_DECLARED`] because the API publishes this expression WITH the
/// pair: the `Pattern` the AWS API reference documents for a VPC endpoint id is
/// `^aos-[a-zA-Z0-9]*$`, anchors included, and a recorded refusal of a value outside it
/// quotes exactly that.
const VPC_ENDPOINT_ID_PATTERN: &str = r"^aos-[a-zA-Z0-9]*$";

/// The model `@pattern` texts this API publishes EXACTLY as the model writes them, anchors
/// included.
///
/// WHY A TABLE and not a rule over the text: the expression a clause quotes is the one the
/// SERVICE publishes for that member's shape — the `Pattern` its API reference documents —
/// and for part of this surface that text is the model's own while for the rest it is the
/// model's text without the whole-value pair ([`quoted_pattern`]). Which of the two a shape
/// is published with is not recoverable from the text: both are spelled `^…$` in the model,
/// so it is reference data about the shape. The EXPRESSION is the key rather than the member
/// path, because what is published belongs to the shape: every member targeting it — in any
/// operation, bound to a URI label, to a body member or to an element of a list — quotes the
/// one expression alike, and so does the element set stated at a collection of them.
///
/// A shape absent from here is published without the pair, which is every other shape of
/// this model whose expression carries no anchor of its own.
const PUBLISHED_AS_DECLARED: &[&str] = &[VPC_ENDPOINT_ID_PATTERN];

/// How this API quotes a `@pattern` requirement: the expression the SERVICE publishes for
/// the member's shape.
///
/// For a shape [`PUBLISHED_AS_DECLARED`] names, that is the model's text verbatim. For every
/// other shape it is the model's text without the whole-value pair the model wrapped that
/// expression in — dropped only where what the pair encloses carries no anchor of its OWN,
/// since an expression whose BRANCHES are each anchored (`^[0-9]{12}$|^GLOBAL$`) was already
/// a whole-value expression before any wrapping.
///
/// The RULE is the fleet's one home for it
/// ([`::service_common::validation::unanchored_unless_published_as_declared`], over
/// [`::service_common::validation::unanchored_keeping_inner_anchors`]), called rather than
/// restated here so this service cannot drift from the others recorded stating either form;
/// what is this crate's is the row above, which is about this model's shapes.
fn quoted_pattern(regex: &str) -> String {
    ::service_common::validation::unanchored_unless_published_as_declared(
        regex,
        PUBLISHED_AS_DECLARED,
    )
}

/// One violation as this service STATES it: the framework's own structure, with the
/// `@pattern` rendering every clause of this service uses applied ([`quoted_pattern`]).
///
/// The CHECK is untouched: the boundary runs the model's pattern as written.
fn restated(violation: &Violation) -> Violation {
    let quoted = match &violation.kind {
        ViolationKind::Pattern(regex) => Some(ViolationKind::Pattern(quoted_pattern(regex))),
        _ => None,
    };
    match quoted {
        Some(kind) => Violation {
            kind,
            ..violation.clone()
        },
        None => violation.clone(),
    }
}

// =============================================================================
// The value half of a clause
// =============================================================================

/// The members of `violations` whose clauses state NO submitted value: the ones the
/// request broke SEVERAL constraints of.
///
/// This front door echoes the value a member submitted while it has ONE thing to say about
/// that member, and states the member and its requirements alone once it has several: what
/// distinguishes the clauses of such a report is the requirement each names, and the value
/// is the same in all of them. A member with one violation keeps the echo, whatever it
/// carried and wherever it was bound — a URI label, a query parameter, a body member
/// nested in a structure.
///
/// Read off the violations the validating WALK reported, before this module restates any of
/// them: whether the front door had several things to say about a member is a fact of the
/// request it read, not of the clauses a surface the model lags adds afterwards ([`STATED`],
/// whose own recorded refusals echo the value in each clause they add).
fn members_stated_without_their_value(violations: &[Violation]) -> BTreeSet<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut several: BTreeSet<String> = BTreeSet::new();
    for violation in violations {
        if !seen.insert(violation.path.as_str()) {
            several.insert(violation.path.clone());
        }
    }
    several
}

// =============================================================================
// The constraints this service VALIDATES, where the model has not caught up
// =============================================================================

/// One requirement this service states for an input member, in the class the framework
/// words the sentence for.
#[derive(Debug)]
enum Stated {
    /// The expression the member's `@pattern` clause QUOTES, in place of the model's own
    /// text. Stated verbatim: it is the expression the service advertises, not a model
    /// `@pattern` to take the whole-value pair off.
    Pattern(&'static str),
    /// A maximum length this service states a clause of its own for, which this model
    /// declares no `@length` for at all.
    MaxLength(i64),
}

/// The input members whose validation surface is NOT the one this model declares, with
/// the requirements this service states for each in the order it states them.
///
/// Every entry is a member whose refusals show the service validating something the
/// model's traits do not express, which is why it cannot be read off a schema:
///
/// * an application `id` is validated against an expression that admits hyphens inside
///   the id and refuses a leading one, and that whole expression — a lookahead included,
///   which is no model `@pattern` wrapping — is what the clause quotes. The model's
///   `@pattern` for `Id` admits neither hyphens nor the lookahead;
/// * an `applicationId` is validated against the hyphen-admitting character class too,
///   and its length is bounded SEPARATELY, so a value past 30 characters is refused with
///   two clauses: the expression it is outside of, then the bound it is past. The model
///   declares the bound inside the pattern's repetition and no `@length` beside it, so
///   the boundary can only ever report the one clause;
/// * a `capabilityName` is validated against an expression the service states WITH the
///   anchors, so that member's clause quotes the expression as the model writes it rather
///   than without the pair every other member's clause drops.
///
/// What is keyed here is the MEMBER PATH, because that is all a boundary violation carries
/// — so an entry holds for every operation that declares the member. An operation that
/// validates its OWN input before the boundary does states its own surface beside its own
/// rule, and `crate::ops::start_migration` is the one that does; its `applicationId` is a
/// plain identifier it is handed rather than the id of an application it resolves, and the
/// refusal recorded for that operation quotes it accordingly, so the two surfaces differ
/// by operation and each is stated where that operation's rule lives.
///
/// A member absent from here states what the model declares, which is every other member
/// of every operation: this is the surface the model has not caught up with, not a second
/// copy of it.
const STATED: &[(&str, &[Stated])] = &[
    ("id", &[Stated::Pattern(r"^(?!-)[a-z0-9\-]{3,30}$")]),
    (
        "applicationId",
        &[Stated::Pattern("[a-z0-9-]{3,30}"), Stated::MaxLength(30)],
    ),
    ("capabilityName", &[Stated::Pattern("^[a-zA-Z0-9-]+$")]),
];

/// The clauses this service states for one ALREADY-RESTATED violation: the requirements
/// [`STATED`] records for that member, and the violation itself for every member it
/// records none for.
///
/// A recorded requirement is stated when the request BROKE it: the member's `@pattern`
/// clause when the boundary found the value outside the model's expression for it (the
/// two expressions describe the same class of value, which is why the model's check is
/// evidence for the service's), and a recorded bound when the value is past it. A
/// violation none of them covers — a minimum the model declares, an absent member —
/// keeps its own clause, so a refusal always states what it refused.
///
/// A requirement substituted here is already the expression the service QUOTES, so it is
/// not put through [`restated`]: that rendering recovers the service's expression from a
/// model `@pattern`, and these did not come from one.
fn stated_violations(violation: &Violation) -> Vec<Violation> {
    let Some((_, requirements)) = STATED.iter().find(|(path, _)| *path == violation.path) else {
        return vec![violation.clone()];
    };
    let length = violation
        .value
        .as_deref()
        .map(|value| value.chars().count())
        .and_then(|length| i64::try_from(length).ok());
    let stated: Vec<Violation> = requirements
        .iter()
        .filter_map(|requirement| match requirement {
            Stated::Pattern(expression) => matches!(violation.kind, ViolationKind::Pattern(_))
                .then(|| ViolationKind::Pattern((*expression).to_owned())),
            Stated::MaxLength(max) => length
                .is_some_and(|length| length > *max)
                .then_some(ViolationKind::MaxLength(*max)),
        })
        .map(|kind| Violation {
            path: violation.path.clone(),
            kind,
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        })
        .collect();
    if stated.is_empty() {
        return vec![violation.clone()];
    }
    stated
}

/// The members this service states a clause for AHEAD of the members the validating walk
/// reported first, highest precedence first.
///
/// The clauses of one refusal follow the order the service states its input members in,
/// which is the order the walk found them for every operation but one pair: an operation
/// naming a CAPABILITY of an application states the capability's own name before the id
/// of the application that scopes it, while this model lists the id first. The members
/// here are named, not the operations, because a violation carries the member path and
/// not the operation it came from — and `capabilityName` is declared by exactly the
/// operations that name a capability.
const STATED_FIRST: &[&str] = &["capabilityName"];

/// `violations` in the order this service states their clauses.
///
/// A STABLE sort on one key, so every member [`STATED_FIRST`] does not name keeps the
/// position the walk found it at, and the clauses of ONE member keep the order they were
/// stated in ([`stated_violations`]).
fn in_stated_order(mut violations: Vec<Violation>) -> Vec<Violation> {
    violations.sort_by_key(|violation| {
        STATED_FIRST
            .iter()
            .position(|stated| *stated == violation.path)
            .unwrap_or(STATED_FIRST.len())
    });
    violations
}

/// The member every operation that takes an IAM role ARN carries it at, as the validation
/// path spells it — a leaf member of the data-source type union, whatever branch of it a
/// request names.
const ROLE_ARN_MEMBER: &str = "roleArn";

/// The role ARN a request carried, when what the boundary refused is that this value
/// cannot be read as an IAM role ARN at all.
fn unreadable_role_arn(violations: &[Violation]) -> Option<&str> {
    violations
        .iter()
        .find(|violation| leaf_member(&violation.path) == ROLE_ARN_MEMBER)?
        .value
        .as_deref()
}

/// The last segment of a validation path: the member a clause is about, inside whatever
/// structure or union carried it.
fn leaf_member(path: &str) -> &str {
    path.rsplit_once('.').map_or(path, |(_, member)| member)
}

/// The refusal of a request whose IAM role ARN cannot be read as one.
///
/// This service resolves the ROLE a data source is read under before it reports on the
/// input's constraints, so a value no role ARN can be parsed out of is refused by that
/// step: the answer names the ARN the request carried and states no constraint report at
/// all — not even for the other members of the same request, which that step never got
/// past.
fn invalid_role_arn(role_arn: &str) -> String {
    format!("Invalid Role Arn : {role_arn}")
}

/// The `ValidationException` message for ONE constraint a HANDLER found broken — a
/// constraint the request boundary does not enforce, so no violation of it ever reaches
/// the refusal seam.
///
/// The envelope, the clause and the member path are the ones a boundary refusal states
/// ([`envelope`], and `constraints::path` for the lowerCamel member spelling), so a
/// caller cannot tell a constraint this service checks itself from one the model's traits
/// carry. `member` is the member's name as the MODEL spells it.
pub(crate) fn constraint_report(member: &str, value: &str, kind: ViolationKind) -> String {
    let violation = Violation {
        path: constraints::path::member(member),
        kind,
        value: Some(value.to_owned()),
        sensitive: false,
    };
    // Through [`stated_report`], the one path every refusal of this service is worded by.
    // One violation always states one clause, so a message is always rendered; the clause
    // alone is the fallback rather than an empty message.
    stated_report(std::slice::from_ref(&violation))
        .map(|refused| refused.message)
        .unwrap_or_else(|| restated(&violation).message())
}

/// The `ValidationException` message for a MAP whose KEY broke a constraint the map's KEY
/// SHAPE declares — the one constraint class no walk ahead of a handler reports, because
/// that walk treats a map's key as a path SEGMENT rather than as a constrained leaf, so
/// the handler applies the key shape's constraints and words the refusal with this.
///
/// The envelope, the clause and the member path are the ones a boundary refusal states
/// ([`::service_common::validation_message::map_key_requirements_refusal`], the fleet's one
/// home for a key clause, and `constraints::path` for the lowerCamel member spelling), and
/// the requirement is the framework's own sentence for that class ([`requirement`]) — so a
/// caller cannot tell a key constraint this service checks itself from one the model's
/// traits carry. `member` is the member's name as the MODEL spells it and `value` the map as
/// the clause quotes it, `None` for a map larger than a clause quotes.
pub(crate) fn map_key_constraint_report(
    member: &str,
    value: Option<&str>,
    kind: ViolationKind,
) -> String {
    let stated = requirement(&kind).unwrap_or_else(|| {
        Violation {
            path: String::new(),
            kind,
            value: None,
            sensitive: false,
        }
        .message()
    });
    ::service_common::validation_message::map_key_requirements_refusal(
        &constraints::path::member(member),
        value,
        std::slice::from_ref(&stated),
    )
}

// =============================================================================
// The element constraint sets this model declares
// =============================================================================

/// One constraint trait off a trait map, downcast to the type that carries its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Every requirement an element shape declares, in the order a clause states them.
///
/// The ORDER is the one the service's recorded refusals show: the `@length` maximum,
/// then its minimum, then the numeric bounds the same way, then the `@pattern`, then an
/// enumerated value set. A `@length` with no declared minimum states `0`, the smallest
/// length a Smithy `@length` can bound.
///
/// Each sentence comes from [`requirement`], so it is the framework's own wording for
/// that bound rather than a second spelling of it, over the kind [`restated`] renders —
/// so the `@pattern` an element set quotes and the one a clause of its own quotes are the
/// same expression.
///
/// Empty when the element shape declares no constraint at all.
fn element_requirements(traits: &TraitMap) -> Vec<String> {
    let mut kinds: Vec<ViolationKind> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            kinds.push(ViolationKind::MaxLength(max));
        }
        kinds.push(ViolationKind::MinLength(length.min.unwrap_or(0)));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            kinds.push(ViolationKind::MaxValue(render_bound(max)));
        }
        if let Some(min) = range.min {
            kinds.push(ViolationKind::MinValue(render_bound(min)));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        kinds.push(ViolationKind::Pattern(pattern.regex.to_string()));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        kinds.push(ViolationKind::EnumValue(
            allowed.values.iter().map(|value| (*value).to_string()).collect(),
        ));
    }
    kinds.iter().filter_map(requirement).collect()
}

/// The element constraint sets this service's input shapes declare, keyed by the member
/// path the validation walk reports that collection member under.
///
/// Built once, from the generated schemas — so what a clause states is the model's, and a
/// member whose element constraints change in the model changes here with it. A path can
/// hold SEVERAL sets, because two operations may name a member the same way over
/// different element shapes; which one a clause states is decided by the constraints that
/// actually failed (see [`element_constraint_set`]).
///
/// Only the members of the INPUT shapes themselves are indexed, so a collection nested
/// inside a structure member is reported against its collection member with the
/// constraints the request broke rather than with the element shape's whole set.
static ELEMENT_CONSTRAINTS: LazyLock<HashMap<String, Vec<Vec<String>>>> = LazyLock::new(|| {
    let mut catalog: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for schema in input_schemas() {
        for member in schema.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            let Some(traits) = member.traits() else {
                continue;
            };
            let Some(wrapper) = constraint_trait::<ElementConstraintsTrait>(
                traits,
                &ElementConstraintsTrait::TRAIT_ID,
            ) else {
                continue;
            };
            let requirements = element_requirements(wrapper.inner());
            if requirements.is_empty() {
                continue;
            }
            let sets = catalog.entry(constraints::path::member(name)).or_default();
            if !sets.contains(&requirements) {
                sets.push(requirements);
            }
        }
    }
    catalog
});

/// The element constraint set a clause at `path` states, given every requirement the walk
/// reported `path`'s elements as having broken.
///
/// Those failures are what identify the element shape when a path is declared by several
/// operations: a set that does not contain all of them is not the set this request broke.
/// They are taken TOGETHER — one bound a request broke may be declared by several element
/// shapes while the combination is not — which is why the set is resolved once per member
/// rather than once per violation. When no set is found, or several candidate sets contain
/// them all, the clause states the broken requirements alone: those are the thing known to
/// be true of the request, where a guessed set would not be.
fn element_constraint_set(path: &str, failing: &[String]) -> Vec<String> {
    let alone = || failing.to_vec();
    let Some(sets) = ELEMENT_CONSTRAINTS.get(path) else {
        return alone();
    };
    let mut matching = sets
        .iter()
        .filter(|set| failing.iter().all(|broken| set.contains(broken)));
    let Some(found) = matching.next() else {
        return alone();
    };
    if matching.any(|other| other != found) {
        return alone();
    }
    found.clone()
}

// =============================================================================
// Clause assembly
// =============================================================================

/// The collection a violation's path points INTO, when the violation is on one of that
/// collection's elements.
///
/// An element path ends with the collection step preceded by the element's 1-based
/// position (`packageUserList.1.member` -> `packageUserList`). The position is what
/// identifies the step, so a member that happens to be NAMED `member`
/// (`filters.1.member.member`) is not mistaken for one.
///
/// A MAP entry is spelled the same way over the key the CALLER chose
/// (`constraints::path::entry`), so an all-digit key is indistinguishable from a list
/// position by the path alone, and an entry under such a key is worded as a list element
/// would be. The member named and the constraint set stated stay the member's either way.
fn enclosing_collection(path: &str) -> Option<&str> {
    let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
    if position.is_empty() || !position.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(head)
}

/// `path` with its collection steps walked out of: the member a clause is reported at,
/// and how deep the nesting is.
///
/// At most [`MAX_DEPTH`] steps are walked, and the path and the depth are clamped
/// TOGETHER so the member named and the nesting stated always agree.
fn collapsed(path: &str) -> (&str, usize) {
    let mut path = path;
    let mut depth = 0;
    while depth < MAX_DEPTH {
        match enclosing_collection(path) {
            Some(collection) => {
                path = collection;
                depth += 1;
            }
            None => break,
        }
    }
    (path, depth)
}

/// `requirement` wrapped in the collection-constraint clause once per collection step,
/// built in ONE allocation rather than re-formatting the string per step.
fn nested(requirement: &str, depth: usize) -> String {
    if depth == 0 {
        return requirement.to_string();
    }
    let mut nested =
        String::with_capacity(requirement.len() + depth * (ELEMENT_SET_OPEN.len() + 1));
    for _ in 0..depth {
        nested.push_str(ELEMENT_SET_OPEN);
    }
    nested.push_str(requirement);
    for _ in 0..depth {
        nested.push(']');
    }
    nested
}

/// One clause of the envelope, accumulated as the violations are walked.
#[derive(Debug)]
enum Clause {
    /// The framework's own clause for one violation, kept verbatim: everything that is
    /// not a violation on a collection's element.
    Verbatim(String),
    /// One collection member's clause: the member it is reported at, the element shape's
    /// constraint set nested once per collection step, and the offending elements in walk
    /// order, each with the violation PATH it was found at — the path is what keeps ONE
    /// element that broke two of its shape's constraints from being rendered twice, while
    /// two different elements are both rendered.
    Collection {
        path: String,
        requirement: String,
        depth: usize,
        values: Vec<(String, String)>,
    },
}

impl Clause {
    /// The clause as the envelope states it.
    ///
    /// KNOWN LIMIT, recorded rather than papered over, in the VALUE a collection clause
    /// echoes: the structured refusal carries the OFFENDING elements, not the collection
    /// they came from, so a collection whose elements are only partly invalid is echoed
    /// as the collection formed from the invalid ones. The member path and the constraint
    /// set — what tells a caller WHICH member is wrong and what it must satisfy — are
    /// exact either way. Closing it is not this seam's to do: the refusal would have to
    /// carry the value the caller submitted, which only an operation-level seam holds.
    /// Inventing the missing elements here would echo bytes the caller never sent.
    fn render(&self) -> String {
        match self {
            Self::Verbatim(clause) => clause.clone(),
            Self::Collection {
                path,
                requirement,
                depth,
                values,
            } => {
                let values: Vec<&str> = values.iter().map(|(_, value)| value.as_str()).collect();
                let subject = if values.is_empty() {
                    "Value null".to_string()
                } else {
                    // One bracket per collection step, over the elements the refusal
                    // named (see the limit noted above).
                    format!(
                        "Value '{}{}{}'",
                        "[".repeat(*depth),
                        values.join(", "),
                        "]".repeat(*depth)
                    )
                };
                format!("{subject} at '{path}' {CONSTRAINT_MARKER}{requirement}")
            }
        }
    }
}

/// The `ValidationException` message for a refused request: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per offending member, in the
/// order the walk found them.
///
/// TWO passes over the violations, so the cost stays linear in their number: the first
/// resolves each collection member's element constraint set, and the second groups every
/// violation into the clause it belongs to, which is what collapses the elements of one
/// collection onto one clause. The count the envelope states is the number of clauses,
/// which for everything but a collection is the number of violations — exactly what the
/// framework's own envelope states.
///
/// `None` only when there is nothing to state.
///
/// `without_value` names the members whose clauses state no submitted value
/// ([`members_stated_without_their_value`]); a clause of any other member keeps the value
/// half the framework wrote.
fn envelope(violations: &[Violation], without_value: &BTreeSet<String>) -> Option<String> {
    // The element constraint set a collection member's clause states, resolved ONCE per
    // member from every requirement its elements broke (see [`element_constraint_set`]):
    // every element violation of that member then states the same requirement, which is
    // what folds them onto one clause.
    let mut broken: HashMap<&str, Vec<String>> = HashMap::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        if depth == 0 {
            continue;
        }
        if let Some(failing) = requirement(&violation.kind) {
            let failures = broken.entry(path).or_default();
            if !failures.contains(&failing) {
                failures.push(failing);
            }
        }
    }
    let element_sets: HashMap<&str, String> = broken
        .iter()
        .map(|(path, failures)| (*path, element_constraint_set(path, failures).join(", ")))
        .collect();

    let mut order: Vec<String> = Vec::new();
    let mut clauses: HashMap<String, Clause> = HashMap::new();
    for violation in violations {
        let (path, depth) = collapsed(&violation.path);
        let collection = (depth > 0)
            .then(|| requirement(&violation.kind))
            .flatten()
            .map(|failing| nested(element_sets.get(path).unwrap_or(&failing), depth));
        let Some(requirement) = collection else {
            // Not a violation on a collection's element: the framework's own clause,
            // kept exactly as it words it — with the value half dropped for a member this
            // report states without it (the drop itself is
            // [`::service_common::validation_message::valueless_clause`], so the
            // requirement, the member path and the punctuation around them stay the
            // framework's).
            let clause = match without_value.contains(&violation.path) {
                true => valueless_clause(violation).unwrap_or_else(|| violation.message()),
                false => violation.message(),
            };
            if clauses
                .insert(clause.clone(), Clause::Verbatim(clause.clone()))
                .is_none()
            {
                order.push(clause);
            }
            continue;
        };
        // A clause is keyed by the member it is reported at and the requirement it
        // states, so every element of ONE collection accumulates into a single clause —
        // and so do two constraints of one element shape, because both state that
        // shape's whole constraint set.
        let key = format!("{path}\u{1f}{requirement}");
        let clause = match clauses.get_mut(&key) {
            Some(clause) => clause,
            None => {
                order.push(key.clone());
                clauses.entry(key).or_insert(Clause::Collection {
                    path: path.to_string(),
                    requirement,
                    depth,
                    values: Vec::new(),
                })
            }
        };
        if let (Clause::Collection { values, .. }, Some(value)) =
            (clause, violation.value.as_deref())
        {
            // One rendered value per violation PATH: the same element breaking two of
            // its shape's constraints is one value, two elements are two.
            if !values.iter().any(|(seen, _)| *seen == violation.path) {
                values.push((violation.path.clone(), value.to_string()));
            }
        }
    }

    let rendered: Vec<String> = order
        .iter()
        .filter_map(|key| clauses.get(key))
        .map(Clause::render)
        .collect();
    match rendered.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", rendered[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            rendered.join("; ")
        )),
    }
}

/// The refusal `violations` are stated as: this service's clause order over the clauses
/// handed in, under the code every constraint refusal of this service travels with.
///
/// THE ONE wording of a refusal, so nothing can word one constraint two ways. Everything
/// a CALLER decides happens before it, and only where the requirement came FROM decides
/// it (see [`stated_report`] and [`aws_validation_refusal`]). `without_value` is the one
/// thing a caller passes in, because which members a report states without their value is
/// decided from the violations the walk REPORTED rather than from the clauses worded here
/// ([`members_stated_without_their_value`]).
fn worded(violations: Vec<Violation>, without_value: &BTreeSet<String>) -> Option<Refused> {
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope(&in_stated_order(violations), without_value)?,
    })
}

/// The refusal for violations whose requirements are the ones the stating code ALREADY
/// decided: an OPERATION that validates its own input before the boundary reports on the
/// model's traits (`crate::service`'s seam over `crate::ops::start_migration`), and a
/// constraint a HANDLER checked ([`constraint_report`]).
///
/// The shared `@pattern` rendering is applied — those requirements are declared the way a
/// model declares one, so the expression a clause quotes is recovered by the same rule
/// every other clause's is — and nothing is substituted: [`STATED`] records what this
/// service states INSTEAD of a requirement the MODEL stated, and there is no such
/// requirement here. Which value set and which expression those violations carry is the
/// stating code's own, recorded where that code's rule lives.
pub(crate) fn stated_report(violations: &[Violation]) -> Option<Refused> {
    worded(
        violations.iter().map(restated).collect(),
        &members_stated_without_their_value(violations),
    )
}

/// The `ValidationException` message for ONE constraint a HANDLER found broken whose
/// requirement is an expression this service PUBLISHES as the model writes it, anchors and
/// all.
///
/// The clause, the member path and the envelope are [`stated_report`]'s, so a caller cannot
/// tell this refusal from a boundary one; what differs is the one thing this entry point
/// decides, and the reason it exists: the pair-dropping rendering every requirement read
/// off a model `@pattern` goes through ([`restated`]) recovers an expression the service
/// advertises from the text a model wrapped, and `expression` is already the text this
/// member's own refusals are recorded quoting, so there is nothing to recover from it.
/// `path` is the member path as a clause spells one (`constraints::path`), which for a
/// member inside a structure is the whole dotted path.
pub(crate) fn published_pattern_report(path: &str, value: &str, expression: &str) -> String {
    let violation = Violation {
        path: path.to_owned(),
        kind: ViolationKind::Pattern(expression.to_owned()),
        value: Some(value.to_owned()),
        sensitive: false,
    };
    // One violation always states one clause, so a message is always rendered; the clause
    // alone is the fallback rather than an empty message.
    worded(
        vec![violation.clone()],
        &members_stated_without_their_value(std::slice::from_ref(&violation)),
    )
    .map_or_else(|| violation.message(), |refused| refused.message)
}

/// State `refusal` the way this service states it, or `None` to keep the framework's own
/// wording.
///
/// This is the whole `Service::override_refusal` body: a constraint refusal is answered
/// as `400 ValidationException` — the status and code the framework's own default picks
/// on this service's `restJson1` protocol — carrying [`envelope`]'s message, so the code
/// a client branches on travels with the message it is given.
///
/// What the message states is decided in five steps, in this order: a request whose IAM
/// role ARN cannot be read as one is answered with [`invalid_role_arn`] alone, because
/// that step runs before the input's constraints are reported on; the instance-type value
/// set is stated as the catalog the service publishes
/// ([`in_published_instance_types`]); every enumerated type's
/// value set the walk reported is stated in the order this service states it
/// ([`in_hashed_enum_order`]); every `@pattern` is
/// quoted as the service publishes it ([`restated`]); and every member [`STATED`] records
/// a surface for states that surface instead of the model's. The clauses are then worded
/// by [`worded`], the one wording every refusal of this service goes through, over the
/// members this report states without their value
/// ([`members_stated_without_their_value`], read off the violations as the walk reported
/// them).
///
/// `None` for anything that is not a constraint refusal (an unauthenticated caller keeps
/// the framework's 403 code and wording), and `None` when there is nothing to state.
pub(super) fn aws_validation_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if let Some(role_arn) = unreadable_role_arn(violations) {
        return Some(Refused {
            code: VALIDATION_EXCEPTION.to_owned(),
            message: invalid_role_arn(role_arn),
        });
    }
    worded(
        violations
            .iter()
            .map(in_published_instance_types)
            .map(|violation| in_hashed_enum_order(&violation))
            .map(|violation| restated(&violation))
            .flat_map(|violation| stated_violations(&violation))
            .collect(),
        &members_stated_without_their_value(violations),
    )
}

/// The instance types the service publishes, resolved ONCE per process: the catalog is
/// reference data rather than anything a request carries, so the walk over it and its
/// ordering are not work a refusal pays per violation.
static PUBLISHED_INSTANCE_TYPES: LazyLock<Vec<&'static str>> =
    LazyLock::new(crate::state::published_instance_types);

/// `violation` with the INSTANCE-TYPE value set it reports replaced by the set the service
/// publishes, and every other violation returned unchanged.
///
/// A refusal of a value outside an enumerated type names the values that type ADMITS, and
/// for the instance types that list is the service's own published catalog
/// ([`PUBLISHED_INSTANCE_TYPES`], over `crate::state::published_instance_types`) — the record
/// of which instance types exist, which the operations resolving one against it refuse a
/// value outside. The `OpenSearchPartitionInstanceType` value set this model carries is an
/// older SNAPSHOT of that catalog (it stops before the seventh-generation Intel, Graviton4,
/// dense-storage and second-generation OpenSearch-optimized families the catalog publishes),
/// so a clause stating the snapshot would tell a caller that instance types the service does
/// publish are invalid, and would omit the ones it has to choose from. The message is the
/// only place the accepted list is stated, so it states the catalog.
///
/// Recognised by the value SET rather than by the member path — the same reading
/// [`::service_common::validation_message::stated_enum_value_set`] applies — so every member
/// targeting that enum is stated alike, in any operation, in whatever binding carried the
/// value and at any depth. Nothing else about the violation moves: the member path, the
/// echoed value and the requirement's wording stay the framework's, and the ORDER the set is
/// listed in is still [`in_hashed_enum_order`]'s, applied after this.
///
/// WHAT THIS DOES NOT CHANGE, recorded rather than papered over: which values the boundary
/// ADMITS. The walk in front of a handler checks a member against the enum codegen captured
/// off the model, so a request naming an instance type only the catalog publishes is still
/// refused before any handler runs; the admissible set is the model's until the model's enum
/// carries the catalog, which no refusal wording can decide. The clause states what the
/// service accepts because that is what the message is for — telling a caller which values
/// to choose from.
///
/// The set is a CONSTANT of this service (its catalog), so the size of the clause this
/// states is a constant of this crate rather than anything one request chooses, exactly as
/// the modelled set it replaces was.
fn in_published_instance_types(violation: &Violation) -> Violation {
    let rows = [(
        crate::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES,
        PUBLISHED_INSTANCE_TYPES.as_slice(),
    )];
    ::service_common::validation_message::in_stated_enum_order(violation, &rows)
}

/// Every INPUT shape of this service, as the generated schema statics.
///
/// All of them, not the ones one report happened to cite: an element constraint is a
/// property of an input shape, so leaving an operation out would silently word its
/// collection clauses with less than the model declares. Regenerate with
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs`, which names exactly these statics; an
/// input shape missing from this list is not a wrong clause, only a clause that states
/// the constraints that failed rather than the element shape's whole set.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::ACCEPT_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::ADD_DATA_SOURCE_INPUT_SCHEMA,
        t::ADD_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::ADD_TAGS_INPUT_SCHEMA,
        t::ASSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::ASSOCIATE_PACKAGES_INPUT_SCHEMA,
        t::ATTACH_DATA_SOURCE_INPUT_SCHEMA,
        t::AUTHORIZE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::CANCEL_DOMAIN_CONFIG_CHANGE_INPUT_SCHEMA,
        t::CANCEL_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::CREATE_APPLICATION_INPUT_SCHEMA,
        t::CREATE_DOMAIN_INPUT_SCHEMA,
        t::CREATE_INDEX_INPUT_SCHEMA,
        t::CREATE_OUTBOUND_CONNECTION_INPUT_SCHEMA,
        t::CREATE_PACKAGE_INPUT_SCHEMA,
        t::CREATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DELETE_APPLICATION_INPUT_SCHEMA,
        t::DELETE_DATA_SOURCE_INPUT_SCHEMA,
        t::DELETE_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::DELETE_DOMAIN_INPUT_SCHEMA,
        t::DELETE_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::DELETE_INDEX_INPUT_SCHEMA,
        t::DELETE_OUTBOUND_CONNECTION_INPUT_SCHEMA,
        t::DELETE_PACKAGE_INPUT_SCHEMA,
        t::DELETE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::DEREGISTER_CAPABILITY_INPUT_SCHEMA,
        t::DESCRIBE_DATA_SOURCE_ATTACHMENT_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_AUTO_TUNES_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_CHANGE_PROGRESS_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_HEALTH_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_INPUT_SCHEMA,
        t::DESCRIBE_DOMAIN_NODES_INPUT_SCHEMA,
        t::DESCRIBE_DOMAINS_INPUT_SCHEMA,
        t::DESCRIBE_DRY_RUN_PROGRESS_INPUT_SCHEMA,
        t::DESCRIBE_INBOUND_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_INSIGHT_DETAILS_INPUT_SCHEMA,
        t::DESCRIBE_INSTANCE_TYPE_LIMITS_INPUT_SCHEMA,
        t::DESCRIBE_OUTBOUND_CONNECTIONS_INPUT_SCHEMA,
        t::DESCRIBE_PACKAGES_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_INSTANCE_OFFERINGS_INPUT_SCHEMA,
        t::DESCRIBE_RESERVED_INSTANCES_INPUT_SCHEMA,
        t::DESCRIBE_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::DETACH_DATA_SOURCE_INPUT_SCHEMA,
        t::DISSOCIATE_PACKAGE_INPUT_SCHEMA,
        t::DISSOCIATE_PACKAGES_INPUT_SCHEMA,
        t::GET_APPLICATION_INPUT_SCHEMA,
        t::GET_CAPABILITY_INPUT_SCHEMA,
        t::GET_COMPATIBLE_VERSIONS_INPUT_SCHEMA,
        t::GET_DATA_SOURCE_INPUT_SCHEMA,
        t::GET_DEFAULT_APPLICATION_SETTING_INPUT_SCHEMA,
        t::GET_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::GET_DOMAIN_MAINTENANCE_STATUS_INPUT_SCHEMA,
        t::GET_INDEX_INPUT_SCHEMA,
        t::GET_MIGRATION_INPUT_SCHEMA,
        t::GET_PACKAGE_VERSION_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_HISTORY_INPUT_SCHEMA,
        t::GET_UPGRADE_STATUS_INPUT_SCHEMA,
        t::INSIGHT_FEEDBACK_INPUT_SCHEMA,
        t::LIST_APPLICATIONS_INPUT_SCHEMA,
        t::LIST_DATA_SOURCE_ATTACHMENTS_INPUT_SCHEMA,
        t::LIST_DATA_SOURCES_INPUT_SCHEMA,
        t::LIST_DIRECT_QUERY_DATA_SOURCES_INPUT_SCHEMA,
        t::LIST_DOMAIN_MAINTENANCES_INPUT_SCHEMA,
        t::LIST_DOMAIN_NAMES_INPUT_SCHEMA,
        t::LIST_DOMAINS_FOR_PACKAGE_INPUT_SCHEMA,
        t::LIST_INSIGHTS_INPUT_SCHEMA,
        t::LIST_INSTANCE_TYPE_DETAILS_INPUT_SCHEMA,
        t::LIST_MIGRATIONS_INPUT_SCHEMA,
        t::LIST_PACKAGES_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_SCHEDULED_ACTIONS_INPUT_SCHEMA,
        t::LIST_TAGS_INPUT_SCHEMA,
        t::LIST_VERSIONS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_FOR_DOMAIN_INPUT_SCHEMA,
        t::LIST_VPC_ENDPOINTS_INPUT_SCHEMA,
        t::PURCHASE_RESERVED_INSTANCE_OFFERING_INPUT_SCHEMA,
        t::PUT_DEFAULT_APPLICATION_SETTING_INPUT_SCHEMA,
        t::REGISTER_CAPABILITY_INPUT_SCHEMA,
        t::REJECT_INBOUND_CONNECTION_INPUT_SCHEMA,
        t::REMOVE_TAGS_INPUT_SCHEMA,
        t::REVOKE_VPC_ENDPOINT_ACCESS_INPUT_SCHEMA,
        t::ROLLBACK_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::START_DOMAIN_MAINTENANCE_INPUT_SCHEMA,
        t::START_MIGRATION_INPUT_SCHEMA,
        t::START_SERVICE_SOFTWARE_UPDATE_INPUT_SCHEMA,
        t::UPDATE_APPLICATION_INPUT_SCHEMA,
        t::UPDATE_DATA_SOURCE_INPUT_SCHEMA,
        t::UPDATE_DIRECT_QUERY_DATA_SOURCE_INPUT_SCHEMA,
        t::UPDATE_DOMAIN_CONFIG_INPUT_SCHEMA,
        t::UPDATE_INDEX_INPUT_SCHEMA,
        t::UPDATE_PACKAGE_INPUT_SCHEMA,
        t::UPDATE_PACKAGE_SCOPE_INPUT_SCHEMA,
        t::UPDATE_SCHEDULED_ACTION_INPUT_SCHEMA,
        t::UPDATE_VPC_ENDPOINT_INPUT_SCHEMA,
        t::UPGRADE_DOMAIN_INPUT_SCHEMA,
    ]
}

#[cfg(test)]
mod tests {
    use super::{aws_validation_refusal, collapsed, ELEMENT_CONSTRAINTS};
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use smithy_server_runtime::Refusal;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
            sensitive: false,
        }
    }

    /// RULE: an element path collapses onto the collection it points into, and nothing
    /// else does.
    #[test]
    fn an_element_path_collapses_onto_its_collection() {
        // (case, the path the walk reported, the member a clause names, the nesting)
        let rows: [(&str, &str, &str, usize); 6] = [
            ("a scalar member", "domainName", "domainName", 0),
            (
                "one list element",
                "packageUserList.1.member",
                "packageUserList",
                1,
            ),
            (
                "an element far down the list",
                "vpcEndpointIds.17.member",
                "vpcEndpointIds",
                1,
            ),
            (
                "a member inside a structure element",
                "packageList.2.member.packageID",
                "packageList.2.member.packageID",
                0,
            ),
            ("a list of lists", "outer.1.member.2.member", "outer", 2),
            (
                "a member merely named member",
                "filters.member",
                "filters.member",
                0,
            ),
        ];
        for (case, path, expected, depth) in rows {
            let actual = collapsed(path);
            assert_eq!(
                actual,
                (expected, depth),
                "case {case:?}: expected {expected:?} at depth {depth}, got {actual:?}"
            );
        }
    }

    /// RULE: a violation on a collection's element is ONE validation error, reported at
    /// the collection member, with the element shape's whole constraint set.
    #[test]
    fn element_violations_of_one_member_collapse_onto_one_clause() {
        // Both constraints of `PackageUser` that one malformed account id breaks, as the
        // walk reports them: at the element's indexed path, one violation per constraint.
        let refusal = Refusal::Invalid(vec![
            violation(
                "packageUserList.1.member",
                ViolationKind::MaxLength(12),
                "not-an-account-id",
            ),
            violation(
                "packageUserList.1.member",
                ViolationKind::Pattern(r"^[0-9]{12}$|^GLOBAL$".to_owned()),
                "not-an-account-id",
            ),
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, "ValidationException",
            "expected the JSON protocols' own refusal code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value '[not-an-account-id]' at 'packageUserList' \
             failed to satisfy constraint: Member must satisfy constraint: [Member must have \
             length less than or equal to 12, Member must have length greater than or equal to \
             6, Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$]",
            "expected one clause at the list member carrying the element shape's whole \
             constraint set, got {:?}",
            stated.message
        );
    }

    /// RULE: a violation that is not on a collection's element keeps the framework's own
    /// clause, and each one is counted on its own.
    #[test]
    fn a_scalar_violation_keeps_the_frameworks_clause() {
        let refusal = Refusal::Invalid(vec![
            violation("domainName", ViolationKind::MinLength(3), "ab"),
            Violation {
                path: "packageID".to_owned(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            },
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value 'ab' at 'domainName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3; Value null at \
             'packageID' failed to satisfy constraint: Member must not be null",
            "expected the framework's own clauses, got {:?}",
            stated.message
        );
    }

    /// RULE: the element constraint set a clause states is read off the MODEL, so it
    /// carries a member's constraints whether or not the request broke them — each stated
    /// the way this service states that requirement, the `@pattern` expression included
    /// (quoted as the model declares it here, since both of its branches are anchored).
    #[test]
    fn the_element_constraint_catalog_is_built_from_the_model() {
        let sets = ELEMENT_CONSTRAINTS
            .get("packageUserList")
            .expect("the model declares element constraints for packageUserList");
        assert!(
            sets.iter().any(|set| set
                == &vec![
                    "Member must have length less than or equal to 12".to_owned(),
                    "Member must have length greater than or equal to 6".to_owned(),
                    "Member must satisfy regular expression pattern: ^[0-9]{12}$|^GLOBAL$"
                        .to_owned(),
                ]),
            "expected PackageUser's whole constraint set in model order, got {sets:?}"
        );
    }

    /// RULE: a violation at a member whose live validation surface the model lags states
    /// THAT surface — the expression the service quotes, and a bound it states a clause of
    /// its own for when the value is past it.
    #[test]
    fn a_stated_member_reports_the_surface_the_service_validates() {
        // One value past the recorded 30-character bound, which the model declares no
        // `@length` for, so the walk reports the pattern alone.
        let refusal = Refusal::Invalid(vec![violation(
            "applicationId",
            ViolationKind::Pattern("^[a-z0-9]{3,30}$".to_owned()),
            "thirty-one-characters-long-x-id",
        )]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value 'thirty-one-characters-long-x-id' at \
             'applicationId' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: [a-z0-9-]{3,30}; Value 'thirty-one-characters-long-x-id' at \
             'applicationId' failed to satisfy constraint: Member must have length less than \
             or equal to 30",
            "expected the expression this service quotes and the bound it states, got {:?}",
            stated.message
        );
    }

    /// RULE: the clauses are stated in the order this service states its input members,
    /// which for a capability's name and the application id that scopes it is not the
    /// order the walk reported them in.
    #[test]
    fn a_capability_name_is_stated_before_the_application_that_scopes_it() {
        let refusal = Refusal::Invalid(vec![
            violation(
                "applicationId",
                ViolationKind::Pattern("^[a-z0-9]{3,30}$".to_owned()),
                "probe_BAD",
            ),
            violation(
                "capabilityName",
                ViolationKind::Pattern("^[a-zA-Z0-9-]+$".to_owned()),
                "BAD_NAME",
            ),
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value 'BAD_NAME' at 'capabilityName' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \
             ^[a-zA-Z0-9-]+$; Value 'probe_BAD' at 'applicationId' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-z0-9-]{3,30}",
            "expected the capability's own name stated first, got {:?}",
            stated.message
        );
    }

    /// RULE: a role ARN that cannot be read as one replaces the whole report, whichever
    /// requirement of that member the value broke and whatever else the request broke.
    #[test]
    fn an_unreadable_role_arn_replaces_the_whole_report() {
        // (case, the violation at the role ARN member, the ARN the sentence names)
        let rows = [
            (
                "a role ARN outside its expression",
                violation(
                    "dataSourceType.cloudWatchLog.roleArn",
                    ViolationKind::Pattern("^arn:aws:iam::".to_owned()),
                    "arn:aws:iam::role/not-a-role-arn",
                ),
                "arn:aws:iam::role/not-a-role-arn",
            ),
            (
                "a role ARN shorter than its minimum",
                violation(
                    "dataSourceType.s3GlueDataCatalog.roleArn",
                    ViolationKind::MinLength(20),
                    "arn:aws:iam::role/x",
                ),
                "arn:aws:iam::role/x",
            ),
        ];
        for (case, role_arn_violation, role_arn) in rows {
            let refusal = Refusal::Invalid(vec![
                violation(
                    "dataSourceName",
                    ViolationKind::Pattern("^[a-z][a-z0-9_]+$".to_owned()),
                    "probe-BAD",
                ),
                role_arn_violation,
            ]);
            let stated = aws_validation_refusal(&refusal).expect("a refusal is stated");
            let expected = format!("Invalid Role Arn : {role_arn}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: a refusal that is no constraint failure at all keeps the framework's wording.
    #[test]
    fn a_refusal_that_is_not_a_constraint_keeps_the_frameworks_wording() {
        assert!(
            aws_validation_refusal(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            aws_validation_refusal(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }
}
